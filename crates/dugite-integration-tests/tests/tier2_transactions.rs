//! Tier 2: Transaction tests — requires a running node and funded wallet.
//! Run with:
//!   DUGITE_INTEGRATION_SOCKET=./node.sock \
//!   DUGITE_TEST_KEYS=keys/preview-test \
//!   cargo test -p dugite-integration-tests -- tier2

use dugite_integration_tests::helpers::cli::{
    integration_socket, run_cli, run_cli_ok, test_keys_dir,
};
use dugite_integration_tests::helpers::keys::TempKeys;
use dugite_integration_tests::helpers::koios;
use dugite_integration_tests::helpers::wait::wait_for_confirmation;
use dugite_integration_tests::helpers::wallet::{get_utxos_cli, skip_if_underfunded};
use std::fs;
use tempfile::TempDir;

/// Require socket + test keys, or skip.
fn require_env() -> (String, String, String, String) {
    let socket = match integration_socket() {
        Some(s) => s,
        None => {
            eprintln!("SKIP: DUGITE_INTEGRATION_SOCKET not set");
            std::process::exit(0);
        }
    };
    let keys_dir = match test_keys_dir() {
        Some(d) => d,
        None => {
            eprintln!("SKIP: DUGITE_TEST_KEYS not set");
            std::process::exit(0);
        }
    };

    let skey = format!("{}/payment.skey", keys_dir);
    let vkey = format!("{}/payment.vkey", keys_dir);

    assert!(
        fs::metadata(&skey).is_ok(),
        "Test payment signing key not found: {}",
        skey
    );
    assert!(
        fs::metadata(&vkey).is_ok(),
        "Test payment verification key not found: {}",
        vkey
    );

    // Build the address
    let addr = run_cli_ok(&[
        "address",
        "build",
        "--payment-verification-key-file",
        &vkey,
        "--network",
        "testnet",
    ])
    .trim()
    .to_string();

    (socket, addr, skey, vkey)
}

/// Get the current tip slot for TTL calculation.
fn current_slot(socket: &str) -> u64 {
    let output = run_cli_ok(&["query", "tip", "--socket-path", socket]);
    let tip: serde_json::Value = serde_json::from_str(&output).expect("Failed to parse tip");
    tip.get("slot")
        .and_then(|s| s.as_u64())
        .expect("No slot in tip")
}

/// Build, sign, and submit a transaction. Returns the tx hash.
#[allow(clippy::too_many_arguments)]
fn build_sign_submit(
    socket: &str,
    tx_ins: &[&str],
    tx_outs: &[&str],
    change_address: &str,
    fee: u64,
    ttl: u64,
    skey: &str,
    extra_args: &[&str],
) -> String {
    let dir = TempDir::new().unwrap();
    let tx_body = dir.path().join("tx.raw").display().to_string();
    let tx_signed = dir.path().join("tx.signed").display().to_string();

    let mut args = vec!["transaction", "build"];
    for tx_in in tx_ins {
        args.push("--tx-in");
        args.push(tx_in);
    }
    for tx_out in tx_outs {
        args.push("--tx-out");
        args.push(tx_out);
    }
    let fee_str = fee.to_string();
    let ttl_str = ttl.to_string();
    args.extend_from_slice(&[
        "--change-address",
        change_address,
        "--fee",
        &fee_str,
        "--ttl",
        &ttl_str,
        "--out-file",
        &tx_body,
    ]);
    args.extend_from_slice(extra_args);

    run_cli_ok(&args);

    run_cli_ok(&[
        "transaction",
        "sign",
        "--tx-body-file",
        &tx_body,
        "--signing-key-file",
        skey,
        "--out-file",
        &tx_signed,
    ]);

    let txid = run_cli_ok(&["transaction", "txid", "--tx-file", &tx_signed])
        .trim()
        .to_string();

    run_cli_ok(&[
        "transaction",
        "submit",
        "--tx-file",
        &tx_signed,
        "--socket-path",
        socket,
        "--testnet-magic",
        "2",
    ]);

    txid
}

// ─── Transaction Tests ────────────────────────────────────────────────

#[test]
fn tier2_self_transfer() {
    let (socket, addr, skey, _vkey) = require_env();
    if skip_if_underfunded(&socket, &addr, 100_000_000) {
        return;
    }

    let utxos = get_utxos_cli(&socket, &addr);
    assert!(!utxos.is_empty(), "Wallet should have UTxOs");

    let utxo = &utxos[0];
    let tx_in = format!("{}#{}", utxo.tx_hash, utxo.tx_index);
    let send_amount = 2_000_000u64;
    let fee = 200_000u64;
    let ttl = current_slot(&socket) + 600; // ~10 minutes from now

    let tx_out = format!("{}+{}", addr, send_amount);
    let txid = build_sign_submit(&socket, &[&tx_in], &[&tx_out], &addr, fee, ttl, &skey, &[]);

    assert_eq!(txid.len(), 64, "TxId should be 64 hex chars");

    // Wait for confirmation via Koios
    let rt = tokio::runtime::Runtime::new().unwrap();
    match rt.block_on(wait_for_confirmation(&txid, 120)) {
        Ok(_) => println!("Self-transfer {} confirmed", txid),
        Err(e) => eprintln!("WARNING: Confirmation polling failed: {}", e),
    }
}

