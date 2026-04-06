# Leadership Schedule CLI Alignment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rewrite `dugite-cli query leadership-schedule` to match `cardano-cli query leadership-schedule` interface exactly — connecting to a running node via socket, querying epoch nonce / stake distribution / epoch boundaries automatically, and producing identical JSON output.

**Architecture:** Replace the current offline-parameter approach with a node-connected flow. The command connects via N2C, queries tip (epoch/slot), protocol state (nonces), and stake snapshots (pool/total stake), reads shelley genesis for slot timing, then calls the existing `compute_leader_schedule()` function and formats output as JSON with slot times.

**Tech Stack:** Existing `N2CClient`, `minicbor` CBOR parsing, `compute_leader_schedule()` from `dugite-consensus`, `serde_json` for output.

---

### Task 1: Update CLI Argument Structure

**Files:**
- Modify: `crates/dugite-cli/src/commands/query.rs:149-169` (LeadershipSchedule variant)

- [ ] **Step 1: Replace the LeadershipSchedule enum variant**

Replace the current offline-parameter fields with cardano-cli-compatible arguments:

```rust
    /// Get the slots the node is expected to mint a block in
    LeadershipSchedule {
        /// Path to the node socket. Overrides CARDANO_NODE_SOCKET_PATH env.
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        /// Use the mainnet magic id
        #[arg(long, group = "network")]
        mainnet: bool,
        /// Specify a testnet magic id
        #[arg(long, group = "network")]
        testnet_magic: Option<u64>,
        /// Shelley genesis filepath
        #[arg(long)]
        genesis: PathBuf,
        /// Stake pool ID (hex-encoded hash)
        #[arg(long)]
        stake_pool_id: Option<String>,
        /// Filepath of the cold verification key
        #[arg(long)]
        cold_verification_key_file: Option<PathBuf>,
        /// Input filepath of the VRF signing key
        #[arg(long)]
        vrf_signing_key_file: PathBuf,
        /// Get the leadership schedule for the current epoch
        #[arg(long, group = "epoch_choice")]
        current: bool,
        /// Get the leadership schedule for the following epoch
        #[arg(long, group = "epoch_choice")]
        next: bool,
        /// Format output to JSON (default)
        #[arg(long)]
        output_json: bool,
        /// Format output to TEXT
        #[arg(long)]
        output_text: bool,
        /// Optional output file. Default is stdout.
        #[arg(long)]
        out_file: Option<PathBuf>,
    },
```

- [ ] **Step 2: Verify it compiles (handler will be addressed in Task 3)**

Temporarily stub the match arm body so it compiles:

```rust
QuerySubcommand::LeadershipSchedule { .. } => {
    anyhow::bail!("Not yet implemented");
}
```

Run: `cargo check -p dugite-cli`
Expected: compiles with no errors

- [ ] **Step 3: Commit**

```bash
git add crates/dugite-cli/src/commands/query.rs
git commit -m "refactor(cli): update leadership-schedule args to match cardano-cli interface"
```

---

### Task 2: Add Protocol State CBOR Parser

**Files:**
- Modify: `crates/dugite-cli/src/commands/query.rs` (add helper function near line ~350, after existing helpers)

The N2C `query_protocol_state()` returns raw CBOR for the PraosState. We need to parse
the epoch nonce and candidate nonce out of it. The wire format is:

```
MsgResult: array [tag, HFC_wrapper [
  array(2) [0,           -- version
    array(7) [            -- PraosState
      lastSlot,           -- [0] WithOrigin: array(1)[0] or array(2)[1, slot]
      ocertCounters,      -- [1] Map<bytes, u64>
      evolvingNonce,      -- [2] Nonce: array(1)[0] or array(2)[1, bytes32]
      candidateNonce,     -- [3] Nonce
      epochNonce,         -- [4] Nonce
      labNonce,           -- [5] Nonce
      lastEpochBlockNonce -- [6] Nonce
    ]
  ]
]]
```

- [ ] **Step 1: Add the PraosNonces struct and parser**

Add after the `release_and_done` function (~line 353):

