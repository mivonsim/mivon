//! ENT-21: CI/CD Integration — Output formats untuk Jenkins, GitLab CI,
//! GitHub Actions, dan tools CI lainnya.
//!
//! Menyediakan:
//! - JUnit XML untuk test results (Jenkins/GitLab/GitHub)
//! - SARIF untuk code scanning (GitHub Code Scanning)
//! - Coverage summary untuk CI badges
//! - Benchmark diff untuk performance regression

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

// ═══ JUnit XML Format ═══

/// Test suite dalam JUnit XML format.
#[derive(Debug, Serialize, Deserialize)]
pub struct JUnitTestSuite {
    pub name: String,
    pub tests: usize,
    pub failures: usize,
    pub errors: usize,
    pub skipped: usize,
    pub time: f64,
    #[serde(default)]
    pub test_cases: Vec<JUnitTestCase>,
}

/// Satu test case dalam JUnit XML.
#[derive(Debug, Serialize, Deserialize)]
pub struct JUnitTestCase {
    pub name: String,
    pub classname: String,
    pub time: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<JUnitFailure>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<JUnitSkipped>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JUnitFailure {
    pub message: String,
    #[serde(rename = "#text", skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JUnitSkipped {
    #[serde(rename = "@message", skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl JUnitTestSuite {
    /// Buat test suite baru.
    pub fn new(name: &str) -> Self {
        JUnitTestSuite {
            name: name.to_string(),
            tests: 0,
            failures: 0,
            errors: 0,
            skipped: 0,
            time: 0.0,
            test_cases: Vec::new(),
        }
    }

    /// Tambah test case.
    pub fn add_case(&mut self, case: JUnitTestCase) {
        if case.failure.is_some() {
            self.failures += 1;
        } else if case.skipped.is_some() {
            self.skipped += 1;
        }
        self.tests += 1;
        self.time += case.time;
        self.test_cases.push(case);
    }

    /// Export sebagai XML string.
    pub fn to_xml(&self) -> String {
        let mut xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuite name="{}" tests="{}" failures="{}" errors="{}" skipped="{}" time="{:.3}">"#,
            escape_xml(&self.name),
            self.tests,
            self.failures,
            self.errors,
            self.skipped,
            self.time
        );

        for tc in &self.test_cases {
            xml.push_str(&format!(
                r#"
  <testcase name="{}" classname="{}" time="{:.3}">"#,
                escape_xml(&tc.name),
                escape_xml(&tc.classname),
                tc.time
            ));

            if let Some(ref f) = tc.failure {
                xml.push_str(&format!(
                    r#"
    <failure message="{}">{}</failure>"#,
                    escape_xml(&f.message),
                    escape_xml(f.details.as_deref().unwrap_or(""))
                ));
            }

            if tc.skipped.is_some() {
                xml.push_str(
                    r#"
    <skipped/>"#,
                );
            }

            xml.push_str(
                r#"
  </testcase>"#,
            );
        }

        xml.push_str(
            r#"
</testsuite>"#,
        );
        xml
    }

