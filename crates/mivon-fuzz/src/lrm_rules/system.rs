//! lrm_rules::system — aturan system task dan system function.
//!
//! Level L2/L4: system function semantics ($clog2, $bits, $display, dll).

#![cfg(feature = "dev")]

use crate::lrm_model::{ClauseRef, LrmRule, RuleCategory, RuleClassification, RuleId};
use super::RuleRegistry;

pub fn register(reg: &mut RuleRegistry) {
    // SV-SYS-CLOG2-001: $clog2(0) = 0, $clog2(1) = 0, $clog2(2) = 1
    // (LRM §20.8.1). Power-of-two correction wajib.
    reg.register(LrmRule {
        id: RuleId("SV-SYS-CLOG2-001"),
        clause: ClauseRef::sv2017("20.8.1"),
        category: RuleCategory::SystemFunction,
        classification: RuleClassification::RequiredBehavior,
        applies_when: Box::new(|_| true),
        requires: vec![],
        expected_description:
            "$clog2(N): 0→0, 1→0, 2→1, 4→2 — power-of-two correction wajib (§20.8.1)",
    });

    // SV-SYS-DISPLAY-001: %d tanpa lebar field menghasilkan field selebar
    // representasi maksimum tipe (LRM §21.2.1.2 Tabel 21-3).
    reg.register(LrmRule {
        id: RuleId("SV-SYS-DISPLAY-001"),
        clause: ClauseRef::sv2017("21.2.1.2"),
        category: RuleCategory::SystemFunction,
        classification: RuleClassification::RequiredBehavior,
        applies_when: Box::new(|_| true),
        requires: vec![],
        expected_description:
            "$display %d: field width = representasi maksimum tipe (Tabel 21-3 §21.2.1.2)",
    });

    // SV-SYS-RTOI-001: $rtoi(x) harus round ke nearest integer,
    // ties away from zero (LRM §20.7.1). Bukan truncate.
    reg.register(LrmRule {
        id: RuleId("SV-SYS-RTOI-001"),
        clause: ClauseRef::sv2017("20.7.1"),
        category: RuleCategory::SystemFunction,
        classification: RuleClassification::RequiredBehavior,
        applies_when: Box::new(|_| true),
        requires: vec![],
        expected_description:
            "$rtoi(x): round to nearest integer, ties away from zero (§20.7.1)",
    });

    // SV-SYS-FINISH-001: $finish harus mengakhiri simulasi (LRM §20.2).
    reg.register(LrmRule {
        id: RuleId("SV-SYS-FINISH-001"),
        clause: ClauseRef::sv2017("20.2"),
        category: RuleCategory::SystemFunction,
        classification: RuleClassification::RequiredBehavior,
        applies_when: Box::new(|_| true),
        requires: vec![],
        expected_description:
            "$finish harus mengakhiri simulasi (§20.2)",
    });

    // SV-SYS-BITS-001: $bits(T) mengembalikan lebar total tipe T dalam bit
    // (LRM §20.6.2).
    reg.register(LrmRule {
        id: RuleId("SV-SYS-BITS-001"),
        clause: ClauseRef::sv2017("20.6.2"),
        category: RuleCategory::SystemFunction,
        classification: RuleClassification::RequiredBehavior,
        applies_when: Box::new(|_| true),
        requires: vec![],
        expected_description:
            "$bits(T) = lebar total tipe T dalam bit (§20.6.2)",
    });
}
