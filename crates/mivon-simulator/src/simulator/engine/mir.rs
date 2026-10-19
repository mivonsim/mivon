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
    fn is_expr_jit_safe(expr: &IrExpr) -> bool {
        match expr {
            IrExpr::Const(_) => true,
            IrExpr::Signal(_, _) => true,
            IrExpr::FillLit(lv) => matches!(lv, LogicVal::Zero | LogicVal::One),
            IrExpr::BinaryOp(_, lhs, rhs) => {
                Self::is_expr_jit_safe(lhs) && Self::is_expr_jit_safe(rhs)
            }
            IrExpr::UnaryOp(
                mivon_ir::UnaryIrOp::BitNot
                | mivon_ir::UnaryIrOp::Minus
                | mivon_ir::UnaryIrOp::Plus,
                inner,
            ) => Self::is_expr_jit_safe(inner),
            IrExpr::UnaryOp(_, _) => false,
            // Cond (ternary) supported with Branch/Jump/Label in MIR JIT phase 3
            IrExpr::Cond(cond, t, f) => {
                Self::is_expr_jit_safe(cond)
                    && Self::is_expr_jit_safe(t)
                    && Self::is_expr_jit_safe(f)
            }
            IrExpr::Concat(exprs) => exprs.iter().all(Self::is_expr_jit_safe),
            IrExpr::Cast { expr: inner, .. } => Self::is_expr_jit_safe(inner),
            IrExpr::Signed(inner) => Self::is_expr_jit_safe(inner),
            _ => false,
        }
    }

    /// Check if an IrStmt body is fully JIT-safe (all expressions within are supported).
    fn is_body_jit_safe(body: &[IrStmt]) -> bool {
        for stmt in body {
            match stmt {
                IrStmt::BlockingAssign { lhs, rhs, delay } => {
                    if delay.is_some() {
                        return false;
                    }
                    if !matches!(lhs, IrLValue::Signal(_, _)) {
                        return false; // Only simple Signal lvalues supported
                    }
                    if !Self::is_expr_jit_safe(rhs) {
                        return false;
                    }
                }
                IrStmt::Block { stmts: inner } | IrStmt::NamedBlock { stmts: inner, .. } => {
                    if !Self::is_body_jit_safe(inner) {
                        return false;
                    }
                }
                IrStmt::If {
                    cond,
                    true_branch,
                    false_branch,
                } => {
                    if !Self::is_expr_jit_safe(cond) {
                        return false;
                    }
                    if !Self::is_body_jit_safe(true_branch) {
                        return false;
                    }
                    if !Self::is_body_jit_safe(false_branch) {
                        return false;
                    }
                }
                IrStmt::Case {
                    expr: case_expr,
                    items,
                    default,
                    ..
                } => {
                    if !Self::is_expr_jit_safe(case_expr) {
                        return false;
                    }
                    for item in items {
                        for pat in &item.labels {
                            if !Self::is_expr_jit_safe(pat) {
                                return false;
                            }
                        }
                        if !Self::is_body_jit_safe(&item.body) {
                            return false;
                        }
                    }
                    if !Self::is_body_jit_safe(default) {
                        return false;
                    }
                }
                IrStmt::LoopWhile { cond, body, .. } | IrStmt::LoopDoWhile { cond, body, .. } => {
                    if !Self::is_expr_jit_safe(cond) {
                        return false;
                    }
                    if !Self::is_body_jit_safe(body) {
                        return false;
                    }
                }
                IrStmt::LoopFor {
                    init,
                    cond,
                    step,
                    body,
                    ..
                } => {
                    if let Some(init_stmt) = init {
                        if !Self::is_body_jit_safe(&[init_stmt.as_ref().clone()]) {
                            return false;
                        }
                    }
                    if !Self::is_expr_jit_safe(cond) {
                        return false;
                    }
                    if let Some(step_stmt) = step {
                        if !Self::is_body_jit_safe(&[step_stmt.as_ref().clone()]) {
                            return false;
                        }
                    }
                    if !Self::is_body_jit_safe(body) {
                        return false;
                    }
                }
                IrStmt::Repeat { count, body, .. } => {
                    if !Self::is_expr_jit_safe(count) {
                        return false;
                    }
                    if !Self::is_body_jit_safe(body) {
                        return false;
                    }
                }
                IrStmt::NonBlockingAssign { lhs, rhs, delay } => {
                    if delay.is_some() {
                        return false;
                    }
                    if !matches!(lhs, IrLValue::Signal(_, _)) {
                        return false;
                    }
                    if !Self::is_expr_jit_safe(rhs) {
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
        expr: &IrExpr,
        instrs: &mut Vec<mivon_compiler::mir::MirInstr>,
        dest_reg: usize,
        next_reg: &mut usize,
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
                Self::ir_expr_to_mir(lhs, instrs, lhs_reg, next_reg);
                Self::ir_expr_to_mir(rhs, instrs, rhs_reg, next_reg);
                let mir_op = Self::ir_binop_to_mir(op);
                let width = Self::compute_expr_width(expr).unwrap_or(64);
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
                Self::ir_expr_to_mir(inner, instrs, inner_reg, next_reg);
                let width = Self::compute_expr_width(expr).unwrap_or(64);
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
                Self::ir_expr_to_mir(cond, instrs, cond_reg, next_reg);
                let _width = Self::compute_expr_width(expr).unwrap_or(1);
                let then_label = Self::next_label(instrs);
                let else_label = Self::next_label(instrs) + 1;
                let end_label = Self::next_label(instrs) + 2;
                instrs.push(mivon_compiler::mir::MirInstr::Branch {
                    cond: cond_reg,
                    then_label,
                    else_label,
                });
                // Then branch: compute true value into dest_reg
                instrs.push(mivon_compiler::mir::MirInstr::Label(then_label));
                Self::ir_expr_to_mir(t, instrs, dest_reg, next_reg);
                instrs.push(mivon_compiler::mir::MirInstr::Jump { label: end_label });
                // Else branch: compute false value into dest_reg
                instrs.push(mivon_compiler::mir::MirInstr::Label(else_label));
                Self::ir_expr_to_mir(f, instrs, dest_reg, next_reg);
                instrs.push(mivon_compiler::mir::MirInstr::Label(end_label));
            }
            // Concat: shift each part into place and OR
            IrExpr::Concat(exprs) => {
                let width = Self::compute_expr_width(expr).unwrap_or(64);
                instrs.push(mivon_compiler::mir::MirInstr::Const {
                    dest: dest_reg,
                    value: 0,
                    width,
                });
                let mut offset = 0usize;
                for part in exprs.iter().rev() {
                    let part_reg = *next_reg;
                    *next_reg += 1;
                    Self::ir_expr_to_mir(part, instrs, part_reg, next_reg);
                    let part_w = Self::compute_expr_width(part).unwrap_or(1);
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
                Self::ir_expr_to_mir(inner, instrs, inner_reg, next_reg);
                if Self::compute_expr_width(inner).unwrap_or(1) != *width {
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
                Self::ir_expr_to_mir(inner, instrs, inner_reg, next_reg);
                let width = Self::compute_expr_width(expr).unwrap_or(64);
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

    /// Generate a unique label number for MIR Branch/Jump/Label instructions.
    fn next_label(instrs: &[mivon_compiler::mir::MirInstr]) -> usize {
        let max_label = instrs
            .iter()
            .filter_map(|i| {
                if let mivon_compiler::mir::MirInstr::Label(l) = i {
                    Some(*l)
                } else {
                    None
                }
            })
            .max()
            .unwrap_or(0);
        max_label + 1
    }

    /// Lower an IrStmt block into MIR instructions.
    /// Returns None if any statement or expression is unsupported (requires interpreter fallback).
    fn ir_body_to_mir(
        body: &[IrStmt],
        n_sigs: usize,
        mir_name: Symbol,
    ) -> Option<mivon_compiler::mir::MirProcess> {
        // Pre-check: ensure ALL statements and sub-expressions are JIT-safe
        if !Self::is_body_jit_safe(body) {
            return None;
        }

        let mut instrs = Vec::new();
        let mut next_reg = 0usize;

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
                    Self::ir_expr_to_mir(rhs, &mut instrs, dest_reg, &mut next_reg);
                    instrs.push(mivon_compiler::mir::MirInstr::Store {
                        signal: *sig_id,
                        src: dest_reg,
                    });
                }
                // Block / NamedBlock: flatten inner statements
                IrStmt::Block { stmts: inner } | IrStmt::NamedBlock { stmts: inner, .. } => {
                    let (inner_instrs, _) = Self::ir_body_to_mir_inner(inner, n_sigs, next_reg)?;
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
                    Self::ir_expr_to_mir(cond, &mut instrs, cond_reg, &mut next_reg);
                    let then_label = Self::next_label(&instrs);
                    let else_label = Self::next_label(&instrs) + 1;
                    let end_label = Self::next_label(&instrs) + 2;
                    instrs.push(mivon_compiler::mir::MirInstr::Branch {
                        cond: cond_reg,
                        then_label,
                        else_label,
                    });
                    instrs.push(mivon_compiler::mir::MirInstr::Label(then_label));
                    let (t_instrs, _) = Self::ir_body_to_mir_inner(true_branch, n_sigs, next_reg)?;
                    instrs.extend(t_instrs);
                    next_reg = Self::max_reg_used(&instrs).unwrap_or(next_reg);
                    instrs.push(mivon_compiler::mir::MirInstr::Jump { label: end_label });
                    instrs.push(mivon_compiler::mir::MirInstr::Label(else_label));
                    let (f_instrs, _) = Self::ir_body_to_mir_inner(false_branch, n_sigs, next_reg)?;
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
                    Self::ir_expr_to_mir(case_expr, &mut instrs, case_reg, &mut next_reg);
                    let mut item_labels = Vec::new();
                    for _ in items {
                        item_labels.push(Self::next_label(&instrs) + item_labels.len() + 1);
                    }
                    let default_label = Self::next_label(&instrs) + item_labels.len() + 1;
                    let end_case_label = Self::next_label(&instrs) + item_labels.len() + 2;
                    for (i, item) in items.iter().enumerate() {
                        // Compare case_reg with each label expression
                        // Use a chain: if match → item_labels[i], else → next comparison or default
                        let next_test_label = if i + 1 < items.len() {
                            Self::next_label(&instrs) + item_labels.len() + 3 + i
                        } else {
                            default_label
                        };
                        for pat in &item.labels {
                            let pat_reg = next_reg;
                            next_reg += 1;
                            Self::ir_expr_to_mir(pat, &mut instrs, pat_reg, &mut next_reg);
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
                            Self::ir_body_to_mir_inner(&item.body, n_sigs, next_reg)?;
                        instrs.extend(item_instrs);
                        next_reg = Self::max_reg_used(&instrs).unwrap_or(next_reg);
                        instrs.push(mivon_compiler::mir::MirInstr::Jump {
                            label: end_case_label,
                        });
                    }
                    // Default body
                    instrs.push(mivon_compiler::mir::MirInstr::Label(default_label));
                    let (def_instrs, _) = Self::ir_body_to_mir_inner(default, n_sigs, next_reg)?;
                    instrs.extend(def_instrs);
                    next_reg = Self::max_reg_used(&instrs).unwrap_or(next_reg);
                    instrs.push(mivon_compiler::mir::MirInstr::Label(end_case_label));
                }
                // LoopWhile: while(cond) body;
                IrStmt::LoopWhile { cond, body, .. } => {
                    let loop_start = Self::next_label(&instrs);
                    instrs.push(mivon_compiler::mir::MirInstr::Label(loop_start));
                    let cond_reg = next_reg;
                    next_reg += 1;
                    Self::ir_expr_to_mir(cond, &mut instrs, cond_reg, &mut next_reg);
                    let body_label = Self::next_label(&instrs) + 1;
                    let end_label = Self::next_label(&instrs) + 2;
                    instrs.push(mivon_compiler::mir::MirInstr::Branch {
                        cond: cond_reg,
                        then_label: body_label,
                        else_label: end_label,
                    });
                    instrs.push(mivon_compiler::mir::MirInstr::Label(body_label));
                    let (b_instrs, _) = Self::ir_body_to_mir_inner(body, n_sigs, next_reg)?;
                    instrs.extend(b_instrs);
                    next_reg = Self::max_reg_used(&instrs).unwrap_or(next_reg);
                    instrs.push(mivon_compiler::mir::MirInstr::Jump { label: loop_start });
                    instrs.push(mivon_compiler::mir::MirInstr::Label(end_label));
                }
                // LoopDoWhile: do body; while(cond);
                IrStmt::LoopDoWhile { cond, body, .. } => {
                    let loop_start = Self::next_label(&instrs);
                    instrs.push(mivon_compiler::mir::MirInstr::Label(loop_start));
                    let (b_instrs, _) = Self::ir_body_to_mir_inner(body, n_sigs, next_reg)?;
                    instrs.extend(b_instrs);
                    next_reg = Self::max_reg_used(&instrs).unwrap_or(next_reg);
                    let cond_reg = next_reg;
                    next_reg += 1;
                    Self::ir_expr_to_mir(cond, &mut instrs, cond_reg, &mut next_reg);
                    let end_label = Self::next_label(&instrs) + 1;
                    instrs.push(mivon_compiler::mir::MirInstr::Branch {
                        cond: cond_reg,
                        then_label: loop_start,
                        else_label: end_label,
                    });
                    instrs.push(mivon_compiler::mir::MirInstr::Label(end_label));
                }
                // LoopFor: for(init; cond; step) body;
                IrStmt::LoopFor {
                    init,
                    cond,
                    step,
                    body,
                    ..
                } => {
                    if let Some(init_stmt) = init {
                        let (i_instrs, _) = Self::ir_body_to_mir_inner(
                            &[init_stmt.as_ref().clone()],
                            n_sigs,
                            next_reg,
                        )?;
                        instrs.extend(i_instrs);
                        next_reg = Self::max_reg_used(&instrs).unwrap_or(next_reg);
                    }
                    let loop_start = Self::next_label(&instrs);
                    instrs.push(mivon_compiler::mir::MirInstr::Label(loop_start));
                    let cond_reg = next_reg;
                    next_reg += 1;
                    Self::ir_expr_to_mir(cond, &mut instrs, cond_reg, &mut next_reg);
                    let body_label = Self::next_label(&instrs) + 1;
                    let end_label = Self::next_label(&instrs) + 2;
                    instrs.push(mivon_compiler::mir::MirInstr::Branch {
                        cond: cond_reg,
                        then_label: body_label,
                        else_label: end_label,
                    });
                    instrs.push(mivon_compiler::mir::MirInstr::Label(body_label));
                    let (b_instrs, _) = Self::ir_body_to_mir_inner(body, n_sigs, next_reg)?;
                    instrs.extend(b_instrs);
                    next_reg = Self::max_reg_used(&instrs).unwrap_or(next_reg);
                    if let Some(step_stmt) = step {
                        let (s_instrs, _) = Self::ir_body_to_mir_inner(
                            &[step_stmt.as_ref().clone()],
                            n_sigs,
                            next_reg,
                        )?;
                        instrs.extend(s_instrs);
                        next_reg = Self::max_reg_used(&instrs).unwrap_or(next_reg);
                    }
                    instrs.push(mivon_compiler::mir::MirInstr::Jump { label: loop_start });
                    instrs.push(mivon_compiler::mir::MirInstr::Label(end_label));
                }
                // Repeat: repeat(count) body;
                IrStmt::Repeat { count, body, .. } => {
                    let count_reg = next_reg;
                    next_reg += 1;
                    Self::ir_expr_to_mir(count, &mut instrs, count_reg, &mut next_reg);
                    let counter_reg = next_reg;
                    next_reg += 1;
                    instrs.push(mivon_compiler::mir::MirInstr::Const {
                        dest: counter_reg,
                        value: 0,
                        width: 32,
                    });
                    let loop_start = Self::next_label(&instrs);
                    instrs.push(mivon_compiler::mir::MirInstr::Label(loop_start));
                    // cond: counter < count
                    let lt_reg = next_reg;
                    next_reg += 1;
                    instrs.push(mivon_compiler::mir::MirInstr::Binary {
                        op: mivon_compiler::mir::MirBinOp::Lt,
                        dest: lt_reg,
                        lhs: counter_reg,
                        rhs: count_reg,
                        width: 1,
                    });
                    let body_label = Self::next_label(&instrs) + 1;
                    let end_label = Self::next_label(&instrs) + 2;
                    instrs.push(mivon_compiler::mir::MirInstr::Branch {
                        cond: lt_reg,
                        then_label: body_label,
                        else_label: end_label,
                    });
                    instrs.push(mivon_compiler::mir::MirInstr::Label(body_label));
                    let (b_instrs, _) = Self::ir_body_to_mir_inner(body, n_sigs, next_reg)?;
                    instrs.extend(b_instrs);
                    next_reg = Self::max_reg_used(&instrs).unwrap_or(next_reg);
                    // counter++
                    let one_reg = next_reg;
                    next_reg += 1;
                    instrs.push(mivon_compiler::mir::MirInstr::Const {
                        dest: one_reg,
                        value: 1,
                        width: 32,
                    });
                    instrs.push(mivon_compiler::mir::MirInstr::Binary {
                        op: mivon_compiler::mir::MirBinOp::Add,
                        dest: counter_reg,
                        lhs: counter_reg,
                        rhs: one_reg,
                        width: 32,
                    });
                    instrs.push(mivon_compiler::mir::MirInstr::Jump { label: loop_start });
                    instrs.push(mivon_compiler::mir::MirInstr::Label(end_label));
                }
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
                    Self::ir_expr_to_mir(rhs, &mut instrs, dest_reg, &mut next_reg);
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
        body: &[IrStmt],
        n_sigs: usize,
        start_reg: usize,
    ) -> Option<(Vec<mivon_compiler::mir::MirInstr>, usize)> {
        if body.is_empty() {
            return Some((Vec::new(), start_reg));
        }
        let mir = Self::ir_body_to_mir(body, n_sigs, Symbol::intern("__inner"))?;
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
        let jit = match &mut self.mir_jit {
            Some(jit) => jit,
            None => return Ok(false),
        };
        // Check if body has any delay statements — if so, fallback
        let has_delay = body.iter().any(|s| matches!(s, IrStmt::Delay { .. }));
        if has_delay {
            return Ok(false);
        }
        // Convert IrStmt block to MIR
        let n_sigs = self.state.signals.len();
        let mir_name = Symbol::intern(&format!("jit_proc_{}", pid));
        let mut mir_process = match Self::ir_body_to_mir(body, n_sigs, mir_name) {
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

        // Compile to native code (or cache hit)
        let compiled = match jit.compile_process(&mir_process, n_sigs) {
            Some(c) => c,
            None => return Ok(false),
        };
        // Extract signal values
        let mut signal_vals = vec![0u64; n_sigs.max(1)];
        let mut out_vals = vec![0u64; n_sigs.max(1)];
        for (i, slot) in signal_vals.iter_mut().enumerate().take(n_sigs) {
            *slot = self.state.read_signal(i).to_u64();
        }
        // Execute compiled native code
        unsafe {
            mivon_compiler::mir::MirJitCompiler::call_process(
                compiled.code_ptr,
                &signal_vals,
                &mut out_vals,
            );
        }
        // Apply output values back to state — differentiate blocking vs NBA
        for (i, &val) in out_vals.iter().enumerate() {
            if i < n_sigs && val != signal_vals[i] {
                let current = self.state.read_signal(i);
                let new_lv = LogicVec::from_u64(val, current.width.max(1));
                if *current != new_lv {
                    if nba_targets.contains(&i) {
                        // Non-blocking assign: queue to nba_pending for NBA region commit
                        self.push_nba_pending(IrLValue::Signal(i, current.width), new_lv);
                    } else {
                        // Blocking assign: apply directly
                        self.state.write_signal(i, new_lv);
                    }
                }
            }
        }
        Ok(true)
    }

    /// Compute approximate width of an IrExpr.
    fn compute_expr_width(expr: &IrExpr) -> Option<usize> {
        match expr {
            IrExpr::Const(lv) => Some(lv.width),
            IrExpr::Signal(_, w) => Some(*w),
            IrExpr::BinaryOp(_, lhs, rhs) => {
                let lw = Self::compute_expr_width(lhs)?;
                let rw = Self::compute_expr_width(rhs)?;
                Some(lw.max(rw))
            }
            IrExpr::UnaryOp(_, inner) => Self::compute_expr_width(inner),
            IrExpr::Cond(_, t, f) => {
                let tw = Self::compute_expr_width(t)?;
                let fw = Self::compute_expr_width(f)?;
                Some(tw.max(fw))
            }
            IrExpr::Concat(exprs) => {
                let total: usize = exprs.iter().filter_map(Self::compute_expr_width).sum();
                if total == 0 {
                    Some(1)
                } else {
                    Some(total)
                }
            }
            IrExpr::Cast { width, .. } => Some(*width),
            IrExpr::Signed(inner) => Self::compute_expr_width(inner),
            _ => None,
        }
    }
}
