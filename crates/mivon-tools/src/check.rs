//! `mcheck` — Project Health Checker.
//!
//! Memeriksa: missing file (`include), circular include, unresolved dependency,
//! module instantiation cycle, inkonsistensi timescale.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::gen::is_mv_source;
use crate::{collect_targets, kv, section};
use mivon_ast::types::GenerateItem;
use mivon_core::error::SimError;

/// Opsi mcheck.
pub struct CheckArgs<'a> {
    pub targets: &'a [String],
    pub all: bool,
    pub missing: bool,
    pub circular: bool,
    pub deps: bool,
    pub cycles: bool,
    pub timescale: bool,
    /// PARSER-13: bandingkan AST target pertama dgn file ini (structural diff).
    pub ast_diff: Option<&'a str>,
    /// ENT-22: Check SV version compatibility.
    pub sv_version: bool,
}

/// Jalankan mcheck.
pub fn run(args: &CheckArgs) -> Result<(), SimError> {
    let all = args.all;
    let check = |flag: bool| flag || all;

    // PARSER-13: AST differential — mode khusus, tidak jalan bareng check lain.
    if let Some(other) = args.ast_diff {
        return run_ast_diff(args.targets, other);
    }
    // ENT-22: Version compatibility check.
    if args.sv_version {
        return run_sv_version_check(args.targets);
    }

    let mut problems = 0usize;

    if check(args.missing) || check(args.circular) {
        problems += scan_includes(args.targets, check(args.missing), check(args.circular))?;
    }
    if check(args.deps) || check(args.cycles) || check(args.timescale) {
        problems += scan_design(
            args.targets,
            check(args.deps),
            check(args.cycles),
            check(args.timescale),
        )?;
    }

    if problems == 0 {
        println!("✅ project sehat — tidak ada masalah ditemukan");
    } else {
        println!("\n⚠️  {} masalah ditemukan", problems);
    }
    Ok(())
}

/// Representasi include per file.
struct IncludeInfo {
    /// Includes yang berhasil di-resolve (path absolut).
    resolved: Vec<PathBuf>,
    /// Include yang tidak ditemukan.
    missing: Vec<String>,
}

/// Scan semua file: cek `include missing + circular include graph.
fn scan_includes(
    targets: &[String],
    do_missing: bool,
    do_circular: bool,
) -> Result<usize, SimError> {
    let files = collect_targets(targets)?;
    if files.is_empty() {
        return Ok(0);
    }

    // Build include graph + missing list
    let mut graph: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    let mut missing_total = 0usize;
    let mut missing_seen: HashSet<PathBuf> = HashSet::new();

    section("Include Scan");
    let mut mv_skipped = 0usize;
    for path in &files {
        // `.mv`/`.mvh` tidak punya preprocessor (lexer `.mv` menolak
        // backtick sebagai karakter tak dikenal), jadi tidak ada directive
        // `include` yang bisa dipindai. Memindai mentah `.mv` akan
        // melaporkan "tidak ada include" — menyesatkan. Lewati + laporkan.
        if is_mv_source(path) {
            mv_skipped += 1;
            continue;
        }
        let info = analyze_includes(path, &files)?;
        graph.insert(path.clone(), info.resolved.clone());
        if do_missing {
            for m in &info.missing {
                println!(
                    "  ! MISSING INCLUDE: {} (`include \"{}\")",
                    path.display(),
                    m
                );
                missing_total += 1;
            }
        }
        missing_seen.extend(info.missing.iter().map(PathBuf::from));
    }
    if mv_skipped > 0 {
        kv("file .mv (tanpa preprocessor, dilewati)", mv_skipped);
    }

    let mut problem = 0usize;

    if do_missing {
        kv("missing includes", missing_total);
        problem += missing_total;
    }

    if do_circular {
        let mut cycles = 0usize;
        let cycle = find_include_cycle(&graph, &files);
        if let Some(c) = cycle {
            println!("  ! CIRCULAR INCLUDE:");
            for p in &c {
                println!("      {}", p.display());
            }
            cycles += 1;
        }
        kv("circular include cycles", cycles);
        problem += cycles;
    }

    Ok(problem)
}

