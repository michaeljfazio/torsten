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
        calculate_ref_script_tiered_fee, cbor_uint_size, compute_min_fee, compute_script_ref_hash,
        estimate_value_cbor_size, evaluate_native_script, script_ref_byte_size,
        MAX_REF_SCRIPT_SIZE_TIER_CAP,
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
        // total = 25601.2 -> ceiling = 25602, floor would be 25601
        let fee = calculate_ref_script_tiered_fee(1, 25_601);
        assert_eq!(
            fee, 25_602,
            "Ceiling division must round up (25600 + 6/5 = 25601.2 -> 25602)"
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

    /// Verify ceiling on multiple partial tiers.
    #[test]
    fn test_ref_script_fee_ceiling_multiple_partial_tiers() {
        // base_fee=1, size=51201 (two full tiers + 1 byte in third tier)
        // tier 1: 25600*1 = 25600
        // tier 2: 25600*6/5 = 30720
        // tier 3: 1*36/25 = 1.44
        // total = 56321.44 -> ceiling = 56322
        let fee = calculate_ref_script_tiered_fee(1, 51_201);
        assert_eq!(
            fee, 56_322,
            "Ceiling must handle partial third tier (25600 + 30720 + 1.44 = 56321.44 -> 56322)"
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
}
