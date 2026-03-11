//! Parser for Haskell PoolParams and PParams CBOR encoding.

use std::collections::HashMap;

use super::parse_tagged_rational;
use super::types::{HaskellPParams, HaskellPoolMetadata, HaskellPoolParams, HaskellRelay};
use crate::error::SerializationError;
use torsten_primitives::hash::{Hash28, Hash32};
use torsten_primitives::value::Lovelace;

/// Parse a single pool's parameters: array(9) matching the Shelley PoolParams encoding.
///
/// Fields:
///   [0] operator: KeyHash28
///   [1] vrf_keyhash: Hash32
///   [2] pledge: Coin
///   [3] cost: Coin
///   [4] margin: Tag(30) [num, den]
///   [5] reward_account: bytes
///   [6] owners: Set of KeyHash28 (encoded as array)
///   [7] relays: StrictSeq of Relay
///   [8] metadata: StrictMaybe PoolMetadata
pub fn parse_pool_params(
    d: &mut minicbor::Decoder,
) -> Result<HaskellPoolParams, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("PoolParams: expected definite array".into())
    })?;
    if len != 9 {
        return Err(SerializationError::CborDecode(format!(
            "PoolParams: expected array(9), got array({len})"
        )));
    }

    // [0] operator: KeyHash28
    let operator = parse_hash28(d, "PoolParams operator")?;

    // [1] vrf_keyhash: Hash32
    let vrf_keyhash = parse_hash32(d, "PoolParams vrf_keyhash")?;

    // [2] pledge: Coin
    let pledge = Lovelace(d.u64()?);

    // [3] cost: Coin
    let cost = Lovelace(d.u64()?);

    // [4] margin: UnitInterval = Tag(30) [num, den]
    let (margin_numerator, margin_denominator) = parse_tagged_rational(d)?;

    // [5] reward_account: raw bytes
    let reward_account = d.bytes()?.to_vec();

    // [6] owners: Set of KeyHash28 (encoded as array)
    let owners = parse_keyhash28_set(d)?;

    // [7] relays: StrictSeq of Relay
    let relays = parse_relays(d)?;

    // [8] metadata: StrictMaybe PoolMetadata
    let metadata = parse_strict_maybe_pool_metadata(d)?;

    Ok(HaskellPoolParams {
        operator,
        vrf_keyhash,
        pledge,
        cost,
        margin_numerator,
        margin_denominator,
        reward_account,
        owners,
        relays,
        metadata,
    })
}

/// Parse the 10-field StakePoolState used in PState's on-disk format.
///
/// Fields:
///   [0] spsVrf: VRFVerKeyHash (32 bytes)
///   [1] spsPledge: Coin
///   [2] spsCost: Coin
///   [3] spsMargin: UnitInterval = Tag(30) [num, den]
///   [4] spsAccountId: Credential Staking (reward account credential)
///   [5] spsOwners: Set(KeyHash)
///   [6] spsRelays: StrictSeq(Relay)
///   [7] spsMetadata: StrictMaybe(PoolMetadata)
///   [8] spsDeposit: CompactForm Coin
///   [9] spsDelegators: Set(Credential)
///
/// Returns a HaskellPoolParams (operator is zeroed, reward_account is empty
/// since these fields are not present in the StakePoolState encoding).
#[allow(dead_code)]
pub fn parse_stake_pool_state(
    d: &mut minicbor::Decoder,
) -> Result<HaskellPoolParams, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("StakePoolState: expected definite array".into())
    })?;
    if len != 10 {
        return Err(SerializationError::CborDecode(format!(
            "StakePoolState: expected array(10), got array({len})"
        )));
    }

    // [0] spsVrf: VRFVerKeyHash (32 bytes)
    let vrf_keyhash = parse_hash32(d, "StakePoolState vrf")?;

    // [1] spsPledge: Coin
    let pledge = Lovelace(d.u64()?);

    // [2] spsCost: Coin
    let cost = Lovelace(d.u64()?);

    // [3] spsMargin: UnitInterval = Tag(30) [num, den]
    let (margin_numerator, margin_denominator) = parse_tagged_rational(d)?;

    // [4] spsAccountId: Credential Staking
    let _account_cred = super::parse_credential(d)?;

    // [5] spsOwners: Set(KeyHash)
    let owners = parse_keyhash28_set(d)?;

    // [6] spsRelays: StrictSeq(Relay)
    let relays = parse_relays(d)?;

    // [7] spsMetadata: StrictMaybe(PoolMetadata)
    let metadata = parse_strict_maybe_pool_metadata(d)?;

    // [8] spsDeposit: CompactForm Coin
    let _deposit = d.u64()?;

    // [9] spsDelegators: Set(Credential) -- encoded as an array
    let delegators_len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("StakePoolState delegators: expected definite array".into())
    })?;
    for _ in 0..delegators_len {
        d.skip()?;
    }

    Ok(HaskellPoolParams {
        operator: Hash28::from_bytes([0u8; 28]),
        vrf_keyhash,
        pledge,
        cost,
        margin_numerator,
        margin_denominator,
        reward_account: Vec::new(),
        owners,
        relays,
        metadata,
    })
}

