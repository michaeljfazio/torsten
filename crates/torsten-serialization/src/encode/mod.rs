//! CBOR encoding for Cardano types.
//!
//! This module is organized into submodules by type:
//! - [`block`] — Block, header, operational cert, VRF, protocol version encoding
//! - [`certificate`] — Certificate, credential, relay, pool params encoding
//! - [`governance`] — Voting procedures, proposal procedures, gov actions encoding
//! - [`protocol_params`] — Protocol parameter update, cost models encoding
//! - [`script`] — Native scripts, script refs, redeemers, witness helpers encoding
//! - [`transaction`] — Transaction body, output, witness set, auxiliary data encoding
//! - [`value`] — Value and multi-asset encoding
//!
//! All public items are re-exported from this module for backwards compatibility.

mod block;
mod certificate;
mod governance;
mod protocol_params;
mod script;
mod transaction;
mod value;

// Re-export all public items for backwards compatibility
pub use block::{
    compute_block_body_hash, encode_block, encode_block_header, encode_block_header_body,
    encode_operational_cert, encode_protocol_version, encode_vrf_result,
};
pub use certificate::encode_certificate;
pub use script::{compute_script_data_hash, encode_native_script};
pub use transaction::{
    compute_transaction_hash, encode_auxiliary_data, encode_transaction, encode_transaction_body,
    encode_transaction_output, encode_witness_set,
};
pub use value::encode_value;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cbor::*;
    use std::collections::BTreeMap;
    use torsten_primitives::address::{Address, EnterpriseAddress};
    use torsten_primitives::block::{
        Block, BlockHeader, OperationalCert, ProtocolVersion, VrfOutput,
    };
    use torsten_primitives::credentials::Credential;
    use torsten_primitives::era::Era;
    use torsten_primitives::hash::{Hash28, Hash32};
    use torsten_primitives::time::{BlockNo, SlotNo};
    use torsten_primitives::transaction::*;
    use torsten_primitives::value::{AssetName, Lovelace, Value};

    #[test]
    fn test_encode_value_pure_ada() {
        let v = Value::lovelace(2_000_000);
        let encoded = encode_value(&v);
        // Should be just the uint encoding of 2000000
        assert_eq!(encoded, encode_uint(2_000_000));
    }

    #[test]
    fn test_encode_value_multi_asset() {
        let policy = Hash28::from_bytes([1u8; 28]);
        let asset_name = AssetName(b"Token".to_vec());
        let mut v = Value::lovelace(5_000_000);
        v.multi_asset
            .entry(policy)
            .or_default()
            .insert(asset_name, 100);

        let encoded = encode_value(&v);
        // Should be [coin, {policy: {name: qty}}]
        assert_eq!(encoded[0], 0x82); // array of 2
    }

    #[test]
    fn test_encode_transaction_output_simple() {
        let output = TransactionOutput {
            address: Address::Enterprise(EnterpriseAddress {
                network: torsten_primitives::network::NetworkId::Mainnet,
                payment: Credential::VerificationKey(Hash28::from_bytes([0u8; 28])),
            }),
            value: Value::lovelace(1_000_000),
            datum: OutputDatum::None,
            script_ref: None,
            raw_cbor: None,
        };
        let encoded = encode_transaction_output(&output);
        assert_eq!(encoded[0], 0xa2); // map of 2 (address + value)
    }

    #[test]
    fn test_encode_transaction_output_with_datum_hash() {
        let output = TransactionOutput {
            address: Address::Enterprise(EnterpriseAddress {
                network: torsten_primitives::network::NetworkId::Mainnet,
                payment: Credential::VerificationKey(Hash28::from_bytes([0u8; 28])),
            }),
            value: Value::lovelace(1_000_000),
            datum: OutputDatum::DatumHash(Hash32::ZERO),
            script_ref: None,
            raw_cbor: None,
        };
        let encoded = encode_transaction_output(&output);
        assert_eq!(encoded[0], 0xa3); // map of 3 (address + value + datum)
    }

    #[test]
    fn test_encode_native_script_pubkey() {
        let script = NativeScript::ScriptPubkey(Hash32::ZERO);
        let encoded = encode_native_script(&script);
        assert_eq!(encoded[0], 0x82); // array of 2
        assert_eq!(encoded[1], 0x00); // type 0
                                      // Key hash should be encoded as 28 bytes (AddrKeyhash), not 32
        assert_eq!(encoded[2], 0x58); // bytes with 1-byte length
        assert_eq!(encoded[3], 0x1c); // 28 bytes
        assert_eq!(encoded.len(), 4 + 28); // header(4) + keyhash(28)
    }

    #[test]
    fn test_encode_native_script_all() {
        let script = NativeScript::ScriptAll(vec![
            NativeScript::ScriptPubkey(Hash32::ZERO),
            NativeScript::ScriptPubkey(Hash32::ZERO),
        ]);
        let encoded = encode_native_script(&script);
        assert_eq!(encoded[0], 0x82); // array of 2
        assert_eq!(encoded[1], 0x01); // type 1 (all)
    }

    #[test]
    fn test_encode_certificate_stake_reg() {
        let cert = Certificate::StakeRegistration(Credential::VerificationKey(Hash28::from_bytes(
            [0u8; 28],
        )));
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x82); // array of 2
        assert_eq!(encoded[1], 0x00); // type 0
    }

    #[test]
    fn test_encode_certificate_pool_retirement() {
        let cert = Certificate::PoolRetirement {
            pool_hash: Hash28::from_bytes([1u8; 28]),
            epoch: 300,
        };
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x83); // array of 3
        assert_eq!(encoded[1], 0x04); // type 4
    }

    #[test]
    fn test_encode_witness_set_empty() {
        let ws = TransactionWitnessSet {
            vkey_witnesses: vec![],
            native_scripts: vec![],
            bootstrap_witnesses: vec![],
            plutus_v1_scripts: vec![],
            plutus_v2_scripts: vec![],
            plutus_v3_scripts: vec![],
            plutus_data: vec![],
            redeemers: vec![],
        };
        let encoded = encode_witness_set(&ws);
        assert_eq!(encoded, vec![0xa0]); // empty map
    }

    #[test]
    fn test_encode_witness_set_with_vkeys() {
        let ws = TransactionWitnessSet {
            vkey_witnesses: vec![VKeyWitness {
                vkey: vec![0u8; 32],
                signature: vec![0u8; 64],
            }],
            native_scripts: vec![],
            bootstrap_witnesses: vec![],
            plutus_v1_scripts: vec![],
            plutus_v2_scripts: vec![],
            plutus_v3_scripts: vec![],
            plutus_data: vec![],
            redeemers: vec![],
        };
        let encoded = encode_witness_set(&ws);
        assert_eq!(encoded[0], 0xa1); // map of 1
    }

    #[test]
    fn test_encode_transaction_body_minimal() {
        let body = TransactionBody {
            inputs: vec![TransactionInput {
                transaction_id: Hash32::ZERO,
                index: 0,
            }],
            outputs: vec![TransactionOutput {
                address: Address::Enterprise(EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Mainnet,
                    payment: Credential::VerificationKey(Hash28::from_bytes([0u8; 28])),
                }),
                value: Value::lovelace(1_000_000),
                datum: OutputDatum::None,
                script_ref: None,
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
        };

        let encoded = encode_transaction_body(&body);
        assert_eq!(encoded[0], 0xa3); // map of 3 (inputs, outputs, fee)
    }

    #[test]
    fn test_encode_transaction_roundtrip_hash() {
        let body = TransactionBody {
            inputs: vec![TransactionInput {
                transaction_id: Hash32::ZERO,
                index: 0,
            }],
            outputs: vec![TransactionOutput {
                address: Address::Enterprise(EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Mainnet,
                    payment: Credential::VerificationKey(Hash28::from_bytes([0u8; 28])),
                }),
                value: Value::lovelace(1_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            }],
            fee: Lovelace(200_000),
            ttl: Some(SlotNo(1000)),
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

        // Hash should be deterministic
        let hash1 = compute_transaction_hash(&body);
        let hash2 = compute_transaction_hash(&body);
        assert_eq!(hash1, hash2);
        assert_ne!(hash1, Hash32::ZERO);
    }

    #[test]
    fn test_encode_transaction_complete() {
        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![TransactionInput {
                    transaction_id: Hash32::ZERO,
                    index: 0,
                }],
                outputs: vec![TransactionOutput {
                    address: Address::Enterprise(EnterpriseAddress {
                        network: torsten_primitives::network::NetworkId::Mainnet,
                        payment: Credential::VerificationKey(Hash28::from_bytes([0u8; 28])),
                    }),
                    value: Value::lovelace(1_000_000),
                    datum: OutputDatum::None,
                    script_ref: None,
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
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let encoded = encode_transaction(&tx);
        assert_eq!(encoded[0], 0x84); // array of 4
    }

    #[test]
    fn test_encode_block_header_body() {
        let header = BlockHeader {
            header_hash: Hash32::ZERO,
            prev_hash: Hash32::from_bytes([1u8; 32]),
            issuer_vkey: vec![0u8; 32],
            vrf_vkey: vec![0u8; 32],
            vrf_result: VrfOutput {
                output: vec![0u8; 64],
                proof: vec![0u8; 80],
            },
            block_number: BlockNo(100),
            slot: SlotNo(500),
            epoch_nonce: Hash32::ZERO,
            body_size: 1024,
            body_hash: Hash32::ZERO,
            operational_cert: OperationalCert {
                hot_vkey: vec![0u8; 32],
                sequence_number: 1,
                kes_period: 200,
                sigma: vec![0u8; 64],
            },
            protocol_version: ProtocolVersion { major: 9, minor: 0 },
            kes_signature: vec![],
        };

        let encoded = encode_block_header_body(&header);
        assert_eq!(encoded[0], 0x8a); // array of 10
    }

    #[test]
    fn test_encode_block_complete() {
        let block = Block {
            header: BlockHeader {
                header_hash: Hash32::ZERO,
                prev_hash: Hash32::from_bytes([1u8; 32]),
                issuer_vkey: vec![0u8; 32],
                vrf_vkey: vec![0u8; 32],
                vrf_result: VrfOutput {
                    output: vec![0u8; 64],
                    proof: vec![0u8; 80],
                },
                block_number: BlockNo(100),
                slot: SlotNo(500),
                epoch_nonce: Hash32::ZERO,
                body_size: 0,
                body_hash: Hash32::ZERO,
                operational_cert: OperationalCert {
                    hot_vkey: vec![0u8; 32],
                    sequence_number: 1,
                    kes_period: 200,
                    sigma: vec![0u8; 64],
                },
                protocol_version: ProtocolVersion { major: 9, minor: 0 },
                kes_signature: vec![],
            },
            transactions: vec![],
            era: Era::Conway,
            raw_cbor: None,
        };

        let kes_sig = vec![0u8; 448]; // KES signature placeholder
        let encoded = encode_block(&block, &kes_sig);
        assert_eq!(encoded[0], 0x82); // outer array of 2 [era_tag, block]
        assert_eq!(encoded[1], 0x07); // era 7 (Conway)
    }

    #[test]
    fn test_encode_auxiliary_data_simple() {
        let mut metadata = BTreeMap::new();
        metadata.insert(1u64, TransactionMetadatum::Text("hello".to_string()));

        let aux = AuxiliaryData {
            metadata,
            native_scripts: vec![],
            plutus_v1_scripts: vec![],
            plutus_v2_scripts: vec![],
            plutus_v3_scripts: vec![],
        };

        let encoded = encode_auxiliary_data(&aux);
        assert_eq!(encoded[0], 0xa1); // map of 1
    }

    #[test]
    fn test_compute_block_body_hash() {
        let hash = compute_block_body_hash(&[]);
        // Hash of empty array (CBOR: 0x80)
        assert_ne!(hash, Hash32::ZERO);
    }

    #[test]
    fn test_encode_redeemer() {
        use script::encode_redeemer;
        let r = Redeemer {
            tag: RedeemerTag::Spend,
            index: 0,
            data: PlutusData::Integer(42),
            ex_units: ExUnits {
                mem: 100000,
                steps: 200000,
            },
        };
        let encoded = encode_redeemer(&r);
        assert_eq!(encoded[0], 0x84); // array of 4
    }

    #[test]
    fn test_encode_drep_variants() {
        use governance::encode_drep;
        let abstain = encode_drep(&DRep::Abstain);
        assert_eq!(abstain, vec![0x81, 0x02]); // [2]

        let no_conf = encode_drep(&DRep::NoConfidence);
        assert_eq!(no_conf, vec![0x81, 0x03]); // [3]

        let key = encode_drep(&DRep::KeyHash(Hash32::ZERO));
        assert_eq!(key[0], 0x82); // [0, hash]
    }

    #[test]
    fn test_encode_certificate_conway_drep() {
        let cert = Certificate::RegDRep {
            credential: Credential::VerificationKey(Hash28::from_bytes([0u8; 28])),
            deposit: Lovelace(500_000_000),
            anchor: Some(Anchor {
                url: "https://example.com".to_string(),
                data_hash: Hash32::ZERO,
            }),
        };
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x84); // array of 4
    }

    #[test]
    fn test_language_views_v1_double_bagged_key() {
        use script::encode_language_views;
        let cost_models = CostModels {
            plutus_v1: Some(vec![100, 200]),
            plutus_v2: None,
            plutus_v3: None,
        };
        let encoded = encode_language_views(&cost_models, true, false, false);
        // map(1), key = bstr(0x00) = [0x41, 0x00]
        assert_eq!(encoded[0], 0xA1); // map(1)
        assert_eq!(encoded[1], 0x41); // bstr(1)
        assert_eq!(encoded[2], 0x00); // inner byte 0x00
                                      // value starts at [3]: bstr wrapping indefinite array
        assert!(
            encoded[3] >= 0x40 && encoded[3] <= 0x5F,
            "Expected bstr header at [3], got 0x{:02X}",
            encoded[3],
        );
    }

    #[test]
    fn test_language_views_v1_indefinite_array() {
        use script::encode_language_views;
        let cost_models = CostModels {
            plutus_v1: Some(vec![1, 2, 3]),
            plutus_v2: None,
            plutus_v3: None,
        };
        let encoded = encode_language_views(&cost_models, true, false, false);
        // The value should be a bstr containing [0x9F, <ints>, 0xFF]
        // Skip map header (1 byte) and key (2 bytes) to get the value
        let value_start = 3;
        // Parse the bstr: first byte tells us the bstr header
        let (bstr_content_start, bstr_len) = if encoded[value_start] < 0x58 {
            // bstr with 1-byte header
            let len = (encoded[value_start] - 0x40) as usize;
            (value_start + 1, len)
        } else {
            // bstr with 2-byte header (0x58 NN)
            let len = encoded[value_start + 1] as usize;
            (value_start + 2, len)
        };
        let inner = &encoded[bstr_content_start..bstr_content_start + bstr_len];
        // First byte should be 0x9F (indefinite array start)
        assert_eq!(inner[0], 0x9F);
        // Last byte should be 0xFF (break)
        assert_eq!(inner[inner.len() - 1], 0xFF);
    }

    #[test]
    fn test_language_views_v2_definite_array() {
        use script::encode_language_views;
        let cost_models = CostModels {
            plutus_v1: None,
            plutus_v2: Some(vec![10, 20]),
            plutus_v3: None,
        };
        let encoded = encode_language_views(&cost_models, false, true, false);
        // map(1), key = uint(1) = [0x01]
        assert_eq!(encoded[0], 0xA1); // map(1)
        assert_eq!(encoded[1], 0x01); // uint 1
                                      // value: definite-length array, NOT byte-wrapped
        assert_eq!(encoded[2], 0x82); // array(2)
    }

    #[test]
    fn test_language_views_sort_order() {
        use script::encode_language_views;
        // When V1 and V2 both present, V2 sorts first (1-byte key < 2-byte key)
        let cost_models = CostModels {
            plutus_v1: Some(vec![1]),
            plutus_v2: Some(vec![2]),
            plutus_v3: None,
        };
        let encoded = encode_language_views(&cost_models, true, true, false);
        assert_eq!(encoded[0], 0xA2); // map(2)
                                      // First entry should be V2 (key = 0x01, 1 byte)
        assert_eq!(encoded[1], 0x01); // V2 key
                                      // Not V1's double-bagged key (0x41, 0x00)
        assert_ne!(encoded[1], 0x41);
    }

    #[test]
    fn test_language_views_all_three_sort_order() {
        use script::encode_language_views;
        let cost_models = CostModels {
            plutus_v1: Some(vec![1]),
            plutus_v2: Some(vec![2]),
            plutus_v3: Some(vec![3]),
        };
        let encoded = encode_language_views(&cost_models, true, true, true);
        assert_eq!(encoded[0], 0xA3); // map(3)
                                      // Order: V2 (0x01), V3 (0x02), V1 (0x41 0x00)
        assert_eq!(encoded[1], 0x01); // V2 key first
                                      // Find V3 key after V2 value
                                      // V2 value: array(1) + int(2) = [0x81, 0x02]
        assert_eq!(encoded[2], 0x81); // array(1) for V2
        assert_eq!(encoded[3], 0x02); // int 2 for V2
        assert_eq!(encoded[4], 0x02); // V3 key second
    }

    #[test]
    fn test_language_views_empty() {
        use script::encode_language_views;
        let cost_models = CostModels {
            plutus_v1: None,
            plutus_v2: None,
            plutus_v3: None,
        };
        let encoded = encode_language_views(&cost_models, false, false, false);
        assert_eq!(encoded, encode_map_header(0));
    }

    #[test]
    fn test_encode_protocol_param_update_empty() {
        use protocol_params::encode_protocol_param_update;
        let ppu = ProtocolParamUpdate::default();
        let encoded = encode_protocol_param_update(&ppu);
        // Empty update = empty map
        assert_eq!(encoded, encode_map_header(0));
    }

    #[test]
    fn test_encode_protocol_param_update_basic_fields() {
        use protocol_params::encode_protocol_param_update;
        let ppu = ProtocolParamUpdate {
            min_fee_a: Some(44),
            min_fee_b: Some(155381),
            max_tx_size: Some(16384),
            ..Default::default()
        };
        let encoded = encode_protocol_param_update(&ppu);

        let mut dec = minicbor::Decoder::new(&encoded);
        let map_len = dec.map().unwrap().unwrap();
        assert_eq!(map_len, 3); // 3 fields set

        // Key 0: min_fee_a = 44
        assert_eq!(dec.u64().unwrap(), 0);
        assert_eq!(dec.u64().unwrap(), 44);
        // Key 1: min_fee_b = 155381
        assert_eq!(dec.u64().unwrap(), 1);
        assert_eq!(dec.u64().unwrap(), 155381);
        // Key 3: max_tx_size = 16384
        assert_eq!(dec.u64().unwrap(), 3);
        assert_eq!(dec.u64().unwrap(), 16384);
    }

    #[test]
    fn test_encode_protocol_param_update_governance_thresholds() {
        use protocol_params::encode_protocol_param_update;
        let ppu = ProtocolParamUpdate {
            pvt_motion_no_confidence: Some(Rational {
                numerator: 51,
                denominator: 100,
            }),
            dvt_hard_fork: Some(Rational {
                numerator: 3,
                denominator: 5,
            }),
            drep_deposit: Some(Lovelace(500_000_000)),
            ..Default::default()
        };
        let encoded = encode_protocol_param_update(&ppu);

        let mut dec = minicbor::Decoder::new(&encoded);
        let map_len = dec.map().unwrap().unwrap();
        assert_eq!(map_len, 3); // pvt group (key 22), dvt group (key 23), drep_deposit (key 28)
    }

    #[test]
    fn test_encode_protocol_param_update_execution_costs() {
        use protocol_params::encode_protocol_param_update;
        let ppu = ProtocolParamUpdate {
            execution_costs: Some(ExUnitPrices {
                mem_price: Rational {
                    numerator: 577,
                    denominator: 10000,
                },
                step_price: Rational {
                    numerator: 721,
                    denominator: 10000000,
                },
            }),
            max_tx_ex_units: Some(ExUnits {
                mem: 14_000_000,
                steps: 10_000_000_000,
            }),
            ..Default::default()
        };
        let encoded = encode_protocol_param_update(&ppu);

        let mut dec = minicbor::Decoder::new(&encoded);
        let map_len = dec.map().unwrap().unwrap();
        assert_eq!(map_len, 2); // keys 16 and 17
    }

    #[test]
    fn test_encode_protocol_param_update_cost_models() {
        use protocol_params::encode_protocol_param_update;
        let ppu = ProtocolParamUpdate {
            cost_models: Some(CostModels {
                plutus_v1: None,
                plutus_v2: Some(vec![100, 200, 300]),
                plutus_v3: None,
            }),
            ..Default::default()
        };
        let encoded = encode_protocol_param_update(&ppu);

        let mut dec = minicbor::Decoder::new(&encoded);
        let map_len = dec.map().unwrap().unwrap();
        assert_eq!(map_len, 1); // key 15 only

        // Key 15
        assert_eq!(dec.u64().unwrap(), 15);
        // Cost models map: {1: [100, 200, 300]}
        let cm_map_len = dec.map().unwrap().unwrap();
        assert_eq!(cm_map_len, 1);
        assert_eq!(dec.u64().unwrap(), 1); // plutus v2 key
        let arr_len = dec.array().unwrap().unwrap();
        assert_eq!(arr_len, 3);
        assert_eq!(dec.i64().unwrap(), 100);
        assert_eq!(dec.i64().unwrap(), 200);
        assert_eq!(dec.i64().unwrap(), 300);
    }

    #[test]
    fn test_encode_gov_action_parameter_change() {
        use governance::encode_gov_action;
        let action = GovAction::ParameterChange {
            prev_action_id: None,
            protocol_param_update: Box::new(ProtocolParamUpdate {
                min_fee_a: Some(44),
                key_deposit: Some(Lovelace(2_000_000)),
                ..Default::default()
            }),
            policy_hash: None,
        };
        let encoded = encode_gov_action(&action);

        let mut dec = minicbor::Decoder::new(&encoded);
        let arr_len = dec.array().unwrap().unwrap();
        assert_eq!(arr_len, 4); // [tag, prev_id, ppu_map, policy_hash]
        assert_eq!(dec.u64().unwrap(), 0); // ParameterChange tag = 0
    }
}
