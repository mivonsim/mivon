//! Lowering IR → MIR + jalur evaluasi MIR JIT (compiled-code simulation path).
//!
//! 1 file = 1 tanggung jawab: hanya konversi `IrStmt`/`IrExpr` menjadi
//! instruksi `MirInstr` (`mivon_compiler::mir`) + eksekusi proses via
//! `MirJitCompiler`. Tidak ada logika scheduler/statement engine di sini —
//! pemanggil (`try_evaluate_mir_jit`) adalah `SimulationEngine::run` /
//! `scheduler/event.rs`; bila lowering gagal (ekspresi tidak JIT-safe),
//! pemanggil fallback ke interpreter.
//!
//! Daftar fungsi:
//! - `is_expr_jit_safe` / `is_body_jit_safe` — guard: hanya ekspresi/statement
//!   yang didukung yang boleh di-lower.
//! - `ir_expr_to_mir` / `ir_body_to_mir` (+ `_inner`) — lowering ekspresi dan
//!   blok statement (If/Case/LoopWhile/LoopDoWhile/LoopFor/Repeat/NBA).
//! - `try_evaluate_mir_jit` — compile process via `MirJitCompiler`, eksekusi
//!   kode native, dan aplikasi hasil (blocking → write langsung, NBA →
//!   `nba_pending`).

use super::SimulationEngine;
use mivon_compiler::mir::*;
use mivon_core::error::SimError;
use mivon_core::Symbol;
use mivon_ir::*;
use std::collections::HashSet;

impl SimulationEngine {
    /// Map Ir BinaryIrOp to MirBinOp (for MIR JIT compilation).
    fn ir_binop_to_mir(op: &mivon_ir::BinaryIrOp) -> MirBinOp {
        match op {
            mivon_ir::BinaryIrOp::Add => MirBinOp::Add,
            mivon_ir::BinaryIrOp::Sub => MirBinOp::Sub,
            mivon_ir::BinaryIrOp::Mul => MirBinOp::Mul,
            mivon_ir::BinaryIrOp::Div => MirBinOp::Div,
            mivon_ir::BinaryIrOp::Mod => MirBinOp::Mod,
            mivon_ir::BinaryIrOp::BitAnd => MirBinOp::And,
            mivon_ir::BinaryIrOp::BitOr => MirBinOp::Or,
            mivon_ir::BinaryIrOp::BitXor => MirBinOp::Xor,
            mivon_ir::BinaryIrOp::Eq
            | mivon_ir::BinaryIrOp::CaseEq
            | mivon_ir::BinaryIrOp::EqWild => MirBinOp::Eq,
            mivon_ir::BinaryIrOp::Neq
            | mivon_ir::BinaryIrOp::CaseNeq
            | mivon_ir::BinaryIrOp::NeqWild => MirBinOp::Ne,
            mivon_ir::BinaryIrOp::Lt => MirBinOp::Lt,
            mivon_ir::BinaryIrOp::Le => MirBinOp::Le,
            mivon_ir::BinaryIrOp::Gt => MirBinOp::Gt,
            mivon_ir::BinaryIrOp::Ge => MirBinOp::Ge,
            mivon_ir::BinaryIrOp::Shl => MirBinOp::Shl,
            mivon_ir::BinaryIrOp::Shr => MirBinOp::Shr,
            mivon_ir::BinaryIrOp::LogicalAnd => MirBinOp::LogicalAnd,
            mivon_ir::BinaryIrOp::LogicalOr => MirBinOp::LogicalOr,
            _ => MirBinOp::Add, // fallback
        }
    }

    /// Check if an IrExpr contains any unsupported variants that would produce
    /// incorrect results in the JIT path. Returns true if the expression can be JIT-compiled.
    fn is_expr_jit_safe(expr: &IrExpr, sig: &[SignalInfo]) -> bool {
        match expr {
            // Const dengan bit X/Z atau lebar > 64 tak bisa dipetakan ke u64
            // tanpa kehilangan informasi (to_u64: X/Z -> 0, lebar dipotong 64)
            // → tolak agar fallback ke interpreter.
            IrExpr::Const(lv) => !lv.has_unknown() && lv.width <= 64,
            IrExpr::Signal(_, _) => true,
            // FillLit ('0/'1) di-fill ke lebar LHS oleh interpreter; JIT
            // me-lower jadi Const 1-bit sehingga `y = '1` jadi 1, bukan
            // all-ones → tolak (JIT-13b).
            IrExpr::FillLit(_) => false,
            IrExpr::BinaryOp(op, lhs, rhs) => {
                Self::is_binop_jit_safe(op, rhs)
                    && Self::is_expr_jit_safe(lhs, sig)
                    && Self::is_expr_jit_safe(rhs, sig)
            }
            // Plus TIDAK didukung lowering (hanya BitNot/Minus) — tanpa
            // penolakan ini `y = +a` memicu unreachable!() (JIT-11b).
            IrExpr::UnaryOp(mivon_ir::UnaryIrOp::BitNot | mivon_ir::UnaryIrOp::Minus, inner) => {
                Self::is_expr_jit_safe(inner, sig)
            }
            IrExpr::UnaryOp(_, _) => false,
            // Cond (ternary) supported with Branch/Jump/Label in MIR JIT phase 3
            IrExpr::Cond(cond, t, f) => {
                Self::is_expr_jit_safe(cond, sig)
                    && Self::is_expr_jit_safe(t, sig)
                    && Self::is_expr_jit_safe(f, sig)
            }
            // Concat: total lebar > 64 tak termodelkan I64 (shl mem-mask
            // count & 63). Tolak bila ada sub-width tak diketahui.
            IrExpr::Concat(exprs) => {
                let total = exprs.iter().try_fold(0usize, |acc, e| {
                    Self::expr_width(e, sig).map(|w| acc.saturating_add(w))
                });
                matches!(total, Some(t) if t <= 64)
                    && exprs.iter().all(|e| Self::is_expr_jit_safe(e, sig))
            }
            // Cast yang melebarkan ke > 64 tak termodelkan.
            IrExpr::Cast { width, expr: inner } => {
                *width <= 64 && Self::is_expr_jit_safe(inner, sig)
            }
            // Signed: JIT pass-through tanpa sign-extend → bisa beda dengan
            // interpreter saat dilebarkan. Literal desimal non-negatif
            // (elaborator membungkus Value::Decimal jadi Signed) aman karena
            // sign-extend == zero-extend; selain itu fallback.
            IrExpr::Signed(inner) => match inner.as_ref() {
                IrExpr::Const(lv) => {
                    !lv.has_unknown()
                        && lv.width <= 64
                        && lv.bits.get(lv.width.saturating_sub(1)) != Some(&LogicVal::One)
                }
                _ => false,
            },
            _ => false,
        }
    }

