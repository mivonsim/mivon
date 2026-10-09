//! pipeline — mengisi lapisan `cache/` dengan data nyata dari pipeline
//! kompilasi (db.md "Saran arsitektur cache": tiap kategori menyimpan artefak
//! tahapnya agar skip ulang saat hash identik).
//!
//! STATUS FASE 2b (Kritik A, internal): populator derivasi 12 kategori
//! tak-terbaca dihapus (preprocess/lexer/parser/semantic/macro/include/
//! dependency/resolve/constant/type/hierarchy) + mirror verify/profile sudah
//! write-through dari setter state/. Tersisa HANYA kategori dibaca tools:
//! elaborate/generate (baca `melab`), optimize/expression (baca `minspect`).
//! simulation/waveform/coverage/lint ditulis langsung tools terkait.
//! Jangan tambah kategori derivasi baru di sini; tulis langsung via
//! `StageStore`/`CacheLayer` di titik produksi.
//!
//! Kategori yang diisi otomatis pada save (data tersedia di compile):
//!
//! | Kategori       | Payload                                    | Kunci        |
//! |----------------|--------------------------------------------|--------------|
//! | elaborate/     | instance, port binding, proses, net (IR)   | nama module  |
//! | generate/      | blok if/for/case + instance hasil generate | nama module  |
//! | optimize/      | const fold + loop unroll (elaborator)      | "last"       |
//! | expression/    | evaluasi ekspresi + sampel hasil fold      | "last"       |
//!
//! 12 kategori lain (preprocess/lexer/parser/semantic/verify/macro/include/
//! dependency/resolve/constant/type/hierarchy) + profile TIDAK diisi populator
//! (verify/profile via mirror setter; sisanya tak punya pembaca produksi).
//!
//! elaborate/ diisi dari IR bila tersedia (dipakai `save_elaborate_cache`
//!   setelah elaborasi); tanpa IR diisi fallback AST (instance saja). optimize/
//!   + expression/ diisi dari snapshot statistik elaborator (dipakai juga
//!     `save_elaborate_cache`). simulation/ + waveform/ diisi `msim` setelah run
//!     (initial state, scheduler, signal index); coverage/ diisi `mcov`/`msim
//!     --coverage`; lint/ diisi `mlint`. Semua lewat [`super::CacheLayer::put`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mivon_ast::types::{Module, ModuleItem};
use mivon_ast::Design;
use serde::{Deserialize, Serialize};

use super::super::verify::{CheckResult, VerifyCheckKind};
use super::{CacheCategory, CacheLayer};
use crate::micd::metadata::path_hash;

// ─── Payload per kategori ───

// (Fase 2b: payload lexer/preprocess/parser/semantic/type/hierarchy/
// constant/resolve/dependency/macro/include dihapus bersama populatornya —
// tak ada pembaca produksi. VerifyPayload tetap (mirror write-through).)

/// Hasil verifikasi (db.md "7. verify/", Kritik 9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifyPayload {
    pub parse_ok: bool,
    pub elab_ok: bool,
    pub err_count: usize,
    pub warn_count: usize,
    pub info_count: usize,
    /// Hasil per kategori analisis — bukan satu blob.
    pub checks: Vec<(VerifyCheckKind, CheckResult)>,
}

/// Satu instance hasil elaborasi (db.md "5. elaborate/": Module Instance +
/// Parameter Override + Port Binding). `param_overrides` hanya berisi parameter
/// yang benar-benar di-override (nilai konstanta hasil resolve).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ElabInstance {
    pub module: String,
    pub instance: String,
    /// Jumlah port yang di-bind ke signal (port binding).
    pub port_bindings: usize,
    /// (nama parameter, nilai override).
    pub param_overrides: Vec<(String, i64)>,
    /// Posisi source (untuk diagnostic).
    pub line: usize,
    pub col: usize,
}

/// Ringkasan proses per module (db.md "5. elaborate/": Always Expansion).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProcessCounts {
    pub combinational: usize,
    pub comb_reactive: usize,
    pub sequential: usize,
    pub initial: usize,
    pub final_: usize,
    pub always_with_delay: usize,
}

/// Net resolution: jumlah signal per net type (db.md "5. elaborate/").
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NetCounts {
    pub wire: usize,
    pub wand: usize,
    pub wor: usize,
    pub tri: usize,
    pub tri0: usize,
    pub tri1: usize,
    pub triand: usize,
    pub trior: usize,
    pub supply0: usize,
    pub supply1: usize,
}

/// Payload kategori elaborate/ per module (db.md "5. elaborate/"): hasil
/// elaborasi — generate expansion, parameter override, module instance,
/// hierarchy, port binding, net resolution, always expansion.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ElaboratePayload {
    pub instance_count: usize,
    pub instances: Vec<ElabInstance>,
    pub processes: ProcessCounts,
    pub net_counts: NetCounts,
}

/// Generate expansion per module (db.md "16. generate/"): jumlah blok
/// if/for/case di AST + jumlah instance hasil ekspansi generate (dari IR bila
/// elaborasi tersedia).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GeneratePayload {
    pub if_blocks: usize,
    pub for_blocks: usize,
    pub case_blocks: usize,
    /// Jumlah instance hasil generate expansion (dari IR bila tersedia).
    pub expanded_instances: usize,
}

