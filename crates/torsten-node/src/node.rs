use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::signal;
use tokio::sync::{watch, RwLock};
use tracing::{debug, error, info, warn};

use torsten_consensus::praos::BlockIssuerInfo;
use torsten_consensus::OuroborosPraos;
use torsten_ledger::LedgerState;
use torsten_mempool::{Mempool, MempoolConfig};
use torsten_network::query_handler::{UtxoQueryProvider, UtxoSnapshot};
use torsten_network::server::NodeServerConfig;
use torsten_network::{
    BlockFetchPool, BlockProvider, ChainSyncEvent, DiffusionMode, HeaderBatchResult, N2CServer,
    NodeServer, NodeStateSnapshot, NodeToNodeClient, PeerManager, PeerManagerConfig,
    PipelinedPeerClient, QueryHandler, TipInfo, TxValidator,
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
        let db = self.chain_db.blocking_read();
        db.get_block(&block_hash).ok().flatten()
    }

    fn has_block(&self, hash: &[u8; 32]) -> bool {
        let block_hash = torsten_primitives::hash::Hash32::from_bytes(*hash);
        let db = self.chain_db.blocking_read();
        db.has_block(&block_hash)
    }

    fn get_tip(&self) -> TipInfo {
        let db = self.chain_db.blocking_read();
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
        TipInfo {
            slot,
            hash,
            block_number: block_no,
        }
    }

    fn get_next_block_after_slot(&self, after_slot: u64) -> Option<(u64, [u8; 32], Vec<u8>)> {
        let db = self.chain_db.blocking_read();
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

/// Convert an f64 to a (numerator, denominator) rational approximation.
/// Handles common Cardano genesis values like 0.05 → (1, 20).
/// Return the number of Byron epochs before the Shelley hard fork for known
/// Cardano networks, identified by network magic.
///
/// Based on CNCLI's `guess_shelley_transition_epoch`.
fn shelley_transition_epoch_for_magic(network_magic: u64) -> u64 {
    match network_magic {
        764824073 => 208, // mainnet
        1 => 4,           // preprod
        2 => 0,           // preview (no Byron era)
        4 => 0,           // sanchonet
        141 => 2,         // guild
        _ => 0,           // unknown — assume no Byron era (safest default)
    }
}

fn float_to_rational(f: f64) -> (u64, u64) {
    if f == 0.0 {
        return (0, 1);
    }
    if f == 1.0 {
        return (1, 1);
    }
    // Try to find exact fraction with small denominators first
    for den in 1..=10000u64 {
        let num = (f * den as f64).round() as u64;
        let reconstructed = num as f64 / den as f64;
        if (reconstructed - f).abs() < 1e-12 {
            // Simplify by GCD
            let g = gcd(num, den);
            return (num / g, den / g);
        }
    }
    // Fallback: use large denominator
    let den = 1_000_000u64;
    let num = (f * den as f64).round() as u64;
    let g = gcd(num, den);
    (num / g, den / g)
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
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

/// Validate genesis blocks against expected hashes from the configuration.
///
/// When syncing from genesis (Origin), the first blocks received are the genesis
/// blocks for the chain. For Byron-era networks (mainnet, preprod), the first
/// block is a Byron Epoch Boundary Block (EBB) whose hash must match the
/// expected Byron genesis hash. For networks that start directly in the Shelley
/// era (preview), the first block's prev_hash should match the expected Shelley
/// genesis hash.
///
/// This validation is crucial to ensure we are syncing the correct chain and
/// not connecting to a peer serving a different network's blocks.
pub fn validate_genesis_blocks(
    blocks: &[torsten_primitives::block::Block],
    expected_byron_hash: Option<&torsten_primitives::hash::Hash32>,
    expected_shelley_hash: Option<&torsten_primitives::hash::Hash32>,
) -> Result<()> {
    if blocks.is_empty() {
        return Ok(());
    }

    let first_block = &blocks[0];

    // Only validate if we're starting from genesis (block 0 at slot 0).
    // If ChainDB already has blocks, genesis was validated on a prior run.
    if first_block.block_number().0 != 0 {
        info!(
            block_no = first_block.block_number().0,
            slot = first_block.slot().0,
            "Skipping genesis validation — not syncing from genesis"
        );
        return Ok(());
    }

    // For Byron-era chains, the first block is the Byron EBB (block 0, slot 0).
    // Its hash must match the expected Byron genesis hash.
    if first_block.era == torsten_primitives::era::Era::Byron {
        if let Some(expected) = expected_byron_hash {
            let actual = first_block.hash();
            if actual != expected {
                return Err(anyhow::anyhow!(
                    "Byron genesis block hash mismatch: expected {}, got {} — \
                     this chain does not match the configured genesis. \
                     Check that you are connecting to the correct network.",
                    expected.to_hex(),
                    actual.to_hex()
                ));
            }
            info!(
                hash = %actual.to_hex(),
                "Byron genesis block validated successfully"
            );
        } else {
            warn!("No Byron genesis hash configured — skipping Byron genesis block validation");
        }
    }

    // For Shelley-first chains (e.g., preview testnet), the first block may be
    // a Shelley-era block. Its prev_hash points to the Shelley genesis hash.
    if first_block.era.is_shelley_based() && first_block.block_number().0 == 0 {
        if let Some(expected) = expected_shelley_hash {
            let prev_hash = first_block.prev_hash();
            if prev_hash != expected {
                return Err(anyhow::anyhow!(
                    "Shelley genesis hash mismatch: expected {}, but first block's \
                     prev_hash is {} — this chain does not match the configured genesis. \
                     Check that you are connecting to the correct network.",
                    expected.to_hex(),
                    prev_hash.to_hex()
                ));
            }
            info!(
                hash = %expected.to_hex(),
                "Shelley genesis block reference validated successfully"
            );
        } else {
            warn!("No Shelley genesis hash configured — skipping Shelley genesis block validation");
        }
    }

    Ok(())
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
    /// Byron epoch length in absolute slots (10 * k). For correct slot
    /// computation on non-mainnet networks.
    byron_epoch_length: u64,
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
    /// Expected Blake2b-256 hash of the Byron genesis block (from config or computed from file)
    expected_byron_genesis_hash: Option<torsten_primitives::hash::Hash32>,
    /// Expected Blake2b-256 hash of the Shelley genesis block (from config or computed from file)
    expected_shelley_genesis_hash: Option<torsten_primitives::hash::Hash32>,
    /// Whether genesis block validation has been performed (only need to validate once)
    genesis_validated: bool,
    /// Count of epoch transitions observed since node startup.
    /// Used to determine when the epoch nonce is reliable for VRF verification.
    /// After Mithril import, we need at least 2 epoch transitions for the
    /// rolling nonce to be correctly accumulated.
    epoch_transitions_observed: u32,
}

impl Node {
    pub fn new(args: NodeArgs) -> Result<Self> {
        let chain_db = Arc::new(RwLock::new(ChainDB::open(&args.database_path)?));
        info!("ChainDB opened at {}", args.database_path.display());

        let mut protocol_params = ProtocolParameters::mainnet_defaults();

        // Load Byron genesis if configured
        let config_dir = args.config_dir.clone();
        let mut byron_epoch_length: u64 = 0; // 0 = use pallas defaults (mainnet)
        let mut byron_genesis_file_hash: Option<torsten_primitives::hash::Hash32> = None;
        let byron_genesis_utxos: Vec<(Vec<u8>, u64)> =
            if let Some(ref genesis_path) = args.config.byron_genesis_file {
                let genesis_path = config_dir.join(genesis_path);
                match ByronGenesis::load_with_hash(&genesis_path) {
                    Ok((genesis, hash)) => {
                        let utxos = genesis.initial_utxos();
                        let k = genesis.security_param();
                        byron_epoch_length = 10 * k;
                        info!(
                            protocol_magic = genesis.protocol_magic(),
                            security_param = k,
                            byron_epoch_length,
                            initial_utxos = utxos.len(),
                            "Byron genesis loaded"
                        );
                        byron_genesis_file_hash = Some(hash);
                        utxos.into_iter().map(|e| (e.address, e.lovelace)).collect()
                    }
                    Err(e) => {
                        warn!("Failed to load Byron genesis: {e}");
                        Vec::new()
                    }
                }
            } else {
                Vec::new()
            };

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
        let mut conway_committee_threshold: Option<(u64, u64)> = None;
        let mut conway_committee_members: Vec<([u8; 32], u64)> = Vec::new();
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
                    conway_committee_threshold = genesis.committee_threshold();
                    conway_committee_members = genesis.committee_members();
                    genesis.apply_to_protocol_params(&mut protocol_params);
                }
                Err(e) => {
                    warn!("Failed to load Conway genesis: {e}");
                }
            }
        }

        // Compute network magic early — needed for shelley transition epoch lookup
        let network_magic = args.config.network_magic.unwrap_or_else(|| {
            if let Some(ref sg) = shelley_genesis {
                sg.network_magic
            } else {
                args.config.network.magic()
            }
        });

        // Try to load existing ledger snapshot
        let snapshot_path = args.database_path.join("ledger-snapshot.bin");
        let mut ledger = if snapshot_path.exists() {
            match LedgerState::load_snapshot(&snapshot_path) {
                Ok(mut state) => {
                    // Re-apply genesis config in case it changed
                    if let Some(ref genesis) = shelley_genesis {
                        state.set_epoch_length(genesis.epoch_length, genesis.security_param);
                        state.set_slot_config(genesis.slot_config());
                        state.set_update_quorum(genesis.update_quorum);
                    }
                    let ste = shelley_transition_epoch_for_magic(network_magic);
                    state.set_shelley_transition(ste, byron_epoch_length);
                    if let Some(hash) = shelley_genesis_hash {
                        state.genesis_hash = hash;
                    }
                    // Validate snapshot tip exists in ChainDB. If the tip hash
                    // is not in storage (e.g., DB was wiped, different chain, or
                    // volatile blocks were lost), the snapshot is stale and must
                    // be discarded to prevent "block does not connect" errors.
                    let snapshot_valid = match state.tip.point {
                        Point::Origin => true,
                        Point::Specific(_, ref hash) => {
                            // Use try_read() since we're in a sync context within tokio.
                            // The lock was just created so this will always succeed.
                            match chain_db.try_read() {
                                Ok(db) => {
                                    let exists = db.has_block(hash);
                                    if !exists {
                                        let db_tip = db.get_tip();
                                        warn!(
                                            snapshot_tip = %state.tip,
                                            chain_db_tip = %db_tip,
                                            "Ledger snapshot tip not found in ChainDB — snapshot is stale"
                                        );
                                    }
                                    exists
                                }
                                Err(_) => {
                                    warn!("Could not acquire ChainDB lock during snapshot validation — assuming valid");
                                    true
                                }
                            }
                        }
                    };

                    if snapshot_valid {
                        info!(
                            epoch = state.epoch.0,
                            utxo_count = state.utxo_set.len(),
                            tip = %state.tip,
                            "Ledger state restored from snapshot"
                        );
                        state
                    } else {
                        warn!("Discarding stale ledger snapshot — will replay from ChainDB");
                        Self::init_fresh_ledger(
                            &protocol_params,
                            shelley_genesis.as_ref(),
                            shelley_genesis_hash,
                            &byron_genesis_utxos,
                            network_magic,
                            byron_epoch_length,
                        )
                    }
                }
                Err(e) => {
                    warn!("Failed to load ledger snapshot, starting fresh: {e}");
                    Self::init_fresh_ledger(
                        &protocol_params,
                        shelley_genesis.as_ref(),
                        shelley_genesis_hash,
                        &byron_genesis_utxos,
                        network_magic,
                        byron_epoch_length,
                    )
                }
            }
        } else {
            Self::init_fresh_ledger(
                &protocol_params,
                shelley_genesis.as_ref(),
                shelley_genesis_hash,
                &byron_genesis_utxos,
                network_magic,
                byron_epoch_length,
            )
        };
        // Apply Conway genesis committee threshold and members if not already set
        if let Some((num, den)) = conway_committee_threshold {
            if ledger.governance.committee_threshold.is_none() {
                use torsten_primitives::transaction::Rational;
                ledger.governance.committee_threshold = Some(Rational {
                    numerator: num,
                    denominator: den,
                });
                info!(
                    numerator = num,
                    denominator = den,
                    "Applied Conway genesis committee quorum threshold"
                );
            }
        }
        // Seed initial committee members from Conway genesis if committee is empty
        if ledger.governance.committee_expiration.is_empty() && !conway_committee_members.is_empty()
        {
            use torsten_primitives::hash::Hash32;
            for (hash_bytes, expiration) in &conway_committee_members {
                let cold_key = Hash32::from_bytes(*hash_bytes);
                ledger
                    .governance
                    .committee_expiration
                    .insert(cold_key, torsten_primitives::EpochNo(*expiration));
            }
            info!(
                count = conway_committee_members.len(),
                "Seeded initial committee members from Conway genesis"
            );
        }
        let ledger_state = Arc::new(RwLock::new(ledger));
        info!("Ledger state initialized");

        let consensus = if let Some(ref genesis) = shelley_genesis {
            OuroborosPraos::with_genesis_params(
                genesis.active_slots_coeff,
                genesis.security_param,
                torsten_primitives::time::EpochLength(genesis.epoch_length),
                genesis.slots_per_k_e_s_period,
                genesis.max_k_e_s_evolutions,
            )
        } else {
            OuroborosPraos::new()
        };
        info!(
            epoch_length = consensus.epoch_length.0,
            security_param = consensus.security_param,
            active_slot_coeff = consensus.active_slot_coeff,
            slots_per_kes_period = consensus.slots_per_kes_period,
            max_kes_evolutions = consensus.max_kes_evolutions,
            "Ouroboros Praos consensus initialized"
        );

        let mempool = Arc::new(Mempool::new(MempoolConfig::default()));
        info!("Mempool initialized");

        let socket_path = args.socket_path.clone();
        let listen_addr: std::net::SocketAddr =
            format!("{}:{}", args.host_addr, args.port).parse()?;
        // network_magic computed earlier (before ledger snapshot loading)
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

        // Determine expected genesis hashes for genesis block validation.
        // Config hash fields take priority (ByronGenesisHash, ShelleyGenesisHash);
        // fall back to hashes computed from the genesis files themselves.
        let expected_byron_genesis_hash = args
            .config
            .byron_genesis_hash
            .as_deref()
            .and_then(|h| torsten_primitives::hash::Hash32::from_hex(h).ok())
            .or(byron_genesis_file_hash);
        let expected_shelley_genesis_hash = args
            .config
            .shelley_genesis_hash
            .as_deref()
            .and_then(|h| torsten_primitives::hash::Hash32::from_hex(h).ok())
            .or(shelley_genesis_hash);

        if let Some(ref h) = expected_byron_genesis_hash {
            info!(hash = %h.to_hex(), "Expected Byron genesis hash");
        }
        if let Some(ref h) = expected_shelley_genesis_hash {
            info!(hash = %h.to_hex(), "Expected Shelley genesis hash");
        }

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
            byron_epoch_length,
            shelley_genesis,
            topology_path: args.topology_path,
            metrics: Arc::new(crate::metrics::NodeMetrics::new()),
            block_producer,
            block_announcement_tx: None,
            rollback_announcement_tx: None,
            metrics_port: args.metrics_port,
            expected_byron_genesis_hash,
            expected_shelley_genesis_hash,
            genesis_validated: false,
            epoch_transitions_observed: 0,
        })
    }

    pub async fn run(&mut self) -> Result<()> {
        let tip = self.chain_db.read().await.get_tip();
        info!("Current chain tip: {tip}");

        // If ChainDB already has blocks, genesis was validated on a prior run
        if tip.point != Point::Origin {
            self.genesis_validated = true;
        }

        {
            let ls = self.ledger_state.read().await;
            info!("UTxO set size: {} entries", ls.utxo_set.len());
        }
        info!("Mempool: {} transactions", self.mempool.len());

        // Replay blocks from ChainDB if the ledger is behind storage.
        // This happens after a Mithril snapshot import — blocks are in storage
        // but the ledger hasn't processed them yet.
        self.replay_ledger_from_storage().await;

        // Initialize query state from current ledger so N2C queries
        // work immediately (before we reach chain tip or the periodic timer fires)
        self.update_query_state().await;
        info!("Query state initialized from current ledger state");

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

        // Start disk space monitor on the database volume
        {
            let db_path = self.database_path.clone();
            let metrics = self.metrics.clone();
            let disk_shutdown_rx = shutdown_rx.clone();
            tokio::spawn(async move {
                crate::disk_monitor::start_disk_monitor(db_path, metrics, disk_shutdown_rx).await;
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
                match tokio::net::lookup_host(format!("{}:{}", peer.address, peer.port)).await {
                    Ok(addrs) => {
                        for socket_addr in addrs {
                            pm.add_config_peer(socket_addr, peer.trustable, peer.advertise);
                        }
                    }
                    Err(e) => {
                        warn!(
                            address = %peer.address,
                            port = peer.port,
                            "Failed to resolve peer address: {e}"
                        );
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
                                match tokio::net::lookup_host(format!(
                                    "{}:{}",
                                    peer.address, peer.port
                                ))
                                .await
                                {
                                    Ok(addrs) => {
                                        for socket_addr in addrs {
                                            pm.add_config_peer(
                                                socket_addr,
                                                peer.trustable,
                                                peer.advertise,
                                            );
                                            added += 1;
                                        }
                                    }
                                    Err(e) => {
                                        warn!(
                                            address = %peer.address,
                                            port = peer.port,
                                            "Failed to resolve peer address during topology reload: {e}"
                                        );
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
                        Ok(mut c) => {
                            c.set_byron_epoch_length(self.byron_epoch_length);
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
                        Ok(mut c) => {
                            c.set_byron_epoch_length(self.byron_epoch_length);
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
                        Ok(mut c) => {
                            c.set_byron_epoch_length(self.byron_epoch_length);
                            let rtt_ms = connect_start.elapsed().as_secs_f64() * 1000.0;
                            let mut pm = peer_manager.write().await;
                            pm.peer_connected(addr, 14, true);
                            pm.record_handshake_rtt(addr, rtt_ms);
                            pm.promote_to_hot(addr);
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
                        Ok(mut c) => {
                            c.set_byron_epoch_length(self.byron_epoch_length);
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
                    Ok(mut pc) => {
                        pc.set_byron_epoch_length(self.byron_epoch_length);
                        info!("Pipelined ChainSync client connected to {target}");
                        // Take the TxSubmission channel and spawn a background tx fetcher
                        if let Some(txsub_channel) = pc.take_txsub_channel() {
                            let mempool = self.mempool.clone();
                            let ledger = self.ledger_state.clone();
                            let slot_config = self.ledger_state.read().await.slot_config;
                            let shutdown = shutdown_rx.clone();
                            tokio::spawn(async move {
                                let validator: Option<Arc<dyn TxValidator>> =
                                    Some(Arc::new(LedgerTxValidator {
                                        ledger,
                                        slot_config,
                                    }));
                                let mut client =
                                    torsten_network::TxSubmissionClient::new(txsub_channel);
                                let mut shutdown = shutdown;
                                tokio::select! {
                                    result = client.run(mempool, validator) => {
                                        match result {
                                            Ok(stats) => {
                                                info!(
                                                    received = stats.received,
                                                    accepted = stats.accepted,
                                                    rejected = stats.rejected,
                                                    duplicate = stats.duplicate,
                                                    "TxSubmission2 client session ended"
                                                );
                                            }
                                            Err(e) => {
                                                debug!("TxSubmission2 client error: {e}");
                                            }
                                        }
                                        // Keep the client (and its AgentChannel) alive until
                                        // the connection is closed. Dropping the channel would
                                        // cause the demuxer to crash when the peer sends a
                                        // delayed response on the TxSubmission2 protocol.
                                        shutdown.changed().await.ok();
                                    }
                                    _ = shutdown.changed() => {
                                        debug!("TxSubmission2 client: shutdown");
                                    }
                                }
                            });
                        }
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

        // Persist ChainDB to disk BEFORE saving ledger snapshot
        // (ensures ledger snapshot is consistent with persisted blocks)
        {
            let mut db = self.chain_db.write().await;
            if let Err(e) = db.persist() {
                error!("Failed to persist ChainDB on shutdown: {e}");
            }
        }
        self.save_ledger_snapshot().await;
        info!("Node shutdown complete");
        Ok(())
    }

    /// Save a ledger state snapshot to the database directory
    async fn save_ledger_snapshot(&self) {
        let ls = self.ledger_state.read().await;
        let epoch = ls.epoch.0;

        // Save epoch-numbered snapshot for rollback safety
        let epoch_path = self
            .database_path
            .join(format!("ledger-snapshot-epoch{epoch}.bin"));
        if let Err(e) = ls.save_snapshot(&epoch_path) {
            error!("Failed to save ledger snapshot: {e}");
            return;
        }

        // Also save as the "latest" snapshot for fast startup
        let latest_path = self.database_path.join("ledger-snapshot.bin");
        if let Err(e) = ls.save_snapshot(&latest_path) {
            error!("Failed to save latest ledger snapshot: {e}");
        }

        drop(ls);

        // Prune old snapshots — keep only the 3 most recent
        self.prune_old_snapshots(3);
    }

    /// Create a fresh ledger state with genesis configuration applied.
    fn init_fresh_ledger(
        protocol_params: &ProtocolParameters,
        shelley_genesis: Option<&ShelleyGenesis>,
        shelley_genesis_hash: Option<torsten_primitives::Hash32>,
        byron_genesis_utxos: &[(Vec<u8>, u64)],
        network_magic: u64,
        byron_epoch_length: u64,
    ) -> LedgerState {
        let mut ledger = LedgerState::new(protocol_params.clone());
        if let Some(genesis) = shelley_genesis {
            ledger.set_epoch_length(genesis.epoch_length, genesis.security_param);
            ledger.set_slot_config(genesis.slot_config());
            ledger.set_update_quorum(genesis.update_quorum);
        }
        // Set Byron→Shelley transition boundary for correct HFC epoch numbering
        let shelley_transition_epoch = shelley_transition_epoch_for_magic(network_magic);
        ledger.set_shelley_transition(shelley_transition_epoch, byron_epoch_length);
        if let Some(hash) = shelley_genesis_hash {
            ledger.set_genesis_hash(hash);
        }
        if !byron_genesis_utxos.is_empty() {
            ledger.seed_genesis_utxos(byron_genesis_utxos);
        }
        ledger
    }

    /// Remove old epoch snapshots, keeping only the N most recent.
    fn prune_old_snapshots(&self, keep: usize) {
        let mut snapshots: Vec<(u64, std::path::PathBuf)> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.database_path) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if let Some(rest) = name_str.strip_prefix("ledger-snapshot-epoch") {
                    if let Some(epoch_str) = rest.strip_suffix(".bin") {
                        if let Ok(epoch) = epoch_str.parse::<u64>() {
                            snapshots.push((epoch, entry.path()));
                        }
                    }
                }
            }
        }
        if snapshots.len() > keep {
            snapshots.sort_by_key(|(epoch, _)| *epoch);
            let to_remove = snapshots.len() - keep;
            for (epoch, path) in snapshots.into_iter().take(to_remove) {
                if let Err(e) = std::fs::remove_file(&path) {
                    warn!(epoch, "Failed to remove old snapshot: {e}");
                } else {
                    debug!(epoch, "Pruned old ledger snapshot");
                }
            }
        }
    }

    /// Find the best epoch snapshot for a rollback to the given slot.
    /// Returns the path to the most recent snapshot whose ledger tip is at or before `rollback_slot`.
    /// Falls back to `ledger-snapshot.bin` if no epoch snapshot qualifies.
    fn find_best_snapshot_for_rollback(&self, rollback_slot: u64) -> Option<std::path::PathBuf> {
        // Collect all epoch-numbered snapshots (sorted newest first)
        let mut epoch_snapshots: Vec<(u64, std::path::PathBuf)> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.database_path) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if let Some(rest) = name_str.strip_prefix("ledger-snapshot-epoch") {
                    if let Some(epoch_str) = rest.strip_suffix(".bin") {
                        if let Ok(epoch) = epoch_str.parse::<u64>() {
                            epoch_snapshots.push((epoch, entry.path()));
                        }
                    }
                }
            }
        }
        // Sort by epoch descending (newest first)
        epoch_snapshots.sort_by(|a, b| b.0.cmp(&a.0));

        // Try each epoch snapshot to find one at or before the rollback slot.
        // We need to actually load the snapshot to check its slot (epoch number alone
        // isn't enough since the snapshot slot could be anywhere in the epoch).
        // To avoid loading huge snapshots just to check, use a heuristic:
        // epoch * epoch_length gives approximate slot. If epoch is clearly too new, skip.
        let epoch_length = {
            // Use a rough estimate; we don't need exact precision here
            if let Some(ref genesis) = self.shelley_genesis {
                genesis.epoch_length
            } else {
                86400
            }
        };

        for (epoch, path) in &epoch_snapshots {
            // Heuristic: if epoch * epoch_length > rollback_slot + epoch_length, skip
            // (snapshot is definitely beyond the rollback point)
            let approx_slot = epoch * epoch_length;
            if approx_slot > rollback_slot + epoch_length {
                continue;
            }

            // This snapshot might work — try loading to check exact slot
            match torsten_ledger::LedgerState::load_snapshot(path) {
                Ok(state) => {
                    let snap_slot = state.tip.point.slot().map(|s| s.0).unwrap_or(0);
                    if snap_slot <= rollback_slot {
                        debug!(
                            epoch,
                            snap_slot, rollback_slot, "Found suitable epoch snapshot for rollback"
                        );
                        return Some(path.clone());
                    }
                }
                Err(e) => {
                    warn!(epoch, "Failed to load epoch snapshot: {e}");
                }
            }
        }

        // Fall back to latest snapshot
        let latest = self.database_path.join("ledger-snapshot.bin");
        if latest.exists() {
            // Check if it's usable (at or before rollback point)
            if let Ok(state) = torsten_ledger::LedgerState::load_snapshot(&latest) {
                let snap_slot = state.tip.point.slot().map(|s| s.0).unwrap_or(0);
                if snap_slot <= rollback_slot {
                    return Some(latest);
                }
            }
        }

        None
    }

    /// Replay blocks from local storage to catch the ledger up to the chain tip.
    ///
    /// After a Mithril snapshot import, ChainDB contains millions of blocks
    /// but the ledger state starts from genesis. This replays blocks locally
    /// (no network needed).
    ///
    /// Two replay modes:
    /// 1. **Chunk file replay** (fast path): If `immutable-replay/` exists in the
    ///    database directory (left by Mithril import), reads blocks sequentially
    ///    from chunk files. This is ~100x faster than LSM lookups because chunk
    ///    files are laid out sequentially on disk.
    /// 2. **LSM replay** (fallback): Reads blocks by block number from the LSM tree.
    ///    Slower due to random I/O but works when chunk files aren't available.
    async fn replay_ledger_from_storage(&self) {
        // Migrate legacy immutable-replay/ to immutable/ (backwards compat)
        let legacy_dir = self.database_path.join("immutable-replay");
        let immutable_dir = self.database_path.join("immutable");
        if legacy_dir.is_dir() && !immutable_dir.is_dir() {
            info!("Migrating legacy immutable-replay/ to immutable/");
            if let Err(e) = std::fs::rename(&legacy_dir, &immutable_dir) {
                warn!(error = %e, "Failed to migrate, will use legacy path");
            }
        }

        // Check for chunk files — ImmutableDB provides permanent historical
        // block storage from Mithril. Chunk files are NOT deleted after replay.
        let chunk_dir = if immutable_dir.is_dir() {
            Some(immutable_dir)
        } else if legacy_dir.is_dir() {
            Some(legacy_dir)
        } else {
            None
        };
        if let Some(ref dir) = chunk_dir {
            let ledger_slot = {
                let ls = self.ledger_state.read().await;
                ls.tip.point.slot().map(|s| s.0).unwrap_or(0)
            };
            // Only replay if the ledger hasn't caught up to the immutable tip
            let imm_tip_slot = self
                .chain_db
                .read()
                .await
                .get_tip()
                .point
                .slot()
                .map(|s| s.0)
                .unwrap_or(0);
            if ledger_slot < imm_tip_slot {
                info!(
                    ledger_slot,
                    imm_tip_slot, "Ledger behind ImmutableDB — replaying from chunk files"
                );
                self.replay_from_chunk_files(dir).await;
                return;
            }
        }

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

        // Check if the user wants to limit replay via environment variable.
        let replay_limit: u64 = std::env::var("TORSTEN_REPLAY_LIMIT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(u64::MAX);

        if blocks_behind > replay_limit {
            warn!(
                blocks_behind,
                replay_limit,
                db_tip_slot,
                ledger_slot,
                "Skipping ledger replay: gap exceeds TORSTEN_REPLAY_LIMIT. \
                 Set TORSTEN_REPLAY_LIMIT to a higher value or remove it to replay all blocks."
            );
            return;
        }

        if blocks_behind > 100_000 {
            info!(
                blocks_behind,
                "Large ledger replay starting — this may take a while. \
                 Snapshots will be saved every 500k blocks. \
                 Set TORSTEN_REPLAY_LIMIT=0 to skip replay."
            );
        }

        info!(
            ledger_slot,
            db_tip_slot,
            blocks_behind,
            "Ledger is behind ChainDB — replaying blocks from local storage"
        );

        // Fallback: replay from LSM tree by block number
        info!("No chunk files found, replaying from LSM tree (slower)");
        self.replay_from_lsm(db_tip).await;
    }

    /// Fast replay: read blocks sequentially from chunk files.
    ///
    /// Runs in a blocking thread since chunk file I/O and ledger application
    /// are CPU-bound synchronous work.
    async fn replay_from_chunk_files(&self, replay_dir: &std::path::Path) {
        let ledger_state = self.ledger_state.clone();
        let snapshot_path = self.database_path.join("ledger-snapshot.bin");
        let replay_dir = replay_dir.to_path_buf();
        let bel = self.byron_epoch_length;

        let result = tokio::task::spawn_blocking(move || {
            let start = std::time::Instant::now();
            let mut replayed = 0u64;
            let mut last_log = std::time::Instant::now();

            // Disable address index and full stake rebuild during replay.
            // Address index is never queried during replay, and the O(n)
            // retain per remove is expensive. Both are rebuilt at the end.
            // Incremental stake tracking is correct during sequential replay.
            {
                let mut ls = ledger_state.blocking_write();
                ls.utxo_set.set_indexing_enabled(false);
                ls.needs_stake_rebuild = false;
            }

            let result = crate::mithril::replay_from_chunk_files(&replay_dir, |cbor| {
                match torsten_serialization::multi_era::decode_block_with_byron_epoch_length(
                    cbor, bel,
                ) {
                    Ok(block) => {
                        let mut ls_guard = ledger_state.blocking_write();
                        if let Err(e) = ls_guard.apply_block(&block) {
                            warn!(slot = block.slot().0, "Ledger replay apply failed: {e}");
                        }
                        replayed += 1;

                        if last_log.elapsed().as_secs() >= 5 {
                            let elapsed = start.elapsed().as_secs_f64();
                            let speed = replayed as f64 / elapsed;
                            let slot = ls_guard.tip.point.slot().map(|s| s.0).unwrap_or(0);
                            let utxos = ls_guard.utxo_set.len();
                            info!(
                                "Replaying | {replayed} blocks | slot {slot} \
                                 | {speed:.0} blocks/s | {utxos} UTxOs"
                            );
                            last_log = std::time::Instant::now();
                        }

                        if replayed.is_multiple_of(500_000) {
                            if let Err(e) = ls_guard.save_snapshot(&snapshot_path) {
                                warn!("Failed to save ledger snapshot during replay: {e}");
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Failed to decode block during chunk replay: {e}");
                    }
                }
                Ok(())
            });

            match &result {
                Ok(total) => {
                    let elapsed = start.elapsed().as_secs_f64();
                    let speed = if elapsed > 0.0 {
                        *total as f64 / elapsed
                    } else {
                        0.0
                    };
                    info!(
                        total,
                        elapsed_secs = elapsed as u64,
                        speed = speed as u64,
                        "Chunk-file replay complete"
                    );
                }
                Err(e) => {
                    error!("Chunk-file replay failed: {e}");
                }
            }

            // Re-enable address indexing and rebuild the index
            {
                let mut ls = ledger_state.blocking_write();
                ls.utxo_set.set_indexing_enabled(true);
                info!("Rebuilding address index after replay...");
                ls.utxo_set.rebuild_address_index();
                info!("Address index rebuilt");
                // Also mark that we need a full stake rebuild now that replay is done
                ls.needs_stake_rebuild = true;
            }

            // Save final snapshot
            {
                let ls = ledger_state.blocking_read();
                if let Err(e) = ls.save_snapshot(&snapshot_path) {
                    error!("Failed to save ledger snapshot after replay: {e}");
                }
            }

            result
        })
        .await;

        if let Err(e) = result {
            error!("Chunk-file replay task panicked: {e}");
        }
    }

    /// Fallback replay: read blocks from LSM tree by block number.
    async fn replay_from_lsm(&self, db_tip: torsten_primitives::block::Tip) {
        let start = std::time::Instant::now();
        let mut replayed = 0u64;
        let mut last_log = std::time::Instant::now();
        let snapshot_path = self.database_path.join("ledger-snapshot.bin");

        let start_block_no = {
            let mut ls = self.ledger_state.write().await;
            ls.utxo_set.set_indexing_enabled(false);
            ls.needs_stake_rebuild = false;
            ls.tip.block_number.0 + 1
        };
        let end_block_no = db_tip.block_number.0;

        for block_no in start_block_no..=end_block_no {
            let block_data = {
                let db = self.chain_db.read().await;
                db.get_block_by_number(torsten_primitives::time::BlockNo(block_no))
            };

            match block_data {
                Ok(Some((slot, _hash, cbor))) => {
                    match torsten_serialization::multi_era::decode_block_with_byron_epoch_length(
                        &cbor,
                        self.byron_epoch_length,
                    ) {
                        Ok(block) => {
                            let mut ls = self.ledger_state.write().await;
                            if let Err(e) = ls.apply_block(&block) {
                                warn!(slot = slot.0, block_no, "Ledger replay apply failed: {e}");
                            }
                            replayed += 1;

                            if last_log.elapsed().as_secs() >= 5 {
                                let elapsed = start.elapsed().as_secs_f64();
                                let speed = replayed as f64 / elapsed;
                                let pct = if end_block_no > 0 {
                                    block_no as f64 / end_block_no as f64 * 100.0
                                } else {
                                    0.0
                                };
                                info!(
                                    "Replaying {pct:.2}% | block {block_no}/{end_block_no} \
                                     | slot {} | {speed:.0} blocks/s \
                                     | {} UTxOs",
                                    slot.0,
                                    ls.utxo_set.len()
                                );
                                last_log = std::time::Instant::now();
                            }

                            if replayed.is_multiple_of(500_000) {
                                if let Err(e) = ls.save_snapshot(&snapshot_path) {
                                    warn!("Failed to save ledger snapshot during replay: {e}");
                                }
                            }
                        }
                        Err(e) => {
                            warn!(block_no, "Failed to decode block during replay: {e}");
                        }
                    }
                }
                Ok(None) => {
                    warn!(block_no, "Block not found in ChainDB during replay");
                    break;
                }
                Err(e) => {
                    warn!(block_no, "Failed to read from ChainDB during replay: {e}");
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
            "LSM replay from local storage complete"
        );

        // Re-enable address indexing and rebuild after replay
        {
            let mut ls = self.ledger_state.write().await;
            ls.utxo_set.set_indexing_enabled(true);
            info!("Rebuilding address index after LSM replay...");
            ls.utxo_set.rebuild_address_index();
            info!("Address index rebuilt");
            ls.needs_stake_rebuild = true;
        }

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
        // Use whichever tip is further ahead as primary intersection.
        // The peer returns the first matching point, so put the furthest-ahead
        // tip first to avoid re-downloading blocks the ledger already has.
        let ledger_slot = ledger_tip.slot().map(|s| s.0).unwrap_or(0);
        let chain_slot = chain_tip.slot().map(|s| s.0).unwrap_or(0);
        if ledger_slot >= chain_slot {
            if ledger_tip != Point::Origin {
                known_points.push(ledger_tip.clone());
            }
            if chain_tip != Point::Origin && chain_tip != ledger_tip {
                known_points.push(chain_tip.clone());
            }
        } else {
            if chain_tip != Point::Origin {
                known_points.push(chain_tip.clone());
            }
            if ledger_tip != Point::Origin && ledger_tip != chain_tip {
                known_points.push(ledger_tip.clone());
            }
        }
        known_points.push(Point::Origin);
        if ledger_tip != chain_tip {
            info!(
                "Ledger tip ({}) differs from ChainDB tip ({}), syncing from furthest-ahead tip",
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
        let mut consecutive_apply_failures: u32 = 0;
        let mut last_snapshot_epoch: u64 = self.ledger_state.read().await.epoch.0;
        let mut last_log_time = std::time::Instant::now();
        let mut last_query_update = std::time::Instant::now();
        let mut blocks_since_last_log: u64 = 0;
        // Header batch size configurable via TORSTEN_HEADER_BATCH_SIZE env var
        let header_batch_size: usize = std::env::var("TORSTEN_HEADER_BATCH_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(if use_pipelined || use_pool { 500 } else { 100 });

        // Slot ticker for block production: fires every slot_length seconds
        let slot_length_secs = self
            .shelley_genesis
            .as_ref()
            .map(|g| g.slot_length)
            .unwrap_or(1);
        let mut forge_ticker =
            tokio::time::interval(tokio::time::Duration::from_secs(slot_length_secs));
        forge_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Track the last slot we checked for forging to avoid duplicate checks
        let mut last_forge_slot: u64 = 0;

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
                                        if !headers.is_empty() {
                                            debug!(
                                                header_count = headers.len(),
                                                first_slot = headers[0].slot,
                                                first_block = headers[0].block_no,
                                                last_slot = headers.last().unwrap().slot,
                                                last_block = headers.last().unwrap().block_no,
                                                "Headers received from pipelined client"
                                            );
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
                                                let applied = self.process_forward_blocks(blocks, &tip, &mut blocks_received, &mut blocks_since_last_log, &mut last_snapshot_epoch, &mut last_log_time, &mut last_query_update).await;
                                                if applied > 0 {
                                                    consecutive_apply_failures = 0;
                                                } else if header_count > 0 {
                                                    consecutive_apply_failures += 1;
                                                    if consecutive_apply_failures >= 5 {
                                                        error!(
                                                            consecutive_apply_failures,
                                                            "Ledger state diverged from chain — \
                                                             blocks do not connect. Triggering \
                                                             reconnect to re-establish intersection."
                                                        );
                                                        break;
                                                    }
                                                }
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
                                        self.enable_strict_verification().await;
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
                                    self.enable_strict_verification().await;
                                    let old = pipelined.take().expect("pipelined client is Some after is_some_and guard");
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
                    _ = forge_ticker.tick(), if self.block_producer.is_some() && pipeline_depth <= 1 => {
                        // Wall-clock slot ticker for block production.
                        // Only enabled at tip (pipeline_depth <= 1) to avoid interrupting
                        // the pipelined header fetch during bulk sync — dropping the header
                        // future mid-read loses already-consumed ChainSync responses.
                        if let Some(wc) = self.current_wall_clock_slot() {
                            if wc.0 > last_forge_slot {
                                last_forge_slot = wc.0;
                                self.try_forge_block().await;
                            }
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
                                    let tip = forward_blocks.last().expect("forward_blocks is non-empty").1.clone();
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
                                            self.enable_strict_verification().await;
                                            self.update_query_state().await;
                                        }
                                        ChainSyncEvent::RollForward(..) => unreachable!(),
                                    }
                                }
                            }
                            Err(e) => { error!("Chain sync error: {e}"); break; }
                        }
                    }
                    _ = forge_ticker.tick(), if self.block_producer.is_some() => {
                        if let Some(wc) = self.current_wall_clock_slot() {
                            if wc.0 > last_forge_slot {
                                last_forge_slot = wc.0;
                                self.try_forge_block().await;
                            }
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

    /// Validate genesis blocks against expected hashes from the configuration.
    fn validate_genesis_blocks(&self, blocks: &[torsten_primitives::block::Block]) -> Result<()> {
        validate_genesis_blocks(
            blocks,
            self.expected_byron_genesis_hash.as_ref(),
            self.expected_shelley_genesis_hash.as_ref(),
        )
    }

    /// Process a batch of forward blocks: store in ChainDB, apply to ledger, validate, log progress.
    /// Returns the number of blocks successfully applied to the ledger (0 if the first block
    /// failed connectivity, indicating a state divergence that the caller should handle).
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
    ) -> u64 {
        if blocks.is_empty() {
            return 0;
        }

        // Genesis block validation: on the very first batch of blocks received
        // during initial sync, verify that the genesis block hash matches the
        // expected hash from the configuration. This prevents syncing from a
        // chain with a different genesis (wrong network).
        if !self.genesis_validated {
            if let Err(e) = self.validate_genesis_blocks(&blocks) {
                error!("Genesis block validation failed: {e}");
                return 0;
            }
            self.genesis_validated = true;
        }

        // Validate ALL block headers BEFORE storing.
        // In strict mode (caught up to tip), validation failures reject the entire batch.
        // In non-strict mode (during sync), failures are logged but non-fatal.
        let strict = self.consensus.strict_verification();
        {
            // Read ledger state once for the whole batch
            let ls = self.ledger_state.read().await;
            let epoch_nonce = ls.epoch_nonce;

            // Per Praos spec, leader eligibility uses the "set" snapshot
            // (stake distribution from the previous epoch boundary).
            // Fall back to current pool_params if snapshots aren't available yet.
            let set_snapshot = ls.snapshots.set.as_ref();
            let total_active_stake: u64 = if let Some(snap) = set_snapshot {
                snap.pool_stake.values().map(|s| s.0).sum()
            } else {
                // During early sync, no snapshots exist yet — skip leader eligibility
                0
            };

            for block in &blocks {
                if !block.era.is_shelley_based() {
                    continue;
                }

                // Populate epoch_nonce — the wire format does not include the nonce;
                // it must be injected from ledger state before VRF verification.
                let mut header_with_nonce = block.header.clone();
                header_with_nonce.epoch_nonce = epoch_nonce;

                // Look up pool registration for VRF key binding and leader eligibility.
                // Uses "set" snapshot for stake (per Praos spec), falls back to current
                // pool_params for VRF key binding if snapshot is not available.
                let pool_id = torsten_primitives::hash::blake2b_224(&block.header.issuer_vkey);
                let issuer_info = if !block.header.issuer_vkey.is_empty() {
                    // Try set snapshot first (correct per spec)
                    let pool_reg = set_snapshot
                        .and_then(|snap| snap.pool_params.get(&pool_id))
                        .or_else(|| ls.pool_params.get(&pool_id));

                    pool_reg.map(|reg| {
                        if total_active_stake == 0 {
                            // No snapshot data — just do VRF key binding, skip leader check
                            return BlockIssuerInfo {
                                vrf_keyhash: reg.vrf_keyhash,
                                relative_stake: 1.0, // Assume eligible when no stake data
                            };
                        }
                        let pool_stake = set_snapshot
                            .and_then(|snap| snap.pool_stake.get(&pool_id))
                            .map(|s| s.0)
                            .unwrap_or(0);
                        BlockIssuerInfo {
                            vrf_keyhash: reg.vrf_keyhash,
                            relative_stake: pool_stake as f64 / total_active_stake as f64,
                        }
                    })
                } else {
                    None
                };

                if let Err(e) = self.consensus.validate_header_full(
                    &header_with_nonce,
                    block.slot(),
                    issuer_info.as_ref(),
                ) {
                    if strict {
                        error!(
                            slot = block.slot().0,
                            block_no = block.block_number().0,
                            "Consensus validation failed (strict): {e} — rejecting batch"
                        );
                        return 0;
                    }
                    warn!(
                        slot = block.slot().0,
                        block_no = block.block_number().0,
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
                return 0;
            }
        }

        // Now apply blocks to ledger — storage is confirmed
        let mut applied_count: u64 = 0;
        {
            let mut ls = self.ledger_state.write().await;
            let ledger_slot = ls.tip.point.slot().map(|s| s.0).unwrap_or(0);
            if !blocks.is_empty() {
                debug!(
                    batch_size = blocks.len(),
                    ledger_slot,
                    first_slot = blocks[0].slot().0,
                    first_block = blocks[0].block_number().0,
                    first_prev_hash = %blocks[0].prev_hash().to_hex(),
                    ledger_tip_hash = %ls.tip.point.hash().map(|h| h.to_hex()).unwrap_or_default(),
                    "Applying block batch to ledger"
                );
            }

            // Gap bridging: if the first unskipped block doesn't connect to the
            // ledger tip, try to replay intermediate blocks from ChainDB storage.
            // This handles the case where ChainDB is ahead of the ledger (e.g.,
            // after a crash mid-batch, or when blocks were stored but ledger
            // apply failed in a previous iteration).
            if let Some(first_new) = blocks.iter().find(|b| b.slot().0 > ledger_slot) {
                let ledger_tip_hash = ls.tip.point.hash().cloned();
                let first_prev = first_new.prev_hash();
                if ledger_tip_hash.as_ref() != Some(first_prev) {
                    info!(
                        ledger_slot,
                        first_block_slot = first_new.slot().0,
                        "Gap detected between ledger and incoming blocks — \
                         attempting to bridge from ChainDB"
                    );
                    let mut bridge_slot = ledger_slot;
                    let target_slot = first_new.slot().0;
                    let mut bridged = 0u64;
                    loop {
                        let block_data = {
                            let db = self.chain_db.read().await;
                            db.get_next_block_after_slot(torsten_primitives::time::SlotNo(
                                bridge_slot,
                            ))
                        };
                        match block_data {
                            Ok(Some((next_slot, _hash, cbor))) => {
                                if next_slot.0 >= target_slot {
                                    break; // Reached the incoming batch
                                }
                                match torsten_serialization::multi_era::decode_block_with_byron_epoch_length(&cbor, self.byron_epoch_length) {
                                    Ok(block) => {
                                        if let Err(e) = ls.apply_block(&block) {
                                            warn!(
                                                slot = next_slot.0,
                                                "Gap bridge apply failed: {e} — aborting bridge"
                                            );
                                            break;
                                        }
                                        bridged += 1;
                                        bridge_slot = next_slot.0;
                                    }
                                    Err(e) => {
                                        warn!(slot = next_slot.0, "Gap bridge decode failed: {e}");
                                        bridge_slot = next_slot.0;
                                    }
                                }
                            }
                            _ => break,
                        }
                    }
                    if bridged > 0 {
                        info!(bridged, "Gap bridged from ChainDB storage");
                    }
                }
            }

            let ledger_slot = ls.tip.point.slot().map(|s| s.0).unwrap_or(0);
            let ledger_tip_hash = ls.tip.point.hash().cloned();
            for block in &blocks {
                // Skip blocks the ledger has already applied (e.g. replaying from origin).
                // After a rollback/fork, a block at the same slot but with a different
                // prev_hash must NOT be skipped — it belongs to the new fork.
                if block.slot().0 <= ledger_slot {
                    let is_fork_block = ledger_tip_hash
                        .as_ref()
                        .is_some_and(|tip_hash| tip_hash == block.prev_hash());
                    if !is_fork_block {
                        continue;
                    }
                }
                if let Err(e) = ls.apply_block(block) {
                    error!(
                        slot = block.slot().0,
                        block_no = block.block_number().0,
                        hash = %block.hash().to_hex(),
                        "Failed to apply block to ledger: {e} — skipping remaining blocks in batch"
                    );
                    break;
                }
                applied_count += 1;
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

        // Take a rollback snapshot after each successful batch.
        // This allows the LSM tree to be atomically restored if a chain
        // reorganization is required.
        {
            let mut db = self.chain_db.write().await;
            db.take_rollback_snapshot();
        }

        let tx_count: u64 = blocks.iter().map(|b| b.transactions.len() as u64).sum();

        *blocks_received += batch_count;
        *blocks_since_last_log += batch_count;
        self.metrics.add_blocks_received(batch_count);
        self.metrics.add_blocks_applied(batch_count);
        self.metrics
            .transactions_received
            .fetch_add(tx_count, std::sync::atomic::Ordering::Relaxed);
        self.metrics
            .transactions_validated
            .fetch_add(tx_count, std::sync::atomic::Ordering::Relaxed);

        let last_block = blocks
            .last()
            .expect("blocks is non-empty (checked at function entry)");
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
                // Count ALL epoch transitions (batches may span multiple epochs)
                let epochs_crossed = (current_epoch - *last_snapshot_epoch) as u32;
                info!(
                    epoch = current_epoch,
                    epochs_crossed, "Epoch transition — saving ledger snapshot"
                );
                self.epoch_transitions_observed = self
                    .epoch_transitions_observed
                    .saturating_add(epochs_crossed);
                self.save_ledger_snapshot().await;
                *last_snapshot_epoch = current_epoch;

                // Prune opcert counters to only keep active pools (prevents unbounded growth)
                let active_pools: std::collections::HashSet<_> = self
                    .ledger_state
                    .read()
                    .await
                    .pool_params
                    .keys()
                    .copied()
                    .collect();
                self.consensus.prune_opcert_counters(&active_pools);
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
                    self.metrics.peers_cold.store(
                        pm.cold_peer_count() as u64,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    self.metrics.peers_warm.store(
                        pm.warm_peer_count() as u64,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    self.metrics.peers_hot.store(
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
                self.metrics.drep_count.store(
                    ls.governance.dreps.len() as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
                self.metrics.proposal_count.store(
                    ls.governance.proposals.len() as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
                self.metrics.pool_count.store(
                    ls.pool_params.len() as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
                // Only show sync progress when catching up, not when following the tip
                if blocks_remaining > 0 {
                    info!(
                        "Syncing {progress:.2}% | slot {slot}/{tip_slot} | block {block_no}/{tip_block} | epoch {} | {blocks_per_sec:.0} blocks/s | {} UTxOs | {blocks_remaining} blocks remaining",
                        ls.epoch.0,
                        ls.utxo_set.len()
                    );
                }
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

        applied_count
    }

    /// Update the query handler with the current ledger state
    /// Enable strict verification mode and update nonce_established based on
    /// whether enough epoch transitions have been observed since node startup
    /// for the epoch nonce to be reliably computed from accumulated VRF outputs.
    /// After Mithril import, needs at least 2 epoch transitions.
    async fn enable_strict_verification(&mut self) {
        self.consensus.set_strict_verification(true);
        self.consensus.nonce_established = self.epoch_transitions_observed >= 2;
        if !self.consensus.nonce_established {
            debug!(
                transitions = self.epoch_transitions_observed,
                "VRF proof verification deferred: epoch nonce not yet established (need 2 epoch transitions)"
            );
        }
    }

    async fn update_query_state(&self) {
        use torsten_network::query_handler::{
            CommitteeMemberSnapshot, CommitteeSnapshot, DRepSnapshot, DRepStakeEntry,
            GenesisConfigSnapshot, PoolParamsSnapshot, PoolStakeSnapshotEntry, ProposalSnapshot,
            ShelleyPParamsSnapshot, StakeAddressSnapshot, StakeDelegDepositEntry,
            StakePoolSnapshot, StakeSnapshotsResult, VoteDelegateeEntry,
        };

        let ls = self.ledger_state.read().await;

        // Build per-pool stake map from delegations for accurate reporting.
        // Per Cardano spec, total stake = UTxO-delegated stake + reward account balance.
        let mut pool_stake_map: std::collections::HashMap<torsten_primitives::hash::Hash28, u64> =
            std::collections::HashMap::new();
        for (cred_hash, pool_id) in &ls.delegations {
            let utxo_stake = ls
                .stake_distribution
                .stake_map
                .get(cred_hash)
                .map(|l| l.0)
                .unwrap_or(0);
            let reward_balance = ls.reward_accounts.get(cred_hash).map(|l| l.0).unwrap_or(0);
            *pool_stake_map.entry(*pool_id).or_default() += utxo_stake + reward_balance;
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

        // Build DRep snapshots with delegator lookup
        let drep_entries: Vec<DRepSnapshot> = ls
            .governance
            .dreps
            .iter()
            .map(|(hash, drep)| {
                let expiry = drep.registered_epoch.0 + ls.protocol_params.drep_activity;
                // Collect stake credentials delegated to this DRep
                let delegator_hashes: Vec<Vec<u8>> = ls
                    .governance
                    .vote_delegations
                    .iter()
                    .filter(|(_, d)| match d {
                        torsten_primitives::transaction::DRep::KeyHash(h) => h == hash,
                        torsten_primitives::transaction::DRep::ScriptHash(h) => {
                            h.to_hash32_padded() == *hash
                        }
                        _ => false,
                    })
                    .map(|(stake_cred, _)| stake_cred.as_ref().to_vec())
                    .collect();
                DRepSnapshot {
                    credential_hash: hash.as_ref().to_vec(),
                    credential_type: 0, // KeyHashObj (we don't track script DReps separately yet)
                    deposit: drep.deposit.0,
                    anchor_url: drep.anchor.as_ref().map(|a| a.url.clone()),
                    anchor_hash: drep.anchor.as_ref().map(|a| a.data_hash.as_ref().to_vec()),
                    expiry_epoch: expiry,
                    delegator_hashes,
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

        // Build stake delegation deposits (registered stake credentials → key_deposit)
        let key_deposit = ls.protocol_params.key_deposit.0;
        let stake_deleg_deposits: Vec<StakeDelegDepositEntry> = ls
            .reward_accounts
            .keys()
            .map(|cred_hash| StakeDelegDepositEntry {
                credential_hash: cred_hash.as_ref()[..28].to_vec(),
                credential_type: 0, // KeyHash (we don't distinguish script creds yet)
                deposit: key_deposit,
            })
            .collect();

        // Build DRep stake distribution (DRep → total delegated stake)
        let drep_stake_distr: Vec<DRepStakeEntry> = {
            use torsten_primitives::transaction::DRep;
            let mut drep_stakes: std::collections::HashMap<String, (u8, Option<Vec<u8>>, u64)> =
                std::collections::HashMap::new();
            for (stake_cred, drep) in &ls.governance.vote_delegations {
                let stake = ls
                    .stake_distribution
                    .stake_map
                    .get(stake_cred)
                    .map(|l| l.0)
                    .unwrap_or(0);
                let (key, drep_type, drep_hash) = match drep {
                    DRep::KeyHash(h) => {
                        let hb = h.as_ref()[..28].to_vec();
                        (format!("0:{}", hex::encode(&hb)), 0u8, Some(hb))
                    }
                    DRep::ScriptHash(h) => {
                        let hb = h.as_ref().to_vec();
                        (format!("1:{}", hex::encode(&hb)), 1u8, Some(hb))
                    }
                    DRep::Abstain => ("2:abstain".to_string(), 2u8, None),
                    DRep::NoConfidence => ("3:noconf".to_string(), 3u8, None),
                };
                let entry = drep_stakes
                    .entry(key)
                    .or_insert((drep_type, drep_hash.clone(), 0));
                entry.2 += stake;
            }
            drep_stakes
                .into_values()
                .map(|(drep_type, drep_hash, stake)| DRepStakeEntry {
                    drep_type,
                    drep_hash,
                    stake,
                })
                .collect()
        };

        // Build vote delegatee entries
        let vote_delegatees: Vec<VoteDelegateeEntry> = {
            use torsten_primitives::transaction::DRep;
            ls.governance
                .vote_delegations
                .iter()
                .map(|(stake_cred, drep)| {
                    let (drep_type, drep_hash) = match drep {
                        DRep::KeyHash(h) => (0u8, Some(h.as_ref()[..28].to_vec())),
                        DRep::ScriptHash(h) => (1u8, Some(h.as_ref().to_vec())),
                        DRep::Abstain => (2u8, None),
                        DRep::NoConfidence => (3u8, None),
                    };
                    VoteDelegateeEntry {
                        credential_hash: stake_cred.as_ref()[..28].to_vec(),
                        credential_type: 0, // KeyHash
                        drep_type,
                        drep_hash,
                    }
                })
                .collect()
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
            enacted_pparam_update: ls
                .governance
                .enacted_pparam_update
                .as_ref()
                .map(|id| (id.transaction_id.as_ref().to_vec(), id.action_index)),
            enacted_hard_fork: ls
                .governance
                .enacted_hard_fork
                .as_ref()
                .map(|id| (id.transaction_id.as_ref().to_vec(), id.action_index)),
            enacted_committee: ls
                .governance
                .enacted_committee
                .as_ref()
                .map(|id| (id.transaction_id.as_ref().to_vec(), id.action_index)),
            enacted_constitution: ls
                .governance
                .enacted_constitution
                .as_ref()
                .map(|id| (id.transaction_id.as_ref().to_vec(), id.action_index)),
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
            slot_length_secs: self.shelley_genesis.as_ref().map_or(1, |g| g.slot_length),
            network_magic: self.network_magic as u32,
            security_param: self.consensus.security_param,
            stake_deleg_deposits,
            drep_stake_distr,
            vote_delegatees,
            era_summaries: self.build_era_summaries(&ls),
            active_slots_coeff_num: self.shelley_genesis.as_ref().map_or(1, |g| {
                let (n, _) = float_to_rational(g.active_slots_coeff);
                n
            }),
            active_slots_coeff_den: self.shelley_genesis.as_ref().map_or(20, |g| {
                let (_, d) = float_to_rational(g.active_slots_coeff);
                d
            }),
            slots_per_kes_period: self
                .shelley_genesis
                .as_ref()
                .map_or(129600, |g| g.slots_per_k_e_s_period),
            max_kes_evolutions: self
                .shelley_genesis
                .as_ref()
                .map_or(62, |g| g.max_k_e_s_evolutions),
            update_quorum: self.shelley_genesis.as_ref().map_or(5, |g| g.update_quorum),
            max_lovelace_supply: self
                .shelley_genesis
                .as_ref()
                .map_or(45_000_000_000_000_000, |g| g.max_lovelace_supply),
            genesis_config: self.shelley_genesis.as_ref().map(|g| {
                let gp = &g.protocol_params;
                // Convert a0 from f64 to rational
                let (a0_num, a0_den) = float_to_rational(gp.a0);
                let (rho_num, rho_den) = float_to_rational(gp.rho);
                let (tau_num, tau_den) = float_to_rational(gp.tau);
                let (asc_num, asc_den) = float_to_rational(g.active_slots_coeff);
                GenesisConfigSnapshot {
                    system_start: g.system_start.clone(),
                    network_magic: g.network_magic as u32,
                    network_id: if g.network_id == "Mainnet" { 1 } else { 0 },
                    active_slots_coeff_num: asc_num,
                    active_slots_coeff_den: asc_den,
                    security_param: g.security_param,
                    epoch_length: g.epoch_length,
                    slots_per_kes_period: g.slots_per_k_e_s_period,
                    max_kes_evolutions: g.max_k_e_s_evolutions,
                    slot_length_micros: g.slot_length * 1_000_000,
                    update_quorum: g.update_quorum,
                    max_lovelace_supply: g.max_lovelace_supply,
                    protocol_params: ShelleyPParamsSnapshot {
                        min_fee_a: gp.min_fee_a,
                        min_fee_b: gp.min_fee_b,
                        max_block_body_size: gp.max_block_body_size as u32,
                        max_tx_size: gp.max_tx_size as u32,
                        max_block_header_size: gp.max_block_header_size as u16,
                        key_deposit: gp.key_deposit,
                        pool_deposit: gp.pool_deposit,
                        e_max: gp.e_max as u32,
                        n_opt: gp.n_opt as u16,
                        a0_num,
                        a0_den,
                        rho_num,
                        rho_den,
                        tau_num,
                        tau_den,
                        d_num: 0,
                        d_den: 1,
                        protocol_version_major: gp.protocol_version.major,
                        protocol_version_minor: gp.protocol_version.minor,
                        min_utxo_value: gp.min_u_tx_o_value,
                        min_pool_cost: gp.min_pool_cost,
                    },
                    gen_delegs: Vec::new(),
                }
            }),
        };

        // Drop the ledger read lock before acquiring the query handler write lock
        drop(ls);

        let mut handler = self.query_handler.write().await;
        handler.update_state(snapshot);
    }

    /// Build era summaries for GetEraHistory responses.
    ///
    /// For testnets (preview/preprod), Shelley starts at slot 0 with uniform parameters.
    /// For mainnet, Byron has 20s slots and 21600 slot epochs before Shelley at slot 4492800.
    /// We produce a simplified summary covering Byron (if mainnet) + Shelley-through-Conway.
    fn build_era_summaries(
        &self,
        _ls: &torsten_ledger::LedgerState,
    ) -> Vec<torsten_network::query_handler::EraSummary> {
        use torsten_network::query_handler::{EraBound, EraSummary};

        let epoch_length = self
            .shelley_genesis
            .as_ref()
            .map(|g| g.epoch_length)
            .unwrap_or(432000);
        let slot_length_ms = self
            .shelley_genesis
            .as_ref()
            .map(|g| g.slot_length * 1000)
            .unwrap_or(1000);
        let safe_zone = self
            .shelley_genesis
            .as_ref()
            .map(|g| g.security_param * 2)
            .unwrap_or(4320);

        let is_mainnet = self.network_magic == 764824073;

        if is_mainnet {
            // Mainnet: Byron era (slots 0..4492800, epochs 0..208, 20s slots, 21600 epoch)
            // then Shelley+ from slot 4492800, epoch 208
            let byron_epoch_len: u64 = 21600;
            let byron_slot_len_ms: u64 = 20000;
            let byron_slots = 208 * byron_epoch_len; // 4492800
            let byron_time_pico =
                (byron_slots as u128 * byron_slot_len_ms as u128 * 1_000_000_000) as u64;

            vec![
                EraSummary {
                    start_slot: 0,
                    start_epoch: 0,
                    start_time_pico: 0,
                    end: Some(EraBound {
                        slot: byron_slots,
                        epoch: 208,
                        time_pico: byron_time_pico,
                    }),
                    epoch_size: byron_epoch_len,
                    slot_length_ms: byron_slot_len_ms,
                    safe_zone: byron_epoch_len, // Byron safe zone = full epoch
                },
                EraSummary {
                    start_slot: byron_slots,
                    start_epoch: 208,
                    start_time_pico: byron_time_pico,
                    end: None,
                    epoch_size: epoch_length,
                    slot_length_ms,
                    safe_zone,
                },
            ]
        } else {
            // Testnets: single era from slot 0
            vec![EraSummary {
                start_slot: 0,
                start_epoch: 0,
                start_time_pico: 0,
                end: None,
                epoch_size: epoch_length,
                slot_length_ms,
                safe_zone,
            }]
        }
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

        // Count every rollback event for observability, even no-ops.
        self.metrics
            .rollback_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

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

        // 1. Roll back ChainDB
        {
            let mut db = self.chain_db.write().await;
            if let Err(e) = db.rollback_to_point(rollback_point) {
                error!("ChainDB rollback failed: {e}");
                return;
            }
        }

        // 2. Find the best ledger snapshot at or before the rollback point.
        //    Try epoch-numbered snapshots first (newest that's <= rollback_slot),
        //    then fall back to the latest snapshot.
        let best_snapshot = self.find_best_snapshot_for_rollback(rollback_slot);

        if let Some(snapshot_path) = best_snapshot {
            match torsten_ledger::LedgerState::load_snapshot(&snapshot_path) {
                Ok(snapshot_state) => {
                    let snapshot_slot = snapshot_state.tip.point.slot().map(|s| s.0).unwrap_or(0);

                    // Restore from snapshot and replay forward to rollback point
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
                                match torsten_serialization::multi_era::decode_block_with_byron_epoch_length(&cbor, self.byron_epoch_length) {
                                    Ok(block) => {
                                        if let Err(e) = ls.apply_block(&block) {
                                            error!(
                                                slot = next_slot.0,
                                                "Ledger apply failed during rollback replay: {e} — aborting replay"
                                            );
                                            break;
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
                        snapshot = %snapshot_path.display(),
                        "Ledger state restored from snapshot and replayed"
                    );
                }
                Err(e) => {
                    error!("Failed to load ledger snapshot for rollback: {e}");
                    let mut ls = self.ledger_state.write().await;
                    *ls = torsten_ledger::LedgerState::new(ls.protocol_params.clone());
                }
            }
        } else {
            warn!("No suitable ledger snapshot found for rollback to slot {rollback_slot}, resetting ledger state");
            let mut ls = self.ledger_state.write().await;
            *ls = torsten_ledger::LedgerState::new(ls.protocol_params.clone());
        }

        // 4. Clear mempool — UTxO set has changed, existing txs may be invalid
        self.mempool.clear();

        // 5. Notify peers
        self.notify_rollback(rollback_point).await;
    }

    /// Compute the current slot number from wall-clock time using Shelley genesis parameters.
    fn current_wall_clock_slot(&self) -> Option<torsten_primitives::time::SlotNo> {
        let genesis = self.shelley_genesis.as_ref()?;
        let system_start = torsten_primitives::time::SystemStart {
            utc_time: chrono::DateTime::parse_from_rfc3339(&genesis.system_start)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .ok()?,
        };
        let slot_length = torsten_primitives::time::SlotLength(genesis.slot_length as f64);
        torsten_primitives::time::SlotNo::from_wall_clock(
            chrono::Utc::now(),
            &system_start,
            slot_length,
        )
    }

    /// Attempt to forge a block if we are in block producer mode and are the slot leader.
    ///
    /// Called every slot when the node is caught up to the chain tip.
    async fn try_forge_block(&mut self) {
        let creds = match &self.block_producer {
            Some(c) => c,
            None => return, // relay-only mode
        };

        // Don't forge if epoch nonce isn't established yet (e.g., post-Mithril import)
        if !self.consensus.nonce_established {
            debug!("Forge: skipping — epoch nonce not yet established");
            return;
        }

        // Compute current slot from wall-clock time
        let wall_clock_slot = self.current_wall_clock_slot();

        let ls = self.ledger_state.read().await;
        let tip_slot = ls.tip.point.slot().map(|s| s.0).unwrap_or(0);
        let next_slot = match wall_clock_slot {
            Some(wc) if wc.0 > tip_slot => wc,
            _ => {
                // No genesis or wall clock behind tip — skip forging
                return;
            }
        };
        let epoch_nonce = ls.epoch_nonce;
        let block_number = torsten_primitives::time::BlockNo(ls.current_block_number().0 + 1);
        let prev_hash = ls
            .tip
            .point
            .hash()
            .copied()
            .unwrap_or(torsten_primitives::hash::Hash32::ZERO);
        let slots_per_kes_period = self.consensus.slots_per_kes_period;

        // Calculate relative stake from the "set" snapshot (used for leader election)
        let (relative_stake, pool_stake_lovelace) = if let Some(set_snapshot) = &ls.snapshots.set {
            let total_stake: u64 = set_snapshot.pool_stake.values().map(|s| s.0).sum();
            let pool_stake = set_snapshot
                .pool_stake
                .get(&creds.pool_id)
                .map(|s| s.0)
                .unwrap_or(0);
            if total_stake > 0 {
                (pool_stake as f64 / total_stake as f64, pool_stake)
            } else {
                (0.0, 0)
            }
        } else {
            debug!(
                pool_id = %creds.pool_id,
                "Forge: skipping — no 'set' snapshot available"
            );
            (0.0, 0)
        };
        drop(ls);

        if relative_stake == 0.0 {
            // Log periodically so the operator knows stake hasn't activated yet
            if next_slot.0 % 100 == 0 {
                debug!(
                    slot = next_slot.0,
                    pool_id = %creds.pool_id,
                    pool_stake = pool_stake_lovelace,
                    "Forge: pool has zero relative stake in 'set' snapshot — waiting for delegation"
                );
            }
            return;
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

        // Collect transactions from mempool using protocol params limits
        let ls = self.ledger_state.read().await;
        let max_block_body_size = ls.protocol_params.max_block_body_size;
        let protocol_version_major = ls.protocol_params.protocol_version_major;
        let protocol_version_minor = ls.protocol_params.protocol_version_minor;
        let current_era = ls.era;
        drop(ls);
        let transactions = self
            .mempool
            .get_txs_for_block(500, max_block_body_size as usize);
        let config = crate::forge::BlockProducerConfig {
            protocol_version: torsten_primitives::block::ProtocolVersion {
                major: protocol_version_major,
                minor: protocol_version_minor,
            },
            max_block_body_size,
            max_txs_per_block: 500,
            era: current_era,
            slots_per_kes_period,
        };

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

#[cfg(test)]
mod tests {
    use super::*;
    use torsten_primitives::block::{
        Block, BlockHeader, OperationalCert, ProtocolVersion, VrfOutput,
    };
    use torsten_primitives::era::Era;
    use torsten_primitives::hash::Hash32;
    use torsten_primitives::time::{BlockNo, SlotNo};

    /// Helper to create a minimal test block with the given era, block number, hash, and prev_hash.
    fn make_test_block(
        era: Era,
        block_no: u64,
        slot: u64,
        hash: Hash32,
        prev_hash: Hash32,
    ) -> Block {
        Block {
            header: BlockHeader {
                header_hash: hash,
                prev_hash,
                issuer_vkey: vec![],
                vrf_vkey: vec![],
                vrf_result: VrfOutput {
                    output: vec![],
                    proof: vec![],
                },
                block_number: BlockNo(block_no),
                slot: SlotNo(slot),
                epoch_nonce: Hash32::ZERO,
                body_size: 0,
                body_hash: Hash32::ZERO,
                operational_cert: OperationalCert {
                    hot_vkey: vec![],
                    sequence_number: 0,
                    kes_period: 0,
                    sigma: vec![],
                },
                protocol_version: ProtocolVersion { major: 0, minor: 0 },
                kes_signature: vec![],
            },
            transactions: vec![],
            era,
            raw_cbor: None,
        }
    }

    #[test]
    fn test_validate_genesis_empty_blocks() {
        // Empty block list should pass validation
        let result = validate_genesis_blocks(&[], None, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_genesis_skips_non_genesis_block() {
        // Block with block_number > 0 should skip validation
        let block = make_test_block(
            Era::Byron,
            42,
            100,
            Hash32::from_bytes([1u8; 32]),
            Hash32::from_bytes([2u8; 32]),
        );
        let result = validate_genesis_blocks(&[block], None, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_byron_genesis_hash_match() {
        let expected_hash = Hash32::from_bytes([0xAA; 32]);
        let block = make_test_block(Era::Byron, 0, 0, expected_hash, Hash32::ZERO);
        let result = validate_genesis_blocks(&[block], Some(&expected_hash), None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_byron_genesis_hash_mismatch() {
        let expected_hash = Hash32::from_bytes([0xAA; 32]);
        let wrong_hash = Hash32::from_bytes([0xBB; 32]);
        let block = make_test_block(Era::Byron, 0, 0, wrong_hash, Hash32::ZERO);
        let result = validate_genesis_blocks(&[block], Some(&expected_hash), None);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Byron genesis block hash mismatch"));
        assert!(err.contains(&expected_hash.to_hex()));
        assert!(err.contains(&wrong_hash.to_hex()));
    }

    #[test]
    fn test_validate_byron_genesis_no_expected_hash() {
        // When no expected hash is configured, validation should pass (with warning)
        let block = make_test_block(
            Era::Byron,
            0,
            0,
            Hash32::from_bytes([0xCC; 32]),
            Hash32::ZERO,
        );
        let result = validate_genesis_blocks(&[block], None, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_shelley_genesis_prev_hash_match() {
        // For Shelley-first chains, prev_hash of block 0 is the genesis hash
        let genesis_hash = Hash32::from_bytes([0xDD; 32]);
        let block = make_test_block(
            Era::Shelley,
            0,
            0,
            Hash32::from_bytes([0x11; 32]),
            genesis_hash,
        );
        let result = validate_genesis_blocks(&[block], None, Some(&genesis_hash));
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_shelley_genesis_prev_hash_mismatch() {
        let expected_genesis = Hash32::from_bytes([0xDD; 32]);
        let wrong_prev = Hash32::from_bytes([0xEE; 32]);
        let block = make_test_block(
            Era::Shelley,
            0,
            0,
            Hash32::from_bytes([0x11; 32]),
            wrong_prev,
        );
        let result = validate_genesis_blocks(&[block], None, Some(&expected_genesis));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Shelley genesis hash mismatch"));
        assert!(err.contains(&expected_genesis.to_hex()));
        assert!(err.contains(&wrong_prev.to_hex()));
    }

    #[test]
    fn test_validate_shelley_genesis_no_expected_hash() {
        // When no expected Shelley hash is configured, validation should pass
        let block = make_test_block(
            Era::Shelley,
            0,
            0,
            Hash32::from_bytes([0x11; 32]),
            Hash32::from_bytes([0x22; 32]),
        );
        let result = validate_genesis_blocks(&[block], None, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_byron_and_shelley_batch() {
        // A batch starting with Byron genesis block 0 followed by more blocks
        let byron_hash = Hash32::from_bytes([0xAA; 32]);
        let b0 = make_test_block(Era::Byron, 0, 0, byron_hash, Hash32::ZERO);
        let b1 = make_test_block(Era::Byron, 1, 1, Hash32::from_bytes([0xBB; 32]), byron_hash);

        let result = validate_genesis_blocks(&[b0, b1], Some(&byron_hash), None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_conway_genesis_prev_hash() {
        // Conway era block at genesis (block 0) — still Shelley-based
        let genesis_hash = Hash32::from_bytes([0xFF; 32]);
        let block = make_test_block(
            Era::Conway,
            0,
            0,
            Hash32::from_bytes([0x33; 32]),
            genesis_hash,
        );
        // Conway is Shelley-based, so Shelley genesis hash should be validated
        let result = validate_genesis_blocks(&[block], None, Some(&genesis_hash));
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_conway_genesis_prev_hash_mismatch() {
        let expected = Hash32::from_bytes([0xFF; 32]);
        let wrong = Hash32::from_bytes([0x00; 32]);
        let block = make_test_block(Era::Conway, 0, 0, Hash32::from_bytes([0x33; 32]), wrong);
        let result = validate_genesis_blocks(&[block], None, Some(&expected));
        assert!(result.is_err());
    }

    #[test]
    fn test_config_genesis_hash_parsing() {
        let json = r#"{
            "Network": "Testnet",
            "NetworkMagic": 2,
            "ByronGenesisFile": "preview-byron-genesis.json",
            "ByronGenesisHash": "81cf23542e33d64c541699926c2b5e6e9c286583f0c8a3fb5f22ea7b352dd174",
            "ShelleyGenesisFile": "preview-shelley-genesis.json",
            "ShelleyGenesisHash": "363498d1024f84bb39d3fa9593ce391483cb40d479b87233f868d6e57c3a400d"
        }"#;

        let config: NodeConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            config.byron_genesis_hash.as_deref(),
            Some("81cf23542e33d64c541699926c2b5e6e9c286583f0c8a3fb5f22ea7b352dd174")
        );
        assert_eq!(
            config.shelley_genesis_hash.as_deref(),
            Some("363498d1024f84bb39d3fa9593ce391483cb40d479b87233f868d6e57c3a400d")
        );

        // Verify the hashes parse into Hash32 correctly
        let byron_hash = Hash32::from_hex(config.byron_genesis_hash.as_ref().unwrap()).unwrap();
        assert_ne!(byron_hash, Hash32::ZERO);

        let shelley_hash = Hash32::from_hex(config.shelley_genesis_hash.as_ref().unwrap()).unwrap();
        assert_ne!(shelley_hash, Hash32::ZERO);
    }

    #[test]
    fn test_config_without_genesis_hashes() {
        let json = r#"{
            "Network": "Testnet",
            "NetworkMagic": 2,
            "ByronGenesisFile": "preview-byron-genesis.json",
            "ShelleyGenesisFile": "preview-shelley-genesis.json"
        }"#;

        let config: NodeConfig = serde_json::from_str(json).unwrap();
        assert!(config.byron_genesis_hash.is_none());
        assert!(config.shelley_genesis_hash.is_none());
        assert!(config.alonzo_genesis_hash.is_none());
        assert!(config.conway_genesis_hash.is_none());
    }
}
