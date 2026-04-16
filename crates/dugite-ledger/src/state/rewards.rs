use super::{LedgerState, PendingRewardUpdate, StakeSnapshot, MAX_LOVELACE_SUPPLY};
use dugite_primitives::hash::{Hash28, Hash32};
use dugite_primitives::protocol_params::ProtocolParameters;
use dugite_primitives::value::Lovelace;
use num_bigint::BigInt;
use num_traits::{Signed, Zero};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, warn};

/// Arbitrary-precision rational number matching Haskell's `Rational`.
///
/// Uses `num_bigint::BigInt` for exact arithmetic with no overflow risk.
/// All intermediate reward calculations produce exact results; `floor_u64()`
/// applies the single floor operation at the end, matching Haskell's
/// `rationalToCoinViaFloor`.
///
/// Previous implementation used i128 with BigInt fallback, but the fallback
/// saturated to i128::MAX when results didn't fit, silently producing wrong
/// answers for mainnet-scale values (~36T circulation denominator).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rat {
    pub n: BigInt,
    pub d: BigInt,
}

impl Rat {
    pub fn new(n: impl Into<BigInt>, d: impl Into<BigInt>) -> Self {
        let d = d.into();
        let n = n.into();
        if d.is_zero() {
            return Rat {
                n: BigInt::from(0),
                d: BigInt::from(1),
            };
        }
        let g = Self::bigint_gcd(&n, &d);
        let (n, d) = (&n / &g, &d / &g);
        // Normalize sign: denominator always positive
        if d < BigInt::from(0) {
            Rat { n: -n, d: -d }
        } else {
            Rat { n, d }
        }
    }

    fn bigint_gcd(a: &BigInt, b: &BigInt) -> BigInt {
        let (mut a, mut b) = (a.abs(), b.abs());
        while !b.is_zero() {
            let t = b.clone();
            b = &a % &t;
            a = t;
        }
        if a.is_zero() {
            BigInt::from(1)
        } else {
            a
        }
    }

    pub fn add(&self, other: &Rat) -> Rat {
        let n = &self.n * &other.d + &other.n * &self.d;
        let d = &self.d * &other.d;
        Rat::new(n, d)
    }

    pub fn sub(&self, other: &Rat) -> Rat {
        let n = &self.n * &other.d - &other.n * &self.d;
        let d = &self.d * &other.d;
        Rat::new(n, d)
    }

    pub fn mul(&self, other: &Rat) -> Rat {
        Rat::new(&self.n * &other.n, &self.d * &other.d)
    }

    pub fn div(&self, other: &Rat) -> Rat {
        if other.n.is_zero() {
            return Rat::new(0i128, 1i128);
        }
        Rat::new(&self.n * &other.d, &self.d * &other.n)
    }

    pub fn min_rat(&self, other: &Rat) -> Rat {
        // a/b <= c/d iff a*d <= c*b (when b, d > 0)
        if &self.n * &other.d <= &other.n * &self.d {
            self.clone()
        } else {
            other.clone()
        }
    }

    pub fn floor_u64(&self) -> u64 {
        if self.d.is_zero() || self.n <= BigInt::from(0) {
            return 0;
        }
        let result = &self.n / &self.d;
        // The result of floor(reward) must always fit in u64
        u64::try_from(result).unwrap_or_else(|_| {
            warn!("Rat::floor_u64 overflow — value exceeds u64::MAX, clamping");
            u64::MAX
        })
    }

    /// Helper: create from i128 values (convenience for the common case)
    pub fn from_i128(n: i128, d: i128) -> Self {
        Rat::new(BigInt::from(n), BigInt::from(d))
    }
}

