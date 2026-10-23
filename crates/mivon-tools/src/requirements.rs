//! ENT-29 / FEAT-09: Requirements traceability — requirement → test → status.
//!
//! Input:
//! - mapping CSV (header-aware): requirement_id, test_name, module,
//!   description, priority. Kolom boleh berisi koma bila di-quote.
//! - hasil test JSON: [{name, passed, duration_ms, module}] atau skema
//!   regression [{name, status: "Pass"|"Fail", duration_ms}].
//!
//! Output (CLI `mivon requirements report|summary`):
//! ```csv
//! Requirement ID,Description,Status,Test,Module
//! "REQ-RTL-001","counter increments","Covered","test_counter","counter"
//! "REQ-RTL-002","alu add/sub","Failed","2 tests","alu"
//! ```
//! Requirement diagregasi per id: Covered (semua pass), Failed (semua
//! gagal), Partial (campuran / sebagian test hilang), Not Covered.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Mapping dari test ke requirement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequirementMapping {
    pub requirement_id: String,
    pub test_name: String,
    pub module: Option<String>,
    pub description: Option<String>,
    pub priority: Option<String>,
}

/// Status coverage requirement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RequirementStatus {
    Covered,        // Test pass
    Failed,         // Test fail
    NotCovered,     // Tidak ada test
    PartialCovered, // Ada test tapi ada fail
}

impl std::fmt::Display for RequirementStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequirementStatus::Covered => write!(f, "Covered"),
            RequirementStatus::Failed => write!(f, "Failed"),
            RequirementStatus::NotCovered => write!(f, "Not Covered"),
            RequirementStatus::PartialCovered => write!(f, "Partial"),
        }
    }
}

/// Test result untuk requirements tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub duration_ms: u64,
    pub module: Option<String>,
}

/// Requirements coverage report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequirementsReport {
    pub total_requirements: usize,
    pub covered: usize,
    pub failed: usize,
    /// Requirement dengan campuran test pass/fail atau sebagian test hilang.
    pub partial: usize,
    pub not_covered: usize,
    pub coverage_percent: f64,
    pub details: Vec<RequirementDetail>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequirementDetail {
    pub requirement_id: String,
    pub description: String,
    pub status: String,
    pub test_name: Option<String>,
    pub module: Option<String>,
}

impl RequirementsReport {
    /// Generate report dari mappings dan test results.
    ///
    /// Requirement diagregasi per `requirement_id` (satu requirement boleh
    /// punya beberapa test): semua pass = Covered, ada fail = Failed/Partial,
    /// tidak ada hasil = NotCovered. Nama test duplikat di results: entri
    /// pertama menang (deterministik).
    pub fn generate(mappings: &[RequirementMapping], results: &[TestResult]) -> Self {
        let mut results_map: HashMap<&str, &TestResult> = HashMap::new();
        for r in results {
            results_map.entry(r.name.as_str()).or_insert(r);
        }

        // Kelompokkan per requirement, pertahankan urutan kemunculan pertama.
        let mut order: Vec<&str> = Vec::new();
        let mut groups: HashMap<&str, Vec<&RequirementMapping>> = HashMap::new();
        for mapping in mappings {
            let id = mapping.requirement_id.as_str();
            if !groups.contains_key(id) {
                order.push(id);
            }
            groups.entry(id).or_default().push(mapping);
        }

        let mut details = Vec::new();
        let mut covered = 0;
        let mut failed = 0;
        let mut partial = 0;
        let mut not_covered = 0;

        for id in &order {
            let group = &groups[id];
            let mut pass = 0usize;
            let mut fail = 0usize;
            let mut missing = 0usize;
            for mapping in group {
                match results_map.get(mapping.test_name.as_str()) {
                    Some(r) if r.passed => pass += 1,
                    Some(_) => fail += 1,
                    None => missing += 1,
                }
            }
            let status = if fail > 0 && pass == 0 {
                RequirementStatus::Failed
            } else if fail > 0 || (pass > 0 && missing > 0) {
                RequirementStatus::PartialCovered
            } else if pass > 0 {
                RequirementStatus::Covered
            } else {
                RequirementStatus::NotCovered
            };
            match status {
                RequirementStatus::Covered => covered += 1,
                RequirementStatus::Failed => failed += 1,
                RequirementStatus::PartialCovered => partial += 1,
                RequirementStatus::NotCovered => not_covered += 1,
            }
            let first = group[0];
            let test_name = if group.len() == 1 {
                Some(first.test_name.clone())
            } else {
                Some(format!("{} tests", group.len()))
            };
            details.push(RequirementDetail {
                requirement_id: id.to_string(),
                description: first.description.clone().unwrap_or_default(),
                status: status.to_string(),
                test_name,
                module: first.module.clone(),
            });
        }

        let total = order.len();
        let coverage_percent = if total > 0 {
            covered as f64 / total as f64 * 100.0
        } else {
            0.0
        };

        RequirementsReport {
            total_requirements: total,
            covered,
            failed,
            partial,
            not_covered,
            coverage_percent,
            details,
        }
    }

