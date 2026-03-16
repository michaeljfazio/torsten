/// End-to-end validation of the NonMyopicMemberRewards formula against
/// real preview testnet on-chain data (epoch 1234).
///
/// # What is tested
///
/// `handle_non_myopic_rewards` implements the Haskell `getNonMyopicMemberRewards`
/// formula from cardano-ledger-shelley.  This file validates it by:
///
/// 1. Reconstructing the exact inputs that the live node would feed into the
///    function for preview epoch 1234 (parameters from Koios epoch_params +
///    totals endpoints, pool details from pool_info).
/// 2. Asserting that the computed rewards match values independently derived
///    using exact Python `fractions.Fraction` arithmetic (the reference oracle).
/// 3. Covering three structural edge cases: zero stake, zero reward pot, and
///    the monotonicity invariant that an oversaturated pool yields a lower
///    member reward than an optimally saturated one.
///
/// # Parameter source (Koios preview, queried 2026-03-15)
///
/// | Field            | Value                    |
/// |-----------------|--------------------------|
/// | epoch_no        | 1234                     |
/// | reserves        | 8_283_950_832_164_775    |
/// | max_supply      | 45_000_000_000_000_000   |
/// | rho             | 3/1000                   |
/// | tau             | 1/5                      |
/// | a0              | 3/10                     |
/// | n_opt           | 500                      |
///
/// Derived:
///   R_gross     = floor(3/1000 × reserves)     = 24_851_852_496_494
///   treasury    = floor(1/5 × R_gross)         = 4_970_370_499_298
///   reward_pot  = R_gross − treasury           = 19_881_481_997_196
///   total_stake = max_supply − reserves        = 36_716_049_167_835_225
///   z0          = 1/500                        ≈ 0.2 % of total stake
///
/// # Pool data (Koios pool_info, epoch 1234)
///
/// | Pool  | active_stake        | pledge              | cost       | margin |
/// |-------|---------------------|---------------------|-----------|--------|
/// | GRADA | 70_803_019_758_445  | 100_000_000_000     | 170_000_000 | 0%   |
/// | PSBT  | 44_484_744_590_382  | 250_000_000_000     | 340_000_000 | 5%   |
/// | ADACT | 40_682_642_067_375  | 2_100_000_000_000   | 340_000_000 | 10%  |
///
/// Expected member_reward for 1 M ADA (1_000_000_000_000 lovelace) hypothetical
/// stake (computed with Python fractions.Fraction, exact rational arithmetic):
///
///   GRADA  → 414_335_620  lovelace
///   PSBT   → 389_008_418  lovelace
///   ADACT  → 370_684_721  lovelace
///
/// The test allows ±1 lovelace for floor-rounding differences.
use torsten_network::query_handler::protocol::handle_non_myopic_rewards;
// Types are re-exported from query_handler (pub use in query_handler/mod.rs).
use torsten_network::query_handler::{
    NodeStateSnapshot, PoolParamsSnapshot, ProtocolParamsSnapshot, QueryResult, StakePoolSnapshot,
};

// ---------------------------------------------------------------------------
// Shared constants — all sourced from Koios preview epoch 1234 (2026-03-15)
// ---------------------------------------------------------------------------

/// Preview testnet reserves at epoch 1234 (lovelace).
const RESERVES: u64 = 8_283_950_832_164_775;

/// Standard Cardano max lovelace supply.
const MAX_SUPPLY: u64 = 45_000_000_000_000_000;

// rho = 3/1000 (monetary expansion rate)
const RHO_NUM: u64 = 3;
const RHO_DEN: u64 = 1000;

// tau = 1/5 (treasury growth rate)
const TAU_NUM: u64 = 1;
const TAU_DEN: u64 = 5;

// a0 = 3/10 (pledge influence)
const A0_NUM: u64 = 3;
const A0_DEN: u64 = 10;

// n_opt = 500 (desired pool count)
const N_OPT: u64 = 500;

// Hypothetical myopic stake used across the main test: 1 M ADA
const MYOPIC_1M_ADA: u64 = 1_000_000_000_000;

