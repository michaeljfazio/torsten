---
name: blueprint-test-vectors
description: Cardano Blueprint available test data and vectors — ledger conformance tests, handshake test data, LocalStateQuery examples, UTCTime encoding mismatch
type: reference
---

# Cardano Blueprint — Test Vectors & Test Data

## Complete File Inventory (as of 2026-03-13, main SHA 7032942f)

```
src/ledger/conformance-test-vectors/README.md
src/ledger/conformance-test-vectors/vectors.tar.gz
src/network/node-to-node/handshake/test-data/test-0   (hex text: 8200a0)
src/network/node-to-node/handshake/test-data/test-1   (hex text: 820283020d617b)
src/network/node-to-node/handshake/test-data/test-2   (hex text: 8200a10e8400f401f4)
src/network/node-to-node/handshake/test-data/test-3   (hex text: 8200a20d8401f501f40e8402f501f4)
src/network/node-to-node/handshake/test-data/test-4   (hex text: 83010e8401f401f4)
src/client/node-to-client/state-query/examples/getSystemStart/query.cbor   (hex text: 82038101)
src/client/node-to-client/state-query/examples/getSystemStart/result.cbor  (hex text: 820483c2581e...)
```

Note: ALL test data files store CBOR as ASCII hex text (not raw binary). GitHub API returns
them base64-encoded, and decoding gives the hex string.

---

## Ledger Conformance Test Vectors

### Location
`src/ledger/conformance-test-vectors/`
- `README.md` — Description and generation instructions
- `vectors.tar.gz` — The actual test data (binary, download from GitHub)

### What They Cover
- **Conway era** ledger state transitions only
- Each vector = one transaction + "before" ledger state + "after" ledger state
- Vectors grouped into directories by unit test that generated them
- Numbered sequentially within each group
- Removed: Ledger V9 tests, `BodyRefScriptsSizeTooBig`, `TxRefScriptsSizeTooBig` (too large)

### Protocol Parameters Optimization
To reduce size, protocol parameter records are stored **by hash**:
- Ledger states reference protocol params by `Hash` instead of inline
- All unique parameter records stored in `pparams-by-hash/` directory

### Generation Source
Generated from SundaeSwap fork of cardano-ledger:
```
git clone git@github.com:SundaeSwap-finance/cardano-ledger-conformance-tests.git
cd cardano-ledger-conformance-tests
git checkout 34365e427e6507442fd8079ddece9f4a565bf1b9
cabal test cardano-ledger-conway
tar czf vectors.tar.gz eras/conway/impl/dump/*
```
Original discussion: https://github.com/IntersectMBO/cardano-ledger/issues/4892#issuecomment-2880444621

### Test Vector Format
1. Starting ledger state (CBOR encoded)
2. One or more transactions (CBOR encoded)
3. Expected resulting ledger state OR expected validation error

---

## Handshake Test Data (NTN)

### Location
`src/network/node-to-node/handshake/test-data/`
5 test cases, each is a hex-text file containing CBOR:

| File   | Hex                              | Decoded                                      | Meaning |
|--------|----------------------------------|----------------------------------------------|---------|
| test-0 | `8200a0`                         | `[0, {}]`                                    | MsgProposeVersions with empty version table |
| test-1 | `820283020d617b`                 | `[2, [2, 13, "{"]]`                          | MsgRefuse: HandshakeDecodeError, version 13, error `{` |
| test-2 | `8200a10e8400f401f4`             | `[0, {14: [0, false, 1, false]}]`            | MsgProposeVersions: v14, magic=0, initiatorOnly=false, peerSharing=1, query=false |
| test-3 | `8200a20d8401f501f40e8402f501f4` | `[0, {13: [1,true,1,false], 14: [2,true,1,false]}]` | MsgProposeVersions: v13 (magic=1, initOnly=true) and v14 (magic=2, initOnly=true) |
| test-4 | `83010e8401f401f4`               | `[1, 14, [1, false, 1, false]]`              | MsgAcceptVersion: version 14, magic=1, initiatorOnly=false, peerSharing=1, query=false |

Key observations:
- versionData field order: `[networkMagic, initiatorOnlyDiffusionMode, peerSharing, query]`
- `false` = CBOR `f4`, `true` = CBOR `f5`
- peerSharing encoded as integer (0 or 1), NOT bool
- Message type tags: 0=MsgProposeVersions, 1=MsgAcceptVersion, 2=MsgRefuse, 3=MsgQueryReply

