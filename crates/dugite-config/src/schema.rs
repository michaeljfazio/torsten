//! Parameter schema — known Cardano node configuration parameters.
//!
//! Each [`ParamDef`] describes a single JSON key that may appear in a Cardano
//! node configuration file: its display name, the logical section it belongs
//! to, its value type (with optional constraints), a human-readable
//! description, its documented default value, and an operator tuning hint.
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
    /// Practical operator tuning guidance shown below the description.
    ///
    /// An empty string means no hint is shown.  Hints explain the *why*
    /// behind a setting — what to change and when — rather than repeating
    /// what the description already says.
    pub tuning_hint: &'static str,
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
        tuning_hint: "Set to 'Testnet' for Preview/Preprod/private deployments \
                      and ensure NetworkMagic matches the genesis file.",
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
        tuning_hint: "Mainnet = 764824073, Preview = 2, Preprod = 1. \
                      A mismatched magic will cause all peer handshakes to fail immediately.",
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
        tuning_hint: "Use 'RequiresMagic' for all testnets, 'RequiresNoMagic' for mainnet.",
    },
    ParamDef {
        key: "DiffusionMode",
        section: "Network",
        param_type: ParamType::Enum {
            values: &["InitiatorOnly", "InitiatorAndResponder"],
        },
        default: "InitiatorAndResponder",
        description: "Controls inbound connection acceptance. 'InitiatorAndResponder' \
                      (default) opens a listening port and accepts inbound N2N connections — \
                      the correct mode for relay nodes. 'InitiatorOnly' makes only outbound \
                      connections, suitable for block producers behind a firewall.",
        tuning_hint: "Use 'InitiatorAndResponder' for relays and public-facing nodes. \
                      Use 'InitiatorOnly' for block producers that should never accept \
                      unsolicited inbound connections.",
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
        tuning_hint: "Use 'PeerSharingPublic' for public relays to help decentralise \
                      peer discovery. Block producers may prefer 'PeerSharingPrivate'.",
    },
    ParamDef {
        key: "TargetNumberOfActivePeers",
        section: "Network",
        param_type: ParamType::U64 { min: 1, max: 100 },
        default: "15",
        description: "Target number of fully active (hot) peers — connections where \
                      block headers and bodies are exchanged. Raising this improves \
                      propagation at the cost of higher CPU and bandwidth.",
        tuning_hint: "20 is good for public relays. \
                      Block producers may want 10-15 for lower latency and less noise.",
    },
    ParamDef {
        key: "TargetNumberOfEstablishedPeers",
        section: "Network",
        param_type: ParamType::U64 { min: 1, max: 200 },
        default: "40",
        description: "Target number of established (warm) peers — TCP connections that \
                      are open but not yet doing full block exchange. Acts as a reservoir \
                      to promote to hot when needed.",
        tuning_hint: "Keep at 2-3x TargetNumberOfActivePeers to ensure a healthy \
                      promotion reservoir. 40 is a sensible default for most relays.",
    },
    ParamDef {
        key: "TargetNumberOfKnownPeers",
        section: "Network",
        param_type: ParamType::U64 { min: 1, max: 500 },
        default: "85",
        description: "Target size of the known-peers set (cold + warm + hot). The peer \
                      governor will attempt to keep at least this many addresses in its \
                      address book at all times.",
        tuning_hint: "100 is a good default. \
                      Increase to 200+ for higher network resilience on busy relays.",
    },
    ParamDef {
        key: "TargetNumberOfRootPeers",
        section: "Network",
        param_type: ParamType::U64 { min: 1, max: 200 },
        default: "60",
        description: "Target number of root peers — connections maintained to the \
                      topology file entries (trusted relays). These anchor the node to \
                      the network before ledger peer discovery kicks in.",
        tuning_hint: "Match or slightly exceed your topology file entry count. \
                      Root peers keep the node anchored during initial bootstrap.",
    },
    ParamDef {
        key: "TargetNumberOfActiveBigLedgerPeers",
        section: "Network",
        param_type: ParamType::U64 { min: 0, max: 50 },
        default: "5",
        description: "Target number of active connections to 'big ledger' peers — \
                      well-staked SPO relays discovered from the on-chain pool params \
                      after useLedgerAfterSlot is reached.",
        tuning_hint: "5-10 is sufficient for most relays. \
                      Big ledger peers are high-quality but may be geographically distant.",
    },
    ParamDef {
        key: "TargetNumberOfEstablishedBigLedgerPeers",
        section: "Network",
        param_type: ParamType::U64 { min: 0, max: 100 },
        default: "10",
        description: "Target number of established (warm) connections to big ledger peers.",
        tuning_hint: "Keep at 2x TargetNumberOfActiveBigLedgerPeers \
                      to allow smooth promotion without cold-start delays.",
    },
    ParamDef {
        key: "TargetNumberOfKnownBigLedgerPeers",
        section: "Network",
        param_type: ParamType::U64 { min: 0, max: 200 },
        default: "15",
        description: "Target size of the known big-ledger-peer set (cold + warm + hot).",
        tuning_hint: "15-25 gives a good pool of candidates for ledger peer selection \
                      without excessive churn.",
    },
    // --- Genesis section ---------------------------------------------------
    ParamDef {
        key: "ByronGenesisFile",
        section: "Genesis",
        param_type: ParamType::Path,
        default: "byron-genesis.json",
        description: "Path to the Byron-era genesis JSON file. Can be relative to the \
                      config file's directory or absolute. Must match ByronGenesisHash.",
        tuning_hint: "Must match the network. Do not change unless switching networks. \
                      Use paths relative to the config file for portability.",
    },
    ParamDef {
        key: "ByronGenesisHash",
        section: "Genesis",
        param_type: ParamType::String,
        default: "",
        description: "Blake2b-256 hash (hex) of the Byron genesis file. \
                      The node verifies this on startup to detect genesis mismatches.",
        tuning_hint: "Must exactly match the hash of the genesis file at ByronGenesisFile. \
                      An incorrect hash will prevent the node from starting.",
    },
    ParamDef {
        key: "ShelleyGenesisFile",
        section: "Genesis",
        param_type: ParamType::Path,
        default: "shelley-genesis.json",
        description: "Path to the Shelley-era genesis JSON file. Contains network \
                      parameters, initial delegation, protocol magic, and epoch length.",
        tuning_hint: "Must match the network. Do not change unless switching networks.",
    },
    ParamDef {
        key: "ShelleyGenesisHash",
        section: "Genesis",
        param_type: ParamType::String,
        default: "",
        description: "Blake2b-256 hash (hex) of the Shelley genesis file.",
        tuning_hint: "Must exactly match the hash of the file at ShelleyGenesisFile.",
    },
    ParamDef {
        key: "AlonzoGenesisFile",
        section: "Genesis",
        param_type: ParamType::Path,
        default: "alonzo-genesis.json",
        description: "Path to the Alonzo-era genesis JSON file. Contains initial Plutus \
                      cost model parameters and collateral percentage.",
        tuning_hint: "Must match the network. Do not change unless switching networks.",
    },
    ParamDef {
        key: "AlonzoGenesisHash",
        section: "Genesis",
        param_type: ParamType::String,
        default: "",
        description: "Blake2b-256 hash (hex) of the Alonzo genesis file.",
        tuning_hint: "Must exactly match the hash of the file at AlonzoGenesisFile.",
    },
    ParamDef {
        key: "ConwayGenesisFile",
        section: "Genesis",
        param_type: ParamType::Path,
        default: "conway-genesis.json",
        description: "Path to the Conway-era genesis JSON file. Contains governance \
                      bootstrap DReps, committee members, and Plutus V3 cost models.",
        tuning_hint: "Must match the network. Do not change unless switching networks.",
    },
    ParamDef {
        key: "ConwayGenesisHash",
        section: "Genesis",
        param_type: ParamType::String,
        default: "",
        description: "Blake2b-256 hash (hex) of the Conway genesis file.",
        tuning_hint: "Must exactly match the hash of the file at ConwayGenesisFile.",
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
        tuning_hint: "Always use 'Cardano' for mainnet and public testnets. \
                      'TPraos'/'Praos' are for private devnet experiments only.",
    },
    ParamDef {
        key: "TraceBlockFetchClient",
        section: "Protocol",
        param_type: ParamType::Bool,
        default: "false",
        description: "Emit detailed block-fetch client trace events. Useful for \
                      diagnosing slow block propagation but very verbose at high sync rates.",
        tuning_hint: "Enable only for debugging slow block propagation. \
                      Increases log volume significantly; disable in production.",
    },
    ParamDef {
        key: "TraceBlockFetchServer",
        section: "Protocol",
        param_type: ParamType::Bool,
        default: "false",
        description: "Emit detailed block-fetch server trace events (blocks served \
                      to downstream peers).",
        tuning_hint: "Enable only for debugging. Increases log volume significantly.",
    },
    ParamDef {
        key: "TraceChainSyncClient",
        section: "Protocol",
        param_type: ParamType::Bool,
        default: "false",
        description: "Emit chain-sync client trace events (header fetch from upstream).",
        tuning_hint: "Enable only for debugging chain-sync issues. \
                      Very verbose during initial sync; keep off in production.",
    },
    ParamDef {
        key: "TraceChainSyncHeaderServer",
        section: "Protocol",
        param_type: ParamType::Bool,
        default: "false",
        description: "Emit chain-sync header server trace events (headers served to \
                      downstream peers).",
        tuning_hint: "Enable only for debugging. Increases log volume significantly.",
    },
    ParamDef {
        key: "TraceChainSyncBlockServer",
        section: "Protocol",
        param_type: ParamType::Bool,
        default: "false",
        description: "Emit chain-sync block server trace events.",
        tuning_hint: "Enable only for debugging. Increases log volume significantly.",
    },
    ParamDef {
        key: "TraceChainDb",
        section: "Protocol",
        param_type: ParamType::Bool,
        default: "false",
        description: "Emit ChainDB trace events (block storage, volatile/immutable flush, \
                      rollback operations). Useful for diagnosing storage-layer issues.",
        tuning_hint: "Enable to debug block storage problems or unexpected rollbacks. \
                      Moderate log volume; safe to leave enabled in production if needed.",
    },
    ParamDef {
        key: "TraceChainSyncServer",
        section: "Protocol",
        param_type: ParamType::Bool,
        default: "false",
        description: "Emit chain-sync server trace events (both header and block serving \
                      to downstream N2N peers).",
        tuning_hint: "Enable only for debugging downstream sync issues. \
                      Increases log volume significantly under heavy peer load.",
    },
    ParamDef {
        key: "TraceForge",
        section: "Protocol",
        param_type: ParamType::Bool,
        default: "false",
        description: "Emit block forging trace events (VRF leader check, block construction, \
                      KES signing, block announcement). Essential for block producer debugging.",
        tuning_hint: "Enable on block producers to diagnose missed slots or forging failures. \
                      Low volume (one event per slot check); safe for production.",
    },
    ParamDef {
        key: "TraceMempool",
        section: "Protocol",
        param_type: ParamType::Bool,
        default: "false",
        description: "Emit mempool trace events (transaction admission, rejection, removal \
                      on block application, TTL expiry).",
        tuning_hint: "Enable to debug transaction flow or mempool capacity issues. \
                      Volume depends on transaction rate; moderate on mainnet.",
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
        tuning_hint: "'Info' is the recommended default. \
                      Use 'Warning' for quiet production nodes. \
                      Use 'Debug' only for active troubleshooting sessions.",
    },
    ParamDef {
        key: "TurnOnLogMetrics",
        section: "Logging",
        param_type: ParamType::Bool,
        default: "true",
        description: "Enable the EKG / Prometheus metrics endpoint. When true, metrics \
                      are published on port 12798 and can be scraped by Prometheus.",
        tuning_hint: "Keep enabled. Disabling removes Prometheus scraping capability \
                      and breaks monitoring dashboards.",
    },
    ParamDef {
        key: "TurnOnScripting",
        section: "Logging",
        param_type: ParamType::Bool,
        default: "false",
        description: "Enable scripted log routing (cardano-node legacy logging system). \
                      Not applicable to Dugite's tracing-subscriber backend.",
        tuning_hint: "Leave disabled for Dugite. This setting is a legacy flag \
                      that has no effect on Dugite's tracing-subscriber backend.",
    },
    ParamDef {
        key: "MetricsPort",
        section: "Logging",
        param_type: ParamType::U64 { min: 0, max: 65535 },
        default: "12798",
        description: "TCP port for the Prometheus metrics endpoint. Set to 0 to disable \
                      the metrics server entirely. The CLI flag --metrics-port takes \
                      precedence over this config value; --no-metrics forces port to 0.",
        tuning_hint: "12798 (default) matches cardano-node. Change only if the port \
                      conflicts with another service. Set to 0 in hardened environments \
                      where metrics scraping is not needed.",
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
        tuning_hint: "4-8 on fast hardware/NVMe with ample RAM. \
                      Lower to 2 if memory is constrained below 8 GB.",
    },
    ParamDef {
        key: "MaxConcurrencyDeadline",
        section: "Advanced",
        param_type: ParamType::U64 { min: 1, max: 32 },
        default: "4",
        description: "Maximum number of parallel block-fetch workers when near the tip \
                      (deadline mode). Lower than bulk to reduce latency jitter.",
        tuning_hint: "Keep lower than MaxConcurrencyBulkSync. \
                      2-4 is optimal; higher values add latency jitter near tip.",
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
        tuning_hint: "72 minutes (default) matches the Haskell node. \
                      Never set to 0 in production — recovery from an unclean \
                      shutdown will require a full replay from genesis.",
    },
    ParamDef {
        key: "ExperimentalHardForksEnabled",
        section: "Advanced",
        param_type: ParamType::Bool,
        default: "false",
        description: "Allow the node to follow experimental hard fork transitions. \
                      Enable only when instructed by the Cardano Foundation for \
                      testnet protocol upgrades.",
        tuning_hint: "Leave disabled unless you have been explicitly asked to enable it \
                      for a specific testnet upgrade. Enabling prematurely can cause \
                      chain divergence on mainnet.",
    },
    ParamDef {
        key: "ChurnIntervalNormalSecs",
        section: "Advanced",
        param_type: ParamType::U64 {
            min: 60,
            max: 86_400,
        },
        default: "3300",
        description: "Peer governor churn interval during normal (caught-up) operation, \
                      in seconds. Controls how often the governor rotates a random subset \
                      of peers to prevent the node from becoming permanently attached to \
                      the same peer set. Default 3300 s (55 minutes) matches cardano-node.",
        tuning_hint: "Lower values increase peer diversity at the cost of more handshakes. \
                      Block producers may prefer higher values (3600+) for connection stability.",
    },
    ParamDef {
        key: "ChurnIntervalSyncSecs",
        section: "Advanced",
        param_type: ParamType::U64 {
            min: 30,
            max: 86_400,
        },
        default: "900",
        description: "Peer governor churn interval during syncing, in seconds. Faster \
                      rotation while catching up allows the node to quickly shed \
                      unresponsive peers. Default 900 s (15 minutes) matches cardano-node.",
        tuning_hint: "Keep below 15 minutes to shed unresponsive peers during catch-up. \
                      Lower values improve sync speed at the cost of more connection churn.",
    },
    ParamDef {
        key: "StallDemotionCycles",
        section: "Advanced",
        param_type: ParamType::U64 { min: 1, max: 100 },
        default: "6",
        description: "Number of consecutive governor evaluation cycles (each ~30 s) in \
                      which a hot peer must serve zero new blocks before it is demoted \
                      back to warm. Default of 6 cycles = 3 minutes of inactivity.",
        tuning_hint: "Increase if hot peers legitimately produce zero blocks for extended \
                      periods (e.g., low-stake pools). Decrease for aggressive stall detection.",
    },
    ParamDef {
        key: "ErrorDemotionThreshold",
        section: "Advanced",
        param_type: ParamType::U64 { min: 1, max: 100 },
        default: "5",
        description: "Failure count threshold above which a hot peer is unconditionally \
                      demoted to warm during each governor evaluation cycle. Local root \
                      peers are exempt from this check.",
        tuning_hint: "Lower to aggressively shed failing peers. Raise if peers are being \
                      demoted too frequently due to transient network issues.",
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
        // First occurrence wins — avoids duplicates.
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
// Network default values (used by `init` subcommand)
// ---------------------------------------------------------------------------

/// The recognised networks for the `init` subcommand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
    Mainnet,
    Preview,
    Preprod,
}

