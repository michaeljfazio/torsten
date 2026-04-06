//! Protocol parameter, genesis, and reward query handlers (tags 2, 3, 4, 5, 8, 11, 12, 13, 14, 29).

use num_bigint::BigInt;
use tracing::debug;

use crate::node::n2c_query::types::{
    GenesisConfigSnapshot, NodeStateSnapshot, NonMyopicRewardEntry, QueryResult,
    ShelleyPParamsSnapshot,
};

// ---------------------------------------------------------------------------
// Exact rational arithmetic (mirrors the Rat struct in dugite-ledger rewards)
// ---------------------------------------------------------------------------

/// Arbitrary-precision rational for exact non-myopic reward calculations.
///
/// Mirrors `dugite_ledger::state::rewards::Rat` so that this crate does not
/// need to depend on dugite-ledger.  All BigInt arithmetic eliminates any
/// overflow risk at mainnet-scale stake values (36 T lovelace denominators).
#[derive(Clone)]
struct Rat {
    n: BigInt,
    d: BigInt,
}

impl Rat {
    fn new(n: impl Into<BigInt>, d: impl Into<BigInt>) -> Self {
        let d = d.into();
        let n = n.into();
        if d == BigInt::from(0) {
            return Rat {
                n: BigInt::from(0),
                d: BigInt::from(1),
            };
        }
        let g = {
            // Euclidean GCD
            let (mut a, mut b) = (
                if n < BigInt::from(0) {
                    -n.clone()
                } else {
                    n.clone()
                },
                if d < BigInt::from(0) {
                    -d.clone()
                } else {
                    d.clone()
                },
            );
            while b != BigInt::from(0) {
                let t = b.clone();
                b = &a % &t;
                a = t;
            }
            if a == BigInt::from(0) {
                BigInt::from(1)
            } else {
                a
            }
        };
        let (n, d) = (&n / &g, &d / &g);
        // Normalize: denominator always positive
        if d < BigInt::from(0) {
            Rat { n: -n, d: -d }
        } else {
            Rat { n, d }
        }
    }

    fn from_u64(n: u64, d: u64) -> Self {
        Rat::new(BigInt::from(n), BigInt::from(d))
    }

    fn add(&self, other: &Rat) -> Rat {
        Rat::new(&self.n * &other.d + &other.n * &self.d, &self.d * &other.d)
    }

    fn sub(&self, other: &Rat) -> Rat {
        Rat::new(&self.n * &other.d - &other.n * &self.d, &self.d * &other.d)
    }

    fn mul(&self, other: &Rat) -> Rat {
        Rat::new(&self.n * &other.n, &self.d * &other.d)
    }

    fn div(&self, other: &Rat) -> Rat {
        if other.n == BigInt::from(0) {
            return Rat::new(0u64, 1u64);
        }
        Rat::new(&self.n * &other.d, &self.d * &other.n)
    }

    fn min_rat(&self, other: &Rat) -> Rat {
        // a/b <= c/d iff a*d <= c*b (b,d > 0)
        if &self.n * &other.d <= &other.n * &self.d {
            self.clone()
        } else {
            other.clone()
        }
    }

    fn floor_u64(&self) -> u64 {
        if self.d == BigInt::from(0) || self.n <= BigInt::from(0) {
            return 0;
        }
        let result = &self.n / &self.d;
        u64::try_from(result).unwrap_or(u64::MAX)
    }
}

// ---------------------------------------------------------------------------
// maxPool' formula
//
// Haskell reference (cardano-ledger-shelley/Cardano/Ledger/Shelley/Rewards.hs):
//
//   maxPool' a0 n0 r sigma p =
//     let z0  = 1 / fromIntegral n0
//         s'  = min p z0
//         s'' = min sigma z0
//     in (r / (1 + a0)) * (s'' + (s' * a0 * (s'' - (s' * (z0 - s'') / z0))) / z0)
//
// Parameters:
//   a0    — pledge influence (protocol param, rational)
//   n_opt — desired number of pools (protocol param)
//   r     — reward pot available for this epoch
//   sigma — pool stake / total stake (rational, pool WITH hypothetical delegator)
//   p     — pledge / total stake (rational, capped at z0)
// ---------------------------------------------------------------------------
fn max_pool_prime(a0_num: u64, a0_den: u64, n_opt: u64, r: u64, sigma: &Rat, p_raw: &Rat) -> u64 {
    let n_opt = n_opt.max(1);
    let a0 = Rat::from_u64(a0_num, a0_den.max(1));
    let z0 = Rat::from_u64(1, n_opt);
    let sigma_c = sigma.min_rat(&z0);
    let p = p_raw.min_rat(&z0);

    // factor4 = (z0 - sigma') / z0
    let f4 = z0.sub(&sigma_c).div(&z0);
    // factor3 = (sigma' - p' * factor4) / z0
    let f3 = sigma_c.sub(&p.mul(&f4)).div(&z0);
    // factor2 = sigma' + p' * a0 * factor3
    let f2 = sigma_c.add(&p.mul(&a0).mul(&f3));
    // factor1 = R / (1 + a0)
    let one = Rat::from_u64(1, 1);
    let f1 = Rat::new(BigInt::from(r), BigInt::from(1)).div(&one.add(&a0));

    f1.mul(&f2).floor_u64()
}

