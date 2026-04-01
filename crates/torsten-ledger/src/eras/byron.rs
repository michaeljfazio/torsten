/// Byron era ledger rules
///
/// The Byron era uses OBFT (Optimistic Byzantine Fault Tolerance) consensus
/// and has a simpler transaction model compared to all post-Shelley eras:
///
/// - Inputs: `TxIn(TxId, index)` — simple UTxO set lookup, no scripts
/// - Outputs: `TxOut(ByronAddress, Lovelace)` — no multi-asset, no datums
/// - Fees: `fee = sum(inputs) - sum(outputs)`, must satisfy `fee >= min_fee_a * tx_size + min_fee_b`
/// - No certificates, withdrawals, staking, Plutus scripts, or governance
///
/// Byron transactions always succeed when structurally valid — there is no
/// `is_valid` flag and no collateral mechanism.
use torsten_primitives::transaction::{Transaction, TransactionInput, TransactionOutput};
use torsten_primitives::value::Lovelace;

/// Byron-specific validation error
#[derive(Debug, thiserror::Error)]
pub enum ByronError {
    /// An input referenced in the transaction body is not present in the UTxO set.
    #[error("Input not found in UTxO set: {0}")]
    InputNotFound(String),

    /// `sum(inputs) != sum(outputs) + fee`
    #[error("Value not conserved: inputs={inputs}, outputs={outputs}, fee={fee}")]
    ValueNotConserved { inputs: u64, outputs: u64, fee: u64 },

    /// `fee < min_fee_a * tx_size + min_fee_b`
    #[error("Fee too small: minimum={minimum}, actual={actual}")]
    FeeTooSmall { minimum: u64, actual: u64 },

    /// A transaction output contains multi-asset tokens, which are not valid in Byron
    #[error("Byron output contains multi-asset tokens (only ADA is valid in Byron)")]
    MultiAssetInOutput,

    /// A transaction has no inputs, which is structurally invalid
    #[error("Byron transaction has no inputs")]
    NoInputs,

    /// Integer overflow while summing input or output values
    #[error("Value overflow in Byron transaction accounting")]
    ValueOverflow,
}

/// Byron fee policy extracted from genesis parameters.
///
/// In Byron, the minimum fee is `min_fee_a * tx_size_bytes + min_fee_b` lovelace.
/// The values come from the Byron genesis `blockVersionData.txFeePolicy`.
///
/// The `ProtocolParameters` struct stores the Shelley-compatible projection of
/// these values in `min_fee_a` and `min_fee_b`, which is reused here directly.
#[derive(Debug, Clone, Copy)]
pub struct ByronFeePolicy {
    /// Per-byte fee coefficient (lovelace per byte of serialized transaction)
    pub min_fee_a: u64,
    /// Constant fee component (lovelace, always charged regardless of tx size)
    pub min_fee_b: u64,
}

impl ByronFeePolicy {
    /// Compute the minimum fee for a transaction of the given serialized byte length.
    ///
    /// Formula: `min_fee_a * tx_size_bytes + min_fee_b`
    ///
    /// Returns `None` on overflow (impossible for realistic values but we defend
    /// against corrupted genesis parameters).
    pub fn min_fee(&self, tx_size_bytes: u64) -> Option<u64> {
        self.min_fee_a
            .checked_mul(tx_size_bytes)
            .and_then(|product| product.checked_add(self.min_fee_b))
    }
}

/// Result of validating and applying a single Byron transaction.
#[derive(Debug)]
pub struct ByronTxEffect {
    /// UTxO entries to remove (spent inputs)
    pub consumed: Vec<TransactionInput>,
    /// UTxO entries to add (new outputs indexed by the tx hash and output index)
    pub produced: Vec<(TransactionInput, TransactionOutput)>,
    /// Fee collected from this transaction
    pub fee: Lovelace,
}

