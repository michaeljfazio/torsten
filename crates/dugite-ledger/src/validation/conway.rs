//! Conway-era specific validation: era gating, governance checks, and
//! certificate deposit/refund accounting.
//!
//! This module handles:
//! - Ensuring Conway-only certificates and governance actions are rejected on
//!   pre-Conway protocol versions (Rule 1d).
//! - Calculating the net deposit and refund amounts for all certificate types
//!   across eras, including pool re-registration logic.

use std::collections::{HashMap, HashSet};

use dugite_primitives::hash::{Hash28, Hash32};
use dugite_primitives::protocol_params::ProtocolParameters;
use dugite_primitives::transaction::Certificate;

use super::ValidationError;

/// Return the human-readable certificate type name when the certificate is
/// Conway-only (requires protocol version >= 9). Returns `None` for
/// pre-Conway certificates that are valid in all post-Shelley eras.
pub(super) fn conway_only_certificate_name(cert: &Certificate) -> Option<&'static str> {
    match cert {
        Certificate::RegDRep { .. } => Some("RegDRep"),
        Certificate::UnregDRep { .. } => Some("UnregDRep"),
        Certificate::UpdateDRep { .. } => Some("UpdateDRep"),
        Certificate::VoteDelegation { .. } => Some("VoteDelegation"),
        Certificate::StakeVoteDelegation { .. } => Some("StakeVoteDelegation"),
        Certificate::CommitteeHotAuth { .. } => Some("CommitteeHotAuth"),
        Certificate::CommitteeColdResign { .. } => Some("CommitteeColdResign"),
        Certificate::RegStakeVoteDeleg { .. } => Some("RegStakeVoteDeleg"),
        Certificate::VoteRegDeleg { .. } => Some("VoteRegDeleg"),
        Certificate::ConwayStakeRegistration { .. } => Some("ConwayStakeRegistration"),
        Certificate::ConwayStakeDeregistration { .. } => Some("ConwayStakeDeregistration"),
        Certificate::RegStakeDeleg { .. } => Some("RegStakeDeleg"),
        // Pre-Conway certificates — valid in all post-Shelley eras
        Certificate::StakeRegistration(_)
        | Certificate::StakeDeregistration(_)
        | Certificate::StakeDelegation { .. }
        | Certificate::PoolRegistration(_)
        | Certificate::PoolRetirement { .. }
        | Certificate::GenesisKeyDelegation { .. }
        | Certificate::MoveInstantaneousRewards { .. } => None,
    }
}

/// Validate era-gating rules (Rule 1d).
///
/// Conway-specific certificates and governance features are only valid when the
/// current protocol major version is >= 9 (Conway era). Violations are pushed
/// onto `errors`.
pub(super) fn check_era_gating(
    params: &ProtocolParameters,
    body: &dugite_primitives::transaction::TransactionBody,
    errors: &mut Vec<ValidationError>,
) {
    let proto_major = params.protocol_version_major;
    let current_era_name = if proto_major >= 9 {
        "Conway"
    } else if proto_major >= 7 {
        "Babbage"
    } else if proto_major >= 6 {
        "Alonzo"
    } else if proto_major >= 4 {
        "Mary"
    } else {
        "Shelley"
    };

    if proto_major < 9 {
        for cert in &body.certificates {
            if let Some(cert_name) = conway_only_certificate_name(cert) {
                errors.push(ValidationError::EraGatingViolation {
                    certificate_type: cert_name.to_string(),
                    required_era: "Conway (protocol >= 9)".to_string(),
                    current_era: format!("{} (protocol {})", current_era_name, proto_major),
                });
            }
        }
        if !body.voting_procedures.is_empty() {
            errors.push(ValidationError::GovernancePreConway {
                current_version: proto_major,
            });
        }
        if !body.proposal_procedures.is_empty() {
            errors.push(ValidationError::GovernancePreConway {
                current_version: proto_major,
            });
        }
    }
}

