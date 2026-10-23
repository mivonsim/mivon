//! Read/write collection helpers for `mlint`.

use std::collections::HashSet;

use mivon_ast::expr::Expr;
use mivon_ast::stmt::Stmt;
use mivon_core::intern::Symbol;

pub(crate) fn lvalue_root(e: &Expr) -> Option<Symbol> {
    match e {
        Expr::Ident { name, .. } => Some(*name),
        Expr::BitSelect { expr, .. }
        | Expr::RangeSelect { expr, .. }
        | Expr::PartSelect { expr, .. } => lvalue_root(expr),
        Expr::MemberAccess { obj, .. } => lvalue_root(obj),
        Expr::Concat(parts) => parts.iter().find_map(lvalue_root),
        _ => None,
    }
}

pub(crate) fn scan_sequence_reads(
    seq: &mivon_ast::types::Sequence,
    reads: &mut HashSet<Symbol>,
    writes: &mut HashSet<Symbol>,
) {
    use mivon_ast::types::Sequence;
    match seq {
        Sequence::Expr(e) => scan_expr_reads(e, reads, writes),
        Sequence::Delay(_) | Sequence::DelayRange(_, _) => {}
        Sequence::Concat(l, r) | Sequence::Or(l, r) | Sequence::And(l, r) => {
            scan_sequence_reads(l, reads, writes);
            scan_sequence_reads(r, reads, writes);
        }
        Sequence::Repeat(s, _) => scan_sequence_reads(s, reads, writes),
        Sequence::Implication(ante, cons) => {
            scan_sequence_reads(ante, reads, writes);
            scan_sequence_reads(cons, reads, writes);
        }
    }
}

#[allow(clippy::only_used_in_recursion)]
pub(crate) fn scan_expr_reads(e: &Expr, reads: &mut HashSet<Symbol>, writes: &mut HashSet<Symbol>) {
    match e {
        Expr::Ident { name, .. } => {
            reads.insert(*name);
        }
        Expr::Value(_) | Expr::Null | Expr::String(_) | Expr::FillLit(_) => {}
        Expr::FuncCall { name, args, .. } => {
            reads.insert(*name);
            for a in args {
                scan_expr_reads(a, reads, writes);
            }
        }
        Expr::ScopedIdent { package, item, .. } => {
            reads.insert(*package);
            reads.insert(*item);
        }
        Expr::RangeSelect { expr, msb, lsb } => {
            scan_expr_reads(expr, reads, writes);
            scan_expr_reads(msb, reads, writes);
            scan_expr_reads(lsb, reads, writes);
        }
        Expr::BitSelect { expr, index } => {
            scan_expr_reads(expr, reads, writes);
            scan_expr_reads(index, reads, writes);
        }
        Expr::PartSelect { expr, base, width } => {
            scan_expr_reads(expr, reads, writes);
            scan_expr_reads(base, reads, writes);
            scan_expr_reads(width, reads, writes);
        }
        Expr::Concat(parts) => {
            for p in parts {
                scan_expr_reads(p, reads, writes);
            }
        }
        Expr::Replicate { count, expr } => {
            scan_expr_reads(count, reads, writes);
            scan_expr_reads(expr, reads, writes);
        }
        Expr::UnaryOp { expr, .. } => scan_expr_reads(expr, reads, writes),
        Expr::BinaryOp { lhs, rhs, .. } => {
            scan_expr_reads(lhs, reads, writes);
            scan_expr_reads(rhs, reads, writes);
        }
        Expr::TernaryOp {
            cond,
            true_expr,
            false_expr,
        } => {
            scan_expr_reads(cond, reads, writes);
            scan_expr_reads(true_expr, reads, writes);
            scan_expr_reads(false_expr, reads, writes);
        }
        Expr::Paren(inner) => scan_expr_reads(inner, reads, writes),
        Expr::IncDec { expr, .. } => {
            if let Expr::Ident { name, .. } = expr.as_ref() {
                writes.insert(*name);
            }
            scan_expr_reads(expr, reads, writes);
        }
        Expr::MethodCall {
            obj,
            method,
            args,
            with_clause,
        } => {
            reads.insert(*method);
            scan_expr_reads(obj, reads, writes);
            for a in args {
                scan_expr_reads(a, reads, writes);
            }
            if let Some(wc) = with_clause {
                scan_expr_reads(wc, reads, writes);
            }
        }
        Expr::MemberAccess { obj, field } => {
            reads.insert(*field);
            scan_expr_reads(obj, reads, writes);
        }
        Expr::Inside { expr, range_list } => {
            scan_expr_reads(expr, reads, writes);
            for r in range_list {
                scan_expr_reads(r, reads, writes);
            }
        }
        Expr::StreamingConcat {
            slice_size, slices, ..
        } => {
            if let Some(s) = slice_size {
                scan_expr_reads(s, reads, writes);
            }
            for s in slices {
                scan_expr_reads(s, reads, writes);
            }
        }
        Expr::Cast { dtype, expr } => {
            reads.insert(*dtype);
            scan_expr_reads(expr, reads, writes);
        }
        Expr::CastWidth { width, expr } => {
            scan_expr_reads(width, reads, writes);
            scan_expr_reads(expr, reads, writes);
        }
        Expr::Dist { expr, items } => {
            scan_expr_reads(expr, reads, writes);
            for it in items {
                match it {
                    mivon_ast::expr::DistItem::Value(v, _) => scan_expr_reads(v, reads, writes),
                    mivon_ast::expr::DistItem::Range(a, b, _) => {
                        scan_expr_reads(a, reads, writes);
                        scan_expr_reads(b, reads, writes);
                    }
                }
            }
        }
        Expr::StructLit { members } => {
            for m in members {
                match m {
                    mivon_ast::expr::StructLitMember::Named(_, e)
                    | mivon_ast::expr::StructLitMember::Positional(e)
                    | mivon_ast::expr::StructLitMember::Default(e) => {
                        scan_expr_reads(e, reads, writes);
                    }
                }
            }
        }
    }
}

