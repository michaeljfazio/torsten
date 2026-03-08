use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::signal;
use tokio::sync::{watch, RwLock};
use tracing::{error, info, warn};

use torsten_consensus::OuroborosPraos;
use torsten_ledger::LedgerState;
use torsten_mempool::{Mempool, MempoolConfig};
use torsten_network::query_handler::{UtxoQueryProvider, UtxoSnapshot};
use torsten_network::server::NodeServerConfig;
use torsten_network::{
    ChainSyncEvent, N2CServer, NodeServer, NodeStateSnapshot, NodeToNodeClient, QueryHandler,
};
use torsten_primitives::block::Point;
use torsten_primitives::protocol_params::ProtocolParameters;
use torsten_storage::ChainDB;

use crate::config::NodeConfig;
use crate::genesis::{AlonzoGenesis, ConwayGenesis, ShelleyGenesis};
use crate::topology::Topology;

pub struct NodeArgs {
    pub config: NodeConfig,
    pub topology: Topology,
    pub database_path: PathBuf,
    pub socket_path: PathBuf,
    pub host_addr: String,
    pub port: u16,
}

/// Provides UTxO lookups from the live ledger state
struct LedgerUtxoProvider {
    ledger: Arc<RwLock<LedgerState>>,
}

impl UtxoQueryProvider for LedgerUtxoProvider {
    fn utxos_at_address_bytes(&self, addr_bytes: &[u8]) -> Vec<UtxoSnapshot> {
        let addr = match torsten_primitives::address::Address::from_bytes(addr_bytes) {
            Ok(a) => a,
            Err(_) => return vec![],
        };
        // Use try_read to avoid blocking — return empty if locked
        let ledger = match self.ledger.try_read() {
            Ok(l) => l,
            Err(_) => return vec![],
        };
        ledger
            .utxo_set
            .utxos_at_address(&addr)
            .into_iter()
            .map(|(input, output)| UtxoSnapshot {
                tx_hash: input.transaction_id.as_ref().to_vec(),
                output_index: input.index,
                address: hex::encode(addr_bytes),
                lovelace: output.value.coin.0,
                has_datum: output.datum != torsten_primitives::transaction::OutputDatum::None,
                has_script_ref: output.script_ref.is_some(),
            })
            .collect()
    }
}

/// The main Torsten node
pub struct Node {
    config: NodeConfig,
    topology: Topology,
    chain_db: ChainDB,
    ledger_state: Arc<RwLock<LedgerState>>,
    consensus: OuroborosPraos,
    mempool: Arc<Mempool>,
    #[allow(dead_code)]
    server: NodeServer,
    query_handler: Arc<RwLock<QueryHandler>>,
    socket_path: PathBuf,
    shelley_genesis: Option<ShelleyGenesis>,
}

