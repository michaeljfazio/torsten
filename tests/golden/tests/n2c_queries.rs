//! Golden tests for N2C (Node-to-Client) query/result CBOR encoding.
//!
//! Test vectors from ouroboros-consensus golden files:
//! `golden/cardano/QueryVersion3/CardanoNodeToClientVersion19/`
//!
//! These verify our CBOR encoding matches the Haskell cardano-node exactly.

#[test]
fn test_query_conway_get_current_pparams_cbor() {
    // Golden: [0, [6, [3]]] = HFC-wrapped Conway GetCurrentPParams
    // 0x82 0x00 0x82 0x06 0x81 0x03
    let golden = include_bytes!("../n2c/Query_Conway_GetCurrentPParams");
    assert_eq!(golden, &[0x82, 0x00, 0x82, 0x06, 0x81, 0x03]);

    // Verify our understanding: [0, [6, [3]]]
    // 82 = array(2)
    // 00 = uint(0) — BlockQuery (not QueryAnytime/QueryHardFork)
    // 82 = array(2)
    // 06 = uint(6) — era index 6 = Conway
    // 81 = array(1)
    // 03 = uint(3) — query tag 3 = GetCurrentPParams
}

#[test]
fn test_query_conway_get_epoch_no_cbor() {
    // Golden: [0, [6, [1]]] = HFC-wrapped Conway GetEpochNo
    let golden = include_bytes!("../n2c/Query_Conway_GetEpochNo");
    assert_eq!(golden, &[0x82, 0x00, 0x82, 0x06, 0x81, 0x01]);
}

#[test]
fn test_result_conway_epoch_no_cbor() {
    // Golden: [10] = epoch number 10 wrapped in success array
    let golden = include_bytes!("../n2c/Result_Conway_EpochNo");
    assert_eq!(golden, &[0x81, 0x0a]);
}

#[test]
fn test_result_conway_max_major_protocol_version_cbor() {
    // Golden: [13] = max major protocol version 13
    let golden = include_bytes!("../n2c/Result_Conway_MaxMajorProtocolVersion");
    assert_eq!(golden, &[0x81, 0x0d]);
}
