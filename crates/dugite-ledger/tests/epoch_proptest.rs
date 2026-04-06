//! Property-based tests for epoch transition invariants.
//!
//! All 3 properties use 256 test cases each and are cross-validated against the
//! Haskell cardano-ledger NEWEPOCH/SNAP/RUPD rules.
//!
//! # Haskell cross-validation notes
//!
//! - **Property 1 (reward pot bound)**: Haskell's `startStep` computes
//!   `rPot = ssFee + deltaR1` where `deltaR1 = floor(rho * reserves)` (when
//!   `d >= 0.8`).  The treasury cut `deltaT = floor(tau * rPot)` and the
//!   reward pot `R = rPot - deltaT`.  All distributed pool/member rewards are
//!   individually floored, so `sum(distributed) <= R`.
//!
//! - **Property 2 (six-pot ADA conservation)**: Haskell's invariant is
//!   `treasury + reserves + utxoValue + rewards + deposits + feePot = maxSupply`.
//!   The RUPD is ADA-neutral: `reserves -= expansion`, `treasury += treasury_cut`,
//!   `rewards += member_rewards`, and `expansion = treasury_cut + member_rewards +
//!   undistributed`, with `fee_pot → ssFee (used in rPot) → now zero`.
//!
//! - **Property 3 (snapshot rotation)**: Haskell's SNAP rule performs
//!   `ssStakeMark' = currentStake`, `ssStakeSet' = ssStakeMark`, `ssStakeGo' =
//!   ssStakeSet`.  The new mark is computed *after* rewards have been applied
//!   (post-applyRUpd ordering in NEWEPOCH).

#[path = "strategies.rs"]
mod strategies;

use dugite_ledger::LedgerState;
use dugite_primitives::time::EpochNo;
use proptest::prelude::*;
use strategies::{arb_ledger_state, LedgerStateConfig};

// ---------------------------------------------------------------------------
// Six-pot total helper
// ---------------------------------------------------------------------------

/// Compute the six-pot ADA total for a ledger state.
///
/// The six pots are:
///   1. `utxo_total`     — total ADA in the UTxO set
///   2. `reserves`       — ADA not yet in circulation
///   3. `treasury`       — protocol treasury balance
///   4. `reward_accounts`— sum of all reward account balances
///   5. `deposits_pot`   — total_stake_key_deposits + sum(pool_deposits)
///   6. `fee_pot`        — accumulated fees for the current epoch
///
/// Also adds `pending_donations` (a seventh pot) if non-zero, because it
/// buffers treasury-bound ADA between block application and epoch transition.
///
/// This mirrors Haskell's `totalAdaES`:
/// ```
///   totalAdaES = utxoBalance + asReserves + asTreasury
///              + sumRewards + deposits + feePot
/// ```
fn compute_six_pot_total(state: &LedgerState) -> u64 {
    let utxo_total = state.utxo_set.total_lovelace().0;
    let reserves = state.reserves.0;
    let treasury = state.treasury.0;
    let reward_accounts: u64 = state.reward_accounts.values().map(|l| l.0).sum();
    let deposits_pot: u64 = state.total_stake_key_deposits
        + state.pool_deposits.values().sum::<u64>()
        + state
            .governance
            .dreps
            .values()
            .map(|d| d.deposit.0)
            .sum::<u64>()
        + state
            .governance
            .proposals
            .values()
            .map(|p| p.procedure.deposit.0)
            .sum::<u64>();
    let fee_pot = state.epoch_fees.0;
    // pending_donations buffers treasury-bound ADA from tx bodies; include it
    // so the identity holds in both pre- and post-transition states.
    let pending_donations = state.pending_donations.0;

    utxo_total
        .saturating_add(reserves)
        .saturating_add(treasury)
        .saturating_add(reward_accounts)
        .saturating_add(deposits_pot)
        .saturating_add(fee_pot)
        .saturating_add(pending_donations)
}

