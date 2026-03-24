//! Peer discovery — DNS, ledger-based, and peer sharing.
//!
//! Provides three discovery mechanisms:
//! - **DNS**: A/AAAA resolution via hickory-resolver
//! - **Ledger**: SPO relay addresses from pool_params (when past useLedgerAfterSlot)
//! - **PeerSharing**: Addresses received from the PeerSharing protocol

use std::net::SocketAddr;

/// A discovered peer address with its source.
#[derive(Debug, Clone)]
pub struct DiscoveredPeer {
    /// Socket address of the peer.
    pub addr: SocketAddr,
    /// How the peer was discovered.
    pub source: super::manager::PeerSource,
}

/// Resolve a DNS hostname to socket addresses.
///
/// Uses hickory-resolver for async DNS resolution. Returns all A and AAAA
/// records for the hostname with the given port.
pub async fn resolve_dns(hostname: &str, port: u16) -> Vec<SocketAddr> {
    use hickory_resolver::TokioResolver;

    let resolver = match TokioResolver::builder_tokio() {
        Ok(builder) => builder.build(),
        Err(e) => {
            tracing::warn!(error = %e, hostname, "failed to create DNS resolver");
            return vec![];
        }
    };

    let mut addrs: Vec<SocketAddr> = Vec::new();

    match resolver.lookup_ip(hostname).await {
        Ok(response) => {
            for ip in response.iter() {
                addrs.push(SocketAddr::new(ip, port));
            }
        }
        Err(e) => {
            tracing::debug!(error = %e, hostname, "DNS lookup failed");
        }
    }

    addrs
}

/// Parse relay addresses from a topology configuration.
///
/// Resolves hostnames via DNS or parses direct IP addresses.
pub async fn resolve_topology_relays(relays: &[(String, u16)]) -> Vec<SocketAddr> {
    let mut addrs = Vec::new();
    for (host, port) in relays {
        // Try to parse as IP address first
        if let Ok(ip) = host.parse::<std::net::IpAddr>() {
            addrs.push(SocketAddr::new(ip, *port));
        } else {
            // DNS resolution
            let resolved = resolve_dns(host, *port).await;
            addrs.extend(resolved);
        }
    }
    addrs
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[tokio::test]
    async fn resolve_ip_address_directly() {
        let addrs = resolve_topology_relays(&[("127.0.0.1".to_string(), 3001)]).await;
        assert_eq!(addrs.len(), 1);
        assert_eq!(
            addrs[0],
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 3001)
        );
    }

    #[tokio::test]
    async fn resolve_ipv6_address_directly() {
        let addrs = resolve_topology_relays(&[("::1".to_string(), 3001)]).await;
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].port(), 3001);
    }
}
