# Mithril STM Certificate Chain Verification

**Issue:** #313
**Date:** 2026-04-01
**Status:** Design

## Problem

The Mithril snapshot import (`crates/dugite-node/src/mithril.rs`) trusts the aggregator API's digest without cryptographic verification of the STM multi-signature certificate chain. An attacker controlling the aggregator URL (or performing MITM) can serve a malicious snapshot that passes the current SHA256 digest check because the digest itself comes from the untrusted aggregator.

## Approach

Use the official `mithril-client` crate (v0.13.2) which provides:
- Certificate chain walking and verification (`client.certificate().verify_chain()`)
- STM multi-signature verification against aggregate verification keys
- Genesis certificate Ed25519 signature verification
- Protocol message reconstruction and matching (`MessageBuilder`)

This avoids reimplementing security-critical STM cryptography and stays in sync with Mithril protocol evolution.

## Design

### Dependency Configuration

Add `mithril-client` to `dugite-node` with pure-Rust backends to match the workspace's existing `rustls-tls` / no-system-deps approach:

```toml
mithril-client = { version = "0.13", default-features = false, features = [
    "num-integer-backend",
    "rustls-tls-webpki-roots",
    "fs",
] }
```

- `num-integer-backend` — pure Rust big-integer math (avoids GMP/rug system dependency)
- `rustls-tls-webpki-roots` — pure Rust TLS (matches workspace's `reqwest` config)
- `fs` — enables `MessageBuilder::compute_snapshot_message()` which re-digests the extracted immutable files to verify they match what was signed

### Genesis Verification Keys

Hardcoded per network, matching the keys published by IOG in the Mithril infra repo:

```rust
fn genesis_verification_key(network_magic: u64) -> Option<&'static str> {
    match network_magic {
        764824073 => Some(MAINNET_GENESIS_VKEY),
        2 => Some(PREVIEW_GENESIS_VKEY),
        1 => Some(PREPROD_GENESIS_VKEY),
        _ => None,
    }
}
```

The keys are JSON hex-encoded Ed25519 verification keys (long strings ~200 chars). A CLI override `--mithril-genesis-vkey` allows using custom keys for private networks or updated keys.

### Modified Import Flow

Current flow (steps 1-8 in `import_snapshot()`):

```
1. Fetch snapshot list → 2. Get download URL → 3. Download archive →
4. Extract → 5. Verify SHA256 digest → 6. (skip) → 7. Move files → 8. Cleanup
```

New flow inserts certificate verification between steps 5 and 7:

```
1. Fetch snapshot list → 2. Get download URL → 3. Download archive →
4. Extract → 5. Verify SHA256 digest →
5b. Build mithril-client → 5c. Verify certificate chain → 5d. Match message →
7. Move files → 8. Cleanup
```

**Step 5b: Build mithril-client**

```rust
let genesis_vkey = genesis_verification_key(network_magic)
    .or(cli_override)
    .context("No genesis verification key for this network")?;

let client = mithril_client::ClientBuilder::aggregator(aggregator, genesis_vkey)
    .build()?;
```

**Step 5c: Verify certificate chain**

The `SnapshotListItem` response includes a `certificate_hash` field (not currently deserialized). We add it to the struct, then:

```rust
let certificate = client
    .certificate()
    .verify_chain(&latest.certificate_hash)
    .await
    .context("Mithril certificate chain verification failed")?;
```

This walks from the snapshot's certificate back through `previous_hash` links to the genesis certificate, verifying each STM multi-signature and the genesis Ed25519 signature.

**Step 5d: Verify message matches snapshot**

```rust
let message = mithril_client::MessageBuilder::new()
    .compute_snapshot_message(&certificate, &extract_dir)
    .await
    .context("Failed to compute snapshot message")?;

if !certificate.match_message(&message) {
    anyhow::bail!(
        "Mithril certificate does not match snapshot content. \
         The snapshot may have been tampered with."
    );
}
```

This re-hashes the extracted immutable files and confirms the digest matches what the Mithril signers actually certified.

### CLI Changes

Add to `MithrilImportArgs`:

```rust
/// Override the Mithril genesis verification key (for private networks)
#[arg(long)]
mithril_genesis_vkey: Option<String>,

/// Skip Mithril certificate chain verification (UNSAFE — for testing only)
#[arg(long)]
skip_certificate_verification: bool,
```

The `--skip-certificate-verification` flag preserves the current behavior for development/testing but emits a prominent warning. The function signature becomes:

```rust
pub async fn import_snapshot(
    network_magic: u64,
    database_path: &Path,
    temp_dir: Option<&Path>,
    genesis_vkey_override: Option<&str>,
    skip_verification: bool,
) -> Result<()>
```

### API Response Changes

Add `certificate_hash` to `SnapshotListItem` and `SnapshotDetail`:

```rust
struct SnapshotListItem {
    digest: String,
    certificate_hash: String,  // NEW
    // ...
}
```

### Error Handling

Certificate verification failures are fatal — the import aborts and the extracted files are cleaned up. Error messages distinguish:
- Chain fetch failure (network issue vs aggregator problem)
- STM signature verification failure (tampered certificate)
- Genesis key mismatch (wrong key for network)
- Message mismatch (snapshot content doesn't match certificate)

### Security Note Removal

Remove the 20-line `SECURITY NOTE` comment block (lines 211-239) and the `warn!()` call, replacing with an `info!()` confirming successful verification.

## Testing

1. **Integration test: real preview snapshot** — Fetch the latest preview snapshot metadata + certificate chain, verify the chain validates. This test hits the real aggregator API (mark `#[ignore]` for CI, run manually).

2. **Unit test: skip flag emits warning** — Verify that `skip_certificate_verification = true` skips verification and logs a warning.

3. **Unit test: missing genesis key fails** — Verify that an unknown network magic without `--mithril-genesis-vkey` returns an error.

4. **Unit test: genesis key lookup** — Verify the correct genesis key is returned for each known network magic.

## Files Changed

| File | Change |
|------|--------|
| `Cargo.toml` (workspace) | Add `mithril-client` to `[workspace.dependencies]` |
| `crates/dugite-node/Cargo.toml` | Add `mithril-client` dependency |
| `crates/dugite-node/src/mithril.rs` | Add certificate verification logic, update API types, add genesis keys |
| `crates/dugite-node/src/main.rs` | Add CLI args, pass to `import_snapshot()` |

## Risks & Mitigations

| Risk | Mitigation |
|------|-----------|
| `mithril-client` dependency tree bloat | Use minimal features, pure-Rust backends only |
| `mithril-client` API breaking changes | Pin to `0.13.x`, update as needed |
| Genesis key rotation | CLI override flag + update hardcoded keys on rotation |
| Aggregator downtime during verification | Certificate chain is fetched from same aggregator; if aggregator is down, download would also fail |
| Compile time increase from mithril deps | Acceptable tradeoff for security correctness |
