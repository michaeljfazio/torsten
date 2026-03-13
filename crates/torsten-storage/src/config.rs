//! Storage configuration types and profiles.
//!
//! Operators choose a preset (`high-memory`, `low-memory`) or individually
//! tune parameters. Both profiles use memory-mapped block indexes by default
//! (benchmarks show 3-4x faster lookups and 4x faster open at scale vs
//! in-memory HashMap). The `low-memory` profile reduces LSM cache sizes
//! for constrained environments.

use serde::{Deserialize, Serialize};

/// Storage profile preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StorageProfile {
    HighMemory,
    LowMemory,
}

impl StorageProfile {
    /// Produce the base configuration for this profile.
    pub fn to_config(self) -> StorageConfig {
        match self {
            StorageProfile::HighMemory => StorageConfig {
                immutable: ImmutableConfig {
                    index_type: BlockIndexType::Mmap,
                    mmap_load_factor: 0.7,
                    mmap_initial_capacity: 0,
                },
                utxo: UtxoConfig {
                    backend: UtxoBackend::Lsm,
                    memtable_size_mb: 128,
                    block_cache_size_mb: 256,
                    bloom_filter_bits_per_key: 10,
                },
            },
            StorageProfile::LowMemory => StorageConfig {
                immutable: ImmutableConfig {
                    index_type: BlockIndexType::Mmap,
                    mmap_load_factor: 0.7,
                    mmap_initial_capacity: 0,
                },
                utxo: UtxoConfig {
                    backend: UtxoBackend::Lsm,
                    memtable_size_mb: 64,
                    block_cache_size_mb: 128,
                    bloom_filter_bits_per_key: 10,
                },
            },
        }
    }
}

impl std::str::FromStr for StorageProfile {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "high-memory" => Ok(StorageProfile::HighMemory),
            "low-memory" => Ok(StorageProfile::LowMemory),
            other => Err(format!(
                "unknown storage profile '{other}', expected 'high-memory' or 'low-memory'"
            )),
        }
    }
}

impl std::fmt::Display for StorageProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageProfile::HighMemory => write!(f, "high-memory"),
            StorageProfile::LowMemory => write!(f, "low-memory"),
        }
    }
}

/// Top-level storage configuration.
#[derive(Debug, Clone)]
pub struct StorageConfig {
    pub immutable: ImmutableConfig,
    pub utxo: UtxoConfig,
}

impl Default for StorageConfig {
    fn default() -> Self {
        StorageProfile::HighMemory.to_config()
    }
}

/// ImmutableDB configuration.
#[derive(Debug, Clone)]
pub struct ImmutableConfig {
    /// Type of block index to use.
    pub index_type: BlockIndexType,
    /// Load factor for the mmap hash table (0.0–1.0). Only used in Mmap mode.
    pub mmap_load_factor: f64,
    /// Initial capacity for the mmap hash table. 0 = auto-detect from secondary indexes.
    pub mmap_initial_capacity: u64,
}

impl Default for ImmutableConfig {
    fn default() -> Self {
        ImmutableConfig {
            index_type: BlockIndexType::Mmap,
            mmap_load_factor: 0.7,
            mmap_initial_capacity: 0,
        }
    }
}

/// Block index implementation strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BlockIndexType {
    /// In-memory HashMap (higher RAM usage, slower lookups than Mmap).
    InMemory,
    /// Memory-mapped on-disk hash table (low RAM, 3-4x faster lookups, faster open at scale).
    Mmap,
}

impl std::str::FromStr for BlockIndexType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "in-memory" => Ok(BlockIndexType::InMemory),
            "mmap" => Ok(BlockIndexType::Mmap),
            other => Err(format!(
                "unknown index type '{other}', expected 'in-memory' or 'mmap'"
            )),
        }
    }
}

/// UTxO store configuration.
#[derive(Debug, Clone)]
pub struct UtxoConfig {
    /// UTxO storage backend.
    pub backend: UtxoBackend,
    /// LSM memtable size in MB.
    pub memtable_size_mb: u64,
    /// LSM block cache size in MB.
    pub block_cache_size_mb: u64,
    /// LSM bloom filter bits per key.
    pub bloom_filter_bits_per_key: u32,
}

impl Default for UtxoConfig {
    fn default() -> Self {
        UtxoConfig {
            backend: UtxoBackend::InMemory,
            memtable_size_mb: 128,
            block_cache_size_mb: 256,
            bloom_filter_bits_per_key: 10,
        }
    }
}

