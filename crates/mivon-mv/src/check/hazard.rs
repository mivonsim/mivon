//! Hazard checker bawaan §9A — Static Analysis & RTL Hazard.
//! Jalan OTOMATIS di `check` (E30xx blokir) + warning W3xxx non-blokir.
//! AST-only, konservatif, tanpa elaborasi. 1 file = 1 tanggung jawab.

use crate::ast::*;
use crate::{MvError, MvWarning};
use std::collections::{HashMap, HashSet};

use super::err_at;

// ── kunci LHS & reads ──

/// Kunci driver: `q` → `q`, `packet.valid` → `packet.valid`,
/// `rom[i]`/`q[hi:lo]`/`q[i +: w]` → base (`rom`/`q`, indeks diabaikan).
fn lhs_key(e: &Expr) -> Option<String> {
    match e {
        Expr::Ident(n, ..) => Some(n.clone()),
        Expr::Member(o, f, ..) => Some(format!("{}.{}", lhs_key(o)?, f)),
        Expr::Index(o, _) => lhs_key(o),
        Expr::Range(o, _, _) => lhs_key(o),
        Expr::PartSelect { base, .. } => lhs_key(base),
        Expr::Paren(inner) => lhs_key(inner),
        _ => None,
    }
}

/// Path baca lengkap untuk E3003: `axi_if.awvalid` → `axi_if.awvalid`
/// (bukan root saja — `awready = awvalid` field-ke-field BUKAN loop).
fn read_path(e: &Expr) -> Option<String> {
    match e {
        Expr::Ident(n, ..) => Some(n.clone()),
        Expr::Member(o, f, ..) => Some(format!("{}.{}", read_path(o)?, f)),
        Expr::Index(o, _) => read_path(o),
        Expr::Range(o, _, _) => read_path(o),
        Expr::PartSelect { base, .. } => read_path(base),
        Expr::Paren(inner) => read_path(inner),
        _ => None,
    }
}

/// Indeks/range di dalam rantai akses (`a[i].v` membaca `i`).
fn reads_idx(e: &Expr, out: &mut Vec<String>) {
    match e {
        Expr::Index(o, i) => {
            reads_idx(o, out);
            reads_paths(i, out);
        }
        Expr::Range(o, hi, lo) => {
            reads_idx(o, out);
            reads_paths(hi, out);
            reads_paths(lo, out);
        }
        Expr::Member(o, ..) => reads_idx(o, out),
        Expr::Paren(inner) => reads_idx(inner, out),
        _ => {}
    }
}

/// Kumpulkan semua path yang dibaca ekspresi (full member-chain + operan).
fn reads_paths(e: &Expr, out: &mut Vec<String>) {
    match e {
        Expr::Ident(n, ..) => out.push(n.clone()),
        Expr::Scoped(..) => {}
        // Member: hanya full path + indeks di dalamnya — base BUKAN read
        // (`q.ready = q.valid` bukan loop).
        Expr::Member(o, ..) => {
            if let Some(p) = read_path(e) {
                out.push(p);
            }
            reads_idx(o, out);
        }
        Expr::Index(o, i) => {
            reads_paths(o, out);
            reads_paths(i, out);
        }
        Expr::Range(o, hi, lo) => {
            reads_paths(o, out);
            reads_paths(hi, out);
            reads_paths(lo, out);
        }
        Expr::PartSelect {
            base, from, width, ..
        } => {
            if let Some(p) = read_path(base) {
                out.push(p);
            }
            reads_paths(base, out);
            reads_paths(from, out);
            reads_paths(width, out);
        }
        Expr::Paren(inner) | Expr::Unary(_, inner) => reads_paths(inner, out),
        Expr::IncDec { expr, .. } => reads_paths(expr, out),
        Expr::Binary(_, l, r) => {
            reads_paths(l, out);
            reads_paths(r, out);
        }
        Expr::Ternary(c, t, f) => {
            reads_paths(c, out);
            reads_paths(t, out);
            reads_paths(f, out);
        }
        Expr::Call(_, args, ..) => {
            for a in args {
                reads_paths(a, out);
            }
        }
        Expr::NamedArg { expr, .. } => reads_paths(expr, out),
        Expr::MethodCall {
            obj,
            args,
            with_clause,
            ..
        } => {
            reads_paths(obj, out);
            for a in args {
                reads_paths(a, out);
            }
            if let Some(wc) = with_clause {
                reads_paths(wc, out);
            }
        }
        Expr::Concat(ps) | Expr::ArrayLit(ps) => {
            for p in ps {
                reads_paths(p, out);
            }
        }
        Expr::Replicate(n, inner) => {
            reads_paths(n, out);
            reads_paths(inner, out);
        }
        Expr::Cast { expr, .. } => reads_paths(expr, out),
        Expr::Inside { expr, items } => {
            reads_paths(expr, out);
            for it in items {
                match it {
                    InsideItem::Value(v) => reads_paths(v, out),
                    InsideItem::Range(lo, hi) => {
                        reads_paths(lo, out);
                        reads_paths(hi, out);
                    }
                }
            }
        }
        Expr::Dist { expr, items } => {
            reads_paths(expr, out);
            for it in items {
                reads_paths(&it.value, out);
                if let Some((lo, hi)) = &it.range {
                    reads_paths(lo, out);
                    reads_paths(hi, out);
                }
                reads_paths(&it.weight, out);
            }
        }
        _ => {}
    }
}

/// Loop combinational di level path: kunci LHS `k` vs path baca `r` —
/// sama persis, atau kunci MEMPERLUAS bacaan (`p.v = p` membaca keseluruhan
/// termasuk targetnya). Arah sebaliknya (`p = p.field`, rakitan struct dari
/// field ter-register) BUKAN loop.
fn is_self_path(key: &str, read: &str) -> bool {
    key == read || key.starts_with(&format!("{read}."))
}

/// Root sebelum `.` — `packet.valid` → `packet`. Untuk himpunan sinyal.
fn root_of(key: &str) -> &str {
    match key.find('.') {
        Some(i) => &key[..i],
        None => key,
    }
}

/// Kumpulkan semua sinyal yang DIBACA ekspresi (Ident biasa).
/// `pkg::item` (scoped) + nama fungsi dilewati — bukan sinyal.
fn reads_expr(e: &Expr, out: &mut Vec<String>) {
    match e {
        Expr::Ident(n, ..) => out.push(n.clone()),
        Expr::Scoped(..) => {}
        Expr::Member(o, ..) => reads_expr(o, out),
        Expr::Index(o, i) => {
            reads_expr(o, out);
            reads_expr(i, out);
        }
        Expr::Range(o, hi, lo) => {
            reads_expr(o, out);
            reads_expr(hi, out);
            reads_expr(lo, out);
        }
        Expr::PartSelect {
            base, from, width, ..
        } => {
            reads_expr(base, out);
            reads_expr(from, out);
            reads_expr(width, out);
        }
        Expr::Paren(inner) | Expr::Unary(_, inner) => reads_expr(inner, out),
        Expr::IncDec { expr, .. } => reads_expr(expr, out),
        Expr::Binary(_, l, r) => {
            reads_expr(l, out);
            reads_expr(r, out);
        }
        Expr::Ternary(c, t, f) => {
            reads_expr(c, out);
            reads_expr(t, out);
            reads_expr(f, out);
        }
        Expr::Call(name, args, ..) => {
            let _ = name;
            for a in args {
                reads_expr(a, out);
            }
        }
        Expr::NamedArg { expr, .. } => reads_expr(expr, out),
        Expr::MethodCall {
            obj,
            args,
            with_clause,
            ..
        } => {
            reads_expr(obj, out);
            for a in args {
                reads_expr(a, out);
            }
            if let Some(wc) = with_clause {
                reads_expr(wc, out);
            }
        }
        Expr::Concat(ps) | Expr::ArrayLit(ps) => {
            for p in ps {
                reads_expr(p, out);
            }
        }
        Expr::Replicate(n, inner) => {
            reads_expr(n, out);
            reads_expr(inner, out);
        }
        Expr::Cast { expr, .. } => reads_expr(expr, out),
        Expr::Inside { expr, items } => {
            reads_expr(expr, out);
            for it in items {
                match it {
                    InsideItem::Value(v) => reads_expr(v, out),
                    InsideItem::Range(lo, hi) => {
                        reads_expr(lo, out);
                        reads_expr(hi, out);
                    }
                }
            }
        }
        Expr::Dist { expr, items } => {
            reads_expr(expr, out);
            for it in items {
                reads_expr(&it.value, out);
                if let Some((lo, hi)) = &it.range {
                    reads_expr(lo, out);
                    reads_expr(hi, out);
                }
                reads_expr(&it.weight, out);
            }
        }
        _ => {}
    }
}