    /// Operator yang punya padanan 1:1 di MIR. Operator lain (Power,
    /// BitXnor, Sshl/Sshr) tidak dipetakan — `ir_binop_to_mir` dulu fallback
    /// ke Add sehingga `a ** 2` dihitung `a + 2`. Div/Mod juga ditolak:
    /// pembagi 0 = trap Cranelift (SIGFPE) sedangkan interpreter = X.
    /// Shift hanya aman bila jumlah shift literal < 64 (Cranelift mem-mask
    /// count ke 63, interpreter memberi 0 saat count >= lebar hasil).
    fn is_binop_jit_safe(op: &BinaryIrOp, rhs: &IrExpr) -> bool {
        match op {
            BinaryIrOp::Add
            | BinaryIrOp::Sub
            | BinaryIrOp::Mul
            | BinaryIrOp::BitAnd
            | BinaryIrOp::BitOr
            | BinaryIrOp::BitXor
            | BinaryIrOp::Eq
            | BinaryIrOp::CaseEq
            | BinaryIrOp::EqWild
            | BinaryIrOp::Neq
            | BinaryIrOp::CaseNeq
            | BinaryIrOp::NeqWild
            | BinaryIrOp::Lt
            | BinaryIrOp::Le
            | BinaryIrOp::Gt
            | BinaryIrOp::Ge
            | BinaryIrOp::LogicalAnd
            | BinaryIrOp::LogicalOr => true,
            BinaryIrOp::Shl | BinaryIrOp::Shr => {
                Self::const_shift_amount(rhs).is_some_and(|v| v < 64)
            }
            _ => false,
        }
    }

    /// Nilai literal (melewati wrapper `Signed` yang dipakai elaborator untuk
    /// literal desimal).
    fn const_shift_amount(rhs: &IrExpr) -> Option<u64> {
        match rhs {
            IrExpr::Const(lv) if !lv.has_unknown() => Some(lv.to_u64()),
            IrExpr::Signed(inner) => Self::const_shift_amount(inner),
            _ => None,
        }
    }

    /// Check if an IrStmt body is fully JIT-safe (all expressions within are supported).
    fn is_body_jit_safe(body: &[IrStmt], sig: &[SignalInfo]) -> bool {
        for stmt in body {
            match stmt {
                IrStmt::BlockingAssign { lhs, rhs, delay } => {
                    if delay.is_some() {
                        return false;
                    }
                    if !matches!(lhs, IrLValue::Signal(_, _)) {
                        return false; // Only simple Signal lvalues supported
                    }
                    if !Self::is_expr_jit_safe(rhs, sig) {
                        return false;
                    }
                }
                IrStmt::Block { stmts: inner } | IrStmt::NamedBlock { stmts: inner, .. } => {
                    if !Self::is_body_jit_safe(inner, sig) {
                        return false;
                    }
                }
                IrStmt::If {
                    cond,
                    true_branch,
                    false_branch,
                } => {
                    if !Self::is_expr_jit_safe(cond, sig) {
                        return false;
                    }
                    if !Self::is_body_jit_safe(true_branch, sig) {
                        return false;
                    }
                    if !Self::is_body_jit_safe(false_branch, sig) {
                        return false;
                    }
                }
                IrStmt::Case {
                    expr: case_expr,
                    items,
                    default,
                    ..
                } => {
                    // Case dengan >= 2 item ATAU item berlabel > 1 PANIK di
                    // codegen: lowering meng-emit Branch berurutan tanpa Label
                    // di antaranya sehingga dua Branch mengisi blok yang sama
                    // ("you cannot add an instruction to a block already
                    // filled"). Guard JIT-12 (item) / JIT-13a (label).
                    if items.len() >= 2 || items.iter().any(|i| i.labels.len() > 1) {
                        return false;
                    }
                    if !Self::is_expr_jit_safe(case_expr, sig) {
                        return false;
                    }
                    for item in items {
                        for pat in &item.labels {
                            if !Self::is_expr_jit_safe(pat, sig) {
                                return false;
                            }
                        }
                        if !Self::is_body_jit_safe(&item.body, sig) {
                            return false;
                        }
                    }
                    if !Self::is_body_jit_safe(default, sig) {
                        return false;
                    }
                }
                // Loop dengan back-edge PANIK di codegen: entry block sudah
                // di-seal eager sehingga declare_block_predecessor gagal
                // ("assertion failed: !self.is_sealed(block)"). Guard JIT-12.
                IrStmt::LoopWhile { .. }
                | IrStmt::LoopDoWhile { .. }
                | IrStmt::LoopFor { .. }
                | IrStmt::Repeat { .. } => return false,
                IrStmt::NonBlockingAssign { lhs, rhs, delay } => {
                    if delay.is_some() {
                        return false;
                    }
                    if !matches!(lhs, IrLValue::Signal(_, _)) {
                        return false;
                    }
                    if !Self::is_expr_jit_safe(rhs, sig) {
                        return false;
                    }
                }
                _ => return false,
            }
        }
        true
    }

