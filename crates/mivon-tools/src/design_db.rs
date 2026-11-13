//! Design database foundation — stage 1 of FEAT-01
//! (Design database server — persistent compile cache across projects and
//! users — Critical, open).
//!
//! Menyimpan metadata kompile per desain (checksum, top module, daftar
//! sinyal, waktu kompile) sehingga kompilasi berikutnya bisa digunakan
//! kembali (incremental) tanpa mengulang lex+parse+elab dari nol.
//! Belum ada server/database nyata — stage 1 adalah model data + tes yang
//! mendokumentasikan gap. Belum di-wire ke MICD/CompileSession.

use mivon_core::intern::Symbol;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Metadata satu desain yang telah di-kompilasi.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesignEntry {
    pub top: Symbol,
    pub source_files: Vec<String>,
    pub signal_count: usize,
    pub process_count: usize,
    pub checksum: String, // content checksum desain (content-addressed)
    pub compiled_at: u64, // UNIX epoch seconds
}

/// Database desain dalam memori (stage 1). Sebuah implementasi nyata akan
/// menyimpan ini ke disk (content-addressed, seperti MICD) dan menyediakan
/// antarmuka server untuk multi-proyek/multi-user.
#[derive(Debug, Clone, Default)]
pub struct DesignDatabase {
    designs: HashMap<String, DesignEntry>, // key: top module name (string)
}

impl DesignDatabase {
    pub fn new() -> Self {
        Self {
            designs: HashMap::new(),
        }
    }

    /// Catat/memperbarui metadata kompile sebuah desain.
    pub fn record(&mut self, entry: DesignEntry) {
        self.designs.insert(entry.top.as_str().to_string(), entry);
    }

    /// Ambil metadata desain menurut nama top module.
    pub fn get(&self, top: &Symbol) -> Option<&DesignEntry> {
        self.designs.get(top.as_str())
    }

    /// Daftar semua top module yang telah di-kompilasi.
    pub fn tops(&self) -> Vec<Symbol> {
        self.designs.keys().map(|k| Symbol::intern(k)).collect()
    }

    pub fn len(&self) -> usize {
        self.designs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.designs.is_empty()
    }

    /// Waktu sekarang dalam UNIX epoch seconds (untuk compiled_at).
    fn now_seconds() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

impl DesignEntry {
    pub fn new(
        top: Symbol,
        source_files: Vec<String>,
        signal_count: usize,
        process_count: usize,
        checksum: String,
    ) -> Self {
        Self {
            top,
            source_files,
            signal_count,
            process_count,
            checksum,
            compiled_at: DesignDatabase::now_seconds(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_get_design() {
        let mut db = DesignDatabase::new();
        let top = Symbol::intern("top");
        let entry = DesignEntry::new(
            top,
            vec!["top.sv".into(), "sub.sv".into()],
            42,
            7,
            "abc123".into(),
        );
        db.record(entry.clone());
        let got = db.get(&top).unwrap();
        assert_eq!(got.signal_count, 42);
        assert_eq!(got.process_count, 7);
        assert_eq!(got.checksum, "abc123");
        assert_eq!(
            got.source_files,
            vec!["top.sv".to_string(), "sub.sv".to_string()]
        );
        assert!(got.compiled_at > 0);
    }

    #[test]
    fn missing_top_returns_none() {
        let db = DesignDatabase::new();
        assert!(db.get(&Symbol::intern("nope")).is_none());
    }

    #[test]
    fn list_tops() {
        let mut db = DesignDatabase::new();
        db.record(DesignEntry::new(
            Symbol::intern("a"),
            vec![],
            0,
            0,
            "x".into(),
        ));
        db.record(DesignEntry::new(
            Symbol::intern("b"),
            vec![],
            0,
            0,
            "y".into(),
        ));
        let mut tops: Vec<Symbol> = db.tops();
        tops.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        assert_eq!(
            tops,
            vec![Symbol::intern("a"), Symbol::intern("b")]
        );
    }

    #[test]
    fn empty_database() {
        let db = DesignDatabase::new();
        assert!(db.is_empty());
        assert_eq!(db.len(), 0);
    }
}