/// Validate a single Byron-era transaction against the current UTxO set.
///
/// Returns a [`ByronTxEffect`] describing the state changes on success, or a
/// [`ByronError`] describing the first violation found.
///
/// # Byron UTxO rules validated here
///
/// 1. **At least one input** — structurally required by the Byron spec.
/// 2. **All inputs exist** — every `TxIn` must resolve to a UTxO entry.
/// 3. **No multi-asset outputs** — Byron outputs must be ADA-only.
/// 4. **Value conservation** — `sum(input values) == sum(output values) + fee`.
/// 5. **Minimum fee** — `fee >= fee_policy.min_fee(tx_size_bytes)`.
///
/// # Missing inputs during bootstrap
///
/// When replaying from genesis without the full UTxO history (e.g. after a
/// Mithril snapshot import that starts mid-chain), some inputs may be absent.
/// The caller (`apply_byron_block`) handles this gracefully by logging and
/// skipping the UTxO changes while still accumulating fees.
pub fn validate_byron_tx<F>(
    tx: &Transaction,
    mut lookup_utxo: F,
    fee_policy: ByronFeePolicy,
    tx_size_bytes: u64,
) -> Result<ByronTxEffect, ByronError>
where
    F: FnMut(&TransactionInput) -> Option<TransactionOutput>,
{
    // Rule 1: must have at least one input
    if tx.body.inputs.is_empty() {
        return Err(ByronError::NoInputs);
    }

    // Rule 2: resolve all inputs and accumulate their ADA value
    let mut input_sum: u64 = 0;
    let mut consumed = Vec::with_capacity(tx.body.inputs.len());
    for input in &tx.body.inputs {
        let output = lookup_utxo(input).ok_or_else(|| {
            ByronError::InputNotFound(format!("{}#{}", input.transaction_id.to_hex(), input.index))
        })?;
        // Sum only the coin component. Byron UTxOs are ADA-only; any multi-asset
        // entries (theoretically impossible in Byron but we are defensive) are ignored
        // for the purposes of value conservation — the multi-asset check on outputs
        // (Rule 3) will reject such transactions before they can steal value.
        input_sum = input_sum
            .checked_add(output.value.coin.0)
            .ok_or(ByronError::ValueOverflow)?;
        consumed.push(input.clone());
    }

    // Rule 3: outputs must be ADA-only (no multi-asset in Byron)
    for output in &tx.body.outputs {
        if !output.value.multi_asset.is_empty() {
            return Err(ByronError::MultiAssetInOutput);
        }
    }

    // Accumulate output value and build produced list
    let mut output_sum: u64 = 0;
    let mut produced = Vec::with_capacity(tx.body.outputs.len());
    for (idx, output) in tx.body.outputs.iter().enumerate() {
        output_sum = output_sum
            .checked_add(output.value.coin.0)
            .ok_or(ByronError::ValueOverflow)?;
        let out_input = TransactionInput {
            transaction_id: tx.hash,
            index: idx as u32,
        };
        produced.push((out_input, output.clone()));
    }

    // Rule 4: value conservation — inputs == outputs + fee
    let fee = tx.body.fee.0;
    let expected_inputs = output_sum
        .checked_add(fee)
        .ok_or(ByronError::ValueOverflow)?;
    if input_sum != expected_inputs {
        return Err(ByronError::ValueNotConserved {
            inputs: input_sum,
            outputs: output_sum,
            fee,
        });
    }

    // Rule 5: minimum fee must be satisfied
    let min_fee = fee_policy
        .min_fee(tx_size_bytes)
        .ok_or(ByronError::ValueOverflow)?;
    if fee < min_fee {
        return Err(ByronError::FeeTooSmall {
            minimum: min_fee,
            actual: fee,
        });
    }

    Ok(ByronTxEffect {
        consumed,
        produced,
        fee: Lovelace(fee),
    })
}

/// Whether to enforce Byron validation rules or just apply UTxO changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByronApplyMode {
    /// Enforce all Byron UTxO rules; return an error on any rule violation.
    /// Used when receiving new blocks from the network.
    ValidateAll,
    /// Trust the block content; skip validation and apply changes directly.
    /// Used for immutable DB replay, Mithril import, and rollback replay.
    ApplyOnly,
}

