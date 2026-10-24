//! FEAT-21: ECO Support — Engineering Change Order tracking.
//!
//! Manages RTL modifications during tapeout phases. Tracks what changed,
//! why, who approved it, and impact analysis.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// ECO severity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EcoSeverity {
    Critical, // silicon bug, must fix
    Major,    // functional issue
    Minor,    // timing/cleanup
    Cosmetic, // style only
}

/// ECO status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EcoStatus {
    Draft,
    Submitted,
    Reviewed,
    Approved,
    Implemented,
    Verified,
    Closed,
    Rejected,
}

/// A single Engineering Change Order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EcoEntry {
    pub id: String,
    pub title: String,
    pub description: String,
    pub severity: EcoSeverity,
    pub status: EcoStatus,
    pub author: String,
    pub reviewer: Option<String>,
    pub affected_files: Vec<String>,
    pub affected_modules: Vec<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub closed_at: Option<u64>,
    pub risk_score: f64, // 0.0-1.0
    pub regression_required: bool,
    pub verification_required: bool,
    pub comments: Vec<EcoComment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EcoComment {
    pub author: String,
    pub text: String,
    pub timestamp: u64,
}

/// ECO database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EcoDb {
    pub entries: Vec<EcoEntry>,
    pub next_id: u32,
}

impl EcoDb {
    pub fn new() -> Self {
        EcoDb {
            entries: Vec::new(),
            next_id: 1,
        }
    }

    /// Create a new ECO.
    pub fn create(
        &mut self,
        title: &str,
        description: &str,
        severity: EcoSeverity,
        author: &str,
    ) -> String {
        let id = format!("ECO-{:04}", self.next_id);
        self.next_id += 1;
        let now = now_secs();
        self.entries.push(EcoEntry {
            id: id.clone(),
            title: title.to_string(),
            description: description.to_string(),
            severity,
            status: EcoStatus::Draft,
            author: author.to_string(),
            reviewer: None,
            affected_files: Vec::new(),
            affected_modules: Vec::new(),
            created_at: now,
            updated_at: now,
            closed_at: None,
            risk_score: 0.0,
            regression_required: true,
            verification_required: true,
            comments: Vec::new(),
        });
        id
    }

    /// Transition ECO status.
    pub fn transition(&mut self, id: &str, new_status: EcoStatus) -> Result<(), String> {
        let entry = self
            .entries
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or_else(|| format!("ECO {} not found", id))?;
        let valid = matches!(
            (&entry.status, &new_status),
            (EcoStatus::Draft, EcoStatus::Submitted)
                | (EcoStatus::Submitted, EcoStatus::Reviewed)
                | (EcoStatus::Reviewed, EcoStatus::Approved)
                | (EcoStatus::Reviewed, EcoStatus::Rejected)
                | (EcoStatus::Approved, EcoStatus::Implemented)
                | (EcoStatus::Implemented, EcoStatus::Verified)
                | (EcoStatus::Verified, EcoStatus::Closed)
        );
        if !valid {
            return Err(format!(
                "Invalid transition {:?} -> {:?}",
                entry.status, new_status
            ));
        }
        // Closed dan Rejected sama-sama terminal: keduanya menutup ECO.
        // Tanpa closed_at pada Rejected, laporan berbasis closed_at melihat
        // ECO yang sudah final sebagai "masih terbuka".
        if matches!(new_status, EcoStatus::Closed | EcoStatus::Rejected) {
            entry.closed_at = Some(now_secs());
        }
        entry.status = new_status;
        entry.updated_at = now_secs();
        Ok(())
    }

    /// Add a comment.
    pub fn comment(&mut self, id: &str, author: &str, text: &str) -> Result<(), String> {
        let entry = self
            .entries
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or_else(|| format!("ECO {} not found", id))?;
        entry.comments.push(EcoComment {
            author: author.to_string(),
            text: text.to_string(),
            timestamp: now_secs(),
        });
        entry.updated_at = now_secs();
        Ok(())
    }

