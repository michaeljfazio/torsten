use super::cbor_utils::*;
use super::pparams::decode_pparams;
use dugite_primitives::hash::Hash32;

// ── decode_uint ────────────────────────────────────────────────────────────────

#[test]
fn test_decode_uint_small() {
    // Values 0-23 are inline in the initial byte (additional info 0-23).
    assert_eq!(decode_uint(&[0x00]).unwrap(), (0, 1));
    assert_eq!(decode_uint(&[0x17]).unwrap(), (23, 1));
    // 24 requires a one-byte follow-on (additional info 24).
    assert_eq!(decode_uint(&[0x18, 0x18]).unwrap(), (24, 2));
    assert_eq!(decode_uint(&[0x18, 0xff]).unwrap(), (255, 2));
}

#[test]
fn test_decode_uint_large() {
    // Two-byte uint (additional info 25).
    assert_eq!(decode_uint(&[0x19, 0x01, 0x00]).unwrap(), (256, 3));
    // Four-byte uint (additional info 26).
    assert_eq!(
        decode_uint(&[0x1a, 0x00, 0x01, 0x00, 0x00]).unwrap(),
        (65536, 5)
    );
    // Eight-byte uint (additional info 27).
    assert_eq!(
        decode_uint(&[0x1b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2c]).unwrap(),
        (44, 9)
    );
}

#[test]
fn test_decode_uint_wrong_major() {
    // Major type 1 (negative integer) should be rejected by decode_uint.
    assert!(decode_uint(&[0x20]).is_err());
}

// ── decode_int ─────────────────────────────────────────────────────────────────

#[test]
fn test_decode_int_positive() {
    assert_eq!(decode_int(&[0x00]).unwrap(), (0, 1));
    assert_eq!(decode_int(&[0x0a]).unwrap(), (10, 1));
}

#[test]
fn test_decode_int_negative() {
    // 0x20 = major 1, info 0 → -1
    assert_eq!(decode_int(&[0x20]).unwrap(), (-1, 1));
    // 0x37 = major 1, info 23 → -24
    assert_eq!(decode_int(&[0x37]).unwrap(), (-24, 1));
}

// ── decode_array_len ───────────────────────────────────────────────────────────

#[test]
fn test_decode_array_len() {
    assert_eq!(decode_array_len(&[0x80]).unwrap(), (0, 1)); // array(0)
    assert_eq!(decode_array_len(&[0x82]).unwrap(), (2, 1)); // array(2)
    assert_eq!(decode_array_len(&[0x87]).unwrap(), (7, 1)); // array(7)
    assert_eq!(decode_array_len(&[0x98, 0x1f]).unwrap(), (31, 2)); // array(31)
}

#[test]
fn test_decode_array_len_wrong_major() {
    // 0xa0 = map(0) — not an array.
    assert!(decode_array_len(&[0xa0]).is_err());
}

// ── decode_map_len ─────────────────────────────────────────────────────────────

#[test]
fn test_decode_map_len_definite() {
    assert_eq!(decode_map_len(&[0xa0]).unwrap(), (Some(0), 1));
    assert_eq!(decode_map_len(&[0xa3]).unwrap(), (Some(3), 1));
}

#[test]
fn test_decode_map_len_indefinite() {
    // 0xbf = indefinite-length map
    assert_eq!(decode_map_len(&[0xbf]).unwrap(), (None, 1));
}

// ── decode_nonce ───────────────────────────────────────────────────────────────

#[test]
fn test_decode_nonce_neutral() {
    // array(1) [0] = NeutralNonce → zero hash
    let data = [0x81, 0x00];
    let (hash, consumed) = decode_nonce(&data).unwrap();
    assert_eq!(consumed, 2);
    assert_eq!(hash, Hash32::ZERO);
}

#[test]
fn test_decode_nonce_value() {
    // array(2) [1, bytes(32)] = Nonce carrying a hash value
    let mut data = vec![0x82, 0x01, 0x58, 0x20];
    data.extend_from_slice(&[0xab; 32]);
    let (hash, consumed) = decode_nonce(&data).unwrap();
    // 1 (array hdr) + 1 (tag uint) + 2 (bytes hdr) + 32 (payload) = 36
    assert_eq!(consumed, 36);
    assert_eq!(hash.as_bytes(), &[0xab; 32]);
}

