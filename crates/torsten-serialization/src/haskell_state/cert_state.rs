//! Parser for Haskell CertState CBOR encoding (Conway era).
//!
//! CertState: array(3) [VState, PState, DState]
//! Note: In Conway, VState is encoded FIRST (different from Shelley ordering).

use std::collections::HashMap;

use minicbor::data::Type;
use torsten_primitives::hash::{Hash28, Hash32};
use torsten_primitives::time::EpochNo;
use torsten_primitives::value::Lovelace;

use super::types::{
    HaskellAccountState, HaskellAnchor, HaskellCertState, HaskellCommitteeAuth, HaskellCredential,
    HaskellDRep, HaskellDRepState, HaskellDState, HaskellPState, HaskellVState,
};
use crate::error::SerializationError;

/// Parse CertState: array(3) [VState, PState, DState]
pub fn parse_cert_state(d: &mut minicbor::Decoder) -> Result<HaskellCertState, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("CertState: expected definite array".into())
    })?;
    if len != 3 {
        return Err(SerializationError::CborDecode(format!(
            "CertState: expected array(3), got array({len})"
        )));
    }

    // [0] VState (encoded FIRST in Conway)
    let vstate = parse_vstate(d)?;
    // [1] PState
    let pstate = parse_pstate(d)?;
    // [2] DState
    let dstate = parse_dstate(d)?;

    Ok(HaskellCertState {
        vstate,
        pstate,
        dstate,
    })
}

/// Parse VState: array(3) [vsDReps, vsCommitteeState, vsNumDormantEpochs]
fn parse_vstate(d: &mut minicbor::Decoder) -> Result<HaskellVState, SerializationError> {
    let len = d
        .array()?
        .ok_or_else(|| SerializationError::CborDecode("VState: expected definite array".into()))?;
    if len != 3 {
        return Err(SerializationError::CborDecode(format!(
            "VState: expected array(3), got array({len})"
        )));
    }

    // [0] vsDReps: Map(Credential -> DRepState)
    let dreps = parse_drep_map(d)?;

    // [1] vsCommitteeState: CommitteeState = Map(Credential -> CommitteeAuthorization)
    let committee_state = parse_committee_state(d)?;

    // [2] vsNumDormantEpochs: EpochNo
    let num_dormant_epochs = EpochNo(d.u64()?);

    Ok(HaskellVState {
        dreps,
        committee_state,
        num_dormant_epochs,
    })
}

/// Parse Map(Credential -> DRepState)
fn parse_drep_map(
    d: &mut minicbor::Decoder,
) -> Result<HashMap<HaskellCredential, HaskellDRepState>, SerializationError> {
    let len = d
        .map()?
        .ok_or_else(|| SerializationError::CborDecode("DRep map: expected definite map".into()))?;
    let mut result = HashMap::with_capacity(len as usize);
    for _ in 0..len {
        let cred = super::parse_credential(d)?;
        let state = parse_drep_state(d)?;
        result.insert(cred, state);
    }
    Ok(result)
}

/// Parse DRepState: array(4) [expiry, anchor, deposit, delegators]
fn parse_drep_state(d: &mut minicbor::Decoder) -> Result<HaskellDRepState, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("DRepState: expected definite array".into())
    })?;
    if len != 4 {
        return Err(SerializationError::CborDecode(format!(
            "DRepState: expected array(4), got array({len})"
        )));
    }

    // [0] drepExpiry: EpochNo
    let expiry = EpochNo(d.u64()?);

    // [1] drepAnchor: StrictMaybe(Anchor)
    //     SNothing = encodeListLen 0
    //     SJust anchor = encodeListLen 1 <> encCBOR anchor
    let anchor = parse_strict_maybe_anchor(d)?;

    // [2] drepDeposit: CompactForm Coin (Word64)
    let deposit = Lovelace(d.u64()?);

    // [3] drepDelegs: Set(Credential) — encoded as array of credentials
    let delegators = parse_credential_set(d)?;

    Ok(HaskellDRepState {
        expiry,
        anchor,
        deposit,
        delegators,
    })
}

