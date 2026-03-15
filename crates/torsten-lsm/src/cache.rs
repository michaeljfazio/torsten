//! LRU block cache for SSTable pages.
//!
//! Caches decoded pages to avoid repeated disk reads and deserialization.
//! Uses a clock-sweep approximation of LRU for O(1) amortized eviction.

use std::collections::HashMap;

use crate::sstable::page::Page;

/// Cache key: (run_id, page_index).
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
pub struct CacheKey {
    pub run_id: u64,
    pub page_index: u32,
}

/// Entry in the cache with a reference bit for clock-sweep eviction.
struct CacheEntry {
    page: Page,
    referenced: bool,
}

/// LRU-approximating block cache using clock-sweep eviction.
///
/// Thread safety: callers must wrap in a Mutex for concurrent access.
pub struct BlockCache {
    map: HashMap<CacheKey, CacheEntry>,
    /// Ordered keys for clock-sweep (circular buffer).
    keys: Vec<CacheKey>,
    /// Clock hand position.
    hand: usize,
    /// Maximum number of pages to cache.
    capacity: usize,
}

impl BlockCache {
    /// Create a new cache with the given capacity (number of pages).
    pub fn new(capacity: usize) -> Self {
        BlockCache {
            map: HashMap::with_capacity(capacity),
            keys: Vec::with_capacity(capacity),
            hand: 0,
            capacity,
        }
    }

    /// Look up a cached page. Marks the entry as recently used.
    pub fn get(&mut self, key: &CacheKey) -> Option<&Page> {
        if let Some(entry) = self.map.get_mut(key) {
            entry.referenced = true;
            Some(&entry.page)
        } else {
            None
        }
    }

    /// Insert a page into the cache, evicting if necessary.
    pub fn insert(&mut self, key: CacheKey, page: Page) {
        if self.capacity == 0 {
            return;
        }

        // If already present, just update
        if let Some(entry) = self.map.get_mut(&key) {
            entry.page = page;
            entry.referenced = true;
            return;
        }

        // Evict if at capacity
        while self.map.len() >= self.capacity {
            self.evict_one();
        }

        self.keys.push(key);
        self.map.insert(
            key,
            CacheEntry {
                page,
                referenced: true,
            },
        );
    }

    /// Remove all entries for a given run (used when a run is deleted during compaction).
    pub fn invalidate_run(&mut self, run_id: u64) {
        self.keys.retain(|k| k.run_id != run_id);
        self.map.retain(|k, _| k.run_id != run_id);
        if !self.keys.is_empty() {
            self.hand %= self.keys.len();
        } else {
            self.hand = 0;
        }
    }

    /// Number of cached pages.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Evict one entry using clock-sweep.
    fn evict_one(&mut self) {
        if self.keys.is_empty() {
            return;
        }

        // Sweep until we find an unreferenced entry
        let len = self.keys.len();
        for _ in 0..len * 2 {
            let key = self.keys[self.hand];
            if let Some(entry) = self.map.get_mut(&key) {
                if entry.referenced {
                    entry.referenced = false;
                    self.hand = (self.hand + 1) % len;
                } else {
                    // Evict this entry
                    self.map.remove(&key);
                    self.keys.swap_remove(self.hand);
                    if !self.keys.is_empty() {
                        self.hand %= self.keys.len();
                    } else {
                        self.hand = 0;
                    }
                    return;
                }
            } else {
                // Stale key in the keys list — remove it
                self.keys.swap_remove(self.hand);
                if !self.keys.is_empty() {
                    self.hand %= self.keys.len();
                } else {
                    self.hand = 0;
                }
                return;
            }
        }

        // All entries were recently referenced — evict the current one anyway
        let key = self.keys[self.hand];
        self.map.remove(&key);
        self.keys.swap_remove(self.hand);
        if !self.keys.is_empty() {
            self.hand %= self.keys.len();
        } else {
            self.hand = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::Key;
    use crate::value::Value;

    fn make_page(n: u8) -> Page {
        Page {
            entries: vec![(Key::from([n]), Some(Value::from([n])))],
        }
    }

    #[test]
    fn test_cache_insert_and_get() {
        let mut cache = BlockCache::new(10);
        let key = CacheKey {
            run_id: 1,
            page_index: 0,
        };
        cache.insert(key, make_page(1));

        let found = cache.get(&key).unwrap();
        assert_eq!(found.entries.len(), 1);
    }

    #[test]
    fn test_cache_miss() {
        let mut cache = BlockCache::new(10);
        let key = CacheKey {
            run_id: 1,
            page_index: 0,
        };
        assert!(cache.get(&key).is_none());
    }

    #[test]
    fn test_cache_eviction() {
        let mut cache = BlockCache::new(3);

        for i in 0..5u32 {
            cache.insert(
                CacheKey {
                    run_id: 1,
                    page_index: i,
                },
                make_page(i as u8),
            );
        }

        // Should have evicted some entries
        assert!(cache.len() <= 3);
    }

    #[test]
    fn test_cache_invalidate_run() {
        let mut cache = BlockCache::new(10);
        cache.insert(
            CacheKey {
                run_id: 1,
                page_index: 0,
            },
            make_page(1),
        );
        cache.insert(
            CacheKey {
                run_id: 1,
                page_index: 1,
            },
            make_page(2),
        );
        cache.insert(
            CacheKey {
                run_id: 2,
                page_index: 0,
            },
            make_page(3),
        );

        cache.invalidate_run(1);
        assert_eq!(cache.len(), 1);
        assert!(cache
            .get(&CacheKey {
                run_id: 1,
                page_index: 0
            })
            .is_none());
        assert!(cache
            .get(&CacheKey {
                run_id: 2,
                page_index: 0
            })
            .is_some());
    }

    #[test]
    fn test_cache_zero_capacity() {
        let mut cache = BlockCache::new(0);
        cache.insert(
            CacheKey {
                run_id: 1,
                page_index: 0,
            },
            make_page(1),
        );
        assert!(cache.is_empty());
    }
}