/// Satu temuan lint (db.md "7. verify/ → lint/").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LintFinding {
    pub module: String,
    pub check: String,
    /// "W" warning / "E" error.
    pub severity: String,
    pub message: String,
}

/// Payload kategori lint/: hasil `mlint` per project — disimpan tool, dibaca
/// `minspect cache` / run berikutnya tanpa menjalankan lint ulang.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LintPayload {
    pub findings: Vec<LintFinding>,
}

/// Ringkasan coverage (db.md "19. coverage/"): line/branch/toggle/FSM —
/// disimpan `mcov` (atau msim dengan --coverage), dibaca tanpa simulasi ulang.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CoveragePayload {
    pub line_items: u64,
    pub line_hits: u64,
    pub branch_total: u64,
    pub branch_covered: u64,
    pub toggle_signals: u64,
    pub toggle_transitions: u64,
    pub fsm_signals: u64,
    pub fsm_states: u64,
}

/// Ringkasan simulasi (db.md "17. simulation/"): initial state, scheduler
/// (event processed, end time), sensitivity list — disimpan `msim` setelah
/// run, dibaca tanpa simulasi ulang (mis. `minspect cache`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SimulationPayload {
    /// End time simulasi (timewheel terakhir).
    pub end_time: u64,
    /// Jumlah event yang diproses scheduler.
    pub events_processed: u64,
    /// Jumlah signal di top (post-flatten).
    pub signal_count: usize,
    /// Jumlah signal dengan initial state non-zero (bukan default x/z).
    pub init_signals: usize,
    /// Jumlah proses per tipe (sensitivity list).
    pub processes: ProcessCounts,
}

/// Satu signal dalam index waveform (db.md "18. waveform/").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaveSignal {
    pub name: String,
    pub width: usize,
    pub kind: String,
    pub net: String,
    pub is_signed: bool,
}

/// Payload kategori waveform/ per module (db.md "18. waveform/"): signal
/// index + metadata (lebar, tipe, net) — agar VCD/FST lebih cepat dibuka
/// tanpa mem-parse ulang file waveform.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WaveformPayload {
    pub signals: Vec<WaveSignal>,
}

/// Payload kategori optimize/ (db.md "6. optimize/"): ringkasan optimasi
/// elaborator — constant folding, loop unroll, statement hasil unroll.
/// Disimpan setelah elaborasi (jalur `--fast`), dibaca tool tanpa compile.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OptimizePayload {
    pub const_folds: usize,
    pub loop_unrolls: usize,
    pub unrolled_stmts: usize,
}

/// Payload kategori expression/ (db.md "10. expression/"): evaluasi ekspresi
/// selama elaborasi — jumlah panggilan `elaborate_expr` + sampel
/// (ekspresi → nilai) hasil constant folding (`4+5 → 9`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExpressionPayload {
    pub expr_evals: usize,
    pub samples: Vec<(String, i64)>,
}

// ─── Input populator ───

/// Data yang dibutuhkan populator. Caller (CompileSession::save_micd / jalur
/// legacy) merakit dari state compile.
pub struct CachePopulateInput<'a> {
    /// Design per file (path → design).
    pub designs: Vec<(&'a PathBuf, &'a Design)>,
    /// IR hasil elaborasi — dipakai untuk kategori elaborate/ + generate/
    /// (db.md "5. elaborate/", "16. generate/"). `None` pada jalur parse-only
    /// (legacy / compile-only): elaborate/ diisi ringkasan AST sebagai
    /// fallback, generate/ tetap diisi dari blok generate AST.
    pub ir_design: Option<&'a mivon_ir::IrDesign>,
    /// Design SETELAH generate expansion (milik elaborator) — dipakai fallback
    /// elaborate/ untuk module TOP yang sub-instance-nya dikonsumsi flatten
    /// IR (`top.sub_instances` kosong post-flatten). `None` bila tidak tersedia
    /// → fallback memakai `designs` (pre-expansion, instance generate belum
    /// terlihat).
    pub expanded_design: Option<&'a Design>,
    /// Statistik optimasi elaborator (db.md "6. optimize/", "10. expression/") —
    /// const fold, loop unroll, evaluasi ekspresi. `None` bila elaborasi belum
    /// berjalan (jalur parse-only / save_micd sebelum elaborate).
    pub opt_snapshot: Option<mivon_elaboration::util::OptimizeSnapshot>,
}

/// Timescale efektif satu file dari design-nya (Fase 2, Kritik A).
///
/// Prioritas: `Design.timescale` (satuan TERHALUS file itu — lihat parser
/// F84 `finest_timescale`) → timescale module pertama yang `Some` → `None`
/// (file tanpa directive → fallback satuan global, perilaku pre-F84). Satu
/// nilai per file adalah pendekatan untuk kasus umum (1 timescale per file);
/// file multi-timescale tetap memakai `timescale_segments` di state/
/// (`PreprocEntry`) yang presisi.
pub fn design_timescale(design: &Design) -> Option<(String, String)> {
    if let Some(ts) = design.timescale.clone() {
        return Some(ts);
    }
    design
        .modules
        .iter()
        .find_map(|m| m.timescale.clone())
}

/// Populator lapisan `cache/` dari data compile.
pub struct CachePopulator;

