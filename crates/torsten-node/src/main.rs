mod config;
mod disk_monitor;
mod forge;
mod genesis;
mod gsm;
mod logging;
mod metrics;
mod mithril;
mod node;
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

    /// Prometheus metrics port (0 to disable)
    #[arg(long, default_value = "12798")]
    metrics_port: u16,

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
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Extract log args and initialize logging before any work
    let log_args = match &cli.command {
        Command::Run(ref args) => &args.log,
        Command::MithrilImport(ref args) => &args.log,
    };
    let _log_guard = logging::init(&build_logging_opts(log_args)?)?;

    match cli.command {
        Command::Run(args) => run_node(*args).await,
        Command::MithrilImport(args) => run_mithril_import(args).await,
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
        metrics_port: args.metrics_port,
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
    node.run().await?;

    Ok(())
}
