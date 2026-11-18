//! ENT-04: Remote Distributed Caching — client/server for compile cache sharing.
//!
//! Framework untuk berbagi compile cache antar developer/mesin.
//! Cache entries diidentifikasi oleh content hash dan bisa di-store
//! di remote server untuk sharing.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Cache entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub key: String,
    pub data: Vec<u8>,
    pub size: u64,
    pub created_at: u64,
    pub expires_at: Option<u64>,
    pub tags: Vec<String>,
}

/// Cache statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStats {
    pub total_entries: usize,
    pub total_size_bytes: u64,
    pub hits: u64,
    pub misses: u64,
    pub hit_rate: f64,
}

/// Remote cache client — connects to a cache server.
pub struct RemoteCacheClient {
    server_url: String,
    local_cache: Mutex<HashMap<String, CacheEntry>>,
    timeout: Duration,
    stats_hits: Mutex<u64>,
    stats_misses: Mutex<u64>,
}

impl RemoteCacheClient {
    pub fn new(server_url: &str) -> Self {
        RemoteCacheClient {
            server_url: server_url.to_string(),
            local_cache: Mutex::new(HashMap::new()),
            timeout: Duration::from_secs(30),
            stats_hits: Mutex::new(0),
            stats_misses: Mutex::new(0),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Get cache entry by key.
    pub fn get(&self, key: &str) -> Option<CacheEntry> {
        // Check local cache first
        if let Some(entry) = self.local_cache.lock().unwrap().get(key) {
            if !is_expired(entry) {
                *self.stats_hits.lock().unwrap() += 1;
                return Some(entry.clone());
            }
        }

        // In real impl, would fetch from remote server
        *self.stats_misses.lock().unwrap() += 1;
        None
    }

    /// Set cache entry.
    pub fn set(&self, entry: CacheEntry) {
        self.local_cache
            .lock()
            .unwrap()
            .insert(entry.key.clone(), entry);
    }

    /// Get or compute (cache-through pattern).
    pub fn get_or_compute<F>(&self, key: &str, compute: F) -> CacheEntry
    where
        F: FnOnce() -> Vec<u8>,
    {
        if let Some(entry) = self.get(key) {
            return entry;
        }

        let data = compute();
        let entry = CacheEntry {
            key: key.to_string(),
            size: data.len() as u64,
            created_at: now_secs(),
            expires_at: None,
            tags: Vec::new(),
            data,
        };
        self.set(entry.clone());
        entry
    }

    /// Invalidate key.
    pub fn invalidate(&self, key: &str) -> bool {
        self.local_cache.lock().unwrap().remove(key).is_some()
    }

    /// Invalidate by tag.
    pub fn invalidate_tag(&self, tag: &str) -> usize {
        let mut cache = self.local_cache.lock().unwrap();
        let before = cache.len();
        cache.retain(|_, e| !e.tags.contains(&tag.to_string()));
        before - cache.len()
    }

    /// Get cache stats.
    pub fn stats(&self) -> CacheStats {
        let hits = *self.stats_hits.lock().unwrap();
        let misses = *self.stats_misses.lock().unwrap();
        let cache = self.local_cache.lock().unwrap();
        let total_entries = cache.len();
        let total_size = cache.values().map(|e| e.size).sum();
        let total = hits + misses;

        CacheStats {
            total_entries,
            total_size_bytes: total_size,
            hits,
            misses,
            hit_rate: if total > 0 {
                hits as f64 / total as f64 * 100.0
            } else {
                0.0
            },
        }
    }

    /// Server URL.
    pub fn server_url(&self) -> &str {
        &self.server_url
    }
}

/// Remote cache server — stores cache entries.
pub struct RemoteCacheServer {
    entries: Mutex<HashMap<String, CacheEntry>>,
    max_size_bytes: u64,
    evictions: Mutex<u64>,
}

impl RemoteCacheServer {
    pub fn new(max_size_bytes: u64) -> Self {
        RemoteCacheServer {
            entries: Mutex::new(HashMap::new()),
            max_size_bytes,
            evictions: Mutex::new(0),
        }
    }

    /// Store entry (with LRU eviction).
    pub fn put(&self, entry: CacheEntry) -> bool {
        let mut entries = self.entries.lock().unwrap();
        let current_size: u64 = entries.values().map(|e| e.size).sum();

        // Evict if needed
        if current_size + entry.size > self.max_size_bytes {
            let needed = entry.size;
            let mut evicted = 0u64;
            // Evict oldest entries
            let mut keys_to_remove: Vec<String> = Vec::new();
            {
                let mut sorted: Vec<_> = entries.iter().collect();
                sorted.sort_by_key(|(_, e)| e.created_at);
                for (key, e) in sorted {
                    if evicted >= needed {
                        break;
                    }
                    evicted += e.size;
                    keys_to_remove.push(key.clone());
                }
            }
            for key in &keys_to_remove {
                entries.remove(key);
            }
            *self.evictions.lock().unwrap() += 1;
        }

        entries.insert(entry.key.clone(), entry);
        true
    }

    /// Get entry.
    pub fn get(&self, key: &str) -> Option<CacheEntry> {
        self.entries
            .lock()
            .unwrap()
            .get(key)
            .filter(|e| !is_expired(e))
            .cloned()
    }

    /// Get entries by tag.
    pub fn get_by_tag(&self, tag: &str) -> Vec<CacheEntry> {
        self.entries
            .lock()
            .unwrap()
            .values()
            .filter(|e| e.tags.contains(&tag.to_string()))
            .cloned()
            .collect()
    }

    /// List all keys.
    pub fn keys(&self) -> Vec<String> {
        self.entries.lock().unwrap().keys().cloned().collect()
    }

    /// Clear all entries.
    pub fn clear(&self) {
        self.entries.lock().unwrap().clear();
    }

    /// Size stats.
    pub fn size(&self) -> (usize, u64) {
        let entries = self.entries.lock().unwrap();
        let total: u64 = entries.values().map(|e| e.size).sum();
        (entries.len(), total)
    }

    /// Number of evictions performed so far.
    pub fn evictions(&self) -> u64 {
        *self.evictions.lock().unwrap()
    }

    pub fn summary(&self) -> String {
        let (count, size) = self.size();
        format!(
            "RemoteCacheServer: {} entries, {} bytes, {} evictions",
            count,
            size,
            *self.evictions.lock().unwrap()
        )
    }
}

fn is_expired(entry: &CacheEntry) -> bool {
    if let Some(exp) = entry.expires_at {
        now_secs() > exp
    } else {
        false
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_set_get() {
        let client = RemoteCacheClient::new("http://localhost:8080");
        let entry = CacheEntry {
            key: "k1".into(),
            data: b"hello".to_vec(),
            size: 5,
            created_at: now_secs(),
            expires_at: None,
            tags: vec![],
        };
        client.set(entry.clone());
        let got = client.get("k1").unwrap();
        assert_eq!(got.data, entry.data);
    }

    #[test]
    fn test_get_or_compute() {
        let client = RemoteCacheClient::new("http://localhost:8080");
        let entry = client.get_or_compute("computed", || b"computed_data".to_vec());
        assert_eq!(entry.data, b"computed_data");

        // Second call should hit cache
        let entry2 = client.get_or_compute("computed", || b"should_not_run".to_vec());
        assert_eq!(entry2.data, b"computed_data");
    }

    #[test]
    fn test_stats() {
        let client = RemoteCacheClient::new("http://localhost:8080");
        client.get("miss1");
        client.get("miss2");
        let entry = CacheEntry {
            key: "k1".into(),
            data: vec![],
            size: 0,
            created_at: now_secs(),
            expires_at: None,
            tags: vec![],
        };
        client.set(entry);
        client.get("k1");

        let stats = client.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 2);
    }

    #[test]
    fn test_server_put_get() {
        let server = RemoteCacheServer::new(1024 * 1024);
        let entry = CacheEntry {
            key: "k1".into(),
            data: b"hello".to_vec(),
            size: 5,
            created_at: now_secs(),
            expires_at: None,
            tags: vec![],
        };
        server.put(entry);
        assert!(server.get("k1").is_some());
    }

    #[test]
    fn test_server_eviction() {
        let server = RemoteCacheServer::new(10); // tiny!
        for i in 0..5 {
            server.put(CacheEntry {
                key: format!("k{}", i),
                data: vec![i as u8; 3],
                size: 3,
                created_at: now_secs() + i,
                expires_at: None,
                tags: vec![],
            });
        }
        let (count, _) = server.size();
        assert!(count <= 4); // some evicted
    }

    #[test]
    fn test_server_evictions_count() {
        let server = RemoteCacheServer::new(10);
        assert_eq!(server.evictions(), 0);
        for i in 0..3 {
            server.put(CacheEntry {
                key: format!("k{}", i),
                data: vec![i as u8; 5],
                size: 5,
                created_at: now_secs() + i,
                expires_at: None,
                tags: vec![],
            });
        }
        // Each put beyond capacity triggers at least one eviction.
        assert!(server.evictions() >= 1);
    }

    #[test]
    fn test_invalidate_tag() {
        let client = RemoteCacheClient::new("http://localhost:8080");
        client.set(CacheEntry {
            key: "a".into(),
            data: vec![],
            size: 0,
            created_at: now_secs(),
            expires_at: None,
            tags: vec!["old".into()],
        });
        client.set(CacheEntry {
            key: "b".into(),
            data: vec![],
            size: 0,
            created_at: now_secs(),
            expires_at: None,
            tags: vec!["new".into()],
        });
        let removed = client.invalidate_tag("old");
        assert_eq!(removed, 1);
        assert!(client.get("a").is_none());
        assert!(client.get("b").is_some());
    }
}
