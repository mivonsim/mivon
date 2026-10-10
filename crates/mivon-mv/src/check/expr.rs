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
    "wait",
    "disable",
    "force",
    "release",
    "return",
    "break",
    "continue",
];

/// E2012: perbandingan relasional yang hasilnya pasti SALAH karena satu
/// operand bertanda UNSIGNED.
///
/// LRM 1800 §11.8.2: begitu satu operand unsigned, seluruh operasi
/// dievaluasi sebagai unsigned. Akibatnya nilai negatif "melewati" batas
/// bawah dan perbandingan tertentu selalu salah:
///
/// - `u < 0` / `u <= -1` — selalu `false` karena `u` tak pernah negatif
///   (buglog-mv #5: `s = -1; s < 0` → false saat `s` tak bertanda);
/// - `u < 256` untuk `u : logic[7:0]` — selalu `false` (nilai maks 255).
///
/// Hanya dilaporkan kalau kita YAKIN: satu operand unsigned, dan lawannya
/// konstanta yang bisa di-fold. Kondisi tak diketahui didiamkan supaya
/// checker tidak menghasilkan false positive.
fn check_signed_compare(
    op: &str,
    l: &Expr,
    r: &Expr,
    ctx: &Ctx,
    scope: &Scope,
    depth: usize,
) -> Result<(), MvError> {
    // Hanya operator relasional. `==`/`!=` TIDAK salah — unsigned 255 == -1
    // tetap true, jadi pola umum `u == 0xFF` untuk checking byte != 0
    // harus tetap lolos.
    if !matches!(op, "<" | "<=" | ">" | ">=") {
        return Ok(());
    }
    // Samakan arah: `(x REL C)` dengan `x` = operand unsigned.
    let (sign_side, const_side, flipped) = match (
        expr_signed(l, ctx, scope, depth + 1),
        expr_signed(r, ctx, scope, depth + 1),
    ) {
        (Some(false), Some(true)) => (l, r, false),
        (Some(true), Some(false)) => (r, l, true),
        (Some(false), Some(false)) => (l, r, false),
        _ => return Ok(()),
    };
    let Some(c) = fold_const(const_side, &scope.params, 0) else {
        return Ok(());
    };
    // Normalisasi arah: `C > x`  ==  `x < C` (dengan && dibalik).
    let (rel, v) = if flipped {
        (flip_rel(op), c)
    } else {
        (op, c)
    };
    // Batas bawah: unsigned >= 0, jadi apa pun yang "melebihi 0" ke bawah
    // tidak akan pernah benar.
    let below = match rel {
        "<" => v <= 0,
        "<=" => v < 0,
        _ => false,
    };
    // Batas atas: unsigned 0..2^W-1, jadi apa pun yang "melebihi" ke atas
    // tidak akan pernah benar.
    let max = expr_width(sign_side, ctx, scope, depth + 1)
        .filter(|w| *w > 0 && *w < 64)
        .map(|w| (1i64 << w) - 1);
    let above = match (rel, max) {
        (">", Some(m)) => v >= m,
        (">=", Some(m)) => v > m,
        ("<", Some(m)) => v > m,
        ("<=", Some(m)) => v > m,
        _ => false,
    };
    if !below && !above {
        return Ok(());
    }
    let (lpos, cpos) = expr_pos_of(sign_side);
    let why = match (below, above) {
        (true, _) => format!("nilai unsigned selalu >= 0, jadi `x {rel} {v}` tidak pernah benar"),
        (_, true) => format!(
            "nilai unsigned maksimum {} (lebar {}), jadi `x {rel} {v}` tidak pernah benar",
            max.unwrap_or(0),
            expr_width(sign_side, ctx, scope, depth + 1).unwrap_or(0),
        ),
        _ => unreachable!(),
    };
    Err(err_at(
        lpos,
        cpos,
        "E2012",
        format!(
            "operand `{sign_side:?}` bertanda UNSIGNED — {why} \
             (LRM 1800 §11.8.2: satu operand unsigned membuat seluruh \
             perbandingan unsigned). declare `signed logic[..]` atau pakai `int`"
        ),
    ))
}