```rust
/// Nonce values extracted from PraosState (protocol state query).
struct PraosNonces {
    /// The epoch nonce used for VRF leader checks in the current epoch.
    epoch_nonce: [u8; 32],
    /// The candidate nonce (becomes the epoch nonce for the next epoch).
    candidate_nonce: [u8; 32],
}

/// Parse a Nonce CBOR value: array(1)[0] = NeutralNonce, array(2)[1, bytes32] = Nonce.
/// Returns 32 zero bytes for NeutralNonce.
fn parse_cbor_nonce(d: &mut minicbor::Decoder<'_>) -> Result<[u8; 32]> {
    let len = d
        .array()?
        .ok_or_else(|| anyhow::anyhow!("Expected definite-length nonce array"))?;
    let tag = d.u8()?;
    if tag == 0 && len == 1 {
        // NeutralNonce
        Ok([0u8; 32])
    } else if tag == 1 && len == 2 {
        let bytes = d.bytes()?;
        let mut nonce = [0u8; 32];
        if bytes.len() == 32 {
            nonce.copy_from_slice(bytes);
        }
        Ok(nonce)
    } else {
        anyhow::bail!("Invalid nonce encoding: tag={tag}, len={len}");
    }
}

/// Parse the raw MsgResult CBOR from `query_protocol_state()` and extract nonces.
///
/// Wire format: MsgResult [tag, HFC [array(2)[version=0, array(7)[...PraosState fields...]]]]
fn parse_protocol_state_nonces(raw: &[u8]) -> Result<PraosNonces> {
    let mut d = minicbor::Decoder::new(raw);

    // MsgResult outer array
    let _ = d.array();
    let tag = d.u32()?;
    if tag != 6 {
        anyhow::bail!("Protocol state query failed (tag {tag})");
    }

    // Strip HFC wrapper: array(2)[1, payload] or array(1)[payload]
    let pos = d.position();
    if let Ok(Some(2)) = d.array() {
        let _ = d.u64(); // HFC success tag
    } else if let Ok(Some(1)) = {
        d.set_position(pos);
        d.array()
    } {
        // single-element wrapper, consumed
    } else {
        d.set_position(pos);
    }

    // Versioned wrapper: array(2)[0, payload]
    let _ = d.array(); // array(2)
    let version = d.u8()?;
    if version != 0 {
        anyhow::bail!("Unexpected PraosState version: {version}");
    }

    // PraosState: array(7)
    let _ = d.array(); // array(7)

    // [0] lastSlot (WithOrigin) — skip
    let slot_len = d
        .array()?
        .ok_or_else(|| anyhow::anyhow!("Expected definite-length slot array"))?;
    let _ = d.u8(); // tag
    if slot_len == 2 {
        let _ = d.u64(); // slot value
    }

    // [1] ocertCounters (Map) — skip
    let map_len = d.map()?.unwrap_or(0);
    for _ in 0..map_len {
        let _ = d.bytes(); // pool hash
        let _ = d.u64(); // counter
    }

    // [2] evolvingNonce — skip
    let _ = parse_cbor_nonce(&mut d)?;

    // [3] candidateNonce
    let candidate_nonce = parse_cbor_nonce(&mut d)?;

    // [4] epochNonce
    let epoch_nonce = parse_cbor_nonce(&mut d)?;

    Ok(PraosNonces {
        epoch_nonce,
        candidate_nonce,
    })
}
```

- [ ] **Step 2: Write a unit test for the nonce parser**

Add to the `#[cfg(test)] mod tests` at the bottom of query.rs:

