//! lrm_judge::syntax — evaluator aturan syntax/grammar/legality.

#![cfg(feature = "dev")]

use crate::lrm_model::{LrmRule, LrmState};
use crate::observer::MivonObservation;
use crate::testcase::TestCase;
use crate::verdict::{Evidence, RuleResult, RuleVerdict};

/// Evaluasi aturan domain syntax terhadap observasi mivon.
pub fn evaluate(
    rule: &LrmRule,
    _testcase: &TestCase,
    observation: &MivonObservation,
    _state: &LrmState,
) -> RuleResult {
    match rule.id.0 {
        "SV-SYN-DIAG-001" => eval_diag_location(rule, observation),
        _ => RuleResult {
            rule: rule.id.clone(),
            verdict: RuleVerdict::NotApplicable,
            evidence: Vec::new(),
            explanation: format!("evaluator syntax untuk {} belum diimplementasi", rule.id),
        },
    }
}

/// SV-SYN-DIAG-001: semua diagnostik error harus punya lokasi file:line:col.
fn eval_diag_location(rule: &LrmRule, obs: &MivonObservation) -> RuleResult {
    // Exempt summary agregat: definisi tunggal di
    // `lrm_model::predicate::is_summary_error` (jalur hidup verdict).
    use crate::lrm_model::predicate::is_summary_error;
    let errors_without_loc: Vec<&str> = obs
        .diagnostics
        .iter()
        .filter(|d| d.is_error() && d.location.is_none())
        // Ringkasan AGREGAT pasca-sim tak punya lokasi tunggal BY DESIGN
        // ("$fatal: simulasi dihentikan (N)", "N assertion(s) gagal" —
        //  lokasi ada di tiap diagnostik penyusunnya, bukan agregat).
        // Sejajar `is_global_error` oracle (E3006/EL3001 global) — temuan
        // fuzz bug_0227/0248: vonis DIAG-001 palsu atas summary $fatal.
        .filter(|d| !is_summary_error(&d.message))
        .map(|d| d.message.as_str())
        .collect();

    if errors_without_loc.is_empty() {
        RuleResult {
            rule: rule.id.clone(),
            verdict: RuleVerdict::Satisfied,
            evidence: vec![Evidence::matching(
                "diagnostics.all_have_location",
                "semua diagnostik error mempunyai lokasi",
            )],
            explanation: format!(
                "Rule {} terpenuhi: semua {} diagnostik mempunyai lokasi",
                rule.id,
                obs.diagnostics.len()
            ),
        }
    } else {
        RuleResult {
            rule: rule.id.clone(),
            verdict: RuleVerdict::Violated {
                reason: format!(
                    "{} diagnostik error tanpa lokasi: {}",
                    errors_without_loc.len(),
                    errors_without_loc
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
            },
            evidence: errors_without_loc
                .iter()
                .map(|_msg| {
                    Evidence::mismatch("diagnostic.location", "<tidak ada>", "file:line:col")
                })
                .collect(),
            explanation: format!(
                "Rule {} (§{}) dilanggar: {} diagnostik tanpa lokasi",
                rule.id,
                rule.clause.section,
                errors_without_loc.len()
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    /// Summary $fatal/assert agregat di-exempt di BOTH evaluator
    /// (definisi tunggal di predikat — smoke test jalur ini).
    #[test]
    fn summary_errors_exempt_from_location() {
        use crate::lrm_model::predicate::is_summary_error;
        assert!(is_summary_error("$fatal: simulasi dihentikan (1 fatal)"));
        assert!(!is_summary_error("assertion failed"));
    }
}
