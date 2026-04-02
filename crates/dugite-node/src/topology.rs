use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Network topology configuration (compatible with cardano-node P2P format)
///
/// Supports the full cardano-node 10.x+ topology format including:
/// - Bootstrap peers (trusted peers for initial sync)
/// - Local root peer groups (with hotValency, warmValency, trustable, behindFirewall)
/// - Public roots (fallback peers before ledger-based discovery)
/// - useLedgerAfterSlot (transition from public roots to ledger peers)
/// - peerSnapshotFile (big ledger peer snapshot for Genesis bootstrap)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Topology {
    /// Legacy format: block-producing peers
    #[serde(default, alias = "Producers")]
    pub producers: Vec<TopologyProducer>,

    /// Bootstrap peers for initial network discovery (cardano-node 10.x+).
    /// These are trusted peers from founding organizations used only during sync.
    /// Set to null/empty to disable bootstrap peers.
    #[serde(default)]
    pub bootstrap_peers: Option<Vec<AccessPoint>>,

    /// P2P topology format: local root peer groups.
    /// Peers the node should always keep as hot or warm (e.g., your block producer,
    /// peer arrangements with other stake pool operators).
    #[serde(default)]
    pub local_roots: Vec<LocalRootGroup>,

    /// Public roots for P2P discovery.
    /// Publicly known nodes (e.g., IOG relays) serving as fallback peers
    /// before the node syncs to the configured slot.
    #[serde(default)]
    pub public_roots: Vec<PublicRoot>,

    /// Enable use of ledger for peer discovery after this slot.
    /// Negative values or omission disables ledger peers entirely.
    #[serde(default)]
    pub use_ledger_after_slot: Option<i64>,

    /// Path to big ledger peer snapshot file for Genesis bootstrap.
    #[serde(default)]
    pub peer_snapshot_file: Option<String>,
}

/// Legacy topology producer entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyProducer {
    pub addr: String,
    pub port: u16,
    #[serde(default)]
    pub valency: u16,
}

/// A local root peer group with full P2P configuration.
///
/// Each group has access points and controls for how many peers should
/// be kept hot (actively syncing) and warm (connected but not syncing).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalRootGroup {
    /// Peer addresses in this group
    pub access_points: Vec<AccessPoint>,

    /// Whether these peers can be shared via the peer sharing protocol
    #[serde(default)]
    pub advertise: bool,

    /// Target number of active (hot) connections in this group.
    /// Deprecated in favor of `hot_valency`.
    #[serde(default = "default_valency")]
    pub valency: u16,

    /// Target number of hot (actively syncing) peers in this group.
    /// Takes precedence over `valency` if both are set.
    #[serde(default)]
    pub hot_valency: Option<u16>,

    /// Target number of warm (connected, not syncing) peers in this group.
    /// Recommended: hotValency + 1 for backup redundancy.
    #[serde(default)]
    pub warm_valency: Option<u16>,

    /// Whether these peers are trusted for sync.
    /// Trustable peers are preferred during initial sync and the node
    /// will disconnect from non-trusted peers when syncing from outdated state.
    #[serde(default, alias = "trust_able")]
    pub trustable: bool,

    /// Whether these peers are behind a firewall.
    /// When true, the node will not initiate outbound connections to these
    /// peers and instead wait for inbound connections from them.
    #[serde(default)]
    pub behind_firewall: Option<bool>,

    /// Per-group diffusion mode override.
    /// "InitiatorAndResponder" (default) or "InitiatorOnly".
    #[serde(default)]
    pub diffusion_mode: Option<String>,
}

/// A single peer access point (address + port)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessPoint {
    pub address: String,
    pub port: u16,
}

/// Public root peer configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicRoot {
    pub access_points: Vec<AccessPoint>,
    #[serde(default)]
    pub advertise: bool,
}

fn default_valency() -> u16 {
    1
}

impl LocalRootGroup {
    /// Whether peers in this group are behind a firewall
    pub fn is_behind_firewall(&self) -> bool {
        self.behind_firewall.unwrap_or(false)
    }

    /// Get effective hot valency (prefers hotValency over deprecated valency)
    pub fn effective_hot_valency(&self) -> u16 {
        self.hot_valency.unwrap_or(self.valency)
    }

    /// Get effective warm valency (defaults to hot_valency + 1 if not set)
    pub fn effective_warm_valency(&self) -> u16 {
        self.warm_valency
            .unwrap_or_else(|| self.effective_hot_valency() + 1)
    }
}