```rust
    #[test]
    fn test_parse_protocol_state_nonces() {
        // Build a minimal PraosState CBOR matching our wire format:
        // MsgResult: array(2)[6, HFC: array(2)[1, Versioned: array(2)[0, array(7)[
        //   lastSlot=[1,0], ocertCounters={}, evolving=[1,0], candidate=[0],
        //   epochNonce=[1, <32 bytes of 0xAA>], lab=[0], lastEpochBlock=[0]
        // ]]]]
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).ok(); // MsgResult
        enc.u32(6).ok(); // success tag
        enc.array(2).ok(); // HFC wrapper
        enc.u64(1).ok(); // HFC success
        enc.array(2).ok(); // Versioned
        enc.u8(0).ok(); // version 0
        enc.array(7).ok(); // PraosState
        // [0] lastSlot = Origin
        enc.array(1).ok();
        enc.u8(0).ok();
        // [1] ocertCounters = empty map
        enc.map(0).ok();
        // [2] evolvingNonce = NeutralNonce
        enc.array(1).ok();
        enc.u8(0).ok();
        // [3] candidateNonce = Nonce(0xBB * 32)
        enc.array(2).ok();
        enc.u8(1).ok();
        enc.bytes(&[0xBB; 32]).ok();
        // [4] epochNonce = Nonce(0xAA * 32)
        enc.array(2).ok();
        enc.u8(1).ok();
        enc.bytes(&[0xAA; 32]).ok();
        // [5] labNonce = NeutralNonce
        enc.array(1).ok();
        enc.u8(0).ok();
        // [6] lastEpochBlockNonce = NeutralNonce
        enc.array(1).ok();
        enc.u8(0).ok();

        let nonces = parse_protocol_state_nonces(&buf).unwrap();
        assert_eq!(nonces.epoch_nonce, [0xAA; 32]);
        assert_eq!(nonces.candidate_nonce, [0xBB; 32]);
    }
```

- [ ] **Step 3: Run the test**

Run: `cargo nextest run -p dugite-cli -E 'test(test_parse_protocol_state_nonces)'`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/dugite-cli/src/commands/query.rs
git commit -m "feat(cli): add PraosState CBOR parser for leadership-schedule nonces"
```

---

### Task 3: Add Stake Snapshot Parser

**Files:**
- Modify: `crates/dugite-cli/src/commands/query.rs` (add helper function after PraosNonces parser)

We need to extract a specific pool's stake and total active stake from the stake snapshot query.

- [ ] **Step 1: Add PoolStakeInfo struct and parser**

```rust
/// Pool stake and total active stake from a stake snapshot query.
struct PoolStakeInfo {
    /// Pool's delegated stake (lovelace) from the "set" snapshot (current) or "mark" (next).
    pool_stake: u64,
    /// Total active stake across all pools (lovelace).
    total_active_stake: u64,
}

/// Parse raw MsgResult from `query_stake_snapshot()` and extract a specific pool's stake.
///
/// Wire format: MsgResult [tag, HFC [array(4) [pool_map, mark_total, set_total, go_total]]]
/// pool_map: Map<pool_hash(28B), array(3)[mark, set, go]>
///
/// `use_mark` controls which snapshot to use: true = mark (for --next), false = set (for --current).
fn parse_stake_for_pool(raw: &[u8], pool_id_hex: &str, use_mark: bool) -> Result<PoolStakeInfo> {
    let mut d = minicbor::Decoder::new(raw);

    // MsgResult outer array
    let _ = d.array();
    let tag = d.u32()?;
    if tag != 6 {
        anyhow::bail!("Stake snapshot query failed (tag {tag})");
    }

    // Strip HFC wrapper
    let pos = d.position();
    if let Ok(Some(2)) = d.array() {
        let _ = d.u64();
    } else if let Ok(Some(1)) = {
        d.set_position(pos);
        d.array()
    } {
    } else {
        d.set_position(pos);
    }

    // array(4) [pool_map, mark_total, set_total, go_total]
    let _ = d.array();

    // pool_map: Map<pool_hash(28B), array(3)[mark, set, go]>
    let pool_count = d.map()?.unwrap_or(0);
    let mut pool_stake: Option<u64> = None;
    let pool_id_lower = pool_id_hex.to_lowercase();

    for _ in 0..pool_count {
        let pool_hash = hex::encode(d.bytes().unwrap_or(&[]));
        let _ = d.array(); // array(3)
        let mark = d.u64().unwrap_or(0);
        let set = d.u64().unwrap_or(0);
        let _go = d.u64().unwrap_or(0);

        if pool_hash == pool_id_lower {
            pool_stake = Some(if use_mark { mark } else { set });
        }
    }

    let total_mark = d.u64().unwrap_or(0);
    let total_set = d.u64().unwrap_or(0);
    let _total_go = d.u64().unwrap_or(0);

    let total_active_stake = if use_mark { total_mark } else { total_set };

    let pool_stake = pool_stake.ok_or_else(|| {
        anyhow::anyhow!(
            "Pool {} not found in stake snapshot. Is the pool registered and has delegated stake?",
            pool_id_hex
        )
    })?;

    Ok(PoolStakeInfo {
        pool_stake,
        total_active_stake,
    })
}
```

- [ ] **Step 2: Write a unit test**

```rust
    #[test]
    fn test_parse_stake_for_pool() {
        let pool_id = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef01";
        let pool_bytes = hex::decode(pool_id).unwrap();

        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).ok(); // MsgResult
        enc.u32(6).ok();
        enc.array(2).ok(); // HFC
        enc.u64(1).ok();
        enc.array(4).ok(); // stake snapshot

        // pool map with 1 entry
        enc.map(1).ok();
        enc.bytes(&pool_bytes).ok();
        enc.array(3).ok();
        enc.u64(5000_000_000).ok(); // mark
        enc.u64(4000_000_000).ok(); // set
        enc.u64(3000_000_000).ok(); // go

        // totals
        enc.u64(100_000_000_000).ok(); // mark_total
        enc.u64(90_000_000_000).ok(); // set_total
        enc.u64(80_000_000_000).ok(); // go_total

        // --current (use set)
        let info = parse_stake_for_pool(&buf, pool_id, false).unwrap();
        assert_eq!(info.pool_stake, 4_000_000_000);
        assert_eq!(info.total_active_stake, 90_000_000_000);

        // --next (use mark)
        let info = parse_stake_for_pool(&buf, pool_id, true).unwrap();
        assert_eq!(info.pool_stake, 5_000_000_000);
        assert_eq!(info.total_active_stake, 100_000_000_000);
    }