/// Ekstrak `include directive dari satu file + resolve terhadap dir/incdirs.
fn analyze_includes(path: &Path, all_files: &[PathBuf]) -> Result<IncludeInfo, SimError> {
    let src = std::fs::read_to_string(path).map_err(|e| {
        SimError::with_diag(
            mivon_core::diagnostics::DiagCode::IoError,
            format!("{}: {}", path.display(), e),
        )
    })?;

    let file_set: HashSet<&Path> = all_files.iter().map(|p| p.as_path()).collect();
    let dir = path.parent().unwrap_or(Path::new("."));

    let mut resolved = Vec::new();
    let mut missing = Vec::new();

    for line in src.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("`include") && !trimmed.starts_with("`INCLUDE") {
            continue;
        }
        // Ekstrak nama file antara tanda kutip
        let Some(open) = trimmed.find('"') else {
            continue;
        };
        let rest = &trimmed[open + 1..];
        let Some(close) = rest.find('"') else {
            continue;
        };
        let name = &rest[..close];
        if name.is_empty() {
            continue;
        }

        // Cari: direktori file dulu, lalu cari di semua file yang di-scan
        let cand = dir.join(name);
        if cand.exists() {
            let abs = std::fs::canonicalize(&cand).unwrap_or(cand);
            resolved.push(abs);
        } else if let Some(hit) = all_files
            .iter()
            .find(|f| f.file_name().map(|n| n == name).unwrap_or(false))
        {
            let abs = std::fs::canonicalize(hit).unwrap_or_else(|_| hit.clone());
            resolved.push(abs);
        } else if file_set.contains(cand.as_path()) {
            resolved.push(cand);
        } else {
            missing.push(name.to_string());
        }
    }
    Ok(IncludeInfo { resolved, missing })
}

/// Deteksi cycle pada include graph via DFS coloring.
fn find_include_cycle(
    graph: &HashMap<PathBuf, Vec<PathBuf>>,
    files: &[PathBuf],
) -> Option<Vec<PathBuf>> {
    #[derive(Clone, Copy, PartialEq)]
    enum Color {
        White,
        Gray,
        Black,
    }
    let mut color: HashMap<PathBuf, Color> = HashMap::new();
    for f in files {
        color.insert(f.clone(), Color::White);
    }

    fn dfs(
        u: &PathBuf,
        graph: &HashMap<PathBuf, Vec<PathBuf>>,
        color: &mut HashMap<PathBuf, Color>,
        stack: &mut Vec<PathBuf>,
    ) -> Option<Vec<PathBuf>> {
        color.insert(u.clone(), Color::Gray);
        stack.push(u.clone());
        if let Some(neighbors) = graph.get(u) {
            for v in neighbors {
                match color.get(v).copied().unwrap_or(Color::White) {
                    Color::Gray => {
                        let start = stack.iter().position(|x| x == v).unwrap_or(0);
                        let mut cycle = stack[start..].to_vec();
                        cycle.push(v.clone());
                        return Some(cycle);
                    }
                    Color::White => {
                        if let Some(c) = dfs(v, graph, color, stack) {
                            return Some(c);
                        }
                    }
                    Color::Black => {}
                }
            }
        }
        stack.pop();
        color.insert(u.clone(), Color::Black);
        None
    }

    let mut stack = Vec::new();
    for f in files {
        if color.get(f).copied().unwrap_or(Color::White) == Color::White {
            if let Some(c) = dfs(f, graph, &mut color, &mut stack) {
                return Some(c);
            }
        }
    }
    None
}

