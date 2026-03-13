use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::signal;
use tokio::sync::{watch, RwLock};
use tracing::{debug, error, info, warn};

use torsten_consensus::praos::BlockIssuerInfo;
use torsten_consensus::{OuroborosPraos, ValidationMode};
use torsten_ledger::{BlockValidationMode, LedgerState};
use torsten_mempool::{Mempool, MempoolConfig};
use torsten_network::query_handler::{UtxoQueryProvider, UtxoSnapshot};
use torsten_network::server::NodeServerConfig;
use torsten_network::{
    BlockFetchPool, BlockProvider, ChainSyncEvent, DiffusionMode, Governor, GovernorEvent,
    HeaderBatchResult, N2CServer, NodeServer, NodeStateSnapshot, NodeToNodeClient, PeerManager,
    PeerManagerConfig, PipelinedPeerClient, QueryHandler, TipInfo, TxValidationError, TxValidator,
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
    /// Prometheus metrics port (0 to disable)
    pub metrics_port: u16,
    /// Maximum number of transactions in the mempool
    pub mempool_max_tx: usize,
    /// Maximum mempool size in bytes
    pub mempool_max_bytes: usize,
    /// Maximum snapshots to retain on disk
    pub snapshot_max_retained: usize,
    /// Minimum blocks between bulk-sync snapshots
    pub snapshot_bulk_min_blocks: u64,
    /// Minimum seconds between bulk-sync snapshots
    pub snapshot_bulk_min_secs: u64,
    /// Storage configuration (block index type, UTxO backend, LSM tuning)
    pub storage_config: torsten_storage::StorageConfig,
    /// Consensus mode: "praos" (default) or "genesis" (enables genesis bootstrap)
    pub consensus_mode: String,
    /// Force ValidateAll mode on every block (paranoid/auditing mode)
    pub validate_all_blocks: bool,
}

/// Provides block data from ChainDB for the N2N server
struct ChainDBBlockProvider {
    chain_db: Arc<RwLock<ChainDB>>,
}

impl BlockProvider for ChainDBBlockProvider {
    fn get_block(&self, hash: &[u8; 32]) -> Option<Vec<u8>> {
        let block_hash = torsten_primitives::hash::Hash32::from_bytes(*hash);
        tokio::task::block_in_place(|| {
            let db = self.chain_db.blocking_read();
            db.get_block(&block_hash).ok().flatten()
        })
    }

    fn has_block(&self, hash: &[u8; 32]) -> bool {
        let block_hash = torsten_primitives::hash::Hash32::from_bytes(*hash);
        tokio::task::block_in_place(|| {
            let db = self.chain_db.blocking_read();
            db.has_block(&block_hash)
        })
    }

    fn get_tip(&self) -> TipInfo {
        tokio::task::block_in_place(|| {
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
        })
    }

    fn get_next_block_after_slot(&self, after_slot: u64) -> Option<(u64, [u8; 32], Vec<u8>)> {
        tokio::task::block_in_place(|| {
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
        })
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
            Err(e) => {
                tracing::warn!(
                    "UTxO query: address decode failed: {e} (bytes len={})",
                    addr_bytes.len()
                );
                return vec![];
            }
        };
        // Use block_in_place + blocking_read so this works correctly even when
        // called from within a tokio async runtime (avoids "cannot block" panic).
        tokio::task::block_in_place(|| {
            let ledger = self.ledger.blocking_read();
            let results: Vec<_> = ledger
                .utxo_set
                .utxos_at_address(&addr)
                .into_iter()
                .map(|(input, output)| utxo_to_snapshot(&input, &output))
                .collect();
            tracing::debug!(
                addr_type = ?std::mem::discriminant(&addr),
                index_size = ledger.utxo_set.address_index_size(),
                utxos_found = results.len(),
                "UTxO query by address"
            );
            results
        })
    }

    fn utxos_by_tx_inputs(&self, inputs: &[(Vec<u8>, u32)]) -> Vec<UtxoSnapshot> {
        tokio::task::block_in_place(|| {
            let ledger = self.ledger.blocking_read();
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
                        results.push(utxo_to_snapshot(&tx_input, &output));
                    }
                }
            }
            results
        })
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

/// Convert a Credential to (type, hash_bytes) for vote maps.
/// Returns (0, hash_28) for VerificationKey, (1, hash_28) for Script.
fn credential_to_bytes(cred: &torsten_primitives::credentials::Credential) -> (u8, Vec<u8>) {
    match cred {
        torsten_primitives::credentials::Credential::VerificationKey(h) => (0, h.as_ref().to_vec()),
        torsten_primitives::credentials::Credential::Script(h) => (1, h.as_ref().to_vec()),
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
    metrics: Arc<crate::metrics::NodeMetrics>,
}

impl TxValidator for LedgerTxValidator {
    fn validate_tx(&self, era_id: u16, tx_bytes: &[u8]) -> Result<(), TxValidationError> {
        let tx = torsten_serialization::decode_transaction(era_id, tx_bytes).map_err(|e| {
            TxValidationError::DecodeFailed {
                reason: e.to_string(),
            }
        })?;

        let ledger = self
            .ledger
            .try_read()
            .map_err(|_| TxValidationError::LedgerStateUnavailable)?;
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
            for err in &errors {
                self.metrics.record_validation_error(&format!("{:?}", err));
            }
            let mut mapped: Vec<TxValidationError> =
                errors.into_iter().map(convert_validation_error).collect();
            if mapped.len() == 1 {
                mapped.pop().expect("vec has exactly one element")
            } else {
                TxValidationError::Multiple(mapped)
            }
        })
    }
}

/// Convert a ledger `ValidationError` into the network-facing `TxValidationError`.
fn convert_validation_error(e: torsten_ledger::validation::ValidationError) -> TxValidationError {
    use torsten_ledger::validation::ValidationError as VE;
    match e {
        VE::NoInputs => TxValidationError::NoInputs,
        VE::InputNotFound(input) => TxValidationError::InputNotFound { input },
        VE::ValueNotConserved {
            inputs,
            outputs,
            fee,
        } => TxValidationError::ValueNotConserved {
            inputs,
            outputs,
            fee,
        },
        VE::FeeTooSmall { minimum, actual } => TxValidationError::FeeTooSmall { minimum, actual },
        VE::OutputTooSmall { minimum, actual } => {
            TxValidationError::OutputTooSmall { minimum, actual }
        }
        VE::TxTooLarge { maximum, actual } => TxValidationError::TxTooLarge { maximum, actual },
        VE::MissingRequiredSigner(signer) => TxValidationError::MissingRequiredSigner { signer },
        VE::MissingWitness(input) => TxValidationError::MissingWitness { input },
        VE::TtlExpired { current_slot, ttl } => TxValidationError::TtlExpired { current_slot, ttl },
        VE::NotYetValid {
            current_slot,
            valid_from,
        } => TxValidationError::NotYetValid {
            current_slot,
            valid_from,
        },
        VE::ScriptFailed(reason) => TxValidationError::ScriptFailed { reason },
        VE::InsufficientCollateral => TxValidationError::InsufficientCollateral,
        VE::TooManyCollateralInputs { max, actual } => {
            TxValidationError::TooManyCollateralInputs { max, actual }
        }
        VE::CollateralNotFound(input) => TxValidationError::CollateralNotFound { input },
        VE::CollateralHasTokens(input) => TxValidationError::CollateralHasTokens { input },
        VE::CollateralMismatch { declared, computed } => {
            TxValidationError::CollateralMismatch { declared, computed }
        }
        VE::ReferenceInputNotFound(input) => TxValidationError::ReferenceInputNotFound { input },
        VE::ReferenceInputOverlapsInput(input) => {
            TxValidationError::ReferenceInputOverlapsInput { input }
        }
        VE::MultiAssetNotConserved {
            policy,
            input_side,
            output_side,
        } => TxValidationError::MultiAssetNotConserved {
            policy,
            input_side,
            output_side,
        },
        VE::InvalidMint => TxValidationError::InvalidMint,
        VE::ExUnitsExceeded => TxValidationError::ExUnitsExceeded,
        VE::ScriptDataHashMismatch { expected, actual } => {
            TxValidationError::ScriptDataHashMismatch { expected, actual }
        }
        VE::UnexpectedScriptDataHash => TxValidationError::UnexpectedScriptDataHash,
        VE::MissingScriptDataHash => TxValidationError::MissingScriptDataHash,
        VE::DuplicateInput(input) => TxValidationError::DuplicateInput { input },
        VE::NativeScriptFailed => TxValidationError::NativeScriptFailed,
        VE::InvalidWitnessSignature(vkey) => TxValidationError::InvalidWitnessSignature { vkey },
        VE::NetworkMismatch { expected, actual } => TxValidationError::NetworkMismatch {
            expected: format!("{expected:?}"),
            actual: format!("{actual:?}"),
        },
        VE::AuxiliaryDataHashWithoutData => TxValidationError::AuxiliaryDataHashWithoutData,
        VE::AuxiliaryDataWithoutHash => TxValidationError::AuxiliaryDataWithoutHash,
        VE::BlockExUnitsExceeded {
            resource,
            limit,
            total,
        } => TxValidationError::BlockExUnitsExceeded {
            resource,
            limit,
            total,
        },
        VE::OutputValueTooLarge { maximum, actual } => {
            TxValidationError::OutputValueTooLarge { maximum, actual }
        }
        VE::MissingRawCbor => TxValidationError::MissingRawCbor,
        VE::MissingSlotConfig => TxValidationError::MissingSlotConfig,
        VE::MissingSpendRedeemer { index } => TxValidationError::MissingSpendRedeemer { index },
        VE::RedeemerIndexOutOfRange { tag, index, max } => {
            TxValidationError::RedeemerIndexOutOfRange { tag, index, max }
        }
        VE::MissingInputWitness(credential) => {
            TxValidationError::MissingInputWitness { credential }
        }
        VE::MissingScriptWitness(credential) => {
            TxValidationError::MissingScriptWitness { credential }
        }
        VE::MissingWithdrawalWitness(credential) => {
            TxValidationError::MissingWithdrawalWitness { credential }
        }
        VE::MissingWithdrawalScriptWitness(credential) => {
            TxValidationError::MissingWithdrawalScriptWitness { credential }
        }
        VE::ValueOverflow => TxValidationError::ValueOverflow,
    }
}

/// Bridges N2N server connection events to the node metrics system.
struct N2NConnectionMetrics {
    metrics: Arc<crate::metrics::NodeMetrics>,
}

