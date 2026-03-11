mod config;
mod disk_monitor;
mod forge;
mod genesis;
mod metrics;
mod mithril;
mod node;
mod topology;

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use tracing::info;
use tracing_subscriber::EnvFilter;

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
    Run(RunArgs),
    /// Import a Mithril snapshot for fast initial sync
    MithrilImport(MithrilImportArgs),
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

    /// Path to the cold signing key file (required for block production)
    #[arg(long)]
    shelley_cold_key: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(true)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Run(args) => run_node(args).await,
        Command::MithrilImport(args) => run_mithril_import(args).await,
    }
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
        "Starting Torsten Cardano Node v{}",
        env!("CARGO_PKG_VERSION")
    );
    info!("Config: {}", args.config.display());
    info!("Database: {}", args.database_path.display());
    info!("Socket: {}", args.socket_path.display());
    info!("Listening on {}:{}", args.host_addr, args.port);

    // Load configuration
    let node_config = config::NodeConfig::load(&args.config)?;
    info!("Network: {:?}", node_config.network);

    // Load topology
    let topology = topology::Topology::load(&args.topology)?;
    let all_peers = topology.all_peers();
    info!(
        "Topology: {} peers configured (producers={}, bootstrap={}, local_roots={}, public_roots={})",
        all_peers.len(),
        topology.producers.len(),
        topology.bootstrap_peers.as_ref().map_or(0, |v| v.len()),
        topology.local_roots.iter().map(|g| g.access_points.len()).sum::<usize>(),
        topology.public_roots.iter().map(|r| r.access_points.len()).sum::<usize>(),
    );

    // Initialize the node
    let config_dir = args
        .config
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf();
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
        shelley_cold_key: args.shelley_cold_key,
        metrics_port: args.metrics_port,
    })?;

    // Run the node
    info!("Node initialized, starting...");
    node.run().await?;

    Ok(())
}