/// Parse Conway PParams: array(31)
///
/// Fields in order (matching Haskell ConwayPParams EncCBOR):
///   [0]  minFeeA (Natural)
///   [1]  minFeeB (Natural)
///   [2]  maxBlockBodySize (Word32)
///   [3]  maxTxSize (Word32)
///   [4]  maxBlockHeaderSize (Word16)
///   [5]  keyDeposit (Coin)
///   [6]  poolDeposit (Coin)
///   [7]  eMax (EpochInterval)
///   [8]  nOpt (Natural/Word16)
///   [9]  a0 (NonNegativeInterval = Tag(30) [num, den])
///   [10] rho (UnitInterval = Tag(30) [num, den])
///   [11] tau (UnitInterval = Tag(30) [num, den])
///   [12] protocolVersion: array(2) [major, minor]
///   [13] minPoolCost (Coin)
///   [14] adaPerUTxOByte (Coin)
///   [15] costModels (Map)
///   [16] prices: array(2) [mem_price, step_price] each Tag(30)
///   [17] maxTxExUnits: array(2) [mem, steps]
///   [18] maxBlockExUnits: array(2) [mem, steps]
///   [19] maxValSize (Natural)
///   [20] collateralPercentage (Natural)
///   [21] maxCollateralInputs (Natural)
///   [22] poolVotingThresholds: array(5) of Tag(30) rationals
///   [23] dRepVotingThresholds: array(10) of Tag(30) rationals
///   [24] committeeMinSize (Natural)
///   [25] committeeMaxTermLength (EpochInterval)
///   [26] govActionLifetime (EpochInterval)
///   [27] govActionDeposit (Coin)
///   [28] dRepDeposit (Coin)
///   [29] dRepActivity (EpochInterval)
///   [30] minFeeRefScriptCostPerByte (NonNegativeInterval = Tag(30))
pub fn parse_pparams(d: &mut minicbor::Decoder) -> Result<HaskellPParams, SerializationError> {
    let len = d
        .array()?
        .ok_or_else(|| SerializationError::CborDecode("PParams: expected definite array".into()))?;
    if len != 31 {
        return Err(SerializationError::CborDecode(format!(
            "PParams: expected array(31), got array({len})"
        )));
    }

    // [0] minFeeA
    let min_fee_a = d.u64()?;
    // [1] minFeeB
    let min_fee_b = d.u64()?;
    // [2] maxBlockBodySize
    let max_block_body_size = d.u64()?;
    // [3] maxTxSize
    let max_tx_size = d.u64()?;
    // [4] maxBlockHeaderSize
    let max_block_header_size = d.u64()?;
    // [5] keyDeposit
    let key_deposit = d.u64()?;
    // [6] poolDeposit
    let pool_deposit = d.u64()?;
    // [7] eMax
    let e_max = d.u64()?;
    // [8] nOpt
    let n_opt = d.u64()?;
    // [9] a0: Tag(30) [num, den]
    let (a0_num, a0_den) = parse_tagged_rational(d)?;
    // [10] rho: Tag(30) [num, den]
    let (rho_num, rho_den) = parse_tagged_rational(d)?;
    // [11] tau: Tag(30) [num, den]
    let (tau_num, tau_den) = parse_tagged_rational(d)?;
    // [12] protocolVersion: array(2) [major, minor]
    let pv_len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("ProtocolVersion: expected definite array".into())
    })?;
    if pv_len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "ProtocolVersion: expected array(2), got array({pv_len})"
        )));
    }
    let protocol_version_major = d.u64()?;
    let protocol_version_minor = d.u64()?;
    // [13] minPoolCost
    let min_pool_cost = d.u64()?;
    // [14] adaPerUTxOByte
    let ada_per_utxo_byte = d.u64()?;
    // [15] costModels: Map(language_id -> [costs])
    let cost_models = parse_cost_models(d)?;
    // [16] prices: array(2) [memPrice, stepPrice] each Tag(30)
    let prices_len = d
        .array()?
        .ok_or_else(|| SerializationError::CborDecode("Prices: expected definite array".into()))?;
    if prices_len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "Prices: expected array(2), got array({prices_len})"
        )));
    }
    let (prices_mem_num, prices_mem_den) = parse_tagged_rational(d)?;
    let (prices_step_num, prices_step_den) = parse_tagged_rational(d)?;
    // [17] maxTxExUnits: array(2) [mem, steps]
    let tx_eu_len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("MaxTxExUnits: expected definite array".into())
    })?;
    if tx_eu_len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "MaxTxExUnits: expected array(2), got array({tx_eu_len})"
        )));
    }
    let max_tx_ex_units_mem = d.u64()?;
    let max_tx_ex_units_steps = d.u64()?;
    // [18] maxBlockExUnits: array(2) [mem, steps]
    let blk_eu_len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("MaxBlockExUnits: expected definite array".into())
    })?;
    if blk_eu_len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "MaxBlockExUnits: expected array(2), got array({blk_eu_len})"
        )));
    }
    let max_block_ex_units_mem = d.u64()?;
    let max_block_ex_units_steps = d.u64()?;
    // [19] maxValSize
    let max_val_size = d.u64()?;
    // [20] collateralPercentage
    let collateral_percentage = d.u64()?;
    // [21] maxCollateralInputs
    let max_collateral_inputs = d.u64()?;
    // [22] poolVotingThresholds: array(5) of Tag(30) rationals
    let pvt_len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("PoolVotingThresholds: expected definite array".into())
    })?;
    if pvt_len != 5 {
        return Err(SerializationError::CborDecode(format!(
            "PoolVotingThresholds: expected array(5), got array({pvt_len})"
        )));
    }
    let (pvt_motion_no_confidence_num, pvt_motion_no_confidence_den) = parse_tagged_rational(d)?;
    let (pvt_committee_normal_num, pvt_committee_normal_den) = parse_tagged_rational(d)?;
    let (pvt_committee_no_confidence_num, pvt_committee_no_confidence_den) =
        parse_tagged_rational(d)?;
    let (pvt_hard_fork_num, pvt_hard_fork_den) = parse_tagged_rational(d)?;
    let (pvt_pp_security_group_num, pvt_pp_security_group_den) = parse_tagged_rational(d)?;
    // [23] dRepVotingThresholds: array(10) of Tag(30) rationals
    let dvt_len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("DRepVotingThresholds: expected definite array".into())
    })?;
    if dvt_len != 10 {
        return Err(SerializationError::CborDecode(format!(
            "DRepVotingThresholds: expected array(10), got array({dvt_len})"
        )));
    }
    let (dvt_motion_no_confidence_num, dvt_motion_no_confidence_den) = parse_tagged_rational(d)?;
    let (dvt_committee_normal_num, dvt_committee_normal_den) = parse_tagged_rational(d)?;
    let (dvt_committee_no_confidence_num, dvt_committee_no_confidence_den) =
        parse_tagged_rational(d)?;
    let (dvt_update_constitution_num, dvt_update_constitution_den) = parse_tagged_rational(d)?;
    let (dvt_hard_fork_num, dvt_hard_fork_den) = parse_tagged_rational(d)?;
    let (dvt_pp_network_group_num, dvt_pp_network_group_den) = parse_tagged_rational(d)?;
    let (dvt_pp_economic_group_num, dvt_pp_economic_group_den) = parse_tagged_rational(d)?;
    let (dvt_pp_technical_group_num, dvt_pp_technical_group_den) = parse_tagged_rational(d)?;
    let (dvt_pp_gov_group_num, dvt_pp_gov_group_den) = parse_tagged_rational(d)?;
    let (dvt_treasury_withdrawal_num, dvt_treasury_withdrawal_den) = parse_tagged_rational(d)?;
    // [24] committeeMinSize
    let committee_min_size = d.u64()?;
    // [25] committeeMaxTermLength
    let committee_max_term_length = d.u64()?;
    // [26] govActionLifetime
    let gov_action_lifetime = d.u64()?;
    // [27] govActionDeposit
    let gov_action_deposit = d.u64()?;
    // [28] dRepDeposit
    let drep_deposit = d.u64()?;
    // [29] dRepActivity
    let drep_activity = d.u64()?;
    // [30] minFeeRefScriptCostPerByte: Tag(30) [num, den]
    let (min_fee_ref_script_cost_per_byte_num, min_fee_ref_script_cost_per_byte_den) =
        parse_tagged_rational(d)?;

    Ok(HaskellPParams {
        min_fee_a,
        min_fee_b,
        max_block_body_size,
        max_tx_size,
        max_block_header_size,
        key_deposit,
        pool_deposit,
        e_max,
        n_opt,
        a0_num,
        a0_den,
        rho_num,
        rho_den,
        tau_num,
        tau_den,
        protocol_version_major,
        protocol_version_minor,
        min_pool_cost,
        ada_per_utxo_byte,
        cost_models,
        prices_mem_num,
        prices_mem_den,
        prices_step_num,
        prices_step_den,
        max_tx_ex_units_mem,
        max_tx_ex_units_steps,
        max_block_ex_units_mem,
        max_block_ex_units_steps,
        max_val_size,
        collateral_percentage,
        max_collateral_inputs,
        pvt_motion_no_confidence_num,
        pvt_motion_no_confidence_den,
        pvt_committee_normal_num,
        pvt_committee_normal_den,
        pvt_committee_no_confidence_num,
        pvt_committee_no_confidence_den,
        pvt_hard_fork_num,
        pvt_hard_fork_den,
        pvt_pp_security_group_num,
        pvt_pp_security_group_den,
        dvt_motion_no_confidence_num,
        dvt_motion_no_confidence_den,
        dvt_committee_normal_num,
        dvt_committee_normal_den,
        dvt_committee_no_confidence_num,
        dvt_committee_no_confidence_den,
        dvt_update_constitution_num,
        dvt_update_constitution_den,
        dvt_hard_fork_num,
        dvt_hard_fork_den,
        dvt_pp_network_group_num,
        dvt_pp_network_group_den,
        dvt_pp_economic_group_num,
        dvt_pp_economic_group_den,
        dvt_pp_technical_group_num,
        dvt_pp_technical_group_den,
        dvt_pp_gov_group_num,
        dvt_pp_gov_group_den,
        dvt_treasury_withdrawal_num,
        dvt_treasury_withdrawal_den,
        committee_min_size,
        committee_max_term_length,
        gov_action_lifetime,
        gov_action_deposit,
        drep_deposit,
        drep_activity,
        min_fee_ref_script_cost_per_byte_num,
        min_fee_ref_script_cost_per_byte_den,
    })
}