// ---------------------------------------------------------------------------
// Property 1: Reward distribution bounded by available pot
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Verify that the total ADA credited to reward accounts during an epoch
    /// transition never exceeds the available reward pot after treasury cut.
    ///
    /// # Haskell RUPD formula
    ///
    /// ```text
    /// expansion            = floor(min(1, eta) * rho * reserves)
    /// totalRewardsAvailable = expansion + ssFee
    /// treasuryCut          = floor(tau * totalRewardsAvailable)
    /// rewardPot            = totalRewardsAvailable - treasuryCut
    /// sum(distributed)    <= rewardPot
    /// ```
    ///
    /// When `d >= 0.8` (fully or heavily federated), `eta = 1`.
    /// `ssFee` is captured by the SNAP rule at the previous epoch boundary.
    ///
    /// Note: `arb_ledger_state` initialises `prev_d = 1.0` (matching
    /// `LedgerState::new` which sets `prev_d = 1.0` for genesis).  Therefore
    /// `d >= 0.8` holds and expansion uses the full monetary expansion formula.
    #[test]
    fn prop_reward_distribution_bounded_by_pot(
        state in arb_ledger_state(LedgerStateConfig {
            max_pools: 5,
            max_delegations: 20,
            epoch: 500,
        })
    ) {
        // ── Step 0: Capture pre-transition inputs for the reward-pot formula ──

        // `ss_fee` is the fee pot captured by the SNAP rule at the previous
        // boundary (Haskell's `ssFee`).  This is what `calculate_rewards_inner`
        // uses as `epoch_fees` — NOT `state.epoch_fees`.
        let ss_fee = state.snapshots.ss_fee.0;

        // `prev_d` determines whether `eta = 1` or a performance ratio.
        // `LedgerState::new` sets `prev_d = 1.0`, so the generated states
        // always take the `d >= 0.8` branch.
        let prev_d = state.prev_d;

        // Use `prev_protocol_params` for `rho` and `tau`, matching Haskell's
        // `startStep` which reads from `prevPParams`.
        let pp = &state.prev_protocol_params;
        let rho_num = pp.rho.numerator;
        let rho_den = pp.rho.denominator.max(1);
        let tau_num = pp.tau.numerator;
        let tau_den = pp.tau.denominator.max(1);

        // ── Step 1: Compute expected reward pot ───────────────────────────────

        // monetary expansion (delta_R1 in Haskell nomenclature)
        let expansion: u64 = if prev_d >= 0.8 {
            // eta = 1: full expansion
            // floor(rho * reserves) — use u128 to avoid overflow on large reserves
            let reserves = state.reserves.0;
            let num = (reserves as u128) * (rho_num as u128);
            let den = rho_den as u128;
            (num / den) as u64
        } else {
            // eta < 1: scale by actual/expected blocks
            // arb_ledger_state sets bprev_block_count = 0 so this is 0.
            0
        };

        let total_rewards_available = expansion.saturating_add(ss_fee);

        // treasury cut = floor(tau * total_rewards_available)
        let treasury_cut: u64 = {
            let num = (total_rewards_available as u128) * (tau_num as u128);
            let den = tau_den as u128;
            if den == 0 { 0 } else { (num / den) as u64 }
        };

        let reward_pot = total_rewards_available.saturating_sub(treasury_cut);

        // ── Step 2: Run the epoch transition ─────────────────────────────────

        let mut state = state;
        let new_epoch = EpochNo(state.epoch.0 + 1);

        // Record reward account balances before the transition.
        let pre_rewards: std::collections::HashMap<_, _> = state
            .reward_accounts
            .iter()
            .map(|(k, v)| (*k, v.0))
            .collect();

        state.process_epoch_transition(new_epoch);

        // ── Step 3: Measure total rewards credited ───────────────────────────

        // Sum the INCREASE in each reward account.  Accounts that decreased
        // (no pool retirements that refund deposits in arb_ledger_state) are
        // not counted.  New accounts (registered via certificates, impossible
        // in arb_ledger_state) would also be counted here.
        let total_credited: u64 = state
            .reward_accounts
            .iter()
            .map(|(cred, &post)| {
                let pre = pre_rewards.get(cred).copied().unwrap_or(0);
                post.0.saturating_sub(pre)
            })
            .sum();

        // ── Step 4: Assert the bound ──────────────────────────────────────────

        // Pool deposits refunded to reward accounts at retirement are not counted
        // in the reward pot.  However, `arb_ledger_state` does not set any
        // pending retirements, so all credits come from RUPD rewards only.
        //
        // The assertion: total rewards credited <= reward_pot.
        prop_assert!(
            total_credited <= reward_pot,
            "Rewards credited ({total_credited}) exceeded reward pot ({reward_pot}): \
             expansion={expansion}, ss_fee={ss_fee}, treasury_cut={treasury_cut}, \
             tau={tau_num}/{tau_den}, rho={rho_num}/{rho_den}, \
             reserves={}",
            state.reserves.0,
        );
    }
}

