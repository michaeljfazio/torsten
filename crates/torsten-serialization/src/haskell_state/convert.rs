//! Conversion from Haskell intermediate types to Torsten primitive types.
//!
//! This module provides helper functions that map `HaskellNewEpochState`
//! sub-types to the equivalent Torsten primitive types.  Since the
//! `torsten-serialization` crate cannot depend on `torsten-ledger`, this
//! module only converts into types from `torsten-primitives`.  The node
//! integration layer (`torsten-node::haskell_ledger`) uses these helpers
//! together with ledger-specific types to build a full `LedgerState`.

use super::types::*;
use torsten_primitives::hash::{Hash28, Hash32};
use torsten_primitives::protocol_params::ProtocolParameters;
use torsten_primitives::transaction::{
    Anchor, Constitution, CostModels, DRep, ExUnitPrices, ExUnits, GovActionId, Rational, Relay,
};
use torsten_primitives::value::Lovelace;

/// Converted pool parameters (uses only `torsten-primitives` types).
#[derive(Debug, Clone)]
pub struct ConvertedPoolParams {
    pub pool_id: Hash28,
    pub vrf_keyhash: Hash32,
    pub pledge: Lovelace,
    pub cost: Lovelace,
    pub margin_numerator: u64,
    pub margin_denominator: u64,
    pub reward_account: Vec<u8>,
    pub owners: Vec<Hash28>,
    pub relays: Vec<Relay>,
    pub metadata_url: Option<String>,
    pub metadata_hash: Option<Hash32>,
}

