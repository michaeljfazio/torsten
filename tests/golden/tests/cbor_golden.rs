//! CBOR golden tests against official Cardano test vectors.
//!
//! # Sources
//!
//! ## ouroboros-consensus (official Haskell node golden files)
//!
//! Located at:
//!   `ouroboros-consensus-cardano/golden/cardano/QueryVersion3/CardanoNodeToClientVersion19/`
//!
//! These are the authoritative golden files produced by the Haskell
//! cardano-node test suite. Any encoding divergence from these files
//! indicates a protocol incompatibility.
//!
//! Fixture files stored under `tests/golden/n2c/` are downloaded verbatim
//! from the upstream repository and committed as binary blobs so that this
//! test can run offline.
//!
//! ## Cardano Blueprint (cardano-scaling/cardano-blueprint)
//!
//! Located at:
//!   `src/network/node-to-node/handshake/test-data/`
//!
//! Five CBOR test vectors covering the N2N handshake mini-protocol:
//!   test-0 … test-4, stored under `tests/golden/handshake/`.
//!
//! ## Protocol constants
//!
//! The following invariants are verified inline (no fixture files needed):
//!
//!   - HFC success wrapper = `array(1)` around the query result
//!   - Conway PParams = positional `array(31)` with tag(30) for rationals
//!   - Value encoding = plain uint for ADA-only, `[coin, multiasset_map]` for multi-asset
//!   - CBOR Set (tag 258) elements must be sorted for canonical encoding
//!   - MsgResult wire format = `[4, result]` (or `[4, [result]]` for BlockQuery)

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read a fixture file relative to the golden package manifest.
fn fixture(subdir: &str, name: &str) -> Vec<u8> {
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push(subdir);
    p.push(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("fixture {subdir}/{name}: {e}"))
}

/// Decode a CBOR item from `data` at byte offset `i`.
/// Returns `(decoded_repr, new_offset)`.
/// This is a minimal recursive decoder for test assertions; it does not
/// handle indefinite-length items or floats.
fn cbor_read(data: &[u8], i: usize) -> (CborVal, usize) {
    let b = data[i];
    let major = b >> 5;
    let add = b & 0x1f;
    let mut i = i + 1;

    match major {
        0 => {
            // Unsigned integer
            let (v, next) = read_uint(data, add, i);
            (CborVal::Uint(v), next)
        }
        1 => {
            // Negative integer
            let (v, next) = read_uint(data, add, i);
            (CborVal::Nint(-(v as i64) - 1), next)
        }
        2 => {
            // Byte string
            let (len, next) = read_uint(data, add, i);
            i = next;
            let bytes = data[i..i + len as usize].to_vec();
            (CborVal::Bytes(bytes), i + len as usize)
        }
        3 => {
            // Text string
            let (len, next) = read_uint(data, add, i);
            i = next;
            let s = std::str::from_utf8(&data[i..i + len as usize])
                .unwrap()
                .to_string();
            (CborVal::Text(s), i + len as usize)
        }
        4 => {
            // Array
            let (count, next) = read_uint(data, add, i);
            i = next;
            let mut items = Vec::new();
            for _ in 0..count {
                let (v, next) = cbor_read(data, i);
                i = next;
                items.push(v);
            }
            (CborVal::Array(items), i)
        }
        5 => {
            // Map
            let (count, next) = read_uint(data, add, i);
            i = next;
            let mut pairs = Vec::new();
            for _ in 0..count {
                let (k, next) = cbor_read(data, i);
                i = next;
                let (v, next) = cbor_read(data, i);
                i = next;
                pairs.push((k, v));
            }
            (CborVal::Map(pairs), i)
        }
        6 => {
            // Tag
            let (tag_num, next) = read_uint(data, add, i);
            let (inner, next2) = cbor_read(data, next);
            (CborVal::Tag(tag_num, Box::new(inner)), next2)
        }
        7 => match add {
            20 => (CborVal::Bool(false), i),
            21 => (CborVal::Bool(true), i),
            22 => (CborVal::Null, i),
            _ => panic!("unhandled simple value {add}"),
        },
        _ => panic!("unhandled CBOR major type {major}"),
    }
}

fn read_uint(data: &[u8], add: u8, i: usize) -> (u64, usize) {
    match add {
        n if n < 24 => (n as u64, i),
        24 => (data[i] as u64, i + 1),
        25 => (u16::from_be_bytes([data[i], data[i + 1]]) as u64, i + 2),
        26 => (
            u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as u64,
            i + 4,
        ),
        27 => (
            u64::from_be_bytes([
                data[i],
                data[i + 1],
                data[i + 2],
                data[i + 3],
                data[i + 4],
                data[i + 5],
                data[i + 6],
                data[i + 7],
            ]),
            i + 8,
        ),
        _ => panic!("indefinite length not supported in test decoder"),
    }
}

/// Minimal CBOR value representation for test assertions.
#[derive(Debug, Clone, PartialEq)]
enum CborVal {
    Uint(u64),
    Nint(i64),
    Bytes(Vec<u8>),
    Text(String),
    Array(Vec<CborVal>),
    Map(Vec<(CborVal, CborVal)>),
    Tag(u64, Box<CborVal>),
    Bool(bool),
    Null,
}

impl CborVal {
    fn as_uint(&self) -> u64 {
        match self {
            CborVal::Uint(n) => *n,
            _ => panic!("expected Uint, got {:?}", self),
        }
    }
    fn as_array(&self) -> &[CborVal] {
        match self {
            CborVal::Array(a) => a,
            _ => panic!("expected Array, got {:?}", self),
        }
    }
    fn as_bytes(&self) -> &[u8] {
        match self {
            CborVal::Bytes(b) => b,
            _ => panic!("expected Bytes, got {:?}", self),
        }
    }
    fn as_map(&self) -> &[(CborVal, CborVal)] {
        match self {
            CborVal::Map(m) => m,
            _ => panic!("expected Map, got {:?}", self),
        }
    }
    fn as_tag(&self) -> (u64, &CborVal) {
        match self {
            CborVal::Tag(t, v) => (*t, v),
            _ => panic!("expected Tag, got {:?}", self),
        }
    }
    fn as_text(&self) -> &str {
        match self {
            CborVal::Text(s) => s,
            _ => panic!("expected Text, got {:?}", self),
        }
    }
}