```

- [ ] **Step 3: Run tests**

Run: `cargo nextest run -p dugite-cli -E 'test(test_parse_stake_for_pool)'`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/dugite-cli/src/commands/query.rs
git commit -m "feat(cli): add stake snapshot parser for leadership-schedule pool lookup"
```

---

### Task 4: Add Genesis Reader Helper

**Files:**
- Modify: `crates/dugite-cli/src/commands/query.rs` (add helper after stake parser)

Read the shelley genesis JSON file to extract `activeSlotsCoeff`, `epochLength`, `systemStart`, and `slotLength`.

- [ ] **Step 1: Add ShelleyGenesisParams struct and reader**

```rust
/// Parameters extracted from the Shelley genesis file needed for leadership schedule.
struct ShelleyGenesisParams {
    /// Active slots coefficient (e.g. 0.05)
    active_slots_coeff: f64,
    /// Number of slots per epoch (e.g. 86400 for preview)
    epoch_length: u64,
    /// Slot duration in seconds (e.g. 1)
    slot_length: u64,
    /// System start as Unix timestamp (seconds since epoch)
    system_start_unix: u64,
}

/// Read the shelley genesis file and extract timing parameters.
fn read_shelley_genesis(path: &std::path::Path) -> Result<ShelleyGenesisParams> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Cannot read genesis file '{}': {e}", path.display()))?;
    let genesis: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| anyhow::anyhow!("Invalid genesis JSON: {e}"))?;

    let active_slots_coeff = genesis["activeSlotsCoeff"]
        .as_f64()
        .ok_or_else(|| anyhow::anyhow!("Missing activeSlotsCoeff in genesis"))?;

    let epoch_length = genesis["epochLength"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("Missing epochLength in genesis"))?;

    let slot_length = genesis["slotLength"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("Missing slotLength in genesis"))?;

    let system_start_str = genesis["systemStart"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing systemStart in genesis"))?;

    let system_start_unix = parse_iso8601_to_unix(system_start_str)
        .ok_or_else(|| anyhow::anyhow!("Cannot parse systemStart: {system_start_str}"))?;

    Ok(ShelleyGenesisParams {
        active_slots_coeff,
        epoch_length,
        slot_length,
        system_start_unix,
    })
}
```

- [ ] **Step 2: Commit**

No separate test needed — this is a thin JSON reader. It will be tested end-to-end.

```bash
git add crates/dugite-cli/src/commands/query.rs
git commit -m "feat(cli): add shelley genesis reader for leadership-schedule timing"
```

---

### Task 5: Add Pool ID Resolution from Cold Key

**Files:**
- Modify: `crates/dugite-cli/src/commands/query.rs` (add helper)

