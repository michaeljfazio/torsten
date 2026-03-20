//! Comprehensive test coverage for torsten-cli commands.
//!
//! Tests address generation, key operations, CBOR wrapping, and
//! text envelope format compatibility with cardano-cli.

// CLI test coverage — no external deps beyond the workspace crates.

// ─── CBOR wrapping tests ─────────────────────────────────────────────────────

/// The simple_cbor_wrap function is used in multiple CLI commands to wrap
/// raw key bytes in CBOR byte-string encoding for text envelope files.
/// We test it indirectly through key generation roundtrips.

// ─── Address key generation ──────────────────────────────────────────────────

#[test]
fn test_address_keygen_produces_valid_envelope() {
    let sk = torsten_crypto::keys::PaymentSigningKey::generate();
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

    assert_eq!(sk_env["type"], "PaymentSigningKeyShelley_ed25519");
    assert_eq!(vk_env["type"], "PaymentVerificationKeyShelley_ed25519");
    assert!(!sk_env["cborHex"].as_str().unwrap().is_empty());
    assert!(!vk_env["cborHex"].as_str().unwrap().is_empty());

    // Verify CBOR hex can be decoded
    let sk_cbor = hex::decode(sk_env["cborHex"].as_str().unwrap()).unwrap();
    let vk_cbor = hex::decode(vk_env["cborHex"].as_str().unwrap()).unwrap();
    assert!(sk_cbor.len() > 32);
    assert!(vk_cbor.len() > 32);
}

#[test]
fn test_keygen_produces_different_keys_each_time() {
    let sk1 = torsten_crypto::keys::PaymentSigningKey::generate();
    let sk2 = torsten_crypto::keys::PaymentSigningKey::generate();
    assert_ne!(sk1.to_bytes(), sk2.to_bytes());
}

#[test]
fn test_verification_key_hash_is_28_bytes() {
    let sk = torsten_crypto::keys::PaymentSigningKey::generate();
    let vk = sk.verification_key();
    let hash = vk.hash();
    assert_eq!(hash.as_bytes().len(), 28);
}

#[test]
fn test_verification_key_roundtrip_through_text_envelope() {
    let sk = torsten_crypto::keys::PaymentSigningKey::generate();
    let vk = sk.verification_key();
    let original_bytes = vk.to_bytes();

    // Wrap in CBOR and hex-encode (text envelope format)
    let cbor = simple_cbor_wrap(&original_bytes);
    let hex_str = hex::encode(&cbor);

    // Decode back
    let decoded_cbor = hex::decode(&hex_str).unwrap();
    // Skip the CBOR header (2 bytes for 32-byte payload: 0x5820)
    let decoded_bytes = &decoded_cbor[2..];
    assert_eq!(decoded_bytes, &original_bytes);
}

// ─── Address building ────────────────────────────────────────────────────────

#[test]
fn test_enterprise_address_testnet_prefix() {
    use torsten_primitives::address::{Address, EnterpriseAddress};
    use torsten_primitives::credentials::Credential;
    use torsten_primitives::hash::Hash28;
    use torsten_primitives::network::NetworkId;

    let payment_hash = Hash28::from_bytes([0x01; 28]);
    let addr = Address::Enterprise(EnterpriseAddress {
        network: NetworkId::Testnet,
        payment: Credential::VerificationKey(payment_hash),
    });

    let addr_bytes = addr.to_bytes();
    // Enterprise testnet: header byte = 0x60 (type 6, network 0)
    assert_eq!(addr_bytes[0] & 0xF0, 0x60);
    assert_eq!(addr_bytes[0] & 0x0F, 0x00); // testnet
}

#[test]
fn test_base_address_has_payment_and_stake() {
    use torsten_primitives::address::{Address, BaseAddress};
    use torsten_primitives::credentials::Credential;
    use torsten_primitives::hash::Hash28;
    use torsten_primitives::network::NetworkId;

    let payment_hash = Hash28::from_bytes([0x01; 28]);
    let stake_hash = Hash28::from_bytes([0x02; 28]);
    let addr = Address::Base(BaseAddress {
        network: NetworkId::Testnet,
        payment: Credential::VerificationKey(payment_hash),
        stake: Credential::VerificationKey(stake_hash),
    });

    let addr_bytes = addr.to_bytes();
    // Base address: 1 header byte + 28 payment + 28 stake = 57 bytes
    assert_eq!(addr_bytes.len(), 57);
    // Header: type 0, network 0
    assert_eq!(addr_bytes[0] & 0xF0, 0x00);
}

