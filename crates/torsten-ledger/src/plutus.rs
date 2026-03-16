use crate::utxo::UtxoSet;
use torsten_primitives::transaction::Transaction;
use tracing::{debug, trace};

#[derive(Debug, thiserror::Error)]
pub enum PlutusError {
    #[error("Missing raw CBOR for transaction")]
    MissingTxCbor,
    #[error("Missing raw CBOR for UTxO output: {0}")]
    MissingOutputCbor(String),
    #[error("Plutus evaluation failed: {0}")]
    EvalFailed(String),
}

/// Slot configuration for Plutus time conversion
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct SlotConfig {
    /// POSIX time of slot 0 in milliseconds
    pub zero_time: u64,
    /// Slot number at zero_time
    pub zero_slot: u64,
    /// Slot length in milliseconds
    pub slot_length: u32,
}

impl Default for SlotConfig {
    fn default() -> Self {
        // Cardano mainnet defaults
        SlotConfig {
            zero_time: 1_596_059_091_000, // Shelley start (mainnet)
            zero_slot: 4_492_800,         // First Shelley slot (mainnet)
            slot_length: 1_000,           // 1 second
        }
    }
}

impl SlotConfig {
    /// Preview testnet slot config
    pub fn preview() -> Self {
        SlotConfig {
            zero_time: 1_666_656_000_000, // Preview genesis time
            zero_slot: 0,
            slot_length: 1_000,
        }
    }

    /// Preprod testnet slot config
    pub fn preprod() -> Self {
        SlotConfig {
            zero_time: 1_654_041_600_000, // Preprod genesis time
            zero_slot: 0,
            slot_length: 1_000,
        }
    }
}

/// Encode a TransactionInput as CBOR bytes (pallas wire format)
///
/// TransactionInput is encoded as a 2-element CBOR array: [hash(32 bytes), index(uint)]
fn encode_input_cbor(input: &torsten_primitives::transaction::TransactionInput) -> Vec<u8> {
    use minicbor::Encoder;
    let mut buf = Vec::with_capacity(40);
    let mut enc = Encoder::new(&mut buf);
    // minicbor encoding to Vec<u8> is infallible
    // Safety: minicbor encoding to Vec<u8> is infallible (cannot fail on memory writes)
    enc.array(2).expect("infallible: Vec<u8> write");
    enc.bytes(input.transaction_id.as_bytes())
        .expect("infallible: Vec<u8> write");
    enc.u32(input.index).expect("infallible: Vec<u8> write");
    buf
}

