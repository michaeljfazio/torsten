//! Conway-era specific validation: era gating, governance checks, and
//! certificate deposit/refund accounting.
//!
//! This module handles:
//! - Ensuring Conway-only certificates and governance actions are rejected on
//!   pre-Conway protocol versions (Rule 1d).
//! - Calculating the net deposit and refund amounts for all certificate types
//!   across eras, including pool re-registration logic.

use std::collections::HashSet;

use torsten_primitives::hash::Hash28;
use torsten_primitives::protocol_params::ProtocolParameters;
use torsten_primitives::transaction::Certificate;

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
    body: &torsten_primitives::transaction::TransactionBody,
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
pub(super) fn calculate_deposits_and_refunds(
    certificates: &[Certificate],
    params: &ProtocolParameters,
    registered_pools: Option<&HashSet<Hash28>>,
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
            Certificate::StakeDeregistration(_) => {
                refunds += params.key_deposit.0;
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
