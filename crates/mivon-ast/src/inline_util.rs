use std::collections::{HashMap, HashSet};

use super::expr::Expr;
use super::stmt::Stmt;
use super::types::{DataType, Decl, FunctionDecl};
use mivon_core::intern::Symbol;
/// Deteksi SEMUA fungsi rekursif — termasuk rekursi TIDAK LANGSUNG
/// (mutual recursion: `a()` memanggil `b()`, `b()` memanggil `a()`, atau
/// siklus 3+ fungsi). Hanya mendeteksi direct recursion membuat inlining
/// `a → b → a → b → …` berjalan tanpa batas → stack overflow saat
/// `replace_func_calls_in_expr` pada project besar (OpenTitan: paket fungsi
/// `tlul_*`, `aes_*` saling memanggil).
///
/// Call graph = fungsi → semua fungsi yang dipanggil di body-nya (langsung
/// ataupun nested dalam statement/ekspresi). Fungsi dianggap rekursif bila ia
/// bisa mencapai dirinya sendiri melalui closure graf (DFS dari nama ke nama
/// itu sendiri) — mencakup direct self-call DAN mutual recursion.
pub(crate) fn detect_recursive_functions(funcs: &HashMap<Symbol, FunctionDecl>) -> HashSet<Symbol> {
    let mut recursive = HashSet::new();
    // Bangun call graph: nama → daftar fungsi yang dipanggil (incl. dirinya
    // sendiri untuk direct self-call). Performa: SATU traversal body per
    // fungsi, mengumpulkan SEMUA nama fungsi yang dipanggil (lihat
    // `collect_called_funcs`). Sebelumnya loop bersarang O(N²) memindai body
    // penuh untuk tiap pasangan (func, other) → sangat lambat di project
    // besar (OpenTitan: ribuan fungsi `tlul_*`/`aes_*`).
    let known: HashSet<Symbol> = funcs.keys().copied().collect();
    let mut calls: HashMap<Symbol, Vec<Symbol>> = HashMap::with_capacity(funcs.len());
    for (name, func) in funcs {
        let mut called = HashSet::new();
        collect_called_funcs(&func.stmts, &known, &mut called);
        calls.insert(*name, called.into_iter().collect());
    }
    // DFS dari setiap fungsi: apakah bisa kembali ke dirinya sendiri?
    for &start in funcs.keys() {
        let mut visited: HashSet<Symbol> = HashSet::new();
        if dfs_reaches_self(start, start, &calls, &mut visited) {
            recursive.insert(start);
        }
    }
    recursive
}

/// DFS: apakah `cur` bisa mencapai `start` (dirinya sendiri) melalui call graph?
fn dfs_reaches_self(
    start: Symbol,
    cur: Symbol,
    calls: &HashMap<Symbol, Vec<Symbol>>,
    visited: &mut HashSet<Symbol>,
) -> bool {
    if !visited.insert(cur) {
        return false;
    }
    if let Some(nexts) = calls.get(&cur) {
        for &next in nexts {
            if next == start {
                return true;
            }
            if dfs_reaches_self(start, next, calls, visited) {
                return true;
            }
        }
    }
    false
}

