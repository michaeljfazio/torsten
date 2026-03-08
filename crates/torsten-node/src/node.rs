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
    BlockFetchPool, BlockProvider, ChainSyncEvent, DiffusionMode, HeaderBatchResult, N2CServer,
    NodeServer, NodeStateSnapshot, NodeToNodeClient, PeerManager, PeerManagerConfig, QueryHandler,
    TxValidator,
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

        // Load block producer credentials if all three key paths are provided
        let block_producer = match (
            &args.shelley_vrf_key,
            &args.shelley_kes_key,
            &args.shelley_operational_certificate,
        ) {
            (Some(vrf_path), Some(kes_path), Some(opcert_path)) => {
                match crate::forge::BlockProducerCredentials::load(vrf_path, kes_path, opcert_path)
                {
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

        // Setup shutdown signal
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        tokio::spawn(async move {
            signal::ctrl_c().await.ok();
            info!("Shutdown signal received");
            shutdown_tx.send(true).ok();
        });

        // SIGHUP handler is set up after peer_manager initialization below

        // Start Prometheus metrics server on port 12798
        {
            let metrics = self.metrics.clone();
            tokio::spawn(async move {
                crate::metrics::start_metrics_server(12798, metrics).await;
            });
        }

        // Start N2C server on Unix socket
        let mut n2c_server = N2CServer::new(self.query_handler.clone(), self.mempool.clone());
        let slot_config = self.ledger_state.read().await.slot_config;
        n2c_server.set_tx_validator(Arc::new(LedgerTxValidator {
            ledger: self.ledger_state.clone(),
            slot_config,
        }));
        info!("N2C server: Plutus tx validation enabled");
        let n2c_socket_path = self.socket_path.clone();
        tokio::spawn(async move {
            if let Err(e) = n2c_server.listen(&n2c_socket_path).await {
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
                // Resolve address to SocketAddr
                if let Ok(mut addrs) =
                    tokio::net::lookup_host(format!("{}:{}", peer.address, peer.port)).await
                {
                    if let Some(socket_addr) = addrs.next() {
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
                let mut hup = signal::unix::signal(signal::unix::SignalKind::hangup()).unwrap();
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
                                if let Ok(mut addrs) = tokio::net::lookup_host(format!(
                                    "{}:{}",
                                    peer.address, peer.port
                                ))
                                .await
                                {
                                    if let Some(socket_addr) = addrs.next() {
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
        info!(
            "N2N server: diffusion_mode={:?}, peer_sharing=enabled",
            self.peer_manager.read().await.diffusion_mode()
        );
        tokio::spawn(async move {
            if let Err(e) = n2n_server.listen().await {
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
                if !fetch_pool.is_empty() {
                    info!(
                        "Block fetch pool: {} fetcher(s) for parallel block retrieval",
                        fetch_pool.len()
                    );
                }
            }

            // Run chain sync with connected peer + fetch pool
            let sync_shutdown = shutdown_rx.clone();
            match self
                .chain_sync_loop(&mut active_client, fetch_pool, sync_shutdown, peer_addr)
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

        // Save final ledger snapshot on shutdown
        self.save_ledger_snapshot().await;
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

    async fn chain_sync_loop(
        &mut self,
        client: &mut NodeToNodeClient,
        fetch_pool: BlockFetchPool,
        mut shutdown_rx: watch::Receiver<bool>,
        peer_addr: std::net::SocketAddr,
    ) -> Result<()> {
        // Find intersection with our current chain
        let chain_tip = self.chain_db.read().await.get_tip().point;
        let ledger_tip = self.ledger_state.read().await.tip.point.clone();
        let mut known_points = Vec::new();
        if ledger_tip > chain_tip {
            known_points.push(ledger_tip);
        }
        if chain_tip != Point::Origin {
            known_points.push(chain_tip);
        }
        known_points.push(Point::Origin);
        let (intersect, remote_tip) = client.find_intersect(known_points).await?;

        match &intersect {
            Some(point) => info!("Chain intersection found at {point}"),
            None => info!("Starting sync from Origin"),
        }
        info!("Remote tip: {remote_tip}");

        let use_pool = !fetch_pool.is_empty();
        if use_pool {
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
        let header_batch_size = if use_pool { 500 } else { 100 };

        loop {
            if *shutdown_rx.borrow() {
                info!("Shutdown requested, stopping sync");
                break;
            }

            if use_pool {
                // Multi-peer mode: collect headers from primary, fetch blocks from pool
                tokio::select! {
                    result = client.request_headers_batch(header_batch_size) => {
                        match result {
                            Ok(batch_result) => {
                                match batch_result {
                                    HeaderBatchResult::Headers(headers, tip) => {
                                        let fetch_start = std::time::Instant::now();
                                        let header_count = headers.len() as u64;
                                        match fetch_pool.fetch_blocks_concurrent(&headers).await {
                                            Ok(blocks) => {
                                                let fetch_ms = fetch_start.elapsed().as_secs_f64() * 1000.0;
                                                self.peer_manager.write().await.record_block_fetch(
                                                    &peer_addr, fetch_ms, header_count, 0,
                                                );
                                                self.process_forward_blocks(blocks, &tip, &mut blocks_received, &mut blocks_since_last_log, &mut last_snapshot_epoch, &mut last_log_time, &mut last_query_update).await;
                                            }
                                            Err(e) => {
                                                warn!("Pool fetch failed, falling back to primary peer: {e}");
                                                match client.fetch_blocks_by_points(&headers).await {
                                                    Ok(blocks) => {
                                                        self.process_forward_blocks(blocks, &tip, &mut blocks_received, &mut blocks_since_last_log, &mut last_snapshot_epoch, &mut last_log_time, &mut last_query_update).await;
                                                    }
                                                    Err(e2) => { error!("Primary peer fetch also failed: {e2}"); break; }
                                                }
                                            }
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
                                        let mut db = self.chain_db.write().await;
                                        if let Err(e) = db.rollback_to_point(&rollback_point) {
                                            error!("Rollback failed: {e}");
                                        }
                                    }
                                    HeaderBatchResult::RollBackward(point, _tip) => {
                                        warn!("Rollback to {point}");
                                        let mut db = self.chain_db.write().await;
                                        if let Err(e) = db.rollback_to_point(&point) {
                                            error!("Rollback failed: {e}");
                                        }
                                    }
                                    HeaderBatchResult::Await => {
                                        info!(blocks_received, "Caught up to chain tip, awaiting new blocks");
                                        self.update_query_state().await;
                                        self.try_forge_block().await;
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
                                    let blocks: Vec<_> = forward_blocks.iter().map(|(b, _)| b.clone()).collect();
                                    let tip = &forward_blocks.last().unwrap().1;
                                    self.process_forward_blocks(blocks, tip, &mut blocks_received, &mut blocks_since_last_log, &mut last_snapshot_epoch, &mut last_log_time, &mut last_query_update).await;
                                }

                                for event in other_events {
                                    match event {
                                        ChainSyncEvent::RollBackward(point, tip) => {
                                            warn!("Rollback to {point}, tip: {tip}");
                                            let mut db = self.chain_db.write().await;
                                            if let Err(e) = db.rollback_to_point(&point) { error!("Rollback failed: {e}"); }
                                        }
                                        ChainSyncEvent::Await => {
                                            info!(blocks_received, "Caught up to chain tip, awaiting new blocks");
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
        blocks: Vec<torsten_primitives::block::Block>,
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
        let batch_count = blocks.len() as u64;

        {
            let batch: Vec<_> = blocks
                .iter()
                .map(|block| {
                    (
                        *block.hash(),
                        block.slot(),
                        block.block_number(),
                        *block.prev_hash(),
                        block.raw_cbor.clone().unwrap_or_default(),
                    )
                })
                .collect();
            let mut db = self.chain_db.write().await;
            if let Err(e) = db.add_blocks_batch(&batch) {
                error!("Failed to store block batch: {e}");
            }
        }

        {
            let mut ls = self.ledger_state.write().await;
            for block in &blocks {
                if let Err(e) = ls.apply_block(block) {
                    error!("Failed to apply block to ledger: {e}");
                }
            }
        }

        // Remove confirmed transactions from mempool
        if !self.mempool.is_empty() {
            let confirmed_hashes: Vec<_> = blocks
                .iter()
                .flat_map(|b| b.transactions.iter().map(|tx| tx.hash))
                .collect();
            if !confirmed_hashes.is_empty() {
                self.mempool.remove_txs(&confirmed_hashes);
            }
        }

        if let Some(last_block) = blocks.last() {
            if last_block.era.is_shelley_based() {
                if let Err(e) = self
                    .consensus
                    .validate_header(&last_block.header, last_block.slot())
                {
                    warn!(
                        slot = last_block.slot().0,
                        "Consensus validation warning: {e}"
                    );
                }
            }
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
                let relays: Vec<String> = reg
                    .relays
                    .iter()
                    .map(|r| match r {
                        torsten_primitives::transaction::Relay::SingleHostAddr {
                            port,
                            ipv4,
                            ..
                        } => {
                            let ip = ipv4
                                .map(|a| format!("{}.{}.{}.{}", a[0], a[1], a[2], a[3]))
                                .unwrap_or_default();
                            format!("{}:{}", ip, port.unwrap_or(0))
                        }
                        torsten_primitives::transaction::Relay::SingleHostName {
                            port,
                            dns_name,
                        } => format!("{}:{}", dns_name, port.unwrap_or(0)),
                        torsten_primitives::transaction::Relay::MultiHostName { dns_name } => {
                            dns_name.clone()
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
                    reward_account: Vec::new(), // Not tracked in PoolRegistration yet
                    owners: Vec::new(),         // Not tracked in PoolRegistration yet
                    relays,
                    metadata_url: None,  // Not tracked in PoolRegistration yet
                    metadata_hash: None, // Not tracked in PoolRegistration yet
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
            stake_snapshots,
            pool_params_entries,
        };

        // Drop the ledger read lock before acquiring the query handler write lock
        drop(ls);

        let mut handler = self.query_handler.write().await;
        handler.update_state(snapshot);
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

                info!(
                    slot = next_slot.0,
                    block_number = block_number.0,
                    tx_count = block.transactions.len(),
                    "Forged block applied to local chain"
                );

                // Note: Block announcement to peers is not yet implemented.
                // The N2N server will serve the block to peers that request it via BlockFetch.
            }
            Err(e) => {
                error!("Block forging failed: {e}");
            }
        }
    }
}
