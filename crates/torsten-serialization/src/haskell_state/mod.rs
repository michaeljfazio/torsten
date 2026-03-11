//! Parser for Haskell NewEpochState CBOR format (from Mithril snapshots).
//!
//! This module parses the HFC-wrapped CBOR ledger state files produced by
//! cardano-node and converts them into Torsten's LedgerState format.
//! This eliminates the ~384-second block replay after Mithril import.

mod cert_state;
pub mod convert;
mod gov_state;
mod pparams;
mod telescope;
pub mod types;
mod utxo_state;

use std::collections::HashMap;

use crate::error::SerializationError;
use torsten_primitives::hash::Hash28;
use torsten_primitives::time::EpochNo;
use torsten_primitives::value::Lovelace;
use types::HaskellNewEpochState;

/// Parse a raw Haskell ledger state file (HFC-wrapped NewEpochState CBOR)
/// into the intermediate HaskellNewEpochState type.
///
/// The input is the raw bytes from a `<slot>_<hash>` file in the
/// `haskell-ledger/` directory of a Mithril snapshot.
pub fn parse_haskell_new_epoch_state(
    data: &[u8],
) -> Result<HaskellNewEpochState, SerializationError> {
    let mut decoder = minicbor::Decoder::new(data);

    // Unwrap the HFC telescope to get to the Conway-era NewEpochState
    let telescope_info = telescope::unwrap_hfc_telescope(&mut decoder)?;
    if telescope_info.era_index != 6 {
        return Err(SerializationError::CborDecode(format!(
            "expected Conway era (index 6) in HFC telescope, got era index {}",
            telescope_info.era_index
        )));
    }

    // Parse the NewEpochState array(7)
    parse_new_epoch_state(&mut decoder)
}

/// Parse NewEpochState: array(7)
fn parse_new_epoch_state(
    d: &mut minicbor::Decoder,
) -> Result<HaskellNewEpochState, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("NewEpochState: expected definite array".into())
    })?;
    if len != 7 {
        return Err(SerializationError::CborDecode(format!(
            "NewEpochState: expected array(7), got array({len})"
        )));
    }

    // [0] EpochNo
    let epoch_no = EpochNo(d.u64()?);

    // [1] BlocksMade (prev epoch)
    let blocks_made_prev = parse_blocks_made(d)?;

    // [2] BlocksMade (current epoch)
    let blocks_made_cur = parse_blocks_made(d)?;

    // [3] EpochState
    let epoch_state = parse_epoch_state(d)?;

    // [4] StrictMaybe PulsingRewUpdate
    let reward_update = parse_strict_maybe_reward_update(d)?;

    // [5] PoolDistr
    let pool_distr = parse_pool_distr(d)?;

    // [6] StashedAVVMAddresses (unit for Conway)
    // Haskell encodes () as encCBOR () -- which is a CBOR array(0)
    // or sometimes as encodeNull. Handle both.
    skip_stashed_avvm(d)?;

    Ok(HaskellNewEpochState {
        epoch_no,
        blocks_made_prev,
        blocks_made_cur,
        epoch_state,
        reward_update,
        pool_distr,
    })
}

/// Parse BlocksMade: Map(KeyHash28 -> Natural)
fn parse_blocks_made(
    d: &mut minicbor::Decoder,
) -> Result<HashMap<Hash28, u64>, SerializationError> {
    let len = d.map()?.ok_or_else(|| {
        SerializationError::CborDecode("BlocksMade: expected definite map".into())
    })?;
    let mut result = HashMap::with_capacity(len as usize);
    for _ in 0..len {
        let key_bytes = d.bytes()?;
        if key_bytes.len() != 28 {
            return Err(SerializationError::CborDecode(format!(
                "BlocksMade key: expected 28 bytes, got {}",
                key_bytes.len()
            )));
        }
        let mut hash = [0u8; 28];
        hash.copy_from_slice(key_bytes);
        let value = d.u64()?;
        result.insert(Hash28::from_bytes(hash), value);
    }
    Ok(result)
}

