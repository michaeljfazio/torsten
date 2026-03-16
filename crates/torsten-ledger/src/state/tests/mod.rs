//! Test suite for the ledger state module.
//!
//! Sub-modules are organised by concern:
//! - `utxo`        — UTxO CRUD, multi-asset, apply_transaction, rollback
//! - `delegation`  — Stake registration/delegation/deregistration, pool lifecycle
//! - `rewards`     — RUPD, epoch-transition reward calculation
//! - `governance`  — DRep, CC, proposals, ratification, voting thresholds
//! - `epoch`       — Epoch boundary, mark/set/go snapshots, PP updates, nonce
//! - `snapshots`   — Snapshot save/load, magic/checksum, format stability
//! - `ebb`         — Byron EBB advance_past_ebb, chain continuity

// Make all production types from state/mod.rs visible in every sub-module.
// The #[cfg(test)] re-exports (governance helper functions) may not all be
// used by every test sub-module, so suppress the unused-import lint here.
#[allow(unused_imports)]
use super::*;

pub mod delegation;
pub mod ebb;
pub mod epoch;
pub mod governance;
pub mod rewards;
pub mod snapshots;
pub mod utxo;

// ─────────────────────────────────────────────────────────────────────────────
// Shared test helpers
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::atomic::{AtomicU64, Ordering};
use torsten_primitives::address::{Address, BaseAddress, EnterpriseAddress};
use torsten_primitives::credentials::Credential;
use torsten_primitives::hash::{Hash28, Hash32};
use torsten_primitives::network::NetworkId;
use torsten_primitives::protocol_params::ProtocolParameters;
use torsten_primitives::transaction::{OutputDatum, TransactionInput, TransactionOutput};
use torsten_primitives::value::{Lovelace, Value};

// ---------------------------------------------------------------------------
// Shared counter for unique UTxO inputs across tests
// ---------------------------------------------------------------------------

/// Global monotonic counter so each test gets a unique `TransactionInput`.
static UTXO_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Return the next unique input index (thread-safe, cast to u32).
pub fn next_utxo_idx() -> u32 {
    (UTXO_COUNTER.fetch_add(1, Ordering::Relaxed) & 0xFFFF_FFFF) as u32
}

// ---------------------------------------------------------------------------
// Address / credential helpers
// ---------------------------------------------------------------------------

/// Deterministic 32-byte hash from a small seed integer.
pub fn make_hash32(seed: u8) -> Hash32 {
    let mut bytes = [0u8; 32];
    bytes[0] = seed;
    bytes[1] = 0xde;
    bytes[2] = 0xad;
    Hash32::from_bytes(bytes)
}

/// Deterministic 28-byte pool-id hash from a small seed integer.
pub fn make_hash28(seed: u8) -> Hash28 {
    let mut bytes = [0u8; 28];
    bytes[0] = seed;
    bytes[1] = 0xbe;
    bytes[2] = 0xef;
    Hash28::from_bytes(bytes)
}

/// Verification-key credential backed by `make_hash28(seed)`.
pub fn make_key_credential(seed: u8) -> Credential {
    Credential::VerificationKey(make_hash28(seed))
}

/// Script credential backed by `make_hash28(seed)`.
pub fn make_script_credential(seed: u8) -> Credential {
    Credential::Script(make_hash28(seed))
}

/// Extract a `Hash32` from a `Credential` (mirrors the production helper).
/// Named `cred_to_hash` to avoid ambiguity with the private `credential_to_hash`
/// function in `state/mod.rs`, which is also visible in this test scope.
pub fn cred_to_hash(credential: &Credential) -> Hash32 {
    credential.to_hash().to_hash32_padded()
}

/// Enterprise address (payment only, no staking part).
pub fn make_enterprise_address(payment_seed: u8) -> Address {
    Address::Enterprise(EnterpriseAddress {
        network: NetworkId::Testnet,
        payment: make_key_credential(payment_seed),
    })
}

/// Base address with both a payment and a staking credential.
pub fn make_base_address(payment_seed: u8, stake_seed: u8) -> Address {
    Address::Base(BaseAddress {
        network: NetworkId::Testnet,
        payment: make_key_credential(payment_seed),
        stake: make_key_credential(stake_seed),
    })
}

// ---------------------------------------------------------------------------
// UTxO helpers
// ---------------------------------------------------------------------------

