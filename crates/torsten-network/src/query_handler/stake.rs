//! Stake and delegation query handlers (tags 10, 16, 17, 18, 19, 20, 21, 22).

use tracing::debug;

use super::parse_credential_set;
use super::types::{LedgerPeerEntry, NodeStateSnapshot, PoolRewardInfo, QueryResult};

/// Handle GetFilteredDelegationsAndRewardAccounts (tag 10).
///
/// Argument: tag(258) Set<Credential> where Credential = [0|1, hash(28)]
pub(crate) fn handle_filtered_delegations(
    state: &NodeStateSnapshot,
    decoder: &mut minicbor::Decoder<'_>,
) -> QueryResult {
    debug!("Query: GetFilteredDelegationsAndRewardAccounts");
    let filter_hashes = parse_credential_set(decoder);
    if filter_hashes.is_empty() {
        QueryResult::StakeAddressInfo(state.stake_addresses.clone())
    } else {
        let filtered = state
            .stake_addresses
            .iter()
            .filter(|s| filter_hashes.iter().any(|h| h == &s.credential_hash))
            .cloned()
            .collect();
        QueryResult::StakeAddressInfo(filtered)
    }
}

/// Handle GetStakePools (tag 16) -- returns Set<KeyHash StakePool>.
pub(crate) fn handle_stake_pools(state: &NodeStateSnapshot) -> QueryResult {
    debug!("Query: GetStakePools");
    let pool_ids: Vec<Vec<u8>> = state
        .stake_pools
        .iter()
        .map(|p| p.pool_id.clone())
        .collect();
    QueryResult::StakePools(pool_ids)
}

/// Handle GetStakePoolParams (tag 17).
///
/// Argument: tag(258) Set<KeyHash StakePool>
pub(crate) fn handle_stake_pool_params(
    state: &NodeStateSnapshot,
    decoder: &mut minicbor::Decoder<'_>,
) -> QueryResult {
    debug!("Query: GetStakePoolParams");
    let filter_pools = parse_pool_id_set(decoder);
    if filter_pools.is_empty() {
        QueryResult::PoolParams(state.pool_params_entries.clone())
    } else {
        let filtered = state
            .pool_params_entries
            .iter()
            .filter(|p| filter_pools.iter().any(|h| h == &p.pool_id))
            .cloned()
            .collect();
        QueryResult::PoolParams(filtered)
    }
}

/// Handle GetPoolState (tag 19) -- returns QueryPoolStateResult.
///
/// Wire format: array(4) [poolParams_map, futurePoolParams_map, retiring_map, deposits_map]
/// Argument: tag(258) Set<KeyHash StakePool>
pub(crate) fn handle_pool_state(
    state: &NodeStateSnapshot,
    decoder: &mut minicbor::Decoder<'_>,
) -> QueryResult {
    debug!("Query: GetPoolState");
    let filter_pools = parse_pool_id_set(decoder);

    let pool_params = if filter_pools.is_empty() {
        state.pool_params_entries.clone()
    } else {
        state
            .pool_params_entries
            .iter()
            .filter(|p| filter_pools.iter().any(|h| h == &p.pool_id))
            .cloned()
            .collect()
    };

    // Build retiring map: flatten pending_retirements into (pool_id, epoch) pairs
    let mut retiring = Vec::new();
    for (epoch, pools) in &state.pending_retirements {
        for pool_id in pools {
            if filter_pools.is_empty() || filter_pools.iter().any(|h| h == pool_id) {
                retiring.push((pool_id.clone(), *epoch));
            }
        }
    }

    // Build deposits map: each registered pool has pool_deposit
    let deposits: Vec<(Vec<u8>, u64)> = pool_params
        .iter()
        .map(|p| (p.pool_id.clone(), state.pool_deposit))
        .collect();

    QueryResult::PoolState {
        pool_params,
        future_pool_params: Vec::new(), // No future params tracking yet
        retiring,
        deposits,
    }
}

/// Handle GetStakeDistribution2 (tag 37) — new PoolDistr format.
///
/// Returns: array(2)[pool_map, total_active_stake]
/// Each pool entry: array(3)[stake_rational, compact_lovelace, vrf_hash]
pub(crate) fn handle_stake_distribution2(state: &NodeStateSnapshot) -> QueryResult {
    debug!("Query: GetStakeDistribution2");
    let total_active_stake: u64 = state.stake_pools.iter().map(|p| p.stake).sum();
    QueryResult::PoolDistr2 {
        pools: state.stake_pools.clone(),
        total_active_stake: total_active_stake.max(1), // NonZero
    }
}