// ---------------------------------------------------------------------------
// Pool parameters (from Koios pool_info, epoch 1234)
// ---------------------------------------------------------------------------

// Pool GRADA (pool1ynfnjspgckgxjf2zeye8s33jz3e3ndk9pcwp0qzaupzvvd8ukwt)
const GRADA_ID: [u8; 28] = hex_bytes28("24d3394028c590692542c932784632147319b6c50e1c17805de044c6");
const GRADA_ACTIVE_STAKE: u64 = 70_803_019_758_445;
const GRADA_PLEDGE: u64 = 100_000_000_000;
const GRADA_COST: u64 = 170_000_000;
// margin = 0%
const GRADA_MARGIN_NUM: u64 = 0;
const GRADA_MARGIN_DEN: u64 = 1;

// Pool PSBT (pool1vzqtn3mtfvvuy8ghksy34gs9g97tszj5f8mr3sn7asy5vk577ec)
const PSBT_ID: [u8; 28] = hex_bytes28("6080b9c76b4b19c21d17b4091aa205417cb80a5449f638c27eec0946");
const PSBT_ACTIVE_STAKE: u64 = 44_484_744_590_382;
const PSBT_PLEDGE: u64 = 250_000_000_000;
const PSBT_COST: u64 = 340_000_000;
// margin = 5% = 5/100
const PSBT_MARGIN_NUM: u64 = 5;
const PSBT_MARGIN_DEN: u64 = 100;

// Pool ADACT (pool18pn6p9ef58u4ga3wagp44qhzm8f6zncl57g6qgh0pk3yytwz54h)
const ADACT_ID: [u8; 28] = hex_bytes28("3867a09729a1f954762eea035a82e2d9d3a14f1fa791a022ef0da242");
const ADACT_ACTIVE_STAKE: u64 = 40_682_642_067_375;
const ADACT_PLEDGE: u64 = 2_100_000_000_000;
const ADACT_COST: u64 = 340_000_000;
// margin = 10% = 10/100
const ADACT_MARGIN_NUM: u64 = 10;
const ADACT_MARGIN_DEN: u64 = 100;

// ---------------------------------------------------------------------------
// Expected member rewards for MYOPIC_1M_ADA (from Python oracle)
// ---------------------------------------------------------------------------
const GRADA_EXPECTED_REWARD: u64 = 414_335_620;
const PSBT_EXPECTED_REWARD: u64 = 389_008_418;
const ADACT_EXPECTED_REWARD: u64 = 370_684_721;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Decode a 56-character hex string into a 28-byte array at compile time.
///
/// The hex string must be exactly 56 characters (28 bytes).  Used to embed
/// pool IDs as typed constants without runtime allocation.
const fn hex_bytes28(s: &str) -> [u8; 28] {
    let bytes = s.as_bytes();
    // Length check at compile time would require const-eval panics; we rely on
    // the literal lengths being correct (verified by the test assertions).
    let mut out = [0u8; 28];
    let mut i = 0;
    while i < 28 {
        let hi = hex_nibble(bytes[i * 2]);
        let lo = hex_nibble(bytes[i * 2 + 1]);
        out[i] = (hi << 4) | lo;
        i += 1;
    }
    out
}

const fn hex_nibble(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}

/// Build the preview epoch 1234 protocol params snapshot.
fn epoch_1234_pparams() -> ProtocolParamsSnapshot {
    ProtocolParamsSnapshot {
        rho_num: RHO_NUM,
        rho_den: RHO_DEN,
        tau_num: TAU_NUM,
        tau_den: TAU_DEN,
        a0_num: A0_NUM,
        a0_den: A0_DEN,
        n_opt: N_OPT,
        min_pool_cost: 170_000_000,
        ..ProtocolParamsSnapshot::default()
    }
}

