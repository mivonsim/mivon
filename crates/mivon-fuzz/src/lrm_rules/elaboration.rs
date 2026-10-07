//! lrm_rules::elaboration — aturan hierarchy, generate, parameter.
//!
//! Level L3: elaboration judge — hierarchy, generate, parameter, port binding.

#![cfg(feature = "dev")]

use crate::lrm_model::{
    ClauseRef, LrmRule, RuleCategory, RuleClassification, RuleId,
    predicate::Requirement,
};
use super::RuleRegistry;

pub fn register(reg: &mut RuleRegistry) {
    // SV-ELAB-PARAM-001: parameter override harus memengaruhi semua
    // penggunaan parameter di dalam instance tersebut (LRM §23.10.1).
    reg.register(LrmRule {
        id: RuleId("SV-ELAB-PARAM-001"),
        clause: ClauseRef::sv2017("23.10.1"),
        category: RuleCategory::Elaboration,
        classification: RuleClassification::RequiredBehavior,
        applies_when: Box::new(|state| {
            state.elaborated_design.parameters.values()
                .any(|p| p.is_overridden)
        }),
        requires: vec![],
        expected_description:
            "parameter override harus memengaruhi semua penggunaan di instance (§23.10.1)",
    });

    // SV-ELAB-PARAM-002: parameter override memengaruhi generate for-loop —
    // jumlah iterasi harus sesuai nilai override (LRM §23.10.1 + §27.4).
    reg.register(LrmRule {
        id: RuleId("SV-ELAB-PARAM-002"),
        clause: ClauseRef::sv2017("27.4"),
        category: RuleCategory::Elaboration,
        classification: RuleClassification::RequiredBehavior,
        applies_when: Box::new(|state| {
            state.elaborated_design.parameters.values().any(|p| p.is_overridden)
                && !state.elaborated_design.generates.is_empty()
        }),
        requires: vec![],
        expected_description:
            "generate for-loop harus menggunakan nilai parameter override (§27.4)",
    });

    // SV-ELAB-PORT-001: koneksi port by-name harus cocok dengan nama port
    // yang dideklarasikan di module (LRM §23.3.2).
    reg.register(LrmRule {
        id: RuleId("SV-ELAB-PORT-001"),
        clause: ClauseRef::sv2017("23.3.2"),
        category: RuleCategory::Elaboration,
        classification: RuleClassification::RequiredBehavior,
        applies_when: Box::new(|state| {
            !state.elaborated_design.port_bindings.is_empty()
        }),
        requires: vec![Requirement::ElaborationSucceeds],
        expected_description:
            "koneksi port by-name harus cocok dengan deklarasi port module (§23.3.2)",
    });

    // SV-ELAB-GEN-001: genvar hanya boleh dipakai di dalam blok generate
    // (LRM §27.4).
    reg.register(LrmRule {
        id: RuleId("SV-ELAB-GEN-001"),
        clause: ClauseRef::sv2017("27.4"),
        category: RuleCategory::Elaboration,
        classification: RuleClassification::RequiredBehavior,
        applies_when: Box::new(|state| {
            !state.elaborated_design.generates.is_empty()
        }),
        requires: vec![],
        expected_description:
            "genvar hanya boleh dipakai di dalam blok generate (§27.4)",
    });

    // SV-ELAB-HIER-001: nama instance dalam hierarki harus unik di scope
    // yang sama (LRM §23.8).
    reg.register(LrmRule {
        id: RuleId("SV-ELAB-HIER-001"),
        clause: ClauseRef::sv2017("23.8"),
        category: RuleCategory::Elaboration,
        classification: RuleClassification::RequiredBehavior,
        applies_when: Box::new(|state| {
            state.elaborated_design.hierarchy.len() > 1
        }),
        requires: vec![],
        expected_description:
            "nama instance harus unik di scope yang sama (§23.8)",
    });
}
