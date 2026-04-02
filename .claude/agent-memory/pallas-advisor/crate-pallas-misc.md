---
name: crate-pallas-misc
description: pallas-addresses, pallas-primitives, pallas-codec, pallas-bech32, pallas-utxorpc — capabilities and dugite usage
type: reference
---

# Miscellaneous Pallas Crates

## pallas-addresses (v1.0.0-alpha.5)

Description: "Encode / decode Cardano addresses of any type"

### Public Types

```rust
pub enum Address {
    Byron(ByronAddress),
    Shelley(ShelleyAddress),
    Stake(StakeAddress),
}
pub struct ShelleyAddress {
    network: Network,
    payment: ShelleyPaymentPart,
    delegation: ShelleyDelegationPart,
}
pub struct StakeAddress { ... }
pub struct ByronAddress { ... }
pub enum ShelleyPaymentPart { Key(PaymentKeyHash), Script(ScriptHash) }
pub enum ShelleyDelegationPart { Key(StakeKeyHash), Script(ScriptHash), Pointer(Pointer), Null }
pub enum Network { Testnet, Mainnet, Other(u8) }
pub enum StakePayload { Stake(StakeKeyHash), Script(ScriptHash) }
pub struct Pointer { slot: Slot, tx_idx: TxIdx, cert_idx: CertIdx }

// Type aliases (all 28-byte hashes):
pub type PaymentKeyHash = Hash<28>
pub type StakeKeyHash = Hash<28>
pub type ScriptHash = Hash<28>
pub type Slot = u64
pub type TxIdx = u64
pub type CertIdx = u64
```

### Operations

- From/To bech32 strings
- From/To hex strings
- From/To base58 (Byron)
- From raw bytes
- Network ID detection
- Script hash detection (`is_script()`)
- Enterprise address detection

### Dugite Usage

Used in `dugite-node/src/` (from Cargo.toml) and `dugite-serialization/src/` (from Cargo.toml). Primary use is address parsing for UTxO outputs and query response formatting.

---

## pallas-primitives (v1.0.0-alpha.5)

Description: "Ledger primitives and cbor codec for the different Cardano eras"

### Era Modules

```
pallas_primitives::
  byron::   // Byron block/tx types
  alonzo::  // Alonzo block/tx types (used for Shelley, Allegra, Mary, Alonzo)
  babbage:: // Babbage block/tx types
  conway::  // Conway block/tx types
  // framework and plutus_data re-exported from root
```

### Root-level Types

```rust
pub type Hash<N> = pallas_crypto::hash::Hash<N>  // re-exported

// Primitive types:
pub type Coin = u64
pub struct ExUnits { mem: u64, steps: u64 }
pub struct ExUnitPrices { mem_price: PositiveInterval, step_price: PositiveInterval }
pub struct PoolMetadata { url: String, hash: Hash<32> }
pub struct TransactionInput { transaction_id: Hash<32>, index: u64 }
pub enum NetworkId { Testnet = 0, Mainnet = 1 }
pub enum Relay { SingleHostAddr, SingleHostName, MultiHostName }
pub struct Metadatum { ... }    // Int, Bytes, Text, Array, Map
pub struct PlutusScript<const VERSION: usize>
pub struct Nonce { variant: NonceVariant, hash: Option<Hash<32>> }
pub struct ProtocolVersion { major: u64, minor: u64 }
pub type Epoch = u64
pub struct CostModel { ... }

// BoundedBytes — bytes with size constraints
pub struct BoundedBytes(Vec<u8>)
```

### Features

- `json` (default): serde + serde_json for JSON representation
- `relaxed`: enables relaxed mode in pallas-crypto

### Dugite Usage

Heavy usage in `dugite-serialization/src/multi_era.rs`:
- Era-specific certificate types: `alonzo::Certificate`, `conway::Certificate`
- DRep: `conway::DRep`
- Governance: `conway::GovAction`, `conway::Vote`, `conway::Voter`
- PlutusData: `conway::PlutusData`, `conway::BigInt`
- Scripts: `conway::ScriptRef`
- Redeemers: `conway::RedeemerTag`
- Native scripts: `alonzo::NativeScript`
- Metadata: `Metadatum`
- Relays: `Relay`
- `BoundedBytes`
- `MaybeIndefArray` (via pallas-codec)

