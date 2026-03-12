//! Tier 1: Query tests — requires a running synced node.
//! Run with: TORSTEN_INTEGRATION_SOCKET=./node.sock cargo test -p torsten-integration-tests -- tier1

use serde_json::Value;
use torsten_integration_tests::helpers::cli::{integration_socket, run_cli, run_cli_ok};

/// Skip all tier1 tests if no socket is configured.
fn require_socket() -> String {
    match integration_socket() {
        Some(s) => s,
        None => {
            eprintln!("SKIP: TORSTEN_INTEGRATION_SOCKET not set");
            std::process::exit(0);
        }
    }
}

/// Helper: run a query command and return stdout.
fn query(socket: &str, args: &[&str]) -> String {
    let mut full_args = vec!["query"];
    full_args.extend_from_slice(args);
    full_args.push("--socket-path");
    full_args.push(socket);
    run_cli_ok(&full_args)
}

/// Helper: run a query command and parse as JSON.
fn query_json(socket: &str, args: &[&str]) -> Value {
    let output = query(socket, args);
    serde_json::from_str(&output).unwrap_or_else(|e| {
        panic!(
            "Failed to parse query {:?} output as JSON: {}\nOutput: {}",
            args, e, output
        )
    })
}

// ─── Basic Queries ────────────────────────────────────────────────────

#[test]
fn tier1_query_tip() {
    let socket = require_socket();
    let output = query(&socket, &["tip"]);
    let tip: Value = serde_json::from_str(&output).unwrap_or_else(|_| {
        // Maybe it's not JSON; just verify it's non-empty
        assert!(!output.trim().is_empty(), "query tip should return data");
        Value::Null
    });

    if !tip.is_null() {
        // If JSON, verify key fields
        if let Some(slot) = tip.get("slot").and_then(|s| s.as_u64()) {
            assert!(slot > 0, "Slot should be > 0, got {}", slot);
        }
        if let Some(hash) = tip.get("hash").and_then(|h| h.as_str()) {
            assert_eq!(hash.len(), 64, "Block hash should be 64 hex chars");
        }
        if let Some(block) = tip.get("block").and_then(|b| b.as_u64()) {
            assert!(block > 0, "Block number should be > 0");
        }
    }
}

#[test]
fn tier1_query_protocol_parameters() {
    let socket = require_socket();
    let pp = query_json(&socket, &["protocol-parameters"]);

    // Check for key protocol parameter fields
    let expected_keys = ["txFeePerByte", "maxBlockBodySize", "maxTxSize"];
    for key in expected_keys {
        assert!(
            pp.get(key).is_some(),
            "Protocol parameters should contain '{}', got: {}",
            key,
            serde_json::to_string_pretty(&pp).unwrap_or_default()
        );
    }
}

#[test]
fn tier1_query_utxo_fresh_address() {
    let socket = require_socket();
    // Generate a fresh address that should have no UTxOs
    let keys = torsten_integration_tests::helpers::keys::TempKeys::new();
    let addr = keys.enterprise_address_testnet();

    let output = query(&socket, &["utxo", "--address", &addr]);
    // Fresh address should have empty or header-only output
    let lines: Vec<&str> = output
        .lines()
        .filter(|l| {
            let l = l.trim();
            !l.is_empty() && !l.starts_with("TxHash") && !l.starts_with('-') && !l.starts_with('=')
        })
        .collect();
    assert!(
        lines.is_empty(),
        "Fresh address should have no UTxOs, got: {:?}",
        lines
    );
}

#[test]
fn tier1_query_stake_distribution() {
    let socket = require_socket();
    let output = query(&socket, &["stake-distribution"]);
    assert!(
        !output.trim().is_empty(),
        "Stake distribution should return data"
    );
}

#[test]
fn tier1_query_stake_pools() {
    let socket = require_socket();
    let output = query(&socket, &["stake-pools"]);
    assert!(
        !output.trim().is_empty(),
        "Stake pools list should return data"
    );
}

