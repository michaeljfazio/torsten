use crate::credentials::{Credential, Pointer, StakeReference};
use crate::hash::Hash28;
use crate::network::NetworkId;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AddressError {
    #[error("Invalid address header byte: {0:#04x}")]
    InvalidHeader(u8),
    #[error("Address too short")]
    TooShort,
    #[error("Invalid bech32 encoding: {0}")]
    Bech32Error(String),
    #[error("Invalid Byron address")]
    InvalidByronAddress,
}

/// Cardano address (all types)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Address {
    /// Base address: payment + staking credential
    Base(BaseAddress),
    /// Enterprise address: payment credential only (no staking)
    Enterprise(EnterpriseAddress),
    /// Pointer address: payment + stake pointer
    Pointer(PointerAddress),
    /// Reward/stake address (for withdrawals)
    Reward(RewardAddress),
    /// Byron-era bootstrap address
    Byron(ByronAddress),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BaseAddress {
    pub network: NetworkId,
    pub payment: Credential,
    pub stake: Credential,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EnterpriseAddress {
    pub network: NetworkId,
    pub payment: Credential,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PointerAddress {
    pub network: NetworkId,
    pub payment: Credential,
    pub pointer: Pointer,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RewardAddress {
    pub network: NetworkId,
    pub stake: Credential,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ByronAddress {
    pub payload: Vec<u8>,
}

impl Address {
    /// Decode an address from raw bytes (Shelley or Byron format)
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, AddressError> {
        if bytes.is_empty() {
            return Err(AddressError::TooShort);
        }

        let header = bytes[0];

        // Byron addresses start with CBOR encoding (0x82, 0x83, etc.)
        // or have the Shelley-era type nibble 0b1000.
        // Detect Byron by checking if the first byte is a CBOR array/tag marker
        // that doesn't match Shelley header patterns.
        // CBOR major type 4 (array) starts at 0x80, major type 6 (tag) starts at 0xC0.
        // Byron addresses are typically CBOR arrays starting with 0x82 or 0x83.
        if header == 0x82 || header == 0x83 {
            return Ok(Address::Byron(ByronAddress {
                payload: bytes.to_vec(),
            }));
        }

        let addr_type = (header >> 4) & 0x0F;
        let network_id =
            NetworkId::from_u8(header & 0x0F).ok_or(AddressError::InvalidHeader(header))?;

        match addr_type {
            // Base addresses (types 0-3)
            0b0000..=0b0011 => {
                if bytes.len() < 57 {
                    return Err(AddressError::TooShort);
                }
                let payment = decode_credential(addr_type & 0b01, &bytes[1..29])?;
                let stake = decode_credential((addr_type >> 1) & 0b01, &bytes[29..57])?;
                Ok(Address::Base(BaseAddress {
                    network: network_id,
                    payment,
                    stake,
                }))
            }
            // Pointer addresses (types 4-5)
            0b0100..=0b0101 => {
                if bytes.len() < 29 {
                    return Err(AddressError::TooShort);
                }
                let payment = decode_credential(addr_type & 0b01, &bytes[1..29])?;
                let (pointer, _) = decode_pointer(&bytes[29..])?;
                Ok(Address::Pointer(PointerAddress {
                    network: network_id,
                    payment,
                    pointer,
                }))
            }
            // Enterprise addresses (types 6-7)
            0b0110..=0b0111 => {
                if bytes.len() < 29 {
                    return Err(AddressError::TooShort);
                }
                let payment = decode_credential(addr_type & 0b01, &bytes[1..29])?;
                Ok(Address::Enterprise(EnterpriseAddress {
                    network: network_id,
                    payment,
                }))
            }
            // Byron address (type 8)
            0b1000 => Ok(Address::Byron(ByronAddress {
                payload: bytes.to_vec(),
            })),
            // Reward addresses (types 14-15)
            0b1110 | 0b1111 => {
                if bytes.len() < 29 {
                    return Err(AddressError::TooShort);
                }
                let stake = decode_credential(addr_type & 0b01, &bytes[1..29])?;
                Ok(Address::Reward(RewardAddress {
                    network: network_id,
                    stake,
                }))
            }
            _ => Err(AddressError::InvalidHeader(header)),
        }
    }

    /// Serialize address to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            Address::Base(addr) => {
                let payment_bit = credential_type_bit(&addr.payment);
                let stake_bit = credential_type_bit(&addr.stake);
                let header = (stake_bit << 5) | (payment_bit << 4) | addr.network.to_u8();
                let mut bytes = vec![header];
                bytes.extend_from_slice(addr.payment.to_hash().as_bytes());
                bytes.extend_from_slice(addr.stake.to_hash().as_bytes());
                bytes
            }
            Address::Enterprise(addr) => {
                let payment_bit = credential_type_bit(&addr.payment);
                let header = (0b0110 | payment_bit) << 4 | addr.network.to_u8();
                let mut bytes = vec![header];
                bytes.extend_from_slice(addr.payment.to_hash().as_bytes());
                bytes
            }
            Address::Reward(addr) => {
                let stake_bit = credential_type_bit(&addr.stake);
                let header = (0b1110 | stake_bit) << 4 | addr.network.to_u8();
                let mut bytes = vec![header];
                bytes.extend_from_slice(addr.stake.to_hash().as_bytes());
                bytes
            }
            Address::Pointer(addr) => {
                let payment_bit = credential_type_bit(&addr.payment);
                let header = (0b0100 | payment_bit) << 4 | addr.network.to_u8();
                let mut bytes = vec![header];
                bytes.extend_from_slice(addr.payment.to_hash().as_bytes());
                bytes.extend(encode_variable_length(addr.pointer.slot));
                bytes.extend(encode_variable_length(addr.pointer.tx_index));
                bytes.extend(encode_variable_length(addr.pointer.cert_index));
                bytes
            }
            Address::Byron(addr) => addr.payload.clone(),
        }
    }

    pub fn network_id(&self) -> Option<NetworkId> {
        match self {
            Address::Base(a) => Some(a.network),
            Address::Enterprise(a) => Some(a.network),
            Address::Pointer(a) => Some(a.network),
            Address::Reward(a) => Some(a.network),
            Address::Byron(_) => None,
        }
    }

    pub fn payment_credential(&self) -> Option<&Credential> {
        match self {
            Address::Base(a) => Some(&a.payment),
            Address::Enterprise(a) => Some(&a.payment),
            Address::Pointer(a) => Some(&a.payment),
            Address::Reward(_) => None,
            Address::Byron(_) => None,
        }
    }

    pub fn stake_reference(&self) -> StakeReference {
        match self {
            Address::Base(a) => StakeReference::StakeCredential(a.stake.clone()),
            Address::Pointer(a) => StakeReference::Pointer(a.pointer),
            _ => StakeReference::Null,
        }
    }
}

fn credential_type_bit(cred: &Credential) -> u8 {
    match cred {
        Credential::VerificationKey(_) => 0,
        Credential::Script(_) => 1,
    }
}

fn decode_credential(type_bit: u8, bytes: &[u8]) -> Result<Credential, AddressError> {
    if bytes.len() < 28 {
        return Err(AddressError::TooShort);
    }
    let mut hash = [0u8; 28];
    hash.copy_from_slice(&bytes[..28]);
    let h = Hash28::from_bytes(hash);
    match type_bit {
        0 => Ok(Credential::VerificationKey(h)),
        1 => Ok(Credential::Script(h)),
        _ => Err(AddressError::InvalidHeader(type_bit)),
    }
}

fn decode_pointer(bytes: &[u8]) -> Result<(Pointer, usize), AddressError> {
    let (slot, n1) = decode_variable_length(bytes).ok_or(AddressError::TooShort)?;
    let (tx_index, n2) = decode_variable_length(&bytes[n1..]).ok_or(AddressError::TooShort)?;
    let (cert_index, n3) =
        decode_variable_length(&bytes[n1 + n2..]).ok_or(AddressError::TooShort)?;
    Ok((
        Pointer {
            slot,
            tx_index,
            cert_index,
        },
        n1 + n2 + n3,
    ))
}

fn decode_variable_length(bytes: &[u8]) -> Option<(u64, usize)> {
    let mut result: u64 = 0;
    for (i, &byte) in bytes.iter().enumerate() {
        result = (result << 7) | (byte & 0x7F) as u64;
        if byte & 0x80 == 0 {
            return Some((result, i + 1));
        }
    }
    None
}

fn encode_variable_length(mut value: u64) -> Vec<u8> {
    if value == 0 {
        return vec![0];
    }
    let mut bytes = Vec::new();
    while value > 0 {
        bytes.push((value & 0x7F) as u8);
        value >>= 7;
    }
    bytes.reverse();
    let last = bytes.len() - 1;
    for b in bytes.iter_mut().take(last) {
        *b |= 0x80;
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_hash28(val: u8) -> Hash28 {
        Hash28::from_bytes([val; 28])
    }

    #[test]
    fn test_base_address_roundtrip() {
        let addr = Address::Base(BaseAddress {
            network: NetworkId::Testnet,
            payment: Credential::VerificationKey(make_hash28(0xaa)),
            stake: Credential::VerificationKey(make_hash28(0xbb)),
        });
        let bytes = addr.to_bytes();
        assert_eq!(bytes.len(), 57);
        // Header: type 0b0000, network 0x00
        assert_eq!(bytes[0] & 0xF0, 0x00);
        assert_eq!(bytes[0] & 0x0F, 0x00);

        let decoded = Address::from_bytes(&bytes).unwrap();
        assert_eq!(addr, decoded);
    }

    #[test]
    fn test_base_address_script_credentials() {
        let addr = Address::Base(BaseAddress {
            network: NetworkId::Mainnet,
            payment: Credential::Script(make_hash28(0xcc)),
            stake: Credential::Script(make_hash28(0xdd)),
        });
        let bytes = addr.to_bytes();
        // type=0b0011 (both script), network=1
        assert_eq!(bytes[0], 0x31);
        let decoded = Address::from_bytes(&bytes).unwrap();
        assert_eq!(addr, decoded);
    }

    #[test]
    fn test_enterprise_address_roundtrip() {
        let addr = Address::Enterprise(EnterpriseAddress {
            network: NetworkId::Testnet,
            payment: Credential::VerificationKey(make_hash28(0xee)),
        });
        let bytes = addr.to_bytes();
        assert_eq!(bytes.len(), 29);
        // type=0b0110, network=0
        assert_eq!(bytes[0], 0x60);
        let decoded = Address::from_bytes(&bytes).unwrap();
        assert_eq!(addr, decoded);
    }

    #[test]
    fn test_reward_address_roundtrip() {
        let addr = Address::Reward(RewardAddress {
            network: NetworkId::Mainnet,
            stake: Credential::VerificationKey(make_hash28(0xff)),
        });
        let bytes = addr.to_bytes();
        assert_eq!(bytes.len(), 29);
        // type=0b1110, network=1
        assert_eq!(bytes[0], 0xe1);
        let decoded = Address::from_bytes(&bytes).unwrap();
        assert_eq!(addr, decoded);
    }

    #[test]
    fn test_pointer_address_roundtrip() {
        let addr = Address::Pointer(PointerAddress {
            network: NetworkId::Testnet,
            payment: Credential::VerificationKey(make_hash28(0x11)),
            pointer: Pointer {
                slot: 100,
                tx_index: 2,
                cert_index: 0,
            },
        });
        let bytes = addr.to_bytes();
        let decoded = Address::from_bytes(&bytes).unwrap();
        assert_eq!(addr, decoded);
    }

    #[test]
    fn test_pointer_address_large_values() {
        let addr = Address::Pointer(PointerAddress {
            network: NetworkId::Mainnet,
            payment: Credential::VerificationKey(make_hash28(0x22)),
            pointer: Pointer {
                slot: 100_000_000,
                tx_index: 300,
                cert_index: 50,
            },
        });
        let bytes = addr.to_bytes();
        let decoded = Address::from_bytes(&bytes).unwrap();
        assert_eq!(addr, decoded);
    }

    #[test]
    fn test_byron_address() {
        // Byron addresses start with 0x82 or 0x83
        let byron_bytes = vec![0x82, 0x01, 0x02, 0x03];
        let addr = Address::from_bytes(&byron_bytes).unwrap();
        match addr {
            Address::Byron(b) => assert_eq!(b.payload, byron_bytes),
            other => panic!("Expected Byron, got {other:?}"),
        }
    }

    #[test]
    fn test_empty_address_error() {
        assert!(Address::from_bytes(&[]).is_err());
    }

    #[test]
    fn test_too_short_base_address() {
        // Base address needs 57 bytes, provide only 30
        let mut bytes = vec![0x00]; // type 0, testnet
        bytes.extend_from_slice(&[0xaa; 28]); // payment only, missing stake
        assert!(Address::from_bytes(&bytes).is_err());
    }

    #[test]
    fn test_too_short_enterprise_address() {
        let bytes = vec![0x60, 0xaa]; // type 6, testnet, only 1 byte of hash
        assert!(Address::from_bytes(&bytes).is_err());
    }

    #[test]
    fn test_network_id() {
        let base = Address::Base(BaseAddress {
            network: NetworkId::Mainnet,
            payment: Credential::VerificationKey(make_hash28(0)),
            stake: Credential::VerificationKey(make_hash28(0)),
        });
        assert_eq!(base.network_id(), Some(NetworkId::Mainnet));

        let byron = Address::Byron(ByronAddress {
            payload: vec![0x82],
        });
        assert_eq!(byron.network_id(), None);
    }

    #[test]
    fn test_payment_credential() {
        let hash = make_hash28(0xab);
        let addr = Address::Enterprise(EnterpriseAddress {
            network: NetworkId::Testnet,
            payment: Credential::VerificationKey(hash),
        });
        assert_eq!(
            addr.payment_credential(),
            Some(&Credential::VerificationKey(hash))
        );

        let reward = Address::Reward(RewardAddress {
            network: NetworkId::Testnet,
            stake: Credential::VerificationKey(hash),
        });
        assert_eq!(reward.payment_credential(), None);
    }

    #[test]
    fn test_stake_reference() {
        let hash = make_hash28(0xcd);
        let base = Address::Base(BaseAddress {
            network: NetworkId::Testnet,
            payment: Credential::VerificationKey(make_hash28(0)),
            stake: Credential::VerificationKey(hash),
        });
        match base.stake_reference() {
            StakeReference::StakeCredential(c) => {
                assert_eq!(c, Credential::VerificationKey(hash));
            }
            other => panic!("Expected StakeCredential, got {other:?}"),
        }

        let enterprise = Address::Enterprise(EnterpriseAddress {
            network: NetworkId::Testnet,
            payment: Credential::VerificationKey(make_hash28(0)),
        });
        assert!(matches!(enterprise.stake_reference(), StakeReference::Null));
    }

    #[test]
    fn test_variable_length_encoding_zero() {
        let encoded = encode_variable_length(0);
        assert_eq!(encoded, vec![0]);
        let (decoded, len) = decode_variable_length(&encoded).unwrap();
        assert_eq!(decoded, 0);
        assert_eq!(len, 1);
    }

    #[test]
    fn test_variable_length_encoding_small() {
        let encoded = encode_variable_length(127);
        assert_eq!(encoded, vec![0x7F]);
        let (decoded, len) = decode_variable_length(&encoded).unwrap();
        assert_eq!(decoded, 127);
        assert_eq!(len, 1);
    }

    #[test]
    fn test_variable_length_encoding_two_bytes() {
        let encoded = encode_variable_length(128);
        assert_eq!(encoded, vec![0x81, 0x00]);
        let (decoded, _) = decode_variable_length(&encoded).unwrap();
        assert_eq!(decoded, 128);
    }

    #[test]
    fn test_variable_length_encoding_large() {
        let value = 100_000_000u64;
        let encoded = encode_variable_length(value);
        let (decoded, _) = decode_variable_length(&encoded).unwrap();
        assert_eq!(decoded, value);
    }
}