---

## pallas-codec (v1.0.0-alpha.5)

Description: "Shared CBOR encoding / decoding using minicbor lib"

### Public Types

```rust
// Fragment trait — types that implement both Encode and Decode with unit context
pub trait Fragment: minicbor::Decode + minicbor::Encode { }

// Re-exports:
pub use minicbor;  // the full minicbor library
```

### Utils Module (`pallas_codec::utils`)

Key utility types for round-trip CBOR:

```rust
pub struct MaybeIndefArray<A>(Vec<A>)
    // Preserves definite vs indefinite-length encoding

pub struct KeyValuePairs<K, V>(Vec<(K, V)>)
    // Ordered map preserving insertion order for canonicalization

pub struct NonEmptyKeyValuePairs<K, V>(KeyValuePairs<K, V>)
    // Enforces non-empty constraint

pub struct Nullable<T>
    // Three states: Some(T), Null, Undefined
    // More fine-grained than Option<T>
    // Note: In v1.0.0-alpha.1+ many uses replaced with Option<T>

pub struct CborWrap<T>
    // Wraps T as IANA CBOR tag (nested CBOR bytes)

pub struct Set<T>       // Optional CBOR tag 258 set
pub struct NonEmptySet<T>  // Non-empty set with tag 258

pub struct KeepRaw<'b, T>
    // Preserves original CBOR bytes during decode
    // Enables recovery of definite vs indefinite length formatting

pub struct AnyCbor
    // Arbitrary CBOR data without type commitment

pub struct OrderPreservingProperties<P>
    // Maintains entry order in map-based structures

pub struct TagWrap<I, T>   // Generic CBOR tag wrapper
pub struct EmptyMap        // Empty map representation
```

### Macro

```rust
codec_by_datatype!  // Maps CBOR data types to enum variant constructors
```

### Dugite Usage

- `MaybeIndefArray` — in multi_era.rs for CBOR arrays
- `BoundedBytes` — (actually in pallas-primitives, re-exported via codec)
- `minicbor` — re-exported for use throughout dugite-serialization

---

## pallas-bech32

Status: Appears in pallas workspace. Not confirmed published separately to crates.io. Provides Bech32 encoding/decoding utilities beyond what pallas-addresses provides. May be internal to other crates.

---

## pallas-utxorpc (v1.0.0-alpha.5)

Description: "Interoperability with the UTxO RPC specification"

Provides integration with the UTxO RPC protocol (gRPC-based API for Cardano). Not relevant to dugite's current use case (direct Ouroboros node). Would only matter if dugite wanted to expose a UTxO RPC endpoint.

**Adoption Recommendation**: Ignore for now. UTxO RPC is a separate ecosystem concern.

---

## API Quirks Relevant to Dugite

### Nullable<T> vs Option<T> Change (alpha.1)

In pallas v0.x, many fields used `Nullable<T>`. In v1.0.0-alpha.1+, these were changed to `Option<T>`. This was a breaking change dugite had to handle during the 0.x→1.x migration.

If encountering old code or documentation using `Nullable`, it's now `Option<T>` in v1.x.

### PseudoDatumOption → DatumOption (alpha.1)

`PseudoDatumOption` was renamed to `DatumOption`. Any code using the old name needs updating to `DatumOption`.

### CBOR Sets (tag 258)

`Set<T>` and `NonEmptySet<T>` use CBOR tag 258. Elements MUST be sorted for canonical encoding (pool IDs, owners, delegators). Failing to sort produces transactions that mainnet nodes may reject.

### 28-byte Hash Padding

`Hash<28>` types (DRep keys, pool voter keys, required signers) cannot be padded to `Hash<32>` via `Hash<32>::from()`. Dugite must manually extend with 4 zero bytes. This is an ongoing sharp edge.