/// Detailed peer info extracted from topology for the peer manager
#[derive(Debug, Clone)]
#[allow(dead_code)] // fields used by networking rewrite (peer management)
pub struct TopologyPeer {
    pub address: String,
    pub port: u16,
    /// Topology source category for this peer
    pub _source: TopologyPeerSource,
    pub trustable: bool,
    pub advertise: bool,
    /// Whether this peer is behind a firewall (inbound-only)
    pub _behind_firewall: bool,
}

/// Source category for a topology peer
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopologyPeerSource {
    /// From bootstrapPeers — trusted sync-only peers
    Bootstrap,
    /// From localRoots — always-keep peers
    LocalRoot,
    /// From publicRoots — fallback peers
    PublicRoot,
    /// Legacy producers list
    Producer,
}

impl Topology {
    pub fn load(path: &Path) -> Result<Self> {
        if path.exists() {
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read topology: {}", path.display()))?;
            serde_json::from_str(&content)
                .with_context(|| format!("Failed to parse topology: {}", path.display()))
        } else {
            Ok(Self::default())
        }
    }

    /// Get all peer addresses from the topology (flat list for backwards compatibility)
    pub fn all_peers(&self) -> Vec<(String, u16)> {
        let mut peers: Vec<(String, u16)> = self
            .producers
            .iter()
            .map(|p| (p.addr.clone(), p.port))
            .collect();

        // Bootstrap peers (cardano-node 10.x+ format)
        if let Some(ref bps) = self.bootstrap_peers {
            for bp in bps {
                peers.push((bp.address.clone(), bp.port));
            }
        }

        for group in &self.local_roots {
            for ap in &group.access_points {
                peers.push((ap.address.clone(), ap.port));
            }
        }

        for root in &self.public_roots {
            for ap in &root.access_points {
                peers.push((ap.address.clone(), ap.port));
            }
        }

        peers
    }

    /// Get detailed peer info with source, trust, and advertise metadata.
    /// This is used by the peer manager to properly categorize peers.
    pub fn detailed_peers(&self) -> Vec<TopologyPeer> {
        let mut peers = Vec::new();

        // Legacy producers
        for p in &self.producers {
            peers.push(TopologyPeer {
                address: p.addr.clone(),
                port: p.port,
                _source: TopologyPeerSource::Producer,
                trustable: false,
                advertise: false,
                _behind_firewall: false,
            });
        }

        // Bootstrap peers — always trusted for sync
        if let Some(ref bps) = self.bootstrap_peers {
            for bp in bps {
                peers.push(TopologyPeer {
                    address: bp.address.clone(),
                    port: bp.port,
                    _source: TopologyPeerSource::Bootstrap,
                    trustable: true,
                    advertise: false,
                    _behind_firewall: false,
                });
            }
        }

        // Local root groups
        for group in &self.local_roots {
            for ap in &group.access_points {
                peers.push(TopologyPeer {
                    address: ap.address.clone(),
                    port: ap.port,
                    _source: TopologyPeerSource::LocalRoot,
                    trustable: group.trustable,
                    advertise: group.advertise,
                    _behind_firewall: group.is_behind_firewall(),
                });
            }
        }

        // Public roots
        for root in &self.public_roots {
            for ap in &root.access_points {
                peers.push(TopologyPeer {
                    address: ap.address.clone(),
                    port: ap.port,
                    _source: TopologyPeerSource::PublicRoot,
                    trustable: false,
                    advertise: root.advertise,
                    _behind_firewall: false,
                });
            }
        }

        peers
    }

    /// Whether ledger-based peer discovery is enabled at the given slot
    pub fn ledger_peers_enabled(&self, current_slot: u64) -> bool {
        match self.use_ledger_after_slot {
            Some(slot) if slot >= 0 => current_slot >= slot as u64,
            _ => false,
        }
    }

    /// Whether bootstrap peers are configured
    pub fn has_bootstrap_peers(&self) -> bool {
        self.bootstrap_peers
            .as_ref()
            .is_some_and(|bps| !bps.is_empty())
    }

    /// Whether any trustable peers are configured (bootstrap or trustable local roots)
    pub fn has_trustable_peers(&self) -> bool {
        if self.has_bootstrap_peers() {
            return true;
        }
        self.local_roots.iter().any(|g| g.trustable)
    }
}

