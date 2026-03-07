use anyhow::Result;
use clap::{Args, Subcommand};
use std::path::PathBuf;
use torsten_crypto::keys::{PaymentSigningKey, TextEnvelope};

#[derive(Args, Debug)]
pub struct KeyCmd {
    #[command(subcommand)]
    command: KeySubcommand,
}

#[derive(Subcommand, Debug)]
enum KeySubcommand {
    /// Generate a payment key pair
    GeneratePaymentKey {
        /// Output path for the signing key
        #[arg(long)]
        signing_key_file: PathBuf,
        /// Output path for the verification key
        #[arg(long)]
        verification_key_file: PathBuf,
    },
    /// Generate a stake key pair
    GenerateStakeKey {
        /// Output path for the signing key
        #[arg(long)]
        signing_key_file: PathBuf,
        /// Output path for the verification key
        #[arg(long)]
        verification_key_file: PathBuf,
    },
    /// Get the verification key hash
    VerificationKeyHash {
        /// Path to the verification key file
        #[arg(long)]
        verification_key_file: PathBuf,
    },
}

impl KeyCmd {
    pub fn run(self) -> Result<()> {
        match self.command {
            KeySubcommand::GeneratePaymentKey {
                signing_key_file,
                verification_key_file,
            } => {
                let sk = PaymentSigningKey::generate();
                let vk = sk.verification_key();

                let sk_envelope = TextEnvelope::payment_signing_key(&sk);
                let vk_envelope = TextEnvelope::payment_verification_key(&vk);

                let sk_json = serde_json::to_string_pretty(&sk_envelope)?;
                let vk_json = serde_json::to_string_pretty(&vk_envelope)?;

                std::fs::write(&signing_key_file, sk_json)?;
                std::fs::write(&verification_key_file, vk_json)?;

                println!(
                    "Payment signing key written to: {}",
                    signing_key_file.display()
                );
                println!(
                    "Payment verification key written to: {}",
                    verification_key_file.display()
                );
                Ok(())
            }
            KeySubcommand::GenerateStakeKey {
                signing_key_file,
                verification_key_file,
            } => {
                let sk = PaymentSigningKey::generate();
                let vk = sk.verification_key();

                let sk_envelope = TextEnvelope::stake_signing_key(&sk);
                let vk_envelope = TextEnvelope::stake_verification_key(&vk);

                let sk_json = serde_json::to_string_pretty(&sk_envelope)?;
                let vk_json = serde_json::to_string_pretty(&vk_envelope)?;

                std::fs::write(&signing_key_file, sk_json)?;
                std::fs::write(&verification_key_file, vk_json)?;

                println!(
                    "Stake signing key written to: {}",
                    signing_key_file.display()
                );
                println!(
                    "Stake verification key written to: {}",
                    verification_key_file.display()
                );
                Ok(())
            }
            KeySubcommand::VerificationKeyHash {
                verification_key_file,
            } => {
                let content = std::fs::read_to_string(&verification_key_file)?;
                let envelope: TextEnvelope = serde_json::from_str(&content)?;

                let cbor_bytes = hex::decode(&envelope.cbor_hex)?;
                // Extract the raw key bytes from CBOR wrapper
                let key_bytes = if cbor_bytes.len() > 2 {
                    &cbor_bytes[2..] // Skip CBOR byte string header
                } else {
                    &cbor_bytes
                };

                let vk = torsten_crypto::keys::PaymentVerificationKey::from_bytes(key_bytes)?;
                let hash = vk.hash();

                println!("{}", hash.to_hex());
                Ok(())
            }
        }
    }
}
