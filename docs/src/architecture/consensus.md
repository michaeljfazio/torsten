# Consensus

Torsten implements the Ouroboros Praos consensus protocol, the proof-of-stake protocol used by Cardano since the Shelley era.

## Ouroboros Praos Overview

Ouroboros Praos divides time into fixed-length slots. Each slot, a slot leader is selected based on their stake proportion. The leader is entitled to produce a block for that slot. Key properties:

- **Slot-based** -- Time is divided into slots (1 second each on mainnet)
- **Epoch-based** -- Slots are grouped into epochs (432000 slots / 5 days on mainnet)
- **Stake-proportional** -- The probability of being elected is proportional to the pool's active stake
- **Private leader selection** -- Only the pool operator knows if they are elected (until they publish the block)

## Slot Leader Election

### VRF-Based Selection

Each slot, the pool operator evaluates a VRF (Verifiable Random Function) using:
- Their VRF signing key
- The slot number
- The epoch nonce

The VRF produces:
1. A **VRF output** -- A deterministic pseudo-random value
2. A **VRF proof** -- A proof that the output was correctly computed

### Leader Threshold

The VRF output is compared against a threshold derived from:
- The pool's relative stake (sigma)
- The active slot coefficient (f = 0.05 on mainnet)

The threshold is computed using the phi function:

```
phi(sigma) = 1 - (1 - f)^sigma
```

A slot leader is elected if `VRF_output < phi(sigma)`.

### Epoch Nonce

The epoch nonce is computed at each epoch boundary:

```
nonce = hash(rolling_nonce || first_block_hash_prev_epoch)
```

Where:
- `rolling_nonce` is updated per-block: `hash(prev_eta_v || hash(vrf_output))`
- `first_block_hash_prev_epoch` is the hash of the first block in the previous epoch

The initial rolling nonce is derived from the Shelley genesis hash.

## Chain Selection

When multiple valid chains exist, Ouroboros Praos selects the chain with the most blocks (longest chain rule). Torsten implements:

1. **Chain comparison** -- Compare the block height of competing chains
2. **Rollback support** -- Roll back up to k=2160 blocks to switch to a longer chain
3. **Immutability** -- Blocks deeper than k are considered final

## Epoch Transitions

At each epoch boundary, Torsten performs:

### Stake Snapshot Rotation

Torsten uses the mark/set/go snapshot model:
- **Mark** -- The current stake distribution (used 2 epochs in the future)
- **Set** -- The previous mark (used 1 epoch in the future)
- **Go** -- The active stake distribution for the current epoch

At each epoch boundary:
1. Go becomes the active snapshot
2. Set moves to go
3. Mark moves to set
4. A new mark is taken from the current ledger state

### Reward Calculation and Distribution

At each epoch boundary, rewards are calculated and distributed:

1. **Monetary expansion** -- New ADA is created from the reserves based on the monetary expansion rate
2. **Fee collection** -- Transaction fees from the epoch are collected
3. **Treasury cut** -- A fraction (tau) of rewards goes to the treasury
4. **Pool rewards** -- Remaining rewards are distributed to pools based on their performance
5. **Member distribution** -- Pool rewards are split between the operator and delegators based on pool parameters (cost, margin, pledge)

## Validation Checks

Torsten validates the following consensus-level properties:

### KES Period Validation

The KES (Key Evolving Signature) period in the block header must be within the valid range for the operational certificate:

```
opcert_start_kes_period <= current_kes_period < opcert_start_kes_period + max_kes_evolutions
```

### VRF Verification

Full VRF verification includes:

1. **VRF key binding** -- `blake2b_256(header.vrf_vkey)` must match the pool's registered `vrf_keyhash`
2. **VRF proof verification** -- The VRF proof is cryptographically verified against the VRF public key
3. **Leader eligibility** -- The VRF leader value is checked against the Praos threshold for the pool's relative stake using the phi function

### Operational Certificate Verification

The operational certificate's Ed25519 signature is verified against the raw bytes signable format (matching Haskell's `OCertSignable`):

```
signable = hot_vkey(32 bytes) || counter(8 bytes BE) || kes_period(8 bytes BE)
signature = sign(cold_skey, signable)
```

The counter must be monotonically increasing per pool to prevent certificate replay.

### KES Signature Verification

Block headers are signed using the Sum6Kes scheme (depth-6 binary sum composition over Ed25519). The KES key is evolved to the correct period offset from the operational certificate's start period. Verification checks:

1. The KES signature over the header body bytes is valid
2. The KES period matches the expected value for the block's slot

### Slot Leader Eligibility

The VRF proof is checked to confirm the block producer was indeed elected for the slot, given the epoch nonce and their pool's stake.