/// Kumpulkan SEMUA nama fungsi (dari `known`) yang dipanggil dalam `stmts`.
/// Satu traversal per function — pengganti pola O(N²) yang memanggil
/// `stmt_has_func_call(name, stmts)` untuk tiap nama. Mengisi `out`.
fn collect_called_funcs(stmts: &[Stmt], known: &HashSet<Symbol>, out: &mut HashSet<Symbol>) {
    for stmt in stmts {
        match stmt {
            Stmt::Block { stmts: inner }
            | Stmt::NamedBlock { stmts: inner, .. }
            | Stmt::LoopForever { stmts: inner }
            | Stmt::LoopWhile { stmts: inner, .. }
            | Stmt::LoopFor { stmts: inner, .. }
            | Stmt::Repeat { stmts: inner, .. }
            | Stmt::DoWhile { stmts: inner, .. } => {
                collect_called_funcs(inner, known, out);
            }
            Stmt::IfElse {
                cond,
                true_branch,
                false_branch,
            } => {
                collect_called_expr(cond, known, out);
                collect_called_funcs(&[true_branch.as_ref().clone()], known, out);
                if let Some(fb) = false_branch {
                    collect_called_funcs(&[fb.as_ref().clone()], known, out);
                }
            }
            Stmt::Case {
                expr,
                items,
                default,
            }
            | Stmt::CaseX {
                expr,
                items,
                default,
            }
            | Stmt::CaseZ {
                expr,
                items,
                default,
            }
            | Stmt::StmtCase {
                expr,
                items,
                default,
            }
            | Stmt::UniqueCase {
                expr,
                items,
                default,
                ..
            }
            | Stmt::PriorityCase {
                expr,
                items,
                default,
                ..
            }
            | Stmt::Unique0Case {
                expr,
                items,
                default,
                ..
            }
            | Stmt::CaseInside {
                expr,
                items,
                default,
            } => {
                collect_called_expr(expr, known, out);
                for item in items {
                    for l in &item.labels {
                        collect_called_expr(l, known, out);
                    }
                    collect_called_funcs(&[item.stmt.as_ref().clone()], known, out);
                }
                if let Some(d) = default {
                    collect_called_funcs(&[d.as_ref().clone()], known, out);
                }
            }
            Stmt::BlockingAssign { rhs, .. } | Stmt::NonBlockingAssign { rhs, .. } => {
                collect_called_expr(rhs, known, out);
            }
            Stmt::StmtAssign { lhs, rhs } => {
                collect_called_expr(lhs, known, out);
                collect_called_expr(rhs, known, out);
            }
            Stmt::Expr { expr } => {
                collect_called_expr(expr, known, out);
            }
            Stmt::Return(Some(e)) => {
                collect_called_expr(e, known, out);
            }
            Stmt::Wait { cond, stmt: wstmt } => {
                collect_called_expr(cond, known, out);
                if let Some(s) = wstmt {
                    collect_called_funcs(&[*s.clone()], known, out);
                }
            }
            Stmt::WaitFork => {}
            Stmt::SysCall { args, .. } => {
                for arg in args {
                    collect_called_expr(arg, known, out);
                }
            }
            Stmt::Fork { processes, .. } => {
                for p in processes {
                    collect_called_funcs(std::slice::from_ref(p), known, out);
                }
            }
            Stmt::Force { rhs, .. } => {
                collect_called_expr(rhs, known, out);
            }
            _ => {}
        }
    }
}

/// Kumpulkan SEMUA nama fungsi (dari `known`) yang dipanggil dalam `expr`.
/// Mengisi `out`.
fn collect_called_expr(expr: &Expr, known: &HashSet<Symbol>, out: &mut HashSet<Symbol>) {
    match expr {
        Expr::FuncCall { name, args, .. } => {
            if known.contains(name) {
                out.insert(*name);
            }
            for arg in args {
                collect_called_expr(arg, known, out);
            }
        }
        Expr::BinaryOp { lhs, rhs, .. } => {
            collect_called_expr(lhs, known, out);
            collect_called_expr(rhs, known, out);
        }
        Expr::UnaryOp { expr: inner, .. } => collect_called_expr(inner, known, out),
        Expr::TernaryOp {
            cond,
            true_expr,
            false_expr,
        } => {
            collect_called_expr(cond, known, out);
            collect_called_expr(true_expr, known, out);
            collect_called_expr(false_expr, known, out);
        }
        Expr::Concat(exprs) => {
            for e in exprs {
                collect_called_expr(e, known, out);
            }
        }
        Expr::Replicate { expr: inner, .. } => collect_called_expr(inner, known, out),
        Expr::Paren(inner) => collect_called_expr(inner, known, out),
        Expr::IncDec { expr: inner, .. } => collect_called_expr(inner, known, out),
        Expr::RangeSelect {
            expr: inner,
            msb,
            lsb,
        } => {
            collect_called_expr(inner, known, out);
            collect_called_expr(msb, known, out);
            collect_called_expr(lsb, known, out);
        }
        Expr::BitSelect { expr: inner, index } => {
            collect_called_expr(inner, known, out);
            collect_called_expr(index, known, out);
        }
        Expr::PartSelect {
            expr: inner,
            base,
            width,
        } => {
            collect_called_expr(inner, known, out);
            collect_called_expr(base, known, out);
            collect_called_expr(width, known, out);
        }
        _ => {}
    }
}

pub(crate) fn func_port_width(func: &FunctionDecl, port_name: Symbol) -> usize {
    if let Some(port) = func.ports.iter().find(|p| p.name == port_name) {
        if let Some(r) = &port.range {
            return r.width();
        }
        // `logic [7:0] in` menyimpan range di expr_range (bukan range)
        // bila batasnya ekspresi/konstanta yang belum di-fold saat parse.
        if let Some(er) = &port.expr_range {
            if let (Ok(msb), Ok(lsb)) = (
                super::types::const_eval_simple(&er.msb),
                super::types::const_eval_simple(&er.lsb),
            ) {
                let width = if msb >= lsb {
                    (msb - lsb + 1) as usize
                } else {
                    (lsb - msb + 1) as usize
                };
                if width > 0 {
                    return width;
                }
            }
        }
    }
    for decl in &func.decls {
        for var in &decl.names {
            if var.name == port_name {
                if let Some(r) = &var.range {
                    return r.width();
                }
                if let Some(er) = &var.expr_range {
                    if let (Ok(msb), Ok(lsb)) = (
                        super::types::const_eval_simple(&er.msb),
                        super::types::const_eval_simple(&er.lsb),
                    ) {
                        let width = if msb >= lsb {
                            (msb - lsb + 1) as usize
                        } else {
                            (lsb - msb + 1) as usize
                        };
                        if width > 0 {
                            return width;
                        }
                    }
                }
                return 1;
            }
        }
    }
    // Port has no range and no matching decl — likely user-defined type (struct/enum)
    // Use a safe default width (64) to avoid width mismatch issues during simulation
    let known_builtin = func
        .ports
        .iter()
        .any(|p| p.name == port_name && p.range.is_none());
    if known_builtin {
        1
    } else {
        64
    }
}

