//! FEAT-07: Regression Management — pass/fail analytics for test suites.
//!
//! Tracks regression test results over time, detects flaky tests,
//! computes pass rates, and generates trend reports.

use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// A single test result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub name: String,
    pub status: TestStatus,
    pub duration_ms: u64,
    pub error_msg: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TestStatus {
    Pass,
    Fail,
    Skip,
    Flaky, // passed after retry
}

/// A regression run (one execution of the full suite).
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
            0.0
        } else {
            let passed = self
                .results
                .iter()
                .filter(|r| matches!(r.status, TestStatus::Pass | TestStatus::Flaky))
                .count();
            passed as f64 / counted as f64 * 100.0
        }
    }
}

/// Regression database — stores runs over time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegressionDb {
    pub runs: Vec<RegressionRun>,
    pub flaky_history: HashMap<String, Vec<bool>>, // test_name -> [pass, fail, pass, ...]
}

impl RegressionDb {
    pub fn new() -> Self {
        RegressionDb {
            runs: Vec::new(),
            flaky_history: HashMap::new(),
        }
    }

    /// Record a new run.
    pub fn record(&mut self, run: RegressionRun) {
        // Update flaky history
        for result in &run.results {
            let history = self.flaky_history.entry(result.name.clone()).or_default();
            history.push(matches!(
                result.status,
                TestStatus::Pass | TestStatus::Flaky
            ));
            // Keep last 50 results
            if history.len() > 50 {
                history.remove(0);
            }
        }
        self.runs.push(run);
        // Keep last 100 runs
        if self.runs.len() > 100 {
            self.runs.remove(0);
        }
    }

    /// Detect flaky tests (passed and failed in recent history).
    pub fn flaky_tests(&self) -> Vec<(String, f64)> {
        let mut flaky = Vec::new();
        for (name, history) in &self.flaky_history {
            if history.len() < 3 {
                continue;
            }
            let recent = &history[history.len().saturating_sub(10)..];
            let has_pass = recent.iter().any(|&p| p);
            let has_fail = recent.iter().any(|&p| !p);
            if has_pass && has_fail {
                let rate = recent.iter().filter(|&&p| p).count() as f64 / recent.len() as f64;
                flaky.push((name.clone(), rate));
            }
        }
        flaky.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        flaky
    }

    /// Compute trend: is the suite improving or degrading?
    ///
    /// Membandingkan dua jendela DISJOINT berukuran sama (maks 5 run):
    /// `older` = jendela sebelumnya, `recent` = jendela terakhir.
    pub fn trend(&self) -> RegressionTrend {
        let len = self.runs.len();
        if len < 2 {
            return RegressionTrend::Stable;
        }
        let window = (len / 2).min(5);
        let recent = &self.runs[len - window..];
        let older = &self.runs[len - 2 * window..len - window];
        let recent_rate: f64 =
            recent.iter().map(|r| r.pass_rate()).sum::<f64>() / recent.len() as f64;
        let older_rate: f64 = older.iter().map(|r| r.pass_rate()).sum::<f64>() / older.len() as f64;
        if recent_rate > older_rate + 1.0 {
            RegressionTrend::Improving
        } else if recent_rate < older_rate - 1.0 {
            RegressionTrend::Degrading
        } else {
            RegressionTrend::Stable
        }
    }

    /// Summary report.
    pub fn summary(&self) -> String {
        let total_runs = self.runs.len();
        if total_runs == 0 {
            return "Regression: no runs recorded".to_string();
        }
        let latest_rate = self.runs.last().map(|r| r.pass_rate()).unwrap_or(0.0);
        let flaky = self.flaky_tests().len();
        let trend = self.trend();
        format!(
            "Regression: {} runs, latest {:.1}% pass, {} flaky tests, trend: {:?}",
            total_runs, latest_rate, flaky, trend,
        )
    }

    /// Save to file (membuat direktori induk bila belum ada).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("gagal buat direktori {}: {}", parent.display(), e))?;
            }
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }

    /// Load from file.
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegressionTrend {
    Improving,
    Stable,
    Degrading,
}

