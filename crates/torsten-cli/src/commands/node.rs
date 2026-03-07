use anyhow::Result;
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct NodeCmd {
    #[command(subcommand)]
    command: NodeSubcommand,
}

#[derive(Subcommand, Debug)]
enum NodeSubcommand {
    /// Show key generation commands
    KeyGen {
        #[arg(long)]
        cold_verification_key_file: PathBuf,
        #[arg(long)]
        cold_signing_key_file: PathBuf,
        #[arg(long)]
        operational_certificate_counter_file: PathBuf,
    },
    /// Issue a new operational certificate
    IssueOpCert {
        #[arg(long)]
        kes_verification_key_file: PathBuf,
        #[arg(long)]
        cold_signing_key_file: PathBuf,
        #[arg(long)]
        operational_certificate_counter_file: PathBuf,
        #[arg(long)]
        kes_period: u64,
        #[arg(long)]
        out_file: PathBuf,
    },
}

impl NodeCmd {
    pub fn run(self) -> Result<()> {
        match self.command {
            NodeSubcommand::KeyGen {
                cold_verification_key_file: _,
                cold_signing_key_file: _,
                operational_certificate_counter_file: _,
            } => {
                println!("Generating node cold keys...");
                let sk = torsten_crypto::keys::PaymentSigningKey::generate();
                let vk = sk.verification_key();
                println!("Cold verification key hash: {}", vk.hash().to_hex());
                // TODO: Write key files
                Ok(())
            }
            NodeSubcommand::IssueOpCert { .. } => {
                println!("Issuing operational certificate...");
                // TODO: Implement operational certificate issuance
                Ok(())
            }
        }
    }
}
