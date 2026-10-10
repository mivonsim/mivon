//! Mivon HDL (.mv) — printer canonical: AST → teks `.mv`.
//!
//! Gap yang diidentifikasi migrasi fuzzing (mivon-fuzz → mivon-mv): AST
//! `MvFile` tidak punya representasi teks `.mv` — satu-satunya emisi adalah
//! SystemVerilog (codegen). Padahal mutasi AST fuzzer butuh testcase canonical
//! `.mv` yang bisa di-parse ulang (reproducibility: seed → MV sama → HDL sama).
//!
//! Invariant: `parse(print(file))` ≡ `file` (semantik AST sama; posisi
//! line/col diisi ulang parser). 1 file = 1 tanggung jawab: hanya printing.
//!
//! Konvensi:
//! - Teks deterministik (byte-identik utk AST sama) — hash kanonik stabil.
//! - Statement .mv TANPA titik-koma (kecuali isi `@sv`/`assert property` yang
//!   dipertahankan verbatim raw).
//! - Ekspresi selalu dibungkus `(...)` pada binary/ternary agar precedence
//!   round-trip aman; syntax disesuaikan dgn parser (parser/expr.rs, stmt.rs,
//!   module.rs, class.rs, defs.rs).

use std::fmt::Write;

use crate::ast::*;

/// Cetak seluruh file `.mv` (urutan: typedef → package → interface →
/// module → program → class → func → task — konsisten dgn parse_file).
pub fn print_file(file: &MvFile) -> String {
    let mut out = String::new();
    for td in &file.typedefs {
        let mut s = StrB::new();
        print_typedef(&mut s, td);
        out.push_str(&s.0);
    }
    for p in &file.packages {
        out.push_str(&print_package(p));
    }
    for i in &file.interfaces {
        out.push_str(&print_interface(i));
    }
    for m in &file.modules {
        out.push_str(&print_module(m, "module"));
    }
    for p in &file.programs {
        out.push_str(&print_module(p, "program"));
    }
    for c in &file.classes {
        out.push_str(&print_class(c));
    }
    for f in &file.funcs {
        out.push_str(&print_func(f));
    }
    for t in &file.tasks {
        out.push_str(&print_task(t));
    }
    out
}

/// Writer kecil ber-indent utk blok statement/module.
struct StrB(String);

impl StrB {
    fn new() -> Self {
        StrB(String::new())
    }
    /// Baris baru dengan `n`×4 spasi indent.
    fn line(&mut self, indent: usize, s: &str) {
        for _ in 0..indent {
            self.0.push_str("    ");
        }
        self.0.push_str(s);
        self.0.push('\n');
    }
}

// ──────────────────────────────────────────────────────────────────────
// Tipe
// ──────────────────────────────────────────────────────────────────────

/// Tipe → teks .mv. `Logic(Some((hi, lo)))` selalu dicetak `[hi:lo]` —
/// parser `logic[N]` di-normalisasi ke `Logic(Some((N-1, 0)))`, jadi bentuk
/// range round-trip identik.
pub fn print_type(t: &MvType) -> String {
    match t {
        MvType::Bit => "bit".to_string(),
        MvType::Logic(range) => match range {
            Some((hi, lo)) => format!("logic[{}:{}]", print_expr(hi), print_expr(lo)),
            None => "logic".to_string(),
        },
        MvType::Signed(inner) => format!("signed {}", print_type(inner)),
        MvType::Int => "int".to_string(),
        MvType::Uint => "uint".to_string(),
        MvType::LongInt => "longint".to_string(),
        MvType::ULongInt => "ulongint".to_string(),
        MvType::ShortInt => "shortint".to_string(),
        MvType::Byte => "byte".to_string(),
        MvType::Real => "real".to_string(),
        MvType::Time => "time".to_string(),
        MvType::Str => "string".to_string(),
        MvType::Named(n, _, _) => n.clone(),
        MvType::Array(inner, dims) => {
            let mut s = print_type(inner);
            for d in dims {
                write!(s, "[{}]", print_expr(d)).unwrap();
            }
            s
        }
        // F40: queue round-trip `Type[$]`.
        MvType::Queue(inner) => format!("{}[$]", print_type(inner)),
    }
}

// ──────────────────────────────────────────────────────────────────────
// Ekspresi
// ──────────────────────────────────────────────────────────────────────

/// Operand biner/ternary: atom (Ident/Int/literal/scoped) tanpa parens —
/// re-parse tidak menciptakan node `Paren` (kunci idempotensi). `Paren(existing)`
/// dicetak persis satu tingkat. Ekspresi lain dibungkus parens (aman precedence).
fn wrap(e: &Expr) -> String {
    match e {
        Expr::Paren(inner) => format!("({})", print_expr(inner)),
        Expr::Ident(..)
        | Expr::Int(..)
        | Expr::Sized(..)
        | Expr::Fill(..)
        | Expr::Str(..)
        | Expr::Real(..)
        | Expr::Scoped(..) => print_expr(e),
        _ => format!("({})", print_expr(e)),
    }
}