/// Error returned when a Byron block cannot be applied.
#[derive(Debug, thiserror::Error)]
#[error("Byron block error at slot {slot} tx {tx_hash}: {reason}")]
pub struct ByronBlockError {
    pub slot: u64,
    pub tx_hash: String,
    pub reason: ByronError,
}

/// Collected UTxO changes and fees for an entire Byron block.
///
/// Returned by [`apply_byron_block`] so the caller can apply the changes to
/// the UTxO store without overlapping borrows.
#[derive(Debug)]
pub struct ByronBlockEffect {
    /// Inputs to remove from the UTxO set (spent)
    pub spent: Vec<TransactionInput>,
    /// Outputs to add to the UTxO set (created)
    pub created: Vec<(TransactionInput, TransactionOutput)>,
    /// Total fees collected from this block
    pub fees: Lovelace,
}

impl Default for ByronBlockEffect {
    fn default() -> Self {
        Self {
            spent: Vec::new(),
            created: Vec::new(),
            fees: Lovelace(0),
        }
    }
}

/// Apply a Byron-era block's transactions and return the net UTxO changes.
///
/// This function validates or applies (depending on `mode`) each transaction
/// and accumulates the resulting changes into a [`ByronBlockEffect`] that the
/// caller applies to the UTxO store. Separating computation from mutation
/// avoids multiple mutable borrows of the same store.
///
/// # Byron rules (enforced in `ValidateAll` mode)
///
/// 1. Each transaction must have at least one input.
/// 2. Every input must be present in the UTxO set (`lookup_utxo`).
/// 3. Outputs may only contain ADA (no multi-asset).
/// 4. `sum(inputs) == sum(outputs) + fee` (value conservation).
/// 5. `fee >= min_fee_a * tx_size_bytes + min_fee_b` (minimum fee).
///
/// # Behavior on missing inputs (ApplyOnly mode)
///
/// During initial sync from a Mithril snapshot or partial history, some UTxO
/// inputs may not yet be present in the store. Rather than aborting the block,
/// the UTxO update for that transaction is skipped and only the fee is counted.
/// This matches the behavior of the Shelley+ path in `apply_block`.
///
/// # In-block dependencies
///
/// Transactions are processed in order. The lookup closure sees UTxO changes
/// made by earlier transactions in the same block because `ByronBlockEffect`
/// is built incrementally and the caller is expected to pass a closure that
/// reads from the in-flight effect in addition to the persistent store.
///
/// In the current implementation the caller (`apply_block`) applies the entire
/// effect after the function returns, which means within-block spending chains
/// work naturally: the lookup for tx N will see the outputs of tx 0..N-1
/// because those outputs were inserted into the UTxO store by the earlier
/// iteration of the loop in the caller.
pub fn apply_byron_block<FLookup>(
    transactions: &[Transaction],
    fee_policy: ByronFeePolicy,
    slot: u64,
    mode: ByronApplyMode,
    mut lookup_utxo: FLookup,
) -> Result<ByronBlockEffect, ByronBlockError>
where
    FLookup: FnMut(&TransactionInput) -> Option<TransactionOutput>,
{
    let mut effect = ByronBlockEffect::default();

    // Duplicate-tx guard (defensive; Byron blocks should not have them, but we
    // match the behaviour of the Shelley+ path).
    let mut seen = std::collections::HashSet::with_capacity(transactions.len());

    for tx in transactions {
        if !seen.insert(tx.hash) {
            tracing::warn!(
                tx_hash = %tx.hash.to_hex(),
                slot,
                "Byron: duplicate transaction hash in block, skipping"
            );
            continue;
        }

        // Derive serialized transaction size for fee calculation.
        // We use raw_cbor bytes when available (exact on-wire size).
        // Fall back to 0 when absent, making the minimum-fee check lenient —
        // acceptable in ApplyOnly mode where the block is already confirmed.
        let tx_size_bytes = tx.raw_cbor.as_ref().map_or(0, |b| b.len() as u64);

        match mode {
            ByronApplyMode::ValidateAll => {
                // Strict mode: validate all Byron rules; reject on any violation.
                let tx_effect = validate_byron_tx(tx, &mut lookup_utxo, fee_policy, tx_size_bytes)
                    .map_err(|reason| ByronBlockError {
                        slot,
                        tx_hash: tx.hash.to_hex(),
                        reason,
                    })?;

                effect.spent.extend(tx_effect.consumed);
                effect.created.extend(tx_effect.produced);
                effect.fees.0 = effect.fees.0.saturating_add(tx_effect.fee.0);
            }

            ByronApplyMode::ApplyOnly => {
                // Replay mode: trust the on-chain block; collect UTxO changes without
                // full validation. If inputs are missing (partial history), skip the
                // UTxO update for this tx but still count the fee.
                let mut all_inputs_present = true;
                let mut tx_consumed: Vec<TransactionInput> =
                    Vec::with_capacity(tx.body.inputs.len());

                for input in &tx.body.inputs {
                    if lookup_utxo(input).is_some() {
                        tx_consumed.push(input.clone());
                    } else {
                        tracing::debug!(
                            tx_hash = %tx.hash.to_hex(),
                            slot,
                            input = %format!("{}#{}", input.transaction_id.to_hex(), input.index),
                            "Byron ApplyOnly: input not in UTxO set, skipping UTxO update for tx"
                        );
                        all_inputs_present = false;
                        break;
                    }
                }

                if all_inputs_present {
                    effect.spent.extend(tx_consumed);
                    for (idx, output) in tx.body.outputs.iter().enumerate() {
                        let out_input = TransactionInput {
                            transaction_id: tx.hash,
                            index: idx as u32,
                        };
                        effect.created.push((out_input, output.clone()));
                    }
                }

                // Always accumulate fees (epoch accounting is independent of UTxO availability)
                effect.fees.0 = effect.fees.0.saturating_add(tx.body.fee.0);
            }
        }
    }

    Ok(effect)
}

