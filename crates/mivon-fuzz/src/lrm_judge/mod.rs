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

use crate::lrm_model::{LrmRule, LrmState};
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
                    detail: observation.raw_stderr.lines().next()
                        .unwrap_or("internal failure").to_string(),
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
            let result = evaluate_rule(rule, observation, state);
            all_evidence.extend(result.evidence.clone());
            rule_results.push(result);
        }

        // 3. Agregasi hasil menjadi verdict tunggal.
        aggregate(testcase, rule_results, all_evidence, state)
    }
}

/// Evaluasi satu aturan LRM terhadap observasi.
fn evaluate_rule(
    rule: &LrmRule,
    observation: &MivonObservation,
    _state: &LrmState,
) -> RuleResult {
    // Periksa applicability.
    let applicability = ApplicabilityProof::new(vec![
        ("applies_when(state)", (rule.applies_when)(_state)),
    ]);
    if !applicability.applies {
        return RuleResult {
            rule: rule.id.clone(),
            verdict: RuleVerdict::NotApplicable,
            evidence: Vec::new(),
            explanation: format!("Rule {} tidak berlaku untuk state ini", rule.id),
        };
    }

    // Evaluasi semua requirements.
    let mut satisfied = true;
    let mut reasons: Vec<String> = Vec::new();
    let mut evidence: Vec<Evidence> = Vec::new();

    for req in &rule.requires {
        let (ok, reason) = req.check(observation);
        if ok {
            evidence.push(Evidence::matching(
                format!("{:?}", req).split('{').next().unwrap_or("req"),
                reason.clone(),
            ));
        } else {
            satisfied = false;
            evidence.push(Evidence::mismatch(
                format!("{:?}", req).split('{').next().unwrap_or("req"),
                &reason,
                rule.expected_description,
            ));
            reasons.push(reason);
        }
    }

    let verdict = if satisfied {
        RuleVerdict::Satisfied
    } else {
        RuleVerdict::Violated {
            reason: reasons.join("; "),
        }
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
        _ => String::new(),
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
    let first_violation = rule_results.iter().find(|r| {
        matches!(r.verdict, RuleVerdict::Violated { .. })
    });

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
        (Verdict::Pass, JudgeConfidence::Proven, None)
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
