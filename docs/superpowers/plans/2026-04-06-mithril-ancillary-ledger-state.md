# Mithril Ancillary Ledger State Import — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** After Mithril snapshot import, restore the correct ledger state from the ancillary archive's Haskell ExtLedgerState snapshot instead of constructing a fresh state from genesis defaults.

**Architecture:** New `haskell_snapshot` module in `dugite-serialization` decodes the CBOR state file. New `mempack` module decodes the MemPack UTxO table. Mithril import upgraded to V2 API with ancillary download + Ed25519 verification. Node startup populates `LedgerState` from decoded snapshot, replays only the gap to immutable tip.

**Tech Stack:** Manual CBOR decoding (existing pattern in `dugite-serialization/src/cbor.rs`), `ed25519-dalek` for manifest verification, `sha2` for file digests, `zstd`/`tar` for archive handling.

**Spec:** `docs/superpowers/specs/2026-04-06-mithril-ancillary-ledger-state-design.md`

---

## File Structure

### New Files
- `crates/dugite-serialization/src/haskell_snapshot/mod.rs` — Top-level decoder: state file → `HaskellLedgerState`
- `crates/dugite-serialization/src/haskell_snapshot/types.rs` — Intermediate types for decoded Haskell state
- `crates/dugite-serialization/src/haskell_snapshot/cbor_utils.rs` — CBOR primitives for snapshot decoding (array, map, nonce, rational, WithOrigin, etc.)
- `crates/dugite-serialization/src/haskell_snapshot/pparams.rs` — PParams array(31) decoder
- `crates/dugite-serialization/src/haskell_snapshot/certstate.rs` — CertState (VState, PState, DState) decoder
- `crates/dugite-serialization/src/haskell_snapshot/praos.rs` — PraosState (nonces, opcert counters) decoder
- `crates/dugite-serialization/src/haskell_snapshot/snapshots.rs` — SnapShots (mark/set/go) decoder
- `crates/dugite-serialization/src/haskell_snapshot/govstate.rs` — ConwayGovState decoder
- `crates/dugite-serialization/src/mempack/mod.rs` — MemPack TxIn/TxOut decoder
- `crates/dugite-serialization/src/mempack/txout.rs` — TxOut variant decoders (tags 0-5)
- `crates/dugite-serialization/src/mempack/compact.rs` — VarLen, CompactAddr, CompactValue decoders
- `crates/dugite-node/src/mithril/ancillary.rs` — Ancillary archive download + verification

### Modified Files
- `crates/dugite-serialization/src/lib.rs` — Add `pub mod haskell_snapshot; pub mod mempack;`
- `crates/dugite-serialization/Cargo.toml` — Add `ed25519-dalek`, `sha2` deps (if not already present)
- `crates/dugite-node/src/mithril.rs` — Integrate ancillary download into import flow
- `crates/dugite-node/src/node/mod.rs` — Load Haskell snapshot after import, remove heuristics (lines 492-541)
- `crates/dugite-ledger/src/state/mod.rs` — Add `from_haskell_snapshot()` method on `LedgerState`

### Test Files
- `crates/dugite-serialization/src/haskell_snapshot/tests.rs` — Golden tests with real ancillary data
- `crates/dugite-serialization/src/mempack/tests.rs` — MemPack decoding tests
- `crates/dugite-node/tests/ancillary_import.rs` — Integration test for full import flow

---

## Task 1: CBOR Utilities for Haskell Snapshot Decoding

**Files:**
- Create: `crates/dugite-serialization/src/haskell_snapshot/cbor_utils.rs`
- Create: `crates/dugite-serialization/src/haskell_snapshot/mod.rs`
- Create: `crates/dugite-serialization/src/haskell_snapshot/types.rs`
- Modify: `crates/dugite-serialization/src/lib.rs`

These are the low-level CBOR primitives needed by all subsequent tasks. The existing `cbor.rs` has `decode_hash32` and `encode_uint` — we need additional decoders for arrays, maps, rationals, nonces, WithOrigin, StrictMaybe, and credential types.

- [ ] **Step 1: Create the module structure**

Add to `crates/dugite-serialization/src/lib.rs`:
```rust
pub mod haskell_snapshot;
```

Create `crates/dugite-serialization/src/haskell_snapshot/mod.rs`:
```rust
pub mod cbor_utils;
pub mod types;

pub use types::*;
```

Create `crates/dugite-serialization/src/haskell_snapshot/types.rs` with intermediate types:
```rust
//! Intermediate types representing the decoded Haskell ExtLedgerState.
//! These mirror the Haskell CBOR structure and are converted to dugite's
//! native LedgerState in a separate step.

use dugite_primitives::hash::{Hash28, Hash32};
use dugite_primitives::protocol_params::ProtocolParameters;
use dugite_primitives::time::{EpochNo, SlotNo};
use std::collections::HashMap;

/// Top-level decoded Haskell ledger snapshot.
#[derive(Debug)]
pub struct HaskellLedgerState {
    pub tip_slot: SlotNo,
    pub tip_block_no: u64,
    pub tip_hash: Hash32,
    pub epoch: EpochNo,
    pub new_epoch_state: HaskellNewEpochState,
    pub praos_state: HaskellPraosState,
}

/// NewEpochState fields extracted from the CBOR.
#[derive(Debug)]
pub struct HaskellNewEpochState {
    pub epoch: EpochNo,
    pub blocks_made_prev: HashMap<Hash28, u64>,
    pub blocks_made_cur: HashMap<Hash28, u64>,
    pub treasury: u64,
    pub reserves: u64,
    pub cur_pparams: ProtocolParameters,
    pub prev_pparams: ProtocolParameters,
    pub deposited: u64,
    pub fees: u64,
    pub donation: u64,
    pub cert_state: HaskellCertState,
    pub snapshots: HaskellSnapShots,
    pub pool_distr: HashMap<Hash28, HaskellPoolDistrEntry>,
    pub pool_distr_total_stake: u64,
    pub gov_state: HaskellGovState,
    /// Raw CBOR bytes for fields we decode but don't fully parse yet
    /// (e.g., NonMyopic, PulsingRewUpdate, InstantStake, DRepPulsingState).
    pub instant_stake: HashMap<(u8, Hash28), u64>,
}

/// PraosState from the HeaderState telescope.
#[derive(Debug)]
pub struct HaskellPraosState {
    pub last_slot: Option<SlotNo>,
    pub opcert_counters: HashMap<Hash28, u64>,
    pub evolving_nonce: Hash32,
    pub candidate_nonce: Hash32,
    pub epoch_nonce: Hash32,
    pub lab_nonce: Hash32,
    pub last_epoch_block_nonce: Hash32,
}

/// Individual pool stake entry from PoolDistr.
#[derive(Debug)]
pub struct HaskellPoolDistrEntry {
    pub stake_ratio_num: u64,
    pub stake_ratio_den: u64,
    pub stake_coin: u64,
    pub vrf_hash: Hash32,
}

/// Decoded CertState (VState + PState + DState).
#[derive(Debug)]
pub struct HaskellCertState {
    pub vstate: HaskellVState,
    pub pstate: HaskellPState,
    pub dstate: HaskellDState,
}

/// VState: DRep registrations + committee state.
#[derive(Debug)]
pub struct HaskellVState {
    /// DRep credential → (expiry_epoch, deposit, anchor_url, anchor_hash)
    pub dreps: HashMap<(u8, Hash28), HaskellDRepState>,
    /// Committee cold credential → authorization (hot credential or resigned)
    pub committee_state: HashMap<(u8, Hash28), HaskellCommitteeAuth>,
    pub dormant_epochs: u64,
}

#[derive(Debug)]
pub struct HaskellDRepState {
    pub expiry: EpochNo,
    pub deposit: u64,
    pub anchor: Option<(String, Hash32)>,
    // delegators set skipped for now — reconstructed from DState accounts
}

#[derive(Debug)]
pub enum HaskellCommitteeAuth {
    Hot(u8, Hash28),        // tag 0: CommitteeHotCredential
    Resigned(Option<(String, Hash32)>),  // tag 1: CommitteeMemberResigned
}

/// PState: pool registrations.
#[derive(Debug)]
pub struct HaskellPState {
    pub vrf_key_hashes: HashMap<Hash32, u64>,  // VRF hash → refcount
    pub stake_pools: HashMap<Hash28, HaskellStakePoolState>,
    pub future_pool_params: HashMap<Hash28, HaskellPoolParams>,
    pub retirements: HashMap<Hash28, EpochNo>,
}

/// StakePoolState (9 or 10 fields from PState).
#[derive(Debug)]
pub struct HaskellStakePoolState {
    pub vrf_hash: Hash32,
    pub pledge: u64,
    pub cost: u64,
    pub margin_num: u64,
    pub margin_den: u64,
    pub reward_account: Vec<u8>,  // raw 29-byte reward address
    pub owners: Vec<Hash28>,
    pub relays: Vec<HaskellRelay>,
    pub metadata: Option<(String, Hash32)>,
    pub deposit: u64,
    // delegators (field 10) skipped — only in newer nodes
}

/// PoolParams (9 fields from future_pool_params).
pub type HaskellPoolParams = HaskellStakePoolState;

#[derive(Debug)]
pub enum HaskellRelay {
    SingleHostAddr(Option<u16>, Option<[u8; 4]>, Option<[u8; 16]>),
    SingleHostName(Option<u16>, String),
    MultiHostName(String),
}

/// DState: accounts + genesis delegates.
#[derive(Debug)]
pub struct HaskellDState {
    /// Credential → ConwayAccountState (balance, deposit, pool_delegation, drep_delegation)
    pub accounts: HashMap<(u8, Hash28), HaskellAccountState>,
    pub genesis_delegates: HashMap<Hash28, (Hash28, Hash32)>,  // genesis key → (delegate, vrf)
    pub i_rewards_reserves: HashMap<(u8, Hash28), u64>,
    pub i_rewards_treasury: HashMap<(u8, Hash28), u64>,
    pub delta_reserves: i64,
    pub delta_treasury: i64,
}

/// ConwayAccountState = array(4) [balance, deposit, pool?, drep?]
#[derive(Debug)]
pub struct HaskellAccountState {
    pub balance: u64,
    pub deposit: u64,
    pub pool_delegation: Option<Hash28>,
    pub drep_delegation: Option<HaskellDRep>,
}

/// DRep delegation target.
#[derive(Debug)]
pub enum HaskellDRep {
    KeyHash(Hash28),
    ScriptHash(Hash28),
    AlwaysAbstain,
    AlwaysNoConfidence,
}

/// SnapShots (mark/set/go) decoded from the EpochState.
#[derive(Debug)]
pub struct HaskellSnapShots {
    pub mark: HaskellSnapShot,
    pub set: HaskellSnapShot,
    pub go: HaskellSnapShot,
    pub fee: u64,
}

/// Individual SnapShot. Handles both old (array 3) and new (array 2) formats.
#[derive(Debug)]
pub struct HaskellSnapShot {
    /// Credential → staked lovelace.
    pub stake: HashMap<(u8, Hash28), u64>,
    /// Credential → pool hash.
    pub delegations: HashMap<(u8, Hash28), Hash28>,
    /// Pool hash → pool snapshot params.
    pub pool_params: HashMap<Hash28, HaskellSnapShotPool>,
}

/// Pool data within a snapshot.
#[derive(Debug)]
pub struct HaskellSnapShotPool {
    pub vrf_hash: Hash32,
    pub pledge: u64,
    pub cost: u64,
    pub margin_num: u64,
    pub margin_den: u64,
    pub reward_account: Vec<u8>,
    pub owners: Vec<Hash28>,
    pub relays: Vec<HaskellRelay>,
    pub metadata: Option<(String, Hash32)>,
}

/// Governance state (simplified — captures fields needed by dugite).
#[derive(Debug)]
pub struct HaskellGovState {
    /// Raw proposals CBOR (complex structure, decoded on-demand).
    pub proposals_raw: Vec<u8>,
    /// Committee (if present).
    pub committee_raw: Option<Vec<u8>>,
    /// Constitution anchor + optional script hash.
    pub constitution: Option<HaskellConstitution>,
    /// DRep pulsing state raw CBOR.
    pub drep_pulsing_raw: Vec<u8>,
    /// FuturePParams variant tag (0=none, 1=definite, 2=potential).
    pub future_pparams_tag: u8,
    pub future_pparams: Option<ProtocolParameters>,
}

#[derive(Debug)]
pub struct HaskellConstitution {
    pub anchor_url: String,
    pub anchor_hash: Hash32,
    pub script_hash: Option<Hash28>,
}
```