/// Parse EpochState: array(4)
fn parse_epoch_state(
    d: &mut minicbor::Decoder,
) -> Result<types::HaskellEpochState, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("EpochState: expected definite array".into())
    })?;
    if len != 4 {
        return Err(SerializationError::CborDecode(format!(
            "EpochState: expected array(4), got array({len})"
        )));
    }

    // [0] ChainAccountState: array(2) [treasury, reserves]
    let acct_len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("ChainAccountState: expected definite array".into())
    })?;
    if acct_len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "ChainAccountState: expected array(2), got array({acct_len})"
        )));
    }
    let treasury = Lovelace(d.u64()?);
    let reserves = Lovelace(d.u64()?);

    // [1] LedgerState: array(2) [CertState, UTxOState] -- NOTE: CertState FIRST
    let ls_len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("LedgerState: expected definite array".into())
    })?;
    if ls_len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "LedgerState: expected array(2), got array({ls_len})"
        )));
    }
    let cert_state = cert_state::parse_cert_state(d)?;
    let utxo_state = utxo_state::parse_utxo_state(d)?;

    let ledger_state = types::HaskellLedgerState {
        cert_state,
        utxo_state,
    };

    // [2] SnapShots: array(4)
    let snapshots = parse_snapshots(d)?;

    // [3] NonMyopic: array(2) -- parse and discard
    let nm_len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("NonMyopic: expected definite array".into())
    })?;
    for _ in 0..nm_len {
        d.skip()?;
    }

    Ok(types::HaskellEpochState {
        treasury,
        reserves,
        ledger_state,
        snapshots,
    })
}

fn parse_snapshots(
    d: &mut minicbor::Decoder,
) -> Result<types::HaskellSnapShots, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("SnapShots: expected definite array".into())
    })?;
    if len != 4 {
        return Err(SerializationError::CborDecode(format!(
            "SnapShots: expected array(4), got array({len})"
        )));
    }

    let mark = parse_snapshot(d)?;
    let set = parse_snapshot(d)?;
    let go = parse_snapshot(d)?;
    let fee = Lovelace(d.u64()?);

    Ok(types::HaskellSnapShots { mark, set, go, fee })
}

fn parse_snapshot(d: &mut minicbor::Decoder) -> Result<types::HaskellSnapShot, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("SnapShot: expected definite array".into())
    })?;

    match len {
        2 => {
            // New format: [ActiveStake(VMap), StakePoolsSnapShot(VMap)]
            // ActiveStake is Map(Credential -> StakeWithDelegation)
            let active_stake = parse_active_stake_map(d)?;
            let pool_params = parse_pool_params_vmap(d)?;

            // Extract stake and delegations from StakeWithDelegation
            let mut stake = HashMap::new();
            let mut delegations = HashMap::new();
            for (cred, (amount, pool_id)) in active_stake {
                stake.insert(cred.clone(), amount);
                delegations.insert(cred, pool_id);
            }

            Ok(types::HaskellSnapShot {
                stake,
                delegations,
                pool_params,
            })
        }
        3 => {
            // Old format: [Stake, Delegations, PoolParams]
            let stake = parse_credential_coin_map(d)?;
            let delegations = parse_credential_keyhash_map(d)?;
            let pool_params = parse_pool_params_vmap(d)?;
            Ok(types::HaskellSnapShot {
                stake,
                delegations,
                pool_params,
            })
        }
        _ => Err(SerializationError::CborDecode(format!(
            "SnapShot: expected array(2) or array(3), got array({len})"
        ))),
    }
}