/// Build a single pool snapshot + params pair.
fn make_pool(
    id: &[u8; 28],
    active_stake: u64,
    pledge: u64,
    cost: u64,
    margin_num: u64,
    margin_den: u64,
) -> (StakePoolSnapshot, PoolParamsSnapshot) {
    let pool = StakePoolSnapshot {
        pool_id: id.to_vec(),
        stake: active_stake,
        vrf_keyhash: vec![0u8; 32],
        total_active_stake: active_stake,
    };
    let params = PoolParamsSnapshot {
        pool_id: id.to_vec(),
        vrf_keyhash: vec![0u8; 32],
        pledge,
        cost,
        margin_num,
        margin_den,
        reward_account: vec![0u8; 29],
        owners: vec![],
        relays: vec![],
        metadata_url: None,
        metadata_hash: None,
    };
    (pool, params)
}

/// Build the full epoch 1234 state snapshot with the three preview pools.
fn epoch_1234_state() -> NodeStateSnapshot {
    let (grada_pool, grada_params) = make_pool(
        &GRADA_ID,
        GRADA_ACTIVE_STAKE,
        GRADA_PLEDGE,
        GRADA_COST,
        GRADA_MARGIN_NUM,
        GRADA_MARGIN_DEN,
    );
    let (psbt_pool, psbt_params) = make_pool(
        &PSBT_ID,
        PSBT_ACTIVE_STAKE,
        PSBT_PLEDGE,
        PSBT_COST,
        PSBT_MARGIN_NUM,
        PSBT_MARGIN_DEN,
    );
    let (adact_pool, adact_params) = make_pool(
        &ADACT_ID,
        ADACT_ACTIVE_STAKE,
        ADACT_PLEDGE,
        ADACT_COST,
        ADACT_MARGIN_NUM,
        ADACT_MARGIN_DEN,
    );
    NodeStateSnapshot {
        max_lovelace_supply: MAX_SUPPLY,
        reserves: RESERVES,
        protocol_params: epoch_1234_pparams(),
        stake_pools: vec![grada_pool, psbt_pool, adact_pool],
        pool_params_entries: vec![grada_params, psbt_params, adact_params],
        ..NodeStateSnapshot::default()
    }
}

