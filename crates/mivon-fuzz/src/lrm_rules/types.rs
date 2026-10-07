//! lrm_rules::types — aturan typing, conversion, sign extension.
//!
//! Level L2: type judge — typing, conversion, sign, width rules.

#![cfg(feature = "dev")]

use crate::lrm_model::{
    ClauseRef, LrmRule, RuleCategory, RuleClassification, RuleId,
    predicate::{Requirement, SemanticProp},
};
use super::RuleRegistry;

pub fn register(reg: &mut RuleRegistry) {
    // SV-TYPE-SIGN-001: operasi relasional (< <= > >=) signed bila SALAH SATU
    // operand signed (LRM §11.6.1).
    reg.register(LrmRule {
        id: RuleId("SV-TYPE-SIGN-001"),
        clause: ClauseRef::sv2017("11.6.1"),
        category: RuleCategory::Typing,
        classification: RuleClassification::RequiredBehavior,
        applies_when: Box::new(|_| true),
        requires: vec![
            Requirement::SemanticProperty(SemanticProp::SignedExtensionPreservesSign),
        ],
        expected_description:
            "operasi relasional signed bila salah satu operand signed (§11.6.1)",
    });

    // SV-TYPE-CONV-001: konversi implicit ke tipe lebih sempit harus truncate
    // bit MSB (LRM §6.24.1).
    reg.register(LrmRule {
        id: RuleId("SV-TYPE-CONV-001"),
        clause: ClauseRef::sv2017("6.24.1"),
        category: RuleCategory::Typing,
        classification: RuleClassification::RequiredBehavior,
        applies_when: Box::new(|_| true),
        requires: vec![],
        expected_description:
            "konversi implicit ke tipe lebih sempit: truncate MSB (§6.24.1)",
    });

    // SV-TYPE-CONV-002: konversi implicit ke tipe lebih lebar dari signed
    // harus sign-extend (LRM §6.24.1).
    reg.register(LrmRule {
        id: RuleId("SV-TYPE-CONV-002"),
        clause: ClauseRef::sv2017("6.24.1"),
        category: RuleCategory::Typing,
        classification: RuleClassification::RequiredBehavior,
        applies_when: Box::new(|_| true),
        requires: vec![],
        expected_description:
            "konversi implicit dari signed ke lebar lebih besar: sign-extend (§6.24.1)",
    });

    // SV-TYPE-REAL-001: konversi real → integer harus round ke nearest,
    // ties away from zero (LRM §6.12.2). BUKAN truncate.
    reg.register(LrmRule {
        id: RuleId("SV-TYPE-REAL-001"),
        clause: ClauseRef::sv2017("6.12.2"),
        category: RuleCategory::Typing,
        classification: RuleClassification::RequiredBehavior,
        applies_when: Box::new(|_| true),
        requires: vec![],
        expected_description:
            "konversi real → integer: round to nearest, ties away from zero (§6.12.2)",
    });

    // SV-TYPE-4STATE-001: operasi AND dengan X menghasilkan X bila input
    // lain non-zero, 0 bila input lain 0 (LRM §11.4.8, truth table).
    reg.register(LrmRule {
        id: RuleId("SV-TYPE-4STATE-001"),
        clause: ClauseRef::sv2017("11.4.8"),
        category: RuleCategory::Typing,
        classification: RuleClassification::RequiredBehavior,
        applies_when: Box::new(|_| true),
        requires: vec![],
        expected_description:
            "AND dengan X: X & 0 = 0, X & 1 = X, X & X = X (LRM tabel §11.4.8)",
    });

    // SV-TYPE-WIDTH-001: lebar ekspresi integer biner ditentukan oleh operand
    // terlebar, bukan hasil (LRM §11.6.2).
    reg.register(LrmRule {
        id: RuleId("SV-TYPE-WIDTH-001"),
        clause: ClauseRef::sv2017("11.6.2"),
        category: RuleCategory::Typing,
        classification: RuleClassification::RequiredBehavior,
        applies_when: Box::new(|_| true),
        requires: vec![],
        expected_description:
            "lebar ekspresi biner = max(lebar operand kiri, kanan) (§11.6.2)",
    });
}