/// Convert `HaskellPParams` to `ProtocolParameters`.
pub fn convert_pparams(h: &HaskellPParams) -> ProtocolParameters {
    // Build cost models from the HashMap<u8, Vec<i64>>
    let plutus_v1 = h.cost_models.get(&0).cloned();
    let plutus_v2 = h.cost_models.get(&1).cloned();
    let plutus_v3 = h.cost_models.get(&2).cloned();

    ProtocolParameters {
        min_fee_a: h.min_fee_a,
        min_fee_b: h.min_fee_b,
        max_block_body_size: h.max_block_body_size,
        max_tx_size: h.max_tx_size,
        max_block_header_size: h.max_block_header_size,
        key_deposit: Lovelace(h.key_deposit),
        pool_deposit: Lovelace(h.pool_deposit),
        e_max: h.e_max,
        n_opt: h.n_opt,
        a0: Rational {
            numerator: h.a0_num,
            denominator: h.a0_den,
        },
        rho: Rational {
            numerator: h.rho_num,
            denominator: h.rho_den,
        },
        tau: Rational {
            numerator: h.tau_num,
            denominator: h.tau_den,
        },
        min_pool_cost: Lovelace(h.min_pool_cost),
        ada_per_utxo_byte: Lovelace(h.ada_per_utxo_byte),
        cost_models: CostModels {
            plutus_v1,
            plutus_v2,
            plutus_v3,
        },
        execution_costs: ExUnitPrices {
            mem_price: Rational {
                numerator: h.prices_mem_num,
                denominator: h.prices_mem_den,
            },
            step_price: Rational {
                numerator: h.prices_step_num,
                denominator: h.prices_step_den,
            },
        },
        max_tx_ex_units: ExUnits {
            mem: h.max_tx_ex_units_mem,
            steps: h.max_tx_ex_units_steps,
        },
        max_block_ex_units: ExUnits {
            mem: h.max_block_ex_units_mem,
            steps: h.max_block_ex_units_steps,
        },
        max_val_size: h.max_val_size,
        collateral_percentage: h.collateral_percentage,
        max_collateral_inputs: h.max_collateral_inputs,
        min_fee_ref_script_cost_per_byte: if h.min_fee_ref_script_cost_per_byte_den != 0 {
            h.min_fee_ref_script_cost_per_byte_num / h.min_fee_ref_script_cost_per_byte_den
        } else {
            0
        },
        drep_deposit: Lovelace(h.drep_deposit),
        drep_activity: h.drep_activity,
        gov_action_deposit: Lovelace(h.gov_action_deposit),
        gov_action_lifetime: h.gov_action_lifetime,
        committee_min_size: h.committee_min_size,
        committee_max_term_length: h.committee_max_term_length,
        dvt_pp_network_group: Rational {
            numerator: h.dvt_pp_network_group_num,
            denominator: h.dvt_pp_network_group_den,
        },
        dvt_pp_economic_group: Rational {
            numerator: h.dvt_pp_economic_group_num,
            denominator: h.dvt_pp_economic_group_den,
        },
        dvt_pp_technical_group: Rational {
            numerator: h.dvt_pp_technical_group_num,
            denominator: h.dvt_pp_technical_group_den,
        },
        dvt_pp_gov_group: Rational {
            numerator: h.dvt_pp_gov_group_num,
            denominator: h.dvt_pp_gov_group_den,
        },
        dvt_hard_fork: Rational {
            numerator: h.dvt_hard_fork_num,
            denominator: h.dvt_hard_fork_den,
        },
        dvt_no_confidence: Rational {
            numerator: h.dvt_motion_no_confidence_num,
            denominator: h.dvt_motion_no_confidence_den,
        },
        dvt_committee_normal: Rational {
            numerator: h.dvt_committee_normal_num,
            denominator: h.dvt_committee_normal_den,
        },
        dvt_committee_no_confidence: Rational {
            numerator: h.dvt_committee_no_confidence_num,
            denominator: h.dvt_committee_no_confidence_den,
        },
        dvt_constitution: Rational {
            numerator: h.dvt_update_constitution_num,
            denominator: h.dvt_update_constitution_den,
        },
        dvt_treasury_withdrawal: Rational {
            numerator: h.dvt_treasury_withdrawal_num,
            denominator: h.dvt_treasury_withdrawal_den,
        },
        pvt_motion_no_confidence: Rational {
            numerator: h.pvt_motion_no_confidence_num,
            denominator: h.pvt_motion_no_confidence_den,
        },
        pvt_committee_normal: Rational {
            numerator: h.pvt_committee_normal_num,
            denominator: h.pvt_committee_normal_den,
        },
        pvt_committee_no_confidence: Rational {
            numerator: h.pvt_committee_no_confidence_num,
            denominator: h.pvt_committee_no_confidence_den,
        },
        pvt_hard_fork: Rational {
            numerator: h.pvt_hard_fork_num,
            denominator: h.pvt_hard_fork_den,
        },
        pvt_pp_security_group: Rational {
            numerator: h.pvt_pp_security_group_num,
            denominator: h.pvt_pp_security_group_den,
        },
        protocol_version_major: h.protocol_version_major,
        protocol_version_minor: h.protocol_version_minor,
        active_slots_coeff: 0.05, // default; overridden from genesis later
    }
}

/// Convert `HaskellCredential` to `Hash32` by padding the 28-byte hash to 32 bytes.
///
/// Torsten stores stake credentials as `Hash32` (padded from 28 bytes with
/// trailing zeros) for uniform key sizes in `HashMap`.
pub fn credential_to_hash32(cred: &HaskellCredential) -> Hash32 {
    let h28 = match cred {
        HaskellCredential::KeyHash(h) => h,
        HaskellCredential::ScriptHash(h) => h,
    };
    h28.to_hash32_padded()
}

/// Convert `HaskellCredential` to `Hash28`.
pub fn credential_to_hash28(cred: &HaskellCredential) -> Hash28 {
    match cred {
        HaskellCredential::KeyHash(h) => *h,
        HaskellCredential::ScriptHash(h) => *h,
    }
}

/// Convert `HaskellAnchor` to `Anchor`.
pub fn convert_anchor(h: &HaskellAnchor) -> Anchor {
    Anchor {
        url: h.url.clone(),
        data_hash: h.data_hash,
    }
}