When `--cold-verification-key-file` is provided instead of `--stake-pool-id`, derive the pool ID (Blake2b-224 of the verification key bytes).

- [ ] **Step 1: Add cold key to pool ID helper**

```rust
/// Derive pool ID (hex) from a cold verification key file.
///
/// The key file is a Cardano text envelope with `cborHex` containing a CBOR-wrapped
/// Ed25519 public key (32 bytes). The pool ID is Blake2b-224 of the raw key bytes.
fn pool_id_from_cold_vkey(path: &std::path::Path) -> Result<String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Cannot read cold key file '{}': {e}", path.display()))?;
    let env: serde_json::Value = serde_json::from_str(&content)?;
    let cbor_hex = env["cborHex"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing cborHex in cold verification key file"))?;
    let cbor_bytes = hex::decode(cbor_hex)?;

    // Strip CBOR wrapper (5820 prefix for 32-byte bytestring)
    let key_bytes = if cbor_bytes.len() > 2 && cbor_bytes[0] == 0x58 && cbor_bytes[1] == 0x20 {
        &cbor_bytes[2..]
    } else if cbor_bytes.len() > 1 && (cbor_bytes[0] & 0xe0) == 0x40 {
        &cbor_bytes[1..]
    } else {
        &cbor_bytes
    };

    if key_bytes.len() != 32 {
        anyhow::bail!(
            "Cold verification key must be 32 bytes, got {}",
            key_bytes.len()
        );
    }

    // Pool ID = Blake2b-224(vkey)
    use blake2::digest::{consts::U28, Digest};
    type Blake2b224 = blake2::Blake2b<U28>;
    let hash = Blake2b224::digest(key_bytes);
    Ok(hex::encode(hash))
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/dugite-cli/src/commands/query.rs
git commit -m "feat(cli): add cold-key to pool-id derivation for leadership-schedule"
```

---

### Task 6: Implement the Full Leadership Schedule Handler

**Files:**
- Modify: `crates/dugite-cli/src/commands/query.rs:1877+` (replace the stub handler)

- [ ] **Step 1: Add slot-to-UTC helper**

Add near the other helpers:

```rust
/// Convert a slot number to a UTC timestamp string ("YYYY-MM-DDThh:mm:ssZ").
///
/// Uses simple arithmetic: time = system_start + slot * slot_length.
/// This is valid for all Shelley+ eras (slot_length is constant at 1s).
fn slot_to_utc(slot: u64, system_start_unix: u64, slot_length: u64) -> String {
    let unix_secs = system_start_unix + slot * slot_length;
    // Convert Unix seconds to UTC date-time string
    let secs_per_day = 86400u64;
    let days = unix_secs / secs_per_day;
    let day_secs = unix_secs % secs_per_day;
    let hours = day_secs / 3600;
    let minutes = (day_secs % 3600) / 60;
    let seconds = day_secs % 60;

    // Days since Unix epoch to Y-M-D (matching our existing parse_iso8601_to_unix logic)
    let mut y = 1970i64;
    let mut remaining = days;
    loop {
        let year_days = if is_leap_year(y as u64) { 366 } else { 365 };
        if remaining < year_days {
            break;
        }
        remaining -= year_days;
        y += 1;
    }
    let leap = is_leap_year(y as u64);
    let month_days: [u64; 12] = if leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut m = 0usize;
    for (i, &md) in month_days.iter().enumerate() {
        if remaining < md {
            m = i;
            break;
        }
        remaining -= md;
    }
    let d = remaining + 1;

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m + 1,
        d,
        hours,
        minutes,
        seconds
    )
}

fn is_leap_year(y: u64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}
```

- [ ] **Step 2: Write the full handler**

Replace the stub match arm for `QuerySubcommand::LeadershipSchedule`:

