use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use torsten_primitives::network::NetworkId;

/// Node configuration (compatible with cardano-node config format)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NodeConfig {
    /// Network identifier
    #[serde(default = "default_network")]
    pub network: NetworkId,

    /// Network magic number
    #[serde(default)]
    pub network_magic: Option<u64>,

    /// Protocol parameters
    #[serde(default)]
    pub protocol: Protocol,

    /// Shelley genesis file path
    #[serde(default)]
    pub shelley_genesis_file: Option<String>,

    /// Byron genesis file path
    #[serde(default)]
    pub byron_genesis_file: Option<String>,

    /// Alonzo genesis file path
    #[serde(default)]
    pub alonzo_genesis_file: Option<String>,

    /// Conway genesis file path
    #[serde(default)]
    pub conway_genesis_file: Option<String>,

    /// Enable P2P networking
    #[serde(default)]
    pub enable_p2_p: bool,

    /// Target number of active peers
    #[serde(default = "default_target_peers")]
    pub target_number_of_active_peers: usize,

    /// Target number of established peers
    #[serde(default = "default_established_peers")]
    pub target_number_of_established_peers: usize,

    /// Target number of known peers
    #[serde(default = "default_known_peers")]
    pub target_number_of_known_peers: usize,

    /// Trace options
    #[serde(default)]
    pub trace_options: TraceOptions,

    /// Minimum severity for logging
    #[serde(default = "default_min_severity")]
    pub min_severity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct Protocol {
    #[serde(default = "default_requires_network_magic")]
    pub requires_network_magic: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct TraceOptions {
    #[serde(default)]
    pub trace_block_fetch_client: bool,
    #[serde(default)]
    pub trace_block_fetch_server: bool,
    #[serde(default)]
    pub trace_chain_db: bool,
    #[serde(default)]
    pub trace_chain_sync_client: bool,
    #[serde(default)]
    pub trace_chain_sync_server: bool,
    #[serde(default)]
    pub trace_forge: bool,
    #[serde(default)]
    pub trace_mempool: bool,
}

fn default_network() -> NetworkId {
    NetworkId::Mainnet
}

fn default_target_peers() -> usize {
    20
}

fn default_established_peers() -> usize {
    40
}

fn default_known_peers() -> usize {
    100
}

fn default_min_severity() -> String {
    "Info".to_string()
}

fn default_requires_network_magic() -> String {
    "RequiresMagic".to_string()
}

impl NodeConfig {
    pub fn load(path: &Path) -> Result<Self> {
        if path.exists() {
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read config file: {}", path.display()))?;

            // Try JSON first (cardano-node format), then TOML
            if path.extension().map_or(false, |e| e == "json") {
                serde_json::from_str(&content)
                    .with_context(|| format!("Failed to parse JSON config: {}", path.display()))
            } else {
                toml::from_str(&content)
                    .with_context(|| format!("Failed to parse TOML config: {}", path.display()))
            }
        } else {
            // Use defaults
            Ok(Self::default())
        }
    }

    pub fn network_magic(&self) -> u64 {
        self.network_magic.unwrap_or_else(|| self.network.magic())
    }
}

impl Default for NodeConfig {
    fn default() -> Self {
        NodeConfig {
            network: NetworkId::Mainnet,
            network_magic: None,
            protocol: Protocol::default(),
            shelley_genesis_file: None,
            byron_genesis_file: None,
            alonzo_genesis_file: None,
            conway_genesis_file: None,
            enable_p2_p: true,
            target_number_of_active_peers: 20,
            target_number_of_established_peers: 40,
            target_number_of_known_peers: 100,
            trace_options: TraceOptions::default(),
            min_severity: "Info".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = NodeConfig::default();
        assert_eq!(config.network, NetworkId::Mainnet);
        assert_eq!(config.network_magic(), 764824073);
    }

    #[test]
    fn test_custom_magic() {
        let mut config = NodeConfig::default();
        config.network_magic = Some(42);
        assert_eq!(config.network_magic(), 42);
    }
}
