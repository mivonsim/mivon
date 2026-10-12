//! Signedness helpers — extract dari mod.rs untuk Arsitektur-06
//! (elaborator monolithic → submodule).
//!
//! Prediksi signedness literal/param dan guard const-fold untuk referensi
//! param signed (probe p12f/p12g).

use std::collections::HashSet;

use mivon_ast::types::is_signed_type;
use mivon_ast::*;
use mivon_core::intern::Symbol;
use mivon_ir::*;

/// Signedness AST-expr dalam konteks param default (mirror const_eval: desimal
/// unsized signed; literal sized ber-suffix `s` signed; unary ±/~ mewarisi).
pub(crate) fn elab_expr_is_signed(e: &Expr) -> bool {
    match e {
        Expr::Value(Value::Decimal(_)) => true,
        Expr::Value(Value::Binary { is_signed, .. })
        | Expr::Value(Value::Hex { is_signed, .. })
        | Expr::Value(Value::Octal { is_signed, .. }) => *is_signed,
        Expr::Paren(inner) => elab_expr_is_signed(inner),
        Expr::UnaryOp { op, expr: inner } => match op {
            UnaryOp::Not
            | UnaryOp::ReductionAnd
            | UnaryOp::ReductionNand
            | UnaryOp::ReductionOr
            | UnaryOp::ReductionNor
            | UnaryOp::ReductionXor
            | UnaryOp::ReductionXnor => false,
            _ => elab_expr_is_signed(inner),
        },
        _ => false,
    }
}

/// Signedness deklarasi param SV (IEEE 1800 §6.11/§6.20): `int/integer/byte/
/// shortint/longint`/signed-typed → signed; tanpa tipe → signedness default
/// ekspresi (desimal signed; `localparam A = -2` → int implicit signed).
pub(crate) fn param_decl_is_signed(p: &ParamDecl) -> bool {
    if let Some(dt) = &p.dtype {
        return is_signed_type(dt);
    }
    match &p.default {
        Some(expr) => elab_expr_is_signed(expr),
        None => true, // implicit integer
    }
}

/// Emit konstanta param dengan signedness: signed → `Signed(Const)` agar
/// is_signed_expr + perbandingan/aritmetik (LRM §11.8.2) memperlakukannya
/// signed (`localparam int X = -2` → `X < 0` benar, `%0d` cetak -2).
pub(crate) fn const_or_signed(lv: LogicVec, is_signed: bool) -> IrExpr {
    if is_signed {
        IrExpr::Signed(Box::new(IrExpr::Const(lv)))
    } else {
        IrExpr::Const(lv)
    }
}

/// Apakah ekspresi mereferensikan param SIGNED (nama di `signed_params`)?
/// Dipakai guard const-fold: `const_eval_with_params` memperlakukan Ident
/// sebagai unsigned, sehingga `PD < 0` (localparam int -2) salah ter-fold
/// jadi false. Referensi param signed → biarkan jalur runtime yang memakai
/// IrExpr::Signed agar perbandingan/aritmetik benar (probe p12f/p12g).
pub(crate) fn expr_refs_signed_param(expr: &Expr, signed_params: &HashSet<Symbol>) -> bool {
    if signed_params.is_empty() {
        return false; // fast path: tidak ada param signed di module ini
    }
    match expr {
        Expr::Ident { name, .. } => signed_params.contains(name),
        Expr::ScopedIdent { package, item, .. } => signed_params.contains(&Symbol::intern(
            &format!("{}::{}", package.as_str(), item.as_str()),
        )),
        Expr::Paren(inner) => expr_refs_signed_param(inner, signed_params),
        Expr::UnaryOp { expr: inner, .. } => expr_refs_signed_param(inner, signed_params),
        Expr::BinaryOp { lhs, rhs, .. } => {
            expr_refs_signed_param(lhs, signed_params) || expr_refs_signed_param(rhs, signed_params)
        }
        Expr::TernaryOp {
            cond,
            true_expr,
            false_expr,
        } => {
            expr_refs_signed_param(cond, signed_params)
                || expr_refs_signed_param(true_expr, signed_params)
                || expr_refs_signed_param(false_expr, signed_params)
        }
        Expr::Concat(parts) => parts
            .iter()
            .any(|p| expr_refs_signed_param(p, signed_params)),
        Expr::Replicate { count, expr: inner } => {
            expr_refs_signed_param(count, signed_params)
                || expr_refs_signed_param(inner, signed_params)
        }
        Expr::MemberAccess { obj, .. } => expr_refs_signed_param(obj, signed_params),
        Expr::RangeSelect {
            expr: inner,
            msb,
            lsb,
        } => {
            expr_refs_signed_param(inner, signed_params)
                || expr_refs_signed_param(msb, signed_params)
                || expr_refs_signed_param(lsb, signed_params)
        }
        Expr::BitSelect { expr: inner, index } => {
            expr_refs_signed_param(inner, signed_params)
                || expr_refs_signed_param(index, signed_params)
        }
        Expr::PartSelect {
            expr: inner,
            base,
            width,
        } => {
            expr_refs_signed_param(inner, signed_params)
                || expr_refs_signed_param(base, signed_params)
                || expr_refs_signed_param(width, signed_params)
        }
        Expr::FuncCall { args, .. } => args
            .iter()
            .any(|a| expr_refs_signed_param(a, signed_params)),
        Expr::MethodCall {
            obj,
            args,
            with_clause,
            ..
        } => {
            expr_refs_signed_param(obj, signed_params)
                || args
                    .iter()
                    .any(|a| expr_refs_signed_param(a, signed_params))
                || with_clause
                    .as_ref()
                    .is_some_and(|w| expr_refs_signed_param(w, signed_params))
        }
        Expr::Cast { expr: inner, .. } | Expr::CastWidth { expr: inner, .. } => {
            expr_refs_signed_param(inner, signed_params)
        }
        Expr::Inside {
            expr: inner,
            range_list,
        } => {
            expr_refs_signed_param(inner, signed_params)
                || range_list
                    .iter()
                    .any(|r| expr_refs_signed_param(r, signed_params))
        }
        Expr::StreamingConcat { slices, .. } => slices
            .iter()
            .any(|s| expr_refs_signed_param(s, signed_params)),
        Expr::Dist { expr: inner, items } => {
            expr_refs_signed_param(inner, signed_params)
                || items.iter().any(|d| match d {
                    mivon_ast::DistItem::Value(e, _) | mivon_ast::DistItem::Range(e, _, _) => {
                        expr_refs_signed_param(e, signed_params)
                    }
                })
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_refs_signed_param_ident() {
        let mut set = HashSet::new();
        set.insert(Symbol::intern("PD"));
        let hit = Expr::Ident {
            name: Symbol::intern("PD"),
            line: 0,
            col: 0,
        };
        let miss = Expr::Ident {
            name: Symbol::intern("OTHER"),
            line: 0,
            col: 0,
        };
        assert!(expr_refs_signed_param(&hit, &set));
        assert!(!expr_refs_signed_param(&miss, &set));
        assert!(!expr_refs_signed_param(&hit, &HashSet::new()));
    }
}