#[test]
fn test_decode_nonce_invalid_tag() {
    // array(1) [2] — tag 2 is not valid for a Nonce
    let data = [0x81, 0x02];
    assert!(decode_nonce(&data).is_err());
}

// ── decode_with_origin_len ─────────────────────────────────────────────────────

#[test]
fn test_decode_with_origin_absent() {
    // array(0) = Origin
    let data = [0x80];
    let (present, consumed) = decode_with_origin_len(&data).unwrap();
    assert_eq!(consumed, 1);
    assert!(present.is_none());
}

#[test]
fn test_decode_with_origin_present() {
    // array(1) = At x; only the array header is consumed by this function
    let data = [0x81, 0x19, 0x04, 0x00]; // [1024]
    let (present, consumed) = decode_with_origin_len(&data).unwrap();
    assert_eq!(consumed, 1); // only the array header byte
    assert!(present.is_some());
}

#[test]
fn test_decode_with_origin_invalid_len() {
    // array(2) is neither Origin nor At — must error
    let data = [0x82, 0x01, 0x02];
    assert!(decode_with_origin_len(&data).is_err());
}

// ── decode_rational ────────────────────────────────────────────────────────────

#[test]
fn test_decode_rational_with_tag() {
    // tag(30) array(2) [3, 10]  =  3/10
    let data = [0xd8, 0x1e, 0x82, 0x03, 0x0a];
    let ((num, den), consumed) = decode_rational(&data).unwrap();
    assert_eq!(num, 3);
    assert_eq!(den, 10);
    assert_eq!(consumed, 5);
}

#[test]
fn test_decode_rational_no_tag() {
    // array(2) [0x19 0x02 0x41, 0x19 0x27 0x10]  =  [577, 10000]
    let data = [0x82, 0x19, 0x02, 0x41, 0x19, 0x27, 0x10];
    let ((num, den), consumed) = decode_rational(&data).unwrap();
    assert_eq!(num, 577);
    assert_eq!(den, 10000);
    assert_eq!(consumed, 7);
}

#[test]
fn test_decode_rational_small() {
    // Plain array(2) [1, 1]  = 1/1
    let data = [0x82, 0x01, 0x01];
    let ((num, den), consumed) = decode_rational(&data).unwrap();
    assert_eq!(num, 1);
    assert_eq!(den, 1);
    assert_eq!(consumed, 3);
}

// ── decode_credential ─────────────────────────────────────────────────────────

#[test]
fn test_decode_credential_keyhash() {
    // array(2) [0, bytes(28)]  = KeyHash credential
    let mut data = vec![0x82, 0x00, 0x58, 0x1c];
    data.extend_from_slice(&[0xaa; 28]);
    let ((tag, hash), consumed) = decode_credential(&data).unwrap();
    assert_eq!(tag, 0);
    assert_eq!(hash.as_bytes(), &[0xaa; 28]);
    // 1 (array hdr) + 1 (tag uint) + 2 (bytes hdr 0x58 0x1c) + 28 (payload) = 32
    assert_eq!(consumed, 32);
}

#[test]
fn test_decode_credential_scripthash() {
    // array(2) [1, bytes(28)]  = ScriptHash credential
    let mut data = vec![0x82, 0x01, 0x58, 0x1c];
    data.extend_from_slice(&[0xbb; 28]);
    let ((tag, hash), consumed) = decode_credential(&data).unwrap();
    assert_eq!(tag, 1);
    assert_eq!(hash.as_bytes(), &[0xbb; 28]);
    assert_eq!(consumed, 32);
}

// ── skip_cbor_value ────────────────────────────────────────────────────────────

#[test]
fn test_skip_uint() {
    assert_eq!(skip_cbor_value(&[0x05]).unwrap(), 1);
    assert_eq!(skip_cbor_value(&[0x18, 0x64]).unwrap(), 2);
}

#[test]
fn test_skip_bytes() {
    // bytes(4) 0x44 0x01 0x02 0x03 0x04
    assert_eq!(skip_cbor_value(&[0x44, 0x01, 0x02, 0x03, 0x04]).unwrap(), 5);
}

#[test]
fn test_skip_nested_array() {
    // array(2) [1, bytes(32)]
    let mut data = vec![0x82, 0x01, 0x58, 0x20];
    data.extend_from_slice(&[0x00; 32]);
    assert_eq!(skip_cbor_value(&data).unwrap(), 36);
}

