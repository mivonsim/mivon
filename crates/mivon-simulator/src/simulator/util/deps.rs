//! Pemetaan operator AST→IR dan ekstraksi dependency sinyal dari ekspresi IR.
//!
//! Dipisah dari `util.rs`: dua helper murni yang dipakai elaborator/engine
//! untuk analisis dataflow (comb loop, sensitivity, DAG).

use mivon_ast::*;
use mivon_ir::*;

pub fn map_ast_binary_op(op: &BinaryOp) -> Result<BinaryIrOp, String> {
    match op {
        BinaryOp::Add => Ok(BinaryIrOp::Add),
        BinaryOp::Sub => Ok(BinaryIrOp::Sub),
        BinaryOp::Mul => Ok(BinaryIrOp::Mul),
        BinaryOp::Div => Ok(BinaryIrOp::Div),
        BinaryOp::Mod => Ok(BinaryIrOp::Mod),
        BinaryOp::Power => Ok(BinaryIrOp::Power),
        BinaryOp::Eq => Ok(BinaryIrOp::Eq),
        BinaryOp::Neq => Ok(BinaryIrOp::Neq),
        BinaryOp::CaseEq => Ok(BinaryIrOp::CaseEq),
        BinaryOp::CaseNeq => Ok(BinaryIrOp::CaseNeq),
        BinaryOp::EqWild => Ok(BinaryIrOp::Eq),
        BinaryOp::NeqWild => Ok(BinaryIrOp::Neq),
        BinaryOp::Lt => Ok(BinaryIrOp::Lt),
        BinaryOp::Le => Ok(BinaryIrOp::Le),
        BinaryOp::Gt => Ok(BinaryIrOp::Gt),
        BinaryOp::Ge => Ok(BinaryIrOp::Ge),
        BinaryOp::BitAnd => Ok(BinaryIrOp::BitAnd),
        BinaryOp::BitOr => Ok(BinaryIrOp::BitOr),
        BinaryOp::BitXor => Ok(BinaryIrOp::BitXor),
        BinaryOp::BitXnor => Ok(BinaryIrOp::BitXnor),
        BinaryOp::Shl => Ok(BinaryIrOp::Shl),
        BinaryOp::Shr => Ok(BinaryIrOp::Shr),
        BinaryOp::Sshl => Ok(BinaryIrOp::Sshl),
        BinaryOp::Sshr => Ok(BinaryIrOp::Sshr),
        BinaryOp::LogicalAnd => Ok(BinaryIrOp::LogicalAnd),
        BinaryOp::LogicalOr => Ok(BinaryIrOp::LogicalOr),
    }
}

pub fn map_ast_unary_op(op: &UnaryOp) -> Result<UnaryIrOp, String> {
    match op {
        UnaryOp::Plus => Ok(UnaryIrOp::Plus),
        UnaryOp::Minus => Ok(UnaryIrOp::Minus),
        UnaryOp::BitNot => Ok(UnaryIrOp::BitNot),
        UnaryOp::Not => Ok(UnaryIrOp::Not),
        UnaryOp::ReductionAnd => Ok(UnaryIrOp::RedAnd),
        UnaryOp::ReductionNand => Ok(UnaryIrOp::RedNand),
        UnaryOp::ReductionOr => Ok(UnaryIrOp::RedOr),
        UnaryOp::ReductionNor => Ok(UnaryIrOp::RedNor),
        UnaryOp::ReductionXor => Ok(UnaryIrOp::RedXor),
        UnaryOp::ReductionXnor => Ok(UnaryIrOp::RedXnor),
    }
}

pub fn extract_signal_deps(expr: &IrExpr) -> Vec<SignalId> {
    let mut deps = Vec::new();
    extract_signal_deps_inner(expr, &mut deps);
    deps
}

/// SignalId dasar dari sebuah lvalue (untuk arm `IrExpr::IncDec` — target
/// write-back juga merupakan dependency). `None` untuk lvalue hierarkis
/// yang belum ter-resolve (nama saja, tanpa SignalId).
fn lvalue_dep_id(lv: &IrLValue) -> Option<SignalId> {
    match lv {
        IrLValue::Signal(id, _)
        | IrLValue::RangeSelect(id, ..)
        | IrLValue::BitSelect(id, _)
        | IrLValue::ArrayIndex { sig_id: id, .. }
        | IrLValue::ArrayRangeSelect { sig_id: id, .. }
        | IrLValue::ArrayBitSelect { sig_id: id, .. }
        | IrLValue::ExprPartSelect { sig_id: id, .. }
        | IrLValue::ObjectField { sig_id: id, .. } => Some(*id),
        IrLValue::Concat(items) => items.first().and_then(lvalue_dep_id),
        IrLValue::HierRef(_) | IrLValue::HierRefIndex { .. } => None,
        IrLValue::VifField { .. } => None,
    }
}

