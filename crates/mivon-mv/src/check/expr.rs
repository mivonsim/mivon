//! Check — ekspresi: validasi (E2001/05/06), tipe & lebar bit, const-fold.
//! 1 file = 1 tanggung jawab.

use super::{err_at, resolve_typedef, Ctx, Params, Scope};
use crate::ast::*;
use crate::MvError;

/// Keyword SV yang TIDAK MUNGKIN jadi nama sistem task/function setelah `$`.
///
/// Blacklist tertutup (bukan whitelist) — sengaja: sistem task custom VPI
/// (`$my_vpi_task`) tak terdaftar di mana pun dan tak boleh ditolak.
/// Kasus: mutasi fuzz `$begin enddisplay(...)` lolos check → codegen
/// menghasilkan `$begin;` → E1002 di parser SV, jauh dari titik salahnya.
pub(crate) const MV_NON_SYSTASK: &[&str] = &[
    "begin",
    "end",
    "module",
    "endmodule",
    "interface",
    "endinterface",
    "class",
    "endclass",
    "function",
    "endfunction",
    "task",
    "endtask",
    "package",
    "endpackage",
    "if",
    "else",
    "case",
    "casex",
    "casez",
    "endcase",
    "default",
    "for",
    "foreach",
    "while",
    "do",
    "forever",
    "repeat",
    "generate",
    "endgenerate",
    "always",
    "always_ff",
    "always_comb",
    "always_latch",
    "initial",
    "final",
    "assign",
    "wire",
    "reg",
    "logic",
    "input",
    "output",
    "inout",
    "typedef",
    "struct",
    "union",
    "enum",
    "import",
    "export",
    "posedge",
    "negedge",
    "fork",
    "join",
    "join_any",
    "join_none",
    "return",
    "break",
    "continue",
];