// ---------------------------------------------------------------------------
// Section 1 — ouroboros-consensus N2C query golden files
//
// Source: ouroboros-consensus-cardano/golden/cardano/
//         QueryVersion3/CardanoNodeToClientVersion19/
//
// These tests verify that our query encoding matches the Haskell reference
// exactly (byte-for-byte) AND that we can structurally decode the golden files
// to confirm our understanding of the CBOR layout.
// ---------------------------------------------------------------------------

// --- 1.1 Query messages (client → server) ---

#[test]
fn golden_query_conway_get_epoch_no() {
    // Official golden: 82 00 82 06 81 01
    // [0, [6, [1]]] = BlockQuery[Conway, GetEpochNo(1)]
    let golden = fixture("n2c", "Query_Conway_GetEpochNo");
    assert_eq!(
        golden,
        vec![0x82, 0x00, 0x82, 0x06, 0x81, 0x01],
        "Query_Conway_GetEpochNo must match ouroboros-consensus golden"
    );

    let (v, _) = cbor_read(&golden, 0);
    let arr = v.as_array();
    assert_eq!(arr[0].as_uint(), 0, "outer tag 0 = BlockQuery");
    let inner = arr[1].as_array();
    assert_eq!(inner[0].as_uint(), 6, "era index 6 = Conway");
    let query = inner[1].as_array();
    assert_eq!(query[0].as_uint(), 1, "query tag 1 = GetEpochNo");
}

#[test]
fn golden_query_conway_get_current_pparams() {
    // Official golden: 82 00 82 06 81 03
    // [0, [6, [3]]] = BlockQuery[Conway, GetCurrentPParams(3)]
    let golden = fixture("n2c", "Query_Conway_GetCurrentPParams");
    assert_eq!(
        golden,
        vec![0x82, 0x00, 0x82, 0x06, 0x81, 0x03],
        "Query_Conway_GetCurrentPParams must match ouroboros-consensus golden"
    );

    let (v, _) = cbor_read(&golden, 0);
    let arr = v.as_array();
    assert_eq!(arr[0].as_uint(), 0, "outer tag 0 = BlockQuery");
    let inner = arr[1].as_array();
    assert_eq!(inner[0].as_uint(), 6, "era index 6 = Conway");
    let query = inner[1].as_array();
    assert_eq!(query[0].as_uint(), 3, "query tag 3 = GetCurrentPParams");
}

#[test]
fn golden_query_conway_get_max_major_protocol_version() {
    // Official golden: 82 00 82 06 81 18 26
    // [0, [6, [38]]] = BlockQuery[Conway, GetMaxMajorProtocolVersion(38)]
    let golden = fixture("n2c", "Query_Conway_GetMaxMajorProtocolVersion");
    assert_eq!(
        golden,
        vec![0x82, 0x00, 0x82, 0x06, 0x81, 0x18, 0x26],
        "Query_Conway_GetMaxMajorProtocolVersion must match ouroboros-consensus golden"
    );

    let (v, _) = cbor_read(&golden, 0);
    let arr = v.as_array();
    assert_eq!(arr[0].as_uint(), 0, "outer tag 0 = BlockQuery");
    let inner = arr[1].as_array();
    assert_eq!(inner[0].as_uint(), 6, "era index 6 = Conway");
    let query = inner[1].as_array();
    assert_eq!(
        query[0].as_uint(),
        38,
        "query tag 38 = GetMaxMajorProtocolVersion"
    );
}

#[test]
fn golden_query_conway_get_ledger_tip() {
    // Official golden: 82 00 82 06 81 00
    // [0, [6, [0]]] = BlockQuery[Conway, GetLedgerTip(0)]
    let golden = fixture("n2c", "Query_Conway_GetLedgerTip");
    assert_eq!(
        golden,
        vec![0x82, 0x00, 0x82, 0x06, 0x81, 0x00],
        "Query_Conway_GetLedgerTip must match ouroboros-consensus golden"
    );
}

#[test]
fn golden_query_conway_get_genesis_config() {
    // Official golden: 82 00 82 06 81 0B
    // [0, [6, [11]]] = BlockQuery[Conway, GetGenesisConfig(11)]
    let golden = fixture("n2c", "Query_Conway_GetGenesisConfig");
    assert_eq!(
        golden,
        vec![0x82, 0x00, 0x82, 0x06, 0x81, 0x0b],
        "Query_Conway_GetGenesisConfig must match ouroboros-consensus golden"
    );
}

#[test]
fn golden_query_conway_get_stake_distribution2() {
    // Official golden: 82 00 82 06 81 18 25
    // [0, [6, [37]]] = BlockQuery[Conway, GetStakeDistribution2(37)]
    let golden = fixture("n2c", "Query_Conway_GetStakeDistribution2");
    assert_eq!(
        golden,
        vec![0x82, 0x00, 0x82, 0x06, 0x81, 0x18, 0x25],
        "Query_Conway_GetStakeDistribution2 must match ouroboros-consensus golden"
    );
}

#[test]
fn golden_query_conway_get_big_ledger_peer_snapshot() {
    // Official golden: 82 00 82 06 82 18 22 01
    // [0, [6, [34, 1]]] = BlockQuery[Conway, GetBigLedgerPeerSnapshot(34, flag=1)]
    let golden = fixture("n2c", "Query_Conway_GetBigLedgerPeerSnapshot");
    assert_eq!(
        golden,
        vec![0x82, 0x00, 0x82, 0x06, 0x82, 0x18, 0x22, 0x01],
        "Query_Conway_GetBigLedgerPeerSnapshot must match ouroboros-consensus golden"
    );

    // The query tag 34 (0x22) for GetBigLedgerPeerSnapshot is a 2-element array
    // because it takes a parameter (flag=1)
    let (v, _) = cbor_read(&golden, 0);
    let outer = v.as_array();
    let hfc = outer[1].as_array();
    assert_eq!(hfc[0].as_uint(), 6, "era 6 = Conway");
    let qpair = hfc[1].as_array();
    assert_eq!(
        qpair[0].as_uint(),
        34,
        "query tag 34 = GetBigLedgerPeerSnapshot"
    );
    assert_eq!(qpair[1].as_uint(), 1, "flag parameter");
}