    /// Export sebagai CSV (semua field di-quote dan di-escape).
    pub fn to_csv(&self) -> String {
        let mut csv = String::from("Requirement ID,Description,Status,Test,Module\n");
        for d in &self.details {
            csv.push_str(&format!(
                "{},{},{},{},{}\n",
                csv_field(&d.requirement_id),
                csv_field(&d.description),
                csv_field(&d.status),
                csv_field(d.test_name.as_deref().unwrap_or("")),
                csv_field(d.module.as_deref().unwrap_or("")),
            ));
        }
        csv
    }

    /// Baris header CSV report (dipakai loader header-aware).
    pub fn csv_header() -> &'static str {
        "Requirement ID,Description,Status,Test,Module"
    }

    /// Export sebagai JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Save CSV ke file.
    pub fn save_csv(&self, path: &Path) -> Result<(), String> {
        let csv = self.to_csv();
        std::fs::write(path, &csv).map_err(|e| format!("gagal tulis {}: {}", path.display(), e))
    }

    /// Summary text.
    pub fn summary(&self) -> String {
        format!(
            "{}/{} requirements covered ({:.1}%), {} failed, {} partial, {} not covered",
            self.covered,
            self.total_requirements,
            self.coverage_percent,
            self.failed,
            self.partial,
            self.not_covered
        )
    }
}

/// Load test results dari file JSON (array TestResult).
///
/// Format: `[{"name":"t1","passed":true,"duration_ms":12,"module":"alu"}]`
pub fn load_results(path: &Path) -> Result<Vec<TestResult>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("gagal baca {}: {}", path.display(), e))?;
    let raw: Vec<RawTestResult> = serde_json::from_str(&content)
        .map_err(|e| format!("JSON hasil test tidak valid di {}: {}", path.display(), e))?;
    let mut results = Vec::with_capacity(raw.len());
    for (index, item) in raw.into_iter().enumerate() {
        let passed = match (item.passed, item.status.as_deref()) {
            (Some(p), _) => p,
            (None, Some("Pass" | "PASS" | "pass" | "passed")) => true,
            (None, Some("Fail" | "FAIL" | "fail" | "failed")) => false,
            _ => {
                return Err(format!(
                    "hasil test #{} ({}): butuh field \"passed\" atau \"status\"",
                    index + 1,
                    item.name
                ))
            }
        };
        results.push(TestResult {
            name: item.name,
            passed,
            duration_ms: item.duration_ms,
            module: item.module,
        });
    }
    Ok(results)
}

/// Bentuk longgar supaya kompatibel dengan skema hasil test lain di repo
/// (regression: status "Pass"/"Fail", testdb: message).
#[derive(Deserialize)]
#[allow(dead_code)]
struct RawTestResult {
    name: String,
    #[serde(default)]
    passed: Option<bool>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    duration_ms: u64,
    #[serde(default)]
    module: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    error_msg: Option<String>,
}

