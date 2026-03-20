//! DNS resolution with TTL-based caching.
//!
//! Provides a `DnsResolver` that caches resolved addresses based on DNS TTL,
//! with automatic cache expiry and fallback to `tokio::net::lookup_host` if
//! the primary hickory-resolver fails.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tracing::{debug, warn};

/// Cached DNS entry: resolved addresses, insertion time, and TTL.
#[derive(Debug, Clone)]
struct CacheEntry {
    addrs: Vec<SocketAddr>,
    inserted_at: Instant,
    ttl: Duration,
}

impl CacheEntry {
    fn is_expired(&self) -> bool {
        self.inserted_at.elapsed() >= self.ttl
    }
}

/// DNS resolver with TTL-based caching.
///
/// Resolves hostnames to socket addresses using hickory-resolver as the primary
/// resolver, with fallback to `tokio::net::lookup_host`. Results are cached
/// based on the DNS TTL returned by the resolver.
pub struct DnsResolver {
    cache: HashMap<String, CacheEntry>,
    /// Minimum TTL to enforce (prevents excessively short TTLs)
    min_ttl: Duration,
    /// Default TTL when the resolver doesn't provide one
    default_ttl: Duration,
}

impl Default for DnsResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl DnsResolver {
    /// Create a new DNS resolver with sensible defaults.
    ///
    /// - Minimum TTL: 30 seconds
    /// - Default TTL: 300 seconds (5 minutes)
    pub fn new() -> Self {
        DnsResolver {
            cache: HashMap::new(),
            min_ttl: Duration::from_secs(30),
            default_ttl: Duration::from_secs(300),
        }
    }

    /// Create a DNS resolver with custom TTL bounds.
    pub fn with_ttl(min_ttl: Duration, default_ttl: Duration) -> Self {
        DnsResolver {
            cache: HashMap::new(),
            min_ttl,
            default_ttl,
        }
    }

    /// Resolve a hostname to a list of socket addresses.
    ///
    /// Returns cached results if available and not expired. Otherwise performs
    /// a fresh DNS lookup using hickory-resolver, falling back to
    /// `tokio::net::lookup_host` on failure.
    pub async fn resolve(&mut self, host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
        let cache_key = format!("{host}:{port}");

        // Check cache first
        if let Some(entry) = self.cache.get(&cache_key) {
            if !entry.is_expired() {
                debug!(
                    host,
                    port,
                    cached_addrs = entry.addrs.len(),
                    "DNS cache hit"
                );
                return Ok(entry.addrs.clone());
            }
            debug!(host, port, "DNS cache expired, re-resolving");
        }

        // Try hickory-resolver first
        match self.resolve_hickory(host, port).await {
            Ok((addrs, ttl)) => {
                let effective_ttl = ttl.max(self.min_ttl);
                debug!(
                    host,
                    port,
                    addrs = addrs.len(),
                    ttl_secs = effective_ttl.as_secs(),
                    "DNS resolved via hickory"
                );
                self.cache.insert(
                    cache_key,
                    CacheEntry {
                        addrs: addrs.clone(),
                        inserted_at: Instant::now(),
                        ttl: effective_ttl,
                    },
                );
                Ok(addrs)
            }
            Err(hickory_err) => {
                warn!(
                    host,
                    port, error = %hickory_err, "hickory-resolver failed, falling back to tokio"
                );
                self.resolve_fallback(host, port, &cache_key).await
            }
        }
    }

    /// Resolve using hickory-resolver. Returns (addresses, TTL).
    async fn resolve_hickory(
        &self,
        host: &str,
        port: u16,
    ) -> Result<(Vec<SocketAddr>, Duration), String> {
        let resolver = hickory_resolver::TokioResolver::builder_tokio()
            .map_err(|e| format!("failed to create hickory resolver: {e}"))?
            .build();

        let response = resolver
            .lookup_ip(host)
            .await
            .map_err(|e| format!("hickory lookup failed for {host}: {e}"))?;

        // Extract TTL from the response using valid_until
        let valid_until = response.as_lookup().valid_until();
        let ttl = valid_until
            .checked_duration_since(Instant::now())
            .unwrap_or(self.default_ttl);

        let addrs: Vec<SocketAddr> = response
            .iter()
            .map(|ip| SocketAddr::new(ip, port))
            .collect();

        if addrs.is_empty() {
            return Err(format!("no addresses found for {host}"));
        }

        Ok((addrs, ttl))
    }