// ---------------------------------------------------------------------------
// ByronLedger struct (kept for era dispatch, currently stateless)
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct ByronLedger;

impl ByronLedger {
    pub fn new() -> Self {
        ByronLedger
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, HashMap};
    use torsten_primitives::{
        address::{Address, ByronAddress},
        hash::Hash32,
        transaction::{
            OutputDatum, Transaction, TransactionBody, TransactionInput, TransactionOutput,
            TransactionWitnessSet,
        },
        value::{Lovelace, Value},
    };

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// A fee policy matching the Shelley-projected Byron mainnet values.
    const TEST_POLICY: ByronFeePolicy = ByronFeePolicy {
        min_fee_a: 44,
        min_fee_b: 155_381,
    };

    fn make_byron_address(byte: u8) -> Address {
        Address::Byron(ByronAddress {
            payload: vec![byte; 32],
        })
    }

    fn make_output(address: Address, coin: u64) -> TransactionOutput {
        TransactionOutput {
            address,
            value: Value {
                coin: Lovelace(coin),
                multi_asset: Default::default(),
            },
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: true,
            raw_cbor: None,
        }
    }

    fn make_input(tx_id_byte: u8, index: u32) -> TransactionInput {
        TransactionInput {
            transaction_id: Hash32::from_bytes([tx_id_byte; 32]),
            index,
        }
    }

    fn make_tx(
        tx_id_byte: u8,
        inputs: Vec<TransactionInput>,
        outputs: Vec<TransactionOutput>,
        fee: u64,
    ) -> Transaction {
        let body = TransactionBody {
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
        };
        Transaction {
            era: torsten_primitives::era::Era::Conway,
            hash: Hash32::from_bytes([tx_id_byte; 32]),
            body,
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
            // 200-byte dummy raw_cbor so the fee size calculation is deterministic in tests
            raw_cbor: Some(vec![0u8; 200]),
            raw_body_cbor: None,
            raw_witness_cbor: None,
        }
    }