- [ ] **Step 2: Implement CBOR utility functions**

Create `crates/dugite-serialization/src/haskell_snapshot/cbor_utils.rs`:
```rust
//! Low-level CBOR decoding utilities for Haskell ledger state snapshots.
//!
//! These functions consume CBOR bytes and return (decoded_value, bytes_consumed).
//! They follow the existing pattern in `crate::cbor` but add array/map/rational
//! decoders needed for the Haskell ExtLedgerState format.

use crate::error::SerializationError;
use dugite_primitives::hash::{Hash28, Hash32};

/// Decode a CBOR unsigned integer (major type 0).
pub fn decode_uint(data: &[u8]) -> Result<(u64, usize), SerializationError> {
    if data.is_empty() {
        return Err(SerializationError::CborDecode("empty input".into()));
    }
    let major = data[0] >> 5;
    let info = data[0] & 0x1f;
    if major != 0 {
        return Err(SerializationError::CborDecode(format!(
            "expected uint (major 0), got major {major} at byte {:#04x}",
            data[0]
        )));
    }
    decode_uint_info(data, info)
}

/// Decode a CBOR integer that could be unsigned (major 0) or negative (major 1).
pub fn decode_int(data: &[u8]) -> Result<(i64, usize), SerializationError> {
    if data.is_empty() {
        return Err(SerializationError::CborDecode("empty input".into()));
    }
    let major = data[0] >> 5;
    let info = data[0] & 0x1f;
    match major {
        0 => {
            let (v, n) = decode_uint_info(data, info)?;
            Ok((v as i64, n))
        }
        1 => {
            let (v, n) = decode_uint_info(data, info)?;
            Ok((-1 - v as i64, n))
        }
        _ => Err(SerializationError::CborDecode(format!(
            "expected int, got major {major}"
        ))),
    }
}

fn decode_uint_info(data: &[u8], info: u8) -> Result<(u64, usize), SerializationError> {
    match info {
        0..=23 => Ok((info as u64, 1)),
        24 => {
            if data.len() < 2 { return Err(eof()); }
            Ok((data[1] as u64, 2))
        }
        25 => {
            if data.len() < 3 { return Err(eof()); }
            Ok((u16::from_be_bytes([data[1], data[2]]) as u64, 3))
        }
        26 => {
            if data.len() < 5 { return Err(eof()); }
            Ok((u32::from_be_bytes([data[1], data[2], data[3], data[4]]) as u64, 5))
        }
        27 => {
            if data.len() < 9 { return Err(eof()); }
            Ok((u64::from_be_bytes([
                data[1], data[2], data[3], data[4],
                data[5], data[6], data[7], data[8],
            ]), 9))
        }
        _ => Err(SerializationError::CborDecode(format!("invalid info {info}"))),
    }
}

/// Decode a CBOR bigint (tag 2 = positive, tag 3 = negative, wrapping a bytestring).
/// Falls back to regular uint if no tag present.
pub fn decode_bigint_or_uint(data: &[u8]) -> Result<(u64, usize), SerializationError> {
    if data.is_empty() {
        return Err(eof());
    }
    let major = data[0] >> 5;
    if major == 0 {
        return decode_uint(data);
    }
    // Tag 2 (positive bignum): 0xc2 + bytestring
    if data[0] == 0xc2 {
        let (bytes, n) = decode_bytes(&data[1..])?;
        let mut val = 0u64;
        for &b in bytes {
            val = val.checked_shl(8).unwrap_or(u64::MAX) | b as u64;
        }
        return Ok((val, 1 + n));
    }
    Err(SerializationError::CborDecode(format!(
        "expected uint or bigint, got {:#04x}", data[0]
    )))
}

/// Decode CBOR array header, returning (length, bytes_consumed).
pub fn decode_array_len(data: &[u8]) -> Result<(usize, usize), SerializationError> {
    if data.is_empty() {
        return Err(eof());
    }
    let major = data[0] >> 5;
    let info = data[0] & 0x1f;
    if major != 4 {
        return Err(SerializationError::CborDecode(format!(
            "expected array (major 4), got major {major} at byte {:#04x}",
            data[0]
        )));
    }
    let (len, consumed) = decode_uint_info(data, info)?;
    Ok((len as usize, consumed))
}

/// Decode CBOR map header, returning (length, bytes_consumed).
/// Returns None for length if indefinite-length map.
pub fn decode_map_len(data: &[u8]) -> Result<(Option<usize>, usize), SerializationError> {
    if data.is_empty() {
        return Err(eof());
    }
    let major = data[0] >> 5;
    let info = data[0] & 0x1f;
    if major != 5 {
        return Err(SerializationError::CborDecode(format!(
            "expected map (major 5), got major {major} at byte {:#04x}",
            data[0]
        )));
    }
    if info == 31 {
        // Indefinite-length map
        return Ok((None, 1));
    }
    let (len, consumed) = decode_uint_info(data, info)?;
    Ok((Some(len as usize), consumed))
}

/// Decode CBOR byte string, returning (&[u8], bytes_consumed).
pub fn decode_bytes<'a>(data: &'a [u8]) -> Result<(&'a [u8], usize), SerializationError> {
    if data.is_empty() {
        return Err(eof());
    }
    let major = data[0] >> 5;
    let info = data[0] & 0x1f;
    if major != 2 {
        return Err(SerializationError::CborDecode(format!(
            "expected bytes (major 2), got major {major} at byte {:#04x}",
            data[0]
        )));
    }
    let (len, hdr) = decode_uint_info(data, info)?;
    let len = len as usize;
    if data.len() < hdr + len {
        return Err(eof());
    }
    Ok((&data[hdr..hdr + len], hdr + len))
}

/// Decode CBOR text string, returning (&str, bytes_consumed).
pub fn decode_text<'a>(data: &'a [u8]) -> Result<(&'a str, usize), SerializationError> {
    if data.is_empty() {
        return Err(eof());
    }
    let major = data[0] >> 5;
    let info = data[0] & 0x1f;
    if major != 3 {
        return Err(SerializationError::CborDecode(format!(
            "expected text (major 3), got major {major}",
            data[0]
        )));
    }
    let (len, hdr) = decode_uint_info(data, info)?;
    let len = len as usize;
    if data.len() < hdr + len {
        return Err(eof());
    }
    let s = std::str::from_utf8(&data[hdr..hdr + len])
        .map_err(|e| SerializationError::CborDecode(format!("invalid utf8: {e}")))?;
    Ok((s, hdr + len))
}

/// Decode a Hash28 (28-byte CBOR bytestring).
pub fn decode_hash28(data: &[u8]) -> Result<(Hash28, usize), SerializationError> {
    let (bytes, n) = decode_bytes(data)?;
    if bytes.len() != 28 {
        return Err(SerializationError::InvalidLength {
            expected: 28,
            got: bytes.len(),
        });
    }
    Ok((Hash28::from_bytes(bytes.try_into().unwrap()), n))
}

/// Decode a Hash32 (32-byte CBOR bytestring).
pub fn decode_hash32(data: &[u8]) -> Result<(Hash32, usize), SerializationError> {
    let (bytes, n) = decode_bytes(data)?;
    if bytes.len() != 32 {
        return Err(SerializationError::InvalidLength {
            expected: 32,
            got: bytes.len(),
        });
    }
    Ok((Hash32::from_bytes(bytes.try_into().unwrap()), n))
}

/// Decode a Nonce: [0] = NeutralNonce → zero hash, [1, bytes(32)] = Nonce.
pub fn decode_nonce(data: &[u8]) -> Result<(Hash32, usize), SerializationError> {
    let (arr_len, mut off) = decode_array_len(data)?;
    let (tag, n) = decode_uint(&data[off..])?;
    off += n;
    match (arr_len, tag) {
        (1, 0) => Ok((Hash32::zero(), off)),
        (2, 1) => {
            let (hash, n) = decode_hash32(&data[off..])?;
            off += n;
            Ok((hash, off))
        }
        _ => Err(SerializationError::CborDecode(format!(
            "invalid nonce: array({arr_len}), tag {tag}"
        ))),
    }
}

/// Decode a Credential: [0, bytes(28)] = KeyHash, [1, bytes(28)] = ScriptHash.
/// Returns (tag, hash28, bytes_consumed).
pub fn decode_credential(data: &[u8]) -> Result<((u8, Hash28), usize), SerializationError> {
    let (arr_len, mut off) = decode_array_len(data)?;
    if arr_len != 2 {
        return Err(SerializationError::InvalidLength { expected: 2, got: arr_len });
    }
    let (tag, n) = decode_uint(&data[off..])?;
    off += n;
    let (hash, n) = decode_hash28(&data[off..])?;
    off += n;
    Ok(((tag as u8, hash), off))
}

/// Decode WithOrigin<T>: [] = None (Origin), [v] = Some(v) (At).
/// Returns the raw bytes of v if present, plus total consumed.
pub fn decode_with_origin_len(data: &[u8]) -> Result<(Option<usize>, usize), SerializationError> {
    let (arr_len, off) = decode_array_len(data)?;
    match arr_len {
        0 => Ok((None, off)),
        1 => Ok((Some(1), off)),
        n => Err(SerializationError::CborDecode(format!(
            "WithOrigin: expected array(0) or array(1), got array({n})"
        ))),
    }
}

/// Decode a Rational (CBOR tag 30 [num, den] or plain [num, den]).
pub fn decode_rational(data: &[u8]) -> Result<((u64, u64), usize), SerializationError> {
    let mut off = 0;
    // Skip tag 30 if present (0xd8 0x1e)
    if data.len() >= 2 && data[0] == 0xd8 && data[1] == 0x1e {
        off += 2;
    }
    let (arr_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if arr_len != 2 {
        return Err(SerializationError::InvalidLength { expected: 2, got: arr_len });
    }
    let (num, n) = decode_bigint_or_uint(&data[off..])?;
    off += n;
    let (den, n) = decode_bigint_or_uint(&data[off..])?;
    off += n;
    Ok(((num, den), off))
}

/// Check if next byte is CBOR null (0xf6). If so, consume and return true.
pub fn decode_null(data: &[u8]) -> Result<(bool, usize), SerializationError> {
    if data.is_empty() {
        return Err(eof());
    }
    if data[0] == 0xf6 {
        Ok((true, 1))
    } else {
        Ok((false, 0))
    }
}

/// Skip over any CBOR value, returning the number of bytes consumed.
/// Used for fields we don't need to fully decode.
pub fn skip_cbor_value(data: &[u8]) -> Result<usize, SerializationError> {
    if data.is_empty() {
        return Err(eof());
    }
    let major = data[0] >> 5;
    let info = data[0] & 0x1f;
    match major {
        0 | 1 => {
            // Unsigned/negative integer
            let (_, n) = decode_uint_info(data, info)?;
            Ok(n)
        }
        2 | 3 => {
            // Byte/text string
            let (_, n) = decode_uint_info(data, info)?;
            let len = match info {
                0..=23 => info as usize,
                24 => data[1] as usize,
                25 => u16::from_be_bytes([data[1], data[2]]) as usize,
                26 => u32::from_be_bytes([data[1], data[2], data[3], data[4]]) as usize,
                27 => u64::from_be_bytes([
                    data[1], data[2], data[3], data[4],
                    data[5], data[6], data[7], data[8],
                ]) as usize,
                _ => return Err(SerializationError::CborDecode("invalid info".into())),
            };
            Ok(n + len)
        }
        4 => {
            // Array
            if info == 31 {
                // Indefinite-length array
                let mut off = 1;
                while data[off] != 0xff {
                    off += skip_cbor_value(&data[off..])?;
                }
                Ok(off + 1) // +1 for break byte
            } else {
                let (count, mut off) = decode_uint_info(data, info)?;
                for _ in 0..count {
                    off += skip_cbor_value(&data[off..])?;
                }
                Ok(off)
            }
        }
        5 => {
            // Map
            if info == 31 {
                let mut off = 1;
                while data[off] != 0xff {
                    off += skip_cbor_value(&data[off..])?; // key
                    off += skip_cbor_value(&data[off..])?; // value
                }
                Ok(off + 1)
            } else {
                let (count, mut off) = decode_uint_info(data, info)?;
                for _ in 0..count {
                    off += skip_cbor_value(&data[off..])?; // key
                    off += skip_cbor_value(&data[off..])?; // value
                }
                Ok(off)
            }
        }
        6 => {
            // Tag
            let (_, n) = decode_uint_info(data, info)?;
            let inner = skip_cbor_value(&data[n..])?;
            Ok(n + inner)
        }
        7 => {
            // Simple values + float
            match info {
                0..=23 => Ok(1),  // simple value (includes null=22, true=21, false=20)
                24 => Ok(2),
                25 => Ok(3),      // float16
                26 => Ok(5),      // float32
                27 => Ok(9),      // float64
                31 => Ok(1),      // break
                _ => Err(SerializationError::CborDecode("invalid simple".into())),
            }
        }
        _ => unreachable!(),
    }
}

fn eof() -> SerializationError {
    SerializationError::CborDecode("unexpected end of input".into())
}
```