// ── driver & reads per statement ──

fn drivers_stmt(s: &Stmt, out: &mut Vec<(String, usize, usize)>) {
    match s {
        Stmt::Assign { lhs, line, col, .. } => {
            if let Some(k) = lhs_key(lhs) {
                out.push((k, *line, *col));
            }
        }
        Stmt::CompoundAssign { lhs, line, col, .. } => {
            if let Some(k) = lhs_key(lhs) {
                out.push((k, *line, *col));
            }
        }
        Stmt::IncDec { lhs, line, col, .. } => {
            if let Some(k) = lhs_key(lhs) {
                out.push((k, *line, *col));
            }
        }
        Stmt::Force { lhs, line, col, .. } => {
            if let Some(k) = lhs_key(lhs) {
                out.push((k, *line, *col));
            }
        }
        Stmt::Block(ss) => {
            for x in ss {
                drivers_stmt(x, out);
            }
        }
        Stmt::NamedBlock { stmts, .. } => {
            for x in stmts {
                drivers_stmt(x, out);
            }
        }
        Stmt::If { then, els, .. } => {
            drivers_stmt(then, out);
            if let Some(e) = els {
                drivers_stmt(e, out);
            }
        }
        Stmt::Case { items, default, .. } => {
            for (_, b) in items {
                drivers_stmt(b, out);
            }
            if let Some(d) = default {
                drivers_stmt(d, out);
            }
        }
        Stmt::CaseInside { items, default, .. } => {
            for (_, b) in items {
                drivers_stmt(b, out);
            }
            if let Some(d) = default {
                drivers_stmt(d, out);
            }
        }
        Stmt::For { body, .. } => drivers_stmt(body, out),
        Stmt::While { body, .. } | Stmt::DoWhile { body, .. } => drivers_stmt(body, out),
        Stmt::Repeat { body, .. } => drivers_stmt(body, out),
        Stmt::Forever(b) => drivers_stmt(b, out),
        Stmt::Wait { body, .. } => drivers_stmt(body, out),
        Stmt::Event { body: Some(b), .. } => drivers_stmt(b, out),
        Stmt::Delay { body, .. } => drivers_stmt(body, out),
        Stmt::Fork { branches, .. } => {
            for b in branches {
                drivers_stmt(b, out);
            }
        }
        Stmt::Foreach { body, .. } => drivers_stmt(body, out),
        Stmt::Assert { pass, fail, .. } | Stmt::Assume { pass, fail, .. } => {
            if let Some(p) = pass {
                drivers_stmt(p, out);
            }
            if let Some(f) = fail {
                drivers_stmt(f, out);
            }
        }
        Stmt::Cover { pass: Some(p), .. } => drivers_stmt(p, out),
        // Statement-level `x++` kadang ter-parse sebagai `ExprStmt(IncDec)`
        // (jalur fallback parser) — IncDec di dalamnya tetap driver.
        Stmt::ExprStmt(e) => drivers_expr(e, out),
        _ => {}
    }
}

/// Driver di dalam ekspresi statement (`ExprStmt(IncDec)`): posisi (0,0) —
/// node `Expr::IncDec` tak membawa span; E3001 tetap menyebut nama sinyal.
fn drivers_expr(e: &Expr, out: &mut Vec<(String, usize, usize)>) {
    match e {
        Expr::IncDec { expr, .. } => {
            if let Some(k) = lhs_key(expr) {
                out.push((k, 0, 0));
            }
            drivers_expr(expr, out);
        }
        Expr::Paren(inner) | Expr::Unary(_, inner) => drivers_expr(inner, out),
        Expr::Binary(_, l, r) => {
            drivers_expr(l, out);
            drivers_expr(r, out);
        }
        Expr::Ternary(c, t, f) => {
            drivers_expr(c, out);
            drivers_expr(t, out);
            drivers_expr(f, out);
        }
        Expr::Call(_, args, ..) => {
            for a in args {
                drivers_expr(a, out);
            }
        }
        Expr::NamedArg { expr, .. } => drivers_expr(expr, out),
        Expr::MethodCall {
            obj,
            args,
            with_clause,
            ..
        } => {
            drivers_expr(obj, out);
            for a in args {
                drivers_expr(a, out);
            }
            if let Some(wc) = with_clause {
                drivers_expr(wc, out);
            }
        }
        Expr::Member(o, ..) => drivers_expr(o, out),
        Expr::Index(o, i) => {
            drivers_expr(o, out);
            drivers_expr(i, out);
        }
        Expr::Range(o, hi, lo) => {
            drivers_expr(o, out);
            drivers_expr(hi, out);
            drivers_expr(lo, out);
        }
        Expr::PartSelect {
            base, from, width, ..
        } => {
            drivers_expr(base, out);
            drivers_expr(from, out);
            drivers_expr(width, out);
        }
        Expr::Concat(ps) | Expr::ArrayLit(ps) => {
            for p in ps {
                drivers_expr(p, out);
            }
        }
        Expr::Replicate(n, inner) => {
            drivers_expr(n, out);
            drivers_expr(inner, out);
        }
        Expr::Cast { expr, .. } => drivers_expr(expr, out),
        Expr::Inside { expr, items } => {
            drivers_expr(expr, out);
            for it in items {
                match it {
                    InsideItem::Value(v) => drivers_expr(v, out),
                    InsideItem::Range(lo, hi) => {
                        drivers_expr(lo, out);
                        drivers_expr(hi, out);
                    }
                }
            }
        }
        Expr::Dist { expr, items } => {
            drivers_expr(expr, out);
            for it in items {
                drivers_expr(&it.value, out);
                if let Some((lo, hi)) = &it.range {
                    drivers_expr(lo, out);
                    drivers_expr(hi, out);
                }
                drivers_expr(&it.weight, out);
            }
        }
        _ => {}
    }
}

/// Reads sisi LHS tanpa root-nya sendiri: `a[i] = x` membaca `i` (dan
/// sub-ekspresi indeks lain) tapi base `a` adalah DRIVER, bukan pembaca —
/// kalau base ikut dihitung read, W3003 (ditulis-tak-dibaca) mati total.
fn reads_lhs_noroot(e: &Expr, out: &mut Vec<String>) {
    match e {
        Expr::Ident(..) | Expr::Scoped(..) => {}
        Expr::Member(o, ..) => reads_lhs_noroot(o, out),
        Expr::Index(o, i) => {
            reads_lhs_noroot(o, out);
            reads_expr(i, out);
        }
        Expr::Range(o, hi, lo) => {
            reads_lhs_noroot(o, out);
            reads_expr(hi, out);
            reads_expr(lo, out);
        }
        Expr::PartSelect {
            base, from, width, ..
        } => {
            reads_lhs_noroot(base, out);
            reads_expr(from, out);
            reads_expr(width, out);
        }
        Expr::Paren(inner) => reads_lhs_noroot(inner, out),
        _ => reads_expr(e, out),
    }
}

