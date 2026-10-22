//! Width helpers for `mlint`.

use std::collections::HashMap;

use mivon_ast::expr::Expr;
use mivon_core::intern::Symbol;

use super::Finding;

pub(crate) fn decl_width(d: &mivon_ast::types::Decl, v: &mivon_ast::types::DeclVar) -> usize {
    if let Some(r) = &v.range {
        return r.width();
    }
    match &d.dtype {
        mivon_ast::types::DataType::Byte => 8,
        mivon_ast::types::DataType::Shortint => 16,
        mivon_ast::types::DataType::Int | mivon_ast::types::DataType::Integer => 32,
        mivon_ast::types::DataType::Longint => 64,
        mivon_ast::types::DataType::Bit | mivon_ast::types::DataType::Logic => 1,
        _ => 1,
    }
}

pub(crate) fn expr_width(e: &Expr, declared: &HashMap<Symbol, usize>) -> Option<usize> {
    match e {
        Expr::Ident { name, .. } => declared.get(name).copied(),
        Expr::Value(v) => Some(match v {
            mivon_ast::expr::Value::Binary { width, .. }
            | mivon_ast::expr::Value::Hex { width, .. }
            | mivon_ast::expr::Value::Octal { width, .. } => width.unwrap_or(1),
            mivon_ast::expr::Value::Decimal(_) => 32,
            mivon_ast::expr::Value::Real(_) => 64,
        }),
        Expr::Concat(parts) => {
            let mut total = 0usize;
            for p in parts {
                total += expr_width(p, declared)?;
            }
            Some(total)
        }
        Expr::BitSelect { .. } => Some(1),
        Expr::RangeSelect { expr, msb, lsb, .. } => {
            if let (
                Expr::Value(mivon_ast::expr::Value::Decimal(m)),
                Expr::Value(mivon_ast::expr::Value::Decimal(l)),
            ) = (&**msb, &**lsb)
            {
                Some(m.abs_diff(*l) as usize + 1)
            } else {
                expr_width(expr, declared)
            }
        }
        Expr::BinaryOp { lhs, rhs, op } => {
            let lw = expr_width(lhs, declared)?;
            let rw = expr_width(rhs, declared)?;
            match op {
                mivon_ast::expr::BinaryOp::Eq
                | mivon_ast::expr::BinaryOp::Neq
                | mivon_ast::expr::BinaryOp::CaseEq
                | mivon_ast::expr::BinaryOp::CaseNeq
                | mivon_ast::expr::BinaryOp::Lt
                | mivon_ast::expr::BinaryOp::Le
                | mivon_ast::expr::BinaryOp::Gt
                | mivon_ast::expr::BinaryOp::Ge
                | mivon_ast::expr::BinaryOp::LogicalAnd
                | mivon_ast::expr::BinaryOp::LogicalOr => Some(1),
                _ => Some(lw.max(rw)),
            }
        }
        Expr::UnaryOp { expr, .. } => expr_width(expr, declared),
        Expr::TernaryOp {
            true_expr,
            false_expr,
            ..
        } => {
            let t = expr_width(true_expr, declared)?;
            let f = expr_width(false_expr, declared)?;
            Some(t.max(f))
        }
        Expr::Paren(inner) => expr_width(inner, declared),
        Expr::FillLit(_) => Some(1),
        Expr::Cast { expr, .. } => expr_width(expr, declared),
        Expr::CastWidth { width, .. } => expr_width(width, declared),
        Expr::MemberAccess { .. } => None,
        _ => None,
    }
}

pub(crate) fn check_width(
    scope: &str,
    lhs: &Expr,
    rhs: &Expr,
    declared: &HashMap<Symbol, usize>,
    out: &mut Vec<Finding>,
) {
    let Some(lw) = expr_width(lhs, declared) else {
        return;
    };
    let Some(rw) = expr_width(rhs, declared) else {
        return;
    };
    if lw != rw {
        out.push(Finding {
            module: scope.to_string(),
            check: "width",
            severity: "W",
            message: format!("width mismatch: lhs {} bit vs rhs {} bit", lw, rw),
        });
    }
}