/// `a > b` ≡ `b < a`, `a >= b` ≡ `b <= a` (untuk E2012).
fn flip_rel(op: &str) -> &str {
    match op {
        ">" => "<",
        ">=" => "<=",
        "<" => ">",
        "<=" => ">=",
        other => other,
    }
}

/// Posisi best-effort ekspresi untuk pesan E2012.
fn expr_pos_of(e: &Expr) -> (usize, usize) {
    match e {
        Expr::Ident(_, l, c) | Expr::Member(_, _, l, c) | Expr::Sized(_, _, _, l, c) => (*l, *c),
        _ => (0, 0),
    }
}

/// Pesan E2011: kalimat yang menjelaskan rentang argumen yang sah.
fn arity_msg(required: usize, total: usize) -> String {
    if required == total {
        format!("taksonya {total} argumen")
    } else {
        format!("taksonya {required}..{total} argumen")
    }
}

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
            if matches!(
                ty.as_ref(),
                MvType::Logic(Some(_)) | MvType::Array(_, _) | MvType::Queue(_)
            ) {
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
        Expr::Binary(op, l, r) => {
            check_expr(l, ctx, scope, depth + 1)?;
            check_expr(r, ctx, scope, depth + 1)?;
            check_signed_compare(op, l, r, ctx, scope, depth)?;
        }
        Expr::Ternary(c, t, f) => {
            check_expr(c, ctx, scope, depth + 1)?;
            check_expr(t, ctx, scope, depth + 1)?;
            check_expr(f, ctx, scope, depth + 1)?;
        }
        Expr::Call(name, args, l, c) => {
            for a in args {
                check_expr(a, ctx, scope, depth + 1)?;
            }
            // E2011: jumlah argumen harus cocok dengan tanda tangan function.
            let n = args.len();
            if let Some(Some((req, total))) = scope.func_arity.get(name.as_str()) {
                if n < *req || n > *total {
                    return Err(err_at(
                        *l,
                        *c,
                        "E2011",
                        format!(
                            "function '{name}' menerima {n} argumen — {}",
                            arity_msg(*req, *total)
                        ),
                    ));
                }
            }
        }
        // Named arg `f(x = 4)` — validasi ekspresi dalam; nama tidak divalidasi
        // (bisa param function eksternal/UVM).
        Expr::NamedArg { expr, .. } => check_expr(expr, ctx, scope, depth + 1)?,
        Expr::MethodCall {
            obj,
            method,
            args,
            line,
            col,
        } => {
            check_expr(obj, ctx, scope, depth + 1)?;
            for a in args {
                check_expr(a, ctx, scope, depth + 1)?;
            }
            // E2011 juga untuk method class (yang dicari = nama method).
            let n = args.len();
            if let Some(Some((req, total))) = ctx.method_arity.get(method.as_str()) {
                if n < *req || n > *total {
                    return Err(err_at(
                        *line,
                        *col,
                        "E2011",
                        format!(
                            "method '{method}' menerima {n} argumen — {}",
                            arity_msg(*req, *total)
                        ),
                    ));
                }
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
        Expr::PartSelect {
            base,
            from,
            width,
            ..
        } => {
            check_expr(base, ctx, scope, depth + 1)?;
            check_expr(from, ctx, scope, depth + 1)?;
            check_expr(width, ctx, scope, depth + 1)?;
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

/// Kelas UVM bawaan yang dikenal engine Mivon (lihat elaborator
/// `build_pkg_param_ctx`: `extends uvm_*` di-remap ke `__uvm_*` builtin).
/// Nama-nama ini SAH sebagai tipe di `.mv` (field/var/sig/port) tanpa
/// definisi lokal — tanpa ini setiap testbench UVM butuh `--no-check`.
const UVM_KNOWN_TYPES: &[&str] = &[
    "uvm_object",
    "uvm_transaction",
    "uvm_component",
    "uvm_report_object",
    "uvm_report_server",
    "uvm_default_report_server",
    "uvm_sequence_item",
    "uvm_sequence",
    "uvm_sequencer",
    "uvm_seq_item_port",
    "uvm_driver",
    "uvm_monitor",
    "uvm_scoreboard",
    "uvm_env",
    "uvm_agent",
    "uvm_subscriber",
    "uvm_analysis_port",
    "uvm_analysis_imp",
    "uvm_analysis_export",
    "uvm_comparator",
    "uvm_in_order_comparator",
    "uvm_heartbeat",
    "uvm_tlm_fifo",
    "uvm_test",
    "uvm_config_db",
    "uvm_event",
    "uvm_barrier",
    "uvm_reg",
    "uvm_reg_field",
    "uvm_reg_map",
    "uvm_reg_block",
];

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
            // F72: kelas UVM bawaan sah tanpa definisi lokal (engine
            // mengeksekusinya sebagai builtin `__uvm_*`).
            if UVM_KNOWN_TYPES.contains(&n.as_str()) {
                return Ok(());
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
        // F40: queue — validasi tipe dalamnya.
        MvType::Queue(inner) => check_type_scope(inner, ctx, scope, depth + 1)?,
        _ => {}
    }
    Ok(())
}

/// Signedness suatu tipe: `Some(true)` = signed, `Some(false)` = unsigned,
/// `None` = tidak berlaku / tidak diketahui (real, time, string, tipe
/// eksternal).
///
/// LRM 1800 §6.2.1 + §11.8.2: `logic`/`bit` **unsigned** kecuali ditandai
/// `signed`; `int`/`byte`/`shortint`/`longint` signed; `uint`/`ulongint`
/// unsigned; `enum` unsigned (kecuali base signed); `real`/`string`/`time`
/// tidak punya signedness aritmetika.
pub(crate) fn type_signed(ty: &MvType, ctx: &Ctx, scope: &Scope, depth: usize) -> Option<bool> {
    if depth > 8 {
        return None;
    }
    match ty {
        MvType::Bit | MvType::Logic(_) => Some(false),
        MvType::Int | MvType::LongInt | MvType::ShortInt | MvType::Byte => Some(true),
        MvType::Uint | MvType::ULongInt => Some(false),
        // real/time/string: signedness tidak relevan untuk perbandingan.
        MvType::Real | MvType::Time | MvType::Str => None,
        MvType::Signed(_) => Some(true),
        MvType::Array(inner, _) | MvType::Queue(inner) => type_signed(inner, ctx, scope, depth + 1),
        MvType::Named(n, ..) => {
            // type parameter module → dari default-nya (kalau ada)
            if let Some(Some(td)) = scope.type_params.get(n.as_str()).copied() {
                return type_signed(td, ctx, scope, depth + 1);
            }
            if let Some(td) = scope.local_types.get(n.as_str()).copied() {
                return typedef_signed(td, ctx, scope, depth + 1);
            }
            let key = n.rsplit("::").next().unwrap_or(n.as_str());
            ctx.types.get(key).copied().and_then(|td| {
                typedef_signed(td, ctx, scope, depth + 1)
            })
        }
    }
}

/// Signedness sebuah typedef.
fn typedef_signed(
    td: &Typedef,
    ctx: &Ctx,
    scope: &Scope,
    depth: usize,
) -> Option<bool> {
    match td {
        Typedef::Alias { ty, .. } => type_signed(ty, ctx, scope, depth + 1),
        // Enum unsigned kecuali ada base signed (LRM 1800 §6.9).
        Typedef::Enum { .. } => Some(false),
        // Struct/union: signedness follow field pertama yang relevan
        // (LRM 1800 §7.2). Tanpa dasar eksplisit, semua field tak
        // bertanda → unsigned.
        Typedef::Struct { fields, .. } | Typedef::Union { fields, .. } => fields
            .first()
            .and_then(|f| type_signed(&f.ty, ctx, scope, depth + 1)),
    }
}

/// Signedness best-effort dari sebuah ekspresi.
pub(crate) fn expr_signed(e: &Expr, ctx: &Ctx, scope: &Scope, depth: usize) -> Option<bool> {
    if depth > 16 {
        return None;
    }
    match e {
        Expr::Int(v) => Some(*v >= 0),
        Expr::Fill(_) => Some(false),
        Expr::Ident(n, ..) => scope
            .types
            .get(n.as_str())
            .and_then(|t| type_signed(t, ctx, scope, depth + 1)),
        Expr::Paren(i) => expr_signed(i, ctx, scope, depth + 1),
        Expr::Binary(_, l, r) => {
            // LRM 1800 §11.8.2: hasil operasi bertanda HANYA bila kedua
            // operand bertanda; begitu ada satu operand unsigned, seluruh
            // ekspresi jadi unsigned.
            let ls = expr_signed(l, ctx, scope, depth + 1);
            let rs = expr_signed(r, ctx, scope, depth + 1);
            match (ls, rs) {
                (Some(true), Some(true)) => Some(true),
                (Some(false), _) | (_, Some(false)) => Some(false),
                _ => None,
            }
        }
        Expr::Cast { ty, .. } => type_signed(ty, ctx, scope, depth + 1),
        // Unary minus TETAP menghasilkan nilai bertanda (Literal minus
        // adalah operand bertanda di LRM 1800 §11.8.1).
        Expr::Unary(o, inner) if o == "-" => {
            expr_signed(inner, ctx, scope, depth + 1).or(Some(true))
        }
        _ => None,
    }
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
                Typedef::Enum { width, members, .. } => {
                    // Lebar TOTAL bit. `enum_width` = `enum_bits + 1` karena
                    // emitter menulis `logic [enum_bits:0]`. Sebelumnya
                    // `enum_bits` dipakai langsung sebagai lebar total →
                    // sinyal enum dianggap 1 bit lebih sempit dari kenyataan,
                    // sehingga E2002 (truncation) tak pernah menyalakan
                    // padahal lebih longgar dari SV.
                    match width {
                        Some(w) => match fold_const(w, &scope.params, 0) {
                            Some(n) if n > 0 => Some(n),
                            _ => Some(crate::enum_width(members.len())),
                        },
                        None => Some(crate::enum_width(members.len())),
                    }
                }
            }
        }
        MvType::Array(inner, _) => type_width(inner, ctx, scope, depth + 1),
        // F40: lebar elemen queue = lebar tipe dalamnya.
        MvType::Queue(inner) => type_width(inner, ctx, scope, depth + 1),
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
        Expr::PartSelect { width, .. } => {
            // Part-select punya lebar tepat sebesar argumen lebarnya
            // (LRM 1800 §11.8.2) — bukan `from..from+width` seperti Range.
            let w = fold_const(width, &scope.params, 0)?;
            if w > 0 {
                Some(w)
            } else {
                None
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
        // F80: literal sized (`8'd10`, `'b101`) — nilai digit sesuai basis.
        Expr::Sized(_, base, digits, ..) => sized_value(*base, digits),
        // F80: fill `'0` → 0, `'1` → -1 (semua bit 1); `'x`/`'z` unknown.
        Expr::Fill(c) => match c {
            '0' => Some(0),
            '1' => Some(-1),
            _ => None,
        },
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
                // F80: perbandingan & logika → 0/1 (error literal E2012 di
                // check/stmt.rs mengandalkan fold ini untuk `u < 0`).
                "==" => Some((a == b) as i64),
                "!=" => Some((a != b) as i64),
                "<" => Some((a < b) as i64),
                "<=" => Some((a <= b) as i64),
                ">" => Some((a > b) as i64),
                ">=" => Some((a >= b) as i64),
                "&&" => Some(((a != 0) && (b != 0)) as i64),
                "||" => Some(((a != 0) || (b != 0)) as i64),
                _ => None,
            }
        }
        Expr::Ternary(c, t, f) => {
            // F80: `c ? t : f` — cabang sesuai kondisi ter-fold.
            let v = fold_const(c, params, depth + 1)?;
            fold_const(if v != 0 { t } else { f }, params, depth + 1)
        }
        Expr::Ident(s, ..) => params.get(s.as_str()).copied(),
        _ => None,
    }
}