/// Calculate total deposits and refunds from certificates in a transaction.
///
/// Deposits are charged for:
/// - Stake registration (pre-Conway: `key_deposit`, Conway: inline deposit amount)
/// - Pool registration (new pools only; re-registrations are free)
/// - DRep registration
/// - Combined registration+delegation certificates (RegStakeDeleg, RegStakeVoteDeleg,
///   VoteRegDeleg)
///
/// Refunds are returned for:
/// - Stake deregistration
/// - DRep unregistration
///
/// When `registered_pools` is `Some`, pool re-registrations (updating an existing
/// pool's parameters) do not charge an additional deposit — only new pool
/// registrations do. When `None`, all pool registrations are treated as new.
///
/// When `stake_key_deposits` is `Some`, pre-Conway `StakeDeregistration` refund
/// amounts are looked up from the per-credential deposit map (the deposit paid
/// at registration time). When `None`, the current `key_deposit` parameter is
/// used as a fallback.
pub(super) fn calculate_deposits_and_refunds(
    certificates: &[Certificate],
    params: &ProtocolParameters,
    registered_pools: Option<&HashSet<Hash28>>,
    stake_key_deposits: Option<&HashMap<Hash32, u64>>,
) -> (u64, u64) {
    let mut deposits = 0u64;
    let mut refunds = 0u64;
    // Track pools newly registered within this transaction so that a second
    // PoolRegistration cert for the same pool in the same tx is treated as an
    // update (no additional deposit).
    let mut newly_registered: HashSet<Hash28> = HashSet::new();

    for cert in certificates {
        match cert {
            Certificate::StakeRegistration(_) => {
                deposits += params.key_deposit.0;
            }
            Certificate::StakeDeregistration(credential) => {
                // Use the stored per-credential deposit for correct refund when
                // key_deposit changes via governance. Falls back to current param
                // when deposit map is unavailable or credential not found.
                let key = credential.to_typed_hash32();
                refunds += stake_key_deposits
                    .and_then(|m| m.get(&key).copied())
                    .unwrap_or(params.key_deposit.0);
            }
            Certificate::ConwayStakeRegistration { deposit, .. } => {
                deposits += deposit.0;
            }
            Certificate::ConwayStakeDeregistration { refund, .. } => {
                refunds += refund.0;
            }
            Certificate::PoolRegistration(pool_params) => {
                // Only charge deposit for NEW pool registrations.
                // Re-registration (update) of an already-registered pool is free.
                let already_registered = registered_pools
                    .is_some_and(|pools| pools.contains(&pool_params.operator))
                    || newly_registered.contains(&pool_params.operator);
                if !already_registered {
                    deposits += params.pool_deposit.0;
                    newly_registered.insert(pool_params.operator);
                }
            }
            Certificate::RegDRep { deposit, .. } => {
                deposits += deposit.0;
            }
            Certificate::UnregDRep { refund, .. } => {
                refunds += refund.0;
            }
            Certificate::RegStakeDeleg { deposit, .. } => {
                deposits += deposit.0;
            }
            Certificate::RegStakeVoteDeleg { deposit, .. } => {
                deposits += deposit.0;
            }
            Certificate::VoteRegDeleg { deposit, .. } => {
                deposits += deposit.0;
            }
            _ => {}
        }
    }

    (deposits, refunds)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap, HashSet};

    use dugite_primitives::credentials::Credential;
    use dugite_primitives::hash::{Hash28, Hash32};
    use dugite_primitives::protocol_params::ProtocolParameters;
    use dugite_primitives::transaction::{
        Anchor, Certificate, DRep, GovAction, GovActionId, PoolParams, ProposalProcedure, Rational,
        TransactionBody, Voter, VotingProcedure,
    };
    use dugite_primitives::value::Lovelace;

    use super::*;

    // ---------------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------------

    /// Build a minimal `TransactionBody` with the given certificates,
    /// voting procedures, and proposal procedures. All other fields are left
    /// empty/default so tests stay focused on what they actually care about.
    fn make_body(
        certificates: Vec<Certificate>,
        voting_procedures: BTreeMap<Voter, BTreeMap<GovActionId, VotingProcedure>>,
    ) -> TransactionBody {
        make_body_full(certificates, voting_procedures, vec![])
    }

    /// Like `make_body` but also accepts a `proposal_procedures` list.
    fn make_body_full(
        certificates: Vec<Certificate>,
        voting_procedures: BTreeMap<Voter, BTreeMap<GovActionId, VotingProcedure>>,
        proposal_procedures: Vec<ProposalProcedure>,
    ) -> TransactionBody {
        TransactionBody {
            inputs: vec![],
            outputs: vec![],
            fee: Lovelace(0),
            ttl: None,
            certificates,
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
            proposal_procedures,
            treasury_value: None,
            donation: None,
        }
    }

    /// A `Credential::VerificationKey` backed by a deterministic 28-byte hash.
    fn test_credential(byte: u8) -> Credential {
        Credential::VerificationKey(Hash28::from_bytes([byte; 28]))
    }

    /// A `PoolParams` stub with the given operator hash. Only `operator` is used
    /// by `calculate_deposits_and_refunds`; the other fields carry no-op values.
    fn make_pool_params(operator_byte: u8) -> PoolParams {
        PoolParams {
            operator: Hash28::from_bytes([operator_byte; 28]),
            vrf_keyhash: Hash32::from_bytes([0u8; 32]),
            pledge: Lovelace(0),
            cost: Lovelace(0),
            margin: Rational {
                numerator: 0,
                denominator: 1,
            },
            reward_account: vec![],
            pool_owners: vec![],
            relays: vec![],
            pool_metadata: None,
        }
    }

    // ---------------------------------------------------------------------------
    // check_era_gating — all 12 Conway-only cert types accepted at PV=9
    // ---------------------------------------------------------------------------

    #[test]
    fn test_conway_cert_in_conway_era() {
        // Protocol version 9 = Conway era; every Conway-only certificate must be
        // accepted without producing an EraGatingViolation.
        let mut params = ProtocolParameters::mainnet_defaults();
        params.protocol_version_major = 9;

        // All 12 Conway-only certificate variants (tags 7-18 in the spec).
        let all_conway_certs: Vec<Certificate> = vec![
            Certificate::ConwayStakeRegistration {
                credential: test_credential(0x01),
                deposit: Lovelace(2_000_000),
            },
            Certificate::ConwayStakeDeregistration {
                credential: test_credential(0x02),
                refund: Lovelace(2_000_000),
            },
            Certificate::RegDRep {
                credential: test_credential(0x03),
                deposit: Lovelace(500_000_000),
                anchor: None,
            },
            Certificate::UnregDRep {
                credential: test_credential(0x04),
                refund: Lovelace(500_000_000),
            },
            Certificate::UpdateDRep {
                credential: test_credential(0x05),
                anchor: None,
            },
            Certificate::VoteDelegation {
                credential: test_credential(0x06),
                drep: DRep::Abstain,
            },
            Certificate::StakeVoteDelegation {
                credential: test_credential(0x07),
                pool_hash: Hash28::from_bytes([0x07u8; 28]),
                drep: DRep::NoConfidence,
            },
            Certificate::CommitteeHotAuth {
                cold_credential: test_credential(0x08),
                hot_credential: test_credential(0x09),
            },
            Certificate::CommitteeColdResign {
                cold_credential: test_credential(0x0A),
                anchor: None,
            },
            Certificate::RegStakeVoteDeleg {
                credential: test_credential(0x0B),
                pool_hash: Hash28::from_bytes([0x0Bu8; 28]),
                drep: DRep::Abstain,
                deposit: Lovelace(2_000_000),
            },
            Certificate::VoteRegDeleg {
                credential: test_credential(0x0C),
                drep: DRep::Abstain,
                deposit: Lovelace(2_000_000),
            },
            Certificate::RegStakeDeleg {
                credential: test_credential(0x0D),
                pool_hash: Hash28::from_bytes([0x0Du8; 28]),
                deposit: Lovelace(2_000_000),
            },
        ];

        // Each cert is tested individually so a failure message names the variant.
        for cert in all_conway_certs {
            // conway_only_certificate_name() tells us what name the production
            // code would use in an error; use it to label assertions.
            let cert_name = conway_only_certificate_name(&cert)
                .expect("all certs in this list must be Conway-only");

            let body = make_body(vec![cert], BTreeMap::new());
            let mut errors: Vec<ValidationError> = vec![];
            check_era_gating(&params, &body, &mut errors);

            let violations: Vec<_> = errors
                .iter()
                .filter(|e| matches!(e, ValidationError::EraGatingViolation { .. }))
                .collect();
            assert!(
                violations.is_empty(),
                "Conway cert '{cert_name}' must be accepted in Conway era (pv=9), got: {violations:?}"
            );
        }
    }

    // ---------------------------------------------------------------------------
    // check_era_gating — Conway cert in Babbage era (error expected)
    // ---------------------------------------------------------------------------

    #[test]
    fn test_conway_cert_in_pre_conway_era() {
        // Protocol version 8 = Babbage era; Conway certs must be rejected.
        let mut params = ProtocolParameters::mainnet_defaults();
        params.protocol_version_major = 8;

        let cert = Certificate::RegDRep {
            credential: test_credential(0xBB),
            deposit: Lovelace(500_000_000),
            anchor: None,
        };
        let body = make_body(vec![cert], BTreeMap::new());

        let mut errors: Vec<ValidationError> = vec![];
        check_era_gating(&params, &body, &mut errors);

        let has_violation = errors
            .iter()
            .any(|e| matches!(e, ValidationError::EraGatingViolation { .. }));
        assert!(
            has_violation,
            "Expected EraGatingViolation for Conway cert in Babbage (pv=8)"
        );
    }

    // ---------------------------------------------------------------------------
    // check_era_gating — voting_procedures and proposal_procedures in pre-Conway
    // era (both branches must each produce a GovernancePreConway error)
    // ---------------------------------------------------------------------------

    #[test]
    fn test_governance_features_era_gated() {
        // Protocol version 8 = Babbage; voting procedures must be rejected.
        let mut params = ProtocolParameters::mainnet_defaults();
        params.protocol_version_major = 8;

        // --- Sub-test 1: non-empty voting_procedures -------------------------
        let gov_action_id = GovActionId {
            transaction_id: Hash32::from_bytes([0x01u8; 32]),
            action_index: 0,
        };
        let voting_procedure = VotingProcedure {
            vote: dugite_primitives::transaction::Vote::Yes,
            anchor: None,
        };
        let voter = Voter::DRep(test_credential(0xCC));
        let mut inner = BTreeMap::new();
        inner.insert(gov_action_id, voting_procedure);
        let mut voting_procedures = BTreeMap::new();
        voting_procedures.insert(voter, inner);

        let body_voting = make_body(vec![], voting_procedures);

        let mut errors: Vec<ValidationError> = vec![];
        check_era_gating(&params, &body_voting, &mut errors);

        let has_gov_error = errors
            .iter()
            .any(|e| matches!(e, ValidationError::GovernancePreConway { .. }));
        assert!(
            has_gov_error,
            "Expected GovernancePreConway for voting_procedures in Babbage (pv=8)"
        );

        // --- Sub-test 2: non-empty proposal_procedures -----------------------
        // The production code has a separate `if !body.proposal_procedures.is_empty()`
        // branch; verify it is also exercised.
        let proposal = ProposalProcedure {
            deposit: Lovelace(1_000_000_000),
            return_addr: vec![0xE1u8], // minimal reward address stub
            gov_action: GovAction::InfoAction,
            anchor: Anchor {
                url: "https://example.com".to_string(),
                data_hash: Hash32::from_bytes([0u8; 32]),
            },
        };

        let body_proposal = make_body_full(vec![], BTreeMap::new(), vec![proposal]);

        let mut errors2: Vec<ValidationError> = vec![];
        check_era_gating(&params, &body_proposal, &mut errors2);

        let has_gov_error2 = errors2
            .iter()
            .any(|e| matches!(e, ValidationError::GovernancePreConway { .. }));
        assert!(
            has_gov_error2,
            "Expected GovernancePreConway for proposal_procedures in Babbage (pv=8)"
        );
    }

    // ---------------------------------------------------------------------------
    // calculate_deposits_and_refunds — StakeRegistration charges key_deposit
    // ---------------------------------------------------------------------------

    #[test]
    fn test_deposit_new_key_registration() {
        let params = ProtocolParameters::mainnet_defaults(); // key_deposit = 2_000_000
        let cert = Certificate::StakeRegistration(test_credential(0x01));

        let (deposits, refunds) = calculate_deposits_and_refunds(&[cert], &params, None, None);

        assert_eq!(
            deposits, params.key_deposit.0,
            "StakeRegistration should charge key_deposit"
        );
        assert_eq!(refunds, 0, "StakeRegistration should produce no refund");
    }

    // ---------------------------------------------------------------------------
    // calculate_deposits_and_refunds — RegDRep charges the inline cert deposit
    // ---------------------------------------------------------------------------

    #[test]
    fn test_deposit_new_drep_registration() {
        let params = ProtocolParameters::mainnet_defaults(); // drep_deposit = 500_000_000

        // Use a deposit amount distinct from params.drep_deposit.0 to prove
        // that the implementation reads the inline cert field rather than
        // falling back to the protocol parameter.
        let inline_deposit: u64 = 750_000_000;
        assert_ne!(
            inline_deposit, params.drep_deposit.0,
            "test setup: inline_deposit must differ from params.drep_deposit for this test to be meaningful"
        );

        let cert = Certificate::RegDRep {
            credential: test_credential(0x02),
            deposit: Lovelace(inline_deposit),
            anchor: None,
        };

        let (deposits, refunds) = calculate_deposits_and_refunds(&[cert], &params, None, None);

        assert_eq!(
            deposits, inline_deposit,
            "RegDRep should charge the inline cert deposit ({inline_deposit}), \
             not params.drep_deposit ({})",
            params.drep_deposit.0
        );
        assert_eq!(refunds, 0, "RegDRep should produce no refund");
    }

    // ---------------------------------------------------------------------------
    // calculate_deposits_and_refunds — PoolRegistration re-registration is free
    // ---------------------------------------------------------------------------

    #[test]
    fn test_deposit_pool_reregistration_free() {
        let params = ProtocolParameters::mainnet_defaults(); // pool_deposit = 500_000_000
        let pool_params = make_pool_params(0x03);
        let operator = pool_params.operator;

        // Pool is already in the registered set.
        let mut registered_pools: HashSet<Hash28> = HashSet::new();
        registered_pools.insert(operator);

        let cert = Certificate::PoolRegistration(pool_params);

        let (deposits, refunds) =
            calculate_deposits_and_refunds(&[cert], &params, Some(&registered_pools), None);

        assert_eq!(
            deposits, 0,
            "Re-registration of an existing pool should charge 0 deposit"
        );
        assert_eq!(refunds, 0);
    }

    // ---------------------------------------------------------------------------
    // calculate_deposits_and_refunds — StakeDeregistration refunds key_deposit
    // ---------------------------------------------------------------------------

    #[test]
    fn test_refund_deregistration() {
        let params = ProtocolParameters::mainnet_defaults(); // key_deposit = 2_000_000
        let credential = test_credential(0x04);
        let cert = Certificate::StakeDeregistration(credential.clone());

        // No deposit map provided — should fall back to current key_deposit.
        let (deposits, refunds) = calculate_deposits_and_refunds(&[cert], &params, None, None);

        assert_eq!(deposits, 0, "StakeDeregistration should produce no deposit");
        assert_eq!(
            refunds, params.key_deposit.0,
            "StakeDeregistration should refund key_deposit when deposit map is absent"
        );
    }

    // ---------------------------------------------------------------------------
    // calculate_deposits_and_refunds — UnregDRep refunds inline cert amount
    // ---------------------------------------------------------------------------

    /// The spec says "Key/DRep deregistration refunds [use] the stored
    /// per-credential deposit amount."  For `UnregDRep` the refund amount is
    /// carried inline in the certificate itself (not looked up from a map), so
    /// the implementation must use `refund.0` directly.
    #[test]
    fn test_refund_unreg_drep() {
        let params = ProtocolParameters::mainnet_defaults(); // drep_deposit = 500_000_000

        // Use a refund amount distinct from params.drep_deposit.0 to prove
        // the code reads the cert field, not the current protocol parameter.
        let original_deposit: u64 = 300_000_000;
        let cert = Certificate::UnregDRep {
            credential: test_credential(0x10),
            refund: Lovelace(original_deposit),
        };

        let (deposits, refunds) = calculate_deposits_and_refunds(&[cert], &params, None, None);

        assert_eq!(deposits, 0, "UnregDRep should produce no deposit");
        assert_eq!(
            refunds, original_deposit,
            "UnregDRep refund must equal the inline cert amount ({original_deposit}), \
             not the current drep_deposit ({})",
            params.drep_deposit.0
        );
    }

    // ---------------------------------------------------------------------------
    // calculate_deposits_and_refunds — ConwayStakeDeregistration uses inline amount
    // ---------------------------------------------------------------------------

    #[test]
    fn test_per_credential_deposit_map() {
        // current key_deposit = 2_000_000 ADA; original deposit was 1_500_000
        // (simulates a governance-changed key_deposit after original registration).
        let mut params = ProtocolParameters::mainnet_defaults();
        params.protocol_version_major = 9;
        let stored_deposit: u64 = 1_500_000;

        let credential = test_credential(0x05);

        // ConwayStakeDeregistration carries the inline refund amount agreed at
        // registration time.
        let cert = Certificate::ConwayStakeDeregistration {
            credential: credential.clone(),
            refund: Lovelace(stored_deposit),
        };

        // The deposit map is not consulted for ConwayStakeDeregistration because
        // the refund amount is encoded inline in the certificate itself.
        let mut deposit_map: HashMap<Hash32, u64> = HashMap::new();
        deposit_map.insert(credential.to_typed_hash32(), stored_deposit);

        let (deposits, refunds) =
            calculate_deposits_and_refunds(&[cert], &params, None, Some(&deposit_map));

        assert_eq!(deposits, 0);
        assert_eq!(
            refunds, stored_deposit,
            "ConwayStakeDeregistration refund must use the inline cert amount, \
             not the current key_deposit ({}) or deposit map",
            params.key_deposit.0
        );
    }

    // ---------------------------------------------------------------------------
    // calculate_deposits_and_refunds — pre-Conway StakeDeregistration uses
    // stake_key_deposits map (not current key_deposit) when the map has an entry
    // ---------------------------------------------------------------------------

    /// Verifies the `StakeDeregistration` branch reads the per-credential stored
    /// deposit amount rather than falling back to the current `key_deposit`
    /// protocol parameter when `stake_key_deposits` is present and contains the
    /// credential.  This matters when `key_deposit` is later changed via
    /// governance action: the refund must equal what was paid at registration
    /// time.
    #[test]
    fn test_pre_conway_deregistration_uses_stored_deposit() {
        let mut params = ProtocolParameters::mainnet_defaults();
        // Simulate a governance-voted change: key_deposit is now 3_000_000
        // but the credential originally deposited 1_800_000.
        params.key_deposit = Lovelace(3_000_000);
        params.protocol_version_major = 8; // pre-Conway Babbage

        let original_deposit: u64 = 1_800_000;
        let credential = test_credential(0x20);
        let key = credential.to_typed_hash32();

        let mut deposit_map: HashMap<Hash32, u64> = HashMap::new();
        deposit_map.insert(key, original_deposit);

        let cert = Certificate::StakeDeregistration(credential.clone());

        let (deposits, refunds) =
            calculate_deposits_and_refunds(&[cert], &params, None, Some(&deposit_map));

        assert_eq!(deposits, 0, "StakeDeregistration should produce no deposit");
        assert_eq!(
            refunds, original_deposit,
            "StakeDeregistration refund must use the stored deposit map entry \
             ({original_deposit}) not the current key_deposit ({})",
            params.key_deposit.0
        );
    }
}
