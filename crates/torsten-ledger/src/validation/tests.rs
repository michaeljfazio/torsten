//! Tests for Phase-1 transaction validation.
//!
//! All tests live here so production modules stay focused on logic only.

#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use std::collections::{BTreeMap, HashSet};

    use torsten_primitives::address::{Address, ByronAddress};
    use torsten_primitives::hash::Hash32;
    use torsten_primitives::protocol_params::ProtocolParameters;
    use torsten_primitives::transaction::*;
    use torsten_primitives::value::Value;

    use crate::utxo::UtxoSet;

    // Public entry points
    use super::super::{validate_transaction, validate_transaction_with_pools, ValidationError};

    // Internal helpers exposed for testing
    use super::super::conway::{calculate_deposits_and_refunds, conway_only_certificate_name};
    use super::super::phase1::extract_reward_credential;
    use super::super::scripts::{
        calculate_ref_script_tiered_fee, cbor_uint_size, check_script_data_hash, compute_min_fee,
        compute_script_ref_hash, estimate_value_cbor_size, evaluate_native_script,
        script_ref_byte_size, MAX_REF_SCRIPT_SIZE_TIER_CAP,
    };

    use torsten_primitives::hash::Hash28;
    use torsten_primitives::time::SlotNo;
    use torsten_primitives::value::{AssetName, Lovelace};

    fn make_simple_utxo_set() -> (UtxoSet, TransactionInput) {
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        let output = TransactionOutput {
            address: Address::Byron(ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value::lovelace(10_000_000),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        };
        utxo_set.insert(input.clone(), output);
        (utxo_set, input)
    }

    fn make_simple_tx(input: TransactionInput, output_value: u64, fee: u64) -> Transaction {
        Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
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
        }
    }

    #[test]
    fn test_valid_transaction() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let tx = make_simple_tx(input, 9_800_000, 200_000);
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_no_inputs() {
        let (utxo_set, _) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(
            TransactionInput {
                transaction_id: Hash32::ZERO,
                index: 0,
            },
            0,
            0,
        );
        tx.body.inputs.clear();
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::NoInputs)));
    }

    #[test]
    fn test_input_not_found() {
        let (utxo_set, _) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let missing_input = TransactionInput {
            transaction_id: Hash32::from_bytes([99u8; 32]),
            index: 0,
        };
        let tx = make_simple_tx(missing_input, 9_800_000, 200_000);
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_value_not_conserved() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let tx = make_simple_tx(input, 10_000_000, 200_000);
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_fee_too_small() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let tx = make_simple_tx(input, 9_999_900, 100);
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_output_too_small() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let tx = make_simple_tx(input, 1000, 9_999_000);
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_ttl_expired() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.ttl = Some(SlotNo(50));
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_not_yet_valid() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.validity_interval_start = Some(SlotNo(200));
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_tx_too_large() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let tx = make_simple_tx(input, 9_800_000, 200_000);
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 20000, None);
        assert!(result.is_err());
    }

    // ---------------------------------------------------------------------------
    // Native script evaluation
    // ---------------------------------------------------------------------------

    #[test]
    fn test_native_script_pubkey_match() {
        let key = Hash32::from_bytes([1u8; 32]);
        let script = NativeScript::ScriptPubkey(key);
        let signers: HashSet<Hash32> = [key].into();
        assert!(evaluate_native_script(&script, &signers, SlotNo(100)));
    }

    #[test]
    fn test_native_script_pubkey_no_match() {
        let key = Hash32::from_bytes([1u8; 32]);
        let other_key = Hash32::from_bytes([2u8; 32]);
        let script = NativeScript::ScriptPubkey(key);
        let signers: HashSet<Hash32> = [other_key].into();
        assert!(!evaluate_native_script(&script, &signers, SlotNo(100)));
    }

    #[test]
    fn test_native_script_all() {
        let k1 = Hash32::from_bytes([1u8; 32]);
        let k2 = Hash32::from_bytes([2u8; 32]);
        let script = NativeScript::ScriptAll(vec![
            NativeScript::ScriptPubkey(k1),
            NativeScript::ScriptPubkey(k2),
        ]);
        let signers: HashSet<Hash32> = [k1, k2].into();
        assert!(evaluate_native_script(&script, &signers, SlotNo(100)));
        let partial: HashSet<Hash32> = [k1].into();
        assert!(!evaluate_native_script(&script, &partial, SlotNo(100)));
    }

    #[test]
    fn test_native_script_any() {
        let k1 = Hash32::from_bytes([1u8; 32]);
        let k2 = Hash32::from_bytes([2u8; 32]);
        let script = NativeScript::ScriptAny(vec![
            NativeScript::ScriptPubkey(k1),
            NativeScript::ScriptPubkey(k2),
        ]);
        let signers: HashSet<Hash32> = [k2].into();
        assert!(evaluate_native_script(&script, &signers, SlotNo(100)));
        let empty: HashSet<Hash32> = HashSet::new();
        assert!(!evaluate_native_script(&script, &empty, SlotNo(100)));
    }

    #[test]
    fn test_native_script_n_of_k() {
        let k1 = Hash32::from_bytes([1u8; 32]);
        let k2 = Hash32::from_bytes([2u8; 32]);
        let k3 = Hash32::from_bytes([3u8; 32]);
        let script = NativeScript::ScriptNOfK(
            2,
            vec![
                NativeScript::ScriptPubkey(k1),
                NativeScript::ScriptPubkey(k2),
                NativeScript::ScriptPubkey(k3),
            ],
        );
        let signers: HashSet<Hash32> = [k1, k3].into();
        assert!(evaluate_native_script(&script, &signers, SlotNo(100)));
        let one: HashSet<Hash32> = [k1].into();
        assert!(!evaluate_native_script(&script, &one, SlotNo(100)));
    }

    #[test]
    fn test_native_script_invalid_before() {
        let script = NativeScript::InvalidBefore(SlotNo(50));
        let signers: HashSet<Hash32> = HashSet::new();
        assert!(evaluate_native_script(&script, &signers, SlotNo(50)));
        assert!(evaluate_native_script(&script, &signers, SlotNo(100)));
        assert!(!evaluate_native_script(&script, &signers, SlotNo(49)));
    }

    #[test]
    fn test_native_script_invalid_hereafter() {
        let script = NativeScript::InvalidHereafter(SlotNo(100));
        let signers: HashSet<Hash32> = HashSet::new();
        assert!(evaluate_native_script(&script, &signers, SlotNo(99)));
        assert!(!evaluate_native_script(&script, &signers, SlotNo(100)));
        assert!(!evaluate_native_script(&script, &signers, SlotNo(101)));
    }

    #[test]
    fn test_native_script_nested_timelock_multisig() {
        let k1 = Hash32::from_bytes([1u8; 32]);
        let script = NativeScript::ScriptAll(vec![
            NativeScript::ScriptPubkey(k1),
            NativeScript::InvalidBefore(SlotNo(50)),
            NativeScript::InvalidHereafter(SlotNo(200)),
        ]);
        let signers: HashSet<Hash32> = [k1].into();
        assert!(evaluate_native_script(&script, &signers, SlotNo(100)));
        assert!(!evaluate_native_script(&script, &signers, SlotNo(49)));
        assert!(!evaluate_native_script(&script, &signers, SlotNo(200)));
        let empty: HashSet<Hash32> = HashSet::new();
        assert!(!evaluate_native_script(&script, &empty, SlotNo(100)));
    }

    // ---------------------------------------------------------------------------
    // Certificates: deposits and refunds
    // ---------------------------------------------------------------------------

    #[test]
    fn test_stake_registration_deposit() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let key_deposit = params.key_deposit.0;
        let mut tx = make_simple_tx(input, 10_000_000 - 200_000 - key_deposit, 200_000);
        tx.body.certificates.push(Certificate::StakeRegistration(
            torsten_primitives::credentials::Credential::VerificationKey(
                torsten_primitives::hash::Hash28::from_bytes([5u8; 28]),
            ),
        ));
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_stake_deregistration_refund() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let key_deposit = params.key_deposit.0;
        let mut tx = make_simple_tx(input, 10_000_000 - 200_000 + key_deposit, 200_000);
        tx.body.certificates.push(Certificate::StakeDeregistration(
            torsten_primitives::credentials::Credential::VerificationKey(
                torsten_primitives::hash::Hash28::from_bytes([5u8; 28]),
            ),
        ));
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_deposit_not_accounted_fails() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.certificates.push(Certificate::StakeRegistration(
            torsten_primitives::credentials::Credential::VerificationKey(
                torsten_primitives::hash::Hash28::from_bytes([5u8; 28]),
            ),
        ));
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_deposits_and_refunds_calculation() {
        let params = ProtocolParameters::mainnet_defaults();
        let cred = torsten_primitives::credentials::Credential::VerificationKey(
            torsten_primitives::hash::Hash28::from_bytes([5u8; 28]),
        );
        let certs = vec![
            Certificate::StakeRegistration(cred.clone()),
            Certificate::StakeRegistration(cred.clone()),
            Certificate::StakeDeregistration(cred),
        ];
        let (deposits, refunds) = calculate_deposits_and_refunds(&certs, &params, None);
        assert_eq!(deposits, params.key_deposit.0 * 2);
        assert_eq!(refunds, params.key_deposit.0);
    }

    // ---------------------------------------------------------------------------
    // Multi-asset
    // ---------------------------------------------------------------------------

    #[test]
    fn test_multi_asset_conservation_valid() {
        let policy = torsten_primitives::hash::Hash28::from_bytes([10u8; 28]);
        let asset = AssetName::new(b"Token".to_vec()).unwrap();
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        let mut input_value = Value::lovelace(10_000_000);
        input_value
            .multi_asset
            .entry(policy)
            .or_default()
            .insert(asset.clone(), 100);
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: input_value,
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let mut output_value = Value::lovelace(9_800_000);
        output_value
            .multi_asset
            .entry(policy)
            .or_default()
            .insert(asset, 100);
        let params = ProtocolParameters::mainnet_defaults();
        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: output_value,
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
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
        };
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_multi_asset_not_conserved() {
        let policy = torsten_primitives::hash::Hash28::from_bytes([10u8; 28]);
        let asset = AssetName::new(b"Token".to_vec()).unwrap();
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        let mut input_value = Value::lovelace(10_000_000);
        input_value
            .multi_asset
            .entry(policy)
            .or_default()
            .insert(asset.clone(), 100);
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: input_value,
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let mut output_value = Value::lovelace(9_800_000);
        output_value
            .multi_asset
            .entry(policy)
            .or_default()
            .insert(asset, 200);
        let params = ProtocolParameters::mainnet_defaults();
        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: output_value,
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
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
        };
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::MultiAssetNotConserved { .. })));
    }

    #[test]
    fn test_multi_asset_with_minting() {
        let script = NativeScript::ScriptAll(vec![]);
        let script_cbor = torsten_serialization::encode_native_script(&script);
        let mut tagged = vec![0x00];
        tagged.extend_from_slice(&script_cbor);
        let policy = torsten_primitives::hash::blake2b_224(&tagged);
        let asset = AssetName::new(b"Token".to_vec()).unwrap();
        let (utxo_set, input) = make_simple_utxo_set();
        let mut output_value = Value::lovelace(9_800_000);
        output_value
            .multi_asset
            .entry(policy)
            .or_default()
            .insert(asset.clone(), 50);
        let mut mint: BTreeMap<torsten_primitives::hash::PolicyId, BTreeMap<AssetName, i64>> =
            BTreeMap::new();
        mint.entry(policy).or_default().insert(asset, 50);
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.outputs[0].value = output_value;
        tx.body.mint = mint;
        tx.witness_set.native_scripts.push(script);
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_multi_asset_burning() {
        let script = NativeScript::ScriptAll(vec![]);
        let script_cbor = torsten_serialization::encode_native_script(&script);
        let mut tagged = vec![0x00];
        tagged.extend_from_slice(&script_cbor);
        let policy = torsten_primitives::hash::blake2b_224(&tagged);
        let asset = AssetName::new(b"Token".to_vec()).unwrap();
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        let mut input_value = Value::lovelace(10_000_000);
        input_value
            .multi_asset
            .entry(policy)
            .or_default()
            .insert(asset.clone(), 100);
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: input_value,
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let mut output_value = Value::lovelace(9_800_000);
        output_value
            .multi_asset
            .entry(policy)
            .or_default()
            .insert(asset.clone(), 70);
        let mut mint: BTreeMap<torsten_primitives::hash::PolicyId, BTreeMap<AssetName, i64>> =
            BTreeMap::new();
        mint.entry(policy).or_default().insert(asset, -30);
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.outputs[0].value = output_value;
        tx.body.mint = mint;
        tx.witness_set.native_scripts.push(script);
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_minting_without_script_rejected() {
        let policy = torsten_primitives::hash::Hash28::from_bytes([10u8; 28]);
        let asset = AssetName::new(b"Token".to_vec()).unwrap();
        let (utxo_set, input) = make_simple_utxo_set();
        let mut output_value = Value::lovelace(9_800_000);
        output_value
            .multi_asset
            .entry(policy)
            .or_default()
            .insert(asset.clone(), 50);
        let mut mint: BTreeMap<torsten_primitives::hash::PolicyId, BTreeMap<AssetName, i64>> =
            BTreeMap::new();
        mint.entry(policy).or_default().insert(asset, 50);
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.outputs[0].value = output_value;
        tx.body.mint = mint;
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::InvalidMint)));
    }

    // ---------------------------------------------------------------------------
    // Collateral helpers
    // ---------------------------------------------------------------------------

    fn make_plutus_tx_with_collateral(
        input: TransactionInput,
        output_value: u64,
        fee: u64,
        collateral: Vec<TransactionInput>,
    ) -> Transaction {
        let mut tx = make_simple_tx(input, output_value, fee);
        tx.body.collateral = collateral;
        tx.witness_set.redeemers.push(Redeemer {
            tag: RedeemerTag::Spend,
            index: 0,
            data: PlutusData::Integer(0),
            ex_units: ExUnits {
                mem: 100,
                steps: 100,
            },
        });
        tx.witness_set.plutus_v2_scripts.push(vec![0x01]);
        let params = ProtocolParameters::mainnet_defaults();
        let computed_hash = torsten_serialization::compute_script_data_hash(
            &tx.witness_set.redeemers,
            &tx.witness_set.plutus_data,
            &params.cost_models,
            !tx.witness_set.plutus_v1_scripts.is_empty(),
            !tx.witness_set.plutus_v2_scripts.is_empty(),
            !tx.witness_set.plutus_v3_scripts.is_empty(),
            None,
            None,
        );
        tx.body.script_data_hash = Some(computed_hash);
        tx
    }

    #[test]
    fn test_plutus_collateral_valid() {
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let col_input = TransactionInput {
            transaction_id: Hash32::from_bytes([2u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            col_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let params = ProtocolParameters::mainnet_defaults();
        let tx = make_plutus_tx_with_collateral(input, 9_800_000, 200_000, vec![col_input]);
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        if let Err(errors) = &result {
            assert!(
                !errors.iter().any(|e| matches!(
                    e,
                    ValidationError::InsufficientCollateral
                        | ValidationError::TooManyCollateralInputs { .. }
                        | ValidationError::CollateralNotFound(_)
                        | ValidationError::CollateralHasTokens(_)
                        | ValidationError::CollateralMismatch { .. }
                )),
                "No collateral errors expected, got: {errors:?}"
            );
        }
    }

    #[test]
    fn test_plutus_collateral_not_found() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let missing_col = TransactionInput {
            transaction_id: Hash32::from_bytes([99u8; 32]),
            index: 0,
        };
        let tx = make_plutus_tx_with_collateral(input, 9_800_000, 200_000, vec![missing_col]);
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::CollateralNotFound(_))));
    }

    #[test]
    fn test_plutus_collateral_has_tokens() {
        let policy = torsten_primitives::hash::Hash28::from_bytes([10u8; 28]);
        let asset = AssetName::new(b"Token".to_vec()).unwrap();
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let col_input = TransactionInput {
            transaction_id: Hash32::from_bytes([2u8; 32]),
            index: 0,
        };
        let mut col_value = Value::lovelace(5_000_000);
        col_value
            .multi_asset
            .entry(policy)
            .or_default()
            .insert(asset, 100);
        utxo_set.insert(
            col_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: col_value,
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let params = ProtocolParameters::mainnet_defaults();
        let tx = make_plutus_tx_with_collateral(input, 9_800_000, 200_000, vec![col_input]);
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::CollateralHasTokens(_))));
    }

    #[test]
    fn test_plutus_too_many_collateral_inputs() {
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let mut collateral = Vec::new();
        for i in 2..=5u8 {
            let col = TransactionInput {
                transaction_id: Hash32::from_bytes([i; 32]),
                index: 0,
            };
            utxo_set.insert(
                col.clone(),
                TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(1_000_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                },
            );
            collateral.push(col);
        }
        let params = ProtocolParameters::mainnet_defaults();
        let tx = make_plutus_tx_with_collateral(input, 9_800_000, 200_000, collateral);
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::TooManyCollateralInputs { .. })));
    }

    #[test]
    fn test_plutus_ex_units_exceeded() {
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let col_input = TransactionInput {
            transaction_id: Hash32::from_bytes([2u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            col_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_plutus_tx_with_collateral(input, 9_800_000, 200_000, vec![col_input]);
        tx.witness_set.redeemers[0].ex_units = ExUnits {
            mem: u64::MAX,
            steps: u64::MAX,
        };
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::ExUnitsExceeded)));
    }

    // ---------------------------------------------------------------------------
    // Reference inputs
    // ---------------------------------------------------------------------------

    #[test]
    fn test_reference_input_valid() {
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let ref_input = TransactionInput {
            transaction_id: Hash32::from_bytes([2u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            ref_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.reference_inputs = vec![ref_input];
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_reference_input_not_found() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let missing_ref = TransactionInput {
            transaction_id: Hash32::from_bytes([99u8; 32]),
            index: 0,
        };
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.reference_inputs = vec![missing_ref];
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::ReferenceInputNotFound(_))));
    }

    #[test]
    fn test_reference_input_overlaps_input() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input.clone(), 9_800_000, 200_000);
        tx.body.reference_inputs = vec![input];
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::ReferenceInputOverlapsInput(_))));
    }

    #[test]
    fn test_required_signer_missing() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.required_signers = vec![Hash32::from_bytes([0xAA; 32])];
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::MissingRequiredSigner(_))));
    }

    #[test]
    fn test_duplicate_input() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input.clone(), 9_800_000, 200_000);
        tx.body.inputs.push(input);
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::DuplicateInput(_))));
    }

    #[test]
    fn test_native_script_validation_in_tx() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        let required_key = Hash32::from_bytes([0xBB; 32]);
        tx.witness_set
            .native_scripts
            .push(NativeScript::ScriptPubkey(required_key));
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::NativeScriptFailed)));
    }

    #[test]
    fn test_native_script_timelock_in_tx() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.witness_set
            .native_scripts
            .push(NativeScript::InvalidBefore(SlotNo(200)));
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let result = validate_transaction(&tx, &utxo_set, &params, 200, 300, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_witness_signature_verification() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.witness_set.vkey_witnesses.push(VKeyWitness {
            vkey: vec![1u8; 32],
            signature: vec![0u8; 64],
        });
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::InvalidWitnessSignature(_))));
    }

    #[test]
    fn test_collateral_return_reduces_effective_collateral() {
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let col_input = TransactionInput {
            transaction_id: Hash32::from_bytes([2u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            col_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_plutus_tx_with_collateral(input, 9_800_000, 200_000, vec![col_input]);
        tx.body.collateral_return = Some(TransactionOutput {
            address: Address::Byron(ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value::lovelace(4_700_000),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        });
        tx.body.total_collateral = Some(Lovelace(300_000));
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        if let Err(errors) = &result {
            assert!(
                !errors.iter().any(|e| matches!(
                    e,
                    ValidationError::InsufficientCollateral
                        | ValidationError::TooManyCollateralInputs { .. }
                        | ValidationError::CollateralNotFound(_)
                        | ValidationError::CollateralHasTokens(_)
                        | ValidationError::CollateralMismatch { .. }
                )),
                "No collateral errors expected, got: {errors:?}"
            );
        }
    }

    #[test]
    fn test_collateral_return_mismatch_total() {
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let col_input = TransactionInput {
            transaction_id: Hash32::from_bytes([2u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            col_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_plutus_tx_with_collateral(input, 9_800_000, 200_000, vec![col_input]);
        tx.body.collateral_return = Some(TransactionOutput {
            address: Address::Byron(ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value::lovelace(4_700_000),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        });
        tx.body.total_collateral = Some(Lovelace(500_000)); // wrong
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::CollateralMismatch { .. })));
    }

    #[test]
    fn test_reference_script_minting_validation() {
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let pubkey_hash = Hash32::from_bytes([42u8; 32]);
        let native_script = NativeScript::ScriptPubkey(pubkey_hash);
        let script_hash = compute_script_ref_hash(&ScriptRef::NativeScript(native_script.clone()));
        let ref_input = TransactionInput {
            transaction_id: Hash32::from_bytes([3u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            ref_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(2_000_000),
                datum: OutputDatum::None,
                script_ref: Some(ScriptRef::NativeScript(native_script)),
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let asset = AssetName(b"Token".to_vec());
        let mut mint: BTreeMap<torsten_primitives::hash::PolicyId, BTreeMap<AssetName, i64>> =
            BTreeMap::new();
        mint.entry(script_hash)
            .or_default()
            .insert(asset.clone(), 10);
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.mint = mint;
        tx.body.reference_inputs.push(ref_input);
        tx.body.outputs[0]
            .value
            .multi_asset
            .entry(script_hash)
            .or_default()
            .insert(asset, 10);
        let params = ProtocolParameters::mainnet_defaults();
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_ok());
    }

    // ---------------------------------------------------------------------------
    // Issue #155: minting policy satisfied by script_ref on a spending input
    //
    // Haskell's `scriptsProvided` collects script_refs from BOTH spending inputs
    // and reference inputs.  Our original Rule 3c and collect_available_script_hashes
    // only scanned reference_inputs, causing false InvalidMint rejections.
    // ---------------------------------------------------------------------------

    #[test]
    fn test_minting_policy_satisfied_by_spending_input_script_ref() {
        // Scenario: the spending input's UTxO itself carries a script_ref whose hash
        // matches the minting policy.  No reference_inputs are needed.
        let native_script = NativeScript::ScriptAll(vec![]);
        let script_hash = compute_script_ref_hash(&ScriptRef::NativeScript(native_script.clone()));

        let mut utxo_set = UtxoSet::new();
        // Spending input carries the script_ref
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                // script_ref on the spending input — the fix makes this count
                script_ref: Some(ScriptRef::NativeScript(native_script.clone())),
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let asset = AssetName(b"Coin".to_vec());
        let mut mint: BTreeMap<torsten_primitives::hash::PolicyId, BTreeMap<AssetName, i64>> =
            BTreeMap::new();
        mint.entry(script_hash)
            .or_default()
            .insert(asset.clone(), 100);

        let mut output_value = Value::lovelace(9_800_000);
        output_value
            .multi_asset
            .entry(script_hash)
            .or_default()
            .insert(asset, 100);

        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.mint = mint;
        tx.body.outputs[0].value = output_value;
        // No reference_inputs; the script comes from the spending input's UTxO

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(
            result.is_ok(),
            "Minting policy satisfied via spending-input script_ref should be accepted: {result:?}"
        );
    }

    #[test]
    fn test_minting_policy_spending_input_script_ref_wrong_hash_fails() {
        // Script_ref on the spending input does NOT match the minting policy hash.
        // Should still fail with InvalidMint.
        let native_script = NativeScript::ScriptAll(vec![]);
        let wrong_policy = Hash28::from_bytes([0xde; 28]); // not the script's hash

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: Some(ScriptRef::NativeScript(native_script)),
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let asset = AssetName(b"Coin".to_vec());
        let mut mint: BTreeMap<torsten_primitives::hash::PolicyId, BTreeMap<AssetName, i64>> =
            BTreeMap::new();
        mint.entry(wrong_policy)
            .or_default()
            .insert(asset.clone(), 100);
        let mut output_value = Value::lovelace(9_800_000);
        output_value
            .multi_asset
            .entry(wrong_policy)
            .or_default()
            .insert(asset, 100);

        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.mint = mint;
        tx.body.outputs[0].value = output_value;

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::InvalidMint)),
            "Wrong script_ref hash should still produce InvalidMint: {errors:?}"
        );
    }

    #[test]
    fn test_witness_completeness_script_input_satisfied_by_spending_input_script_ref() {
        // Rule 9b: a script-locked input's script can be satisfied by a script_ref
        // on another spending input (not just reference inputs).
        use torsten_primitives::address::EnterpriseAddress;
        use torsten_primitives::credentials::Credential;

        let native_script = NativeScript::ScriptAll(vec![]);
        let script_hash = compute_script_ref_hash(&ScriptRef::NativeScript(native_script.clone()));

        let mut utxo_set = UtxoSet::new();

        // Input 0: carries the script_ref (the "provider" input)
        let provider_input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            provider_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(1_000_000),
                datum: OutputDatum::None,
                script_ref: Some(ScriptRef::NativeScript(native_script)),
                is_legacy: false,
                raw_cbor: None,
            },
        );

        // Input 1: locked by the script whose hash is script_hash
        let script_input = TransactionInput {
            transaction_id: Hash32::from_bytes([2u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            script_input.clone(),
            TransactionOutput {
                address: Address::Enterprise(EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Testnet,
                    payment: Credential::Script(script_hash),
                }),
                value: Value::lovelace(9_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(provider_input.clone(), 9_800_000, 200_000);
        // Spend both inputs together
        tx.body.inputs.push(script_input);
        // Recalculate value conservation: 1_000_000 + 9_000_000 = 10_000_000
        tx.body.outputs[0].value = Value::lovelace(9_800_000);
        // No explicit scripts in the witness_set — only the spending input's script_ref

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        // After the fix, Rule 9b finds the script via the spending input's script_ref.
        match result {
            Ok(()) => {}
            Err(ref errors) => {
                assert!(
                    !errors.iter().any(|e| matches!(e, ValidationError::MissingScriptWitness(_))),
                    "Script witness should be satisfied via spending-input script_ref, got: {errors:?}"
                );
            }
        }
    }

    #[test]
    fn test_compute_script_ref_hash_plutus_v2() {
        let script_bytes = vec![0x01, 0x02, 0x03, 0x04];
        let hash = compute_script_ref_hash(&ScriptRef::PlutusV2(script_bytes.clone()));
        let mut tagged = vec![0x02];
        tagged.extend_from_slice(&script_bytes);
        let expected = torsten_primitives::hash::blake2b_224(&tagged);
        assert_eq!(hash, expected);
    }

    #[test]
    fn test_ref_script_tiered_fee_calculation() {
        assert_eq!(calculate_ref_script_tiered_fee(15, 1000), 15_000);
        assert_eq!(calculate_ref_script_tiered_fee(15, 25_600), 384_000);
        let fee = calculate_ref_script_tiered_fee(15, 26_600);
        assert_eq!(fee, 384_000 + 18_000);
        assert_eq!(calculate_ref_script_tiered_fee(15, 0), 0);
        assert_eq!(
            calculate_ref_script_tiered_fee(15, 25_600 * 3),
            1_397_760,
            "3 tiers must use exact rational arithmetic (21.6 not 21)"
        );
    }

    #[test]
    fn test_script_ref_byte_size() {
        let v2_script = ScriptRef::PlutusV2(vec![0u8; 500]);
        assert_eq!(script_ref_byte_size(&v2_script), 500);
        let v3_script = ScriptRef::PlutusV3(vec![0u8; 1024]);
        assert_eq!(script_ref_byte_size(&v3_script), 1024);
    }

    #[test]
    fn test_auxiliary_data_hash_without_data() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_000_000, 1_000_000);
        tx.body.auxiliary_data_hash = Some(Hash32::from_bytes([0xAB; 32]));
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::AuxiliaryDataHashWithoutData)));
    }

    #[test]
    fn test_auxiliary_data_without_hash() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_000_000, 1_000_000);
        tx.auxiliary_data = Some(AuxiliaryData {
            metadata: BTreeMap::new(),
            native_scripts: vec![],
            plutus_v1_scripts: vec![],
            plutus_v2_scripts: vec![],
            plutus_v3_scripts: vec![],
        });
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::AuxiliaryDataWithoutHash)));
    }

    #[test]
    fn test_auxiliary_data_with_hash_valid() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_000_000, 1_000_000);
        tx.body.auxiliary_data_hash = Some(Hash32::from_bytes([0xAB; 32]));
        tx.auxiliary_data = Some(AuxiliaryData {
            metadata: BTreeMap::new(),
            native_scripts: vec![],
            plutus_v1_scripts: vec![],
            plutus_v2_scripts: vec![],
            plutus_v3_scripts: vec![],
        });
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_cbor_uint_size() {
        assert_eq!(cbor_uint_size(0), 1);
        assert_eq!(cbor_uint_size(23), 1);
        assert_eq!(cbor_uint_size(24), 2);
        assert_eq!(cbor_uint_size(255), 2);
        assert_eq!(cbor_uint_size(256), 3);
        assert_eq!(cbor_uint_size(65535), 3);
        assert_eq!(cbor_uint_size(65536), 5);
        assert_eq!(cbor_uint_size(0xFFFF_FFFF), 5);
        assert_eq!(cbor_uint_size(0x1_0000_0000), 9);
    }

    #[test]
    fn test_estimate_value_cbor_size_ada_only() {
        let value = Value::lovelace(1_000_000);
        let size = estimate_value_cbor_size(&value);
        assert_eq!(size, cbor_uint_size(1_000_000));
    }

    #[test]
    fn test_estimate_value_cbor_size_multi_asset() {
        let policy = torsten_primitives::hash::Hash28::from_bytes([10u8; 28]);
        let asset = AssetName::new(b"Token".to_vec()).unwrap();
        let mut value = Value::lovelace(2_000_000);
        value
            .multi_asset
            .entry(policy)
            .or_default()
            .insert(asset, 100);
        let size = estimate_value_cbor_size(&value);
        assert_eq!(size, 45);
    }

    #[test]
    fn test_output_value_too_large() {
        let policy = torsten_primitives::hash::Hash28::from_bytes([10u8; 28]);
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(100_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let mut output_value = Value::lovelace(99_800_000);
        for i in 0..100u8 {
            let asset = AssetName::new(vec![i; 32]).unwrap();
            output_value
                .multi_asset
                .entry(policy)
                .or_default()
                .insert(asset, 1_000_000);
        }
        let mut params = ProtocolParameters::mainnet_defaults();
        params.max_val_size = 50;
        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: output_value,
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
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
        };
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::OutputValueTooLarge { .. })));
    }

    #[test]
    fn test_ada_only_output_skips_max_val_size_check() {
        let (utxo_set, input) = make_simple_utxo_set();
        let mut params = ProtocolParameters::mainnet_defaults();
        params.max_val_size = 1;
        let tx = make_simple_tx(input, 9_800_000, 200_000);
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_redeemer_index_out_of_range() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input.clone(), 9_000_000, 1_000_000);
        tx.witness_set.redeemers.push(Redeemer {
            tag: RedeemerTag::Spend,
            index: 5,
            data: PlutusData::Integer(0),
            ex_units: ExUnits {
                mem: 100,
                steps: 100,
            },
        });
        tx.witness_set.plutus_v2_scripts.push(vec![0x01, 0x02]);
        let col_input = TransactionInput {
            transaction_id: Hash32::from_bytes([2u8; 32]),
            index: 0,
        };
        let mut utxo = utxo_set;
        utxo.insert(
            col_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        tx.body.collateral = vec![col_input];
        let result = validate_transaction(&tx, &utxo, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::RedeemerIndexOutOfRange { .. })));
    }

    #[test]
    fn test_script_locked_input_missing_redeemer() {
        use torsten_primitives::address::EnterpriseAddress;
        use torsten_primitives::credentials::Credential;

        let script_hash = Hash28::from_bytes([0xaa; 28]);
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([3u8; 32]),
            index: 0,
        };
        let mut utxo_set = UtxoSet::new();
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Enterprise(EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Testnet,
                    payment: Credential::Script(script_hash),
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let col_input = TransactionInput {
            transaction_id: Hash32::from_bytes([4u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            col_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let params = ProtocolParameters::mainnet_defaults();
        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(9_000_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(1_000_000),
                ttl: None,
                certificates: vec![],
                withdrawals: BTreeMap::new(),
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint: BTreeMap::new(),
                script_data_hash: None,
                collateral: vec![col_input],
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
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![vec![0x01]],
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
        };
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::MissingSpendRedeemer { .. })));
    }

    #[test]
    fn test_script_locked_input_with_redeemer_ok() {
        use torsten_primitives::address::EnterpriseAddress;
        use torsten_primitives::credentials::Credential;

        let script_hash = Hash28::from_bytes([0xbb; 28]);
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([5u8; 32]),
            index: 0,
        };
        let mut utxo_set = UtxoSet::new();
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Enterprise(EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Testnet,
                    payment: Credential::Script(script_hash),
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let col_input = TransactionInput {
            transaction_id: Hash32::from_bytes([6u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            col_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let params = ProtocolParameters::mainnet_defaults();
        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(9_000_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(1_000_000),
                ttl: None,
                certificates: vec![],
                withdrawals: BTreeMap::new(),
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint: BTreeMap::new(),
                script_data_hash: Some(Hash32::ZERO),
                collateral: vec![col_input],
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
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![vec![0x01]],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                redeemers: vec![Redeemer {
                    tag: RedeemerTag::Spend,
                    index: 0,
                    data: PlutusData::Integer(42),
                    ex_units: ExUnits {
                        mem: 1000,
                        steps: 1000,
                    },
                }],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        match result {
            Ok(()) => {}
            Err(errors) => {
                assert!(!errors
                    .iter()
                    .any(|e| matches!(e, ValidationError::MissingSpendRedeemer { .. })));
            }
        }
    }

    #[test]
    fn test_treasury_donation_value_conservation() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 8_800_000, 200_000);
        tx.body.donation = Some(Lovelace(1_000_000));
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_treasury_donation_value_not_conserved() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.donation = Some(Lovelace(1_000_000));
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::ValueNotConserved { .. })));
    }

    // ---------------------------------------------------------------------------
    // Bug fix tests
    // ---------------------------------------------------------------------------

    #[test]
    fn test_script_data_hash_mismatch_rejects() {
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let col_input = TransactionInput {
            transaction_id: Hash32::from_bytes([2u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            col_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.collateral = vec![col_input];
        tx.witness_set.plutus_v2_scripts.push(vec![0x01]);
        tx.witness_set.redeemers.push(Redeemer {
            tag: RedeemerTag::Spend,
            index: 0,
            data: PlutusData::Integer(42),
            ex_units: ExUnits {
                mem: 1000,
                steps: 1000,
            },
        });
        tx.body.script_data_hash = Some(Hash32::from_bytes([0xDE; 32]));
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::ScriptDataHashMismatch { .. })),
            "Expected ScriptDataHashMismatch error, got: {errors:?}"
        );
    }

    #[test]
    fn test_min_fee_includes_execution_unit_costs() {
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(100_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let col_input = TransactionInput {
            transaction_id: Hash32::from_bytes([2u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            col_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(50_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let params = ProtocolParameters::mainnet_defaults();
        let tx_size: u64 = 300;
        let base_fee = params.min_fee(tx_size).0;
        let mem_units: u64 = 14_000_000;
        let step_units: u64 = 10_000_000_000;
        let fee_without_ex = base_fee;
        let output_value = 100_000_000 - fee_without_ex;
        let mut tx = make_simple_tx(input, output_value, fee_without_ex);
        tx.body.collateral = vec![col_input];
        tx.witness_set.plutus_v2_scripts.push(vec![0x01]);
        tx.witness_set.redeemers.push(Redeemer {
            tag: RedeemerTag::Spend,
            index: 0,
            data: PlutusData::Integer(42),
            ex_units: ExUnits {
                mem: mem_units,
                steps: step_units,
            },
        });
        tx.body.script_data_hash = Some(Hash32::from_bytes([0xAB; 32]));
        let result = validate_transaction(&tx, &utxo_set, &params, 100, tx_size, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::FeeTooSmall { .. })),
            "Expected FeeTooSmall error when ex unit costs not covered, got: {errors:?}"
        );
    }

    #[test]
    fn test_min_fee_no_redeemers_no_ex_unit_cost() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let tx_size: u64 = 300;
        let base_fee = params.min_fee(tx_size).0;
        let output_value = 10_000_000 - base_fee;
        let tx = make_simple_tx(input, output_value, base_fee);
        let result = validate_transaction(&tx, &utxo_set, &params, 100, tx_size, None);
        assert!(
            result.is_ok(),
            "Simple tx with exact base fee should pass: {result:?}"
        );
    }

    // ---------------------------------------------------------------------------
    // Pool re-registration
    // ---------------------------------------------------------------------------

    fn make_pool_params(pool_id: Hash28) -> PoolParams {
        PoolParams {
            operator: pool_id,
            vrf_keyhash: Hash32::from_bytes([0u8; 32]),
            pledge: Lovelace(100_000_000),
            cost: Lovelace(340_000_000),
            margin: Rational {
                numerator: 1,
                denominator: 100,
            },
            reward_account: vec![0xe0; 29],
            pool_owners: vec![pool_id],
            relays: vec![],
            pool_metadata: None,
        }
    }

    #[test]
    fn test_pool_reregistration_no_duplicate_deposit() {
        let pool_id = torsten_primitives::hash::Hash28::from_bytes([42u8; 28]);
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body
            .certificates
            .push(Certificate::PoolRegistration(make_pool_params(pool_id)));
        let mut registered = HashSet::new();
        registered.insert(pool_id);
        let result = validate_transaction_with_pools(
            &tx,
            &utxo_set,
            &params,
            100,
            300,
            None,
            Some(&registered),
            None,
            None,
            None,
        );
        assert!(
            result.is_ok(),
            "Pool re-registration should not charge deposit: {result:?}"
        );
    }

    #[test]
    fn test_new_pool_registration_charges_deposit() {
        let pool_id = torsten_primitives::hash::Hash28::from_bytes([42u8; 28]);
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(1_000_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let params = ProtocolParameters::mainnet_defaults();
        let pool_deposit = params.pool_deposit.0;
        let fee = 200_000u64;
        let output = 1_000_000_000 - fee - pool_deposit;
        let mut tx = make_simple_tx(input, output, fee);
        tx.body
            .certificates
            .push(Certificate::PoolRegistration(make_pool_params(pool_id)));
        let registered: HashSet<Hash28> = HashSet::new();
        let result = validate_transaction_with_pools(
            &tx,
            &utxo_set,
            &params,
            100,
            300,
            None,
            Some(&registered),
            None,
            None,
            None,
        );
        assert!(
            result.is_ok(),
            "New pool registration should charge deposit: {result:?}"
        );
    }

    #[test]
    fn test_new_pool_registration_without_deposit_fails() {
        let pool_id = torsten_primitives::hash::Hash28::from_bytes([42u8; 28]);
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body
            .certificates
            .push(Certificate::PoolRegistration(make_pool_params(pool_id)));
        let registered: HashSet<Hash28> = HashSet::new();
        let result = validate_transaction_with_pools(
            &tx,
            &utxo_set,
            &params,
            100,
            300,
            None,
            Some(&registered),
            None,
            None,
            None,
        );
        assert!(result.is_err(), "New pool reg without deposit should fail");
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::ValueNotConserved { .. })));
    }

    #[test]
    fn test_calculate_deposits_pool_rereg_no_deposit() {
        let params = ProtocolParameters::mainnet_defaults();
        let pool_id = torsten_primitives::hash::Hash28::from_bytes([42u8; 28]);
        let certs = vec![Certificate::PoolRegistration(make_pool_params(pool_id))];
        let (deposits_new, _) = calculate_deposits_and_refunds(&certs, &params, None);
        assert_eq!(deposits_new, params.pool_deposit.0);
        let mut registered = HashSet::new();
        registered.insert(pool_id);
        let (deposits_rereg, _) =
            calculate_deposits_and_refunds(&certs, &params, Some(&registered));
        assert_eq!(deposits_rereg, 0);
    }

    // ---------------------------------------------------------------------------
    // Plutus Phase-2 prerequisites
    // ---------------------------------------------------------------------------

    fn make_plutus_utxo_and_tx(raw_cbor: Option<Vec<u8>>) -> (UtxoSet, Transaction) {
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        let col_input = TransactionInput {
            transaction_id: Hash32::from_bytes([2u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        utxo_set.insert(
            col_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let redeemers = vec![Redeemer {
            tag: RedeemerTag::Spend,
            index: 0,
            data: PlutusData::Integer(0),
            ex_units: ExUnits {
                mem: 100,
                steps: 100,
            },
        }];
        let plutus_v1_scripts = vec![vec![0x01, 0x02, 0x03]];
        let params = ProtocolParameters::mainnet_defaults();
        let script_data_hash = torsten_serialization::compute_script_data_hash(
            &redeemers,
            &[],
            &params.cost_models,
            true,
            false,
            false,
            None,
            None,
        );
        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(9_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
                ttl: None,
                certificates: vec![],
                withdrawals: BTreeMap::new(),
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint: BTreeMap::new(),
                script_data_hash: Some(script_data_hash),
                collateral: vec![col_input],
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
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts,
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                redeemers,
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor,
        };
        (utxo_set, tx)
    }

    #[test]
    fn test_plutus_tx_missing_raw_cbor_returns_error() {
        let (utxo_set, tx) = make_plutus_utxo_and_tx(None);
        let params = ProtocolParameters::mainnet_defaults();
        let slot_config = crate::plutus::SlotConfig::default();
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, Some(&slot_config));
        assert!(
            result.is_err(),
            "Should reject Plutus tx with missing raw_cbor"
        );
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingRawCbor)),
            "Should contain MissingRawCbor error, got: {errors:?}"
        );
    }

    #[test]
    fn test_plutus_tx_missing_slot_config_returns_error() {
        let (utxo_set, tx) = make_plutus_utxo_and_tx(Some(vec![0x84, 0x00]));
        let params = ProtocolParameters::mainnet_defaults();
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(
            result.is_err(),
            "Should reject Plutus tx with missing slot_config"
        );
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingSlotConfig)),
            "Should contain MissingSlotConfig error, got: {errors:?}"
        );
    }

    #[test]
    fn test_plutus_tx_missing_both_raw_cbor_and_slot_config() {
        let (utxo_set, tx) = make_plutus_utxo_and_tx(None);
        let params = ProtocolParameters::mainnet_defaults();
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingRawCbor)),
            "Should contain MissingRawCbor, got: {errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingSlotConfig)),
            "Should contain MissingSlotConfig, got: {errors:?}"
        );
    }

    #[test]
    fn test_non_plutus_tx_missing_raw_cbor_passes() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let tx = make_simple_tx(input, 9_800_000, 200_000);
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(
            result.is_ok(),
            "Non-Plutus tx should pass without raw_cbor/slot_config"
        );
    }

    #[test]
    fn test_plutus_tx_with_raw_cbor_and_slot_config_reaches_evaluation() {
        let (utxo_set, tx) = make_plutus_utxo_and_tx(Some(vec![0x84, 0x00]));
        let params = ProtocolParameters::mainnet_defaults();
        let slot_config = crate::plutus::SlotConfig::default();
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, Some(&slot_config));
        if let Err(errors) = &result {
            assert!(
                !errors
                    .iter()
                    .any(|e| matches!(e, ValidationError::MissingRawCbor)),
                "Should NOT contain MissingRawCbor when raw_cbor is present"
            );
            assert!(
                !errors
                    .iter()
                    .any(|e| matches!(e, ValidationError::MissingSlotConfig)),
                "Should NOT contain MissingSlotConfig when slot_config is present"
            );
        }
    }

    // ---------------------------------------------------------------------------
    // Witness completeness
    // ---------------------------------------------------------------------------

    fn make_vkey_witness_from_bytes(vkey: [u8; 32]) -> VKeyWitness {
        VKeyWitness {
            vkey: vkey.to_vec(),
            signature: vec![0u8; 64],
        }
    }

    fn make_reward_account_vkey(keyhash: Hash28) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(29);
        bytes.push(0xe0);
        bytes.extend_from_slice(keyhash.as_bytes());
        bytes
    }

    fn make_reward_account_script(script_hash: Hash28) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(29);
        bytes.push(0xf0);
        bytes.extend_from_slice(script_hash.as_bytes());
        bytes
    }

    #[test]
    fn test_witness_completeness_vkey_input_with_matching_witness() {
        let vkey_bytes = [0xAA; 32];
        let keyhash = torsten_primitives::hash::blake2b_224(&vkey_bytes);
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Enterprise(torsten_primitives::address::EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Testnet,
                    payment: torsten_primitives::credentials::Credential::VerificationKey(keyhash),
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.witness_set
            .vkey_witnesses
            .push(make_vkey_witness_from_bytes(vkey_bytes));
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        match result {
            Ok(()) => {}
            Err(errors) => {
                assert!(
                    !errors
                        .iter()
                        .any(|e| matches!(e, ValidationError::MissingInputWitness(_))),
                    "Should not have MissingInputWitness, got: {errors:?}"
                );
            }
        }
    }

    #[test]
    fn test_witness_completeness_vkey_input_missing_witness() {
        let keyhash = Hash28::from_bytes([0x11; 28]);
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Enterprise(torsten_primitives::address::EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Testnet,
                    payment: torsten_primitives::credentials::Credential::VerificationKey(keyhash),
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let params = ProtocolParameters::mainnet_defaults();
        let tx = make_simple_tx(input, 9_800_000, 200_000);
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingInputWitness(_))),
            "Expected MissingInputWitness, got: {errors:?}"
        );
    }

    #[test]
    fn test_witness_completeness_withdrawal_vkey_missing_witness() {
        let keyhash = Hash28::from_bytes([0xCC; 28]);
        let reward_account = make_reward_account_vkey(keyhash);
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 10_100_000, 400_000);
        tx.body
            .withdrawals
            .insert(reward_account, Lovelace(500_000));
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingWithdrawalWitness(_))),
            "Expected MissingWithdrawalWitness, got: {errors:?}"
        );
    }

    #[test]
    fn test_witness_completeness_withdrawal_script_missing_witness() {
        let script_hash = Hash28::from_bytes([0xDD; 28]);
        let reward_account = make_reward_account_script(script_hash);
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 10_100_000, 400_000);
        tx.body
            .withdrawals
            .insert(reward_account, Lovelace(500_000));
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingWithdrawalScriptWitness(_))),
            "Expected MissingWithdrawalScriptWitness, got: {errors:?}"
        );
    }

    #[test]
    fn test_extract_reward_credential_vkey() {
        let keyhash = Hash28::from_bytes([0x42; 28]);
        let reward_account = make_reward_account_vkey(keyhash);
        let cred = extract_reward_credential(&reward_account);
        assert_eq!(
            cred,
            Some(torsten_primitives::credentials::Credential::VerificationKey(keyhash))
        );
    }

    #[test]
    fn test_extract_reward_credential_script() {
        let script_hash = Hash28::from_bytes([0x43; 28]);
        let reward_account = make_reward_account_script(script_hash);
        let cred = extract_reward_credential(&reward_account);
        assert_eq!(
            cred,
            Some(torsten_primitives::credentials::Credential::Script(
                script_hash
            ))
        );
    }

    #[test]
    fn test_extract_reward_credential_too_short() {
        let cred = extract_reward_credential(&[0xe0, 0x01, 0x02]);
        assert_eq!(cred, None);
    }

    #[test]
    fn test_extract_reward_credential_invalid_type() {
        let mut bytes = vec![0x00];
        bytes.extend_from_slice(&[0x00; 28]);
        let cred = extract_reward_credential(&bytes);
        assert_eq!(cred, None);
    }

    // ---------------------------------------------------------------------------
    // Era gating
    // ---------------------------------------------------------------------------

    #[test]
    fn test_era_gating_conway_cert_in_pre_conway_tx_rejected() {
        let (utxo_set, input) = make_simple_utxo_set();
        let mut params = ProtocolParameters::mainnet_defaults();
        params.protocol_version_major = 8;
        params.key_deposit = Lovelace(0);
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.certificates.push(Certificate::RegDRep {
            credential: torsten_primitives::credentials::Credential::VerificationKey(
                Hash28::from_bytes([0xAAu8; 28]),
            ),
            deposit: Lovelace(0),
            anchor: None,
        });
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err(), "Should reject Conway cert in Babbage era");
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::EraGatingViolation { .. })),
            "Should contain EraGatingViolation error, got: {errors:?}"
        );
    }

    #[test]
    fn test_era_gating_conway_cert_in_conway_tx_accepted() {
        let (utxo_set, input) = make_simple_utxo_set();
        let mut params = ProtocolParameters::mainnet_defaults();
        params.protocol_version_major = 9;
        params.key_deposit = Lovelace(0);
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.certificates.push(Certificate::RegDRep {
            credential: torsten_primitives::credentials::Credential::VerificationKey(
                Hash28::from_bytes([0xAAu8; 28]),
            ),
            deposit: Lovelace(0),
            anchor: None,
        });
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        if let Err(errors) = &result {
            assert!(
                !errors
                    .iter()
                    .any(|e| matches!(e, ValidationError::EraGatingViolation { .. })),
                "Should NOT have EraGatingViolation in Conway era, got: {errors:?}"
            );
        }
    }

    #[test]
    fn test_conway_only_certificate_name_classification() {
        let conway_certs = vec![
            Certificate::RegDRep {
                credential: torsten_primitives::credentials::Credential::VerificationKey(
                    Hash28::from_bytes([0u8; 28]),
                ),
                deposit: Lovelace(0),
                anchor: None,
            },
            Certificate::UnregDRep {
                credential: torsten_primitives::credentials::Credential::VerificationKey(
                    Hash28::from_bytes([0u8; 28]),
                ),
                refund: Lovelace(0),
            },
            Certificate::CommitteeHotAuth {
                cold_credential: torsten_primitives::credentials::Credential::VerificationKey(
                    Hash28::from_bytes([0u8; 28]),
                ),
                hot_credential: torsten_primitives::credentials::Credential::VerificationKey(
                    Hash28::from_bytes([1u8; 28]),
                ),
            },
        ];
        for cert in &conway_certs {
            assert!(
                conway_only_certificate_name(cert).is_some(),
                "Should be classified as Conway-only: {cert:?}"
            );
        }
        let pre_conway_certs = vec![
            Certificate::StakeRegistration(
                torsten_primitives::credentials::Credential::VerificationKey(Hash28::from_bytes(
                    [0u8; 28],
                )),
            ),
            Certificate::PoolRetirement {
                pool_hash: Hash28::from_bytes([0u8; 28]),
                epoch: 100,
            },
        ];
        for cert in &pre_conway_certs {
            assert!(
                conway_only_certificate_name(cert).is_none(),
                "Should NOT be classified as Conway-only: {cert:?}"
            );
        }
    }

    // ---------------------------------------------------------------------------
    // Additional error-path tests
    // ---------------------------------------------------------------------------

    #[test]
    fn test_fee_too_small_with_zero_fee() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let tx = make_simple_tx(input, 10_000_000, 0);
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::FeeTooSmall { .. })));
    }

    #[test]
    fn test_ttl_expired_with_ttl_less_than_current_slot() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.ttl = Some(SlotNo(10));
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::TtlExpired { .. })));
    }

    #[test]
    fn test_value_not_conserved_outputs_exceed_inputs() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let tx = make_simple_tx(input, 11_000_000, 200_000);
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::ValueNotConserved { .. })));
    }

    #[test]
    fn test_missing_required_signer() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body
            .required_signers
            .push(Hash32::from_bytes([0xEE; 32]));
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::MissingRequiredSigner(_))));
    }

    // ---------------------------------------------------------------------------
    // Issue #81 — FeeTooSmall false positive: is_valid byte excluded from fee size
    // ---------------------------------------------------------------------------

    /// Verify that `compute_min_fee` uses the Haskell-compatible fee size for Alonzo+
    /// transactions (4-element CBOR array `[body, wits, is_valid, aux_data]`).
    ///
    /// Haskell's `toCBORForSizeComputation` encodes as a 3-element array, omitting
    /// `is_valid` for backwards-compatibility with Mary-era fee formula. The on-chain
    /// raw CBOR is 4-element. We must subtract 1 byte when computing min fee.
    ///
    /// The first byte of the raw CBOR determines the era:
    /// - `0x84` = CBOR array(4) → Alonzo+ tx, subtract 1 byte for is_valid
    /// - `0x83` = CBOR array(3) → pre-Alonzo tx, no adjustment
    #[test]
    fn test_fee_size_excludes_is_valid_for_alonzo_plus_txs() {
        let params = ProtocolParameters::mainnet_defaults();
        let min_fee_a = params.min_fee_a; // 44 lovelace/byte on mainnet

        // Build a minimal UTxO + transaction for fee calculation.
        // The fee conservation check uses Rule 3; we set fees high enough to pass.
        let (utxo_set, input) = make_simple_utxo_set();

        // A fake raw_cbor that starts with 0x84 (CBOR array of 4 elements) — Alonzo+.
        // Content beyond the first byte doesn't matter for fee_tx_size detection.
        let raw_cbor_alonzo: Vec<u8> = std::iter::once(0x84u8)
            .chain(std::iter::repeat_n(0u8, 299))
            .collect();
        let tx_size: u64 = raw_cbor_alonzo.len() as u64; // 300 bytes

        // Haskell would compute fee using (tx_size - 1) = 299 bytes.
        let expected_fee_alonzo = min_fee_a * (tx_size - 1) + params.min_fee_b;

        // Build a tx with is_valid=true and the 0x84-prefixed raw_cbor.
        let fee_alonzo = expected_fee_alonzo;
        let output_value = 10_000_000 - fee_alonzo;
        let mut tx = make_simple_tx(input.clone(), output_value, fee_alonzo);
        tx.raw_cbor = Some(raw_cbor_alonzo);

        let computed = compute_min_fee(&tx, &utxo_set, &params, tx_size);
        assert_eq!(
            computed.0, expected_fee_alonzo,
            "Alonzo+ tx: compute_min_fee must subtract 1 byte (is_valid) from tx_size. \
             Expected {expected_fee_alonzo}, got {}",
            computed.0
        );

        // Also verify full validation passes at exactly the Haskell-compatible fee.
        let result = validate_transaction(&tx, &utxo_set, &params, 100, tx_size, None);
        assert!(
            result.is_ok(),
            "Alonzo+ tx with Haskell-compatible fee should pass validation: {result:?}"
        );
    }

    /// Verify that the fee size adjustment is NOT applied for pre-Alonzo transactions
    /// (3-element CBOR array `[body, wits, aux_data]`, first byte `0x83`).
    #[test]
    fn test_fee_size_unchanged_for_pre_alonzo_txs() {
        let params = ProtocolParameters::mainnet_defaults();
        let min_fee_a = params.min_fee_a;
        let (utxo_set, input) = make_simple_utxo_set();

        // A fake raw_cbor starting with 0x83 (CBOR array of 3 elements) — pre-Alonzo.
        let raw_cbor_shelley: Vec<u8> = std::iter::once(0x83u8)
            .chain(std::iter::repeat_n(0u8, 299))
            .collect();
        let tx_size: u64 = raw_cbor_shelley.len() as u64; // 300 bytes

        // Pre-Alonzo: no adjustment, fee uses the full tx_size.
        let expected_fee_shelley = min_fee_a * tx_size + params.min_fee_b;

        let fee_shelley = expected_fee_shelley;
        let output_value = 10_000_000 - fee_shelley;
        let mut tx = make_simple_tx(input, output_value, fee_shelley);
        tx.raw_cbor = Some(raw_cbor_shelley);

        let computed = compute_min_fee(&tx, &utxo_set, &params, tx_size);
        assert_eq!(
            computed.0, expected_fee_shelley,
            "pre-Alonzo tx: compute_min_fee must NOT subtract any byte from tx_size. \
             Expected {expected_fee_shelley}, got {}",
            computed.0
        );
    }

    /// Regression test for GitHub issue #81.
    ///
    /// TX `9816fcc8efdd80f350a2cca600a268a0e65c2df1b28022f07b99c382112c0fe2`
    /// (mainnet, slot 182,039,011) paid a fee of 168,537 lovelace. Haskell
    /// accepted it; Torsten incorrectly computed minimum = 168,581 (delta = 44 =
    /// 1 byte × min_fee_a(44)).  The root cause: Torsten included the `is_valid`
    /// boolean in tx_size, while Haskell's `toCBORForSizeComputation` omits it.
    ///
    /// With the fix, a 3,830-byte Alonzo+ tx at mainnet params yields:
    ///   min_fee = 44 × (3830 - 1) + 155381 = 44 × 3829 + 155381 = 168,276 + 155,381 = 323,657?
    ///
    /// Actually the concrete numbers depend on the exact tx_size and params.
    /// We test the invariant: for an Alonzo+ tx, the 1-byte deduction is applied.
    ///
    /// Concrete check: if raw_cbor is 3,830 bytes (0x84-prefixed), mainnet params
    /// (min_fee_a=44, min_fee_b=155381), then:
    ///   Torsten-OLD: 44 × 3830 + 155381 = 168,520 + 155,381 = (irrelevant - just verify delta)
    ///   Torsten-NEW: 44 × 3829 + 155381 = 168,476 + 155,381 = (irrelevant)
    ///   Delta = 44 lovelace (exactly as reported in #81)
    #[test]
    fn test_issue_81_is_valid_byte_excluded_from_fee_size() {
        // mainnet Conway protocol parameters (defaults match mainnet genesis)
        let params = ProtocolParameters::mainnet_defaults();
        let min_fee_a = params.min_fee_a;
        let min_fee_b = params.min_fee_b;

        // Simulate an Alonzo+ tx of N bytes where N is chosen so that
        // the uncorrected fee would be 168,581 (the wrong value from #81).
        // 168,581 = 44 * N + 155,381  →  N = (168,581 - 155,381) / 44 = 300 bytes.
        // (Simplified: we just verify the 44-lovelace delta is correct.)
        let tx_size: u64 = 300; // raw CBOR size (includes is_valid byte)
        let wrong_min_fee = min_fee_a * tx_size + min_fee_b;
        let correct_min_fee = min_fee_a * (tx_size - 1) + min_fee_b;
        assert_eq!(
            wrong_min_fee - correct_min_fee,
            44,
            "Delta must be exactly 1 × min_fee_a = 44 lovelace"
        );

        // A tx that pays `correct_min_fee` must pass validation when
        // raw_cbor starts with 0x84 (Alonzo+ 4-element array).
        let (utxo_set, input) = make_simple_utxo_set();
        let output_value = 10_000_000 - correct_min_fee;
        let mut tx = make_simple_tx(input, output_value, correct_min_fee);
        // 0x84-prefixed raw_cbor of the correct total size
        tx.raw_cbor = Some(
            std::iter::once(0x84u8)
                .chain(std::iter::repeat_n(0u8, (tx_size - 1) as usize))
                .collect(),
        );

        let result = validate_transaction(&tx, &utxo_set, &params, 100, tx_size, None);
        assert!(
            result.is_ok(),
            "Issue #81: tx paying correct Haskell-compatible fee must not be rejected: {result:?}"
        );
    }

    #[test]
    fn test_script_data_hash_allowed_with_reference_scripts() {
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let ref_input = TransactionInput {
            transaction_id: Hash32::from_bytes([3u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            ref_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(2_000_000),
                datum: OutputDatum::None,
                script_ref: Some(ScriptRef::PlutusV2(vec![0x01, 0x02, 0x03])),
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.reference_inputs = vec![ref_input];
        tx.body.script_data_hash = Some(Hash32::from_bytes([0xAB; 32]));
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        match result {
            Ok(()) => {}
            Err(errors) => {
                assert!(
                    !errors.iter().any(|e| matches!(e, ValidationError::UnexpectedScriptDataHash)),
                    "UnexpectedScriptDataHash should not fire when reference scripts exist: {errors:?}"
                );
            }
        }
    }

    // ===================================================================
    //  Coverage Sprint: Reference script fee ceiling division tests
    // ===================================================================

    /// Verify that ceiling division is used for ref script fees, producing
    /// values that differ from floor division for fractional accumulations.
    #[test]
    fn test_ref_script_fee_ceiling_vs_floor() {
        // base_fee=1, size=25601: tier 1 = 25600*1 = 25600, tier 2 = 1 * 6/5 = 1.2
        // total = 25601.2 -> Haskell uses floor = 25601
        let fee = calculate_ref_script_tiered_fee(1, 25_601);
        assert_eq!(
            fee, 25_601,
            "Floor: 25600 + 6/5 = 25601.2 -> floor = 25601 (matching Haskell tierRefScriptFee)"
        );
    }

    /// Exact division should produce the same result for ceiling and floor.
    #[test]
    fn test_ref_script_fee_exact_no_rounding() {
        // Single tier, exact: 15 * 1 = 15 (no fraction)
        assert_eq!(calculate_ref_script_tiered_fee(15, 1), 15);
        // Full first tier: 15 * 25600 = 384000 (exact)
        assert_eq!(calculate_ref_script_tiered_fee(15, 25_600), 384_000);
    }

    /// Verify floor on multiple partial tiers (matching Haskell tierRefScriptFee).
    #[test]
    fn test_ref_script_fee_floor_multiple_partial_tiers() {
        // base_fee=1, size=51201 (two full tiers + 1 byte in third tier)
        // tier 1: 25600*1 = 25600
        // tier 2: 25600*6/5 = 30720
        // tier 3: 1*36/25 = 1.44
        // total = 56321.44 -> Haskell floor = 56321
        let fee = calculate_ref_script_tiered_fee(1, 51_201);
        assert_eq!(
            fee, 56_321,
            "Floor: 25600 + 30720 + 1.44 = 56321.44 -> floor = 56321 (matching Haskell)"
        );
    }

    /// Verify that base_fee=0 always returns 0 regardless of size.
    #[test]
    fn test_ref_script_fee_zero_base_fee() {
        assert_eq!(calculate_ref_script_tiered_fee(0, 100_000), 0);
        assert_eq!(calculate_ref_script_tiered_fee(0, 0), 0);
        assert_eq!(calculate_ref_script_tiered_fee(0, 1), 0);
    }

    /// Large script size spanning many tiers should not overflow.
    #[test]
    fn test_ref_script_fee_large_size_no_overflow() {
        // 200 KiB = 204800 bytes = 8 full tiers
        // Should not panic from u128 overflow
        let fee = calculate_ref_script_tiered_fee(15, 204_800);
        assert!(fee > 0, "Large script fee must be positive");
        // Verify monotonicity: larger scripts cost more
        let fee_smaller = calculate_ref_script_tiered_fee(15, 204_799);
        assert!(
            fee > fee_smaller,
            "Fee must increase with size: {} vs {}",
            fee,
            fee_smaller
        );
    }

    /// At exactly the block-body tier cap (1 MiB), the fee must be a finite positive
    /// integer with no f64 arithmetic involved.
    #[test]
    fn test_ref_script_fee_at_tier_cap_exact() {
        // 1 MiB = 1,048,576 bytes = 40 full 25 KiB tiers + one partial tier of 24,576 bytes.
        // base_fee_per_byte = 15 (mainnet default for minFeeRefScriptCostPerByte).
        let total = MAX_REF_SCRIPT_SIZE_TIER_CAP;
        let fee = calculate_ref_script_tiered_fee(15, total);
        assert!(fee > 0, "Fee at tier cap must be positive (got 0)");
        assert_ne!(
            fee,
            u64::MAX,
            "Fee at tier cap must not saturate to u64::MAX"
        );
        // Monotonicity: 1 MiB costs more than 1 MiB - 1 byte.
        let fee_minus1 = calculate_ref_script_tiered_fee(15, total - 1);
        assert!(
            fee >= fee_minus1,
            "Fee at cap ({fee}) must be >= fee at cap-1 ({fee_minus1})"
        );
    }

    /// One byte over the tier cap must saturate to u64::MAX (no f64, no panic).
    #[test]
    fn test_ref_script_fee_over_tier_cap_saturates() {
        let over_cap = MAX_REF_SCRIPT_SIZE_TIER_CAP + 1;
        assert_eq!(
            calculate_ref_script_tiered_fee(15, over_cap),
            u64::MAX,
            "Size one byte over cap must saturate to u64::MAX"
        );
    }

    /// A script size of 1.25 MiB (the original overflow-triggering size from issue #115)
    /// must return u64::MAX without panicking, not an f64-derived value.
    #[test]
    fn test_ref_script_fee_issue115_overflow_size() {
        // 1.25 MiB = 1,310,720 bytes — the first size that triggered u128 overflow
        // in the old code (tier 51/52 causes 6^51 to overflow u128).
        let size_125_mib: u64 = 1_310_720;
        assert!(
            size_125_mib > MAX_REF_SCRIPT_SIZE_TIER_CAP,
            "Test prerequisite: 1.25 MiB must exceed the tier cap"
        );
        let fee = calculate_ref_script_tiered_fee(15, size_125_mib);
        assert_eq!(
            fee,
            u64::MAX,
            "1.25 MiB script must saturate to u64::MAX, got {fee}"
        );
    }

    /// Very large size (u64::MAX bytes) must saturate cleanly to u64::MAX.
    #[test]
    fn test_ref_script_fee_max_u64_size_saturates() {
        let fee = calculate_ref_script_tiered_fee(15, u64::MAX);
        assert_eq!(
            fee,
            u64::MAX,
            "u64::MAX byte size must saturate fee to u64::MAX, got {fee}"
        );
    }

    /// Verify the tier cap constant matches the hardcoded block-body rule value.
    ///
    /// This test exists to catch accidental changes to either the constant in
    /// scripts.rs or the constant in apply.rs that would cause them to diverge.
    #[test]
    fn test_tier_cap_equals_block_body_limit() {
        const EXPECTED_BLOCK_BODY_LIMIT: u64 = 1024 * 1024; // 1 MiB (from apply.rs)
        assert_eq!(
            MAX_REF_SCRIPT_SIZE_TIER_CAP, EXPECTED_BLOCK_BODY_LIMIT,
            "MAX_REF_SCRIPT_SIZE_TIER_CAP must match the Conway block-body limit"
        );
    }

    /// Verify that fee calculation at exactly each tier boundary is exact and consistent
    /// with the geometric series formula.
    ///
    /// At N full tiers of TIER_SIZE bytes each:
    ///   fee = base * TIER_SIZE * sum_{i=0}^{N-1} (6/5)^i
    ///       = base * TIER_SIZE * 5 * ((6/5)^N - 1)
    ///
    /// For N=1, base=15: 15 * 25600 = 384,000 (exact, no fraction).
    /// For N=2, base=15: 384,000 + 15*25600*(6/5) = 384,000 + 460,800 = 844,800.
    /// For N=3, base=15: 844,800 + 15*25600*(6/5)^2 = 844,800 + 552,960 = 1,397,760.
    #[test]
    fn test_ref_script_fee_tier_boundaries_exact() {
        // N=1: one full tier
        assert_eq!(calculate_ref_script_tiered_fee(15, 25_600), 384_000);
        // N=2: two full tiers
        assert_eq!(calculate_ref_script_tiered_fee(15, 51_200), 844_800);
        // N=3: three full tiers (also tested in test_ref_script_tiered_fee_calculation)
        assert_eq!(calculate_ref_script_tiered_fee(15, 76_800), 1_397_760);
    }

    /// base_fee_per_byte = 1 (not divisible by 5): exercises worst-case GCD path
    /// where the rational denominator grows more than for base=15.
    ///
    /// Verified values computed against Haskell tierRefScriptFee with base=1:
    ///   tier 0: 25600 * 1 = 25600
    ///   tier 1: 25600 * 6/5 = 30720  → total = 56320
    ///   tier 2: 25600 * 36/25 = 36864 → total = 93184
    #[test]
    fn test_ref_script_fee_base1_worst_case_gcd() {
        assert_eq!(calculate_ref_script_tiered_fee(1, 25_600), 25_600);
        assert_eq!(calculate_ref_script_tiered_fee(1, 51_200), 56_320);
        assert_eq!(calculate_ref_script_tiered_fee(1, 76_800), 93_184);
        // At the tier cap, must not panic and must be a positive finite value.
        let fee_at_cap = calculate_ref_script_tiered_fee(1, MAX_REF_SCRIPT_SIZE_TIER_CAP);
        assert!(fee_at_cap > 0 && fee_at_cap < u64::MAX);
    }

    // ===================================================================
    //  Coverage Sprint: Auxiliary data hash tests
    // ===================================================================

    /// Auxiliary data hash present without auxiliary data → error type check.
    #[test]
    fn test_aux_data_hash_without_data_error_variant() {
        let utxo_set = UtxoSet::new();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = Transaction::empty_with_hash(Hash32::from_bytes([1u8; 32]));
        tx.body.auxiliary_data_hash = Some(Hash32::from_bytes([0xAB; 32]));
        tx.auxiliary_data = None;
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 200, None);
        assert!(result.is_err(), "Hash without data must be rejected");
        if let Err(errors) = result {
            assert!(
                errors
                    .iter()
                    .any(|e| matches!(e, ValidationError::AuxiliaryDataHashWithoutData)),
                "Expected AuxiliaryDataHashWithoutData error"
            );
        }
    }

    /// Auxiliary data present without hash → error variant check.
    #[test]
    fn test_aux_data_without_hash_error_variant() {
        let utxo_set = UtxoSet::new();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = Transaction::empty_with_hash(Hash32::from_bytes([2u8; 32]));
        tx.body.auxiliary_data_hash = None;
        tx.auxiliary_data = Some(AuxiliaryData {
            metadata: std::collections::BTreeMap::new(),
            native_scripts: vec![],
            plutus_v1_scripts: vec![],
            plutus_v2_scripts: vec![],
            plutus_v3_scripts: vec![],
        });
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 200, None);
        assert!(result.is_err(), "Data without hash must be rejected");
        if let Err(errors) = result {
            assert!(
                errors
                    .iter()
                    .any(|e| matches!(e, ValidationError::AuxiliaryDataWithoutHash)),
                "Expected AuxiliaryDataWithoutHash error"
            );
        }
    }

    /// Both auxiliary data and hash absent → valid (no error from this rule).
    #[test]
    fn test_auxiliary_data_both_absent_ok() {
        let utxo_set = UtxoSet::new();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = Transaction::empty_with_hash(Hash32::from_bytes([3u8; 32]));
        tx.body.auxiliary_data_hash = None;
        tx.auxiliary_data = None;
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 200, None);
        // May fail for other reasons (no inputs), but should NOT have aux data errors
        if let Err(errors) = result {
            assert!(
                !errors.iter().any(|e| matches!(
                    e,
                    ValidationError::AuxiliaryDataHashWithoutData
                        | ValidationError::AuxiliaryDataWithoutHash
                )),
                "No auxiliary data errors expected when both absent"
            );
        }
    }

    /// Empty PostAlonzo aux data (empty metadata map + no scripts) with hash → valid.
    #[test]
    fn test_auxiliary_data_empty_post_alonzo() {
        let utxo_set = UtxoSet::new();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = Transaction::empty_with_hash(Hash32::from_bytes([4u8; 32]));
        // Empty aux data (like tag(259){})
        let empty_aux = AuxiliaryData {
            metadata: std::collections::BTreeMap::new(),
            native_scripts: vec![],
            plutus_v1_scripts: vec![],
            plutus_v2_scripts: vec![],
            plutus_v3_scripts: vec![],
        };
        tx.auxiliary_data = Some(empty_aux);
        tx.body.auxiliary_data_hash = Some(Hash32::from_bytes([0xCD; 32]));
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 200, None);
        // Should NOT have AuxiliaryDataHashWithoutData or AuxiliaryDataWithoutHash
        if let Err(errors) = result {
            assert!(
                !errors.iter().any(|e| matches!(
                    e,
                    ValidationError::AuxiliaryDataHashWithoutData
                        | ValidationError::AuxiliaryDataWithoutHash
                )),
                "Empty PostAlonzo aux data with hash should pass consistency check"
            );
        }
    }

    /// Script-only aux data (no metadata, only scripts) with hash → valid consistency.
    #[test]
    fn test_auxiliary_data_script_only() {
        let utxo_set = UtxoSet::new();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = Transaction::empty_with_hash(Hash32::from_bytes([5u8; 32]));
        let script_only_aux = AuxiliaryData {
            metadata: std::collections::BTreeMap::new(),
            native_scripts: vec![],
            plutus_v1_scripts: vec![vec![0x01, 0x02, 0x03]],
            plutus_v2_scripts: vec![],
            plutus_v3_scripts: vec![],
        };
        tx.auxiliary_data = Some(script_only_aux);
        tx.body.auxiliary_data_hash = Some(Hash32::from_bytes([0xEF; 32]));
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 200, None);
        if let Err(errors) = result {
            assert!(
                !errors.iter().any(|e| matches!(
                    e,
                    ValidationError::AuxiliaryDataHashWithoutData
                        | ValidationError::AuxiliaryDataWithoutHash
                )),
                "Script-only aux data with hash should pass consistency check"
            );
        }
    }

    /// Mixed metadata + script aux data with hash → valid consistency.
    #[test]
    fn test_auxiliary_data_mixed_metadata_and_scripts() {
        use torsten_primitives::transaction::TransactionMetadatum;

        let utxo_set = UtxoSet::new();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = Transaction::empty_with_hash(Hash32::from_bytes([6u8; 32]));
        let mut metadata = std::collections::BTreeMap::new();
        metadata.insert(674, TransactionMetadatum::Text("test".to_string()));
        let mixed_aux = AuxiliaryData {
            metadata,
            native_scripts: vec![],
            plutus_v1_scripts: vec![],
            plutus_v2_scripts: vec![vec![0xAA, 0xBB]],
            plutus_v3_scripts: vec![],
        };
        tx.auxiliary_data = Some(mixed_aux);
        tx.body.auxiliary_data_hash = Some(Hash32::from_bytes([0xAB; 32]));
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 200, None);
        if let Err(errors) = result {
            assert!(
                !errors.iter().any(|e| matches!(
                    e,
                    ValidationError::AuxiliaryDataHashWithoutData
                        | ValidationError::AuxiliaryDataWithoutHash
                )),
                "Mixed metadata+script aux data with hash should pass consistency check"
            );
        }
    }

    // ===================================================================
    //  Issue #98: Within-block reference script resolution and fee counting
    //  (spending inputs with script_ref contribute to txNonDistinctRefScriptsSize)
    // ===================================================================

    /// Verify that `calculate_ref_script_size` counts scripts from BOTH spending
    /// inputs and reference inputs, matching Haskell's `txNonDistinctRefScriptsSize`.
    ///
    /// Before the fix, the function only iterated `reference_inputs` so any
    /// `script_ref` carried on a spending-input UTxO was silently ignored.
    #[test]
    fn test_ref_script_size_counts_spending_inputs() {
        use super::super::scripts::calculate_ref_script_size;

        let mut utxo_set = UtxoSet::new();
        let script_bytes: Vec<u8> = vec![0xDE, 0xAD, 0xBE, 0xEF]; // 4-byte mock Plutus script

        // Create a spending input whose UTxO carries a PlutusV2 script_ref (4 bytes).
        let spending_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x11u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            spending_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: Some(ScriptRef::PlutusV2(script_bytes.clone())),
                is_legacy: false,
                raw_cbor: None,
            },
        );

        // Spending input only — should count the script_ref from the spending UTxO.
        let size_spend_only =
            calculate_ref_script_size(std::slice::from_ref(&spending_input), &[], &utxo_set);
        assert_eq!(
            size_spend_only,
            script_bytes.len() as u64,
            "calculate_ref_script_size must count script_ref from spending inputs"
        );

        // Add a separate reference input with a different script (4 bytes).
        let ref_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x22u8; 32]),
            index: 0,
        };
        let ref_script_bytes: Vec<u8> = vec![0x01, 0x02, 0x03, 0x04]; // 4-byte V1 script
        utxo_set.insert(
            ref_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(2_000_000),
                datum: OutputDatum::None,
                script_ref: Some(ScriptRef::PlutusV1(ref_script_bytes.clone())),
                is_legacy: false,
                raw_cbor: None,
            },
        );

        // Both spending input AND reference input — total must be the sum of both script sizes.
        let size_both = calculate_ref_script_size(
            std::slice::from_ref(&spending_input),
            std::slice::from_ref(&ref_input),
            &utxo_set,
        );
        assert_eq!(
            size_both,
            script_bytes.len() as u64 + ref_script_bytes.len() as u64,
            "calculate_ref_script_size must sum scripts from both inputs and reference_inputs"
        );

        // Neither input — should be zero.
        let size_empty = calculate_ref_script_size(&[], &[], &utxo_set);
        assert_eq!(size_empty, 0, "Empty inputs must yield size 0");
    }

    /// Verify that `compute_min_fee` includes script_ref bytes from SPENDING inputs
    /// in the tiered reference-script fee component.
    ///
    /// A transaction that SPENDS a UTxO carrying a PlutusV2 script_ref must pay a
    /// tiered fee for those script bytes, even though the script is not in
    /// `reference_inputs`.  The fix adds `tx.body.inputs` to the size scan.
    #[test]
    fn test_min_fee_includes_spending_input_script_ref() {
        let mut utxo_set = UtxoSet::new();

        // 500-byte PlutusV2 script embedded in the spending-input UTxO.
        let script_bytes: Vec<u8> = vec![0xABu8; 500];

        let spending_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x33u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            spending_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: Some(ScriptRef::PlutusV2(script_bytes)),
                is_legacy: false,
                raw_cbor: None,
            },
        );

        // Build a minimal transaction spending that input (no reference inputs).
        let tx = make_simple_tx(spending_input, 9_500_000, 200_000);
        // No reference_inputs — the script_ref lives on the spending UTxO.
        assert!(tx.body.reference_inputs.is_empty());

        let params = ProtocolParameters::mainnet_defaults();
        let fee_with_ref = compute_min_fee(&tx, &utxo_set, &params, 300).0;

        // For comparison, compute the fee as if the spending input had no script_ref.
        let mut utxo_set_no_script = UtxoSet::new();
        let plain_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x44u8; 32]),
            index: 0,
        };
        utxo_set_no_script.insert(
            plain_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let tx_plain = make_simple_tx(plain_input, 9_500_000, 200_000);
        let fee_without_ref = compute_min_fee(&tx_plain, &utxo_set_no_script, &params, 300).0;

        assert!(
            fee_with_ref > fee_without_ref,
            "Min fee must be higher when the spending input carries a script_ref: \
             fee_with_ref={fee_with_ref}, fee_without_ref={fee_without_ref}"
        );
    }

    /// Verify that a minting transaction can succeed when the minting policy script
    /// lives in a UTxO that was produced by a PRIOR transaction in the same block.
    ///
    /// This is the within-block reference-script scenario: tx[0] produces a UTxO
    /// with a `script_ref`; tx[1] uses that UTxO as a `reference_input` to satisfy
    /// the minting policy check in Rule 3c.  The sequential `apply_block` loop
    /// ensures `self.utxo_set` contains tx[0]'s outputs when tx[1] is validated.
    ///
    /// We test the underlying mechanism directly: insert the script-bearing UTxO into
    /// the UTxO set before running validation, confirming the lookup succeeds.
    #[test]
    fn test_within_block_ref_script_for_minting_resolution() {
        let mut utxo_set = UtxoSet::new();

        // The minting policy native script.
        let signer_hash = Hash32::from_bytes([0x7Fu8; 32]);
        let native_script = NativeScript::ScriptPubkey(signer_hash);
        let script_hash = compute_script_ref_hash(&ScriptRef::NativeScript(native_script.clone()));

        // Spending input — the UTxO that the minting transaction actually spends.
        let spending_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xAAu8; 32]),
            index: 0,
        };
        utxo_set.insert(
            spending_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        // The script-bearing UTxO — this simulates a UTxO created by a prior tx in
        // the same block and subsequently visible via `self.utxo_set`.
        let script_utxo_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xBBu8; 32]),
            index: 0,
        };
        utxo_set.insert(
            script_utxo_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(2_000_000),
                datum: OutputDatum::None,
                script_ref: Some(ScriptRef::NativeScript(native_script.clone())),
                is_legacy: false,
                raw_cbor: None,
            },
        );

        // Build the minting transaction: mint 10 tokens under the policy,
        // use the script UTxO as a reference input.
        let asset = AssetName(b"MINTED".to_vec());
        let mut mint_map: BTreeMap<torsten_primitives::hash::PolicyId, BTreeMap<AssetName, i64>> =
            BTreeMap::new();
        mint_map
            .entry(script_hash)
            .or_default()
            .insert(asset.clone(), 10);

        let mut tx = make_simple_tx(spending_input, 9_800_000, 200_000);
        tx.body.mint = mint_map;
        tx.body.reference_inputs = vec![script_utxo_input];
        // Add the minted tokens to the output so multi-asset conservation holds.
        tx.body.outputs[0]
            .value
            .multi_asset
            .entry(script_hash)
            .or_default()
            .insert(asset, 10);

        let params = ProtocolParameters::mainnet_defaults();
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);

        assert!(
            result.is_ok(),
            "Minting with a reference-input script_ref from a within-block UTxO must succeed; \
             errors: {:?}",
            result.err()
        );
    }

    /// Mirror of `test_within_block_ref_script_for_minting_resolution` but for
    /// the failure case: the minting policy is NOT available (script_ref not found
    /// in any witness or reference input) → `InvalidMint` must be returned.
    #[test]
    fn test_minting_without_available_script_fails_with_invalid_mint() {
        let mut utxo_set = UtxoSet::new();

        // Spending input — no script_ref.
        let spending_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xCCu8; 32]),
            index: 0,
        };
        utxo_set.insert(
            spending_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        // Use an arbitrary policy ID that has no backing script anywhere.
        let phantom_policy = Hash28::from_bytes([0xFFu8; 28]);
        let asset = AssetName(b"GHOST".to_vec());
        let mut mint_map: BTreeMap<torsten_primitives::hash::PolicyId, BTreeMap<AssetName, i64>> =
            BTreeMap::new();
        mint_map
            .entry(phantom_policy)
            .or_default()
            .insert(asset.clone(), 5);

        let mut tx = make_simple_tx(spending_input, 9_800_000, 200_000);
        tx.body.mint = mint_map;
        // Add the minted tokens to the output so multi-asset conservation holds.
        tx.body.outputs[0]
            .value
            .multi_asset
            .entry(phantom_policy)
            .or_default()
            .insert(asset, 5);

        let params = ProtocolParameters::mainnet_defaults();
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);

        assert!(
            result
                .as_ref()
                .err()
                .map(|errs| errs
                    .iter()
                    .any(|e| matches!(e, ValidationError::InvalidMint)))
                .unwrap_or(false),
            "Minting without any matching script must produce InvalidMint; result: {:?}",
            result
        );
    }

    /// Verify that `calculate_ref_script_size` correctly handles the non-distinct
    /// (no deduplication) counting when the same script_ref appears in multiple UTxOs.
    ///
    /// Haskell: `txNonDistinctRefScriptsSize` — "non-distinct" means each UTxO
    /// contributes its script size independently even if two UTxOs carry identical
    /// script bytes.
    #[test]
    fn test_ref_script_size_non_distinct_no_dedup() {
        use super::super::scripts::calculate_ref_script_size;

        let script_bytes: Vec<u8> = vec![0x01u8; 100]; // 100-byte script

        let mut utxo_set = UtxoSet::new();

        // Two inputs that carry the SAME 100-byte Plutus V2 script.
        let input_a = TransactionInput {
            transaction_id: Hash32::from_bytes([0x10u8; 32]),
            index: 0,
        };
        let input_b = TransactionInput {
            transaction_id: Hash32::from_bytes([0x20u8; 32]),
            index: 0,
        };
        for inp in [input_a.clone(), input_b.clone()] {
            utxo_set.insert(
                inp,
                TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(1_000_000),
                    datum: OutputDatum::None,
                    script_ref: Some(ScriptRef::PlutusV2(script_bytes.clone())),
                    is_legacy: false,
                    raw_cbor: None,
                },
            );
        }

        // Both inputs carry the same 100-byte script → total must be 200 (not 100).
        let size = calculate_ref_script_size(&[input_a], &[input_b], &utxo_set);
        assert_eq!(
            size, 200,
            "Non-distinct counting: identical scripts in two UTxOs must each contribute \
             their full byte size (2 × 100 = 200)"
        );
    }

    // =========================================================================
    // Bug C5: Collateral percentage ceiling division
    // =========================================================================

    /// Bug C5: Haskell uses ceiling(fee * pct / 100), not truncating division.
    ///
    /// fee=101, pct=150: exact = 151.5 → required = 152 (ceiling), not 151 (truncating).
    /// A collateral of exactly 151 lovelace should be rejected; 152 should pass.
    #[test]
    fn test_collateral_minimum_exact_ceiling() {
        use super::super::collateral::check_collateral;

        let mut utxo_set = UtxoSet::new();

        // A collateral UTxO that provides exactly the truncated value (151), which
        // is one less than the correct ceiling (152).  Must be rejected.
        let col_input_low = TransactionInput {
            transaction_id: Hash32::from_bytes([0xC1; 32]),
            index: 0,
        };
        utxo_set.insert(
            col_input_low.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                // Exactly 151 — truncating division would (wrongly) accept this.
                value: Value::lovelace(151),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        // A collateral UTxO that provides exactly the ceiling (152).  Must pass.
        let col_input_ok = TransactionInput {
            transaction_id: Hash32::from_bytes([0xC2; 32]),
            index: 0,
        };
        utxo_set.insert(
            col_input_ok.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(152),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        // Build a minimal protocol parameters struct with the test values.
        let mut params = ProtocolParameters::mainnet_defaults();
        params.collateral_percentage = 150;

        // Build a minimal transaction body with fee = 101.
        // We only need the fields read by check_collateral; the rest are ignored.
        let spend_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xA0; 32]),
            index: 0,
        };
        let make_tx = |col: TransactionInput| -> Transaction {
            let mut tx = make_simple_tx(spend_input.clone(), 0, 101);
            tx.body.collateral = vec![col];
            tx
        };

        // --- collateral = 151 (truncating would accept; ceiling must reject) ---
        let tx_low = make_tx(col_input_low);
        let mut errors_low: Vec<ValidationError> = Vec::new();
        check_collateral(&tx_low, &utxo_set, &params, &mut errors_low);
        assert!(
            errors_low
                .iter()
                .any(|e| matches!(e, ValidationError::InsufficientCollateral)),
            "Collateral of 151 should fail with ceiling(101*150/100)=152: {errors_low:?}"
        );

        // --- collateral = 152 (exactly the ceiling; must pass) ---
        let tx_ok = make_tx(col_input_ok);
        let mut errors_ok: Vec<ValidationError> = Vec::new();
        check_collateral(&tx_ok, &utxo_set, &params, &mut errors_ok);
        let collateral_errors: Vec<_> = errors_ok
            .iter()
            .filter(|e| matches!(e, ValidationError::InsufficientCollateral))
            .collect();
        assert!(
            collateral_errors.is_empty(),
            "Collateral of 152 should satisfy ceiling(101*150/100)=152: {errors_ok:?}"
        );
    }

    /// Verify the formula handles exact multiples (no rounding required).
    /// fee=100, pct=150: exact = 150.0 → ceiling = 150.
    #[test]
    fn test_collateral_minimum_no_rounding_needed() {
        use super::super::collateral::check_collateral;

        let mut utxo_set = UtxoSet::new();
        let col_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xD1; 32]),
            index: 0,
        };
        utxo_set.insert(
            col_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(150),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let mut params = ProtocolParameters::mainnet_defaults();
        params.collateral_percentage = 150;
        let spend_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xA1; 32]),
            index: 0,
        };
        let mut tx = make_simple_tx(spend_input, 0, 100);
        tx.body.collateral = vec![col_input];
        let mut errors: Vec<ValidationError> = Vec::new();
        check_collateral(&tx, &utxo_set, &params, &mut errors);
        let insuff: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::InsufficientCollateral))
            .collect();
        assert!(
            insuff.is_empty(),
            "fee=100, pct=150, collateral=150: ceiling(150.0)=150 should pass: {errors:?}"
        );
    }

    // =========================================================================
    // Bug C5 extra: collateral_return interacts correctly with ceiling arithmetic
    // =========================================================================

    /// Verifies that collateral_return is subtracted before the ceiling check.
    ///
    /// col_input provides 300 lovelace, collateral_return takes 148 back,
    /// effective = 152.  With fee=101 and pct=150, ceiling(151.5)=152 must pass.
    /// If effective were 151 (e.g., return=149) the check must reject.
    #[test]
    fn test_collateral_return_reduces_effective_collateral_ceiling() {
        use super::super::collateral::check_collateral;

        let mut utxo_set = UtxoSet::new();
        let col_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xE0; 32]),
            index: 0,
        };
        utxo_set.insert(
            col_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(300),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let mut params = ProtocolParameters::mainnet_defaults();
        params.collateral_percentage = 150;

        let spend_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xA2; 32]),
            index: 0,
        };

        // effective = 300 - 148 = 152 → ceiling(101*150/100) = 152 → must pass
        let mut tx_pass = make_simple_tx(spend_input.clone(), 0, 101);
        tx_pass.body.collateral = vec![col_input.clone()];
        tx_pass.body.collateral_return = Some(TransactionOutput {
            address: Address::Byron(ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value::lovelace(148),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        });
        let mut errors_pass: Vec<ValidationError> = Vec::new();
        check_collateral(&tx_pass, &utxo_set, &params, &mut errors_pass);
        assert!(
            !errors_pass
                .iter()
                .any(|e| matches!(e, ValidationError::InsufficientCollateral)),
            "effective=152 should pass ceiling check: {errors_pass:?}"
        );

        // effective = 300 - 149 = 151 → ceiling(101*150/100) = 152 → must fail
        let mut tx_fail = make_simple_tx(spend_input, 0, 101);
        tx_fail.body.collateral = vec![col_input];
        tx_fail.body.collateral_return = Some(TransactionOutput {
            address: Address::Byron(ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value::lovelace(149),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        });
        let mut errors_fail: Vec<ValidationError> = Vec::new();
        check_collateral(&tx_fail, &utxo_set, &params, &mut errors_fail);
        assert!(
            errors_fail
                .iter()
                .any(|e| matches!(e, ValidationError::InsufficientCollateral)),
            "effective=151 should fail ceiling check: {errors_fail:?}"
        );
    }

    // =========================================================================
    // Bug C1: Missing Reward redeemer for script-locked withdrawals
    // =========================================================================

    /// Constructs a transaction that has a script-locked withdrawal but no
    /// matching Reward redeemer.  Must be rejected with MissingRedeemer { tag:
    /// "Reward", .. }.
    ///
    /// Reward address format: [0xF0, script_hash_bytes[0..28]]
    #[test]
    fn test_script_withdrawal_missing_reward_redeemer() {
        use super::super::collateral::check_script_redeemers;

        let script_hash = Hash28::from_bytes([0xCC; 28]);

        // Reward address: header 0xF0 = script stake credential (no network bit),
        // followed by the 28-byte script hash.
        let mut reward_addr = vec![0xF0u8];
        reward_addr.extend_from_slice(script_hash.as_bytes());

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x10; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        // Transaction with a script-locked withdrawal but NO Reward redeemer.
        let mut withdrawals = BTreeMap::new();
        withdrawals.insert(reward_addr, Lovelace(1_000_000));

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(9_000_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(0),
                ttl: None,
                certificates: vec![],
                withdrawals,
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
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![vec![0xAB]],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                // No Reward redeemer — this is the bug we are testing.
                redeemers: vec![],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors: Vec<ValidationError> = Vec::new();
        check_script_redeemers(&tx, &utxo_set, &mut errors);

        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingRedeemer { tag, index: 0 } if tag == "Reward")),
            "Expected MissingRedeemer {{ tag: Reward, index: 0 }}, got: {errors:?}"
        );
    }

    /// A script-locked withdrawal WITH a matching Reward redeemer must not
    /// produce a MissingRedeemer error.
    #[test]
    fn test_script_withdrawal_with_reward_redeemer_ok() {
        use super::super::collateral::check_script_redeemers;

        let script_hash = Hash28::from_bytes([0xCC; 28]);
        let mut reward_addr = vec![0xF0u8];
        reward_addr.extend_from_slice(script_hash.as_bytes());

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x11; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let mut withdrawals = BTreeMap::new();
        withdrawals.insert(reward_addr, Lovelace(1_000_000));

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(9_000_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(0),
                ttl: None,
                certificates: vec![],
                withdrawals,
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
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![vec![0xAB]],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                // Reward redeemer at index 0 — matches the single withdrawal.
                redeemers: vec![Redeemer {
                    tag: RedeemerTag::Reward,
                    index: 0,
                    data: PlutusData::Integer(0),
                    ex_units: ExUnits {
                        mem: 100,
                        steps: 100,
                    },
                }],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors: Vec<ValidationError> = Vec::new();
        check_script_redeemers(&tx, &utxo_set, &mut errors);

        assert!(
            !errors.iter().any(
                |e| matches!(e, ValidationError::MissingRedeemer { tag, .. } if tag == "Reward")
            ),
            "Reward redeemer present; must not produce MissingRedeemer: {errors:?}"
        );
    }

    // =========================================================================
    // Bug C1: Missing Mint redeemer for Plutus minting policies
    // =========================================================================

    /// A Plutus minting policy (V2 script in witness set) without a Mint redeemer
    /// must be rejected with MissingRedeemer { tag: "Mint", index: 0 }.
    #[test]
    fn test_minting_policy_missing_mint_redeemer() {
        use super::super::collateral::check_script_redeemers;

        // Create a fake Plutus V2 script and compute its policy hash.
        let script_bytes: Vec<u8> = vec![0x11, 0x22, 0x33];
        let policy_id = torsten_primitives::hash::blake2b_224_tagged(2, &script_bytes);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x20; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let asset = AssetName::new(b"MyToken".to_vec()).unwrap();
        let mut mint: BTreeMap<torsten_primitives::hash::PolicyId, BTreeMap<AssetName, i64>> =
            BTreeMap::new();
        mint.entry(policy_id).or_default().insert(asset, 100);

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(9_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
                ttl: None,
                certificates: vec![],
                withdrawals: BTreeMap::new(),
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint,
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
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                // The Plutus V2 script is present in the witness set.
                plutus_v2_scripts: vec![script_bytes],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                // No Mint redeemer — this is the bug we are testing.
                redeemers: vec![],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors: Vec<ValidationError> = Vec::new();
        check_script_redeemers(&tx, &utxo_set, &mut errors);

        assert!(
            errors.iter().any(
                |e| matches!(e, ValidationError::MissingRedeemer { tag, index: 0 } if tag == "Mint")
            ),
            "Expected MissingRedeemer {{ tag: Mint, index: 0 }}, got: {errors:?}"
        );
    }

    /// A Plutus minting policy WITH a matching Mint redeemer must not produce
    /// a MissingRedeemer error.
    #[test]
    fn test_minting_policy_with_mint_redeemer_ok() {
        use super::super::collateral::check_script_redeemers;

        let script_bytes: Vec<u8> = vec![0x11, 0x22, 0x33];
        let policy_id = torsten_primitives::hash::blake2b_224_tagged(2, &script_bytes);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x21; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let asset = AssetName::new(b"MyToken".to_vec()).unwrap();
        let mut mint: BTreeMap<torsten_primitives::hash::PolicyId, BTreeMap<AssetName, i64>> =
            BTreeMap::new();
        mint.entry(policy_id).or_default().insert(asset, 100);

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(9_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
                ttl: None,
                certificates: vec![],
                withdrawals: BTreeMap::new(),
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint,
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
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![script_bytes],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                // Mint redeemer at index 0 — matches the single minting policy.
                redeemers: vec![Redeemer {
                    tag: RedeemerTag::Mint,
                    index: 0,
                    data: PlutusData::Integer(0),
                    ex_units: ExUnits {
                        mem: 100,
                        steps: 100,
                    },
                }],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors: Vec<ValidationError> = Vec::new();
        check_script_redeemers(&tx, &utxo_set, &mut errors);

        assert!(
            !errors.iter().any(
                |e| matches!(e, ValidationError::MissingRedeemer { tag, .. } if tag == "Mint")
            ),
            "Mint redeemer present; must not produce MissingRedeemer: {errors:?}"
        );
    }

    /// Two Plutus V2 minting policies in sorted order.  The Mint redeemer at
    /// index 0 must map to the lexicographically-first policy ID, and index 1
    /// to the second.  Missing either redeemer must produce the correct index.
    #[test]
    fn test_mint_redeemer_sorted_index_order() {
        use super::super::collateral::check_script_redeemers;

        // Two different Plutus V2 scripts.  Their policy IDs will be sorted by
        // BTreeMap, so we must ensure the test acknowledges that order.
        let script_a: Vec<u8> = vec![0x01];
        let script_b: Vec<u8> = vec![0xFF];
        let policy_a = torsten_primitives::hash::blake2b_224_tagged(2, &script_a);
        let policy_b = torsten_primitives::hash::blake2b_224_tagged(2, &script_b);

        // Determine which policy comes first in BTreeMap order.
        let (policy_first, policy_second) = if policy_a < policy_b {
            (policy_a, policy_b)
        } else {
            (policy_b, policy_a)
        };

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x30; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let asset = AssetName::new(b"T".to_vec()).unwrap();
        let mut mint: BTreeMap<torsten_primitives::hash::PolicyId, BTreeMap<AssetName, i64>> =
            BTreeMap::new();
        mint.entry(policy_first)
            .or_default()
            .insert(asset.clone(), 1);
        mint.entry(policy_second).or_default().insert(asset, 1);

        // Only a redeemer for index 0 — the second policy (index 1) is missing.
        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(9_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
                ttl: None,
                certificates: vec![],
                withdrawals: BTreeMap::new(),
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint,
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
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![script_a, script_b],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                // Only the first policy has a redeemer.
                redeemers: vec![Redeemer {
                    tag: RedeemerTag::Mint,
                    index: 0,
                    data: PlutusData::Integer(0),
                    ex_units: ExUnits {
                        mem: 100,
                        steps: 100,
                    },
                }],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors: Vec<ValidationError> = Vec::new();
        check_script_redeemers(&tx, &utxo_set, &mut errors);

        // The second policy (index 1) is missing a Mint redeemer.
        assert!(
            errors.iter().any(
                |e| matches!(e, ValidationError::MissingRedeemer { tag, index: 1 } if tag == "Mint")
            ),
            "Expected MissingRedeemer {{ tag: Mint, index: 1 }}, got: {errors:?}"
        );
        // The first policy (index 0) has a redeemer and must not trigger an error.
        assert!(
            !errors.iter().any(
                |e| matches!(e, ValidationError::MissingRedeemer { tag, index: 0 } if tag == "Mint")
            ),
            "Index 0 redeemer present; must not trigger MissingRedeemer: {errors:?}"
        );
    }

    // =========================================================================
    // Bug C1: Two spending inputs — two different Plutus V2 scripts, both with
    // correct Spend redeemers at sorted indices (integration smoke test).
    // =========================================================================

    /// Two script-locked inputs, each locked by a different Plutus V2 script,
    /// both with Spend redeemers at the correct sorted indices.  check_script_redeemers
    /// must not produce any MissingSpendRedeemer or MissingRedeemer error.
    #[test]
    fn test_multi_script_two_spending_inputs() {
        use super::super::collateral::check_script_redeemers;
        use torsten_primitives::address::EnterpriseAddress;
        use torsten_primitives::credentials::Credential;

        // Two fake Plutus V2 scripts with distinct hashes.
        let script_a: Vec<u8> = vec![0xA1, 0xA2];
        let script_b: Vec<u8> = vec![0xB1, 0xB2];
        let hash_a = torsten_primitives::hash::blake2b_224_tagged(2, &script_a);
        let hash_b = torsten_primitives::hash::blake2b_224_tagged(2, &script_b);

        // Two inputs locked by the respective scripts.
        // After sorting by tx_id the order is input_a (0x01…) < input_b (0x02…).
        let input_a = TransactionInput {
            transaction_id: Hash32::from_bytes([0x01; 32]),
            index: 0,
        };
        let input_b = TransactionInput {
            transaction_id: Hash32::from_bytes([0x02; 32]),
            index: 0,
        };

        let mut utxo_set = UtxoSet::new();
        utxo_set.insert(
            input_a.clone(),
            TransactionOutput {
                address: Address::Enterprise(EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Testnet,
                    payment: Credential::Script(hash_a),
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        utxo_set.insert(
            input_b.clone(),
            TransactionOutput {
                address: Address::Enterprise(EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Testnet,
                    payment: Credential::Script(hash_b),
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input_a, input_b],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(9_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
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
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![script_a, script_b],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                // Spend redeemers for sorted index 0 (input_a) and index 1 (input_b).
                redeemers: vec![
                    Redeemer {
                        tag: RedeemerTag::Spend,
                        index: 0,
                        data: PlutusData::Integer(1),
                        ex_units: ExUnits {
                            mem: 100,
                            steps: 100,
                        },
                    },
                    Redeemer {
                        tag: RedeemerTag::Spend,
                        index: 1,
                        data: PlutusData::Integer(2),
                        ex_units: ExUnits {
                            mem: 100,
                            steps: 100,
                        },
                    },
                ],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors: Vec<ValidationError> = Vec::new();
        check_script_redeemers(&tx, &utxo_set, &mut errors);

        let redeemer_errors: Vec<_> = errors
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    ValidationError::MissingSpendRedeemer { .. }
                        | ValidationError::MissingRedeemer { .. }
                )
            })
            .collect();
        assert!(
            redeemer_errors.is_empty(),
            "Both Spend redeemers present; expected no errors, got: {errors:?}"
        );
    }

    // =========================================================================
    // Bug C3: PlutusV3 non-Unit return value validation (plutus.rs tests)
    // =========================================================================

    // Note: These tests exercise the PlutusV3 Unit-return check indirectly by
    // verifying the `plutus_script_version_map` and `has_any_v3` detection logic
    // in isolation, since constructing a fully-evaluated Plutus V3 transaction
    // requires a valid on-chain CBOR encoding that is not practical to fabricate
    // in a unit test.
    //
    // The full end-to-end V3 behavior is covered by integration tests against
    // the preview testnet (see tests/reward_cross_validation.rs for the pattern).

    /// Verifies that `plutus_script_version_map` correctly identifies V3 scripts
    /// and returns version tag 3 for each.
    #[test]
    fn test_plutus_script_version_map_v3_detection() {
        use super::super::collateral::plutus_script_version_map;

        let script_v1: Vec<u8> = vec![0x11];
        let script_v2: Vec<u8> = vec![0x22];
        let script_v3: Vec<u8> = vec![0x33];

        let hash_v1 = torsten_primitives::hash::blake2b_224_tagged(1, &script_v1);
        let hash_v2 = torsten_primitives::hash::blake2b_224_tagged(2, &script_v2);
        let hash_v3 = torsten_primitives::hash::blake2b_224_tagged(3, &script_v3);

        let mut utxo_set = UtxoSet::new();
        // Minimal tx: one key-locked input, no reference inputs.
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x50; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![],
                fee: Lovelace(0),
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
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![script_v1],
                plutus_v2_scripts: vec![script_v2],
                plutus_v3_scripts: vec![script_v3],
                plutus_data: vec![],
                redeemers: vec![],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let map = plutus_script_version_map(&tx, &utxo_set);

        assert_eq!(
            map.get(&hash_v1),
            Some(&1u8),
            "V1 script should map to version 1"
        );
        assert_eq!(
            map.get(&hash_v2),
            Some(&2u8),
            "V2 script should map to version 2"
        );
        assert_eq!(
            map.get(&hash_v3),
            Some(&3u8),
            "V3 script should map to version 3"
        );

        // Verify the has_any_v3 condition used by evaluate_plutus_scripts.
        let has_any_v3 = map.values().any(|v| *v == 3);
        assert!(
            has_any_v3,
            "has_any_v3 must be true when V3 scripts are present"
        );

        // Without V3 scripts, the flag must be false.
        let mut tx_no_v3 = tx.clone();
        tx_no_v3.witness_set.plutus_v3_scripts.clear();
        let map_no_v3 = plutus_script_version_map(&tx_no_v3, &utxo_set);
        let has_v3 = map_no_v3.values().any(|v| *v == 3);
        assert!(
            !has_v3,
            "has_any_v3 must be false when no V3 scripts are present"
        );
    }

    /// When only V1/V2 scripts are present, `plutus_script_version_map` must
    /// return no version-3 entries — the strict Unit check must not fire.
    #[test]
    fn test_plutus_script_version_map_no_v3_no_strict_check() {
        use super::super::collateral::plutus_script_version_map;

        let script_v2: Vec<u8> = vec![0xBB];
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x60; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![],
                fee: Lovelace(0),
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
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![script_v2],
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
        };

        let map = plutus_script_version_map(&tx, &utxo_set);
        let has_any_v3 = map.values().any(|v| *v == 3);
        assert!(
            !has_any_v3,
            "No V3 scripts present; strict Unit check must not fire: {map:?}"
        );
    }

    // =========================================================================
    // Issue #131 Item 1: Reference script Phase-2 path
    //
    // Verify that check_script_redeemers and collect_plutus_script_hashes
    // correctly handle the case where the Plutus script body lives in a
    // reference input UTxO (script_ref) rather than the witness set.  A
    // Spend redeemer for a script-locked input must be required, and when the
    // Spend redeemer IS present (with the Plutus hash reachable via the
    // reference input), no MissingSpendRedeemer error should fire.
    // =========================================================================

    /// When a Plutus V2 script body lives in a reference input UTxO (as
    /// `script_ref = PlutusV2(bytes)`) and a spending input is locked by the
    /// corresponding script hash, `check_script_redeemers` must require a Spend
    /// redeemer for that input.
    ///
    /// This confirms the Phase-2 reference-script path is wired in correctly:
    /// `collect_plutus_script_hashes` scans reference input UTxOs and registers
    /// the script hash, so the missing-redeemer check applies to it.
    #[test]
    fn test_ref_script_phase2_missing_spend_redeemer() {
        use super::super::collateral::check_script_redeemers;
        use torsten_primitives::address::EnterpriseAddress;
        use torsten_primitives::credentials::Credential;
        use torsten_primitives::transaction::ScriptRef;

        // A fake Plutus V2 script stored in a reference input UTxO.
        let plutus_script_bytes: Vec<u8> = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let script_hash = torsten_primitives::hash::blake2b_224_tagged(2, &plutus_script_bytes);

        let mut utxo_set = UtxoSet::new();

        // Spending input locked by the script hash.
        let spend_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x70; 32]),
            index: 0,
        };
        utxo_set.insert(
            spend_input.clone(),
            TransactionOutput {
                address: Address::Enterprise(EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Testnet,
                    payment: Credential::Script(script_hash),
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        // Reference input UTxO carrying the Plutus V2 script body.
        let ref_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x71; 32]),
            index: 0,
        };
        utxo_set.insert(
            ref_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(2_000_000),
                datum: OutputDatum::None,
                // The Plutus script body lives here — not in the witness set.
                script_ref: Some(ScriptRef::PlutusV2(plutus_script_bytes)),
                is_legacy: false,
                raw_cbor: None,
            },
        );

        // Transaction: spending input locked by script hash, reference input
        // provides the script body.  NO redeemers — this is what we are testing.
        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![spend_input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(4_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
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
                reference_inputs: vec![ref_input],
                update: None,
                voting_procedures: BTreeMap::new(),
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                // Script body NOT in witness set — it's in the reference input.
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                // No redeemers — must trigger MissingSpendRedeemer.
                redeemers: vec![],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors: Vec<ValidationError> = Vec::new();
        check_script_redeemers(&tx, &utxo_set, &mut errors);

        // The spending input is locked by a Plutus script whose body is provided
        // via a reference input.  A Spend redeemer is still required — the check
        // must fire even when the script is not in the witness set.
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingSpendRedeemer { index: 0 })),
            "Expected MissingSpendRedeemer {{ index: 0 }} for ref-script-locked input, got: {errors:?}"
        );
    }

    /// When a Plutus V2 script body lives in a reference input UTxO and the
    /// transaction includes a matching Spend redeemer, `check_script_redeemers`
    /// must NOT produce a MissingSpendRedeemer error.
    ///
    /// This is the "happy path" for reference-script Phase-2 execution: the
    /// script is provided via a reference input, the Spend redeemer is present,
    /// and the redeemer-check logic correctly accepts the transaction.
    #[test]
    fn test_ref_script_phase2_with_spend_redeemer_ok() {
        use super::super::collateral::check_script_redeemers;
        use torsten_primitives::address::EnterpriseAddress;
        use torsten_primitives::credentials::Credential;
        use torsten_primitives::transaction::ScriptRef;

        let plutus_script_bytes: Vec<u8> = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let script_hash = torsten_primitives::hash::blake2b_224_tagged(2, &plutus_script_bytes);

        let mut utxo_set = UtxoSet::new();

        let spend_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x72; 32]),
            index: 0,
        };
        utxo_set.insert(
            spend_input.clone(),
            TransactionOutput {
                address: Address::Enterprise(EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Testnet,
                    payment: Credential::Script(script_hash),
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let ref_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x73; 32]),
            index: 0,
        };
        utxo_set.insert(
            ref_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(2_000_000),
                datum: OutputDatum::None,
                script_ref: Some(ScriptRef::PlutusV2(plutus_script_bytes)),
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![spend_input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(4_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
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
                reference_inputs: vec![ref_input],
                update: None,
                voting_procedures: BTreeMap::new(),
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
                // Spend redeemer at index 0 — matches the single script-locked input.
                redeemers: vec![Redeemer {
                    tag: RedeemerTag::Spend,
                    index: 0,
                    data: PlutusData::Integer(0),
                    ex_units: ExUnits {
                        mem: 100,
                        steps: 100,
                    },
                }],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors: Vec<ValidationError> = Vec::new();
        check_script_redeemers(&tx, &utxo_set, &mut errors);

        let spend_errors: Vec<_> = errors
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    ValidationError::MissingSpendRedeemer { .. }
                        | ValidationError::MissingRedeemer { .. }
                )
            })
            .collect();
        assert!(
            spend_errors.is_empty(),
            "Spend redeemer present for ref-script path; expected no redeemer errors, got: {errors:?}"
        );
    }

    // =========================================================================
    // Issue #131 Item 2: Certificate redeemers (Cert tag)
    //
    // Per Haskell's `conwayCertsNeeded`, every certificate whose relevant
    // credential is a script hash requires a Cert redeemer at the certificate's
    // 0-based positional index in the body's certificate list.
    // =========================================================================

    /// A `ConwayStakeDeregistration` certificate with a Script credential and
    /// no Cert redeemer must be rejected with MissingRedeemer { tag: "Cert",
    /// index: 0 }.
    #[test]
    fn test_cert_redeemer_conway_deregistration_missing() {
        use super::super::collateral::check_script_redeemers;
        use torsten_primitives::credentials::Credential;
        use torsten_primitives::transaction::Certificate;
        use torsten_primitives::value::Lovelace;

        let script_hash = Hash28::from_bytes([0xD0; 28]);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x80; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(4_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
                ttl: None,
                // Conway deregistration with a Script credential — Cert redeemer required.
                certificates: vec![Certificate::ConwayStakeDeregistration {
                    credential: Credential::Script(script_hash),
                    refund: Lovelace(2_000_000),
                }],
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
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![vec![0xAA]],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                // No Cert redeemer — this is what we are testing.
                redeemers: vec![],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors: Vec<ValidationError> = Vec::new();
        check_script_redeemers(&tx, &utxo_set, &mut errors);

        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingRedeemer { tag, index: 0 } if tag == "Cert")),
            "Expected MissingRedeemer {{ tag: Cert, index: 0 }} for ConwayStakeDeregistration with Script credential, got: {errors:?}"
        );
    }

    /// A `ConwayStakeDeregistration` with a Script credential and a matching
    /// Cert redeemer at index 0 must NOT produce a MissingRedeemer error.
    #[test]
    fn test_cert_redeemer_conway_deregistration_present_ok() {
        use super::super::collateral::check_script_redeemers;
        use torsten_primitives::credentials::Credential;
        use torsten_primitives::transaction::Certificate;
        use torsten_primitives::value::Lovelace;

        let script_hash = Hash28::from_bytes([0xD1; 28]);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x81; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(4_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
                ttl: None,
                certificates: vec![Certificate::ConwayStakeDeregistration {
                    credential: Credential::Script(script_hash),
                    refund: Lovelace(2_000_000),
                }],
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
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![vec![0xAA]],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                // Cert redeemer at index 0 — matches the certificate.
                redeemers: vec![Redeemer {
                    tag: RedeemerTag::Cert,
                    index: 0,
                    data: PlutusData::Integer(0),
                    ex_units: ExUnits {
                        mem: 100,
                        steps: 100,
                    },
                }],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors: Vec<ValidationError> = Vec::new();
        check_script_redeemers(&tx, &utxo_set, &mut errors);

        assert!(
            !errors.iter().any(
                |e| matches!(e, ValidationError::MissingRedeemer { tag, .. } if tag == "Cert")
            ),
            "Cert redeemer present; must not produce MissingRedeemer: {errors:?}"
        );
    }

    /// A `StakeDeregistration` (pre-Conway) with a Script credential and no
    /// Cert redeemer must be rejected.  Pre-Conway deregistration also requires
    /// a Cert redeemer when the credential is a script hash.
    #[test]
    fn test_cert_redeemer_pre_conway_deregistration_missing() {
        use super::super::collateral::check_script_redeemers;
        use torsten_primitives::credentials::Credential;
        use torsten_primitives::transaction::Certificate;

        let script_hash = Hash28::from_bytes([0xD2; 28]);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x82; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(4_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
                ttl: None,
                certificates: vec![Certificate::StakeDeregistration(Credential::Script(
                    script_hash,
                ))],
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
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![vec![0xBB]],
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
        };

        let mut errors: Vec<ValidationError> = Vec::new();
        check_script_redeemers(&tx, &utxo_set, &mut errors);

        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingRedeemer { tag, index: 0 } if tag == "Cert")),
            "Expected MissingRedeemer {{ tag: Cert, index: 0 }} for StakeDeregistration with Script credential, got: {errors:?}"
        );
    }

    /// A `StakeRegistration` with a Script credential must NOT require a Cert
    /// redeemer — registrations are always permitted without a script witness.
    #[test]
    fn test_cert_redeemer_registration_no_redeemer_required() {
        use super::super::collateral::check_script_redeemers;
        use torsten_primitives::credentials::Credential;
        use torsten_primitives::transaction::Certificate;

        let script_hash = Hash28::from_bytes([0xD3; 28]);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x83; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(4_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
                ttl: None,
                // Registration does not need a redeemer even with a Script credential.
                certificates: vec![Certificate::StakeRegistration(Credential::Script(
                    script_hash,
                ))],
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
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![vec![0xCC]],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                // No Cert redeemer — registration must not require one.
                redeemers: vec![],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors: Vec<ValidationError> = Vec::new();
        check_script_redeemers(&tx, &utxo_set, &mut errors);

        assert!(
            !errors.iter().any(
                |e| matches!(e, ValidationError::MissingRedeemer { tag, .. } if tag == "Cert")
            ),
            "StakeRegistration must not require a Cert redeemer, got: {errors:?}"
        );
    }

    /// A `StakeDeregistration` with a VerificationKey credential must NOT
    /// require a Cert redeemer — only Script credentials need redeemers.
    #[test]
    fn test_cert_redeemer_key_credential_no_redeemer_required() {
        use super::super::collateral::check_script_redeemers;
        use torsten_primitives::credentials::Credential;
        use torsten_primitives::transaction::Certificate;

        let key_hash = Hash28::from_bytes([0xD4; 28]);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x84; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(4_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
                ttl: None,
                // Deregistration with VKey credential — redeemer not needed.
                certificates: vec![Certificate::StakeDeregistration(
                    Credential::VerificationKey(key_hash),
                )],
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
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                // No Cert redeemer — VKey credential must not require one.
                redeemers: vec![],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors: Vec<ValidationError> = Vec::new();
        check_script_redeemers(&tx, &utxo_set, &mut errors);

        assert!(
            !errors.iter().any(
                |e| matches!(e, ValidationError::MissingRedeemer { tag, .. } if tag == "Cert")
            ),
            "VerificationKey credential must not require a Cert redeemer, got: {errors:?}"
        );
    }

    /// Three certificates in order:
    ///   0: StakeRegistration(Script)  — no redeemer required
    ///   1: ConwayStakeDeregistration(Script)  — Cert redeemer at index 1 required
    ///   2: StakeDelegation(Script)  — Cert redeemer at index 2 required
    ///
    /// Only the redeemer for index 1 is provided.  The check must fire for
    /// index 2 only, and must NOT fire for index 0 or index 1.
    #[test]
    fn test_cert_redeemer_positional_index_mixed_certs() {
        use super::super::collateral::check_script_redeemers;
        use torsten_primitives::credentials::Credential;
        use torsten_primitives::transaction::Certificate;
        use torsten_primitives::value::Lovelace;

        let hash_a = Hash28::from_bytes([0xD5; 28]);
        let hash_b = Hash28::from_bytes([0xD6; 28]);
        let pool_hash = Hash28::from_bytes([0xD7; 28]);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x85; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(9_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
                ttl: None,
                certificates: vec![
                    // Index 0: registration — no redeemer required even for Script.
                    Certificate::StakeRegistration(Credential::Script(hash_a)),
                    // Index 1: deregistration with Script — Cert redeemer at index 1 needed.
                    Certificate::ConwayStakeDeregistration {
                        credential: Credential::Script(hash_a),
                        refund: Lovelace(2_000_000),
                    },
                    // Index 2: delegation with Script — Cert redeemer at index 2 needed.
                    Certificate::StakeDelegation {
                        credential: Credential::Script(hash_b),
                        pool_hash,
                    },
                ],
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
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![vec![0xDD]],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                // Only cert index 1 has a redeemer.  Index 2 is missing.
                redeemers: vec![Redeemer {
                    tag: RedeemerTag::Cert,
                    index: 1,
                    data: PlutusData::Integer(0),
                    ex_units: ExUnits {
                        mem: 100,
                        steps: 100,
                    },
                }],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors: Vec<ValidationError> = Vec::new();
        check_script_redeemers(&tx, &utxo_set, &mut errors);

        // Index 2 (StakeDelegation with Script) must fire.
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingRedeemer { tag, index: 2 } if tag == "Cert")),
            "Expected MissingRedeemer {{ tag: Cert, index: 2 }} for StakeDelegation, got: {errors:?}"
        );

        // Index 0 (StakeRegistration) must NOT fire.
        assert!(
            !errors.iter().any(
                |e| matches!(e, ValidationError::MissingRedeemer { tag, index: 0 } if tag == "Cert")
            ),
            "StakeRegistration at index 0 must not require a Cert redeemer, got: {errors:?}"
        );

        // Index 1 (ConwayStakeDeregistration) has a redeemer — must NOT fire.
        assert!(
            !errors.iter().any(
                |e| matches!(e, ValidationError::MissingRedeemer { tag, index: 1 } if tag == "Cert")
            ),
            "Index 1 has a Cert redeemer; must not trigger MissingRedeemer, got: {errors:?}"
        );
    }

    /// An `UnregDRep` certificate with a Script credential and no Cert redeemer
    /// must be rejected.  `RegDRep` with the same credential must NOT require
    /// a redeemer (DRep registration is one-sided, like stake registration).
    #[test]
    fn test_cert_redeemer_drep_unreg_requires_cert_reg_does_not() {
        use super::super::collateral::check_script_redeemers;
        use torsten_primitives::credentials::Credential;
        use torsten_primitives::transaction::Certificate;
        use torsten_primitives::value::Lovelace;

        let drep_script_hash = Hash28::from_bytes([0xD8; 28]);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x86; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        // --- Part A: RegDRep with Script credential — no redeemer required ---

        let tx_reg = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input.clone()],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(4_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
                ttl: None,
                // DRep registration — no redeemer required.
                certificates: vec![Certificate::RegDRep {
                    credential: Credential::Script(drep_script_hash),
                    deposit: Lovelace(500_000_000),
                    anchor: None,
                }],
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
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![vec![0xEE]],
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
        };

        let mut reg_errors: Vec<ValidationError> = Vec::new();
        check_script_redeemers(&tx_reg, &utxo_set, &mut reg_errors);

        assert!(
            !reg_errors.iter().any(
                |e| matches!(e, ValidationError::MissingRedeemer { tag, .. } if tag == "Cert")
            ),
            "RegDRep must not require a Cert redeemer, got: {reg_errors:?}"
        );

        // --- Part B: UnregDRep with Script credential — redeemer required ---

        let tx_unreg = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(4_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
                ttl: None,
                // DRep unregistration — redeemer required.
                certificates: vec![Certificate::UnregDRep {
                    credential: Credential::Script(drep_script_hash),
                    refund: Lovelace(500_000_000),
                }],
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
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![vec![0xEE]],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                // No Cert redeemer — must trigger MissingRedeemer.
                redeemers: vec![],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut unreg_errors: Vec<ValidationError> = Vec::new();
        check_script_redeemers(&tx_unreg, &utxo_set, &mut unreg_errors);

        assert!(
            unreg_errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingRedeemer { tag, index: 0 } if tag == "Cert")),
            "Expected MissingRedeemer {{ tag: Cert, index: 0 }} for UnregDRep with Script credential, got: {unreg_errors:?}"
        );
    }

    /// A `CommitteeColdResign` certificate with a Script cold credential and
    /// no Cert redeemer must be rejected.
    #[test]
    fn test_cert_redeemer_committee_cold_resign_missing() {
        use super::super::collateral::check_script_redeemers;
        use torsten_primitives::credentials::Credential;
        use torsten_primitives::transaction::Certificate;

        let cold_script_hash = Hash28::from_bytes([0xD9; 28]);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x87; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(4_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
                ttl: None,
                certificates: vec![Certificate::CommitteeColdResign {
                    cold_credential: Credential::Script(cold_script_hash),
                    anchor: None,
                }],
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
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![vec![0xFF]],
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
        };

        let mut errors: Vec<ValidationError> = Vec::new();
        check_script_redeemers(&tx, &utxo_set, &mut errors);

        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingRedeemer { tag, index: 0 } if tag == "Cert")),
            "Expected MissingRedeemer {{ tag: Cert, index: 0 }} for CommitteeColdResign with Script cold credential, got: {errors:?}"
        );
    }

    // =========================================================================
    // Vote redeemer tests (Issue #179)
    // =========================================================================

    /// A DRep voter with a Script credential and NO matching Vote redeemer must
    /// produce a `MissingRedeemer { tag: "Vote", index: 0 }` error.
    #[test]
    fn test_vote_redeemer_script_drep_missing() {
        use super::super::collateral::check_script_redeemers;
        use torsten_primitives::credentials::Credential;
        use torsten_primitives::transaction::{GovActionId, Vote, Voter, VotingProcedure};

        let script_hash = Hash28::from_bytes([0xE0; 28]);

        // Build a UTxO with a key-locked input (so Spend redeemer is not needed).
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xA0; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        // One DRep voter with a Script credential. No Vote redeemer provided.
        let mut voting_procedures: BTreeMap<Voter, BTreeMap<GovActionId, VotingProcedure>> =
            BTreeMap::new();
        let gov_action_id = GovActionId {
            transaction_id: Hash32::from_bytes([0xB0; 32]),
            action_index: 0,
        };
        let mut votes: BTreeMap<GovActionId, VotingProcedure> = BTreeMap::new();
        votes.insert(
            gov_action_id,
            VotingProcedure {
                vote: Vote::Yes,
                anchor: None,
            },
        );
        voting_procedures.insert(Voter::DRep(Credential::Script(script_hash)), votes);

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(4_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
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
                voting_procedures,
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                // Include a V2 script so has_plutus_scripts() returns true.
                plutus_v2_scripts: vec![vec![0xE1]],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                // No Vote redeemer — this is what we are testing.
                redeemers: vec![],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors: Vec<ValidationError> = Vec::new();
        check_script_redeemers(&tx, &utxo_set, &mut errors);

        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingRedeemer { tag, index: 0 } if tag == "Vote")),
            "Expected MissingRedeemer {{ tag: Vote, index: 0 }} for Script DRep voter, got: {errors:?}"
        );
    }

    /// A DRep voter with a Script credential and a matching Vote redeemer at
    /// index 0 must NOT produce a MissingRedeemer error.
    #[test]
    fn test_vote_redeemer_script_drep_present_ok() {
        use super::super::collateral::check_script_redeemers;
        use torsten_primitives::credentials::Credential;
        use torsten_primitives::transaction::{GovActionId, Vote, Voter, VotingProcedure};

        let script_hash = Hash28::from_bytes([0xE2; 28]);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xA1; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let mut voting_procedures: BTreeMap<Voter, BTreeMap<GovActionId, VotingProcedure>> =
            BTreeMap::new();
        let gov_action_id = GovActionId {
            transaction_id: Hash32::from_bytes([0xB1; 32]),
            action_index: 0,
        };
        let mut votes: BTreeMap<GovActionId, VotingProcedure> = BTreeMap::new();
        votes.insert(
            gov_action_id,
            VotingProcedure {
                vote: Vote::No,
                anchor: None,
            },
        );
        voting_procedures.insert(Voter::DRep(Credential::Script(script_hash)), votes);

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(4_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
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
                voting_procedures,
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![vec![0xE2]],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                // Vote redeemer at index 0 — matches the script DRep voter.
                redeemers: vec![Redeemer {
                    tag: RedeemerTag::Vote,
                    index: 0,
                    data: PlutusData::Integer(0),
                    ex_units: ExUnits {
                        mem: 100,
                        steps: 100,
                    },
                }],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors: Vec<ValidationError> = Vec::new();
        check_script_redeemers(&tx, &utxo_set, &mut errors);

        assert!(
            !errors.iter().any(
                |e| matches!(e, ValidationError::MissingRedeemer { tag, .. } if tag == "Vote")
            ),
            "Vote redeemer present at index 0; must not produce MissingRedeemer: {errors:?}"
        );
    }

    /// A Vote redeemer at the wrong index must produce a MissingRedeemer error
    /// at the correct index (0) even though index 1 has a redeemer.
    #[test]
    fn test_vote_redeemer_wrong_index_rejected() {
        use super::super::collateral::check_script_redeemers;
        use torsten_primitives::credentials::Credential;
        use torsten_primitives::transaction::{GovActionId, Vote, Voter, VotingProcedure};

        let script_hash = Hash28::from_bytes([0xE3; 28]);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xA2; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        // Script DRep voter is at BTreeMap position 0.
        let mut voting_procedures: BTreeMap<Voter, BTreeMap<GovActionId, VotingProcedure>> =
            BTreeMap::new();
        let gov_action_id = GovActionId {
            transaction_id: Hash32::from_bytes([0xB2; 32]),
            action_index: 0,
        };
        let mut votes: BTreeMap<GovActionId, VotingProcedure> = BTreeMap::new();
        votes.insert(
            gov_action_id,
            VotingProcedure {
                vote: Vote::Abstain,
                anchor: None,
            },
        );
        voting_procedures.insert(Voter::DRep(Credential::Script(script_hash)), votes);

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(4_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
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
                voting_procedures,
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![vec![0xE3]],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                // Redeemer at index 1, but the script voter is at position 0.
                redeemers: vec![Redeemer {
                    tag: RedeemerTag::Vote,
                    index: 1,
                    data: PlutusData::Integer(0),
                    ex_units: ExUnits {
                        mem: 100,
                        steps: 100,
                    },
                }],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors: Vec<ValidationError> = Vec::new();
        check_script_redeemers(&tx, &utxo_set, &mut errors);

        // The script voter at index 0 has no redeemer — expect MissingRedeemer.
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingRedeemer { tag, index: 0 } if tag == "Vote")),
            "Expected MissingRedeemer {{ tag: Vote, index: 0 }} when only index 1 has a redeemer, got: {errors:?}"
        );
    }

    /// A key-credential DRep voter does NOT require a Vote redeemer — no error
    /// should be produced even without a redeemer.
    #[test]
    fn test_vote_redeemer_key_drep_no_redeemer_required() {
        use super::super::collateral::check_script_redeemers;
        use torsten_primitives::credentials::Credential;
        use torsten_primitives::transaction::{GovActionId, Vote, Voter, VotingProcedure};

        // VerificationKey credential — not a script.
        let drep_key_hash = Hash28::from_bytes([0xE4; 28]);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xA3; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let mut voting_procedures: BTreeMap<Voter, BTreeMap<GovActionId, VotingProcedure>> =
            BTreeMap::new();
        let gov_action_id = GovActionId {
            transaction_id: Hash32::from_bytes([0xB3; 32]),
            action_index: 0,
        };
        let mut votes: BTreeMap<GovActionId, VotingProcedure> = BTreeMap::new();
        votes.insert(
            gov_action_id,
            VotingProcedure {
                vote: Vote::Yes,
                anchor: None,
            },
        );
        // VerificationKey credential DRep — redeemer not required.
        voting_procedures.insert(
            Voter::DRep(Credential::VerificationKey(drep_key_hash)),
            votes,
        );

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(4_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
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
                voting_procedures,
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![vec![0xE4]],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                // No Vote redeemer — but none is required for a key-credential voter.
                redeemers: vec![],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors: Vec<ValidationError> = Vec::new();
        check_script_redeemers(&tx, &utxo_set, &mut errors);

        assert!(
            !errors.iter().any(
                |e| matches!(e, ValidationError::MissingRedeemer { tag, .. } if tag == "Vote")
            ),
            "Key-credential DRep voter must not require a Vote redeemer, got: {errors:?}"
        );
    }

    /// An SPO voter (`StakePool`) never requires a Vote redeemer regardless of
    /// the pool ID hash value — SPOs are always key-based.
    #[test]
    fn test_vote_redeemer_spo_voter_no_redeemer_required() {
        use super::super::collateral::check_script_redeemers;
        use torsten_primitives::transaction::{GovActionId, Vote, Voter, VotingProcedure};

        let pool_id = Hash32::from_bytes([0xE5; 32]);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xA4; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let mut voting_procedures: BTreeMap<Voter, BTreeMap<GovActionId, VotingProcedure>> =
            BTreeMap::new();
        let gov_action_id = GovActionId {
            transaction_id: Hash32::from_bytes([0xB4; 32]),
            action_index: 0,
        };
        let mut votes: BTreeMap<GovActionId, VotingProcedure> = BTreeMap::new();
        votes.insert(
            gov_action_id,
            VotingProcedure {
                vote: Vote::No,
                anchor: None,
            },
        );
        voting_procedures.insert(Voter::StakePool(pool_id), votes);

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(4_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
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
                voting_procedures,
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![vec![0xE5]],
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
        };

        let mut errors: Vec<ValidationError> = Vec::new();
        check_script_redeemers(&tx, &utxo_set, &mut errors);

        assert!(
            !errors.iter().any(
                |e| matches!(e, ValidationError::MissingRedeemer { tag, .. } if tag == "Vote")
            ),
            "SPO voter must not require a Vote redeemer, got: {errors:?}"
        );
    }

    // =========================================================================
    // Propose redeemer tests (Issue #179)
    // =========================================================================

    /// A `ParameterChange` proposal with a non-None `policy_hash` and no
    /// matching Propose redeemer must produce `MissingRedeemer { tag: "Propose", index: 0 }`.
    #[test]
    fn test_propose_redeemer_parameter_change_with_policy_hash_missing() {
        use super::super::collateral::check_script_redeemers;
        use torsten_primitives::hash::ScriptHash;
        use torsten_primitives::transaction::{GovAction, ProposalProcedure, ProtocolParamUpdate};

        let policy_script_hash: ScriptHash = Hash28::from_bytes([0xF0; 28]);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xC0; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(9_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
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
                proposal_procedures: vec![ProposalProcedure {
                    deposit: Lovelace(100_000_000),
                    return_addr: vec![0xE0, 0x01, 0x02],
                    gov_action: GovAction::ParameterChange {
                        prev_action_id: None,
                        protocol_param_update: Box::new(ProtocolParamUpdate::default()),
                        policy_hash: Some(policy_script_hash),
                    },
                    anchor: torsten_primitives::transaction::Anchor {
                        url: "https://example.com".to_string(),
                        data_hash: Hash32::from_bytes([0xD0; 32]),
                    },
                }],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                // Include the constitutionality script so check_script_redeemers
                // treats this as a Plutus transaction.
                plutus_v3_scripts: vec![vec![0xF1]],
                plutus_v2_scripts: vec![],
                plutus_data: vec![],
                // No Propose redeemer — this is what we are testing.
                redeemers: vec![],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors: Vec<ValidationError> = Vec::new();
        check_script_redeemers(&tx, &utxo_set, &mut errors);

        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingRedeemer { tag, index: 0 } if tag == "Propose")),
            "Expected MissingRedeemer {{ tag: Propose, index: 0 }} for ParameterChange with policy_hash, got: {errors:?}"
        );
    }

    /// A `TreasuryWithdrawals` proposal with a non-None `policy_hash` and no
    /// matching Propose redeemer must produce `MissingRedeemer { tag: "Propose", index: 0 }`.
    #[test]
    fn test_propose_redeemer_treasury_withdrawals_with_policy_hash_missing() {
        use super::super::collateral::check_script_redeemers;
        use torsten_primitives::hash::ScriptHash;
        use torsten_primitives::transaction::{GovAction, ProposalProcedure};

        let policy_script_hash: ScriptHash = Hash28::from_bytes([0xF2; 28]);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xC1; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        // TreasuryWithdrawals with policy_hash set.
        let mut treasury_withdrawals: BTreeMap<Vec<u8>, Lovelace> = BTreeMap::new();
        treasury_withdrawals.insert(vec![0xE1, 0x02, 0x03], Lovelace(500_000_000));

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(9_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
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
                proposal_procedures: vec![ProposalProcedure {
                    deposit: Lovelace(100_000_000),
                    return_addr: vec![0xE1, 0x02, 0x03],
                    gov_action: GovAction::TreasuryWithdrawals {
                        withdrawals: treasury_withdrawals,
                        policy_hash: Some(policy_script_hash),
                    },
                    anchor: torsten_primitives::transaction::Anchor {
                        url: "https://example.com/treasury".to_string(),
                        data_hash: Hash32::from_bytes([0xD1; 32]),
                    },
                }],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v3_scripts: vec![vec![0xF3]],
                plutus_v2_scripts: vec![],
                plutus_data: vec![],
                redeemers: vec![],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors: Vec<ValidationError> = Vec::new();
        check_script_redeemers(&tx, &utxo_set, &mut errors);

        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingRedeemer { tag, index: 0 } if tag == "Propose")),
            "Expected MissingRedeemer {{ tag: Propose, index: 0 }} for TreasuryWithdrawals with policy_hash, got: {errors:?}"
        );
    }

    /// A `ParameterChange` proposal WITHOUT a `policy_hash` does NOT require a
    /// Propose redeemer.
    #[test]
    fn test_propose_redeemer_no_policy_hash_not_required() {
        use super::super::collateral::check_script_redeemers;
        use torsten_primitives::transaction::{GovAction, ProposalProcedure, ProtocolParamUpdate};

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xC2; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(9_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
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
                // ParameterChange with policy_hash = None — no redeemer needed.
                proposal_procedures: vec![ProposalProcedure {
                    deposit: Lovelace(100_000_000),
                    return_addr: vec![0xE2, 0x02, 0x03],
                    gov_action: GovAction::ParameterChange {
                        prev_action_id: None,
                        protocol_param_update: Box::new(ProtocolParamUpdate::default()),
                        policy_hash: None,
                    },
                    anchor: torsten_primitives::transaction::Anchor {
                        url: "https://example.com/pchange".to_string(),
                        data_hash: Hash32::from_bytes([0xD2; 32]),
                    },
                }],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![vec![0xF4]],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                // No Propose redeemer — must not be required when policy_hash is None.
                redeemers: vec![],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors: Vec<ValidationError> = Vec::new();
        check_script_redeemers(&tx, &utxo_set, &mut errors);

        assert!(
            !errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingRedeemer { tag, .. } if tag == "Propose")),
            "ParameterChange without policy_hash must not require a Propose redeemer, got: {errors:?}"
        );
    }

    /// A `HardForkInitiation` proposal never requires a Propose redeemer
    /// regardless of content.
    #[test]
    fn test_propose_redeemer_hard_fork_not_required() {
        use super::super::collateral::check_script_redeemers;
        use torsten_primitives::transaction::{GovAction, ProposalProcedure};

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xC3; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(9_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
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
                // HardForkInitiation — no policy_hash, never requires a Propose redeemer.
                proposal_procedures: vec![ProposalProcedure {
                    deposit: Lovelace(100_000_000),
                    return_addr: vec![0xE3, 0x02, 0x03],
                    gov_action: GovAction::HardForkInitiation {
                        prev_action_id: None,
                        protocol_version: (10, 0),
                    },
                    anchor: torsten_primitives::transaction::Anchor {
                        url: "https://example.com/hf".to_string(),
                        data_hash: Hash32::from_bytes([0xD3; 32]),
                    },
                }],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![vec![0xF5]],
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
        };

        let mut errors: Vec<ValidationError> = Vec::new();
        check_script_redeemers(&tx, &utxo_set, &mut errors);

        assert!(
            !errors.iter().any(
                |e| matches!(e, ValidationError::MissingRedeemer { tag, .. } if tag == "Propose")
            ),
            "HardForkInitiation must not require a Propose redeemer, got: {errors:?}"
        );
    }

    /// A `ParameterChange` with `policy_hash` provided WITH a matching Propose
    /// redeemer at index 0 must NOT produce a MissingRedeemer error.
    #[test]
    fn test_propose_redeemer_parameter_change_present_ok() {
        use super::super::collateral::check_script_redeemers;
        use torsten_primitives::hash::ScriptHash;
        use torsten_primitives::transaction::{GovAction, ProposalProcedure, ProtocolParamUpdate};

        let policy_script_hash: ScriptHash = Hash28::from_bytes([0xF6; 28]);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xC4; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(9_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
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
                proposal_procedures: vec![ProposalProcedure {
                    deposit: Lovelace(100_000_000),
                    return_addr: vec![0xE4, 0x02, 0x03],
                    gov_action: GovAction::ParameterChange {
                        prev_action_id: None,
                        protocol_param_update: Box::new(ProtocolParamUpdate::default()),
                        policy_hash: Some(policy_script_hash),
                    },
                    anchor: torsten_primitives::transaction::Anchor {
                        url: "https://example.com/ppresent".to_string(),
                        data_hash: Hash32::from_bytes([0xD4; 32]),
                    },
                }],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v3_scripts: vec![vec![0xF6]],
                plutus_v2_scripts: vec![],
                plutus_data: vec![],
                // Propose redeemer at index 0 — matches the ParameterChange proposal.
                redeemers: vec![Redeemer {
                    tag: RedeemerTag::Propose,
                    index: 0,
                    data: PlutusData::Integer(1),
                    ex_units: ExUnits {
                        mem: 200,
                        steps: 200,
                    },
                }],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors: Vec<ValidationError> = Vec::new();
        check_script_redeemers(&tx, &utxo_set, &mut errors);

        assert!(
            !errors.iter().any(
                |e| matches!(e, ValidationError::MissingRedeemer { tag, .. } if tag == "Propose")
            ),
            "Propose redeemer present at index 0; must not produce MissingRedeemer: {errors:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Rule 9c: Datum witness completeness tests
    //
    // These tests exercise `datum::check_datum_witnesses` directly (the
    // function that implements Rule 9c), plus edge cases covering all six
    // scenarios mandated by the spec.
    // -----------------------------------------------------------------------

    // Helper: build a script-locked enterprise address from an arbitrary Hash28.
    fn script_enterprise_address(script_hash: Hash28) -> Address {
        use torsten_primitives::address::EnterpriseAddress;
        use torsten_primitives::credentials::Credential;
        use torsten_primitives::network::NetworkId;
        Address::Enterprise(EnterpriseAddress {
            network: NetworkId::Testnet,
            payment: Credential::Script(script_hash),
        })
    }

    // Helper: build a VKey-locked enterprise address (for datum tests).
    fn vk_enterprise_address_datum(key_hash: Hash28) -> Address {
        use torsten_primitives::address::EnterpriseAddress;
        use torsten_primitives::credentials::Credential;
        use torsten_primitives::network::NetworkId;
        Address::Enterprise(EnterpriseAddress {
            network: NetworkId::Testnet,
            payment: Credential::VerificationKey(key_hash),
        })
    }

    // Helper: compute datum hash = blake2b_256(CBOR(datum)).
    fn datum_hash_of(datum: &PlutusData) -> Hash32 {
        let cbor = torsten_serialization::encode_plutus_data(datum);
        torsten_primitives::hash::blake2b_256(&cbor)
    }

    /// Script-locked input with DatumHash but no matching datum in the witness
    /// set must produce `MissingDatumWitness`.
    #[test]
    fn test_datum_witness_missing_for_script_locked_input() {
        use super::super::datum::check_datum_witnesses;

        let datum = PlutusData::Integer(42);
        let hash = datum_hash_of(&datum);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xA1u8; 32]),
            index: 0,
        };
        // UTxO is script-locked and carries a DatumHash.
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: script_enterprise_address(Hash28::from_bytes([0xAA; 28])),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::DatumHash(hash),
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![],
                fee: Lovelace(0),
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
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                // No datum witness supplied — must trigger MissingDatumWitness.
                plutus_data: vec![],
                redeemers: vec![],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors = Vec::new();
        check_datum_witnesses(&tx, &utxo_set, &mut errors);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingDatumWitness(_))),
            "Expected MissingDatumWitness, got: {errors:?}"
        );
    }

    /// Script-locked input with DatumHash and the matching datum in the witness
    /// set must pass without errors.
    #[test]
    fn test_datum_witness_present_for_script_locked_input_ok() {
        use super::super::datum::check_datum_witnesses;

        let datum = PlutusData::Integer(42);
        let hash = datum_hash_of(&datum);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xA2u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: script_enterprise_address(Hash28::from_bytes([0xBB; 28])),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::DatumHash(hash),
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![],
                fee: Lovelace(0),
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
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                // Matching datum supplied — no error expected.
                plutus_data: vec![datum],
                redeemers: vec![],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors = Vec::new();
        check_datum_witnesses(&tx, &utxo_set, &mut errors);
        assert!(errors.is_empty(), "Expected no errors, got: {errors:?}");
    }

    /// Script-locked input with `OutputDatum::InlineDatum` does NOT require a
    /// datum witness — the datum is already embedded in the UTxO itself.
    #[test]
    fn test_datum_witness_inline_datum_no_witness_needed() {
        use super::super::datum::check_datum_witnesses;

        let datum = PlutusData::Integer(99);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xA3u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: script_enterprise_address(Hash28::from_bytes([0xCC; 28])),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::InlineDatum {
                    data: datum,
                    raw_cbor: None,
                },
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![],
                fee: Lovelace(0),
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
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                // No witness datum — inline UTxO datum is sufficient.
                plutus_data: vec![],
                redeemers: vec![],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors = Vec::new();
        check_datum_witnesses(&tx, &utxo_set, &mut errors);
        assert!(
            errors.is_empty(),
            "Expected no errors for inline datum UTxO, got: {errors:?}"
        );
    }

    /// A VKey-locked input with a DatumHash on its UTxO does NOT require a
    /// datum witness — only script-locked inputs need one.
    #[test]
    fn test_datum_witness_non_script_input_datum_hash_no_witness_needed() {
        use super::super::datum::check_datum_witnesses;

        let datum = PlutusData::Bytes(vec![0xDE, 0xAD]);
        let hash = datum_hash_of(&datum);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xA4u8; 32]),
            index: 0,
        };
        // VKey-locked input carrying a DatumHash — unusual but not forbidden.
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: vk_enterprise_address_datum(Hash28::from_bytes([0xDD; 28])),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::DatumHash(hash),
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![],
                fee: Lovelace(0),
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
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                // No datum witness — VKey input, none required.
                plutus_data: vec![],
                redeemers: vec![],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors = Vec::new();
        check_datum_witnesses(&tx, &utxo_set, &mut errors);
        assert!(
            errors.is_empty(),
            "Expected no errors for VKey input with DatumHash, got: {errors:?}"
        );
    }

    /// A datum in the witness set whose hash is not referenced by any spending
    /// input UTxO or any transaction output must produce `ExtraDatumWitness`.
    #[test]
    fn test_datum_witness_extra_unreferenced_datum_rejected() {
        use super::super::datum::check_datum_witnesses;

        // Spurious datum — no input or output needs this.
        let spurious = PlutusData::Integer(12345);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xA5u8; 32]),
            index: 0,
        };
        // Plain VKey-locked input with no datum.
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: vk_enterprise_address_datum(Hash28::from_bytes([0xEE; 28])),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![],
                fee: Lovelace(0),
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
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                // Spurious datum not needed by any input or output.
                plutus_data: vec![spurious],
                redeemers: vec![],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors = Vec::new();
        check_datum_witnesses(&tx, &utxo_set, &mut errors);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::ExtraDatumWitness(_))),
            "Expected ExtraDatumWitness for unreferenced datum, got: {errors:?}"
        );
    }

    /// Both needed datums present and no extras — must pass cleanly.
    #[test]
    fn test_datum_witness_all_needed_present_no_extras_ok() {
        use super::super::datum::check_datum_witnesses;

        let datum_a = PlutusData::Integer(1);
        let datum_b = PlutusData::Bytes(vec![0x01, 0x02, 0x03]);
        let hash_a = datum_hash_of(&datum_a);
        let hash_b = datum_hash_of(&datum_b);

        let mut utxo_set = UtxoSet::new();

        let input_a = TransactionInput {
            transaction_id: Hash32::from_bytes([0xA6u8; 32]),
            index: 0,
        };
        let input_b = TransactionInput {
            transaction_id: Hash32::from_bytes([0xA7u8; 32]),
            index: 0,
        };

        utxo_set.insert(
            input_a.clone(),
            TransactionOutput {
                address: script_enterprise_address(Hash28::from_bytes([0x11; 28])),
                value: Value::lovelace(3_000_000),
                datum: OutputDatum::DatumHash(hash_a),
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        utxo_set.insert(
            input_b.clone(),
            TransactionOutput {
                address: script_enterprise_address(Hash28::from_bytes([0x22; 28])),
                value: Value::lovelace(3_000_000),
                datum: OutputDatum::DatumHash(hash_b),
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input_a, input_b],
                outputs: vec![],
                fee: Lovelace(0),
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
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                // Exactly the two needed datums — no extras.
                plutus_data: vec![datum_a, datum_b],
                redeemers: vec![],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors = Vec::new();
        check_datum_witnesses(&tx, &utxo_set, &mut errors);
        assert!(
            errors.is_empty(),
            "Expected no errors when all needed datums present, got: {errors:?}"
        );
    }

    /// A transaction output that declares a `DatumHash` makes the corresponding
    /// datum bytes an "allowed supplemental" datum.  Supplying those bytes as a
    /// witness must NOT produce `ExtraDatumWitness`.
    #[test]
    fn test_datum_witness_output_datum_hash_allows_supplemental_witness() {
        use super::super::datum::check_datum_witnesses;

        let datum = PlutusData::List(vec![PlutusData::Integer(7)]);
        let hash = datum_hash_of(&datum);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xA8u8; 32]),
            index: 0,
        };
        // Plain VKey-locked input — no datum witness required for the input.
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: vk_enterprise_address_datum(Hash28::from_bytes([0x33; 28])),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                // Output declares a DatumHash — witness datum is supplemental.
                outputs: vec![TransactionOutput {
                    address: script_enterprise_address(Hash28::from_bytes([0x44; 28])),
                    value: Value::lovelace(4_000_000),
                    datum: OutputDatum::DatumHash(hash),
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(1_000_000),
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
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                // Supplemental datum matching the output's DatumHash.
                plutus_data: vec![datum],
                redeemers: vec![],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let mut errors = Vec::new();
        check_datum_witnesses(&tx, &utxo_set, &mut errors);
        assert!(
            errors.is_empty(),
            "Expected no errors for supplemental output datum witness, got: {errors:?}"
        );
    }

    /// A transaction with no datums anywhere (no script inputs, no output datum
    /// hashes, no witness datums) must pass cleanly — nothing to check.
    #[test]
    fn test_datum_witness_no_datums_anywhere_ok() {
        use super::super::datum::check_datum_witnesses;

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xA9u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: vk_enterprise_address_datum(Hash28::from_bytes([0x55; 28])),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![],
                fee: Lovelace(0),
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
        };

        let mut errors = Vec::new();
        check_datum_witnesses(&tx, &utxo_set, &mut errors);
        assert!(errors.is_empty(), "Expected no errors, got: {errors:?}");
    }

    // ---------------------------------------------------------------------------
    // Issue #182 — script_data_hash: spending-input script_refs must not be
    // mistakenly flagged as UnexpectedScriptDataHash.
    // ---------------------------------------------------------------------------

    /// A transaction that spends a UTxO carrying a `script_ref` (PlutusV2) may
    /// legitimately include a `script_data_hash` in its body — the ref-script
    /// byte size contributes to the cost model.  Before the fix, the
    /// `check_script_data_hash` helper only scanned `reference_inputs` for
    /// ref-scripts, which caused this valid transaction to be rejected with
    /// `UnexpectedScriptDataHash`.
    ///
    /// This test verifies that the spending-input path now passes correctly.
    /// We use a dummy (all-zeros) script_data_hash so we do NOT trigger the
    /// `ScriptDataHashMismatch` branch — the goal is only to confirm that the
    /// `UnexpectedScriptDataHash` error is NOT emitted when the spending input
    /// carries a script_ref.
    #[test]
    fn test_issue_182_spending_input_script_ref_allows_script_data_hash() {
        let mut utxo_set = UtxoSet::new();

        // Spending input: a UTxO that carries a PlutusV2 script_ref.
        let spending_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xA1u8; 32]),
            index: 0,
        };
        let script_bytes = vec![0xCAu8; 64]; // dummy 64-byte Plutus script
        utxo_set.insert(
            spending_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: Some(ScriptRef::PlutusV2(script_bytes)),
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let params = ProtocolParameters::mainnet_defaults();

        // Build a simple transaction: spend the script-ref-bearing UTxO,
        // no inline Plutus scripts in the witness set, but a script_data_hash
        // present in the body.  The hash value is all-zeros — it won't match
        // the real computed hash so we'll get ScriptDataHashMismatch (the hash
        // is there, it just has the wrong value), but NOT UnexpectedScriptDataHash.
        let mut tx = make_simple_tx(spending_input, 9_800_000, 200_000);
        tx.body.script_data_hash = Some(Hash32::ZERO);

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);

        // We expect either Ok (unlikely given zero hash) or an error that is
        // NOT UnexpectedScriptDataHash.  The important invariant is that
        // UnexpectedScriptDataHash must not appear.
        let no_unexpected_hash = match &result {
            Ok(_) => true,
            Err(errors) => !errors
                .iter()
                .any(|e| matches!(e, ValidationError::UnexpectedScriptDataHash)),
        };
        assert!(
            no_unexpected_hash,
            "Expected no UnexpectedScriptDataHash when spending input carries script_ref; \
             got errors: {:?}",
            result.err()
        );
    }

    /// Counterpart: verify that `check_script_data_hash` still emits
    /// `UnexpectedScriptDataHash` when a `script_data_hash` is present in the
    /// transaction body but NEITHER spending inputs NOR reference inputs carry
    /// any `script_ref`.
    ///
    /// This test calls `check_script_data_hash` directly (bypassing the
    /// `has_plutus_scripts` gate in `validate_transaction`) because the
    /// `UnexpectedScriptDataHash` branch is only reachable for transactions with
    /// no redeemers, no datums, and no inline Plutus scripts — a configuration
    /// that never satisfies `has_plutus_scripts()` via `validate_transaction`.
    ///
    /// By calling the function directly we confirm that the unit-level logic is
    /// correct: inputs without ref-scripts must not suppress the error.
    #[test]
    fn test_issue_182_no_script_ref_still_rejects_unexpected_script_data_hash() {
        let mut utxo_set = UtxoSet::new();

        // Spending input: plain UTxO with NO script_ref.
        let spending_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xB2u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            spending_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None, // no script ref — must still be rejected
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let params = ProtocolParameters::mainnet_defaults();

        // Transaction: spending input present, no redeemers, no datums,
        // no inline Plutus scripts, but script_data_hash is set.
        let mut tx = make_simple_tx(spending_input, 9_800_000, 200_000);
        tx.body.script_data_hash = Some(Hash32::ZERO);
        // Leave witness_set.plutus_v* empty and redeemers empty so the
        // else-if branch inside check_script_data_hash is exercised.

        let mut errors = Vec::new();
        check_script_data_hash(&tx, &utxo_set, &params, &mut errors);

        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::UnexpectedScriptDataHash)),
            "Expected UnexpectedScriptDataHash when no ref-scripts exist; got: {errors:?}"
        );
    }

    // ---------------------------------------------------------------------------
    // Issue #186 — Treasury value Phase-1 check
    //
    // Conway LEDGERS rule: when a transaction body declares `currentTreasuryValue`
    // (body field 19), `validate_transaction_with_pools` must reject the transaction
    // if the declared value does not match the `current_treasury` argument.
    //
    // Three sub-cases:
    //   1. Mismatch            → TreasuryValueMismatch
    //   2. Exact match         → no error
    //   3. current_treasury=None → check is skipped entirely (pre-Conway mempool)
    //
    // Reference: Cardano Blueprint LEDGERS flowchart, "submittedTreasuryValue ==
    // currentTreasuryValue" predicate; Haskell `conwayLedgerFn` in
    // `Cardano.Ledger.Conway.Rules.Ledger`.
    // ---------------------------------------------------------------------------

    #[test]
    fn test_issue_186_treasury_value_mismatch_rejects() {
        // A tx that declares treasury_value = 999 when the ledger holds 500 must
        // be rejected with TreasuryValueMismatch.
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xD0u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let mut params = ProtocolParameters::mainnet_defaults();
        // mainnet_defaults already sets protocol_version_major = 9 (Conway),
        // confirming the treasury check is enabled.
        params.protocol_version_major = 9;

        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        // Declare a treasury value that does NOT match current_treasury.
        tx.body.treasury_value = Some(Lovelace(999));

        let result = validate_transaction_with_pools(
            &tx,
            &utxo_set,
            &params,
            100,       // current_slot
            300,       // tx_size
            None,      // slot_config
            None,      // registered_pools
            Some(500), // current_treasury — mismatches declared 999
            None,      // reward_accounts
            None,      // current_epoch
        );

        assert!(
            result.is_err(),
            "Expected TreasuryValueMismatch when declared treasury != actual; got Ok"
        );
        let errors = result.unwrap_err();
        assert!(
            errors.iter().any(|e| matches!(
                e,
                ValidationError::TreasuryValueMismatch {
                    declared: 999,
                    actual: 500,
                }
            )),
            "Expected TreasuryValueMismatch(declared=999, actual=500); got: {errors:?}"
        );
    }

    #[test]
    fn test_issue_186_treasury_value_match_passes() {
        // A tx that declares treasury_value matching current_treasury must not
        // produce a TreasuryValueMismatch error.
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xD1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let mut params = ProtocolParameters::mainnet_defaults();
        params.protocol_version_major = 9;

        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        // Declared value matches actual.
        tx.body.treasury_value = Some(Lovelace(500));

        let result = validate_transaction_with_pools(
            &tx,
            &utxo_set,
            &params,
            100,
            300,
            None,
            None,
            Some(500), // current_treasury matches declared
            None,      // reward_accounts
            None,      // current_epoch
        );

        // The tx may still fail other rules, but it must NOT fail with
        // TreasuryValueMismatch.
        let has_mismatch = matches!(&result, Err(errors) if errors.iter().any(|e| {
            matches!(e, ValidationError::TreasuryValueMismatch { .. })
        }));
        assert!(
            !has_mismatch,
            "Expected no TreasuryValueMismatch when declared treasury matches actual; got: {result:?}"
        );
    }

    #[test]
    fn test_issue_186_treasury_value_none_skips_check() {
        // When current_treasury is None (e.g. pre-Conway mempool), the check must
        // be skipped entirely even if treasury_value is present in the tx body.
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xD2u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        let mut params = ProtocolParameters::mainnet_defaults();
        params.protocol_version_major = 9;

        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        // Set a non-zero treasury_value in the body.
        tx.body.treasury_value = Some(Lovelace(12345));

        let result = validate_transaction_with_pools(
            &tx, &utxo_set, &params, 100, 300, None, None,
            None, // current_treasury = None → check must be skipped
            None, // reward_accounts
            None, // current_epoch
        );

        // Must not produce TreasuryValueMismatch regardless of the declared value.
        let has_mismatch = matches!(&result, Err(errors) if errors.iter().any(|e| {
            matches!(e, ValidationError::TreasuryValueMismatch { .. })
        }));
        assert!(
            !has_mismatch,
            "Expected no TreasuryValueMismatch when current_treasury is None; got: {result:?}"
        );
    }
}
