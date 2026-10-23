//! FEAT-11: Verification management dashboard.
//!
//! Menggabungkan artefak verifikasi yang sudah ada menjadi satu halaman HTML
//! mandiri (tanpa aset eksternal):
//! - coverage  ← `<prefix>.coverage.json` (output `mivon mcov --json`)
//! - regression ← `.mivon/regression.json` (RegressionDb, FEAT-07)
//! - bugs      ← `.mivon/bugs.json` (BugTracker, FEAT-08)
//!
//! 1 file = 1 tanggung jawab: agregasi + render. Sumber data tetap dimiliki
//! modulnya masing-masing (tidak ada parsing ulang format internal).

use std::path::Path;

use crate::bugtrack::{BugStatus, BugTracker};
use crate::regression::RegressionDb;

/// Ringkasan coverage (dibaca dari JSON `mcov --json`).
#[derive(Debug, Clone, Default)]
pub struct CoverageSection {
    pub top_module: String,
    pub line_percent: f64,
    pub branch_percent: f64,
    pub toggle_signals: u64,
    pub toggle_transitions: u64,
    pub fsm_signals: u64,
    pub fsm_states: u64,
    pub covergroup_points: u64,
    pub covergroup_covered: u64,
    pub covergroup_percent: f64,
}

impl CoverageSection {
    /// Parse dokumen `coverage.json` hasil `mivon mcov --json`.
    /// Field yang hilang dianggap 0 (format antar-versi tetap terbaca).
    pub fn from_json_str(src: &str) -> Result<Self, String> {
        let v: serde_json::Value =
            serde_json::from_str(src).map_err(|e| format!("coverage json: {e}"))?;
        let cov = v.get("coverage").unwrap_or(&v);
        let f = |path: &[&str]| -> f64 {
            let mut cur = cov;
            for k in path {
                match cur.get(k) {
                    Some(next) => cur = next,
                    None => return 0.0,
                }
            }
            cur.as_f64().unwrap_or(0.0)
        };
        Ok(CoverageSection {
            // top_module ada di root dokumen, bukan di dalam objek coverage.
            top_module: v
                .get("top_module")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            line_percent: f(&["line", "percent"]),
            branch_percent: f(&["branch", "percent"]),
            toggle_signals: f(&["toggle", "signals"]) as u64,
            toggle_transitions: f(&["toggle", "transitions"]) as u64,
            fsm_signals: f(&["fsm", "signals"]) as u64,
            fsm_states: f(&["fsm", "states"]) as u64,
            covergroup_points: f(&["covergroup", "points"]) as u64,
            covergroup_covered: f(&["covergroup", "covered"]) as u64,
            covergroup_percent: f(&["covergroup", "percent"]),
        })
    }
}

/// Ringkasan regression dari run terakhir + trend.
#[derive(Debug, Clone)]
pub struct RegressionSection {
    pub runs: usize,
    pub latest_pass_rate: f64,
    pub latest_total: usize,
    pub latest_failed: usize,
    pub trend: String,
    pub flaky: Vec<String>,
    pub last_branch: String,
    pub last_commit: String,
}

impl RegressionSection {
    pub fn from_db(db: &RegressionDb) -> Self {
        let latest = db.runs.last();
        RegressionSection {
            runs: db.runs.len(),
            latest_pass_rate: latest.map(|r| r.pass_rate()).unwrap_or(0.0),
            latest_total: latest.map(|r| r.results.len()).unwrap_or(0),
            latest_failed: latest.map(|r| r.fail_count()).unwrap_or(0),
            trend: format!("{:?}", db.trend()),
            flaky: db.flaky_tests().into_iter().map(|(n, _)| n).collect(),
            last_branch: latest.map(|r| r.branch.clone()).unwrap_or_default(),
            last_commit: latest.map(|r| r.commit.clone()).unwrap_or_default(),
        }
    }
}

/// Ringkasan bug tracker.
#[derive(Debug, Clone, Default)]
pub struct BugSection {
    pub total: usize,
    pub open: usize,
    pub fixed: usize,
    pub wontfix: usize,
    /// Judul bug yang masih Open (untuk daftar di dashboard).
    pub open_titles: Vec<String>,
}

impl BugSection {
    pub fn from_tracker(t: &BugTracker) -> Self {
        let count = |s: BugStatus| t.bugs.iter().filter(|b| b.status == s).count();
        BugSection {
            total: t.bugs.len(),
            open: count(BugStatus::Open),
            fixed: count(BugStatus::Fixed),
            wontfix: count(BugStatus::WontFix),
            open_titles: t
                .bugs
                .iter()
                .filter(|b| b.status == BugStatus::Open)
                .map(|b| format!("{}: {}", b.id, b.title))
                .collect(),
        }
    }
}