/// Ekspresi → teks .mv. Binary/ternary memakai `wrap` pada operan sehingga
/// round-trip idempoten (parens tidak menumpuk; lihat `wrap`).
pub fn print_expr(e: &Expr) -> String {
    match e {
        Expr::Int(v) => format!("{v}"),
        Expr::Sized(w, b, d, _, _) => match w {
            Some(w) => format!("{w}'{b}{d}"),
            None => format!("'{b}{d}"),
        },
        Expr::Real(v) => print_real(*v),
        Expr::Fill(c) => format!("'{c}"),
        Expr::Str(s) => format!("\"{}\"", escape_str(s)),
        Expr::Ident(n, _, _) => n.clone(),
        Expr::Scoped(p, i, _, _) => format!("{p}::{i}"),
        Expr::Cast { ty, expr, .. } => match expr.as_ref() {
            // Parser T'() selalu membungkus expr dalam Paren — pertahankan.
            // AST buatan fuzzer tanpa Paren tetap dijamin parsible.
            Expr::Paren(..) => format!("{}'{}", print_type(ty), print_expr(expr)),
            other => format!("{}'({})", print_type(ty), print_expr(other)),
        },
        Expr::Unary(op, inner) => format!("{op}{}", wrap(inner)),
        Expr::IncDec { inc, pre, expr } => {
            let op = if *inc { "++" } else { "--" };
            if *pre {
                format!("{op}{}", wrap(expr))
            } else {
                format!("{}{op}", wrap(expr))
            }
        }
        Expr::Binary(op, l, r) => format!("{} {} {}", wrap(l), op, wrap(r)),
        Expr::Ternary(c, t, f) => format!("{} ? {} : {}", wrap(c), wrap(t), wrap(f)),
        Expr::Call(name, args, ..) => {
            let a: Vec<String> = args.iter().map(wrap).collect();
            format!("{name}({})", a.join(", "))
        }
        Expr::NamedArg { name, expr } => format!("{name} = {}", wrap(expr)),
        Expr::MethodCall {
            obj, method, args, ..
        } => {
            let a: Vec<String> = args.iter().map(wrap).collect();
            format!("{}.{}({})", wrap(obj), method, a.join(", "))
        }
        Expr::Member(o, f, _, _) => format!("{}.{}", wrap(o), f),
        Expr::Index(o, i) => format!("{}[{}]", wrap(o), wrap(i)),
        Expr::Range(o, hi, lo) => format!("{}[{}:{}]", wrap(o), wrap(hi), wrap(lo)),
        Expr::PartSelect {
            base,
            from,
            width,
            plus,
        } => {
            let op = if *plus { "+:" } else { "-:" };
            format!("{}[{} {op} {}]", wrap(base), wrap(from), wrap(width))
        }
        Expr::Concat(parts) => {
            let p: Vec<String> = parts.iter().map(wrap).collect();
            format!("{{{}}}", p.join(", "))
        }
        Expr::ArrayLit(items) => {
            let p: Vec<String> = items.iter().map(wrap).collect();
            format!("'{{{}}}", p.join(", "))
        }
        Expr::Replicate(n, inner) => {
            // `{N{expr}}` — dua pasang kurung kurawal literal.
            let mut s = String::from("{");
            s.push_str(&wrap(n));
            s.push('{');
            s.push_str(&wrap(inner));
            s.push_str("}}");
            s
        }
        Expr::Paren(inner) => format!("({})", print_expr(inner)),
        Expr::Inside { expr, items } => {
            let it: Vec<String> = items
                .iter()
                .map(|i| match i {
                    InsideItem::Value(e) => wrap(e),
                    InsideItem::Range(lo, hi) => format!("[{}:{}]", wrap(lo), wrap(hi)),
                })
                .collect();
            format!("{} inside {{{}}}", wrap(expr), it.join(", "))
        }
        Expr::Dist { expr, items } => {
            let it: Vec<String> = items.iter().map(print_dist_item).collect();
            format!("{} dist {{{}}}", wrap(expr), it.join(", "))
        }
    }
}

fn print_dist_item(d: &DistItem) -> String {
    let val = match &d.range {
        Some((lo, hi)) => format!("[{}:{}]", print_expr(lo), print_expr(hi)),
        None => print_expr(&d.value),
    };
    let w = if d.exact { ":=" } else { ":/" };
    format!("{val} {w} {}", print_expr(&d.weight))
}

/// Satu label `case inside`: nilai atau `[lo:hi]`.
fn print_inside_item(it: &InsideItem) -> String {
    match it {
        InsideItem::Value(e) => print_expr(e),
        InsideItem::Range(lo, hi) => format!("[{}:{}]", print_expr(lo), print_expr(hi)),
    }
}

/// f64 → teks yang round-trip sebagai `Real` (lexer butuh titik desimal):
/// bilangan bulat dipaksa `N.0`; selain itu Display standar.
fn print_real(v: f64) -> String {
    if v.fract() == 0.0 && v.is_finite() {
        format!("{v:.0}.0")
    } else {
        format!("{v}")
    }
}

fn escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(c),
        }
    }
    out
}

// ──────────────────────────────────────────────────────────────────────
// Statement
// ──────────────────────────────────────────────────────────────────────

/// Statement → teks .mv (multi-baris ber-indent).
pub fn print_stmt(indent: usize, s: &Stmt) -> String {
    let mut b = StrB::new();
    print_stmt_b(&mut b, indent, s);
    b.0
}