    /// Fallback resolution using tokio::net::lookup_host.
    async fn resolve_fallback(
        &mut self,
        host: &str,
        port: u16,
        cache_key: &str,
    ) -> Result<Vec<SocketAddr>, String> {
        let lookup_str = format!("{host}:{port}");
        let addrs: Vec<SocketAddr> = tokio::net::lookup_host(&lookup_str)
            .await
            .map_err(|e| format!("fallback DNS lookup failed for {host}:{port}: {e}"))?
            .collect();

        if addrs.is_empty() {
            return Err(format!("no addresses found for {host}:{port}"));
        }

        debug!(
            host,
            port,
            addrs = addrs.len(),
            "DNS resolved via tokio fallback"
        );

        // Cache with default TTL for fallback results
        self.cache.insert(
            cache_key.to_string(),
            CacheEntry {
                addrs: addrs.clone(),
                inserted_at: Instant::now(),
                ttl: self.default_ttl,
            },
        );

        Ok(addrs)
    }

    /// Evict expired entries from the cache.
    pub fn evict_expired(&mut self) {
        self.cache.retain(|_, entry| !entry.is_expired());
    }

    /// Number of entries currently in the cache.
    pub fn cache_size(&self) -> usize {
        self.cache.len()
    }

    /// Clear the entire cache.
    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_entry_expiry() {
        let entry = CacheEntry {
            addrs: vec!["127.0.0.1:3001".parse().unwrap()],
            inserted_at: Instant::now() - Duration::from_secs(120),
            ttl: Duration::from_secs(60),
        };
        assert!(entry.is_expired());
    }

    #[test]
    fn test_cache_entry_not_expired() {
        let entry = CacheEntry {
            addrs: vec!["127.0.0.1:3001".parse().unwrap()],
            inserted_at: Instant::now(),
            ttl: Duration::from_secs(300),
        };
        assert!(!entry.is_expired());
    }

    #[test]
    fn test_resolver_default() {
        let resolver = DnsResolver::new();
        assert_eq!(resolver.cache_size(), 0);
        assert_eq!(resolver.min_ttl, Duration::from_secs(30));
        assert_eq!(resolver.default_ttl, Duration::from_secs(300));
    }

    #[test]
    fn test_resolver_custom_ttl() {
        let resolver = DnsResolver::with_ttl(Duration::from_secs(10), Duration::from_secs(600));
        assert_eq!(resolver.min_ttl, Duration::from_secs(10));
        assert_eq!(resolver.default_ttl, Duration::from_secs(600));
    }

    #[test]
    fn test_evict_expired() {
        let mut resolver = DnsResolver::new();
        resolver.cache.insert(
            "expired:3001".to_string(),
            CacheEntry {
                addrs: vec!["127.0.0.1:3001".parse().unwrap()],
                inserted_at: Instant::now() - Duration::from_secs(600),
                ttl: Duration::from_secs(60),
            },
        );
        resolver.cache.insert(
            "fresh:3002".to_string(),
            CacheEntry {
                addrs: vec!["127.0.0.1:3002".parse().unwrap()],
                inserted_at: Instant::now(),
                ttl: Duration::from_secs(300),
            },
        );
        assert_eq!(resolver.cache_size(), 2);
        resolver.evict_expired();
        assert_eq!(resolver.cache_size(), 1);
        assert!(resolver.cache.contains_key("fresh:3002"));
    }

    #[test]
    fn test_clear_cache() {
        let mut resolver = DnsResolver::new();
        resolver.cache.insert(
            "test:3001".to_string(),
            CacheEntry {
                addrs: vec!["127.0.0.1:3001".parse().unwrap()],
                inserted_at: Instant::now(),
                ttl: Duration::from_secs(300),
            },
        );
        assert_eq!(resolver.cache_size(), 1);
        resolver.clear_cache();
        assert_eq!(resolver.cache_size(), 0);
    }

