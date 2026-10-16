//! ledger — catatan seed yang SUDAH dievaluasi kampanye (persisten).
//!
//! Tujuan: mendukung target "semua seed dijalankan tanpa ada yang di-skip".
//! `build_seed_plan` lama hanya menjamah SEBAGIAN kandidat bila `cases <
//! kandidat` (stride) dan tak punya memori antar-kampanye — seed di paruh
//! yang sama terus terpilih, sisanya tak pernah. Ledger menyimpan kunci
//! seed yang sudah dievaluasi; kampanye berikutnya mengutamakan yang belum.
//!
//! Kunci ledger = `path|size|mtime` (murah, metadata saja — tanpa baca isi).
//! File seed yang berubah (size/mtime) otomatis dianggap baru → dievaluasi
//! ulang, jadi ledger tidak menyembunyikan perubahan konten.
//!
//! Format file: satu kunci per baris (teks polos), tulis atomik temp+rename.

#![cfg(feature = "dev")]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Jalur state default: `.mivon-fuzz-state/seen-seeds.txt` (di-gitignore).
pub fn default_state_path() -> PathBuf {
    PathBuf::from(".mivon-fuzz-state/seen-seeds.txt")
}

/// Kunci stabil untuk satu seed: path + ukuran + mtime (ns).
///
/// Metadata gagal dibaca (file hilang) → fallback path saja (tetap stabil).
pub fn seed_key(path: &Path) -> String {
    match std::fs::metadata(path) {
        Ok(m) => {
            let mtime = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            format!("{}|{}|{}", path.display(), m.len(), mtime)
        }
        Err(_) => path.display().to_string(),
    }
}

/// Ledger seed ter-evaluasi (in-memory + persist ke file).
pub struct SeedLedger {
    path: PathBuf,
    seen: BTreeSet<String>,
    /// Jumlah kunci baru sejak load — utk laporan ringkas.
    new_marks: usize,
}

impl SeedLedger {
    /// Muat dari jalur default.
    pub fn load() -> Self {
        Self::load_from(default_state_path())
    }

    /// Muat dari jalur eksplisit (dipakai test).
    pub fn load_from(path: PathBuf) -> Self {
        let seen = std::fs::read_to_string(&path)
            .map(|s| {
                s.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        Self {
            path,
            seen,
            new_marks: 0,
        }
    }

    /// Apakah kunci sudah pernah dievaluasi.
    pub fn is_seen(&self, key: &str) -> bool {
        self.seen.contains(key)
    }

    /// Tandai satu seed (idempoten — kunci dobel tak menambah baris).
    pub fn mark(&mut self, key: &str) {
        if self.seen.insert(key.to_string()) {
            self.new_marks += 1;
        }
    }

    /// Jumlah kunci tersimpan.
    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }

    /// Kunci baru sejak load (diagnostik kampanye).
    pub fn new_marks(&self) -> usize {
        self.new_marks
    }

    /// Simpan seluruh set (tulis atomik ala MICD: temp + rename).
    pub fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let mut body = String::with_capacity(self.seen.len() * 48);
        for k in &self.seen {
            body.push_str(k);
            body.push('\n');
        }
        let tmp = self.path.with_extension("txt.tmp");
        std::fs::write(&tmp, body.as_bytes())?;
        std::fs::rename(&tmp, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_ledger(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fzledger_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("seen-seeds.txt")
    }

    /// mark → save → load kembali: kunci bertahan, dobel tak menggandakan.
    #[test]
    fn ledger_roundtrip_and_dedup() {
        let path = tmp_ledger("roundtrip");
        let mut l = SeedLedger::load_from(path.clone());
        assert!(l.is_empty());
        l.mark("a.sv|10|111");
        l.mark("b.sv|20|222");
        l.mark("a.sv|10|111"); // dobel
        assert_eq!(l.len(), 2, "dobel tidak menambah");
        assert_eq!(l.new_marks(), 2);
        l.save().unwrap();

        let l2 = SeedLedger::load_from(path);
        assert_eq!(l2.len(), 2);
        assert!(l2.is_seen("a.sv|10|111"));
        assert!(l2.is_seen("b.sv|20|222"));
        assert!(!l2.is_seen("c.sv|1|1"));
        assert_eq!(l2.new_marks(), 0, "load ulang bukan mark baru");
    }

    /// File state hilang / korup → ledger kosong, bukan panic.
    #[test]
    fn ledger_missing_or_corrupt_is_empty() {
        let l = SeedLedger::load_from(PathBuf::from("/nonexistent-fz/seen-seeds.txt"));
        assert!(l.is_empty());
        assert!(!l.is_seen("x"));
    }

    /// Kunci berubah bila file seed berubah (size/mtime) — seed diedit
    /// wajib dievaluasi ulang, ledger tak boleh menyembunyikannya.
    #[test]
    fn seed_key_changes_when_file_changes() {
        let dir = std::env::temp_dir().join(format!("fzledger_key_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("s.sv");
        std::fs::write(&f, "module a; endmodule").unwrap();
        let k1 = seed_key(&f);
        std::fs::write(&f, "module a; endmodule // bigger change").unwrap();
        let k2 = seed_key(&f);
        assert_ne!(k1, k2, "ukuran berubah → kunci berubah");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
