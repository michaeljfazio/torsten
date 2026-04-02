use anyhow::Result;
use clap::{Args, Subcommand};
use dugite_crypto::keys::{PaymentVerificationKey, TextEnvelope};
use dugite_primitives::address::{Address, BaseAddress, EnterpriseAddress};
use dugite_primitives::credentials::Credential;
use dugite_primitives::network::NetworkId;
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct AddressCmd {
    #[command(subcommand)]
    command: AddressSubcommand,
}

#[derive(Subcommand, Debug)]
enum AddressSubcommand {
    /// Generate a payment key pair
    KeyGen {
        /// Output verification key file
        #[arg(long)]
        verification_key_file: PathBuf,
        /// Output signing key file
        #[arg(long)]
        signing_key_file: PathBuf,
    },
    /// Get the hash of a verification key
    KeyHash {
        /// Payment verification key file
        #[arg(long)]
        payment_verification_key_file: PathBuf,
    },
    /// Build an address from verification keys
    Build {
        /// Payment verification key file
        #[arg(long)]
        payment_verification_key_file: PathBuf,
        /// Stake verification key file (optional - creates base address if provided)
        #[arg(long)]
        stake_verification_key_file: Option<PathBuf>,
        /// Network (mainnet or testnet)
        #[arg(long, default_value = "mainnet")]
        network: String,
        /// Output file (prints to stdout if not provided)
        #[arg(long)]
        out_file: Option<PathBuf>,
    },
    /// Show address info
    Info {
        /// Bech32 address
        #[arg(long)]
        address: String,
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

impl AddressCmd {
    pub fn run(self) -> Result<()> {
        match self.command {
            AddressSubcommand::KeyGen {
                verification_key_file,
                signing_key_file,
            } => {
                let sk = dugite_crypto::keys::PaymentSigningKey::generate();
                let vk = sk.verification_key();

                let sk_env = serde_json::json!({
                    "type": "PaymentSigningKeyShelley_ed25519",
                    "description": "Payment Signing Key",
                    "cborHex": hex::encode(simple_cbor_wrap(&sk.to_bytes()))
                });
                let vk_env = serde_json::json!({
                    "type": "PaymentVerificationKeyShelley_ed25519",
                    "description": "Payment Verification Key",
                    "cborHex": hex::encode(simple_cbor_wrap(&vk.to_bytes()))
                });

                std::fs::write(&signing_key_file, serde_json::to_string_pretty(&sk_env)?)?;
                std::fs::write(
                    &verification_key_file,
                    serde_json::to_string_pretty(&vk_env)?,
                )?;

                println!("Payment key pair generated.");
                println!("Verification key: {}", verification_key_file.display());
                println!("Signing key: {}", signing_key_file.display());
                Ok(())
            }
            AddressSubcommand::KeyHash {
                payment_verification_key_file,
            } => {
                let vk = load_verification_key(&payment_verification_key_file)?;
                let hash = vk.hash();
                println!("{}", hash.to_hex());
                Ok(())
            }
            AddressSubcommand::Build {
                payment_verification_key_file,
                stake_verification_key_file,
                network,
                out_file,
            } => {
                let network_id = match network.as_str() {
                    "mainnet" => NetworkId::Mainnet,
                    "testnet" | "testnet-magic" => NetworkId::Testnet,
                    _ => NetworkId::Testnet,
                };

                let payment_vk = load_verification_key(&payment_verification_key_file)?;
                let payment_hash = payment_vk.hash();
                let payment_cred = Credential::VerificationKey(payment_hash);

                let address = if let Some(stake_vk_file) = stake_verification_key_file {
                    let stake_vk = load_verification_key(&stake_vk_file)?;
                    let stake_hash = stake_vk.hash();
                    let stake_cred = Credential::VerificationKey(stake_hash);
                    Address::Base(BaseAddress {
                        network: network_id,
                        payment: payment_cred,
                        stake: stake_cred,
                    })
                } else {
                    Address::Enterprise(EnterpriseAddress {
                        network: network_id,
                        payment: payment_cred,
                    })
                };

                let addr_bytes = address.to_bytes();
                let hrp = match (&address, network_id) {
                    (
                        Address::Base(_) | Address::Enterprise(_) | Address::Pointer(_),
                        NetworkId::Mainnet,
                    ) => "addr",
                    (
                        Address::Base(_) | Address::Enterprise(_) | Address::Pointer(_),
                        NetworkId::Testnet,
                    ) => "addr_test",
                    _ => "addr",
                };

                let bech32_addr =
                    bech32::encode::<bech32::Bech32>(bech32::Hrp::parse(hrp)?, &addr_bytes)?;

                if let Some(out) = out_file {
                    std::fs::write(&out, &bech32_addr)?;
                    println!("Address written to: {}", out.display());
                } else {
                    println!("{}", bech32_addr);
                }

                Ok(())
            }
            AddressSubcommand::Info { address } => {
                println!("Address: {}", address);
                // Decode bech32 and show info
                let (hrp, data) = bech32::decode(&address)?;
                println!("HRP: {}", hrp);
                println!("Bytes: {} bytes", data.len());

                match Address::from_bytes(&data) {
                    Ok(addr) => match &addr {
                        Address::Base(a) => {
                            println!("Type: Base");
                            println!("Network: {:?}", a.network);
                            println!("Payment: {:?}", a.payment);
                            println!("Stake: {:?}", a.stake);
                        }
                        Address::Enterprise(a) => {
                            println!("Type: Enterprise");
                            println!("Network: {:?}", a.network);
                            println!("Payment: {:?}", a.payment);
                        }
                        Address::Reward(a) => {
                            println!("Type: Reward");
                            println!("Network: {:?}", a.network);
                            println!("Stake: {:?}", a.stake);
                        }
                        Address::Pointer(a) => {
                            println!("Type: Pointer");
                            println!("Network: {:?}", a.network);
                        }
                        Address::Byron(_) => {
                            println!("Type: Byron (legacy)");
                        }
                    },
                    Err(e) => {
                        println!("Could not decode address: {}", e);
                    }
                }

                Ok(())
            }
        }
    }
}

fn load_verification_key(path: &PathBuf) -> Result<PaymentVerificationKey> {
    let content = std::fs::read_to_string(path)?;
    let envelope: TextEnvelope = serde_json::from_str(&content)?;
    let cbor_bytes = hex::decode(&envelope.cbor_hex)?;
    let key_bytes = if cbor_bytes.len() > 2 {
        &cbor_bytes[2..]
    } else {
        &cbor_bytes
    };
    Ok(PaymentVerificationKey::from_bytes(key_bytes)?)
}