/// Convert `HaskellPoolParams` to `ConvertedPoolParams`.
pub fn convert_pool_params(h: &HaskellPoolParams) -> ConvertedPoolParams {
    let relays: Vec<Relay> = h
        .relays
        .iter()
        .map(|r| match r {
            HaskellRelay::SingleHostAddr { port, ipv4, ipv6 } => Relay::SingleHostAddr {
                port: *port,
                ipv4: *ipv4,
                ipv6: *ipv6,
            },
            HaskellRelay::SingleHostName { port, dns_name } => Relay::SingleHostName {
                port: *port,
                dns_name: dns_name.clone(),
            },
            HaskellRelay::MultiHostName { dns_name } => Relay::MultiHostName {
                dns_name: dns_name.clone(),
            },
        })
        .collect();

    let (metadata_url, metadata_hash) = match &h.metadata {
        Some(meta) => (Some(meta.url.clone()), Some(meta.hash)),
        None => (None, None),
    };

    ConvertedPoolParams {
        pool_id: h.operator,
        vrf_keyhash: h.vrf_keyhash,
        pledge: h.pledge,
        cost: h.cost,
        margin_numerator: h.margin_numerator,
        margin_denominator: h.margin_denominator,
        reward_account: h.reward_account.clone(),
        owners: h.owners.clone(),
        relays,
        metadata_url,
        metadata_hash,
    }
}

/// Convert `HaskellDRep` to `DRep`.
pub fn convert_drep(h: &HaskellDRep) -> DRep {
    match h {
        HaskellDRep::KeyHash(h28) => DRep::KeyHash(h28.to_hash32_padded()),
        HaskellDRep::ScriptHash(h28) => DRep::ScriptHash(*h28),
        HaskellDRep::Abstain => DRep::Abstain,
        HaskellDRep::NoConfidence => DRep::NoConfidence,
    }
}

/// Convert `HaskellGovActionId` to `GovActionId`.
pub fn convert_gov_action_id(h: &HaskellGovActionId) -> GovActionId {
    GovActionId {
        transaction_id: h.tx_id,
        action_index: h.action_index,
    }
}

/// Convert `HaskellConstitution` to `Constitution`.
pub fn convert_constitution(h: &HaskellConstitution) -> Constitution {
    Constitution {
        anchor: convert_anchor(&h.anchor),
        script_hash: h.guardrail_script.map(|h32| {
            // ScriptHash is Hash28 — extract the first 28 bytes from Hash32
            let mut bytes = [0u8; 28];
            bytes.copy_from_slice(&h32.as_bytes()[..28]);
            Hash28::from_bytes(bytes)
        }),
    }
}

