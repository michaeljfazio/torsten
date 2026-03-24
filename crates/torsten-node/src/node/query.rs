//! N2C LocalStateQuery response building.
//!
//! Contains `Node::update_query_state()` which assembles the `NodeStateSnapshot`
//! pushed into the `QueryHandler` on every block or periodically at tip, as well
//! as `build_era_summaries()` for `GetEraHistory` responses.

use super::Node;

// ─── Arithmetic helpers ───────────────────────────────────────────────────────

/// Convert an f64 to a (numerator, denominator) rational approximation.
///
/// Handles common Cardano genesis values like 0.05 → (1, 20).
pub(crate) fn float_to_rational(f: f64) -> (u64, u64) {
    if f == 0.0 {
        return (0, 1);
    }
    if f == 1.0 {
        return (1, 1);
    }
    // Try to find exact fraction with small denominators first
    for den in 1..=10000u64 {
        let num = (f * den as f64).round() as u64;
        let reconstructed = num as f64 / den as f64;
        if (reconstructed - f).abs() < 1e-12 {
            // Simplify by GCD
            let g = gcd(num, den);
            return (num / g, den / g);
        }
    }
    // Fallback: use large denominator
    let den = 1_000_000u64;
    let num = (f * den as f64).round() as u64;
    let g = gcd(num, den);
    (num / g, den / g)
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

/// Convert a Credential to (type, hash_bytes) for vote maps.
/// Returns (0, hash_28) for VerificationKey, (1, hash_28) for Script.
pub(crate) fn credential_to_bytes(
    cred: &torsten_primitives::credentials::Credential,
) -> (u8, Vec<u8>) {
    match cred {
        torsten_primitives::credentials::Credential::VerificationKey(h) => (0, h.as_ref().to_vec()),
        torsten_primitives::credentials::Credential::Script(h) => (1, h.as_ref().to_vec()),
    }
}

/// Truncate a padded `Hash32` key back to 28 bytes for N2C wire encoding.
///
/// The ledger uses `Hash32` (32 bytes) as HashMap keys for stake credentials,
/// DRep credential hashes, committee credential hashes, and pool voter keys.
/// These are Blake2b-224 (28-byte) hashes that were zero-padded to 32 bytes
/// to enable use as uniform HashMap keys (see `Hash28::to_hash32_padded()`).
///
/// The Cardano N2C wire format expects 28 bytes for all credential/pool-ID
/// hashes.  Sending 32 bytes causes cardano-cli to reject the response with
/// "hash bytes wrong size, expected 28 but got 32".
///
/// Only call this on `Hash32` values that are known to be padded 28-byte
/// hashes (credentials, pool IDs, DRep keys).  Do NOT call it on genuine
/// 32-byte hashes such as transaction IDs, block hashes, or VRF key hashes.
#[inline]
fn hash32_padded_to_28_bytes(h: &torsten_primitives::hash::Hash32) -> Vec<u8> {
    h.as_ref()[..28].to_vec()
}

/// Build a `SnapshotStakeData` from a single `StakeSnapshot`.
///
/// The `script_creds` set distinguishes script-hash credentials (type=1) from
/// verification-key credentials (type=0).  We use the live ledger's
/// `script_stake_credentials` set as an approximation — credential types are
/// stable once registered, so this is accurate.
///
/// Each credential hash is truncated to 28 bytes (the snapshot stores Hash32
/// where the upper 4 bytes are zero-padding used for HashMap keying).
fn build_snapshot_stake_data(
    snap: &torsten_ledger::state::StakeSnapshot,
    script_creds: &std::collections::HashSet<torsten_primitives::hash::Hash32>,
) -> torsten_network::query_handler::SnapshotStakeData {
    use torsten_network::query_handler::{PoolParamsSnapshot, RelaySnapshot, SnapshotStakeData};

    // stake_entries: one entry per delegated credential
    let mut stake_entries = Vec::with_capacity(snap.stake_distribution.len());
    for (cred_hash, lovelace) in snap.stake_distribution.iter() {
        let cred_type = script_creds.contains(cred_hash) as u8;
        // Use the lower 28 bytes of the Hash32 key
        let hash28 = cred_hash.as_ref()[..28].to_vec();
        stake_entries.push((cred_type, hash28, lovelace.0));
    }

    // delegation_entries: map credential → pool_id
    let mut delegation_entries = Vec::with_capacity(snap.delegations.len());
    for (cred_hash, pool_id) in snap.delegations.iter() {
        let cred_type = script_creds.contains(cred_hash) as u8;
        let hash28 = cred_hash.as_ref()[..28].to_vec();
        delegation_entries.push((cred_type, hash28, pool_id.as_ref().to_vec()));
    }

    // pool_params: convert snapshot pool params to PoolParamsSnapshot
    let pool_params: Vec<PoolParamsSnapshot> = snap
        .pool_params
        .iter()
        .map(|(pool_id, reg)| {
            let relays: Vec<RelaySnapshot> = reg
                .relays
                .iter()
                .map(|r| match r {
                    torsten_primitives::transaction::Relay::SingleHostAddr { port, ipv4, ipv6 } => {
                        RelaySnapshot::SingleHostAddr {
                            port: *port,
                            ipv4: *ipv4,
                            ipv6: *ipv6,
                        }
                    }
                    torsten_primitives::transaction::Relay::SingleHostName { port, dns_name } => {
                        RelaySnapshot::SingleHostName {
                            port: *port,
                            dns_name: dns_name.clone(),
                        }
                    }
                    torsten_primitives::transaction::Relay::MultiHostName { dns_name } => {
                        RelaySnapshot::MultiHostName {
                            dns_name: dns_name.clone(),
                        }
                    }
                })
                .collect();
            PoolParamsSnapshot {
                pool_id: pool_id.as_ref().to_vec(),
                vrf_keyhash: reg.vrf_keyhash.as_ref().to_vec(),
                pledge: reg.pledge.0,
                cost: reg.cost.0,
                margin_num: reg.margin_numerator,
                margin_den: reg.margin_denominator,
                reward_account: reg.reward_account.clone(),
                owners: reg.owners.iter().map(|o| o.as_ref().to_vec()).collect(),
                relays,
                metadata_url: reg.metadata_url.clone(),
                metadata_hash: reg.metadata_hash.map(|h| h.as_ref().to_vec()),
            }
        })
        .collect();

    SnapshotStakeData {
        stake_entries,
        delegation_entries,
        pool_params,
    }
}

// ─── Node impl: query state ───────────────────────────────────────────────────

impl Node {
    /// Update the query handler with the current ledger state.
    ///
    /// Called whenever a block is applied at tip and periodically during sync
    /// (every 30 seconds) so that N2C `LocalStateQuery` requests reflect recent
    /// on-chain state.
    pub async fn update_query_state(&self) {
        use torsten_network::query_handler::{
            CommitteeMemberSnapshot, CommitteeSnapshot, DRepDelegationEntry, DRepSnapshot,
            DRepStakeEntry, GenesisConfigSnapshot, PoolParamsSnapshot, PoolStakeSnapshotEntry,
            ProposalSnapshot, ShelleyPParamsSnapshot, StakeAddressSnapshot, StakeDelegDepositEntry,
            StakePoolSnapshot, StakeSnapshotsResult, VoteDelegateeEntry,
        };

        let ls = self.ledger_state.read().await;

        // Build per-pool stake map from delegations for accurate reporting.
        // Per Cardano spec, total stake = UTxO-delegated stake + reward account balance.
        let mut pool_stake_map: std::collections::HashMap<torsten_primitives::hash::Hash28, u64> =
            std::collections::HashMap::new();
        for (cred_hash, pool_id) in ls.delegations.iter() {
            let utxo_stake = ls
                .stake_distribution
                .stake_map
                .get(cred_hash)
                .map(|l| l.0)
                .unwrap_or(0);
            let reward_balance = ls.reward_accounts.get(cred_hash).map(|l| l.0).unwrap_or(0);
            *pool_stake_map.entry(*pool_id).or_default() += utxo_stake + reward_balance;
        }

        // Build stake pool snapshots with actual per-pool stake
        let total_active_stake: u64 = pool_stake_map.values().sum();
        let stake_pools: Vec<StakePoolSnapshot> = ls
            .pool_params
            .iter()
            .map(|(pool_id, reg)| StakePoolSnapshot {
                pool_id: pool_id.as_ref().to_vec(),
                stake: pool_stake_map.get(pool_id).copied().unwrap_or(0),
                vrf_keyhash: reg.vrf_keyhash.as_ref().to_vec(),
                total_active_stake,
            })
            .collect();

        // Build DRep snapshots with delegator lookup
        let drep_entries: Vec<DRepSnapshot> = ls
            .governance
            .dreps
            .iter()
            .map(|(hash, drep)| {
                let expiry = drep.registered_epoch.0 + ls.protocol_params.drep_activity;
                // Collect stake credentials delegated to this DRep
                let delegator_hashes: Vec<Vec<u8>> = ls
                    .governance
                    .vote_delegations
                    .iter()
                    .filter(|(_, d)| match d {
                        torsten_primitives::transaction::DRep::KeyHash(h) => h == hash,
                        torsten_primitives::transaction::DRep::ScriptHash(h) => {
                            h.to_hash32_padded() == *hash
                        }
                        _ => false,
                    })
                    // stake_cred is a Hash32 padded from a 28-byte key hash;
                    // truncate to 28 bytes for N2C wire format.
                    .map(|(stake_cred, _)| hash32_padded_to_28_bytes(stake_cred))
                    .collect();
                DRepSnapshot {
                    // DRep hash keys are Hash32 padded from 28-byte credential hashes.
                    credential_hash: hash32_padded_to_28_bytes(hash),
                    // DRepRegistration stores the full Credential enum, so we can derive the type
                    // directly: 0 = VerificationKey (KeyHashObj), 1 = Script (ScriptHashObj).
                    credential_type: drep.credential.is_script() as u8,
                    deposit: drep.deposit.0,
                    anchor_url: drep.anchor.as_ref().map(|a| a.url.clone()),
                    anchor_hash: drep.anchor.as_ref().map(|a| a.data_hash.as_ref().to_vec()),
                    expiry_epoch: expiry,
                    delegator_hashes,
                }
            })
            .collect();

        // Build governance proposal snapshots.
        // ALL governance action types (InfoAction, ParameterChange, HardForkInitiation,
        // UpdateCommittee, NewConstitution, NoConfidence, TreasuryWithdrawals) are stored
        // in governance.proposals by process_proposal().  We faithfully convert every
        // one of them here, carrying the full GovAction enum so the CBOR encoder can
        // reproduce the complete action body on the wire (fixes issue #172).
        let governance_proposals: Vec<ProposalSnapshot> = ls
            .governance
            .proposals
            .iter()
            .map(|(action_id, state)| {
                let action_type = gov_action_type_str(&state.procedure.gov_action);
                // Build per-credential vote maps from votes_by_action
                let (committee_votes, drep_votes, spo_votes) = build_vote_maps(&ls, action_id);
                ProposalSnapshot {
                    tx_id: action_id.transaction_id.as_ref().to_vec(),
                    action_index: action_id.action_index,
                    action_type: action_type.to_string(),
                    proposed_epoch: state.proposed_epoch.0,
                    expires_epoch: state.expires_epoch.0,
                    yes_votes: state.yes_votes,
                    no_votes: state.no_votes,
                    abstain_votes: state.abstain_votes,
                    deposit: state.procedure.deposit.0,
                    return_addr: state.procedure.return_addr.clone(),
                    anchor_url: state.procedure.anchor.url.clone(),
                    anchor_hash: state.procedure.anchor.data_hash.as_ref().to_vec(),
                    gov_action: state.procedure.gov_action.clone(),
                    committee_votes,
                    drep_votes,
                    spo_votes,
                }
            })
            .collect();

        // Build committee snapshot.
        // Iterate committee_expiration (the canonical member list) rather than
        // committee_hot_keys, so that members without hot key authorization
        // (MemberNotAuthorized) are included in the response.
        let resigned_set: std::collections::HashSet<_> =
            ls.governance.committee_resigned.keys().collect();
        let committee = CommitteeSnapshot {
            members: ls
                .governance
                .committee_expiration
                .iter()
                .map(|(cold, _expiry)| {
                    let is_resigned = resigned_set.contains(cold);
                    let hot_key = ls.governance.committee_hot_keys.get(cold);

                    // Determine hot credential authorization status:
                    // 0 = MemberAuthorized (has hot key), 1 = MemberNotAuthorized, 2 = Resigned
                    let hot_status = if is_resigned {
                        2
                    } else if hot_key.is_some() {
                        0
                    } else {
                        1 // MemberNotAuthorized: in expiration map but no hot key
                    };

                    CommitteeMemberSnapshot {
                        // Committee cold/hot credentials are stored as Hash32 (padded from
                        // 28-byte Blake2b-224 hashes). Truncate to 28 bytes for N2C wire format.
                        cold_credential: hash32_padded_to_28_bytes(cold),
                        // Use the script_committee_credentials set to correctly distinguish
                        // key credentials (0) from script credentials (1).
                        cold_credential_type: ls
                            .governance
                            .script_committee_credentials
                            .contains(cold) as u8,
                        hot_status,
                        hot_credential: match hot_key {
                            Some(hk) if !is_resigned => Some(hash32_padded_to_28_bytes(hk)),
                            _ => None,
                        },
                        // Hot credential type: 0=KeyHash, 1=ScriptHash.
                        // Resolved by probing script_committee_hot_credentials with the current
                        // hot key hash.  The set is keyed by hot credential hash so that
                        // re-authorization with a different hot key is handled naturally: the
                        // new hot key either is or is not in the script set independently of
                        // any prior authorization for the same cold key.
                        hot_credential_type: match hot_key {
                            Some(hk) if !is_resigned => {
                                ls.governance.script_committee_hot_credentials.contains(hk) as u8
                            }
                            _ => 0,
                        },
                        member_status: 0, // Active (simplified)
                        expiry_epoch: Some(_expiry.0),
                    }
                })
                .collect(),
            threshold: ls
                .governance
                .committee_threshold
                .as_ref()
                .map(|r| (r.numerator, r.denominator))
                .or(Some((2, 3))), // Fallback to 2/3 if not set
            current_epoch: ls.epoch.0,
        };

        // Build stake address snapshots (delegations + rewards).
        // `cred_hash` is a Hash32 padded from a 28-byte stake key hash; truncate to 28 bytes.
        // `pool_id` from `delegations` is a Hash28, already the right size.
        let stake_addresses: Vec<StakeAddressSnapshot> = ls
            .reward_accounts
            .iter()
            .map(|(cred_hash, rewards)| {
                let delegated_pool = ls
                    .delegations
                    .get(cred_hash)
                    .map(|pool_id| pool_id.as_ref().to_vec());
                StakeAddressSnapshot {
                    // reward_accounts keys are Hash32 padded from 28-byte credential hashes.
                    credential_hash: hash32_padded_to_28_bytes(cred_hash),
                    delegated_pool,
                    reward_balance: rewards.0,
                }
            })
            .collect();

        // Build stake snapshots (mark/set/go)
        let stake_snapshots = {
            // Collect all unique pool IDs across all snapshots
            let mut all_pool_ids = std::collections::BTreeSet::new();
            if let Some(ref snap) = ls.snapshots.mark {
                all_pool_ids.extend(snap.pool_stake.keys().cloned());
            }
            if let Some(ref snap) = ls.snapshots.set {
                all_pool_ids.extend(snap.pool_stake.keys().cloned());
            }
            if let Some(ref snap) = ls.snapshots.go {
                all_pool_ids.extend(snap.pool_stake.keys().cloned());
            }

            let pools: Vec<PoolStakeSnapshotEntry> = all_pool_ids
                .iter()
                .map(|pid| PoolStakeSnapshotEntry {
                    pool_id: pid.as_ref().to_vec(),
                    mark_stake: ls
                        .snapshots
                        .mark
                        .as_ref()
                        .and_then(|s| s.pool_stake.get(pid))
                        .map(|l| l.0)
                        .unwrap_or(0),
                    set_stake: ls
                        .snapshots
                        .set
                        .as_ref()
                        .and_then(|s| s.pool_stake.get(pid))
                        .map(|l| l.0)
                        .unwrap_or(0),
                    go_stake: ls
                        .snapshots
                        .go
                        .as_ref()
                        .and_then(|s| s.pool_stake.get(pid))
                        .map(|l| l.0)
                        .unwrap_or(0),
                })
                .collect();

            let total_mark_stake = pools.iter().map(|p| p.mark_stake).sum();
            let total_set_stake = pools.iter().map(|p| p.set_stake).sum();
            let total_go_stake = pools.iter().map(|p| p.go_stake).sum();

            StakeSnapshotsResult {
                pools,
                total_mark_stake,
                total_set_stake,
                total_go_stake,
            }
        };

        // Build full per-credential snapshot data for DebugNewEpochState (cncli snapshot).
        // We use the live script_stake_credentials set to determine credential types.
        let snap_mark = ls
            .snapshots
            .mark
            .as_ref()
            .map(|s| build_snapshot_stake_data(s, &ls.script_stake_credentials))
            .unwrap_or_default();
        let snap_set = ls
            .snapshots
            .set
            .as_ref()
            .map(|s| build_snapshot_stake_data(s, &ls.script_stake_credentials))
            .unwrap_or_default();
        let snap_go = ls
            .snapshots
            .go
            .as_ref()
            .map(|s| build_snapshot_stake_data(s, &ls.script_stake_credentials))
            .unwrap_or_default();

        // Build per-pool epoch block count map for NewEpochState [1]/[2] fields.
        // Haskell places the *previous* epoch's BlocksMade at [1] and the current
        // at [2].  We expose the current epoch's counts as [1] (best approximation
        // without a previous-epoch tracker), and leave [2] empty.
        let epoch_blocks_by_pool: Vec<(Vec<u8>, u64)> = ls
            .epoch_blocks_by_pool
            .iter()
            .map(|(pool_id, count)| (pool_id.as_ref().to_vec(), *count))
            .collect();

        // Build pool params entries
        let pool_params_entries: Vec<PoolParamsSnapshot> = ls
            .pool_params
            .iter()
            .map(|(pool_id, reg)| {
                use torsten_network::query_handler::RelaySnapshot;
                let relays: Vec<RelaySnapshot> = reg
                    .relays
                    .iter()
                    .map(|r| match r {
                        torsten_primitives::transaction::Relay::SingleHostAddr {
                            port,
                            ipv4,
                            ipv6,
                        } => RelaySnapshot::SingleHostAddr {
                            port: *port,
                            ipv4: *ipv4,
                            ipv6: *ipv6,
                        },
                        torsten_primitives::transaction::Relay::SingleHostName {
                            port,
                            dns_name,
                        } => RelaySnapshot::SingleHostName {
                            port: *port,
                            dns_name: dns_name.clone(),
                        },
                        torsten_primitives::transaction::Relay::MultiHostName { dns_name } => {
                            RelaySnapshot::MultiHostName {
                                dns_name: dns_name.clone(),
                            }
                        }
                    })
                    .collect();
                PoolParamsSnapshot {
                    pool_id: pool_id.as_ref().to_vec(),
                    vrf_keyhash: reg.vrf_keyhash.as_ref().to_vec(),
                    pledge: reg.pledge.0,
                    cost: reg.cost.0,
                    margin_num: reg.margin_numerator,
                    margin_den: reg.margin_denominator,
                    reward_account: reg.reward_account.clone(),
                    owners: reg.owners.iter().map(|o| o.as_ref().to_vec()).collect(),
                    relays,
                    metadata_url: reg.metadata_url.clone(),
                    metadata_hash: reg.metadata_hash.map(|h| h.as_ref().to_vec()),
                }
            })
            .collect();

        // Build protocol params snapshot for CBOR encoding
        let pp = &ls.protocol_params;
        let protocol_params = torsten_network::query_handler::ProtocolParamsSnapshot {
            min_fee_a: pp.min_fee_a,
            min_fee_b: pp.min_fee_b,
            max_block_body_size: pp.max_block_body_size,
            max_tx_size: pp.max_tx_size,
            max_block_header_size: pp.max_block_header_size,
            key_deposit: pp.key_deposit.0,
            pool_deposit: pp.pool_deposit.0,
            e_max: pp.e_max,
            n_opt: pp.n_opt,
            a0_num: pp.a0.numerator,
            a0_den: pp.a0.denominator,
            rho_num: pp.rho.numerator,
            rho_den: pp.rho.denominator,
            tau_num: pp.tau.numerator,
            tau_den: pp.tau.denominator,
            min_pool_cost: pp.min_pool_cost.0,
            ada_per_utxo_byte: pp.ada_per_utxo_byte.0,
            cost_models_v1: pp.cost_models.plutus_v1.clone(),
            cost_models_v2: pp.cost_models.plutus_v2.clone(),
            cost_models_v3: pp.cost_models.plutus_v3.clone(),
            execution_costs_mem_num: pp.execution_costs.mem_price.numerator,
            execution_costs_mem_den: pp.execution_costs.mem_price.denominator,
            execution_costs_step_num: pp.execution_costs.step_price.numerator,
            execution_costs_step_den: pp.execution_costs.step_price.denominator,
            max_tx_ex_mem: pp.max_tx_ex_units.mem,
            max_tx_ex_steps: pp.max_tx_ex_units.steps,
            max_block_ex_mem: pp.max_block_ex_units.mem,
            max_block_ex_steps: pp.max_block_ex_units.steps,
            max_val_size: pp.max_val_size,
            collateral_percentage: pp.collateral_percentage,
            max_collateral_inputs: pp.max_collateral_inputs,
            protocol_version_major: pp.protocol_version_major,
            protocol_version_minor: pp.protocol_version_minor,
            min_fee_ref_script_cost_per_byte: pp.min_fee_ref_script_cost_per_byte,
            drep_deposit: pp.drep_deposit.0,
            drep_activity: pp.drep_activity,
            gov_action_deposit: pp.gov_action_deposit.0,
            gov_action_lifetime: pp.gov_action_lifetime,
            committee_min_size: pp.committee_min_size,
            committee_max_term_length: pp.committee_max_term_length,
            dvt_pp_network_group_num: pp.dvt_pp_network_group.numerator,
            dvt_pp_network_group_den: pp.dvt_pp_network_group.denominator,
            dvt_pp_economic_group_num: pp.dvt_pp_economic_group.numerator,
            dvt_pp_economic_group_den: pp.dvt_pp_economic_group.denominator,
            dvt_pp_technical_group_num: pp.dvt_pp_technical_group.numerator,
            dvt_pp_technical_group_den: pp.dvt_pp_technical_group.denominator,
            dvt_pp_gov_group_num: pp.dvt_pp_gov_group.numerator,
            dvt_pp_gov_group_den: pp.dvt_pp_gov_group.denominator,
            dvt_hard_fork_num: pp.dvt_hard_fork.numerator,
            dvt_hard_fork_den: pp.dvt_hard_fork.denominator,
            dvt_no_confidence_num: pp.dvt_no_confidence.numerator,
            dvt_no_confidence_den: pp.dvt_no_confidence.denominator,
            dvt_committee_normal_num: pp.dvt_committee_normal.numerator,
            dvt_committee_normal_den: pp.dvt_committee_normal.denominator,
            dvt_committee_no_confidence_num: pp.dvt_committee_no_confidence.numerator,
            dvt_committee_no_confidence_den: pp.dvt_committee_no_confidence.denominator,
            dvt_constitution_num: pp.dvt_constitution.numerator,
            dvt_constitution_den: pp.dvt_constitution.denominator,
            dvt_treasury_withdrawal_num: pp.dvt_treasury_withdrawal.numerator,
            dvt_treasury_withdrawal_den: pp.dvt_treasury_withdrawal.denominator,
            pvt_motion_no_confidence_num: pp.pvt_motion_no_confidence.numerator,
            pvt_motion_no_confidence_den: pp.pvt_motion_no_confidence.denominator,
            pvt_committee_normal_num: pp.pvt_committee_normal.numerator,
            pvt_committee_normal_den: pp.pvt_committee_normal.denominator,
            pvt_committee_no_confidence_num: pp.pvt_committee_no_confidence.numerator,
            pvt_committee_no_confidence_den: pp.pvt_committee_no_confidence.denominator,
            pvt_hard_fork_num: pp.pvt_hard_fork.numerator,
            pvt_hard_fork_den: pp.pvt_hard_fork.denominator,
            pvt_pp_security_group_num: pp.pvt_pp_security_group.numerator,
            pvt_pp_security_group_den: pp.pvt_pp_security_group.denominator,
        };

        // Build stake delegation deposits (registered stake credentials → key_deposit)
        let key_deposit = ls.protocol_params.key_deposit.0;
        let stake_deleg_deposits: Vec<StakeDelegDepositEntry> = ls
            .reward_accounts
            .keys()
            .map(|cred_hash| StakeDelegDepositEntry {
                credential_hash: cred_hash.as_ref()[..28].to_vec(),
                // Use the script_stake_credentials set to distinguish key (0) from script (1).
                credential_type: ls.script_stake_credentials.contains(cred_hash) as u8,
                deposit: key_deposit,
            })
            .collect();

        // Build DRep stake distribution (DRep → total delegated stake)
        let drep_stake_distr: Vec<DRepStakeEntry> = {
            use torsten_primitives::transaction::DRep;
            let mut drep_stakes: std::collections::HashMap<String, (u8, Option<Vec<u8>>, u64)> =
                std::collections::HashMap::new();
            for (stake_cred, drep) in &ls.governance.vote_delegations {
                let stake = ls
                    .stake_distribution
                    .stake_map
                    .get(stake_cred)
                    .map(|l| l.0)
                    .unwrap_or(0);
                let (key, drep_type, drep_hash) = match drep {
                    DRep::KeyHash(h) => {
                        let hb = h.as_ref()[..28].to_vec();
                        (format!("0:{}", hex::encode(&hb)), 0u8, Some(hb))
                    }
                    DRep::ScriptHash(h) => {
                        let hb = h.as_ref().to_vec();
                        (format!("1:{}", hex::encode(&hb)), 1u8, Some(hb))
                    }
                    DRep::Abstain => ("2:abstain".to_string(), 2u8, None),
                    DRep::NoConfidence => ("3:noconf".to_string(), 3u8, None),
                };
                let entry = drep_stakes
                    .entry(key)
                    .or_insert((drep_type, drep_hash.clone(), 0));
                entry.2 += stake;
            }
            drep_stakes
                .into_values()
                .map(|(drep_type, drep_hash, stake)| DRepStakeEntry {
                    drep_type,
                    drep_hash,
                    stake,
                })
                .collect()
        };

        // Build vote delegatee entries.
        // `stake_cred` is a Hash32 padded from a 28-byte stake key hash; truncate to 28 bytes.
        // DRep::KeyHash contains a Hash32 padded from a 28-byte DRep key hash; also truncate.
        // DRep::ScriptHash contains a Hash28 (ScriptHash); already correct size.
        let vote_delegatees: Vec<VoteDelegateeEntry> = {
            use torsten_primitives::transaction::DRep;
            ls.governance
                .vote_delegations
                .iter()
                .map(|(stake_cred, drep)| {
                    let (drep_type, drep_hash) = match drep {
                        // DRep::KeyHash stores the DRep key as Hash32 (padded from 28 bytes).
                        DRep::KeyHash(h) => (0u8, Some(h.as_ref()[..28].to_vec())),
                        // DRep::ScriptHash stores the script hash as Hash28 (correct size).
                        DRep::ScriptHash(h) => (1u8, Some(h.as_ref().to_vec())),
                        DRep::Abstain => (2u8, None),
                        DRep::NoConfidence => (3u8, None),
                    };
                    VoteDelegateeEntry {
                        // vote_delegations keys are Hash32 padded from 28-byte stake key hashes.
                        credential_hash: hash32_padded_to_28_bytes(stake_cred),
                        // Use the script_stake_credentials set to distinguish key (0) from script (1).
                        credential_type: ls.script_stake_credentials.contains(stake_cred) as u8,
                        drep_type,
                        drep_hash,
                    }
                })
                .collect()
        };

        // Build DRep delegation entries for GetDRepDelegations (tag 39, V23+).
        // Uses the same source as vote_delegatees (ls.governance.vote_delegations) but
        // produces DRepDelegationEntry values, keeping the two query types independent.
        let drep_delegations: Vec<DRepDelegationEntry> = {
            use torsten_primitives::transaction::DRep;
            ls.governance
                .vote_delegations
                .iter()
                .map(|(stake_cred, drep)| {
                    let (drep_type, drep_hash) = match drep {
                        DRep::KeyHash(h) => (0u8, Some(h.as_ref()[..28].to_vec())),
                        DRep::ScriptHash(h) => (1u8, Some(h.as_ref().to_vec())),
                        DRep::Abstain => (2u8, None),
                        DRep::NoConfidence => (3u8, None),
                    };
                    DRepDelegationEntry {
                        credential_hash: hash32_padded_to_28_bytes(stake_cred),
                        credential_type: ls.script_stake_credentials.contains(stake_cred) as u8,
                        drep_type,
                        drep_hash,
                    }
                })
                .collect()
        };

        // Build ratify_enacted proposals from governance.last_ratified.
        // Include the full GovAction so the CBOR encoder can faithfully reproduce
        // the action body in GetRatifyState responses (same fix as for governance_proposals).
        let ratify_enacted = ls
            .governance
            .last_ratified
            .iter()
            .map(|(action_id, state)| {
                let action_type = gov_action_type_str(&state.procedure.gov_action);
                let (committee_votes, drep_votes, spo_votes) = build_vote_maps(&ls, action_id);
                let proposal = ProposalSnapshot {
                    tx_id: action_id.transaction_id.as_ref().to_vec(),
                    action_index: action_id.action_index,
                    action_type: action_type.to_string(),
                    proposed_epoch: state.proposed_epoch.0,
                    expires_epoch: state.expires_epoch.0,
                    yes_votes: state.yes_votes,
                    no_votes: state.no_votes,
                    abstain_votes: state.abstain_votes,
                    deposit: state.procedure.deposit.0,
                    return_addr: state.procedure.return_addr.clone(),
                    anchor_url: state.procedure.anchor.url.clone(),
                    anchor_hash: state.procedure.anchor.data_hash.as_ref().to_vec(),
                    gov_action: state.procedure.gov_action.clone(),
                    committee_votes,
                    drep_votes,
                    spo_votes,
                };
                let gov_id = torsten_network::query_handler::GovActionId {
                    tx_id: action_id.transaction_id.as_ref().to_vec(),
                    action_index: action_id.action_index,
                };
                (proposal, gov_id)
            })
            .collect();

        let snapshot = torsten_network::NodeStateSnapshot {
            tip: ls.tip.clone(),
            epoch: ls.epoch,
            era: ls.era.to_era_index(),
            block_number: ls.current_block_number(),
            system_start: self
                .shelley_genesis
                .as_ref()
                .map(|g| g.system_start.clone())
                .unwrap_or_else(|| self.config.network.system_start().to_string()),
            utxo_count: ls.utxo_set.len(),
            delegations_count: ls.delegations.len(),
            pool_count: ls.pool_params.len(),
            treasury: ls.treasury.0,
            reserves: ls.reserves.0,
            // Active DRep count: only DReps whose activity window has not expired.
            // Inactive DReps (active=false) remain registered in the map until
            // explicitly deregistered via UnregDRep, but external tools (Koios,
            // cardano-cli) report only the active count.
            drep_count: ls.governance.active_drep_count(),
            proposal_count: ls.governance.proposals.len(),
            protocol_params,
            stake_pools,
            drep_entries,
            governance_proposals,
            enacted_pparam_update: ls
                .governance
                .enacted_pparam_update
                .as_ref()
                .map(|id| (id.transaction_id.as_ref().to_vec(), id.action_index)),
            enacted_hard_fork: ls
                .governance
                .enacted_hard_fork
                .as_ref()
                .map(|id| (id.transaction_id.as_ref().to_vec(), id.action_index)),
            enacted_committee: ls
                .governance
                .enacted_committee
                .as_ref()
                .map(|id| (id.transaction_id.as_ref().to_vec(), id.action_index)),
            enacted_constitution: ls
                .governance
                .enacted_constitution
                .as_ref()
                .map(|id| (id.transaction_id.as_ref().to_vec(), id.action_index)),
            committee,
            constitution_url: ls
                .governance
                .constitution
                .as_ref()
                .map(|c| c.anchor.url.clone())
                .unwrap_or_default(),
            constitution_hash: ls
                .governance
                .constitution
                .as_ref()
                .map(|c| c.anchor.data_hash.as_ref().to_vec())
                .unwrap_or_else(|| vec![0u8; 32]),
            constitution_script: ls
                .governance
                .constitution
                .as_ref()
                .and_then(|c| c.script_hash.as_ref().map(|h| h.as_ref().to_vec())),
            stake_addresses,
            stake_snapshots,
            snap_mark,
            snap_set,
            snap_go,
            snap_fee: 0, // Haskell tracks accumulated unclaimed fees; we use 0 as approximation
            epoch_blocks_by_pool,
            pool_params_entries,
            pending_retirements: ls
                .pending_retirements
                .iter()
                .map(|(epoch, pools)| {
                    (epoch.0, pools.iter().map(|h| h.as_ref().to_vec()).collect())
                })
                .collect(),
            pool_deposit: ls.protocol_params.pool_deposit.0,
            epoch_length: ls.epoch_length,
            slot_length_secs: self.shelley_genesis.as_ref().map_or(1, |g| g.slot_length),
            network_magic: self.network_magic as u32,
            security_param: self.consensus.security_param,
            stake_deleg_deposits,
            drep_stake_distr,
            vote_delegatees,
            drep_delegations,
            era_summaries: self.build_era_summaries(&ls),
            active_slots_coeff_num: self.shelley_genesis.as_ref().map_or(1, |g| {
                let (n, _) = float_to_rational(g.active_slots_coeff);
                n
            }),
            active_slots_coeff_den: self.shelley_genesis.as_ref().map_or(20, |g| {
                let (_, d) = float_to_rational(g.active_slots_coeff);
                d
            }),
            slots_per_kes_period: self
                .shelley_genesis
                .as_ref()
                .map_or(129600, |g| g.slots_per_k_e_s_period),
            max_kes_evolutions: self
                .shelley_genesis
                .as_ref()
                .map_or(62, |g| g.max_k_e_s_evolutions),
            update_quorum: self.shelley_genesis.as_ref().map_or(5, |g| g.update_quorum),
            max_lovelace_supply: self
                .shelley_genesis
                .as_ref()
                .map_or(45_000_000_000_000_000, |g| g.max_lovelace_supply),
            ratify_enacted,
            ratify_expired: ls
                .governance
                .last_expired
                .iter()
                .map(|id| torsten_network::query_handler::GovActionId {
                    tx_id: id.transaction_id.as_ref().to_vec(),
                    action_index: id.action_index,
                })
                .collect(),
            ratify_delayed: ls.governance.last_ratify_delayed,
            epoch_nonce: ls.epoch_nonce.as_ref().to_vec(),
            evolving_nonce: ls.evolving_nonce.as_ref().to_vec(),
            candidate_nonce: ls.candidate_nonce.as_ref().to_vec(),
            lab_nonce: ls.lab_nonce.as_ref().to_vec(),
            total_active_stake: ls
                .pool_params
                .keys()
                .filter_map(|pid| {
                    ls.snapshots
                        .set
                        .as_ref()
                        .and_then(|s| s.pool_stake.get(pid))
                        .map(|s| s.0)
                })
                .sum(),
            total_rewards: ls.reward_accounts.values().map(|r| r.0).sum(),
            active_delegations: ls.delegations.len() as u64,
            protocol_version_major: ls.protocol_params.protocol_version_major,
            protocol_version_minor: ls.protocol_params.protocol_version_minor,
            genesis_config: self.shelley_genesis.as_ref().map(|g| {
                let gp = &g.protocol_params;
                // Convert a0 from f64 to rational
                let (a0_num, a0_den) = float_to_rational(gp.a0);
                let (rho_num, rho_den) = float_to_rational(gp.rho);
                let (tau_num, tau_den) = float_to_rational(gp.tau);
                let (asc_num, asc_den) = float_to_rational(g.active_slots_coeff);
                GenesisConfigSnapshot {
                    system_start: g.system_start.clone(),
                    network_magic: g.network_magic as u32,
                    network_id: if g.network_id == "Mainnet" { 1 } else { 0 },
                    active_slots_coeff_num: asc_num,
                    active_slots_coeff_den: asc_den,
                    security_param: g.security_param,
                    epoch_length: g.epoch_length,
                    slots_per_kes_period: g.slots_per_k_e_s_period,
                    max_kes_evolutions: g.max_k_e_s_evolutions,
                    slot_length_micros: g.slot_length * 1_000_000,
                    update_quorum: g.update_quorum,
                    max_lovelace_supply: g.max_lovelace_supply,
                    protocol_params: ShelleyPParamsSnapshot {
                        min_fee_a: gp.min_fee_a,
                        min_fee_b: gp.min_fee_b,
                        max_block_body_size: gp.max_block_body_size as u32,
                        max_tx_size: gp.max_tx_size as u32,
                        max_block_header_size: gp.max_block_header_size as u16,
                        key_deposit: gp.key_deposit,
                        pool_deposit: gp.pool_deposit,
                        e_max: gp.e_max as u32,
                        n_opt: gp.n_opt as u16,
                        a0_num,
                        a0_den,
                        rho_num,
                        rho_den,
                        tau_num,
                        tau_den,
                        d_num: 0,
                        d_den: 1,
                        protocol_version_major: gp.protocol_version.major,
                        protocol_version_minor: gp.protocol_version.minor,
                        min_utxo_value: gp.min_u_tx_o_value,
                        min_pool_cost: gp.min_pool_cost,
                    },
                    gen_delegs: Vec::new(),
                }
            }),
        };

        // Drop the ledger read lock before acquiring the query handler write lock
        drop(ls);

        let mut handler = self.query_handler.write().await;
        handler.update_state(snapshot);
    }

    /// Build era summaries for GetEraHistory responses.
    ///
    /// For testnets (preview/preprod), Shelley starts at slot 0 with uniform parameters.
    /// For mainnet, Byron has 20s slots and 21600 slot epochs before Shelley at slot 4492800.
    /// We produce a simplified summary covering Byron (if mainnet) + Shelley-through-Conway.
    pub fn build_era_summaries(
        &self,
        ls: &torsten_ledger::LedgerState,
    ) -> Vec<torsten_network::query_handler::EraSummary> {
        use torsten_network::query_handler::{EraBound, EraSummary};

        let shelley_epoch_length = self
            .shelley_genesis
            .as_ref()
            .map(|g| g.epoch_length)
            .unwrap_or(432000);
        let shelley_slot_length_ms = self
            .shelley_genesis
            .as_ref()
            .map(|g| g.slot_length * 1000)
            .unwrap_or(1000);
        let k = self
            .shelley_genesis
            .as_ref()
            .map(|g| g.security_param)
            .unwrap_or(2160);
        let active_slots_coeff = self
            .shelley_genesis
            .as_ref()
            .map(|g| g.active_slots_coeff)
            .unwrap_or(0.05);

        let is_mainnet = self.network_magic == 764824073;

        // Byron params: epoch length and slot duration from genesis
        let byron_epoch_len: u64 = if self.byron_epoch_length > 0 {
            self.byron_epoch_length
        } else if is_mainnet {
            21600
        } else {
            4320
        };
        let byron_slot_len_ms: u64 = self.byron_slot_duration_ms;
        let byron_safe_zone = k * 2; // Byron safe zone = 2k (864 for preview, matches Haskell)
        let byron_genesis_window = k * 2;

        // Shelley+ safe zone and genesis window: 3 * k / f
        let shelley_safe_zone = (3.0 * k as f64 / active_slots_coeff).floor() as u64;
        let shelley_genesis_window = shelley_safe_zone;

        if is_mainnet {
            // Mainnet: Byron ran for 208 epochs with 21600-slot epochs at 20s slots
            let byron_end_epoch: u64 = 208;
            let byron_end_slot = byron_end_epoch * byron_epoch_len;
            let byron_end_time_pico =
                byron_end_slot as u128 * byron_slot_len_ms as u128 * 1_000_000_000;

            // Compute how many Shelley slots have elapsed since Byron ended
            let shelley_start_slot = byron_end_slot;
            let shelley_start_epoch = byron_end_epoch;

            // Current epoch determines how far the Shelley+ eras extend
            let current_epoch = ls.epoch.0;

            // For mainnet, Babbage started at epoch 365, Conway at epoch 517
            let babbage_epoch: u64 = 365;
            let conway_epoch: u64 = 517;

            let babbage_slot =
                shelley_start_slot + (babbage_epoch - shelley_start_epoch) * shelley_epoch_length;
            let babbage_time_pico = byron_end_time_pico
                + (babbage_slot - shelley_start_slot) as u128
                    * shelley_slot_length_ms as u128
                    * 1_000_000_000;

            let conway_slot =
                shelley_start_slot + (conway_epoch - shelley_start_epoch) * shelley_epoch_length;
            let conway_time_pico = byron_end_time_pico
                + (conway_slot - shelley_start_slot) as u128
                    * shelley_slot_length_ms as u128
                    * 1_000_000_000;

            let shelley_era =
                |start_slot, start_epoch, start_time: u128, end: Option<EraBound>| EraSummary {
                    start_slot,
                    start_epoch,
                    start_time_pico: start_time as u64,
                    end,
                    epoch_size: shelley_epoch_length,
                    slot_length_ms: shelley_slot_length_ms,
                    safe_zone: shelley_safe_zone,
                    genesis_window: shelley_genesis_window,
                };

            let bound = |slot, epoch, time_pico: u128| EraBound {
                slot,
                epoch,
                time_pico: time_pico as u64,
            };

            let mut summaries = vec![
                // Byron
                EraSummary {
                    start_slot: 0,
                    start_epoch: 0,
                    start_time_pico: 0,
                    end: Some(bound(byron_end_slot, byron_end_epoch, byron_end_time_pico)),
                    epoch_size: byron_epoch_len,
                    slot_length_ms: byron_slot_len_ms,
                    safe_zone: byron_safe_zone,
                    genesis_window: byron_genesis_window,
                },
                // Shelley (208..365)
                shelley_era(
                    shelley_start_slot,
                    shelley_start_epoch,
                    byron_end_time_pico,
                    if current_epoch >= babbage_epoch {
                        Some(bound(babbage_slot, babbage_epoch, babbage_time_pico))
                    } else {
                        None
                    },
                ),
            ];

            if current_epoch >= babbage_epoch {
                // Babbage (365..517)
                summaries.push(shelley_era(
                    babbage_slot,
                    babbage_epoch,
                    babbage_time_pico,
                    if current_epoch >= conway_epoch {
                        Some(bound(conway_slot, conway_epoch, conway_time_pico))
                    } else {
                        None
                    },
                ));
            }
            if current_epoch >= conway_epoch {
                // Conway (517..current)
                summaries.push(shelley_era(
                    conway_slot,
                    conway_epoch,
                    conway_time_pico,
                    None,
                ));
            }

            summaries
        } else {
            // Testnets: Byron/Shelley/Allegra/Mary/Alonzo all start at epoch 0 (instant HF)
            // then Babbage and Conway at their actual transition epochs.
            //
            // The Haskell node returns era summaries matching the HFC type list.
            // For preview: Byron(0) → Shelley(0) → Allegra(0) → Mary(0) → Alonzo(0→3) →
            //              Babbage(3→646) → Conway(646→...)
            let origin = EraBound {
                slot: 0,
                epoch: 0,
                time_pico: 0,
            };

            // Build Shelley-era template (all Shelley+ eras share same params)
            let shelley_era = |start: EraBound, end: Option<EraBound>| EraSummary {
                start_slot: start.slot,
                start_epoch: start.epoch,
                start_time_pico: start.time_pico,
                end,
                epoch_size: shelley_epoch_length,
                slot_length_ms: shelley_slot_length_ms,
                safe_zone: shelley_safe_zone,
                genesis_window: shelley_genesis_window,
            };

            // Determine era transitions from ledger state
            // Preview testnet: Byron/Shelley/Allegra/Mary all at epoch 0
            // Alonzo ends at epoch 3, Babbage at epoch 646, Conway ongoing
            let current_epoch = ls.epoch.0;

            // For preview: all pre-Alonzo eras are instant (start=end=origin)
            // Alonzo starts at origin, ends at epoch 3
            let alonzo_end_epoch: u64 = if current_epoch >= 3 { 3 } else { 0 };
            let alonzo_end_slot = alonzo_end_epoch * shelley_epoch_length;
            let alonzo_end_time_pico =
                alonzo_end_slot as u128 * shelley_slot_length_ms as u128 * 1_000_000_000;

            // Babbage starts at epoch 3, ends at epoch 646
            let babbage_end_epoch: u64 = 646;
            let babbage_end_slot = babbage_end_epoch * shelley_epoch_length;
            let babbage_end_time_pico =
                babbage_end_slot as u128 * shelley_slot_length_ms as u128 * 1_000_000_000;

            let mut summaries = vec![
                // Byron (instant transition at epoch 0)
                EraSummary {
                    start_slot: 0,
                    start_epoch: 0,
                    start_time_pico: 0,
                    end: Some(origin.clone()),
                    epoch_size: byron_epoch_len,
                    slot_length_ms: byron_slot_len_ms,
                    safe_zone: byron_safe_zone,
                    genesis_window: byron_genesis_window,
                },
                // Shelley (instant at epoch 0)
                shelley_era(origin.clone(), Some(origin.clone())),
                // Allegra (instant at epoch 0)
                shelley_era(origin.clone(), Some(origin.clone())),
                // Mary (instant at epoch 0)
                shelley_era(origin.clone(), Some(origin.clone())),
            ];

            if current_epoch < alonzo_end_epoch {
                // Still in Alonzo or earlier — unbounded
                summaries.push(shelley_era(origin, None));
            } else {
                let alonzo_end = EraBound {
                    slot: alonzo_end_slot,
                    epoch: alonzo_end_epoch,
                    time_pico: alonzo_end_time_pico as u64,
                };
                // Alonzo (epoch 0..3)
                summaries.push(shelley_era(origin, Some(alonzo_end.clone())));

                if current_epoch < babbage_end_epoch {
                    // Babbage (epoch 3..unbounded)
                    summaries.push(shelley_era(alonzo_end, None));
                } else {
                    let babbage_end = EraBound {
                        slot: babbage_end_slot,
                        epoch: babbage_end_epoch,
                        time_pico: babbage_end_time_pico as u64,
                    };
                    // Babbage (epoch 3..646)
                    summaries.push(shelley_era(alonzo_end, Some(babbage_end.clone())));
                    // Conway (epoch 646..unbounded)
                    summaries.push(shelley_era(babbage_end, None));
                }
            }

            summaries
        }
    }
}

// ─── Private helpers ──────────────────────────────────────────────────────────

/// Return the canonical action-type string for a `GovAction`.
fn gov_action_type_str(action: &torsten_primitives::transaction::GovAction) -> &'static str {
    use torsten_primitives::transaction::GovAction;
    match action {
        GovAction::ParameterChange { .. } => "ParameterChange",
        GovAction::HardForkInitiation { .. } => "HardForkInitiation",
        GovAction::TreasuryWithdrawals { .. } => "TreasuryWithdrawals",
        GovAction::NoConfidence { .. } => "NoConfidence",
        GovAction::UpdateCommittee { .. } => "UpdateCommittee",
        GovAction::NewConstitution { .. } => "NewConstitution",
        GovAction::InfoAction => "InfoAction",
    }
}