// ---------------------------------------------------------------------------
// Helper parsers
// ---------------------------------------------------------------------------

/// Parse a 28-byte hash from CBOR bytes.
fn parse_hash28(d: &mut minicbor::Decoder, ctx: &str) -> Result<Hash28, SerializationError> {
    let bytes = d.bytes()?;
    if bytes.len() != 28 {
        return Err(SerializationError::CborDecode(format!(
            "{ctx}: expected 28 bytes, got {}",
            bytes.len()
        )));
    }
    let mut hash = [0u8; 28];
    hash.copy_from_slice(bytes);
    Ok(Hash28::from_bytes(hash))
}

/// Parse a 32-byte hash from CBOR bytes.
fn parse_hash32(d: &mut minicbor::Decoder, ctx: &str) -> Result<Hash32, SerializationError> {
    let bytes = d.bytes()?;
    if bytes.len() != 32 {
        return Err(SerializationError::CborDecode(format!(
            "{ctx}: expected 32 bytes, got {}",
            bytes.len()
        )));
    }
    let mut hash = [0u8; 32];
    hash.copy_from_slice(bytes);
    Ok(Hash32::from_bytes(hash))
}

/// Parse a set of KeyHash28 values, encoded as a CBOR array.
fn parse_keyhash28_set(d: &mut minicbor::Decoder) -> Result<Vec<Hash28>, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("KeyHash28 set: expected definite array".into())
    })?;
    let mut result = Vec::with_capacity(len as usize);
    for _ in 0..len {
        result.push(parse_hash28(d, "KeyHash28 set element")?);
    }
    Ok(result)
}