/// Handle GetPoolDistr2 (tag 36) — filtered new PoolDistr format.
///
/// Argument: Maybe (tag(258) Set<KeyHash StakePool>)
pub(crate) fn handle_pool_distr2(
    state: &NodeStateSnapshot,
    decoder: &mut minicbor::Decoder<'_>,
) -> QueryResult {
    debug!("Query: GetPoolDistr2");
    let filter_pools = parse_pool_id_set(decoder);
    let total_active_stake: u64 = state.stake_pools.iter().map(|p| p.stake).sum();
    if filter_pools.is_empty() {
        QueryResult::PoolDistr2 {
            pools: state.stake_pools.clone(),
            total_active_stake: total_active_stake.max(1),
        }
    } else {
        let filtered: Vec<_> = state
            .stake_pools
            .iter()
            .filter(|p| filter_pools.iter().any(|h| h == &p.pool_id))
            .cloned()
            .collect();
        QueryResult::PoolDistr2 {
            pools: filtered,
            total_active_stake: total_active_stake.max(1),
        }
    }
}

/// Handle GetSPOStakeDistr (tag 30) — filtered SPO stake distribution.
///
/// Argument: tag(258) Set<KeyHash StakePool>
/// Returns: Map<pool_hash(28), Coin> — SPO voting power per pool (lovelace).
///
/// NOTE: This is NOT the same as GetStakeDistribution (tag 5) which uses
/// IndividualPoolStake (rational + VRF hash). GetSPOStakeDistr returns a plain
/// map from pool key hash to absolute stake in lovelace, used for governance
/// vote tallying.
pub(crate) fn handle_spo_stake_distr(
    state: &NodeStateSnapshot,
    decoder: &mut minicbor::Decoder<'_>,
) -> QueryResult {
    debug!("Query: GetSPOStakeDistr");
    let filter_pools = parse_pool_id_set(decoder);
    let entries: Vec<(Vec<u8>, u64)> = if filter_pools.is_empty() {
        state
            .stake_pools
            .iter()
            .map(|p| (p.pool_id.clone(), p.stake))
            .collect()
    } else {
        state
            .stake_pools
            .iter()
            .filter(|p| filter_pools.iter().any(|h| h == &p.pool_id))
            .map(|p| (p.pool_id.clone(), p.stake))
            .collect()
    };
    QueryResult::SPOStakeDistr(entries)
}

/// Handle GetStakeSnapshots (tag 20).
pub(crate) fn handle_stake_snapshots(state: &NodeStateSnapshot) -> QueryResult {
    debug!("Query: GetStakeSnapshots");
    QueryResult::StakeSnapshots(state.stake_snapshots.clone())
}

/// Handle GetPoolDistr (tag 21) -- returns pool stake distribution.
///
/// Argument: tag(258) Set<KeyHash StakePool> (optional filter)
pub(crate) fn handle_pool_distr(
    state: &NodeStateSnapshot,
    decoder: &mut minicbor::Decoder<'_>,
) -> QueryResult {
    debug!("Query: GetPoolDistr");
    let filter_pools = parse_pool_id_set(decoder);
    if filter_pools.is_empty() {
        QueryResult::PoolDistr(state.stake_pools.clone())
    } else {
        let filtered = state
            .stake_pools
            .iter()
            .filter(|p| filter_pools.iter().any(|h| h == &p.pool_id))
            .cloned()
            .collect();
        QueryResult::PoolDistr(filtered)
    }
}

/// Handle GetStakeDelegDeposits (tag 22).
///
/// Argument: tag(258) Set<Credential>
/// Returns: Map<Credential, Coin> -- deposit amount per registered stake credential
pub(crate) fn handle_stake_deleg_deposits(
    state: &NodeStateSnapshot,
    decoder: &mut minicbor::Decoder<'_>,
) -> QueryResult {
    debug!("Query: GetStakeDelegDeposits");
    let filter_hashes = parse_credential_set(decoder);
    if filter_hashes.is_empty() {
        QueryResult::StakeDelegDeposits(state.stake_deleg_deposits.clone())
    } else {
        let filtered = state
            .stake_deleg_deposits
            .iter()
            .filter(|d| filter_hashes.iter().any(|h| h == &d.credential_hash))
            .cloned()
            .collect();
        QueryResult::StakeDelegDeposits(filtered)
    }
}

