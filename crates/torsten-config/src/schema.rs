//! Parameter schema — known Cardano node configuration parameters.
//!
//! Each [`ParamDef`] describes a single JSON key that may appear in a Cardano
//! node configuration file: its display name, the logical section it belongs
//! to, its value type (with optional constraints), a human-readable
//! description, and its documented default value.
//!
//! Unknown keys found in the loaded JSON file are displayed as raw JSON values
//! (editable as strings) and reported under the [`SECTION_UNKNOWN`] sentinel.
//!
//! # Sections
//!
//! Parameters are grouped into logical sections for the left-panel tree:
//!
//! | Section  | Contents                                                  |
//! |----------|-----------------------------------------------------------|
//! | Network  | P2P flags, peer targets, network magic                    |
//! | Genesis  | Paths and hashes for all four genesis files               |
//! | Protocol | Protocol name, Cardano mode, HFC flags                    |
//! | Logging  | Minimum severity, tracers, log format                     |
//! | Advanced | Performance knobs, memory limits, etc.                    |

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Section identifiers
// ---------------------------------------------------------------------------

/// Section name for parameters that have no known definition.
pub const SECTION_UNKNOWN: &str = "Unknown";

// ---------------------------------------------------------------------------
// Value type
// ---------------------------------------------------------------------------

/// The type of a configuration parameter's value.
///
/// Used both to validate user edits and to drive the appropriate in-place
/// editor widget (toggle, free-form text input, or enum cycling).
#[derive(Debug, Clone, PartialEq)]
pub enum ParamType {
    /// A JSON boolean (`true` / `false`).
    Bool,
    /// An unsigned integer in the range `[min, max]`.
    U64 { min: u64, max: u64 },
    /// A free-form UTF-8 string (no validation beyond non-empty).
    String,
    /// One of a fixed set of string values (cycled with arrow keys).
    Enum { values: &'static [&'static str] },
    /// A file-system path (stored as a JSON string, shown with a path icon).
    Path,
}

