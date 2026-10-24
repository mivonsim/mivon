//! FEAT-10: Coverage Closure Analytics — track test contributions to coverage.
//!
//! Maps which tests contribute to which coverage points,
//! identifies coverage gaps, and suggests tests to improve closure.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

/// A coverage point (line, branch, or toggle).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoveragePoint {
    pub id: String,
    pub point_type: PointType,
    pub file: String,
    pub line: u32,
    pub column: Option<u32>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PointType {
    Line,
    Branch,
    Toggle,
    FSM,
    Assertion,
    Expression,
}

/// Coverage contribution from a test.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestContribution {
    pub test_name: String,
    pub covered_points: Vec<String>, // point IDs
    pub unique_points: Vec<String>,  // points only covered by this test
}

/// Coverage closure analytics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageClosure {
    pub points: Vec<CoveragePoint>,
    pub test_contributions: Vec<TestContribution>,
    pub uncovered_points: Vec<String>,
}

/// Input kontribusi test (format JSON untuk subcommand build).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestCoverage {
    pub test: String,
    #[serde(default)]
    pub covered: Vec<String>,
}

/// Muat daftar coverage point dari JSON.
pub fn load_points(path: &Path) -> Result<Vec<CoveragePoint>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("gagal baca {}: {}", path.display(), e))?;
    serde_json::from_str(&text)
        .map_err(|e| format!("JSON point tidak valid di {}: {}", path.display(), e))
}

/// Muat kontribusi test dari JSON.
pub fn load_tests(path: &Path) -> Result<Vec<(String, Vec<String>)>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("gagal baca {}: {}", path.display(), e))?;
    let parsed: Vec<TestCoverage> = serde_json::from_str(&text)
        .map_err(|e| format!("JSON test tidak valid di {}: {}", path.display(), e))?;
    Ok(parsed.into_iter().map(|t| (t.test, t.covered)).collect())
}

impl CoverageClosure {
    pub fn new() -> Self {
        CoverageClosure {
            points: Vec::new(),
            test_contributions: Vec::new(),
            uncovered_points: Vec::new(),
        }
    }

    /// Add coverage points.
    pub fn add_points(&mut self, points: Vec<CoveragePoint>) {
        self.points.extend(points);
    }

    /// Record test contribution.
    pub fn record_contribution(&mut self, test: &str, covered: Vec<String>) {
        self.test_contributions.push(TestContribution {
            test_name: test.to_string(),
            covered_points: covered.clone(),
            unique_points: Vec::new(),
        });
        // Recompute unique points
        self.recompute_unique();
    }

    fn recompute_unique(&mut self) {
        let known: HashSet<String> = self.points.iter().map(|p| p.id.clone()).collect();
        let mut point_count: HashMap<String, usize> = HashMap::new();
        for tc in &self.test_contributions {
            // Dedup dalam satu test: ["p1","p1"] bukan dua kontribusi.
            let mut seen: HashSet<&str> = HashSet::new();
            for pid in &tc.covered_points {
                if known.contains(pid.as_str()) && seen.insert(pid.as_str()) {
                    *point_count.entry(pid.clone()).or_insert(0) += 1;
                }
            }
        }
        for tc in &mut self.test_contributions {
            let mut seen: HashSet<String> = HashSet::new();
            tc.unique_points = tc
                .covered_points
                .iter()
                .filter(|pid| known.contains(pid.as_str()))
                .filter(|pid| seen.insert((*pid).clone()))
                .filter(|pid| point_count.get(pid.as_str()).copied().unwrap_or(0) == 1)
                .cloned()
                .collect();
        }
    }

    /// Compute overall coverage.
    ///
    /// Hanya ID yang benar-benar ada di daftar point yang dihitung, sehingga
    /// ID asing tidak bisa membuat coverage > 100%.
    pub fn overall_coverage(&self) -> f64 {
        if self.points.is_empty() {
            return 100.0;
        }
        let known: HashSet<&str> = self.points.iter().map(|p| p.id.as_str()).collect();
        let covered: HashSet<&str> = self
            .test_contributions
            .iter()
            .flat_map(|tc| tc.covered_points.iter().map(|s| s.as_str()))
            .filter(|id| known.contains(id))
            .collect();
        // Denominator = point unik yang dikenal (points.json boleh berisi duplikat).
        (covered.len() as f64 / known.len() as f64 * 100.0).min(100.0)
    }

