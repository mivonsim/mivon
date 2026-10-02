//! ──────────────────────────────────────────────────────────────────────────────
//! CATATAN: File ini adalah bagian dari pemisahan util.rs (SRP Refactoring).
//! Tanggung jawab: Signal reading analysis & combinational sensitivity.
//!
//! Fungsi:
//!   - infer_comb_sensitivity()     — inferensi sensitivity combinational
//!   - collect_read_signals_stmts() — kumpulkan signal baca dari statements
//!   - collect_read_signals_stmt()  — kumpulkan signal baca dari satu statement
//!   - collect_read_signals_expr()  — kumpulkan signal baca dari expression
//!   - resolve_expr_signal()        — resolusi expression ke signal ID
//!   - collect_sensitivity()        — kumpulkan sensitivity list dari expr
//!   - detect_sync_reset()          — deteksi reset sinkron dari body process
//!
//! ──────────────────────────────────────────────────────────────────────────────

use std::collections::HashMap;

use mivon_ast::*;
use mivon_core::intern::Symbol;
use mivon_ir::*;

/// Inferensi sensitivity list untuk process combinational dari body IR.
pub fn infer_comb_sensitivity(body: &[IrStmt]) -> Vec<SignalId> {
    let mut sigs = Vec::new();
    collect_read_signals_stmts(body, &mut sigs);
    sigs.sort();
    sigs.dedup();
    sigs
}

/// Kumpulkan semua signal yang dibaca dari daftar IR statements.
pub fn collect_read_signals_stmts(stmts: &[IrStmt], out: &mut Vec<SignalId>) {
    for stmt in stmts {
        collect_read_signals_stmt(stmt, out);
    }
}

/// Kumpulkan semua signal yang dibaca dari satu IR statement.
pub fn collect_read_signals_stmt(stmt: &IrStmt, out: &mut Vec<SignalId>) {
    match stmt {
        IrStmt::Block { stmts } => collect_read_signals_stmts(stmts, out),
        IrStmt::BlockingAssign { rhs, .. } | IrStmt::NonBlockingAssign { rhs, .. } => {
            collect_read_signals_expr(rhs, out);
        }
        IrStmt::If {
            cond,
            true_branch,
            false_branch,
        } => {
            collect_read_signals_expr(cond, out);
            collect_read_signals_stmts(true_branch, out);
            collect_read_signals_stmts(false_branch, out);
        }
        IrStmt::Case {
            expr,
            items,
            default,
            ..
        } => {
            collect_read_signals_expr(expr, out);
            for item in items {
                for label in &item.labels {
                    collect_read_signals_expr(label, out);
                }
                collect_read_signals_stmts(&item.body, out);
            }
            collect_read_signals_stmts(default, out);
        }
        IrStmt::Delay { body, .. } => collect_read_signals_stmts(body, out),
        IrStmt::Wait { cond, body } => {
            collect_read_signals_expr(cond, out);
            collect_read_signals_stmts(body, out);
        }
        IrStmt::SysCall { args, .. } => {
            for arg in args {
                collect_read_signals_expr(arg, out);
            }
        }
        IrStmt::LoopWhile { cond, body } => {
            collect_read_signals_expr(cond, out);
            collect_read_signals_stmts(body, out);
        }
        IrStmt::LoopDoWhile { cond, body } => {
            collect_read_signals_expr(cond, out);
            collect_read_signals_stmts(body, out);
        }
        IrStmt::LoopFor {
            init,
            cond,
            step,
            body,
        } => {
            if let Some(s) = init {
                collect_read_signals_stmt(s, out);
            }
            collect_read_signals_expr(cond, out);
            if let Some(s) = step {
                collect_read_signals_stmt(s, out);
            }
            collect_read_signals_stmts(body, out);
        }
        IrStmt::EventControl { sigs, body, .. } => {
            for (sid, _) in sigs {
                out.push(*sid);
            }
            collect_read_signals_stmts(body, out);
        }
        IrStmt::EventTrigger { sig_id } => {
            out.push(*sig_id);
        }
        IrStmt::MethodCallStmt { obj, args, .. } => {
            collect_read_signals_expr(obj, out);
            for arg in args {
                collect_read_signals_expr(arg, out);
            }
        }
        IrStmt::NamedBlock { stmts, .. } => {
            collect_read_signals_stmts(stmts, out);
        }
        IrStmt::Release { .. } | IrStmt::Deassign { .. } => {}
        IrStmt::Force { rhs, .. } => {
            collect_read_signals_expr(rhs, out);
        }
        IrStmt::Disable { .. } => {}
        IrStmt::RandCase { items } => {
            for (w_expr, body) in items {
                collect_read_signals_expr(w_expr, out);
                collect_read_signals_stmts(body, out);
            }
        }
        _ => {}
    }
}