impl ParamType {
    /// Return a short display label for the type (shown in the description panel).
    pub fn label(&self) -> &'static str {
        match self {
            ParamType::Bool => "bool",
            ParamType::U64 { .. } => "u64",
            ParamType::String => "string",
            ParamType::Enum { .. } => "enum",
            ParamType::Path => "path",
        }
    }

    /// Validate a raw string edit value against this type.
    ///
    /// Returns `Ok(())` if the value is acceptable, or an error message that
    /// can be shown to the user in the footer.
    pub fn validate(&self, raw: &str) -> Result<(), String> {
        match self {
            ParamType::Bool => {
                if raw == "true" || raw == "false" {
                    Ok(())
                } else {
                    Err(format!("must be 'true' or 'false', got '{raw}'"))
                }
            }
            ParamType::U64 { min, max } => raw
                .parse::<u64>()
                .map_err(|_| format!("must be an integer, got '{raw}'"))
                .and_then(|v| {
                    if v >= *min && v <= *max {
                        Ok(())
                    } else {
                        Err(format!("must be between {min} and {max}, got {v}"))
                    }
                }),
            ParamType::String | ParamType::Path => Ok(()),
            ParamType::Enum { values } => {
                if values.contains(&raw) {
                    Ok(())
                } else {
                    Err(format!(
                        "must be one of [{}], got '{raw}'",
                        values.join(", ")
                    ))
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Parameter definition
// ---------------------------------------------------------------------------

/// A single known configuration parameter.
#[derive(Debug, Clone)]
pub struct ParamDef {
    /// The JSON key exactly as it appears in the config file.
    pub key: &'static str,
    /// The logical section this parameter belongs to.
    pub section: &'static str,
    /// The value type (drives validation and editor mode).
    pub param_type: ParamType,
    /// Default value as a display string (informational only).
    pub default: &'static str,
    /// Human-readable description shown in the right-hand description panel.
    pub description: &'static str,
}

// ---------------------------------------------------------------------------
// Known parameter table
// ---------------------------------------------------------------------------

/// All known Cardano node configuration parameters, in section/display order.
///
/// When a key from the loaded JSON file matches an entry here, its metadata is
/// used for display and validation. Unknown keys fall back to raw-string
/// editing under the [`SECTION_UNKNOWN`] section.
pub static KNOWN_PARAMS: &[ParamDef] = &[
    // --- Network section ---------------------------------------------------
    ParamDef {
        key: "Network",
        section: "Network",
        param_type: ParamType::Enum {
            values: &["Mainnet", "Testnet"],
        },
        default: "Mainnet",
        description: "Cardano network identifier. 'Mainnet' for the main chain; \
                      'Testnet' for any test network (requires NetworkMagic).",
    },
    ParamDef {
        key: "NetworkMagic",
        section: "Network",
        param_type: ParamType::U64 {
            min: 0,
            max: u64::MAX,
        },
        default: "764824073",
        description: "Network magic number. Mainnet = 764824073, Preview = 2, Preprod = 1. \
                      Must match the genesis files and all connecting peers.",
    },
    ParamDef {
        key: "RequiresNetworkMagic",
        section: "Network",
        param_type: ParamType::Enum {
            values: &["RequiresNoMagic", "RequiresMagic"],
        },
        default: "RequiresMagic",
        description: "Controls whether the network magic is enforced on peer handshakes. \
                      Use 'RequiresMagic' for all non-mainnet deployments.",
    },
    ParamDef {
        key: "EnableP2P",
        section: "Network",
        param_type: ParamType::Bool,
        default: "true",
        description: "Enable the Ouroboros P2P networking stack. When true, Torsten \
                      uses the diffusion layer for peer discovery and connection management. \
                      Set to false only for legacy non-P2P relay configurations.",
    },
    ParamDef {
        key: "PeerSharing",
        section: "Network",
        param_type: ParamType::Enum {
            values: &["NoPeerSharing", "PeerSharingPrivate", "PeerSharingPublic"],
        },
        default: "PeerSharingPublic",
        description: "Peer sharing policy. 'PeerSharingPublic' allows this node to share \
                      its known peers with others. 'PeerSharingPrivate' refuses peer share \
                      requests. 'NoPeerSharing' disables the mini-protocol entirely.",
    },
    ParamDef {
        key: "TargetNumberOfActivePeers",
        section: "Network",
        param_type: ParamType::U64 { min: 1, max: 100 },
        default: "15",
        description: "Target number of fully active (hot) peers — connections where \
                      block headers and bodies are exchanged. Raising this improves \
                      propagation at the cost of higher CPU and bandwidth.",
    },
    ParamDef {
        key: "TargetNumberOfEstablishedPeers",
        section: "Network",
        param_type: ParamType::U64 { min: 1, max: 200 },
        default: "40",
        description: "Target number of established (warm) peers — TCP connections that \
                      are open but not yet doing full block exchange. Acts as a reservoir \
                      to promote to hot when needed.",
    },
    ParamDef {
        key: "TargetNumberOfKnownPeers",
        section: "Network",
        param_type: ParamType::U64 { min: 1, max: 500 },
        default: "85",
        description: "Target size of the known-peers set (cold + warm + hot). The peer \
                      governor will attempt to keep at least this many addresses in its \
                      address book at all times.",
    },
    ParamDef {
        key: "TargetNumberOfRootPeers",
        section: "Network",
        param_type: ParamType::U64 { min: 1, max: 50 },
        default: "60",
        description: "Target number of root peers — connections maintained to the \
                      topology file entries (trusted relays). These anchor the node to \
                      the network before ledger peer discovery kicks in.",
    },
    ParamDef {
        key: "TargetNumberOfActiveBigLedgerPeers",
        section: "Network",
        param_type: ParamType::U64 { min: 0, max: 50 },
        default: "5",
        description: "Target number of active connections to 'big ledger' peers — \
                      well-staked SPO relays discovered from the on-chain pool params \
                      after useLedgerAfterSlot is reached.",
    },
    ParamDef {
        key: "TargetNumberOfEstablishedBigLedgerPeers",
        section: "Network",
        param_type: ParamType::U64 { min: 0, max: 100 },
        default: "10",
        description: "Target number of established (warm) connections to big ledger peers.",
    },
    ParamDef {
        key: "TargetNumberOfKnownBigLedgerPeers",
        section: "Network",
        param_type: ParamType::U64 { min: 0, max: 200 },
        default: "15",
        description: "Target size of the known big-ledger-peer set (cold + warm + hot).",
    },
    // --- Genesis section ---------------------------------------------------
    ParamDef {
        key: "ByronGenesisFile",
        section: "Genesis",
        param_type: ParamType::Path,
        default: "byron-genesis.json",
        description: "Path to the Byron-era genesis JSON file. Can be relative to the \
                      config file's directory or absolute. Must match ByronGenesisHash.",
    },
    ParamDef {
        key: "ByronGenesisHash",
        section: "Genesis",
        param_type: ParamType::String,
        default: "",
        description: "Blake2b-256 hash (hex) of the Byron genesis file. \
                      The node verifies this on startup to detect genesis mismatches.",
    },
    ParamDef {
        key: "ShelleyGenesisFile",
        section: "Genesis",
        param_type: ParamType::Path,
        default: "shelley-genesis.json",
        description: "Path to the Shelley-era genesis JSON file. Contains network \
                      parameters, initial delegation, protocol magic, and epoch length.",
    },
    ParamDef {
        key: "ShelleyGenesisHash",
        section: "Genesis",
        param_type: ParamType::String,
        default: "",
        description: "Blake2b-256 hash (hex) of the Shelley genesis file.",
    },
    ParamDef {
        key: "AlonzoGenesisFile",
        section: "Genesis",
        param_type: ParamType::Path,
        default: "alonzo-genesis.json",
        description: "Path to the Alonzo-era genesis JSON file. Contains initial Plutus \
                      cost model parameters and collateral percentage.",
    },
    ParamDef {
        key: "AlonzoGenesisHash",
        section: "Genesis",
        param_type: ParamType::String,
        default: "",
        description: "Blake2b-256 hash (hex) of the Alonzo genesis file.",
    },
    ParamDef {
        key: "ConwayGenesisFile",
        section: "Genesis",
        param_type: ParamType::Path,
        default: "conway-genesis.json",
        description: "Path to the Conway-era genesis JSON file. Contains governance \
                      bootstrap DReps, committee members, and Plutus V3 cost models.",
    },
    ParamDef {
        key: "ConwayGenesisHash",
        section: "Genesis",
        param_type: ParamType::String,
        default: "",
        description: "Blake2b-256 hash (hex) of the Conway genesis file.",
    },
    // --- Protocol section --------------------------------------------------
    ParamDef {
        key: "Protocol",
        section: "Protocol",
        param_type: ParamType::Enum {
            values: &["Cardano", "TPraos", "Praos"],
        },
        default: "Cardano",
        description: "Consensus protocol. 'Cardano' runs the full Hard Fork Combinator \
                      covering all eras from Byron to Conway. 'TPraos' and 'Praos' are \
                      single-era modes used only for isolated test networks.",
    },
    ParamDef {
        key: "TraceBlockFetchClient",
        section: "Protocol",
        param_type: ParamType::Bool,
        default: "false",
        description: "Emit detailed block-fetch client trace events. Useful for \
                      diagnosing slow block propagation but very verbose at high sync rates.",
    },
    ParamDef {
        key: "TraceBlockFetchServer",
        section: "Protocol",
        param_type: ParamType::Bool,
        default: "false",
        description: "Emit detailed block-fetch server trace events (blocks served \
                      to downstream peers).",
    },
    ParamDef {
        key: "TraceChainSyncClient",
        section: "Protocol",
        param_type: ParamType::Bool,
        default: "false",
        description: "Emit chain-sync client trace events (header fetch from upstream).",
    },
    ParamDef {
        key: "TraceChainSyncHeaderServer",
        section: "Protocol",
        param_type: ParamType::Bool,
        default: "false",
        description: "Emit chain-sync header server trace events (headers served to \
                      downstream peers).",
    },
    ParamDef {
        key: "TraceChainSyncBlockServer",
        section: "Protocol",
        param_type: ParamType::Bool,
        default: "false",
        description: "Emit chain-sync block server trace events.",
    },
    // --- Logging section ---------------------------------------------------
    ParamDef {
        key: "MinSeverity",
        section: "Logging",
        param_type: ParamType::Enum {
            values: &[
                "Debug",
                "Info",
                "Notice",
                "Warning",
                "Error",
                "Critical",
                "Alert",
                "Emergency",
            ],
        },
        default: "Info",
        description: "Minimum log severity. Messages below this level are silently \
                      discarded. 'Debug' is very verbose; 'Warning' is suitable for \
                      production deployments.",
    },
    ParamDef {
        key: "TurnOnLogMetrics",
        section: "Logging",
        param_type: ParamType::Bool,
        default: "true",
        description: "Enable the EKG / Prometheus metrics endpoint. When true, metrics \
                      are published on port 12798 and can be scraped by Prometheus.",
    },
    ParamDef {
        key: "TurnOnScripting",
        section: "Logging",
        param_type: ParamType::Bool,
        default: "false",
        description: "Enable scripted log routing (cardano-node legacy logging system). \
                      Not applicable to Torsten's tracing-subscriber backend.",
    },
    // --- Advanced section --------------------------------------------------
    ParamDef {
        key: "MaxConcurrencyBulkSync",
        section: "Advanced",
        param_type: ParamType::U64 { min: 1, max: 64 },
        default: "2",
        description: "Maximum number of parallel block-fetch workers during bulk \
                      (catch-up) sync. Higher values saturate bandwidth faster at the \
                      cost of higher memory usage.",
    },
    ParamDef {
        key: "MaxConcurrencyDeadline",
        section: "Advanced",
        param_type: ParamType::U64 { min: 1, max: 32 },
        default: "4",
        description: "Maximum number of parallel block-fetch workers when near the tip \
                      (deadline mode). Lower than bulk to reduce latency jitter.",
    },
    ParamDef {
        key: "SnapshotInterval",
        section: "Advanced",
        param_type: ParamType::U64 {
            min: 0,
            max: 86_400,
        },
        default: "72",
        description: "Interval in minutes between ledger state snapshots. \
                      Snapshots allow faster restart after an unclean shutdown. \
                      0 disables periodic snapshotting (not recommended).",
    },
    ParamDef {
        key: "ExperimentalHardForksEnabled",
        section: "Advanced",
        param_type: ParamType::Bool,
        default: "false",
        description: "Allow the node to follow experimental hard fork transitions. \
                      Enable only when instructed by the Cardano Foundation for \
                      testnet protocol upgrades.",
    },
    ParamDef {
        key: "EnableP2P",
        section: "Advanced",
        param_type: ParamType::Bool,
        default: "true",
        description: "Duplicate of the Network section entry — shown here only if the \
                      key is encountered in the Advanced section context.",
    },
];

// ---------------------------------------------------------------------------
// Lookup index
// ---------------------------------------------------------------------------

/// Build a lookup map from JSON key name to the corresponding [`ParamDef`].
///
/// Only the first occurrence of a key is used (the static table is deduplicated
/// in definition order). The returned map is suitable for O(1) lookups during
/// config file parsing.
pub fn build_lookup() -> HashMap<&'static str, &'static ParamDef> {
    let mut map = HashMap::new();
    for def in KNOWN_PARAMS {
        // First occurrence wins — avoids duplicates like the EnableP2P one above.
        map.entry(def.key).or_insert(def);
    }
    map
}

// ---------------------------------------------------------------------------
// Section ordering
// ---------------------------------------------------------------------------

/// Canonical display order for sections in the left-panel tree.
///
/// Sections not listed here are appended after the last known section,
/// with [`SECTION_UNKNOWN`] always last.
pub const SECTION_ORDER: &[&str] = &["Network", "Genesis", "Protocol", "Logging", "Advanced"];

/// Return the display priority index of a section name (lower = earlier).
pub fn section_priority(section: &str) -> usize {
    SECTION_ORDER
        .iter()
        .position(|s| *s == section)
        .unwrap_or(SECTION_ORDER.len())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_param_type_validate_bool() {
        let t = ParamType::Bool;
        assert!(t.validate("true").is_ok());
        assert!(t.validate("false").is_ok());
        assert!(t.validate("yes").is_err());
        assert!(t.validate("1").is_err());
    }

    #[test]
    fn test_param_type_validate_u64_range() {
        let t = ParamType::U64 { min: 1, max: 100 };
        assert!(t.validate("1").is_ok());
        assert!(t.validate("100").is_ok());
        assert!(t.validate("0").is_err());
        assert!(t.validate("101").is_err());
        assert!(t.validate("abc").is_err());
    }

    #[test]
    fn test_param_type_validate_enum() {
        let t = ParamType::Enum {
            values: &["A", "B", "C"],
        };
        assert!(t.validate("A").is_ok());
        assert!(t.validate("D").is_err());
    }

    #[test]
    fn test_build_lookup_no_key_collisions() {
        let map = build_lookup();
        // Every entry in the map should point to a real ParamDef.
        for (key, def) in &map {
            assert_eq!(*key, def.key);
        }
    }

    #[test]
    fn test_section_priority_order() {
        assert!(section_priority("Network") < section_priority("Genesis"));
        assert!(section_priority("Genesis") < section_priority("Protocol"));
        assert!(section_priority("Protocol") < section_priority("Logging"));
        assert!(section_priority("Logging") < section_priority("Advanced"));
        assert!(section_priority("Advanced") < section_priority(SECTION_UNKNOWN));
    }

    #[test]
    fn test_param_type_label() {
        assert_eq!(ParamType::Bool.label(), "bool");
        assert_eq!(ParamType::U64 { min: 0, max: 10 }.label(), "u64");
        assert_eq!(ParamType::String.label(), "string");
        assert_eq!(ParamType::Enum { values: &["a"] }.label(), "enum");
        assert_eq!(ParamType::Path.label(), "path");
    }
}
