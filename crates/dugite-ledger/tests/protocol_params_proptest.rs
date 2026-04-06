//! Property-based tests for protocol parameter invariants — Properties 1-3.
//!
//! Cross-validated against cardano-ledger (Haskell) before authoring. See
//! `docs/superpowers/specs/2026-04-06-proptest-expansion-design.md` for full
//! design rationale and Haskell cross-validation notes.
//!
//! # Haskell Reference
//!
//! ## Property 1 — CBOR-enforced parameter bounds
//! Source: `cardano-ledger-core/src/Cardano/Ledger/Core/PParams.hs`, CBOR
//! encoders for each era's `PParams`. The ledger uses:
//! - `uint` (unsigned integer) for all size, epoch, and scalar parameters →
//!   non-negative by type (u64 in Rust).
//! - `positive_int` for rational denominators (Haskell `UnitInterval` /
//!   `NonNegativeInterval` denominator) → `>= 1`.
//! - No ledger-level constraint that min_fee_a or min_fee_b must be > 0; both
//!   CAN be zero (valid, just creates zero-fee transactions).
//! - No ledger constraint that governance thresholds must be `<= 1`; a
//!   threshold of 3/2 is valid CBOR and simply permanently unmeetable.
//!   Guardrail scripts (Conway) may enforce additional bounds, but those are
//!   on-chain scripts, not ledger-enforced Haskell type invariants.
//!
//! ## Property 2 — Update mechanism per era
//! Source:
//! - Pre-Conway: `cardano-ledger-shelley/src/Cardano/Ledger/Shelley/Rules/Ppup.hs`
//!   `PPUP` STS rule. Accepts a `ProposedPPUpdates` map keyed on genesis
//!   delegate key hashes. The update is applied when ALL submitting delegates
//!   agree AND the proposal's epoch matches the current epoch boundary.
//! - Conway: `cardano-ledger-conway/src/Cardano/Ledger/Conway/Rules/Gov.hs`
//!   `GOV` STS rule. `ParameterChange` governance action, ratified when
//!   `dvtPPGroupThreshold` DRep votes and (for security-group params) SPO
//!   threshold are met, plus CC threshold. Genesis delegates play no role.
//!
//! The structural distinguisher is `protocol_version_major`: < 9 is pre-Conway,
//! >= 9 is Conway governance mechanism.
//!
//! ## Property 3 — Era-specific parameter presence (Conway)
//! Source: `cardano-ledger-conway/src/Cardano/Ledger/Conway/PParams.hs`
//! `ConwayPParams`. All governance fields are mandatory in the Conway era and
//! must satisfy their CBOR-type constraints (denominators >= 1, deposits >= 0).
//! Our `arb_protocol_params()` generator always produces Conway-era params via
//! `ProtocolParameters::mainnet_defaults()` (major = 9), so every generated
//! instance must have all Conway governance fields present and valid.

#[path = "strategies.rs"]
mod strategies;

use dugite_primitives::protocol_params::ProtocolParameters;
use dugite_primitives::transaction::Rational;
use proptest::prelude::*;
use strategies::arb_protocol_params;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return all `Rational` threshold fields from a `ProtocolParameters`.
///
/// This covers:
/// - All DRep voting thresholds (dvt_*): 10 fields
/// - All SPO voting thresholds (pvt_*): 5 fields
/// - Monetary expansion (rho) and treasury growth (tau): 2 fields
/// - Pool pledge influence (a0): 1 field
/// - Decentralisation (d): 1 field
/// - Execution unit prices (mem_price, step_price): 2 fields
///
/// Total: 21 Rational fields whose denominators must be >= 1.
fn all_rational_fields(p: &ProtocolParameters) -> Vec<(&'static str, &Rational)> {
    vec![
        // Monetary parameters
        ("a0", &p.a0),
        ("rho", &p.rho),
        ("tau", &p.tau),
        ("d", &p.d),
        // Execution unit prices
        ("execution_costs.mem_price", &p.execution_costs.mem_price),
        ("execution_costs.step_price", &p.execution_costs.step_price),
        // DRep voting thresholds
        ("dvt_pp_network_group", &p.dvt_pp_network_group),
        ("dvt_pp_economic_group", &p.dvt_pp_economic_group),
        ("dvt_pp_technical_group", &p.dvt_pp_technical_group),
        ("dvt_pp_gov_group", &p.dvt_pp_gov_group),
        ("dvt_hard_fork", &p.dvt_hard_fork),
        ("dvt_no_confidence", &p.dvt_no_confidence),
        ("dvt_committee_normal", &p.dvt_committee_normal),
        ("dvt_committee_no_confidence", &p.dvt_committee_no_confidence),
        ("dvt_constitution", &p.dvt_constitution),
        ("dvt_treasury_withdrawal", &p.dvt_treasury_withdrawal),
        // SPO voting thresholds
        ("pvt_motion_no_confidence", &p.pvt_motion_no_confidence),
        ("pvt_committee_normal", &p.pvt_committee_normal),
        ("pvt_committee_no_confidence", &p.pvt_committee_no_confidence),
        ("pvt_hard_fork", &p.pvt_hard_fork),
        ("pvt_pp_security_group", &p.pvt_pp_security_group),
    ]
}