/// Convert `HaskellRational` to `Rational`.
pub fn convert_rational(h: &HaskellRational) -> Rational {
    Rational {
        numerator: h.numerator,
        denominator: h.denominator,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn sample_pparams() -> HaskellPParams {
        HaskellPParams {
            min_fee_a: 44,
            min_fee_b: 155381,
            max_block_body_size: 90112,
            max_tx_size: 16384,
            max_block_header_size: 1100,
            key_deposit: 2_000_000,
            pool_deposit: 500_000_000,
            e_max: 18,
            n_opt: 500,
            a0_num: 3,
            a0_den: 10,
            rho_num: 3,
            rho_den: 1000,
            tau_num: 2,
            tau_den: 10,
            protocol_version_major: 9,
            protocol_version_minor: 0,
            min_pool_cost: 170_000_000,
            ada_per_utxo_byte: 4310,
            cost_models: HashMap::new(),
            prices_mem_num: 577,
            prices_mem_den: 10000,
            prices_step_num: 721,
            prices_step_den: 10000000,
            max_tx_ex_units_mem: 14_000_000,
            max_tx_ex_units_steps: 10_000_000_000,
            max_block_ex_units_mem: 62_000_000,
            max_block_ex_units_steps: 40_000_000_000,
            max_val_size: 5000,
            collateral_percentage: 150,
            max_collateral_inputs: 3,
            pvt_motion_no_confidence_num: 51,
            pvt_motion_no_confidence_den: 100,
            pvt_committee_normal_num: 51,
            pvt_committee_normal_den: 100,
            pvt_committee_no_confidence_num: 51,
            pvt_committee_no_confidence_den: 100,
            pvt_hard_fork_num: 51,
            pvt_hard_fork_den: 100,
            pvt_pp_security_group_num: 51,
            pvt_pp_security_group_den: 100,
            dvt_motion_no_confidence_num: 67,
            dvt_motion_no_confidence_den: 100,
            dvt_committee_normal_num: 67,
            dvt_committee_normal_den: 100,
            dvt_committee_no_confidence_num: 60,
            dvt_committee_no_confidence_den: 100,
            dvt_update_constitution_num: 75,
            dvt_update_constitution_den: 100,
            dvt_hard_fork_num: 60,
            dvt_hard_fork_den: 100,
            dvt_pp_network_group_num: 67,
            dvt_pp_network_group_den: 100,
            dvt_pp_economic_group_num: 67,
            dvt_pp_economic_group_den: 100,
            dvt_pp_technical_group_num: 67,
            dvt_pp_technical_group_den: 100,
            dvt_pp_gov_group_num: 67,
            dvt_pp_gov_group_den: 100,
            dvt_treasury_withdrawal_num: 67,
            dvt_treasury_withdrawal_den: 100,
            committee_min_size: 7,
            committee_max_term_length: 146,
            gov_action_lifetime: 6,
            gov_action_deposit: 100_000_000_000,
            drep_deposit: 500_000_000,
            drep_activity: 20,
            min_fee_ref_script_cost_per_byte_num: 15,
            min_fee_ref_script_cost_per_byte_den: 1,
        }
    }

    #[test]
    fn test_convert_pparams_basic_fields() {
        let hp = sample_pparams();
        let pp = convert_pparams(&hp);

        assert_eq!(pp.min_fee_a, 44);
        assert_eq!(pp.min_fee_b, 155381);
        assert_eq!(pp.max_block_body_size, 90112);
        assert_eq!(pp.max_tx_size, 16384);
        assert_eq!(pp.max_block_header_size, 1100);
        assert_eq!(pp.key_deposit, Lovelace(2_000_000));
        assert_eq!(pp.pool_deposit, Lovelace(500_000_000));
        assert_eq!(pp.e_max, 18);
        assert_eq!(pp.n_opt, 500);
        assert_eq!(pp.protocol_version_major, 9);
        assert_eq!(pp.protocol_version_minor, 0);
    }

    #[test]
    fn test_convert_pparams_rationals() {
        let hp = sample_pparams();
        let pp = convert_pparams(&hp);

        assert_eq!(pp.a0.numerator, 3);
        assert_eq!(pp.a0.denominator, 10);
        assert_eq!(pp.rho.numerator, 3);
        assert_eq!(pp.rho.denominator, 1000);
        assert_eq!(pp.tau.numerator, 2);
        assert_eq!(pp.tau.denominator, 10);
    }

    #[test]
    fn test_convert_pparams_execution_costs() {
        let hp = sample_pparams();
        let pp = convert_pparams(&hp);

        assert_eq!(pp.execution_costs.mem_price.numerator, 577);
        assert_eq!(pp.execution_costs.mem_price.denominator, 10000);
        assert_eq!(pp.execution_costs.step_price.numerator, 721);
        assert_eq!(pp.execution_costs.step_price.denominator, 10000000);
        assert_eq!(pp.max_tx_ex_units.mem, 14_000_000);
        assert_eq!(pp.max_tx_ex_units.steps, 10_000_000_000);
        assert_eq!(pp.max_block_ex_units.mem, 62_000_000);
        assert_eq!(pp.max_block_ex_units.steps, 40_000_000_000);
    }

    #[test]
    fn test_convert_pparams_governance_thresholds() {
        let hp = sample_pparams();
        let pp = convert_pparams(&hp);

        assert_eq!(pp.pvt_motion_no_confidence.numerator, 51);
        assert_eq!(pp.pvt_motion_no_confidence.denominator, 100);
        assert_eq!(pp.dvt_pp_network_group.numerator, 67);
        assert_eq!(pp.dvt_pp_network_group.denominator, 100);
        assert_eq!(pp.dvt_constitution.numerator, 75);
        assert_eq!(pp.dvt_constitution.denominator, 100);
        assert_eq!(pp.dvt_hard_fork.numerator, 60);
        assert_eq!(pp.dvt_hard_fork.denominator, 100);
    }

    #[test]
    fn test_convert_pparams_conway_fields() {
        let hp = sample_pparams();
        let pp = convert_pparams(&hp);

        assert_eq!(pp.drep_deposit, Lovelace(500_000_000));
        assert_eq!(pp.drep_activity, 20);
        assert_eq!(pp.gov_action_deposit, Lovelace(100_000_000_000));
        assert_eq!(pp.gov_action_lifetime, 6);
        assert_eq!(pp.committee_min_size, 7);
        assert_eq!(pp.committee_max_term_length, 146);
        assert_eq!(pp.min_fee_ref_script_cost_per_byte, 15);
    }

    #[test]
    fn test_convert_pparams_cost_models() {
        let mut hp = sample_pparams();
        hp.cost_models.insert(0, vec![100, 200, 300]); // PlutusV1
        hp.cost_models.insert(1, vec![400, 500]); // PlutusV2
        hp.cost_models.insert(2, vec![600, 700, 800, 900]); // PlutusV3
        let pp = convert_pparams(&hp);

        assert_eq!(pp.cost_models.plutus_v1, Some(vec![100, 200, 300]));
        assert_eq!(pp.cost_models.plutus_v2, Some(vec![400, 500]));
        assert_eq!(pp.cost_models.plutus_v3, Some(vec![600, 700, 800, 900]));
    }

    #[test]
    fn test_convert_pparams_ref_script_cost_zero_den() {
        let mut hp = sample_pparams();
        hp.min_fee_ref_script_cost_per_byte_den = 0;
        let pp = convert_pparams(&hp);
        assert_eq!(pp.min_fee_ref_script_cost_per_byte, 0);
    }

    #[test]
    fn test_credential_to_hash32_keyhash() {
        let h28 = Hash28::from_bytes([0xAB; 28]);
        let cred = HaskellCredential::KeyHash(h28);
        let h32 = credential_to_hash32(&cred);
        assert_eq!(&h32.as_bytes()[..28], h28.as_bytes());
        assert_eq!(&h32.as_bytes()[28..], &[0u8; 4]);
    }

    #[test]
    fn test_credential_to_hash32_scripthash() {
        let h28 = Hash28::from_bytes([0xCD; 28]);
        let cred = HaskellCredential::ScriptHash(h28);
        let h32 = credential_to_hash32(&cred);
        assert_eq!(&h32.as_bytes()[..28], h28.as_bytes());
        assert_eq!(&h32.as_bytes()[28..], &[0u8; 4]);
    }

    #[test]
    fn test_credential_to_hash28() {
        let h28 = Hash28::from_bytes([0x11; 28]);
        let cred = HaskellCredential::KeyHash(h28);
        assert_eq!(credential_to_hash28(&cred), h28);

        let h28s = Hash28::from_bytes([0x22; 28]);
        let cred_s = HaskellCredential::ScriptHash(h28s);
        assert_eq!(credential_to_hash28(&cred_s), h28s);
    }

    #[test]
    fn test_convert_anchor() {
        let ha = HaskellAnchor {
            url: "https://example.com/metadata.json".to_string(),
            data_hash: Hash32::from_bytes([0xFF; 32]),
        };
        let a = convert_anchor(&ha);
        assert_eq!(a.url, "https://example.com/metadata.json");
        assert_eq!(a.data_hash, Hash32::from_bytes([0xFF; 32]));
    }

    #[test]
    fn test_convert_pool_params_basic() {
        let hp = HaskellPoolParams {
            operator: Hash28::from_bytes([0x01; 28]),
            vrf_keyhash: Hash32::from_bytes([0x02; 32]),
            pledge: Lovelace(100_000_000),
            cost: Lovelace(340_000_000),
            margin_numerator: 1,
            margin_denominator: 100,
            reward_account: vec![0xe0, 0x01, 0x02],
            owners: vec![
                Hash28::from_bytes([0x03; 28]),
                Hash28::from_bytes([0x04; 28]),
            ],
            relays: vec![],
            metadata: None,
        };
        let cp = convert_pool_params(&hp);
        assert_eq!(cp.pool_id, Hash28::from_bytes([0x01; 28]));
        assert_eq!(cp.vrf_keyhash, Hash32::from_bytes([0x02; 32]));
        assert_eq!(cp.pledge, Lovelace(100_000_000));
        assert_eq!(cp.cost, Lovelace(340_000_000));
        assert_eq!(cp.margin_numerator, 1);
        assert_eq!(cp.margin_denominator, 100);
        assert_eq!(cp.owners.len(), 2);
        assert!(cp.metadata_url.is_none());
        assert!(cp.metadata_hash.is_none());
    }

    #[test]
    fn test_convert_pool_params_with_relays() {
        let hp = HaskellPoolParams {
            operator: Hash28::ZERO,
            vrf_keyhash: Hash32::ZERO,
            pledge: Lovelace(0),
            cost: Lovelace(0),
            margin_numerator: 0,
            margin_denominator: 1,
            reward_account: vec![],
            owners: vec![],
            relays: vec![
                HaskellRelay::SingleHostAddr {
                    port: Some(3001),
                    ipv4: Some([127, 0, 0, 1]),
                    ipv6: None,
                },
                HaskellRelay::SingleHostName {
                    port: Some(3001),
                    dns_name: "relay.example.com".to_string(),
                },
                HaskellRelay::MultiHostName {
                    dns_name: "multi.example.com".to_string(),
                },
            ],
            metadata: Some(HaskellPoolMetadata {
                url: "https://pool.example.com/meta.json".to_string(),
                hash: Hash32::from_bytes([0xAA; 32]),
            }),
        };
        let cp = convert_pool_params(&hp);
        assert_eq!(cp.relays.len(), 3);
        match &cp.relays[0] {
            Relay::SingleHostAddr { port, ipv4, ipv6 } => {
                assert_eq!(*port, Some(3001));
                assert_eq!(*ipv4, Some([127, 0, 0, 1]));
                assert!(ipv6.is_none());
            }
            _ => panic!("expected SingleHostAddr"),
        }
        match &cp.relays[1] {
            Relay::SingleHostName { port, dns_name } => {
                assert_eq!(*port, Some(3001));
                assert_eq!(dns_name, "relay.example.com");
            }
            _ => panic!("expected SingleHostName"),
        }
        match &cp.relays[2] {
            Relay::MultiHostName { dns_name } => {
                assert_eq!(dns_name, "multi.example.com");
            }
            _ => panic!("expected MultiHostName"),
        }
        assert_eq!(
            cp.metadata_url.as_deref(),
            Some("https://pool.example.com/meta.json")
        );
        assert_eq!(cp.metadata_hash, Some(Hash32::from_bytes([0xAA; 32])));
    }

    #[test]
    fn test_convert_drep_keyhash() {
        let h28 = Hash28::from_bytes([0x01; 28]);
        let hdrep = HaskellDRep::KeyHash(h28);
        let drep = convert_drep(&hdrep);
        match drep {
            DRep::KeyHash(h32) => {
                assert_eq!(&h32.as_bytes()[..28], h28.as_bytes());
                assert_eq!(&h32.as_bytes()[28..], &[0u8; 4]);
            }
            _ => panic!("expected DRep::KeyHash"),
        }
    }

    #[test]
    fn test_convert_drep_scripthash() {
        let h28 = Hash28::from_bytes([0x02; 28]);
        let hdrep = HaskellDRep::ScriptHash(h28);
        let drep = convert_drep(&hdrep);
        match drep {
            DRep::ScriptHash(sh) => assert_eq!(sh, h28),
            _ => panic!("expected DRep::ScriptHash"),
        }
    }

    #[test]
    fn test_convert_drep_abstain_and_noconfidence() {
        assert!(matches!(convert_drep(&HaskellDRep::Abstain), DRep::Abstain));
        assert!(matches!(
            convert_drep(&HaskellDRep::NoConfidence),
            DRep::NoConfidence
        ));
    }

    #[test]
    fn test_convert_gov_action_id() {
        let hga = HaskellGovActionId {
            tx_id: Hash32::from_bytes([0xFF; 32]),
            action_index: 42,
        };
        let ga = convert_gov_action_id(&hga);
        assert_eq!(ga.transaction_id, Hash32::from_bytes([0xFF; 32]));
        assert_eq!(ga.action_index, 42);
    }

    #[test]
    fn test_convert_constitution_with_guardrail() {
        let hc = HaskellConstitution {
            anchor: HaskellAnchor {
                url: "https://constitution.example.com".to_string(),
                data_hash: Hash32::from_bytes([0x55; 32]),
            },
            guardrail_script: Some(Hash32::from_bytes([0x66; 32])),
        };
        let c = convert_constitution(&hc);
        assert_eq!(c.anchor.url, "https://constitution.example.com");
        assert_eq!(c.anchor.data_hash, Hash32::from_bytes([0x55; 32]));
        assert!(c.script_hash.is_some());
        let sh = c.script_hash.unwrap();
        // First 28 bytes should match
        assert_eq!(sh.as_bytes(), &[0x66; 28]);
    }

    #[test]
    fn test_convert_constitution_without_guardrail() {
        let hc = HaskellConstitution {
            anchor: HaskellAnchor {
                url: "https://example.com".to_string(),
                data_hash: Hash32::ZERO,
            },
            guardrail_script: None,
        };
        let c = convert_constitution(&hc);
        assert!(c.script_hash.is_none());
    }

    #[test]
    fn test_convert_rational() {
        let hr = HaskellRational {
            numerator: 3,
            denominator: 10,
        };
        let r = convert_rational(&hr);
        assert_eq!(r.numerator, 3);
        assert_eq!(r.denominator, 10);
    }

    #[test]
    fn test_credential_to_hash32_roundtrip() {
        // Verify that Hash28 -> Hash32 padding is consistent with to_hash32_padded
        let h28 = Hash28::from_bytes([0x42; 28]);
        let cred = HaskellCredential::KeyHash(h28);
        let h32 = credential_to_hash32(&cred);
        let expected = h28.to_hash32_padded();
        assert_eq!(h32, expected);
    }

    #[test]
    fn test_convert_pool_params_no_metadata() {
        let hp = HaskellPoolParams {
            operator: Hash28::ZERO,
            vrf_keyhash: Hash32::ZERO,
            pledge: Lovelace(0),
            cost: Lovelace(0),
            margin_numerator: 0,
            margin_denominator: 1,
            reward_account: vec![],
            owners: vec![],
            relays: vec![],
            metadata: None,
        };
        let cp = convert_pool_params(&hp);
        assert!(cp.metadata_url.is_none());
        assert!(cp.metadata_hash.is_none());
    }
}
