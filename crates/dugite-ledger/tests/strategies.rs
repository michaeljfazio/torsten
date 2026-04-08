//! Shared proptest strategy module for dugite-ledger property tests.
//!
//! Provides reusable generators for:
//! - Core Cardano primitives (Hash28, Hash32, Credential, Lovelace, Rational)
//! - Pool registrations
//! - Transaction inputs/outputs and simple transactions
//! - UTxO sets
//! - Protocol parameters
//! - Full LedgerState with six-pot identity
//!
//! Usage in test files:
//! ```rust,ignore
//! #[path = "strategies.rs"]
//! mod strategies;
//! ```

#![allow(dead_code)]

use dugite_ledger::state::{
    EpochSnapshots, PoolRegistration, StakeDistributionState, StakeSnapshot, MAX_LOVELACE_SUPPLY,
};
use dugite_ledger::LedgerState;
use dugite_ledger::UtxoSet;
use dugite_primitives::address::{Address, BaseAddress, ByronAddress};
use dugite_primitives::block::{Block, BlockHeader, OperationalCert, ProtocolVersion, VrfOutput};
use dugite_primitives::credentials::Credential;
use dugite_primitives::era::Era;
use dugite_primitives::hash::{Hash28, Hash32};
use dugite_primitives::network::NetworkId;
use dugite_primitives::protocol_params::ProtocolParameters;
use dugite_primitives::time::{BlockNo, EpochNo, SlotNo};
use dugite_primitives::transaction::{
    OutputDatum, Rational, Transaction, TransactionBody, TransactionInput, TransactionOutput,
    TransactionWitnessSet,
};
use dugite_primitives::value::{Lovelace, Value};
use proptest::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Core hash generators
// ---------------------------------------------------------------------------

/// Generate an arbitrary 28-byte hash (Hash28).
pub fn arb_hash28() -> impl Strategy<Value = Hash28> {
    proptest::array::uniform28(any::<u8>()).prop_map(Hash28::from_bytes)
}

/// Generate an arbitrary 32-byte hash (Hash32).
pub fn arb_hash32() -> impl Strategy<Value = Hash32> {
    proptest::array::uniform32(any::<u8>()).prop_map(Hash32::from_bytes)
}

// ---------------------------------------------------------------------------
// Credential generators
// ---------------------------------------------------------------------------

/// Generate an arbitrary stake credential (VerificationKey or Script).
pub fn arb_stake_credential() -> impl Strategy<Value = Credential> {
    prop_oneof![
        arb_hash28().prop_map(Credential::VerificationKey),
        arb_hash28().prop_map(Credential::Script),
    ]
}

/// Generate an arbitrary reward account byte vector.
///
/// The wire format is: `0xe0` (mainnet staking key header) followed by a 28-byte hash.
/// This matches the Cardano reward address encoding for key-hash stake credentials.
pub fn arb_reward_account() -> impl Strategy<Value = Vec<u8>> {
    arb_hash28().prop_map(|h| {
        let mut v = Vec::with_capacity(29);
        v.push(0xe0u8); // mainnet reward key address header
        v.extend_from_slice(h.as_bytes());
        v
    })
}

// ---------------------------------------------------------------------------
// Value generators
// ---------------------------------------------------------------------------

/// Generate a Lovelace amount in the range `[min, max]`.
pub fn arb_lovelace(min: u64, max: u64) -> impl Strategy<Value = Lovelace> {
    (min..=max).prop_map(Lovelace)
}

/// Generate a pool ID (Hash28 used as Blake2b-224 pool cold key hash).
pub fn arb_pool_id() -> impl Strategy<Value = Hash28> {
    arb_hash28()
}

// ---------------------------------------------------------------------------
// Rational generators
// ---------------------------------------------------------------------------

