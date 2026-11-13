//! Database artefak precompiled per module (analogue VCS `AN.DB/`, Questa
//! `_info`). Menyimpan hasil analisis module agar tool downstream membaca
//! tanpa compile ulang.
//!
//! Penyimpanan: satu blob biner versioned di `<root>/precompiled/<pid>/`
//! (bukan database/index). Artefak ini turunan node `parse`/`elaborate` dan
//! tidak menentukan cache hit — validitas tetap milik node_cache.

use crate::filesystem::{atomic_write, Layout};
use crate::identity::ContentId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

/// Versi format serialisasi `PrecompiledModule`.
pub const PRECOMPILED_FORMAT_VERSION: u64 = 1;

/// Nama file manifest precompiled.
pub const MANIFEST_FILE: &str = "modules.bin";

/// Info port satu module.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortInfo {
    pub name: String,
    pub dir: String,
    pub width: usize,
    pub is_signed: bool,
}

/// Hasil analisis satu module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrecompiledModule {
    pub name: String,
    pub content_hash: u64,
    pub ast_hash: u64,
    pub type_signature: u64,
    pub source_file: PathBuf,
    pub analyzed_at_ns: u64,
    pub library: String,
    pub ports: Vec<PortInfo>,
    pub depends_on: Vec<String>,
    pub depended_by: Vec<String>,
    pub token_count: u64,
    pub process_count: usize,
    pub signal_count: usize,
    pub error_count: usize,
    pub warn_count: usize,
    #[serde(default)]
    pub ast_bytes: Vec<u8>,
    #[serde(default)]
    pub ir_bytes: Vec<u8>,
    pub checksum: u64,
}

impl PrecompiledModule {
    /// Checksum seluruh field utama (integritas saat load).
    pub fn compute_checksum(&self) -> u64 {
        let mut h = blake3::Hasher::new();
        h.update(self.name.as_bytes());
        h.update(&self.content_hash.to_le_bytes());
        h.update(&self.ast_hash.to_le_bytes());
        h.update(&self.type_signature.to_le_bytes());
        h.update(self.source_file.to_string_lossy().as_bytes());
        for p in &self.ports {
            h.update(p.name.as_bytes());
            h.update(p.dir.as_bytes());
            h.update(&(p.width as u64).to_le_bytes());
            h.update(&[u8::from(p.is_signed)]);
        }
        for d in self.depends_on.iter().chain(self.depended_by.iter()) {
            h.update(d.as_bytes());
        }
        h.update(&self.ast_bytes.len().to_le_bytes());
        h.update(&self.ir_bytes.len().to_le_bytes());
        u64::from_le_bytes(h.finalize().as_bytes()[..8].try_into().unwrap())
    }

    pub fn verify_checksum(&self) -> bool {
        self.compute_checksum() == self.checksum
    }
}

/// Statistik database precompiled.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrecompiledStats {
    pub modules: usize,
    pub clean_modules: usize,
    pub total_tokens: u64,
    pub total_errors: usize,
    pub total_warnings: usize,
    pub total_bytes: u64,
}

/// Database artefak precompiled per project.
#[derive(Debug, Clone, Default)]
pub struct PrecompiledDb {
    pub root: PathBuf,
    pub modules: HashMap<String, PrecompiledModule>,
    pub dirty: bool,
}

impl PrecompiledDb {
    /// Buka database di `root` (dibuat bila belum ada). Blob rusak/versi beda
    /// → kosong (hasil analisis bisa dibangun ulang).
    pub fn open(root: &Path) -> Self {
        let mut db = PrecompiledDb {
            root: root.to_path_buf(),
            modules: HashMap::new(),
            dirty: false,
        };
        let path = root.join(MANIFEST_FILE);
        if let Ok(bytes) = std::fs::read(&path) {
            if let Ok(stored) = bincode::deserialize::<StoredDb>(&bytes) {
                if stored.version == PRECOMPILED_FORMAT_VERSION {
                    db.modules = stored
                        .modules
                        .into_iter()
                        .filter(|m| m.verify_checksum())
                        .map(|m| (m.name.clone(), m))
                        .collect();
                }
            }
        }
        db
    }