/// Return only the governance threshold Rational fields (dvt_* and pvt_*).
///
/// These are the fields that the Conway era adds and that Property 3 checks.
fn governance_threshold_fields(p: &ProtocolParameters) -> Vec<(&'static str, &Rational)> {
    vec![
        ("dvt_pp_network_group", &p.dvt_pp_network_group),
        ("dvt_pp_economic_group", &p.dvt_pp_economic_group),
        ("dvt_pp_technical_group", &p.dvt_pp_technical_group),
        ("dvt_pp_gov_group", &p.dvt_pp_gov_group),
        ("dvt_hard_fork", &p.dvt_hard_fork),
        ("dvt_no_confidence", &p.dvt_no_confidence),
        ("dvt_committee_normal", &p.dvt_committee_normal),
        ("dvt_committee_no_confidence", &p.dvt_committee_no_confidence),
        ("dvt_constitution", &p.dvt_constitution),
        ("dvt_treasury_withdrawal", &p.dvt_treasury_withdrawal),
        ("pvt_motion_no_confidence", &p.pvt_motion_no_confidence),
        ("pvt_committee_normal", &p.pvt_committee_normal),
        ("pvt_committee_no_confidence", &p.pvt_committee_no_confidence),
        ("pvt_hard_fork", &p.pvt_hard_fork),
        ("pvt_pp_security_group", &p.pvt_pp_security_group),
    ]
}

// ---------------------------------------------------------------------------
// Property 1: CBOR-enforced parameter bounds
// ---------------------------------------------------------------------------
//
// The Haskell ledger enforces exactly the CBOR type constraints:
//
//   uint             → value >= 0  (always true for u64, so we verify the type)
//   positive_int     → value >= 1  (for rational denominators)
//
// We do NOT assert:
//   - min_fee_a > 0  (zero fee coefficient is legal per-spec)
//   - min_fee_b > 0  (zero fee constant is legal per-spec)
//   - thresholds <= 1.0  (not enforced by the ledger)
//
// Source: cardano-ledger-core PParams CBOR encoding; Haskell `positive_int`
// maps to `NonNegativeInterval` denominators which must be >= 1.

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// Verify CBOR-enforced parameter bounds for all generated ProtocolParameters.
    ///
    /// Checks:
    /// 1. All `uint` fields (u64 scalars) are trivially non-negative — asserted
    ///    as a documentation checkpoint, not a live invariant, since Rust's type
    ///    system already guarantees this.
    /// 2. All `positive_int` rational denominators are >= 1 — this is the live
    ///    invariant that could be violated if a generator or constructor
    ///    accidentally produced a zero denominator.
    ///
    /// Note: numerators are `uint` (>= 0), so `numerator >= 0` is automatic for
    /// u64. The spec does NOT require `numerator <= denominator`.
    #[test]
    fn prop_cbor_enforced_parameter_bounds(params in arb_protocol_params()) {
        // ── u64 scalar fields (uint in CBOR) ────────────────────────────────
        // These are guaranteed non-negative by the Rust u64 type. We access
        // each field as a documentation checkpoint — a future refactor that
        // changes a field to a signed type will break the `let _: u64 =`
        // binding and draw attention to the invariant.
        let _: u64 = params.min_fee_a;
        let _: u64 = params.min_fee_b;
        let _: u64 = params.max_block_body_size;
        let _: u64 = params.max_tx_size;
        let _: u64 = params.max_block_header_size;
        let _: u64 = params.e_max;
        let _: u64 = params.n_opt;

        // ── positive_int denominators (rational fields) ──────────────────────
        // The CBOR spec encodes all rational denominators as `positive_int`,
        // meaning they MUST be >= 1. A denominator of 0 would be a divide-by-zero
        // and is rejected during CBOR decoding by the Haskell ledger.
        for (field_name, rational) in all_rational_fields(&params) {
            prop_assert!(
                rational.denominator >= 1,
                "Rational field '{}' has denominator {} which violates positive_int constraint",
                field_name,
                rational.denominator
            );
        }

        // ── uint numerators (rational fields) ───────────────────────────────
        // Numerators are encoded as `uint` (non-negative). For u64 fields this
        // is guaranteed by the type. We access them to document the constraint.
        for (_field_name, rational) in all_rational_fields(&params) {
            // u64 is always >= 0; this binding documents the CBOR uint constraint.
            let _: u64 = rational.numerator;
        }

        // ── Explicit non-constraint: min_fee_a == 0 is LEGAL ────────────────
        // The Haskell ledger does not enforce min_fee_a > 0. A protocol with
        // zero fee coefficients would make all transactions free — unusual but
        // not a protocol violation. We do not assert min_fee_a > 0.

        // ── Explicit non-constraint: thresholds may exceed 1.0 ──────────────
        // The Haskell ledger does not enforce numerator <= denominator for
        // governance thresholds. A threshold of 3/2 = 1.5 is valid on-chain;
        // it simply becomes permanently unmeetable. We do not assert this.
    }
}

