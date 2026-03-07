use anyhow::Result;
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct QueryCmd {
    #[command(subcommand)]
    command: QuerySubcommand,
}

#[derive(Subcommand, Debug)]
enum QuerySubcommand {
    /// Query the current tip
    Tip {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
    },
    /// Query UTxOs at an address
    Utxo {
        #[arg(long)]
        address: String,
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
    },
    /// Query protocol parameters
    ProtocolParameters {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        out_file: Option<PathBuf>,
    },
    /// Query stake distribution
    StakeDistribution {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
    },
    /// Query stake address info
    StakeAddressInfo {
        #[arg(long)]
        address: String,
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
    },
    /// Query governance state (Conway era)
    GovState {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
    },
    /// Query DRep state (Conway era)
    DrepState {
        #[arg(long)]
        drep_key_hash: Option<String>,
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
    },
    /// Query committee state (Conway era)
    CommitteeState {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
    },
}

impl QueryCmd {
    pub fn run(self) -> Result<()> {
        match self.command {
            QuerySubcommand::Tip { socket_path } => {
                // TODO: Connect to node and query tip
                println!("Querying tip via {}...", socket_path.display());
                println!("{{");
                println!("  \"slot\": 0,");
                println!("  \"hash\": \"0000000000000000000000000000000000000000000000000000000000000000\",");
                println!("  \"block\": 0,");
                println!("  \"epoch\": 0,");
                println!("  \"era\": \"Conway\",");
                println!("  \"syncProgress\": \"0.00\"");
                println!("}}");
                Ok(())
            }
            QuerySubcommand::Utxo {
                address,
                socket_path,
            } => {
                println!("Querying UTxOs for {}...", address);
                // TODO: Connect to node and query UTxOs
                Ok(())
            }
            QuerySubcommand::ProtocolParameters {
                socket_path,
                out_file,
            } => {
                let params = torsten_primitives::protocol_params::ProtocolParameters::mainnet_defaults();
                let json = serde_json::to_string_pretty(&params)?;
                if let Some(out) = out_file {
                    std::fs::write(&out, &json)?;
                    println!("Protocol parameters written to: {}", out.display());
                } else {
                    println!("{}", json);
                }
                Ok(())
            }
            QuerySubcommand::StakeDistribution { socket_path } => {
                println!("Querying stake distribution...");
                Ok(())
            }
            QuerySubcommand::StakeAddressInfo {
                address,
                socket_path,
            } => {
                println!("Querying stake address info for {}...", address);
                Ok(())
            }
            QuerySubcommand::GovState { socket_path } => {
                println!("Querying governance state...");
                Ok(())
            }
            QuerySubcommand::DrepState {
                drep_key_hash,
                socket_path,
            } => {
                println!("Querying DRep state...");
                Ok(())
            }
            QuerySubcommand::CommitteeState { socket_path } => {
                println!("Querying committee state...");
                Ok(())
            }
        }
    }
}