/// Handle GetNonMyopicMemberRewards (tag 2).
///
/// Implements the Haskell `getNonMyopicMemberRewards` formula from
/// cardano-ledger-shelley.
///
/// For each requested stake amount `t` and each registered pool:
///
/// 1. Compute the epoch reward pot:
///    `R = floor(rho * reserves) * (1 - tau)`
///    (Uses eta = 1, the non-myopic / ideal-performance assumption.)
/// 2. Compute total stake (circulation):
///    `total_stake = max_lovelace_supply - reserves`
/// 3. For each pool (active stake `s`, pledge `pledge`):
///    `sigma_hyp = (s + t) / total_stake` — pool stake with hypothetical delegator
///    `p_raw     = pledge / total_stake`
///    `max_pool  = maxPool'(a0, nOpt, R, sigma_hyp, p_raw)`
/// 4. Hypothetical delegator share (assuming ideal performance, perf = 1):
///    If `max_pool > cost`:
///    `member_reward = floor((max_pool - cost) * (1 - margin) * t / (s + t))`
///    Otherwise: `member_reward = 0`
///
/// The result is a map from pool_id to expected lovelace reward for each
/// requested stake amount, suitable for the cardano-wallet delegation advisor.
pub fn handle_non_myopic_rewards(
    state: &NodeStateSnapshot,
    decoder: &mut minicbor::Decoder<'_>,
) -> QueryResult {
    debug!("Query: GetNonMyopicMemberRewards");

    // --- decode the requested stake amounts ---
    let mut amounts = Vec::new();
    if let Ok(Some(n)) = decoder.array() {
        for _ in 0..n {
            if let Ok(amt) = decoder.u64() {
                amounts.push(amt);
            } else {
                decoder.skip().ok();
            }
        }
    }
    let stake_amounts = if amounts.is_empty() {
        // Default: 1 ADA (1,000,000 lovelace) is an arbitrary small stake used
        // when the caller wants pool rankings without a specific amount.
        vec![1_000_000_000_000]
    } else {
        amounts
    };

    // --- reward pot (eta = 1: non-myopic ideal-performance assumption) ---
    //
    // R_gross = floor(rho * reserves)
    // R_net   = R_gross - floor(tau * R_gross)
    //         = R_gross * (1 - tau)
    let rho_num = state.protocol_params.rho_num;
    let rho_den = state.protocol_params.rho_den.max(1);
    let tau_num = state.protocol_params.tau_num;
    let tau_den = state.protocol_params.tau_den.max(1);

    let r_gross = Rat::from_u64(rho_num, rho_den)
        .mul(&Rat::new(BigInt::from(state.reserves), BigInt::from(1)))
        .floor_u64();
    let treasury_cut = Rat::from_u64(tau_num, tau_den)
        .mul(&Rat::new(BigInt::from(r_gross), BigInt::from(1)))
        .floor_u64();
    let reward_pot = r_gross.saturating_sub(treasury_cut);

    // --- total stake (circulation = max_supply - reserves) ---
    //
    // This is sigma's denominator in maxPool', matching calculate_rewards().
    let total_stake = state.max_lovelace_supply.saturating_sub(state.reserves);
    if total_stake == 0 || reward_pot == 0 {
        // No rewards computable — return empty maps for each requested amount.
        let result = stake_amounts
            .iter()
            .map(|&amount| NonMyopicRewardEntry {
                stake_amount: amount,
                pool_rewards: Vec::new(),
            })
            .collect();
        return QueryResult::NonMyopicMemberRewards(result);
    }

    // --- protocol params needed for maxPool' ---
    let a0_num = state.protocol_params.a0_num;
    let a0_den = state.protocol_params.a0_den.max(1);
    let n_opt = state.protocol_params.n_opt.max(1);

    // --- pool-params lookup: pool_id -> (pledge, cost, margin_num, margin_den) ---
    let pool_params_map: std::collections::HashMap<
        &[u8],
        &crate::node::n2c_query::types::PoolParamsSnapshot,
    > = state
        .pool_params_entries
        .iter()
        .map(|pp| (pp.pool_id.as_slice(), pp))
        .collect();

    // --- compute per-amount results ---
    let mut result = Vec::new();

    for &myopic_stake in &stake_amounts {
        let mut pool_rewards: Vec<(Vec<u8>, u64)> = Vec::new();

        for pool in &state.stake_pools {
            let pool_active_stake = pool.stake;

            // Retrieve registered params; use conservative defaults if missing.
            let (pledge, cost, margin_num, margin_den) =
                if let Some(pp) = pool_params_map.get(pool.pool_id.as_slice()) {
                    (pp.pledge, pp.cost, pp.margin_num, pp.margin_den.max(1))
                } else {
                    // Pool has stake distribution entry but no registered params
                    // (unusual, but handle gracefully with zero reward).
                    continue;
                };

            // Hypothetical pool stake: current active stake + the candidate amount.
            // This is the core of the non-myopic model — the delegator is added
            // to this pool, increasing sigma by t/total_stake.
            let hyp_pool_stake = pool_active_stake.saturating_add(myopic_stake);

            // sigma_hyp = (pool_active_stake + t) / total_stake
            let sigma_hyp = Rat::from_u64(hyp_pool_stake, total_stake);

            // p_raw = pledge / total_stake
            let p_raw = Rat::from_u64(pledge, total_stake);

            // maxPool'(sigma_hyp, p_raw) — assumes ideal performance (perf = 1)
            let max_pool = max_pool_prime(a0_num, a0_den, n_opt, reward_pot, &sigma_hyp, &p_raw);

            // Hypothetical member reward for `myopic_stake`:
            //   if max_pool > cost:
            //     member_reward = floor((max_pool - cost) * (1 - margin) * t / (s + t))
            let member_reward = if max_pool > cost && hyp_pool_stake > 0 {
                let remainder = max_pool - cost;
                let one_minus_margin =
                    Rat::from_u64(margin_den - margin_num.min(margin_den), margin_den);
                let delegator_fraction = Rat::from_u64(myopic_stake, hyp_pool_stake);
                Rat::new(BigInt::from(remainder), BigInt::from(1))
                    .mul(&one_minus_margin)
                    .mul(&delegator_fraction)
                    .floor_u64()
            } else {
                0
            };

            pool_rewards.push((pool.pool_id.clone(), member_reward));
        }

        result.push(NonMyopicRewardEntry {
            stake_amount: myopic_stake,
            pool_rewards,
        });
    }

    QueryResult::NonMyopicMemberRewards(result)
}

