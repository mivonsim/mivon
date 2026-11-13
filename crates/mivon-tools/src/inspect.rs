//! `minspect` — Mivon Inspect.
//!
//! X-ray cepat struktur project tanpa compile penuh (hanya parse paralel +
//! index). Subcommand: `stats`, `modules`, `hierarchy`, `packages`, `classes`,
//! `interfaces`, `parameters`, `deps`, `cache` (laporan lapisan cache node_cache).

use std::collections::{HashMap, HashSet};

use crate::{kv, open_project, section};
use mivon_ast::types::{GenerateItem, Module, ModuleItem, PortDirection, TypedefDecl};
use mivon_compiler::frontend::compile_session::CompileSession;
use mivon_compiler::frontend::module_index::EntryKind;
use mivon_compiler::node_cache::cache::CacheCategory;
use mivon_core::intern::Symbol;

/// Opsi minspect.
pub struct InspectArgs<'a> {
    pub targets: &'a [String],
    pub command: Option<String>,
    pub incdirs: &'a [String],
    pub defines: &'a [String],
    pub top: Option<&'a str>,
    pub json: bool,
}

/// Jalankan minspect sesuai subcommand.
pub fn run(args: &InspectArgs) -> Result<(), mivon_core::error::SimError> {
    let cmd = args.command.as_deref().unwrap_or("stats");
    // `cache` membaca lapisan `cache/<pid>/` (db.md) TANPA compile — cukup
    // buka database node_cache project dan lapor statistik per kategori.
    if cmd == "cache" {
        return cache_stats(args);
    }

    let (design, session) = open_project(args.targets, args.incdirs, args.defines, args.top)?;

    let json = args.json;
    match cmd {
        "stats" => stats(&design, &session, json),
        "modules" => modules(&design, &session, args.top),
        "hierarchy" => hierarchy(&design, args.top),
        "packages" => packages(&design),
        "classes" => classes(&design),
        "interfaces" => interfaces(&design),
        "parameters" => parameters(&design, args.top),
        "deps" => deps(&design),
        other => Err(mivon_core::error::SimError::with_diag(
            mivon_core::diagnostics::DiagCode::InvalidSyntax,
            format!(
                "subcommand minspect tidak dikenal: '{}' (pilih: stats, modules, hierarchy, packages, classes, interfaces, parameters, deps, cache)",
                other
            ),
        )),
    }
}

