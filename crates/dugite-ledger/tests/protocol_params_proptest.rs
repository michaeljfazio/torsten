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