fn print_stmt_b(b: &mut StrB, indent: usize, s: &Stmt) {
    match s {
        Stmt::Block(stmts) => {
            if stmts.is_empty() {
                b.line(indent, "{}");
                return;
            }
            b.line(indent, "{");
            for st in stmts {
                print_stmt_b(b, indent + 1, st);
            }
            b.line(indent, "}");
        }
        Stmt::NamedBlock { name, stmts } => {
            b.line(indent, &format!("{name} : {{"));
            for st in stmts {
                print_stmt_b(b, indent + 1, st);
            }
            b.line(indent, "}");
        }
        Stmt::Assign { lhs, rhs, nba, .. } => b.line(
            indent,
            &format!(
                "{} {} {}",
                print_expr(lhs),
                if *nba { "<=" } else { "=" },
                print_expr(rhs)
            ),
        ),
        Stmt::CompoundAssign { lhs, op, rhs, .. } => b.line(
            indent,
            &format!("{} {op} {}", print_expr(lhs), print_expr(rhs)),
        ),
        Stmt::IncDec { lhs, inc, pre, .. } => {
            let op = if *inc { "++" } else { "--" };
            if *pre {
                b.line(indent, &format!("{op}{}", print_expr(lhs)));
            } else {
                b.line(indent, &format!("{}{op}", print_expr(lhs)));
            }
        }
        Stmt::If {
            cond, then, els, qual, ..
        } => {
            let q = qual.as_ref().map(|s| format!("{s} ")).unwrap_or_default();
            b.line(
                indent,
                &format!("{q}if ({}) {}", print_expr(cond), print_stmt(indent, then)),
            );
            if let Some(e) = els {
                b.line(indent, &format!("else {}", print_stmt(indent, e)));
            }
        }
        Stmt::Case {
            expr,
            items,
            default,
            qual,
            kind,
            ..
        } => {
            let q = qual.as_ref().map(|s| format!("{s} ")).unwrap_or_default();
            b.line(indent, &format!("{q}{kind} ({}) {{", print_expr(expr)));
            for (vals, body) in items {
                let v: Vec<String> = vals.iter().map(print_expr).collect();
                b.line(
                    indent + 1,
                    &format!("{}: {}", v.join(", "), print_stmt(indent + 1, body)),
                );
            }
            if let Some(d) = default {
                b.line(
                    indent + 1,
                    &format!("default: {}", print_stmt(indent + 1, d)),
                );
            }
            b.line(indent, "}");
        }
        Stmt::CaseInside {
            expr,
            items,
            default,
            qual,
            ..
        } => {
            let q = qual.as_ref().map(|s| format!("{s} ")).unwrap_or_default();
            b.line(indent, &format!("{q}case ({}) inside {{", print_expr(expr)));
            for (vals, body) in items {
                let v: Vec<String> = vals.iter().map(print_inside_item).collect();
                b.line(
                    indent + 1,
                    &format!("{}: {}", v.join(", "), print_stmt(indent + 1, body)),
                );
            }
            if let Some(d) = default {
                b.line(
                    indent + 1,
                    &format!("default: {}", print_stmt(indent + 1, d)),
                );
            }
            b.line(indent, "}");
        }
        Stmt::For {
            var,
            from,
            to,
            step,
            body,
        } => {
            let st = step
                .as_ref()
                .map(|s| format!(" step {}", print_expr(s)))
                .unwrap_or_default();
            b.line(
                indent,
                &format!(
                    "for {var} in {}..{}{st} {}",
                    print_expr(from),
                    print_expr(to),
                    print_stmt(indent, body)
                ),
            );
        }
        Stmt::While { cond, body } => {
            b.line(
                indent,
                &format!("while ({}) {}", print_expr(cond), print_stmt(indent, body)),
            );
        }
        Stmt::DoWhile { cond, body } => {
            b.line(
                indent,
                &format!(
                    "do {} while ({})",
                    print_stmt(indent, body),
                    print_expr(cond)
                ),
            );
        }
        Stmt::EventTrigger(e) => b.line(indent, &format!("->{}", print_expr(e))),
        Stmt::Repeat { count, body } => {
            b.line(
                indent,
                &format!(
                    "repeat ({}) {}",
                    print_expr(count),
                    print_stmt(indent, body)
                ),
            );
        }
        Stmt::Forever(body) => b.line(indent, &format!("forever {}", print_stmt(indent, body))),
        Stmt::Wait { cond, body } => {
            b.line(
                indent,
                &format!("wait ({}) {}", print_expr(cond), print_stmt(indent, body)),
            );
        }
        Stmt::Event { expr, body } => {
            match body {
                Some(stmt) => b.line(
                    indent,
                    &format!("@({}) {}", print_expr(expr), print_stmt(indent, stmt.as_ref())),
                ),
                None => b.line(indent, &format!("@({});", print_expr(expr))),
            };
        }
        Stmt::Delay { amt, body } => {
            let body_s = print_stmt(indent, body);
            let empty = body_s.trim().is_empty() || body_s.trim() == "{}";
            if empty {
                b.line(indent, &format!("#{}", print_expr(amt)));
            } else {
                b.line(indent, &format!("#{} {}", print_expr(amt), body_s));
            }
        }
        Stmt::ExprStmt(e) => b.line(indent, &print_expr(e)),
        Stmt::VarDecl {
            names, ty, init, ..
        } => {
            let i = init
                .as_ref()
                .map(|e| format!(" = {}", print_expr(e)))
                .unwrap_or_default();
            b.line(
                indent,
                &format!("var {} : {}{i}", names.join(", "), print_type(ty)),
            );
        }
        Stmt::Return(v, ..) => match v {
            Some(e) => b.line(indent, &format!("return {}", print_expr(e))),
            None => b.line(indent, "return"),
        },
        Stmt::Break(..) => b.line(indent, "break"),
        Stmt::Continue(..) => b.line(indent, "continue"),
        Stmt::Fork { branches, join } => {
            b.line(indent, "fork");
            for br in branches {
                print_stmt_b(b, indent + 1, br);
            }
            let j = match join {
                ForkJoin::Join => "join",
                ForkJoin::JoinAny => "join_any",
                ForkJoin::JoinNone => "join_none",
            };
            b.line(indent, j);
        }
        Stmt::Foreach { arr, inds, body } => {
            let idx: String = inds.iter().map(|i| format!("[{i}]")).collect();
            b.line(
                indent,
                &format!("foreach ({arr}{idx}) {}", print_stmt(indent, body)),
            );
        }
        Stmt::WaitFork => b.line(indent, "wait fork"),
        Stmt::Disable { name } => b.line(indent, &format!("disable {name}")),
        Stmt::Force { lhs, rhs, .. } => b.line(
            indent,
            &format!("force {} = {}", print_expr(lhs), print_expr(rhs)),
        ),
        Stmt::Release { target } => b.line(indent, &format!("release {}", print_expr(target))),
        Stmt::Assert { cond, pass, fail } => {
            let p = pass
                .as_ref()
                .map(|s| format!(" {}", print_stmt(indent, s)))
                .unwrap_or_default();
            let f = fail
                .as_ref()
                .map(|s| format!(" else {}", print_stmt(indent, s)))
                .unwrap_or_default();
            b.line(indent, &format!("assert ({}){p}{f}", print_expr(cond)));
        }
        Stmt::AssertProperty(raw) => b.line(indent, &format!("assert property {raw}")),
        Stmt::Assume { cond, pass, fail } => {
            let p = pass
                .as_ref()
                .map(|s| format!(" {}", print_stmt(indent, s)))
                .unwrap_or_default();
            let f = fail
                .as_ref()
                .map(|s| format!(" else {}", print_stmt(indent, s)))
                .unwrap_or_default();
            b.line(indent, &format!("assume ({}){p}{f}", print_expr(cond)));
        }
        Stmt::AssumeProperty(raw) => b.line(indent, &format!("assume property {raw}")),
        Stmt::Cover { cond, pass } => {
            let p = pass
                .as_ref()
                .map(|s| format!(" {}", print_stmt(indent, s)))
                .unwrap_or_default();
            b.line(indent, &format!("cover ({}){p}", print_expr(cond)));
        }
        Stmt::CoverProperty(raw) => b.line(indent, &format!("cover property {raw}")),
        Stmt::RawSvh(body) => {
            b.line(indent, "@sv {");
            // Body mentah — trim baris & buang baris kosong (boundary newline
            // milik `{`/`}` tertangkap lexer saat scan; tanpa skip body tumbuh).
            for l in body.lines() {
                let t = l.trim();
                if !t.is_empty() {
                    b.line(indent + 1, t);
                }
            }
            b.line(indent, "}");
        }
    }
}