pub fn extract_signal_deps_inner(expr: &IrExpr, deps: &mut Vec<SignalId>) {
    match expr {
        IrExpr::Signal(id, _) => {
            if !deps.contains(id) {
                deps.push(*id);
            }
        }
        IrExpr::RangeSelect(id, _, _)
        | IrExpr::BitSelect(id, _)
        | IrExpr::ArrayIndex { sig_id: id, .. } => {
            if !deps.contains(id) {
                deps.push(*id);
            }
        }
        IrExpr::ExprRangeSelect(e, _, _) | IrExpr::ExprBitSelect(e, _) => {
            extract_signal_deps_inner(e, deps);
        }
        IrExpr::ExprPartSelect(e1, e2, e3) => {
            extract_signal_deps_inner(e1, deps);
            extract_signal_deps_inner(e2, deps);
            extract_signal_deps_inner(e3, deps);
        }
        IrExpr::Concat(exprs) => {
            for e in exprs {
                extract_signal_deps_inner(e, deps);
            }
        }
        IrExpr::Replicate(_, e) => {
            extract_signal_deps_inner(e, deps);
        }
        IrExpr::UnaryOp(_, e) => {
            extract_signal_deps_inner(e, deps);
        }
        IrExpr::BinaryOp(_, e1, e2) => {
            extract_signal_deps_inner(e1, deps);
            extract_signal_deps_inner(e2, deps);
        }
        IrExpr::Cond(c, t, e) => {
            extract_signal_deps_inner(c, deps);
            extract_signal_deps_inner(t, deps);
            extract_signal_deps_inner(e, deps);
        }
        IrExpr::Signed(e) => {
            extract_signal_deps_inner(e, deps);
        }
        IrExpr::MethodCall { obj, args, .. } => {
            extract_signal_deps_inner(obj, deps);
            for a in args {
                extract_signal_deps_inner(a, deps);
            }
        }
        IrExpr::MemberAccess { obj, .. } => {
            extract_signal_deps_inner(obj, deps);
        }
        IrExpr::NewCall { args, .. } => {
            for a in args {
                extract_signal_deps_inner(a, deps);
            }
        }
        IrExpr::SysFunc { args, .. } => {
            for a in args {
                extract_signal_deps_inner(a, deps);
            }
        }
        IrExpr::DpiCall { args, .. } => {
            for a in args {
                extract_signal_deps_inner(a, deps);
            }
        }
        IrExpr::HierRef(_) => {}
        IrExpr::Inside { expr, list } => {
            extract_signal_deps_inner(expr, deps);
            for item in list {
                extract_signal_deps_inner(item, deps);
            }
        }
        IrExpr::InsideRange { expr, lo, hi } => {
            extract_signal_deps_inner(expr, deps);
            extract_signal_deps_inner(lo, deps);
            extract_signal_deps_inner(hi, deps);
        }
        IrExpr::Cast { expr, .. } => {
            extract_signal_deps_inner(expr, deps);
        }
        IrExpr::Dist { expr, .. } => {
            extract_signal_deps_inner(expr, deps);
        }
        IrExpr::StreamingConcat { slices, .. } => {
            for e in slices {
                extract_signal_deps_inner(e, deps);
            }
        }
        IrExpr::UdpLookup { args, .. } => {
            for a in args {
                extract_signal_deps_inner(a, deps);
            }
        }
        IrExpr::VifBinding { .. } => {}
        IrExpr::VirtualIfaceAccess { .. } => {}
        IrExpr::FuncCall { args, .. } => {
            for a in args {
                extract_signal_deps_inner(a, deps);
            }
        }
        // RMW `i++`: operand terbaca + target lvalue dibaca/ditulis.
        IrExpr::IncDec { read, lv, .. } => {
            extract_signal_deps_inner(read, deps);
            if let Some(id) = lvalue_dep_id(lv) {
                if !deps.contains(&id) {
                    deps.push(id);
                }
            }
        }
        IrExpr::Const(_)
        | IrExpr::RealConst(_)
        | IrExpr::FillLit(_)
        | IrExpr::String(_)
        | IrExpr::This => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regresi pemisahan modul: dependency sinyal unik dan berurutan.
    #[test]
    fn signal_deps_dedup_and_order() {
        let e = IrExpr::BinaryOp(
            BinaryIrOp::Add,
            Box::new(IrExpr::Signal(3, 8)),
            Box::new(IrExpr::Signal(3, 8)),
        );
        assert_eq!(extract_signal_deps(&e), vec![3]);
    }

    /// Operator wildcard dipetakan ke Eq/Neq (LRM: ==? / !=? tanpa X-propagasi
    /// khusus di IR ini).
    #[test]
    fn wildcard_ops_map_to_eq_neq() {
        assert_eq!(
            map_ast_binary_op(&BinaryOp::EqWild).unwrap(),
            BinaryIrOp::Eq
        );
        assert_eq!(
            map_ast_binary_op(&BinaryOp::NeqWild).unwrap(),
            BinaryIrOp::Neq
        );
    }
}
