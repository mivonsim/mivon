//! JIT Evaluator — integrasi Cranelift JIT ke SimulationEngine.
//!
//! Menerjemahkan `BinaryIrOp` / `UnaryIrOp` ke `JitOp` dan mengkompilasi
//! ke native code via Cranelift untuk evaluasi ekspresi yang lebih cepat.
//!
//! Fallback ke interpreted evaluation jika:
//! - Operand mengandung X/Z (4-state logic)
//! - Operasi tidak didukung oleh JIT (Div, Mod, Power, dll)
//! - Cranelift tidak tersedia (non-x86_64)

use crate::simulator::jit_cranelift::{CraneliftEngine, JitOp};
use mivon_ir::{BinaryIrOp, IrExpr, LogicVal, LogicVec, UnaryIrOp};

/// JIT Evaluator — wraps CraneliftEngine dengan API evaluasi LogicVec.
pub struct JITEvaluator {
    /// Cranelift engine (opsional — None jika platform tidak support)
    engine: Option<CraneliftEngine>,
    /// Jumlah evaluasi via JIT
    jit_eval_count: u64,
    /// Jumlah fallback ke interpreted
    fallback_count: u64,
}

impl JITEvaluator {
    /// Create new JIT evaluator.
    /// Cranelift engine di-inisialisasi secara lazy.
    pub fn new() -> Self {
        let engine = CraneliftEngine::new();
        JITEvaluator {
            engine,
            jit_eval_count: 0,
            fallback_count: 0,
        }
    }

    /// Evaluate a binary operation using JIT (if available).
    ///
    /// Returns `Some(result)` if JIT was used, `None` if fallback needed.
    pub fn eval_binary(
        &mut self,
        op: &BinaryIrOp,
        lhs: &LogicVec,
        rhs: &LogicVec,
    ) -> Option<LogicVec> {
        let engine = self.engine.as_mut()?;

        // Map BinaryIrOp to JitOp
        let jit_op = match op {
            BinaryIrOp::Add => JitOp::Add,
            BinaryIrOp::Sub => JitOp::Sub,
            BinaryIrOp::Mul => JitOp::Mul,
            BinaryIrOp::BitAnd => JitOp::And,
            BinaryIrOp::BitOr => JitOp::Or,
            BinaryIrOp::BitXor => JitOp::Xor,
            BinaryIrOp::Eq | BinaryIrOp::CaseEq => JitOp::Eq,
            BinaryIrOp::Neq | BinaryIrOp::CaseNeq => JitOp::Ne,
            BinaryIrOp::Lt => JitOp::Lt,
            BinaryIrOp::Le => JitOp::Le,
            BinaryIrOp::Gt => JitOp::Gt,
            BinaryIrOp::Ge => JitOp::Ge,
            BinaryIrOp::Shl => JitOp::Shl,
            BinaryIrOp::Shr => JitOp::Shr,
            // Operations not supported by JIT — fallback
            _ => return None,
        };

        // Determine effective width
        let width = lhs.width.max(rhs.width);
        if width == 0 {
            return Some(LogicVec::new(0));
        }
        // JIT Cranelift bekerja pada u64 — operan >64-bit WAJIB fallback ke
        // evaluator utama (jalur u128); dulu to_u64 memotong bit tinggi
        // diam-diam (ditemukan wide_fuzz seed=11: Mul 96-bit salah).
        if width > 64 {
            self.fallback_count += 1;
            return None;
        }

        // Check for X/Z — JIT hanya untuk 2-state
        let lhs_clean = !lhs
            .bits
            .iter()
            .any(|b| matches!(b, LogicVal::X | LogicVal::Z));
        let rhs_clean = !rhs
            .bits
            .iter()
            .any(|b| matches!(b, LogicVal::X | LogicVal::Z));
        if !lhs_clean || !rhs_clean {
            self.fallback_count += 1;
            return None;
        }

        // Compile (or cache hit)
        let compiled = engine.compile_binary(jit_op, width)?;

        // Evaluate via JIT
        let lval = lhs.to_u64();
        let rval = rhs.to_u64();
        let result = unsafe { CraneliftEngine::call_binary(compiled.code_ptr, lval, rval) };

        self.jit_eval_count += 1;

        // For comparison operations (Eq, Ne, Lt, Le, Gt, Ge), result is 1-bit
        let result_width = match op {
            BinaryIrOp::Eq
            | BinaryIrOp::Neq
            | BinaryIrOp::CaseEq
            | BinaryIrOp::CaseNeq
            | BinaryIrOp::Lt
            | BinaryIrOp::Le
            | BinaryIrOp::Gt
            | BinaryIrOp::Ge => 1,
            _ => width,
        };

        Some(LogicVec::from_u64(result, result_width))
    }

