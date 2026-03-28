mod config;
mod disk_monitor;
// forge is declared in lib.rs and re-used here via the crate root
use torsten_node::forge;
mod genesis;
mod gsm;
mod logging;
mod metrics;
mod mithril;
mod node;
mod startup;
mod topology;

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use tracing::info;

/// Torsten - A Rust implementation of the Cardano node
#[derive(Parser, Debug)]
#[command(name = "torsten-node", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    /// Run the node
    Run(Box<RunArgs>),
    /// Import a Mithril snapshot for fast initial sync
    MithrilImport(MithrilImportArgs),
    /// Dump ledger state at epoch boundaries (for cross-validation with cardano-streamer)
    DumpSnapshot(DumpSnapshotArgs),
    /// Database inspection and maintenance tools
    Db(DbArgs),
}

#[derive(clap::Args, Debug)]
struct DbArgs {
    #[command(subcommand)]
    command: DbCommand,
}

#[derive(clap::Subcommand, Debug)]
enum DbCommand {
    /// Show database size and block count information
    Info(DbInfoArgs),
}

#[derive(clap::Args, Debug)]
struct DbInfoArgs {
    /// Path to the database directory
    #[arg(long, default_value = "db")]
    database_path: PathBuf,

    /// Storage profile: ultra-memory, high-memory (default), low-memory, or minimal
    #[arg(long, default_value = "high-memory")]
    storage_profile: String,
}

/// Shared logging arguments for all subcommands
#[derive(clap::Args, Debug, Clone)]
struct LogArgs {
    /// Log output targets: stdout, file, journald (can specify multiple)
    #[arg(long = "log-output", default_value = "stdout")]
    log_outputs: Vec<String>,

    /// Log level (trace, debug, info, warn, error). Overridden by RUST_LOG env var.
    #[arg(long)]
    log_level: Option<String>,

    /// Directory for log files (used with --log-output file)
    #[arg(long, default_value = "logs")]
    log_dir: PathBuf,

    /// Log output format: text (human-readable) or json (structured)
    #[arg(long, default_value = "text")]
    log_format: String,

    /// Log file rotation strategy: daily, hourly, never
    #[arg(long, default_value = "daily")]
    log_file_rotation: String,

    /// Disable ANSI colors in stdout output
    #[arg(long)]
    log_no_color: bool,

    /// Number of days to retain log files (default: 7)
    #[arg(long, default_value = "7")]
    log_retention_days: u64,
}

#[derive(clap::Args, Debug)]
struct RunArgs {
    /// Path to the node configuration file
    #[arg(long, default_value = "config/mainnet-config.json")]
    config: PathBuf,

    /// Path to the topology file
    #[arg(long, default_value = "config/mainnet-topology.json")]
    topology: PathBuf,

    /// Path to the database directory
    #[arg(long, default_value = "db")]
    database_path: PathBuf,

    /// Unix domain socket path for local clients
    #[arg(long, default_value = "node.sock")]
    socket_path: PathBuf,

    /// TCP port for node-to-node connections
    #[arg(long, default_value = "3001")]
    port: u16,

    /// Host address to bind to
    #[arg(long, default_value = "0.0.0.0")]
    host_addr: String,

    /// Prometheus metrics port.
    ///
    /// Overrides the MetricsPort value from the config file.
    /// Pass 0 to disable the metrics server.
    /// If not specified, the config file value is used; if neither is set,
    /// the default port 12798 is used.
    #[arg(long)]
    metrics_port: Option<u16>,

    /// Disable the Prometheus metrics server entirely.
    ///
    /// Equivalent to `--metrics-port 0`. Takes precedence over `--metrics-port`
    /// and the MetricsPort config file field.
    #[arg(long)]
    no_metrics: bool,

    /// Also emit `cardano_node_metrics_*` compatibility aliases in the Prometheus
    /// output alongside the native `torsten_*` metrics.
    ///
    /// Enables reuse of existing cardano-node Grafana dashboards without
    /// modification.  Disabled by default to avoid polluting the metrics
    /// namespace for operators who do not need it.
    #[arg(long)]
    compat_metrics: bool,

    /// Maximum number of transactions in the mempool
    #[arg(long, default_value = "16384")]
    mempool_max_tx: usize,

    /// Maximum mempool size in bytes
    #[arg(long, default_value = "536870912")]
    mempool_max_bytes: usize,

    /// Maximum number of ledger snapshots to retain on disk
    #[arg(long, default_value = "2")]
    snapshot_max_retained: usize,

    /// Minimum blocks between bulk-sync snapshots
    #[arg(long, default_value = "50000")]
    snapshot_bulk_min_blocks: u64,

    /// Minimum seconds between bulk-sync snapshots
    #[arg(long, default_value = "360")]
    snapshot_bulk_min_secs: u64,

    /// Storage profile: ultra-memory (32GB), high-memory (16GB, default), low-memory (8GB), or minimal (4GB)
    #[arg(long, default_value = "high-memory")]
    storage_profile: String,

    /// Override: block index type (in-memory or mmap)
    #[arg(long)]
    immutable_index_type: Option<String>,

    /// Override: UTxO backend (in-memory or lsm)
    #[arg(long)]
    utxo_backend: Option<String>,

    /// Override: LSM memtable size in MB
    #[arg(long)]
    utxo_memtable_size_mb: Option<u64>,

    /// Override: LSM block cache size in MB
    #[arg(long)]
    utxo_block_cache_size_mb: Option<u64>,

    /// Override: LSM bloom filter bits per key
    #[arg(long)]
    utxo_bloom_filter_bits: Option<u32>,