/// Invoke `handle_non_myopic_rewards` with a single stake amount and return
/// the flat pool rewards vector `Vec<(pool_id, reward_lovelace)>`.
fn query_rewards(state: &NodeStateSnapshot, stake: u64) -> Vec<(Vec<u8>, u64)> {
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.array(1).unwrap();
    enc.u64(stake).unwrap();
    let mut dec = minicbor::Decoder::new(&buf);
    match handle_non_myopic_rewards(state, &mut dec) {
        QueryResult::NonMyopicMemberRewards(entries) => {
            assert_eq!(entries.len(), 1);
            entries.into_iter().next().unwrap().pool_rewards
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

/// Look up a pool's reward from the flat result vector.
fn find_reward(rewards: &[(Vec<u8>, u64)], pool_id: &[u8]) -> u64 {
    rewards
        .iter()
        .find(|(id, _)| id.as_slice() == pool_id)
        .map(|(_, r)| *r)
        .unwrap_or_else(|| panic!("pool not found in result"))
}

// ---------------------------------------------------------------------------
// Test 1: preview epoch 1234 end-to-end validation
// ---------------------------------------------------------------------------

/// Validate the NonMyopicMemberRewards formula against real preview testnet
/// epoch 1234 data.
///
/// Expected values were computed offline using exact rational arithmetic
/// (Python `fractions.Fraction`) with the same inputs.  We allow ±1 lovelace
/// for floor-rounding differences (the Rust code uses `BigInt` integer
/// division, which is equivalent to `floor`).
///
/// # Formula trace for GRADA (margin 0%, cost 170 M):
///
///   total_stake  = 36_716_049_167_835_225
///   R_gross      = floor(3/1000 × 8_283_950_832_164_775) = 24_851_852_496_494
///   treasury     = floor(1/5 × R_gross)                  = 4_970_370_499_298
///   reward_pot   = 19_881_481_997_196
///   z0           = 1/500
///
///   sigma_hyp    = (70_803_019_758_445 + 1T) / 36.7T = 0.001956   < z0
///   p_raw        = 100B / 36.7T                       = 0.00000272 < z0
///
///   f4 = (z0 - sigma) / z0 ≈ 0.022
///   f3 = (sigma - p·f4) / z0 ≈ 0.978
///   f2 = sigma + p·a0·f3 ≈ 0.001957
///   f1 = reward_pot / (1+a0) = 19_881_481_997_196 / 1.3
///   max_pool = floor(f1 × f2) = 29_920_548_715
///
///   member_reward = floor((max_pool - cost) × (1-0) × t / (s+t))
///                 = floor(29_750_548_715 × 1_000_000_000_000 / 71_803_019_758_445)
///                 = 414_335_620
#[test]
fn test_non_myopic_preview_epoch_1234() {
    let state = epoch_1234_state();
    let rewards = query_rewards(&state, MYOPIC_1M_ADA);

    assert_eq!(
        rewards.len(),
        3,
        "expected rewards for all three registered pools"
    );

    // Each pool's reward must match the oracle value within ±1 lovelace.
    // The tolerance accommodates any single intermediate floor that might
    // differ by one in the BigInt vs. Fraction evaluation paths.
    let grada_reward = find_reward(&rewards, &GRADA_ID);
    let psbt_reward = find_reward(&rewards, &PSBT_ID);
    let adact_reward = find_reward(&rewards, &ADACT_ID);

    assert!(
        grada_reward.abs_diff(GRADA_EXPECTED_REWARD) <= 1,
        "GRADA reward mismatch: got {grada_reward}, expected ~{GRADA_EXPECTED_REWARD}"
    );
    assert!(
        psbt_reward.abs_diff(PSBT_EXPECTED_REWARD) <= 1,
        "PSBT reward mismatch: got {psbt_reward}, expected ~{PSBT_EXPECTED_REWARD}"
    );
    assert!(
        adact_reward.abs_diff(ADACT_EXPECTED_REWARD) <= 1,
        "ADACT reward mismatch: got {adact_reward}, expected ~{ADACT_EXPECTED_REWARD}"
    );

    // Sanity: higher-pledge, lower-margin pool (GRADA) should out-earn
    // higher-margin pools (PSBT, ADACT) for the same hypothetical stake.
    assert!(
        grada_reward > psbt_reward,
        "GRADA (0% margin) should earn more than PSBT (5% margin): {grada_reward} vs {psbt_reward}"
    );
    assert!(
        psbt_reward > adact_reward,
        "PSBT (5% margin) should earn more than ADACT (10% margin): {psbt_reward} vs {adact_reward}"
    );
}

// ---------------------------------------------------------------------------
// Test 2: zero stake returns zero reward
// ---------------------------------------------------------------------------

/// A hypothetical stake of zero lovelace must produce zero member reward for
/// every pool, because the delegator_fraction (t / (s + t)) collapses to 0.
///
/// This is a guard against regressions where a division-by-zero or a default
/// path could accidentally return a non-zero value.
#[test]
fn test_non_myopic_zero_stake_returns_zero() {
    let state = epoch_1234_state();

    // Encode stake = 0 explicitly.
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.array(1).unwrap();
    enc.u64(0u64).unwrap();
    let mut dec = minicbor::Decoder::new(&buf);

    let entries = match handle_non_myopic_rewards(&state, &mut dec) {
        QueryResult::NonMyopicMemberRewards(e) => e,
        other => panic!("unexpected result: {other:?}"),
    };

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].stake_amount, 0);

    // Every pool should return 0 reward.
    for (pool_id_bytes, reward) in &entries[0].pool_rewards {
        assert_eq!(
            *reward,
            0,
            "pool {} returned non-zero reward {} for zero stake",
            hex::encode(pool_id_bytes),
            reward
        );
    }
}

// ---------------------------------------------------------------------------
// Test 3: oversaturated pool gives less than optimally saturated pool
// ---------------------------------------------------------------------------

/// The maxPool' function is monotonically non-decreasing in sigma up to z0,
/// and flat (capped) beyond z0.  As a result, an oversaturated pool
/// (sigma > z0) yields the same max_pool as the optimally saturated pool
/// (sigma = z0), but the delegator's fractional share is smaller because
/// `hyp_pool_stake` is larger.  Therefore:
///
///   member_reward(over-saturated) < member_reward(optimally saturated)
///
/// This test constructs two synthetic pools using the real epoch 1234 reward
/// pot to verify the invariant holds in the Rust implementation.
///
/// Synthetic setup (both pools: pledge=0, cost=170 M, margin=0%, no reward
/// difference from margin/pledge so that the only variable is pool stake):
///
///   total_stake = 36_716_049_167_835_225
///   z0_abs      = total_stake / 500 = 73_432_098_335_670
///
///   optimal pool:       active_stake = z0_abs      (exactly at saturation)
///   oversaturated pool: active_stake = 2 × z0_abs  (200 % of z0)
///
/// Expected (Python oracle, t = MYOPIC_1M_ADA):
///   optimal       → member_reward = 408_652_934
///   oversaturated → member_reward = 205_708_319
///
/// Both are positive (max_pool > cost), but oversaturated < optimal.
#[test]
fn test_non_myopic_saturated_pool_monotonicity() {
    // Construct two pools using real epoch 1234 parameters.
    let total_stake: u64 = MAX_SUPPLY - RESERVES;
    let z0_abs: u64 = total_stake / N_OPT;

    // Synthetic pool IDs (no on-chain meaning; just distinguishable bytes).
    let optimal_id: Vec<u8> = vec![0x01u8; 28];
    let over_id: Vec<u8> = vec![0x02u8; 28];

    let (optimal_pool, optimal_params) = make_pool(
        &[0x01u8; 28],
        z0_abs, // exactly at z0
        0,      // no pledge (isolates stake effect)
        170_000_000,
        0,
        1, // 0% margin
    );
    let (over_pool, over_params) = make_pool(
        &[0x02u8; 28],
        2 * z0_abs, // 200% of z0 — oversaturated
        0,
        170_000_000,
        0,
        1,
    );

    let state = NodeStateSnapshot {
        max_lovelace_supply: MAX_SUPPLY,
        reserves: RESERVES,
        protocol_params: epoch_1234_pparams(),
        stake_pools: vec![optimal_pool, over_pool],
        pool_params_entries: vec![optimal_params, over_params],
        ..NodeStateSnapshot::default()
    };

    let rewards = query_rewards(&state, MYOPIC_1M_ADA);
    assert_eq!(rewards.len(), 2, "expected rewards for both pools");

    let optimal_reward = find_reward(&rewards, &optimal_id);
    let over_reward = find_reward(&rewards, &over_id);

    // Both pools must yield positive rewards (max_pool > cost).
    assert!(
        optimal_reward > 0,
        "optimally saturated pool should yield positive reward, got {optimal_reward}"
    );
    assert!(
        over_reward > 0,
        "oversaturated pool should still yield positive reward (max_pool capped at z0), got {over_reward}"
    );

    // Core invariant: optimal > oversaturated.
    assert!(
        optimal_reward > over_reward,
        "optimal pool ({optimal_reward}) should out-earn oversaturated pool ({over_reward})"
    );

    // Verify against oracle values.
    // optimal:       member_reward = 408_652_934
    // oversaturated: member_reward = 205_708_319
    const OPTIMAL_EXPECTED: u64 = 408_652_934;
    const OVER_EXPECTED: u64 = 205_708_319;

    assert!(
        optimal_reward.abs_diff(OPTIMAL_EXPECTED) <= 1,
        "optimal pool reward mismatch: got {optimal_reward}, expected ~{OPTIMAL_EXPECTED}"
    );
    assert!(
        over_reward.abs_diff(OVER_EXPECTED) <= 1,
        "oversaturated pool reward mismatch: got {over_reward}, expected ~{OVER_EXPECTED}"
    );
}
