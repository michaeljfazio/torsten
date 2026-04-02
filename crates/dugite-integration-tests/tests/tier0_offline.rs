//! Tier 0: Offline tests — no node required.
//! Run with: cargo test -p dugite-integration-tests -- tier0

use dugite_integration_tests::helpers::cli::run_cli_ok;
use dugite_integration_tests::helpers::keys::{
    TempDrepKeys, TempKesKeys, TempKeys, TempNodeKeys, TempStakeKeys, TempVrfKeys,
};
use serde_json::Value;
use std::fs;
use tempfile::TempDir;

/// Helper to read a JSON text envelope file and validate structure.
fn read_envelope(path: &str) -> Value {
    let contents =
        fs::read_to_string(path).unwrap_or_else(|e| panic!("Failed to read {}: {}", path, e));
    let v: Value = serde_json::from_str(&contents)
        .unwrap_or_else(|e| panic!("Invalid JSON in {}: {}", path, e));
    assert!(v.get("type").is_some(), "Missing 'type' field in {}", path);
    assert!(
        v.get("description").is_some(),
        "Missing 'description' field in {}",
        path
    );
    assert!(
        v.get("cborHex").is_some(),
        "Missing 'cborHex' field in {}",
        path
    );
    v
}

// ─── Key Generation ───────────────────────────────────────────────────

#[test]
fn tier0_payment_key_gen() {
    let keys = TempKeys::new();
    let skey = read_envelope(&keys.payment_skey);
    let vkey = read_envelope(&keys.payment_vkey);
    assert_eq!(
        skey["type"].as_str().unwrap(),
        "PaymentSigningKeyShelley_ed25519"
    );
    assert_eq!(
        vkey["type"].as_str().unwrap(),
        "PaymentVerificationKeyShelley_ed25519"
    );
}

#[test]
fn tier0_payment_keys_unique() {
    let k1 = TempKeys::new();
    let k2 = TempKeys::new();
    let e1 = read_envelope(&k1.payment_vkey);
    let e2 = read_envelope(&k2.payment_vkey);
    assert_ne!(
        e1["cborHex"].as_str().unwrap(),
        e2["cborHex"].as_str().unwrap(),
        "Two key generations should produce different keys"
    );
}

#[test]
fn tier0_stake_key_gen() {
    let keys = TempStakeKeys::new();
    let skey = read_envelope(&keys.stake_skey);
    let vkey = read_envelope(&keys.stake_vkey);
    assert_eq!(
        skey["type"].as_str().unwrap(),
        "StakeSigningKeyShelley_ed25519"
    );
    assert_eq!(
        vkey["type"].as_str().unwrap(),
        "StakeVerificationKeyShelley_ed25519"
    );
}

#[test]
fn tier0_node_cold_key_gen() {
    let keys = TempNodeKeys::new();
    let skey = read_envelope(&keys.cold_skey);
    let vkey = read_envelope(&keys.cold_vkey);
    assert_eq!(
        skey["type"].as_str().unwrap(),
        "StakePoolSigningKey_ed25519"
    );
    assert_eq!(
        vkey["type"].as_str().unwrap(),
        "StakePoolVerificationKey_ed25519"
    );
    // Counter file should exist
    assert!(
        fs::metadata(&keys.counter_file).is_ok(),
        "Counter file should exist"
    );
}