    /// Bangun closure dari daftar point + kontribusi test.
    pub fn from_inputs(points: Vec<CoveragePoint>, tests: &[(String, Vec<String>)]) -> Self {
        let mut closure = CoverageClosure::new();
        closure.add_points(points);
        // Push langsung lalu recompute sekali (hindari O(T^2)).
        for (test, covered) in tests {
            closure.test_contributions.push(TestContribution {
                test_name: test.clone(),
                covered_points: covered.clone(),
                unique_points: Vec::new(),
            });
        }
        closure.recompute_unique();
        closure.uncovered_points = closure
            .find_uncovered()
            .iter()
            .map(|p| p.id.clone())
            .collect();
        closure
    }

    /// Find uncovered points.
    pub fn find_uncovered(&self) -> Vec<&CoveragePoint> {
        let covered: HashSet<&str> = self
            .test_contributions
            .iter()
            .flat_map(|tc| tc.covered_points.iter().map(|s| s.as_str()))
            .collect();
        self.points
            .iter()
            .filter(|p| !covered.contains(p.id.as_str()))
            .collect()
    }

    /// Find "critical" tests (that cover unique points).
    pub fn critical_tests(&self) -> Vec<&TestContribution> {
        self.test_contributions
            .iter()
            .filter(|tc| !tc.unique_points.is_empty())
            .collect()
    }

    /// Coverage by type.
    pub fn coverage_by_type(&self) -> BTreeMap<String, f64> {
        let covered: HashSet<&str> = self
            .test_contributions
            .iter()
            .flat_map(|tc| tc.covered_points.iter().map(|s| s.as_str()))
            .collect();
        let mut by_type: HashMap<String, (usize, usize)> = HashMap::new();
        for p in &self.points {
            let key = format!("{:?}", p.point_type);
            let entry = by_type.entry(key).or_insert((0, 0));
            entry.0 += 1;
            if covered.contains(p.id.as_str()) {
                entry.1 += 1;
            }
        }
        by_type
            .into_iter()
            .map(|(k, (total, cov))| (k, cov as f64 / total as f64 * 100.0))
            .collect()
    }

    /// Summary.
    pub fn summary(&self) -> String {
        let overall = self.overall_coverage();
        let uncovered = self.find_uncovered().len();
        let critical = self.critical_tests().len();
        format!(
            "CoverageClosure: {:.1}% overall, {} uncovered, {} critical tests, {} total points",
            overall,
            uncovered,
            critical,
            self.points.len(),
        )
    }

    /// Save to JSON.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }

    /// Load from JSON.
    pub fn load(path: &Path) -> Result<Self, String> {
        let json = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        serde_json::from_str(&json).map_err(|e| e.to_string())
    }
}

impl Default for CoverageClosure {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_point(id: &str, pt: PointType) -> CoveragePoint {
        CoveragePoint {
            id: id.into(),
            point_type: pt,
            file: "test.sv".into(),
            line: 1,
            column: None,
            description: None,
        }
    }

    #[test]
    fn load_points_and_tests_from_json() {
        let dir = tempfile::tempdir().unwrap();
        let points_path = dir.path().join("points.json");
        let tests_path = dir.path().join("tests.json");
        std::fs::write(
            &points_path,
            r#"[{"id":"p1","point_type":"Line","file":"a.sv","line":1,"column":null,"description":null}]"#,
        )
        .unwrap();
        std::fs::write(
            &tests_path,
            r#"[{"test":"t1","covered":["p1"]},{"test":"t2"}]"#,
        )
        .unwrap();
        let points = load_points(&points_path).expect("points");
        let tests = load_tests(&tests_path).expect("tests");
        assert_eq!(points.len(), 1);
        assert_eq!(tests.len(), 2);
        assert_eq!(tests[0].1, vec!["p1".to_string()]);
        assert!(tests[1].1.is_empty());
    }