    /// Lower an IrExpr into MIR instructions, storing the result in `dest_reg`.
    /// Caller must ensure `is_expr_jit_safe()` returned true first.
    fn ir_expr_to_mir(
        &self,
        expr: &IrExpr,
        instrs: &mut Vec<mivon_compiler::mir::MirInstr>,
        dest_reg: usize,
        next_reg: &mut usize,
        labels: &mut usize,
    ) {
        match expr {
            IrExpr::Const(lv) => {
                instrs.push(mivon_compiler::mir::MirInstr::Const {
                    dest: dest_reg,
                    value: lv.to_u64(),
                    width: lv.width.max(1),
                });
            }
            IrExpr::Signal(id, _width) => {
                instrs.push(mivon_compiler::mir::MirInstr::Load {
                    dest: dest_reg,
                    signal: *id,
                });
            }
            IrExpr::FillLit(lv) => {
                let val = match lv {
                    LogicVal::Zero => 0u64,
                    LogicVal::One => 1u64,
                    _ => 0u64,
                };
                instrs.push(mivon_compiler::mir::MirInstr::Const {
                    dest: dest_reg,
                    value: val,
                    width: 1,
                });
            }
            IrExpr::BinaryOp(op, lhs, rhs) => {
                let lhs_reg = *next_reg;
                *next_reg += 1;
                let rhs_reg = *next_reg;
                *next_reg += 1;
                self.ir_expr_to_mir(lhs, instrs, lhs_reg, next_reg, labels);
                self.ir_expr_to_mir(rhs, instrs, rhs_reg, next_reg, labels);
                let mir_op = Self::ir_binop_to_mir(op);
                let width = self.compute_expr_width(expr).unwrap_or(64);
                instrs.push(mivon_compiler::mir::MirInstr::Binary {
                    op: mir_op,
                    dest: dest_reg,
                    lhs: lhs_reg,
                    rhs: rhs_reg,
                    width,
                });
            }
            IrExpr::UnaryOp(op, inner) => {
                let inner_reg = *next_reg;
                *next_reg += 1;
                self.ir_expr_to_mir(inner, instrs, inner_reg, next_reg, labels);
                let width = self.compute_expr_width(expr).unwrap_or(64);
                let mir_op = match op {
                    mivon_ir::UnaryIrOp::BitNot => mivon_compiler::mir::MirUnOp::Not,
                    mivon_ir::UnaryIrOp::Minus => mivon_compiler::mir::MirUnOp::Neg,
                    _ => unreachable!(), // is_expr_jit_safe guarantees this
                };
                instrs.push(mivon_compiler::mir::MirInstr::Unary {
                    op: mir_op,
                    dest: dest_reg,
                    operand: inner_reg,
                    width,
                });
            }
            // Cond (ternary): write result directly to dest_reg via separate branches
            IrExpr::Cond(cond, t, f) => {
                let cond_reg = *next_reg;
                *next_reg += 1;
                self.ir_expr_to_mir(cond, instrs, cond_reg, next_reg, labels);
                let _width = self.compute_expr_width(expr).unwrap_or(1);
                // Label Cond WAJIB dari counter bersama: memakai
                // next_label(instrs) membuat label bertabrakan dengan If/Case
                // induk (JIT-13b).
                let then_label = Self::alloc_label(labels);
                let else_label = Self::alloc_label(labels);
                let end_label = Self::alloc_label(labels);
                instrs.push(mivon_compiler::mir::MirInstr::Branch {
                    cond: cond_reg,
                    then_label,
                    else_label,
                });
                // Then branch: compute true value into dest_reg
                instrs.push(mivon_compiler::mir::MirInstr::Label(then_label));
                self.ir_expr_to_mir(t, instrs, dest_reg, next_reg, labels);
                instrs.push(mivon_compiler::mir::MirInstr::Jump { label: end_label });
                // Else branch: compute false value into dest_reg
                instrs.push(mivon_compiler::mir::MirInstr::Label(else_label));
                self.ir_expr_to_mir(f, instrs, dest_reg, next_reg, labels);
                instrs.push(mivon_compiler::mir::MirInstr::Label(end_label));
            }
            // Concat: shift each part into place and OR
            IrExpr::Concat(exprs) => {
                let width = self.compute_expr_width(expr).unwrap_or(64);
                instrs.push(mivon_compiler::mir::MirInstr::Const {
                    dest: dest_reg,
                    value: 0,
                    width,
                });
                let mut offset = 0usize;
                for part in exprs.iter().rev() {
                    let part_reg = *next_reg;
                    *next_reg += 1;
                    self.ir_expr_to_mir(part, instrs, part_reg, next_reg, labels);
                    let part_w = self.compute_expr_width(part).unwrap_or(1);
                    if offset > 0 {
                        let shift_reg = *next_reg;
                        *next_reg += 1;
                        instrs.push(mivon_compiler::mir::MirInstr::Const {
                            dest: shift_reg,
                            value: offset as u64,
                            width: 64,
                        });
                        instrs.push(mivon_compiler::mir::MirInstr::Binary {
                            op: mivon_compiler::mir::MirBinOp::Shl,
                            dest: part_reg,
                            lhs: part_reg,
                            rhs: shift_reg,
                            width,
                        });
                    }
                    instrs.push(mivon_compiler::mir::MirInstr::Binary {
                        op: mivon_compiler::mir::MirBinOp::Or,
                        dest: dest_reg,
                        lhs: dest_reg,
                        rhs: part_reg,
                        width,
                    });
                    offset += part_w;
                }
            }
            // Cast: resize width (extend or truncate)
            IrExpr::Cast { expr: inner, width } => {
                let inner_reg = *next_reg;
                *next_reg += 1;
                self.ir_expr_to_mir(inner, instrs, inner_reg, next_reg, labels);
                if self.compute_expr_width(inner).unwrap_or(1) != *width {
                    let mask = if *width < 64 {
                        ((1u64 << width) - 1) as i64
                    } else {
                        -1i64
                    };
                    let mask_reg = *next_reg;
                    *next_reg += 1;
                    instrs.push(mivon_compiler::mir::MirInstr::Const {
                        dest: mask_reg,
                        value: mask as u64,
                        width: *width,
                    });
                    instrs.push(mivon_compiler::mir::MirInstr::Binary {
                        op: mivon_compiler::mir::MirBinOp::And,
                        dest: dest_reg,
                        lhs: inner_reg,
                        rhs: mask_reg,
                        width: *width,
                    });
                } else {
                    // Same width — copy via OR with 0
                    instrs.push(mivon_compiler::mir::MirInstr::Const {
                        dest: dest_reg,
                        value: 0,
                        width: *width,
                    });
                    instrs.push(mivon_compiler::mir::MirInstr::Binary {
                        op: mivon_compiler::mir::MirBinOp::Or,
                        dest: dest_reg,
                        lhs: dest_reg,
                        rhs: inner_reg,
                        width: *width,
                    });
                }
            }
            // Signed: pass-through (width already accounts for signedness in MIR)
            IrExpr::Signed(inner) => {
                let inner_reg = *next_reg;
                *next_reg += 1;
                self.ir_expr_to_mir(inner, instrs, inner_reg, next_reg, labels);
                let width = self.compute_expr_width(expr).unwrap_or(64);
                instrs.push(mivon_compiler::mir::MirInstr::Const {
                    dest: dest_reg,
                    value: 0,
                    width,
                });
                instrs.push(mivon_compiler::mir::MirInstr::Binary {
                    op: mivon_compiler::mir::MirBinOp::Or,
                    dest: dest_reg,
                    lhs: dest_reg,
                    rhs: inner_reg,
                    width,
                });
            }
            // Pre-check `is_expr_jit_safe` menyaring semua varian yang tidak
            // di-lower ke MIR; sisanya dieksekusi lewat interpreter (fallback).
            _ => unreachable!("ir_expr_to_mir: variant filtered by is_expr_jit_safe"),
        }
    }