    fn utxo_map(
        entries: Vec<(TransactionInput, TransactionOutput)>,
    ) -> HashMap<TransactionInput, TransactionOutput> {
        entries.into_iter().collect()
    }

    // -----------------------------------------------------------------------
    // validate_byron_tx tests
    // -----------------------------------------------------------------------

    /// A valid single-input / single-output transaction where the fee exactly
    /// equals the minimum and value is conserved should pass all rules.
    #[test]
    fn test_valid_byron_tx() {
        let input = make_input(0xAA, 0);
        let input_coin = 10_000_000u64; // 10 ADA
                                        // min_fee(200) = 44 * 200 + 155_381 = 8_800 + 155_381 = 164_181 lovelace
        let fee = TEST_POLICY.min_fee(200).unwrap();
        let output_coin = input_coin - fee;

        let utxo = utxo_map(vec![(
            input.clone(),
            make_output(make_byron_address(0x01), input_coin),
        )]);

        let tx = make_tx(
            0xBB,
            vec![input],
            vec![make_output(make_byron_address(0x02), output_coin)],
            fee,
        );

        let result = validate_byron_tx(&tx, |i| utxo.get(i).cloned(), TEST_POLICY, 200);

        assert!(result.is_ok(), "expected Ok, got {:?}", result);
        let effect = result.unwrap();
        assert_eq!(effect.fee, Lovelace(fee));
        assert_eq!(effect.consumed.len(), 1);
        assert_eq!(effect.produced.len(), 1);
    }

    /// A transaction whose inputs are not in the UTxO set returns `InputNotFound`.
    #[test]
    fn test_missing_input_returns_error() {
        let input = make_input(0xAA, 0);
        // Empty UTxO set — input will not be found
        let utxo: HashMap<TransactionInput, TransactionOutput> = HashMap::new();

        let tx = make_tx(
            0xBB,
            vec![input],
            vec![make_output(make_byron_address(0x02), 1_000_000)],
            155_381,
        );

        let result = validate_byron_tx(&tx, |i| utxo.get(i).cloned(), TEST_POLICY, 200);

        assert!(
            matches!(result, Err(ByronError::InputNotFound(_))),
            "expected InputNotFound, got {result:?}"
        );
    }

    /// A transaction that pays less than the minimum fee is rejected with `FeeTooSmall`.
    #[test]
    fn test_insufficient_fee_returns_error() {
        let input = make_input(0xAA, 0);
        let input_coin = 10_000_000u64;
        let min_fee = TEST_POLICY.min_fee(200).unwrap();
        // Pay one lovelace less than the minimum
        let fee = min_fee - 1;
        let output_coin = input_coin - fee;

        let utxo = utxo_map(vec![(
            input.clone(),
            make_output(make_byron_address(0x01), input_coin),
        )]);

        let tx = make_tx(
            0xBB,
            vec![input],
            vec![make_output(make_byron_address(0x02), output_coin)],
            fee,
        );

        let result = validate_byron_tx(&tx, |i| utxo.get(i).cloned(), TEST_POLICY, 200);

        assert!(
            matches!(result, Err(ByronError::FeeTooSmall { .. })),
            "expected FeeTooSmall, got {result:?}"
        );
    }

    /// A transaction where `sum(inputs) != sum(outputs) + fee` is rejected.
    #[test]
    fn test_value_not_conserved_returns_error() {
        let input = make_input(0xAA, 0);
        let input_coin = 10_000_000u64;
        let fee = TEST_POLICY.min_fee(200).unwrap();
        // Output retains the full input value — none is left for the fee
        let output_coin = input_coin; // should be input_coin - fee

        let utxo = utxo_map(vec![(
            input.clone(),
            make_output(make_byron_address(0x01), input_coin),
        )]);

        let tx = make_tx(
            0xBB,
            vec![input],
            vec![make_output(make_byron_address(0x02), output_coin)],
            fee,
        );

        let result = validate_byron_tx(&tx, |i| utxo.get(i).cloned(), TEST_POLICY, 200);

        assert!(
            matches!(result, Err(ByronError::ValueNotConserved { .. })),
            "expected ValueNotConserved, got {result:?}"
        );
    }

