//! CI/CD Integration — panel status CI/CD bawaan GUI.
//!
//! Membaca status CI dari file JSON lokal (`ci-status.json` di root project,
//! atau path yang dikonfigurasi). File ini diharapkan dihasilkan oleh CI
//! eksternal (GitHub Actions, GitLab CI, dll.) dan disalin ke working
//! directory project. GUI hanya menampilkan status; tidak berkomunikasi langsung
//! dengan CI server (untuk menghindari dependency HTTP dan secret management
//! di bawaan GUI).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Status satu job CI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum CiJobStatus {
    Pending,
    Running,
    Success,
    Failure,
    Skipped,
    Canceled,
}

/// Satu job CI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CiJob {
    pub name: String,
    pub status: CiJobStatus,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub duration_ms: Option<u64>,
    pub url: Option<String>,
    pub error: Option<String>,
}

/// Satu run CI (satu pipeline/workflow run).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CiRun {
    pub id: String,
    pub workflow: String,
    pub branch: String,
    pub commit: String,
    pub status: CiJobStatus,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub jobs: Vec<CiJob>,
}

/// Database CI — simpan run terakhir.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CiDatabase {
    pub runs: Vec<CiRun>,
}

impl CiDatabase {
    pub fn new() -> Self {
        Self::default()
    }

    /// Muat dari file JSON.
    pub fn load(path: &Path) -> Result<Self, String> {
        let json = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        serde_json::from_str(&json).map_err(|e| e.to_string())
    }

    /// Simpan ke file JSON.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }

    /// Run terbaru.
    pub fn latest(&self) -> Option<&CiRun> {
        self.runs.last()
    }

    /// Jumlah run.
    pub fn run_count(&self) -> usize {
        self.runs.len()
    }

    /// Hitung jumlah job per status.
    pub fn status_counts(&self) -> HashMap<CiJobStatus, usize> {
        let mut counts: HashMap<CiJobStatus, usize> = HashMap::new();
        for run in &self.runs {
            for job in &run.jobs {
                *counts.entry(job.status.clone()).or_insert(0) += 1;
            }
        }
        counts
    }

    /// Daftar job yang gagal dari run terbaru.
    pub fn failed_jobs(&self) -> Vec<&CiJob> {
        self.latest()
            .map(|r| {
                r.jobs
                    .iter()
                    .filter(|j| j.status == CiJobStatus::Failure)
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Path default database CI: `<project>/.mivon/ci-status.json`.
pub fn ci_path(project_root: &Path) -> PathBuf {
    project_root.join(".mivon").join("ci-status.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::state::GuiState;
    use std::sync::mpsc::channel;

    fn test_state() -> GuiState {
        let (tx, rx) = channel();
        GuiState::new(tx, rx)
    }

    #[test]
    fn ci_database_save_load_roundtrip() {
        let mut db = CiDatabase::new();
        db.runs.push(CiRun {
            id: "run-1".to_string(),
            workflow: "test".to_string(),
            branch: "main".to_string(),
            commit: "abc".to_string(),
            status: CiJobStatus::Success,
            started_at: None,
            completed_at: None,
            jobs: vec![],
        });
        let dir = std::env::temp_dir().join(format!("mivon_ci_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("ci-status.json");
        db.save(&p).unwrap();
        let loaded = CiDatabase::load(&p).unwrap();
        assert_eq!(loaded.run_count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ci_status_counts() {
        let mut db = CiDatabase::new();
        db.runs.push(CiRun {
            id: "run-1".to_string(),
            workflow: "test".to_string(),
            branch: "main".to_string(),
            commit: "abc".to_string(),
            status: CiJobStatus::Success,
            started_at: None,
            completed_at: None,
            jobs: vec![
                CiJob {
                    name: "build".to_string(),
                    status: CiJobStatus::Success,
                    started_at: None,
                    completed_at: None,
                    duration_ms: None,
                    url: None,
                    error: None,
                },
                CiJob {
                    name: "test".to_string(),
                    status: CiJobStatus::Failure,
                    started_at: None,
                    completed_at: None,
                    duration_ms: None,
                    url: None,
                    error: None,
                },
            ],
        });
        let counts = db.status_counts();
        assert_eq!(counts.get(&CiJobStatus::Success), Some(&1));
        assert_eq!(counts.get(&CiJobStatus::Failure), Some(&1));
        assert_eq!(db.failed_jobs().len(), 1);
        assert_eq!(db.failed_jobs()[0].name, "test");
    }

    #[test]
    fn ci_path_default() {
        let p = ci_path(Path::new("/proj"));
        assert_eq!(p, Path::new("/proj/.mivon/ci-status.json"));
    }

    #[test]
    fn ci_register_in_state() {
        let mut s = test_state();
        assert!(s.ci_database.is_none());
        s.ci_database = Some(CiDatabase::new());
        assert!(s.ci_database.is_some());
    }
}
