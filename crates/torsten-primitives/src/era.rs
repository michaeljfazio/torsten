use serde::{Deserialize, Serialize};

/// Cardano ledger eras
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Era {
    /// Byron era (original OBFT consensus)
    Byron,
    /// Shelley era (decentralization, staking)
    Shelley,
    /// Allegra era (token locking, time-lock scripts)
    Allegra,
    /// Mary era (multi-asset support)
    Mary,
    /// Alonzo era (Plutus smart contracts)
    Alonzo,
    /// Babbage era (Plutus V2, reference inputs/scripts)
    Babbage,
    /// Conway era (on-chain governance, CIP-1694)
    Conway,
}

impl Era {
    pub fn is_shelley_based(&self) -> bool {
        !matches!(self, Era::Byron)
    }

    pub fn supports_native_assets(&self) -> bool {
        matches!(self, Era::Mary | Era::Alonzo | Era::Babbage | Era::Conway)
    }

    pub fn supports_plutus(&self) -> bool {
        matches!(self, Era::Alonzo | Era::Babbage | Era::Conway)
    }

    pub fn supports_plutus_v2(&self) -> bool {
        matches!(self, Era::Babbage | Era::Conway)
    }

    pub fn supports_plutus_v3(&self) -> bool {
        matches!(self, Era::Conway)
    }

    pub fn supports_governance(&self) -> bool {
        matches!(self, Era::Conway)
    }

    pub fn supports_reference_inputs(&self) -> bool {
        matches!(self, Era::Babbage | Era::Conway)
    }

    pub fn supports_inline_datums(&self) -> bool {
        matches!(self, Era::Babbage | Era::Conway)
    }

    pub fn supports_reference_scripts(&self) -> bool {
        matches!(self, Era::Babbage | Era::Conway)
    }

    /// Era index for the N2C protocol (hard-fork combinator era index)
    pub fn to_era_index(self) -> u32 {
        match self {
            Era::Byron => 0,
            Era::Shelley => 1,
            Era::Allegra => 2,
            Era::Mary => 3,
            Era::Alonzo => 4,
            Era::Babbage => 5,
            Era::Conway => 6,
        }
    }
}

impl std::fmt::Display for Era {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Era::Byron => write!(f, "Byron"),
            Era::Shelley => write!(f, "Shelley"),
            Era::Allegra => write!(f, "Allegra"),
            Era::Mary => write!(f, "Mary"),
            Era::Alonzo => write!(f, "Alonzo"),
            Era::Babbage => write!(f, "Babbage"),
            Era::Conway => write!(f, "Conway"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_shelley_based() {
        assert!(!Era::Byron.is_shelley_based());
        assert!(Era::Shelley.is_shelley_based());
        assert!(Era::Conway.is_shelley_based());
    }

    #[test]
    fn test_supports_native_assets() {
        assert!(!Era::Byron.supports_native_assets());
        assert!(!Era::Shelley.supports_native_assets());
        assert!(!Era::Allegra.supports_native_assets());
        assert!(Era::Mary.supports_native_assets());
        assert!(Era::Conway.supports_native_assets());
    }

    #[test]
    fn test_supports_plutus() {
        assert!(!Era::Mary.supports_plutus());
        assert!(Era::Alonzo.supports_plutus());
        assert!(Era::Babbage.supports_plutus());
        assert!(Era::Conway.supports_plutus());
    }

    #[test]
    fn test_supports_plutus_v2_v3() {
        assert!(!Era::Alonzo.supports_plutus_v2());
        assert!(Era::Babbage.supports_plutus_v2());
        assert!(!Era::Babbage.supports_plutus_v3());
        assert!(Era::Conway.supports_plutus_v3());
    }

    #[test]
    fn test_supports_governance() {
        assert!(!Era::Babbage.supports_governance());
        assert!(Era::Conway.supports_governance());
    }

    #[test]
    fn test_supports_reference_features() {
        assert!(!Era::Alonzo.supports_reference_inputs());
        assert!(Era::Babbage.supports_reference_inputs());
        assert!(Era::Babbage.supports_inline_datums());
        assert!(Era::Babbage.supports_reference_scripts());
        assert!(Era::Conway.supports_reference_inputs());
    }

    #[test]
    fn test_era_index() {
        assert_eq!(Era::Byron.to_era_index(), 0);
        assert_eq!(Era::Shelley.to_era_index(), 1);
        assert_eq!(Era::Allegra.to_era_index(), 2);
        assert_eq!(Era::Mary.to_era_index(), 3);
        assert_eq!(Era::Alonzo.to_era_index(), 4);
        assert_eq!(Era::Babbage.to_era_index(), 5);
        assert_eq!(Era::Conway.to_era_index(), 6);
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", Era::Byron), "Byron");
        assert_eq!(format!("{}", Era::Conway), "Conway");
    }

    #[test]
    fn test_ordering() {
        assert!(Era::Byron < Era::Shelley);
        assert!(Era::Shelley < Era::Conway);
        assert!(Era::Alonzo < Era::Babbage);
    }
}
