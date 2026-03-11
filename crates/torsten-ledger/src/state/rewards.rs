use super::{LedgerState, StakeSnapshot, MAX_LOVELACE_SUPPLY};
use std::collections::HashMap;
use std::sync::Arc;
use torsten_primitives::hash::{Hash28, Hash32};
use torsten_primitives::value::Lovelace;
use tracing::{debug, info, warn};

/// Reduced rational number (i128 numerator/denominator with GCD reduction).
/// Matches Haskell's Rational for reward calculations with rationalToCoinViaFloor.
#[derive(Clone, Copy)]
pub(crate) struct Rat {
    pub(crate) n: i128,
    pub(crate) d: i128,
}

impl Rat {
    pub(crate) fn new(n: i128, d: i128) -> Self {
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
        if b == 0 {
            a
        } else {
            Self::gcd(b, a % b)
        }
    }

    pub(crate) fn add(&self, other: &Rat) -> Rat {
        // Cross-reduce before adding to prevent overflow:
        // a/b + c/d = (a*(d/g) + c*(b/g)) / (b/g*d)  where g = gcd(b,d)
        let g = Self::gcd(self.d.unsigned_abs(), other.d.unsigned_abs()) as i128;
        let bd = self.d / g;
        Rat::new(self.n * (other.d / g) + other.n * bd, bd * other.d)
    }

    pub(crate) fn sub(&self, other: &Rat) -> Rat {
        let g = Self::gcd(self.d.unsigned_abs(), other.d.unsigned_abs()) as i128;
        let bd = self.d / g;
        Rat::new(self.n * (other.d / g) - other.n * bd, bd * other.d)
    }

    pub(crate) fn mul(&self, other: &Rat) -> Rat {
        // Cross-reduce before multiplying to prevent overflow:
        // (a/b) * (c/d) = (a/g1 * c/g2) / (b/g2 * d/g1) where g1=gcd(a,d), g2=gcd(b,c)
        let g1 = Self::gcd(self.n.unsigned_abs(), other.d.unsigned_abs()) as i128;
        let g2 = Self::gcd(self.d.unsigned_abs(), other.n.unsigned_abs()) as i128;
        Rat::new(
            (self.n / g1) * (other.n / g2),
            (self.d / g2) * (other.d / g1),
        )
    }

    pub(crate) fn div(&self, other: &Rat) -> Rat {
        // (a/b) / (c/d) = (a/b) * (d/c)
        let g1 = Self::gcd(self.n.unsigned_abs(), other.n.unsigned_abs()) as i128;
        let g2 = Self::gcd(self.d.unsigned_abs(), other.d.unsigned_abs()) as i128;
        Rat::new(
            (self.n / g1) * (other.d / g2),
            (self.d / g2) * (other.n / g1),
        )
    }

    pub(crate) fn min_rat(&self, other: &Rat) -> Rat {
        // Compare using cross-multiplication: a/b <= c/d iff a*d <= c*b (when b,d > 0)
        if self.n * other.d <= other.n * self.d {
            *self
        } else {
            *other
        }
    }

    pub(crate) fn floor_u64(&self) -> u64 {
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
        let total_active_stake: u64 = go_snapshot.pool_stake.values().map(|s| s.0).sum();
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

        info!(
            "Rewards distributed: {} lovelace to accounts, {} to treasury (expansion: {}, fees: {})",
            total_distributed, treasury_cut + undistributed, expansion, self.epoch_fees.0
        );
    }
}