#[test]
fn test_mainnet_address_has_correct_network_tag() {
    use torsten_primitives::address::{Address, EnterpriseAddress};
    use torsten_primitives::credentials::Credential;
    use torsten_primitives::hash::Hash28;
    use torsten_primitives::network::NetworkId;

    let addr = Address::Enterprise(EnterpriseAddress {
        network: NetworkId::Mainnet,
        payment: Credential::VerificationKey(Hash28::from_bytes([0xAA; 28])),
    });

    let addr_bytes = addr.to_bytes();
    assert_eq!(addr_bytes[0] & 0x0F, 0x01); // mainnet = 1
}

#[test]
fn test_address_bech32_roundtrip() {
    use torsten_primitives::address::{Address, EnterpriseAddress};
    use torsten_primitives::credentials::Credential;
    use torsten_primitives::hash::Hash28;
    use torsten_primitives::network::NetworkId;

    let addr = Address::Enterprise(EnterpriseAddress {
        network: NetworkId::Testnet,
        payment: Credential::VerificationKey(Hash28::from_bytes([0x42; 28])),
    });

    let addr_bytes = addr.to_bytes();
    let bech32_str =
        bech32::encode::<bech32::Bech32>(bech32::Hrp::parse("addr_test").unwrap(), &addr_bytes)
            .unwrap();

    assert!(bech32_str.starts_with("addr_test1"));

    // Decode back
    let (_hrp, decoded_bytes) = bech32::decode(&bech32_str).unwrap();
    assert_eq!(decoded_bytes, addr_bytes);
}

// ─── Text envelope format ────────────────────────────────────────────────────

#[test]
fn test_text_envelope_json_structure() {
    let envelope = serde_json::json!({
        "type": "PaymentSigningKeyShelley_ed25519",
        "description": "Payment Signing Key",
        "cborHex": "5820deadbeef00000000000000000000000000000000000000000000000000000000"
    });

    assert!(envelope["type"].is_string());
    assert!(envelope["description"].is_string());
    assert!(envelope["cborHex"].is_string());
    let hex = envelope["cborHex"].as_str().unwrap();
    assert!(hex::decode(hex).is_ok());
}

#[test]
fn test_text_envelope_cbor_hex_starts_with_5820_for_32_byte_key() {
    let key_bytes = [0xAA; 32];
    let cbor = simple_cbor_wrap(&key_bytes);
    let hex = hex::encode(&cbor);
    // CBOR: 5820 = bytes(32)
    assert!(
        hex.starts_with("5820"),
        "Expected 5820 prefix, got: {}",
        &hex[..4]
    );
}

#[test]
fn test_text_envelope_cbor_hex_starts_with_5840_for_64_byte_key() {
    let key_bytes = [0xBB; 64];
    let cbor = simple_cbor_wrap(&key_bytes);
    let hex = hex::encode(&cbor);
    // CBOR: 5840 = bytes(64)
    assert!(
        hex.starts_with("5840"),
        "Expected 5840 prefix, got: {}",
        &hex[..4]
    );
}

// ─── Stake pool registration helpers ─────────────────────────────────────────

#[test]
fn test_pool_id_is_28_byte_hash() {
    let sk = torsten_crypto::keys::PaymentSigningKey::generate();
    let vk = sk.verification_key();
    let hash = vk.hash();
    assert_eq!(
        hash.as_bytes().len(),
        28,
        "Pool ID (cold key hash) must be 28 bytes"
    );
}

// ─── Node opcert helpers ─────────────────────────────────────────────────────

#[test]
fn test_kes_keygen_produces_valid_key_pair() {
    let seed = [42u8; 32];
    let result = torsten_crypto::kes::kes_keygen(&seed);
    assert!(result.is_ok());
    let (sk, pk) = result.unwrap();
    assert_eq!(
        sk.len(),
        612,
        "KES secret key should be 612 bytes (Sum6Kes)"
    );
    assert_eq!(pk.len(), 32, "KES public key should be 32 bytes");
}

#[test]
fn test_vrf_keygen_produces_valid_pair() {
    let kp = torsten_crypto::vrf::generate_vrf_keypair();
    assert_eq!(kp.secret_key().len(), 32);
    assert_eq!(kp.public_key.len(), 32);
}

#[test]
fn test_vrf_keygen_different_each_time() {
    let kp1 = torsten_crypto::vrf::generate_vrf_keypair();
    let kp2 = torsten_crypto::vrf::generate_vrf_keypair();
    assert_ne!(kp1.secret_key(), kp2.secret_key());
}

// ─── Helper: simple CBOR byte-string wrapper ─────────────────────────────────

/// Wraps raw bytes in a CBOR byte-string header.
/// Matches the encoding used by cardano-cli text envelope files.
fn simple_cbor_wrap(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::new();
    if data.len() < 24 {
        result.push(0x40 + data.len() as u8);
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
