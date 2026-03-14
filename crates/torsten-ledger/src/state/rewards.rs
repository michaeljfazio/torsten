use super::{LedgerState, StakeSnapshot, MAX_LOVELACE_SUPPLY};
use std::collections::HashMap;
use std::sync::Arc;
use torsten_primitives::hash::{Hash28, Hash32};
use torsten_primitives::value::Lovelace;
use tracing::{debug, warn};

/// Reduced rational number (i128 numerator/denominator with GCD reduction).
/// Matches Haskell's Rational for reward calculations with rationalToCoinViaFloor.
///
/// Uses checked arithmetic with widening to i256 (via two i128s) to prevent
/// overflow on large intermediate products. Cross-reduction is applied first
/// as an optimization; widening handles the residual cases.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rat {
    pub n: i128,
    pub d: i128,
}

impl Rat {
    pub fn new(n: i128, d: i128) -> Self {
        if d == 0 {
            return Rat { n: 0, d: 1 };
        }
        let g = Self::gcd(n.unsigned_abs(), d.unsigned_abs()) as i128;
        let sign = if d < 0 { -1 } else { 1 };
        Rat {
            n: sign * n / g,
            d: sign * d / g,
        }
    }

    fn gcd(a: u128, b: u128) -> u128 {
        let (mut a, mut b) = (a, b);
        while b != 0 {
            let t = b;
            b = a % b;
            a = t;
        }
        a
    }

    /// Checked i128 multiply: returns Some if no overflow, None otherwise.
    #[inline]
    fn checked_mul(a: i128, b: i128) -> Option<i128> {
        a.checked_mul(b)
    }

    /// Widening multiply: (a * b) using i256 arithmetic, then reduce to i128.
    /// Returns (numerator_product, denominator_product) as a reduced Rat.
    fn wide_mul_rat(an: i128, ad: i128, bn: i128, bd: i128) -> Rat {
        // Use num_bigint for overflow-safe arithmetic
        use num_bigint::BigInt;
        let n = BigInt::from(an) * BigInt::from(bn);
        let d = BigInt::from(ad) * BigInt::from(bd);
        // Reduce
        let g = Self::bigint_gcd(&n, &d);
        let rn = &n / &g;
        let rd = &d / &g;
        // Convert back — if still doesn't fit in i128, this is a genuinely huge
        // number that shouldn't occur in practice. Saturate to keep going.
        let rn_i128: i128 = rn.try_into().unwrap_or_else(|_| {
            if n < BigInt::from(0) {
                i128::MIN
            } else {
                i128::MAX
            }
        });
        let rd_i128: i128 = rd.try_into().unwrap_or(i128::MAX);
        Rat::new(rn_i128, rd_i128)
    }

    fn bigint_gcd(a: &num_bigint::BigInt, b: &num_bigint::BigInt) -> num_bigint::BigInt {
        use num_traits::{Signed, Zero};
        let (mut a, mut b) = (a.abs(), b.abs());
        while !b.is_zero() {
            let t = b.clone();
            b = &a % &t;
            a = t;
        }
        if a.is_zero() {
            num_bigint::BigInt::from(1)
        } else {
            a
        }
    }

    /// Wide addition: a/b + c/d using BigInt when i128 overflows.
    fn wide_add_rat(an: i128, ad: i128, bn: i128, bd: i128) -> Rat {
        use num_bigint::BigInt;
        let n = BigInt::from(an) * BigInt::from(bd) + BigInt::from(bn) * BigInt::from(ad);
        let d = BigInt::from(ad) * BigInt::from(bd);
        let g = Self::bigint_gcd(&n, &d);
        let rn = &n / &g;
        let rd = &d / &g;
        let rn_i128: i128 = rn.try_into().unwrap_or_else(|_| {
            if n < BigInt::from(0) {
                i128::MIN
            } else {
                i128::MAX
            }
        });
        let rd_i128: i128 = rd.try_into().unwrap_or(i128::MAX);
        Rat::new(rn_i128, rd_i128)
    }

    /// Wide comparison: a/b <= c/d using BigInt to avoid overflow in cross-multiply.
    fn wide_le(an: i128, ad: i128, bn: i128, bd: i128) -> bool {
        use num_bigint::BigInt;
        BigInt::from(an) * BigInt::from(bd) <= BigInt::from(bn) * BigInt::from(ad)
    }

