//! Codegen — statement SV: emit_stmt, emit_body, else-chain, compaction
//! single-line (repeat/forever/@/#/assert), escape hatch `@sv` raw.
//! 1 file = 1 tanggung jawab.

use super::expr::emit_expr;
use super::expr::emit_inside_item;
use super::line;
use crate::ast::*;

pub(crate) fn emit_stmt(out: &mut String, indent: usize, stmt: &Stmt) {
    match stmt {
        Stmt::Block(stmts) => {
            line(out, indent, "begin");
            for s in stmts {
                emit_stmt(out, indent + 1, s);
            }
            line(out, indent, "end");
        }
        // F47: `label : { ... }` → `begin : label ... end` (target `disable`).
        Stmt::NamedBlock { name, stmts } => {
            line(out, indent, &format!("begin : {name}"));
            for s in stmts {
                emit_stmt(out, indent + 1, s);
            }
            line(out, indent, "end");
        }
        Stmt::Assign { lhs, rhs, nba, .. } => {
            let op = if *nba { "<=" } else { "=" };
            line(
                out,
                indent,
                &format!("{} {op} {};", emit_expr(lhs), emit_expr(rhs)),
            );
        }
        Stmt::CompoundAssign { lhs, op, rhs, .. } => {
            line(
                out,
                indent,
                &format!("{} {op} {};", emit_expr(lhs), emit_expr(rhs)),
            );
        }
        Stmt::IncDec { lhs, inc, pre, .. } => {
            let op = if *inc { "++" } else { "--" };
            if *pre {
                line(out, indent, &format!("{}{};", op, emit_expr(lhs)));
            } else {
                line(out, indent, &format!("{}{};", emit_expr(lhs), op));
            }
        }
        Stmt::If { cond, then, els } => {
            line(out, indent, &format!("if ({}) begin", emit_expr(cond)));
            emit_body(out, indent + 1, then);
            emit_else_chain(out, indent, els.as_deref());
        }
        Stmt::Case {
            expr,
            items,
            default,
            qual,
            kind,
            ..
        } => {
            let kw = match kind.as_str() {
                "casez" => "casez",
                "casex" => "casex",
                _ => "case",
            };
            let head = match qual {
                Some(q) => format!("{} {}", q, kw),
                None => kw.to_string(),
            };
            line(out, indent, &format!("{} ({})", head, emit_expr(expr)));
            for (vals, body) in items {
                let vs: Vec<String> = vals.iter().map(emit_expr).collect();
                line(out, indent + 1, &format!("{}: begin", vs.join(", ")));
                emit_body(out, indent + 2, body);
                line(out, indent + 1, "end");
            }
            if let Some(d) = default {
                line(out, indent + 1, "default: begin");
                emit_body(out, indent + 2, d);
                line(out, indent + 1, "end");
            }
            line(out, indent, "endcase");
        }
        Stmt::CaseInside {
            expr,
            items,
            default,
            qual,
            ..
        } => {
            let head = match qual {
                Some(q) => format!("{} case", q),
                None => "case".to_string(),
            };
            line(out, indent, &format!("{} ({}) inside", head, emit_expr(expr)));
            for (vals, body) in items {
                let vs: Vec<String> = vals.iter().map(emit_inside_item).collect();
                line(out, indent + 1, &format!("{}: begin", vs.join(", ")));
                emit_body(out, indent + 2, body);
                line(out, indent + 1, "end");
            }
            if let Some(d) = default {
                line(out, indent + 1, "default: begin");
                emit_body(out, indent + 2, d);
                line(out, indent + 1, "end");
            }
            line(out, indent, "endcase");
        }
        Stmt::For {
            var,
            from,
            to,
            step,
            body,
        } => {
            line(
                out,
                indent,
                &format!(
                    "for (int {var} = {}; {var} < {}; {var} = {}) begin",
                    emit_expr(from),
                    emit_expr(to),
                    super::for_inc(var, step.as_ref())
                ),
            );
            emit_body(out, indent + 1, body);
            line(out, indent, "end");
        }
        Stmt::While { cond, body } => {
            line(out, indent, &format!("while ({}) begin", emit_expr(cond)));
            emit_body(out, indent + 1, body);
            line(out, indent, "end");
        }
        Stmt::DoWhile { cond, body } => {
            line(out, indent, "do begin");
            emit_body(out, indent + 1, body);
            line(out, indent, &format!("end while ({});", emit_expr(cond)));
        }
        Stmt::EventTrigger(ev) => {
            line(out, indent, &format!("-> {};", emit_expr(ev)));
        }
        Stmt::Fork { branches, join } => {
            line(out, indent, "fork");
            for b in branches {
                line(out, indent + 1, "begin");
                emit_body(out, indent + 2, b);
                line(out, indent + 1, "end");
            }
            let j = match join {
                ForkJoin::Join => "join",
                ForkJoin::JoinAny => "join_any",
                ForkJoin::JoinNone => "join_none",
            };
            line(out, indent, j);
        }
        Stmt::Foreach { arr, inds, body } => {
            let dims: String = inds.iter().map(|iv| format!("[{iv}]")).collect();
            if let Some(s) = single_line_stmt(body) {
                line(out, indent, &format!("foreach ({arr}{dims}) {s}"));
            } else {
                line(out, indent, &format!("foreach ({arr}{dims}) begin"));
                emit_body(out, indent + 1, body);
                line(out, indent, "end");
            }
        }
        Stmt::Repeat { count, body } => {
            if let Some(s) = single_line_stmt(body) {
                line(out, indent, &format!("repeat ({}) {s}", emit_expr(count)));
            } else {
                line(out, indent, &format!("repeat ({}) begin", emit_expr(count)));
                emit_body(out, indent + 1, body);
                line(out, indent, "end");
            }
        }
        Stmt::Forever(body) => {
            if let Some(s) = single_line_stmt(body) {
                line(out, indent, &format!("forever {s}"));
            } else {
                line(out, indent, "forever begin");
                emit_body(out, indent + 1, body);
                line(out, indent, "end");
            }
        }
        Stmt::Wait { cond, body } => {
            line(out, indent, &format!("wait ({}) begin", emit_expr(cond)));
            emit_body(out, indent + 1, body);
            line(out, indent, "end");
        }
        Stmt::WaitFork => {
            line(out, indent, "wait fork;");
        }
        Stmt::Disable { name } => {
            line(out, indent, &format!("disable {name};"));
        }
        Stmt::Force { lhs, rhs, .. } => {
            line(
                out,
                indent,
                &format!("force {} = {};", emit_expr(lhs), emit_expr(rhs)),
            );
        }
        Stmt::Release { target } => {
            line(out, indent, &format!("release {};", emit_expr(target)));
        }
        Stmt::Event { expr, body } => {
            match body {
                Some(b) => {
                    if let Some(s) = single_line_stmt(b) {
                        line(out, indent, &format!("@({}) {s}", emit_expr(expr)));
                    } else {
                        line(out, indent, &format!("@({}) begin", emit_expr(expr)));
                        emit_body(out, indent + 1, b);
                        line(out, indent, "end");
                    }
                }
                None => {
                    line(out, indent, &format!("@({});", emit_expr(expr)));
                }
            }
        }
        Stmt::Delay { amt, body } => {
            if let Some(s) = single_line_stmt(body) {
                line(out, indent, &format!("#{} {s}", emit_expr(amt)));
            } else if matches!(body.as_ref(), Stmt::Block(v) if v.is_empty()) {
                line(out, indent, &format!("#{};", emit_expr(amt)));
            } else {
                line(out, indent, &format!("#{} begin", emit_expr(amt)));
                emit_body(out, indent + 1, body);
                line(out, indent, "end");
            }
        }
        Stmt::ExprStmt(e) => {
            line(out, indent, &format!("{};", emit_expr(e)));
        }
        Stmt::Return(v, ..) => match v {
            Some(e) => line(out, indent, &format!("return {};", emit_expr(e))),
            None => line(out, indent, "return;"),
        },
        Stmt::Break(..) => line(out, indent, "break;"),
        Stmt::Continue(..) => line(out, indent, "continue;"),
        Stmt::VarDecl {
            names, ty, init, ..
        } => {
            let init_s = super::emit_init(init);
            line(
                out,
                indent,
                &format!("{}{};", super::emit_signal_decl_multi(ty, names), init_s),
            );
        }
        Stmt::Assert { cond, pass, fail } => {
            if let Some(s) = single_line_stmt(stmt) {
                line(out, indent, &s);
            } else {
                line(out, indent, &format!("assert ({})", emit_expr(cond)));
                if let Some(p) = pass {
                    emit_stmt(out, indent + 1, p);
                }
                if let Some(f) = fail {
                    line(out, indent, "else");
                    emit_stmt(out, indent + 1, f);
                }
                line(out, indent, ";");
            }
        }
        // Catatan: blok `begin…end` sudah membawa `;` sendiri lewat
        // `emit_stmt` masing-masing branch, jadi `assert (c) begin … end
        // else begin … end;` sah (LRM 1800 §20.11).
        Stmt::AssertProperty(raw) => {
            line(out, indent, &format!("assert property {raw};"));
        }
        Stmt::Assume { cond, pass, fail } => {
            if let Some(s) = single_line_stmt(stmt) {
                line(out, indent, &s);
            } else {
                line(out, indent, &format!("assume ({})", emit_expr(cond)));
                if let Some(p) = pass {
                    emit_stmt(out, indent + 1, p);
                }
                if let Some(f) = fail {
                    line(out, indent, "else");
                    emit_stmt(out, indent + 1, f);
                }
                line(out, indent, ";");
            }
        }
        // Catatan: blok `begin…end` sudah membawa `;` sendiri lewat
        // `emit_stmt` masing-masing branch, jadi `assume (c) begin … end
        // else begin … end;` sah (LRM 1800 §20.11).
        Stmt::AssumeProperty(raw) => {
            line(out, indent, &format!("assume property {raw};"));
        }
        Stmt::Cover { cond, pass } => {
            if let Some(s) = single_line_stmt(stmt) {
                line(out, indent, &s);
            } else {
                line(out, indent, &format!("cover ({})", emit_expr(cond)));
                if let Some(p) = pass {
                    emit_stmt(out, indent + 1, p);
                }
                line(out, indent, ";");
            }
        }
        Stmt::CoverProperty(raw) => {
            line(out, indent, &format!("cover property {raw};"));
        }
        // Escape hatch `@sv { ... }` — emit body SV mentah verbatim.
        Stmt::RawSvh(text) => emit_raw(out, indent, text),
    }
}

