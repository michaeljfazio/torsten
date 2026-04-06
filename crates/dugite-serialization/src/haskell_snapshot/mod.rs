pub mod cbor_utils;
pub mod certstate;
pub mod govstate;
pub mod pparams;
pub mod praos;
pub mod snapshots;
pub mod types;

pub use certstate::decode_certstate;
pub use govstate::decode_govstate;
pub use pparams::{decode_cost_models, decode_min_fee_ref_script, decode_pparams};
pub use praos::decode_praos_state;
pub use snapshots::decode_snapshots;
pub use types::*;

use crate::error::SerializationError;
use cbor_utils::{
    decode_array_len, decode_credential, decode_hash28, decode_hash32, decode_rational,
    decode_uint, decode_with_origin_len, skip_cbor_value, MapReader,
};
use dugite_primitives::hash::{Hash28, Hash32};
use dugite_primitives::time::{EpochNo, SlotNo};
use std::collections::HashMap;

/// Decode a complete Haskell `ExtLedgerState` state file (e.g. from a Mithril
/// ancillary archive or cardano-node `--save-state` dump).
///
/// The outer structure is:
/// ```text
/// state_file = array(2) [version=1, ExtLedgerState]
/// ExtLedgerState = array(2) [HFC_Ledger_Telescope, HeaderState]
/// ```
///
/// This function navigates the HFC telescope structures to reach the Conway-era
/// `ShelleyLedgerState` and `PraosState`, then calls the sub-decoders from
/// Tasks 2-6 to populate a [`HaskellLedgerState`].
pub fn decode_state_file(data: &[u8]) -> Result<HaskellLedgerState, SerializationError> {
    let mut off = 0;

    // ── Outer wrapper: array(2) [version, ExtLedgerState] ──────────────────
    let (outer_len, n) = decode_array_len(data)?;
    off += n;
    if outer_len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "state file: expected array(2), got array({outer_len})"
        )));
    }

    // Version must be 1.
    let (version, n) = decode_uint(&data[off..])?;
    off += n;
    if version != 1 {
        return Err(SerializationError::CborDecode(format!(
            "state file: expected version 1, got {version}"
        )));
    }

    // ── ExtLedgerState: array(2) [ledger_telescope, header_state] ──────────
    let (ext_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if ext_len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "ExtLedgerState: expected array(2), got array({ext_len})"
        )));
    }

    // ── Ledger telescope ───────────────────────────────────────────────────
    let (tip_slot, tip_block_no, tip_hash, epoch, new_epoch_state, n) =
        decode_ledger_telescope(&data[off..])?;
    off += n;

    // ── Header state: array(2) [WithOrigin(AnnTip), consensus_telescope] ──
    let (praos_state, n) = decode_header_state(&data[off..])?;
    off += n;

    // Sanity: we should have consumed exactly all the data.
    if off != data.len() {
        // Not fatal — the file may have trailing padding or metadata.
        // Log but do not fail.
    }

    Ok(HaskellLedgerState {
        tip_slot,
        tip_block_no,
        tip_hash,
        epoch,
        new_epoch_state,
        praos_state,
    })
}

// ═══════════════════════════════════════════════════════════════════════════════
// Internal helpers
// ═══════════════════════════════════════════════════════════════════════════════