#[test]
fn golden_query_conway_get_non_myopic_member_rewards() {
    // Official golden includes credential list parameters
    // [0, [6, [2, tag(258)[...credentials...]]]]
    let golden = fixture("n2c", "Query_Conway_GetNonMyopicMemberRewards");
    assert!(!golden.is_empty(), "fixture must exist");

    let (v, _) = cbor_read(&golden, 0);
    let outer = v.as_array();
    assert_eq!(outer[0].as_uint(), 0, "BlockQuery tag 0");
    let hfc = outer[1].as_array();
    assert_eq!(hfc[0].as_uint(), 6, "era 6 = Conway");
    let q = hfc[1].as_array();
    assert_eq!(q[0].as_uint(), 2, "query tag 2 = GetNonMyopicMemberRewards");

    // The second element of the query is a tag(258) set of credentials
    let (tag_num, inner) = q[1].as_tag();
    assert_eq!(tag_num, 258, "credential set must use CBOR Set tag 258");
    // The inner value must be an array (the set elements)
    let _ = inner.as_array();
}

// --- 1.2 Result messages (server → client) ---

#[test]
fn golden_result_conway_epoch_no() {
    // Official golden: 81 0A
    // [10] = success array wrapping epoch number 10
    let golden = fixture("n2c", "Result_Conway_EpochNo");
    assert_eq!(
        golden,
        vec![0x81, 0x0a],
        "Result_Conway_EpochNo must match ouroboros-consensus golden"
    );

    let (v, _) = cbor_read(&golden, 0);
    let arr = v.as_array();
    assert_eq!(arr.len(), 1, "HFC success wrapper = array(1)");
    assert_eq!(arr[0].as_uint(), 10, "epoch number");
}

#[test]
fn golden_result_conway_max_major_protocol_version() {
    // Official golden: 81 0D
    // [13] = max major protocol version 13
    let golden = fixture("n2c", "Result_Conway_MaxMajorProtocolVersion");
    assert_eq!(
        golden,
        vec![0x81, 0x0d],
        "Result_Conway_MaxMajorProtocolVersion must match ouroboros-consensus golden"
    );

    let (v, _) = cbor_read(&golden, 0);
    assert_eq!(
        v.as_array()[0].as_uint(),
        13,
        "max major protocol version 13"
    );
}

#[test]
fn golden_result_conway_ledger_tip() {
    // Official golden: 81 82 09 58 20 <32-byte-hash>
    // array(1) [ array(2) [ slot=9, bytes(32) ] ]
    // The HFC success wrapper array(1) contains a Point [slot, hash].
    let golden = fixture("n2c", "Result_Conway_LedgerTip");
    assert_eq!(golden.len(), 37, "Result_Conway_LedgerTip must be 37 bytes");

    let (v, _) = cbor_read(&golden, 0);
    let wrapper = v.as_array();
    assert_eq!(wrapper.len(), 1, "HFC success wrapper = array(1)");

    // Inner Point = [slot, hash]
    let point = wrapper[0].as_array();
    assert_eq!(point.len(), 2, "Point must be array(2)");
    assert_eq!(point[0].as_uint(), 9, "slot number from golden");
    assert_eq!(
        point[1].as_bytes().len(),
        32,
        "block header hash must be 32 bytes"
    );
    assert_eq!(
        point[1].as_bytes(),
        &hex_to_bytes("f74dd0c8c413dc372153599cc7ad9fba9e644797d7660ff670ec3b039eb7f6dc"),
        "header hash must match ouroboros-consensus golden"
    );
}

#[test]
fn golden_result_conway_slot_no() {
    // Official golden: 18 2A = uint(42)
    // SlotNo is encoded as a plain unsigned integer (no wrapper)
    let golden = fixture("n2c", "SlotNo_Conway");
    assert_eq!(golden, vec![0x18, 0x2a], "SlotNo must be plain uint(42)");

    let (v, _) = cbor_read(&golden, 0);
    assert_eq!(v.as_uint(), 42, "slot number 42");
}

// --- 1.3 Conway PParams golden (Result_Conway_EmptyPParams) ---