/// Parse a StrictSeq of Relay values.
///
/// Relay is a sum type encoded as:
///   SingleHostAddr (tag=0): array(4) [0, port_or_null, ipv4_or_null, ipv6_or_null]
///   SingleHostName (tag=1): array(3) [1, port_or_null, dns_name]
///   MultiHostName  (tag=2): array(2) [2, dns_name]
fn parse_relays(d: &mut minicbor::Decoder) -> Result<Vec<HaskellRelay>, SerializationError> {
    let len = d
        .array()?
        .ok_or_else(|| SerializationError::CborDecode("Relays: expected definite array".into()))?;
    let mut result = Vec::with_capacity(len as usize);
    for _ in 0..len {
        result.push(parse_relay(d)?);
    }
    Ok(result)
}

/// Parse a single Relay value.
fn parse_relay(d: &mut minicbor::Decoder) -> Result<HaskellRelay, SerializationError> {
    let len = d
        .array()?
        .ok_or_else(|| SerializationError::CborDecode("Relay: expected definite array".into()))?;
    let tag = d.u32()?;

    match (tag, len) {
        (0, 4) => {
            // SingleHostAddr: [0, port_or_null, ipv4_or_null, ipv6_or_null]
            let port = parse_nullable_u16(d)?;
            let ipv4 = parse_nullable_ipv4(d)?;
            let ipv6 = parse_nullable_ipv6(d)?;
            Ok(HaskellRelay::SingleHostAddr { port, ipv4, ipv6 })
        }
        (1, 3) => {
            // SingleHostName: [1, port_or_null, dns_name]
            let port = parse_nullable_u16(d)?;
            let dns_name = d.str()?.to_string();
            Ok(HaskellRelay::SingleHostName { port, dns_name })
        }
        (2, 2) => {
            // MultiHostName: [2, dns_name]
            let dns_name = d.str()?.to_string();
            Ok(HaskellRelay::MultiHostName { dns_name })
        }
        _ => Err(SerializationError::CborDecode(format!(
            "Relay: unexpected tag={tag} with array length={len}"
        ))),
    }
}