impl Default for Topology {
    fn default() -> Self {
        Topology {
            producers: vec![],
            bootstrap_peers: Some(vec![
                AccessPoint {
                    address: "backbone.cardano.iog.io".to_string(),
                    port: 3001,
                },
                AccessPoint {
                    address: "backbone.mainnet.cardanofoundation.org".to_string(),
                    port: 3001,
                },
                AccessPoint {
                    address: "backbone.mainnet.emurgornd.com".to_string(),
                    port: 3001,
                },
            ]),
            local_roots: vec![],
            public_roots: vec![],
            use_ledger_after_slot: Some(177724800),
            peer_snapshot_file: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_topology() {
        let topo = Topology::default();
        assert!(topo.has_bootstrap_peers());
        let peers = topo.all_peers();
        assert_eq!(peers.len(), 3);
        assert_eq!(peers[0].0, "backbone.cardano.iog.io");
    }

    #[test]
    fn test_all_peers() {
        let topo = Topology {
            producers: vec![TopologyProducer {
                addr: "127.0.0.1".to_string(),
                port: 3001,
                valency: 1,
            }],
            bootstrap_peers: Some(vec![AccessPoint {
                address: "bootstrap.example.com".to_string(),
                port: 3001,
            }]),
            local_roots: vec![LocalRootGroup {
                access_points: vec![AccessPoint {
                    address: "192.168.1.1".to_string(),
                    port: 3002,
                }],
                advertise: false,
                valency: 1,
                hot_valency: None,
                warm_valency: None,
                trustable: true,
                behind_firewall: None,
                diffusion_mode: None,
            }],
            public_roots: vec![],
            use_ledger_after_slot: None,
            peer_snapshot_file: None,
        };

        let peers = topo.all_peers();
        assert_eq!(peers.len(), 3);
    }

    #[test]
    fn test_detailed_peers() {
        let topo = Topology {
            producers: vec![],
            bootstrap_peers: Some(vec![AccessPoint {
                address: "bootstrap.example.com".to_string(),
                port: 3001,
            }]),
            local_roots: vec![LocalRootGroup {
                access_points: vec![AccessPoint {
                    address: "192.168.1.1".to_string(),
                    port: 3002,
                }],
                advertise: true,
                valency: 1,
                hot_valency: Some(2),
                warm_valency: Some(3),
                trustable: true,
                behind_firewall: Some(true),
                diffusion_mode: Some("InitiatorOnly".to_string()),
            }],
            public_roots: vec![PublicRoot {
                access_points: vec![AccessPoint {
                    address: "public.example.com".to_string(),
                    port: 3001,
                }],
                advertise: false,
            }],
            use_ledger_after_slot: Some(100),
            peer_snapshot_file: None,
        };

        let detailed = topo.detailed_peers();
        assert_eq!(detailed.len(), 3);

        // Bootstrap peer is trusted
        assert_eq!(detailed[0]._source, TopologyPeerSource::Bootstrap);
        assert!(detailed[0].trustable);

        // Local root peer
        assert_eq!(detailed[1]._source, TopologyPeerSource::LocalRoot);
        assert!(detailed[1].trustable);
        assert!(detailed[1].advertise);
        assert!(detailed[1]._behind_firewall);

        // Public root
        assert_eq!(detailed[2]._source, TopologyPeerSource::PublicRoot);
        assert!(!detailed[2].trustable);
    }

    #[test]
    fn test_parse_official_preview_topology() {
        let json = r#"{
            "bootstrapPeers": [
                { "address": "preview-node.play.dev.cardano.org", "port": 3001 }
            ],
            "localRoots": [{ "accessPoints": [], "advertise": false, "trustable": false, "valency": 1 }],
            "publicRoots": [{ "accessPoints": [], "advertise": false }],
            "useLedgerAfterSlot": 102729600
        }"#;
        let topo: Topology = serde_json::from_str(json).unwrap();
        let peers = topo.all_peers();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].0, "preview-node.play.dev.cardano.org");
        assert_eq!(peers[0].1, 3001);
        assert!(topo.has_trustable_peers()); // bootstrap peers are trustable
    }

    #[test]
    fn test_parse_official_mainnet_topology() {
        let json = r#"{
            "bootstrapPeers": [
                { "address": "backbone.cardano.iog.io", "port": 3001 },
                { "address": "backbone.mainnet.cardanofoundation.org", "port": 3001 },
                { "address": "backbone.mainnet.emurgornd.com", "port": 3001 }
            ],
            "localRoots": [{ "accessPoints": [], "advertise": false, "valency": 1 }],
            "publicRoots": [{ "accessPoints": [], "advertise": false }],
            "useLedgerAfterSlot": 177724800
        }"#;
        let topo: Topology = serde_json::from_str(json).unwrap();
        let peers = topo.all_peers();
        assert_eq!(peers.len(), 3);
    }

    #[test]
    fn test_hot_warm_valency() {
        let group = LocalRootGroup {
            access_points: vec![],
            advertise: false,
            valency: 2,
            hot_valency: Some(3),
            warm_valency: Some(5),
            trustable: false,
            behind_firewall: None,
            diffusion_mode: None,
        };
        assert_eq!(group.effective_hot_valency(), 3);
        assert_eq!(group.effective_warm_valency(), 5);

        // Without hot_valency, falls back to valency
        let group2 = LocalRootGroup {
            access_points: vec![],
            advertise: false,
            valency: 2,
            hot_valency: None,
            warm_valency: None,
            trustable: false,
            behind_firewall: None,
            diffusion_mode: None,
        };
        assert_eq!(group2.effective_hot_valency(), 2);
        assert_eq!(group2.effective_warm_valency(), 3); // hot + 1
    }

    #[test]
    fn test_ledger_peers_enabled() {
        let topo = Topology {
            use_ledger_after_slot: Some(100),
            ..Topology::default()
        };
        assert!(!topo.ledger_peers_enabled(50));
        assert!(topo.ledger_peers_enabled(100));
        assert!(topo.ledger_peers_enabled(200));

        // Negative value disables
        let topo2 = Topology {
            use_ledger_after_slot: Some(-1),
            ..Topology::default()
        };
        assert!(!topo2.ledger_peers_enabled(1000000));

        // None disables
        let topo3 = Topology {
            use_ledger_after_slot: None,
            ..Topology::default()
        };
        assert!(!topo3.ledger_peers_enabled(1000000));
    }

    #[test]
    fn test_null_bootstrap_peers() {
        let json = r#"{
            "bootstrapPeers": null,
            "localRoots": [],
            "publicRoots": [],
            "useLedgerAfterSlot": 0
        }"#;
        let topo: Topology = serde_json::from_str(json).unwrap();
        assert!(!topo.has_bootstrap_peers());
    }

    #[test]
    fn test_behind_firewall() {
        let group = LocalRootGroup {
            access_points: vec![],
            advertise: false,
            valency: 1,
            hot_valency: None,
            warm_valency: None,
            trustable: false,
            behind_firewall: Some(true),
            diffusion_mode: None,
        };
        assert!(group.is_behind_firewall());

        let group2 = LocalRootGroup {
            behind_firewall: None,
            ..group
        };
        assert!(!group2.is_behind_firewall());
    }

    #[test]
    fn test_peer_snapshot_file() {
        let json = r#"{
            "bootstrapPeers": [],
            "localRoots": [],
            "publicRoots": [],
            "useLedgerAfterSlot": 0,
            "peerSnapshotFile": "peer-snapshot.json"
        }"#;
        let topo: Topology = serde_json::from_str(json).unwrap();
        assert_eq!(
            topo.peer_snapshot_file.as_deref(),
            Some("peer-snapshot.json")
        );
    }

    #[test]
    fn test_full_topology_with_all_fields() {
        let json = r#"{
            "bootstrapPeers": [
                { "address": "bootstrap.example.com", "port": 3001 }
            ],
            "localRoots": [{
                "accessPoints": [
                    { "address": "192.168.1.1", "port": 3002 }
                ],
                "advertise": true,
                "hotValency": 2,
                "warmValency": 3,
                "trustable": true,
                "behindFirewall": false,
                "diffusionMode": "InitiatorAndResponder"
            }],
            "publicRoots": [{
                "accessPoints": [
                    { "address": "public.example.com", "port": 3001 }
                ],
                "advertise": true
            }],
            "useLedgerAfterSlot": 177724800,
            "peerSnapshotFile": "snapshot.json"
        }"#;
        let topo: Topology = serde_json::from_str(json).unwrap();

        assert!(topo.has_bootstrap_peers());
        assert!(topo.has_trustable_peers());
        assert_eq!(topo.all_peers().len(), 3);

        let detailed = topo.detailed_peers();
        assert_eq!(detailed.len(), 3);

        let local = &topo.local_roots[0];
        assert_eq!(local.effective_hot_valency(), 2);
        assert_eq!(local.effective_warm_valency(), 3);
        assert!(!local.is_behind_firewall());
    }
}