impl Node {
    pub fn new(args: NodeArgs) -> Result<Self> {
        let chain_db = ChainDB::open(&args.database_path)?;
        info!("ChainDB opened at {}", args.database_path.display());

        let mut protocol_params = ProtocolParameters::mainnet_defaults();

        // Load Shelley genesis if configured (with hash for nonce initialization)
        let (shelley_genesis, shelley_genesis_hash) =
            if let Some(ref genesis_path) = args.config.shelley_genesis_file {
                let genesis_path = std::path::Path::new(genesis_path);
                match ShelleyGenesis::load_with_hash(genesis_path) {
                    Ok((genesis, hash)) => {
                        info!(
                            "Shelley genesis loaded: magic={}, system_start={}, epoch_length={}",
                            genesis.network_magic, genesis.system_start, genesis.epoch_length
                        );
                        genesis.apply_to_protocol_params(&mut protocol_params);
                        (Some(genesis), Some(hash))
                    }
                    Err(e) => {
                        warn!("Failed to load Shelley genesis: {e}");
                        (None, None)
                    }
                }
            } else {
                (None, None)
            };

        // Load Alonzo genesis if configured
        if let Some(ref genesis_path) = args.config.alonzo_genesis_file {
            let genesis_path = std::path::Path::new(genesis_path);
            match AlonzoGenesis::load(genesis_path) {
                Ok(genesis) => {
                    info!(
                        max_val_size = genesis.max_value_size,
                        collateral_pct = genesis.collateral_percentage,
                        max_tx_ex_mem = genesis.max_tx_ex_units.ex_units_mem,
                        "Alonzo genesis loaded"
                    );
                    genesis.apply_to_protocol_params(&mut protocol_params);
                }
                Err(e) => {
                    warn!("Failed to load Alonzo genesis: {e}");
                }
            }
        }

        // Load Conway genesis if configured
        if let Some(ref genesis_path) = args.config.conway_genesis_file {
            let genesis_path = std::path::Path::new(genesis_path);
            match ConwayGenesis::load(genesis_path) {
                Ok(genesis) => {
                    info!(
                        drep_deposit = genesis.d_rep_deposit,
                        gov_action_deposit = genesis.gov_action_deposit,
                        committee_min_size = genesis.committee_min_size,
                        "Conway genesis loaded"
                    );
                    genesis.apply_to_protocol_params(&mut protocol_params);
                }
                Err(e) => {
                    warn!("Failed to load Conway genesis: {e}");
                }
            }
        }

        let mut ledger = LedgerState::new(protocol_params);
        // Apply epoch length and genesis hash from Shelley genesis
        if let Some(ref genesis) = shelley_genesis {
            ledger.set_epoch_length(genesis.epoch_length, genesis.security_param);
        }
        if let Some(hash) = shelley_genesis_hash {
            ledger.set_genesis_hash(hash);
        }
        let ledger_state = Arc::new(RwLock::new(ledger));
        info!("Ledger state initialized");

        let consensus = if let Some(ref genesis) = shelley_genesis {
            OuroborosPraos::with_params(
                genesis.active_slots_coeff,
                genesis.security_param,
                torsten_primitives::time::EpochLength(genesis.epoch_length),
            )
        } else {
            OuroborosPraos::new()
        };
        info!(
            epoch_length = consensus.epoch_length.0,
            security_param = consensus.security_param,
            active_slot_coeff = consensus.active_slot_coeff,
            "Ouroboros Praos consensus initialized"
        );

        let mempool = Arc::new(Mempool::new(MempoolConfig::default()));
        info!("Mempool initialized");

        let socket_path = args.socket_path.clone();
        let server_config = NodeServerConfig {
            listen_addr: format!("{}:{}", args.host_addr, args.port).parse()?,
            socket_path: args.socket_path,
            max_connections: 200,
        };
        let server = NodeServer::new(server_config);

        // Wire up live UTxO provider before wrapping in lock
        let mut qh = QueryHandler::new();
        qh.set_utxo_provider(Arc::new(LedgerUtxoProvider {
            ledger: ledger_state.clone(),
        }));
        let query_handler = Arc::new(RwLock::new(qh));

        Ok(Node {
            config: args.config,
            topology: args.topology,
            chain_db,
            ledger_state,
            consensus,
            mempool,
            server,
            query_handler,
            socket_path,
            shelley_genesis,
        })
    }