/// Handle GetCurrentPParams (tag 3).
pub(crate) fn handle_current_pparams(state: &NodeStateSnapshot) -> QueryResult {
    debug!("Query: GetCurrentPParams");
    QueryResult::ProtocolParams(Box::new(state.protocol_params.clone()))
}

/// Handle GetProposedPParamsUpdates (tag 4) -- deprecated in Conway.
pub(crate) fn handle_proposed_pparams_updates() -> QueryResult {
    debug!("Query: GetProposedPParamsUpdates");
    QueryResult::ProposedPParamsUpdates
}

/// Handle GetStakeDistribution (tag 5).
pub(crate) fn handle_stake_distribution(state: &NodeStateSnapshot) -> QueryResult {
    debug!("Query: GetStakeDistribution");
    QueryResult::StakeDistribution(state.stake_pools.clone())
}

/// Handle GetGenesisConfig (tag 11) -- CompactGenesis.
pub(crate) fn handle_genesis_config(state: &NodeStateSnapshot, n2c_version: u16) -> QueryResult {
    debug!("Query: GetGenesisConfig (N2C V{n2c_version})");
    if let Some(ref gc) = state.genesis_config {
        QueryResult::GenesisConfig(Box::new(gc.clone()), n2c_version)
    } else {
        // Fallback: genesis config from node state fields
        QueryResult::GenesisConfig(
            Box::new(GenesisConfigSnapshot {
                system_start: state.system_start.clone(),
                network_magic: state.network_magic,
                network_id: if state.network_magic == 764824073 {
                    1
                } else {
                    0
                },
                active_slots_coeff_num: state.active_slots_coeff_num,
                active_slots_coeff_den: state.active_slots_coeff_den,
                security_param: state.security_param,
                epoch_length: state.epoch_length,
                slots_per_kes_period: state.slots_per_kes_period,
                max_kes_evolutions: state.max_kes_evolutions,
                slot_length_micros: state.slot_length_secs * 1_000_000,
                update_quorum: state.update_quorum,
                max_lovelace_supply: state.max_lovelace_supply,
                protocol_params: ShelleyPParamsSnapshot {
                    min_fee_a: state.protocol_params.min_fee_a,
                    min_fee_b: state.protocol_params.min_fee_b,
                    max_block_body_size: state.protocol_params.max_block_body_size as u32,
                    max_tx_size: state.protocol_params.max_tx_size as u32,
                    max_block_header_size: state.protocol_params.max_block_header_size as u16,
                    key_deposit: state.protocol_params.key_deposit,
                    pool_deposit: state.protocol_params.pool_deposit,
                    e_max: state.protocol_params.e_max as u32,
                    n_opt: state.protocol_params.n_opt as u16,
                    a0_num: state.protocol_params.a0_num,
                    a0_den: state.protocol_params.a0_den,
                    rho_num: state.protocol_params.rho_num,
                    rho_den: state.protocol_params.rho_den,
                    tau_num: state.protocol_params.tau_num,
                    tau_den: state.protocol_params.tau_den,
                    d_num: 0,
                    d_den: 1,
                    protocol_version_major: state.protocol_params.protocol_version_major,
                    protocol_version_minor: state.protocol_params.protocol_version_minor,
                    min_utxo_value: 0,
                    min_pool_cost: state.protocol_params.min_pool_cost,
                },
                gen_delegs: Vec::new(),
            }),
            n2c_version,
        )
    }
}

/// Handle GetAccountState (tag 29) -- treasury + reserves.
pub(crate) fn handle_account_state(state: &NodeStateSnapshot) -> QueryResult {
    debug!("Query: GetAccountState");
    QueryResult::AccountState {
        treasury: state.treasury,
        reserves: state.reserves,
    }
}