impl CachePopulator {
    /// Isi kategori yang datanya tersedia. Best-effort — kegagalan satu
    /// kategori tidak menggagalkan yang lain (cache bersifat non-kritis).
    ///
    /// Fase 2b (internal, tanpa beban kompatibel publik): HANYA kategori yang
    /// dibaca tools yang diisi (elaborate/generate/optimize/expression).
    /// 12 kategori tak-terbaca produksi (preprocess/lexer/parser/semantic/
    /// verify/macro/include/dependency/resolve/constant/type/hierarchy)
    /// dihapus dari sini — verify/profile sudah di-mirror write-through dari
    /// setter state/ (Fase 2), sisanya tak punya pembaca (grep: hanya writer
    /// + test). Menulisnya tiap build = I/O dobel tanpa manfaat.
    pub fn populate(layer: &mut CacheLayer, input: &CachePopulateInput) {
        Self::populate_elab(layer, input);
    }

    /// Isi hanya kategori elaborate/ + generate/ + optimize/ + expression/
    /// (dipakai jalur yang sudah punya IR setelah elaborasi — save_micd
    /// dipanggil sebelum elaborate agar cache parse tetap tersimpan walau
    /// elaborasi gagal). `populate()` delegasi ke sini (satu badan).
    pub fn populate_elab(layer: &mut CacheLayer, input: &CachePopulateInput) {
        Self::populate_elaborate(layer, input);
        Self::populate_generate(layer, input);
        Self::populate_optimize(layer, input);
    }

    /// elaborate/: per module dari IR (db.md "5. elaborate/" — generate
    /// expansion, parameter override, module instance, hierarchy, port
    /// binding, net resolution, always expansion). Module TOP diambil dari
    /// `ir.top` (post-flatten: proses/net tetap ada, sub-instance dikonsumsi
    /// flatten → instance diambil dari `expanded_design` post-expansion bila
    /// tersedia). Module non-top dari `ir.modules`. Tanpa IR (parse-only),
    /// isi fallback ringkas dari AST: instance + port binding + param override
    /// (nilai tak ter-resolve → 0).
    fn populate_elaborate(layer: &mut CacheLayer, input: &CachePopulateInput) {
        let ir_by_name: HashMap<String, &mivon_ir::IrModule> = input
            .ir_design
            .map(|ir| {
                let mut m: HashMap<String, &mivon_ir::IrModule> =
                    ir.modules.iter().map(|(k, v)| (k.to_string(), v)).collect();
                // Top module: proses/net ada di ir.top (post-flatten), tapi
                // sub-instance dikonsumsi flatten → diisi dari expanded_design.
                m.insert(ir.top.name.to_string(), &ir.top);
                m
            })
            .unwrap_or_default();
        let ast_by_name: HashMap<String, &Module> = input
            .expanded_design
            .map(|d| d.modules.iter().map(|m| (m.name.to_string(), m)).collect())
            .unwrap_or_default();
        for (_path, design) in &input.designs {
            for m in &design.modules {
                let name = m.name.to_string();
                let ir_module = ir_by_name.get(&name).copied();
                let is_top = input
                    .ir_design
                    .map(|ir| ir.top.name.as_str() == name)
                    .unwrap_or(false);
                let payload = match (ir_module, is_top) {
                    // Top: IR (proses/net) + instance dari AST post-expansion
                    // (sub-instance IR di-flatten → hierarki asli hilang).
                    (Some(ir), true) => {
                        let mut p = elaborate_from_ir(ir);
                        if let Some(ast) = ast_by_name.get(&name) {
                            let ast_p = elaborate_from_ast(ast);
                            p.instance_count = ast_p.instance_count;
                            p.instances = ast_p.instances;
                        }
                        p
                    }
                    (Some(ir), false) => elaborate_from_ir(ir),
                    (None, _) => match ast_by_name.get(&name) {
                        Some(ast) => elaborate_from_ast(ast),
                        None => elaborate_from_ast(m),
                    },
                };
                if let Ok(b) = bincode::serialize(&payload) {
                    let _ = layer.put(CacheCategory::Elaborate, &name, &b);
                }
            }
        }
    }

    /// generate/: jumlah blok generate if/for/case dari AST + instance hasil
    /// ekspansi generate (db.md "16. generate/"). `expanded_instances` diambil
    /// dari jumlah sub-instance IR untuk module non-top; untuk TOP (sub-
    /// instance IR dikonsumsi flatten) dihitung dari `expanded_design` AST
    /// post-expansion bila tersedia, atau dari `designs` pre-expansion bila
    /// tidak (fallback: instance dalam blok generate belum terlihat → 0).
    fn populate_generate(layer: &mut CacheLayer, input: &CachePopulateInput) {
        for (_path, design) in &input.designs {
            for m in &design.modules {
                let mut payload = GeneratePayload::default();
                for item in &m.items {
                    count_generate_item(item, &mut payload);
                }
                if let Some(ir) = input.ir_design {
                    if let Some(irm) = ir.modules.get(&m.name) {
                        payload.expanded_instances = irm.sub_instances.len();
                    } else if ir.top.name == m.name {
                        // Top: IR post-flatten tidak membawa hierarki — hitung
                        // instance dari AST post-expansion bila tersedia.
                        let src = input
                            .expanded_design
                            .and_then(|d| d.modules.iter().find(|mm| mm.name == m.name))
                            .unwrap_or(m);
                        payload.expanded_instances = direct_instance_count(src);
                    }
                }
                let name = m.name.to_string();
                if let Ok(b) = bincode::serialize(&payload) {
                    let _ = layer.put(CacheCategory::Generate, &name, &b);
                }
            }
        }
    }