/// Dashboard gabungan. Section `None` = artefak tidak tersedia.
#[derive(Debug, Clone, Default)]
pub struct Dashboard {
    pub coverage: Option<CoverageSection>,
    pub regression: Option<RegressionSection>,
    pub bugs: Option<BugSection>,
}

impl Dashboard {
    /// Muat semua artefak yang ada. File yang tidak ada → section None;
    /// file ada tetapi rusak → error (jangan sembunyikan data korup).
    pub fn collect(
        coverage_path: Option<&Path>,
        regression_path: &Path,
        bugs_path: &Path,
    ) -> Result<Self, String> {
        let coverage = match coverage_path {
            Some(p) if p.exists() => {
                let src = std::fs::read_to_string(p)
                    .map_err(|e| format!("baca {}: {e}", p.display()))?;
                Some(CoverageSection::from_json_str(&src)?)
            }
            _ => None,
        };
        let regression = if regression_path.exists() {
            let db = RegressionDb::load(regression_path)?;
            Some(RegressionSection::from_db(&db))
        } else {
            None
        };
        let bugs = if bugs_path.exists() {
            let t = BugTracker::load(bugs_path)?;
            Some(BugSection::from_tracker(&t))
        } else {
            None
        };
        Ok(Dashboard {
            coverage,
            regression,
            bugs,
        })
    }

    /// Render HTML satu file (CSS inline, tanpa JS).
    pub fn render_html(&self) -> String {
        let mut h = String::new();
        h.push_str("<!DOCTYPE html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\n");
        h.push_str("<title>Mivon Verification Dashboard</title>\n<style>\n");
        h.push_str(concat!(
            "body{font-family:system-ui,sans-serif;margin:2rem;color:#1f2328;background:#fafafa}\n",
            "h1{margin:0 0 .25rem} .meta{color:#666;margin:0 0 1.5rem}\n",
            "section{margin:1.5rem 0;padding:1rem 1.25rem;background:#fff;border:1px solid #d8dee4;border-radius:8px}\n",
            "h2{margin:0 0 .75rem;font-size:1.1rem}\n",
            "table{border-collapse:collapse;width:100%} td,th{text-align:left;padding:.35rem .5rem;border-bottom:1px solid #eee}\n",
            ".cards{display:flex;flex-wrap:wrap;gap:.75rem} .card{flex:1 1 140px;padding:.75rem;border:1px solid #eee;border-radius:6px}\n",
            ".num{font-size:1.4rem;font-weight:600} .label{color:#666;font-size:.8rem}\n",
            ".ok{color:#1a7f37} .bad{color:#cf222e} .empty{color:#888;font-style:italic}\n",
            "</style></head><body>\n",
        ));
        h.push_str("<h1>Mivon Verification Dashboard</h1>\n");
        h.push_str("<p class=\"meta\">Generated by mivon vdashboard</p>\n");

        h.push_str("<section><h2>Coverage</h2>\n");
        match &self.coverage {
            Some(c) => {
                h.push_str("<div class=\"cards\">\n");
                let pct_class = |p: f64| if p >= 90.0 { "ok" } else { "bad" };
                card_html(
                    &mut h,
                    "Line coverage",
                    &format!("{:.1}%", c.line_percent),
                    pct_class(c.line_percent),
                );
                card_html(
                    &mut h,
                    "Branch coverage",
                    &format!("{:.1}%", c.branch_percent),
                    pct_class(c.branch_percent),
                );
                card_html(
                    &mut h,
                    "Functional coverage",
                    &format!("{:.1}%", c.covergroup_percent),
                    pct_class(c.covergroup_percent),
                );
                card_html(&mut h, "Toggle signals", &c.toggle_signals.to_string(), "");
                card_html(&mut h, "FSM signals", &c.fsm_signals.to_string(), "");
                h.push_str("</div>\n");
                h.push_str(&format!(
                    "<p class=\"meta\">top: {} &middot; toggle {} transitions &middot; FSM {} states &middot; covergroup {}/{}</p>\n",
                    escape_html(if c.top_module.is_empty() {
                        "-"
                    } else {
                        &c.top_module
                    }),
                    c.toggle_transitions,
                    c.fsm_states,
                    c.covergroup_covered,
                    c.covergroup_points
                ));
            }
            None => h.push_str("<p class=\"empty\">coverage.json tidak ditemukan</p>\n"),
        }
        h.push_str("</section>\n");

        h.push_str("<section><h2>Regression</h2>\n");
        match &self.regression {
            Some(r) => {
                let cls = if r.latest_failed == 0 { "ok" } else { "bad" };
                h.push_str(&format!(
                    "<p>Runs: <b>{}</b> &middot; latest pass rate: <b class=\"{}\">{:.1}%</b> ({}/{} failed) &middot; trend: <b>{}</b></p>\n",
                    r.runs,
                    cls,
                    r.latest_pass_rate,
                    r.latest_failed,
                    r.latest_total,
                    escape_html(&r.trend)
                ));
                if !r.last_branch.is_empty() || !r.last_commit.is_empty() {
                    h.push_str(&format!(
                        "<p class=\"meta\">branch {} &middot; commit {}</p>\n",
                        escape_html(&r.last_branch),
                        escape_html(&r.last_commit)
                    ));
                }
                if r.flaky.is_empty() {
                    h.push_str("<p class=\"empty\">tidak ada flaky test</p>\n");
                } else {
                    h.push_str("<p>Flaky tests:</p><ul>\n");
                    for name in &r.flaky {
                        h.push_str(&format!("<li>{}</li>\n", escape_html(name)));
                    }
                    h.push_str("</ul>\n");
                }
            }
            None => h.push_str("<p class=\"empty\">regression.json tidak ditemukan</p>\n"),
        }
        h.push_str("</section>\n");

        h.push_str("<section><h2>Bugs</h2>\n");
        match &self.bugs {
            Some(b) => {
                h.push_str("<table><tr><th>Total</th><th>Open</th><th>Fixed</th><th>WontFix</th></tr>\n");
                h.push_str(&format!(
                    "<tr><td>{}</td><td class=\"{}\">{}</td><td>{}</td><td>{}</td></tr></table>\n",
                    b.total,
                    if b.open > 0 { "bad" } else { "ok" },
                    b.open,
                    b.fixed,
                    b.wontfix
                ));
                if !b.open_titles.is_empty() {
                    h.push_str("<p>Open bugs:</p><ul>\n");
                    for t in &b.open_titles {
                        h.push_str(&format!("<li>{}</li>\n", escape_html(t)));
                    }
                    h.push_str("</ul>\n");
                }
            }
            None => h.push_str("<p class=\"empty\">bugs.json tidak ditemukan</p>\n"),
        }
        h.push_str("</section>\n");

        h.push_str("</body></html>\n");
        h
    }

