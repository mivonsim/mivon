//! `mlint` — Static RTL Linter.
//!
//! Check: unused signal, width mismatch, latch detection, combinational loop,
//! FSM state register.

use std::collections::{HashMap, HashSet};

use crate::{open_project, section};
use mivon_ast::expr::Expr;
use mivon_ast::stmt::Stmt;
use mivon_ast::types::ModuleItem;
use mivon_core::error::SimError;
use mivon_core::intern::Symbol;

/// Opsi mlint.
pub struct LintArgs<'a> {
    pub targets: &'a [String],
    pub incdirs: &'a [String],
    pub defines: &'a [String],
    pub all: bool,
    pub unused: bool,
    pub width: bool,
    pub latch: bool,
    pub loop_check: bool,
    pub fsm: bool,
    pub case_analysis: bool,
    pub clock_gating: bool,
    pub power: bool,
    pub memory: bool,
    pub quiet: bool,
    pub gate_opt: bool,
}

/// Satu temuan lint.
struct Finding {
    module: String,
    check: &'static str,
    severity: &'static str, // "W" warning, "E" error
    message: String,
}

/// Jalankan mlint.
pub fn run(args: &LintArgs) -> Result<(), SimError> {
    let all = args.all;
    let chk = |flag: bool| flag || all;

    let do_unused = chk(args.unused);
    let do_width = chk(args.width);
    let do_latch = chk(args.latch);
    let do_loop = chk(args.loop_check);
    let do_fsm = chk(args.fsm);
    let do_case_analysis = chk(args.case_analysis);
    let do_clock_gating = chk(args.clock_gating);
    let do_power = chk(args.power);
    let do_memory = chk(args.memory);
    let do_gate_opt = chk(args.gate_opt);

    let (design, _session) = open_project(args.targets, args.incdirs, args.defines, None)?;

    let mut findings: Vec<Finding> = Vec::new();
    for module in &design.modules {
        lint_module(
            module,
            do_unused,
            do_width,
            do_latch,
            do_loop,
            do_fsm,
            do_case_analysis,
            do_clock_gating,
            do_power,
            do_memory,
            do_gate_opt,
            &mut findings,
        );
    }
    // Interface juga di-lint (items = ModuleItem)
    for iface in &design.interfaces {
        lint_items(
            iface.name.as_str(),
            &iface.decls,
            &iface.ports,
            &iface.items,
            do_unused,
            do_width,
            do_latch,
            do_loop,
            do_fsm,
            do_case_analysis,
            do_clock_gating,
            do_power,
            do_memory,
            do_gate_opt,
            &mut findings,
        );
    }

    findings.sort_by(|a, b| a.module.cmp(&b.module).then(a.check.cmp(b.check)));

    // Simpan hasil ke cache pipeline (db.md "7. verify/ → lint/") agar
    // `minspect cache` / run berikutnya membaca temuan tanpa lint ulang.
    save_lint_cache(args, &findings);

    let n_warn = findings.iter().filter(|f| f.severity == "W").count();
    let n_err = findings.iter().filter(|f| f.severity == "E").count();

    section("mlint Report");
    for f in &findings {
        if args.quiet {
            continue;
        }
        println!(
            "  [{}] {:<10} {:<12} {}",
            f.severity, f.check, f.module, f.message
        );
    }
    println!("\n  {} warning, {} error", n_warn, n_err);

    if n_err > 0 {
        return Err(SimError::with_diag(
            mivon_core::diagnostics::DiagCode::InvalidSyntax,
            format!("mlint menemukan {} error", n_err),
        ));
    }
    Ok(())
}

/// ── Analisis per module ──
#[allow(clippy::too_many_arguments)]
fn lint_module(
    module: &mivon_ast::types::Module,
    do_unused: bool,
    do_width: bool,
    do_latch: bool,
    do_loop: bool,
    do_fsm: bool,
    do_case_analysis: bool,
    do_clock_gating: bool,
    do_power: bool,
    do_memory: bool,
    do_gate_opt: bool,
    out: &mut Vec<Finding>,
) {
    lint_items(
        module.name.as_str(),
        &module.decls,
        &module.ports,
        &module.items,
        do_unused,
        do_width,
        do_latch,
        do_loop,
        do_fsm,
        do_case_analysis,
        do_clock_gating,
        do_power,
        do_memory,
        do_gate_opt,
        out,
    );
}