    /// optimize/ + expression/: statistik optimasi elaborator (db.md
    /// "6. optimize/", "10. expression/"). Disimpan sekali per build (`"last"`)
    /// dari snapshot opt_stats elaborator.
    fn populate_optimize(layer: &mut CacheLayer, input: &CachePopulateInput) {
        let Some(snap) = &input.opt_snapshot else {
            return;
        };
        let opt = OptimizePayload {
            const_folds: snap.const_folds,
            loop_unrolls: snap.loop_unrolls,
            unrolled_stmts: snap.unrolled_stmts,
        };
        if let Ok(b) = bincode::serialize(&opt) {
            let _ = layer.put(CacheCategory::Optimize, "last", &b);
        }
        let expr = ExpressionPayload {
            expr_evals: snap.expr_evals,
            samples: snap.expr_samples.clone(),
        };
        if let Ok(b) = bincode::serialize(&expr) {
            let _ = layer.put(CacheCategory::Expression, "last", &b);
        }
    }
}

/// ElaboratePayload dari IR (data penuh: proses + net resolution).
fn elaborate_from_ir(ir: &mivon_ir::IrModule) -> ElaboratePayload {
    let mut p = ElaboratePayload::default();
    for inst in &ir.sub_instances {
        p.instance_count += 1;
        p.instances.push(ElabInstance {
            module: inst.module_name.to_string(),
            instance: inst.instance_name.to_string(),
            port_bindings: inst.port_map.len(),
            param_overrides: inst
                .param_map
                .iter()
                .map(|(k, v)| (k.to_string(), *v))
                .collect(),
            line: inst.line,
            col: inst.col,
        });
    }
    for proc in &ir.processes {
        match proc {
            mivon_ir::Process::Combinational { .. } => p.processes.combinational += 1,
            mivon_ir::Process::CombReactive { .. } => p.processes.comb_reactive += 1,
            mivon_ir::Process::Sequential { .. } => p.processes.sequential += 1,
            mivon_ir::Process::Initial { .. } => p.processes.initial += 1,
            mivon_ir::Process::Final { .. } => p.processes.final_ += 1,
            mivon_ir::Process::AlwaysWithDelay { .. } => p.processes.always_with_delay += 1,
        }
    }
    for s in &ir.signals {
        use mivon_ir::NetType::*;
        match s.net_type {
            Wire => p.net_counts.wire += 1,
            Wand => p.net_counts.wand += 1,
            Wor => p.net_counts.wor += 1,
            Tri => p.net_counts.tri += 1,
            Tri0 => p.net_counts.tri0 += 1,
            Tri1 => p.net_counts.tri1 += 1,
            TriAnd => p.net_counts.triand += 1,
            TriOr => p.net_counts.trior += 1,
            Supply0 => p.net_counts.supply0 += 1,
            Supply1 => p.net_counts.supply1 += 1,
        }
    }
    p
}

/// ElaboratePayload fallback dari AST (tanpa IR): instance + port binding +
/// nama param override. Nilai override tidak ter-resolve → 0; proses/net
/// tidak diketahui (kosong).
fn elaborate_from_ast(m: &Module) -> ElaboratePayload {
    let mut p = ElaboratePayload::default();
    for item in &m.items {
        if let ModuleItem::Instance(inst) = item {
            p.instance_count += 1;
            p.instances.push(ElabInstance {
                module: inst.module_name.to_string(),
                instance: inst.instance_name.to_string(),
                port_bindings: inst.port_conns.len(),
                param_overrides: inst
                    .param_assigns
                    .keys()
                    .map(|k| (k.to_string(), 0))
                    .collect(),
                line: inst.line,
                col: inst.col,
            });
        }
    }
    p
}

/// Hitung blok generate if/for/case secara rekursif dari satu item module.
fn count_generate_item(item: &ModuleItem, p: &mut GeneratePayload) {
    use mivon_ast::types::{CaseGenerateItem, GenerateItem};
    let ModuleItem::Generate(gen) = item else {
        return;
    };
    for gi in &gen.items {
        match gi {
            GenerateItem::If {
                true_items,
                false_items,
                ..
            } => {
                p.if_blocks += 1;
                for it in true_items.iter().chain(false_items.iter()) {
                    count_generate_item(it, p);
                }
            }
            GenerateItem::For { body_items, .. } => {
                p.for_blocks += 1;
                for it in body_items {
                    count_generate_item(it, p);
                }
            }
            GenerateItem::Case { items, default, .. } => {
                p.case_blocks += 1;
                for ci in items {
                    let CaseGenerateItem { body, .. } = ci;
                    for it in body {
                        count_generate_item(it, p);
                    }
                }
                for it in default.iter().flatten() {
                    count_generate_item(it, p);
                }
            }
            GenerateItem::Items(items) => {
                for it in items {
                    count_generate_item(it, p);
                }
            }
        }
    }
}