/// Navigate the HFC ledger telescope to the Conway era and decode the
/// `ShelleyLedgerState` payload.
///
/// Returns `(tip_slot, tip_block_no, tip_hash, epoch, new_epoch_state, bytes_consumed)`.
fn decode_ledger_telescope(
    data: &[u8],
) -> Result<(SlotNo, u64, Hash32, EpochNo, HaskellNewEpochState, usize), SerializationError> {
    let mut off = 0;

    // The telescope is an array(N) where N = era_index + 1.
    // Conway is era index 6, so N=7.
    let (tele_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if tele_len < 1 {
        return Err(SerializationError::CborDecode(
            "ledger telescope: empty array".into(),
        ));
    }

    // Skip past eras (indices 0..N-2): each is array(2) [start_bound, end_bound].
    for i in 0..tele_len - 1 {
        let (past_len, n) = decode_array_len(&data[off..])?;
        off += n;
        if past_len != 2 {
            return Err(SerializationError::CborDecode(format!(
                "ledger telescope past era {i}: expected array(2), got array({past_len})"
            )));
        }
        // Skip both bounds.
        off += skip_cbor_value(&data[off..])?; // start_bound
        off += skip_cbor_value(&data[off..])?; // end_bound
    }

    // Current era (last element): array(2) [start_bound, current_state].
    let (cur_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if cur_len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "ledger telescope current era: expected array(2), got array({cur_len})"
        )));
    }
    // Skip start_bound.
    off += skip_cbor_value(&data[off..])?;

    // current_state = array(2) [shelley_version=2, ShelleyLedgerState]
    let (cs_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if cs_len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "current_state: expected array(2), got array({cs_len})"
        )));
    }
    let (shelley_ver, n) = decode_uint(&data[off..])?;
    off += n;
    if shelley_ver != 2 {
        return Err(SerializationError::CborDecode(format!(
            "ShelleyLedgerState version: expected 2, got {shelley_ver}"
        )));
    }

    // ShelleyLedgerState = array(3|4) [tip, NewEpochState, transition, peras?]
    let (sls_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if sls_len != 3 && sls_len != 4 {
        return Err(SerializationError::CborDecode(format!(
            "ShelleyLedgerState: expected array(3|4), got array({sls_len})"
        )));
    }

    // [0] WithOrigin(ShelleyTip)
    let (tip_slot, tip_block_no, tip_hash, n) = decode_shelley_tip(&data[off..])?;
    off += n;

    // [1] NewEpochState
    let (epoch, new_epoch_state, n) = decode_new_epoch_state(&data[off..])?;
    off += n;

    // [2] shelley_transition: uint — skip
    off += skip_cbor_value(&data[off..])?;

    // [3] peras_round: StrictMaybe — skip if present (array(4) format)
    if sls_len == 4 {
        off += skip_cbor_value(&data[off..])?;
    }

    Ok((
        tip_slot,
        tip_block_no,
        tip_hash,
        epoch,
        new_epoch_state,
        off,
    ))
}

/// Decode `WithOrigin(ShelleyTip)`:
/// - `[]` (array(0)) = Origin → slot 0, block 0, zero hash
/// - `[ShelleyTip]` where ShelleyTip = `array(3) [slot, blockNo, hash]`
fn decode_shelley_tip(data: &[u8]) -> Result<(SlotNo, u64, Hash32, usize), SerializationError> {
    let mut off = 0;
    let (present, n) = decode_with_origin_len(data)?;
    off += n;

    match present {
        None => {
            // Origin — no tip yet.
            Ok((SlotNo(0), 0, Hash32::ZERO, off))
        }
        Some(_) => {
            // Decode inner ShelleyTip = array(3) [slot, blockNo, hash]
            let (tip_len, n) = decode_array_len(&data[off..])?;
            off += n;
            if tip_len != 3 {
                return Err(SerializationError::CborDecode(format!(
                    "ShelleyTip: expected array(3), got array({tip_len})"
                )));
            }
            let (slot, n) = decode_uint(&data[off..])?;
            off += n;
            let (block_no, n) = decode_uint(&data[off..])?;
            off += n;
            let (hash, n) = decode_hash32(&data[off..])?;
            off += n;
            Ok((SlotNo(slot), block_no, hash, off))
        }
    }
}

