use anyhow::Result;
use clap::{Args, Subcommand};
use std::path::PathBuf;
use torsten_crypto::keys::{PaymentVerificationKey, TextEnvelope};
use torsten_primitives::address::{Address, BaseAddress, EnterpriseAddress};
use torsten_primitives::credentials::Credential;
use torsten_primitives::network::NetworkId;

#[derive(Args, Debug)]
pub struct AddressCmd {
    #[command(subcommand)]
    command: AddressSubcommand,
}

#[derive(Subcommand, Debug)]
enum AddressSubcommand {
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

impl AddressCmd {
    pub fn run(self) -> Result<()> {
        match self.command {
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
