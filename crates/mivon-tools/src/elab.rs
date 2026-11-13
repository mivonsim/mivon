//! `melab` — Standalone Elaborator.
//!
//! Hanya melakukan: parameter resolve, generate expansion, hierarchy.
//! Tidak menjalankan simulasi.

use crate::{kv, open_elaborated, section};
use mivon_core::error::SimError;
use mivon_elaboration::elaborator::ElaborateMode;

/// Opsi melab.
pub struct ElabArgs<'a> {
    pub files: &'a [String],
    pub incdirs: &'a [String],
    pub defines: &'a [String],
    pub top: Option<&'a str>,
    pub tree: bool,
    pub params: bool,
    pub signals: bool,
    /// SIM-22: analisis reset-domain crossing (RDC).
    pub reset_domain: bool,
    /// Baca hasil elaborasi dari cache pipeline (tanpa menjalankan elaborator).
    pub from_cache: bool,
}

/// Jalankan melab.
pub fn run(args: &ElabArgs) -> Result<(), SimError> {
    if args.from_cache {
        return run_from_cache(args);
    }
    // Use AnalysisRecovery mode for analysis tools (Rule 10)
    let (session, design, ir) = open_elaborated(
        args.files,
        args.incdirs,
        args.defines,
        args.top,
        ElaborateMode::AnalysisRecovery,
    )?;
    let top_name = ir.top.name.as_str();

    section("Elaboration Result");
    kv("top module", top_name);
    kv("modules", ir.modules.len());
    kv("signals (top)", ir.top.signals.len());
    kv("processes (top)", ir.top.processes.len());
    kv("classes", ir.classes.len());
    kv(
        "timescale",
        ir.timescale
            .as_ref()
            .map(|t| format!("{}/{}", t.0, t.1))
            .unwrap_or_else(|| "-".into()),
    );
    kv("parse time", format!("{} µs", session.timing.parse_us));
    kv("elab time", format!("{} µs", session.timing.elab_us));

    if args.tree {
        print_tree(&ir);
    }

    if args.params {
        section("Parameters (per module, default dari source)");
        let mut mods: Vec<(&str, &mivon_ast::types::Module)> = design
            .modules
            .iter()
            .map(|m| (m.name.as_str(), m))
            .collect();
        mods.sort_by_key(|(n, _)| *n);
        for (name, m) in mods {
            let mut params: Vec<&mivon_ast::types::ParamDecl> = m.params.iter().collect();
            for it in &m.items {
                if let mivon_ast::types::ModuleItem::Param(p) = it {
                    params.push(p);
                }
            }
            if params.is_empty() {
                continue;
            }
            println!("  {}", name);
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
                println!("    {:<10} {:<32}{}", kind, p.name.as_str(), default);
            }
        }
    }

    if args.reset_domain {
        // SIM-22: analisis reset-domain crossing pada IR ter-elaborasi.
        section("Reset-Domain Crossing (RDC)");
        let rdc = mivon_simulator::scheduler::reset_domain::ResetDomainAnalysis::analyze(&ir);
        print!("{}", rdc.report());
    }

    if args.signals {
        section("Signals (top module)");
        for s in &ir.top.signals {
            let kind = match s.kind {
                mivon_ir::SignalKind::Reg => "reg",
                mivon_ir::SignalKind::Wire => "wire",
                mivon_ir::SignalKind::Logic => "logic",
                mivon_ir::SignalKind::Input => "input",
                mivon_ir::SignalKind::Output => "output",
                mivon_ir::SignalKind::Inout => "inout",
            };
            println!(
                "  {:<8} {:<6} bits={:<4} name={}",
                kind,
                format!("[{}:0]", s.width - 1),
                s.width,
                s.name.as_str()
            );
        }
    }

    Ok(())
}

/// Cetak hierarchy tree dari IR (sub_instances).
fn print_tree(ir: &mivon_ir::IrDesign) {
    section("Hierarchy Tree");
    println!("  └── {}", ir.top.name.as_str());
    let mut visited: std::collections::HashSet<mivon_core::intern::Symbol> =
        std::collections::HashSet::new();
    for inst in &ir.top.sub_instances {
        print_inst(inst, ir, &mut visited, 1);
    }
}