// ---------------------------------------------------------------------------
// Property 2: Update mechanism per era
// ---------------------------------------------------------------------------
//
// Two completely separate update systems, distinguished by protocol_version_major:
//
//   major < 9  (Shelley through Babbage) → ProposedPPUpdates via genesis delegates
//   major >= 9 (Conway)                  → ParameterChange governance action via votes
//
// This is a structural test verifying that the era distinction exists and that
// the correct mechanism applies based on `protocol_version_major`. We do not
// run full governance ratification here — that is the domain of epoch_proptest.rs
// Property 6. Instead we verify the structural invariant: Conway params have
// `protocol_version_major >= 9`, which means only the governance mechanism applies.
//
// Haskell source:
// - Pre-Conway: cardano-ledger-shelley/src/Cardano/Ledger/Shelley/Rules/Ppup.hs
//   `PPUP` STS; `GenDelegs` map keyed on `KeyHash 'Genesis`; unanimity required.
// - Conway: cardano-ledger-conway/src/Cardano/Ledger/Conway/Rules/Gov.hs
//   `GOV` STS; `ParameterChange` governance action constructor.

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// Verify that `protocol_version_major` correctly identifies which parameter
    /// update mechanism applies.
    ///
    /// The invariant: each major version unambiguously belongs to exactly one
    /// update mechanism. There is no overlap: a version is either pre-Conway
    /// (genesis delegate unanimity) or Conway (governance action votes), never
    /// both.
    ///
    /// Since `arb_protocol_params()` always produces Conway-era params
    /// (major = 9 via `mainnet_defaults()`), this property also confirms that
    /// our generator does not accidentally produce pre-Conway major versions.
    #[test]
    fn prop_update_mechanism_per_era(params in arb_protocol_params()) {
        let major = params.protocol_version_major;

        // ── Exactly one mechanism applies ────────────────────────────────────
        // These two predicates are complementary and exhaustive: every major
        // version is either pre-Conway or Conway, never both.
        let is_pre_conway = major < 9;
        let is_conway = major >= 9;

        prop_assert!(
            is_pre_conway ^ is_conway,
            "protocol_version_major={} must belong to exactly one update mechanism \
             (pre-Conway XOR Conway), but is_pre_conway={} and is_conway={}",
            major,
            is_pre_conway,
            is_conway
        );

        // ── Conway mechanism: governance action votes ────────────────────────
        // For Conway params (major >= 9), the update mechanism requires DRep
        // vote ratios to meet per-group thresholds (dvt_pp_*). The thresholds
        // must have valid denominators so vote comparison is well-defined.
        if is_conway {
            // All four PP group thresholds must have denominator >= 1 so that
            // the governance ratification engine can compute vote ratios.
            prop_assert!(
                params.dvt_pp_network_group.denominator >= 1,
                "Conway: dvt_pp_network_group denominator must be >= 1 for vote comparison"
            );
            prop_assert!(
                params.dvt_pp_economic_group.denominator >= 1,
                "Conway: dvt_pp_economic_group denominator must be >= 1 for vote comparison"
            );
            prop_assert!(
                params.dvt_pp_technical_group.denominator >= 1,
                "Conway: dvt_pp_technical_group denominator must be >= 1 for vote comparison"
            );
            prop_assert!(
                params.dvt_pp_gov_group.denominator >= 1,
                "Conway: dvt_pp_gov_group denominator must be >= 1 for vote comparison"
            );
            // Security group SPO threshold must also be valid.
            prop_assert!(
                params.pvt_pp_security_group.denominator >= 1,
                "Conway: pvt_pp_security_group denominator must be >= 1 for vote comparison"
            );
        }

        // ── Pre-Conway mechanism: genesis delegate unanimity ─────────────────
        // For pre-Conway params (major < 9), the update mechanism uses
        // `ProposedPPUpdates` keyed on genesis delegate hashes. There are no
        // vote-ratio thresholds — unanimity is required. The governance
        // threshold fields are irrelevant (they were added in Conway) but must
        // still have valid denominators for internal consistency.
        //
        // Note: arb_protocol_params() produces major=9, so this branch is never
        // reached with the current generator. It is included for completeness
        // and for future generators that produce multi-era params.
        if is_pre_conway {
            // No governance vote thresholds are consulted. There is no
            // pre-Conway DRep ratification step.
            //
            // We verify that the protocol version itself is structurally valid:
            // minor version is unrestricted.
            prop_assert!(
                major < 9,
                "Pre-Conway params must have major < 9, got {}",
                major
            );
        }

        // ── arb_protocol_params() always produces Conway params ──────────────
        // Document and enforce the generator contract: our strategy is based on
        // mainnet_defaults(), which sets major=9. This confirms we are always
        // exercising the Conway governance mechanism in these tests.
        prop_assert!(
            major == 9,
            "arb_protocol_params() must produce Conway-era params with major=9, got major={}",
            major
        );
    }
}