/// Cek berbasis AST: unresolved dependency, module cycle, timescale.
fn scan_design(
    targets: &[String],
    do_deps: bool,
    do_cycles: bool,
    do_timescale: bool,
) -> Result<usize, SimError> {
    let (design, session) = crate::open_project(targets, &[], &[], None)?;
    let mut problem = 0usize;

    let mods: HashMap<mivon_core::intern::Symbol, _> =
        design.modules.iter().map(|m| (m.name, m.clone())).collect();

    if do_deps {
        section("Dependency Check");
        let mut known: HashSet<mivon_core::intern::Symbol> = mods.keys().copied().collect();
        for i in &design.interfaces {
            known.insert(i.name);
        }
        let mut unresolved: Vec<String> = Vec::new();
        for m in &design.modules {
            let mut insts = Vec::new();
            collect_mod_instances(&m.items, &mut insts);
            for i in insts {
                if !known.contains(&i) {
                    let s = i.as_str().to_string();
                    if !unresolved.contains(&s) {
                        unresolved.push(s);
                    }
                }
            }
        }
        unresolved.sort();
        for u in &unresolved {
            println!(
                "  ! UNRESOLVED: module '{}' di-instantiate tapi tidak terdefinisi",
                u
            );
        }
        kv("unresolved modules", unresolved.len());
        problem += unresolved.len();
    }

    if do_cycles {
        section("Instantiation Cycle Check");
        if let Some(cycle) = session.module_index.detect_cycles() {
            let path = cycle.join(" → ");
            println!("  ! CYCLE: {}", path);
            problem += 1;
        } else {
            kv("cycles", 0);
        }
    }

    if do_timescale {
        section("Timescale Check");
        let mut seen: HashMap<String, usize> = HashMap::new();
        for m in &design.modules {
            // Pakai timescale design (dari file terakhir) — cek per module tidak
            // punya info timescale; verifikasi lewat Preprocessor per file.
            let _ = m;
        }
        if let Some(ts) = &design.timescale {
            let key = format!("{}/{}", ts.0, ts.1);
            *seen.entry(key).or_insert(0) += 1;
        }
        // Scan tiap file untuk `timescale
        let files = collect_targets(targets)?;
        for f in &files {
            // `.mv`/`.mvh` tidak punya directive backtick — lewat saja.
            if is_mv_source(f) {
                continue;
            }
            if let Ok(src) = std::fs::read_to_string(f) {
                for line in src.lines() {
                    let t = line.trim();
                    if t.starts_with('`') && t.contains("timescale") {
                        // Ambil dua token setelah backtick-timescale
                        let after = t.replacen('`', "", 1);
                        let parts: Vec<&str> = after
                            .split_whitespace()
                            .filter(|w| !w.is_empty() && *w != "timescale")
                            .collect();
                        if parts.len() >= 2 {
                            let key = format!("{}/{}", parts[0].trim_matches('`'), parts[1]);
                            *seen.entry(key).or_insert(0) += 1;
                        }
                        break;
                    }
                }
            }
        }
        if seen.len() > 1 {
            let mut list: Vec<String> = seen
                .iter()
                .map(|(k, v)| format!("{} ({} file)", k, v))
                .collect();
            list.sort();
            for l in &list {
                println!("  ! {}", l);
            }
            println!("  ! INKONSISTENSI timescale: {} nilai berbeda", seen.len());
            problem += seen.len().saturating_sub(1);
        } else if let Some((k, v)) = seen.iter().next() {
            kv("timescale", format!("{} ({} file)", k, v));
        }
    }

    Ok(problem)
}

fn collect_mod_instances(
    items: &[mivon_ast::types::ModuleItem],
    out: &mut Vec<mivon_core::intern::Symbol>,
) {
    use mivon_ast::types::ModuleItem;
    for it in items {
        match it {
            ModuleItem::Instance(inst) => out.push(inst.module_name),
            ModuleItem::Generate(g) => {
                for gi in &g.items {
                    collect_gen_instances(gi, out);
                }
            }
            _ => {}
        }
    }
}

