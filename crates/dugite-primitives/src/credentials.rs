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

    /// Convert to a 32-byte hash that preserves the credential TYPE.
    ///
    /// The 28-byte hash is zero-padded to 32 bytes, with byte 28 set to
    /// `0x01` for script credentials and `0x00` for key credentials.
    /// This ensures that a key hash and script hash with identical 28-byte
    /// values produce DIFFERENT Hash32 keys, matching Haskell's `Credential`
    /// type which distinguishes `KeyHashObj` from `ScriptHashObj`.
    pub fn to_typed_hash32(&self) -> crate::hash::Hash<32> {
        let mut bytes = [0u8; 32];
        bytes[..28].copy_from_slice(self.to_hash().as_bytes());
        if self.is_script() {
            bytes[28] = 0x01;
        }
        crate::hash::Hash::<32>(bytes)
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