/// Emit teks SV mentah dari `@sv { ... }`: tiap baris di-indent seragam
/// `indent` (baris kosong tetap kosong). Deterministik.
fn emit_raw(out: &mut String, indent: usize, text: &str) {
    let pad = "    ".repeat(indent);
    for ln in text.split('\n') {
        if ln.trim().is_empty() {
            out.push('\n');
        } else {
            out.push_str(&pad);
            out.push_str(ln);
            out.push('\n');
        }
    }
}

/// Statement sederhana yang bisa di-emit satu baris (tanpa begin/end) —
/// dipakai untuk `assert (...)` dan body `@(...)`/`#amt` yang pendek.
pub(crate) fn single_line_stmt(stmt: &Stmt) -> Option<String> {
    match stmt {
        Stmt::Assign { lhs, rhs, nba, .. } => {
            let op = if *nba { "<=" } else { "=" };
            Some(format!("{} {op} {};", emit_expr(lhs), emit_expr(rhs)))
        }
        Stmt::CompoundAssign { lhs, op, rhs, .. } => {
            Some(format!("{} {op} {};", emit_expr(lhs), emit_expr(rhs)))
        }
        Stmt::IncDec { lhs, inc, pre, .. } => {
            let op = if *inc { "++" } else { "--" };
            if *pre {
                Some(format!("{}{};", op, emit_expr(lhs)))
            } else {
                Some(format!("{}{};", emit_expr(lhs), op))
            }
        }
        Stmt::ExprStmt(e) => Some(format!("{};", emit_expr(e))),
        Stmt::DoWhile { cond, body } => {
            single_line_stmt(body).map(|s| format!("do {s} while ({});", emit_expr(cond)))
        }
        Stmt::EventTrigger(ev) => Some(format!("-> {};", emit_expr(ev))),
        Stmt::WaitFork => Some("wait fork;".to_string()),
        Stmt::Disable { name } => Some(format!("disable {name};")),
        Stmt::Force { lhs, rhs, .. } => {
            Some(format!("force {} = {};", emit_expr(lhs), emit_expr(rhs)))
        }
        Stmt::Release { target } => Some(format!("release {};", emit_expr(target))),
        Stmt::AssertProperty(raw) => Some(format!("assert property {raw};")),
        Stmt::AssumeProperty(raw) => Some(format!("assume property {raw};")),
        Stmt::CoverProperty(raw) => Some(format!("cover property {raw};")),
        Stmt::Event { expr, body } => {
            match body {
                Some(b) => {
                    // Event control statement: @(expr) body — selalu single-line
                    // (SV memperbolehkan @(event) statement dengan statement apa pun).
                    // Emit body tanpa bungkus begin/end ekstra untuk Event.
                    let mut buf = String::new();
                    buf.push_str(&format!("@({}) ", emit_expr(expr)));
                    // Cek apakah body adalah empty block
                    if let Stmt::Block(stmts) = &**b {
                        if stmts.is_empty() {
                            // Empty block → null statement `;` agar event control
                            // tidak "mencuri" statement berikutnya di SV.
                            buf.push(';');
                            return Some(buf);
                        }
                    }
                    // Gunakan emit_body dengan indent 0 agar block body dicetak
                    // tanpa indent tambahan (Event sendiri tidak perlu begin/end).
                    emit_body(&mut buf, 0, b);
                    Some(buf.trim_end().to_string())
                }
                None => Some(format!("@({});", emit_expr(expr))),
            }
        }
        Stmt::Delay { amt, body } => {
            single_line_stmt(body).map(|s| format!("#{} {s}", emit_expr(amt)))
        }
        Stmt::Assert { cond, pass, fail } => {
            let p = match pass.as_ref().map(|s| assert_branch_stmt(s)) {
                Some(Some(s)) => Some(s),
                Some(None) => return None,
                None => None,
            };
            let f = match fail.as_ref().map(|s| assert_branch_stmt(s)) {
                Some(Some(s)) => Some(s),
                Some(None) => return None,
                None => None,
            };
            let c = emit_expr(cond);
            // LRM 1800 §20.11: `action_block_or_null` adalah STATEMENT (butuh
            // `;`), `null_action` juga `;`. Branch pass karena itu WAJIB
            // diakhiri `;` sebelum `else` — tanpa itu
            // `assert (c) $info("ok") else $error("bad");` bukan grammar SV
            // (verilator: "unexpected else, expecting ';'").
            match (p, f) {
                (Some(p), Some(f)) => Some(format!("assert ({c}) {p}; else {f};")),
                (Some(p), None) => Some(format!("assert ({c}) {p};")),
                (None, Some(f)) => Some(format!("assert ({c}) else {f};")),
                (None, None) => Some(format!("assert ({c});")),
            }
        }
        Stmt::Assume { cond, pass, fail } => {
            let p = match pass.as_ref().map(|s| assert_branch_stmt(s)) {
                Some(Some(s)) => Some(s),
                Some(None) => return None,
                None => None,
            };
            let f = match fail.as_ref().map(|s| assert_branch_stmt(s)) {
                Some(Some(s)) => Some(s),
                Some(None) => return None,
                None => None,
            };
            let c = emit_expr(cond);
            // Sama seperti `assert` (LRM 1800 §20.11): branch pass wajib `;`.
            match (p, f) {
                (Some(p), Some(f)) => Some(format!("assume ({c}) {p}; else {f};")),
                (Some(p), None) => Some(format!("assume ({c}) {p};")),
                (None, Some(f)) => Some(format!("assume ({c}) else {f};")),
                (None, None) => Some(format!("assume ({c});")),
            }
        }
        Stmt::Cover { cond, pass } => {
            let p = match pass.as_ref().map(|s| assert_branch_stmt(s)) {
                Some(Some(s)) => Some(s),
                Some(None) => return None,
                None => None,
            };
            let c = emit_expr(cond);
            // `cover` tanpa `else`: `cover (c) action;` / `cover (c);`.
            match p {
                Some(p) => Some(format!("cover ({c}) {p};")),
                None => Some(format!("cover ({c});")),
            }
        }
        _ => None,
    }
}