fn reads_stmt(s: &Stmt, out: &mut Vec<String>) {
    match s {
        Stmt::Assign { lhs, rhs, .. } => {
            reads_lhs_noroot(lhs, out);
            reads_expr(rhs, out);
        }
        // `q += x` / `q++` membaca base-nya juga (read-modify-write),
        // selain sub-ekspresi indeks.
        Stmt::CompoundAssign { lhs, rhs, .. } => {
            reads_lhs_noroot(lhs, out);
            if let Some(k) = lhs_key(lhs) {
                out.push(root_of(&k).to_string());
            }
            reads_expr(rhs, out);
        }
        Stmt::IncDec { lhs, .. } => {
            reads_lhs_noroot(lhs, out);
            if let Some(k) = lhs_key(lhs) {
                out.push(root_of(&k).to_string());
            }
        }
        Stmt::Force { lhs, rhs, .. } => {
            reads_lhs_noroot(lhs, out);
            reads_expr(rhs, out);
        }
        Stmt::Release { target } => reads_expr(target, out),
        Stmt::Block(ss) => {
            for x in ss {
                reads_stmt(x, out);
            }
        }
        Stmt::NamedBlock { stmts, .. } => {
            for x in stmts {
                reads_stmt(x, out);
            }
        }
        Stmt::If {
            cond, then, els, ..
        } => {
            reads_expr(cond, out);
            reads_stmt(then, out);
            if let Some(e) = els {
                reads_stmt(e, out);
            }
        }
        Stmt::Case {
            expr,
            items,
            default,
            ..
        } => {
            reads_expr(expr, out);
            for (vs, b) in items {
                for v in vs {
                    reads_expr(v, out);
                }
                reads_stmt(b, out);
            }
            if let Some(d) = default {
                reads_stmt(d, out);
            }
        }
        Stmt::CaseInside {
            expr,
            items,
            default,
            ..
        } => {
            reads_expr(expr, out);
            for (vs, b) in items {
                for v in vs {
                    match v {
                        InsideItem::Value(x) => reads_expr(x, out),
                        InsideItem::Range(lo, hi) => {
                            reads_expr(lo, out);
                            reads_expr(hi, out);
                        }
                    }
                }
                reads_stmt(b, out);
            }
            if let Some(d) = default {
                reads_stmt(d, out);
            }
        }
        Stmt::For {
            from,
            to,
            step,
            body,
            ..
        } => {
            reads_expr(from, out);
            reads_expr(to, out);
            if let Some(s) = step {
                reads_expr(s, out);
            }
            reads_stmt(body, out);
        }
        Stmt::While { cond, body, .. } | Stmt::DoWhile { cond, body, .. } => {
            reads_expr(cond, out);
            reads_stmt(body, out);
        }
        Stmt::Repeat { count, body, .. } => {
            reads_expr(count, out);
            reads_stmt(body, out);
        }
        Stmt::Forever(b) => reads_stmt(b, out),
        Stmt::Wait { cond, body, .. } => {
            reads_expr(cond, out);
            reads_stmt(body, out);
        }
        Stmt::Event { expr, body, .. } => {
            reads_expr(expr, out);
            if let Some(b) = body {
                reads_stmt(b, out);
            }
        }
        Stmt::EventTrigger(e) | Stmt::ExprStmt(e) => reads_expr(e, out),
        Stmt::Delay { amt, body, .. } => {
            reads_expr(amt, out);
            reads_stmt(body, out);
        }
        Stmt::VarDecl { init: Some(e), .. } => reads_expr(e, out),
        Stmt::Return(Some(e), ..) => reads_expr(e, out),
        Stmt::Fork { branches, .. } => {
            for b in branches {
                reads_stmt(b, out);
            }
        }
        // `arr` yang di-foreach dibaca (sebaliknya sinyal yang hanya muncul
        // di sini kena W3004 palsu).
        Stmt::Foreach { arr, body, .. } => {
            out.push(arr.clone());
            reads_stmt(body, out);
        }
        Stmt::Assert { cond, pass, fail } | Stmt::Assume { cond, pass, fail } => {
            reads_expr(cond, out);
            if let Some(p) = pass {
                reads_stmt(p, out);
            }
            if let Some(f) = fail {
                reads_stmt(f, out);
            }
        }
        Stmt::Cover { cond, pass } => {
            reads_expr(cond, out);
            if let Some(p) = pass {
                reads_stmt(p, out);
            }
        }
        _ => {}
    }
}

// ── sumber prosedural per module ──

#[derive(Clone, Copy, PartialEq, Eq)]
enum SrcKind {
    Seq,
    Comb,
    Always,
    Initial,
    Final,
    Assign,
}

struct Src {
    kind: SrcKind,
    drivers: Vec<(String, usize, usize)>,
    reads: Vec<String>,
    // seq saja
    clk: Option<String>,
    reset: Option<String>,
    reset_sync: bool,
    line: usize,
    col: usize,
    /// Konteks cabang generate: daftar (branch_id, side). Dua sumber yang
    /// berbagi ancestor dengan side BERBEDA (then vs else, case-item berbeda)
    /// saling eksklusif — hanya satu yang survive elaborasi (§9A, E3001).
    /// `GenFor` TIDAK menambah tag (iterasi koeksis).
    branch: Vec<(usize, usize)>,
}

fn push_stmt_src(
    kind: SrcKind,
    body: &Stmt,
    line: usize,
    col: usize,
    branch: &[(usize, usize)],
    out: &mut Vec<Src>,
) {
    let mut drivers = Vec::new();
    drivers_stmt(body, &mut drivers);
    let mut reads = Vec::new();
    reads_stmt(body, &mut reads);
    out.push(Src {
        kind,
        drivers,
        reads,
        clk: None,
        reset: None,
        reset_sync: false,
        line,
        col,
        branch: branch.to_vec(),
    });
}