/// Generate an arbitrary Rational in the range [0/d, 1000/d] where d ∈ [1, 1000].
///
/// Suitable for protocol parameters like `rho` and `tau` which can be > 1
/// in degenerate test cases without breaking invariants.
pub fn arb_rational() -> impl Strategy<Value = Rational> {
    (1u64..=1000u64, 0u64..=1000u64).prop_map(|(denominator, numerator)| Rational {
        numerator,
        denominator,
    })
}

/// Generate a unit Rational in the range [0, 1] (numerator ≤ denominator).
///
/// Suitable for margins, thresholds, and other parameters constrained to [0,1].
pub fn arb_unit_rational() -> impl Strategy<Value = Rational> {
    (1u64..=1000u64).prop_flat_map(|denominator| {
        (0u64..=denominator).prop_map(move |numerator| Rational {
            numerator,
            denominator,
        })
    })
}

// ---------------------------------------------------------------------------
// Pool registration generator
// ---------------------------------------------------------------------------

/// Generate an arbitrary `PoolRegistration` for the given pool ID.
///
/// Uses modest cost/pledge values representative of a testnet pool.
pub fn arb_pool_registration(pool_id: Hash28) -> impl Strategy<Value = PoolRegistration> {
    (
        arb_hash32(),                             // vrf_keyhash
        arb_lovelace(0, 50_000_000_000),          // pledge
        arb_lovelace(340_000_000, 1_000_000_000), // cost (min pool cost range)
        arb_unit_rational(),                      // margin
        arb_reward_account(),                     // reward_account
    )
        .prop_map(move |(vrf_keyhash, pledge, cost, margin, reward_account)| {
            PoolRegistration {
                pool_id,
                vrf_keyhash,
                pledge,
                cost,
                margin_numerator: margin.numerator,
                margin_denominator: margin.denominator,
                reward_account,
                owners: vec![],
                relays: vec![],
                metadata_url: None,
                metadata_hash: None,
            }
        })
}

// ---------------------------------------------------------------------------
// Transaction input / output generators
// ---------------------------------------------------------------------------

/// Generate an arbitrary `TransactionInput`.
pub fn arb_tx_input() -> impl Strategy<Value = TransactionInput> {
    (arb_hash32(), 0u32..4u32).prop_map(|(transaction_id, index)| TransactionInput {
        transaction_id,
        index,
    })
}

/// Generate an arbitrary `TransactionOutput` with a Byron address and a
/// Lovelace value in `[min_ada, max_ada]`.
///
/// Byron addresses are the simplest address type (no staking credential),
/// making them ideal for basic validation tests.
pub fn arb_tx_output(min_ada: u64, max_ada: u64) -> impl Strategy<Value = TransactionOutput> {
    arb_lovelace(min_ada, max_ada).prop_map(|lovelace| TransactionOutput {
        address: Address::Byron(ByronAddress {
            payload: vec![0u8; 32],
        }),
        value: Value::lovelace(lovelace.0),
        datum: OutputDatum::None,
        script_ref: None,
        is_legacy: false,
        raw_cbor: None,
    })
}

/// Generate an arbitrary `(TransactionInput, TransactionOutput)` UTxO entry
/// with a Lovelace value in `[min_ada, max_ada]`.
pub fn arb_utxo_entry(
    min_ada: u64,
    max_ada: u64,
) -> impl Strategy<Value = (TransactionInput, TransactionOutput)> {
    (arb_tx_input(), arb_tx_output(min_ada, max_ada))
}

// ---------------------------------------------------------------------------
// Composite generators
// ---------------------------------------------------------------------------

