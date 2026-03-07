use crate::credentials::Credential;
use crate::hash::Hash32;
use crate::value::Lovelace;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Snapshot of stake distribution for an epoch
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakeDistribution {
    pub pool_stakes: BTreeMap<Hash32, PoolStake>,
    pub total_stake: Lovelace,
}

/// Individual pool's stake
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolStake {
    pub pool_id: Hash32,
    pub stake: Lovelace,
    pub relative_stake: f64,
    pub delegator_count: u64,
}

/// Delegation state for a single credential
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationState {
    pub credential: Credential,
    pub pool: Option<Hash32>,
    pub reward_balance: Lovelace,
    pub deposit: Lovelace,
}