    /// Potong nilai di register `reg` ke lebar target (mask). Tanpa ini
    /// `Store` menulis nilai penuh ke buffer input, sehingga Load berikutnya
    /// dalam proses yang sama melihat nilai TIDAK ter-truncate, sedangkan
    /// interpreter memotong ke lebar target (JIT-12c). No-op untuk lebar
    /// >= 64 / tak diketahui.
    fn mask_reg_to_width(
        &self,
        instrs: &mut Vec<mivon_compiler::mir::MirInstr>,
        reg: usize,
        width: usize,
        eval_width: usize,
        next_reg: &mut usize,
    ) {
        if width == 0 || width >= 64 {
            return;
        }
        // Mask dijalankan pada lebar EVALUASI (>= lebar target). Bila
        // memakai lebar target, mask = full-mask lebar itu sehingga
        // optimizer MIR membuangnya sebagai identitas (JIT-12c).
        let and_width = eval_width.max(width).min(64);
        if and_width == width {
            return;
        }
        let mask = (1u64 << width) - 1;
        let mask_reg = *next_reg;
        *next_reg += 1;
        instrs.push(mivon_compiler::mir::MirInstr::Const {
            dest: mask_reg,
            value: mask,
            width: and_width,
        });
        instrs.push(mivon_compiler::mir::MirInstr::Binary {
            op: mivon_compiler::mir::MirBinOp::And,
            dest: reg,
            lhs: reg,
            rhs: mask_reg,
            width: and_width,
        });
    }

    /// Ambil label unik dari counter bersama (per proses).
    fn alloc_label(labels: &mut usize) -> usize {
        let l = *labels;
        *labels += 1;
        l
    }

    /// Lower an IrStmt block into MIR instructions.
    /// Returns None if any statement or expression is unsupported (requires interpreter fallback).
    fn ir_body_to_mir(
        &self,
        body: &[IrStmt],
        n_sigs: usize,
        mir_name: Symbol,
    ) -> Option<mivon_compiler::mir::MirProcess> {
        let mut labels = 1usize;
        self.ir_body_to_mir_with(body, n_sigs, mir_name, &mut labels, 0)
    }

