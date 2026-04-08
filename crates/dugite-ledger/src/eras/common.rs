//! Shared helpers used across multiple era rule implementations.
//!
//! These are NOT on the EraRules trait -- they are internal building blocks
//! that era impls compose to avoid duplicating logic. The pattern is
//! composition over inheritance.
//!
//! NOTE: These helpers are currently unused -- they are building blocks that will
//! be called by ShelleyRules, AlonzoRules, BabbageRules, and ConwayRules
//! implementations (Tasks 9-11).
//!
//! Each function takes sub-state references as parameters (not `&mut LedgerState`),
//! enabling independent borrow checking and clean composition in era rule
//! implementations.
//!
//! # Functions
//!
//! | Helper | Used By | Description |
//! |--------|---------|-------------|
//! | [`apply_utxo_changes`] | Shelley, Alonzo, Babbage, Conway | Consume inputs, produce outputs, record fee |
//! | [`apply_collateral_consumption`] | Alonzo, Babbage, Conway | IsValid=false collateral forfeiture |
//! | [`process_shelley_certs`] | Shelley, Allegra, Mary, Alonzo, Babbage | Shelley-era certificate processing |
//! | [`drain_withdrawal_accounts`] | Shelley+ | Zero reward accounts referenced by tx withdrawals |
//! | [`compute_shelley_nonce`] | Shelley+ | VRF-based nonce evolution and block counting |
//! | [`validate_shelley_base`] | (stub) | Phase-1 validation rules common to all Shelley+ eras |

use std::sync::Arc;

use dugite_primitives::address::Address;
use dugite_primitives::block::BlockHeader;
use dugite_primitives::credentials::{Credential, Pointer};
use dugite_primitives::hash::{blake2b_224, blake2b_256, Hash32};
use dugite_primitives::protocol_params::ProtocolParameters;
use dugite_primitives::time::EpochNo;
use dugite_primitives::transaction::{Certificate, Transaction, TransactionInput};
use dugite_primitives::value::Lovelace;
use tracing::debug;

use crate::state::substates::{
    CertSubState, ConsensusSubState, EpochSubState, GovSubState, UtxoSubState,
};
use crate::state::PoolRegistration;
use crate::utxo_diff::UtxoDiff;

// ---------------------------------------------------------------------------
// Stake routing (local mirror of state::StakeRouting)
// ---------------------------------------------------------------------------

/// The stake routing outcome for a UTxO output address.
///
/// Mirrors `state::StakeRouting`. Defined here so that `common.rs` helpers do
/// not depend on private types in the `state` module.
enum StakeRouting {
    /// Credential hash -- route coins to `stake_distribution.stake_map`.
    Credential(Hash32),
    /// Pointer key -- route coins to `ptr_stake` (deferred resolution at SNAP time).
    Pointer(Pointer),
    /// No stake routing (Enterprise / Byron / unknown).
    None,
}

/// Classify a UTxO address into its stake-routing bucket.
///
/// * Base / Reward  -> `StakeRouting::Credential` (eager resolution)
/// * Pointer        -> `StakeRouting::Pointer` (deferred)
/// * Everything else -> `StakeRouting::None`
///
/// When `exclude_ptrs` is true (Conway era), pointer addresses return
/// `StakeRouting::None`.
fn stake_routing(address: &Address, exclude_ptrs: bool) -> StakeRouting {
    match address {
        Address::Base(base) => StakeRouting::Credential(credential_to_hash(&base.stake)),
        Address::Reward(reward) => StakeRouting::Credential(credential_to_hash(&reward.stake)),
        Address::Pointer(ptr_addr) => {
            if exclude_ptrs {
                StakeRouting::None
            } else {
                StakeRouting::Pointer(ptr_addr.pointer)
            }
        }
        _ => StakeRouting::None,
    }
}

/// Extract a Hash32 from a Credential for use as a map key.
///
/// Uses `to_typed_hash32()` which encodes the credential TYPE (key vs script)
/// in byte 28 of the padding, matching Haskell's `KeyHashObj`/`ScriptHashObj`
/// distinction.
fn credential_to_hash(credential: &Credential) -> Hash32 {
    credential.to_typed_hash32()
}

/// Extract a Hash32 from a raw reward account byte string (29 bytes: 1-byte
/// header + 28-byte credential hash).
///
/// Mirrors `LedgerState::reward_account_to_hash` from `state/certificates.rs`.
fn reward_account_to_hash(reward_account: &[u8]) -> Hash32 {
    let mut key_bytes = [0u8; 32];
    if reward_account.len() >= 29 {
        // Copy exactly 28 bytes of the credential (skip the 1-byte header).
        key_bytes[..28].copy_from_slice(&reward_account[1..29]);
        // Encode credential type from the header byte:
        // Bit 4 of the header: 0 = key hash, 1 = script hash
        // Reward address headers: 0xe0/0xe1 = key, 0xf0/0xf1 = script
        if reward_account[0] & 0x10 != 0 {
            key_bytes[28] = 0x01; // script credential
        }
    }
    Hash32::from_bytes(key_bytes)
}

// ============================================================================
// 1. apply_utxo_changes
// ============================================================================

