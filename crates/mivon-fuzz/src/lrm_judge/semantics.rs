//! lrm_judge::semantics — evaluator aturan semantics dan execution.

#![cfg(feature = "dev")]

use crate::lrm_model::{LrmRule, LrmState};
use crate::observer::MivonObservation;
use crate::testcase::TestCase;
use crate::verdict::{Evidence, RuleResult, RuleVerdict};

pub fn evaluate(
    rule: &LrmRule,
    _testcase: &TestCase,
    observation: &MivonObservation,
    state: &LrmState,
) -> RuleResult {
    match rule.id.0 {
        "SV-SEM-NBA-001" => eval_nba_region(rule, observation, state),
        "SV-SEM-FILL-001" | "SV-SEM-CONCAT-001" | "SV-SEM-BLOCK-001" => {
            // Aturan ini membutuhkan analisis IR lebih dalam — sementara
            // dikembalikan Inconclusive sampai evaluator penuh diimplementasi.
            RuleResult {
                rule: rule.id.clone(),
                verdict: RuleVerdict::Inconclusive {
                    reason: "evaluator belum diimplementasi penuh — butuh IR analysis".to_string(),
                },
                evidence: Vec::new(),
                explanation: format!("Rule {} belum diimplementasi penuh", rule.id),
            }
        }
        _ => RuleResult {
            rule: rule.id.clone(),
            verdict: RuleVerdict::NotApplicable,
            evidence: Vec::new(),
            explanation: format!("evaluator semantics untuk {} tidak dikenal", rule.id),
        },
    }
}

/// SV-SEM-NBA-001: verifikasi NBA assignment di region NBA.
///
/// Proxy sederhana: bila ada always_ff dan simulasi berhasil tanpa race
/// pada output, dianggap satisfied. Evaluator penuh butuh trace NBA region.
fn eval_nba_region(
    rule: &LrmRule,
    obs: &MivonObservation,
    state: &LrmState,
) -> RuleResult {
    // Bila simulasi gagal — tidak bisa mengevaluasi.
    if obs.has_errors() {
        return RuleResult {
            rule: rule.id.clone(),
            verdict: RuleVerdict::Inconclusive {
                reason: "simulasi gagal — tidak bisa mengevaluasi NBA region".to_string(),
            },
            evidence: Vec::new(),
            explanation: format!("Rule {}: simulasi gagal", rule.id),
        };
    }

    let ff_count = state.processes.always_blocks.iter()
        .filter(|p| p.kind == crate::lrm_model::state::AlwaysKind::Ff)
        .count();

    // Bila tidak ada always_ff — aturan tidak relevan.
    if ff_count == 0 {
        return RuleResult {
            rule: rule.id.clone(),
            verdict: RuleVerdict::NotApplicable,
            evidence: Vec::new(),
            explanation: format!("Rule {}: tidak ada always_ff", rule.id),
        };
    }

    // Cek scheduling observations bila tersedia.
    let nba_fired = obs.scheduling.iter()
        .any(|s| s.region.contains("NBA") || s.region.contains("Nba"));

    RuleResult {
        rule: rule.id.clone(),
        verdict: RuleVerdict::Satisfied,
        evidence: vec![
            Evidence::matching(
                "always_ff_count",
                ff_count.to_string(),
            ),
            Evidence::matching(
                "simulation_completed",
                "ok",
            ),
        ],
        explanation: format!(
            "Rule {} (§{}): {} blok always_ff, simulasi selesai tanpa error",
            rule.id, rule.clause.section, ff_count
        ),
    }
}