/// Laporan lapisan cache pipeline per kategori (db.md "Saran arsitektur
/// cache" + Kritik 14: statistik hit/miss/ukuran/umur per kategori).
/// Membuka database node_cache project (tanpa compile) dan membaca `cache/<pid>/`.
/// ProjectID dihitung dari target yang sama seperti `run`/`run_fast`, jadi
/// laporan menunjuk lapisan cache project yang sedang dikerjakan.
fn cache_stats(args: &InspectArgs) -> Result<(), mivon_core::error::SimError> {
    let files = crate::collect_targets(args.targets)?;
    let (mut layer, pid) = crate::open_cache_layer(args.targets, args.incdirs, args.defines)?;

    section("Pipeline Cache");
    kv(
        "root",
        mivon_compiler::node_cache::ProjectCache::default_root().display(),
    );
    kv("project id", &pid);
    kv("files", files.len());

    let st = layer.stats();
    println!();
    println!(
        "  {:<12} {:>8} {:>12} {:>7} {:>7} {:>7}",
        "Category", "Entries", "Bytes", "Hits", "Misses", "Hit%"
    );
    for cs in &st.per_category {
        println!(
            "  {:<12} {:>8} {:>12} {:>7} {:>7} {:>6}%",
            format!("{}/", cs.category.name()),
            cs.entries,
            crate::human_bytes(cs.bytes),
            cs.hits,
            cs.misses,
            cs.hit_rate_pct(),
        );
    }

    // Detail payload kategori hasil tool (lint/, coverage/) — dibaca tanpa
    // menjalankan tool ulang (db.md "19. coverage/", "7. verify/ → lint/").
    if layer.contains(CacheCategory::Lint, "report") {
        if let Some(bytes) = layer.get(CacheCategory::Lint, "report") {
            if let Ok(p) =
                bincode::deserialize::<mivon_compiler::node_cache::cache::pipeline::LintPayload>(&bytes)
            {
                let w = p.findings.iter().filter(|f| f.severity == "W").count();
                let e = p.findings.iter().filter(|f| f.severity == "E").count();
                section("Lint (dari cache)");
                kv("findings", p.findings.len());
                kv("warning", w);
                kv("error", e);
                for f in p.findings.iter().take(8) {
                    println!(
                        "    [{}] {:<10} {:<12} {}",
                        f.severity, f.check, f.module, f.message
                    );
                }
                if p.findings.len() > 8 {
                    println!("    … {} temuan lainnya", p.findings.len() - 8);
                }
            }
        }
    }
    if layer.contains(CacheCategory::Coverage, "last") {
        if let Some(bytes) = layer.get(CacheCategory::Coverage, "last") {
            if let Ok(p) = bincode::deserialize::<
                mivon_compiler::node_cache::cache::pipeline::CoveragePayload,
            >(&bytes)
            {
                let line_pct = if p.line_items > 0 {
                    p.line_hits as f64 / p.line_items as f64 * 100.0
                } else {
                    0.0
                };
                let branch_pct = if p.branch_total > 0 {
                    p.branch_covered as f64 / p.branch_total as f64 * 100.0
                } else {
                    0.0
                };
                section("Coverage (dari cache)");
                kv(
                    "line",
                    format!("{}/{} ({:.1}%)", p.line_hits, p.line_items, line_pct),
                );
                kv(
                    "branch",
                    format!(
                        "{}/{} ({:.1}%)",
                        p.branch_covered, p.branch_total, branch_pct
                    ),
                );
                kv(
                    "toggle",
                    format!(
                        "{} signals, {} transitions",
                        p.toggle_signals, p.toggle_transitions
                    ),
                );
                kv(
                    "fsm",
                    format!("{} signals, {} states", p.fsm_signals, p.fsm_states),
                );
            }
        }
    }
    if layer.contains(CacheCategory::Simulation, "last") {
        if let Some(bytes) = layer.get(CacheCategory::Simulation, "last") {
            if let Ok(p) = bincode::deserialize::<
                mivon_compiler::node_cache::cache::pipeline::SimulationPayload,
            >(&bytes)
            {
                section("Simulation (dari cache)");
                kv("end time", format!("#{}", p.end_time));
                kv("events processed", p.events_processed);
                kv(
                    "signals",
                    format!("{} ({} init non-zero)", p.signal_count, p.init_signals),
                );
                kv(
                    "sensitivity",
                    format!(
                        "{} comb, {} seq, {} initial, {} final, {} delay",
                        p.processes.combinational,
                        p.processes.sequential,
                        p.processes.initial,
                        p.processes.final_,
                        p.processes.always_with_delay
                    ),
                );
            }
        }
    }
    if layer.contains(CacheCategory::Waveform, "last") {
        if let Some(bytes) = layer.get(CacheCategory::Waveform, "last") {
            if let Ok(p) = bincode::deserialize::<
                mivon_compiler::node_cache::cache::pipeline::WaveformPayload,
            >(&bytes)
            {
                section("Waveform (dari cache)");
                kv("signals", p.signals.len());
                for s in p.signals.iter().take(8) {
                    println!(
                        "    {:<24} {} bit  {:<6} net={} {}",
                        s.name,
                        s.width,
                        s.kind,
                        s.net,
                        if s.is_signed { "signed" } else { "" }
                    );
                }
                if p.signals.len() > 8 {
                    println!("    … {} signal lainnya", p.signals.len() - 8);
                }
            }
        }
    }
    if layer.contains(CacheCategory::Optimize, "last") {
        if let Some(bytes) = layer.get(CacheCategory::Optimize, "last") {
            if let Ok(p) = bincode::deserialize::<
                mivon_compiler::node_cache::cache::pipeline::OptimizePayload,
            >(&bytes)
            {
                section("Optimize (dari cache)");
                kv("constant folds", p.const_folds);
                kv("loop unrolls", p.loop_unrolls);
                kv("unrolled stmts", p.unrolled_stmts);
            }
        }
    }
    if layer.contains(CacheCategory::Expression, "last") {
        if let Some(bytes) = layer.get(CacheCategory::Expression, "last") {
            if let Ok(p) = bincode::deserialize::<
                mivon_compiler::node_cache::cache::pipeline::ExpressionPayload,
            >(&bytes)
            {
                section("Expression (dari cache)");
                kv("expr evals", p.expr_evals);
                for (e, v) in p.samples.iter().take(8) {
                    println!("    {} → {}", e, v);
                }
            }
        }
    }

    // ── Precompiled modules (VCS AN.DB / Questa _info analog) ──
    {
        use mivon_compiler::node_cache::ProjectCache;
        let micd_root = ProjectCache::default_root();
        let all_files = crate::collect_targets(args.targets)?;
        let pid = ProjectCache::project_id(
            &std::env::current_dir().unwrap_or_default(),
            &all_files,
            &args
                .incdirs
                .iter()
                .map(std::path::PathBuf::from)
                .collect::<Vec<_>>(),
            &args
                .defines
                .iter()
                .filter_map(|d| d.split_once('='))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<Vec<_>>(),
        );
        let pdb_root = micd_root.join("precompiled").join(&pid);
        let pdb = mivon_compiler::node_cache::PrecompiledDb::open(&pdb_root);
        let pst = pdb.stats();
        section("Precompiled Modules");
        kv("modules", pst.modules);
        kv("clean", pst.clean_modules);
        kv("tokens", pst.total_tokens);
        kv("errors", pst.total_errors);
        kv("warnings", pst.total_warnings);
        for (name, m) in pdb.modules.iter().take(12) {
            println!(
                "    {:<24} hash={:016x} ports={} deps={}",
                name,
                m.content_hash,
                m.ports.len(),
                m.depends_on.len()
            );
        }
        if pdb.len() > 12 {
            println!("    ... and {} more", pdb.len() - 12);
        }
    }

    section("Cache Summary");
    kv("categories", st.stores);
    kv("entries", st.total_entries);
    kv("bytes", crate::human_bytes(st.total_bytes));
    kv("hit rate", format!("{}%", st.hit_rate_pct()));
    kv("rebuilt", st.rebuilt);
    Ok(())
}