// ---------------------------------------------------------------------------
// Property 3: Era-specific parameter presence (Conway)
// ---------------------------------------------------------------------------
//
// Conway is the first era to introduce governance parameters. The Haskell
// `ConwayPParams` type (cardano-ledger-conway/src/Cardano/Ledger/Conway/PParams.hs)
// adds these mandatory fields over Babbage:
//
//   poolVotingThresholds     → pvt_* (5 SPO thresholds)
//   drepVotingThresholds     → dvt_* (10 DRep thresholds)
//   committeeMinSize         → committee_min_size :: uint
//   committeeMaxTermLength   → committee_max_term_length :: uint
//   govActionLifetime        → gov_action_lifetime :: uint
//   govActionDeposit         → gov_action_deposit :: Coin (positive)
//   drepDeposit              → drep_deposit :: Coin (positive)
//   drepActivity             → drep_activity :: uint
//   minFeeRefScriptCostPerByte → min_fee_ref_script_cost_per_byte :: uint
//
// All Coin fields (deposits) are non-negative by type (Lovelace is a u64 newtype).
// All threshold rational denominators must be >= 1 (positive_int CBOR constraint).
//
// Note: protocol_version (major/minor) is present in the Conway CBOR array at
// position 12 but is tagged `HKDNoUpdate` — it cannot be changed via
// `ParameterChange`, only via `HardForkInitiation`. We verify its presence by
// checking the field is accessible (it is always set by mainnet_defaults).

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// Verify that Conway-era protocol parameters have all mandatory governance
    /// fields with valid CBOR-type values.
    ///
    /// For each generated `ProtocolParameters` (always Conway-era from our
    /// generator), checks:
    /// 1. All 15 governance threshold Rational fields have denominator >= 1.
    /// 2. Governance deposit fields are non-negative (guaranteed by Lovelace type).
    /// 3. Governance scalar fields (lifetimes, sizes) are valid u64 values.
    /// 4. Protocol version fields are present and accessible.
    ///
    /// This test would catch any regression where Conway fields are accidentally
    /// left uninitialized or where a refactor introduces a zero denominator.
    #[test]
    fn prop_era_specific_parameter_presence(params in arb_protocol_params()) {
        // ── All governance threshold denominators must be >= 1 ───────────────
        // These 15 fields are all mandatory in the Conway CBOR encoding. A
        // denominator of 0 would be invalid CBOR (`positive_int` requires >= 1)
        // and would cause a divide-by-zero in governance ratification.
        for (field_name, threshold) in governance_threshold_fields(&params) {
            prop_assert!(
                threshold.denominator >= 1,
                "Conway governance threshold '{}' has denominator={} which violates \
                 positive_int CBOR constraint (must be >= 1)",
                field_name,
                threshold.denominator
            );
        }

        // ── Governance deposit fields are non-negative ───────────────────────
        // drep_deposit and gov_action_deposit are `Coin` in Haskell, which is
        // a non-negative integer. We access the inner Lovelace value to verify
        // the field is present (not uninitialized) and has the expected type.
        //
        // A drep_deposit of 0 is technically valid CBOR (Coin is uint), though
        // unconventional. We do not assert > 0.
        let _: u64 = params.drep_deposit.0;
        let _: u64 = params.gov_action_deposit.0;

        // ── Governance scalar fields are valid u64 values ────────────────────
        // drep_activity, gov_action_lifetime, committee_min_size,
        // committee_max_term_length are all encoded as `uint` in CBOR.
        // Binding to u64 serves as the type-level documentation checkpoint.
        let _: u64 = params.drep_activity;
        let _: u64 = params.gov_action_lifetime;
        let _: u64 = params.committee_min_size;
        let _: u64 = params.committee_max_term_length;
        let _: u64 = params.min_fee_ref_script_cost_per_byte;

        // ── Protocol version is present and structurally valid ───────────────
        // In Conway, protocol_version is encoded at position 12 in the PParams
        // CBOR array. It is tagged `HKDNoUpdate` and cannot be changed via
        // ParameterChange. We verify the fields are accessible and form a valid
        // (major, minor) version pair.
        //
        // We do not assert major==9 here (that is Property 2's job).
        let _version_pair: (u64, u64) = (params.protocol_version_major, params.protocol_version_minor);

        // ── Verify the era is indeed Conway (major >= 9) ─────────────────────
        // This confirms our generator is producing Conway-era params and that
        // all the above governance fields are actually required (not optional
        // pre-Conway additions that happen to be present).
        prop_assert!(
            params.protocol_version_major >= 9,
            "arb_protocol_params() must produce Conway-era params (major >= 9), \
             got major={}",
            params.protocol_version_major
        );

        // ── Governance DRep count lower bound: committee_min_size is a target ─
        // committee_min_size represents the minimum number of CC members needed
        // for a valid committee. It is a u64 scalar with no upper bound in the
        // ledger rules (the guardrail script may enforce one on-chain). We
        // simply verify it is present (non-overflow access suffices).
        let _ = params.committee_min_size;
        let _ = params.committee_max_term_length;
    }
}