/// Compute a reward update from explicit parameters, without requiring a `LedgerState`.
///
/// This is the standalone version of the reward calculation that was previously
/// only accessible via `LedgerState::calculate_rewards_inner`. It implements the
/// full Haskell `startStep` / RUPD formula:
///   - Monetary expansion with eta performance adjustment
///   - Treasury tau cut
///   - Per-pool maxPool' with pledge influence (a0)
///   - Apparent performance (mkApparentPerformance)
///   - Operator/member reward split
///
/// # Parameters
///
/// * `params` — Previous epoch's protocol parameters (Haskell's `prevPParams`)
/// * `prev_d` — Decentralisation parameter from the previous epoch
/// * `prev_protocol_version_major` — Major protocol version from the previous epoch (for pre-Babbage prefilter)
/// * `go_snapshot` — GO stake snapshot (delegations, pool params, stake distribution). `None` yields empty rewards.
/// * `bprev_blocks_by_pool` — Per-pool block production counts from the previous epoch (Haskell's `nesBprev`)
/// * `ss_fee` — Fee pot from SNAP at previous boundary (Haskell's `ssFee`)
/// * `reserves` — Current reserves
/// * `_treasury` — Current treasury (reserved for future use)
/// * `reward_accounts` — Registered reward accounts (for pre-Babbage prefilter check)
/// * `epoch_length` — Shelley epoch length in slots
/// * `_shelley_transition_epoch` — Number of Byron epochs before Shelley (reserved for future use)
#[allow(clippy::too_many_arguments)]
pub fn compute_reward_update(
    params: &ProtocolParameters,
    prev_d: f64,
    prev_protocol_version_major: u64,
    go_snapshot: Option<&StakeSnapshot>,
    bprev_blocks_by_pool: &HashMap<Hash28, u64>,
    ss_fee: Lovelace,
    reserves: Lovelace,
    _treasury: Lovelace,
    reward_accounts: &HashMap<Hash32, Lovelace>,
    epoch_length: u64,
    _shelley_transition_epoch: u64,
) -> PendingRewardUpdate {
    let go = match go_snapshot {
        Some(s) => s,
        None => return PendingRewardUpdate::default(),
    };

    let pp = params;
    let rho_num = pp.rho.numerator as i128;
    let rho_den = pp.rho.denominator.max(1) as i128;
    let tau_num = pp.tau.numerator as i128;
    let tau_den = pp.tau.denominator.max(1) as i128;

    let d = prev_d;
    let actual_blocks: u64 = bprev_blocks_by_pool.values().sum();
    let epoch_fees = ss_fee.0;

    let rho = Rat::from_i128(rho_num, rho_den);

    let expansion = if d >= 0.8 {
        rho.mul(&Rat::from_i128(reserves.0 as i128, 1)).floor_u64()
    } else {
        let (f_num, f_den) = pp.active_slot_coeff_rational();

        let d_den = 1_000_000_000u128;
        let d_num = (d * d_den as f64).round() as u128;
        let one_minus_d_num = d_den.saturating_sub(d_num);

        let numerator = one_minus_d_num * f_num as u128 * epoch_length as u128;
        let denominator = d_den * f_den as u128;

        let raw_expected_blocks = (numerator / denominator) as u64;
        if raw_expected_blocks == 0 {
            warn!(
                "expected_blocks rounded to 0 (d={d}, f_num={f_num}, f_den={f_den}, \
                 epoch_length={epoch_length}), clamping to 1",
            );
        }
        let expected_blocks = raw_expected_blocks.max(1);

        let effective_blocks = actual_blocks.min(expected_blocks);
        rho.mul(&Rat::from_i128(reserves.0 as i128, 1))
            .mul(&Rat::from_i128(
                effective_blocks as i128,
                expected_blocks as i128,
            ))
            .floor_u64()
    };

    let total_rewards_available = expansion + epoch_fees;

    if total_rewards_available == 0 {
        return PendingRewardUpdate::default();
    }

    let tau = Rat::from_i128(tau_num, tau_den);
    let treasury_cut = tau
        .mul(&Rat::from_i128(total_rewards_available as i128, 1))
        .floor_u64();

    let reward_pot = total_rewards_available - treasury_cut;

    let total_stake = MAX_LOVELACE_SUPPLY.saturating_sub(reserves.0);
    if total_stake == 0 {
        let net = treasury_cut.saturating_sub(epoch_fees);
        return PendingRewardUpdate {
            delta_reserves: net,
            delta_treasury: treasury_cut,
            rewards: HashMap::new(),
        };
    }

    let total_active_stake: u64 = go
        .pool_stake
        .iter()
        .filter(|(pool_id, _)| go.pool_params.contains_key(pool_id))
        .fold(0u64, |acc, (_, s)| acc.saturating_add(s.0));
    if total_active_stake == 0 {
        debug!(
            "No active stake: GO pools={}, GO pool_stake entries={}",
            go.pool_params.len(),
            go.pool_stake.len()
        );
        let net = treasury_cut.saturating_sub(epoch_fees);
        return PendingRewardUpdate {
            delta_reserves: net,
            delta_treasury: treasury_cut,
            rewards: HashMap::new(),
        };
    }

    let total_blocks_in_epoch: u64 = bprev_blocks_by_pool.values().sum::<u64>().max(1);

    let n_opt = pp.n_opt.max(1);

    let mut total_distributed: u64 = 0;
    let mut reward_map: HashMap<Hash32, Lovelace> = HashMap::new();

    let mut delegators_by_pool: HashMap<Hash28, Vec<Hash32>> = HashMap::new();
    for (cred_hash, pool_id) in go.delegations.iter() {
        delegators_by_pool
            .entry(*pool_id)
            .or_default()
            .push(*cred_hash);
    }

    let mut owner_stake_by_pool: HashMap<Hash28, u64> = HashMap::new();
    for (pool_id, pool_reg) in go.pool_params.iter() {
        let mut owner_stake = 0u64;
        for owner in &pool_reg.owners {
            let owner_key = owner.to_hash32_padded();
            if go.delegations.get(&owner_key) == Some(pool_id) {
                owner_stake += go
                    .stake_distribution
                    .get(&owner_key)
                    .map(|l| l.0)
                    .unwrap_or(0);
            }
        }
        owner_stake_by_pool.insert(*pool_id, owner_stake);
    }

    for (pool_id, pool_active_stake) in &go.pool_stake {
        if bprev_blocks_by_pool.get(pool_id).copied().unwrap_or(0) == 0 {
            continue;
        }

        let pool_reg = match go.pool_params.get(pool_id) {
            Some(reg) => reg,
            None => continue,
        };

        {
            let prefilter_active = prev_protocol_version_major <= 6;
            if prefilter_active {
                let op_key = LedgerState::reward_account_to_hash(&pool_reg.reward_account);
                if !reward_accounts.contains_key(&op_key) {
                    debug!(
                        pool = ?pool_id.as_bytes()[..4],
                        "Pool excluded: pre-Babbage prefilter (proto <= 6, unregistered reward account)"
                    );
                    continue;
                }
            }
        }

        let self_delegated = owner_stake_by_pool.get(pool_id).copied().unwrap_or(0);
        if self_delegated < pool_reg.pledge.0 {
            debug!(
                "Pool {} pledge not met: {} < {}",
                pool_id.to_hex(),
                self_delegated,
                pool_reg.pledge.0
            );
            continue;
        }

        let a0_r = Rat::from_i128(pp.a0.numerator as i128, pp.a0.denominator.max(1) as i128);
        let z0 = Rat::from_i128(1, n_opt as i128);
        let sigma_raw = Rat::from_i128(pool_active_stake.0 as i128, total_stake as i128);
        let p_raw = Rat::from_i128(pool_reg.pledge.0 as i128, total_stake as i128);
        let sigma = sigma_raw.min_rat(&z0);
        let p = p_raw.min_rat(&z0);

        let f4 = z0.sub(&sigma).div(&z0);
        let f3 = sigma.sub(&p.mul(&f4)).div(&z0);
        let f2 = sigma.add(&p.mul(&a0_r).mul(&f3));
        let f1 = Rat::from_i128(reward_pot as i128, 1).div(&Rat::from_i128(1, 1).add(&a0_r));
        let max_pool = f1.mul(&f2).floor_u64();

        let blocks_made = bprev_blocks_by_pool.get(pool_id).copied().unwrap_or(0);
        debug!(
            pool = ?pool_id.as_bytes()[..4],
            blocks_made,
            max_pool,
            pool_stake = pool_active_stake.0,
            total_stake,
            total_active_stake,
            total_blocks = total_blocks_in_epoch,
            reward_pot,
            self_delegated,
            pledge = pool_reg.pledge.0,
            n_opt,
            d,
            "Per-pool reward input"
        );

        let pool_reward = if pool_active_stake.0 == 0 {
            0u64
        } else if d >= 0.8 {
            max_pool
        } else if blocks_made == 0 {
            0u64
        } else {
            let perf = Rat::from_i128(blocks_made as i128, total_blocks_in_epoch as i128).mul(
                &Rat::from_i128(total_active_stake as i128, pool_active_stake.0 as i128),
            );
            perf.mul(&Rat::from_i128(max_pool as i128, 1)).floor_u64()
        };

        if pool_reward == 0 {
            continue;
        }

        let cost = pool_reg.cost.0;
        let margin_num = pool_reg.margin_numerator as i128;
        let margin_den = pool_reg.margin_denominator.max(1) as i128;

        let operator_reward = if pool_reward <= cost {
            pool_reward
        } else {
            let remainder = pool_reward - cost;
            let margin = Rat::from_i128(margin_num, margin_den);
            let one_minus_margin = Rat::from_i128(margin_den - margin_num, margin_den);
            let s_over_sigma = Rat::from_i128(self_delegated as i128, pool_active_stake.0 as i128);
            let share = margin.add(&one_minus_margin.mul(&s_over_sigma));
            let op_extra = share.mul(&Rat::from_i128(remainder as i128, 1)).floor_u64();
            cost + op_extra
        };

        let owner_set: std::collections::HashSet<Hash32> = pool_reg
            .owners
            .iter()
            .map(|o| o.to_hash32_padded())
            .collect();

        if let Some(delegators) = delegators_by_pool.get(pool_id) {
            for cred_hash in delegators {
                if owner_set.contains(cred_hash) {
                    continue;
                }

                let member_stake = go
                    .stake_distribution
                    .get(cred_hash)
                    .copied()
                    .unwrap_or(Lovelace(0))
                    .0;

                if member_stake == 0 || pool_active_stake.0 == 0 {
                    continue;
                }

                let member_share = if pool_reward <= cost {
                    0u64
                } else {
                    let remainder = pool_reward - cost;
                    let one_minus_margin = Rat::from_i128(margin_den - margin_num, margin_den);
                    let member_frac =
                        Rat::from_i128(member_stake as i128, pool_active_stake.0 as i128);
                    Rat::from_i128(remainder as i128, 1)
                        .mul(&one_minus_margin)
                        .mul(&member_frac)
                        .floor_u64()
                };

                if member_share > 0 {
                    *reward_map.entry(*cred_hash).or_insert(Lovelace(0)) += Lovelace(member_share);
                    total_distributed += member_share;
                }
            }
        }

        if operator_reward > 0 {
            let op_key = LedgerState::reward_account_to_hash(&pool_reg.reward_account);
            *reward_map.entry(op_key).or_insert(Lovelace(0)) += Lovelace(operator_reward);
            total_distributed += operator_reward;
        }
    }

    let undistributed = reward_pot.saturating_sub(total_distributed);

    debug!(
        "Rewards calculated: {} lovelace to {} accounts, {} to treasury (expansion: {}, fees: {})",
        total_distributed,
        reward_map.len(),
        treasury_cut + undistributed,
        expansion,
        epoch_fees
    );

    let gross = treasury_cut + total_distributed;
    let net_reserve_decrease = gross.saturating_sub(epoch_fees);
    if epoch_fees > 0 {
        debug!("Fee offset: gross={gross}, epoch_fees={epoch_fees}, net={net_reserve_decrease}");
    }
    PendingRewardUpdate {
        rewards: reward_map,
        delta_treasury: treasury_cut,
        delta_reserves: net_reserve_decrease,
    }
}

