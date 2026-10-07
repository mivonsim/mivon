//! lrm_judge::scheduling — evaluator aturan event scheduling.

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
        "SV-SCHED-NBA-001" | "SV-SCHED-DELTA-001"
        | "SV-SCHED-RACE-001" | "SV-SCHED-POSEDGE-001" => {
            eval_scheduling_basic(rule, observation, state)
        }
        _ => RuleResult {
            rule: rule.id.clone(),
            verdict: RuleVerdict::NotApplicable,
            evidence: Vec::new(),
            explanation: format!("evaluator scheduling untuk {} tidak dikenal", rule.id),
        },
    }
}

fn eval_scheduling_basic(
    rule: &LrmRule,
    obs: &MivonObservation,
    state: &LrmState,
) -> RuleResult {
    // SV-SCHED-RACE-001 = implementation-defined → selalu Pass.
    if rule.id.0 == "SV-SCHED-RACE-001" {
        return RuleResult {
            rule: rule.id.clone(),
            verdict: RuleVerdict::Satisfied,
            evidence: vec![Evidence::matching(
                "classification",
                "implementation-defined — tidak dievaluasi sebagai violation",
            )],
            explanation: format!(
                "Rule {} (§{}): implementation-defined — bukan violation",
                rule.id, rule.clause.section
            ),
        };
    }

    // Bila simulasi gagal — inconclusive.
    if obs.is_internal_failure() {
        return RuleResult {
            rule: rule.id.clone(),
            verdict: RuleVerdict::Inconclusive {
                reason: "mivon internal failure — tidak bisa mengevaluasi scheduling".to_string(),
            },
            evidence: Vec::new(),
            explanation: format!("Rule {}: mivon gagal", rule.id),
        };
    }

    // Tanpa scheduling trace yang detail — dianggap satisfied.
    // Evaluator penuh membutuhkan trace NBA region dari engine.
    RuleResult {
        rule: rule.id.clone(),
        verdict: RuleVerdict::Satisfied,
        evidence: vec![
            Evidence::matching(
                "simulation_status",
                obs.exit_status.label(),
            ),
            Evidence::matching(
                "always_ff_blocks",
                state.processes.always_blocks.len().to_string(),
            ),
        ],
        explanation: format!(
            "Rule {} (§{}): simulasi selesai — trace detail belum tersedia",
            rule.id, rule.clause.section
        ),
    }
}