    /// Consensus mode: praos (default) or genesis (enables genesis bootstrap from empty DB)
    #[arg(long, default_value = "praos")]
    consensus_mode: String,

    /// Force full Phase-2 Plutus validation on all blocks, even during initial sync.
    /// Normally only blocks at tip are fully validated; this enables paranoid/auditing mode.
    #[arg(long)]
    validate_all_blocks: bool,

    // Block producer options (optional — enables block production mode)
    /// Path to the KES signing key file
    #[arg(long)]
    shelley_kes_key: Option<PathBuf>,

    /// Path to the VRF signing key file
    #[arg(long)]
    shelley_vrf_key: Option<PathBuf>,

    /// Path to the operational certificate file
    #[arg(long)]
    shelley_operational_certificate: Option<PathBuf>,

    /// Path to the cold signing key file (for pool ID derivation)
    #[arg(long)]
    shelley_cold_key: Option<PathBuf>,

    #[command(flatten)]
    log: LogArgs,
}

#[derive(clap::Args, Debug)]
struct MithrilImportArgs {
    /// Network magic value (764824073=mainnet, 2=preview, 1=preprod)
    #[arg(long, default_value = "764824073")]
    network_magic: u64,

    /// Path to the database directory
    #[arg(long, default_value = "db")]
    database_path: PathBuf,

    /// Temporary directory for download and extraction
    #[arg(long)]
    temp_dir: Option<PathBuf>,

    #[command(flatten)]
    log: LogArgs,
}

#[derive(clap::Args, Debug)]
struct DumpSnapshotArgs {
    /// Path to the node configuration file
    #[arg(long)]
    config: PathBuf,

    /// Path to the database directory (must contain immutable/ chunk files)
    #[arg(long, default_value = "db")]
    database_path: PathBuf,

    /// Stop replaying at this slot (dump state at the epoch boundary at or before this slot).
    /// If omitted, replays the entire chain and dumps at every epoch boundary.
    #[arg(long)]
    stop_slot: Option<u64>,

    /// Output file path for JSON dumps. Each epoch's state is one JSON object per line.
    /// Defaults to stdout if not specified.
    #[arg(long)]
    output: Option<PathBuf>,

    /// Output directory for per-epoch JSON files. If set, writes one {epoch}.json
    /// file per epoch instead of NDJSON to --output/stdout.
    #[arg(long)]
    output_dir: Option<PathBuf>,

    #[command(flatten)]
    log: LogArgs,
}

