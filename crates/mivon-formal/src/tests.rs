//! Unit tests for the formal verification engine.
//!
//! Tests expression translation (IrExpr → Z3), assertion/assignment collection,
//! and basic BMC behavior on minimal designs.

#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use crate::bmc::{collect_assertions, collect_combinational_assignments};
    use crate::*;
    use mivon_core::Symbol;
    use mivon_ir::*;
    use std::collections::HashMap;

    // ── Helper: create a minimal FormalEngine for testing ──

    fn test_engine() -> FormalEngine {
        let cfg = FormalConfig {
            timeout: 10, // 10 second timeout for tests
            ..Default::default()
        };
        let mut engine = FormalEngine::new(cfg);
        engine.init();
        engine
    }

    // ── Expression Translation Tests ──

    #[test]
    fn test_expr_to_z3_int_const() {
        let engine = test_engine();
        let solver = engine.solver.as_ref().unwrap();

        // 5 + 3 == 8
        let five = IrExpr::Const(LogicVec::from_u64(5, 8));
        let three = IrExpr::Const(LogicVec::from_u64(3, 8));
        let add = IrExpr::BinaryOp(BinaryIrOp::Add, Box::new(five), Box::new(three));

        let add_z3 = engine.expr_to_z3_int(&add);
        assert!(add_z3.is_some(), "expr_to_z3_int should translate Add");
        let add_z3 = add_z3.unwrap();

        let eight = z3::ast::BV::from_u64(8, 8);
        solver.assert(add_z3.eq(&eight));
        assert_eq!(solver.check(), z3::SatResult::Sat, "5 + 3 should equal 8");
    }

    #[test]
    fn test_expr_to_z3_int_sub_mul() {
        let engine = test_engine();
        let solver = engine.solver.as_ref().unwrap();

        // (10 - 3) * 2 == 14
        let ten = IrExpr::Const(LogicVec::from_u64(10, 8));
        let three = IrExpr::Const(LogicVec::from_u64(3, 8));
        let two = IrExpr::Const(LogicVec::from_u64(2, 8));
        let sub = IrExpr::BinaryOp(BinaryIrOp::Sub, Box::new(ten), Box::new(three));
        let mul = IrExpr::BinaryOp(BinaryIrOp::Mul, Box::new(sub), Box::new(two));

        let mul_z3 = engine.expr_to_z3_int(&mul).unwrap();
        let fourteen = z3::ast::BV::from_u64(14, 8);
        solver.assert(mul_z3.eq(&fourteen));
        assert_eq!(
            solver.check(),
            z3::SatResult::Sat,
            "(10 - 3) * 2 should equal 14"
        );
    }

    #[test]
    fn test_expr_to_z3_int_bitwise() {
        let engine = test_engine();
        let solver = engine.solver.as_ref().unwrap();

        // 5 & 3 == 1 (0101 & 0011 = 0001)
        let five = IrExpr::Const(LogicVec::from_u64(5, 4));
        let three = IrExpr::Const(LogicVec::from_u64(3, 4));
        let bitand = IrExpr::BinaryOp(BinaryIrOp::BitAnd, Box::new(five), Box::new(three));

        let and_z3 = engine.expr_to_z3_int(&bitand).unwrap();
        let one = z3::ast::BV::from_u64(1, 4);
        solver.assert(and_z3.eq(&one));
        assert_eq!(solver.check(), z3::SatResult::Sat, "5 & 3 should equal 1");
    }

    #[test]
    fn test_expr_to_z3_bool_eq() {
        let engine = test_engine();
        let solver = engine.solver.as_ref().unwrap();

        // 5 == 5 should be true
        let five_a = IrExpr::Const(LogicVec::from_u64(5, 8));
        let five_b = IrExpr::Const(LogicVec::from_u64(5, 8));
        let eq = IrExpr::BinaryOp(BinaryIrOp::Eq, Box::new(five_a), Box::new(five_b));

        let eq_bool = engine.expr_to_z3_bool(&eq).unwrap();
        solver.assert(&eq_bool);
        assert_eq!(
            solver.check(),
            z3::SatResult::Sat,
            "5 == 5 should be satisfiable (true)"
        );
    }

    #[test]
    fn test_expr_to_z3_bool_lt() {
        let engine = test_engine();
        let solver = engine.solver.as_ref().unwrap();

        // 3 < 5 should be true
        let three = IrExpr::Const(LogicVec::from_u64(3, 8));
        let five = IrExpr::Const(LogicVec::from_u64(5, 8));
        let lt = IrExpr::BinaryOp(BinaryIrOp::Lt, Box::new(three), Box::new(five));

        let lt_bool = engine.expr_to_z3_bool(&lt).unwrap();
        // Assert NOT(3 < 5) and verify UNSAT
        solver.assert(lt_bool.not());
        assert_eq!(
            solver.check(),
            z3::SatResult::Unsat,
            "NOT(3 < 5) should be unsat"
        );
    }

    #[test]
    fn test_expr_to_z3_bool_cond() {
        let engine = test_engine();
        let solver = engine.solver.as_ref().unwrap();

        // cond ? 1 : 0 — with cond = true, result should be 1
        let cond = IrExpr::Const(LogicVec::from_u64(1, 1)); // true
        let one = IrExpr::Const(LogicVec::from_u64(1, 8));
        let zero = IrExpr::Const(LogicVec::from_u64(0, 8));
        let cond_expr = IrExpr::Cond(Box::new(cond), Box::new(one), Box::new(zero));

        let cond_z3 = engine.expr_to_z3_int(&cond_expr).unwrap();
        let one_z3 = z3::ast::BV::from_u64(1, 8);
        solver.assert(cond_z3.eq(&one_z3));
        assert_eq!(solver.check(), z3::SatResult::Sat, "true ? 1 : 0 == 1");
    }

    #[test]
    fn test_expr_to_z3_int_fill_lit() {
        let engine = test_engine();
        let solver = engine.solver.as_ref().unwrap();

        // Fill '1 → BV 1 (width 1)
        let fill = IrExpr::FillLit(LogicVal::One);
        let fill_z3 = engine.expr_to_z3_int(&fill).unwrap();
        let one = z3::ast::BV::from_u64(1, 1);
        solver.assert(fill_z3.eq(&one));
        assert_eq!(solver.check(), z3::SatResult::Sat, "FillLit(One) == 1");
    }

    #[test]
    fn test_expr_to_z3_bool_not() {
        let engine = test_engine();
        let solver = engine.solver.as_ref().unwrap();

        // NOT(0) should be true
        let zero = IrExpr::Const(LogicVec::from_u64(0, 1));
        let not = IrExpr::UnaryOp(UnaryIrOp::Not, Box::new(zero));

        let not_bool = engine.expr_to_z3_bool(&not).unwrap();
        solver.assert(&not_bool);
        assert_eq!(solver.check(), z3::SatResult::Sat, "NOT(0) should be true");
    }

    // ── Assertion Collection Tests ──

    #[test]
    fn test_collect_assertions_empty() {
        let result = collect_assertions(&[]);
        assert!(result.is_empty(), "No processes → no assertions");
    }

    #[test]
    fn test_collect_assertions_simple() {
        let cond = IrExpr::Const(LogicVec::from_u64(1, 1));
        let assert_stmt = IrStmt::Assert {
            cond: cond.clone(),
            pass_stmt: vec![],
            fail_stmt: vec![],
            clock_event: None,
            disable_iff: None,
            sequence: None,
            line: 0,
            col: 0,
        };

        let processes = vec![Process::Combinational {
            name: Symbol::from("test_proc"),
            sensitivity: vec![],
            body: vec![assert_stmt],
        }];

        let result = collect_assertions(&processes);
        assert_eq!(result.len(), 1, "Should find 1 assertion");
        assert_eq!(
            result[0].0, "test_proc",
            "Assertion should be from test_proc"
        );
    }

    #[test]
    fn test_collect_assertions_nested_block() {
        let cond = IrExpr::Const(LogicVec::from_u64(1, 1));
        let assert_stmt = IrStmt::Assert {
            cond: cond.clone(),
            pass_stmt: vec![],
            fail_stmt: vec![],
            clock_event: None,
            disable_iff: None,
            sequence: None,
            line: 0,
            col: 0,
        };

        let inner_block = IrStmt::Block {
            stmts: vec![assert_stmt],
        };

        let processes = vec![Process::Initial {
            name: Symbol::from("init_proc"),
            body: vec![inner_block],
        }];

        let result = collect_assertions(&processes);
        assert_eq!(result.len(), 1, "Should find 1 assertion in nested block");
    }

    #[test]
    fn test_collect_assertions_if_branches() {
        let cond_true = IrExpr::Const(LogicVec::from_u64(1, 1));
        let assert_a = IrStmt::Assert {
            cond: IrExpr::Const(LogicVec::from_u64(0, 1)),
            pass_stmt: vec![],
            fail_stmt: vec![],
            clock_event: None,
            disable_iff: None,
            sequence: None,
            line: 0,
            col: 0,
        };
        let assert_b = IrStmt::Assert {
            cond: IrExpr::Const(LogicVec::from_u64(1, 1)),
            pass_stmt: vec![],
            fail_stmt: vec![],
            clock_event: None,
            disable_iff: None,
            sequence: None,
            line: 0,
            col: 0,
        };

        let if_stmt = IrStmt::If {
            cond: cond_true,
            true_branch: vec![assert_a],
            false_branch: vec![assert_b],
        };

        let processes = vec![Process::Combinational {
            name: Symbol::from("if_proc"),
            sensitivity: vec![],
            body: vec![if_stmt],
        }];

        let result = collect_assertions(&processes);
        assert_eq!(result.len(), 2, "Should find 2 assertions (both branches)");
    }

    // ── Assignment Collection Tests ──

    #[test]
    fn test_collect_assignments_empty() {
        let result = collect_combinational_assignments(&[]);
        assert_eq!(result.len(), 1, "Should return one empty depth group");
        assert!(result[0].is_empty(), "Depth group should be empty");
    }

    #[test]
    fn test_collect_assignments_simple() {
        let rhs = IrExpr::Const(LogicVec::from_u64(42, 8));
        let assign = IrStmt::BlockingAssign {
            lhs: IrLValue::Signal(0, 8),
            rhs,
            delay: None,
        };

        let processes = vec![Process::Combinational {
            name: Symbol::from("test_proc"),
            sensitivity: vec![],
            body: vec![assign],
        }];

        let result = collect_combinational_assignments(&processes);
        assert_eq!(result.len(), 1, "Should have 1 depth group");
        assert_eq!(result[0].len(), 1, "Should have 1 assignment");
        assert_eq!(result[0][0].0, 0, "Signal ID should be 0");
    }

    #[test]
    fn test_collect_assignments_no_dup() {
        // Same signal assigned twice in same process — should only collect first
        let rhs1 = IrExpr::Const(LogicVec::from_u64(10, 8));
        let rhs2 = IrExpr::Const(LogicVec::from_u64(20, 8));
        let assign1 = IrStmt::BlockingAssign {
            lhs: IrLValue::Signal(0, 8),
            rhs: rhs1,
            delay: None,
        };
        let assign2 = IrStmt::BlockingAssign {
            lhs: IrLValue::Signal(0, 8),
            rhs: rhs2,
            delay: None,
        };

        let processes = vec![Process::Combinational {
            name: Symbol::from("dup_proc"),
            sensitivity: vec![],
            body: vec![assign1, assign2],
        }];

        let result = collect_combinational_assignments(&processes);
        // Should only have 1 assignment for signal 0 (first one)
        let sig_ids: Vec<usize> = result[0].iter().map(|(id, _)| *id).collect();
        assert_eq!(
            sig_ids.len(),
            1,
            "Should only collect first assignment per signal"
        );
    }

    // ── BMC Smoke Tests ──

    #[test]
    fn test_bmc_smoke_no_processes() {
        let design = IrDesign {
            top: IrModule {
                name: Symbol::from("test"),
                signals: vec![],
                inputs: vec![],
                outputs: vec![],
                inouts: vec![],
                processes: vec![],
                sub_instances: vec![],
            },
            modules: HashMap::new(),
            classes: HashMap::new(),
            covergroups: vec![],
            dpi_imports: vec![],
            hier_signal_map: HashMap::new(),
            udp_defs: vec![],
            specify_items: vec![],
            timescale: None,
            source_file: None,
            source_lines: None,
            module_functions: HashMap::new(),
            pkg_scoped_consts: HashMap::new(),
            coverage_exclusions: Vec::new(),
            stmt_lines: HashMap::new(),
            net_aliases: HashMap::new(),
        };

        let mut engine = test_engine();
        let results = engine.check_assertions_bmc(&design);
        assert!(results.is_empty(), "No processes → no BMC results");
    }

    #[test]
    fn test_bmc_smoke_simple_assert() {
        // Create a minimal design with 1 combinational process containing assert(1)
        // This assertion should always pass (true is never false)
        let cond_true = IrExpr::Const(LogicVec::from_u64(1, 1));
        let assert_stmt = IrStmt::Assert {
            cond: cond_true,
            pass_stmt: vec![],
            fail_stmt: vec![],
            clock_event: None,
            disable_iff: None,
            sequence: None,
            line: 0,
            col: 0,
        };

        let design = IrDesign {
            top: IrModule {
                name: Symbol::from("test"),
                signals: vec![],
                inputs: vec![],
                outputs: vec![],
                inouts: vec![],
                processes: vec![Process::Combinational {
                    name: Symbol::from("always_comb"),
                    sensitivity: vec![],
                    body: vec![assert_stmt],
                }],
                sub_instances: vec![],
            },
            modules: HashMap::new(),
            classes: HashMap::new(),
            covergroups: vec![],
            dpi_imports: vec![],
            hier_signal_map: HashMap::new(),
            udp_defs: vec![],
            specify_items: vec![],
            timescale: None,
            source_file: None,
            source_lines: None,
            module_functions: HashMap::new(),
            pkg_scoped_consts: HashMap::new(),
            coverage_exclusions: Vec::new(),
            stmt_lines: HashMap::new(),
            net_aliases: HashMap::new(),
        };

        let mut engine = test_engine();
        let results = engine.check_assertions_bmc(&design);
        // Should have 1 result (the assertion)
        assert_eq!(results.len(), 1, "Should have 1 BMC result");
        // assert(1) is always true, so negation is always false → UNSAT → Pass
        assert_eq!(results[0].1, FormalResult::Pass, "assert(1) should pass");
    }

    #[test]
    fn test_bmc_counterexample_false_assert() {
        // assert(0) should always fail with counterexample at depth 0
        let cond_false = IrExpr::Const(LogicVec::from_u64(0, 1));
        let assert_stmt = IrStmt::Assert {
            cond: cond_false,
            pass_stmt: vec![],
            fail_stmt: vec![],
            clock_event: None,
            disable_iff: None,
            sequence: None,
            line: 0,
            col: 0,
        };

        let design = IrDesign {
            top: IrModule {
                name: Symbol::from("test"),
                signals: vec![],
                inputs: vec![],
                outputs: vec![],
                inouts: vec![],
                processes: vec![Process::Combinational {
                    name: Symbol::from("always_comb"),
                    sensitivity: vec![],
                    body: vec![assert_stmt],
                }],
                sub_instances: vec![],
            },
            modules: HashMap::new(),
            classes: HashMap::new(),
            covergroups: vec![],
            dpi_imports: vec![],
            hier_signal_map: HashMap::new(),
            udp_defs: vec![],
            specify_items: vec![],
            timescale: None,
            source_file: None,
            source_lines: None,
            module_functions: HashMap::new(),
            pkg_scoped_consts: HashMap::new(),
            coverage_exclusions: Vec::new(),
            stmt_lines: HashMap::new(),
            net_aliases: HashMap::new(),
        };

        let mut engine = test_engine();
        let results = engine.check_assertions_bmc(&design);
        assert_eq!(results.len(), 1, "Should have 1 BMC result");
        // assert(0) is always false → negation is always true → SAT → Counterexample
        assert!(
            matches!(results[0].1, FormalResult::Counterexample(_)),
            "assert(0) should give Counterexample, got {:?}",
            results[0].1
        );
    }

    // ── Expression Translation Edge Cases ──

    #[test]
    fn test_expr_to_z3_int_shift() {
        let engine = test_engine();
        let solver = engine.solver.as_ref().unwrap();

        // 1 << 3 == 8
        let one = IrExpr::Const(LogicVec::from_u64(1, 8));
        let three = IrExpr::Const(LogicVec::from_u64(3, 8));
        let shl = IrExpr::BinaryOp(BinaryIrOp::Shl, Box::new(one), Box::new(three));

        let shl_z3 = engine.expr_to_z3_int(&shl).unwrap();
        let eight = z3::ast::BV::from_u64(8, 8);
        solver.assert(shl_z3.eq(&eight));
        assert_eq!(solver.check(), z3::SatResult::Sat, "1 << 3 == 8");
    }

    #[test]
    fn test_expr_to_z3_bool_logical_or() {
        let engine = test_engine();
        let solver = engine.solver.as_ref().unwrap();

        // false || false should be false
        let f_a = IrExpr::Const(LogicVec::from_u64(0, 1));
        let f_b = IrExpr::Const(LogicVec::from_u64(0, 1));
        let or_expr = IrExpr::BinaryOp(BinaryIrOp::LogicalOr, Box::new(f_a), Box::new(f_b));

        let or_bool = engine.expr_to_z3_bool(&or_expr).unwrap();
        solver.assert(&or_bool);
        // false || false = false, so asserting it should be UNSAT
        assert_eq!(
            solver.check(),
            z3::SatResult::Unsat,
            "false || false should be false (unsat)"
        );
    }

    #[test]
    fn test_expr_to_z3_bool_logical_and() {
        let engine = test_engine();
        let solver = engine.solver.as_ref().unwrap();

        // true && true should be true
        let t_a = IrExpr::Const(LogicVec::from_u64(1, 1));
        let t_b = IrExpr::Const(LogicVec::from_u64(1, 1));
        let and_expr = IrExpr::BinaryOp(BinaryIrOp::LogicalAnd, Box::new(t_a), Box::new(t_b));

        let and_bool = engine.expr_to_z3_bool(&and_expr).unwrap();
        solver.assert(and_bool.not());
        assert_eq!(
            solver.check(),
            z3::SatResult::Unsat,
            "NOT(true && true) should be unsat"
        );
    }

    #[test]
    fn test_expr_to_z3_int_unsupported_op() {
        let engine = test_engine();

        // Div is not supported → should return None
        let lhs = IrExpr::Const(LogicVec::from_u64(10, 8));
        let rhs = IrExpr::Const(LogicVec::from_u64(2, 8));
        let div = IrExpr::BinaryOp(BinaryIrOp::Div, Box::new(lhs), Box::new(rhs));

        let result = engine.expr_to_z3_int(&div);
        assert!(result.is_none(), "Div should return None (unsupported)");
    }

    // ── k-Induction Tests ──

    fn test_engine_with_induction() -> FormalEngine {
        let cfg = FormalConfig {
            timeout: 10,
            induction: true,
            bound: 5,
            ..Default::default()
        };
        let mut engine = FormalEngine::new(cfg);
        engine.init();
        engine
    }

    #[test]
    fn test_induction_trivially_true() {
        // assert(1) is always true — induction should prove it
        let mut engine = test_engine_with_induction();
        let cond_true = IrExpr::Const(LogicVec::from_u64(1, 1));
        let assert_stmt = IrStmt::Assert {
            cond: cond_true,
            pass_stmt: vec![],
            fail_stmt: vec![],
            clock_event: None,
            disable_iff: None,
            sequence: None,
            line: 0,
            col: 0,
        };

        let design = IrDesign {
            top: IrModule {
                name: Symbol::from("test"),
                signals: vec![],
                inputs: vec![],
                outputs: vec![],
                inouts: vec![],
                processes: vec![Process::Combinational {
                    name: Symbol::from("always_comb"),
                    sensitivity: vec![],
                    body: vec![assert_stmt],
                }],
                sub_instances: vec![],
            },
            modules: HashMap::new(),
            classes: HashMap::new(),
            covergroups: vec![],
            dpi_imports: vec![],
            hier_signal_map: HashMap::new(),
            udp_defs: vec![],
            specify_items: vec![],
            timescale: None,
            source_file: None,
            source_lines: None,
            module_functions: HashMap::new(),
            pkg_scoped_consts: HashMap::new(),
            coverage_exclusions: Vec::new(),
            stmt_lines: HashMap::new(),
            net_aliases: HashMap::new(),
        };

        let results = engine.check_assertions_bmc(&design);
        assert_eq!(results.len(), 1);
        // Trivially true assertion should get InductiveProof with induction enabled
        assert!(
            matches!(
                results[0].1,
                FormalResult::InductiveProof(_) | FormalResult::Pass
            ),
            "assert(1) with induction should prove: got {:?}",
            results[0].1
        );
    }

    #[test]
    fn test_induction_signal_invariant() {
        // Design with sig[0] = 5 (constant assignment)
        // Assert: sig[0] == 5 — should always hold (inductive proof)
        let sig_assign = IrStmt::BlockingAssign {
            lhs: IrLValue::Signal(0, 8),
            rhs: IrExpr::Const(LogicVec::from_u64(5, 8)),
            delay: None,
        };
        let assert_cond = IrExpr::BinaryOp(
            BinaryIrOp::Eq,
            Box::new(IrExpr::Signal(0, 0)),
            Box::new(IrExpr::Const(LogicVec::from_u64(5, 8))),
        );
        let assert_stmt = IrStmt::Assert {
            cond: assert_cond,
            pass_stmt: vec![],
            fail_stmt: vec![],
            clock_event: None,
            disable_iff: None,
            sequence: None,
            line: 0,
            col: 0,
        };

        let mut engine = test_engine_with_induction();
        let design = IrDesign {
            top: IrModule {
                name: Symbol::from("test"),
                signals: vec![SignalInfo {
                    name: Symbol::from("sig"),
                    width: 8,
                    init_val: LogicVec::from_u64(5, 8),
                    ..Default::default()
                }],
                inputs: vec![0],
                outputs: vec![],
                inouts: vec![],
                processes: vec![Process::Combinational {
                    name: Symbol::from("always_comb"),
                    sensitivity: vec![],
                    body: vec![sig_assign, assert_stmt],
                }],
                sub_instances: vec![],
            },
            modules: HashMap::new(),
            classes: HashMap::new(),
            covergroups: vec![],
            dpi_imports: vec![],
            hier_signal_map: HashMap::new(),
            udp_defs: vec![],
            specify_items: vec![],
            timescale: None,
            source_file: None,
            source_lines: None,
            module_functions: HashMap::new(),
            pkg_scoped_consts: HashMap::new(),
            coverage_exclusions: Vec::new(),
            stmt_lines: HashMap::new(),
            net_aliases: HashMap::new(),
        };

        let results = engine.check_assertions_bmc(&design);
        assert!(!results.is_empty(), "Should have BMC result");
        // sig[0] == 5 is an invariant — should be provable by induction
        assert!(
            matches!(
                results[0].1,
                FormalResult::InductiveProof(_) | FormalResult::Pass
            ),
            "Signal invariant should prove: got {:?}",
            results[0].1
        );
    }

    #[test]
    fn test_k_induction_needs_depth_2() {
        // FORMAL-04: invariant yang TIDAK 1-inductive tapi TERBUKTI di k=2.
        //
        // Design: a' = b (combinational), b tanpa driver (frame: b'=b).
        // Init: a=0, b=1.
        // Invariant P = a | b — selalu benar pada state reachable.
        //
        // k=1: assume P(F0); check ¬P(F1). State palsu (a=1,b=0) memenuhi
        //      asumsi tapi transisi → (0,0) melanggar → SAT (spurious).
        // k=2: assume P(F0) ∧ P(F1) — rantai (1,0)→(0,0) terblokir karena
        //      F1=(0,0) melanggar asumsi → UNSAT → PROOF di k=2.
        let assign_a_eq_b = IrStmt::BlockingAssign {
            lhs: IrLValue::Signal(0, 1),
            rhs: IrExpr::Signal(1, 0),
            delay: None,
        };
        let assert_cond = IrExpr::BinaryOp(
            BinaryIrOp::LogicalOr,
            Box::new(IrExpr::Signal(0, 0)),
            Box::new(IrExpr::Signal(1, 0)),
        );
        let assert_stmt = IrStmt::Assert {
            cond: assert_cond,
            pass_stmt: vec![],
            fail_stmt: vec![],
            clock_event: None,
            disable_iff: None,
            sequence: None,
            line: 0,
            col: 0,
        };

        let mut engine = test_engine_with_induction();
        engine.config.max_k = 4;
        let design = IrDesign {
            top: IrModule {
                name: Symbol::from("test"),
                signals: vec![
                    SignalInfo {
                        name: Symbol::from("a"),
                        width: 1,
                        init_val: LogicVec::from_u64(0, 1),
                        ..Default::default()
                    },
                    SignalInfo {
                        name: Symbol::from("b"),
                        width: 1,
                        init_val: LogicVec::from_u64(1, 1),
                        ..Default::default()
                    },
                ],
                inputs: vec![],
                outputs: vec![0],
                inouts: vec![],
                processes: vec![Process::Combinational {
                    name: Symbol::from("always_comb"),
                    sensitivity: vec![],
                    body: vec![assign_a_eq_b, assert_stmt],
                }],
                sub_instances: vec![],
            },
            modules: HashMap::new(),
            classes: HashMap::new(),
            covergroups: vec![],
            dpi_imports: vec![],
            hier_signal_map: HashMap::new(),
            udp_defs: vec![],
            specify_items: vec![],
            timescale: None,
            source_file: None,
            source_lines: None,
            module_functions: HashMap::new(),
            pkg_scoped_consts: HashMap::new(),
            coverage_exclusions: Vec::new(),
            stmt_lines: HashMap::new(),
            net_aliases: HashMap::new(),
        };

        let results = engine.check_assertions_bmc(&design);
        assert_eq!(results.len(), 1);
        match &results[0].1 {
            FormalResult::InductiveProof(k) => {
                assert!(
                    *k >= 2,
                    "invariant ini butuh k>=2 (k=1 spurious): terbukti di k={}",
                    k
                );
            }
            other => panic!(
                "invariant a|b harus terbukti k-induction (k>=2): got {:?}",
                other
            ),
        }
    }

    #[test]
    fn test_assume_constrains_counterexample() {
        // FORMAL-10 tahap 1: `assume` dikonstrain pada semua depth —
        // counterexample hanya dilaporkan bila reachable DI BAWAH asumsi.
        //
        // Design: grant' = req (comb), assert(grant == 1).
        //   Tanpa asumsi : req unconstrained → req=0 → grant=0 → FAIL.
        //   assume(req==1): req dipaksa 1 di semua depth → PASS.
        let assign_grant = IrStmt::BlockingAssign {
            lhs: IrLValue::Signal(1, 1),
            rhs: IrExpr::Signal(0, 0),
            delay: None,
        };
        let mk_assert = |cond| IrStmt::Assert {
            cond,
            pass_stmt: vec![],
            fail_stmt: vec![],
            clock_event: None,
            disable_iff: None,
            sequence: None,
            line: 0,
            col: 0,
        };
        let mk_design = |processes: Vec<Process>| IrDesign {
            top: IrModule {
                name: Symbol::from("test"),
                signals: vec![
                    SignalInfo {
                        name: Symbol::from("req"),
                        width: 1,
                        ..Default::default()
                    },
                    SignalInfo {
                        name: Symbol::from("grant"),
                        width: 1,
                        ..Default::default()
                    },
                ],
                inputs: vec![],
                outputs: vec![1],
                inouts: vec![],
                processes,
                sub_instances: vec![],
            },
            modules: HashMap::new(),
            classes: HashMap::new(),
            covergroups: vec![],
            dpi_imports: vec![],
            hier_signal_map: HashMap::new(),
            udp_defs: vec![],
            specify_items: vec![],
            timescale: None,
            source_file: None,
            source_lines: None,
            module_functions: HashMap::new(),
            pkg_scoped_consts: HashMap::new(),
            coverage_exclusions: Vec::new(),
            stmt_lines: HashMap::new(),
            net_aliases: HashMap::new(),
        };

        // Tanpa asumsi → FAIL (counterexample).
        let mut engine = test_engine();
        let design = mk_design(vec![Process::Combinational {
            name: Symbol::from("comb"),
            sensitivity: vec![],
            body: vec![assign_grant.clone(), mk_assert(IrExpr::Signal(1, 0))],
        }]);
        let results = engine.check_assertions_bmc(&design);
        assert!(
            matches!(results[0].1, FormalResult::Counterexample(_)),
            "tanpa asumsi harus FAIL: got {:?}",
            results[0].1
        );

        // Dengan assume(req == 1) → PASS.
        let mut engine2 = test_engine();
        let assume_stmt = IrStmt::Assume {
            cond: IrExpr::BinaryOp(
                BinaryIrOp::Eq,
                Box::new(IrExpr::Signal(0, 0)),
                Box::new(IrExpr::Const(LogicVec::from_u64(1, 1))),
            ),
            pass_stmt: vec![],
            fail_stmt: vec![],
            clock_event: None,
            disable_iff: None,
            sequence: None,
            line: 0,
            col: 0,
        };
        let design2 = mk_design(vec![Process::Combinational {
            name: Symbol::from("comb"),
            sensitivity: vec![],
            body: vec![assign_grant, assume_stmt, mk_assert(IrExpr::Signal(1, 0))],
        }]);
        let results2 = engine2.check_assertions_bmc(&design2);
        assert!(
            matches!(results2[0].1, FormalResult::Pass),
            "dengan asumsi req==1 harus PASS: got {:?}",
            results2[0].1
        );
    }

    #[test]
    fn test_connectivity_chain_and_isolated() {
        // FORMAL-13: jalur kombinational.
        //   b = a; c = b | a; d = c
        //   check(a, c) → connected, 1 hop (edge langsung via OR)
        //   check(a, d) → connected, 2 hop [a, c, d]
        //   check(c, a) → NOT connected
        //   check(zz, a) → error sinyal tak dikenal
        use crate::connectivity::check_connectivity;
        let assigns = vec![
            IrStmt::BlockingAssign {
                lhs: IrLValue::Signal(1, 1),
                rhs: IrExpr::Signal(0, 0),
                delay: None,
            },
            IrStmt::BlockingAssign {
                lhs: IrLValue::Signal(2, 1),
                rhs: IrExpr::BinaryOp(
                    BinaryIrOp::BitOr,
                    Box::new(IrExpr::Signal(1, 0)),
                    Box::new(IrExpr::Signal(0, 0)),
                ),
                delay: None,
            },
            IrStmt::BlockingAssign {
                lhs: IrLValue::Signal(3, 1),
                rhs: IrExpr::Signal(2, 0),
                delay: None,
            },
        ];
        let design = IrDesign {
            top: IrModule {
                name: Symbol::from("t"),
                signals: vec![
                    SignalInfo {
                        name: Symbol::from("a"),
                        width: 1,
                        ..Default::default()
                    },
                    SignalInfo {
                        name: Symbol::from("b"),
                        width: 1,
                        ..Default::default()
                    },
                    SignalInfo {
                        name: Symbol::from("c"),
                        width: 1,
                        ..Default::default()
                    },
                    SignalInfo {
                        name: Symbol::from("d"),
                        width: 1,
                        ..Default::default()
                    },
                ],
                inputs: vec![],
                outputs: vec![],
                inouts: vec![],
                processes: vec![Process::Combinational {
                    name: Symbol::from("comb"),
                    sensitivity: vec![],
                    body: assigns,
                }],
                sub_instances: vec![],
            },
            modules: HashMap::new(),
            classes: HashMap::new(),
            covergroups: vec![],
            dpi_imports: vec![],
            hier_signal_map: HashMap::new(),
            udp_defs: vec![],
            specify_items: vec![],
            timescale: None,
            source_file: None,
            source_lines: None,
            module_functions: HashMap::new(),
            pkg_scoped_consts: HashMap::new(),
            coverage_exclusions: Vec::new(),
            stmt_lines: HashMap::new(),
            net_aliases: HashMap::new(),
        };

        let pairs = vec![
            ("a".to_string(), "c".to_string()),
            ("c".to_string(), "a".to_string()),
            ("a".to_string(), "d".to_string()),
            ("zz".to_string(), "a".to_string()),
        ];
        let r = check_connectivity(&design, &pairs);
        assert_eq!(r.len(), 4);

        // a→c: connected langsung 1 hop (c = b|a memuat edge a→c).
        assert!(r[0].connected);
        assert_eq!(r[0].path_len, Some(1));
        assert_eq!(r[0].path, vec!["a", "c"]);

        // c→a: tidak ada jalur mundur.
        assert!(!r[1].connected);

        // a→d: jalur 2 hop [a, c, d].
        assert!(r[2].connected);
        assert_eq!(r[2].path_len, Some(2));
        assert_eq!(r[2].path, vec!["a", "c", "d"]);

        // Sinyal tidak dikenal → error.
        assert!(r[3].error.is_some());
        assert!(!r[3].connected);
    }

    #[test]
    fn test_connectivity_seq_nba() {
        // FORMAL-13 tahap 2a: NBA Sequential `q <= d` hanya terlihat
        // via including_seq (1 hop lintas cycle), bukan kombinational.
        use crate::connectivity::{check_connectivity, check_connectivity_including_seq};
        let body = vec![IrStmt::NonBlockingAssign {
            lhs: IrLValue::Signal(1, 1),
            rhs: IrExpr::Signal(0, 0),
            delay: None,
        }];
        let design = IrDesign {
            top: IrModule {
                name: Symbol::from("t"),
                signals: vec![
                    SignalInfo { name: Symbol::from("d"), width: 1, ..Default::default() },
                    SignalInfo { name: Symbol::from("q"), width: 1, ..Default::default() },
                ],
                inputs: vec![],
                outputs: vec![],
                inouts: vec![],
                processes: vec![Process::Sequential {
                    name: Symbol::from("seq"),
                    clock: ClockEdge::PosEdge(0),
                    reset: None,
                    body,
                    iff: None,
                }],
                sub_instances: vec![],
            },
            modules: HashMap::new(),
            classes: HashMap::new(),
            covergroups: vec![],
            dpi_imports: vec![],
            hier_signal_map: HashMap::new(),
            udp_defs: vec![],
            specify_items: vec![],
            timescale: None,
            source_file: None,
            source_lines: None,
            module_functions: HashMap::new(),
            pkg_scoped_consts: HashMap::new(),
            coverage_exclusions: Vec::new(),
            stmt_lines: HashMap::new(),
            net_aliases: HashMap::new(),
        };
        let pairs = vec![("d".to_string(), "q".to_string())];
        let comb = check_connectivity(&design, &pairs);
        assert!(!comb[0].connected, "kombinational harus abaikan NBA");
        let seq = check_connectivity_including_seq(&design, &pairs);
        assert!(seq[0].connected);
        assert_eq!(seq[0].path_len, Some(1));
        assert_eq!(seq[0].path, vec!["d", "q"]);
        // BlockingAssign di Sequential tetap diabaikan (hindari false positive).
        let mut design_b = design.clone();
        if let Process::Sequential { body, .. } = &mut design_b.top.processes[0] {
            body[0] = IrStmt::BlockingAssign {
                lhs: IrLValue::Signal(1, 1),
                rhs: IrExpr::Signal(0, 0),
                delay: None,
            };
        }
        let seq_b = check_connectivity_including_seq(&design_b, &pairs);
        assert!(!seq_b[0].connected, "blocking sequential harus diabaikan");
    }

    #[test]
    fn test_z3_bvadd_simple() {
        // Minimal Z3 test: a=0, b=a+1. Assert b < 0 (should be unsat).
        // Then assert b = 1 (should be sat).
        let engine = test_engine();
        let solver = engine.solver.as_ref().unwrap();

        let a = z3::ast::BV::new_const("a", 8);
        let b = z3::ast::BV::new_const("b", 8);
        let zero = z3::ast::BV::from_u64(0, 8);
        let one = z3::ast::BV::from_u64(1, 8);

        solver.assert(a.eq(&zero)); // a == 0
        solver.assert(b.eq(a.bvadd(&one))); // b == a + 1

        // Check: b == 1?
        solver.push();
        solver.assert(b.eq(&one));
        assert_eq!(
            solver.check(),
            z3::SatResult::Sat,
            "a=0, b=a+1 → b=1 should be SAT"
        );
        solver.pop(1);

        // Check: b < 0? (should be unsat)
        solver.push();
        solver.assert(b.bvslt(&zero));
        assert_eq!(
            solver.check(),
            z3::SatResult::Unsat,
            "a=0, b=a+1 → b<0 should be UNSAT"
        );
        solver.pop(1);

        // Check: b > 2? (should be unsat)
        solver.push();
        let two = z3::ast::BV::from_u64(2, 8);
        solver.assert(b.bvsgt(&two));
        assert_eq!(
            solver.check(),
            z3::SatResult::Unsat,
            "a=0, b=a+1 → b>2 should be UNSAT"
        );
        solver.pop(1);

        // Check: b == 1 is forced
        solver.push();
        let one_bv = z3::ast::BV::from_u64(1, 8);
        solver.assert(b.ne(&one_bv));
        assert_eq!(
            solver.check(),
            z3::SatResult::Unsat,
            "a=0, b=a+1 ¬(b=1) should be UNSAT"
        );
        solver.pop(1);
    }

    #[test]
    fn test_induction_counterexample_signal_zero() {
        // counter < 0 — fails at depth 0 (init=0, 0 < 0 is false)
        let sig_assign = IrStmt::BlockingAssign {
            lhs: IrLValue::Signal(0, 8),
            rhs: IrExpr::BinaryOp(
                BinaryIrOp::Add,
                Box::new(IrExpr::Signal(0, 0)),
                Box::new(IrExpr::Const(LogicVec::from_u64(1, 8))),
            ),
            delay: None,
        };
        let assert_cond = IrExpr::BinaryOp(
            BinaryIrOp::Lt,
            Box::new(IrExpr::Signal(0, 0)),
            Box::new(IrExpr::Const(LogicVec::from_u64(0, 8))),
        );
        let assert_stmt = IrStmt::Assert {
            cond: assert_cond,
            pass_stmt: vec![],
            fail_stmt: vec![],
            clock_event: None,
            disable_iff: None,
            sequence: None,
            line: 0,
            col: 0,
        };

        let mut engine = test_engine_with_induction();
        let design = IrDesign {
            top: IrModule {
                name: Symbol::from("test"),
                signals: vec![SignalInfo {
                    name: Symbol::from("counter"),
                    width: 8,
                    init_val: LogicVec::from_u64(0, 8),
                    ..Default::default()
                }],
                inputs: vec![0],
                outputs: vec![],
                inouts: vec![],
                processes: vec![Process::Combinational {
                    name: Symbol::from("always_comb"),
                    sensitivity: vec![],
                    body: vec![sig_assign, assert_stmt],
                }],
                sub_instances: vec![],
            },
            modules: HashMap::new(),
            classes: HashMap::new(),
            covergroups: vec![],
            dpi_imports: vec![],
            hier_signal_map: HashMap::new(),
            udp_defs: vec![],
            specify_items: vec![],
            timescale: None,
            source_file: None,
            source_lines: None,
            module_functions: HashMap::new(),
            pkg_scoped_consts: HashMap::new(),
            coverage_exclusions: Vec::new(),
            stmt_lines: HashMap::new(),
            net_aliases: HashMap::new(),
        };

        let results = engine.check_assertions_bmc(&design);
        assert!(!results.is_empty(), "Should have BMC result");
        assert!(
            matches!(results[0].1, FormalResult::Counterexample(d) if d == 0),
            "counter < 0 should fail at depth 0: got {:?}",
            results[0].1
        );
    }

    #[test]
    fn test_induction_counterexample_signal_three() {
        // counter < 3 — holds at depths 0..2, fails at depth 3
        // This verifies that both the init constraint AND the transition
        // relation (counter = counter + 1) work correctly.
        // With push/pop fix, ¬P(0)=false doesn't poison depth 3 check.
        let sig_assign = IrStmt::BlockingAssign {
            lhs: IrLValue::Signal(0, 8),
            rhs: IrExpr::BinaryOp(
                BinaryIrOp::Add,
                Box::new(IrExpr::Signal(0, 0)),
                Box::new(IrExpr::Const(LogicVec::from_u64(1, 8))),
            ),
            delay: None,
        };
        let assert_cond = IrExpr::BinaryOp(
            BinaryIrOp::Lt,
            Box::new(IrExpr::Signal(0, 0)),
            Box::new(IrExpr::Const(LogicVec::from_u64(3, 8))),
        );
        let assert_stmt = IrStmt::Assert {
            cond: assert_cond,
            pass_stmt: vec![],
            fail_stmt: vec![],
            clock_event: None,
            disable_iff: None,
            sequence: None,
            line: 0,
            col: 0,
        };

        let mut engine = test_engine_with_induction();
        let design = IrDesign {
            top: IrModule {
                name: Symbol::from("test"),
                signals: vec![SignalInfo {
                    name: Symbol::from("counter"),
                    width: 8,
                    init_val: LogicVec::from_u64(0, 8),
                    ..Default::default()
                }],
                inputs: vec![0],
                outputs: vec![],
                inouts: vec![],
                processes: vec![Process::Combinational {
                    name: Symbol::from("always_comb"),
                    sensitivity: vec![],
                    body: vec![sig_assign, assert_stmt],
                }],
                sub_instances: vec![],
            },
            modules: HashMap::new(),
            classes: HashMap::new(),
            covergroups: vec![],
            dpi_imports: vec![],
            hier_signal_map: HashMap::new(),
            udp_defs: vec![],
            specify_items: vec![],
            timescale: None,
            source_file: None,
            source_lines: None,
            module_functions: HashMap::new(),
            pkg_scoped_consts: HashMap::new(),
            coverage_exclusions: Vec::new(),
            stmt_lines: HashMap::new(),
            net_aliases: HashMap::new(),
        };

        let results = engine.check_assertions_bmc(&design);
        assert!(!results.is_empty(), "Should have BMC result");
        // counter = 0→1→2→3, so at depth 3: 3 < 3 is false → Counterexample(3)
        assert!(
            matches!(results[0].1, FormalResult::Counterexample(d) if d == 3),
            "counter < 3 should fail at depth 3: got {:?}",
            results[0].1
        );
    }

    // ── FORMAL-15: Unreachable Assertion Detection Tests ──

    #[test]
    fn test_unreachable_assertion_always_true() {
        // assert(1) is trivially always true — unreachable
        let cond_true = IrExpr::Const(LogicVec::from_u64(1, 1));
        let assert_stmt = IrStmt::Assert {
            cond: cond_true,
            pass_stmt: vec![],
            fail_stmt: vec![],
            clock_event: None,
            disable_iff: None,
            sequence: None,
            line: 0,
            col: 0,
        };

        let design = IrDesign {
            top: IrModule {
                name: Symbol::from("test"),
                signals: vec![],
                inputs: vec![],
                outputs: vec![],
                inouts: vec![],
                processes: vec![Process::Combinational {
                    name: Symbol::from("always_comb"),
                    sensitivity: vec![],
                    body: vec![assert_stmt],
                }],
                sub_instances: vec![],
            },
            modules: HashMap::new(),
            classes: HashMap::new(),
            covergroups: vec![],
            dpi_imports: vec![],
            hier_signal_map: HashMap::new(),
            udp_defs: vec![],
            specify_items: vec![],
            timescale: None,
            source_file: None,
            source_lines: None,
            module_functions: HashMap::new(),
            pkg_scoped_consts: HashMap::new(),
            coverage_exclusions: Vec::new(),
            stmt_lines: HashMap::new(),
            net_aliases: HashMap::new(),
        };

        let mut engine = test_engine();
        let results = engine.detect_unreachable_assertions(&design);
        assert_eq!(results.len(), 1, "Should detect 1 unreachable assertion");
        assert!(results[0].always_true, "assert(1) should be always true");
        assert!(!results[0].can_violate, "assert(1) should never violate");
    }

    #[test]
    fn test_unreachable_assertion_reachable() {
        // assert(signal[0]) — signal is unconstrained, can be 0 or 1
        // So ¬P is SAT → assertion is reachable (can violate)
        let assert_cond = IrExpr::Signal(0, 0);
        let assert_stmt = IrStmt::Assert {
            cond: assert_cond,
            pass_stmt: vec![],
            fail_stmt: vec![],
            clock_event: None,
            disable_iff: None,
            sequence: None,
            line: 0,
            col: 0,
        };

        let design = IrDesign {
            top: IrModule {
                name: Symbol::from("test"),
                signals: vec![SignalInfo {
                    name: Symbol::from("sig"),
                    width: 1,
                    ..Default::default()
                }],
                inputs: vec![0],
                outputs: vec![],
                inouts: vec![],
                processes: vec![Process::Combinational {
                    name: Symbol::from("always_comb"),
                    sensitivity: vec![],
                    body: vec![assert_stmt],
                }],
                sub_instances: vec![],
            },
            modules: HashMap::new(),
            classes: HashMap::new(),
            covergroups: vec![],
            dpi_imports: vec![],
            hier_signal_map: HashMap::new(),
            udp_defs: vec![],
            specify_items: vec![],
            timescale: None,
            source_file: None,
            source_lines: None,
            module_functions: HashMap::new(),
            pkg_scoped_consts: HashMap::new(),
            coverage_exclusions: Vec::new(),
            stmt_lines: HashMap::new(),
            net_aliases: HashMap::new(),
        };

        let mut engine = test_engine();
        let results = engine.detect_unreachable_assertions(&design);
        assert_eq!(results.len(), 1);
        assert!(
            !results[0].always_true,
            "unconstrained signal should not be always true"
        );
        assert!(
            results[0].can_violate,
            "unconstrained signal should be reachable"
        );
    }

    #[test]
    fn test_unreachable_assertion_no_assertions() {
        let design = IrDesign {
            top: IrModule {
                name: Symbol::from("test"),
                signals: vec![],
                inputs: vec![],
                outputs: vec![],
                inouts: vec![],
                processes: vec![],
                sub_instances: vec![],
            },
            modules: HashMap::new(),
            classes: HashMap::new(),
            covergroups: vec![],
            dpi_imports: vec![],
            hier_signal_map: HashMap::new(),
            udp_defs: vec![],
            specify_items: vec![],
            timescale: None,
            source_file: None,
            source_lines: None,
            module_functions: HashMap::new(),
            pkg_scoped_consts: HashMap::new(),
            coverage_exclusions: Vec::new(),
            stmt_lines: HashMap::new(),
            net_aliases: HashMap::new(),
        };

        let mut engine = test_engine();
        let results = engine.detect_unreachable_assertions(&design);
        assert!(results.is_empty(), "No assertions → no unreachable results");
    }
}