/// Kumpulkan module + metadata ringkas dari design.
fn module_map(design: &mivon_ast::types::Design) -> HashMap<Symbol, Module> {
    design.modules.iter().map(|m| (m.name, m.clone())).collect()
}

/// Semua nama module yang di-instantiate (langsung) oleh module lain.
fn instantiated_set(design: &mivon_ast::types::Design) -> HashSet<Symbol> {
    let mut set = HashSet::new();
    for m in &design.modules {
        let mut insts = Vec::new();
        collect_instances(&m.items, &mut insts);
        set.extend(insts);
    }
    set
}

/// Kumpulkan instance module dari ModuleItem tree (rekursif ke generate).
fn collect_instances(items: &[ModuleItem], out: &mut Vec<Symbol>) {
    for it in items {
        match it {
            ModuleItem::Instance(inst) => out.push(inst.module_name),
            ModuleItem::Generate(g) => {
                for gi in &g.items {
                    collect_generate_instances(gi, out);
                }
            }
            _ => {}
        }
    }
}

fn collect_generate_instances(gi: &GenerateItem, out: &mut Vec<Symbol>) {
    match gi {
        GenerateItem::Items(items) => collect_instances(items, out),
        GenerateItem::If {
            true_items,
            false_items,
            ..
        } => {
            collect_instances(true_items, out);
            collect_instances(false_items, out);
        }
        GenerateItem::For { body_items, .. } => collect_instances(body_items, out),
        GenerateItem::Case { items, default, .. } => {
            for ci in items {
                collect_instances(&ci.body, out);
            }
            if let Some(d) = default {
                collect_instances(d, out);
            }
        }
    }
}

/// Hitung statistik dari item tree (rekursif generate).
fn count_typedefs(items: &[ModuleItem]) -> usize {
    let mut n = 0;
    for it in items {
        match it {
            ModuleItem::Typedef(_) => n += 1,
            ModuleItem::Generate(g) => {
                for gi in &g.items {
                    n += count_typedefs_generate(gi);
                }
            }
            _ => {}
        }
    }
    n
}

fn count_typedefs_generate(gi: &GenerateItem) -> usize {
    match gi {
        GenerateItem::Items(items) => count_typedefs(items),
        GenerateItem::If {
            true_items,
            false_items,
            ..
        } => count_typedefs(true_items) + count_typedefs(false_items),
        GenerateItem::For { body_items, .. } => count_typedefs(body_items),
        GenerateItem::Case { items, default, .. } => {
            items
                .iter()
                .map(|ci| count_typedefs(&ci.body))
                .sum::<usize>()
                + default.as_deref().map(count_typedefs).unwrap_or(0)
        }
    }
}

fn count_generates(items: &[ModuleItem]) -> usize {
    let mut n = 0;
    for it in items {
        if let ModuleItem::Generate(_) = it {
            n += 1;
        }
    }
    n
}