---

## LocalStateQuery Examples (GetSystemStart)

### Location
`src/client/node-to-client/state-query/examples/getSystemStart/`

### Query CBOR
Hex: `82038101`
Decoded: `[3, [1]]`
Meaning: MsgQuery (tag 3), inner query `[1]` = GetSystemStart (outer tag 1 in LSQ)

### Result CBOR
Hex: `820483c2581e65fea62360470c59141d0ba6cc897f99e050184606937264a1f8c5026abc3b3a5d754770442481c3581e50670ee65e805e3cc5aadf6619e791db8b1c2237dd918ba3b6818e7c258a`

Structure:
```
82 04         = array(2), 4         [MsgResult tag=4, payload]
83            = array(3)             [UTCTime = 3 fields]
c2 58 1e ...  = tag(2) bytes(30)    year (positive bignum, 30 bytes)
3b ...        = neg uint 8-byte     dayOfYear (negative, 8-byte)
c3 58 1e ...  = tag(3) bytes(30)    timeOfDayPico (negative bignum, 30 bytes)
```

### CRITICAL: UTCTime Encoding Mismatch

**dugite's current encoding** (encode_system_start in state_query.rs):
- Produces: `8204831907e6185b00` for 2022-04-01T00:00:00Z
- = `[4, [2022, 91, 0]]` — plain u64 integers

**Haskell cardano-node encoding** (Blueprint reference vector):
- Uses CBOR tag(2)/tag(3) bignums (30 bytes each)
- Values are NOT calendar year/day/pico — they are Haskell internal UTCTime representation
- The Blueprint CDDL (`year = bigint, dayOfYear = int, timeOfDayPico = bigint`) documents this

**This is a known conformance gap** — dugite sends `[year_u64, day_u64, pico_u64]` but cardano-node
sends `[tag(2) bignum, neg_int, tag(3) bignum]`. cardano-cli likely rejects our GetSystemStart
response. Fix requires implementing Haskell cborg UTCTime encoding.

The Haskell `serialise` package UTCTime encoding uses the `time` library's internal binary
representation, not a human-readable [year, dayOfYear, pico] decomposition.

---

## Transaction Fee Worked Example

In `src/ledger/transaction-fee.md`:
- **TxID**: `f06e17af7b0085b44bcc13f76008202c69865795841c692875810bc92948d609`
- **Era**: Conway, Protocol version 10, Mainnet
- **Tx size**: 1358 bytes
- **Reference scripts**: 2 scripts (2469 + 15728 = 18197 bytes total)
- **Redeemers**: 3 (Spend#0: 1057954mem/335346191steps, Spend#2: 28359mem/8270119steps, Withdraw#0: 40799mem/12323280steps)
- **Protocol params**: minFeeConstant=155381, minFeeCoefficient=44, minFeeRefScripts base=15/mult=1.2/range=25600
- **Computed min fee**: 578,786 lovelace (base=215133 + refscripts=272955 + exec=90698)
- **On-chain declared fee**: 601,677 lovelace

Full tx hex is in the document; use it to test CIP-0112 tiered ref-script fee calculation.

---

## Related External Test Resources

- **Plutus conformance tests**: https://github.com/IntersectMBO/plutus/tree/master/plutus-conformance
- **Aiken acceptance tests**: https://github.com/aiken-lang/aiken/tree/main/examples/acceptance_tests/script_context/v3
- **Cardano-ledger Imp tests**: https://github.com/IntersectMBO/cardano-ledger/blob/master/eras/conway/impl/testlib/Test/Cardano/Ledger/Conway/Imp.hs
- **Cardano-ledger conformance test suite**: https://github.com/IntersectMBO/cardano-ledger/tree/master/libs/cardano-ledger-conformance
- **Ethereum tests (inspiration model)**: https://github.com/ethereum/tests

---

## What Is NOT Available in Blueprint

- No consensus test vectors (VRF, KES, opcerts, block headers)
- No ChainSync/BlockFetch/TxSubmission2/KeepAlive protocol test vectors
- No NTC mini-protocol test vectors (only NTN handshake + GetSystemStart)
- No Byron era test vectors
- No pre-Conway ledger test vectors
- No chain selection test vectors
- No mempool test vectors
- No Plutus execution test vectors (those are in IntersectMBO/plutus)
- No protocol parameter wire format test vectors