- [ ] **Step 3: Write tests for CBOR utilities**

Add to `crates/dugite-serialization/src/haskell_snapshot/mod.rs`:
```rust
#[cfg(test)]
mod tests;
```

Create `crates/dugite-serialization/src/haskell_snapshot/tests.rs`:
```rust
use super::cbor_utils::*;
use dugite_primitives::hash::Hash32;

#[test]
fn test_decode_uint_small() {
    assert_eq!(decode_uint(&[0x00]).unwrap(), (0, 1));
    assert_eq!(decode_uint(&[0x17]).unwrap(), (23, 1));
    assert_eq!(decode_uint(&[0x18, 0x18]).unwrap(), (24, 2));
    assert_eq!(decode_uint(&[0x18, 0xff]).unwrap(), (255, 2));
}

#[test]
fn test_decode_uint_large() {
    assert_eq!(decode_uint(&[0x19, 0x01, 0x00]).unwrap(), (256, 3));
    assert_eq!(decode_uint(&[0x1a, 0x00, 0x01, 0x00, 0x00]).unwrap(), (65536, 5));
    assert_eq!(
        decode_uint(&[0x1b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2c]).unwrap(),
        (44, 9)
    );
}

#[test]
fn test_decode_array_len() {
    assert_eq!(decode_array_len(&[0x80]).unwrap(), (0, 1));
    assert_eq!(decode_array_len(&[0x82]).unwrap(), (2, 1));
    assert_eq!(decode_array_len(&[0x87]).unwrap(), (7, 1));
    assert_eq!(decode_array_len(&[0x98, 0x1f]).unwrap(), (31, 2));
}

#[test]
fn test_decode_nonce_neutral() {
    // [0] = NeutralNonce → zero hash
    let data = [0x81, 0x00];
    let (hash, consumed) = decode_nonce(&data).unwrap();
    assert_eq!(consumed, 2);
    assert_eq!(hash, Hash32::zero());
}

#[test]
fn test_decode_nonce_value() {
    // [1, bytes(32)] = Nonce with hash
    let mut data = vec![0x82, 0x01, 0x58, 0x20];
    data.extend_from_slice(&[0xab; 32]);
    let (hash, consumed) = decode_nonce(&data).unwrap();
    assert_eq!(consumed, 36);
    assert_eq!(hash.as_bytes(), &[0xab; 32]);
}

#[test]
fn test_decode_with_origin_absent() {
    // [] = Origin
    let data = [0x80];
    let (present, consumed) = decode_with_origin_len(&data).unwrap();
    assert_eq!(consumed, 1);
    assert!(present.is_none());
}

#[test]
fn test_decode_with_origin_present() {
    // [x] = At x
    let data = [0x81, 0x19, 0x04, 0x00]; // [1024]
    let (present, consumed) = decode_with_origin_len(&data).unwrap();
    assert_eq!(consumed, 1); // only array header consumed
    assert!(present.is_some());
}

#[test]
fn test_decode_rational() {
    // tag(30) [3, 10] = 3/10
    let data = [0xd8, 0x1e, 0x82, 0x03, 0x0a];
    let ((num, den), consumed) = decode_rational(&data).unwrap();
    assert_eq!(num, 3);
    assert_eq!(den, 10);
    assert_eq!(consumed, 5);
}

#[test]
fn test_decode_rational_no_tag() {
    // [577, 10000] without tag 30
    let data = [0x82, 0x19, 0x02, 0x41, 0x19, 0x27, 0x10];
    let ((num, den), consumed) = decode_rational(&data).unwrap();
    assert_eq!(num, 577);
    assert_eq!(den, 10000);
    assert_eq!(consumed, 7);
}

#[test]
fn test_decode_credential() {
    // [0, bytes(28)] = KeyHash credential
    let mut data = vec![0x82, 0x00, 0x58, 0x1c];
    data.extend_from_slice(&[0xaa; 28]);
    let ((tag, hash), consumed) = decode_credential(&data).unwrap();
    assert_eq!(tag, 0);
    assert_eq!(hash.as_bytes(), &[0xaa; 28]);
    assert_eq!(consumed, 32);
}

#[test]
fn test_skip_cbor_value() {
    // Skip a nested array(2) [uint, bytes(32)]
    let mut data = vec![0x82, 0x01, 0x58, 0x20];
    data.extend_from_slice(&[0x00; 32]);
    assert_eq!(skip_cbor_value(&data).unwrap(), 36);
}

#[test]
fn test_decode_null() {
    assert_eq!(decode_null(&[0xf6]).unwrap(), (true, 1));
    assert_eq!(decode_null(&[0x00]).unwrap(), (false, 0));
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p dugite-serialization -E 'test(haskell_snapshot)'`

Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-serialization/src/haskell_snapshot/ crates/dugite-serialization/src/lib.rs
git commit -m "feat(serialization): add CBOR utilities and types for Haskell snapshot decoding (#347)"
```

---

## Task 2: PParams Array(31) Decoder

**Files:**
- Create: `crates/dugite-serialization/src/haskell_snapshot/pparams.rs`
- Modify: `crates/dugite-serialization/src/haskell_snapshot/mod.rs`
- Modify: `crates/dugite-serialization/src/haskell_snapshot/tests.rs`

Decode the Conway PParams array(31) into dugite's `ProtocolParameters`. This is the most critical field — all 31 indices have been verified against Koios.

- [ ] **Step 1: Write a failing test using real ancillary data**

Download a test fixture from the real ancillary state file. Save the PParams CBOR bytes as a hex constant:

Add to `tests.rs`:
```rust
use dugite_primitives::protocol_params::ProtocolParameters;
use super::pparams::decode_pparams;