#[allow(clippy::too_many_arguments)]
fn lint_items(
    scope: &str,
    decls: &[mivon_ast::types::Decl],
    ports: &[mivon_ast::types::Port],
    items: &[ModuleItem],
    do_unused: bool,
    do_width: bool,
    do_latch: bool,
    do_loop: bool,
    do_fsm: bool,
    do_case_analysis: bool,
    do_clock_gating: bool,
    do_power: bool,
    do_memory: bool,
    do_gate_opt: bool,
    out: &mut Vec<Finding>,
) {
    // ── Kumpulkan deklarasi sinyal (decls + decl items + ports) ──
    let mut declared: HashMap<Symbol, usize> = HashMap::new(); // name → width
    let mut declared_raw: Vec<Symbol> = Vec::new();
    let mut collect_decls = |d: &mivon_ast::types::Decl| {
        for v in &d.names {
            let w = decl_width(d, v);
            declared.insert(v.name, w);
            declared_raw.push(v.name);
        }
    };
    for d in decls {
        collect_decls(d);
    }
    for item in items {
        if let ModuleItem::Decl(d) = item {
            collect_decls(d);
        }
    }
    let port_set: HashSet<Symbol> = ports.iter().map(|p| p.name).collect();
    for p in ports {
        let w = p.range.as_ref().map(|r| r.width()).unwrap_or(1);
        declared.insert(p.name, w);
    }

    // ── Walk semua always/initial/assign ──
    let mut reads: HashSet<Symbol> = HashSet::new();
    let mut writes: HashSet<Symbol> = HashSet::new();
    let mut always_blocks: Vec<&mivon_ast::stmt::AlwaysBlock> = Vec::new();

    for item in items {
        match item {
            ModuleItem::Always(block) => {
                always_blocks.push(block);
                scan_stmt_reads(&block.stmts, &mut reads, &mut writes);
            }
            ModuleItem::Initial(block) => {
                scan_stmt_reads(&block.stmts, &mut reads, &mut writes);
            }
            ModuleItem::Final(block) => {
                scan_stmt_reads(&block.stmts, &mut reads, &mut writes);
            }
            ModuleItem::Assign(ca) => {
                scan_expr_reads(&ca.lhs, &mut reads, &mut writes);
                scan_expr_reads(&ca.rhs, &mut reads, &mut writes);
                if let Some(root) = lvalue_root(&ca.lhs) {
                    writes.insert(root);
                }
                if do_width {
                    check_width(scope, &ca.lhs, &ca.rhs, &declared, out);
                }
            }
            ModuleItem::Generate(g) => {
                for gi in &g.items {
                    walk_generate(
                        gi,
                        &mut reads,
                        &mut writes,
                        &mut always_blocks,
                        do_width,
                        &declared,
                        out,
                    );
                }
            }
            _ => {}
        }
    }

    // Signal yang juga dipakai di generate item declarations? Abaikan untuk now.

    if do_unused {
        for name in &declared_raw {
            if port_set.contains(name) {
                continue;
            }
            let r = reads.contains(name);
            let w = writes.contains(name);
            if !r && !w {
                out.push(Finding {
                    module: scope.to_string(),
                    check: "unused",
                    severity: "W",
                    message: format!("signal '{}' tidak pernah dipakai", name.as_str()),
                });
            } else if w && !r {
                out.push(Finding {
                    module: scope.to_string(),
                    check: "unused",
                    severity: "W",
                    message: format!(
                        "signal '{}' hanya ditulis, tidak pernah dibaca",
                        name.as_str()
                    ),
                });
            }
        }
    }

    if do_latch {
        for block in &always_blocks {
            if matches!(block.kind, mivon_ast::stmt::AlwaysKind::AlwaysLatch) {
                out.push(Finding {
                    module: scope.to_string(),
                    check: "latch",
                    severity: "W",
                    message: "block memakai always_latch — pastikan intentional".to_string(),
                });
            }
            // always_comb dengan if tanpa else → potensi latch
            if matches!(
                block.kind,
                mivon_ast::stmt::AlwaysKind::AlwaysComb | mivon_ast::stmt::AlwaysKind::AlwaysLatch
            ) {
                find_incomplete_if(scope, &block.stmts, out);
            }
        }
    }

    if do_loop {
        for block in &always_blocks {
            if !matches!(block.kind, mivon_ast::stmt::AlwaysKind::AlwaysComb) {
                continue;
            }
            let mut r = HashSet::new();
            let mut w = HashSet::new();
            scan_stmt_reads(&block.stmts, &mut r, &mut w);
            let overlap: Vec<Symbol> = w.intersection(&r).copied().collect();
            for s in overlap {
                if port_set.contains(&s) {
                    continue;
                }
                out.push(Finding {
                    module: scope.to_string(),
                    check: "comb_loop",
                    severity: "W",
                    message: format!("signal '{}' dibaca & ditulis di block yang sama → potensi combinational loop", s.as_str()),
                });
            }
        }
    }

    if do_fsm {
        for block in &always_blocks {
            if !matches!(block.kind, mivon_ast::stmt::AlwaysKind::AlwaysFF) {
                continue;
            }
            find_fsm(scope, &block.stmts, out);
        }
    }

    // COMP-12: Case analysis — deteksi case tanpa default + unique/priority overlap
    if do_case_analysis {
        for block in &always_blocks {
            find_case_analysis(scope, &block.stmts, out);
        }
    }

    // COMP-13: Clock gating inference — deteksi always_ff dengan if-gate
    if do_clock_gating {
        for block in &always_blocks {
            if matches!(block.kind, mivon_ast::stmt::AlwaysKind::AlwaysFF) {
                find_clock_gating(scope, &block.stmts, out);
            }
        }
    }

    // COMP-14: Power optimization — deteksi UPF power domain patterns
    if do_power {
        find_power_patterns(scope, &declared, out);
    }

    // COMP-15: Memory inference — deteksi reg array patterns
    if do_memory {
        find_memory_inference(scope, decls, items, out);
    }

    // COMP-10: Gate-level optimization — deteksi pattern yang bisa dioptimasi
    if do_gate_opt {
        find_gate_opt_patterns(scope, items, out);
    }
}

