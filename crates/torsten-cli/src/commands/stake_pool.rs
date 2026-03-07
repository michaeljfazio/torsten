use anyhow::Result;
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct StakePoolCmd {
    #[command(subcommand)]
    command: StakePoolSubcommand,
}

#[derive(Subcommand, Debug)]
enum StakePoolSubcommand {
    /// Generate pool keys (cold, VRF, KES)
    KeyGen {
        #[arg(long)]
        cold_verification_key_file: PathBuf,
        #[arg(long)]
        cold_signing_key_file: PathBuf,
        #[arg(long)]
        operational_certificate_counter_file: PathBuf,
    },
    /// Get pool ID from verification key
    Id {
        #[arg(long)]
        cold_verification_key_file: PathBuf,
    },
    /// Generate VRF key pair
    VrfKeyGen {
        #[arg(long)]
        verification_key_file: PathBuf,
        #[arg(long)]
        signing_key_file: PathBuf,
    },
    /// Generate KES key pair
    KesKeyGen {
        #[arg(long)]
        verification_key_file: PathBuf,
        #[arg(long)]
        signing_key_file: PathBuf,
    },
    /// Issue operational certificate
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

impl StakePoolCmd {
    pub fn run(self) -> Result<()> {
        match self.command {
            StakePoolSubcommand::KeyGen {
                cold_verification_key_file,
                cold_signing_key_file,
                operational_certificate_counter_file,
            } => {
                let sk = torsten_crypto::keys::PaymentSigningKey::generate();
                let vk = sk.verification_key();

                let sk_env = serde_json::json!({
                    "type": "StakePoolSigningKey_ed25519",
                    "description": "Stake Pool Operator Signing Key",
                    "cborHex": hex::encode(simple_cbor_wrap(&sk.to_bytes()))
                });
                let vk_env = serde_json::json!({
                    "type": "StakePoolVerificationKey_ed25519",
                    "description": "Stake Pool Operator Verification Key",
                    "cborHex": hex::encode(simple_cbor_wrap(&vk.to_bytes()))
                });
                let counter = serde_json::json!({
                    "type": "NodeOperationalCertificateIssueCounter",
                    "description": "",
                    "cborHex": "820058200000000000000000000000000000000000000000000000000000000000000000"
                });

                std::fs::write(
                    &cold_signing_key_file,
                    serde_json::to_string_pretty(&sk_env)?,
                )?;
                std::fs::write(
                    &cold_verification_key_file,
                    serde_json::to_string_pretty(&vk_env)?,
                )?;
                std::fs::write(
                    &operational_certificate_counter_file,
                    serde_json::to_string_pretty(&counter)?,
                )?;

                println!("Pool cold keys generated.");
                Ok(())
            }
            StakePoolSubcommand::Id {
                cold_verification_key_file,
            } => {
                let content = std::fs::read_to_string(&cold_verification_key_file)?;
                let env: serde_json::Value = serde_json::from_str(&content)?;
                let cbor_hex = env["cborHex"].as_str().unwrap_or("");
                let cbor_bytes = hex::decode(cbor_hex)?;
                let key_bytes = if cbor_bytes.len() > 2 {
                    &cbor_bytes[2..]
                } else {
                    &cbor_bytes
                };
                let hash = torsten_primitives::hash::blake2b_224(key_bytes);
                let pool_id =
                    bech32::encode::<bech32::Bech32>(bech32::Hrp::parse("pool")?, hash.as_bytes())?;
                println!("{}", pool_id);
                Ok(())
            }
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