/// Apply UTxO changes for a valid transaction (IsValid=true path).
///
/// Core UTxO state mutation logic shared by all post-Byron eras:
///
/// 1. Snapshot spent outputs (for stake distribution and diff recording).
/// 2. Subtract spent coins from the stake distribution.
/// 3. Remove inputs from the UTxO set (best-effort: missing inputs are logged,
///    not fatal -- matches Haskell `applyTx` for confirmed on-chain blocks).
/// 4. Insert new outputs unconditionally (prevents cascade divergence).
/// 5. Add new output coins to the stake distribution.
/// 6. Accumulate the transaction fee.
///
/// Returns a `UtxoDiff` recording all inserts and deletes for rollback support.
///
/// # Parameters
///
/// * `tx` -- the transaction to apply.
/// * `utxo` -- mutable UTxO sub-state (utxo_set, epoch_fees, diff_seq).
/// * `certs` -- mutable cert sub-state (stake_distribution for tracking).
/// * `epochs` -- epoch sub-state (ptr_stake for pointer-addressed UTxOs).
pub(crate) fn apply_utxo_changes(
    tx: &Transaction,
    utxo: &mut UtxoSubState,
    certs: &mut CertSubState,
    epochs: &mut EpochSubState,
) -> UtxoDiff {
    let mut diff = UtxoDiff::new();

    // --- Phase 1: snapshot spent outputs before mutation ---
    //
    // Collect (input, output) pairs for inputs that exist in the UTxO set.
    // Missing inputs (pre-replay gaps) are silently skipped.
    let spent_outputs: Vec<_> = tx
        .body
        .inputs
        .iter()
        .filter_map(|input| {
            utxo.utxo_set
                .lookup(input)
                .map(|output| (input.clone(), output))
        })
        .collect();

    // --- Phase 2: update stake distribution from consumed inputs (subtract) ---
    for (_input, spent_output) in &spent_outputs {
        let coin = spent_output.value.coin.0;
        match stake_routing(&spent_output.address, epochs.ptr_stake_excluded) {
            StakeRouting::Credential(cred_hash) => {
                if let Some(stake) = certs.stake_distribution.stake_map.get_mut(&cred_hash) {
                    stake.0 = stake.0.saturating_sub(coin);
                }
            }
            StakeRouting::Pointer(ptr) => {
                if let Some(entry) = epochs.ptr_stake.get_mut(&ptr) {
                    *entry = entry.saturating_sub(coin);
                }
            }
            StakeRouting::None => {}
        }
    }

    // --- Phase 3: remove inputs (best-effort) ---
    let mut missing_inputs = 0usize;
    for input in &tx.body.inputs {
        if utxo.utxo_set.contains(input) {
            utxo.utxo_set.remove(input);
        } else {
            missing_inputs += 1;
            debug!(
                tx_hash = %tx.hash.to_hex(),
                input = %input,
                "apply_utxo_changes: input not found in UTxO set (already spent or \
                 pre-replay gap) -- outputs will still be created"
            );
        }
    }
    if missing_inputs > 0 {
        debug!(
            tx_hash = %tx.hash.to_hex(),
            missing = missing_inputs,
            total = tx.body.inputs.len(),
            "apply_utxo_changes: {} of {} inputs were absent; outputs inserted regardless",
            missing_inputs,
            tx.body.inputs.len(),
        );
    }

    // Record deletions for diff.
    for (input, output) in spent_outputs {
        diff.record_delete(input, output);
    }

    // --- Phase 4: insert new outputs unconditionally ---
    for (idx, output) in tx.body.outputs.iter().enumerate() {
        let new_input = TransactionInput {
            transaction_id: tx.hash,
            index: idx as u32,
        };
        diff.record_insert(new_input.clone(), output.clone());
        utxo.utxo_set.insert(new_input, output.clone());
    }

    // --- Phase 5: update stake distribution from new outputs (add) ---
    for output in &tx.body.outputs {
        let coin = output.value.coin.0;
        match stake_routing(&output.address, epochs.ptr_stake_excluded) {
            StakeRouting::Credential(cred_hash) => {
                *certs
                    .stake_distribution
                    .stake_map
                    .entry(cred_hash)
                    .or_insert(Lovelace(0)) += Lovelace(coin);
            }
            StakeRouting::Pointer(ptr) => {
                *epochs.ptr_stake.entry(ptr).or_insert(0) += coin;
            }
            StakeRouting::None => {}
        }
    }

    // --- Phase 6: accumulate fee ---
    utxo.epoch_fees += tx.body.fee;

    diff
}

// ============================================================================
// 2. apply_collateral_consumption
// ============================================================================

/// Apply collateral consumption for an invalid transaction (IsValid=false path).
///
/// When a Plutus script fails Phase-2 validation, the block producer marks
/// the transaction as invalid. The regular inputs/outputs/certificates are NOT
/// applied. Instead:
///
/// 1. Collateral inputs are consumed (forfeited to the block producer).
/// 2. If `collateral_return` is present (Babbage+), it becomes a new UTxO.
/// 3. The fee is either `total_collateral` (if declared) or the difference
///    between collateral input value and collateral return value.
///
/// Returns a `UtxoDiff` recording collateral-related inserts and deletes.
///
/// # Parameters
///
/// * `tx` -- the invalid transaction.
/// * `utxo` -- mutable UTxO sub-state.
/// * `certs` -- mutable cert sub-state (stake distribution updates).
/// * `epochs` -- mutable epoch sub-state (ptr_stake for pointer routing).
///
/// # Eras
///
/// Only relevant for Alonzo+ (collateral was introduced in the Alonzo era).
/// Babbage added `collateral_return` and `total_collateral` fields.
pub(crate) fn apply_collateral_consumption(
    tx: &Transaction,
    utxo: &mut UtxoSubState,
    certs: &mut CertSubState,
    epochs: &mut EpochSubState,
) -> UtxoDiff {
    let mut diff = UtxoDiff::new();
    let mut collateral_input_value: u64 = 0;

    // Consume collateral inputs and update stake distribution.
    for col_input in &tx.body.collateral {
        if let Some(spent) = utxo.utxo_set.lookup(col_input) {
            collateral_input_value += spent.value.coin.0;
            let coin = spent.value.coin.0;
            match stake_routing(&spent.address, epochs.ptr_stake_excluded) {
                StakeRouting::Credential(cred) => {
                    if let Some(stake) = certs.stake_distribution.stake_map.get_mut(&cred) {
                        stake.0 = stake.0.saturating_sub(coin);
                    }
                }
                StakeRouting::Pointer(ptr) => {
                    if let Some(entry) = epochs.ptr_stake.get_mut(&ptr) {
                        *entry = entry.saturating_sub(coin);
                    }
                }
                StakeRouting::None => {}
            }
            diff.record_delete(col_input.clone(), spent);
        }
        utxo.utxo_set.remove(col_input);
    }

    // If there's a collateral return output, add it to the UTxO set.
    let collateral_return_value = if let Some(col_return) = &tx.body.collateral_return {
        let coin = col_return.value.coin.0;
        match stake_routing(&col_return.address, epochs.ptr_stake_excluded) {
            StakeRouting::Credential(cred) => {
                *certs
                    .stake_distribution
                    .stake_map
                    .entry(cred)
                    .or_insert(Lovelace(0)) += Lovelace(coin);
            }
            StakeRouting::Pointer(ptr) => {
                *epochs.ptr_stake.entry(ptr).or_insert(0) += coin;
            }
            StakeRouting::None => {}
        }
        let return_input = TransactionInput {
            transaction_id: tx.hash,
            // Collateral return is placed after regular outputs.
            index: tx.body.outputs.len() as u32,
        };
        diff.record_insert(return_input.clone(), col_return.clone());
        utxo.utxo_set.insert(return_input, col_return.clone());
        col_return.value.coin.0
    } else {
        0
    };

    // Fee collected is the actual collateral forfeited, NOT the declared fee.
    // If total_collateral is set, use it; otherwise compute from inputs - return.
    let collateral_fee = if let Some(tc) = tx.body.total_collateral {
        tc
    } else {
        Lovelace(collateral_input_value.saturating_sub(collateral_return_value))
    };
    utxo.epoch_fees += collateral_fee;

    diff
}