/// UTxO storage backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UtxoBackend {
    /// In-memory HashMap (default, current behavior).
    InMemory,
    /// On-disk LSM tree via cardano-lsm.
    Lsm,
}

impl std::str::FromStr for UtxoBackend {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "in-memory" => Ok(UtxoBackend::InMemory),
            "lsm" => Ok(UtxoBackend::Lsm),
            other => Err(format!(
                "unknown UTxO backend '{other}', expected 'in-memory' or 'lsm'"
            )),
        }
    }
}

/// JSON-serializable storage configuration section for the config file.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StorageConfigJson {
    /// Storage profile name (base defaults).
    #[serde(default)]
    pub profile: Option<String>,
    /// Override: block index type.
    #[serde(default)]
    pub immutable_index_type: Option<String>,
    /// Override: mmap load factor.
    #[serde(default)]
    pub mmap_load_factor: Option<f64>,
    /// Override: UTxO backend.
    #[serde(default)]
    pub utxo_backend: Option<String>,
    /// Override: LSM memtable size in MB.
    #[serde(default)]
    pub utxo_memtable_size_mb: Option<u64>,
    /// Override: LSM block cache size in MB.
    #[serde(default)]
    pub utxo_block_cache_size_mb: Option<u64>,
    /// Override: LSM bloom filter bits per key.
    #[serde(default)]
    pub utxo_bloom_filter_bits: Option<u32>,
}