pub(crate) fn func_return_width(func: &FunctionDecl) -> usize {
    if let Some(er) = &func.range {
        if let (Ok(msb), Ok(lsb)) = (
            super::types::const_eval_simple(&er.msb),
            super::types::const_eval_simple(&er.lsb),
        ) {
            let msb = msb as usize;
            let lsb = lsb as usize;
            return if msb >= lsb {
                msb - lsb + 1
            } else {
                lsb - msb + 1
            };
        }
    }
    match &func.return_type {
        Some(inner) => match inner.as_ref() {
            DataType::Void => 0,
            DataType::Byte => 8,
            DataType::Shortint => 16,
            DataType::Int | DataType::Integer => 32,
            DataType::Longint => 64,
            DataType::Time => 64,
            DataType::Signed(s) => match s.as_ref() {
                DataType::Bit => 1,
                DataType::Logic => 1,
                DataType::Byte => 8,
                DataType::Shortint => 16,
                DataType::Int | DataType::Integer => 32,
                DataType::Longint => 64,
                DataType::Time => 64,
                _ => 1,
            },
            _ => 1,
        },
        _ => 1,
    }
}

pub(crate) fn rename_in_stmt(stmt: &Stmt, rename_map: &HashMap<Symbol, Symbol>) -> Stmt {
    match stmt.clone() {
        Stmt::Block { stmts } => Stmt::Block {
            stmts: stmts
                .iter()
                .map(|s| rename_in_stmt(s, rename_map))
                .collect(),
        },
        Stmt::NamedBlock { name, stmts, decls } => Stmt::NamedBlock {
            name,
            stmts: stmts
                .iter()
                .map(|s| rename_in_stmt(s, rename_map))
                .collect(),
            decls,
        },
        Stmt::IfElse {
            cond,
            true_branch,
            false_branch,
        } => Stmt::IfElse {
            cond: rename_in_expr(cond, rename_map),
            true_branch: Box::new(rename_in_stmt(&true_branch, rename_map)),
            false_branch: false_branch.map(|fb| Box::new(rename_in_stmt(&fb, rename_map))),
        },
        Stmt::Case {
            expr,
            items,
            default,
        } => Stmt::Case {
            expr: rename_in_expr(expr, rename_map),
            items: items
                .iter()
                .map(|item| super::stmt::CaseItem {
                    labels: item
                        .labels
                        .iter()
                        .map(|l| rename_in_expr(l.clone(), rename_map))
                        .collect(),
                    stmt: Box::new(rename_in_stmt(&item.stmt, rename_map)),
                })
                .collect(),
            default: default.map(|d| Box::new(rename_in_stmt(&d, rename_map))),
        },
        Stmt::CaseX {
            expr,
            items,
            default,
        } => Stmt::CaseX {
            expr: rename_in_expr(expr, rename_map),
            items: items
                .iter()
                .map(|item| super::stmt::CaseItem {
                    labels: item
                        .labels
                        .iter()
                        .map(|l| rename_in_expr(l.clone(), rename_map))
                        .collect(),
                    stmt: Box::new(rename_in_stmt(&item.stmt, rename_map)),
                })
                .collect(),
            default: default.map(|d| Box::new(rename_in_stmt(&d, rename_map))),
        },
        Stmt::CaseZ {
            expr,
            items,
            default,
        } => Stmt::CaseZ {
            expr: rename_in_expr(expr, rename_map),
            items: items
                .iter()
                .map(|item| super::stmt::CaseItem {
                    labels: item
                        .labels
                        .iter()
                        .map(|l| rename_in_expr(l.clone(), rename_map))
                        .collect(),
                    stmt: Box::new(rename_in_stmt(&item.stmt, rename_map)),
                })
                .collect(),
            default: default.map(|d| Box::new(rename_in_stmt(&d, rename_map))),
        },
        Stmt::LoopForever { stmts } => Stmt::LoopForever {
            stmts: stmts
                .iter()
                .map(|s| rename_in_stmt(s, rename_map))
                .collect(),
        },
        Stmt::LoopWhile { cond, stmts } => Stmt::LoopWhile {
            cond: rename_in_expr(cond, rename_map),
            stmts: stmts
                .iter()
                .map(|s| rename_in_stmt(s, rename_map))
                .collect(),
        },
        Stmt::LoopFor {
            init,
            cond,
            step,
            stmts,
        } => Stmt::LoopFor {
            init: init.map(|i| Box::new(rename_in_stmt(&i, rename_map))),
            cond: cond.map(|c| rename_in_expr(c, rename_map)),
            step: step.map(|s| Box::new(rename_in_stmt(&s, rename_map))),
            stmts: stmts
                .iter()
                .map(|s| rename_in_stmt(s, rename_map))
                .collect(),
        },
        Stmt::Repeat { count, stmts } => Stmt::Repeat {
            count: rename_in_expr(count, rename_map),
            stmts: stmts
                .iter()
                .map(|s| rename_in_stmt(s, rename_map))
                .collect(),
        },
        Stmt::BlockingAssign { lhs, rhs, delay } => Stmt::BlockingAssign {
            lhs: rename_in_expr(lhs, rename_map),
            rhs: rename_in_expr(rhs, rename_map),
            delay,
        },
        Stmt::NonBlockingAssign { lhs, rhs, delay } => Stmt::NonBlockingAssign {
            lhs: rename_in_expr(lhs, rename_map),
            rhs: rename_in_expr(rhs, rename_map),
            delay,
        },
        Stmt::StmtAssign { lhs, rhs } => Stmt::StmtAssign {
            lhs: rename_in_expr(lhs, rename_map),
            rhs: rename_in_expr(rhs, rename_map),
        },
        Stmt::StmtCase {
            expr,
            items,
            default,
        } => Stmt::StmtCase {
            expr: rename_in_expr(expr, rename_map),
            items: items
                .iter()
                .map(|item| super::stmt::CaseItem {
                    labels: item
                        .labels
                        .iter()
                        .map(|l| rename_in_expr(l.clone(), rename_map))
                        .collect(),
                    stmt: Box::new(rename_in_stmt(&item.stmt, rename_map)),
                })
                .collect(),
            default: default.map(|d| Box::new(rename_in_stmt(&d, rename_map))),
        },
        Stmt::SysCall {
            name,
            args,
            line,
            col,
        } => Stmt::SysCall {
            name,
            args: args
                .into_iter()
                .map(|a| rename_in_expr(a, rename_map))
                .collect(),
            line,
            col,
        },
        Stmt::SysFinish => Stmt::SysFinish,
        Stmt::Delay { delay, stmt } => Stmt::Delay {
            delay: rename_in_expr(delay, rename_map),
            stmt: Box::new(rename_in_stmt(&stmt, rename_map)),
        },
        Stmt::Disable { name } => Stmt::Disable { name },
        Stmt::Force { lhs, rhs } => Stmt::Force {
            lhs: rename_in_expr(lhs, rename_map),
            rhs: rename_in_expr(rhs, rename_map),
        },
        Stmt::Release { expr } => Stmt::Release {
            expr: rename_in_expr(expr, rename_map),
        },
        Stmt::Deassign { expr } => Stmt::Deassign {
            expr: rename_in_expr(expr, rename_map),
        },
        Stmt::Wait { cond, stmt } => Stmt::Wait {
            cond: rename_in_expr(cond, rename_map),
            stmt: stmt.map(|s| Box::new(rename_in_stmt(&s, rename_map))),
        },
        Stmt::WaitFork => Stmt::WaitFork,
        Stmt::EventControl { events, stmt } => Stmt::EventControl {
            events: events.clone(),
            stmt: stmt.map(|s| Box::new(rename_in_stmt(&s, rename_map))),
        },
        Stmt::EventTrigger { name } => Stmt::EventTrigger { name },
        Stmt::Expr { expr } => Stmt::Expr {
            expr: rename_in_expr(expr, rename_map),
        },
        Stmt::Null => Stmt::Null,
        Stmt::Return(expr) => {
            let renamed_expr = expr.map(|e| Box::new(rename_in_expr(*e, rename_map)));
            Stmt::Return(renamed_expr)
        }
        Stmt::ForeachLoop {
            array_var,
            index_vars,
            stmts,
        } => Stmt::ForeachLoop {
            // F44: array formal task/function (`ref queue d_in [$]`) — nama
            // array di-rename saat inline agar foreach tetap ter-resolve.
            array_var: rename_map.get(&array_var).copied().unwrap_or(array_var),
            index_vars,
            stmts: stmts
                .into_iter()
                .map(|s| rename_in_stmt(&s, rename_map))
                .collect(),
        },
        Stmt::Break => Stmt::Break,
        Stmt::Continue => Stmt::Continue,
        Stmt::DoWhile { cond, stmts } => Stmt::DoWhile {
            cond: rename_in_expr(cond, rename_map),
            stmts: stmts
                .into_iter()
                .map(|s| rename_in_stmt(&s, rename_map))
                .collect(),
        },
        Stmt::UniqueCase {
            expr,
            items,
            default,
            kind,
        } => Stmt::UniqueCase {
            expr: rename_in_expr(expr, rename_map),
            items: items
                .iter()
                .map(|item| super::stmt::CaseItem {
                    labels: item
                        .labels
                        .iter()
                        .map(|l| rename_in_expr(l.clone(), rename_map))
                        .collect(),
                    stmt: Box::new(rename_in_stmt(&item.stmt, rename_map)),
                })
                .collect(),
            default: default.map(|d| Box::new(rename_in_stmt(&d, rename_map))),
            kind,
        },
        Stmt::PriorityCase {
            expr,
            items,
            default,
            kind,
        } => Stmt::PriorityCase {
            expr: rename_in_expr(expr, rename_map),
            items: items
                .iter()
                .map(|item| super::stmt::CaseItem {
                    labels: item
                        .labels
                        .iter()
                        .map(|l| rename_in_expr(l.clone(), rename_map))
                        .collect(),
                    stmt: Box::new(rename_in_stmt(&item.stmt, rename_map)),
                })
                .collect(),
            default: default.map(|d| Box::new(rename_in_stmt(&d, rename_map))),
            kind,
        },
        Stmt::Unique0Case {
            expr,
            items,
            default,
            kind,
        } => Stmt::Unique0Case {
            expr: rename_in_expr(expr, rename_map),
            items: items
                .iter()
                .map(|item| super::stmt::CaseItem {
                    labels: item
                        .labels
                        .iter()
                        .map(|l| rename_in_expr(l.clone(), rename_map))
                        .collect(),
                    stmt: Box::new(rename_in_stmt(&item.stmt, rename_map)),
                })
                .collect(),
            default: default.map(|d| Box::new(rename_in_stmt(&d, rename_map))),
            kind,
        },
        Stmt::CaseInside {
            expr,
            items,
            default,
        } => Stmt::CaseInside {
            expr: rename_in_expr(expr, rename_map),
            items: items
                .iter()
                .map(|item| super::stmt::CaseItem {
                    labels: item
                        .labels
                        .iter()
                        .map(|l| rename_in_expr(l.clone(), rename_map))
                        .collect(),
                    stmt: Box::new(rename_in_stmt(&item.stmt, rename_map)),
                })
                .collect(),
            default: default.map(|d| Box::new(rename_in_stmt(&d, rename_map))),
        },
        Stmt::Assert {
            cond,
            pass_stmt,
            fail_stmt,
            ..
        } => Stmt::Assert {
            cond: rename_in_expr(cond, rename_map),
            pass_stmt: pass_stmt.map(|s| Box::new(rename_in_stmt(&s, rename_map))),
            fail_stmt: fail_stmt.map(|s| Box::new(rename_in_stmt(&s, rename_map))),
            clock_event: None,
            disable_iff: None,
        },
        Stmt::PropertySeq {
            sequence,
            pass_stmt,
            fail_stmt,
            clock_event,
            disable_iff,
        } => Stmt::PropertySeq {
            sequence: rename_in_sequence(sequence, rename_map),
            pass_stmt: pass_stmt.map(|s| Box::new(rename_in_stmt(&s, rename_map))),
            fail_stmt: fail_stmt.map(|s| Box::new(rename_in_stmt(&s, rename_map))),
            clock_event,
            disable_iff: disable_iff.map(|e| Box::new(rename_in_expr(*e, rename_map))),
        },
        Stmt::Assume {
            cond,
            pass_stmt,
            fail_stmt,
            ..
        } => Stmt::Assume {
            cond: rename_in_expr(cond, rename_map),
            pass_stmt: pass_stmt.map(|s| Box::new(rename_in_stmt(&s, rename_map))),
            fail_stmt: fail_stmt.map(|s| Box::new(rename_in_stmt(&s, rename_map))),
            clock_event: None,
            disable_iff: None,
        },
        Stmt::Cover {
            cond, pass_stmt, ..
        } => Stmt::Cover {
            cond: rename_in_expr(cond, rename_map),
            pass_stmt: pass_stmt.map(|s| Box::new(rename_in_stmt(&s, rename_map))),
            clock_event: None,
            disable_iff: None,
        },
        Stmt::Expect {
            cond,
            pass_stmt,
            fail_stmt,
        } => Stmt::Expect {
            cond: rename_in_expr(cond, rename_map),
            pass_stmt: pass_stmt.map(|s| Box::new(rename_in_stmt(&s, rename_map))),
            fail_stmt: fail_stmt.map(|s| Box::new(rename_in_stmt(&s, rename_map))),
        },
        Stmt::WaitOrder { events, fail_stmt } => Stmt::WaitOrder {
            events,
            fail_stmt: fail_stmt.map(|s| Box::new(rename_in_stmt(&s, rename_map))),
        },
        Stmt::UniqueIf {
            cond,
            true_branch,
            false_branch,
        } => Stmt::UniqueIf {
            cond: rename_in_expr(cond, rename_map),
            true_branch: Box::new(rename_in_stmt(&true_branch, rename_map)),
            false_branch: false_branch.map(|s| Box::new(rename_in_stmt(&s, rename_map))),
        },
        Stmt::PriorityIf {
            cond,
            true_branch,
            false_branch,
        } => Stmt::PriorityIf {
            cond: rename_in_expr(cond, rename_map),
            true_branch: Box::new(rename_in_stmt(&true_branch, rename_map)),
            false_branch: false_branch.map(|s| Box::new(rename_in_stmt(&s, rename_map))),
        },
        Stmt::Fork {
            processes,
            join_type,
        } => Stmt::Fork {
            processes: processes
                .into_iter()
                .map(|s| rename_in_stmt(&s, rename_map))
                .collect(),
            join_type,
        },
        Stmt::RandCase { items } => Stmt::RandCase {
            items: items
                .into_iter()
                .map(|rc| crate::stmt::RandCaseItem {
                    weight: rc.weight,
                    stmt: Box::new(rename_in_stmt(&rc.stmt, rename_map)),
                })
                .collect(),
        },
        Stmt::RandSequence { productions } => Stmt::RandSequence {
            productions: productions
                .into_iter()
                .map(|p| crate::stmt::RandSeqProduction {
                    name: p.name,
                    items: p
                        .items
                        .into_iter()
                        .map(|item| crate::stmt::RandSeqItem {
                            value: Box::new(rename_in_stmt(&item.value, rename_map)),
                            weight: item.weight,
                        })
                        .collect(),
                })
                .collect(),
        },
    }
}