/// Jumlah instance langsung (ModuleItem::Instance) di module — dipakai untuk
/// top post-generate-expansion (AST sudah memuat instance hasil generate).
fn direct_instance_count(m: &Module) -> usize {
    m.items
        .iter()
        .filter(|it| matches!(it, ModuleItem::Instance(_)))
        .count()
}

/// Hash key stabil untuk path (dipakai tool pembaca cache).
pub fn cache_key_path(path: &Path) -> u64 {
    path_hash(path)
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;
    use mivon_ast::expr::Expr;
    use mivon_ast::types::{Module, Port, PortDirection, Range};
    use mivon_core::intern::Symbol;

    fn sample_design() -> Design {
        let mut m = Module {
            name: Symbol::intern("counter"),
            ports: vec![
                Port {
                    name: Symbol::intern("clk"),
                    direction: PortDirection::Input,
                    range: None,
                    expr_range: None,
                    dtype_name: None,
                    array_range: None,
                    extra_unpacked_dims: vec![],
                    extra_packed_dims: vec![],
                    init_expr: None,
                },
                Port {
                    name: Symbol::intern("out"),
                    direction: PortDirection::Output,
                    range: Some(Range { msb: 7, lsb: 0 }),
                    expr_range: None,
                    dtype_name: None,
                    array_range: None,
                    extra_unpacked_dims: vec![],
                    extra_packed_dims: vec![],
                    init_expr: None,
                },
            ],
            params: vec![],
            decls: vec![],
            items: vec![ModuleItem::Instance(mivon_ast::types::ModuleInstance {
                module_name: Symbol::intern("alu"),
                instance_name: Symbol::intern("u_alu"),
                range: None,
                param_assigns: Default::default(),
                type_param_assigns: Default::default(),
                port_conns: vec![],
                line: 1,
                col: 1,
                attrs: Vec::new(),
            })],
            timescale: None,
        };
        m.params.push(mivon_ast::types::ParamDecl {
            name: Symbol::intern("WIDTH"),
            dtype: None,
            range: None,
            default: Some(Expr::Value(mivon_ast::expr::Value::Decimal(8))),
            is_localparam: false,
            is_type_param: false,
            type_default: None,
        });
        let mut d = Design::default();
        d.modules.push(m);
        d
    }

    fn test_root(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mivon_cache_pipeline_{}_{}",
            std::process::id(),
            name
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    // ── Fase 2 (Kritik A): timescale tidak boleh hilang ──

    #[test]
    fn test_fase2_design_timescale_prioritas() {
        // Design.timescale menang; fallback module pertama; kosong → None.
        let mut d = Design::default();
        assert_eq!(design_timescale(&d), None);
        let mut m = sample_design().modules.pop().unwrap();
        m.timescale = Some(("1ns".to_string(), "1ps".to_string()));
        d.modules.push(m);
        assert_eq!(
            design_timescale(&d),
            Some(("1ns".to_string(), "1ps".to_string()))
        );
        d.timescale = Some(("10ns".to_string(), "1ns".to_string()));
        assert_eq!(
            design_timescale(&d),
            Some(("10ns".to_string(), "1ns".to_string())),
            "Design.timescale menang atas module"
        );
    }

    #[test]
    fn test_populate_all_categories() {
        let root = test_root("pop");
        let db = root.join("db");
        std::fs::create_dir_all(&db).unwrap();
        let mut layer = CacheLayer::open(&db, "pid", 0).unwrap();

        let path = PathBuf::from("counter.sv");
        let design = sample_design();

        let input = CachePopulateInput {
            designs: vec![(&path, &design)],
            ir_design: None,
            expanded_design: None,
            opt_snapshot: None,
        };
        CachePopulator::populate(&mut layer, &input);
        layer.save().unwrap();

        // Fase 2b: HANYA kategori dibaca tools yang terisi (elaborate dari
        // fallback AST + generate). 12 kategori tak-terbaca + verify/profile
        // (mirror write-through) tetap kosong.
        assert!(layer.entry_count(CacheCategory::Elaborate) >= 1, "elaborate terisi");
        assert!(layer.entry_count(CacheCategory::Generate) >= 1, "generate terisi");
        for cat in [
            CacheCategory::Preprocess,
            CacheCategory::Lexer,
            CacheCategory::Parser,
            CacheCategory::Macro,
            CacheCategory::Include,
            CacheCategory::Verify,
            CacheCategory::Dependency,
            CacheCategory::Resolve,
            CacheCategory::Semantic,
            CacheCategory::Type,
            CacheCategory::Constant,
            CacheCategory::Hierarchy,
            CacheCategory::Profile,
        ] {
            assert_eq!(layer.entry_count(cat), 0, "{} harus kosong (tak diisi)", cat.name());
        }

        // Periksa isi elaborate fallback AST: 1 instance alu.
        let elab: ElaboratePayload =
            bincode::deserialize(&layer.get(CacheCategory::Elaborate, "counter").unwrap()).unwrap();
        assert_eq!(elab.instance_count, 1);
        assert_eq!(elab.instances[0].module, "alu");
        // Kategori tanpa data tetap kosong (fungsional, bukan diisi).
        assert_eq!(layer.entry_count(CacheCategory::Simulation), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_persist_after_populate() {
        let root = test_root("persist");
        let db = root.join("db");
        std::fs::create_dir_all(&db).unwrap();
        let path = PathBuf::from("a.sv");
        let design = sample_design();
        {
            let mut layer = CacheLayer::open(&db, "pid", 0).unwrap();
            let input = CachePopulateInput {
                designs: vec![(&path, &design)],
                ir_design: None,
                expanded_design: None,
                opt_snapshot: None,
            };
            CachePopulator::populate(&mut layer, &input);
            layer.save().unwrap();
        }
        {
            let mut layer = CacheLayer::open(&db, "pid", 0).unwrap();
            assert!(layer.contains(CacheCategory::Generate, "counter"));
            let gen: GeneratePayload =
                bincode::deserialize(&layer.get(CacheCategory::Generate, "counter").unwrap()).unwrap();
            assert_eq!(gen, GeneratePayload::default());
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Design dengan blok generate-for (db.md "16. generate/").
    fn generate_design() -> Design {
        use mivon_ast::types::{GenerateBlock, GenerateItem};
        let mut m = Module {
            name: Symbol::intern("genmod"),
            ports: vec![],
            params: vec![],
            decls: vec![],
            items: vec![ModuleItem::Generate(GenerateBlock {
                items: vec![GenerateItem::For {
                    var: Symbol::intern("i"),
                    init: None,
                    cond: None,
                    step: None,
                    body_items: vec![ModuleItem::Instance(mivon_ast::types::ModuleInstance {
                        module_name: Symbol::intern("alu"),
                        instance_name: Symbol::intern("u_alu"),
                        range: None,
                        param_assigns: Default::default(),
                        type_param_assigns: Default::default(),
                        port_conns: vec![],
                        line: 1,
                        col: 1,
                        attrs: Vec::new(),
                    })],
                    label: None,
                }],
            })],
            timescale: None,
        };
        m.params.push(mivon_ast::types::ParamDecl {
            name: Symbol::intern("N"),
            dtype: None,
            range: None,
            default: Some(Expr::Value(mivon_ast::expr::Value::Decimal(4))),
            is_localparam: false,
            is_type_param: false,
            type_default: None,
        });
        let mut d = Design::default();
        d.modules.push(m);
        d
    }

    /// IR dengan satu module `genmod` berisi 2 sub-instance hasil generate
    /// expansion (db.md "5. elaborate/": 1000 instance generate-for → cache).
    fn sample_ir() -> mivon_ir::IrDesign {
        use mivon_ir::{IrInstance, IrModule, Process};
        let mut ir = mivon_ir::IrDesign::default();
        let mut m = IrModule {
            name: Symbol::intern("genmod"),
            ..Default::default()
        };
        for i in 0..2 {
            m.sub_instances.push(IrInstance {
                module_name: Symbol::intern("alu"),
                instance_name: Symbol::intern(&format!("u_alu_{}", i)),
                port_map: std::sync::Arc::new(HashMap::from([(Symbol::intern("a"), 1)])),
                param_map: std::sync::Arc::new(HashMap::from([(Symbol::intern("WIDTH"), 8)])),
                type_param_map: std::sync::Arc::new(HashMap::new()),
                line: 10 + i,
                col: 1,
                attrs: Vec::new(),
            });
        }
        m.processes.push(Process::Sequential {
            name: Symbol::intern("clk_proc"),
            clock: mivon_ir::ClockEdge::PosEdge(0),
            reset: None,
            body: vec![],
            iff: None,
        });
        m.signals.push(mivon_ir::SignalInfo {
            name: Symbol::intern("clk"),
            width: 1,
            kind: mivon_ir::SignalKind::Input,
            net_type: mivon_ir::NetType::Wire,
            multi_driver: false,
            init_val: mivon_core::LogicVec::fill(mivon_core::LogicVal::Zero, 1),
            ..Default::default()
        });
        ir.modules.insert(Symbol::intern("genmod"), m);
        ir
    }

    /// Design post-generate-expansion untuk module TOP (sub-instance IR top
    /// dikonsumsi flatten → hierarki diambil dari AST post-expansion).
    fn expanded_design() -> Design {
        let m = Module {
            name: Symbol::intern("genmod"),
            ports: vec![],
            params: vec![],
            decls: vec![],
            items: vec![
                ModuleItem::Instance(mivon_ast::types::ModuleInstance {
                    module_name: Symbol::intern("alu"),
                    instance_name: Symbol::intern("u_a"),
                    range: None,
                    param_assigns: Default::default(),
                    type_param_assigns: Default::default(),
                    port_conns: vec![],
                    line: 1,
                    col: 1,
                    attrs: Vec::new(),
                }),
                ModuleItem::Instance(mivon_ast::types::ModuleInstance {
                    module_name: Symbol::intern("alu"),
                    instance_name: Symbol::intern("u_b"),
                    range: None,
                    param_assigns: Default::default(),
                    type_param_assigns: Default::default(),
                    port_conns: vec![],
                    line: 1,
                    col: 1,
                    attrs: Vec::new(),
                }),
            ],
            timescale: None,
        };
        let mut d = Design::default();
        d.modules.push(m);
        d
    }

    #[test]
    fn test_populate_elaborate_top_uses_expanded_design() {
        // Simulasi jalur run_fast: IR post-flatten (top tanpa sub_instances),
        // designs pre-expansion (generate block), expanded_design post-expansion.
        let root = test_root("elab_top");
        let db = root.join("db");
        std::fs::create_dir_all(&db).unwrap();
        let mut layer = CacheLayer::open(&db, "pid", 0).unwrap();

        let path = PathBuf::from("gen.sv");
        let design = generate_design(); // pre-expansion: ModuleItem::Generate
        let expanded = expanded_design(); // post-expansion: 2 instance langsung
                                          // IR: `genmod` HANYA di ir.top (post-flatten, sub_instances kosong).
        let mut ir = sample_ir();
        let mut top_ir = ir.modules.remove(&Symbol::intern("genmod")).unwrap();
        top_ir.sub_instances.clear(); // flatten mengkonsumsi sub-instance top
        ir.modules.clear();
        ir.top = top_ir;
        let input = CachePopulateInput {
            designs: vec![(&path, &design)],
            ir_design: Some(&ir),
            expanded_design: Some(&expanded),
            opt_snapshot: None,
        };
        CachePopulator::populate(&mut layer, &input);

        // Top: instance dari expanded_design (2), proses/net dari ir.top.
        let elab: ElaboratePayload =
            bincode::deserialize(&layer.get(CacheCategory::Elaborate, "genmod").unwrap()).unwrap();
        assert_eq!(elab.instance_count, 2, "top: instance dari expanded_design");
        assert_eq!(elab.instances[0].instance, "u_a");
        assert_eq!(elab.processes.sequential, 1, "proses dari ir.top");
        assert_eq!(elab.net_counts.wire, 1, "net dari ir.top");

        // generate/: expanded_instances top dari expanded_design.
        let gen: GeneratePayload =
            bincode::deserialize(&layer.get(CacheCategory::Generate, "genmod").unwrap()).unwrap();
        assert_eq!(gen.expanded_instances, 2);
        assert_eq!(gen.for_blocks, 1, "blok generate dari design pre-expansion");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_populate_elaborate_and_generate() {
        let root = test_root("elab_gen");
        let db = root.join("db");
        std::fs::create_dir_all(&db).unwrap();
        let mut layer = CacheLayer::open(&db, "pid", 0).unwrap();

        let path = PathBuf::from("gen.sv");
        let design = generate_design();
        let ir = sample_ir();
        let input = CachePopulateInput {
            designs: vec![(&path, &design)],
            ir_design: Some(&ir),
            expanded_design: None,
            opt_snapshot: None,
        };
        CachePopulator::populate(&mut layer, &input);
        layer.save().unwrap();

        // generate/: blok for dari AST + instance hasil ekspansi dari IR.
        let gen: GeneratePayload =
            bincode::deserialize(&layer.get(CacheCategory::Generate, "genmod").unwrap()).unwrap();
        assert_eq!(gen.for_blocks, 1, "satu generate-for di AST");
        assert_eq!(gen.if_blocks, 0);
        assert_eq!(gen.case_blocks, 0);
        assert_eq!(
            gen.expanded_instances, 2,
            "dua instance hasil ekspansi generate"
        );

        // elaborate/: instance IR + port binding + param override + proses + net.
        let elab: ElaboratePayload =
            bincode::deserialize(&layer.get(CacheCategory::Elaborate, "genmod").unwrap()).unwrap();
        assert_eq!(elab.instance_count, 2);
        assert_eq!(elab.instances[0].module, "alu");
        assert_eq!(elab.instances[0].instance, "u_alu_0");
        assert_eq!(elab.instances[0].port_bindings, 1);
        assert_eq!(
            elab.instances[0].param_overrides,
            vec![("WIDTH".to_string(), 8)]
        );
        assert_eq!(elab.processes.sequential, 1);
        assert_eq!(elab.net_counts.wire, 1);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_lint_and_coverage_payload_roundtrip() {
        // Payload hasil tool (mlint/mcov) — disimpan via CacheLayer::put,
        // dibaca minspect cache. Verifikasi serialisasi bincode bundar.
        let root = test_root("lint_cov");
        let db = root.join("db");
        std::fs::create_dir_all(&db).unwrap();
        let mut layer = CacheLayer::open(&db, "pid", 0).unwrap();

        let lint = LintPayload {
            findings: vec![LintFinding {
                module: "counter".into(),
                check: "unused".into(),
                severity: "W".into(),
                message: "signal tidak dipakai".into(),
            }],
        };
        let lint_b = bincode::serialize(&lint).unwrap();
        layer.put(CacheCategory::Lint, "report", &lint_b);

        let cov = CoveragePayload {
            line_items: 3,
            line_hits: 3,
            branch_total: 4,
            branch_covered: 2,
            toggle_signals: 1,
            toggle_transitions: 2,
            fsm_signals: 0,
            fsm_states: 0,
        };
        let cov_b = bincode::serialize(&cov).unwrap();
        layer.put(CacheCategory::Coverage, "last", &cov_b);
        layer.save().unwrap();

        // Baca ulang (lapisan baru) — data bertahan lintas open.
        let mut layer2 = CacheLayer::open(&db, "pid", 0).unwrap();
        let got_lint: LintPayload =
            bincode::deserialize(&layer2.get(CacheCategory::Lint, "report").unwrap()).unwrap();
        assert_eq!(got_lint.findings.len(), 1);
        assert_eq!(got_lint.findings[0].check, "unused");
        let got_cov: CoveragePayload =
            bincode::deserialize(&layer2.get(CacheCategory::Coverage, "last").unwrap()).unwrap();
        assert_eq!(got_cov.line_hits, 3);
        assert_eq!(got_cov.branch_covered, 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_populate_elab_fallback_without_ir() {
        // Tanpa IR (jalur parse-only): elaborate/ diisi fallback AST (instance),
        // generate/ tetap terisi dari blok generate AST.
        let root = test_root("elab_fb");
        let db = root.join("db");
        std::fs::create_dir_all(&db).unwrap();
        let mut layer = CacheLayer::open(&db, "pid", 0).unwrap();
        let path = PathBuf::from("gen.sv");
        let design = generate_design();
        let input = CachePopulateInput {
            designs: vec![(&path, &design)],
            ir_design: None,
            expanded_design: None,
            opt_snapshot: None,
        };
        CachePopulator::populate(&mut layer, &input);

        // AST fallback: instance di dalam body generate-for dihitung (for_blocks=1).
        let gen: GeneratePayload =
            bincode::deserialize(&layer.get(CacheCategory::Generate, "genmod").unwrap()).unwrap();
        assert_eq!(gen.for_blocks, 1);
        assert_eq!(
            gen.expanded_instances, 0,
            "tanpa IR tidak ada info ekspansi"
        );
        let elab: ElaboratePayload =
            bincode::deserialize(&layer.get(CacheCategory::Elaborate, "genmod").unwrap()).unwrap();
        assert_eq!(
            elab.instance_count, 0,
            "fallback AST: instance di dalam blok generate tidak dihitung (belum diekspansi)"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_sim_and_waveform_payload_roundtrip() {
        // Payload hasil msim (db.md "17. simulation/", "18. waveform/") —
        // disimpan via CacheLayer::put, dibaca minspect cache.
        let root = test_root("sim_wave");
        let db = root.join("db");
        std::fs::create_dir_all(&db).unwrap();
        let mut layer = CacheLayer::open(&db, "pid", 0).unwrap();

        let sim = SimulationPayload {
            end_time: 101,
            events_processed: 42,
            signal_count: 4,
            init_signals: 1,
            processes: ProcessCounts {
                combinational: 1,
                sequential: 1,
                initial: 1,
                ..Default::default()
            },
        };
        layer.put(
            CacheCategory::Simulation,
            "last",
            &bincode::serialize(&sim).unwrap(),
        );
        let wave = WaveformPayload {
            signals: vec![WaveSignal {
                name: "count".into(),
                width: 8,
                kind: "output".into(),
                net: "wire".into(),
                is_signed: false,
            }],
        };
        layer.put(
            CacheCategory::Waveform,
            "last",
            &bincode::serialize(&wave).unwrap(),
        );
        layer.save().unwrap();

        let mut layer2 = CacheLayer::open(&db, "pid", 0).unwrap();
        let got_sim: SimulationPayload =
            bincode::deserialize(&layer2.get(CacheCategory::Simulation, "last").unwrap()).unwrap();
        assert_eq!(got_sim.end_time, 101);
        assert_eq!(got_sim.processes.sequential, 1);
        assert_eq!(got_sim.init_signals, 1);
        let got_wave: WaveformPayload =
            bincode::deserialize(&layer2.get(CacheCategory::Waveform, "last").unwrap()).unwrap();
        assert_eq!(got_wave.signals.len(), 1);
        assert_eq!(got_wave.signals[0].name, "count");
        assert_eq!(got_wave.signals[0].width, 8);
        assert_eq!(got_wave.signals[0].kind, "output");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_populate_optimize_and_expression_from_snapshot() {
        // Statistik optimasi elaborator (db.md "6. optimize/", "10. expression/")
        // di-populate dari OptimizeSnapshot (Cell counters → snapshot) dan
        // dibaca ulang dari cache.
        let root = test_root("opt_expr");
        let db = root.join("db");
        std::fs::create_dir_all(&db).unwrap();
        let mut layer = CacheLayer::open(&db, "pid", 0).unwrap();
        let path = PathBuf::from("m.sv");
        let design = sample_design();
        let input = CachePopulateInput {
            designs: vec![(&path, &design)],
            ir_design: None,
            expanded_design: None,
            opt_snapshot: Some(mivon_elaboration::util::OptimizeSnapshot {
                const_folds: 5,
                loop_unrolls: 2,
                unrolled_stmts: 16,
                expr_evals: 42,
                expr_samples: vec![("WIDTH*8".to_string(), 256)],
            }),
        };
        CachePopulator::populate(&mut layer, &input);
        layer.save().unwrap();

        let opt: OptimizePayload =
            bincode::deserialize(&layer.get(CacheCategory::Optimize, "last").unwrap()).unwrap();
        assert_eq!(opt.const_folds, 5);
        assert_eq!(opt.loop_unrolls, 2);
        assert_eq!(opt.unrolled_stmts, 16);
        let expr: ExpressionPayload =
            bincode::deserialize(&layer.get(CacheCategory::Expression, "last").unwrap()).unwrap();
        assert_eq!(expr.expr_evals, 42);
        assert_eq!(expr.samples, vec![("WIDTH*8".to_string(), 256)]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
