use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::signal;
use tokio::sync::{watch, RwLock};
use tracing::{debug, error, info, warn};

use torsten_consensus::OuroborosPraos;
use torsten_ledger::LedgerState;
use torsten_mempool::{Mempool, MempoolConfig};
use torsten_network::query_handler::{UtxoQueryProvider, UtxoSnapshot};
use torsten_network::server::NodeServerConfig;
use torsten_network::{
    BlockFetchPool, BlockProvider, ChainSyncEvent, DiffusionMode, HeaderBatchResult, N2CServer,
    NodeServer, NodeStateSnapshot, NodeToNodeClient, PeerManager, PeerManagerConfig,
    PipelinedPeerClient, QueryHandler, TxValidator,
};
use torsten_primitives::block::Point;
use torsten_primitives::protocol_params::ProtocolParameters;
use torsten_storage::ChainDB;

use crate::config::NodeConfig;
use crate::genesis::{AlonzoGenesis, ByronGenesis, ConwayGenesis, ShelleyGenesis};
use crate::topology::Topology;

pub struct NodeArgs {
    pub config: NodeConfig,
    pub topology: Topology,
    pub topology_path: PathBuf,
    pub database_path: PathBuf,
    pub socket_path: PathBuf,
    pub host_addr: String,
    pub port: u16,
    /// Directory containing the config file (for resolving relative genesis paths)
    pub config_dir: PathBuf,
    /// Path to KES signing key (enables block production)
    pub shelley_kes_key: Option<PathBuf>,
    /// Path to VRF signing key (enables block production)
    pub shelley_vrf_key: Option<PathBuf>,
    /// Path to operational certificate (enables block production)
    pub shelley_operational_certificate: Option<PathBuf>,
    /// Path to cold signing key (required for block production)
    pub shelley_cold_key: Option<PathBuf>,
    /// Prometheus metrics port (0 to disable)
    pub metrics_port: u16,
}

/// Provides block data from ChainDB for the N2N server
struct ChainDBBlockProvider {
    chain_db: Arc<RwLock<ChainDB>>,
}

impl BlockProvider for ChainDBBlockProvider {
    fn get_block(&self, hash: &[u8; 32]) -> Option<Vec<u8>> {
        let block_hash = torsten_primitives::hash::Hash32::from_bytes(*hash);
        let db = self.chain_db.try_read().ok()?;
        db.get_block(&block_hash).ok().flatten()
    }

    fn has_block(&self, hash: &[u8; 32]) -> bool {
        let block_hash = torsten_primitives::hash::Hash32::from_bytes(*hash);
        match self.chain_db.try_read() {
            Ok(db) => db.has_block(&block_hash),
            Err(_) => false,
        }
    }

    fn get_tip(&self) -> (u64, [u8; 32], u64) {
        match self.chain_db.try_read() {
            Ok(db) => {
                let tip = db.get_tip();
                let slot = tip.point.slot().map(|s| s.0).unwrap_or(0);
                let hash = tip
                    .point
                    .hash()
                    .map(|h| {
                        let bytes: &[u8] = h.as_ref();
                        let mut arr = [0u8; 32];
                        arr.copy_from_slice(bytes);
                        arr
                    })
                    .unwrap_or([0u8; 32]);
                let block_no = tip.block_number.0;
                (slot, hash, block_no)
            }
            Err(_) => (0, [0u8; 32], 0),
        }
    }

    fn get_next_block_after_slot(&self, after_slot: u64) -> Option<(u64, [u8; 32], Vec<u8>)> {
        let db = self.chain_db.try_read().ok()?;
        let slot = torsten_primitives::time::SlotNo(after_slot);
        match db.get_next_block_after_slot(slot) {
            Ok(Some((s, hash, cbor))) => {
                let mut hash_arr = [0u8; 32];
                hash_arr.copy_from_slice(hash.as_bytes());
                Some((s.0, hash_arr, cbor))
            }
            _ => None,
        }
    }
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
            .map(|(input, output)| utxo_to_snapshot(input, output))
            .collect()
    }

    fn utxos_by_tx_inputs(&self, inputs: &[(Vec<u8>, u32)]) -> Vec<UtxoSnapshot> {
        let ledger = match self.ledger.try_read() {
            Ok(l) => l,
            Err(_) => return vec![],
        };
        let mut results = Vec::new();
        for (tx_hash_bytes, idx) in inputs {
            if tx_hash_bytes.len() == 32 {
                let mut hash_arr = [0u8; 32];
                hash_arr.copy_from_slice(tx_hash_bytes);
                let tx_input = torsten_primitives::transaction::TransactionInput {
                    transaction_id: torsten_primitives::hash::Hash32::from_bytes(hash_arr),
                    index: *idx,
                };
                if let Some(output) = ledger.utxo_set.lookup(&tx_input) {
                    results.push(utxo_to_snapshot(&tx_input, output));
                }
            }
        }
        results
    }
}

/// Convert a UTxO entry to a snapshot for N2C queries
fn utxo_to_snapshot(
    input: &torsten_primitives::transaction::TransactionInput,
    output: &torsten_primitives::transaction::TransactionOutput,
) -> UtxoSnapshot {
    let multi_asset: torsten_network::query_handler::MultiAssetSnapshot = output
        .value
        .multi_asset
        .iter()
        .map(|(policy, assets)| {
            let assets_vec: Vec<(Vec<u8>, u64)> = assets
                .iter()
                .map(|(name, qty)| (name.0.clone(), *qty))
                .collect();
            (policy.as_ref().to_vec(), assets_vec)
        })
        .collect();

    let datum_hash = match &output.datum {
        torsten_primitives::transaction::OutputDatum::DatumHash(h) => Some(h.as_ref().to_vec()),
        _ => None,
    };

    UtxoSnapshot {
        tx_hash: input.transaction_id.as_ref().to_vec(),
        output_index: input.index,
        address_bytes: output.address.to_bytes(),
        lovelace: output.value.coin.0,
        multi_asset,
        datum_hash,
        raw_cbor: output.raw_cbor.clone(),
    }
}

/// Validates transactions against the live ledger state (Phase-1 + Phase-2 Plutus)
struct LedgerTxValidator {
    ledger: Arc<RwLock<LedgerState>>,
    slot_config: torsten_ledger::plutus::SlotConfig,
}

impl TxValidator for LedgerTxValidator {
    fn validate_tx(&self, era_id: u16, tx_bytes: &[u8]) -> Result<(), String> {
        let tx = torsten_serialization::decode_transaction(era_id, tx_bytes)
            .map_err(|e| format!("Failed to decode transaction: {e}"))?;

        let ledger = self.ledger.try_read().map_err(|_| "Ledger state busy")?;
        let tx_size = tx_bytes.len() as u64;
        let current_slot = ledger.tip.point.slot().map(|s| s.0).unwrap_or(0);

        torsten_ledger::validation::validate_transaction(
            &tx,
            &ledger.utxo_set,
            &ledger.protocol_params,
            current_slot,
            tx_size,
            Some(&self.slot_config),
        )
        .map_err(|errors| {
            errors
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("; ")
        })
    }
}

/// The main Torsten node
pub struct Node {
    config: NodeConfig,
    topology: Topology,
    chain_db: Arc<RwLock<ChainDB>>,
    ledger_state: Arc<RwLock<LedgerState>>,
    consensus: OuroborosPraos,
    mempool: Arc<Mempool>,
    #[allow(dead_code)]
    server: NodeServer,
    query_handler: Arc<RwLock<QueryHandler>>,
    peer_manager: Arc<RwLock<PeerManager>>,
    socket_path: PathBuf,
    database_path: PathBuf,
    listen_addr: std::net::SocketAddr,
    network_magic: u64,
    shelley_genesis: Option<ShelleyGenesis>,
    topology_path: PathBuf,
    metrics: Arc<crate::metrics::NodeMetrics>,
    /// Block producer credentials (None = relay-only mode)
    block_producer: Option<crate::forge::BlockProducerCredentials>,
    /// Broadcast sender for announcing forged blocks to connected peers
    block_announcement_tx:
        Option<tokio::sync::broadcast::Sender<torsten_network::BlockAnnouncement>>,
    /// Broadcast sender for notifying connected peers of chain rollbacks
    rollback_announcement_tx:
        Option<tokio::sync::broadcast::Sender<torsten_network::RollbackAnnouncement>>,
    /// Prometheus metrics port
    metrics_port: u16,
}