pub(crate) fn rename_func_decls_in_stmt(stmt: Stmt, rename_map: &HashMap<Symbol, Symbol>) -> Stmt {
    match stmt {
        Stmt::NamedBlock { name, stmts, decls } => {
            let new_decls: Vec<Decl> = decls
                .into_iter()
                .map(|mut d| {
                    for var in &mut d.names {
                        if let Some(new_name) = rename_map.get(&var.name) {
                            var.name = *new_name;
                        }
                    }
                    d
                })
                .collect();
            let new_stmts = stmts
                .into_iter()
                .map(|s| rename_func_decls_in_stmt(s, rename_map))
                .collect();
            Stmt::NamedBlock {
                name,
                stmts: new_stmts,
                decls: new_decls,
            }
        }
        Stmt::Block { stmts } => Stmt::Block {
            stmts: stmts
                .into_iter()
                .map(|s| rename_func_decls_in_stmt(s, rename_map))
                .collect(),
        },
        other => other,
    }
}

pub(crate) fn rename_in_sequence(
    sequence: super::types::Sequence,
    rename_map: &HashMap<Symbol, Symbol>,
) -> super::types::Sequence {
    use super::types::Sequence;
    match sequence {
        Sequence::Expr(e) => Sequence::Expr(rename_in_expr(e, rename_map)),
        Sequence::Delay(n) => Sequence::Delay(n),
        Sequence::DelayRange(a, b) => Sequence::DelayRange(a, b),
        Sequence::Concat(l, r) => Sequence::Concat(
            Box::new(rename_in_sequence(*l, rename_map)),
            Box::new(rename_in_sequence(*r, rename_map)),
        ),
        Sequence::Or(l, r) => Sequence::Or(
            Box::new(rename_in_sequence(*l, rename_map)),
            Box::new(rename_in_sequence(*r, rename_map)),
        ),
        Sequence::And(l, r) => Sequence::And(
            Box::new(rename_in_sequence(*l, rename_map)),
            Box::new(rename_in_sequence(*r, rename_map)),
        ),
        Sequence::Repeat(s, n) => Sequence::Repeat(Box::new(rename_in_sequence(*s, rename_map)), n),
        Sequence::Implication(ante, cons) => Sequence::Implication(
            Box::new(rename_in_sequence(*ante, rename_map)),
            Box::new(rename_in_sequence(*cons, rename_map)),
        ),
    }
}