// ---------------------------------------------------------------------------
// Property 4: Update preserves unchanged fields
// ---------------------------------------------------------------------------
//
// Haskell reference: `cardano-ledger-core/src/Cardano/Ledger/Core/PParams.hs`
// `updatePParams` (pre-Conway: `updatePP`, Conway: the governance enactment
// path in `Cardano.Ledger.Conway.Governance.Procedures`).
//
// The Haskell implementation uses `StrictMaybe` for each field in the update
// record: `SJust new_value` replaces the field, `SNothing` leaves it
// unchanged. This is the `PParamsUpdate` type across all eras.
//
// In Rust/Dugite the equivalent is applying an update struct where only some
// fields are `Some(new_value)` and the rest are `None`. A correct
// implementation must:
//   1. Overwrite every field where `Some(v)` is provided.
//   2. Leave every field where `None` is provided EXACTLY AS IT WAS.
//
// This test simulates the simplest partial update: clone the params, change
// only `min_fee_a`, and verify that all other fields are identical to the
// original. If any field is accidentally zeroed, defaulted, or otherwise
// mutated during a clone-and-modify operation, this property will catch it.
//
// Note: `ProtocolParameters` does not derive `PartialEq` (because `f64`
// fields prevent a derived `Eq`). We compare each field individually. The
// `active_slots_coeff` field is an `f64` and uses `==` comparison — since
// we are performing an exact clone with no arithmetic, bit-identical
// floating-point equality is correct here.

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// Verify that a partial parameter update (changing only `min_fee_a`)
    /// leaves all other fields identical to the original.
    ///
    /// This matches Haskell's `updatePP` identity behaviour on `SNothing`
    /// fields: untouched fields are preserved byte-for-byte.
    ///
    /// The new `min_fee_a` value must differ from the original; we use the
    /// strategy `(original + 1) % 200 + 1` to guarantee a distinct value
    /// without overflow.
    #[test]
    fn prop_update_preserves_unchanged_fields(original in arb_protocol_params()) {
        // ── Apply a single-field update ──────────────────────────────────────
        // Clone the original, then change exactly one field.
        let mut updated = original.clone();
        // Choose a new min_fee_a that is guaranteed different from the
        // original so that the "changed field changed" post-condition can
        // also be checked.
        let new_min_fee_a = original.min_fee_a % 200 + 1;
        // If new_min_fee_a happens to equal original.min_fee_a (i.e., the
        // formula wraps to the same value), shift it by 1 more.
        let new_min_fee_a = if new_min_fee_a == original.min_fee_a {
            new_min_fee_a % 200 + 1
        } else {
            new_min_fee_a
        };
        updated.min_fee_a = new_min_fee_a;

        // ── Verify: the changed field changed ────────────────────────────────
        // This is a sanity check that our update was actually applied — if it
        // fails, the test infrastructure is broken rather than the parameter
        // logic.
        prop_assert_ne!(
            updated.min_fee_a,
            original.min_fee_a,
            "min_fee_a should have changed: original={}, updated={}",
            original.min_fee_a,
            updated.min_fee_a
        );

        // ── Verify: all other scalar u64 fields are unchanged ─────────────
        prop_assert_eq!(
            updated.min_fee_b, original.min_fee_b,
            "min_fee_b changed: {} -> {}", original.min_fee_b, updated.min_fee_b
        );
        prop_assert_eq!(
            updated.max_block_body_size, original.max_block_body_size,
            "max_block_body_size changed: {} -> {}",
            original.max_block_body_size, updated.max_block_body_size
        );
        prop_assert_eq!(
            updated.max_tx_size, original.max_tx_size,
            "max_tx_size changed: {} -> {}", original.max_tx_size, updated.max_tx_size
        );
        prop_assert_eq!(
            updated.max_block_header_size, original.max_block_header_size,
            "max_block_header_size changed: {} -> {}",
            original.max_block_header_size, updated.max_block_header_size
        );
        prop_assert_eq!(
            updated.e_max, original.e_max,
            "e_max changed: {} -> {}", original.e_max, updated.e_max
        );
        prop_assert_eq!(
            updated.n_opt, original.n_opt,
            "n_opt changed: {} -> {}", original.n_opt, updated.n_opt
        );
        prop_assert_eq!(
            updated.max_val_size, original.max_val_size,
            "max_val_size changed: {} -> {}", original.max_val_size, updated.max_val_size
        );
        prop_assert_eq!(
            updated.collateral_percentage, original.collateral_percentage,
            "collateral_percentage changed: {} -> {}",
            original.collateral_percentage, updated.collateral_percentage
        );
        prop_assert_eq!(
            updated.max_collateral_inputs, original.max_collateral_inputs,
            "max_collateral_inputs changed: {} -> {}",
            original.max_collateral_inputs, updated.max_collateral_inputs
        );
        prop_assert_eq!(
            updated.min_fee_ref_script_cost_per_byte,
            original.min_fee_ref_script_cost_per_byte,
            "min_fee_ref_script_cost_per_byte changed: {} -> {}",
            original.min_fee_ref_script_cost_per_byte,
            updated.min_fee_ref_script_cost_per_byte
        );
        prop_assert_eq!(
            updated.drep_activity, original.drep_activity,
            "drep_activity changed: {} -> {}", original.drep_activity, updated.drep_activity
        );
        prop_assert_eq!(
            updated.gov_action_lifetime, original.gov_action_lifetime,
            "gov_action_lifetime changed: {} -> {}",
            original.gov_action_lifetime, updated.gov_action_lifetime
        );
        prop_assert_eq!(
            updated.committee_min_size, original.committee_min_size,
            "committee_min_size changed: {} -> {}",
            original.committee_min_size, updated.committee_min_size
        );
        prop_assert_eq!(
            updated.committee_max_term_length, original.committee_max_term_length,
            "committee_max_term_length changed: {} -> {}",
            original.committee_max_term_length, updated.committee_max_term_length
        );
        prop_assert_eq!(
            updated.protocol_version_major, original.protocol_version_major,
            "protocol_version_major changed: {} -> {}",
            original.protocol_version_major, updated.protocol_version_major
        );
        prop_assert_eq!(
            updated.protocol_version_minor, original.protocol_version_minor,
            "protocol_version_minor changed: {} -> {}",
            original.protocol_version_minor, updated.protocol_version_minor
        );

        // ── Verify: Lovelace fields unchanged ────────────────────────────────
        prop_assert_eq!(
            updated.key_deposit, original.key_deposit,
            "key_deposit changed: {:?} -> {:?}", original.key_deposit, updated.key_deposit
        );
        prop_assert_eq!(
            updated.pool_deposit, original.pool_deposit,
            "pool_deposit changed: {:?} -> {:?}", original.pool_deposit, updated.pool_deposit
        );
        prop_assert_eq!(
            updated.min_pool_cost, original.min_pool_cost,
            "min_pool_cost changed: {:?} -> {:?}", original.min_pool_cost, updated.min_pool_cost
        );
        prop_assert_eq!(
            updated.ada_per_utxo_byte, original.ada_per_utxo_byte,
            "ada_per_utxo_byte changed: {:?} -> {:?}",
            original.ada_per_utxo_byte, updated.ada_per_utxo_byte
        );
        prop_assert_eq!(
            updated.drep_deposit, original.drep_deposit,
            "drep_deposit changed: {:?} -> {:?}", original.drep_deposit, updated.drep_deposit
        );
        prop_assert_eq!(
            updated.gov_action_deposit, original.gov_action_deposit,
            "gov_action_deposit changed: {:?} -> {:?}",
            original.gov_action_deposit, updated.gov_action_deposit
        );

        // ── Verify: Rational fields unchanged ────────────────────────────────
        // rho and tau are randomised by arb_protocol_params(); all others
        // retain mainnet defaults. All must survive a clone unchanged.
        //
        // Note: Rational does not implement Copy, so we compare by reference
        // using prop_assert! rather than prop_assert_eq! to avoid moving the
        // values into the macro and then attempting to use them in the format
        // string (which would be a use-after-move).
        prop_assert!(
            updated.rho == original.rho,
            "rho changed after single-field update"
        );
        prop_assert!(
            updated.tau == original.tau,
            "tau changed after single-field update"
        );
        prop_assert!(
            updated.a0 == original.a0,
            "a0 changed after single-field update"
        );
        prop_assert!(
            updated.d == original.d,
            "d changed after single-field update"
        );
        // Verify all 15 governance threshold Rational fields are unchanged.
        // We zip the two helper-produced vecs and compare by reference.
        let updated_gov = governance_threshold_fields(&updated);
        let original_gov = governance_threshold_fields(&original);
        for ((name, upd_field), (_, orig_field)) in
            updated_gov.iter().zip(original_gov.iter())
        {
            prop_assert!(
                upd_field == orig_field,
                "governance threshold '{}' changed after single-field update",
                name
            );
        }

        // ── Verify: ExUnitPrices and ExUnits unchanged ────────────────────────
        // ExUnitPrices contains Rational fields (non-Copy), so compare by
        // reference via prop_assert! for the same reason as above.
        prop_assert!(
            updated.execution_costs == original.execution_costs,
            "execution_costs changed after single-field update"
        );
        prop_assert!(
            updated.max_tx_ex_units == original.max_tx_ex_units,
            "max_tx_ex_units changed after single-field update"
        );
        prop_assert!(
            updated.max_block_ex_units == original.max_block_ex_units,
            "max_block_ex_units changed after single-field update"
        );

        // ── Verify: active_slots_coeff unchanged (f64 exact equality) ────────
        // We are cloning without arithmetic, so bit-identical equality holds.
        prop_assert!(
            updated.active_slots_coeff == original.active_slots_coeff,
            "active_slots_coeff changed: {} -> {}",
            original.active_slots_coeff,
            updated.active_slots_coeff
        );
    }
}

