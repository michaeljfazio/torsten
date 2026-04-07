use anyhow::{Context, Result};
use dugite_primitives::block::ProtocolVersion;
use dugite_primitives::network::NetworkId;
use dugite_storage::StorageConfigJson;
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use std::path::Path;

/// Consensus protocol mode.
///
/// Matches cardano-node's `ConsensusMode`:
/// - `PraosMode`: Standard Ouroboros Praos operation.
/// - `GenesisMode`: Enables Ouroboros Genesis for trustless bulk sync.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConsensusMode {
    /// Standard Ouroboros Praos operation.
    #[default]
    PraosMode,
    /// Ouroboros Genesis for trustless bulk sync from untrusted peers.
    GenesisMode,
}

/// Inbound connection limits (matches Haskell AcceptedConnectionsLimit).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptedConnectionsLimit {
    /// Refuse new inbound connections beyond this count.
    #[serde(default = "default_hard_limit")]
    pub accepted_connections_hard_limit: u32,
    /// Start delaying new connections at this count.
    #[serde(default = "default_soft_limit")]
    pub accepted_connections_soft_limit: u32,
    /// Delay in seconds applied to connections above soft limit.
    #[serde(default = "default_conn_delay")]
    pub accepted_connections_delay: u32,
}

impl Default for AcceptedConnectionsLimit {
    fn default() -> Self {
        Self {
            accepted_connections_hard_limit: 512,
            accepted_connections_soft_limit: 384,
            accepted_connections_delay: 5,
        }
    }
}

/// Diffusion mode — controls whether the node accepts inbound N2N connections.
///
/// Matches cardano-node's `DiffusionMode` config field:
/// - `InitiatorAndResponder` (default): full P2P mode — opens listening port
///   and accepts inbound connections.
/// - `InitiatorOnly`: node only makes outbound connections, never listens for
///   inbound.  Advertises `initiator_only = true` in the N2N handshake so
///   remote peers do not attempt reverse connections.  Typical for block
///   producers behind a firewall.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffusionMode {
    /// Only initiate outbound connections (block producer behind NAT/firewall).
    InitiatorOnly,
    /// Both initiate outbound and accept inbound connections (relay).
    #[default]
    InitiatorAndResponder,
}

