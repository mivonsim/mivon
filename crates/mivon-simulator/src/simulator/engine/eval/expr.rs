use super::super::engine_utils::{edge_matches_abbrev, evaluate_string_method, sym_char_matches};
use super::super::SimulationEngine;
use crate::simulator::packed::PackedLogicVec;
use crate::simulator::packed_eval::{eval_binary_packed, eval_unary_packed, is_packable_binary_op};
use crate::simulator::types::*;
use crate::simulator::util::*;
use crate::simulator::value::*;
use mivon_ast::*;
use mivon_core::diagnostics::DiagCode;
use mivon_core::error::SimError;
use mivon_core::Symbol;
use mivon_ir::*;
use rand::Rng;
use rand::SeedableRng;
use std::collections::{HashMap, VecDeque};
use std::io::Write;

impl SimulationEngine {
    pub(crate) fn eval_assign_rhs(
        &mut self,
        expr: &IrExpr,
        lhs: &IrLValue,
    ) -> Result<LogicVec, SimError> {
        if let IrExpr::FillLit(v) = expr {
            let w = self.get_lvalue_width(lhs);
            Ok(LogicVec::fill(*v, w))
        } else if is_signed_expr(expr, &self.design.top.signals) {
            // RHS signed (LRM §11.8.2): assignment ke LHS lebih lebar harus
            // SIGN-extend. Dulu hanya cabang `IrExpr::Signed(inner)` top-level
            // — `-8'sd3` (UnaryOp Minus atas literal signed) terlewat sehingga
            // zero-extend: -3 jadi 253 (probe p12i). Generalisasi via
            // is_signed_expr menangani unary/binary/signal/ternary signed.
            let mut val = self.evaluate_expr(expr)?;
            let target_w = self.get_lvalue_width(lhs);
            if val.width < target_w {
                let msb = val.bits.last().copied().unwrap_or(LogicVal::Zero);
                val.bits.resize(target_w, msb);
                val.width = target_w;
            }
            Ok(val)
        } else if let IrExpr::NewCall { class_name, args } = expr {
            if class_name.is_empty() && args.len() == 1 {
                let size_val = self.evaluate_expr(&args[0])?;
                let size = size_val.to_u64() as usize;
                if let Some(sig_id) = self.signal_id_from_lvalue(lhs) {
                    let elem_width = self
                        .design
                        .top
                        .signals
                        .get(sig_id)
                        .map(|s| s.elem_width)
                        .unwrap_or(1);
                    Ok(LogicVec::fill(LogicVal::X, size * elem_width))
                } else {
                    self.evaluate_expr(expr)
                }
            } else {
                self.evaluate_expr(expr)
            }
        } else {
            // Verilog context-width (IEEE 1800-2017 §11.6): width ekspresi RHS
            // = max(width LHS, self-determined). Kritis untuk SHIFT — tanpa
            // ini `(a[31:12]) << 12` dievaluasi dalam 20-bit operand dan semua
            // bit high terbuang (kasus nyata: picorv32
            // `decoded_imm <= mem_rdata_q[31:12] << 12` menghasilkan 0).
            let target_w = self.get_lvalue_width(lhs);
            self.evaluate_expr_ctx(expr, target_w)
        }
    }

    /// Evaluasi expression dengan width konteks assignment. Hanya shift yang
    /// butuh perlakuan khusus (operand di-extend ke ctx_width sebelum digeser);
    /// node lain dievaluasi normal.
    pub(crate) fn evaluate_expr_ctx(
        &mut self,
        expr: &IrExpr,
        ctx_width: usize,
    ) -> Result<LogicVec, SimError> {
        if let IrExpr::BinaryOp(op, l, r) = expr {
            if matches!(
                op,
                BinaryIrOp::Shl | BinaryIrOp::Shr | BinaryIrOp::Sshl | BinaryIrOp::Sshr
            ) {
                let mut lv = self.evaluate_expr(l)?;
                if lv.width < ctx_width {
                    // >>> (Sshr) arithmetic HANYA untuk operand signed
                    // (extend sign bit); unsigned → zero-extend (>>> = logical,
                    // IEEE 1800-2017 §11.4.10). Shl/Shr/Sshl selalu zero.
                    let sign_fill = matches!(op, BinaryIrOp::Sshr)
                        && is_signed_expr(l, &self.design.top.signals);
                    let fill = if sign_fill {
                        lv.bits.last().copied().unwrap_or(LogicVal::Zero)
                    } else {
                        LogicVal::Zero
                    };
                    lv.bits.resize(ctx_width, fill);
                    lv.width = ctx_width;
                }
                let rv = self.evaluate_expr(r)?;
                // Dispatch >>> identik dengan jalur evaluate_expr_impl:
                // signed → arithmetic (eval_sshr_signed), unsigned → logical.
                if *op == BinaryIrOp::Sshr {
                    if is_signed_expr(l, &self.design.top.signals) {
                        return Ok(eval_sshr_signed(&lv, &rv));
                    }
                    return Ok(eval_binary(BinaryIrOp::Shr, &lv, &rv));
                }
                return Ok(eval_binary(op.clone(), &lv, &rv));
            }
        }
        self.evaluate_expr(expr)
    }

    pub(crate) fn evaluate_expr(&mut self, expr: &IrExpr) -> Result<LogicVec, SimError> {
        self.expr_recursion_depth += 1;
        if self.expr_recursion_depth > 4096 {
            self.expr_recursion_depth = 0;
            return Err(self.diag_error(mivon_core::diagnostics::DiagCode::InternalError, "expression recursion depth exceeded (possible infinite recursion in expression evaluation)"));
        }

        // ── Expression-level JIT: try to compile + evaluate entire IrExpr tree ──
        // Hanya untuk simple expression trees (Const/Signal/BinaryOp/UnaryOp/Cond)
        // dengan sinyal 2-state (no X/Z) dan ≤8 unique signal references.
        // Jika JIT gagal, fallback ke evaluate_expr_impl recursive descent.
        //
        // STEP 1: Pre-check — apakah expression JIT-compatible?
        // (Gunakan collect_signal_ids sebagai pre-check murah sebelum build signal_values)
        if self.use_jit_expression {
            #[cfg_attr(not(feature = "jit"), allow(unused_mut))]
            let mut sig_ids = Vec::new();
            #[cfg(feature = "jit")]
            let is_compatible = crate::simulator::jit_eval::collect_signal_ids(expr, &mut sig_ids);
            #[cfg(not(feature = "jit"))]
            let is_compatible = false;
            if is_compatible && !sig_ids.is_empty() && sig_ids.len() <= 8 {
                // Compute result_width FIRST to avoid borrow conflict with jit_evaluator
                let result_width = self.compute_jit_expr_width(expr);

                if let Some(ref mut jit) = self.jit_evaluator {
                    if jit.is_available() {
                        // STEP 2: Build signal_values hanya untuk sinyal yang direferensi
                        let n_sigs = self.state.signals.len();
                        let mut all_clean = true;
                        for &sid in &sig_ids {
                            if sid < n_sigs {
                                let sig = self.state.read_signal(sid);
                                if sig
                                    .bits
                                    .iter()
                                    .any(|b| matches!(b, LogicVal::X | LogicVal::Z))
                                {
                                    all_clean = false;
                                    break;
                                }
                            } else {
                                all_clean = false;
                                break;
                            }
                        }

                        if all_clean {
                            // STEP 3: Build full signal_values array (eval_expression expects
                            // array indexed by global signal ID)
                            let mut signal_values: Vec<u64> = Vec::with_capacity(n_sigs);
                            for i in 0..n_sigs {
                                signal_values.push(self.state.read_signal(i).to_u64());
                            }

                            let jit_result =
                                jit.eval_expression(expr, &signal_values, result_width);
                            if let Some(result) = jit_result {
                                self.expr_recursion_depth =
                                    self.expr_recursion_depth.saturating_sub(1);
                                return Ok(result);
                            }
                        }
                    }
                }
            }
        }

        let result = self.evaluate_expr_impl(expr);
        self.expr_recursion_depth = self.expr_recursion_depth.saturating_sub(1);
        result
    }