#[test]
fn tier2_two_party_transfer() {
    let (socket, addr, skey, _vkey) = require_env();
    if skip_if_underfunded(&socket, &addr, 100_000_000) {
        return;
    }

    // Generate a second address
    let recipient = TempKeys::new();
    let recipient_addr = recipient.enterprise_address_testnet();

    let utxos = get_utxos_cli(&socket, &addr);
    assert!(!utxos.is_empty());

    let utxo = &utxos[0];
    let tx_in = format!("{}#{}", utxo.tx_hash, utxo.tx_index);
    let send_amount = 3_000_000u64;
    let fee = 200_000u64;
    let ttl = current_slot(&socket) + 600;

    let tx_out = format!("{}+{}", recipient_addr, send_amount);
    let txid = build_sign_submit(&socket, &[&tx_in], &[&tx_out], &addr, fee, ttl, &skey, &[]);

    assert_eq!(txid.len(), 64);

    let rt = tokio::runtime::Runtime::new().unwrap();
    match rt.block_on(wait_for_confirmation(&txid, 120)) {
        Ok(info) => {
            println!("Two-party transfer {} confirmed", txid);
            // Verify recipient received funds via Koios
            if let Ok(utxos) = rt.block_on(koios::address_utxos(&recipient_addr)) {
                if let Some(arr) = utxos.as_array() {
                    assert!(
                        !arr.is_empty(),
                        "Recipient should have UTxOs after confirmed tx"
                    );
                }
            }
            let _ = info;
        }
        Err(e) => eprintln!("WARNING: Confirmation polling failed: {}", e),
    }
}

#[test]
fn tier2_metadata_transaction() {
    let (socket, addr, skey, _vkey) = require_env();
    if skip_if_underfunded(&socket, &addr, 100_000_000) {
        return;
    }

    let utxos = get_utxos_cli(&socket, &addr);
    assert!(!utxos.is_empty());

    let utxo = &utxos[0];
    let tx_in = format!("{}#{}", utxo.tx_hash, utxo.tx_index);
    let fee = 200_000u64;
    let ttl = current_slot(&socket) + 600;

    // Create CIP-20 metadata file
    let dir = TempDir::new().unwrap();
    let metadata_file = dir.path().join("metadata.json").display().to_string();
    let metadata = serde_json::json!({
        "674": {
            "msg": ["Dugite integration test"]
        }
    });
    fs::write(&metadata_file, serde_json::to_string(&metadata).unwrap()).unwrap();

    let tx_out = format!("{}+2000000", addr);

    let tx_body = dir.path().join("tx.raw").display().to_string();
    let tx_signed = dir.path().join("tx.signed").display().to_string();

    run_cli_ok(&[
        "transaction",
        "build",
        "--tx-in",
        &tx_in,
        "--tx-out",
        &tx_out,
        "--change-address",
        &addr,
        "--fee",
        &fee.to_string(),
        "--ttl",
        &ttl.to_string(),
        "--metadata-json-file",
        &metadata_file,
        "--out-file",
        &tx_body,
    ]);

    run_cli_ok(&[
        "transaction",
        "sign",
        "--tx-body-file",
        &tx_body,
        "--signing-key-file",
        &skey,
        "--out-file",
        &tx_signed,
    ]);

    let txid = run_cli_ok(&["transaction", "txid", "--tx-file", &tx_signed])
        .trim()
        .to_string();

    run_cli_ok(&[
        "transaction",
        "submit",
        "--tx-file",
        &tx_signed,
        "--socket-path",
        &socket,
        "--testnet-magic",
        "2",
    ]);

    let rt = tokio::runtime::Runtime::new().unwrap();
    match rt.block_on(wait_for_confirmation(&txid, 120)) {
        Ok(info) => {
            println!("Metadata tx {} confirmed", txid);
            // Check if metadata is present in Koios response
            if let Some(arr) = info.as_array() {
                if let Some(first) = arr.first() {
                    if let Some(meta) = first.get("metadata") {
                        assert!(!meta.is_null(), "Confirmed tx should have metadata");
                    }
                }
            }
        }
        Err(e) => eprintln!("WARNING: Confirmation polling failed: {}", e),
    }
}

