# GetEraHistory / GetInterpreter Wire Format

## Query Encoding (Client → Server)
Full MsgQuery: `[3, [2, 0, [2, 2, [1, 0]]]]`
- Outer: `[2, 0, block_query]` — tag 0 = BlockQuery
- HFC: `[2, 2, hf_query]` — tag 2 = QueryHardFork
- Inner: `[1, 0]` — tag 0 = GetInterpreter (tag 1 = GetCurrentEra)

## Response Encoding
QueryHardFork results are NOT wrapped in HFC success `[result]` envelope.
Direct encoding: `[4, <list_of_era_summaries>]` (MsgResult tag + raw list)

## EraSummary = array(3)
`[bound_start, era_end, era_params]`

## Bound = array(3) (pre-Peras) or array(4) (Peras)
`[relative_time, slot_no, epoch_no]`
- relative_time: Pico (Fixed E12) as raw Integer, 1 second = 10^12
- slot_no: Word64
- epoch_no: Word64

## EraEnd
- Bounded: Bound encoding (same as above)
- Unbounded: CBOR null (0xf6)

## EraParams = array(4) (pre-Peras) or array(5)
`[epoch_size, slot_length, safe_zone, genesis_window]`
- epoch_size: Word64
- slot_length: Integer in MILLISECONDS (1s = 1000)
- safe_zone: see below
- genesis_window: Word64

## SafeZone
- StandardSafeZone(n): `[3, 0, n, [1, 0]]` — [1,0] is legacy compat field
- UnsafeIndefiniteSafeZone: `[1, 1]`

## Key Source Files
- Summary.hs: Bound/EraEnd/EraSummary/Summary Serialise instances
- EraParams.hs: EraParams/SafeZone Serialise instances
- Qry.hs:471: `newtype Interpreter xs = Interpreter (Summary xs)`
- SerialiseNodeToClient.hs:509: QueryHardFork uses direct encoding (no EitherMismatch)
- cardano-slotting Time.hs: RelativeTime=Pico integer, SlotLength=milliseconds
- cardano-binary ToCBOR.hs:503: Fixed a encoded as raw Integer

## Mainnet Example Values
Byron: epochSize=21600, slotLen=20000ms, safeZone=Standard(864000), genesisWin=36000
Shelley+: epochSize=432000, slotLen=1000ms, genesisWin=36000
Conway (final): safeZone=UnsafeIndefiniteSafeZone, eraEnd=null