fn count_params(module: &Module) -> usize {
    let mut n = module.params.len();
    for it in &module.items {
        if let ModuleItem::Param(_) = it {
            n += 1;
        }
    }
    n
}

/// Statistik project.
fn stats(
    design: &mivon_ast::types::Design,
    session: &CompileSession,
    json: bool,
) -> Result<(), mivon_core::error::SimError> {
    let mut generate = 0usize;
    let mut parameters = 0usize;
    let mut typedefs = 0usize;
    let mut item_counts: Vec<(Symbol, usize)> = Vec::new();

    for m in &design.modules {
        generate += count_generates(&m.items);
        parameters += count_params(m);
        typedefs += count_typedefs(&m.items);
        let mut n = m.items.len();
        for gi in &m.items {
            n += item_count_deep(gi);
        }
        item_counts.push((m.name, n));
    }
    // Statistik tambahan: package & interface items
    for p in &design.packages {
        parameters += p
            .items
            .iter()
            .filter(|i| matches!(i, mivon_ast::types::PackageItem::Param(_)))
            .count();
        typedefs += p
            .items
            .iter()
            .filter(|i| matches!(i, mivon_ast::types::PackageItem::Typedef(_)))
            .count();
    }
    for i in &design.interfaces {
        parameters += i.params.len();
        typedefs += count_typedefs(&i.items);
    }

    let inst_set = instantiated_set(design);
    let mut tops: Vec<Symbol> = design
        .modules
        .iter()
        .map(|m| m.name)
        .filter(|n| !inst_set.contains(n))
        .collect();
    tops.sort_by_key(|s| s.as_str());

    let total_items: usize = item_counts.iter().map(|(_, n)| *n).sum();
    let largest = item_counts
        .iter()
        .max_by_key(|(_, n)| *n)
        .map(|(n, c)| (n.as_str().to_string(), *c));

    if json {
        use serde_json::json;
        let obj = json!({
            "modules": design.modules.len(),
            "interfaces": design.interfaces.len(),
            "packages": design.packages.len(),
            "classes": design.classes.len(),
            "generate": generate,
            "parameters": parameters,
            "typedefs": typedefs,
            "files": session.source_count(),
            "top_modules": tops.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            "largest_module": largest.map(|(n, c)| json!({"name": n, "items": c})),
            "total_items": total_items,
        });
        println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
        return Ok(());
    }

    section("Project Statistics");
    kv("Modules", design.modules.len());
    kv("Interfaces", design.interfaces.len());
    kv("Packages", design.packages.len());
    kv("Classes", design.classes.len());
    kv("Generate", generate);
    kv("Parameters", parameters);
    kv("Typedefs", typedefs);
    kv("Files", session.source_count());

    section("Top Modules");
    if tops.is_empty() {
        println!("  (tidak ada — semua module di-instantiate)");
    }
    for t in tops.iter().take(50) {
        println!("  {}", t.as_str());
    }
    if tops.len() > 50 {
        println!("  … dan {} lagi", tops.len() - 50);
    }

    section("Largest Module");
    if let Some((name, count)) = largest {
        kv("name", name);
        kv("items", count);
    } else {
        println!("  (tidak ada module)");
    }

    section("Average Items/module");
    if !design.modules.is_empty() {
        kv("items", total_items / design.modules.len());
    }

    Ok(())
}

fn item_count_deep(item: &ModuleItem) -> usize {
    match item {
        ModuleItem::Generate(g) => g.items.iter().map(count_generate_deep).sum::<usize>(),
        _ => 0,
    }
}

fn count_generate_deep(gi: &GenerateItem) -> usize {
    let mut n = 1;
    match gi {
        GenerateItem::Items(items) => n += items.iter().map(item_count_deep).sum::<usize>(),
        GenerateItem::If {
            true_items,
            false_items,
            ..
        } => {
            n += true_items.iter().map(item_count_deep).sum::<usize>();
            n += false_items.iter().map(item_count_deep).sum::<usize>();
        }
        GenerateItem::For { body_items, .. } => {
            n += body_items.iter().map(item_count_deep).sum::<usize>();
        }
        GenerateItem::Case { items, default, .. } => {
            for ci in items {
                n += ci.body.iter().map(item_count_deep).sum::<usize>();
            }
            if let Some(d) = default {
                n += d.iter().map(item_count_deep).sum::<usize>();
            }
        }
    }
    n
}