    /// Save ke file.
    pub fn save_xml(&self, path: &Path) -> Result<(), String> {
        let xml = self.to_xml();
        std::fs::write(path, &xml).map_err(|e| format!("gagal tulis {}: {}", path.display(), e))
    }
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ═══ SARIF Format (GitHub Code Scanning) ═══

/// SARIF log untuk GitHub Code Scanning.
#[derive(Debug, Serialize, Deserialize)]
pub struct SarifLog {
    pub version: String,
    #[serde(rename = "$schema")]
    pub schema: String,
    pub runs: Vec<SarifRun>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SarifRun {
    pub tool: SarifTool,
    #[serde(default)]
    pub results: Vec<SarifResult>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SarifTool {
    pub driver: SarifDriver,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SarifDriver {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub rules: Vec<SarifRule>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SarifRule {
    pub id: String,
    pub name: String,
    pub short_description: SarifText,
    pub default_configuration: SarifLevel,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SarifLevel {
    pub level: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SarifResult {
    pub rule_id: String,
    pub message: SarifText,
    pub locations: Vec<SarifLocation>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SarifText {
    pub text: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SarifLocation {
    pub physical_location: SarifPhysicalLocation,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SarifPhysicalLocation {
    pub artifact_location: SarifArtifactLocation,
    pub region: SarifRegion,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SarifArtifactLocation {
    pub uri: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SarifRegion {
    pub start_line: u32,
    pub start_column: u32,
}

impl Default for SarifLog {
    fn default() -> Self {
        Self::new()
    }
}

impl SarifLog {
    /// Buat SARIF log baru.
    pub fn new() -> Self {
        SarifLog {
            version: "2.1.0".into(),
            schema: "https://json.schemastore.org/sarif-2.1.0.json".into(),
            runs: vec![SarifRun {
                tool: SarifTool {
                    driver: SarifDriver {
                        name: "mivon-lint".into(),
                        version: env!("CARGO_PKG_VERSION").into(),
                        rules: Vec::new(),
                    },
                },
                results: Vec::new(),
            }],
        }
    }

    /// Add a lint finding.
    pub fn add_finding(
        &mut self,
        rule_id: &str,
        message: &str,
        file: &str,
        line: u32,
        column: u32,
    ) {
        if self.runs.is_empty() {
            self.runs.push(SarifRun {
                tool: SarifTool {
                    driver: SarifDriver {
                        name: "mivon-lint".into(),
                        version: env!("CARGO_PKG_VERSION").into(),
                        rules: Vec::new(),
                    },
                },
                results: Vec::new(),
            });
        }

        let run = &mut self.runs[0];
        run.results.push(SarifResult {
            rule_id: rule_id.to_string(),
            message: SarifText {
                text: message.to_string(),
            },
            locations: vec![SarifLocation {
                physical_location: SarifPhysicalLocation {
                    artifact_location: SarifArtifactLocation {
                        uri: file.to_string(),
                    },
                    region: SarifRegion {
                        start_line: line,
                        start_column: column,
                    },
                },
            }],
        });
    }

    /// Export sebagai JSON string.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Save ke file.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let json = self.to_json();
        std::fs::write(path, &json).map_err(|e| format!("gagal tulis {}: {}", path.display(), e))
    }
}

// ═══ Coverage Summary ═══

/// Coverage summary untuk CI badges.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageSummary {
    pub line_percent: f64,
    pub branch_percent: f64,
    pub toggle_percent: f64,
    pub overall_percent: f64,
    pub total_statements: usize,
    pub covered_statements: usize,
}

impl CoverageSummary {
    /// Badge URL untuk shields.io.
    pub fn badge_url(&self) -> String {
        let color = if self.overall_percent >= 90.0 {
            "brightgreen"
        } else if self.overall_percent >= 75.0 {
            "green"
        } else if self.overall_percent >= 50.0 {
            "yellow"
        } else {
            "red"
        };
        format!(
            "https://img.shields.io/badge/coverage-{}%25-{}",
            self.overall_percent as u32, color
        )
    }

    /// Export sebagai JSON untuk CI.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

// ═══ Benchmark Diff ═══

/// Benchmark result untuk perbandingan CI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkResult {
    pub name: String,
    pub value: f64,
    pub unit: String,
    pub lower_is_better: bool,
}

/// Perbandingan benchmark antara dua run.
#[derive(Debug, Serialize, Deserialize)]
pub struct BenchmarkDiff {
    pub results: Vec<BenchmarkDiffEntry>,
    pub regressions: usize,
    pub improvements: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BenchmarkDiffEntry {
    pub name: String,
    pub baseline: f64,
    pub current: f64,
    pub change_percent: f64,
    pub status: String, // "ok", "regression", "improvement"
}

impl BenchmarkDiff {
    /// Compare two sets of benchmark results.
    pub fn compare(baseline: &[BenchmarkResult], current: &[BenchmarkResult]) -> Self {
        let mut entries = Vec::new();
        let mut regressions = 0;
        let mut improvements = 0;

        let baseline_map: HashMap<&str, &BenchmarkResult> =
            baseline.iter().map(|b| (b.name.as_str(), b)).collect();

        for cur in current {
            if let Some(base) = baseline_map.get(cur.name.as_str()) {
                let change = ((cur.value - base.value) / base.value) * 100.0;
                let is_regression = if base.lower_is_better {
                    change > 5.0 // 5% slower = regression
                } else {
                    change < -5.0 // 5% lower = regression
                };
                let is_improvement = if base.lower_is_better {
                    change < -5.0
                } else {
                    change > 5.0
                };

                if is_regression {
                    regressions += 1;
                }
                if is_improvement {
                    improvements += 1;
                }

                entries.push(BenchmarkDiffEntry {
                    name: cur.name.clone(),
                    baseline: base.value,
                    current: cur.value,
                    change_percent: change,
                    status: if is_regression {
                        "regression".into()
                    } else if is_improvement {
                        "improvement".into()
                    } else {
                        "ok".into()
                    },
                });
            }
        }

        BenchmarkDiff {
            results: entries,
            regressions,
            improvements,
        }
    }

    /// Export sebagai JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Summary text untuk CI log.
    pub fn summary(&self) -> String {
        format!(
            "benchmark: {} regressions, {} improvements, {} stable",
            self.regressions,
            self.improvements,
            self.results.len() - self.regressions - self.improvements
        )
    }
}

// ═══ ENT-28: Bug Tracking Integration ═══

/// Lint finding untuk bug tracking export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LintFinding {
    pub module: String,
    pub check: String,
    pub severity: String,
    pub message: String,
    pub file: Option<String>,
    pub line: Option<u32>,
}

/// Export lint findings ke CSV format untuk Jira import.
pub fn export_jira_csv(findings: &[LintFinding]) -> String {
    let mut csv = String::from("Summary,Description,Priority,Labels\n");
    for f in findings {
        let priority = match f.severity.as_str() {
            "E" => "Highest",
            "W" => "High",
            _ => "Medium",
        };
        let summary = format!("[{}] {} — {}", f.check, f.module, f.message);
        let description = format!(
            "Module: {}\nCheck: {}\nSeverity: {}\nFile: {}\nLine: {}",
            f.module,
            f.check,
            f.severity,
            f.file.as_deref().unwrap_or("unknown"),
            f.line.map_or("N/A".into(), |l| l.to_string())
        );
        csv.push_str(&format!(
            "\"{}\",\"{}\",\"{}\",\"mivon-lint\"\n",
            escape_csv(&summary),
            escape_csv(&description),
            priority
        ));
    }
    csv
}

/// Export lint findings ke Redmine CSV format.
pub fn export_redmine_csv(findings: &[LintFinding]) -> String {
    let mut csv = String::from("Subject,Description,Priority,Tracker,Status\n");
    for f in findings {
        let priority = match f.severity.as_str() {
            "E" => "Immediate",
            "W" => "High",
            _ => "Normal",
        };
        let subject = format!("[mivon-lint] {} in {}", f.check, f.module);
        let description = format!(
            "**Module:** {}\n**Check:** {}\n**Severity:** {}\n**Message:** {}",
            f.module, f.check, f.severity, f.message
        );
        csv.push_str(&format!(
            "\"{}\",\"{}\",\"{}\",\"Defect\",\"New\"\n",
            escape_csv(&subject),
            escape_csv(&description),
            priority
        ));
    }
    csv
}

fn escape_csv(s: &str) -> String {
    s.replace('\"', "\"\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_junit_xml() {
        let mut suite = JUnitTestSuite::new("mivon-tests");
        suite.add_case(JUnitTestCase {
            name: "test_counter".into(),
            classname: "mivon::tests".into(),
            time: 0.1,
            failure: None,
            skipped: None,
        });
        suite.add_case(JUnitTestCase {
            name: "test_fail".into(),
            classname: "mivon::tests".into(),
            time: 0.05,
            failure: Some(JUnitFailure {
                message: "assertion failed".into(),
                details: None,
            }),
            skipped: None,
        });
        let xml = suite.to_xml();
        assert!(xml.contains("tests=\"2\""));
        assert!(xml.contains("failures=\"1\""));
        assert!(xml.contains("test_counter"));
        assert!(xml.contains("test_fail"));
        assert!(xml.contains("<failure"));
    }

    #[test]
    fn test_junit_save() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("results.xml");
        let mut suite = JUnitTestSuite::new("test");
        suite.add_case(JUnitTestCase {
            name: "t1".into(),
            classname: "c".into(),
            time: 0.01,
            failure: None,
            skipped: None,
        });
        suite.save_xml(&path).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("testsuite"));
    }

    #[test]
    fn test_sarif() {
        let mut sarif = SarifLog::new();
        sarif.add_finding("unused-signal", "signal 'x' is unused", "test.sv", 10, 5);
        let json = sarif.to_json();
        assert!(json.contains("unused-signal"));
        assert!(json.contains("test.sv"));
    }

    #[test]
    fn test_coverage_badge() {
        let summary = CoverageSummary {
            line_percent: 85.5,
            branch_percent: 72.0,
            toggle_percent: 90.0,
            overall_percent: 82.5,
            total_statements: 1000,
            covered_statements: 825,
        };
        let badge = summary.badge_url();
        assert!(badge.contains("82%"));
        assert!(badge.contains("green"));
    }

    #[test]
    fn test_benchmark_diff() {
        let baseline = vec![BenchmarkResult {
            name: "compile".into(),
            value: 100.0,
            unit: "ms".into(),
            lower_is_better: true,
        }];
        let current = vec![BenchmarkResult {
            name: "compile".into(),
            value: 120.0,
            unit: "ms".into(),
            lower_is_better: true,
        }];
        let diff = BenchmarkDiff::compare(&baseline, &current);
        assert_eq!(diff.regressions, 1);
        assert!(diff.results[0].status == "regression");
    }

    #[test]
    fn test_sarif_add_finding() {
        let mut log = SarifLog::new();
        log.add_finding(
            "RT1006",
            "NBA write conflict on signal a",
            "/home/user/dut.sv",
            42,
            0,
        );
        let json = log.to_json();
        assert!(json.contains("RT1006"));
        assert!(json.contains("NBA write conflict"));
        assert!(json.contains("/home/user/dut.sv"));
        assert!(json.contains("\"start_line\": 42"));
    }
}