/// Generate a `UtxoSet` with exactly `count` entries.
///
/// Returns `(UtxoSet, Vec<TransactionInput>)` so callers can reference
/// the inputs for building test transactions.
///
/// All inputs use distinct transaction IDs derived from the entry index to
/// guarantee no collisions.
pub fn arb_utxo_set(count: usize) -> impl Strategy<Value = (UtxoSet, Vec<TransactionInput>)> {
    // Generate a Vec of (ada_amount) values; inputs are deterministic from index.
    proptest::collection::vec(1_000_000u64..=100_000_000u64, count).prop_map(move |amounts| {
        let mut utxo = UtxoSet::new();
        let mut inputs = Vec::with_capacity(count);

        for (i, amount) in amounts.into_iter().enumerate() {
            // Derive a unique, deterministic transaction ID from the index.
            let mut id_bytes = [0u8; 32];
            let idx_bytes = (i as u64).to_be_bytes();
            id_bytes[..8].copy_from_slice(&idx_bytes);
            id_bytes[8] = 0xAB; // sentinel byte to avoid all-zero confusion

            let input = TransactionInput {
                transaction_id: Hash32::from_bytes(id_bytes),
                index: 0,
            };
            let output = TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(amount),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            };
            inputs.push(input.clone());
            utxo.insert(input, output);
        }

        (utxo, inputs)
    })
}

/// Generate a `ProtocolParameters` instance based on mainnet defaults with
/// randomised fee parameters, monetary expansion (rho), and treasury growth (tau).
///
/// All other parameters retain their mainnet values so that validation logic
/// (size limits, collateral, etc.) behaves predictably.
pub fn arb_protocol_params() -> impl Strategy<Value = ProtocolParameters> {
    (
        1u64..=100u64,             // min_fee_a (coefficient per byte)
        100_000u64..=1_000_000u64, // min_fee_b (constant)
        arb_unit_rational(),       // rho: monetary expansion [0,1]
        arb_unit_rational(),       // tau: treasury growth [0,1]
    )
        .prop_map(|(min_fee_a, min_fee_b, rho, tau)| {
            let mut p = ProtocolParameters::mainnet_defaults();
            p.min_fee_a = min_fee_a;
            p.min_fee_b = min_fee_b;
            p.rho = rho;
            p.tau = tau;
            p
        })
}

// ---------------------------------------------------------------------------
// Transaction builder
// ---------------------------------------------------------------------------

/// Build a simple ADA-only transaction with the given inputs, output value,
/// and fee.
///
/// The transaction body is structurally valid (single output, specified fee)
/// but is NOT signed and carries no witnesses. It is intended for ledger-level
/// validation tests, not for submission to an actual network.
pub fn build_simple_tx(inputs: Vec<TransactionInput>, output_value: u64, fee: u64) -> Transaction {
    Transaction {
        hash: Hash32::ZERO,
        era: Era::Conway,
        body: TransactionBody {
            inputs,
            outputs: vec![TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(output_value),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            }],
            fee: Lovelace(fee),
            ttl: None,
            certificates: vec![],
            withdrawals: std::collections::BTreeMap::new(),
            auxiliary_data_hash: None,
            validity_interval_start: None,
            mint: std::collections::BTreeMap::new(),
            script_data_hash: None,
            collateral: vec![],
            required_signers: vec![],
            network_id: None,
            collateral_return: None,
            total_collateral: None,
            reference_inputs: vec![],
            update: None,
            voting_procedures: std::collections::BTreeMap::new(),
            proposal_procedures: vec![],
            treasury_value: None,
            donation: None,
        },
        witness_set: TransactionWitnessSet {
            vkey_witnesses: vec![],
            native_scripts: vec![],
            bootstrap_witnesses: vec![],
            plutus_v1_scripts: vec![],
            plutus_v2_scripts: vec![],
            plutus_v3_scripts: vec![],
            plutus_data: vec![],
            redeemers: vec![],
            raw_redeemers_cbor: None,
            raw_plutus_data_cbor: None,
            pallas_script_data_hash: None,
        },
        is_valid: true,
        auxiliary_data: None,
        raw_cbor: None,
        raw_body_cbor: None,
        raw_witness_cbor: None,
    }
}

// ---------------------------------------------------------------------------
// Block builder
// ---------------------------------------------------------------------------

