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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::Hash;

    #[test]
    fn test_stake_distribution_serde_roundtrip_empty() {
        let sd = StakeDistribution {
            pool_stakes: BTreeMap::new(),
            total_stake: Lovelace(0),
        };
        let json = serde_json::to_string(&sd).unwrap();
        let sd2: StakeDistribution = serde_json::from_str(&json).unwrap();
        assert!(sd2.pool_stakes.is_empty());
        assert_eq!(sd2.total_stake, Lovelace(0));
    }

    #[test]
    fn test_stake_distribution_serde_roundtrip_populated() {
        let pool_id = Hash::from_bytes([0xaa; 32]);
        let mut pool_stakes = BTreeMap::new();
        pool_stakes.insert(
            pool_id,
            PoolStake {
                pool_id,
                stake: Lovelace(1_000_000),
                relative_stake: 0.5,
                delegator_count: 10,
            },
        );
        let sd = StakeDistribution {
            pool_stakes,
            total_stake: Lovelace(2_000_000),
        };
        let json = serde_json::to_string(&sd).unwrap();
        let sd2: StakeDistribution = serde_json::from_str(&json).unwrap();
        assert_eq!(sd2.pool_stakes.len(), 1);
        assert_eq!(sd2.total_stake, Lovelace(2_000_000));
        let ps = sd2.pool_stakes.get(&pool_id).unwrap();
        assert_eq!(ps.stake, Lovelace(1_000_000));
        assert_eq!(ps.delegator_count, 10);
    }

    #[test]
    fn test_delegation_state_serde_roundtrip() {
        let ds = DelegationState {
            credential: Credential::VerificationKey(Hash::from_bytes([0x01; 28])),
            pool: Some(Hash::from_bytes([0xbb; 32])),
            reward_balance: Lovelace(500),
            deposit: Lovelace(2_000_000),
        };
        let json = serde_json::to_string(&ds).unwrap();
        let ds2: DelegationState = serde_json::from_str(&json).unwrap();
        assert_eq!(ds2.reward_balance, Lovelace(500));
        assert_eq!(ds2.deposit, Lovelace(2_000_000));
        assert!(ds2.pool.is_some());
    }

    #[test]
    fn test_delegation_state_no_pool() {
        let ds = DelegationState {
            credential: Credential::Script(Hash::from_bytes([0x02; 28])),
            pool: None,
            reward_balance: Lovelace(0),
            deposit: Lovelace(0),
        };
        let json = serde_json::to_string(&ds).unwrap();
        let ds2: DelegationState = serde_json::from_str(&json).unwrap();
        assert!(ds2.pool.is_none());
    }
}