/// Kumpulkan signal baca dari index-ekspresi di dalam lvalue (untuk arm
/// `IrExpr::IncDec` — read-modify-write: index array/bit yang dinamis ikut
/// dibaca saat operand lvalue di-resolve).
fn collect_lvalue_read_index_exprs(lv: &IrLValue, out: &mut Vec<SignalId>) {
    match lv {
        IrLValue::ArrayIndex { index, .. }
        | IrLValue::ArrayRangeSelect { index, .. }
        | IrLValue::ExprPartSelect { base: index, .. }
        | IrLValue::HierRefIndex { index, .. } => {
            collect_read_signals_expr(index, out);
        }
        IrLValue::ArrayBitSelect { index, bit, .. } => {
            collect_read_signals_expr(index, out);
            collect_read_signals_expr(bit, out);
        }
        IrLValue::Concat(parts) => {
            for p in parts {
                collect_lvalue_read_index_exprs(p, out);
            }
        }
        IrLValue::Signal(..)
        | IrLValue::RangeSelect(..)
        | IrLValue::BitSelect(..)
        | IrLValue::ObjectField { .. }
        | IrLValue::HierRef(_) => {}
    }
}

/// Kumpulkan semua signal yang dibaca dari satu IR expression.
pub fn collect_read_signals_expr(expr: &IrExpr, out: &mut Vec<SignalId>) {
    match expr {
        IrExpr::Signal(id, _) | IrExpr::RangeSelect(id, ..) | IrExpr::BitSelect(id, _) => {
            out.push(*id);
        }
        // ArrayIndex dynamic (`rom[idx]`) — index juga sinyal yang DIBACA:
        // always_comb `val = rom[idx]` harus sensitivity ke {rom, idx}. Tanpa
        // ini idx berubah → val tak recompute (stuck di nilai lama).
        IrExpr::ArrayIndex {
            sig_id: id, index, ..
        } => {
            out.push(*id);
            collect_read_signals_expr(index, out);
        }
        IrExpr::Const(_) | IrExpr::String(_) | IrExpr::FillLit(_) => {}
        IrExpr::Concat(exprs) => {
            for e in exprs {
                collect_read_signals_expr(e, out);
            }
        }
        IrExpr::Replicate(_, inner) => {
            collect_read_signals_expr(inner, out);
        }
        IrExpr::UnaryOp(_, inner) => collect_read_signals_expr(inner, out),
        IrExpr::BinaryOp(_, lhs, rhs) => {
            collect_read_signals_expr(lhs, out);
            collect_read_signals_expr(rhs, out);
        }
        IrExpr::Cond(c, t, f) => {
            collect_read_signals_expr(c, out);
            collect_read_signals_expr(t, out);
            collect_read_signals_expr(f, out);
        }
        IrExpr::Signed(inner) => collect_read_signals_expr(inner, out),
        IrExpr::NewCall { args, .. } => {
            for arg in args {
                collect_read_signals_expr(arg, out);
            }
        }
        IrExpr::This => {}
        IrExpr::SysFunc { args, .. } => {
            for arg in args {
                collect_read_signals_expr(arg, out);
            }
        }
        IrExpr::MethodCall { obj, args, .. } => {
            collect_read_signals_expr(obj, out);
            for arg in args {
                collect_read_signals_expr(arg, out);
            }
        }
        IrExpr::MemberAccess { obj, .. } => {
            collect_read_signals_expr(obj, out);
        }
        IrExpr::ExprRangeSelect(inner, _, _) => {
            collect_read_signals_expr(inner, out);
        }
        IrExpr::ExprBitSelect(inner, _) => {
            collect_read_signals_expr(inner, out);
        }
        IrExpr::ExprPartSelect(inner, base_expr, width_expr) => {
            collect_read_signals_expr(inner, out);
            collect_read_signals_expr(base_expr, out);
            collect_read_signals_expr(width_expr, out);
        }
        IrExpr::DpiCall { args, .. } => {
            for arg in args {
                collect_read_signals_expr(arg, out);
            }
        }
        IrExpr::HierRef(_) => {}
        IrExpr::Inside { expr, list } => {
            collect_read_signals_expr(expr, out);
            for item in list {
                collect_read_signals_expr(item, out);
            }
        }
        IrExpr::InsideRange { expr, lo, hi } => {
            collect_read_signals_expr(expr, out);
            collect_read_signals_expr(lo, out);
            collect_read_signals_expr(hi, out);
        }
        IrExpr::Cast { expr, .. } => {
            collect_read_signals_expr(expr, out);
        }
        IrExpr::Dist { expr, .. } => {
            collect_read_signals_expr(expr, out);
        }
        IrExpr::StreamingConcat { slices, .. } => {
            for e in slices {
                collect_read_signals_expr(e, out);
            }
        }
        IrExpr::UdpLookup { args, .. } => {
            for arg in args {
                collect_read_signals_expr(arg, out);
            }
        }
        IrExpr::VifBinding { .. } => {}
        IrExpr::VirtualIfaceAccess { .. } => {}
        IrExpr::FuncCall { args, .. } => {
            for arg in args {
                collect_read_signals_expr(arg, out);
            }
        }
        // `i++` dalam ekspresi = read-modify-write: operand terbaca DAN
        // ditulis — kumpulkan keduanya (read + index lvalue) agar sensitivity
        // / analysis tidak kehilangan sinyal.
        IrExpr::IncDec { read, lv, .. } => {
            collect_read_signals_expr(read, out);
            collect_lvalue_read_index_exprs(lv, out);
        }
    }
}

