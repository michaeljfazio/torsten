---
name: N2C Hash32 Padding Convention
description: The ledger uses Hash32 (32-byte padded) as HashMap keys for 28-byte credential/pool hashes; always truncate to 28 bytes in N2C wire output.
type: reference
---

## The Padding Convention

The ledger stores Blake2b-224 (28-byte) hashes as `Hash32` (zero-padded to 32 bytes)
for use as uniform `HashMap` keys. The padding is done via `Hash28::to_hash32_padded()`:

```rust
pub fn to_hash32_padded(&self) -> Hash32 {
    let mut bytes = [0u8; 32];
    bytes[..28].copy_from_slice(self.as_bytes());
    Hash32::from_bytes(bytes)
}
```

## Affected LedgerState Fields (Hash32 padded from 28-byte hashes)

These are the ledger state fields that use padded Hash32 as keys/values:

| Field | Type | Notes |
|-------|------|-------|
| `delegations` | `HashMap<Hash32, Hash28>` | keys are padded credential hashes |
| `reward_accounts` | `HashMap<Hash32, Lovelace>` | keys are padded credential hashes |
| `governance.dreps` | `HashMap<Hash32, DRepRegistration>` | keys are padded DRep credential hashes |
| `governance.vote_delegations` | `HashMap<Hash32, DRep>` | keys are padded stake credential hashes; `DRep::KeyHash` value also padded |
| `governance.committee_hot_keys` | `HashMap<Hash32, Hash32>` | both cold and hot creds are padded |
| `governance.committee_expiration` | `HashMap<Hash32, EpochNo>` | cold creds are padded |
| `governance.committee_resigned` | `HashMap<Hash32, Option<Anchor>>` | cold creds are padded |
| `script_stake_credentials` | `HashSet<Hash32>` | lookup only, not output |
| `script_committee_credentials` | `HashSet<Hash32>` | lookup only, not output |

## Correct-size Fields (Hash28, already 28 bytes)

These do NOT need truncation:

| Field | Type | Notes |
|-------|------|-------|
| `pool_params` | `HashMap<Hash28, PoolRegistration>` | already 28 bytes |
| `pending_retirements` | `BTreeMap<EpochNo, Vec<Hash28>>` | already 28 bytes |
| `epoch_blocks_by_pool` | `HashMap<Hash28, u64>` | already 28 bytes |
| `PoolRegistration.owners` | `Vec<Hash28>` | already 28 bytes |
| `DRep::ScriptHash` | `Hash28` (ScriptHash) | already 28 bytes |
| `delegations` value | `Hash28` pool ID | already 28 bytes |

## The Fix Pattern

When building `NodeStateSnapshot` in `update_query_state()`, use the helper:

```rust
fn hash32_padded_to_28_bytes(h: &Hash32) -> Vec<u8> {
    h.as_ref()[..28].to_vec()
}
```

Apply to all credential/pool hash fields sourced from the `Hash32`-keyed maps above.

## Why This Bug Happens

When `Vote::as_ref().to_vec()` is called on a padded Hash32, it returns 32 bytes.
The Cardano N2C wire format expects exactly 28 bytes for all credentials, pool IDs,
and DRep credential hashes (all are Blake2b-224 = 28-byte hashes).

cardano-cli rejects 32-byte hashes with: "hash bytes wrong size, expected 28 but got 32"

## Fixed In

Commit e8b58c9: "Fix N2C hash encoding: truncate padded Hash32 to 28 bytes for credentials/pool IDs"
GitHub issue #97.

Files changed:
- `crates/torsten-node/src/node/query.rs` — fix all snapshot field assignments
- `crates/torsten-network/src/n2c/query/encoding.rs` — 16 regression tests