    /// Evaluate a unary operation using JIT (if available).
    pub fn eval_unary(&mut self, op: &UnaryIrOp, val: &LogicVec) -> Option<LogicVec> {
        let engine = self.engine.as_mut()?;

        let jit_op = match op {
            UnaryIrOp::BitNot => JitOp::Not,
            UnaryIrOp::Minus => JitOp::Neg,
            // Other ops not supported — fallback
            _ => return None,
        };

        let width = val.width;
        if width == 0 {
            return Some(LogicVec::new(0));
        }
        // >64-bit: JIT u64 memotong — fallback (wide_fuzz seed=11).
        if width > 64 {
            self.fallback_count += 1;
            return None;
        }

        // Check for X/Z
        let clean = !val
            .bits
            .iter()
            .any(|b| matches!(b, LogicVal::X | LogicVal::Z));
        if !clean {
            self.fallback_count += 1;
            return None;
        }

        // Comparison ops return 1-bit — not applicable for unary
        let compiled = engine.compile_unary(jit_op, width)?;

        let v = val.to_u64();
        let result = unsafe { CraneliftEngine::call_unary(compiled.code_ptr, v) };

        self.jit_eval_count += 1;
        Some(LogicVec::from_u64(result, width))
    }

    /// Evaluate an expression tree using expression-level JIT compilation.
    ///
    /// Walks the IrExpr tree, collects unique signal references, compiles
    /// the entire tree into one native function, and evaluates it.
    /// Returns `Some(LogicVec)` if JIT was used, `None` for fallback.
    ///
    /// Supports simple expression trees (≤8 signals, only Const/Signal/BinaryOp/UnaryOp/Cond).
    pub fn eval_expression(
        &mut self,
        expr: &IrExpr,
        signal_values: &[u64],
        result_width: usize,
    ) -> Option<LogicVec> {
        let engine = self.engine.as_mut()?;

        // Collect unique signal IDs from the expression
        let mut sig_ids = Vec::new();
        if !collect_signal_ids(expr, &mut sig_ids) {
            return None; // Contains unsupported variant
        }
        // De-duplicate while preserving order
        sig_ids.sort();
        sig_ids.dedup();

        if sig_ids.is_empty() || sig_ids.len() > 8 {
            return None; // No signals or too many
        }

        // Compile the expression tree (or cache hit)
        let compiled = engine.compile_expression(expr, &sig_ids)?;

        // Extract signal values in the order expected by the compiled function
        let mut sig_vals = Vec::with_capacity(sig_ids.len());
        for &sid in &sig_ids {
            if sid < signal_values.len() {
                sig_vals.push(signal_values[sid]);
            } else {
                sig_vals.push(0);
            }
        }

        // Call the compiled native function
        let result = unsafe { CraneliftEngine::call_expression(compiled.code_ptr, &sig_vals) };

        self.jit_eval_count += 1;

        // Mask result to the required width
        if result_width < 64 {
            let mask = (1u64 << result_width) - 1;
            Some(LogicVec::from_u64(result & mask, result_width))
        } else {
            Some(LogicVec::from_u64(result, result_width))
        }
    }

    /// Statistics
    pub fn stats(&self) -> (u64, u64, f64) {
        let total = self.jit_eval_count + self.fallback_count;
        let pct = if total == 0 {
            0.0
        } else {
            self.jit_eval_count as f64 / total as f64 * 100.0
        };
        (self.jit_eval_count, self.fallback_count, pct)
    }

    /// Check if JIT is available on this platform
    pub fn is_available(&self) -> bool {
        self.engine.is_some()
    }

    /// Check if JIT has any compiled expressions
    pub fn compiled_count(&self) -> usize {
        self.engine.as_ref().map_or(0, |e| e.compiled_count())
    }

    /// JIT cache hit rate
    pub fn cache_hit_rate(&self) -> f64 {
        self.engine.as_ref().map_or(0.0, |e| e.cache_hit_rate())
    }
}

