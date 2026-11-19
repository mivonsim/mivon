//! FORMAL-09: Abstract Interpretation Framework.
//!
//! Approximates signal values through abstract domains to detect
//! potential issues without full simulation. Used for:
//! - Constant propagation analysis
//! - Reachability analysis
//! - Potential bug detection (uninitialized signals, dead code)
//!
//! Abstract domains:
//! - ConstDomain: exact constant values
//! - IntervalDomain: value ranges [min, max]
//! - SignDomain: positive/negative/zero

use mivon_ir::{IrDesign, IrExpr, IrStmt, LogicVal, Process, SignalId};
use std::collections::HashMap;

/// Abstract value domain for signal analysis.
#[derive(Debug, Clone, PartialEq)]
pub enum AbsValue {
    /// Exact constant value
    Const(u64),
    /// Value range [min, max] (inclusive)
    Interval { min: u64, max: u64 },
    /// Unknown / unconstrained
    Top,
    /// Unreachable (contradiction)
    Bottom,
}

impl AbsValue {
    /// Join (least upper bound) of two abstract values.
    pub fn join(&self, other: &AbsValue) -> AbsValue {
        match (self, other) {
            (AbsValue::Const(a), AbsValue::Const(b)) => {
                if a == b {
                    AbsValue::Const(*a)
                } else {
                    AbsValue::Interval {
                        min: (*a).min(*b),
                        max: (*a).max(*b),
                    }
                }
            }
            (AbsValue::Const(v), AbsValue::Interval { min, max })
            | (AbsValue::Interval { min, max }, AbsValue::Const(v)) => AbsValue::Interval {
                min: (*v).min(*min),
                max: (*v).max(*max),
            },
            (
                AbsValue::Interval {
                    min: a_min,
                    max: a_max,
                },
                AbsValue::Interval {
                    min: b_min,
                    max: b_max,
                },
            ) => AbsValue::Interval {
                min: (*a_min).min(*b_min),
                max: (*a_max).max(*b_max),
            },
            (AbsValue::Bottom, x) | (x, AbsValue::Bottom) => x.clone(),
            _ => AbsValue::Top,
        }
    }

    /// Widening operator for convergence acceleration.
    pub fn widen(&self, other: &AbsValue) -> AbsValue {
        match (self, other) {
            (
                AbsValue::Interval {
                    min: a_min,
                    max: a_max,
                },
                AbsValue::Interval {
                    min: b_min,
                    max: b_max,
                },
            ) => {
                if b_min < a_min || b_max > a_max {
                    // Unbounded widening (conservative)
                    AbsValue::Top
                } else {
                    other.clone()
                }
            }
            _ => other.clone(),
        }
    }

    /// Narrowing operator for precision recovery.
    pub fn narrow(&self, other: &AbsValue) -> AbsValue {
        match (self, other) {
            (
                AbsValue::Interval {
                    min: a_min,
                    max: a_max,
                },
                AbsValue::Interval {
                    min: b_min,
                    max: b_max,
                },
            ) => AbsValue::Interval {
                min: (*a_min).max(*b_min),
                max: (*a_max).min(*b_max),
            },
            _ => other.clone(),
        }
    }

    /// Check if this value is definitely zero.
    pub fn is_definitely_zero(&self) -> bool {
        matches!(self, AbsValue::Const(v) if *v == 0)
    }

    /// Check if this value is definitely non-zero.
    pub fn is_definitely_nonzero(&self) -> bool {
        matches!(self, AbsValue::Const(v) if *v != 0)
    }

    /// Check if this value could be zero.
    pub fn may_be_zero(&self) -> bool {
        match self {
            AbsValue::Const(v) => *v == 0,
            AbsValue::Interval { min, .. } => *min == 0,
            AbsValue::Top => true,
            AbsValue::Bottom => false,
        }
    }
}

/// Abstract state: maps signal IDs to abstract values.
#[derive(Debug, Clone)]
pub struct AbsState {
    pub values: HashMap<SignalId, AbsValue>,
}

impl AbsState {
    pub fn new() -> Self {
        AbsState {
            values: HashMap::new(),
        }
    }

    /// Join two abstract states (pointwise join).
    pub fn join(&self, other: &AbsState) -> AbsState {
        let mut result = AbsState::new();
        // Start with self's values
        for (id, val) in &self.values {
            result.values.insert(*id, val.clone());
        }
        // Join with other's values
        for (id, val) in &other.values {
            let joined = result
                .values
                .get(id)
                .map(|existing| existing.join(val))
                .unwrap_or_else(|| val.clone());
            result.values.insert(*id, joined);
        }
        result
    }

