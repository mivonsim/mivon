//! metadata.mdb — informasi file sumber untuk incremental build.
//!
//! Satu record per file: hash konten (xxh3), mtime, size, status, hash flags,
//! dependencies, dan waktu kompilasi. Lookup O(1) via hash path → key.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cache::checksum::compute_checksum;

/// Status kompilasi sebuah file pada build terakhir.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileStatus {
    /// Belum pernah dikompilasi.
    New,
    /// Konten identik dengan build sebelumnya.
    Unchanged,
    /// File dikompilasi ulang pada build ini.
    Recompiled,
    /// File gagal diproses (parse error dll).
    Error,
}

/// Metadata satu file di database.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileMeta {
    pub path: PathBuf,
    /// Hash konten (xxh3-64) — dasar invalidation, bukan timestamp.
    pub content_hash: u64,
    /// mtime terakhir (nanoseconds since epoch) — informasi saja.
    pub mtime_ns: u64,
    /// Ukuran file (byte).
    pub size: u64,
    pub status: FileStatus,
    /// Hash flags compiler (defines/incdirs) — berubah → semua file dirty.
    pub flags_hash: u64,
    /// File yang menjadi dependensi file ini (instantiations/imports/includes).
    pub deps: Vec<PathBuf>,
    /// File include + hash kontennya saat build. Hash ini diverifikasi saat
    /// restore: jika header berubah, AST/preprocessed tidak boleh di-reuse
    /// (preprocessed output sudah meng-embed konten header yang lama).
    pub include_hashes: Vec<(PathBuf, u64)>,
    /// Waktu kompilasi terakhir (unix ns).
    pub compiled_at_ns: u64,
    /// Versi format AST yang menyimpan desain file ini.
    pub ast_format_version: u64,
}

/// Hash canonical sebuah path (untuk key di object store).
pub fn path_hash(path: &Path) -> u64 {
    compute_checksum(path.to_string_lossy().as_bytes())
}

/// Hash kombinasi flags compiler (defines + incdirs) → seluruh build invalid
/// bila flags berubah.
pub fn flags_hash(defines: &[(String, String)], incdirs: &[PathBuf]) -> u64 {
    let mut h = 0u64;
    for (k, v) in defines {
        h = h
            .wrapping_mul(31)
            .wrapping_add(compute_checksum(k.as_bytes()));
        h = h
            .wrapping_mul(31)
            .wrapping_add(compute_checksum(v.as_bytes()));
    }
    for d in incdirs {
        h = h.wrapping_mul(31).wrapping_add(path_hash(d));
    }
    h
}

/// Manifest: daftar semua path yang tersimpan (untuk enumerasi saat load).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetadataManifest {
    pub paths: Vec<PathBuf>,
    /// Hash flags kompilasi build terakhir.
    pub flags_hash: u64,
    /// Versi compiler.
    pub compiler_version: String,
    /// Waktu pembuatan database.
    pub created_ns: u64,
    /// Versi skema database (Kritik 3 db.md). Berubah → seluruh database
    /// lama dianggap tidak kompatibel dan dibangun ulang dari kosong.
    pub schema_version: u64,
    /// Fase 3: versi skema per stage (nama → versi). Dibandingkan tiap open;
    /// beda → rebuild (evict selektif per-store = lanjutan Fase 3b).
    #[serde(default)]
    pub stage_schemas: Vec<(String, u64)>,
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_hash_stable() {
        let p = Path::new("/home/user/project/uart.sv");
        assert_eq!(path_hash(p), path_hash(p));
        assert_ne!(
            path_hash(p),
            path_hash(Path::new("/home/user/project/uart2.sv"))
        );
    }

    #[test]
    fn test_flags_hash_detects_change() {
        let a = flags_hash(&[("TOP".into(), "1".into())], &[]);
        let b = flags_hash(&[("TOP".into(), "0".into())], &[]);
        let c = flags_hash(&[("TOP".into(), "1".into())], &[PathBuf::from("inc/")]);
        assert_eq!(a, a);
        assert_ne!(a, b);
        assert_ne!(a, c);
    }
}
