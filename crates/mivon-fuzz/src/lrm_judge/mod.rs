//! lrm_judge — evaluator aturan LRM → verdict.
//!
//! Hakim tunggal: IEEE 1800 LRM. Bukan simulator lain.
//!
//! Pipeline:
//! ```text
//! TestCase + MivonObservation
//!         │
//!         ▼
//!    LrmJudge::judge()
//!         │
//!         ├── build LrmState dari IrDesign
//!         ├── cari applicable rules dari RuleRegistry
//!         ├── evaluasi setiap rule → RuleResult
//!         └── agregasi → JudgeReport (Verdict + Confidence + Provenance)
//! ```

#![cfg(feature = "dev")]

pub mod elaboration;
pub mod scheduling;
pub mod semantics;
pub mod syntax;
pub mod types;

use crate::lrm_model::{CheckOutcome, LrmRule, LrmState, RuleCategory};
use crate::lrm_rules::RuleRegistry;
use crate::observer::MivonObservation;
use crate::testcase::TestCase;
use crate::verdict::{
    ApplicabilityProof, Evidence, FailureKind, JudgeConfidence, JudgeReport, Provenance,
    RuleResult, RuleVerdict, Verdict,
};

/// Hakim LRM — mengevaluasi testcase + observasi terhadap aturan LRM.
pub struct LrmJudge {
    registry: RuleRegistry,
}

impl LrmJudge {
    pub fn new(registry: RuleRegistry) -> Self {
        Self { registry }
    }

    /// Entry point utama: evaluasi testcase + observasi → JudgeReport.
    pub fn judge(
        &self,
        testcase: &TestCase,
        observation: &MivonObservation,
        state: &LrmState,
    ) -> JudgeReport {
        // 1. Kegagalan internal mivon — bukan conformance issue tapi tetap bug.
        if observation.is_internal_failure() {
            let kind = map_failure_kind(observation);
            return JudgeReport {
                testcase_id: testcase.id.clone(),
                verdict: Verdict::MivonInternalFailure {
                    kind,
                    detail: observation
                        .raw_stderr
                        .lines()
                        .next()
                        .unwrap_or("internal failure")
                        .to_string(),
                },
                confidence: JudgeConfidence::Proven,
                rule_results: Vec::new(),
                evidence: Vec::new(),
                provenance: None,
            };
        }

        // 2. Evaluasi semua aturan yang berlaku.
        let applicable = self.registry.applicable(state);
        let mut rule_results: Vec<RuleResult> = Vec::new();
        let mut all_evidence: Vec<Evidence> = Vec::new();

        for rule in &applicable {
            let result = evaluate_rule(rule, testcase, observation, state);
            all_evidence.extend(result.evidence.clone());
            rule_results.push(result);
        }

        // 3. Agregasi hasil menjadi verdict tunggal.
        aggregate(testcase, rule_results, all_evidence, state)
    }
}

/// Evaluasi satu aturan LRM terhadap observasi.
///
/// Prioritas: evaluator DOMAIN spesifik (lrm_judge::syntax/semantics/types/
/// elaboration/scheduling) bila ada & memberi verdict non-NotApplicable;
/// fallback ke evaluator generik berbasis `Requirement`.
fn evaluate_rule(
    rule: &LrmRule,
    testcase: &TestCase,
    observation: &MivonObservation,
    state: &LrmState,
) -> RuleResult {
    // Periksa applicability.
    let applicability =
        ApplicabilityProof::new(vec![("applies_when(state)", (rule.applies_when)(state))]);
    if !applicability.applies {
        return RuleResult {
            rule: rule.id.clone(),
            verdict: RuleVerdict::NotApplicable,
            evidence: Vec::new(),
            explanation: format!("Rule {} tidak berlaku untuk state ini", rule.id),
        };
    }

    // Evaluator domain (logika lebih kaya daripada generik).
    let domain = match rule.category {
        RuleCategory::Syntax | RuleCategory::Grammar | RuleCategory::Legality => {
            Some(syntax::evaluate(rule, testcase, observation, state))
        }
        RuleCategory::Semantics => Some(semantics::evaluate(rule, testcase, observation, state)),
        RuleCategory::Typing => Some(types::evaluate(rule, testcase, observation, state)),
        RuleCategory::Elaboration => {
            Some(elaboration::evaluate(rule, testcase, observation, state))
        }
        RuleCategory::Scheduling => Some(scheduling::evaluate(rule, testcase, observation, state)),
        _ => None,
    };
    if let Some(r) = domain {
        if !matches!(r.verdict, RuleVerdict::NotApplicable) {
            return r;
        }
    }

    evaluate_rule_generic(rule, observation)
}