/// Build a minimal test block containing the given transactions.
///
/// The header fields are filled with sensible zero/default values and are
/// intentionally NOT cryptographically valid — this helper is for ledger
/// application tests, not consensus verification.
pub fn make_test_block(slot: u64, block_no: u64, transactions: Vec<Transaction>) -> Block {
    // Derive a unique header hash from the block number so that each test
    // block has a distinct `header_hash` (important for nonce tracking).
    let mut hash_bytes = [0u8; 32];
    hash_bytes[..8].copy_from_slice(&block_no.to_be_bytes());
    hash_bytes[8] = 0xBB; // sentinel byte

    Block {
        header: BlockHeader {
            header_hash: Hash32::from_bytes(hash_bytes),
            prev_hash: Hash32::ZERO,
            issuer_vkey: vec![],
            vrf_vkey: vec![],
            vrf_result: VrfOutput {
                output: vec![],
                proof: vec![],
            },
            nonce_vrf_output: vec![],
            nonce_vrf_proof: vec![],
            block_number: BlockNo(block_no),
            slot: SlotNo(slot),
            epoch_nonce: Hash32::ZERO,
            body_size: 0,
            body_hash: Hash32::ZERO,
            operational_cert: OperationalCert {
                hot_vkey: vec![],
                sequence_number: 0,
                kes_period: 0,
                sigma: vec![],
            },
            protocol_version: ProtocolVersion { major: 9, minor: 0 },
            kes_signature: vec![],
        },
        transactions,
        era: Era::Conway,
        raw_cbor: None,
    }
}

// ---------------------------------------------------------------------------
// LedgerState generator configuration
// ---------------------------------------------------------------------------

/// Configuration controlling the shape of the generated `LedgerState`.
///
/// All size parameters are kept small by default to keep test execution fast.
#[derive(Debug, Clone)]
pub struct LedgerStateConfig {
    /// Number of stake pools to register (0..=max_pools).
    pub max_pools: usize,
    /// Number of stake credential / delegation entries (0..=max_delegations).
    pub max_delegations: usize,
    /// Epoch number to assign to the generated state.
    pub epoch: u64,
}

impl Default for LedgerStateConfig {
    fn default() -> Self {
        LedgerStateConfig {
            max_pools: 5,
            max_delegations: 10,
            epoch: 500,
        }
    }
}

// ---------------------------------------------------------------------------
// LedgerState generator
// ---------------------------------------------------------------------------

