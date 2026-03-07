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
        matches!(
            self,
            Era::Mary | Era::Alonzo | Era::Babbage | Era::Conway
        )
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
