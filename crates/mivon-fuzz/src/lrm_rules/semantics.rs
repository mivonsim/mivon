//! lrm_rules::semantics — aturan evaluasi ekspresi dan statement.
//!
//! Level L1: semantic judge — makna konstruksi, scope, binding.
//! Level L4: execution judge — blocking/non-blocking, prosedural.

#![cfg(feature = "dev")]

use crate::lrm_model::{
    ClauseRef, LrmRule, RuleCategory, RuleClassification, RuleId,
    predicate::{Requirement, SemanticProp},
};
use super::RuleRegistry;

pub fn register(reg: &mut RuleRegistry) {
    // SV-SEM-NBA-001: non-blocking assignment (<=) diselesaikan di region NBA,
    // bukan Active (LRM §10.4.2).
    reg.register(LrmRule {
        id: RuleId("SV-SEM-NBA-001"),
        clause: ClauseRef::sv2017("10.4.2"),
        category: RuleCategory::Semantics,
        classification: RuleClassification::RequiredBehavior,
        applies_when: Box::new(|state| {
            // Berlaku bila ada proses always_ff (NBA paling sering di sini).
            !state.processes.always_blocks.is_empty()
        }),
        requires: vec![
            Requirement::SemanticProperty(SemanticProp::NbaInNbaRegion),
        ],
        expected_description:
            "non-blocking assignment harus diselesaikan di region NBA (§10.4.2)",
    });

    // SV-SEM-FILL-001: fill literal ('0,'1,'x,'z) harus diexpand sesuai
    // lebar target assignment (LRM §5.7.1).
    reg.register(LrmRule {
        id: RuleId("SV-SEM-FILL-001"),
        clause: ClauseRef::sv2017("5.7.1"),
        category: RuleCategory::Semantics,
        classification: RuleClassification::RequiredBehavior,
        applies_when: Box::new(|_| true),
        requires: vec![
            Requirement::SemanticProperty(SemanticProp::FillLiteralExpandsToTargetWidth),
        ],
        expected_description:
            "fill literal ('0,'1,'x,'z) harus diexpand sesuai lebar target",
    });

    // SV-SEM-CONCAT-001: lebar concat harus sama dengan jumlah lebar semua
    // operand (LRM §11.4.12).
    reg.register(LrmRule {
        id: RuleId("SV-SEM-CONCAT-001"),
        clause: ClauseRef::sv2017("11.4.12"),
        category: RuleCategory::Semantics,
        classification: RuleClassification::RequiredBehavior,
        applies_when: Box::new(|_| true),
        requires: vec![
            Requirement::SemanticProperty(SemanticProp::ConcatWidthIsSum),
        ],
        expected_description:
            "lebar ekspresi concat harus = jumlah lebar semua operand",
    });

    // SV-SEM-BLOCK-001: blocking assignment (=) dalam always_ff menghasilkan
    // perilaku implementation-defined (LRM §10.4.1 Note).
    reg.register(LrmRule {
        id: RuleId("SV-SEM-BLOCK-001"),
        clause: ClauseRef::sv2017("10.4.1"),
        category: RuleCategory::Semantics,
        classification: RuleClassification::ImplementationDefined,
        applies_when: Box::new(|state| {
            state.processes.always_blocks.iter()
                .any(|p| p.kind == crate::lrm_model::state::AlwaysKind::Ff)
        }),
        requires: vec![],
        expected_description:
            "blocking assignment dalam always_ff = implementation-defined (LRM note §10.4.1)",
    });
}