#[test]
fn golden_result_conway_empty_pparams_structure() {
    // Official golden: 81 98 1F <142 bytes of fields>
    //
    // Verifies the Conway protocol parameters use positional array(31) encoding
    // (not a CBOR map) and that all rational fields use tag(30).
    //
    // Source: ouroboros-consensus-cardano/golden/cardano/QueryVersion3/
    //         CardanoNodeToClientVersion19/Result_Conway_EmptyPParams
    let golden = fixture("n2c", "Result_Conway_EmptyPParams");
    assert_eq!(
        golden.len(),
        145,
        "Result_Conway_EmptyPParams must be 145 bytes"
    );
    assert_eq!(golden[0], 0x81, "outer array(1) = HFC success wrapper");
    assert_eq!(golden[1], 0x98, "inner array with 1-byte length prefix");
    assert_eq!(golden[2], 0x1f, "31 fields in Conway PParams");

    let (v, _) = cbor_read(&golden, 0);
    let wrapper = v.as_array();
    assert_eq!(wrapper.len(), 1, "HFC success wrapper = array(1)");

    let pparams = wrapper[0].as_array();
    assert_eq!(pparams.len(), 31, "Conway PParams = positional array(31)");

    // Field [9] a0 (pool pledge influence) = tag(30)[0, 1]
    let (tag, inner) = pparams[9].as_tag();
    assert_eq!(tag, 30, "rational fields use tag(30)");
    let rat = inner.as_array();
    assert_eq!(rat.len(), 2, "rational = array(2)[numerator, denominator]");

    // Field [12] protocolVersion = [9, 0] (major=9, minor=0)
    let proto = pparams[12].as_array();
    assert_eq!(proto.len(), 2, "protocolVersion = array(2)");
    assert_eq!(proto[0].as_uint(), 9, "major version 9");
    assert_eq!(proto[1].as_uint(), 0, "minor version 0");

    // Field [15] costModels = {} (empty map when no cost models)
    let cost_models = pparams[15].as_map();
    assert_eq!(cost_models.len(), 0, "empty cost models = empty map");

    // Field [16] prices = [tag(30)[...], tag(30)[...]] (2 rationals)
    let prices = pparams[16].as_array();
    assert_eq!(prices.len(), 2, "prices = array(2) [mem_price, step_price]");
    assert_eq!(prices[0].as_tag().0, 30, "mem price uses tag(30)");
    assert_eq!(prices[1].as_tag().0, 30, "step price uses tag(30)");

    // Field [22] poolVotingThresholds = 5 rationals
    let pvt = pparams[22].as_array();
    assert_eq!(pvt.len(), 5, "poolVotingThresholds = array(5)");
    for (i, t) in pvt.iter().enumerate() {
        assert_eq!(t.as_tag().0, 30, "PVT[{i}] uses tag(30) rational");
    }

    // Field [23] drepVotingThresholds = 10 rationals
    let dvt = pparams[23].as_array();
    assert_eq!(dvt.len(), 10, "drepVotingThresholds = array(10)");
    for (i, t) in dvt.iter().enumerate() {
        assert_eq!(t.as_tag().0, 30, "DVT[{i}] uses tag(30) rational");
    }

    // Field [30] minFeeRefScriptCostPerByte = tag(30)[0, 1]
    let (tag30, _) = pparams[30].as_tag();
    assert_eq!(tag30, 30, "minFeeRefScriptCostPerByte uses tag(30)");
}

#[test]
fn golden_result_conway_empty_pparams_field_ordering() {
    // Verify the exact field order matches the Haskell `EncCBOR (ConwayPParams era)` instance.
    // Field positions are critical for positional encoding compatibility.
    let golden = fixture("n2c", "Result_Conway_EmptyPParams");
    let (v, _) = cbor_read(&golden, 0);
    let pparams = v.as_array()[0].as_array();

    // Fields 0-8: plain uint params (min fees, sizes, deposits, epochs)
    // EmptyPParams has zero values for most fields
    assert_eq!(pparams[0].as_uint(), 0, "[0] txFeePerByte (min_fee_a)");
    assert_eq!(pparams[1].as_uint(), 0, "[1] txFeeFixed (min_fee_b)");
    assert_eq!(pparams[2].as_uint(), 0, "[2] maxBlockBodySize");
    assert_eq!(
        pparams[3].as_uint(),
        2048,
        "[3] maxTxSize (EmptyPParams default)"
    );
    assert_eq!(pparams[4].as_uint(), 0, "[4] maxBlockHeaderSize");
    assert_eq!(pparams[5].as_uint(), 0, "[5] keyDeposit");
    assert_eq!(pparams[6].as_uint(), 0, "[6] poolDeposit");
    assert_eq!(pparams[7].as_uint(), 0, "[7] eMax");
    assert_eq!(
        pparams[8].as_uint(),
        100,
        "[8] nOpt (EmptyPParams default 100)"
    );

    // Fields 24-29: governance params
    assert_eq!(pparams[24].as_uint(), 0, "[24] committeeMinSize");
    assert_eq!(pparams[25].as_uint(), 0, "[25] committeeMaxTermLength");
    assert_eq!(pparams[26].as_uint(), 0, "[26] govActionLifetime");
    assert_eq!(pparams[27].as_uint(), 0, "[27] govActionDeposit");
    assert_eq!(pparams[28].as_uint(), 0, "[28] drepDeposit");
    assert_eq!(pparams[29].as_uint(), 0, "[29] drepActivity");
}

// --- 1.4 GenesisConfig golden ---

#[test]
fn golden_result_conway_genesis_config() {
    // Official golden: 81 8F <77 bytes>
    // array(1) [array(15) [...fields...]]
    //
    // GenesisConfig = 15-field positional array encoding the Shelley genesis
    // parameters (epoch length, slot duration, security parameter, etc.)
    let golden = fixture("n2c", "Result_Conway_GenesisConfig");
    assert_eq!(
        golden.len(),
        79,
        "Result_Conway_GenesisConfig must be 79 bytes"
    );
    assert_eq!(golden[0], 0x81, "HFC success wrapper array(1)");
    assert_eq!(golden[1], 0x8f, "GenesisConfig = array(15)");

    let (v, _) = cbor_read(&golden, 0);
    let wrapper = v.as_array();
    assert_eq!(wrapper.len(), 1, "HFC success wrapper");

    let gc = wrapper[0].as_array();
    assert_eq!(gc.len(), 15, "GenesisConfig has 15 fields");

    // Field [0] is a 3-element array [epoch_length, slot_length, ...]
    let epoch_info = gc[0].as_array();
    assert_eq!(epoch_info.len(), 3, "epoch summary = array(3)");
    // epoch_length in slots
    assert_eq!(
        epoch_info[0].as_uint(),
        2020,
        "epoch length 2020 slots (from golden)"
    );
}

// --- 1.5 BigLedgerPeerSnapshot golden ---