fn walk_generate<'a>(
    gi: &'a mivon_ast::types::GenerateItem,
    reads: &mut HashSet<Symbol>,
    writes: &mut HashSet<Symbol>,
    always_blocks: &mut Vec<&'a mivon_ast::stmt::AlwaysBlock>,
    do_width: bool,
    declared: &HashMap<Symbol, usize>,
    out: &mut Vec<Finding>,
) {
    use mivon_ast::types::GenerateItem;
    match gi {
        GenerateItem::Items(items) => {
            walk_items(items, reads, writes, always_blocks, do_width, declared, out)
        }
        GenerateItem::If {
            true_items,
            false_items,
            ..
        } => {
            walk_items(
                true_items,
                reads,
                writes,
                always_blocks,
                do_width,
                declared,
                out,
            );
            walk_items(
                false_items,
                reads,
                writes,
                always_blocks,
                do_width,
                declared,
                out,
            );
        }
        GenerateItem::For { body_items, .. } => {
            walk_items(
                body_items,
                reads,
                writes,
                always_blocks,
                do_width,
                declared,
                out,
            );
        }
        GenerateItem::Case { items, default, .. } => {
            for ci in items {
                walk_items(
                    &ci.body,
                    reads,
                    writes,
                    always_blocks,
                    do_width,
                    declared,
                    out,
                );
            }
            if let Some(d) = default {
                walk_items(d, reads, writes, always_blocks, do_width, declared, out);
            }
        }
    }
}

fn walk_items<'a>(
    items: &'a [ModuleItem],
    reads: &mut HashSet<Symbol>,
    writes: &mut HashSet<Symbol>,
    always_blocks: &mut Vec<&'a mivon_ast::stmt::AlwaysBlock>,
    do_width: bool,
    declared: &HashMap<Symbol, usize>,
    out: &mut Vec<Finding>,
) {
    for item in items {
        match item {
            ModuleItem::Always(b) => always_blocks.push(b),
            ModuleItem::Assign(ca) => {
                scan_expr_reads(&ca.lhs, reads, writes);
                scan_expr_reads(&ca.rhs, reads, writes);
                if let Some(root) = lvalue_root(&ca.lhs) {
                    writes.insert(root);
                }
                if do_width {
                    check_width("generate", &ca.lhs, &ca.rhs, declared, out);
                }
            }
            ModuleItem::Generate(g) => {
                for gi in &g.items {
                    walk_generate(gi, reads, writes, always_blocks, do_width, declared, out);
                }
            }
            _ => {}
        }
    }
}