fn collect_srcs(
    items: &[MItem],
    branch: &[(usize, usize)],
    next_id: &mut usize,
    out: &mut Vec<Src>,
) {
    for it in items {
        match it {
            MItem::Seq(spec, body) => {
                let mut drivers = Vec::new();
                drivers_stmt(body, &mut drivers);
                let mut reads = Vec::new();
                reads_stmt(body, &mut reads);
                out.push(Src {
                    kind: SrcKind::Seq,
                    drivers,
                    reads,
                    clk: Some(spec.clk.clone()),
                    reset: spec.reset.as_ref().map(|(n, _, _)| n.clone()),
                    reset_sync: spec.reset.as_ref().map(|(_, _, s)| *s).unwrap_or(true),
                    line: spec.line,
                    col: spec.col,
                    branch: branch.to_vec(),
                });
            }
            MItem::Comb(b) => push_stmt_src(SrcKind::Comb, b, 0, 0, branch, out),
            MItem::Always(b) | MItem::Latch(b) => {
                push_stmt_src(SrcKind::Always, b, 0, 0, branch, out)
            }
            MItem::Initial(b) => push_stmt_src(SrcKind::Initial, b, 0, 0, branch, out),
            MItem::Final(b) => push_stmt_src(SrcKind::Final, b, 0, 0, branch, out),
            MItem::Assign {
                lhs,
                rhs,
                line,
                col,
            } => {
                let mut drivers = Vec::new();
                if let Some(k) = lhs_key(lhs) {
                    drivers.push((k, *line, *col));
                }
                let mut reads = Vec::new();
                reads_lhs_noroot(lhs, &mut reads);
                reads_expr(rhs, &mut reads);
                out.push(Src {
                    kind: SrcKind::Assign,
                    drivers,
                    reads,
                    clk: None,
                    reset: None,
                    reset_sync: false,
                    line: *line,
                    col: *col,
                    branch: branch.to_vec(),
                });
            }
            // GenFor: iterasi koeksis — teruskan konteks tanpa tag baru.
            MItem::GenFor { body, .. } => collect_srcs(body, branch, next_id, out),
            // GenIf then vs else saling eksklusif — tag sisi berbeda.
            MItem::GenIf { then, els, .. } => {
                let id = *next_id;
                *next_id += 1;
                let mut br_then = branch.to_vec();
                br_then.push((id, 0));
                collect_srcs(then, &br_then, next_id, out);
                let mut br_else = branch.to_vec();
                br_else.push((id, 1));
                collect_srcs(els, &br_else, next_id, out);
            }
            // GenCase: tiap item + default saling eksklusif.
            MItem::GenCase { items, default, .. } => {
                let id = *next_id;
                *next_id += 1;
                for (idx, (_, b)) in items.iter().enumerate() {
                    let mut br = branch.to_vec();
                    br.push((id, idx));
                    collect_srcs(b, &br, next_id, out);
                }
                let mut br_def = branch.to_vec();
                br_def.push((id, usize::MAX));
                collect_srcs(default, &br_def, next_id, out);
            }
            _ => {}
        }
    }
}

/// Nama net (wire/wand/...) — dikecualikan E3001 (resolusi multi-driver).
fn net_names(m: &Module) -> HashSet<String> {
    let mut s = HashSet::new();
    fn walk(items: &[MItem], s: &mut HashSet<String>) {
        for it in items {
            match it {
                MItem::Wire { names, .. } => {
                    for n in names {
                        s.insert(n.clone());
                    }
                }
                MItem::GenFor { body, .. } => walk(body, s),
                MItem::GenIf { then, els, .. } => {
                    walk(then, s);
                    walk(els, s);
                }
                MItem::GenCase { items, default, .. } => {
                    for (_, b) in items {
                        walk(b, s);
                    }
                    walk(default, s);
                }
                _ => {}
            }
        }
    }
    walk(&m.items, &mut s);
    s
}

fn is_tb(m: &Module) -> bool {
    m.items
        .iter()
        .any(|i| matches!(i, MItem::Initial(_) | MItem::Final(_)))
}

/// Sinyal disebut di koneksi instansiasi — heuristik konservatif:
/// dianggap sudah dipakai (lewati W3003/W3004) + dianggap driven (E3004).
fn inst_mentions(m: &Module) -> HashSet<String> {
    let mut s = HashSet::new();
    fn walk(items: &[MItem], s: &mut HashSet<String>) {
        for it in items {
            match it {
                MItem::Inst { conns, .. } | MItem::Bind { conns, .. } => {
                    for c in conns {
                        match c {
                            Conn::Named { port, expr } => {
                                s.insert(port.clone());
                                if let Some(e) = expr {
                                    let mut r = Vec::new();
                                    reads_expr(e, &mut r);
                                    for x in r {
                                        s.insert(x);
                                    }
                                    if let Some(k) = lhs_key(e) {
                                        s.insert(root_of(&k).to_string());
                                    }
                                } else {
                                    s.insert(port.clone());
                                }
                            }
                            Conn::Positional(e) => {
                                let mut r = Vec::new();
                                reads_expr(e, &mut r);
                                for x in r {
                                    s.insert(x);
                                }
                            }
                        }
                    }
                }
                MItem::GenFor { body, .. } => walk(body, s),
                MItem::GenIf { then, els, .. } => {
                    walk(then, s);
                    walk(els, s);
                }
                MItem::GenCase { items, default, .. } => {
                    for (_, b) in items {
                        walk(b, s);
                    }
                    walk(default, s);
                }
                _ => {}
            }
        }
    }
    walk(&m.items, &mut s);
    s
}

// ── latch helpers (E3002) ──

/// Kunci yang di-drive tak-bersyarat di level atas body `comb`
/// (idiom anti-latch `q = 0; if (en) { q = d }` — bukan latch).
fn top_driven_keys(s: &Stmt, out: &mut HashSet<String>) {
    match s {
        Stmt::Block(ss) => {
            for x in ss {
                top_driven_keys(x, out);
            }
        }
        Stmt::Assign { lhs, .. }
        | Stmt::CompoundAssign { lhs, .. }
        | Stmt::IncDec { lhs, .. }
        | Stmt::Force { lhs, .. } => {
            if let Some(k) = lhs_key(lhs) {
                out.insert(k);
            }
        }
        Stmt::ExprStmt(e) => {
            let mut d = Vec::new();
            drivers_expr(e, &mut d);
            for (k, _, _) in d {
                out.insert(k);
            }
        }
        _ => {}
    }
}

fn branch_drivers(s: &Stmt) -> Vec<(String, usize, usize)> {
    let mut d = Vec::new();
    drivers_stmt(s, &mut d);
    d
}

/// Cari pola latch di body `comb`: `if` tanpa `else` / `case` tanpa
/// `default` yang meng-assign sinyal TAK TERCOVER `safe`
/// (default-assignment tak-bersyarat). Return posisi driver pertama.
fn find_latch(s: &Stmt, safe: &HashSet<String>) -> Option<(usize, usize)> {
    match s {
        Stmt::If { then, els, .. } => {
            if els.is_none() {
                let d = branch_drivers(then);
                if let Some((_, l, c)) = d.iter().find(|(k, _, _)| !safe.contains(k)) {
                    return Some((*l, *c));
                }
            }
            if let Some(x) = find_latch(then, safe) {
                return Some(x);
            }
            if let Some(e) = els {
                return find_latch(e, safe);
            }
            None
        }
        Stmt::Case { items, default, .. } => {
            if default.is_none() {
                for (_, b) in items {
                    let d = branch_drivers(b);
                    if let Some((_, l, c)) = d.iter().find(|(k, _, _)| !safe.contains(k)) {
                        return Some((*l, *c));
                    }
                }
            }
            for (_, b) in items {
                if let Some(x) = find_latch(b, safe) {
                    return Some(x);
                }
            }
            if let Some(d) = default {
                return find_latch(d, safe);
            }
            None
        }
        Stmt::CaseInside { items, default, .. } => {
            if default.is_none() {
                for (_, b) in items {
                    let d = branch_drivers(b);
                    if let Some((_, l, c)) = d.iter().find(|(k, _, _)| !safe.contains(k)) {
                        return Some((*l, *c));
                    }
                }
            }
            for (_, b) in items {
                if let Some(x) = find_latch(b, safe) {
                    return Some(x);
                }
            }
            if let Some(d) = default {
                return find_latch(d, safe);
            }
            None
        }
        Stmt::Block(ss) | Stmt::Fork { branches: ss, .. } => {
            for x in ss {
                if let Some(p) = find_latch(x, safe) {
                    return Some(p);
                }
            }
            None
        }
        Stmt::NamedBlock { stmts, .. } => {
            for x in stmts {
                if let Some(p) = find_latch(x, safe) {
                    return Some(p);
                }
            }
            None
        }
        Stmt::For { body, .. }
        | Stmt::While { body, .. }
        | Stmt::DoWhile { body, .. }
        | Stmt::Repeat { body, .. }
        | Stmt::Forever(body)
        | Stmt::Foreach { body, .. } => find_latch(body, safe),
        Stmt::Wait { body, .. } => find_latch(body, safe),
        Stmt::Event { body, .. } => body.as_ref().and_then(|b| find_latch(b, safe)),
        Stmt::Delay { body, .. } => find_latch(body, safe),
        _ => None,
    }
}

