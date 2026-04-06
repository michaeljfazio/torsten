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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::Hash;

    fn key_hash() -> Hash28 {
        Hash::from_bytes([0x01; 28])
    }

    fn script_hash() -> ScriptHash {
        Hash::from_bytes([0x02; 28])
    }

    // ========== Credential::to_hash ==========

    #[test]
    fn test_to_hash_verification_key() {
        let cred = Credential::VerificationKey(key_hash());
        assert_eq!(cred.to_hash(), &key_hash());
    }

    #[test]
    fn test_to_hash_script() {
        let cred = Credential::Script(script_hash());
        assert_eq!(cred.to_hash(), &script_hash());
    }

    // ========== Credential::is_script ==========

    #[test]
    fn test_is_script_false_for_key() {
        let cred = Credential::VerificationKey(key_hash());
        assert!(!cred.is_script());
    }

    #[test]
    fn test_is_script_true_for_script() {
        let cred = Credential::Script(script_hash());
        assert!(cred.is_script());
    }

    // ========== Credential::to_typed_hash32 ==========

    #[test]
    fn test_to_typed_hash32_key_padding() {
        let cred = Credential::VerificationKey(key_hash());
        let h32 = cred.to_typed_hash32();
        let bytes = h32.as_bytes();
        // First 28 bytes match the input hash
        assert_eq!(&bytes[..28], &[0x01; 28]);
        // Byte 28 is 0x00 for key credentials
        assert_eq!(bytes[28], 0x00);
        // Remaining bytes are zero
        assert_eq!(&bytes[29..], &[0x00; 3]);
    }

    #[test]
    fn test_to_typed_hash32_script_padding() {
        let cred = Credential::Script(script_hash());
        let h32 = cred.to_typed_hash32();
        let bytes = h32.as_bytes();
        // First 28 bytes match the input hash
        assert_eq!(&bytes[..28], &[0x02; 28]);
        // Byte 28 is 0x01 for script credentials
        assert_eq!(bytes[28], 0x01);
        // Remaining bytes are zero
        assert_eq!(&bytes[29..], &[0x00; 3]);
    }

    #[test]
    fn test_to_typed_hash32_distinctness() {
        // Critical invariant: same 28-byte hash produces DIFFERENT Hash32
        // for key vs script credentials.
        let same_hash = Hash::from_bytes([0xaa; 28]);
        let key_cred = Credential::VerificationKey(same_hash);
        let script_cred = Credential::Script(same_hash);
        assert_ne!(key_cred.to_typed_hash32(), script_cred.to_typed_hash32());
    }

    // ========== Credential Ord ==========

    #[test]
    fn test_credential_ord_key_before_script() {
        // Derived Ord: enum variant order (VerificationKey=0 < Script=1)
        let key = Credential::VerificationKey(key_hash());
        let script = Credential::Script(script_hash());
        assert!(key < script);
    }

    // ========== StakeReference ==========

    #[test]
    fn test_stake_reference_serde_roundtrip() {
        let variants = vec![
            StakeReference::StakeCredential(Credential::VerificationKey(key_hash())),
            StakeReference::Pointer(Pointer {
                slot: 100,
                tx_index: 2,
                cert_index: 0,
            }),
            StakeReference::Null,
        ];
        for v in variants {
            let json = serde_json::to_string(&v).unwrap();
            let v2: StakeReference = serde_json::from_str(&json).unwrap();
            assert_eq!(v, v2);
        }
    }

    // ========== Pointer ==========

    #[test]
    fn test_pointer_ord() {
        let p1 = Pointer {
            slot: 1,
            tx_index: 0,
            cert_index: 0,
        };
        let p2 = Pointer {
            slot: 2,
            tx_index: 0,
            cert_index: 0,
        };
        assert!(p1 < p2);
    }

    #[test]
    fn test_pointer_serde_roundtrip() {
        let p = Pointer {
            slot: 42,
            tx_index: 3,
            cert_index: 1,
        };
        let json = serde_json::to_string(&p).unwrap();
        let p2: Pointer = serde_json::from_str(&json).unwrap();
        assert_eq!(p, p2);
    }
}