/// Real PParams from preview epoch 1259 ancillary snapshot.
/// Extracted via: python3 -c "import cbor2; obj=cbor2.loads(open('state','rb').read());
/// pp=obj[1][0][6][1][1][1][3][1][1][3][3]; print(cbor2.dumps(pp).hex())"
#[test]
fn test_decode_pparams_preview_epoch_1259() {
    // This test uses real CBOR bytes from a preview ancillary snapshot.
    // The expected values match Koios epoch_params for epoch 1259.
    let pp_cbor = include_bytes!("../../test_fixtures/preview_pparams_e1259.cbor");
    let (pp, _consumed) = decode_pparams(pp_cbor).unwrap();

    assert_eq!(pp.min_fee_a, 44);
    assert_eq!(pp.min_fee_b, 155381);
    assert_eq!(pp.max_block_body_size, 90112);
    assert_eq!(pp.max_tx_size, 16384);
    assert_eq!(pp.max_block_header_size, 1100);
    assert_eq!(pp.key_deposit.0, 2_000_000);
    assert_eq!(pp.pool_deposit.0, 500_000_000);
    assert_eq!(pp.e_max, 18);
    assert_eq!(pp.n_opt, 500);
    assert_eq!(pp.protocol_version_major, 10);
    assert_eq!(pp.protocol_version_minor, 0);
    assert_eq!(pp.min_pool_cost.0, 170_000_000);
    assert_eq!(pp.ada_per_utxo_byte.0, 4310);
    assert_eq!(pp.max_tx_ex_units.mem, 16_500_000);
    assert_eq!(pp.max_tx_ex_units.steps, 10_000_000_000);
    assert_eq!(pp.max_block_ex_units.mem, 72_000_000);
    assert_eq!(pp.max_block_ex_units.steps, 20_000_000_000);
    assert_eq!(pp.max_val_size, 5000);
    assert_eq!(pp.collateral_percentage, 150);
    assert_eq!(pp.max_collateral_inputs, 3);
    assert_eq!(pp.committee_min_size, 3);
    assert_eq!(pp.committee_max_term_length, 365);
    assert_eq!(pp.gov_action_lifetime, 30);
    assert_eq!(pp.gov_action_deposit.0, 100_000_000_000);
    assert_eq!(pp.drep_deposit.0, 500_000_000);
    assert_eq!(pp.drep_activity, 31);
}
```

- [ ] **Step 2: Extract the test fixture from the real ancillary archive**

```bash
mkdir -p crates/dugite-serialization/test_fixtures
python3 -c "
import cbor2
with open('/tmp/mithril-ancillary/ledger/108794365/state', 'rb') as f:
    obj = cbor2.loads(f.read())
pp = obj[1][0][6][1][1][1][3][1][1][3][3]
with open('crates/dugite-serialization/test_fixtures/preview_pparams_e1259.cbor', 'wb') as f:
    f.write(cbor2.dumps(pp))
"
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo nextest run -p dugite-serialization -E 'test(test_decode_pparams)'`

Expected: FAIL with "cannot find function `decode_pparams`"

- [ ] **Step 4: Implement decode_pparams**

Create `crates/dugite-serialization/src/haskell_snapshot/pparams.rs`:
```rust
//! Decode Conway PParams from CBOR array(31).
//!
//! Field order verified against cardano-ledger Conway/PParams.hs eraPParams list
//! and cross-validated with Koios epoch_params for preview epoch 1259.

use crate::error::SerializationError;
use crate::haskell_snapshot::cbor_utils::*;
use dugite_primitives::protocol_params::{
    CostModels, ExUnitPrices, ExUnits, Lovelace, ProtocolParameters, Rational,
};

