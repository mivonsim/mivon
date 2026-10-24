//! `mlint` — Static RTL Linter.
//!
//! Check: unused signal, width mismatch, latch detection, combinational loop,
//! FSM state register.

mod cache;
mod case_analysis;
mod clock_gating;
mod fsm;
mod gate_opt;
mod latch;
mod memory;
mod power;
mod scan;
mod walk;
mod width;

use std::collections::{HashMap, HashSet};

use crate::{open_project, section};
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
    cache::save_lint_cache(args, &findings);

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
            let w = width::decl_width(d, v);
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
                scan::scan_stmt_reads(&block.stmts, &mut reads, &mut writes);
            }
            ModuleItem::Initial(block) => {
                scan::scan_stmt_reads(&block.stmts, &mut reads, &mut writes);
            }
            ModuleItem::Final(block) => {
                scan::scan_stmt_reads(&block.stmts, &mut reads, &mut writes);
            }
            ModuleItem::Assign(ca) => {
                scan::scan_expr_reads(&ca.lhs, &mut reads, &mut writes);
                scan::scan_expr_reads(&ca.rhs, &mut reads, &mut writes);
                if let Some(root) = scan::lvalue_root(&ca.lhs) {
                    writes.insert(root);
                }
                if do_width {
                    width::check_width(scope, &ca.lhs, &ca.rhs, &declared, out);
                }
            }
            ModuleItem::Generate(g) => {
                for gi in &g.items {
                    walk::walk_generate(
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
                latch::find_incomplete_if(scope, &block.stmts, out);
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
            scan::scan_stmt_reads(&block.stmts, &mut r, &mut w);
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
            fsm::find_fsm(scope, &block.stmts, out);
        }
    }

    // COMP-12: Case analysis — deteksi case tanpa default + unique/priority overlap
    if do_case_analysis {
        for block in &always_blocks {
            case_analysis::find_case_analysis(scope, &block.stmts, out);
        }
    }

    // COMP-13: Clock gating inference — deteksi always_ff dengan if-gate
    if do_clock_gating {
        for block in &always_blocks {
            if matches!(block.kind, mivon_ast::stmt::AlwaysKind::AlwaysFF) {
                clock_gating::find_clock_gating(scope, &block.stmts, out);
            }
        }
    }

    // COMP-14: Power optimization — deteksi UPF power domain patterns
    if do_power {
        power::find_power_patterns(scope, &declared, out);
    }

    // COMP-15: Memory inference — deteksi reg array patterns
    if do_memory {
        memory::find_memory_inference(scope, decls, items, out);
    }

    // COMP-10: Gate-level optimization — deteksi pattern yang bisa dioptimasi
    if do_gate_opt {
        gate_opt::find_gate_opt_patterns(scope, items, out);
    }
}