/// Build per-credential committee/DRep/SPO vote vectors for a governance action.
#[allow(clippy::type_complexity)]
fn build_vote_maps(
    ls: &torsten_ledger::LedgerState,
    action_id: &torsten_primitives::transaction::GovActionId,
) -> (
    Vec<(Vec<u8>, u8, u8)>,
    Vec<(Vec<u8>, u8, u8)>,
    Vec<(Vec<u8>, u8)>,
) {
    use torsten_primitives::transaction::Voter;
    let mut committee_votes = Vec::new();
    let mut drep_votes = Vec::new();
    let mut spo_votes = Vec::new();
    if let Some(votes) = ls.governance.votes_by_action.get(action_id) {
        for (voter, procedure) in votes {
            let vote_u8 = match procedure.vote {
                torsten_primitives::transaction::Vote::No => 0u8,
                torsten_primitives::transaction::Vote::Yes => 1u8,
                torsten_primitives::transaction::Vote::Abstain => 2u8,
            };
            match voter {
                Voter::ConstitutionalCommittee(cred) => {
                    let (cred_type, hash) = credential_to_bytes(cred);
                    committee_votes.push((hash, cred_type, vote_u8));
                }
                Voter::DRep(cred) => {
                    let (cred_type, hash) = credential_to_bytes(cred);
                    drep_votes.push((hash, cred_type, vote_u8));
                }
                Voter::StakePool(pool_hash) => {
                    spo_votes.push((pool_hash.as_ref()[..28].to_vec(), vote_u8));
                }
            }
        }
    }
    (committee_votes, drep_votes, spo_votes)
}