    fn evaluate_expr_impl(&mut self, expr: &IrExpr) -> Result<LogicVec, SimError> {
        match expr {
            IrExpr::Const(val) => Ok(val.clone()),
            IrExpr::FillLit(val) => Ok(LogicVec::fill(*val, 1)),
            IrExpr::Signal(id, _) => {
                let mut val = self.state.read_signal(*id).clone();
                // UPF power-aware: if signal's domain is OFF, force to X (or isolation clamp)
                if let Some(ref pi) = self.power_intent {
                    if pi.enabled {
                        if let Some(sig) = self.design.top.signals.get(*id) {
                            if pi.is_signal_powered_off(sig.name.as_str()) {
                                // Check for isolation cell: clamp to specified value instead of X
                                if let Some(clamp) = pi.get_isolation_clamp(sig.name.as_str()) {
                                    val = LogicVec::fill(clamp, val.width);
                                } else {
                                    val = LogicVec::fill(LogicVal::X, val.width);
                                }
                            }
                        }
                    }
                }
                sanitize_for_2state(&self.design.top.signals, *id, &mut val);
                Ok(val)
            }
            IrExpr::RangeSelect(sig_id, msb, lsb) => {
                let val = self.state.read_signal(*sig_id);
                let (start, end) = if *msb > *lsb {
                    (*lsb, *msb)
                } else {
                    (*msb, *lsb)
                };
                // Out-of-range select → bit luar batas X, bit dalam batas
                // tetap NILAI ASLI (LRM 1800 §11.5.1). Dulu seluruh hasil
                // X bila sebagian di luar batas — menghilangkan bit known-0
                // yang seharusnya mendominasi reduction (&(b[14:1]) pada
                // reg 2-bit harus 0, bukan X; ditemukan guided_fuzz
                // seed=111666772; emas + Icarus).
                let n = val.bits.len();
                if start > end || n == 0 {
                    let w = (end - start + 1).max(1);
                    return Ok(LogicVec::fill(LogicVal::X, w));
                }
                let w = end - start + 1;
                let mut bits = Vec::with_capacity(w);
                for i in start..=end {
                    bits.push(val.bits.get(i).copied().unwrap_or(LogicVal::X));
                }
                Ok(LogicVec { width: w, bits })
            }
            IrExpr::BitSelect(sig_id, idx) => {
                let val = self.state.read_signal(*sig_id);
                let bit = val.bits.get(*idx).copied().unwrap_or(LogicVal::X);
                Ok(LogicVec {
                    bits: vec![bit],
                    width: 1,
                })
            }
            IrExpr::ExprRangeSelect(inner, msb, lsb) => {
                let val = self.evaluate_expr(inner)?;
                let (start, end) = if *msb > *lsb {
                    (*lsb, *msb)
                } else {
                    (*msb, *lsb)
                };
                // Out-of-range select → bit luar batas X, bit dalam batas
                // tetap nilai asli (LRM §11.5.1 — sama dgn RangeSelect).
                if start > end {
                    let w = (end - start + 1).max(1);
                    return Ok(LogicVec::fill(LogicVal::X, w));
                }
                let w = end - start + 1;
                let mut bits = Vec::with_capacity(w);
                for i in start..=end {
                    bits.push(val.bits.get(i).copied().unwrap_or(LogicVal::X));
                }
                Ok(LogicVec { width: w, bits })
            }
            IrExpr::ExprBitSelect(inner, idx) => {
                let val = self.evaluate_expr(inner)?;
                let bit = val.bits.get(*idx).copied().unwrap_or(LogicVal::X);
                Ok(LogicVec {
                    bits: vec![bit],
                    width: 1,
                })
            }
            IrExpr::ExprPartSelect(inner, base_expr, width_expr) => {
                let val = self.evaluate_expr(inner)?;
                let base_lv = self.evaluate_expr(base_expr)?;
                let width = self.evaluate_expr(width_expr)?;
                let base_w = base_lv.width;
                let base = base_lv.to_u64() as usize;
                let width = width.to_u64() as usize;
                if width == 0 {
                    return Ok(LogicVec::new(1));
                }
                // LRM 1800 §11.5.1 + realita Icarus: indexed part-select
                // sebagian di luar batas → bit luar batas x, bit dalam
                // batas tetap nilai asli (`a[0 +: 6]` pada reg [3:0] =
                // xx1010). Dulu seluruh hasil x (ditemukan saat verifikasi
                // fix guided_fuzz seed=111666772).
                let mut bits = Vec::with_capacity(width);
                // Reinterpretasi base ter-wrap NEGATIF: hasil rewrite parser
                // `a[b -: ws]` → base = b-(ws-1) yang bisa wrap two's-
                // complement (mis. 0xFFFFFFFB @32 = -5). HANYA bentuk
                // Const/Sub yang direinterpretasi — base unsigned murni
                // (mis. reg 17-bit bernilai besar pada `a[b +: ws]`) tetap
                // unsigned sehingga seluruhnya OOB→X sesuai Icarus
                // (LRM §11.5.1; guided_fuzz partsel).
                let neg_ok = matches!(
                    base_expr.as_ref(),
                    IrExpr::Const(_) | IrExpr::BinaryOp(BinaryIrOp::Sub, ..)
                );
                let neg_base: i128 = if neg_ok
                    && base >= val.bits.len()
                    && base_w > 0
                    && base_w < 64
                    && ((base as u64) >> (base_w - 1)) & 1 == 1
                {
                    ((base as u64) as i64).wrapping_sub(1i64 << base_w) as i128
                } else {
                    base as i128
                };
                for k in 0..width {
                    let idx = neg_base + k as i128;
                    if idx >= 0 {
                        bits.push(val.bits.get(idx as usize).copied().unwrap_or(LogicVal::X));
                    } else {
                        bits.push(LogicVal::X);
                    }
                }
                Ok(LogicVec {
                    width: bits.len(),
                    bits,
                })
            }
            IrExpr::ArrayIndex {
                sig_id,
                index,
                elem_width,
            } => {
                let key_val = self.evaluate_expr(index)?;
                // Check if this is an associative array
                let sig_info = self.design.top.signals.get(*sig_id);
                if sig_info.map(|s| s.is_associative).unwrap_or(false) {
                    let assoc_map = self.assoc_data.entry(*sig_id).or_default();
                    if let Some(val) = assoc_map.get(&key_val) {
                        return Ok(val.clone());
                    }
                    return Ok(LogicVec::new(*elem_width));
                }
                // Baca slot LANGSUNG dari state (borrow) — jangan clone seluruh
                // array (1e9 bit/GB) utk mengambil elem_width bit (OOM).
                let idx = key_val.to_u64() as usize;
                let start = idx * elem_width;
                let mut bits = Vec::with_capacity(*elem_width);
                for i in 0..*elem_width {
                    bits.push(self.state.read_signal_bit(*sig_id, start + i));
                }
                Ok(LogicVec {
                    width: *elem_width,
                    bits,
                })
            }
            IrExpr::Concat(exprs) => {
                // String concat (`{s1, s2}` / `{"a", s}`) mempertahankan urutan
                // operan kiri→kanan — string bukan bit-vector (IEEE 1800 §6.16.2).
                // Bit concat: operan source MSB-first → bangun LSB-first (rev)
                // karena `extend` menambahkan part sebagai bit rendah.
                let is_string_concat = exprs.iter().any(|e| match e {
                    IrExpr::String(_) => true,
                    IrExpr::Signal(id, _) => self
                        .design
                        .top
                        .signals
                        .get(*id)
                        .map(|s| s.is_string)
                        .unwrap_or(false),
                    _ => false,
                });
                let mut result = LogicVec::new(0);
                if is_string_concat {
                    for e in exprs.iter() {
                        let part = self.evaluate_expr(e)?;
                        result = result.extend(&part);
                    }
                } else {
                    for e in exprs.iter().rev() {
                        let part = self.evaluate_expr(e)?;
                        result = result.extend(&part);
                    }
                }
                Ok(result)
            }
            IrExpr::Replicate(count, inner) => {
                let val = self.evaluate_expr(inner)?;
                let mut result = LogicVec::new(0);
                for _ in 0..*count {
                    result = result.extend(&val);
                }
                Ok(result)
            }
            IrExpr::UnaryOp(op, inner) => {
                let val = self.evaluate_expr(inner)?;
                let inner_is_real = matches!(inner.as_ref(), IrExpr::Signal(id, _) if self.design.top.signals.get(*id).map(|s| s.is_real).unwrap_or(false));
                if inner_is_real {
                    let a = f64::from_bits(val.to_u64());
                    let result = match op {
                        UnaryIrOp::Minus => -a,
                        UnaryIrOp::Plus => a,
                        _ => return Ok(eval_unary(op.clone(), &val)),
                    };
                    return Ok(LogicVec::from_u64(result.to_bits(), 64));
                }
                // Try packed eval first (SIMD-ready bitmask ops)
                if self.use_packed_eval {
                    let packed = PackedLogicVec::from_logicvec(&val);
                    if let Some(packed_result) = eval_unary_packed(op, &packed) {
                        let result = packed_result.to_logicvec();
                        return Ok(result);
                    }
                }
                // Try JIT for non-real unary operations
                if let Some(ref mut jit) = self.jit_evaluator {
                    if let Some(result) = jit.eval_unary(op, &val) {
                        return Ok(result);
                    }
                }
                Ok(eval_unary(op.clone(), &val))
            }
            IrExpr::BinaryOp(op, lhs, rhs) => {
                let lval = self.evaluate_expr(lhs)?;
                let rval = self.evaluate_expr(rhs)?;
                if std::env::var("MIVON_DBG_PS").is_ok() {
                    fn tag(e: &IrExpr) -> &'static str {
                        match e {
                            IrExpr::Signed(_) => "Signed",
                            IrExpr::Cast { .. } => "Cast",
                            IrExpr::Const(_) => "Const",
                            IrExpr::Signal(..) => "Signal",
                            IrExpr::BinaryOp(..) => "BinOp",
                            _ => "Other",
                        }
                    }
                    eprintln!(
                        "[DBG-BIN] op={:?} l={:x}({}b) r={:x}({}b) ls={} rs={} lnode={} rnode={} lxz={} rxz={}",
                        op, lval.to_u64(), lval.width, rval.to_u64(), rval.width,
                        is_signed_expr(lhs.as_ref(), &self.design.top.signals),
                        is_signed_expr(rhs.as_ref(), &self.design.top.signals),
                        tag(lhs.as_ref()), tag(rhs.as_ref()),
                        lval.bits.iter().any(|b| matches!(b, LogicVal::X | LogicVal::Z)),
                        rval.bits.iter().any(|b| matches!(b, LogicVal::X | LogicVal::Z)),
                    );
                }
                let lhs_is_real = matches!(lhs.as_ref(), IrExpr::Signal(id, _) if self.design.top.signals.get(*id).map(|s| s.is_real).unwrap_or(false));
                let rhs_is_real = matches!(rhs.as_ref(), IrExpr::Signal(id, _) if self.design.top.signals.get(*id).map(|s| s.is_real).unwrap_or(false));
                if lhs_is_real || rhs_is_real {
                    let a = f64::from_bits(lval.to_u64());
                    let b = f64::from_bits(rval.to_u64());
                    let result = match op {
                        BinaryIrOp::Add => a + b,
                        BinaryIrOp::Sub => a - b,
                        BinaryIrOp::Mul => a * b,
                        BinaryIrOp::Div => a / b,
                        BinaryIrOp::Mod => a % b,
                        BinaryIrOp::Power => a.powf(b),
                        BinaryIrOp::Lt => {
                            return Ok(LogicVec::from_u64(if a < b { 1 } else { 0 }, 32))
                        }
                        BinaryIrOp::Le => {
                            return Ok(LogicVec::from_u64(if a <= b { 1 } else { 0 }, 32))
                        }
                        BinaryIrOp::Gt => {
                            return Ok(LogicVec::from_u64(if a > b { 1 } else { 0 }, 32))
                        }
                        BinaryIrOp::Ge => {
                            return Ok(LogicVec::from_u64(if a >= b { 1 } else { 0 }, 32))
                        }
                        BinaryIrOp::Eq => {
                            return Ok(LogicVec::from_u64(if a == b { 1 } else { 0 }, 32))
                        }
                        BinaryIrOp::Neq => {
                            return Ok(LogicVec::from_u64(if a != b { 1 } else { 0 }, 32))
                        }
                        _ => return Ok(eval_binary(op.clone(), &lval, &rval)),
                    };
                    Ok(LogicVec::from_u64(result.to_bits(), 64))
                } else if matches!(
                    op,
                    BinaryIrOp::Add
                        | BinaryIrOp::Sub
                        | BinaryIrOp::Mul
                        | BinaryIrOp::Eq
                        | BinaryIrOp::Neq
                        | BinaryIrOp::CaseEq
                        | BinaryIrOp::CaseNeq
                        | BinaryIrOp::Lt
                        | BinaryIrOp::Le
                        | BinaryIrOp::Gt
                        | BinaryIrOp::Ge
                ) && (is_signed_expr(lhs.as_ref(), &self.design.top.signals)
                    && is_signed_expr(rhs.as_ref(), &self.design.top.signals))
                {
                    // LRM §11.8.2 any-unsigned: perbandingan & ARITMETIK
                    // dilakukan SIGNED hanya bila KEDUA operand signed
                    // (mis. `logic [1:0] m = 3; m > 2` → 3 > 2 = 1, bukan
                    // sign-extend 2-bit 11 → -1 > 2 = 0). eval_binary_signed
                    // men-sign-extend operan ke lebar umum: `-8'sd4 + 1`
                    // (lebar campur) harus -3, bukan 253 (probe q07 — dulu
                    // Add/Sub/Mul jatuh ke eval_binary zero-extend).
                    // Eq/CaseEq ikut (sign-extend operan ke lebar umum):
                    // `int X = -2; X === -2` salah bila zero-extend
                    // (0xFFFFFFFFFFFFFFFE vs 0x00000000FFFFFFFE — probe p12f).
                    Ok(eval_binary_signed(op.clone(), &lval, &rval))
                } else if matches!(op, BinaryIrOp::Div | BinaryIrOp::Mod)
                    && (is_signed_expr(lhs.as_ref(), &self.design.top.signals)
                        && is_signed_expr(rhs.as_ref(), &self.design.top.signals))
                {
                    // Div/Mod signed: operand sinyal mungkin SUDAH zero-extend ke
                    // lebar konteks saat evaluate (bug #7: 8-bit -4 → 0x000000FC
                    // → 252). Clip ke lebar ASLI signal agar eval_binary_signed
                    // melakukan sign-extend yang benar.
                    let lv_c = signed_raw_operand(lhs.as_ref(), &lval, &self.design.top.signals);
                    let rv_c = signed_raw_operand(rhs.as_ref(), &rval, &self.design.top.signals);
                    Ok(eval_binary_signed(op.clone(), &lv_c, &rv_c))
                } else if matches!(op, BinaryIrOp::Sshr) {
                    // `>>>` (IEEE 1800 §11.4.10): ARITHMETIC bila lhs signed,
                    // LOGICAL bila unsigned. eval_sshr_signed memakai lebar
                    // ASLI lhs — extend_to selalu zero-extend sehingga Sshr
                    // lama kehilangan sign bit (signed [7:0] -128 >>> 2 = 0x20
                    // padahal harusnya 0xE0).
                    if is_signed_expr(lhs.as_ref(), &self.design.top.signals) {
                        Ok(eval_sshr_signed(&lval, &rval))
                    } else {
                        Ok(eval_binary(BinaryIrOp::Shr, &lval, &rval))
                    }
                } else {
                    // Try packed eval first (SIMD-ready bitmask ops for bitwise ops only)
                    if self.use_packed_eval && is_packable_binary_op(op) {
                        let packed_lhs = PackedLogicVec::from_logicvec(&lval);
                        let packed_rhs = PackedLogicVec::from_logicvec(&rval);
                        if let Some(packed_result) =
                            eval_binary_packed(op, &packed_lhs, &packed_rhs)
                        {
                            let result = packed_result.to_logicvec();
                            return Ok(result);
                        }
                    }
                    // Try JIT for non-real binary operations (but not shifts/comparisons - JIT uses max width which is wrong for shifts, and signed comparison for all comparisons)
                    let is_shift = matches!(
                        op,
                        BinaryIrOp::Shl | BinaryIrOp::Shr | BinaryIrOp::Sshl | BinaryIrOp::Sshr
                    );
                    let is_comparison = matches!(
                        op,
                        BinaryIrOp::Lt | BinaryIrOp::Le | BinaryIrOp::Gt | BinaryIrOp::Ge
                    );
                    if !is_shift && !is_comparison {
                        if let Some(ref mut jit) = self.jit_evaluator {
                            if let Some(result) = jit.eval_binary(op, &lval, &rval) {
                                return Ok(result);
                            }
                        }
                    }
                    Ok(eval_binary(op.clone(), &lval, &rval))
                }
            }
            IrExpr::Cond(cond, true_expr, false_expr) => {
                let cval = self.evaluate_expr(cond)?;
                let tv = self.evaluate_expr(true_expr)?;
                let fv = self.evaluate_expr(false_expr)?;
                let w = tv.width.max(fv.width);
                let tv = if tv.width < w { tv.resize(w) } else { tv };
                let fv = if fv.width < w { fv.resize(w) } else { fv };
                match cval.to_bool() {
                    Some(true) => Ok(tv),
                    Some(false) => Ok(fv),
                    // IEEE 1800-2017 §11.4.11: kondisi unknown/z → hasil
                    // kombinasi bitwise per Tabel 11-22 (bit kedua-branch
                    // sama → nilai itu; berbeda/x/z → X). Sebelumnya jatuh ke
                    // cabang else (perilaku if-statement §12.4) — salah untuk
                    // operator kondisional: `x ? a : b` harus menghasilkan X,
                    // bukan b (ditemukan probe p04 ternary-x).
                    None => {
                        let mut bits = Vec::with_capacity(w);
                        for i in 0..w {
                            let out = match (tv.bits.get(i), fv.bits.get(i)) {
                                (Some(LogicVal::Zero), Some(LogicVal::Zero)) => LogicVal::Zero,
                                (Some(LogicVal::One), Some(LogicVal::One)) => LogicVal::One,
                                _ => LogicVal::X,
                            };
                            bits.push(out);
                        }
                        Ok(LogicVec { bits, width: w })
                    }
                }
            }
            IrExpr::Signed(inner) => self.evaluate_expr(inner),
            IrExpr::String(s) => {
                let mut bits = Vec::with_capacity(s.len() * 8);
                for c in s.chars() {
                    let byte = c as u8;
                    for i in 0..8 {
                        bits.push(if (byte >> i) & 1 == 1 {
                            LogicVal::One
                        } else {
                            LogicVal::Zero
                        });
                    }
                }
                Ok(LogicVec {
                    width: bits.len(),
                    bits,
                })
            }
            IrExpr::SysFunc {
                name,
                args,
                line,
                col,
            } => {
                // F20: catat posisi sysfunc agar warning runtime punya lokasi.
                self.set_cur_src_pos(*line, *col);
                match name.as_str() {
                    "$random" => {
                        self.rand_call_count += 1;
                        // If seed argument provided, reseed RNG for reproducibility
                        if let Some(seed_arg) = args.first() {
                            if let Ok(seed_val) = self.evaluate_expr(seed_arg) {
                                let seed = seed_val.to_u64();
                                self.rng = rand::rngs::StdRng::seed_from_u64(seed);
                                self.rand_seed = seed;
                            }
                        }
                        let val: i32 = self.rng.gen();
                        Ok(LogicVec::from_u64(val as u64, 32))
                    }
                    "$urandom" => {
                        self.rand_call_count += 1;
                        let val: u32 = self.rng.gen();
                        Ok(LogicVec::from_u64(val as u64, 32))
                    }
                    "$urandom_range" => {
                        self.rand_call_count += 1;
                        let args_eval: Vec<LogicVec> = args
                            .iter()
                            .map(|a| self.evaluate_expr(a))
                            .collect::<Result<_, _>>()?;
                        let maxval = args_eval.first().map(|v| v.to_u64()).unwrap_or(0);
                        let minval = args_eval.get(1).map(|v| v.to_u64()).unwrap_or(0);
                        if maxval <= minval {
                            Ok(LogicVec::from_u64(minval, 32))
                        } else {
                            let range = maxval - minval + 1;
                            let val: u64 = if range <= 1 {
                                minval
                            } else {
                                minval + (self.rng.gen::<u64>() % range)
                            };
                            Ok(LogicVec::from_u64(val, 32))
                        }
                    }
                    "$urandom_seed" => {
                        let prev_seed = self.rand_seed;
                        let seed = args
                            .first()
                            .map(|a| self.evaluate_expr(a).unwrap_or(LogicVec::from_u64(0, 32)))
                            .unwrap_or(LogicVec::from_u64(0, 32))
                            .to_u64();
                        self.rng = rand::rngs::StdRng::seed_from_u64(seed);
                        self.rand_seed = seed;
                        self.rand_call_count += 1;
                        Ok(LogicVec::from_u64(prev_seed, 64))
                    }
                    "$srandom" => {
                        let prev_seed = self.rand_seed;
                        if let Some(seed_arg) = args.first() {
                            if let Ok(seed_val) = self.evaluate_expr(seed_arg) {
                                let seed = seed_val.to_u64();
                                self.rng = rand::rngs::StdRng::seed_from_u64(seed);
                                self.rand_seed = seed;
                            }
                        }
                        Ok(LogicVec::from_u64(prev_seed, 64))
                    }
                    "$get_randcount" => Ok(LogicVec::from_u64(self.rand_call_count, 32)),
                    "$get_randstate" => Ok(LogicVec::from_u64(self.rand_seed, 64)),
                    "$signed" => {
                        if let Some(arg) = args.first() {
                            let val = self.evaluate_expr(arg)?;
                            // Sign-extend: copy the MSB to all higher bits
                            if val.width > 0 {
                                let msb = val.bits.last().copied().unwrap_or(LogicVal::Zero);
                                let new_width = val.width.max(1);
                                let mut bits = val.bits.clone();
                                bits.resize(new_width, msb);
                                Ok(LogicVec {
                                    width: new_width,
                                    bits,
                                })
                            } else {
                                Ok(val)
                            }
                        } else {
                            Err(self.diag_error(
                                mivon_core::diagnostics::DiagCode::DpiError,
                                "$signed expects 1 argument",
                            ))
                        }
                    }
                    "$unsigned" => {
                        if let Some(arg) = args.first() {
                            let val = self.evaluate_expr(arg)?;
                            // Unsigned: zero-extend (already the default)
                            Ok(val)
                        } else {
                            Err(self.diag_error(
                                mivon_core::diagnostics::DiagCode::DpiError,
                                "$unsigned expects 1 argument",
                            ))
                        }
                    }
                    "$countones" => {
                        if let Some(arg) = args.first() {
                            let val = self.evaluate_expr(arg)?;
                            let count =
                                val.bits.iter().filter(|b| **b == LogicVal::One).count() as u64;
                            Ok(LogicVec::from_u64(count, 32))
                        } else {
                            Err(self.diag_error(
                                mivon_core::diagnostics::DiagCode::DpiError,
                                "$countones expects 1 argument",
                            ))
                        }
                    }
                    "$onehot" => {
                        if let Some(arg) = args.first() {
                            let val = self.evaluate_expr(arg)?;
                            let ones = val.bits.iter().filter(|b| **b == LogicVal::One).count();
                            let is_onehot = ones == 1;
                            Ok(LogicVec::from_u64(if is_onehot { 1 } else { 0 }, 1))
                        } else {
                            Err(self.diag_error(
                                mivon_core::diagnostics::DiagCode::DpiError,
                                "$onehot expects 1 argument",
                            ))
                        }
                    }
                    "$isunknown" => {
                        if let Some(arg) = args.first() {
                            let val = self.evaluate_expr(arg)?;
                            let has_x_or_z = val
                                .bits
                                .iter()
                                .any(|b| *b == LogicVal::X || *b == LogicVal::Z);
                            Ok(LogicVec::from_u64(if has_x_or_z { 1 } else { 0 }, 1))
                        } else {
                            Err(self.diag_error(
                                mivon_core::diagnostics::DiagCode::DpiError,
                                "$isunknown expects 1 argument",
                            ))
                        }
                    }
                    "$countbits" => {
                        if let Some(arg) = args.first() {
                            let val = self.evaluate_expr(arg)?;
                            // $countbits counts non-zero bits (1, X, Z)
                            let count =
                                val.bits.iter().filter(|b| **b != LogicVal::Zero).count() as u64;
                            Ok(LogicVec::from_u64(count, 32))
                        } else {
                            Err(self.diag_error(
                                mivon_core::diagnostics::DiagCode::DpiError,
                                "$countbits expects 1 argument",
                            ))
                        }
                    }
                    "$dimensions" => {
                        if let Some(arg) = args.first() {
                            // $dimensions - for runtime, try to get array dimensions from signal info
                            // If it's a signal reference, we can check the signal info from design
                            if let IrExpr::Signal(sig_id, _) = arg {
                                if *sig_id < self.design.top.signals.len() {
                                    let sig_info = &self.design.top.signals[*sig_id];
                                    let packed_dims = sig_info.packed_dims.len();
                                    let unpacked_dims = sig_info.array_dims.len();
                                    let dims = packed_dims + unpacked_dims;
                                    Ok(LogicVec::from_u64(dims as u64, 32))
                                } else {
                                    Ok(LogicVec::from_u64(0, 32))
                                }
                            } else {
                                // For non-signal expressions, evaluate and return 0 as fallback
                                let _val = self.evaluate_expr(arg)?;
                                Ok(LogicVec::from_u64(0, 32))
                            }
                        } else {
                            Err(self.diag_error(
                                mivon_core::diagnostics::DiagCode::DpiError,
                                "$dimensions expects 1 argument",
                            ))
                        }
                    }
                    "$onehot0" => {
                        if let Some(arg) = args.first() {
                            let val = self.evaluate_expr(arg)?;
                            let ones = val.bits.iter().filter(|b| **b == LogicVal::One).count();
                            Ok(LogicVec::from_u64(if ones <= 1 { 1 } else { 0 }, 1))
                        } else {
                            Err(self.diag_error(
                                mivon_core::diagnostics::DiagCode::DpiError,
                                "$onehot0 expects 1 argument",
                            ))
                        }
                    }
                    "$cast" => {
                        if args.len() >= 2 {
                            // $cast(dest, src) - dynamic cast for class handles or type cast
                            // First argument is destination (lvalue), second is source
                            // For class handles: check if src object class is same or subclass of dest class
                            // For type cast: just assign and return success

                            // Evaluate source first (returns object ID for class handles)
                            let src_val = self.evaluate_expr(&args[1])?;

                            // Check if destination is a class handle signal
                            let dest_arg = &args[0];
                            let mut success = 1u64;

                            // If dest is a signal, check its SignalInfo for class_name
                            if let IrExpr::Signal(sig_id, _) = dest_arg {
                                if *sig_id < self.design.top.signals.len() {
                                    let sig_info = &self.design.top.signals[*sig_id];
                                    if let Some(dest_class_name) = sig_info.class_name {
                                        // Destination signal is a class handle - check class hierarchy
                                        // Source value is the object ID to cast from
                                        let src_obj_id = src_val.to_u64();

                                        if src_obj_id == 0 {
                                            // Casting null to class handle - always succeeds, sets to null
                                            self.state.write_signal(
                                                *sig_id,
                                                LogicVec::from_u64(0, sig_info.width),
                                            );
                                        } else if src_obj_id < self.state.objects.len() as u64 {
                                            let src_obj = &self.state.objects[src_obj_id as usize];
                                            if !src_obj.class_name.is_empty() {
                                                // Check if src_obj.class_name is same as or subclass of dest_class_name
                                                let src_class = src_obj.class_name;
                                                success = if self
                                                    .is_subclass_or_same(src_class, dest_class_name)
                                                {
                                                    1
                                                } else {
                                                    0
                                                };
                                                if success == 1 {
                                                    // Perform the cast - write source object ID to destination
                                                    self.state.write_signal(
                                                        *sig_id,
                                                        LogicVec::from_u64(
                                                            src_obj_id,
                                                            sig_info.width,
                                                        ),
                                                    );
                                                } else {
                                                    // Cast failed - write null (0)
                                                    self.state.write_signal(
                                                        *sig_id,
                                                        LogicVec::from_u64(0, sig_info.width),
                                                    );
                                                }
                                            }
                                        }
                                    } else {
                                        // Destination is not a class handle - simple type cast, always succeeds
                                        // Just assign the value (truncate/extend as needed)
                                        let dest_width = sig_info.width;
                                        let assigned = src_val.resize(dest_width);
                                        self.state.write_signal(*sig_id, assigned);
                                    }
                                }
                            } else {
                                // Other destination types (e.g., HierRef, etc.) - simple assignment
                                // For now, just evaluate and return success
                                success = 1;
                            }

                            Ok(LogicVec::from_u64(success, 1))
                        } else {
                            Err(self.diag_error(
                                mivon_core::diagnostics::DiagCode::DpiError,
                                "$cast requires two arguments",
                            ))
                        }
                    }
                    "$typename" => {
                        if let Some(arg) = args.first() {
                            // $typename returns type name as string in 8-bit per char format
                            // For now, we evaluate the argument to get its type info
                            // The type name was already resolved during elaboration and stored as Const string
                            let val = self.evaluate_expr(arg)?;
                            Ok(val)
                        } else {
                            Err(self.diag_error(
                                mivon_core::diagnostics::DiagCode::DpiError,
                                "$typename expects 1 argument",
                            ))
                        }
                    }
                    "$fopen" => {
                        let fname = args.first().and_then(|a| {
                            if let IrExpr::String(s) = a {
                                Some(s.clone())
                            } else {
                                None
                            }
                        });
                        if let Some(fname) = fname {
                            let mode = args.get(1).and_then(|a| {
                                if let IrExpr::String(s) = a {
                                    Some(s.as_str())
                                } else {
                                    None
                                }
                            });
                            let open_result = match mode {
                                Some("r") | Some("rb") => std::fs::File::open(&fname),
                                _ => std::fs::OpenOptions::new()
                                    .read(true)
                                    .write(true)
                                    .create(true)
                                    .truncate(true)
                                    .open(&fname),
                            };
                            match open_result {
                                Ok(f) => {
                                    let handle = self.next_file_handle;
                                    self.next_file_handle += 1;
                                    self.file_handles.insert(handle, f);
                                    self.file_read_pos.insert(handle, 0);
                                    Ok(LogicVec::from_u64(handle as u64, 32))
                                }
                                Err(_) => Ok(LogicVec::from_u64(0, 32)),
                            }
                        } else {
                            Ok(LogicVec::from_u64(0, 32))
                        }
                    }
                    "$fdisplay" => {
                        let handle = args
                            .first()
                            .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u32));
                        if let Some(h) = handle {
                            let msg = self.format_display(&args[1..]);
                            if let Some(f) = self.file_handles.get_mut(&h) {
                                let _ = write!(f, "{}", msg);
                            }
                        }
                        Ok(LogicVec::from_u64(0, 1))
                    }
                    "$fread" => {
                        let target = args.first().and_then(|a| {
                            if let IrExpr::Signal(id, _) = a {
                                Some(*id)
                            } else {
                                None
                            }
                        });
                        let src = args.get(1);
                        let data = if let Some(IrExpr::String(fname)) = src {
                            std::fs::read(fname).ok()
                        } else if let Some(arg) = src {
                            let handle = self
                                .evaluate_expr(arg)
                                .ok()
                                .map(|v| v.to_u64() as u32)
                                .unwrap_or(0);
                            if handle > 0 {
                                use std::io::Read;
                                self.file_handles.get_mut(&handle).and_then(|f| {
                                    let mut buf = Vec::new();
                                    f.read_to_end(&mut buf).ok().map(|_| buf)
                                })
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                        if let (Some(sid), Some(bytes)) = (target, data) {
                            let mut bits = Vec::with_capacity(bytes.len() * 8);
                            for byte in bytes {
                                for i in 0..8 {
                                    bits.push(if (byte >> i) & 1 == 1 {
                                        LogicVal::One
                                    } else {
                                        LogicVal::Zero
                                    });
                                }
                            }
                            self.state.write_signal(
                                sid,
                                LogicVec {
                                    width: bits.len(),
                                    bits,
                                },
                            );
                        }
                        Ok(LogicVec::from_u64(0, 1))
                    }
                    "$fclose" => {
                        let handle = args
                            .first()
                            .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u32));
                        if let Some(h) = handle {
                            self.file_handles.remove(&h);
                            self.file_read_pos.remove(&h);
                        }
                        Ok(LogicVec::from_u64(0, 1))
                    }
                    "$fflush" => {
                        let handle = args
                            .first()
                            .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u32));
                        if let Some(h) = handle {
                            if let Some(f) = self.file_handles.get_mut(&h) {
                                use std::io::Write;
                                let _ = f.flush();
                            }
                        }
                        Ok(LogicVec::from_u64(0, 1))
                    }
                    "$fseek" => {
                        let handle = args
                            .first()
                            .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u32));
                        let offset = args
                            .get(1)
                            .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as i64));
                        let op = args
                            .get(2)
                            .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64()));
                        if let (Some(h), Some(off)) = (handle, offset) {
                            if let Some(f) = self.file_handles.get_mut(&h) {
                                use std::io::{Seek, SeekFrom};
                                let seek_from = match op {
                                    Some(1) => SeekFrom::Current(off),
                                    Some(2) => SeekFrom::End(off),
                                    _ => SeekFrom::Start(off as u64),
                                };
                                let _ = f.seek(seek_from);
                                if let Ok(pos) = f.stream_position() {
                                    self.file_read_pos.insert(h, pos);
                                }
                            }
                        }
                        Ok(LogicVec::from_u64(0, 1))
                    }
                    "$ftell" => {
                        let handle = args
                            .first()
                            .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u32));
                        if let Some(h) = handle {
                            if let Some(f) = self.file_handles.get_mut(&h) {
                                use std::io::Seek;
                                let pos = f.stream_position().unwrap_or(0);
                                return Ok(LogicVec::from_u64(pos, 32));
                            }
                        }
                        Ok(LogicVec::from_u64(0, 32))
                    }
                    "$feof" => {
                        let handle = args
                            .first()
                            .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u32));
                        if let Some(h) = handle {
                            if let Some(f) = self.file_handles.get_mut(&h) {
                                use std::io::{Read, Seek};
                                let pos = f.stream_position().unwrap_or(0);
                                let mut byte = [0u8; 1];
                                let n = f.read(&mut byte).unwrap_or(0);
                                f.seek(std::io::SeekFrom::Start(pos)).ok();
                                return Ok(LogicVec::from_u64(if n == 0 { 1 } else { 0 }, 1));
                            }
                        }
                        Ok(LogicVec::from_u64(1, 1))
                    }
                    "$rewind" => {
                        // $rewind(fd) — rewind file to beginning (same as $fseek(fd, 0, 0))
                        let handle = args
                            .first()
                            .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u32));
                        if let Some(h) = handle {
                            if let Some(f) = self.file_handles.get_mut(&h) {
                                use std::io::{Seek, SeekFrom};
                                let _ = f.seek(SeekFrom::Start(0));
                                self.file_read_pos.insert(h, 0);
                                self.file_ungetc_buf.remove(&h);
                            }
                        }
                        Ok(LogicVec::from_u64(0, 1))
                    }
                    "$fgets" => {
                        // $fgets(str_var, fd) — read a line from file handle into string var
                        let str_arg = args.first();
                        let handle = args
                            .get(1)
                            .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u32));
                        if let Some(h) = handle {
                            if let Some(f) = self.file_handles.get_mut(&h) {
                                use std::io::{BufRead, BufReader};
                                let mut reader = BufReader::new(f.by_ref());
                                let mut line = String::new();
                                let bytes = reader.read_line(&mut line).unwrap_or(0);
                                if bytes > 0 {
                                    // Trim trailing newline for Verilog string compatibility
                                    if line.ends_with('\n') {
                                        line.pop();
                                    }
                                    if line.ends_with('\r') {
                                        line.pop();
                                    }
                                    // Convert string to LogicVec
                                    let mut bits = Vec::with_capacity(line.len() * 8);
                                    for c in line.chars() {
                                        let byte = c as u8;
                                        for i in 0..8 {
                                            bits.push(if (byte >> i) & 1 == 1 {
                                                LogicVal::One
                                            } else {
                                                LogicVal::Zero
                                            });
                                        }
                                    }
                                    // Write into the string variable
                                    if let Some(IrExpr::Signal(sid, _)) = str_arg {
                                        self.state.write_signal(
                                            *sid,
                                            LogicVec {
                                                width: bits.len(),
                                                bits,
                                            },
                                        );
                                    }
                                    return Ok(LogicVec::from_u64(bytes as u64, 32));
                                }
                            }
                        }
                        Ok(LogicVec::from_u64(0, 32))
                    }
                    "$fgetc" => {
                        // $fgetc(fd) — read a single character from file handle
                        let handle = args
                            .first()
                            .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u32));
                        if let Some(h) = handle {
                            // Check ungetc buffer first
                            if let Some(buf) = self.file_ungetc_buf.get_mut(&h) {
                                if let Some(byte) = buf.pop() {
                                    return Ok(LogicVec::from_u64(byte as u64, 32));
                                }
                            }
                            if let Some(f) = self.file_handles.get_mut(&h) {
                                use std::io::Read;
                                let mut byte = [0u8; 1];
                                let bytes = f.read(&mut byte).unwrap_or(0);
                                if bytes > 0 {
                                    return Ok(LogicVec::from_u64(byte[0] as u64, 32));
                                }
                            }
                        }
                        Ok(LogicVec::from_u64(!0u64, 32)) // EOF: returns 32'hFFFFFFFF
                    }
                    "$ungetc" => {
                        // $ungetc(char, fd) — push back a character to file handle
                        let char_val = args
                            .first()
                            .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u8));
                        let handle = args
                            .get(1)
                            .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u32));
                        if let (Some(c), Some(h)) = (char_val, handle) {
                            self.file_ungetc_buf.entry(h).or_default().push(c);
                        }
                        Ok(LogicVec::from_u64(0, 1))
                    }
                    "$fscanf" => {
                        let handle = args
                            .first()
                            .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u32));
                        if let Some(h) = handle {
                            if let Some(f) = self.file_handles.get_mut(&h) {
                                use std::io::{Read, Seek};
                                let read_pos = self.file_read_pos.entry(h).or_insert(0);
                                f.seek(std::io::SeekFrom::Start(*read_pos)).ok();
                                let mut content = String::new();
                                let _bytes_read = f.read_to_string(&mut content).unwrap_or(0);
                                *read_pos = f.stream_position().unwrap_or(0);
                                let fmt = args.get(1).and_then(|a| {
                                    if let IrExpr::String(s) = a {
                                        Some(s.clone())
                                    } else {
                                        None
                                    }
                                });
                                if let Some(ref fmt_str) = fmt {
                                    let tokens: Vec<&str> = content.split_whitespace().collect();
                                    let mut ti = 0;
                                    let mut ai = 0;
                                    let mut chars = fmt_str.chars().peekable();
                                    while let Some(c) = chars.next() {
                                        if c == '%' {
                                            if let Some(spec) = chars.next() {
                                                if spec == 'd' || spec == 'h' || spec == 'b' {
                                                    if let Some(tok) = tokens.get(ti) {
                                                        if let Ok(val) = if spec == 'h' {
                                                            i64::from_str_radix(tok, 16)
                                                        } else if spec == 'b' {
                                                            i64::from_str_radix(tok, 2)
                                                        } else {
                                                            tok.parse::<i64>()
                                                        } {
                                                            let out_idx = 2 + ai;
                                                            if let Some(IrExpr::Signal(sid, _)) =
                                                                args.get(out_idx)
                                                            {
                                                                self.state.write_signal(
                                                                    *sid,
                                                                    LogicVec::from_u64(
                                                                        val as u64, 32,
                                                                    ),
                                                                );
                                                            }
                                                            ai += 1;
                                                        }
                                                    }
                                                    ti += 1;
                                                }
                                            }
                                        }
                                    }
                                    // $fscanf returns number of items matched (or EOF)
                                    return Ok(LogicVec::from_u64(ai as u64, 32));
                                }
                            }
                        }
                        Ok(LogicVec::from_u64(0, 32))
                    }
                    "$sformatf" => {
                        if args.is_empty() {
                            return Ok(LogicVec::new(0));
                        }
                        let msg = self.format_display(args);
                        let mut bits = Vec::with_capacity(msg.len() * 8);
                        for c in msg.chars() {
                            let byte = c as u8;
                            for i in 0..8 {
                                bits.push(if (byte >> i) & 1 == 1 {
                                    LogicVal::One
                                } else {
                                    LogicVal::Zero
                                });
                            }
                        }
                        Ok(LogicVec {
                            width: bits.len(),
                            bits,
                        })
                    }
                    "$clog2" => {
                        if let Some(arg) = args.first() {
                            let val = self.evaluate_expr(arg)?;
                            let n = val.to_u64();
                            if n <= 1 {
                                Ok(LogicVec::from_u64(0, 32))
                            } else {
                                let bits = (64 - n.leading_zeros()) as u64;
                                if n.is_power_of_two() {
                                    Ok(LogicVec::from_u64(bits - 1, 32))
                                } else {
                                    Ok(LogicVec::from_u64(bits, 32))
                                }
                            }
                        } else {
                            Ok(LogicVec::from_u64(0, 32))
                        }
                    }
                    "$time" => Ok(LogicVec::from_u64(self.state.time, 64)),
                    // $stime = waktu simulasi 32-bit (IEEE 1800 §20.17); $time
                    // 64-bit. Sebelumnya $stime tak dikenal → 0 (probe x01).
                    "$stime" => Ok(LogicVec::from_u64(self.state.time & 0xFFFF_FFFF, 32)),
                    "$realtime" => {
                        let t = self.state.time as f64;
                        Ok(LogicVec::from_u64(t.to_bits(), 64))
                    }
                    "process::self" => {
                        let pid = self.current_process_id.unwrap_or(0);
                        if pid == 0 {
                            let pid = self.state.alloc_object("__process".into());
                            self.process_map.insert(
                                pid,
                                ProcessInfo {
                                    status: ProcessStatus::Running,
                                    await_continuations: Vec::new(),
                                },
                            );
                            self.current_process_id = Some(pid);
                        }
                        Ok(LogicVec::from_u64(
                            self.current_process_id.unwrap_or(0) as u64,
                            64,
                        ))
                    }
                    "run_test" => {
                        // F18: run_test("name") — buat objek test & jalankan
                        // fase UVM. No-op bila execute_phases (auto-detect di
                        // run()) sudah menjalankan fase (guard uvm_phases_started
                        // di run_uvm_test).
                        let arg_vals: Vec<LogicVec> = args
                            .iter()
                            .map(|a| self.evaluate_expr(a))
                            .collect::<Result<_, _>>()?;
                        let test_name =
                            arg_vals.first().map(logicvec_to_string).unwrap_or_default();
                        self.run_uvm_test(&test_name)?;
                        Ok(LogicVec::from_u64(1, 1))
                    }
                    "uvm_test_top" => {
                        // F18: handle global root test UVM — obj id dari
                        // run_test/execute_phases; 0 (null) bila tidak ada.
                        Ok(LogicVec::from_u64(
                            self.root_test_obj_id.unwrap_or(0) as u64,
                            64,
                        ))
                    }
                    "uvm_root::get" => {
                        // VERIF-04: singleton uvm_root — semua get() → obj id
                        // sama (pola sama dengan uvm_cmdline_processor::get).
                        let obj_id = match self.uvm_root_id {
                            Some(id) => id,
                            None => {
                                let id = self.state.alloc_object(Symbol::intern("uvm_root"));
                                self.uvm_root_id = Some(id);
                                id
                            }
                        };
                        Ok(LogicVec::from_u64(obj_id as u64, 64))
                    }
                    "uvm_root::get_top" => {
                        // VERIF-04: komponen top (uvm_test_top) — obj id dari
                        // run_test/execute_phases; 0 (null) bila tidak ada.
                        Ok(LogicVec::from_u64(
                            self.root_test_obj_id.unwrap_or(0) as u64,
                            64,
                        ))
                    }
                    "uvm_root::run_test" => {
                        // VERIF-04: varian class-method run_test("name") —
                        // sama dgn bare run_test (F18).
                        let arg_vals: Vec<LogicVec> = args
                            .iter()
                            .map(|a| self.evaluate_expr(a))
                            .collect::<Result<_, _>>()?;
                        let test_name =
                            arg_vals.first().map(logicvec_to_string).unwrap_or_default();
                        self.run_uvm_test(&test_name)?;
                        Ok(LogicVec::from_u64(1, 1))
                    }
                    "uvm_tr_database::get_db" => {
                        // VERIF-18: singleton uvm_tr_database — semua get_db()
                        // → obj id sama (pola sama dgn uvm_root::get).
                        let obj_id = match self.uvm_tr_db_id {
                            Some(id) => id,
                            None => {
                                let id = self.state.alloc_object(Symbol::intern("uvm_tr_database"));
                                self.uvm_tr_db_id = Some(id);
                                id
                            }
                        };
                        Ok(LogicVec::from_u64(obj_id as u64, 64))
                    }
                    "uvm_tr_database::get_stream" => {
                        // VERIF-18: get_stream(name) — stream obj id (create/
                        // reuse), pola sama dgn method dispatch.
                        let arg_vals: Vec<LogicVec> = args
                            .iter()
                            .map(|a| self.evaluate_expr(a))
                            .collect::<Result<_, _>>()?;
                        let stream_name =
                            arg_vals.first().map(logicvec_to_string).unwrap_or_default();
                        let id = self.tr_stream_get(&stream_name);
                        Ok(LogicVec::from_u64(id as u64, 64))
                    }
                    "uvm_tr_database::get_tr_count" => {
                        // VERIF-18: jumlah record transaksi (semua stream).
                        Ok(LogicVec::from_u64(self.tr_records.len() as u64, 64))
                    }
                    "uvm_tr_database::set_stream" => {
                        // VERIF-18: set_stream(name) — stream default db.
                        let arg_vals: Vec<LogicVec> = args
                            .iter()
                            .map(|a| self.evaluate_expr(a))
                            .collect::<Result<_, _>>()?;
                        let stream_name =
                            arg_vals.first().map(logicvec_to_string).unwrap_or_default();
                        self.tr_stream_get(&stream_name);
                        self.tr_db_default_stream = Some(stream_name);
                        Ok(LogicVec::from_u64(0, 64))
                    }
                    "uvm_config_db::set" => {
                        let arg_vals: Vec<LogicVec> = args
                            .iter()
                            .map(|a| self.evaluate_expr(a))
                            .collect::<Result<_, _>>()?;
                        let inst_name = if arg_vals.len() > 1 {
                            logicvec_to_string(&arg_vals[1])
                        } else {
                            String::new()
                        };
                        let field_name = if arg_vals.len() > 2 {
                            logicvec_to_string(&arg_vals[2])
                        } else {
                            String::new()
                        };
                        let value = if arg_vals.len() > 3 {
                            arg_vals[3].clone()
                        } else {
                            LogicVec::new(1)
                        };
                        self.uvm_config_db_data
                            .insert((inst_name.clone(), field_name.clone()), value);
                        // VERIF-06: wait_modified ter-blokir menunggu key ini.
                        self.config_db_release_waiters(&inst_name, &field_name)?;
                        Ok(LogicVec::from_u64(1, 1))
                    }
                    // VERIF-06: exists(inst, field) → 1/0 (non-blocking).
                    "uvm_config_db::exists" => {
                        let arg_vals: Vec<LogicVec> = args
                            .iter()
                            .map(|a| self.evaluate_expr(a))
                            .collect::<Result<_, _>>()?;
                        let mut inst_name = if arg_vals.len() > 1 {
                            logicvec_to_string(&arg_vals[1])
                        } else {
                            String::new()
                        };
                        let field_name = if arg_vals.len() > 2 {
                            logicvec_to_string(&arg_vals[2])
                        } else {
                            String::new()
                        };
                        if inst_name.is_empty() {
                            if let Some(oid) = self.current_this {
                                inst_name = self.uvm_object_full_path(oid);
                            }
                        }
                        Ok(LogicVec::from_u64(
                            if self.config_db_exists(&inst_name, &field_name) {
                                1
                            } else {
                                0
                            },
                            1,
                        ))
                    }
                    // VERIF-06: wait_modified — kondisi terkini (blocking
                    // di-intercept block.rs; di sini query saja).
                    "uvm_config_db::wait_modified" => {
                        let arg_vals: Vec<LogicVec> = args
                            .iter()
                            .map(|a| self.evaluate_expr(a))
                            .collect::<Result<_, _>>()?;
                        let inst_name = if arg_vals.len() > 1 {
                            logicvec_to_string(&arg_vals[1])
                        } else {
                            String::new()
                        };
                        let field_name = if arg_vals.len() > 2 {
                            logicvec_to_string(&arg_vals[2])
                        } else {
                            String::new()
                        };
                        Ok(LogicVec::from_u64(
                            if self.config_db_exists(&inst_name, &field_name) {
                                1
                            } else {
                                0
                            },
                            1,
                        ))
                    }
                    "uvm_config_db::get" => {
                        let arg_vals: Vec<LogicVec> = args
                            .iter()
                            .map(|a| self.evaluate_expr(a))
                            .collect::<Result<_, _>>()?;
                        let mut inst_name = if arg_vals.len() > 1 {
                            logicvec_to_string(&arg_vals[1])
                        } else {
                            String::new()
                        };
                        let field_name = if arg_vals.len() > 2 {
                            logicvec_to_string(&arg_vals[2])
                        } else {
                            String::new()
                        };
                        // F19: inst_path kosong (`get(this, "", ...)`) →
                        // resolve ke path hierarki penuh objek saat ini.
                        if inst_name.is_empty() {
                            if let Some(oid) = self.current_this {
                                inst_name = self.uvm_object_full_path(oid);
                            }
                        }
                        // F19: exact match menang, lalu wildcard paling spesifik.
                        let stored = self.config_db_find(&inst_name, &field_name);
                        if let Some(val) = stored {
                            if let Some(IrExpr::Signal(sig_id, _)) = args.get(3) {
                                self.state.write_signal(*sig_id, val);
                            }
                            Ok(LogicVec::from_u64(1, 1))
                        } else {
                            Ok(LogicVec::from_u64(0, 1))
                        }
                    }
                    "uvm_resource_db::set" => {
                        let arg_vals: Vec<LogicVec> = args
                            .iter()
                            .map(|a| self.evaluate_expr(a))
                            .collect::<Result<_, _>>()?;
                        let scope = if !arg_vals.is_empty() {
                            logicvec_to_string(&arg_vals[0])
                        } else {
                            String::new()
                        };
                        let name = if arg_vals.len() > 1 {
                            logicvec_to_string(&arg_vals[1])
                        } else {
                            String::new()
                        };
                        let value = if arg_vals.len() > 2 {
                            arg_vals[2].clone()
                        } else {
                            LogicVec::new(1)
                        };
                        self.uvm_resource_db_data.insert((scope, name), value);
                        Ok(LogicVec::from_u64(1, 1))
                    }
                    "uvm_resource_db::write_by_name" => {
                        // VERIF-07: alias set dgn arg ke-4 rw access type (diabaikan).
                        let arg_vals: Vec<LogicVec> = args
                            .iter()
                            .map(|a| self.evaluate_expr(a))
                            .collect::<Result<_, _>>()?;
                        let scope = if !arg_vals.is_empty() {
                            logicvec_to_string(&arg_vals[0])
                        } else {
                            String::new()
                        };
                        let name = if arg_vals.len() > 1 {
                            logicvec_to_string(&arg_vals[1])
                        } else {
                            String::new()
                        };
                        let value = if arg_vals.len() > 2 {
                            arg_vals[2].clone()
                        } else {
                            LogicVec::new(1)
                        };
                        self.uvm_resource_db_data.insert((scope, name), value);
                        Ok(LogicVec::from_u64(1, 1))
                    }
                    "uvm_resource_db::get" | "uvm_resource_db::read_by_name" => {
                        // VERIF-07: lookup exact dulu, lalu wildcard scope paling
                        // spesifik (sama dgn config_db) — `set("*.env", ...)`
                        // terbaca oleh `get("tb.env", ...)`. read_by_name = alias
                        // get (arg ke-4 rw access type diabaikan).
                        let arg_vals: Vec<LogicVec> = args
                            .iter()
                            .map(|a| self.evaluate_expr(a))
                            .collect::<Result<_, _>>()?;
                        let scope = if !arg_vals.is_empty() {
                            logicvec_to_string(&arg_vals[0])
                        } else {
                            String::new()
                        };
                        let rname = if arg_vals.len() > 1 {
                            logicvec_to_string(&arg_vals[1])
                        } else {
                            String::new()
                        };
                        let stored = self.resource_db_find(&scope, &rname);
                        if let Some(val) = stored {
                            if let Some(IrExpr::Signal(sig_id, _)) = args.get(2) {
                                self.state.write_signal(*sig_id, val);
                            }
                            Ok(LogicVec::from_u64(1, 1))
                        } else {
                            Ok(LogicVec::from_u64(0, 1))
                        }
                    }
                    "uvm_resource_db::exists" => {
                        // VERIF-07: 1 bila resource (scope, name) tersedia
                        // (exact atau wildcard paling spesifik), 0 bila tidak.
                        let arg_vals: Vec<LogicVec> = args
                            .iter()
                            .map(|a| self.evaluate_expr(a))
                            .collect::<Result<_, _>>()?;
                        let scope = if !arg_vals.is_empty() {
                            logicvec_to_string(&arg_vals[0])
                        } else {
                            String::new()
                        };
                        let rname = if arg_vals.len() > 1 {
                            logicvec_to_string(&arg_vals[1])
                        } else {
                            String::new()
                        };
                        Ok(LogicVec::from_u64(
                            if self.resource_db_exists(&scope, &rname) {
                                1
                            } else {
                                0
                            },
                            1,
                        ))
                    }
                    "uvm_cmdline_processor::get" => {
                        // VERIF-03: singleton uvm_cmdline_processor. Return
                        // handle objek (obj_id) — variabel `cl` menyimpannya;
                        // method has_plusarg/get_arg_value di-dispatch via
                        // execute_uvm_cmdline_method (class uvm_cmdline_processor).
                        let obj_id = match self.uvm_cmdline_id {
                            Some(id) => id,
                            None => {
                                let id = self
                                    .state
                                    .alloc_object(Symbol::intern("uvm_cmdline_processor"));
                                self.uvm_cmdline_id = Some(id);
                                id
                            }
                        };
                        Ok(LogicVec::from_u64(obj_id as u64, 64))
                    }
                    "uvm_factory::set_type_override_by_type" => {
                        let arg_vals: Vec<LogicVec> = args
                            .iter()
                            .map(|a| self.evaluate_expr(a))
                            .collect::<Result<_, _>>()?;
                        let orig = if !arg_vals.is_empty() {
                            logicvec_to_string(&arg_vals[0])
                        } else {
                            String::new()
                        };
                        let override_type = if arg_vals.len() > 1 {
                            logicvec_to_string(&arg_vals[1])
                        } else {
                            String::new()
                        };
                        self.factory_type_overrides.insert(orig, override_type);
                        Ok(LogicVec::from_u64(1, 1))
                    }
                    "$test$plusargs" => {
                        if let Some(pattern) = args.first() {
                            if let Ok(pat_val) = self.evaluate_expr(pattern) {
                                let pat_str = logicvec_to_string(&pat_val);
                                for key in self.plusargs.keys() {
                                    if key.starts_with(&pat_str) {
                                        return Ok(LogicVec::from_u64(1, 32));
                                    }
                                }
                            }
                        }
                        Ok(LogicVec::from_u64(0, 32))
                    }
                    "$value$plusargs" => {
                        if let Some(pattern) = args.first() {
                            if let Ok(pat_val) = self.evaluate_expr(pattern) {
                                let pat_str = logicvec_to_string(&pat_val);
                                let plusarg_name = pat_str
                                    .split('%')
                                    .next()
                                    .unwrap_or(&pat_str)
                                    .trim_end_matches('=');
                                let plusargs = self.plusargs.clone();
                                for (key, val) in &plusargs {
                                    if key == plusarg_name {
                                        if let Some(var_arg) = args.get(1) {
                                            let num = if let Some(hex) = val
                                                .strip_prefix("0x")
                                                .or_else(|| val.strip_prefix("0X"))
                                            {
                                                u64::from_str_radix(hex, 16).unwrap_or(0)
                                            } else {
                                                val.parse::<u64>().unwrap_or(0)
                                            };
                                            let bits = LogicVec::from_u64(num, 32);
                                            if let IrExpr::Signal(id, _) = var_arg {
                                                self.state.write_signal(*id, bits);
                                            }
                                        }
                                        return Ok(LogicVec::from_u64(1, 32));
                                    }
                                }
                            }
                        }
                        Ok(LogicVec::from_u64(0, 32))
                    }
                    "$rose" => {
                        if let Some(arg) = args.first() {
                            let val = self.evaluate_expr(arg)?;
                            let key = format!("$rose({:?})", arg);
                            let prev = self
                                .sysfunc_prev
                                .entry(Symbol::intern(&key))
                                .or_insert_with(|| LogicVec::fill(LogicVal::Zero, val.width));
                            let rose =
                                !prev.to_bool().unwrap_or(false) && val.to_bool().unwrap_or(false);
                            *prev = val;
                            Ok(LogicVec::from_u64(if rose { 1 } else { 0 }, 1))
                        } else {
                            Ok(LogicVec::from_u64(0, 1))
                        }
                    }
                    "$fell" => {
                        if let Some(arg) = args.first() {
                            let val = self.evaluate_expr(arg)?;
                            let key = format!("$fell({:?})", arg);
                            let prev = self
                                .sysfunc_prev
                                .entry(Symbol::intern(&key))
                                .or_insert_with(|| LogicVec::fill(LogicVal::Zero, val.width));
                            let fell =
                                prev.to_bool().unwrap_or(false) && !val.to_bool().unwrap_or(false);
                            *prev = val;
                            Ok(LogicVec::from_u64(if fell { 1 } else { 0 }, 1))
                        } else {
                            Ok(LogicVec::from_u64(0, 1))
                        }
                    }
                    "$stable" => {
                        if let Some(arg) = args.first() {
                            let val = self.evaluate_expr(arg)?;
                            let key = format!("$stable({:?})", arg);
                            let prev = self
                                .sysfunc_prev
                                .entry(Symbol::intern(&key))
                                .or_insert_with(|| LogicVec::fill(LogicVal::Zero, val.width));
                            let stable = *prev == val;
                            *prev = val;
                            Ok(LogicVec::from_u64(if stable { 1 } else { 0 }, 1))
                        } else {
                            Ok(LogicVec::from_u64(0, 1))
                        }
                    }
                    "$changed" => {
                        if let Some(arg) = args.first() {
                            let val = self.evaluate_expr(arg)?;
                            let key = format!("$changed({:?})", arg);
                            let prev = self
                                .sysfunc_prev
                                .entry(Symbol::intern(&key))
                                .or_insert_with(|| LogicVec::fill(LogicVal::Zero, val.width));
                            let changed = *prev != val;
                            *prev = val;
                            Ok(LogicVec::from_u64(if changed { 1 } else { 0 }, 1))
                        } else {
                            Ok(LogicVec::from_u64(0, 1))
                        }
                    }
                    "$past" => {
                        if let Some(arg) = args.first() {
                            let val = self.evaluate_expr(arg)?;
                            let n = if args.len() > 1 {
                                if let Ok(nv) = self.evaluate_expr(&args[1]) {
                                    nv.to_u64().max(1) as usize
                                } else {
                                    1
                                }
                            } else {
                                1
                            };
                            let key = format!("$past({:?})", arg);
                            let hist = self
                                .sysfunc_history
                                .entry(Symbol::intern(&key))
                                .or_default();
                            hist.push_back(val);
                            // Cap riwayat ke n+1 entry terbaru — `$past` tidak
                            // pernah butuh data lebih tua dari kedalaman n,
                            // jadi jangan numpuk O(cycles) per call-site.
                            while hist.len() > n + 1 {
                                hist.pop_front();
                            }
                            if hist.len() > n {
                                let past = hist[hist.len() - 1 - n].clone();
                                Ok(past)
                            } else {
                                Ok(LogicVec::fill(LogicVal::Zero, hist[0].width))
                            }
                        } else {
                            Ok(LogicVec::from_u64(0, 32))
                        }
                    }
                    _ => {
                        // Dukungan bit↔real bit-pattern 64-bit: `$bitstoreal(x)`
                        // = interpret x sbg f64-bits, `$realtobits(x)` = f64-bits
                        // → nilai 64-bit sama (passthrough). Menghilangkan
                        // warning RT9003 spam utk float-ALU (aex_alu).
                        let fn_name = name.as_str();
                        if fn_name == "$bitstoreal" || fn_name == "$realtobits" {
                            if let Some(a) = args.first() {
                                return self.evaluate_expr(a);
                            }
                            return Ok(LogicVec::from_u64(0, 64));
                        }
                        // Try VPI registered system functions first
                        if crate::vpi::systf::call_registered_systf(fn_name, true) {
                            return Ok(LogicVec::from_u64(0, 32));
                        }
                        // F20: via DiagSink agar warning punya file:line:col.
                        // Dedup: emit SEKALI per nama fungsi (bukan per
                        // penggunaan — utk `$realtobits` di 4 assign harusnya
                        // 1 warning, bukan 4-20).
                        if self.notimpl_warned.insert(*name) {
                            self.emit_warning(
                                mivon_core::diagnostics::DiagCode::NotImplemented,
                                format!("unsupported system function '{}'", fn_name),
                            );
                        }
                        Ok(LogicVec::from_u64(0, 32))
                    }
                }
            }
            IrExpr::NewCall { class_name, args } => {
                let arg_vals: Vec<LogicVec> = args
                    .iter()
                    .map(|a| self.evaluate_expr(a))
                    .collect::<Result<_, _>>()?;
                // Check if this is a covergroup instantiation
                let is_cg = self
                    .design
                    .covergroups
                    .iter()
                    .any(|c| c.name == *class_name);
                let effective_name = if is_cg {
                    format!("__covergroup_{}", class_name)
                } else if let Some(override_type) =
                    self.factory_type_overrides.get(class_name.as_str())
                {
                    override_type.to_string()
                } else {
                    class_name.to_string()
                };
                let obj_id = self.state.alloc_object(Symbol::intern(&effective_name));
                if class_name == "__mailbox" {
                    self.mailbox_queues.insert(obj_id, VecDeque::new());
                    // LANG-24: `new(bound)` — simpan batas kapasitas bila argumen
                    // pertama diberikan (bounded mode). Absen = unbounded.
                    if let Some(bound) = arg_vals.first() {
                        let b = bound.to_u64() as usize;
                        if b > 0 {
                            self.mailbox_bounds.insert(obj_id, b);
                        }
                    }
                } else if class_name == "__semaphore" {
                    let init = if !arg_vals.is_empty() {
                        arg_vals[0].to_u64() as u32
                    } else {
                        0
                    };
                    self.semaphore_counts.insert(obj_id, init);
                } else if is_cg {
                    // Auto-sample covergroup immediately on new() — VERIF-28:
                    // instance obj id diteruskan utk per-instance tracking.
                    self.sample_covergroup(class_name.as_str(), Some(obj_id))?;
                } else if !class_name.is_empty() {
                    if let Some(cls) = self.design.classes.get(class_name) {
                        if let Some(obj) = self.state.get_object_mut(obj_id) {
                            for field in &cls.fields {
                                obj.fields
                                    .entry(field.name)
                                    .or_insert_with(|| LogicVec::from_u64(0, field.width));
                            }
                        }
                    }
                    if self.is_uvm_object_hierarchy(class_name.as_str()) {
                        self.uvm_object_data
                            .entry(obj_id)
                            .or_insert_with(|| UvmObjectData {
                                name: String::new(),
                            });
                    }
                    if self.is_uvm_analysis_port_hierarchy(class_name.as_str()) {
                        let pname = if !arg_vals.is_empty() {
                            logicvec_to_string(&arg_vals[0])
                        } else {
                            String::new()
                        };
                        self.uvm_analysis_port_data
                            .entry(obj_id)
                            .or_insert_with(|| UvmAnalysisPortData {
                                connections: Vec::new(),
                                name: pname.clone(),
                            });
                        self.uvm_object_data
                            .entry(obj_id)
                            .or_insert_with(|| UvmObjectData { name: pname });
                    }
                    if self.is_uvm_analysis_imp_hierarchy(class_name.as_str()) {
                        let pname = if !arg_vals.is_empty() {
                            logicvec_to_string(&arg_vals[0])
                        } else {
                            String::new()
                        };
                        let parent_obj = arg_vals.get(1).map(|a| a.to_u64() as ObjId).unwrap_or(0);
                        self.uvm_analysis_imp_data.entry(obj_id).or_insert_with(|| {
                            UvmAnalysisImpData {
                                parent: if parent_obj != 0 {
                                    Some(parent_obj)
                                } else {
                                    None
                                },
                                name: pname.clone(),
                            }
                        });
                        self.uvm_object_data
                            .entry(obj_id)
                            .or_insert_with(|| UvmObjectData { name: pname });
                    }
                    if self.is_uvm_component_hierarchy(class_name.as_str()) {
                        let name = logicvec_to_string(&arg_vals[0]);
                        let parent_obj = arg_vals.get(1).map(|a| a.to_u64() as ObjId).unwrap_or(0);
                        self.uvm_object_data
                            .insert(obj_id, UvmObjectData { name: name.clone() });
                        let mut cd = UvmComponentData {
                            parent: None,
                            children: Vec::new(),
                            report_verbosity: crate::simulator::engine::uvm::object::UVM_MEDIUM,
                        };
                        if parent_obj != 0 {
                            cd.parent = Some(parent_obj);
                            if let Some(pd) = self.uvm_component_data.get_mut(&parent_obj) {
                                pd.children.push(obj_id);
                            }
                        }
                        self.uvm_component_data.insert(obj_id, cd);
                    }
                    // F24: guard hierarchy UVM di jalur IR (module initial) —
                    // helper sama dengan jalur AST (F21-F24). Builtin UVM punya
                    // `methods: vec![]` → find_method_in_hierarchy("new") gagal
                    // → data tak pernah di-insert. Tanpa ini `drv = new(...)`
                    // di module initial membuat driver tanpa uvm_driver_data.
                    if self.uvm_needs_new_dispatch(class_name.as_str()) {
                        self.execute_method(obj_id, "new", &arg_vals)?;
                    }
                }
                Ok(LogicVec::from_u64(obj_id as u64, 64))
            }
            IrExpr::This => {
                if let Some(obj_id) = self.current_this {
                    Ok(LogicVec::from_u64(obj_id as u64, 64))
                } else {
                    Err(self.diag_error(
                        mivon_core::diagnostics::DiagCode::NullHandle,
                        "'this' used outside of class method",
                    ))
                }
            }
            IrExpr::MethodCall {
                obj,
                method,
                args,
                with_clause,
            } => {
                // LANG-33: `obj.<constraint_block>.constraint_mode(0/1)` —
                // set/query mode constraint block (IEEE 1800-2017 §18.5.12).
                // Di-intercept SEBELUM evaluasi obj (field block bukan data
                // field — evaluasi sebagai MemberAccess akan error).
                if method.as_str() == "constraint_mode" {
                    if let IrExpr::MemberAccess { obj: inner, field } = obj.as_ref() {
                        let obj_val = self.evaluate_expr(inner)?;
                        let obj_id = obj_val.to_u64() as ObjId;
                        // LANG-32: block STATIC — constraint_mode() berlaku
                        // global untuk SEMUA instance class (§18.5.10), jadi
                        // set/query di static_constraint_modes (key class+block).
                        let class_sym = self
                            .state
                            .objects
                            .get(obj_id)
                            .map(|o| o.class_name)
                            .unwrap_or(Symbol::EMPTY);
                        let is_static = self
                            .design
                            .classes
                            .get(&class_sym)
                            .map(|cd| cd.constraints.iter().any(|(bn, st, _)| bn == field && *st))
                            .unwrap_or(false);
                        if let Some(arg) = args.first() {
                            let mode = self.evaluate_expr(arg)?.to_u64() != 0;
                            if is_static {
                                self.static_constraint_modes
                                    .insert((class_sym, *field), mode);
                            } else {
                                self.constraint_modes.insert((obj_id, *field), mode);
                            }
                            return Ok(LogicVec::from_u64(1, 1));
                        }
                        // Tanpa argumen: query mode saat ini (default enabled).
                        let mode =
                            self.constraint_block_enabled(obj_id, class_sym, *field, is_static);
                        return Ok(LogicVec::from_u64(if mode { 1 } else { 0 }, 1));
                    }
                }
                if let IrExpr::String(s) = obj.as_ref() {
                    let arg_vals: Vec<LogicVec> = args
                        .iter()
                        .map(|a| self.evaluate_expr(a))
                        .collect::<Result<_, _>>()?;
                    let result = evaluate_string_method(s.as_str(), method.as_str(), &arg_vals)?;
                    return Ok(result);
                }
                if let IrExpr::Signal(id, _) = obj.as_ref() {
                    if let Some(sig) = self.design.top.signals.get(*id) {
                        if sig.is_string {
                            let lv = self.state.read_signal(*id);
                            let s = logicvec_to_string(lv);
                            let arg_vals: Vec<LogicVec> = args
                                .iter()
                                .map(|a| self.evaluate_expr(a))
                                .collect::<Result<_, _>>()?;
                            let result = evaluate_string_method(&s, method.as_str(), &arg_vals)?;
                            return Ok(result);
                        }
                    }
                    if let Some(sig) = self.design.top.signals.get(*id) {
                        if let Some(ref cn) = sig.class_name {
                            let is_arr = sig.is_dynamic || sig.is_queue;
                            if !is_arr && !sig.is_string {
                                // Check if this class_name matches a covergroup or class
                                let is_cg = self.design.covergroups.iter().any(|c| c.name == *cn);
                                if is_cg || self.design.classes.contains_key(cn) {
                                    let obj_val = self.state.read_signal(*id);
                                    let obj_id = obj_val.to_u64() as ObjId;
                                    if obj_id == 0
                                        && !self.state.objects.is_empty()
                                        && self.state.objects[0].class_name.is_empty()
                                    {
                                        let class_for_obj = if is_cg {
                                            format!("__covergroup_{}", cn)
                                        } else {
                                            cn.to_string()
                                        };
                                        let new_id =
                                            self.state.alloc_object(Symbol::intern(&class_for_obj));
                                        self.state.write_signal(
                                            *id,
                                            LogicVec::from_u64(new_id as u64, 64),
                                        );
                                        let arg_vals: Vec<LogicVec> = args
                                            .iter()
                                            .map(|a| self.evaluate_expr(a))
                                            .collect::<Result<_, _>>()?;
                                        return self.execute_method(
                                            new_id,
                                            method.as_str(),
                                            &arg_vals,
                                        );
                                    }
                                }
                            }
                        }
                    }
                    let is_arr = self
                        .design
                        .top
                        .signals
                        .get(*id)
                        .map(|s| s.is_dynamic || s.is_queue)
                        .unwrap_or(false);
                    if is_arr {
                        let sig_info = self
                            .design
                            .top
                            .signals
                            .get(*id)
                            .cloned()
                            .unwrap_or_default();
                        return self.evaluate_array_method(
                            *id,
                            &sig_info,
                            method.as_str(),
                            args,
                            with_clause.as_deref(),
                        );
                    }
                }
                // F36: method call pada instance interface / hier instance yang
                // tidak punya method tersimulasi (`clk_if.set_period_ps(...)`,
                // `sck_clk.set_active()`) — receiver berupa HierRef yang tidak
                // resolve ke signal → no-op (return 0). Interface method DV
                // tidak di-model.
                if let IrExpr::HierRef(name) = obj.as_ref() {
                    if self.find_signal(name.as_str()).is_none() {
                        let _: Vec<LogicVec> = args
                            .iter()
                            .map(|a| self.evaluate_expr(a))
                            .collect::<Result<_, _>>()?;
                        return Ok(LogicVec::from_u64(0, 1));
                    }
                }
                let obj_val = self.evaluate_expr(obj)?;
                let obj_id = obj_val.to_u64() as ObjId;
                let arg_vals: Vec<LogicVec> = args
                    .iter()
                    .map(|a| self.evaluate_expr(a))
                    .collect::<Result<_, _>>()?;
                // Handle randomize() with inline constraint
                if method == "randomize" && with_clause.is_some() {
                    let class_name = self
                        .state
                        .get_object(obj_id)
                        .map(|o| o.class_name)
                        .unwrap_or_default();
                    return self.execute_randomize_with(
                        obj_id,
                        class_name.as_str(),
                        with_clause.as_deref(),
                    );
                }
                let result = self.execute_method(obj_id, method.as_str(), &arg_vals)?;
                Ok(result)
            }
            IrExpr::MemberAccess { obj, field } => {
                let obj_val = self.evaluate_expr(obj)?;
                let obj_id = obj_val.to_u64() as ObjId;
                // Handle OOB (stale id) → null default, bukan error (konsisten
                // dgn jalur AST; sim tidak boleh abort utk bug handle).
                let Some(obj_data) = self.state.get_object(obj_id) else {
                    return Ok(LogicVec::new(1));
                };
                let val = obj_data
                    .fields
                    .get(field)
                    .cloned()
                    .unwrap_or_else(|| LogicVec::new(1));
                Ok(val)
            }
            IrExpr::DpiCall {
                name,
                args,
                return_width,
            } => self.evaluate_dpi_call(name.as_str(), args, *return_width),
            IrExpr::HierRef(name) => {
                if let Some(sig_id) = self.find_signal(name.as_str()) {
                    let mut val = self.state.read_signal(sig_id).clone();
                    sanitize_for_2state(&self.design.top.signals, sig_id, &mut val);
                    Ok(val)
                } else {
                    Err(SimError::with_diag(
                        DiagCode::NullHandle,
                        format!("hierarchical signal '{}' not found", name),
                    ))
                }
            }
            IrExpr::InsideRange { expr, lo, hi } => {
                let val = self.evaluate_expr(expr)?;
                let lo_v = self.evaluate_expr(lo)?;
                let hi_v = self.evaluate_expr(hi)?;
                let v = val.to_u64();
                if v >= lo_v.to_u64() && v <= hi_v.to_u64() {
                    Ok(LogicVec::from_u64(1, 1))
                } else {
                    Ok(LogicVec::from_u64(0, 1))
                }
            }
            IrExpr::Inside { expr, list } => {
                let val = self.evaluate_expr(expr)?;
                for item in list {
                    // Range inside `{[a:b]}` — periksa lo <= val <= hi.
                    if let IrExpr::InsideRange { lo, hi, .. } = item {
                        let lo_v = self.evaluate_expr(lo)?;
                        let hi_v = self.evaluate_expr(hi)?;
                        let v = val.to_u64();
                        let lo_u = lo_v.to_u64();
                        let hi_u = hi_v.to_u64();
                        if v >= lo_u && v <= hi_u {
                            return Ok(LogicVec::from_u64(1, 1));
                        }
                        continue;
                    }
                    let item_val = self.evaluate_expr(item)?;
                    // Normalisasi lebar sebelum case_eq: `a[7:0] inside {20}`
                    // (8-bit vs literal 32-bit) — bits tidak sama walau nilai
                    // sama tanpa resize (sama seperti evaluator AST F12).
                    let w = val.width.max(item_val.width);
                    let eq = val.resize(w).case_eq(&item_val.resize(w));
                    if eq == LogicVec::from_u64(1, 1) {
                        return Ok(LogicVec::from_u64(1, 1));
                    }
                }
                Ok(LogicVec::from_u64(0, 1))
            }
            IrExpr::Dist { expr: _expr, items } => {
                // Dist expression in randomize context: use weighted random selection
                if self.current_method == Some(Symbol::intern("randomize")) {
                    let total_weight: i64 = items
                        .iter()
                        .map(|item| {
                            let count = match (item.range_lo, item.range_hi) {
                                (Some(lo), Some(hi)) if hi >= lo => (hi - lo + 1).max(1),
                                _ => 1,
                            };
                            match item.weight_type {
                                DistWeightType::Item => item.weight * count,
                                DistWeightType::Range => item.weight,
                            }
                        })
                        .sum();
                    if total_weight > 0 {
                        let r = (self.rng.gen::<u64>() % total_weight as u64) as i64;
                        let mut cumulative = 0i64;
                        for item in items {
                            let count = match (item.range_lo, item.range_hi) {
                                (Some(lo), Some(hi)) if hi >= lo => (hi - lo + 1).max(1),
                                _ => 1,
                            };
                            let step = match item.weight_type {
                                DistWeightType::Item => item.weight * count,
                                DistWeightType::Range => item.weight,
                            };
                            cumulative += step;
                            if r < cumulative {
                                let v = match (item.range_lo, item.range_hi) {
                                    (Some(lo), Some(hi)) if hi >= lo => {
                                        lo + (self.rng.gen::<u64>() % ((hi - lo + 1) as u64)) as i64
                                    }
                                    (Some(v), _) | (_, Some(v)) => v,
                                    _ => 0i64,
                                };
                                return Ok(LogicVec::from_u64(v as u64, 32));
                            }
                        }
                    }
                }
                Ok(LogicVec::from_u64(1, 1))
            }
            IrExpr::Cast { width, expr } => {
                let val = self.evaluate_expr(expr)?;
                // Size-cast mempertahankan signedness operand (LRM §6.24.1):
                // widening operand SIGNED → sign-extend; unsigned → zero-extend.
                // Dulu selalu `resize` (zero-extend) → `sn + 1` (sn signed
                // [7:0]) mem-bungkus operand jadi Cast{32} 0x000000FC → Add
                // jadi 253 padahal -3 (probe q15).
                if *width > val.width && is_signed_expr(expr, &self.design.top.signals) {
                    let msb = val.bits.last().copied().unwrap_or(LogicVal::Zero);
                    let mut out = val;
                    out.bits.resize(*width, msb);
                    out.width = *width;
                    Ok(out)
                } else {
                    Ok(val.resize(*width))
                }
            }
            IrExpr::StreamingConcat {
                op,
                slice_size,
                slices,
            } => {
                let mut vals = Vec::new();
                for sl in slices {
                    vals.push(self.evaluate_expr(sl)?);
                }
                let all_bits: Vec<LogicVal> =
                    vals.iter().flat_map(|v| v.bits.iter().copied()).collect();
                let slen = slice_size.unwrap_or(1);
                if slen == 0 {
                    return Err(self.diag_error(
                        mivon_core::diagnostics::DiagCode::MemoryOutOfBounds,
                        "streaming slice size must be > 0",
                    ));
                }
                let mut result = Vec::new();
                if op == ">>" {
                    // Right streaming: reverse slice order only (same as <<).
                    // Direction only matters for padding on non-multiple widths.
                    for chunk in all_bits.chunks(slen).rev() {
                        result.extend(chunk.iter());
                    }
                } else {
                    // reverse slice order only
                    for chunk in all_bits.chunks(slen).rev() {
                        result.extend(chunk.iter());
                    }
                }
                Ok(LogicVec {
                    width: result.len(),
                    bits: result,
                })
            }
            IrExpr::UdpLookup { udp_name, args } => {
                let udp = self
                    .design
                    .udp_defs
                    .iter()
                    .find(|u| u.name == *udp_name)
                    .cloned()
                    .ok_or_else(|| format!("UDP '{}' not found", udp_name))?;
                let arg_vals: Vec<LogicVec> = args
                    .iter()
                    .map(|a| self.evaluate_expr(a))
                    .collect::<Result<_, _>>()?;

                // Get previous arg values for edge detection
                let prev_vals = self.udp_prev_args.get(udp_name);
                let current_bits: Vec<LogicVal> = arg_vals
                    .iter()
                    .map(|v| v.bits.first().copied().unwrap_or(LogicVal::X))
                    .collect();
                let prev_bits: Option<Vec<LogicVal>> = prev_vals.map(|pv| {
                    pv.iter()
                        .map(|v| v.bits.first().copied().unwrap_or(LogicVal::X))
                        .collect()
                });

                // Scan table entries for first match
                'table: for entry in &udp.table {
                    for (i, sym) in entry.inputs.iter().enumerate() {
                        let bit = current_bits.get(i).copied().unwrap_or(LogicVal::X);
                        let matched = match sym {
                            UdpSymbol::Zero => bit == LogicVal::Zero,
                            UdpSymbol::One => bit == LogicVal::One,
                            UdpSymbol::X => bit == LogicVal::X,
                            UdpSymbol::DontCare => true,
                            UdpSymbol::Edge(edge_str) => {
                                // Edge detection: compare prev vs current
                                if let Some(ref pb) = prev_bits {
                                    let prev_bit = pb.get(i).copied().unwrap_or(LogicVal::X);
                                    let chars: Vec<char> = edge_str.chars().collect();
                                    if chars.len() == 2 {
                                        sym_char_matches(chars[0], prev_bit)
                                            && sym_char_matches(chars[1], bit)
                                    } else {
                                        // Abbreviated edge: r, f, p, n, *
                                        edge_matches_abbrev(edge_str.as_str(), prev_bit, bit)
                                    }
                                } else {
                                    // No previous value — can't detect edge
                                    false
                                }
                            }
                            UdpSymbol::NoChange => true,
                        };
                        if !matched {
                            continue 'table;
                        }
                    }
                    // All inputs matched — determine output
                    let result = match &entry.output {
                        UdpSymbol::Zero => LogicVec::fill(LogicVal::Zero, 1),
                        UdpSymbol::One => LogicVec::fill(LogicVal::One, 1),
                        UdpSymbol::X => LogicVec::fill(LogicVal::X, 1),
                        UdpSymbol::DontCare => LogicVec::fill(LogicVal::X, 1),
                        UdpSymbol::NoChange => {
                            // For sequential UDP, return the current output value (last arg = state)
                            arg_vals
                                .last()
                                .cloned()
                                .unwrap_or(LogicVec::fill(LogicVal::X, 1))
                        }
                        UdpSymbol::Edge(s) => {
                            let v = s
                                .chars()
                                .last()
                                .map(|c| match c {
                                    '0' => LogicVal::Zero,
                                    '1' => LogicVal::One,
                                    _ => LogicVal::X,
                                })
                                .unwrap_or(LogicVal::X);
                            LogicVec::fill(v, 1)
                        }
                    };
                    // Store current arg values for next evaluation
                    self.udp_prev_args.insert(*udp_name, arg_vals.clone());
                    return Ok(result);
                }
                // No match — return X (or retain current value for sequential)
                let result = if udp.is_sequential {
                    arg_vals
                        .last()
                        .cloned()
                        .unwrap_or(LogicVec::fill(LogicVal::X, 1))
                } else {
                    LogicVec::fill(LogicVal::X, 1)
                };
                self.udp_prev_args.insert(*udp_name, arg_vals.clone());
                Ok(result)
            }
            IrExpr::FuncCall { func_name, args } => {
                // F35: dispatch ke helper runtime function bersama — dipakai
                // juga oleh jalur AST (ast.rs) sehingga pemanggilan REKURSIF
                // di dalam body function (yang dieksekusi via AST eval) tetap
                // mengenali module function. Sebelumnya jalur AST fallback
                // → RT9003 "unknown function" → hasil 0 (siluman).
                let arg_vals: Vec<LogicVec> = args
                    .iter()
                    .map(|a| self.evaluate_expr(a))
                    .collect::<Result<_, _>>()?;
                self.execute_module_function_call(func_name, &arg_vals)
            }
            IrExpr::VifBinding { instance_name } => {
                // Look up the instance in the signal hierarchy
                // Find the first signal belonging to this instance and return its SignalId as binding handle
                let mut binding_handle: Option<usize> = None;
                let prefix = format!("{instance_name}.");
                for (sid, sig) in self.design.top.signals.iter().enumerate() {
                    if sig.name.starts_with(&prefix) || sig.name == *instance_name {
                        binding_handle = Some(sid);
                        break;
                    }
                }
                if let Some(handle) = binding_handle {
                    return Ok(LogicVec::from_u64(handle as u64, 64));
                }
                // Fallback: match instance name as any path component: top.instance.sig
                let target = instance_name.as_str();
                for (sid, sig) in self.design.top.signals.iter().enumerate() {
                    let parts: Vec<&str> = sig.name.as_str().split('.').collect();
                    if parts.contains(&target) {
                        binding_handle = Some(sid);
                        break;
                    }
                }
                match binding_handle {
                    Some(handle) => Ok(LogicVec::from_u64(handle as u64, 64)),
                    None => Ok(LogicVec::fill(LogicVal::X, 64)),
                }
            }
            IrExpr::VirtualIfaceAccess {
                vif_name,
                field,
                field_width,
            } => {
                // Find the vif signal and read its binding handle (SignalId of a signal in the bound instance)
                let mut result = LogicVec::fill(LogicVal::X, *field_width);
                for (sid, sig) in self.design.top.signals.iter().enumerate() {
                    if sig.iface_type.is_some() && sig.name == *vif_name {
                        let binding_val = self.state.read_signal(sid);
                        let handle = binding_val.to_u64() as usize;
                        if handle > 0 && handle < self.design.top.signals.len() {
                            // Bound — extract instance path from the bound signal's name
                            let bound_sig_name = self
                                .design
                                .top
                                .signals
                                .get(handle)
                                .map(|s| s.name.as_str())
                                .unwrap_or("<out-of-bounds>");
                            // Strip the signal name to get instance path: top.inst.sig -> top.inst
                            if let Some(dot_pos) = bound_sig_name.rfind('.') {
                                let inst_path = &bound_sig_name[..dot_pos];
                                let sig_key = format!("{}.{}", inst_path, field);
                                if let Some(&field_sid) =
                                    self.design.hier_signal_map.get(&Symbol::intern(&sig_key))
                                {
                                    result = self.state.read_signal(field_sid).clone();
                                }
                            }
                        }
                        break;
                    }
                }
                Ok(result)
            }
        }
    }

    /// Compute the expected result width for an IrExpr tree (used by JIT expression eval).
    /// For comparison operations (Eq, Neq, Lt, Le, Gt, Ge), returns 1.
    /// For other operations, returns the max width of all operands/signals/constants.
    fn compute_jit_expr_width(&self, expr: &IrExpr) -> usize {
        match expr {
            IrExpr::Const(lv) => lv.width,
            IrExpr::FillLit(_) => 1,
            IrExpr::Signal(_, w) => *w,
            IrExpr::BinaryOp(op, lhs, rhs) => {
                let lw = self.compute_jit_expr_width(lhs);
                let rw = self.compute_jit_expr_width(rhs);
                match op {
                    BinaryIrOp::Eq
                    | BinaryIrOp::Neq
                    | BinaryIrOp::CaseEq
                    | BinaryIrOp::CaseNeq
                    | BinaryIrOp::Lt
                    | BinaryIrOp::Le
                    | BinaryIrOp::Gt
                    | BinaryIrOp::Ge
                    | BinaryIrOp::EqWild
                    | BinaryIrOp::NeqWild => 1,
                    _ => lw.max(rw),
                }
            }
            IrExpr::UnaryOp(_, inner) => self.compute_jit_expr_width(inner),
            IrExpr::Cond(_, t, f) => {
                let tw = self.compute_jit_expr_width(t);
                let fw = self.compute_jit_expr_width(f);
                tw.max(fw)
            }
            IrExpr::Signed(inner) => self.compute_jit_expr_width(inner),
            _ => 64, // Complex expression types: default to 64-bit
        }
    }

    /// F35: eksekusi function module-level (recursive, tidak di-inline) secara
    /// runtime. Helper BERSAMA untuk jalur IR (`IrExpr::FuncCall` di sini) dan
    /// jalur AST (`Expr::FuncCall` di eval/ast.rs) — pemanggilan REKURSIF di
    /// dalam body function dieksekusi via AST eval (`evaluate_ast_block_with_delay_fork`),
    /// jadi jalur AST juga harus mengenali module function. Sebelumnya jalur
    /// AST fallback → RT9003 "unknown function" → hasil 0 (bug siluman).
    /// Body function dieksekusi via AST (bukan IR) agar `return`/local tetap
    /// bekerja — pola yang sama dengan class method runtime.
    pub(crate) fn execute_module_function_call(
        &mut self,
        func_name: &Symbol,
        arg_vals: &[LogicVec],
    ) -> Result<LogicVec, SimError> {
        let name = func_name;
        // Check recursion depth
        let depth = self.recursion_depth.get(name).copied().unwrap_or(0);
        if depth >= self.max_recursion_depth {
            return Err(self.diag_error(
                mivon_core::diagnostics::DiagCode::InternalError,
                format!(
                    "recursion depth exceeded for function '{}' (max {})",
                    name, self.max_recursion_depth
                ),
            ));
        }
        self.recursion_depth.insert(*name, depth + 1);

        // Find the function declaration
        let func = self
            .design
            .module_functions
            .get(name)
            .cloned()
            .ok_or_else(|| {
                self.diag_error(
                    mivon_core::diagnostics::DiagCode::DpiError,
                    format!("function '{}' not found for runtime call", name),
                )
            })?;

        // Compute return width from function declaration
        let ret_width = if let Some(er) = &func.range {
            if let (Ok(msb), Ok(lsb)) = (
                mivon_ast::types::const_eval_simple(&er.msb),
                mivon_ast::types::const_eval_simple(&er.lsb),
            ) {
                let msb = msb as usize;
                let lsb = lsb as usize;
                if msb >= lsb {
                    msb - lsb + 1
                } else {
                    lsb - msb + 1
                }
            } else {
                1
            }
        } else {
            match &func.return_type {
                Some(dt) => match dt.as_ref() {
                    mivon_ast::types::DataType::Void => 0,
                    mivon_ast::types::DataType::Byte => 8,
                    mivon_ast::types::DataType::Shortint => 16,
                    mivon_ast::types::DataType::Int | mivon_ast::types::DataType::Integer => 32,
                    mivon_ast::types::DataType::Longint => 64,
                    mivon_ast::types::DataType::Time => 64,
                    _ => 1,
                },
                None => 1,
            }
        };

        // Create new local scope
        let depth_idx = self.method_locals.len();
        let mut locals = HashMap::new();

        // F35: pre-insert nama function sebagai lokal agar LHS `fact = expr`
        // (gaya non-ANSI) menulis tanpa RT0001. `__func_ret` TIDAK pre-insert
        // — hanya Stmt::Return yang menulis ke `__func_ret` via set_local.
        // Tanpa pre-insert `__func_ret`, get_local("__func_ret") = None (fallback
        // ke name) untuk non-ANSI; untuk ANSI `return expr`, set_local membuat
        // entry baru di frame (set_local selalu insert).
        if ret_width > 0 {
            locals.insert(*name, LogicVec::new(ret_width.max(1)));
        }

        // Bind arguments to port names
        for (i, arg_val) in arg_vals.iter().enumerate() {
            if let Some(port) = func.ports.get(i) {
                locals.insert(port.name, arg_val.clone());
            }
        }

        // Initialize internal variables. Automatic: X setiap call (initializer
        // ditangani jalur inline; runtime path hanya untuk function recursive
        // / ber-static yang tidak di-inline). Static (IEEE 1800 §6.21):
        // inisialisasi SEKALI di store persisten `static_locals`, nilai
        // dipertahankan antar pemanggilan function/task yang sama.
        let mut static_init_jobs: Vec<(Symbol, usize)> = Vec::new();
        for decl in &func.decls {
            for var in &decl.names {
                if locals.contains_key(&var.name) {
                    continue;
                }
                let width = if let Some(r) = &var.range {
                    r.width()
                } else {
                    1
                };
                if var.is_static {
                    let key = (*name, var.name);
                    if let Some(existing) = self.static_locals.get(&key) {
                        locals.insert(var.name, existing.clone());
                    } else {
                        locals.insert(var.name, LogicVec::new(width));
                        static_init_jobs.push((var.name, width));
                    }
                } else {
                    locals.insert(var.name, LogicVec::new(width));
                }
            }
        }

        self.method_locals.push(locals);

        // Evaluasi initializer static lokal PERTAMA KALI (frame sudah aktif
        // agar ekspresi init bisa membaca argumen/port via get_local).
        for (var_name, width) in static_init_jobs {
            let mut init_val = LogicVec::new(width);
            for decl in &func.decls {
                if let Some(var) = decl.names.iter().find(|v| v.name == var_name) {
                    if let Some(init_expr) = &var.expr {
                        if let Ok(v) = self.evaluate_ast_expr(init_expr) {
                            init_val = v;
                        }
                    }
                    break;
                }
            }
            self.static_locals
                .insert((*name, var_name), init_val.clone());
            self.set_local(var_name.as_str(), init_val);
        }

        // Save and set current_method so Stmt::Return stores into method_locals
        let saved_method = self.current_method.take();
        self.current_method = Some(Symbol::intern("__func_ret"));

        // F35 review: simpan hasil body — state di-restore di SEMUA jalur
        // (sukses ATAU error). Sebelumnya `?` mempropagasi error sebelum
        // truncate/restore → frame method_locals + recursion_depth bocor dan
        // pemanggilan function berikutnya membaca frame stale (get_local
        // shadow argumen).
        let body_result = self.evaluate_ast_block_with_delay_fork(&func.stmts, None);

        // F35: `return` di body function menandai ast_return_pending utk
        // stop-blok lintas nested. Clear di sini (wrapper terluar) agar flag
        // tidak bocor ke evaluasi blok lain setelah function selesai.
        self.ast_return_pending = false;
        self.current_method = saved_method;

        // Simpan nilai static lokal saat ini kembali ke store persisten
        // (body bisa mengubah static var; nilai harus survive truncate frame).
        for decl in &func.decls {
            for var in &decl.names {
                if var.is_static {
                    if let Some(v) = self.get_local(var.name.as_str()) {
                        self.static_locals.insert((*name, var.name), v);
                    }
                }
            }
        }

        // Read return value from method_locals. `__func_ret` di-set oleh
        // Stmt::Return (gaya ANSI `return expr`); `name` di-set oleh LHS
        // `fact = expr` (gaya non-ANSI). Baca `__func_ret` DULU: `name`
        // selalu di-pre-insert sebagai 0 (slot return), jadi get_local(name)
        // short-circuit or_else dan menutupi nilai `__func_ret` yang asli —
        // itulah kenapa function gaya ANSI selalu return 0 (F35 fix 2).
        // Untuk non-ANSI `__func_ret` tidak pernah ada → fallback ke name.
        // PENTING: baca SEBELUM truncate frame (frame masih berisi nilai
        // return yang ditulis body).
        let return_val = if ret_width > 0 {
            self.get_local("__func_ret")
                .or_else(|| self.get_local(name.as_str()))
                .unwrap_or_else(|| LogicVec::new(ret_width))
        } else {
            LogicVec::new(0)
        };

        // Restore scope + depth di SEMUA jalur (sukses ATAU error).
        self.method_locals.truncate(depth_idx);
        self.recursion_depth.insert(*name, depth);

        body_result?;

        Ok(return_val)
    }
}

/// Potong operand sinyal ke lebar ASLI signal (sebelum resize konteks zero-
/// extend) — agar Div/Mod signed sign-extend dari lebar benar (bug #7).
fn signed_raw_operand(e: &IrExpr, v: &LogicVec, signals: &[SignalInfo]) -> LogicVec {
    // Cast/Signed wrapper — buka; lebar asli ada di inner signal.
    if let IrExpr::Signed(inner) = e {
        return signed_raw_operand(inner, v, signals);
    }
    if let IrExpr::Cast { expr: inner, .. } = e {
        return signed_raw_operand(inner, v, signals);
    }
    let w = match e {
        IrExpr::Signal(id, _) | IrExpr::BitSelect(id, _) | IrExpr::RangeSelect(id, ..) => signals
            .get(*id)
            .map(|s| s.width.min(v.width))
            .unwrap_or(v.width),
        _ => v.width,
    };
    let bits = v.bits.iter().take(w).cloned().collect();
    LogicVec {
        bits,
        width: w.max(1),
    }
}
