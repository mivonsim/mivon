//! Model data regression manager (mirror `mivon-tools::regression` untuk
//! bacaan database `.mivon/regression.json`). Logika analisis regression
//! tetap tinggal di CLI `mregression`; GUI hanya menampilkan dan menyimpan
//! database JSON agar GUI tidak terpaksa meng-depend crate mivon-tools yang
//! berat (banyak modul web/rest/etc. yang tidak relevan TUI).
//!
//! Format JSON kompatibel dengan `mivon-tools::regression::RegressionDb`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Satu hasil tes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub name: String,
    pub status: TestStatus,
    pub duration_ms: u64,
    pub error_msg: Option<String>,
}

/// Status satu tes dalam satu run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TestStatus {
    Pass,
    Fail,
    Skip,
    Flaky, // lulus setelah retry
}

/// Satu run regression (satu eksekusi seluruh suite).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegressionRun {
    pub id: String,
    pub timestamp: u64,
    pub branch: String,
    pub commit: String,
    pub results: Vec<TestResult>,
    pub total_duration_ms: u64,
}

impl RegressionRun {
    pub fn pass_count(&self) -> usize {
        self.results
            .iter()
            .filter(|r| r.status == TestStatus::Pass)
            .count()
    }
    pub fn fail_count(&self) -> usize {
        self.results
            .iter()
            .filter(|r| r.status == TestStatus::Fail)
            .count()
    }
    /// Pass rate: Pass + Flaky dianggap lulus; Skip tidak masuk denominator.
    pub fn pass_rate(&self) -> f64 {
        let counted = self
            .results
            .iter()
            .filter(|r| r.status != TestStatus::Skip)
            .count();
        if counted == 0 {
            return 0.0;
        }
        let passed = self
            .results
            .iter()
            .filter(|r| matches!(r.status, TestStatus::Pass | TestStatus::Flaky))
            .count();
        passed as f64 / counted as f64 * 100.0
    }
}

/// Database regression — simpan run seiring waktu.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegressionDb {
    pub runs: Vec<RegressionRun>,
    pub flaky_history: HashMap<String, Vec<bool>>, // test_name -> [pass, fail, ...]
}

impl RegressionDb {
    pub fn new() -> Self {
        RegressionDb {
            runs: Vec::new(),
            flaky_history: HashMap::new(),
        }
    }

    /// Catat run baru (update flaky history, cap 50/run).
    pub fn record(&mut self, run: RegressionRun) {
        for result in &run.results {
            let history = self
                .flaky_history
                .entry(result.name.clone())
                .or_default();
            history.push(matches!(
                result.status,
                TestStatus::Pass | TestStatus::Flaky
            ));
            if history.len() > 50 {
                history.remove(0);
            }
        }
        self.runs.push(run);
        // Cap total run agar database tak tumbuh tanpa batas.
        if self.runs.len() > 200 {
            self.runs.drain(..self.runs.len() - 200);
        }
    }

    /// Run terbaru (None jika belum ada run).
    pub fn latest(&self) -> Option<&RegressionRun> {
        self.runs.last()
    }

    /// Jumlah run tersimpan.
    pub fn run_count(&self) -> usize {
        self.runs.len()
    }

    /// Tes yang flaky (pass rate antara 0.2–0.8 dan ≥3 riwayat).
    pub fn flaky_tests(&self) -> Vec<(String, f64)> {
        let mut out: Vec<(String, f64)> = Vec::new();
        for (name, history) in &self.flaky_history {
            if history.len() < 3 {
                continue;
            }
            let pass = history.iter().filter(|&&b| b).count() as f64;
            let rate = pass / history.len() as f64 * 100.0;
            if rate > 20.0 && rate < 80.0 {
                out.push((name.clone(), rate));
            }
        }
        out.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        out
    }

    /// Ringkasan teks ringkas untuk latest run.
    pub fn summary(&self) -> String {
        match self.latest() {
            Some(r) => format!(
                "Run {} — pass {} / fail {} / skip {} / flaky {} · {:.1}%",
                r.id,
                r.pass_count(),
                r.fail_count(),
                r.results.iter().filter(|r| r.status == TestStatus::Skip).count(),
                r.results.iter().filter(|r| r.status == TestStatus::Flaky).count(),
                r.pass_rate()
            ),
            None => "Belum ada run regression".to_string(),
        }
    }

    /// Simpan ke file JSON.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }

    /// Muat dari file JSON.
    pub fn load(path: &Path) -> Result<Self, String> {
        let json = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        serde_json::from_str(&json).map_err(|e| e.to_string())
    }
}

impl Default for RegressionDb {
    fn default() -> Self {
        Self::new()
    }
}

/// Path default database regression: `<project>/.mivon/regression.json`.
pub fn regression_path(project_root: &Path) -> PathBuf {
    project_root.join(".mivon").join("regression.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_run() -> RegressionRun {
        RegressionRun {
            id: "run-1".to_string(),
            timestamp: 1000,
            branch: "main".to_string(),
            commit: "abc".to_string(),
            results: vec![
                TestResult {
                    name: "t1".to_string(),
                    status: TestStatus::Pass,
                    duration_ms: 10,
                    error_msg: None,
                },
                TestResult {
                    name: "t2".to_string(),
                    status: TestStatus::Fail,
                    duration_ms: 20,
                    error_msg: Some("boom".to_string()),
                },
                TestResult {
                    name: "t3".to_string(),
                    status: TestStatus::Skip,
                    duration_ms: 0,
                    error_msg: None,
                },
            ],
            total_duration_ms: 30,
        }
    }

    #[test]
    fn pass_rate_excludes_skip() {
        let r = make_run();
        assert!((r.pass_rate() - 50.0).abs() < f64::EPSILON); // 1 pass / 2 counted
    }

    #[test]
    fn save_load_roundtrip() {
        let mut db = RegressionDb::new();
        db.record(make_run());
        let dir = std::env::temp_dir().join(format!("mivon_reg_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("regression.json");
        db.save(&p).unwrap();
        let loaded = RegressionDb::load(&p).unwrap();
        assert_eq!(loaded.run_count(), 1);
        assert_eq!(loaded.latest().unwrap().fail_count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn flaky_detects_borderline() {
        let mut db = RegressionDb::new();
        // t1: 2 pass 2 fail → flaky
        for _ in 0..2 {
            db.record(RegressionRun {
                id: "x".to_string(),
                timestamp: 0,
                branch: "b".to_string(),
                commit: "c".to_string(),
                results: vec![TestResult {
                    name: "t1".to_string(),
                    status: TestStatus::Pass,
                    duration_ms: 1,
                    error_msg: None,
                }],
                total_duration_ms: 1,
            });
            db.record(RegressionRun {
                id: "y".to_string(),
                timestamp: 0,
                branch: "b".to_string(),
                commit: "c".to_string(),
                results: vec![TestResult {
                    name: "t1".to_string(),
                    status: TestStatus::Fail,
                    duration_ms: 1,
                    error_msg: None,
                }],
                total_duration_ms: 1,
            });
        }
        let flaky = db.flaky_tests();
        assert_eq!(flaky.len(), 1);
        assert_eq!(flaky[0].0, "t1");
    }
}