/// Daftar module + lokasi file + jumlah port/param.
fn modules(
    design: &mivon_ast::types::Design,
    session: &CompileSession,
    top: Option<&str>,
) -> Result<(), mivon_core::error::SimError> {
    let mut mods: Vec<(Symbol, Module)> =
        design.modules.iter().map(|m| (m.name, m.clone())).collect();
    mods.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));

    section("Modules");
    for (name, m) in &mods {
        let file = session
            .module_index
            .lookup(*name, EntryKind::Module)
            .map(|meta| meta.file.display().to_string())
            .unwrap_or_else(|| "?".into());
        let has_top = top
            .map(|t| t == name.as_str())
            .unwrap_or_else(|| design.top_module.map(|t| t == *name).unwrap_or(false));
        let marker = if has_top { " (top)" } else { "" };
        println!(
            "  {:<40} ports={:<3} params={:<3} {}  [{}]",
            name.as_str(),
            m.ports.len(),
            count_params(m),
            marker,
            file
        );
    }
    Ok(())
}

/// Hierarchy tree dari top module (atau semua top bila tidak ditentukan).
fn hierarchy(
    design: &mivon_ast::types::Design,
    top: Option<&str>,
) -> Result<(), mivon_core::error::SimError> {
    let mods = module_map(design);
    let inst_set = instantiated_set(design);

    let roots: Vec<Symbol> = if let Some(t) = top {
        vec![Symbol::intern(t)]
    } else {
        let mut r: Vec<Symbol> = mods
            .keys()
            .copied()
            .filter(|n| !inst_set.contains(n))
            .collect();
        r.sort_by_key(|s| s.as_str());
        r
    };
    // Bangun adjacency: module → children module names
    let mut adj: HashMap<Symbol, Vec<Symbol>> = HashMap::new();
    for (name, m) in &mods {
        let mut insts = Vec::new();
        collect_instances(&m.items, &mut insts);
        insts.sort_by_key(|s| s.as_str());
        adj.insert(*name, insts);
    }

    section("Module Hierarchy");
    let mut visited: HashSet<Symbol> = HashSet::new();
    for root in &roots {
        print_tree(*root, &mods, &adj, &mut visited, 0);
    }
    Ok(())
}

fn print_tree(
    name: Symbol,
    mods: &HashMap<Symbol, Module>,
    adj: &HashMap<Symbol, Vec<Symbol>>,
    visited: &mut HashSet<Symbol>,
    depth: usize,
) {
    let prefix = "  ".repeat(depth);
    let known = mods.contains_key(&name);
    let marker = if known { "" } else { " (MISSING)" };
    println!("{}{}{}", prefix, name.as_str(), marker);
    if visited.contains(&name) {
        println!("{}^ cycle", prefix);
        return;
    }
    if !known {
        return;
    }
    visited.insert(name);
    if let Some(children) = adj.get(&name) {
        for c in children {
            print_tree(*c, mods, adj, visited, depth + 1);
        }
    }
    visited.remove(&name);
}

/// Daftar package.
fn packages(design: &mivon_ast::types::Design) -> Result<(), mivon_core::error::SimError> {
    section("Packages");
    let mut pkgs: Vec<_> = design.packages.iter().collect();
    pkgs.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    for p in pkgs {
        let n_items = p.items.len();
        let n_typedefs = p
            .items
            .iter()
            .filter(|i| matches!(i, mivon_ast::types::PackageItem::Typedef(_)))
            .count();
        let n_params = p
            .items
            .iter()
            .filter(|i| matches!(i, mivon_ast::types::PackageItem::Param(_)))
            .count();
        let n_classes = p
            .items
            .iter()
            .filter(|i| matches!(i, mivon_ast::types::PackageItem::Class(_)))
            .count();
        println!(
            "  {:<40} items={:<4} typedefs={:<4} params={:<4} classes={}",
            p.name.as_str(),
            n_items,
            n_typedefs,
            n_params,
            n_classes
        );
    }
    Ok(())
}

/// Daftar class.
fn classes(design: &mivon_ast::types::Design) -> Result<(), mivon_core::error::SimError> {
    section("Classes");
    let mut cls: Vec<_> = design.classes.iter().collect();
    cls.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    for c in cls {
        let base = c
            .extends
            .map(|e| e.as_str().to_string())
            .unwrap_or_default();
        let n_members = c.members.len();
        println!(
            "  {:<40} members={:<4} extends={}",
            c.name.as_str(),
            n_members,
            base
        );
    }
    Ok(())
}