#[allow(dead_code)] // util waktu — dipakai fitur report selanjutnya
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[allow(dead_code)] // util waktu — dipakai fitur report selanjutnya
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_run(results: Vec<(&str, bool)>) -> RegressionRun {
        RegressionRun {
            id: "run-001".into(),
            timestamp: now_secs(),
            branch: "main".into(),
            commit: "abc123".into(),
            results: results
                .into_iter()
                .map(|(name, pass)| TestResult {
                    name: name.into(),
                    status: if pass {
                        TestStatus::Pass
                    } else {
                        TestStatus::Fail
                    },
                    duration_ms: 100,
                    error_msg: None,
                })
                .collect(),
            total_duration_ms: 1000,
        }
    }

    #[test]
    fn test_pass_rate() {
        let run = make_run(vec![("t1", true), ("t2", true), ("t3", false)]);
        assert_eq!(run.pass_count(), 2);
        assert_eq!(run.fail_count(), 1);
        assert!((run.pass_rate() - 66.67).abs() < 0.1);
    }

    #[test]
    fn test_record_and_summary() {
        let mut db = RegressionDb::new();
        db.record(make_run(vec![("t1", true), ("t2", true)]));
        db.record(make_run(vec![("t1", true), ("t2", false)]));
        let s = db.summary();
        assert!(s.contains("2 runs"));
        assert!(s.contains("flaky"));
    }

    #[test]
    fn test_flaky_detection() {
        let mut db = RegressionDb::new();
        for i in 0..5 {
            db.record(make_run(vec![("flaky_test", i % 2 == 0)]));
        }
        let flaky = db.flaky_tests();
        assert!(flaky.iter().any(|(n, _)| n == "flaky_test"));
    }

    #[test]
    fn test_trend_stable() {
        let mut db = RegressionDb::new();
        for _ in 0..10 {
            db.record(make_run(vec![("t1", true), ("t2", true)]));
        }
        assert_eq!(db.trend(), RegressionTrend::Stable);
    }

    #[test]
    fn trend_handles_short_histories_without_panic() {
        for n in 2..=9 {
            let mut db = RegressionDb::new();
            for _ in 0..n {
                db.record(make_run(vec![("t1", true)]));
            }
            assert_eq!(db.trend(), RegressionTrend::Stable, "n={}", n);
            let _ = db.summary();
        }
    }

    #[test]
    fn trend_improving_when_pass_rate_rises() {
        let mut db = RegressionDb::new();
        for _ in 0..5 {
            db.record(make_run(vec![("t1", false)]));
        }
        for _ in 0..5 {
            db.record(make_run(vec![("t1", true)]));
        }
        assert_eq!(db.trend(), RegressionTrend::Improving);
    }

    #[test]
    fn pass_rate_ignores_skip_and_counts_flaky() {
        let run = RegressionRun {
            id: "r".into(),
            timestamp: 0,
            branch: "main".into(),
            commit: "c".into(),
            results: vec![
                TestResult {
                    name: "a".into(),
                    status: TestStatus::Pass,
                    duration_ms: 1,
                    error_msg: None,
                },
                TestResult {
                    name: "b".into(),
                    status: TestStatus::Flaky,
                    duration_ms: 1,
                    error_msg: None,
                },
                TestResult {
                    name: "c".into(),
                    status: TestStatus::Skip,
                    duration_ms: 1,
                    error_msg: None,
                },
            ],
            total_duration_ms: 3,
        };
        assert_eq!(run.pass_rate(), 100.0);
    }

    #[test]
    fn summary_without_runs_is_explicit() {
        let db = RegressionDb::new();
        assert!(db.summary().contains("no runs"));
    }

    #[test]
    fn save_creates_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/deeper/regression.json");
        let mut db = RegressionDb::new();
        db.record(make_run(vec![("t1", true)]));
        db.save(&path).expect("save");
        let loaded = RegressionDb::load(&path).expect("load");
        assert_eq!(loaded.runs.len(), 1);
        assert_eq!(loaded.runs[0].results[0].name, "t1");
    }

    #[test]
    fn trend_degrades_when_pass_rate_drops() {
        let mut db = RegressionDb::new();
        for _ in 0..5 {
            db.record(make_run(vec![("t1", true), ("t2", true)]));
        }
        for _ in 0..5 {
            db.record(make_run(vec![("t1", false), ("t2", false)]));
        }
        assert_eq!(db.trend(), RegressionTrend::Degrading);
    }

    #[test]
    fn flaky_requires_at_least_three_runs() {
        let mut db = RegressionDb::new();
        db.record(make_run(vec![("t1", true)]));
        db.record(make_run(vec![("t1", false)]));
        assert!(db.flaky_tests().is_empty(), "2 run belum cukup");
        db.record(make_run(vec![("t1", true)]));
        assert_eq!(db.flaky_tests().len(), 1);
    }

    #[test]
    fn history_is_capped() {
        let mut db = RegressionDb::new();
        for i in 0..105 {
            db.record(make_run(vec![("t1", i % 2 == 0)]));
        }
        assert_eq!(db.runs.len(), 100, "maksimal 100 run disimpan");
        assert_eq!(db.flaky_history["t1"].len(), 50, "maksimal 50 history");
    }

    #[test]
    fn test_json_roundtrip() {
        let mut db = RegressionDb::new();
        db.record(make_run(vec![("t1", true)]));
        let json = serde_json::to_string(&db).unwrap();
        let restored: RegressionDb = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.runs.len(), 1);
    }
}