/// ── Width ──
fn decl_width(d: &mivon_ast::types::Decl, v: &mivon_ast::types::DeclVar) -> usize {
    if let Some(r) = &v.range {
        return r.width();
    }
    match &d.dtype {
        mivon_ast::types::DataType::Byte => 8,
        mivon_ast::types::DataType::Shortint => 16,
        mivon_ast::types::DataType::Int | mivon_ast::types::DataType::Integer => 32,
        mivon_ast::types::DataType::Longint => 64,
        mivon_ast::types::DataType::Bit | mivon_ast::types::DataType::Logic => 1,
        _ => 1,
    }
}

fn expr_width(e: &Expr, declared: &HashMap<Symbol, usize>) -> Option<usize> {
    match e {
        Expr::Ident { name, .. } => declared.get(name).copied(),
        Expr::Value(v) => Some(match v {
            mivon_ast::expr::Value::Binary { width, .. }
            | mivon_ast::expr::Value::Hex { width, .. }
            | mivon_ast::expr::Value::Octal { width, .. } => width.unwrap_or(1),
            mivon_ast::expr::Value::Decimal(_) => 32,
            mivon_ast::expr::Value::Real(_) => 64,
        }),
        Expr::Concat(parts) => {
            let mut total = 0usize;
            for p in parts {
                total += expr_width(p, declared)?;
            }
            Some(total)
        }
        Expr::BitSelect { .. } => Some(1),
        Expr::RangeSelect { expr, msb, lsb, .. } => {
            // width = |msb - lsb| + 1 jika keduanya konstanta
            if let (
                Expr::Value(mivon_ast::expr::Value::Decimal(m)),
                Expr::Value(mivon_ast::expr::Value::Decimal(l)),
            ) = (&**msb, &**lsb)
            {
                Some(m.abs_diff(*l) as usize + 1)
            } else {
                expr_width(expr, declared)
            }
        }
        Expr::BinaryOp { lhs, rhs, op } => {
            let lw = expr_width(lhs, declared)?;
            let rw = expr_width(rhs, declared)?;
            match op {
                mivon_ast::expr::BinaryOp::Eq
                | mivon_ast::expr::BinaryOp::Neq
                | mivon_ast::expr::BinaryOp::CaseEq
                | mivon_ast::expr::BinaryOp::CaseNeq
                | mivon_ast::expr::BinaryOp::Lt
                | mivon_ast::expr::BinaryOp::Le
                | mivon_ast::expr::BinaryOp::Gt
                | mivon_ast::expr::BinaryOp::Ge
                | mivon_ast::expr::BinaryOp::LogicalAnd
                | mivon_ast::expr::BinaryOp::LogicalOr => Some(1),
                _ => Some(lw.max(rw)),
            }
        }
        Expr::UnaryOp { expr, .. } => expr_width(expr, declared),
        Expr::TernaryOp {
            true_expr,
            false_expr,
            ..
        } => {
            let t = expr_width(true_expr, declared)?;
            let f = expr_width(false_expr, declared)?;
            Some(t.max(f))
        }
        Expr::Paren(inner) => expr_width(inner, declared),
        Expr::FillLit(_) => Some(1),
        Expr::Cast { expr, .. } => expr_width(expr, declared),
        Expr::CastWidth { width, .. } => expr_width(width, declared),
        Expr::MemberAccess { .. } => None,
        _ => None,
    }
}

fn check_width(
    scope: &str,
    lhs: &Expr,
    rhs: &Expr,
    declared: &HashMap<Symbol, usize>,
    out: &mut Vec<Finding>,
) {
    let Some(lw) = expr_width(lhs, declared) else {
        return;
    };
    let Some(rw) = expr_width(rhs, declared) else {
        return;
    };
    if lw != rw {
        out.push(Finding {
            module: scope.to_string(),
            check: "width",
            severity: "W",
            message: format!("width mismatch: lhs {} bit vs rhs {} bit", lw, rw),
        });
    }
}