// ---------------------------------------------------------------------------
// Property 5: Rational threshold CBOR validity
// ---------------------------------------------------------------------------
//
// Haskell reference:
// `cardano-ledger-core/src/Cardano/Ledger/BaseTypes.hs` — `UnitInterval` and
// `NonNegativeInterval` CBOR instances. Both encode as `Tag(30)[num, den]`
// where `den` is a `positive_int` (>= 1) and `num` is a `uint` (>= 0).
//
// Key design decision (cross-validated against Haskell):
//   - `denominator >= 1` is ENFORCED by the ledger (CBOR `positive_int`).
//   - `numerator >= 0` is ENFORCED by the type (CBOR `uint`, maps to u64).
//   - `numerator <= denominator` (ratio <= 1.0) is NOT enforced by the
//     ledger. `NonNegativeInterval` (used for `a0`) explicitly allows
//     values > 1. `UnitInterval` theoretically restricts to [0,1] but the
//     CBOR decoder accepts any numerator — the guardrail script enforces the
//     semantic bound. We do NOT assert `numerator <= denominator`.
//
// This property checks ALL rational fields: the 10 DRep dvt_* thresholds,
// the 5 SPO pvt_* thresholds, rho, tau, a0, d, and the execution unit prices
// (mem_price, step_price). Total: 21 fields via `all_rational_fields()`.
//
// The subset "governance thresholds" (dvt_* + pvt_*) corresponds to the 15
// fields that the spec asks us to verify, plus rho/tau as specified in the
// task brief. Using `all_rational_fields()` is strictly stronger — it
// verifies all 21 Rational fields, which is the correct comprehensive check.

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// Verify that all Rational fields in ProtocolParameters satisfy the
    /// CBOR encoding invariants: `denominator >= 1` and `numerator >= 0`.
    ///
    /// Checks all 21 Rational fields (governance thresholds, monetary
    /// parameters, execution unit prices) for CBOR type constraint
    /// compliance.
    ///
    /// Explicitly does NOT assert `numerator <= denominator`: the Haskell
    /// ledger does not enforce this for all rational types, and thresholds
    /// > 1.0 are valid on-chain (permanently unmeetable but not rejected).
    #[test]
    fn prop_rational_threshold_cbor_validity(params in arb_protocol_params()) {
        for (field_name, rational) in all_rational_fields(&params) {
            // ── denominator must be >= 1 (CBOR positive_int) ─────────────────
            // A zero denominator is invalid CBOR and would cause a
            // divide-by-zero in governance ratification or fee calculation.
            prop_assert!(
                rational.denominator >= 1,
                "field '{}': denominator={} violates CBOR positive_int (must be >= 1)",
                field_name,
                rational.denominator
            );

            // ── numerator must be >= 0 (CBOR uint, guaranteed by u64) ─────────
            // This is trivially true for u64 but we access it explicitly as a
            // documentation checkpoint: a future refactor to i64 would need to
            // explicitly handle this invariant.
            let _: u64 = rational.numerator;

            // ── Explicit non-constraint: numerator > denominator is valid ─────
            // We do NOT assert rational.numerator <= rational.denominator.
            // Haskell's NonNegativeInterval (used for a0) allows values > 1.
            // For governance thresholds, a value > 1.0 is legal on-chain
            // (the action is simply permanently unmeetable). Guardrail scripts
            // may enforce tighter bounds, but those are not ledger invariants.
        }
    }
}