impl Network {
    /// Parse a network name string (case-insensitive).
    pub fn from_str(s: &str) -> Option<Network> {
        match s.to_lowercase().as_str() {
            "mainnet" => Some(Network::Mainnet),
            "preview" => Some(Network::Preview),
            "preprod" => Some(Network::Preprod),
            _ => None,
        }
    }

    /// The network magic integer for this network.
    pub fn magic(self) -> u64 {
        match self {
            Network::Mainnet => 764_824_073,
            Network::Preview => 2,
            Network::Preprod => 1,
        }
    }

    /// Whether network magic enforcement is needed (mainnet uses RequiresNoMagic).
    pub fn requires_magic(self) -> &'static str {
        match self {
            Network::Mainnet => "RequiresNoMagic",
            Network::Preview | Network::Preprod => "RequiresMagic",
        }
    }

    /// Display name used in genesis file path prefixes.
    pub fn genesis_prefix(self) -> &'static str {
        match self {
            Network::Mainnet => "mainnet",
            Network::Preview => "preview",
            Network::Preprod => "preprod",
        }
    }
}

/// Build a `serde_json::Map` with sensible defaults for the given network.
///
/// The returned map is ready to be pretty-printed and written to disk as a
/// starter configuration file.  All paths use the conventional relative
/// names expected alongside an official Cardano node config directory.
pub fn network_defaults(network: Network) -> serde_json::Map<String, serde_json::Value> {
    use serde_json::{json, Map, Value};

    let prefix = network.genesis_prefix();
    let magic = network.magic();
    let req_magic = network.requires_magic();
    let network_str = match network {
        Network::Mainnet => "Mainnet",
        _ => "Testnet",
    };

    let mut map = Map::new();

    // Network identity.
    map.insert("Network".into(), json!(network_str));
    map.insert("NetworkMagic".into(), json!(magic));
    map.insert("RequiresNetworkMagic".into(), json!(req_magic));

    // P2P networking.
    map.insert("DiffusionMode".into(), json!("InitiatorAndResponder"));
    map.insert("PeerSharing".into(), json!("PeerSharingPublic"));
    map.insert("TargetNumberOfActivePeers".into(), json!(15));
    map.insert("TargetNumberOfEstablishedPeers".into(), json!(40));
    map.insert("TargetNumberOfKnownPeers".into(), json!(85));
    map.insert("TargetNumberOfRootPeers".into(), json!(60));
    map.insert("TargetNumberOfActiveBigLedgerPeers".into(), json!(5));
    map.insert("TargetNumberOfEstablishedBigLedgerPeers".into(), json!(10));
    map.insert("TargetNumberOfKnownBigLedgerPeers".into(), json!(15));

    // Genesis files (conventional relative paths).
    map.insert(
        "ByronGenesisFile".into(),
        Value::String(format!("{prefix}-byron-genesis.json")),
    );
    map.insert("ByronGenesisHash".into(), json!(""));
    map.insert(
        "ShelleyGenesisFile".into(),
        Value::String(format!("{prefix}-shelley-genesis.json")),
    );
    map.insert("ShelleyGenesisHash".into(), json!(""));
    map.insert(
        "AlonzoGenesisFile".into(),
        Value::String(format!("{prefix}-alonzo-genesis.json")),
    );
    map.insert("AlonzoGenesisHash".into(), json!(""));
    map.insert(
        "ConwayGenesisFile".into(),
        Value::String(format!("{prefix}-conway-genesis.json")),
    );
    map.insert("ConwayGenesisHash".into(), json!(""));

    // Protocol.
    map.insert("Protocol".into(), json!("Cardano"));
    map.insert("TraceBlockFetchClient".into(), json!(false));
    map.insert("TraceBlockFetchServer".into(), json!(false));
    map.insert("TraceChainSyncClient".into(), json!(false));
    map.insert("TraceChainSyncHeaderServer".into(), json!(false));
    map.insert("TraceChainSyncBlockServer".into(), json!(false));
    map.insert("TraceChainDb".into(), json!(false));
    map.insert("TraceChainSyncServer".into(), json!(false));
    map.insert("TraceForge".into(), json!(false));
    map.insert("TraceMempool".into(), json!(false));

    // Logging.
    map.insert("MinSeverity".into(), json!("Info"));
    map.insert("TurnOnLogMetrics".into(), json!(true));
    map.insert("TurnOnScripting".into(), json!(false));
    map.insert("MetricsPort".into(), json!(12798));

    // Advanced.
    map.insert("MaxConcurrencyBulkSync".into(), json!(2));
    map.insert("MaxConcurrencyDeadline".into(), json!(4));
    map.insert("SnapshotInterval".into(), json!(72));
    map.insert("ExperimentalHardForksEnabled".into(), json!(false));
    map.insert("ChurnIntervalNormalSecs".into(), json!(3300));
    map.insert("ChurnIntervalSyncSecs".into(), json!(900));
    map.insert("StallDemotionCycles".into(), json!(6));
    map.insert("ErrorDemotionThreshold".into(), json!(5));

    map
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

    #[test]
    fn test_all_params_have_tuning_hints() {
        // Every entry in KNOWN_PARAMS must carry a non-empty tuning hint so
        // the description panel always has operator guidance to show.
        for def in KNOWN_PARAMS {
            assert!(
                !def.tuning_hint.is_empty(),
                "ParamDef '{}' is missing a tuning_hint",
                def.key
            );
        }
    }

    #[test]
    fn test_network_defaults_mainnet_magic() {
        let map = network_defaults(Network::Mainnet);
        assert_eq!(map["NetworkMagic"], serde_json::json!(764_824_073_u64));
        assert_eq!(
            map["RequiresNetworkMagic"],
            serde_json::json!("RequiresNoMagic")
        );
        assert_eq!(map["Network"], serde_json::json!("Mainnet"));
    }

    #[test]
    fn test_network_defaults_preview_magic() {
        let map = network_defaults(Network::Preview);
        assert_eq!(map["NetworkMagic"], serde_json::json!(2_u64));
        assert_eq!(
            map["RequiresNetworkMagic"],
            serde_json::json!("RequiresMagic")
        );
        assert_eq!(map["Network"], serde_json::json!("Testnet"));
    }

    #[test]
    fn test_network_defaults_genesis_paths() {
        let map = network_defaults(Network::Preview);
        assert_eq!(
            map["ByronGenesisFile"],
            serde_json::json!("preview-byron-genesis.json")
        );
        assert_eq!(
            map["ConwayGenesisFile"],
            serde_json::json!("preview-conway-genesis.json")
        );
    }

    #[test]
    fn test_network_from_str() {
        assert_eq!(Network::from_str("mainnet"), Some(Network::Mainnet));
        assert_eq!(Network::from_str("PREVIEW"), Some(Network::Preview));
        assert_eq!(Network::from_str("preprod"), Some(Network::Preprod));
        assert_eq!(Network::from_str("devnet"), None);
    }
}