/// Parse StrictMaybe(Anchor):
///   SNothing = encodeListLen 0
///   SJust anchor = encodeListLen 1 <> encCBOR anchor
fn parse_strict_maybe_anchor(
    d: &mut minicbor::Decoder,
) -> Result<Option<HaskellAnchor>, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("StrictMaybe Anchor: expected definite array".into())
    })?;
    match len {
        0 => Ok(None),
        1 => {
            let anchor = parse_anchor(d)?;
            Ok(Some(anchor))
        }
        _ => Err(SerializationError::CborDecode(format!(
            "StrictMaybe Anchor: expected array(0) or array(1), got array({len})"
        ))),
    }
}

/// Parse Anchor: array(2) [url_text, hash_bytes32]
fn parse_anchor(d: &mut minicbor::Decoder) -> Result<HaskellAnchor, SerializationError> {
    let len = d
        .array()?
        .ok_or_else(|| SerializationError::CborDecode("Anchor: expected definite array".into()))?;
    if len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "Anchor: expected array(2), got array({len})"
        )));
    }
    let url = d.str()?.to_string();
    let hash_bytes = d.bytes()?;
    if hash_bytes.len() != 32 {
        return Err(SerializationError::CborDecode(format!(
            "Anchor hash: expected 32 bytes, got {}",
            hash_bytes.len()
        )));
    }
    let mut hash = [0u8; 32];
    hash.copy_from_slice(hash_bytes);
    Ok(HaskellAnchor {
        url,
        data_hash: Hash32::from_bytes(hash),
    })
}

/// Parse a Set(Credential) encoded as a CBOR array of credentials
fn parse_credential_set(
    d: &mut minicbor::Decoder,
) -> Result<Vec<HaskellCredential>, SerializationError> {
    // Haskell encodes Set as tag(258) around the array, but also sometimes as a plain array.
    // Handle both cases.
    let has_set_tag = d.datatype()? == Type::Tag;
    if has_set_tag {
        let tag = d.tag()?;
        let tag_val = u64::from(tag);
        if tag_val != 258 {
            return Err(SerializationError::CborDecode(format!(
                "Credential set: expected tag(258), got tag({tag_val})"
            )));
        }
    }

    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("Credential set: expected definite array".into())
    })?;
    let mut result = Vec::with_capacity(len as usize);
    for _ in 0..len {
        result.push(super::parse_credential(d)?);
    }
    Ok(result)
}

/// Parse CommitteeState: Map(Credential -> CommitteeAuthorization)
fn parse_committee_state(
    d: &mut minicbor::Decoder,
) -> Result<HashMap<HaskellCredential, HaskellCommitteeAuth>, SerializationError> {
    let len = d.map()?.ok_or_else(|| {
        SerializationError::CborDecode("CommitteeState: expected definite map".into())
    })?;
    let mut result = HashMap::with_capacity(len as usize);
    for _ in 0..len {
        let cred = super::parse_credential(d)?;
        let auth = parse_committee_authorization(d)?;
        result.insert(cred, auth);
    }
    Ok(result)
}

/// Parse CommitteeAuthorization: tagged sum type
///   tag 0: CommitteeHotCredential(Credential) — array(2) [tag, credential] after the CBOR tag
///   tag 1: CommitteeMemberResigned(StrictMaybe Anchor)
fn parse_committee_authorization(
    d: &mut minicbor::Decoder,
) -> Result<HaskellCommitteeAuth, SerializationError> {
    // Haskell EncCBOR for sum types: encodeListLen (1 + arity) <> encodeWord tag <> fields
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("CommitteeAuthorization: expected definite array".into())
    })?;
    let tag = d.u32()?;

    match tag {
        0 => {
            // CommitteeHotCredential(Credential)
            if len != 2 {
                return Err(SerializationError::CborDecode(format!(
                    "CommitteeHotCredential: expected array(2), got array({len})"
                )));
            }
            let hot_cred = super::parse_credential(d)?;
            Ok(HaskellCommitteeAuth::HotCredential(hot_cred))
        }
        1 => {
            // CommitteeMemberResigned(StrictMaybe Anchor)
            if len != 2 {
                return Err(SerializationError::CborDecode(format!(
                    "CommitteeMemberResigned: expected array(2), got array({len})"
                )));
            }
            let anchor = parse_strict_maybe_anchor(d)?;
            Ok(HaskellCommitteeAuth::Resigned(anchor))
        }
        _ => Err(SerializationError::CborDecode(format!(
            "CommitteeAuthorization: unknown tag {tag}"
        ))),
    }
}