    pub fn has_valid(&self, name: &str, content_hash: u64) -> bool {
        self.modules
            .get(name)
            .map(|m| m.content_hash == content_hash && m.verify_checksum())
            .unwrap_or(false)
    }

    pub fn put(&mut self, module: PrecompiledModule) {
        self.modules.insert(module.name.clone(), module);
        self.dirty = true;
    }

    pub fn save(&mut self) -> io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        std::fs::create_dir_all(&self.root)?;
        let stored = StoredDb {
            version: PRECOMPILED_FORMAT_VERSION,
            modules: self.modules.values().cloned().collect(),
        };
        let bytes = bincode::serialize(&stored).map_err(io::Error::other)?;
        atomic_write(&self.root.join(MANIFEST_FILE), &bytes)?;
        self.dirty = false;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.modules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    pub fn stats(&self) -> PrecompiledStats {
        let mut st = PrecompiledStats {
            modules: self.modules.len(),
            ..Default::default()
        };
        for m in self.modules.values() {
            if m.error_count == 0 {
                st.clean_modules += 1;
            }
            st.total_tokens += m.token_count;
            st.total_errors += m.error_count;
            st.total_warnings += m.warn_count;
            st.total_bytes += (m.ast_bytes.len() + m.ir_bytes.len()) as u64;
        }
        st
    }
}

#[derive(Serialize, Deserialize)]
struct StoredDb {
    version: u64,
    modules: Vec<PrecompiledModule>,
}

/// Root direktori precompiled untuk project.
pub fn precompiled_root(layout: &Layout, pid: &str) -> PathBuf {
    layout.root().join("precompiled").join(pid)
}

/// Content hash dari bytes (untuk fingerprint module).
pub fn content_hash(bytes: &[u8]) -> ContentId {
    ContentId::of(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::unique_suffix;

    fn module(name: &str, hash: u64) -> PrecompiledModule {
        let mut m = PrecompiledModule {
            name: name.into(),
            content_hash: hash,
            ast_hash: 1,
            type_signature: 2,
            source_file: PathBuf::from("a.sv"),
            analyzed_at_ns: 0,
            library: "work".into(),
            ports: vec![PortInfo {
                name: "clk".into(),
                dir: "input".into(),
                width: 1,
                is_signed: false,
            }],
            depends_on: vec![],
            depended_by: vec![],
            token_count: 10,
            process_count: 1,
            signal_count: 2,
            error_count: 0,
            warn_count: 0,
            ast_bytes: b"ast".to_vec(),
            ir_bytes: Vec::new(),
            checksum: 0,
        };
        m.checksum = m.compute_checksum();
        m
    }

    #[test]
    fn roundtrip_and_fingerprint() {
        let root = std::env::temp_dir().join(format!(
            "mivon-precompiled-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let mut db = PrecompiledDb::open(&root);
        db.put(module("top", 42));
        db.save().unwrap();

        let db2 = PrecompiledDb::open(&root);
        assert_eq!(db2.len(), 1);
        assert!(db2.has_valid("top", 42));
        assert!(!db2.has_valid("top", 43));
        assert_eq!(db2.stats().clean_modules, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn corrupted_module_is_dropped() {
        let root = std::env::temp_dir().join(format!(
            "mivon-precompiled-bad-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let mut db = PrecompiledDb::open(&root);
        let mut m = module("top", 42);
        m.checksum ^= 1;
        db.put(m);
        db.save().unwrap();
        let db2 = PrecompiledDb::open(&root);
        assert_eq!(db2.len(), 0);
        let _ = std::fs::remove_dir_all(&root);
    }
}
