mod config;
mod genesis;
mod metrics;
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
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(true)
        .with_thread_ids(true)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Run(args) => run_node(args).await,
    }
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
    info!(
        "Topology: {} producers configured",
        topology.producers.len()
    );

    // Initialize the node
    let mut node = node::Node::new(node::NodeArgs {
        config: node_config,
        topology,
        database_path: args.database_path,
        socket_path: args.socket_path,
        host_addr: args.host_addr,
        port: args.port,
    })?;

    // Run the node
    info!("Node initialized, starting...");
    node.run().await?;

    Ok(())
}