/// `case`/`case inside` tanpa `default` di body `seq` → W3005.
fn find_case_no_default(s: &Stmt, out: &mut Vec<(usize, usize)>) {
    match s {
        Stmt::Case {
            default: None,
            line,
            col,
            ..
        }
        | Stmt::CaseInside {
            default: None,
            line,
            col,
            ..
        } => {
            out.push((*line, *col));
        }
        Stmt::Case { items, default, .. } => {
            for (_, b) in items {
                find_case_no_default(b, out);
            }
            if let Some(d) = default {
                find_case_no_default(d, out);
            }
        }
        Stmt::CaseInside { items, default, .. } => {
            for (_, b) in items {
                find_case_no_default(b, out);
            }
            if let Some(d) = default {
                find_case_no_default(d, out);
            }
        }
        Stmt::Block(ss) | Stmt::Fork { branches: ss, .. } => {
            for x in ss {
                find_case_no_default(x, out);
            }
        }
        Stmt::NamedBlock { stmts, .. } => {
            for x in stmts {
                find_case_no_default(x, out);
            }
        }
        Stmt::If { then, els, .. } => {
            find_case_no_default(then, out);
            if let Some(e) = els {
                find_case_no_default(e, out);
            }
        }
        Stmt::For { body, .. }
        | Stmt::While { body, .. }
        | Stmt::DoWhile { body, .. }
        | Stmt::Repeat { body, .. }
        | Stmt::Forever(body)
        | Stmt::Foreach { body, .. } => find_case_no_default(body, out),
        Stmt::Wait { body, .. } => find_case_no_default(body, out),
        Stmt::Event { body: Some(b), .. } => find_case_no_default(b, out),
        Stmt::Event { body: None, .. } => {}
        Stmt::Delay { body, .. } => find_case_no_default(body, out),
        _ => {}
    }
}

/// Kedalaman `if` maksimum di statement.
fn max_if_depth(s: &Stmt, cur: usize) -> usize {
    match s {
        Stmt::If { then, els, .. } => {
            let d = cur + 1;
            let mut m = d.max(max_if_depth(then, d));
            if let Some(e) = els {
                m = m.max(max_if_depth(e, d));
            }
            m
        }
        Stmt::Block(ss) | Stmt::Fork { branches: ss, .. } => {
            ss.iter().map(|x| max_if_depth(x, cur)).max().unwrap_or(cur)
        }
        Stmt::NamedBlock { stmts, .. } => stmts
            .iter()
            .map(|x| max_if_depth(x, cur))
            .max()
            .unwrap_or(cur),
        Stmt::Case { items, default, .. } => {
            let mut m = cur;
            for (_, b) in items {
                m = m.max(max_if_depth(b, cur));
            }
            if let Some(d) = default {
                m = m.max(max_if_depth(d, cur));
            }
            m
        }
        Stmt::CaseInside { items, default, .. } => {
            let mut m = cur;
            for (_, b) in items {
                m = m.max(max_if_depth(b, cur));
            }
            if let Some(d) = default {
                m = m.max(max_if_depth(d, cur));
            }
            m
        }
        Stmt::For { body, .. }
        | Stmt::While { body, .. }
        | Stmt::DoWhile { body, .. }
        | Stmt::Repeat { body, .. }
        | Stmt::Forever(body)
        | Stmt::Foreach { body, .. } => max_if_depth(body, cur),
        Stmt::Wait { body, .. } => max_if_depth(body, cur),
        Stmt::Event { body, .. } => body.as_ref().map(|b| max_if_depth(b, cur)).unwrap_or(cur),
        Stmt::Delay { body, .. } => max_if_depth(body, cur),
        Stmt::Assert { pass, fail, .. } | Stmt::Assume { pass, fail, .. } => {
            let mut m = cur;
            if let Some(p) = pass {
                m = m.max(max_if_depth(p, cur));
            }
            if let Some(f) = fail {
                m = m.max(max_if_depth(f, cur));
            }
            m
        }
        Stmt::Cover { pass, .. } => pass.as_ref().map(|p| max_if_depth(p, cur)).unwrap_or(cur),
        _ => cur,
    }
}

// ── entry error ──

/// Dua sumber saling eksklusif bila berbagi ancestor generate dengan side
/// berbeda (then vs else, item case berbeda) — hanya satu yang survive.
fn mutually_exclusive(a: &[(usize, usize)], b: &[(usize, usize)]) -> bool {
    for (id_a, side_a) in a {
        for (id_b, side_b) in b {
            if id_a == id_b && side_a != side_b {
                return true;
            }
        }
    }
    false
}

/// `true` bila module memuat `@sv` verbatim di body prosedural mana pun.
/// Checker tak bisa melihat driver di dalamnya → E3004 + W3003/W3004
/// dilewati (konservatif, hindari false positive).
fn stmt_has_raw(s: &Stmt) -> bool {
    match s {
        Stmt::RawSvh(_) => true,
        Stmt::Block(ss) | Stmt::Fork { branches: ss, .. } => ss.iter().any(stmt_has_raw),
        Stmt::NamedBlock { stmts, .. } => stmts.iter().any(stmt_has_raw),
        Stmt::If { then, els, .. } => {
            stmt_has_raw(then) || els.as_ref().is_some_and(|e| stmt_has_raw(e))
        }
        Stmt::Case { items, default, .. } => {
            items.iter().any(|(_, b)| stmt_has_raw(b))
                || default.as_ref().is_some_and(|d| stmt_has_raw(d))
        }
        Stmt::CaseInside { items, default, .. } => {
            items.iter().any(|(_, b)| stmt_has_raw(b))
                || default.as_ref().is_some_and(|d| stmt_has_raw(d))
        }
        Stmt::For { body, .. }
        | Stmt::While { body, .. }
        | Stmt::DoWhile { body, .. }
        | Stmt::Repeat { body, .. }
        | Stmt::Forever(body)
        | Stmt::Foreach { body, .. } => stmt_has_raw(body),
        Stmt::Wait { body, .. } => stmt_has_raw(body),
        Stmt::Event { body, .. } => body.as_ref().is_some_and(|b| stmt_has_raw(b)),
        Stmt::Delay { body, .. } => stmt_has_raw(body),
        Stmt::Assert { pass, fail, .. } | Stmt::Assume { pass, fail, .. } => {
            pass.as_ref().is_some_and(|p| stmt_has_raw(p))
                || fail.as_ref().is_some_and(|f| stmt_has_raw(f))
        }
        Stmt::Cover { pass, .. } => pass.as_ref().is_some_and(|p| stmt_has_raw(p)),
        _ => false,
    }
}

fn items_have_raw(items: &[MItem]) -> bool {
    for it in items {
        let hit = match it {
            MItem::Seq(_, b)
            | MItem::Comb(b)
            | MItem::Always(b)
            | MItem::Latch(b)
            | MItem::Initial(b)
            | MItem::Final(b) => stmt_has_raw(b),
            MItem::GenFor { body, .. } => items_have_raw(body),
            MItem::GenIf { then, els, .. } => items_have_raw(then) || items_have_raw(els),
            MItem::GenCase { items, default, .. } => {
                items.iter().any(|(_, b)| items_have_raw(b)) || items_have_raw(default)
            }
            _ => false,
        };
        if hit {
            return true;
        }
    }
    false
}