/// Parse a CBOR null-or-u16 (StrictMaybe Port via encodeNullStrictMaybe).
fn parse_nullable_u16(d: &mut minicbor::Decoder) -> Result<Option<u16>, SerializationError> {
    if d.datatype()? == minicbor::data::Type::Null {
        d.null()?;
        Ok(None)
    } else {
        Ok(Some(d.u16()?))
    }
}

/// Parse a CBOR null-or-ipv4 (StrictMaybe IPv4 via encodeNullStrictMaybe).
/// IPv4 is encoded as 4 raw bytes.
fn parse_nullable_ipv4(d: &mut minicbor::Decoder) -> Result<Option<[u8; 4]>, SerializationError> {
    if d.datatype()? == minicbor::data::Type::Null {
        d.null()?;
        Ok(None)
    } else {
        let bytes = d.bytes()?;
        if bytes.len() != 4 {
            return Err(SerializationError::CborDecode(format!(
                "IPv4: expected 4 bytes, got {}",
                bytes.len()
            )));
        }
        let mut addr = [0u8; 4];
        addr.copy_from_slice(bytes);
        Ok(Some(addr))
    }
}

/// Parse a CBOR null-or-ipv6 (StrictMaybe IPv6 via encodeNullStrictMaybe).
/// IPv6 is encoded as 16 raw bytes.
fn parse_nullable_ipv6(d: &mut minicbor::Decoder) -> Result<Option<[u8; 16]>, SerializationError> {
    if d.datatype()? == minicbor::data::Type::Null {
        d.null()?;
        Ok(None)
    } else {
        let bytes = d.bytes()?;
        if bytes.len() != 16 {
            return Err(SerializationError::CborDecode(format!(
                "IPv6: expected 16 bytes, got {}",
                bytes.len()
            )));
        }
        let mut addr = [0u8; 16];
        addr.copy_from_slice(bytes);
        Ok(Some(addr))
    }
}

/// Parse StrictMaybe PoolMetadata.
///
/// Encoded as:
///   SNothing: array(0)
///   SJust:    array(1) followed by PoolMetadata = array(2) [url, hash]
fn parse_strict_maybe_pool_metadata(
    d: &mut minicbor::Decoder,
) -> Result<Option<HaskellPoolMetadata>, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("StrictMaybe PoolMetadata: expected definite array".into())
    })?;
    match len {
        0 => Ok(None),
        1 => Ok(Some(parse_pool_metadata(d)?)),
        _ => Err(SerializationError::CborDecode(format!(
            "StrictMaybe PoolMetadata: expected array(0) or array(1), got array({len})"
        ))),
    }
}

/// Parse PoolMetadata: array(2) [url(text), hash(bytes32)]
fn parse_pool_metadata(
    d: &mut minicbor::Decoder,
) -> Result<HaskellPoolMetadata, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("PoolMetadata: expected definite array".into())
    })?;
    if len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "PoolMetadata: expected array(2), got array({len})"
        )));
    }
    let url = d.str()?.to_string();
    let hash = parse_hash32(d, "PoolMetadata hash")?;
    Ok(HaskellPoolMetadata { url, hash })
}

