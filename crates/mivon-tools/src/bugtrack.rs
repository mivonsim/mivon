//! FEAT-08: Bug tracking integration — tautkan kegagalan regression ke bug ID.
//!
//! Tool EDA komersial menghubungkan hasil regresi ke sistem bug (JIRA/Bugzilla).
//! Modul ini menyediakan database bug lokal yang dapat di-commit bersama
//! project (.mivon/bugs.json) dan diimpor/diekspor sebagai JSON:
//!
//! - tambah bug (id unik, judul, status),
//! - tautkan nama test ke bug (satu test bisa punya beberapa bug),
//! - cari bug terbuka untuk sebuah test (dipakai gate CI: test gagal yang
//!   sudah punya bug terbuka tidak dianggap regresi baru),
//! - ringkasan jumlah open/fixed.
//!
//! 1 file = 1 tanggung jawab: model + persistensi bug tracker.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Status bug.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BugStatus {
    Open,
    Fixed,
    WontFix,
}

impl std::fmt::Display for BugStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BugStatus::Open => write!(f, "Open"),
            BugStatus::Fixed => write!(f, "Fixed"),
            BugStatus::WontFix => write!(f, "WontFix"),
        }
    }
}

/// Satu entri bug.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bug {
    pub id: String,
    pub title: String,
    pub status: BugStatus,
    /// Nama test yang terkait (boleh kosong).
    #[serde(default)]
    pub tests: Vec<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

/// Database bug (disimpan sebagai JSON).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BugTracker {
    #[serde(default)]
    pub bugs: Vec<Bug>,
}

impl BugTracker {
    pub fn new() -> Self {
        BugTracker { bugs: Vec::new() }
    }

    /// Tambah bug baru; id harus unik.
    pub fn add(&mut self, id: &str, title: &str) -> Result<(), String> {
        let id = id.trim();
        let title = title.trim();
        if id.is_empty() {
            return Err("bug id tidak boleh kosong".to_string());
        }
        if title.is_empty() {
            return Err("judul bug tidak boleh kosong".to_string());
        }
        if self.bugs.iter().any(|b| b.id == id) {
            return Err(format!("bug \"{}\" sudah ada", id));
        }
        self.bugs.push(Bug {
            id: id.to_string(),
            title: title.to_string(),
            status: BugStatus::Open,
            tests: Vec::new(),
            notes: None,
        });
        Ok(())
    }

    /// Tautkan nama test ke bug (idempoten).
    pub fn link(&mut self, id: &str, test: &str) -> Result<(), String> {
        let test = test.trim();
        if test.is_empty() {
            return Err("nama test tidak boleh kosong".to_string());
        }
        let bug = self
            .bugs
            .iter_mut()
            .find(|b| b.id == id.trim())
            .ok_or_else(|| format!("bug \"{}\" tidak ditemukan", id))?;
        if !bug.tests.iter().any(|t| t == test) {
            bug.tests.push(test.to_string());
        }
        Ok(())
    }

    /// Ubah status bug.
    pub fn set_status(&mut self, id: &str, status: BugStatus) -> Result<(), String> {
        let bug = self
            .bugs
            .iter_mut()
            .find(|b| b.id == id)
            .ok_or_else(|| format!("bug \"{}\" tidak ditemukan", id))?;
        bug.status = status;
        Ok(())
    }

    /// Bug (semua status) yang tertaut ke test tertentu.
    pub fn bugs_for_test(&self, test: &str) -> Vec<&Bug> {
        self.bugs
            .iter()
            .filter(|b| b.tests.iter().any(|t| t == test))
            .collect()
    }

    /// True bila test punya bug berstatus Open (kegagalan sudah diketahui).
    pub fn has_open_bug(&self, test: &str) -> bool {
        self.bugs
            .iter()
            .any(|b| b.status == BugStatus::Open && b.tests.iter().any(|t| t == test))
    }

    pub fn open_count(&self) -> usize {
        self.bugs.iter().filter(|b| b.status == BugStatus::Open).count()
    }

    /// Exit code gate CI: 0 = test punya bug Open (kegagalan diketahui),
    /// 1 = tidak ada bug Open (regresi baru / belum dilaporkan).
    pub fn check_exit_code(&self, test: &str) -> i32 {
        if self.has_open_bug(test) {
            0
        } else {
            1
        }
    }

    /// Export CSV siap impor ke Jira.
    pub fn export_jira_csv(&self) -> String {
        let mut csv = String::from("Summary,Description,Priority,Labels\n");
        for bug in &self.bugs {
            let summary = format!("[{}] {}", bug.id, bug.title);
            let description = format!(
                "Status: {}\nTests: {}\nNotes: {}",
                bug.status,
                bug.tests.join(", "),
                bug.notes.as_deref().unwrap_or("-")
            );
            csv.push_str(&format!(
                "\"{}\",\"{}\",\"Medium\",\"mivon-bug\"\n",
                escape_csv(&summary),
                escape_csv(&description)
            ));
        }
        csv
    }

    /// Export CSV siap impor ke Redmine.
    pub fn export_redmine_csv(&self) -> String {
        let mut csv = String::from("Subject,Description,Priority,Tracker,Status\n");
        for bug in &self.bugs {
            let subject = format!("[mivon] {} {}", bug.id, bug.title);
            let description = format!(
                "**Status:** {}\n**Tests:** {}",
                bug.status,
                bug.tests.join(", ")
            );
            csv.push_str(&format!(
                "\"{}\",\"{}\",\"Normal\",\"Defect\",\"New\"\n",
                escape_csv(&subject),
                escape_csv(&description)
            ));
        }
        csv
    }

