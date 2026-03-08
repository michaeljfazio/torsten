use serde::{Deserialize, Serialize};

/// Cardano network identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NetworkId {
    Testnet,
    Mainnet,
}

impl NetworkId {
    pub fn to_u8(self) -> u8 {
        match self {
            NetworkId::Testnet => 0,
            NetworkId::Mainnet => 1,
        }
    }

    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(NetworkId::Testnet),
            1 => Some(NetworkId::Mainnet),
            _ => None,
        }
    }

    /// The network magic number used in handshake
    pub fn magic(self) -> u64 {
        match self {
            NetworkId::Mainnet => 764824073,
            NetworkId::Testnet => 1,
        }
    }

    pub fn bech32_hrp_addr(self) -> &'static str {
        match self {
            NetworkId::Mainnet => "addr",
            NetworkId::Testnet => "addr_test",
        }
    }

    pub fn bech32_hrp_stake(self) -> &'static str {
        match self {
            NetworkId::Mainnet => "stake",
            NetworkId::Testnet => "stake_test",
        }
    }

    /// Default system start time for the network.
    /// For testnets, this returns the preprod start; use Shelley genesis for exact value.
    pub fn system_start(self) -> &'static str {
        match self {
            NetworkId::Mainnet => "2017-09-23T21:44:51Z",
            NetworkId::Testnet => "2022-06-01T00:00:00Z",
        }
    }
}
