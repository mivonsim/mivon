//! Validasi dependency: fingerprint input + invalidation inkremental
//! (node_cache.md §7).
//!
//! Empat lapisan validasi yang dipakai resolver:
//! 1. input — isi source/dependency aktual (hash konten, bukan mtime/size);
//! 2. configuration — flags/macro/parameter/target;
//! 3. implementation — versi/fingerprint stage;
//! 4. artifact — keberadaan, ukuran, checksum, skema.
//!
//! Modul ini menangani lapisan (1): hash isi file dan verifikasi header
//! include terhadap kondisi disk saat ini.

use crate::identity::{ContentId, NodeIdentity};
use std::path::{Path, PathBuf};

/// Hash 64-bit isi file — sama dengan `compute_checksum` compiler (xxh3-64),
/// agar hash yang tersimpan di metadata file tetap kompatibel antar jalur.
pub fn checksum64(bytes: &[u8]) -> u64 {
    xxhash_rust::xxh3::xxh3_64(bytes)
}

/// Hash isi file di disk. `None` bila file tidak bisa dibaca.
pub fn file_checksum(path: &Path) -> Option<u64> {
    std::fs::read(path).ok().map(|b| checksum64(&b))
}

/// Content ID (BLAKE3) isi file — dipakai sebagai input node.
pub fn file_content_id(path: &Path) -> Option<ContentId> {
    std::fs::read(path).ok().map(|b| ContentId::of(&b))
}

/// Dependency file-level: path + hash konten saat komputasi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDep {
    pub path: PathBuf,
    pub content_hash: u64,
}

impl FileDep {
    pub fn capture(path: impl Into<PathBuf>) -> Option<Self> {
        let path = path.into();
        let content_hash = file_checksum(&path)?;
        Some(FileDep { path, content_hash })
    }

    /// Apakah file masih identik dengan saat capture?
    pub fn is_valid(&self) -> bool {
        file_checksum(&self.path) == Some(self.content_hash)
    }
}

/// Set dependency yang bisa divalidasi ulang.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DependencySet {
    pub files: Vec<FileDep>,
    pub nodes: Vec<(NodeIdentity, ContentId)>,
}

impl DependencySet {
    pub fn new(files: Vec<FileDep>, nodes: Vec<(NodeIdentity, ContentId)>) -> Self {
        DependencySet { files, nodes }
    }

    /// Semua file dependency masih identik.
    pub fn files_valid(&self) -> bool {
        self.files.iter().all(|d| d.is_valid())
    }

    /// Dependency pertama yang berubah (untuk pesan diagnostik).
    pub fn first_stale(&self) -> Option<&FileDep> {
        self.files.iter().find(|d| !d.is_valid())
    }

    /// Fingerprint 64-bit set dependency (untuk config/graph hash).
    pub fn fingerprint(&self) -> u64 {
        let mut h = xxhash_rust::xxh3::Xxh3::new();
        for dep in &self.files {
            h.update(dep.path.to_string_lossy().as_bytes());
            h.update(&dep.content_hash.to_le_bytes());
        }
        for (node, out) in &self.nodes {
            h.update(node.0.as_bytes());
            h.update(out.as_bytes());
        }
        h.digest()
    }
}

/// Validasi daftar `(path, hash)` include terhadap isi disk sekarang.
///
/// File include yang hilang dianggap berubah (preprocessed output lama tidak
/// boleh dipakai: isinya meng-embed header yang sudah tidak ada).
pub fn includes_valid(includes: &[(PathBuf, u64)]) -> bool {
    includes
        .iter()
        .all(|(path, hash)| file_checksum(path) == Some(*hash))
}

/// Bangun daftar hash include dari path.
pub fn capture_includes(paths: &[PathBuf]) -> Vec<(PathBuf, u64)> {
    paths
        .iter()
        .map(|p| (p.clone(), file_checksum(p).unwrap_or(0)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::unique_suffix;

    fn temp_file(tag: &str, content: &[u8]) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "mivon-dep-{}-{}-{}",
            tag,
            std::process::id(),
            unique_suffix()
        ));
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn dependency_detects_same_size_mutation() {
        let p = temp_file("same-size", b"abcd");
        let dep = FileDep::capture(p.clone()).unwrap();
        assert!(dep.is_valid());
        // Ukuran sama, isi berubah.
        std::fs::write(&p, b"abce").unwrap();
        assert!(!dep.is_valid());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn dependency_ignores_mtime() {
        let p = temp_file("mtime", b"same");
        let dep = FileDep::capture(p.clone()).unwrap();
        // Sentuh mtime tanpa mengubah isi → tetap valid.
        let now = std::time::SystemTime::now();
        let _ = filetime_set(&p, now);
        assert!(dep.is_valid());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn missing_include_invalidates() {
        let p = temp_file("inc", b"header");
        let inc = capture_includes(&[p.clone()]);
        assert!(includes_valid(&inc));
        std::fs::remove_file(&p).unwrap();
        assert!(!includes_valid(&inc));
    }

    #[test]
    fn dependency_fingerprint_is_stable() {
        let p = temp_file("fp", b"x");
        let a = DependencySet::new(vec![FileDep::capture(p.clone()).unwrap()], Vec::new());
        let b = DependencySet::new(vec![FileDep::capture(p.clone()).unwrap()], Vec::new());
        assert_eq!(a.fingerprint(), b.fingerprint());
        std::fs::write(&p, b"y").unwrap();
        let c = DependencySet::new(vec![FileDep::capture(p.clone()).unwrap()], Vec::new());
        assert_ne!(a.fingerprint(), c.fingerprint());
        let _ = std::fs::remove_file(&p);
    }

    fn filetime_set(path: &Path, t: std::time::SystemTime) -> std::io::Result<()> {
        // Set mtime via File::set_times (std 1.75+).
        let f = std::fs::OpenOptions::new().write(true).open(path)?;
        f.set_modified(t)
    }
}
