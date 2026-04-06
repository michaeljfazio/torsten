//! Decoder for the Haskell `SnapShots` snapshot format.
//!
//! The `SnapShots` value sits inside `EpochState` and carries the three
//! stake-distribution snapshots used by the Ouroboros reward calculation:
//!
//! ```text
//! SnapShots = array(4) [mark, set, go, fee: uint]
//! ```
//!
//! Each individual `SnapShot` is encoded in one of two formats depending on the
//! version of the Haskell node that produced the snapshot file:
//!
//! **Old format (array(3))** — current on preview at epoch 1259:
//! ```text
//! SnapShot = array(3) [
//!   stake:      map(credential → uint),       -- lovelace per staker
//!   delegs:     map(credential → bytes(28)),  -- staker → pool hash
//!   poolParams: map(bytes(28) → SnapShotPool) -- pool hash → pool data
//! ]
//! ```
//!
//! **New format (array(2))** — HEAD of cardano-ledger (not yet on testnet):
//! ```text
//! SnapShot = array(2) [
//!   stakeWithDelegation: map(credential → array(2)[coin, bytes(28)]),
//!   poolSnapshots:       map(bytes(28) → SnapShotPool)
//! ]
//! ```
//!
//! Both formats are handled by checking the outer array length.
//!
//! ## Pool snapshot layout (old and new)
//!
//! ```text
//! SnapShotPool = array(9 | 10) [
//!   [0] pool_id:        bytes(28)           -- pool key hash (same as map key, skip)
//!   [1] vrf_hash:       bytes(32)
//!   [2] pledge:         uint
//!   [3] cost:           uint
//!   [4] margin:         tag(30) [num, den]
//!   [5] reward_account: bytes(29)
//!   [6] owners:         tag(258)? array([bytes(28)])
//!   [7] relays:         array([relay])
//!   [8] metadata:       null | array(2)[url, hash32]
//!   [9] (optional)      extra field — skipped
//! ]
//! ```

use crate::error::SerializationError;
use crate::haskell_snapshot::cbor_utils::{
    decode_array_len, decode_bytes, decode_credential, decode_hash28, decode_hash32, decode_map_len,
    decode_null, decode_rational, decode_text, decode_uint, skip_cbor_value,
};
use crate::haskell_snapshot::types::{HaskellRelay, HaskellSnapShot, HaskellSnapShotPool, HaskellSnapShots};
use dugite_primitives::hash::{Hash28, Hash32};
use std::collections::HashMap;