impl fmt::Display for DiffusionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InitiatorOnly => write!(f, "InitiatorOnly"),
            Self::InitiatorAndResponder => write!(f, "InitiatorAndResponder"),
        }
    }
}

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

    /// Diffusion mode — controls inbound connection acceptance.
    ///
    /// `"InitiatorAndResponder"` (default): full relay mode, accepts inbound.
    /// `"InitiatorOnly"`: block producer behind NAT, outbound only.
    /// Matches cardano-node's `DiffusionMode` config field.
    #[serde(default)]
    pub diffusion_mode: DiffusionMode,

    /// Enable peer sharing mini-protocol (default: `None` = auto).
    ///
    /// When `None`, peer sharing is automatically disabled for block producers
    /// (when `--shelley-kes-key` is provided) and enabled for relays — matching
    /// the Haskell cardano-node default behaviour.  Set explicitly to override.
    #[serde(default)]
    pub peer_sharing: Option<bool>,

    /// Target number of root peers (default: 60, matching cardano-node)
    #[serde(default = "default_root_peers")]
    pub target_number_of_root_peers: usize,

    /// Target number of active peers (default: 15, matching cardano-node)
    #[serde(default = "default_active_peers")]
    pub target_number_of_active_peers: usize,

    /// Target number of established peers (default: 40, matching cardano-node)
    #[serde(default = "default_established_peers")]
    pub target_number_of_established_peers: usize,

    /// Target number of known peers (default: 85, matching cardano-node)
    #[serde(default = "default_known_peers")]
    pub target_number_of_known_peers: usize,

    /// Target number of active big ledger peers (default: 5, matching cardano-node)
    #[serde(default = "default_active_big_ledger_peers")]
    pub target_number_of_active_big_ledger_peers: usize,

    /// Target number of established big ledger peers (default: 10, matching cardano-node)
    #[serde(default = "default_established_big_ledger_peers")]
    pub target_number_of_established_big_ledger_peers: usize,

    /// Target number of known big ledger peers (default: 15, matching cardano-node)
    #[serde(default = "default_known_big_ledger_peers")]
    pub target_number_of_known_big_ledger_peers: usize,

    /// Trace options
    #[serde(default)]
    pub trace_options: TraceOptions,

    /// Minimum severity for logging
    #[serde(default = "default_min_severity")]
    pub min_severity: String,

    /// Prometheus metrics port.
    ///
    /// When set to 0 the metrics server is disabled.  The CLI flag
    /// `--metrics-port` takes precedence over this field; the CLI flag
    /// `--no-metrics` forces the port to 0 regardless of this value.
    /// If neither the CLI flag nor this field is present the node falls back
    /// to the Cardano-node default of 12798.
    #[serde(default)]
    pub metrics_port: Option<u16>,

    /// Storage configuration (optional overrides for storage profiles)
    #[serde(default)]
    pub storage: Option<StorageConfigJson>,

    /// Governor churn interval during normal (caught-up) operation, in seconds.
    ///
    /// Controls how often the governor rotates a random subset of peers to
    /// ensure the node does not become permanently attached to the same set.
    /// Matches cardano-node default of 3300 s (55 minutes).
    #[serde(default = "default_churn_interval_normal_secs")]
    pub churn_interval_normal_secs: u64,

    /// Governor churn interval during syncing, in seconds.
    ///
    /// Faster rotation while syncing so that the node can quickly shed
    /// unresponsive peers.  Matches cardano-node default of 900 s (15 minutes).
    #[serde(default = "default_churn_interval_sync_secs")]
    pub churn_interval_sync_secs: u64,

    /// Number of consecutive governor evaluation cycles in which a hot peer
    /// must serve zero new blocks before it is demoted back to warm (stall
    /// detection).  A cycle runs every 30 seconds, so the default of 6 cycles
    /// corresponds to a 3-minute stall window.
    #[serde(default = "default_stall_demotion_cycles")]
    pub stall_demotion_cycles: u32,

    /// Failure count threshold above which a hot peer is unconditionally
    /// demoted to warm during each governor evaluation cycle.  Local root
    /// peers are exempt from this check and will never be demoted by the
    /// governor.  Default: 5 failures.
    #[serde(default = "default_error_demotion_threshold")]
    pub error_demotion_threshold: u32,

    /// Enable experimental hard fork transitions (default: false).
    ///
    /// When true, the node signals `ProtVer 11 0` in forged block headers,
    /// advertising readiness for the next major protocol version (Dijkstra era).
    /// When false (default), the node signals `ProtVer 10 8` — the maximum
    /// Conway-era protocol version supported by this software release.
    ///
    /// Matches cardano-node's `ExperimentalHardForksEnabled` config field.
    /// Must remain false on mainnet unless instructed otherwise.
    #[serde(default)]
    pub experimental_hard_forks_enabled: bool,

    /// Consensus protocol mode (PraosMode or GenesisMode).
    #[serde(default)]
    pub consensus_mode: ConsensusMode,

    // ── Genesis mode sync targets ──────────────────────────────────────
    /// Active peers during Genesis bulk sync (default: 0).
    #[serde(default)]
    pub sync_target_number_of_active_peers: usize,
    /// Established peers during Genesis bulk sync (default: 0).
    #[serde(default)]
    pub sync_target_number_of_established_peers: usize,
    /// Known peers during Genesis bulk sync (default: 0).
    #[serde(default)]
    pub sync_target_number_of_known_peers: usize,
    /// Root peers during Genesis bulk sync (default: 0).
    #[serde(default)]
    pub sync_target_number_of_root_peers: usize,
    /// Active big ledger peers during Genesis bulk sync (default: 30).
    #[serde(default = "default_sync_active_blp")]
    pub sync_target_number_of_active_big_ledger_peers: usize,
    /// Established big ledger peers during Genesis bulk sync (default: 50).
    #[serde(default = "default_sync_established_blp")]
    pub sync_target_number_of_established_big_ledger_peers: usize,
    /// Known big ledger peers during Genesis bulk sync (default: 100).
    #[serde(default = "default_sync_known_blp")]
    pub sync_target_number_of_known_big_ledger_peers: usize,
    /// Pause sync if active BLPs drop below this (Genesis safety gate, default: 5).
    #[serde(default = "default_min_blp_trusted")]
    pub min_big_ledger_peers_for_trusted_state: usize,

    // ── Connection management ──────────────────────────────────────────
    /// Inbound connection limits (hard/soft/delay).
    #[serde(default)]
    pub accepted_connections_limit: Option<AcceptedConnectionsLimit>,
    /// Time before idle mini-protocol connection is pruned (seconds, default: 5).
    ///
    /// Accepts fractional seconds, matching Haskell's `DiffTime` type.
    #[serde(default = "default_protocol_idle_timeout")]
    pub protocol_idle_timeout: f64,
    /// Connection TIME_WAIT duration after close (seconds, default: 60).
    ///
    /// Accepts fractional seconds, matching Haskell's `DiffTime` type.
    #[serde(default = "default_time_wait_timeout")]
    pub time_wait_timeout: f64,
    /// Outbound governor poll interval (seconds, default: 0).
    ///
    /// 0 means the governor runs as fast as events arrive (Haskell default).
    /// Accepts fractional seconds, matching Haskell's `DiffTime` type.
    #[serde(default = "default_egress_poll_interval")]
    pub egress_poll_interval: f64,
    /// ChainSync-specific idle timeout (seconds, 0 = no timeout).
    ///
    /// Accepts fractional seconds, matching Haskell's `DiffTime` type.
    #[serde(default)]
    pub chain_sync_idle_timeout: Option<f64>,
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

