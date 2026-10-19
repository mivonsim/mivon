use mivon_ast::const_eval_ext::{eval_expr, CVal, PkgCtx};
use mivon_ast::expr::{Expr, Value};
use mivon_ast::types::PackageItem;
use mivon_core::intern::Symbol;
use std::collections::HashMap;

fn eval(e: &Expr) -> CVal {
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
    eval_expr(e, &ctx, None).unwrap()
}

#[test]
fn test_inside_range() {
    let e = Expr::Inside {
        expr: Box::new(Expr::Value(Value::Decimal(3))),
        range_list: vec![Expr::RangeSelect {
            expr: Box::new(Expr::Value(Value::Decimal(0))),
            msb: Box::new(Expr::Value(Value::Decimal(1))),
            lsb: Box::new(Expr::Value(Value::Decimal(5))),
        }],
    };
    assert_eq!(eval(&e), CVal::Scalar(1));
}

#[test]
fn test_inside_exact_and_miss() {
    let hit = Expr::Inside {
        expr: Box::new(Expr::Value(Value::Decimal(7))),
        range_list: vec![Expr::Value(Value::Decimal(7))],
    };
    assert_eq!(eval(&hit), CVal::Scalar(1));
    let miss = Expr::Inside {
        expr: Box::new(Expr::Value(Value::Decimal(8))),
        range_list: vec![Expr::Value(Value::Decimal(7))],
    };
    assert_eq!(eval(&miss), CVal::Scalar(0));
}

#[test]
fn test_incdec() {
    let pre_inc = Expr::IncDec { expr: Box::new(Expr::Value(Value::Decimal(5))), inc: true, pre: true };
    assert_eq!(eval(&pre_inc), CVal::Scalar(6));
    let post_inc = Expr::IncDec { expr: Box::new(Expr::Value(Value::Decimal(5))), inc: true, pre: false };
    assert_eq!(eval(&post_inc), CVal::Scalar(5));
    let pre_dec = Expr::IncDec { expr: Box::new(Expr::Value(Value::Decimal(5))), inc: false, pre: true };
    assert_eq!(eval(&pre_dec), CVal::Scalar(4));
}

#[test]
fn test_dist_returns_value() {
    let e = Expr::Dist { expr: Box::new(Expr::Value(Value::Decimal(9))), items: vec![] };
    assert_eq!(eval(&e), CVal::Scalar(9));
}

#[test]
fn test_streaming_concat_order() {
    let right = Expr::StreamingConcat {
        op: String::from(">>"),
        slice_size: None,
        slices: vec![Expr::Value(Value::Decimal(1)), Expr::Value(Value::Decimal(2))],
    };
    assert_eq!(eval(&right), CVal::Array(vec![CVal::Scalar(1), CVal::Scalar(2)]));
    let left = Expr::StreamingConcat {
        op: String::from("<<"),
        slice_size: None,
        slices: vec![Expr::Value(Value::Decimal(1)), Expr::Value(Value::Decimal(2))],
    };
    assert_eq!(eval(&left), CVal::Array(vec![CVal::Scalar(2), CVal::Scalar(1)]));
}

#[test]
fn test_method_size() {
    let s = Expr::MethodCall {
        obj: Box::new(Expr::String(String::from("hello"))),
        method: Symbol::intern("size"),
        args: vec![],
        with_clause: None,
    };
    assert_eq!(eval(&s), CVal::Scalar(5));
    let a = Expr::MethodCall {
        obj: Box::new(Expr::Concat(vec![
            Expr::Value(Value::Decimal(1)),
            Expr::Value(Value::Decimal(2)),
            Expr::Value(Value::Decimal(3)),
        ])),
        method: Symbol::intern("len"),
        args: vec![],
        with_clause: None,
    };
    assert_eq!(eval(&a), CVal::Scalar(3));
}

#[test]
fn test_real_truncates() {
    assert_eq!(eval(&Expr::Value(Value::Real(3.9))), CVal::Scalar(3));
}
