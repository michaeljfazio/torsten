use anyhow::{Context, Result};
use std::path::Path;
use torsten_primitives::block::{Block, BlockHeader, OperationalCert, ProtocolVersion, VrfOutput};
use torsten_primitives::era::Era;
use torsten_primitives::hash::{blake2b_256, Hash28, Hash32};
use torsten_primitives::time::{BlockNo, SlotNo};
use torsten_primitives::transaction::Transaction;
use tracing::{debug, info};

/// Block producer credentials loaded from disk
#[allow(dead_code)]
pub struct BlockProducerCredentials {
    /// VRF secret key (32 bytes)
    pub vrf_skey: [u8; 32],
    /// VRF verification key (32 bytes)
    pub vrf_vkey: [u8; 32],
    /// Cold signing key for operational certificate verification
    pub cold_skey: torsten_crypto::keys::PaymentSigningKey,
    /// Cold verification key
    pub cold_vkey: Vec<u8>,
    /// KES secret key bytes (Sum6Kes format, 612 bytes)
    pub kes_skey: Vec<u8>,
    /// KES verification key (hot key from opcert)
    pub kes_vkey: Vec<u8>,
    /// Operational certificate sequence number
    pub opcert_sequence: u64,
    /// KES period from the operational certificate
    pub opcert_kes_period: u64,
    /// Operational certificate cold key signature
    pub opcert_sigma: Vec<u8>,
    /// Pool ID (hash of cold verification key)
    pub pool_id: Hash28,
}