/// Error keras E3001–E3006 untuk satu module/program. Fail-fast.
pub(crate) fn hazard_check_module(m: &Module) -> Result<(), MvError> {
    let mut srcs = Vec::new();
    let mut next_id = 0usize;
    collect_srcs(&m.items, &[], &mut next_id, &mut srcs);
    let nets = net_names(m);
    let inst = inst_mentions(m);
    let tb = is_tb(m);

    // ── E3001 multi-driver (driver dari `initial`/`final` tidak dihitung —
    // stimulus sekali-jalan, mis. idiom `clk = 0` + `forever` clock-gen;
    // cabang generate eksklusif tidak dihitung) ──
    {
        let mut owners: HashMap<String, Vec<(usize, usize, usize)>> = HashMap::new();
        for (idx, s) in srcs.iter().enumerate() {
            if matches!(s.kind, SrcKind::Initial | SrcKind::Final) {
                continue;
            }
            let mut seen = HashSet::new();
            for (k, l, c) in &s.drivers {
                if !seen.insert(k.clone()) {
                    continue;
                }
                // Net resolution dikecualikan; field interface (`a.b`)
                // di-track per-field (kunci penuh) — sudah distinct.
                // `force` dihitung driver (override fault-injection).
                if nets.contains(root_of(k)) {
                    continue;
                }
                owners.entry(k.clone()).or_default().push((idx, *l, *c));
            }
        }
        // Urutan key di-sort agar error-pertama deterministik.
        let mut keys: Vec<&String> = owners.keys().collect();
        keys.sort();
        for k in keys {
            let v = &owners[k];
            // Kelompokkan sumber per konteks eksklusif: dua sumber dalam
            // kelompok sama hanya bila TIDAK saling eksklusif. Error bila
            // ada kelompok berisi >1 sumber berbeda.
            let mut groups: Vec<Vec<(usize, usize, usize)>> = Vec::new();
            'outer: for cand in v {
                for g in groups.iter_mut() {
                    // Masuk grup pertama yang semua anggotanya koeksis.
                    if g.iter().all(|(i, _, _)| {
                        !mutually_exclusive(&srcs[*i].branch, &srcs[cand.0].branch)
                    }) {
                        g.push(*cand);
                        continue 'outer;
                    }
                }
                groups.push(vec![*cand]);
            }
            if let Some(g) = groups
                .iter()
                .find(|g| g.iter().map(|(i, _, _)| *i).collect::<HashSet<_>>().len() > 1)
            {
                let (_, l, c) = g[1];
                let n = g.iter().map(|(i, _, _)| *i).collect::<HashSet<_>>().len();
                return Err(err_at(
                    l,
                    c,
                    "E3001",
                    format!(
                        "sinyal '{k}' di-drive {n} sumber (multi-driver) — di '{}'",
                        m.name
                    ),
                ));
            }
        }
    }

    // ── E3002 latch di comb (cek langsung dari items agar posisi tepat) ──
    fn check_comb_items(items: &[MItem], mname: &str) -> Result<(), MvError> {
        for it in items {
            match it {
                MItem::Comb(b) => {
                    // Sinyal yang sudah di-drive tak-bersyarat di body yang
                    // sama bukan latch (idiom default-assignment).
                    let mut safe = HashSet::new();
                    top_driven_keys(b, &mut safe);
                    if let Some((l, c)) = find_latch(b, &safe) {
                        return Err(err_at(
                            l,
                            c,
                            "E3002",
                            format!("comb tanpa else/default meng-infer latch — di '{mname}'"),
                        ));
                    }
                }
                MItem::GenFor { body, .. } => check_comb_items(body, mname)?,
                MItem::GenIf { then, els, .. } => {
                    check_comb_items(then, mname)?;
                    check_comb_items(els, mname)?;
                }
                MItem::GenCase { items, default, .. } => {
                    for (_, b) in items {
                        check_comb_items(b, mname)?;
                    }
                    check_comb_items(default, mname)?;
                }
                _ => {}
            }
        }
        Ok(())
    }
    check_comb_items(&m.items, &m.name)?;

    // ── E3003 combinational loop (assign kontinu + comb self-read) ──
    {
        // assign kontinu level module
        fn check_assign_items(items: &[MItem], mname: &str) -> Result<(), MvError> {
            for it in items {
                match it {
                    MItem::Assign {
                        lhs,
                        rhs,
                        line,
                        col,
                    } => {
                        if let Some(k) = lhs_key(lhs) {
                            let mut reads = Vec::new();
                            reads_paths(rhs, &mut reads);
                            if reads.iter().any(|x| is_self_path(&k, x)) {
                                return Err(err_at(
                                    *line,
                                    *col,
                                    "E3003",
                                    format!(
                                        "combinational loop pada '{}' (self-reference) — di '{mname}'",
                                        root_of(&k)
                                    ),
                                ));
                            }
                        }
                    }
                    MItem::GenFor { body, .. } => check_assign_items(body, mname)?,
                    MItem::GenIf { then, els, .. } => {
                        check_assign_items(then, mname)?;
                        check_assign_items(els, mname)?;
                    }
                    MItem::GenCase { items, default, .. } => {
                        for (_, b) in items {
                            check_assign_items(b, mname)?;
                        }
                        check_assign_items(default, mname)?;
                    }
                    _ => {}
                }
            }
            Ok(())
        }
        check_assign_items(&m.items, &m.name)?;
        // comb/always self-read: `comb { y = y & 1 }`, `comb { q++ }`
        fn check_comb_self(s: &Stmt, mname: &str) -> Result<(), MvError> {
            match s {
                Stmt::Assign {
                    lhs,
                    rhs,
                    line,
                    col,
                    ..
                }
                | Stmt::CompoundAssign {
                    lhs,
                    rhs,
                    line,
                    col,
                    ..
                } => {
                    if let Some(k) = lhs_key(lhs) {
                        let mut reads = Vec::new();
                        reads_paths(rhs, &mut reads);
                        if reads.iter().any(|x| is_self_path(&k, x)) {
                            return Err(err_at(
                                *line,
                                *col,
                                "E3003",
                                format!("combinational loop pada '{}' — di '{mname}'", root_of(&k)),
                            ));
                        }
                    }
                    Ok(())
                }
                // `q++` di comb/always = read-modify-write kombinasional.
                Stmt::IncDec { lhs, line, col, .. } => {
                    if let Some(k) = lhs_key(lhs) {
                        return Err(err_at(
                            *line,
                            *col,
                            "E3003",
                            format!("combinational loop pada '{}' — di '{mname}'", root_of(&k)),
                        ));
                    }
                    Ok(())
                }
                Stmt::Block(ss) | Stmt::Fork { branches: ss, .. } => {
                    for x in ss {
                        check_comb_self(x, mname)?;
                    }
                    Ok(())
                }
                Stmt::NamedBlock { stmts, .. } => {
                    for x in stmts {
                        check_comb_self(x, mname)?;
                    }
                    Ok(())
                }
                Stmt::If { then, els, .. } => {
                    check_comb_self(then, mname)?;
                    if let Some(e) = els {
                        check_comb_self(e, mname)?;
                    }
                    Ok(())
                }
                Stmt::Case { items, default, .. } => {
                    for (_, b) in items {
                        check_comb_self(b, mname)?;
                    }
                    if let Some(d) = default {
                        check_comb_self(d, mname)?;
                    }
                    Ok(())
                }
                Stmt::CaseInside { items, default, .. } => {
                    for (_, b) in items {
                        check_comb_self(b, mname)?;
                    }
                    if let Some(d) = default {
                        check_comb_self(d, mname)?;
                    }
                    Ok(())
                }
                Stmt::For { body, .. }
                | Stmt::While { body, .. }
                | Stmt::DoWhile { body, .. }
                | Stmt::Repeat { body, .. }
                | Stmt::Forever(body)
                | Stmt::Foreach { body, .. } => check_comb_self(body, mname),
                Stmt::Wait { body, .. } => check_comb_self(body, mname),
                Stmt::Event { body, .. } => {
                    if let Some(b) = body {
                        check_comb_self(b, mname)?;
                    }
                    Ok(())
                }
                Stmt::Delay { body, .. } => check_comb_self(body, mname),
                _ => Ok(()),
            }
        }
        fn check_comb_items_self(items: &[MItem], mname: &str) -> Result<(), MvError> {
            for it in items {
                match it {
                    MItem::Comb(b) | MItem::Always(b) => check_comb_self(b, mname)?,
                    MItem::GenFor { body, .. } => check_comb_items_self(body, mname)?,
                    MItem::GenIf { then, els, .. } => {
                        check_comb_items_self(then, mname)?;
                        check_comb_items_self(els, mname)?;
                    }
                    MItem::GenCase { items, default, .. } => {
                        for (_, b) in items {
                            check_comb_items_self(b, mname)?;
                        }
                        check_comb_items_self(default, mname)?;
                    }
                    _ => {}
                }
            }
            Ok(())
        }
        check_comb_items_self(&m.items, &m.name)?;
    }

    // ── E3004 output mengambang (DUT saja) ──
    // Dilewati bila: testbench, modul stub tanpa driver sama sekali (uji
    // koneksi F29–F32 memakai modul stub port-only), atau modul memuat `@sv`
    // verbatim (driver tersembunyi tak terlihat checker).
    let has_any_driver = srcs.iter().any(|s| !s.drivers.is_empty());
    if !tb && has_any_driver && !items_have_raw(&m.items) {
        let mut driven_roots: HashSet<String> = HashSet::new();
        for s in &srcs {
            for (k, _, _) in &s.drivers {
                driven_roots.insert(root_of(k).to_string());
            }
        }
        for it in &m.items {
            if let MItem::Port(p) = it {
                if p.dir != Dir::Out {
                    continue;
                }
                for n in &p.names {
                    if driven_roots.contains(n) || inst.contains(n) {
                        continue;
                    }
                    return Err(err_at(
                        p.line,
                        p.col,
                        "E3004",
                        format!("output '{n}' tidak pernah di-drive — di '{}'", m.name),
                    ));
                }
            }
        }
    }

    // ── E3005 reset async tanpa cabang ──
    for s in &srcs {
        if s.kind != SrcKind::Seq {
            continue;
        }
        let rname = match &s.reset {
            Some(n) if !s.reset_sync => n.clone(),
            _ => continue,
        };
        let rbase = rname.split('.').next().unwrap_or(&rname).to_string();
        let mut seen = false;
        for r in &s.reads {
            if r == &rbase || r == &rname {
                seen = true;
                break;
            }
        }
        if !seen {
            for (k, _, _) in &s.drivers {
                if root_of(k) == rbase || k == &rname {
                    seen = true;
                    break;
                }
            }
        }
        if !seen {
            return Err(err_at(
                s.line,
                s.col,
                "E3005",
                format!(
                    "reset async '{rname}' tak pernah ditangani di seq — di '{}'",
                    m.name
                ),
            ));
        }
    }

    // ── E3006 clock di-drive logika (`initial`/`final` dikecualikan:
    // clock-gen idiomatik; `latch` tercakup via Always, `force` via sumber
    // yang memuatnya) ──
    // Clock dibandingkan FULL path (`iface.clk` ≠ `iface.awready`).
    {
        let mut clks: HashSet<String> = HashSet::new();
        for s in &srcs {
            if s.kind == SrcKind::Seq {
                if let Some(c) = &s.clk {
                    clks.insert(c.clone());
                }
            }
        }
        for s in &srcs {
            if matches!(s.kind, SrcKind::Initial | SrcKind::Final | SrcKind::Seq) {
                continue;
            }
            for (k, l, c) in &s.drivers {
                if clks.contains(k) {
                    return Err(err_at(
                        *l,
                        *c,
                        "E3006",
                        format!("clock '{k}' di-drive logika — di '{}'", m.name),
                    ));
                }
            }
        }
    }

    Ok(())
}

