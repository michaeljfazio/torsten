use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Network topology configuration (compatible with cardano-node format)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Topology {
    /// Block-producing peers (P2P format)
    #[serde(default, alias = "Producers")]
    pub producers: Vec<TopologyProducer>,

    /// P2P topology format
    #[serde(default)]
    pub local_roots: Vec<LocalRootGroup>,

    /// Public roots for P2P discovery
    #[serde(default)]
    pub public_roots: Vec<PublicRoot>,

    /// Enable use of ledger for peer discovery
    #[serde(default)]
    pub use_ledger_after_slot: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyProducer {
    pub addr: String,
    pub port: u16,
    #[serde(default)]
    pub valency: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalRootGroup {
    pub access_points: Vec<AccessPoint>,
    pub advertise: bool,
    pub valency: u16,
    #[serde(default)]
    pub trust_able: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessPoint {
    pub address: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicRoot {
    pub access_points: Vec<AccessPoint>,
    pub advertise: bool,
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

    /// Get all peer addresses from the topology
    pub fn all_peers(&self) -> Vec<(String, u16)> {
        let mut peers: Vec<(String, u16)> = self
            .producers
            .iter()
            .map(|p| (p.addr.clone(), p.port))
            .collect();

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
}

impl Default for Topology {
    fn default() -> Self {
        Topology {
            producers: vec![],
            local_roots: vec![],
            public_roots: vec![
                PublicRoot {
                    access_points: vec![
                        AccessPoint {
                            address: "relays-new.cardano-mainnet.iohk.io".to_string(),
                            port: 3001,
                        },
                    ],
                    advertise: false,
                },
            ],
            use_ledger_after_slot: Some(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_topology() {
        let topo = Topology::default();
        assert!(!topo.public_roots.is_empty());
        let peers = topo.all_peers();
        assert!(!peers.is_empty());
    }

    #[test]
    fn test_all_peers() {
        let topo = Topology {
            producers: vec![TopologyProducer {
                addr: "127.0.0.1".to_string(),
                port: 3001,
                valency: 1,
            }],
            local_roots: vec![LocalRootGroup {
                access_points: vec![AccessPoint {
                    address: "192.168.1.1".to_string(),
                    port: 3002,
                }],
                advertise: false,
                valency: 1,
                trust_able: true,
            }],
            public_roots: vec![],
            use_ledger_after_slot: None,
        };

        let peers = topo.all_peers();
        assert_eq!(peers.len(), 2);
    }
}
