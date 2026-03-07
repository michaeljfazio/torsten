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
    /// Decode an address from raw bytes (Shelley format)
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, AddressError> {
        if bytes.is_empty() {
            return Err(AddressError::TooShort);
        }

        let header = bytes[0];
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