// ---------------------------------------------------------------------------
// Property 2: Total ADA conservation (six-pot identity)
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Verify that the six-pot ADA sum is conserved across an epoch transition.
    ///
    /// The epoch transition must be ADA-neutral: the sum of all six pots is
    /// identical before and after `process_epoch_transition()`.
    ///
    /// The six pots are:
    ///   `utxo_total + reserves + treasury + sum(reward_accounts) + deposits_pot + fee_pot`
    ///
    /// The ADA flows during RUPD are:
    ///   - `reserves -= (expansion - ssFee_offset)` (net_reserve_decrease)
    ///   - `treasury += treasury_cut`
    ///   - `reward_accounts += member_rewards`
    ///   - `epoch_fees (ssFee via ss_fee) → consumed by rPot; epoch_fees reset to 0`
    ///
    /// These flows are ADA-neutral when all six pots are included:
    ///   `expansion = treasury_cut + member_rewards + undistributed`
    ///   `net_reserve_decrease = treasury_cut + member_rewards - ssFee`
    ///   `change = -net_reserve_decrease + treasury_cut + member_rewards + 0 - ssFee = 0`
    ///
    /// # Why we check pre == post, not pre == MAX_LOVELACE_SUPPLY
    ///
    /// The `arb_ledger_state` generator uses integer division when distributing
    /// ADA across pots (e.g. `reward_per_key = rewards_total / n_delegations`),
    /// producing a total that may be 1–10 lovelace below `MAX_LOVELACE_SUPPLY`.
    /// Rather than constraining the generator (which would complicate it for
    /// marginal benefit), we verify only that the *transition itself* is
    /// ADA-neutral.  This directly tests the invariant that matters: the epoch
    /// transition cannot create or destroy ADA.
    #[test]
    fn prop_ada_conservation_across_epoch(
        state in arb_ledger_state(LedgerStateConfig {
            max_pools: 5,
            max_delegations: 20,
            epoch: 500,
        })
    ) {
        // ── Step 1: Record pre-transition six-pot total ───────────────────────

        let pre_total = compute_six_pot_total(&state);

        // ── Step 2: Run the epoch transition ─────────────────────────────────

        let mut state = state;
        let new_epoch = EpochNo(state.epoch.0 + 1);
        state.process_epoch_transition(new_epoch);

        // ── Step 3: Verify the post-transition total is unchanged ─────────────

        let post_total = compute_six_pot_total(&state);
        let post_deposits = state.total_stake_key_deposits
            + state.pool_deposits.values().sum::<u64>()
            + state.governance.dreps.values().map(|d| d.deposit.0).sum::<u64>()
            + state.governance.proposals.values().map(|p| p.procedure.deposit.0).sum::<u64>();
        prop_assert!(
            post_total == pre_total,
            "Six-pot total changed during epoch transition: pre={} post={}: \
             utxo={}, reserves={}, treasury={}, rewards={}, deposits={}, fee_pot={}",
            pre_total,
            post_total,
            state.utxo_set.total_lovelace().0,
            state.reserves.0,
            state.treasury.0,
            state.reward_accounts.values().map(|l| l.0).sum::<u64>(),
            post_deposits,
            state.epoch_fees.0,
        );
    }
}