/// Generate a `LedgerState` satisfying the six-pot identity:
///
/// ```text
/// utxo_total + reserves + treasury + reward_accounts + deposits_pot + fee_pot
///   == MAX_LOVELACE_SUPPLY (45_000_000_000_000_000)
/// ```
///
/// where:
/// - `deposits_pot = sum(key_deposit * registered_creds) + sum(pool_deposit * pools)`
/// - Snapshot state (mark/set/go) is consistent with the live pool/delegation maps.
///
/// The generated state uses mainnet defaults for protocol parameters and is
/// pre-seeded with a small number of pools and delegations so that epoch
/// transition tests have something non-trivial to process.
pub fn arb_ledger_state(config: LedgerStateConfig) -> impl Strategy<Value = LedgerState> {
    (
        // How many pools to register
        0..=config.max_pools,
        // How many delegations to create
        0..=config.max_delegations,
        // Fraction of MAX_LOVELACE_SUPPLY to put into UTxO (0..90%)
        // Remaining is distributed among reserves, treasury, rewards, deposits.
        0u64..90u64,
        // Fee pot: 0..10M lovelace
        0u64..10_000_000u64,
    )
        .prop_flat_map(move |(n_pools, n_delegations, utxo_pct, fee_pot)| {
            let cfg = config.clone();

            // Pre-compute fixed-cost deposits from mainnet defaults.
            let params = ProtocolParameters::mainnet_defaults();
            let key_deposit = params.key_deposit.0;
            let pool_deposit = params.pool_deposit.0;

            // Pool IDs are deterministic from index so different call sites
            // produce non-colliding generators.
            let pool_ids: Vec<Hash28> = (0..n_pools)
                .map(|i| {
                    let mut b = [0x10u8; 28];
                    b[0] = (i & 0xFF) as u8;
                    b[1] = ((i >> 8) & 0xFF) as u8;
                    Hash28::from_bytes(b)
                })
                .collect();

            // Stake credential hashes are deterministic from index.
            let stake_keys: Vec<Hash32> = (0..n_delegations)
                .map(|i| {
                    let mut b = [0x20u8; 32];
                    b[0] = (i & 0xFF) as u8;
                    b[1] = ((i >> 8) & 0xFF) as u8;
                    Hash32::from_bytes(b)
                })
                .collect();

            let deposits_pot: u64 =
                (n_delegations as u64) * key_deposit + (n_pools as u64) * pool_deposit;

            // Budget the pots:
            //   available = MAX_LOVELACE_SUPPLY - deposits_pot - fee_pot
            //   utxo_total = available * utxo_pct / 100
            //   remaining  = available - utxo_total
            //   treasury   = remaining / 3
            //   rewards    = remaining / 3 (shared equally across delegations)
            //   reserves   = MAX_LOVELACE_SUPPLY - all others
            let available = MAX_LOVELACE_SUPPLY
                .saturating_sub(deposits_pot)
                .saturating_sub(fee_pot);
            let utxo_total = available * utxo_pct / 100;
            let remaining = available.saturating_sub(utxo_total);
            let treasury = remaining / 3;
            let rewards_total = remaining / 3;
            // reserves absorbs any rounding to maintain the identity exactly.
            let reserves = MAX_LOVELACE_SUPPLY
                - utxo_total
                - treasury
                - rewards_total
                - deposits_pot
                - fee_pot;

            let reward_per_key: u64 = if n_delegations > 0 {
                rewards_total / n_delegations as u64
            } else {
                0
            };

            // Distribute UTxO total across `n_delegations` entries if possible,
            // otherwise use a single genesis-style entry.
            let utxo_entries: usize = if n_delegations > 0 { n_delegations } else { 1 };
            let utxo_per_entry = if utxo_entries > 0 {
                utxo_total / utxo_entries as u64
            } else {
                0
            };

            // Generate per-pool pledge amounts (0..=pledge_max where pledge_max
            // fits within the overall pool cost budget — we just use a small
            // deterministic value for simplicity).
            Just(()).prop_map(move |_| {
                let mut state = LedgerState::new(params.clone());
                state.epoch = EpochNo(cfg.epoch);
                state.epochs.treasury = Lovelace(treasury);
                state.epochs.reserves = Lovelace(reserves);
                state.utxo.epoch_fees = Lovelace(fee_pot);

                // ── Pools ────────────────────────────────────────────────────
                let mut pool_map = HashMap::new();
                let mut pool_deposits_map = HashMap::new();
                for pool_id in &pool_ids {
                    let reg = PoolRegistration {
                        pool_id: *pool_id,
                        vrf_keyhash: Hash32::ZERO,
                        pledge: Lovelace(1_000_000_000),
                        cost: Lovelace(340_000_000),
                        margin_numerator: 5,
                        margin_denominator: 100,
                        reward_account: {
                            let mut v = vec![0xe0u8];
                            v.extend_from_slice(pool_id.as_bytes());
                            v
                        },
                        owners: vec![],
                        relays: vec![],
                        metadata_url: None,
                        metadata_hash: None,
                    };
                    pool_map.insert(*pool_id, reg);
                    pool_deposits_map.insert(*pool_id, pool_deposit);
                }
                state.certs.pool_params = Arc::new(pool_map);
                state.certs.pool_deposits = pool_deposits_map;

                // ── Reward accounts and delegations ──────────────────────────
                let mut reward_accounts = HashMap::new();
                let mut delegations = HashMap::new();
                let mut stake_key_deposits = HashMap::new();
                let mut stake_map = HashMap::new();

                for (i, sk) in stake_keys.iter().enumerate() {
                    // Register reward account
                    reward_accounts.insert(*sk, Lovelace(reward_per_key));

                    // Delegate to a pool (round-robin)
                    if !pool_ids.is_empty() {
                        delegations.insert(*sk, pool_ids[i % pool_ids.len()]);
                    }

                    // Record the deposit
                    stake_key_deposits.insert(*sk, key_deposit);

                    // Add a UTxO for this stake key (Base address, mainnet)
                    let payment_cred =
                        Credential::VerificationKey(Hash28::from_bytes([0xFFu8; 28]));
                    let stake_cred = Credential::VerificationKey(Hash28::from_bytes({
                        let mut b = [0u8; 28];
                        b[..8].copy_from_slice(&(i as u64).to_be_bytes());
                        b
                    }));
                    let addr = Address::Base(BaseAddress {
                        network: NetworkId::Mainnet,
                        payment: payment_cred,
                        stake: stake_cred,
                    });
                    let mut tx_id = [0x30u8; 32];
                    tx_id[0] = (i & 0xFF) as u8;
                    tx_id[1] = ((i >> 8) & 0xFF) as u8;
                    let input = TransactionInput {
                        transaction_id: Hash32::from_bytes(tx_id),
                        index: 0,
                    };
                    let output = TransactionOutput {
                        address: addr,
                        value: Value::lovelace(utxo_per_entry),
                        datum: OutputDatum::None,
                        script_ref: None,
                        is_legacy: false,
                        raw_cbor: None,
                    };
                    state.utxo.utxo_set.insert(input, output);

                    // Update incremental stake map to match the UTxO
                    *stake_map.entry(*sk).or_insert(Lovelace(0)) += Lovelace(utxo_per_entry);
                }

                // If no delegations, add a single genesis UTxO for the full utxo_total.
                if n_delegations == 0 && utxo_total > 0 {
                    let input = TransactionInput {
                        transaction_id: Hash32::from_bytes([0xFFu8; 32]),
                        index: 0,
                    };
                    let output = TransactionOutput {
                        address: Address::Byron(ByronAddress {
                            payload: vec![0u8; 32],
                        }),
                        value: Value::lovelace(utxo_total),
                        datum: OutputDatum::None,
                        script_ref: None,
                        is_legacy: false,
                        raw_cbor: None,
                    };
                    state.utxo.utxo_set.insert(input, output);
                }

                state.certs.reward_accounts = Arc::new(reward_accounts.clone());
                state.certs.delegations = Arc::new(delegations.clone());
                state.certs.total_stake_key_deposits = (n_delegations as u64) * key_deposit;
                state.certs.stake_key_deposits = stake_key_deposits;
                state.certs.stake_distribution = StakeDistributionState { stake_map };

                // ── Mark / set / go snapshots ────────────────────────────────
                // All three snapshots mirror the live pool and delegation state
                // so that reward and leader-election code has consistent data.
                let snapshot = StakeSnapshot {
                    epoch: EpochNo(cfg.epoch.saturating_sub(1)),
                    delegations: Arc::new(delegations.clone()),
                    pool_stake: {
                        let mut ps = HashMap::new();
                        for pool_id in &pool_ids {
                            ps.insert(*pool_id, Lovelace(utxo_per_entry));
                        }
                        ps
                    },
                    pool_params: state.certs.pool_params.clone(),
                    stake_distribution: Arc::new(HashMap::new()),
                    epoch_fees: Lovelace(fee_pot),
                    epoch_block_count: 0,
                    epoch_blocks_by_pool: Arc::new(HashMap::new()),
                };
                state.epochs.snapshots = EpochSnapshots {
                    mark: Some(snapshot.clone()),
                    set: Some(snapshot.clone()),
                    go: Some(snapshot),
                    ss_fee: Lovelace(fee_pot),
                    bprev_block_count: 0,
                    bprev_blocks_by_pool: Arc::new(HashMap::new()),
                    rupd_ready: cfg.epoch >= 2,
                };

                // Skip stake rebuild since we populated stake_map manually.
                state.epochs.needs_stake_rebuild = false;

                state
            })
        })
}