    /// Inti lowering; `labels` dibagi lintas body bersarang agar label If/Case
    /// TIDAK bertabrakan (label ganda membuat branch mendarat di blok salah
    /// atau memicu "block already filled"/"!is_sealed") — JIT-13.
    fn ir_body_to_mir_with(
        &self,
        body: &[IrStmt],
        n_sigs: usize,
        mir_name: Symbol,
        labels: &mut usize,
        start_reg: usize,
    ) -> Option<mivon_compiler::mir::MirProcess> {
        // Pre-check: ensure ALL statements and sub-expressions are JIT-safe
        if !Self::is_body_jit_safe(body, &self.design.top.signals) {
            return None;
        }

        let mut instrs = Vec::new();
        // Register mulai dari `start_reg` agar body bersarang tidak memakai
        // ulang register induknya (nilai saling menimpa) — JIT-13.
        let mut next_reg = start_reg;

        for stmt in body {
            match stmt {
                IrStmt::BlockingAssign {
                    lhs: IrLValue::Signal(sig_id, _),
                    rhs,
                    ..
                } => {
                    if *sig_id >= n_sigs {
                        return None;
                    }
                    let dest_reg = next_reg;
                    next_reg += 1;
                    self.ir_expr_to_mir(rhs, &mut instrs, dest_reg, &mut next_reg, labels);
                    let tw = self
                        .design
                        .top
                        .signals
                        .get(*sig_id)
                        .map(|s| s.width)
                        .unwrap_or(0);
                    let rw = self.compute_expr_width(rhs).unwrap_or(tw);
                    self.mask_reg_to_width(&mut instrs, dest_reg, tw, rw, &mut next_reg);
                    instrs.push(mivon_compiler::mir::MirInstr::Store {
                        signal: *sig_id,
                        src: dest_reg,
                    });
                }
                // Block / NamedBlock: flatten inner statements
                IrStmt::Block { stmts: inner } | IrStmt::NamedBlock { stmts: inner, .. } => {
                    let (inner_instrs, _) =
                        self.ir_body_to_mir_inner(inner, n_sigs, next_reg, labels)?;
                    instrs.extend(inner_instrs);
                    next_reg = Self::max_reg_used(&instrs).unwrap_or(next_reg);
                }
                // If: branch on condition
                IrStmt::If {
                    cond,
                    true_branch,
                    false_branch,
                } => {
                    let cond_reg = next_reg;
                    next_reg += 1;
                    self.ir_expr_to_mir(cond, &mut instrs, cond_reg, &mut next_reg, labels);
                    let then_label = Self::alloc_label(labels);
                    let else_label = Self::alloc_label(labels);
                    let end_label = Self::alloc_label(labels);
                    instrs.push(mivon_compiler::mir::MirInstr::Branch {
                        cond: cond_reg,
                        then_label,
                        else_label,
                    });
                    instrs.push(mivon_compiler::mir::MirInstr::Label(then_label));
                    let (t_instrs, _) =
                        self.ir_body_to_mir_inner(true_branch, n_sigs, next_reg, labels)?;
                    instrs.extend(t_instrs);
                    next_reg = Self::max_reg_used(&instrs).unwrap_or(next_reg);
                    instrs.push(mivon_compiler::mir::MirInstr::Jump { label: end_label });
                    instrs.push(mivon_compiler::mir::MirInstr::Label(else_label));
                    let (f_instrs, _) =
                        self.ir_body_to_mir_inner(false_branch, n_sigs, next_reg, labels)?;
                    instrs.extend(f_instrs);
                    next_reg = Self::max_reg_used(&instrs).unwrap_or(next_reg);
                    instrs.push(mivon_compiler::mir::MirInstr::Label(end_label));
                }
                // Case: equality chain with dispatch
                IrStmt::Case {
                    expr: case_expr,
                    items,
                    default,
                    ..
                } => {
                    let case_reg = next_reg;
                    next_reg += 1;
                    self.ir_expr_to_mir(case_expr, &mut instrs, case_reg, &mut next_reg, labels);
                    let mut item_labels = Vec::new();
                    for _ in items {
                        item_labels.push(Self::alloc_label(labels));
                    }
                    let default_label = Self::alloc_label(labels);
                    let end_case_label = Self::alloc_label(labels);
                    for (i, item) in items.iter().enumerate() {
                        // Compare case_reg with each label expression
                        // Use a chain: if match → item_labels[i], else → next comparison or default
                        // Untuk >= 2 item, test item berikutnya butuh label
                        // nyata. Guard menolak case >= 2 item (JIT-12), jadi
                        // jalur ini hanya default_label.
                        let next_test_label = if i + 1 < items.len() {
                            Self::alloc_label(labels)
                        } else {
                            default_label
                        };
                        for pat in &item.labels {
                            let pat_reg = next_reg;
                            next_reg += 1;
                            self.ir_expr_to_mir(pat, &mut instrs, pat_reg, &mut next_reg, labels);
                            let eq_reg = next_reg;
                            next_reg += 1;
                            instrs.push(mivon_compiler::mir::MirInstr::Binary {
                                op: mivon_compiler::mir::MirBinOp::Eq,
                                dest: eq_reg,
                                lhs: case_reg,
                                rhs: pat_reg,
                                width: 1,
                            });
                            // If match → item body, else continue to next pat/next item
                            let next_pat_label = next_test_label;
                            instrs.push(mivon_compiler::mir::MirInstr::Branch {
                                cond: eq_reg,
                                then_label: item_labels[i],
                                else_label: next_pat_label,
                            });
                        }
                    }
                    // Item bodies
                    for (i, item) in items.iter().enumerate() {
                        instrs.push(mivon_compiler::mir::MirInstr::Label(item_labels[i]));
                        let (item_instrs, _) =
                            self.ir_body_to_mir_inner(&item.body, n_sigs, next_reg, labels)?;
                        instrs.extend(item_instrs);
                        next_reg = Self::max_reg_used(&instrs).unwrap_or(next_reg);
                        instrs.push(mivon_compiler::mir::MirInstr::Jump {
                            label: end_case_label,
                        });
                    }
                    // Default body
                    instrs.push(mivon_compiler::mir::MirInstr::Label(default_label));
                    let (def_instrs, _) =
                        self.ir_body_to_mir_inner(default, n_sigs, next_reg, labels)?;
                    instrs.extend(def_instrs);
                    next_reg = Self::max_reg_used(&instrs).unwrap_or(next_reg);
                    instrs.push(mivon_compiler::mir::MirInstr::Label(end_case_label));
                }
                // Loop (While/DoWhile/For/Repeat) DITOLAK guard JIT-12:
                // back-edge ke entry block yang sudah di-seal memicu panic
                // Cranelift. Arm lowering lama dihapus agar label/register
                // tidak pernah dialokasikan untuk jalur mati ini.
                IrStmt::LoopWhile { .. }
                | IrStmt::LoopDoWhile { .. }
                | IrStmt::LoopFor { .. }
                | IrStmt::Repeat { .. } => return None,

                // NonBlockingAssign: write to output buffer (JIT: same as Store, caller handles NBA semantics)
                IrStmt::NonBlockingAssign {
                    lhs: IrLValue::Signal(sig_id, _),
                    rhs,
                    ..
                } => {
                    if *sig_id >= n_sigs {
                        return None;
                    }
                    let dest_reg = next_reg;
                    next_reg += 1;
                    self.ir_expr_to_mir(rhs, &mut instrs, dest_reg, &mut next_reg, labels);
                    let tw = self
                        .design
                        .top
                        .signals
                        .get(*sig_id)
                        .map(|s| s.width)
                        .unwrap_or(0);
                    let rw = self.compute_expr_width(rhs).unwrap_or(tw);
                    self.mask_reg_to_width(&mut instrs, dest_reg, tw, rw, &mut next_reg);
                    instrs.push(mivon_compiler::mir::MirInstr::NonBlocking {
                        signal: *sig_id,
                        src: dest_reg,
                        delay: None,
                    });
                }
                _ => {
                    return None;
                }
            }
        }

        if instrs.is_empty() {
            return None;
        }

        Some(mivon_compiler::mir::MirProcess {
            name: mir_name,
            sensitivity: mivon_compiler::mir::MirSensitivity::AlwaysComb,
            instrs,
        })
    }