/// Resolve the final StorageConfig from profile + config file + CLI overrides.
///
/// Resolution order: profile defaults < config file overrides < CLI overrides.
pub fn resolve_storage_config(
    profile: StorageProfile,
    config_json: Option<&StorageConfigJson>,
    cli_immutable_index_type: Option<&str>,
    cli_utxo_backend: Option<&str>,
    cli_utxo_memtable_size_mb: Option<u64>,
    cli_utxo_block_cache_size_mb: Option<u64>,
    cli_utxo_bloom_filter_bits: Option<u32>,
) -> Result<StorageConfig, String> {
    let mut config = profile.to_config();

    // Apply config file overrides
    if let Some(json) = config_json {
        if let Some(ref idx) = json.immutable_index_type {
            config.immutable.index_type = idx.parse()?;
        }
        if let Some(lf) = json.mmap_load_factor {
            config.immutable.mmap_load_factor = lf;
        }
        if let Some(ref backend) = json.utxo_backend {
            config.utxo.backend = backend.parse()?;
        }
        if let Some(mb) = json.utxo_memtable_size_mb {
            config.utxo.memtable_size_mb = mb;
        }
        if let Some(mb) = json.utxo_block_cache_size_mb {
            config.utxo.block_cache_size_mb = mb;
        }
        if let Some(bits) = json.utxo_bloom_filter_bits {
            config.utxo.bloom_filter_bits_per_key = bits;
        }
    }

    // Apply CLI overrides (highest priority)
    if let Some(idx) = cli_immutable_index_type {
        config.immutable.index_type = idx.parse()?;
    }
    if let Some(backend) = cli_utxo_backend {
        config.utxo.backend = backend.parse()?;
    }
    if let Some(mb) = cli_utxo_memtable_size_mb {
        config.utxo.memtable_size_mb = mb;
    }
    if let Some(mb) = cli_utxo_block_cache_size_mb {
        config.utxo.block_cache_size_mb = mb;
    }
    if let Some(bits) = cli_utxo_bloom_filter_bits {
        config.utxo.bloom_filter_bits_per_key = bits;
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_high_memory_profile_defaults() {
        let config = StorageProfile::HighMemory.to_config();
        assert_eq!(config.immutable.index_type, BlockIndexType::Mmap);
        assert_eq!(config.utxo.backend, UtxoBackend::Lsm);
        assert_eq!(config.utxo.memtable_size_mb, 128);
        assert_eq!(config.utxo.block_cache_size_mb, 256);
        assert_eq!(config.utxo.bloom_filter_bits_per_key, 10);
    }

    #[test]
    fn test_low_memory_profile_defaults() {
        let config = StorageProfile::LowMemory.to_config();
        assert_eq!(config.immutable.index_type, BlockIndexType::Mmap);
        assert_eq!(config.utxo.backend, UtxoBackend::Lsm);
        assert_eq!(config.utxo.memtable_size_mb, 64);
        assert_eq!(config.utxo.block_cache_size_mb, 128);
    }

    #[test]
    fn test_profile_from_str() {
        assert_eq!(
            "high-memory".parse::<StorageProfile>().unwrap(),
            StorageProfile::HighMemory
        );
        assert_eq!(
            "low-memory".parse::<StorageProfile>().unwrap(),
            StorageProfile::LowMemory
        );
        assert!("unknown".parse::<StorageProfile>().is_err());
    }

    #[test]
    fn test_block_index_type_from_str() {
        assert_eq!(
            "in-memory".parse::<BlockIndexType>().unwrap(),
            BlockIndexType::InMemory
        );
        assert_eq!(
            "mmap".parse::<BlockIndexType>().unwrap(),
            BlockIndexType::Mmap
        );
        assert!("bad".parse::<BlockIndexType>().is_err());
    }

    #[test]
    fn test_utxo_backend_from_str() {
        assert_eq!(
            "in-memory".parse::<UtxoBackend>().unwrap(),
            UtxoBackend::InMemory
        );
        assert_eq!("lsm".parse::<UtxoBackend>().unwrap(), UtxoBackend::Lsm);
        assert!("bad".parse::<UtxoBackend>().is_err());
    }

    #[test]
    fn test_cli_override_beats_profile() {
        let config = resolve_storage_config(
            StorageProfile::HighMemory,
            None,
            Some("mmap"),
            Some("in-memory"),
            Some(32),
            Some(64),
            Some(5),
        )
        .unwrap();

        assert_eq!(config.immutable.index_type, BlockIndexType::Mmap);
        assert_eq!(config.utxo.backend, UtxoBackend::InMemory);
        assert_eq!(config.utxo.memtable_size_mb, 32);
        assert_eq!(config.utxo.block_cache_size_mb, 64);
        assert_eq!(config.utxo.bloom_filter_bits_per_key, 5);
    }

    #[test]
    fn test_config_file_override() {
        let json = StorageConfigJson {
            profile: None,
            immutable_index_type: Some("mmap".to_string()),
            mmap_load_factor: None,
            utxo_backend: None,
            utxo_memtable_size_mb: Some(96),
            utxo_block_cache_size_mb: None,
            utxo_bloom_filter_bits: None,
        };
        let config = resolve_storage_config(
            StorageProfile::HighMemory,
            Some(&json),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        assert_eq!(config.immutable.index_type, BlockIndexType::Mmap);
        assert_eq!(config.utxo.memtable_size_mb, 96);
        // Unchanged from profile
        assert_eq!(config.utxo.block_cache_size_mb, 256);
    }

    #[test]
    fn test_resolution_order_profile_config_cli() {
        // Profile: high-memory (memtable=128)
        // Config: memtable=96
        // CLI: memtable=32
        let json = StorageConfigJson {
            utxo_memtable_size_mb: Some(96),
            ..Default::default()
        };
        let config = resolve_storage_config(
            StorageProfile::HighMemory,
            Some(&json),
            None,
            None,
            Some(32),
            None,
            None,
        )
        .unwrap();
        // CLI wins
        assert_eq!(config.utxo.memtable_size_mb, 32);
    }

    #[test]
    fn test_config_file_json_parsing() {
        let json_str = r#"{
            "profile": "low-memory",
            "immutableIndexType": "mmap",
            "utxoMemtableSizeMb": 64
        }"#;
        let parsed: StorageConfigJson = serde_json::from_str(json_str).unwrap();
        assert_eq!(parsed.profile.as_deref(), Some("low-memory"));
        assert_eq!(parsed.immutable_index_type.as_deref(), Some("mmap"));
        assert_eq!(parsed.utxo_memtable_size_mb, Some(64));
    }

    #[test]
    fn test_default_storage_config() {
        // Default StorageConfig uses HighMemory profile
        let config = StorageConfig::default();
        assert_eq!(config.immutable.index_type, BlockIndexType::Mmap);
        assert_eq!(config.utxo.backend, UtxoBackend::Lsm);
        assert_eq!(config.utxo.memtable_size_mb, 128);
        assert_eq!(config.utxo.block_cache_size_mb, 256);
    }

    #[test]
    fn test_default_immutable_config() {
        let config = ImmutableConfig::default();
        assert_eq!(config.index_type, BlockIndexType::Mmap);
        assert_eq!(config.mmap_load_factor, 0.7);
        assert_eq!(config.mmap_initial_capacity, 0);
    }

    #[test]
    fn test_default_utxo_config() {
        let config = UtxoConfig::default();
        assert_eq!(config.backend, UtxoBackend::InMemory);
        assert_eq!(config.memtable_size_mb, 128);
        assert_eq!(config.block_cache_size_mb, 256);
        assert_eq!(config.bloom_filter_bits_per_key, 10);
    }

    #[test]
    fn test_profile_display() {
        assert_eq!(StorageProfile::HighMemory.to_string(), "high-memory");
        assert_eq!(StorageProfile::LowMemory.to_string(), "low-memory");
    }

    #[test]
    fn test_resolve_all_none_overrides() {
        // No overrides → pure profile config
        let config = resolve_storage_config(
            StorageProfile::LowMemory,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(config.immutable.index_type, BlockIndexType::Mmap);
        assert_eq!(config.utxo.memtable_size_mb, 64);
        assert_eq!(config.utxo.block_cache_size_mb, 128);
    }

    #[test]
    fn test_resolve_invalid_index_type() {
        let result = resolve_storage_config(
            StorageProfile::HighMemory,
            None,
            Some("bad-type"),
            None,
            None,
            None,
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_invalid_utxo_backend() {
        let result = resolve_storage_config(
            StorageProfile::HighMemory,
            None,
            None,
            Some("invalid"),
            None,
            None,
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_config_json_all_fields() {
        let json_str = r#"{
            "profile": "high-memory",
            "immutableIndexType": "in-memory",
            "mmapLoadFactor": 0.8,
            "utxoBackend": "lsm",
            "utxoMemtableSizeMb": 256,
            "utxoBlockCacheSizeMb": 512,
            "utxoBloomFilterBits": 15
        }"#;
        let parsed: StorageConfigJson = serde_json::from_str(json_str).unwrap();
        assert_eq!(parsed.profile.as_deref(), Some("high-memory"));
        assert_eq!(parsed.immutable_index_type.as_deref(), Some("in-memory"));
        assert_eq!(parsed.mmap_load_factor, Some(0.8));
        assert_eq!(parsed.utxo_backend.as_deref(), Some("lsm"));
        assert_eq!(parsed.utxo_memtable_size_mb, Some(256));
        assert_eq!(parsed.utxo_block_cache_size_mb, Some(512));
        assert_eq!(parsed.utxo_bloom_filter_bits, Some(15));
    }

    #[test]
    fn test_config_json_empty_object() {
        let parsed: StorageConfigJson = serde_json::from_str("{}").unwrap();
        assert!(parsed.profile.is_none());
        assert!(parsed.immutable_index_type.is_none());
        assert!(parsed.utxo_backend.is_none());
        assert!(parsed.utxo_memtable_size_mb.is_none());
    }

    #[test]
    fn test_config_json_partial_overrides() {
        // Config file sets only memtable size; everything else stays at profile defaults
        let json = StorageConfigJson {
            utxo_memtable_size_mb: Some(96),
            ..Default::default()
        };
        let config = resolve_storage_config(
            StorageProfile::HighMemory,
            Some(&json),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        // Changed
        assert_eq!(config.utxo.memtable_size_mb, 96);
        // Unchanged from HighMemory profile
        assert_eq!(config.immutable.index_type, BlockIndexType::Mmap);
        assert_eq!(config.utxo.backend, UtxoBackend::Lsm);
        assert_eq!(config.utxo.block_cache_size_mb, 256);
        assert_eq!(config.utxo.bloom_filter_bits_per_key, 10);
    }

    #[test]
    fn test_config_file_mmap_load_factor_override() {
        let json = StorageConfigJson {
            mmap_load_factor: Some(0.5),
            ..Default::default()
        };
        let config = resolve_storage_config(
            StorageProfile::LowMemory,
            Some(&json),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(config.immutable.mmap_load_factor, 0.5);
    }

    #[test]
    fn test_cli_overrides_config_file() {
        // Config file says memtable=96, CLI says memtable=32 → CLI wins
        let json = StorageConfigJson {
            utxo_memtable_size_mb: Some(96),
            utxo_backend: Some("lsm".to_string()),
            ..Default::default()
        };
        let config = resolve_storage_config(
            StorageProfile::HighMemory,
            Some(&json),
            None,
            Some("in-memory"), // CLI overrides config file's "lsm"
            Some(32),          // CLI overrides config file's 96
            None,
            None,
        )
        .unwrap();
        assert_eq!(config.utxo.backend, UtxoBackend::InMemory);
        assert_eq!(config.utxo.memtable_size_mb, 32);
    }
}