/// Load requirement mappings dari CSV file.
pub fn load_mappings(path: &Path) -> Result<Vec<RequirementMapping>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("gagal baca {}: {}", path.display(), e))?;
    let mut rows = content
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty());
    let Some((_, first)) = rows.next() else {
        return Ok(Vec::new());
    };
    let first_fields = split_csv_line(first);
    let columns = detect_columns(&first_fields);
    let mut mappings = Vec::new();
    let mut add = |fields: Vec<String>, line_no: usize| -> Result<(), String> {
        let get = |index: Option<usize>| {
            index
                .and_then(|i| fields.get(i))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        let requirement_id = get(Some(columns.id)).ok_or_else(|| {
            format!("baris {}: kolom requirement id kosong", line_no)
        })?;
        let test_name = get(Some(columns.test)).ok_or_else(|| {
            format!("baris {}: kolom test name kosong", line_no)
        })?;
        mappings.push(RequirementMapping {
            requirement_id,
            test_name,
            module: get(columns.module),
            description: get(columns.description),
            priority: get(columns.priority),
        });
        Ok(())
    };
    if columns.has_header {
        for (index, line) in rows {
            add(split_csv_line(line), index + 1)?;
        }
    } else {
        add(first_fields, 1)?;
        for (index, line) in rows {
            add(split_csv_line(line), index + 1)?;
        }
    }
    Ok(mappings)
}

/// Posisi kolom pada CSV mapping (header-aware, fallback posisional).
struct Columns {
    has_header: bool,
    id: usize,
    test: usize,
    module: Option<usize>,
    description: Option<usize>,
    priority: Option<usize>,
}

fn detect_columns(first: &[String]) -> Columns {
    let lower: Vec<String> = first.iter().map(|s| s.trim().to_ascii_lowercase()).collect();
    let find = |needle: &str| lower.iter().position(|h| h.contains(needle));
    let is_header = find("requirement").is_some() && find("test").is_some();
    if is_header {
        Columns {
            has_header: true,
            id: find("requirement").unwrap_or(0),
            test: find("test").unwrap_or(1),
            module: find("module"),
            description: find("description"),
            priority: find("priority"),
        }
    } else {
        Columns {
            has_header: false,
            id: 0,
            test: 1,
            module: Some(2),
            description: Some(3),
            priority: Some(4),
        }
    }
}