    /// Render teks ringkas (untuk terminal).
    pub fn render_text(&self) -> String {
        let mut out = String::new();
        out.push_str("Verification dashboard\n");
        match &self.coverage {
            Some(c) => out.push_str(&format!(
                "  coverage   : line {:.1}%, branch {:.1}%, functional {:.1}% (top {})\n",
                c.line_percent,
                c.branch_percent,
                c.covergroup_percent,
                if c.top_module.is_empty() {
                    "-"
                } else {
                    &c.top_module
                }
            )),
            None => out.push_str("  coverage   : (tidak ada)\n"),
        }
        match &self.regression {
            Some(r) => out.push_str(&format!(
                "  regression : {} runs, latest {:.1}%, {} failed, trend {}, {} flaky\n",
                r.runs,
                r.latest_pass_rate,
                r.latest_failed,
                r.trend,
                r.flaky.len()
            )),
            None => out.push_str("  regression : (tidak ada)\n"),
        }
        match &self.bugs {
            Some(b) => out.push_str(&format!(
                "  bugs       : {} total, {} open, {} fixed, {} wontfix\n",
                b.total, b.open, b.fixed, b.wontfix
            )),
            None => out.push_str("  bugs       : (tidak ada)\n"),
        }
        out
    }
}

/// Satu kartu metrik pada section coverage.
fn card_html(h: &mut String, label: &str, value: &str, class: &str) {
    h.push_str(&format!(
        "<div class=\"card\"><div class=\"num {}\">{}</div><div class=\"label\">{}</div></div>\n",
        class,
        escape_html(value),
        escape_html(label)
    ));
}

