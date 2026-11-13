//! Lapisan artefak pipeline per kategori (`cache/<pid>/<category>/`).
//!
//! Ini BUKAN cache komputasi dan bukan database: hanya artefak turunan
//! (ringkasan elaborate/generate/optimize/expression/lint/coverage/simulation/
//! waveform) yang dibaca tool tanpa compile ulang. Cache hit kompilasi tetap
//! ditentukan node content-addressed.

use crate::filesystem::{atomic_write, sanitize_name, Layout};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

/// Kategori artefak pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CacheCategory {
    Preprocess,
    Lexer,
    Parser,
    Semantic,
    Verify,
    Macro,
    Include,
    Dependency,
    Resolve,
    Constant,
    Type,
    Hierarchy,
    Elaborate,
    Generate,
    Optimize,
    Expression,
    Simulation,
    Waveform,
    Coverage,
    Lint,
    Profile,
}

impl CacheCategory {
    pub const ALL: [CacheCategory; 21] = [
        CacheCategory::Preprocess,
        CacheCategory::Lexer,
        CacheCategory::Parser,
        CacheCategory::Semantic,
        CacheCategory::Verify,
        CacheCategory::Macro,
        CacheCategory::Include,
        CacheCategory::Dependency,
        CacheCategory::Resolve,
        CacheCategory::Constant,
        CacheCategory::Type,
        CacheCategory::Hierarchy,
        CacheCategory::Elaborate,
        CacheCategory::Generate,
        CacheCategory::Optimize,
        CacheCategory::Expression,
        CacheCategory::Simulation,
        CacheCategory::Waveform,
        CacheCategory::Coverage,
        CacheCategory::Lint,
        CacheCategory::Profile,
    ];

    /// Nama direktori kategori.
    pub fn name(self) -> &'static str {
        match self {
            CacheCategory::Preprocess => "preprocess",
            CacheCategory::Lexer => "lexer",
            CacheCategory::Parser => "parser",
            CacheCategory::Semantic => "semantic",
            CacheCategory::Verify => "verify",
            CacheCategory::Macro => "macro",
            CacheCategory::Include => "include",
            CacheCategory::Dependency => "dependency",
            CacheCategory::Resolve => "resolve",
            CacheCategory::Constant => "constant",
            CacheCategory::Type => "type",
            CacheCategory::Hierarchy => "hierarchy",
            CacheCategory::Elaborate => "elaborate",
            CacheCategory::Generate => "generate",
            CacheCategory::Optimize => "optimize",
            CacheCategory::Expression => "expression",
            CacheCategory::Simulation => "simulation",
            CacheCategory::Waveform => "waveform",
            CacheCategory::Coverage => "coverage",
            CacheCategory::Lint => "lint",
            CacheCategory::Profile => "profile",
        }
    }
}

/// Store satu kategori: map key → payload (blob biner versioned).
#[derive(Debug, Clone, Default)]
pub struct CategoryStore {
    pub category: CacheCategory,
    pub root: PathBuf,
    pub entries: HashMap<String, Vec<u8>>,
    pub dirty: bool,
}

impl CategoryStore {
    pub fn open(root: PathBuf, category: CacheCategory) -> Self {
        let mut store = CategoryStore {
            category,
            root,
            entries: HashMap::new(),
            dirty: false,
        };
        if let Ok(bytes) = std::fs::read(store.blob_path()) {
            if let Ok(stored) = bincode::deserialize::<StoredCategory>(&bytes) {
                if stored.version == STORE_VERSION && stored.category == category as u8 {
                    store.entries = stored.entries.into_iter().collect();
                }
            }
        }
        store
    }

    fn blob_path(&self) -> PathBuf {
        self.root.join(format!("{}.bin", self.category.name()))
    }

    pub fn keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.entries.keys().cloned().collect();
        keys.sort();
        keys
    }

    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.entries.get(key).cloned()
    }

    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    pub fn put(&mut self, key: &str, bytes: &[u8]) -> u64 {
        self.entries.insert(key.to_string(), bytes.to_vec());
        self.dirty = true;
        crate::dependency::checksum64(bytes)
    }

    pub fn remove(&mut self, key: &str) -> bool {
        let removed = self.entries.remove(key).is_some();
        if removed {
            self.dirty = true;
        }
        removed
    }

    pub fn bytes(&self) -> u64 {
        self.entries.values().map(|v| v.len() as u64).sum()
    }

    pub fn save(&mut self) -> io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        std::fs::create_dir_all(&self.root)?;
        let stored = StoredCategory {
            version: STORE_VERSION,
            category: self.category as u8,
            entries: self
                .entries
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        };
        let bytes = bincode::serialize(&stored).map_err(io::Error::other)?;
        atomic_write(&self.blob_path(), &bytes)?;
        self.dirty = false;
        Ok(())
    }
}

const STORE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct StoredCategory {
    version: u32,
    category: u8,
    entries: Vec<(String, Vec<u8>)>,
}

/// Statistik satu kategori.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CategoryStats {
    pub category: CacheCategory,
    pub entries: usize,
    pub bytes: u64,
    pub hits: u64,
    pub misses: u64,
}

impl CategoryStats {
    pub fn hit_rate_pct(&self) -> u8 {
        let total = self.hits + self.misses;
        if total == 0 {
            return 0;
        }
        ((self.hits as f64 / total as f64) * 100.0) as u8
    }
}

impl Default for CacheCategory {
    fn default() -> Self {
        CacheCategory::Preprocess
    }
}

/// Statistik seluruh lapisan.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheLayerStats {
    pub stores: usize,
    pub total_entries: usize,
    pub total_bytes: u64,
    /// Entri yang ditulis ulang (0 — artefak kini immutable/deduplicated).
    pub rebuilt: u64,
    pub per_category: Vec<CategoryStats>,
}