    /// Lower an IrStmt block into MIR instructions (inner helper).
    /// Delegates to ir_body_to_mir for full statement support (If, Case, loops, etc.).
    fn ir_body_to_mir_inner(
        &self,
        body: &[IrStmt],
        n_sigs: usize,
        start_reg: usize,
        labels: &mut usize,
    ) -> Option<(Vec<mivon_compiler::mir::MirInstr>, usize)> {
        if body.is_empty() {
            return Some((Vec::new(), start_reg));
        }
        let mir =
            self.ir_body_to_mir_with(body, n_sigs, Symbol::intern("__inner"), labels, start_reg)?;
        let max_reg = mir
            .instrs
            .iter()
            .filter_map(|i| match i {
                mivon_compiler::mir::MirInstr::Const { dest, .. }
                | mivon_compiler::mir::MirInstr::Load { dest, .. }
                | mivon_compiler::mir::MirInstr::Binary { dest, .. }
                | mivon_compiler::mir::MirInstr::Unary { dest, .. } => Some(*dest + 1),
                _ => None,
            })
            .max()
            .unwrap_or(start_reg);
        Some((mir.instrs, max_reg.max(start_reg)))
    }

    /// Find the maximum register index used in a list of MIR instructions.
    fn max_reg_used(instrs: &[mivon_compiler::mir::MirInstr]) -> Option<usize> {
        let mut max_reg = 0usize;
        for instr in instrs {
            match instr {
                mivon_compiler::mir::MirInstr::Const { dest, .. }
                | mivon_compiler::mir::MirInstr::Load { dest, .. }
                | mivon_compiler::mir::MirInstr::Binary { dest, .. }
                | mivon_compiler::mir::MirInstr::Unary { dest, .. } => {
                    max_reg = max_reg.max(*dest);
                }
                _ => {}
            }
        }
        if max_reg == 0 {
            None
        } else {
            Some(max_reg + 1)
        }
    }

    /// True bila nilai sinyal tak bisa dimodelkan jalur JIT: tri-state (X/Z)
    /// atau lebar > 64 bit (register JIT = I64).
    fn signal_unrepresentable(sig: &LogicVec) -> bool {
        sig.width > 64 || sig.has_unknown()
    }