/// Recursively collect signal IDs from an IrExpr tree.
/// Returns false if any unsupported variant is encountered.
///
/// NOTE: Shift operators (Shl/Shr/Sshl/Sshr) are excluded because the
/// expression-level JIT operates on raw 64-bit values without width masking.
/// `~(2'b10) << 2'b11` evaluates as `0xFFFFFFFFFFFFFFFD << 3` at 64-bit,
/// producing a huge value instead of the correct 2-bit result. Width masking
/// is only done in `eval_binary` / `evaluate_expr_ctx`, so shifts must go
/// through the interpreted path. (ditemukan bug comparison-context-width:
/// `y = (ternary < ~(2'b10) << 2'b11)` → wrong sign due to 64-bit shift.)
pub(crate) fn collect_signal_ids(expr: &IrExpr, ids: &mut Vec<usize>) -> bool {
    match expr {
        IrExpr::Const(_) | IrExpr::FillLit(_) => true,
        IrExpr::Signal(id, _) => {
            ids.push(*id);
            true
        }
        IrExpr::BinaryOp(op, lhs, rhs) => {
            // Shifts need width masking that JIT doesn't provide
            if matches!(
                op,
                BinaryIrOp::Shl | BinaryIrOp::Shr | BinaryIrOp::Sshl | BinaryIrOp::Sshr
            ) {
                return false;
            }
            collect_signal_ids(lhs, ids) && collect_signal_ids(rhs, ids)
        }
        IrExpr::UnaryOp(_, inner) => collect_signal_ids(inner, ids),
        IrExpr::Cond(cond, t, f) => {
            collect_signal_ids(cond, ids)
                && collect_signal_ids(t, ids)
                && collect_signal_ids(f, ids)
        }
        // Unsupported: stop collection
        _ => false,
    }
}