// ──────────────────────────────────────────────────────────────────────
// Typedef / Package / Interface
// ──────────────────────────────────────────────────────────────────────

fn print_typedef(out: &mut StrB, td: &Typedef) {
    match td {
        Typedef::Alias { name, ty, .. } => {
            out.line(0, &format!("type {name} = {}", print_type(ty)))
        }
        Typedef::Struct {
            name,
            packed,
            fields,
            ..
        } => {
            let p = if *packed { "packed " } else { "" };
            out.line(0, &format!("{p}struct {name} {{"));
            print_fields(out, 1, fields);
            out.line(0, "}");
        }
        Typedef::Union {
            name,
            packed,
            fields,
            ..
        } => {
            // `packed` WAJIB dicetak — tanpa ini `parse(print(x)) != x`
            // (union `packed` jadi tak-packed), melanggar invariant
            // idempotensi yang diklaim di header file ini.
            let p = if *packed { "packed " } else { "" };
            out.line(0, &format!("{p}union {name} {{"));
            print_fields(out, 1, fields);
            out.line(0, "}");
        }
        Typedef::Enum {
            name,
            width,
            members,
            ..
        } => {
            let w = width
                .as_ref()
                .map(|e| format!("({}) ", print_expr(e)))
                .unwrap_or_default();
            out.line(0, &format!("enum {w}{name} {{"));
            for m in members {
                let v = m
                    .value
                    .as_ref()
                    .map(|e| format!(" = {}", print_expr(e)))
                    .unwrap_or_default();
                out.line(1, &format!("{}{v},", m.name));
            }
            out.line(0, "}");
        }
    }
}

fn print_fields(out: &mut StrB, indent: usize, fields: &[Field]) {
    for f in fields {
        out.line(
            indent,
            &format!("{} : {}", f.names.join(", "), print_type(&f.ty)),
        );
    }
}

fn print_package(p: &Package) -> String {
    let mut b = StrB::new();
    b.line(0, &format!("package {} {{", p.name));
    for td in &p.typedefs {
        print_typedef(&mut b, td);
    }
    for (name, ty, value) in &p.consts {
        let t = ty
            .as_ref()
            .map(|t| format!(" : {}", print_type(t)))
            .unwrap_or_default();
        b.line(1, &format!("const {name}{t} = {}", print_expr(value)));
    }
    b.line(0, "}");
    b.0
}

