//! judge — pipeline sekali-jalan: source → observasi → LrmState → JudgeReport.
//!
//! Menyambungkan modul yang selama ini hanya dipakai test (observer,
//! lrm_judge, lrm_rules, testcase, verdict): entry point subcommand
//! `mivon-fuzz judge <file.sv>` dan calon hook kampanye (lapor
//! `Violation`/`MivonInternalFailure` sebagai bug).

#![cfg(feature = "dev")]

use crate::lrm_judge::LrmJudge;
use crate::lrm_model::LrmState;
use crate::lrm_rules::RuleRegistry;
use crate::observer::{build_lrm_state, collect_from_runner};
use crate::testcase::TestCase;
use crate::verdict::{JudgeConfidence, JudgeReport, Verdict};

/// Nilai satu source terhadap hakim LRM (IEEE 1800).
///
/// 1. Observasi black-box via runner subprocess (exit/diagnostics/marker).
/// 2. Compile in-process → state struktural. Gagal compile → verdict
///    `InvalidTest` eksplisit (BUKAN judge di atas state kosong: mayoritas
///    rule `applies_when = |_| true` sehingga state kosong tetap
///    applicable dan jatuh ke `Pass` palsu).
/// 3. `LrmJudge::judge` atas registry penuh.
pub fn judge_single(source: &str, timeout_ms: u64) -> JudgeReport {
    let tc = TestCase::new("judge-single", source);
    let outcome = crate::oracle::with_micd_isolated(|| crate::runner::run_file(source, timeout_ms));
    let obs = collect_from_runner(&outcome);
    let compiled = std::panic::catch_unwind(|| mivon_api::compile_str_quiet(source))
        .ok()
        .and_then(|r| r.ok());
    let Some(ir) = compiled else {
        return JudgeReport {
            testcase_id: tc.id.clone(),
            verdict: Verdict::InvalidTest {
                reason: "source tidak ter-compile — bukan testcase LRM yang valid".to_string(),
            },
            confidence: JudgeConfidence::Insufficient,
            rule_results: Vec::new(),
            evidence: Vec::new(),
            provenance: None,
        };
    };
    let state: LrmState = build_lrm_state(&ir);
    let judge = LrmJudge::new(RuleRegistry::build());
    judge.judge(&tc, &obs, &state)
}

/// Render JudgeReport satu baris per aturan + ringkasan (untuk CLI).
pub fn render_report(report: &JudgeReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("verdict: {}\n", report.verdict.label()));
    out.push_str(&format!("confidence: {:?}\n", report.confidence));
    out.push_str(&format!(
        "rules: {} evaluated, {} evidence\n",
        report.rule_results.len(),
        report.evidence.len()
    ));
    for r in &report.rule_results {
        let first = r.explanation.lines().next().unwrap_or("");
        out.push_str(&format!("  - {} [{:?}]: {}\n", (r.rule.0), r.verdict, first));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Judge jalan ujung-ke-ujung pada design sepele tanpa crash:
    /// verdict terdefinisi + minimal satu aturan terevaluasi.
    /// (Registry punya >30 aturan; design kecil memicu subset syntax/types.)
    #[test]
    fn judge_single_runs_on_trivial_design() {
        let src = "module tiny(input logic a, output logic b);\n  assign b = ~a;\nendmodule\n";
        let rep = judge_single(src, 20_000);
        // Design valid tanpa stimulus: Pass atau Insufficient — bukan
        // InternalFailure, dan pipeline tak panic.
        assert!(
            !matches!(
                rep.verdict,
                crate::verdict::Verdict::MivonInternalFailure { .. }
            ),
            "design sepele tak boleh internal failure: {:?}",
            rep.verdict
        );
    }

    /// Source rusak total → compile gagal → verdict InvalidTest eksplisit
    /// (bukan Pass palsu di atas state kosong), dan pipeline tak crash.
    #[test]
    fn judge_single_survives_garbage() {
        let rep = judge_single("@@@ {{{ ;;;", 20_000);
        assert!(
            matches!(rep.verdict, crate::verdict::Verdict::InvalidTest { .. }),
            "garbage harus InvalidTest, dapat {:?}",
            rep.verdict
        );
        assert!(!rep.should_save(), "InvalidTest bukan bug tersimpan");
        let _ = render_report(&rep);
    }
}
