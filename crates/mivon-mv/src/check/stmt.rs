//! Check — statement: assignment rules (E2002/03/04), kontrol flow, loop
//! depth, fork, assert, delay/event. 1 file = 1 tanggung jawab.

use super::{err_at, expr::check_expr, BlockKind, Ctx, Scope};
use crate::ast::*;
use crate::MvError;

/// Kunci kanonik untuk label `case` — konstanta yang bernilai sama di dua
/// branch dianggap label yang sama. `2'd0` dan `0` harus menghasilkan kunci
/// yang sama, kalau tidak cek duplikat lolos.
fn expr_label_key(e: &Expr) -> String {
    match e {
        Expr::Int(v) => format!("#{v}"),
        Expr::Sized(w, b, d, ..) => {
            // Digit x/z/? (wildcard) tak bisa di-fold ke integer — kunci
            // fallback berbasis teks, dipisah dari bentuk numerik.
            let clean: String = d.chars().filter(|c| *c != '_').collect();
            let radix = match b {
                'b' => Some(2),
                'o' => Some(8),
                'd' => Some(10),
                'h' => Some(16),
                _ => None,
            };
            match (radix, i64::from_str_radix(&clean, radix.unwrap_or(10))) {
                (Some(_), Ok(v)) => format!("#{v}"),
                _ => format!("{}'{b}{clean}", w.unwrap_or(-1)),
            }
        }
        Expr::Fill(c) => format!("'{c}"),
        Expr::Unary(o, inner) => format!("({o}{})", expr_label_key(inner)),
        Expr::Paren(inner) => expr_label_key(inner),
        Expr::Binary(o, l, r) => format!("({}{o}{})", expr_label_key(l), expr_label_key(r)),
        Expr::Ident(n, ..) => n.clone(),
        other => format!("{other:?}"),
    }
}