/// Evaluator generik: jalankan setiap `Requirement` atas observasi.
fn evaluate_rule_generic(rule: &LrmRule, observation: &MivonObservation) -> RuleResult {
    // Rule tanpa requirement yang dapat dievaluasi = belum diimplementasi.
    // Jujur Inconclusive (dulu `requires: vec![]` → Satisfied palsu).
    if rule.requires.is_empty() {
        return RuleResult {
            rule: rule.id.clone(),
            verdict: RuleVerdict::Inconclusive {
                reason: "rule tanpa requirement yang dapat dievaluasi".to_string(),
            },
            evidence: Vec::new(),
            explanation: format!(
                "Rule {} (§{}) belum diimplementasi (tidak ada requirement checkable)",
                rule.id, rule.clause.section
            ),
        };
    }

    let mut violated: Vec<String> = Vec::new();
    let mut inconclusive: Vec<String> = Vec::new();
    let mut evidence: Vec<Evidence> = Vec::new();

    for req in &rule.requires {
        let outcome = req.check(observation);
        let label = format!("{:?}", req)
            .split('{')
            .next()
            .unwrap_or("req")
            .to_string();
        match outcome {
            CheckOutcome::Satisfied(reason) => {
                evidence.push(Evidence::matching(label, reason));
            }
            CheckOutcome::Violated(reason) => {
                evidence.push(Evidence::mismatch(
                    label,
                    &reason,
                    rule.expected_description,
                ));
                violated.push(reason);
            }
            CheckOutcome::Inconclusive(reason) => {
                inconclusive.push(reason);
            }
        }
    }

    let verdict = if !violated.is_empty() {
        RuleVerdict::Violated {
            reason: violated.join("; "),
        }
    } else if !inconclusive.is_empty() {
        RuleVerdict::Inconclusive {
            reason: inconclusive.join("; "),
        }
    } else {
        RuleVerdict::Satisfied
    };

    let explanation = match &verdict {
        RuleVerdict::Satisfied => format!(
            "Rule {} (§{}) terpenuhi: {}",
            rule.id, rule.clause.section, rule.expected_description
        ),
        RuleVerdict::Violated { reason } => format!(
            "Rule {} (§{}) dilanggar: {} — observed: {}",
            rule.id, rule.clause.section, rule.expected_description, reason
        ),
        RuleVerdict::Inconclusive { reason } => format!(
            "Rule {} (§{}) belum dapat dievaluasi: {}",
            rule.id, rule.clause.section, reason
        ),
        RuleVerdict::NotApplicable => String::new(),
    };

    RuleResult {
        rule: rule.id.clone(),
        verdict,
        evidence,
        explanation,
    }
}

/// Agregasi semua RuleResult → satu JudgeReport.
fn aggregate(
    testcase: &TestCase,
    rule_results: Vec<RuleResult>,
    evidence: Vec<Evidence>,
    _state: &LrmState,
) -> JudgeReport {
    // Cari violation pertama yang paling kuat.
    let first_violation = rule_results
        .iter()
        .find(|r| matches!(r.verdict, RuleVerdict::Violated { .. }));

    let (verdict, confidence, provenance) = if let Some(viol) = first_violation {
        // Tentukan confidence berdasarkan jumlah bukti dan kejelasan aturan.
        let conf = if evidence.iter().filter(|e| !e.matches).count() > 0 {
            JudgeConfidence::Proven
        } else {
            JudgeConfidence::Strong
        };

        let prov = Provenance {
            rule: viol.rule.clone(),
            clause: crate::lrm_model::ClauseRef::sv2017("0"), // diisi evaluator domain
            applicability: ApplicabilityProof::always(),
            expected: "sesuai LRM".to_string(),
            observed: match &viol.verdict {
                RuleVerdict::Violated { reason } => reason.clone(),
                _ => String::new(),
            },
        };

        let v = Verdict::Violation {
            rule: viol.rule.clone(),
            clause: prov.clause.clone(),
            expected: "sesuai LRM".to_string(),
            observed: match &viol.verdict {
                RuleVerdict::Violated { reason } => reason.clone(),
                _ => String::new(),
            },
        };

        (v, conf, Some(prov))
    } else if rule_results.is_empty() {
        (
            Verdict::JudgeInsufficient {
                reason: "tidak ada aturan yang berlaku untuk testcase ini".to_string(),
                missing: Vec::new(),
            },
            JudgeConfidence::Insufficient,
            None,
        )
    } else {
        // Tidak ada pelanggaran, TAPI ada aturan yang belum dapat dievaluasi
        // → jujur `JudgeInsufficient`, bukan `Pass` palsu.
        let inconclusive: Vec<&str> = rule_results
            .iter()
            .filter(|r| matches!(r.verdict, RuleVerdict::Inconclusive { .. }))
            .map(|r| r.rule.0)
            .collect();
        if !inconclusive.is_empty() {
            (
                Verdict::JudgeInsufficient {
                    reason: format!("{} aturan belum dapat dievaluasi", inconclusive.len()),
                    missing: inconclusive.iter().map(|s| s.to_string()).collect(),
                },
                JudgeConfidence::Insufficient,
                None,
            )
        } else {
            (Verdict::Pass, JudgeConfidence::Proven, None)
        }
    };

    JudgeReport {
        testcase_id: testcase.id.clone(),
        verdict,
        confidence,
        rule_results,
        evidence,
        provenance,
    }
}

fn map_failure_kind(obs: &MivonObservation) -> FailureKind {
    use crate::observer::ExitStatus;
    match &obs.exit_status {
        ExitStatus::Panic => FailureKind::Panic,
        ExitStatus::Hang => FailureKind::Hang,
        ExitStatus::Abort => FailureKind::Abort,
        ExitStatus::Crash { .. } => FailureKind::Crash,
        _ => FailureKind::Panic,
    }
}