#[test]
fn test_skip_map() {
    // map(1) {0 => 1}  =  0xa1 0x00 0x01
    assert_eq!(skip_cbor_value(&[0xa1, 0x00, 0x01]).unwrap(), 3);
}

#[test]
fn test_skip_tagged_value() {
    // tag(30) array(2) [1, 2]  = rational 1/2
    assert_eq!(
        skip_cbor_value(&[0xd8, 0x1e, 0x82, 0x01, 0x02]).unwrap(),
        5
    );
}

// ── decode_null ───────────────────────────────────────────────────────────────

#[test]
fn test_decode_null_is_null() {
    assert_eq!(decode_null(&[0xf6]).unwrap(), (true, 1));
}

#[test]
fn test_decode_null_not_null() {
    // A non-null value: cursor should not be advanced.
    assert_eq!(decode_null(&[0x00]).unwrap(), (false, 0));
    assert_eq!(decode_null(&[0x80]).unwrap(), (false, 0));
}

// ── decode_bytes / decode_text ─────────────────────────────────────────────────

#[test]
fn test_decode_bytes_short() {
    // bytes(3) 0x41 0x42 0x43
    let data = [0x43, 0x41, 0x42, 0x43];
    let (b, n) = decode_bytes(&data).unwrap();
    assert_eq!(b, b"ABC");
    assert_eq!(n, 4);
}

#[test]
fn test_decode_text_short() {
    // text(5) "hello"  = 0x65 h e l l o
    let data = [0x65, b'h', b'e', b'l', b'l', b'o'];
    let (s, n) = decode_text(&data).unwrap();
    assert_eq!(s, "hello");
    assert_eq!(n, 6);
}

#[test]
fn test_decode_text_wrong_major() {
    // 0x43 is bytes(3), not text
    assert!(decode_text(&[0x43, 0x41, 0x42, 0x43]).is_err());
}

// ── decode_hash28 / decode_hash32 ─────────────────────────────────────────────

#[test]
fn test_decode_hash28_correct_length() {
    let mut data = vec![0x58, 0x1c]; // bytes(28)
    data.extend_from_slice(&[0xde; 28]);
    let (h, n) = decode_hash28(&data).unwrap();
    assert_eq!(h.as_bytes(), &[0xde; 28]);
    assert_eq!(n, 30);
}

#[test]
fn test_decode_hash28_wrong_length() {
    // bytes(32) should be rejected by decode_hash28
    let mut data = vec![0x58, 0x20];
    data.extend_from_slice(&[0x00; 32]);
    assert!(decode_hash28(&data).is_err());
}

#[test]
fn test_decode_hash32_correct_length() {
    let mut data = vec![0x58, 0x20]; // bytes(32)
    data.extend_from_slice(&[0xef; 32]);
    let (h, n) = decode_hash32(&data).unwrap();
    assert_eq!(h.as_bytes(), &[0xef; 32]);
    assert_eq!(n, 34);
}

#[test]
fn test_decode_hash32_wrong_length() {
    // bytes(28) should be rejected by decode_hash32
    let mut data = vec![0x58, 0x1c];
    data.extend_from_slice(&[0x00; 28]);
    assert!(decode_hash32(&data).is_err());
}

// ── decode_bigint_or_uint ─────────────────────────────────────────────────────

#[test]
fn test_decode_bigint_plain_uint() {
    assert_eq!(decode_bigint_or_uint(&[0x0a]).unwrap(), (10, 1));
}

#[test]
fn test_decode_bigint_tag2() {
    // tag(2) bytes(2) [0x01, 0x00]  = bignum 256
    let data = [0xc2, 0x42, 0x01, 0x00];
    let (v, n) = decode_bigint_or_uint(&data).unwrap();
    assert_eq!(v, 256);
    assert_eq!(n, 4);
}

// ── decode_pparams ─────────────────────────────────────────────────────────