    /// List open ECOs.
    pub fn open_ecos(&self) -> Vec<&EcoEntry> {
        self.entries
            .iter()
            .filter(|e| !matches!(e.status, EcoStatus::Closed | EcoStatus::Rejected))
            .collect()
    }

    /// List by severity.
    pub fn by_severity(&self, sev: &EcoSeverity) -> Vec<&EcoEntry> {
        self.entries.iter().filter(|e| &e.severity == sev).collect()
    }

    /// Summary.
    pub fn summary(&self) -> String {
        let total = self.entries.len();
        let open = self.open_ecos().len();
        let critical = self
            .by_severity(&EcoSeverity::Critical)
            .iter()
            .filter(|e| !matches!(e.status, EcoStatus::Closed | EcoStatus::Rejected))
            .count();
        format!(
            "ECO: {} total, {} open, {} critical open",
            total, open, critical
        )
    }

    /// Save to JSON (atomik, membuat direktori induk bila perlu).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("gagal buat direktori {}: {}", parent.display(), e))?;
            }
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        // Temp di direktori yang sama (rename selalu satu filesystem) dan
        // mempertahankan nama penuh file: `eco.json` → `eco.json.tmp`
        // (with_extension akan MENGGANTI ekstensi sehingga dua path berbeda
        // di direktori sama bisa bertabrakan).
        let mut temp_name = path
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_else(|| std::ffi::OsString::from("eco.json"));
        temp_name.push(".tmp");
        let temp = path.with_file_name(temp_name);
        std::fs::write(&temp, json)
            .map_err(|e| format!("gagal tulis {}: {}", temp.display(), e))?;
        std::fs::rename(&temp, path).map_err(|e| format!("gagal rename {}: {}", path.display(), e))
    }

    /// Load from JSON.
    pub fn load(path: &Path) -> Result<Self, String> {
        let json = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        serde_json::from_str(&json).map_err(|e| e.to_string())
    }
}

impl Default for EcoDb {
    fn default() -> Self {
        Self::new()
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_eco() {
        let mut db = EcoDb::new();
        let id = db.create(
            "Fix counter overflow",
            "Counter wraps at max",
            EcoSeverity::Critical,
            "alice",
        );
        assert_eq!(id, "ECO-0001");
        assert_eq!(db.entries.len(), 1);
    }

    #[test]
    fn test_transition_valid() {
        let mut db = EcoDb::new();
        let id = db.create("test", "desc", EcoSeverity::Major, "alice");
        assert!(db.transition(&id, EcoStatus::Submitted).is_ok());
        assert!(db.transition(&id, EcoStatus::Reviewed).is_ok());
        assert!(db.transition(&id, EcoStatus::Approved).is_ok());
        assert_eq!(db.entries[0].status, EcoStatus::Approved);
    }

    #[test]
    fn test_transition_invalid() {
        let mut db = EcoDb::new();
        let id = db.create("test", "desc", EcoSeverity::Major, "alice");
        // Draft -> Implemented is invalid
        assert!(db.transition(&id, EcoStatus::Implemented).is_err());
    }

    #[test]
    fn test_comment() {
        let mut db = EcoDb::new();
        let id = db.create("test", "desc", EcoSeverity::Major, "alice");
        db.comment(&id, "bob", "Looks good").unwrap();
        assert_eq!(db.entries[0].comments.len(), 1);
    }

    #[test]
    fn test_open_ecos() {
        let mut db = EcoDb::new();
        let id1 = db.create("t1", "d1", EcoSeverity::Major, "alice");
        let id2 = db.create("t2", "d2", EcoSeverity::Minor, "bob");
        db.transition(&id1, EcoStatus::Submitted).unwrap();
        db.transition(&id1, EcoStatus::Reviewed).unwrap();
        db.transition(&id1, EcoStatus::Approved).unwrap();
        db.transition(&id1, EcoStatus::Implemented).unwrap();
        db.transition(&id1, EcoStatus::Verified).unwrap();
        db.transition(&id1, EcoStatus::Closed).unwrap();
        assert_eq!(db.open_ecos().len(), 1);
        assert_eq!(db.open_ecos()[0].id, id2);
    }

    #[test]
    fn save_creates_parent_and_roundtrips_next_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/eco.json");
        let mut db = EcoDb::new();
        db.create("t1", "d1", EcoSeverity::Major, "alice");
        db.save(&path).expect("save");
        let mut loaded = EcoDb::load(&path).expect("load");
        assert_eq!(loaded.entries.len(), 1);
        let id = loaded.create("t2", "d2", EcoSeverity::Minor, "bob");
        assert_eq!(id, "ECO-0002", "next_id tersimpan");
    }

