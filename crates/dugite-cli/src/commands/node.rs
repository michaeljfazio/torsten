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
    /// Generate node cold keys
    KeyGen {
        #[arg(long)]
        cold_verification_key_file: PathBuf,
        #[arg(long)]
        cold_signing_key_file: PathBuf,
        #[arg(long)]
        operational_certificate_counter_file: PathBuf,
    },
    /// Generate a KES key pair
    KeyGenKes {
        #[arg(long)]
        verification_key_file: PathBuf,
        #[arg(long)]
        signing_key_file: PathBuf,
    },
    /// Generate a VRF key pair
    KeyGenVrf {
        #[arg(long)]
        verification_key_file: PathBuf,
        #[arg(long)]
        signing_key_file: PathBuf,
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
    /// Create a new operational certificate issue counter
    NewCounter {
        #[arg(long)]
        cold_verification_key_file: PathBuf,
        #[arg(long)]
        counter_value: u64,
        #[arg(long)]
        operational_certificate_counter_file: PathBuf,
    },
    /// Get the hash of a VRF verification key
    KeyHashVrf {
        #[arg(long)]
        verification_key_file: PathBuf,
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

impl NodeCmd {
    pub fn run(self) -> Result<()> {
        match self.command {
            NodeSubcommand::KeyGen {
                cold_verification_key_file,
                cold_signing_key_file,
                operational_certificate_counter_file,
            } => {
                let sk = dugite_crypto::keys::PaymentSigningKey::generate();
                let vk = sk.verification_key();

                let sk_env = serde_json::json!({
                    "type": "StakePoolSigningKey_ed25519",
                    "description": "Stake Pool Operator Cold Signing Key",
                    "cborHex": hex::encode(simple_cbor_wrap(&sk.to_bytes()))
                });
                let vk_env = serde_json::json!({
                    "type": "StakePoolVerificationKey_ed25519",
                    "description": "Stake Pool Operator Cold Verification Key",
                    "cborHex": hex::encode(simple_cbor_wrap(&vk.to_bytes()))
                });

                // Counter starts at 0, includes the cold vkey
                let mut counter_cbor = Vec::new();
                let mut enc = minicbor::Encoder::new(&mut counter_cbor);
                enc.array(2)?;
                enc.u64(0)?; // counter starts at 0
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

                println!("Node cold keys generated.");
                println!(
                    "Cold verification key: {}",
                    cold_verification_key_file.display()
                );
                println!("Cold signing key: {}", cold_signing_key_file.display());
                println!(
                    "Counter: {}",
                    operational_certificate_counter_file.display()
                );
                Ok(())
            }
            NodeSubcommand::KeyGenKes {
                verification_key_file,
                signing_key_file,
            } => {
                // Generate proper Sum6Kes key pair (depth-6 binary sum composition)
                use rand::RngCore;
                let mut seed = [0u8; 32];
                rand::thread_rng().fill_bytes(&mut seed);

                let (sk_bytes, pk_bytes) = dugite_crypto::kes::kes_keygen(&seed)
                    .map_err(|e| anyhow::anyhow!("KES key generation failed: {e}"))?;

                let sk_env = serde_json::json!({
                    "type": "KesSigningKey_ed25519_kes_2^6",
                    "description": "KES Signing Key",
                    "cborHex": hex::encode(simple_cbor_wrap(&sk_bytes))
                });
                let vk_env = serde_json::json!({
                    "type": "KesVerificationKey_ed25519_kes_2^6",
                    "description": "KES Period Verification Key",
                    "cborHex": hex::encode(simple_cbor_wrap(&pk_bytes))
                });

                std::fs::write(&signing_key_file, serde_json::to_string_pretty(&sk_env)?)?;
                std::fs::write(
                    &verification_key_file,
                    serde_json::to_string_pretty(&vk_env)?,
                )?;

                println!("KES key pair generated.");
                println!("Verification key: {}", verification_key_file.display());
                println!("Signing key: {}", signing_key_file.display());
                Ok(())
            }
            NodeSubcommand::KeyGenVrf {
                verification_key_file,
                signing_key_file,
            } => {
                let kp = dugite_crypto::vrf::generate_vrf_keypair();

                let sk_env = serde_json::json!({
                    "type": "VrfSigningKey_PraosVRF",
                    "description": "VRF Signing Key",
                    "cborHex": hex::encode(simple_cbor_wrap(kp.secret_key()))
                });
                let vk_env = serde_json::json!({
                    "type": "VrfVerificationKey_PraosVRF",
                    "description": "VRF Verification Key",
                    "cborHex": hex::encode(simple_cbor_wrap(&kp.public_key))
                });

                std::fs::write(&signing_key_file, serde_json::to_string_pretty(&sk_env)?)?;
                std::fs::write(
                    &verification_key_file,
                    serde_json::to_string_pretty(&vk_env)?,
                )?;

                println!("VRF key pair generated.");
                println!("Verification key: {}", verification_key_file.display());
                println!("Signing key: {}", signing_key_file.display());
                Ok(())
            }
            NodeSubcommand::IssueOpCert {
                kes_verification_key_file,
                cold_signing_key_file,
                operational_certificate_counter_file,
                kes_period,
                out_file,
            } => issue_op_cert(
                &kes_verification_key_file,
                &cold_signing_key_file,
                &operational_certificate_counter_file,
                kes_period,
                &out_file,
            ),
            NodeSubcommand::NewCounter {
                cold_verification_key_file,
                counter_value,
                operational_certificate_counter_file,
            } => {
                let vk_content = std::fs::read_to_string(&cold_verification_key_file)?;
                let vk_env: serde_json::Value = serde_json::from_str(&vk_content)?;
                let vk_cbor_hex = vk_env["cborHex"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing cborHex in cold vkey file"))?;
                let vk_cbor = hex::decode(vk_cbor_hex)?;

                let mut counter_cbor = Vec::new();
                let mut enc = minicbor::Encoder::new(&mut counter_cbor);
                enc.array(2)?;
                enc.u64(counter_value)?;
                enc.bytes(&vk_cbor)?;

                let counter_env = serde_json::json!({
                    "type": "NodeOperationalCertificateIssueCounter",
                    "description": format!("Next certificate issue number: {counter_value}"),
                    "cborHex": hex::encode(&counter_cbor)
                });
                std::fs::write(
                    &operational_certificate_counter_file,
                    serde_json::to_string_pretty(&counter_env)?,
                )?;

                println!("Counter created: {counter_value}");
                println!(
                    "Counter file: {}",
                    operational_certificate_counter_file.display()
                );
                Ok(())
            }
            NodeSubcommand::KeyHashVrf {
                verification_key_file,
            } => {
                let content = std::fs::read_to_string(&verification_key_file)?;
                let env: serde_json::Value = serde_json::from_str(&content)?;
                let cbor_hex = env["cborHex"].as_str().ok_or_else(|| {
                    anyhow::anyhow!("Missing cborHex in {}", verification_key_file.display())
                })?;
                let cbor = hex::decode(cbor_hex)?;
                let vrf_key_bytes = if cbor.len() > 2 && cbor[0] == 0x58 {
                    &cbor[2..]
                } else if cbor.len() > 1 && (cbor[0] & 0xe0) == 0x40 {
                    &cbor[1..]
                } else {
                    &cbor
                };
                let hash = dugite_primitives::hash::blake2b_256(vrf_key_bytes);
                println!("{}", hex::encode(hash.as_bytes()));
                Ok(())
            }
        }
    }
}

/// Issue an operational certificate. Shared between `node issue-op-cert` and `stake-pool issue-op-cert`.
pub fn issue_op_cert(
    kes_verification_key_file: &PathBuf,
    cold_signing_key_file: &PathBuf,
    operational_certificate_counter_file: &PathBuf,
    kes_period: u64,
    out_file: &PathBuf,
) -> Result<()> {
    // Read the KES verification key
    let kes_content = std::fs::read_to_string(kes_verification_key_file)?;
    let kes_env: serde_json::Value = serde_json::from_str(&kes_content)?;
    let kes_cbor_hex = kes_env["cborHex"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing cborHex in KES vkey file"))?;
    let kes_cbor = hex::decode(kes_cbor_hex)?;
    let kes_vkey = if kes_cbor.len() > 2 {
        &kes_cbor[2..]
    } else {
        &kes_cbor
    };

    // Read the cold signing key
    let cold_content = std::fs::read_to_string(cold_signing_key_file)?;
    let cold_env: serde_json::Value = serde_json::from_str(&cold_content)?;
    let cold_cbor_hex = cold_env["cborHex"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing cborHex in cold skey file"))?;
    let cold_cbor = hex::decode(cold_cbor_hex)?;
    let cold_key_bytes = if cold_cbor.len() > 2 {
        &cold_cbor[2..]
    } else {
        &cold_cbor
    };
    let cold_sk = dugite_crypto::keys::PaymentSigningKey::from_bytes(cold_key_bytes)?;

    // Read the counter
    let counter_content = std::fs::read_to_string(operational_certificate_counter_file)?;
    let counter_env: serde_json::Value = serde_json::from_str(&counter_content)?;
    let counter_cbor_hex = counter_env["cborHex"].as_str().unwrap_or("8200");
    let counter_cbor = hex::decode(counter_cbor_hex)?;

    // Parse counter value
    let mut decoder = minicbor::Decoder::new(&counter_cbor);
    let _ = decoder.array();
    let counter_value = decoder.u64().unwrap_or(0);

    // Build the operational certificate body to sign:
    // [hot_vkey, sequence_number, kes_period]
    let mut cert_body = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut cert_body);
    enc.array(3)?;
    enc.bytes(kes_vkey)?;
    enc.u64(counter_value)?;
    enc.u64(kes_period)?;

    // Sign with the cold key
    let signature = cold_sk.sign(&cert_body);

    // Build the full operational certificate matching Haskell's OperationalCertificate:
    // array(2) [ocert, cold_vkey]
    // where ocert = array(4) [hot_vkey, sequence_number, kes_period, cold_key_signature]
    let cold_vk = cold_sk.verification_key();
    let mut opcert_cbor = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut opcert_cbor);
    enc.array(2)?;
    // [0] OCert body
    enc.array(4)?;
    enc.bytes(kes_vkey)?;
    enc.u64(counter_value)?;
    enc.u64(kes_period)?;
    enc.bytes(&signature)?;
    // [1] Cold verification key (raw 32 bytes)
    enc.bytes(&cold_vk.to_bytes())?;

    let opcert_env = serde_json::json!({
        "type": "NodeOperationalCertificate",
        "description": "",
        "cborHex": hex::encode(&opcert_cbor)
    });

    std::fs::write(out_file, serde_json::to_string_pretty(&opcert_env)?)?;

    // Increment the counter
    let new_counter = counter_value + 1;
    let mut new_counter_cbor = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut new_counter_cbor);
    enc.array(2)?;
    enc.u64(new_counter)?;
    enc.bytes(&simple_cbor_wrap(&cold_vk.to_bytes()))?;

    let new_counter_env = serde_json::json!({
        "type": "NodeOperationalCertificateIssueCounter",
        "description": format!("Next certificate issue number: {new_counter}"),
        "cborHex": hex::encode(&new_counter_cbor)
    });
    std::fs::write(
        operational_certificate_counter_file,
        serde_json::to_string_pretty(&new_counter_env)?,
    )?;

    println!("Operational certificate issued.");
    println!("Certificate: {}", out_file.display());
    println!("KES period: {kes_period}");
    println!("Counter: {counter_value} -> {new_counter}");
    Ok(())
}