// ============================================================================
// 3. process_shelley_certs
// ============================================================================

/// Process Shelley-era certificate types from a transaction.
///
/// Handles the five certificate types introduced in Shelley that persist across
/// all subsequent eras:
///
/// - `StakeRegistration` -- register a stake credential (creates reward account,
///   tracks deposit, updates stake_map).
/// - `StakeDeregistration` -- deregister a stake credential (refunds deposit,
///   removes delegation and reward account).
/// - `StakeDelegation` -- delegate stake to a pool.
/// - `PoolRegistration` -- register or re-register a stake pool.
/// - `PoolRetirement` -- schedule a pool retirement at a future epoch.
///
/// This function also handles pointer map updates for registration-class
/// certificates, enabling Pointer address resolution.
///
/// Conway-era combined certificates (`ConwayStakeRegistration`,
/// `RegStakeDeleg`, etc.) and governance certificates (`RegDRep`, `VoteDelegation`,
/// etc.) are NOT processed here -- those are handled by `process_conway_certs`
/// in the Conway era rule implementation.
///
/// # Parameters
///
/// * `tx` -- the transaction containing certificates.
/// * `slot` -- the block slot (for pointer map entries).
/// * `tx_index` -- the transaction's index within the block.
/// * `certs` -- mutable cert sub-state (delegations, pool_params, reward_accounts, etc.).
/// * `epochs` -- epoch sub-state (protocol_params for deposit amounts).
/// * `gov` -- mutable governance sub-state (for deregistration cleanup of DRep
///   vote delegations, matching Haskell's unified map semantics).
pub(crate) fn process_shelley_certs(
    tx: &Transaction,
    slot: u64,
    tx_index: u64,
    certs: &mut CertSubState,
    epochs: &EpochSubState,
    gov: &mut GovSubState,
) {
    for (cert_index, cert) in tx.body.certificates.iter().enumerate() {
        // Populate pointer_map for registration certificates.
        if let Certificate::StakeRegistration(credential) = cert {
            let key = credential_to_hash(credential);
            let pointer = Pointer {
                slot,
                tx_index,
                cert_index: cert_index as u64,
            };
            certs.pointer_map.insert(pointer, key);
        }

        match cert {
            Certificate::StakeRegistration(credential) => {
                let key = credential_to_hash(credential);
                certs
                    .stake_distribution
                    .stake_map
                    .entry(key)
                    .or_insert(Lovelace(0));
                Arc::make_mut(&mut certs.reward_accounts)
                    .entry(key)
                    .or_insert(Lovelace(0));
                if matches!(credential, Credential::Script(_)) {
                    certs.script_stake_credentials.insert(key);
                }
                certs.total_stake_key_deposits += epochs.protocol_params.key_deposit.0;
                certs
                    .stake_key_deposits
                    .insert(key, epochs.protocol_params.key_deposit.0);
                debug!("Stake key registered: {}", key.to_hex());
            }
            Certificate::StakeDeregistration(credential) => {
                let key = credential_to_hash(credential);
                let stored_deposit = certs
                    .stake_key_deposits
                    .remove(&key)
                    .unwrap_or(epochs.protocol_params.key_deposit.0);
                certs.total_stake_key_deposits = certs
                    .total_stake_key_deposits
                    .saturating_sub(stored_deposit);
                Arc::make_mut(&mut certs.delegations).remove(&key);
                Arc::make_mut(&mut certs.reward_accounts).remove(&key);
                // Remove DRep delegation -- Haskell's unified map clears all credential
                // data on deregistration, including vote delegations.
                Arc::make_mut(&mut gov.governance)
                    .vote_delegations
                    .remove(&key);
                certs.script_stake_credentials.remove(&key);
                certs.pointer_map.retain(|_, v| *v != key);
                debug!("Stake key deregistered: {}", key.to_hex());
            }
            Certificate::StakeDelegation {
                credential,
                pool_hash,
            } => {
                let key = credential_to_hash(credential);
                Arc::make_mut(&mut certs.delegations).insert(key, *pool_hash);
                debug!("Stake delegated to pool: {}", pool_hash.to_hex());
            }
            Certificate::PoolRegistration(params) => {
                let pool_reg = PoolRegistration {
                    pool_id: params.operator,
                    vrf_keyhash: params.vrf_keyhash,
                    pledge: params.pledge,
                    cost: params.cost,
                    margin_numerator: params.margin.numerator,
                    margin_denominator: params.margin.denominator,
                    reward_account: params.reward_account.clone(),
                    owners: params.pool_owners.clone(),
                    relays: params.relays.clone(),
                    metadata_url: params.pool_metadata.as_ref().map(|m| m.url.clone()),
                    metadata_hash: params.pool_metadata.as_ref().map(|m| m.hash),
                };
                // Re-registration: defer to future_pool_params and cancel pending retirement.
                // First registration: apply immediately and record deposit.
                if certs.pool_params.contains_key(&params.operator) {
                    certs.pending_retirements.remove(&params.operator);
                    certs.future_pool_params.insert(params.operator, pool_reg);
                    debug!(
                        "Pool re-registered (deferred, retirement cancelled): {}",
                        params.operator.to_hex()
                    );
                } else {
                    Arc::make_mut(&mut certs.pool_params).insert(params.operator, pool_reg);
                    certs
                        .pool_deposits
                        .insert(params.operator, epochs.protocol_params.pool_deposit.0);
                    debug!("Pool registered: {}", params.operator.to_hex());
                }
            }
            Certificate::PoolRetirement { pool_hash, epoch } => {
                debug!(
                    "Pool retirement scheduled at epoch {}: {}",
                    epoch,
                    pool_hash.to_hex()
                );
                certs
                    .pending_retirements
                    .insert(*pool_hash, EpochNo(*epoch));
            }
            // Skip non-Shelley certificates -- they are handled by era-specific code.
            _ => {}
        }
    }
}