/// Parse CostModels: Map(language_id -> [costs]).
///
/// Language keys: 0=PlutusV1, 1=PlutusV2, 2=PlutusV3.
fn parse_cost_models(
    d: &mut minicbor::Decoder,
) -> Result<HashMap<u8, Vec<i64>>, SerializationError> {
    let len = d.map()?.ok_or_else(|| {
        SerializationError::CborDecode("CostModels: expected definite map".into())
    })?;
    let mut result = HashMap::new();
    for _ in 0..len {
        let lang = d.u8()?;
        let costs_len = d.array()?.ok_or_else(|| {
            SerializationError::CborDecode("CostModel: expected definite array".into())
        })?;
        let mut costs = Vec::with_capacity(costs_len as usize);
        for _ in 0..costs_len {
            costs.push(d.i64()?);
        }
        result.insert(lang, costs);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode CBOR into a Vec using a closure that receives the encoder.
    fn cbor_encode(f: impl FnOnce(&mut minicbor::Encoder<&mut Vec<u8>>)) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut enc = minicbor::Encoder::new(&mut buf);
            f(&mut enc);
        }
        buf
    }

    /// Encode a Tag(30) rational into a buffer.
    fn encode_tagged_rational(enc: &mut minicbor::Encoder<&mut Vec<u8>>, num: u64, den: u64) {
        enc.tag(minicbor::data::Tag::new(30)).unwrap();
        enc.array(2).unwrap();
        enc.u64(num).unwrap();
        enc.u64(den).unwrap();
    }

    /// Encode a nullable port (CBOR null or u16).
    fn encode_nullable_port(enc: &mut minicbor::Encoder<&mut Vec<u8>>, port: Option<u16>) {
        match port {
            Some(p) => {
                enc.u16(p).unwrap();
            }
            None => {
                enc.null().unwrap();
            }
        }
    }

    /// Encode a nullable IPv4 (CBOR null or 4 bytes).
    fn encode_nullable_ipv4(enc: &mut minicbor::Encoder<&mut Vec<u8>>, ipv4: Option<[u8; 4]>) {
        match ipv4 {
            Some(a) => {
                enc.bytes(&a).unwrap();
            }
            None => {
                enc.null().unwrap();
            }
        }
    }

    /// Encode a nullable IPv6 (CBOR null or 16 bytes).
    fn encode_nullable_ipv6(enc: &mut minicbor::Encoder<&mut Vec<u8>>, ipv6: Option<[u8; 16]>) {
        match ipv6 {
            Some(a) => {
                enc.bytes(&a).unwrap();
            }
            None => {
                enc.null().unwrap();
            }
        }
    }

    /// Build a complete CBOR-encoded PoolParams array(9) for testing.
    /// Uses a struct-like approach to avoid too-many-arguments.
    struct PoolParamsInput<'a> {
        operator: &'a [u8; 28],
        vrf: &'a [u8; 32],
        pledge: u64,
        cost: u64,
        margin_num: u64,
        margin_den: u64,
        reward_account: &'a [u8],
        owners: &'a [[u8; 28]],
        relays_raw: &'a [Vec<u8>],
        metadata: Option<(&'a str, &'a [u8; 32])>,
    }

    fn build_pool_params_cbor(input: &PoolParamsInput) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.array(9).unwrap();
            enc.bytes(input.operator).unwrap();
            enc.bytes(input.vrf).unwrap();
            enc.u64(input.pledge).unwrap();
            enc.u64(input.cost).unwrap();
            encode_tagged_rational(&mut enc, input.margin_num, input.margin_den);
            enc.bytes(input.reward_account).unwrap();
            enc.array(input.owners.len() as u64).unwrap();
            for o in input.owners {
                enc.bytes(o).unwrap();
            }
            enc.array(input.relays_raw.len() as u64).unwrap();
        }
        for r in input.relays_raw {
            buf.extend_from_slice(r);
        }
        {
            let mut enc = minicbor::Encoder::new(&mut buf);
            match input.metadata {
                None => {
                    enc.array(0).unwrap();
                }
                Some((url, hash)) => {
                    enc.array(1).unwrap();
                    enc.array(2).unwrap();
                    enc.str(url).unwrap();
                    enc.bytes(hash).unwrap();
                }
            }
        }
        buf
    }

    /// Encode a SingleHostAddr relay to raw CBOR.
    fn encode_relay_single_host_addr(
        port: Option<u16>,
        ipv4: Option<[u8; 4]>,
        ipv6: Option<[u8; 16]>,
    ) -> Vec<u8> {
        cbor_encode(|enc| {
            enc.array(4).unwrap();
            enc.u32(0).unwrap();
            encode_nullable_port(enc, port);
            encode_nullable_ipv4(enc, ipv4);
            encode_nullable_ipv6(enc, ipv6);
        })
    }

    #[test]
    fn test_parse_pool_params_basic() {
        let relay = encode_relay_single_host_addr(Some(3001), Some([127, 0, 0, 1]), None);
        let buf = build_pool_params_cbor(&PoolParamsInput {
            operator: &[0xAA; 28],
            vrf: &[0xBB; 32],
            pledge: 1_000_000,
            cost: 340_000_000,
            margin_num: 1,
            margin_den: 100,
            reward_account: &[0xE0, 0xCC, 0xCC],
            owners: &[[0xDD; 28]],
            relays_raw: &[relay],
            metadata: None,
        });

        let mut decoder = minicbor::Decoder::new(&buf);
        let result = parse_pool_params(&mut decoder).unwrap();

        assert_eq!(result.operator, Hash28::from_bytes([0xAA; 28]));
        assert_eq!(result.vrf_keyhash, Hash32::from_bytes([0xBB; 32]));
        assert_eq!(result.pledge, Lovelace(1_000_000));
        assert_eq!(result.cost, Lovelace(340_000_000));
        assert_eq!(result.margin_numerator, 1);
        assert_eq!(result.margin_denominator, 100);
        assert_eq!(result.reward_account, vec![0xE0, 0xCC, 0xCC]);
        assert_eq!(result.owners.len(), 1);
        assert_eq!(result.owners[0], Hash28::from_bytes([0xDD; 28]));
        assert_eq!(result.relays.len(), 1);
        match &result.relays[0] {
            HaskellRelay::SingleHostAddr { port, ipv4, ipv6 } => {
                assert_eq!(*port, Some(3001));
                assert_eq!(*ipv4, Some([127, 0, 0, 1]));
                assert!(ipv6.is_none());
            }
            _ => panic!("expected SingleHostAddr"),
        }
        assert!(result.metadata.is_none());
    }

    #[test]
    fn test_parse_pool_params_with_metadata() {
        let buf = build_pool_params_cbor(&PoolParamsInput {
            operator: &[0x11; 28],
            vrf: &[0x22; 32],
            pledge: 500_000_000,
            cost: 170_000_000,
            margin_num: 3,
            margin_den: 200,
            reward_account: &[0xE1, 0x33],
            owners: &[],
            relays_raw: &[],
            metadata: Some(("https://example.com/pool.json", &[0x44; 32])),
        });

        let mut decoder = minicbor::Decoder::new(&buf);
        let result = parse_pool_params(&mut decoder).unwrap();

        assert_eq!(result.pledge, Lovelace(500_000_000));
        assert_eq!(result.margin_numerator, 3);
        assert_eq!(result.margin_denominator, 200);
        assert!(result.owners.is_empty());
        assert!(result.relays.is_empty());
        let meta = result.metadata.unwrap();
        assert_eq!(meta.url, "https://example.com/pool.json");
        assert_eq!(meta.hash, Hash32::from_bytes([0x44; 32]));
    }

    #[test]
    fn test_parse_pool_params_multiple_relays() {
        let relay_sha = encode_relay_single_host_addr(Some(6000), None, None);

        // SingleHostName relay
        let relay_shn = cbor_encode(|enc| {
            enc.array(3).unwrap();
            enc.u32(1).unwrap();
            enc.null().unwrap();
            enc.str("relay.example.com").unwrap();
        });

        // MultiHostName relay
        let relay_mhn = cbor_encode(|enc| {
            enc.array(2).unwrap();
            enc.u32(2).unwrap();
            enc.str("_srv.example.com").unwrap();
        });

        let buf = build_pool_params_cbor(&PoolParamsInput {
            operator: &[0x55; 28],
            vrf: &[0x66; 32],
            pledge: 100,
            cost: 200,
            margin_num: 1,
            margin_den: 2,
            reward_account: &[0xE0],
            owners: &[],
            relays_raw: &[relay_sha, relay_shn, relay_mhn],
            metadata: None,
        });

        let mut decoder = minicbor::Decoder::new(&buf);
        let result = parse_pool_params(&mut decoder).unwrap();
        assert_eq!(result.relays.len(), 3);

        match &result.relays[0] {
            HaskellRelay::SingleHostAddr { port, ipv4, ipv6 } => {
                assert_eq!(*port, Some(6000));
                assert!(ipv4.is_none());
                assert!(ipv6.is_none());
            }
            _ => panic!("expected SingleHostAddr"),
        }
        match &result.relays[1] {
            HaskellRelay::SingleHostName { port, dns_name } => {
                assert!(port.is_none());
                assert_eq!(dns_name, "relay.example.com");
            }
            _ => panic!("expected SingleHostName"),
        }
        match &result.relays[2] {
            HaskellRelay::MultiHostName { dns_name } => {
                assert_eq!(dns_name, "_srv.example.com");
            }
            _ => panic!("expected MultiHostName"),
        }
    }

    #[test]
    fn test_parse_pool_params_wrong_array_len() {
        let buf = cbor_encode(|enc| {
            enc.array(5).unwrap();
        });

        let mut decoder = minicbor::Decoder::new(&buf);
        let err = parse_pool_params(&mut decoder).unwrap_err();
        assert!(err.to_string().contains("expected array(9)"), "got: {err}");
    }

    #[test]
    fn test_parse_relay_single_host_name() {
        let buf = cbor_encode(|enc| {
            enc.array(3).unwrap();
            enc.u32(1).unwrap();
            enc.null().unwrap();
            enc.str("relay.example.com").unwrap();
        });

        let mut decoder = minicbor::Decoder::new(&buf);
        let relay = parse_relay(&mut decoder).unwrap();
        match relay {
            HaskellRelay::SingleHostName { port, dns_name } => {
                assert!(port.is_none());
                assert_eq!(dns_name, "relay.example.com");
            }
            _ => panic!("expected SingleHostName"),
        }
    }

    #[test]
    fn test_parse_relay_multi_host_name() {
        let buf = cbor_encode(|enc| {
            enc.array(2).unwrap();
            enc.u32(2).unwrap();
            enc.str("_srv.example.com").unwrap();
        });

        let mut decoder = minicbor::Decoder::new(&buf);
        let relay = parse_relay(&mut decoder).unwrap();
        match relay {
            HaskellRelay::MultiHostName { dns_name } => {
                assert_eq!(dns_name, "_srv.example.com");
            }
            _ => panic!("expected MultiHostName"),
        }
    }

    #[test]
    fn test_parse_relay_single_host_addr_all_null() {
        let buf = cbor_encode(|enc| {
            enc.array(4).unwrap();
            enc.u32(0).unwrap();
            enc.null().unwrap();
            enc.null().unwrap();
            enc.null().unwrap();
        });

        let mut decoder = minicbor::Decoder::new(&buf);
        let relay = parse_relay(&mut decoder).unwrap();
        match relay {
            HaskellRelay::SingleHostAddr { port, ipv4, ipv6 } => {
                assert!(port.is_none());
                assert!(ipv4.is_none());
                assert!(ipv6.is_none());
            }
            _ => panic!("expected SingleHostAddr"),
        }
    }

    #[test]
    fn test_parse_relay_single_host_addr_with_ipv6() {
        let ipv6_bytes: [u8; 16] = [
            0x20, 0x01, 0x0d, 0xb8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x01,
        ];
        let buf = cbor_encode(|enc| {
            enc.array(4).unwrap();
            enc.u32(0).unwrap();
            enc.u16(3001).unwrap();
            enc.null().unwrap();
            enc.bytes(&ipv6_bytes).unwrap();
        });

        let mut decoder = minicbor::Decoder::new(&buf);
        let relay = parse_relay(&mut decoder).unwrap();
        match relay {
            HaskellRelay::SingleHostAddr { port, ipv4, ipv6 } => {
                assert_eq!(port, Some(3001));
                assert!(ipv4.is_none());
                assert_eq!(ipv6, Some(ipv6_bytes));
            }
            _ => panic!("expected SingleHostAddr"),
        }
    }

    #[test]
    fn test_parse_cost_models() {
        let buf = cbor_encode(|enc| {
            enc.map(2).unwrap();
            enc.u8(0).unwrap();
            enc.array(3).unwrap();
            enc.i64(100).unwrap();
            enc.i64(-200).unwrap();
            enc.i64(300).unwrap();
            enc.u8(1).unwrap();
            enc.array(2).unwrap();
            enc.i64(50).unwrap();
            enc.i64(60).unwrap();
        });

        let mut decoder = minicbor::Decoder::new(&buf);
        let cm = super::parse_cost_models(&mut decoder).unwrap();
        assert_eq!(cm.len(), 2);
        assert_eq!(cm[&0], vec![100, -200, 300]);
        assert_eq!(cm[&1], vec![50, 60]);
    }

    #[test]
    fn test_parse_pool_metadata() {
        let buf = cbor_encode(|enc| {
            enc.array(2).unwrap();
            enc.str("https://example.com/meta.json").unwrap();
            enc.bytes(&[0xFF; 32]).unwrap();
        });

        let mut decoder = minicbor::Decoder::new(&buf);
        let meta = parse_pool_metadata(&mut decoder).unwrap();
        assert_eq!(meta.url, "https://example.com/meta.json");
        assert_eq!(meta.hash, Hash32::from_bytes([0xFF; 32]));
    }

    #[test]
    fn test_parse_strict_maybe_pool_metadata_nothing() {
        let buf = cbor_encode(|enc| {
            enc.array(0).unwrap();
        });

        let mut decoder = minicbor::Decoder::new(&buf);
        let result = parse_strict_maybe_pool_metadata(&mut decoder).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_strict_maybe_pool_metadata_just() {
        let buf = cbor_encode(|enc| {
            enc.array(1).unwrap();
            enc.array(2).unwrap();
            enc.str("https://pool.io/m.json").unwrap();
            enc.bytes(&[0xAB; 32]).unwrap();
        });

        let mut decoder = minicbor::Decoder::new(&buf);
        let result = parse_strict_maybe_pool_metadata(&mut decoder).unwrap();
        let meta = result.unwrap();
        assert_eq!(meta.url, "https://pool.io/m.json");
        assert_eq!(meta.hash, Hash32::from_bytes([0xAB; 32]));
    }

    #[test]
    fn test_parse_pool_params_multiple_owners() {
        let buf = build_pool_params_cbor(&PoolParamsInput {
            operator: &[0x01; 28],
            vrf: &[0x02; 32],
            pledge: 0,
            cost: 0,
            margin_num: 0,
            margin_den: 1,
            reward_account: &[0xE0],
            owners: &[[0xA1; 28], [0xA2; 28], [0xA3; 28]],
            relays_raw: &[],
            metadata: None,
        });

        let mut decoder = minicbor::Decoder::new(&buf);
        let result = parse_pool_params(&mut decoder).unwrap();
        assert_eq!(result.owners.len(), 3);
        assert_eq!(result.owners[0], Hash28::from_bytes([0xA1; 28]));
        assert_eq!(result.owners[1], Hash28::from_bytes([0xA2; 28]));
        assert_eq!(result.owners[2], Hash28::from_bytes([0xA3; 28]));
    }
}
