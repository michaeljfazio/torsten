use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::signal;
use tracing::info;

use torsten_consensus::OuroborosPraos;
use torsten_ledger::LedgerState;
use torsten_mempool::{Mempool, MempoolConfig};
use torsten_network::{NodeServer, server::NodeServerConfig};
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
#[allow(dead_code)]
pub struct Node {
    config: NodeConfig,
    topology: Topology,
    chain_db: ChainDB,
    ledger_state: LedgerState,
    consensus: OuroborosPraos,
    mempool: Arc<Mempool>,
    server: NodeServer,
}

impl Node {
    pub fn new(args: NodeArgs) -> Result<Self> {
        // Initialize storage
        let chain_db = ChainDB::open(&args.database_path)?;
        info!("ChainDB opened at {}", args.database_path.display());

        // Initialize ledger state
        let protocol_params = ProtocolParameters::mainnet_defaults();
        let ledger_state = LedgerState::new(protocol_params);
        info!("Ledger state initialized");

        // Initialize consensus
        let consensus = OuroborosPraos::new();
        info!("Ouroboros Praos consensus initialized");

        // Initialize mempool
        let mempool = Arc::new(Mempool::new(MempoolConfig::default()));
        info!("Mempool initialized");

        // Initialize network server
        let server_config = NodeServerConfig {
            listen_addr: format!("{}:{}", args.host_addr, args.port).parse()?,
            socket_path: args.socket_path,
            max_connections: 200,
        };
        let server = NodeServer::new(server_config);
        info!("Network server configured");

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
        info!("Current chain tip: {}", tip);
        info!(
            "UTxO set size: {} entries",
            self.ledger_state.utxo_set.len()
        );
        info!("Mempool: {} transactions", self.mempool.len());

        let peers = self.topology.all_peers();
        info!("Connecting to {} peers...", peers.len());

        for (addr, port) in &peers {
            info!("  -> {}:{}", addr, port);
        }

        info!("Torsten node is running. Press Ctrl+C to stop.");

        // Wait for shutdown signal
        signal::ctrl_c().await?;

        info!("Shutting down gracefully...");
        Ok(())
    }
}
