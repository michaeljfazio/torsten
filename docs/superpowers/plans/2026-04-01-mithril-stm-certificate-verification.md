# Mithril STM Certificate Chain Verification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add cryptographic verification of the Mithril STM certificate chain during snapshot import, so that tampered snapshots are rejected before the node uses them.

**Architecture:** Use the official `mithril-client` crate to verify the certificate chain from the snapshot's certificate back to the genesis certificate. After chain verification, reconstruct the protocol message from the extracted files and confirm it matches what the signers certified. A `--skip-certificate-verification` flag preserves current behavior for testing.

**Tech Stack:** `mithril-client` v0.13 (with `num-integer-backend`, `rustls-tls-webpki-roots`, `fs` features)

**Spec:** `docs/superpowers/specs/2026-04-01-mithril-stm-certificate-verification-design.md`

---

### Task 1: Add mithril-client dependency

**Files:**
- Modify: `Cargo.toml` (workspace root, line ~127 in `[workspace.dependencies]`)
- Modify: `crates/dugite-node/Cargo.toml` (line ~58, dependencies section)

- [ ] **Step 1: Add mithril-client to workspace dependencies**

In `Cargo.toml` (workspace root), add after the `rayon = "1"` line in `[workspace.dependencies]`:

```toml
mithril-client = { version = "0.13", default-features = false, features = ["num-integer-backend", "rustls-tls-webpki-roots", "fs"] }
```

- [ ] **Step 2: Add mithril-client to dugite-node dependencies**

In `crates/dugite-node/Cargo.toml`, add after the `tokio-util` line:

```toml
mithril-client = { workspace = true }
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p dugite-node 2>&1 | tail -5`
Expected: compilation succeeds (warnings OK at this stage)

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml crates/dugite-node/Cargo.toml Cargo.lock
git commit -m "deps: add mithril-client for STM certificate verification (#313)"
```

---

### Task 2: Add genesis verification keys and certificate_hash to API types

**Files:**
- Modify: `crates/dugite-node/src/mithril.rs`

- [ ] **Step 1: Add genesis verification key constants**

After the existing aggregator URL constants (line 31), add:

```rust
// ---------------------------------------------------------------------------
// Mithril genesis verification keys (from mithril-infra/configuration/)
// ---------------------------------------------------------------------------

/// Mainnet genesis verification key (Ed25519, JSON hex-encoded)
const MAINNET_GENESIS_VKEY: &str =
    "5b3139312c36362c3134302c3138352c3133382c31312c3233372c3230372c3235302c3134342c32372c322c3138382c33302c31322c38312c3135352c3230342c31302c3137392c37352c32332c3133382c3139362c3231372c352c31342c32302c35372c37392c33392c3137365d";

/// Preview genesis verification key (Ed25519, JSON hex-encoded)
const PREVIEW_GENESIS_VKEY: &str =
    "5b3132372c37332c3132342c3136312c362c3133372c3133312c3231332c3230372c3131372c3139382c38352c3137362c3139392c3136322c3234312c36382c3132332c3131392c3134352c31332c3233322c3234332c34392c3232392c322c3234392c3230352c3230352c33392c3233352c34345d";

/// Preprod genesis verification key (Ed25519, JSON hex-encoded)
/// Note: same key as preview
const PREPROD_GENESIS_VKEY: &str =
    "5b3132372c37332c3132342c3136312c362c3133372c3133312c3231332c3230372c3131372c3139382c38352c3137362c3139392c3136322c3234312c36382c3132332c3131392c3134352c31332c3233322c3234332c34392c3232392c322c3234392c3230352c3230352c33392c3233352c34345d";