    /// Get abstract value for a signal.
    pub fn get(&self, id: SignalId) -> &AbsValue {
        self.values.get(&id).unwrap_or(&AbsValue::Top)
    }

    /// Set abstract value for a signal.
    pub fn set(&mut self, id: SignalId, val: AbsValue) {
        self.values.insert(id, val);
    }
}

impl Default for AbsState {
    fn default() -> Self {
        Self::new()
    }
}

/// Abstract interpreter for RTL designs.
pub struct AbstractInterpreter {
    pub max_iterations: usize,
}

impl AbstractInterpreter {
    pub fn new() -> Self {
        AbstractInterpreter {
            max_iterations: 128,
        }
    }

    /// Analyze a design and return abstract state at each process.
    pub fn analyze(&self, design: &IrDesign) -> Vec<AbsState> {
        let mut results = Vec::new();

        for process in &design.top.processes {
            let mut state = AbsState::new();

            // Initialize signals with their initial values
            for (id, sig) in design.top.signals.iter().enumerate() {
                let init = &sig.init_val;
                if init.bits.iter().all(|b| *b == LogicVal::Zero) {
                    state.set(id, AbsValue::Const(0));
                } else if init.bits.iter().all(|b| *b == LogicVal::One) {
                    let val = init.to_u64();
                    state.set(id, AbsValue::Const(val));
                }
            }

            // Analyze process body
            match process {
                Process::Combinational { body, .. }
                | Process::Sequential { body, .. }
                | Process::Initial { body, .. }
                | Process::CombReactive { body, .. } => {
                    self.analyze_stmts(body, &mut state);
                }
                _ => {}
            }

            results.push(state);
        }

        results
    }

    /// Analyze a design and return abstract state keyed by signal NAME.
    ///
    /// Joins the per-process abstract states for each signal. Signals that
    /// are never assigned in any process body remain `AbsValue::Top`
    /// (unknown) — these are typically design inputs or potentially
    /// uninitialized registers.
    pub fn analyze_design(&self, design: &IrDesign) -> HashMap<String, AbsValue> {
        let mut result: HashMap<String, AbsValue> = HashMap::new();

        // Initialize every signal as Top (unknown), then apply init values
        // and per-process assignments on top.
        for sig in &design.top.signals {
            let init = if sig.init_val.bits.iter().all(|b| *b == LogicVal::Zero) {
                AbsValue::Const(0)
            } else if sig.init_val.bits.iter().all(|b| *b == LogicVal::One) {
                AbsValue::Const(sig.init_val.to_u64())
            } else {
                AbsValue::Top
            };
            result.insert(sig.name.as_str().to_string(), init);
        }

        for state in self.analyze(design) {
            for (id, val) in &state.values {
                if let Some(sig) = design.top.signals.get(*id) {
                    let name = sig.name.as_str().to_string();
                    result
                        .entry(name)
                        .and_modify(|existing| *existing = existing.join(val))
                        .or_insert(val.clone());
                }
            }
        }

        result
    }

    /// Return names of signals whose abstract value is `Top` (unknown)
    /// after analysis — i.e. signals that are read but never assigned a
    /// concrete value, or design inputs (which are legitimately unknown).
    /// Callers can filter by `SignalKind` if they want to exclude inputs.
    pub fn unknown_signals(&self, design: &IrDesign) -> Vec<String> {
        let analyzed = self.analyze_design(design);
        let mut unknown: Vec<String> = analyzed
            .into_iter()
            .filter(|(_, val)| matches!(val, AbsValue::Top))
            .map(|(name, _)| name)
            .collect();
        unknown.sort();
        unknown
    }

    /// Analyze a list of statements abstractly.
    fn analyze_stmts(&self, stmts: &[IrStmt], state: &mut AbsState) {
        for stmt in stmts {
            self.analyze_stmt(stmt, state);
        }
    }

