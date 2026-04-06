//! Decoder for the Haskell `CertState` snapshot format.
//!
//! CertState contains three sub-states:
//!
//! ```text
//! CertState = array(3) [
//!   VState  — DRep registrations + committee state + dormant epochs
//!   PState  — pool registrations, future params, retirements, VRF hashes
//!   DState  — staking accounts, genesis delegates, instantaneous rewards
//! ]
//! ```
//!
//! Each sub-state is decoded independently and the results are assembled into
//! [`HaskellCertState`].

use crate::error::SerializationError;
use crate::haskell_snapshot::cbor_utils::{
    decode_array_len, decode_bytes, decode_credential, decode_hash28, decode_hash32, decode_int,
    decode_null, decode_rational, decode_text, decode_uint, skip_cbor_value, MapReader,
};
use crate::haskell_snapshot::types::{
    HaskellAccountState, HaskellCertState, HaskellCommitteeAuth, HaskellDRep, HaskellDRepState,
    HaskellDState, HaskellPState, HaskellRelay, HaskellStakePoolState, HaskellVState,
};
use dugite_primitives::hash::{Hash28, Hash32};
use dugite_primitives::time::EpochNo;
use std::collections::HashMap;

/// Decode a complete `CertState = array(3) [VState, PState, DState]`.
///
/// Returns `(cert_state, bytes_consumed)`.
pub fn decode_certstate(data: &[u8]) -> Result<(HaskellCertState, usize), SerializationError> {
    let mut off = 0;

    let (arr_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if arr_len != 3 {
        return Err(SerializationError::CborDecode(format!(
            "CertState: expected array(3), got array({arr_len})"
        )));
    }

    // [0] VState
    let (vstate, n) = decode_vstate(&data[off..])?;
    off += n;

    // [1] PState
    let (pstate, n) = decode_pstate(&data[off..])?;
    off += n;

    // [2] DState
    let (dstate, n) = decode_dstate(&data[off..])?;
    off += n;

    Ok((
        HaskellCertState {
            vstate,
            pstate,
            dstate,
        },
        off,
    ))
}

// ── VState ────────────────────────────────────────────────────────────────────

/// Decode `VState = array(3) [dreps, committeeState, dormantEpochs]`.
fn decode_vstate(data: &[u8]) -> Result<(HaskellVState, usize), SerializationError> {
    let mut off = 0;

    let (arr_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if arr_len != 3 {
        return Err(SerializationError::CborDecode(format!(
            "VState: expected array(3), got array({arr_len})"
        )));
    }

    // [0] dreps: map(credential → DRepState)
    let (mut reader, n) = MapReader::new(&data[off..])?;
    off += n;
    let mut dreps = HashMap::with_capacity(reader.size_hint());
    while reader.has_next(&data[off..])? {
        let (cred, n) = decode_credential(&data[off..])?;
        off += n;
        let (state, n) = decode_drep_state(&data[off..])?;
        off += n;
        dreps.insert(cred, state);
    }
    off += reader.finish(&data[off..])?;

    // [1] committeeState: map((tag,credential) → CommitteeAuth)
    let (mut reader, n) = MapReader::new(&data[off..])?;
    off += n;
    let mut committee_state = HashMap::with_capacity(reader.size_hint());
    while reader.has_next(&data[off..])? {
        let (cred, n) = decode_credential(&data[off..])?;
        off += n;
        let (auth, n) = decode_committee_auth(&data[off..])?;
        off += n;
        committee_state.insert(cred, auth);
    }
    off += reader.finish(&data[off..])?;

    // [2] dormantEpochs: uint
    let (dormant_epochs, n) = decode_uint(&data[off..])?;
    off += n;

    Ok((
        HaskellVState {
            dreps,
            committee_state,
            dormant_epochs,
        },
        off,
    ))
}

/// Decode `DRepState = array(4) [expiry, anchor, deposit, delegators_set]`.
///
/// The delegators set (field 3) is skipped — it is reconstructed from DState.
fn decode_drep_state(data: &[u8]) -> Result<(HaskellDRepState, usize), SerializationError> {
    let mut off = 0;

    let (arr_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if arr_len != 4 {
        return Err(SerializationError::CborDecode(format!(
            "DRepState: expected array(4), got array({arr_len})"
        )));
    }

    // [0] expiry: uint (epoch number)
    let (expiry, n) = decode_uint(&data[off..])?;
    off += n;

    // [1] anchor: null | array(2) [url, hash32]
    let (anchor, n) = decode_optional_anchor(&data[off..])?;
    off += n;

    // [2] deposit: uint
    let (deposit, n) = decode_uint(&data[off..])?;
    off += n;

    // [3] delegators_set: skip (CBOR set tag 258 or array)
    let n = skip_cbor_value(&data[off..])?;
    off += n;

    Ok((
        HaskellDRepState {
            expiry: EpochNo(expiry),
            deposit,
            anchor,
        },
        off,
    ))
}

/// Decode `CommitteeAuth = array(2) [tag, payload]`.
///
/// - tag 0: `[0, credential]` → `Hot(type, hash28)`
/// - tag 1: `[1, null | anchor]` → `Resigned(option)`
fn decode_committee_auth(data: &[u8]) -> Result<(HaskellCommitteeAuth, usize), SerializationError> {
    let mut off = 0;

    let (arr_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if arr_len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "CommitteeAuth: expected array(2), got array({arr_len})"
        )));
    }

    let (tag, n) = decode_uint(&data[off..])?;
    off += n;

    match tag {
        0 => {
            // Hot credential: [0, credential]
            let ((cred_type, hash), n) = decode_credential(&data[off..])?;
            off += n;
            Ok((HaskellCommitteeAuth::Hot(cred_type, hash), off))
        }
        1 => {
            // Resigned: [1, null | anchor]
            let (anchor, n) = decode_optional_anchor(&data[off..])?;
            off += n;
            Ok((HaskellCommitteeAuth::Resigned(anchor), off))
        }
        _ => Err(SerializationError::CborDecode(format!(
            "CommitteeAuth: unknown tag {tag}"
        ))),
    }
}

