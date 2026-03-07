use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::signal;
use tokio::sync::watch;
use tracing::{error, info, warn};

use torsten_consensus::OuroborosPraos;
use torsten_ledger::LedgerState;
use torsten_mempool::{Mempool, MempoolConfig};
use torsten_network::server::NodeServerConfig;
use torsten_network::{ChainSyncEvent, NodeServer, NodeToNodeClient};
use torsten_primitives::block::Point;
use torsten_primitives::protocol_params::ProtocolParameters;
use torsten_storage::ChainDB;

use crate::config::NodeConfig;
use crate::topology::Topology;

pub struct NodeArgs {
    pub config: NodeConfig,
    pub topology: Topology,
    pub database_path: PathBuf,
    pub socket_path: PathBuf,
    pub host_addr: String,
    pub port: u16,
}

/// The main Torsten node
pub struct Node {
    config: NodeConfig,
    topology: Topology,
    chain_db: ChainDB,
    ledger_state: LedgerState,
    consensus: OuroborosPraos,
    mempool: Arc<Mempool>,
    #[allow(dead_code)]
    server: NodeServer,
}

impl Node {
    pub fn new(args: NodeArgs) -> Result<Self> {
        let chain_db = ChainDB::open(&args.database_path)?;
        info!("ChainDB opened at {}", args.database_path.display());

        let protocol_params = ProtocolParameters::mainnet_defaults();
        let ledger_state = LedgerState::new(protocol_params);
        info!("Ledger state initialized");

        let consensus = OuroborosPraos::new();
        info!("Ouroboros Praos consensus initialized");

        let mempool = Arc::new(Mempool::new(MempoolConfig::default()));
        info!("Mempool initialized");

        let server_config = NodeServerConfig {
            listen_addr: format!("{}:{}", args.host_addr, args.port).parse()?,
            socket_path: args.socket_path,
            max_connections: 200,
        };
        let server = NodeServer::new(server_config);

        Ok(Node {
            config: args.config,
            topology: args.topology,
            chain_db,
            ledger_state,
            consensus,
            mempool,
            server,
        })
    }

    pub async fn run(&mut self) -> Result<()> {
        let tip = self.chain_db.get_tip();
        info!("Current chain tip: {tip}");
        info!("UTxO set size: {} entries", self.ledger_state.utxo_set.len());
        info!("Mempool: {} transactions", self.mempool.len());

        // Setup shutdown signal
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        tokio::spawn(async move {
            signal::ctrl_c().await.ok();
            info!("Shutdown signal received");
            shutdown_tx.send(true).ok();
        });

        // Get all peers from topology
        let peers = self.topology.all_peers();
        if peers.is_empty() {
            warn!("No peers configured in topology");
            return Ok(());
        }

        let network_magic = self.config.network_magic.unwrap_or_else(|| self.config.network.magic());

        // Try each peer until we connect successfully
        let mut client = None;
        for (addr, port) in &peers {
            let target = format!("{addr}:{port}");
            info!("Attempting connection to {target}...");

            match NodeToNodeClient::connect(&*target, network_magic).await {
                Ok(c) => {
                    info!("Connected to {target}");
                    client = Some(c);
                    break;
                }
                Err(e) => {
                    warn!("Failed to connect to {target}: {e}");
                    continue;
                }
            }
        }

        let mut client = match client {
            Some(c) => c,
            None => {
                error!("Could not connect to any peer");
                return Ok(());
            }
        };

        // Run chain sync
        self.chain_sync_loop(&mut client, shutdown_rx).await?;

        // Clean shutdown
        client.disconnect().await;
        info!("Node shutdown complete");
        Ok(())
    }

    async fn chain_sync_loop(
        &mut self,
        client: &mut NodeToNodeClient,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> Result<()> {
        // Find intersection with our current chain
        let known_points = vec![self.chain_db.get_tip().point, Point::Origin];
        let (intersect, remote_tip) = client.find_intersect(known_points).await?;

        match &intersect {
            Some(point) => info!("Chain intersection found at {point}"),
            None => info!("Starting sync from Origin"),
        }
        info!("Remote tip: {remote_tip}");

        // Main sync loop
        let mut blocks_received: u64 = 0;
        let mut last_log_slot: u64 = 0;

        loop {
            // Check for shutdown
            if *shutdown_rx.borrow() {
                info!("Shutdown requested, stopping sync");
                break;
            }

            tokio::select! {
                result = client.request_next() => {
                    match result {
                        Ok(ChainSyncEvent::RollForward(block, tip)) => {
                            let slot = block.slot().0;
                            let block_no = block.block_number().0;
                            let tx_count = block.tx_count();

                            // Store the block
                            if let Err(e) = self.chain_db.add_block(
                                *block.hash(),
                                block.slot(),
                                block.block_number(),
                                *block.prev_hash(),
                                block.raw_cbor.clone().unwrap_or_default(),
                            ) {
                                error!("Failed to store block: {e}");
                            }

                            // Update consensus tip
                            self.consensus.update_tip(block.tip());

                            blocks_received += 1;

                            // Log progress periodically
                            if slot - last_log_slot >= 10000 || blocks_received <= 5 {
                                let tip_slot = tip.point.slot().map(|s| s.0).unwrap_or(0);
                                let progress = if tip_slot > 0 {
                                    (slot as f64 / tip_slot as f64 * 100.0).min(100.0)
                                } else {
                                    0.0
                                };
                                info!(
                                    slot,
                                    block_no,
                                    tx_count,
                                    blocks_received,
                                    progress = format!("{progress:.2}%"),
                                    "sync progress"
                                );
                                last_log_slot = slot;
                            }
                        }
                        Ok(ChainSyncEvent::RollBackward(point, tip)) => {
                            warn!("Rollback to {point}, tip: {tip}");
                            // TODO: Handle chain rollback in storage/ledger
                        }
                        Ok(ChainSyncEvent::Await) => {
                            info!(
                                blocks_received,
                                "Caught up to chain tip, awaiting new blocks"
                            );
                            // Continue the loop to wait for new blocks
                        }
                        Err(e) => {
                            error!("Chain sync error: {e}");
                            break;
                        }
                    }
                }
                _ = shutdown_rx.changed() => {
                    info!("Shutdown requested during sync");
                    break;
                }
            }
        }

        info!("Chain sync stopped after {blocks_received} blocks");
        Ok(())
    }
}