/// Decode a Conway PParams from CBOR array(31).
pub fn decode_pparams(data: &[u8]) -> Result<(ProtocolParameters, usize), SerializationError> {
    let (arr_len, mut off) = decode_array_len(data)?;
    if arr_len != 31 {
        return Err(SerializationError::InvalidLength { expected: 31, got: arr_len });
    }

    // [0] txFeePerByte (minFeeA)
    let (min_fee_a, n) = decode_uint(&data[off..])?; off += n;
    // [1] txFeeFixed (minFeeB)
    let (min_fee_b, n) = decode_uint(&data[off..])?; off += n;
    // [2] maxBlockBodySize
    let (max_block_body_size, n) = decode_uint(&data[off..])?; off += n;
    // [3] maxTxSize
    let (max_tx_size, n) = decode_uint(&data[off..])?; off += n;
    // [4] maxBlockHeaderSize
    let (max_block_header_size, n) = decode_uint(&data[off..])?; off += n;
    // [5] keyDeposit
    let (key_deposit, n) = decode_uint(&data[off..])?; off += n;
    // [6] poolDeposit
    let (pool_deposit, n) = decode_uint(&data[off..])?; off += n;
    // [7] eMax (EpochInterval)
    let (e_max, n) = decode_uint(&data[off..])?; off += n;
    // [8] nOpt
    let (n_opt, n) = decode_uint(&data[off..])?; off += n;
    // [9] a0 (NonNegativeInterval = rational)
    let ((a0_num, a0_den), n) = decode_rational(&data[off..])?; off += n;
    // [10] rho (UnitInterval = rational)
    let ((rho_num, rho_den), n) = decode_rational(&data[off..])?; off += n;
    // [11] tau (UnitInterval = rational)
    let ((tau_num, tau_den), n) = decode_rational(&data[off..])?; off += n;
    // [12] protocolVersion = array(2) [major, minor]
    let (pv_len, n) = decode_array_len(&data[off..])?; off += n;
    if pv_len != 2 {
        return Err(SerializationError::InvalidLength { expected: 2, got: pv_len });
    }
    let (pv_major, n) = decode_uint(&data[off..])?; off += n;
    let (pv_minor, n) = decode_uint(&data[off..])?; off += n;
    // [13] minPoolCost
    let (min_pool_cost, n) = decode_uint(&data[off..])?; off += n;
    // [14] coinsPerUTxOByte
    let (ada_per_utxo_byte, n) = decode_uint(&data[off..])?; off += n;
    // [15] costModels = map(uint → array)
    let cost_models = decode_cost_models(&data[off..], &mut off)?;
    // [16] prices = array(2) [mem_rational, step_rational]
    let (prices_len, n) = decode_array_len(&data[off..])?; off += n;
    if prices_len != 2 {
        return Err(SerializationError::InvalidLength { expected: 2, got: prices_len });
    }
    let ((mem_price_num, mem_price_den), n) = decode_rational(&data[off..])?; off += n;
    let ((step_price_num, step_price_den), n) = decode_rational(&data[off..])?; off += n;
    // [17] maxTxExUnits = array(2) [mem, steps]
    let (_, n) = decode_array_len(&data[off..])?; off += n;
    let (tx_ex_mem, n) = decode_uint(&data[off..])?; off += n;
    let (tx_ex_steps, n) = decode_uint(&data[off..])?; off += n;
    // [18] maxBlockExUnits = array(2) [mem, steps]
    let (_, n) = decode_array_len(&data[off..])?; off += n;
    let (blk_ex_mem, n) = decode_uint(&data[off..])?; off += n;
    let (blk_ex_steps, n) = decode_uint(&data[off..])?; off += n;
    // [19] maxValSize
    let (max_val_size, n) = decode_uint(&data[off..])?; off += n;
    // [20] collateralPercentage
    let (collateral_pct, n) = decode_uint(&data[off..])?; off += n;
    // [21] maxCollateralInputs
    let (max_collateral_inputs, n) = decode_uint(&data[off..])?; off += n;
    // [22] poolVotingThresholds = array(5)
    let (pvt_len, n) = decode_array_len(&data[off..])?; off += n;
    if pvt_len != 5 {
        return Err(SerializationError::InvalidLength { expected: 5, got: pvt_len });
    }
    let ((pvt_motion_nc_num, pvt_motion_nc_den), n) = decode_rational(&data[off..])?; off += n;
    let ((pvt_committee_normal_num, pvt_committee_normal_den), n) = decode_rational(&data[off..])?; off += n;
    let ((pvt_committee_nc_num, pvt_committee_nc_den), n) = decode_rational(&data[off..])?; off += n;
    let ((pvt_hard_fork_num, pvt_hard_fork_den), n) = decode_rational(&data[off..])?; off += n;
    let ((pvt_pp_security_num, pvt_pp_security_den), n) = decode_rational(&data[off..])?; off += n;
    // [23] dRepVotingThresholds = array(10)
    let (dvt_len, n) = decode_array_len(&data[off..])?; off += n;
    if dvt_len != 10 {
        return Err(SerializationError::InvalidLength { expected: 10, got: dvt_len });
    }
    let ((dvt_pp_network_num, dvt_pp_network_den), n) = decode_rational(&data[off..])?; off += n;
    let ((dvt_pp_economic_num, dvt_pp_economic_den), n) = decode_rational(&data[off..])?; off += n;
    let ((dvt_pp_technical_num, dvt_pp_technical_den), n) = decode_rational(&data[off..])?; off += n;
    let ((dvt_pp_gov_num, dvt_pp_gov_den), n) = decode_rational(&data[off..])?; off += n;
    let ((dvt_treasury_num, dvt_treasury_den), n) = decode_rational(&data[off..])?; off += n;
    let ((dvt_no_confidence_num, dvt_no_confidence_den), n) = decode_rational(&data[off..])?; off += n;
    let ((dvt_committee_normal_num, dvt_committee_normal_den), n) = decode_rational(&data[off..])?; off += n;
    let ((dvt_committee_nc_num, dvt_committee_nc_den), n) = decode_rational(&data[off..])?; off += n;
    let ((dvt_constitution_num, dvt_constitution_den), n) = decode_rational(&data[off..])?; off += n;
    let ((dvt_hard_fork_num, dvt_hard_fork_den), n) = decode_rational(&data[off..])?; off += n;
    // [24] committeeMinSize
    let (committee_min_size, n) = decode_uint(&data[off..])?; off += n;
    // [25] committeeMaxTermLength
    let (committee_max_term_length, n) = decode_uint(&data[off..])?; off += n;
    // [26] govActionLifetime
    let (gov_action_lifetime, n) = decode_uint(&data[off..])?; off += n;
    // [27] govActionDeposit
    let (gov_action_deposit, n) = decode_uint(&data[off..])?; off += n;
    // [28] dRepDeposit
    let (drep_deposit, n) = decode_uint(&data[off..])?; off += n;
    // [29] dRepActivity
    let (drep_activity, n) = decode_uint(&data[off..])?; off += n;
    // [30] minFeeRefScriptCostPerByte (NonNegativeInterval = rational or uint)
    let (min_fee_ref_script, n) = decode_min_fee_ref_script(&data[off..])?; off += n;

    let pp = ProtocolParameters {
        min_fee_a,
        min_fee_b,
        max_block_body_size,
        max_tx_size,
        max_block_header_size,
        key_deposit: Lovelace(key_deposit),
        pool_deposit: Lovelace(pool_deposit),
        e_max,
        n_opt,
        a0: Rational { numerator: a0_num, denominator: a0_den },
        rho: Rational { numerator: rho_num, denominator: rho_den },
        tau: Rational { numerator: tau_num, denominator: tau_den },
        protocol_version_major: pv_major,
        protocol_version_minor: pv_minor,
        min_pool_cost: Lovelace(min_pool_cost),
        ada_per_utxo_byte: Lovelace(ada_per_utxo_byte),
        cost_models,
        execution_costs: ExUnitPrices {
            mem_price: Rational { numerator: mem_price_num, denominator: mem_price_den },
            step_price: Rational { numerator: step_price_num, denominator: step_price_den },
        },
        max_tx_ex_units: ExUnits { mem: tx_ex_mem, steps: tx_ex_steps },
        max_block_ex_units: ExUnits { mem: blk_ex_mem, steps: blk_ex_steps },
        max_val_size,
        collateral_percentage: collateral_pct,
        max_collateral_inputs,
        min_fee_ref_script_cost_per_byte: min_fee_ref_script,
        drep_deposit: Lovelace(drep_deposit),
        drep_activity,
        gov_action_deposit: Lovelace(gov_action_deposit),
        gov_action_lifetime,
        committee_min_size,
        committee_max_term_length,
        dvt_pp_network_group: Rational { numerator: dvt_pp_network_num, denominator: dvt_pp_network_den },
        dvt_pp_economic_group: Rational { numerator: dvt_pp_economic_num, denominator: dvt_pp_economic_den },
        dvt_pp_technical_group: Rational { numerator: dvt_pp_technical_num, denominator: dvt_pp_technical_den },
        dvt_pp_gov_group: Rational { numerator: dvt_pp_gov_num, denominator: dvt_pp_gov_den },
        dvt_hard_fork: Rational { numerator: dvt_hard_fork_num, denominator: dvt_hard_fork_den },
        dvt_no_confidence: Rational { numerator: dvt_no_confidence_num, denominator: dvt_no_confidence_den },
        dvt_committee_normal: Rational { numerator: dvt_committee_normal_num, denominator: dvt_committee_normal_den },
        dvt_committee_no_confidence: Rational { numerator: dvt_committee_nc_num, denominator: dvt_committee_nc_den },
        dvt_constitution: Rational { numerator: dvt_constitution_num, denominator: dvt_constitution_den },
        dvt_treasury_withdrawal: Rational { numerator: dvt_treasury_num, denominator: dvt_treasury_den },
        pvt_motion_no_confidence: Rational { numerator: pvt_motion_nc_num, denominator: pvt_motion_nc_den },
        pvt_committee_normal: Rational { numerator: pvt_committee_normal_num, denominator: pvt_committee_normal_den },
        pvt_committee_no_confidence: Rational { numerator: pvt_committee_nc_num, denominator: pvt_committee_nc_den },
        pvt_hard_fork: Rational { numerator: pvt_hard_fork_num, denominator: pvt_hard_fork_den },
        pvt_pp_security_group: Rational { numerator: pvt_pp_security_num, denominator: pvt_pp_security_den },
        // These fields come from genesis, not from PParams array(31)
        active_slots_coeff: 0.0,
        d: Rational { numerator: 0, denominator: 1 },
    };

    Ok((pp, off))
}