/// Daftar interface + modport.
fn interfaces(design: &mivon_ast::types::Design) -> Result<(), mivon_core::error::SimError> {
    section("Interfaces");
    let mut ifs: Vec<_> = design.interfaces.iter().collect();
    ifs.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    for i in ifs {
        let modports: Vec<&str> = i.modports.iter().map(|m| m.name.as_str()).collect();
        println!(
            "  {:<40} ports={:<3} modports={}",
            i.name.as_str(),
            i.ports.len(),
            modports.join(", ")
        );
    }
    Ok(())
}

/// Parameter per module.
fn parameters(
    design: &mivon_ast::types::Design,
    _top: Option<&str>,
) -> Result<(), mivon_core::error::SimError> {
    section("Parameters");
    let mut mods: Vec<(Symbol, Module)> =
        design.modules.iter().map(|m| (m.name, m.clone())).collect();
    mods.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));

    for (name, m) in &mods {
        let mut params: Vec<&mivon_ast::types::ParamDecl> = m.params.iter().collect();
        for it in &m.items {
            if let ModuleItem::Param(p) = it {
                params.push(p);
            }
        }
        if params.is_empty() {
            continue;
        }
        println!("  {}", name.as_str());
        for p in params {
            let kind = if p.is_localparam {
                "localparam"
            } else {
                "parameter"
            };
            let default = p
                .default
                .as_ref()
                .map(|e| format!(" = {}", crate::expr_to_string(e)))
                .unwrap_or_default();
            println!("    {:<8} {:<32}{}", kind, p.name.as_str(), default);
        }
    }
    Ok(())
}

/// Dependency antar module (module → module yang di-instance).
fn deps(design: &mivon_ast::types::Design) -> Result<(), mivon_core::error::SimError> {
    section("Module Dependencies");
    let mods = module_map(design);
    let mut entries: Vec<(Symbol, Vec<Symbol>)> = Vec::new();
    for (name, m) in &mods {
        let mut insts = Vec::new();
        collect_instances(&m.items, &mut insts);
        insts.sort_by_key(|s| s.as_str());
        insts.dedup();
        entries.push((*name, insts));
    }
    entries.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));

    for (name, insts) in &entries {
        if insts.is_empty() {
            continue;
        }
        let deps_str: Vec<&str> = insts.iter().map(|s| s.as_str()).collect();
        println!("  {:<40} → {}", name.as_str(), deps_str.join(", "));
    }

    // Unresolved dependency (module tidak ditemukan)
    let known: HashSet<Symbol> = mods.keys().copied().collect();
    let mut missing: HashSet<Symbol> = HashSet::new();
    for (_, insts) in &entries {
        for i in insts {
            if !known.contains(i) {
                missing.insert(*i);
            }
        }
    }
    if !missing.is_empty() {
        section("Unresolved Dependencies");
        let mut miss: Vec<&str> = missing.iter().map(|s| s.as_str()).collect();
        miss.sort();
        for m in miss {
            println!("  ! {}", m);
        }
    }
    Ok(())
}

// ── Typedef helpers (dipakai untuk menghitung typedef dalam deps/modules) ──
#[allow(dead_code)]
fn _typedef_names(items: &[ModuleItem], out: &mut Vec<TypedefDecl>) {
    for it in items {
        match it {
            ModuleItem::Typedef(t) => out.push(t.clone()),
            ModuleItem::Generate(g) => {
                for gi in &g.items {
                    _typedef_names_generate(gi, out);
                }
            }
            _ => {}
        }
    }
}

fn _typedef_names_generate(gi: &GenerateItem, out: &mut Vec<TypedefDecl>) {
    match gi {
        GenerateItem::Items(items) => _typedef_names(items, out),
        GenerateItem::If {
            true_items,
            false_items,
            ..
        } => {
            _typedef_names(true_items, out);
            _typedef_names(false_items, out);
        }
        GenerateItem::For { body_items, .. } => _typedef_names(body_items, out),
        GenerateItem::Case { items, default, .. } => {
            for ci in items {
                _typedef_names(&ci.body, out);
            }
            if let Some(d) = default {
                _typedef_names(d, out);
            }
        }
    }
}

#[allow(dead_code)]
fn _port_direction(dir: PortDirection) -> &'static str {
    match dir {
        PortDirection::Input => "input",
        PortDirection::Output => "output",
        PortDirection::Inout => "inout",
        PortDirection::Ref => "ref",
    }
}