pub(crate) fn scan_stmt_reads(
    stmts: &[Stmt],
    reads: &mut HashSet<Symbol>,
    writes: &mut HashSet<Symbol>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Block { stmts }
            | Stmt::LoopForever { stmts }
            | Stmt::NamedBlock { stmts, .. } => {
                scan_stmt_reads(stmts, reads, writes);
            }
            Stmt::IfElse {
                cond,
                true_branch,
                false_branch,
            } => {
                scan_expr_reads(cond, reads, writes);
                scan_stmt_reads(std::slice::from_ref(true_branch), reads, writes);
                if let Some(fb) = false_branch {
                    scan_stmt_reads(std::slice::from_ref(fb), reads, writes);
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
                scan_expr_reads(expr, reads, writes);
                for ci in items {
                    for l in &ci.labels {
                        scan_expr_reads(l, reads, writes);
                    }
                    scan_stmt_reads(std::slice::from_ref(&ci.stmt), reads, writes);
                }
                if let Some(d) = default {
                    scan_stmt_reads(std::slice::from_ref(d), reads, writes);
                }
            }
            Stmt::LoopWhile { cond, stmts }
            | Stmt::Repeat { count: cond, stmts }
            | Stmt::DoWhile { cond, stmts } => {
                scan_expr_reads(cond, reads, writes);
                scan_stmt_reads(stmts, reads, writes);
            }
            Stmt::LoopFor {
                init,
                cond,
                step,
                stmts,
            } => {
                if let Some(i) = init {
                    scan_stmt_reads(std::slice::from_ref(i), reads, writes);
                }
                if let Some(c) = cond {
                    scan_expr_reads(c, reads, writes);
                }
                if let Some(s) = step {
                    scan_stmt_reads(std::slice::from_ref(s), reads, writes);
                }
                scan_stmt_reads(stmts, reads, writes);
            }
            Stmt::BlockingAssign { lhs, rhs, .. }
            | Stmt::NonBlockingAssign { lhs, rhs, .. }
            | Stmt::StmtAssign { lhs, rhs } => {
                scan_expr_reads(rhs, reads, writes);
                if let Some(root) = lvalue_root(lhs) {
                    writes.insert(root);
                }
            }
            Stmt::Force { lhs, rhs } => {
                scan_expr_reads(rhs, reads, writes);
                if let Some(root) = lvalue_root(lhs) {
                    writes.insert(root);
                }
            }
            Stmt::Release { expr } => {
                if let Some(root) = lvalue_root(expr) {
                    writes.insert(root);
                }
            }
            Stmt::Deassign { expr } => {
                if let Some(root) = lvalue_root(expr) {
                    writes.insert(root);
                }
            }
            Stmt::Expr { expr } => scan_expr_reads(expr, reads, writes),
            Stmt::SysCall { name, args, .. } => {
                reads.insert(*name);
                for a in args {
                    scan_expr_reads(a, reads, writes);
                }
            }
            Stmt::Delay { delay, stmt } => {
                scan_expr_reads(delay, reads, writes);
                scan_stmt_reads(std::slice::from_ref(stmt), reads, writes);
            }
            Stmt::Wait { cond, stmt } => {
                scan_expr_reads(cond, reads, writes);
                if let Some(s) = stmt {
                    scan_stmt_reads(std::slice::from_ref(s), reads, writes);
                }
            }
            Stmt::WaitFork => {}
            Stmt::EventControl { events, stmt } => {
                for ev in events {
                    let inner = match ev {
                        mivon_ast::stmt::SensitivityEvent::Iff { event, cond } => {
                            scan_expr_reads(cond, reads, writes);
                            event.as_ref()
                        }
                        other => other,
                    };
                    match inner {
                        mivon_ast::stmt::SensitivityEvent::PosEdge(e)
                        | mivon_ast::stmt::SensitivityEvent::NegEdge(e)
                        | mivon_ast::stmt::SensitivityEvent::Level(e) => {
                            scan_expr_reads(e, reads, writes);
                        }
                        mivon_ast::stmt::SensitivityEvent::Iff { .. }
                        | mivon_ast::stmt::SensitivityEvent::Wildcard => {}
                    }
                }
                if let Some(s) = stmt {
                    scan_stmt_reads(std::slice::from_ref(s), reads, writes);
                }
            }
            Stmt::PropertySeq {
                sequence,
                pass_stmt,
                fail_stmt,
                clock_event,
                disable_iff,
            } => {
                scan_sequence_reads(sequence, reads, writes);
                if let Some(ce) = clock_event {
                    let s = match ce {
                        mivon_ast::types::ClockEvent::Posedge(s)
                        | mivon_ast::types::ClockEvent::Negedge(s)
                        | mivon_ast::types::ClockEvent::Edge(s) => s,
                    };
                    reads.insert(*s);
                }
                if let Some(di) = disable_iff {
                    scan_expr_reads(di, reads, writes);
                }
                if let Some(ps) = pass_stmt {
                    scan_stmt_reads(std::slice::from_ref(ps), reads, writes);
                }
                if let Some(fs) = fail_stmt {
                    scan_stmt_reads(std::slice::from_ref(fs), reads, writes);
                }
            }
            Stmt::Assert {
                cond,
                pass_stmt,
                fail_stmt,
                clock_event,
                disable_iff,
            }
            | Stmt::Assume {
                cond,
                pass_stmt,
                fail_stmt,
                clock_event,
                disable_iff,
            } => {
                scan_expr_reads(cond, reads, writes);
                if let Some(ce) = clock_event {
                    let s = match ce {
                        mivon_ast::types::ClockEvent::Posedge(s)
                        | mivon_ast::types::ClockEvent::Negedge(s)
                        | mivon_ast::types::ClockEvent::Edge(s) => s,
                    };
                    reads.insert(*s);
                }
                if let Some(d) = disable_iff {
                    scan_expr_reads(d, reads, writes);
                }
                if let Some(ps) = pass_stmt {
                    scan_stmt_reads(std::slice::from_ref(ps), reads, writes);
                }
                if let Some(fs) = fail_stmt {
                    scan_stmt_reads(std::slice::from_ref(fs), reads, writes);
                }
            }
            Stmt::Cover {
                cond,
                pass_stmt,
                clock_event,
                disable_iff,
            } => {
                scan_expr_reads(cond, reads, writes);
                if let Some(ce) = clock_event {
                    let s = match ce {
                        mivon_ast::types::ClockEvent::Posedge(s)
                        | mivon_ast::types::ClockEvent::Negedge(s)
                        | mivon_ast::types::ClockEvent::Edge(s) => s,
                    };
                    reads.insert(*s);
                }
                if let Some(d) = disable_iff {
                    scan_expr_reads(d, reads, writes);
                }
                if let Some(ps) = pass_stmt {
                    scan_stmt_reads(std::slice::from_ref(ps), reads, writes);
                }
            }
            Stmt::Expect {
                cond,
                pass_stmt,
                fail_stmt,
            } => {
                scan_expr_reads(cond, reads, writes);
                if let Some(ps) = pass_stmt {
                    scan_stmt_reads(std::slice::from_ref(ps), reads, writes);
                }
                if let Some(fs) = fail_stmt {
                    scan_stmt_reads(std::slice::from_ref(fs), reads, writes);
                }
            }
            Stmt::WaitOrder {
                events,
                fail_stmt,
                ..
            } => {
                for e in events {
                    reads.insert(*e);
                }
                if let Some(fs) = fail_stmt {
                    scan_stmt_reads(std::slice::from_ref(fs), reads, writes);
                }
            }
            Stmt::UniqueIf {
                cond,
                true_branch,
                false_branch,
            }
            | Stmt::PriorityIf {
                cond,
                true_branch,
                false_branch,
            } => {
                scan_expr_reads(cond, reads, writes);
                scan_stmt_reads(std::slice::from_ref(true_branch), reads, writes);
                if let Some(fb) = false_branch {
                    scan_stmt_reads(std::slice::from_ref(fb), reads, writes);
                }
            }
            Stmt::Fork { processes, .. } => {
                for p in processes {
                    scan_stmt_reads(std::slice::from_ref(p), reads, writes);
                }
            }
            Stmt::RandCase { items } => {
                for it in items {
                    scan_stmt_reads(std::slice::from_ref(&it.stmt), reads, writes);
                }
            }
            Stmt::RandSequence { productions } => {
                for p in productions {
                    for it in &p.items {
                        scan_stmt_reads(std::slice::from_ref(&it.value), reads, writes);
                    }
                }
            }
            Stmt::Return(Some(e)) => scan_expr_reads(e, reads, writes),
            Stmt::Disable { name } => {
                reads.insert(*name);
            }
            Stmt::EventTrigger { name } => {
                writes.insert(*name);
            }
            Stmt::ForeachLoop {
                array_var,
                index_vars,
                stmts,
            } => {
                reads.insert(*array_var);
                for iv in index_vars {
                    reads.insert(*iv);
                }
                scan_stmt_reads(stmts, reads, writes);
            }
            Stmt::Break | Stmt::Continue | Stmt::Null | Stmt::SysFinish | Stmt::Return(None) => {}
        }
    }
}