    pub fn add(&self, other: &Rat) -> Rat {
        // Cross-reduce before adding to prevent overflow:
        // a/b + c/d = (a*(d/g) + c*(b/g)) / (b/g*d)  where g = gcd(b,d)
        let g = Self::gcd(self.d.unsigned_abs(), other.d.unsigned_abs()) as i128;
        let bd = self.d / g;
        let dg = other.d / g;
        // Try i128 fast path
        if let (Some(t1), Some(t2), Some(den)) = (
            Self::checked_mul(self.n, dg),
            Self::checked_mul(other.n, bd),
            Self::checked_mul(bd, other.d),
        ) {
            if let Some(num) = t1.checked_add(t2) {
                return Rat::new(num, den);
            }
        }
        // Fallback to BigInt
        Self::wide_add_rat(self.n, self.d, other.n, other.d)
    }

    pub fn sub(&self, other: &Rat) -> Rat {
        self.add(&Rat::new(-other.n, other.d))
    }

    pub fn mul(&self, other: &Rat) -> Rat {
        // Cross-reduce before multiplying to prevent overflow:
        // (a/b) * (c/d) = (a/g1 * c/g2) / (b/g2 * d/g1) where g1=gcd(a,d), g2=gcd(b,c)
        let g1 = Self::gcd(self.n.unsigned_abs(), other.d.unsigned_abs()) as i128;
        let g2 = Self::gcd(self.d.unsigned_abs(), other.n.unsigned_abs()) as i128;
        let an = self.n / g1;
        let cn = other.n / g2;
        let bg = self.d / g2;
        let dg = other.d / g1;
        // Try i128 fast path
        if let (Some(num), Some(den)) = (Self::checked_mul(an, cn), Self::checked_mul(bg, dg)) {
            return Rat::new(num, den);
        }
        // Fallback to BigInt
        Self::wide_mul_rat(an, bg, cn, dg)
    }

    pub fn div(&self, other: &Rat) -> Rat {
        if other.n == 0 {
            return Rat::new(0, 1);
        }
        // (a/b) / (c/d) = (a/b) * (d/c)
        let g1 = Self::gcd(self.n.unsigned_abs(), other.n.unsigned_abs()) as i128;
        let g2 = Self::gcd(self.d.unsigned_abs(), other.d.unsigned_abs()) as i128;
        let an = self.n / g1;
        let od = other.d / g2;
        let bg = self.d / g2;
        let cn = other.n / g1;
        // Try i128 fast path
        if let (Some(num), Some(den)) = (Self::checked_mul(an, od), Self::checked_mul(bg, cn)) {
            return Rat::new(num, den);
        }
        // Fallback to BigInt
        Self::wide_mul_rat(an, bg, od, cn)
    }

    pub fn min_rat(&self, other: &Rat) -> Rat {
        // Compare using cross-multiplication: a/b <= c/d iff a*d <= c*b (when b,d > 0)
        let le = if let (Some(lhs), Some(rhs)) = (
            Self::checked_mul(self.n, other.d),
            Self::checked_mul(other.n, self.d),
        ) {
            lhs <= rhs
        } else {
            Self::wide_le(self.n, self.d, other.n, other.d)
        };
        if le {
            *self
        } else {
            *other
        }
    }

    pub fn floor_u64(&self) -> u64 {
        if self.d == 0 || self.n <= 0 {
            0
        } else {
            (self.n / self.d) as u64
        }
    }
}