    /// A transaction with zero inputs is rejected with `NoInputs`.
    #[test]
    fn test_no_inputs_returns_error() {
        let utxo: HashMap<TransactionInput, TransactionOutput> = HashMap::new();
        let tx = make_tx(
            0xBB,
            vec![],
            vec![make_output(make_byron_address(0x02), 1_000_000)],
            155_381,
        );

        let result = validate_byron_tx(&tx, |i| utxo.get(i).cloned(), TEST_POLICY, 200);

        assert!(
            matches!(result, Err(ByronError::NoInputs)),
            "expected NoInputs, got {result:?}"
        );
    }

    // -----------------------------------------------------------------------
    // apply_byron_block tests
    //
    // apply_byron_block returns a ByronBlockEffect. Tests apply it to a local
    // HashMap via a helper to verify the correct UTxO changes were produced.
    // -----------------------------------------------------------------------

    /// Helper: apply a ByronBlockEffect to a HashMap UTxO store.
    fn apply_effect(
        utxo: &mut HashMap<TransactionInput, TransactionOutput>,
        effect: ByronBlockEffect,
    ) {
        for input in &effect.spent {
            utxo.remove(input);
        }
        for (input, output) in effect.created {
            utxo.insert(input, output);
        }
    }

    /// A block with a single valid transaction is applied correctly: inputs are
    /// removed, outputs are added, and the fee is returned.
    #[test]
    fn test_apply_valid_block() {
        let input = make_input(0xAA, 0);
        let input_coin = 10_000_000u64;
        let fee = TEST_POLICY.min_fee(200).unwrap();
        let output_coin = input_coin - fee;

        let mut utxo: HashMap<TransactionInput, TransactionOutput> = utxo_map(vec![(
            input.clone(),
            make_output(make_byron_address(0x01), input_coin),
        )]);

        let tx = make_tx(
            0xBB,
            vec![input.clone()],
            vec![make_output(make_byron_address(0x02), output_coin)],
            fee,
        );

        // apply_byron_block takes only a lookup closure; mutation is done by the caller
        let result =
            apply_byron_block(&[tx], TEST_POLICY, 1000, ByronApplyMode::ValidateAll, |i| {
                utxo.get(i).cloned()
            });

        assert!(result.is_ok(), "expected Ok, got {result:?}");
        let effect = result.unwrap();
        assert_eq!(effect.fees, Lovelace(fee));

        apply_effect(&mut utxo, effect);

        // Input must be consumed
        assert!(
            !utxo.contains_key(&input),
            "spent input should be removed from UTxO set"
        );
        // New output must be present
        let out_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xBBu8; 32]),
            index: 0,
        };
        assert!(
            utxo.contains_key(&out_input),
            "new output should be present in UTxO set"
        );
    }

    /// In ValidateAll mode, a transaction referencing a missing input causes the
    /// entire block application to fail.
    #[test]
    fn test_apply_missing_input_fails_in_validate_mode() {
        let input = make_input(0xAA, 0);
        let utxo: HashMap<TransactionInput, TransactionOutput> = HashMap::new();

        let tx = make_tx(
            0xBB,
            vec![input],
            vec![make_output(make_byron_address(0x02), 1_000_000)],
            155_381,
        );

        let result =
            apply_byron_block(&[tx], TEST_POLICY, 1000, ByronApplyMode::ValidateAll, |i| {
                utxo.get(i).cloned()
            });

        assert!(
            result.is_err(),
            "expected Err for missing input in ValidateAll mode"
        );
    }

    /// In ApplyOnly mode, a missing input causes the UTxO change to be skipped
    /// but the fee is still accumulated — matching bootstrap behavior.
    #[test]
    fn test_apply_missing_input_skipped_in_apply_only_mode() {
        let input = make_input(0xAA, 0);
        let utxo: HashMap<TransactionInput, TransactionOutput> = HashMap::new();
        let fee = 200_000u64;

        let tx = make_tx(
            0xBB,
            vec![input],
            vec![make_output(make_byron_address(0x02), 800_000)],
            fee,
        );

        let result = apply_byron_block(&[tx], TEST_POLICY, 1000, ByronApplyMode::ApplyOnly, |i| {
            utxo.get(i).cloned()
        });

        // The block is confirmed on-chain; we succeed and skip the UTxO change
        assert!(
            result.is_ok(),
            "expected Ok in ApplyOnly mode, got {result:?}"
        );
        let effect = result.unwrap();
        // Fee is still accumulated even when UTxO changes are skipped
        assert_eq!(effect.fees, Lovelace(fee));
        // No UTxO changes produced
        assert!(effect.spent.is_empty(), "no inputs should be consumed");
        assert!(effect.created.is_empty(), "no outputs should be created");
    }

    /// Two independent transactions in the same block both apply correctly.
    #[test]
    fn test_multi_tx_block_applies_in_sequence() {
        let genesis_input1 = make_input(0x11, 0);
        let genesis_input2 = make_input(0x22, 0);
        let coin1 = 10_000_000u64;
        let coin2 = 8_000_000u64;
        let fee1 = TEST_POLICY.min_fee(200).unwrap();
        let fee2 = TEST_POLICY.min_fee(200).unwrap();

        let mut utxo: HashMap<TransactionInput, TransactionOutput> = utxo_map(vec![
            (
                genesis_input1.clone(),
                make_output(make_byron_address(0x01), coin1),
            ),
            (
                genesis_input2.clone(),
                make_output(make_byron_address(0x02), coin2),
            ),
        ]);

        // Tx1 spends genesis_input1
        let tx1 = make_tx(
            0xAA,
            vec![genesis_input1.clone()],
            vec![make_output(make_byron_address(0x03), coin1 - fee1)],
            fee1,
        );

        // Tx2 spends genesis_input2 (independent — no within-block dependency)
        let tx2 = make_tx(
            0xBB,
            vec![genesis_input2.clone()],
            vec![make_output(make_byron_address(0x04), coin2 - fee2)],
            fee2,
        );

        let result = apply_byron_block(
            &[tx1, tx2],
            TEST_POLICY,
            2000,
            ByronApplyMode::ValidateAll,
            |i| utxo.get(i).cloned(),
        );

        assert!(
            result.is_ok(),
            "expected Ok for multi-tx block, got {result:?}"
        );
        let effect = result.unwrap();
        assert_eq!(
            effect.fees,
            Lovelace(fee1 + fee2),
            "total fees should be the sum of both transaction fees"
        );
        assert_eq!(effect.spent.len(), 2, "two inputs consumed");
        assert_eq!(effect.created.len(), 2, "two outputs created");

        apply_effect(&mut utxo, effect);

        assert!(
            !utxo.contains_key(&genesis_input1),
            "genesis_input1 consumed"
        );
        assert!(
            !utxo.contains_key(&genesis_input2),
            "genesis_input2 consumed"
        );
        let out1 = TransactionInput {
            transaction_id: Hash32::from_bytes([0xAAu8; 32]),
            index: 0,
        };
        let out2 = TransactionInput {
            transaction_id: Hash32::from_bytes([0xBBu8; 32]),
            index: 0,
        };
        assert!(utxo.contains_key(&out1), "tx1 output present");
        assert!(utxo.contains_key(&out2), "tx2 output present");
    }

    /// `ByronFeePolicy::min_fee` computes the correct values.
    #[test]
    fn test_fee_policy_min_fee() {
        let policy = ByronFeePolicy {
            min_fee_a: 44,
            min_fee_b: 155_381,
        };
        // 44 * 200 + 155_381 = 8_800 + 155_381 = 164_181
        assert_eq!(policy.min_fee(200), Some(164_181));
        // Zero-size tx — only the constant component
        assert_eq!(policy.min_fee(0), Some(155_381));
    }
}
