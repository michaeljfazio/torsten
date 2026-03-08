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
    /// Create stake pool retirement certificate
    RetirementCertificate {
        #[arg(long)]
        cold_verification_key_file: PathBuf,
        /// Epoch at which the pool retires
        #[arg(long)]
        epoch: u64,
        #[arg(long)]
        out_file: PathBuf,
    },
    /// Create stake pool registration certificate
    RegistrationCertificate {
        #[arg(long)]
        cold_verification_key_file: PathBuf,
        #[arg(long)]
        vrf_verification_key_file: PathBuf,
        #[arg(long)]
        pledge: u64,
        #[arg(long)]
        cost: u64,
        #[arg(long)]
        margin: f64,
        #[arg(long)]
        reward_account_verification_key_file: PathBuf,
        #[arg(long)]
        pool_owner_verification_key_file: Vec<PathBuf>,
        /// Pool relay: IP address (e.g., "1.2.3.4:3001")
        #[arg(long)]
        pool_relay_ipv4: Vec<String>,
        /// Pool relay: DNS hostname with port (e.g., "relay.example.com:3001")
        #[arg(long)]
        single_host_pool_relay: Vec<String>,
        /// Pool relay: DNS SRV record name (e.g., "_cardano._tcp.example.com")
        #[arg(long)]
        multi_host_pool_relay: Vec<String>,
        /// Pool metadata URL
        #[arg(long)]
        metadata_url: Option<String>,
        /// Pool metadata hash (hex)
        #[arg(long)]
        metadata_hash: Option<String>,
        #[arg(long)]
        out_file: PathBuf,
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

                let mut counter_cbor = Vec::new();
                let mut enc = minicbor::Encoder::new(&mut counter_cbor);
                enc.array(2)?;
                enc.u64(0)?;
                enc.bytes(&simple_cbor_wrap(&vk.to_bytes()))?;

                let counter = serde_json::json!({
                    "type": "NodeOperationalCertificateIssueCounter",
                    "description": "Next certificate issue number: 0",
                    "cborHex": hex::encode(&counter_cbor)
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
                println!("{pool_id}");
                Ok(())
            }
            StakePoolSubcommand::VrfKeyGen {
                verification_key_file,
                signing_key_file,
            } => {
                // VRF keys are 32-byte Ed25519 keys (same generation, different type label)
                let sk = torsten_crypto::keys::PaymentSigningKey::generate();
                let vk = sk.verification_key();

                let sk_env = serde_json::json!({
                    "type": "VrfSigningKey_PraosVRF",
                    "description": "VRF Signing Key",
                    "cborHex": hex::encode(simple_cbor_wrap(&sk.to_bytes()))
                });
                let vk_env = serde_json::json!({
                    "type": "VrfVerificationKey_PraosVRF",
                    "description": "VRF Verification Key",
                    "cborHex": hex::encode(simple_cbor_wrap(&vk.to_bytes()))
                });

                std::fs::write(&signing_key_file, serde_json::to_string_pretty(&sk_env)?)?;
                std::fs::write(
                    &verification_key_file,
                    serde_json::to_string_pretty(&vk_env)?,
                )?;

                println!("VRF key pair generated.");
                println!("VRF verification key hash: {}", vk.hash().to_hex());
                Ok(())
            }
            StakePoolSubcommand::KesKeyGen {
                verification_key_file,
                signing_key_file,
            } => {
                // KES keys are Ed25519 keys (placeholder - real KES uses sum composition)
                let sk = torsten_crypto::keys::PaymentSigningKey::generate();
                let vk = sk.verification_key();

                let sk_env = serde_json::json!({
                    "type": "KesSigningKey_ed25519_kes_2^6",
                    "description": "KES Signing Key",
                    "cborHex": hex::encode(simple_cbor_wrap(&sk.to_bytes()))
                });
                let vk_env = serde_json::json!({
                    "type": "KesVerificationKey_ed25519_kes_2^6",
                    "description": "KES Period Verification Key",
                    "cborHex": hex::encode(simple_cbor_wrap(&vk.to_bytes()))
                });

                std::fs::write(&signing_key_file, serde_json::to_string_pretty(&sk_env)?)?;
                std::fs::write(
                    &verification_key_file,
                    serde_json::to_string_pretty(&vk_env)?,
                )?;

                println!("KES key pair generated.");
                Ok(())
            }
            StakePoolSubcommand::IssueOpCert {
                kes_verification_key_file,
                cold_signing_key_file,
                operational_certificate_counter_file,
                kes_period,
                out_file,
            } => super::node::issue_op_cert(
                &kes_verification_key_file,
                &cold_signing_key_file,
                &operational_certificate_counter_file,
                kes_period,
                &out_file,
            ),
            StakePoolSubcommand::RetirementCertificate {
                cold_verification_key_file,
                epoch,
                out_file,
            } => {
                let pool_hash = load_vkey_hash(&cold_verification_key_file)?;

                // PoolRetirement (cert type 4) = [4, pool_hash, epoch]
                let mut cert_cbor = Vec::new();
                let mut enc = minicbor::Encoder::new(&mut cert_cbor);
                enc.array(3)?;
                enc.u32(4)?;
                enc.bytes(&pool_hash)?;
                enc.u64(epoch)?;

                let cert_env = serde_json::json!({
                    "type": "CertificateShelley",
                    "description": "Stake Pool Retirement Certificate",
                    "cborHex": hex::encode(&cert_cbor)
                });

                std::fs::write(&out_file, serde_json::to_string_pretty(&cert_env)?)?;
                println!(
                    "Pool retirement certificate written to: {}",
                    out_file.display()
                );
                println!("Pool retires at epoch: {epoch}");
                Ok(())
            }
            StakePoolSubcommand::RegistrationCertificate {
                cold_verification_key_file,
                vrf_verification_key_file,
                pledge,
                cost,
                margin,
                reward_account_verification_key_file,
                pool_owner_verification_key_file,
                pool_relay_ipv4,
                single_host_pool_relay,
                multi_host_pool_relay,
                metadata_url,
                metadata_hash,
                out_file,
            } => {
                // Read pool operator (cold) vkey
                let cold_vk = load_vkey_hash(&cold_verification_key_file)?;
                // Read VRF vkey
                let vrf_vk = load_vkey_hash(&vrf_verification_key_file)?;
                // Read reward account key
                let reward_vk = load_vkey_hash(&reward_account_verification_key_file)?;
                // Read pool owner keys
                let owners: Vec<Vec<u8>> = pool_owner_verification_key_file
                    .iter()
                    .map(|f| load_vkey_hash(f).map(|h| h.to_vec()))
                    .collect::<Result<_>>()?;

                // Convert margin to rational (find close fraction)
                let margin_num = (margin * 1_000_000.0) as u64;
                let margin_den = 1_000_000u64;

                // Build relay list
                let mut relays: Vec<RelaySpec> = Vec::new();
                for ipv4_str in &pool_relay_ipv4 {
                    let parts: Vec<&str> = ipv4_str.rsplitn(2, ':').collect();
                    let (port, ip) = if parts.len() == 2 {
                        (parts[0].parse::<u16>().unwrap_or(3001), parts[1])
                    } else {
                        (3001, ipv4_str.as_str())
                    };
                    let octets: Vec<u8> = ip.split('.').filter_map(|s| s.parse().ok()).collect();
                    if octets.len() == 4 {
                        relays.push(RelaySpec::SingleHostAddr {
                            port,
                            ipv4: [octets[0], octets[1], octets[2], octets[3]],
                        });
                    }
                }
                for dns_str in &single_host_pool_relay {
                    let parts: Vec<&str> = dns_str.rsplitn(2, ':').collect();
                    let (port, host) = if parts.len() == 2 {
                        (parts[0].parse::<u16>().unwrap_or(3001), parts[1])
                    } else {
                        (3001, dns_str.as_str())
                    };
                    relays.push(RelaySpec::SingleHostName {
                        port,
                        dns_name: host.to_string(),
                    });
                }
                for dns_name in &multi_host_pool_relay {
                    relays.push(RelaySpec::MultiHostName {
                        dns_name: dns_name.clone(),
                    });
                }

                // Build registration certificate CBOR
                // Certificate type 3 = PoolRegistration
                let mut cert_cbor = Vec::new();
                let mut enc = minicbor::Encoder::new(&mut cert_cbor);

                // [3, pool_params...]
                enc.array(10)?;
                enc.u32(3)?; // Certificate tag for PoolRegistration
                enc.bytes(&cold_vk)?; // operator (pool_id = hash of cold vkey)
                enc.bytes(&vrf_vk)?; // vrf_keyhash
                enc.u64(pledge)?;
                enc.u64(cost)?;
                // margin as tag 30 [num, den]
                enc.tag(minicbor::data::Tag::new(30))?;
                enc.array(2)?;
                enc.u64(margin_num)?;
                enc.u64(margin_den)?;
                // reward account (e1 prefix for stake key hash on mainnet)
                let mut reward_account = vec![0xe1u8];
                reward_account.extend_from_slice(&reward_vk);
                enc.bytes(&reward_account)?;
                // pool owners
                enc.array(owners.len() as u64)?;
                for owner in &owners {
                    enc.bytes(owner)?;
                }
                // relays
                enc.array(relays.len() as u64)?;
                for relay in &relays {
                    encode_relay(&mut enc, relay)?;
                }
                // pool metadata
                match (&metadata_url, &metadata_hash) {
                    (Some(url), Some(hash_hex)) => {
                        let hash_bytes = hex::decode(hash_hex)?;
                        enc.array(2)?;
                        enc.str(url)?;
                        enc.bytes(&hash_bytes)?;
                    }
                    _ => {
                        enc.null()?;
                    }
                }

                let cert_env = serde_json::json!({
                    "type": "CertificateShelley",
                    "description": "Stake Pool Registration Certificate",
                    "cborHex": hex::encode(&cert_cbor)
                });

                std::fs::write(&out_file, serde_json::to_string_pretty(&cert_env)?)?;
                println!(
                    "Pool registration certificate written to: {}",
                    out_file.display()
                );
                if !relays.is_empty() {
                    println!("  Relays: {}", relays.len());
                }
                if metadata_url.is_some() {
                    println!("  Metadata URL: {}", metadata_url.as_deref().unwrap_or(""));
                }
                Ok(())
            }
        }
    }
}

