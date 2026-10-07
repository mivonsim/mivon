//! lrm_rules::syntax — aturan grammar dan legality IEEE 1800.
//!
//! Level L0: syntax judge — apa yang syntactically legal.

#![cfg(feature = "dev")]

use crate::lrm_model::{
    ClauseRef, LrmRule, RuleCategory, RuleClassification, RuleId,
    predicate::Requirement,
};
use super::RuleRegistry;

pub fn register(reg: &mut RuleRegistry) {
    // SV-SYN-DIAG-001: setiap diagnostik error harus menyertakan lokasi
    // file:line:col (LRM §22.2 mensyaratkan tool melaporkan source location).
    reg.register(LrmRule {
        id: RuleId("SV-SYN-DIAG-001"),
        clause: ClauseRef::sv2017("22.2"),
        category: RuleCategory::Syntax,
        classification: RuleClassification::RequiredBehavior,
        applies_when: Box::new(|_state| {
            // Berlaku selalu — setiap run yang menghasilkan error harus punya lokasi.
            true
        }),
        requires: vec![Requirement::DiagnosticHasLocation],
        expected_description:
            "setiap diagnostik error harus menyertakan lokasi file:line:col",
    });

    // SV-SYN-LEGAL-001: module harus diakhiri endmodule (LRM §23.2).
    reg.register(LrmRule {
        id: RuleId("SV-SYN-LEGAL-001"),
        clause: ClauseRef::sv2017("23.2"),
        category: RuleCategory::Grammar,
        classification: RuleClassification::RequiredBehavior,
        applies_when: Box::new(|_| true),
        requires: vec![],
        expected_description:
            "deklarasi module harus diakhiri dengan endmodule",
    });

    // SV-SYN-LEGAL-002: identifier tidak boleh sama dengan keyword reserved
    // (LRM §5.6).
    reg.register(LrmRule {
        id: RuleId("SV-SYN-LEGAL-002"),
        clause: ClauseRef::sv2017("5.6"),
        category: RuleCategory::Legality,
        classification: RuleClassification::RequiredBehavior,
        applies_when: Box::new(|_| true),
        requires: vec![],
        expected_description:
            "identifier tidak boleh merupakan keyword reserved SV",
    });
}