fn collect_gen_instances(gi: &GenerateItem, out: &mut Vec<mivon_core::intern::Symbol>) {
    match gi {
        GenerateItem::Items(items) => collect_mod_instances(items, out),
        GenerateItem::If {
            true_items,
            false_items,
            ..
        } => {
            collect_mod_instances(true_items, out);
            collect_mod_instances(false_items, out);
        }
        GenerateItem::For { body_items, .. } => collect_mod_instances(body_items, out),
        GenerateItem::Case { items, default, .. } => {
            for ci in items {
                collect_mod_instances(&ci.body, out);
            }
            if let Some(d) = default {
                collect_mod_instances(d, out);
            }
        }
    }
}

/// PARSER-13: AST differential — bandingkan AST elaborasi dua file .sv,
/// cetak perbedaan struktural (module/signal/proses/instance). Exit code
/// 0 bila identik, 1 bila ada perbedaan (utk regression gate).
/// PARSER-13: bandingkan dua IrDesign (structural: module/signal/proses/
/// class/covergroup) — padanan compare_asts API tanpa depend mivon-api.
fn compare_ir_designs(a: &mivon_ir::IrDesign, b: &mivon_ir::IrDesign) -> Vec<String> {
    let mut diffs = Vec::new();
    if a.modules.len() != b.modules.len() {
        diffs.push(format!(
            "module count: {} vs {}",
            a.modules.len(),
            b.modules.len()
        ));
    }
    if a.top.signals.len() != b.top.signals.len() {
        diffs.push(format!(
            "top signal count: {} vs {}",
            a.top.signals.len(),
            b.top.signals.len()
        ));
    }
    if a.top.processes.len() != b.top.processes.len() {
        diffs.push(format!(
            "process count: {} vs {}",
            a.top.processes.len(),
            b.top.processes.len()
        ));
    }
    // Bandingkan signal per-NAMA (bukan pairwise index): index-pairing
    // menyesatkan saat urutan/set signal dua design beda — diff palsu dan
    // ASIMETRIS A→B != B→A (ditemukan mivon-fuzz target astdiff). Cocokkan
    // by nama, laporkan only-in-A/B eksplisit. Sort by nama → deterministik.
    {
        use std::collections::BTreeMap;
        let ma = a
            .top
            .signals
            .iter()
            .map(|s| (s.name.to_string(), s))
            .collect::<BTreeMap<String, &mivon_ir::SignalInfo>>();
        let mb = b
            .top
            .signals
            .iter()
            .map(|s| (s.name.to_string(), s))
            .collect::<BTreeMap<String, &mivon_ir::SignalInfo>>();
        let names: std::collections::BTreeSet<String> =
            ma.keys().chain(mb.keys()).cloned().collect();
        for name in &names {
            match (ma.get(name), mb.get(name)) {
                (Some(sa), Some(sb)) => {
                    if sa.width != sb.width {
                        diffs.push(format!(
                            "signal '{}' width: {} vs {}",
                            name, sa.width, sb.width
                        ));
                    }
                    if sa.is_signed != sb.is_signed {
                        diffs.push(format!(
                            "signal '{}' signed: {} vs {}",
                            name, sa.is_signed, sb.is_signed
                        ));
                    }
                }
                (Some(_), None) => diffs.push(format!("signal '{}' only in A", name)),
                (None, Some(_)) => diffs.push(format!("signal '{}' only in B", name)),
                _ => {}
            }
        }
    }
    if a.classes.len() != b.classes.len() {
        diffs.push(format!(
            "class count: {} vs {}",
            a.classes.len(),
            b.classes.len()
        ));
    }
    if a.covergroups.len() != b.covergroups.len() {
        diffs.push(format!(
            "covergroup count: {} vs {}",
            a.covergroups.len(),
            b.covergroups.len()
        ));
    }
    diffs
}