/// Get the genesis verification key for a given network magic.
fn genesis_verification_key(network_magic: u64) -> Option<&'static str> {
    match network_magic {
        764824073 => Some(MAINNET_GENESIS_VKEY),
        2 => Some(PREVIEW_GENESIS_VKEY),
        1 => Some(PREPROD_GENESIS_VKEY),
        _ => None,
    }
}
```

- [ ] **Step 2: Add certificate_hash to SnapshotListItem**

Change the `SnapshotListItem` struct to include `certificate_hash`:

```rust
#[derive(Debug, serde::Deserialize)]
struct SnapshotListItem {
    digest: String,
    certificate_hash: String,
    #[serde(rename = "network")]
    _network: String,
    size: u64,
    #[serde(rename = "beacon")]
    beacon: SnapshotBeacon,
    #[serde(rename = "compression_algorithm", default)]
    _compression_algorithm: Option<String>,
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p dugite-node 2>&1 | tail -5`
Expected: compiles (genesis_verification_key unused warning is fine)

- [ ] **Step 4: Commit**

```bash
git add crates/dugite-node/src/mithril.rs
git commit -m "feat(mithril): add genesis verification keys and certificate_hash field (#313)"
```

---

### Task 3: Add CLI arguments for certificate verification control

**Files:**
- Modify: `crates/dugite-node/src/main.rs`

- [ ] **Step 1: Add CLI args to MithrilImportArgs**

Add two new fields to the `MithrilImportArgs` struct (after `temp_dir`):

```rust
/// Override the Mithril genesis verification key (for private networks).
/// The key must be a JSON hex-encoded Ed25519 verification key string.
#[arg(long)]
mithril_genesis_vkey: Option<String>,

/// Skip Mithril STM certificate chain verification (UNSAFE — for testing only).
/// When set, the snapshot digest is trusted from the aggregator without
/// cryptographic proof of the certificate chain.
#[arg(long)]
skip_certificate_verification: bool,
```

- [ ] **Step 2: Update run_mithril_import to pass new args**

Update the `run_mithril_import` function to pass the new args through:

```rust
async fn run_mithril_import(args: MithrilImportArgs) -> Result<()> {
    info!(
        "Starting Mithril snapshot import for network magic {}",
        args.network_magic
    );
    mithril::import_snapshot(
        args.network_magic,
        &args.database_path,
        args.temp_dir.as_deref(),
        args.mithril_genesis_vkey.as_deref(),
        args.skip_certificate_verification,
    )
    .await
}
```

- [ ] **Step 3: Verify it compiles (expect error — signature mismatch)**

Run: `cargo check -p dugite-node 2>&1 | tail -10`
Expected: compile error because `import_snapshot` signature doesn't match yet. This is expected — Task 4 will fix it.

- [ ] **Step 4: Commit**

```bash
git add crates/dugite-node/src/main.rs
git commit -m "feat(mithril): add CLI args for certificate verification control (#313)"
```

---

### Task 4: Implement certificate chain verification in import_snapshot

**Files:**
- Modify: `crates/dugite-node/src/mithril.rs`

- [ ] **Step 1: Update import_snapshot signature**

Change the `import_snapshot` function signature to accept the new parameters:

```rust
pub async fn import_snapshot(
    network_magic: u64,
    database_path: &Path,
    temp_dir: Option<&Path>,
    genesis_vkey_override: Option<&str>,
    skip_verification: bool,
) -> Result<()> {
```

- [ ] **Step 2: Replace the SECURITY NOTE block with certificate verification**

Replace the entire block from `// SECURITY NOTE: STM certificate chain verification is NOT implemented.` through the `warn!("Mithril STM certificate chain verification is NOT implemented...")` call (lines 211-245) with:

```rust
    // Step 5b: Verify the Mithril STM certificate chain.
    //
    // This cryptographically proves that ≥ 2/3 of Cardano stake signed this
    // snapshot by walking the certificate chain back to the genesis certificate
    // and verifying each STM multi-signature.
    if skip_verification {
        warn!(
            "Mithril STM certificate chain verification SKIPPED (--skip-certificate-verification). \
             The snapshot is trusted without cryptographic proof. \
             Do NOT use this in production."
        );
    } else {
        let genesis_vkey = genesis_vkey_override
            .or_else(|| genesis_verification_key(network_magic))
            .context(
                "No Mithril genesis verification key for this network. \
                 Use --mithril-genesis-vkey to provide one for private networks.",
            )?;

        info!("Verifying Mithril STM certificate chain...");

        let mithril = mithril_client::ClientBuilder::aggregator(aggregator, genesis_vkey)
            .build()
            .context("Failed to build Mithril client")?;

        // Verify the full certificate chain from the snapshot's certificate
        // back to the genesis certificate. Each certificate's STM multi-signature
        // is verified against the aggregate verification key, and the genesis
        // certificate's Ed25519 signature is verified against the hardcoded key.
        let certificate = mithril
            .certificate()
            .verify_chain(&latest.certificate_hash)
            .await
            .context("Mithril certificate chain verification FAILED — snapshot rejected")?;

        info!(
            certificate_hash = %latest.certificate_hash,
            epoch = certificate.epoch,
            "Certificate chain verified"
        );

        // Verify that the extracted snapshot content matches what the Mithril
        // signers actually certified. This re-hashes all immutable files and
        // checks the digest against the certificate's signed message.
        let message = mithril_client::MessageBuilder::new()
            .compute_snapshot_message(&certificate, &extract_dir)
            .await
            .context("Failed to compute snapshot message from extracted files")?;

        if !certificate.match_message(&message) {
            anyhow::bail!(
                "Mithril snapshot content does not match the certified message. \
                 The snapshot may have been tampered with after signing."
            );
        }

        info!("Snapshot content verified against certificate");
    }
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p dugite-node 2>&1 | tail -10`
Expected: compiles successfully

- [ ] **Step 4: Run all dugite-node tests**

Run: `cargo nextest run -p dugite-node 2>&1 | tail -20`
Expected: all existing tests pass

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-node/src/mithril.rs
git commit -m "feat(mithril): implement STM certificate chain verification (#313)

Adds full Mithril STM certificate chain verification during snapshot import.
Uses the official mithril-client library to:
- Walk the certificate chain from snapshot cert back to genesis
- Verify STM multi-signatures on each certificate
- Verify genesis certificate Ed25519 signature
- Re-hash extracted files and match against signed message

Supports --skip-certificate-verification for testing and
--mithril-genesis-vkey for private networks."
```

---

### Task 5: Add unit tests

**Files:**
- Modify: `crates/dugite-node/src/mithril.rs`

- [ ] **Step 1: Add genesis key lookup tests**

Add to the existing `#[cfg(test)] mod tests` block in mithril.rs:

```rust
    #[test]
    fn test_genesis_verification_key_known_networks() {
        // Mainnet
        assert!(genesis_verification_key(764824073).is_some());
        let mainnet_key = genesis_verification_key(764824073).unwrap();
        assert!(mainnet_key.starts_with("5b31393"));
        assert_ne!(mainnet_key, genesis_verification_key(2).unwrap(),
            "mainnet key should differ from preview");

        // Preview
        assert!(genesis_verification_key(2).is_some());

        // Preprod (same key as preview)
        assert!(genesis_verification_key(1).is_some());
        assert_eq!(
            genesis_verification_key(2).unwrap(),
            genesis_verification_key(1).unwrap(),
            "preview and preprod share the same genesis key"
        );
    }

    #[test]
    fn test_genesis_verification_key_unknown_network() {
        assert!(genesis_verification_key(999).is_none());
        assert!(genesis_verification_key(0).is_none());
    }

    #[test]
    fn test_genesis_keys_are_valid_hex() {
        // Each genesis key should be a valid hex string that decodes to a JSON array
        for magic in [764824073, 2, 1] {
            let key = genesis_verification_key(magic).unwrap();
            let decoded = hex::decode(key)
                .unwrap_or_else(|_| panic!("genesis key for magic {magic} is not valid hex"));
            let json_str = std::str::from_utf8(&decoded)
                .unwrap_or_else(|_| panic!("genesis key for magic {magic} is not valid UTF-8"));
            assert!(json_str.starts_with('[') && json_str.ends_with(']'),
                "genesis key for magic {magic} should decode to a JSON array, got: {json_str}");
        }
    }
```

- [ ] **Step 2: Run tests**

Run: `cargo nextest run -p dugite-node -E 'test(genesis_verification_key)' 2>&1 | tail -10`
Expected: all 3 tests pass

- [ ] **Step 3: Add integration test for real certificate chain verification (ignored by default)**

Add to the test module:

```rust
    /// Integration test: verify a real Mithril preview certificate chain.
    ///
    /// This test hits the real Mithril aggregator API and verifies that we can
    /// successfully build a client, fetch a snapshot, and verify its certificate
    /// chain back to genesis. Run manually with:
    ///   cargo nextest run -p dugite-node -E 'test(verify_preview_certificate_chain)' -- --ignored
    #[tokio::test]
    #[ignore]
    async fn test_verify_preview_certificate_chain() {
        let aggregator = aggregator_url(2); // preview
        let genesis_vkey = genesis_verification_key(2).unwrap();

        // Build the Mithril client
        let client = mithril_client::ClientBuilder::aggregator(aggregator, genesis_vkey)
            .build()
            .expect("Failed to build Mithril client");

        // Fetch latest snapshot to get its certificate_hash
        let http = reqwest::Client::builder()
            .user_agent("dugite-test/0.1")
            .build()
            .unwrap();

        let snapshots: Vec<serde_json::Value> = http
            .get(format!("{aggregator}/artifact/snapshots"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        let cert_hash = snapshots[0]["certificate_hash"]
            .as_str()
            .expect("No certificate_hash in snapshot");

        // Verify the certificate chain — this is the core test
        let certificate = client
            .certificate()
            .verify_chain(cert_hash)
            .await
            .expect("Certificate chain verification failed");

        assert!(certificate.epoch > 0, "certificate epoch should be positive");
        println!(
            "Certificate chain verified: epoch={}, hash={}",
            certificate.epoch, cert_hash
        );
    }
```

- [ ] **Step 4: Run the unit tests (not the ignored integration test)**

Run: `cargo nextest run -p dugite-node -E 'test(genesis)' 2>&1 | tail -10`
Expected: all genesis key tests pass

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-node/src/mithril.rs
git commit -m "test(mithril): add genesis key and certificate chain verification tests (#313)"
```

---

### Task 6: Lint, format, and final verification

**Files:** All modified files

- [ ] **Step 1: Format check**

Run: `cargo fmt --all -- --check 2>&1 | tail -10`
Expected: no formatting issues. If there are, run `cargo fmt --all` and continue.

- [ ] **Step 2: Clippy**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -20`
Expected: no warnings. Fix any that appear.

- [ ] **Step 3: Full test suite**

Run: `cargo nextest run --workspace 2>&1 | tail -20`
Expected: all tests pass

- [ ] **Step 4: Commit any fixes**

```bash
git add -A
git commit -m "style(mithril): fix formatting and clippy warnings (#313)"
```
