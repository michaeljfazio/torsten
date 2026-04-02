use anyhow::Result;
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct GenesisCmd {
    #[command(subcommand)]
    command: GenesisSubcommand,
}

#[derive(Subcommand, Debug)]
enum GenesisSubcommand {
    /// Generate genesis keys
    KeyGen {
        #[arg(long)]
        verification_key_file: PathBuf,
        #[arg(long)]
        signing_key_file: PathBuf,
    },
    /// Print the hash of a genesis key
    KeyHash {
        #[arg(long)]
        verification_key_file: PathBuf,
    },
    /// Create a genesis delegation certificate
    GenesisDelegation {
        #[arg(long)]
        genesis_verification_key_file: PathBuf,
        #[arg(long)]
        drep_verification_key_file: PathBuf,
        #[arg(long)]
        out_file: PathBuf,
    },
    /// Create an initial UTxO transaction
    InitialTxIn {
        #[arg(long)]
        genesis_utxo_verify_key_file: PathBuf,
        #[arg(long)]
        out_file: PathBuf,
    },
    /// Hash a genesis file
    Hash {
        #[arg(long)]
        genesis_file: PathBuf,
    },
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

impl GenesisCmd {
    pub fn run(self) -> Result<()> {
        match self.command {
            GenesisSubcommand::KeyGen {
                verification_key_file,
                signing_key_file,
            } => {
                let sk = dugite_crypto::keys::PaymentSigningKey::generate();
                let vk = sk.verification_key();

                let sk_env = serde_json::json!({
                    "type": "GenesisSigningKey_ed25519",
                    "description": "Genesis Signing Key",
                    "cborHex": hex::encode(simple_cbor_wrap(&sk.to_bytes()))
                });
                let vk_env = serde_json::json!({
                    "type": "GenesisVerificationKey_ed25519",
                    "description": "Genesis Verification Key",
                    "cborHex": hex::encode(simple_cbor_wrap(&vk.to_bytes()))
                });

                std::fs::write(&signing_key_file, serde_json::to_string_pretty(&sk_env)?)?;
                std::fs::write(
                    &verification_key_file,
                    serde_json::to_string_pretty(&vk_env)?,
                )?;

                println!("Genesis key pair generated.");
                Ok(())
            }
            GenesisSubcommand::KeyHash {
                verification_key_file,
            } => {
                let content = std::fs::read_to_string(&verification_key_file)?;
                let env: serde_json::Value = serde_json::from_str(&content)?;
                let cbor_hex = env["cborHex"].as_str().ok_or_else(|| {
                    anyhow::anyhow!("Missing cborHex in {}", verification_key_file.display())
                })?;
                let cbor_bytes = hex::decode(cbor_hex)?;
                let key_bytes = if cbor_bytes.len() > 2 {
                    &cbor_bytes[2..]
                } else {
                    &cbor_bytes
                };
                let hash = dugite_primitives::hash::blake2b_224(key_bytes);
                println!("{}", hash.to_hex());
                Ok(())
            }
            GenesisSubcommand::GenesisDelegation {
                genesis_verification_key_file,
                drep_verification_key_file,
                out_file,
            } => {
                let genesis_content = std::fs::read_to_string(&genesis_verification_key_file)?;
                let genesis_env: serde_json::Value = serde_json::from_str(&genesis_content)?;
                let genesis_cbor_hex = genesis_env["cborHex"].as_str().ok_or_else(|| {
                    anyhow::anyhow!(
                        "Missing cborHex in {}",
                        genesis_verification_key_file.display()
                    )
                })?;
                let genesis_cbor = hex::decode(genesis_cbor_hex)?;
                let genesis_key_bytes = if genesis_cbor.len() > 2 {
                    &genesis_cbor[2..]
                } else {
                    &genesis_cbor
                };
                let genesis_hash = dugite_primitives::hash::blake2b_224(genesis_key_bytes);

                let drep_content = std::fs::read_to_string(&drep_verification_key_file)?;
                let drep_env: serde_json::Value = serde_json::from_str(&drep_content)?;
                let drep_cbor_hex = drep_env["cborHex"].as_str().ok_or_else(|| {
                    anyhow::anyhow!(
                        "Missing cborHex in {}",
                        drep_verification_key_file.display()
                    )
                })?;
                let drep_cbor = hex::decode(drep_cbor_hex)?;
                let drep_key_bytes = if drep_cbor.len() > 2 {
                    &drep_cbor[2..]
                } else {
                    &drep_cbor
                };

                let mut cert_cbor = Vec::new();
                let mut enc = minicbor::Encoder::new(&mut cert_cbor);
                enc.array(4)?;
                enc.u32(3)?; // GenesisDelegation certificate type
                enc.bytes(genesis_hash.as_bytes())?;
                enc.bytes(drep_key_bytes)?;
                enc.u64(0)?; // epoch (placeholder)

                let cert_env = serde_json::json!({
                    "type": "CertificateShelley",
                    "description": "Genesis Delegation Certificate",
                    "cborHex": hex::encode(&cert_cbor)
                });

                std::fs::write(&out_file, serde_json::to_string_pretty(&cert_env)?)?;
                println!(
                    "Genesis delegation certificate written to: {}",
                    out_file.display()
                );
                Ok(())
            }
            GenesisSubcommand::InitialTxIn {
                genesis_utxo_verify_key_file,
                out_file,
            } => {
                let content = std::fs::read_to_string(&genesis_utxo_verify_key_file)?;
                let env: serde_json::Value = serde_json::from_str(&content)?;
                let cbor_hex = env["cborHex"].as_str().ok_or_else(|| {
                    anyhow::anyhow!(
                        "Missing cborHex in {}",
                        genesis_utxo_verify_key_file.display()
                    )
                })?;
                let cbor_bytes = hex::decode(cbor_hex)?;
                let key_bytes = if cbor_bytes.len() > 2 {
                    &cbor_bytes[2..]
                } else {
                    &cbor_bytes
                };
                let hash = dugite_primitives::hash::blake2b_224(key_bytes);

                let mut output = Vec::new();
                let mut enc = minicbor::Encoder::new(&mut output);
                enc.array(2)?;
                enc.bytes(hash.as_bytes())?;
                enc.u32(0)?; // output index

                let result = serde_json::json!({
                    "cborHex": hex::encode(&output),
                    "description": "Genesis initial UTxO",
                    "type": "GenesisUTxO"
                });

                std::fs::write(&out_file, serde_json::to_string_pretty(&result)?)?;
                println!("Genesis initial UTxO written to: {}", out_file.display());
                Ok(())
            }
            GenesisSubcommand::Hash { genesis_file } => {
                let content = std::fs::read_to_string(&genesis_file)?;
                let json: serde_json::Value = serde_json::from_str(&content)?;
                let canonical = serde_json::to_vec(&json)?;
                let hash = dugite_primitives::hash::blake2b_256(&canonical);
                println!("{}", hash.to_hex());
                Ok(())
            }
        }
    }
}
