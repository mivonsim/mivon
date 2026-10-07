//! lrm_rules::scheduling — aturan event scheduling LRM §4.
//!
//! Level L5: scheduling judge — event regions, delta cycles, NBA queue.

#![cfg(feature = "dev")]

use crate::lrm_model::{ClauseRef, LrmRule, RuleCategory, RuleClassification, RuleId};
use super::RuleRegistry;

pub fn register(reg: &mut RuleRegistry) {
    // SV-SCHED-NBA-001: NBA assignment harus terjadi di region NBA,
    // setelah seluruh Active region untuk time step yang sama (LRM §4.4.2).
    reg.register(LrmRule {
        id: RuleId("SV-SCHED-NBA-001"),
        clause: ClauseRef::sv2017("4.4.2"),
        category: RuleCategory::Scheduling,
        classification: RuleClassification::RequiredBehavior,
        applies_when: Box::new(|state| {
            !state.processes.always_blocks.is_empty()
        }),
        requires: vec![],
        expected_description:
            "NBA assignment diselesaikan di region NBA setelah Active (§4.4.2)",
    });

    // SV-SCHED-DELTA-001: satu time step boleh mempunyai lebih dari satu
    // delta cycle. Setiap delta Active+NBA = satu iterasi (LRM §4.4).
    reg.register(LrmRule {
        id: RuleId("SV-SCHED-DELTA-001"),
        clause: ClauseRef::sv2017("4.4"),
        category: RuleCategory::Scheduling,
        classification: RuleClassification::RequiredBehavior,
        applies_when: Box::new(|_| true),
        requires: vec![],
        expected_description:
            "satu time step boleh mempunyai lebih dari satu delta cycle (§4.4)",
    });

    // SV-SCHED-RACE-001: dua blocking assignment ke sinyal yang sama di
    // dua always block berbeda dalam Active region = race condition,
    // hasilnya implementation-defined (LRM §4.7).
    reg.register(LrmRule {
        id: RuleId("SV-SCHED-RACE-001"),
        clause: ClauseRef::sv2017("4.7"),
        category: RuleCategory::Scheduling,
        classification: RuleClassification::ImplementationDefined,
        applies_when: Box::new(|state| {
            state.processes.always_blocks.len() > 1
        }),
        requires: vec![],
        expected_description:
            "race condition pada blocking assignment di Active = implementation-defined (§4.7)",
    });

    // SV-SCHED-POSEDGE-001: @(posedge clk) hanya trigger saat transisi
    // 0→1 atau x/z→1 (LRM §9.4.2.1).
    reg.register(LrmRule {
        id: RuleId("SV-SCHED-POSEDGE-001"),
        clause: ClauseRef::sv2017("9.4.2.1"),
        category: RuleCategory::Scheduling,
        classification: RuleClassification::RequiredBehavior,
        applies_when: Box::new(|state| {
            state.processes.always_blocks.iter()
                .any(|p| p.sensitivity.iter().any(|s| s.contains("posedge")))
        }),
        requires: vec![],
        expected_description:
            "@(posedge clk) trigger pada transisi 0→1 atau x/z→1 (§9.4.2.1)",
    });
}