/// Relay specification for pool registration
enum RelaySpec {
    SingleHostAddr { port: u16, ipv4: [u8; 4] },
    SingleHostName { port: u16, dns_name: String },
    MultiHostName { dns_name: String },
}

/// Encode a relay as CBOR for the pool registration certificate
fn encode_relay(enc: &mut minicbor::Encoder<&mut Vec<u8>>, relay: &RelaySpec) -> Result<()> {
    match relay {
        RelaySpec::SingleHostAddr { port, ipv4 } => {
            // [0, port, ipv4, null(ipv6)]
            enc.array(4)?;
            enc.u32(0)?;
            enc.u16(*port)?;
            enc.bytes(ipv4)?;
            enc.null()?;
        }
        RelaySpec::SingleHostName { port, dns_name } => {
            // [1, port, dns_name]
            enc.array(3)?;
            enc.u32(1)?;
            enc.u16(*port)?;
            enc.str(dns_name)?;
        }
        RelaySpec::MultiHostName { dns_name } => {
            // [2, dns_name]
            enc.array(2)?;
            enc.u32(2)?;
            enc.str(dns_name)?;
        }
    }
    Ok(())
}

/// Load a verification key file and return the blake2b-224 hash of the raw key bytes
fn load_vkey_hash(path: &PathBuf) -> Result<Vec<u8>> {
    let content = std::fs::read_to_string(path)?;
    let env: serde_json::Value = serde_json::from_str(&content)?;
    let cbor_hex = env["cborHex"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing cborHex in {}", path.display()))?;
    let cbor_bytes = hex::decode(cbor_hex)?;
    let key_bytes = if cbor_bytes.len() > 2 {
        &cbor_bytes[2..]
    } else {
        &cbor_bytes
    };
    let hash = torsten_primitives::hash::blake2b_224(key_bytes);
    Ok(hash.as_bytes().to_vec())
}