/// F80: nilai member enum (`RED` dalam `enum { IDLE, RED }`) — auto-increment
/// dari nilai eksplisit (sinkron dengan emisi codegen/defs.rs). Cari di
/// typedef lokal module, lalu global (file + package).
pub(crate) fn enum_member_value(name: &str, ctx: &Ctx, scope: &Scope) -> Option<i64> {
    let mut found: Option<&Typedef> = None;
    for td in scope.local_types.values() {
        if let Typedef::Enum { members, .. } = td {
            if members.iter().any(|m| m.name == name) {
                found = Some(td);
                break;
            }
        }
    }
    if found.is_none() {
        for td in ctx.types.values() {
            if let Typedef::Enum { members, .. } = td {
                if members.iter().any(|m| m.name == name) {
                    found = Some(td);
                    break;
                }
            }
        }
    }
    let td = found?;
    // Nilai eksplisit di-fold (bisa merujuk konstanta lain); depth+1.
    let mut next: i64 = 0;
    if let Typedef::Enum { members, .. } = td {
        for m in members {
            let v = match &m.value {
                Some(e) => fold_const(e, &scope.params, 1)?,
                None => next,
            };
            if m.name == name {
                return Some(v);
            }
            next = v.wrapping_add(1);
        }
    }
    None
}