fn parse_active_stake_map(
    d: &mut minicbor::Decoder,
) -> Result<HashMap<types::HaskellCredential, (Lovelace, Hash28)>, SerializationError> {
    // ActiveStake is a VMap encoded as a map
    let len = d.map()?.ok_or_else(|| {
        SerializationError::CborDecode("ActiveStake: expected definite map".into())
    })?;
    let mut result = HashMap::with_capacity(len as usize);
    for _ in 0..len {
        let cred = parse_credential(d)?;
        // StakeWithDelegation: array(2) [NonZero(CompactCoin), KeyHash]
        let swd_len = d.array()?.ok_or_else(|| {
            SerializationError::CborDecode("StakeWithDelegation: expected definite array".into())
        })?;
        if swd_len != 2 {
            return Err(SerializationError::CborDecode(format!(
                "StakeWithDelegation: expected array(2), got array({swd_len})"
            )));
        }
        let stake = Lovelace(d.u64()?);
        let pool_bytes = d.bytes()?;
        if pool_bytes.len() != 28 {
            return Err(SerializationError::CborDecode(format!(
                "Pool KeyHash: expected 28 bytes, got {}",
                pool_bytes.len()
            )));
        }
        let mut pool_hash = [0u8; 28];
        pool_hash.copy_from_slice(pool_bytes);
        result.insert(cred, (stake, Hash28::from_bytes(pool_hash)));
    }
    Ok(result)
}

fn parse_credential_coin_map(
    d: &mut minicbor::Decoder,
) -> Result<HashMap<types::HaskellCredential, Lovelace>, SerializationError> {
    let len = d.map()?.ok_or_else(|| {
        SerializationError::CborDecode("Credential-Coin map: expected definite map".into())
    })?;
    let mut result = HashMap::with_capacity(len as usize);
    for _ in 0..len {
        let cred = parse_credential(d)?;
        let coin = Lovelace(d.u64()?);
        result.insert(cred, coin);
    }
    Ok(result)
}

fn parse_credential_keyhash_map(
    d: &mut minicbor::Decoder,
) -> Result<HashMap<types::HaskellCredential, Hash28>, SerializationError> {
    let len = d.map()?.ok_or_else(|| {
        SerializationError::CborDecode("Credential-KeyHash map: expected definite map".into())
    })?;
    let mut result = HashMap::with_capacity(len as usize);
    for _ in 0..len {
        let cred = parse_credential(d)?;
        let key_bytes = d.bytes()?;
        if key_bytes.len() != 28 {
            return Err(SerializationError::CborDecode(format!(
                "KeyHash: expected 28 bytes, got {}",
                key_bytes.len()
            )));
        }
        let mut hash = [0u8; 28];
        hash.copy_from_slice(key_bytes);
        result.insert(cred, Hash28::from_bytes(hash));
    }
    Ok(result)
}

fn parse_pool_params_vmap(
    d: &mut minicbor::Decoder,
) -> Result<HashMap<Hash28, types::HaskellPoolParams>, SerializationError> {
    // Could be VMap format (array(2) [keys, values]) or simple map
    let len = d.map()?.ok_or_else(|| {
        SerializationError::CborDecode("PoolParams VMap: expected definite map".into())
    })?;
    let mut result = HashMap::with_capacity(len as usize);
    for _ in 0..len {
        let key_bytes = d.bytes()?;
        if key_bytes.len() != 28 {
            return Err(SerializationError::CborDecode(format!(
                "Pool KeyHash: expected 28 bytes, got {}",
                key_bytes.len()
            )));
        }
        let mut hash = [0u8; 28];
        hash.copy_from_slice(key_bytes);
        let pool = pparams::parse_pool_params(d)?;
        result.insert(Hash28::from_bytes(hash), pool);
    }
    Ok(result)
}

/// Parse a Haskell Credential: array(2) [type_tag, hash_bytes]
pub(crate) fn parse_credential(
    d: &mut minicbor::Decoder,
) -> Result<types::HaskellCredential, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("Credential: expected definite array".into())
    })?;
    if len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "Credential: expected array(2), got array({len})"
        )));
    }
    let tag = d.u32()?;
    let hash_bytes = d.bytes()?;
    if hash_bytes.len() != 28 {
        return Err(SerializationError::CborDecode(format!(
            "Credential hash: expected 28 bytes, got {}",
            hash_bytes.len()
        )));
    }
    let mut hash = [0u8; 28];
    hash.copy_from_slice(hash_bytes);
    match tag {
        0 => Ok(types::HaskellCredential::KeyHash(Hash28::from_bytes(hash))),
        1 => Ok(types::HaskellCredential::ScriptHash(Hash28::from_bytes(
            hash,
        ))),
        _ => Err(SerializationError::CborDecode(format!(
            "Credential: unknown tag {tag}"
        ))),
    }
}