/// Decode `NewEpochState = array(7) [...]` and return `(epoch, HaskellNewEpochState, consumed)`.
fn decode_new_epoch_state(
    data: &[u8],
) -> Result<(EpochNo, HaskellNewEpochState, usize), SerializationError> {
    let mut off = 0;

    let (nes_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if nes_len != 7 {
        return Err(SerializationError::CborDecode(format!(
            "NewEpochState: expected array(7), got array({nes_len})"
        )));
    }

    // [0] epoch: uint
    let (epoch_val, n) = decode_uint(&data[off..])?;
    off += n;
    let epoch = EpochNo(epoch_val);

    // [1] blocksMadePrev: map(bytes(28) → uint)
    let (blocks_made_prev, n) = decode_blocks_made_map(&data[off..])?;
    off += n;

    // [2] blocksMadeCur: map(bytes(28) → uint)
    let (blocks_made_cur, n) = decode_blocks_made_map(&data[off..])?;
    off += n;

    // [3] EpochState = array(4) [ChainAccountState, LedgerState, SnapShots, NonMyopic]
    let (es_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if es_len != 4 {
        return Err(SerializationError::CborDecode(format!(
            "EpochState: expected array(4), got array({es_len})"
        )));
    }

    // [3][0] ChainAccountState = array(2) [treasury, reserves]
    let (cas_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if cas_len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "ChainAccountState: expected array(2), got array({cas_len})"
        )));
    }
    let (treasury, n) = decode_uint(&data[off..])?;
    off += n;
    let (reserves, n) = decode_uint(&data[off..])?;
    off += n;

    // [3][1] LedgerState = array(2) [CertState, UTxOState]
    let (ls_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if ls_len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "LedgerState: expected array(2), got array({ls_len})"
        )));
    }

    // CertState
    let (cert_state, n) = decode_certstate(&data[off..])?;
    off += n;

    // UTxOState = array(6) [utxo, deposited, fees, GovState, instantStake, donation]
    let (us_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if us_len != 6 {
        return Err(SerializationError::CborDecode(format!(
            "UTxOState: expected array(6), got array({us_len})"
        )));
    }

    // [0] utxo_map — skip (empty in EmptyMK snapshot)
    off += skip_cbor_value(&data[off..])?;

    // [1] deposited: uint
    let (deposited, n) = decode_uint(&data[off..])?;
    off += n;

    // [2] fees: uint
    let (fees, n) = decode_uint(&data[off..])?;
    off += n;

    // [3] GovState (ConwayGovState)
    let (gov_state, n) = decode_govstate(&data[off..])?;
    off += n;

    // [4] instantStake: map(credential → coin)
    let (instant_stake, n) = decode_instant_stake_map(&data[off..])?;
    off += n;

    // [5] donation: uint
    let (donation, n) = decode_uint(&data[off..])?;
    off += n;

    // [3][2] SnapShots
    let (snapshots, n) = decode_snapshots(&data[off..])?;
    off += n;

    // [3][3] NonMyopic — skip (array(2) [map, uint])
    off += skip_cbor_value(&data[off..])?;

    // [4] rewardUpdate: StrictMaybe — [] for none, [content] for some
    off += skip_cbor_value(&data[off..])?;

    // [5] PoolDistr = array(2) [map, total_stake]
    let (pool_distr, pool_distr_total_stake, n) = decode_pool_distr(&data[off..])?;
    off += n;

    // [6] stashedAVVM: null (Conway era)
    off += skip_cbor_value(&data[off..])?;

    // Copy PParams from gov_state into the top-level struct.
    let cur_pparams = gov_state.cur_pparams.clone();
    let prev_pparams = gov_state.prev_pparams.clone();

    let new_epoch_state = HaskellNewEpochState {
        epoch,
        blocks_made_prev,
        blocks_made_cur,
        treasury,
        reserves,
        cur_pparams,
        prev_pparams,
        deposited,
        fees,
        donation,
        cert_state,
        snapshots,
        pool_distr,
        pool_distr_total_stake,
        gov_state,
        instant_stake,
    };

    Ok((epoch, new_epoch_state, off))
}