/// Branch pass/fail assertion yang aman di-compact tanpa semicolon di antara
/// branch. Call-like saja (`$info(...)`, `assert/assume/cover property`,
/// nested `assert`/`assume`/`cover`).
fn assert_branch_stmt(stmt: &Stmt) -> Option<String> {
    match stmt {
        Stmt::ExprStmt(e) => Some(emit_expr(e)),
        Stmt::AssertProperty(raw) => Some(format!("assert property {raw}")),
        Stmt::AssumeProperty(raw) => Some(format!("assume property {raw}")),
        Stmt::CoverProperty(raw) => Some(format!("cover property {raw}")),
        Stmt::Assert { .. } | Stmt::Assume { .. } | Stmt::Cover { .. } => {
            single_line_stmt(stmt).map(|s| s.trim_end_matches(';').to_string())
        }
        _ => None,
    }
}

/// Emit rantai `else if` / `else` — `end else if (c) begin` menyambung
/// langsung dari blok sebelumnya agar output satu blok utuh.
fn emit_else_chain(out: &mut String, indent: usize, els: Option<&Stmt>) {
    match els {
        Some(Stmt::If { cond, then, els }) => {
            line(
                out,
                indent,
                &format!("end else if ({}) begin", emit_expr(cond)),
            );
            emit_body(out, indent + 1, then);
            emit_else_chain(out, indent, els.as_deref());
        }
        Some(e) => {
            line(out, indent, "end else begin");
            emit_body(out, indent + 1, e);
            line(out, indent, "end");
        }
        None => {
            line(out, indent, "end");
        }
    }
}

/// Emit isi blok: `Stmt::Block` di-unwrap (tanpa begin/end ganda) karena
/// pemanggil sudah membuka `X begin`. Stmt lain di-emit apa adanya.
pub(crate) fn emit_body(out: &mut String, indent: usize, stmt: &Stmt) {
    match stmt {
        Stmt::Block(stmts) => {
            for s in stmts {
                emit_stmt(out, indent, s);
            }
        }
        other => emit_stmt(out, indent, other),
    }
}