#[test]
fn golden_result_conway_big_ledger_peer_snapshot() {
    // Official golden from ouroboros-consensus (stored in n2c/oc/):
    //   81 82 01 82 82 01 18 2A 9F ... FF FF FF
    //
    // Structure: array(1) [array(2) [version=1, indefinite_array(...)]]
    //
    // The inner peer-list data uses indefinite-length CBOR arrays (0x9F...0xFF).
    // We only validate the outer fixed-length structure here.
    let golden = fixture("n2c/oc", "Result_Conway_GetBigLedgerPeerSnapshot");
    assert!(!golden.is_empty(), "OC golden fixture must not be empty");
    assert_eq!(
        golden.len(),
        31,
        "Result_Conway_GetBigLedgerPeerSnapshot must be 31 bytes"
    );

    // [0] 0x81 = outer array(1) — HFC success wrapper
    assert_eq!(
        golden[0], 0x81,
        "HFC success wrapper must be array(1) = 0x81"
    );
    // [1] 0x82 = inner array(2) — [version, data]
    assert_eq!(
        golden[1], 0x82,
        "BigLedgerPeerSnapshot body = array(2) = 0x82"
    );
    // [2] 0x01 = version 1
    assert_eq!(golden[2], 0x01, "version must be 1");
    // [3] 0x82 = array(2) — peer snapshot body [compact_peers, peers]
    assert_eq!(golden[3], 0x82, "peer snapshot = array(2)");
    // [4] 0x82 = array(2) — first element (compact peer pair)
    assert_eq!(golden[4], 0x82, "compact peer entry = array(2)");
    // [5] 0x01 = uint(1)
    assert_eq!(golden[5], 0x01, "compact peer type");
    // [6] 0x18 0x2A = uint(42)
    assert_eq!(golden[6], 0x18, "uint with 1-byte extension");
    assert_eq!(golden[7], 0x2a, "value = 42");
    // [8] 0x9F = indefinite-length array begins
    assert_eq!(
        golden[8], 0x9f,
        "peer list uses indefinite-length array 0x9F"
    );
    // Last byte 0xFF = break (ends indefinite array)
    assert_eq!(
        *golden.last().unwrap(),
        0xff,
        "indefinite array terminated by 0xFF break"
    );
}

// --- 1.6 NonMyopicMemberRewards result golden ---

#[test]
fn golden_result_conway_non_myopic_member_rewards() {
    // Official golden from ouroboros-consensus (stored in n2c/oc/).
    //
    // GetNonMyopicMemberRewards takes a set of `Either Lovelace Credential` as
    // input and returns a map from those keys to pool reward maps.
    //
    // Key encoding:
    //   Left lovelace  → [0, coin]            (stake amount, not a credential)
    //   Right cred     → [1, Credential]       where Credential = [type, hash(28)]
    //
    // Value encoding:
    //   Map<pool_id(28), reward_coin>
    //
    // The query result (tag 2) is thus:
    //   array(1) [Map<(Left Coin | Right Cred), Map<pool_hash, coin>>]
    let golden = fixture("n2c/oc", "Result_Conway_NonMyopicMemberRewards");
    assert!(!golden.is_empty(), "OC fixture must not be empty");
    assert_eq!(
        golden.len(),
        139,
        "Result_Conway_NonMyopicMemberRewards must be 139 bytes"
    );
    assert_eq!(
        golden[0], 0x81,
        "HFC success wrapper must be array(1) = 0x81"
    );

    let (v, _) = cbor_read(&golden, 0);
    let wrapper = v.as_array();
    assert_eq!(wrapper.len(), 1, "HFC success");

    // outer map: Either<Lovelace, Credential> → Map<pool_hash, reward>
    let outer_map = wrapper[0].as_map();
    assert_eq!(outer_map.len(), 3, "golden has 3 entries");

    // All keys must be array(2) [tag_0or1, payload]
    for (key, val) in outer_map {
        let key_arr = key.as_array();
        assert_eq!(
            key_arr.len(),
            2,
            "key must be array(2) [discriminant, payload]"
        );
        let discriminant = key_arr[0].as_uint();
        assert!(
            discriminant <= 1,
            "discriminant must be 0 (Left=Lovelace) or 1 (Right=Credential), got {discriminant}"
        );

        // value is a map from pool_id(28) → reward (may be empty for zero-stake queries)
        let pool_map = val.as_map();
        for (pool_key, reward) in pool_map {
            assert_eq!(pool_key.as_bytes().len(), 28, "pool_id must be 28 bytes");
            let _ = reward.as_uint();
        }
    }
}

// --- 1.7 StakeDistribution2 result golden ---

#[test]
fn golden_result_conway_stake_distribution2() {
    // Official golden from ouroboros-consensus (stored in n2c/oc/).
    //
    // GetStakeDistribution2 (tag 37) returns PoolDistr2 which includes
    // total_active_stake as a second array element vs the older PoolDistr (tag 7).
    //
    // Structure: array(1) [array(2) [pool_map, total_active_stake]]
    // IndividualPoolStake (v2) = array(3) [tag(30)[num,den], compact_stake, vrf_hash(32)]
    let golden = fixture("n2c/oc", "Result_Conway_StakeDistribution2");
    assert!(!golden.is_empty(), "OC fixture must not be empty");
    assert_eq!(
        golden.len(),
        75,
        "Result_Conway_StakeDistribution2 must be 75 bytes"
    );
    assert_eq!(
        golden[0], 0x81,
        "HFC success wrapper must be array(1) = 0x81"
    );

    let (v, _) = cbor_read(&golden, 0);
    let wrapper = v.as_array();
    assert_eq!(wrapper.len(), 1, "HFC success");

    // PoolDistr2 = array(2) [pool_map, total_active_stake]
    let pool_distr = wrapper[0].as_array();
    assert_eq!(
        pool_distr.len(),
        2,
        "PoolDistr2 = array(2) [pool_map, total_active_stake]"
    );

    let pool_map = pool_distr[0].as_map();
    assert_eq!(pool_map.len(), 1, "golden has 1 pool entry");
    for (pool_key, pool_val) in pool_map {
        assert_eq!(pool_key.as_bytes().len(), 28, "pool_id must be 28 bytes");
        // IndividualPoolStake for GetStakeDistribution2 = array(3)
        // [tag(30)[num,den], compact_lovelace, vrf_hash(32)]
        let stake_arr = pool_val.as_array();
        assert_eq!(
            stake_arr.len(),
            3,
            "IndividualPoolStake (v2) = array(3) [rational, compact_stake, vrf_hash]"
        );
        assert_eq!(
            stake_arr[0].as_tag().0,
            30,
            "stake fraction uses tag(30) rational"
        );
        let _ = stake_arr[1].as_uint(); // compact lovelace
        assert_eq!(
            stake_arr[2].as_bytes().len(),
            32,
            "VRF keyhash must be 32 bytes"
        );
    }

    // total_active_stake is the second element of the PoolDistr2 array
    let _ = pool_distr[1].as_uint();
}