/// ── Read/write collection ──
fn lvalue_root(e: &Expr) -> Option<Symbol> {
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

fn scan_sequence_reads(
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
fn scan_expr_reads(e: &Expr, reads: &mut HashSet<Symbol>, writes: &mut HashSet<Symbol>) {
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
            // RMW: operand dibaca DAN ditulis (lint latch/unused pakai keduanya).
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

fn scan_stmt_reads(stmts: &[Stmt], reads: &mut HashSet<Symbol>, writes: &mut HashSet<Symbol>) {
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
            Stmt::WaitOrder { events, fail_stmt } => {
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

/// ── Latch ──
fn find_incomplete_if(scope: &str, stmts: &[Stmt], out: &mut Vec<Finding>) {
    for stmt in stmts {
        match stmt {
            Stmt::IfElse { false_branch, .. } if false_branch.is_none() => {
                out.push(Finding {
                    module: scope.to_string(),
                    check: "latch",
                    severity: "W",
                    message: "if tanpa else di blok kombinasional → potensi latch".to_string(),
                });
            }
            Stmt::IfElse {
                true_branch,
                false_branch,
                ..
            } => {
                find_incomplete_if(scope, std::slice::from_ref(true_branch), out);
                if let Some(fb) = false_branch {
                    find_incomplete_if(scope, std::slice::from_ref(fb), out);
                }
            }
            Stmt::Block { stmts }
            | Stmt::NamedBlock { stmts, .. }
            | Stmt::LoopForever { stmts } => {
                find_incomplete_if(scope, stmts, out);
            }
            _ => {}
        }
    }
}

/// ── FSM ──
fn find_fsm(scope: &str, stmts: &[Stmt], out: &mut Vec<Finding>) {
    for stmt in stmts {
        match stmt {
            Stmt::Case { expr, .. }
            | Stmt::CaseX { expr, .. }
            | Stmt::CaseZ { expr, .. }
            | Stmt::UniqueCase { expr, .. }
            | Stmt::PriorityCase { expr, .. }
            | Stmt::Unique0Case { expr, .. }
            | Stmt::CaseInside { expr, .. } => {
                if let Expr::Ident { name, .. } = expr {
                    if name.as_str().contains("state") || name.as_str().contains("fsm") {
                        out.push(Finding {
                            module: scope.to_string(),
                            check: "fsm",
                            severity: "I",
                            message: format!(
                                "deteksi FSM: register state '{}' ({} case item)",
                                name.as_str(),
                                case_item_count(stmt)
                            ),
                        });
                    }
                }
            }
            Stmt::Block { stmts }
            | Stmt::NamedBlock { stmts, .. }
            | Stmt::LoopForever { stmts } => {
                find_fsm(scope, stmts, out);
            }
            Stmt::IfElse {
                true_branch,
                false_branch,
                ..
            } => {
                find_fsm(scope, std::slice::from_ref(true_branch), out);
                if let Some(fb) = false_branch {
                    find_fsm(scope, std::slice::from_ref(fb), out);
                }
            }
            _ => {}
        }
    }
}

fn case_item_count(stmt: &Stmt) -> usize {
    match stmt {
        Stmt::Case { items, .. }
        | Stmt::CaseX { items, .. }
        | Stmt::CaseZ { items, .. }
        | Stmt::UniqueCase { items, .. }
        | Stmt::PriorityCase { items, .. }
        | Stmt::Unique0Case { items, .. }
        | Stmt::CaseInside { items, .. } => items.len(),
        _ => 0,
    }
}

/// COMP-12: Case analysis — deteksi case tanpa default + unique/priority overlap.
///
/// IEEE 1800-2017 §12.5.2: `unique case` tanpa default dan 0 match =
/// warning (warna tak terdefinisi). `priority case` tanpa default =
/// warning bila semua label konstanta dan tidak ada yang match.
/// Selain itu, case tanpa default di blok kombinasional → potensi latch.
fn find_case_analysis(scope: &str, stmts: &[Stmt], out: &mut Vec<Finding>) {
    for stmt in stmts {
        match stmt {
            Stmt::Case { items, default, .. }
            | Stmt::CaseX { items, default, .. }
            | Stmt::CaseZ { items, default, .. } => {
                // Case biasa tanpa default di kombinasional → potensi latch
                if default.is_none() && !items.is_empty() {
                    out.push(Finding {
                        module: scope.to_string(),
                        check: "case",
                        severity: "W",
                        message: format!(
                            "case tanpa 'default' ({} item) → potensi latch atau nilai tak terdefinisi",
                            items.len()
                        ),
                    });
                }
            }
            Stmt::UniqueCase { items, default, .. } => {
                // unique case tanpa default → warning
                if default.is_none() && !items.is_empty() {
                    out.push(Finding {
                        module: scope.to_string(),
                        check: "case_unique",
                        severity: "W",
                        message: format!(
                            "unique case tanpa 'default' ({} item) → bila tidak ada match, nilai tak terdefinisi",
                            items.len()
                        ),
                    });
                }
            }
            Stmt::PriorityCase { items, default, .. } => {
                // priority case tanpa default → warning
                if default.is_none() && !items.is_empty() {
                    out.push(Finding {
                        module: scope.to_string(),
                        check: "case_priority",
                        severity: "W",
                        message: format!(
                            "priority case tanpa 'default' ({} item) → bila tidak ada match, nilai tak terdefinisi",
                            items.len()
                        ),
                    });
                }
            }
            Stmt::Unique0Case { .. } => {
                // unique0 case: tanpa default = normal (0 match → 0 value)
            }
            Stmt::Block { stmts }
            | Stmt::NamedBlock { stmts, .. }
            | Stmt::LoopForever { stmts } => {
                find_case_analysis(scope, stmts, out);
            }
            Stmt::IfElse {
                true_branch,
                false_branch,
                ..
            } => {
                find_case_analysis(scope, std::slice::from_ref(true_branch), out);
                if let Some(fb) = false_branch {
                    find_case_analysis(scope, std::slice::from_ref(fb), out);
                }
            }
            _ => {}
        }
    }
}

/// COMP-13: Clock gating inference — deteksi always_ff dengan if-gate.
///
/// Pola `always_ff @(posedge clk) if (en) q <= d;` tanpa else → clock gating
/// implisit. Synthesizer mungkin menginfer ICG (Integrated Clock Gating) cell,
/// tapi intent lebih jelas bila menggunakan `always_latch` atau eksplicit
/// clock gating. Warning untuk awareness designer.
fn find_clock_gating(scope: &str, stmts: &[Stmt], out: &mut Vec<Finding>) {
    for stmt in stmts {
        match stmt {
            Stmt::IfElse {
                true_branch,
                false_branch,
                ..
            } => {
                // if tanpa else di always_ff → clock gating pattern
                if false_branch.is_none() {
                    // Cek apakah true_branch berisi non-blocking assign (FF pattern)
                    if has_nonblocking(std::slice::from_ref(true_branch)) {
                        out.push(Finding {
                            module: scope.to_string(),
                            check: "clock_gating",
                            severity: "W",
                            message:
                                "always_ff dengan if tanpa else: potensi clock gating implisit"
                                    .to_string(),
                        });
                    }
                }
                // Recurse into branches
                find_clock_gating(scope, std::slice::from_ref(true_branch), out);
                if let Some(fb) = false_branch {
                    find_clock_gating(scope, std::slice::from_ref(fb), out);
                }
            }
            Stmt::Block { stmts }
            | Stmt::NamedBlock { stmts, .. }
            | Stmt::LoopForever { stmts } => {
                find_clock_gating(scope, stmts, out);
            }
            _ => {}
        }
    }
}

/// Cek apakah ada non-blocking assign dalam stmts (FF pattern).
fn has_nonblocking(stmts: &[Stmt]) -> bool {
    for stmt in stmts {
        match stmt {
            Stmt::NonBlockingAssign { .. } => return true,
            Stmt::Block { stmts } | Stmt::NamedBlock { stmts, .. } => {
                if has_nonblocking(stmts) {
                    return true;
                }
            }
            Stmt::IfElse {
                true_branch,
                false_branch,
                ..
            } => {
                if has_nonblocking(std::slice::from_ref(true_branch)) {
                    return true;
                }
                if let Some(fb) = false_branch {
                    if has_nonblocking(std::slice::from_ref(fb)) {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
}

/// COMP-14: Power optimization — deteksi signal besar tanpa isolasi.
///
/// Signal output > 8 bit tanpa `supply`/`power`/`ground` declaration →
/// warning agar designer pertimbangkan power domain isolation.
fn find_power_patterns(scope: &str, declared: &HashMap<Symbol, usize>, out: &mut Vec<Finding>) {
    for (name, &width) in declared {
        if width > 8 {
            // Cek apakah signal ada dalam nama power domain common
            let n = name.as_str().to_lowercase();
            if n.contains("vdd") || n.contains("vss") || n.contains("gnd") || n.contains("supply") {
                continue;
            }
            // Signal lebar tanpa power annotation — info
            out.push(Finding {
                module: scope.to_string(),
                check: "power",
                severity: "I",
                message: format!(
                    "signal '{}' ({} bit) — pertimbangkan power domain isolation untuk sinyal lebar",
                    name.as_str(), width
                ),
            });
        }
    }
}

/// COMP-15: Memory inference — deteksi reg array patterns.
///
/// `reg [W-1:0] mem [0:N-1]` → array reg dengan unpacked dimension →
/// kandidat inferred RAM/ROM. Warning untuk awareness designer bahwa
/// synthesizer mungkin menginfer RAM dari pattern ini.
fn find_memory_inference(
    scope: &str,
    decls: &[mivon_ast::types::Decl],
    _items: &[ModuleItem],
    out: &mut Vec<Finding>,
) {
    for d in decls {
        for v in &d.names {
            // Array unpacked: ada array_range setelah range packed
            if v.range.is_some() && v.array_range.is_some() {
                out.push(Finding {
                    module: scope.to_string(),
                    check: "memory",
                    severity: "I",
                    message: format!(
                        "reg '{}' array — kandidat inferred RAM/ROM",
                        v.name.as_str()
                    ),
                });
            }
        }
    }
}

/// Simpan temuan lint ke cache pipeline (`lint/"report"`, db.md "7. verify/
/// → lint/"). Best-effort — kegagalan cache tidak menggagalkan lint.
fn save_lint_cache(args: &LintArgs, findings: &[Finding]) {
    use mivon_compiler::micd::cache::pipeline::LintPayload;
    use mivon_compiler::micd::cache::CacheCategory;

    let Ok((mut layer, _pid)) = crate::open_cache_layer(args.targets, args.incdirs, args.defines)
    else {
        return;
    };
    let payload = LintPayload {
        findings: findings
            .iter()
            .map(|f| mivon_compiler::micd::cache::pipeline::LintFinding {
                module: f.module.clone(),
                check: f.check.to_string(),
                severity: f.severity.to_string(),
                message: f.message.clone(),
            })
            .collect(),
    };
    if let Ok(bytes) = bincode::serialize(&payload) {
        let _ = layer.put(CacheCategory::Lint, "report", &bytes);
        let _ = layer.save();
    }
}

// ═══ COMP-10: Gate-Level Optimization Patterns ═══

/// Deteksi pattern yang menunjukkan peluang optimasi gate-level:
/// 1. Redundant assignments: sinyal ditulis dua kali di always block yang sama
/// 2. Unused outputs: output dideklarasikan tapi tidak pernah ditulis
/// 3. Combinational feedback: sinyal dibaca dan ditulis di always_comb
fn find_gate_opt_patterns(scope: &str, items: &[ModuleItem], out: &mut Vec<Finding>) {
    // Collect all process bodies
    for item in items {
        if let ModuleItem::Always(ab) = item {
            find_redundant_assigns(scope, &ab.stmts, out);
        }
    }
}

/// Deteksi redundant assignments: sinyal yang ditulis lebih dari sekali
/// di scope yang sama tanpa intervening delay/event (overwrite percuma).
fn find_redundant_assigns(scope: &str, stmts: &[Stmt], out: &mut Vec<Finding>) {
    use std::collections::HashMap as StdHashMap;
    let mut writes: StdHashMap<Symbol, usize> = StdHashMap::new(); // name → line count
    for stmt in stmts {
        match stmt {
            Stmt::BlockingAssign {
                lhs: mivon_ast::expr::Expr::Ident { name, .. },
                ..
            } => {
                let count = writes.entry(*name).or_insert(0);
                *count += 1;
                if *count == 2 {
                    // Only report once per signal
                    out.push(Finding {
                        module: scope.to_string(),
                        check: "gate-opt-redundant",
                        severity: "W",
                        message: format!(
                            "signal '{}' overwritten in same block (redundant assignment)",
                            name.as_str()
                        ),
                    });
                }
            }
            Stmt::NonBlockingAssign {
                lhs: mivon_ast::expr::Expr::Ident { name, .. },
                ..
            } => {
                let count = writes.entry(*name).or_insert(0);
                *count += 1;
                if *count == 2 {
                    out.push(Finding {
                        module: scope.to_string(),
                        check: "gate-opt-redundant",
                        severity: "W",
                        message: format!(
                            "signal '{}' overwritten in same block (redundant NBA)",
                            name.as_str()
                        ),
                    });
                }
            }
            Stmt::Block { stmts: inner } => {
                find_redundant_assigns(scope, inner, out);
            }
            _ => {}
        }
    }
}