    /// Analyze a single statement abstractly.
    fn analyze_stmt(&self, stmt: &IrStmt, state: &mut AbsState) {
        match stmt {
            IrStmt::Block { stmts } | IrStmt::NamedBlock { stmts, .. } => {
                self.analyze_stmts(stmts, state);
            }
            IrStmt::NonBlockingAssign { lhs, rhs, .. }
            | IrStmt::BlockingAssign { lhs, rhs, .. } => {
                // Evaluate RHS abstractly
                let rhs_val = self.eval_expr(rhs, state);
                // Update LHS signal
                if let mivon_ir::IrLValue::Signal(id, _) = lhs {
                    state.set(*id, rhs_val);
                }
            }
            IrStmt::If {
                cond,
                true_branch,
                false_branch,
            } => {
                // Abstract interpretation of condition
                let cond_val = self.eval_expr(cond, state);
                match cond_val {
                    AbsValue::Const(0) => {
                        // Condition definitely false
                        self.analyze_stmts(false_branch, state);
                    }
                    AbsValue::Const(_) => {
                        // Condition definitely true
                        self.analyze_stmts(true_branch, state);
                    }
                    _ => {
                        // Unknown condition — analyze both branches and join
                        let mut true_state = state.clone();
                        let mut false_state = state.clone();
                        self.analyze_stmts(true_branch, &mut true_state);
                        self.analyze_stmts(false_branch, &mut false_state);
                        *state = state.join(&true_state.join(&false_state));
                    }
                }
            }
            IrStmt::Case {
                expr,
                items,
                default,
                ..
            } => {
                let _ = self.eval_expr(expr, state);
                // Conservative: join all branches
                let mut joined = state.clone();
                for item in items {
                    let mut branch_state = state.clone();
                    self.analyze_stmts(&item.body, &mut branch_state);
                    joined = joined.join(&branch_state);
                }
                if !default.is_empty() {
                    let mut default_state = state.clone();
                    self.analyze_stmts(default, &mut default_state);
                    joined = joined.join(&default_state);
                }
                *state = joined;
            }
            _ => {}
        }
    }

    /// Evaluate an expression abstractly.
    fn eval_expr(&self, expr: &IrExpr, state: &AbsState) -> AbsValue {
        match expr {
            IrExpr::Const(v) => AbsValue::Const(v.to_u64()),
            IrExpr::Signal(id, _) => state.get(*id).clone(),
            IrExpr::BinaryOp(op, lhs, rhs) => {
                let l = self.eval_expr(lhs, state);
                let r = self.eval_expr(rhs, state);
                self.eval_binop(op, &l, &r)
            }
            IrExpr::UnaryOp(op, operand) => {
                let v = self.eval_expr(operand, state);
                self.eval_unop(op, &v)
            }
            _ => AbsValue::Top,
        }
    }

    /// Evaluate binary operation abstractly.
    fn eval_binop(&self, op: &mivon_ir::BinaryIrOp, l: &AbsValue, r: &AbsValue) -> AbsValue {
        use mivon_ir::BinaryIrOp;
        match (l, r) {
            (AbsValue::Const(a), AbsValue::Const(b)) => {
                let result = match op {
                    BinaryIrOp::Add => a.wrapping_add(*b),
                    BinaryIrOp::Sub => a.wrapping_sub(*b),
                    BinaryIrOp::Mul => a.wrapping_mul(*b),
                    BinaryIrOp::Div => a.checked_div(*b).unwrap_or(0),
                    BinaryIrOp::Mod => a.checked_rem(*b).unwrap_or(0),
                    BinaryIrOp::BitAnd => a & b,
                    BinaryIrOp::BitOr => a | b,
                    BinaryIrOp::BitXor => a ^ b,
                    BinaryIrOp::Eq => (*a == *b) as u64,
                    BinaryIrOp::Neq => (*a != *b) as u64,
                    BinaryIrOp::Lt => (*a < *b) as u64,
                    BinaryIrOp::Le => (*a <= *b) as u64,
                    BinaryIrOp::Gt => (*a > *b) as u64,
                    BinaryIrOp::Ge => (*a >= *b) as u64,
                    BinaryIrOp::Shl => a << b.min(&63),
                    BinaryIrOp::Shr => a >> b.min(&63),
                    _ => 0, // Conservative fallback for unhandled ops
                };
                AbsValue::Const(result)
            }
            // Interval arithmetic (conservative)
            (
                AbsValue::Interval {
                    min: l_min,
                    max: l_max,
                },
                AbsValue::Interval {
                    min: r_min,
                    max: r_max,
                },
            ) => match op {
                BinaryIrOp::Add => AbsValue::Interval {
                    min: l_min.wrapping_add(*r_min),
                    max: l_max.wrapping_add(*r_max),
                },
                BinaryIrOp::Sub => AbsValue::Interval {
                    min: l_min.wrapping_sub(*r_max),
                    max: l_max.wrapping_sub(*r_min),
                },
                _ => AbsValue::Top,
            },
            _ => AbsValue::Top,
        }
    }