/// Quote satu field CSV (escape kutip ganda).
fn csv_field(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

/// Split satu baris CSV dengan dukungan tanda kutip (termasuk koma di
/// dalam kutip dan kutip ganda escape).
fn split_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted => {
                if chars.peek() == Some(&'"') {
                    current.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            }
            '"' => quoted = true,
            ',' if !quoted => {
                fields.push(std::mem::take(&mut current));
            }
            _ => current.push(c),
        }
    }
    fields.push(current);
    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_requirements_report() {
        let mappings = vec![
            RequirementMapping {
                requirement_id: "REQ-001".into(),
                test_name: "test_a".into(),
                module: Some("mod_a".into()),
                description: Some("requirement a".into()),
                priority: None,
            },
            RequirementMapping {
                requirement_id: "REQ-002".into(),
                test_name: "test_b".into(),
                module: Some("mod_b".into()),
                description: Some("requirement b".into()),
                priority: None,
            },
            RequirementMapping {
                requirement_id: "REQ-003".into(),
                test_name: "test_c".into(),
                module: None,
                description: None,
                priority: None,
            },
        ];
        let results = vec![
            TestResult {
                name: "test_a".into(),
                passed: true,
                duration_ms: 10,
                module: None,
            },
            TestResult {
                name: "test_b".into(),
                passed: false,
                duration_ms: 5,
                module: None,
            },
            // test_c not in results → NotCovered
        ];
        let report = RequirementsReport::generate(&mappings, &results);
        assert_eq!(report.total_requirements, 3);
        assert_eq!(report.covered, 1);
        assert_eq!(report.failed, 1);
        assert_eq!(report.not_covered, 1);
        assert!((report.coverage_percent - 33.33).abs() < 0.1);
    }

    #[test]
    fn test_requirements_csv() {
        let mappings = vec![RequirementMapping {
            requirement_id: "REQ-1".into(),
            test_name: "t1".into(),
            module: None,
            description: Some("desc".into()),
            priority: None,
        }];
        let results = vec![TestResult {
            name: "t1".into(),
            passed: true,
            duration_ms: 1,
            module: None,
        }];
        let report = RequirementsReport::generate(&mappings, &results);
        let csv = report.to_csv();
        assert!(csv.contains("REQ-1"));
        assert!(csv.contains("Covered"));
    }

    fn mapping(id: &str, test: &str) -> RequirementMapping {
        RequirementMapping {
            requirement_id: id.into(),
            test_name: test.into(),
            module: None,
            description: None,
            priority: None,
        }
    }

    fn result(name: &str, passed: bool) -> TestResult {
        TestResult {
            name: name.into(),
            passed,
            duration_ms: 1,
            module: None,
        }
    }

    #[test]
    fn requirements_are_aggregated_per_id() {
        let mappings = vec![
            mapping("REQ-1", "t1"),
            mapping("REQ-1", "t2"),
            mapping("REQ-1", "t3"),
            mapping("REQ-2", "t4"),
        ];
        let results = vec![result("t1", true), result("t2", false), result("t4", true)];
        let report = RequirementsReport::generate(&mappings, &results);
        assert_eq!(report.total_requirements, 2, "unik per requirement_id");
        assert_eq!(report.covered, 1);
        assert_eq!(report.partial, 1);
        assert_eq!(report.coverage_percent, 50.0);
        assert_eq!(report.details.len(), 2);
        assert_eq!(report.details[0].test_name.as_deref(), Some("3 tests"));
    }

    #[test]
    fn duplicate_results_are_deterministic_first_wins() {
        let mappings = vec![mapping("R1", "t1")];
        let results = vec![result("t1", false), result("t1", true)];
        let report = RequirementsReport::generate(&mappings, &results);
        assert_eq!(report.failed, 1);
        assert_eq!(report.covered, 0);
    }

    #[test]
    fn load_mappings_handles_quoted_comma() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("map.csv");
        std::fs::write(
            &path,
            "requirement_id,test_name,module,description,priority\n\
             REQ-1,test_a,alu,\"ALU add, sub, and or\",high\n",
        )
        .unwrap();
        let maps = load_mappings(&path).expect("parse csv");
        assert_eq!(maps.len(), 1);
        assert_eq!(maps[0].description.as_deref(), Some("ALU add, sub, and or"));
        assert_eq!(maps[0].priority.as_deref(), Some("high"));
    }

    #[test]
    fn report_csv_can_be_read_back_as_mappings() {
        let mappings = vec![mapping("REQ-1", "t1"), mapping("REQ-2", "t2")];
        let results = vec![result("t1", true)];
        let report = RequirementsReport::generate(&mappings, &results);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.csv");
        std::fs::write(&path, report.to_csv()).unwrap();
        let loaded = load_mappings(&path).expect("roundtrip");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].requirement_id, "REQ-1");
        assert_eq!(loaded[0].test_name, "t1");
    }

    #[test]
    fn load_results_accepts_regression_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reg.json");
        std::fs::write(
            &path,
            r#"[{"name":"t1","status":"Pass","duration_ms":4,"error_msg":null}]"#,
        )
        .unwrap();
        let results = load_results(&path).expect("parse regression schema");
        assert_eq!(results.len(), 1);
        assert!(results[0].passed);
    }

    #[test]
    fn load_results_reads_json_array() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("results.json");
        std::fs::write(
            &path,
            r#"[{"name":"t1","passed":true,"duration_ms":7,"module":null},
                {"name":"t2","passed":false,"duration_ms":3,"module":"alu"}]"#,
        )
        .unwrap();
        let results = load_results(&path).expect("parse results");
        assert_eq!(results.len(), 2);
        assert!(results[0].passed);
        assert_eq!(results[1].duration_ms, 3);
    }

    #[test]
    fn load_results_rejects_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "{not json").unwrap();
        assert!(load_results(&path).is_err());
    }

    #[test]
    fn test_requirements_summary() {
        let mappings = vec![
            RequirementMapping {
                requirement_id: "R1".into(),
                test_name: "t1".into(),
                module: None,
                description: None,
                priority: None,
            },
            RequirementMapping {
                requirement_id: "R2".into(),
                test_name: "t2".into(),
                module: None,
                description: None,
                priority: None,
            },
        ];
        let results = vec![TestResult {
            name: "t1".into(),
            passed: true,
            duration_ms: 1,
            module: None,
        }];
        let report = RequirementsReport::generate(&mappings, &results);
        let s = report.summary();
        assert!(s.contains("1/2"));
        assert!(s.contains("50.0%"));
    }
}
