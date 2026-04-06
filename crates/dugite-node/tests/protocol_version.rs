use dugite_node::config::NodeConfig;

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