    #[test]
    fn closing_sets_closed_at() {
        let mut db = EcoDb::new();
        let id = db.create("t", "d", EcoSeverity::Major, "alice");
        for status in [
            EcoStatus::Submitted,
            EcoStatus::Reviewed,
            EcoStatus::Approved,
            EcoStatus::Implemented,
            EcoStatus::Verified,
            EcoStatus::Closed,
        ] {
            db.transition(&id, status).unwrap();
        }
        assert!(db.entries[0].closed_at.is_some());
    }

    #[test]
    fn rejected_sets_closed_at_and_is_terminal() {
        let mut db = EcoDb::new();
        let id = db.create("t", "d", EcoSeverity::Minor, "alice");
        db.transition(&id, EcoStatus::Submitted).unwrap();
        db.transition(&id, EcoStatus::Reviewed).unwrap();
        db.transition(&id, EcoStatus::Rejected).unwrap();
        assert!(db.entries[0].closed_at.is_some(), "Rejected menutup ECO");
        assert!(db.open_ecos().is_empty());
        assert!(
            db.transition(&id, EcoStatus::Submitted).is_err(),
            "Rejected terminal"
        );
        assert!(db.transition(&id, EcoStatus::Draft).is_err());
    }

    #[test]
    fn closed_is_terminal() {
        let mut db = EcoDb::new();
        let id = db.create("t", "d", EcoSeverity::Critical, "alice");
        for status in [
            EcoStatus::Submitted,
            EcoStatus::Reviewed,
            EcoStatus::Approved,
            EcoStatus::Implemented,
            EcoStatus::Verified,
            EcoStatus::Closed,
        ] {
            db.transition(&id, status).unwrap();
        }
        assert!(db.transition(&id, EcoStatus::Submitted).is_err());
    }

    #[test]
    fn save_twice_overwrites_without_tmp_leftover() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("eco.json");
        let mut db = EcoDb::new();
        db.create("first", "d", EcoSeverity::Minor, "alice");
        db.save(&path).expect("save 1");
        db.create("second", "d", EcoSeverity::Major, "bob");
        db.save(&path).expect("save 2");
        let loaded = EcoDb::load(&path).expect("load");
        assert_eq!(loaded.entries.len(), 2, "isi terbaru menang");
        assert_eq!(loaded.next_id, 3);
        let tmp = dir.path().join("eco.json.tmp");
        assert!(!tmp.exists(), "temp tidak boleh tertinggal");
    }

    #[test]
    fn unknown_eco_transition_is_error() {
        let mut db = EcoDb::new();
        assert!(db.transition("ECO-9999", EcoStatus::Submitted).is_err());
        assert!(db.comment("ECO-9999", "a", "x").is_err());
    }

    #[test]
    fn test_summary() {
        let mut db = EcoDb::new();
        db.create("t1", "d1", EcoSeverity::Critical, "alice");
        db.create("t2", "d2", EcoSeverity::Minor, "bob");
        let s = db.summary();
        assert!(s.contains("2 total"));
        assert!(s.contains("1 critical open"));
    }
}
