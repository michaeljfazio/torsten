//! Main Torsten node: struct definition, initialization, and run loop orchestration.
//!
//! This module owns the `Node` struct and the top-level lifecycle methods (`new`,
//! `run`).  All subsystem logic is delegated to focused sub-modules:
//!
//! - [`epoch`]  — Snapshot policy, ledger snapshot save/prune/restore
//! - [`serve`]  — N2N/N2C server adapters (BlockProvider, TxValidator, metrics bridges)
//! - [`query`]  — N2C LocalStateQuery response building (`update_query_state`)
//! - [`sync`]   — Pipelined ChainSync loop, block processing, rollback, replay

#[allow(dead_code)] // networking rewrite module, wired in soon
pub(crate) mod block_fetch_logic;
#[allow(dead_code)] // networking rewrite module, wired in soon
pub(crate) mod connection_lifecycle;
pub(crate) mod epoch;
pub(crate) mod n2c_query;
pub(crate) mod networking;
#[allow(dead_code)] // networking rewrite module, wired in soon
pub(crate) mod peer_connection;
pub(crate) mod query;
pub(crate) mod serve;
pub(crate) mod sync;

use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::signal;
use tokio::sync::{mpsc, watch, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::node::block_fetch_logic::BlockFetchLogicTask;
use crate::node::connection_lifecycle::{
    CandidateChainState, ConnectResult, ConnectionLifecycleManager, FetchedBlock, LifecycleError,
};

use torsten_consensus::chain_fragment::ChainFragment;
use torsten_consensus::OuroborosPraos;
use torsten_ledger::{BlockValidationMode, LedgerState};
use torsten_mempool::{Mempool, MempoolConfig};
use torsten_network::{Governor, GovernorConfig, PeerTargets};

use crate::node::n2c_query::QueryHandler;
use crate::node::networking::{
    DiffusionMode, NodePeerManager, PeerManagerConfig, RollbackAnnouncement,
};
use torsten_primitives::block::Point;
use torsten_primitives::protocol_params::ProtocolParameters;
use torsten_storage::background::{CopyToImmutable, GcScheduler, SnapshotScheduler};
use torsten_storage::{ChainDB, ChainSelHandle};

use crate::config::NodeConfig;
use crate::genesis::{AlonzoGenesis, ByronGenesis, ConwayGenesis, ShelleyGenesis};
use crate::topology::Topology;

// ─── NodeArgs ────────────────────────────────────────────────────────────────

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
    /// Path to cold signing key (accepted for cardano-node compatibility)
    pub _shelley_cold_key: Option<PathBuf>,
    /// Prometheus metrics port (0 to disable)
    pub metrics_port: u16,
    /// Emit `cardano_node_metrics_*` compatibility aliases alongside native metrics
    pub compat_metrics: bool,
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

// ─── Node struct ─────────────────────────────────────────────────────────────

pub struct Node {
    pub(crate) config: NodeConfig,
    pub(crate) topology: Topology,
    pub(crate) chain_db: Arc<RwLock<ChainDB>>,
    pub(crate) ledger_state: Arc<RwLock<LedgerState>>,
    pub(crate) consensus: OuroborosPraos,
    pub(crate) mempool: Arc<Mempool>,
    /// Connection lifecycle manager — one TCP connection per peer,
    /// temperature-based protocol activation matching Haskell PeerStateActions.
    /// Created in `new()`, used in `run()` for Governor action dispatch.
    connection_lifecycle: Option<ConnectionLifecycleManager>,
    /// Handle to the BlockFetch decision task (independent tokio task).
    /// Runs the decision loop that assigns fetch ranges to per-peer workers.
    block_fetch_task: Option<JoinHandle<()>>,
    /// Receiver for blocks fetched by per-peer BlockFetch workers.
    /// The main run loop consumes these and applies them to the ledger.
    fetched_blocks_rx: Option<mpsc::Receiver<FetchedBlock>>,
    pub(crate) query_handler: Arc<RwLock<QueryHandler>>,
    pub(crate) peer_manager: Arc<RwLock<NodePeerManager>>,
    pub(crate) socket_path: PathBuf,
    pub(crate) database_path: PathBuf,
    pub(crate) listen_addr: std::net::SocketAddr,
    pub(crate) network_magic: u64,
    /// Byron epoch length in absolute slots (10 * k). For correct slot
    /// computation on non-mainnet networks.
    pub(crate) byron_epoch_length: u64,
    pub(crate) shelley_genesis: Option<ShelleyGenesis>,
    /// HFC era history state machine — tracks era boundaries with slot/epoch/time
    /// arithmetic. Initialized from genesis configs and extended during sync as
    /// era transitions are detected in the block stream.
    pub(crate) era_history: Arc<RwLock<torsten_consensus::EraHistory>>,
    pub(crate) topology_path: PathBuf,
    pub(crate) metrics: Arc<crate::metrics::NodeMetrics>,
    /// Block producer credentials (None = relay-only mode)
    pub(crate) block_producer: Option<crate::forge::BlockProducerCredentials>,
    /// Broadcast sender for announcing forged blocks to connected peers
    pub(crate) block_announcement_tx:
        Option<tokio::sync::broadcast::Sender<torsten_network::BlockAnnouncement>>,
    /// Broadcast sender for notifying connected peers of chain rollbacks
    pub(crate) rollback_announcement_tx:
        Option<tokio::sync::broadcast::Sender<RollbackAnnouncement>>,
    /// Prometheus metrics port
    pub(crate) metrics_port: u16,
    /// Expected Blake2b-256 hash of the Byron genesis block (from config or computed from file)
    pub(crate) expected_byron_genesis_hash: Option<torsten_primitives::hash::Hash32>,
    /// Expected Blake2b-256 hash of the Shelley genesis block (from config or computed from file)
    pub(crate) expected_shelley_genesis_hash: Option<torsten_primitives::hash::Hash32>,
    /// Whether genesis block validation has been performed (only need to validate once)
    pub(crate) genesis_validated: bool,
    /// Count of epoch transitions observed since node startup.
    /// Used to determine when the epoch nonce is reliable for VRF verification.
    /// After Mithril import, we need at least 2 epoch transitions for the
    /// rolling nonce to be correctly accumulated.
    pub(crate) epoch_transitions_observed: u32,
    /// Live (post-replay) epoch transitions — only incremented during the sync
    /// loop, not during chunk replay.  Used for `snapshots_established` since
    /// replay-built snapshots may have approximate stake values.
    pub(crate) live_epoch_transitions: u32,
    /// Snapshot policy controlling when ledger snapshots are taken.
    pub(crate) snapshot_policy: epoch::SnapshotPolicy,
    /// Consensus mode: "praos" (default) or "genesis"
    pub(crate) consensus_mode: String,
    /// Force full Phase-2 Plutus validation on all blocks
    pub(crate) validate_all_blocks: bool,
    /// Watch receiver for current disk space level, updated by disk monitor
    pub(crate) disk_space_rx: watch::Receiver<crate::disk_monitor::DiskSpaceLevel>,
    /// Genesis State Machine — drives LoE enforcement and GDD.
    ///
    /// When genesis mode is disabled the GSM immediately enters CaughtUp
    /// and `loe_limit()` always returns `None`, so there is no overhead on
    /// the normal (Praos) sync path.  Stored as an `Arc<RwLock<…>>` so that
    /// the background GSM evaluation task can write state transitions while
    /// the sync pipeline holds a read lock to query `loe_limit()`.
    pub(crate) gsm: Arc<RwLock<crate::gsm::GenesisStateMachine>>,
    /// Anchored chain fragment representing the volatile portion of the
    /// selected chain (the last k block headers not yet in ImmutableDB).
    ///
    /// Matches Haskell's `AnchoredFragment` — anchored at the immutable tip,
    /// headers grow as new blocks are adopted.  Used for:
    /// - ChainSync server: `find_intersect` for downstream peers
    /// - Background copy-to-immutable: fragment length > k triggers copy
    /// - Chain selection: comparing candidate chains against the current chain
    ///
    /// Protected by `RwLock` so both the sync loop (write) and N2N server
    /// tasks (read, for intersection finding) can access it concurrently.
    pub(crate) chain_fragment: Arc<RwLock<ChainFragment>>,
    /// Chain-selection queue handle.
    ///
    /// All blocks (from peers and from the local forger) are submitted
    /// through this handle.  The background `add_block_runner` task owns
    /// the receiving end and writes blocks to VolatileDB sequentially,
    /// avoiding concurrency hazards between storage writes and chain
    /// selection.  This is Torsten's implementation of Haskell's
    /// `addBlockAsync` / `addBlockRunner` pattern.
    ///
    /// `None` only during the constructor before the runner task is spawned.
    pub(crate) chain_sel_handle: Option<ChainSelHandle>,

    // ── Phase 5: Background maintenance operations ────────────────────────
    //
    // These match Haskell's Background.hs: copy-to-immutable, GC, and
    // snapshot scheduling.  All three are synchronous value types — they
    // are called from the main processing loop after each block is applied
    // (or periodically from a dedicated tick).  Wrapping them in a Mutex
    // allows the sync loop (`&mut self`) and future ticker tasks (`Arc`) to
    // share them if needed, but for now only the sync loop touches them.
    /// Copies the oldest volatile block to ImmutableDB when the fragment
    /// grows beyond the security parameter k.
    ///
    /// Matches Haskell's `copyToImmutableDB` in Background.hs.
    pub(crate) copy_to_immutable: CopyToImmutable,

    /// Deferred GC for VolatileDB entries after copy-to-immutable.
    ///
    /// Entries are scheduled with a 60-second delay (matching Haskell's
    /// `gcDelay`) and removed on the next `run_pending` call after expiry.
    /// Matches Haskell's `garbageCollectBlocks` / `GcSchedule`.
    pub(crate) gc_scheduler: GcScheduler,

    /// Decides when to save LedgerSeq anchor snapshots to disk.
    ///
    /// Triggers at epoch boundaries, every N blocks, and on graceful
    /// shutdown.  Matches Haskell's snapshot policy in Background.hs.
    pub(crate) bg_snapshot_scheduler: SnapshotScheduler,
}

// ─── Node impl: new() ────────────────────────────────────────────────────────

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

        // Load Alonzo genesis if configured (with hash validation)
        let mut alonzo_genesis_file_hash: Option<torsten_primitives::hash::Hash32> = None;
        if let Some(ref genesis_path) = args.config.alonzo_genesis_file {
            let genesis_path = config_dir.join(genesis_path);
            match AlonzoGenesis::load_with_hash(&genesis_path) {
                Ok((genesis, hash)) => {
                    info!(
                        max_val_size = genesis.max_value_size,
                        collateral_pct = genesis.collateral_percentage,
                        "Alonzo genesis loaded",
                    );
                    alonzo_genesis_file_hash = Some(hash);
                    genesis.apply_to_protocol_params(&mut protocol_params);
                }
                Err(e) => {
                    warn!("Failed to load Alonzo genesis: {e}");
                }
            }
        }

        // Validate Alonzo genesis hash if configured
        if let Some(ref expected_hex) = args.config.alonzo_genesis_hash {
            if let Ok(expected) = torsten_primitives::hash::Hash32::from_hex(expected_hex) {
                if let Some(ref actual) = alonzo_genesis_file_hash {
                    if *actual != expected {
                        anyhow::bail!(
                            "Alonzo genesis hash mismatch: expected {}, got {}",
                            expected.to_hex(),
                            actual.to_hex()
                        );
                    }
                    debug!("Alonzo genesis hash validated: {}", expected.to_hex());
                }
            }
        }

        // Load Conway genesis if configured (with hash validation)
        let mut conway_committee_threshold: Option<(u64, u64)> = None;
        let mut conway_committee_members: Vec<([u8; 32], u64)> = Vec::new();
        let mut conway_genesis_file_hash: Option<torsten_primitives::hash::Hash32> = None;
        if let Some(ref genesis_path) = args.config.conway_genesis_file {
            let genesis_path = config_dir.join(genesis_path);
            match ConwayGenesis::load_with_hash(&genesis_path) {
                Ok((genesis, hash)) => {
                    info!(
                        drep_deposit = genesis.d_rep_deposit,
                        gov_deposit = genesis.gov_action_deposit,
                        committee_min = genesis.committee_min_size,
                        "Conway genesis loaded",
                    );
                    conway_genesis_file_hash = Some(hash);
                    conway_committee_threshold = genesis.committee_threshold();
                    conway_committee_members = genesis.committee_members();
                    genesis.apply_to_protocol_params(&mut protocol_params);
                }
                Err(e) => {
                    warn!("Failed to load Conway genesis: {e}");
                }
            }
        }

        // Validate Conway genesis hash if configured
        if let Some(ref expected_hex) = args.config.conway_genesis_hash {
            if let Ok(expected) = torsten_primitives::hash::Hash32::from_hex(expected_hex) {
                if let Some(ref actual) = conway_genesis_file_hash {
                    if *actual != expected {
                        anyhow::bail!(
                            "Conway genesis hash mismatch: expected {}, got {}",
                            expected.to_hex(),
                            actual.to_hex()
                        );
                    }
                    debug!("Conway genesis hash validated: {}", expected.to_hex());
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
                    let ste = epoch::shelley_transition_epoch_for_magic(network_magic);
                    state.set_shelley_transition(ste, byron_epoch_length);
                    if let Some(hash) = shelley_genesis_hash {
                        state.genesis_hash = hash;
                    }

                    // Recalculate the epoch from the tip slot using the now-correct
                    // genesis parameters.  Snapshots saved with wrong epoch_length
                    // (e.g. mainnet default 432000 instead of preview 86400) have
                    // incorrect epoch numbers baked in.  Without this correction,
                    // apply_block would try to process hundreds of spurious epoch
                    // transitions (445 → 1239) and the stake snapshots would be at
                    // wrong epochs, causing pool_stake=0 for block producers.
                    if state.tip.point != Point::Origin {
                        let tip_slot = state.tip.point.slot().map(|s| s.0).unwrap_or(0);
                        let correct_epoch = state.epoch_of_slot(tip_slot);
                        if correct_epoch != state.epoch.0 {
                            warn!(
                                snapshot_epoch = state.epoch.0,
                                correct_epoch,
                                tip_slot,
                                "Snapshot epoch differs from computed epoch — correcting"
                            );
                            state.epoch = torsten_primitives::time::EpochNo(correct_epoch);
                        }
                    }

                    // Detect stale snapshots whose protocol_params were captured before
                    // Alonzo/Conway genesis files were applied. The canonical signal is
                    // max_tx_ex_units.mem == 14_000_000, which is the hardcoded
                    // mainnet_defaults() value. No live Cardano network (mainnet, preview,
                    // preprod) has ever used 14_000_000 as a settled governance value:
                    //   - Preview/Preprod Alonzo genesis: 10_000_000
                    //   - Mainnet Alonzo genesis: 14_000_000 (initial, but mainnet has
                    //     since been updated to 16_500_000 via governance action)
                    //
                    // For all testnets this value unambiguously indicates a broken baseline.
                    // For mainnet, governance may have updated it past 14_000_000 — but
                    // the snapshot would already carry the post-governance value in that case.
                    // If it still says 14_000_000 on a mainnet snapshot, that means it was
                    // captured before governance ran, and will be corrected by replay anyway.
                    //
                    // Additionally check committee_min_size: a value of 7 is the mainnet
                    // default but never the correct value for preview (genesis says 0) or
                    // preprod. If the snapshot was taken with wrong committee_min_size,
                    // ALL governance actions requiring CC approval would fail to ratify.
                    let genesis_mem = protocol_params.max_tx_ex_units.mem;
                    let snapshot_mem = state.protocol_params.max_tx_ex_units.mem;
                    let defaults_mem =
                        torsten_primitives::protocol_params::ProtocolParameters::mainnet_defaults()
                            .max_tx_ex_units
                            .mem;
                    let snapshot_appears_stale = snapshot_mem == defaults_mem
                        && genesis_mem != defaults_mem
                        && genesis_mem > 0;

                    if snapshot_appears_stale {
                        // The snapshot's protocol_params may have stale defaults
                        // from genesis initialization. Rather than discarding the
                        // entire snapshot (forcing a multi-minute full replay),
                        // overlay the genesis protocol params on top. The correct
                        // on-chain values will be restored when blocks with
                        // governance parameter updates are replayed.
                        warn!(
                            snapshot_max_tx_ex_mem = snapshot_mem,
                            genesis_max_tx_ex_mem = genesis_mem,
                            "Snapshot protocol_params have stale defaults — \
                             applying genesis params overlay",
                        );
                        state.protocol_params = protocol_params.clone();
                    }
                    {
                        // Validate snapshot tip canonicality.
                        //
                        // A snapshot whose tip is *within the ImmutableDB slot range*
                        // must match the canonical hash at that slot.  If it does not,
                        // the snapshot was saved on a fork chain and must be discarded.
                        //
                        // Root cause context: Torsten snapshots at the volatile ledger
                        // tip, which can be a fork block.  Haskell only snapshots at
                        // the ImmutableDB-confirmed anchor, so fork snapshots cannot
                        // occur there.  This check aligns our behaviour with Haskell.
                        //
                        // If the snapshot tip is *ahead* of the ImmutableDB tip (in
                        // the volatile region), we cannot verify canonicality yet —
                        // accept it and let the normal startup path handle divergence.
                        let snapshot_valid = match state.tip.point {
                            Point::Origin => true,
                            Point::Specific(snapshot_slot, ref hash) => {
                                match chain_db.try_read() {
                                    Ok(db) => {
                                        let exists = db.has_block(hash);
                                        if exists {
                                            // Hash found in ChainDB (volatile or ImmutableDB) — canonical.
                                            true
                                        } else {
                                            let imm_tip = db.get_immutable_tip();
                                            let imm_tip_slot =
                                                imm_tip.point.slot().map(|s| s.0).unwrap_or(0);
                                            let db_tip = db.get_tip();
                                            let db_tip_slot =
                                                db_tip.point.slot().map(|s| s.0).unwrap_or(0);

                                            if snapshot_slot.0 > db_tip_slot {
                                                // Snapshot is genuinely ahead of all storage —
                                                // crash before ChainDB persist.
                                                warn!(
                                                    "Ledger snapshot is ahead of ChainDB \
                                                     (snapshot={}, chaindb={}); node may have \
                                                     crashed before ChainDB persist — discarding \
                                                     snapshot, will replay from storage",
                                                    state.tip, db_tip,
                                                );
                                                false
                                            } else if snapshot_slot.0 <= imm_tip_slot {
                                                // Snapshot slot is within the finalized ImmutableDB
                                                // range, but the hash is not in ChainDB.  This means
                                                // the snapshot tip is on a fork chain that was never
                                                // written to the ImmutableDB canonical chain.
                                                //
                                                // Verify by checking what hash the ImmutableDB
                                                // actually has at this slot.  If it differs, the
                                                // snapshot is a fork snapshot and must be rejected
                                                // to prevent replay with a corrupted base state.
                                                let canonical_at_slot = db
                                                    .get_immutable_tip_point()
                                                    .and_then(|p| match p {
                                                        Point::Specific(s, h)
                                                            if s.0 == snapshot_slot.0 =>
                                                        {
                                                            Some(h)
                                                        }
                                                        _ => None,
                                                    });
                                                let is_fork = match canonical_at_slot {
                                                    Some(canonical_hash) => canonical_hash != *hash,
                                                    // Can't verify exact slot — use block-at-or-after
                                                    None => {
                                                        match db.get_block_at_or_after_slot(
                                                            snapshot_slot,
                                                        ) {
                                                            Ok(Some((
                                                                found_slot,
                                                                found_hash,
                                                                _,
                                                            ))) if found_slot.0
                                                                == snapshot_slot.0 =>
                                                            {
                                                                found_hash != *hash
                                                            }
                                                            // No block at that slot (empty slot or
                                                            // slot beyond what can be verified) —
                                                            // can't confirm fork, accept with warning.
                                                            _ => false,
                                                        }
                                                    }
                                                };

                                                if is_fork {
                                                    warn!(
                                                        snapshot_slot = snapshot_slot.0,
                                                        imm_tip_slot,
                                                        "Ledger snapshot tip is on a fork (hash not \
                                                         in canonical ImmutableDB chain at slot {}). \
                                                         Discarding fork snapshot to prevent UTxO \
                                                         corruption — will replay from genesis.",
                                                        snapshot_slot.0,
                                                    );
                                                    false
                                                } else {
                                                    // Hash mismatch but can't confirm fork — could be
                                                    // hash computation difference (pallas vs cardano-node).
                                                    // Accept and let the chunk replay handle the gap.
                                                    warn!(
                                                        "Ledger snapshot hash not found in ChainDB \
                                                         but slot {} <= ImmutableDB tip {} — \
                                                         accepting snapshot (hash mismatch may be \
                                                         due to hash computation difference)",
                                                        snapshot_slot.0, imm_tip_slot,
                                                    );
                                                    true
                                                }
                                            } else {
                                                // snapshot_slot is in the volatile range (between
                                                // imm_tip and db_tip).  Hash is not in VolatileDB
                                                // (WAL may have been empty on restart).  Accept —
                                                // the chunk replay will catch up to the correct point.
                                                debug!(
                                                    snapshot_slot = snapshot_slot.0,
                                                    imm_tip_slot,
                                                    db_tip_slot,
                                                    "Snapshot tip in volatile range, hash not in WAL \
                                                     — accepting (will replay from chunk files)"
                                                );
                                                true
                                            }
                                        }
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
                    } // end else (snapshot not stale)
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
                    // attach_utxo_store calls rebuild_address_index() which populates
                    // the in-memory count from the LSM. Read the count AFTER attach
                    // to avoid a false-zero from the freshly-opened store (count starts
                    // at 0 and is only set correctly once rebuild_address_index runs).
                    ledger.attach_utxo_store(store);
                    let store_count = ledger.utxo_set.len();

                    // If the ledger has a non-origin tip but the UTxO store has
                    // significantly fewer entries than expected, the store data
                    // was lost or incomplete (crash, session lock, etc.).
                    // Force a full re-replay by resetting the ledger tip to origin.
                    let ledger_slot = ledger.tip.point.slot().map(|s| s.0).unwrap_or(0);
                    if ledger_slot > 0 {
                        // A synced preview testnet has ~3M UTxOs, mainnet ~15M.
                        // If the store has less than 100K entries for a non-trivial
                        // ledger tip, the data is almost certainly incomplete.
                        let min_expected = if ledger_slot > 10_000_000 { 100_000 } else { 0 };
                        if store_count < min_expected {
                            warn!(
                                utxo_count = store_count,
                                ledger_slot,
                                min_expected,
                                "UTxO store appears incomplete ({} entries for slot {}). \
                                 Resetting ledger to force full re-replay.",
                                store_count,
                                ledger_slot
                            );
                            ledger.reset_to_origin();
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to open UTxO store at {}: {e}, continuing with in-memory UTxOs",
                        utxo_path.display()
                    );
                }
            }
        }

        // After attaching the UTxO store, rebuild stake distribution and snapshot
        // pool stakes so that block production has correct data immediately on restart.
        // Without this, a block producer restored from snapshot may see pool_stake=0
        // if the saved snapshot's incremental stake tracking was stale.
        if ledger.tip.point != Point::Origin && !ledger.utxo_set.is_empty() {
            info!("Rebuilding stake distribution from UTxO store for snapshot consistency");
            ledger.rebuild_stake_distribution();
            // Log delegation/pool_params stats before recompute for diagnostics
            debug!(
                main_delegations = ledger.delegations.len(),
                pool_params = ledger.pool_params.len(),
                stake_credentials = ledger.stake_distribution.stake_map.len(),
                reward_accounts = ledger.reward_accounts.len(),
                mark_delegations = ledger
                    .snapshots
                    .mark
                    .as_ref()
                    .map(|s| s.delegations.len())
                    .unwrap_or(0),
                set_delegations = ledger
                    .snapshots
                    .set
                    .as_ref()
                    .map(|s| s.delegations.len())
                    .unwrap_or(0),
                go_delegations = ledger
                    .snapshots
                    .go
                    .as_ref()
                    .map(|s| s.delegations.len())
                    .unwrap_or(0),
                "Snapshot state before recompute",
            );
            ledger.recompute_snapshot_pool_stakes();
        }

        // Capture the ledger epoch before the ledger is moved into the Arc.
        // A snapshot saved at epoch N means N epoch transitions were processed
        // before the snapshot was taken, so the epoch nonce in the snapshot is
        // already reliable. We prime epoch_transitions_observed from this value
        // so that enable_strict_verification() can set nonce_established=true
        // immediately at tip — no live epoch boundary is required after restore.
        //
        // This fixes the bug where a node restored from a snapshot and catching
        // up to tip without crossing an epoch boundary would leave nonce_established
        // false, disabling strict VRF verification and blocking block forging.
        //
        // The epoch number is the correct proxy because:
        //   - Every epoch transition runs process_epoch_transition() which
        //     accumulates VRF nonce contributions before the snapshot is saved.
        //   - A snapshot at epoch 0 (fresh start) correctly gives 0 here,
        //     meaning the node must observe a live boundary before forging.
        let snapshot_epoch_transitions: u32 = ledger.epoch.0 as u32;

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
        // Capture security_param before consensus is moved into the Node struct.
        let consensus_security_param = consensus.security_param;
        info!(
            epoch_len = consensus.epoch_length.0,
            k = consensus.security_param,
            f = consensus.active_slot_coeff,
            kes_period = consensus.slots_per_kes_period,
            max_kes = consensus.max_kes_evolutions,
            "Consensus: Praos",
        );

        // Build the HFC era history state machine from genesis parameters.
        // This replaces the hardcoded era lookup tables with a proper state machine
        // that tracks era boundaries and provides slot↔time conversions.
        let era_history = {
            use torsten_consensus::era_history::{EraHistory, EraParams};

            let k = consensus.security_param;
            let active_slots_coeff = consensus.active_slot_coeff;
            let genesis_window = k * 2;

            let byron_params = EraParams {
                epoch_size: if byron_epoch_length > 0 {
                    byron_epoch_length
                } else {
                    // Fallback: mainnet default (10 * 2160)
                    21600
                },
                slot_length_ms: byron_slot_duration_ms,
                safe_zone: k * 2,
            };

            let shelley_epoch_length = shelley_genesis
                .as_ref()
                .map(|g| g.epoch_length)
                .unwrap_or(432000);
            let shelley_slot_length_ms = shelley_genesis
                .as_ref()
                .map(|g| g.slot_length * 1000)
                .unwrap_or(1000);
            let shelley_safe_zone = (3.0 * k as f64 / active_slots_coeff).floor() as u64;

            let shelley_params = EraParams {
                epoch_size: shelley_epoch_length,
                slot_length_ms: shelley_slot_length_ms,
                safe_zone: shelley_safe_zone,
            };

            let shelley_transition_epoch = epoch::shelley_transition_epoch_for_magic(network_magic);

            let mut eh = EraHistory::from_genesis(
                byron_params,
                shelley_params,
                shelley_transition_epoch,
                genesis_window,
            );

            // If we loaded a ledger snapshot, reconstruct past era transitions
            // so the era history covers all eras up to the current ledger era.
            // This uses the same hardcoded era boundaries as the previous
            // build_era_summaries() for known networks — only needed once on
            // first startup after the EraHistory feature is introduced.
            {
                let ls = ledger_state.blocking_read();
                let current_era = ls.era;
                let is_mainnet = network_magic == 764824073;

                if is_mainnet {
                    // Mainnet era transitions at known epochs.
                    // Shelley→Babbage: epoch 365, Babbage→Conway: epoch 517.
                    // (Shelley covers Shelley/Allegra/Mary/Alonzo per Haskell HFC type list)
                    if current_era >= torsten_primitives::era::Era::Babbage {
                        eh.record_era_transition(torsten_primitives::era::Era::Babbage, 365);
                    }
                    if current_era >= torsten_primitives::era::Era::Conway {
                        eh.record_era_transition(torsten_primitives::era::Era::Conway, 517);
                    }
                } else {
                    // Testnets: Byron/Shelley/Allegra/Mary at epoch 0 (instant HF),
                    // then Alonzo→Babbage, Babbage→Conway at testnet-specific epochs.
                    // For preview: Alonzo 0→3, Babbage 3→646, Conway 646+.
                    let eras_and_epochs: &[(torsten_primitives::era::Era, u64)] =
                        match network_magic {
                            2 => &[
                                (torsten_primitives::era::Era::Allegra, 0),
                                (torsten_primitives::era::Era::Mary, 0),
                                (torsten_primitives::era::Era::Alonzo, 0),
                                (torsten_primitives::era::Era::Babbage, 3),
                                (torsten_primitives::era::Era::Conway, 646),
                            ],
                            1 => &[
                                // Preprod
                                (torsten_primitives::era::Era::Allegra, 0),
                                (torsten_primitives::era::Era::Mary, 0),
                                (torsten_primitives::era::Era::Alonzo, 0),
                                (torsten_primitives::era::Era::Babbage, 4),
                                (torsten_primitives::era::Era::Conway, 186),
                            ],
                            _ => &[
                                // Generic testnet: assume instant transitions
                                (torsten_primitives::era::Era::Allegra, 0),
                                (torsten_primitives::era::Era::Mary, 0),
                                (torsten_primitives::era::Era::Alonzo, 0),
                                (torsten_primitives::era::Era::Babbage, 0),
                                (torsten_primitives::era::Era::Conway, 0),
                            ],
                        };
                    for &(era, epoch) in eras_and_epochs {
                        if current_era >= era && eh.current_era() < era {
                            eh.record_era_transition(era, epoch);
                        }
                    }
                }
            }

            info!(
                eras = eh.len(),
                current = %eh.current_era(),
                "HFC era history initialized",
            );

            Arc::new(RwLock::new(eh))
        };

        let mempool = Arc::new(Mempool::new(MempoolConfig {
            max_transactions: args.mempool_max_tx,
            max_bytes: args.mempool_max_bytes,
            ..MempoolConfig::default()
        }));

        let socket_path = args.socket_path.clone();
        let listen_addr: std::net::SocketAddr =
            format!("{}:{}", args.host_addr, args.port).parse()?;
        // network_magic computed earlier (before ledger snapshot loading).
        // Server tasks are spawned in run() and live for the node's lifetime.

        // Wire up live UTxO provider before wrapping in lock
        let mut qh = QueryHandler::new();
        qh.set_utxo_provider(Arc::new(serve::LedgerUtxoProvider {
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
            let vrf_path = args.shelley_vrf_key.as_ref().ok_or_else(|| {
                anyhow::anyhow!("VRF signing key path required for block production")
            })?;
            let kes_path = args.shelley_kes_key.as_ref().ok_or_else(|| {
                anyhow::anyhow!("KES signing key path required for block production")
            })?;
            let opcert_path = args
                .shelley_operational_certificate
                .as_ref()
                .ok_or_else(|| {
                    anyhow::anyhow!("Operational certificate path required for block production")
                })?;
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

        // Build the GSM here so it is owned by the Node struct. This lets
        // `process_forward_blocks` query `loe_limit()` without needing to
        // pass the Arc through every call site.  When genesis mode is off the
        // GSM starts in CaughtUp and `loe_limit()` always returns None.
        let genesis_enabled = args.consensus_mode == "genesis";
        let gsm_config = crate::gsm::GsmConfig {
            marker_path: args.database_path.join("caught_up.marker"),
            ..Default::default()
        };
        let gsm = Arc::new(RwLock::new(crate::gsm::GenesisStateMachine::new(
            gsm_config,
            genesis_enabled,
        )));

        // Build and configure metrics before assembling the node struct so we
        // can set the network magic immediately (the TUI reads it on first scrape).
        let node_metrics = {
            let m = crate::metrics::NodeMetrics::new();
            m.set_network_magic(network_magic);
            m.set_compat_metrics(args.compat_metrics);
            // Advertise block producer mode so the TUI shows the correct role and
            // displays the abbreviated pool ID in the Node panel.
            let is_bp = block_producer.is_some();
            if let Some(ref creds) = block_producer {
                m.set_block_producer(&creds.pool_id.to_hex());
            }
            // Advertise P2P configuration so the TUI can display the real state
            // rather than guessing from peer counts.
            let effective_peer_sharing = args.config.effective_peer_sharing(is_bp);
            m.set_p2p_config(
                args.config.enable_p2_p,
                &args.config.diffusion_mode,
                effective_peer_sharing,
            );
            Arc::new(m)
        };

        // Log P2P configuration at startup for diagnostics.
        info!(
            p2p_enabled = args.config.enable_p2_p,
            diffusion_mode = %args.config.diffusion_mode,
            peer_sharing = args.config.effective_peer_sharing(block_producer.is_some()),
            "P2P networking configuration"
        );
        if !args.config.enable_p2_p {
            warn!(
                "P2P is disabled — running in static topology mode \
                 (no peer governor, no churn, no ledger-based discovery)"
            );
        }

        // ── Phase 1: Initialize ChainFragment from ImmutableDB tip ──────────
        //
        // On startup, the chain fragment represents the volatile window of the
        // selected chain.  We anchor it at the current ImmutableDB tip and
        // populate it with any volatile block headers that form a chain from
        // that tip.  This mirrors Haskell's `openDBInternal` startup step 5.
        //
        // For a fresh node (Origin), the fragment is empty with Origin as anchor.
        // For a node restarted after syncing, we seed the fragment from the
        // VolatileDB (via ChainDB) so the chain selection has correct context.
        //
        // Use `try_read()` to avoid blocking in the async runtime.
        // At this point in startup, no other tasks hold the lock.
        let chain_fragment = {
            let db = chain_db
                .try_read()
                .expect("ChainDB lock available during startup");
            let immutable_tip = db.get_immutable_tip();
            let anchor = match &immutable_tip.point {
                Point::Origin => Point::Origin,
                Point::Specific(slot, hash) => Point::Specific(*slot, *hash),
            };

            // Collect volatile block headers to seed the fragment.
            // We use the ChainDB volatile chain (selected_chain) which is already
            // ordered from anchor to tip.  Convert to BlockHeader stubs — we only
            // need slot + hash for the fragment invariant; full headers are available
            // in VolatileDB if needed.
            let volatile_headers = db.get_volatile_chain_headers();

            ChainFragment::from_headers(anchor, volatile_headers)
        };

        // ── Phase 1: Initialize ChainSelHandle ──────────────────────────────
        //
        // Create the chain-selection queue.  The runner future is NOT yet
        // spawned here — `Node::new()` is sync, so we store it and spawn in
        // `run()` instead.  The handle is stored so the sync loop and forge
        // path can submit blocks without holding any other locks.
        let (chain_sel_handle, chain_sel_runner) = ChainSelHandle::new(Arc::clone(&chain_db));
        // Spawn the runner.  `new()` is called from within a tokio runtime
        // (from main() which is `#[tokio::main]`), so `tokio::spawn` is safe.
        tokio::spawn(chain_sel_runner);

        Ok(Node {
            config: args.config,
            topology: args.topology,
            chain_db,
            ledger_state,
            consensus,
            mempool,
            // Lifecycle manager, fetch task, and fetch channel are initialized
            // in run() once the block_announcement_tx is created.
            connection_lifecycle: None,
            block_fetch_task: None,
            fetched_blocks_rx: None,
            query_handler,
            peer_manager: Arc::new(RwLock::new(NodePeerManager::new(
                PeerManagerConfig::default(),
            ))),
            socket_path,
            database_path: args.database_path,
            listen_addr,
            network_magic,
            byron_epoch_length,
            snapshot_policy: epoch::SnapshotPolicy::with_params(
                shelley_genesis
                    .as_ref()
                    .map(|g| g.security_param)
                    .unwrap_or(2160),
                args.snapshot_max_retained,
                args.snapshot_bulk_min_blocks,
                args.snapshot_bulk_min_secs,
            ),
            shelley_genesis,
            era_history,
            topology_path: args.topology_path,
            metrics: node_metrics,
            block_producer,
            block_announcement_tx: None,
            rollback_announcement_tx: None,
            metrics_port: args.metrics_port,
            expected_byron_genesis_hash,
            expected_shelley_genesis_hash,
            genesis_validated: false,
            // Primed from the snapshot epoch so the nonce is immediately
            // considered established after a restore (see comment above where
            // snapshot_epoch_transitions is assigned).  Starts at 0 for a
            // fresh start with no snapshot, meaning a live epoch boundary must
            // be observed before nonce_established becomes true.
            epoch_transitions_observed: snapshot_epoch_transitions,
            live_epoch_transitions: 0,
            consensus_mode: args.consensus_mode,
            validate_all_blocks: args.validate_all_blocks,
            disk_space_rx: watch::channel(crate::disk_monitor::DiskSpaceLevel::Ok).1,
            gsm,
            chain_fragment: Arc::new(RwLock::new(chain_fragment)),
            chain_sel_handle: Some(chain_sel_handle),

            // ── Phase 5: Background operations ───────────────────────────────
            //
            // The security parameter k is taken from the consensus object,
            // which was already initialised from the Shelley genesis above.
            // For fresh nodes without genesis config, consensus defaults to
            // 2160 (mainnet/preview/preprod all use k=2160).
            copy_to_immutable: CopyToImmutable::new(consensus_security_param as usize),
            gc_scheduler: GcScheduler::new(),
            bg_snapshot_scheduler: SnapshotScheduler::new(),
        })
    }

    // ─── run() ───────────────────────────────────────────────────────────────

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

            // Initialize Prometheus metrics from loaded ledger state so they
            // are accurate immediately on startup (before any blocks arrive).
            self.metrics.set_epoch(ls.epoch.0);
            self.metrics.set_utxo_count(ls.utxo_set.len() as u64);
            self.metrics.set_mempool_count(self.mempool.len() as u64);
            self.metrics.set_mempool_max(self.mempool.capacity() as u64);
            self.metrics.delegation_count.store(
                ls.delegations.len() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            self.metrics
                .treasury_lovelace
                .store(ls.treasury.0, std::sync::atomic::Ordering::Relaxed);
            self.metrics.pool_count.store(
                ls.pool_params.len() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            self.metrics.drep_count.store(
                ls.governance.active_drep_count() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            self.metrics.proposal_count.store(
                ls.governance.proposals.len() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            // Set slot/block from tip and compute sync progress
            if let Some(slot) = tip.point.slot() {
                self.metrics.set_slot(slot.0);
                self.metrics.set_block_number(tip.block_number.0);
                // Compute initial sync progress from tip slot
                let tip_slot = slot.0;
                if tip_slot > 0 {
                    // At startup with a snapshot, we're close to or at the tip.
                    // A more accurate progress would need the network tip, but
                    // 100% is a reasonable initial estimate for a loaded snapshot.
                    self.metrics.set_sync_progress(100.0);
                }
                // Initialize tip slot time for tip_age_seconds computation
                let sc = &ls.slot_config;
                let slot_time_ms =
                    sc.zero_time + slot.0.saturating_sub(sc.zero_slot) * sc.slot_length as u64;
                self.metrics.set_tip_slot_time_ms(slot_time_ms);
            }
        }

        // Setup shutdown signal (SIGINT + SIGTERM) early so the node can be
        // gracefully stopped during replay (which can take 30+ minutes).
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        #[cfg(unix)]
        {
            let shutdown_tx_clone = shutdown_tx.clone();
            tokio::spawn(async move {
                // Startup-time panic is acceptable — if we can't register signal
                // handlers, the node cannot shut down gracefully.
                let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate())
                    .expect("failed to register SIGTERM handler");
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

        // Start Prometheus metrics server before replay so /health, /ready,
        // and /metrics are available during the (potentially long) replay window.
        if self.metrics_port > 0 {
            let metrics = self.metrics.clone();
            let port = self.metrics_port;
            let metrics_shutdown_rx = shutdown_rx.clone();
            tokio::spawn(async move {
                if let Err(e) =
                    crate::metrics::start_metrics_server(port, metrics, metrics_shutdown_rx).await
                {
                    error!(
                        port,
                        "Metrics server failed to start: {e} — node will continue without metrics"
                    );
                }
            });
        }

        // Replay blocks from ChainDB if the ledger is behind storage.
        // This happens after a Mithril snapshot import — blocks are in storage
        // but the ledger hasn't processed them yet.
        let replay_start = std::time::Instant::now();
        self.replay_ledger_from_storage(shutdown_rx.clone()).await;
        self.metrics
            .set_replay_duration_secs(replay_start.elapsed().as_secs());
        if *shutdown_rx.borrow() {
            info!("Shutdown requested during replay, exiting");
            return Ok(());
        }

        // epoch_transitions_observed is primed from the snapshot epoch in Node::new().
        // A snapshot at epoch N means the epoch nonce has been correctly established
        // through N prior transitions, so nonce_established becomes true immediately
        // at tip without requiring a live epoch boundary.  For a fresh start (no
        // snapshot, epoch=0) or after a full reset the value starts at 0 and
        // nonce_established remains false until the first live boundary is crossed.

        // If running as block producer, log the pool's stake in the set snapshot
        // so operators can immediately diagnose eligibility issues.
        if let Some(ref creds) = self.block_producer {
            let ls = self.ledger_state.read().await;
            if let Some(ref set_snap) = ls.snapshots.set {
                let total_stake: u64 = set_snap.pool_stake.values().map(|s| s.0).sum();
                let pool_stake = set_snap
                    .pool_stake
                    .get(&creds.pool_id)
                    .map(|s| s.0)
                    .unwrap_or(0);
                let relative_stake = if total_stake > 0 {
                    pool_stake as f64 / total_stake as f64
                } else {
                    0.0
                };
                info!(
                    pool_id = %creds.pool_id,
                    snapshot_epoch = set_snap.epoch.0,
                    pool_stake_lovelace = pool_stake,
                    total_active_stake_lovelace = total_stake,
                    relative_stake = format_args!("{relative_stake:.8}"),
                    "Block producer: pool stake in 'set' snapshot (used for leader election)",
                );
                if pool_stake == 0 {
                    // Diagnostic: check if pool is in delegations, pool_params,
                    // and if any credentials delegate to it.
                    let pool_in_params = ls.pool_params.contains_key(&creds.pool_id);
                    let delegators_to_pool = set_snap
                        .delegations
                        .values()
                        .filter(|pid| **pid == creds.pool_id)
                        .count();
                    let main_delegators_to_pool = ls
                        .delegations
                        .values()
                        .filter(|pid| **pid == creds.pool_id)
                        .count();
                    warn!(
                        pool_id = %creds.pool_id,
                        snapshot_epoch = set_snap.epoch.0,
                        total_pools_in_snapshot = set_snap.pool_stake.len(),
                        pool_in_params,
                        snapshot_delegators = delegators_to_pool,
                        main_delegators = main_delegators_to_pool,
                        total_snapshot_delegations = set_snap.delegations.len(),
                        total_main_delegations = ls.delegations.len(),
                        "Block producer has ZERO stake in 'set' snapshot — will not be elected slot leader. \
                         Pool may not be in snapshot or stake distribution may need rebuilding.",
                    );
                }
            } else {
                warn!(
                    pool_id = %creds.pool_id,
                    "Block producer: no 'set' snapshot available — leader election disabled until epoch transition"
                );
            }
        }

        // Initialize query state from current ledger so N2C queries
        // work immediately (before we reach chain tip or the periodic timer fires)
        self.update_query_state().await;

        // SIGHUP handler is set up after peer_manager initialization below

        // Start disk space monitor on the database volume
        {
            let (disk_level_tx, disk_level_rx) =
                watch::channel(crate::disk_monitor::DiskSpaceLevel::Ok);
            self.disk_space_rx = disk_level_rx;
            let db_path = self.database_path.clone();
            let metrics = self.metrics.clone();
            let disk_shutdown_rx = shutdown_rx.clone();
            tokio::spawn(async move {
                crate::disk_monitor::start_disk_monitor(
                    db_path,
                    metrics,
                    disk_shutdown_rx,
                    disk_level_tx,
                )
                .await;
            });
        }

        // Start N2C server on Unix socket.
        //
        // Each accepted connection gets its own Mux and set of protocol tasks:
        //   - Handshake (protocol 0, responder)
        //   - LocalChainSync (protocol 5, responder)
        //   - LocalTxSubmission (protocol 6, responder)
        //   - LocalStateQuery (protocol 7, responder)
        //   - LocalTxMonitor (protocol 9, responder)
        {
            let n2c_socket_path = self.socket_path.clone();
            let n2c_shutdown_rx = shutdown_rx.clone();
            let n2c_network_magic = self.network_magic;
            let n2c_query_handler = self.query_handler.clone();
            let n2c_mempool = self.mempool.clone();
            let n2c_ledger = self.ledger_state.clone();
            let n2c_metrics = self.metrics.clone();
            // Build the block provider for LocalChainSync
            let n2c_block_provider = Arc::new(serve::ChainDBBlockProvider {
                chain_db: self.chain_db.clone(),
            });
            // Build the tx validator for LocalTxSubmission
            let n2c_slot_config = self
                .shelley_genesis
                .as_ref()
                .map(|g| g.slot_config())
                .unwrap_or(torsten_ledger::plutus::SlotConfig {
                    zero_time: 0,
                    zero_slot: 0,
                    slot_length: 1000,
                });
            let n2c_tx_validator = Arc::new(serve::LedgerTxValidator {
                ledger: self.ledger_state.clone(),
                slot_config: n2c_slot_config,
                metrics: self.metrics.clone(),
                mempool: Some(self.mempool.clone()),
            });

            // Remove stale socket file if it exists (e.g., from a previous unclean shutdown).
            if n2c_socket_path.exists() {
                if let Err(e) = std::fs::remove_file(&n2c_socket_path) {
                    warn!(
                        "Failed to remove stale socket {}: {e}",
                        n2c_socket_path.display()
                    );
                }
            }

            let listener = match tokio::net::UnixListener::bind(&n2c_socket_path) {
                Ok(l) => l,
                Err(e) => {
                    error!(
                        "Failed to bind N2C Unix socket at {}: {e}",
                        n2c_socket_path.display()
                    );
                    return Err(e.into());
                }
            };
            info!(
                socket = %n2c_socket_path.display(),
                "N2C server listening"
            );

            // We need a block_announcement_tx for LocalChainSync server.
            // It will be set below when the N2N server creates the broadcast channels.
            // For now, create a placeholder that will be replaced.
            // Actually, we share the same broadcast channel — create it here and use
            // it for both N2C LocalChainSync and N2N ChainSync server.
            let (block_ann_tx, _) =
                tokio::sync::broadcast::channel::<torsten_network::BlockAnnouncement>(64);
            let (rollback_ann_tx, _) = tokio::sync::broadcast::channel::<RollbackAnnouncement>(16);
            self.block_announcement_tx = Some(block_ann_tx.clone());
            self.rollback_announcement_tx = Some(rollback_ann_tx);
            let n2c_block_ann_tx = block_ann_tx;

            tokio::spawn(async move {
                let mut shutdown = n2c_shutdown_rx;
                // Track spawned connection handlers so we can abort them on
                // shutdown — otherwise they block indefinitely waiting for
                // client I/O, preventing the process from exiting.
                let mut conn_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();
                loop {
                    tokio::select! {
                        accept_result = listener.accept() => {
                            match accept_result {
                                Ok((stream, _addr)) => {
                                    let conn_metrics = n2c_metrics.clone();
                                    conn_metrics
                                        .n2c_connections_total
                                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                    conn_metrics
                                        .n2c_connections_active
                                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                                    let qh = n2c_query_handler.clone();
                                    let bp = n2c_block_provider.clone();
                                    let mp = n2c_mempool.clone();
                                    let tv = n2c_tx_validator.clone();
                                    let ledger = n2c_ledger.clone();
                                    let metrics = conn_metrics.clone();
                                    let ann_rx = n2c_block_ann_tx.subscribe();
                                    let magic = n2c_network_magic;

                                    let handle = tokio::spawn(async move {
                                        if let Err(e) = Self::handle_n2c_connection(
                                            stream, magic, qh, bp, mp, tv, ledger, ann_rx,
                                            metrics.clone(),
                                        )
                                        .await
                                        {
                                            debug!("N2C connection ended: {e}");
                                        }
                                        metrics
                                            .n2c_connections_active
                                            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                                    });
                                    conn_handles.push(handle);
                                }
                                Err(e) => {
                                    warn!("N2C accept error: {e}");
                                }
                            }
                        }
                        _ = shutdown.changed() => {
                            info!("N2C server shutting down");
                            break;
                        }
                    }
                }
                // Abort all active N2C connection handlers.
                for handle in &conn_handles {
                    handle.abort();
                }
            });
        }

        // Initialize peer manager
        {
            let pm_config = PeerManagerConfig {
                diffusion_mode: match self.config.diffusion_mode {
                    crate::config::DiffusionMode::InitiatorOnly => DiffusionMode::InitiatorOnly,
                    crate::config::DiffusionMode::InitiatorAndResponder => {
                        DiffusionMode::InitiatorAndResponder
                    }
                },
                peer_sharing_enabled: self
                    .config
                    .effective_peer_sharing(self.block_producer.is_some()),
                target_hot_peers: self.config.target_number_of_active_peers,
                target_warm_peers: self
                    .config
                    .target_number_of_established_peers
                    .saturating_sub(self.config.target_number_of_active_peers),
                target_known_peers: self.config.target_number_of_known_peers,
                ..PeerManagerConfig::default()
            };
            let mut pm = NodePeerManager::new(pm_config);
            // Register our own listen address to prevent self-connections
            // (peers may share our address back to us via peer sharing)
            pm.set_local_addr(self.listen_addr);
            *self.peer_manager.write().await = pm;
        }
        let peer_manager = self.peer_manager.clone();

        // Register topology peers in the peer manager with full metadata
        let detailed_peers = self.topology.detailed_peers();
        if detailed_peers.is_empty() {
            warn!("No peers configured in topology");
            return Ok(());
        }
        if self.topology.has_bootstrap_peers() {
            info!(
                "Bootstrap peers configured (trustable: {})",
                self.topology.has_trustable_peers()
            );
        }
        {
            // Resolve all DNS addresses BEFORE acquiring the write lock to avoid
            // holding the lock during potentially slow DNS lookups.
            let mut resolved_peers: Vec<std::net::SocketAddr> = Vec::new();
            for peer in &detailed_peers {
                match tokio::net::lookup_host(format!("{}:{}", peer.address, peer.port)).await {
                    Ok(addrs) => {
                        for socket_addr in addrs {
                            resolved_peers.push(socket_addr);
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

            // Resolve local root group members for per-group valency registration.
            // Each entry is (resolved_addrs_for_group, hot_valency, warm_valency).
            // We collect these here (pre-lock) so that `add_local_root_group` can
            // be called with already-resolved addresses while holding the PM lock.
            let mut resolved_groups: Vec<(Vec<std::net::SocketAddr>, usize, usize)> = Vec::new();
            for group in &self.topology.local_roots {
                let hot_val = usize::from(group.effective_hot_valency());
                let warm_val = usize::from(group.effective_warm_valency());
                let mut group_addrs = Vec::new();
                for ap in &group.access_points {
                    match tokio::net::lookup_host(format!("{}:{}", ap.address, ap.port)).await {
                        Ok(addrs) => {
                            for socket_addr in addrs {
                                group_addrs.push(socket_addr);
                            }
                        }
                        Err(e) => {
                            warn!(
                                address = %ap.address,
                                port = ap.port,
                                "Failed to resolve local root group member address: {e}"
                            );
                        }
                    }
                }
                if !group_addrs.is_empty() {
                    resolved_groups.push((group_addrs, hot_val, warm_val));
                }
            }

            let mut pm = peer_manager.write().await;
            for socket_addr in resolved_peers {
                pm.add_config_peer(socket_addr);
            }
            // Register per-group valency targets.  This must happen AFTER
            // add_config_peer() calls so the peer table contains the members.
            for (group_addrs, hot_val, warm_val) in resolved_groups {
                pm.add_local_root_group(networking::LocalRootGroupInfo {
                    name: String::new(),
                    addrs: group_addrs,
                    hot_valency: hot_val,
                    warm_valency: warm_val,
                });
            }
            let stats = pm.stats();
            info!(
                known = stats.cold + stats.warm + stats.hot,
                local_root_groups = pm.local_root_groups().len(),
                mode = ?pm.diffusion_mode(),
                "Peers",
            );
        }
        let _peers = self.topology.all_peers();

        // Setup SIGHUP handler for topology reload
        #[cfg(unix)]
        {
            let topology_path = self.topology_path.clone();
            let pm_for_sighup = peer_manager.clone();
            let mut hup_shutdown_rx = shutdown_rx.clone();
            tokio::spawn(async move {
                let mut hup = match signal::unix::signal(signal::unix::SignalKind::hangup()) {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("Failed to setup SIGHUP handler: {e}");
                        return;
                    }
                };
                loop {
                    tokio::select! {
                        _ = hup.recv() => {
                            info!(
                                "SIGHUP received — reloading topology from {}",
                                topology_path.display()
                            );
                            match Topology::load(&topology_path) {
                                Ok(new_topology) => {
                                    let new_peers = new_topology.detailed_peers();
                                    // Resolve DNS before acquiring the write lock
                                    let mut resolved: Vec<std::net::SocketAddr> = Vec::new();
                                    for peer in &new_peers {
                                        match tokio::net::lookup_host(format!(
                                            "{}:{}",
                                            peer.address, peer.port
                                        ))
                                        .await
                                        {
                                            Ok(addrs) => {
                                                for socket_addr in addrs {
                                                    resolved.push(socket_addr);
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
                                    for socket_addr in resolved {
                                        pm.add_config_peer(socket_addr);
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
                        _ = hup_shutdown_rx.changed() => {
                            info!("SIGHUP handler shutting down");
                            break;
                        }
                    }
                }
            });
        }

        // Start N2N server for inbound peer connections.
        //
        // When DiffusionMode is InitiatorOnly, skip the N2N listener entirely —
        // the node only makes outbound connections (typical for block producers
        // behind a firewall).  Matches Haskell's `runM` branch that skips
        // `Server.with` for InitiatorOnlyDiffusionMode.
        //
        // Each accepted TCP connection gets its own Mux and set of protocol tasks:
        //   - Handshake (protocol 0, responder)
        //   - ChainSync (protocol 2, responder)
        //   - BlockFetch (protocol 3, responder)
        //   - TxSubmission2 (protocol 4, responder)
        //   - KeepAlive (protocol 8, responder)
        //   - PeerSharing (protocol 10, responder)
        //
        // The broadcast channels were already created by the N2C server above.
        // If block_announcement_tx is None (N2C server was skipped for some reason),
        // create the channels here as a fallback.
        if self.block_announcement_tx.is_none() {
            let (block_ann_tx, _) =
                tokio::sync::broadcast::channel::<torsten_network::BlockAnnouncement>(64);
            let (rollback_ann_tx, _) = tokio::sync::broadcast::channel::<RollbackAnnouncement>(16);
            self.block_announcement_tx = Some(block_ann_tx);
            self.rollback_announcement_tx = Some(rollback_ann_tx);
        }
        if self.config.diffusion_mode == crate::config::DiffusionMode::InitiatorAndResponder {
            let n2n_listen_addr = self.listen_addr;
            let n2n_shutdown_rx = shutdown_rx.clone();
            let n2n_network_magic = self.network_magic;
            let n2n_peer_sharing = self
                .config
                .effective_peer_sharing(self.block_producer.is_some());
            let n2n_metrics = self.metrics.clone();
            let n2n_peer_manager = peer_manager.clone();
            let n2n_block_provider = Arc::new(serve::ChainDBBlockProvider {
                chain_db: self.chain_db.clone(),
            });
            let n2n_mempool = self.mempool.clone();
            let n2n_block_ann_tx = self
                .block_announcement_tx
                .as_ref()
                .expect("block_announcement_tx was just set")
                .clone();

            let diffusion_mode = self.peer_manager.read().await.diffusion_mode();
            info!(
                listen = %n2n_listen_addr,
                diffusion_mode = ?diffusion_mode,
                "N2N server listening"
            );

            let tcp_listener = match tokio::net::TcpListener::bind(n2n_listen_addr).await {
                Ok(l) => l,
                Err(e) => {
                    error!("Failed to bind N2N TCP listener on {n2n_listen_addr}: {e}");
                    return Err(e.into());
                }
            };

            tokio::spawn(async move {
                let mut shutdown = n2n_shutdown_rx;
                // Track spawned inbound connection handlers so we can abort
                // them on shutdown — otherwise they block waiting for peer I/O.
                let mut conn_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();
                loop {
                    tokio::select! {
                        accept_result = tcp_listener.accept() => {
                            match accept_result {
                                Ok((stream, peer_addr)) => {
                                    info!(%peer_addr, "N2N inbound connection accepted");
                                    let conn_metrics = n2n_metrics.clone();
                                    conn_metrics
                                        .n2n_connections_total
                                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                    conn_metrics
                                        .n2n_connections_active
                                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                                    let bp = n2n_block_provider.clone();
                                    let mp = n2n_mempool.clone();
                                    let metrics = conn_metrics.clone();
                                    let pm = n2n_peer_manager.clone();
                                    let ann_rx = n2n_block_ann_tx.subscribe();
                                    let magic = n2n_network_magic;
                                    let ps = n2n_peer_sharing;

                                    // Start the connection handler immediately without
                                    // waiting for the peer manager lock — the handshake
                                    // must complete within the Haskell timeout (10s).
                                    // Peer registration happens after the handshake.
                                    let handle = tokio::spawn(async move {
                                        if let Err(e) = Self::handle_n2n_connection(
                                            stream, magic, ps, bp, mp, pm.clone(),
                                            peer_addr, ann_rx,
                                        )
                                        .await
                                        {
                                            debug!(
                                                %peer_addr,
                                                "N2N inbound connection ended: {e}"
                                            );
                                        }
                                        metrics
                                            .n2n_connections_active
                                            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                                        pm.write().await.peer_disconnected(&peer_addr);
                                    });
                                    conn_handles.push(handle);
                                }
                                Err(e) => {
                                    warn!("N2N accept error: {e}");
                                }
                            }
                        }
                        _ = shutdown.changed() => {
                            info!("N2N server shutting down");
                            break;
                        }
                    }
                }
                // Abort all active N2N inbound connection handlers.
                for handle in &conn_handles {
                    handle.abort();
                }
            });
        } else {
            info!("N2N server skipped (DiffusionMode=InitiatorOnly, outbound connections only)");
        }

        // Start ledger-based peer discovery task (only when P2P is enabled)
        if self.config.enable_p2_p {
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

                    // Extract relay addresses from registered pools and
                    // identify Big Ledger Peers (top 90% of active stake).
                    type RelayList = Vec<(String, u16)>;
                    let (relays, blp_relays): (RelayList, RelayList) = {
                        let ls = ledger.read().await;

                        // Build pool_id -> stake map for BLP classification
                        let pool_stakes: Vec<_> = ls
                            .pool_params
                            .keys()
                            .map(|pool_id| {
                                let stake = ls
                                    .snapshots
                                    .set
                                    .as_ref()
                                    .and_then(|s| s.pool_stake.get(pool_id))
                                    .map(|s| s.0)
                                    .unwrap_or(0);
                                (pool_id.as_bytes().to_vec(), stake)
                            })
                            .collect();
                        let (big_pool_ids, _) = crate::gsm::identify_big_ledger_peers(&pool_stakes);
                        let big_pool_set: std::collections::HashSet<Vec<u8>> =
                            big_pool_ids.into_iter().collect();

                        let mut relays = Vec::new();
                        let mut blp_relays = Vec::new();
                        for (pool_id, pool_reg) in ls.pool_params.iter() {
                            let is_blp = big_pool_set.contains(pool_id.as_bytes().as_slice());
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
                                            relays.push((addr.clone(), *port));
                                            if is_blp {
                                                blp_relays.push((addr, *port));
                                            }
                                        }
                                    }
                                    torsten_primitives::transaction::Relay::SingleHostName {
                                        port,
                                        dns_name,
                                    } => {
                                        if let Some(port) = port {
                                            relays.push((dns_name.clone(), *port));
                                            if is_blp {
                                                blp_relays.push((dns_name.clone(), *port));
                                            }
                                        }
                                    }
                                    torsten_primitives::transaction::Relay::MultiHostName {
                                        dns_name,
                                    } => {
                                        relays.push((dns_name.clone(), 3001));
                                        if is_blp {
                                            blp_relays.push((dns_name.clone(), 3001));
                                        }
                                    }
                                }
                            }
                        }
                        (relays, blp_relays)
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
                    // Also resolve BLP relay addresses
                    let blp_set: std::collections::HashSet<String> =
                        blp_relays.iter().map(|(h, p)| format!("{h}:{p}")).collect();
                    let mut blp_resolved = std::collections::HashSet::new();
                    for addr in &resolved_addrs {
                        // Check if this resolved address came from a BLP relay
                        // (approximate: check if any BLP relay resolves to this addr)
                        if blp_set
                            .iter()
                            .any(|blp_hp| blp_hp.ends_with(&format!(":{}", addr.port())))
                        {
                            blp_resolved.insert(*addr);
                        }
                    }

                    if !resolved_addrs.is_empty() {
                        let mut pm_w = pm.write().await;
                        for socket_addr in &resolved_addrs {
                            pm_w.add_ledger_peer(*socket_addr);
                            if blp_resolved.contains(socket_addr) {
                                pm_w.add_big_ledger_peer(*socket_addr);
                            }
                        }
                        let added = resolved_addrs.len();
                        let blp_count = blp_resolved.len();
                        debug!(
                            "Ledger peer discovery: +{added} peers ({blp_count} BLPs) from {} relays, {}",
                            relays.len(),
                            pm_w.stats()
                        );
                    }
                }
            });
        }

        // ─── Initialize ConnectionLifecycleManager ─────────────────────────
        //
        // The lifecycle manager owns all peer connections and handles
        // temperature transitions (Cold -> Warm -> Hot and back).
        // Governor actions are dispatched through the lifecycle manager,
        // which creates/tears down protocol tasks on the single per-peer
        // mux connection.
        let (fetched_blocks_tx, fetched_blocks_rx) = mpsc::channel::<FetchedBlock>(1000);
        let candidate_chains: Arc<RwLock<HashMap<std::net::SocketAddr, CandidateChainState>>> =
            Arc::new(RwLock::new(HashMap::new()));

        let connect_timeout = Duration::from_secs(5);
        // Read security_param from genesis config; fall back to mainnet default.
        let security_param = self
            .shelley_genesis
            .as_ref()
            .map(|g| g.security_param)
            .unwrap_or(2160);
        let lifecycle = ConnectionLifecycleManager::new(
            self.network_magic,
            self.config.diffusion_mode == crate::config::DiffusionMode::InitiatorOnly,
            self.config
                .effective_peer_sharing(self.block_producer.is_some()),
            connect_timeout,
            candidate_chains.clone(),
            fetched_blocks_tx.clone(),
            self.block_announcement_tx
                .as_ref()
                .expect("block_announcement_tx was just set")
                .clone(),
            self.chain_db.clone(),
            self.ledger_state.clone(),
            self.byron_epoch_length,
            security_param,
            self.metrics.clone(),
            self.mempool.clone(),
        );
        self.connection_lifecycle = Some(lifecycle);
        self.fetched_blocks_rx = Some(fetched_blocks_rx);

        // ─── Spawn BlockFetch Decision Task ──────────────────────────────
        //
        // Independent task matching Haskell's `blockFetchLogic` thread.
        // Reads candidate chain state from ChainSync tasks, dispatches
        // fetch ranges to per-peer BlockFetch workers.
        {
            let bf_cancel = tokio_util::sync::CancellationToken::new();
            let mut bf_task = BlockFetchLogicTask::new(
                candidate_chains.clone(),
                fetched_blocks_tx,
                self.byron_epoch_length,
                bf_cancel.clone(),
            );
            let bf_shutdown = shutdown_rx.clone();
            let bf_handle = tokio::spawn(async move {
                // Shut down the decision task when the node shuts down.
                let mut shutdown = bf_shutdown;
                tokio::select! {
                    _ = bf_task.run() => {}
                    _ = shutdown.changed() => {
                        bf_cancel.cancel();
                    }
                }
            });
            self.block_fetch_task = Some(bf_handle);
        }

        // ─── GSM (Genesis State Machine) ─────────────────────────────────
        let genesis_enabled = self.consensus_mode == "genesis";
        if genesis_enabled {
            info!(
                state = %self.gsm.blocking_read().state(),
                "Genesis mode enabled — note: lightweight checkpointing and Genesis-specific \
                 peer selection are not yet implemented. The GSM provides basic state tracking \
                 (PreSyncing/Syncing/CaughtUp) and density-based peer disconnection."
            );
        }

        // Spawn GSM evaluation task
        if genesis_enabled {
            let gsm_ref = self.gsm.clone();
            let gsm_pm = peer_manager.clone();
            let gsm_metrics = self.metrics.clone();
            let gsm_shutdown = shutdown_rx.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(10));
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
                    let tip_age_secs = gsm_metrics
                        .tip_age_secs
                        .load(std::sync::atomic::Ordering::Relaxed);
                    let chainsync_idle = gsm_metrics
                        .chainsync_idle_secs
                        .load(std::sync::atomic::Ordering::Relaxed);
                    let all_idle = chainsync_idle > 30;
                    gsm_w.evaluate(active_blp, all_idle, tip_age_secs);
                }
            });
        }

        // ─── Main Run Loop ───────────────────────────────────────────────
        //
        // Single event loop that processes:
        // 1. Fetched blocks from BlockFetch workers -> apply to ledger
        // 2. Governor evaluation (every 2s) -> temperature transitions
        // 3. Forge ticker (every slot) -> block production
        // 4. Shutdown signal
        //
        // This replaces the old dual-path architecture (separate governor
        // connections + separate sync connections) with a unified loop that
        // receives blocks from the lifecycle-managed connections.
        let gov_config = {
            let cfg = &self.config;
            GovernorConfig {
                targets: PeerTargets {
                    target_warm: cfg.target_number_of_established_peers,
                    target_hot: cfg.target_number_of_active_peers,
                    max_cold: cfg.target_number_of_known_peers,
                },
                ..Default::default()
            }
        };
        let mut governor = Governor::new(gov_config);

        // Governor evaluation every 2 seconds — matches Haskell's warm-promotion
        // check frequency for responsive peer lifecycle management.
        let mut governor_ticker = tokio::time::interval(Duration::from_secs(2));
        // Skip mode: if the main loop was busy (e.g. applying blocks), we do NOT
        // want to burst-fire all missed governor ticks — one evaluation per interval
        // is sufficient and avoids multiple simultaneous peer-connect waves.
        governor_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        governor_ticker.tick().await; // skip first immediate tick

        // Channel for background cold->warm connection results.
        // The governor spawns connect tasks instead of awaiting them inline;
        // completed results arrive here for registration in the main loop.
        let (connect_result_tx, mut connect_result_rx) = mpsc::channel::<ConnectResult>(64);

        // Peers currently being connected in background tasks.
        // Prevents duplicate spawns when the governor fires repeatedly before
        // a slow TCP connect (up to connect_timeout) finishes.
        let mut in_flight_connects: std::collections::HashSet<std::net::SocketAddr> =
            std::collections::HashSet::new();

        // Take the fetched_blocks_rx out of self so we can use it in the select! loop
        // without holding a mutable borrow on self for the entire duration.
        let mut fetched_blocks_rx = self
            .fetched_blocks_rx
            .take()
            .expect("fetched_blocks_rx was just set");

        // Forge ticker — fires every second (slot granularity) to check
        // for block production opportunities.  Only active when the node
        // is configured as a block producer.
        let has_block_producer = self.block_producer.is_some();
        let mut forge_ticker = tokio::time::interval(Duration::from_secs(1));
        forge_ticker.tick().await; // skip first immediate tick

        // Buffer for out-of-order blocks, keyed by prev_hash.
        // When a block is applied, we check the buffer for the next block
        // that connects to the new tip.
        let mut pending_blocks: std::collections::HashMap<
            torsten_primitives::hash::Hash32,
            FetchedBlock,
        > = std::collections::HashMap::new();

        loop {
            tokio::select! {
                // ── Process fetched blocks from BlockFetch workers ───────
                //
                // Blocks may arrive out of order from multiple peers.
                // We attempt to apply each block directly; if it doesn't
                // connect to the ledger tip, we buffer it. After each
                // successful apply, we drain buffered blocks that now connect.
                Some(fetched) = fetched_blocks_rx.recv() => {
                    // Skip blocks the ledger has already processed.
                    // This happens when ChainDB is behind the ledger
                    // (e.g. after snapshot restore) and ChainSync re-sends
                    // old blocks from the ChainDB intersection.
                    {
                        let ls = self.ledger_state.read().await;
                        if fetched.block.block_number().0 <= ls.tip.block_number.0 {
                            continue;
                        }
                    }
                    let prev_hash = *fetched.block.prev_hash();
                    let block_hash = *fetched.block.hash();
                    debug!(
                        slot = fetched.block.slot().0,
                        block = fetched.block.block_number().0,
                        peer = %fetched.peer,
                        prev_hash = %prev_hash.to_hex(),
                        "Run loop: received fetched block",
                    );
                    let connects = {
                        let ls = self.ledger_state.read().await;
                        let tip_hash = ls.tip.point.hash().cloned();
                        let tip_block = ls.tip.block_number.0;
                        let block_no = fetched.block.block_number().0;

                        let hash_connects = match tip_hash.as_ref() {
                            Some(tip_hash) => prev_hash == *tip_hash,
                            None => true,
                        };

                        // If hash doesn't match but block number is the immediate
                        // successor, accept it. This handles the case where the
                        // ledger tip hash was computed from chunk file replay
                        // bytes that differ from BlockFetch wire format bytes
                        // (same block, different CBOR serialization → different
                        // header hash). Once the first network block is applied,
                        // subsequent blocks connect normally via hash.
                        let seq_connects = !hash_connects && block_no == tip_block + 1;

                        if seq_connects {
                            debug!(
                                block_no,
                                tip_block,
                                "Block connects by sequence number (hash mismatch — \
                                 likely replay vs network serialization difference)",
                            );
                        }

                        hash_connects || seq_connects
                    };

                    if connects {
                        debug!(slot = fetched.block.slot().0, "RUN LOOP: block connects, applying to ledger");
                        self.apply_fetched_block(fetched).await;
                        // Drain buffered blocks that now connect.
                        let mut current_hash = block_hash;
                        while let Some(next) = pending_blocks.remove(&current_hash) {
                            let next_hash = *next.block.hash();
                            self.apply_fetched_block(next).await;
                            current_hash = next_hash;
                        }
                    } else {
                        // Buffer for later — store keyed by prev_hash
                        // so we can find the next block when the tip advances.
                        debug!(
                            slot = fetched.block.slot().0,
                            block = fetched.block.block_number().0,
                            prev_hash = %prev_hash.to_hex(),
                            pending_count = pending_blocks.len(),
                            "Run loop: block does NOT connect, buffering",
                        );
                        pending_blocks.insert(prev_hash, fetched);
                    }

                    // Prune stale entries (blocks far behind ledger tip).
                    if pending_blocks.len() > 10_000 {
                        let tip_slot = self.ledger_state.read().await
                            .tip.point.slot().map(|s| s.0).unwrap_or(0);
                        pending_blocks.retain(|_, fb| fb.block.slot().0 > tip_slot.saturating_sub(1000));
                    }
                }

                // ── Governor evaluation (periodic, every 2s) ────────────
                _ = governor_ticker.tick() => {
                    // When P2P is disabled, skip governor evaluation entirely —
                    // static topology connections are maintained without churn.
                    if !self.config.enable_p2_p {
                        continue;
                    }

                    // Compute governor actions based on current peer state.
                    let actions = {
                        let pm = peer_manager.read().await;
                        governor.compute_actions(&pm.inner)
                    };

                    if !actions.is_empty() {
                        if let Some(ref mut lifecycle) = self.connection_lifecycle {
                            // PromoteToWarm: spawn background tasks so TCP
                            // connect + handshake never blocks the main loop.
                            // Each connect can take up to connect_timeout (default
                            // 10s); doing them sequentially here would starve
                            // fetched_blocks_rx for that entire duration.
                            for action in &actions {
                                if let torsten_network::peer::governor::GovernorAction::PromoteToWarm(addr) = action {
                                    // Skip peers that are already connected or
                                    // already have an in-flight background task.
                                    if lifecycle.has_connection(addr)
                                        || in_flight_connects.contains(addr)
                                    {
                                        continue;
                                    }
                                    in_flight_connects.insert(*addr);
                                    lifecycle.spawn_connect(*addr, connect_result_tx.clone());
                                }
                            }

                            // Non-connect actions (demote, disconnect, etc.) are
                            // still handled inline — they are fast O(1) operations.
                            let mut pm = peer_manager.write().await;
                            for action in actions {
                                match action {
                                    torsten_network::peer::governor::GovernorAction::PromoteToWarm(_) => {} // handled above
                                    other => {
                                        lifecycle.handle_governor_action(other, &mut pm).await;
                                    }
                                }
                            }

                            pm.recompute_reputations();

                            // Update peer metrics immediately after state transitions
                            // so counters reflect reality without waiting for the
                            // periodic metrics poll in the sync loop.
                            self.update_peer_metrics(&pm);
                        }
                    }

                    // Cleanup dead connections (mux terminated).
                    if let Some(ref mut lifecycle) = self.connection_lifecycle {
                        let mut pm = peer_manager.write().await;
                        // NOTE: cleanup to debug connection deaths
                        lifecycle.cleanup_dead_connections(&mut pm).await;

                        // Update metrics after removing dead connections.
                        self.update_peer_metrics(&pm);
                    }
                }

                // ── Background cold->warm connection results ─────────────
                //
                // The governor spawns `PeerConnection::connect()` in background
                // tasks (see `spawn_connect`) so TCP timeouts never block this
                // loop. Results arrive here; on success we register the peer as
                // warm and immediately promote to hot.
                Some(result) = connect_result_rx.recv() => {
                    match result {
                        Ok((addr, conn, rtt_ms)) => {
                            in_flight_connects.remove(&addr);
                            if let Some(ref mut lifecycle) = self.connection_lifecycle {
                                let mut pm = peer_manager.write().await;
                                match lifecycle.register_warm_connection(
                                    addr, conn, rtt_ms, &mut pm,
                                ) {
                                    Ok(()) => {
                                        // Promote straight to hot (matching
                                        // Haskell's established→active path).
                                        if let Err(e) =
                                            lifecycle.promote_to_hot(addr, &mut pm).await
                                        {
                                            warn!(%addr, "Warm→Hot failed after background connect: {e}");
                                        }
                                        self.update_peer_metrics(&pm);
                                    }
                                    Err(LifecycleError::AlreadyConnected(_)) => {
                                        // A concurrent inbound connection beat us;
                                        // discard the duplicate — it drops cleanly.
                                        debug!(%addr, "background connect raced inbound; discarding duplicate");
                                    }
                                    Err(e) => {
                                        warn!(%addr, "register_warm_connection failed: {e}");
                                        pm.peer_failed(&addr);
                                    }
                                }
                            }
                        }
                        Err((addr, error)) => {
                            in_flight_connects.remove(&addr);
                            warn!(%addr, "background cold->warm failed: {error}");
                            let mut pm = peer_manager.write().await;
                            pm.peer_failed(&addr);
                        }
                    }
                }

                // ── Forge ticker (block production) ─────────────────────
                _ = forge_ticker.tick(), if has_block_producer => {
                    self.try_forge_block().await;
                }

                // ── Shutdown ────────────────────────────────────────────
                _ = shutdown_rx.changed() => {
                    info!("Shutdown signal received");
                    break;
                }
            }
        }

        // Shut down all peer connections in parallel with a global timeout.
        // Each connection's shutdown() stops hot/warm protocols (up to 5s each)
        // and aborts the mux — doing this sequentially with N peers could take
        // minutes, so we run them all concurrently.
        if let Some(ref mut lifecycle) = self.connection_lifecycle {
            let connections = lifecycle.drain_connections();
            let count = connections.len();
            if count > 0 {
                info!(count, "Shutting down peer connections in parallel...");
                let shutdown_futs = connections.into_iter().map(|mut conn| async move {
                    conn.shutdown().await;
                });
                match tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    futures::future::join_all(shutdown_futs),
                )
                .await
                {
                    Ok(_) => info!(count, "All peer connections shut down"),
                    Err(_) => warn!(
                        count,
                        "Peer connection shutdown timed out after 10s, continuing"
                    ),
                }
            }
        }

        // Abort the BlockFetch decision task.
        if let Some(handle) = self.block_fetch_task.take() {
            handle.abort();
        }

        // Flush volatile blocks, persist ChainDB, and save ledger snapshot,
        // with a timeout to prevent hanging on shutdown.
        let shutdown_result = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            {
                let mut db = self.chain_db.write().await;
                match db.flush_all_to_immutable() {
                    Ok(n) if n > 0 => {
                        info!(blocks = n, "Flushed volatile blocks to ImmutableDB")
                    }
                    Ok(_) => {}
                    Err(e) => error!("Failed to flush volatile blocks on shutdown: {e}"),
                }
                if let Err(e) = db.persist() {
                    error!("Failed to persist ChainDB on shutdown: {e}");
                }
            }
            self.save_ledger_snapshot().await;
        })
        .await;

        match shutdown_result {
            Ok(()) => info!("Shutdown complete"),
            Err(_) => {
                error!("Graceful shutdown timed out after 30s, forcing exit");
                std::process::exit(1);
            }
        }
        Ok(())
    }

    // ─── Peer Metrics ────────────────────────────────────────────────────────

    /// Update peer metrics from current PeerManager state.
    ///
    /// Called immediately after lifecycle transitions (governor actions,
    /// dead connection cleanup) so Prometheus counters reflect reality
    /// without waiting for the periodic sync loop poll.
    fn update_peer_metrics(&self, pm: &crate::node::networking::NodePeerManager) {
        use std::sync::atomic::Ordering::Relaxed;
        self.metrics
            .peers_connected
            .store((pm.warm_peer_count() + pm.hot_peer_count()) as u64, Relaxed);
        self.metrics
            .peers_cold
            .store(pm.cold_peer_count() as u64, Relaxed);
        self.metrics
            .peers_warm
            .store(pm.warm_peer_count() as u64, Relaxed);
        self.metrics
            .peers_hot
            .store(pm.hot_peer_count() as u64, Relaxed);
        self.metrics
            .peers_outbound
            .store(pm.outbound_peer_count() as u64, Relaxed);
        self.metrics
            .peers_inbound
            .store(pm.inbound_peer_count() as u64, Relaxed);
        self.metrics
            .peers_duplex
            .store(pm.duplex_peer_count() as u64, Relaxed);
    }

    // ─── apply_fetched_block() ──────────────────────────────────────────────

    /// Apply a block fetched by a per-peer BlockFetch worker to the ledger.
    ///
    /// This is the main integration point between the BlockFetch pipeline and
    /// the ledger. Blocks arrive here from per-peer workers via the
    /// `fetched_blocks_rx` channel, already deserialized. We:
    ///
    /// 1. Store the block in ChainDB (via ChainSelQueue if available)
    /// 2. Apply to ledger state
    /// 3. Update metrics, chain fragment, and consensus tip
    /// 4. Announce to downstream peers
    ///
    /// Matches the flow previously handled inline in `chain_sync_loop()`.
    async fn apply_fetched_block(&mut self, fetched: FetchedBlock) {
        let block = fetched.block;
        let block_slot = block.slot();
        let block_number = block.block_number();
        let block_hash = *block.hash();

        debug!(
            peer = %fetched.peer,
            slot = block_slot.0,
            block = block_number.0,
            "Applying fetched block",
        );

        // Store in ChainDB via ChainSelQueue.
        let storage_succeeded = if let Some(ref handle) = self.chain_sel_handle {
            let cbor = block.raw_cbor.clone().unwrap_or_default();
            let result = handle
                .submit_block(
                    block_hash,
                    block_slot,
                    block_number,
                    *block.prev_hash(),
                    cbor,
                )
                .await;
            match result {
                Some(torsten_storage::AddBlockResult::AdoptedAsTip)
                | Some(torsten_storage::AddBlockResult::StoredNotAdopted)
                | Some(torsten_storage::AddBlockResult::AlreadyKnown) => true,
                Some(torsten_storage::AddBlockResult::SwitchedToFork {
                    intersection_hash,
                    rollback,
                    apply,
                }) => {
                    // Chain selection determined a competing fork is strictly
                    // preferred.  The VolatileDB chain switch is already committed.
                    // Phase 3 will wire the full ledger rollback + replay here.
                    info!(
                        intersection = %intersection_hash.to_hex(),
                        rollback_count = rollback.len(),
                        apply_count = apply.len(),
                        "Chain selection: fork switch detected (Phase 3 ledger rollback pending)"
                    );
                    true
                }
                Some(torsten_storage::AddBlockResult::Invalid(reason)) => {
                    warn!(
                        slot = block_slot.0,
                        block = block_number.0,
                        reason,
                        "Block rejected by ChainSelQueue"
                    );
                    false
                }
                None => {
                    error!("ChainSelQueue runner exited — block not stored");
                    false
                }
            }
        } else {
            // Fallback: direct ChainDB write.
            let cbor = block.raw_cbor.clone().unwrap_or_default();
            let mut db = self.chain_db.write().await;
            db.add_block(
                block_hash,
                block_slot,
                block_number,
                *block.prev_hash(),
                cbor,
            )
            .is_ok()
        };

        if !storage_succeeded {
            warn!(
                slot = block_slot.0,
                "Failed to store fetched block — skipping ledger apply"
            );
            return;
        }

        // Check if this block connects to the current ledger tip.
        // Blocks may arrive out of order from multiple peers. Only apply
        // blocks that extend the current chain; others are stored in ChainDB
        // (via ChainSelQueue above) and will be applied when the chain catches up.
        let prev_hash = *block.prev_hash();
        let connects_to_tip = {
            let ls = self.ledger_state.read().await;
            let tip_block = ls.tip.block_number.0;
            let hash_match = match ls.tip.point.hash() {
                Some(tip_hash) => prev_hash == *tip_hash,
                None => true, // Origin — any block connects
            };
            // Fallback: accept block_number == tip + 1 when hash doesn't
            // match (replay vs network serialization hash difference).
            hash_match || block_number.0 == tip_block + 1
        };

        if !connects_to_tip {
            debug!(
                slot = block_slot.0,
                block = block_number.0,
                "Block stored in ChainDB but skipping ledger apply (out of order)"
            );
            return;
        }

        // Determine validation mode.
        // Blocks from the network get full validation by default; only
        // ImmutableDB replay uses ApplyOnly.
        let validation_mode = if self.validate_all_blocks {
            BlockValidationMode::ValidateAll
        } else {
            BlockValidationMode::ApplyOnly
        };

        // Apply to ledger state.
        {
            let mut ls = self.ledger_state.write().await;
            if let Err(e) = ls.apply_block(&block, validation_mode) {
                warn!(
                    slot = block_slot.0,
                    block = block_number.0,
                    "Fetched block failed ledger apply: {e}"
                );
                return;
            }
            // Consume pending era transition and propagate to the HFC state machine.
            if let Some((prev_era, new_era, epoch)) = ls.pending_era_transition.take() {
                let mut eh = self.era_history.write().await;
                if eh.current_era() < new_era {
                    eh.record_era_transition(new_era, epoch.0);
                    info!(
                        prev = %prev_era,
                        new = %new_era,
                        epoch = epoch.0,
                        "Era transition recorded in HFC era history",
                    );
                }
            }
        }

        // Update chain fragment.
        {
            let mut fragment = self.chain_fragment.write().await;
            fragment.push(block.header.clone());
        }

        // Update consensus tip.
        self.consensus.update_tip(block.tip());

        // Log the new block at INFO level so operators can see chain advancement
        let hash_hex = block.header.header_hash.to_hex();
        info!(
            era = %block.era,
            slot = block_slot.0,
            block = block_number.0,
            txs = block.transactions.len(),
            hash = %hash_hex,
            "Chain extended",
        );

        // Update metrics.
        self.metrics.record_block_received();
        self.metrics
            .blocks_received
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.metrics
            .blocks_applied
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.metrics.set_slot(block_slot.0);
        self.metrics.set_block_number(block_number.0);
        // Update tip slot time so tip_age_seconds stays fresh
        {
            let ls = self.ledger_state.read().await;
            let sc = &ls.slot_config;
            let slot_time_ms =
                sc.zero_time + block_slot.0.saturating_sub(sc.zero_slot) * sc.slot_length as u64;
            self.metrics.set_tip_slot_time_ms(slot_time_ms);
            self.metrics.set_epoch(ls.epoch.0);
        }
        // Block arrived via live BlockFetch — node is following the chain tip.
        // Set progress to 100% so health_status() reports "healthy".
        self.metrics.set_sync_progress(100.0);

        // Announce to downstream peers.
        if let Some(ref tx) = self.block_announcement_tx {
            let mut hash_bytes = [0u8; 32];
            hash_bytes.copy_from_slice(block.header.header_hash.as_ref());
            tx.send(torsten_network::BlockAnnouncement {
                slot: block_slot.0,
                hash: hash_bytes,
                block_number: block_number.0,
            })
            .ok();
            self.metrics
                .blocks_announced
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }

        // Remove confirmed transactions from mempool.
        let confirmed: Vec<_> = block.transactions.iter().map(|tx| tx.hash).collect();
        if !confirmed.is_empty() {
            self.mempool.remove_txs(&confirmed);
        }

        // Sweep remaining mempool transactions for invalidity after the new block.
        // Catches double-spends (consumed inputs), TTL expiry, and orphaned chained
        // txs whose parent was confirmed. Mirrors process_forward_blocks() sync.rs:1355.
        //
        // Note: This uses a heuristic closure, not full validate_transaction().
        // Full reapplyTx-style revalidation is tracked as future work.
        if !self.mempool.is_empty() {
            let consumed_inputs: std::collections::HashSet<_> = block
                .transactions
                .iter()
                .flat_map(|tx| tx.body.inputs.iter().cloned())
                .collect();
            let tip_slot = block_slot; // already captured at top of apply_fetched_block
            let ls = self.ledger_state.read().await;
            self.mempool.revalidate_all(|tx| {
                // Evict if any input was consumed by this block (double-spend).
                if tx.body.inputs.iter().any(|i| consumed_inputs.contains(i)) {
                    return false;
                }
                // Evict if TTL has expired (half-open: slot >= ttl means expired).
                if let Some(ttl) = tx.body.ttl {
                    if tip_slot.0 >= ttl.0 {
                        return false;
                    }
                }
                // Evict if any input is absent from both on-chain UTxO and mempool
                // virtual UTxO (catches orphaned chained txs whose parent was removed).
                for input in &tx.body.inputs {
                    if !ls.utxo_set.contains(input)
                        && self.mempool.lookup_virtual_utxo(input).is_none()
                    {
                        return false;
                    }
                }
                true
            });
            drop(ls); // Release read lock before update_query_state() acquires it.
        }

        // Update mempool metrics so Prometheus reflects confirmed-tx removal
        // immediately. Placed unconditionally so the metric reaches 0 even
        // when the mempool is empty after remove_txs().
        self.metrics.set_mempool_count(self.mempool.len() as u64);
        self.metrics.mempool_bytes.store(
            self.mempool.total_bytes() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

        // Refresh the N2C query handler snapshot so LocalStateQuery clients
        // (e.g. `torsten-cli query tip`) see the latest ledger state immediately
        // after each block rather than waiting for the 30-second periodic refresh.
        self.update_query_state().await;

        // Run background maintenance (copy-to-immutable, GC, snapshot).
        // This matches the pattern from the old sync loop where these
        // operations run after each block application.
        self.run_background_maintenance().await;
    }

    // ─── run_background_maintenance() ────────────────────────────────────────

    /// Run periodic background maintenance after block application.
    ///
    /// Handles copy-to-immutable (when chain fragment grows beyond k),
    /// GC of old volatile entries, and snapshot scheduling. Matches
    /// Haskell's Background.hs pattern.
    ///
    /// Note: The full integration with CopyToImmutable, GcScheduler, and
    /// SnapshotScheduler requires the same detailed parameters as the
    /// existing chain_sync_loop path (fragment length, oldest header,
    /// ledger anchor advancement callback). These operations are already
    /// performed in process_forward_blocks() during sync. For blocks
    /// arriving via the new fetched_blocks channel, this is a placeholder
    /// that will be unified with process_forward_blocks() in Task 7.
    async fn run_background_maintenance(&mut self) {
        // Background maintenance (copy-to-immutable, GC, snapshot) is
        // handled by the existing process_forward_blocks() code path.
        // This method exists as a hook point for when the fetched-block
        // pipeline is fully wired in. For now, the sync.rs chain_sync_loop
        // continues to drive these operations.
    }

    // ─── handle_n2c_connection() ─────────────────────────────────────────────

    /// Handle a single N2C (Unix socket) connection.
    ///
    /// Sets up a Mux over the bearer, runs the N2C handshake, then spawns
    /// protocol tasks for LocalChainSync, LocalTxSubmission, LocalStateQuery,
    /// and LocalTxMonitor.
    #[allow(clippy::too_many_arguments)]
    async fn handle_n2c_connection(
        stream: tokio::net::UnixStream,
        network_magic: u64,
        query_handler: Arc<RwLock<QueryHandler>>,
        block_provider: Arc<serve::ChainDBBlockProvider>,
        mempool: Arc<Mempool>,
        tx_validator: Arc<serve::LedgerTxValidator>,
        ledger: Arc<RwLock<LedgerState>>,
        announcement_rx: tokio::sync::broadcast::Receiver<torsten_network::BlockAnnouncement>,
        metrics: Arc<crate::metrics::NodeMetrics>,
    ) -> Result<()> {
        use torsten_network::protocol;

        let bearer = torsten_network::UnixBearer::new(stream);
        let mut mux = torsten_network::Mux::new(bearer, false); // we are responder

        // Subscribe protocol channels (responder direction for all)
        let mut hs_ch = mux.subscribe(
            protocol::PROTOCOL_HANDSHAKE,
            torsten_network::Direction::ResponderDir,
            65536,
        );
        let mut cs_ch = mux.subscribe(
            protocol::PROTOCOL_N2C_CHAINSYNC,
            torsten_network::Direction::ResponderDir,
            1_048_576,
        );
        let mut tx_ch = mux.subscribe(
            protocol::PROTOCOL_N2C_TXSUBMISSION,
            torsten_network::Direction::ResponderDir,
            1_048_576,
        );
        let mut sq_ch = mux.subscribe(
            protocol::PROTOCOL_N2C_STATEQUERY,
            torsten_network::Direction::ResponderDir,
            1_048_576,
        );
        let mut tm_ch = mux.subscribe(
            protocol::PROTOCOL_N2C_TXMONITOR,
            torsten_network::Direction::ResponderDir,
            1_048_576,
        );

        // Start the mux tasks (egress/ingress)
        let mux_handle = tokio::spawn(async move { mux.run().await });

        // Run N2C handshake as server
        let our_data = torsten_network::N2CVersionData::new(network_magic);
        let hs_result =
            torsten_network::handshake::run_n2c_handshake_server(&mut hs_ch, &our_data).await;
        match hs_result {
            Ok(r) => {
                debug!(version = r.version, "N2C handshake accepted");
            }
            Err(e) => {
                debug!("N2C handshake failed: {e}");
                mux_handle.abort();
                return Ok(());
            }
        }

        // Spawn protocol tasks — each runs until the client disconnects
        // or an error occurs. The mux handle keeps the transport alive.

        // LocalChainSync server
        let lcs_bp = block_provider.clone();
        let lcs_ann_rx = announcement_rx;
        let lcs_task = tokio::spawn(async move {
            let mut server = protocol::local_chainsync::server::LocalChainSyncServer::new();
            if let Err(e) = server.run(&mut cs_ch, lcs_bp.as_ref(), lcs_ann_rx).await {
                debug!("N2C LocalChainSync ended: {e}");
            }
        });

        // LocalTxSubmission server
        let lts_validator = tx_validator;
        let lts_mempool = mempool.clone();
        let lts_metrics = metrics.clone();
        let lts_task = tokio::spawn(async move {
            let on_accepted = |era_id: u16, tx_bytes: Vec<u8>| {
                // Decode the transaction and add it to the mempool.
                let size_bytes = tx_bytes.len();
                match torsten_serialization::decode_transaction(era_id, &tx_bytes) {
                    Ok(tx) => {
                        let tx_hash = tx.hash;
                        debug!("N2C tx accepted, adding to mempool: {}", tx_hash);
                        if let Err(e) = lts_mempool.add_tx(tx_hash, tx, size_bytes) {
                            debug!("N2C tx accepted but mempool add failed: {e}");
                        }
                        // Update mempool metrics immediately after accepting a transaction
                        lts_metrics.set_mempool_count(lts_mempool.len() as u64);
                        lts_metrics.mempool_bytes.store(
                            lts_mempool.total_bytes() as u64,
                            std::sync::atomic::Ordering::Relaxed,
                        );
                    }
                    Err(e) => {
                        debug!("N2C tx decode for mempool failed: {e}");
                    }
                }
            };
            match protocol::local_tx_submission::server::LocalTxSubmissionServer::run(
                &mut tx_ch,
                lts_validator.as_ref(),
                on_accepted,
            )
            .await
            {
                Ok(stats) => {
                    // Update N2C transaction metrics
                    lts_metrics
                        .n2c_txs_submitted
                        .fetch_add(stats.submitted, std::sync::atomic::Ordering::Relaxed);
                    lts_metrics
                        .n2c_txs_accepted
                        .fetch_add(stats.accepted, std::sync::atomic::Ordering::Relaxed);
                    lts_metrics
                        .n2c_txs_rejected
                        .fetch_add(stats.rejected, std::sync::atomic::Ordering::Relaxed);
                    debug!(
                        submitted = stats.submitted,
                        accepted = stats.accepted,
                        rejected = stats.rejected,
                        "N2C LocalTxSubmission ended"
                    );
                }
                Err(e) => {
                    debug!("N2C LocalTxSubmission error: {e}");
                }
            }
        });

        // LocalStateQuery server
        let lsq_handler = query_handler;
        let lsq_task = tokio::spawn(async move {
            let handler = lsq_handler.read().await;
            if let Err(e) = protocol::local_state_query::server::LocalStateQueryServer::run(
                &mut sq_ch, &*handler,
            )
            .await
            {
                debug!("N2C LocalStateQuery ended: {e}");
            }
        });

        // LocalTxMonitor server
        let ltm_mempool = mempool;
        let ltm_ledger = ledger;
        let ltm_task = tokio::spawn(async move {
            let current_slot = || {
                // Use try_read to avoid blocking — return 0 if lock is contended
                ltm_ledger
                    .try_read()
                    .map(|ls| ls.tip.point.slot().map(|s| s.0).unwrap_or(0))
                    .unwrap_or(0)
            };
            if let Err(e) = protocol::local_tx_monitor::server::LocalTxMonitorServer::run(
                &mut tm_ch,
                ltm_mempool.as_ref(),
                current_slot,
            )
            .await
            {
                debug!("N2C LocalTxMonitor ended: {e}");
            }
        });

        // Wait for any protocol task to complete (usually means client disconnected),
        // then abort all others and clean up.
        tokio::select! {
            _ = lcs_task => {}
            _ = lts_task => {}
            _ = lsq_task => {}
            _ = ltm_task => {}
            r = mux_handle => {
                if let Ok(Err(e)) = r {
                    debug!("N2C mux error: {e}");
                }
            }
        }

        Ok(())
    }

    // ─── handle_n2n_connection() ─────────────────────────────────────────────

    /// Handle a single inbound N2N (TCP) connection.
    ///
    /// Sets up a Mux over the bearer, runs the N2N handshake as server, then
    /// spawns protocol tasks for ChainSync, BlockFetch, TxSubmission2,
    /// KeepAlive, and PeerSharing.
    #[allow(clippy::too_many_arguments)]
    async fn handle_n2n_connection(
        stream: tokio::net::TcpStream,
        network_magic: u64,
        peer_sharing: bool,
        block_provider: Arc<serve::ChainDBBlockProvider>,
        mempool: Arc<Mempool>,
        peer_manager: Arc<RwLock<NodePeerManager>>,
        peer_addr: std::net::SocketAddr,
        announcement_rx: tokio::sync::broadcast::Receiver<torsten_network::BlockAnnouncement>,
    ) -> Result<()> {
        use torsten_network::protocol;

        let bearer = torsten_network::TcpBearer::new(stream)?;
        let mut mux = torsten_network::Mux::new(bearer, false); // we are responder

        // Subscribe protocol channels (responder direction for all)
        let mut hs_ch = mux.subscribe(
            protocol::PROTOCOL_HANDSHAKE,
            torsten_network::Direction::ResponderDir,
            65536,
        );
        let mut cs_ch = mux.subscribe(
            protocol::PROTOCOL_N2N_CHAINSYNC,
            torsten_network::Direction::ResponderDir,
            1_048_576,
        );
        let mut bf_ch = mux.subscribe(
            protocol::PROTOCOL_N2N_BLOCKFETCH,
            torsten_network::Direction::ResponderDir,
            4_194_304,
        );
        let mut tx_ch = mux.subscribe(
            protocol::PROTOCOL_N2N_TXSUBMISSION,
            torsten_network::Direction::ResponderDir,
            1_048_576,
        );
        let mut ka_ch = mux.subscribe(
            protocol::PROTOCOL_N2N_KEEPALIVE,
            torsten_network::Direction::ResponderDir,
            65536,
        );
        let mut ps_ch = mux.subscribe(
            protocol::PROTOCOL_N2N_PEERSHARING,
            torsten_network::Direction::ResponderDir,
            65536,
        );

        // Start the mux tasks
        let mux_handle = tokio::spawn(async move { mux.run().await });

        // Run N2N handshake as server (responder).
        // This must complete quickly — the Haskell peer times out after 10 seconds.
        info!(%peer_addr, "N2N inbound: starting handshake");
        // When handling inbound connections, we are always in
        // InitiatorAndResponder mode (the listener is only started in that
        // mode), so initiator_only is false.
        let our_data = torsten_network::N2NVersionData::new(network_magic, false, peer_sharing);
        let hs_result =
            torsten_network::handshake::run_n2n_handshake_server(&mut hs_ch, &our_data).await;
        match hs_result {
            Ok(r) => {
                info!(
                    %peer_addr,
                    version = r.version,
                    "N2N inbound handshake accepted"
                );
            }
            Err(e) => {
                warn!(%peer_addr, "N2N inbound handshake failed: {e}");
                mux_handle.abort();
                return Ok(());
            }
        }

        // Register the peer in the peer manager asynchronously — do NOT block
        // protocol task startup on the write lock. The lifecycle manager may hold
        // the peer manager lock for 5+ seconds during TCP connect timeouts,
        // which would delay ChainSync/BlockFetch/TxSubmission2 server startup
        // and cause the Haskell peer to time out its ChainSync idle timeout.
        {
            let pm_clone = peer_manager.clone();
            let addr = peer_addr;
            tokio::spawn(async move {
                let mut pm_w = pm_clone.write().await;
                pm_w.peer_connected(&addr, crate::node::networking::ConnectionDirection::Inbound);
            });
        }

        // ChainSync server
        let cs_bp = block_provider.clone();
        let cs_ann_rx = announcement_rx;
        let cs_task = tokio::spawn(async move {
            let mut server = protocol::chainsync::server::ChainSyncServer::new();
            if let Err(e) = server.run(&mut cs_ch, cs_bp.as_ref(), cs_ann_rx).await {
                debug!("N2N ChainSync server ended: {e}");
            }
        });

        // BlockFetch server
        let bf_bp = block_provider.clone();
        let bf_task = tokio::spawn(async move {
            if let Err(e) =
                protocol::blockfetch::server::BlockFetchServer::run(&mut bf_ch, bf_bp.as_ref())
                    .await
            {
                debug!("N2N BlockFetch server ended: {e}");
            }
        });

        // TxSubmission2 server
        let tx_mempool = mempool;
        let tx_task = tokio::spawn(async move {
            let on_tx = |tx_hash: [u8; 32], tx_bytes: Vec<u8>| -> bool {
                // Best-effort mempool admission for peer-submitted txs.
                // Try all supported eras for decoding (Conway=6, Babbage=5, Alonzo=4, etc.)
                let size_bytes = tx_bytes.len();
                for era_id in [6u16, 5, 4, 3, 2] {
                    if let Ok(tx) = torsten_serialization::decode_transaction(era_id, &tx_bytes) {
                        let hash = torsten_primitives::hash::Hash32::from_bytes(tx_hash);
                        return tx_mempool.add_tx(hash, tx, size_bytes).is_ok();
                    }
                }
                false
            };
            match torsten_network::TxSubmissionServer::run(&mut tx_ch, on_tx).await {
                Ok(stats) => {
                    debug!(
                        tx_ids = stats.tx_ids_received,
                        txs_received = stats.txs_received,
                        accepted = stats.txs_accepted,
                        rejected = stats.txs_rejected,
                        "N2N TxSubmission2 server ended"
                    );
                }
                Err(e) => {
                    debug!("N2N TxSubmission2 server error: {e}");
                }
            }
        });

        // KeepAlive server
        let ka_task = tokio::spawn(async move {
            if let Err(e) = torsten_network::KeepAliveServer::run(&mut ka_ch).await {
                debug!("N2N KeepAlive server ended: {e}");
            }
        });

        // PeerSharing server — share routable peer addresses
        let ps_pm = peer_manager;
        let ps_task = tokio::spawn(async move {
            let peers: Vec<std::net::SocketAddr> = {
                let pm = ps_pm.read().await;
                pm.connected_peer_addrs()
            };
            if let Err(e) =
                protocol::peersharing::server::PeerSharingServer::run(&mut ps_ch, &peers).await
            {
                debug!("N2N PeerSharing server ended: {e}");
            }
        });

        // Wait for any protocol task to complete, then clean up.
        tokio::select! {
            _ = cs_task => {}
            _ = bf_task => {}
            _ = tx_task => {}
            _ = ka_task => {}
            _ = ps_task => {}
            r = mux_handle => {
                if let Ok(Err(e)) = r {
                    debug!(%peer_addr, "N2N mux error: {e}");
                }
            }
        }

        Ok(())
    }

    // ─── init_fresh_ledger() ─────────────────────────────────────────────────

    /// Create a fresh ledger state with genesis configuration applied.
    pub(crate) fn init_fresh_ledger(
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
        let shelley_transition_epoch = epoch::shelley_transition_epoch_for_magic(network_magic);
        ledger.set_shelley_transition(shelley_transition_epoch, byron_epoch_length);
        if let Some(hash) = shelley_genesis_hash {
            ledger.set_genesis_hash(hash);
        }
        if !byron_genesis_utxos.is_empty() {
            ledger.seed_genesis_utxos(byron_genesis_utxos);
        }
        ledger
    }

    // ─── try_forge_block() ───────────────────────────────────────────────────

    /// Attempt to forge a block if we are in block producer mode and are the slot leader.
    ///
    /// Called every slot when the node is caught up to the chain tip.
    /// Convenience wrapper that reads the wall-clock slot and calls
    /// `try_forge_block_at`.  Used by code paths where the slot hasn't
    /// already been sampled (e.g. after catching up to tip).
    pub(crate) async fn try_forge_block(&mut self) {
        if let Some(wc) = self.current_wall_clock_slot() {
            self.try_forge_block_at(wc).await;
        }
    }

    /// Try to forge a block at the given wall-clock slot.
    ///
    /// The slot is passed from the caller (sync loop forge ticker) to avoid
    /// a TOCTOU race: the sync loop reads the wall clock once and passes the
    /// same value here, preventing a double-forge if the clock advances
    /// between the guard check and the actual forge attempt.
    pub(crate) async fn try_forge_block_at(
        &mut self,
        wall_clock_slot: torsten_primitives::time::SlotNo,
    ) {
        let creds = match &self.block_producer {
            Some(c) => c,
            None => return, // relay-only mode
        };

        // Log (but don't skip) if epoch nonce isn't established yet.
        // Allow forging even with uncertain nonce — the network will reject
        // our block if the nonce is wrong, but it's better to try.
        if !self.consensus.nonce_established {
            debug!("Forge: epoch nonce not yet confirmed — forging anyway (block may be rejected)");
        }

        let ls = self.ledger_state.read().await;
        let tip_slot = ls.tip.point.slot().map(|s| s.0).unwrap_or(0);
        let next_slot = if wall_clock_slot.0 > tip_slot {
            wall_clock_slot
        } else {
            // Wall clock slot is at or behind chain tip — not our slot to forge
            debug!(
                wall_clock = wall_clock_slot.0,
                tip_slot, "Forge: wall clock slot <= tip slot, skipping"
            );
            return;
        };
        // Use epoch_nonce_for_slot to handle first slot of new epoch correctly.
        // At epoch boundaries, the TICKN transition hasn't been applied yet, so
        // ls.epoch_nonce still holds the previous epoch's nonce. epoch_nonce_for_slot
        // pre-computes the correct nonce, matching the sync path.
        let epoch_nonce = ls.epoch_nonce_for_slot(next_slot.0);
        let block_number = torsten_primitives::time::BlockNo(ls.current_block_number().0 + 1);
        let prev_hash = ls
            .tip
            .point
            .hash()
            .copied()
            .unwrap_or(torsten_primitives::hash::Hash32::ZERO);
        let slots_per_kes_period = self.consensus.slots_per_kes_period;

        // Calculate stake from the "set" snapshot (used for leader election).
        // Keep as raw u64 values to use exact rational arithmetic in the VRF check.
        let (pool_stake, total_active_stake) = if let Some(set_snapshot) = &ls.snapshots.set {
            let total_stake: u64 = set_snapshot.pool_stake.values().map(|s| s.0).sum();
            let pool_stake = set_snapshot
                .pool_stake
                .get(&creds.pool_id)
                .map(|s| s.0)
                .unwrap_or(0);
            (pool_stake, total_stake)
        } else {
            debug!(
                pool_id = %creds.pool_id,
                "Forge: skipping — no 'set' snapshot available"
            );
            (0, 0)
        };
        drop(ls);

        if pool_stake == 0 || total_active_stake == 0 {
            // Log periodically so the operator knows stake hasn't activated yet
            if next_slot.0 % 100 == 0 {
                debug!(
                    slot = next_slot.0,
                    pool_id = %creds.pool_id,
                    pool_stake = pool_stake,
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
            pool_stake,
            total_active_stake,
            self.consensus.active_slot_coeff_rational,
        );

        let relative_stake_display = if total_active_stake > 0 {
            pool_stake as f64 / total_active_stake as f64
        } else {
            0.0
        };

        if !is_leader {
            self.metrics
                .leader_checks_not_elected
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            debug!(
                slot = next_slot.0,
                pool_id = %creds.pool_id,
                stake = format_args!("{relative_stake_display:.6}"),
                "Slot leader check: not elected"
            );
            return;
        }

        info!(
            slot = next_slot.0,
            pool_id = %creds.pool_id,
            stake = format_args!("{relative_stake_display:.6}"),
            "Slot leader check: ELECTED — forging block",
        );

        // Collect transactions from mempool using protocol params limits.
        // Enforce byte-size AND execution-unit budgets so the forged block
        // stays within maxBlockBodySize and maxBlockExecutionUnits.
        let ls = self.ledger_state.read().await;
        let max_block_body_size = ls.protocol_params.max_block_body_size;
        let max_block_ex_mem = ls.protocol_params.max_block_ex_units.mem;
        let max_block_ex_steps = ls.protocol_params.max_block_ex_units.steps;
        let protocol_version_major = ls.protocol_params.protocol_version_major;
        let protocol_version_minor = ls.protocol_params.protocol_version_minor;
        let current_era = ls.era;
        drop(ls);
        let transactions = self.mempool.get_txs_for_block_with_ex_units(
            500,
            max_block_body_size as usize,
            max_block_ex_mem,
            max_block_ex_steps,
        );
        let config = crate::forge::BlockProducerConfig {
            protocol_version: torsten_primitives::block::ProtocolVersion {
                major: protocol_version_major,
                minor: protocol_version_minor,
            },
            _max_block_body_size: max_block_body_size,
            _max_txs_per_block: 500,
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
                // ── Phase 2: Submit forged block via ChainSelQueue ────────────
                //
                // Per the Haskell architecture, all blocks — including locally
                // forged ones — enter the node via the same `addBlock` path.
                // This means the ChainSelQueue receives the forged block,
                // writes it to VolatileDB, and (once chain selection is fully
                // wired in Phase 3) will determine whether the chain should
                // advance.  For now the runner returns `StoredNotAdopted` or
                // `AdoptedAsTip`; we treat both as "block is in storage" and
                // proceed with the ledger apply below.
                //
                // If no handle is available (should not happen in practice),
                // fall back to the direct ChainDB write path.
                let storage_succeeded = if let Some(ref handle) = self.chain_sel_handle {
                    // Submit via queue — cbor is moved here.
                    let result = handle
                        .submit_block(
                            *block.hash(),
                            block.slot(),
                            block.block_number(),
                            *block.prev_hash(),
                            cbor,
                        )
                        .await;
                    match result {
                        Some(torsten_storage::AddBlockResult::AdoptedAsTip)
                        | Some(torsten_storage::AddBlockResult::StoredNotAdopted)
                        | Some(torsten_storage::AddBlockResult::AlreadyKnown) => true,
                        Some(torsten_storage::AddBlockResult::SwitchedToFork {
                            intersection_hash,
                            rollback,
                            apply,
                        }) => {
                            // A fork switch was triggered by storing our own
                            // forged block.  This is theoretically impossible
                            // (a freshly-forged block extends our own tip), but
                            // handle it defensively.  The VolatileDB is already
                            // switched; log and proceed — the ledger apply below
                            // will restore consistency on the next sync round.
                            warn!(
                                intersection = %intersection_hash.to_hex(),
                                slot = next_slot.0,
                                rollback_count = rollback.len(),
                                apply_count = apply.len(),
                                "Unexpected fork switch when storing forged block"
                            );
                            true
                        }
                        Some(torsten_storage::AddBlockResult::Invalid(reason)) => {
                            // The forged block itself was rejected by storage.
                            // This is highly unusual and indicates a bug — log
                            // and trace it as `TraceDidntAdoptBlock`.
                            error!(
                                slot = next_slot.0,
                                block = block_number.0,
                                reason,
                                "TraceDidntAdoptBlock: forged block rejected by ChainSelQueue"
                            );
                            false
                        }
                        None => {
                            // Runner exited — the block was lost. Log and fail.
                            error!("ChainSelQueue runner exited unexpectedly — forged block not stored");
                            false
                        }
                    }
                } else {
                    // No ChainSelHandle (should not happen after Node::new).
                    // Fall back to direct ChainDB write to preserve correctness.
                    warn!("No ChainSelHandle available — storing forged block directly (fallback)");
                    let mut db = self.chain_db.write().await;
                    db.add_block(
                        *block.hash(),
                        block.slot(),
                        block.block_number(),
                        *block.prev_hash(),
                        cbor,
                    )
                    .is_ok()
                };

                if !storage_succeeded {
                    error!("Failed to store forged block — NOT announcing");
                    return;
                }

                // Apply to ledger with full validation.
                // Re-validate our own forged block before announcing it to peers,
                // matching Haskell cardano-node behavior. This prevents producing
                // and propagating blocks that contain invalid transactions.
                {
                    let mut ls = self.ledger_state.write().await;
                    if let Err(e) = ls.apply_block(&block, BlockValidationMode::ValidateAll) {
                        error!(
                            slot = next_slot.0,
                            block = block_number.0,
                            "Forged block failed validation — NOT announcing: {e}"
                        );
                        return;
                    }
                }

                // Update chain fragment with the new forged block header.
                // This keeps the fragment in sync with the selected chain so
                // ChainSync servers can find intersects correctly.
                {
                    let mut fragment = self.chain_fragment.write().await;
                    fragment.push(block.header.clone());
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

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use tokio::sync::RwLock;

    use torsten_primitives::block::{
        Block, BlockHeader, OperationalCert, ProtocolVersion, VrfOutput,
    };
    use torsten_primitives::era::Era;
    use torsten_primitives::hash::Hash32;
    use torsten_primitives::time::{BlockNo, SlotNo};

    use super::serve::ChainDBBlockProvider;
    use super::sync::validate_genesis_blocks;
    use crate::config::NodeConfig;

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
                nonce_vrf_output: vec![],
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
        use torsten_network::BlockProvider;
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