/// Decode cost models map: {0: [..], 1: [..], 2: [..]}
fn decode_cost_models(data: &[u8], off: &mut usize) -> Result<CostModels, SerializationError> {
    let (map_len, n) = decode_map_len(&data[*off..])?;
    *off += n;
    let count = map_len.ok_or_else(|| {
        SerializationError::CborDecode("indefinite map not expected for cost models".into())
    })?;

    let mut plutus_v1 = None;
    let mut plutus_v2 = None;
    let mut plutus_v3 = None;

    for _ in 0..count {
        let (key, n) = decode_uint(&data[*off..])?;
        *off += n;
        let (arr_len, n) = decode_array_len(&data[*off..])?;
        *off += n;
        let mut costs = Vec::with_capacity(arr_len);
        for _ in 0..arr_len {
            let (val, n) = decode_int(&data[*off..])?;
            *off += n;
            costs.push(val);
        }
        match key {
            0 => plutus_v1 = Some(costs),
            1 => plutus_v2 = Some(costs),
            2 => plutus_v3 = Some(costs),
            _ => {} // ignore unknown cost model versions
        }
    }

    Ok(CostModels { plutus_v1, plutus_v2, plutus_v3 })
}

/// Decode minFeeRefScriptCostPerByte — can be a rational or a plain uint.
fn decode_min_fee_ref_script(data: &[u8]) -> Result<(u64, usize), SerializationError> {
    // In practice this is encoded as a rational (e.g., 15/1) or a plain uint (15).
    // Check first byte to determine which.
    if data.is_empty() {
        return Err(SerializationError::CborDecode("eof".into()));
    }
    let major = data[0] >> 5;
    if major == 0 {
        // Plain uint
        decode_uint(data)
    } else if major == 6 || major == 4 {
        // Tag(30) [num, den] or plain [num, den] — extract numerator only
        // since dugite stores this as u64, do integer division
        let ((num, den), consumed) = decode_rational(data)?;
        if den == 0 {
            return Err(SerializationError::CborDecode("zero denominator".into()));
        }
        Ok((num / den, consumed))
    } else {
        Err(SerializationError::CborDecode(format!(
            "unexpected major type {major} for minFeeRefScript"
        )))
    }
}
```

Update `mod.rs` to export:
```rust
pub mod pparams;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo nextest run -p dugite-serialization -E 'test(test_decode_pparams)'`

Expected: PASS — all 26 field assertions match Koios values.

- [ ] **Step 6: Commit**

```bash
git add crates/dugite-serialization/src/haskell_snapshot/pparams.rs \
        crates/dugite-serialization/test_fixtures/preview_pparams_e1259.cbor
git commit -m "feat(serialization): PParams array(31) decoder verified against Koios (#347)"
```

---

## Task 3: PraosState Decoder (Nonces + OpCert Counters)

**Files:**
- Create: `crates/dugite-serialization/src/haskell_snapshot/praos.rs`
- Modify: `crates/dugite-serialization/src/haskell_snapshot/mod.rs`
- Modify: `crates/dugite-serialization/src/haskell_snapshot/tests.rs`

Decode the PraosState from the HeaderState telescope — extracts epoch nonces and opcert counters. Handles both array(7) and array(8) formats.

- [ ] **Step 1: Extract test fixture**

```bash
python3 -c "
import cbor2
with open('/tmp/mithril-ancillary/ledger/108794365/state', 'rb') as f:
    obj = cbor2.loads(f.read())
# PraosState: HeaderState.Telescope[6].current[1] = array(2)[0, array(7)[...]]
praos = obj[1][1][1][6][1]
with open('crates/dugite-serialization/test_fixtures/preview_praos_e1259.cbor', 'wb') as f:
    f.write(cbor2.dumps(praos))
"
```

- [ ] **Step 2: Write failing test**

Add to `tests.rs`:
```rust
use super::praos::decode_praos_state;

#[test]
fn test_decode_praos_state() {
    let data = include_bytes!("../../test_fixtures/preview_praos_e1259.cbor");
    let (praos, _) = decode_praos_state(data).unwrap();

    assert_eq!(praos.last_slot, Some(108794365));
    assert_eq!(praos.opcert_counters.len(), 456);
    // Nonces should not be all-zeros (that was the bug we're fixing)
    assert_ne!(praos.evolving_nonce, dugite_primitives::hash::Hash32::zero());
    assert_ne!(praos.epoch_nonce, dugite_primitives::hash::Hash32::zero());
    assert_ne!(praos.lab_nonce, dugite_primitives::hash::Hash32::zero());
}
```

- [ ] **Step 3: Implement decode_praos_state**

Create `crates/dugite-serialization/src/haskell_snapshot/praos.rs`:
```rust
//! Decode PraosState from the HeaderState consensus telescope.
//!
//! Wire format: array(2) [version=0, array(7|8) [lastSlot, oCertCounters,
//!   evolvingNonce, candidateNonce, epochNonce, ?previousEpochNonce,
//!   labNonce, lastEpochBlockNonce]]

use crate::error::SerializationError;
use crate::haskell_snapshot::cbor_utils::*;
use crate::haskell_snapshot::types::HaskellPraosState;
use dugite_primitives::hash::{Hash28, Hash32};
use std::collections::HashMap;

/// Decode versioned PraosState: array(2) [version=0, array(7|8)[...]].
pub fn decode_praos_state(data: &[u8]) -> Result<(HaskellPraosState, usize), SerializationError> {
    let (arr_len, mut off) = decode_array_len(data)?;
    if arr_len != 2 {
        return Err(SerializationError::InvalidLength { expected: 2, got: arr_len });
    }
    let (version, n) = decode_uint(&data[off..])?;
    off += n;
    if version != 0 {
        return Err(SerializationError::CborDecode(format!(
            "unsupported PraosState version {version}, expected 0"
        )));
    }

    let (inner_len, n) = decode_array_len(&data[off..])?;
    off += n;
    // Handle both array(7) (pre-previousEpochNonce) and array(8)
    if inner_len != 7 && inner_len != 8 {
        return Err(SerializationError::CborDecode(format!(
            "PraosState: expected array(7) or array(8), got array({inner_len})"
        )));
    }

    // [0] lastSlot: WithOrigin(SlotNo) = [] or [slot]
    let (wo_len, n) = decode_with_origin_len(&data[off..])?;
    off += n;
    let last_slot = if wo_len.is_some() {
        let (slot, n) = decode_uint(&data[off..])?;
        off += n;
        Some(slot)
    } else {
        None
    };

    // [1] oCertCounters: map(bytes(28) → uint)
    let (map_len, n) = decode_map_len(&data[off..])?;
    off += n;
    let count = map_len.unwrap_or(0);
    let mut opcert_counters = HashMap::with_capacity(count);
    for _ in 0..count {
        let (key_hash, n) = decode_hash28(&data[off..])?;
        off += n;
        let (counter, n) = decode_uint(&data[off..])?;
        off += n;
        opcert_counters.insert(key_hash, counter);
    }

    // [2] evolvingNonce
    let (evolving_nonce, n) = decode_nonce(&data[off..])?; off += n;
    // [3] candidateNonce
    let (candidate_nonce, n) = decode_nonce(&data[off..])?; off += n;
    // [4] epochNonce
    let (epoch_nonce, n) = decode_nonce(&data[off..])?; off += n;

    // [5] previousEpochNonce (only in array(8))
    if inner_len == 8 {
        let n = skip_cbor_value(&data[off..])?; // skip previousEpochNonce — not used by dugite
        off += n;
    }

    // [5 or 6] labNonce
    let (lab_nonce, n) = decode_nonce(&data[off..])?; off += n;
    // [6 or 7] lastEpochBlockNonce
    let (last_epoch_block_nonce, n) = decode_nonce(&data[off..])?; off += n;

    Ok((HaskellPraosState {
        last_slot,
        opcert_counters,
        evolving_nonce,
        candidate_nonce,
        epoch_nonce,
        lab_nonce,
        last_epoch_block_nonce,
    }, off))
}
```

- [ ] **Step 4: Run tests, verify pass**

Run: `cargo nextest run -p dugite-serialization -E 'test(test_decode_praos)'`

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-serialization/src/haskell_snapshot/praos.rs \
        crates/dugite-serialization/test_fixtures/preview_praos_e1259.cbor
git commit -m "feat(serialization): PraosState decoder for nonces and opcert counters (#347)"
```

---

## Task 4: CertState Decoder (VState + PState + DState)

**Files:**
- Create: `crates/dugite-serialization/src/haskell_snapshot/certstate.rs`

Decode the CertState with VState (DReps, committee), PState (pool registrations), and DState (accounts/delegations/rewards). This is the largest decoder since it extracts delegations, pool params, and reward accounts.

- [ ] **Step 1: Extract test fixture**

```bash
python3 -c "
import cbor2
with open('/tmp/mithril-ancillary/ledger/108794365/state', 'rb') as f:
    obj = cbor2.loads(f.read())
cert = obj[1][0][6][1][1][1][3][1][0]  # CertState = EpochState.LedgerState[0]
with open('crates/dugite-serialization/test_fixtures/preview_certstate_e1259.cbor', 'wb') as f:
    f.write(cbor2.dumps(cert))
"
```

- [ ] **Step 2: Write failing test**

