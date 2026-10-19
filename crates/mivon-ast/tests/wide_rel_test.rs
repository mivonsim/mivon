use mivon_ast::const_eval_ext::{eval_expr, CVal, PkgCtx};
use mivon_ast::expr::{BinaryOp, Expr};
use mivon_ast::types::PackageItem;
use mivon_core::intern::Symbol;
use std::collections::HashMap;

fn eval_rel(a: &[i64], b: &[i64], op: BinaryOp) -> i64 {
    let scalars = HashMap::new();
    let mut arrays = HashMap::new();
    arrays.insert(Symbol::intern("A"), a.to_vec());
    arrays.insert(Symbol::intern("B"), b.to_vec());
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
        lhs: Box::new(Expr::Ident { name: Symbol::intern("A"), line: 0, col: 0 }),
        rhs: Box::new(Expr::Ident { name: Symbol::intern("B"), line: 0, col: 0 }),
    };
    match eval_expr(&e, &ctx, None) {
        Ok(CVal::Scalar(v)) => v,
        _ => 99,
    }
}

#[test]
fn test_wide_rel_less() {
    assert_eq!(eval_rel(&[5, 1], &[5, 2], BinaryOp::Lt), 1);
    assert_eq!(eval_rel(&[5, 1], &[5, 2], BinaryOp::Ge), 0);
}

#[test]
fn test_wide_rel_msb_dominates() {
    assert_eq!(eval_rel(&[1, 9], &[2, 0], BinaryOp::Lt), 1);
    assert_eq!(eval_rel(&[2, 0], &[1, 9], BinaryOp::Gt), 1);
}

#[test]
fn test_wide_rel_equal() {
    assert_eq!(eval_rel(&[7, 7], &[7, 7], BinaryOp::Le), 1);
    assert_eq!(eval_rel(&[7, 7], &[7, 7], BinaryOp::Ge), 1);
    assert_eq!(eval_rel(&[7, 7], &[7, 7], BinaryOp::Lt), 0);
    assert_eq!(eval_rel(&[7, 7], &[7, 7], BinaryOp::Gt), 0);
}

#[test]
fn test_wide_rel_leading_zero_pad() {
    assert_eq!(eval_rel(&[1], &[0, 1], BinaryOp::Eq), 0);
    assert_eq!(eval_rel(&[1], &[0, 1], BinaryOp::Le), 1);
    assert_eq!(eval_rel(&[1], &[0, 1], BinaryOp::Ge), 1);
}