fn run_ast_diff(targets: &[String], other: &str) -> Result<(), SimError> {
    use mivon_core::diagnostics::DiagCode;
    let a_file = targets.first().ok_or_else(|| {
        SimError::with_diag(
            DiagCode::InvalidSyntax,
            "--ast-diff butuh file pertama: mcheck a.sv --ast-diff b.sv",
        )
    })?;
    println!("AST diff: {} vs {}", a_file, other);

    let (_, _, ir_a) = crate::open_elaborated(
        std::slice::from_ref(a_file),
        &[],
        &[],
        None,
        mivon_elaboration::elaborator::ElaborateMode::StrictSimulation,
    )
    .map_err(|e| {
        SimError::with_diag(
            DiagCode::InvalidSyntax,
            format!("gagal compile '{}': {}", a_file, e),
        )
    })?;
    let (_, _, ir_b) = crate::open_elaborated(
        &[other.to_string()],
        &[],
        &[],
        None,
        mivon_elaboration::elaborator::ElaborateMode::StrictSimulation,
    )
    .map_err(|e| {
        SimError::with_diag(
            DiagCode::InvalidSyntax,
            format!("gagal compile '{}': {}", other, e),
        )
    })?;

    let diffs = compare_ir_designs(&ir_a, &ir_b);
    if diffs.is_empty() {
        println!("✅ AST identik (0 perbedaan)");
        Ok(())
    } else {
        println!("⚠️  {} perbedaan struktural:", diffs.len());
        for d in &diffs {
            println!("  - {}", d);
        }
        Err(SimError::with_diag(
            mivon_core::diagnostics::DiagCode::AssertionFailed,
            format!("AST berbeda: {} perbedaan", diffs.len()),
        ))
    }
}

// ═══ ENT-22: SV Version Compatibility Check ═══