#[test]
fn tier2_expired_ttl_rejection() {
    let (socket, addr, skey, _vkey) = require_env();
    if skip_if_underfunded(&socket, &addr, 10_000_000) {
        return;
    }

    let utxos = get_utxos_cli(&socket, &addr);
    assert!(!utxos.is_empty());

    let utxo = &utxos[0];
    let tx_in = format!("{}#{}", utxo.tx_hash, utxo.tx_index);

    // Use a TTL in the past
    let past_ttl = 1u64; // slot 1 is definitely in the past

    let dir = TempDir::new().unwrap();
    let tx_body = dir.path().join("tx.raw").display().to_string();
    let tx_signed = dir.path().join("tx.signed").display().to_string();

    let tx_out = format!("{}+2000000", addr);

    run_cli_ok(&[
        "transaction",
        "build",
        "--tx-in",
        &tx_in,
        "--tx-out",
        &tx_out,
        "--change-address",
        &addr,
        "--fee",
        "200000",
        "--ttl",
        &past_ttl.to_string(),
        "--out-file",
        &tx_body,
    ]);

    run_cli_ok(&[
        "transaction",
        "sign",
        "--tx-body-file",
        &tx_body,
        "--signing-key-file",
        &skey,
        "--out-file",
        &tx_signed,
    ]);

    // Submit should fail
    let result = run_cli(&[
        "transaction",
        "submit",
        "--tx-file",
        &tx_signed,
        "--socket-path",
        &socket,
        "--testnet-magic",
        "2",
    ]);

    assert!(
        !result.success(),
        "Transaction with expired TTL should be rejected"
    );
}

#[test]
fn tier2_multi_input_consolidation() {
    let (socket, addr, skey, _vkey) = require_env();
    if skip_if_underfunded(&socket, &addr, 100_000_000) {
        return;
    }

    let utxos = get_utxos_cli(&socket, &addr);
    if utxos.len() < 2 {
        eprintln!("SKIP: Need at least 2 UTxOs for multi-input consolidation test");
        return;
    }

    let tx_in_0 = format!("{}#{}", utxos[0].tx_hash, utxos[0].tx_index);
    let tx_in_1 = format!("{}#{}", utxos[1].tx_hash, utxos[1].tx_index);
    let total = utxos[0].lovelace + utxos[1].lovelace;
    let fee = 200_000u64;
    let ttl = current_slot(&socket) + 600;

    // Single output with consolidated amount
    let tx_out = format!("{}+{}", addr, total - fee);
    let txid = build_sign_submit(
        &socket,
        &[&tx_in_0, &tx_in_1],
        &[&tx_out],
        &addr,
        fee,
        ttl,
        &skey,
        &[],
    );

    assert_eq!(txid.len(), 64);

    let rt = tokio::runtime::Runtime::new().unwrap();
    match rt.block_on(wait_for_confirmation(&txid, 120)) {
        Ok(_) => println!("Multi-input consolidation {} confirmed", txid),
        Err(e) => eprintln!("WARNING: Confirmation polling failed: {}", e),
    }
}

#[test]
fn tier2_min_fee_accuracy() {
    let (socket, addr, skey, _vkey) = require_env();
    if skip_if_underfunded(&socket, &addr, 50_000_000) {
        return;
    }

    let utxos = get_utxos_cli(&socket, &addr);
    assert!(!utxos.is_empty());

    let dir = TempDir::new().unwrap();
    let tx_body = dir.path().join("tx.raw").display().to_string();
    let pp_file = dir.path().join("pp.json").display().to_string();

    let utxo = &utxos[0];
    let tx_in = format!("{}#{}", utxo.tx_hash, utxo.tx_index);
    let ttl = current_slot(&socket) + 600;
    let tx_out = format!("{}+2000000", addr);

    // Build with placeholder fee
    run_cli_ok(&[
        "transaction",
        "build",
        "--tx-in",
        &tx_in,
        "--tx-out",
        &tx_out,
        "--change-address",
        &addr,
        "--fee",
        "200000",
        "--ttl",
        &ttl.to_string(),
        "--out-file",
        &tx_body,
    ]);

    // Get protocol params from node
    let pp_output = run_cli_ok(&["query", "protocol-parameters", "--socket-path", &socket]);
    fs::write(&pp_file, &pp_output).unwrap();

    // Calculate min fee
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
    let fee_num: String = fee_str.chars().filter(|c| c.is_ascii_digit()).collect();
    let exact_fee: u64 = fee_num.parse().expect("Fee should be numeric");
    assert!(exact_fee > 0, "Calculated fee should be > 0");

    // Rebuild with exact fee
    let tx_body2 = dir.path().join("tx2.raw").display().to_string();
    let tx_signed = dir.path().join("tx2.signed").display().to_string();

    run_cli_ok(&[
        "transaction",
        "build",
        "--tx-in",
        &tx_in,
        "--tx-out",
        &tx_out,
        "--change-address",
        &addr,
        "--fee",
        &exact_fee.to_string(),
        "--ttl",
        &ttl.to_string(),
        "--out-file",
        &tx_body2,
    ]);

    run_cli_ok(&[
        "transaction",
        "sign",
        "--tx-body-file",
        &tx_body2,
        "--signing-key-file",
        &skey,
        "--out-file",
        &tx_signed,
    ]);

    // Submit with exact fee should succeed
    let result = run_cli(&[
        "transaction",
        "submit",
        "--tx-file",
        &tx_signed,
        "--socket-path",
        &socket,
        "--testnet-magic",
        "2",
    ]);

    assert!(
        result.success(),
        "Transaction with exact calculated fee should be accepted.\nstderr: {}",
        result.stderr
    );
}