impl LedgerState {
    /// Apply a pending reward update to the ledger state.
    ///
    /// This is called at the BEGINNING of an epoch transition to apply rewards
    /// computed during the previous epoch transition, matching Haskell's RUPD
    /// deferred application pattern.
    pub(crate) fn apply_pending_reward_update(&mut self) {
        if let Some(rupd) = self.epochs.pending_reward_update.take() {
            // Apply reserves decrease (monetary expansion)
            self.epochs.reserves.0 = self.epochs.reserves.0.saturating_sub(rupd.delta_reserves);

            // Apply treasury increase (tau cut + undistributed)
            self.epochs.treasury.0 = self.epochs.treasury.0.saturating_add(rupd.delta_treasury);

            // Apply per-account rewards (matching Haskell's applyRUpdFiltered):
            // registered credentials → reward account; unregistered → treasury.
            let mut total_applied = 0u64;
            let mut unregistered_total = 0u64;
            for (cred_hash, reward) in &rupd.rewards {
                if reward.0 > 0 {
                    if self.certs.reward_accounts.contains_key(cred_hash) {
                        *Arc::make_mut(&mut self.certs.reward_accounts)
                            .entry(*cred_hash)
                            .or_insert(Lovelace(0)) += *reward;
                        total_applied += reward.0;
                    } else {
                        self.epochs.treasury.0 = self.epochs.treasury.0.saturating_add(reward.0);
                        unregistered_total += reward.0;
                    }
                }
            }

            debug!(
                "Applied pending reward update: {} lovelace to {} accounts \
                 ({} unregistered→treasury), treasury +{}, reserves -{}",
                total_applied,
                rupd.rewards.len(),
                unregistered_total,
                rupd.delta_treasury,
                rupd.delta_reserves,
            );
        }
    }