impl Default for JITEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jit_evaluator_create() {
        let eval = JITEvaluator::new();
        // May or may not have Cranelift (depends on platform/x86_64)
        // Just check it doesn't crash
        let _ = eval.stats();
    }

    #[test]
    fn test_jit_eval_binary_add() {
        let mut eval = JITEvaluator::new();
        if !eval.is_available() {
            eprintln!("Skipping: Cranelift not available");
            return;
        }
        let a = LogicVec::from_u64(10, 32);
        let b = LogicVec::from_u64(20, 32);
        let result = eval.eval_binary(&BinaryIrOp::Add, &a, &b);
        assert!(result.is_some());
        assert_eq!(result.unwrap().to_u64(), 30);
    }

    #[test]
    fn test_jit_eval_binary_sub() {
        let mut eval = JITEvaluator::new();
        if !eval.is_available() {
            return;
        }
        let result = eval.eval_binary(
            &BinaryIrOp::Sub,
            &LogicVec::from_u64(50, 32),
            &LogicVec::from_u64(23, 32),
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap().to_u64(), 27);
    }

    #[test]
    fn test_jit_eval_binary_and() {
        let mut eval = JITEvaluator::new();
        if !eval.is_available() {
            return;
        }
        let result = eval.eval_binary(
            &BinaryIrOp::BitAnd,
            &LogicVec::from_u64(0xFF, 8),
            &LogicVec::from_u64(0x0F, 8),
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap().to_u64(), 0x0F);
    }

    #[test]
    fn test_jit_eval_binary_or() {
        let mut eval = JITEvaluator::new();
        if !eval.is_available() {
            return;
        }
        let result = eval.eval_binary(
            &BinaryIrOp::BitOr,
            &LogicVec::from_u64(0xF0, 8),
            &LogicVec::from_u64(0x0F, 8),
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap().to_u64(), 0xFF);
    }

    #[test]
    fn test_jit_eval_binary_xor() {
        let mut eval = JITEvaluator::new();
        if !eval.is_available() {
            return;
        }
        let result = eval.eval_binary(
            &BinaryIrOp::BitXor,
            &LogicVec::from_u64(0xFF, 8),
            &LogicVec::from_u64(0x0F, 8),
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap().to_u64(), 0xF0);
    }

    #[test]
    fn test_jit_eval_binary_eq() {
        let mut eval = JITEvaluator::new();
        if !eval.is_available() {
            return;
        }
        let result = eval.eval_binary(
            &BinaryIrOp::Eq,
            &LogicVec::from_u64(5, 32),
            &LogicVec::from_u64(5, 32),
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap().to_u64(), 1);
    }

    #[test]
    fn test_jit_eval_binary_lt() {
        let mut eval = JITEvaluator::new();
        if !eval.is_available() {
            return;
        }
        let result = eval.eval_binary(
            &BinaryIrOp::Lt,
            &LogicVec::from_u64(3, 32),
            &LogicVec::from_u64(7, 32),
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap().to_u64(), 1);
    }

    #[test]
    fn test_jit_eval_binary_shl() {
        let mut eval = JITEvaluator::new();
        if !eval.is_available() {
            return;
        }
        let result = eval.eval_binary(
            &BinaryIrOp::Shl,
            &LogicVec::from_u64(1, 8),
            &LogicVec::from_u64(3, 8),
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap().to_u64(), 8);
    }

    #[test]
    fn test_jit_eval_fallback_on_x() {
        let mut eval = JITEvaluator::new();
        if !eval.is_available() {
            return;
        }
        // X values should trigger fallback
        let x_vec = LogicVec {
            width: 4,
            bits: vec![LogicVal::X, LogicVal::Zero, LogicVal::One, LogicVal::Zero],
        };
        let result = eval.eval_binary(&BinaryIrOp::Add, &x_vec, &LogicVec::from_u64(5, 4));
        assert!(result.is_none(), "X values should cause fallback");
    }

    #[test]
    fn test_jit_eval_fallback_on_unsupported_op() {
        let mut eval = JITEvaluator::new();
        if !eval.is_available() {
            return;
        }
        // EqWild (wildcard equality) is not supported by JIT
        let result = eval.eval_binary(
            &BinaryIrOp::EqWild,
            &LogicVec::from_u64(10, 32),
            &LogicVec::from_u64(2, 32),
        );
        assert!(result.is_none(), "EqWild should fallback");
    }

    #[test]
    fn test_jit_eval_unary_not() {
        let mut eval = JITEvaluator::new();
        if !eval.is_available() {
            return;
        }
        let result = eval.eval_unary(&UnaryIrOp::BitNot, &LogicVec::from_u64(0xFF, 8));
        assert!(result.is_some());
        assert_eq!(result.unwrap().to_u64(), 0x00);
    }

    #[test]
    fn test_jit_eval_unary_neg() {
        let mut eval = JITEvaluator::new();
        if !eval.is_available() {
            return;
        }
        let result = eval.eval_unary(&UnaryIrOp::Minus, &LogicVec::from_u64(42, 32));
        assert!(result.is_some());
        // Two's complement: -42 in 32-bit = 0xFFFFFFD6
        assert_eq!(result.unwrap().to_u64() as i32, -42);
    }

    #[test]
    fn test_jit_eval_width_masking() {
        let mut eval = JITEvaluator::new();
        if !eval.is_available() {
            return;
        }
        // 4-bit add: 15 + 1 = 16 → masked to 4 bits → 0
        let result = eval.eval_binary(
            &BinaryIrOp::Add,
            &LogicVec::from_u64(15, 4),
            &LogicVec::from_u64(1, 4),
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap().to_u64(), 0, "4-bit add should wrap: 15+1=0");
    }

    #[test]
    fn test_jit_eval_stats() {
        let mut eval = JITEvaluator::new();
        if !eval.is_available() {
            return;
        }

        // Do some evaluations
        let _ = eval.eval_binary(
            &BinaryIrOp::Add,
            &LogicVec::from_u64(1, 32),
            &LogicVec::from_u64(2, 32),
        );
        let _ = eval.eval_binary(
            &BinaryIrOp::Add,
            &LogicVec::from_u64(3, 32),
            &LogicVec::from_u64(4, 32),
        );
        // Unsupported ops (like EqWild) return None without incrementing fallback —
        // fallback only counts X/Z data that COULD be JIT but was not
        let _ = eval.eval_binary(
            &BinaryIrOp::EqWild,
            &LogicVec::from_u64(1, 32),
            &LogicVec::from_u64(2, 32),
        ); // unsupported -> None, no fallback count

        let (jit, fallback, _) = eval.stats();
        assert_eq!(jit, 2, "2 JIT evals");
        assert_eq!(
            fallback, 0,
            "0 fallbacks (EqWild is unsupported, not a fallback)"
        );
    }
}
