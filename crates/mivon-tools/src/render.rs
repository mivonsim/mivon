//! Modul presentasi tunggal untuk output tool mivon-tools.
//!
//! Mengonsolidasi semua helper cetak (section/kv/footer/human_bytes/
//! expr_to_string) ke dalam satu tempat agar tidak tersebar dan duplikat
//! di tiap tool. Tool lain cukup `use crate::render::*;` atau memanggil
//! `crate::section(...)` (di-re-export dari lib.rs).

use mivon_ast::expr::{BinaryOp, DistItem, Expr, StructLitMember, UnaryOp, Value};
use mivon_ir::LogicVal;

/// Cetak angka byte dalam bentuk manusia-readable (B/KB/MB/GB/TB).
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{} B", n)
    } else {
        format!("{:.1} {}", v, UNITS[u])
    }
}

/// Cetak header section yang konsisten.
pub fn section(title: &str) {
    println!("\n── {} ──", title);
}

/// Cetak satu baris bullet.
pub fn bullet(text: &str) {
    println!("  • {}", text);
}

/// Cetak pasangan label/value rata kiri.
pub fn kv(label: &str, value: impl std::fmt::Display) {
    println!("  {:<26} {}", label, value);
}

/// Cetak footer dengan pesan sukses/error count.
pub fn footer(ok: usize, warn: usize) {
    println!("\n── selesai: {} ok, {} warning ──", ok, warn);
}

/// Render ekspresi AST menjadi string ringkas (untuk output tool).
pub fn expr_to_string(e: &Expr) -> String {
    match e {
        Expr::Value(v) => match v {
            Value::Binary { bits, .. } | Value::Hex { bits, .. } | Value::Octal { bits, .. } => {
                format!("{}{}", base_prefix(v), bits)
            }
            Value::Decimal(d) => d.to_string(),
            Value::Real(r) => r.to_string(),
        },
        Expr::FillLit(v) => format!(
            "'{}",
            match v {
                LogicVal::Zero => "0",
                LogicVal::One => "1",
                LogicVal::X => "x",
                LogicVal::Z => "z",
            }
        ),
        Expr::Ident { name, .. } => name.as_str().to_string(),
        Expr::FuncCall { name, args, .. } => {
            let a: Vec<String> = args.iter().map(expr_to_string).collect();
            format!("{}({})", name.as_str(), a.join(", "))
        }
        Expr::RangeSelect { expr, msb, lsb, .. } => {
            format!(
                "{}[{}:{}]",
                expr_to_string(expr),
                expr_to_string(msb),
                expr_to_string(lsb)
            )
        }
        Expr::BitSelect { expr, index, .. } => {
            format!("{}[{}]", expr_to_string(expr), expr_to_string(index))
        }
        Expr::PartSelect {
            expr, base, width, ..
        } => {
            format!(
                "{}[{} +: {}]",
                expr_to_string(expr),
                expr_to_string(base),
                expr_to_string(width)
            )
        }
        Expr::Concat(parts) => {
            let p: Vec<String> = parts.iter().map(expr_to_string).collect();
            format!("'{{{}}}", p.join(", "))
        }
        Expr::Replicate { count, expr } => {
            format!("{{{}{{{}}}}}", expr_to_string(count), expr_to_string(expr))
        }
        Expr::UnaryOp { op, expr } => format!("{}{}", unary_op_str(op), expr_to_string(expr)),
        Expr::BinaryOp { op, lhs, rhs } => {
            format!(
                "{} {} {}",
                expr_to_string(lhs),
                bin_op_str(op),
                expr_to_string(rhs)
            )
        }
        Expr::TernaryOp {
            cond,
            true_expr,
            false_expr,
        } => format!(
            "{} ? {} : {}",
            expr_to_string(cond),
            expr_to_string(true_expr),
            expr_to_string(false_expr)
        ),
        Expr::Paren(inner) => format!("({})", expr_to_string(inner)),
        Expr::IncDec { expr, inc, pre } => {
            let op = if *inc { "++" } else { "--" };
            if *pre {
                format!("{}{}", op, expr_to_string(expr))
            } else {
                format!("{}{}", expr_to_string(expr), op)
            }
        }
        Expr::String(s) => format!("\"{}\"", s),
        Expr::MemberAccess { obj, field } => {
            format!("{}.{}", expr_to_string(obj), field.as_str())
        }
        Expr::MethodCall {
            obj, method, args, ..
        } => {
            let a: Vec<String> = args.iter().map(expr_to_string).collect();
            format!(
                "{}.{}({})",
                expr_to_string(obj),
                method.as_str(),
                a.join(", ")
            )
        }
        Expr::ScopedIdent { package, item, .. } => {
            format!("{}::{}", package.as_str(), item.as_str())
        }
        Expr::Cast { dtype, expr } => format!("{}({})", dtype.as_str(), expr_to_string(expr)),
        Expr::CastWidth { width, expr } => {
            format!("{}'({})", expr_to_string(width), expr_to_string(expr))
        }
        Expr::Null => "null".to_string(),
        Expr::Inside { expr, range_list } => {
            let r: Vec<String> = range_list.iter().map(expr_to_string).collect();
            format!("{} inside {{ {} }}", expr_to_string(expr), r.join(", "))
        }
        Expr::StreamingConcat {
            op,
            slice_size,
            slices,
        } => {
            let s: Vec<String> = slices.iter().map(expr_to_string).collect();
            let w = slice_size
                .as_ref()
                .map(|e| format!("[{}]", expr_to_string(e)))
                .unwrap_or_default();
            format!("{{{} {} {}}}", op, w, s.join(", "))
        }
        Expr::Dist { expr, items } => {
            let its: Vec<String> = items
                .iter()
                .map(|d| match d {
                    DistItem::Value(v, _) => expr_to_string(v),
                    DistItem::Range(a, b, _) => {
                        format!("[{}:{}]", expr_to_string(a), expr_to_string(b))
                    }
                })
                .collect();
            format!("{} dist {{ {} }}", expr_to_string(expr), its.join(", "))
        }
        Expr::StructLit { members } => {
            let inner: Vec<String> = members
                .iter()
                .map(|m| match m {
                    StructLitMember::Named(n, e) => {
                        format!("{}: {}", n.as_str(), expr_to_string(e))
                    }
                    StructLitMember::Positional(e) => expr_to_string(e),
                    StructLitMember::Default(e) => {
                        format!("default: {}", expr_to_string(e))
                    }
                })
                .collect();
            format!("'{{{}}}", inner.join(", "))
        }
    }
}