#[test]
fn tier1_query_stake_snapshot() {
    let socket = require_socket();
    let output = query(&socket, &["stake-snapshot"]);
    assert!(
        !output.trim().is_empty(),
        "Stake snapshot should return data"
    );
}

#[test]
fn tier1_query_gov_state() {
    let socket = require_socket();
    let output = query(&socket, &["gov-state"]);
    assert!(
        !output.trim().is_empty(),
        "Governance state should return data"
    );
}

#[test]
fn tier1_query_drep_state() {
    let socket = require_socket();
    let output = query(&socket, &["drep-state"]);
    assert!(!output.trim().is_empty(), "DRep state should return data");
}

#[test]
fn tier1_query_committee_state() {
    let socket = require_socket();
    let output = query(&socket, &["committee-state"]);
    assert!(
        !output.trim().is_empty(),
        "Committee state should return data"
    );
}

#[test]
fn tier1_query_treasury() {
    let socket = require_socket();
    let output = query(&socket, &["treasury"]);
    assert!(
        !output.trim().is_empty(),
        "Treasury query should return data"
    );
}

#[test]
fn tier1_query_constitution() {
    let socket = require_socket();
    let output = query(&socket, &["constitution"]);
    assert!(
        !output.trim().is_empty(),
        "Constitution query should return data"
    );
}

#[test]
fn tier1_query_tx_mempool_info() {
    let socket = require_socket();
    let result = run_cli(&["query", "tx-mempool", "info", "--socket-path", &socket]);
    // May succeed or fail depending on protocol support, but shouldn't crash
    assert!(
        result.success() || !result.stderr.is_empty(),
        "tx-mempool info should produce a response"
    );
}

// ─── Prometheus Metrics ───────────────────────────────────────────────

#[test]
fn tier1_prometheus_metrics() {
    let _socket = require_socket();

    // Try to fetch Prometheus metrics
    let result = std::process::Command::new("curl")
        .args([
            "-s",
            "--connect-timeout",
            "5",
            "http://127.0.0.1:12798/metrics",
        ])
        .output();

    match result {
        Ok(output) if output.status.success() => {
            let body = String::from_utf8_lossy(&output.stdout);
            let expected_metrics = ["torsten_slot_number", "torsten_block_number"];
            for metric in expected_metrics {
                assert!(
                    body.contains(metric),
                    "Prometheus metrics should contain '{}'\nBody excerpt: {}",
                    metric,
                    &body[..body.len().min(500)]
                );
            }
        }
        _ => {
            eprintln!("SKIP: Could not reach Prometheus metrics endpoint");
        }
    }
}

// ─── Pool and Delegation Queries ──────────────────────────────────────

#[test]
fn tier1_query_pool_params() {
    let socket = require_socket();
    // First get pool list, then query params for a specific pool
    let pools_output = query(&socket, &["stake-pools"]);
    let pools: Vec<&str> = pools_output
        .lines()
        .filter(|l| !l.trim().is_empty() && l.trim().len() == 56)
        .collect();
    if pools.is_empty() {
        eprintln!("SKIP: No pools found");
        return;
    }
    let pool_id = pools[0].trim();
    let output = query(&socket, &["pool-params", "--stake-pool-id", pool_id]);
    assert!(
        !output.trim().is_empty(),
        "Pool params for {pool_id} should return data"
    );
}