    /// Evaluate unary operation abstractly.
    fn eval_unop(&self, op: &mivon_ir::UnaryIrOp, v: &AbsValue) -> AbsValue {
        use mivon_ir::UnaryIrOp;
        match v {
            AbsValue::Const(a) => {
                let result = match op {
                    UnaryIrOp::Not => (*a == 0) as u64,
                    UnaryIrOp::BitNot => !a,
                    UnaryIrOp::Minus => a.wrapping_neg(),
                    _ => *a,
                };
                AbsValue::Const(result)
            }
            _ => AbsValue::Top,
        }
    }
}

impl Default for AbstractInterpreter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mivon_ir::{ClockEdge, IrModule, LogicVec};

    #[test]
    fn test_abs_value_join_const() {
        let a = AbsValue::Const(5);
        let b = AbsValue::Const(5);
        assert_eq!(a.join(&b), AbsValue::Const(5));

        let c = AbsValue::Const(10);
        assert_eq!(a.join(&c), AbsValue::Interval { min: 5, max: 10 });
    }

    #[test]
    fn test_abs_value_join_interval() {
        let a = AbsValue::Interval { min: 1, max: 5 };
        let b = AbsValue::Interval { min: 3, max: 8 };
        assert_eq!(a.join(&b), AbsValue::Interval { min: 1, max: 8 });
    }

    #[test]
    fn test_abs_value_join_bottom() {
        let a = AbsValue::Bottom;
        let b = AbsValue::Const(5);
        assert_eq!(a.join(&b), AbsValue::Const(5));
    }

    #[test]
    fn test_abs_value_join_top() {
        let a = AbsValue::Const(5);
        let b = AbsValue::Top;
        assert_eq!(a.join(&b), AbsValue::Top);
    }

    #[test]
    fn test_abs_value_is_definitely_zero() {
        assert!(AbsValue::Const(0).is_definitely_zero());
        assert!(!AbsValue::Const(1).is_definitely_zero());
        assert!(!AbsValue::Top.is_definitely_zero());
        assert!(!AbsValue::Bottom.is_definitely_zero());
    }

    #[test]
    fn test_abs_value_may_be_zero() {
        assert!(AbsValue::Const(0).may_be_zero());
        assert!(!AbsValue::Const(1).may_be_zero());
        assert!(AbsValue::Interval { min: 0, max: 5 }.may_be_zero());
        assert!(!AbsValue::Interval { min: 1, max: 5 }.may_be_zero());
        assert!(AbsValue::Top.may_be_zero());
        assert!(!AbsValue::Bottom.may_be_zero());
    }

    #[test]
    fn test_eval_binop_const() {
        let interp = AbstractInterpreter::new();
        let a = AbsValue::Const(10);
        let b = AbsValue::Const(3);
        assert_eq!(
            interp.eval_binop(&mivon_ir::BinaryIrOp::Add, &a, &b),
            AbsValue::Const(13)
        );
        assert_eq!(
            interp.eval_binop(&mivon_ir::BinaryIrOp::Sub, &a, &b),
            AbsValue::Const(7)
        );
        assert_eq!(
            interp.eval_binop(&mivon_ir::BinaryIrOp::Mul, &a, &b),
            AbsValue::Const(30)
        );
        assert_eq!(
            interp.eval_binop(&mivon_ir::BinaryIrOp::Eq, &a, &b),
            AbsValue::Const(0)
        );
        assert_eq!(
            interp.eval_binop(&mivon_ir::BinaryIrOp::Eq, &a, &a),
            AbsValue::Const(1)
        );
    }

    #[test]
    fn test_eval_binop_interval() {
        let interp = AbstractInterpreter::new();
        let a = AbsValue::Interval { min: 1, max: 5 };
        let b = AbsValue::Interval { min: 2, max: 3 };
        // Addition: [1+2, 5+3] = [3, 8]
        assert_eq!(
            interp.eval_binop(&mivon_ir::BinaryIrOp::Add, &a, &b),
            AbsValue::Interval { min: 3, max: 8 }
        );
    }

    #[test]
    fn test_eval_unop_const() {
        let interp = AbstractInterpreter::new();
        let v = AbsValue::Const(0xFF);
        assert_eq!(
            interp.eval_unop(&mivon_ir::UnaryIrOp::BitNot, &v),
            AbsValue::Const(!0xFFu64)
        );
        assert_eq!(
            interp.eval_unop(&mivon_ir::UnaryIrOp::Not, &v),
            AbsValue::Const(0)
        );
        assert_eq!(
            interp.eval_unop(&mivon_ir::UnaryIrOp::Not, &AbsValue::Const(0)),
            AbsValue::Const(1)
        );
    }

    #[test]
    fn test_abs_state_join() {
        let mut s1 = AbsState::new();
        s1.set(0, AbsValue::Const(5));
        s1.set(1, AbsValue::Const(10));

        let mut s2 = AbsState::new();
        s2.set(0, AbsValue::Const(7));
        s2.set(2, AbsValue::Const(20));

        let joined = s1.join(&s2);
        assert_eq!(joined.get(0), &AbsValue::Interval { min: 5, max: 7 });
        assert_eq!(joined.get(1), &AbsValue::Const(10));
        assert_eq!(joined.get(2), &AbsValue::Const(20));
    }

    #[test]
    fn test_analyze_design_named_signals() {
        use mivon_core::intern::Symbol;
        use mivon_ir::{BinaryIrOp, IrExpr, IrLValue, IrStmt, LogicVal, Process, SignalInfo, SignalKind};

        let clk_init = LogicVec::from_u64(0, 1);
        let x_init = LogicVec::from_u64(0, 8);
        let unused_init = LogicVec { bits: vec![LogicVal::X; 8], width: 8 };

        let design = IrDesign {
            top: IrModule {
                name: Symbol::from("test"),
                signals: vec![
                    SignalInfo {
                        name: Symbol::from("clk"),
                        width: 1,
                        kind: SignalKind::Input,
                        init_val: clk_init,
                        ..Default::default()
                    },
                    SignalInfo {
                        name: Symbol::from("x"),
                        width: 8,
                        kind: SignalKind::Reg,
                        init_val: x_init.clone(),
                        ..Default::default()
                    },
                    SignalInfo {
                        name: Symbol::from("y"),
                        width: 8,
                        kind: SignalKind::Wire,
                        init_val: x_init.clone(),
                        ..Default::default()
                    },
                    SignalInfo {
                        name: Symbol::from("unused"),
                        width: 8,
                        kind: SignalKind::Reg,
                        init_val: unused_init,
                        ..Default::default()
                    },
                ],
                inputs: vec![],
                outputs: vec![],
                inouts: vec![],
                processes: vec![
                    Process::Combinational {
                        name: Symbol::from("comb"),
                        sensitivity: vec![],
                        body: vec![IrStmt::BlockingAssign {
                            lhs: IrLValue::Signal(2, 8),
                            rhs: IrExpr::Signal(1, 8),
                            delay: None,
                        }],
                    },
                    Process::Sequential {
                        name: Symbol::from("ff"),
                        clock: ClockEdge::PosEdge(0),
                        reset: None,
                        body: vec![IrStmt::NonBlockingAssign {
                            lhs: IrLValue::Signal(1, 8),
                            rhs: IrExpr::BinaryOp(
                                BinaryIrOp::Add,
                                Box::new(IrExpr::Signal(1, 8)),
                                Box::new(IrExpr::Const(LogicVec::from_u64(1, 8))),
                            ),
                            delay: None,
                        }],
                        iff: None,
                    },
                ],
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

        let interp = AbstractInterpreter::new();
        let result = interp.analyze_design(&design);

        // clk: Input, init 0 → Const(0), never assigned
        assert_eq!(result.get("clk"), Some(&AbsValue::Const(0)));

        // x: Reg, init 0. comb: read as Const(0), not assigned. ff: x <= x+1 → Const(1).
        // Join(Const(0), Const(1)) = Interval{0, 1}
        assert_eq!(
            result.get("x"),
            Some(&AbsValue::Interval { min: 0, max: 1 })
        );

        // y: Wire, init 0. comb: y = x → Const(0). ff: not assigned → Const(0). Join: Const(0).
        assert_eq!(result.get("y"), Some(&AbsValue::Const(0)));

        // unused: Reg, init X → Top, never assigned → Top
        assert_eq!(result.get("unused"), Some(&AbsValue::Top));

        // unknown_signals: only "unused" (clk has init 0 → Const(0), not Top)
        let unknown = interp.unknown_signals(&design);
        assert_eq!(unknown, vec!["unused"]);
    }

    #[test]
    fn test_analyze_design_empty_design() {
        use mivon_core::intern::Symbol;

        let design = IrDesign {
            top: IrModule {
                name: Symbol::from("empty"),
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

        let interp = AbstractInterpreter::new();
        let result = interp.analyze_design(&design);
        assert!(result.is_empty());

        let unknown = interp.unknown_signals(&design);
        assert!(unknown.is_empty());
    }
}