/// Parse PState: array(4) [stakePoolParams, futurePoolParams, retiring, deposits]
fn parse_pstate(d: &mut minicbor::Decoder) -> Result<HaskellPState, SerializationError> {
    let len = d
        .array()?
        .ok_or_else(|| SerializationError::CborDecode("PState: expected definite array".into()))?;
    if len != 4 {
        return Err(SerializationError::CborDecode(format!(
            "PState: expected array(4), got array({len})"
        )));
    }

    // [0] psStakePoolParams: Map(KeyHash28 -> PoolParams)
    let stake_pool_params = parse_keyhash_pool_params_map(d)?;

    // [1] psFutureStakePoolParams: Map(KeyHash28 -> PoolParams)
    let future_pool_params = parse_keyhash_pool_params_map(d)?;

    // [2] psRetiring: Map(KeyHash28 -> EpochNo)
    let retiring = parse_keyhash_epoch_map(d)?;

    // [3] psDeposits: Map(KeyHash28 -> Coin)
    let deposits = parse_keyhash_coin_map(d)?;

    Ok(HaskellPState {
        stake_pool_params,
        future_pool_params,
        retiring,
        deposits,
    })
}

/// Parse Map(KeyHash28 -> PoolParams)
fn parse_keyhash_pool_params_map(
    d: &mut minicbor::Decoder,
) -> Result<HashMap<Hash28, super::types::HaskellPoolParams>, SerializationError> {
    let len = d.map()?.ok_or_else(|| {
        SerializationError::CborDecode("KeyHash-PoolParams map: expected definite map".into())
    })?;
    let mut result = HashMap::with_capacity(len as usize);
    for _ in 0..len {
        let hash = parse_keyhash28(d)?;
        let pool = super::pparams::parse_pool_params(d)?;
        result.insert(hash, pool);
    }
    Ok(result)
}

/// Parse Map(KeyHash28 -> EpochNo)
fn parse_keyhash_epoch_map(
    d: &mut minicbor::Decoder,
) -> Result<HashMap<Hash28, EpochNo>, SerializationError> {
    let len = d.map()?.ok_or_else(|| {
        SerializationError::CborDecode("KeyHash-Epoch map: expected definite map".into())
    })?;
    let mut result = HashMap::with_capacity(len as usize);
    for _ in 0..len {
        let hash = parse_keyhash28(d)?;
        let epoch = EpochNo(d.u64()?);
        result.insert(hash, epoch);
    }
    Ok(result)
}

/// Parse Map(KeyHash28 -> Coin)
fn parse_keyhash_coin_map(
    d: &mut minicbor::Decoder,
) -> Result<HashMap<Hash28, Lovelace>, SerializationError> {
    let len = d.map()?.ok_or_else(|| {
        SerializationError::CborDecode("KeyHash-Coin map: expected definite map".into())
    })?;
    let mut result = HashMap::with_capacity(len as usize);
    for _ in 0..len {
        let hash = parse_keyhash28(d)?;
        let coin = Lovelace(d.u64()?);
        result.insert(hash, coin);
    }
    Ok(result)
}

/// Parse a 28-byte key hash from CBOR bytes
fn parse_keyhash28(d: &mut minicbor::Decoder) -> Result<Hash28, SerializationError> {
    let key_bytes = d.bytes()?;
    if key_bytes.len() != 28 {
        return Err(SerializationError::CborDecode(format!(
            "KeyHash: expected 28 bytes, got {}",
            key_bytes.len()
        )));
    }
    let mut hash = [0u8; 28];
    hash.copy_from_slice(key_bytes);
    Ok(Hash28::from_bytes(hash))
}