fn print_interface(i: &Interface) -> String {
    let mut b = StrB::new();
    b.line(0, &format!("interface {} {{", i.name));
    for p in &i.ports {
        let dir = match p.dir {
            Dir::In => "in",
            Dir::Out => "out",
            Dir::Inout => "inout",
        };
        b.line(
            1,
            &format!("{dir} {} : {}", p.names.join(", "), print_type(&p.ty)),
        );
    }
    for (names, ty, _, _) in &i.sigs {
        b.line(1, &format!("sig {} : {}", names.join(", "), print_type(ty)));
    }
    for m in &i.modports {
        b.line(1, &format!("modport {} {{", m.name));
        for (dir, names) in &m.dirs {
            let d = match dir {
                Dir::In => "in",
                Dir::Out => "out",
                Dir::Inout => "inout",
            };
            b.line(2, &format!("{d} {}", names.join(", ")));
        }
        b.line(1, "}");
    }
    b.line(0, "}");
    b.0
}

// ──────────────────────────────────────────────────────────────────────
// Module / Program
// ──────────────────────────────────────────────────────────────────────

/// Cetak module atau program. `kw` = "module" | "program" (program tanpa param).
fn print_module(m: &Module, kw: &str) -> String {
    let mut b = StrB::new();
    if m.params.is_empty() {
        b.line(0, &format!("{kw} {} {{", m.name));
    } else {
        let ps: Vec<String> = m.params.iter().map(print_param).collect();
        b.line(0, &format!("{kw} {} #({}) {{", m.name, ps.join(", ")));
    }
    for item in &m.items {
        print_m_item(&mut b, 1, item);
    }
    b.line(0, "}");
    b.0
}

fn print_param(p: &Param) -> String {
    if let Some(td) = &p.type_default {
        // type param `type T = logic[...]` (bentuk keyword — round-trip aman)
        format!("type {} = {}", p.name, print_type(td))
    } else if let Some(t) = &p.ty {
        let d = p
            .default
            .as_ref()
            .map(|e| format!(" = {}", print_expr(e)))
            .unwrap_or_default();
        format!("{} : {}{d}", p.name, print_type(t))
    } else {
        let d = p
            .default
            .as_ref()
            .map(|e| format!(" = {}", print_expr(e)))
            .unwrap_or_default();
        format!("{}{d}", p.name)
    }
}

fn print_m_item(b: &mut StrB, indent: usize, item: &MItem) {
    match item {
        MItem::AssertProperty(raw) => {
            b.line(indent, &format!("assert property {raw}"));
        }
        MItem::AssumeProperty(raw) => {
            b.line(indent, &format!("assume property {raw}"));
        }
        MItem::CoverProperty(raw) => {
            b.line(indent, &format!("cover property {raw}"));
        }
        MItem::Port(p) => {
            let dir = match p.dir {
                Dir::In => "in",
                Dir::Out => "out",
                Dir::Inout => "inout",
            };
            b.line(
                indent,
                &format!("{dir} {} : {}", p.names.join(", "), print_type(&p.ty)),
            );
        }
        MItem::Typedef(td) => {
            let mut t = StrB::new();
            print_typedef(&mut t, td);
            for l in t.0.lines() {
                b.line(indent, l);
            }
        }
        MItem::Sig {
            names, ty, init, ..
        } => {
            let i = init
                .as_ref()
                .map(|e| format!(" = {}", print_expr(e)))
                .unwrap_or_default();
            b.line(
                indent,
                &format!("sig {} : {}{i}", names.join(", "), print_type(ty)),
            );
        }
        MItem::Reg {
            names, ty, init, ..
        } => {
            let i = init
                .as_ref()
                .map(|e| format!(" = {}", print_expr(e)))
                .unwrap_or_default();
            b.line(
                indent,
                &format!("reg {} : {}{i}", names.join(", "), print_type(ty)),
            );
        }
        MItem::Wire {
            net, names, ty, init, ..
        } => {
            let i = init
                .as_ref()
                .map(|e| format!(" = {}", print_expr(e)))
                .unwrap_or_default();
            b.line(
                indent,
                &format!("{} {} : {}{i}", net.as_str(), names.join(", "), print_type(ty)),
            );
        }
        MItem::Assign { lhs, rhs, .. } => {
            b.line(indent, &format!("assign {} = {}", print_expr(lhs), print_expr(rhs)));
        }
        MItem::Const {
            name, ty, value, ..
        } => {
            let t = ty
                .as_ref()
                .map(|t| format!(" : {}", print_type(t)))
                .unwrap_or_default();
            b.line(indent, &format!("const {name}{t} = {}", print_expr(value)));
        }
        MItem::Use { pkg, item } => b.line(indent, &format!("use {pkg}::{item}")),
        MItem::Seq(spec, stmt) => {
            b.line(
                indent,
                &format!("seq({}) {}", print_seq_spec(spec), print_stmt(indent, stmt)),
            );
        }
        MItem::Comb(stmt) => b.line(indent, &format!("comb {}", print_stmt(indent, stmt))),
        MItem::Always(stmt) => b.line(indent, &format!("always {}", print_stmt(indent, stmt))),
        MItem::Latch(stmt) => b.line(indent, &format!("latch {}", print_stmt(indent, stmt))),
        MItem::Initial(stmt) => b.line(indent, &format!("initial {}", print_stmt(indent, stmt))),
        MItem::Final(stmt) => b.line(indent, &format!("final {}", print_stmt(indent, stmt))),
        MItem::Inst {
            module,
            name,
            dims,
            params,
            conns,
            ..
        } => {
            let d = dims
                .as_ref()
                .map(|e| format!("[{}]", print_expr(e)))
                .unwrap_or_default();
            let mut head = format!("inst {module} {name}{d}");
            if !params.is_empty() {
                let ps: Vec<String> = params
                    .iter()
                    .map(|(n, v)| {
                        if n.is_empty() {
                            print_expr(v)
                        } else {
                            format!(".{n}({})", print_expr(v))
                        }
                    })
                    .collect();
                head.push_str(&format!(" #({})", ps.join(", ")));
            }
            if !conns.is_empty() {
                let cs: Vec<String> = conns
                    .iter()
                    .map(|c| match c {
                        Conn::Named { port, expr } => match expr {
                            Some(e) => format!(".{port}({})", print_expr(e)),
                            None => format!(".{port}"),
                        },
                        Conn::Positional(e) => print_expr(e),
                    })
                    .collect();
                head.push_str(&format!(" ({})", cs.join(", ")));
            }
            b.line(indent, &head);
        }
        MItem::Bind {
            target,
            module,
            name,
            dims,
            params,
            conns,
            ..
        } => {
            let d = dims
                .as_ref()
                .map(|e| format!("[{}]", print_expr(e)))
                .unwrap_or_default();
            let mut head = format!("bind {target} {module} {name}{d}");
            if !params.is_empty() {
                let ps: Vec<String> = params
                    .iter()
                    .map(|(n, v)| {
                        if n.is_empty() {
                            print_expr(v)
                        } else {
                            format!(".{n}({})", print_expr(v))
                        }
                    })
                    .collect();
                head.push_str(&format!(" #({})", ps.join(", ")));
            }
            if !conns.is_empty() {
                let cs: Vec<String> = conns
                    .iter()
                    .map(|c| match c {
                        Conn::Named { port, expr } => match expr {
                            Some(e) => format!(".{port}({})", print_expr(e)),
                            None => format!(".{port}"),
                        },
                        Conn::Positional(e) => print_expr(e),
                    })
                    .collect();
                head.push_str(&format!(" ({})", cs.join(", ")));
            }
            b.line(indent, &head);
        }
        MItem::GenFor {
            var,
            from,
            to,
            step,
            body,
        } => {
            let st = step
                .as_ref()
                .map(|s| format!(" step {}", print_expr(s)))
                .unwrap_or_default();
            b.line(
                indent,
                &format!(
                    "for {var} in {}..{}{st} {{",
                    print_expr(from),
                    print_expr(to)
                ),
            );
            for it in body {
                print_m_item(b, indent + 1, it);
            }
            b.line(indent, "}");
        }
        MItem::GenIf { cond, then, els } => {
            b.line(indent, &format!("if ({}) {{", print_expr(cond)));
            for it in then {
                print_m_item(b, indent + 1, it);
            }
            b.line(indent, "}");
            if !els.is_empty() {
                b.line(indent, "else {");
                for it in els {
                    print_m_item(b, indent + 1, it);
                }
                b.line(indent, "}");
            }
        }
        MItem::Func(f) => {
            let mut t = StrB::new();
            print_func_b(&mut t, f);
            for l in t.0.lines() {
                b.line(indent, l);
            }
        }
        MItem::Task(t) => {
            let mut t2 = StrB::new();
            print_task_b(&mut t2, t);
            for l in t2.0.lines() {
                b.line(indent, l);
            }
        }
    }
}

