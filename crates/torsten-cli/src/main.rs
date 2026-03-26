mod commands;

use anyhow::Result;
use clap::Parser;

/// Torsten CLI - Cardano-CLI compatible command-line interface
#[derive(Parser, Debug)]
#[command(name = "torsten-cli", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand, Debug)]
enum Commands {
    /// Address commands
    Address(commands::address::AddressCmd),
    /// Key generation commands
    Key(commands::key::KeyCmd),
    /// Transaction commands
    Transaction(commands::transaction::TransactionCmd),
    /// Query commands
    Query(commands::query::QueryCmd),
    /// Stake address commands
    StakeAddress(commands::stake_address::StakeAddressCmd),
    /// Stake pool commands
    StakePool(commands::stake_pool::StakePoolCmd),
    /// Governance commands (Conway era)
    Governance(commands::governance::GovernanceCmd),
    /// Node-related commands
    Node(commands::node::NodeCmd),
    /// Genesis block commands
    Genesis(commands::genesis::GenesisCmd),
    /// Text-view file commands
    TextView(commands::text_view::TextViewCmd),
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Address(cmd) => cmd.run(),
        Commands::Key(cmd) => cmd.run(),
        Commands::Transaction(cmd) => cmd.run(),
        Commands::Query(cmd) => cmd.run(),
        Commands::StakeAddress(cmd) => cmd.run(),
        Commands::StakePool(cmd) => cmd.run(),
        Commands::Governance(cmd) => cmd.run(),
        Commands::Node(cmd) => cmd.run(),
        Commands::Genesis(cmd) => cmd.run(),
        Commands::TextView(cmd) => cmd.run(),
    }
}