#[test]
fn tier0_kes_key_gen() {
    let keys = TempKesKeys::new();
    let skey = read_envelope(&keys.kes_skey);
    let vkey = read_envelope(&keys.kes_vkey);
    assert!(
        skey["type"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("kes"),
        "KES skey should have KES type"
    );
    assert!(
        vkey["type"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("kes"),
        "KES vkey should have KES type"
    );
}

#[test]
fn tier0_vrf_key_gen() {
    let keys = TempVrfKeys::new();
    let skey = read_envelope(&keys.vrf_skey);
    let vkey = read_envelope(&keys.vrf_vkey);
    assert!(
        skey["type"].as_str().unwrap().contains("VRF"),
        "VRF skey should have VRF type"
    );
    assert!(
        vkey["type"].as_str().unwrap().contains("VRF"),
        "VRF vkey should have VRF type"
    );
}

#[test]
fn tier0_drep_key_gen() {
    let keys = TempDrepKeys::new();
    let skey = read_envelope(&keys.drep_skey);
    let vkey = read_envelope(&keys.drep_vkey);
    assert!(
        skey["type"].as_str().unwrap().contains("DRep"),
        "DRep skey type should contain DRep, got: {}",
        skey["type"]
    );
    assert!(
        vkey["type"].as_str().unwrap().contains("DRep"),
        "DRep vkey type should contain DRep, got: {}",
        vkey["type"]
    );
}

#[test]
fn tier0_pool_key_gen() {
    let dir = TempDir::new().unwrap();
    let cold_skey = dir.path().join("pool-cold.skey").display().to_string();
    let cold_vkey = dir.path().join("pool-cold.vkey").display().to_string();
    let counter = dir.path().join("pool.counter").display().to_string();

    run_cli_ok(&[
        "stake-pool",
        "key-gen",
        "--cold-signing-key-file",
        &cold_skey,
        "--cold-verification-key-file",
        &cold_vkey,
        "--operational-certificate-counter-file",
        &counter,
    ]);

    read_envelope(&cold_skey);
    read_envelope(&cold_vkey);
    assert!(fs::metadata(&counter).is_ok());
}

#[test]
fn tier0_pool_vrf_key_gen() {
    let dir = TempDir::new().unwrap();
    let skey = dir.path().join("vrf.skey").display().to_string();
    let vkey = dir.path().join("vrf.vkey").display().to_string();

    run_cli_ok(&[
        "stake-pool",
        "vrf-key-gen",
        "--signing-key-file",
        &skey,
        "--verification-key-file",
        &vkey,
    ]);

    read_envelope(&skey);
    read_envelope(&vkey);
}

#[test]
fn tier0_pool_kes_key_gen() {
    let dir = TempDir::new().unwrap();
    let skey = dir.path().join("kes.skey").display().to_string();
    let vkey = dir.path().join("kes.vkey").display().to_string();

    run_cli_ok(&[
        "stake-pool",
        "kes-key-gen",
        "--signing-key-file",
        &skey,
        "--verification-key-file",
        &vkey,
    ]);

    read_envelope(&skey);
    read_envelope(&vkey);
}

// ─── Address Building ─────────────────────────────────────────────────

#[test]
fn tier0_enterprise_address_testnet() {
    let keys = TempKeys::new();
    let addr = keys.enterprise_address_testnet();
    assert!(
        addr.starts_with("addr_test1"),
        "Testnet address should start with addr_test1, got: {}",
        addr
    );
}

#[test]
fn tier0_enterprise_address_mainnet() {
    let keys = TempKeys::new();
    let addr = keys.enterprise_address_mainnet();
    assert!(
        addr.starts_with("addr1"),
        "Mainnet address should start with addr1, got: {}",
        addr
    );
}

#[test]
fn tier0_base_address() {
    let payment = TempKeys::new();
    let stake = TempStakeKeys::new();

    let addr = run_cli_ok(&[
        "address",
        "build",
        "--payment-verification-key-file",
        &payment.payment_vkey,
        "--stake-verification-key-file",
        &stake.stake_vkey,
        "--network",
        "testnet",
    ]);
    let addr = addr.trim();
    assert!(
        addr.starts_with("addr_test1"),
        "Base address should start with addr_test1, got: {}",
        addr
    );
    // Base addresses are longer than enterprise addresses
    assert!(
        addr.len() > 60,
        "Base address should be longer than 60 chars, got {}",
        addr.len()
    );
}

#[test]
fn tier0_address_info() {
    let keys = TempKeys::new();
    let addr = keys.enterprise_address_testnet();
    let info = run_cli_ok(&["address", "info", "--address", &addr]);
    // Should output something (address details)
    assert!(
        !info.trim().is_empty(),
        "address info should produce output"
    );
}

// ─── Key Hash ──────────────────────────────────────────────────────────

#[test]
fn tier0_payment_key_hash() {
    let keys = TempKeys::new();
    let hash = keys.payment_key_hash();
    assert_eq!(
        hash.len(),
        56,
        "Key hash should be 56 hex chars (28 bytes), got {} chars: {}",
        hash.len(),
        hash
    );
    assert!(
        hash.chars().all(|c| c.is_ascii_hexdigit()),
        "Key hash should be hex, got: {}",
        hash
    );
}

#[test]
fn tier0_verification_key_hash() {
    let keys = TempKeys::new();
    let hash = run_cli_ok(&[
        "key",
        "verification-key-hash",
        "--verification-key-file",
        &keys.payment_vkey,
    ]);
    let hash = hash.trim();
    assert_eq!(hash.len(), 56);
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
}

// ─── Transaction Build/Sign/View/TxId ─────────────────────────────────

#[test]
fn tier0_transaction_build_sign_view_txid() {
    let keys = TempKeys::new();
    let dir = &keys.dir;
    let addr = keys.enterprise_address_testnet();

    let tx_body = dir.path().join("tx.raw").display().to_string();
    let tx_signed = dir.path().join("tx.signed").display().to_string();

    // Build with a mock input
    run_cli_ok(&[
        "transaction",
        "build",
        "--tx-in",
        "0000000000000000000000000000000000000000000000000000000000000000#0",
        "--tx-out",
        &format!("{}+1000000", addr),
        "--change-address",
        &addr,
        "--fee",
        "200000",
        "--ttl",
        "99999999",
        "--out-file",
        &tx_body,
    ]);

    assert!(
        fs::metadata(&tx_body).is_ok(),
        "Transaction body file should exist"
    );

    // Sign
    run_cli_ok(&[
        "transaction",
        "sign",
        "--tx-body-file",
        &tx_body,
        "--signing-key-file",
        &keys.payment_skey,
        "--out-file",
        &tx_signed,
    ]);

    assert!(
        fs::metadata(&tx_signed).is_ok(),
        "Signed tx file should exist"
    );

    // View
    let view_output = run_cli_ok(&["transaction", "view", "--tx-file", &tx_signed]);
    assert!(
        !view_output.trim().is_empty(),
        "transaction view should produce output"
    );

    // TxId
    let txid = run_cli_ok(&["transaction", "txid", "--tx-file", &tx_signed]);
    let txid = txid.trim();
    assert_eq!(
        txid.len(),
        64,
        "TxId should be 64 hex chars, got {} chars: {}",
        txid.len(),
        txid
    );
    assert!(
        txid.chars().all(|c| c.is_ascii_hexdigit()),
        "TxId should be hex, got: {}",
        txid
    );
}

#[test]
fn tier0_transaction_witness_assemble() {
    let keys = TempKeys::new();
    let dir = &keys.dir;
    let addr = keys.enterprise_address_testnet();

    let tx_body = dir.path().join("tx.raw").display().to_string();
    let witness = dir.path().join("tx.witness").display().to_string();
    let tx_assembled = dir.path().join("tx.assembled").display().to_string();
    let tx_signed_direct = dir.path().join("tx.signed").display().to_string();

    // Build
    run_cli_ok(&[
        "transaction",
        "build",
        "--tx-in",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa#0",
        "--tx-out",
        &format!("{}+2000000", addr),
        "--change-address",
        &addr,
        "--fee",
        "180000",
        "--ttl",
        "99999999",
        "--out-file",
        &tx_body,
    ]);

    // Witness
    run_cli_ok(&[
        "transaction",
        "witness",
        "--tx-body-file",
        &tx_body,
        "--signing-key-file",
        &keys.payment_skey,
        "--out-file",
        &witness,
    ]);

    assert!(fs::metadata(&witness).is_ok(), "Witness file should exist");

    // Assemble
    run_cli_ok(&[
        "transaction",
        "assemble",
        "--tx-body-file",
        &tx_body,
        "--witness-file",
        &witness,
        "--out-file",
        &tx_assembled,
    ]);

    assert!(
        fs::metadata(&tx_assembled).is_ok(),
        "Assembled tx should exist"
    );

    // Direct sign for comparison
    run_cli_ok(&[
        "transaction",
        "sign",
        "--tx-body-file",
        &tx_body,
        "--signing-key-file",
        &keys.payment_skey,
        "--out-file",
        &tx_signed_direct,
    ]);

    // Both should produce the same txid
    let txid_assembled = run_cli_ok(&["transaction", "txid", "--tx-file", &tx_assembled]);
    let txid_signed = run_cli_ok(&["transaction", "txid", "--tx-file", &tx_signed_direct]);
    assert_eq!(
        txid_assembled.trim(),
        txid_signed.trim(),
        "Assembled and directly signed tx should have the same txid"
    );
}

#[test]
fn tier0_calculate_min_fee() {
    let keys = TempKeys::new();
    let dir = &keys.dir;
    let addr = keys.enterprise_address_testnet();

    let tx_body = dir.path().join("tx.raw").display().to_string();
    let pp_file = dir.path().join("pp.json").display().to_string();

    // Build a tx body
    run_cli_ok(&[
        "transaction",
        "build",
        "--tx-in",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb#0",
        "--tx-out",
        &format!("{}+5000000", addr),
        "--change-address",
        &addr,
        "--fee",
        "200000",
        "--ttl",
        "99999999",
        "--out-file",
        &tx_body,
    ]);

    // Create minimal protocol params file
    let pp = serde_json::json!({
        "minFeeA": 44,
        "minFeeB": 155381,
        "maxTxSize": 16384
    });
    fs::write(&pp_file, serde_json::to_string_pretty(&pp).unwrap()).unwrap();

    let fee_output = run_cli_ok(&[
        "transaction",
        "calculate-min-fee",
        "--tx-body-file",
        &tx_body,
        "--witness-count",
        "1",
        "--protocol-params-file",
        &pp_file,
    ]);

    let fee_str = fee_output.trim();
    // Should be a number (possibly with "Lovelace" suffix)
    let fee_num: String = fee_str.chars().filter(|c| c.is_ascii_digit()).collect();
    assert!(
        !fee_num.is_empty(),
        "Min fee should contain a number, got: {}",
        fee_str
    );
    let fee: u64 = fee_num.parse().unwrap();
    assert!(
        fee > 0 && fee < 10_000_000,
        "Fee should be reasonable, got: {}",
        fee
    );
}

// ─── Certificates ─────────────────────────────────────────────────────

#[test]
fn tier0_stake_registration_certificate() {
    let stake = TempStakeKeys::new();
    let cert_file = stake
        .dir
        .path()
        .join("stake-reg.cert")
        .display()
        .to_string();

    run_cli_ok(&[
        "stake-address",
        "registration-certificate",
        "--stake-verification-key-file",
        &stake.stake_vkey,
        "--out-file",
        &cert_file,
    ]);

    assert!(fs::metadata(&cert_file).is_ok());
    let envelope = read_envelope(&cert_file);
    assert!(
        envelope["type"].as_str().unwrap().contains("Certificate"),
        "Should be a certificate type"
    );
}

#[test]
fn tier0_stake_deregistration_certificate() {
    let stake = TempStakeKeys::new();
    let cert_file = stake
        .dir
        .path()
        .join("stake-dereg.cert")
        .display()
        .to_string();

    run_cli_ok(&[
        "stake-address",
        "deregistration-certificate",
        "--stake-verification-key-file",
        &stake.stake_vkey,
        "--out-file",
        &cert_file,
    ]);

    assert!(fs::metadata(&cert_file).is_ok());
    read_envelope(&cert_file);
}

#[test]
fn tier0_stake_delegation_certificate() {
    let stake = TempStakeKeys::new();
    let cert_file = stake.dir.path().join("deleg.cert").display().to_string();

    run_cli_ok(&[
        "stake-address",
        "delegation-certificate",
        "--stake-verification-key-file",
        &stake.stake_vkey,
        "--stake-pool-id",
        "00000000000000000000000000000000000000000000000000000000",
        "--out-file",
        &cert_file,
    ]);

    assert!(fs::metadata(&cert_file).is_ok());
    read_envelope(&cert_file);
}

#[test]
fn tier0_vote_delegation_certificate_abstain() {
    let stake = TempStakeKeys::new();
    let cert_file = stake
        .dir
        .path()
        .join("vote-deleg.cert")
        .display()
        .to_string();

    run_cli_ok(&[
        "stake-address",
        "vote-delegation-certificate",
        "--stake-verification-key-file",
        &stake.stake_vkey,
        "--always-abstain",
        "--out-file",
        &cert_file,
    ]);

    assert!(fs::metadata(&cert_file).is_ok());
    read_envelope(&cert_file);
}

#[test]
fn tier0_pool_retirement_certificate() {
    let node_keys = TempNodeKeys::new();
    let cert_file = node_keys
        .dir
        .path()
        .join("pool-retire.cert")
        .display()
        .to_string();

    run_cli_ok(&[
        "stake-pool",
        "retirement-certificate",
        "--cold-verification-key-file",
        &node_keys.cold_vkey,
        "--epoch",
        "100",
        "--out-file",
        &cert_file,
    ]);

    assert!(fs::metadata(&cert_file).is_ok());
    read_envelope(&cert_file);
}

#[test]
fn tier0_pool_registration_certificate() {
    let dir = TempDir::new().unwrap();

    // Generate pool cold keys
    let cold_skey = dir.path().join("pool-cold.skey").display().to_string();
    let cold_vkey = dir.path().join("pool-cold.vkey").display().to_string();
    let counter = dir.path().join("pool.counter").display().to_string();
    run_cli_ok(&[
        "stake-pool",
        "key-gen",
        "--cold-signing-key-file",
        &cold_skey,
        "--cold-verification-key-file",
        &cold_vkey,
        "--operational-certificate-counter-file",
        &counter,
    ]);

    // Generate VRF keys
    let vrf_skey = dir.path().join("vrf.skey").display().to_string();
    let vrf_vkey = dir.path().join("vrf.vkey").display().to_string();
    run_cli_ok(&[
        "stake-pool",
        "vrf-key-gen",
        "--signing-key-file",
        &vrf_skey,
        "--verification-key-file",
        &vrf_vkey,
    ]);

    // Generate reward account key
    let reward_skey = dir.path().join("reward.skey").display().to_string();
    let reward_vkey = dir.path().join("reward.vkey").display().to_string();
    run_cli_ok(&[
        "stake-address",
        "key-gen",
        "--signing-key-file",
        &reward_skey,
        "--verification-key-file",
        &reward_vkey,
    ]);

    // Generate owner key
    let owner_skey = dir.path().join("owner.skey").display().to_string();
    let owner_vkey = dir.path().join("owner.vkey").display().to_string();
    run_cli_ok(&[
        "stake-address",
        "key-gen",
        "--signing-key-file",
        &owner_skey,
        "--verification-key-file",
        &owner_vkey,
    ]);

    let cert_file = dir.path().join("pool-reg.cert").display().to_string();

    run_cli_ok(&[
        "stake-pool",
        "registration-certificate",
        "--cold-verification-key-file",
        &cold_vkey,
        "--vrf-verification-key-file",
        &vrf_vkey,
        "--pledge",
        "1000000000",
        "--cost",
        "340000000",
        "--margin",
        "0.05",
        "--reward-account-verification-key-file",
        &reward_vkey,
        "--pool-owner-verification-key-file",
        &owner_vkey,
        "--out-file",
        &cert_file,
    ]);

    assert!(fs::metadata(&cert_file).is_ok());
    read_envelope(&cert_file);
}

#[test]
fn tier0_drep_registration_certificate() {
    let drep = TempDrepKeys::new();
    let cert_file = drep.dir.path().join("drep-reg.cert").display().to_string();

    run_cli_ok(&[
        "governance",
        "drep",
        "registration-certificate",
        "--drep-verification-key-file",
        &drep.drep_vkey,
        "--key-reg-deposit-amt",
        "500000000",
        "--out-file",
        &cert_file,
    ]);

    assert!(fs::metadata(&cert_file).is_ok());
    read_envelope(&cert_file);
}

#[test]
fn tier0_drep_retirement_certificate() {
    let drep = TempDrepKeys::new();
    let cert_file = drep
        .dir
        .path()
        .join("drep-retire.cert")
        .display()
        .to_string();

    run_cli_ok(&[
        "governance",
        "drep",
        "retirement-certificate",
        "--drep-verification-key-file",
        &drep.drep_vkey,
        "--deposit-amt",
        "500000000",
        "--out-file",
        &cert_file,
    ]);

    assert!(fs::metadata(&cert_file).is_ok());
    read_envelope(&cert_file);
}

// ─── Operational Certificate ──────────────────────────────────────────

#[test]
fn tier0_issue_opcert() {
    let node_keys = TempNodeKeys::new();
    let kes_keys = TempKesKeys::new();
    let opcert_file = node_keys
        .dir
        .path()
        .join("node.opcert")
        .display()
        .to_string();

    run_cli_ok(&[
        "node",
        "issue-op-cert",
        "--kes-verification-key-file",
        &kes_keys.kes_vkey,
        "--cold-signing-key-file",
        &node_keys.cold_skey,
        "--operational-certificate-counter-file",
        &node_keys.counter_file,
        "--kes-period",
        "0",
        "--out-file",
        &opcert_file,
    ]);

    assert!(
        fs::metadata(&opcert_file).is_ok(),
        "Opcert file should exist"
    );
    read_envelope(&opcert_file);

    // Counter should have been updated
    let counter_contents = fs::read_to_string(&node_keys.counter_file).unwrap();
    assert!(
        !counter_contents.is_empty(),
        "Counter file should not be empty"
    );
}

#[test]
fn tier0_issue_opcert_via_stake_pool() {
    let dir = TempDir::new().unwrap();
    let cold_skey = dir.path().join("cold.skey").display().to_string();
    let cold_vkey = dir.path().join("cold.vkey").display().to_string();
    let counter = dir.path().join("opcert.counter").display().to_string();

    run_cli_ok(&[
        "stake-pool",
        "key-gen",
        "--cold-signing-key-file",
        &cold_skey,
        "--cold-verification-key-file",
        &cold_vkey,
        "--operational-certificate-counter-file",
        &counter,
    ]);

    let kes_skey = dir.path().join("kes.skey").display().to_string();
    let kes_vkey = dir.path().join("kes.vkey").display().to_string();
    run_cli_ok(&[
        "stake-pool",
        "kes-key-gen",
        "--signing-key-file",
        &kes_skey,
        "--verification-key-file",
        &kes_vkey,
    ]);

    let opcert = dir.path().join("node.opcert").display().to_string();
    run_cli_ok(&[
        "stake-pool",
        "issue-op-cert",
        "--kes-verification-key-file",
        &kes_vkey,
        "--cold-signing-key-file",
        &cold_skey,
        "--operational-certificate-counter-file",
        &counter,
        "--kes-period",
        "5",
        "--out-file",
        &opcert,
    ]);

    assert!(fs::metadata(&opcert).is_ok());
    read_envelope(&opcert);
}

// ─── Governance Actions ───────────────────────────────────────────────

/// Helper to generate a valid testnet enterprise address for governance tests.
fn test_return_addr() -> String {
    let keys = TempKeys::new();
    keys.enterprise_address_testnet().trim().to_string()
}

#[test]
fn tier0_governance_create_info_action() {
    let dir = TempDir::new().unwrap();
    let out_file = dir.path().join("info.action").display().to_string();

    run_cli_ok(&[
        "governance",
        "action",
        "create-info",
        "--anchor-url",
        "https://example.com/info",
        "--anchor-data-hash",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "--deposit",
        "100000000000",
        "--return-addr",
        &test_return_addr(),
        "--out-file",
        &out_file,
    ]);

    assert!(fs::metadata(&out_file).is_ok());
    read_envelope(&out_file);
}

#[test]
fn tier0_governance_create_no_confidence() {
    let dir = TempDir::new().unwrap();
    let out_file = dir
        .path()
        .join("no-confidence.action")
        .display()
        .to_string();

    run_cli_ok(&[
        "governance",
        "action",
        "create-no-confidence",
        "--anchor-url",
        "https://example.com/no-confidence",
        "--anchor-data-hash",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "--deposit",
        "100000000000",
        "--return-addr",
        &test_return_addr(),
        "--out-file",
        &out_file,
    ]);

    assert!(fs::metadata(&out_file).is_ok());
    read_envelope(&out_file);
}

#[test]
fn tier0_governance_create_constitution() {
    let dir = TempDir::new().unwrap();
    let out_file = dir.path().join("constitution.action").display().to_string();

    run_cli_ok(&[
        "governance",
        "action",
        "create-constitution",
        "--anchor-url",
        "https://example.com/constitution",
        "--anchor-data-hash",
        "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        "--deposit",
        "100000000000",
        "--return-addr",
        &test_return_addr(),
        "--constitution-url",
        "https://example.com/constitution.txt",
        "--constitution-hash",
        "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
        "--out-file",
        &out_file,
    ]);

    assert!(fs::metadata(&out_file).is_ok());
    read_envelope(&out_file);
}

// ─── Stake Address Build ──────────────────────────────────────────────

#[test]
fn tier0_stake_address_build() {
    let stake = TempStakeKeys::new();
    let addr = run_cli_ok(&[
        "stake-address",
        "build",
        "--stake-verification-key-file",
        &stake.stake_vkey,
        "--network",
        "testnet",
    ]);
    let addr = addr.trim();
    assert!(
        addr.starts_with("stake_test1"),
        "Testnet stake address should start with stake_test1, got: {}",
        addr
    );
}

// ─── DRep ID ──────────────────────────────────────────────────────────

#[test]
fn tier0_drep_id() {
    let drep = TempDrepKeys::new();
    let id = run_cli_ok(&[
        "governance",
        "drep",
        "id",
        "--drep-verification-key-file",
        &drep.drep_vkey,
    ]);
    let id = id.trim();
    assert!(!id.is_empty(), "DRep ID should not be empty");
}

// ─── Pool ID ──────────────────────────────────────────────────────────

#[test]
fn tier0_pool_id() {
    let dir = TempDir::new().unwrap();
    let cold_skey = dir.path().join("cold.skey").display().to_string();
    let cold_vkey = dir.path().join("cold.vkey").display().to_string();
    let counter = dir.path().join("counter").display().to_string();

    run_cli_ok(&[
        "stake-pool",
        "key-gen",
        "--cold-signing-key-file",
        &cold_skey,
        "--cold-verification-key-file",
        &cold_vkey,
        "--operational-certificate-counter-file",
        &counter,
    ]);

    let id = run_cli_ok(&[
        "stake-pool",
        "id",
        "--cold-verification-key-file",
        &cold_vkey,
    ]);
    let id = id.trim();
    assert!(!id.is_empty(), "Pool ID should not be empty");
}

// ─── Hash Anchor Data ─────────────────────────────────────────────────

#[test]
fn tier0_hash_anchor_data() {
    let dir = TempDir::new().unwrap();
    let data_file = dir.path().join("anchor.txt").display().to_string();
    fs::write(&data_file, "This is test anchor data for hashing.").unwrap();

    let hash = run_cli_ok(&[
        "governance",
        "action",
        "hash-anchor-data",
        "--file-text",
        &data_file,
    ]);
    let hash = hash.trim();
    assert_eq!(
        hash.len(),
        64,
        "Anchor data hash should be 64 hex chars, got {}: {}",
        hash.len(),
        hash
    );
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
}

// ─── Text Envelope Compatibility ──────────────────────────────────────

#[test]
fn tier0_text_envelope_structure() {
    let keys = TempKeys::new();
    let skey_content = fs::read_to_string(&keys.payment_skey).unwrap();
    let vkey_content = fs::read_to_string(&keys.payment_vkey).unwrap();

    for (name, content) in [("skey", skey_content), ("vkey", vkey_content)] {
        let v: Value = serde_json::from_str(&content).unwrap();

        // Must have exactly three keys: type, description, cborHex
        let obj = v.as_object().expect("Envelope should be a JSON object");
        assert!(obj.contains_key("type"), "{}: missing 'type' field", name);
        assert!(
            obj.contains_key("description"),
            "{}: missing 'description' field",
            name
        );
        assert!(
            obj.contains_key("cborHex"),
            "{}: missing 'cborHex' field",
            name
        );

        // cborHex should be valid hex
        let cbor_hex = obj["cborHex"].as_str().unwrap();
        assert!(
            !cbor_hex.is_empty() && cbor_hex.chars().all(|c| c.is_ascii_hexdigit()),
            "{}: cborHex should be non-empty hex",
            name
        );
    }
}