impl LedgerState {
    /// Calculate and distribute rewards according to the Cardano Shelley reward formula.
    ///
    /// Implements the formula from cardano-ledger-shelley:
    ///   - maxPool'(a0, nOpt, R, sigma, p) for pledge-influenced pool rewards
    ///   - mkApparentPerformance for beta/sigma performance calculation
    ///   - Pledge verification (pool gets zero if owner stake < declared pledge)
    ///   - Operator reward includes self-delegation share (margin + proportional)
    ///   - Operator reward goes to pool's registered reward account
    pub(crate) fn calculate_and_distribute_rewards(&mut self, go_snapshot: StakeSnapshot) {
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
        let actual_blocks = self.epoch_block_count;
        // eta = min(1, actual/expected) — applied as rational: min(1, actual/expected)
        // expansion = floor(min(actual, expected) / expected * rho * reserves)
        let effective_blocks = actual_blocks.min(expected_blocks);
        // Use Rat to avoid i128 overflow: rho * reserves * (effective/expected)
        let rho = Rat::new(rho_num, rho_den);
        let expansion_rat = rho
            .mul(&Rat::new(self.reserves.0 as i128, 1))
            .mul(&Rat::new(effective_blocks as i128, expected_blocks as i128));
        let expansion = expansion_rat.floor_u64();
        let total_rewards_available = expansion + self.epoch_fees.0;

        if total_rewards_available == 0 {
            return;
        }

        // Move expansion from reserves
        self.reserves.0 = self.reserves.0.saturating_sub(expansion);

        // Treasury cut: floor(tau * total_rewards)
        let tau = Rat::new(tau_num, tau_den);
        let treasury_cut = tau
            .mul(&Rat::new(total_rewards_available as i128, 1))
            .floor_u64();
        self.treasury.0 = self.treasury.0.saturating_add(treasury_cut);

        let reward_pot = total_rewards_available - treasury_cut;

        // Total stake for sigma denominator: circulation = maxSupply - reserves
        let total_stake = MAX_LOVELACE_SUPPLY.saturating_sub(self.reserves.0);
        if total_stake == 0 {
            self.treasury.0 = self.treasury.0.saturating_add(reward_pot);
            return;
        }

        // Total active stake (for apparent performance denominator)
        let total_active_stake: u64 = go_snapshot
            .pool_stake
            .values()
            .fold(0u64, |acc, s| acc.saturating_add(s.0));
        if total_active_stake == 0 {
            self.treasury.0 = self.treasury.0.saturating_add(reward_pot);
            return;
        }

        // Total blocks produced this epoch
        let total_blocks_in_epoch = self.epoch_block_count.max(1);

        // Saturation point: z0 = 1/nOpt
        let n_opt = self.protocol_params.n_opt.max(1);

        let mut total_distributed: u64 = 0;

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

            // maxPool'(a0, nOpt, R, sigma, p) using rational arithmetic:
            //   z0 = 1/nOpt
            //   sigma' = min(sigma, z0), p' = min(p, z0)
            //   maxPool = floor(R/(1+a0) * (sigma' + p' * a0 * (sigma' - p'*(z0-sigma')/z0) / z0))
            //
            // Uses Rat (i128 num/den with GCD reduction) to match Haskell's Rational.
            let a0_r = Rat::new(
                self.protocol_params.a0.numerator as i128,
                self.protocol_params.a0.denominator.max(1) as i128,
            );
            let z0 = Rat::new(1, n_opt as i128);
            let sigma_raw = Rat::new(pool_active_stake.0 as i128, total_stake as i128);
            let p_raw = Rat::new(pool_reg.pledge.0 as i128, total_stake as i128);
            let sigma = sigma_raw.min_rat(&z0);
            let p = p_raw.min_rat(&z0);

            // factor4 = (z0 - sigma') / z0
            let f4 = z0.sub(&sigma).div(&z0);
            // factor3 = (sigma' - p' * factor4) / z0
            let f3 = sigma.sub(&p.mul(&f4)).div(&z0);
            // factor2 = sigma' + p' * a0 * factor3
            let f2 = sigma.add(&p.mul(&a0_r).mul(&f3));
            // factor1 = R / (1 + a0)
            let f1 = Rat::new(reward_pot as i128, 1).div(&Rat::new(1, 1).add(&a0_r));
            // maxPool = floor(factor1 * factor2)
            let max_pool = f1.mul(&f2).floor_u64();

            // Apparent performance: beta / sigma_a (rational arithmetic)
            //   perf = (blocks_made / total_blocks) / (pool_stake / total_active_stake)
            //        = (blocks_made * total_active_stake) / (total_blocks * pool_stake)
            let blocks_made = self.epoch_blocks_by_pool.get(pool_id).copied().unwrap_or(0);
            let pool_reward = if blocks_made == 0 || pool_active_stake.0 == 0 {
                0u64
            } else {
                // perf = (blocks_made / total_blocks) / (pool_stake / total_active_stake)
                // Use Rat chained multiplication to avoid i128 overflow
                let perf = Rat::new(blocks_made as i128, total_blocks_in_epoch as i128).mul(
                    &Rat::new(total_active_stake as i128, pool_active_stake.0 as i128),
                );
                perf.mul(&Rat::new(max_pool as i128, 1)).floor_u64()
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
                // operator_share = margin + (1-margin) * s/sigma
                // Use Rat to avoid i128 overflow in cross terms
                let margin = Rat::new(margin_num, margin_den);
                let one_minus_margin = Rat::new(margin_den - margin_num, margin_den);
                let s_over_sigma = Rat::new(self_delegated as i128, pool_active_stake.0 as i128);
                let share = margin.add(&one_minus_margin.mul(&s_over_sigma));
                let op_extra = share.mul(&Rat::new(remainder as i128, 1)).floor_u64();
                cost + op_extra
            };

            // Distribute member rewards proportionally to delegators.
            // Pool owners are excluded — they receive only the operator reward.
            // Build owner set (as Hash32 keys) for filtering
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
                        // Use Rat to avoid i128 overflow in cross terms
                        let one_minus_margin = Rat::new(margin_den - margin_num, margin_den);
                        let member_frac =
                            Rat::new(member_stake as i128, pool_active_stake.0 as i128);
                        Rat::new(remainder as i128, 1)
                            .mul(&one_minus_margin)
                            .mul(&member_frac)
                            .floor_u64()
                    };