/// Handle DebugEpochState (tag 8).
///
/// Returns the full Haskell-compatible `EpochState` structure:
/// `array(4) [ChainAccountState, LedgerState, SnapShots, NonMyopic]`
///
/// `ChainAccountState` = `array(2)[treasury, reserves]`
/// `LedgerState`       = simplified placeholder (CBOR-skippable)
/// `SnapShots`         = `array(4)[mark, set, go, fee]` with real data
/// `NonMyopic`         = `array(2)[likelihoods_map, reward_pot]`
///
/// Tools like db-analyser expect the EpochState structure at the top level of
/// the result; the SnapShots field at position [2] is typically what callers
/// need for epoch-boundary analysis.
pub(crate) fn handle_debug_epoch_state(state: &NodeStateSnapshot) -> QueryResult {
    debug!("Query: DebugEpochState");
    QueryResult::DebugEpochState {
        treasury: state.treasury,
        reserves: state.reserves,
        snap_mark: Box::new(state.snap_mark.clone()),
        snap_set: Box::new(state.snap_set.clone()),
        snap_go: Box::new(state.snap_go.clone()),
        snap_fee: state.snap_fee,
    }
}

/// Handle DebugNewEpochState (tag 12) — full Haskell-compatible NewEpochState.
///
/// cncli's `snapshot` command uses this query to extract the per-credential
/// stake distribution (mark/set/go snapshots) for pool leader-schedule
/// computation.  We return the full array(7) structure that Haskell encodes.
pub(crate) fn handle_debug_new_epoch_state(state: &NodeStateSnapshot) -> QueryResult {
    debug!("Query: DebugNewEpochState");

    // Build block-count maps from the per-pool epoch block counts.
    // cncli doesn't use these for snapshot purposes, but the Haskell structure
    // places them at [0] and [1] of NewEpochState, so we include them.
    let blocks_made_prev: Vec<(Vec<u8>, u64)> = state.epoch_blocks_by_pool.clone();
    let blocks_made_cur: Vec<(Vec<u8>, u64)> = Vec::new();

    QueryResult::DebugNewEpochState {
        epoch: state.epoch.0,
        blocks_made_prev,
        blocks_made_cur,
        treasury: state.treasury,
        reserves: state.reserves,
        snap_mark: Box::new(state.snap_mark.clone()),
        snap_set: Box::new(state.snap_set.clone()),
        snap_go: Box::new(state.snap_go.clone()),
        snap_fee: state.snap_fee,
        total_active_stake: state.total_active_stake,
        pool_distr: state.stake_pools.clone(),
    }
}

/// Handle DebugChainDepState (tag 13).
///
/// Returns the Haskell-compatible `PraosState` CBOR structure.  Haskell uses
/// `encodeVersion 0` (from `Ouroboros.Consensus.Util.Versioned`) which wraps
/// the payload as `array(2)[0, payload]`.  The payload is `array(7)` containing
/// the seven `PraosState` fields from `ouroboros-consensus-protocol-0.13.0.0`
/// (the version shipped with cardano-node 10.6.x / 10.7.x).
///
/// Field order: lastSlot, ocertCounters, evolvingNonce, candidateNonce,
/// epochNonce, labNonce, lastEpochBlockNonce.
///
/// NOTE: The `praosStatePreviousEpochNonce` field (for Peras) was added to the
/// unreleased main branch but is absent from all released cardano-node versions.
/// We deliberately omit it to stay compatible with cardano-cli 10.15.
///
/// The `OCertCounters` map (`praosStateOCertCounters`) is not tracked in
/// `NodeStateSnapshot` — we emit an empty map, which is safe because tools
/// reading this query for nonce inspection do not use the counter map.
pub(crate) fn handle_debug_chain_dep_state(state: &NodeStateSnapshot) -> QueryResult {
    debug!("Query: DebugChainDepState");
    let (last_slot, is_origin) = match state.tip.point.slot() {
        Some(s) => (s.0, false),
        None => (0, true),
    };
    QueryResult::DebugChainDepState {
        last_slot,
        last_slot_is_origin: is_origin,
        // OCertCounters: not tracked per peer; emit empty map
        ocert_counters: Vec::new(),
        evolving_nonce: state.evolving_nonce.clone(),
        candidate_nonce: state.candidate_nonce.clone(),
        epoch_nonce: state.epoch_nonce.clone(),
        lab_nonce: state.lab_nonce.clone(),
        last_epoch_block_nonce: state.lab_nonce.clone(),
    }
}