pub(crate) fn check_stmt<'a>(
    stmt: &'a Stmt,
    ctx: &'a Ctx<'a>,
    scope: &mut Scope<'a>,
    kind: BlockKind,
) -> Result<(), MvError> {
    match stmt {
        Stmt::Block(stmts) => {
            for s in stmts {
                check_stmt(s, ctx, scope, kind)?;
            }
            Ok(())
        }
        // F47: blok bernama — isi di-check seperti Block biasa.
        Stmt::NamedBlock { stmts, .. } => {
            for s in stmts {
                check_stmt(s, ctx, scope, kind)?;
            }
            Ok(())
        }
        Stmt::Assign {
            lhs,
            rhs,
            nba,
            line,
            col,
        } => {
            // E2004: operator assignment di konteks salah
            if kind == BlockKind::Seq && !*nba {
                return Err(err_at(
                    *line,
                    *col,
                    "E2004",
                    format!(
                        "blocking assign '=' tidak boleh di dalam seq (pakai '<=') — di '{}'",
                        scope.env.mname
                    ),
                ));
            }
            if kind != BlockKind::Seq && *nba {
                return Err(err_at(
                    *line,
                    *col,
                    "E2004",
                    format!(
                        "non-blocking assign '<=' hanya boleh di dalam seq — di '{}'",
                        scope.env.mname
                    ),
                ));
            }
            // E2003: drive port input hanya diizinkan di body initial/final.
            if kind != BlockKind::Tb {
                if let Some(base) = base_ident(lhs) {
                    if let Some(Dir::In) = scope.env.ports.get(base) {
                        return Err(err_at(
                            *line,
                            *col,
                            "E2003",
                            format!(
                                "cannot drive input port '{base}' — di '{}'",
                                scope.env.mname
                            ),
                        ));
                    }
                }
            }
            check_expr(lhs, ctx, scope, 0)?;
            check_expr(rhs, ctx, scope, 0)?;
            check_lvalue_not_const(lhs, scope, *line, *col)?;
            // E2002: RHS lebih lebar dari LHS → truncation
            let wl = super::expr::expr_width(lhs, ctx, scope, 0);
            let wr = super::expr::expr_width(rhs, ctx, scope, 0);
            if let (Some(l), Some(r)) = (wl, wr) {
                if r > l {
                    return Err(err_at(
                        *line,
                        *col,
                        "E2002",
                        format!(
                            "lebar {r} bit ke sinyal {l}-bit '{}' — di '{}'",
                            describe_lhs(lhs),
                            scope.env.mname
                        ),
                    ));
                }
            }
            Ok(())
        }
        // F36: `lhs += rhs` — compound assignment (blocking, seperti `=`).
        Stmt::CompoundAssign {
            lhs,
            op,
            rhs,
            line,
            col,
        } => {
            if kind == BlockKind::Seq {
                return Err(err_at(
                    *line,
                    *col,
                    "E2004",
                    format!(
                        "compound assign '{op}' tidak boleh di dalam seq (pakai '<=') — di '{}'",
                        scope.env.mname
                    ),
                ));
            }
            if kind != BlockKind::Tb {
                if let Some(base) = base_ident(lhs) {
                    if let Some(Dir::In) = scope.env.ports.get(base) {
                        return Err(err_at(
                            *line,
                            *col,
                            "E2003",
                            format!(
                                "cannot drive input port '{base}' — di '{}'",
                                scope.env.mname
                            ),
                        ));
                    }
                }
            }
            check_expr(lhs, ctx, scope, 0)?;
            check_expr(rhs, ctx, scope, 0)?;
            check_lvalue_not_const(lhs, scope, *line, *col)?;
            let wl = super::expr::expr_width(lhs, ctx, scope, 0);
            let wr = super::expr::expr_width(rhs, ctx, scope, 0);
            if let (Some(l), Some(r)) = (wl, wr) {
                if r > l {
                    return Err(err_at(
                        *line,
                        *col,
                        "E2002",
                        format!(
                            "lebar {r} bit ke sinyal {l}-bit '{}' — di '{}'",
                            describe_lhs(lhs),
                            scope.env.mname
                        ),
                    ));
                }
            }
            Ok(())
        }
        // F36: `lhs++` / `lhs--` — increment (blocking, seperti `=`).
        Stmt::IncDec { lhs, line, col, .. } => {
            if kind == BlockKind::Seq {
                return Err(err_at(
                    *line,
                    *col,
                    "E2004",
                    format!(
                        "increment/decrement tidak boleh di dalam seq (pakai '<=') — di '{}'",
                        scope.env.mname
                    ),
                ));
            }
            if kind != BlockKind::Tb {
                if let Some(base) = base_ident(lhs) {
                    if let Some(Dir::In) = scope.env.ports.get(base) {
                        return Err(err_at(
                            *line,
                            *col,
                            "E2003",
                            format!(
                                "cannot drive input port '{base}' — di '{}'",
                                scope.env.mname
                            ),
                        ));
                    }
                }
            }
            check_expr(lhs, ctx, scope, 0)?;
            check_lvalue_not_const(lhs, scope, *line, *col)?;
            Ok(())
        }
        Stmt::If {
            cond, then, els, ..
        } => {
            check_expr(cond, ctx, scope, 0)?;
            check_stmt(then, ctx, scope, kind)?;
            if let Some(e) = els {
                check_stmt(e, ctx, scope, kind)?;
            }
            Ok(())
        }
        Stmt::Case {
            expr,
            items,
            default,
            qual,
            line,
            col,
            ..
        } => {
            check_expr(expr, ctx, scope, 0)?;
            // E2013: label `case` yang duplikat membuat branch kedua TAK
            // PERNAH dieksekusi — bug senyap yang lolos review karena SV
            // sendiri tidak menolak (untuk `unique`/`priority` dia hanya
            // memberi peringatan runtime, LRM 1800 §10.10.4).
            //
            // Kelengkapan `default` SENGAJA tidak dipaksa: pada `case`
            // biasa SV meng-fallback ke x/z (LRM 1800 §10.10.1) dan pada
            // `casez`/`casex` pola tanpa-default itu justru idiom dekoder
            // yang wajar — memaksanya akan jadi false positive.
            let mut seen: Vec<String> = Vec::new();
            for (vals, body) in items {
                for v in vals {
                    check_expr(v, ctx, scope, 0)?;
                    let key = expr_label_key(v);
                    if seen.contains(&key) {
                        let qual_s = qual.as_deref().unwrap_or("");
                        return Err(err_at(
                            *line,
                            *col,
                            "E2013",
                            format!(
                                "label case '{key}' duplikat — branch ini tidak akan \
                                 pernah dieksekusi{}",
                                if qual_s.is_empty() {
                                    String::new()
                                } else {
                                    format!(" (qualifier '{qual_s}')")
                                }
                            ),
                        ));
                    }
                    seen.push(key);
                }
                check_stmt(body, ctx, scope, kind)?;
            }
            if let Some(d) = default {
                check_stmt(d, ctx, scope, kind)?;
            }
            Ok(())
        }
        Stmt::CaseInside {
            expr,
            items,
            default,
            qual,
            line,
            col,
        } => {
            check_expr(expr, ctx, scope, 0)?;
            // E2013 untuk label NILAI duplikat (kunci sama seperti `case`
            // biasa). Rentang `[lo:hi]` dilewati konservatif — overlap
            // parsial antar-rentang bukan duplikat pasti.
            let mut seen: Vec<String> = Vec::new();
            for (vals, body) in items {
                for v in vals {
                    match v {
                        InsideItem::Value(e) => {
                            check_expr(e, ctx, scope, 0)?;
                            let key = expr_label_key(e);
                            if seen.contains(&key) {
                                let qual_s = qual.as_deref().unwrap_or("");
                                return Err(err_at(
                                    *line,
                                    *col,
                                    "E2013",
                                    format!(
                                        "label case inside '{key}' duplikat — branch ini tidak akan \
                                         pernah dieksekusi{}",
                                        if qual_s.is_empty() {
                                            String::new()
                                        } else {
                                            format!(" (qualifier '{qual_s}')")
                                        }
                                    ),
                                ));
                            }
                            seen.push(key);
                        }
                        InsideItem::Range(lo, hi) => {
                            check_expr(lo, ctx, scope, 0)?;
                            check_expr(hi, ctx, scope, 0)?;
                        }
                    }
                }
                check_stmt(body, ctx, scope, kind)?;
            }
            if let Some(d) = default {
                check_stmt(d, ctx, scope, kind)?;
            }
            Ok(())
        }
        Stmt::For {
            var,
            from,
            to,
            step,
            body,
        } => {
            check_expr(from, ctx, scope, 0)?;
            check_expr(to, ctx, scope, 0)?;
            if let Some(s) = step {
                check_expr(s, ctx, scope, 0)?;
            }
            let mut inner = scope.clone();
            inner.sigs.insert(var.as_str());
            inner.loop_depth += 1;
            check_stmt(body, ctx, &mut inner, kind)
        }
        Stmt::While { cond, body } => {
            check_expr(cond, ctx, scope, 0)?;
            let mut inner = scope.clone();
            inner.loop_depth += 1;
            check_stmt(body, ctx, &mut inner, kind)
        }
        // F38: `do { body } while (cond)` — post-test.
        Stmt::DoWhile { cond, body } => {
            let mut inner = scope.clone();
            inner.loop_depth += 1;
            check_stmt(body, ctx, &mut inner, kind)?;
            check_expr(cond, ctx, scope, 0)
        }
        // F38: event trigger `->ev` — target harus signal/event yang dikenal.
        Stmt::EventTrigger(ev) => check_expr(ev, ctx, scope, 0),
        // F39: fork/join — tiap branch divalidasi di scope yang sama.
        Stmt::Fork { branches, .. } => {
            for b in branches {
                check_stmt(b, ctx, scope, kind)?;
            }
            Ok(())
        }
        // `foreach (arr[i]) { ... }` — arr harus sinyal dikenal; index var
        // otomatis lokal di dalam body.
        Stmt::Foreach { arr, inds, body } => {
            if !scope.known(arr) {
                return Err(err_at(
                    0,
                    0,
                    "E2001",
                    format!(
                        "undefined signal '{arr}' (foreach) — di '{}'",
                        scope.env.mname
                    ),
                ));
            }
            let mut inner = scope.clone();
            for iv in inds {
                inner.sigs.insert(iv.as_str());
            }
            inner.loop_depth += 1;
            check_stmt(body, ctx, &mut inner, kind)
        }
        Stmt::Repeat { count, body } => {
            check_expr(count, ctx, scope, 0)?;
            let mut inner = scope.clone();
            inner.loop_depth += 1;
            check_stmt(body, ctx, &mut inner, kind)
        }
        Stmt::Forever(body) => {
            let mut inner = scope.clone();
            inner.loop_depth += 1;
            check_stmt(body, ctx, &mut inner, kind)
        }
        Stmt::Wait { cond, body } => {
            check_expr(cond, ctx, scope, 0)?;
            check_stmt(body, ctx, scope, kind)
        }
        // F45: `wait fork;` — tanpa operand, selalu valid.
        Stmt::WaitFork => Ok(()),
        // F45: `disable fork;` / `disable <label>;` — konservatif: label
        // blok tidak dilacak checker (seperti assert property), selalu lolos.
        Stmt::Disable { .. } => Ok(()),
        // F46: `force lhs = rhs;` — blocking seperti `=` (E2004 di seq,
        // E2003 drive input, E2002 truncation).
        Stmt::Force { lhs, rhs, line, col } => {
            if kind == BlockKind::Seq {
                return Err(err_at(
                    *line,
                    *col,
                    "E2004",
                    format!(
                        "force tidak boleh di dalam seq (blocking) — di '{}'",
                        scope.env.mname
                    ),
                ));
            }
            if kind != BlockKind::Tb {
                if let Some(base) = base_ident(lhs) {
                    if let Some(Dir::In) = scope.env.ports.get(base) {
                        return Err(err_at(
                            *line,
                            *col,
                            "E2003",
                            format!(
                                "cannot drive input port '{base}' — di '{}'",
                                scope.env.mname
                            ),
                        ));
                    }
                }
            }
            check_expr(lhs, ctx, scope, 0)?;
            check_expr(rhs, ctx, scope, 0)?;
            let wl = super::expr::expr_width(lhs, ctx, scope, 0);
            let wr = super::expr::expr_width(rhs, ctx, scope, 0);
            if let (Some(l), Some(r)) = (wl, wr) {
                if r > l {
                    return Err(err_at(
                        *line,
                        *col,
                        "E2002",
                        format!(
                            "lebar {r} bit ke sinyal {l}-bit '{}' — di '{}'",
                            describe_lhs(lhs),
                            scope.env.mname
                        ),
                    ));
                }
            }
            Ok(())
        }
        // F46: `release target;` — target harus ekspresi dikenal.
        Stmt::Release { target } => check_expr(target, ctx, scope, 0),
        Stmt::Event { expr, body } => {
            check_expr(expr, ctx, scope, 0)?;
            if let Some(b) = body {
                check_stmt(b, ctx, scope, kind)?;
            }
            Ok(())
        }
        Stmt::Delay { amt, body } => {
            check_expr(amt, ctx, scope, 0)?;
            check_stmt(body, ctx, scope, kind)
        }
        Stmt::ExprStmt(e) => check_expr(e, ctx, scope, 0),
        Stmt::VarDecl {
            names,
            ty,
            init,
            line,
            col,
        } => {
            super::expr::check_type_scope(ty, ctx, Some(scope), 0)?;
            if let Some(i) = init {
                check_expr(i, ctx, scope, 0)?;
            }
            for n in names {
                if scope.sigs.contains(n.as_str()) {
                    return Err(err_at(
                        *line,
                        *col,
                        "E2007",
                        format!("variabel '{n}' sudah dideklarasikan di scope ini"),
                    ));
                }
                scope.sigs.insert(n.as_str());
                scope.types.insert(n.as_str(), ty);
            }
            Ok(())
        }
        Stmt::Return(v, line, col) => {
            if let Some(v) = v {
                if scope.in_task {
                    return Err(err_at(
                        *line,
                        *col,
                        "E2008",
                        "task tidak boleh mengembalikan nilai (return expr) — \
                         pakai `func` kalau butuh nilai balik"
                            .to_string(),
                    ));
                }
                check_expr(v, ctx, scope, 0)?;
            }
            Ok(())
        }
        Stmt::Break(line, col) | Stmt::Continue(line, col) => {
            if scope.loop_depth == 0 {
                return Err(err_at(
                    *line,
                    *col,
                    "E2009",
                    "break/continue hanya boleh di dalam loop".to_string(),
                ));
            }
            Ok(())
        }
        Stmt::Assert { cond, pass, fail } => {
            check_expr(cond, ctx, scope, 0)?;
            if let Some(p) = pass {
                check_stmt(p, ctx, scope, kind)?;
            }
            if let Some(f) = fail {
                check_stmt(f, ctx, scope, kind)?;
            }
            Ok(())
        }
        // `assert property (...)` — body RAW, konservatif: isi tidak dianalisis.
        Stmt::AssertProperty(_) => Ok(()),
        Stmt::Assume { cond, pass, fail } => {
            check_expr(cond, ctx, scope, 0)?;
            if let Some(p) = pass {
                check_stmt(p, ctx, scope, kind)?;
            }
            if let Some(f) = fail {
                check_stmt(f, ctx, scope, kind)?;
            }
            Ok(())
        }
        // `assume property (...)` — mirror `assert property` (RAW, konservatif).
        Stmt::AssumeProperty(_) => Ok(()),
        Stmt::Cover { cond, pass } => {
            check_expr(cond, ctx, scope, 0)?;
            if let Some(p) = pass {
                check_stmt(p, ctx, scope, kind)?;
            }
            Ok(())
        }
        // `cover property (...)` — mirror `assert property` (RAW, konservatif).
        Stmt::CoverProperty(_) => Ok(()),
        // Escape hatch `@sv { ... }` — teks SV mentah, ditangani lexer/codegen;
        // check tidak menganalisis isi (konservatif, seperti assert property).
        Stmt::RawSvh(_) => Ok(()),
    }
}