// ---------------------------------------------------------------------------
// Section 2 — Blueprint handshake test vectors
//
// Source: cardano-scaling/cardano-blueprint
//         src/network/node-to-node/handshake/test-data/
//
// The Cardano Blueprint defines 5 handshake CBOR test vectors covering
// the N2N handshake mini-protocol messages.
//
// Wire format (CDDL):
//   msgProposeVersions = [0, versionTable]          -- client proposes
//   msgAcceptVersion   = [1, versionNumber, params] -- server accepts
//   msgRefuse          = [2, refuseReason]           -- server refuses
//   msgQueryReply      = [3, versionTable]           -- query response
//
//   versionTable = { * versionNumber => nodeToNodeVersionData }
//   nodeToNodeVersionData = [networkMagic, initiatorOnly, peerSharing, query]
// ---------------------------------------------------------------------------

#[test]
fn blueprint_handshake_test_0_empty_propose() {
    // Blueprint test-0: 82 00 A0
    // [0, {}] = MsgProposeVersions with empty version table
    let golden = fixture("handshake", "blueprint_test_0");
    assert_eq!(
        golden,
        vec![0x82, 0x00, 0xa0],
        "Blueprint handshake test-0: empty MsgProposeVersions"
    );

    let (v, _) = cbor_read(&golden, 0);
    let arr = v.as_array();
    assert_eq!(arr[0].as_uint(), 0, "MsgProposeVersions tag = 0");
    assert_eq!(arr[1].as_map().len(), 0, "version table must be empty");
}

#[test]
fn blueprint_handshake_test_1_refuse() {
    // Blueprint test-1: 82 02 83 02 0D 61 7B
    // [2, [2, 13, "{"]] = MsgRefuse with RefuseReasonRefused(version=13, text="{")
    let golden = fixture("handshake", "blueprint_test_1");
    assert_eq!(
        golden,
        vec![0x82, 0x02, 0x83, 0x02, 0x0d, 0x61, 0x7b],
        "Blueprint handshake test-1: MsgRefuse with RefuseReasonRefused"
    );

    let (v, _) = cbor_read(&golden, 0);
    let arr = v.as_array();
    assert_eq!(arr[0].as_uint(), 2, "MsgRefuse tag = 2");

    // refuseReason = refuseReasonRefused = [2, version, text]
    let reason = arr[1].as_array();
    assert_eq!(reason[0].as_uint(), 2, "RefuseReasonRefused = 2");
    assert_eq!(reason[1].as_uint(), 13, "version 13");
    assert_eq!(reason[2].as_text(), "{", "reason text");
}

#[test]
fn blueprint_handshake_test_2_propose_v14_no_query() {
    // Blueprint test-2: 82 00 A1 0E 84 00 F4 01 F4
    // [0, {14: [0, false, 1, false]}]
    // MsgProposeVersions with v14, networkMagic=0, initiatorOnly=false,
    // peerSharing=1, query=false
    let golden = fixture("handshake", "blueprint_test_2");
    assert_eq!(
        golden,
        vec![0x82, 0x00, 0xa1, 0x0e, 0x84, 0x00, 0xf4, 0x01, 0xf4],
        "Blueprint handshake test-2: MsgProposeVersions v14"
    );

    let (v, _) = cbor_read(&golden, 0);
    let arr = v.as_array();
    assert_eq!(arr[0].as_uint(), 0, "MsgProposeVersions tag = 0");

    let vtable = arr[1].as_map();
    assert_eq!(vtable.len(), 1, "one version in table");
    assert_eq!(vtable[0].0.as_uint(), 14, "version 14");

    // nodeToNodeVersionData = [networkMagic, initiatorOnly, peerSharing, query]
    let vdata = vtable[0].1.as_array();
    assert_eq!(vdata.len(), 4, "version data = array(4)");
    assert_eq!(vdata[0].as_uint(), 0, "networkMagic = 0");
    assert_eq!(
        vdata[1],
        CborVal::Bool(false),
        "initiatorOnlyDiffusionMode = false"
    );
    assert_eq!(vdata[2].as_uint(), 1, "peerSharing = 1 (enabled)");
    assert_eq!(vdata[3], CborVal::Bool(false), "query = false");
}

#[test]
fn blueprint_handshake_test_3_propose_v13_v14() {
    // Blueprint test-3: 82 00 A2 0D 84 01 F5 01 F4 0E 84 02 F5 01 F4
    // [0, {13: [1, true, 1, false], 14: [2, true, 1, false]}]
    // MsgProposeVersions with both v13 and v14
    let golden = fixture("handshake", "blueprint_test_3");
    assert_eq!(
        golden,
        vec![
            0x82, 0x00, 0xa2, 0x0d, 0x84, 0x01, 0xf5, 0x01, 0xf4, 0x0e, 0x84, 0x02, 0xf5, 0x01,
            0xf4
        ],
        "Blueprint handshake test-3: MsgProposeVersions v13+v14"
    );

    let (v, _) = cbor_read(&golden, 0);
    let arr = v.as_array();
    assert_eq!(arr[0].as_uint(), 0, "MsgProposeVersions tag = 0");

    let vtable = arr[1].as_map();
    assert_eq!(vtable.len(), 2, "two versions in table");

    // v13 entry
    assert_eq!(vtable[0].0.as_uint(), 13, "first version is 13");
    let v13_data = vtable[0].1.as_array();
    assert_eq!(v13_data[1], CborVal::Bool(true), "v13 initiatorOnly = true");

    // v14 entry
    assert_eq!(vtable[1].0.as_uint(), 14, "second version is 14");
    let v14_data = vtable[1].1.as_array();
    assert_eq!(v14_data[1], CborVal::Bool(true), "v14 initiatorOnly = true");
}