/// Handle GetRewardProvenance (tag 14) — reward calculation provenance.
///
/// Returns aggregate reward provenance data: total rewards pot, treasury tax,
/// and total active stake for the current epoch.
pub(crate) fn handle_reward_provenance(state: &NodeStateSnapshot) -> QueryResult {
    debug!("Query: GetRewardProvenance");
    let total_active_stake: u64 = state.stake_pools.iter().map(|p| p.stake).sum();
    // Reward pot = reserves * rho (monetary expansion)
    let rho_num = state.protocol_params.rho_num;
    let rho_den = state.protocol_params.rho_den.max(1);
    let total_rewards_pot = (state.reserves as u128 * rho_num as u128 / rho_den as u128) as u64;
    // Treasury tax = reward_pot * tau
    let tau_num = state.protocol_params.tau_num;
    let tau_den = state.protocol_params.tau_den.max(1);
    let treasury_tax = (total_rewards_pot as u128 * tau_num as u128 / tau_den as u128) as u64;
    QueryResult::RewardProvenance {
        epoch: state.epoch.0,
        total_rewards_pot,
        treasury_tax,
        active_stake: total_active_stake,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::n2c_query::types::{
        NodeStateSnapshot, ProtocolParamsSnapshot, StakePoolSnapshot,
    };

    fn make_state() -> NodeStateSnapshot {
        NodeStateSnapshot {
            epoch: dugite_primitives::time::EpochNo(42),
            treasury: 1_000_000_000,
            reserves: 10_000_000_000,
            pool_count: 3,
            utxo_count: 5000,
            block_number: dugite_primitives::time::BlockNo(999),
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
                    stake: 500_000_000,
                    vrf_keyhash: vec![0u8; 32],
                    total_active_stake: 1_000_000_000,
                },
                StakePoolSnapshot {
                    pool_id: vec![2u8; 28],
                    stake: 500_000_000,
                    vrf_keyhash: vec![0u8; 32],
                    total_active_stake: 1_000_000_000,
                },
            ],
            ..NodeStateSnapshot::default()
        }
    }

    #[test]
    fn test_debug_epoch_state() {
        let state = make_state();
        let result = handle_debug_epoch_state(&state);
        match result {
            QueryResult::DebugEpochState {
                treasury, reserves, ..
            } => {
                assert_eq!(treasury, 1_000_000_000);
                assert_eq!(reserves, 10_000_000_000);
            }
            _ => panic!("Expected DebugEpochState"),
        }
    }

    #[test]
    fn test_debug_new_epoch_state() {
        let state = make_state();
        let result = handle_debug_new_epoch_state(&state);
        match result {
            QueryResult::DebugNewEpochState {
                epoch,
                treasury,
                reserves,
                ..
            } => {
                assert_eq!(epoch, 42);
                assert_eq!(treasury, 1_000_000_000);
                assert_eq!(reserves, 10_000_000_000);
            }
            _ => panic!("Expected DebugNewEpochState"),
        }
    }

    #[test]
    fn test_debug_chain_dep_state() {
        let state = make_state();
        let result = handle_debug_chain_dep_state(&state);
        match result {
            QueryResult::DebugChainDepState { last_slot, .. } => {
                assert_eq!(last_slot, 0); // origin tip
            }
            _ => panic!("Expected DebugChainDepState"),
        }
    }

    #[test]
    fn test_reward_provenance() {
        let state = make_state();
        let result = handle_reward_provenance(&state);
        match result {
            QueryResult::RewardProvenance {
                epoch,
                total_rewards_pot,
                treasury_tax,
                active_stake,
            } => {
                assert_eq!(epoch, 42);
                // reserves=10B, rho=3/1000 => pot=30M
                assert_eq!(total_rewards_pot, 30_000_000);
                // pot=30M, tau=2/10 => tax=6M
                assert_eq!(treasury_tax, 6_000_000);
                assert_eq!(active_stake, 1_000_000_000);
            }
            _ => panic!("Expected RewardProvenance"),
        }
    }

    #[test]
    fn test_reward_provenance_zero_reserves() {
        let mut state = make_state();
        state.reserves = 0;
        let result = handle_reward_provenance(&state);
        match result {
            QueryResult::RewardProvenance {
                total_rewards_pot,
                treasury_tax,
                ..
            } => {
                assert_eq!(total_rewards_pot, 0);
                assert_eq!(treasury_tax, 0);
            }
            _ => panic!("Expected RewardProvenance"),
        }
    }

    #[test]
    fn test_current_pparams() {
        let state = make_state();
        let result = handle_current_pparams(&state);
        match result {
            QueryResult::ProtocolParams(pp) => {
                assert_eq!(pp.rho_num, 3);
                assert_eq!(pp.rho_den, 1000);
            }
            _ => panic!("Expected ProtocolParams"),
        }
    }

    #[test]
    fn test_proposed_pparams_updates() {
        let result = handle_proposed_pparams_updates();
        assert!(matches!(result, QueryResult::ProposedPParamsUpdates));
    }

    #[test]
    fn test_stake_distribution() {
        let state = make_state();
        let result = handle_stake_distribution(&state);
        match result {
            QueryResult::StakeDistribution(pools) => {
                assert_eq!(pools.len(), 2);
                assert_eq!(pools[0].stake, 500_000_000);
            }
            _ => panic!("Expected StakeDistribution"),
        }
    }

    #[test]
    fn test_account_state() {
        let state = make_state();
        let result = handle_account_state(&state);
        match result {
            QueryResult::AccountState { treasury, reserves } => {
                assert_eq!(treasury, 1_000_000_000);
                assert_eq!(reserves, 10_000_000_000);
            }
            _ => panic!("Expected AccountState"),
        }
    }

    #[test]
    fn test_genesis_config_from_snapshot() {
        use crate::node::n2c_query::types::GenesisConfigSnapshot;
        let state = NodeStateSnapshot {
            genesis_config: Some(GenesisConfigSnapshot {
                system_start: "2022-04-01T00:00:00Z".to_string(),
                network_magic: 2,
                network_id: 0,
                active_slots_coeff_num: 1,
                active_slots_coeff_den: 20,
                security_param: 2160,
                epoch_length: 86400,
                slots_per_kes_period: 129600,
                max_kes_evolutions: 62,
                slot_length_micros: 1_000_000,
                update_quorum: 5,
                max_lovelace_supply: 45_000_000_000_000_000,
                protocol_params: crate::node::n2c_query::types::ShelleyPParamsSnapshot {
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
                    d_num: 0,
                    d_den: 1,
                    protocol_version_major: 9,
                    protocol_version_minor: 0,
                    min_utxo_value: 0,
                    min_pool_cost: 170_000_000,
                },
                gen_delegs: Vec::new(),
            }),
            ..NodeStateSnapshot::default()
        };
        let result = handle_genesis_config(&state, 0);
        match result {
            QueryResult::GenesisConfig(gc, _version) => {
                assert_eq!(gc.network_magic, 2);
                assert_eq!(gc.network_id, 0);
                assert_eq!(gc.epoch_length, 86400);
            }
            _ => panic!("Expected GenesisConfig"),
        }
    }

    #[test]
    fn test_genesis_config_fallback() {
        // No genesis_config set — should use fallback from state fields
        let state = NodeStateSnapshot {
            system_start: "2022-04-01T00:00:00Z".to_string(),
            network_magic: 2,
            epoch_length: 86400,
            security_param: 2160,
            ..NodeStateSnapshot::default()
        };
        let result = handle_genesis_config(&state, 0);
        match result {
            QueryResult::GenesisConfig(gc, _version) => {
                assert_eq!(gc.network_magic, 2);
                assert_eq!(gc.network_id, 0); // non-mainnet magic → testnet
                assert_eq!(gc.epoch_length, 86400);
            }
            _ => panic!("Expected GenesisConfig"),
        }
    }

    #[test]
    fn test_genesis_config_mainnet_network_id() {
        let state = NodeStateSnapshot {
            network_magic: 764824073, // mainnet
            ..NodeStateSnapshot::default()
        };
        let result = handle_genesis_config(&state, 0);
        match result {
            QueryResult::GenesisConfig(gc, _version) => {
                assert_eq!(gc.network_id, 1); // mainnet = 1
            }
            _ => panic!("Expected GenesisConfig"),
        }
    }

    /// Build a NodeStateSnapshot suitable for non-myopic reward tests.
    ///
    /// Uses a toy network with:
    ///   max_lovelace_supply = 1_000_000_000_000  (1 T)
    ///   reserves            =   900_000_000_000  (900 B) => circulation = 100 B
    ///   rho = 3/1000,  tau = 2/10
    ///   a0  = 3/10,    nOpt = 10
    ///
    /// R_gross = floor(3/1000 * 900B) = 2_700_000_000
    /// treasury = floor(2/10 * 2_700_000_000) = 540_000_000
    /// reward_pot = 2_700_000_000 - 540_000_000 = 2_160_000_000
    fn make_nm_state() -> NodeStateSnapshot {
        use crate::node::n2c_query::types::PoolParamsSnapshot;
        NodeStateSnapshot {
            max_lovelace_supply: 1_000_000_000_000,
            reserves: 900_000_000_000,
            stake_pools: vec![
                // Pool A: 50B stake, pledge 10B, cost 340M, margin 5%
                StakePoolSnapshot {
                    pool_id: vec![0xAAu8; 28],
                    stake: 50_000_000_000,
                    vrf_keyhash: vec![0u8; 32],
                    total_active_stake: 100_000_000_000,
                },
                // Pool B: 30B stake, pledge 1B, cost 340M, margin 1%
                StakePoolSnapshot {
                    pool_id: vec![0xBBu8; 28],
                    stake: 30_000_000_000,
                    vrf_keyhash: vec![0u8; 32],
                    total_active_stake: 100_000_000_000,
                },
            ],
            pool_params_entries: vec![
                PoolParamsSnapshot {
                    pool_id: vec![0xAAu8; 28],
                    vrf_keyhash: vec![0u8; 32],
                    pledge: 10_000_000_000,
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
                    pool_id: vec![0xBBu8; 28],
                    vrf_keyhash: vec![0u8; 32],
                    pledge: 1_000_000_000,
                    cost: 340_000_000,
                    margin_num: 1,
                    margin_den: 100,
                    reward_account: vec![1u8; 29],
                    owners: vec![],
                    relays: vec![],
                    metadata_url: None,
                    metadata_hash: None,
                },
            ],
            protocol_params: ProtocolParamsSnapshot {
                rho_num: 3,
                rho_den: 1000,
                tau_num: 2,
                tau_den: 10,
                a0_num: 3,
                a0_den: 10,
                n_opt: 10,
                ..ProtocolParamsSnapshot::default()
            },
            ..NodeStateSnapshot::default()
        }
    }

    #[test]
    fn test_non_myopic_rewards_basic() {
        let state = make_nm_state();

        // Request 1B lovelace hypothetical stake
        let myopic = 1_000_000_000u64;
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(1).ok();
        enc.u64(myopic).ok();
        let mut dec = minicbor::Decoder::new(&buf);

        let result = handle_non_myopic_rewards(&state, &mut dec);
        let entries = match result {
            QueryResult::NonMyopicMemberRewards(e) => e,
            _ => panic!("Expected NonMyopicMemberRewards"),
        };

        assert_eq!(entries.len(), 1, "one entry per requested amount");
        assert_eq!(entries[0].stake_amount, myopic);

        // Both pools have registered params — both should appear.
        assert_eq!(
            entries[0].pool_rewards.len(),
            2,
            "rewards for both registered pools"
        );

        // Verify the formula manually for Pool A.
        //   total_stake      = 1T - 900B = 100B
        //   reward_pot       = floor(3/1000 * 900B) * (1 - 2/10) = 2_700_000_000 * 0.8 = 2_160_000_000
        //   nOpt = 10  => z0 = 1/10
        //   hyp_pool_stake   = 50B + 1B = 51B
        //   sigma_hyp        = 51B/100B = 0.51  > z0(0.1) => capped at 0.1
        //   p_raw            = 10B/100B = 0.10 == z0 => capped at 0.1
        //
        // maxPool'(sigma=0.1, p=0.1):
        //   f4 = (z0 - sigma') / z0  = (0.1-0.1)/0.1 = 0
        //   f3 = (sigma' - p'*f4)/z0 = (0.1 - 0)/0.1 = 1
        //   f2 = sigma' + p'*a0*f3   = 0.1 + 0.1*(3/10)*1 = 0.1 + 0.03 = 0.13
        //   f1 = R/(1+a0)            = 2_160_000_000 / 1.3 = 1_661_538_461.5... => floor = 1_661_538_461
        //   max_pool = floor(f1*f2)  = floor(1_661_538_461 * 0.13) = floor(216_000_000) = 216_000_000
        //
        // delegator_fraction = 1B / 51B
        // member_reward = floor((216M - 340M) * ...) but 216M < cost(340M) => 0
        //
        // Pool A cost (340M) > max_pool (216M) => member_reward = 0.
        let pool_a_entry = entries[0]
            .pool_rewards
            .iter()
            .find(|(id, _)| id == &vec![0xAAu8; 28])
            .expect("Pool A entry missing");
        assert_eq!(
            pool_a_entry.1, 0,
            "Pool A max_pool < cost, member reward must be 0"
        );

        // Pool B is smaller and should yield positive reward.
        //   hyp_pool_stake = 30B + 1B = 31B
        //   sigma_hyp = 31B/100B = 0.31 > z0(0.1) => capped at 0.1
        //   p_raw = 1B/100B = 0.01 < z0 => p = 0.01
        //
        // maxPool'(sigma=0.1, p=0.01):
        //   f4 = (0.1-0.1)/0.1 = 0
        //   f3 = (0.1 - 0.01*0)/0.1 = 1
        //   f2 = 0.1 + 0.01*(3/10)*1 = 0.1 + 0.003 = 0.103
        //   f1 = 2_160_000_000 / 1.3 = 1_661_538_461
        //   max_pool = floor(1_661_538_461 * 0.103) = floor(171_138_461) = 171_138_461
        //
        // 171M < cost(340M) => member_reward = 0 for Pool B too.
        // Both pools are over-saturated: their individual stakes alone already
        // hit z0 = 10%, so adding the hypothetical delegator doesn't change
        // sigma and the small reward_pot yields max_pool < cost.
        // This is expected behaviour: the delegator would earn nothing in either pool.
        let pool_b_entry = entries[0]
            .pool_rewards
            .iter()
            .find(|(id, _)| id == &vec![0xBBu8; 28])
            .expect("Pool B entry missing");
        assert_eq!(
            pool_b_entry.1, 0,
            "Pool B max_pool < cost, member reward must be 0"
        );
    }

    #[test]
    fn test_non_myopic_rewards_large_pot_yields_positive_reward() {
        // Set reward_pot large enough that max_pool > cost.
        //   reserves = 10  (tiny, so circulation = max_supply - 10 ≈ max_supply)
        //   max_lovelace_supply = 45T
        //   rho = 1/1 (100% expansion) => R_gross = reserves = 10
        // That won't work — let's use a simple configuration where
        // the arithmetic produces a positive member reward.
        //
        //   max_lovelace_supply = 1_000_000 (toy)
        //   reserves            =   500_000
        //   rho = 1/1  => R_gross = 500_000
        //   tau = 0/1  => treasury = 0, reward_pot = 500_000
        //   nOpt = 1   => z0 = 1  (no saturation cap)
        //   a0   = 0   => no pledge influence
        //
        //   Pool: stake = 200_000, pledge = 0, cost = 1_000, margin = 0%
        //   total_stake = 1M - 500K = 500_000
        //   myopic = 10_000
        //   sigma_hyp = (200_000 + 10_000) / 500_000 = 0.42 < z0(1) => sigma = 0.42
        //   p_raw = 0
        //   f4 = (1 - 0.42)/1 = 0.58
        //   f3 = (0.42 - 0)/1 = 0.42
        //   f2 = 0.42 + 0*0*0.42 = 0.42
        //   f1 = 500_000 / (1+0) = 500_000
        //   max_pool = floor(500_000 * 0.42) = 210_000
        //
        //   member_reward = floor((210_000 - 1_000) * (1-0) * 10_000 / 210_000)
        //                 = floor(209_000 * 10_000 / 210_000)
        //                 = floor(9_952.38...) = 9_952
        use crate::node::n2c_query::types::PoolParamsSnapshot;
        let state = NodeStateSnapshot {
            max_lovelace_supply: 1_000_000,
            reserves: 500_000,
            stake_pools: vec![StakePoolSnapshot {
                pool_id: vec![0xCCu8; 28],
                stake: 200_000,
                vrf_keyhash: vec![0u8; 32],
                total_active_stake: 200_000,
            }],
            pool_params_entries: vec![PoolParamsSnapshot {
                pool_id: vec![0xCCu8; 28],
                vrf_keyhash: vec![0u8; 32],
                pledge: 0,
                cost: 1_000,
                margin_num: 0,
                margin_den: 1,
                reward_account: vec![0u8; 29],
                owners: vec![],
                relays: vec![],
                metadata_url: None,
                metadata_hash: None,
            }],
            protocol_params: ProtocolParamsSnapshot {
                rho_num: 1,
                rho_den: 1,
                tau_num: 0,
                tau_den: 1,
                a0_num: 0,
                a0_den: 1,
                n_opt: 1,
                ..ProtocolParamsSnapshot::default()
            },
            ..NodeStateSnapshot::default()
        };

        let myopic = 10_000u64;
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(1).ok();
        enc.u64(myopic).ok();
        let mut dec = minicbor::Decoder::new(&buf);

        let result = handle_non_myopic_rewards(&state, &mut dec);
        let entries = match result {
            QueryResult::NonMyopicMemberRewards(e) => e,
            _ => panic!("Expected NonMyopicMemberRewards"),
        };

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].pool_rewards.len(), 1);
        let (_, reward) = &entries[0].pool_rewards[0];
        assert_eq!(*reward, 9_952, "member reward should be 9_952 lovelace");
    }

    #[test]
    fn test_non_myopic_rewards_empty_amounts_uses_default() {
        let state = make_nm_state();

        // Empty amounts array — should fall back to 1 trillion lovelace default.
        let mut buf = Vec::new();
        minicbor::Encoder::new(&mut buf).array(0).ok();
        let mut dec = minicbor::Decoder::new(&buf);

        let result = handle_non_myopic_rewards(&state, &mut dec);
        match result {
            QueryResult::NonMyopicMemberRewards(entries) => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].stake_amount, 1_000_000_000_000);
            }
            _ => panic!("Expected NonMyopicMemberRewards"),
        }
    }

    #[test]
    fn test_non_myopic_rewards_pool_without_params_excluded() {
        // A pool that has stake but no pool_params_entries entry should be
        // silently excluded from the result (we cannot compute its reward).
        use crate::node::n2c_query::types::PoolParamsSnapshot;
        let state = NodeStateSnapshot {
            max_lovelace_supply: 1_000_000,
            reserves: 500_000,
            stake_pools: vec![
                StakePoolSnapshot {
                    pool_id: vec![0xAAu8; 28],
                    stake: 100_000,
                    vrf_keyhash: vec![0u8; 32],
                    total_active_stake: 200_000,
                },
                // Pool B has no pool_params entry — should be excluded.
                StakePoolSnapshot {
                    pool_id: vec![0xBBu8; 28],
                    stake: 100_000,
                    vrf_keyhash: vec![0u8; 32],
                    total_active_stake: 200_000,
                },
            ],
            pool_params_entries: vec![PoolParamsSnapshot {
                pool_id: vec![0xAAu8; 28],
                vrf_keyhash: vec![0u8; 32],
                pledge: 0,
                cost: 1_000,
                margin_num: 0,
                margin_den: 1,
                reward_account: vec![0u8; 29],
                owners: vec![],
                relays: vec![],
                metadata_url: None,
                metadata_hash: None,
            }],
            protocol_params: ProtocolParamsSnapshot {
                rho_num: 1,
                rho_den: 1,
                tau_num: 0,
                tau_den: 1,
                a0_num: 0,
                a0_den: 1,
                n_opt: 1,
                ..ProtocolParamsSnapshot::default()
            },
            ..NodeStateSnapshot::default()
        };

        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(1).ok();
        enc.u64(10_000u64).ok();
        let mut dec = minicbor::Decoder::new(&buf);

        let result = handle_non_myopic_rewards(&state, &mut dec);
        let entries = match result {
            QueryResult::NonMyopicMemberRewards(e) => e,
            _ => panic!("Expected NonMyopicMemberRewards"),
        };
        assert_eq!(
            entries[0].pool_rewards.len(),
            1,
            "only Pool A (with registered params) should appear"
        );
        assert_eq!(
            entries[0].pool_rewards[0].0,
            vec![0xAAu8; 28],
            "result should be for Pool A"
        );
    }

    #[test]
    fn test_non_myopic_rewards_zero_reserves_returns_empty() {
        let mut state = make_nm_state();
        state.reserves = 0; // No reserves => reward_pot = 0

        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(1).ok();
        enc.u64(1_000_000u64).ok();
        let mut dec = minicbor::Decoder::new(&buf);

        let result = handle_non_myopic_rewards(&state, &mut dec);
        match result {
            QueryResult::NonMyopicMemberRewards(entries) => {
                assert_eq!(entries.len(), 1);
                assert!(
                    entries[0].pool_rewards.is_empty(),
                    "zero reserves => no rewards"
                );
            }
            _ => panic!("Expected NonMyopicMemberRewards"),
        }
    }
}
