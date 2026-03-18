use super::{LedgerState, PendingRewardUpdate, StakeSnapshot, MAX_LOVELACE_SUPPLY};
use num_bigint::BigInt;
use num_traits::{Signed, Zero};
use std::collections::HashMap;
use std::sync::Arc;
use torsten_primitives::hash::{Hash28, Hash32};
use torsten_primitives::value::Lovelace;
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

impl LedgerState {
    /// Apply a pending reward update to the ledger state.
    ///
    /// This is called at the BEGINNING of an epoch transition to apply rewards
    /// computed during the previous epoch transition, matching Haskell's RUPD
    /// deferred application pattern.
    pub(crate) fn apply_pending_reward_update(&mut self) {
        if let Some(rupd) = self.pending_reward_update.take() {
            // Apply reserves decrease (monetary expansion)
            self.reserves.0 = self.reserves.0.saturating_sub(rupd.delta_reserves);

            // Apply treasury increase (tau cut + undistributed)
            self.treasury.0 = self.treasury.0.saturating_add(rupd.delta_treasury);

            // Apply per-account rewards
            let mut total_applied = 0u64;
            for (cred_hash, reward) in &rupd.rewards {
                if reward.0 > 0 {
                    *Arc::make_mut(&mut self.reward_accounts)
                        .entry(*cred_hash)
                        .or_insert(Lovelace(0)) += *reward;
                    total_applied += reward.0;
                }
            }

            debug!(
                "Applied pending reward update: {} lovelace to {} accounts, \
                 treasury +{}, reserves -{}",
                total_applied,
                rupd.rewards.len(),
                rupd.delta_treasury,
                rupd.delta_reserves,
            );
        }
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
    /// Returns a `PendingRewardUpdate` that should be stored and applied at the
    /// NEXT epoch boundary, matching Haskell's RUPD timing.
    pub(crate) fn calculate_rewards(&self, go_snapshot: &StakeSnapshot) -> PendingRewardUpdate {
        let rho_num = self.protocol_params.rho.numerator as i128;
        let rho_den = self.protocol_params.rho.denominator.max(1) as i128;
        let tau_num = self.protocol_params.tau.numerator as i128;
        let tau_den = self.protocol_params.tau.denominator.max(1) as i128;

        // Monetary expansion with eta performance adjustment:
        //   expected_blocks = floor(active_slot_coeff * epoch_length) (since d=0 in Conway)
        //   eta = min(1, actual_blocks / expected_blocks)
        //   deltaR1 = floor(eta * rho * reserves)
        let raw_expected_blocks =
            (self.protocol_params.active_slot_coeff() * self.epoch_length as f64).floor() as u64;
        if raw_expected_blocks == 0 {
            warn!(
                "expected_blocks rounded to 0 (active_slot_coeff={}, epoch_length={}), clamping to 1",
                self.protocol_params.active_slot_coeff(),
                self.epoch_length
            );
        }
        let expected_blocks = raw_expected_blocks.max(1);
        // Use the go snapshot's block count — Haskell's RUPD computes eta
        // from the go-epoch block production, not the live epoch counters.
        let actual_blocks = go_snapshot.epoch_block_count;
        // eta = min(1, actual/expected) — applied as rational: min(actual, expected) / expected
        let effective_blocks = actual_blocks.min(expected_blocks);
        // expansion = floor(rho * reserves * (effective/expected))
        let rho = Rat::from_i128(rho_num, rho_den);
        let expansion_rat =
            rho.mul(&Rat::from_i128(self.reserves.0 as i128, 1))
                .mul(&Rat::from_i128(
                    effective_blocks as i128,
                    expected_blocks as i128,
                ));
        let expansion = expansion_rat.floor_u64();
        let total_rewards_available = expansion + go_snapshot.epoch_fees.0;

        if total_rewards_available == 0 {
            return PendingRewardUpdate::default();
        }

        // Treasury cut: floor(tau * total_rewards)
        let tau = Rat::from_i128(tau_num, tau_den);
        let treasury_cut = tau
            .mul(&Rat::from_i128(total_rewards_available as i128, 1))
            .floor_u64();

        let reward_pot = total_rewards_available - treasury_cut;

        // Total stake for sigma denominator: circulation = maxSupply - reserves.
        // Per Haskell PulsingReward.hs: totalStake = circulation es maxSupply
        // where circulation = supply <-> casReserves (maxSupply - reserves).
        // This is distinct from total_active_stake (used only for sigmaA in
        // apparent performance).
        let total_stake = MAX_LOVELACE_SUPPLY.saturating_sub(self.reserves.0);
        if total_stake == 0 {
            return PendingRewardUpdate {
                delta_reserves: expansion,
                delta_treasury: treasury_cut + reward_pot,
                rewards: HashMap::new(),
            };
        }

        // Total active stake (for apparent performance denominator only)
        let total_active_stake: u64 = go_snapshot
            .pool_stake
            .values()
            .fold(0u64, |acc, s| acc.saturating_add(s.0));
        if total_active_stake == 0 {
            return PendingRewardUpdate {
                delta_reserves: expansion,
                delta_treasury: treasury_cut + reward_pot,
                rewards: HashMap::new(),
            };
        }

        // Total blocks produced in the go epoch (for apparent performance)
        let total_blocks_in_epoch = go_snapshot.epoch_block_count.max(1);

        // Saturation point: z0 = 1/nOpt
        let n_opt = self.protocol_params.n_opt.max(1);

        let mut total_distributed: u64 = 0;
        let mut reward_map: HashMap<Hash32, Lovelace> = HashMap::new();

        // Build delegators-by-pool index for O(n) reward distribution
        let mut delegators_by_pool: HashMap<Hash28, Vec<Hash32>> = HashMap::new();
        for (cred_hash, pool_id) in go_snapshot.delegations.iter() {
            delegators_by_pool
                .entry(*pool_id)
                .or_default()
                .push(*cred_hash);
        }

        // Build owner-delegated-stake per pool for pledge check
        let mut owner_stake_by_pool: HashMap<Hash28, u64> = HashMap::new();
        for (pool_id, pool_reg) in go_snapshot.pool_params.iter() {
            let mut owner_stake = 0u64;
            for owner in &pool_reg.owners {
                let owner_key = owner.to_hash32_padded();
                if go_snapshot.delegations.get(&owner_key) == Some(pool_id) {
                    owner_stake += go_snapshot
                        .stake_distribution
                        .get(&owner_key)
                        .map(|l| l.0)
                        .unwrap_or(0);
                }
            }
            owner_stake_by_pool.insert(*pool_id, owner_stake);
        }

        // Calculate rewards per pool
        for (pool_id, pool_active_stake) in &go_snapshot.pool_stake {
            let pool_reg = match go_snapshot.pool_params.get(pool_id) {
                Some(reg) => reg,
                None => continue,
            };

            // Pledge check: if owner-delegated stake < declared pledge, pool gets zero
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

            // maxPool'(a0, nOpt, R, sigma, p) using BigInt-backed Rat:
            //   z0 = 1/nOpt
            //   sigma' = min(sigma, z0), p' = min(p, z0)
            //   maxPool = floor(R/(1+a0) * (sigma' + p' * a0 * (sigma' - p'*(z0-sigma')/z0) / z0))
            let a0_r = Rat::from_i128(
                self.protocol_params.a0.numerator as i128,
                self.protocol_params.a0.denominator.max(1) as i128,
            );
            let z0 = Rat::from_i128(1, n_opt as i128);
            let sigma_raw = Rat::from_i128(pool_active_stake.0 as i128, total_stake as i128);
            let p_raw = Rat::from_i128(pool_reg.pledge.0 as i128, total_stake as i128);
            let sigma = sigma_raw.min_rat(&z0);
            let p = p_raw.min_rat(&z0);

            // factor4 = (z0 - sigma') / z0
            let f4 = z0.sub(&sigma).div(&z0);
            // factor3 = (sigma' - p' * factor4) / z0
            let f3 = sigma.sub(&p.mul(&f4)).div(&z0);
            // factor2 = sigma' + p' * a0 * factor3
            let f2 = sigma.add(&p.mul(&a0_r).mul(&f3));
            // factor1 = R / (1 + a0)
            let f1 = Rat::from_i128(reward_pot as i128, 1).div(&Rat::from_i128(1, 1).add(&a0_r));
            // maxPool = floor(factor1 * factor2)
            let max_pool = f1.mul(&f2).floor_u64();

            // Apparent performance: beta / sigma_a (using total_active_stake)
            //   perf = (blocks_made / total_blocks) / (pool_stake / total_active_stake)
            //        = (blocks_made * total_active_stake) / (total_blocks * pool_stake)
            let blocks_made = go_snapshot
                .epoch_blocks_by_pool
                .get(pool_id)
                .copied()
                .unwrap_or(0);
            let pool_reward = if blocks_made == 0 || pool_active_stake.0 == 0 {
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

            // Operator reward: cost + (margin + (1-margin) * s/sigma) * max(0, pool_reward - cost)
            // where s/sigma = self_delegated / pool_stake (owner's fraction of pool)
            let cost = pool_reg.cost.0;
            let margin_num = pool_reg.margin_numerator as i128;
            let margin_den = pool_reg.margin_denominator.max(1) as i128;

            let operator_reward = if pool_reward <= cost {
                pool_reward
            } else {
                let remainder = pool_reward - cost;
                let margin = Rat::from_i128(margin_num, margin_den);
                let one_minus_margin = Rat::from_i128(margin_den - margin_num, margin_den);
                let s_over_sigma =
                    Rat::from_i128(self_delegated as i128, pool_active_stake.0 as i128);
                let share = margin.add(&one_minus_margin.mul(&s_over_sigma));
                let op_extra = share.mul(&Rat::from_i128(remainder as i128, 1)).floor_u64();
                cost + op_extra
            };

            // Distribute member rewards proportionally to delegators.
            // Pool owners are excluded — they receive only the operator reward.
            let owner_set: std::collections::HashSet<Hash32> = pool_reg
                .owners
                .iter()
                .map(|o| o.to_hash32_padded())
                .collect();

            if let Some(delegators) = delegators_by_pool.get(pool_id) {
                for cred_hash in delegators {
                    // Skip pool owners — they only get leader/operator reward
                    if owner_set.contains(cred_hash) {
                        continue;
                    }

                    let member_stake = go_snapshot
                        .stake_distribution
                        .get(cred_hash)
                        .copied()
                        .unwrap_or(Lovelace(0))
                        .0;

                    if member_stake == 0 || pool_active_stake.0 == 0 {
                        continue;
                    }

                    // Member share: floor((pool_reward - cost) * (1 - margin) * member_stake / pool_stake)
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
                        *reward_map.entry(*cred_hash).or_insert(Lovelace(0)) +=
                            Lovelace(member_share);
                        total_distributed += member_share;
                    }
                }
            }

            // Operator reward goes to pool's registered reward account
            if operator_reward > 0 {
                let op_key = Self::reward_account_to_hash(&pool_reg.reward_account);
                *reward_map.entry(op_key).or_insert(Lovelace(0)) += Lovelace(operator_reward);
                total_distributed += operator_reward;
            }
        }

        // Any undistributed rewards go to treasury
        let undistributed = reward_pot.saturating_sub(total_distributed);

        debug!(
            "Rewards calculated: {} lovelace to {} accounts, {} to treasury (expansion: {}, fees: {})",
            total_distributed,
            reward_map.len(),
            treasury_cut + undistributed,
            expansion,
            go_snapshot.epoch_fees.0
        );

        PendingRewardUpdate {
            rewards: reward_map,
            delta_treasury: treasury_cut + undistributed,
            delta_reserves: expansion,
        }
    }

    /// Legacy compatibility: calculate and immediately distribute rewards.
    ///
    /// Used by tests that expect immediate reward application. New code should
    /// use `calculate_rewards()` + `apply_pending_reward_update()` for correct
    /// Haskell-compatible RUPD timing.
    #[cfg(test)]
    pub(crate) fn calculate_and_distribute_rewards(&mut self, go_snapshot: StakeSnapshot) {
        let rupd = self.calculate_rewards(&go_snapshot);
        // Apply immediately (legacy behavior for test compatibility)
        self.reserves.0 = self.reserves.0.saturating_sub(rupd.delta_reserves);
        self.treasury.0 = self.treasury.0.saturating_add(rupd.delta_treasury);
        for (cred_hash, reward) in &rupd.rewards {
            if reward.0 > 0 {
                *Arc::make_mut(&mut self.reward_accounts)
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