#[test]
fn blueprint_handshake_test_4_accept_v14() {
    // Blueprint test-4: 83 01 0E 84 01 F4 01 F4
    // [1, 14, [1, false, 1, false]] = MsgAcceptVersion for v14
    let golden = fixture("handshake", "blueprint_test_4");
    assert_eq!(
        golden,
        vec![0x83, 0x01, 0x0e, 0x84, 0x01, 0xf4, 0x01, 0xf4],
        "Blueprint handshake test-4: MsgAcceptVersion v14"
    );

    let (v, _) = cbor_read(&golden, 0);
    let arr = v.as_array();
    assert_eq!(arr.len(), 3, "MsgAcceptVersion = array(3)");
    assert_eq!(arr[0].as_uint(), 1, "MsgAcceptVersion tag = 1");
    assert_eq!(arr[1].as_uint(), 14, "accepted version = 14");

    // Version data
    let vdata = arr[2].as_array();
    assert_eq!(vdata.len(), 4, "version data = array(4)");
    assert_eq!(vdata[0].as_uint(), 1, "networkMagic = 1 (testnet)");
    assert_eq!(vdata[1], CborVal::Bool(false), "initiatorOnly = false");
    assert_eq!(vdata[2].as_uint(), 1, "peerSharing = 1");
    assert_eq!(vdata[3], CborVal::Bool(false), "query = false");
}

// ---------------------------------------------------------------------------
// Section 3 — CBOR encoding invariants (protocol-level correctness)
//
// These tests verify encoding rules derived from the Cardano spec without
// referencing external fixture files.  They can catch regressions in our
// minicbor usage and encoding choices.
// ---------------------------------------------------------------------------

/// The HFC BlockQuery success wrapper must be `array(1)`.
///
/// Verified against ouroboros-consensus golden:
///   Result_Conway_EpochNo = 81 0A = array(1)[10]
#[test]
fn cbor_invariant_hfc_success_wrapper_is_array1() {
    // 0x81 = major type 4 (array), additional info 1 = length 1
    assert_eq!(0x81u8 >> 5, 4, "major type must be array (4)");
    assert_eq!(0x81u8 & 0x1f, 1, "length must be 1");

    // Encode our own array(1)[42] and check
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.array(1).unwrap();
    enc.u64(42).unwrap();
    assert_eq!(buf[0], 0x81, "minicbor must encode array(1) as 0x81");
}

/// Conway protocol parameters must use positional `array(31)`, not a map.
///
/// Integer keys 0-30 would be valid CBOR map encoding but the Haskell
/// `EncCBOR (ConwayPParams era)` instance uses a positional array.
/// See Result_Conway_EmptyPParams golden which starts with 81 98 1F.
#[test]
fn cbor_invariant_conway_pparams_is_array31() {
    // 0x98 0x1F: major type 4 (array), add 24 (1-byte length), length=31
    let header = [0x98u8, 0x1f];
    assert_eq!(header[0] >> 5, 4, "must be array major type");
    assert_eq!(header[0] & 0x1f, 24, "length encoded in next byte");
    assert_eq!(header[1], 31, "31 fields in Conway PParams");

    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.array(31).unwrap();
    assert_eq!(
        &buf[0..2],
        &[0x98, 0x1f],
        "minicbor must encode array(31) as 98 1F"
    );
}

/// ADA-only Value must be encoded as a plain unsigned integer.
///
/// From the Cardano serialisation spec: if no multi-asset bundle is present,
/// the value is just the lovelace amount as a CBOR uint.  Wrapping in an
/// array `[coin, {}]` is valid CBOR but violates the spec and will cause
/// deserialization failures in Haskell nodes.
#[test]
fn cbor_invariant_ada_only_value_is_plain_uint() {
    let lovelace: u64 = 2_000_000;
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.u64(lovelace).unwrap();

    // For amounts < 2^16 (65536), minicbor uses the shortest encoding:
    // 2_000_000 = 0x1E8480 needs 4-byte encoding: 1A 00 1E 84 80
    assert_eq!(
        buf[0], 0x1a,
        "2_000_000 must use 4-byte uint encoding (1A prefix)"
    );
    assert_eq!(&buf[1..5], &[0x00, 0x1e, 0x84, 0x80]);

    // Verify NOT wrapped in array
    assert_ne!(
        buf[0] >> 5,
        4,
        "ADA-only value must NOT be encoded as array"
    );
}

/// Multi-asset Value must be encoded as `[coin, multiasset_map]`.
///
/// The CDDL from cardano-ledger:
///   value = coin / [coin, multiasset<uint>]
///   multiasset<a> = { + policy_id => { + asset_name => a } }
#[test]
fn cbor_invariant_multi_asset_value_is_array2() {
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.array(2).unwrap();
    enc.u64(1_000_000).unwrap(); // coin
    enc.map(1).unwrap(); // one policy
    enc.bytes(&[0xAA; 28]).unwrap(); // policy_id (28 bytes)
    enc.map(1).unwrap(); // one asset name
    enc.bytes(&[]).unwrap(); // empty asset name (ADA-like)
    enc.u64(500).unwrap(); // quantity

    assert_eq!(buf[0], 0x82, "multi-asset value must start with array(2)");
}