```rust
            QuerySubcommand::LeadershipSchedule {
                socket_path,
                mainnet,
                testnet_magic,
                genesis,
                stake_pool_id,
                cold_verification_key_file,
                vrf_signing_key_file,
                current,
                next,
                output_json: _,
                output_text,
                out_file,
            } => {
                // Resolve network magic
                let testnet_magic = if mainnet {
                    Some(764824073)
                } else {
                    testnet_magic
                };

                // Resolve socket path from env if default
                let socket_path = if socket_path == std::path::Path::new("node.sock") {
                    if let Ok(env_sock) = std::env::var("CARDANO_NODE_SOCKET_PATH") {
                        PathBuf::from(env_sock)
                    } else {
                        socket_path
                    }
                } else {
                    socket_path
                };

                // Determine epoch choice (default to --current)
                let use_next = next && !current;

                // Resolve pool ID
                let pool_id_hex = if let Some(ref id) = stake_pool_id {
                    id.to_lowercase()
                } else if let Some(ref path) = cold_verification_key_file {
                    pool_id_from_cold_vkey(path)?
                } else {
                    anyhow::bail!(
                        "Either --stake-pool-id or --cold-verification-key-file is required"
                    );
                };

                // Load VRF signing key
                let vrf_content = std::fs::read_to_string(&vrf_signing_key_file)?;
                let vrf_env: serde_json::Value = serde_json::from_str(&vrf_content)?;
                let vrf_cbor_hex = vrf_env["cborHex"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing cborHex in VRF skey file"))?;
                let vrf_cbor = hex::decode(vrf_cbor_hex)?;
                let vrf_key_bytes = if vrf_cbor.len() > 2 && vrf_cbor[0] == 0x58 {
                    &vrf_cbor[2..]
                } else if vrf_cbor.len() > 1 && (vrf_cbor[0] & 0xe0) == 0x40 {
                    &vrf_cbor[1..]
                } else {
                    &vrf_cbor
                };
                let vrf_seed = match vrf_key_bytes.len() {
                    32 => vrf_key_bytes,
                    64 => &vrf_key_bytes[..32],
                    n => anyhow::bail!("VRF secret key must be 32 or 64 bytes, got {n}"),
                };
                let mut vrf_skey = [0u8; 32];
                vrf_skey.copy_from_slice(vrf_seed);

                // Read shelley genesis for timing params
                let gp = read_shelley_genesis(&genesis)?;

                // Connect to node and query state
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                // Query tip for current epoch info
                let tip = client
                    .query_tip()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query tip: {e}"))?;
                let epoch_no = client
                    .query_epoch()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query epoch: {e}"))?;

                // Compute epoch start slot from tip
                let slot_in_epoch = tip.slot % gp.epoch_length;
                let current_epoch_start = tip.slot - slot_in_epoch;
                let epoch_start_slot = if use_next {
                    current_epoch_start + gp.epoch_length
                } else {
                    current_epoch_start
                };

                // Query protocol state for nonces
                let proto_raw = client
                    .query_protocol_state()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query protocol state: {e}"))?;
                let nonces = parse_protocol_state_nonces(&proto_raw)?;

                let epoch_nonce_bytes = if use_next {
                    nonces.candidate_nonce
                } else {
                    nonces.epoch_nonce
                };
                let epoch_nonce =
                    dugite_primitives::hash::Hash32::from(epoch_nonce_bytes);

                // Query stake snapshot
                let stake_raw = client
                    .query_stake_snapshot()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query stake snapshot: {e}"))?;

                release_and_done(&mut client).await;

                let stake = parse_stake_for_pool(&stake_raw, &pool_id_hex, use_next)?;

                // Convert active_slots_coeff to exact rational
                let (f_num, f_den) = f64_to_rational_approx(gp.active_slots_coeff);

                // Compute leadership schedule
                let schedule = dugite_consensus::compute_leader_schedule(
                    &vrf_skey,
                    &epoch_nonce,
                    epoch_start_slot,
                    gp.epoch_length,
                    stake.pool_stake,
                    stake.total_active_stake,
                    f_num,
                    f_den,
                );

                // Format output matching cardano-cli JSON:
                // [{"slotNumber": N, "slotTime": "YYYY-MM-DDThh:mm:ssZ"}, ...]
                let json_entries: Vec<serde_json::Value> = schedule
                    .iter()
                    .map(|s| {
                        let time = slot_to_utc(s.slot.0, gp.system_start_unix, gp.slot_length);
                        serde_json::json!({
                            "slotNumber": s.slot.0,
                            "slotTime": time,
                        })
                    })
                    .collect();

                let output = if output_text {
                    // Text format matching cardano-cli --output-text
                    let mut lines = Vec::new();
                    lines.push(format!(
                        "{:<15} {}",
                        "SlotNo", "UTC Time"
                    ));
                    lines.push("-".repeat(50));
                    for entry in &json_entries {
                        lines.push(format!(
                            "{:<15} {}",
                            entry["slotNumber"],
                            entry["slotTime"].as_str().unwrap_or("")
                        ));
                    }
                    lines.join("\n")
                } else {
                    serde_json::to_string_pretty(&json_entries)?
                };

                if let Some(ref path) = out_file {
                    std::fs::write(path, &output)?;
                    eprintln!("Leadership schedule written to: {}", path.display());
                } else {
                    println!("{output}");
                }

                Ok(())
            }
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p dugite-cli`
Expected: compiles with no errors