impl CacheLayerStats {
    pub fn summary(&self) -> String {
        format!(
            "{} kategori, {} entri, {} byte",
            self.stores, self.total_entries, self.total_bytes
        )
    }

    /// Hit rate agregat seluruh kategori (0..100).
    pub fn hit_rate_pct(&self) -> u8 {
        let hits: u64 = self.per_category.iter().map(|c| c.hits).sum();
        let misses: u64 = self.per_category.iter().map(|c| c.misses).sum();
        let total = hits + misses;
        if total == 0 {
            return 0;
        }
        ((hits as f64 / total as f64) * 100.0) as u8
    }
}

/// Aggregator lapisan cache per project.
#[derive(Debug)]
pub struct CacheLayer {
    /// `<root>/cache/<pid>/`.
    pub root: PathBuf,
    stores: HashMap<CacheCategory, CategoryStore>,
    hits: HashMap<CacheCategory, u64>,
    misses: HashMap<CacheCategory, u64>,
}

impl CacheLayer {
    /// Buka lapisan cache; membuat direktori bila perlu.
    pub fn open(db_root: &Path, pid: &str, _config_hash: u64) -> io::Result<CacheLayer> {
        let layout = Layout::new(db_root);
        let root = layout.root().join("cache").join(sanitize_name(pid));
        std::fs::create_dir_all(&root)?;
        let mut stores = HashMap::with_capacity(CacheCategory::ALL.len());
        for cat in CacheCategory::ALL {
            stores.insert(cat, CategoryStore::open(root.clone(), cat));
        }
        Ok(CacheLayer {
            root,
            stores,
            hits: HashMap::new(),
            misses: HashMap::new(),
        })
    }

    pub fn category_dir(&self, cat: CacheCategory) -> PathBuf {
        self.root.join(cat.name())
    }

    pub fn store(&self, cat: CacheCategory) -> Option<&CategoryStore> {
        self.stores.get(&cat)
    }

    pub fn store_mut(&mut self, cat: CacheCategory) -> Option<&mut CategoryStore> {
        self.stores.get_mut(&cat)
    }

    pub fn put(&mut self, cat: CacheCategory, key: &str, bytes: &[u8]) -> Option<u64> {
        self.stores.get_mut(&cat)?.put(key, bytes).into()
    }

    pub fn get(&mut self, cat: CacheCategory, key: &str) -> Option<Vec<u8>> {
        let entry = self.store(cat)?.get(key);
        let counter = if entry.is_some() {
            self.hits.entry(cat).or_insert(0)
        } else {
            self.misses.entry(cat).or_insert(0)
        };
        *counter += 1;
        entry
    }

    pub fn contains(&self, cat: CacheCategory, key: &str) -> bool {
        self.stores
            .get(&cat)
            .map(|s| s.contains(key))
            .unwrap_or(false)
    }

    pub fn remove(&mut self, cat: CacheCategory, key: &str) -> bool {
        self.stores
            .get_mut(&cat)
            .map(|s| s.remove(key))
            .unwrap_or(false)
    }

    /// Simpan seluruh kategori yang berubah.
    pub fn save(&mut self) -> io::Result<()> {
        for store in self.stores.values_mut() {
            store.save()?;
        }
        Ok(())
    }

    /// Statistik per kategori.
    pub fn stats(&self) -> CacheLayerStats {
        let mut out = CacheLayerStats {
            stores: self.stores.len(),
            ..Default::default()
        };
        let mut cats: Vec<CacheCategory> = self.stores.keys().copied().collect();
        cats.sort();
        for cat in cats {
            let store = &self.stores[&cat];
            let entries = store.entries.len();
            let bytes = store.bytes();
            out.total_entries += entries;
            out.total_bytes += bytes;
            out.per_category.push(CategoryStats {
                category: cat,
                entries,
                bytes,
                hits: self.hits.get(&cat).copied().unwrap_or(0),
                misses: self.misses.get(&cat).copied().unwrap_or(0),
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::unique_suffix;

    fn temp_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "mivon-cachelayer-{}-{}-{}",
            tag,
            std::process::id(),
            unique_suffix()
        ))
    }

    #[test]
    fn put_get_and_persist() {
        let root = temp_root("persist");
        let mut layer = CacheLayer::open(&root, "pid", 0).unwrap();
        layer.put(CacheCategory::Elaborate, "top", b"payload");
        assert!(layer.contains(CacheCategory::Elaborate, "top"));
        assert_eq!(
            layer.get(CacheCategory::Elaborate, "top").as_deref(),
            Some(&b"payload"[..])
        );
        layer.save().unwrap();

        let mut reopened = CacheLayer::open(&root, "pid", 0).unwrap();
        assert_eq!(
            reopened.get(CacheCategory::Elaborate, "top").as_deref(),
            Some(&b"payload"[..])
        );
        let keys = reopened
            .store(CacheCategory::Elaborate)
            .unwrap()
            .keys();
        assert_eq!(keys, vec!["top".to_string()]);
        let st = reopened.stats();
        assert_eq!(st.stores, 21);
        assert_eq!(st.total_entries, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn corrupted_category_blob_is_ignored() {
        let root = temp_root("corrupt");
        let mut layer = CacheLayer::open(&root, "pid", 0).unwrap();
        layer.put(CacheCategory::Lint, "report", b"x");
        layer.save().unwrap();
        let path = layer.root.join("lint.bin");
        std::fs::write(&path, b"garbage").unwrap();
        let reopened = CacheLayer::open(&root, "pid", 0).unwrap();
        assert!(reopened.store(CacheCategory::Lint).unwrap().keys().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