impl torsten_network::ConnectionMetrics for N2NConnectionMetrics {
    fn on_connect(&self) {
        self.metrics
            .n2n_connections_total
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.metrics
            .n2n_connections_active
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    fn on_disconnect(&self) {
        self.metrics
            .n2n_connections_active
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
    fn on_error(&self, label: &str) {
        self.metrics.record_protocol_error(label);
    }
}

/// Bridges N2C server connection events to the node metrics system.
struct N2CConnectionMetrics {
    metrics: Arc<crate::metrics::NodeMetrics>,
}

impl torsten_network::ConnectionMetrics for N2CConnectionMetrics {
    fn on_connect(&self) {
        self.metrics
            .n2c_connections_total
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.metrics
            .n2c_connections_active
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    fn on_disconnect(&self) {
        self.metrics
            .n2c_connections_active
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
    fn on_error(&self, label: &str) {
        self.metrics.record_protocol_error(label);
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
        debug!(
            "Skipping genesis validation — not syncing from genesis (block={})",
            first_block.block_number().0,
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
            debug!("Byron genesis block validated: {}", actual.to_hex());
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
            debug!("Shelley genesis ref validated: {}", expected.to_hex());
        } else {
            warn!("No Shelley genesis hash configured — skipping Shelley genesis block validation");
        }
    }

    Ok(())
}

/// The main Torsten node
/// Snapshot policy matching Haskell cardano-node's `SnapshotPolicy`.
///
/// Controls when ledger snapshots are taken based on time and block counts.
/// Two modes:
/// - **Normal operation:** snapshot every `k * 2` seconds (~72 minutes for k=2160)
/// - **Bulk sync (replay):** snapshot every `bulk_min_blocks` blocks AND `bulk_min_interval` elapsed
struct SnapshotPolicy {
    /// Time between snapshots during normal operation (k * 2 seconds)
    normal_interval: std::time::Duration,
    /// Minimum blocks processed before snapshot during bulk sync
    bulk_min_blocks: u64,
    /// Minimum time between snapshots during bulk sync
    bulk_min_interval: std::time::Duration,
    /// Maximum snapshots to retain on disk
    max_snapshots: usize,
    /// Last snapshot time
    last_snapshot_time: std::time::Instant,
    /// Blocks since last snapshot
    blocks_since_snapshot: u64,
}

impl SnapshotPolicy {
    /// Create a new snapshot policy with defaults matching Haskell cardano-node.
    fn new(security_param_k: u64) -> Self {
        SnapshotPolicy {
            normal_interval: std::time::Duration::from_secs(security_param_k * 2),
            bulk_min_blocks: 50_000,
            bulk_min_interval: std::time::Duration::from_secs(360), // 6 minutes
            max_snapshots: 2,
            last_snapshot_time: std::time::Instant::now(),
            blocks_since_snapshot: 0,
        }
    }

    /// Create with custom parameters (from CLI flags).
    fn with_params(
        security_param_k: u64,
        max_snapshots: usize,
        bulk_min_blocks: u64,
        bulk_min_secs: u64,
    ) -> Self {
        SnapshotPolicy {
            normal_interval: std::time::Duration::from_secs(security_param_k * 2),
            bulk_min_blocks,
            bulk_min_interval: std::time::Duration::from_secs(bulk_min_secs),
            max_snapshots,
            last_snapshot_time: std::time::Instant::now(),
            blocks_since_snapshot: 0,
        }
    }

    /// Record that blocks have been applied.
    fn record_blocks(&mut self, count: u64) {
        self.blocks_since_snapshot += count;
    }

    /// Check if a snapshot should be taken during normal (at-tip) operation.
    fn should_snapshot_normal(&self) -> bool {
        self.last_snapshot_time.elapsed() >= self.normal_interval
    }

    /// Check if a snapshot should be taken during bulk sync (replay).
    fn should_snapshot_bulk(&self) -> bool {
        self.blocks_since_snapshot >= self.bulk_min_blocks
            && self.last_snapshot_time.elapsed() >= self.bulk_min_interval
    }

    /// Mark that a snapshot was taken.
    fn snapshot_taken(&mut self) {
        self.last_snapshot_time = std::time::Instant::now();
        self.blocks_since_snapshot = 0;
    }
}

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
    /// Byron slot duration in milliseconds (from genesis, default 20000).
    byron_slot_duration_ms: u64,
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
    /// Snapshot policy controlling when ledger snapshots are taken.
    snapshot_policy: SnapshotPolicy,
    /// Consensus mode: "praos" (default) or "genesis"
    consensus_mode: String,
    /// Force full Phase-2 Plutus validation on all blocks
    validate_all_blocks: bool,
}

impl Node {
    pub fn new(args: NodeArgs) -> Result<Self> {
        let chain_db = Arc::new(RwLock::new(ChainDB::open_with_config(
            &args.database_path,
            &args.storage_config.immutable,
        )?));

        let mut protocol_params = ProtocolParameters::mainnet_defaults();

        // Load Byron genesis if configured
        let config_dir = args.config_dir.clone();
        let mut byron_epoch_length: u64 = 0; // 0 = use pallas defaults (mainnet)
        let mut byron_slot_duration_ms: u64 = 20_000; // default 20s, overridden by genesis
        let mut byron_genesis_file_hash: Option<torsten_primitives::hash::Hash32> = None;
        let byron_genesis_utxos: Vec<(Vec<u8>, u64)> =
            if let Some(ref genesis_path) = args.config.byron_genesis_file {
                let genesis_path = config_dir.join(genesis_path);
                match ByronGenesis::load_with_hash(&genesis_path) {
                    Ok((genesis, hash)) => {
                        let utxos = genesis.initial_utxos();
                        let k = genesis.security_param();
                        byron_epoch_length = 10 * k;
                        byron_slot_duration_ms = genesis.slot_duration_ms();
                        info!(
                            magic = genesis.protocol_magic(),
                            k,
                            epoch_len = byron_epoch_length,
                            slot_duration_ms = byron_slot_duration_ms,
                            utxos = utxos.len(),
                            "Byron genesis loaded",
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
                            magic = genesis.network_magic,
                            start = %genesis.system_start,
                            epoch_len = genesis.epoch_length,
                            "Shelley genesis loaded",
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
                        "Alonzo genesis loaded",
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
                        gov_deposit = genesis.gov_action_deposit,
                        committee_min = genesis.committee_min_size,
                        "Conway genesis loaded",
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
                        Point::Specific(snapshot_slot, ref hash) => {
                            // Use try_read() since we're in a sync context within tokio.
                            // The lock was just created so this will always succeed.
                            match chain_db.try_read() {
                                Ok(db) => {
                                    let exists = db.has_block(hash);
                                    if !exists {
                                        let db_tip = db.get_tip();
                                        let db_tip_slot =
                                            db_tip.point.slot().map(|s| s.0).unwrap_or(0);
                                        if snapshot_slot.0 > db_tip_slot {
                                            warn!(
                                                "Ledger snapshot is ahead of ChainDB (snapshot={}, chaindb={}); \
                                                 node may have crashed before ChainDB persist — discarding snapshot, \
                                                 will replay from storage",
                                                state.tip, db_tip,
                                            );
                                        } else {
                                            warn!(
                                                "Ledger snapshot is stale (snapshot={}, chaindb={})",
                                                state.tip, db_tip,
                                            );
                                        }
                                    }
                                    exists
                                }
                                Err(_) => {
                                    warn!("Could not acquire ChainDB lock for snapshot validation, assuming valid");
                                    true
                                }
                            }
                        }
                    };

                    if snapshot_valid {
                        info!(
                            epoch = state.epoch.0,
                            utxos = state.utxo_set.len(),
                            tip = %state.tip,
                            "Ledger restored from snapshot",
                        );
                        state
                    } else {
                        warn!("Discarding stale ledger snapshot, will replay from ChainDB");
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
            // No native snapshot — start fresh and replay from ChainDB.
            // (Haskell ledger state import is not supported for UTxO-HD format.)
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
                std::sync::Arc::make_mut(&mut ledger.governance).committee_threshold =
                    Some(Rational {
                        numerator: num,
                        denominator: den,
                    });
                debug!("Applied Conway genesis committee quorum threshold ({num}/{den})");
            }
        }
        // Seed initial committee members from Conway genesis if committee is empty
        if ledger.governance.committee_expiration.is_empty() && !conway_committee_members.is_empty()
        {
            use torsten_primitives::hash::Hash32;
            for (hash_bytes, expiration) in &conway_committee_members {
                let cold_key = Hash32::from_bytes(*hash_bytes);
                std::sync::Arc::make_mut(&mut ledger.governance)
                    .committee_expiration
                    .insert(cold_key, torsten_primitives::EpochNo(*expiration));
            }
            debug!(
                "Seeded {} initial committee members from Conway genesis",
                conway_committee_members.len()
            );
        }

        // Wire up on-disk UTxO store if LSM backend is configured
        if matches!(
            args.storage_config.utxo.backend,
            torsten_storage::UtxoBackend::Lsm
        ) {
            let utxo_path = args.database_path.join("utxo-store");
            let utxo_cfg = &args.storage_config.utxo;
            match torsten_ledger::utxo_store::UtxoStore::open_with_config(
                &utxo_path,
                utxo_cfg.memtable_size_mb,
                utxo_cfg.block_cache_size_mb,
                utxo_cfg.bloom_filter_bits_per_key,
            ) {
                Ok(store) => {
                    info!(
                        path = %utxo_path.display(),
                        memtable_mb = utxo_cfg.memtable_size_mb,
                        cache_mb = utxo_cfg.block_cache_size_mb,
                        "UTxO store attached (LSM)"
                    );
                    ledger.attach_utxo_store(store);
                }
                Err(e) => {
                    warn!(
                        "Failed to open UTxO store at {}: {e}, continuing with in-memory UTxOs",
                        utxo_path.display()
                    );
                }
            }
        }

        let ledger_state = Arc::new(RwLock::new(ledger));

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
            epoch_len = consensus.epoch_length.0,
            k = consensus.security_param,
            f = consensus.active_slot_coeff,
            kes_period = consensus.slots_per_kes_period,
            max_kes = consensus.max_kes_evolutions,
            "Consensus: Praos",
        );

        let mempool = Arc::new(Mempool::new(MempoolConfig {
            max_transactions: args.mempool_max_tx,
            max_bytes: args.mempool_max_bytes,
            ..MempoolConfig::default()
        }));

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

        // Load block producer credentials if key paths are provided.
        // If ANY block production flag is set, ALL three must be present — a partial
        // configuration is an error, not a silent fallback to relay mode.
        let bp_flags = [
            ("--shelley-vrf-key", &args.shelley_vrf_key),
            ("--shelley-kes-key", &args.shelley_kes_key),
            (
                "--shelley-operational-certificate",
                &args.shelley_operational_certificate,
            ),
        ];
        let provided: Vec<&str> = bp_flags
            .iter()
            .filter(|(_, v)| v.is_some())
            .map(|(name, _)| *name)
            .collect();
        let missing: Vec<&str> = bp_flags
            .iter()
            .filter(|(_, v)| v.is_none())
            .map(|(name, _)| *name)
            .collect();

        let block_producer = if provided.is_empty() {
            info!("Relay-only mode (no block producer keys)");
            None
        } else if !missing.is_empty() {
            return Err(anyhow::anyhow!(
                "Incomplete block producer configuration: provided {} but missing {}. \
                 All three flags (--shelley-kes-key, --shelley-vrf-key, \
                 --shelley-operational-certificate) are required for block production.",
                provided.join(", "),
                missing.join(", "),
            ));
        } else {
            let vrf_path = args.shelley_vrf_key.as_ref().unwrap();
            let kes_path = args.shelley_kes_key.as_ref().unwrap();
            let opcert_path = args.shelley_operational_certificate.as_ref().unwrap();
            let creds =
                crate::forge::BlockProducerCredentials::load(vrf_path, kes_path, opcert_path)
                    .map_err(|e| {
                        anyhow::anyhow!(
                            "Failed to load block producer credentials: {e}. \
                     Check that the key files and operational certificate are valid."
                        )
                    })?;
            info!(
                pool = %creds.pool_id,
                opcert_seq = creds.opcert_sequence,
                kes_period = creds.opcert_kes_period,
                "Block producer mode",
            );
            Some(creds)
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
            debug!("Expected Byron genesis hash: {}", h.to_hex());
        }
        if let Some(ref h) = expected_shelley_genesis_hash {
            debug!("Expected Shelley genesis hash: {}", h.to_hex());
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
            byron_slot_duration_ms,
            snapshot_policy: SnapshotPolicy::with_params(
                shelley_genesis
                    .as_ref()
                    .map(|g| g.security_param)
                    .unwrap_or(2160),
                args.snapshot_max_retained,
                args.snapshot_bulk_min_blocks,
                args.snapshot_bulk_min_secs,
            ),
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
            consensus_mode: args.consensus_mode,
            validate_all_blocks: args.validate_all_blocks,
        })
    }

    pub async fn run(&mut self) -> Result<()> {
        let tip = self.chain_db.read().await.get_tip();

        // If ChainDB already has blocks, genesis was validated on a prior run
        if tip.point != Point::Origin {
            self.genesis_validated = true;
        }

        {
            let ls = self.ledger_state.read().await;
            info!(
                tip = %tip,
                utxos = ls.utxo_set.len(),
                mempool_txs = self.mempool.len(),
                "Chain tip",
            );
        }

        // Replay blocks from ChainDB if the ledger is behind storage.
        // This happens after a Mithril snapshot import — blocks are in storage
        // but the ledger hasn't processed them yet.
        self.replay_ledger_from_storage().await;

        // Initialize query state from current ledger so N2C queries
        // work immediately (before we reach chain tip or the periodic timer fires)
        self.update_query_state().await;

        // Setup shutdown signal (SIGINT + SIGTERM)
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        #[cfg(unix)]
        {
            let shutdown_tx_clone = shutdown_tx.clone();
            tokio::spawn(async move {
                let mut sigterm =
                    signal::unix::signal(signal::unix::SignalKind::terminate()).unwrap();
                tokio::select! {
                    _ = signal::ctrl_c() => {
                        info!("SIGINT received, shutting down");
                    }
                    _ = sigterm.recv() => {
                        info!("SIGTERM received, shutting down");
                    }
                }
                shutdown_tx_clone.send(true).ok();
            });
        }
        #[cfg(not(unix))]
        {
            let shutdown_tx_clone = shutdown_tx.clone();
            tokio::spawn(async move {
                signal::ctrl_c().await.ok();
                info!("Shutdown signal received");
                shutdown_tx_clone.send(true).ok();
            });
        }

        // SIGHUP handler is set up after peer_manager initialization below

        // Start Prometheus metrics server
        if self.metrics_port > 0 {
            let metrics = self.metrics.clone();
            let port = self.metrics_port;
            let metrics_shutdown_tx = shutdown_tx.clone();
            tokio::spawn(async move {
                if let Err(_e) = crate::metrics::start_metrics_server(port, metrics).await {
                    metrics_shutdown_tx.send(true).ok();
                }
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
            metrics: self.metrics.clone(),
        }));
        n2c_server.set_block_provider(Arc::new(ChainDBBlockProvider {
            chain_db: self.chain_db.clone(),
        }));
        n2c_server.set_connection_metrics(Arc::new(N2CConnectionMetrics {
            metrics: self.metrics.clone(),
        }));
        debug!("N2C server: Plutus tx validation and block delivery enabled");
        let n2c_socket_path = self.socket_path.clone();
        let n2c_shutdown_rx = shutdown_rx.clone();
        let n2c_shutdown_tx = shutdown_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = n2c_server.listen(&n2c_socket_path, n2c_shutdown_rx).await {
                error!("N2C server error: {e}");
                n2c_shutdown_tx.send(true).ok();
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
            // Resolve all DNS addresses BEFORE acquiring the write lock to avoid
            // holding the lock during potentially slow DNS lookups.
            let mut resolved_peers = Vec::new();
            for peer in &detailed_peers {
                match tokio::net::lookup_host(format!("{}:{}", peer.address, peer.port)).await {
                    Ok(addrs) => {
                        for socket_addr in addrs {
                            resolved_peers.push((socket_addr, peer.trustable, peer.advertise));
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
            let mut pm = peer_manager.write().await;
            for (socket_addr, trustable, advertise) in resolved_peers {
                pm.add_config_peer(socket_addr, trustable, advertise);
            }
            let stats = pm.stats();
            info!(
                known = stats.known_peers,
                mode = ?pm.diffusion_mode(),
                "Peers",
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
                            // Resolve DNS before acquiring the write lock
                            let mut resolved = Vec::new();
                            for peer in &new_peers {
                                match tokio::net::lookup_host(format!(
                                    "{}:{}",
                                    peer.address, peer.port
                                ))
                                .await
                                {
                                    Ok(addrs) => {
                                        for socket_addr in addrs {
                                            resolved.push((
                                                socket_addr,
                                                peer.trustable,
                                                peer.advertise,
                                            ));
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
                            let mut pm = pm_for_sighup.write().await;
                            let added = resolved.len();
                            for (socket_addr, trustable, advertise) in resolved {
                                pm.add_config_peer(socket_addr, trustable, advertise);
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
        n2n_server.set_connection_metrics(Arc::new(N2NConnectionMetrics {
            metrics: self.metrics.clone(),
        }));
        // Get the broadcast senders before spawning the server
        self.block_announcement_tx = Some(n2n_server.block_announcement_sender());
        self.rollback_announcement_tx = Some(n2n_server.rollback_announcement_sender());
        debug!(
            "N2N server: diffusion_mode={:?}, peer_sharing=enabled",
            self.peer_manager.read().await.diffusion_mode()
        );
        let n2n_shutdown_rx = shutdown_rx.clone();
        let n2n_shutdown_tx = shutdown_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = n2n_server.listen(n2n_shutdown_rx).await {
                error!("N2N server error: {e}");
                // Fatal: trigger node shutdown on bind failure (e.g. address already in use)
                n2n_shutdown_tx.send(true).ok();
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

                    // Resolve all DNS addresses before acquiring the write lock
                    let mut resolved_addrs = Vec::new();
                    for (host, port) in sample {
                        if let Ok(mut addrs) =
                            tokio::net::lookup_host(format!("{host}:{port}")).await
                        {
                            if let Some(socket_addr) = addrs.next() {
                                resolved_addrs.push(socket_addr);
                            }
                        }
                    }
                    if !resolved_addrs.is_empty() {
                        let mut pm_w = pm.write().await;
                        for socket_addr in &resolved_addrs {
                            pm_w.add_ledger_peer(*socket_addr);
                        }
                        let added = resolved_addrs.len();
                        debug!(
                            "Ledger peer discovery: +{added} peers from {} relays, {}",
                            relays.len(),
                            pm_w.stats()
                        );
                    }
                }
            });
        }

        let network_magic = self.network_magic;

        // Initialize Genesis State Machine (GSM)
        let genesis_enabled = self.consensus_mode == "genesis";
        let gsm_config = crate::gsm::GsmConfig {
            marker_path: self.database_path.join("caught_up.marker"),
            ..Default::default()
        };
        let gsm = std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::gsm::GenesisStateMachine::new(gsm_config, genesis_enabled),
        ));
        if genesis_enabled {
            info!(
                state = %gsm.blocking_read().state(),
                "Genesis mode enabled"
            );
        }

        // Spawn GSM evaluation task
        if genesis_enabled {
            let gsm_ref = gsm.clone();
            let gsm_pm = peer_manager.clone();
            let gsm_shutdown = shutdown_rx.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
                interval.tick().await;
                let mut shutdown = gsm_shutdown;
                loop {
                    tokio::select! {
                        _ = interval.tick() => {}
                        _ = shutdown.changed() => { break; }
                    }

                    let active_blp = {
                        let pm = gsm_pm.read().await;
                        pm.active_big_ledger_peer_count()
                    };

                    let mut gsm_w = gsm_ref.write().await;
                    // TODO: compute tip_age and chainsync_idle from actual state
                    let tip_age_secs = 0u64;
                    let all_idle = false;
                    gsm_w.evaluate(active_blp, all_idle, tip_age_secs);
                }
            });
        }

        // Spawn the P2P governor task — periodically evaluates peer targets
        // and emits connect/disconnect/promote/demote events
        {
            let governor_pm = peer_manager.clone();
            let governor_shutdown = shutdown_rx.clone();
            tokio::spawn(async move {
                let mut governor = Governor::new(Default::default());
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
                interval.tick().await; // skip first immediate tick
                let mut shutdown = governor_shutdown;
                loop {
                    tokio::select! {
                        _ = interval.tick() => {}
                        _ = shutdown.changed() => { break; }
                    }

                    // Run governor evaluation
                    let events = {
                        let pm = governor_pm.read().await;
                        let mut all_events = governor.evaluate(&pm);
                        // Also check churn
                        all_events.extend(governor.maybe_churn(&pm));
                        all_events
                    };

                    // Execute governor events
                    if !events.is_empty() {
                        let mut pm = governor_pm.write().await;
                        for event in &events {
                            match event {
                                GovernorEvent::Promote(addr) => {
                                    pm.promote_to_hot(addr);
                                }
                                GovernorEvent::Demote(addr) => {
                                    pm.demote_to_warm(addr);
                                }
                                GovernorEvent::Disconnect(addr) => {
                                    pm.peer_disconnected(addr);
                                }
                                GovernorEvent::Connect(_) => {
                                    // Connection events are handled by the main loop
                                    // via peers_to_connect()
                                }
                            }
                        }
                        pm.recompute_reputations();
                    }
                }
            });
        }

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
                    if *shutdown_rx.borrow() {
                        break;
                    }
                    let target = addr.to_string();
                    debug!("Connecting to peer {target}...");
                    let connect_start = std::time::Instant::now();
                    let connect_result = tokio::select! {
                        r = NodeToNodeClient::connect(&*target, network_magic) => r,
                        _ = shutdown_rx.changed() => break,
                    };
                    match connect_result {
                        Ok(mut c) => {
                            c.set_byron_epoch_length(self.byron_epoch_length);
                            let rtt_ms = connect_start.elapsed().as_secs_f64() * 1000.0;
                            self.metrics.record_handshake_rtt(rtt_ms);
                            let mut pm = peer_manager.write().await;
                            pm.peer_connected(addr, 14, true);
                            pm.record_handshake_rtt(addr, rtt_ms);
                            pm.promote_to_hot(addr);
                            drop(pm);
                            info!(peer = %target, rtt_ms = format_args!("{rtt_ms:.0}"), "Peer connected");
                            client = Some((c, *addr));
                            break;
                        }
                        Err(e) => {
                            peer_manager.write().await.peer_failed(addr);
                            debug!("Failed to connect to {target}: {e}");
                        }
                    }
                }
            } else {
                // Fallback: try topology peers directly
                for (addr, port) in &peers {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                    let target = format!("{addr}:{port}");
                    debug!("Connecting to peer {target}...");
                    let connect_result = tokio::select! {
                        r = NodeToNodeClient::connect(&*target, network_magic) => r,
                        _ = shutdown_rx.changed() => break,
                    };
                    match connect_result {
                        Ok(mut c) => {
                            c.set_byron_epoch_length(self.byron_epoch_length);
                            info!(peer = %target, "Peer connected");
                            let sock_addr = c.remote_addr().to_owned();
                            client = Some((c, sock_addr));
                            break;
                        }
                        Err(e) => {
                            debug!("Failed to connect to {target}: {e}");
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
                debug!("P2P: {}", pm.stats());
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
                                debug!(
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
                    if *shutdown_rx.borrow() {
                        break;
                    }
                    let target = addr.to_string();
                    let connect_start = std::time::Instant::now();
                    let connect_result = tokio::select! {
                        r = NodeToNodeClient::connect(&*target, network_magic) => r,
                        _ = shutdown_rx.changed() => break,
                    };
                    match connect_result {
                        Ok(mut c) => {
                            c.set_byron_epoch_length(self.byron_epoch_length);
                            let rtt_ms = connect_start.elapsed().as_secs_f64() * 1000.0;
                            self.metrics.record_handshake_rtt(rtt_ms);
                            let mut pm = peer_manager.write().await;
                            pm.peer_connected(addr, 14, true);
                            pm.record_handshake_rtt(addr, rtt_ms);
                            pm.promote_to_hot(addr);
                            drop(pm);
                            debug!("Connected block fetcher to {target} ({rtt_ms:.0}ms)");
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
                if fetch_pool.is_empty() && !*shutdown_rx.borrow() {
                    let target = peer_addr.to_string();
                    let connect_result = tokio::select! {
                        r = NodeToNodeClient::connect(&*target, network_magic) => r,
                        _ = shutdown_rx.changed() => {
                            info!(fetchers = 0, "Block fetchers ready");
                            continue;
                        }
                    };
                    match connect_result {
                        Ok(mut c) => {
                            c.set_byron_epoch_length(self.byron_epoch_length);
                            debug!("Connected dedicated block fetcher to primary peer {target}");
                            fetch_pool.add_fetcher(c);
                        }
                        Err(e) => {
                            warn!("Failed to connect dedicated fetcher to {target}: {e}");
                        }
                    }
                }
                info!(fetchers = fetch_pool.len(), "Block fetchers ready");
            }

            // Create pipelined ChainSync connection to same peer for high-throughput headers
            if *shutdown_rx.borrow() {
                break;
            }
            let pipelined_client = {
                let target = peer_addr.to_string();
                let connect_result = tokio::select! {
                    r = PipelinedPeerClient::connect(&*target, network_magic) => r,
                    _ = shutdown_rx.changed() => { break; }
                };
                match connect_result {
                    Ok(mut pc) => {
                        pc.set_byron_epoch_length(self.byron_epoch_length);
                        debug!("Pipelined ChainSync client connected to {target}");
                        // Take the TxSubmission channel and spawn a background tx fetcher
                        if let Some(txsub_channel) = pc.take_txsub_channel() {
                            let mempool = self.mempool.clone();
                            let ledger = self.ledger_state.clone();
                            let slot_config = self.ledger_state.read().await.slot_config;
                            let shutdown = shutdown_rx.clone();
                            let txsub_metrics = self.metrics.clone();
                            tokio::spawn(async move {
                                let validator: Option<Arc<dyn TxValidator>> =
                                    Some(Arc::new(LedgerTxValidator {
                                        ledger,
                                        slot_config,
                                        metrics: txsub_metrics,
                                    }));
                                let mut client =
                                    torsten_network::TxSubmissionClient::new(txsub_channel);
                                let mut shutdown = shutdown;
                                tokio::select! {
                                    result = client.run(mempool, validator) => {
                                        match result {
                                            Ok(stats) => {
                                                debug!(
                                                    "TxSubmission2 session ended (rx={}, ok={}, rej={}, dup={})",
                                                    stats.received, stats.accepted, stats.rejected, stats.duplicate,
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
                    info!("Peer disconnected, reconnecting...");
                }
                Err(e) => {
                    // Mark as failed (not just disconnected) so PeerManager
                    // deprioritizes this peer on the next connection attempt.
                    // This is important after sleep/hibernate where stale peers
                    // should be avoided in favor of responsive ones.
                    peer_manager.write().await.peer_failed(&peer_addr);
                    warn!("Sync error: {e}, will reconnect...");
                }
            }

            // Brief delay before reconnecting
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
                _ = shutdown_rx.changed() => { break; }
            }
        }

        // Flush all volatile blocks to ImmutableDB, then persist.
        // This ensures the ledger snapshot (at the volatile tip) is consistent
        // with the ImmutableDB tip on restart, avoiding a full replay.
        {
            let mut db = self.chain_db.write().await;
            match db.flush_all_to_immutable() {
                Ok(n) if n > 0 => info!(blocks = n, "Flushed volatile blocks to ImmutableDB"),
                Ok(_) => {}
                Err(e) => error!("Failed to flush volatile blocks on shutdown: {e}"),
            }
            if let Err(e) = db.persist() {
                error!("Failed to persist ChainDB on shutdown: {e}");
            }
        }
        self.save_ledger_snapshot().await;
        info!("Shutdown complete");
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

        // Copy to "latest" for fast startup (avoids double-serializing ~1 GB)
        let latest_path = self.database_path.join("ledger-snapshot.bin");
        if let Err(e) = std::fs::copy(&epoch_path, &latest_path) {
            error!("Failed to copy latest ledger snapshot: {e}");
        }

        drop(ls);

        // Prune old snapshots — keep only the configured maximum
        self.prune_old_snapshots(self.snapshot_policy.max_snapshots + 1);
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
    async fn replay_ledger_from_storage(&mut self) {
        // Migrate legacy immutable-replay/ to immutable/ (backwards compat)
        let legacy_dir = self.database_path.join("immutable-replay");
        let immutable_dir = self.database_path.join("immutable");
        if legacy_dir.is_dir() && !immutable_dir.is_dir() {
            debug!("Migrating legacy immutable-replay/ to immutable/");
            if let Err(e) = std::fs::rename(&legacy_dir, &immutable_dir) {
                warn!("Failed to migrate immutable-replay/ to immutable/: {e}");
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
                    immutable_tip_slot = imm_tip_slot,
                    "Replaying ledger from chunk files",
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
            info!(blocks_behind, "Replaying blocks (time-based snapshots)",);
        }

        info!(
            ledger_slot,
            db_tip_slot, blocks_behind, "Replaying ledger from ChainDB (LSM mode)",
        );
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

        let security_param = self
            .shelley_genesis
            .as_ref()
            .map(|g| g.security_param)
            .unwrap_or(2160);
        let result = tokio::task::spawn_blocking(move || {
            let start = std::time::Instant::now();
            let mut replayed = 0u64;
            let mut skipped = 0u64;
            let mut last_log = std::time::Instant::now();
            let mut snapshot_policy = SnapshotPolicy::new(security_param);

            // Get ledger tip slot so we can skip blocks already applied.
            let ledger_tip_slot = {
                let ls = ledger_state.blocking_read();
                info!(
                    ledger_tip_slot = ls.tip.point.slot().map(|s| s.0).unwrap_or(0),
                    utxos = ls.utxo_set.len(),
                    "Chunk replay starting",
                );
                ls.tip.point.slot().map(|s| s.0).unwrap_or(0)
            };

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
                        // Skip blocks at or before the ledger snapshot position
                        if block.slot().0 <= ledger_tip_slot {
                            skipped += 1;
                            return Ok(());
                        }

                        let mut ls_guard = ledger_state.blocking_write();
                        if let Err(e) = ls_guard.apply_block(&block, BlockValidationMode::ApplyOnly) {
                            warn!(slot = block.slot().0, error = %e, "Ledger apply failed during replay");
                        }
                        replayed += 1;
                        snapshot_policy.record_blocks(1);

                        if last_log.elapsed().as_secs() >= 5 {
                            let elapsed = start.elapsed().as_secs_f64();
                            let speed = replayed as f64 / elapsed;
                            let slot = ls_guard.tip.point.slot().map(|s| s.0).unwrap_or(0);
                            let utxos = ls_guard.utxo_set.len();
                            info!(
                                blocks = replayed,
                                slot,
                                speed = format_args!("{speed:.0} blk/s"),
                                utxos,
                                "Replay",
                            );
                            last_log = std::time::Instant::now();
                        }

                        if snapshot_policy.should_snapshot_bulk() {
                            if let Err(e) = ls_guard.save_snapshot(&snapshot_path) {
                                warn!("Failed to save ledger snapshot during replay: {e}");
                            }
                            snapshot_policy.snapshot_taken();
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
                        replayed as f64 / elapsed
                    } else {
                        0.0
                    };
                    info!(
                        "Replay       complete ({} blocks in {}s, {} applied, {} skipped, {} blk/s)",
                        total, elapsed as u64, replayed, skipped, speed as u64,
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
                ls.utxo_set.rebuild_address_index();
                // Rebuild stake distribution from UTxO set and recompute snapshot pool_stakes
                // to ensure the saved snapshot has correct values.
                ls.needs_stake_rebuild = true;
                ls.rebuild_stake_distribution();
                ls.recompute_snapshot_pool_stakes();
                debug!("Rebuilt address index and stake distribution after chunk replay");
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
    async fn replay_from_lsm(&mut self, db_tip: torsten_primitives::block::Tip) {
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
                            if let Err(e) = ls.apply_block(&block, BlockValidationMode::ApplyOnly) {
                                warn!(
                                    "Replay       ledger apply failed at slot {} block {}: {e}",
                                    slot.0, block_no
                                );
                            }
                            replayed += 1;
                            self.snapshot_policy.record_blocks(1);

                            if last_log.elapsed().as_secs() >= 5 {
                                let elapsed = start.elapsed().as_secs_f64();
                                let speed = replayed as f64 / elapsed;
                                let pct = if end_block_no > 0 {
                                    block_no as f64 / end_block_no as f64 * 100.0
                                } else {
                                    0.0
                                };
                                info!(
                                    progress = format_args!("{pct:>6.2}%"),
                                    block = block_no,
                                    total = end_block_no,
                                    slot = slot.0,
                                    speed = format_args!("{speed:.0} blk/s"),
                                    utxos = ls.utxo_set.len(),
                                    "Replay",
                                );
                                last_log = std::time::Instant::now();
                            }

                            if self.snapshot_policy.should_snapshot_bulk() {
                                if let Err(e) = ls.save_snapshot(&snapshot_path) {
                                    warn!("Failed to save ledger snapshot during replay: {e}");
                                }
                                self.snapshot_policy.snapshot_taken();
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
            blocks = replayed,
            elapsed_secs = elapsed as u64,
            speed = format_args!("{} blk/s", speed as u64),
            "Replay complete",
        );

        // Re-enable address indexing and rebuild after replay
        {
            let mut ls = self.ledger_state.write().await;
            ls.utxo_set.set_indexing_enabled(true);
            ls.utxo_set.rebuild_address_index();
            // Rebuild stake distribution from UTxO set and recompute snapshot pool_stakes
            // to ensure the saved snapshot has correct values.
            ls.needs_stake_rebuild = true;
            ls.rebuild_stake_distribution();
            ls.recompute_snapshot_pool_stakes();
            debug!("Rebuilt address index and stake distribution after LSM replay");
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
        // When ChainDB is ahead of the ledger, verify the chain connects.
        // If ChainDB has blocks on a different fork (e.g., from a previous
        // run that stored blocks but couldn't apply them), use the ledger
        // tip to avoid re-downloading blocks that won't connect.
        let chain_tip = self.chain_db.read().await.get_tip().point;
        let ledger_tip = self.ledger_state.read().await.tip.point.clone();
        let mut known_points = Vec::new();
        let ledger_slot = ledger_tip.slot().map(|s| s.0).unwrap_or(0);
        let chain_slot = chain_tip.slot().map(|s| s.0).unwrap_or(0);

        // When ChainDB is ahead, check if its chain connects to the ledger.
        // If not (fork divergence), prefer the ledger tip for intersection.
        let mut use_chain_tip = chain_slot > ledger_slot;
        if use_chain_tip && ledger_tip != Point::Origin {
            // Check if the first ChainDB block after ledger tip connects
            let db = self.chain_db.read().await;
            if let Ok(Some((_next_slot, _hash, cbor))) =
                db.get_next_block_after_slot(torsten_primitives::time::SlotNo(ledger_slot))
            {
                if let Ok(block) =
                    torsten_serialization::multi_era::decode_block_with_byron_epoch_length(
                        &cbor,
                        self.byron_epoch_length,
                    )
                {
                    let ledger_hash = ledger_tip.hash();
                    if ledger_hash.is_some_and(|h| h != block.prev_hash()) {
                        warn!(
                            "ChainDB fork divergence detected: ChainDB blocks after ledger tip \
                             do not connect (expected prev_hash={}, got {}). \
                             Using ledger tip for intersection.",
                            ledger_hash.map(|h| h.to_hex()).unwrap_or_default(),
                            block.prev_hash().to_hex()
                        );
                        use_chain_tip = false;
                    }
                }
            }
        }

        if use_chain_tip {
            if chain_tip != Point::Origin {
                known_points.push(chain_tip.clone());
            }
            if ledger_tip != Point::Origin && ledger_tip != chain_tip {
                known_points.push(ledger_tip.clone());
            }
        } else {
            if ledger_tip != Point::Origin {
                known_points.push(ledger_tip.clone());
            }
            if chain_tip != Point::Origin && chain_tip != ledger_tip {
                known_points.push(chain_tip.clone());
            }
        }
        known_points.push(Point::Origin);
        if ledger_tip != chain_tip {
            debug!(
                "Ledger tip ({}) differs from ChainDB tip ({}), using {} for intersection",
                ledger_tip,
                chain_tip,
                if use_chain_tip { "ChainDB" } else { "ledger" }
            );
        }
        // Find intersection: use pipelined client if available, otherwise serial client
        let (intersect, remote_tip) = if let Some(ref mut pc) = pipelined {
            pc.find_intersect(known_points.clone()).await?
        } else {
            client.find_intersect(known_points).await?
        };

        match &intersect {
            Some(point) => info!(point = %point, "Sync intersection found"),
            None => info!("Sync starting from Origin"),
        }
        info!(remote_tip = %remote_tip, "Remote tip");

        // Stale peer detection: if the remote tip is significantly behind the
        // current wall-clock slot, this peer is likely stale or stuck. Disconnect
        // and let the outer loop try a different peer. This handles the case where
        // the node reconnects after sleep/hibernate and reaches a stale peer.
        if let Some(wall_clock) = self.current_wall_clock_slot() {
            let remote_tip_slot = remote_tip.point.slot().map(|s| s.0).unwrap_or(0);
            let lag_slots = wall_clock.0.saturating_sub(remote_tip_slot);
            // Allow 120 slots (2 minutes) of lag for normal network propagation
            if lag_slots > 120 {
                warn!(
                    remote_tip_slot,
                    wall_clock_slot = wall_clock.0,
                    lag_slots,
                    "Peer tip is {} slots behind wall clock, skipping stale peer",
                    lag_slots
                );
                return Err(anyhow::anyhow!(
                    "peer tip is {lag_slots} slots behind wall clock (stale)"
                ));
            }
        }

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
                depth = max_pipeline_depth,
                fetchers = fetch_pool.len(),
                "Sync mode: pipelined",
            );
        } else if use_pool {
            info!(fetchers = fetch_pool.len(), "Sync mode: multi-peer");
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

        // Pipeline decoupling: when both a pipelined client and fetch pool are
        // available, spawn a separate task for header/block fetching. This allows
        // network I/O to stay saturated while the main task processes blocks.
        // The fetch task sends blocks through a bounded channel; the main task
        // consumes and applies them. This matches cardano-node's architecture
        // where block download and ledger application run on separate threads.
        if use_pipelined && use_pool {
            /// Messages from the block fetch pipeline to the processing loop.
            enum PipelineMsg {
                /// A batch of blocks fetched from the network.
                Batch {
                    blocks: Vec<torsten_primitives::block::Block>,
                    tip: torsten_primitives::block::Tip,
                    fetch_ms: f64,
                    header_count: u64,
                },
                /// Chain rollback — process any preceding blocks, then rollback.
                Rollback(Point),
                /// Caught up to chain tip — enable strict verification.
                AtTip,
                /// Fetch error — abort the pipeline.
                FetchError(String),
            }

            let mut pc = pipelined
                .take()
                .expect("use_pipelined implies pipelined is Some");
            let (depth_tx, depth_rx) = tokio::sync::watch::channel(max_pipeline_depth);
            // Bounded channel: 4 batches of buffering allows network to stay
            // saturated while CPU catches up on block processing.
            let (block_tx, mut block_rx) = tokio::sync::mpsc::channel::<PipelineMsg>(4);
            let fetch_shutdown = shutdown_rx.clone();

            let fetch_handle = tokio::spawn(async move {
                loop {
                    if *fetch_shutdown.borrow() {
                        break;
                    }
                    let depth = *depth_rx.borrow();

                    let result = pc
                        .request_headers_pipelined_with_depth(header_batch_size, depth)
                        .await;
                    match result {
                        Ok(HeaderBatchResult::Headers(headers, tip)) => {
                            if headers.is_empty() {
                                continue;
                            }
                            debug!(
                                header_count = headers.len(),
                                first_slot = headers[0].slot,
                                last_slot = headers.last().expect("headers is non-empty").slot,
                                "Pipeline: headers received"
                            );
                            let fetch_start = std::time::Instant::now();
                            let header_count = headers.len() as u64;
                            match fetch_pool.fetch_blocks_concurrent(&headers).await {
                                Ok(blocks) => {
                                    let fetch_ms = fetch_start.elapsed().as_secs_f64() * 1000.0;
                                    if block_tx
                                        .send(PipelineMsg::Batch {
                                            blocks,
                                            tip,
                                            fetch_ms,
                                            header_count,
                                        })
                                        .await
                                        .is_err()
                                    {
                                        break; // receiver dropped
                                    }
                                }
                                Err(e) => {
                                    let _ = block_tx
                                        .send(PipelineMsg::FetchError(format!("{e}")))
                                        .await;
                                    break;
                                }
                            }
                        }
                        Ok(HeaderBatchResult::HeadersAndRollback {
                            headers,
                            tip,
                            rollback_point,
                            ..
                        }) => {
                            // Fetch blocks for headers before the rollback point
                            if !headers.is_empty() {
                                if let Ok(blocks) =
                                    fetch_pool.fetch_blocks_concurrent(&headers).await
                                {
                                    let _ = block_tx
                                        .send(PipelineMsg::Batch {
                                            blocks,
                                            tip,
                                            fetch_ms: 0.0,
                                            header_count: headers.len() as u64,
                                        })
                                        .await;
                                }
                            }
                            if block_tx
                                .send(PipelineMsg::Rollback(rollback_point))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                        Ok(HeaderBatchResult::RollBackward(point, _)) => {
                            if block_tx.send(PipelineMsg::Rollback(point)).await.is_err() {
                                break;
                            }
                        }
                        Ok(HeaderBatchResult::HeadersAtTip(headers, tip)) => {
                            // We got headers AND caught up to tip. Fetch the
                            // blocks, send the batch, then signal AtTip.
                            if !headers.is_empty() {
                                debug!(
                                    header_count = headers.len(),
                                    first_slot = headers[0].slot,
                                    last_slot = headers.last().expect("non-empty").slot,
                                    "Pipeline: headers at tip"
                                );
                                let fetch_start = std::time::Instant::now();
                                let header_count = headers.len() as u64;
                                match fetch_pool.fetch_blocks_concurrent(&headers).await {
                                    Ok(blocks) => {
                                        let fetch_ms = fetch_start.elapsed().as_secs_f64() * 1000.0;
                                        if block_tx
                                            .send(PipelineMsg::Batch {
                                                blocks,
                                                tip,
                                                fetch_ms,
                                                header_count,
                                            })
                                            .await
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        let _ = block_tx
                                            .send(PipelineMsg::FetchError(format!("{e}")))
                                            .await;
                                        break;
                                    }
                                }
                            }
                            if block_tx.send(PipelineMsg::AtTip).await.is_err() {
                                break;
                            }
                        }
                        Ok(HeaderBatchResult::Await) => {
                            // Depth reduction is signaled by the main loop via
                            // the watch channel when it processes AtTip.
                            if block_tx.send(PipelineMsg::AtTip).await.is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            let _ = block_tx.send(PipelineMsg::FetchError(format!("{e}"))).await;
                            break;
                        }
                    }

                    // Connection stays open at tip — remaining in-flight
                    // requests drain naturally as new blocks arrive.
                }
            });

            // Processing loop: consume block batches from the pipeline channel.
            // Block processing and network fetching now run concurrently.
            loop {
                tokio::select! {
                    msg = block_rx.recv() => {
                        match msg {
                            Some(PipelineMsg::Batch { blocks, tip, fetch_ms, header_count }) => {
                                if header_count > 0 {
                                    self.metrics.record_block_fetch_latency(fetch_ms / header_count as f64);
                                }
                                self.peer_manager.write().await.record_block_fetch(
                                    &peer_addr, fetch_ms, header_count, 0,
                                );
                                let applied = self.process_forward_blocks(
                                    blocks, &tip, &mut blocks_received,
                                    &mut blocks_since_last_log, &mut last_snapshot_epoch,
                                    &mut last_log_time, &mut last_query_update,
                                ).await;
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
                            Some(PipelineMsg::Rollback(point)) => {
                                warn!("Rollback to {point}");
                                self.handle_rollback(&point).await;
                            }
                            Some(PipelineMsg::AtTip) => {
                                if !self.consensus.strict_verification() {
                                    info!(blocks_applied = blocks_received, "Caught up to chain tip");
                                    self.enable_strict_verification().await;
                                }
                                self.update_query_state().await;
                                self.try_forge_block().await;
                                // Reduce pipeline depth to 1 at tip
                                let _ = depth_tx.send(1);
                            }
                            Some(PipelineMsg::FetchError(e)) => {
                                warn!("Block fetch pipeline error: {e}");
                                break;
                            }
                            None => {
                                // Channel closed — fetch task exited (stale or shutdown)
                                debug!("Fetch pipeline channel closed, ending sync loop");
                                break;
                            }
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
                        info!("Shutdown: stopping sync");
                        break;
                    }
                }
            }

            // Cleanup: close channel and abort fetch task
            drop(block_rx);
            fetch_handle.abort();
        } else {
            // Sequential mode: no pipeline decoupling (single peer or no fetch pool)
            loop {
                if *shutdown_rx.borrow() {
                    info!("Shutdown: stopping sync");
                    break;
                }

                if use_pipelined || use_pool {
                    // Pipelined/multi-peer mode without separate fetch pool
                    let header_future = async {
                        if let Some(ref mut pc) = pipelined {
                            pc.request_headers_pipelined_with_depth(
                                header_batch_size,
                                pipeline_depth,
                            )
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
                                            if headers.len() > 10 && pipeline_depth < max_pipeline_depth {
                                                pipeline_depth = max_pipeline_depth;
                                            }
                                            if !headers.is_empty() {
                                                debug!(
                                                    header_count = headers.len(),
                                                    first_slot = headers[0].slot,
                                                    first_block = headers[0].block_no,
                                                    last_slot = headers.last().expect("headers is non-empty").slot,
                                                    last_block = headers.last().expect("headers is non-empty").block_no,
                                                    "Headers received from pipelined client"
                                                );
                                            }
                                            let fetch_start = std::time::Instant::now();
                                            let header_count = headers.len() as u64;
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
                                                    if header_count > 0 {
                                                        self.metrics.record_block_fetch_latency(fetch_ms / header_count as f64);
                                                    }
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
                                        HeaderBatchResult::HeadersAtTip(headers, tip) => {
                                            // Got headers AND caught up to tip
                                            if !headers.is_empty() {
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
                                                if let Ok(blocks) = blocks_result {
                                                    self.process_forward_blocks(blocks, &tip, &mut blocks_received, &mut blocks_since_last_log, &mut last_snapshot_epoch, &mut last_log_time, &mut last_query_update).await;
                                                }
                                            }
                                            if !self.consensus.strict_verification() {
                                                info!(blocks_applied = blocks_received, "Caught up to chain tip");
                                                self.enable_strict_verification().await;
                                            }
                                            self.update_query_state().await;
                                            self.try_forge_block().await;
                                            pipeline_depth = 1;
                                        }
                                        HeaderBatchResult::Await => {
                                            if !self.consensus.strict_verification() {
                                                info!(blocks_applied = blocks_received, "Caught up to chain tip");
                                                self.enable_strict_verification().await;
                                            }
                                            self.update_query_state().await;
                                            self.try_forge_block().await;
                                            pipeline_depth = 1;
                                        }
                                    }
                                    // Connection stays open at tip — remaining in-flight
                                    // requests drain naturally as new blocks arrive.
                                }
                                Err(e) => { error!("Chain sync error: {e}"); break; }
                            }
                        }
                        _ = forge_ticker.tick(), if self.block_producer.is_some() && pipeline_depth <= 1 => {
                            if let Some(wc) = self.current_wall_clock_slot() {
                                if wc.0 > last_forge_slot {
                                    last_forge_slot = wc.0;
                                    self.try_forge_block().await;
                                }
                            }
                        }
                        _ = shutdown_rx.changed() => {
                            info!("Shutdown: stopping sync");
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
                                        let tip = forward_blocks.last().expect("forward_blocks is non-empty (checked above)").1.clone();
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
                                                if !self.consensus.strict_verification() {
                                                    info!(blocks_applied = blocks_received, "Caught up to chain tip");
                                                    self.enable_strict_verification().await;
                                                }
                                                self.update_query_state().await;
                                            }
                                            ChainSyncEvent::RollForward(..) => {
                                                warn!("Unexpected RollForward in other_events, skipping");
                                                continue;
                                            }
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
                            info!("Shutdown: stopping sync");
                            break;
                        }
                    }
                }
            }
            fetch_pool.disconnect_all().await;
        }

        debug!("Chain sync stopped after {blocks_received} blocks");
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
        // Two-phase validation matching Haskell's cardano-node:
        //
        // During initial sync (non-strict), use Replay mode — skip all cryptographic
        // verification (VRF, KES, opcert Ed25519). This matches Haskell's
        // `reupdateChainDepState` behavior for blocks from the immutable chain.
        // Historical blocks are validated by hash-chain connectivity.
        //
        // At tip (strict), use Full mode with parallel crypto verification via rayon.
        // This matches Haskell's `updateChainDepState` for new network blocks.
        let strict = self.consensus.strict_verification();
        let mode = if strict {
            ValidationMode::Full
        } else {
            ValidationMode::Replay
        };
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

            // Phase 1: Sequential structural validation + state updates.
            // Uses Replay mode during sync (skip crypto) or Full mode at tip.
            // Opcert counter tracking and structural checks always run.
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
                    mode,
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
                    debug!(
                        "Gap detected (ledger slot={}, first block slot={}) — bridging from ChainDB",
                        ledger_slot, first_new.slot().0,
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
                                        if let Err(e) = ls.apply_block(&block, BlockValidationMode::ApplyOnly) {
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
                                        warn!(slot = next_slot.0, error = %e, "Gap bridge decode failed");
                                        bridge_slot = next_slot.0;
                                    }
                                }
                            }
                            _ => break,
                        }
                    }
                    if bridged > 0 {
                        debug!("Bridged {bridged} blocks from ChainDB storage");
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
                let ledger_mode = if strict || self.validate_all_blocks {
                    BlockValidationMode::ValidateAll
                } else {
                    BlockValidationMode::ApplyOnly
                };
                if let Err(e) = ls.apply_block(block, ledger_mode) {
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

        // Remove confirmed transactions from mempool, then full revalidation
        if !self.mempool.is_empty() {
            let confirmed_hashes: Vec<_> = blocks
                .iter()
                .flat_map(|b| b.transactions.iter().map(|tx| tx.hash))
                .collect();
            if !confirmed_hashes.is_empty() {
                self.mempool.remove_txs(&confirmed_hashes);
            }

            // Full revalidation: check each remaining tx for input conflicts,
            // TTL expiry, and any other invalidity in one pass.
            let consumed_inputs: std::collections::HashSet<_> = blocks
                .iter()
                .flat_map(|b| b.transactions.iter())
                .flat_map(|tx| tx.body.inputs.iter().cloned())
                .collect();
            let current_slot = blocks.last().map(|b| b.slot());
            self.mempool.revalidate_all(|tx| {
                // Reject if any input was consumed by the new block
                if tx
                    .body
                    .inputs
                    .iter()
                    .any(|input| consumed_inputs.contains(input))
                {
                    return false;
                }
                // Reject if TTL has expired
                if let (Some(ttl), Some(slot)) = (tx.body.ttl, current_slot) {
                    if slot.0 > ttl.0 {
                        return false;
                    }
                }
                true
            });
        }

        if let Some(last_block) = blocks.last() {
            self.consensus.update_tip(last_block.tip());
        }

        // Flush finalized blocks from VolatileDB to ImmutableDB.
        // Blocks deeper than k are appended to chunk files.
        {
            let mut db = self.chain_db.write().await;
            if let Err(e) = db.flush_to_immutable() {
                warn!(error = %e, "Failed to flush blocks to immutable storage");
            }
        }

        let tx_count: u64 = blocks.iter().map(|b| b.transactions.len() as u64).sum();

        *blocks_received += batch_count;
        *blocks_since_last_log += batch_count;
        self.snapshot_policy.record_blocks(batch_count);
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
            // Safety: function returns early if blocks.is_empty()
            .expect("blocks is non-empty (checked at function entry)");
        let slot = last_block.slot().0;
        let block_no = last_block.block_number().0;
        self.metrics.set_slot(slot);
        self.metrics.set_block_number(block_no);

        // Log each new block when following the tip (individual blocks matter at tip)
        // and announce to connected downstream peers so they receive new blocks
        if strict {
            for block in &blocks {
                let hash_hex = block.hash().to_hex();
                info!(
                    era = %block.era,
                    slot = block.slot().0,
                    block = block.block_number().0,
                    txs = block.transactions.len(),
                    hash = %hash_hex,
                    "New block",
                );
            }

            // Announce the latest block to all connected N2N peers
            // This enables relay behavior: downstream peers waiting at tip (MsgAwaitReply)
            // will receive MsgRollForward for blocks we synced from upstream
            if let Some(ref tx) = self.block_announcement_tx {
                let mut hash_bytes = [0u8; 32];
                hash_bytes.copy_from_slice(last_block.hash().as_ref());
                tx.send(torsten_network::BlockAnnouncement {
                    slot,
                    hash: hash_bytes,
                    block_number: block_no,
                })
                .ok();
            }
        }

        {
            let current_epoch = self.ledger_state.read().await.epoch.0;
            if current_epoch > *last_snapshot_epoch {
                // Count ALL epoch transitions (batches may span multiple epochs)
                let epochs_crossed = (current_epoch - *last_snapshot_epoch) as u32;
                info!(
                    epoch = current_epoch,
                    crossed = epochs_crossed,
                    "Epoch transition",
                );
                self.epoch_transitions_observed = self
                    .epoch_transitions_observed
                    .saturating_add(epochs_crossed);

                // Finalize immutable chunk at epoch boundary and persist
                {
                    let mut db = self.chain_db.write().await;
                    if let Err(e) = db.finalize_immutable_chunk() {
                        warn!(error = %e, "Failed to finalize immutable chunk at epoch transition");
                    }
                    match db.persist() {
                        Ok(()) => info!(
                            epoch = current_epoch,
                            "ChainDB persisted at epoch transition"
                        ),
                        Err(e) => {
                            warn!(error = %e, "Failed to persist ChainDB at epoch transition")
                        }
                    }
                }
                if self.snapshot_policy.should_snapshot_normal() {
                    self.save_ledger_snapshot().await;
                    self.snapshot_policy.snapshot_taken();
                }
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
                        progress = format_args!("{progress:.2}%"),
                        epoch = ls.epoch.0,
                        block = block_no,
                        tip = tip_block,
                        remaining = blocks_remaining,
                        speed = format_args!("{} blk/s", blocks_per_sec as u64),
                        utxos = ls.utxo_set.len(),
                        "Syncing",
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
        // Stake snapshots need 3 epoch transitions to fully rotate with correct
        // rebuilt data (mark→set→go). Until then, VRF leader eligibility failures
        // are non-fatal to avoid rejecting valid blocks with approximate sigma values.
        self.consensus.snapshots_established = self.epoch_transitions_observed >= 3;
        if !self.consensus.nonce_established {
            debug!(
                transitions = self.epoch_transitions_observed,
                "VRF proof verification deferred: epoch nonce not yet established (need 2 epoch transitions)"
            );
        }
        if !self.consensus.snapshots_established {
            debug!(
                transitions = self.epoch_transitions_observed,
                "VRF leader check non-fatal: stake snapshots not yet established (need 3 epoch transitions)"
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
        for (cred_hash, pool_id) in ls.delegations.iter() {
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
                // Build per-credential vote maps from votes_by_action
                let mut committee_votes = Vec::new();
                let mut drep_votes = Vec::new();
                let mut spo_votes = Vec::new();
                if let Some(votes) = ls.governance.votes_by_action.get(action_id) {
                    for (voter, procedure) in votes {
                        let vote_u8 = match procedure.vote {
                            torsten_primitives::transaction::Vote::No => 0u8,
                            torsten_primitives::transaction::Vote::Yes => 1u8,
                            torsten_primitives::transaction::Vote::Abstain => 2u8,
                        };
                        use torsten_primitives::transaction::Voter;
                        match voter {
                            Voter::ConstitutionalCommittee(cred) => {
                                let (cred_type, hash) = credential_to_bytes(cred);
                                committee_votes.push((hash, cred_type, vote_u8));
                            }
                            Voter::DRep(cred) => {
                                let (cred_type, hash) = credential_to_bytes(cred);
                                drep_votes.push((hash, cred_type, vote_u8));
                            }
                            Voter::StakePool(pool_hash) => {
                                // SPO uses bare KeyHash (28 bytes)
                                spo_votes.push((pool_hash.as_ref()[..28].to_vec(), vote_u8));
                            }
                        }
                    }
                }

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
                    committee_votes,
                    drep_votes,
                    spo_votes,
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
            pending_retirements: ls
                .pending_retirements
                .iter()
                .map(|(epoch, pools)| {
                    (epoch.0, pools.iter().map(|h| h.as_ref().to_vec()).collect())
                })
                .collect(),
            pool_deposit: ls.protocol_params.pool_deposit.0,
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
            ratify_enacted: ls
                .governance
                .last_ratified
                .iter()
                .map(|(action_id, state)| {
                    let action_type = match &state.procedure.gov_action {
                        torsten_primitives::transaction::GovAction::ParameterChange { .. } => {
                            "ParameterChange"
                        }
                        torsten_primitives::transaction::GovAction::HardForkInitiation {
                            ..
                        } => "HardForkInitiation",
                        torsten_primitives::transaction::GovAction::TreasuryWithdrawals {
                            ..
                        } => "TreasuryWithdrawals",
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
                    // Build vote maps from votes_by_action (using pre-removal snapshot)
                    let mut committee_votes = Vec::new();
                    let mut drep_votes = Vec::new();
                    let mut spo_votes = Vec::new();
                    if let Some(votes) = ls.governance.votes_by_action.get(action_id) {
                        for (voter, procedure) in votes {
                            let vote_u8 = match procedure.vote {
                                torsten_primitives::transaction::Vote::No => 0u8,
                                torsten_primitives::transaction::Vote::Yes => 1u8,
                                torsten_primitives::transaction::Vote::Abstain => 2u8,
                            };
                            use torsten_primitives::transaction::Voter;
                            match voter {
                                Voter::ConstitutionalCommittee(cred) => {
                                    let (cred_type, hash) = credential_to_bytes(cred);
                                    committee_votes.push((hash, cred_type, vote_u8));
                                }
                                Voter::DRep(cred) => {
                                    let (cred_type, hash) = credential_to_bytes(cred);
                                    drep_votes.push((hash, cred_type, vote_u8));
                                }
                                Voter::StakePool(pool_hash) => {
                                    spo_votes.push((pool_hash.as_ref()[..28].to_vec(), vote_u8));
                                }
                            }
                        }
                    }
                    let proposal = ProposalSnapshot {
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
                        committee_votes,
                        drep_votes,
                        spo_votes,
                    };
                    let gov_id = torsten_network::query_handler::GovActionId {
                        tx_id: action_id.transaction_id.as_ref().to_vec(),
                        action_index: action_id.action_index,
                    };
                    (proposal, gov_id)
                })
                .collect(),
            ratify_expired: ls
                .governance
                .last_expired
                .iter()
                .map(|id| torsten_network::query_handler::GovActionId {
                    tx_id: id.transaction_id.as_ref().to_vec(),
                    action_index: id.action_index,
                })
                .collect(),
            ratify_delayed: ls.governance.last_ratify_delayed,
            epoch_nonce: ls.epoch_nonce.as_ref().to_vec(),
            evolving_nonce: ls.evolving_nonce.as_ref().to_vec(),
            candidate_nonce: ls.candidate_nonce.as_ref().to_vec(),
            lab_nonce: ls.lab_nonce.as_ref().to_vec(),
            total_active_stake: ls
                .pool_params
                .keys()
                .filter_map(|pid| {
                    ls.snapshots
                        .set
                        .as_ref()
                        .and_then(|s| s.pool_stake.get(pid))
                        .map(|s| s.0)
                })
                .sum(),
            total_rewards: ls.reward_accounts.values().map(|r| r.0).sum(),
            active_delegations: ls.delegations.len() as u64,
            protocol_version_major: ls.protocol_params.protocol_version_major,
            protocol_version_minor: ls.protocol_params.protocol_version_minor,
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
        ls: &torsten_ledger::LedgerState,
    ) -> Vec<torsten_network::query_handler::EraSummary> {
        use torsten_network::query_handler::{EraBound, EraSummary};

        let shelley_epoch_length = self
            .shelley_genesis
            .as_ref()
            .map(|g| g.epoch_length)
            .unwrap_or(432000);
        let shelley_slot_length_ms = self
            .shelley_genesis
            .as_ref()
            .map(|g| g.slot_length * 1000)
            .unwrap_or(1000);
        let k = self
            .shelley_genesis
            .as_ref()
            .map(|g| g.security_param)
            .unwrap_or(2160);
        let active_slots_coeff = self
            .shelley_genesis
            .as_ref()
            .map(|g| g.active_slots_coeff)
            .unwrap_or(0.05);

        let is_mainnet = self.network_magic == 764824073;

        // Byron params: epoch length and slot duration from genesis
        let byron_epoch_len: u64 = if self.byron_epoch_length > 0 {
            self.byron_epoch_length
        } else if is_mainnet {
            21600
        } else {
            4320
        };
        let byron_slot_len_ms: u64 = self.byron_slot_duration_ms;
        let byron_safe_zone = k * 2; // Byron safe zone = 2k (864 for preview, matches Haskell)
        let byron_genesis_window = k * 2;

        // Shelley+ safe zone and genesis window: 3 * k / f
        let shelley_safe_zone = (3.0 * k as f64 / active_slots_coeff).floor() as u64;
        let shelley_genesis_window = shelley_safe_zone;

        if is_mainnet {
            // Mainnet: Byron ran for 208 epochs with 21600-slot epochs at 20s slots
            let byron_end_epoch: u64 = 208;
            let byron_end_slot = byron_end_epoch * byron_epoch_len;
            let byron_end_time_pico =
                byron_end_slot as u128 * byron_slot_len_ms as u128 * 1_000_000_000;

            // Compute how many Shelley slots have elapsed since Byron ended
            let shelley_start_slot = byron_end_slot;
            let shelley_start_epoch = byron_end_epoch;

            // Current epoch determines how far the Shelley+ eras extend
            let current_epoch = ls.epoch.0;

            // For mainnet, Babbage started at epoch 365, Conway at epoch 517
            let babbage_epoch: u64 = 365;
            let conway_epoch: u64 = 517;

            let babbage_slot =
                shelley_start_slot + (babbage_epoch - shelley_start_epoch) * shelley_epoch_length;
            let babbage_time_pico = byron_end_time_pico
                + (babbage_slot - shelley_start_slot) as u128
                    * shelley_slot_length_ms as u128
                    * 1_000_000_000;

            let conway_slot =
                shelley_start_slot + (conway_epoch - shelley_start_epoch) * shelley_epoch_length;
            let conway_time_pico = byron_end_time_pico
                + (conway_slot - shelley_start_slot) as u128
                    * shelley_slot_length_ms as u128
                    * 1_000_000_000;

            let shelley_era =
                |start_slot, start_epoch, start_time: u128, end: Option<EraBound>| EraSummary {
                    start_slot,
                    start_epoch,
                    start_time_pico: start_time as u64,
                    end,
                    epoch_size: shelley_epoch_length,
                    slot_length_ms: shelley_slot_length_ms,
                    safe_zone: shelley_safe_zone,
                    genesis_window: shelley_genesis_window,
                };

            let bound = |slot, epoch, time_pico: u128| EraBound {
                slot,
                epoch,
                time_pico: time_pico as u64,
            };

            let mut summaries = vec![
                // Byron
                EraSummary {
                    start_slot: 0,
                    start_epoch: 0,
                    start_time_pico: 0,
                    end: Some(bound(byron_end_slot, byron_end_epoch, byron_end_time_pico)),
                    epoch_size: byron_epoch_len,
                    slot_length_ms: byron_slot_len_ms,
                    safe_zone: byron_safe_zone,
                    genesis_window: byron_genesis_window,
                },
                // Shelley (208..365)
                shelley_era(
                    shelley_start_slot,
                    shelley_start_epoch,
                    byron_end_time_pico,
                    if current_epoch >= babbage_epoch {
                        Some(bound(babbage_slot, babbage_epoch, babbage_time_pico))
                    } else {
                        None
                    },
                ),
            ];

            if current_epoch >= babbage_epoch {
                // Babbage (365..517)
                summaries.push(shelley_era(
                    babbage_slot,
                    babbage_epoch,
                    babbage_time_pico,
                    if current_epoch >= conway_epoch {
                        Some(bound(conway_slot, conway_epoch, conway_time_pico))
                    } else {
                        None
                    },
                ));
            }
            if current_epoch >= conway_epoch {
                // Conway (517..current)
                summaries.push(shelley_era(
                    conway_slot,
                    conway_epoch,
                    conway_time_pico,
                    None,
                ));
            }

            summaries
        } else {
            // Testnets: Byron/Shelley/Allegra/Mary/Alonzo all start at epoch 0 (instant HF)
            // then Babbage and Conway at their actual transition epochs.
            //
            // The Haskell node returns era summaries matching the HFC type list.
            // For preview: Byron(0) → Shelley(0) → Allegra(0) → Mary(0) → Alonzo(0→3) →
            //              Babbage(3→646) → Conway(646→...)
            let origin = EraBound {
                slot: 0,
                epoch: 0,
                time_pico: 0,
            };

            // Build Shelley-era template (all Shelley+ eras share same params)
            let shelley_era = |start: EraBound, end: Option<EraBound>| EraSummary {
                start_slot: start.slot,
                start_epoch: start.epoch,
                start_time_pico: start.time_pico,
                end,
                epoch_size: shelley_epoch_length,
                slot_length_ms: shelley_slot_length_ms,
                safe_zone: shelley_safe_zone,
                genesis_window: shelley_genesis_window,
            };

            // Determine era transitions from ledger state
            // Preview testnet: Byron/Shelley/Allegra/Mary all at epoch 0
            // Alonzo ends at epoch 3, Babbage at epoch 646, Conway ongoing
            let current_epoch = ls.epoch.0;

            // For preview: all pre-Alonzo eras are instant (start=end=origin)
            // Alonzo starts at origin, ends at epoch 3
            let alonzo_end_epoch: u64 = if current_epoch >= 3 { 3 } else { 0 };
            let alonzo_end_slot = alonzo_end_epoch * shelley_epoch_length;
            let alonzo_end_time_pico =
                alonzo_end_slot as u128 * shelley_slot_length_ms as u128 * 1_000_000_000;

            // Babbage starts at epoch 3, ends at epoch 646
            let babbage_end_epoch: u64 = 646;
            let babbage_end_slot = babbage_end_epoch * shelley_epoch_length;
            let babbage_end_time_pico =
                babbage_end_slot as u128 * shelley_slot_length_ms as u128 * 1_000_000_000;

            let mut summaries = vec![
                // Byron (instant transition at epoch 0)
                EraSummary {
                    start_slot: 0,
                    start_epoch: 0,
                    start_time_pico: 0,
                    end: Some(origin.clone()),
                    epoch_size: byron_epoch_len,
                    slot_length_ms: byron_slot_len_ms,
                    safe_zone: byron_safe_zone,
                    genesis_window: byron_genesis_window,
                },
                // Shelley (instant at epoch 0)
                shelley_era(origin.clone(), Some(origin.clone())),
                // Allegra (instant at epoch 0)
                shelley_era(origin.clone(), Some(origin.clone())),
                // Mary (instant at epoch 0)
                shelley_era(origin.clone(), Some(origin.clone())),
            ];

            if current_epoch < alonzo_end_epoch {
                // Still in Alonzo or earlier — unbounded
                summaries.push(shelley_era(origin, None));
            } else {
                let alonzo_end = EraBound {
                    slot: alonzo_end_slot,
                    epoch: alonzo_end_epoch,
                    time_pico: alonzo_end_time_pico as u64,
                };
                // Alonzo (epoch 0..3)
                summaries.push(shelley_era(origin, Some(alonzo_end.clone())));

                if current_epoch < babbage_end_epoch {
                    // Babbage (epoch 3..unbounded)
                    summaries.push(shelley_era(alonzo_end, None));
                } else {
                    let babbage_end = EraBound {
                        slot: babbage_end_slot,
                        epoch: babbage_end_epoch,
                        time_pico: babbage_end_time_pico as u64,
                    };
                    // Babbage (epoch 3..646)
                    summaries.push(shelley_era(alonzo_end, Some(babbage_end.clone())));
                    // Conway (epoch 646..unbounded)
                    summaries.push(shelley_era(babbage_end, None));
                }
            }

            summaries
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
                                        if let Err(e) = ls.apply_block(&block, BlockValidationMode::ApplyOnly) {
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
                    debug!(
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
        self.metrics
            .leader_checks_total
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let is_leader = crate::forge::check_slot_leadership(
            creds,
            next_slot,
            &epoch_nonce,
            relative_stake,
            self.consensus.active_slot_coeff,
        );

        if !is_leader {
            self.metrics
                .leader_checks_not_elected
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            // Log periodically so operators can confirm the VRF check is running
            if next_slot.0 % 20 == 0 {
                info!(
                    slot = next_slot.0,
                    pool_id = %creds.pool_id,
                    stake = format_args!("{relative_stake:.6}"),
                    "Slot leader check: not elected"
                );
            }
            return;
        }

        info!(
            slot = next_slot.0,
            pool_id = %creds.pool_id,
            stake = format_args!("{relative_stake:.6}"),
            "Slot leader check: ELECTED — forging block",
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
                    if let Err(e) = ls.apply_block(&block, BlockValidationMode::ApplyOnly) {
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
                    block = block_number.0,
                    slot = next_slot.0,
                    txs = block.transactions.len(),
                    "Block forged",
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
                    self.metrics
                        .blocks_announced
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
            Err(e) => {
                self.metrics
                    .forge_failures
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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

    /// Regression test: BlockProvider methods must not panic when called
    /// from within a tokio async runtime. Previously, bare `blocking_read()`
    /// would panic with "Cannot block the current thread from within a runtime".
    /// The fix wraps them in `tokio::task::block_in_place`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_block_provider_works_inside_async_runtime() {
        use torsten_network::n2n_server::BlockProvider;
        use torsten_storage::ChainDB;

        let tmp = tempfile::tempdir().unwrap();
        let db = ChainDB::open(tmp.path()).unwrap();
        let provider = ChainDBBlockProvider {
            chain_db: Arc::new(RwLock::new(db)),
        };

        // These would panic before the block_in_place fix
        let tip = provider.get_tip();
        assert_eq!(tip.block_number, 0);

        let result = provider.get_block(&[0u8; 32]);
        assert!(result.is_none());

        let result = provider.has_block(&[0u8; 32]);
        assert!(!result);

        let result = provider.get_next_block_after_slot(0);
        assert!(result.is_none());
    }

    /// Regression test: tokio RwLock blocking_read inside block_in_place
    /// must not panic in a multi-threaded async runtime. This covers the
    /// pattern used by both LedgerUtxoProvider and ChainDBBlockProvider.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_blocking_read_via_block_in_place_does_not_panic() {
        let lock = Arc::new(RwLock::new(42u64));
        let value = tokio::task::block_in_place(|| {
            let guard = lock.blocking_read();
            *guard
        });
        assert_eq!(value, 42);
    }
}