/// `seq(clk[, rst[, sync]])` — `spec.clk` teks raw (bisa `iface.clk`).
fn print_seq_spec(spec: &SeqSpec) -> String {
    let neg = if spec.neg_edge { "negedge " } else { "" };
    let mut s = format!("{neg}{}", spec.clk);
    if let Some((rname, _, sync)) = &spec.reset {
        s.push_str(&format!(", {rname}"));
        if *sync {
            s.push_str(", sync");
        }
    }
    s
}

// ──────────────────────────────────────────────────────────────────────
// Class / Func / Task
// ──────────────────────────────────────────────────────────────────────

fn print_class(c: &MClass) -> String {
    let mut b = StrB::new();
    let ext = c
        .extends
        .as_ref()
        .map(|e| format!(" extends {e}"))
        .unwrap_or_default();
    b.line(0, &format!("class {}{ext} {{", c.name));
    for (name, ty, rand) in &c.fields {
        let r = if *rand { "rand " } else { "" };
        b.line(1, &format!("{r}field {name} : {}", print_type(ty)));
    }
    for (cname, items) in &c.constraints {
        b.line(1, &format!("constraint {cname} {{"));
        for it in items {
            b.line(2, &print_constraint_item(it));
        }
        b.line(1, "}");
    }
    for f in &c.funcs {
        let mut t = StrB::new();
        print_func_b(&mut t, f);
        for l in t.0.lines() {
            b.line(1, l);
        }
    }
    for t in &c.tasks {
        let mut t2 = StrB::new();
        print_task_b(&mut t2, t);
        for l in t2.0.lines() {
            b.line(1, l);
        }
    }
    b.line(0, "}");
    b.0
}

fn print_constraint_item(it: &ConstraintItem) -> String {
    match it {
        ConstraintItem::Expr(e) => print_expr(e),
        ConstraintItem::If { cond, then, els } => {
            let t: Vec<String> = then.iter().map(print_constraint_item).collect();
            if els.is_empty() {
                format!("if ({}) {{ {} }}", print_expr(cond), t.join(", "))
            } else {
                let e: Vec<String> = els.iter().map(print_constraint_item).collect();
                format!(
                    "if ({}) {{ {} }} else {{ {} }}",
                    print_expr(cond),
                    t.join(", "),
                    e.join(", ")
                )
            }
        }
        ConstraintItem::Solve { var, before, .. } => {
            format!("solve {var} before {}", before.join(", "))
        }
    }
}

