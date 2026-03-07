use anyhow::Result;
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct GovernanceCmd {
    #[command(subcommand)]
    command: GovernanceSubcommand,
}

#[derive(Subcommand, Debug)]
enum GovernanceSubcommand {
    /// DRep commands
    Drep {
        #[command(subcommand)]
        command: DRepSubcommand,
    },
    /// Vote on governance actions
    Vote {
        #[command(subcommand)]
        command: VoteSubcommand,
    },
    /// Create governance actions
    Action {
        #[command(subcommand)]
        command: ActionSubcommand,
    },
}

#[derive(Subcommand, Debug)]
enum DRepSubcommand {
    /// Generate DRep keys
    KeyGen {
        #[arg(long)]
        verification_key_file: PathBuf,
        #[arg(long)]
        signing_key_file: PathBuf,
    },
    /// Get DRep ID
    Id {
        #[arg(long)]
        drep_verification_key_file: PathBuf,
    },
    /// Create DRep registration certificate
    RegistrationCertificate {
        #[arg(long)]
        drep_verification_key_file: PathBuf,
        #[arg(long)]
        key_reg_deposit_amt: u64,
        #[arg(long)]
        out_file: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
enum VoteSubcommand {
    /// Create a vote file
    Create {
        #[arg(long)]
        governance_action_tx_id: String,
        #[arg(long)]
        governance_action_index: u32,
        #[arg(long)]
        vote: String,
        #[arg(long)]
        drep_verification_key_file: Option<PathBuf>,
        #[arg(long)]
        out_file: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
enum ActionSubcommand {
    /// Create an info action
    CreateInfo {
        #[arg(long)]
        anchor_url: String,
        #[arg(long)]
        anchor_data_hash: String,
        #[arg(long)]
        deposit: u64,
        #[arg(long)]
        return_addr: String,
        #[arg(long)]
        out_file: PathBuf,
    },
}

impl GovernanceCmd {
    pub fn run(self) -> Result<()> {
        match self.command {
            GovernanceSubcommand::Drep { command } => match command {
                DRepSubcommand::KeyGen {
                    verification_key_file,
                    signing_key_file,
                } => {
                    let sk = torsten_crypto::keys::PaymentSigningKey::generate();
                    let vk = sk.verification_key();

                    let sk_env = serde_json::json!({
                        "type": "DRepSigningKey_ed25519",
                        "description": "Delegated Representative Signing Key",
                        "cborHex": hex::encode(simple_cbor_wrap(&sk.to_bytes()))
                    });
                    let vk_env = serde_json::json!({
                        "type": "DRepVerificationKey_ed25519",
                        "description": "Delegated Representative Verification Key",
                        "cborHex": hex::encode(simple_cbor_wrap(&vk.to_bytes()))
                    });

                    std::fs::write(&signing_key_file, serde_json::to_string_pretty(&sk_env)?)?;
                    std::fs::write(&verification_key_file, serde_json::to_string_pretty(&vk_env)?)?;

                    println!("DRep keys generated.");
                    Ok(())
                }
                DRepSubcommand::Id {
                    drep_verification_key_file,
                } => {
                    let content = std::fs::read_to_string(&drep_verification_key_file)?;
                    let env: serde_json::Value = serde_json::from_str(&content)?;
                    let cbor_hex = env["cborHex"].as_str().unwrap_or("");
                    let cbor_bytes = hex::decode(cbor_hex)?;
                    let key_bytes = if cbor_bytes.len() > 2 {
                        &cbor_bytes[2..]
                    } else {
                        &cbor_bytes
                    };
                    let hash = torsten_primitives::hash::blake2b_224(key_bytes);
                    let drep_id = bech32::encode::<bech32::Bech32>(
                        bech32::Hrp::parse("drep")?,
                        hash.as_bytes(),
                    )?;
                    println!("{}", drep_id);
                    Ok(())
                }
                _ => {
                    println!("Command not yet implemented");
                    Ok(())
                }
            },
            _ => {
                println!("Command not yet implemented");
                Ok(())
            }
        }
    }
}

fn simple_cbor_wrap(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::new();
    if data.len() < 24 {
        result.push(0x40 | data.len() as u8);
    } else if data.len() < 256 {
        result.push(0x58);
        result.push(data.len() as u8);
    } else {
        result.push(0x59);
        result.extend_from_slice(&(data.len() as u16).to_be_bytes());
    }
    result.extend_from_slice(data);
    result
}