pub(crate) fn check_expr<'a>(
    e: &'a Expr,
    ctx: &'a Ctx<'a>,
    scope: &Scope<'a>,
    depth: usize,
) -> Result<(), MvError> {
    if depth > 24 {
        return Ok(()); // guard rekursi dalam
    }
    match e {
        Expr::Int(_) | Expr::Real(_) | Expr::Fill(_) | Expr::Str(_) => {}
        Expr::Sized(Some(w), base, digits, l, c) => {
            // E2006: literal melebihi lebar
            if let Some(v) = sized_value(*base, digits) {
                if v >= (1i64 << (*w).min(62)) {
                    return Err(err_at(
                        *l,
                        *c,
                        "E2006",
                        format!(
                            "literal {w}'{base}{digits} melebihi {w} bit — di '{}'",
                            scope.env.mname
                        ),
                    ));
                }
            }
        }
        Expr::Sized(None, ..) => {}
        Expr::Ident(s, l, c) => {
            // `$finish`/`$display`/`$past`/`$clog2` — system task/function.
            if s.starts_with('$') {
                // BLACKLIST keyword SV setelah `$`: tak mungkin sistem task
                // (tak ada `$begin`/`$end` di IEEE 1800) — menutup kasus
                // mutasi sampah merembes jadi output SV `$begin;` (E1002 di
                // parser SV, jauh dari sumbernya). Sengaja BLACKLIST, bukan
                // whitelist: repo mendukung VPI custom task (`$my_vpi_task`,
                // `$unregistered_task`) yang tak terdaftar di mana pun —
                // whitelist statis akan merusaknya.
                let name = s.strip_prefix('$').unwrap_or(s.as_str());
                if MV_NON_SYSTASK.contains(&name) {
                    return Err(err_at(
                        *l,
                        *c,
                        "E2001",
                        format!(
                            "'{s}' bukan sistem task/function — keyword SV setelah `$` \
                             (mungkin sisa mutasi/typo); di '{}'",
                            scope.env.mname
                        ),
                    ));
                }
                return Ok(());
            }
            // `this`/`super` — kata kunci konteks di method class (F7)
            if s == "this" || s == "super" {
                return Ok(());
            }
            if !scope.known(s) {
                return Err(err_at(
                    *l,
                    *c,
                    "E2001",
                    format!("undefined signal '{s}' — di '{}'", scope.env.mname),
                ));
            }
        }
        Expr::Scoped(p, i, l, c) => {
            if let Some(pkg) = ctx.packages.get(p.as_str()) {
                let in_pkg = pkg.typedefs.iter().any(|td| td_name(td) == i)
                    || pkg.consts.iter().any(|(n, _, _)| n == i)
                    || pkg
                        .typedefs
                        .iter()
                        .any(|td| matches!(td, Typedef::Enum { members, .. } if members.iter().any(|mm| mm.name == *i)));
                if !in_pkg {
                    return Err(err_at(
                        *l,
                        *c,
                        "E2001",
                        format!(
                            "item '{i}' tidak ada di package '{p}' — di '{}'",
                            scope.env.mname
                        ),
                    ));
                }
            }
        }
        // F33: type cast `T'(x)` — tipe target harus dikenal (E2005).
        Expr::Cast {
            ty,
            expr,
            line,
            col,
        } => {
            // F33 fix review: cast target tidak boleh punya range/array.
            if matches!(ty.as_ref(), MvType::Logic(Some(_)) | MvType::Array(_, _)) {
                return Err(err_at(
                    *line,
                    *col,
                    "E2005",
                    "cast target tidak boleh punya range/array — pakai tipe sederhana (mis. `Word16'(x)`)",
                ));
            }
            // Size cast via parameter (`WIDTH'(x)`) — lebar = nilai param.
            if let MvType::Named(n, ..) = ty.as_ref() {
                if scope.params.contains_key(n.as_str()) {
                    check_expr(expr, ctx, scope, depth + 1)?;
                    return Ok(());
                }
            }
            check_type_scope(ty, ctx, Some(scope), 0)?;
            check_expr(expr, ctx, scope, depth + 1)?;
        }
        Expr::Unary(_, inner) => check_expr(inner, ctx, scope, depth + 1)?,
        Expr::IncDec { expr, .. } => check_expr(expr, ctx, scope, depth + 1)?,
        Expr::Binary(_, l, r) => {
            check_expr(l, ctx, scope, depth + 1)?;
            check_expr(r, ctx, scope, depth + 1)?;
        }
        Expr::Ternary(c, t, f) => {
            check_expr(c, ctx, scope, depth + 1)?;
            check_expr(t, ctx, scope, depth + 1)?;
            check_expr(f, ctx, scope, depth + 1)?;
        }
        Expr::Call(_, args) => {
            for a in args {
                check_expr(a, ctx, scope, depth + 1)?;
            }
        }
        // Named arg `f(x = 4)` — validasi ekspresi dalam; nama tidak divalidasi
        // (bisa param function eksternal/UVM).
        Expr::NamedArg { expr, .. } => check_expr(expr, ctx, scope, depth + 1)?,
        Expr::MethodCall {
            obj,
            method: _,
            args,
        } => {
            check_expr(obj, ctx, scope, depth + 1)?;
            for a in args {
                check_expr(a, ctx, scope, depth + 1)?;
            }
        }
        Expr::Member(obj, f, l, c) => {
            check_expr(obj, ctx, scope, depth + 1)?;
            // field struct: jika objek adalah Ident dengan tipe struct in-file
            if let Expr::Ident(base, ..) = obj.as_ref() {
                if let Some(ty) = scope.types.get(base.as_str()) {
                    if let Some(fields) = resolve_fields(ty, ctx, 0) {
                        if !fields.iter().any(|fl| fl.names.iter().any(|n| n == f)) {
                            return Err(err_at(
                                *l,
                                *c,
                                "E2001",
                                format!(
                                    "field '{f}' tidak ada di struct — di '{}'",
                                    scope.env.mname
                                ),
                            ));
                        }
                    }
                }
            }
        }
        Expr::Index(obj, i) => {
            check_expr(obj, ctx, scope, depth + 1)?;
            check_expr(i, ctx, scope, depth + 1)?;
        }
        Expr::Range(obj, a, b) => {
            check_expr(obj, ctx, scope, depth + 1)?;
            check_expr(a, ctx, scope, depth + 1)?;
            check_expr(b, ctx, scope, depth + 1)?;
        }
        Expr::Concat(parts) => {
            for p in parts {
                check_expr(p, ctx, scope, depth + 1)?;
            }
        }
        // Array literal `'{...}` — validasi tiap elemen (lebar/tipe).
        Expr::ArrayLit(items) => {
            for it in items {
                check_expr(it, ctx, scope, depth + 1)?;
            }
        }
        Expr::Replicate(n, inner) => {
            check_expr(n, ctx, scope, depth + 1)?;
            check_expr(inner, ctx, scope, depth + 1)?;
        }
        Expr::Paren(i) => check_expr(i, ctx, scope, depth + 1)?,
        // F12: `x inside {a, b, [lo:hi]}`
        Expr::Inside { expr, items } => {
            check_expr(expr, ctx, scope, depth + 1)?;
            for it in items {
                match it {
                    InsideItem::Value(v) => check_expr(v, ctx, scope, depth + 1)?,
                    InsideItem::Range(lo, hi) => {
                        check_expr(lo, ctx, scope, depth + 1)?;
                        check_expr(hi, ctx, scope, depth + 1)?;
                    }
                }
            }
        }
        // F12: `x dist { v := w, [lo:hi] :/ w }`
        Expr::Dist { expr, items } => {
            check_expr(expr, ctx, scope, depth + 1)?;
            for it in items {
                check_expr(&it.value, ctx, scope, depth + 1)?;
                if let Some((lo, hi)) = &it.range {
                    check_expr(lo, ctx, scope, depth + 1)?;
                    check_expr(hi, ctx, scope, depth + 1)?;
                }
                check_expr(&it.weight, ctx, scope, depth + 1)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn td_name(td: &Typedef) -> &str {
    super::td_name(td)
}

fn sized_value(base: char, digits: &str) -> Option<i64> {
    let radix = match base {
        'd' => 10,
        'h' => 16,
        'o' => 8,
        'b' => 2,
        _ => return None,
    };
    i64::from_str_radix(digits, radix).ok()
}

// ── Tipe: validasi (E2005) + lebar bit ──

/// Validasi tipe: `Named` harus ter-resolve (E2005). Rekursif ke dalam.
pub(crate) fn check_type(ty: &MvType, ctx: &Ctx, depth: usize) -> Result<(), MvError> {
    check_type_scope(ty, ctx, None, depth)
}

/// Validasi tipe dgn scope module (F32): `Named` yang merupakan type
/// parameter module (`T`) sah — tidak perlu ada di ctx.types.
pub(crate) fn check_type_scope(
    ty: &MvType,
    ctx: &Ctx,
    scope: Option<&Scope>,
    depth: usize,
) -> Result<(), MvError> {
    if depth > 8 {
        return Ok(());
    }
    match ty {
        MvType::Named(n, l, c) => {
            // F32: type param module sah sbg tipe di dalam module
            if let Some(sc) = scope {
                if sc.type_params.contains_key(n.as_str()) {
                    return Ok(());
                }
                // typedef lokal module (`type X = ...` di badan module)
                if sc.local_types.contains_key(n.as_str()) {
                    return Ok(());
                }
            }
            if let Some((pkg, item)) = n.split_once("::") {
                if let Some(p) = ctx.packages.get(pkg) {
                    if !p.typedefs.iter().any(|td| td_name(td) == item) {
                        return Err(err_at(
                            *l,
                            *c,
                            "E2005",
                            format!(
                                "tipe tak dikenal '{n}' (package '{pkg}' tidak punya '{item}')"
                            ),
                        ));
                    }
                }
                // package eksternal (file lain) → dilewati
            } else if !ctx.types.contains_key(n.as_str())
                && !ctx.classes.contains(n.as_str())
                && !ctx.interfaces.contains(n.as_str())
            {
                return Err(err_at(*l, *c, "E2005", format!("tipe tak dikenal '{n}'")));
            }
        }
        MvType::Signed(inner) => check_type_scope(inner, ctx, scope, depth + 1)?,
        MvType::Array(inner, _) => check_type_scope(inner, ctx, scope, depth + 1)?,
        _ => {}
    }
    Ok(())
}

/// Lebar bit tipe; None bila tidak diketahui (eksternal / non-konstanta).
pub(crate) fn type_width(ty: &MvType, ctx: &Ctx, scope: &Scope, depth: usize) -> Option<i64> {
    if depth > 8 {
        return None;
    }
    match ty {
        MvType::Bit => Some(1),
        MvType::Logic(r) => match r {
            None => Some(1),
            Some((a, b)) => {
                let x = fold_const(a, &scope.params, 0);
                let y = fold_const(b, &scope.params, 0);
                match (x, y) {
                    (Some(ai), Some(bi)) => Some((ai - bi).abs() + 1),
                    _ => None,
                }
            }
        },
        MvType::Signed(inner) => type_width(inner, ctx, scope, depth + 1),
        MvType::Int => Some(32),
        MvType::Uint => Some(32),
        MvType::LongInt => Some(64),
        MvType::ULongInt => Some(64),
        MvType::ShortInt => Some(16),
        MvType::Byte => Some(8),
        MvType::Time => Some(64),
        MvType::Real | MvType::Str => None,
        MvType::Named(n, ..) => {
            // F32: type param module — lebar mengikuti default tipe-nya.
            if let Some(Some(td)) = scope.type_params.get(n.as_str()) {
                return type_width(td, ctx, scope, depth + 1);
            }
            let td = resolve_typedef(n, ctx)?;
            match td {
                Typedef::Alias { ty, .. } => type_width(ty, ctx, scope, depth + 1),
                Typedef::Struct {
                    packed: true,
                    fields,
                    ..
                } => {
                    let mut total = 0i64;
                    for f in fields {
                        total += type_width(&f.ty, ctx, scope, depth + 1)?;
                    }
                    Some(total)
                }
                Typedef::Struct { packed: false, .. } => None,
                Typedef::Union {
                    packed: true,
                    fields,
                    ..
                } => {
                    let mut max_w = 0i64;
                    for f in fields {
                        let w = type_width(&f.ty, ctx, scope, depth + 1)?;
                        if w > max_w {
                            max_w = w;
                        }
                    }
                    Some(max_w)
                }
                Typedef::Union { packed: false, .. } => None,
                Typedef::Enum { width, members, .. } => match width {
                    Some(Expr::Int(n)) => Some(*n),
                    _ => Some(crate::enum_bits(members.len())),
                },
            }
        }
        MvType::Array(inner, _) => type_width(inner, ctx, scope, depth + 1),
    }
}

/// Lebar elemen hasil select `x[i]`: vektor → 1 bit; array → lebar elemen.
pub(crate) fn element_width(ty: &MvType, ctx: &Ctx, scope: &Scope, depth: usize) -> Option<i64> {
    if depth > 8 {
        return None;
    }
    match ty {
        MvType::Logic(_) | MvType::Bit => Some(1),
        MvType::Array(inner, _) => type_width(inner, ctx, scope, depth + 1),
        MvType::Named(n, ..) => {
            if let Some(td) = resolve_typedef(n, ctx) {
                match td {
                    Typedef::Alias { ty, .. } => element_width(ty, ctx, scope, depth + 1),
                    _ => Some(1),
                }
            } else {
                None
            }
        }
        _ => type_width(ty, ctx, scope, depth + 1),
    }
}

/// Field struct bila tipe ter-resolve ke struct in-file.
pub(crate) fn resolve_fields<'a>(
    ty: &'a MvType,
    ctx: &'a Ctx<'a>,
    depth: usize,
) -> Option<Vec<&'a Field>> {
    if depth > 8 {
        return None;
    }
    match ty {
        MvType::Named(n, ..) => {
            let td = resolve_typedef(n, ctx)?;
            match td {
                Typedef::Struct { fields, .. } | Typedef::Union { fields, .. } => {
                    Some(fields.iter().collect())
                }
                Typedef::Alias { ty, .. } => resolve_fields(ty, ctx, depth + 1),
                Typedef::Enum { .. } => None,
            }
        }
        _ => None,
    }
}

/// Lebar bit ekspresi; None bila tidak dapat dihitung secara konstan.
pub(crate) fn expr_width(e: &Expr, ctx: &Ctx, scope: &Scope, depth: usize) -> Option<i64> {
    if depth > 16 {
        return None;
    }
    match e {
        Expr::Int(v) => Some(bit_len(*v)),
        Expr::Sized(Some(w), ..) => Some(*w),
        Expr::Sized(None, ..) => None,
        Expr::Real(_) | Expr::Fill(_) | Expr::Str(_) => None,
        Expr::Ident(s, ..) => {
            if let Some(ty) = scope.types.get(s.as_str()) {
                type_width(ty, ctx, scope, depth + 1)
            } else if let Some(v) = scope.params.get(s.as_str()) {
                Some(bit_len(*v))
            } else {
                scope.enum_members.get(s.as_str()).copied()
            }
        }
        Expr::Scoped(..) => None,
        Expr::Unary(op, inner) => match op.as_str() {
            "&" | "|" | "^" | "~&" | "~|" | "~^" | "!" => Some(1),
            "~" | "-" | "+" => expr_width(inner, ctx, scope, depth + 1),
            _ => None,
        },
        Expr::IncDec { expr, .. } => expr_width(expr, ctx, scope, depth + 1),
        Expr::Binary(op, l, r) => {
            let wl = expr_width(l, ctx, scope, depth + 1);
            let wr = expr_width(r, ctx, scope, depth + 1);
            match op.as_str() {
                "==" | "!=" | "===" | "!==" | "<" | "<=" | ">" | ">=" | "&&" | "||" => Some(1),
                "<<" | ">>" | "<<<" | ">>>" => wl,
                "*" => match (wl, wr) {
                    (Some(a), Some(b)) => Some(a + b),
                    _ => None,
                },
                "**" => None,
                _ => match (wl, wr) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    _ => None,
                },
            }
        }
        Expr::Ternary(_, t, f) => {
            let wt = expr_width(t, ctx, scope, depth + 1);
            let wf = expr_width(f, ctx, scope, depth + 1);
            match (wt, wf) {
                (Some(a), Some(b)) => Some(a.max(b)),
                _ => None,
            }
        }
        Expr::Call(..) | Expr::MethodCall { .. } => None,
        // Named arg — lebar dari ekspresi dalam (dipakai Call args? tidak
        // dihitung E2002 utk call; None aman).
        Expr::NamedArg { expr, .. } => expr_width(expr, ctx, scope, depth + 1),
        // F33: lebar cast = lebar tipe target.
        Expr::Cast { ty, .. } => {
            // Size cast via parameter — lebar = NILAI param.
            if let MvType::Named(n, ..) = ty.as_ref() {
                if let Some(v) = scope.params.get(n.as_str()) {
                    return Some(*v);
                }
            }
            type_width(ty, ctx, scope, depth + 1)
        }
        Expr::Member(obj, f, ..) => {
            if let Expr::Ident(base, ..) = obj.as_ref() {
                if let Some(ty) = scope.types.get(base.as_str()) {
                    if let Some(fields) = resolve_fields(ty, ctx, 0) {
                        for fl in fields {
                            if fl.names.iter().any(|n| n == f) {
                                return type_width(&fl.ty, ctx, scope, depth + 1);
                            }
                        }
                    }
                }
            }
            None
        }
        Expr::Index(obj, _) => {
            if let Expr::Ident(base, ..) = obj.as_ref() {
                if let Some(ty) = scope.types.get(base.as_str()) {
                    return element_width(ty, ctx, scope, depth + 1);
                }
            }
            None
        }
        Expr::Range(_obj, a, b) => {
            let wa = fold_const(a, &scope.params, 0);
            let wb = fold_const(b, &scope.params, 0);
            match (wa, wb) {
                (Some(x), Some(y)) => Some((x - y).abs() + 1),
                _ => None,
            }
        }
        Expr::Concat(parts) => {
            let mut total = 0i64;
            for p in parts {
                total += expr_width(p, ctx, scope, depth + 1)?;
            }
            Some(total)
        }
        Expr::Replicate(n, inner) => {
            let c = fold_const(n, &scope.params, 0)?;
            Some(c * expr_width(inner, ctx, scope, depth + 1)?)
        }
        Expr::Paren(i) => expr_width(i, ctx, scope, depth + 1),
        // F12: inside/dist adalah predikat boolean → 1 bit
        Expr::Inside { .. } | Expr::Dist { .. } => Some(1),
        // Array literal (unpacked) — bukan scalar, tidak punya lebar tunggal.
        Expr::ArrayLit(_) => None,
    }
}

/// Minimal bit untuk nilai integer (negatif → anggap 32-bit).
pub(crate) fn bit_len(v: i64) -> i64 {
    if v == 0 {
        1
    } else if v > 0 {
        64 - (v as u64).leading_zeros() as i64
    } else {
        32
    }
}

/// Const-fold ekspresi integer sederhana. `Ident` hanya dari parameter.
pub(crate) fn fold_const(e: &Expr, params: &Params, depth: usize) -> Option<i64> {
    if depth > 8 {
        return None;
    }
    match e {
        Expr::Int(v) => Some(*v),
        Expr::Paren(i) => fold_const(i, params, depth + 1),
        Expr::Unary(op, i) => {
            let v = fold_const(i, params, depth + 1)?;
            match op.as_str() {
                "-" => Some(-v),
                "+" => Some(v),
                "~" => Some(!v),
                _ => None,
            }
        }
        Expr::Binary(op, l, r) => {
            let a = fold_const(l, params, depth + 1)?;
            let b = fold_const(r, params, depth + 1)?;
            match op.as_str() {
                "+" => Some(a.wrapping_add(b)),
                "-" => Some(a.wrapping_sub(b)),
                "*" => Some(a.wrapping_mul(b)),
                "/" => (b != 0).then(|| a.wrapping_div(b)),
                "%" => (b != 0).then(|| a.wrapping_rem(b)),
                "<<" => Some(a.wrapping_shl(b as u32)),
                ">>" => Some(a.wrapping_shr(b as u32)),
                "&" => Some(a & b),
                "|" => Some(a | b),
                "^" => Some(a ^ b),
                _ => None,
            }
        }
        Expr::Ident(s, ..) => params.get(s.as_str()).copied(),
        _ => None,
    }
}

/// F32: ekspresi yang BERBENTUK tipe (bukan nilai) — ident yang merupakan
/// typedef/enum-member/class/interface, atau `pkg::item` scoped.
pub(crate) fn is_type_like(e: &Expr, ctx: &Ctx) -> bool {
    match e {
        Expr::Ident(s, ..) => {
            ctx.types.contains_key(s.as_str())
                || ctx.enum_members.contains_key(s.as_str())
                || ctx.classes.contains(s.as_str())
                || ctx.interfaces.contains(s.as_str())
        }
        Expr::Scoped(..) => true,
        _ => false,
    }
}