/// Resolusi expression AST ke signal ID (jika berupa signal sederhana).
pub fn resolve_expr_signal(
    expr: &Expr,
    signal_map: &HashMap<Symbol, SignalId>,
) -> Option<SignalId> {
    match expr {
        Expr::Ident { name, .. } => signal_map.get(name).copied(),
        // Indexed/range-select (`sig[idx]`, `sig[a:b]`) → resolve ke signal dasar.
        Expr::BitSelect { expr: inner, .. } => resolve_expr_signal(inner, signal_map),
        Expr::RangeSelect { expr: inner, .. } => resolve_expr_signal(inner, signal_map),
        Expr::MethodCall { .. } => None,
        Expr::MemberAccess { .. } => None,
        // Paren (`(sig)`) → resolve ke signal dasar yang sama.
        Expr::Paren(inner) => resolve_expr_signal(inner, signal_map),
        _ => None,
    }
}

/// Deteksi reset sinkron dari body IR process.
/// Cari pola: if (signal) sebagai statement pertama.
pub fn detect_sync_reset(body: &[IrStmt]) -> Option<ResetInfo> {
    if let Some(IrStmt::If {
        cond: IrExpr::Signal(sig_id, _),
        ..
    }) = body.first()
    {
        return Some(ResetInfo {
            signal: *sig_id,
            polarity: true,
            r#async: false,
            value: LogicVec::new(1),
        });
    }
    None
}