pub(crate) fn rename_in_expr(expr: Expr, rename_map: &HashMap<Symbol, Symbol>) -> Expr {
    match expr {
        Expr::Ident { name, .. } => rename_map.get(&name).map_or(
            Expr::Ident {
                name,
                line: 0,
                col: 0,
            },
            |n| Expr::Ident {
                name: *n,
                line: 0,
                col: 0,
            },
        ),
        Expr::BinaryOp { op, lhs, rhs } => Expr::BinaryOp {
            op,
            lhs: Box::new(rename_in_expr(*lhs, rename_map)),
            rhs: Box::new(rename_in_expr(*rhs, rename_map)),
        },
        Expr::UnaryOp { op, expr: inner } => Expr::UnaryOp {
            op,
            expr: Box::new(rename_in_expr(*inner, rename_map)),
        },
        Expr::TernaryOp {
            cond,
            true_expr,
            false_expr,
        } => Expr::TernaryOp {
            cond: Box::new(rename_in_expr(*cond, rename_map)),
            true_expr: Box::new(rename_in_expr(*true_expr, rename_map)),
            false_expr: Box::new(rename_in_expr(*false_expr, rename_map)),
        },
        Expr::Concat(exprs) => Expr::Concat(
            exprs
                .into_iter()
                .map(|e| rename_in_expr(e, rename_map))
                .collect(),
        ),
        Expr::Replicate { count, expr: inner } => Expr::Replicate {
            count: Box::new(rename_in_expr(*count, rename_map)),
            expr: Box::new(rename_in_expr(*inner, rename_map)),
        },
        Expr::Paren(inner) => Expr::Paren(Box::new(rename_in_expr(*inner, rename_map))),
        Expr::RangeSelect {
            expr: inner,
            msb,
            lsb,
        } => Expr::RangeSelect {
            expr: Box::new(rename_in_expr(*inner, rename_map)),
            msb: Box::new(rename_in_expr(*msb, rename_map)),
            lsb: Box::new(rename_in_expr(*lsb, rename_map)),
        },
        Expr::BitSelect { expr: inner, index } => Expr::BitSelect {
            expr: Box::new(rename_in_expr(*inner, rename_map)),
            index: Box::new(rename_in_expr(*index, rename_map)),
        },
        Expr::PartSelect {
            expr: inner,
            base,
            width,
        } => Expr::PartSelect {
            expr: Box::new(rename_in_expr(*inner, rename_map)),
            base: Box::new(rename_in_expr(*base, rename_map)),
            width: Box::new(rename_in_expr(*width, rename_map)),
        },
        Expr::FuncCall {
            name,
            args,
            line,
            col,
        } => Expr::FuncCall {
            name: rename_map.get(&name).cloned().unwrap_or(name),
            args: args
                .into_iter()
                .map(|a| rename_in_expr(a, rename_map))
                .collect(),
            line,
            col,
        },
        Expr::Inside {
            expr: inner,
            range_list,
        } => Expr::Inside {
            expr: Box::new(rename_in_expr(*inner, rename_map)),
            range_list: range_list
                .into_iter()
                .map(|e| rename_in_expr(e, rename_map))
                .collect(),
        },
        // Member access `payload.addr = tl.a` — rename obj-nya (payload → temp,
        // tl → argumen aktual) agar elaborasi bisa resolve struct_fields.
        Expr::MemberAccess { obj, field } => Expr::MemberAccess {
            obj: Box::new(rename_in_expr(*obj, rename_map)),
            field,
        },
        Expr::Cast { dtype, expr: inner } => Expr::Cast {
            dtype,
            expr: Box::new(rename_in_expr(*inner, rename_map)),
        },
        Expr::CastWidth { width, expr: inner } => Expr::CastWidth {
            width: Box::new(rename_in_expr(*width, rename_map)),
            expr: Box::new(rename_in_expr(*inner, rename_map)),
        },
        Expr::MethodCall {
            obj,
            method,
            args,
            with_clause,
        } => Expr::MethodCall {
            obj: Box::new(rename_in_expr(*obj, rename_map)),
            method,
            args: args
                .into_iter()
                .map(|a| rename_in_expr(a, rename_map))
                .collect(),
            with_clause: with_clause.map(|wc| Box::new(rename_in_expr(*wc, rename_map))),
        },
        Expr::StreamingConcat {
            op,
            slice_size,
            slices,
        } => Expr::StreamingConcat {
            op,
            slice_size: slice_size.map(|ss| Box::new(rename_in_expr(*ss, rename_map))),
            slices: slices
                .into_iter()
                .map(|e| rename_in_expr(e, rename_map))
                .collect(),
        },
        Expr::Dist { expr: inner, items } => Expr::Dist {
            expr: Box::new(rename_in_expr(*inner, rename_map)),
            items,
        },
        Expr::ScopedIdent { package, item, .. } => Expr::ScopedIdent {
            package,
            item,
            line: 0,
            col: 0,
        },
        Expr::Value(_) | Expr::FillLit(_) | Expr::String(_) | Expr::Null => expr,
        Expr::IncDec {
            expr: inner,
            inc,
            pre,
        } => Expr::IncDec {
            expr: Box::new(rename_in_expr(*inner, rename_map)),
            inc,
            pre,
        },
        other => other,
    }
}