    /// Try to evaluate a process using MIR JIT (compiled-code simulation path).
    /// Returns Ok(true) if JIT was used, Ok(false) if fallback to interpreted needed.
    ///
    /// Handles both blocking and non-blocking assignments:
    /// - Blocking assignments (IrStmt::BlockingAssign) → applied directly via write_signal
    /// - Non-blocking assignments (IrStmt::NonBlockingAssign) → queued to nba_pending
    pub fn try_evaluate_mir_jit(&mut self, pid: usize, body: &[IrStmt]) -> Result<bool, SimError> {
        if !self.use_mir_jit {
            return Ok(false);
        }
        if self.mir_jit.is_none() {
            return Ok(false);
        }
        // Check if body has any delay statements — if so, fallback
        let has_delay = body.iter().any(|s| matches!(s, IrStmt::Delay { .. }));
        if has_delay {
            return Ok(false);
        }
        // Convert IrStmt block to MIR
        let n_sigs = self.state.signals.len();
        let mir_name = Symbol::intern(&format!("jit_proc_{}", pid));
        let mut mir_process = match self.ir_body_to_mir(body, n_sigs, mir_name) {
            Some(p) => p,
            None => return Ok(false),
        };
        // COMP-01: pipeline optimasi MIR (fixed-point: constant fold, copy
        // propagate, DSE, branch fold, strength reduce, nop removal) kini
        // TER-WIRE ke jalur JIT — instruksi teroptimasi sebelum compile.
        mivon_compiler::mir::optimize_process(&mut mir_process.instrs);
        // Pre-scan: recursively identify all signal IDs with NBA assignments in this body.
        // These must go through nba_pending, not direct write_signal.
        // Uses recursion to handle NBA inside if/case/loop/block control flow.
        fn collect_nba_signals(stmts: &[IrStmt]) -> HashSet<usize> {
            let mut targets = HashSet::new();
            for stmt in stmts {
                match stmt {
                    IrStmt::NonBlockingAssign {
                        lhs: IrLValue::Signal(id, _),
                        ..
                    } => {
                        targets.insert(*id);
                    }
                    IrStmt::NonBlockingAssign { .. } => {}
                    IrStmt::Block { stmts: inner } | IrStmt::NamedBlock { stmts: inner, .. } => {
                        targets.extend(collect_nba_signals(inner));
                    }
                    IrStmt::If {
                        true_branch,
                        false_branch,
                        ..
                    } => {
                        targets.extend(collect_nba_signals(true_branch));
                        targets.extend(collect_nba_signals(false_branch));
                    }
                    IrStmt::Case { items, default, .. } => {
                        for item in items {
                            targets.extend(collect_nba_signals(&item.body));
                        }
                        targets.extend(collect_nba_signals(default));
                    }
                    IrStmt::LoopWhile { body, .. }
                    | IrStmt::LoopDoWhile { body, .. }
                    | IrStmt::LoopFor { body, .. }
                    | IrStmt::Repeat { body, .. } => {
                        targets.extend(collect_nba_signals(body));
                    }
                    _ => {}
                }
            }
            targets
        }
        let nba_targets = collect_nba_signals(body);

        // Guard nilai tak termodelkan JIT: aritmetika u64 (X/Z -> 0, lebar
        // dipotong 64). Fallback ke interpreter bila sinyal yang DIBACA
        // tri-state / > 64 bit, ATAU target TULIS > 64 bit (write-back
        // from_u64 zero-extend, sedangkan interpreter bisa sign-extend).
        // Regresi bug_0366cd02200e_differential.
        let mut read_sigs: HashSet<usize> = HashSet::new();
        let mut write_sigs: HashSet<usize> = HashSet::new();
        let mut store_sigs: HashSet<usize> = HashSet::new();
        let mut nba_sigs: HashSet<usize> = HashSet::new();
        for ins in &mir_process.instrs {
            match ins {
                MirInstr::Load { signal, .. } => {
                    read_sigs.insert(*signal);
                }
                MirInstr::Store { signal, .. } => {
                    write_sigs.insert(*signal);
                    store_sigs.insert(*signal);
                }
                MirInstr::NonBlocking { signal, .. } => {
                    write_sigs.insert(*signal);
                    nba_sigs.insert(*signal);
                }
                _ => {}
            }
        }
        // Campuran blocking + NBA pada sinyal yang SAMA berbagi satu slot
        // `out_vals`, sehingga urutan Store/NonBlocking hilang (mis.
        // `q <= 2; q = 1;` harusnya 2). Fallback interpreter.
        if store_sigs.iter().any(|s| nba_sigs.contains(s)) {
            return Ok(false);
        }
        // ID di luar jangkauan juga fallback (codegen diam-diam membuang load).
        // Signedness: JIT memakai u64 + mask (zero-extend) saat nilai
        // dilebarkan, sedangkan interpreter sign-extend bila
        // `is_signed_expr` benar (util/value.rs). Karena sinyal signed bisa
        // dilebarkan tanpa node IrExpr::Signed (mis. `y = s`), guard harus
        // menolak sinyal signed yang DIBACA — JIT-09 (a).
        let sig_info = &self.design.top.signals;
        let bad_read = read_sigs.iter().any(|&s| {
            s >= n_sigs
                || Self::signal_unrepresentable(self.state.read_signal(s))
                || sig_info.get(s).is_some_and(|si| si.is_signed)
        });
        // Target tulis yang saat ini X/Z juga fallback: out_vals berbasis
        // u64 (X -> 0) sehingga target yang TIDAK dieksekusi (cabang tak
        // diambil) akan berubah X -> 0 (JIT-05c). Write-back tak bisa
        // membedakan "tak ditulis" dari "ditulis 0" untuk nilai X.
        let bad_write = write_sigs.iter().any(|&s| {
            s >= n_sigs
                || self.state.read_signal(s).width > 64
                || self.state.read_signal(s).has_unknown()
        });
        if bad_read || bad_write {
            return Ok(false);
        }

        // Compile to native code (or cache hit)
        let compiled = match self
            .mir_jit
            .as_mut()
            .and_then(|jit| jit.compile_process(&mir_process, n_sigs))
        {
            Some(c) => c,
            None => return Ok(false),
        };
        // Extract signal values. `out_vals` dimulai dari salinan nilai saat
        // ini (bukan nol) agar target yang TIDAK dieksekusi (mis. cabang if
        // tidak diambil) tidak tertimpa 0 saat apply-back (JIT-05c).
        let mut signal_vals = vec![0u64; n_sigs.max(1)];
        for (i, slot) in signal_vals.iter_mut().enumerate().take(n_sigs) {
            *slot = self.state.read_signal(i).to_u64();
        }
        // Salinan nilai awal: target yang tak dieksekusi tetap bernilai lama
        // (out_vals di-zero-init membuatnya tertimpa 0 — JIT-05c).
        let mut out_vals = signal_vals.clone();
        // Execute compiled native code
        unsafe {
            mivon_compiler::mir::MirJitCompiler::call_process(
                compiled.code_ptr,
                &mut signal_vals,
                &mut out_vals,
            );
        }
        // Apply output values back to state — HANYA sinyal yang benar-benar
        // ditulis proses (Store/NonBlocking di MIR). `out_vals` di-zero-init
        // dan codegen hanya menulis slot target, jadi iterasi seluruh array
        // akan menimpa sinyal lain menjadi 0 (JIT-05). Perbandingan dilakukan
        // di level LogicVec (bukan u64) agar target X/Z yang seharusnya
        // menjadi 0 tidak di-skip.
        let mut targets: Vec<usize> = write_sigs.iter().copied().collect();
        targets.sort_unstable();
        for i in targets {
            if i >= n_sigs {
                continue;
            }
            let val = out_vals[i];
            let current = self.state.read_signal(i);
            let new_lv = LogicVec::from_u64(val, current.width.max(1));
            if *current == new_lv {
                continue;
            }
            if nba_targets.contains(&i) {
                // Non-blocking assign: queue to nba_pending for NBA region commit
                self.push_nba_pending(IrLValue::Signal(i, current.width), new_lv);
            } else {
                // Blocking assign: apply directly
                self.state.write_signal(i, new_lv);
            }
        }
        Ok(true)
    }

    /// Compute approximate width of an IrExpr.
    fn compute_expr_width(&self, expr: &IrExpr) -> Option<usize> {
        Self::expr_width(expr, &self.design.top.signals)
    }

