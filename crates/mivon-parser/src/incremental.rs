//! Incremental parsing support (PARSER-01, Critical - foundational piece).
//!
//! Primitive ini melacak hash konten tiap file sumber sehingga parser hanya
//! perlu mem-parse ulang file yang benar-benar berubah sejak run terakhir,
//! bukan seluruh design. Ini bangunan dasar untuk incremental parsing;
//! integrasi penuh ke Parser::parse_design menyusul.
//!
//! Satu file = satu tanggung jawab.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

/// Tracker hash konten file untuk menentukan file mana yang perlu di-parse ulang.
#[derive(Debug, Clone, Default)]
pub struct IncrementalCache {
    hashes: HashMap<PathBuf, u64>,
}

fn hash_str(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

impl IncrementalCache {
    /// Cache kosong.
    pub fn new() -> Self {
        Self::default()
    }

    /// Cache dari map hash yang sudah ada (mis. dimuat dari disk).
    pub fn from_map(map: HashMap<PathBuf, u64>) -> Self {
        Self { hashes: map }
    }

    /// Ekstrak map hash saat ini (untuk disimpan ke disk).
    pub fn into_map(self) -> HashMap<PathBuf, u64> {
        self.hashes
    }

    /// True bila file perlu di-parse ulang: konten berubah atau belum pernah
    /// terlihat pada run sebelumnya. Update state cache.
    pub fn needs_reparse(&mut self, path: &PathBuf, content: &str) -> bool {
        let h = hash_str(content);
        match self.hashes.get(path) {
            Some(&old) if old == h => false,
            _ => {
                self.hashes.insert(path.clone(), h);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_file_skips_reparse() {
        let mut cache = IncrementalCache::new();
        let p = PathBuf::from("a.sv");
        assert!(cache.needs_reparse(&p, "module a; endmodule"));
        assert!(!cache.needs_reparse(&p, "module a; endmodule"));
    }

    #[test]
    fn changed_content_triggers_reparse() {
        let mut cache = IncrementalCache::new();
        let p = PathBuf::from("a.sv");
        assert!(cache.needs_reparse(&p, "module a; endmodule"));
        assert!(cache.needs_reparse(&p, "module a; // changed"));
        assert!(cache.needs_reparse(&p, "module a; endmodule"));
    }

    #[test]
    fn distinct_files_tracked_separately() {
        let mut cache = IncrementalCache::new();
        let a = PathBuf::from("a.sv");
        let b = PathBuf::from("b.sv");
        assert!(cache.needs_reparse(&a, "A"));
        assert!(cache.needs_reparse(&b, "B"));
        assert!(!cache.needs_reparse(&a, "A"));
        assert!(!cache.needs_reparse(&b, "B"));
    }

    #[test]
    fn map_roundtrip_preserves_state() {
        let mut cache = IncrementalCache::new();
        let p = PathBuf::from("a.sv");
        assert!(cache.needs_reparse(&p, "X"));
        let map = cache.into_map();
        let mut cache2 = IncrementalCache::from_map(map);
        assert!(!cache2.needs_reparse(&p, "X"));
    }
}