fn build_logging_opts(log: &LogArgs) -> Result<logging::LoggingOpts> {
    let outputs: Result<Vec<logging::LogOutput>, _> =
        log.log_outputs.iter().map(|s| s.parse()).collect();
    let outputs = outputs.map_err(|e| anyhow::anyhow!(e))?;

    let format: logging::LogFormat = log
        .log_format
        .parse()
        .map_err(|e: String| anyhow::anyhow!(e))?;

    let rotation: logging::LogRotation = log
        .log_file_rotation
        .parse()
        .map_err(|e: String| anyhow::anyhow!(e))?;

    Ok(logging::LoggingOpts {
        outputs,
        format,
        level: log.log_level.clone().unwrap_or_else(|| "info".to_string()),
        log_dir: log.log_dir.to_string_lossy().into_owned(),
        rotation,
        no_color: log.log_no_color,
        _log_retention_days: log.log_retention_days,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    // Install a panic hook that writes a structured message to stderr *and*
    // emits a tracing ERROR event before the process aborts.
    //
    // The release profile uses `panic = "abort"` which normally kills the
    // process immediately — bypassing any buffered log output — making silent
    // crashes extremely difficult to diagnose. This hook ensures that at
    // minimum the panic location and message are written to stderr, and gives
    // the tracing subscriber a brief window to flush its internal buffer.
    std::panic::set_hook(Box::new(|info| {
        // Always write to stderr directly (bypasses any log buffering).
        eprintln!("PANIC: {info}");

        // Also emit through tracing so the message appears in structured log
        // files / journald / file appenders if they are still live.
        tracing::error!(panic_info = %info, "Node panicked — aborting");

        // Give the subscriber a brief window to flush its internal buffer.
        // We cannot call `shutdown_tracer()` here because the subscriber is not
        // guaranteed to be a TracingSubscriber, and `tracing` itself does not
        // expose a flush primitive. A short sleep is a best-effort approach;
        // the subsequent `panic=abort` will terminate the process regardless.
        std::thread::sleep(std::time::Duration::from_millis(50));
    }));

    let cli = Cli::parse();

    // Extract log args and initialize logging before any work
    let log_args = match &cli.command {
        Command::Run(ref args) => Some(&args.log),
        Command::MithrilImport(ref args) => Some(&args.log),
        Command::DumpSnapshot(ref args) => Some(&args.log),
        Command::Db(_) => None,
    };
    let _log_guard = if let Some(log_args) = log_args {
        Some(logging::init(&build_logging_opts(log_args)?)?)
    } else {
        None
    };

    match cli.command {
        Command::Run(args) => run_node(*args).await,
        Command::MithrilImport(args) => run_mithril_import(args).await,
        Command::DumpSnapshot(args) => run_dump_snapshot(args).await,
        Command::Db(args) => run_db_command(args).await,
    }
}

/// Replay blocks from ImmutableDB and dump ledger state at epoch boundaries.
///
/// Produces JSON output compatible with cardano-streamer's `dump-snapshot`
/// format for cross-validation of epoch fees, reserves, treasury, and
/// stake distribution.
async fn run_dump_snapshot(args: DumpSnapshotArgs) -> Result<()> {
    use std::io::Write;

    info!(
        config = %args.config.display(),
        database_path = %args.database_path.display(),
        stop_slot = ?args.stop_slot,
        "dump-snapshot: starting epoch-by-epoch ledger state dump"
    );

    // Load node config
    let node_config = config::NodeConfig::load(&args.config)?;
    let config_dir = args
        .config
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf();
    node_config.validate(&config_dir)?;

    // Load genesis files and build protocol parameters (same as Node::new)
    let mut protocol_params =
        torsten_primitives::protocol_params::ProtocolParameters::mainnet_defaults();

    let mut byron_epoch_length: u64 = 0;
    let mut byron_initial_funds: u64 = 0;
    if let Some(ref genesis_path) = node_config.byron_genesis_file {
        let genesis_path = config_dir.join(genesis_path);
        if let Ok((genesis, _hash)) = genesis::ByronGenesis::load_with_hash(&genesis_path) {
            let k = genesis.security_param();
            byron_epoch_length = 10 * k;
            // Sum initial fund distribution (nonAvvmBalances + avvmDistr)
            // These funds are distributed at genesis and must be subtracted
            // from reserves to match the Haskell reference implementation.
            byron_initial_funds = genesis.initial_utxos().iter().map(|e| e.lovelace).sum();
            info!(
                k,
                epoch_len = byron_epoch_length,
                initial_funds = byron_initial_funds,
                "Byron genesis loaded"
            );
        }
    }

    let mut shelley_genesis_opt: Option<genesis::ShelleyGenesis> = None;
    let mut shelley_genesis_hash: Option<torsten_primitives::hash::Hash32> = None;
    if let Some(ref genesis_path) = node_config.shelley_genesis_file {
        let genesis_path = config_dir.join(genesis_path);
        if let Ok((genesis, hash)) = genesis::ShelleyGenesis::load_with_hash(&genesis_path) {
            genesis.apply_to_protocol_params(&mut protocol_params);
            info!(epoch_len = genesis.epoch_length, "Shelley genesis loaded");
            shelley_genesis_hash = Some(hash);
            shelley_genesis_opt = Some(genesis);
        }
    }

    if let Some(ref genesis_path) = node_config.alonzo_genesis_file {
        let genesis_path = config_dir.join(genesis_path);
        if let Ok(genesis) = genesis::AlonzoGenesis::load(&genesis_path) {
            genesis.apply_to_protocol_params(&mut protocol_params);
            info!("Alonzo genesis loaded");
        }
    }

    if let Some(ref genesis_path) = node_config.conway_genesis_file {
        let genesis_path = config_dir.join(genesis_path);
        if let Ok(genesis) = genesis::ConwayGenesis::load(&genesis_path) {
            genesis.apply_to_protocol_params(&mut protocol_params);
            info!("Conway genesis loaded");
        }
    }

    // Initialize fresh ledger state from genesis params
    let mut ledger = torsten_ledger::LedgerState::new(protocol_params);

    // Apply Shelley genesis configuration (epoch length, slot config, reserves)
    // Must use set_epoch_length() (not direct field assignment) to compute the
    // correct stability windows (3k/f for Alonzo/Babbage, 4k/f for Conway+)
    // from the network's security parameter k.  With direct assignment the
    // windows default to mainnet values, which are larger than preview's epoch
    // length and cause candidate_nonce to never update.
    if let Some(ref sg) = shelley_genesis_opt {
        ledger.set_slot_config(sg.slot_config());
        ledger.set_epoch_length(sg.epoch_length, sg.security_param);
        ledger.update_quorum = sg.update_quorum;
        // reserves = maxLovelaceSupply - initial fund distribution (Byron genesis)
        // The Byron nonAvvmBalances are distributed at genesis and enter
        // circulation immediately, reducing the reserve pool.
        ledger.reserves = torsten_primitives::value::Lovelace(
            sg.max_lovelace_supply.saturating_sub(byron_initial_funds),
        );
        info!(
            max_supply = sg.max_lovelace_supply,
            initial_funds = byron_initial_funds,
            reserves = ledger.reserves.0,
            "Reserves initialized (maxSupply - initialFunds)"
        );
    }

    // Seed the nonce state machine from the Shelley genesis hash (matching
    // the running node path in Node::init_ledger_state). Without this,
    // evolving/candidate/epoch nonces all start as ZERO and the entire
    // nonce evolution chain diverges from the Haskell reference.
    if let Some(hash) = shelley_genesis_hash {
        ledger.set_genesis_hash(hash);
    }

    // NOTE: Byron genesis UTxOs are NOT seeded here (unlike the running node).
    // The genesis transaction's inputs will show "not found" warnings but
    // outputs are still created. This produces the correct Shelley UTxO set
    // because the Byron inputs are consumed by the genesis transaction.
    // Seeding would require matching the exact tx_hash derivation used by
    // the Haskell node's Byron UTxO format, which is complex.

    // Set the Shelley transition epoch and Byron epoch length.
    // On preview/preprod (no Byron era), transition = 0 and blocks start
    // directly in Alonzo. On mainnet, transition = 208 (Byron epochs 0-207).
    // The default LedgerState uses mainnet values (208/21600) which would
    // produce incorrect epoch boundaries for other networks.
    // Derive network magic from the Shelley genesis (most reliable source),
    // falling back to node config.  The cstreamer-compatible config files
    // often lack an explicit networkMagic field, which caused the fallback
    // to return mainnet magic (764824073) and completely wrong epoch offsets.
    let network_magic = shelley_genesis_opt
        .as_ref()
        .map(|sg| sg.network_magic)
        .or(node_config.network_magic)
        .unwrap_or_else(|| node_config.network.magic());
    let shelley_transition_epoch =
        crate::node::epoch::shelley_transition_epoch_for_magic(network_magic);
    ledger.set_shelley_transition(shelley_transition_epoch, byron_epoch_length);
    info!(
        network_magic,
        shelley_transition_epoch, byron_epoch_length, "HFC epoch configuration set"
    );

    let immutable_dir = args.database_path.join("immutable");
    if !immutable_dir.is_dir() {
        anyhow::bail!(
            "No immutable directory found at {}. Run mithril-import first.",
            immutable_dir.display()
        );
    }

    // Open output (file or stdout) for NDJSON mode (used when --output-dir is not set).
    let mut output: Box<dyn Write> = match &args.output {
        Some(path) => Box::new(std::io::BufWriter::new(std::fs::File::create(path)?)),
        None => Box::new(std::io::stdout().lock()),
    };

    // Create the per-epoch output directory if requested.
    if let Some(ref dir) = args.output_dir {
        std::fs::create_dir_all(dir)?;
    }

    // Extract max_lovelace_supply from Shelley genesis for correct totalStake
    // computation (RC2): cstreamer defines totalStake = maxLovelaceSupply - reserves,
    // not the sum of pool stakes from the set snapshot.
    let max_lovelace_supply = shelley_genesis_opt
        .as_ref()
        .map(|sg| sg.max_lovelace_supply)
        .unwrap_or(45_000_000_000_000_000u64);

    let stop_slot = args.stop_slot.unwrap_or(u64::MAX);
    let mut last_epoch = u64::MAX;
    let mut epoch_fees: u64 = 0;
    let mut blocks_applied = 0u64;
    let mut epochs_written = 0u64;
    let start_time = std::time::Instant::now();

    // Skip the expensive full-UTxO rebuild_stake_distribution at each epoch boundary.
    // During dump-snapshot replay from genesis, every block is applied sequentially
    // with full incremental stake tracking, so the stake_map is always accurate.
    // The full rebuild is only needed after Mithril import (which skips incremental
    // tracking) or snapshot restore.
    // Incremental stake tracking is accurate from genesis — no full UTxO rebuild
    // needed at epoch boundaries. needs_stake_rebuild defaults to false.

    info!("Replaying blocks from ImmutableDB...");

    mithril::replay_from_chunk_files(&immutable_dir, |cbor| {
        let block = torsten_serialization::multi_era::decode_block_minimal_with_byron_epoch_length(
            cbor,
            byron_epoch_length,
        )
        .map_err(|e| anyhow::anyhow!("Block decode error: {e}"))?;

        let block_slot = block.slot().0;
        if block_slot > stop_slot {
            return Err(anyhow::anyhow!("STOP"));
        }

        // Capture the ledger's accumulated epoch fees BEFORE apply_block, so we
        // can compute the delta (actual fees collected by the ledger, which correctly
        // handles invalid tx collateral fees vs declared fees).
        let fees_before = ledger.epoch_fees.0;

        if let Err(e) = ledger.apply_block(&block, torsten_ledger::BlockValidationMode::ApplyOnly) {
            if !format!("{e}").contains("Block does not connect") {
                tracing::warn!(slot = block_slot, "Block apply failed: {e}");
            }
            return Ok(());
        }

        blocks_applied += 1;

        let current_epoch = ledger.epoch.0;

        // Dump state at each epoch transition.
        // The epoch transition (NEWEPOCH rule: reward distribution, nonce rotation,
        // snapshot rotation, protocol param updates) fires inside apply_block when
        // processing the first block of the new epoch.  Cstreamer captures state
        // AFTER the transition, so we read from `ledger` (post-apply) and label
        // with the NEW epoch (current_epoch), matching cstreamer's convention.
        //
        // RC3: accumulate epoch_fees AFTER the transition check so the first block
        //      of a new epoch's fees go into the new epoch's bucket, not the old one.
        if last_epoch != u64::MAX && current_epoch > last_epoch {
            let snapshot =
                build_epoch_snapshot(&ledger, current_epoch, epoch_fees, max_lovelace_supply);

            write_epoch_snapshot(&snapshot, current_epoch, &args.output_dir, &mut output)
                .map_err(|e| anyhow::anyhow!("Snapshot write error: {e}"))?;

            epochs_written += 1;
            info!(
                epoch = current_epoch,
                treasury = ledger.treasury.0,
                reserves = ledger.reserves.0,
                pools = ledger.pool_params.len(),
                fees = epoch_fees,
                era = %format!("{}", ledger.era),
                "Epoch snapshot dumped"
            );

            epoch_fees = 0;
        }

        // Use the ledger's own fee tracking (which correctly handles invalid tx
        // collateral fees). After the epoch transition, ledger.epoch_fees is reset
        // and only includes the current block's fees. For inter-epoch blocks, it
        // accumulates the delta since fees_before.
        let ledger_fees_now = ledger.epoch_fees.0;
        if current_epoch > last_epoch && last_epoch != u64::MAX {
            // Epoch transitioned: fees_before was the OLD epoch's total.
            // The ledger reset epoch_fees and then added this block's fee.
            // epoch_fees was already captured above; now add this block's fees
            // to the NEW epoch bucket.
            epoch_fees = ledger_fees_now;
        } else {
            // Same epoch: add the delta.
            epoch_fees += ledger_fees_now - fees_before;
        }
        last_epoch = current_epoch;
        Ok(())
    })
    .or_else(|e| {
        if format!("{e}").contains("STOP") {
            Ok(0)
        } else {
            Err(e)
        }
    })?;

    // Dump final epoch (the current in-progress epoch at the stop point).
    if blocks_applied > 0 && last_epoch != u64::MAX {
        let snapshot = build_epoch_snapshot(&ledger, last_epoch, epoch_fees, max_lovelace_supply);

        write_epoch_snapshot(&snapshot, last_epoch, &args.output_dir, &mut output)?;
        epochs_written += 1;
    }

    let elapsed = start_time.elapsed();
    info!(
        blocks = blocks_applied,
        epochs_written,
        elapsed_secs = elapsed.as_secs(),
        "dump-snapshot complete"
    );

    Ok(())
}

/// Greatest common divisor (Euclidean algorithm).
fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

/// Serialise one mark/set/go `StakeSnapshot` into the cstreamer JSON format.
///
/// Cstreamer includes full delegation maps, pool parameters, individual stake, and
/// per-pool block counts so that the cross-validation script can catch divergences in
/// snapshot rotation, staking, and pool parameter tracking.
fn serialize_stake_snapshot(
    name: &str,
    snapshot: &torsten_ledger::state::StakeSnapshot,
    override_blocks: Option<&std::collections::HashMap<torsten_primitives::hash::Hash28, u64>>,
) -> serde_json::Value {
    use torsten_primitives::transaction::Relay;

    // delegations: credential hash → pool ID
    // Key format matches cstreamer: "keyHash-{56_hex_chars}" (28-byte hash).
    // Torsten stores credentials as Hash32 (padded), so trim to 28 bytes.
    let delegations: serde_json::Map<String, serde_json::Value> = snapshot
        .delegations
        .iter()
        .map(|(cred, pool_id)| {
            let key = format!("keyHash-{}", hex::encode(&cred.as_bytes()[..28]));
            let val = serde_json::Value::String(hex::encode(pool_id.as_bytes()));
            (key, val)
        })
        .collect();

    // poolParams: pool_id hex → rich pool params object
    let pool_params: serde_json::Map<String, serde_json::Value> = snapshot
        .pool_params
        .iter()
        .map(|(pool_id, reg)| {
            let key = hex::encode(pool_id.as_bytes());

            // Owners: list of 28-byte key hash hex strings
            let owners: Vec<serde_json::Value> = reg
                .owners
                .iter()
                .map(|o| serde_json::Value::String(hex::encode(o.as_bytes())))
                .collect();

            // margin as f64 ratio
            let margin = if reg.margin_denominator == 0 {
                0.0f64
            } else {
                reg.margin_numerator as f64 / reg.margin_denominator as f64
            };

            // rewardAccount: decode the raw bytes (byte 0 = header, bytes 1..29 = cred hash)
            let reward_account_json = if reg.reward_account.len() >= 29 {
                let header = reg.reward_account[0];
                let network = if header & 0x0F == 1 {
                    "Mainnet"
                } else {
                    "Testnet"
                };
                let cred_hex = hex::encode(&reg.reward_account[1..29]);
                serde_json::json!({
                    "credential": { "keyHash": cred_hex },
                    "network": network,
                })
            } else {
                serde_json::Value::Null
            };

            // relays
            let relays: Vec<serde_json::Value> = reg
                .relays
                .iter()
                .map(|r| match r {
                    Relay::SingleHostAddr { port, ipv4, ipv6 } => serde_json::json!({
                        "type": "SingleHostAddr",
                        "port": port,
                        "ipv4": ipv4.map(|ip| format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])),
                        "ipv6": ipv6.map(hex::encode),
                    }),
                    Relay::SingleHostName { port, dns_name } => serde_json::json!({
                        "type": "SingleHostName",
                        "port": port,
                        "dnsName": dns_name,
                    }),
                    Relay::MultiHostName { dns_name } => serde_json::json!({
                        "type": "MultiHostName",
                        "dnsName": dns_name,
                    }),
                })
                .collect();

            let val = serde_json::json!({
                "publicKey": hex::encode(reg.pool_id.as_bytes()),
                "owners": owners,
                "pledge": reg.pledge.0,
                "cost": reg.cost.0,
                "margin": margin,
                "rewardAccount": reward_account_json,
                "vrf": hex::encode(reg.vrf_keyhash.as_bytes()),
                "relays": relays,
                "metadata": reg.metadata_url.as_ref().map(|url| serde_json::json!({
                    "url": url,
                    "hash": reg.metadata_hash.as_ref().map(|h| hex::encode(h.as_bytes())),
                })),
            });

            (key, val)
        })
        .collect();

    // stake: per-credential lovelace
    // Key format: "keyHash-{56_hex_chars}" (28-byte hash, trimmed from Hash32).
    let stake: serde_json::Map<String, serde_json::Value> = snapshot
        .stake_distribution
        .iter()
        .map(|(cred, lovelace)| {
            let key = format!("keyHash-{}", hex::encode(&cred.as_bytes()[..28]));
            let val = serde_json::Value::Number(lovelace.0.into());
            (key, val)
        })
        .collect();

    // blocks: per-pool block production count.
    // Cstreamer uses Haskell's nesBcur/nesBprev (tracked separately from snapshots):
    //   - mark.blocks = nesBcur (blocks produced in current epoch so far)
    //   - go.blocks   = nesBprev (blocks from previous epoch)
    //   - set.blocks   = not included (None/omitted)
    // Callers pass the appropriate block source, or None to omit.
    let mut result = serde_json::json!({
        "name": name,
        "epoch": snapshot.epoch.0,
        "delegations": delegations,
        "poolParams": pool_params,
        "stake": stake,
    });
    if let Some(block_map) = override_blocks {
        let blocks: serde_json::Map<String, serde_json::Value> = block_map
            .iter()
            .map(|(pool_id, count)| {
                let key = hex::encode(pool_id.as_bytes());
                let val = serde_json::Value::Number((*count).into());
                (key, val)
            })
            .collect();
        result["blocks"] = serde_json::Value::Object(blocks);
    }
    result
}