// ---------------------------------------------------------------------------
// Property 3: Snapshot rotation correctness
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Verify that the mark/set/go snapshot rotation follows the Haskell SNAP rule.
    ///
    /// At each epoch boundary, Haskell's SNAP rule performs:
    ///   - `ssStakeGo'   = ssStakeSet`      (old set becomes new go)
    ///   - `ssStakeSet'  = ssStakeMark`     (old mark becomes new set)
    ///   - `ssStakeMark' = currentStake`    (new mark from post-RUPD stake)
    ///
    /// The new mark epoch tag should equal `new_epoch` (the epoch being entered).
    /// The new set epoch tag should equal the old mark's epoch tag.
    /// The new go epoch tag should equal the old set's epoch tag.
    ///
    /// # Ordering note
    ///
    /// In Haskell's NEWEPOCH ordering, RUPD is applied BEFORE SNAP.  Therefore
    /// the new mark is computed from the stake distribution that includes
    /// freshly-credited rewards — matching the spec's "new mark computed after
    /// rewards are credited" requirement.
    ///
    /// # None handling
    ///
    /// `arb_ledger_state` initialises all three snapshots to `Some(...)` with
    /// epoch = `config.epoch - 1`.  After rotation:
    ///   - `mark`  = `Some(...)` with `epoch = new_epoch`
    ///   - `set`   = the old `mark` (same reference after take/put)
    ///   - `go`    = the old `set`
    #[test]
    fn prop_snapshot_rotation(
        state in arb_ledger_state(LedgerStateConfig {
            max_pools: 5,
            max_delegations: 20,
            epoch: 500,
        })
    ) {
        // ── Step 1: Record pre-transition snapshot epochs ────────────────────

        // The epoch tag stored inside each snapshot tells us which epoch it
        // was captured at.  We use these to verify the rotation without
        // comparing full snapshot content (which would be expensive).
        let pre_mark_epoch: Option<EpochNo> = state.snapshots.mark.as_ref().map(|s| s.epoch);
        let pre_set_epoch: Option<EpochNo> = state.snapshots.set.as_ref().map(|s| s.epoch);
        let pre_go_epoch: Option<EpochNo> = state.snapshots.go.as_ref().map(|s| s.epoch);

        // Also record the number of pools in each snapshot for a structural
        // (not just epoch-tag) sanity check that the correct snapshot was rotated.
        let pre_mark_pools: Option<usize> =
            state.snapshots.mark.as_ref().map(|s| s.pool_params.len());
        let pre_set_pools: Option<usize> =
            state.snapshots.set.as_ref().map(|s| s.pool_params.len());

        // ── Step 2: Run the epoch transition ─────────────────────────────────

        let mut state = state;
        let new_epoch = EpochNo(state.epoch.0 + 1);
        state.process_epoch_transition(new_epoch);

        // ── Step 3: Verify the new mark has the correct epoch tag ────────────

        // After rotation: new mark epoch = new_epoch (the epoch being entered).
        let post_mark_epoch = state.snapshots.mark.as_ref().map(|s| s.epoch);
        prop_assert_eq!(
            post_mark_epoch,
            Some(new_epoch),
            "New mark epoch should be new_epoch={} but was {:?}",
            new_epoch.0,
            post_mark_epoch,
        );

        // ── Step 4: Verify old mark became new set (epoch tag preserved) ─────

        // Haskell: ssStakeSet' = ssStakeMark.  The epoch tag inside the
        // snapshot does not change during rotation — only the slot (mark/set/go)
        // it occupies changes.
        let post_set_epoch = state.snapshots.set.as_ref().map(|s| s.epoch);
        prop_assert_eq!(
            post_set_epoch,
            pre_mark_epoch,
            "New set epoch ({:?}) should equal old mark epoch ({:?})",
            post_set_epoch,
            pre_mark_epoch,
        );

        // ── Step 5: Verify old set became new go (epoch tag preserved) ───────

        // Haskell: ssStakeGo' = ssStakeSet.
        let post_go_epoch = state.snapshots.go.as_ref().map(|s| s.epoch);
        prop_assert_eq!(
            post_go_epoch,
            pre_set_epoch,
            "New go epoch ({:?}) should equal old set epoch ({:?})",
            post_go_epoch,
            pre_set_epoch,
        );

        // ── Step 6: Structural pool-count check ──────────────────────────────

        // Verify that the *content* of the rotated snapshots matches (not just
        // the epoch tag).  We compare the pool count as a lightweight proxy for
        // "same snapshot data moved to the next slot".
        //
        // New set == old mark (pool count should be identical).
        let post_set_pools = state.snapshots.set.as_ref().map(|s| s.pool_params.len());
        prop_assert_eq!(
            post_set_pools,
            pre_mark_pools,
            "New set pool count ({:?}) should equal old mark pool count ({:?})",
            post_set_pools,
            pre_mark_pools,
        );

        // New go == old set (pool count should be identical).
        let post_go_pools = state.snapshots.go.as_ref().map(|s| s.pool_params.len());
        prop_assert_eq!(
            post_go_pools,
            pre_set_pools,
            "New go pool count ({:?}) should equal old set pool count ({:?})",
            post_go_pools,
            pre_set_pools,
        );

        // ── Step 7: Epoch monotonicity ────────────────────────────────────────

        // The go snapshot's epoch is at most the set's epoch, which is at most
        // the mark's epoch, which is at most new_epoch.
        if let (Some(go), Some(set), Some(mark)) = (post_go_epoch, post_set_epoch, post_mark_epoch) {
            prop_assert!(
                go.0 <= set.0,
                "go epoch ({}) must be <= set epoch ({})",
                go.0,
                set.0,
            );
            prop_assert!(
                set.0 <= mark.0,
                "set epoch ({}) must be <= mark epoch ({})",
                set.0,
                mark.0,
            );
        }

        // The pre-existing go snapshot (if any) is replaced by the old set.
        // Only assert the go changed when old_set and old_go had DIFFERENT epoch
        // tags — if they were the same (as in arb_ledger_state which initialises
        // all three to epoch-1), the new go will naturally equal the old go.
        if let (Some(pre_go), Some(pre_set)) = (pre_go_epoch, pre_set_epoch) {
            if pre_go != pre_set {
                // After rotation, new go == old set (not old go).
                prop_assert_eq!(
                    post_go_epoch,
                    Some(pre_set),
                    "New go epoch ({:?}) should equal old set epoch ({:?}), not old go ({:?})",
                    post_go_epoch,
                    pre_set,
                    pre_go,
                );
            }
        }
    }
}