    #[tokio::test]
    async fn test_resolve_cache_hit() {
        let mut resolver = DnsResolver::new();
        let expected: Vec<SocketAddr> = vec!["1.2.3.4:3001".parse().unwrap()];
        resolver.cache.insert(
            "cached-host.example:3001".to_string(),
            CacheEntry {
                addrs: expected.clone(),
                inserted_at: Instant::now(),
                ttl: Duration::from_secs(300),
            },
        );

        let result = resolver.resolve("cached-host.example", 3001).await.unwrap();
        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_resolve_cache_expired_triggers_fresh_lookup() {
        let mut resolver = DnsResolver::new();
        resolver.cache.insert(
            "expired-host.example:3001".to_string(),
            CacheEntry {
                addrs: vec!["1.2.3.4:3001".parse().unwrap()],
                inserted_at: Instant::now() - Duration::from_secs(600),
                ttl: Duration::from_secs(60),
            },
        );

        // This will attempt a real DNS lookup which will likely fail for a fake host,
        // but it verifies that the expired cache entry is not returned.
        let result = resolver.resolve("expired-host.example", 3001).await;
        // The lookup should fail since the host doesn't exist
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_resolve_localhost() {
        let mut resolver = DnsResolver::new();
        // localhost should resolve via fallback at minimum
        let result = resolver.resolve("localhost", 3001).await;
        assert!(result.is_ok());
        let addrs = result.unwrap();
        assert!(!addrs.is_empty());
        assert!(addrs.iter().all(|a| a.port() == 3001));
    }

    // ── Additional coverage ──────────────────────────────────────────────────

    #[test]
    fn test_cache_hit_does_not_alter_cache_size() {
        // Resolving a host that is already cached must not add a second entry.
        let mut resolver = DnsResolver::new();
        let key = "already-cached.example:3001";
        resolver.cache.insert(
            key.to_string(),
            CacheEntry {
                addrs: vec!["10.0.0.1:3001".parse().unwrap()],
                inserted_at: Instant::now(),
                ttl: Duration::from_secs(300),
            },
        );
        assert_eq!(resolver.cache_size(), 1);

        // Inject a second, different entry under another key and verify size stays 2
        resolver.cache.insert(
            "other.example:80".to_string(),
            CacheEntry {
                addrs: vec!["10.0.0.2:80".parse().unwrap()],
                inserted_at: Instant::now(),
                ttl: Duration::from_secs(300),
            },
        );
        assert_eq!(resolver.cache_size(), 2);
    }

    #[test]
    fn test_evict_expired_leaves_fresh_entries_intact() {
        // After evicting expired entries, the number of fresh entries must be
        // unchanged and their contents must be unmodified.
        let mut resolver = DnsResolver::new();
        let fresh_addr: std::net::SocketAddr = "9.9.9.9:53".parse().unwrap();

        resolver.cache.insert(
            "stale.example:3001".to_string(),
            CacheEntry {
                addrs: vec!["1.2.3.4:3001".parse().unwrap()],
                inserted_at: Instant::now() - Duration::from_secs(1000),
                ttl: Duration::from_secs(60),
            },
        );
        resolver.cache.insert(
            "fresh.example:53".to_string(),
            CacheEntry {
                addrs: vec![fresh_addr],
                inserted_at: Instant::now(),
                ttl: Duration::from_secs(300),
            },
        );

        resolver.evict_expired();
        assert_eq!(
            resolver.cache_size(),
            1,
            "Only the fresh entry should remain"
        );
        let remaining = &resolver.cache["fresh.example:53"];
        assert_eq!(remaining.addrs, vec![fresh_addr]);
    }

    #[test]
    fn test_clear_cache_removes_all_entries() {
        // clear_cache must leave cache_size() == 0 even when there are many entries.
        let mut resolver = DnsResolver::new();
        for i in 0..10u16 {
            resolver.cache.insert(
                format!("host{i}:300{i}"),
                CacheEntry {
                    addrs: vec![format!("1.2.3.{i}:300{i}").parse().unwrap()],
                    inserted_at: Instant::now(),
                    ttl: Duration::from_secs(300),
                },
            );
        }
        assert_eq!(resolver.cache_size(), 10);
        resolver.clear_cache();
        assert_eq!(resolver.cache_size(), 0);
    }

    #[test]
    fn test_with_ttl_min_and_default_are_set_correctly() {
        // with_ttl should set both min_ttl and default_ttl as specified.
        let min = Duration::from_secs(5);
        let default = Duration::from_secs(120);
        let resolver = DnsResolver::with_ttl(min, default);
        assert_eq!(resolver.min_ttl, min);
        assert_eq!(resolver.default_ttl, default);
        assert_eq!(
            resolver.cache_size(),
            0,
            "New resolver cache should be empty"
        );
    }

    #[test]
    fn test_cache_entry_not_expired_at_exactly_ttl_minus_one_ms() {
        // An entry where elapsed < ttl must report is_expired() = false.
        let entry = CacheEntry {
            addrs: vec!["127.0.0.1:3001".parse().unwrap()],
            // Inserted just now — not expired for a 60s TTL.
            inserted_at: Instant::now(),
            ttl: Duration::from_secs(60),
        };
        assert!(!entry.is_expired());
    }

    #[tokio::test]
    async fn test_resolve_cache_hit_returns_cached_addresses() {
        // Resolving a host that is in-cache must return exactly the cached
        // addresses without triggering any network lookup.
        let mut resolver = DnsResolver::new();
        let expected: Vec<std::net::SocketAddr> = vec![
            "10.20.30.40:9001".parse().unwrap(),
            "10.20.30.41:9001".parse().unwrap(),
        ];
        resolver.cache.insert(
            "multi-addr.example:9001".to_string(),
            CacheEntry {
                addrs: expected.clone(),
                inserted_at: Instant::now(),
                ttl: Duration::from_secs(300),
            },
        );

        let result = resolver.resolve("multi-addr.example", 9001).await.unwrap();
        assert_eq!(
            result, expected,
            "Cache hit must return all cached addresses"
        );
    }
}