// ── PState ────────────────────────────────────────────────────────────────────

/// Decode `PState = array(4) [vrfKeyHashes, stakePools, futurePoolParams, retirements]`.
fn decode_pstate(data: &[u8]) -> Result<(HaskellPState, usize), SerializationError> {
    let mut off = 0;

    let (arr_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if arr_len != 4 {
        return Err(SerializationError::CborDecode(format!(
            "PState: expected array(4), got array({arr_len})"
        )));
    }

    // [0] vrfKeyHashes: map(bytes(32) → uint)
    let (mut reader, n) = MapReader::new(&data[off..])?;
    off += n;
    let mut vrf_key_hashes = HashMap::with_capacity(reader.size_hint());
    while reader.has_next(&data[off..])? {
        let (hash, n) = decode_hash32(&data[off..])?;
        off += n;
        let (count, n) = decode_uint(&data[off..])?;
        off += n;
        vrf_key_hashes.insert(hash, count);
    }
    off += reader.finish(&data[off..])?;

    // [1] stakePools: map(bytes(28) → StakePoolState(9|10))
    let (mut reader, n) = MapReader::new(&data[off..])?;
    off += n;
    let mut stake_pools = HashMap::with_capacity(reader.size_hint());
    while reader.has_next(&data[off..])? {
        let (pool_id, n) = decode_hash28(&data[off..])?;
        off += n;
        let (pool, n) = decode_stake_pool_state(&data[off..])?;
        off += n;
        stake_pools.insert(pool_id, pool);
    }
    off += reader.finish(&data[off..])?;

    // [2] futureStakePoolParams: map(bytes(28) → PoolParams(9))
    let (mut reader, n) = MapReader::new(&data[off..])?;
    off += n;
    let mut future_pool_params = HashMap::with_capacity(reader.size_hint());
    while reader.has_next(&data[off..])? {
        let (pool_id, n) = decode_hash28(&data[off..])?;
        off += n;
        let (params, n) = decode_stake_pool_state(&data[off..])?;
        off += n;
        future_pool_params.insert(pool_id, params);
    }
    off += reader.finish(&data[off..])?;

    // [3] retirements: map(bytes(28) → uint)
    let (mut reader, n) = MapReader::new(&data[off..])?;
    off += n;
    let mut retirements = HashMap::with_capacity(reader.size_hint());
    while reader.has_next(&data[off..])? {
        let (pool_id, n) = decode_hash28(&data[off..])?;
        off += n;
        let (epoch, n) = decode_uint(&data[off..])?;
        off += n;
        retirements.insert(pool_id, EpochNo(epoch));
    }
    off += reader.finish(&data[off..])?;

    Ok((
        HaskellPState {
            vrf_key_hashes,
            stake_pools,
            future_pool_params,
            retirements,
        },
        off,
    ))
}

/// Decode a `StakePoolState` (array of 9 or 10 fields).
///
/// If 10 fields are present, the last field (delegators set) is skipped.
fn decode_stake_pool_state(
    data: &[u8],
) -> Result<(HaskellStakePoolState, usize), SerializationError> {
    let mut off = 0;

    let (arr_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if arr_len != 9 && arr_len != 10 {
        return Err(SerializationError::CborDecode(format!(
            "StakePoolState: expected array(9) or array(10), got array({arr_len})"
        )));
    }

    // [0] vrf_hash: bytes(32)
    let (vrf_hash, n) = decode_hash32(&data[off..])?;
    off += n;

    // [1] pledge: uint
    let (pledge, n) = decode_uint(&data[off..])?;
    off += n;

    // [2] cost: uint
    let (cost, n) = decode_uint(&data[off..])?;
    off += n;

    // [3] margin: rational (tag30 [num, den])
    let ((margin_num, margin_den), n) = decode_rational(&data[off..])?;
    off += n;

    // [4] reward_account: bytes(29) — raw reward address
    let (reward_bytes, n) = decode_bytes(&data[off..])?;
    off += n;
    let reward_account = reward_bytes.to_vec();

    // [5] owners: set (tag 258) or plain array of bytes(28)
    let n_owners = skip_set_tag(&data[off..])?;
    off += n_owners;
    let (owners_len, n) = decode_array_len(&data[off..])?;
    off += n;
    let mut owners = Vec::with_capacity(owners_len);
    for _ in 0..owners_len {
        let (hash, n) = decode_hash28(&data[off..])?;
        off += n;
        owners.push(hash);
    }

    // [6] relays: array of relay encodings
    let (relays_len, n) = decode_array_len(&data[off..])?;
    off += n;
    let mut relays = Vec::with_capacity(relays_len);
    for _ in 0..relays_len {
        let (relay, n) = decode_relay(&data[off..])?;
        off += n;
        relays.push(relay);
    }

    // [7] metadata: null | array(2) [url, hash32]
    let (metadata, n) = decode_optional_anchor(&data[off..])?;
    off += n;

    // [8] deposit: uint
    let (deposit, n) = decode_uint(&data[off..])?;
    off += n;

    // [9] delegators set (only in array(10)) — skip
    if arr_len == 10 {
        let n = skip_cbor_value(&data[off..])?;
        off += n;
    }

    Ok((
        HaskellStakePoolState {
            vrf_hash,
            pledge,
            cost,
            margin_num,
            margin_den,
            reward_account,
            owners,
            relays,
            metadata,
            deposit,
        },
        off,
    ))
}

/// Decode a pool relay descriptor.
///
/// - `[0, port?, ipv4?, ipv6?]` → SingleHostAddr
/// - `[1, port?, dns_text]`     → SingleHostName
/// - `[2, dns_text]`            → MultiHostName
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
                    "SingleHostAddr relay: expected array(4), got array({arr_len})"
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
                    "SingleHostName relay: expected array(3), got array({arr_len})"
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
                    "MultiHostName relay: expected array(2), got array({arr_len})"
                )));
            }

            let (dns, n) = decode_text(&data[off..])?;
            off += n;

            Ok((HaskellRelay::MultiHostName(dns.to_string()), off))
        }
        _ => Err(SerializationError::CborDecode(format!(
            "Relay: unknown tag {tag}"
        ))),
    }
}

