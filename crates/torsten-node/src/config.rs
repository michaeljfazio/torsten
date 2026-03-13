use anyhow::{Context, Result};
use serde::{Deserialize, Deserializer, Serialize};
use std::path::Path;
use torsten_primitives::network::NetworkId;
use torsten_storage::StorageConfigJson;

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

    /// Protocol parameters (can be a string like "Cardano" or a struct)
    #[serde(default, deserialize_with = "deserialize_protocol")]
    pub protocol: Protocol,

    /// RequiresNetworkMagic at top level (guild/newer configs)
    #[serde(default)]
    pub requires_network_magic: Option<String>,

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

    /// Expected Blake2b-256 hash of the Byron genesis file (hex string)
    #[serde(default)]
    pub byron_genesis_hash: Option<String>,

    /// Expected Blake2b-256 hash of the Shelley genesis file (hex string)
    #[serde(default)]
    pub shelley_genesis_hash: Option<String>,

    /// Expected Blake2b-256 hash of the Alonzo genesis file (hex string)
    #[serde(default)]
    pub alonzo_genesis_hash: Option<String>,

    /// Expected Blake2b-256 hash of the Conway genesis file (hex string)
    #[serde(default)]
    pub conway_genesis_hash: Option<String>,

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

    /// Storage configuration (optional overrides for storage profiles)
    #[serde(default)]
    pub storage: Option<StorageConfigJson>,
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

/// Deserialize Protocol from either a string (e.g. "Cardano") or a struct
fn deserialize_protocol<'de, D>(deserializer: D) -> Result<Protocol, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de;

    struct ProtocolVisitor;

    impl<'de> de::Visitor<'de> for ProtocolVisitor {
        type Value = Protocol;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string or Protocol object")
        }

        fn visit_str<E: de::Error>(self, _value: &str) -> Result<Protocol, E> {
            Ok(Protocol::default())
        }

        fn visit_map<M: de::MapAccess<'de>>(self, map: M) -> Result<Protocol, M::Error> {
            Deserialize::deserialize(de::value::MapAccessDeserializer::new(map))
        }
    }

    deserializer.deserialize_any(ProtocolVisitor)
}

impl NodeConfig {
    pub fn load(path: &Path) -> Result<Self> {
        if path.exists() {
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read config file: {}", path.display()))?;

            // Try JSON first (cardano-node format), then TOML
            if path.extension().is_some_and(|e| e == "json") {
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

    #[allow(dead_code)]
    pub fn network_magic(&self) -> u64 {
        self.network_magic.unwrap_or_else(|| self.network.magic())
    }

    /// Validate configuration at startup: check genesis file existence and hash formats.
    /// `config_dir` is the directory containing the config file, used to resolve
    /// relative genesis file paths.
    pub fn validate(&self, config_dir: &Path) -> Result<()> {
        let genesis_files: &[(&str, &Option<String>, &Option<String>)] = &[
            ("Byron", &self.byron_genesis_file, &self.byron_genesis_hash),
            (
                "Shelley",
                &self.shelley_genesis_file,
                &self.shelley_genesis_hash,
            ),
            (
                "Alonzo",
                &self.alonzo_genesis_file,
                &self.alonzo_genesis_hash,
            ),
            (
                "Conway",
                &self.conway_genesis_file,
                &self.conway_genesis_hash,
            ),
        ];

        for (era, file_opt, hash_opt) in genesis_files {
            if let Some(ref file_path) = file_opt {
                let resolved = config_dir.join(file_path);
                if !resolved.exists() {
                    anyhow::bail!(
                        "{era} genesis file not found: {} (resolved from config dir {})",
                        resolved.display(),
                        config_dir.display()
                    );
                }
            }
            if let Some(ref hash_hex) = hash_opt {
                if hash_hex.len() != 64 || !hash_hex.chars().all(|c| c.is_ascii_hexdigit()) {
                    anyhow::bail!(
                        "{era} genesis hash is not a valid 64-character hex string: {hash_hex}"
                    );
                }
            }
        }

        Ok(())
    }
}

impl Default for NodeConfig {
    fn default() -> Self {
        NodeConfig {
            network: NetworkId::Mainnet,
            network_magic: None,
            protocol: Protocol::default(),
            requires_network_magic: None,
            shelley_genesis_file: None,
            byron_genesis_file: None,
            alonzo_genesis_file: None,
            conway_genesis_file: None,
            byron_genesis_hash: None,
            shelley_genesis_hash: None,
            alonzo_genesis_hash: None,
            conway_genesis_hash: None,
            enable_p2_p: true,
            target_number_of_active_peers: 20,
            target_number_of_established_peers: 40,
            target_number_of_known_peers: 100,
            trace_options: TraceOptions::default(),
            min_severity: "Info".to_string(),
            storage: None,
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
        let config = NodeConfig {
            network_magic: Some(42),
            ..NodeConfig::default()
        };
        assert_eq!(config.network_magic(), 42);
    }

    #[test]
    fn test_validate_default_config_passes() {
        let config = NodeConfig::default();
        assert!(config.validate(Path::new(".")).is_ok());
    }

    #[test]
    fn test_validate_missing_genesis_file() {
        let config = NodeConfig {
            shelley_genesis_file: Some("nonexistent-genesis.json".to_string()),
            ..NodeConfig::default()
        };
        let err = config.validate(Path::new(".")).unwrap_err();
        assert!(err.to_string().contains("Shelley genesis file not found"));
    }

    #[test]
    fn test_validate_invalid_genesis_hash_too_short() {
        let config = NodeConfig {
            byron_genesis_hash: Some("abcdef".to_string()),
            ..NodeConfig::default()
        };
        let err = config.validate(Path::new(".")).unwrap_err();
        assert!(err.to_string().contains("not a valid 64-character hex"));
    }

    #[test]
    fn test_validate_invalid_genesis_hash_non_hex() {
        let config = NodeConfig {
            alonzo_genesis_hash: Some(
                "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz".to_string(),
            ),
            ..NodeConfig::default()
        };
        let err = config.validate(Path::new(".")).unwrap_err();
        assert!(err.to_string().contains("Alonzo genesis hash"));
    }

    #[test]
    fn test_validate_valid_genesis_hash() {
        let config = NodeConfig {
            shelley_genesis_hash: Some(
                "363498d1024f84bb39d3fa9593ce391483cb40d479b87233f868d6e57c3a400d".to_string(),
            ),
            ..NodeConfig::default()
        };
        assert!(config.validate(Path::new(".")).is_ok());
    }
}
