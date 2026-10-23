//! ENT-29: Requirements Management Integration — export test results
//! ke format yang bisa ditelusuri ke requirements (DOORS/Polarion/TestRail).
//!
//! Format: CSV dengan mapping test → requirement ID + coverage status.
//!
//! Contoh CSV output:
//! ```csv
//! Requirement ID,Test Name,Status,Module,Duration
//! REQ-RTL-001,test_counter,Pass,counter,0.1s
//! REQ-RTL-002,test_alu,Fail,alu,0.05s
//! ```

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
    pub fn generate(mappings: &[RequirementMapping], results: &[TestResult]) -> Self {
        let results_map: HashMap<&str, &TestResult> =
            results.iter().map(|r| (r.name.as_str(), r)).collect();

        let mut details = Vec::new();
        let mut covered = 0;
        let mut failed = 0;
        let mut not_covered = 0;

        for mapping in mappings {
            let status = if let Some(result) = results_map.get(mapping.test_name.as_str()) {
                if result.passed {
                    covered += 1;
                    RequirementStatus::Covered
                } else {
                    failed += 1;
                    RequirementStatus::Failed
                }
            } else {
                not_covered += 1;
                RequirementStatus::NotCovered
            };

            details.push(RequirementDetail {
                requirement_id: mapping.requirement_id.clone(),
                description: mapping.description.clone().unwrap_or_default(),
                status: status.to_string(),
                test_name: Some(mapping.test_name.clone()),
                module: mapping.module.clone(),
            });
        }

        let total = mappings.len();
        let coverage_percent = if total > 0 {
            covered as f64 / total as f64 * 100.0
        } else {
            0.0
        };

        RequirementsReport {
            total_requirements: total,
            covered,
            failed,
            not_covered,
            coverage_percent,
            details,
        }
    }

    /// Export sebagai CSV.
    pub fn to_csv(&self) -> String {
        let mut csv = String::from("Requirement ID,Description,Status,Test,Module\n");
        for d in &self.details {
            csv.push_str(&format!(
                "\"{}\",\"{}\",\"{}\",\"{}\",\"{}\"\n",
                d.requirement_id,
                d.description.replace('"', "\"\""),
                d.status,
                d.test_name.as_deref().unwrap_or(""),
                d.module.as_deref().unwrap_or(""),
            ));
        }
        csv
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
            "{}/{} requirements covered ({:.1}%), {} failed, {} not covered",
            self.covered,
            self.total_requirements,
            self.coverage_percent,
            self.failed,
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
    serde_json::from_str(&content)
        .map_err(|e| format!("JSON hasil test tidak valid di {}: {}", path.display(), e))
}

/// Load requirement mappings dari CSV file.
pub fn load_mappings(path: &Path) -> Result<Vec<RequirementMapping>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("gagal baca {}: {}", path.display(), e))?;
    let mut mappings = Vec::new();
    for (i, line) in content.lines().enumerate() {
        if i == 0 || line.trim().is_empty() {
            continue; // skip header
        }
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() >= 2 {
            mappings.push(RequirementMapping {
                requirement_id: parts[0].trim().trim_matches('"').to_string(),
                test_name: parts[1].trim().trim_matches('"').to_string(),
                module: parts.get(2).map(|s| s.trim().trim_matches('"').to_string()),
                description: parts.get(3).map(|s| s.trim().trim_matches('"').to_string()),
                priority: parts.get(4).map(|s| s.trim().trim_matches('"').to_string()),
            });
        }
    }
    Ok(mappings)
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