    #[test]
    fn foreign_ids_are_excluded_from_numerator_not_just_clamped() {
        let mut cc = CoverageClosure::new();
        cc.add_points(vec![
            make_point("p1", PointType::Line),
            make_point("p2", PointType::Line),
        ]);
        cc.record_contribution("t1", vec!["p1".into(), "GHOST".into()]);
        // Tanpa filter hasilnya 100% (2/2); dengan filter yang benar = 50%.
        assert_eq!(cc.overall_coverage(), 50.0);
    }

    #[test]
    fn critical_tests_ignore_unknown_ids_and_duplicates() {
        let mut cc = CoverageClosure::new();
        cc.add_points(vec![make_point("p1", PointType::Line)]);
        cc.record_contribution("ghost_only", vec!["GHOST".into()]);
        cc.record_contribution("real", vec!["p1".into(), "p1".into()]);
        let critical: Vec<&str> = cc
            .critical_tests()
            .iter()
            .map(|t| t.test_name.as_str())
            .collect();
        assert_eq!(
            critical,
            vec!["real"],
            "duplikat tidak menghapus status unik"
        );
    }

    #[test]
    fn duplicate_points_do_not_dilute_coverage() {
        let mut cc = CoverageClosure::new();
        cc.add_points(vec![
            make_point("p1", PointType::Line),
            make_point("p1", PointType::Line),
        ]);
        cc.record_contribution("t1", vec!["p1".into()]);
        assert_eq!(cc.overall_coverage(), 100.0);
    }

    #[test]
    fn unknown_point_ids_do_not_exceed_100_percent() {
        let mut cc = CoverageClosure::new();
        cc.add_points(vec![make_point("p1", PointType::Line)]);
        cc.record_contribution("t1", vec!["p1".into(), "GHOST".into()]);
        assert_eq!(cc.overall_coverage(), 100.0);
    }

    #[test]
    fn from_inputs_marks_uncovered_points() {
        let points = vec![
            make_point("p1", PointType::Line),
            make_point("p2", PointType::Line),
        ];
        let tests = vec![("t1".to_string(), vec!["p1".to_string()])];
        let closure = CoverageClosure::from_inputs(points, &tests);
        assert_eq!(closure.overall_coverage(), 50.0);
        assert_eq!(closure.uncovered_points, vec!["p2".to_string()]);
        assert_eq!(closure.find_uncovered().len(), 1);
    }

    #[test]
    fn test_overall_coverage() {
        let mut cc = CoverageClosure::new();
        cc.add_points(vec![
            make_point("p1", PointType::Line),
            make_point("p2", PointType::Line),
            make_point("p3", PointType::Branch),
        ]);
        cc.record_contribution("test_a", vec!["p1".into()]);
        cc.record_contribution("test_b", vec!["p2".into()]);
        assert!((cc.overall_coverage() - 66.67).abs() < 0.1);
    }

    #[test]
    fn test_uncovered() {
        let mut cc = CoverageClosure::new();
        cc.add_points(vec![
            make_point("p1", PointType::Line),
            make_point("p2", PointType::Line),
        ]);
        cc.record_contribution("test_a", vec!["p1".into()]);
        let uncovered = cc.find_uncovered();
        assert_eq!(uncovered.len(), 1);
        assert_eq!(uncovered[0].id, "p2");
    }

    #[test]
    fn test_critical_tests() {
        let mut cc = CoverageClosure::new();
        cc.add_points(vec![
            make_point("p1", PointType::Line),
            make_point("p2", PointType::Line),
        ]);
        cc.record_contribution("test_a", vec!["p1".into()]);
        cc.record_contribution("test_b", vec!["p1".into(), "p2".into()]);
        let critical = cc.critical_tests();
        assert!(critical.iter().any(|tc| tc.test_name == "test_b"));
    }

    #[test]
    fn test_summary() {
        let mut cc = CoverageClosure::new();
        cc.add_points(vec![make_point("p1", PointType::Line)]);
        cc.record_contribution("test_a", vec!["p1".into()]);
        let s = cc.summary();
        assert!(s.contains("100.0%"));
    }
}
