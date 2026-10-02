//! Codegen — ekspresi SV (`emit_expr`) + tipe (`emit_type`).
//! 1 file = 1 tanggung jawab.

use crate::ast::*;

/// Emit tipe SV.
pub(crate) fn emit_type(t: &MvType) -> String {
    match t {
        MvType::Bit => "bit".into(),
        MvType::Logic(None) => "logic".into(),
        MvType::Logic(Some((a, b))) => format!("logic [{}:{}]", emit_expr(a), emit_expr(b)),
        MvType::Signed(inner) => {
            // `signed logic[8]` → `logic signed [7:0]`
            match inner.as_ref() {
                MvType::Logic(r) => match r {
                    Some((a, b)) => format!("logic signed [{}:{}]", emit_expr(a), emit_expr(b)),
                    None => "logic signed".into(),
                },
                MvType::Bit => "logic signed".into(),
                other => format!("signed {}", emit_type(other)),
            }
        }
        MvType::Int => "int".into(),
        MvType::Uint => "logic [31:0]".into(),
        MvType::LongInt => "longint".into(),
        MvType::ULongInt => "longint unsigned".into(),
        MvType::ShortInt => "shortint".into(),
        MvType::Byte => "byte".into(),
        MvType::Real => "real".into(),
        MvType::Time => "time".into(),
        MvType::Str => "string".into(),
        MvType::Named(s, ..) => s.clone(),
        MvType::Array(inner, dims) => {
            let dims_s: Vec<String> = dims.iter().map(|d| format!("[{}]", emit_expr(d))).collect();
            format!("{} {}", emit_type(inner), dims_s.join(" "))
        }
        // F40: queue — `logic[7:0][$]` → `logic [7:0] [$]` (konteks type
        // in-isolation; utk deklarasi signal pakai emit_signal_decl* yang
        // menaruh `[$]` SETELAH nama: `logic [7:0] q[$]`).
        MvType::Queue(inner) => format!("{} [$]", emit_type(inner)),
    }
}

/// Emit ekspresi SV.
pub(crate) fn emit_expr(e: &Expr) -> String {
    match e {
        Expr::Int(v) => v.to_string(),
        Expr::Sized(Some(w), b, d, ..) => format!("{w}'{b}{d}"),
        Expr::Sized(None, b, d, ..) => format!("'{b}{d}"),
        Expr::Real(v) => {
            let s = v.to_string();
            if s.contains('.') {
                s
            } else {
                format!("{s}.0")
            }
        }
        Expr::Fill(c) => format!("'{c}"),
        Expr::Str(s) => format!("\"{}\"", s),
        Expr::Ident(s, ..) => s.clone(),
        Expr::Scoped(p, i, ..) => format!("{p}::{i}"),
        Expr::Cast { ty, expr, .. } => format!("{}'({})", emit_type(ty), emit_expr(expr)),
        Expr::Unary(op, inner) => {
            if op == "posedge" || op == "negedge" {
                format!("{op} {}", emit_expr(inner))
            } else {
                let inner_s = emit_expr(inner);
                format!("{op}{}", maybe_paren(inner, inner_s))
            }
        }
        Expr::IncDec { inc, pre, expr } => {
            let op = if *inc { "++" } else { "--" };
            let es = emit_expr(expr);
            if *pre {
                format!("{op}{}", maybe_paren(expr, es))
            } else {
                format!("{}{op}", maybe_paren(expr, es))
            }
        }
        Expr::Binary(op, l, r) => format!("{} {op} {}", emit_expr(l), emit_expr(r)),
        Expr::Ternary(c, t, f) => format!("{} ? {} : {}", emit_expr(c), emit_expr(t), emit_expr(f)),
        Expr::Call(name, args, ..) => {
            let a: Vec<String> = args.iter().map(emit_expr).collect();
            format!("{name}({})", a.join(", "))
        }
        // Named arg `f(10, factor = 4)` → SV `.factor(4)`.
        Expr::NamedArg { name, expr } => format!(".{name}({})", emit_expr(expr)),
        Expr::MethodCall {
            obj, method, args, ..
        } => {
            let a: Vec<String> = args.iter().map(emit_expr).collect();
            format!("{}.{method}({})", emit_expr(obj), a.join(", "))
        }
        Expr::Member(obj, f, ..) => format!("{}.{f}", emit_expr(obj)),
        Expr::Index(obj, i) => format!("{}[{}]", emit_expr(obj), emit_expr(i)),
        Expr::Range(obj, a, b) => format!("{}[{}:{}]", emit_expr(obj), emit_expr(a), emit_expr(b)),
        Expr::PartSelect {
            base,
            from,
            width,
            plus,
        } => {
            // Indexed part-select naik/turun (LRM 1800 §11.8.2). `width` emit
            // apa adanya — SV memang mengartikannya sebagai LEBAR.
            let op = if *plus { "+:" } else { "-:" };
            format!("{}[{} {op} {}]", emit_expr(base), emit_expr(from), emit_expr(width))
        }
        Expr::Concat(parts) => {
            let p: Vec<String> = parts.iter().map(emit_expr).collect();
            format!("{{{}}}", p.join(", "))
        }
        // Array literal `'{e0, e1, ...}` — assignment pattern unpacked array.
        Expr::ArrayLit(items) => {
            let p: Vec<String> = items.iter().map(emit_expr).collect();
            format!("'{{{}}}", p.join(", "))
        }
        Expr::Replicate(n, inner) => format!("{{{}{{{}}}}}", emit_expr(n), emit_expr(inner)),
        Expr::Paren(inner) => format!("({})", emit_expr(inner)),
        // F12: urutan item inside dijaga 1:1 (`{[1:10], 20}` ≠ `{20, [1:10]}`)
        Expr::Inside { expr, items } => {
            let parts: Vec<String> = items.iter().map(emit_inside_item).collect();
            format!("{} inside {{{}}}", emit_expr(expr), parts.join(", "))
        }
        Expr::Dist { expr, items } => {
            let parts: Vec<String> = items.iter().map(emit_dist_item).collect();
            format!("{} dist {{{}}}", emit_expr(expr), parts.join(", "))
        }
    }
}

/// Emit satu item inside: nilai tunggal atau `[lo:hi]` (F12).
fn emit_inside_item(it: &InsideItem) -> String {
    match it {
        InsideItem::Value(e) => emit_expr(e),
        InsideItem::Range(lo, hi) => format!("[{}:{}]", emit_expr(lo), emit_expr(hi)),
    }
}

/// Emit satu item dist: `[lo:hi] := w` / `v := w` / `v :/ w` (F12).
fn emit_dist_item(it: &DistItem) -> String {
    let op = if it.exact { ":=" } else { ":/" };
    match &it.range {
        Some((lo, hi)) => format!(
            "[{}:{}] {op} {}",
            emit_expr(lo),
            emit_expr(hi),
            emit_expr(&it.weight)
        ),
        None => format!("{} {op} {}", emit_expr(&it.value), emit_expr(&it.weight)),
    }
}

fn maybe_paren(e: &Expr, s: String) -> String {
    if matches!(e, Expr::Binary(..) | Expr::Ternary(..) | Expr::Concat(..)) {
        format!("({s})")
    } else {
        s
    }
}