    /// Versi murni (tanpa state) dari perhitungan lebar ekspresi. IR
    /// menyimpan width 0 untuk identifier polos, jadi lebar sinyal WAJIB
    /// diambil dari tabel `SignalInfo`; width 0 di tabel berarti tak
    /// diketahui → None (pemanggil memilih fallback konservatif).
    fn expr_width(expr: &IrExpr, sig: &[SignalInfo]) -> Option<usize> {
        match expr {
            IrExpr::Const(lv) => Some(lv.width),
            // IR memakai width 0 untuk identifier polos; lebar sebenarnya
            // ada di tabel sinyal. 0 di keduanya = tak diketahui → None.
            IrExpr::Signal(id, w) => sig
                .get(*id)
                .map(|s| s.width)
                .filter(|w| *w > 0)
                .or_else(|| (*w > 0).then_some(*w)),
            IrExpr::BinaryOp(op, lhs, rhs) => {
                let lw = Self::expr_width(lhs, sig)?;
                // LRM §11.8.1: hasil shift selebar operan kiri; jumlah shift
                // self-determined sehingga TIDAK melebarkan hasil (sama dengan
                // interpreter di value.rs). Tanpa ini mask JIT memakai 32 bit
                // (lebar literal shift) → hasil beda dari interpreter.
                if matches!(
                    op,
                    mivon_ir::BinaryIrOp::Shl
                        | mivon_ir::BinaryIrOp::Shr
                        | mivon_ir::BinaryIrOp::Sshl
                        | mivon_ir::BinaryIrOp::Sshr
                ) {
                    return Some(lw);
                }
                let rw = Self::expr_width(rhs, sig)?;
                Some(lw.max(rw))
            }
            IrExpr::UnaryOp(_, inner) => Self::expr_width(inner, sig),
            IrExpr::Cond(_, t, f) => {
                let tw = Self::expr_width(t, sig)?;
                let fw = Self::expr_width(f, sig)?;
                Some(tw.max(fw))
            }
            // Concat: lebar = jumlah lebar bagian; bagian tak diketahui
            // membuat total tak diketahui (jangan diam-diam di-skip).
            IrExpr::Concat(exprs) => {
                let total = exprs.iter().try_fold(0usize, |acc, e| {
                    Self::expr_width(e, sig).map(|w| acc.saturating_add(w))
                })?;
                Some(total.max(1))
            }
            IrExpr::Cast { width, .. } => Some(*width),
            IrExpr::Signed(inner) => Self::expr_width(inner, sig),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Predikat guard dengan tabel sinyal kosong (semua ekspresi uji hanya
    /// memakai Const/Cast/Concat, jadi lebar sinyal tak diperlukan).
    fn safe(e: &IrExpr) -> bool {
        SimulationEngine::is_expr_jit_safe(e, &[])
    }

    /// Guard JIT deterministik (tak butuh feature jit/cranelift): Const dengan
    /// bit X/Z TIDAK boleh JIT-safe karena to_u64 memetakan X/Z ke 0 (nilai
    /// salah). Regresi bug_0366cd02200e_differential (JIT-04/JIT-06).
    #[test]
    fn const_with_x_is_not_jit_safe() {
        // LogicVec::new(w) = semua bit X.
        let all_x = LogicVec::new(4);
        assert!(!safe(&IrExpr::Const(all_x)));
    }

    /// Konstanta bersih tetap JIT-safe (tak ada regresi untuk desain X-free).
    #[test]
    fn clean_const_stays_jit_safe() {
        let clean = LogicVec::from_u64(0b1010, 4);
        assert!(safe(&IrExpr::Const(clean)));
    }

    /// Konstanta > 64 bit tak bisa direpresentasikan di register I64 JIT.
    #[test]
    fn wide_const_not_jit_safe() {
        let wide = LogicVec::from_u64(1, 128);
        assert!(!safe(&IrExpr::Const(wide)));
    }

    /// Predikat guard runtime: X/Z atau lebar > 64 bit -> tak termodelkan JIT.
    #[test]
    fn signal_unrepresentable_predicate() {
        assert!(SimulationEngine::signal_unrepresentable(&LogicVec::new(4)));
        assert!(!SimulationEngine::signal_unrepresentable(
            &LogicVec::from_u64(1, 4)
        ));
        assert!(SimulationEngine::signal_unrepresentable(
            &LogicVec::from_u64(1, 128)
        ));
    }

    /// Concat total > 64 bit -> tak JIT-safe; <= 64 tetap aman.
    #[test]
    fn concat_width_guard() {
        let a = IrExpr::Const(LogicVec::from_u64(1, 32));
        let b = IrExpr::Const(LogicVec::from_u64(1, 32));
        let c = IrExpr::Const(LogicVec::from_u64(1, 64));
        assert!(safe(&IrExpr::Concat(vec![a.clone(), b.clone()])));
        assert!(!safe(&IrExpr::Concat(vec![a, b, c])));
    }

    /// Signed ditolak (JIT tak sign-extend) KECUALI literal non-negatif.
    #[test]
    fn signed_expr_not_jit_safe() {
        // Non-negatif (MSB 0): sign-extend == zero-extend → aman. Elaborator
        // membungkus semua literal desimal jadi Signed, jadi ini wajib.
        let pos = IrExpr::Const(LogicVec::from_u64(1, 8));
        assert!(safe(&IrExpr::Signed(Box::new(pos))));
        // MSB 1 (nilai negatif): sign-extend berbeda → tolak.
        let neg = IrExpr::Const(LogicVec::from_u64(0x80, 8));
        assert!(!safe(&IrExpr::Signed(Box::new(neg))));
        // Signed atas sinyal: signedness sinyal tak diketahui di sini → tolak.
        assert!(!safe(&IrExpr::Signed(Box::new(IrExpr::Signal(0, 8)))));
    }

    /// Cast ke lebar > 64 bit tak termodelkan I64 -> fallback; <= 64 aman.
    #[test]
    fn cast_width_guard() {
        let inner = IrExpr::Const(LogicVec::from_u64(1, 8));
        assert!(safe(&IrExpr::Cast {
            width: 32,
            expr: Box::new(inner.clone()),
        }));
        assert!(!safe(&IrExpr::Cast {
            width: 128,
            expr: Box::new(inner),
        }));
    }
}
