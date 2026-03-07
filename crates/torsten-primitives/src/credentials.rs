use crate::hash::{Hash28, ScriptHash};
use serde::{Deserialize, Serialize};

/// Payment or staking credential
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Credential {
    /// Verification key hash credential
    VerificationKey(Hash28),
    /// Script hash credential (native or Plutus)
    Script(ScriptHash),
}

impl Credential {
    pub fn to_hash(&self) -> &Hash28 {
        match self {
            Credential::VerificationKey(h) => h,
            Credential::Script(h) => h,
        }
    }

    pub fn is_script(&self) -> bool {
        matches!(self, Credential::Script(_))
    }
}

/// Stake credential reference
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum StakeReference {
    /// Stake credential embedded in the address
    StakeCredential(Credential),
    /// Pointer to a stake registration certificate
    Pointer(Pointer),
    /// No staking component
    Null,
}

/// Certificate pointer (slot, tx_index, cert_index)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Pointer {
    pub slot: u64,
    pub tx_index: u64,
    pub cert_index: u64,
}
