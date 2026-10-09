//! manifest — metadata cache kategori (Kritik 3 db.md: versioned schema).
//!
//! `manifest.mdb` di root tiap kategori cache menyimpan versi skema, versi
//! compiler, hash konfigurasi (defines/incdirs), waktu, dan ringkasan isi.
//! Skema berubah → kategori dibangun ulang dari kosong (store lama tidak
//! kompatibel).

use serde::{Deserialize, Serialize};

use super::super::{compiler_base_matches, COMPILER_VERSION};
use super::super::verify::now_ns;

/// Versi skema lapisan `cache/`. Naikkan bila struktur persistensi kategori
/// berubah (field payload, layout index). Store dengan versi berbeda dianggap
/// tidak kompatibel → di-rebuild (Kritik 3 db.md).
pub const CACHE_SCHEMA_VERSION: u64 = 1;

/// Metadata satu kategori cache, disimpan di `manifest.mdb`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CacheManifest {
    pub schema_version: u64,
    pub compiler_version: String,
    /// Hash konfigurasi build (defines + incdirs) — berubah → hasil berbeda.
    pub config_hash: u64,
    pub created_ns: u64,
    pub updated_ns: u64,
    pub entry_count: u64,
    /// Total bytes payload.
    pub bytes: u64,
}

impl CacheManifest {
    pub fn fresh(config_hash: u64) -> Self {
        CacheManifest {
            schema_version: CACHE_SCHEMA_VERSION,
            compiler_version: COMPILER_VERSION.to_string(),
            config_hash,
            created_ns: now_ns(),
            updated_ns: now_ns(),
            entry_count: 0,
            bytes: 0,
        }
    }

    /// Apakah skema kompatibel dengan versi terkini. Fase 3 (Kritik C):
    /// bandingkan BASE compiler (bump `-p<N>` manual) — fingerprint binary
    /// tidak lagi me-rebuild; manifest legacy (base+suffix) tetap diterima.
    pub fn valid(&self) -> bool {
        self.schema_version == CACHE_SCHEMA_VERSION
            && compiler_base_matches(&self.compiler_version)
    }
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fresh_is_valid() {
        let m = CacheManifest::fresh(42);
        assert!(m.valid());
        assert_eq!(m.schema_version, CACHE_SCHEMA_VERSION);
        assert_eq!(m.config_hash, 42);
    }

    #[test]
    fn test_old_schema_invalid() {
        let m = CacheManifest {
            schema_version: CACHE_SCHEMA_VERSION - 1,
            ..CacheManifest::fresh(0)
        };
        assert!(!m.valid());
    }

    #[test]
    fn test_serialize_roundtrip() {
        let m = CacheManifest::fresh(7);
        let bytes = bincode::serialize(&m).unwrap();
        let m2: CacheManifest = bincode::deserialize(&bytes).unwrap();
        assert_eq!(m, m2);
    }
}