/// Kumpulkan sensitivity list dari expression AST (untuk always_comb).
/// Kumpulkan nama identifier polos yang TIDAK dikenal dalam ekspresi (bukan
/// signal terdeklarasi, bukan parameter, bukan konstanta package, bukan
/// `$`-system, bukan scoped ident). Dipakai untuk implicit net pada continuous
/// assign — pola generated code OpenTitan (`assign tl_reg_h2d = tl_i;`,
/// `assign tl_o_pre = tl_reg_d2h;`) di mana net tak terdeklarasi dulu
/// dikoneksikan ke reg block yang di-optimalkan oleh reggen.
pub fn collect_implicit_net_idents(
    expr: &Expr,
    signal_map: &HashMap<Symbol, SignalId>,
    param_vals: &HashMap<Symbol, i64>,
    pkg_ctx: &HashMap<Symbol, i64>,
    out: &mut Vec<(Symbol, usize, usize)>,
) {
    match expr {
        Expr::Ident { name, line, col } => {
            if name == "this" || name.starts_with("$") {
                return;
            }
            if signal_map.contains_key(name)
                || param_vals.contains_key(name)
                || pkg_ctx.contains_key(name)
            {
                return;
            }
            out.push((*name, *line, *col));
        }
        Expr::ScopedIdent { .. }
        | Expr::Value(_)
        | Expr::String(_)
        | Expr::Null
        | Expr::FillLit(_) => {}
        Expr::BinaryOp { lhs, rhs, .. } => {
            collect_implicit_net_idents(lhs, signal_map, param_vals, pkg_ctx, out);
            collect_implicit_net_idents(rhs, signal_map, param_vals, pkg_ctx, out);
        }
        Expr::UnaryOp { expr: inner, .. } => {
            collect_implicit_net_idents(inner, signal_map, param_vals, pkg_ctx, out)
        }
        Expr::Concat(items) => {
            for e in items {
                collect_implicit_net_idents(e, signal_map, param_vals, pkg_ctx, out);
            }
        }
        Expr::Replicate { count, expr: inner } => {
            collect_implicit_net_idents(count, signal_map, param_vals, pkg_ctx, out);
            collect_implicit_net_idents(inner, signal_map, param_vals, pkg_ctx, out);
        }
        Expr::RangeSelect {
            expr: inner,
            msb,
            lsb,
        } => {
            collect_implicit_net_idents(inner, signal_map, param_vals, pkg_ctx, out);
            collect_implicit_net_idents(msb, signal_map, param_vals, pkg_ctx, out);
            collect_implicit_net_idents(lsb, signal_map, param_vals, pkg_ctx, out);
        }
        Expr::BitSelect { expr: inner, index } => {
            collect_implicit_net_idents(inner, signal_map, param_vals, pkg_ctx, out);
            collect_implicit_net_idents(index, signal_map, param_vals, pkg_ctx, out);
        }
        Expr::PartSelect {
            expr: inner,
            base,
            width,
        } => {
            collect_implicit_net_idents(inner, signal_map, param_vals, pkg_ctx, out);
            collect_implicit_net_idents(base, signal_map, param_vals, pkg_ctx, out);
            collect_implicit_net_idents(width, signal_map, param_vals, pkg_ctx, out);
        }
        Expr::TernaryOp {
            cond,
            true_expr,
            false_expr,
        } => {
            collect_implicit_net_idents(cond, signal_map, param_vals, pkg_ctx, out);
            collect_implicit_net_idents(true_expr, signal_map, param_vals, pkg_ctx, out);
            collect_implicit_net_idents(false_expr, signal_map, param_vals, pkg_ctx, out);
        }
        Expr::Paren(inner) => {
            collect_implicit_net_idents(inner, signal_map, param_vals, pkg_ctx, out)
        }
        Expr::IncDec { expr: inner, .. } => {
            collect_implicit_net_idents(inner, signal_map, param_vals, pkg_ctx, out)
        }
        Expr::Cast { expr: inner, .. } => {
            collect_implicit_net_idents(inner, signal_map, param_vals, pkg_ctx, out)
        }
        Expr::CastWidth { width, expr: inner } => {
            collect_implicit_net_idents(width, signal_map, param_vals, pkg_ctx, out);
            collect_implicit_net_idents(inner, signal_map, param_vals, pkg_ctx, out);
        }
        Expr::MemberAccess { obj, .. } => {
            // JANGAN rekursif ke obj: `uut.x` → `uut` adalah nama instance,
            // bukan signal standalone. Tanpa guard ini, `uut` dijadikan implicit
            // net 1-bit oleh Verilog-2001 semantic → elaborator salah treat
            // sebagai signal (bukan instance) → HierRef tidak di-emisi.
            // Untuk struct field access (`my_struct.field`), `my_struct` sudah
            // ada di signal_map sebagai signal yang dideklarasikan secara eksplisit.
            let _ = obj;
        }
        Expr::MethodCall { obj, args, .. } => {
            collect_implicit_net_idents(obj, signal_map, param_vals, pkg_ctx, out);
            for a in args {
                collect_implicit_net_idents(a, signal_map, param_vals, pkg_ctx, out);
            }
        }
        Expr::FuncCall { args, .. } => {
            for a in args {
                collect_implicit_net_idents(a, signal_map, param_vals, pkg_ctx, out);
            }
        }
        Expr::Inside {
            expr: inner,
            range_list,
        } => {
            collect_implicit_net_idents(inner, signal_map, param_vals, pkg_ctx, out);
            for it in range_list {
                collect_implicit_net_idents(it, signal_map, param_vals, pkg_ctx, out);
            }
        }
        Expr::StreamingConcat {
            slice_size, slices, ..
        } => {
            if let Some(ss) = slice_size {
                collect_implicit_net_idents(ss, signal_map, param_vals, pkg_ctx, out);
            }
            for s in slices {
                collect_implicit_net_idents(s, signal_map, param_vals, pkg_ctx, out);
            }
        }
        Expr::StructLit { members } => {
            for m in members {
                match m {
                    mivon_ast::expr::StructLitMember::Named(_, e)
                    | mivon_ast::expr::StructLitMember::Positional(e)
                    | mivon_ast::expr::StructLitMember::Default(e) => {
                        collect_implicit_net_idents(e, signal_map, param_vals, pkg_ctx, out)
                    }
                }
            }
        }
        Expr::Dist { expr: inner, items } => {
            collect_implicit_net_idents(inner, signal_map, param_vals, pkg_ctx, out);
            for it in items {
                match it {
                    mivon_ast::expr::DistItem::Value(e, _) => {
                        collect_implicit_net_idents(e, signal_map, param_vals, pkg_ctx, out);
                    }
                    mivon_ast::expr::DistItem::Range(a, b, _) => {
                        collect_implicit_net_idents(a, signal_map, param_vals, pkg_ctx, out);
                        collect_implicit_net_idents(b, signal_map, param_vals, pkg_ctx, out);
                    }
                }
            }
        }
    }
}