impl Node {
    pub fn new(args: NodeArgs) -> Result<Self> {
        let chain_db = Arc::new(RwLock::new(ChainDB::open(&args.database_path)?));
        info!("ChainDB opened at {}", args.database_path.display());

        let mut protocol_params = ProtocolParameters::mainnet_defaults();

        // Load Byron genesis if configured
        let config_dir = args.config_dir.clone();
        if let Some(ref genesis_path) = args.config.byron_genesis_file {
            let genesis_path = config_dir.join(genesis_path);
            match ByronGenesis::load(&genesis_path) {
                Ok(genesis) => {
                    let utxos = genesis.initial_utxos();
                    info!(
                        protocol_magic = genesis.protocol_magic(),
                        security_param = genesis.security_param(),
                        initial_utxos = utxos.len(),
                        "Byron genesis loaded"
                    );
                }
                Err(e) => {
                    warn!("Failed to load Byron genesis: {e}");
                }
            }
        }

        // Load Shelley genesis if configured (with hash for nonce initialization)
        let (shelley_genesis, shelley_genesis_hash) =
            if let Some(ref genesis_path) = args.config.shelley_genesis_file {
                let genesis_path = config_dir.join(genesis_path);
                match ShelleyGenesis::load_with_hash(&genesis_path) {
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
            let genesis_path = config_dir.join(genesis_path);
            match AlonzoGenesis::load(&genesis_path) {
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
            let genesis_path = config_dir.join(genesis_path);
            match ConwayGenesis::load(&genesis_path) {
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

        // Try to load existing ledger snapshot
        let snapshot_path = args.database_path.join("ledger-snapshot.bin");
        let ledger = if snapshot_path.exists() {
            match LedgerState::load_snapshot(&snapshot_path) {
                Ok(mut state) => {
                    // Re-apply genesis config in case it changed
                    if let Some(ref genesis) = shelley_genesis {
                        state.epoch_length = genesis.epoch_length;
                        state.set_slot_config(genesis.slot_config());
                    }
                    if let Some(hash) = shelley_genesis_hash {
                        state.genesis_hash = hash;
                    }
                    info!(
                        epoch = state.epoch.0,
                        utxo_count = state.utxo_set.len(),
                        tip = %state.tip,
                        "Ledger state restored from snapshot"
                    );
                    state
                }
                Err(e) => {
                    warn!("Failed to load ledger snapshot, starting fresh: {e}");
                    let mut ledger = LedgerState::new(protocol_params.clone());
                    if let Some(ref genesis) = shelley_genesis {
                        ledger.set_epoch_length(genesis.epoch_length, genesis.security_param);
                        ledger.set_slot_config(genesis.slot_config());
                    }
                    if let Some(hash) = shelley_genesis_hash {
                        ledger.set_genesis_hash(hash);
                    }
                    ledger
                }
            }
        } else {
            let mut ledger = LedgerState::new(protocol_params.clone());
            // Apply epoch length and genesis hash from Shelley genesis
            if let Some(ref genesis) = shelley_genesis {
                ledger.set_epoch_length(genesis.epoch_length, genesis.security_param);
                ledger.set_slot_config(genesis.slot_config());
            }
            if let Some(hash) = shelley_genesis_hash {
                ledger.set_genesis_hash(hash);
            }
            ledger
        };
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
        let listen_addr: std::net::SocketAddr =
            format!("{}:{}", args.host_addr, args.port).parse()?;
        let network_magic = args
            .config
            .network_magic
            .unwrap_or_else(|| args.config.network.magic());
        let server_config = NodeServerConfig {
            listen_addr,
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

        // Load block producer credentials if all key paths are provided
        let block_producer = match (
            &args.shelley_vrf_key,
            &args.shelley_kes_key,
            &args.shelley_operational_certificate,
            &args.shelley_cold_key,
        ) {
            (Some(vrf_path), Some(kes_path), Some(opcert_path), Some(cold_key_path)) => {
                match crate::forge::BlockProducerCredentials::load_with_cold_key(
                    vrf_path,
                    kes_path,
                    opcert_path,
                    cold_key_path,
                ) {
                    Ok(creds) => {
                        info!(
                            pool_id = %creds.pool_id,
                            opcert_seq = creds.opcert_sequence,
                            kes_period = creds.opcert_kes_period,
                            "Block producer mode enabled"
                        );
                        Some(creds)
                    }
                    Err(e) => {
                        warn!("Failed to load block producer credentials: {e}");
                        None
                    }
                }
            }
            (Some(_), Some(_), Some(_), None) => {
                warn!(
                    "Block producer keys provided but --shelley-cold-key is missing. \
                     Running in relay-only mode."
                );
                None
            }
            _ => {
                info!("Running in relay-only mode (no block producer keys configured)");
                None
            }
        };

        Ok(Node {
            config: args.config,
            topology: args.topology,
            chain_db,
            ledger_state,
            consensus,
            mempool,
            server,
            query_handler,
            peer_manager: Arc::new(RwLock::new(PeerManager::new(PeerManagerConfig::default()))),
            socket_path,
            database_path: args.database_path,
            listen_addr,
            network_magic,
            shelley_genesis,
            topology_path: args.topology_path,
            metrics: Arc::new(crate::metrics::NodeMetrics::new()),
            block_producer,
            block_announcement_tx: None,
            rollback_announcement_tx: None,
            metrics_port: args.metrics_port,
        })
    }

    pub async fn run(&mut self) -> Result<()> {
        let tip = self.chain_db.read().await.get_tip();
        info!("Current chain tip: {tip}");
        {
            let ls = self.ledger_state.read().await;
            info!("UTxO set size: {} entries", ls.utxo_set.len());
        }
        info!("Mempool: {} transactions", self.mempool.len());

        // Replay blocks from ChainDB if the ledger is behind storage
        // This happens after a Mithril snapshot import — blocks are in storage
        // but the ledger hasn't processed them yet.
        self.replay_ledger_from_storage().await;

        // Setup shutdown signal
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        tokio::spawn(async move {
            signal::ctrl_c().await.ok();
            info!("Shutdown signal received");
            shutdown_tx.send(true).ok();
        });

        // SIGHUP handler is set up after peer_manager initialization below

        // Start Prometheus metrics server
        if self.metrics_port > 0 {
            let metrics = self.metrics.clone();
            let port = self.metrics_port;
            tokio::spawn(async move {
                crate::metrics::start_metrics_server(port, metrics).await;
            });
        }

        // Start N2C server on Unix socket
        let mut n2c_server = N2CServer::new(self.query_handler.clone(), self.mempool.clone());
        let slot_config = self.ledger_state.read().await.slot_config;
        n2c_server.set_tx_validator(Arc::new(LedgerTxValidator {
            ledger: self.ledger_state.clone(),
            slot_config,
        }));
        n2c_server.set_block_provider(Arc::new(ChainDBBlockProvider {
            chain_db: self.chain_db.clone(),
        }));
        info!("N2C server: Plutus tx validation and block delivery enabled");
        let n2c_socket_path = self.socket_path.clone();
        let n2c_shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            if let Err(e) = n2c_server.listen(&n2c_socket_path, n2c_shutdown_rx).await {
                error!("N2C server error: {e}");
            }
        });

        // Initialize peer manager
        {
            let pm_config = PeerManagerConfig {
                diffusion_mode: DiffusionMode::InitiatorAndResponder,
                peer_sharing_enabled: true,
                ..PeerManagerConfig::default()
            };
            *self.peer_manager.write().await = PeerManager::new(pm_config);
        }
        let peer_manager = self.peer_manager.clone();

        // Register topology peers in the peer manager with full metadata
        let detailed_peers = self.topology.detailed_peers();
        if detailed_peers.is_empty() {
            warn!("No peers configured in topology");
            return Ok(());
        }
        {
            let mut pm = peer_manager.write().await;
            for peer in &detailed_peers {
                // Resolve address to SocketAddr — register ALL resolved IPs
                if let Ok(addrs) =
                    tokio::net::lookup_host(format!("{}:{}", peer.address, peer.port)).await
                {
                    for socket_addr in addrs {
                        pm.add_config_peer(socket_addr, peer.trustable, peer.advertise);
                    }
                }
            }
            let stats = pm.stats();
            info!(
                "Peer manager initialized: {} known peers, mode={:?}",
                stats.known_peers,
                pm.diffusion_mode()
            );
        }
        let peers = self.topology.all_peers();

        // Setup SIGHUP handler for topology reload
        #[cfg(unix)]
        {
            let topology_path = self.topology_path.clone();
            let pm_for_sighup = peer_manager.clone();
            tokio::spawn(async move {
                let mut hup = match signal::unix::signal(signal::unix::SignalKind::hangup()) {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("Failed to setup SIGHUP handler: {e}");
                        return;
                    }
                };
                loop {
                    hup.recv().await;
                    info!(
                        "SIGHUP received — reloading topology from {}",
                        topology_path.display()
                    );
                    match Topology::load(&topology_path) {
                        Ok(new_topology) => {
                            let new_peers = new_topology.detailed_peers();
                            let mut pm = pm_for_sighup.write().await;
                            let mut added = 0usize;
                            for peer in &new_peers {
                                if let Ok(addrs) = tokio::net::lookup_host(format!(
                                    "{}:{}",
                                    peer.address, peer.port
                                ))
                                .await
                                {
                                    for socket_addr in addrs {
                                        pm.add_config_peer(
                                            socket_addr,
                                            peer.trustable,
                                            peer.advertise,
                                        );
                                        added += 1;
                                    }
                                }
                            }
                            info!(
                                "Topology reloaded: {added} peers registered, {}",
                                pm.stats()
                            );
                        }
                        Err(e) => {
                            error!("Failed to reload topology: {e}");
                        }
                    }
                }
            });
        }

        // Start N2N server for inbound peer connections (bidirectional mode)
        let mut n2n_server = torsten_network::n2n_server::N2NServer::with_config(
            self.listen_addr,
            self.network_magic,
            self.query_handler.clone(),
            Arc::new(ChainDBBlockProvider {
                chain_db: self.chain_db.clone(),
            }),
            200,
            self.peer_manager.read().await.diffusion_mode() == DiffusionMode::InitiatorAndResponder,
            torsten_network::n2n_server::PeerSharingMode::PeerSharingEnabled,
        );
        n2n_server.set_mempool(self.mempool.clone());
        n2n_server.set_peer_manager(self.peer_manager.clone());
        // Get the broadcast senders before spawning the server
        self.block_announcement_tx = Some(n2n_server.block_announcement_sender());
        self.rollback_announcement_tx = Some(n2n_server.rollback_announcement_sender());
        info!(
            "N2N server: diffusion_mode={:?}, peer_sharing=enabled",
            self.peer_manager.read().await.diffusion_mode()
        );
        let n2n_shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            if let Err(e) = n2n_server.listen(n2n_shutdown_rx).await {
                error!("N2N server error: {e}");
            }
        });

        // Start ledger-based peer discovery task
        {
            let ledger = self.ledger_state.clone();
            let pm = peer_manager.clone();
            let topology = self.topology.clone();
            let shutdown = shutdown_rx.clone();
            tokio::spawn(async move {
                // Check every 5 minutes for new ledger peers
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
                interval.tick().await; // skip first immediate tick
                let mut shutdown = shutdown;
                loop {
                    tokio::select! {
                        _ = interval.tick() => {}
                        _ = shutdown.changed() => { break; }
                    }

                    let current_slot = {
                        let ls = ledger.read().await;
                        ls.tip.point.slot().map(|s| s.0).unwrap_or(0)
                    };

                    if !topology.ledger_peers_enabled(current_slot) {
                        continue;
                    }

                    // Extract relay addresses from registered pools
                    let relays: Vec<(String, u16)> = {
                        let ls = ledger.read().await;
                        let mut relays = Vec::new();
                        for pool_reg in ls.pool_params.values() {
                            for relay in &pool_reg.relays {
                                match relay {
                                    torsten_primitives::transaction::Relay::SingleHostAddr {
                                        port,
                                        ipv4,
                                        ..
                                    } => {
                                        if let (Some(port), Some(ipv4)) = (port, ipv4) {
                                            let addr = format!(
                                                "{}.{}.{}.{}",
                                                ipv4[0], ipv4[1], ipv4[2], ipv4[3]
                                            );
                                            relays.push((addr, *port));
                                        }
                                    }
                                    torsten_primitives::transaction::Relay::SingleHostName {
                                        port,
                                        dns_name,
                                    } => {
                                        if let Some(port) = port {
                                            relays.push((dns_name.clone(), *port));
                                        }
                                    }
                                    torsten_primitives::transaction::Relay::MultiHostName {
                                        dns_name,
                                    } => {
                                        relays.push((dns_name.clone(), 3001));
                                    }
                                }
                            }
                        }
                        relays
                    };

                    if relays.is_empty() {
                        continue;
                    }

                    // Sample a subset of ledger peers
                    // (don't try to resolve all thousands of pool relays)
                    let sample_size = 20.min(relays.len());
                    let step = relays.len() / sample_size;
                    let offset = (current_slot as usize) % step.max(1);
                    let sample: Vec<_> = relays
                        .iter()
                        .skip(offset)
                        .step_by(step.max(1))
                        .take(sample_size)
                        .collect();

                    let mut added = 0u32;
                    for (host, port) in sample {
                        if let Ok(mut addrs) =
                            tokio::net::lookup_host(format!("{host}:{port}")).await
                        {
                            if let Some(socket_addr) = addrs.next() {
                                let mut pm_w = pm.write().await;
                                pm_w.add_ledger_peer(socket_addr);
                                added += 1;
                            }
                        }
                    }
                    if added > 0 {
                        let pm_r = pm.read().await;
                        info!(
                            "Ledger peer discovery: added {added} peers from {} pool relays (slot {current_slot}), {}",
                            relays.len(),
                            pm_r.stats()
                        );
                    }
                }
            });
        }

        let network_magic = self.network_magic;

        // Main connection loop — connect to peers and sync
        let mut retry_count = 0u32;
        let base_delay_secs = 5u64;
        let max_delay_secs = 60u64;

        loop {
            if *shutdown_rx.borrow() {
                break;
            }

            // Get peers to connect to from the peer manager
            let targets: Vec<std::net::SocketAddr> = {
                let pm = peer_manager.read().await;
                pm.peers_to_connect()
            };

            // If peer manager has no targets, fall back to topology list
            let mut client = None;
            if !targets.is_empty() {
                for addr in &targets {
                    let target = addr.to_string();
                    info!("Connecting to peer {target}...");
                    let connect_start = std::time::Instant::now();
                    match NodeToNodeClient::connect(&*target, network_magic).await {
                        Ok(c) => {
                            let rtt_ms = connect_start.elapsed().as_secs_f64() * 1000.0;
                            let mut pm = peer_manager.write().await;
                            pm.peer_connected(addr, 14, true);
                            pm.record_handshake_rtt(addr, rtt_ms);
                            pm.promote_to_hot(addr);
                            drop(pm);
                            info!("Connected to {target} (handshake {rtt_ms:.0}ms)");
                            client = Some((c, *addr));
                            break;
                        }
                        Err(e) => {
                            peer_manager.write().await.peer_failed(addr);
                            warn!("Failed to connect to {target}: {e}");
                        }
                    }
                }
            } else {
                // Fallback: try topology peers directly
                for (addr, port) in &peers {
                    let target = format!("{addr}:{port}");
                    info!("Connecting to peer {target}...");
                    match NodeToNodeClient::connect(&*target, network_magic).await {
                        Ok(c) => {
                            info!("Connected to {target}");
                            let sock_addr = c.remote_addr().to_owned();
                            client = Some((c, sock_addr));
                            break;
                        }
                        Err(e) => {
                            warn!("Failed to connect to {target}: {e}");
                        }
                    }
                }
            }

            let (mut active_client, peer_addr) = match client {
                Some(c) => {
                    retry_count = 0;
                    c
                }
                None => {
                    retry_count += 1;
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

            // Log peer manager state
            {
                let pm = peer_manager.read().await;
                info!("P2P: {}", pm.stats());
            }

            // Spawn PeerSharing client: request peers from connected peer in background
            {
                let ps_peer_addr = peer_addr;
                let ps_network_magic = network_magic;
                let ps_peer_manager = peer_manager.clone();
                tokio::spawn(async move {
                    match torsten_network::request_peers_from(
                        ps_peer_addr.to_string().as_str(),
                        ps_network_magic,
                        10,
                    )
                    .await
                    {
                        Ok(peers) => {
                            if peers.is_empty() {
                                debug!("PeerSharing: no peers received from {ps_peer_addr}");
                            } else {
                                info!(
                                    "PeerSharing: received {} peers from {ps_peer_addr}",
                                    peers.len()
                                );
                                let mut pm = ps_peer_manager.write().await;
                                for addr in peers {
                                    pm.add_shared_peer(addr);
                                }
                            }
                        }
                        Err(e) => {
                            debug!("PeerSharing with {ps_peer_addr}: {e}");
                        }
                    }
                });
            }

            // Connect additional peers as block fetchers for parallel block fetch
            let mut fetch_pool = BlockFetchPool::new();
            {
                let pm = peer_manager.read().await;
                let additional_peers: Vec<std::net::SocketAddr> = pm
                    .peers_to_connect()
                    .into_iter()
                    .filter(|a| *a != peer_addr)
                    .take(4)
                    .collect();
                drop(pm);

                for addr in &additional_peers {
                    let target = addr.to_string();
                    let connect_start = std::time::Instant::now();
                    match NodeToNodeClient::connect(&*target, network_magic).await {
                        Ok(c) => {
                            let rtt_ms = connect_start.elapsed().as_secs_f64() * 1000.0;
                            let mut pm = peer_manager.write().await;
                            pm.peer_connected(addr, 14, true);
                            pm.record_handshake_rtt(addr, rtt_ms);
                            drop(pm);
                            info!("Connected block fetcher to {target} (handshake {rtt_ms:.0}ms)");
                            fetch_pool.add_fetcher(c);
                        }
                        Err(e) => {
                            warn!("Failed to connect fetcher to {target}: {e}");
                        }
                    }
                }
                // If no fetchers connected, add a dedicated fetcher to the primary peer.
                // This is necessary because the primary client connection is used for
                // pipelined ChainSync headers and can't simultaneously fetch blocks.
                if fetch_pool.is_empty() {
                    let target = peer_addr.to_string();
                    match NodeToNodeClient::connect(&*target, network_magic).await {
                        Ok(c) => {
                            info!("Connected dedicated block fetcher to primary peer {target}");
                            fetch_pool.add_fetcher(c);
                        }
                        Err(e) => {
                            warn!("Failed to connect dedicated fetcher to {target}: {e}");
                        }
                    }
                }
                info!(
                    "Block fetch pool: {} fetcher(s) for block retrieval",
                    fetch_pool.len()
                );
            }

            // Create pipelined ChainSync connection to same peer for high-throughput headers
            let pipelined_client = {
                let target = peer_addr.to_string();
                match PipelinedPeerClient::connect(&*target, network_magic).await {
                    Ok(pc) => {
                        info!("Pipelined ChainSync client connected to {target}");
                        Some(pc)
                    }
                    Err(e) => {
                        warn!("Pipelined client failed, using serial headers: {e}");
                        None
                    }
                }
            };

            // Run chain sync with connected peer + fetch pool
            let sync_shutdown = shutdown_rx.clone();
            match self
                .chain_sync_loop(
                    &mut active_client,
                    pipelined_client,
                    fetch_pool,
                    sync_shutdown,
                    peer_addr,
                )
                .await
            {
                Ok(()) => {
                    active_client.disconnect().await;
                    peer_manager.write().await.peer_disconnected(&peer_addr);
                    if *shutdown_rx.borrow() {
                        break;
                    }
                    info!("Sync ended, will reconnect...");
                }
                Err(e) => {
                    peer_manager.write().await.peer_disconnected(&peer_addr);
                    warn!("Sync error: {e}, will reconnect...");
                }
            }

            // Brief delay before reconnecting
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
                _ = shutdown_rx.changed() => { break; }
            }
        }

        // Save final ledger snapshot and flush ChainDB on shutdown
        self.save_ledger_snapshot().await;
        {
            let mut db = self.chain_db.write().await;
            if let Err(e) = db.flush_volatile_to_immutable() {
                error!("Failed to flush ChainDB on shutdown: {e}");
            }
        }
        info!("Node shutdown complete");
        Ok(())
    }

    /// Save a ledger state snapshot to the database directory
    async fn save_ledger_snapshot(&self) {
        let snapshot_path = self.database_path.join("ledger-snapshot.bin");
        let ls = self.ledger_state.read().await;
        if let Err(e) = ls.save_snapshot(&snapshot_path) {
            error!("Failed to save ledger snapshot: {e}");
        }
    }

    /// Replay blocks from local ChainDB to catch the ledger up to storage tip.
    ///
    /// After a Mithril snapshot import, ChainDB contains millions of blocks
    /// but the ledger state starts from genesis. This replays blocks locally
    /// (no network needed) which is much faster than re-downloading from peers.
    async fn replay_ledger_from_storage(&self) {
        let db_tip = self.chain_db.read().await.get_tip();
        let ledger_slot = {
            let ls = self.ledger_state.read().await;
            ls.tip.point.slot().map(|s| s.0).unwrap_or(0)
        };
        let db_tip_slot = db_tip.point.slot().map(|s| s.0).unwrap_or(0);

        if db_tip_slot <= ledger_slot {
            return; // Ledger is already caught up
        }

        let blocks_behind = db_tip.block_number.0.saturating_sub({
            let ls = self.ledger_state.read().await;
            ls.tip.block_number.0
        });

        // If the gap is very large (>50k blocks), skip the replay — it would
        // take too long without an up-to-date ledger snapshot. The ledger state
        // was already loaded from any existing snapshot before reaching here,
        // so blocks_behind reflects the snapshot's position (or genesis if none).
        if blocks_behind > 50_000 {
            warn!(
                blocks_behind,
                db_tip_slot,
                ledger_slot,
                "Skipping ledger replay: gap too large. \
                 The node will sync from peers and build ledger state incrementally."
            );
            return;
        }

        info!(
            ledger_slot,
            db_tip_slot,
            blocks_behind,
            "Ledger is behind ChainDB — replaying blocks from local storage"
        );

        let start = std::time::Instant::now();
        let mut current_slot = ledger_slot;
        let mut replayed = 0u64;
        let mut last_log = std::time::Instant::now();
        let snapshot_path = self.database_path.join("ledger-snapshot.bin");

        loop {
            // Read block from ChainDB
            let block_data = {
                let db = self.chain_db.read().await;
                db.get_next_block_after_slot(torsten_primitives::time::SlotNo(current_slot))
            };

            match block_data {
                Ok(Some((next_slot, _hash, cbor))) => {
                    if next_slot.0 > db_tip_slot {
                        break;
                    }
                    match torsten_serialization::multi_era::decode_block(&cbor) {
                        Ok(block) => {
                            let mut ls = self.ledger_state.write().await;
                            if let Err(e) = ls.apply_block(&block) {
                                warn!(slot = next_slot.0, "Ledger replay apply failed: {e}");
                            }
                            replayed += 1;
                            current_slot = next_slot.0;

                            // Log progress every 5 seconds
                            if last_log.elapsed().as_secs() >= 5 {
                                let elapsed = start.elapsed().as_secs_f64();
                                let speed = replayed as f64 / elapsed;
                                let pct = if db_tip_slot > 0 {
                                    current_slot as f64 / db_tip_slot as f64 * 100.0
                                } else {
                                    0.0
                                };
                                info!(
                                    "Replaying {pct:.2}% | slot {current_slot}/{db_tip_slot} \
                                     | {replayed} blocks | {speed:.0} blocks/s \
                                     | {} UTxOs",
                                    ls.utxo_set.len()
                                );
                                last_log = std::time::Instant::now();
                            }

                            // Save ledger snapshot every epoch (~100k blocks on preview, 432k on mainnet)
                            if replayed.is_multiple_of(100_000) {
                                if let Err(e) = ls.save_snapshot(&snapshot_path) {
                                    warn!("Failed to save ledger snapshot during replay: {e}");
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                slot = next_slot.0,
                                "Failed to decode block during replay: {e}"
                            );
                            current_slot = next_slot.0;
                        }
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    warn!("Failed to read from ChainDB during replay: {e}");
                    break;
                }
            }
        }

        let elapsed = start.elapsed().as_secs_f64();
        let speed = if elapsed > 0.0 {
            replayed as f64 / elapsed
        } else {
            0.0
        };
        info!(
            replayed,
            elapsed_secs = elapsed as u64,
            speed = speed as u64,
            "Ledger replay from local storage complete"
        );

        // Save final snapshot after replay
        {
            let ls = self.ledger_state.read().await;
            if let Err(e) = ls.save_snapshot(&snapshot_path) {
                error!("Failed to save ledger snapshot after replay: {e}");
            }
        }
    }

    async fn chain_sync_loop(
        &mut self,
        client: &mut NodeToNodeClient,
        pipelined_client: Option<PipelinedPeerClient>,
        fetch_pool: BlockFetchPool,
        mut shutdown_rx: watch::Receiver<bool>,
        peer_addr: std::net::SocketAddr,
    ) -> Result<()> {
        let mut pipelined = pipelined_client;
        // Find intersection with our current chain.
        // Use the furthest-ahead tip (ChainDB or ledger) as the primary
        // intersection point. After a Mithril import, ChainDB may be far
        // ahead of the ledger — we don't want to re-download blocks that
        // are already stored. The ledger builds state incrementally from
        // new blocks as they arrive.
        let chain_tip = self.chain_db.read().await.get_tip().point;
        let ledger_tip = self.ledger_state.read().await.tip.point.clone();
        let mut known_points = Vec::new();
        // Use whichever tip is further ahead as primary intersection
        if chain_tip != Point::Origin {
            known_points.push(chain_tip.clone());
        }
        if ledger_tip != Point::Origin && ledger_tip != chain_tip {
            known_points.push(ledger_tip.clone());
        }
        known_points.push(Point::Origin);
        if ledger_tip != chain_tip {
            info!(
                "Ledger tip ({}) differs from ChainDB tip ({}), syncing from ChainDB tip",
                ledger_tip, chain_tip
            );
        }
        // Find intersection: use pipelined client if available, otherwise serial client
        let (intersect, remote_tip) = if let Some(ref mut pc) = pipelined {
            pc.find_intersect(known_points.clone()).await?
        } else {
            client.find_intersect(known_points).await?
        };

        match &intersect {
            Some(point) => info!("Chain intersection found at {point}"),
            None => info!("Starting sync from Origin"),
        }
        info!("Remote tip: {remote_tip}");

        let use_pool = !fetch_pool.is_empty();
        let use_pipelined = pipelined.is_some();
        // Pipeline depth configurable via TORSTEN_PIPELINE_DEPTH env var (default: 150)
        // Benchmarked optimal: 150 yields ~275 blocks/sec vs ~151 at depth 100
        let max_pipeline_depth: usize = std::env::var("TORSTEN_PIPELINE_DEPTH")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(150);
        // When at tip, reduce to 1 to avoid sending many MsgRequestNext that
        // each need a new block (~20s) before the server responds.
        let mut pipeline_depth = max_pipeline_depth;
        if use_pipelined {
            info!(
                "Pipelined ChainSync enabled (pipeline depth {}), blocks from {} fetcher(s)",
                max_pipeline_depth,
                fetch_pool.len()
            );
        } else if use_pool {
            info!(
                "Multi-peer sync: headers from primary peer, blocks from {} fetcher(s)",
                fetch_pool.len()
            );
        }

        let mut blocks_received: u64 = 0;
        let mut last_snapshot_epoch: u64 = self.ledger_state.read().await.epoch.0;
        let mut last_log_time = std::time::Instant::now();
        let mut last_query_update = std::time::Instant::now();
        let mut blocks_since_last_log: u64 = 0;
        // Header batch size configurable via TORSTEN_HEADER_BATCH_SIZE env var
        let header_batch_size: usize = std::env::var("TORSTEN_HEADER_BATCH_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(if use_pipelined || use_pool { 500 } else { 100 });

        loop {
            if *shutdown_rx.borrow() {
                info!("Shutdown requested, stopping sync");
                break;
            }

            if use_pipelined || use_pool {
                // Pipelined/multi-peer mode: collect headers, fetch blocks from pool
                let header_future = async {
                    if let Some(ref mut pc) = pipelined {
                        pc.request_headers_pipelined_with_depth(header_batch_size, pipeline_depth)
                            .await
                    } else {
                        client.request_headers_batch(header_batch_size).await
                    }
                };
                tokio::select! {
                    result = header_future => {
                        match result {
                            Ok(batch_result) => {
                                match batch_result {
                                    HeaderBatchResult::Headers(headers, tip) => {
                                        // If we got a substantial batch, we're not at tip:
                                        // restore full pipeline depth for throughput
                                        if headers.len() > 10 && pipeline_depth < max_pipeline_depth {
                                            pipeline_depth = max_pipeline_depth;
                                        }
                                        let fetch_start = std::time::Instant::now();
                                        let header_count = headers.len() as u64;
                                        // Use fetch pool if available, otherwise primary peer
                                        let blocks_result = if fetch_pool.is_empty() {
                                            client.fetch_blocks_by_points(&headers).await
                                        } else {
                                            match fetch_pool.fetch_blocks_concurrent(&headers).await {
                                                Ok(blocks) => Ok(blocks),
                                                Err(e) => {
                                                    warn!("Pool fetch failed, falling back to primary peer: {e}");
                                                    client.fetch_blocks_by_points(&headers).await
                                                }
                                            }
                                        };
                                        match blocks_result {
                                            Ok(blocks) => {
                                                let fetch_ms = fetch_start.elapsed().as_secs_f64() * 1000.0;
                                                self.peer_manager.write().await.record_block_fetch(
                                                    &peer_addr, fetch_ms, header_count, 0,
                                                );
                                                self.process_forward_blocks(blocks, &tip, &mut blocks_received, &mut blocks_since_last_log, &mut last_snapshot_epoch, &mut last_log_time, &mut last_query_update).await;
                                            }
                                            Err(e) => { error!("Block fetch failed: {e}"); break; }
                                        }
                                    }
                                    HeaderBatchResult::HeadersAndRollback { headers, tip, rollback_point, .. } => {
                                        // Process any headers before the rollback
                                        if !headers.is_empty() {
                                            match fetch_pool.fetch_blocks_concurrent(&headers).await {
                                                Ok(blocks) => {
                                                    self.process_forward_blocks(blocks, &tip, &mut blocks_received, &mut blocks_since_last_log, &mut last_snapshot_epoch, &mut last_log_time, &mut last_query_update).await;
                                                }
                                                Err(e) => { warn!("Pool fetch failed during rollback batch: {e}"); }
                                            }
                                        }
                                        warn!("Rollback to {rollback_point}");
                                        self.handle_rollback(&rollback_point).await;
                                    }
                                    HeaderBatchResult::RollBackward(point, _tip) => {
                                        warn!("Rollback to {point}");
                                        self.handle_rollback(&point).await;
                                    }
                                    HeaderBatchResult::Await => {
                                        info!(blocks_received, "Caught up to chain tip, awaiting new blocks");
                                        // Enable strict VRF/KES verification now that we're synced
                                        self.consensus.set_strict_verification(true);
                                        self.update_query_state().await;
                                        self.try_forge_block().await;
                                        // At tip: reduce pipeline depth to 1 to avoid
                                        // sending many MsgRequestNext that pile up
                                        pipeline_depth = 1;
                                    }
                                }
                                // Reconnect pipelined client if it became stale
                                // (has pending in-flight requests from pipelining
                                // that would block for minutes waiting for new blocks)
                                if pipelined.as_ref().is_some_and(|pc| pc.is_stale()) {
                                    // We hit the tip — reduce pipeline depth and
                                    // enable strict verification for new blocks
                                    pipeline_depth = 1;
                                    self.consensus.set_strict_verification(true);
                                    let old = pipelined.take().unwrap();
                                    let addr = old.remote_addr();
                                    old.abort().await;
                                    match PipelinedPeerClient::connect(&addr.to_string() as &str, self.network_magic).await {
                                        Ok(mut new_pc) => {
                                            let tip = self.ledger_state.read().await.tip.point.clone();
                                            let mut pts = Vec::new();
                                            if tip != Point::Origin { pts.push(tip); }
                                            pts.push(Point::Origin);
                                            match new_pc.find_intersect(pts).await {
                                                Ok(_) => {
                                                    info!("Reconnected pipelined client after tip sync");
                                                    pipelined = Some(new_pc);
                                                }
                                                Err(e) => warn!("Pipelined reconnect intersect failed: {e}"),
                                            }
                                        }
                                        Err(e) => warn!("Pipelined reconnect failed: {e}"),
                                    }
                                }
                            }
                            Err(e) => { error!("Chain sync error: {e}"); break; }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        info!("Shutdown requested during sync");
                        break;
                    }
                }
            } else {
                // Single-peer mode: use request_next_batch (headers + blocks from same peer)
                tokio::select! {
                    result = client.request_next_batch(header_batch_size) => {
                        match result {
                            Ok(events) => {
                                let mut forward_blocks = Vec::new();
                                let mut other_events = Vec::new();

                                for event in events {
                                    match event {
                                        ChainSyncEvent::RollForward(block, tip) => {
                                            forward_blocks.push((*block, tip));
                                        }
                                        other => other_events.push(other),
                                    }
                                }

                                if !forward_blocks.is_empty() {
                                    let tip = forward_blocks.last().unwrap().1.clone();
                                    let blocks: Vec<_> = forward_blocks.into_iter().map(|(b, _)| b).collect();
                                    self.process_forward_blocks(blocks, &tip, &mut blocks_received, &mut blocks_since_last_log, &mut last_snapshot_epoch, &mut last_log_time, &mut last_query_update).await;
                                }

                                for event in other_events {
                                    match event {
                                        ChainSyncEvent::RollBackward(point, tip) => {
                                            warn!("Rollback to {point}, tip: {tip}");
                                            self.handle_rollback(&point).await;
                                        }
                                        ChainSyncEvent::Await => {
                                            info!(blocks_received, "Caught up to chain tip, awaiting new blocks");
                                            self.consensus.set_strict_verification(true);
                                            self.update_query_state().await;
                                        }
                                        ChainSyncEvent::RollForward(..) => unreachable!(),
                                    }
                                }
                            }
                            Err(e) => { error!("Chain sync error: {e}"); break; }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        info!("Shutdown requested during sync");
                        break;
                    }
                }
            }
        }

        self.save_ledger_snapshot().await;
        fetch_pool.disconnect_all().await;
        info!("Chain sync stopped after {blocks_received} blocks");
        Ok(())
    }

    /// Process a batch of forward blocks: store in ChainDB, apply to ledger, validate, log progress
    #[allow(clippy::too_many_arguments)]
    async fn process_forward_blocks(
        &mut self,
        mut blocks: Vec<torsten_primitives::block::Block>,
        tip: &torsten_primitives::block::Tip,
        blocks_received: &mut u64,
        blocks_since_last_log: &mut u64,
        last_snapshot_epoch: &mut u64,
        last_log_time: &mut std::time::Instant,
        last_query_update: &mut std::time::Instant,
    ) {
        if blocks.is_empty() {
            return;
        }

        // Validate block headers BEFORE storing. Log warnings for validation
        // failures. VRF/KES verification is not yet fully implemented, so
        // failures are non-fatal to allow the node to continue syncing.
        let strict = self.consensus.strict_verification();
        if let Some(last_block) = blocks.last() {
            if last_block.era.is_shelley_based() {
                // Populate epoch_nonce from ledger state — the deserialized header
                // always carries Hash32::ZERO for epoch_nonce since the wire format
                // does not include the nonce; it must be injected from ledger state
                // before VRF verification.
                let epoch_nonce = {
                    let ls = self.ledger_state.read().await;
                    ls.epoch_nonce
                };
                let mut header_with_nonce = last_block.header.clone();
                header_with_nonce.epoch_nonce = epoch_nonce;
                if let Err(e) = self
                    .consensus
                    .validate_header(&header_with_nonce, last_block.slot())
                {
                    warn!(
                        slot = last_block.slot().0,
                        block_no = last_block.block_number().0,
                        "Consensus validation: {e}"
                    );
                }
            }
        }

        let batch_count = blocks.len() as u64;

        // Build ChainDB batch data, taking ownership of raw_cbor to avoid cloning
        let db_batch: Vec<_> = blocks
            .iter_mut()
            .map(|block| {
                (
                    *block.hash(),
                    block.slot(),
                    block.block_number(),
                    *block.prev_hash(),
                    block.raw_cbor.take().unwrap_or_default(),
                )
            })
            .collect();

        // Store blocks to ChainDB FIRST, then apply to ledger.
        // This ordering ensures the ledger never advances past what's persisted in storage,
        // preventing state divergence if storage fails.
        {
            let mut db = self.chain_db.write().await;
            if let Err(e) = db.add_blocks_batch(db_batch) {
                error!(
                    "FATAL: Failed to store block batch: {e} — halting to prevent state divergence"
                );
                return;
            }
        }

        // Now apply blocks to ledger — storage is confirmed
        {
            let mut ls = self.ledger_state.write().await;
            let ledger_slot = ls.tip.point.slot().map(|s| s.0).unwrap_or(0);
            for block in &blocks {
                // Skip blocks the ledger has already applied (e.g. replaying from origin)
                if block.slot().0 <= ledger_slot {
                    continue;
                }
                if let Err(e) = ls.apply_block(block) {
                    error!("Failed to apply block to ledger: {e}");
                }
            }
        }

        // Remove confirmed transactions from mempool and revalidate
        if !self.mempool.is_empty() {
            let confirmed_hashes: Vec<_> = blocks
                .iter()
                .flat_map(|b| b.transactions.iter().map(|tx| tx.hash))
                .collect();
            if !confirmed_hashes.is_empty() {
                self.mempool.remove_txs(&confirmed_hashes);
            }

            // Remove mempool txs whose inputs conflict with the confirmed block inputs
            let consumed_inputs: std::collections::HashSet<_> = blocks
                .iter()
                .flat_map(|b| b.transactions.iter())
                .flat_map(|tx| tx.body.inputs.iter().cloned())
                .collect();
            self.mempool.revalidate_against_inputs(&consumed_inputs);

            // Evict expired transactions based on current slot
            if let Some(last_block) = blocks.last() {
                self.mempool.evict_expired(last_block.slot());
            }
        }

        if let Some(last_block) = blocks.last() {
            self.consensus.update_tip(last_block.tip());
        }

        *blocks_received += batch_count;
        *blocks_since_last_log += batch_count;
        self.metrics.add_blocks_received(batch_count);
        self.metrics.add_blocks_applied(batch_count);

        let last_block = blocks.last().unwrap();
        let slot = last_block.slot().0;
        let block_no = last_block.block_number().0;
        self.metrics.set_slot(slot);
        self.metrics.set_block_number(block_no);

        // Log each new block when following the tip
        if strict {
            for block in &blocks {
                info!(
                    slot = block.slot().0,
                    block_no = block.block_number().0,
                    hash = %block.hash().to_hex(),
                    txs = block.transactions.len(),
                    "New block"
                );
            }
        }

        {
            let current_epoch = self.ledger_state.read().await.epoch.0;
            if current_epoch > *last_snapshot_epoch {
                info!(
                    epoch = current_epoch,
                    "Epoch transition — saving ledger snapshot"
                );
                self.save_ledger_snapshot().await;
                *last_snapshot_epoch = current_epoch;
            }
        }

        let elapsed = last_log_time.elapsed();
        if elapsed.as_secs() >= 5 || *blocks_received <= 5 {
            let tip_slot = tip.point.slot().map(|s| s.0).unwrap_or(0);
            let tip_block = tip.block_number.0;
            let progress = if tip_slot > 0 {
                (slot as f64 / tip_slot as f64 * 100.0).min(100.0)
            } else {
                0.0
            };
            let blocks_per_sec = if elapsed.as_secs_f64() > 0.0 {
                *blocks_since_last_log as f64 / elapsed.as_secs_f64()
            } else {
                0.0
            };
            let blocks_remaining = tip_block.saturating_sub(block_no);
            {
                let ls = self.ledger_state.read().await;
                self.metrics.set_epoch(ls.epoch.0);
                self.metrics.set_utxo_count(ls.utxo_set.len() as u64);
                self.metrics.set_sync_progress(progress);
                self.metrics.set_mempool_count(self.mempool.len() as u64);
                self.metrics.mempool_bytes.store(
                    self.mempool.total_bytes() as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
                {
                    let pm = self.peer_manager.read().await;
                    self.metrics.peers_connected.store(
                        pm.hot_peer_count() as u64,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                }
                self.metrics.delegation_count.store(
                    ls.delegations.len() as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
                self.metrics
                    .treasury_lovelace
                    .store(ls.treasury.0, std::sync::atomic::Ordering::Relaxed);
                info!(
                    "Syncing {progress:.2}% | slot {slot}/{tip_slot} | block {block_no}/{tip_block} | epoch {} | {blocks_per_sec:.0} blocks/s | {} UTxOs | {blocks_remaining} blocks remaining",
                    ls.epoch.0,
                    ls.utxo_set.len()
                );
            }
            *last_log_time = std::time::Instant::now();
            *blocks_since_last_log = 0;
            if last_query_update.elapsed().as_secs() >= 30 {
                self.update_query_state().await;
                // Recompute peer reputations periodically
                self.peer_manager.write().await.recompute_reputations();
                *last_query_update = std::time::Instant::now();
            }
        }
    }

    /// Update the query handler with the current ledger state
    async fn update_query_state(&self) {
        use torsten_network::query_handler::{
            CommitteeMemberSnapshot, CommitteeSnapshot, DRepSnapshot, PoolParamsSnapshot,
            PoolStakeSnapshotEntry, ProposalSnapshot, StakeAddressSnapshot, StakePoolSnapshot,
            StakeSnapshotsResult,
        };

        let ls = self.ledger_state.read().await;

        // Build per-pool stake map from delegations for accurate reporting
        let mut pool_stake_map: std::collections::HashMap<torsten_primitives::hash::Hash28, u64> =
            std::collections::HashMap::new();
        for (cred_hash, pool_id) in &ls.delegations {
            let stake = ls
                .stake_distribution
                .stake_map
                .get(cred_hash)
                .map(|l| l.0)
                .unwrap_or(0);
            *pool_stake_map.entry(*pool_id).or_default() += stake;
        }

        // Build stake pool snapshots with actual per-pool stake
        let total_active_stake: u64 = pool_stake_map.values().sum();
        let stake_pools: Vec<StakePoolSnapshot> = ls
            .pool_params
            .iter()
            .map(|(pool_id, reg)| StakePoolSnapshot {
                pool_id: pool_id.as_ref().to_vec(),
                stake: pool_stake_map.get(pool_id).copied().unwrap_or(0),
                vrf_keyhash: reg.vrf_keyhash.as_ref().to_vec(),
                total_active_stake,
            })
            .collect();

        // Build DRep snapshots
        let drep_entries: Vec<DRepSnapshot> = ls
            .governance
            .dreps
            .iter()
            .map(|(hash, drep)| {
                let expiry = drep.registered_epoch.0 + ls.protocol_params.drep_activity;
                DRepSnapshot {
                    credential_hash: hash.as_ref().to_vec(),
                    credential_type: 0, // KeyHashObj (we don't track script DReps separately yet)
                    deposit: drep.deposit.0,
                    anchor_url: drep.anchor.as_ref().map(|a| a.url.clone()),
                    anchor_hash: drep.anchor.as_ref().map(|a| a.data_hash.as_ref().to_vec()),
                    expiry_epoch: expiry,
                    delegator_hashes: Vec::new(), // TODO: populate from delegation index
                }
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
                    deposit: state.procedure.deposit.0,
                    return_addr: state.procedure.return_addr.clone(),
                    anchor_url: state.procedure.anchor.url.clone(),
                    anchor_hash: state.procedure.anchor.data_hash.as_ref().to_vec(),
                }
            })
            .collect();

        // Build committee snapshot
        let resigned_set: std::collections::HashSet<_> =
            ls.governance.committee_resigned.keys().collect();
        let committee = CommitteeSnapshot {
            members: ls
                .governance
                .committee_hot_keys
                .iter()
                .map(|(cold, hot)| {
                    let is_resigned = resigned_set.contains(cold);
                    CommitteeMemberSnapshot {
                        cold_credential: cold.as_ref().to_vec(),
                        cold_credential_type: 0, // KeyHashObj
                        hot_status: if is_resigned { 2 } else { 0 },
                        hot_credential: if is_resigned {
                            None
                        } else {
                            Some(hot.as_ref().to_vec())
                        },
                        member_status: 0, // Active (simplified)
                        expiry_epoch: None,
                    }
                })
                .collect(),
            threshold: Some((2, 3)), // Default quorum 2/3
            current_epoch: ls.epoch.0,
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

        // Build stake snapshots (mark/set/go)
        let stake_snapshots = {
            // Collect all unique pool IDs across all snapshots
            let mut all_pool_ids = std::collections::BTreeSet::new();
            if let Some(ref snap) = ls.snapshots.mark {
                all_pool_ids.extend(snap.pool_stake.keys().cloned());
            }
            if let Some(ref snap) = ls.snapshots.set {
                all_pool_ids.extend(snap.pool_stake.keys().cloned());
            }
            if let Some(ref snap) = ls.snapshots.go {
                all_pool_ids.extend(snap.pool_stake.keys().cloned());
            }

            let pools: Vec<PoolStakeSnapshotEntry> = all_pool_ids
                .iter()
                .map(|pid| PoolStakeSnapshotEntry {
                    pool_id: pid.as_ref().to_vec(),
                    mark_stake: ls
                        .snapshots
                        .mark
                        .as_ref()
                        .and_then(|s| s.pool_stake.get(pid))
                        .map(|l| l.0)
                        .unwrap_or(0),
                    set_stake: ls
                        .snapshots
                        .set
                        .as_ref()
                        .and_then(|s| s.pool_stake.get(pid))
                        .map(|l| l.0)
                        .unwrap_or(0),
                    go_stake: ls
                        .snapshots
                        .go
                        .as_ref()
                        .and_then(|s| s.pool_stake.get(pid))
                        .map(|l| l.0)
                        .unwrap_or(0),
                })
                .collect();

            let total_mark_stake = pools.iter().map(|p| p.mark_stake).sum();
            let total_set_stake = pools.iter().map(|p| p.set_stake).sum();
            let total_go_stake = pools.iter().map(|p| p.go_stake).sum();

            StakeSnapshotsResult {
                pools,
                total_mark_stake,
                total_set_stake,
                total_go_stake,
            }
        };

        // Build pool params entries
        let pool_params_entries: Vec<PoolParamsSnapshot> = ls
            .pool_params
            .iter()
            .map(|(pool_id, reg)| {
                use torsten_network::query_handler::RelaySnapshot;
                let relays: Vec<RelaySnapshot> = reg
                    .relays
                    .iter()
                    .map(|r| match r {
                        torsten_primitives::transaction::Relay::SingleHostAddr {
                            port,
                            ipv4,
                            ipv6,
                        } => RelaySnapshot::SingleHostAddr {
                            port: *port,
                            ipv4: *ipv4,
                            ipv6: *ipv6,
                        },
                        torsten_primitives::transaction::Relay::SingleHostName {
                            port,
                            dns_name,
                        } => RelaySnapshot::SingleHostName {
                            port: *port,
                            dns_name: dns_name.clone(),
                        },
                        torsten_primitives::transaction::Relay::MultiHostName { dns_name } => {
                            RelaySnapshot::MultiHostName {
                                dns_name: dns_name.clone(),
                            }
                        }
                    })
                    .collect();
                PoolParamsSnapshot {
                    pool_id: pool_id.as_ref().to_vec(),
                    vrf_keyhash: reg.vrf_keyhash.as_ref().to_vec(),
                    pledge: reg.pledge.0,
                    cost: reg.cost.0,
                    margin_num: reg.margin_numerator,
                    margin_den: reg.margin_denominator,
                    reward_account: reg.reward_account.clone(),
                    owners: reg.owners.iter().map(|o| o.as_ref().to_vec()).collect(),
                    relays,
                    metadata_url: reg.metadata_url.clone(),
                    metadata_hash: reg.metadata_hash.map(|h| h.as_ref().to_vec()),
                }
            })
            .collect();

        // Build protocol params snapshot for CBOR encoding
        let pp = &ls.protocol_params;
        let protocol_params = torsten_network::query_handler::ProtocolParamsSnapshot {
            min_fee_a: pp.min_fee_a,
            min_fee_b: pp.min_fee_b,
            max_block_body_size: pp.max_block_body_size,
            max_tx_size: pp.max_tx_size,
            max_block_header_size: pp.max_block_header_size,
            key_deposit: pp.key_deposit.0,
            pool_deposit: pp.pool_deposit.0,
            e_max: pp.e_max,
            n_opt: pp.n_opt,
            a0_num: pp.a0.numerator,
            a0_den: pp.a0.denominator,
            rho_num: pp.rho.numerator,
            rho_den: pp.rho.denominator,
            tau_num: pp.tau.numerator,
            tau_den: pp.tau.denominator,
            min_pool_cost: pp.min_pool_cost.0,
            ada_per_utxo_byte: pp.ada_per_utxo_byte.0,
            cost_models_v1: pp.cost_models.plutus_v1.clone(),
            cost_models_v2: pp.cost_models.plutus_v2.clone(),
            cost_models_v3: pp.cost_models.plutus_v3.clone(),
            execution_costs_mem_num: pp.execution_costs.mem_price.numerator,
            execution_costs_mem_den: pp.execution_costs.mem_price.denominator,
            execution_costs_step_num: pp.execution_costs.step_price.numerator,
            execution_costs_step_den: pp.execution_costs.step_price.denominator,
            max_tx_ex_mem: pp.max_tx_ex_units.mem,
            max_tx_ex_steps: pp.max_tx_ex_units.steps,
            max_block_ex_mem: pp.max_block_ex_units.mem,
            max_block_ex_steps: pp.max_block_ex_units.steps,
            max_val_size: pp.max_val_size,
            collateral_percentage: pp.collateral_percentage,
            max_collateral_inputs: pp.max_collateral_inputs,
            protocol_version_major: pp.protocol_version_major,
            protocol_version_minor: pp.protocol_version_minor,
            min_fee_ref_script_cost_per_byte: pp.min_fee_ref_script_cost_per_byte,
            drep_deposit: pp.drep_deposit.0,
            drep_activity: pp.drep_activity,
            gov_action_deposit: pp.gov_action_deposit.0,
            gov_action_lifetime: pp.gov_action_lifetime,
            committee_min_size: pp.committee_min_size,
            committee_max_term_length: pp.committee_max_term_length,
            dvt_pp_network_group_num: pp.dvt_pp_network_group.numerator,
            dvt_pp_network_group_den: pp.dvt_pp_network_group.denominator,
            dvt_pp_economic_group_num: pp.dvt_pp_economic_group.numerator,
            dvt_pp_economic_group_den: pp.dvt_pp_economic_group.denominator,
            dvt_pp_technical_group_num: pp.dvt_pp_technical_group.numerator,
            dvt_pp_technical_group_den: pp.dvt_pp_technical_group.denominator,
            dvt_pp_gov_group_num: pp.dvt_pp_gov_group.numerator,
            dvt_pp_gov_group_den: pp.dvt_pp_gov_group.denominator,
            dvt_hard_fork_num: pp.dvt_hard_fork.numerator,
            dvt_hard_fork_den: pp.dvt_hard_fork.denominator,
            dvt_no_confidence_num: pp.dvt_no_confidence.numerator,
            dvt_no_confidence_den: pp.dvt_no_confidence.denominator,
            dvt_committee_normal_num: pp.dvt_committee_normal.numerator,
            dvt_committee_normal_den: pp.dvt_committee_normal.denominator,
            dvt_committee_no_confidence_num: pp.dvt_committee_no_confidence.numerator,
            dvt_committee_no_confidence_den: pp.dvt_committee_no_confidence.denominator,
            dvt_constitution_num: pp.dvt_constitution.numerator,
            dvt_constitution_den: pp.dvt_constitution.denominator,
            dvt_treasury_withdrawal_num: pp.dvt_treasury_withdrawal.numerator,
            dvt_treasury_withdrawal_den: pp.dvt_treasury_withdrawal.denominator,
            pvt_motion_no_confidence_num: pp.pvt_motion_no_confidence.numerator,
            pvt_motion_no_confidence_den: pp.pvt_motion_no_confidence.denominator,
            pvt_committee_normal_num: pp.pvt_committee_normal.numerator,
            pvt_committee_normal_den: pp.pvt_committee_normal.denominator,
            pvt_committee_no_confidence_num: pp.pvt_committee_no_confidence.numerator,
            pvt_committee_no_confidence_den: pp.pvt_committee_no_confidence.denominator,
            pvt_hard_fork_num: pp.pvt_hard_fork.numerator,
            pvt_hard_fork_den: pp.pvt_hard_fork.denominator,
            pvt_pp_security_group_num: pp.pvt_pp_security_group.numerator,
            pvt_pp_security_group_den: pp.pvt_pp_security_group.denominator,
        };

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
            protocol_params,
            stake_pools,
            drep_entries,
            governance_proposals,
            committee,
            constitution_url: ls
                .governance
                .constitution
                .as_ref()
                .map(|c| c.anchor.url.clone())
                .unwrap_or_default(),
            constitution_hash: ls
                .governance
                .constitution
                .as_ref()
                .map(|c| c.anchor.data_hash.as_ref().to_vec())
                .unwrap_or_else(|| vec![0u8; 32]),
            constitution_script: ls
                .governance
                .constitution
                .as_ref()
                .and_then(|c| c.script_hash.as_ref().map(|h| h.as_ref().to_vec())),
            stake_addresses,
            stake_snapshots,
            pool_params_entries,
            epoch_length: ls.epoch_length,
            slot_length_secs: 1, // Shelley slot length is always 1 second
            network_magic: self.network_magic as u32,
            security_param: self.consensus.security_param,
        };

        // Drop the ledger read lock before acquiring the query handler write lock
        drop(ls);

        let mut handler = self.query_handler.write().await;
        handler.update_state(snapshot);
    }

    /// Notify connected N2N peers of a chain rollback by sending MsgRollBackward.
    async fn notify_rollback(&self, rollback_point: &Point) {
        if let Some(ref tx) = self.rollback_announcement_tx {
            let (tip_slot, tip_hash, tip_block_number) = {
                let db = self.chain_db.read().await;
                let tip = db.get_tip();
                let slot = tip.point.slot().map(|s| s.0).unwrap_or(0);
                let hash = tip
                    .point
                    .hash()
                    .map(|h| {
                        let bytes: &[u8] = h.as_ref();
                        let mut arr = [0u8; 32];
                        arr.copy_from_slice(bytes);
                        arr
                    })
                    .unwrap_or([0u8; 32]);
                (slot, hash, tip.block_number.0)
            };

            let rb_slot = rollback_point.slot().map(|s| s.0).unwrap_or(0);
            let rb_hash = rollback_point
                .hash()
                .map(|h| {
                    let bytes: &[u8] = h.as_ref();
                    let mut arr = [0u8; 32];
                    arr.copy_from_slice(bytes);
                    arr
                })
                .unwrap_or([0u8; 32]);

            let _ = tx.send(torsten_network::RollbackAnnouncement {
                slot: rb_slot,
                hash: rb_hash,
                tip_slot,
                tip_hash,
                tip_block_number,
            });
        }
    }

    /// Handle a chain rollback: roll back ChainDB, reload ledger state from snapshot,
    /// and replay blocks from the snapshot up to the rollback point.
    async fn handle_rollback(&self, rollback_point: &Point) {
        let rollback_slot = rollback_point.slot().map(|s| s.0).unwrap_or(0);

        // If the rollback point is at or beyond our ledger tip, it's a no-op.
        // This commonly happens after reconnection when the server confirms
        // the intersection by sending a RollBackward to the same point.
        {
            let ls = self.ledger_state.read().await;
            let ledger_slot = ls.tip.point.slot().map(|s| s.0).unwrap_or(0);
            if rollback_slot >= ledger_slot {
                debug!(
                    rollback_slot,
                    ledger_slot, "Rollback point is at or ahead of ledger tip, skipping"
                );
                return;
            }
        }

        self.metrics
            .rollback_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // 1. Roll back ChainDB
        {
            let mut db = self.chain_db.write().await;
            if let Err(e) = db.rollback_to_point(rollback_point) {
                error!("ChainDB rollback failed: {e}");
                return;
            }
        }

        // 2. Reload ledger state from the last snapshot
        let snapshot_path = self.database_path.join("ledger-snapshot.bin");
        if snapshot_path.exists() {
            match torsten_ledger::LedgerState::load_snapshot(&snapshot_path) {
                Ok(snapshot_state) => {
                    let snapshot_slot = snapshot_state.tip.point.slot().map(|s| s.0).unwrap_or(0);
                    let rollback_slot = rollback_point.slot().map(|s| s.0).unwrap_or(0);

                    if snapshot_slot <= rollback_slot {
                        // Snapshot is at or before the rollback point — replay forward
                        let mut ls = self.ledger_state.write().await;
                        *ls = snapshot_state;
                        let replay_from = snapshot_slot;

                        // 3. Replay blocks from snapshot tip to rollback point
                        let db = self.chain_db.read().await;
                        let mut current_slot = replay_from;
                        let mut replayed = 0u64;
                        while current_slot < rollback_slot {
                            match db.get_next_block_after_slot(torsten_primitives::time::SlotNo(
                                current_slot,
                            )) {
                                Ok(Some((next_slot, _hash, cbor))) => {
                                    if next_slot.0 > rollback_slot {
                                        break;
                                    }
                                    match torsten_serialization::multi_era::decode_block(&cbor) {
                                        Ok(block) => {
                                            if let Err(e) = ls.apply_block(&block) {
                                                warn!("Ledger apply failed during replay: {e}");
                                            }
                                            replayed += 1;
                                            current_slot = next_slot.0;
                                        }
                                        Err(e) => {
                                            warn!("Failed to decode block during replay: {e}");
                                            break;
                                        }
                                    }
                                }
                                Ok(None) => break,
                                Err(e) => {
                                    warn!("Failed to read block during replay: {e}");
                                    break;
                                }
                            }
                        }
                        info!(
                            snapshot_slot,
                            rollback_slot,
                            replayed,
                            "Ledger state restored from snapshot and replayed"
                        );
                    } else {
                        // Snapshot is ahead of rollback point — can't use it, reset to genesis
                        warn!(
                            snapshot_slot,
                            rollback_slot = rollback_point.slot().map(|s| s.0).unwrap_or(0),
                            "Snapshot is ahead of rollback point, resetting ledger state"
                        );
                        let mut ls = self.ledger_state.write().await;
                        *ls = torsten_ledger::LedgerState::new(ls.protocol_params.clone());
                    }
                }
                Err(e) => {
                    error!("Failed to load ledger snapshot for rollback: {e}");
                    // Reset to empty ledger state as fallback
                    let mut ls = self.ledger_state.write().await;
                    *ls = torsten_ledger::LedgerState::new(ls.protocol_params.clone());
                }
            }
        } else {
            // No snapshot — reset to empty ledger state
            warn!("No ledger snapshot found for rollback, resetting ledger state");
            let mut ls = self.ledger_state.write().await;
            *ls = torsten_ledger::LedgerState::new(ls.protocol_params.clone());
        }

        // 4. Clear mempool — UTxO set has changed, existing txs may be invalid
        self.mempool.clear();

        // 5. Notify peers
        self.notify_rollback(rollback_point).await;
    }

    /// Attempt to forge a block if we are in block producer mode and are the slot leader.
    ///
    /// Called when the node is caught up to the chain tip.
    async fn try_forge_block(&mut self) {
        let creds = match &self.block_producer {
            Some(c) => c,
            None => return, // relay-only mode
        };

        let ls = self.ledger_state.read().await;
        let current_slot = ls.tip.point.slot().map(|s| s.0).unwrap_or(0);
        let next_slot = torsten_primitives::time::SlotNo(current_slot + 1);
        let epoch_nonce = ls.epoch_nonce;
        let block_number = torsten_primitives::time::BlockNo(ls.current_block_number().0 + 1);
        let prev_hash = ls
            .tip
            .point
            .hash()
            .copied()
            .unwrap_or(torsten_primitives::hash::Hash32::ZERO);

        // Calculate relative stake from the "set" snapshot (used for leader election)
        let relative_stake = if let Some(set_snapshot) = &ls.snapshots.set {
            let total_stake: u64 = set_snapshot.pool_stake.values().map(|s| s.0).sum();
            let pool_stake = set_snapshot
                .pool_stake
                .get(&creds.pool_id)
                .map(|s| s.0)
                .unwrap_or(0);
            if total_stake > 0 {
                pool_stake as f64 / total_stake as f64
            } else {
                0.0
            }
        } else {
            0.0
        };
        drop(ls);

        if relative_stake == 0.0 {
            return; // No stake, can't be leader
        }

        // Check if we are the slot leader
        if !crate::forge::check_slot_leadership(
            creds,
            next_slot,
            &epoch_nonce,
            relative_stake,
            self.consensus.active_slot_coeff,
        ) {
            return; // Not our slot
        }

        info!(
            slot = next_slot.0,
            relative_stake = format!("{:.6}", relative_stake),
            "Elected as slot leader!"
        );

        // Collect transactions from mempool (up to limits)
        let transactions = self.mempool.get_txs_for_block(500, 90112);
        let config = crate::forge::BlockProducerConfig::default();

        match crate::forge::forge_block(
            creds,
            &config,
            next_slot,
            block_number,
            prev_hash,
            &epoch_nonce,
            transactions,
        ) {
            Ok((block, cbor)) => {
                // Store the forged block in ChainDB
                {
                    let mut db = self.chain_db.write().await;
                    if let Err(e) = db.add_block(
                        *block.hash(),
                        block.slot(),
                        block.block_number(),
                        *block.prev_hash(),
                        cbor,
                    ) {
                        error!("Failed to store forged block: {e}");
                        return;
                    }
                }

                // Apply to ledger
                {
                    let mut ls = self.ledger_state.write().await;
                    if let Err(e) = ls.apply_block(&block) {
                        error!("Failed to apply forged block to ledger: {e}");
                        return;
                    }
                }

                // Remove confirmed transactions from mempool
                let confirmed: Vec<_> = block.transactions.iter().map(|tx| tx.hash).collect();
                if !confirmed.is_empty() {
                    self.mempool.remove_txs(&confirmed);
                }

                // Update consensus tip
                self.consensus.update_tip(block.tip());

                self.metrics
                    .blocks_forged
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                info!(
                    slot = next_slot.0,
                    block_number = block_number.0,
                    tx_count = block.transactions.len(),
                    "Forged block applied to local chain"
                );

                // Announce the new block to all connected peers
                if let Some(ref tx) = self.block_announcement_tx {
                    let mut hash_bytes = [0u8; 32];
                    hash_bytes.copy_from_slice(block.header.header_hash.as_ref());
                    tx.send(torsten_network::BlockAnnouncement {
                        slot: next_slot.0,
                        hash: hash_bytes,
                        block_number: block_number.0,
                    })
                    .ok();
                }
            }
            Err(e) => {
                error!("Block forging failed: {e}");
            }
        }
    }
}