fn base_prefix(v: &Value) -> String {
    match v {
        Value::Binary { width, .. } => width
            .map(|w| format!("{}'b", w))
            .unwrap_or_else(|| "'b".into()),
        Value::Hex { width, .. } => width
            .map(|w| format!("{}'h", w))
            .unwrap_or_else(|| "'h".into()),
        Value::Octal { width, .. } => width
            .map(|w| format!("{}'o", w))
            .unwrap_or_else(|| "'o".into()),
        _ => "".into(),
    }
}

fn unary_op_str(op: &UnaryOp) -> &'static str {
    match op {
        UnaryOp::Plus => "+",
        UnaryOp::Minus => "-",
        UnaryOp::BitNot => "~",
        UnaryOp::Not => "!",
        UnaryOp::ReductionAnd => "&",
        UnaryOp::ReductionNand => "~&",
        UnaryOp::ReductionOr => "|",
        UnaryOp::ReductionNor => "~|",
        UnaryOp::ReductionXor => "^",
        UnaryOp::ReductionXnor => "~^",
    }
}

fn bin_op_str(op: &BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Mod => "%",
        BinaryOp::Power => "**",
        BinaryOp::Eq => "==",
        BinaryOp::Neq => "!=",
        BinaryOp::CaseEq => "===",
        BinaryOp::CaseNeq => "!==",
        BinaryOp::EqWild => "==?",
        BinaryOp::NeqWild => "!=?",
        BinaryOp::Lt => "<",
        BinaryOp::Le => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::Ge => ">=",
        BinaryOp::LogicalAnd => "&&",
        BinaryOp::LogicalOr => "||",
        BinaryOp::BitAnd => "&",
        BinaryOp::BitOr => "|",
        BinaryOp::BitXor => "^",
        BinaryOp::BitXnor => "~^",
        BinaryOp::Shl => "<<",
        BinaryOp::Shr => ">>",
        BinaryOp::Sshl => "<<<",
        BinaryOp::Sshr => ">>>",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mivon_ast::expr::Value;

    #[test]
    fn test_human_bytes() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(1048576), "1.0 MB");
    }

    #[test]
    fn test_expr_to_string_literal() {
        let e = Expr::Value(Value::Decimal(42));
        assert_eq!(expr_to_string(&e), "42");
        let s = Expr::String("hi".to_string());
        assert_eq!(expr_to_string(&s), "\"hi\"");
    }

    #[test]
    fn test_expr_to_string_binop() {
        let e = Expr::BinaryOp {
            op: BinaryOp::Add,
            lhs: Box::new(Expr::Value(Value::Decimal(1))),
            rhs: Box::new(Expr::Value(Value::Decimal(2))),
        };
        assert_eq!(expr_to_string(&e), "1 + 2");
    }

    #[test]
    fn test_bullet_does_not_panic() {
        // bullet menulis ke stdout; cukup pastikan tidak panic.
        bullet("x");
        section("judul");
        kv("k", 1);
        footer(1, 0);
    }
}