/// Parse DState: array(4) [accounts, futureGenDelegs, genDelegs, iRewards]
fn parse_dstate(d: &mut minicbor::Decoder) -> Result<HaskellDState, SerializationError> {
    let len = d
        .array()?
        .ok_or_else(|| SerializationError::CborDecode("DState: expected definite array".into()))?;
    if len != 4 {
        return Err(SerializationError::CborDecode(format!(
            "DState: expected array(4), got array({len})"
        )));
    }

    // [0] dsAccounts: Map(Credential -> ConwayAccountState)
    let accounts = parse_accounts_map(d)?;

    // [1] dsFutureGenDelegs: Map — always empty in Conway, skip
    d.skip()?;

    // [2] dsGenDelegs: GenDelegs — always empty in Conway, skip
    d.skip()?;

    // [3] dsIRewards: InstantaneousRewards — always empty in Conway, skip
    d.skip()?;

    Ok(HaskellDState { accounts })
}

/// Parse Map(Credential -> ConwayAccountState)
fn parse_accounts_map(
    d: &mut minicbor::Decoder,
) -> Result<HashMap<HaskellCredential, HaskellAccountState>, SerializationError> {
    let len = d.map()?.ok_or_else(|| {
        SerializationError::CborDecode("DState accounts: expected definite map".into())
    })?;
    let mut result = HashMap::with_capacity(len as usize);
    for _ in 0..len {
        let cred = super::parse_credential(d)?;
        let account = parse_conway_account_state(d)?;
        result.insert(cred, account);
    }
    Ok(result)
}

/// Parse ConwayAccountState: array(4) [balance, deposit, poolDelegation, drepDelegation]
fn parse_conway_account_state(
    d: &mut minicbor::Decoder,
) -> Result<HaskellAccountState, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("ConwayAccountState: expected definite array".into())
    })?;
    if len != 4 {
        return Err(SerializationError::CborDecode(format!(
            "ConwayAccountState: expected array(4), got array({len})"
        )));
    }

    // [0] casBalance: CompactForm Coin (Word64)
    let rewards = Lovelace(d.u64()?);

    // [1] casDeposit: CompactForm Coin (Word64)
    let deposit = Lovelace(d.u64()?);

    // [2] casStakePoolDelegation: NullStrictMaybe(KeyHash) — CBOR null or value
    let pool_delegation = parse_null_strict_maybe_keyhash(d)?;

    // [3] casDRepDelegation: NullStrictMaybe(DRep) — CBOR null or value
    let drep_delegation = parse_null_strict_maybe_drep(d)?;

    Ok(HaskellAccountState {
        rewards,
        deposit,
        pool_delegation,
        drep_delegation,
    })
}

/// Parse NullStrictMaybe(KeyHash28): CBOR null for SNothing, or 28-byte hash for SJust
fn parse_null_strict_maybe_keyhash(
    d: &mut minicbor::Decoder,
) -> Result<Option<Hash28>, SerializationError> {
    if d.datatype()? == Type::Null {
        d.null()?;
        return Ok(None);
    }
    let hash = parse_keyhash28(d)?;
    Ok(Some(hash))
}

/// Parse NullStrictMaybe(DRep): CBOR null for SNothing, or DRep encoding for SJust
fn parse_null_strict_maybe_drep(
    d: &mut minicbor::Decoder,
) -> Result<Option<HaskellDRep>, SerializationError> {
    if d.datatype()? == Type::Null {
        d.null()?;
        return Ok(None);
    }
    let drep = parse_drep(d)?;
    Ok(Some(drep))
}