// ============================================================================
// 4. drain_withdrawal_accounts
// ============================================================================

/// Drain withdrawal accounts referenced by a transaction.
///
/// For each withdrawal in the transaction body, sets the corresponding reward
/// account balance to zero. Per the Cardano specification, the withdrawal
/// amount must exactly equal the reward balance; during sync from genesis we
/// may not have accumulated all rewards yet, so mismatches are logged at DEBUG
/// level only (best-effort, matching the existing behavior).
///
/// # Parameters
///
/// * `tx` -- the transaction containing withdrawals.
/// * `certs` -- mutable cert sub-state (reward_accounts).
pub(crate) fn drain_withdrawal_accounts(tx: &Transaction, certs: &mut CertSubState) {
    for (reward_account, amount) in &tx.body.withdrawals {
        let key = reward_account_to_hash(reward_account);
        if let Some(balance) = Arc::make_mut(&mut certs.reward_accounts).get_mut(&key) {
            if balance.0 != amount.0 {
                debug!(
                    account = %key.to_hex(),
                    balance = balance.0,
                    withdrawal = amount.0,
                    "drain_withdrawal_accounts: withdrawal amount does not match reward balance"
                );
            }
            // Always zero the balance -- rewards were consumed in the on-chain transaction.
            balance.0 = 0;
        }
    }
}

// ============================================================================
// 5. compute_shelley_nonce
// ============================================================================

/// Evolve nonce state after processing a Shelley+ block header.
///
/// Implements Haskell's `reupdateChainDepState` nonce state machine:
///
/// 1. **evolving_nonce** is updated for EVERY block using the era-specific
///    nonce VRF contribution (`nonce_vrf_output` on the header):
///    - `evolving' = blake2b_256(evolving || blake2b_256(nonce_vrf_output))`
///
/// 2. **candidate_nonce** tracks `evolving_nonce` UNLESS the block is within
///    the stability window of the epoch end, in which case the candidate
///    freezes so the epoch nonce is stable.
///
/// 3. **lab_nonce** = `prevHashToNonce(block.prevHash)` -- direct assignment
///    of `prev_hash` bytes (castHash is a type-level reinterpret, no rehash).
///
/// 4. **epoch_blocks_by_pool** and **epoch_block_count** are incremented.
///    Block counting respects the overlay schedule: when `d >= 0.8` (federated
///    era) or in Babbage+ (`d = 0` by definition for proto >= 7), the overlay
///    parameter controls whether blocks count toward pool rewards.
///
/// # Parameters
///
/// * `header` -- the block header with VRF output and issuer vkey.
/// * `block_slot` -- the slot of the block being processed.
/// * `current_epoch_first_slot_of_next` -- first slot of the next epoch,
///   used to determine if we are inside the stability window.
/// * `stability_window` -- the number of slots before epoch end where
///   candidate_nonce freezes (3k/f for Babbage, 4k/f for Conway+).
/// * `d_value` -- the decentralization parameter (0.0 for Babbage+).
/// * `consensus` -- mutable consensus sub-state.
pub(crate) fn compute_shelley_nonce(
    header: &BlockHeader,
    block_slot: u64,
    first_slot_of_next_epoch: u64,
    stability_window: u64,
    d_value: f64,
    consensus: &mut ConsensusSubState,
) {
    // Update evolving nonce if nonce_vrf_output is present.
    if !header.nonce_vrf_output.is_empty() {
        // Compute eta = blake2b_256(nonce_vrf_output), then
        // evolving' = blake2b_256(evolving || eta).
        let eta_hash = blake2b_256(&header.nonce_vrf_output);
        let mut data = Vec::with_capacity(64);
        data.extend_from_slice(consensus.evolving_nonce.as_bytes());
        data.extend_from_slice(eta_hash.as_bytes());
        consensus.evolving_nonce = blake2b_256(&data);

        // Candidate nonce tracks evolving nonce outside the stability window.
        if block_slot.saturating_add(stability_window) < first_slot_of_next_epoch {
            consensus.candidate_nonce = consensus.evolving_nonce;
        }
    }

    // lab_nonce = prevHashToNonce(block.prevHash).
    // prevHashToNonce: GenesisHash -> NeutralNonce; BlockHash h -> Nonce(h).
    // castHash is a type-reinterpret (no rehashing).
    consensus.lab_nonce = header.prev_hash;

    // Track block production by pool (issuer vkey hash).
    //
    // Matches Haskell's `incrBlocks`: only non-overlay blocks are counted
    // in BlocksMade. When d >= 0.8 (federated era), blocks should NOT be
    // counted toward pool rewards.
    if d_value < 0.8 && !header.issuer_vkey.is_empty() {
        let pool_id = blake2b_224(&header.issuer_vkey);
        *Arc::make_mut(&mut consensus.epoch_blocks_by_pool)
            .entry(pool_id)
            .or_insert(0) += 1;
    }
    consensus.epoch_block_count += 1;
}

// ============================================================================
// 6. validate_shelley_base (stub)
// ============================================================================