/// Decode the HeaderState telescope and extract PraosState.
///
/// ```text
/// HeaderState = array(2) [
///   WithOrigin(AnnTip),           // skip
///   HFC_Consensus_Telescope       // navigate to last element → PraosState
/// ]
/// ```
fn decode_header_state(data: &[u8]) -> Result<(HaskellPraosState, usize), SerializationError> {
    let mut off = 0;

    let (hs_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if hs_len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "HeaderState: expected array(2), got array({hs_len})"
        )));
    }

    // [0] WithOrigin(AnnTip) — skip entirely
    off += skip_cbor_value(&data[off..])?;

    // [1] HFC consensus telescope: array(N) [past_eras..., current_era]
    let (tele_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if tele_len < 1 {
        return Err(SerializationError::CborDecode(
            "consensus telescope: empty array".into(),
        ));
    }

    // Skip past eras (0..N-2): each is array(2) [bound, bound].
    for i in 0..tele_len - 1 {
        let (past_len, n) = decode_array_len(&data[off..])?;
        off += n;
        if past_len != 2 {
            return Err(SerializationError::CborDecode(format!(
                "consensus telescope past era {i}: expected array(2), got array({past_len})"
            )));
        }
        off += skip_cbor_value(&data[off..])?;
        off += skip_cbor_value(&data[off..])?;
    }

    // Current era (last): array(2) [bound, PraosState_versioned]
    let (cur_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if cur_len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "consensus telescope current era: expected array(2), got array({cur_len})"
        )));
    }
    // Skip start_bound.
    off += skip_cbor_value(&data[off..])?;

    // Decode PraosState.
    let (praos, n) = decode_praos_state(&data[off..])?;
    off += n;

    Ok((praos, off))
}

/// Decode a `BlocksMade` map: `map(bytes(28) → uint)`.
///
/// Supports both definite-length and indefinite-length CBOR maps (the Haskell
/// node emits indefinite-length maps for BlocksMade).
fn decode_blocks_made_map(
    data: &[u8],
) -> Result<(HashMap<Hash28, u64>, usize), SerializationError> {
    let mut off = 0;
    let (mut reader, n) = MapReader::new(&data[off..])?;
    off += n;

    let mut map = HashMap::with_capacity(reader.size_hint());
    while reader.has_next(&data[off..])? {
        let (hash, n) = decode_hash28(&data[off..])?;
        off += n;
        let (blocks, n) = decode_uint(&data[off..])?;
        off += n;
        map.insert(hash, blocks);
    }
    off += reader.finish(&data[off..])?;

    Ok((map, off))
}

/// Credential-keyed coin map used by instant stake.
type CredentialCoinMap = HashMap<(u8, Hash28), u64>;

/// Decode the `InstantStake` map: `map(credential → uint)`.
///
/// Credential = `array(2) [tag, bytes(28)]`.
/// Supports both definite-length and indefinite-length CBOR maps.
fn decode_instant_stake_map(data: &[u8]) -> Result<(CredentialCoinMap, usize), SerializationError> {
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

/// Decode `PoolDistr = array(2) [map(bytes(28) → IndividualPoolStake), total_stake]`.
///
/// IndividualPoolStake = `array(3) [rational, compact_coin, vrf_hash]`.
fn decode_pool_distr(
    data: &[u8],
) -> Result<(HashMap<Hash28, HaskellPoolDistrEntry>, u64, usize), SerializationError> {
    let mut off = 0;

    let (pd_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if pd_len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "PoolDistr: expected array(2), got array({pd_len})"
        )));
    }

    // The map of pool distributions.
    let (mut reader, n) = MapReader::new(&data[off..])?;
    off += n;

    let mut map = HashMap::with_capacity(reader.size_hint());
    while reader.has_next(&data[off..])? {
        let (pool_id, n) = decode_hash28(&data[off..])?;
        off += n;

        // IndividualPoolStake = array(3) [rational, compact_coin, vrf_hash]
        let (ips_len, n) = decode_array_len(&data[off..])?;
        off += n;
        if ips_len != 3 {
            return Err(SerializationError::CborDecode(format!(
                "IndividualPoolStake: expected array(3), got array({ips_len})"
            )));
        }

        let ((num, den), n) = decode_rational(&data[off..])?;
        off += n;
        let (coin, n) = decode_uint(&data[off..])?;
        off += n;
        let (vrf_hash, n) = decode_hash32(&data[off..])?;
        off += n;

        map.insert(
            pool_id,
            HaskellPoolDistrEntry {
                stake_ratio_num: num,
                stake_ratio_den: den,
                stake_coin: coin,
                vrf_hash,
            },
        );
    }
    off += reader.finish(&data[off..])?;

    // total_stake: uint
    let (total_stake, n) = decode_uint(&data[off..])?;
    off += n;

    Ok((map, total_stake, off))
}

#[cfg(test)]
mod tests;