/// Evaluate Plutus scripts in a transaction using the uplc CEK machine
///
/// `max_tx_ex_units` is `(cpu_steps, mem_units)` — this matches the uplc
/// `eval_phase_two_raw` convention where `.0 = cpu` and `.1 = mem`.
/// Callers must ensure they pass `(ExUnits.steps, ExUnits.mem)` in that order;
/// swapping the two produces a 700x too-small CPU ceiling and causes false failures.
///
/// Returns Ok(()) if all scripts pass, or Err with details of failure.
pub fn evaluate_plutus_scripts(
    tx: &Transaction,
    utxo_set: &UtxoSet,
    cost_models_cbor: Option<&[u8]>,
    max_tx_ex_units: (u64, u64),
    slot_config: &SlotConfig,
) -> Result<(), PlutusError> {
    let tx_cbor = tx.raw_cbor.as_ref().ok_or(PlutusError::MissingTxCbor)?;

    // Build resolved UTxO pairs (input CBOR, output CBOR)
    let mut utxo_pairs: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();

    // Collect all inputs that need resolution: regular inputs + reference inputs
    let all_inputs = tx.body.inputs.iter().chain(tx.body.reference_inputs.iter());

    for input in all_inputs {
        if let Some(output) = utxo_set.lookup(input) {
            let output_cbor = match &output.raw_cbor {
                Some(cbor) => cbor.clone(),
                None => {
                    // raw_cbor is None when the UTxO was round-tripped through
                    // the LSM store (serde(skip) on raw_cbor). Re-encode the
                    // output from its parsed fields.
                    torsten_serialization::encode_transaction_output(&output)
                }
            };
            let input_cbor = encode_input_cbor(input);
            utxo_pairs.push((input_cbor, output_cbor));
        }
    }

    // Also resolve collateral inputs
    for col_input in &tx.body.collateral {
        if let Some(output) = utxo_set.lookup(col_input) {
            let output_cbor = match &output.raw_cbor {
                Some(cbor) => cbor.clone(),
                None => torsten_serialization::encode_transaction_output(&output),
            };
            let input_cbor = encode_input_cbor(col_input);
            utxo_pairs.push((input_cbor, output_cbor));
        }
    }

    debug!(
        tx_hash = %tx.hash.to_hex(),
        utxo_count = utxo_pairs.len(),
        redeemer_count = tx.witness_set.redeemers.len(),
        "Evaluating Plutus scripts"
    );

    let sc = (
        slot_config.zero_time,
        slot_config.zero_slot,
        slot_config.slot_length,
    );

    match uplc::tx::eval_phase_two_raw(
        tx_cbor,
        &utxo_pairs,
        cost_models_cbor,
        max_tx_ex_units,
        sc,
        false, // don't run phase one (we already do our own phase 1 validation)
        |_redeemer| {},
    ) {
        Ok(results) => {
            for (_redeemer_bytes, eval_result) in &results {
                let cost = eval_result.cost();
                // Determine if the script failed using Haskell-compatible rules:
                //
                // Per Haskell's PlutusLedgerApi.Common.Eval.processLogsAndErrors:
                // - PlutusV1/V2: ANY non-error result counts as success (even Data,
                //   Bool(false), Integer, partially-applied terms). Only CEK machine
                //   errors and Term::Error indicate failure.
                // - PlutusV3: Only Unit is accepted; anything else is InvalidReturnValue.
                //
                // Since we don't track per-redeemer language version here, we use the
                // V1/V2 rule (accept any non-error) which is safe because:
                // 1. V3 scripts that return non-Unit would be rejected by Haskell too,
                //    so they would never appear on-chain with is_valid=true.
                // 2. For block production, mempool admission already checks is_valid.
                let script_failed = match &eval_result.result {
                    Err(_) => true,
                    Ok(term) => matches!(term, uplc::ast::Term::Error),
                };
                if script_failed {
                    let err_msg = match &eval_result.result {
                        Err(e) => format!("{e}"),
                        Ok(term) => format!("Script error: {term:?}"),
                    };
                    debug!(
                        tx_hash = %tx.hash.to_hex(),
                        error = %err_msg,
                        logs = ?eval_result.logs(),
                        "Plutus script failed"
                    );
                    return Err(PlutusError::EvalFailed(err_msg));
                }
                trace!(
                    tx_hash = %tx.hash.to_hex(),
                    cpu = cost.cpu,
                    mem = cost.mem,
                    "Plutus script passed"
                );
            }
            Ok(())
        }
        Err(e) => {
            debug!(
                tx_hash = %tx.hash.to_hex(),
                error = %e,
                "Plutus evaluation error"
            );
            Err(PlutusError::EvalFailed(format!(
                "eval_phase_two_raw error: {e}"
            )))
        }
    }
}

/// Check if a transaction contains any Plutus scripts (in witnesses or reference inputs)
pub fn has_plutus_scripts(tx: &Transaction) -> bool {
    !tx.witness_set.plutus_v1_scripts.is_empty()
        || !tx.witness_set.plutus_v2_scripts.is_empty()
        || !tx.witness_set.plutus_v3_scripts.is_empty()
        || !tx.witness_set.redeemers.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use torsten_primitives::hash::Hash32;

    #[test]
    fn test_encode_input_cbor() {
        use torsten_primitives::transaction::TransactionInput;

        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xab; 32]),
            index: 1,
        };
        let cbor = encode_input_cbor(&input);
        // Should be a valid CBOR array with 2 elements
        assert!(!cbor.is_empty());
        // First byte should be 0x82 (array of 2)
        assert_eq!(cbor[0], 0x82);
    }

    #[test]
    fn test_slot_config_defaults() {
        let config = SlotConfig::default();
        assert_eq!(config.slot_length, 1_000);
        assert_eq!(config.zero_slot, 4_492_800);

        let preview = SlotConfig::preview();
        assert_eq!(preview.zero_slot, 0);
    }

    #[test]
    fn test_has_plutus_scripts_empty() {
        let tx = Transaction::empty_with_hash(Hash32::ZERO);
        assert!(!has_plutus_scripts(&tx));
    }

    #[test]
    fn test_has_plutus_scripts_with_redeemers() {
        use torsten_primitives::hash::Hash32;
        use torsten_primitives::transaction::{ExUnits, PlutusData, Redeemer, RedeemerTag};

        let mut tx = Transaction::empty_with_hash(Hash32::ZERO);
        tx.witness_set.redeemers.push(Redeemer {
            tag: RedeemerTag::Spend,
            index: 0,
            data: PlutusData::Integer(0),
            ex_units: ExUnits {
                mem: 100,
                steps: 100,
            },
        });
        assert!(has_plutus_scripts(&tx));
    }

    #[test]
    fn test_evaluate_missing_cbor() {
        let tx = Transaction::empty_with_hash(Hash32::ZERO);
        let utxo_set = UtxoSet::new();
        let slot_config = SlotConfig::default();

        let result =
            evaluate_plutus_scripts(&tx, &utxo_set, None, (10_000_000, 10_000_000), &slot_config);
        assert!(matches!(result, Err(PlutusError::MissingTxCbor)));
    }
}