/// Scan source files for SV feature usage patterns.
/// Returns list of (feature_name, is_supported, locations).
fn run_sv_version_check(targets: &[String]) -> Result<(), SimError> {
    use std::collections::HashMap;

    let files = crate::collect_targets(targets)?;
    if files.is_empty() {
        eprintln!("No files found.");
        return Ok(());
    }

    // Feature patterns: (feature_name, regex-like keyword, sv_version, supported).
    let features: Vec<(&str, &str, &str, bool)> = vec![
        ("always_ff", "always_ff", "1800-2012", true),
        ("always_comb", "always_comb", "1800-2012", true),
        ("always_latch", "always_latch", "1800-2012", true),
        ("logic", "logic", "1800-2005", true),
        ("bit", "bit ", "1800-2005", true),
        ("byte", "byte ", "1800-2005", true),
        ("shortint", "shortint", "1800-2005", true),
        ("longint", "longint", "1800-2005", true),
        ("int", "int ", "1800-2005", true),
        ("string", "string ", "1800-2005", true),
        ("class", "class ", "1800-2005", true),
        ("constraint", "constraint ", "1800-2005", true),
        ("rand", "rand ", "1800-2005", true),
        ("randc", "randc ", "1800-2005", true),
        ("covergroup", "covergroup", "1800-2005", true),
        ("coverpoint", "coverpoint", "1800-2005", true),
        ("cross", "cross ", "1800-2005", true),
        ("import", "import ", "1800-2005", true),
        ("export", "export ", "1800-2005", true),
        ("package", "package ", "1800-2005", true),
        ("interface", "interface ", "1800-2001", true),
        ("modport", "modport ", "1800-2005", true),
        ("clocking", "clocking ", "1800-2005", true),
        ("typedef", "typedef ", "1800-2005", true),
        ("enum", "enum ", "1800-2005", true),
        ("struct", "struct ", "1800-2005", true),
        ("union", "union ", "1800-2005", true),
        ("virtual", "virtual ", "1800-2005", true),
        ("extends", "extends ", "1800-2005", true),
        ("forever", "forever ", "1800-2001", true),
        ("foreach", "foreach", "1800-2005", true),
        ("do-while", "do ", "1800-2001", true),
        ("return", "return ", "1800-2001", true),
        ("break", "break;", "1800-2005", true),
        ("continue", "continue;", "1800-2005", true),
        ("fork-join_any", "join_any", "1800-2005", true),
        ("fork-join_none", "join_none", "1800-2005", true),
        ("unique-case", "unique case", "1800-2009", true),
        ("priority-case", "priority case", "1800-2009", true),
        ("inside", "inside", "1800-2009", true),
        ("$clog2", "$clog2", "1800-2005", true),
        ("$bits", "$bits", "1800-2009", true),
        ("$countones", "$countones", "1800-2005", true),
        ("$onehot", "$onehot", "1800-2009", true),
        ("$readmemh", "$readmemh", "1364-2001", true),
        ("$readmemb", "$readmemb", "1364-2001", true),
        ("$urandom", "$urandom", "1800-2009", true),
        ("DPI-C", "DPI-C", "1800-2009", true),
        ("assert", "assert ", "1800-2005", true),
        ("assume", "assume ", "1800-2009", true),
        ("cover property", "cover property", "1800-2009", true),
        // Features NOT supported by Mivon
        ("checker", "checker ", "1800-2009", false),
        ("property", "property ", "1800-2009", false),
        ("sequence", "sequence ", "1800-2009", false),
        ("let", "let ", "1800-2009", false),
        ("nettype", "nettype ", "1800-2009", false),
        ("alias", "alias ", "1800-2009", false),
    ];

    let mut feature_counts: HashMap<String, (usize, bool, &str)> = HashMap::new();
    let mut file_count = 0usize;
    let mut mv_skipped = 0usize;

    for file in &files {
        let path = std::path::Path::new(file);
        // `.mv`/`.mvh` BUKAN SystemVerilog — grep kata kunci SV di atas akan
        // salah klasifikasi (mis. `int` di `print`, `bit` di `orbit`). Yang
        // relevan untuk `.mv` adalah versi SV hasil `mgen`, jadi di sini
        // dilewati dan dilaporkan terpisah.
        if is_mv_source(path) {
            mv_skipped += 1;
            continue;
        }
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Warning: cannot read {}: {}", path.display(), e);
                continue;
            }
        };
        file_count += 1;

        // Skip comments (simple line-based).
        let lines: Vec<&str> = content
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect();
        let cleaned: String = lines.join("\n");

        for (name, pattern, version, supported) in &features {
            if cleaned.contains(pattern) {
                let entry = feature_counts
                    .entry(name.to_string())
                    .or_insert((0, *supported, version));
                entry.0 += 1;
            }
        }
    }

    // Report.
    println!("═══ ENT-22: SV Version Compatibility Report ═══");
    println!("Files scanned: {}", file_count);
    if mv_skipped > 0 {
        println!(
            "Mivon HDL (.mv/.mvh) skipped: {} — bukan SV; cek versi SV-nya \
             di output `mgen`",
            mv_skipped
        );
    }
    println!();

    let mut supported: Vec<_> = Vec::new();
    let mut unsupported: Vec<_> = Vec::new();
    for (name, (count, is_supported, version)) in &feature_counts {
        if *is_supported {
            supported.push((name, count, version));
        } else {
            unsupported.push((name, count, version));
        }
    }
    supported.sort_by(|a, b| b.1.cmp(a.1));
    unsupported.sort_by(|a, b| b.1.cmp(a.1));

    if !supported.is_empty() {
        println!("✅ Supported features used:");
        for (name, count, version) in &supported {
            println!(
                "  {:<25} {:>4} occurrences  (IEEE {})",
                name, count, version
            );
        }
        println!();
    }

    if !unsupported.is_empty() {
        println!("⚠️  Features NOT fully supported by Mivon:");
        for (name, count, version) in &unsupported {
            println!(
                "  {:<25} {:>4} occurrences  (IEEE {})",
                name, count, version
            );
        }
        println!();
        println!("Note: These features may parse but have limited runtime support.");
    } else {
        println!("✅ All detected features are supported by Mivon.");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Nama file unik per test — 3 test berjalan PARALLEL; bila semua memakai
    // `t.sv` di dir sama, salah satu test meng-overwrite file test lain →
    // isi campur → diff palsu (flaky). Dir per-test juga unik.
    fn elabor(file: &str) -> mivon_ir::IrDesign {
        let dir = std::env::temp_dir().join(format!(
            "mivon_astdiff_{}_{}",
            std::process::id(),
            std::thread::current()
                .name()
                .unwrap_or("t")
                .replace("::", "_")
        ));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("t.sv");
        std::fs::write(&path, file).unwrap();
        let (_, _, ir) = crate::open_elaborated(
            &[path.to_string_lossy().to_string()],
            &[],
            &[],
            None,
            mivon_elaboration::elaborator::ElaborateMode::StrictSimulation,
        )
        .unwrap();
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
        ir
    }

    #[test]
    fn test_ast_diff_identical() {
        // PARSER-13: dua file identik → 0 perbedaan.
        let src = "module m(input clk); reg [7:0] q; always @(posedge clk) q <= q + 1; endmodule";
        let a = elabor(src);
        let b = elabor(src);
        assert!(
            compare_ir_designs(&a, &b).is_empty(),
            "file identik harus 0 diff"
        );
    }

    #[test]
    fn test_ast_diff_width_mismatch() {
        // PARSER-13: lebar signal berbeda → diff terdeteksi.
        let a =
            elabor("module m(input clk); reg [7:0] q; always @(posedge clk) q <= q + 1; endmodule");
        let b = elabor(
            "module m(input clk); reg [15:0] q; always @(posedge clk) q <= q + 1; endmodule",
        );
        let diffs = compare_ir_designs(&a, &b);
        assert!(
            diffs
                .iter()
                .any(|d| d.contains("width") && d.contains("8 vs 16")),
            "lebar 8 vs 16 harus terdeteksi: {:?}",
            diffs
        );
    }

    #[test]
    fn test_ast_diff_process_count_mismatch() {
        // PARSER-13: jumlah proses berbeda → diff terdeteksi.
        let a = elabor("module m(input clk); reg q; always @(posedge clk) q <= 1; endmodule");
        let b = elabor(
            "module m(input clk); reg q; always @(posedge clk) q <= 1; always @(posedge clk) q <= 0; endmodule",
        );
        let diffs = compare_ir_designs(&a, &b);
        assert!(
            diffs.iter().any(|d| d.contains("process count")),
            "jumlah proses harus terdeteksi: {:?}",
            diffs
        );
    }

    #[test]
    fn test_ast_diff_order_insensitive() {
        // PARSER-13: dua design dengan SET signal sama tapi URUTAN deklarasi
        // beda harus 0 perbedaan (name-based, bukan pairwise index). Sebelum
        // fix by-name: zip by index membandingkan q vs r → diff palsu
        // "width: 8 vs 15" + asimetri A→B != B→A (ditemukan mivon-fuzz
        // target astdiff).
        let a = elabor(
            "module m(input clk); reg [7:0] q; reg [15:0] r; always @(posedge clk) q <= q + 1; endmodule",
        );
        let b = elabor(
            "module m(input clk); reg [15:0] r; reg [7:0] q; always @(posedge clk) q <= q + 1; endmodule",
        );
        let diffs = compare_ir_designs(&a, &b);
        assert!(
            diffs.is_empty(),
            "order deklarasi beda tidak boleh menghasilkan diff: {:?}",
            diffs
        );
        // Simetri: arah swap juga 0 diff.
        assert!(compare_ir_designs(&b, &a).is_empty());
    }

    #[test]
    fn test_ast_diff_missing_signal() {
        // PARSER-13: signal hanya ada di A → dilaporkan "only in A" eksplisit
        // (bukan diff width palsu hasil zip index yang bergeser).
        let a = elabor(
            "module m(input clk); reg [7:0] q; reg [7:0] r; always @(posedge clk) q <= q + 1; endmodule",
        );
        let b =
            elabor("module m(input clk); reg [7:0] q; always @(posedge clk) q <= q + 1; endmodule");
        let diffs = compare_ir_designs(&a, &b);
        assert!(
            diffs.iter().any(|d| d.contains("'r' only in A")),
            "signal hilang harus 'only in A': {:?}",
            diffs
        );
    }
}