/// Build the richer epoch-snapshot JSON object from the current ledger state.
///
/// Called at every epoch transition (the ledger already reflects the NEWEPOCH
/// rule — reward distribution, nonce rotation, snapshot rotation, protocol
/// param updates) and for the final in-progress epoch.  Fields match the
/// cstreamer reference format for cross-validation.
fn build_epoch_snapshot(
    ledger: &torsten_ledger::LedgerState,
    epoch: u64,
    epoch_fees: u64,
    max_lovelace_supply: u64,
) -> serde_json::Value {
    // RC2: totalStake = maxLovelaceSupply - reserves (matches cstreamer).
    let total_stake = max_lovelace_supply.saturating_sub(ledger.reserves.0);

    // Active stake from the "go" snapshot (used for reward distribution).
    let active_stake: u64 = ledger
        .snapshots
        .go
        .as_ref()
        .map(|s| s.pool_stake.values().map(|v| v.0).sum())
        .unwrap_or(0);

    // Pool distribution from the "set" snapshot with extended cstreamer fields.
    let total_active_stake = ledger
        .snapshots
        .set
        .as_ref()
        .map(|s| s.pool_stake.values().map(|v| v.0).sum::<u64>())
        .unwrap_or(0);

    let pool_distribution: Vec<serde_json::Value> = ledger
        .snapshots
        .set
        .as_ref()
        .map(|s| {
            s.pool_stake
                .iter()
                .map(|(pool_id, stake_lovelace)| {
                    let lv = stake_lovelace.0;
                    let pct = if total_active_stake > 0 {
                        lv as f64 / total_active_stake as f64 * 100.0
                    } else {
                        0.0
                    };
                    // Reduce the stake fraction to simplest form (matching cstreamer).
                    let (num, den) = if lv == 0 {
                        (0, 1) // Zero stake: 0/1
                    } else if total_active_stake > 0 {
                        let g = gcd(lv, total_active_stake);
                        (lv / g, total_active_stake / g)
                    } else {
                        (0, 1)
                    };
                    serde_json::json!({
                        "poolId": hex::encode(pool_id.as_bytes()),
                        "stake": { "numerator": num, "denominator": den },
                        "stakeLovelace": lv,
                        "stakePercent": pct,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    // Deposit accounting.
    let pp = &ledger.protocol_params;
    let deposit_stake_key = ledger.reward_accounts.len() as u64 * pp.key_deposit.0;
    let deposit_pool = ledger.pool_params.len() as u64 * pp.pool_deposit.0;
    let deposit_drep = ledger.governance.dreps.len() as u64 * pp.drep_deposit.0;
    let deposit_proposal: u64 = 0;
    let deposit_total = deposit_stake_key + deposit_pool + deposit_drep + deposit_proposal;

    // Protocol params summary.
    // Use prev_protocol_params (esPrevPp) to match cstreamer's convention:
    // cstreamer dumps the params that governed the PREVIOUS epoch, not the
    // post-UPEC params for the current epoch.
    let prev_pp = &ledger.prev_protocol_params;
    let protocol_params = serde_json::json!({
        "a0": { "numerator": prev_pp.a0.numerator, "denominator": prev_pp.a0.denominator },
        "d":  { "numerator": prev_pp.d.numerator,  "denominator": prev_pp.d.denominator  },
        "rho": { "numerator": prev_pp.rho.numerator, "denominator": prev_pp.rho.denominator },
        "tau": { "numerator": prev_pp.tau.numerator, "denominator": prev_pp.tau.denominator },
        "nOpt": prev_pp.n_opt,
        "minPoolCost": prev_pp.min_pool_cost.0,
        "protocolVersion": {
            "major": prev_pp.protocol_version_major,
            "minor": prev_pp.protocol_version_minor,
        },
    });

    // Pending reward update.
    let rupd_next: serde_json::Value = match &ledger.pending_reward_update {
        None => serde_json::Value::Null,
        Some(pu) => {
            let total_distributed: u64 = pu.rewards.values().map(|v| v.0).sum();
            serde_json::json!({
                "deltaR1": pu.delta_reserves,
                "deltaT1": pu.delta_treasury,
                "totalDistributed": total_distributed,
            })
        }
    };

    // RC4: full mark/set/go stake snapshots for cross-validation.
    // Block counts match Haskell's nesBcur/nesBprev (tracked outside snapshots):
    //   mark → nesBcur (blocks produced so far in current epoch)
    //   set  → no blocks (not a Haskell concept)
    //   go   → nesBprev (blocks from previous epoch)
    let snap_mark = ledger
        .snapshots
        .mark
        .as_ref()
        .map(|s| serialize_stake_snapshot("mark", s, Some(ledger.epoch_blocks_by_pool.as_ref())))
        .unwrap_or(serde_json::Value::Null);
    let snap_set = ledger
        .snapshots
        .set
        .as_ref()
        .map(|s| serialize_stake_snapshot("set", s, None))
        .unwrap_or(serde_json::Value::Null);
    let snap_go = if let Some(s) = ledger.snapshots.go.as_ref() {
        serialize_stake_snapshot(
            "go",
            s,
            Some(ledger.snapshots.bprev_blocks_by_pool.as_ref()),
        )
    } else {
        // In Haskell, snapshots are never null — empty SnapShot with nesBprev blocks.
        let bprev_blocks: serde_json::Map<String, serde_json::Value> = ledger
            .snapshots
            .bprev_blocks_by_pool
            .iter()
            .map(|(pool_id, count)| {
                (
                    hex::encode(pool_id.as_bytes()),
                    serde_json::Value::Number((*count).into()),
                )
            })
            .collect();
        serde_json::json!({
            "name": "go",
            "epoch": 0,
            "delegations": {},
            "poolParams": {},
            "stake": {},
            "blocks": bprev_blocks,
        })
    };

    serde_json::json!({
        "epoch": epoch,
        "epochFees": epoch_fees,
        "reserves": ledger.reserves.0,
        "treasury": ledger.treasury.0,
        "totalStake": total_stake,
        "activeStake": active_stake,
        "totalPools": pool_distribution.len(),
        "poolDistribution": pool_distribution,
        "snapshotEraName": format!("{}", ledger.era),
        "epochNonce": hex::encode(ledger.epoch_nonce.0),
        "deposits": {
            "stakeKey": deposit_stake_key,
            "pool": deposit_pool,
            "dRep": deposit_drep,
            "proposal": deposit_proposal,
            "total": deposit_total,
        },
        "protocolParams": protocol_params,
        "rupdNext": rupd_next,
        "snapshots": {
            "mark": snap_mark,
            "set": snap_set,
            "go": snap_go,
        },
    })
}

/// Write an epoch snapshot either to a per-epoch file in `output_dir` (when set)
/// or as an NDJSON line to the shared `output` writer (fallback).
fn write_epoch_snapshot(
    snapshot: &serde_json::Value,
    epoch: u64,
    output_dir: &Option<std::path::PathBuf>,
    output: &mut Box<dyn std::io::Write>,
) -> anyhow::Result<()> {
    use std::io::Write;

    if let Some(dir) = output_dir {
        // Write {epoch}.json — pretty-printed for human readability.
        let path = dir.join(format!("{epoch}.json"));
        let file = std::fs::File::create(&path)
            .map_err(|e| anyhow::anyhow!("Cannot create {}: {e}", path.display()))?;
        let writer = std::io::BufWriter::new(file);
        serde_json::to_writer_pretty(writer, snapshot)
            .map_err(|e| anyhow::anyhow!("JSON serialise error: {e}"))?;
    } else {
        // NDJSON: one compact JSON object per line.
        serde_json::to_writer(&mut *output, snapshot)
            .map_err(|e| anyhow::anyhow!("JSON write error: {e}"))?;
        writeln!(output).map_err(|e| anyhow::anyhow!("Write error: {e}"))?;
    }
    Ok(())
}

async fn run_db_command(args: DbArgs) -> Result<()> {
    match args.command {
        DbCommand::Info(info_args) => run_db_info(info_args).await,
    }
}

async fn run_db_info(args: DbInfoArgs) -> Result<()> {
    let db_path = &args.database_path;
    if !db_path.exists() {
        anyhow::bail!("Database path does not exist: {}", db_path.display());
    }

    let storage_profile: torsten_storage::StorageProfile = args
        .storage_profile
        .parse()
        .map_err(|e: String| anyhow::anyhow!(e))?;
    let storage_config = torsten_storage::config::resolve_storage_config(
        storage_profile,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .map_err(|e| anyhow::anyhow!(e))?;

    // Open the ChainDB read-only
    let chain_db = torsten_storage::ChainDB::open_with_config(db_path, &storage_config.immutable)?;

    // Immutable DB info
    let immutable_dir = db_path.join("immutable");
    let (chunk_count, immutable_size) = if immutable_dir.exists() {
        let mut count = 0u64;
        let mut total_size = 0u64;
        for entry in std::fs::read_dir(&immutable_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.ends_with(".chunk") {
                count += 1;
            }
            total_size += entry.metadata().map(|m| m.len()).unwrap_or(0);
        }
        (count, total_size)
    } else {
        (0, 0)
    };

    // VolatileDB block count (from ChainDB tip info)
    let volatile_count = chain_db.volatile_block_count();

    // Ledger snapshot info
    let snapshot_dir = db_path.join("snapshots");
    let (snapshot_count, snapshot_size) = if snapshot_dir.exists() {
        let mut count = 0u64;
        let mut total_size = 0u64;
        for entry in std::fs::read_dir(&snapshot_dir)? {
            let entry = entry?;
            count += 1;
            total_size += entry.metadata().map(|m| m.len()).unwrap_or(0);
        }
        (count, total_size)
    } else {
        (0, 0)
    };

    let tip = chain_db.get_tip();

    println!("Torsten Database Info");
    println!("=====================");
    println!("  Database path:      {}", db_path.display());
    println!(
        "  Chain tip slot:     {}",
        tip.point.slot().map(|s| s.0).unwrap_or(0)
    );
    println!("  Chain tip block:    {}", tip.block_number.0);
    println!();
    println!("ImmutableDB:");
    println!("  Chunk files:        {chunk_count}");
    println!("  Total size:         {}", format_size(immutable_size));
    println!();
    println!("VolatileDB:");
    println!("  Block count:        {volatile_count}");
    println!();
    println!("Ledger Snapshots:");
    println!("  Snapshot count:     {snapshot_count}");
    println!("  Total size:         {}", format_size(snapshot_size));

    Ok(())
}

fn format_size(bytes: u64) -> String {
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB ({bytes} bytes)", b / GB)
    } else if b >= MB {
        format!("{:.2} MB ({bytes} bytes)", b / MB)
    } else if b >= KB {
        format!("{:.2} KB ({bytes} bytes)", b / KB)
    } else {
        format!("{bytes} bytes")
    }
}

async fn run_mithril_import(args: MithrilImportArgs) -> Result<()> {
    info!(
        "Starting Mithril snapshot import for network magic {}",
        args.network_magic
    );
    mithril::import_snapshot(
        args.network_magic,
        &args.database_path,
        args.temp_dir.as_deref(),
    )
    .await
}

async fn run_node(args: RunArgs) -> Result<()> {
    info!(
        version = env!("CARGO_PKG_VERSION"),
        "Torsten Cardano Node starting"
    );

    // Load configuration
    let node_config = config::NodeConfig::load(&args.config)?;
    let config_dir = args
        .config
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf();
    node_config.validate(&config_dir)?;

    // Resolve effective metrics port using a three-level priority:
    //   1. --no-metrics flag → 0 (disabled), takes highest precedence
    //   2. --metrics-port <PORT> CLI arg → explicit operator override
    //   3. MetricsPort field in config JSON → site-wide default from config file
    //   4. Cardano-node default: 12798
    const DEFAULT_METRICS_PORT: u16 = 12798;
    let effective_metrics_port: u16 = if args.no_metrics {
        0
    } else if let Some(cli_port) = args.metrics_port {
        cli_port
    } else {
        node_config.metrics_port.unwrap_or(DEFAULT_METRICS_PORT)
    };

    // Load topology
    let topology = topology::Topology::load(&args.topology)?;
    let all_peers = topology.all_peers();

    info!(config = %args.config.display(), "Configuration");
    info!(path = %args.database_path.display(), "Database");
    info!(path = %args.socket_path.display(), "Socket");
    info!(
        network = ?node_config.network,
        magic = node_config.network_magic.unwrap_or_else(|| node_config.network.magic()),
        "Network",
    );
    info!(host = %args.host_addr, port = args.port, "Listen");
    if effective_metrics_port > 0 {
        info!(port = effective_metrics_port, "Metrics");
    } else {
        info!("Metrics disabled");
    }
    info!(
        total = all_peers.len(),
        producers = topology.producers.len(),
        bootstrap = topology.bootstrap_peers.as_ref().map_or(0, |v| v.len()),
        local = topology
            .local_roots
            .iter()
            .map(|g| g.access_points.len())
            .sum::<usize>(),
        public = topology
            .public_roots
            .iter()
            .map(|r| r.access_points.len())
            .sum::<usize>(),
        "Topology",
    );

    // Resolve storage configuration: profile < config file < CLI
    let storage_profile: torsten_storage::StorageProfile = args
        .storage_profile
        .parse()
        .map_err(|e: String| anyhow::anyhow!(e))?;
    let storage_config = torsten_storage::config::resolve_storage_config(
        storage_profile,
        node_config.storage.as_ref(),
        args.immutable_index_type.as_deref(),
        args.utxo_backend.as_deref(),
        args.utxo_memtable_size_mb,
        args.utxo_block_cache_size_mb,
        args.utxo_bloom_filter_bits,
    )
    .map_err(|e| anyhow::anyhow!(e))?;

    info!(
        profile = %storage_profile,
        index = ?storage_config.immutable.index_type,
        utxo = ?storage_config.utxo.backend,
        "Storage",
    );

    // Initialize the node
    let mut node = node::Node::new(node::NodeArgs {
        config: node_config,
        topology,
        topology_path: args.topology.clone(),
        database_path: args.database_path,
        socket_path: args.socket_path,
        host_addr: args.host_addr,
        port: args.port,
        config_dir,
        shelley_kes_key: args.shelley_kes_key,
        shelley_vrf_key: args.shelley_vrf_key,
        shelley_operational_certificate: args.shelley_operational_certificate,
        _shelley_cold_key: args.shelley_cold_key,
        metrics_port: effective_metrics_port,
        compat_metrics: args.compat_metrics,
        mempool_max_tx: args.mempool_max_tx,
        mempool_max_bytes: args.mempool_max_bytes,
        snapshot_max_retained: args.snapshot_max_retained,
        snapshot_bulk_min_blocks: args.snapshot_bulk_min_blocks,
        snapshot_bulk_min_secs: args.snapshot_bulk_min_secs,
        storage_config,
        consensus_mode: args.consensus_mode,
        validate_all_blocks: args.validate_all_blocks,
    })?;

    info!("");

    // Run the node with a concurrent SIGTERM/SIGINT watcher so the process
    // exits cleanly (flushing logs, releasing the LSM lock, etc.) when the
    // service manager stops it.  Without this, `panic=abort` means SIGTERM is
    // handled by the OS with no log flush or resource cleanup.
    tokio::select! {
        result = node.run() => {
            result?;
        }
        _ = async {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{signal, SignalKind};
                let mut sigterm = signal(SignalKind::terminate())
                    .expect("SIGTERM handler registration failed");
                sigterm.recv().await;
                info!("SIGTERM received — shutting down");
            }
            #[cfg(not(unix))]
            {
                tokio::signal::ctrl_c().await.ok();
                info!("CTRL-C received — shutting down");
            }
        } => {}
        _ = async {
            tokio::signal::ctrl_c().await.ok();
            info!("CTRL-C received — shutting down");
        } => {}
    }

    Ok(())
}
