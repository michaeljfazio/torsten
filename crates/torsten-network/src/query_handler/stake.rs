//! Stake and delegation query handlers (tags 10, 16, 17, 18, 19, 20, 21, 22).

use tracing::debug;

use super::parse_credential_set;
use super::types::{NodeStateSnapshot, PoolRewardInfo, QueryResult};

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
}