pub(crate) fn base_ident(e: &Expr) -> Option<&str> {
    match e {
        Expr::Ident(s, ..) => Some(s),
        Expr::Member(o, ..) => base_ident(o),
        Expr::Index(o, _) => base_ident(o),
        Expr::Range(o, _, _) => base_ident(o),
        _ => None,
    }
}

pub(crate) fn describe_lhs(e: &Expr) -> String {
    match e {
        Expr::Ident(s, ..) => s.clone(),
        Expr::Member(o, f, ..) => format!("{}.{}", describe_lhs(o), f),
        Expr::Index(o, i) => format!("{}[{}]", describe_lhs(o), describe_lhs(i)),
        Expr::Range(o, a, b) => format!(
            "{}[{}:{}]",
            describe_lhs(o),
            describe_lhs(a),
            describe_lhs(b)
        ),
        Expr::PartSelect {
            base,
            from,
            width,
            plus,
        } => format!(
            "{}[{} {} {}]",
            describe_lhs(base),
            describe_lhs(from),
            if *plus { "+:" } else { "-:" },
            describe_lhs(width)
        ),
        other => format!("{other:?}"),
    }
}

/// E2010: LHS tak boleh menunjuk `parameter` module atau `const` module.
///
/// SV menandai keduanya `localparam` (konstanta waktu-elipsi) — menulisnya
/// adalah illegal (LRM 1800 §6.20 "shall be illegal"). Sebelumnya hanya
/// aturan "jangan drive input port" (E2003) yang ada, jadi
/// `WIDTH = 8` di dalam `seq` lolos type-check lalu menghasilkan SV yang
/// ditolak elaborator.
pub(crate) fn check_lvalue_not_const(
    lhs: &Expr,
    scope: &Scope,
    line: usize,
    col: usize,
) -> Result<(), MvError> {
    let Some(base) = base_ident(lhs) else { return Ok(()) };
    // `scope.params` berisi parameter module DAN konstanta module yang
    // ter-fold (lihat `check/module.rs` MItem::Const) — keduanya immutable.
    if scope.params.contains_key(base) || scope.local_consts.contains(base) {
        return Err(err_at(
            line,
            col,
            "E2010",
            format!(
                "'{base}' adalah konstanta (parameter/const) — tidak bisa \
                 di-assign (LRM 1800 §6.20). buat sinyal terpisah jika perlu \
                 nilai yang bisa berubah"
            ),
        ));
    }
    Ok(())
}