fn default_root_peers() -> usize {
    60
}

fn default_active_peers() -> usize {
    15
}

fn default_established_peers() -> usize {
    40
}

fn default_known_peers() -> usize {
    85
}

fn default_active_big_ledger_peers() -> usize {
    5
}

fn default_established_big_ledger_peers() -> usize {
    10
}

fn default_known_big_ledger_peers() -> usize {
    15
}

fn default_churn_interval_normal_secs() -> u64 {
    3300 // 55 minutes, matching cardano-node
}

fn default_churn_interval_sync_secs() -> u64 {
    900 // 15 minutes, matching cardano-node
}

fn default_stall_demotion_cycles() -> u32 {
    6 // 6 × 30 s = 3 minutes of inactivity triggers demotion
}

fn default_error_demotion_threshold() -> u32 {
    5 // 5 accumulated failures triggers demotion
}

fn default_hard_limit() -> u32 {
    512
}

fn default_soft_limit() -> u32 {
    384
}

fn default_conn_delay() -> u32 {
    5
}

fn default_sync_active_blp() -> usize {
    30
}

fn default_sync_established_blp() -> usize {
    50
}

fn default_sync_known_blp() -> usize {
    100
}

fn default_min_blp_trusted() -> usize {
    5
}

fn default_protocol_idle_timeout() -> f64 {
    5.0
}

fn default_time_wait_timeout() -> f64 {
    60.0
}