/// Parse DRep: tagged sum type
///   Haskell EncCBOR sum encoding: encodeListLen (1 + arity) <> encodeWord tag <> fields
///   tag 0: DRepKeyHash(KeyHash28) — array(2) [0, keyhash_bytes]
///   tag 1: DRepScriptHash(ScriptHash28) — array(2) [1, scripthash_bytes]
///   tag 2: DRepAlwaysAbstain — array(1) [2]
///   tag 3: DRepAlwaysNoConfidence — array(1) [3]
fn parse_drep(d: &mut minicbor::Decoder) -> Result<HaskellDRep, SerializationError> {
    let len = d
        .array()?
        .ok_or_else(|| SerializationError::CborDecode("DRep: expected definite array".into()))?;
    let tag = d.u32()?;

    match tag {
        0 => {
            // DRepKeyHash(KeyHash28)
            if len != 2 {
                return Err(SerializationError::CborDecode(format!(
                    "DRepKeyHash: expected array(2), got array({len})"
                )));
            }
            let hash = parse_keyhash28(d)?;
            Ok(HaskellDRep::KeyHash(hash))
        }
        1 => {
            // DRepScriptHash(ScriptHash28)
            if len != 2 {
                return Err(SerializationError::CborDecode(format!(
                    "DRepScriptHash: expected array(2), got array({len})"
                )));
            }
            let hash_bytes = d.bytes()?;
            if hash_bytes.len() != 28 {
                return Err(SerializationError::CborDecode(format!(
                    "DRep ScriptHash: expected 28 bytes, got {}",
                    hash_bytes.len()
                )));
            }
            let mut hash = [0u8; 28];
            hash.copy_from_slice(hash_bytes);
            Ok(HaskellDRep::ScriptHash(Hash28::from_bytes(hash)))
        }
        2 => {
            // DRepAlwaysAbstain — no additional fields
            if len != 1 {
                return Err(SerializationError::CborDecode(format!(
                    "DRepAlwaysAbstain: expected array(1), got array({len})"
                )));
            }
            Ok(HaskellDRep::Abstain)
        }
        3 => {
            // DRepAlwaysNoConfidence — no additional fields
            if len != 1 {
                return Err(SerializationError::CborDecode(format!(
                    "DRepAlwaysNoConfidence: expected array(1), got array({len})"
                )));
            }
            Ok(HaskellDRep::NoConfidence)
        }
        _ => Err(SerializationError::CborDecode(format!(
            "DRep: unknown tag {tag}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use minicbor::Encoder;

    /// Helper: encode a credential (KeyHash variant)
    fn encode_credential_keyhash(e: &mut Encoder<&mut Vec<u8>>, hash: &[u8; 28]) {
        e.array(2).unwrap();
        e.u32(0).unwrap();
        e.bytes(hash).unwrap();
    }

    /// Helper: encode a credential (ScriptHash variant)
    fn encode_credential_scripthash(e: &mut Encoder<&mut Vec<u8>>, hash: &[u8; 28]) {
        e.array(2).unwrap();
        e.u32(1).unwrap();
        e.bytes(hash).unwrap();
    }

    #[test]
    fn test_parse_drep_keyhash() {
        let mut buf = Vec::new();
        let mut e = Encoder::new(&mut buf);
        let hash = [0xABu8; 28];
        e.array(2).unwrap();
        e.u32(0).unwrap();
        e.bytes(&hash).unwrap();

        let mut d = minicbor::Decoder::new(&buf);
        let drep = parse_drep(&mut d).unwrap();
        assert!(matches!(drep, HaskellDRep::KeyHash(h) if h.as_bytes() == &hash));
    }

    #[test]
    fn test_parse_drep_scripthash() {
        let mut buf = Vec::new();
        let mut e = Encoder::new(&mut buf);
        let hash = [0xCDu8; 28];
        e.array(2).unwrap();
        e.u32(1).unwrap();
        e.bytes(&hash).unwrap();

        let mut d = minicbor::Decoder::new(&buf);
        let drep = parse_drep(&mut d).unwrap();
        assert!(matches!(drep, HaskellDRep::ScriptHash(h) if h.as_bytes() == &hash));
    }

    #[test]
    fn test_parse_drep_abstain() {
        let mut buf = Vec::new();
        let mut e = Encoder::new(&mut buf);
        e.array(1).unwrap();
        e.u32(2).unwrap();

        let mut d = minicbor::Decoder::new(&buf);
        let drep = parse_drep(&mut d).unwrap();
        assert!(matches!(drep, HaskellDRep::Abstain));
    }

    #[test]
    fn test_parse_drep_no_confidence() {
        let mut buf = Vec::new();
        let mut e = Encoder::new(&mut buf);
        e.array(1).unwrap();
        e.u32(3).unwrap();

        let mut d = minicbor::Decoder::new(&buf);
        let drep = parse_drep(&mut d).unwrap();
        assert!(matches!(drep, HaskellDRep::NoConfidence));
    }

    #[test]
    fn test_parse_drep_invalid_tag() {
        let mut buf = Vec::new();
        let mut e = Encoder::new(&mut buf);
        e.array(1).unwrap();
        e.u32(99).unwrap();

        let mut d = minicbor::Decoder::new(&buf);
        let result = parse_drep(&mut d);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_anchor() {
        let mut buf = Vec::new();
        let mut e = Encoder::new(&mut buf);
        let hash = [0x11u8; 32];
        e.array(2).unwrap();
        e.str("https://example.com/metadata.json").unwrap();
        e.bytes(&hash).unwrap();

        let mut d = minicbor::Decoder::new(&buf);
        let anchor = parse_anchor(&mut d).unwrap();
        assert_eq!(anchor.url, "https://example.com/metadata.json");
        assert_eq!(anchor.data_hash.as_bytes(), &hash);
    }

    #[test]
    fn test_parse_strict_maybe_anchor_nothing() {
        let mut buf = Vec::new();
        let mut e = Encoder::new(&mut buf);
        e.array(0).unwrap();

        let mut d = minicbor::Decoder::new(&buf);
        let result = parse_strict_maybe_anchor(&mut d).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_strict_maybe_anchor_just() {
        let mut buf = Vec::new();
        let mut e = Encoder::new(&mut buf);
        let hash = [0x22u8; 32];
        e.array(1).unwrap();
        // inner anchor
        e.array(2).unwrap();
        e.str("https://drep.example.com").unwrap();
        e.bytes(&hash).unwrap();

        let mut d = minicbor::Decoder::new(&buf);
        let result = parse_strict_maybe_anchor(&mut d).unwrap();
        assert!(result.is_some());
        let anchor = result.unwrap();
        assert_eq!(anchor.url, "https://drep.example.com");
    }

    #[test]
    fn test_parse_null_strict_maybe_keyhash_null() {
        let mut buf = Vec::new();
        let mut e = Encoder::new(&mut buf);
        e.null().unwrap();

        let mut d = minicbor::Decoder::new(&buf);
        let result = parse_null_strict_maybe_keyhash(&mut d).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_null_strict_maybe_keyhash_present() {
        let mut buf = Vec::new();
        let mut e = Encoder::new(&mut buf);
        let hash = [0x33u8; 28];
        e.bytes(&hash).unwrap();

        let mut d = minicbor::Decoder::new(&buf);
        let result = parse_null_strict_maybe_keyhash(&mut d).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().as_bytes(), &hash);
    }

    #[test]
    fn test_parse_null_strict_maybe_drep_null() {
        let mut buf = Vec::new();
        let mut e = Encoder::new(&mut buf);
        e.null().unwrap();

        let mut d = minicbor::Decoder::new(&buf);
        let result = parse_null_strict_maybe_drep(&mut d).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_null_strict_maybe_drep_present() {
        let mut buf = Vec::new();
        let mut e = Encoder::new(&mut buf);
        e.array(1).unwrap();
        e.u32(2).unwrap(); // DRepAlwaysAbstain

        let mut d = minicbor::Decoder::new(&buf);
        let result = parse_null_strict_maybe_drep(&mut d).unwrap();
        assert!(result.is_some());
        assert!(matches!(result.unwrap(), HaskellDRep::Abstain));
    }

    #[test]
    fn test_parse_committee_authorization_hot() {
        let mut buf = Vec::new();
        let mut e = Encoder::new(&mut buf);
        let hash = [0x44u8; 28];
        e.array(2).unwrap();
        e.u32(0).unwrap();
        encode_credential_keyhash(&mut e, &hash);

        let mut d = minicbor::Decoder::new(&buf);
        let auth = parse_committee_authorization(&mut d).unwrap();
        assert!(
            matches!(auth, HaskellCommitteeAuth::HotCredential(HaskellCredential::KeyHash(h)) if h.as_bytes() == &hash)
        );
    }

    #[test]
    fn test_parse_committee_authorization_resigned_no_anchor() {
        let mut buf = Vec::new();
        let mut e = Encoder::new(&mut buf);
        e.array(2).unwrap();
        e.u32(1).unwrap();
        e.array(0).unwrap(); // SNothing

        let mut d = minicbor::Decoder::new(&buf);
        let auth = parse_committee_authorization(&mut d).unwrap();
        assert!(matches!(auth, HaskellCommitteeAuth::Resigned(None)));
    }

    #[test]
    fn test_parse_committee_authorization_resigned_with_anchor() {
        let mut buf = Vec::new();
        let mut e = Encoder::new(&mut buf);
        let hash = [0x55u8; 32];
        e.array(2).unwrap();
        e.u32(1).unwrap();
        // SJust anchor
        e.array(1).unwrap();
        e.array(2).unwrap();
        e.str("https://resigned.example.com").unwrap();
        e.bytes(&hash).unwrap();

        let mut d = minicbor::Decoder::new(&buf);
        let auth = parse_committee_authorization(&mut d).unwrap();
        match auth {
            HaskellCommitteeAuth::Resigned(Some(a)) => {
                assert_eq!(a.url, "https://resigned.example.com");
            }
            _ => panic!("expected Resigned with anchor"),
        }
    }

    #[test]
    fn test_parse_conway_account_state() {
        let mut buf = Vec::new();
        let mut e = Encoder::new(&mut buf);
        let pool_hash = [0x66u8; 28];
        e.array(4).unwrap();
        e.u64(5_000_000).unwrap(); // rewards
        e.u64(2_000_000).unwrap(); // deposit
        e.bytes(&pool_hash).unwrap(); // pool delegation
        e.null().unwrap(); // no drep delegation

        let mut d = minicbor::Decoder::new(&buf);
        let account = parse_conway_account_state(&mut d).unwrap();
        assert_eq!(account.rewards.0, 5_000_000);
        assert_eq!(account.deposit.0, 2_000_000);
        assert!(account.pool_delegation.is_some());
        assert!(account.drep_delegation.is_none());
    }

    #[test]
    fn test_parse_conway_account_state_with_drep() {
        let mut buf = Vec::new();
        let mut e = Encoder::new(&mut buf);
        e.array(4).unwrap();
        e.u64(1_000_000).unwrap(); // rewards
        e.u64(500_000).unwrap(); // deposit
        e.null().unwrap(); // no pool delegation
                           // DRep: AlwaysNoConfidence
        e.array(1).unwrap();
        e.u32(3).unwrap();

        let mut d = minicbor::Decoder::new(&buf);
        let account = parse_conway_account_state(&mut d).unwrap();
        assert_eq!(account.rewards.0, 1_000_000);
        assert!(account.pool_delegation.is_none());
        assert!(matches!(
            account.drep_delegation,
            Some(HaskellDRep::NoConfidence)
        ));
    }

    #[test]
    fn test_parse_drep_state() {
        let mut buf = Vec::new();
        let mut e = Encoder::new(&mut buf);
        let anchor_hash = [0x77u8; 32];
        let cred_hash = [0x88u8; 28];
        e.array(4).unwrap();
        e.u64(100).unwrap(); // expiry epoch
                             // SJust anchor
        e.array(1).unwrap();
        e.array(2).unwrap();
        e.str("https://drep.example.com").unwrap();
        e.bytes(&anchor_hash).unwrap();
        // deposit
        e.u64(500_000_000).unwrap();
        // delegators set (with tag 258)
        e.tag(minicbor::data::Tag::new(258)).unwrap();
        e.array(1).unwrap();
        encode_credential_keyhash(&mut e, &cred_hash);

        let mut d = minicbor::Decoder::new(&buf);
        let state = parse_drep_state(&mut d).unwrap();
        assert_eq!(state.expiry.0, 100);
        assert!(state.anchor.is_some());
        assert_eq!(state.deposit.0, 500_000_000);
        assert_eq!(state.delegators.len(), 1);
    }

    #[test]
    fn test_parse_drep_state_no_anchor_no_delegators() {
        let mut buf = Vec::new();
        let mut e = Encoder::new(&mut buf);
        e.array(4).unwrap();
        e.u64(50).unwrap(); // expiry epoch
        e.array(0).unwrap(); // SNothing anchor
        e.u64(2_000_000).unwrap(); // deposit
                                   // delegators: empty set (no tag)
        e.array(0).unwrap();

        let mut d = minicbor::Decoder::new(&buf);
        let state = parse_drep_state(&mut d).unwrap();
        assert_eq!(state.expiry.0, 50);
        assert!(state.anchor.is_none());
        assert_eq!(state.deposit.0, 2_000_000);
        assert!(state.delegators.is_empty());
    }

    #[test]
    fn test_parse_vstate_empty() {
        let mut buf = Vec::new();
        let mut e = Encoder::new(&mut buf);
        e.array(3).unwrap();
        e.map(0).unwrap(); // empty dreps
        e.map(0).unwrap(); // empty committee state
        e.u64(0).unwrap(); // num_dormant_epochs

        let mut d = minicbor::Decoder::new(&buf);
        let vstate = parse_vstate(&mut d).unwrap();
        assert!(vstate.dreps.is_empty());
        assert!(vstate.committee_state.is_empty());
        assert_eq!(vstate.num_dormant_epochs.0, 0);
    }

    #[test]
    fn test_parse_keyhash_epoch_map() {
        let mut buf = Vec::new();
        let mut e = Encoder::new(&mut buf);
        let hash = [0x99u8; 28];
        e.map(1).unwrap();
        e.bytes(&hash).unwrap();
        e.u64(200).unwrap();

        let mut d = minicbor::Decoder::new(&buf);
        let map = parse_keyhash_epoch_map(&mut d).unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(map[&Hash28::from_bytes(hash)].0, 200);
    }

    #[test]
    fn test_parse_keyhash_coin_map() {
        let mut buf = Vec::new();
        let mut e = Encoder::new(&mut buf);
        let hash = [0xAAu8; 28];
        e.map(1).unwrap();
        e.bytes(&hash).unwrap();
        e.u64(500_000_000).unwrap();

        let mut d = minicbor::Decoder::new(&buf);
        let map = parse_keyhash_coin_map(&mut d).unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(map[&Hash28::from_bytes(hash)].0, 500_000_000);
    }

    #[test]
    fn test_parse_dstate_empty() {
        let mut buf = Vec::new();
        let mut e = Encoder::new(&mut buf);
        e.array(4).unwrap();
        e.map(0).unwrap(); // accounts
        e.map(0).unwrap(); // futureGenDelegs
        e.map(0).unwrap(); // genDelegs (a map with inner structure — but empty)
                           // InstantaneousRewards: array(2) [map, map] — both empty
        e.array(2).unwrap();
        e.map(0).unwrap();
        e.map(0).unwrap();

        let mut d = minicbor::Decoder::new(&buf);
        let dstate = parse_dstate(&mut d).unwrap();
        assert!(dstate.accounts.is_empty());
    }

    #[test]
    fn test_parse_credential_set_with_tag() {
        let mut buf = Vec::new();
        let mut e = Encoder::new(&mut buf);
        let hash1 = [0xBBu8; 28];
        let hash2 = [0xCCu8; 28];
        e.tag(minicbor::data::Tag::new(258)).unwrap();
        e.array(2).unwrap();
        encode_credential_keyhash(&mut e, &hash1);
        encode_credential_scripthash(&mut e, &hash2);

        let mut d = minicbor::Decoder::new(&buf);
        let creds = parse_credential_set(&mut d).unwrap();
        assert_eq!(creds.len(), 2);
        assert!(matches!(&creds[0], HaskellCredential::KeyHash(_)));
        assert!(matches!(&creds[1], HaskellCredential::ScriptHash(_)));
    }

    #[test]
    fn test_parse_credential_set_without_tag() {
        let mut buf = Vec::new();
        let mut e = Encoder::new(&mut buf);
        let hash = [0xDDu8; 28];
        e.array(1).unwrap();
        encode_credential_keyhash(&mut e, &hash);

        let mut d = minicbor::Decoder::new(&buf);
        let creds = parse_credential_set(&mut d).unwrap();
        assert_eq!(creds.len(), 1);
    }
}