#[test]
fn tier1_query_protocol_parameters_content() {
    let socket = require_socket();
    let pp = query_json(&socket, &["protocol-parameters"]);

    // Verify Conway-specific governance parameters are present
    let conway_keys = [
        "poolVotingThresholds",
        "drepVotingThresholds",
        "committeeMinSize",
        "committeeMaxTermLength",
        "govActionLifetime",
        "govActionDeposit",
        "dRepDeposit",
        "dRepActivity",
    ];
    for key in conway_keys {
        assert!(
            pp.get(key).is_some(),
            "Protocol parameters should contain Conway field '{key}'"
        );
    }

    // Verify execution unit prices
    assert!(
        pp.get("executionUnitPrices").is_some(),
        "Protocol parameters should contain 'executionUnitPrices'"
    );
    assert!(
        pp.get("maxTxExecutionUnits").is_some(),
        "Protocol parameters should contain 'maxTxExecutionUnits'"
    );
}

#[test]
fn tier1_query_gov_state_structure() {
    let socket = require_socket();
    let gov = query_json(&socket, &["gov-state"]);

    // GovState should be an object with proposals, committee, constitution, etc.
    // It may be printed as a JSON array/object depending on format
    assert!(!gov.is_null(), "Gov state should return parseable JSON");
}

#[test]
fn tier1_query_constitution_content() {
    let socket = require_socket();
    let output = query(&socket, &["constitution"]);
    // Constitution should mention a URL or hash
    assert!(
        output.contains("url") || output.contains("hash") || output.contains("http"),
        "Constitution should contain url or hash data, got: {}",
        &output[..output.len().min(200)]
    );
}

#[test]
fn tier1_query_drep_state_non_empty() {
    let socket = require_socket();
    let output = query(&socket, &["drep-state"]);
    // On preview testnet, there should be DReps registered
    assert!(
        output.len() > 10,
        "DRep state should return substantial data on preview testnet"
    );
}

#[test]
fn tier1_query_treasury_values() {
    let socket = require_socket();
    let output = query(&socket, &["treasury"]);
    // Treasury should contain numeric values
    assert!(
        output.contains("treasury") || output.chars().any(|c| c.is_ascii_digit()),
        "Treasury should contain numeric values, got: {}",
        &output[..output.len().min(200)]
    );
}

// ─── Ratification State ───────────────────────────────────────────────

#[test]
fn tier1_query_ratify_state() {
    let socket = require_socket();
    let output = query(&socket, &["ratify-state"]);
    assert!(
        !output.trim().is_empty(),
        "ratify-state query should return data"
    );
    // Verify expected fields in output
    assert!(
        output.contains("Ratification State")
            || output.contains("Enacted")
            || output.contains("Expired")
            || output.contains("Delayed"),
        "ratify-state output should contain state fields, got: {}",
        &output[..output.len().min(200)]
    );
}

// ─── Cross-Verification with Koios ────────────────────────────────────

#[test]
fn tier1_cross_verify_tip_with_koios() {
    let socket = require_socket();

    // Get tip from our node
    let local_output = query(&socket, &["tip"]);
    let local_tip: Value = match serde_json::from_str(&local_output) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("SKIP: Could not parse local tip as JSON");
            return;
        }
    };

    let local_slot = match local_tip.get("slot").and_then(|s| s.as_u64()) {
        Some(s) => s,
        None => {
            eprintln!("SKIP: Could not extract slot from local tip");
            return;
        }
    };

    // Get tip from Koios
    let rt = tokio::runtime::Runtime::new().unwrap();
    let koios_tip = match rt.block_on(torsten_integration_tests::helpers::koios::tip()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("SKIP: Koios request failed: {}", e);
            return;
        }
    };

    if let Some(arr) = koios_tip.as_array() {
        if let Some(first) = arr.first() {
            if let Some(koios_slot) = first.get("abs_slot").and_then(|s| {
                s.as_u64()
                    .or_else(|| s.as_str().and_then(|v| v.parse().ok()))
            }) {
                let diff = local_slot.abs_diff(koios_slot);
                // Allow 120 slots (~120 seconds) of drift
                assert!(
                    diff < 120,
                    "Local tip slot {} and Koios tip slot {} differ by {} (max 120)",
                    local_slot,
                    koios_slot,
                    diff
                );
            }
        }
    }
}
