use super::{LedgerError, LedgerState};
use dugite_primitives::transaction::{ProtocolParamUpdate, Rational};

impl LedgerState {
    /// Validate that a governance threshold rational is in the range [0, 1]
    /// with a non-zero denominator.
    pub(crate) fn validate_threshold(name: &str, r: &Rational) -> Result<(), LedgerError> {
        if r.denominator == 0 {
            return Err(LedgerError::InvalidProtocolParam(format!(
                "{}: zero denominator",
                name
            )));
        }
        if r.numerator > r.denominator {
            return Err(LedgerError::InvalidProtocolParam(format!(
                "{}: threshold {}/{} exceeds 1",
                name, r.numerator, r.denominator
            )));
        }
        Ok(())
    }

    /// Apply a single ProtocolParamUpdate to the current protocol parameters.
    /// Each field in the update, if Some, overwrites the corresponding parameter.
    /// Used by both pre-Conway update proposals and Conway governance actions.
    /// Returns an error if any governance threshold is out of range [0, 1].
    pub(crate) fn apply_protocol_param_update(
        &mut self,
        update: &ProtocolParamUpdate,
    ) -> Result<(), LedgerError> {
        if let Some(v) = update.min_fee_a {
            self.protocol_params.min_fee_a = v;
        }
        if let Some(v) = update.min_fee_b {
            self.protocol_params.min_fee_b = v;
        }
        if let Some(v) = update.max_block_body_size {
            self.protocol_params.max_block_body_size = v;
        }
        if let Some(v) = update.max_tx_size {
            self.protocol_params.max_tx_size = v;
        }
        if let Some(v) = update.max_block_header_size {
            self.protocol_params.max_block_header_size = v;
        }
        if let Some(v) = update.key_deposit {
            self.protocol_params.key_deposit = v;
        }
        if let Some(v) = update.pool_deposit {
            self.protocol_params.pool_deposit = v;
        }
        if let Some(v) = update.e_max {
            self.protocol_params.e_max = v;
        }
        if let Some(v) = update.n_opt {
            self.protocol_params.n_opt = v;
        }
        if let Some(ref v) = update.a0 {
            self.protocol_params.a0 = v.clone();
        }
        if let Some(ref v) = update.rho {
            self.protocol_params.rho = v.clone();
        }
        if let Some(ref v) = update.tau {
            self.protocol_params.tau = v.clone();
        }
        if let Some(ref v) = update.d {
            self.protocol_params.d = v.clone();
        }
        if let Some(v) = update.min_pool_cost {
            self.protocol_params.min_pool_cost = v;
        }
        if let Some(v) = update.ada_per_utxo_byte {
            self.protocol_params.ada_per_utxo_byte = v;
        }
        if let Some(ref v) = update.cost_models {
            if let Some(ref v1) = v.plutus_v1 {
                self.protocol_params.cost_models.plutus_v1 = Some(v1.clone());
            }
            if let Some(ref v2) = v.plutus_v2 {
                self.protocol_params.cost_models.plutus_v2 = Some(v2.clone());
            }
            if let Some(ref v3) = v.plutus_v3 {
                self.protocol_params.cost_models.plutus_v3 = Some(v3.clone());
            }
        }
        if let Some(ref v) = update.execution_costs {
            self.protocol_params.execution_costs = v.clone();
        }
        if let Some(v) = update.max_tx_ex_units {
            self.protocol_params.max_tx_ex_units = v;
        }
        if let Some(v) = update.max_block_ex_units {
            self.protocol_params.max_block_ex_units = v;
        }
        if let Some(v) = update.max_val_size {
            self.protocol_params.max_val_size = v;
        }
        if let Some(v) = update.collateral_percentage {
            self.protocol_params.collateral_percentage = v;
        }
        if let Some(v) = update.max_collateral_inputs {
            self.protocol_params.max_collateral_inputs = v;
        }
        if let Some(v) = update.min_fee_ref_script_cost_per_byte {
            self.protocol_params.min_fee_ref_script_cost_per_byte = v;
        }
        if let Some(v) = update.drep_deposit {
            self.protocol_params.drep_deposit = v;
        }
        if let Some(v) = update.gov_action_lifetime {
            self.protocol_params.gov_action_lifetime = v;
        }
        if let Some(v) = update.gov_action_deposit {
            self.protocol_params.gov_action_deposit = v;
        }
        if let Some(ref v) = update.dvt_pp_network_group {
            Self::validate_threshold("dvt_pp_network_group", v)?;
            self.protocol_params.dvt_pp_network_group = v.clone();
        }
        if let Some(ref v) = update.dvt_pp_economic_group {
            Self::validate_threshold("dvt_pp_economic_group", v)?;
            self.protocol_params.dvt_pp_economic_group = v.clone();
        }
        if let Some(ref v) = update.dvt_pp_technical_group {
            Self::validate_threshold("dvt_pp_technical_group", v)?;
            self.protocol_params.dvt_pp_technical_group = v.clone();
        }
        if let Some(ref v) = update.dvt_pp_gov_group {
            Self::validate_threshold("dvt_pp_gov_group", v)?;
            self.protocol_params.dvt_pp_gov_group = v.clone();
        }
        if let Some(ref v) = update.dvt_hard_fork {
            Self::validate_threshold("dvt_hard_fork", v)?;
            self.protocol_params.dvt_hard_fork = v.clone();
        }
        if let Some(ref v) = update.dvt_no_confidence {
            Self::validate_threshold("dvt_no_confidence", v)?;
            self.protocol_params.dvt_no_confidence = v.clone();
        }
        if let Some(ref v) = update.dvt_committee_normal {
            Self::validate_threshold("dvt_committee_normal", v)?;
            self.protocol_params.dvt_committee_normal = v.clone();
        }
        if let Some(ref v) = update.dvt_committee_no_confidence {
            Self::validate_threshold("dvt_committee_no_confidence", v)?;
            self.protocol_params.dvt_committee_no_confidence = v.clone();
        }
        if let Some(ref v) = update.dvt_constitution {
            Self::validate_threshold("dvt_constitution", v)?;
            self.protocol_params.dvt_constitution = v.clone();
        }
        if let Some(ref v) = update.dvt_treasury_withdrawal {
            Self::validate_threshold("dvt_treasury_withdrawal", v)?;
            self.protocol_params.dvt_treasury_withdrawal = v.clone();
        }
        if let Some(ref v) = update.pvt_motion_no_confidence {
            Self::validate_threshold("pvt_motion_no_confidence", v)?;
            self.protocol_params.pvt_motion_no_confidence = v.clone();
        }
        if let Some(ref v) = update.pvt_committee_normal {
            Self::validate_threshold("pvt_committee_normal", v)?;
            self.protocol_params.pvt_committee_normal = v.clone();
        }
        if let Some(ref v) = update.pvt_committee_no_confidence {
            Self::validate_threshold("pvt_committee_no_confidence", v)?;
            self.protocol_params.pvt_committee_no_confidence = v.clone();
        }
        if let Some(ref v) = update.pvt_hard_fork {
            Self::validate_threshold("pvt_hard_fork", v)?;
            self.protocol_params.pvt_hard_fork = v.clone();
        }
        if let Some(ref v) = update.pvt_pp_security_group {
            Self::validate_threshold("pvt_pp_security_group", v)?;
            self.protocol_params.pvt_pp_security_group = v.clone();
        }
        if let Some(v) = update.min_committee_size {
            self.protocol_params.committee_min_size = v;
        }
        if let Some(v) = update.committee_term_limit {
            self.protocol_params.committee_max_term_length = v;
        }
        if let Some(v) = update.drep_activity {
            self.protocol_params.drep_activity = v;
        }
        if let Some(v) = update.protocol_version_major {
            self.protocol_params.protocol_version_major = v;
        }
        if let Some(v) = update.protocol_version_minor {
            self.protocol_params.protocol_version_minor = v;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dugite_primitives::protocol_params::ProtocolParameters;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Create a fresh `LedgerState` with mainnet default protocol parameters.
    fn make_state() -> LedgerState {
        LedgerState::new(ProtocolParameters::mainnet_defaults())
    }

    // -----------------------------------------------------------------------
    // apply_protocol_param_update tests
    // -----------------------------------------------------------------------

    /// Applying an update with only `min_fee_a` set must change exactly that
    /// field and leave every other parameter at its default value.
    #[test]
    fn test_apply_partial_update() {
        let mut state = make_state();
        let defaults = ProtocolParameters::mainnet_defaults();

        let update = ProtocolParamUpdate {
            min_fee_a: Some(50),
            ..Default::default()
        };

        state
            .apply_protocol_param_update(&update)
            .expect("partial update should succeed");

        // The targeted field must have changed.
        assert_eq!(state.protocol_params.min_fee_a, 50);

        // Every other scalar field must be untouched.
        assert_eq!(state.protocol_params.min_fee_b, defaults.min_fee_b);
        assert_eq!(
            state.protocol_params.max_block_body_size,
            defaults.max_block_body_size
        );
        assert_eq!(state.protocol_params.max_tx_size, defaults.max_tx_size);
        assert_eq!(state.protocol_params.key_deposit, defaults.key_deposit);
        assert_eq!(state.protocol_params.pool_deposit, defaults.pool_deposit);
        assert_eq!(state.protocol_params.n_opt, defaults.n_opt);
    }

    /// Applying an update with multiple fields set must change exactly those
    /// fields and leave every other parameter at its default value.
    #[test]
    fn test_apply_full_update() {
        let mut state = make_state();
        let defaults = ProtocolParameters::mainnet_defaults();

        let update = ProtocolParamUpdate {
            min_fee_a: Some(100),
            min_fee_b: Some(200_000),
            max_tx_size: Some(32768),
            ..Default::default()
        };

        state
            .apply_protocol_param_update(&update)
            .expect("multi-field update should succeed");

        assert_eq!(state.protocol_params.min_fee_a, 100);
        assert_eq!(state.protocol_params.min_fee_b, 200_000);
        assert_eq!(state.protocol_params.max_tx_size, 32768);

        // Fields not mentioned in the update remain unchanged.
        assert_eq!(
            state.protocol_params.max_block_body_size,
            defaults.max_block_body_size
        );
        assert_eq!(
            state.protocol_params.max_block_header_size,
            defaults.max_block_header_size
        );
        assert_eq!(state.protocol_params.key_deposit, defaults.key_deposit);
    }

    /// An update with all fields set to `None` is a legal no-op and must not
    /// modify any parameter.
    #[test]
    fn test_noop_update() {
        let mut state = make_state();
        let defaults = ProtocolParameters::mainnet_defaults();

        // Default::default() produces an all-None ProtocolParamUpdate.
        let update = ProtocolParamUpdate::default();

        let result = state.apply_protocol_param_update(&update);
        assert!(result.is_ok(), "no-op update must not return an error");

        // Spot-check a handful of fields to confirm nothing changed.
        assert_eq!(state.protocol_params.min_fee_a, defaults.min_fee_a);
        assert_eq!(state.protocol_params.min_fee_b, defaults.min_fee_b);
        assert_eq!(state.protocol_params.max_tx_size, defaults.max_tx_size);
        assert_eq!(state.protocol_params.n_opt, defaults.n_opt);
        assert_eq!(state.protocol_params.key_deposit, defaults.key_deposit);
    }

    // -----------------------------------------------------------------------
    // validate_threshold tests
    // -----------------------------------------------------------------------

    /// A valid threshold with numerator < denominator must return `Ok(())`.
    #[test]
    fn test_threshold_valid() {
        let r = Rational {
            numerator: 1,
            denominator: 2,
        };
        assert!(
            LedgerState::validate_threshold("test", &r).is_ok(),
            "1/2 is a valid threshold in [0, 1]"
        );
    }

    /// A threshold where numerator > denominator must return an `Err` containing
    /// `LedgerError::InvalidProtocolParam`.
    #[test]
    fn test_threshold_num_exceeds_denom() {
        let r = Rational {
            numerator: 3,
            denominator: 2,
        };
        let err = LedgerState::validate_threshold("test", &r)
            .expect_err("3/2 exceeds 1 and must be rejected");

        // Verify the error variant and that the message names the parameter.
        match err {
            LedgerError::InvalidProtocolParam(msg) => {
                assert!(
                    msg.contains("test"),
                    "error message should name the parameter; got: {msg}"
                );
                assert!(
                    msg.contains("exceeds"),
                    "error message should mention 'exceeds'; got: {msg}"
                );
            }
            other => panic!("expected InvalidProtocolParam, got: {other:?}"),
        }
    }

    /// A threshold with a zero denominator must return an `Err` containing
    /// `LedgerError::InvalidProtocolParam`.
    #[test]
    fn test_threshold_zero_denominator() {
        let r = Rational {
            numerator: 1,
            denominator: 0,
        };
        let err = LedgerState::validate_threshold("test", &r)
            .expect_err("denominator 0 is invalid and must be rejected");

        match err {
            LedgerError::InvalidProtocolParam(msg) => {
                assert!(
                    msg.contains("test"),
                    "error message should name the parameter; got: {msg}"
                );
                assert!(
                    msg.contains("zero denominator"),
                    "error message should mention 'zero denominator'; got: {msg}"
                );
            }
            other => panic!("expected InvalidProtocolParam, got: {other:?}"),
        }
    }
}