/// Error file (modules + programs).
pub(crate) fn hazard_check_file(file: &MvFile) -> Result<(), MvError> {
    for m in file.modules.iter().chain(file.programs.iter()) {
        hazard_check_module(m)?;
    }
    Ok(())
}

// ── warnings ──

fn decls_of(m: &Module) -> Vec<(String, usize, usize)> {
    let mut out = Vec::new();
    fn walk(items: &[MItem], out: &mut Vec<(String, usize, usize)>) {
        for it in items {
            match it {
                MItem::Sig {
                    names, line, col, ..
                }
                | MItem::Reg {
                    names, line, col, ..
                }
                | MItem::Wire {
                    names, line, col, ..
                } => {
                    for n in names {
                        out.push((n.clone(), *line, *col));
                    }
                }
                MItem::GenFor { body, .. } => walk(body, out),
                MItem::GenIf { then, els, .. } => {
                    walk(then, out);
                    walk(els, out);
                }
                MItem::GenCase { items, default, .. } => {
                    for (_, b) in items {
                        walk(b, out);
                    }
                    walk(default, out);
                }
                _ => {}
            }
        }
    }
    walk(&m.items, &mut out);
    out
}

/// Bacaan di luar sumber prosedural untuk akuntansi W3003/W3004:
/// init `sig`/`reg`/`wire`, nilai `const`, body `func`/`task` (hanya READS —
/// body func/task bukan driver kontinu, jadi tidak masuk `written`),
/// `coverpoint`/bins, dan item `specify`.
fn extra_reads(items: &[MItem], read: &mut HashSet<String>) {
    fn stmts_body(ss: &[Stmt], read: &mut HashSet<String>) {
        let mut r = Vec::new();
        for s in ss {
            reads_stmt(s, &mut r);
        }
        for x in r {
            read.insert(x);
        }
    }
    for it in items {
        match it {
            MItem::Sig { init, .. } | MItem::Reg { init, .. } | MItem::Wire { init, .. } => {
                if let Some(e) = init {
                    let mut r = Vec::new();
                    reads_expr(e, &mut r);
                    for x in r {
                        read.insert(x);
                    }
                }
            }
            MItem::Const { value, .. } => {
                let mut r = Vec::new();
                reads_expr(value, &mut r);
                for x in r {
                    read.insert(x);
                }
            }
            MItem::Func(f) => {
                for (_, _, _, def) in &f.args {
                    if let Some(e) = def {
                        let mut r = Vec::new();
                        reads_expr(e, &mut r);
                        for x in r {
                            read.insert(x);
                        }
                    }
                }
                stmts_body(&f.body, read);
            }
            MItem::Task(t) => {
                for (_, _, _, def) in &t.args {
                    if let Some(e) = def {
                        let mut r = Vec::new();
                        reads_expr(e, &mut r);
                        for x in r {
                            read.insert(x);
                        }
                    }
                }
                stmts_body(&t.body, read);
            }
            MItem::Covergroup(cg) => {
                if let Some((_, sig)) = &cg.event {
                    read.insert(sig.clone());
                }
                for cp in &cg.points {
                    let mut r = Vec::new();
                    reads_expr(&cp.expr, &mut r);
                    for b in &cp.bins {
                        for item in &b.items {
                            match item {
                                InsideItem::Value(v) => reads_expr(v, &mut r),
                                InsideItem::Range(lo, hi) => {
                                    reads_expr(lo, &mut r);
                                    reads_expr(hi, &mut r);
                                }
                            }
                        }
                    }
                    for x in r {
                        read.insert(x);
                    }
                }
            }
            MItem::Specify(spec) => {
                for item in &spec.items {
                    match item {
                        SpecifyItem::Specparam { value, .. } => {
                            let mut r = Vec::new();
                            reads_expr(value, &mut r);
                            for x in r {
                                read.insert(x);
                            }
                        }
                        SpecifyItem::Setup {
                            data, clock, limit, ..
                        }
                        | SpecifyItem::Hold {
                            data, clock, limit, ..
                        } => {
                            let mut r = Vec::new();
                            reads_expr(data, &mut r);
                            reads_expr(limit, &mut r);
                            for x in r {
                                read.insert(x);
                            }
                            read.insert(clock.clone());
                        }
                    }
                }
            }
            MItem::GenFor { body, .. } => extra_reads(body, read),
            MItem::GenIf { then, els, .. } => {
                extra_reads(then, read);
                extra_reads(els, read);
            }
            MItem::GenCase { items, default, .. } => {
                for (_, b) in items {
                    extra_reads(b, read);
                }
                extra_reads(default, read);
            }
            _ => {}
        }
    }
}