/// Build a `TransactionInput` with a unique, deterministic index.
pub fn make_input(seed: u8) -> TransactionInput {
    TransactionInput {
        transaction_id: make_hash32(seed),
        index: next_utxo_idx(),
    }
}

/// Build an ADA-only `TransactionOutput` to an enterprise address.
pub fn make_output(lovelace: u64) -> TransactionOutput {
    TransactionOutput {
        address: make_enterprise_address(1),
        value: Value {
            coin: Lovelace(lovelace),
            multi_asset: std::collections::BTreeMap::new(),
        },
        datum: OutputDatum::None,
        script_ref: None,
        is_legacy: false,
        raw_cbor: None,
    }
}

/// Build an ADA-only output directed to a base address (with staking part).
pub fn make_stake_output(lovelace: u64, payment_seed: u8, stake_seed: u8) -> TransactionOutput {
    TransactionOutput {
        address: make_base_address(payment_seed, stake_seed),
        value: Value {
            coin: Lovelace(lovelace),
            multi_asset: std::collections::BTreeMap::new(),
        },
        datum: OutputDatum::None,
        script_ref: None,
        is_legacy: false,
        raw_cbor: None,
    }
}

/// Insert one UTxO entry into the ledger's UTxO set and return the input used.
pub fn add_utxo(state: &mut LedgerState, lovelace: u64) -> TransactionInput {
    let input = make_input(0);
    let output = make_output(lovelace);
    state.utxo_set.insert(input.clone(), output);
    input
}

/// Insert a UTxO backed by a base address so the stake is tracked.
#[allow(dead_code)]
pub fn add_stake_utxo(
    state: &mut LedgerState,
    lovelace: u64,
    payment_seed: u8,
    stake_seed: u8,
) -> (TransactionInput, Hash32) {
    let input = TransactionInput {
        transaction_id: make_hash32(stake_seed),
        index: next_utxo_idx(),
    };
    let output = make_stake_output(lovelace, payment_seed, stake_seed);
    let stake_hash = cred_to_hash(&make_key_credential(stake_seed));

    // Track stake in stake_distribution so snapshots include it.
    *state
        .stake_distribution
        .stake_map
        .entry(stake_hash)
        .or_insert(Lovelace(0)) += Lovelace(lovelace);

    state.utxo_set.insert(input.clone(), output);
    (input, stake_hash)
}

// ---------------------------------------------------------------------------
// Pool helpers
// ---------------------------------------------------------------------------

/// Build a minimal `PoolRegistration` with the given pool seed and pledge.
pub fn make_pool_params(pool_seed: u8, pledge: u64) -> PoolRegistration {
    PoolRegistration {
        pool_id: make_hash28(pool_seed),
        vrf_keyhash: make_hash32(pool_seed),
        pledge: Lovelace(pledge),
        cost: Lovelace(340_000_000), // 340 ADA minimum cost
        margin_numerator: 1,
        margin_denominator: 100, // 1%
        // 29-byte reward account: network byte (0xe0 = testnet stake key) + 28-byte key hash
        reward_account: {
            let mut ra = vec![0xe0u8];
            ra.extend_from_slice(make_hash28(pool_seed).as_bytes());
            ra
        },
        owners: vec![make_hash28(pool_seed)],
        relays: vec![],
        metadata_url: None,
        metadata_hash: None,
    }
}

/// Build a `LedgerState` with Conway-era mainnet defaults (protocol version 10,
/// so Conway certificates are accepted).
pub fn make_ledger() -> LedgerState {
    let mut params = ProtocolParameters::mainnet_defaults();
    // Use protocol version 10 so Conway-only certificates are processed.
    params.protocol_version_major = 10;
    // Use simple reward/expansion parameters so reward tests are predictable.
    params.rho = torsten_primitives::transaction::Rational {
        numerator: 3,
        denominator: 1000,
    };
    params.tau = torsten_primitives::transaction::Rational {
        numerator: 2,
        denominator: 10,
    };
    params.n_opt = 500;
    params.a0 = torsten_primitives::transaction::Rational {
        numerator: 0,
        denominator: 1,
    };
    params.active_slots_coeff = 0.05;
    let mut state = LedgerState::new(params);
    // Epoch length for mainnet default.
    state.epoch_length = 432_000;
    // Disable stake rebuild so tests don't need a full UTxO scan.
    state.needs_stake_rebuild = false;
    state
}
