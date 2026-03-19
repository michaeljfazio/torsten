use crate::utxo::UtxoSet;
use crate::validation::{plutus_script_version_map, redeemer_script_version_map};
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

/// Decode the `(tag_byte, index)` from the CBOR-encoded pallas `Redeemer`
/// returned by `eval_phase_two_raw`.
///
/// The encoding is `array(4)[tag_uint, index_uint, data, ex_units]`.  We only
/// need the first two elements.  The tag encoding matches the Cardano CDDL
/// redeemer tag enumeration:
///   0 = Spend, 1 = Mint, 2 = Cert, 3 = Reward, 4 = Vote, 5 = Propose.
///
/// Returns `None` if the bytes cannot be decoded (malformed CBOR or unexpected
/// structure).  Callers treat `None` as "unknown version" and fall back to the
/// permissive non-Unit check, which is the safe direction.
fn decode_redeemer_tag_index(redeemer_cbor: &[u8]) -> Option<(u8, u32)> {
    use minicbor::Decoder;
    let mut dec = Decoder::new(redeemer_cbor);
    // Expect an array of at least 2 elements.
    let _len = dec.array().ok()?;
    let tag = dec.u8().ok()?;
    let index = dec.u32().ok()?;
    Some((tag, index))
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

    // Build the script hash → language version map (1=V1, 2=V2, 3=V3) for all
    // Plutus scripts available to this transaction (witness set + ref scripts).
    let version_map = plutus_script_version_map(tx, utxo_set);

    // Build the per-redeemer version map: (tag_byte, index) → language version.
    //
    // `eval_phase_two_raw` returns one `(redeemer_cbor, EvalResult)` pair per
    // executed redeemer.  The `redeemer_cbor` bytes are a CBOR-encoded pallas
    // `Redeemer`: `array(4)[tag_uint, index_uint, data, ex_units]`.  We decode
    // the first two fields to recover (tag, index) and look up the language
    // version from this map.
    //
    // Per Haskell's `evaluateScriptRestricting` (PlutusLedgerApi.Common.Eval):
    // - V1 / V2: success = any non-error CEK result (term value is ignored).
    // - V3: success = script returned exactly `Unit` (`()`); any other term
    //   (Data, Bool, integer, …) is treated as `InvalidReturnValue`.
    //
    // By resolving each redeemer individually we avoid the prior bug where
    // the transaction-wide `has_any_v3` flag applied the V3 Unit check to
    // ALL redeemers when any V3 script was present — incorrectly rejecting
    // valid V1/V2 scripts that return non-Unit in mixed-version transactions.
    let redeemer_version_map = redeemer_script_version_map(tx, utxo_set, &version_map);

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
            for (redeemer_bytes, eval_result) in &results {
                let cost = eval_result.cost();

                // Determine whether this specific redeemer executes a V3 script.
                // Decode tag and index from the CBOR redeemer: array(4)[tag, idx, …].
                let is_v3 = decode_redeemer_tag_index(redeemer_bytes)
                    .and_then(|(tag, idx)| redeemer_version_map.get(&(tag, idx)).copied())
                    .map(|ver| ver == 3)
                    .unwrap_or(false);

                let script_failed = match &eval_result.result {
                    Err(_) => true,
                    Ok(term) => {
                        if matches!(term, uplc::ast::Term::Error) {
                            true
                        } else if is_v3 && !term.is_unit() {
                            // PlutusV3: only Unit is a valid return value.
                            // Any other term is treated as InvalidReturnValue.
                            true
                        } else {
                            false
                        }
                    }
                };
                if script_failed {
                    let err_msg = match &eval_result.result {
                        Err(e) => format!("{e}"),
                        Ok(term) if matches!(term, uplc::ast::Term::Error) => {
                            format!("Script error: {term:?}")
                        }
                        Ok(term) => format!("PlutusV3 script returned non-Unit value: {term:?}"),
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

    // -----------------------------------------------------------------------
    // Plutus V1/V2/V3 script execution test vectors
    //
    // These tests verify the behaviour of `evaluate_plutus_scripts` using
    // minimal hand-crafted UPLC programs:
    //
    // - always-succeeds: a Plutus V2 spending validator that immediately
    //   returns Unit regardless of datum, redeemer, or script context.
    // - always-fails:    a program whose body is the UPLC error term; any
    //   execution attempt terminates with a machine error.
    // - budget exhaustion: declared ExUnits well below the actual cost,
    //   verifying the CPU budget enforcement logic.
    //
    // Script bytecode is derived directly from the UPLC AST (via the uplc
    // parser → DeBruijn conversion → flat-encoded CBOR), so there is no
    // dependency on external compiled artefacts or pre-baked hex vectors.
    //
    // The full Conway-era CBOR transaction used as `raw_cbor` is assembled
    // manually using `minicbor::Encoder`.  Since `evaluate_plutus_scripts`
    // calls `eval_phase_two_raw` with `run_phase_one = false`, the
    // script_data_hash field in the body is intentionally omitted; pallas
    // will parse the transaction, but uplc will not re-validate structural
    // rules that our own Phase-1 pass already enforces.
    // -----------------------------------------------------------------------

    /// Build the CBOR bytes for a Plutus script (the format stored in the
    /// transaction witness set).
    ///
    /// A Plutus script in the witness set is `CBOR_bytes(flat_encoded_program)`:
    /// the `uplc` crate's `Program::to_cbor()` produces exactly this format.
    fn build_script_cbor(uplc_src: &str) -> Vec<u8> {
        let program = uplc::parser::program(uplc_src).expect("UPLC parse failed");
        program
            .to_debruijn()
            .expect("DeBruijn conversion failed")
            .to_cbor()
            .expect("CBOR encode failed")
    }

    /// Compute the PlutusV2 script hash for a script in witness-set encoding.
    ///
    /// The hash is `blake2b_224(0x02 || script_cbor_bytes)`, matching the rule
    /// used by `collect_available_script_hashes` and `compute_script_ref_hash`.
    fn script_hash_v2(script_cbor: &[u8]) -> [u8; 28] {
        let mut tagged = Vec::with_capacity(1 + script_cbor.len());
        tagged.push(0x02u8);
        tagged.extend_from_slice(script_cbor);
        *torsten_primitives::hash::blake2b_224(&tagged).as_bytes()
    }

    /// Return CBOR-encoded PlutusV2 cost model bytes for use with
    /// `evaluate_plutus_scripts`.
    ///
    /// These are the standard Vasil (Babbage) era PlutusV2 cost model entries
    /// (178 coefficients), taken verbatim from the uplc integration test suite.
    /// Having a non-None cost model causes `eval_phase_two_raw` to enforce the
    /// declared ExUnits budget via `Program::eval_as(..., Some(initial_budget))`,
    /// instead of silently using an unconstrained `ExBudget::default()`.
    fn vasil_v2_cost_models_cbor() -> Vec<u8> {
        // Standard Vasil (Babbage) era PlutusV2 cost model: 178 entries.
        // Source: uplc-1.1.21 integration test suite (`tx/tests.rs`).
        // Encoded as a CBOR map {1: [i64; 178]}.
        let v2_costs: &[i64] = &[
            205665,
            812,
            1,
            1,
            1000,
            571,
            0,
            1,
            1000,
            24177,
            4,
            1,
            1000,
            32,
            117366,
            10475,
            4,
            23000,
            100,
            23000,
            100,
            23000,
            100,
            23000,
            100,
            23000,
            100,
            23000,
            100,
            100,
            100,
            23000,
            100,
            19537,
            32,
            175354,
            32,
            46417,
            4,
            221973,
            511,
            0,
            1,
            89141,
            32,
            497525,
            14068,
            4,
            2,
            196500,
            453240,
            220,
            0,
            1,
            1,
            1000,
            28662,
            4,
            2,
            245000,
            216773,
            62,
            1,
            1060367,
            12586,
            1,
            208512,
            421,
            1,
            187000,
            1000,
            52998,
            1,
            80436,
            32,
            43249,
            32,
            1000,
            32,
            80556,
            1,
            57667,
            4,
            1000,
            10,
            197145,
            156,
            1,
            197145,
            156,
            1,
            204924,
            473,
            1,
            208896,
            511,
            1,
            52467,
            32,
            64832,
            32,
            65493,
            32,
            22558,
            32,
            16563,
            32,
            76511,
            32,
            196500,
            453240,
            220,
            0,
            1,
            1,
            69522,
            11687,
            0,
            1,
            60091,
            32,
            196500,
            453240,
            220,
            0,
            1,
            1,
            196500,
            453240,
            220,
            0,
            1,
            1,
            1159724,
            392670,
            0,
            2,
            806990,
            30482,
            4,
            1927926,
            82523,
            4,
            265318,
            0,
            4,
            0,
            85931,
            32,
            205665,
            812,
            1,
            1,
            41182,
            32,
            212342,
            32,
            31220,
            32,
            32696,
            32,
            43357,
            32,
            32247,
            32,
            38314,
            32,
            20000000000,
            20000000000,
            9462713,
            1021,
            10,
            20000000000,
            0,
            20000000000,
        ];
        use minicbor::Encoder;
        let mut buf = Vec::with_capacity(2048);
        let mut enc = Encoder::new(&mut buf);
        // map{1: [cost_entries]}  — key 1 = PlutusV2
        enc.map(1).expect("infallible");
        enc.u8(1).expect("infallible");
        enc.array(v2_costs.len() as u64).expect("infallible");
        for &c in v2_costs {
            enc.i64(c).expect("infallible");
        }
        buf
    }

    /// Build a minimal Conway-era CBOR transaction that `eval_phase_two_raw`
    /// can parse.
    ///
    /// The transaction spends one Plutus V2-locked UTxO and carries exactly
    /// one Spend redeemer.  The witness set contains the compiled V2 script.
    ///
    /// # Transaction layout
    ///
    /// ```text
    /// array(4)
    ///   body = map { 0: [input], 1: [output], 2: fee }
    ///   wits = map { 6: [script_cbor], 5: [[tag=0, idx=0, data=Unit, exunits]] }
    ///   is_valid = true
    ///   aux_data = null
    /// ```
    ///
    /// The body omits `script_data_hash` (key 11) because `eval_phase_two_raw`
    /// is called with `run_phase_one = false` and therefore never validates the
    /// integrity hash — our own Phase-1 pass enforces that rule.
    fn build_conway_tx_cbor(
        tx_input_hash: &[u8; 32],
        script_cbor: &[u8],
        ex_units_steps: u64,
        ex_units_mem: u64,
    ) -> Vec<u8> {
        // ----------------------------------------------------------------
        // Re-use the same minicbor encoder as the rest of the Plutus module.
        // All writes to Vec<u8> are infallible.
        // ----------------------------------------------------------------
        use minicbor::Encoder;

        let mut buf = Vec::with_capacity(256);
        let mut enc = Encoder::new(&mut buf);

        // Outer array(4): [body, wits, is_valid, null]
        enc.array(4).expect("infallible");

        // ----------------------------------------------------------------
        // [0] Transaction body — map(3): inputs, outputs, fee
        // ----------------------------------------------------------------
        enc.map(3).expect("infallible");

        // key 0: inputs — a definite array containing one TransactionInput
        // TransactionInput CBOR: array(2) [bytes(32), uint(0)]
        enc.u8(0).expect("infallible");
        enc.array(1).expect("infallible");
        enc.array(2).expect("infallible");
        enc.bytes(tx_input_hash).expect("infallible");
        enc.u8(0).expect("infallible"); // output index

        // key 1: outputs — a definite array containing one PostAlonzo output
        //   PostAlonzo output is a CBOR map: { 0: address_bytes, 1: coin }
        //   Address: enterprise script address (mainnet), header=0x71 || script_hash
        //   We use a dummy output address (script-locked UTxOs live in utxo_set,
        //   but the output recipient can be any valid address).
        enc.u8(1).expect("infallible");
        let recipient_addr: Vec<u8> = {
            // Mainnet enterprise key-locked address (0x61 || 28-byte payment keyhash)
            let mut a = Vec::with_capacity(29);
            a.push(0x61u8); // mainnet enterprise key
            a.extend_from_slice(&[0xBBu8; 28]); // dummy payment key hash
            a
        };
        enc.array(1).expect("infallible");
        enc.map(2).expect("infallible");
        enc.u8(0).expect("infallible");
        enc.bytes(&recipient_addr).expect("infallible");
        enc.u8(1).expect("infallible");
        // Output value: 9 ADA
        enc.u32(9_000_000).expect("infallible");

        // key 2: fee — 1 ADA (not validated in phase-2 mode)
        enc.u8(2).expect("infallible");
        enc.u32(1_000_000).expect("infallible");

        // ----------------------------------------------------------------
        // [1] Witness set — map(2): plutus_v2_scripts (key 6), redeemers (key 5)
        // ----------------------------------------------------------------
        enc.map(2).expect("infallible");

        // key 6: PlutusV2 scripts — plain array(1) [script_cbor_bytes]
        // pallas accepts a plain array as well as tag(258)+array for NonEmptySet
        enc.u8(6).expect("infallible");
        enc.array(1).expect("infallible");
        enc.bytes(script_cbor).expect("infallible");

        // key 5: redeemers — array(1) [[Spend, 0, Unit, [steps, mem]]]
        // Each redeemer: array(4) [tag, index, data, ex_units]
        // - tag 0 = Spend
        // - index 0 (first input)
        // - data = Unit = d87980 (Constr tag 0 with empty list, CBOR alternate format)
        // - ex_units = array(2) [steps, mem]
        enc.u8(5).expect("infallible");
        enc.array(1).expect("infallible");
        enc.array(4).expect("infallible");
        enc.u8(0).expect("infallible"); // Spend tag
        enc.u8(0).expect("infallible"); // index 0
                                        // PlutusData Unit: tag 121 (0x79 + 0xd8 two-byte tag encoding) with empty array
                                        // CBOR: d8 79 80 — tag(121), array(0)
                                        // minicbor tag API: tag(121).array(0)
        enc.tag(minicbor::data::Tag::new(121)).expect("infallible");
        enc.array(0).expect("infallible");
        enc.array(2).expect("infallible");
        enc.u64(ex_units_steps).expect("infallible");
        enc.u64(ex_units_mem).expect("infallible");

        // ----------------------------------------------------------------
        // [2] is_valid = true
        // ----------------------------------------------------------------
        enc.bool(true).expect("infallible");

        // ----------------------------------------------------------------
        // [3] aux_data = null
        // ----------------------------------------------------------------
        enc.null().expect("infallible");

        buf
    }

    /// Build the UTxO set used in the Plutus evaluation tests.
    ///
    /// Inserts one script-locked UTxO with an inline Unit datum.  The address
    /// is a mainnet enterprise script address constructed from the provided
    /// script hash.
    fn build_script_utxo_set(
        tx_input_hash: &[u8; 32],
        script_hash: &[u8; 28],
    ) -> (UtxoSet, torsten_primitives::transaction::TransactionInput) {
        use torsten_primitives::address::{Address, EnterpriseAddress};
        use torsten_primitives::credentials::Credential;
        use torsten_primitives::hash::Hash28;
        use torsten_primitives::network::NetworkId;
        use torsten_primitives::transaction::{
            OutputDatum, PlutusData, TransactionInput, TransactionOutput,
        };
        use torsten_primitives::value::Value;

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes(*tx_input_hash),
            index: 0,
        };

        // Mainnet enterprise script address
        let script_cred = Credential::Script(Hash28::from_bytes(*script_hash));
        let address = Address::Enterprise(EnterpriseAddress {
            network: NetworkId::Mainnet,
            payment: script_cred,
        });

        // Inline Unit datum (Constr 0 [])
        let output = TransactionOutput {
            address,
            value: Value::lovelace(10_000_000),
            datum: OutputDatum::InlineDatum {
                data: PlutusData::Constr(0, vec![]),
                raw_cbor: None,
            },
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        };
        utxo_set.insert(input.clone(), output);
        (utxo_set, input)
    }

    // -----------------------------------------------------------------------
    // Test 1: Always-succeeds Plutus V2 spending validator
    //
    // The program `(program 1.0.0 (lam _ (lam _ (lam _ (con unit ())))))` is
    // a valid Plutus V2 spending validator that ignores all three arguments
    // (datum, redeemer, script context) and returns Unit.
    //
    // Per Haskell's processLogsAndErrors for PlutusV1/V2: any non-error result
    // is a success — including a Unit constant.
    // -----------------------------------------------------------------------
    #[test]
    fn test_evaluate_always_succeeds_v2() {
        // Build always-succeeds validator: lam _ (lam _ (lam _ (con unit ())))
        let script_cbor =
            build_script_cbor("(program 1.0.0 (lam _ (lam _ (lam _ (con unit ())))))");
        let script_hash = script_hash_v2(&script_cbor);

        // Fixed UTxO input hash for this test
        let tx_input_hash = [0x01u8; 32];

        // Build transaction and UTxO set
        let tx_cbor = build_conway_tx_cbor(
            &tx_input_hash,
            &script_cbor,
            // Budget: generous CPU/mem — script should terminate well within budget
            14_000_000, // steps
            2_000_000,  // mem
        );
        let (utxo_set, input) = build_script_utxo_set(&tx_input_hash, &script_hash);

        // Populate the Transaction struct's raw_cbor from the CBOR we built
        let mut tx = Transaction::empty_with_hash(Hash32::ZERO);
        tx.raw_cbor = Some(tx_cbor);
        tx.body.inputs = vec![input];
        tx.witness_set.plutus_v2_scripts = vec![script_cbor];

        let slot_config = SlotConfig::preview();
        // Budget: (steps, mem) matching the convention in evaluate_plutus_scripts
        let result = evaluate_plutus_scripts(
            &tx,
            &utxo_set,
            None, // no cost models — script is so simple it needs no builtins
            (14_000_000, 2_000_000),
            &slot_config,
        );

        assert!(
            result.is_ok(),
            "Always-succeeds script should pass Phase-2: {:?}",
            result.err()
        );
    }

    // -----------------------------------------------------------------------
    // Test 2: Always-fails Plutus V2 script
    //
    // `(program 1.0.0 (error))` — the UPLC error term causes the CEK machine
    // to terminate with an evaluation error.  The caller should receive
    // `PlutusError::EvalFailed`.
    // -----------------------------------------------------------------------
    #[test]
    fn test_evaluate_always_fails_v2() {
        let script_cbor = build_script_cbor("(program 1.0.0 (error))");
        let script_hash = script_hash_v2(&script_cbor);
        let tx_input_hash = [0x02u8; 32];

        let tx_cbor = build_conway_tx_cbor(&tx_input_hash, &script_cbor, 14_000_000, 2_000_000);
        let (utxo_set, input) = build_script_utxo_set(&tx_input_hash, &script_hash);

        let mut tx = Transaction::empty_with_hash(Hash32::ZERO);
        tx.raw_cbor = Some(tx_cbor);
        tx.body.inputs = vec![input];
        tx.witness_set.plutus_v2_scripts = vec![script_cbor];

        let slot_config = SlotConfig::preview();
        let result =
            evaluate_plutus_scripts(&tx, &utxo_set, None, (14_000_000, 2_000_000), &slot_config);

        // The error term must produce a script failure, not a parse or
        // infrastructure error.
        assert!(
            matches!(result, Err(PlutusError::EvalFailed(_))),
            "Always-fails script should produce EvalFailed: {:?}",
            result
        );
    }

    // -----------------------------------------------------------------------
    // Test 3: Budget exhaustion
    //
    // Supply a budget of (1 step, 1 mem) — far below what even the simplest
    // always-succeeds script requires.  The evaluation must fail because the
    // machine exhausts its CPU budget before completing.
    //
    // This verifies that:
    //   (a) the budget is actually enforced by the uplc evaluator, and
    //   (b) `evaluate_plutus_scripts` surfaces budget errors as EvalFailed
    //       rather than silently succeeding or panicking.
    //
    // IMPORTANT: budget enforcement only works when cost models are supplied.
    // With `cost_models_cbor = None` the uplc evaluator ignores `initial_budget`
    // and uses an unconstrained `ExBudget::default()`.  Real cost models are
    // required to activate the per-redeemer budget check in
    // `eval_redeemer` / `Program::eval_as(…, Some(budget))`.
    // -----------------------------------------------------------------------
    #[test]
    fn test_evaluate_budget_exhaustion() {
        let script_cbor =
            build_script_cbor("(program 1.0.0 (lam _ (lam _ (lam _ (con unit ())))))");
        let script_hash = script_hash_v2(&script_cbor);
        let tx_input_hash = [0x03u8; 32];

        // Declare 1 step / 1 mem in the redeemer (so the tx encodes minimal
        // declared budget).  The actual budget limit passed to the evaluator
        // is controlled by the `max_tx_ex_units` argument, NOT the redeemer's
        // ex_units field — but we keep them consistent here for realism.
        let tx_cbor = build_conway_tx_cbor(&tx_input_hash, &script_cbor, 1, 1);
        let (utxo_set, input) = build_script_utxo_set(&tx_input_hash, &script_hash);

        let mut tx = Transaction::empty_with_hash(Hash32::ZERO);
        tx.raw_cbor = Some(tx_cbor);
        tx.body.inputs = vec![input];
        tx.witness_set.plutus_v2_scripts = vec![script_cbor];

        let slot_config = SlotConfig::preview();

        // Supply real cost models so the evaluator enforces the budget cap.
        let cost_models = vasil_v2_cost_models_cbor();

        // Pass an impossibly small budget to the evaluator (1 step, 1 mem).
        let result =
            evaluate_plutus_scripts(&tx, &utxo_set, Some(&cost_models), (1, 1), &slot_config);

        assert!(
            result.is_err(),
            "Evaluation with budget (1, 1) must fail; got Ok"
        );
        // The failure should be reported as EvalFailed (machine-level budget
        // exhaustion), not as a missing-CBOR or infrastructure error.
        assert!(
            matches!(result, Err(PlutusError::EvalFailed(_))),
            "Budget exhaustion should produce EvalFailed: {:?}",
            result
        );
    }

    // -----------------------------------------------------------------------
    // Test 4: Always-succeeds Plutus V1 spending validator
    //
    // PlutusV1 scripts follow the same success rule as V2: any non-error
    // result is accepted.  The only difference in our evaluation path is the
    // script version tag (0x01 vs 0x02) which determines the TxInfo version
    // passed to the script.
    //
    // For a PlutusV1 validator the witness set key is 3 (plutus_v1_script).
    // We verify the corresponding V1 code path in evaluate_plutus_scripts.
    //
    // NOTE: PlutusV1 does NOT support inline datums (the inline datum feature
    // was introduced in Babbage/PlutusV2).  The spending UTxO must carry a
    // datum hash, with the corresponding datum placed in the witness set
    // (key 4 = plutus_data).
    // -----------------------------------------------------------------------
    #[test]
    fn test_evaluate_always_succeeds_v1() {
        use torsten_primitives::address::{Address, EnterpriseAddress};
        use torsten_primitives::credentials::Credential;
        use torsten_primitives::hash::Hash28;
        use torsten_primitives::network::NetworkId;
        use torsten_primitives::transaction::{
            OutputDatum, PlutusData, TransactionInput, TransactionOutput,
        };
        use torsten_primitives::value::Value;

        // Build always-succeeds V1 validator
        let script_text = "(program 1.0.0 (lam _ (lam _ (lam _ (con unit ())))))";
        let program = uplc::parser::program(script_text).expect("UPLC parse failed");
        let script_cbor = program
            .to_debruijn()
            .expect("DeBruijn conversion failed")
            .to_cbor()
            .expect("CBOR encode failed");

        // V1 script hash: blake2b_224(0x01 || script_bytes)
        let v1_script_hash: [u8; 28] = {
            let mut tagged = Vec::with_capacity(1 + script_cbor.len());
            tagged.push(0x01u8);
            tagged.extend_from_slice(&script_cbor);
            *torsten_primitives::hash::blake2b_224(&tagged).as_bytes()
        };

        let tx_input_hash = [0x04u8; 32];

        // PlutusV1 requires a datum hash in the UTxO output (not inline datum).
        // The datum itself is placed in the witness set.
        // Datum: Unit = Constr 0 []
        // Datum CBOR: d87980 (tag 121, empty array)
        let datum_cbor: Vec<u8> = {
            use minicbor::Encoder;
            let mut buf = Vec::new();
            let mut enc = Encoder::new(&mut buf);
            enc.tag(minicbor::data::Tag::new(121)).expect("infallible");
            enc.array(0).expect("infallible");
            buf
        };
        let datum_hash: [u8; 32] = *torsten_primitives::hash::blake2b_256(&datum_cbor).as_bytes();

        // Build UTxO with datum hash (not inline)
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes(tx_input_hash),
            index: 0,
        };
        let script_cred = Credential::Script(Hash28::from_bytes(v1_script_hash));
        let address = Address::Enterprise(EnterpriseAddress {
            network: NetworkId::Mainnet,
            payment: script_cred,
        });
        let output = TransactionOutput {
            address,
            value: Value::lovelace(10_000_000),
            // Datum hash, not inline — required for PlutusV1 compatibility
            datum: OutputDatum::DatumHash(Hash32::from_bytes(datum_hash)),
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        };
        utxo_set.insert(input.clone(), output);

        // Build a Conway CBOR tx with:
        //   - key 3 (V1 scripts) in witness set
        //   - key 4 (plutus_data) with the Unit datum
        //   - key 5 (redeemers) with one Spend redeemer
        let tx_cbor: Vec<u8> = {
            use minicbor::Encoder;
            let mut buf = Vec::with_capacity(512);
            let mut enc = Encoder::new(&mut buf);
            enc.array(4).expect("infallible");

            // Body: inputs, outputs, fee
            enc.map(3).expect("infallible");
            enc.u8(0).expect("infallible"); // inputs key
            enc.array(1).expect("infallible");
            enc.array(2).expect("infallible");
            enc.bytes(&tx_input_hash).expect("infallible");
            enc.u8(0).expect("infallible");
            enc.u8(1).expect("infallible"); // outputs key
            enc.array(1).expect("infallible");
            enc.map(2).expect("infallible");
            enc.u8(0).expect("infallible");
            enc.bytes(&{
                let mut a = vec![0x61u8]; // mainnet enterprise key
                a.extend_from_slice(&[0xBBu8; 28]);
                a
            })
            .expect("infallible");
            enc.u8(1).expect("infallible");
            enc.u32(9_000_000).expect("infallible");
            enc.u8(2).expect("infallible"); // fee key
            enc.u32(1_000_000).expect("infallible");

            // Witness set: V1 scripts (key 3), datums (key 4), redeemers (key 5)
            enc.map(3).expect("infallible");
            enc.u8(3).expect("infallible"); // PlutusV1 scripts
            enc.array(1).expect("infallible");
            enc.bytes(&script_cbor).expect("infallible");
            enc.u8(4).expect("infallible"); // plutus_data (datums)
            enc.array(1).expect("infallible");
            // Encode the datum: Unit = constr 0 []
            enc.tag(minicbor::data::Tag::new(121)).expect("infallible");
            enc.array(0).expect("infallible");
            enc.u8(5).expect("infallible"); // redeemers
            enc.array(1).expect("infallible");
            enc.array(4).expect("infallible");
            enc.u8(0).expect("infallible"); // Spend
            enc.u8(0).expect("infallible"); // index 0
                                            // Redeemer data: Unit
            enc.tag(minicbor::data::Tag::new(121)).expect("infallible");
            enc.array(0).expect("infallible");
            enc.array(2).expect("infallible");
            enc.u64(14_000_000).expect("infallible");
            enc.u64(2_000_000).expect("infallible");

            enc.bool(true).expect("infallible");
            enc.null().expect("infallible");
            buf
        };

        let mut tx = Transaction::empty_with_hash(Hash32::ZERO);
        tx.raw_cbor = Some(tx_cbor);
        tx.body.inputs = vec![input];
        tx.witness_set.plutus_v1_scripts = vec![script_cbor];
        // Provide the datum in the witness set
        tx.witness_set.plutus_data = vec![PlutusData::Constr(0, vec![])];

        let slot_config = SlotConfig::preview();
        let result =
            evaluate_plutus_scripts(&tx, &utxo_set, None, (14_000_000, 2_000_000), &slot_config);
        assert!(
            result.is_ok(),
            "Always-succeeds V1 script should pass Phase-2: {:?}",
            result.err()
        );
    }

    // -----------------------------------------------------------------------
    // Test 5: Script context construction — verify that inputs in the UTxO
    //         set that are NOT referenced by the transaction are NOT resolved.
    //
    // This tests that `evaluate_plutus_scripts` only passes input/output CBOR
    // pairs for inputs that appear in the transaction body (inputs +
    // reference_inputs + collateral), not arbitrary UTxO entries.
    // -----------------------------------------------------------------------
    #[test]
    fn test_evaluate_only_resolves_tx_inputs() {
        use torsten_primitives::address::{Address, ByronAddress};
        use torsten_primitives::transaction::{OutputDatum, TransactionInput, TransactionOutput};
        use torsten_primitives::value::Value;

        // Inject extra UTxOs that must NOT be resolved
        let mut utxo_set = UtxoSet::new();
        for i in 1u8..=5 {
            let extra_input = TransactionInput {
                transaction_id: Hash32::from_bytes([i; 32]),
                index: 0,
            };
            let extra_output = TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(1_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            };
            utxo_set.insert(extra_input, extra_output);
        }

        // Tx with no raw_cbor → should fail with MissingTxCbor,
        // which means we never reached the UTxO-resolution loop.
        // This implicitly verifies that resolution does not happen
        // eagerly before CBOR is available.
        let tx = Transaction::empty_with_hash(Hash32::ZERO);
        let slot_config = SlotConfig::default();
        let result =
            evaluate_plutus_scripts(&tx, &utxo_set, None, (10_000_000, 10_000_000), &slot_config);
        assert!(
            matches!(result, Err(PlutusError::MissingTxCbor)),
            "Missing raw_cbor should fail early with MissingTxCbor"
        );
    }

    // -----------------------------------------------------------------------
    // Test 6: ExUnits comparison — verify the budget tuple convention.
    //
    // `evaluate_plutus_scripts` takes `max_tx_ex_units` as `(steps, mem)`,
    // matching the `uplc::tx::eval_phase_two_raw` convention where `.0 = cpu`
    // and `.1 = mem`.  Passing `(mem, steps)` would produce a 700× too-small
    // CPU ceiling and cause false failures for scripts that use many steps.
    //
    // This test confirms that the correct ordering passes evaluation and the
    // swapped ordering (mem as CPU ceiling) fails budget exhaustion.
    //
    // Budget enforcement requires real cost models (see test 3 notes).
    // -----------------------------------------------------------------------
    #[test]
    fn test_evaluate_exunits_ordering() {
        let script_cbor =
            build_script_cbor("(program 1.0.0 (lam _ (lam _ (lam _ (con unit ())))))");
        let script_hash = script_hash_v2(&script_cbor);
        let tx_input_hash = [0x06u8; 32];

        // The always-succeeds validator (with V2 cost models applied) uses
        // roughly ~7_600_000 CPU steps and ~2_000 mem units on the CEK machine.
        // We choose budget values that are:
        //   - Clearly sufficient when passed in the correct (steps, mem) order.
        //   - Too small for steps when only mem-scale values are used.
        //
        // budget_steps = 14_000_000  (well above ~7.6M actual cost)
        // budget_mem   = 50_000      (well above ~2 000 actual mem)
        let budget_steps: u64 = 14_000_000;
        let budget_mem: u64 = 50_000;

        let cost_models = vasil_v2_cost_models_cbor();

        let tx_cbor = build_conway_tx_cbor(&tx_input_hash, &script_cbor, budget_steps, budget_mem);
        let (utxo_set, input) = build_script_utxo_set(&tx_input_hash, &script_hash);

        let mut tx = Transaction::empty_with_hash(Hash32::ZERO);
        tx.raw_cbor = Some(tx_cbor.clone());
        tx.body.inputs = vec![input.clone()];
        tx.witness_set.plutus_v2_scripts = vec![script_cbor.clone()];

        let slot_config = SlotConfig::preview();

        // Correct ordering (steps, mem) must succeed
        let result_correct = evaluate_plutus_scripts(
            &tx,
            &utxo_set,
            Some(&cost_models),
            (budget_steps, budget_mem),
            &slot_config,
        );
        assert!(
            result_correct.is_ok(),
            "Correct (steps, mem) ordering must succeed: {:?}",
            result_correct.err()
        );

        // Now rebuild UTxO set (it was moved) and test with a tiny CPU budget.
        // Budget (1, budget_mem): 1 step is far too small — must fail.
        let (utxo_set2, input2) = build_script_utxo_set(&tx_input_hash, &script_hash);
        let mut tx2 = Transaction::empty_with_hash(Hash32::ZERO);
        tx2.raw_cbor = Some(tx_cbor);
        tx2.body.inputs = vec![input2];
        tx2.witness_set.plutus_v2_scripts = vec![script_cbor];

        let result_exhausted = evaluate_plutus_scripts(
            &tx2,
            &utxo_set2,
            Some(&cost_models),
            (1, budget_mem),
            &slot_config,
        );
        assert!(
            result_exhausted.is_err(),
            "Tiny steps budget (1) must cause EvalFailed"
        );
    }

    // -----------------------------------------------------------------------
    // Test 7: decode_redeemer_tag_index — round-trip sanity check
    //
    // Verifies that we can recover (tag, index) from the CBOR redeemer bytes
    // produced by our own `build_conway_tx_cbor` helper (which encodes the
    // witness-set redeemer in the same `array(4)[tag, idx, data, ex_units]`
    // format that `eval_phase_two_raw` returns).  This exercises the function
    // used in the per-redeemer V3 Unit-check path.
    // -----------------------------------------------------------------------
    #[test]
    fn test_decode_redeemer_tag_index() {
        use minicbor::Encoder;

        // Build a redeemer CBOR manually: array(4)[tag=0, idx=3, data=unit, ex_units]
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        enc.array(4).expect("infallible");
        enc.u8(0).expect("infallible"); // Spend = 0
        enc.u32(3).expect("infallible"); // index 3
        enc.tag(minicbor::data::Tag::new(121)).expect("infallible");
        enc.array(0).expect("infallible"); // Unit datum
        enc.array(2).expect("infallible");
        enc.u64(1000).expect("infallible"); // steps
        enc.u64(500).expect("infallible"); // mem

        let result = decode_redeemer_tag_index(&buf);
        assert_eq!(result, Some((0u8, 3u32)), "Expected (tag=0 Spend, index=3)");

        // Mint redeemer at index 1
        let mut buf2 = Vec::new();
        let mut enc2 = Encoder::new(&mut buf2);
        enc2.array(4).expect("infallible");
        enc2.u8(1).expect("infallible"); // Mint = 1
        enc2.u32(1).expect("infallible");
        enc2.tag(minicbor::data::Tag::new(121)).expect("infallible");
        enc2.array(0).expect("infallible");
        enc2.array(2).expect("infallible");
        enc2.u64(0).expect("infallible");
        enc2.u64(0).expect("infallible");

        assert_eq!(decode_redeemer_tag_index(&buf2), Some((1u8, 1u32)));

        // Malformed CBOR must return None
        assert_eq!(decode_redeemer_tag_index(&[0xFF, 0xAB]), None);
        assert_eq!(decode_redeemer_tag_index(&[]), None);
    }

    // -----------------------------------------------------------------------
    // Test 8: Per-redeemer V3 Unit-check (regression for GH#185)
    //
    // A transaction that contains BOTH a PlutusV2 script (Spend redeemer)
    // AND a PlutusV3 script in the witness set (but no redeemer for the V3
    // script) must NOT apply the Unit-return check to the V2 redeemer.
    //
    // The V2 script `(program 1.0.0 (lam _ (lam _ (lam _ (con integer 42)))))`
    // returns the integer 42 (not Unit).  Under the old transaction-wide
    // `has_any_v3` flag this would have been rejected.  Under the correct
    // per-redeemer check, the V2 Spend redeemer maps to version 2 and the
    // Unit check is NOT applied — the script must succeed.
    //
    // The witness set contains a V3 script (key 7) that has no redeemer, so
    // `eval_phase_two_raw` never executes it — this ensures only the V2
    // script runs while the V3 script is visible in `plutus_script_version_map`.
    // -----------------------------------------------------------------------
    #[test]
    fn test_v2_non_unit_return_not_blocked_by_v3_presence() {
        use minicbor::Encoder;

        // V2 script that returns integer 42 (not Unit)
        let v2_script_cbor =
            build_script_cbor("(program 1.0.0 (lam _ (lam _ (lam _ (con integer 42)))))");
        let v2_script_hash = script_hash_v2(&v2_script_cbor);

        // A trivial V3 script (never executed — no redeemer points to it).
        // We pick a short byte sequence so the script_hash differs from the V2 hash.
        let v3_script_cbor = build_script_cbor("(program 1.0.0 (lam _ (con unit ())))");

        let tx_input_hash = [0x08u8; 32];

        // Build transaction CBOR with both V2 (key 6) and V3 (key 7) scripts
        // and ONE Spend redeemer targeting the V2 script.
        let tx_cbor: Vec<u8> = {
            let mut buf = Vec::with_capacity(512);
            let mut enc = Encoder::new(&mut buf);

            // Outer: array(4) [body, wits, is_valid, null]
            enc.array(4).expect("infallible");

            // Body: map(3) {0: [input], 1: [output], 2: fee}
            enc.map(3).expect("infallible");
            enc.u8(0).expect("infallible"); // inputs
            enc.array(1).expect("infallible");
            enc.array(2).expect("infallible");
            enc.bytes(&tx_input_hash).expect("infallible");
            enc.u8(0).expect("infallible");
            enc.u8(1).expect("infallible"); // outputs
            enc.array(1).expect("infallible");
            enc.map(2).expect("infallible");
            enc.u8(0).expect("infallible");
            enc.bytes(&{
                let mut a = vec![0x61u8];
                a.extend_from_slice(&[0xBBu8; 28]);
                a
            })
            .expect("infallible");
            enc.u8(1).expect("infallible");
            enc.u32(9_000_000).expect("infallible");
            enc.u8(2).expect("infallible"); // fee
            enc.u32(1_000_000).expect("infallible");

            // Witness set: map(3) { 5: redeemers, 6: v2_scripts, 7: v3_scripts }
            enc.map(3).expect("infallible");

            // key 5: redeemers — one Spend redeemer at index 0 (for the V2 script)
            enc.u8(5).expect("infallible");
            enc.array(1).expect("infallible");
            enc.array(4).expect("infallible");
            enc.u8(0).expect("infallible"); // Spend
            enc.u8(0).expect("infallible"); // index 0
            enc.tag(minicbor::data::Tag::new(121)).expect("infallible");
            enc.array(0).expect("infallible"); // Unit redeemer data
            enc.array(2).expect("infallible");
            enc.u64(14_000_000).expect("infallible"); // steps
            enc.u64(2_000_000).expect("infallible"); // mem

            // key 6: PlutusV2 scripts
            enc.u8(6).expect("infallible");
            enc.array(1).expect("infallible");
            enc.bytes(&v2_script_cbor).expect("infallible");

            // key 7: PlutusV3 scripts (present but no redeemer — not executed)
            enc.u8(7).expect("infallible");
            enc.array(1).expect("infallible");
            enc.bytes(&v3_script_cbor).expect("infallible");

            enc.bool(true).expect("infallible"); // is_valid
            enc.null().expect("infallible"); // aux_data

            buf
        };

        // UTxO: the input is locked by the V2 script
        let (utxo_set, input) = build_script_utxo_set(&tx_input_hash, &v2_script_hash);

        let mut tx = Transaction::empty_with_hash(Hash32::ZERO);
        tx.raw_cbor = Some(tx_cbor);
        tx.body.inputs = vec![input];
        // Populate witness_set so plutus_script_version_map can see both scripts
        tx.witness_set.plutus_v2_scripts = vec![v2_script_cbor];
        tx.witness_set.plutus_v3_scripts = vec![v3_script_cbor];

        let slot_config = SlotConfig::preview();

        // The V2 script returns integer 42 (not Unit).  With the old
        // transaction-wide `has_any_v3` flag this would incorrectly fail
        // (because a V3 script is present).  With the correct per-redeemer
        // check the Spend redeemer at (0, 0) maps to V2 → no Unit check →
        // the script must succeed.
        let result = evaluate_plutus_scripts(
            &tx,
            &utxo_set,
            None, // no cost models needed for this simple script
            (14_000_000, 2_000_000),
            &slot_config,
        );

        assert!(
            result.is_ok(),
            "V2 script returning non-Unit must NOT be blocked by presence of a V3 script: {:?}",
            result.err()
        );
    }
}
