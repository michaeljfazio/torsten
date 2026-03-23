//! Main Torsten node: struct definition, initialization, and run loop orchestration.
//!
//! This module owns the `Node` struct and the top-level lifecycle methods (`new`,
//! `run`).  All subsystem logic is delegated to focused sub-modules:
//!
//! - [`epoch`]  — Snapshot policy, ledger snapshot save/prune/restore
//! - [`serve`]  — N2N/N2C server adapters (BlockProvider, TxValidator, metrics bridges)
//! - [`query`]  — N2C LocalStateQuery response building (`update_query_state`)
//! - [`sync`]   — Pipelined ChainSync loop, block processing, rollback, replay

pub(crate) mod epoch;
pub(crate) mod query;
pub(crate) mod serve;
pub(crate) mod sync;

use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::signal;
use tokio::sync::{watch, RwLock};
use tracing::{debug, error, info, warn};

use torsten_consensus::OuroborosPraos;
use torsten_ledger::{BlockValidationMode, LedgerState};
use torsten_mempool::{Mempool, MempoolConfig};
use torsten_network::server::NodeServerConfig;
use torsten_network::{
    BlockFetchPool, DiffusionMode, DuplexPeerConnection, Governor, GovernorEvent, N2CServer,
    NodeServer, NodeToNodeClient, PeerManager, PeerManagerConfig, PipelinedPeerClient,
    QueryHandler, TxValidator,
};
use torsten_primitives::block::Point;
use torsten_primitives::protocol_params::ProtocolParameters;
use torsten_storage::ChainDB;

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
    /// Held to keep the N2N/N2C server tasks alive for the node's lifetime
    _server: NodeServer,
    pub(crate) query_handler: Arc<RwLock<QueryHandler>>,
    pub(crate) peer_manager: Arc<RwLock<PeerManager>>,
    pub(crate) socket_path: PathBuf,
    pub(crate) database_path: PathBuf,
    pub(crate) listen_addr: std::net::SocketAddr,
    pub(crate) network_magic: u64,
    /// Byron epoch length in absolute slots (10 * k). For correct slot
    /// computation on non-mainnet networks.
    pub(crate) byron_epoch_length: u64,
    /// Byron slot duration in milliseconds (from genesis, default 20000).
    pub(crate) byron_slot_duration_ms: u64,
    pub(crate) shelley_genesis: Option<ShelleyGenesis>,
    pub(crate) topology_path: PathBuf,
    pub(crate) metrics: Arc<crate::metrics::NodeMetrics>,
    /// Block producer credentials (None = relay-only mode)
    pub(crate) block_producer: Option<crate::forge::BlockProducerCredentials>,
    /// Broadcast sender for announcing forged blocks to connected peers
    pub(crate) block_announcement_tx:
        Option<tokio::sync::broadcast::Sender<torsten_network::BlockAnnouncement>>,
    /// Broadcast sender for notifying connected peers of chain rollbacks
    pub(crate) rollback_announcement_tx:
        Option<tokio::sync::broadcast::Sender<torsten_network::RollbackAnnouncement>>,
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
    /// Network timeout configuration (keepalive, await-reply, connection).
    pub(crate) timeout_config: torsten_network::TimeoutConfig,
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
    /// Number of consecutive peers that returned Origin as the intersection
    /// point while we had a non-trivial ledger tip.
    ///
    /// A single peer returning Origin can mean it is on a different fork, is
    /// momentarily stale, or has a corrupted chain.  Only after this counter
    /// exceeds `ORIGIN_INTERSECT_THRESHOLD` do we conclude that *we* are the
    /// divergent party and trigger a full ledger reset.  The counter is
    /// reset whenever any peer returns a non-Origin intersection.
    pub(crate) consecutive_origin_intersections: u32,
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
                        warn!(
                            snapshot_max_tx_ex_mem = snapshot_mem,
                            genesis_max_tx_ex_mem = genesis_mem,
                            "Ledger snapshot protocol params appear to predate genesis \
                             initialization (max_tx_ex_units.mem={} matches mainnet_defaults, \
                             but genesis says {}); discarding snapshot for fresh replay",
                            snapshot_mem,
                            genesis_mem,
                        );
                        Self::init_fresh_ledger(
                            &protocol_params,
                            shelley_genesis.as_ref(),
                            shelley_genesis_hash,
                            &byron_genesis_utxos,
                            network_magic,
                            byron_epoch_length,
                        )
                    } else {
                        // Validate snapshot tip is within the ChainDB's slot range.
                        // We check by hash first (exact match), then fall back to slot
                        // proximity.  Hash mismatches can occur when ImmutableDB blocks
                        // have been contaminated by an orphan fork flush — the snapshot
                        // may point to a canonical block whose hash is computed slightly
                        // differently (e.g. by pallas vs cardano-node).  As long as the
                        // snapshot slot is within the ChainDB range, the snapshot is
                        // usable; the chunk replay will handle any gap.
                        let snapshot_valid = match state.tip.point {
                            Point::Origin => true,
                            Point::Specific(snapshot_slot, ref hash) => {
                                match chain_db.try_read() {
                                    Ok(db) => {
                                        let exists = db.has_block(hash);
                                        if exists {
                                            true
                                        } else {
                                            let db_tip = db.get_tip();
                                            let db_tip_slot =
                                                db_tip.point.slot().map(|s| s.0).unwrap_or(0);
                                            if snapshot_slot.0 > db_tip_slot {
                                                // Snapshot is genuinely ahead of storage
                                                warn!(
                                                    "Ledger snapshot is ahead of ChainDB (snapshot={}, chaindb={}); \
                                                     node may have crashed before ChainDB persist — discarding snapshot, \
                                                     will replay from storage",
                                                    state.tip, db_tip,
                                                );
                                                false
                                            } else {
                                                // Snapshot slot is within ChainDB range but
                                                // hash not found (likely ImmutableDB contamination
                                                // or hash computation mismatch). Accept the
                                                // snapshot — the chunk replay will skip ahead
                                                // to the correct slot.
                                                warn!(
                                                    "Ledger snapshot hash not found in ChainDB but slot {} <= ChainDB tip {} — \
                                                     accepting snapshot (hash mismatch likely due to fork recovery)",
                                                    snapshot_slot.0, db_tip_slot,
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
            if let Some(ref creds) = block_producer {
                m.set_block_producer(&creds.pool_id.to_hex());
            }
            Arc::new(m)
        };

        Ok(Node {
            config: args.config,
            topology: args.topology,
            chain_db,
            ledger_state,
            consensus,
            mempool,
            _server: server,
            query_handler,
            peer_manager: Arc::new(RwLock::new(PeerManager::new(PeerManagerConfig::default()))),
            socket_path,
            database_path: args.database_path,
            listen_addr,
            network_magic,
            byron_epoch_length,
            byron_slot_duration_ms,
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
            timeout_config: Default::default(),
            consensus_mode: args.consensus_mode,
            validate_all_blocks: args.validate_all_blocks,
            disk_space_rx: watch::channel(crate::disk_monitor::DiskSpaceLevel::Ok).1,
            gsm,
            consecutive_origin_intersections: 0,
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
            tokio::spawn(async move {
                if let Err(e) = crate::metrics::start_metrics_server(port, metrics).await {
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

        // Start N2C server on Unix socket
        let mut n2c_server = N2CServer::new(self.query_handler.clone(), self.mempool.clone());
        let slot_config = self.ledger_state.read().await.slot_config;
        n2c_server.set_tx_validator(Arc::new(serve::LedgerTxValidator {
            ledger: self.ledger_state.clone(),
            slot_config,
            metrics: self.metrics.clone(),
            mempool: Some(self.mempool.clone()),
        }));
        n2c_server.set_block_provider(Arc::new(serve::ChainDBBlockProvider {
            chain_db: self.chain_db.clone(),
        }));
        n2c_server.set_connection_metrics(Arc::new(serve::N2CConnectionMetrics {
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
                target_hot_peers: self.config.target_number_of_active_peers,
                target_warm_peers: self
                    .config
                    .target_number_of_established_peers
                    .saturating_sub(self.config.target_number_of_active_peers),
                target_known_peers: self.config.target_number_of_known_peers,
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
        if self.topology.has_bootstrap_peers() {
            info!(
                "Bootstrap peers configured (trustable: {})",
                self.topology.has_trustable_peers()
            );
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
            for (socket_addr, trustable, advertise) in resolved_peers {
                pm.add_config_peer(socket_addr, trustable, advertise);
            }
            // Register per-group valency targets.  This must happen AFTER
            // add_config_peer() calls so the peer table contains the members.
            for (group_addrs, hot_val, warm_val) in resolved_groups {
                pm.add_local_root_group(group_addrs, hot_val, warm_val);
            }
            let stats = pm.stats();
            info!(
                known = stats.known_peers,
                local_root_groups = pm.local_root_groups().len(),
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
            Arc::new(serve::ChainDBBlockProvider {
                chain_db: self.chain_db.clone(),
            }),
            200,
            self.peer_manager.read().await.diffusion_mode() == DiffusionMode::InitiatorAndResponder,
            torsten_network::n2n_server::PeerSharingMode::PeerSharingEnabled,
        );
        n2n_server.set_mempool(self.mempool.clone());
        n2n_server.set_peer_manager(self.peer_manager.clone());
        n2n_server.set_connection_metrics(Arc::new(serve::N2NConnectionMetrics {
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

        let network_magic = self.network_magic;

        // The GSM is initialized in Node::new() and stored as self.gsm.
        // Clone the Arc here so the background evaluation task can hold a
        // reference independently of the borrow on `self`.
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
                    // Read actual tip age and ChainSync idle state from metrics.
                    // tip_age_secs: seconds since the last received block's slot time.
                    // all_idle: true when ChainSync has been idle long enough to indicate
                    // we're at the chain tip (no new blocks arriving).
                    let tip_age_secs = gsm_metrics
                        .tip_age_secs
                        .load(std::sync::atomic::Ordering::Relaxed);
                    let chainsync_idle = gsm_metrics
                        .chainsync_idle_secs
                        .load(std::sync::atomic::Ordering::Relaxed);
                    // Consider "all idle" when ChainSync has been idle for >30 seconds
                    // (indicating no new blocks arriving — we're likely at tip)
                    let all_idle = chainsync_idle > 30;
                    gsm_w.evaluate(active_blp, all_idle, tip_age_secs);
                }
            });
        }

        // Spawn the P2P governor task — periodically evaluates peer targets
        // and emits connect/disconnect/promote/demote events.
        //
        // Two timer loops run concurrently in the same task:
        //
        //  • Full evaluation (30 s): runs evaluate() + maybe_churn() to handle
        //    all deficits/surpluses, churn rotation, and BLP promotion.
        //
        //  • Warm-promotion check (2 s): runs check_warm_promotions() so that
        //    peers are promoted to Hot promptly once their WARM_DWELL_TIME
        //    (5 s) has elapsed, rather than waiting up to 30 s for the next
        //    full evaluation cycle.  This keeps peers visible as Warm in the
        //    TUI for the intended dwell period without adding latency.
        // Shared set of peer addresses managed by the sync loop.
        // The governor skips Connect events for these addresses to avoid
        // creating duplicate TCP connections. Declared here so both the
        // governor task and sync loop can access it.
        let sync_managed_peers: Arc<
            tokio::sync::RwLock<std::collections::HashSet<std::net::SocketAddr>>,
        > = Arc::new(tokio::sync::RwLock::new(std::collections::HashSet::new()));

        {
            let governor_pm = peer_manager.clone();
            let governor_shutdown = shutdown_rx.clone();
            // Capture fields needed by governor-initiated connect tasks.
            let gov_network_magic = self.network_magic;
            let gov_listen_port = self.listen_addr.port();
            let gov_metrics = self.metrics.clone();
            let gov_byron_epoch_length = self.byron_epoch_length;
            // Mempool reference for TxSubmission2 responder tasks on governor connections.
            let gov_mempool: Arc<dyn torsten_primitives::mempool::MempoolProvider> =
                self.mempool.clone();
            // Block provider for DuplexPeerConnection (held for API compatibility;
            // governor connections serve TxSubmission2 only — not ChainSync/BlockFetch).
            let gov_block_provider: Arc<dyn torsten_network::BlockProvider> =
                Arc::new(serve::ChainDBBlockProvider {
                    chain_db: self.chain_db.clone(),
                });
            let gov_config = {
                use torsten_network::{GovernorConfig, PeerTargets};
                let cfg = &self.config;
                GovernorConfig {
                    normal_targets: PeerTargets {
                        root_peers: cfg.target_number_of_root_peers,
                        known_peers: cfg.target_number_of_known_peers,
                        established_peers: cfg.target_number_of_established_peers,
                        active_peers: cfg.target_number_of_active_peers,
                        known_blp: cfg.target_number_of_known_big_ledger_peers,
                        established_blp: cfg.target_number_of_established_big_ledger_peers,
                        active_blp: cfg.target_number_of_active_big_ledger_peers,
                    },
                    ..Default::default()
                }
            };

            let gov_sync_managed = sync_managed_peers.clone();

            // Persistent map of live governor-managed DuplexPeerConnections.
            //
            // Each entry keeps the TCP session, plexer, KeepAlive loop, and
            // TxSubmission2 responder alive as long as the peer is warm/hot.
            // The governor task is the sole writer; connect tasks store entries
            // here via this Arc after a successful handshake, and the governor
            // loop removes entries when Disconnect/EvictColdPeer events fire.
            //
            // Mutex (not RwLock) because every removal needs `abort()`, which
            // requires ownership of the `DuplexPeerConnection`, and we never
            // hold the lock during async I/O.
            let gov_duplex_conns: Arc<
                tokio::sync::Mutex<
                    std::collections::HashMap<std::net::SocketAddr, DuplexPeerConnection>,
                >,
            > = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));

            tokio::spawn(async move {
                let mut governor = Governor::new(gov_config);
                // Full evaluation every 30 s; skip the first immediate tick so
                // the node has time to connect peers before the first evaluation.
                let mut full_interval = tokio::time::interval(std::time::Duration::from_secs(30));
                full_interval.tick().await; // skip first tick
                                            // Warm-promotion check every 2 s — no initial skip; we want to
                                            // pick up newly-warm peers as soon as their dwell time expires.
                let mut warm_interval = tokio::time::interval(std::time::Duration::from_secs(2));
                let mut shutdown = governor_shutdown;

                // Track in-flight governor-initiated connections so that we
                // never spawn two concurrent tasks for the same address.
                //
                // When a spawned task finishes (success or failure) it sends
                // the peer address back on this channel so we can remove it
                // from the set.  The channel is unbounded so sends never block.
                let (connect_done_tx, mut connect_done_rx) =
                    tokio::sync::mpsc::unbounded_channel::<std::net::SocketAddr>();
                // Set of addresses for which a connect task is currently running.
                let mut connecting_peers: std::collections::HashSet<std::net::SocketAddr> =
                    std::collections::HashSet::new();

                // Track in-flight peer-sharing requests so that we never send
                // more than one PeerSharing request to the same peer concurrently.
                // Mirrors the connect_done_tx/connecting_peers pattern above.
                let (peer_sharing_done_tx, mut peer_sharing_done_rx) =
                    tokio::sync::mpsc::unbounded_channel::<std::net::SocketAddr>();
                // Set of peers for which a PeerSharing request is in flight.
                let mut peer_sharing_in_flight: std::collections::HashSet<std::net::SocketAddr> =
                    std::collections::HashSet::new();
                // Maximum concurrent PeerSharing requests.  Peer sharing is
                // low-priority background work; a small cap keeps resource
                // usage bounded without starving higher-priority connects.
                const MAX_CONCURRENT_PEER_SHARING: usize = 4;
                // Maximum concurrent governor-initiated outbound connections.
                // This prevents a burst of `Connect` events from opening dozens
                // of TCP connections simultaneously.
                const MAX_CONCURRENT_GOV_CONNECTS: usize = 8;
                // TCP connect timeout for governor-initiated connections.
                const GOV_CONNECT_TIMEOUT_SECS: u64 = 5;

                loop {
                    // Collect events from whichever timer fires next, or exit
                    // immediately on shutdown.
                    let events: Vec<GovernorEvent> = tokio::select! {
                        _ = full_interval.tick() => {
                            let pm = governor_pm.read().await;
                            let mut all_events = governor.evaluate(&pm);
                            all_events.extend(governor.maybe_churn(&pm));
                            all_events
                        }
                        _ = warm_interval.tick() => {
                            // Fast-path warm promotion: run a lightweight
                            // evaluate to promote dwell-eligible warm peers.
                            let pm = governor_pm.read().await;
                            governor.evaluate(&pm)
                        }
                        _ = shutdown.changed() => { break; }
                    };

                    // Drain completed-connection notifications before processing
                    // new events, so that connect slots are freed as soon as
                    // possible and the per-peer duplicate guard stays accurate.
                    while let Ok(addr) = connect_done_rx.try_recv() {
                        connecting_peers.remove(&addr);
                    }
                    // Drain completed peer-sharing tasks before processing new events.
                    while let Ok(addr) = peer_sharing_done_rx.try_recv() {
                        peer_sharing_in_flight.remove(&addr);
                    }

                    // Apply events to the peer manager.
                    if !events.is_empty() {
                        // Collect Connect events that need spawning and addresses
                        // that need teardown before acquiring the write lock, so
                        // we can check circuit state and temperature under the
                        // lock and then release it before doing async I/O.
                        let mut addrs_to_connect: Vec<std::net::SocketAddr> = Vec::new();
                        let mut addrs_to_disconnect: Vec<std::net::SocketAddr> = Vec::new();
                        // Collect peer-sharing requests that need spawning.
                        let mut addrs_to_share: Vec<(std::net::SocketAddr, u8)> = Vec::new();

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
                                    // Tear down the persistent duplex connection, if
                                    // one exists.  Aborting the connection shuts down
                                    // the plexer and TxSubmission2 responder task.
                                    addrs_to_disconnect.push(*addr);
                                }
                                GovernorEvent::EvictColdPeer(addr) => {
                                    pm.peer_disconnected(addr);
                                    addrs_to_disconnect.push(*addr);
                                }
                                GovernorEvent::Connect(addr) => {
                                    // Check if we already have an inbound duplex
                                    // connection from this IP.  If so, skip the
                                    // outbound connection — the existing inbound
                                    // connection already provides bidirectional
                                    // mini-protocol support.
                                    if let Some(existing) = pm.find_inbound_duplex_by_ip(addr.ip())
                                    {
                                        debug!(
                                            %addr,
                                            existing = %existing,
                                            "Governor: skipping outbound connect — \
                                             inbound duplex connection exists"
                                        );
                                        continue;
                                    }
                                    // Skip if sync loop already manages this peer.
                                    // Prevents duplicate TCP connections per peer.
                                    if gov_sync_managed.read().await.contains(addr) {
                                        debug!(
                                            %addr,
                                            "Governor: skipping Connect — peer managed by sync loop"
                                        );
                                        continue;
                                    }
                                    // Skip if we already have an in-flight
                                    // connect attempt for this address.
                                    if connecting_peers.contains(addr) {
                                        continue;
                                    }
                                    // Skip if the connection cap is reached.
                                    if connecting_peers.len() >= MAX_CONCURRENT_GOV_CONNECTS {
                                        debug!(
                                            %addr,
                                            in_flight = connecting_peers.len(),
                                            "Governor: Connect skipped — concurrent limit reached"
                                        );
                                        continue;
                                    }
                                    // Skip if the circuit breaker is open for
                                    // this peer (transitions Open→HalfOpen as a
                                    // side-effect when the cooldown has expired).
                                    if !pm.should_attempt_connection(addr) {
                                        continue;
                                    }
                                    // Skip peers that are already warm or hot —
                                    // they are already connected.
                                    if pm.connected_peer_addrs().contains(addr) {
                                        continue;
                                    }
                                    connecting_peers.insert(*addr);
                                    addrs_to_connect.push(*addr);
                                }
                                GovernorEvent::RequestPeerSharing(addr, count) => {
                                    // Skip if a PeerSharing request is already in flight
                                    // to this peer (avoid duplicate concurrent requests).
                                    if peer_sharing_in_flight.contains(addr) {
                                        continue;
                                    }
                                    // Enforce concurrency cap so that a large batch of
                                    // sharing events doesn't open many connections at once.
                                    if peer_sharing_in_flight.len() >= MAX_CONCURRENT_PEER_SHARING {
                                        debug!(
                                            %addr,
                                            in_flight = peer_sharing_in_flight.len(),
                                            "Governor: PeerSharing skipped — concurrent limit reached"
                                        );
                                        continue;
                                    }
                                    peer_sharing_in_flight.insert(*addr);
                                    addrs_to_share.push((*addr, *count));
                                }
                            }
                        }
                        pm.recompute_reputations();
                        // Release the write lock before spawning async tasks.
                        drop(pm);

                        // Tear down connections for peers that were
                        // Disconnect/EvictColdPeer'd in this cycle.
                        if !addrs_to_disconnect.is_empty() {
                            let mut conns = gov_duplex_conns.lock().await;
                            for addr in addrs_to_disconnect {
                                if let Some(conn) = conns.remove(&addr) {
                                    debug!(%addr, "Governor: aborting duplex connection on disconnect");
                                    conn.abort().await;
                                }
                            }
                        }

                        // Spawn one fire-and-forget connect task per address.
                        // Each task:
                        //   1. Attempts TCP + Ouroboros handshake with a 5 s timeout
                        //      using DuplexPeerConnection (InitiatorAndResponder mode).
                        //   2. On success: marks the peer Warm in the peer manager,
                        //      records RTT, and stores the live connection in
                        //      gov_duplex_conns so TxSubmission2 keeps running.
                        //   3. On failure: calls peer_failed() to update the circuit
                        //      breaker so the governor backs off future attempts.
                        //   4. Always sends the address back on connect_done_tx so the
                        //      outer loop removes it from connecting_peers.
                        for addr in addrs_to_connect {
                            let task_pm = governor_pm.clone();
                            let task_metrics = gov_metrics.clone();
                            let task_magic = gov_network_magic;
                            let task_byron = gov_byron_epoch_length;
                            let task_done_tx = connect_done_tx.clone();
                            let task_mempool = gov_mempool.clone();
                            let _task_block_provider = gov_block_provider.clone();
                            let task_duplex_conns = gov_duplex_conns.clone();
                            let task_listen_port = gov_listen_port;
                            let task_sync_managed = gov_sync_managed.clone();
                            tokio::spawn(async move {
                                // Re-check sync-managed and existing duplex connections
                                // right before connecting to avoid races.
                                if task_sync_managed.read().await.contains(&addr) {
                                    debug!(
                                        %addr,
                                        "Governor: aborting connect — peer now sync-managed"
                                    );
                                    let _ = task_done_tx.send(addr);
                                    return;
                                }
                                if task_duplex_conns.lock().await.contains_key(&addr) {
                                    debug!(
                                        %addr,
                                        "Governor: aborting connect — duplex connection already exists"
                                    );
                                    let _ = task_done_tx.send(addr);
                                    return;
                                }
                                let target = addr.to_string();
                                debug!(%addr, "Governor: initiating outbound duplex connection");
                                let connect_start = std::time::Instant::now();
                                let connect_result = tokio::time::timeout(
                                    std::time::Duration::from_secs(GOV_CONNECT_TIMEOUT_SECS),
                                    // Governor connections use connect_no_chainsync to avoid
                                    // demuxer stall: subscribing to ChainSync/BlockFetch without
                                    // reading fills the bounded channel, blocking the demuxer
                                    // and killing all protocols including KeepAlive.
                                    DuplexPeerConnection::connect_no_chainsync(
                                        &*target,
                                        task_magic,
                                        task_mempool,
                                        task_listen_port,
                                    ),
                                )
                                .await
                                .unwrap_or_else(|_| {
                                    Err(torsten_network::DuplexError::Connection(format!(
                                        "{target}: connection timed out after {GOV_CONNECT_TIMEOUT_SECS}s"
                                    )))
                                });
                                match connect_result {
                                    Ok(mut conn) => {
                                        conn.set_byron_epoch_length(task_byron);
                                        let rtt_ms = connect_start.elapsed().as_secs_f64() * 1000.0;
                                        task_metrics.record_handshake_rtt(rtt_ms);
                                        let mut pm = task_pm.write().await;
                                        // Land in Warm; the governor's 2 s warm-promotion
                                        // check will promote to Hot after WARM_DWELL_TIME.
                                        pm.peer_connected(
                                            &addr,
                                            14,
                                            torsten_network::ConnectionDirection::Outbound,
                                        );
                                        pm.record_handshake_rtt(&addr, rtt_ms);
                                        // Mark as duplex — governor connections use
                                        // DuplexPeerConnection (InitiatorAndResponder).
                                        pm.mark_peer_duplex(&addr);
                                        drop(pm);
                                        info!(
                                            peer = %target,
                                            rtt_ms = format_args!("{rtt_ms:.0}"),
                                            "Governor: peer connected (warm, TxSubmission2 active)"
                                        );
                                        // Store the live connection so the plexer, KeepAlive
                                        // loop, and TxSubmission2 responder task remain alive
                                        // for as long as the peer is warm/hot.  The governor
                                        // loop removes this entry on Disconnect/EvictColdPeer.
                                        task_duplex_conns.lock().await.insert(addr, conn);
                                    }
                                    Err(e) => {
                                        task_pm.write().await.peer_failed(&addr);
                                        debug!(%addr, "Governor: failed to connect — {e}");
                                    }
                                }
                                // Always notify the outer loop that this slot is free.
                                let _ = task_done_tx.send(addr);
                            });
                        }

                        // Spawn one fire-and-forget peer-sharing task per address.
                        //
                        // Each task:
                        //   1. Opens a fresh N2N connection to the target peer.
                        //   2. Performs Ouroboros handshake.
                        //   3. Sends MsgShareRequest(count) on the PeerSharing channel.
                        //   4. Reads MsgSharePeers and adds each returned address as a
                        //      cold peer via PeerManager::add_shared_peer (non-routable
                        //      addresses are silently rejected inside that method).
                        //   5. Sends MsgDone and disconnects.
                        //   6. Always notifies the outer loop so the slot is freed.
                        //
                        // The connection opened here is solely for peer discovery;
                        // it does not interact with ChainSync or BlockFetch state.
                        for (addr, count) in addrs_to_share {
                            let task_pm = governor_pm.clone();
                            let task_magic = gov_network_magic;
                            let task_done_tx = peer_sharing_done_tx.clone();
                            tokio::spawn(async move {
                                debug!(
                                    %addr,
                                    count,
                                    "Governor: initiating PeerSharing request"
                                );
                                // 60 s total timeout for the entire PeerSharing exchange
                                // (connect + handshake + request/response round trip).
                                const PEER_SHARING_TIMEOUT_SECS: u64 = 60;
                                let result = tokio::time::timeout(
                                    std::time::Duration::from_secs(PEER_SHARING_TIMEOUT_SECS),
                                    torsten_network::request_peers_from(addr, task_magic, count),
                                )
                                .await;

                                match result {
                                    Ok(Ok(peers)) if !peers.is_empty() => {
                                        let discovered = peers.len();
                                        let mut pm = task_pm.write().await;
                                        for peer_addr in peers {
                                            pm.add_shared_peer(peer_addr);
                                        }
                                        drop(pm);
                                        info!(
                                            %addr,
                                            discovered,
                                            "PeerSharing: added peers from share response"
                                        );
                                    }
                                    Ok(Ok(_)) => {
                                        // Empty response — peer returned no addresses.
                                        debug!(%addr, "PeerSharing: peer returned no addresses");
                                    }
                                    Ok(Err(e)) => {
                                        debug!(%addr, "PeerSharing: request failed — {e}");
                                    }
                                    Err(_) => {
                                        debug!(
                                            %addr,
                                            timeout_secs = PEER_SHARING_TIMEOUT_SECS,
                                            "PeerSharing: request timed out"
                                        );
                                    }
                                }
                                // Always notify the outer loop that this slot is free.
                                let _ = task_done_tx.send(addr);
                            });
                        }
                    }
                }

                // Node is shutting down — abort all remaining governor connections.
                let mut conns = gov_duplex_conns.lock().await;
                for (addr, conn) in conns.drain() {
                    debug!(%addr, "Governor: aborting duplex connection on shutdown");
                    conn.abort().await;
                }
            });
        }

        // Main connection loop — connect to peers and sync
        //
        // Backoff parameters are intentionally short so that after a network
        // outage (e.g., sleep/hibernate, router restart) the node reconnects
        // within seconds rather than minutes.  The exponential schedule is:
        //   retry 1:  2 * 2^1 =  4 s
        //   retry 2:  2 * 2^2 =  8 s
        //   retry 3:  2 * 2^3 = 16 s
        //   retry 4+: clamped to 20 s
        // Maximum total reconnect time (4 retries, 4+8+16+20 s) < 60 s.
        let mut retry_count = 0u32;
        let base_delay_secs = 2u64;
        let max_delay_secs = 20u64;

        // Per-peer TCP connect timeout.  Keeping this tight (5 s) ensures
        // that a topology list with several unreachable peers does not add
        // more than ~5 s per peer before we move on to the next candidate.
        // The OS default can be 20-120 s depending on the platform.
        let connect_timeout = std::time::Duration::from_secs(5);

        loop {
            if *shutdown_rx.borrow() {
                break;
            }

            // Get peers to connect to from the peer manager.
            // During PreSyncing/Syncing (Genesis), prefer BLP peers for block download.
            let targets: Vec<std::net::SocketAddr> = {
                let pm = peer_manager.read().await;
                let gsm_state = self.gsm.read().await.state();
                let mut peers = pm.peers_to_connect();
                if gsm_state != crate::gsm::GenesisSyncState::CaughtUp {
                    // Sort BLPs first for Genesis-mode sync
                    peers.sort_by_key(|addr| {
                        if pm.peer_category(addr)
                            == Some(torsten_network::PeerCategory::BigLedgerPeer)
                        {
                            0 // BLPs first
                        } else {
                            1
                        }
                    });
                }
                peers
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
                        r = tokio::time::timeout(
                            connect_timeout,
                            NodeToNodeClient::connect(&*target, network_magic),
                        ) => r.unwrap_or_else(|_| Err(torsten_network::ClientError::Connection(
                            format!("{target}: connection timed out after {}s", connect_timeout.as_secs()),
                        ))),
                        _ = shutdown_rx.changed() => break,
                    };
                    match connect_result {
                        Ok(mut c) => {
                            c.set_byron_epoch_length(self.byron_epoch_length);
                            let rtt_ms = connect_start.elapsed().as_secs_f64() * 1000.0;
                            self.metrics.record_handshake_rtt(rtt_ms);
                            let mut pm = peer_manager.write().await;
                            // Transition peer to Warm.  Promotion to Hot is deferred
                            // to the governor's warm-promotion check (every 2 s) so
                            // that the peer is visible as Warm in the TUI for at
                            // least WARM_DWELL_TIME (5 s) before jumping to Hot.
                            pm.peer_connected(
                                addr,
                                14,
                                torsten_network::ConnectionDirection::Outbound,
                            );
                            pm.record_handshake_rtt(addr, rtt_ms);
                            drop(pm);
                            info!(peer = %target, rtt_ms = format_args!("{rtt_ms:.0}"), "Peer connected (warm, dwell pending)");
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
                        r = tokio::time::timeout(
                            connect_timeout,
                            NodeToNodeClient::connect(&*target, network_magic),
                        ) => r.unwrap_or_else(|_| Err(torsten_network::ClientError::Connection(
                            format!("{target}: connection timed out after {}s", connect_timeout.as_secs()),
                        ))),
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
                    // Register this peer as sync-managed so the governor
                    // doesn't create a duplicate connection.
                    sync_managed_peers.write().await.insert(c.1);
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
                    // Use the same 5s TCP connect timeout as the primary peer
                    // to avoid 20-120s stalls per unreachable fetcher candidate.
                    let connect_result = tokio::select! {
                        r = tokio::time::timeout(
                            connect_timeout,
                            NodeToNodeClient::connect(&*target, network_magic),
                        ) => r.unwrap_or_else(|_| Err(torsten_network::ClientError::Connection(
                            format!("{target}: fetcher connection timed out after {}s", connect_timeout.as_secs()),
                        ))),
                        _ = shutdown_rx.changed() => break,
                    };
                    match connect_result {
                        Ok(mut c) => {
                            c.set_byron_epoch_length(self.byron_epoch_length);
                            let rtt_ms = connect_start.elapsed().as_secs_f64() * 1000.0;
                            self.metrics.record_handshake_rtt(rtt_ms);
                            let mut pm = peer_manager.write().await;
                            // Same deferral as the primary peer: land in Warm
                            // first and let the governor promote after dwell time.
                            pm.peer_connected(
                                addr,
                                14,
                                torsten_network::ConnectionDirection::Outbound,
                            );
                            pm.record_handshake_rtt(addr, rtt_ms);
                            drop(pm);
                            debug!("Connected block fetcher to {target} ({rtt_ms:.0}ms, dwell pending)");
                            fetch_pool.add_fetcher(c);
                        }
                        Err(e) => {
                            peer_manager.write().await.peer_failed(addr);
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
                        r = tokio::time::timeout(
                            connect_timeout,
                            NodeToNodeClient::connect(&*target, network_magic),
                        ) => r.unwrap_or_else(|_| Err(torsten_network::ClientError::Connection(
                            format!("{target}: dedicated fetcher connection timed out after {}s", connect_timeout.as_secs()),
                        ))),
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

            // Create pipelined ChainSync connection to the primary peer for
            // high-throughput headers.
            //
            // Phase 3 — full-duplex upgrade:
            //   We first attempt `DuplexPeerConnection::connect()`, which
            //   negotiates InitiatorAndResponder mode and starts a background
            //   TxSubmission2 responder so the peer can pull our mempool txs.
            //   On success the connection is converted to a `PipelinedPeerClient`
            //   (same ChainSync/BlockFetch channels) and the peer manager records
            //   the duplex flag.
            //
            //   If the duplex connect fails (e.g. the peer only supports
            //   InitiatorOnly mode or a transient TCP error), we fall back to a
            //   plain `PipelinedPeerClient::connect()` so sync is never blocked by
            //   the duplex upgrade attempt.
            if *shutdown_rx.borrow() {
                break;
            }

            // Both TxSubmission2 task handles must be kept alive for the duration
            // of the sync session: dropping either handle would abort the
            // corresponding background task prematurely.
            // Declared here (outside the block) so they live as long as `pipelined_client`.
            let mut _txsub_responder_handle: Option<tokio::task::JoinHandle<()>> = None;
            let mut _txsub_initiator_handle: Option<tokio::task::JoinHandle<()>> = None;

            let pipelined_client = {
                let target = peer_addr.to_string();
                let mempool_for_duplex = self.mempool.clone();
                // Build a block_provider for DuplexPeerConnection (currently unused
                // inside duplex — kept for future ChainSync/BlockFetch server tasks).
                let block_provider_for_duplex: Arc<dyn torsten_network::BlockProvider> =
                    Arc::new(serve::ChainDBBlockProvider {
                        chain_db: self.chain_db.clone(),
                    });

                // ── Attempt full-duplex connection ───────────────────────────
                let duplex_result = tokio::select! {
                    r = tokio::time::timeout(
                        connect_timeout,
                        DuplexPeerConnection::connect(
                            &*target,
                            network_magic,
                            mempool_for_duplex,
                            block_provider_for_duplex,
                            self.listen_addr.port(),
                        ),
                    ) => r.unwrap_or_else(|_| Err(torsten_network::DuplexError::Connection(
                        format!("{target}: duplex connect timed out after {}s", connect_timeout.as_secs()),
                    ))),
                    _ = shutdown_rx.changed() => { break; }
                };

                match duplex_result {
                    Ok(duplex_conn) => {
                        // Convert DuplexPeerConnection → PipelinedPeerClient.
                        // Both TxSubmission2 task handles are kept alive in the outer scope.
                        //   responder_handle: serves our mempool to the remote peer
                        //   initiator_handle: pulls the remote peer's mempool txs into ours
                        info!(peer = %target, "Full-duplex connection established (InitiatorAndResponder, TxSub client+server)");
                        let (mut pc, responder_handle, initiator_handle) =
                            duplex_conn.into_pipelined();
                        pc.set_byron_epoch_length(self.byron_epoch_length);
                        pc.set_await_reply_timeout(self.timeout_config.await_reply_timeout());

                        // Record the duplex flag in the peer manager so the
                        // TUI and metrics can surface it.
                        {
                            let mut pm = peer_manager.write().await;
                            pm.mark_peer_duplex(&peer_addr);
                        }
                        // Update the Prometheus duplex peer gauge.
                        {
                            let pm = peer_manager.read().await;
                            self.metrics.peers_duplex.store(
                                pm.duplex_peer_count() as u64,
                                std::sync::atomic::Ordering::Relaxed,
                            );
                        }

                        // Keep both task handles alive until `pipelined_client` is dropped.
                        _txsub_responder_handle = Some(responder_handle);
                        _txsub_initiator_handle = Some(initiator_handle);

                        Some(pc)
                    }
                    Err(duplex_err) => {
                        // Duplex upgrade failed — fall back to plain pipelined connection.
                        // This is non-fatal: the peer may only support InitiatorOnly mode.
                        debug!(peer = %target, "Duplex connect failed ({duplex_err}), falling back to InitiatorOnly pipelined client");

                        let connect_result = tokio::select! {
                            r = PipelinedPeerClient::connect(&*target, network_magic) => r,
                            _ = shutdown_rx.changed() => { break; }
                        };
                        match connect_result {
                            Ok(mut pc) => {
                                pc.set_byron_epoch_length(self.byron_epoch_length);
                                pc.set_await_reply_timeout(
                                    self.timeout_config.await_reply_timeout(),
                                );
                                debug!("Pipelined ChainSync client connected to {target} (InitiatorOnly fallback)");

                                // Spawn a TxSubmission2 CLIENT on the fallback connection
                                // so that we receive mempool txs from the peer.
                                // (On the duplex path the peer receives OUR txs instead.)
                                if let Some(txsub_channel) = pc.take_txsub_channel() {
                                    let mempool = self.mempool.clone();
                                    let ledger = self.ledger_state.clone();
                                    let slot_config = self.ledger_state.read().await.slot_config;
                                    let shutdown = shutdown_rx.clone();
                                    let txsub_metrics = self.metrics.clone();
                                    tokio::spawn(async move {
                                        let validator: Option<Arc<dyn TxValidator>> =
                                            Some(Arc::new(serve::LedgerTxValidator {
                                                ledger,
                                                slot_config,
                                                metrics: txsub_metrics,
                                                mempool: None, // N2N TxSubmission doesn't need chaining
                                            }));
                                        let mut client =
                                            torsten_network::TxSubmissionClient::new(txsub_channel);
                                        let mut shutdown = shutdown;
                                        tokio::select! {
                                            result = client.run(mempool, validator) => {
                                                match result {
                                                    Ok(stats) => {
                                                        debug!(
                                                            "TxSubmission2 session ended \
                                                             (rx={}, ok={}, rej={}, dup={})",
                                                            stats.received, stats.accepted,
                                                            stats.rejected, stats.duplicate,
                                                        );
                                                    }
                                                    Err(e) => {
                                                        debug!("TxSubmission2 client error: {e}");
                                                    }
                                                }
                                                // Keep the channel alive until the connection closes
                                                // so the demuxer doesn't crash on delayed responses.
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
                    // Refresh the duplex peer count after disconnection
                    // (peer_disconnected clears the duplex flag).
                    {
                        let pm = peer_manager.read().await;
                        self.metrics.peers_duplex.store(
                            pm.duplex_peer_count() as u64,
                            std::sync::atomic::Ordering::Relaxed,
                        );
                    }
                    if *shutdown_rx.borrow() {
                        break;
                    }
                    // Unregister from sync-managed so governor can connect.
                    sync_managed_peers.write().await.remove(&peer_addr);
                    info!("Peer disconnected, reconnecting...");
                }
                Err(e) => {
                    // Unregister from sync-managed so governor can connect.
                    sync_managed_peers.write().await.remove(&peer_addr);
                    // Mark as failed (not just disconnected) so PeerManager
                    // deprioritizes this peer on the next connection attempt.
                    // This is important after sleep/hibernate where stale peers
                    // should be avoided in favor of responsive ones.
                    peer_manager.write().await.peer_failed(&peer_addr);
                    // Refresh the duplex peer count after a failed sync session.
                    // peer_failed does not clear the duplex flag directly, but we
                    // know the connection is gone so count what the manager reports.
                    {
                        let pm = peer_manager.read().await;
                        self.metrics.peers_duplex.store(
                            pm.duplex_peer_count() as u64,
                            std::sync::atomic::Ordering::Relaxed,
                        );
                    }
                    warn!("Sync error: {e}, will reconnect...");
                }
            }

            // Brief delay before reconnecting — short enough that a transient
            // disconnect (e.g., peer restart) is recovered in well under 5 s.
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
                _ = shutdown_rx.changed() => { break; }
            }
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