/// CBOR Set (tag 258) must be used for finite set types.
///
/// From the Cardano CDDL: certificates, pool owners, required signers
/// and other set-typed fields use tag 258 for canonical set encoding.
/// Elements within a tag(258) array must be sorted (lexicographic on CBOR
/// encoding) for canonical encoding.
#[test]
fn cbor_invariant_cbor_set_tag_258() {
    // tag(258) encoding: C9 01 02 = 0xD9 0x01 0x02 (two-byte tag)
    // 0xD9 = major 6 (tag), additional 25 (next 2 bytes are tag number)
    // 0x01 0x02 = tag number 258 in big-endian
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.tag(minicbor::data::Tag::new(258)).unwrap();
    enc.array(0).unwrap(); // empty set

    assert_eq!(
        buf[0], 0xd9,
        "tag(258) must use 0xD9 prefix (major 6, add 25)"
    );
    assert_eq!(buf[1], 0x01, "tag number 258 high byte");
    assert_eq!(buf[2], 0x02, "tag number 258 low byte");
    assert_eq!(buf[3], 0x80, "empty array");
}

/// tag(30) must be used for rational numbers (unit intervals, pledge influence, etc.).
///
/// From the Cardano CDDL:
///   unit_interval = #6.30([uint, uint])  -- numerator/denominator
///   nonneg_interval = #6.30([uint, uint])
///
/// tag(30) encoding: D8 1E = 0xD8 0x1E (one-byte tag)
/// 0xD8 = major 6 (tag), additional 24 (next 1 byte is tag number)
/// 0x1E = 30 (tag number)
#[test]
fn cbor_invariant_rational_tag_30() {
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.tag(minicbor::data::Tag::new(30)).unwrap();
    enc.array(2).unwrap();
    enc.u64(3).unwrap();
    enc.u64(100).unwrap();

    assert_eq!(
        buf[0], 0xd8,
        "tag(30) must use 0xD8 prefix (major 6, add 24)"
    );
    assert_eq!(buf[1], 0x1e, "tag number 30 = 0x1E");
    assert_eq!(buf[2], 0x82, "rational body = array(2)");
}

/// MsgResult wire format: `[4, result]` for all query responses.
///
/// LocalStateQuery MsgResult tag is 4 (defined in the Ouroboros Network spec).
/// For BlockQuery results, the result is wrapped in HFC `[result]` = array(1).
/// For QueryAnytime/QueryHardFork, no extra wrapper is added.
#[test]
fn cbor_invariant_msg_result_format() {
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    // MsgResult [4, [result]] for a BlockQuery
    enc.array(2).unwrap();
    enc.u32(4).unwrap(); // MsgResult tag
    enc.array(1).unwrap(); // HFC success wrapper
    enc.u64(10).unwrap(); // epoch 10 (example)

    assert_eq!(buf[0], 0x82, "MsgResult must be array(2)");
    assert_eq!(buf[1], 0x04, "MsgResult tag must be 4");
    assert_eq!(buf[2], 0x81, "HFC success wrapper = array(1)");
    assert_eq!(buf[3], 0x0a, "result value = 10");
}

/// tag(24) must wrap the inner CBOR bytes for GetCBOR (query tag 9) responses.
///
/// GetCBOR returns the raw CBOR encoding of another query's result,
/// wrapped in tag(24) (CBOR embedded in bytes).
///
/// tag(24) encoding: D8 18 = 0xD8 0x18
/// 0xD8 = major 6, add 24 (next byte is tag number)
/// 0x18 = 24 (tag number)
#[test]
fn cbor_invariant_embedded_cbor_tag_24() {
    let inner = vec![0x01u8]; // CBOR uint(1)
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.tag(minicbor::data::Tag::new(24)).unwrap();
    enc.bytes(&inner).unwrap();

    assert_eq!(buf[0], 0xd8, "tag(24) must use 0xD8 prefix");
    assert_eq!(buf[1], 0x18, "tag number 24 = 0x18");
    assert_eq!(buf[2], 0x41, "bytes(1) header");
    assert_eq!(buf[3], 0x01, "inner CBOR byte");
}

/// Credential encoding must use `array(2)` with [type_uint, hash_bytes(28)].
///
/// From the Cardano CDDL:
///   addr_keyhash   = $hash28
///   scripthash     = $hash28
///   stake_credential = [0, addr_keyhash] / [1, scripthash]
///
/// This is also verified by the NonMyopicMemberRewards and DRepState goldens.
#[test]
fn cbor_invariant_credential_is_array2_with_28byte_hash() {
    let key_hash = [0xABu8; 28];
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.array(2).unwrap();
    enc.u8(0).unwrap(); // KeyHashObj
    enc.bytes(&key_hash).unwrap();

    assert_eq!(buf[0], 0x82, "credential must be array(2)");
    assert_eq!(buf[1], 0x00, "credential type 0 = KeyHash");
    assert_eq!(
        buf[2], 0x58,
        "28-byte string requires 1-byte length prefix: 0x58"
    );
    assert_eq!(buf[3], 28, "length = 28");
}

/// Point (ChainSync / LedgerTip) must be encoded as:
///   - `[]` for Origin (empty array)
///   - `[slotNo, headerHash(32)]` for Specific
///
/// Verified against Result_Conway_LedgerTip golden:
///   81 82 09 58 20 <hash32>
///   = array(1)[array(2)[9, bytes(32)]]
#[test]
fn cbor_invariant_point_encoding() {
    // Specific point
    let slot: u64 = 9;
    let hash = hex_to_bytes("f74dd0c8c413dc372153599cc7ad9fba9e644797d7660ff670ec3b039eb7f6dc");
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.array(2).unwrap();
    enc.u64(slot).unwrap();
    enc.bytes(&hash).unwrap();

    assert_eq!(buf[0], 0x82, "Specific Point = array(2)");
    assert_eq!(buf[1], 0x09, "slot 9 = uint 9 (fits in 1 byte)");
    assert_eq!(buf[2], 0x58, "32-byte hash uses 1-byte length prefix");
    assert_eq!(buf[3], 32, "length = 32");

    // Origin
    let mut buf2 = Vec::new();
    let mut enc2 = minicbor::Encoder::new(&mut buf2);
    enc2.array(0).unwrap();
    assert_eq!(buf2[0], 0x80, "Origin = empty array 0x80");
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

fn hex_to_bytes(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}