fn print_inst(
    inst: &mivon_ir::IrInstance,
    ir: &mivon_ir::IrDesign,
    visited: &mut std::collections::HashSet<mivon_core::intern::Symbol>,
    depth: usize,
) {
    let prefix = "  ".repeat(depth + 1);
    println!(
        "{}{} ({}),",
        prefix,
        inst.instance_name.as_str(),
        inst.module_name.as_str()
    );
    if visited.contains(&inst.module_name) {
        println!("{}^ cycle", prefix);
        return;
    }
    if let Some(sub) = ir.modules.get(&inst.module_name) {
        visited.insert(inst.module_name);
        for c in &sub.sub_instances {
            print_inst(c, ir, visited, depth + 1);
        }
        visited.remove(&inst.module_name);
    }
}

/// Baca hasil elaborasi dari cache pipeline (db.md "5. elaborate/",
/// "16. generate/") tanpa menjalankan elaborator — hierarki, instance, port
/// binding, parameter override, proses, net resolution, dan blok generate
/// yang disimpan pada build sebelumnya ("1000 instance generate tidak perlu
/// dielaborasi ulang").
pub fn run_from_cache(args: &ElabArgs) -> Result<(), SimError> {
    use mivon_compiler::node_cache::cache::pipeline::{ElaboratePayload, GeneratePayload};
    use mivon_compiler::node_cache::cache::CacheCategory;

    let (mut layer, pid) = crate::open_cache_layer(args.files, args.incdirs, args.defines)?;

    let keys = layer
        .store(CacheCategory::Elaborate)
        .map(|s| s.keys())
        .unwrap_or_default();
    if keys.is_empty() {
        return Err(SimError::runtime(
            "cache elaborate/ kosong — jalankan `melab` (tanpa --from-cache) atau `mivon --fast` sekali dulu",
        ));
    }
    let mut keys = keys;
    keys.sort();

    section("Elaboration Result (dari cache)");
    kv("project id", &pid);
    kv("modules (cache)", keys.len());
    kv("sumber", "cache pipeline (tanpa elaborasi)");

    for name in &keys {
        let Some(bytes) = layer.get(CacheCategory::Elaborate, name) else {
            continue;
        };
        let Ok(elab) = bincode::deserialize::<ElaboratePayload>(&bytes) else {
            continue;
        };
        let gen = layer
            .get(CacheCategory::Generate, name)
            .and_then(|b| bincode::deserialize::<GeneratePayload>(&b).ok())
            .unwrap_or_default();

        println!();
        println!("  {}", name);
        kv("  instances", elab.instance_count);
        if gen.if_blocks > 0 || gen.for_blocks > 0 || gen.case_blocks > 0 {
            kv(
                "  generate",
                format!(
                    "if={} for={} case={} (expanded {})",
                    gen.if_blocks, gen.for_blocks, gen.case_blocks, gen.expanded_instances
                ),
            );
        }
        if elab.processes.combinational + elab.processes.sequential + elab.processes.initial > 0 {
            kv(
                "  processes",
                format!(
                    "comb={} seq={} initial={} final={} delay={}",
                    elab.processes.combinational,
                    elab.processes.sequential,
                    elab.processes.initial,
                    elab.processes.final_,
                    elab.processes.always_with_delay
                ),
            );
        }
        if args.signals {
            kv(
                "  net",
                format!(
                    "wire={} tri={} wor={} supply={}",
                    elab.net_counts.wire,
                    elab.net_counts.tri,
                    elab.net_counts.wor,
                    elab.net_counts.supply0 + elab.net_counts.supply1
                ),
            );
        }
        for inst in &elab.instances {
            let ov: Vec<String> = inst
                .param_overrides
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect();
            println!(
                "    {} {} ({} ports{}",
                inst.module,
                inst.instance,
                inst.port_bindings,
                if ov.is_empty() {
                    ")".to_string()
                } else {
                    format!(", param {})", ov.join(", "))
                }
            );
        }
    }

    if args.tree {
        section("Hierarchy (dari cache)");
        for name in &keys {
            if let Some(bytes) = layer.get(CacheCategory::Elaborate, name) {
                if let Ok(elab) = bincode::deserialize::<ElaboratePayload>(&bytes) {
                    if elab.instance_count == 0 {
                        continue;
                    }
                    println!("  {}", name);
                    for inst in &elab.instances {
                        println!("    └── {} ({})", inst.instance, inst.module);
                    }
                }
            }
        }
    }

    Ok(())
}