/// Haskell default is 0 — governor runs on-demand without a fixed poll interval.
fn default_egress_poll_interval() -> f64 {
    0.0
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
    /// Returns the protocol version this node should stamp on forged block headers.
    ///
    /// This is a **software capability signal**, not the on-chain protocol version.
    /// It tells the network the maximum protocol version this node supports.
    ///
    /// Matches cardano-node's `cardanoProtocolVersion` in `Cardano.Node.Protocol.Cardano.hs`:
    /// - `ExperimentalHardForksEnabled = false` → `ProtVer 10 8`
    /// - `ExperimentalHardForksEnabled = true`  → `ProtVer 11 0`
    pub fn node_protocol_version(&self) -> ProtocolVersion {
        if self.experimental_hard_forks_enabled {
            ProtocolVersion {
                major: 11,
                minor: 0,
            }
        } else {
            ProtocolVersion {
                major: 10,
                minor: 8,
            }
        }
    }

    /// Returns the maximum major protocol version this node can validate.
    ///
    /// Derived from the node protocol version's major component.
    /// Used by the Praos consensus layer for the obsolete-node envelope check:
    /// if the on-chain ledger protocol version exceeds this, the node rejects
    /// all block headers (forcing an upgrade).
    pub fn max_major_protocol_version(&self) -> u64 {
        self.node_protocol_version().major
    }

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

    /// Get effective network magic (from explicit field or network default)
    #[cfg(test)]
    pub fn network_magic(&self) -> u64 {
        self.network_magic.unwrap_or_else(|| self.network.magic())
    }

    /// Resolve effective peer sharing setting.
    ///
    /// If `peer_sharing` is explicitly set in the config, returns that value.
    /// Otherwise, returns `false` for block producers (when `is_block_producer`
    /// is true) and `true` for relays — matching Haskell cardano-node defaults.
    pub fn effective_peer_sharing(&self, is_block_producer: bool) -> bool {
        self.peer_sharing.unwrap_or(!is_block_producer)
    }

    /// Validate configuration at startup: check genesis file existence and hash formats.
    /// `config_dir` is the directory containing the config file, used to resolve
    /// relative genesis file paths.
    pub fn validate(&self, config_dir: &Path) -> Result<()> {
        // ── Peer target ordering ──────────────────────────────────────
        // Haskell cardano-node validates at startup that:
        //   known >= established >= active >= 0
        // Violation causes a startup failure with explicit error message.

        // Regular peer targets.
        if self.target_number_of_known_peers < self.target_number_of_established_peers {
            anyhow::bail!(
                "TargetNumberOfKnownPeers ({}) must be >= TargetNumberOfEstablishedPeers ({})",
                self.target_number_of_known_peers,
                self.target_number_of_established_peers,
            );
        }
        if self.target_number_of_established_peers < self.target_number_of_active_peers {
            anyhow::bail!(
                "TargetNumberOfEstablishedPeers ({}) must be >= TargetNumberOfActivePeers ({})",
                self.target_number_of_established_peers,
                self.target_number_of_active_peers,
            );
        }

        // Big Ledger Peer targets.
        if self.target_number_of_known_big_ledger_peers
            < self.target_number_of_established_big_ledger_peers
        {
            anyhow::bail!(
                "TargetNumberOfKnownBigLedgerPeers ({}) must be >= \
                 TargetNumberOfEstablishedBigLedgerPeers ({})",
                self.target_number_of_known_big_ledger_peers,
                self.target_number_of_established_big_ledger_peers,
            );
        }
        if self.target_number_of_established_big_ledger_peers
            < self.target_number_of_active_big_ledger_peers
        {
            anyhow::bail!(
                "TargetNumberOfEstablishedBigLedgerPeers ({}) must be >= \
                 TargetNumberOfActiveBigLedgerPeers ({})",
                self.target_number_of_established_big_ledger_peers,
                self.target_number_of_active_big_ledger_peers,
            );
        }

        // Sync targets (when Genesis mode is configured).
        if self.consensus_mode == ConsensusMode::GenesisMode {
            if self.sync_target_number_of_known_peers < self.sync_target_number_of_established_peers
            {
                anyhow::bail!(
                    "SyncTargetNumberOfKnownPeers ({}) must be >= \
                     SyncTargetNumberOfEstablishedPeers ({})",
                    self.sync_target_number_of_known_peers,
                    self.sync_target_number_of_established_peers,
                );
            }
            if self.sync_target_number_of_established_peers
                < self.sync_target_number_of_active_peers
            {
                anyhow::bail!(
                    "SyncTargetNumberOfEstablishedPeers ({}) must be >= \
                     SyncTargetNumberOfActivePeers ({})",
                    self.sync_target_number_of_established_peers,
                    self.sync_target_number_of_active_peers,
                );
            }
        }

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
            diffusion_mode: DiffusionMode::default(),
            peer_sharing: None,
            target_number_of_root_peers: 60,
            target_number_of_active_peers: 15,
            target_number_of_established_peers: 40,
            target_number_of_known_peers: 85,
            target_number_of_active_big_ledger_peers: 5,
            target_number_of_established_big_ledger_peers: 10,
            target_number_of_known_big_ledger_peers: 15,
            trace_options: TraceOptions::default(),
            min_severity: "Info".to_string(),
            metrics_port: None,
            storage: None,
            churn_interval_normal_secs: default_churn_interval_normal_secs(),
            churn_interval_sync_secs: default_churn_interval_sync_secs(),
            stall_demotion_cycles: default_stall_demotion_cycles(),
            error_demotion_threshold: default_error_demotion_threshold(),
            experimental_hard_forks_enabled: false,
            consensus_mode: ConsensusMode::default(),
            sync_target_number_of_active_peers: 0,
            sync_target_number_of_established_peers: 0,
            sync_target_number_of_known_peers: 0,
            sync_target_number_of_root_peers: 0,
            sync_target_number_of_active_big_ledger_peers: default_sync_active_blp(),
            sync_target_number_of_established_big_ledger_peers: default_sync_established_blp(),
            sync_target_number_of_known_big_ledger_peers: default_sync_known_blp(),
            min_big_ledger_peers_for_trusted_state: default_min_blp_trusted(),
            accepted_connections_limit: None,
            protocol_idle_timeout: default_protocol_idle_timeout(),
            time_wait_timeout: default_time_wait_timeout(),
            egress_poll_interval: default_egress_poll_interval(),
            chain_sync_idle_timeout: None,
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

    // ── MetricsPort config field ──────────────────────────────────────────────

    #[test]
    fn test_default_config_has_no_metrics_port() {
        // When the field is absent from the config file the operator gets None,
        // and the node binary falls back to the Cardano-node default of 12798.
        let config = NodeConfig::default();
        assert!(config.metrics_port.is_none());
    }

    #[test]
    fn test_metrics_port_from_json() {
        // Verify that "MetricsPort" is correctly deserialised from config JSON.
        let json = r#"{"MetricsPort": 9876}"#;
        let config: NodeConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.metrics_port, Some(9876));
    }

    #[test]
    fn test_metrics_port_zero_from_json() {
        // Port 0 in the config file should disable metrics (same semantics as
        // the --metrics-port 0 CLI flag).
        let json = r#"{"MetricsPort": 0}"#;
        let config: NodeConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.metrics_port, Some(0));
    }

    #[test]
    fn test_metrics_port_absent_from_json() {
        // Absence of the field must deserialise as None so the node can fall
        // through to the default port.
        let json = r#"{}"#;
        let config: NodeConfig = serde_json::from_str(json).unwrap();
        assert!(config.metrics_port.is_none());
    }

    #[test]
    fn test_metrics_port_round_trip_serialise() {
        // Confirm that a port value survives a JSON round-trip.
        let original = NodeConfig {
            metrics_port: Some(8080),
            ..NodeConfig::default()
        };
        let json = serde_json::to_string(&original).unwrap();
        let restored: NodeConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.metrics_port, Some(8080));
    }

    // ── Metrics port resolution priority ─────────────────────────────────────
    //
    // The node binary resolves the effective port with this priority:
    //   1. --no-metrics  → 0
    //   2. --metrics-port → explicit CLI value
    //   3. config MetricsPort → site-wide default from file
    //   4. 12798 (Cardano-node historical default)
    //
    // We test the rule table here using plain functions that mirror the
    // logic in run_node() so the tests stay fast and do not require spawning
    // an actual server.

    const CARDANO_DEFAULT_METRICS_PORT: u16 = 12798;

    fn resolve_metrics_port(no_metrics: bool, cli: Option<u16>, config: Option<u16>) -> u16 {
        if no_metrics {
            0
        } else if let Some(p) = cli {
            p
        } else {
            config.unwrap_or(CARDANO_DEFAULT_METRICS_PORT)
        }
    }

    #[test]
    fn test_resolve_no_metrics_flag_wins_over_all() {
        // --no-metrics must win even when a CLI port and a config port are set.
        assert_eq!(resolve_metrics_port(true, Some(9000), Some(8000)), 0);
    }

    #[test]
    fn test_resolve_cli_port_wins_over_config() {
        assert_eq!(resolve_metrics_port(false, Some(9000), Some(8000)), 9000);
    }

    #[test]
    fn test_resolve_config_port_used_when_no_cli() {
        assert_eq!(resolve_metrics_port(false, None, Some(8080)), 8080);
    }

    #[test]
    fn test_resolve_falls_back_to_default_12798() {
        assert_eq!(resolve_metrics_port(false, None, None), 12798);
    }

    #[test]
    fn test_resolve_cli_port_zero_disables_metrics() {
        // Passing --metrics-port 0 from the CLI should disable the server.
        assert_eq!(resolve_metrics_port(false, Some(0), None), 0);
    }

    #[test]
    fn test_resolve_config_port_zero_disables_metrics() {
        // Setting MetricsPort=0 in the config file should also disable the server.
        assert_eq!(resolve_metrics_port(false, None, Some(0)), 0);
    }

    // ── DiffusionMode config field ──────────────────────────────────────────

    #[test]
    fn test_default_diffusion_mode() {
        let config = NodeConfig::default();
        assert_eq!(config.diffusion_mode, DiffusionMode::InitiatorAndResponder);
    }

    #[test]
    fn test_diffusion_mode_initiator_only_from_json() {
        let json = r#"{"DiffusionMode": "InitiatorOnly"}"#;
        let config: NodeConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.diffusion_mode, DiffusionMode::InitiatorOnly);
    }

    #[test]
    fn test_diffusion_mode_initiator_and_responder_from_json() {
        let json = r#"{"DiffusionMode": "InitiatorAndResponder"}"#;
        let config: NodeConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.diffusion_mode, DiffusionMode::InitiatorAndResponder);
    }

    #[test]
    fn test_diffusion_mode_absent_defaults_to_initiator_and_responder() {
        let json = r#"{}"#;
        let config: NodeConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.diffusion_mode, DiffusionMode::InitiatorAndResponder);
    }

    #[test]
    fn test_diffusion_mode_display() {
        assert_eq!(DiffusionMode::InitiatorOnly.to_string(), "InitiatorOnly");
        assert_eq!(
            DiffusionMode::InitiatorAndResponder.to_string(),
            "InitiatorAndResponder"
        );
    }

    // ── PeerSharing config field ────────────────────────────────────────────

    #[test]
    fn test_default_peer_sharing_is_none() {
        let config = NodeConfig::default();
        assert!(config.peer_sharing.is_none());
    }

    #[test]
    fn test_peer_sharing_true_from_json() {
        let json = r#"{"PeerSharing": true}"#;
        let config: NodeConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.peer_sharing, Some(true));
    }

    #[test]
    fn test_peer_sharing_false_from_json() {
        let json = r#"{"PeerSharing": false}"#;
        let config: NodeConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.peer_sharing, Some(false));
    }

    #[test]
    fn test_effective_peer_sharing_auto_relay() {
        // Relay (not BP) with no explicit setting → enabled
        let config = NodeConfig::default();
        assert!(config.effective_peer_sharing(false));
    }

    #[test]
    fn test_effective_peer_sharing_auto_block_producer() {
        // Block producer with no explicit setting → disabled
        let config = NodeConfig::default();
        assert!(!config.effective_peer_sharing(true));
    }

    #[test]
    fn test_effective_peer_sharing_explicit_override() {
        // Explicit true overrides BP auto-disable
        let config = NodeConfig {
            peer_sharing: Some(true),
            ..NodeConfig::default()
        };
        assert!(config.effective_peer_sharing(true));
    }

    // ── ConsensusMode config field ──────────────────────────────────────────

    #[test]
    fn test_consensus_mode_default() {
        let config = NodeConfig::default();
        assert_eq!(config.consensus_mode, ConsensusMode::PraosMode);
    }

    #[test]
    fn test_consensus_mode_genesis_from_json() {
        let json = r#"{"ConsensusMode": "GenesisMode"}"#;
        let config: NodeConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.consensus_mode, ConsensusMode::GenesisMode);
    }

    // ── Genesis sync targets ────────────────────────────────────────────────

    #[test]
    fn test_sync_targets_from_json() {
        let json = r#"{
            "SyncTargetNumberOfActiveBigLedgerPeers": 25,
            "MinBigLedgerPeersForTrustedState": 10
        }"#;
        let config: NodeConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.sync_target_number_of_active_big_ledger_peers, 25);
        assert_eq!(config.min_big_ledger_peers_for_trusted_state, 10);
    }

    // ── AcceptedConnectionsLimit ────────────────────────────────────────────

    #[test]
    fn test_accepted_connections_limit_from_json() {
        let json = r#"{
            "AcceptedConnectionsLimit": {
                "acceptedConnectionsHardLimit": 256,
                "acceptedConnectionsSoftLimit": 200,
                "acceptedConnectionsDelay": 2
            }
        }"#;
        let config: NodeConfig = serde_json::from_str(json).unwrap();
        let limit = config.accepted_connections_limit.unwrap();
        assert_eq!(limit.accepted_connections_hard_limit, 256);
        assert_eq!(limit.accepted_connections_soft_limit, 200);
        assert_eq!(limit.accepted_connections_delay, 2);
    }

    // ── Connection timeouts ─────────────────────────────────────────────────

    #[test]
    fn test_connection_timeouts_from_json() {
        let json = r#"{
            "ProtocolIdleTimeout": 10,
            "TimeWaitTimeout": 120,
            "EgressPollInterval": 20
        }"#;
        let config: NodeConfig = serde_json::from_str(json).unwrap();
        assert!((config.protocol_idle_timeout - 10.0_f64).abs() < f64::EPSILON);
        assert!((config.time_wait_timeout - 120.0_f64).abs() < f64::EPSILON);
        assert!((config.egress_poll_interval - 20.0_f64).abs() < f64::EPSILON);
    }

    #[test]
    fn test_connection_timeouts_fractional() {
        // Fractional seconds must parse correctly — Haskell uses DiffTime.
        let json = r#"{
            "ProtocolIdleTimeout": 5.5,
            "TimeWaitTimeout": 60.25,
            "EgressPollInterval": 0.1,
            "ChainSyncIdleTimeout": 3373.5
        }"#;
        let config: NodeConfig = serde_json::from_str(json).unwrap();
        assert!((config.protocol_idle_timeout - 5.5_f64).abs() < f64::EPSILON);
        assert!((config.time_wait_timeout - 60.25_f64).abs() < f64::EPSILON);
        assert!((config.egress_poll_interval - 0.1_f64).abs() < f64::EPSILON);
        assert!((config.chain_sync_idle_timeout.unwrap() - 3373.5_f64).abs() < f64::EPSILON);
    }

    // ── All new fields absent → defaults ────────────────────────────────────

    #[test]
    fn test_new_config_fields_absent_use_defaults() {
        let json = r#"{}"#;
        let config: NodeConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.consensus_mode, ConsensusMode::PraosMode);
        assert_eq!(config.sync_target_number_of_active_big_ledger_peers, 30);
        assert_eq!(
            config.sync_target_number_of_established_big_ledger_peers,
            50
        );
        assert_eq!(config.sync_target_number_of_known_big_ledger_peers, 100);
        assert_eq!(config.min_big_ledger_peers_for_trusted_state, 5);
        assert!((config.protocol_idle_timeout - 5.0_f64).abs() < f64::EPSILON);
        assert!((config.time_wait_timeout - 60.0_f64).abs() < f64::EPSILON);
        assert!((config.egress_poll_interval - 0.0_f64).abs() < f64::EPSILON);
        assert!(config.accepted_connections_limit.is_none());
        assert!(config.chain_sync_idle_timeout.is_none());
    }

    // ── Peer target ordering validation tests ─────────────────────────

    #[test]
    fn test_validate_known_less_than_established_fails() {
        let config = NodeConfig {
            target_number_of_known_peers: 10,
            target_number_of_established_peers: 20,
            ..NodeConfig::default()
        };
        let err = config.validate(Path::new(".")).unwrap_err();
        assert!(err.to_string().contains("TargetNumberOfKnownPeers"));
    }

    #[test]
    fn test_validate_established_less_than_active_fails() {
        let config = NodeConfig {
            target_number_of_established_peers: 5,
            target_number_of_active_peers: 10,
            ..NodeConfig::default()
        };
        let err = config.validate(Path::new(".")).unwrap_err();
        assert!(err.to_string().contains("TargetNumberOfEstablishedPeers"));
    }

    #[test]
    fn test_validate_blp_known_less_than_established_fails() {
        let config = NodeConfig {
            target_number_of_known_big_ledger_peers: 3,
            target_number_of_established_big_ledger_peers: 10,
            ..NodeConfig::default()
        };
        let err = config.validate(Path::new(".")).unwrap_err();
        assert!(err.to_string().contains("BigLedgerPeers"));
    }

    #[test]
    fn test_validate_genesis_sync_targets() {
        let config = NodeConfig {
            consensus_mode: ConsensusMode::GenesisMode,
            sync_target_number_of_known_peers: 5,
            sync_target_number_of_established_peers: 10,
            ..NodeConfig::default()
        };
        let err = config.validate(Path::new(".")).unwrap_err();
        assert!(err.to_string().contains("SyncTargetNumberOfKnownPeers"));
    }

    #[test]
    fn test_validate_sync_targets_skipped_in_praos_mode() {
        // Same invalid sync targets but in PraosMode — should pass.
        let config = NodeConfig {
            consensus_mode: ConsensusMode::PraosMode,
            sync_target_number_of_known_peers: 5,
            sync_target_number_of_established_peers: 10,
            ..NodeConfig::default()
        };
        assert!(config.validate(Path::new(".")).is_ok());
    }
}