impl BlockProducerCredentials {
    /// Load block producer credentials from the given file paths.
    ///
    /// Requires all four keys: VRF signing key, KES signing key, operational
    /// certificate, and cold signing key. The cold key is needed to derive
    /// the pool ID (blake2b-224 of the cold verification key).
    pub fn load_with_cold_key(
        vrf_skey_path: &Path,
        kes_skey_path: &Path,
        opcert_path: &Path,
        cold_skey_path: &Path,
    ) -> Result<Self> {
        // Load VRF signing key
        let vrf_content = std::fs::read_to_string(vrf_skey_path)
            .with_context(|| format!("Failed to read VRF skey: {}", vrf_skey_path.display()))?;
        let vrf_env: serde_json::Value = serde_json::from_str(&vrf_content)?;
        let vrf_cbor_hex = vrf_env["cborHex"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing cborHex in VRF skey file"))?;
        let vrf_cbor = hex::decode(vrf_cbor_hex)?;
        let vrf_key_bytes = unwrap_cbor(&vrf_cbor);
        let mut vrf_skey = [0u8; 32];
        if vrf_key_bytes.len() != 32 {
            anyhow::bail!(
                "VRF secret key must be 32 bytes, got {}",
                vrf_key_bytes.len()
            );
        }
        vrf_skey.copy_from_slice(vrf_key_bytes);

        // Derive VRF public key from secret key
        let vrf_keypair = torsten_crypto::vrf::generate_vrf_keypair_from_secret(&vrf_skey);
        let vrf_vkey = vrf_keypair.public_key;

        // Load KES signing key
        let kes_content = std::fs::read_to_string(kes_skey_path)
            .with_context(|| format!("Failed to read KES skey: {}", kes_skey_path.display()))?;
        let kes_env: serde_json::Value = serde_json::from_str(&kes_content)?;
        let kes_cbor_hex = kes_env["cborHex"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing cborHex in KES skey file"))?;
        let kes_cbor = hex::decode(kes_cbor_hex)?;
        let kes_key_bytes = unwrap_cbor(&kes_cbor).to_vec();

        // Load operational certificate
        let opcert_content = std::fs::read_to_string(opcert_path)
            .with_context(|| format!("Failed to read opcert: {}", opcert_path.display()))?;
        let opcert_env: serde_json::Value = serde_json::from_str(&opcert_content)?;
        let opcert_cbor_hex = opcert_env["cborHex"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing cborHex in opcert file"))?;
        let opcert_cbor = hex::decode(opcert_cbor_hex)?;

        // Parse opcert CBOR: [hot_vkey, sequence_number, kes_period, sigma]
        let mut decoder = minicbor::Decoder::new(&opcert_cbor);
        let _array_len = decoder.array()?;
        let kes_vkey_bytes = decoder.bytes()?.to_vec();
        let opcert_sequence = decoder.u64()?;
        let opcert_kes_period = decoder.u64()?;
        let opcert_sigma = decoder.bytes()?.to_vec();

        // Load cold signing key
        let cold_content = std::fs::read_to_string(cold_skey_path)
            .with_context(|| format!("Failed to read cold skey: {}", cold_skey_path.display()))?;
        let cold_env: serde_json::Value = serde_json::from_str(&cold_content)?;
        let cold_cbor_hex = cold_env["cborHex"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing cborHex in cold skey file"))?;
        let cold_cbor = hex::decode(cold_cbor_hex)?;
        let cold_key_bytes = unwrap_cbor(&cold_cbor);
        let cold_skey = torsten_crypto::keys::PaymentSigningKey::from_bytes(cold_key_bytes)?;
        let cold_vk = cold_skey.verification_key();
        let cold_vkey = cold_vk.to_bytes().to_vec();

        // Pool ID = blake2b-224 of the cold verification key
        let pool_id = torsten_primitives::hash::blake2b_224(&cold_vk.to_bytes());

        Ok(BlockProducerCredentials {
            vrf_skey,
            vrf_vkey,
            cold_skey,
            cold_vkey,
            kes_skey: kes_key_bytes,
            kes_vkey: kes_vkey_bytes,
            opcert_sequence,
            opcert_kes_period,
            opcert_sigma,
            pool_id,
        })
    }
}

/// Strip CBOR byte string wrapper (0x58 0x20 prefix or short form)
fn unwrap_cbor(data: &[u8]) -> &[u8] {
    if data.len() > 2 && data[0] == 0x58 {
        &data[2..]
    } else if data.len() > 1 && (data[0] & 0xe0) == 0x40 {
        &data[1..]
    } else {
        data
    }
}

/// Configuration for the block producer
#[allow(dead_code)]
pub struct BlockProducerConfig {
    /// Protocol version to stamp on forged blocks
    pub protocol_version: ProtocolVersion,
    /// Maximum block body size
    pub max_block_body_size: u64,
    /// Maximum number of transactions per block
    pub max_txs_per_block: usize,
}

impl Default for BlockProducerConfig {
    fn default() -> Self {
        BlockProducerConfig {
            protocol_version: ProtocolVersion { major: 9, minor: 0 },
            max_block_body_size: 90112,
            max_txs_per_block: 500,
        }
    }
}

/// Forge a new block from mempool transactions.
///
/// Returns the constructed block and its CBOR encoding.
/// The block header is signed with the KES secret key at the appropriate period.
pub fn forge_block(
    creds: &BlockProducerCredentials,
    config: &BlockProducerConfig,
    slot: SlotNo,
    block_number: BlockNo,
    prev_hash: Hash32,
    epoch_nonce: &Hash32,
    transactions: Vec<Transaction>,
) -> Result<(Block, Vec<u8>)> {
    // Generate VRF proof for this slot
    let vrf_seed = torsten_consensus::slot_leader::vrf_input(epoch_nonce, slot);
    let (vrf_proof, vrf_output) =
        torsten_crypto::vrf::generate_vrf_proof(&creds.vrf_skey, &vrf_seed)
            .map_err(|e| anyhow::anyhow!("VRF proof generation failed: {e}"))?;

    // Encode transaction bodies to compute body hash and size
    let body_hash = torsten_serialization::compute_block_body_hash(&transactions);
    let body_size = compute_body_size(&transactions);

    // Build the block header
    let header = BlockHeader {
        header_hash: Hash32::ZERO, // Will be set after encoding
        prev_hash,
        issuer_vkey: creds.cold_vkey.clone(),
        vrf_vkey: creds.vrf_vkey.to_vec(),
        vrf_result: VrfOutput {
            output: vrf_output.to_vec(),
            proof: vrf_proof.to_vec(),
        },
        block_number,
        slot,
        epoch_nonce: *epoch_nonce,
        body_size,
        body_hash,
        operational_cert: OperationalCert {
            hot_vkey: creds.kes_vkey.clone(),
            sequence_number: creds.opcert_sequence,
            kes_period: creds.opcert_kes_period,
            sigma: creds.opcert_sigma.clone(),
        },
        protocol_version: config.protocol_version,
        kes_signature: vec![], // Set after signing below
    };

    // Encode the header body for hashing and KES signing
    let header_body_cbor = torsten_serialization::encode_block_header_body(&header);
    let header_hash = blake2b_256(&header_body_cbor);

    // KES signing: evolve key to the correct period and sign the header body
    let current_slot_kes_period = torsten_crypto::kes::kes_period_for_slot(slot.0);
    let kes_period_offset = current_slot_kes_period.saturating_sub(creds.opcert_kes_period);

    let kes_signature = if !creds.kes_skey.is_empty() {
        let evolved_kes =
            torsten_crypto::kes::kes_evolve_to_period(&creds.kes_skey, kes_period_offset as u32)
                .map_err(|e| anyhow::anyhow!("KES key evolution failed: {e}"))?;

        let (sig_bytes, period) =
            torsten_crypto::kes::kes_sign_bytes(&evolved_kes, &header_body_cbor)
                .map_err(|e| anyhow::anyhow!("KES signing failed: {e}"))?;

        debug!(
            kes_period = period,
            slot_kes_period = current_slot_kes_period,
            opcert_kes_period = creds.opcert_kes_period,
            "KES signature produced"
        );
        sig_bytes
    } else {
        anyhow::bail!("Cannot forge block: KES secret key is empty");
    };

    // Build the final block with correct header hash
    let mut block = Block {
        header,
        transactions,
        era: Era::Conway,
        raw_cbor: None,
    };
    block.header.header_hash = header_hash;
    block.header.kes_signature = kes_signature.clone();

    // Encode the full block
    let block_cbor = torsten_serialization::encode_block(&block, &kes_signature);
    block.raw_cbor = Some(block_cbor.clone());

    info!(
        slot = slot.0,
        block_number = block_number.0,
        tx_count = block.transactions.len(),
        body_size = body_size,
        header_hash = %header_hash,
        "Block forged"
    );

    Ok((block, block_cbor))
}

/// Compute approximate body size from transactions
fn compute_body_size(transactions: &[Transaction]) -> u64 {
    let mut size: u64 = 0;
    for tx in transactions {
        if let Some(ref cbor) = tx.raw_cbor {
            size += cbor.len() as u64;
        } else {
            // Estimate from encoding
            let encoded = torsten_serialization::encode_transaction(tx);
            size += encoded.len() as u64;
        }
    }
    size
}

/// Check if we are the slot leader for a given slot
pub fn check_slot_leadership(
    creds: &BlockProducerCredentials,
    slot: SlotNo,
    epoch_nonce: &Hash32,
    relative_stake: f64,
    active_slot_coeff: f64,
) -> bool {
    let vrf_seed = torsten_consensus::slot_leader::vrf_input(epoch_nonce, slot);
    match torsten_crypto::vrf::generate_vrf_proof(&creds.vrf_skey, &vrf_seed) {
        Ok((_proof, output)) => torsten_consensus::slot_leader::is_slot_leader(
            &output,
            relative_stake,
            active_slot_coeff,
        ),
        Err(e) => {
            debug!("VRF proof failed for slot {}: {e}", slot.0);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use torsten_primitives::transaction::TransactionBody;

    fn make_test_credentials() -> BlockProducerCredentials {
        let vrf_kp = torsten_crypto::vrf::generate_vrf_keypair();
        let cold_sk = torsten_crypto::keys::PaymentSigningKey::generate();
        let cold_vk = cold_sk.verification_key();

        // Generate a KES key pair for testing
        let seed = [42u8; 32];
        let (kes_sk, kes_pk) = torsten_crypto::kes::kes_keygen(&seed).unwrap();

        BlockProducerCredentials {
            vrf_skey: vrf_kp.secret_key,
            vrf_vkey: vrf_kp.public_key,
            cold_skey: cold_sk,
            cold_vkey: cold_vk.to_bytes().to_vec(),
            kes_skey: kes_sk,
            kes_vkey: kes_pk.to_vec(),
            opcert_sequence: 0,
            opcert_kes_period: 0,
            opcert_sigma: vec![0u8; 64],
            pool_id: torsten_primitives::hash::blake2b_224(&cold_vk.to_bytes()),
        }
    }

    #[test]
    fn test_forge_empty_block() {
        let creds = make_test_credentials();
        let config = BlockProducerConfig::default();
        let epoch_nonce = Hash32::from_bytes([42u8; 32]);

        let (block, cbor) = forge_block(
            &creds,
            &config,
            SlotNo(1000),
            BlockNo(100),
            Hash32::ZERO,
            &epoch_nonce,
            vec![],
        )
        .expect("forge_block should succeed");

        assert_eq!(block.transactions.len(), 0);
        assert_eq!(block.header.slot, SlotNo(1000));
        assert_eq!(block.header.block_number, BlockNo(100));
        assert_ne!(block.header.header_hash, Hash32::ZERO);
        assert!(!cbor.is_empty());
    }

    #[test]
    fn test_forge_block_with_transactions() {
        let creds = make_test_credentials();
        let config = BlockProducerConfig::default();
        let epoch_nonce = Hash32::from_bytes([42u8; 32]);

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![torsten_primitives::transaction::TransactionInput {
                    transaction_id: Hash32::ZERO,
                    index: 0,
                }],
                outputs: vec![torsten_primitives::transaction::TransactionOutput {
                    address: torsten_primitives::address::Address::Enterprise(
                        torsten_primitives::address::EnterpriseAddress {
                            network: torsten_primitives::network::NetworkId::Mainnet,
                            payment: torsten_primitives::credentials::Credential::VerificationKey(
                                Hash28::from_bytes([0u8; 28]),
                            ),
                        },
                    ),
                    value: torsten_primitives::value::Value::lovelace(1_000_000),
                    datum: torsten_primitives::transaction::OutputDatum::None,
                    script_ref: None,
                    raw_cbor: None,
                }],
                fee: torsten_primitives::value::Lovelace(200_000),
                ttl: None,
                certificates: vec![],
                withdrawals: BTreeMap::new(),
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint: BTreeMap::new(),
                script_data_hash: None,
                collateral: vec![],
                required_signers: vec![],
                network_id: None,
                collateral_return: None,
                total_collateral: None,
                reference_inputs: vec![],
                update: None,
                voting_procedures: BTreeMap::new(),
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: torsten_primitives::transaction::TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                redeemers: vec![],
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let (block, _cbor) = forge_block(
            &creds,
            &config,
            SlotNo(2000),
            BlockNo(200),
            Hash32::ZERO,
            &epoch_nonce,
            vec![tx],
        )
        .expect("forge_block should succeed");

        assert_eq!(block.transactions.len(), 1);
        assert!(block.header.body_size > 0);
    }

    #[test]
    fn test_check_slot_leadership() {
        let creds = make_test_credentials();
        let epoch_nonce = Hash32::from_bytes([42u8; 32]);

        // With 100% stake, should be leader for some slots
        let mut leader_count = 0;
        for i in 0..100 {
            if check_slot_leadership(&creds, SlotNo(i), &epoch_nonce, 1.0, 0.05) {
                leader_count += 1;
            }
        }

        // With f=0.05 and 100% stake, expect ~5 leader slots out of 100
        assert!(leader_count > 0, "Should win some slots with 100% stake");
        assert!(
            leader_count < 50,
            "Should not win too many slots with f=0.05"
        );
    }

    #[test]
    fn test_check_slot_leadership_zero_stake() {
        let creds = make_test_credentials();
        let epoch_nonce = Hash32::from_bytes([42u8; 32]);

        for i in 0..100 {
            assert!(
                !check_slot_leadership(&creds, SlotNo(i), &epoch_nonce, 0.0, 0.05),
                "Zero stake should never be leader"
            );
        }
    }

    #[test]
    fn test_header_hash_deterministic() {
        let creds = make_test_credentials();
        let config = BlockProducerConfig::default();
        let epoch_nonce = Hash32::from_bytes([42u8; 32]);

        let (block1, _) = forge_block(
            &creds,
            &config,
            SlotNo(1000),
            BlockNo(100),
            Hash32::ZERO,
            &epoch_nonce,
            vec![],
        )
        .unwrap();

        let (block2, _) = forge_block(
            &creds,
            &config,
            SlotNo(1000),
            BlockNo(100),
            Hash32::ZERO,
            &epoch_nonce,
            vec![],
        )
        .unwrap();

        assert_eq!(block1.header.header_hash, block2.header.header_hash);
    }
}