/// LANG-40: substitusi ident parameter `let` dengan ekspresi argumen
/// (rekursif). Varian umum ditangani eksplisit; varian leaf/jarang (Value,
/// FillLit, String, ScopedIdent, StreamingConcat, CastWidth, Dist, StructLit)
/// dikembalikan apa adanya (let body tipikal berisi aritmatika/perbandingan/
/// select yang tercakup di sini). Dipakai elaborator (IR) dan evaluator
/// engine (jalur AST class method).
pub fn substitute_let_args(expr: Expr, map: &HashMap<Symbol, &Expr>) -> Expr {
    match expr {
        Expr::Ident { name, .. } => match map.get(&name) {
            Some(repl) => (*repl).clone(),
            None => Expr::Ident {
                name,
                line: 0,
                col: 0,
            },
        },
        Expr::RangeSelect { expr, msb, lsb } => Expr::RangeSelect {
            expr: Box::new(substitute_let_args(*expr, map)),
            msb: Box::new(substitute_let_args(*msb, map)),
            lsb: Box::new(substitute_let_args(*lsb, map)),
        },
        Expr::BitSelect { expr, index } => Expr::BitSelect {
            expr: Box::new(substitute_let_args(*expr, map)),
            index: Box::new(substitute_let_args(*index, map)),
        },
        Expr::PartSelect { expr, base, width } => Expr::PartSelect {
            expr: Box::new(substitute_let_args(*expr, map)),
            base: Box::new(substitute_let_args(*base, map)),
            width: Box::new(substitute_let_args(*width, map)),
        },
        Expr::Concat(items) => Expr::Concat(
            items
                .into_iter()
                .map(|e| substitute_let_args(e, map))
                .collect(),
        ),
        Expr::Replicate { count, expr } => Expr::Replicate {
            count: Box::new(substitute_let_args(*count, map)),
            expr: Box::new(substitute_let_args(*expr, map)),
        },
        Expr::UnaryOp { op, expr } => Expr::UnaryOp {
            op,
            expr: Box::new(substitute_let_args(*expr, map)),
        },
        Expr::BinaryOp { op, lhs, rhs } => Expr::BinaryOp {
            op,
            lhs: Box::new(substitute_let_args(*lhs, map)),
            rhs: Box::new(substitute_let_args(*rhs, map)),
        },
        Expr::TernaryOp {
            cond,
            true_expr,
            false_expr,
        } => Expr::TernaryOp {
            cond: Box::new(substitute_let_args(*cond, map)),
            true_expr: Box::new(substitute_let_args(*true_expr, map)),
            false_expr: Box::new(substitute_let_args(*false_expr, map)),
        },
        Expr::Paren(inner) => Expr::Paren(Box::new(substitute_let_args(*inner, map))),
        Expr::FuncCall {
            name,
            args,
            line,
            col,
        } => Expr::FuncCall {
            name,
            args: args
                .into_iter()
                .map(|e| substitute_let_args(e, map))
                .collect(),
            line,
            col,
        },
        Expr::MethodCall {
            obj,
            method,
            args,
            with_clause,
        } => Expr::MethodCall {
            obj: Box::new(substitute_let_args(*obj, map)),
            method,
            args: args
                .into_iter()
                .map(|e| substitute_let_args(e, map))
                .collect(),
            with_clause: with_clause.map(|w| Box::new(substitute_let_args(*w, map))),
        },
        Expr::MemberAccess { obj, field } => Expr::MemberAccess {
            obj: Box::new(substitute_let_args(*obj, map)),
            field,
        },
        Expr::Inside { expr, range_list } => Expr::Inside {
            expr: Box::new(substitute_let_args(*expr, map)),
            range_list: range_list
                .into_iter()
                .map(|e| substitute_let_args(e, map))
                .collect(),
        },
        Expr::Cast { expr, dtype } => Expr::Cast {
            expr: Box::new(substitute_let_args(*expr, map)),
            dtype,
        },
        Expr::IncDec {
            expr: inner,
            inc,
            pre,
        } => Expr::IncDec {
            expr: Box::new(substitute_let_args(*inner, map)),
            inc,
            pre,
        },
        other => other,
    }
}
