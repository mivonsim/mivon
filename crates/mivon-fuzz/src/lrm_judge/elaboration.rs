//! lrm_judge::elaboration — evaluator aturan elaboration.

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
        "SV-ELAB-PARAM-001" | "SV-ELAB-PARAM-002" => {
            eval_param_override(rule, observation, state)
        }
        "SV-ELAB-PORT-001" => eval_port_binding(rule, observation, state),
        "SV-ELAB-HIER-001" => eval_hierarchy_unique(rule, observation, state),
        _ => RuleResult {
            rule: rule.id.clone(),
            verdict: RuleVerdict::NotApplicable,
            evidence: Vec::new(),
            explanation: format!("evaluator elaboration untuk {} tidak dikenal", rule.id),
        },
    }
}

/// SV-ELAB-PARAM-001/002: parameter override harus memengaruhi design.
fn eval_param_override(
    rule: &LrmRule,
    obs: &MivonObservation,
    state: &LrmState,
) -> RuleResult {
    // Cek elaborasi berhasil.
    if obs.has_errors() {
        let codes = obs.error_codes();
        // E3006 / EL3001 = no top — bukan param error.
        let is_param_error = codes.iter().any(|c| c.starts_with("EL") || c.contains("param"));
        if is_param_error {
            return RuleResult {
                rule: rule.id.clone(),
                verdict: RuleVerdict::Violated {
                    reason: format!("elaborasi gagal dengan kode: {}", codes.join(", ")),
                },
                evidence: vec![Evidence::mismatch(
                    "elaboration",
                    &format!("error: {}", codes.join(", ")),
                    "success",
                )],
                explanation: format!("Rule {}: elaborasi parameter gagal", rule.id),
            };
        }
        return RuleResult {
            rule: rule.id.clone(),
            verdict: RuleVerdict::Inconclusive {
                reason: format!("elaborasi gagal ({}), tidak bisa mengevaluasi parameter", codes.join(", ")),
            },
            evidence: Vec::new(),
            explanation: format!("Rule {}: elaborasi gagal", rule.id),
        };
    }

    let overridden: Vec<_> = state.elaborated_design.parameters.values()
        .filter(|p| p.is_overridden)
        .collect();

    if overridden.is_empty() {
        return RuleResult {
            rule: rule.id.clone(),
            verdict: RuleVerdict::NotApplicable,
            evidence: Vec::new(),
            explanation: format!("Rule {}: tidak ada parameter override", rule.id),
        };
    }

    let ev: Vec<Evidence> = overridden.iter().map(|p| {
        Evidence::matching(
            format!("{}.{}", p.instance_path, p.param_name),
            p.override_value.clone().unwrap_or_default(),
        )
    }).collect();

    RuleResult {
        rule: rule.id.clone(),
        verdict: RuleVerdict::Satisfied,
        evidence: ev,
        explanation: format!(
            "Rule {} (§{}): {} parameter override tercatat di elaborated design",
            rule.id, rule.clause.section, overridden.len()
        ),
    }
}

/// SV-ELAB-PORT-001: port binding valid.
fn eval_port_binding(
    rule: &LrmRule,
    obs: &MivonObservation,
    state: &LrmState,
) -> RuleResult {
    if obs.has_errors() {
        let codes = obs.error_codes();
        let has_port_err = codes.iter().any(|c| c.contains("port") || c.contains("PORT")
            || *c == "E1003" || *c == "E1004");
        if has_port_err {
            return RuleResult {
                rule: rule.id.clone(),
                verdict: RuleVerdict::Violated {
                    reason: format!("error port binding: {}", codes.join(", ")),
                },
                evidence: vec![Evidence::mismatch("port_binding", "error", "success")],
                explanation: format!("Rule {}: port binding error", rule.id),
            };
        }
    }

    RuleResult {
        rule: rule.id.clone(),
        verdict: RuleVerdict::Satisfied,
        evidence: vec![Evidence::matching(
            "port_bindings",
            state.elaborated_design.port_bindings.len().to_string(),
        )],
        explanation: format!(
            "Rule {} (§{}): {} port binding tanpa error",
            rule.id, rule.clause.section,
            state.elaborated_design.port_bindings.len()
        ),
    }
}

/// SV-ELAB-HIER-001: nama instance unik di scope yang sama.
fn eval_hierarchy_unique(
    rule: &LrmRule,
    obs: &MivonObservation,
    state: &LrmState,
) -> RuleResult {
    use std::collections::HashSet;
    let paths: Vec<&str> = state.elaborated_design.hierarchy.iter()
        .map(|n| n.instance_path.as_str())
        .collect();
    let unique: HashSet<&&str> = paths.iter().collect();

    if paths.len() != unique.len() {
        return RuleResult {
            rule: rule.id.clone(),
            verdict: RuleVerdict::Violated {
                reason: "nama instance duplikat di hierarki".to_string(),
            },
            evidence: vec![Evidence::mismatch(
                "hierarchy.unique",
                &format!("{} total, {} unique", paths.len(), unique.len()),
                "semua unique",
            )],
            explanation: format!("Rule {}: ada instance dengan nama duplikat", rule.id),
        };
    }

    RuleResult {
        rule: rule.id.clone(),
        verdict: RuleVerdict::Satisfied,
        evidence: vec![Evidence::matching(
            "hierarchy.instances",
            format!("{} instance, semua unik", paths.len()),
        )],
        explanation: format!(
            "Rule {} (§{}): {} instance, semua unik",
            rule.id, rule.clause.section, paths.len()
        ),
    }
}