```rust
use super::certstate::decode_certstate;

#[test]
fn test_decode_certstate() {
    let data = include_bytes!("../../test_fixtures/preview_certstate_e1259.cbor");
    let (cert, _) = decode_certstate(data).unwrap();

    // VState
    assert!(cert.vstate.dreps.len() > 8000); // ~8796 DReps on preview
    assert_eq!(cert.vstate.committee_state.len(), 8);
    assert_eq!(cert.vstate.dormant_epochs, 0);

    // PState
    assert!(cert.pstate.stake_pools.len() > 600); // ~664 pools
    assert!(cert.pstate.retirements.is_empty());

    // DState — must contain our SAND pool delegation
    assert!(cert.dstate.accounts.len() > 30000); // ~38630 accounts
    assert_eq!(cert.dstate.genesis_delegates.len(), 7);
}
```

- [ ] **Step 3: Implement decode_certstate**

Create `crates/dugite-serialization/src/haskell_snapshot/certstate.rs` implementing:
- `decode_certstate(data) → HaskellCertState`
- `decode_vstate(data) → HaskellVState`
- `decode_pstate(data) → HaskellPState`
- `decode_dstate(data) → HaskellDState`
- Helper decoders for DRepState, CommitteeAuth, StakePoolState, ConwayAccountState, relays

Each follows the same pattern: decode array header, decode fields positionally, handle optional/nullable fields with `decode_null()`.

- [ ] **Step 4: Run tests, verify pass**
- [ ] **Step 5: Commit**

---

## Task 5: SnapShots Decoder (Mark/Set/Go)

**Files:**
- Create: `crates/dugite-serialization/src/haskell_snapshot/snapshots.rs`

Decode the SnapShots (mark/set/go stake snapshots). Must handle both old array(3) and new array(2) formats.

- [ ] **Step 1-5: Same pattern** — extract fixture, write test, implement decoder, verify, commit.

Key: test that mark snapshot has ~9626 stake entries and ~664 pool entries (matching real data).

---

## Task 6: ConwayGovState Decoder

**Files:**
- Create: `crates/dugite-serialization/src/haskell_snapshot/govstate.rs`