/// Escape teks untuk konteks HTML (teks & atribut).
fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::regression::{RegressionRun, TestResult, TestStatus};

    fn cov_json() -> String {
        r#"{"tool":"mivon mcov","top_module":"tb","coverage":{
            "line":{"items":10,"hits":9,"percent":90.0},
            "branch":{"total":4,"covered":3,"percent":75.0},
            "toggle":{"signals":8,"transitions":17},
            "fsm":{"signals":2,"states":5},
            "covergroup":{"points":20,"covered":15,"percent":75.0}}}"#
        .to_string()
    }

    #[test]
    fn parse_coverage_json() {
        let c = CoverageSection::from_json_str(&cov_json()).unwrap();
        assert_eq!(c.top_module, "tb");
        assert!((c.line_percent - 90.0).abs() < 1e-9);
        assert!((c.branch_percent - 75.0).abs() < 1e-9);
        assert_eq!(c.toggle_transitions, 17);
        assert_eq!(c.fsm_states, 5);
        assert_eq!(c.covergroup_covered, 15);
        assert_eq!(c.covergroup_points, 20);
    }

    #[test]
    fn malformed_coverage_is_error() {
        assert!(CoverageSection::from_json_str("not json").is_err());
    }

    fn run(id: &str, pass: usize, fail: usize) -> RegressionRun {
        let mut results = Vec::new();
        for i in 0..pass {
            results.push(TestResult {
                name: format!("t{}", i),
                status: TestStatus::Pass,
                duration_ms: 1,
                error_msg: None,
            });
        }
        for i in 0..fail {
            results.push(TestResult {
                name: format!("f{}", i),
                status: TestStatus::Fail,
                duration_ms: 1,
                error_msg: Some("boom".into()),
            });
        }
        RegressionRun {
            id: id.into(),
            timestamp: 0,
            branch: "main".into(),
            commit: "abc".into(),
            results,
            total_duration_ms: 1,
        }
    }

    #[test]
    fn regression_section_uses_latest_run() {
        let mut db = RegressionDb::new();
        db.record(run("r1", 10, 0));
        db.record(run("r2", 3, 1));
        let s = RegressionSection::from_db(&db);
        assert_eq!(s.runs, 2);
        assert_eq!(s.latest_total, 4);
        assert_eq!(s.latest_failed, 1);
        assert!((s.latest_pass_rate - 75.0).abs() < 1e-9);
        assert_eq!(s.last_branch, "main");
    }

    #[test]
    fn bug_section_counts_status() {
        let mut t = BugTracker::new();
        t.add("B-1", "a").unwrap();
        t.add("B-2", "b").unwrap();
        t.add("B-3", "c").unwrap();
        t.set_status("B-2", BugStatus::Fixed).unwrap();
        t.set_status("B-3", BugStatus::WontFix).unwrap();
        let s = BugSection::from_tracker(&t);
        assert_eq!((s.total, s.open, s.fixed, s.wontfix), (3, 1, 1, 1));
        assert_eq!(s.open_titles, vec!["B-1: a".to_string()]);
    }

    #[test]
    fn html_escapes_untrusted_text() {
        let dash = Dashboard {
            coverage: None,
            regression: Some(RegressionSection {
                runs: 1,
                latest_pass_rate: 50.0,
                latest_total: 2,
                latest_failed: 1,
                trend: "Stable".into(),
                flaky: vec!["<script>alert(1)</script>".into()],
                last_branch: "main".into(),
                last_commit: "abc".into(),
            }),
            bugs: None,
        };
        let html = dash.render_html();
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!html.contains("<script>alert(1)"));
    }

    #[test]
    fn collect_missing_files_yields_empty_sections() {
        let dir = tempfile::tempdir().unwrap();
        let dash = Dashboard::collect(
            Some(&dir.path().join("nope.coverage.json")),
            &dir.path().join("nope-regression.json"),
            &dir.path().join("nope-bugs.json"),
        )
        .unwrap();
        assert!(dash.coverage.is_none());
        assert!(dash.regression.is_none());
        assert!(dash.bugs.is_none());
        let text = dash.render_text();
        assert!(text.contains("coverage   : (tidak ada)"));
    }

    #[test]
    fn collect_reads_real_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let cov = dir.path().join("top.coverage.json");
        std::fs::write(&cov, cov_json()).unwrap();
        let reg = dir.path().join("regression.json");
        let mut db = RegressionDb::new();
        db.record(run("r1", 5, 0));
        db.save(&reg).unwrap();
        let bugs = dir.path().join("bugs.json");
        let mut t = BugTracker::new();
        t.add("B-1", "x").unwrap();
        t.save(&bugs).unwrap();

        let dash = Dashboard::collect(Some(&cov), &reg, &bugs).unwrap();
        assert_eq!(dash.coverage.as_ref().unwrap().top_module, "tb");
        assert_eq!(dash.regression.as_ref().unwrap().runs, 1);
        assert_eq!(dash.bugs.as_ref().unwrap().open, 1);
        let html = dash.render_html();
        assert!(html.contains("90.0%"));
        assert!(html.contains("Regression"));
    }
}