/// Decode a complete `SnapShots = array(4) [mark, set, go, fee]`.
///
/// Returns `(snapshots, bytes_consumed)`.
pub fn decode_snapshots(data: &[u8]) -> Result<(HaskellSnapShots, usize), SerializationError> {
    let mut off = 0;

    // Top-level array(4)
    let (arr_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if arr_len != 4 {
        return Err(SerializationError::CborDecode(format!(
            "SnapShots: expected array(4), got array({arr_len})"
        )));
    }

    // [0] mark
    let (mark, n) = decode_snapshot(&data[off..])?;
    off += n;

    // [1] set
    let (set, n) = decode_snapshot(&data[off..])?;
    off += n;

    // [2] go
    let (go, n) = decode_snapshot(&data[off..])?;
    off += n;

    // [3] fee: uint (accumulated fees not yet included in rewards)
    let (fee, n) = decode_uint(&data[off..])?;
    off += n;

    Ok((HaskellSnapShots { mark, set, go, fee }, off))
}

/// Decode a single `SnapShot`, dispatching on old (array(3)) vs new (array(2)) format.
fn decode_snapshot(data: &[u8]) -> Result<(HaskellSnapShot, usize), SerializationError> {
    let mut off = 0;

    let (arr_len, n) = decode_array_len(&data[off..])?;
    off += n;

    match arr_len {
        3 => decode_snapshot_old(data, &mut off, arr_len).map(|s| (s, off)),
        2 => decode_snapshot_new(data, &mut off, arr_len).map(|s| (s, off)),
        _ => Err(SerializationError::CborDecode(format!(
            "SnapShot: expected array(2) or array(3), got array({arr_len})"
        ))),
    }
}

/// Decode old-format `SnapShot = array(3) [stake_map, delegs_map, pool_params_map]`.
///
/// `off` is positioned immediately after the array header on entry and advanced
/// to the end of the snapshot on return.
fn decode_snapshot_old(
    data: &[u8],
    off: &mut usize,
    _len: usize,
) -> Result<HaskellSnapShot, SerializationError> {
    // [0] stake: map(credential → uint)
    let (stake_count, n) = decode_definite_map_len(&data[*off..])?;
    *off += n;
    let mut stake: HashMap<(u8, Hash28), u64> = HashMap::with_capacity(stake_count);
    for _ in 0..stake_count {
        let (cred, n) = decode_credential(&data[*off..])?;
        *off += n;
        let (coin, n) = decode_uint(&data[*off..])?;
        *off += n;
        stake.insert(cred, coin);
    }

    // [1] delegations: map(credential → bytes(28))
    let (deleg_count, n) = decode_definite_map_len(&data[*off..])?;
    *off += n;
    let mut delegations: HashMap<(u8, Hash28), Hash28> = HashMap::with_capacity(deleg_count);
    for _ in 0..deleg_count {
        let (cred, n) = decode_credential(&data[*off..])?;
        *off += n;
        let (pool_hash, n) = decode_hash28(&data[*off..])?;
        *off += n;
        delegations.insert(cred, pool_hash);
    }

    // [2] poolParams: map(bytes(28) → SnapShotPool)
    let (pool_count, n) = decode_definite_map_len(&data[*off..])?;
    *off += n;
    let mut pool_params: HashMap<Hash28, HaskellSnapShotPool> =
        HashMap::with_capacity(pool_count);
    for _ in 0..pool_count {
        // Map key: bytes(28) pool hash
        let (pool_id, n) = decode_hash28(&data[*off..])?;
        *off += n;
        // Map value: SnapShotPool array
        let (pool, n) = decode_snapshot_pool(&data[*off..])?;
        *off += n;
        pool_params.insert(pool_id, pool);
    }

    Ok(HaskellSnapShot {
        stake,
        delegations,
        pool_params,
    })
}

/// Decode new-format `SnapShot = array(2) [active_stake_map, pool_snapshots_map]`.
///
/// The active stake map combines stake and delegations:
/// ```text
/// active_stake_map: map(credential → array(2)[coin: uint, pool: bytes(28)])
/// ```
///
/// `off` is positioned immediately after the array header on entry and advanced
/// to the end of the snapshot on return.
fn decode_snapshot_new(
    data: &[u8],
    off: &mut usize,
    _len: usize,
) -> Result<HaskellSnapShot, SerializationError> {
    // [0] stakeWithDelegation: map(credential → [coin, pool_hash])
    let (active_count, n) = decode_definite_map_len(&data[*off..])?;
    *off += n;
    let mut stake: HashMap<(u8, Hash28), u64> = HashMap::with_capacity(active_count);
    let mut delegations: HashMap<(u8, Hash28), Hash28> = HashMap::with_capacity(active_count);
    for _ in 0..active_count {
        let (cred, n) = decode_credential(&data[*off..])?;
        *off += n;

        // StakeWithDelegation = array(2) [coin: uint, pool: bytes(28)]
        let (inner_len, n) = decode_array_len(&data[*off..])?;
        *off += n;
        if inner_len != 2 {
            return Err(SerializationError::CborDecode(format!(
                "StakeWithDelegation: expected array(2), got array({inner_len})"
            )));
        }
        let (coin, n) = decode_uint(&data[*off..])?;
        *off += n;
        let (pool_hash, n) = decode_hash28(&data[*off..])?;
        *off += n;

        stake.insert(cred, coin);
        delegations.insert(cred, pool_hash);
    }

    // [1] poolSnapshots: map(bytes(28) → SnapShotPool)
    let (pool_count, n) = decode_definite_map_len(&data[*off..])?;
    *off += n;
    let mut pool_params: HashMap<Hash28, HaskellSnapShotPool> =
        HashMap::with_capacity(pool_count);
    for _ in 0..pool_count {
        let (pool_id, n) = decode_hash28(&data[*off..])?;
        *off += n;
        let (pool, n) = decode_snapshot_pool(&data[*off..])?;
        *off += n;
        pool_params.insert(pool_id, pool);
    }

    Ok(HaskellSnapShot {
        stake,
        delegations,
        pool_params,
    })
}

/// Decode a `SnapShotPool` array.
///
/// The Haskell `SnapShotPool` encodes both the pool identity key and pool
/// parameters together in a single array:
///
/// ```text
/// SnapShotPool = array(9 | 10) [
///   [0] pool_id:        bytes(28)          -- pool key hash (same as map key — skipped)
///   [1] vrf_hash:       bytes(32)
///   [2] pledge:         uint
///   [3] cost:           uint
///   [4] margin:         tag(30)? [num, den]
///   [5] reward_account: bytes(29)          -- raw reward address bytes
///   [6] owners:         tag(258)? array([bytes(28)])
///   [7] relays:         array([relay])
///   [8] metadata:       null | array(2)[url, hash32]
///   [9] (optional)      extra field — skipped for forward compatibility
/// ]
/// ```
///
/// Returns `(pool, bytes_consumed)`.
fn decode_snapshot_pool(data: &[u8]) -> Result<(HaskellSnapShotPool, usize), SerializationError> {
    let mut off = 0;

    let (arr_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if arr_len < 9 {
        return Err(SerializationError::CborDecode(format!(
            "SnapShotPool: expected array(9+), got array({arr_len})"
        )));
    }

    // [0] pool_id: bytes(28) — identical to the outer map key; skip it.
    let n = skip_cbor_value(&data[off..])?;
    off += n;

    // [1] vrf_hash: bytes(32)
    let (vrf_hash, n) = decode_hash32(&data[off..])?;
    off += n;

    // [2] pledge: uint
    let (pledge, n) = decode_uint(&data[off..])?;
    off += n;

    // [3] cost: uint
    let (cost, n) = decode_uint(&data[off..])?;
    off += n;

    // [4] margin: Rational — tag(30) [num, den] or plain [num, den]
    let ((margin_num, margin_den), n) = decode_rational(&data[off..])?;
    off += n;

    // [5] reward_account: bytes(29) — raw reward address bytes (network tag + hash)
    let (reward_bytes, n) = decode_bytes(&data[off..])?;
    off += n;
    let reward_account = reward_bytes.to_vec();

    // [6] owners: CBOR set tag(258)? array([bytes(28)])
    //
    // The owners field may be wrapped in CBOR set tag 258 (0xd9 0x01 0x02).
    // Skip the tag if present, then decode the array.
    let n = skip_set_tag(&data[off..])?;
    off += n;
    let (owners_len, n) = decode_array_len(&data[off..])?;
    off += n;
    let mut owners = Vec::with_capacity(owners_len);
    for _ in 0..owners_len {
        let (hash, n) = decode_hash28(&data[off..])?;
        off += n;
        owners.push(hash);
    }

    // [7] relays: array([relay])
    let (relays_len, n) = decode_array_len(&data[off..])?;
    off += n;
    let mut relays = Vec::with_capacity(relays_len);
    for _ in 0..relays_len {
        let (relay, n) = decode_relay(&data[off..])?;
        off += n;
        relays.push(relay);
    }

    // [8] metadata: null | array(2)[url, hash32]
    let (metadata, n) = decode_optional_pool_metadata(&data[off..])?;
    off += n;

    // [9+] any extra fields — skip for forward compatibility
    for _ in 9..arr_len {
        let n = skip_cbor_value(&data[off..])?;
        off += n;
    }

    Ok((
        HaskellSnapShotPool {
            vrf_hash,
            pledge,
            cost,
            margin_num,
            margin_den,
            reward_account,
            owners,
            relays,
            metadata,
        },
        off,
    ))
}

// ── Relay decoding ─────────────────────────────────────────────────────────────
//
// Pool relays use the same CBOR encoding as in CertState.  The logic is
// duplicated here (rather than shared via a `pub` helper in certstate.rs) to
// keep each module self-contained and avoid coupling the snapshot decoder to
// internal certstate implementation details.

/// Decode a pool relay descriptor.
///
/// - `[0, port?, ipv4?, ipv6?]` → `SingleHostAddr`
/// - `[1, port?, dns_text]`     → `SingleHostName`
/// - `[2, dns_text]`            → `MultiHostName`
fn decode_relay(data: &[u8]) -> Result<(HaskellRelay, usize), SerializationError> {
    let mut off = 0;

    let (arr_len, n) = decode_array_len(&data[off..])?;
    off += n;

    let (tag, n) = decode_uint(&data[off..])?;
    off += n;

    match tag {
        0 => {
            // SingleHostAddr: [0, port?, ipv4?, ipv6?]
            if arr_len != 4 {
                return Err(SerializationError::CborDecode(format!(
                    "SnapShot SingleHostAddr relay: expected array(4), got array({arr_len})"
                )));
            }

            // port: nullable uint
            let (is_null, n) = decode_null(&data[off..])?;
            off += n;
            let port = if is_null {
                None
            } else {
                let (p, n) = decode_uint(&data[off..])?;
                off += n;
                Some(p as u16)
            };

            // ipv4: nullable bytes(4)
            let (is_null, n) = decode_null(&data[off..])?;
            off += n;
            let ipv4 = if is_null {
                None
            } else {
                let (b, n) = decode_bytes(&data[off..])?;
                off += n;
                if b.len() == 4 {
                    Some([b[0], b[1], b[2], b[3]])
                } else {
                    None
                }
            };

            // ipv6: nullable bytes(16)
            let (is_null, n) = decode_null(&data[off..])?;
            off += n;
            let ipv6 = if is_null {
                None
            } else {
                let (b, n) = decode_bytes(&data[off..])?;
                off += n;
                if b.len() == 16 {
                    let mut arr = [0u8; 16];
                    arr.copy_from_slice(b);
                    Some(arr)
                } else {
                    None
                }
            };

            Ok((HaskellRelay::SingleHostAddr(port, ipv4, ipv6), off))
        }
        1 => {
            // SingleHostName: [1, port?, dns_text]
            if arr_len != 3 {
                return Err(SerializationError::CborDecode(format!(
                    "SnapShot SingleHostName relay: expected array(3), got array({arr_len})"
                )));
            }

            // port: nullable uint
            let (is_null, n) = decode_null(&data[off..])?;
            off += n;
            let port = if is_null {
                None
            } else {
                let (p, n) = decode_uint(&data[off..])?;
                off += n;
                Some(p as u16)
            };

            // dns: text
            let (dns, n) = decode_text(&data[off..])?;
            off += n;

            Ok((HaskellRelay::SingleHostName(port, dns.to_string()), off))
        }
        2 => {
            // MultiHostName: [2, dns_text]
            if arr_len != 2 {
                return Err(SerializationError::CborDecode(format!(
                    "SnapShot MultiHostName relay: expected array(2), got array({arr_len})"
                )));
            }

            let (dns, n) = decode_text(&data[off..])?;
            off += n;

            Ok((HaskellRelay::MultiHostName(dns.to_string()), off))
        }
        _ => Err(SerializationError::CborDecode(format!(
            "SnapShot Relay: unknown tag {tag}"
        ))),
    }
}

// ── Shared helpers ─────────────────────────────────────────────────────────────

/// Decode pool metadata: `null` or `array(2) [url_text, hash32]`.
///
/// Unlike the more general `decode_optional_anchor` in certstate.rs, pool
/// metadata in snapshot pool params is always either CBOR null or a plain
/// `array(2)` — there is no `StrictMaybe` wrapping here.
fn decode_optional_pool_metadata(
    data: &[u8],
) -> Result<(Option<(String, Hash32)>, usize), SerializationError> {
    // Check for CBOR null (0xf6)
    let (is_null, n) = decode_null(data)?;
    if is_null {
        return Ok((None, n));
    }

    let mut off = 0;
    let (arr_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if arr_len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "SnapShot pool metadata: expected array(2) or null, got array({arr_len})"
        )));
    }

    let (url, n) = decode_text(&data[off..])?;
    off += n;
    let (hash, n) = decode_hash32(&data[off..])?;
    off += n;

    Ok((Some((url.to_string(), hash)), off))
}

/// Decode a definite-length map header, returning the element count.
///
/// Returns an error on indefinite-length maps, which are not used in the
/// Haskell snapshot wire format.
fn decode_definite_map_len(data: &[u8]) -> Result<(usize, usize), SerializationError> {
    let (opt_len, n) = decode_map_len(data)?;
    match opt_len {
        Some(len) => Ok((len, n)),
        None => Err(SerializationError::CborDecode(
            "SnapShots: expected definite-length map, got indefinite".into(),
        )),
    }
}

/// Skip CBOR set tag 258 (`0xd9 0x01 0x02`) if present.
///
/// Returns the number of bytes consumed (3 if the tag is present, 0 if not).
fn skip_set_tag(data: &[u8]) -> Result<usize, SerializationError> {
    if data.len() >= 3 && data[0] == 0xd9 && data[1] == 0x01 && data[2] == 0x02 {
        Ok(3)
    } else {
        Ok(0)
    }
}
