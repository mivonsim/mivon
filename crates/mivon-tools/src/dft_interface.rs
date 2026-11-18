//! ENT-35: DFT Tool Integration — Tessent/DFTMAX interface.
//!
//! Generates DFT insertion reports and test access configurations.

use serde::{Deserialize, Serialize};

/// DFT report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DftReport {
    pub design_name: String,
    pub scan_chains: Vec<ScanChain>,
    pub atpg: AtpgReport,
    pub bist: BistReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanChain {
    pub name: String,
    pub length: u32,
    pub flops: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtpgReport {
    pub total_patterns: u32,
    pub coverage: f64,
    pub undetected_faults: u32,
    pub aborted_faults: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BistReport {
    pub memory_bist_count: u32,
    pub logic_bist_count: u32,
    pub bist_coverage: f64,
}

impl DftReport {
    /// Generate Tessent-compatible report.
    pub fn to_tessent_report(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("Tessent DFT Report: {}\n", self.design_name));
        out.push_str(&format!("Scan Chains: {}\n\n", self.scan_chains.len()));

        for chain in &self.scan_chains {
            out.push_str(&format!(
                "  Chain {} (length: {}, flops: {})\n",
                chain.name,
                chain.length,
                chain.flops.len(),
            ));
        }

        out.push_str(&format!(
            "\nATPG: {} patterns, {:.1}% coverage, {} undetected, {} aborted\n",
            self.atpg.total_patterns,
            self.atpg.coverage * 100.0,
            self.atpg.undetected_faults,
            self.atpg.aborted_faults,
        ));

        out.push_str(&format!(
            "BIST: {} mem, {} logic, {:.1}% coverage\n",
            self.bist.memory_bist_count,
            self.bist.logic_bist_count,
            self.bist.bist_coverage * 100.0,
        ));

        out
    }

    /// Generate DFTMAX-compatible configuration.
    pub fn to_dftmax_config(&self) -> String {
        format!(
            "# DFTMAX configuration for {}\n\
             set_scan_chains {}\n\
             set_atpg_patterns {}\n\
             set_coverage_target 95.0\n\
             set_bist_count {}\n",
            self.design_name,
            self.scan_chains.len(),
            self.atpg.total_patterns,
            self.bist.memory_bist_count + self.bist.logic_bist_count,
        )
    }

    /// Check if test coverage meets threshold.
    pub fn meets_coverage(&self, threshold: f64) -> bool {
        self.atpg.coverage >= threshold
    }

    /// Summary.
    pub fn summary(&self) -> String {
        format!(
            "{}: {} chains, {} patterns, {:.0}% coverage",
            self.design_name,
            self.scan_chains.len(),
            self.atpg.total_patterns,
            self.atpg.coverage * 100.0,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_report() -> DftReport {
        DftReport {
            design_name: "top".into(),
            scan_chains: vec![
                ScanChain {
                    name: "chain0".into(),
                    length: 100,
                    flops: vec!["ff0".into()],
                },
                ScanChain {
                    name: "chain1".into(),
                    length: 150,
                    flops: vec!["ff1".into()],
                },
            ],
            atpg: AtpgReport {
                total_patterns: 500,
                coverage: 0.97,
                undetected_faults: 10,
                aborted_faults: 2,
            },
            bist: BistReport {
                memory_bist_count: 4,
                logic_bist_count: 2,
                bist_coverage: 0.95,
            },
        }
    }

    #[test]
    fn test_tessent_report() {
        let report = make_report();
        let text = report.to_tessent_report();
        assert!(text.contains("Tessent"));
        assert!(text.contains("chain0"));
    }

    #[test]
    fn test_dftmax_config() {
        let report = make_report();
        let config = report.to_dftmax_config();
        assert!(config.contains("set_scan_chains"));
        assert!(config.contains("2"));
    }

    #[test]
    fn test_meets_coverage() {
        let report = make_report();
        assert!(report.meets_coverage(0.95));
        assert!(!report.meets_coverage(0.99));
    }

    #[test]
    fn test_summary() {
        let report = make_report();
        let s = report.summary();
        assert!(s.contains("top"));
        assert!(s.contains("97%"));
    }

    #[test]
    fn test_meets_coverage_true() {
        let report = make_report();
        assert!(report.meets_coverage(0.95));
        assert!(report.meets_coverage(0.90));
    }

    #[test]
    fn test_meets_coverage_false() {
        let report = make_report();
        assert!(!report.meets_coverage(0.98));
        assert!(!report.meets_coverage(1.0));
    }
}
