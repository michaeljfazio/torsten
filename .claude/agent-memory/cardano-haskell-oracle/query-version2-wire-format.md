---
name: QueryVersion2 Wire Format
description: Complete CBOR wire format for LocalStateQuery QueryVersion2 - three-level nesting, NS encoding, EitherMismatch result wrapping, all outer/inner tags
type: reference
---

# QueryVersion2 Wire Format

## Source Files
- Top-level Query: `ouroboros-consensus/.../Consensus/Ledger/Query.hs` (lines 421-512)
- HFC BlockQuery: `ouroboros-consensus/.../HardFork/Combinator/Serialisation/SerialiseNodeToClient.hs` (lines 420-568)
- NS/EitherMismatch: `ouroboros-consensus/.../HardFork/Combinator/Serialisation/Common.hs` (encodeNS line 409, encodeEitherMismatch line 550)
- Shelley query tags: `ouroboros-consensus-cardano/.../Shelley/Ledger/Query.hs` (lines 843-929)
- HFC Query types: `ouroboros-consensus/.../HardFork/Combinator/Ledger/Query.hs`

## Three-Level Nesting

### Level 1: Query discriminator (queryEncodeNodeToClient)
- tag 0, listlen 2: BlockQuery → `[2, 0, <hfc_query>]`
- tag 1, listlen 1: GetSystemStart → `[1, 1]`
- tag 2, listlen 1: GetChainBlockNo → `[1, 2]`
- tag 3, listlen 1: GetChainPoint → `[1, 3]`
- tag 4, listlen 1: DebugLedgerConfig → `[1, 4]` (V3+ only)

### Level 2: HFC discriminator (encodeNodeToClient for SomeBlockQuery)
- tag 0, listlen 2: QueryIfCurrent → `[2, 0, <NS-encoded>]`
- tag 1, listlen 3: QueryAnytime → `[3, 1, <query>, <era_index>]`
- tag 2, listlen 2: QueryHardFork → `[2, 2, <hf_query>]`

### Level 3: NS encoding (encodeNS)
`[2, era_idx, <payload>]`
- era 0=Byron, 1=Shelley, 2=Allegra, 3=Mary, 4=Alonzo, 5=Babbage, 6=Conway, 7=Dijkstra

### QueryAnytime inner payload
- GetEraStart: `[1, 0]`

### QueryHardFork inner payload
- GetInterpreter: `[1, 0]`
- GetCurrentEra: `[1, 1]`

## Golden Test Examples (verified from ouroboros-consensus golden files)
- Conway GetCurrentPParams: `82 00 82 06 81 03` → `[0, [6, [3]]]`
- Conway GetLedgerTip: `82 00 82 06 81 00` → `[0, [6, [0]]]`
- Shelley GetLedgerTip: `82 00 82 01 81 00` → `[0, [1, [0]]]`
- Byron query: `82 00 82 00 00` → `[0, [0, 0]]`
- AnytimeByron: `83 01 81 00 00` → `[1, [0], 0]`
- AnytimeShelley: `83 01 81 00 01` → `[1, [0], 1]`
- HardFork GetInterpreter: `82 02 81 00` → `[2, [0]]`

NOTE: These golden hex values are the HFC-level encoding ONLY (no Level 1 wrapping).
Full MsgQuery adds `[3, <full_query>]` protocol message tag.

## Result Wrapping Rules (MsgResult = [4, <result_encoding>])

### QueryIfCurrent results → EitherMismatch wrapped (encodeEitherMismatch in Common.hs)
- Success: `array(1) + <era_result>` (hex prefix `81`) — NO era index, NO NS wrapping
- Failure: `array(2) + <NS era1_name> + <NS era2_name>` (hex prefix `82`)
- CRITICAL: There is NO NS era-index wrapping on results. The server knows the era from QueryIfCurrent structure.
- Golden proof: Conway EpochNo = `81 0a`, Shelley EpochNo = `81 0a` (IDENTICAL — no era differentiation)
- Golden proof: Conway LedgerTip = `81 82 09 58 20 f7 4d...` = `[[9, hash_32bytes]]`

### QueryAnytime results → NO wrapping
- Goes through encodeQueryAnytimeResult, NOT encodeEitherMismatch
- GetEraStart returns `Maybe Bound` via Serialise.encode

### QueryHardFork results → NO wrapping
- Goes through encodeQueryHardForkResult, NOT encodeEitherMismatch
- GetInterpreter returns Interpreter directly (list of EraSummary)
- GetCurrentEra returns era index directly

### Top-level queries → NO wrapping (not BlockQuery at all)
- GetSystemStart: `toCBOR SystemStart` (text string)
- GetChainBlockNo: `toCBOR (WithOrigin BlockNo)` = `[block_no]` or `[]`
- GetChainPoint: `encodePoint encode` = `[slot, hash]` or `[]`
- WithOrigin encoding: `[] / [v]` (CDDL: `withOrigin<v> = [] / [v]`)
- Point encoding: `[] / [slotno, hash]` (CDDL: `point = [] / [slotno, hash]`)

### Full MsgResult wire examples:
- BlockQuery GetEpochNo (epoch 10): `82 04 81 0a` = `[4, [10]]`
- GetChainBlockNo (block 42000): `82 04 81 19 a4 10` = `[4, [42000]]`
- GetChainBlockNo (origin): `82 04 80` = `[4, []]`
- GetChainPoint (known): `82 04 82 <slot> 58 20 <hash>` = `[4, [slot, hash]]`
- GetChainPoint (genesis): `82 04 80` = `[4, []]`

### Dugite bug history:
- BUG: sent `[4, [6, result]]` (NS era wrapping) — wrong, should be `[4, [result]]` (EitherMismatch Right)
- BUG: sent `[4, block_no]` for GetChainBlockNo — wrong, should be `[4, [block_no]]` (WithOrigin wrapping)
- BUG: ChainTip used `[[slot,hash], block_no]` for GetChainPoint — wrong, should be `[slot, hash]` (Point, not Tip)

## Shelley Inner Query Tags (0-37)
See n2c-protocol-details.md for complete list.
Key additions beyond tag 24:
- 29: GetAccountState
- 30: GetSPOStakeDistr
- 31: GetProposals
- 32: GetRatifyState
- 33: GetFuturePParams
- 34: GetLedgerPeerSnapshot (listlen 1 or 2 depending on V15)
- 35: QueryStakePoolDefaultVote
- 36: GetPoolDistr2
- 37: GetStakeDistribution2
