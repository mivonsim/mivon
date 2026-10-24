use mivon_ast::const_eval_ext::{eval_expr, CVal, PkgCtx};
use mivon_ast::expr::{BinaryOp, Expr};
use mivon_ast::types::PackageItem;
use mivon_core::intern::Symbol;
use std::collections::HashMap;

/// Evaluasi (A OP B) di mana A dan B adalah packed vector lebar (>64-bit)
/// yang direpresentasikan sebagai CVal::Array. Menguji langsung path == / !=
/// antara dua array (fix untuk error "comparison of wide packed vectors not
/// supported" di const_eval_ext::eval_expr).
fn eval_wide_cmp(a: &[i64], b: &[i64], op: BinaryOp) -> i64 {
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
        lhs: Box::new(Expr::Ident {
            name: Symbol::intern("A"),
            line: 0,
            col: 0,
        }),
        rhs: Box::new(Expr::Ident {
            name: Symbol::intern("B"),
            line: 0,
            col: 0,
        }),
    };
    match eval_expr(&e, &ctx, None) {
        Ok(CVal::Scalar(v)) => v,
        other => panic!("unexpected eval result: {:?}", other),
    }
}

#[test]
fn test_wide_vector_eq_equal() {
    assert_eq!(eval_wide_cmp(&[1, 2, 3, 4], &[1, 2, 3, 4], BinaryOp::Eq), 1);
    assert_eq!(
        eval_wide_cmp(&[1, 2, 3, 4], &[1, 2, 3, 4], BinaryOp::Neq),
        0
    );
}

#[test]
fn test_wide_vector_eq_different() {
    assert_eq!(eval_wide_cmp(&[1, 2, 3, 4], &[9, 9, 9, 9], BinaryOp::Eq), 0);
    assert_eq!(
        eval_wide_cmp(&[1, 2, 3, 4], &[9, 9, 9, 9], BinaryOp::Neq),
        1
    );
}

#[test]
fn test_wide_vector_eq_diff_len() {
    assert_eq!(eval_wide_cmp(&[1, 2, 3], &[1, 2, 3, 4], BinaryOp::Eq), 0);
    assert_eq!(eval_wide_cmp(&[1, 2, 3], &[1, 2, 3, 4], BinaryOp::Neq), 1);
}