fn parse_strict_maybe_reward_update(
    d: &mut minicbor::Decoder,
) -> Result<Option<types::HaskellRewardUpdate>, SerializationError> {
    // StrictMaybe in Haskell's EncCBOR:
    // SNothing = encodeListLen 0
    // SJust x = encodeListLen 1 <> encCBOR x
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("StrictMaybe: expected definite array".into())
    })?;
    match len {
        0 => Ok(None),
        1 => {
            // PulsingRewUpdate is tag-based sum type
            // For now, skip the complex pulsing/complete structure
            // We only need it if it contains a Complete reward update
            d.skip()?;
            Ok(None) // Skip reward updates for now
        }
        _ => Err(SerializationError::CborDecode(format!(
            "StrictMaybe: expected array(0) or array(1), got array({len})"
        ))),
    }
}

fn parse_pool_distr(
    d: &mut minicbor::Decoder,
) -> Result<types::HaskellPoolDistr, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("PoolDistr: expected definite array".into())
    })?;
    if len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "PoolDistr: expected array(2), got array({len})"
        )));
    }

    // [0] Map(KeyHash28 -> IndividualPoolStake)
    let map_len = d.map()?.ok_or_else(|| {
        SerializationError::CborDecode("PoolDistr map: expected definite map".into())
    })?;
    let mut individual_stakes = HashMap::with_capacity(map_len as usize);
    for _ in 0..map_len {
        let key_bytes = d.bytes()?;
        if key_bytes.len() != 28 {
            return Err(SerializationError::CborDecode(format!(
                "PoolDistr key: expected 28 bytes, got {}",
                key_bytes.len()
            )));
        }
        let mut hash = [0u8; 28];
        hash.copy_from_slice(key_bytes);

        // IndividualPoolStake: array(3)
        let ips_len = d.array()?.ok_or_else(|| {
            SerializationError::CborDecode("IndividualPoolStake: expected definite array".into())
        })?;
        if ips_len != 3 {
            return Err(SerializationError::CborDecode(format!(
                "IndividualPoolStake: expected array(3), got array({ips_len})"
            )));
        }
        // Rational: tag(30) [num, den]
        let (num, den) = parse_tagged_rational(d)?;
        let total_stake = Lovelace(d.u64()?);
        let vrf_bytes = d.bytes()?;
        if vrf_bytes.len() != 32 {
            return Err(SerializationError::CborDecode(format!(
                "VRF hash: expected 32 bytes, got {}",
                vrf_bytes.len()
            )));
        }
        let mut vrf_hash = [0u8; 32];
        vrf_hash.copy_from_slice(vrf_bytes);

        individual_stakes.insert(
            Hash28::from_bytes(hash),
            types::HaskellIndividualPoolStake {
                stake_ratio_num: num,
                stake_ratio_den: den,
                total_stake,
                vrf_hash: torsten_primitives::hash::Hash32::from_bytes(vrf_hash),
            },
        );
    }

    // [1] NonZero Coin (total active stake)
    let total_active_stake = Lovelace(d.u64()?);

    Ok(types::HaskellPoolDistr {
        individual_stakes,
        total_active_stake,
    })
}

/// Parse a Tag(30) rational number
pub(crate) fn parse_tagged_rational(
    d: &mut minicbor::Decoder,
) -> Result<(u64, u64), SerializationError> {
    let tag = d.tag()?;
    if tag != minicbor::data::Tag::new(30) {
        return Err(SerializationError::CborDecode(format!(
            "Expected tag(30) for rational, got tag({})",
            u64::from(tag)
        )));
    }
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("Rational: expected definite array".into())
    })?;
    if len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "Rational: expected array(2), got array({len})"
        )));
    }
    let num = d.u64()?;
    let den = d.u64()?;
    Ok((num, den))
}

fn skip_stashed_avvm(d: &mut minicbor::Decoder) -> Result<(), SerializationError> {
    // Conway: () encoded as some CBOR value -- just skip it
    d.skip()?;
    Ok(())
}