// ── DState ────────────────────────────────────────────────────────────────────

/// Decode `DState = array(4) [accounts, futureGenDelegs, genDelegs, iRewards]`.
fn decode_dstate(data: &[u8]) -> Result<(HaskellDState, usize), SerializationError> {
    let mut off = 0;

    let (arr_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if arr_len != 4 {
        return Err(SerializationError::CborDecode(format!(
            "DState: expected array(4), got array({arr_len})"
        )));
    }

    // [0] accounts: map(credential → ConwayAccountState(4))
    let (mut reader, n) = MapReader::new(&data[off..])?;
    off += n;
    let mut accounts = HashMap::with_capacity(reader.size_hint());
    while reader.has_next(&data[off..])? {
        let (cred, n) = decode_credential(&data[off..])?;
        off += n;
        let (acct, n) = decode_account_state(&data[off..])?;
        off += n;
        accounts.insert(cred, acct);
    }
    off += reader.finish(&data[off..])?;

    // [1] futureGenDelegs: map (empty on Conway — skip all entries)
    let (mut reader, n) = MapReader::new(&data[off..])?;
    off += n;
    while reader.has_next(&data[off..])? {
        // key + value: skip both
        let n = skip_cbor_value(&data[off..])?;
        off += n;
        let n = skip_cbor_value(&data[off..])?;
        off += n;
    }
    off += reader.finish(&data[off..])?;

    // [2] genDelegs: map(bytes(28) → array(2)[bytes(28), bytes(32)])
    let (mut reader, n) = MapReader::new(&data[off..])?;
    off += n;
    let mut genesis_delegates = HashMap::with_capacity(reader.size_hint());
    while reader.has_next(&data[off..])? {
        let (gen_key, n) = decode_hash28(&data[off..])?;
        off += n;

        let (inner_len, n) = decode_array_len(&data[off..])?;
        off += n;
        if inner_len != 2 {
            return Err(SerializationError::CborDecode(format!(
                "genDelegs value: expected array(2), got array({inner_len})"
            )));
        }
        let (delegate, n) = decode_hash28(&data[off..])?;
        off += n;
        let (vrf, n) = decode_hash32(&data[off..])?;
        off += n;

        genesis_delegates.insert(gen_key, (delegate, vrf));
    }
    off += reader.finish(&data[off..])?;

    // [3] iRewards: array(4) [reserves_map, treasury_map, delta_reserves, delta_treasury]
    let (irewards_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if irewards_len != 4 {
        return Err(SerializationError::CborDecode(format!(
            "InstantaneousRewards: expected array(4), got array({irewards_len})"
        )));
    }

    // reserves_map: map(credential → uint)
    let (i_rewards_reserves, n) = decode_credential_coin_map(&data[off..])?;
    off += n;

    // treasury_map: map(credential → uint)
    let (i_rewards_treasury, n) = decode_credential_coin_map(&data[off..])?;
    off += n;

    // delta_reserves: int (signed)
    let (delta_reserves, n) = decode_int(&data[off..])?;
    off += n;

    // delta_treasury: int (signed)
    let (delta_treasury, n) = decode_int(&data[off..])?;
    off += n;

    Ok((
        HaskellDState {
            accounts,
            genesis_delegates,
            i_rewards_reserves,
            i_rewards_treasury,
            delta_reserves,
            delta_treasury,
        },
        off,
    ))
}

/// Decode `ConwayAccountState = array(4) [balance, deposit, pool?, drep?]`.
fn decode_account_state(data: &[u8]) -> Result<(HaskellAccountState, usize), SerializationError> {
    let mut off = 0;

    let (arr_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if arr_len != 4 {
        return Err(SerializationError::CborDecode(format!(
            "AccountState: expected array(4), got array({arr_len})"
        )));
    }

    // [0] balance: uint
    let (balance, n) = decode_uint(&data[off..])?;
    off += n;

    // [1] deposit: uint
    let (deposit, n) = decode_uint(&data[off..])?;
    off += n;

    // [2] pool_delegation: null | bytes(28)
    let (is_null, n) = decode_null(&data[off..])?;
    off += n;
    let pool_delegation = if is_null {
        None
    } else {
        let (hash, n) = decode_hash28(&data[off..])?;
        off += n;
        Some(hash)
    };

    // [3] drep_delegation: null | DRep encoding
    let (is_null, n) = decode_null(&data[off..])?;
    off += n;
    let drep_delegation = if is_null {
        None
    } else {
        let (drep, n) = decode_drep(&data[off..])?;
        off += n;
        Some(drep)
    };

    Ok((
        HaskellAccountState {
            balance,
            deposit,
            pool_delegation,
            drep_delegation,
        },
        off,
    ))
}

/// Decode a DRep delegation target.
///
/// - `[0, bytes(28)]` → KeyHash
/// - `[1, bytes(28)]` → ScriptHash
/// - `[2]`            → AlwaysAbstain
/// - `[3]`            → AlwaysNoConfidence
fn decode_drep(data: &[u8]) -> Result<(HaskellDRep, usize), SerializationError> {
    let mut off = 0;

    let (arr_len, n) = decode_array_len(&data[off..])?;
    off += n;

    let (tag, n) = decode_uint(&data[off..])?;
    off += n;

    match tag {
        0 => {
            if arr_len != 2 {
                return Err(SerializationError::CborDecode(format!(
                    "DRep KeyHash: expected array(2), got array({arr_len})"
                )));
            }
            let (hash, n) = decode_hash28(&data[off..])?;
            off += n;
            Ok((HaskellDRep::KeyHash(hash), off))
        }
        1 => {
            if arr_len != 2 {
                return Err(SerializationError::CborDecode(format!(
                    "DRep ScriptHash: expected array(2), got array({arr_len})"
                )));
            }
            let (hash, n) = decode_hash28(&data[off..])?;
            off += n;
            Ok((HaskellDRep::ScriptHash(hash), off))
        }
        2 => {
            if arr_len != 1 {
                return Err(SerializationError::CborDecode(format!(
                    "DRep AlwaysAbstain: expected array(1), got array({arr_len})"
                )));
            }
            Ok((HaskellDRep::AlwaysAbstain, off))
        }
        3 => {
            if arr_len != 1 {
                return Err(SerializationError::CborDecode(format!(
                    "DRep AlwaysNoConfidence: expected array(1), got array({arr_len})"
                )));
            }
            Ok((HaskellDRep::AlwaysNoConfidence, off))
        }
        _ => Err(SerializationError::CborDecode(format!(
            "DRep: unknown tag {tag}"
        ))),
    }
}

// ── Shared helpers ────────────────────────────────────────────────────────────

/// Decode an optional anchor.
///
/// Haskell encodes `StrictMaybe Anchor` in several ways depending on context:
/// - CBOR null (`0xf6`) → `None`
/// - `[]` (array(0)) → `None` (StrictMaybe SNothing)
/// - `[array(2)[url, hash]]` (array(1)) → `Some` (StrictMaybe SJust wrapping the anchor)
/// - `array(2)[url, hash]` directly → `Some` (plain anchor, used in pool metadata)
fn decode_optional_anchor(
    data: &[u8],
) -> Result<(Option<(String, Hash32)>, usize), SerializationError> {
    // Check for CBOR null first.
    let (is_null, n) = decode_null(data)?;
    if is_null {
        return Ok((None, n));
    }

    let mut off = 0;
    let (arr_len, n) = decode_array_len(&data[off..])?;
    off += n;

    match arr_len {
        0 => {
            // StrictMaybe SNothing = empty array
            Ok((None, off))
        }
        1 => {
            // StrictMaybe SJust: unwrap the inner array(2) [url, hash]
            let (inner_len, n) = decode_array_len(&data[off..])?;
            off += n;
            if inner_len != 2 {
                return Err(SerializationError::CborDecode(format!(
                    "Anchor inner: expected array(2), got array({inner_len})"
                )));
            }
            let (url, n) = decode_text(&data[off..])?;
            off += n;
            let (hash, n) = decode_hash32(&data[off..])?;
            off += n;
            Ok((Some((url.to_string(), hash)), off))
        }
        2 => {
            // Plain anchor: array(2) [url, hash] (used for pool metadata)
            let (url, n) = decode_text(&data[off..])?;
            off += n;
            let (hash, n) = decode_hash32(&data[off..])?;
            off += n;
            Ok((Some((url.to_string(), hash)), off))
        }
        _ => Err(SerializationError::CborDecode(format!(
            "Anchor: unexpected array({arr_len})"
        ))),
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

/// Credential-keyed coin map used by instantaneous rewards.
type CredentialCoinMap = HashMap<(u8, Hash28), u64>;

/// Decode a `map(credential → uint)` used by instantaneous rewards.
fn decode_credential_coin_map(
    data: &[u8],
) -> Result<(CredentialCoinMap, usize), SerializationError> {
    let mut off = 0;

    let (mut reader, n) = MapReader::new(&data[off..])?;
    off += n;

    let mut map = HashMap::with_capacity(reader.size_hint());
    while reader.has_next(&data[off..])? {
        let (cred, n) = decode_credential(&data[off..])?;
        off += n;
        let (coin, n) = decode_uint(&data[off..])?;
        off += n;
        map.insert(cred, coin);
    }
    off += reader.finish(&data[off..])?;

    Ok((map, off))
}