/// Phase-1 validation rules common to all Shelley+ eras (rules 1-10).
///
/// This is a stub. The existing validation code in `validation/mod.rs` and
/// `validation/phase1.rs` is tightly coupled with the `ValidationContext`
/// struct and the `validate_transaction_with_pools` function. Extracting it
/// cleanly requires refactoring the validation module, which is planned for
/// a separate task.
///
/// # Rules covered (when implemented)
///
/// 1. **InputsExist** -- all tx inputs exist in the UTxO set.
/// 2. **FeeSufficient** -- declared fee >= min_fee(tx).
/// 3. **TTLValid** -- transaction TTL has not expired (slot <= ttl).
/// 4. **ValuePreserved** -- sum(inputs) = sum(outputs) + fee (+ deposits - refunds).
/// 5. **OutputTooSmall** -- each output meets min_utxo_value / coins_per_utxo_byte.
/// 6. **OutputBootAddress** -- Byron/Bootstrap addresses cannot carry multi-asset.
/// 7. **TxSizeLimit** -- serialized tx size <= max_tx_size.
/// 8. **NetworkMismatch** -- output addresses match the expected network.
/// 9. **WitnessSetComplete** -- all required vkey witnesses are present.
/// 10. **CollateralValid** -- collateral inputs exist and are sufficient (Alonzo+).
///
/// # Parameters (planned)
///
/// * `tx` -- the transaction to validate.
/// * `utxo` -- read-only UTxO sub-state for input lookup.
/// * `params` -- current protocol parameters (min_fee, max_tx_size, etc.).
/// * `current_slot` -- the block slot for TTL checking.
///
/// # Returns
///
/// `Ok(())` if all Phase-1 rules pass, or `Err(Vec<ValidationError>)`.
///
/// # Status
///
/// **STUB** -- not yet implemented. Era rule impls should continue to call
/// `validate_transaction_with_pools` from the validation module until this
/// function is fleshed out.
pub(crate) fn validate_shelley_base(
    _tx: &Transaction,
    _utxo: &UtxoSubState,
    _params: &ProtocolParameters,
    _current_slot: u64,
) -> Result<(), Vec<String>> {
    // TODO: Extract Phase-1 rules 1-10 from validation/phase1.rs.
    // The current validation module is tightly coupled with ValidationContext
    // and validate_transaction_with_pools. This will be implemented when the
    // validation module is refactored to work with sub-state references.
    Ok(())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::substates::*;
    use crate::state::StakeDistributionState;
    use crate::utxo::UtxoSet;
    use crate::utxo_diff::DiffSeq;
    use dugite_primitives::address::{Address, BaseAddress, EnterpriseAddress, PointerAddress};
    use dugite_primitives::block::{OperationalCert, ProtocolVersion, VrfOutput};
    use dugite_primitives::credentials::Credential;
    use dugite_primitives::hash::{Hash28, Hash32};
    use dugite_primitives::network::NetworkId;
    use dugite_primitives::protocol_params::ProtocolParameters;
    use dugite_primitives::time::BlockNo;
    use dugite_primitives::time::SlotNo;
    use dugite_primitives::transaction::{OutputDatum, TransactionInput, TransactionOutput};
    use dugite_primitives::value::{Lovelace, Value};
    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::sync::Arc;

    // -----------------------------------------------------------------------
    // Constants
    // -----------------------------------------------------------------------

    const ZERO32: Hash32 = Hash32::ZERO;
    const ZERO28: Hash28 = Hash28::ZERO;

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    /// Create a minimal UtxoSubState for testing.
    fn empty_utxo_sub() -> UtxoSubState {
        UtxoSubState {
            utxo_set: UtxoSet::new(),
            diff_seq: DiffSeq::new(),
            epoch_fees: Lovelace(0),
            pending_donations: Lovelace(0),
        }
    }

    /// Create a minimal CertSubState for testing.
    fn empty_cert_sub() -> CertSubState {
        CertSubState {
            delegations: Arc::new(HashMap::new()),
            pool_params: Arc::new(HashMap::new()),
            future_pool_params: HashMap::new(),
            pending_retirements: HashMap::new(),
            reward_accounts: Arc::new(HashMap::new()),
            stake_key_deposits: HashMap::new(),
            pool_deposits: HashMap::new(),
            total_stake_key_deposits: 0,
            pointer_map: HashMap::new(),
            stake_distribution: StakeDistributionState {
                stake_map: HashMap::new(),
            },
            script_stake_credentials: HashSet::new(),
        }
    }

    /// Create a minimal EpochSubState for testing.
    fn empty_epoch_sub() -> EpochSubState {
        use crate::state::EpochSnapshots;
        EpochSubState {
            snapshots: EpochSnapshots::default(),
            treasury: Lovelace(0),
            reserves: Lovelace(0),
            pending_reward_update: None,
            pending_pp_updates: BTreeMap::new(),
            future_pp_updates: BTreeMap::new(),
            needs_stake_rebuild: false,
            ptr_stake: HashMap::new(),
            ptr_stake_excluded: false,
            protocol_params: ProtocolParameters::mainnet_defaults(),
            prev_protocol_params: ProtocolParameters::mainnet_defaults(),
            prev_protocol_version_major: 0,
            prev_d: 0.0,
        }
    }

    /// Create a minimal GovSubState for testing.
    fn empty_gov_sub() -> GovSubState {
        use crate::state::GovernanceState;
        GovSubState {
            governance: Arc::new(GovernanceState::default()),
        }
    }

    /// Create a minimal ConsensusSubState for testing.
    fn empty_consensus_sub() -> ConsensusSubState {
        ConsensusSubState {
            evolving_nonce: ZERO32,
            candidate_nonce: ZERO32,
            epoch_nonce: ZERO32,
            lab_nonce: ZERO32,
            last_epoch_block_nonce: ZERO32,
            rolling_nonce: ZERO32,
            first_block_hash_of_epoch: None,
            prev_epoch_first_block_hash: None,
            epoch_blocks_by_pool: Arc::new(HashMap::new()),
            epoch_block_count: 0,
            opcert_counters: HashMap::new(),
        }
    }

    /// Create a simple enterprise address output (no stake routing).
    fn enterprise_output(coin: u64) -> TransactionOutput {
        TransactionOutput {
            address: Address::Enterprise(EnterpriseAddress {
                network: NetworkId::Testnet,
                payment: Credential::VerificationKey(ZERO28),
            }),
            value: Value {
                coin: Lovelace(coin),
                multi_asset: BTreeMap::new(),
            },
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        }
    }

    /// Create a base address output (has stake routing via credential).
    fn base_output(coin: u64, stake_cred: Hash32) -> TransactionOutput {
        let mut h28 = [0u8; 28];
        h28.copy_from_slice(&stake_cred.as_bytes()[..28]);
        TransactionOutput {
            address: Address::Base(BaseAddress {
                network: NetworkId::Testnet,
                payment: Credential::VerificationKey(ZERO28),
                stake: Credential::VerificationKey(Hash28::from_bytes(h28)),
            }),
            value: Value {
                coin: Lovelace(coin),
                multi_asset: BTreeMap::new(),
            },
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        }
    }

    /// Create a minimal BlockHeader for testing.
    fn make_header(
        nonce_vrf_output: Vec<u8>,
        prev_hash: Hash32,
        issuer_vkey: Vec<u8>,
    ) -> BlockHeader {
        BlockHeader {
            header_hash: ZERO32,
            prev_hash,
            issuer_vkey,
            vrf_vkey: vec![],
            vrf_result: VrfOutput {
                output: vec![],
                proof: vec![],
            },
            block_number: BlockNo(0),
            slot: SlotNo(0),
            epoch_nonce: ZERO32,
            body_size: 0,
            body_hash: ZERO32,
            operational_cert: OperationalCert {
                hot_vkey: vec![],
                sequence_number: 0,
                kes_period: 0,
                sigma: vec![],
            },
            protocol_version: ProtocolVersion { major: 8, minor: 0 },
            kes_signature: vec![],
            nonce_vrf_output,
            nonce_vrf_proof: vec![],
        }
    }

    /// Create a minimal transaction with specified inputs, outputs, and fee.
    fn make_tx(
        hash: Hash32,
        inputs: Vec<TransactionInput>,
        outputs: Vec<TransactionOutput>,
        fee: u64,
    ) -> Transaction {
        Transaction {
            hash,
            era: dugite_primitives::era::Era::Babbage,
            is_valid: true,
            body: dugite_primitives::transaction::TransactionBody {
                inputs,
                outputs,
                fee: Lovelace(fee),
                ttl: None,
                certificates: vec![],
                withdrawals: BTreeMap::new(),
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint: BTreeMap::new(),
                script_data_hash: None,
                collateral: vec![],
                required_signers: vec![],
                network_id: None,
                collateral_return: None,
                total_collateral: None,
                reference_inputs: vec![],
                update: None,
                voting_procedures: BTreeMap::new(),
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: dugite_primitives::transaction::TransactionWitnessSet {
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
            auxiliary_data: None,
            raw_cbor: None,
            raw_body_cbor: None,
            raw_witness_cbor: None,
        }
    }

    // -----------------------------------------------------------------------
    // 1. apply_utxo_changes tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_apply_utxo_changes_basic_spend() {
        let mut utxo = empty_utxo_sub();
        let mut certs = empty_cert_sub();
        let mut epochs = empty_epoch_sub();

        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo.utxo_set
            .insert(input.clone(), enterprise_output(10_000_000));

        let tx = make_tx(
            Hash32::from_bytes([2u8; 32]),
            vec![input.clone()],
            vec![enterprise_output(8_000_000), enterprise_output(1_800_000)],
            200_000,
        );

        let diff = apply_utxo_changes(&tx, &mut utxo, &mut certs, &mut epochs);

        assert!(!utxo.utxo_set.contains(&input));
        let out0 = TransactionInput {
            transaction_id: tx.hash,
            index: 0,
        };
        let out1 = TransactionInput {
            transaction_id: tx.hash,
            index: 1,
        };
        assert!(utxo.utxo_set.contains(&out0));
        assert!(utxo.utxo_set.contains(&out1));
        assert_eq!(utxo.epoch_fees.0, 200_000);
        assert_eq!(diff.deletes.len(), 1);
        assert_eq!(diff.inserts.len(), 2);
    }

    #[test]
    fn test_apply_utxo_changes_missing_input_still_creates_outputs() {
        let mut utxo = empty_utxo_sub();
        let mut certs = empty_cert_sub();
        let mut epochs = empty_epoch_sub();

        let missing_input = TransactionInput {
            transaction_id: Hash32::from_bytes([99u8; 32]),
            index: 0,
        };

        let tx = make_tx(
            Hash32::from_bytes([3u8; 32]),
            vec![missing_input],
            vec![enterprise_output(5_000_000)],
            100_000,
        );

        let diff = apply_utxo_changes(&tx, &mut utxo, &mut certs, &mut epochs);

        let out0 = TransactionInput {
            transaction_id: tx.hash,
            index: 0,
        };
        assert!(utxo.utxo_set.contains(&out0));
        assert_eq!(diff.deletes.len(), 0);
        assert_eq!(diff.inserts.len(), 1);
        assert_eq!(utxo.epoch_fees.0, 100_000);
    }

    #[test]
    fn test_apply_utxo_changes_stake_distribution_updated() {
        let mut utxo = empty_utxo_sub();
        let mut certs = empty_cert_sub();
        let mut epochs = empty_epoch_sub();

        // Build a proper stake credential hash: use credential_to_hash to get
        // the typed Hash32 that stake_routing will produce from the address.
        let stake_h28 = Hash28::from_bytes([42u8; 28]);
        let stake_cred = Credential::VerificationKey(stake_h28);
        let stake_key = credential_to_hash(&stake_cred);

        certs
            .stake_distribution
            .stake_map
            .insert(stake_key, Lovelace(10_000_000));

        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([10u8; 32]),
            index: 0,
        };
        utxo.utxo_set
            .insert(input.clone(), base_output(10_000_000, stake_key));

        let tx = make_tx(
            Hash32::from_bytes([11u8; 32]),
            vec![input],
            vec![base_output(9_800_000, stake_key)],
            200_000,
        );

        apply_utxo_changes(&tx, &mut utxo, &mut certs, &mut epochs);

        // Stake should be: 10M (initial) - 10M (spent) + 9.8M (output) = 9.8M
        let stake = certs.stake_distribution.stake_map.get(&stake_key).unwrap();
        assert_eq!(stake.0, 9_800_000);
    }

    // -----------------------------------------------------------------------
    // 2. apply_collateral_consumption tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_apply_collateral_consumption_basic() {
        let mut utxo = empty_utxo_sub();
        let mut certs = empty_cert_sub();
        let mut epochs = empty_epoch_sub();

        let col_input = TransactionInput {
            transaction_id: Hash32::from_bytes([20u8; 32]),
            index: 0,
        };
        utxo.utxo_set
            .insert(col_input.clone(), enterprise_output(5_000_000));

        let mut tx = make_tx(Hash32::from_bytes([21u8; 32]), vec![], vec![], 0);
        tx.is_valid = false;
        tx.body.collateral = vec![col_input.clone()];

        let diff = apply_collateral_consumption(&tx, &mut utxo, &mut certs, &mut epochs);

        assert!(!utxo.utxo_set.contains(&col_input));
        assert_eq!(diff.deletes.len(), 1);
        assert_eq!(diff.inserts.len(), 0);
        assert_eq!(utxo.epoch_fees.0, 5_000_000);
    }

    #[test]
    fn test_apply_collateral_consumption_with_return() {
        let mut utxo = empty_utxo_sub();
        let mut certs = empty_cert_sub();
        let mut epochs = empty_epoch_sub();

        let col_input = TransactionInput {
            transaction_id: Hash32::from_bytes([30u8; 32]),
            index: 0,
        };
        utxo.utxo_set
            .insert(col_input.clone(), enterprise_output(10_000_000));

        let mut tx = make_tx(Hash32::from_bytes([31u8; 32]), vec![], vec![], 0);
        tx.is_valid = false;
        tx.body.collateral = vec![col_input.clone()];
        tx.body.collateral_return = Some(enterprise_output(8_000_000));
        tx.body.total_collateral = Some(Lovelace(2_000_000));

        let diff = apply_collateral_consumption(&tx, &mut utxo, &mut certs, &mut epochs);

        assert!(!utxo.utxo_set.contains(&col_input));
        let return_input = TransactionInput {
            transaction_id: tx.hash,
            index: 0,
        };
        assert!(utxo.utxo_set.contains(&return_input));
        assert_eq!(diff.deletes.len(), 1);
        assert_eq!(diff.inserts.len(), 1);
        assert_eq!(utxo.epoch_fees.0, 2_000_000);
    }

    // -----------------------------------------------------------------------
    // 3. process_shelley_certs tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_process_shelley_certs_stake_registration() {
        let mut certs = empty_cert_sub();
        let epochs = empty_epoch_sub();
        let mut gov = empty_gov_sub();

        let cred = Credential::VerificationKey(Hash28::from_bytes([5u8; 28]));
        let mut tx = make_tx(Hash32::from_bytes([50u8; 32]), vec![], vec![], 0);
        tx.body.certificates = vec![Certificate::StakeRegistration(cred.clone())];

        process_shelley_certs(&tx, 100, 0, &mut certs, &epochs, &mut gov);

        let key = credential_to_hash(&cred);
        assert_eq!(certs.reward_accounts.get(&key), Some(&Lovelace(0)));
        assert!(certs.stake_distribution.stake_map.contains_key(&key));
        assert_eq!(
            certs.total_stake_key_deposits,
            epochs.protocol_params.key_deposit.0
        );
        assert_eq!(
            certs.stake_key_deposits.get(&key),
            Some(&epochs.protocol_params.key_deposit.0)
        );
        let ptr = Pointer {
            slot: 100,
            tx_index: 0,
            cert_index: 0,
        };
        assert_eq!(certs.pointer_map.get(&ptr), Some(&key));
    }

    #[test]
    fn test_process_shelley_certs_stake_deregistration() {
        let mut certs = empty_cert_sub();
        let epochs = empty_epoch_sub();
        let mut gov = empty_gov_sub();

        let cred = Credential::VerificationKey(Hash28::from_bytes([6u8; 28]));
        let key = credential_to_hash(&cred);

        Arc::make_mut(&mut certs.reward_accounts).insert(key, Lovelace(500));
        Arc::make_mut(&mut certs.delegations).insert(key, Hash28::from_bytes([7u8; 28]));
        certs.stake_key_deposits.insert(key, 2_000_000);
        certs.total_stake_key_deposits = 2_000_000;

        let mut tx = make_tx(Hash32::from_bytes([51u8; 32]), vec![], vec![], 0);
        tx.body.certificates = vec![Certificate::StakeDeregistration(cred)];

        process_shelley_certs(&tx, 200, 0, &mut certs, &epochs, &mut gov);

        assert!(!certs.reward_accounts.contains_key(&key));
        assert!(!certs.delegations.contains_key(&key));
        assert_eq!(certs.total_stake_key_deposits, 0);
        assert!(!certs.stake_key_deposits.contains_key(&key));
    }

    #[test]
    fn test_process_shelley_certs_pool_registration() {
        let mut certs = empty_cert_sub();
        let epochs = empty_epoch_sub();
        let mut gov = empty_gov_sub();

        let pool_id = Hash28::from_bytes([8u8; 28]);
        let pool_params = dugite_primitives::transaction::PoolParams {
            operator: pool_id,
            vrf_keyhash: Hash32::from_bytes([9u8; 32]),
            pledge: Lovelace(1_000_000),
            cost: Lovelace(340_000_000),
            margin: dugite_primitives::transaction::Rational {
                numerator: 1,
                denominator: 100,
            },
            reward_account: vec![0xe0; 29],
            pool_owners: vec![pool_id],
            relays: vec![],
            pool_metadata: None,
        };

        let mut tx = make_tx(Hash32::from_bytes([52u8; 32]), vec![], vec![], 0);
        tx.body.certificates = vec![Certificate::PoolRegistration(pool_params)];

        process_shelley_certs(&tx, 300, 0, &mut certs, &epochs, &mut gov);

        assert!(certs.pool_params.contains_key(&pool_id));
        assert_eq!(
            certs.pool_deposits.get(&pool_id),
            Some(&epochs.protocol_params.pool_deposit.0)
        );
    }

    #[test]
    fn test_process_shelley_certs_pool_retirement() {
        let mut certs = empty_cert_sub();
        let epochs = empty_epoch_sub();
        let mut gov = empty_gov_sub();

        let pool_id = Hash28::from_bytes([10u8; 28]);

        let mut tx = make_tx(Hash32::from_bytes([53u8; 32]), vec![], vec![], 0);
        tx.body.certificates = vec![Certificate::PoolRetirement {
            pool_hash: pool_id,
            epoch: 100,
        }];

        process_shelley_certs(&tx, 400, 0, &mut certs, &epochs, &mut gov);

        assert_eq!(certs.pending_retirements.get(&pool_id), Some(&EpochNo(100)));
    }

    // -----------------------------------------------------------------------
    // 4. drain_withdrawal_accounts tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_drain_withdrawal_accounts_zeroes_balance() {
        let mut certs = empty_cert_sub();

        let mut reward_addr = vec![0xe0u8];
        reward_addr.extend_from_slice(&[11u8; 28]);
        let key = reward_account_to_hash(&reward_addr);
        Arc::make_mut(&mut certs.reward_accounts).insert(key, Lovelace(500));

        let mut tx = make_tx(Hash32::from_bytes([60u8; 32]), vec![], vec![], 0);
        tx.body.withdrawals = BTreeMap::from([(reward_addr, Lovelace(500))]);

        drain_withdrawal_accounts(&tx, &mut certs);

        assert_eq!(certs.reward_accounts.get(&key), Some(&Lovelace(0)));
    }

    #[test]
    fn test_drain_withdrawal_accounts_mismatch_still_zeroes() {
        let mut certs = empty_cert_sub();

        let mut reward_addr = vec![0xe0u8];
        reward_addr.extend_from_slice(&[12u8; 28]);
        let key = reward_account_to_hash(&reward_addr);
        Arc::make_mut(&mut certs.reward_accounts).insert(key, Lovelace(1000));

        let mut tx = make_tx(Hash32::from_bytes([61u8; 32]), vec![], vec![], 0);
        tx.body.withdrawals = BTreeMap::from([(reward_addr, Lovelace(500))]);

        drain_withdrawal_accounts(&tx, &mut certs);

        assert_eq!(certs.reward_accounts.get(&key), Some(&Lovelace(0)));
    }

    // -----------------------------------------------------------------------
    // 5. compute_shelley_nonce tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_compute_shelley_nonce_evolving_updates() {
        let mut consensus = empty_consensus_sub();
        let initial_evolving = consensus.evolving_nonce;

        let header = make_header(vec![1u8; 64], Hash32::from_bytes([2u8; 32]), vec![3u8; 32]);

        compute_shelley_nonce(&header, 100, 43200, 1000, 0.0, &mut consensus);

        assert_ne!(consensus.evolving_nonce, initial_evolving);
        assert_eq!(consensus.candidate_nonce, consensus.evolving_nonce);
        assert_eq!(consensus.lab_nonce, header.prev_hash);
        assert_eq!(consensus.epoch_block_count, 1);
        let pool_id = blake2b_224(&header.issuer_vkey);
        assert_eq!(consensus.epoch_blocks_by_pool.get(&pool_id), Some(&1));
    }

    #[test]
    fn test_compute_shelley_nonce_candidate_freezes_in_stability_window() {
        let mut consensus = empty_consensus_sub();
        let initial_candidate = consensus.candidate_nonce;

        let header = make_header(vec![4u8; 64], Hash32::from_bytes([5u8; 32]), vec![6u8; 32]);

        // 42500 + 1000 = 43500 >= 43200 -> inside stability window
        compute_shelley_nonce(&header, 42500, 43200, 1000, 0.0, &mut consensus);

        assert_ne!(consensus.evolving_nonce, ZERO32);
        assert_eq!(consensus.candidate_nonce, initial_candidate);
    }

    #[test]
    fn test_compute_shelley_nonce_overlay_blocks_not_counted() {
        let mut consensus = empty_consensus_sub();

        let header = make_header(vec![7u8; 64], Hash32::from_bytes([8u8; 32]), vec![9u8; 32]);

        // d = 0.9 >= 0.8 -> overlay -> pool blocks not counted
        compute_shelley_nonce(&header, 500, 43200, 1000, 0.9, &mut consensus);

        assert_eq!(consensus.epoch_block_count, 1);
        assert!(consensus.epoch_blocks_by_pool.is_empty());
    }

    #[test]
    fn test_compute_shelley_nonce_empty_vrf_output() {
        let mut consensus = empty_consensus_sub();
        let initial_evolving = consensus.evolving_nonce;

        let header = make_header(vec![], Hash32::from_bytes([10u8; 32]), vec![]);

        compute_shelley_nonce(&header, 100, 43200, 1000, 0.0, &mut consensus);

        assert_eq!(consensus.evolving_nonce, initial_evolving);
        assert_eq!(consensus.lab_nonce, header.prev_hash);
        assert_eq!(consensus.epoch_block_count, 1);
    }

    // -----------------------------------------------------------------------
    // 6. validate_shelley_base tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_validate_shelley_base_stub_returns_ok() {
        let utxo = empty_utxo_sub();
        let params = ProtocolParameters::mainnet_defaults();
        let tx = make_tx(Hash32::from_bytes([70u8; 32]), vec![], vec![], 0);

        let result = validate_shelley_base(&tx, &utxo, &params, 100);
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // Helper function tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_reward_account_to_hash_key_credential() {
        let mut addr = vec![0xe0u8];
        addr.extend_from_slice(&[42u8; 28]);
        let hash = reward_account_to_hash(&addr);
        assert_eq!(&hash.as_bytes()[..28], &[42u8; 28]);
        assert_eq!(hash.as_bytes()[28], 0x00);
    }

    #[test]
    fn test_reward_account_to_hash_script_credential() {
        let mut addr = vec![0xf0u8];
        addr.extend_from_slice(&[43u8; 28]);
        let hash = reward_account_to_hash(&addr);
        assert_eq!(&hash.as_bytes()[..28], &[43u8; 28]);
        assert_eq!(hash.as_bytes()[28], 0x01);
    }

    #[test]
    fn test_stake_routing_base_address() {
        let cred = Credential::VerificationKey(Hash28::from_bytes([1u8; 28]));
        let addr = Address::Base(BaseAddress {
            network: NetworkId::Testnet,
            payment: Credential::VerificationKey(ZERO28),
            stake: cred.clone(),
        });
        match stake_routing(&addr, false) {
            StakeRouting::Credential(h) => {
                assert_eq!(h, credential_to_hash(&cred));
            }
            _ => panic!("Expected StakeRouting::Credential for base address"),
        }
    }

    #[test]
    fn test_stake_routing_pointer_excluded_in_conway() {
        let addr = Address::Pointer(PointerAddress {
            network: NetworkId::Testnet,
            payment: Credential::VerificationKey(ZERO28),
            pointer: Pointer {
                slot: 1,
                tx_index: 0,
                cert_index: 0,
            },
        });
        match stake_routing(&addr, true) {
            StakeRouting::None => {}
            _ => panic!("Expected StakeRouting::None for pointer address in Conway"),
        }
        match stake_routing(&addr, false) {
            StakeRouting::Pointer(_) => {}
            _ => panic!("Expected StakeRouting::Pointer for pointer address pre-Conway"),
        }
    }
}