/// Handle GetRewardInfoPools (tag 18) — per-pool reward provenance data.
///
/// Returns estimated reward breakdown for each active pool: leader/member rewards,
/// margin, cost, and stake.
pub(crate) fn handle_reward_info_pools(state: &NodeStateSnapshot) -> QueryResult {
    debug!("Query: GetRewardInfoPools");
    let total_active_stake: u64 = state.stake_pools.iter().map(|p| p.stake).sum();
    // Compute reward pot from reserves * rho
    let rho_num = state.protocol_params.rho_num;
    let rho_den = state.protocol_params.rho_den.max(1);
    let total_rewards_pot = (state.reserves as u128 * rho_num as u128 / rho_den as u128) as u64;
    // Treasury tax
    let tau_num = state.protocol_params.tau_num;
    let tau_den = state.protocol_params.tau_den.max(1);
    let treasury_tax = (total_rewards_pot as u128 * tau_num as u128 / tau_den as u128) as u64;
    let distributable = total_rewards_pot.saturating_sub(treasury_tax);

    // Build pool params lookup for cost/margin
    let pool_params_map: std::collections::HashMap<&[u8], &super::types::PoolParamsSnapshot> =
        state
            .pool_params_entries
            .iter()
            .map(|pp| (pp.pool_id.as_slice(), pp))
            .collect();

    let mut entries = Vec::new();
    for pool in &state.stake_pools {
        if pool.stake == 0 || total_active_stake == 0 {
            continue;
        }
        let pool_reward =
            (pool.stake as u128 * distributable as u128 / total_active_stake as u128) as u64;
        let (cost, margin_num, margin_den, owner_stake) =
            if let Some(pp) = pool_params_map.get(pool.pool_id.as_slice()) {
                let os: u64 = pp
                    .owners
                    .iter()
                    .filter_map(|owner_hash| {
                        state
                            .stake_addresses
                            .iter()
                            .find(|sa| sa.credential_hash == *owner_hash)
                            .and_then(|sa| {
                                sa.delegated_pool
                                    .as_ref()
                                    .filter(|dp| dp.as_slice() == pool.pool_id.as_slice())
                                    .map(|_| sa.reward_balance)
                            })
                    })
                    .sum();
                (pp.cost, pp.margin_num, pp.margin_den, os)
            } else {
                (340_000_000, 0u64, 1u64, 0u64)
            };
        let after_cost = pool_reward.saturating_sub(cost);
        let margin_take =
            (after_cost as u128 * margin_num as u128 / margin_den.max(1) as u128) as u64;
        // Leader gets cost + margin; cap at pool_reward to prevent overflow
        let leader_reward = (cost + margin_take).min(pool_reward);
        let member_reward = pool_reward.saturating_sub(leader_reward);
        entries.push(PoolRewardInfo {
            pool_id: pool.pool_id.clone(),
            stake: pool.stake,
            owner_stake,
            pool_reward,
            leader_reward,
            member_reward,
            margin: (margin_num, margin_den),
            cost,
        });
    }
    QueryResult::RewardInfoPools(entries)
}

/// Handle QueryStakePoolDefaultVote (tag 35) — single pool default vote.
///
/// Per CIP-1694, the default vote depends on the pool operator's DRep delegation:
/// - AlwaysAbstain (drep_type=2) → DefaultAbstain = 1
/// - AlwaysNoConfidence (drep_type=3) → DefaultNoConfidence = 2
/// - Specific DRep (drep_type=0|1) → DefaultNo = 0
/// - No delegation → DefaultNo = 0
///
/// Argument: single KeyHash StakePool (28 bytes) — NOT a Set
/// Returns: bare word8 (DefaultVote)
pub(crate) fn handle_pool_default_vote(
    state: &NodeStateSnapshot,
    decoder: &mut minicbor::Decoder<'_>,
) -> QueryResult {
    debug!("Query: QueryStakePoolDefaultVote");

    // Parse single pool hash (28 bytes), NOT a Set
    let pool_hash = decoder.bytes().map(|b| b.to_vec()).unwrap_or_default();

    // Build lookup: owner credential hash → DRep delegation type
    let vote_deleg_map: std::collections::HashMap<&[u8], u8> = state
        .vote_delegatees
        .iter()
        .map(|v| (v.credential_hash.as_slice(), v.drep_type))
        .collect();

    // Find the pool params for the requested pool
    let default_vote = state
        .pool_params_entries
        .iter()
        .find(|pp| pp.pool_id == pool_hash)
        .map(|pp| {
            // Check if any pool owner has a vote delegation
            pp.owners
                .iter()
                .find_map(|owner| vote_deleg_map.get(owner.as_slice()))
                .map(|drep_type| match drep_type {
                    // Haskell DefaultVote encoding:
                    // 0 = DefaultNo, 1 = DefaultAbstain, 2 = DefaultNoConfidence
                    2 => 1, // AlwaysAbstain → DefaultAbstain
                    3 => 2, // AlwaysNoConfidence → DefaultNoConfidence
                    _ => 0, // Specific DRep or other → DefaultNo
                })
                .unwrap_or(0) // No delegation → DefaultNo
        })
        .unwrap_or(0); // Pool not found → DefaultNo

    QueryResult::StakePoolDefaultVote(default_vote)
}