    /// Ringkasan satu baris.
    pub fn summary(&self) -> String {
        let open = self.open_count();
        let fixed = self
            .bugs
            .iter()
            .filter(|b| b.status == BugStatus::Fixed)
            .count();
        let wontfix = self
            .bugs
            .iter()
            .filter(|b| b.status == BugStatus::WontFix)
            .count();
        format!(
            "Bugs: {} total, {} open, {} fixed, {} wontfix",
            self.bugs.len(),
            open,
            fixed,
            wontfix
        )
    }

    /// Simpan JSON secara atomik (temp + rename), membuat direktori induk.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("gagal buat direktori {}: {}", parent.display(), e))?;
            }
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, json)
            .map_err(|e| format!("gagal tulis {}: {}", temp.display(), e))?;
        std::fs::rename(&temp, path)
            .map_err(|e| format!("gagal rename {}: {}", path.display(), e))
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let json = std::fs::read_to_string(path)
            .map_err(|e| format!("gagal baca {}: {}", path.display(), e))?;
        serde_json::from_str(&json)
            .map_err(|e| format!("JSON bug tracker tidak valid di {}: {}", path.display(), e))
    }
}

/// Escape satu field CSV (kutip ganda, newline jadi spasi).
fn escape_csv(value: &str) -> String {
    value
        .replace(['\n', '\r'], " ")
        .replace('"', "\"\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_link_and_lookup() {
        let mut tracker = BugTracker::new();
        tracker.add("BUG-1", "counter overflow").unwrap();
        tracker.link("BUG-1", "test_counter").unwrap();
        tracker.link("BUG-1", "test_counter").unwrap();
        let bugs = tracker.bugs_for_test("test_counter");
        assert_eq!(bugs.len(), 1);
        assert_eq!(bugs[0].tests.len(), 1, "link idempoten");
        assert!(tracker.has_open_bug("test_counter"));
        assert!(!tracker.has_open_bug("test_alu"));
    }

    #[test]
    fn duplicate_id_rejected() {
        let mut tracker = BugTracker::new();
        tracker.add("BUG-1", "a").unwrap();
        assert!(tracker.add("BUG-1", "b").is_err());
        assert!(tracker.add("  ", "c").is_err());
    }

    #[test]
    fn closing_bug_clears_open_gate() {
        let mut tracker = BugTracker::new();
        tracker.add("BUG-1", "a").unwrap();
        tracker.link("BUG-1", "t1").unwrap();
        tracker.set_status("BUG-1", BugStatus::Fixed).unwrap();
        assert!(!tracker.has_open_bug("t1"));
        assert_eq!(tracker.open_count(), 0);
        assert!(tracker.summary().contains("1 fixed"));
    }

    #[test]
    fn save_load_roundtrip_nested_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/bugs.json");
        let mut tracker = BugTracker::new();
        tracker.add("BUG-7", "fsm stuck").unwrap();
        tracker.link("BUG-7", "test_fsm").unwrap();
        tracker.save(&path).expect("save");
        let loaded = BugTracker::load(&path).expect("load");
        assert_eq!(loaded.bugs.len(), 1);
        assert!(loaded.has_open_bug("test_fsm"));
        assert!(loaded.summary().contains("1 open"));
    }

    #[test]
    fn check_exit_code_reflects_open_bug() {
        let mut tracker = BugTracker::new();
        tracker.add("BUG-1", "a").unwrap();
        tracker.link("BUG-1", "t1").unwrap();
        assert_eq!(tracker.check_exit_code("t1"), 0, "bug open = gate lolos");
        assert_eq!(tracker.check_exit_code("t2"), 1, "tanpa bug = gate gagal");
        tracker.set_status("BUG-1", BugStatus::Fixed).unwrap();
        assert_eq!(tracker.check_exit_code("t1"), 1);
    }

    #[test]
    fn export_csv_contains_bug_and_escapes() {
        let mut tracker = BugTracker::new();
        tracker.add("BUG-1", "title \"quoted\"").unwrap();
        tracker.link("BUG-1", "t1").unwrap();
        let jira = tracker.export_jira_csv();
        assert!(jira.starts_with("Summary,Description,Priority,Labels"));
        assert!(jira.contains("BUG-1"));
        assert!(jira.contains("\"\"quoted\"\""), "kutip di-escape: {jira}");
        let redmine = tracker.export_redmine_csv();
        assert!(redmine.contains("Subject,Description,Priority,Tracker,Status"));
        assert!(redmine.contains("BUG-1"));
    }

    #[test]
    fn ids_and_tests_are_trimmed_and_validated() {
        let mut tracker = BugTracker::new();
        tracker.add(" BUG-1 ", " title ").unwrap();
        assert_eq!(tracker.bugs[0].id, "BUG-1");
        assert_eq!(tracker.bugs[0].title, "title");
        tracker.link("BUG-1", " t1 ").unwrap();
        assert!(tracker.has_open_bug("t1"));
        assert!(tracker.link("BUG-1", "   ").is_err());
        assert!(tracker.add("BUG-2", "   ").is_err());
    }

    #[test]
    fn linking_unknown_bug_is_error() {
        let mut tracker = BugTracker::new();
        assert!(tracker.link("NOPE", "t1").is_err());
        assert!(tracker.set_status("NOPE", BugStatus::Fixed).is_err());
    }
}