                    if member_share > 0 {
                        *Arc::make_mut(&mut self.reward_accounts)
                            .entry(*cred_hash)
                            .or_insert(Lovelace(0)) += Lovelace(member_share);
                        total_distributed += member_share;
                    }
                }
            }

            // Operator reward goes to pool's registered reward account
            if operator_reward > 0 {
                let op_key = Self::reward_account_to_hash(&pool_reg.reward_account);
                *Arc::make_mut(&mut self.reward_accounts)
                    .entry(op_key)
                    .or_insert(Lovelace(0)) += Lovelace(operator_reward);
                total_distributed += operator_reward;
            }
        }

        // Any undistributed rewards go to treasury
        let undistributed = reward_pot.saturating_sub(total_distributed);
        if undistributed > 0 {
            self.treasury.0 = self.treasury.0.saturating_add(undistributed);
        }

        debug!(
            "Rewards distributed: {} lovelace to accounts, {} to treasury (expansion: {}, fees: {})",
            total_distributed, treasury_cut + undistributed, expansion, self.epoch_fees.0
        );
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
        assert_eq!(Rat::new(13, 17).n, 13);
        assert_eq!(Rat::new(13, 17).d, 17);
    }

    #[test]
    fn test_gcd_reduces_fractions() {
        let r = Rat::new(6, 9);
        assert_eq!(r.n, 2);
        assert_eq!(r.d, 3);
    }

    #[test]
    fn test_gcd_large_values() {
        // GCD(2^60, 2^40) = 2^40
        let a = 1i128 << 60;
        let b = 1i128 << 40;
        let r = Rat::new(a, b);
        assert_eq!(r.n, 1i128 << 20);
        assert_eq!(r.d, 1);
    }

    // -----------------------------------------------------------------------
    // Rat multiplication near i128::MAX
    // -----------------------------------------------------------------------

    #[test]
    fn test_rat_mul_near_i128_max() {
        // Two large values that would overflow i128 without BigInt fallback
        let a = Rat::new(i128::MAX / 2, 1);
        let b = Rat::new(3, 1);
        let result = a.mul(&b);
        // Should not panic; result is valid (uses BigInt fallback)
        assert!(result.d > 0);
        // (MAX/2)*3 ≈ 1.5*MAX, so result.n should be saturated to i128::MAX
        // or handled via BigInt
        assert!(result.n > 0);
    }

    #[test]
    fn test_rat_mul_cross_reduce_prevents_overflow() {
        // (large/small) * (small/large) should cross-reduce cleanly
        let a = Rat::new(1_000_000_000_000_000, 7);
        let b = Rat::new(7, 1_000_000_000_000_000);
        let result = a.mul(&b);
        assert_eq!(result.n, 1);
        assert_eq!(result.d, 1);
    }

    // -----------------------------------------------------------------------
    // Rat addition near i128::MAX
    // -----------------------------------------------------------------------

    #[test]
    fn test_rat_add_near_i128_max() {
        let a = Rat::new(i128::MAX / 2, 1);
        let b = Rat::new(i128::MAX / 2, 1);
        let result = a.add(&b);
        // Should not panic; uses BigInt fallback
        assert!(result.n > 0);
        assert!(result.d > 0);
    }

    #[test]
    fn test_rat_add_different_denominators() {
        let a = Rat::new(1, 3);
        let b = Rat::new(1, 6);
        let result = a.add(&b);
        // 1/3 + 1/6 = 3/6 = 1/2
        assert_eq!(result.n, 1);
        assert_eq!(result.d, 2);
    }

    // -----------------------------------------------------------------------
    // Division producing very small fractions
    // -----------------------------------------------------------------------

    #[test]
    fn test_rat_div_very_small_fraction() {
        let a = Rat::new(1, 1_000_000_000);
        let b = Rat::new(1_000_000_000, 1);
        let result = a.div(&b);
        // 1/10^9 / 10^9 = 1/10^18
        assert_eq!(result.n, 1);
        assert_eq!(result.d, 1_000_000_000_000_000_000);
    }

    #[test]
    fn test_rat_div_by_zero_returns_zero() {
        let a = Rat::new(5, 3);
        let b = Rat::new(0, 1);
        let result = a.div(&b);
        assert_eq!(result.n, 0);
    }

    // -----------------------------------------------------------------------
    // Negative Rat values
    // -----------------------------------------------------------------------

    #[test]
    fn test_rat_negative_numerator() {
        let r = Rat::new(-3, 4);
        assert_eq!(r.n, -3);
        assert_eq!(r.d, 4);
    }

    #[test]
    fn test_rat_negative_denominator_normalized() {
        // Negative denominator should be normalized to positive
        let r = Rat::new(3, -4);
        assert_eq!(r.n, -3);
        assert_eq!(r.d, 4);
    }

    #[test]
    fn test_rat_both_negative() {
        let r = Rat::new(-6, -8);
        assert_eq!(r.n, 3);
        assert_eq!(r.d, 4);
    }

    #[test]
    fn test_rat_sub_produces_negative() {
        let a = Rat::new(1, 4);
        let b = Rat::new(3, 4);
        let result = a.sub(&b);
        assert_eq!(result.n, -1);
        assert_eq!(result.d, 2);
    }

    // -----------------------------------------------------------------------
    // Floor
    // -----------------------------------------------------------------------

    #[test]
    fn test_rat_floor_u64_negative_returns_zero() {
        let r = Rat::new(-5, 3);
        assert_eq!(r.floor_u64(), 0);
    }

    #[test]
    fn test_rat_floor_u64_exact_division() {
        let r = Rat::new(10, 5);
        assert_eq!(r.floor_u64(), 2);
    }

    #[test]
    fn test_rat_floor_u64_truncates() {
        let r = Rat::new(7, 3);
        assert_eq!(r.floor_u64(), 2); // 7/3 = 2.333...
    }

    // -----------------------------------------------------------------------
    // min_rat
    // -----------------------------------------------------------------------

    #[test]
    fn test_rat_min_rat() {
        let a = Rat::new(1, 3);
        let b = Rat::new(1, 2);
        assert_eq!(a.min_rat(&b), a);
        assert_eq!(b.min_rat(&a), a);
    }

    #[test]
    fn test_rat_min_rat_equal() {
        let a = Rat::new(2, 4);
        let b = Rat::new(1, 2);
        // Both are 1/2 after reduction
        let result = a.min_rat(&b);
        assert_eq!(result.n, 1);
        assert_eq!(result.d, 2);
    }

    // -----------------------------------------------------------------------
    // Zero denominator
    // -----------------------------------------------------------------------

    #[test]
    fn test_rat_zero_denominator() {
        let r = Rat::new(5, 0);
        assert_eq!(r.n, 0);
        assert_eq!(r.d, 1);
    }
}