- [ ] **Step 4: Write a test for slot_to_utc**

```rust
    #[test]
    fn test_slot_to_utc() {
        // Preview systemStart = 2022-10-25T00:00:00Z = Unix 1666656000
        // slot_length = 1
        // Slot 0 = 2022-10-25T00:00:00Z
        assert_eq!(slot_to_utc(0, 1666656000, 1), "2022-10-25T00:00:00Z");
        // Slot 86400 = 2022-10-26T00:00:00Z (one day later)
        assert_eq!(slot_to_utc(86400, 1666656000, 1), "2022-10-26T00:00:00Z");
        // Slot 3600 = 2022-10-25T01:00:00Z
        assert_eq!(slot_to_utc(3600, 1666656000, 1), "2022-10-25T01:00:00Z");
    }
```

- [ ] **Step 5: Run all tests**

Run: `cargo nextest run -p dugite-cli`
Expected: all tests pass

- [ ] **Step 6: Commit**

```bash
git add crates/dugite-cli/src/commands/query.rs
git commit -m "feat(cli): implement leadership-schedule with node queries matching cardano-cli"
```

---

### Task 7: End-to-End Verification

**Files:** None (testing only)

- [ ] **Step 1: Build release binary**

Run: `cargo build --release -p dugite-cli`

- [ ] **Step 2: Start the node**

```bash
rm -rf db-preview/utxo-store
./target/release/dugite-node run \
  --config config/preview-config.json \
  --topology config/preview-topology.json \
  --database-path ./db-preview \
  --socket-path ./node.sock \
  --host-addr 0.0.0.0 --port 3001
```

Wait for it to sync to tip.

- [ ] **Step 3: Run dugite-cli leadership schedule**

```bash
CARDANO_NODE_SOCKET_PATH=./node.sock ./target/release/dugite-cli query leadership-schedule \
  --testnet-magic 2 \
  --genesis config/preview-shelley-genesis.json \
  --stake-pool-id 6954ec11cf7097a693721104139b96c54e7f3e2a8f9e7577630f7856 \
  --vrf-signing-key-file keys/vrf.skey \
  --current
```

- [ ] **Step 4: Run cardano-cli leadership schedule**

```bash
CARDANO_NODE_SOCKET_PATH=./node.sock cardano-cli query leadership-schedule \
  --testnet-magic 2 \
  --genesis config/preview-shelley-genesis.json \
  --stake-pool-id 6954ec11cf7097a693721104139b96c54e7f3e2a8f9e7577630f7856 \
  --vrf-signing-key-file keys/vrf.skey \
  --current \
  --output-json
```

- [ ] **Step 5: Cross-reference results**

Compare the JSON output from both commands. The `slotNumber` values must match exactly. The `slotTime` values must match (same UTC timestamps). If there is any discrepancy, cardano-cli is authoritative — investigate and fix.

- [ ] **Step 6: Test --cold-verification-key-file path**

```bash
CARDANO_NODE_SOCKET_PATH=./node.sock ./target/release/dugite-cli query leadership-schedule \
  --testnet-magic 2 \
  --genesis config/preview-shelley-genesis.json \
  --cold-verification-key-file keys/cold.vkey \
  --vrf-signing-key-file keys/vrf.skey \
  --current
```

Expected: identical output to the --stake-pool-id version.

- [ ] **Step 7: Final commit with any fixes**

```bash
git add -A
git commit -m "fix(cli): leadership-schedule alignment fixes from cross-validation"
```