    /// Calculate rewards using the GO snapshot and a separate fee value.
    ///
    /// Legacy entry point that uses GO snapshot for both stake AND block data.
    #[allow(dead_code)]
    pub(crate) fn calculate_rewards_with_fee(
        &self,
        go_snapshot: &StakeSnapshot,
        ss_fee: Lovelace,
    ) -> PendingRewardUpdate {
        self.calculate_rewards_inner(go_snapshot, go_snapshot, ss_fee.0)
    }

    /// Calculate rewards matching Haskell's `startStep` exactly.
    ///
    /// Uses THREE separate data sources:
    /// - `go_snapshot`: ssStakeGo — stake distribution, delegations, pool params (2 epochs ago)
    /// - `bprev_snapshot`: nesBprev equivalent — block production counts (1 epoch ago, from SET)
    /// - `ss_fee`: ssFee — fee pot from SNAP at previous boundary
    pub(crate) fn calculate_rewards_full(
        &self,
        go_snapshot: &StakeSnapshot,
        bprev_snapshot: &StakeSnapshot,
        ss_fee: Lovelace,
    ) -> PendingRewardUpdate {
        self.calculate_rewards_inner(go_snapshot, bprev_snapshot, ss_fee.0)
    }

    /// Calculate rewards and return a PendingRewardUpdate for deferred application.
    ///
    /// Implements the formula from cardano-ledger-shelley:
    ///   - maxPool'(a0, nOpt, R, sigma, p) for pledge-influenced pool rewards
    ///   - mkApparentPerformance for beta/sigma performance calculation
    ///   - Pledge verification (pool gets zero if owner stake < declared pledge)
    ///   - Operator reward includes self-delegation share (margin + proportional)
    ///   - Operator reward goes to pool's registered reward account
    ///
    /// Legacy entry point that reads fees from the snapshot itself. New code
    /// should use `calculate_rewards_full` which separates GO/bprev/fees.
    #[cfg(test)]
    pub(crate) fn calculate_rewards(&self, rupd_snapshot: &StakeSnapshot) -> PendingRewardUpdate {
        self.calculate_rewards_inner(rupd_snapshot, rupd_snapshot, rupd_snapshot.epoch_fees.0)
    }

