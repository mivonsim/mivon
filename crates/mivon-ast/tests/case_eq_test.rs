use mivon_ast::const_eval_ext::{eval_expr, CVal, PkgCtx};
use mivon_ast::expr::{BinaryOp, Expr, Value};
use mivon_ast::types::PackageItem;
use mivon_core::intern::Symbol;
use std::collections::HashMap;

fn eval_scalar_cmp(a: i64, b: i64, op: BinaryOp) -> i64 {
    let scalars = HashMap::new();
    let arrays = HashMap::new();
    let pkg: HashMap<Symbol, HashMap<Symbol, PackageItem>> = HashMap::new();
    let structs = HashMap::new();
    let array_structs = HashMap::new();
    let ctx = PkgCtx {
        scalars: &scalars,
        arrays: &arrays,
        package_symbols: &pkg,
        structs: &structs,
        array_structs: &array_structs,
    };
    let e = Expr::BinaryOp {
        op,
        lhs: Box::new(Expr::Value(Value::Decimal(a))),
        rhs: Box::new(Expr::Value(Value::Decimal(b))),
    };
    match eval_expr(&e, &ctx, None) {
        Ok(CVal::Scalar(v)) => v,
        _ => 0,
    }
}

#[test]
fn test_case_eq_equal() {
    assert_eq!(eval_scalar_cmp(10, 10, BinaryOp::CaseEq), 1);
    assert_eq!(eval_scalar_cmp(10, 5, BinaryOp::CaseEq), 0);
}

#[test]
fn test_case_neq() {
    assert_eq!(eval_scalar_cmp(10, 5, BinaryOp::CaseNeq), 1);
    assert_eq!(eval_scalar_cmp(10, 10, BinaryOp::CaseNeq), 0);
}

#[test]
fn test_eq_wild_equal() {
    assert_eq!(eval_scalar_cmp(10, 10, BinaryOp::EqWild), 1);
    assert_eq!(eval_scalar_cmp(10, 5, BinaryOp::EqWild), 0);
}

#[test]
fn test_neq_wild() {
    assert_eq!(eval_scalar_cmp(10, 5, BinaryOp::NeqWild), 1);
    assert_eq!(eval_scalar_cmp(10, 10, BinaryOp::NeqWild), 0);
}