/// Handle GetLedgerPeerSnapshot (tag 34) — relay peers from pool registrations.
///
/// Builds a snapshot of pool relay addresses weighted by stake for peer discovery.
/// Returns: array(2) [version, peers_list]
pub(crate) fn handle_ledger_peer_snapshot(state: &NodeStateSnapshot) -> QueryResult {
    debug!("Query: GetLedgerPeerSnapshot");
    // Build a stake lookup from stake_pools
    let stake_map: std::collections::HashMap<&[u8], u64> = state
        .stake_pools
        .iter()
        .map(|p| (p.pool_id.as_slice(), p.stake))
        .collect();

    let entries: Vec<LedgerPeerEntry> = state
        .pool_params_entries
        .iter()
        .filter(|pp| !pp.relays.is_empty())
        .map(|pp| LedgerPeerEntry {
            pool_id: pp.pool_id.clone(),
            stake: stake_map.get(pp.pool_id.as_slice()).copied().unwrap_or(0),
            relays: pp.relays.clone(),
        })
        .collect();

    QueryResult::LedgerPeerSnapshot(entries)
}

/// Parse a set of pool ID hashes from CBOR.
/// Handles: tag(258) [pool_hash_bytes, ...] or plain array of bytes.
fn parse_pool_id_set(decoder: &mut minicbor::Decoder<'_>) -> Vec<Vec<u8>> {
    let mut pools = Vec::new();
    let _ = decoder.tag();
    if let Ok(Some(n)) = decoder.array() {
        for _ in 0..n {
            if let Ok(bytes) = decoder.bytes() {
                pools.push(bytes.to_vec());
            }
        }
    }
    pools
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query_handler::types::{
        NodeStateSnapshot, PoolParamsSnapshot, ProtocolParamsSnapshot, StakePoolSnapshot,
    };

    fn make_state_with_pools() -> NodeStateSnapshot {
        NodeStateSnapshot {
            reserves: 10_000_000_000,
            protocol_params: ProtocolParamsSnapshot {
                rho_num: 3,
                rho_den: 1000,
                tau_num: 2,
                tau_den: 10,
                ..ProtocolParamsSnapshot::default()
            },
            stake_pools: vec![
                StakePoolSnapshot {
                    pool_id: vec![1u8; 28],
                    stake: 600_000_000,
                    vrf_keyhash: vec![0u8; 32],
                    total_active_stake: 1_000_000_000,
                },
                StakePoolSnapshot {
                    pool_id: vec![2u8; 28],
                    stake: 400_000_000,
                    vrf_keyhash: vec![0u8; 32],
                    total_active_stake: 1_000_000_000,
                },
            ],
            pool_params_entries: vec![
                PoolParamsSnapshot {
                    pool_id: vec![1u8; 28],
                    vrf_keyhash: vec![0u8; 32],
                    pledge: 100_000_000,
                    cost: 340_000_000,
                    margin_num: 5,
                    margin_den: 100,
                    reward_account: vec![0u8; 29],
                    owners: vec![],
                    relays: vec![],
                    metadata_url: None,
                    metadata_hash: None,
                },
                PoolParamsSnapshot {
                    pool_id: vec![2u8; 28],
                    vrf_keyhash: vec![0u8; 32],
                    pledge: 50_000_000,
                    cost: 170_000_000,
                    margin_num: 10,
                    margin_den: 100,
                    reward_account: vec![0u8; 29],
                    owners: vec![],
                    relays: vec![],
                    metadata_url: None,
                    metadata_hash: None,
                },
            ],
            ..NodeStateSnapshot::default()
        }
    }

    #[test]
    fn test_reward_info_pools_returns_all_pools() {
        let state = make_state_with_pools();
        let result = handle_reward_info_pools(&state);
        match result {
            QueryResult::RewardInfoPools(pools) => {
                assert_eq!(pools.len(), 2);
                // Pool 1 has 60% stake, pool 2 has 40%
                assert_eq!(pools[0].pool_id, vec![1u8; 28]);
                assert_eq!(pools[1].pool_id, vec![2u8; 28]);
                assert_eq!(pools[0].stake, 600_000_000);
                assert_eq!(pools[1].stake, 400_000_000);
                assert_eq!(pools[0].margin, (5, 100));
                assert_eq!(pools[1].margin, (10, 100));
                assert_eq!(pools[0].cost, 340_000_000);
                assert_eq!(pools[1].cost, 170_000_000);
            }
            _ => panic!("Expected RewardInfoPools"),
        }
    }

    #[test]
    fn test_reward_info_pools_reward_split() {
        let state = make_state_with_pools();
        let result = handle_reward_info_pools(&state);
        match result {
            QueryResult::RewardInfoPools(pools) => {
                for pool in &pools {
                    // leader_reward + member_reward = pool_reward
                    assert_eq!(
                        pool.leader_reward + pool.member_reward,
                        pool.pool_reward,
                        "leader + member should equal pool reward for pool {:?}",
                        pool.pool_id[0]
                    );
                    // pool_reward should be > 0
                    assert!(pool.pool_reward > 0);
                }
            }
            _ => panic!("Expected RewardInfoPools"),
        }
    }

    #[test]
    fn test_reward_info_pools_empty() {
        let state = NodeStateSnapshot::default();
        let result = handle_reward_info_pools(&state);
        match result {
            QueryResult::RewardInfoPools(pools) => {
                assert!(pools.is_empty());
            }
            _ => panic!("Expected RewardInfoPools"),
        }
    }

    #[test]
    fn test_reward_info_pools_zero_stake_pool_excluded() {
        let mut state = make_state_with_pools();
        // Set one pool's stake to 0
        state.stake_pools[1].stake = 0;
        let result = handle_reward_info_pools(&state);
        match result {
            QueryResult::RewardInfoPools(pools) => {
                assert_eq!(pools.len(), 1);
                assert_eq!(pools[0].pool_id, vec![1u8; 28]);
            }
            _ => panic!("Expected RewardInfoPools"),
        }
    }

    #[test]
    fn test_spo_stake_distr_no_filter() {
        let state = make_state_with_pools();
        // Empty CBOR: tag(258) + empty array
        let cbor = {
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.tag(minicbor::data::Tag::new(258)).ok();
            enc.array(0).ok();
            buf
        };
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_spo_stake_distr(&state, &mut dec);
        match result {
            QueryResult::SPOStakeDistr(entries) => {
                assert_eq!(entries.len(), 2);
                // Should be (pool_hash, stake_lovelace) pairs
                assert_eq!(entries[0].0, vec![1u8; 28]);
                assert_eq!(entries[0].1, 600_000_000);
                assert_eq!(entries[1].0, vec![2u8; 28]);
                assert_eq!(entries[1].1, 400_000_000);
            }
            _ => panic!("Expected SPOStakeDistr"),
        }
    }

    #[test]
    fn test_spo_stake_distr_filtered() {
        let state = make_state_with_pools();
        let cbor = {
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.tag(minicbor::data::Tag::new(258)).ok();
            enc.array(1).ok();
            enc.bytes(&[1u8; 28]).ok();
            buf
        };
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_spo_stake_distr(&state, &mut dec);
        match result {
            QueryResult::SPOStakeDistr(entries) => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].0, vec![1u8; 28]);
                assert_eq!(entries[0].1, 600_000_000);
            }
            _ => panic!("Expected SPOStakeDistr"),
        }
    }

    #[test]
    fn test_ledger_peer_snapshot_with_relays() {
        use crate::query_handler::types::RelaySnapshot;
        let mut state = make_state_with_pools();
        state.pool_params_entries[0].relays = vec![RelaySnapshot::SingleHostName {
            port: Some(3001),
            dns_name: "relay1.example.com".to_string(),
        }];
        let result = handle_ledger_peer_snapshot(&state);
        match result {
            QueryResult::LedgerPeerSnapshot(peers) => {
                // Only pool 1 has relays
                assert_eq!(peers.len(), 1);
                assert_eq!(peers[0].pool_id, vec![1u8; 28]);
                assert_eq!(peers[0].stake, 600_000_000);
                assert_eq!(peers[0].relays.len(), 1);
            }
            _ => panic!("Expected LedgerPeerSnapshot"),
        }
    }

    #[test]
    fn test_ledger_peer_snapshot_no_relays() {
        let state = make_state_with_pools();
        let result = handle_ledger_peer_snapshot(&state);
        match result {
            QueryResult::LedgerPeerSnapshot(peers) => {
                // No pools have relays in the default fixture
                assert!(peers.is_empty());
            }
            _ => panic!("Expected LedgerPeerSnapshot"),
        }
    }

    #[test]
    fn test_pool_default_vote_no_delegation_with_owners() {
        let mut state = make_state_with_pools();
        // Give pools owners but no vote delegatees
        state.pool_params_entries[0].owners = vec![vec![10u8; 28]];
        state.pool_params_entries[1].owners = vec![vec![20u8; 28]];
        // Query pool 1: no delegation → DefaultNo (0)
        let cbor = {
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.bytes(&[1u8; 28]).ok();
            buf
        };
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_pool_default_vote(&state, &mut dec);
        match result {
            QueryResult::StakePoolDefaultVote(vote) => {
                assert_eq!(vote, 0, "No delegation → DefaultNo (0)");
            }
            _ => panic!("Expected StakePoolDefaultVote"),
        }
    }

    fn make_empty_filter_cbor() -> Vec<u8> {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.tag(minicbor::data::Tag::new(258)).ok();
        enc.array(0).ok();
        buf
    }

    fn make_pool_filter_cbor(pool_id: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.tag(minicbor::data::Tag::new(258)).ok();
        enc.array(1).ok();
        enc.bytes(pool_id).ok();
        buf
    }

    fn make_credential_filter_cbor(cred_hash: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.tag(minicbor::data::Tag::new(258)).ok();
        enc.array(1).ok();
        enc.array(2).ok();
        enc.u8(0).ok(); // KeyHash
        enc.bytes(cred_hash).ok();
        buf
    }

    // ─── GetFilteredDelegations (tag 10) ──────────────────────────────

    #[test]
    fn test_filtered_delegations_no_filter() {
        use crate::query_handler::types::StakeAddressSnapshot;
        let state = NodeStateSnapshot {
            stake_addresses: vec![
                StakeAddressSnapshot {
                    credential_hash: vec![0xAA; 28],
                    delegated_pool: Some(vec![1u8; 28]),
                    reward_balance: 1_000_000,
                },
                StakeAddressSnapshot {
                    credential_hash: vec![0xBB; 28],
                    delegated_pool: None,
                    reward_balance: 0,
                },
            ],
            ..NodeStateSnapshot::default()
        };
        let cbor = make_empty_filter_cbor();
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_filtered_delegations(&state, &mut dec);
        match result {
            QueryResult::StakeAddressInfo(addrs) => assert_eq!(addrs.len(), 2),
            _ => panic!("Expected StakeAddressInfo"),
        }
    }

    #[test]
    fn test_filtered_delegations_filtered() {
        use crate::query_handler::types::StakeAddressSnapshot;
        let state = NodeStateSnapshot {
            stake_addresses: vec![
                StakeAddressSnapshot {
                    credential_hash: vec![0xAA; 28],
                    delegated_pool: Some(vec![1u8; 28]),
                    reward_balance: 1_000_000,
                },
                StakeAddressSnapshot {
                    credential_hash: vec![0xBB; 28],
                    delegated_pool: None,
                    reward_balance: 0,
                },
            ],
            ..NodeStateSnapshot::default()
        };
        let cbor = make_credential_filter_cbor(&[0xAA; 28]);
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_filtered_delegations(&state, &mut dec);
        match result {
            QueryResult::StakeAddressInfo(addrs) => {
                assert_eq!(addrs.len(), 1);
                assert_eq!(addrs[0].credential_hash, vec![0xAA; 28]);
                assert_eq!(addrs[0].reward_balance, 1_000_000);
            }
            _ => panic!("Expected StakeAddressInfo"),
        }
    }

    // ─── GetStakePools (tag 16) ────────────────────────────────────────

    #[test]
    fn test_stake_pools() {
        let state = make_state_with_pools();
        let result = handle_stake_pools(&state);
        match result {
            QueryResult::StakePools(pool_ids) => {
                assert_eq!(pool_ids.len(), 2);
                assert_eq!(pool_ids[0], vec![1u8; 28]);
                assert_eq!(pool_ids[1], vec![2u8; 28]);
            }
            _ => panic!("Expected StakePools"),
        }
    }

    #[test]
    fn test_stake_pools_empty() {
        let state = NodeStateSnapshot::default();
        let result = handle_stake_pools(&state);
        match result {
            QueryResult::StakePools(pool_ids) => assert!(pool_ids.is_empty()),
            _ => panic!("Expected StakePools"),
        }
    }

    // ─── GetStakePoolParams (tag 17) ──────────────────────────────────

    #[test]
    fn test_stake_pool_params_no_filter() {
        let state = make_state_with_pools();
        let cbor = make_empty_filter_cbor();
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_stake_pool_params(&state, &mut dec);
        match result {
            QueryResult::PoolParams(params) => assert_eq!(params.len(), 2),
            _ => panic!("Expected PoolParams"),
        }
    }

    #[test]
    fn test_stake_pool_params_filtered() {
        let state = make_state_with_pools();
        let cbor = make_pool_filter_cbor(&[1u8; 28]);
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_stake_pool_params(&state, &mut dec);
        match result {
            QueryResult::PoolParams(params) => {
                assert_eq!(params.len(), 1);
                assert_eq!(params[0].pool_id, vec![1u8; 28]);
                assert_eq!(params[0].cost, 340_000_000);
            }
            _ => panic!("Expected PoolParams"),
        }
    }

    // ─── GetPoolState (tag 19) ──────────────────────────────────────────

    #[test]
    fn test_pool_state_no_filter() {
        let mut state = make_state_with_pools();
        state.pending_retirements = vec![(150, vec![vec![2u8; 28]])];
        state.pool_deposit = 500_000_000;
        let cbor = make_empty_filter_cbor();
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_pool_state(&state, &mut dec);
        match result {
            QueryResult::PoolState {
                pool_params,
                future_pool_params,
                retiring,
                deposits,
            } => {
                assert_eq!(pool_params.len(), 2);
                assert!(future_pool_params.is_empty());
                assert_eq!(retiring.len(), 1);
                assert_eq!(retiring[0].0, vec![2u8; 28]);
                assert_eq!(retiring[0].1, 150);
                assert_eq!(deposits.len(), 2);
                assert!(deposits.iter().all(|(_, d)| *d == 500_000_000));
            }
            _ => panic!("Expected PoolState"),
        }
    }

    #[test]
    fn test_pool_state_filtered() {
        let mut state = make_state_with_pools();
        state.pending_retirements = vec![(150, vec![vec![1u8; 28], vec![2u8; 28]])];
        let cbor = make_pool_filter_cbor(&[1u8; 28]);
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_pool_state(&state, &mut dec);
        match result {
            QueryResult::PoolState {
                pool_params,
                retiring,
                deposits,
                ..
            } => {
                assert_eq!(pool_params.len(), 1);
                assert_eq!(pool_params[0].pool_id, vec![1u8; 28]);
                // Only pool 1 retirement should be included
                assert_eq!(retiring.len(), 1);
                assert_eq!(retiring[0].0, vec![1u8; 28]);
                assert_eq!(deposits.len(), 1);
            }
            _ => panic!("Expected PoolState"),
        }
    }

    // ─── GetStakeSnapshots (tag 20) ────────────────────────────────────

    #[test]
    fn test_stake_snapshots() {
        use crate::query_handler::types::{PoolStakeSnapshotEntry, StakeSnapshotsResult};
        let state = NodeStateSnapshot {
            stake_snapshots: StakeSnapshotsResult {
                pools: vec![PoolStakeSnapshotEntry {
                    pool_id: vec![1u8; 28],
                    mark_stake: 100,
                    set_stake: 200,
                    go_stake: 300,
                }],
                total_mark_stake: 100,
                total_set_stake: 200,
                total_go_stake: 300,
            },
            ..NodeStateSnapshot::default()
        };
        let result = handle_stake_snapshots(&state);
        match result {
            QueryResult::StakeSnapshots(ss) => {
                assert_eq!(ss.pools.len(), 1);
                assert_eq!(ss.total_mark_stake, 100);
                assert_eq!(ss.total_set_stake, 200);
                assert_eq!(ss.total_go_stake, 300);
            }
            _ => panic!("Expected StakeSnapshots"),
        }
    }

    // ─── GetPoolDistr (tag 21) ──────────────────────────────────────────

    #[test]
    fn test_pool_distr_no_filter() {
        let state = make_state_with_pools();
        let cbor = make_empty_filter_cbor();
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_pool_distr(&state, &mut dec);
        match result {
            QueryResult::PoolDistr(pools) => assert_eq!(pools.len(), 2),
            _ => panic!("Expected PoolDistr"),
        }
    }

    #[test]
    fn test_pool_distr_filtered() {
        let state = make_state_with_pools();
        let cbor = make_pool_filter_cbor(&[2u8; 28]);
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_pool_distr(&state, &mut dec);
        match result {
            QueryResult::PoolDistr(pools) => {
                assert_eq!(pools.len(), 1);
                assert_eq!(pools[0].pool_id, vec![2u8; 28]);
            }
            _ => panic!("Expected PoolDistr"),
        }
    }

    // ─── GetStakeDelegDeposits (tag 22) ─────────────────────────────────

    #[test]
    fn test_stake_deleg_deposits_no_filter() {
        use crate::query_handler::types::StakeDelegDepositEntry;
        let state = NodeStateSnapshot {
            stake_deleg_deposits: vec![
                StakeDelegDepositEntry {
                    credential_hash: vec![0xAA; 28],
                    credential_type: 0,
                    deposit: 2_000_000,
                },
                StakeDelegDepositEntry {
                    credential_hash: vec![0xBB; 28],
                    credential_type: 1,
                    deposit: 2_000_000,
                },
            ],
            ..NodeStateSnapshot::default()
        };
        let cbor = make_empty_filter_cbor();
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_stake_deleg_deposits(&state, &mut dec);
        match result {
            QueryResult::StakeDelegDeposits(deps) => assert_eq!(deps.len(), 2),
            _ => panic!("Expected StakeDelegDeposits"),
        }
    }

    #[test]
    fn test_stake_deleg_deposits_filtered() {
        use crate::query_handler::types::StakeDelegDepositEntry;
        let state = NodeStateSnapshot {
            stake_deleg_deposits: vec![
                StakeDelegDepositEntry {
                    credential_hash: vec![0xAA; 28],
                    credential_type: 0,
                    deposit: 2_000_000,
                },
                StakeDelegDepositEntry {
                    credential_hash: vec![0xBB; 28],
                    credential_type: 0,
                    deposit: 2_000_000,
                },
            ],
            ..NodeStateSnapshot::default()
        };
        let cbor = make_credential_filter_cbor(&[0xBB; 28]);
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_stake_deleg_deposits(&state, &mut dec);
        match result {
            QueryResult::StakeDelegDeposits(deps) => {
                assert_eq!(deps.len(), 1);
                assert_eq!(deps[0].credential_hash, vec![0xBB; 28]);
            }
            _ => panic!("Expected StakeDelegDeposits"),
        }
    }

    // ─── GetStakeDistribution2 (tag 37) / GetPoolDistr2 (tag 36) ──────

    #[test]
    fn test_stake_distribution2() {
        let state = make_state_with_pools();
        let result = handle_stake_distribution2(&state);
        match result {
            QueryResult::PoolDistr2 {
                pools,
                total_active_stake,
            } => {
                assert_eq!(pools.len(), 2);
                assert_eq!(total_active_stake, 1_000_000_000);
            }
            _ => panic!("Expected PoolDistr2"),
        }
    }

    #[test]
    fn test_stake_distribution2_empty() {
        let state = NodeStateSnapshot::default();
        let result = handle_stake_distribution2(&state);
        match result {
            QueryResult::PoolDistr2 {
                pools,
                total_active_stake,
            } => {
                assert!(pools.is_empty());
                assert_eq!(total_active_stake, 1); // NonZero
            }
            _ => panic!("Expected PoolDistr2"),
        }
    }

    #[test]
    fn test_pool_distr2_no_filter() {
        let state = make_state_with_pools();
        let cbor = make_empty_filter_cbor();
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_pool_distr2(&state, &mut dec);
        match result {
            QueryResult::PoolDistr2 {
                pools,
                total_active_stake,
            } => {
                assert_eq!(pools.len(), 2);
                assert_eq!(total_active_stake, 1_000_000_000);
            }
            _ => panic!("Expected PoolDistr2"),
        }
    }

    #[test]
    fn test_pool_distr2_filtered() {
        let state = make_state_with_pools();
        let cbor = make_pool_filter_cbor(&[2u8; 28]);
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_pool_distr2(&state, &mut dec);
        match result {
            QueryResult::PoolDistr2 {
                pools,
                total_active_stake,
            } => {
                assert_eq!(pools.len(), 1);
                assert_eq!(pools[0].pool_id, vec![2u8; 28]);
                // total_active_stake is sum of ALL pools, not filtered
                assert_eq!(total_active_stake, 1_000_000_000);
            }
            _ => panic!("Expected PoolDistr2"),
        }
    }

    #[test]
    fn test_pool_default_vote_with_delegations() {
        use crate::query_handler::types::VoteDelegateeEntry;
        let mut state = make_state_with_pools();
        state.pool_params_entries[0].owners = vec![vec![10u8; 28]];
        state.pool_params_entries[1].owners = vec![vec![20u8; 28]];
        // Owner of pool 1 delegates to AlwaysNoConfidence (type 3)
        // Owner of pool 2 delegates to a specific DRep (type 0)
        state.vote_delegatees = vec![
            VoteDelegateeEntry {
                credential_hash: vec![10u8; 28],
                credential_type: 0,
                drep_type: 3, // AlwaysNoConfidence
                drep_hash: None,
            },
            VoteDelegateeEntry {
                credential_hash: vec![20u8; 28],
                credential_type: 0,
                drep_type: 0, // KeyHash DRep
                drep_hash: Some(vec![30u8; 28]),
            },
        ];

        // Query pool 1 (owner delegates to AlwaysNoConfidence → DefaultNoConfidence = 2)
        let cbor1 = {
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.bytes(&[1u8; 28]).ok();
            buf
        };
        let mut dec = minicbor::Decoder::new(&cbor1);
        let result = handle_pool_default_vote(&state, &mut dec);
        match result {
            QueryResult::StakePoolDefaultVote(vote) => {
                assert_eq!(
                    vote, 2,
                    "AlwaysNoConfidence delegation → DefaultNoConfidence (2)"
                );
            }
            _ => panic!("Expected StakePoolDefaultVote"),
        }

        // Query pool 2 (owner delegates to specific DRep → DefaultNo = 0)
        let cbor2 = {
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.bytes(&[2u8; 28]).ok();
            buf
        };
        let mut dec = minicbor::Decoder::new(&cbor2);
        let result = handle_pool_default_vote(&state, &mut dec);
        match result {
            QueryResult::StakePoolDefaultVote(vote) => {
                assert_eq!(vote, 0, "Specific DRep delegation → DefaultNo (0)");
            }
            _ => panic!("Expected StakePoolDefaultVote"),
        }
    }

    #[test]
    fn test_pool_default_vote_no_delegation() {
        let state = make_state_with_pools();
        // Pool 1 has no owners with vote delegation → DefaultNo = 0
        let cbor = {
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.bytes(&[1u8; 28]).ok();
            buf
        };
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_pool_default_vote(&state, &mut dec);
        match result {
            QueryResult::StakePoolDefaultVote(vote) => {
                assert_eq!(vote, 0, "No delegation → DefaultNo (0)");
            }
            _ => panic!("Expected StakePoolDefaultVote"),
        }
    }

    #[test]
    fn test_pool_default_vote_unknown_pool() {
        let state = make_state_with_pools();
        // Unknown pool → DefaultNo = 0
        let cbor = {
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.bytes(&[0xFFu8; 28]).ok();
            buf
        };
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_pool_default_vote(&state, &mut dec);
        match result {
            QueryResult::StakePoolDefaultVote(vote) => {
                assert_eq!(vote, 0, "Unknown pool → DefaultNo (0)");
            }
            _ => panic!("Expected StakePoolDefaultVote"),
        }
    }
}