Decode ConwayGovState array(7) — extract curPParams/prevPParams (via Task 2's decoder), proposals (raw CBOR preserved for later parsing), committee, constitution, futurePParams, and DRepPulsingState.

For the initial implementation, only fully decode PParams and constitution. Proposals and DRepPulsingState are preserved as raw CBOR bytes since dugite's governance replay during gap blocks will build the live state.

- [ ] **Step 1: Extract test fixture**
- [ ] **Step 2: Write failing test** — verify curPParams.min_fee_a == 44, constitution is decoded
- [ ] **Step 3: Implement decode_govstate** — calls `decode_pparams` for fields [3] and [4], skips complex fields
- [ ] **Step 4: Run tests, verify pass**
- [ ] **Step 5: Commit**

---

## Task 7: Top-Level ExtLedgerState Decoder

**Files:**
- Modify: `crates/dugite-serialization/src/haskell_snapshot/mod.rs`

Wire together all sub-decoders to decode the full state file into `HaskellLedgerState`.

- [ ] **Step 1: Extract full state file as test fixture** (the 16MB state file)

```bash
cp /tmp/mithril-ancillary/ledger/108794365/state \
   crates/dugite-serialization/test_fixtures/preview_state_e1259.bin
```

Note: this is a large fixture (~16MB). Consider git-lfs or .gitignore + download script.

- [ ] **Step 2: Write integration test**

```rust
#[test]
fn test_decode_full_state_file() {
    let data = include_bytes!("../../test_fixtures/preview_state_e1259.bin");
    let state = super::decode_state_file(data).unwrap();

    assert_eq!(state.epoch, 1259);
    assert_eq!(state.tip_slot, 108794365);
    assert_eq!(state.tip_block_no, 4168174);
    assert_eq!(state.new_epoch_state.treasury, 6565463297854481);
    assert_eq!(state.new_epoch_state.reserves, 8198151574246707);
    assert_eq!(state.new_epoch_state.cur_pparams.min_fee_a, 44);
    assert_ne!(state.praos_state.epoch_nonce, Hash32::zero());
    assert!(state.new_epoch_state.cert_state.pstate.stake_pools.len() > 600);
}
```

- [ ] **Step 3: Implement decode_state_file**

Navigate the HFC telescope, unwrap version wrappers, and call sub-decoders:
1. Decode `array(2)[1, ExtLedgerState]` — verify version = 1
2. Decode `ExtLedgerState = array(2)[ledger_telescope, header_state]`
3. Navigate ledger telescope: find Conway era (last element), decode `current_state`
4. Unwrap `array(2)[shelley_version=2, array(3|4)[tip, NES, transition, ?peras]]`
5. Decode NewEpochState by calling pparams, certstate, snapshots decoders
6. Navigate header telescope: find Conway, decode PraosState
7. Return `HaskellLedgerState`

- [ ] **Step 4: Run test, verify pass**
- [ ] **Step 5: Commit**

---

## Task 8: MemPack UTxO Decoder

**Files:**
- Create: `crates/dugite-serialization/src/mempack/mod.rs`
- Create: `crates/dugite-serialization/src/mempack/compact.rs`
- Create: `crates/dugite-serialization/src/mempack/txout.rs`
- Modify: `crates/dugite-serialization/src/lib.rs`

Decode the tables/tvar file: parse the CBOR indefinite map, decode MemPack TxIn keys (34-byte: TxId BE + TxIx LE) and MemPack TxOut values (tag-based binary format).

- [ ] **Step 1: Extract test fixture** (first 64KB of tvar for unit tests)

```bash
head -c 65536 /tmp/mithril-ancillary/ledger/108794365/tables/tvar > \
  crates/dugite-serialization/test_fixtures/preview_tvar_head_64k.bin
```

- [ ] **Step 2: Write test for TxIn decoding**

```rust
#[test]
fn test_decode_mempack_txin() {
    // Real key from preview tvar: 34 bytes
    let key = hex::decode(
        "00000c339a7d28e08060a69e3d9adf16846382f59a4d321f8b9580ffdb597c0b0100"
    ).unwrap();
    let (txid, txix) = super::decode_mempack_txin(&key).unwrap();
    assert_eq!(hex::encode(txid.as_bytes()), "00000c339a7d28e08060a69e3d9adf16846382f59a4d321f8b9580ffdb597c0b");
    assert_eq!(txix, 1); // 0x0100 little-endian = 1
}
```

- [ ] **Step 3: Write test for TxOut decoding (tag 0 = ADA-only)**

```rust
#[test]
fn test_decode_mempack_txout_ada_only() {
    let val = hex::decode(
        "001d60986cdecfc4f555a8605d621505a4a82c25c574f59fd0b79e2acdaf0200eedd01"
    ).unwrap();
    let txout = super::decode_mempack_txout(&val).unwrap();
    // Enterprise address on testnet (type 0x60)
    assert!(txout.address.starts_with(&[0x60]));
    assert_eq!(txout.address.len(), 29);
    // Coin value (decoded from CompactValue)
    assert!(txout.coin > 0);
    assert!(txout.multi_asset.is_none());
    assert!(txout.datum.is_none());
    assert!(txout.script_ref.is_none());
}
```

- [ ] **Step 4: Implement MemPack decoders**

`mod.rs`: Top-level tvar file iterator — parse CBOR array(1)[map(indef)], yield (TxIn, TxOut) pairs.

`compact.rs`: VarLen decoder (7-bit groups, continuation in MSB), CompactAddr, CompactValue.

`txout.rs`: Tag-based dispatcher for tags 0-5. Start with tags 0 and 2 (ADA-only, 83% of entries).

- [ ] **Step 5: Write streaming tvar iterator test**

```rust
#[test]
fn test_tvar_iterator() {
    let data = include_bytes!("../../test_fixtures/preview_tvar_head_64k.bin");
    let mut count = 0;
    for result in super::TvarIterator::new(data) {
        let (txin, txout) = result.unwrap();
        assert_eq!(txin.txid.as_bytes().len(), 32);
        assert!(txout.coin > 0 || !txout.multi_asset.as_ref().map_or(true, |m| m.is_empty()));
        count += 1;
    }
    assert!(count > 100, "Expected >100 entries in 64KB, got {count}");
}
```

- [ ] **Step 6: Run tests, verify pass**
- [ ] **Step 7: Commit**

---

## Task 9: Ancillary Archive Download and Verification

**Files:**
- Create: `crates/dugite-node/src/mithril/ancillary.rs`
- Modify: `crates/dugite-node/src/mithril.rs`

Implement V2 API integration: fetch cardano-database snapshot, download ancillary tar.zst, verify Ed25519 manifest signature + per-file SHA-256 digests.

- [ ] **Step 1: Add API response types for V2**

In `mithril.rs` or `mithril/ancillary.rs`, add:
```rust
#[derive(Debug, serde::Deserialize)]
struct CardanoDatabaseSnapshot {
    hash: String,
    beacon: SnapshotBeacon,
    ancillary: AncillaryInfo,
    certificate_hash: String,
}

#[derive(Debug, serde::Deserialize)]
struct AncillaryInfo {
    size_uncompressed: u64,
    locations: Vec<AncillaryLocation>,
}

#[derive(Debug, serde::Deserialize)]
struct AncillaryLocation {
    #[serde(rename = "type")]
    location_type: String,
    uri: String,
    compression_algorithm: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct AncillaryManifest {
    data: std::collections::BTreeMap<String, String>,
    signature: Option<String>,
}
```

- [ ] **Step 2: Implement ancillary verification**

In `ancillary.rs`:
```rust
/// Verify the ancillary manifest: check per-file SHA-256 digests and Ed25519 signature.
pub fn verify_ancillary_manifest(
    base_dir: &Path,
    manifest: &AncillaryManifest,
    verification_key: &[u8; 32],
) -> Result<()>
```

Algorithm:
1. For each `(path, expected_hash)` in `manifest.data`, compute SHA-256 of `base_dir/path`, compare hex strings.
2. Compute manifest hash: `SHA256(key1_utf8 || val1_utf8 || key2_utf8 || val2_utf8 || ...)` over BTreeMap entries.
3. Verify Ed25519 signature using `ed25519_dalek::VerifyingKey::verify_strict()`.

- [ ] **Step 3: Implement download_ancillary**

```rust
pub async fn download_ancillary(
    aggregator_url: &str,
    snapshot_hash: &str,
    temp_dir: &Path,
) -> Result<PathBuf>
```

1. GET `{aggregator}/artifact/cardano-database/{hash}` → parse `CardanoDatabaseSnapshot`
2. Extract first `cloud_storage` location from `ancillary.locations`
3. Download tar.zst to `{temp_dir}/ancillary-{hash}.tar.zst` with progress bar
4. Extract to `{temp_dir}/ancillary-{hash}/`
5. Parse + verify `ancillary_manifest.json`
6. Return path to extracted directory

- [ ] **Step 4: Add ancillary verification keys as constants**

```rust
const PREVIEW_ANCILLARY_VKEY: &str = "5b3138392c3139322c...";
const PREPROD_ANCILLARY_VKEY: &str = "5b3138392c3139322c..."; // same as preview
const MAINNET_ANCILLARY_VKEY: &str = "5b32332c37312c...";

fn ancillary_verification_key(network_magic: u64) -> Option<[u8; 32]> {
    // Decode hex-encoded JSON byte array → [u8; 32]
}
```

- [ ] **Step 5: Write test for manifest verification**

Use the real `ancillary_manifest.json` from `/tmp/mithril-ancillary/` as a test fixture.

- [ ] **Step 6: Run tests, verify pass**
- [ ] **Step 7: Commit**

---

## Task 10: Integrate Ancillary Into Mithril Import Flow

**Files:**
- Modify: `crates/dugite-node/src/mithril.rs`

Update `import_snapshot()` to download the ancillary archive alongside immutables. Keep the legacy flow as fallback.

- [ ] **Step 1: Add `--include-ancillary` flag** (default true)

- [ ] **Step 2: After immutable import, download ancillary**

Insert after the chunk file move (line ~354):
```rust
// Download and verify ancillary archive (ledger state + UTxO)
if include_ancillary {
    let ancillary_dir = download_ancillary(aggregator, &snapshot.hash, temp_dir).await?;
    // Move ledger/ directory to database_path
    // Move immutable trio to database_path/immutable/
}
```

- [ ] **Step 3: Preserve ancillary ledger files instead of deleting them**

The current code deletes `ledger-snapshot*.bin` (line 375-382). Change to:
- If ancillary import succeeded: move `ancillary_dir/ledger/<slot>/` to `database_path/haskell-ledger/`
- Still delete old `ledger-snapshot*.bin` (dugite format) to force fresh load from Haskell snapshot

- [ ] **Step 4: Test with real Mithril import on preview** (manual integration test)
- [ ] **Step 5: Commit**

---

## Task 11: LedgerState Population from Haskell Snapshot

**Files:**
- Modify: `crates/dugite-ledger/src/state/mod.rs`
- Modify: `crates/dugite-node/src/node/mod.rs`

The critical integration: after Mithril import with ancillary, decode the Haskell snapshot and populate dugite's `LedgerState` instead of constructing from genesis defaults.

- [ ] **Step 1: Add `from_haskell_snapshot()` to LedgerState**

In `crates/dugite-ledger/src/state/mod.rs`:
```rust
/// Create a LedgerState from a decoded Haskell ExtLedgerState snapshot.
/// This is used after Mithril ancillary import to restore the correct
/// ledger state without replaying from genesis.
pub fn from_haskell_snapshot(
    hs: &HaskellLedgerState,
    shelley_genesis: &ShelleyGenesis,
    protocol_params_overlay: &ProtocolParameters, // for active_slots_coeff, d
) -> Self
```

Map all fields from `HaskellLedgerState` → `LedgerState`:
- PParams from `hs.new_epoch_state.cur_pparams` (fill `active_slots_coeff` from genesis)
- Nonces from `hs.praos_state`
- Treasury/reserves from `hs.new_epoch_state`
- Delegations: extract from DState accounts `pool_delegation` field
- Pool params: convert from PState `stake_pools`
- Reward accounts: extract from DState accounts `balance` field
- Stake snapshots: convert from SnapShots
- Governance: convert from GovState
- OpCert counters from PraosState

- [ ] **Step 2: Modify Node::new() to load Haskell snapshot**

In `crates/dugite-node/src/node/mod.rs`, after snapshot loading block (~line 422):
```rust
// Check for Haskell ledger snapshot from ancillary import
let haskell_ledger_dir = database_path.join("haskell-ledger");
if haskell_ledger_dir.exists() {
    // Find newest snapshot directory (highest slot number)
    // Decode state file
    // Decode tvar file → populate UTxO store
    // Create LedgerState via from_haskell_snapshot()
    // Save as native dugite snapshot for subsequent restarts
    // Delete haskell-ledger/ (consumed)
}
```

- [ ] **Step 3: Populate UTxO store from tvar**

```rust
// Stream tvar entries into UtxoStore
let tvar_path = ledger_dir.join("tables/tvar");
let tvar_data = std::fs::read(&tvar_path)?;
for result in TvarIterator::new(&tvar_data) {
    let (txin, txout) = result?;
    utxo_store.insert(txin, txout)?;
}
```

- [ ] **Step 4: Add gap replay from snapshot to immutable tip**

After loading the Haskell state, replay blocks from `snapshot_slot` to `immutable_tip_slot`:
```rust
let gap_blocks = chain_db.get_blocks_in_range(snapshot_slot + 1, immutable_tip_slot)?;
for block in gap_blocks {
    ledger_state.apply_block(&block, BlockValidationMode::ApplyOnly)?;
}
```

- [ ] **Step 5: Write integration test**
- [ ] **Step 6: Commit**

---

## Task 12: Remove Heuristic Corrections

**Files:**
- Modify: `crates/dugite-node/src/node/mod.rs`

Remove the band-aid code that is no longer needed once ancillary import provides correct state.

- [ ] **Step 1: Remove stale defaults detection (lines ~492-510)**

Delete the code that checks `snapshot_mem == defaults_mem` and overlays genesis params.

- [ ] **Step 2: Remove protocol version behind era correction (lines ~512-541)**

Delete the code that maps era → expected protocol version and corrects mismatches.

- [ ] **Step 3: Run full test suite**

Run: `cargo nextest run --workspace`

Verify no tests depend on the heuristic behavior.

- [ ] **Step 4: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings`

- [ ] **Step 5: Commit**

```bash
git commit -m "refactor(node): remove stale-defaults and protocol-version heuristic corrections (#347)

These heuristics are no longer needed now that Mithril ancillary import
provides the correct ledger state from the Haskell snapshot."
```

---

## Task 13: End-to-End Integration Test

**Files:**
- Create: `crates/dugite-node/tests/ancillary_import.rs`

Full flow: download ancillary from preview, decode state, verify fields against Koios, replay gap.

- [ ] **Step 1: Write integration test** (requires network, tagged #[ignore] for CI)

```rust
#[tokio::test]
#[ignore] // Requires network access to Mithril aggregator
async fn test_ancillary_import_preview() {
    // 1. Download ancillary archive
    // 2. Verify manifest
    // 3. Decode state file
    // 4. Verify PParams match Koios
    // 5. Verify nonces are non-zero
    // 6. Verify treasury/reserves match Koios
    // 7. Populate UTxO store from tvar
    // 8. Verify UTxO count > 100K
}
```

- [ ] **Step 2: Run integration test manually**

Run: `cargo nextest run -p dugite-node -E 'test(ancillary_import)' -- --ignored`

- [ ] **Step 3: Commit**

```bash
git commit -m "test(node): add end-to-end ancillary import integration test (#347)"
```

---

## Task 14: Documentation and Cleanup

**Files:**
- Modify: `docs/` (mdBook)
- Modify: `CLAUDE.md` (if needed)

- [ ] **Step 1: Update mdBook docs** with Mithril ancillary import section
- [ ] **Step 2: Run cargo fmt and clippy**

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
```

- [ ] **Step 3: Run full test suite**

```bash
cargo nextest run --workspace
```

- [ ] **Step 4: Final commit and push**

```bash
git push origin main
```
