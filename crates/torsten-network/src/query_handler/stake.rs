//! Stake and delegation query handlers (tags 10, 16, 17, 19, 20, 21, 22).

use tracing::debug;

use super::parse_credential_set;
use super::types::{NodeStateSnapshot, QueryResult};

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

/// Handle GetPoolState (tag 19) -- returns pool params (same data as tag 17).
///
/// Argument: tag(258) Set<KeyHash StakePool>
pub(crate) fn handle_pool_state(
    state: &NodeStateSnapshot,
    decoder: &mut minicbor::Decoder<'_>,
) -> QueryResult {
    debug!("Query: GetPoolState");
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