/// Round-trip test against the real preview testnet PParams captured at
/// epoch 1259.  All expected values are verified against Koios on-chain data.
#[test]
fn test_decode_pparams_preview_epoch_1259() {
    let pp_cbor = include_bytes!("../../test_fixtures/preview_pparams_e1259.cbor");
    let (pp, consumed) = decode_pparams(pp_cbor).unwrap();

    // All bytes must be consumed — the fixture contains exactly one PParams value.
    assert_eq!(consumed, pp_cbor.len(), "not all bytes were consumed");

    // ── basic fee / size fields ──────────────────────────────────────────────
    assert_eq!(pp.min_fee_a, 44, "minFeeA");
    assert_eq!(pp.min_fee_b, 155381, "minFeeB");
    assert_eq!(pp.max_block_body_size, 90112, "maxBlockBodySize");
    assert_eq!(pp.max_tx_size, 16384, "maxTxSize");
    assert_eq!(pp.max_block_header_size, 1100, "maxBlockHeaderSize");

    // ── staking deposits ────────────────────────────────────────────────────
    assert_eq!(pp.key_deposit.0, 2_000_000, "keyDeposit");
    assert_eq!(pp.pool_deposit.0, 500_000_000, "poolDeposit");

    // ── economy parameters ───────────────────────────────────────────────────
    assert_eq!(pp.e_max, 18, "eMax");
    assert_eq!(pp.n_opt, 500, "nOpt");
    // a0 = 3/10
    assert_eq!(pp.a0.numerator, 3, "a0.num");
    assert_eq!(pp.a0.denominator, 10, "a0.den");
    // rho = 3/1000
    assert_eq!(pp.rho.numerator, 3, "rho.num");
    assert_eq!(pp.rho.denominator, 1000, "rho.den");
    // tau = 1/5
    assert_eq!(pp.tau.numerator, 1, "tau.num");
    assert_eq!(pp.tau.denominator, 5, "tau.den");

    // ── protocol version ─────────────────────────────────────────────────────
    assert_eq!(pp.protocol_version_major, 10, "protocolVersion.major");
    assert_eq!(pp.protocol_version_minor, 0, "protocolVersion.minor");

    // ── UTxO and pool minimums ────────────────────────────────────────────────
    assert_eq!(pp.min_pool_cost.0, 170_000_000, "minPoolCost");
    assert_eq!(pp.ada_per_utxo_byte.0, 4310, "adaPerUTxOByte");

    // ── cost models present ──────────────────────────────────────────────────
    assert!(pp.cost_models.plutus_v1.is_some(), "PlutusV1 cost model missing");
    assert!(pp.cost_models.plutus_v2.is_some(), "PlutusV2 cost model missing");
    assert!(pp.cost_models.plutus_v3.is_some(), "PlutusV3 cost model missing");
    // Spot-check entry counts (matches cardano-node 10.x Conway cost models)
    assert_eq!(
        pp.cost_models.plutus_v1.as_ref().unwrap().len(),
        166,
        "PlutusV1 cost count"
    );
    assert_eq!(
        pp.cost_models.plutus_v2.as_ref().unwrap().len(),
        175,
        "PlutusV2 cost count"
    );
    assert_eq!(
        pp.cost_models.plutus_v3.as_ref().unwrap().len(),
        297,
        "PlutusV3 cost count"
    );

    // ── execution unit prices ────────────────────────────────────────────────
    // mem_price = 577/10000, step_price = 721/10000000
    assert_eq!(pp.execution_costs.mem_price.numerator, 577, "mem_price.num");
    assert_eq!(
        pp.execution_costs.mem_price.denominator, 10000, "mem_price.den"
    );
    assert_eq!(
        pp.execution_costs.step_price.numerator, 721, "step_price.num"
    );
    assert_eq!(
        pp.execution_costs.step_price.denominator, 10_000_000, "step_price.den"
    );

    // ── execution unit limits ────────────────────────────────────────────────
    assert_eq!(pp.max_tx_ex_units.mem, 16_500_000, "maxTxExUnits.mem");
    assert_eq!(
        pp.max_tx_ex_units.steps, 10_000_000_000, "maxTxExUnits.steps"
    );
    assert_eq!(pp.max_block_ex_units.mem, 72_000_000, "maxBlockExUnits.mem");
    assert_eq!(
        pp.max_block_ex_units.steps, 20_000_000_000, "maxBlockExUnits.steps"
    );

    // ── collateral ───────────────────────────────────────────────────────────
    assert_eq!(pp.max_val_size, 5000, "maxValSize");
    assert_eq!(pp.collateral_percentage, 150, "collateralPercentage");
    assert_eq!(pp.max_collateral_inputs, 3, "maxCollateralInputs");

    // ── Conway governance ────────────────────────────────────────────────────
    assert_eq!(pp.committee_min_size, 3, "committeeMinSize");
    assert_eq!(pp.committee_max_term_length, 365, "committeeMaxTermLength");
    assert_eq!(pp.gov_action_lifetime, 30, "govActionLifetime");
    assert_eq!(pp.gov_action_deposit.0, 100_000_000_000, "govActionDeposit");
    assert_eq!(pp.drep_deposit.0, 500_000_000, "dRepDeposit");
    assert_eq!(pp.drep_activity, 31, "dRepActivity");

    // ── ref script fee ───────────────────────────────────────────────────────
    // Haskell encodes as rational 15/1; we extract 15.
    assert_eq!(
        pp.min_fee_ref_script_cost_per_byte, 15,
        "minFeeRefScriptCostPerByte"
    );

    // ── pool voting thresholds (all 51/100 on preview) ────────────────────────
    assert_eq!(
        pp.pvt_motion_no_confidence.numerator, 51,
        "pvtMotionNoConfidence.num"
    );
    assert_eq!(
        pp.pvt_motion_no_confidence.denominator, 100,
        "pvtMotionNoConfidence.den"
    );
    assert_eq!(pp.pvt_committee_normal.numerator, 51, "pvtCommitteeNormal.num");
    assert_eq!(
        pp.pvt_committee_normal.denominator, 100, "pvtCommitteeNormal.den"
    );
    assert_eq!(
        pp.pvt_committee_no_confidence.numerator, 51,
        "pvtCommitteeNoConfidence.num"
    );
    assert_eq!(
        pp.pvt_committee_no_confidence.denominator, 100,
        "pvtCommitteeNoConfidence.den"
    );
    assert_eq!(pp.pvt_hard_fork.numerator, 51, "pvtHardFork.num");
    assert_eq!(pp.pvt_hard_fork.denominator, 100, "pvtHardFork.den");
    assert_eq!(
        pp.pvt_pp_security_group.numerator, 51, "pvtPPSecurityGroup.num"
    );
    assert_eq!(
        pp.pvt_pp_security_group.denominator, 100, "pvtPPSecurityGroup.den"
    );

    // ── DRep voting thresholds ────────────────────────────────────────────────
    // dvtMotionNoConfidence = 67/100
    assert_eq!(
        pp.dvt_no_confidence.numerator, 67, "dvtNoConfidence.num"
    );
    assert_eq!(
        pp.dvt_no_confidence.denominator, 100, "dvtNoConfidence.den"
    );
    // dvtCommitteeNormal = 67/100
    assert_eq!(
        pp.dvt_committee_normal.numerator, 67, "dvtCommitteeNormal.num"
    );
    // dvtCommitteeNoConfidence = 3/5
    assert_eq!(
        pp.dvt_committee_no_confidence.numerator, 3,
        "dvtCommitteeNoConfidence.num"
    );
    assert_eq!(
        pp.dvt_committee_no_confidence.denominator, 5,
        "dvtCommitteeNoConfidence.den"
    );
    // dvtUpdateToConstitution (→ dvt_constitution) = 3/4
    assert_eq!(
        pp.dvt_constitution.numerator, 3, "dvtConstitution.num"
    );
    assert_eq!(
        pp.dvt_constitution.denominator, 4, "dvtConstitution.den"
    );
    // dvtHardForkInitiation = 3/5
    assert_eq!(pp.dvt_hard_fork.numerator, 3, "dvtHardFork.num");
    assert_eq!(pp.dvt_hard_fork.denominator, 5, "dvtHardFork.den");
    // dvtPPNetworkGroup = 67/100
    assert_eq!(
        pp.dvt_pp_network_group.numerator, 67, "dvtPPNetworkGroup.num"
    );
    assert_eq!(
        pp.dvt_pp_network_group.denominator, 100, "dvtPPNetworkGroup.den"
    );
    // dvtPPGovGroup = 3/4
    assert_eq!(pp.dvt_pp_gov_group.numerator, 3, "dvtPPGovGroup.num");
    assert_eq!(pp.dvt_pp_gov_group.denominator, 4, "dvtPPGovGroup.den");
    // dvtTreasuryWithdrawal = 67/100
    assert_eq!(
        pp.dvt_treasury_withdrawal.numerator, 67, "dvtTreasuryWithdrawal.num"
    );
    assert_eq!(
        pp.dvt_treasury_withdrawal.denominator, 100, "dvtTreasuryWithdrawal.den"
    );
}