/// F80: fold konstanta untuk KONDISI GENERATE — seperti `fold_const`
/// ditambah resolusi atom penuh: parameter/konstanta module, member enum
/// (nilai auto-increment), dan konstanta package (`pkg::ITEM`, rekursif
/// depth-limited). `Ident` lain (sinyal/port) dan call/sistem → None.
pub(crate) fn gen_const_value(e: &Expr, scope: &Scope, ctx: &Ctx, depth: usize) -> Option<i64> {
    if depth > 8 {
        return None;
    }
    match e {
        Expr::Int(v) => Some(*v),
        Expr::Paren(i) => gen_const_value(i, scope, ctx, depth + 1),
        Expr::Sized(_, base, digits, ..) => sized_value(*base, digits),
        Expr::Fill(c) => match c {
            '0' => Some(0),
            '1' => Some(-1),
            _ => None,
        },
        Expr::Unary(op, i) => {
            let v = gen_const_value(i, scope, ctx, depth + 1)?;
            match op.as_str() {
                "-" => Some(-v),
                "+" => Some(v),
                "~" => Some(!v),
                "!" => Some((v == 0) as i64),
                _ => None,
            }
        }
        Expr::Binary(op, l, r) => {
            let a = gen_const_value(l, scope, ctx, depth + 1)?;
            let b = gen_const_value(r, scope, ctx, depth + 1)?;
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
                "==" => Some((a == b) as i64),
                "!=" => Some((a != b) as i64),
                "<" => Some((a < b) as i64),
                "<=" => Some((a <= b) as i64),
                ">" => Some((a > b) as i64),
                ">=" => Some((a >= b) as i64),
                "&&" => Some(((a != 0) && (b != 0)) as i64),
                "||" => Some(((a != 0) || (b != 0)) as i64),
                _ => None,
            }
        }
        Expr::Ternary(c, t, f) => {
            let v = gen_const_value(c, scope, ctx, depth + 1)?;
            gen_const_value(if v != 0 { t } else { f }, scope, ctx, depth + 1)
        }
        Expr::Ident(s, ..) => {
            if let Some(v) = scope.params.get(s.as_str()).copied() {
                return Some(v);
            }
            enum_member_value(s, ctx, scope)
        }
        Expr::Scoped(p, i, ..) => {
            // Konstanta package — nilai ekspresinya di-fold rekursif.
            let pkg = ctx.packages.get(p.as_str())?;
            let (_, _, value) = pkg.consts.iter().find(|(n, _, _)| n == i)?;
            gen_const_value(value, scope, ctx, depth + 1)
        }
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