    pub async fn run(&mut self) -> Result<()> {
        let tip = self.chain_db.get_tip();
        info!("Current chain tip: {tip}");
        {
            let ls = self.ledger_state.read().await;
            info!("UTxO set size: {} entries", ls.utxo_set.len());
        }
        info!("Mempool: {} transactions", self.mempool.len());

        // Setup shutdown signal
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        tokio::spawn(async move {
            signal::ctrl_c().await.ok();
            info!("Shutdown signal received");
            shutdown_tx.send(true).ok();
        });

        // Start N2C server on Unix socket
        let n2c_server = N2CServer::new(self.query_handler.clone(), self.mempool.clone());
        let n2c_socket_path = self.socket_path.clone();
        tokio::spawn(async move {
            if let Err(e) = n2c_server.listen(&n2c_socket_path).await {
                error!("N2C server error: {e}");
            }
        });

        // Get all peers from topology
        let peers = self.topology.all_peers();
        if peers.is_empty() {
            warn!("No peers configured in topology");
            return Ok(());
        }

        let network_magic = self
            .config
            .network_magic
            .unwrap_or_else(|| self.config.network.magic());

        // Main connection loop with reconnection support
        let mut retry_count = 0u32;
        let max_retries = 100; // effectively unlimited retries
        let base_delay_secs = 5u64;
        let max_delay_secs = 60u64;

        loop {
            if *shutdown_rx.borrow() {
                break;
            }

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
                Some(c) => {
                    retry_count = 0; // Reset on successful connection
                    c
                }
                None => {
                    retry_count += 1;
                    if retry_count > max_retries {
                        error!("Exhausted connection retries");
                        break;
                    }
                    let delay = base_delay_secs
                        .saturating_mul(2u64.saturating_pow(retry_count.min(4)))
                        .min(max_delay_secs);
                    warn!(
                        retry_count,
                        delay_secs = delay,
                        "Could not connect to any peer, retrying..."
                    );
                    tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_secs(delay)) => {}
                        _ = shutdown_rx.changed() => { break; }
                    }
                    continue;
                }
            };

            // Run chain sync — returns when disconnected or error
            let sync_shutdown = shutdown_rx.clone();
            match self.chain_sync_loop(&mut client, sync_shutdown).await {
                Ok(()) => {
                    client.disconnect().await;
                    if *shutdown_rx.borrow() {
                        break; // Clean shutdown
                    }
                    // Sync ended without shutdown — likely peer disconnected
                    info!("Sync ended, will reconnect...");
                }
                Err(e) => {
                    warn!("Sync error: {e}, will reconnect...");
                }
            }

            // Brief delay before reconnecting
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
                _ = shutdown_rx.changed() => { break; }
            }
        }

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

        // Main sync loop — fetch blocks in batches for faster sync
        let mut blocks_received: u64 = 0;
        let mut last_log_slot: u64 = 0;
        let batch_size = 100;

        loop {
            // Check for shutdown
            if *shutdown_rx.borrow() {
                info!("Shutdown requested, stopping sync");
                break;
            }

            tokio::select! {
                result = client.request_next_batch(batch_size) => {
                    match result {
                        Ok(events) => {
                            for event in events {
                                match event {
                                    ChainSyncEvent::RollForward(block, tip) => {
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

                                        // Validate block header against consensus rules
                                        // (skip for Byron-era blocks which have different structure)
                                        if block.era.is_shelley_based() {
                                            if let Err(e) = self.consensus.validate_header(
                                                &block.header,
                                                block.slot(), // accept the block's own slot during sync
                                            ) {
                                                warn!(slot, "Consensus validation warning: {e}");
                                            }
                                        }

                                        // Apply block to ledger state
                                        {
                                            let mut ls = self.ledger_state.write().await;
                                            if let Err(e) = ls.apply_block(&block) {
                                                error!("Failed to apply block to ledger: {e}");
                                            }
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
                                            {
                                                let ls = self.ledger_state.read().await;
                                                let utxo_count = ls.utxo_set.len();
                                                let epoch = ls.epoch.0;
                                                info!(
                                                    slot,
                                                    block_no,
                                                    tx_count,
                                                    blocks_received,
                                                    utxo_count,
                                                    epoch,
                                                    progress = format!("{progress:.2}%"),
                                                    "sync progress"
                                                );
                                            }
                                            last_log_slot = slot;
                                            // Update N2C query handler with latest state
                                            self.update_query_state().await;
                                        }
                                    }
                                    ChainSyncEvent::RollBackward(point, tip) => {
                                        warn!("Rollback to {point}, tip: {tip}");
                                        if let Err(e) = self.chain_db.rollback_to_point(&point) {
                                            error!("Rollback failed: {e}");
                                        }
                                    }
                                    ChainSyncEvent::Await => {
                                        info!(
                                            blocks_received,
                                            "Caught up to chain tip, awaiting new blocks"
                                        );
                                    }
                                }
                            }
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

    /// Update the query handler with the current ledger state
    async fn update_query_state(&self) {
        use torsten_network::query_handler::{
            CommitteeMemberSnapshot, CommitteeSnapshot, DRepSnapshot, ProposalSnapshot,
            StakeAddressSnapshot, StakePoolSnapshot,
        };

        let ls = self.ledger_state.read().await;

        // Build stake pool snapshots
        let stake_pools: Vec<StakePoolSnapshot> = ls
            .pool_params
            .iter()
            .map(|(pool_id, reg)| StakePoolSnapshot {
                pool_id: pool_id.as_ref().to_vec(),
                stake: ls
                    .stake_distribution
                    .stake_map
                    .values()
                    .map(|l| l.0)
                    .sum::<u64>()
                    / ls.pool_params.len().max(1) as u64, // approximate per-pool
                pledge: reg.pledge.0,
                cost: reg.cost.0,
                margin_num: reg.margin_numerator,
                margin_den: reg.margin_denominator,
            })
            .collect();

        // Build DRep snapshots
        let drep_entries: Vec<DRepSnapshot> = ls
            .governance
            .dreps
            .iter()
            .map(|(hash, drep)| DRepSnapshot {
                credential_hash: hash.as_ref().to_vec(),
                deposit: drep.deposit.0,
                anchor_url: drep.anchor.as_ref().map(|a| a.url.clone()),
                registered_epoch: drep.registered_epoch.0,
            })
            .collect();

        // Build governance proposal snapshots
        let governance_proposals: Vec<ProposalSnapshot> = ls
            .governance
            .proposals
            .iter()
            .map(|(action_id, state)| {
                let action_type = match &state.procedure.gov_action {
                    torsten_primitives::transaction::GovAction::ParameterChange { .. } => {
                        "ParameterChange"
                    }
                    torsten_primitives::transaction::GovAction::HardForkInitiation { .. } => {
                        "HardForkInitiation"
                    }
                    torsten_primitives::transaction::GovAction::TreasuryWithdrawals { .. } => {
                        "TreasuryWithdrawals"
                    }
                    torsten_primitives::transaction::GovAction::NoConfidence { .. } => {
                        "NoConfidence"
                    }
                    torsten_primitives::transaction::GovAction::UpdateCommittee { .. } => {
                        "UpdateCommittee"
                    }
                    torsten_primitives::transaction::GovAction::NewConstitution { .. } => {
                        "NewConstitution"
                    }
                    torsten_primitives::transaction::GovAction::InfoAction => "InfoAction",
                };
                ProposalSnapshot {
                    tx_id: action_id.transaction_id.as_ref().to_vec(),
                    action_index: action_id.action_index,
                    action_type: action_type.to_string(),
                    proposed_epoch: state.proposed_epoch.0,
                    expires_epoch: state.expires_epoch.0,
                    yes_votes: state.yes_votes,
                    no_votes: state.no_votes,
                    abstain_votes: state.abstain_votes,
                }
            })
            .collect();

        // Build committee snapshot
        let committee = CommitteeSnapshot {
            members: ls
                .governance
                .committee_hot_keys
                .iter()
                .map(|(cold, hot)| CommitteeMemberSnapshot {
                    cold_credential: cold.as_ref().to_vec(),
                    hot_credential: hot.as_ref().to_vec(),
                })
                .collect(),
            resigned: ls
                .governance
                .committee_resigned
                .keys()
                .map(|k| k.as_ref().to_vec())
                .collect(),
        };

        // Build stake address snapshots (delegations + rewards)
        let stake_addresses: Vec<StakeAddressSnapshot> = ls
            .reward_accounts
            .iter()
            .map(|(cred_hash, rewards)| {
                let delegated_pool = ls
                    .delegations
                    .get(cred_hash)
                    .map(|pool_id| pool_id.as_ref().to_vec());
                StakeAddressSnapshot {
                    credential_hash: cred_hash.as_ref().to_vec(),
                    delegated_pool,
                    reward_balance: rewards.0,
                }
            })
            .collect();

        // Serialize protocol params
        let protocol_params_json =
            serde_json::to_string_pretty(&ls.protocol_params).unwrap_or_default();

        let snapshot = NodeStateSnapshot {
            tip: ls.tip.clone(),
            epoch: ls.epoch,
            era: ls.era.to_era_index(),
            block_number: ls.current_block_number(),
            system_start: self
                .shelley_genesis
                .as_ref()
                .map(|g| g.system_start.clone())
                .unwrap_or_else(|| self.config.network.system_start().to_string()),
            utxo_count: ls.utxo_set.len(),
            delegations_count: ls.delegations.len(),
            pool_count: ls.pool_params.len(),
            treasury: ls.treasury.0,
            reserves: ls.reserves.0,
            drep_count: ls.governance.dreps.len(),
            proposal_count: ls.governance.proposals.len(),
            protocol_params_json,
            stake_pools,
            drep_entries,
            governance_proposals,
            committee,
            stake_addresses,
        };

        // Drop the ledger read lock before acquiring the query handler write lock
        drop(ls);

        let mut handler = self.query_handler.write().await;
        handler.update_state(snapshot);
    }
}
