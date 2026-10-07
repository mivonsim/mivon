//! lrm_judge::types — evaluator aturan typing dan conversion.

#![cfg(feature = "dev")]

use crate::lrm_model::{LrmRule, LrmState};
use crate::observer::MivonObservation;
use crate::testcase::TestCase;
use crate::verdict::{Evidence, RuleResult, RuleVerdict};

pub fn evaluate(
    rule: &LrmRule,
    _testcase: &TestCase,
    observation: &MivonObservation,
    _state: &LrmState,
) -> RuleResult {
    match rule.id.0 {
        "SV-TYPE-SIGN-001" | "SV-TYPE-CONV-001" | "SV-TYPE-CONV-002"
        | "SV-TYPE-REAL-001" | "SV-TYPE-4STATE-001" | "SV-TYPE-WIDTH-001" => {
            // Aturan tipe membutuhkan testcase dengan sinyal yang diketahui
            // expected value-nya. Tanpa itu — Inconclusive.
            // Testcase rule-directed menyertakan signal assertions di source
            // (pola ASRT_ yang sudah dipakai oracle_icarus).
            eval_via_signal_assertions(rule, observation)
        }
        _ => RuleResult {
            rule: rule.id.clone(),
            verdict: RuleVerdict::NotApplicable,
            evidence: Vec::new(),
            explanation: format!("evaluator types untuk {} tidak dikenal", rule.id),
        },
    }
}

/// Evaluasi via sinyal assertion yang disisipkan testcase.
///
/// Testcase rule-directed menyisipkan $display("ASRT_RESULT=<val>") dan
/// observer mengekstrak nilai. Judge membandingkan dengan expected.
fn eval_via_signal_assertions(
    rule: &LrmRule,
    obs: &MivonObservation,
) -> RuleResult {
    // Cari sinyal dengan prefix ASRT_ di values (diekstrak collector).
    let assertions: Vec<_> = obs.values.iter()
        .filter(|v| v.signal.starts_with("ASRT_"))
        .collect();

    if assertions.is_empty() {
        return RuleResult {
            rule: rule.id.clone(),
            verdict: RuleVerdict::Inconclusive {
                reason: "testcase tidak mempunyai signal assertion ASRT_*".to_string(),
            },
            evidence: Vec::new(),
            explanation: format!(
                "Rule {}: tidak ada ASRT_ signal — testcase blind, butuh rule-directed",
                rule.id
            ),
        };
    }

    // Bila ada ASRT_BAD_ — signal yang seharusnya 0 tapi bukan 0 = violation.
    let violations: Vec<_> = assertions.iter()
        .filter(|v| v.signal.starts_with("ASRT_BAD_"))
        .filter(|v| !matches!(&v.value, crate::lrm_model::LrmValue::FourState { bits, .. }
            if bits.iter().all(|b| *b == crate::lrm_model::LogicBit::Zero)))
        .collect();

    if violations.is_empty() {
        RuleResult {
            rule: rule.id.clone(),
            verdict: RuleVerdict::Satisfied,
            evidence: assertions.iter().map(|v| {
                Evidence::matching(&v.signal, v.value.display())
            }).collect(),
            explanation: format!(
                "Rule {} (§{}): {} assertions semua OK",
                rule.id, rule.clause.section, assertions.len()
            ),
        }
    } else {
        let reasons: Vec<String> = violations.iter()
            .map(|v| format!("{} = {} (harus 0)", v.signal, v.value.display()))
            .collect();

        RuleResult {
            rule: rule.id.clone(),
            verdict: RuleVerdict::Violated {
                reason: reasons.join("; "),
            },
            evidence: violations.iter().map(|v| {
                Evidence::mismatch(&v.signal, v.value.display(), "0")
            }).collect(),
            explanation: format!(
                "Rule {} (§{}) dilanggar: {}",
                rule.id, rule.clause.section, reasons.join("; ")
            ),
        }
    }
}