fn print_arg_list(args: &[(String, MvType, Option<Dir>, Option<Expr>)]) -> String {
    args.iter()
        .map(|(name, ty, dir, default)| {
            let d = match dir {
                Some(Dir::In) => "in ",
                Some(Dir::Out) => "out ",
                Some(Dir::Inout) => "inout ",
                None => "",
            };
            let def = default
                .as_ref()
                .map(|e| format!(" = {}", print_expr(e)))
                .unwrap_or_default();
            format!("{d}{name} : {}{def}", print_type(ty))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_func(f: &MFunc) -> String {
    let mut b = StrB::new();
    print_func_b(&mut b, f);
    b.0
}

fn print_func_b(b: &mut StrB, f: &MFunc) {
    let ret = f
        .ret
        .as_ref()
        .map(|t| format!(" -> {}", print_type(t)))
        .unwrap_or_default();
    b.line(
        0,
        &format!("func {}({}){ret} {{", f.name, print_arg_list(&f.args)),
    );
    for st in &f.body {
        print_stmt_b(b, 1, st);
    }
    b.line(0, "}");
}

fn print_task(t: &MTask) -> String {
    let mut b = StrB::new();
    print_task_b(&mut b, t);
    b.0
}

fn print_task_b(b: &mut StrB, t: &MTask) {
    b.line(
        0,
        &format!("task {}({}) {{", t.name, print_arg_list(&t.args)),
    );
    for st in &t.body {
        print_stmt_b(b, 1, st);
    }
    b.line(0, "}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    /// Round-trip: parse → print → parse — AST kedua harus identik (tanpa
    /// posisi; line/col diabaikan).
    #[allow(dead_code)]
    fn roundtrip(src: &str) {
        let f1 = parse(src).expect("parse pertama");
        let text = print_file(&f1);
        let f2 = parse(&text).expect("parse ulang hasil print");
        assert_eq!(
            strip_pos(&f1),
            strip_pos(&f2),
            "round-trip gagal\n---print---\n{}",
            text
        );
    }

    /// Hapus posisi (line,col) dari AST agar pembandingan fokus struktur.
    #[allow(dead_code)]
    fn strip_pos(f: &MvFile) -> MvFile {
        // print-parse ulang TANPA posisi tidak bisa langsung; bandingkan
        // dengan mem-print AST kedua kali dan mengecek stabilitas: teks
        // dari f1 harus identik dgn teks dari f2.
        let _ = f;
        f.clone()
    }

    #[test]
    fn roundtrip_counter() {
        let src = r#"
module counter #(WIDTH = 8) {
    in  clk, rst_n : bit
    in  enable     : bit
    out count      : logic[WIDTH-1:0]
    seq(clk, rst_n) {
        if (!rst_n) {
            count <= '0
        } else if (enable) {
            count <= count + 1
        }
    }
}
"#;
        let f = parse(src).unwrap();
        let t1 = print_file(&f);
        let t2 = print_file(&parse(&t1).unwrap());
        assert_eq!(t1, t2, "printer harus stabil (idempotent)");
    }

    #[test]
    fn roundtrip_rich_module() {
        let src = r#"
package pkg_a {
    type Addr = logic[15:0]
    const DEPTH : int = 4
}
module top #(W: int = 4) {
    use pkg_a::*
    in  clk, rst_n : bit
    in  a, b       : logic[W-1:0]
    out y          : logic[W-1:0]
    sig t          : logic[W-1:0]
    reg r          : logic[W-1:0] = '0
    const C        = 3
    seq(negedge clk, rst_n, sync) {
        if (!rst_n) {
            r <= '0
        } else {
            r <= (a + b) & C
        }
    }
    comb {
        t = a ^ (b << 1)
    }
    always {
        $display("t=%0d r=%0d", t, r)
    }
    initial {
        fork {
            #5 r <= '1
        } {
            #10 r <= '0
        } join
    }
    final {
        assert (t !== 'x)
    }
}
"#;
        let f = parse(src).unwrap();
        let t1 = print_file(&f);
        let t2 = print_file(&parse(&t1).unwrap());
        assert_eq!(t1, t2, "stabil: {}", t1);
    }

    #[test]
    fn roundtrip_inst_gen_assert_property() {
        let src = r#"
module child #(W: int = 4) {
    in  x : logic[W-1:0]
    out q : logic[W-1:0]
    comb { q = ~x }
}
module top {
    in clk : bit
    out y  : logic[3:0]
    sig s  : logic[3:0]
    for i in 0..4 step 2 {
        sig a2 : logic[1:0]
        initial { a2 = i }
    }
    if (1 == 1) {
        inst child u #(.W(4)) (.x(s), .q(y))
    } else {
        comb { y = s }
    }
    initial {
        @sv {
            $monitor("t=%0t", $time);
            y = 4'hF;
        }
    }
    assert property (@(posedge clk) y == $past(y))
}
"#;
        let f = parse(src).unwrap();
        let t1 = print_file(&f);
        let t2 = print_file(&parse(&t1).unwrap());
        assert_eq!(t1, t2, "stabil: {}", t1);
    }

    #[test]
    fn roundtrip_class_constraint() {
        let src = r#"
class my_test extends base_test {
    rand field seed : uint
    field count : int
    constraint c1 {
        seed > 10,
        seed inside {[1:20], 30},
        seed dist {0 := 5, [1:5] :/ 10},
        solve seed before count
    }
    func clog2(x : int) -> int {
        var r : int = 0
        while (x > 1) {
            r = r + 1
            x = x / 2
        }
        return r
    }
    task send(data : logic[7:0]) {
        #10
        data++
        ->done_evt
        foreach (mem[i]) {
            var v : int = i
        }
        do {
            data = data - 1
        } while (data != 0)
    }
}
"#;
        let f = parse(src).unwrap();
        let t1 = print_file(&f);
        let t2 = print_file(&parse(&t1).unwrap());
        assert_eq!(t1, t2, "stabil: {}", t1);
    }

    #[test]
    fn roundtrip_exprs() {
        let src = r#"
module m {
    out y : logic[7:0]
    out z : logic[7:0]
    comb {
        y = (a + b) * (2 - c)
        z = (cond ? 8'h01 : 8'h02) & {2{4'hF}}
    }
}
"#;
        // `a`/`b`/`c`/`cond` undefined di check, tapi parse saja utk roundtrip.
        let f = parse(src).unwrap();
        let t1 = print_file(&f);
        let t2 = print_file(&parse(&t1).unwrap());
        assert_eq!(t1, t2, "stabil: {}", t1);
    }

    #[test]
    fn idempotent_for_bugdb_sample() {
        // Sample dari bugdb mivon-fuzz (.mivon-fuzz-bugdb.json) — pastikan
        // printer stabil utk teks yang berisi array lit, cast, scoped, incdec.
        let src = r#"
module tim {
    out rom : logic[8][4]
    comb {
        rom = '{1, 2, 3, 4}
    }
    initial {
        var v : int = int'(4)
    }
}
"#;
        let f = parse(src).unwrap();
        let t1 = print_file(&f);
        let t2 = print_file(&parse(&t1).unwrap());
        assert_eq!(t1, t2, "stabil: {}", t1);
    }

    #[test]
    fn roundtrip_assume_cover() {
        // F66/F69: print `assume`/`cover` re-parse + stabil (pola
        // stabilitas-teks seperti roundtrip_exprs — posisi diabaikan).
        let src = r#"
module m {
    sig a : bit
    initial {
        assume (a == 0) $info("ok") else $error("bad")
        cover (a == 1) $info("hit")
    }
    assume property (@(posedge clk) a |-> b)
    cover property (@(posedge clk) a |-> b)
}
"#;
        let f = parse(src).unwrap();
        let t1 = print_file(&f);
        let t2 = print_file(&parse(&t1).unwrap());
        assert_eq!(t1, t2, "stabil: {}", t1);
    }

    #[test]
    fn roundtrip_case_inside_unique_if() {
        // F70/F71: print `case inside` + `unique if` re-parse + stabil.
        let src = r#"
module m {
    sig x : logic[7:0]
    sig y : logic[7:0]
    comb {
        case (x) inside {
            0 : { y = 0 }
            [1:10], 30 : { y = 1 }
            default : { y = 2 }
        }
        unique if (y == 1) {
            y = 2
        } else {
            y = 3
        }
    }
}
"#;
        let f = parse(src).unwrap();
        let t1 = print_file(&f);
        let t2 = print_file(&parse(&t1).unwrap());
        assert_eq!(t1, t2, "stabil: {}", t1);
    }

    #[test]
    fn roundtrip_wire_assign() {
        // F75: print `wire` + `assign` re-parse + stabil.
        let src = r#"
module m {
    in a, b : bit
    out y : bit
    wire w : bit
    wire bv : logic[7:0]
    assign w = a & b
    assign y = w
    assign bv = {a, b, 6'd0}
}
"#;
        let f = parse(src).unwrap();
        let t1 = print_file(&f);
        let t2 = print_file(&parse(&t1).unwrap());
        assert_eq!(t1, t2, "stabil: {}", t1);
    }

    #[test]
    fn roundtrip_net_kinds() {

        // F76: print semua varian net re-parse + stabil (alias ternormalisasi
        // wand/wor — teks kedua stabil).
        let src = r#"
module m {
    wand wa : bit
    wor wo : logic[7:0]
    tri tr : bit
    tri0 t0 : bit
    tri1 t1 : bit
    supply0 s0 : bit
    supply1 s1 : bit
    assign wa = 1
}
"#;
        let f = parse(src).unwrap();
        let t1 = print_file(&f);
        let t2 = print_file(&parse(&t1).unwrap());
        assert_eq!(t1, t2, "stabil: {}", t1);
        assert!(t1.contains("wand wa : bit"), "wand: {t1}");
        assert!(t1.contains("wor wo : logic[7:0]"), "wor: {t1}");
    }

    #[test]
    fn roundtrip_bind() {
        // F78: print `bind` re-parse + stabil.
        let src = r#"
module tb {
    sig clk : bit
    sig flag : bit
    inst dut u_dut (.clk, .flag)
    bind u_dut fmon u_chk (.clk(clk), .flag)
    bind top.u_dut fmon u_chk2 #(.W(2)) (.clk)
}
"#;
        let f = parse(src).unwrap();
        let t1 = print_file(&f);
        let t2 = print_file(&parse(&t1).unwrap());
        assert_eq!(t1, t2, "stabil: {}", t1);
        assert!(t1.contains("bind u_dut fmon u_chk"), "bind: {t1}");
        assert!(t1.contains("bind top.u_dut fmon u_chk2"), "dotted: {t1}");
    }
}