pub fn collect_sensitivity(expr: &Expr, signal_map: &HashMap<Symbol, SignalId>) -> Vec<SignalId> {
    match expr {
        Expr::Ident { name, .. } => signal_map.get(name).map(|&id| vec![id]).unwrap_or_default(),
        Expr::BinaryOp { lhs, rhs, .. } => {
            let mut v = collect_sensitivity(lhs, signal_map);
            v.extend(collect_sensitivity(rhs, signal_map));
            v
        }
        Expr::UnaryOp { expr: inner, .. } => collect_sensitivity(inner, signal_map),
        Expr::Concat(exprs) => exprs
            .iter()
            .flat_map(|e| collect_sensitivity(e, signal_map))
            .collect(),
        Expr::BitSelect { expr: inner, index } => {
            let mut v = collect_sensitivity(inner, signal_map);
            v.extend(collect_sensitivity(index, signal_map));
            v
        }
        Expr::RangeSelect {
            expr: inner,
            msb,
            lsb,
        } => {
            let mut v = collect_sensitivity(inner, signal_map);
            v.extend(collect_sensitivity(msb, signal_map));
            v.extend(collect_sensitivity(lsb, signal_map));
            v
        }
        Expr::PartSelect {
            expr: inner,
            base,
            width,
        } => {
            let mut v = collect_sensitivity(inner, signal_map);
            v.extend(collect_sensitivity(base, signal_map));
            v.extend(collect_sensitivity(width, signal_map));
            v
        }
        Expr::TernaryOp {
            cond,
            true_expr,
            false_expr,
        } => {
            let mut v = collect_sensitivity(cond, signal_map);
            v.extend(collect_sensitivity(true_expr, signal_map));
            v.extend(collect_sensitivity(false_expr, signal_map));
            v
        }
        Expr::MethodCall { obj, .. } => collect_sensitivity(obj, signal_map),
        Expr::MemberAccess { obj, .. } => collect_sensitivity(obj, signal_map),
        Expr::Dist { expr, .. } => collect_sensitivity(expr, signal_map),
        // Paren (`(sig)`) hanyalah pengelompokan — sinyal di dalamnya TETAP
        // sensitif. Sebelumnya jatuh ke `_ => vec![]` → assign/always_comb
        // tidak re-trigger saat sinyal di dalam tanda kurung berubah.
        Expr::Paren(inner) => collect_sensitivity(inner, signal_map),
        // Variant berisi sub-ekspresi yang bisa membawa signal — dulu jatuh ke
        // `_ => vec![]` → SENSITIVITY KOSONG senyap → assign tak re-evaluate
        // saat signal di dalamnya berubah (fuzzer: `logic'(mx[C1] > mx[C0])`
        // pada sel tree prim_max_tree — sel stale pasca update mx →
        // max_idx salah utk perubahan kedua, W2 `2/1` vs LRM `0/5`).
        Expr::Cast { expr: inner, .. } => collect_sensitivity(inner, signal_map),
        Expr::CastWidth { width, expr: inner } => {
            let mut v = collect_sensitivity(inner, signal_map);
            v.extend(collect_sensitivity(width, signal_map));
            v
        }
        Expr::FuncCall { args, .. } => args
            .iter()
            .flat_map(|a| collect_sensitivity(a, signal_map))
            .collect(),
        Expr::Replicate { count, expr: inner } => {
            let mut v = collect_sensitivity(inner, signal_map);
            v.extend(collect_sensitivity(count, signal_map));
            v
        }
        Expr::Inside {
            expr: inner,
            range_list,
        } => {
            let mut v = collect_sensitivity(inner, signal_map);
            for r in range_list {
                v.extend(collect_sensitivity(r, signal_map));
            }
            v
        }
        Expr::StreamingConcat {
            op: _,
            slice_size,
            slices,
        } => {
            let mut v = Vec::new();
            for s in slices {
                v.extend(collect_sensitivity(s, signal_map));
            }
            if let Some(ss) = slice_size {
                v.extend(collect_sensitivity(ss, signal_map));
            }
            v
        }
        _ => vec![],
    }
}