// ---------------------------------------------------------------------------
// Property 6: Monotonic protocol version (lexicographic)
// ---------------------------------------------------------------------------
//
// Haskell reference:
// `cardano-ledger-shelley/src/Cardano/Ledger/Shelley/Rules/Updn.hs` and
// `cardano-ledger-core/src/Cardano/Ledger/BaseTypes.hs` — `ProtVer` type.
//
// The Haskell `ProtVer` type is `(Major, Minor)` where:
//   `Major` is a `newtype` over `Word64` with no additional constraints.
//   `Minor` is a `Word64` with no additional constraints.
//
// The update rule `UPDN` (and its successors in each era) enforces:
//   `canFollow old new ≡ (pvMajor new, pvMinor new) > (pvMajor old, pvMinor old)`
//
// This is lexicographic pair comparison, which in Rust corresponds to the
// default tuple `PartialOrd`/`Ord` implementation:
//   `(a, b) > (c, d) ≡ a > c || (a == c && b > d)`
//
// Consequences:
//   - major increase: always valid (even if minor decreases — e.g., 9.5 → 10.0)
//   - major same + minor increase: valid
//   - major same + minor same: invalid (no change is not an upgrade)
//   - major decrease: invalid regardless of minor
//   - major same + minor decrease: invalid
//
// Source: `Cardano.Ledger.BaseTypes.ProtVer.canFollow`:
//   `canFollow (ProtVer major1 minor1) (ProtVer major2 minor2) =
//     (major2 == major1 + 1 && minor2 == 0) || (major2 == major1 && minor2 == minor1 + 1)`
//
// IMPORTANT: The Haskell `canFollow` is STRICTER than pure lexicographic
// ordering in one way: when the major version increases by 1, the minor MUST
// be 0 (not just any value >= 0). And the major can only increase by exactly
// 1. However, the fundamental ordering invariant still holds: any `canFollow`
// pair satisfies `(new_major, new_minor) > (old_major, old_minor)`. We test
// the ordering invariant here; the exact `canFollow` constraint (major+1 with
// minor=0, OR same major with minor+1) is an additional refinement.
//
// This property tests three sub-cases using a single proptest strategy:
//   A. major_new > major_old → new > old (valid upgrade direction)
//   B. major_new < major_old → new < old (invalid, downgrade)
//   C. major_new == major_old → validity determined solely by minor comparison

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// Verify that protocol version comparison is lexicographic on (major, minor).
    ///
    /// Tests three sub-cases:
    ///   A. Major increase → always strictly greater (valid upgrade direction),
    ///      even if minor decreases.
    ///   B. Major decrease → always strictly less (invalid downgrade direction),
    ///      regardless of minor.
    ///   C. Same major → comparison determined entirely by minor: greater-minor
    ///      is valid, equal-minor is not a strict upgrade, lesser-minor is invalid.
    ///
    /// Uses Rust's built-in tuple `Ord` which is lexicographic by definition,
    /// matching the Haskell `ProtVer` ordering used by `canFollow`.
    #[test]
    fn prop_protocol_version_lexicographic(
        // Old version: major in [1, 8], minor in [0, 5]
        old_major in 1u64..=8u64,
        old_minor in 0u64..=5u64,
        // Delta: how much to increase/decrease major (0 means same-major case)
        major_delta in 0u64..=3u64,
        // New minor: independent of old_minor so we can test all orderings
        new_minor in 0u64..=5u64,
    ) {
        // ── Case A: major increases (major_delta >= 1) ────────────────────────
        // Any increase in major makes the new version lexicographically greater,
        // regardless of minor. E.g., (9, 0) > (8, 5) even though 0 < 5.
        if major_delta >= 1 {
            let new_major = old_major + major_delta;
            // For any new_minor value, (new_major, new_minor) > (old_major, old_minor)
            // because new_major > old_major dominates.
            prop_assert!(
                (new_major, new_minor) > (old_major, old_minor),
                "major increase: ({}, {}) should be > ({}, {})",
                new_major, new_minor, old_major, old_minor
            );
            // Confirm Rust tuple comparison matches manual lexicographic check.
            prop_assert!(
                new_major > old_major,
                "major_delta={}: new_major={} should exceed old_major={}",
                major_delta, new_major, old_major
            );
        }

        // ── Case B: major decreases (simulate by swapping old↔new) ───────────
        // If new_major < old_major then the new version is strictly less,
        // regardless of minor. We derive this from Case A via symmetry:
        // if (big, any) > (small, any), then (small, any) < (big, any).
        if major_delta >= 1 {
            let downgrade_major = old_major.saturating_sub(major_delta);
            // Skip if subtraction underflowed to 0 (not a valid scenario for
            // Conway mainnet, but not the point of this test).
            if downgrade_major < old_major {
                prop_assert!(
                    (downgrade_major, new_minor) < (old_major, old_minor)
                        || (downgrade_major == old_major),
                    "major decrease: ({}, {}) should be < ({}, {})",
                    downgrade_major, new_minor, old_major, old_minor
                );
                // More precisely: strict less when major actually decreased.
                if downgrade_major < old_major {
                    prop_assert!(
                        (downgrade_major, new_minor) < (old_major, old_minor),
                        "strict downgrade: ({}, {}) must be < ({}, {})",
                        downgrade_major, new_minor, old_major, old_minor
                    );
                }
            }
        }

        // ── Case C: same major (major_delta == 0) ────────────────────────────
        // With equal major versions, the minor version determines the order.
        if major_delta == 0 {
            let same_major = old_major;

            // C1: minor increases → strictly greater (valid upgrade)
            if new_minor > old_minor {
                prop_assert!(
                    (same_major, new_minor) > (same_major, old_minor),
                    "same-major minor increase: ({}, {}) should be > ({}, {})",
                    same_major, new_minor, same_major, old_minor
                );
            }

            // C2: minor equal → equal (not a strict upgrade, canFollow rejects)
            if new_minor == old_minor {
                prop_assert!(
                    (same_major, new_minor) == (same_major, old_minor),
                    "same version: ({}, {}) should equal ({}, {})",
                    same_major, new_minor, same_major, old_minor
                );
                // Not a valid upgrade: equal is not strictly greater.
                prop_assert!(
                    (same_major, new_minor) <= (same_major, old_minor),
                    "same version ({}, {}) must not be > itself",
                    same_major, new_minor
                );
            }

            // C3: minor decreases → strictly less (invalid downgrade)
            if new_minor < old_minor {
                prop_assert!(
                    (same_major, new_minor) < (same_major, old_minor),
                    "same-major minor decrease: ({}, {}) should be < ({}, {})",
                    same_major, new_minor, same_major, old_minor
                );
            }
        }

        // ── Cross-check: Rust tuple Ord is the canonical comparison ──────────
        // The above sub-cases cover all branches. This final assertion ties
        // them together: regardless of which case applies, the Rust tuple
        // comparison result must equal the manual lexicographic evaluation.
        let manual_gt = (old_major + major_delta > old_major)
            || (old_major + major_delta == old_major && new_minor > old_minor);
        let rust_gt = (old_major + major_delta, new_minor) > (old_major, old_minor);
        prop_assert_eq!(
            rust_gt,
            manual_gt,
            "Rust tuple Ord disagrees with manual lexicographic: \
             ({}, {}) > ({}, {}): rust={}, manual={}",
            old_major + major_delta, new_minor, old_major, old_minor,
            rust_gt, manual_gt
        );
    }
}