    /// Inner reward calculation — thin wrapper around [`compute_reward_update`].
    ///
    /// `stake_snapshot`: provides stake distribution, delegations, pool params (GO)
    /// `block_snapshot`: provides epoch_block_count, epoch_blocks_by_pool (nesBprev/SET)
    /// `epoch_fees`: ssFee from SNAP
    fn calculate_rewards_inner(
        &self,
        stake_snapshot: &StakeSnapshot,
        block_snapshot: &StakeSnapshot,
        epoch_fees: u64,
    ) -> PendingRewardUpdate {
        compute_reward_update(
            &self.epochs.prev_protocol_params,
            self.epochs.prev_d,
            self.epochs.prev_protocol_version_major,
            Some(stake_snapshot),
            &block_snapshot.epoch_blocks_by_pool,
            Lovelace(epoch_fees),
            self.epochs.reserves,
            self.epochs.treasury,
            &self.certs.reward_accounts,
            self.epoch_length,
            self.shelley_transition_epoch,
        )
    }

    /// Legacy compatibility: calculate and immediately distribute rewards.
    ///
    /// Used by tests that expect immediate reward application. New code should
    /// use `calculate_rewards()` + apply at the epoch boundary for correct
    /// Haskell-compatible RUPD timing.
    #[cfg(test)]
    pub(crate) fn calculate_and_distribute_rewards(&mut self, rupd_snapshot: StakeSnapshot) {
        // Use self.utxo.epoch_fees (matching the live path which uses ss_fee from SNAP).
        // Tests set state.utxo.epoch_fees before calling this function.
        let rupd =
            self.calculate_rewards_inner(&rupd_snapshot, &rupd_snapshot, self.utxo.epoch_fees.0);
        // Apply immediately (legacy behavior for test compatibility)
        self.epochs.reserves.0 = self.epochs.reserves.0.saturating_sub(rupd.delta_reserves);
        self.epochs.treasury.0 = self.epochs.treasury.0.saturating_add(rupd.delta_treasury);
        for (cred_hash, reward) in &rupd.rewards {
            if reward.0 > 0 {
                *Arc::make_mut(&mut self.certs.reward_accounts)
                    .entry(*cred_hash)
                    .or_insert(Lovelace(0)) += *reward;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Rat;

    // -----------------------------------------------------------------------
    // GCD correctness
    // -----------------------------------------------------------------------

    #[test]
    fn test_gcd_coprime_numbers() {
        // 13 and 17 are coprime
        let r = Rat::from_i128(13, 17);
        assert_eq!(r.n, 13.into());
        assert_eq!(r.d, 17.into());
    }

    #[test]
    fn test_gcd_reduces_fractions() {
        let r = Rat::from_i128(6, 9);
        assert_eq!(r.n, 2.into());
        assert_eq!(r.d, 3.into());
    }

    #[test]
    fn test_gcd_large_values() {
        // GCD(2^60, 2^40) = 2^40
        let a = 1i128 << 60;
        let b = 1i128 << 40;
        let r = Rat::from_i128(a, b);
        assert_eq!(r.n, (1i128 << 20).into());
        assert_eq!(r.d, 1.into());
    }

    // -----------------------------------------------------------------------
    // Rat multiplication with large values
    // -----------------------------------------------------------------------

    #[test]
    fn test_rat_mul_near_i128_max() {
        // Two large values — BigInt handles this correctly
        let a = Rat::from_i128(i128::MAX / 2, 1);
        let b = Rat::from_i128(3, 1);
        let result = a.mul(&b);
        assert!(result.d > 0.into());
        assert!(result.n > 0.into());
        // Should be exactly (MAX/2)*3, no saturation
        let expected = num_bigint::BigInt::from(i128::MAX / 2) * num_bigint::BigInt::from(3);
        assert_eq!(result.n, expected);
    }

    #[test]
    fn test_rat_mul_cross_reduce_prevents_overflow() {
        let a = Rat::from_i128(1_000_000_000_000_000, 7);
        let b = Rat::from_i128(7, 1_000_000_000_000_000);
        let result = a.mul(&b);
        assert_eq!(result.n, 1.into());
        assert_eq!(result.d, 1.into());
    }

    // -----------------------------------------------------------------------
    // Rat addition with large values
    // -----------------------------------------------------------------------

    #[test]
    fn test_rat_add_near_i128_max() {
        let a = Rat::from_i128(i128::MAX / 2, 1);
        let b = Rat::from_i128(i128::MAX / 2, 1);
        let result = a.add(&b);
        assert!(result.n > 0.into());
        assert!(result.d > 0.into());
        // Should be exact, no saturation
        let expected = num_bigint::BigInt::from(i128::MAX / 2) * 2;
        assert_eq!(result.n, expected);
    }

    #[test]
    fn test_rat_add_different_denominators() {
        let a = Rat::from_i128(1, 3);
        let b = Rat::from_i128(1, 6);
        let result = a.add(&b);
        assert_eq!(result.n, 1.into());
        assert_eq!(result.d, 2.into());
    }

    // -----------------------------------------------------------------------
    // Division producing very small fractions
    // -----------------------------------------------------------------------

    #[test]
    fn test_rat_div_very_small_fraction() {
        let a = Rat::from_i128(1, 1_000_000_000);
        let b = Rat::from_i128(1_000_000_000, 1);
        let result = a.div(&b);
        assert_eq!(result.n, 1.into());
        assert_eq!(result.d, 1_000_000_000_000_000_000i128.into());
    }

    #[test]
    fn test_rat_div_by_zero_returns_zero() {
        let a = Rat::from_i128(5, 3);
        let b = Rat::from_i128(0, 1);
        let result = a.div(&b);
        assert_eq!(result.n, 0.into());
    }

    // -----------------------------------------------------------------------
    // Negative Rat values
    // -----------------------------------------------------------------------

    #[test]
    fn test_rat_negative_numerator() {
        let r = Rat::from_i128(-3, 4);
        assert_eq!(r.n, (-3).into());
        assert_eq!(r.d, 4.into());
    }

    #[test]
    fn test_rat_negative_denominator_normalized() {
        let r = Rat::from_i128(3, -4);
        assert_eq!(r.n, (-3).into());
        assert_eq!(r.d, 4.into());
    }

    #[test]
    fn test_rat_both_negative() {
        let r = Rat::from_i128(-6, -8);
        assert_eq!(r.n, 3.into());
        assert_eq!(r.d, 4.into());
    }

    #[test]
    fn test_rat_sub_produces_negative() {
        let a = Rat::from_i128(1, 4);
        let b = Rat::from_i128(3, 4);
        let result = a.sub(&b);
        assert_eq!(result.n, (-1).into());
        assert_eq!(result.d, 2.into());
    }

    // -----------------------------------------------------------------------
    // Floor
    // -----------------------------------------------------------------------

    #[test]
    fn test_rat_floor_u64_negative_returns_zero() {
        let r = Rat::from_i128(-5, 3);
        assert_eq!(r.floor_u64(), 0);
    }

    #[test]
    fn test_rat_floor_u64_exact_division() {
        let r = Rat::from_i128(10, 5);
        assert_eq!(r.floor_u64(), 2);
    }

    #[test]
    fn test_rat_floor_u64_truncates() {
        let r = Rat::from_i128(7, 3);
        assert_eq!(r.floor_u64(), 2); // 7/3 = 2.333...
    }

    // -----------------------------------------------------------------------
    // min_rat
    // -----------------------------------------------------------------------

    #[test]
    fn test_rat_min_rat() {
        let a = Rat::from_i128(1, 3);
        let b = Rat::from_i128(1, 2);
        assert_eq!(a.min_rat(&b), a);
        assert_eq!(b.min_rat(&a), a);
    }

    #[test]
    fn test_rat_min_rat_equal() {
        let a = Rat::from_i128(2, 4);
        let b = Rat::from_i128(1, 2);
        let result = a.min_rat(&b);
        assert_eq!(result.n, 1.into());
        assert_eq!(result.d, 2.into());
    }

    // -----------------------------------------------------------------------
    // Zero denominator
    // -----------------------------------------------------------------------

    #[test]
    fn test_rat_zero_denominator() {
        let r = Rat::from_i128(5, 0);
        assert_eq!(r.n, 0.into());
        assert_eq!(r.d, 1.into());
    }

    // -----------------------------------------------------------------------
    // Mainnet-scale precision tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_rat_mainnet_scale_sigma_chain() {
        // Reproduce the exact computation chain from maxPool' with
        // mainnet-scale values (36T circulation denominator).
        // This MUST NOT overflow or saturate.
        let pool_stake: i128 = 4_733_011_000_060;
        let circulation: i128 = 36_706_064_193_192_852;
        let pledge: i128 = 100_000_000_000;
        let n_opt: i128 = 500;
        let reward_pot: i128 = 1_000_000_000_000; // 1T

        let a0 = Rat::from_i128(3, 10);
        let z0 = Rat::from_i128(1, n_opt);
        let sigma = Rat::from_i128(pool_stake, circulation).min_rat(&z0);
        let p = Rat::from_i128(pledge, circulation).min_rat(&z0);

        let f4 = z0.sub(&sigma).div(&z0);
        let f3 = sigma.sub(&p.mul(&f4)).div(&z0);
        let f2 = sigma.add(&p.mul(&a0).mul(&f3));
        let f1 = Rat::from_i128(reward_pot, 1).div(&Rat::from_i128(1, 1).add(&a0));
        let max_pool = f1.mul(&f2).floor_u64();

        // sigma ~ 0.000129 < z0 = 0.002, so sigma is NOT capped
        // maxPool should be approximately R/1.3 * sigma ~ 1T/1.3 * 0.000129 ~ 99M
        assert!(
            max_pool > 90_000_000 && max_pool < 110_000_000,
            "maxPool at mainnet scale should be ~99M for R=1T, got {max_pool}"
        );

        // Verify it's not the buggy saturated value (769B)
        assert!(
            max_pool < 1_000_000_000,
            "maxPool must not be the saturated value"
        );
    }

    // -----------------------------------------------------------------------
    // maxPool' formula unit tests
    // -----------------------------------------------------------------------

    fn max_pool_prime(
        a0_num: i128,
        a0_den: i128,
        n_opt: u64,
        reward_pot: u64,
        pool_stake: u64,
        pledge: u64,
        total_stake: u64,
    ) -> u64 {
        let a0 = Rat::from_i128(a0_num, a0_den);
        let z0 = Rat::from_i128(1, n_opt as i128);
        let sigma_raw = Rat::from_i128(pool_stake as i128, total_stake as i128);
        let p_raw = Rat::from_i128(pledge as i128, total_stake as i128);
        let sigma = sigma_raw.min_rat(&z0);
        let p = p_raw.min_rat(&z0);

        let f4 = z0.sub(&sigma).div(&z0);
        let f3 = sigma.sub(&p.mul(&f4)).div(&z0);
        let f2 = sigma.add(&p.mul(&a0).mul(&f3));
        let f1 = Rat::from_i128(reward_pot as i128, 1).div(&Rat::from_i128(1, 1).add(&a0));
        f1.mul(&f2).floor_u64()
    }

    #[test]
    fn test_max_pool_saturated_pool() {
        let result = max_pool_prime(3, 10, 500, 10_000_000_000, 10_000, 0, 1_000_000);
        assert_eq!(result, 15_384_615);
    }

    #[test]
    fn test_max_pool_unsaturated_zero_pledge() {
        let result = max_pool_prime(3, 10, 500, 10_000_000_000, 1_000, 0, 1_000_000);
        assert_eq!(result, 7_692_307);
    }

    #[test]
    fn test_max_pool_pledge_influence() {
        let no_pledge = max_pool_prime(3, 10, 500, 10_000_000_000, 1_000, 0, 1_000_000);
        let with_pledge = max_pool_prime(3, 10, 500, 10_000_000_000, 1_000, 500, 1_000_000);
        assert!(
            with_pledge > no_pledge,
            "Pledge should increase maxPool reward"
        );
    }

    #[test]
    fn test_max_pool_a0_zero_no_pledge_influence() {
        let no_pledge = max_pool_prime(0, 1, 500, 10_000_000_000, 1_000, 0, 1_000_000);
        let with_pledge = max_pool_prime(0, 1, 500, 10_000_000_000, 1_000, 500, 1_000_000);
        assert_eq!(no_pledge, with_pledge);
    }

    // -----------------------------------------------------------------------
    // Cross-validation against real Koios on-chain data (preview testnet)
    // -----------------------------------------------------------------------

    #[test]
    fn test_koios_pool_fee_split() {
        let total_pool_reward: u64 = 578_845_970 + 2_149_613_734;
        assert_eq!(total_pool_reward, 2_728_459_704);

        let cost = 340_000_000u64;
        let margin = Rat::from_i128(1, 10);
        let remainder = total_pool_reward - cost;

        let expected_pool_fees = cost
            + margin
                .mul(&Rat::from_i128(remainder as i128, 1))
                .floor_u64();
        assert_eq!(expected_pool_fees, 578_845_970);

        let one_minus_margin = Rat::from_i128(9, 10);
        let expected_deleg_rewards = one_minus_margin
            .mul(&Rat::from_i128(remainder as i128, 1))
            .floor_u64();
        // Koios: 2,149,613,734. floor(9/10 * 2,388,459,704) = 2,149,613,733.
        // 1 lovelace gap: cardano-node computes member_rewards = total - leader_share
        // (subtraction) rather than independent floor, avoiding double-floor loss.
        assert!(
            (expected_deleg_rewards as i64 - 2_149_613_734i64).unsigned_abs() <= 1,
            "deleg_rewards off by >1: got {expected_deleg_rewards}"
        );
    }

    #[test]
    fn test_koios_max_pool_and_performance() {
        let pool_stake: u64 = 4_733_011_000_060;
        let pledge: u64 = 100_000_000_000;
        let total_active_stake: u64 = 1_177_946_537_741_239;
        let circulation: u64 = 45_000_000_000_000_000 - 8_293_935_806_807_148;
        let blocks_made: u64 = 24;
        let total_blocks: u64 = 2578;

        // Apparent performance uses sigmaA (total_active_stake)
        let perf = Rat::from_i128(blocks_made as i128, total_blocks as i128).mul(&Rat::from_i128(
            total_active_stake as i128,
            pool_stake as i128,
        ));

        let perf_approx = {
            let n: i128 = (&perf.n).try_into().unwrap_or(i128::MAX);
            let d: i128 = (&perf.d).try_into().unwrap_or(i128::MAX);
            n as f64 / d as f64
        };
        assert!(
            (perf_approx - 2.317).abs() < 0.01,
            "Performance should be ~2.317, got {perf_approx}"
        );

        // maxPool uses sigma = pool_stake / circulation (NOT total_active_stake)
        let max_pool_1t = max_pool_prime(
            3,
            10,
            500,
            1_000_000_000_000,
            pool_stake,
            pledge,
            circulation,
        );

        let pool_reward_per_1t = perf
            .mul(&Rat::from_i128(max_pool_1t as i128, 1))
            .floor_u64();

        let known_total_pool_reward: u64 = 2_728_459_704;
        let reward_pot = Rat::from_i128(known_total_pool_reward as i128, 1)
            .mul(&Rat::from_i128(
                1_000_000_000_000,
                pool_reward_per_1t as i128,
            ))
            .floor_u64();

        let max_pool = max_pool_prime(3, 10, 500, reward_pot, pool_stake, pledge, circulation);
        let computed_pool_reward = perf.mul(&Rat::from_i128(max_pool as i128, 1)).floor_u64();

        // Back-computation through multiple floor() operations loses precision.
        // The actual forward calculation (with exact R from epoch data) is exact.
        let diff = (computed_pool_reward as i64 - known_total_pool_reward as i64).unsigned_abs();
        assert!(
            diff <= 10,
            "maxPool' * perf should reproduce Koios pool reward within tolerance: \
             computed={computed_pool_reward}, expected={known_total_pool_reward}, diff={diff}"
        );
    }

    #[test]
    fn test_koios_operator_member_split() {
        let total_reward = 2_728_459_704u64;
        let cost = 340_000_000u64;
        let margin = Rat::from_i128(1, 10);
        let one_minus_margin = Rat::from_i128(9, 10);
        let remainder = total_reward - cost;

        let deleg_rewards = one_minus_margin
            .mul(&Rat::from_i128(remainder as i128, 1))
            .floor_u64();
        assert!(
            (deleg_rewards as i64 - 2_149_613_734i64).unsigned_abs() <= 1,
            "deleg_rewards off by >1: got {deleg_rewards}"
        );

        let pool_fees = cost
            + margin
                .mul(&Rat::from_i128(remainder as i128, 1))
                .floor_u64();
        assert_eq!(pool_fees, 578_845_970, "pool_fees mismatch");
    }

    #[test]
    fn test_compute_reward_update_free_fn() {
        // Call the free function directly with no GO snapshot — should return empty rewards.
        let params = dugite_primitives::protocol_params::ProtocolParameters::mainnet_defaults();
        let bprev_blocks_by_pool = std::collections::HashMap::new();
        let reward_accounts = std::collections::HashMap::new();

        let rupd = super::compute_reward_update(
            &params,
            0.0,  // prev_d
            8,    // prev_protocol_version_major (Conway)
            None, // no GO snapshot
            &bprev_blocks_by_pool,
            dugite_primitives::value::Lovelace(0), // ss_fee
            dugite_primitives::value::Lovelace(0), // reserves
            dugite_primitives::value::Lovelace(0), // treasury
            &reward_accounts,
            86400, // epoch_length
            0,     // shelley_transition_epoch
        );

        assert!(
            rupd.rewards.is_empty(),
            "No GO snapshot should yield empty rewards"
        );
        assert_eq!(rupd.delta_treasury, 0);
        assert_eq!(rupd.delta_reserves, 0);
    }

    #[test]
    fn test_sigma_uses_circulation_not_active_stake() {
        let pool_stake: u64 = 4_733_011_000_060;
        let total_active_stake: u64 = 1_177_946_537_741_239;
        let circulation: u64 = 36_709_439_229_911_673;

        // sigma (for maxPool') = pool_stake / circulation ~ 0.000129 < z0 = 0.002
        let sigma = Rat::from_i128(pool_stake as i128, circulation as i128);
        let sigma_f64 = {
            let n: i128 = (&sigma.n).try_into().unwrap_or(i128::MAX);
            let d: i128 = (&sigma.d).try_into().unwrap_or(i128::MAX);
            n as f64 / d as f64
        };
        assert!(
            sigma_f64 < 0.002,
            "sigma relative to circulation should be below z0"
        );

        // sigmaA (for performance only) = pool_stake / total_active_stake ~ 0.004
        let sigma_a = Rat::from_i128(pool_stake as i128, total_active_stake as i128);
        let sigma_a_f64 = {
            let n: i128 = (&sigma_a.n).try_into().unwrap_or(i128::MAX);
            let d: i128 = (&sigma_a.d).try_into().unwrap_or(i128::MAX);
            n as f64 / d as f64
        };
        assert!(
            sigma_a_f64 > 0.002,
            "sigmaA relative to active stake exceeds z0"
        );

        // maxPool with circulation denominator must produce correct (modest) result
        let max_pool = max_pool_prime(
            3,
            10,
            500,
            1_000_000_000_000,
            pool_stake,
            100_000_000_000,
            circulation,
        );
        assert!(
            max_pool < 200_000_000,
            "maxPool with circulation denominator should be ~99M, got {max_pool}"
        );
    }
}