/// Warning W3001–W3006 untuk satu module/program (dedup, deterministik).
pub(crate) fn hazard_warnings_module(m: &Module) -> Vec<MvWarning> {
    let mut out: Vec<MvWarning> = Vec::new();
    let mut srcs = Vec::new();
    let mut next_id = 0usize;
    collect_srcs(&m.items, &[], &mut next_id, &mut srcs);
    let inst = inst_mentions(m);
    let has_raw = items_have_raw(&m.items);

    // W3001 CDC: >1 clock berbeda
    {
        let mut seen: Vec<(String, usize, usize)> = Vec::new();
        for s in &srcs {
            if s.kind != SrcKind::Seq {
                continue;
            }
            if let Some(c) = &s.clk {
                let base = c.split('.').next().unwrap_or(c).to_string();
                if !seen.iter().any(|(x, _, _)| x == &base) {
                    seen.push((base, s.line, s.col));
                }
            }
        }
        if seen.len() > 1 {
            let (_, l, c) = seen[1].clone();
            let names: Vec<String> = seen.iter().map(|(x, _, _)| x.clone()).collect();
            out.push(MvWarning::new(
                l,
                c,
                "W3001",
                format!(
                    "CDC: {} domain clock ({}) dalam '{}' — pastikan sinkronisasi",
                    seen.len(),
                    names.join(", "),
                    m.name
                ),
            ));
        }
    }

    // W3002 async reset perlu sync-release
    for s in &srcs {
        if s.kind != SrcKind::Seq {
            continue;
        }
        if let Some(r) = &s.reset {
            if !s.reset_sync {
                out.push(MvWarning::new(
                    s.line,
                    s.col,
                    "W3002",
                    format!(
                        "reset async '{r}' — pastikan deassert sinkron (sync-release) di '{}'",
                        m.name
                    ),
                ));
            }
        }
    }

    // W3003/W3004 unused/undriven internal (dilewati bila ada @sv verbatim).
    if !has_raw {
        let mut written: HashSet<String> = HashSet::new();
        let mut read: HashSet<String> = HashSet::new();
        for s in &srcs {
            for (k, _, _) in &s.drivers {
                written.insert(root_of(k).to_string());
            }
            for r in &s.reads {
                read.insert(r.clone());
            }
        }
        // Bacaan di luar sumber prosedural: init deklarasi, nilai const,
        // body func/task, coverpoint/bins, specify — kalau tidak, sinyal
        // yang hanya dipakai di sana kena W3003 palsu.
        extra_reads(&m.items, &mut read);
        // koneksi inst dihitung terpakai (baca+tulis) — konservatif.
        for x in &inst {
            written.insert(x.clone());
            read.insert(x.clone());
        }
        // port bukan internal — kecualikan dari unused.
        let mut ports: HashSet<String> = HashSet::new();
        for it in &m.items {
            if let MItem::Port(p) = it {
                for n in &p.names {
                    ports.insert(n.clone());
                }
            }
        }
        for (n, l, c) in decls_of(m) {
            if ports.contains(&n) || inst.contains(&n) {
                continue;
            }
            let w = written.contains(&n);
            let r = read.contains(&n);
            if w && !r {
                out.push(MvWarning::new(
                    l,
                    c,
                    "W3003",
                    format!(
                        "sinyal '{n}' ditulis tapi tak pernah dibaca — di '{}'",
                        m.name
                    ),
                ));
            } else if r && !w {
                out.push(MvWarning::new(
                    l,
                    c,
                    "W3004",
                    format!(
                        "sinyal '{n}' dibaca tapi tak pernah di-drive — di '{}'",
                        m.name
                    ),
                ));
            }
        }
    }

    // W3005 case tanpa default di seq
    {
        fn walk_seq(items: &[MItem], out: &mut Vec<MvWarning>, mname: &str) {
            for it in items {
                match it {
                    MItem::Seq(_, b) => {
                        let mut v = Vec::new();
                        find_case_no_default(b, &mut v);
                        for (l, c) in v {
                            out.push(MvWarning::new(
                                l,
                                c,
                                "W3005",
                                format!(
                                    "case tanpa default di seq — rawan X-propagation di '{mname}'"
                                ),
                            ));
                        }
                    }
                    MItem::GenFor { body, .. } => walk_seq(body, out, mname),
                    MItem::GenIf { then, els, .. } => {
                        walk_seq(then, out, mname);
                        walk_seq(els, out, mname);
                    }
                    MItem::GenCase { items, default, .. } => {
                        for (_, b) in items {
                            walk_seq(b, out, mname);
                        }
                        walk_seq(default, out, mname);
                    }
                    _ => {}
                }
            }
        }
        walk_seq(&m.items, &mut out, &m.name);
    }

    // W3006 nesting if >4
    {
        fn walk_depth(items: &[MItem]) -> usize {
            let mut mx = 0;
            for it in items {
                let d = match it {
                    MItem::Seq(_, b)
                    | MItem::Comb(b)
                    | MItem::Always(b)
                    | MItem::Initial(b)
                    | MItem::Final(b) => max_if_depth(b, 0),
                    MItem::GenFor { body, .. } => walk_depth(body),
                    MItem::GenIf { then, els, .. } => walk_depth(then).max(walk_depth(els)),
                    MItem::GenCase { items, default, .. } => {
                        let mut mm = walk_depth(default);
                        for (_, b) in items {
                            mm = mm.max(walk_depth(b));
                        }
                        mm
                    }
                    _ => 0,
                };
                mx = mx.max(d);
            }
            mx
        }
        let d = walk_depth(&m.items);
        if d > 4 {
            out.push(MvWarning::new(
                m.line,
                m.col,
                "W3006",
                format!(
                    "nesting if kedalaman {d} (>4) — pertimbangkan case di '{}'",
                    m.name
                ),
            ));
        }
    }

    // dedup (kode+baris+kolom), urutan deterministik
    let mut seen = HashSet::new();
    out.retain(|w| seen.insert((w.code.clone(), w.line, w.col, w.msg.clone())));
    out.sort_by(|a, b| (a.line, a.col, &a.code).cmp(&(b.line, b.col, &b.code)));
    out
}

/// Warning file (modules + programs).
pub(crate) fn hazard_warnings_file(file: &MvFile) -> Vec<MvWarning> {
    let mut out = Vec::new();
    for m in file.modules.iter().chain(file.programs.iter()) {
        out.extend(hazard_warnings_module(m));
    }
    out
}
