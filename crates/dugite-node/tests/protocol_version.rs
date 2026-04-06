use dugite_node::config::NodeConfig;
use dugite_node::forge::BlockProducerConfig;
use dugite_primitives::block::ProtocolVersion;

// ---------------------------------------------------------------------------
// NodeConfig → node_protocol_version()
// ---------------------------------------------------------------------------

/// Default config (no ExperimentalHardForksEnabled) should produce ProtVer 10,8.
#[test]
fn default_config_protocol_version_is_10_8() {
    let json = r#"{}"#;
    let config: NodeConfig = serde_json::from_str(json).unwrap();
    let pv = config.node_protocol_version();
    assert_eq!(pv.major, 10);
    assert_eq!(pv.minor, 8);
}

/// ExperimentalHardForksEnabled=false should produce ProtVer 10,8.
#[test]
fn experimental_false_protocol_version_is_10_8() {
    let json = r#"{"ExperimentalHardForksEnabled": false}"#;
    let config: NodeConfig = serde_json::from_str(json).unwrap();
    let pv = config.node_protocol_version();
    assert_eq!(pv.major, 10);
    assert_eq!(pv.minor, 8);
}

/// ExperimentalHardForksEnabled=true should produce ProtVer 11,0.
#[test]
fn experimental_true_protocol_version_is_11_0() {
    let json = r#"{"ExperimentalHardForksEnabled": true}"#;
    let config: NodeConfig = serde_json::from_str(json).unwrap();
    let pv = config.node_protocol_version();
    assert_eq!(pv.major, 11);
    assert_eq!(pv.minor, 0);
}

// ---------------------------------------------------------------------------
// NodeConfig → max_major_protocol_version()
// ---------------------------------------------------------------------------

/// max_major_protocol_version() must equal node_protocol_version().major.
#[test]
fn max_major_protocol_version_matches_node_pv_major() {
    let default: NodeConfig = serde_json::from_str("{}").unwrap();
    assert_eq!(default.max_major_protocol_version(), 10);

    let experimental: NodeConfig =
        serde_json::from_str(r#"{"ExperimentalHardForksEnabled": true}"#).unwrap();
    assert_eq!(experimental.max_major_protocol_version(), 11);
}

// ---------------------------------------------------------------------------
// BlockProducerConfig default
// ---------------------------------------------------------------------------

/// Default BlockProducerConfig must match cardano-node 10.7.x (ProtVer 10,8).
#[test]
fn default_block_producer_config_matches_cardano_node() {
    let config = BlockProducerConfig::default();
    assert_eq!(
        config.protocol_version,
        ProtocolVersion { major: 10, minor: 8 },
        "Default BlockProducerConfig should match cardano-node 10.7.x (ProtVer 10,8)"
    );
}

/// BlockProducerConfig accepts custom protocol version for experimental mode.
#[test]
fn block_producer_config_accepts_experimental_version() {
    let config = BlockProducerConfig {
        protocol_version: ProtocolVersion { major: 11, minor: 0 },
        ..Default::default()
    };
    assert_eq!(config.protocol_version.major, 11);
    assert_eq!(config.protocol_version.minor, 0);
}

// ---------------------------------------------------------------------------
// End-to-end: NodeConfig → BlockProducerConfig
// ---------------------------------------------------------------------------

/// Verify that NodeConfig.node_protocol_version() produces a value suitable
/// for BlockProducerConfig in both default and experimental modes.
#[test]
fn config_to_block_producer_config_end_to_end() {
    // Default mode
    let node_config: NodeConfig = serde_json::from_str("{}").unwrap();
    let bp_config = BlockProducerConfig {
        protocol_version: node_config.node_protocol_version(),
        ..Default::default()
    };
    assert_eq!(bp_config.protocol_version, ProtocolVersion { major: 10, minor: 8 });

    // Experimental mode
    let node_config: NodeConfig =
        serde_json::from_str(r#"{"ExperimentalHardForksEnabled": true}"#).unwrap();
    let bp_config = BlockProducerConfig {
        protocol_version: node_config.node_protocol_version(),
        ..Default::default()
    };
    assert_eq!(bp_config.protocol_version, ProtocolVersion { major: 11, minor: 0 });
}
