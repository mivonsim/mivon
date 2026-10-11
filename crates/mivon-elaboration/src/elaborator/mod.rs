use std::collections::HashMap;

use super::util::*;
use mivon_ast::const_eval_ext::{eval_cval_full, eval_param_default_full, CVal, SField};
use mivon_ast::types::const_eval_with_params;
use mivon_ast::*;
use mivon_core::diagnostics::diagnostic::{
    DiagCode, DiagLevel, DiagSink, Diagnostic, RuntimeContext, SourceSnippet,
};
use mivon_core::error::SimError;
use mivon_core::intern::Symbol;
pub mod ext;
pub mod flatten;

/// Flatten nested array-literal `Concat` ke daftar elemen row-major (F39).
/// `elems` = elemen terluar `'{...}`; `dims` = seluruh dimensi unpacked
/// (`array_dims` sinyal). Contoh: dims [2,2] dan `'{'{1,2},'{3,4}}` →
/// [1,2,3,4]. Daftar datar (`'{1,2,3,4}` dengan len == produk dims) juga
/// diterima: flat index == elemen row-major. Struktur yang tidak cocok
/// (panjang salah / bercampur) → None (pemanggil jatuh ke write utuh).
fn flatten_array_init<'a>(elems: &'a [Expr], dims: &[usize]) -> Option<Vec<&'a Expr>> {
    if dims.is_empty() {
        return None;
    }
    if elems.len() == dims.iter().product::<usize>() {
        // Daftar datar menutupi semua elemen — flat langsung.
        return Some(elems.iter().collect());
    }
    if elems.len() != dims[0] {
        return None;
    }
    // Bersarang: tiap elemen = Concat untuk sisa dims.
    let mut out: Vec<&Expr> = Vec::with_capacity(dims.iter().product());
    for e in elems {
        match e {
            Expr::Concat(inner) => {
                let sub = flatten_array_init(inner, &dims[1..])?;
                out.extend(sub);
            }
            _ => return None,
        }
    }
    Some(out)
}

/// Format a prefix + integer counter into a Symbol without heap allocation.
/// Uses a stack buffer and writes the decimal number directly.
fn format_sym(prefix: &[u8], n: usize) -> Symbol {
    let mut buf = [0u8; 32];
    let plen = prefix.len().min(buf.len() - 1);
    buf[..plen].copy_from_slice(&prefix[..plen]);
    let mut i = n;
    let mut end = buf.len();
    loop {
        end -= 1;
        buf[end] = b'0' + (i % 10) as u8;
        i /= 10;
        if i == 0 {
            break;
        }
    }
    // Shift digits to right after prefix
    let dlen = buf.len() - end;
    let total = plen + dlen;
    buf.copy_within(end..end + dlen, plen);
    Symbol::intern(unsafe { std::str::from_utf8_unchecked(&buf[..total]) })
}

/// Signedness AST-expr dalam konteks param default (mirror const_eval: desimal
/// unsized signed; literal sized ber-suffix `s` signed; unary ±/~ mewarisi).
fn elab_expr_is_signed(e: &Expr) -> bool {
    match e {
        Expr::Value(Value::Decimal(_)) => true,
        Expr::Value(Value::Binary { is_signed, .. })
        | Expr::Value(Value::Hex { is_signed, .. })
        | Expr::Value(Value::Octal { is_signed, .. }) => *is_signed,
        Expr::Paren(inner) => elab_expr_is_signed(inner),
        Expr::UnaryOp { op, expr: inner } => match op {
            UnaryOp::Not
            | UnaryOp::ReductionAnd
            | UnaryOp::ReductionNand
            | UnaryOp::ReductionOr
            | UnaryOp::ReductionNor
            | UnaryOp::ReductionXor
            | UnaryOp::ReductionXnor => false,
            _ => elab_expr_is_signed(inner),
        },
        _ => false,
    }
}

/// Signedness deklarasi param SV (IEEE 1800 §6.11/§6.20): `int/integer/byte/
/// shortint/longint`/signed-typed → signed; tanpa tipe → signedness default
/// ekspresi (desimal signed; `localparam A = -2` → int implicit signed).
fn param_decl_is_signed(p: &ParamDecl) -> bool {
    if let Some(dt) = &p.dtype {
        return is_signed_type(dt);
    }
    match &p.default {
        Some(expr) => elab_expr_is_signed(expr),
        None => true, // implicit integer
    }
}

/// Emit konstanta param dengan signedness: signed → `Signed(Const)` agar
/// is_signed_expr + perbandingan/aritmetik (LRM §11.8.2) memperlakukannya
/// signed (`localparam int X = -2` → `X < 0` benar, `%0d` cetak -2).
fn const_or_signed(lv: LogicVec, is_signed: bool) -> IrExpr {
    if is_signed {
        IrExpr::Signed(Box::new(IrExpr::Const(lv)))
    } else {
        IrExpr::Const(lv)
    }
}

/// Apakah ekspresi mereferensikan param SIGNED (nama di `signed_params`)?
/// Dipakai guard const-fold: `const_eval_with_params` memperlakukan Ident
/// sebagai unsigned, sehingga `PD < 0` (localparam int -2) salah ter-fold
/// jadi false. Referensi param signed → biarkan jalur runtime yang memakai
/// IrExpr::Signed agar perbandingan/aritmetik benar (probe p12f/p12g).
fn expr_refs_signed_param(expr: &Expr, signed_params: &std::collections::HashSet<Symbol>) -> bool {
    if signed_params.is_empty() {
        return false; // fast path: tidak ada param signed di module ini
    }
    match expr {
        Expr::Ident { name, .. } => signed_params.contains(name),
        Expr::ScopedIdent { package, item, .. } => signed_params.contains(&Symbol::intern(
            &format!("{}::{}", package.as_str(), item.as_str()),
        )),
        Expr::Paren(inner) => expr_refs_signed_param(inner, signed_params),
        Expr::UnaryOp { expr: inner, .. } => expr_refs_signed_param(inner, signed_params),
        Expr::BinaryOp { lhs, rhs, .. } => {
            expr_refs_signed_param(lhs, signed_params) || expr_refs_signed_param(rhs, signed_params)
        }
        Expr::TernaryOp {
            cond,
            true_expr,
            false_expr,
        } => {
            expr_refs_signed_param(cond, signed_params)
                || expr_refs_signed_param(true_expr, signed_params)
                || expr_refs_signed_param(false_expr, signed_params)
        }
        Expr::Concat(parts) => parts
            .iter()
            .any(|p| expr_refs_signed_param(p, signed_params)),
        Expr::Replicate { count, expr: inner } => {
            expr_refs_signed_param(count, signed_params)
                || expr_refs_signed_param(inner, signed_params)
        }
        Expr::MemberAccess { obj, .. } => expr_refs_signed_param(obj, signed_params),
        Expr::RangeSelect {
            expr: inner,
            msb,
            lsb,
        } => {
            expr_refs_signed_param(inner, signed_params)
                || expr_refs_signed_param(msb, signed_params)
                || expr_refs_signed_param(lsb, signed_params)
        }
        Expr::BitSelect { expr: inner, index } => {
            expr_refs_signed_param(inner, signed_params)
                || expr_refs_signed_param(index, signed_params)
        }
        Expr::PartSelect {
            expr: inner,
            base,
            width,
        } => {
            expr_refs_signed_param(inner, signed_params)
                || expr_refs_signed_param(base, signed_params)
                || expr_refs_signed_param(width, signed_params)
        }
        Expr::FuncCall { args, .. } => args
            .iter()
            .any(|a| expr_refs_signed_param(a, signed_params)),
        Expr::MethodCall {
            obj,
            args,
            with_clause,
            ..
        } => {
            expr_refs_signed_param(obj, signed_params)
                || args
                    .iter()
                    .any(|a| expr_refs_signed_param(a, signed_params))
                || with_clause
                    .as_ref()
                    .is_some_and(|w| expr_refs_signed_param(w, signed_params))
        }
        Expr::Cast { expr: inner, .. } | Expr::CastWidth { expr: inner, .. } => {
            expr_refs_signed_param(inner, signed_params)
        }
        Expr::Inside {
            expr: inner,
            range_list,
        } => {
            expr_refs_signed_param(inner, signed_params)
                || range_list
                    .iter()
                    .any(|r| expr_refs_signed_param(r, signed_params))
        }
        Expr::StreamingConcat { slices, .. } => slices
            .iter()
            .any(|s| expr_refs_signed_param(s, signed_params)),
        Expr::Dist { expr: inner, items } => {
            expr_refs_signed_param(inner, signed_params)
                || items.iter().any(|d| match d {
                    mivon_ast::DistItem::Value(e, _) | mivon_ast::DistItem::Range(e, _, _) => {
                        expr_refs_signed_param(e, signed_params)
                    }
                })
        }
        _ => false,
    }
}
pub mod always;
pub mod classes;
pub mod expr;
pub mod stmt;
pub mod struct_lit;

use stmt::{lvalue_signal_id, propagate_context_width};
pub mod types;
use mivon_ir::*;

/// Kumpulkan nama modul yang di-instansiasi sebuah module item, termasuk
/// instance di dalam blok generate (recursive: If/For/Case/Items).
fn collect_inst_names(item: &ModuleItem, out: &mut Vec<Symbol>) {
    match item {
        ModuleItem::Instance(inst) => out.push(inst.module_name),
        ModuleItem::Generate(GenerateBlock { items }) => {
            for gi in items {
                match gi {
                    GenerateItem::If {
                        true_items,
                        false_items,
                        ..
                    } => {
                        for it in true_items.iter().chain(false_items.iter()) {
                            collect_inst_names(it, out);
                        }
                    }
                    GenerateItem::For { body_items, .. } => {
                        for it in body_items {
                            collect_inst_names(it, out);
                        }
                    }
                    GenerateItem::Case { items, default, .. } => {
                        for ci in items {
                            for m in &ci.body {
                                collect_inst_names(m, out);
                            }
                        }
                        if let Some(mis) = default {
                            for m in mis {
                                collect_inst_names(m, out);
                            }
                        }
                    }
                    GenerateItem::Items(mis) => {
                        for m in mis {
                            collect_inst_names(m, out);
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

/// Graf instansiasi module-level: nama module → nama module yang di-instansiasi
/// (direct + generate). Dipakai auto top resolution (cone size) dan penentuan
/// candidate top (module yang TIDAK di-instansiasi siapa pun).
fn build_inst_graph(design: &Design) -> HashMap<Symbol, Vec<Symbol>> {
    let mut graph: HashMap<Symbol, Vec<Symbol>> = HashMap::new();
    for m in &design.modules {
        let mut deps: Vec<Symbol> = Vec::new();
        for item in &m.items {
            collect_inst_names(item, &mut deps);
        }
        graph.insert(m.name, deps);
    }
    graph
}

/// Ukuran cone (BFS) sebuah module candidate: banyaknya modul yang di-reach
/// secara transitif. SoC chip top (menginstansiasi ratusan modul) jauh lebih
/// besar daripada testbench/bind/assertion kecil — diskriminator utama auto
/// top resolution.
fn module_cone_size(start: Symbol, graph: &HashMap<Symbol, Vec<Symbol>>) -> usize {
    let mut seen: std::collections::HashSet<Symbol> = std::collections::HashSet::new();
    let mut stack: Vec<Symbol> = vec![start];
    while let Some(n) = stack.pop() {
        if !seen.insert(n) {
            continue;
        }
        if let Some(deps) = graph.get(&n) {
            stack.extend(deps.iter().copied());
        }
    }
    seen.len()
}

/// Skor kandidat top untuk auto resolution: ukuran cone (transitive module
/// count) sebagai bobot DOMINAN (×20) + sinyal nama sebagai tie-breaker.
/// Preferensi simulasi (`verilator`/`_sim`/`tb`) dan `chip`; penalti wrapper
/// teknologi khusus (`_asic`/`_cw`/`fpga`) dan assertion bind
/// (`_bind`/`_fpv`/`_sva`/`_assert`) yang bukan top fungsional. Bobot cone
/// sengaja jauh di atas bonus nama — SoC chip (cone 500+) selalu menang atas
/// testbench kecil walau tb diberi bonus nama.
fn score_auto_top(name: Symbol, cone: usize) -> i64 {
    let lower = name.as_str().to_ascii_lowercase();
    let mut s = (cone as i64) * 20;
    if lower.contains("verilator")
        || lower.contains("_sim")
        || lower.starts_with("tb")
        || lower.contains("_tb")
    {
        s += 100;
    }
    if lower.contains("chip") {
        s += 100;
    }
    if lower.contains("_asic") || lower.contains("_cw") || lower.contains("fpga") {
        s -= 300;
    }
    if lower.contains("_bind")
        || lower.contains("_fpv")
        || lower.contains("_sva")
        || lower.contains("_assert")
        || lower.contains("_sca_wrapper")
    {
        s -= 500;
    }
    s
}

/// Batas lebar packed vector (bit) yang masih di-elaborate. Di atas batas ini
/// input ditolak dengan diag bersih (E3012) — bukan clamp senyap / hang /
/// OOM. Nilai jauh di atas packed terbesar realistis (OpenTitan SRAM elem 76
/// bit; datapath kripto 4096 bit) tapi di bawah ambang alokasi absurd.
/// Temuan fuzzer: `parameter W = 2**31` lalu `logic [W-1:0]` → elab hang
/// (loop init 256 × 2^31 iterasi) di inisialisasi array.
const MAX_PACKED_WIDTH: usize = 1 << 24;

const BUILTIN_UVM_CLASSES: &[&str] = &[
    "uvm_object",
    "uvm_transaction",
    "uvm_component",
    "uvm_sequence_item",
    "uvm_tr_database",
    "uvm_tr_stream",
    "uvm_sequence",
    "uvm_sequencer",
    "uvm_driver",
    "uvm_monitor",
    "uvm_env",
    "uvm_agent",
    "uvm_scoreboard",
    "uvm_analysis_port",
    "uvm_analysis_imp",
    "uvm_analysis_export",
    "uvm_subscriber",
    "uvm_tlm_fifo",
    "uvm_seq_item_port",
    "uvm_test",
    "uvm_config_db",
    "uvm_report_object",
    "uvm_factory",
    "uvm_resource_db",
    // VERIF-03: uvm_cmdline_processor — singleton pembaca plusarg CLI.
    "uvm_cmdline_processor",
    // VERIF-12: uvm_printer / uvm_table_printer — format object jadi tabel.
    "uvm_printer",
    "uvm_table_printer",
    // VERIF-13: uvm_comparator / uvm_in_order_comparator — pembanding TLM.
    "uvm_comparator",
    "uvm_in_order_comparator",
    // VERIF-15: uvm_heartbeat — monitor liveness object.
    "uvm_heartbeat",
    // VERIF-04: uvm_root — singleton root + top-level component.
    "uvm_root",
    // VERIF-05: uvm_phase — handle phase (jump/get_name/skip).
    "uvm_phase",
    // VERIF-20: OVM compatibility — OVM class names map to UVM equivalents.
    "ovm_object",
    "ovm_component",
    "ovm_driver",
    "ovm_monitor",
    "ovm_sequencer",
    "ovm_sequence",
    "ovm_sequence_item",
    "ovm_env",
    "ovm_agent",
    "ovm_test",
    "ovm_scoreboard",
    "ovm_config_db",
    "ovm_factory",
    "ovm_report_object",
    "ovm_tlm_fifo",
    "ovm_analysis_port",
    "ovm_analysis_imp",
    "ovm_analysis_export",
    "ovm_subscriber",
];

/// Kumpulkan nama module yang diinstansiasi dari daftar ModuleItem,
/// MENURUNI generate block (If/For/Case/Items). Instansiasi di dalam
/// generate — raw AST maupun hasil partial expansion — wajib terlihat oleh
/// reachability analysis dari top; tanpa ini module yang sah (mis. ibex_decoder
/// di dalam ibex_id_stage) bisa salah di-flag unreachable dan di-prune.
/// LANG-08: ekstrak nama net (Ident polos / ScopedIdent sederhana) dari
/// ekspresi lhs/rhs `alias a = b;` → SignalId bila ada di top signal map.
/// Bit-select (`a[3]`) tidak didukung utk alias (bisa ditambah nanti).
fn net_expr_signal_id(expr: &mivon_ast::Expr, map: &HashMap<Symbol, SignalId>) -> Option<SignalId> {
    match expr {
        mivon_ast::Expr::Ident { name, .. } => map.get(name).copied(),
        // `pkg::net` scoped — peta signal top memakai nama polos; hanya
        // resolve bila nama cocok persis.
        mivon_ast::Expr::ScopedIdent { item, .. } => map.get(item).copied(),
        _ => None,
    }
}

fn collect_instance_names(items: &[ModuleItem], out: &mut Vec<Symbol>) {
    for item in items {
        match item {
            ModuleItem::Instance(inst) => out.push(inst.module_name),
            ModuleItem::Generate(gen) => {
                for gi in &gen.items {
                    match gi {
                        GenerateItem::If {
                            true_items,
                            false_items,
                            ..
                        } => {
                            collect_instance_names(true_items, out);
                            collect_instance_names(false_items, out);
                        }
                        GenerateItem::For { body_items, .. } => {
                            collect_instance_names(body_items, out);
                        }
                        GenerateItem::Case { items, default, .. } => {
                            for ci in items {
                                collect_instance_names(&ci.body, out);
                            }
                            if let Some(d) = default {
                                collect_instance_names(d, out);
                            }
                        }
                        GenerateItem::Items(items) => {
                            collect_instance_names(items, out);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElaborateMode {
    StrictSimulation,
    AnalysisRecovery,
}

pub struct Elaborator {
    pub design: Design,
    pub modules: HashMap<Symbol, IrModule>,
    pub param_vals: HashMap<Symbol, i64>,
    /// Lebar type param module yang SEDANG dielaborasi (nama → lebar bit
    /// efektif). Lihat inisialisasi di constructor untuk dokumentasi.
    pub cur_type_param_widths: HashMap<Symbol, usize>,
    /// Nama param bertipe SIGNED (int/integer/byte/.../sign-typed) pada module
    /// saat ini. IEEE 1800 §6.11 + §6.20: `localparam int X = -2` bersifat
    /// signed — tanpa ini, resolve param jadi `Const(u64,64)` UNSIGNED
    /// sehingga `X < 0` false dan `%0d` cetak 2^64-2 (probe p12f/p12g).
    pub param_is_signed: std::collections::HashSet<Symbol>,
    /// LANG-40: `let` declaration module saat ini (Symbol → LetDecl) — alias
    /// ekspresi scoped (IEEE 1800-2017 §11.12.2). Di-set per-module di
    /// elaborate_module_with_params_and_type; di-resolve elaborate_expr
    /// (ident untuk let tanpa parameter, FuncCall untuk let berparameter).
    pub let_decls: HashMap<Symbol, LetDecl>,
    /// LANG-10: deklarasi checker (nama → CheckerDecl) — dikumpulkan dari
    /// module items saat elaborate; instance checker di-resolve ke sini lalu
    /// assertion body di-drive dengan port binding.
    pub checker_decls: HashMap<Symbol, CheckerDecl>,
    pub typedef_map: HashMap<Symbol, usize>,
    pub typedef_field_map: HashMap<Symbol, Vec<StructFieldInfo>>,
    /// Range + packed dims typedef module/package (untuk mengisi `packed_dims`
    /// signal bertipe UserDefined seperti `box_t` → `[4:0][4:0][W-1:0]`).
    pub typedef_dims: HashMap<Symbol, (Option<ExprRange>, Vec<ExprRange>)>,
    pub package_symbols: HashMap<Symbol, HashMap<Symbol, PackageItem>>,
    /// Konstanta package ter-evaluasi (kualifikasi `pkg::name`): skalar & array.
    pub pkg_const_scalars: HashMap<Symbol, i64>,
    pub pkg_const_arrays: HashMap<Symbol, Vec<i64>>,
    /// Index konstanta STRUCT package per base name (`name` → fields),
    /// dibangun SEKALI dari `pkg_const_scalars` (key `pkg::name.<field>`).
    /// Dipakai body-param loop untuk default param berbentuk ident
    /// (`Info = PartInfoDefault`) — lookup O(1), bukan scan map per param
    /// (bottleneck ~30k key × ribuan param di OpenTitan).
    pub pkg_struct_ref_index: HashMap<Symbol, Vec<SField>>,
    /// Context package global (qualified `pkg::name` + enum members) yang
    /// dihitung SEKALI per compile. Tidak bergantung pada module sehingga bisa
    /// dipakai bersama (clone) oleh semua module — menghindari rescan semua
    /// package + fixed-point 64 iterasi untuk tiap module (bottleneck di
    /// desain besar seperti OpenTitan).
    pub pkg_param_ctx: HashMap<Symbol, i64>,
    /// Context hasil `import pkg::*` / `import pkg::item` level $unit
    /// (unit_imports). Juga konstan antar-module sehingga dihitung sekali.
    pub unit_import_ctx: HashMap<Symbol, i64>,
    /// Nilai plain param per package (dengan base = context global package).
    /// Dipakai untuk `import pkg::item`/`pkg::*` milik module — menghindari
    /// evaluasi ulang default param package untuk tiap module.
    pub pkg_plain_params: HashMap<Symbol, HashMap<Symbol, i64>>,
    pub specialized_classes: std::cell::RefCell<Vec<ClassDecl>>,
    pub diag_sink: DiagSink,
    /// F28: job alias hier port interface (port_name, iface_name, inst_name)
    /// yang dikumpulkan saat flatten — diproses POST-pass di flatten_instances
    /// agar tidak bergantung pada urutan sub_instances (instance interface
    /// boleh muncul setelah child module di AST).
    pub iface_alias_jobs: std::cell::RefCell<Vec<(Symbol, Symbol, Symbol)>>,
    /// Package tempat function sedang di-inline. Dipakai untuk resolve fungsi
    /// saudara plain-name di body function (mis. `mubi4_and` dipanggil di dalam
    /// `mubi4_and_hi`) tanpa perlu import eksplisit di module.
    pub inline_func_pkg: std::cell::Cell<Option<Symbol>>,
    /// Peta (nama function → package asal) untuk fungsi package yang disalin ke
    /// body module via `import pkg::func`. Dipakai resolve fungsi saudara yang
    /// dipanggil di dalam body function yang di-inline (AST inline pass).
    pub func_source_pkg: HashMap<Symbol, HashMap<Symbol, Symbol>>,
    pub source_lines: Vec<String>,
    pub source_file: String,
    /// Cache hasil `find_name_in_source` per nama (pure function dari
    /// source_lines). Nama yang sama di-query berkali-kali (mis. pesan error
    /// `'X' not found in parameter context` untuk parameter yang sama di
    /// banyak module) — tanpa cache, setiap query meng-scan 1.1M baris merged
    /// source (~1.5-2s) yang menjadi bottleneck terbesar di desain besar.
    pub source_name_loc: std::cell::RefCell<HashMap<Symbol, (usize, usize)>>,
    /// Cache hasil `find_literal_in_source` — hasilnya KONSTAN per source
    /// (targets literal tetap, message tidak memengaruhi hasil). Sebelumnya
    /// dihitung ulang untuk SETIAP panggilan fallback error-path
    /// (elab_diag_at line=0,col=0 → nama tidak plausibel → literal scan):
    /// desain besar (OpenTitan) memicu ribuan panggilan × scan penuh 1.1M
    /// baris merged source (~1.5-2s per panggilan) → run terlihat hang
    /// (100% CPU, tidak pernah selesai). Memoize SEKALI per Elaborator
    /// (source_lines immutable selama elaborasi).
    pub source_literal_loc: std::cell::RefCell<Option<(usize, usize)>>,
    /// SIM-29: peta baris statement untuk line coverage exclusion — key
    /// `format!("{}.{:?}", process_name, discriminant)` SAMA dengan key
    /// `record_line_hit` di engine. Diisi saat menerjemahkan statement
    /// (elaborate_stmt) dari AST via `expr_location`; disalin ke IrDesign
    /// di akhir `elaborate`. RefCell karena `elaborate_stmt` menerima `&self`.
    pub stmt_lines: std::cell::RefCell<HashMap<Symbol, usize>>,
    /// SIM-29: nama proses yang sedang dielaborasi (untuk key stmt_lines).
    pub current_proc_name: std::cell::RefCell<Option<Symbol>>,
    pub current_module: Option<Symbol>,
    /// Faktor skala delay per module (LRM 1800-2017 §19.8).
    ///
    /// Tick simulasi mivon memakai satuan DASAR (finest) design. Dulu
    /// `design.timescale` diambil dari directive TERAKHIR, jadi `#5` di module
    /// `1us/1ns` ikut diskalakan seolah 5 tick basis `1us` (atau `1ns`),
    /// padahal LRM: satuan delay milik module itu sendiri.
    ///
    /// `module_delay_scale: module → faktor` (1 = sudah basis). Delay
    /// `IrStmt::Delay` dikali faktor ini saat elaborasi. Key `None`/tak
    /// terdaftar = 1 (fallback kompatibilitas).
    pub module_delay_scale: std::collections::HashMap<Symbol, u64>,
    /// Set module reachable dari top (F38). Error elaborasi di module yang
    /// TIDAK ada di set ini (TB/DV terpisah, dependensi hilang) di-downgrade
    /// ke warning agar tidak memblokir cone RTL yang valid.
    pub reachable: std::collections::HashSet<Symbol>,
    /// Top-level tidak bisa di-resolve secara unik (multiple candidate tops,
    /// circular hierarchy, atau root module tidak ada) dan fallback recovery
    /// terpaksa dipakai. Dipakai main.rs untuk menonaktifkan simulasi/VCD:
    /// mode analisis berhasil (diagnostik dilaporkan), tapi desain tidak
    /// boleh disimulasikan dari modul tebakan.
    pub recovered: bool,
    // ── Incremental elaboration cache ──
    /// Global cache: signature → cached IrModule (session-wide)
    pub module_cache: HashMap<u64, IrModule>,
    /// Stats for profiling
    pub cache_hits: usize,
    pub cache_misses: usize,
    // ── Instance param-IR cache (flatten) ──
    /// Cache IR hasil `elaborate_module_with_params_and_type` per
    /// (module_name, param override signature). OpenTitan punya ribuan
    /// instance dengan override IDENTIK (mis. `prim_buf #(.W(8))` dipakai
    /// puluhan kali) — tanpa cache, setiap instance meng-clone AST + resolve
    /// 62k-ctx + elaborasi penuh ulang. Dengan cache, pekerjaan berat hanya
    /// dilakukan SEKALI per signature unik. Dibatasi (bounded) agar memori
    /// tidak membengkak pada desain dengan ribuan signature berbeda.
    pub param_ir_cache: HashMap<(Symbol, u64), IrModule>,
    /// AST modul SEBELUM expand generate (hanya untuk modul yang punya param
    /// DAN generate block). `expand_all_generates` memutasi
    /// `design.modules` in-place memakai nilai param DEFAULT — sehingga
    /// instance dengan override (`.W(8)`) meng-clone AST yang generate-nya
    /// sudah ter--expand dengan nilai default (bug: `gen_sub #(.W(8))` dgn
    /// `for (gi=0; gi<W...)` hanya ter-expand 4 iterasi). Snapshot ini
    /// dipakai flatten saat re-elaborasi per signature param.
    pub pristine_param_modules: HashMap<Symbol, Module>,
    /// Statistik optimasi untuk cache pipeline (db.md "6. optimize/",
    /// "10. expression/") — const fold, loop unroll, evaluasi ekspresi.
    pub opt_stats: super::util::OptStats,
    // Debug per-module (DBG_ELAB_STEP): kumulatif assign-context-width utk
    // mengidentifikasi bottleneck 221ms/assign rv_core_ibex (10 assign ~2s).
    pub(crate) dbg_apply_us: std::cell::Cell<u64>,
    pub(crate) dbg_apply_n: std::cell::Cell<u64>,
    /// Cache nama DPI import dari design (dihitung SEKALI di constructor).
    /// Dipakai oleh is_dpi check di expr.rs/stmt.rs untuk menentukan apakah
    /// function tak dikenal harus di-degrade ke DPI stub. Tanpa cache ini,
    /// mem::take(&mut self.design.modules) di elaboration loop mengosongkan
    /// modules → is_dpi selalu false → semua DPI function jadi hard error E3001.
    pub dpi_import_names: std::collections::HashSet<Symbol>,
    /// Index `module name → index di design.modules`, dibangun SEKALI di
    /// `elaborate()` sebelum loop elaborasi. design.modules TIDAK berubah
    /// selama loop, jadi index aman di-reuse. Sebelumnya di-rebuild untuk
    /// SETIAP module di `elaborate_module_with_params_and_type` → O(n²)
    /// (4002 module × 4002 insert = 16M hash insert ≈ 62% total instruction
    /// elaborate di desain besar — confirmed callgrind demo3200 4.8G instr).
    pub module_idx: HashMap<Symbol, usize>,
}

impl Elaborator {
    pub fn new(design: Design) -> Self {
        Self::with_source(design, Vec::new(), String::new())
    }

    /// F38: apakah module yang sedang di-elaborasi termasuk cone reachable
    /// dari top. False untuk TB/DV terpisah atau module dgn dependensi hilang
    /// — error-nya di-downgrade ke warning agar tidak memblokir cone valid.
    pub(crate) fn is_current_module_reachable(&self) -> bool {
        match self.current_module {
            Some(m) => self.reachable.contains(&m),
            None => true,
        }
    }

    /// Fragment modules (implicit unit wrappers for include snippets like
    /// compare_helper.sv, abs.sv) have no real declarations; signals come
    /// from the enclosing context. All elaboration errors in these modules
    /// are downgraded to warnings (mirrors the parse-level fragment
    /// downgrade: files without real modules → non-blocking diagnostics).
    pub(crate) fn is_fragment_module(&self) -> bool {
        self.current_module
            .map(|m| m.as_str().starts_with("__unit__"))
            .unwrap_or(false)
    }

    /// Set module reachable dari top (BFS closure atas instance graph).
    /// Dipakai DUA kali: (a) PRE-expansion utk meng-gate generate expansion
    /// (hemat kerja di luar cone saat `--top`), (b) POST-expansion (block
    /// asli di elaborate) utk pruning main-elaborate + warning unreachable.
    /// Top eksplisit → closure darinya; tanpa top → kandidat root pertama
    /// (module yang tidak diinstansiasi module lain) — konsisten dgn
    /// pemilihan auto-top di bawah elaborate().
    pub(crate) fn compute_reachable_set(
        &self,
        top: Option<Symbol>,
        _include_warnings: bool,
    ) -> std::collections::HashSet<Symbol> {
        use std::collections::{HashSet, VecDeque};
        let module_map: HashMap<Symbol, &Module> =
            self.design.modules.iter().map(|m| (m.name, m)).collect();
        let all_names: HashSet<Symbol> = module_map.keys().copied().collect();
        let mut reachable: HashSet<Symbol> = HashSet::new();
        let mut queue: VecDeque<Symbol> = VecDeque::new();
        if let Some(ref t) = top {
            if all_names.contains(t) {
                queue.push_back(*t);
                reachable.insert(*t);
            }
        } else if let Some(cand) = {
            let mut instantiated: HashSet<Symbol> = HashSet::new();
            for m in &self.design.modules {
                let mut insts = Vec::new();
                collect_instance_names(&m.items, &mut insts);
                for mn in insts {
                    instantiated.insert(mn);
                }
            }
            self.design
                .modules
                .iter()
                .find(|m| !instantiated.contains(&m.name))
        } {
            queue.push_back(cand.name);
            reachable.insert(cand.name);
        } else if let Some(first) = self.design.modules.first() {
            queue.push_back(first.name);
            reachable.insert(first.name);
        }
        while let Some(name) = queue.pop_front() {
            if let Some(module) = module_map.get(&name) {
                let mut insts = Vec::new();
                collect_instance_names(&module.items, &mut insts);
                for mn in insts {
                    if all_names.contains(&mn) && !reachable.contains(&mn) {
                        reachable.insert(mn);
                        queue.push_back(mn);
                    }
                }
            }
        }
        reachable
    }

    pub fn with_source(design: Design, source_lines: Vec<String>, source_file: String) -> Self {
        let mut package_symbols: HashMap<Symbol, HashMap<Symbol, PackageItem>> = HashMap::new();
        // First pass: collect directly declared items
        for pkg in &design.packages {
            let mut items = HashMap::new();
            for item in &pkg.items {
                let name = match item {
                    PackageItem::Param(p) => p.name,
                    PackageItem::Typedef(t) => t.name,
                    PackageItem::Function(f) => f.name,
                    PackageItem::Task(t) => t.name,
                    PackageItem::Class(c) => c.name,
                    PackageItem::Decl(d) => {
                        d.names.first().map(|v| v.name).unwrap_or(Symbol::EMPTY)
                    }
                    PackageItem::Import { .. } => continue,
                    PackageItem::Export { .. } => continue,
                    PackageItem::DpiImport(_) => continue,
                };
                items.insert(name, item.clone());
            }
            // Package dengan nama yang SAMA bisa muncul dari beberapa top
            // (OpenTitan: `tl_main_pkg` ada di top_darjeeling + top_earlgrey +
            // top_englishbreakfast dengan item yang BERBEDA — mis.
            // `ADDR_SPACE_ROM_CTRL0__ROM` hanya ada di copy darjeeling).
            // MODULE di-dedup "tie → definisi TERAKHIR" (lihat blok dedup
            // module); package harus konsisten dengan pilihan module, jadi
            // item yang berkonflik memakai definisi TERAKHIR (bukan
            // first-wins). Sebelumnya first-wins membuat package = varian
            // top PERTAMA di filelist (darjeeling) sedangkan module =
            // varian TERAKHIR (englishbreakfast) → `hw2reg.recov_err_code.
            // io_div4_measure_err.d` di clkmgr tidak ter-resolve (struct
            // `clkmgr_hw2reg_recov_err_code_reg_t` varian pertama cuma 5
            // field, tidak punya io_div4_measure_err) → E2001.
            let pkg_items = package_symbols.entry(pkg.name).or_default();
            for (k, v) in items {
                pkg_items.insert(k, v);
            }
        }
        // Second pass: resolve imports within packages
        let imports: Vec<(Symbol, Symbol, Symbol)> = design
            .packages
            .iter()
            .flat_map(|pkg| {
                pkg.items.iter().filter_map(|item| {
                    if let PackageItem::Import {
                        package,
                        item: import_item,
                    } = item
                    {
                        Some((pkg.name, *package, *import_item))
                    } else {
                        None
                    }
                })
            })
            .collect();
        for (pkg_name, source_pkg_name, import_item) in &imports {
            let source_items = package_symbols.get(source_pkg_name).cloned();
            if let Some(source_items) = source_items {
                if let Some(pkg_items) = package_symbols.get_mut(pkg_name) {
                    let names: Vec<Symbol> = if import_item == &Symbol::intern("*") {
                        source_items.keys().copied().collect()
                    } else {
                        vec![*import_item]
                    };
                    for name in names {
                        if let Some(source_item) = source_items.get(&name) {
                            pkg_items.entry(name).or_insert(source_item.clone());
                        }
                    }
                }
            }
        }
        // Third pass: resolve exports within packages (re-export items from other packages)
        let exports: Vec<(Symbol, Symbol, Symbol)> = design
            .packages
            .iter()
            .flat_map(|pkg| {
                pkg.items.iter().filter_map(|item| {
                    if let PackageItem::Export {
                        package,
                        item: export_item,
                    } = item
                    {
                        Some((pkg.name, *package, *export_item))
                    } else {
                        None
                    }
                })
            })
            .collect();
        for (pkg_name, source_pkg_name, export_item) in &exports {
            let source_items = package_symbols.get(source_pkg_name).cloned();
            if let Some(source_items) = source_items {
                if let Some(pkg_items) = package_symbols.get_mut(pkg_name) {
                    let names: Vec<Symbol> = if export_item == &Symbol::intern("*") {
                        source_items.keys().copied().collect()
                    } else {
                        vec![*export_item]
                    };
                    for name in names {
                        if let Some(source_item) = source_items.get(&name) {
                            pkg_items.entry(name).or_insert(source_item.clone());
                        }
                    }
                }
            }
        }

        // Evaluate package constants once (scalars + arrays)
        // eval_package_constants menerima referensi — tidak perlu clone map.
        let (pkg_const_scalars, pkg_const_arrays) =
            mivon_ast::const_eval_ext::eval_package_constants(&package_symbols);

        // Index konstanta STRUCT package: key `pkg::name.<field>` di
        // pkg_const_scalars → `name` → fields. Dibangun sekali agar body-param
        // loop bisa resolve `Info = PartInfoDefault` via lookup O(1).
        let mut pkg_struct_ref_index: HashMap<Symbol, Vec<SField>> = HashMap::new();
        for (k, v) in &pkg_const_scalars {
            let s = k.as_str();
            if let Some(idx) = s.rfind("::") {
                let after = &s[idx + 2..];
                if let Some(dot) = after.find('.') {
                    let base = &after[..dot];
                    let field = &after[dot + 1..];
                    // Field level-1 saja (key nested `base.sub.field` tidak
                    // menghasilkan pseudo-field).
                    if !base.is_empty() && !field.is_empty() && !field.contains('.') {
                        let entry = pkg_struct_ref_index
                            .entry(Symbol::intern(base))
                            .or_default();
                        if !entry
                            .iter()
                            .any(|f| f.name.map(|n| n.as_str() == field).unwrap_or(false))
                        {
                            entry.push(SField::named(Symbol::intern(field), CVal::Scalar(*v)));
                        }
                    }
                }
            }
        }

        if std::env::var("DBG_ELAB").is_ok() {
            let gp = Symbol::intern("gpio_env_pkg");
            let dv = Symbol::intern("dv_utils_pkg");
            eprintln!("[DBG-ELAB] with_source: design.packages={} package_symbols={} gpio_env_pkg={} dv_utils_pkg={}", design.packages.len(), package_symbols.len(), package_symbols.contains_key(&gp), package_symbols.contains_key(&dv));
        }
        if std::env::var("DBG_ELAB").is_ok() {
            let mut pi_keys: Vec<&str> = pkg_const_scalars
                .keys()
                .filter(|k| {
                    k.as_str().contains("PartInfo") || k.as_str().contains("PartInfoDefault")
                })
                .map(|k| k.as_str())
                .collect();
            pi_keys.sort();
            eprintln!(
                "[DBG-ELAB] pkg_const_scalars keys with PartInfo ({}): {:?}",
                pi_keys.len(),
                &pi_keys[..pi_keys.len().min(12)]
            );
            eprintln!(
                "[DBG-ELAB] pkg_struct_ref_index len={} has PartInfo[0]={} has PartInfoDefault={}",
                pkg_struct_ref_index.len(),
                pkg_struct_ref_index.contains_key(&Symbol::intern("PartInfo[0]")),
                pkg_struct_ref_index.contains_key(&Symbol::intern("PartInfoDefault"))
            );
        }

        // Cache DPI import names SEKALI di constructor — self.design.modules
        // akan di-mem::take() saat elaboration loop, jadi is_dpi check harus
        // pakai cache ini (bukan scan self.design.modules yang sudah kosong).
        let dpi_import_names: std::collections::HashSet<Symbol> = design
            .modules
            .iter()
            .flat_map(|m| m.items.iter())
            .filter_map(|item| match item {
                ModuleItem::DpiImport(d) => Some(d.name),
                _ => None,
            })
            .collect();

        Elaborator {
            design,
            modules: HashMap::new(),
            param_vals: HashMap::new(),
            param_is_signed: std::collections::HashSet::new(),
            let_decls: HashMap::new(),
            checker_decls: HashMap::new(),
            typedef_map: HashMap::new(),
            typedef_dims: HashMap::new(),
            typedef_field_map: HashMap::new(),
            package_symbols,
            pkg_const_scalars,
            pkg_const_arrays,
            pkg_struct_ref_index,
            pkg_param_ctx: HashMap::new(),
            unit_import_ctx: HashMap::new(),
            pkg_plain_params: HashMap::new(),
            specialized_classes: std::cell::RefCell::new(Vec::new()),
            diag_sink: DiagSink::new(),
            iface_alias_jobs: std::cell::RefCell::new(Vec::new()),
            inline_func_pkg: std::cell::Cell::new(None),
            func_source_pkg: HashMap::new(),
            source_lines,
            source_file,
            source_name_loc: std::cell::RefCell::new(HashMap::new()),
            source_literal_loc: std::cell::RefCell::new(None),
            stmt_lines: std::cell::RefCell::new(HashMap::new()),
            current_proc_name: std::cell::RefCell::new(None),
            current_module: None,
            module_delay_scale: std::collections::HashMap::new(),
            // Lebar type param module yang SEDANG dielaborasi (nama → lebar
            // bit efektif, termasuk override instance). Dibaca
            // `resolve_cast_name_width` agar `T'(x)` tidak jatuh ke lebar 1
            // (data loss). Diinstal per-panggilan
            // `elaborate_module_with_params_and_type` (pola yang sama seperti
            // `current_module`: setiap elaborasi modul menimpa miliknya).
            cur_type_param_widths: HashMap::new(),
            reachable: std::collections::HashSet::new(),
            recovered: false,

            module_cache: HashMap::new(),
            cache_hits: 0,
            cache_misses: 0,
            param_ir_cache: HashMap::new(),
            pristine_param_modules: HashMap::new(),
            opt_stats: super::util::OptStats::default(),
            dbg_apply_us: std::cell::Cell::new(0),
            dbg_apply_n: std::cell::Cell::new(0),
            dpi_import_names,
            module_idx: HashMap::new(),
        }
    }

    pub fn elaborate(
        &mut self,
        top_module: Option<&str>,
        mode: ElaborateMode,
    ) -> Result<IrDesign, SimError> {
        let elab_t0 = std::time::Instant::now();
        if std::env::var("DBG_ELAB").is_ok() {
            eprintln!(
                "[DBG-ELAB] elaborate() start (n_modules={})",
                self.design.modules.len()
            );
        }
        // Build global package param context ONCE — dipakai bersama oleh semua
        // module. Sebelumnya dihitung ulang per-modul (rescan semua package +
        // fixed-point 64 iterasi) yang menjadi bottleneck di desain besar.
        self.build_pkg_param_ctx();

        // F84 (LRM 1800-2017 §19.8): hitung faktor skala delay per module.
        // Delay module `1us` = 1000 tick bila basis design `1ns`.
        self.compute_module_delay_scales();

        // LANG-10: kumpulkan deklarasi checker dari SEMUA module (pre-pass)
        // agar instance checker (di module mana pun) ter-resolve. Checker
        // bisa dideklarasikan sebelum/ sesudah instance-nya di file yang sama.
        for module in &self.design.modules {
            for item in &module.items {
                if let ModuleItem::Checker(cd) = item {
                    self.checker_decls
                        .entry(cd.name)
                        .or_insert_with(|| cd.clone());
                } else if let ModuleItem::Class(cd) = item {
                    // Collect class declarations inside modules
                    self.checker_decls.entry(cd.name).or_insert_with(|| {
                        // Convert ClassDecl to CheckerDecl for hierarchy purposes
                        CheckerDecl {
                            name: cd.name,
                            ports: Vec::new(),
                            items: Vec::new(),
                        }
                    });
                }
            }
        }

        // ── Deduplicate module definitions (pilih yang self-contained) ──
        // Filelist project besar (OpenTitan) sering memuat BEBERAPA varian
        // teknologi dari module yang sama (mis. `prim_buf`: asap7 / generic /
        // xilinx). Tanpa dedup, definisi mana yang dipakai TIDAK deterministik
        // dan bisa jatuh ke varian yang menginstansiasi modul library yang
        // tidak tersedia (mis. sel stdcell `BUFx2_ASAP7_75t_R` / `BUFGCTRL`)
        // → error E3001 palsu yang memblokir simulasi. Pilih definisi yang
        // PALING self-contained: jumlah instance yang tidak tersolve paling
        // kecil (0 = semua instance ter-resolve — umumnya implementasi generik/
        // simulasi); tie → definisi terakhir (deterministik, urutan filelist).
        {
            use std::collections::{HashMap as HMap, HashSet as HSet};
            let _before = self.design.modules.len();
            let all_names: HSet<Symbol> = self
                .design
                .modules
                .iter()
                .map(|m| m.name)
                .chain(self.design.interfaces.iter().map(|i| i.name))
                .chain(self.design.packages.iter().map(|p| p.name))
                .collect();
            let mut groups: HMap<Symbol, Vec<usize>> = HMap::new();
            for (idx, m) in self.design.modules.iter().enumerate() {
                groups.entry(m.name).or_default().push(idx);
            }
            let mut keep = vec![true; self.design.modules.len()];
            for idxs in groups.values() {
                if idxs.len() <= 1 {
                    continue;
                }
                let mut best = *idxs.last().unwrap();
                let mut best_missing = usize::MAX;
                for &idx in idxs {
                    let mut insts: Vec<Symbol> = Vec::new();
                    collect_instance_names(&self.design.modules[idx].items, &mut insts);
                    let missing = insts.iter().filter(|n| !all_names.contains(*n)).count();
                    // missing lebih kecil menang; tie (<=) → definisi terakhir
                    // (deterministik, konsisten urutan filelist).
                    if missing <= best_missing {
                        best_missing = missing;
                        best = idx;
                    }
                }
                for &idx in idxs {
                    if idx != best {
                        keep[idx] = false;
                    }
                }
            }
            let removed = keep.iter().filter(|k| !**k).count();
            if removed > 0 {
                let mut iter = keep.into_iter();
                self.design.modules.retain(|_| iter.next().unwrap());
                self.diag_sink.push(Diagnostic::new(
                    DiagLevel::Warning,
                    DiagCode::DuplicateDeclaration,
                    format!(
                        "{} module definition(s) dengan nama yang sama di-deduplikasi — memakai definisi yang paling self-contained (instance ter-resolve; tie → terakhir)",
                        removed
                    ),
                ));
            }
        }
        // ── Reachability PRE-expansion: hitung SEKALI di sini (sebelum
        // bind/inline/generate — semua prepass per-module) utk meng-gate
        // kerja hanya ke cone top saat `--top` diberikan. Binder/impor/inline
        // tidak mengubah instance module, jadi closure stabil hingga generate
        // (yang menambah instance → reachable dihitung ulang post-expansion
        // utk pruning main-elaborate). ──
        let top_sym = top_module.map(Symbol::intern);
        let reachable_pre: std::collections::HashSet<Symbol> =
            self.compute_reachable_set(top_sym, false);
        if std::env::var("DBG_ELAB").is_ok() {
            eprintln!(
                "[DBG-ELAB] pre-expansion reachable={} (top={:?})",
                reachable_pre.len(),
                top_sym.map(|s| s.as_str().to_string())
            );
        }
        if std::env::var("DBG_ELAB").is_ok() {
            let n_arr_elems: usize = self.pkg_const_arrays.values().map(|v| v.len()).sum();
            eprintln!("[DBG-ELAB] global package param ctx built in {}us ({} entries; scalars={} arrays={} array_elems={})", elab_t0.elapsed().as_micros(), self.pkg_param_ctx.len(), self.pkg_const_scalars.len(), self.pkg_const_arrays.len(), n_arr_elems);
        }
        // Process bind declarations: add bound instances to target modules
        let binds = std::mem::take(&mut self.design.binds);
        for bind in &binds {
            if let Some(target_module) = self
                .design
                .modules
                .iter_mut()
                .find(|m| m.name == bind.target)
            {
                target_module
                    .items
                    .push(ModuleItem::Instance(bind.instance.clone()));
            } else {
                let (file, dl) = self.resolve_source_location(bind.instance.line);
                let source_line =
                    if bind.instance.line > 0 && bind.instance.line <= self.source_lines.len() {
                        Some(self.source_lines[bind.instance.line - 1].clone())
                    } else {
                        None
                    };
                let mut diag = Diagnostic::new(
                    DiagLevel::Error,
                    DiagCode::ModuleNotFound,
                    format!("bind target '{}' not found", bind.target),
                )
                .with_code_context();
                if let Some(snippet) = source_line {
                    diag = diag.with_source_snippet(SourceSnippet::new(
                        file,
                        dl,
                        bind.instance.col,
                        &snippet,
                    ));
                }
                if let Some(ref mod_name) = self.current_module {
                    diag = diag
                        .with_runtime_context(RuntimeContext::new().with_module(mod_name.as_str()));
                }
                self.diag_sink.push(diag);
            }
        }

        // Pre-pass: import package functions/tasks into modules + inject $unit declarations
        let pkg_symbols = &self.package_symbols;
        let unit_funcs = &self.design.unit_funcs;
        let unit_tasks = &self.design.unit_tasks;
        let unit_imports = &self.design.unit_imports;
        for module in &mut self.design.modules {
            // ── Gate prepass ke cone top (--top) ──
            if top_sym.is_some() && !reachable_pre.contains(&module.name) {
                continue;
            }
            // Collect module-level imports
            let imports: Vec<(Symbol, Symbol)> = module
                .items
                .iter()
                .filter_map(|item| {
                    if let ModuleItem::Import {
                        package,
                        item: import_item,
                    } = item
                    {
                        Some((*package, *import_item))
                    } else {
                        None
                    }
                })
                .collect();
            // Merge with $unit-level imports
            let all_imports: Vec<(Symbol, Symbol)> = {
                let mut imps = imports;
                for (pkg, item) in unit_imports {
                    if !imps.iter().any(|(p, i)| p == pkg && i == item) {
                        imps.push((*pkg, *item));
                    }
                }
                imps
            };
            // Process package imports
            for (package, import_item) in &all_imports {
                if let Some(pkg_items) = pkg_symbols.get(package) {
                    let names: Vec<&str> = if *import_item == Symbol::intern("*") {
                        pkg_items.keys().map(|s| s.as_str()).collect()
                    } else {
                        vec![import_item.as_str()]
                    };
                    for name in names {
                        if let Some(pkg_item) = pkg_items.get(&Symbol::intern(name)) {
                            match pkg_item {
                                PackageItem::Function(f) => {
                                    let entry =
                                        self.func_source_pkg.entry(module.name).or_default();
                                    entry.insert(f.name, *package);
                                    if !module.items.iter().any(|mi| matches!(mi, ModuleItem::Func(fd) if fd.name == f.name)) {
                                        module.items.push(ModuleItem::Func(f.clone()));
                                    }
                                }
                                PackageItem::Task(t) => {
                                    let entry =
                                        self.func_source_pkg.entry(module.name).or_default();
                                    entry.insert(t.name, *package);
                                    if !module.items.iter().any(|mi| matches!(mi, ModuleItem::Func(fd) if fd.name == t.name)) {
                                        module.items.push(ModuleItem::Func(FunctionDecl {
                                            name: t.name,
                                            range: None,
                                            return_type: None,
                                            ports: t.ports.clone(),
                                            decls: t.decls.clone(),
                                            stmts: t.stmts.clone(),
                                            virtual_flag: t.virtual_flag,
                                            is_static: t.is_static,
                                        }));
                                    }
                                }
                                _ => {}
                            }
                        }
                        // Resolve type parameter widths from module's param declarations and overrides
                        // moved to elaborate_module_with_params_and_type
                    }
                }
            }
            // Inject $unit function/task declarations
            for func in unit_funcs {
                if !module
                    .items
                    .iter()
                    .any(|mi| matches!(mi, ModuleItem::Func(fd) if fd.name == func.name))
                {
                    module.items.push(ModuleItem::Func(func.clone()));
                }
            }
            for task in unit_tasks {
                if !module
                    .items
                    .iter()
                    .any(|mi| matches!(mi, ModuleItem::Func(fd) if fd.name == task.name))
                {
                    module.items.push(ModuleItem::Func(FunctionDecl {
                        name: task.name,
                        range: None,
                        return_type: None,
                        ports: task.ports.clone(),
                        decls: task.decls.clone(),
                        stmts: task.stmts.clone(),
                        virtual_flag: task.virtual_flag,
                        is_static: task.is_static,
                    }));
                }
            }
        }

        if std::env::var("DBG_ELAB").is_ok() {
            eprintln!(
                "[DBG-ELAB] bind+import prepass done in {}us",
                elab_t0.elapsed().as_micros()
            );
        }
        // Inline function calls in all modules
        for module in &mut self.design.modules {
            // ── Gate inline ke cone top (--top) ──   (per-module AST rewrite
            // mahal; module di luar cone tidak di-elaborasi pula)
            if top_sym.is_some() && !reachable_pre.contains(&module.name) {
                continue;
            }
            if std::env::var("DBG_ELAB").is_ok() {
                eprintln!("[DBG-ELAB] inline module '{}'", module.name.as_str());
            }
            let temps = mivon_ast::inline::inline_func_calls_in_module(module)?;
            for (
                name,
                width,
                typedef_name,
                carried_range,
                arr_range,
                arr_size,
                temp_dtype,
                static_init,
            ) in temps
            {
                module.decls.push(Decl {
                    // Temp signal dari variabel lokal / return function bertipe
                    // typedef (struct) dipakai untuk member access — set dtype
                    // UserDefined agar elaborator mengisi struct_fields.
                    // Tipe dari inliner (mis. `Int` utk return `-> int`)
                    // diprioritaskan agar signedness temp benar — tanpa itu
                    // `$display("%0d", f())` utk nilai negatif tercetak
                    // unsigned.
                    dtype: match (typedef_name, temp_dtype) {
                        (Some(tn), _) => DataType::UserDefined(tn),
                        (None, Some(dt)) => dt,
                        (None, None) => DataType::Logic,
                    },
                    kind: DeclKind::Reg,
                    names: vec![DeclVar {
                        name,
                        range: None,
                        // Range asli `[Width-1:0]` (dari function return / decl
                        // lokal) diprioritaskan — elaborator me-resolve-nya
                        // dengan effective_params. Fallback: lebar estimasi dari
                        // inline (hanya bila range tidak tersedia).
                        expr_range: carried_range.or_else(|| {
                            if width > 1 {
                                Some(ExprRange {
                                    msb: Expr::Value(mivon_ast::expr::Value::Decimal(
                                        (width - 1) as i64,
                                    )),
                                    lsb: Expr::Value(mivon_ast::expr::Value::Decimal(0)),
                                })
                            } else {
                                None
                            }
                        }),
                        array_range: arr_range,
                        array_size_expr: arr_size,
                        extra_unpacked_dims: vec![],
                        extra_packed_dims: vec![],
                        is_dynamic: false,
                        is_queue: false,
                        is_associative: false,
                        assoc_key_type: None,
                        is_rand: false,
                        is_const: false,
                        // Variabel STATIC (fungsi `static` / lokal `static`):
                        // nilai awal dipasang di DEKLARASI (LRM 1800 §8.21)
                        // → di-init sekali saat time 0, bertahan antar-call.
                        is_static: static_init.is_some(),
                        expr: static_init,
                    }],
                });
            }
        }

        if std::env::var("DBG_ELAB").is_ok() {
            eprintln!(
                "[DBG-ELAB] inline done in {}us",
                elab_t0.elapsed().as_micros()
            );
        }
        // Expand generates in all modules (with resolved params)
        // Gate: dengan `--top`, hanya expand module di cone (reachable_pre);
        // module di luar cone TIDAK di-expand → hemat waktu+memori besar
        // (generate expansion = fase berat OpenTitan: 975 modul bertambah
        // ribuan generate instance). Setelah expansion, reachable dihitung
        // ulang (generate bisa menambah instance) — module baru yang ter-
        // reveal di-expand oleh sweep lanjutan di bawah.
        let mut expanded_set: std::collections::HashSet<Symbol> = std::collections::HashSet::new();
        for i in 0..self.design.modules.len() {
            if top_sym.is_some() && !reachable_pre.contains(&self.design.modules[i].name) {
                continue;
            }
            expanded_set.insert(self.design.modules[i].name);
            // Snapshot AST SEBELUM expand untuk modul ber-param + generate
            // (dipakai flatten saat re-elaborasi per override param).
            {
                let m = &self.design.modules[i];
                let has_gen = m
                    .items
                    .iter()
                    .any(|it| matches!(it, ModuleItem::Generate(_)));
                if has_gen && !m.params.is_empty() {
                    self.pristine_param_modules
                        .insert(m.name, m.clone());
                }
            }
            let mod_t0 = std::time::Instant::now();
            let ctx = self.collect_package_param_ctx(&self.design.modules[i]);
            let _ctx_us = mod_t0.elapsed().as_micros();
            // Konteks package untuk evaluasi PENUH default param ($bits(typedef),
            // inlining fungsi package) pada jalur resolusi localparam header.
            // `structs` = index struct package GLOBAL (base name → fields,
            // dibangun sekali di build_pkg_param_ctx) — tanpa ini, default
            // `Info = PartInfoDefault` di generate expansion phase TIDAK
            // mendapat member keys `Info.size`/`Info.integrity`, sehingga
            // generate if / port width member access di otp_ctrl_part_buf
            // gagal const-eval (error E3001/E3003 "member access not allowed").
            let pkg_full = PkgFullCtx {
                scalars: &self.pkg_const_scalars,
                arrays: &self.pkg_const_arrays,
                package_symbols: &self.package_symbols,
                structs: &self.pkg_struct_ref_index,
            };
            let resolve_params_t0 = std::time::Instant::now();
            let param_vals = resolve_param_values_with_ctx(
                &self.design.modules[i],
                &HashMap::new(),
                &ctx,
                Some(&pkg_full),
            )
            .map_err(|e| self.elab_diag_at(DiagCode::SimulationError, e, 0, 0))?;
            let resolve_params_us = resolve_params_t0.elapsed().as_micros();
            let module_name = self.design.modules[i].name;
            self.current_module = Some(module_name);
            let expand_t0 = std::time::Instant::now();
            let signed_for_gen = self.module_param_signed_set(&self.design.modules[i]);
            let gen_result = {
                let module = &mut self.design.modules[i];
                expand_all_generates(
                    module,
                    &param_vals,
                    &signed_for_gen,
                    &self.diag_sink,
                    &self.source_lines,
                    &self.source_file,
                )
            };
            let expand_us = expand_t0.elapsed().as_micros();
            if std::env::var("DBG_ELAB").is_ok() {
                eprintln!(
                    "[DBG-ELAB] '{}' ({}/{}) resolve={}us expand={}us",
                    module_name.as_str(),
                    i + 1,
                    self.design.modules.len(),
                    resolve_params_us,
                    expand_us
                );
            }
            if let Err(e) = gen_result {
                return Err(self.elab_diag_at(
                    DiagCode::ModuleNotFound,
                    format!("generate expansion failed in '{}': {}", module_name, e.msg),
                    e.line,
                    e.col,
                ));
            }
        }

        if std::env::var("DBG_ELAB").is_ok() {
            eprintln!(
                "[DBG-ELAB] generate expansion done in {}us",
                elab_t0.elapsed().as_micros()
            );
        }
        // Dead module detection: find unreachable modules via reachability from top
        let top_sym = top_module.map(Symbol::intern);
        // Set reachable dihitung sekali dan dipakai (a) warning unreachable,
        // (b) pruning elaborasi module/interface yang TIDAK reachable dari top
        // (semantik Verilator — hanya cone dari top yang dielaborasi; error di
        // module mati seperti chip wrapper / TB / DV tidak boleh memblokir
        // simulasi desain yang valid). Pruning hanya aktif bila `--top` diberikan.
        let reachable: std::collections::HashSet<Symbol> = {
            use std::collections::{HashSet, VecDeque};
            let module_map: HashMap<Symbol, &Module> =
                self.design.modules.iter().map(|m| (m.name, m)).collect();
            let all_names: HashSet<Symbol> = module_map.keys().copied().collect();
            // Satu pass scan merged source: nama module → (baris, kolom)
            // deklarasi. Dipakai untuk posisi warning "module unreachable".
            let mut module_decl_lines: HashMap<Symbol, (usize, usize)> = HashMap::new();
            for (i, line) in self.source_lines.iter().enumerate() {
                if let Some(off) = line.find("module ") {
                    // Token identifier pertama setelah `module ` — buang
                    // trailing `;`/`(`/`,` dsb (`module top;`, `module foo(`).
                    if let Some(name) = line[off + "module ".len()..]
                        .split(|c: char| !c.is_alphanumeric() && c != '_')
                        .next()
                        .filter(|s| !s.is_empty())
                    {
                        let col = off + "module ".len() + 1;
                        module_decl_lines
                            .entry(Symbol::intern(name))
                            .or_insert((i + 1, col));
                    }
                }
            }
            let mut reachable: HashSet<Symbol> = HashSet::new();
            let mut queue: VecDeque<Symbol> = VecDeque::new();
            if let Some(ref top) = top_sym {
                if all_names.contains(top) {
                    queue.push_back(*top);
                    reachable.insert(*top);
                }
            } else if let Some(cand) = {
                // Auto-top: module yang TIDAK diinstansiasi module lain
                // (candidate top pertama — konsisten dgn pemilihan top di
                // bagian bawah elaborate()). Reachability dihitung dari
                // kandidat ini agar error di module mati (TB/DV terpisah,
                // modul dengan dependensi hilang) tidak memblokir simulasi
                // cone yang valid, sama seperti perilaku `--top`.
                let mut instantiated: HashSet<Symbol> = HashSet::new();
                for m in &self.design.modules {
                    let mut insts = Vec::new();
                    collect_instance_names(&m.items, &mut insts);
                    for mn in insts {
                        instantiated.insert(mn);
                    }
                }
                self.design
                    .modules
                    .iter()
                    .find(|m| !instantiated.contains(&m.name))
            } {
                queue.push_back(cand.name);
                reachable.insert(cand.name);
            } else if let Some(first) = self.design.modules.first() {
                queue.push_back(first.name);
                reachable.insert(first.name);
            }
            while let Some(name) = queue.pop_front() {
                if let Some(module) = module_map.get(&name) {
                    let mut insts = Vec::new();
                    collect_instance_names(&module.items, &mut insts);
                    for mn in insts {
                        if all_names.contains(&mn) && !reachable.contains(&mn) {
                            reachable.insert(mn);
                            queue.push_back(mn);
                        }
                    }
                }
            }
            for m in &self.design.modules {
                if !reachable.contains(&m.name)
                    && top_sym.as_ref().map(|t| *t != m.name).unwrap_or(true)
                {
                    let mut diag = Diagnostic::new(
                        DiagLevel::Warning,
                        DiagCode::UnusedSignal,
                        format!(
                            "module '{}' is unreachable (not instantiated from top)",
                            m.name
                        ),
                    )
                    .with_explanation(
                        "The module is defined in the source files but never \
                         instantiated from the top module, so it is not part \
                         of the elaborated design and will not be simulated.",
                    )
                    .with_suggestion(
                        "Remove the module from the source list, or instantiate \
                         it from the top module if it should be part of the \
                         simulation.",
                    );
                    // Posisi deklarasi module (bila ditemukan di merged source)
                    // — tanpa ini warning hanya menampilkan nama module tanpa
                    // file:line:col.
                    if let Some(&(ml, mc)) = module_decl_lines.get(&m.name) {
                        if ml > 0 && ml <= self.source_lines.len() {
                            let (file, dl) = self.resolve_source_location(ml);
                            let snippet =
                                SourceSnippet::new(file, dl, mc, &self.source_lines[ml - 1]);
                            diag = diag.with_source_snippet(snippet);
                        }
                    }
                    self.diag_sink.push(diag);
                }
            }
            reachable
        };
        self.reachable = reachable.clone();

        // ── Sweep: expand module yang baru ter-reveal oleh generate ──
        // Generate di module cone bisa menginstansiasi modul yang TIDAK
        // direferensikan pre-expansion → modul itu reachable (post) tapi
        // belum di-expand. Expand mereka + hitung ulang reachable (bounded).
        if top_sym.is_some() {
            for _sweep in 0..4 {
                let newly: Vec<usize> = self
                    .design
                    .modules
                    .iter()
                    .enumerate()
                    .filter(|(_, m)| reachable.contains(&m.name) && !expanded_set.contains(&m.name))
                    .map(|(i, _)| i)
                    .collect();
                if newly.is_empty() {
                    break;
                }
                for &i in &newly {
                    let ctx = self.collect_package_param_ctx(&self.design.modules[i]);
                    let pkg_full = PkgFullCtx {
                        scalars: &self.pkg_const_scalars,
                        arrays: &self.pkg_const_arrays,
                        package_symbols: &self.package_symbols,
                        structs: &self.pkg_struct_ref_index,
                    };
                    if let Ok(param_vals) = resolve_param_values_with_ctx(
                        &self.design.modules[i],
                        &HashMap::new(),
                        &ctx,
                        Some(&pkg_full),
                    ) {
                        let signed_for_gen2 = self.module_param_signed_set(&self.design.modules[i]);
                        let module = &mut self.design.modules[i];
                        let _ = expand_all_generates(
                            module,
                            &param_vals,
                            &signed_for_gen2,
                            &self.diag_sink,
                            &self.source_lines,
                            &self.source_file,
                        );
                        expanded_set.insert(module.name);
                    }
                }
                let reachable2: std::collections::HashSet<Symbol> =
                    self.compute_reachable_set(top_sym, false);
                if reachable2.len() == reachable.len() {
                    break;
                }
            }
            // ── DROP module di luar cone dari AST ──
            // Main-elaborate hanya memproses cone (prune_unreachable), tapi
            // design.modules (AST penuh ~1.5GB utk OpenTitan DV+multi-top)
            // tetap di RAM selama elaborasi+sim → OOM. Buang module non-cone
            // SEKARANG (setelah generate expansion + reachability final):
            // module di luar cone tidak diperlukan lagi (topo/checksum/module_idx
            // dibangun dari design.modules setelah titik ini).
            let reachable = self.compute_reachable_set(top_sym, false);
            let before = self.design.modules.len();
            self.design.modules.retain(|m| reachable.contains(&m.name));
            if std::env::var("DBG_ELAB").is_ok() {
                eprintln!(
                    "[DBG-ELAB] dropped {} non-reachable module AST(s) (cone={} before={})",
                    before - self.design.modules.len(),
                    self.design.modules.len(),
                    before
                );
            }
        }

        // ── Index module name → design.modules (SEKALI, bukan per-module) ──
        // design.modules tidak berubah lagi setelah titik ini (bind/import/
        // inline/generate expansion sudah selesai) — bangun peta name→index
        // sekali dan reuse di elaborate_module_with_params_and_type untuk
        // resolve instance → target module. Sebelumnya dibangun ulang di
        // dalam pemanggilan per-module → O(n²): 4002 module × 4002 insert =
        // 16M hash insert (62% waktu elaborate di desain besar).
        self.module_idx = self
            .design
            .modules
            .iter()
            .enumerate()
            .map(|(i, m)| (m.name, i))
            .collect();

        if std::env::var("DBG_ELAB").is_ok() {
            eprintln!(
                "[DBG-ELAB] dead-module detection done in {}us (reachable={})",
                elab_t0.elapsed().as_micros(),
                reachable.len()
            );
        }
        // ── Incremental elaboration pass ──
        // 1. Compute structural checksums for all modules
        // 2. Compute topological order (children before parents)
        // 3. Compute dependency-aware signatures
        // 4. Check cache before each elaborate_module()

        let module_names: Vec<Symbol> = self.design.modules.iter().map(|m| m.name).collect();

        // Phase A: Compute structural checksums from design.modules directly (no clone needed)
        let mut struct_sigs: HashMap<Symbol, u64> = HashMap::new();
        for module in &self.design.modules {
            let sig = self.compute_module_checksum(module);
            struct_sigs.insert(module.name, sig);
        }

        // Phase B: Compute topological order from design.modules directly
        let topo_order = self.compute_topo_order(&self.design.modules, &module_names);

        // Phase C: Clone modules for elaboration (avoids borrow conflict with &mut self)
        let modules_snapshot: Vec<Module> = self.design.modules.clone();
        let snapshot_map: HashMap<Symbol, &Module> =
            modules_snapshot.iter().map(|m| (m.name, m)).collect();

        let mut dep_sigs: HashMap<Symbol, u64> = HashMap::new();

        if std::env::var("DBG_ELAB").is_ok() {
            eprintln!(
                "[DBG-ELAB] checksums+topo done in {}us (topo_len={})",
                elab_t0.elapsed().as_micros(),
                topo_order.len()
            );
        }
        // Bila `--top` diberikan: lewati elaborasi module yang tidak reachable
        // dari top (sudah diperingatkan sebagai unreachable di atas). Error di
        // module mati tidak memblokir simulasi cone yang valid.
        let prune_unreachable = top_module.is_some();
        for &mod_name in &topo_order {
            if prune_unreachable && !reachable.contains(&mod_name) {
                continue;
            }
            let module = snapshot_map.get(&mod_name).ok_or_else(|| {
                self.elab_diag(
                    DiagCode::ModuleNotFound,
                    format!("module '{}' not found in snapshot", mod_name),
                )
            })?;

            let structural = struct_sigs.get(&mod_name).copied().unwrap_or(0);

            // Combine with dependency (child) signatures
            let mut dep_aware = structural;
            for item in &module.items {
                if let ModuleItem::Instance(inst) = item {
                    if let Some(child_sig) = dep_sigs.get(&inst.module_name) {
                        dep_aware = mivon_core::checksum::combine_checksum(dep_aware, *child_sig);
                    }
                }
            }
            dep_sigs.insert(mod_name, dep_aware);

            // Check cache: if dependency-aware signature matches, skip elaboration
            if let Some(cached_ir) = self.module_cache.get(&dep_aware) {
                self.modules.insert(mod_name, cached_ir.clone());
                self.cache_hits += 1;
            } else {
                let mod_t0 = std::time::Instant::now();
                if std::env::var("DBG_ELAB").is_ok() {
                    eprintln!(
                        "[DBG-ELAB] >> elaborate module '{}' ({}/{})",
                        mod_name.as_str(),
                        self.cache_hits + self.cache_misses + 1,
                        topo_order.len()
                    );
                }
                match self.elaborate_module(module, &module_names) {
                    Ok(ir) => {
                        if std::env::var("DBG_ELAB").is_ok()
                            && mod_t0.elapsed().as_micros() > 100_000
                        {
                            eprintln!(
                                "[DBG-ELAB]   module '{}' elaborated in {}us",
                                mod_name.as_str(),
                                mod_t0.elapsed().as_micros()
                            );
                        }
                        self.module_cache.insert(dep_aware, ir.clone());
                        self.modules.insert(mod_name, ir);
                        self.cache_misses += 1;
                    }
                    Err(e) => {
                        // Module yang gagal elaborasi dilewati (tidak dimatikan
                        // seluruh elaborasi — desain parsial tetap bisa di-sim),
                        // TAPI dilaporkan sebagai ERROR (bukan warning): module
                        // yang di-skip tidak ikut disimulasikan, jadi ini bukan
                        // sekadar peringatan kosmetik. Teruskan Diagnostic ASLI
                        // dari error (sudah membawa source snippet
                        // file:line:col) sehingga posisi error tidak hilang.
                        let mut diag = e.to_diagnostic();
                        diag.level = DiagLevel::Error;
                        diag.code = DiagCode::ModuleNotFound;
                        diag.message = format!(
                            "module '{}' skipped due to elaboration error: {}",
                            mod_name, diag.message
                        )
                        .into();
                        // F38: module TIDAK reachable dari top (TB/DV terpisah,
                        // dependensi hilang) — downgrade ke warning agar error
                        // di module mati tidak memblokir cone RTL yang valid
                        // (semantik Verilator; konsisten dgn pruning `--top`).
                        if !reachable.contains(&mod_name) && !prune_unreachable {
                            diag.level = DiagLevel::Warning;
                            diag.code = DiagCode::UnusedSignal;
                            diag.message = format!(
                                "module '{}' is unreachable from top — elaboration issue treated as warning: {}",
                                mod_name, e
                            )
                            .into();
                        }
                        self.diag_sink.push(diag);
                    }
                }
            }
        }

        if std::env::var("DBG_ELAB").is_ok() {
            eprintln!(
                "[DBG-ELAB] module elaboration loop done in {}us (hits={} misses={})",
                elab_t0.elapsed().as_micros(),
                self.cache_hits,
                self.cache_misses
            );
        }

        // Elaborate interfaces as modules (ports + decls + processes), so
        // interface initial/always/assign blocks actually run inside the
        // flattened hierarchy. Falls back to a signal-only module if the
        // interface body references unsupported constructs.
        let interfaces_snapshot: Vec<Interface> = self.design.interfaces.clone();
        // Interface hanya dielaborasi bila direferensikan oleh module reachable
        // (instansiasi / tipe port / decl bertipe interface). Interface mati dari
        // DV/TB di-skip — error-nya tidak memblokir cone RTL yang valid.
        let referenced_ifaces: std::collections::HashSet<Symbol> = {
            let mut s = std::collections::HashSet::new();
            for m in &self.design.modules {
                if prune_unreachable && !reachable.contains(&m.name) {
                    continue;
                }
                let mut insts = Vec::new();
                collect_instance_names(&m.items, &mut insts);
                for mn in insts {
                    s.insert(mn);
                }
                for port in &m.ports {
                    if let Some(name) = &port.dtype_name {
                        s.insert(*name);
                    }
                }
                for d in &m.decls {
                    if let DataType::UserDefined(name) = &d.dtype {
                        s.insert(*name);
                    }
                }
            }
            s
        };
        for iface in &interfaces_snapshot {
            if prune_unreachable && !referenced_ifaces.contains(&iface.name) {
                continue;
            }
            let synthetic = Module {
                name: iface.name,
                ports: iface.ports.clone(),
                params: iface.params.clone(),
                decls: iface.decls.clone(),
                items: iface.items.clone(),
                // F84: interface → Module sintetik harus membawa satuannya.
                timescale: iface.timescale.clone(),
            };
            // Basis globalkomputasi ulang per-design (F84) dalam
            // `compute_module_delay_scales`; synthetic interface tak ada di
            // `design.modules`, jadi skalanya perlu dihitung di sini juga —
            // tanpa ini `current_delay_scale()` jatuh ke 1 dan delay interface
            // `1us` diperlakukan sebagai `1ns`.
            let iface_scale = self.delay_scale_for_unit(iface.timescale.as_ref());
            self.module_delay_scale.insert(iface.name, iface_scale);
            match self.elaborate_module(&synthetic, &module_names) {
                Ok(ir) => {
                    self.modules.insert(iface.name, ir);
                }
                Err(e) => {
                    let mut diag = e.to_diagnostic();
                    diag.level = DiagLevel::Error;
                    diag.code = DiagCode::ModuleNotFound;
                    diag.message = format!(
                        "interface '{}' skipped due to elaboration error: {}",
                        iface.name, diag.message
                    )
                    .into();
                    // F38: interface TIDAK dipakai oleh cone reachable —
                    // downgrade ke warning (sama seperti module unreachable).
                    if !reachable.contains(&iface.name) && !prune_unreachable {
                        diag.level = DiagLevel::Warning;
                        diag.code = DiagCode::UnusedSignal;
                        diag.message = format!(
                            "interface '{}' is unreachable from top — elaboration issue treated as warning: {}",
                            iface.name, e
                        )
                        .into();
                    }
                    self.diag_sink.push(diag);
                    self.modules.insert(
                        iface.name,
                        IrModule {
                            name: iface.name,
                            signals: Vec::new(),
                            inputs: vec![],
                            outputs: vec![],
                            inouts: vec![],
                            processes: vec![],
                            sub_instances: vec![],
                        },
                    );
                }
            }
        }

        if std::env::var("DBG_ELAB").is_ok() {
            eprintln!(
                "[DBG-ELAB] interfaces done in {}us",
                elab_t0.elapsed().as_micros()
            );
        }
        // Find top module
        let inst_graph = build_inst_graph(&self.design);
        let mut instantiated_modules = std::collections::HashSet::new();
        for deps in inst_graph.values() {
            for d in deps {
                instantiated_modules.insert(*d);
            }
        }
        let candidate_tops: Vec<Symbol> = self
            .design
            .modules
            .iter()
            .map(|m| m.name)
            .filter(|name| !instantiated_modules.contains(name))
            .collect();

        let top_name = match top_module {
            Some(name) => {
                let sym = Symbol::intern(name);
                if !self.design.modules.iter().any(|m| m.name == sym) {
                    if mode == ElaborateMode::StrictSimulation {
                        return Err(self.elab_diag(
                            DiagCode::TopResolutionFailed,
                            format!(
                                "Unable to determine top-level design.\n\
                                 Simulation cancelled.\n\n\
                                 Reason:\n\
                                 • missing root module (requested top module '{}' not found in design)",
                                name
                            )
                        ));
                    }
                    self.recovered = true;
                }
                sym
            }
            None => {
                if candidate_tops.is_empty() {
                    if self.design.modules.is_empty() {
                        if mode == ElaborateMode::StrictSimulation {
                            return Err(self.elab_diag(
                                DiagCode::TopResolutionFailed,
                                "Unable to determine top-level design.\n\
                                 Simulation cancelled.\n\n\
                                 Reason:\n\
                                 • missing root module (no modules found in design)"
                                    .to_string(),
                            ));
                        }
                        self.recovered = true;
                        self.design
                            .modules
                            .first()
                            .map(|m| m.name)
                            .unwrap_or(Symbol::EMPTY)
                    } else {
                        if mode == ElaborateMode::StrictSimulation {
                            return Err(self.elab_diag(
                                DiagCode::TopResolutionFailed,
                                "Unable to determine top-level design.\n\
                                 Simulation cancelled.\n\n\
                                 Reason:\n\
                                 • circular hierarchy (all modules are instantiated by others)"
                                    .to_string(),
                            ));
                        }
                        self.recovered = true;
                        self.design
                            .modules
                            .first()
                            .map(|m| m.name)
                            .unwrap_or(Symbol::EMPTY)
                    }
                } else if candidate_tops.len() > 1 {
                    // Auto top resolution: pilih kandidat dengan skor unik
                    // tertinggi (cone transitif + sinyal nama). Design besar
                    // (filelist OpenTitan: chip SoC + ratusan testbench/bind)
                    // biasanya punya banyak candidate — heuristik ini memilih
                    // top yang PALING lengkap secara deterministik. Hanya bila
                    // ada pemenang unik; seri → perilaku lama (error/recovery).
                    let mut scored: Vec<(i64, Symbol)> = candidate_tops
                        .iter()
                        .map(|c| (score_auto_top(*c, module_cone_size(*c, &inst_graph)), *c))
                        .collect();
                    scored
                        .sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.as_str().cmp(b.1.as_str())));
                    let unique_winner = scored.len() >= 2 && scored[0].0 > scored[1].0;
                    if unique_winner {
                        scored[0].1
                    } else if mode == ElaborateMode::StrictSimulation {
                        let mut reason = "Unable to determine top-level design.\n\
                                          Simulation cancelled.\n\n\
                                          Reason:\n\
                                          • multiple candidate tops (auto-resolution tie):"
                            .to_string();
                        for cand in &candidate_tops {
                            reason.push_str(&format!("\n   - {}", cand.as_str()));
                        }
                        return Err(self.elab_diag(DiagCode::MultipleCandidateTops, reason));
                    } else {
                        self.recovered = true;
                        scored[0].1
                    }
                } else {
                    candidate_tops[0]
                }
            }
        };

        let mut top = match self.modules.remove(&top_name) {
            Some(m) => m,
            None => {
                if mode == ElaborateMode::StrictSimulation {
                    return Err(self.elab_diag(
                        DiagCode::TopResolutionFailed,
                        format!(
                            "Unable to determine top-level design.\n\
                             Simulation cancelled.\n\n\
                             Reason:\n\
                             • unresolved instantiation or compilation error in top module '{}'",
                            top_name.as_str()
                        ),
                    ));
                }

                // Fallback only for AnalysisRecovery (Rule 2)
                self.recovered = true;
                let _total_modules = self.design.modules.len();
                let _success_modules = self.modules.len();

                // Explicit Recovery Mode warning (Rule 4)
                eprintln!(
                    "\nWARNING\n\n\
                     Top-level design not found.\n\n\
                     Recovered module:\n\n\
                         {}\n\n\
                     Recovery mode enabled.\n\n\
                     Simulation disabled.\n",
                    top_name.as_str()
                );

                let fallback = self
                    .design
                    .modules
                    .iter()
                    .filter_map(|m| self.modules.get(&m.name).cloned())
                    .next();
                match fallback {
                    Some(fb) => fb,
                    _ => {
                        return Err(self.elab_diag(
                            DiagCode::TopResolutionFailed,
                            format!(
                                "Unable to determine top-level design.\n\
                                 Simulation cancelled.\n\n\
                                 Reason:\n\
                                 • unresolved instantiation or compilation error in top module '{}'",
                                top_name.as_str()
                            )
                        ))
                    }
                }
            }
        };

        if std::env::var("DBG_ELAB").is_ok() {
            eprintln!(
                "[DBG-ELAB] top found in {}us",
                elab_t0.elapsed().as_micros()
            );
        }
        // Flatten instances: merge child module processes into the top module
        let hier_signal_map = self.flatten_instances(&mut top)?;

        // Post-flatten sensitivity fixup: inject HierRef signals into
        // combinational process sensitivity lists. `collect_sensitivity` (AST-level)
        // can't resolve hierarchical names like `uut.x` that only exist in
        // `hier_signal_map` after flatten.
        fix_hier_sensitivity(&mut top.processes, &hier_signal_map);

        // Merge specialized parameterized classes into design classes before elaboration
        {
            let mut specialized = self.specialized_classes.borrow_mut();
            for spec in specialized.drain(..) {
                if !self.design.classes.iter().any(|c| c.name == spec.name) {
                    self.design.classes.push(spec);
                }
            }
        }
        // Add package classes to design.classes so they're available for hierarchy walk
        for items in self.package_symbols.values() {
            for item in items.values() {
                if let PackageItem::Class(c) = item {
                    if !self
                        .design
                        .classes
                        .iter()
                        .any(|existing| existing.name == c.name)
                    {
                        self.design.classes.push(c.clone());
                    }
                }
            }
        }

        let mut classes = self.elaborate_classes()?;

        // Inject built-in __uvm_object and __uvm_component classes
        if !classes.contains_key(&Symbol::intern("__uvm_object")) {
            for cls in classes.values_mut() {
                let extends_str = cls.extends.map(|s| s.as_str());
                match extends_str {
                    Some("uvm_object") => cls.extends = Some(Symbol::intern("__uvm_object")),
                    Some("uvm_transaction") => {
                        // VERIF-17: uvm_transaction — kelas dasar uvm_sequence_item.
                        cls.extends = Some(Symbol::intern("__uvm_transaction"))
                    }
                    Some("uvm_component") => cls.extends = Some(Symbol::intern("__uvm_component")),
                    Some("uvm_sequence_item") => {
                        cls.extends = Some(Symbol::intern("__uvm_sequence_item"))
                    }
                    Some("uvm_sequence") => cls.extends = Some(Symbol::intern("__uvm_sequence")),
                    Some("uvm_sequencer") => cls.extends = Some(Symbol::intern("__uvm_sequencer")),
                    Some("uvm_driver") => cls.extends = Some(Symbol::intern("__uvm_driver")),
                    Some("uvm_monitor") => cls.extends = Some(Symbol::intern("__uvm_monitor")),
                    Some("uvm_env") => cls.extends = Some(Symbol::intern("__uvm_env")),
                    Some("uvm_agent") => cls.extends = Some(Symbol::intern("__uvm_agent")),
                    Some("uvm_scoreboard") => {
                        cls.extends = Some(Symbol::intern("__uvm_scoreboard"))
                    }
                    Some("uvm_analysis_port") => {
                        cls.extends = Some(Symbol::intern("__uvm_analysis_port"))
                    }
                    Some("uvm_analysis_imp") => {
                        cls.extends = Some(Symbol::intern("__uvm_analysis_imp"))
                    }
                    Some("uvm_analysis_export") => {
                        cls.extends = Some(Symbol::intern("__uvm_analysis_export"))
                    }
                    Some("uvm_subscriber") => {
                        cls.extends = Some(Symbol::intern("__uvm_subscriber"))
                    }
                    // VERIF-13: uvm_comparator / uvm_in_order_comparator.
                    Some("uvm_comparator") | Some("uvm_in_order_comparator") => {
                        cls.extends = Some(Symbol::intern("__uvm_comparator"))
                    }
                    // VERIF-15: uvm_heartbeat.
                    Some("uvm_heartbeat") => cls.extends = Some(Symbol::intern("__uvm_heartbeat")),
                    Some("uvm_tlm_fifo") => cls.extends = Some(Symbol::intern("__uvm_tlm_fifo")),
                    Some("uvm_seq_item_port") => {
                        cls.extends = Some(Symbol::intern("__uvm_seq_item_port"))
                    }
                    Some("uvm_test") => cls.extends = Some(Symbol::intern("__uvm_test")),
                    Some("uvm_config_db") => cls.extends = Some(Symbol::intern("__uvm_config_db")),
                    Some("uvm_event") => cls.extends = Some(Symbol::intern("__uvm_event")),
                    Some("uvm_barrier") => cls.extends = Some(Symbol::intern("__uvm_barrier")),
                    Some("uvm_report_object") => {
                        cls.extends = Some(Symbol::intern("__uvm_report_object"))
                    }
                    // OpenTitan DV: `dv_report_server extends uvm_default_report_server`
                    // (`uvm_report_server` di UVM asli extends uvm_report_object →
                    // uvm_object). Tanpa remap ini extends tak dikenal → super.new
                    // gagal (RT8001) saat sim.
                    Some("uvm_report_server") | Some("uvm_default_report_server") => {
                        cls.extends = Some(Symbol::intern("__uvm_report_object"))
                    }
                    Some("uvm_factory") => cls.extends = Some(Symbol::intern("__uvm_factory")),
                    Some("uvm_resource_db") => {
                        cls.extends = Some(Symbol::intern("__uvm_resource_db"))
                    }
                    _ => {}
                }
            }
            classes.insert(
                Symbol::intern("__uvm_object"),
                IrClassDef {
                    name: Symbol::intern("__uvm_object"),
                    extends: None,
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![],
                    constraints: vec![],
                    rand_fields: vec![],
                    lets: vec![],
                },
            );
            classes.insert(
                Symbol::intern("__uvm_report_object"),
                IrClassDef {
                    name: Symbol::intern("__uvm_report_object"),
                    extends: Some(Symbol::intern("__uvm_object")),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![],
                    constraints: vec![],
                    rand_fields: vec![],
                    lets: vec![],
                },
            );
            classes.insert(
                Symbol::intern("__uvm_component"),
                IrClassDef {
                    name: Symbol::intern("__uvm_component"),
                    extends: Some(Symbol::intern("__uvm_report_object")),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![],
                    constraints: vec![],
                    rand_fields: vec![],
                    lets: vec![],
                },
            );
            classes.insert(
                Symbol::intern("__uvm_transaction"),
                IrClassDef {
                    name: Symbol::intern("__uvm_transaction"),
                    // VERIF-17: uvm_transaction extends uvm_object (per UVM 1.2).
                    extends: Some(Symbol::intern("__uvm_object")),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![],
                    constraints: vec![],
                    rand_fields: vec![],
                    lets: vec![],
                },
            );
            classes.insert(
                Symbol::intern("__uvm_sequence_item"),
                IrClassDef {
                    name: Symbol::intern("__uvm_sequence_item"),
                    // VERIF-17: uvm_sequence_item extends uvm_transaction (UVM 1.2).
                    extends: Some(Symbol::intern("__uvm_transaction")),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![],
                    constraints: vec![],
                    rand_fields: vec![],
                    lets: vec![],
                },
            );
            classes.insert(
                Symbol::intern("__uvm_sequence"),
                IrClassDef {
                    name: Symbol::intern("__uvm_sequence"),
                    extends: Some(Symbol::intern("__uvm_sequence_item")),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![],
                    constraints: vec![],
                    rand_fields: vec![],
                    lets: vec![],
                },
            );
            classes.insert(
                Symbol::intern("__uvm_sequencer"),
                IrClassDef {
                    name: Symbol::intern("__uvm_sequencer"),
                    extends: Some(Symbol::intern("__uvm_component")),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![],
                    constraints: vec![],
                    rand_fields: vec![],
                    lets: vec![],
                },
            );
            classes.insert(
                Symbol::intern("__uvm_driver"),
                IrClassDef {
                    name: Symbol::intern("__uvm_driver"),
                    extends: Some(Symbol::intern("__uvm_component")),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![],
                    constraints: vec![],
                    rand_fields: vec![],
                    lets: vec![],
                },
            );
            classes.insert(
                Symbol::intern("__uvm_monitor"),
                IrClassDef {
                    name: Symbol::intern("__uvm_monitor"),
                    extends: Some(Symbol::intern("__uvm_component")),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![],
                    constraints: vec![],
                    rand_fields: vec![],
                    lets: vec![],
                },
            );
            classes.insert(
                Symbol::intern("__uvm_scoreboard"),
                IrClassDef {
                    name: Symbol::intern("__uvm_scoreboard"),
                    extends: Some(Symbol::intern("__uvm_component")),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![],
                    constraints: vec![],
                    rand_fields: vec![],
                    lets: vec![],
                },
            );
            classes.insert(
                Symbol::intern("__uvm_analysis_port"),
                IrClassDef {
                    name: Symbol::intern("__uvm_analysis_port"),
                    extends: Some(Symbol::intern("__uvm_object")),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![],
                    constraints: vec![],
                    rand_fields: vec![],
                    lets: vec![],
                },
            );
            classes.insert(
                Symbol::intern("__uvm_analysis_imp"),
                IrClassDef {
                    name: Symbol::intern("__uvm_analysis_imp"),
                    extends: Some(Symbol::intern("__uvm_object")),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![],
                    constraints: vec![],
                    rand_fields: vec![],
                    lets: vec![],
                },
            );
            // F23: uvm_analysis_export — passthrough port (connect + write
            // broadcast, data sama dengan analysis port). extends analysis port
            // sehingga is_uvm_analysis_port_hierarchy otomatis benar.
            classes.insert(
                Symbol::intern("__uvm_analysis_export"),
                IrClassDef {
                    name: Symbol::intern("__uvm_analysis_export"),
                    extends: Some(Symbol::intern("__uvm_analysis_port")),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![],
                    constraints: vec![],
                    rand_fields: vec![],
                    lets: vec![],
                },
            );
            // F23: uvm_tlm_fifo — FIFO TLM blocking put/get/peek + export
            // analysis internal (__uvm_fifo_export) utk `fifo.analysis_export`.
            classes.insert(
                Symbol::intern("__uvm_tlm_fifo"),
                IrClassDef {
                    name: Symbol::intern("__uvm_tlm_fifo"),
                    extends: Some(Symbol::intern("__uvm_component")),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![],
                    constraints: vec![],
                    rand_fields: vec![],
                    lets: vec![],
                },
            );
            classes.insert(
                Symbol::intern("__uvm_fifo_export"),
                IrClassDef {
                    name: Symbol::intern("__uvm_fifo_export"),
                    extends: Some(Symbol::intern("__uvm_analysis_export")),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![],
                    constraints: vec![],
                    rand_fields: vec![],
                    lets: vec![],
                },
            );
            // F24: uvm_seq_item_port — port driver↔sequencer. Method
            // get_next_item/item_done/try_next_item mendelegasi ke sequencer
            // yang di-connect (`connect(seqr)`). Blocking get_next_item
            // di-intercept block.rs (waiter keyed by sequencer).
            classes.insert(
                Symbol::intern("__uvm_seq_item_port"),
                IrClassDef {
                    name: Symbol::intern("__uvm_seq_item_port"),
                    extends: Some(Symbol::intern("__uvm_component")),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![],
                    constraints: vec![],
                    rand_fields: vec![],
                    lets: vec![],
                },
            );
            // F22: uvm_subscriber — komponen penerima broadcast analysis port.
            // `new` di-dispatch builtin (auto-buat analysis_imp child + set
            // field `analysis_imp`); `write` user override jalan normal
            // (pattern override-check di method.rs); tanpa override → no-op.
            classes.insert(
                Symbol::intern("__uvm_heartbeat"),
                IrClassDef {
                    name: Symbol::intern("__uvm_heartbeat"),
                    extends: Some(Symbol::intern("__uvm_component")),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![],
                    constraints: vec![],
                    rand_fields: vec![],
                    lets: vec![],
                },
            );
            classes.insert(
                Symbol::intern("__uvm_comparator"),
                IrClassDef {
                    name: Symbol::intern("__uvm_comparator"),
                    extends: Some(Symbol::intern("__uvm_component")),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![],
                    constraints: vec![],
                    rand_fields: vec![],
                    lets: vec![],
                },
            );
            classes.insert(
                Symbol::intern("__uvm_subscriber"),
                IrClassDef {
                    name: Symbol::intern("__uvm_subscriber"),
                    extends: Some(Symbol::intern("__uvm_component")),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![],
                    constraints: vec![],
                    rand_fields: vec![],
                    lets: vec![],
                },
            );
            classes.insert(
                Symbol::intern("__uvm_env"),
                IrClassDef {
                    name: Symbol::intern("__uvm_env"),
                    extends: Some(Symbol::intern("__uvm_component")),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![],
                    constraints: vec![],
                    rand_fields: vec![],
                    lets: vec![],
                },
            );
            classes.insert(
                Symbol::intern("__uvm_agent"),
                IrClassDef {
                    name: Symbol::intern("__uvm_agent"),
                    extends: Some(Symbol::intern("__uvm_component")),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![],
                    constraints: vec![],
                    rand_fields: vec![],
                    lets: vec![],
                },
            );
            classes.insert(
                Symbol::intern("__uvm_test"),
                IrClassDef {
                    name: Symbol::intern("__uvm_test"),
                    extends: Some(Symbol::intern("__uvm_component")),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![],
                    constraints: vec![],
                    rand_fields: vec![],
                    lets: vec![],
                },
            );
            classes.insert(
                Symbol::intern("__uvm_config_db"),
                IrClassDef {
                    name: Symbol::intern("__uvm_config_db"),
                    extends: Some(Symbol::intern("__uvm_object")),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![],
                    constraints: vec![],
                    rand_fields: vec![],
                    lets: vec![],
                },
            );
            classes.insert(
                Symbol::intern("__uvm_resource_db"),
                IrClassDef {
                    name: Symbol::intern("__uvm_resource_db"),
                    extends: Some(Symbol::intern("__uvm_object")),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![],
                    constraints: vec![],
                    rand_fields: vec![],
                    lets: vec![],
                },
            );
            // F21: uvm_event — sinkronisasi trigger/wait antar komponen.
            // Method di-dispatch di builtin.rs execute_uvm_event_method;
            // blocking wait_trigger/wait_on di-block di block.rs (jalur AST).
            classes.insert(
                Symbol::intern("__uvm_event"),
                IrClassDef {
                    name: Symbol::intern("__uvm_event"),
                    extends: Some(Symbol::intern("__uvm_object")),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![],
                    constraints: vec![],
                    rand_fields: vec![],
                    lets: vec![],
                },
            );
            // F21: uvm_barrier — sinkronisasi N-proses (threshold).
            classes.insert(
                Symbol::intern("__uvm_barrier"),
                IrClassDef {
                    name: Symbol::intern("__uvm_barrier"),
                    extends: Some(Symbol::intern("__uvm_object")),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![],
                    constraints: vec![],
                    rand_fields: vec![],
                    lets: vec![],
                },
            );
            classes.insert(
                Symbol::intern("__uvm_factory"),
                IrClassDef {
                    name: Symbol::intern("__uvm_factory"),
                    extends: Some(Symbol::intern("__uvm_object")),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![],
                    constraints: vec![],
                    rand_fields: vec![],
                    lets: vec![],
                },
            );
        }

        self.detect_multi_driver_signals(&mut top)?;

        let top_signal_map: HashMap<Symbol, SignalId> = top
            .signals
            .iter()
            .enumerate()
            .map(|(i, s)| (s.name, i))
            .collect();
        // LANG-08: net alias (IEEE 1800-2017 §10.9) — `alias a = b = c;`
        // menyatukan semua net dalam rantai jadi satu jaringan: resolve nama
        // net → SignalId, union-find, pilih canonical (id terkecil), simpan
        // peta member → canonical di IrDesign. Engine mer-direct read/write
        // member ke canonical (state.alias_redirect) sehingga menulis ke salah
        // satu terlihat di semua (short).
        let mut net_aliases: HashMap<SignalId, SignalId> = HashMap::new();
        if let Some(top_ast) = self.design.modules.iter().find(|m| m.name == top_name) {
            let mut parent: HashMap<SignalId, SignalId> = HashMap::new();
            for item in &top_ast.items {
                if let ModuleItem::NetAlias(pairs) = item {
                    for (lhs, rhs) in pairs {
                        let l_id = net_expr_signal_id(lhs, &top_signal_map);
                        let r_id = net_expr_signal_id(rhs, &top_signal_map);
                        if let (Some(a), Some(b)) = (l_id, r_id) {
                            // union-find (simple path-compression)
                            let root = |x: SignalId, p: &mut HashMap<SignalId, SignalId>| {
                                let mut cur = x;
                                while let Some(&n) = p.get(&cur) {
                                    cur = n;
                                }
                                let mut back = x;
                                while let Some(&n) = p.get(&back) {
                                    p.insert(back, cur);
                                    back = n;
                                }
                                cur
                            };
                            let ra = root(a, &mut parent);
                            let rb = root(b, &mut parent);
                            if ra != rb {
                                parent.insert(ra, rb); // ra → rb
                            }
                        }
                    }
                }
            }
            for member in top_signal_map.values().copied() {
                let mut cur = member;
                let mut hops = 0;
                while let Some(&n) = parent.get(&cur) {
                    cur = n;
                    hops += 1;
                    if hops > 64 {
                        break;
                    }
                }
                if cur != member {
                    net_aliases.insert(member, cur);
                }
            }
        }
        let covergroups =
            self.elaborate_covergroups(top_name.as_str(), &top_signal_map, &top.signals)?;
        let dpi_imports = self.elaborate_dpi_imports()?;

        let mut specify_items = Vec::new();
        for module in &self.design.modules {
            for item in &module.items {
                if let ModuleItem::Specify(sb) = item {
                    specify_items.extend(sb.items.clone());
                }
            }
        }

        // Collect recursive function declarations from module items for runtime evaluation
        let mut module_functions: HashMap<Symbol, mivon_ast::types::FunctionDecl> = HashMap::new();
        for module in &self.design.modules {
            for item in &module.items {
                if let ModuleItem::Func(f) = item {
                    module_functions.insert(f.name, f.clone());
                }
            }
        }
        // Package functions yang dipanggil runtime (fungsi dengan body statements,
        // bukan inline satu-return) didaftarkan dengan nama qualified `pkg::func`.
        for (pkg, items) in &self.package_symbols {
            for (name, item) in items {
                if let PackageItem::Function(f) = item {
                    let qualified = Symbol::intern(&format!("{}::{}", pkg.as_str(), name.as_str()));
                    module_functions
                        .entry(qualified)
                        .or_insert_with(|| f.clone());
                }
            }
        }

        // Error di satu tempat bersifat GLOBAL: dalam mode StrictSimulation,
        // kehadiran diagnostic error apa pun (statement gagal, module gagal
        // elaborasi, unresolved instantiation, dll) membuat elaborasi gagal
        // total → caller (compile_str / run) tahu design tidak valid dan tidak
        // boleh disimulasikan. AnalysisRecovery tetap lanjut (untuk index/
        // lint/report tanpa simulasi).
        if mode == ElaborateMode::StrictSimulation {
            let diags = self.diag_sink.diagnostics();
            if let Some(first_err) = diags.iter().find(|d| d.is_error()) {
                return Err(SimError::from_diagnostic(first_err));
            }
        }

        // Cache IR per-signature (flatten instance custom-param) hanya
        // dibutuhkan SELAMA elaborasi. Setelah IR final dibangun (di bawah),
        // lepaskan cache agar memori cone free untuk SIMULASI — pada desain
        // besar (top_earlgrey) duplikat IR ini ~ratusan MB-1GB, penyumbang
        // peak memori yang membuat OOM di mesin kecil (RAM 6GB).
        self.module_cache.clear();

        Ok(IrDesign {
            top,
            modules: std::mem::take(&mut self.modules),
            classes,
            covergroups,
            dpi_imports,
            hier_signal_map,
            udp_defs: self.design.udp_defs.clone(),
            specify_items,
            timescale: self.design.timescale.clone(),
            module_functions,
            source_lines: if self.source_lines.is_empty() {
                None
            } else {
                Some(std::mem::take(&mut self.source_lines))
            },
            source_file: if self.source_file.is_empty() {
                None
            } else {
                Some(self.source_file.clone())
            },
            pkg_scoped_consts: std::mem::take(&mut self.pkg_param_ctx),
            coverage_exclusions: Vec::new(),
            stmt_lines: self.stmt_lines.take(),
            net_aliases,
        })
    }

    // ── Module signature computation (for incremental caching) ──

    /// Compute a structural checksum for a module AST.
    /// Uses Debug formatting of the entire module to capture ALL content:
    /// ports, params, decls, always/initial/assign bodies, function bodies, etc.
    /// Dependency instance names are also included for topological signature combining.
    fn compute_module_checksum(&self, module: &Module) -> u64 {
        use mivon_core::checksum::{combine_checksum, compute_checksum, compute_str_checksum};

        // Hash structural fields instead of Debug-formatting the entire AST
        let mut h = compute_str_checksum(module.name.as_str());
        h = combine_checksum(
            h,
            compute_checksum(&(module.ports.len() as u64).to_le_bytes()),
        );
        for port in &module.ports {
            h = combine_checksum(h, compute_str_checksum(port.name.as_str()));
            h = combine_checksum(h, compute_checksum(&[(port.direction as u8)]));
        }
        for param in &module.params {
            h = combine_checksum(h, compute_str_checksum(param.name.as_str()));
        }
        for item in &module.items {
            match item {
                ModuleItem::Instance(inst) => {
                    h = combine_checksum(h, compute_str_checksum(inst.module_name.as_str()));
                    h = combine_checksum(
                        h,
                        compute_checksum(&(inst.port_conns.len() as u64).to_le_bytes()),
                    );
                }
                ModuleItem::Param(p) => {
                    h = combine_checksum(h, compute_str_checksum(p.name.as_str()));
                }
                ModuleItem::Decl(d) => {
                    for v in &d.names {
                        h = combine_checksum(h, compute_str_checksum(v.name.as_str()));
                    }
                }
                ModuleItem::Func(f) => {
                    h = combine_checksum(h, compute_str_checksum(f.name.as_str()));
                }
                ModuleItem::Generate(g) => {
                    h = combine_checksum(
                        h,
                        compute_checksum(&(g.items.len() as u64).to_le_bytes()),
                    );
                }
                ModuleItem::Import { package, item } => {
                    h = combine_checksum(h, compute_str_checksum(package.as_str()));
                    h = combine_checksum(h, compute_str_checksum(item.as_str()));
                }
                _ => {}
            }
        }
        h
    }

    /// Compute topological order of modules so children (instantiated) come before parents.
    /// Uses Kahn's algorithm.
    fn compute_topo_order(&self, modules: &[Module], module_names: &[Symbol]) -> Vec<Symbol> {
        use std::collections::{HashMap, HashSet, VecDeque};

        let name_set: HashSet<Symbol> = module_names.iter().copied().collect();
        let mut in_degree: HashMap<Symbol, usize> = HashMap::new();
        let mut dependents: HashMap<Symbol, Vec<Symbol>> = HashMap::new(); // child → [parents]

        for module in modules {
            in_degree.entry(module.name).or_insert(0);
            for item in &module.items {
                if let ModuleItem::Instance(inst) = item {
                    if name_set.contains(&inst.module_name) {
                        // parent depends on child (child must be elaborated first)
                        dependents
                            .entry(inst.module_name)
                            .or_default()
                            .push(module.name);
                        *in_degree.entry(module.name).or_insert(0) += 1;
                    }
                }
            }
        }

        // Kahn: start with modules that have no dependencies (in_degree == 0)
        let mut queue: VecDeque<Symbol> = VecDeque::new();
        for (&name, &deg) in &in_degree {
            if deg == 0 {
                queue.push_back(name);
            }
        }

        let mut order = Vec::new();
        let mut order_set: HashSet<Symbol> = HashSet::new();
        while let Some(name) = queue.pop_front() {
            order.push(name);
            order_set.insert(name);
            if let Some(children) = dependents.get(&name) {
                for &parent in children {
                    if let Some(deg) = in_degree.get_mut(&parent) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(parent);
                        }
                    }
                }
            }
        }

        // Handle cycles: add remaining modules not in order
        for module in modules {
            if !order_set.contains(&module.name) {
                order.push(module.name);
                order_set.insert(module.name);
            }
        }

        order
    }

    fn resolve_param_values(
        &self,
        module: &Module,
        instance_overrides: &HashMap<Symbol, i64>,
    ) -> Result<HashMap<Symbol, i64>, SimError> {
        let ctx = self.collect_package_param_ctx(module);
        // Evaluasi penuh: $bits(typedef) & inlining fungsi package untuk default
        // param — evaluator skalar sederhana tidak punya akses package_symbols.
        // `structs` = index struct package GLOBAL (lihat komentar di jalur
        // generate expansion) agar default `Info = PartInfoDefault` membawa
        // member keys `Info.size` dst. ke param_vals.
        let pkg_full = PkgFullCtx {
            scalars: &self.pkg_const_scalars,
            arrays: &self.pkg_const_arrays,
            package_symbols: &self.package_symbols,
            structs: &self.pkg_struct_ref_index,
        };
        resolve_param_values_with_ctx(module, instance_overrides, &ctx, Some(&pkg_full))
            .map_err(|e| self.elab_diag(DiagCode::ParamMismatch, e))
    }

    /// Hitung context package global SEKALI: qualified `pkg::name` untuk semua
    /// parameter package + enum member (plain & qualified) dari semua package,
    /// plus konstanta package ter-evaluasi. Hasil disimpan di `self.pkg_param_ctx`
    /// Basis eksponen satuan (satuan TERHALUS) design — dipakai bersama
    /// `compute_module_delay_scales` dan `delay_scale_for_unit`.
    fn design_base_exp(&self) -> i32 {
        let mut exps: Vec<i32> = Vec::new();
        if let Some(e) = self
            .design
            .timescale
            .as_ref()
            .and_then(|(u, _)| crate::util::generate::time_unit_exponent(u))
        {
            exps.push(e);
        }
        for module in &self.design.modules {
            if let Some(e) = module
                .timescale
                .as_ref()
                .and_then(|(u, _)| crate::util::generate::time_unit_exponent(u))
            {
                exps.push(e);
            }
        }
        for iface in &self.design.interfaces {
            if let Some(e) = iface
                .timescale
                .as_ref()
                .and_then(|(u, _)| crate::util::generate::time_unit_exponent(u))
            {
                exps.push(e);
            }
        }
        exps.into_iter().min().unwrap_or(-9)
    }

    /// Faktor skala delay untuk satu satuan `timescale` (F84).
    ///
    /// Dipakai untuk entri yang TIDAK ada di `design.modules` — misalnya
    /// `Interface` yang dielaborasi sebagai `Module` sintetik. Tanpa ini
    /// interface `1us` punya faktor 1 dan delay-nya salah diam-diam.
    fn delay_scale_for_unit(&self, unit: Option<&(String, String)>) -> u64 {
        let Some((u, _)) = unit else {
            return 1;
        };
        let Some(exp) = crate::util::generate::time_unit_exponent(u) else {
            return 1;
        };
        let diff = exp - self.design_base_exp();
        if diff <= 0 {
            1
        } else {
            // Batasi 12 digit supaya satuan raksasa (`1s` vs basis `1fs`) tak
            // overflow u64 dan delay jadi tak masuk akal.
            10u64.checked_pow(diff.min(12) as u32).unwrap_or(1)
        }
    }

    /// Hitung peta `module → faktor skala delay` (F84, LRM §19.8).
    ///
    /// `#N` di module dengan satuan lebih besar dari basis (`1us` pada basis
    /// `1ns`) dikali 10^diff; module pada basis dapat faktor 1. Karena basis =
    /// satuan TERHALUS design, `diff` tak pernah negatif — delay tak pernah
    /// terpotong jadi 0 dan tak ada risiko loop tak berujung.
    ///
    /// Module TANPA directive `timescale` tidak masuk peta (faktor 1). Itu
    /// keputusan produk, bukan LRM: §19.8 menyatakan modul tanpa directive
    /// memakai default simulator, dan `iverilog` memakai `1s` — jauh dari
    /// default mivon `1ns`. Memakai `1ns` menjaga kompatibilitas dengan
    /// ribuan design/test lama yang tak pernah menulis directive.
    fn compute_module_delay_scales(&mut self) {
        use std::collections::HashMap;

        let mut scales: HashMap<Symbol, u64> = HashMap::new();
        for module in &self.design.modules {
            let scale = self.delay_scale_for_unit(module.timescale.as_ref());
            if module.timescale.is_some() {
                scales.insert(module.name, scale);
            }
        }
        // Interface ikut dihitung di sini karena `elaborate_module` untuk
        // module sintetiknya berjalan SETELAH fungsi ini; entri yang diinsert
        // nanti di loop interface (lihat pemanggil) tidak akan terpakai kalau
        // basisnya berubah di tengah. Aman: basis design-wide, bukan per-item.
        for iface in &self.design.interfaces {
            let scale = self.delay_scale_for_unit(iface.timescale.as_ref());
            if iface.timescale.is_some() {
                scales.insert(iface.name, scale);
            }
        }
        self.module_delay_scale = scales;
    }

    /// Faktor skala delay untuk module yang sedang dielaborasi (F84).
    fn current_delay_scale(&self) -> u64 {
        self.current_module
            .and_then(|m| self.module_delay_scale.get(&m).copied())
            .unwrap_or(1)
    }

    /// Bangun global package param context SEKALI: qualified `pkg::name` untuk semua
    /// parameter package + enum member (plain & qualified) dari semua package,
    /// plus konstanta package ter-evaluasi. Hasil disimpan di `self.pkg_param_ctx`
    /// dan di-clone oleh tiap module (lihat `collect_package_param_ctx`).
    fn build_pkg_param_ctx(&mut self) {
        let mut ctx: HashMap<Symbol, i64> = HashMap::new();
        // Enum member dari typedef $unit (top-level `typedef enum { A, B } E;`
        // di luar package/module) — GENARASI dari `mgen` menaruh typedef level
        // file di `.svh`, jadi pola `st = RUN` setelahnya HARUS resolve.
        // Sebelumnya hanya enum PACKAGE yang di-register → E2001 "signal
        // 'RUN' not found" untuk desain `.mv` hasil generate (dan SV murni).
        // Counter `last` di-reset per typedef (sama seperti enum package).
        // Nilai dihitung di dalam fixpoint di bawah (bersama package) agar
        // referensi-maju/mundur antar-typedef ikut resolve; member yang
        // ekspresinya tak ter-evaluasi dilewati (bukan ditebak) dan
        // dilaporkan pasca-fixpoint oleh verify_enum_members.
        let unit_enums: Vec<Vec<(Symbol, Option<Expr>)>> = self
            .design
            .unit_typedefs
            .iter()
            .filter_map(|td| {
                if let DataType::EnumType { members, .. } = &td.dtype {
                    Some(members.clone())
                } else {
                    None
                }
            })
            .collect();
        // Enum member constants dari package (plain + qualified, sequential).
        // Struktur per-typedef (Vec<Vec<…>>) — member enum tanpa nilai eksplisit
        // melanjutkan counter HANYA dalam typedef yang sama (standar SV).
        // Sebelumnya di-flatten jadi satu list per package sehingga counter
        // `last` bocor lintas enum → mis. `alu_op_base_e` di otbn_pkg mendapat
        // nilai 256+ (harusnya 0..8) karena menumpuk setelah enum lain.
        let pkg_enums: Vec<(Symbol, Vec<Vec<(Symbol, Option<Expr>)>>)> = self
            .package_symbols
            .iter()
            .filter_map(|(pkg_name, items)| {
                let enums: Vec<Vec<(Symbol, Option<Expr>)>> = items
                    .values()
                    .filter_map(|item| {
                        if let PackageItem::Typedef(td) = item {
                            if let DataType::EnumType { members, .. } = &td.dtype {
                                Some(members.clone())
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    })
                    .collect();
                if enums.is_empty() {
                    None
                } else {
                    Some((*pkg_name, enums))
                }
            })
            .collect();
        // Cache param package-sendiri per package (plain name → nilai) dari
        // skalar precompute — fallback forward-ref `enum {F = W}` ke param
        // yang dideklarasikan belakangan (valid SV, elaborasi order-free).
        // Dibangun sekali (nilai statis); dipakai hanya di jalur gagal.
        let mut pkg_param_cache: HashMap<Symbol, Vec<(Symbol, i64)>> = HashMap::new();
        for (k, v) in &self.pkg_const_scalars {
            if let Some((pkg, rest)) = k.as_str().split_once("::") {
                if !rest.contains([':', '.', '[']) {
                    pkg_param_cache
                        .entry(Symbol::intern(pkg))
                        .or_default()
                        .push((Symbol::intern(rest), *v));
                }
            }
        }
        for _ in 0..64 {
            let mut changed = false;
            // Enum $unit di dalam fixpoint (bukan sekali jalan) agar
            // referensi ke package-const maupun antar-typedef resolve.
            // GAGAL eval → lewati (jangan tebak `last`): percobaan ulang
            // iterasi berikut; yang tersisa dilaporkan pasca-fixpoint.
            for members in &unit_enums {
                let mut last: Option<i64> = Some(0);
                for (member_name, member_expr) in members {
                    let val = match member_expr {
                        Some(expr) => Self::eval_enum_member_value(expr, &ctx, None),
                        None => last,
                    };
                    if let Some(v) = val {
                        if !ctx.contains_key(member_name) {
                            ctx.insert(*member_name, v);
                            changed = true;
                        }
                    }
                    last = val.map(|v| v.wrapping_add(1));
                }
            }
            // Qualified names untuk SEMUA package (agar scoped reference resolve)
            for (pkg_name, items) in &self.package_symbols {
                for (name, item) in items {
                    let PackageItem::Param(p) = item else {
                        continue;
                    };
                    let qualified = Symbol::intern(&format!("{}::{}", pkg_name, name));
                    if ctx.contains_key(&qualified) {
                        continue;
                    }
                    if let Some(expr) = &p.default {
                        if let Ok(val) = const_eval_with_params(expr, &ctx) {
                            ctx.insert(qualified, val);
                            changed = true;
                        }
                    }
                }
            }
            // Enum member constants di package (plain + qualified, sequential).
            // `last` di-reset per typedef enum (lihat komentar pkg_enums).
            // GAGAL eval → lewati (jangan tebak): retry iterasi berikut.
            // Overlay param package-sendiri (scope LRM menang atas global):
            // dibangun sekali per package per iterasi.
            for (pkg_name, enums) in &pkg_enums {
                let own: Option<&[(Symbol, i64)]> =
                    pkg_param_cache.get(pkg_name).map(|v| v.as_slice());
                let overlay = Self::package_overlay(&ctx, own);
                for members in enums {
                    let mut last: Option<i64> = Some(0);
                    for (member_name, member_expr) in members {
                        let val = match member_expr {
                            Some(expr) => {
                                Self::eval_enum_member_value(expr, &ctx, overlay.as_ref())
                            }
                            None => last,
                        };
                        if let Some(v) = val {
                            if !ctx.contains_key(member_name) {
                                ctx.insert(*member_name, v);
                                changed = true;
                            }
                            let qualified =
                                Symbol::intern(&format!("{}::{}", pkg_name, member_name));
                            if let std::collections::hash_map::Entry::Vacant(e) =
                                ctx.entry(qualified)
                            {
                                e.insert(v);
                                changed = true;
                            }
                        }
                        last = val.map(|v| v.wrapping_add(1));
                    }
                }
            }
            if !changed {
                break;
            }
        }
        // Merge konstanta package yang sudah dievaluasi penuh (skalar + array).
        mivon_ast::const_eval_ext::flatten_consts_into_ctx(
            &self.pkg_const_scalars,
            &self.pkg_const_arrays,
            &mut ctx,
        );
        // Verifikasi: member enum eksplisit yang tetap tak ter-evaluasi
        // setelah fixpoint (typo/unknown, bukan forward-ref) dilaporkan —
        // sebelumnya ditebak diam-diam dari counter `last` (silent wrong).
        self.verify_enum_members(&unit_enums, "lingkup $unit", &ctx, None);
        for (pkg_name, enums) in &pkg_enums {
            let own: Option<&[(Symbol, i64)]> =
                pkg_param_cache.get(pkg_name).map(|v| v.as_slice());
            let overlay = Self::package_overlay(&ctx, own);
            for members in enums {
                self.verify_enum_members(
                    std::slice::from_ref(members),
                    &format!("package '{pkg_name}'"),
                    &ctx,
                    overlay.as_ref(),
                );
            }
        }
        self.pkg_param_ctx = ctx;

        // ── Context $unit imports (unit_imports) — sekali, untuk semua module ──
        // Resolusi plain param names + flatten konstanta untuk import set global.
        let unit_imports: Vec<(Symbol, Symbol)> = self.design.unit_imports.clone();
        let mut unit_ctx: HashMap<Symbol, i64> = self.pkg_param_ctx.clone();
        for _ in 0..64 {
            let mut changed = false;
            for (package, import_item) in &unit_imports {
                let Some(pkg_items) = self.package_symbols.get(package) else {
                    continue;
                };
                let names: Vec<Symbol> = if import_item.as_str() == "*" {
                    pkg_items.keys().copied().collect()
                } else {
                    vec![*import_item]
                };
                for name in names {
                    if let Some(PackageItem::Param(p)) = pkg_items.get(&name) {
                        if unit_ctx.contains_key(&p.name) {
                            continue;
                        }
                        if let Some(expr) = &p.default {
                            if let Ok(val) = const_eval_with_params(expr, &unit_ctx) {
                                unit_ctx.insert(p.name, val);
                                changed = true;
                            }
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
        for (package, import_item) in &unit_imports {
            mivon_ast::const_eval_ext::flatten_imported_consts_into_ctx(
                package.as_str(),
                import_item.as_str(),
                &self.pkg_const_scalars,
                &self.pkg_const_arrays,
                &mut unit_ctx,
            );
        }
        // Hanya simpan DELTA vs pkg_param_ctx — sebagian besar entri sudah sama
        // (qualified names). Collect per-modul tinggal extend delta kecil,
        // menghindari ~33k insert duplikat untuk tiap module.
        let mut delta: HashMap<Symbol, i64> = HashMap::new();
        for (k, v) in &unit_ctx {
            if self.pkg_param_ctx.get(k) != Some(v) {
                delta.insert(*k, *v);
            }
        }
        // ── Bake unit-import delta ke pkg_param_ctx GLOBAL ──
        // Setiap module menerima extend(unit_import_ctx) yang SAMA di
        // collect_package_param_ctx — baking ke base sekali membuat module
        // TANPA import/enum milik sendiri bisa meminjam base (Cow::Borrowed)
        // tanpa clone 104k-entry per module (kerja berulang terbesar).
        for (k, v) in &delta {
            self.pkg_param_ctx.insert(*k, *v);
        }
        self.unit_import_ctx = delta;

        // ── Plain param per package — sekali, untuk import milik module ──
        // Base: context global yang sudah lengkap (qualified + unit imports).
        let base: HashMap<Symbol, i64> = self.unit_import_ctx.clone();
        let mut plain_map: HashMap<Symbol, HashMap<Symbol, i64>> = HashMap::new();
        for (pkg_name, items) in &self.package_symbols {
            let mut pctx = base.clone();
            for _ in 0..64 {
                let mut changed = false;
                for item in items.values() {
                    let PackageItem::Param(p) = item else {
                        continue;
                    };
                    if pctx.contains_key(&p.name) {
                        continue;
                    }
                    if let Some(expr) = &p.default {
                        if let Ok(val) = const_eval_with_params(expr, &pctx) {
                            pctx.insert(p.name, val);
                            changed = true;
                        }
                    }
                }
                if !changed {
                    break;
                }
            }
            let mut plain: HashMap<Symbol, i64> = HashMap::new();
            for (name, item) in items {
                let PackageItem::Param(p) = item else {
                    continue;
                };
                if let Some(&v) = pctx.get(&p.name) {
                    plain.insert(*name, v);
                }
            }
            // Member keys struct / array-of-struct package juga harus terlihat
            // sebagai PLAIN name (`PartInfo[0].offset`, `Info.size`) untuk
            // `import pkg::*` — bukan hanya qualified `pkg::PartInfo[0].offset`.
            // Tanpa ini, generate if `PartInfo[k].offset == 0` di module yang
            // meng-import package gagal const-eval (member access not allowed
            // in constant expression) → error E3001/E3003 palsu di OpenTitan
            // (otp_ctrl / otp_ctrl_part_buf / otp_ctrl_part_unbuf).
            let pkg_prefix = format!("{}::", pkg_name.as_str());
            for (qname, v) in &self.pkg_const_scalars {
                if let Some(rest) = qname.as_str().strip_prefix(&pkg_prefix) {
                    // Hanya member keys (mengandung `.`) — param skalar & enum
                    // member sudah masuk via loop di atas / pkg_param_ctx.
                    if rest.contains('.') {
                        plain.entry(Symbol::intern(rest)).or_insert(*v);
                    }
                }
            }
            plain_map.insert(*pkg_name, plain);
        }
        self.pkg_plain_params = plain_map;
        if std::env::var("DBG_ELAB").is_ok() {
            let gp = Symbol::intern("gpio_env_pkg");
            let gn = Symbol::intern("NUM_GPIOS");
            let plain_n = self.pkg_plain_params.get(&gp).map(|m| m.len()).unwrap_or(0);
            let has_ng = self
                .pkg_plain_params
                .get(&gp)
                .map(|m| m.contains_key(&gn))
                .unwrap_or(false);
            let pkg_has = self.package_symbols.contains_key(&gp);
            eprintln!("[DBG-ELAB]   pkg_plain_params[gpio_env_pkg] len={} has_NUM_GPIOS={} pkg_present={}", plain_n, has_ng, pkg_has);
        }
    }

    /// Kumpulkan nilai parameter package yang terlihat oleh module via
    /// `import pkg::*`/`import pkg::item` (baik di header module maupun $unit).
    /// Dipakai sebagai base context saat evaluasi konstanta (generate limit,
    /// parameter default, dll.) sehingga package parameter bisa di-resolve.
    /// Package param yang di-referensikan secara scoped (`pkg::name`) juga
    /// didaftarkan agar `const_eval_with_params` bisa me-resolve.
    fn collect_package_param_ctx(
        &self,
        module: &Module,
    ) -> std::borrow::Cow<'_, HashMap<Symbol, i64>> {
        // ── Fast path: module TANPA import pkg milik sendiri & tanpa enum
        // modul → base GLOBAL (pkg_param_ctx dengan unit-import delta sudah
        // di-bake saat build) dipinjam TANPA clone. Sebelumnya map 100k+
        // entry di-clone utk SETIAP module × setiap fase (expansion loop +
        // tiap resolve_param_values) = kerja berulang terbesar elaborate. ──
        let mut has_own_imports = false;
        let mut has_own_enums = false;
        for item in &module.items {
            match item {
                ModuleItem::Import { .. } => {
                    has_own_imports = true;
                    if has_own_enums {
                        break;
                    }
                }
                ModuleItem::Typedef(td) => {
                    if matches!(td.dtype, DataType::EnumType { .. }) {
                        has_own_enums = true;
                        if has_own_imports {
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
        if !has_own_imports && !has_own_enums {
            return std::borrow::Cow::Borrowed(&self.pkg_param_ctx);
        }
        let dbg = std::env::var("DBG_ELAB").is_ok();
        let t0 = std::time::Instant::now();
        let mut ctx: HashMap<Symbol, i64> = self.pkg_param_ctx.clone();
        let t1 = std::time::Instant::now();
        if dbg {
            eprintln!(
                "[DBG-ELAB]   collect clone pkg ctx: {}us (pkg_ctx={})",
                t1.duration_since(t0).as_micros(),
                self.pkg_param_ctx.len()
            );
        }
        // Context $unit imports sudah di-precompute (konstan antar-module).
        ctx.extend(self.unit_import_ctx.iter().map(|(k, v)| (*k, *v)));
        let t2 = std::time::Instant::now();
        if dbg {
            eprintln!(
                "[DBG-ELAB]   collect extend unit ctx: {}us (unit_ctx={})",
                t2.duration_since(t1).as_micros(),
                self.unit_import_ctx.len()
            );
        }
        // Import set milik module itu sendiri (di luar $unit).
        let module_imports: Vec<(Symbol, Symbol)> = module
            .items
            .iter()
            .filter_map(|item| {
                if let ModuleItem::Import { package, item } = item {
                    Some((*package, *item))
                } else {
                    None
                }
            })
            .collect();
        // Enum member constants dari typedef di body module (e.g. LastEdnEntry)
        let module_enums: Vec<Vec<(Symbol, Option<Expr>)>> = module
            .items
            .iter()
            .filter_map(|item| {
                if let ModuleItem::Typedef(td) = item {
                    if let DataType::EnumType { members, .. } = &td.dtype {
                        Some(members.clone())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();
        // Fixed-point: modul-local enum member & plain import names bisa
        // mereferensikan package param (dan satu sama lain). Plain param
        // package sudah di-precompute (pkg_plain_params); hanya enum modul yang
        // perlu fixed-point.
        let tf = std::time::Instant::now();
        for _ in 0..64 {
            let mut changed = false;
            // Enum member constants dari body module (sequential values).
            // GAGAL eval → lewati (jangan tebak): retry iterasi berikut.
            for members in &module_enums {
                let mut last: Option<i64> = Some(0);
                for (member_name, member_expr) in members {
                    let val = match member_expr {
                        Some(expr) => match const_eval_with_params(expr, &ctx) {
                            Ok(v) => Some(v),
                            Err(_) => None,
                        },
                        None => last,
                    };
                    if let Some(v) = val {
                        if !ctx.contains_key(member_name) {
                            ctx.insert(*member_name, v);
                            changed = true;
                        }
                    }
                    last = val.map(|v| v.wrapping_add(1));
                }
            }
            // Plain names untuk package yang di-import module
            for (package, import_item) in &module_imports {
                let Some(plain) = self.pkg_plain_params.get(package) else {
                    continue;
                };
                if import_item.as_str() == "*" {
                    for (&name, &val) in plain {
                        ctx.entry(name).or_insert(val);
                    }
                } else if let Some(&val) = plain.get(import_item) {
                    ctx.entry(*import_item).or_insert(val);
                }
            }
            if !changed {
                break;
            }
        }
        if dbg && tf.elapsed().as_micros() > 20_000 {
            eprintln!(
                "[DBG-ELAB]   collect fixed-point: {}us (module_imports={}, enums={})",
                tf.elapsed().as_micros(),
                module_imports.len(),
                module_enums.len()
            );
        }
        // Verifikasi di sini SENGAJA dilewati: enum lokal module diverifikasi
        // sekali di elaborate_module_with_params (ctx effective_params lebih
        // lengkap: parameter module + generate). Verifikasi ganda = warning
        // duplikat untuk typedef yang sama.
        // Merge konstanta package yang sudah dievaluasi penuh — hanya untuk
        // import set milik module ($unit sudah ada di unit_import_ctx).
        let tl = std::time::Instant::now();
        for (package, import_item) in &module_imports {
            mivon_ast::const_eval_ext::flatten_imported_consts_into_ctx(
                package.as_str(),
                import_item.as_str(),
                &self.pkg_const_scalars,
                &self.pkg_const_arrays,
                &mut ctx,
            );
        }
        if dbg && module.name.as_str() == "rv_core_ibex_peri" {
            let gn = Symbol::intern("NumRegions");
            let ga = Symbol::intern("NumAlerts");
            eprintln!(
                "[DBG-ELAB]   peri imports={:?}",
                module_imports
                    .iter()
                    .map(|(p, i)| format!("{}::{}", p.as_str(), i.as_str()))
                    .collect::<Vec<_>>()
            );
            eprintln!("[DBG-ELAB]   peri has_pkg_reg={} NumRegions_in_ctx={} NumAlerts_in_ctx={} ctx_len={}", self.package_symbols.contains_key(&Symbol::intern("rv_core_ibex_reg_pkg")), ctx.contains_key(&gn), ctx.contains_key(&ga), ctx.len());
            eprintln!(
                "[DBG-ELAB]   pkg_plain_params has reg_pkg={}",
                self.pkg_plain_params
                    .contains_key(&Symbol::intern("rv_core_ibex_reg_pkg"))
            );
        }
        if dbg && module.name.as_str() == "tb" {
            let gn = Symbol::intern("NUM_GPIOS");
            eprintln!(
                "[DBG-ELAB]   tb imports={:?} NUM_GPIOS_in_ctx={}",
                module_imports
                    .iter()
                    .map(|(p, i)| format!("{}::{}", p.as_str(), i.as_str()))
                    .collect::<Vec<_>>(),
                ctx.contains_key(&gn)
            );
        }
        if dbg && tl.elapsed().as_micros() > 20_000 {
            eprintln!(
                "[DBG-ELAB]   collect flatten-module: {}us",
                tl.elapsed().as_micros()
            );
        }
        if dbg && t0.elapsed().as_micros() > 50_000 {
            eprintln!("[DBG-ELAB]   collect total: {}us", t0.elapsed().as_micros());
        }
        std::borrow::Cow::Owned(ctx)
    }

    /// Buat structured diagnostic untuk elaboration error dengan error code tepat.
    fn elab_diag(&self, code: DiagCode, message: impl Into<String>) -> SimError {
        self.elab_diag_at(code, message, 0, 0)
    }

    /// Evaluasi nilai eksplisit member enum: overlay param-sendiri DULU
    /// (scope LRM menang atas global untuk nama sama), lalu konteks global.
    /// Overlay (`None` = tanpa param sendiri) dibangun malas oleh pemanggil
    /// agar tak clone map besar sia-sia.
    fn eval_enum_member_value(
        expr: &Expr,
        ctx: &HashMap<Symbol, i64>,
        overlay: Option<&HashMap<Symbol, i64>>,
    ) -> Option<i64> {
        if let Some(o) = overlay {
            if let Ok(v) = const_eval_with_params(expr, o) {
                return Some(v);
            }
        }
        const_eval_with_params(expr, ctx).ok()
    }

    /// Overlay param-sendiri untuk satu package: global + param package
    /// (menang). `None` bila package tak punya param ter-cache (tak perlu).
    fn package_overlay(
        ctx: &HashMap<Symbol, i64>,
        own: Option<&[(Symbol, i64)]>,
    ) -> Option<HashMap<Symbol, i64>> {
        let params = own.filter(|p| !p.is_empty())?;
        let mut overlay = ctx.clone();
        for (n, v) in params {
            overlay.insert(*n, *v);
        }
        Some(overlay)
    }

    /// Verifikasi pasca-fixpoint: member enum bernilai eksplisit yang tetap
    /// tak ter-evaluasi (typo/unknown — bukan forward-ref yang sudah
    /// konvergen) dilaporkan sebagai warning, bukan ditebak diam-diam dari
    /// counter `last`. Satu warning per typedef (member gagal pertama);
    /// nama yang sudah di-warning dilewati (scan ganda items+decls).
    fn verify_enum_members(
        &self,
        enums: &[Vec<(Symbol, Option<Expr>)>],
        context: &str,
        ctx: &HashMap<Symbol, i64>,
        overlay: Option<&HashMap<Symbol, i64>>,
    ) {
        let mut warned: std::collections::HashSet<Symbol> = std::collections::HashSet::new();
        for members in enums {
            let mut last: Option<i64> = Some(0);
            for (member_name, member_expr) in members {
                let val = match member_expr {
                    Some(expr) => Self::eval_enum_member_value(expr, ctx, overlay),
                    None => last,
                };
                if member_expr.is_some() && val.is_none() && warned.insert(*member_name) {
                    self.elab_warn(
                        DiagCode::ParamMismatch,
                        format!(
                            "nilai enum member '{member_name}' bukan ekspresi konstan ({context}) — \
                             member dilewati; rujukan ke sana gagal E2001"
                        ),
                    );
                    break;
                }
                last = val.map(|v| v.wrapping_add(1));
            }
        }
    }

    /// Tolak packed width di atas `MAX_PACKED_WIDTH` dgn diag bersih (E3012).
    /// Mencegah hang/OOM/silent-clamp pada width absurd (mis. `2**31`).
    /// Nama module yang sedang di-elaborasi disertakan dalam pesan — tanpa
    /// ini error E3012 tercatat tanpa konteks modul dan `find_name_in_source`
    /// (fallback lokasi) bisa menunjuk ke kemunculan nama port yang sama di
    /// FILE/modul lain (pesan menyesatkan).
    fn check_packed_width(&self, width: usize, what: &str) -> Result<(), SimError> {
        if width > MAX_PACKED_WIDTH {
            let mod_ctx = self
                .current_module
                .map(|m| format!(" in module '{}'", m.as_str()))
                .unwrap_or_default();
            return Err(self.elab_diag(
                DiagCode::PackedWidthTooLarge,
                format!(
                    "{what}{mod_ctx}: packed width {width} bit exceeds the implementation limit \
                     ({MAX_PACKED_WIDTH} bits) — refusing to elaborate an impractical \
                     vector (hang/OOM/incorrect truncation)"
                ),
            ));
        }
        Ok(())
    }

    /// Extract source file name from `line directives in source_lines for a given line.
    /// The source_lines contain `` `line 1 "filename.sv" `` directives from preprocessing.
    /// Resolve nama file + baris relatif-file untuk posisi di merged source.
    fn resolve_source_location(&self, line: usize) -> (String, usize) {
        mivon_core::diagnostics::resolve_source_location(
            &self.source_lines,
            &self.source_file,
            line,
        )
    }

    /// Cari posisi (baris, kolom) kemunculan pertama sebuah nama di merged
    /// source. Dipakai sebagai FALLBACK saat error/warning elaboration tidak
    /// membawa lokasi (line=0,col=0): nama diekstrak dari pesan `'X'` lalu
    /// baris pertama yang memuat X dijadikan posisi snippet. Baris directive
    /// `` `line `` dilewati (bukan konten user). Mengembalikan (0,0) bila nama
    /// tidak bisa diekstrak / tidak ditemukan — diagnostic tetap tanpa snippet.
    fn find_name_in_source(&self, message: &str) -> (usize, usize) {
        let name = match message.find('\'') {
            Some(s) => {
                let rest = &message[s + 1..];
                match rest.find('\'') {
                    Some(e) => &rest[..e],
                    None => return self.find_literal_in_source(message),
                }
            }
            None => {
                // Pesan dengan string literal (mis. `invalid lvalue expression:
                // string literal "fz.sv"`) memakai double-quote — ambil isinya
                // agar posisi bisa ditemukan di source.
                let Some(s) = message.find('"') else {
                    return self.find_literal_in_source(message);
                };
                let rest = &message[s + 1..];
                match rest.find('"') {
                    Some(e) => &rest[..e],
                    None => return self.find_literal_in_source(message),
                }
            }
        };
        if name.is_empty() || name.len() > 128 {
            return self.find_literal_in_source(message);
        }
        // Nama yang TIDAK plausibel sebagai token source (mengandung spasi,
        // kurung kurawal, dll. — mis. debug-format `Ident { name: Symbol(...)
        // }.mosi` dari pesan width-mismatch) TIDAK akan pernah ditemukan di
        // source. Tanpa guard ini, setiap nama unik memaksa scan penuh 1.1M
        // baris merged source (~1.5-2s) — module TB/DV dengan banyak diagnostic
        // seperti itu (mis. spid_status_tb) bisa memakan 100+ detik.
        // Karakter yang diizinkan: identifier SV + kualifikasi `::` +
        // member access `.` + array `[]` + `$` (system) + angka.
        if !name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | ':' | '.' | '[' | ']' | '$' | '-'))
        {
            return self.find_literal_in_source(message);
        }
        // Pure function dari source_lines — memoize per nama. Tanpa ini setiap
        // diagnostic tanpa lokasi meng-scan seluruh merged source (1.1M baris
        // di OpenTitan) yang memakan ~1.5-2s per panggilan.
        let sym = Symbol::intern(name);
        if let Some(&loc) = self.source_name_loc.borrow().get(&sym) {
            return loc;
        }
        let result = (|| {
            // Cocokkan nama sebagai TOKEN utuh (bukan substring): fallback
            // lokasi untuk `in_i` tidak boleh menunjuk ke `tl_in_i`/`in_io` di
            // file lain (snippet menyesatkan). Boundary luar = bukan
            // alphanumeric/'_'. Nama bisa memuat `::`/`.`/`[]` — substring
            // utuh dibandingkan, hanya tepi luar yang dicek.
            let is_id = |c: char| c.is_alphanumeric() || c == '_';
            for (i, line) in self.source_lines.iter().enumerate() {
                if line.trim_start().starts_with('`') {
                    continue;
                }
                let mut start = 0usize;
                while let Some(rel) = line[start..].find(name) {
                    let abs = start + rel;
                    let before_ok = match line[..abs].chars().next_back() {
                        Some(c) => !is_id(c),
                        None => true,
                    };
                    let after_end = abs + name.len();
                    let after_ok = match line[after_end..].chars().next() {
                        Some(c) => !is_id(c),
                        None => true,
                    };
                    if before_ok && after_ok {
                        return (i + 1, abs + 1);
                    }
                    start = abs + 1;
                }
            }
            (0, 0)
        })();
        self.source_name_loc.borrow_mut().insert(sym, result);
        result
    }

    /// Fallback: pesan tanpa nama-quoted (mis. `fill literal ('0/'1/'x/'z)`)
    /// — cari pola literal di source (baris/lokasi assignment paling dekat).
    fn find_literal_in_source(&self, message: &str) -> (usize, usize) {
        // Hasil scan HANYA bergantung pada source_lines: targets literal
        // tetap (['0,'1,'x,'z,'X,'Z) dan message tidak dipakai untuk memilih
        // baris — jadi hasil konstan per Elaborator. Lihat komentar field
        // `source_literal_loc` (hotspot gdb: 3 sample identik, 100% CPU di
        // sini pada run OpenTitan).
        let _ = message;
        if let Some(loc) = *self.source_literal_loc.borrow() {
            return loc;
        }
        // Cari pola literal yang mungkin muncul di pesan: `'0`, `'1`, `'x`, `'z`.
        let targets = ["'0", "'1", "'x", "'z", "'X", "'Z"];
        let mut best: Option<(usize, usize)> = None;
        for (i, line) in self.source_lines.iter().enumerate() {
            if line.trim_start().starts_with('`') {
                continue;
            }
            let fits = targets.iter().find(|t| line.contains(**t));
            if let Some(t) = fits {
                if let Some(col) = line.find(t) {
                    // Pilih lokasi PALING BELAKANG di source (assignment
                    // biasanya di baris akhir blok) — memoize per-pesan tak
                    // perlu di sini (hasil konstan per source).
                    best = Some((i + 1, col + 1));
                }
            }
        }
        // Tidak menemukan literal — pakai baris terakhir source sebagai
        // anker (masih lebih baik dari tanpa lokasi).
        let loc = best.unwrap_or_else(|| {
            let n = self.source_lines.len();
            if n > 0 {
                (n, 1)
            } else {
                (0, 0)
            }
        });
        *self.source_literal_loc.borrow_mut() = Some(loc);
        loc
    }

    /// Buat error diagnostic dengan posisi source.
    fn elab_diag_at(
        &self,
        code: DiagCode,
        message: impl Into<String>,
        line: usize,
        col: usize,
    ) -> SimError {
        let msg: String = message.into();
        let (line, col) = if line > 0 && col > 0 {
            (line, col)
        } else {
            // Fallback global: cari nama dari pesan di source (mis.
            // `'UVM_HDL_MAX_WIDTH' not found in parameter context` dari
            // resolve_param_values yang tidak membawa lokasi).
            self.find_name_in_source(&msg)
        };
        let mut diag = Diagnostic::new(DiagLevel::Error, code, msg).with_code_context();
        if line > 0 && line <= self.source_lines.len() {
            let source_line = &self.source_lines[line - 1];
            let (file, display_line) = self.resolve_source_location(line);
            let snippet = SourceSnippet::new(file, display_line, col, source_line);
            diag = diag.with_source_snippet(snippet);
        }
        if let Some(ref mod_name) = self.current_module {
            let ctx = RuntimeContext::new().with_module(mod_name.as_str());
            diag = diag.with_runtime_context(ctx);
        }
        SimError::from_elab_diagnostic(diag)
    }

    /// Emit warning diagnostic ke DiagSink (elaboration-time warnings).
    #[allow(dead_code)]
    fn elab_warn(&self, code: DiagCode, message: impl Into<String>) {
        self.elab_warn_at(code, message, 0, 0)
    }

    /// Emit warning dengan posisi source.
    fn elab_warn_at(&self, code: DiagCode, message: impl Into<String>, line: usize, col: usize) {
        let msg: String = message.into();
        let (line, col) = if line > 0 && col > 0 {
            (line, col)
        } else {
            // Fallback global (lihat elab_diag_at): `unknown type 'X' is not
            // defined in this scope` (resolve_type_width) dipanggil tanpa
            // lokasi — posisi diambil dari baris pemakaian nama type.
            self.find_name_in_source(&msg)
        };
        let mut diag = Diagnostic::new(DiagLevel::Warning, code, msg).with_code_context();
        if line > 0 && line <= self.source_lines.len() {
            let source_line = &self.source_lines[line - 1];
            let (file, display_line) = self.resolve_source_location(line);
            let snippet = SourceSnippet::new(file, display_line, col, source_line);
            diag = diag.with_source_snippet(snippet);
        }
        self.diag_sink.push(diag);
    }

    /// Flush diagnostics from DiagSink and return them.
    pub fn flush_diagnostics(&self) -> Vec<Diagnostic> {
        self.diag_sink.diagnostics()
    }

    /// Prime the module cache from an existing cache (session-level persistence).
    pub fn set_cache(&mut self, cache: HashMap<u64, IrModule>) {
        self.module_cache = cache;
    }

    /// Take the module cache out for session-level storage after elaboration.
    pub fn take_cache(&mut self) -> HashMap<u64, IrModule> {
        std::mem::take(&mut self.module_cache)
    }
}

impl Elaborator {
    /// Nama param bertipe SIGNED di module (header + body + $unit). Dipakai
    /// resolve `Expr::Ident(parameter)` dan evaluasi kondisi generate-if.
    fn module_param_signed_set(&self, module: &Module) -> std::collections::HashSet<Symbol> {
        let mut set = std::collections::HashSet::new();
        for p in &module.params {
            if param_decl_is_signed(p) {
                set.insert(p.name);
            }
        }
        for item in &module.items {
            if let ModuleItem::Param(p) = item {
                if param_decl_is_signed(p) {
                    set.insert(p.name);
                }
            }
        }
        for p in &self.design.unit_params {
            if param_decl_is_signed(p) {
                set.insert(p.name);
            }
        }
        set
    }

    fn elaborate_module(
        &mut self,
        module: &Module,
        known_modules: &[Symbol],
    ) -> Result<IrModule, SimError> {
        self.current_module = Some(module.name);
        let param_vals = self.resolve_param_values(module, &HashMap::new())?;
        let ir = self.elaborate_module_with_params(module, known_modules, &param_vals)?;
        // Ringkasan assign-context-width per module (DBG_ELAB_STEP).
        if std::env::var("DBG_ELAB_STEP").is_ok() && self.dbg_apply_n.get() > 0 {
            eprintln!(
                "[DBG-ASSIGN] {} assign_calls={} apply_us_total={} apply_ms={}",
                module.name.as_str(),
                self.dbg_apply_n.get(),
                self.dbg_apply_us.get(),
                self.dbg_apply_us.get().saturating_div(1000)
            );
            self.dbg_apply_us.set(0);
            self.dbg_apply_n.set(0);
        }
        Ok(ir)
    }

    fn elaborate_module_with_params(
        &mut self,
        module: &Module,
        known_modules: &[Symbol],
        param_vals: &HashMap<Symbol, i64>,
    ) -> Result<IrModule, SimError> {
        self.elaborate_module_with_params_and_type(
            module,
            known_modules,
            param_vals,
            &HashMap::new(),
        )
    }

    fn elaborate_module_with_params_and_type(
        &mut self,
        module: &Module,
        known_modules: &[Symbol],
        param_vals: &HashMap<Symbol, i64>,
        type_param_overrides: &HashMap<Symbol, usize>,
    ) -> Result<IrModule, SimError> {
        // Konteks diagnostic module: SEMUA jalur (loop utama, flatten child
        // via flatten_instances_inner, interface sintetis) masuk lewat sini.
        // Sebelumnya hanya `elaborate_module` yang menyetel — flatten child
        // dipanggil LANGSUNG (tanpa elaborate_module) sehingga current_module
        // menyimpan module STALE dari pemanggilan sebelumnya → error port/
        // signal terlampir module yang salah (diag menyesatkan, mis. E3012
        // "port 'in_i'" atas nama interface JTAG_DV padahal port milik modul
        // lain yang sedang di-flatten).
        self.current_module = Some(module.name);
        use std::collections::HashSet;
        let dbg_step = std::env::var("DBG_ELAB_STEP").is_ok();
        let step_t0 = std::time::Instant::now();
        let step_ck = |name: &str, t0: &std::time::Instant| {
            if dbg_step {
                eprintln!(
                    "[DBG-STEP] {}: {} in {}us",
                    module.name.as_str(),
                    name,
                    t0.elapsed().as_micros()
                );
            }
        };
        let mut effective_params = param_vals.clone();

        // Process $unit parameters (top-level param declarations)
        for param in &self.design.unit_params {
            if !effective_params.contains_key(&param.name) {
                if let Some(expr) = &param.default {
                    if let Ok(val) = const_eval_with_params(expr, &effective_params) {
                        effective_params.insert(param.name, val);
                    }
                }
            }
        }

        // Process $unit imports (params) — iterasi langsung tanpa `collect`
        // (project besar: package bisa punya puluhan ribu item; mengumpulkan
        // Vec<&str> lalu get-per-key = alokasi + hash lookup berlebih per module).
        for (package, import_item) in &self.design.unit_imports {
            if let Some(pkg_items) = self.package_symbols.get(package) {
                let items: Box<dyn Iterator<Item = &PackageItem> + '_> =
                    if import_item.as_str() == "*" {
                        Box::new(pkg_items.values())
                    } else {
                        match pkg_items.get(import_item) {
                            Some(i) => Box::new(std::iter::once(i)),
                            None => Box::new(std::iter::empty()),
                        }
                    };
                for pkg_item in items {
                    if let PackageItem::Param(p) = pkg_item {
                        if !effective_params.contains_key(&p.name) {
                            if let Some(expr) = &p.default {
                                if let Ok(val) = const_eval_with_params(expr, &effective_params) {
                                    effective_params.insert(p.name, val);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Process package imports: add package params + typedefs, collect in-module typedefs
        if std::env::var("DBG_ELAB").is_ok() && module.name.as_str() == "cheriot_regs_reg_top" {
            let imports: Vec<String> = module
                .items
                .iter()
                .filter_map(|item| match item {
                    ModuleItem::Import { package, item } => {
                        Some(format!("{}::{}", package.as_str(), item.as_str()))
                    }
                    _ => None,
                })
                .collect();
            eprintln!(
                "[DBG-ELAB] cheriot_regs_reg_top imports={:?} pkg_in_symbols={} pkg_plain_len={}",
                imports,
                self.package_symbols
                    .contains_key(&Symbol::intern("cheriot_reg_pkg")),
                self.pkg_plain_params
                    .get(&Symbol::intern("cheriot_reg_pkg"))
                    .map(|m| m.len())
                    .unwrap_or(0)
            );
        }
        for item in &module.items {
            match item {
                ModuleItem::Import {
                    package,
                    item: import_item,
                } => {
                    if let Some(pkg_items) = self.package_symbols.get(package) {
                        // Iterasi langsung tanpa `collect` (lihat komentar di atas).
                        let items: Box<dyn Iterator<Item = &PackageItem> + '_> =
                            if import_item.as_str() == "*" {
                                Box::new(pkg_items.values())
                            } else {
                                match pkg_items.get(import_item) {
                                    Some(i) => Box::new(std::iter::once(i)),
                                    None => Box::new(std::iter::empty()),
                                }
                            };
                        let mut struct_imports: Vec<(Symbol, DataType)> = Vec::new();
                        for pkg_item in items {
                            {
                                match pkg_item {
                                    PackageItem::Param(p) => {
                                        if !effective_params.contains_key(&p.name) {
                                            if let Some(expr) = &p.default {
                                                if let Ok(val) =
                                                    const_eval_with_params(expr, &effective_params)
                                                {
                                                    effective_params.insert(p.name, val);
                                                } else {
                                                    // Default tidak bisa const-eval ke i64 (mis.
                                                    // 128-bit token yang overflow i64, seperti
                                                    // `RndCnstRawUnlockTokenHashed` di
                                                    // lc_ctrl_token_pkg) — daftarkan 0 agar
                                                    // plain name resolve dan desain tetap
                                                    // elaborate. Nilai rujukan (konstanta token)
                                                    // jarang dibandingkan dalam simulasi cone.
                                                    effective_params.insert(p.name, 0);
                                                }
                                                if std::env::var("DBG_ELAB").is_ok()
                                                    && p.name.as_str()
                                                        == "CHERIOT_ALERT_TEST_OFFSET"
                                                {
                                                    eprintln!(
                                                        "[DBG-ELAB] import param CHERIOT_ALERT_TEST_OFFSET inserted in module {}",
                                                        module.name.as_str()
                                                    );
                                                }
                                            }
                                        }
                                    }
                                    PackageItem::Typedef(td) => {
                                        if !self.typedef_map.contains_key(&td.name) {
                                            let width = self.resolve_typedef_width_dims(
                                                &td.dtype,
                                                td.range.as_ref(),
                                                &td.extra_packed_dims,
                                                &effective_params,
                                            );
                                            self.typedef_map.insert(td.name, width);
                                            self.typedef_dims.insert(
                                                td.name,
                                                (td.range.clone(), td.extra_packed_dims.clone()),
                                            );
                                        }
                                        if matches!(
                                            &td.dtype,
                                            DataType::StructType { .. }
                                                | DataType::UnionType { .. }
                                        ) {
                                            struct_imports.push((td.name, td.dtype.clone()));
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        for (name, dtype) in struct_imports {
                            self.store_typedef_fields(name, &dtype);
                        }
                    }
                }
                ModuleItem::Typedef(td) => {
                    let width = self.resolve_typedef_width_dims(
                        &td.dtype,
                        td.range.as_ref(),
                        &td.extra_packed_dims,
                        &effective_params,
                    );
                    self.typedef_map.insert(td.name, width);
                    self.typedef_dims
                        .insert(td.name, (td.range.clone(), td.extra_packed_dims.clone()));
                    if matches!(
                        &td.dtype,
                        DataType::StructType { .. } | DataType::UnionType { .. }
                    ) {
                        self.store_typedef_fields(td.name, &td.dtype);
                    }
                }
                // LANG-08: nettype — daftarkan nama sebagai tipe (lebar = base
                // type × range) agar `mynet x;` ter-resolve (UserDefined →
                // typedef_map).
                ModuleItem::Nettype(nt) => {
                    let width = self.resolve_typedef_width_dims(
                        &nt.base,
                        nt.range.as_ref(),
                        &[],
                        &effective_params,
                    );
                    self.typedef_map.insert(nt.name, width);
                    self.typedef_dims
                        .insert(nt.name, (nt.range.clone(), vec![]));
                }
                _ => {}
            }
        }
        // Merge konstanta package + import module ke effective_params SUDAH
        // dikerjakan di collect_package_param_ctx (jalur resolve_param_values):
        //   - pkg_param_ctx   sudah memuat flatten_consts_into_ctx SEKALI
        //     (line build_pkg_param_ctx) — qualified + key array `name[i]`.
        //   - unit_import_ctx sudah memuat flatten $unit imports (delta).
        //   - module.items import di-flatten di collect_package_param_ctx.
        // Blok re-flatten di sini MENGULANG format!("{}[{}]") + Symbol::intern
        // untuk ribuan elemen konstanta per-module (bottleneck: 43% memcmp +
        // 28% Symbol::as_str di OpenTitan — interning berulang). Dihapus.
        self.param_vals = effective_params.clone();
        // Signedness param module saat ini (IEEE §6.11): dipakai resolve
        // `Expr::Ident(parameter)` agar `localparam int X = -2` di-emit
        // sebagai IrExpr::Signed (bukan Const unsigned 64) — `X < 0` benar.
        // Juga dipakai kondisi generate-if (util/generate.rs).
        self.param_is_signed = self.module_param_signed_set(module);
        // Context package GLOBAL (qualified `pkg::name` + enum member) sudah
        // dijamin ada di effective_params: param_vals selalu berasal dari
        // resolve_param_values → collect_package_param_ctx yang meng-clone
        // pkg_param_ctx (yang meng-flatten konstanta package). Merge loop
        // 63k-entry per module adalah no-op (semua key sudah ada) dan dihapus
        // (bottleneck di desain besar).
        // Pre-pass: process $unit typedefs (top-level typedefs outside any module)
        let unit_typedefs = self.design.unit_typedefs.clone();
        for td in &unit_typedefs {
            let width = self.resolve_typedef_width_dims(
                &td.dtype,
                td.range.as_ref(),
                &td.extra_packed_dims,
                &effective_params,
            );
            self.typedef_map.insert(td.name, width);
            self.typedef_dims
                .insert(td.name, (td.range.clone(), td.extra_packed_dims.clone()));
            if matches!(
                &td.dtype,
                DataType::StructType { .. } | DataType::UnionType { .. }
            ) {
                self.store_typedef_fields(td.name, &td.dtype);
            }
        }
        // Pre-pass: process $unit imports for typedefs.
        // `typedef_map` adalah cache GLOBAL (keyed by nama typedef) — typedef
        // package tidak bergantung pada param module, jadi cukup di-resolve
        // SEKALI. Guard cache mencegah resolve-ulang ribuan typedef untuk tiap
        // module (sebelumnya O(modules × typedef_package) di project besar).
        for (package, import_item) in &self.design.unit_imports {
            if let Some(pkg_items) = self.package_symbols.get(package) {
                let items: Box<dyn Iterator<Item = &PackageItem> + '_> =
                    if import_item.as_str() == "*" {
                        Box::new(pkg_items.values())
                    } else {
                        match pkg_items.get(import_item) {
                            Some(i) => Box::new(std::iter::once(i)),
                            None => Box::new(std::iter::empty()),
                        }
                    };
                for pkg_item in items {
                    if let PackageItem::Typedef(td) = pkg_item {
                        if !self.typedef_map.contains_key(&td.name) {
                            let width = self.resolve_typedef_width_dims(
                                &td.dtype,
                                td.range.as_ref(),
                                &td.extra_packed_dims,
                                &effective_params,
                            );
                            self.typedef_map.insert(td.name, width);
                            self.typedef_dims
                                .insert(td.name, (td.range.clone(), td.extra_packed_dims.clone()));
                        }
                    }
                }
            }
        }
        // Pre-pass: store struct/union fields for $unit import typedefs
        let unit_imports = self.design.unit_imports.clone();
        let typedef_imports: Vec<(Symbol, DataType)> = unit_imports
            .iter()
            .filter_map(|(package, import_item)| {
                self.package_symbols.get(package).and_then(|pkg_items| {
                    let names: Vec<Symbol> = if import_item.as_str() == "*" {
                        pkg_items.keys().copied().collect()
                    } else {
                        vec![*import_item]
                    };
                    names.iter().find_map(|name| {
                        if let Some(PackageItem::Typedef(td)) = pkg_items.get(name) {
                            if matches!(
                                &td.dtype,
                                DataType::StructType { .. } | DataType::UnionType { .. }
                            ) {
                                Some((td.name, td.dtype.clone()))
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    })
                })
            })
            .collect();
        for (name, dtype) in &typedef_imports {
            self.store_typedef_fields(*name, dtype);
        }
        // Pre-pass: process package imports for typedefs before declaration processing
        // Pre-pass: process package imports for struct/union typedef fields
        let import_typedefs: Vec<(Symbol, DataType)> = module
            .items
            .iter()
            .filter_map(|item| {
                if let ModuleItem::Import {
                    package,
                    item: import_item,
                } = item
                {
                    self.package_symbols.get(package).and_then(|pkg_items| {
                        let names: Vec<Symbol> = if import_item.as_str() == "*" {
                            pkg_items.keys().copied().collect()
                        } else {
                            vec![*import_item]
                        };
                        names.iter().find_map(|name| {
                            let name_sym = *name;

                            if let Some(PackageItem::Typedef(td)) = pkg_items.get(&name_sym) {
                                if matches!(
                                    &td.dtype,
                                    DataType::StructType { .. } | DataType::UnionType { .. }
                                ) {
                                    Some((td.name, td.dtype.clone()))
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        })
                    })
                } else {
                    None
                }
            })
            .collect();
        for (name, dtype) in &import_typedefs {
            let fields = self.compute_struct_fields(dtype);
            if !fields.is_empty() {
                self.typedef_field_map.entry(*name).or_insert(fields);
            }
        }

        // ── Daftarkan lebar typedef ke konteks parameter ──
        // `$bits(typedef)` / `$size(typedef)` / scoped `pkg::typedef` dipakai
        // dalam konstanta (Replicate count, instance range, generate-if, cast
        // `type'(x)`). const_eval_with_params hanya melihat param_vals, jadi
        // nama typedef harus terdaftar di sini agar tidak jatuh ke
        // "'X' not found in parameter context" (E9001 tanpa lokasi).
        // Nama param asli menang (entry().or_insert) — typedef hanya mengisi
        // slot kosong. typedef_map adalah global lintas module; over-collection
        // aman (nama typedef jarang bentrok dgn nama param lokal).
        let typedef_width_regs: Vec<(Symbol, i64)> = self
            .typedef_map
            .iter()
            .map(|(name, &w)| (*name, w as i64))
            .collect();
        for (name, w) in &typedef_width_regs {
            effective_params.entry(*name).or_insert(*w);
        }
        // Qualified `pkg::typedef` juga didaftarkan (mis. `kmac_pkg::app_ses_config_t`
        // di otbn_kmac_if) — const_eval ScopedIdent mencoba qualified key dulu.
        // Kumpulkan nama dulu (borrow &self selesai) lalu insert ke effective_params.
        let mut qual_typedef_regs: Vec<(Symbol, i64)> = Vec::new();
        for (pkg, items) in &self.package_symbols {
            for (name, item) in items {
                if let PackageItem::Typedef(td) = item {
                    let width = self.typedef_map.get(&td.name).copied().unwrap_or(1) as i64;
                    qual_typedef_regs.push((
                        Symbol::intern(&format!("{}::{}", pkg.as_str(), name.as_str())),
                        width,
                    ));
                }
            }
        }
        for (qn, w) in &qual_typedef_regs {
            effective_params.entry(*qn).or_insert(*w);
        }

        // Resolve type parameter widths from module's param declarations and overrides
        let mut type_param_widths: HashMap<Symbol, usize> = HashMap::new();
        for param in &module.params {
            if param.is_type_param {
                let width = if let Some(w) = type_param_overrides.get(&param.name) {
                    *w
                } else if let Some((msb, lsb)) = &param.range {
                    // F32 fix: `T = logic [7:0]` — lebar dari range default type
                    // param (parser kini menyimpan range; sebelumnya dibuang →
                    // `T` selalu 1-bit). Range bisa memuat parameter lain
                    // (`logic [W-1:0]`) → resolve dgn effective_params.
                    match (
                        const_eval_params(msb, &effective_params),
                        const_eval_params(lsb, &effective_params),
                    ) {
                        (Ok(m), Ok(l)) => m.abs_diff(l) as usize + 1,
                        _ => param
                            .type_default
                            .as_ref()
                            .map(|td| td.width())
                            .unwrap_or(1),
                    }
                } else if let Some(td) = &param.type_default {
                    // `parameter type T = int` → 32-bit; `T = logic` → 1-bit;
                    // `T = byte` → 8-bit.
                    td.width()
                } else {
                    1
                };
                type_param_widths.insert(param.name, width);
            }
        }
        // Instal peta lebar type param utk modul ini (dibaca
        // `resolve_cast_name_width` saat elaborasi ekspresi `T'(x)`).
        // Pola yang sama seperti `current_module`: setiap elaborasi modul
        // menimpa miliknya sendiri sebelum memakai.
        self.cur_type_param_widths = type_param_widths.clone();

        let mut signals = Vec::new();
        let mut signal_map: HashMap<Symbol, SignalId> = HashMap::new();
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        let mut inouts = Vec::new();
        let mut processes = Vec::new();
        let mut sub_instances = Vec::new();
        let mut next_id = 0usize;
        // F40: initializer port ANSI (`output reg [7:0] b = 8'h2A`) —
        // dikumpulkan di port loop, di-emit sebagai Process::Initial di blok
        // decl init (proc_counter tersedia di sana).
        let mut port_inits: Vec<(Symbol, Expr)> = Vec::new();

        // Helper to get or create signal
        let get_or_create_signal = |name: Symbol,
                                    width: usize,
                                    kind: SignalKind,
                                    net_type: NetType,
                                    signals: &mut Vec<SignalInfo>,
                                    signal_map: &mut HashMap<Symbol, SignalId>,
                                    id: &mut SignalId,
                                    array_depth: usize,
                                    elem_width: usize,
                                    msb: usize,
                                    lsb: usize,
                                    is_2state: bool,
                                    is_signed: bool|
         -> SignalId {
            if let Some(&sid) = signal_map.get(&name) {
                sid
            } else {
                let sid = *id;
                *id += 1;
                signal_map.insert(name, sid);
                let init_val = match kind {
                    SignalKind::Wire | SignalKind::Inout => LogicVec::fill(LogicVal::Z, width),
                    _ => LogicVec::new(width),
                };
                signals.push(SignalInfo {
                    name,
                    width,
                    kind,
                    net_type,
                    multi_driver: false,
                    init_val,
                    array_depth,
                    elem_width,
                    array_dims: vec![],
                    class_name: None,
                    is_string: false,
                    is_mailbox: false,
                    is_semaphore: false,
                    is_real: false,
                    is_2state,
                    is_dynamic: false,
                    is_queue: false,
                    is_associative: false,
                    is_signed,
                    is_const: false,
                    msb,
                    lsb,
                    struct_fields: vec![],
                    packed_dims: vec![],
                    delay_rise: None,
                    delay_fall: None,
                    iface_type: None,
                    iface_modport: None,
                });
                sid
            }
        };

        // Process ports with parameter-aware width resolution
        for port in &module.ports {
            // Error lebar port yang gagal di-resolve (mis. `$bits(typedef)` di
            // range) di-lampiri nama port + lokasi fallback agar selalu punya
            // col & line (find_name_in_source mencari nama port di source).
            let port_name = port.name;
            let pwa = |port: &Port,
                       ep: &HashMap<Symbol, i64>,
                       sm: &HashMap<Symbol, SignalId>,
                       sg: &[SignalInfo]| {
                self.port_width_aware(port, ep, sm, sg).or_else(|e| {
                    // Width port gagal di-resolve (mis. member access struct
                    // `otp_lc_data_i.secrets_valid`, param package hilang,
                    // virtual interface) → fallback lebar 1 + warning agar
                    // modul tetap elaborate (bukan skip berantai). Port
                    // bertipe interface tetap 64 (handle).
                    let fb = if port.dtype_name.is_some() {
                        let base = port
                            .dtype_name
                            .map(|tn| tn.as_str().split('.').next().unwrap_or(""))
                            .unwrap_or("");
                        if self
                            .design
                            .interfaces
                            .iter()
                            .any(|i| i.name.as_str() == base)
                        {
                            64
                        } else {
                            1
                        }
                    } else {
                        1
                    };
                    self.elab_warn_at(
                        DiagCode::SimulationError,
                        format!(
                            "width of port '{}' cannot be resolved ({}) — fallback lebar {}",
                            port_name.as_str(),
                            e,
                            fb
                        ),
                        0,
                        0,
                    );
                    Ok::<usize, String>(fb)
                })
            };
            // F27: port bertipe interface (`bus_if b` / `bus_if.m b`) — port
            // handle 64-bit (pola virtual interface). Field interface diakses
            // via hier_signal_map (`b.clk` → HierRef) setelah flatten, jadi
            // lebar port hanya dipakai type-check lebar di flatten_instances.
            let iface_of_port = port.dtype_name.and_then(|tn| {
                let base = tn.as_str().split('.').next().unwrap_or(tn.as_str());
                self.design
                    .interfaces
                    .iter()
                    .any(|i| i.name.as_str() == base)
                    .then_some(tn)
            });
            let width = if iface_of_port.is_some() {
                64
            } else if let Some(tn) = &port.dtype_name {
                if let Some(tw) = type_param_widths.get(tn) {
                    if port.expr_range.is_some() || port.range.is_some() {
                        pwa(port, &effective_params, &signal_map, &signals)?
                    } else {
                        *tw
                    }
                } else {
                    pwa(port, &effective_params, &signal_map, &signals)?
                }
            } else {
                pwa(port, &effective_params, &signal_map, &signals)?
            };
            self.check_packed_width(width, &format!("port '{}'", port.name.as_str()))?;
            let kind = match port.direction {
                PortDirection::Input => SignalKind::Input,
                PortDirection::Output => SignalKind::Output,
                PortDirection::Inout => SignalKind::Inout,
                PortDirection::Ref => SignalKind::Inout,
            };
            let (p_msb, p_lsb) = if !port.extra_packed_dims.is_empty() {
                // Packed multi-dimensi: signal flat mewakili SELURUH array → [width-1:0].
                (width.saturating_sub(1), 0)
            } else if let Some(r) = &port.range {
                (r.msb, r.lsb)
            } else if let Some(er) = &port.expr_range {
                if let Ok(r) = resolve_expr_range(er, &effective_params) {
                    (r.msb, r.lsb)
                } else {
                    (width - 1, 0)
                }
            } else {
                (width - 1, 0)
            };
            let net_type = match port.direction {
                PortDirection::Inout => NetType::Tri,
                _ => NetType::Wire,
            };
            // Unpacked array port: lipat dimensi array ke lebar total
            // (F39: multi-dimensi `[a][b]` didukung — array_dims penuh).
            let dims = |ar: &Range| {
                if ar.msb >= ar.lsb {
                    ar.msb - ar.lsb + 1
                } else {
                    ar.lsb - ar.msb + 1
                }
            };
            let mut port_all_dims: Vec<usize> = Vec::new();
            if let Some(ar) = &port.array_range {
                port_all_dims.push(dims(ar));
                for (ext_r, ext_sz) in &port.extra_unpacked_dims {
                    if let Some(r) = ext_r {
                        port_all_dims.push(dims(r));
                    } else if let Some(sz) = ext_sz {
                        match const_eval_params(sz, &effective_params) {
                            Ok(n) if n > 0 => port_all_dims.push(n as usize),
                            _ => {}
                        }
                    }
                }
            }
            let (array_depth, total_width, port_msb, port_lsb) = if !port_all_dims.is_empty() {
                let total_depth: usize = port_all_dims.iter().product();
                (total_depth, width * total_depth, width * total_depth - 1, 0)
            } else {
                (1, width, p_msb, p_lsb)
            };
            let sid = get_or_create_signal(
                port.name,
                total_width,
                kind.clone(),
                net_type,
                &mut signals,
                &mut signal_map,
                &mut next_id,
                array_depth,
                width,
                port_msb,
                port_lsb,
                false,
                false,
            );
            match port.direction {
                PortDirection::Input => inputs.push(sid),
                PortDirection::Output => outputs.push(sid),
                PortDirection::Inout => inouts.push(sid),
                PortDirection::Ref => inouts.push(sid),
            }
            // F40: initializer port ANSI — SV legal, dulu di-drop parser.
            if let Some(init_expr) = &port.init_expr {
                port_inits.push((port.name, init_expr.clone()));
            }
            // Struct-typed port: isi struct_fields agar member access
            // (`hw2reg.val.d = ...`) bisa di-resolve sebagai lvalue. dtype_name
            // bisa berformat `pkg::type` (scoped) atau nama typedef biasa.
            if let Some(tn) = &port.dtype_name {
                if let Some(fields) = self.lookup_struct_fields(tn.as_str()) {
                    if !fields.is_empty() {
                        let sig = &mut signals[sid];
                        sig.struct_fields = fields;
                    }
                }
            }
            // F27: tandai port interface (iface_type) agar member access
            // (`b.clk`) dikompil sebagai HierRef dan flatten mengenali port
            // ini sebagai handle interface (bukan signal lebar 64 nyata).
            if let Some(tn) = &iface_of_port {
                let sig = &mut signals[sid];
                sig.iface_type = Some(*tn);
                sig.is_2state = true;
                // Modport bagian dari nama (`axi_lite.dut`) — simpan sebagai
                // metadata arah (belum dipakai runtime, hanya untuk info).
                if let Some((_, mp)) = tn.as_str().split_once('.') {
                    sig.iface_modport = Some(Symbol::intern(mp));
                }
            }
            // Set array_dims untuk port array multi-dimensi (F39) — indexing
            // `mat[i][j]` & fold di elaborator memakai array_dims.
            if !port_all_dims.is_empty() {
                let sig = &mut signals[sid];
                sig.array_dims = port_all_dims.clone();
            }
            // Set packed_dims untuk port packed multi-dimensi (`[a:b][c:d] name`)
            // agar akses elemen `name[k]` benar (via RangeSelect).
            if !port.extra_packed_dims.is_empty() {
                let mut pd = Vec::with_capacity(port.extra_packed_dims.len() + 1);
                let base_w = if let Some(r) = &port.range {
                    r.width()
                } else if let Some(er) = &port.expr_range {
                    resolve_expr_range(er, &effective_params)
                        .map(|r| r.width())
                        .unwrap_or(1)
                } else {
                    1
                };
                pd.push(base_w);
                for er in &port.extra_packed_dims {
                    if let Ok(r) = resolve_expr_range(er, &effective_params) {
                        pd.push(r.width());
                    }
                }
                let sig = &mut signals[sid];
                sig.packed_dims = pd;
            }
        }

        step_ck("after ports", &step_t0);

        // ── Generate expansion (SEBELUM pemrosesan deklarasi) ──
        // Expand generate blocks in module items — dilakukan lebih awal agar
        // deklarasi yang dihasilkan branch generate (mis. `logic wptr_err;`
        // saat Secure=1) ikut terdaftar sebagai sinyal. Tanpa ini, sinyal
        // seperti itu hilang → error "signal not found" (E2001).
        // Typedef lokal module harus terlihat oleh resolusi lebar `$bits(t)` /
        // cast `t'(...)` di param & width context (mis. struct `lfsr_data_t` /
        // `cpu_ctrl_sts_part_t` lokal di ibex). Daftarkan sebagai pseudo-package
        // `__local_typedefs::<module>` di package_symbols; dibersihkan tiap
        // module agar typedef module lain tidak bocor. resolve_typedef_ident_width
        // memberi prioritas package asli, jadi ini hanya fallback.
        self.package_symbols
            .retain(|k, _| !k.as_str().starts_with("__local_typedefs::"));
        {
            let mut mod_typedefs: HashMap<Symbol, PackageItem> = HashMap::new();
            for item in &module.items {
                if let ModuleItem::Typedef(td) = item {
                    mod_typedefs
                        .entry(td.name)
                        .or_insert_with(|| PackageItem::Typedef(td.clone()));
                }
            }
            if !mod_typedefs.is_empty() {
                let pseudo = Symbol::intern(&format!("__local_typedefs::{}", module.name.as_str()));
                self.package_symbols.insert(pseudo, mod_typedefs);
            }
        }
        // Collect body-level params (localparam, parameter) into effective_params.
        // Parser menaruh localparam body (`localparam int W = ...`) di
        // `module.params` (mirip header params), jadi iterasi keduanya.
        let mut body_param_defaults: Vec<(Symbol, &Expr)> = Vec::new();
        for p in &module.params {
            if let Some(e) = &p.default {
                body_param_defaults.push((p.name, e));
            }
        }
        for item in &module.items {
            if let ModuleItem::Param(p) = item {
                if let Some(e) = &p.default {
                    body_param_defaults.push((p.name, e));
                }
            }
        }
        // Evaluasi body localparam dengan evaluator PENUH (member access
        // struct, `$bits(typedef)`, fungsi package) + fixed-point agar chain
        // `Info.size → PayloadByte → PayloadPtrW` (pola spi_device /
        // prim_generic_flash_bank OpenTitan) ter-resolve. Struct localparam
        // disimpan terpisah (`struct_vals`) dan diteruskan ke evaluator agar
        // `name.field` bisa di-const-eval. `merged` = konstanta package
        // (qualified) + effective_params (plain, module menang).
        let mut struct_vals: HashMap<Symbol, Vec<SField>> = HashMap::new();
        let mut struct_lit_done: HashSet<Symbol> = HashSet::new();
        // Default param berupa ARRAY multi-elemen (`localparam logic [7:0]
        // L[4] = '{...}`) — JANGAN didaftarkan sebagai skalar (dulu
        // eval_param_default_full mengambil elemen-0 via `.first()` → param_vals
        // ["L"]=0xAA menaungi signal const-array sehingga `L[i]` dinamis
        // me-resolve Const(elemen-0) lalu bit-select → 0; probe u04/u08).
        // Array ditangani blok "Localparam ARRAY" (materialisasi signal +
        // key ter-flatten `L[i]`). `array_done` mencegah fixed-point loop
        // mengulang tanpa henti.
        let mut array_done: HashSet<Symbol> = HashSet::new();
        // Struct literal yang SUDAH berhasil diproses (masuk struct_vals ATAU
        // effective_params) — mencegah loop tak berujung: tanpa penanda ini,
        // struct literal selalu memenuhi kondisi proses tiap iterasi (skip
        // condition di-skip untuk struct) sehingga `changed` selalu true dan
        // `loop` tidak pernah break (timeout 600s pada full run).
        // Konteks skalar = effective_params itu sendiri: ia sudah ⊇
        // pkg_const_scalars (param_vals dari resolve_param_values meng-clone
        // pkg_param_ctx yang meng-flatten konstanta package), jadi map
        // `merged` terpisah (clone pkg_const_scalars + merge effective_params
        // per module) redundant dan dihapus (bottleneck di desain besar).
        loop {
            let mut changed = false;
            for (pname, expr) in &body_param_defaults {
                // Struct literal TETAP diproses meski sudah ada di
                // effective_params sebagai skalar fallback 0 (hasil
                // resolve_param_values yang tak paham struct) — kalau di-skip,
                // struct_vals tidak pernah terisi dan `Info.size`/
                // `Info.zeroizable` gagal const-eval (otp_ctrl_part_buf).
                // Namun jika sudah berhasil diproses (struct_lit_done /
                // struct_vals), jangan proses ulang agar loop berhenti.
                let is_struct_lit = matches!(expr, Expr::StructLit { .. });
                // Default berbentuk ident yang mereferensikan konstanta struct
                // package (`parameter part_info_t Info = PartInfoDefault`,
                // `PartInfoDefault` = parameter struct di otp_ctrl_part_pkg
                // dengan fields ter-flatten di pkg_const_scalars sbg key
                // `pkg::PartInfoDefault.<field>`). resolve_param_values
                // menjadikannya skalar 0 (struct tak paham skalar) sehingga
                // `Info.size`/`Info.zeroizable` gagal — perlakukan seperti
                // struct literal agar struct_vals terisi.
                let is_struct_ref = !is_struct_lit
                    && matches!(expr, Expr::Ident { .. } | Expr::ScopedIdent { .. })
                    && ident_refs_pkg_struct(expr, &self.pkg_struct_ref_index);
                let is_struct_like = is_struct_lit || is_struct_ref;
                let already_done = if is_struct_like {
                    struct_lit_done.contains(pname) || struct_vals.contains_key(pname)
                } else {
                    effective_params.contains_key(pname) || struct_vals.contains_key(pname)
                };
                if already_done || array_done.contains(pname) {
                    continue;
                }
                // 0.5) Struct via referensi konstanta struct package
                //     (`Info = PartInfoDefault`) — fields dari index package.
                if is_struct_ref {
                    if let Some(fields) =
                        pkg_struct_fields_for_ref(expr, &self.pkg_struct_ref_index)
                    {
                        struct_vals.insert(*pname, fields);
                        struct_lit_done.insert(*pname);
                        changed = true;
                        continue;
                    }
                }
                // Guard ARRAY: default array multi-elemen → jangan insert skalar
                // (lihat array_done). Ditangani blok materialisasi const-array.
                if let Some(CVal::Array(ref a)) = eval_cval_full(
                    expr,
                    &effective_params,
                    &self.pkg_const_arrays,
                    &self.package_symbols,
                    &struct_vals,
                ) {
                    if a.len() > 1 {
                        array_done.insert(*pname);
                        changed = true;
                        continue;
                    }
                }
                // 1) Skalar via evaluator penuh ($bits(typedef), fungsi package,
                //    member access struct package).
                if let Some(val) = eval_param_default_full(
                    expr,
                    &effective_params,
                    &self.pkg_const_arrays,
                    &self.package_symbols,
                    &struct_vals,
                ) {
                    effective_params.insert(*pname, val);
                    if is_struct_lit {
                        struct_lit_done.insert(*pname);
                    }
                    changed = true;
                    continue;
                }
                // 2) Struct via evaluator penuh — simpan untuk member access.
                if let Some(CVal::Struct(fields)) = eval_cval_full(
                    expr,
                    &effective_params,
                    &self.pkg_const_arrays,
                    &self.package_symbols,
                    &struct_vals,
                ) {
                    struct_vals.insert(*pname, fields);
                    struct_lit_done.insert(*pname);
                    changed = true;
                    continue;
                }
                // 3) Jalur cepat skalar lama (literal murni, operator dasar).
                if let Ok(val) = const_eval_with_params(expr, &effective_params) {
                    effective_params.insert(*pname, val);
                    if is_struct_lit {
                        struct_lit_done.insert(*pname);
                    }
                    changed = true;
                    continue;
                }
                // 4) Width-aware evaluator (fallback historis).
                if let Some(val) = super::util::width::eval_width_aware_param(
                    expr,
                    &signal_map,
                    &signals,
                    &effective_params,
                    &self.package_symbols,
                ) {
                    effective_params.insert(*pname, val);
                    if is_struct_lit {
                        struct_lit_done.insert(*pname);
                    }
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        // Flatten struct localparam/parameter (`struct_vals`) ke key `name.field`
        // di effective_params — agar member access struct dalam konteks konstanta
        // (`Info.size*8-1:0` di lebar port, `Info.offset` di localparam, kondisi
        // generate-if `Info.zeroizable` — pola otp_ctrl_part_buf/
        // otp_ctrl_part_unbuf) bisa di-const-eval oleh const_eval_with_params,
        // bukan hanya evaluator penuh yang membawa struct_vals.
        for (base, fields) in &struct_vals {
            let mut stack: Vec<(String, &CVal)> = Vec::new();
            for f in fields {
                if let Some(fname) = f.name {
                    stack.push((format!("{}.{}", base.as_str(), fname.as_str()), &f.val));
                }
            }
            while let Some((path, val)) = stack.pop() {
                match val {
                    CVal::Scalar(v) => {
                        let key = Symbol::intern(&path);
                        effective_params.entry(key).or_insert(*v);
                    }
                    CVal::Array(elems) => {
                        for (i, e) in elems.iter().enumerate() {
                            stack.push((format!("{}[{}]", path, i), e));
                        }
                    }
                    CVal::Struct(fs) => {
                        for f in fs {
                            if let Some(fname) = f.name {
                                stack.push((format!("{}.{}", path, fname.as_str()), &f.val));
                            }
                        }
                    }
                }
            }
        }
        // Fallback global: localparam/parameter body yang GAGAL dievaluasi
        // (default `'x`, `$bits(member)` tak ter-resolve, referensi package
        // DPI yang tidak ada — mis. `CYCLES_PER_SYMBOL = FREQ/BAUD` di uartdpi
        // dengan FREQ/BAUD = 'x) tetap didaftarkan dengan nilai 1 agar
        // referensi nama TIDAK menjadi "signal not found" (E2001). Array
        // (Concat multi-elemen) dan struct literal dikecualikan — keduanya
        // didaftarkan lewat jalur array/struct di atas.
        for (pname, expr) in &body_param_defaults {
            if effective_params.contains_key(pname) || struct_vals.contains_key(pname) {
                continue;
            }
            if matches!(expr, Expr::Concat(parts) if parts.len() > 1) {
                continue;
            }
            if matches!(expr, Expr::StructLit { .. }) {
                continue;
            }
            self.elab_warn_at(
                DiagCode::ModuleNotFound,
                format!(
                    "localparam '{}' gagal dievaluasi — fallback nilai 1 (referensi tidak dapat di-const-eval)",
                    pname.as_str()
                ),
                0,
                0,
            );
            effective_params.insert(*pname, 1);
        }
        // Daftarkan nama struct localparam/parameter (yang tersimpan di
        // `struct_vals`) ke `effective_params` dengan nilai placeholder 0.
        // Ini membuat referensi nama UTUH (mis. port connection
        // `.racl_policy_sel_ranges_i (RaclPolicySelRangesEgressbuffer)`) bisa
        // di-resolve sebagai konstanta — tanpa ini nama tersebut "signal not
        // found" (E2001). Member access (`name.field`) tetap benar karena
        // evaluator penuh (eval_cval_full/eval_param_default_full) memakai
        // `struct_vals` yang sudah terisi fixed-point di atas.
        for sname in struct_vals.keys() {
            if !effective_params.contains_key(sname) {
                effective_params.insert(*sname, 0);
            }
        }
        self.param_vals = effective_params.clone();

        let expanded_items: Vec<ModuleItem> = {
            let mut items = Vec::new();
            for item in &module.items {
                match item {
                    ModuleItem::Generate(gen) => {
                        let expanded = match expand_generate_block(
                            gen,
                            &effective_params,
                            &self.param_is_signed,
                            &self.diag_sink,
                            &self.source_lines,
                            &self.source_file,
                            "",
                        ) {
                            Ok(v) => v,
                            Err(e) => {
                                // Sama seperti design-level pass
                                // (expand_all_generates): blok generate yang
                                // gagal diekspansi (limit for merujuk param
                                // yang tak bisa di-const-eval) dilewati
                                // dengan warning, bukan mematikan seluruh
                                // modul. Modul tetap elaborate tanpa blok
                                // ini — perilaku degrade konsisten global.
                                self.elab_warn_at(
                                    DiagCode::InvalidSyntax,
                                    format!(
                                        "generate block expansion skipped in '{}': {}",
                                        module.name.as_str(),
                                        e.msg
                                    ),
                                    e.line,
                                    e.col,
                                );
                                Vec::new()
                            }
                        };
                        // Collect params from expanded generate items too
                        for ei in &expanded {
                            if let ModuleItem::Param(p) = ei {
                                if !effective_params.contains_key(&p.name) {
                                    if let Some(expr) = &p.default {
                                        if let Ok(val) =
                                            const_eval_with_params(expr, &effective_params)
                                        {
                                            effective_params.insert(p.name, val);
                                        }
                                    }
                                }
                            }
                        }
                        items.extend(expanded);
                    }
                    other => items.push(other.clone()),
                }
            }
            items
        };

        // Update param_vals after generate expansion which may add body-level params
        self.param_vals = effective_params.clone();

        // LANG-40: kumpulkan `let` declaration module ini (items + hasil
        // ekspansi generate) — di-resolve elaborate_expr sebagai alias
        // ekspresi (IEEE 1800-2017 §11.12.2). Scope per-module: setiap module
        // menimpa map.
        let mut let_decls: HashMap<Symbol, LetDecl> = HashMap::new();
        for item in &module.items {
            if let ModuleItem::Let(ld) = item {
                let_decls.insert(ld.name, ld.clone());
            }
        }
        for item in &expanded_items {
            if let ModuleItem::Let(ld) = item {
                let_decls.entry(ld.name).or_insert_with(|| ld.clone());
            }
        }
        self.let_decls = let_decls;

        // Enum member constants dari typedef di DALAM blok generate (hasil
        // ekspansi) — mis. `typedef enum {...PmEnLastPos} rv_dm_pm_en_e;` di
        // `else begin : gen_jtag_gating` (rv_dm.sv). Member dipakai sebagai
        // range array (`[PmEnLastPos-1:0]`) dan index. collect_package_param_ctx
        // hanya melihat module.items (pra-ekspansi), jadi member di dalam
        // generate belum terdaftar → decl array gagal resolve range → sinyal
        // "pinmux_hw_debug_en not found". Registrasi fixed-point di sini
        // (bisa saling referensi + referensi param lain).
        for _ in 0..64 {
            let mut changed = false;
            // Enum member dari: (1) typedef di items (+ hasil generate),
            // (2) enum INLINE di deklarasi signal (`enum logic [2:0]{RAM,
            // DEBUG, ROM, UNMAP, IDLE_READ} select_rdata_d;` — mm_ram.sv).
            // Parser menaruh deklarasi signal di `module.decls` DAN
            // `module.items`, jadi dua-duanya di-scan. Member dipakai sebagai
            // nilai di statement (`select_rdata_d = IDLE_READ`) dan range;
            // tanpa registrasi → "signal 'IDLE_READ' not found".
            let mut typedefs: Vec<Vec<(Symbol, Option<Expr>)>> = Vec::new();
            for item in module.items.iter().chain(expanded_items.iter()) {
                if let ModuleItem::Typedef(td) = item {
                    if let DataType::EnumType { members, .. } = &td.dtype {
                        typedefs.push(members.clone());
                    }
                } else if let ModuleItem::Decl(d) = item {
                    if let DataType::EnumType { members, .. } = &d.dtype {
                        typedefs.push(members.clone());
                    }
                }
            }
            for d in &module.decls {
                if let DataType::EnumType { members, .. } = &d.dtype {
                    typedefs.push(members.clone());
                }
            }
            for members in typedefs {
                // GAGAL eval → lewati (jangan tebak): retry iterasi berikut.
                let mut last: Option<i64> = Some(0);
                for (member_name, member_expr) in members {
                    let val = match member_expr {
                        Some(expr) => Self::eval_enum_member_value(&expr, &effective_params, None),
                        None => last,
                    };
                    if let Some(v) = val {
                        if let std::collections::hash_map::Entry::Vacant(e) =
                            effective_params.entry(member_name)
                        {
                            e.insert(v);
                            changed = true;
                        }
                    }
                    last = val.map(|v| v.wrapping_add(1));
                }
            }
            if !changed {
                break;
            }
        }
        // Verifikasi pasca-fixpoint (lihat build_pkg_param_ctx): nilai enum
        // lokal yang tak ter-evaluasi dilaporkan, bukan ditebak.
        {
            let mut verify_typedefs: Vec<Vec<(Symbol, Option<Expr>)>> = Vec::new();
            for item in module.items.iter().chain(expanded_items.iter()) {
                if let ModuleItem::Typedef(td) = item {
                    if let DataType::EnumType { members, .. } = &td.dtype {
                        verify_typedefs.push(members.clone());
                    }
                } else if let ModuleItem::Decl(d) = item {
                    if let DataType::EnumType { members, .. } = &d.dtype {
                        verify_typedefs.push(members.clone());
                    }
                }
            }
            for d in &module.decls {
                if let DataType::EnumType { members, .. } = &d.dtype {
                    verify_typedefs.push(members.clone());
                }
            }
            self.verify_enum_members(
                &verify_typedefs,
                &format!("module '{}'", module.name.as_str()),
                &effective_params,
                None,
            );
        }
        self.param_vals = effective_params.clone();

        if std::env::var("DBG_DECL2").is_ok() {
            eprintln!("DBG-DECL2 enter module={} decls={} items={}", module.name.as_str(), module.decls.len(), module.items.len());
            for it in &module.items {
                if let mivon_ast::ModuleItem::Decl(d) = it {
                    for v in &d.names {
                        eprintln!("DBG-DECL2 item-decl {} ar={:?}", v.name.as_str(), v.array_range);
                    }
                } else {
                    eprintln!("DBG-DECL2 item other");
                }
            }
        }
        if std::env::var("DBG_DECL").is_ok() {
            for d in &module.decls {
                for v in &d.names {
                    eprintln!("DBG-DECL module-decl {} dtype={:?} ar={:?}", v.name.as_str(), d.dtype, v.array_range);
                }
            }
        }
        // Gabungkan deklarasi module-level dengan deklarasi hasil generate.
        // Parser menaruh deklarasi normal di module.decls DAN module.items,
        // jadi dedup by nama (get_or_create_signal juga idempoten).
        let mut seen_names: std::collections::HashSet<Symbol> = std::collections::HashSet::new();
        let mut all_decls: Vec<Decl> = Vec::new();
        for decl in &module.decls {
            let mut new_vars = Vec::new();
            for var in &decl.names {
                if seen_names.insert(var.name) {
                    new_vars.push(var.clone());
                }
            }
            if !new_vars.is_empty() {
                all_decls.push(Decl {
                    dtype: decl.dtype.clone(),
                    kind: decl.kind.clone(),
                    names: new_vars,
                });
            }
        }
        for item in &expanded_items {
            if let ModuleItem::Decl(d) = item {
                let mut new_vars = Vec::new();
                for var in &d.names {
                    if seen_names.insert(var.name) {
                        new_vars.push(var.clone());
                    }
                }
                if !new_vars.is_empty() {
                    all_decls.push(Decl {
                        dtype: d.dtype.clone(),
                        kind: d.kind.clone(),
                        names: new_vars,
                    });
                }
            }
        }

        // Process class declarations inside module (ModuleItem::Class)
        for item in &expanded_items {
            if let ModuleItem::Class(cd) = item {
                // Add class to design classes for elaboration
                self.design.classes.push(cd.clone());
            }
        }

        // Kumpulkan deklarasi procedural lokal (`int index_x1;` di dalam
        // always/initial block). Parser menyimpannya sebagai `Stmt::NamedBlock`
        // dengan `decls`; variabel lokal ini harus terdaftar di signal_map agar
        // referensi di dalam loop yang di-unroll bisa di-resolve (sebelumnya
        // parser membuangnya jadi `Stmt::Null` → "signal 'x' not found").
        let mut procedural_decls: Vec<Decl> = Vec::new();
        for item in &expanded_items {
            let stmts: &[Stmt] = match item {
                ModuleItem::Always(a) => &a.stmts,
                ModuleItem::Initial(i) => &i.stmts,
                ModuleItem::Final(f) => &f.stmts,
                _ => continue,
            };
            collect_procedural_decls(stmts, &mut procedural_decls);
        }
        for d in procedural_decls {
            let mut new_vars = Vec::new();
            for var in &d.names {
                if seen_names.insert(var.name) {
                    new_vars.push(var.clone());
                }
            }
            if !new_vars.is_empty() {
                all_decls.push(Decl {
                    dtype: d.dtype.clone(),
                    kind: d.kind.clone(),
                    names: new_vars,
                });
            }
        }
        // Procedural LOCALPARAM block-scoped (`localparam logic [4:0] X =
        // const_expr;` di dalam for/begin block — otp_ctrl_scrmbl) didaftarkan
        // sebagai signal oleh loop di atas, TAPI nilainya juga wajib masuk
        // param context agar referensi `X` di statement ter-fold jadi
        // konstanta (elaborator/expr.rs fold ident yang ada di param_vals) —
        // kalau hanya signal, nilai tetap X. Const-eval fixed-point agar
        // localparam bisa saling mereferensikan.
        for _ in 0..32 {
            let mut changed = false;
            for d in &all_decls {
                for var in &d.names {
                    if effective_params.contains_key(&var.name) {
                        continue;
                    }
                    if let Some(expr) = &var.expr {
                        if let Ok(v) = const_eval_with_params(expr, &effective_params) {
                            effective_params.insert(var.name, v);
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }

        step_ck("after generate+decls", &step_t0);

        // Process declarations with parameter-aware width resolution
        for decl in &all_decls {
            let class_name = match &decl.dtype {
                DataType::UserDefined(cn) if cn.as_str() == "process" => {
                    Some("__process".to_string())
                }
                DataType::UserDefined(cn) => Some(cn.as_str().to_string()),
                _ => None,
            };
            let decl_is_2state = is_2state_type(&decl.dtype);
            for var in &decl.names {
                let is_real = decl.dtype == DataType::Real || decl.dtype == DataType::Realtime;
                if is_real || decl.dtype == DataType::String {
                    let sid = next_id;
                    next_id += 1;
                    signal_map.insert(var.name, sid);
                    signals.push(SignalInfo {
                        name: var.name,
                        width: if is_real { 64 } else { 0 },
                        kind: SignalKind::Reg,
                        net_type: NetType::Wire,
                        multi_driver: false,
                        init_val: LogicVec::new(if is_real { 64 } else { 0 }),
                        array_depth: 1,
                        elem_width: if is_real { 64 } else { 0 },
                        array_dims: vec![],
                        class_name: None,
                        is_string: decl.dtype == DataType::String,
                        is_mailbox: false,
                        is_semaphore: false,
                        is_real,
                        is_2state: false,
                        is_dynamic: false,
                        is_queue: false,
                        is_associative: false,
                        is_signed: false,
                        is_const: false,
                        msb: if is_real { 63 } else { 0 },
                        lsb: 0,
                        struct_fields: vec![],
                        packed_dims: vec![],
                        delay_rise: None,
                        delay_fall: None,
                        iface_type: None,
                        iface_modport: None,
                    });
                    continue;
                }
                if var.is_dynamic || var.is_queue {
                    let dtype_width = self.resolve_dtype_width(&decl.dtype, &type_param_widths)?;
                    let elem_width = dtype_width
                        .max(
                            self.var_resolved_width_aware(
                                var,
                                &effective_params,
                                &signal_map,
                                &signals,
                            )
                            .map_err(|e| self.elab_diag(DiagCode::ParamMismatch, e))?,
                        )
                        .max(decl.kind.default_width());
                    let sid = next_id;
                    next_id += 1;
                    signal_map.insert(var.name, sid);
                    signals.push(SignalInfo {
                        name: var.name,
                        width: 0,
                        kind: SignalKind::Reg,
                        net_type: NetType::Wire,
                        multi_driver: false,
                        init_val: LogicVec::new(0),
                        array_depth: 0,
                        elem_width,
                        array_dims: vec![],
                        class_name: None,
                        is_string: false,
                        is_mailbox: false,
                        is_semaphore: false,
                        is_real: false,
                        is_2state: decl_is_2state,
                        is_dynamic: var.is_dynamic,
                        is_queue: var.is_queue,
                        is_associative: var.is_associative,
                        is_signed: is_signed_type(&decl.dtype),
                        is_const: false,
                        msb: 0,
                        lsb: 0,
                        struct_fields: vec![],
                        packed_dims: vec![],
                        delay_rise: None,
                        delay_fall: None,
                        iface_type: None,
                        iface_modport: None,
                    });
                }
                let dtype_width = self.resolve_dtype_width(&decl.dtype, &type_param_widths)?;
                let elem_width = dtype_width
                    .max(
                        self.var_resolved_width_aware(
                            var,
                            &effective_params,
                            &signal_map,
                            &signals,
                        )
                        .map_err(|e| self.elab_diag(DiagCode::ParamMismatch, e))?,
                    )
                    .max(decl.kind.default_width());
                self.check_packed_width(elem_width, &format!("signal '{}'", var.name.as_str()))?;
                let (kind, net_type) = match decl.kind {
                    DeclKind::Wire => (SignalKind::Wire, NetType::Wire),
                    DeclKind::Wand => (SignalKind::Wire, NetType::Wand),
                    DeclKind::Wor => (SignalKind::Wire, NetType::Wor),
                    DeclKind::Tri => (SignalKind::Wire, NetType::Tri),
                    DeclKind::Tri0 => (SignalKind::Wire, NetType::Tri0),
                    DeclKind::Tri1 => (SignalKind::Wire, NetType::Tri1),
                    DeclKind::TriAnd => (SignalKind::Wire, NetType::TriAnd),
                    DeclKind::TriOr => (SignalKind::Wire, NetType::TriOr),
                    DeclKind::Supply0 => (SignalKind::Wire, NetType::Supply0),
                    DeclKind::Supply1 => (SignalKind::Wire, NetType::Supply1),
                    DeclKind::Reg | DeclKind::Logic | DeclKind::Int | DeclKind::Integer => {
                        (SignalKind::Reg, NetType::Wire)
                    }
                };
                let (d_msb, d_lsb) = if let Some(r) = &var.range {
                    (r.msb, r.lsb)
                } else if let Some(er) = &var.expr_range {
                    if let Ok(r) = resolve_expr_range(er, &effective_params) {
                        (r.msb, r.lsb)
                    } else {
                        (elem_width - 1, 0)
                    }
                } else {
                    (elem_width - 1, 0)
                };
                // Unpacked array dimension: prioritas `array_range` (sudah
                // di-resolve saat parse); fallback ke `array_size_expr` yang
                // bisa memuat parameter (`logic [N-1:0] arr [Width]`).
                let resolved_arr = var.array_range.clone().or_else(|| {
                    var.array_size_expr.as_ref().and_then(|sz| {
                        const_eval_params(sz, &effective_params)
                            .ok()
                            .filter(|n| *n > 0)
                            .map(|n| Range {
                                msb: (n - 1) as usize,
                                lsb: 0,
                            })
                    })
                });
                if let Some(ar) = &resolved_arr {
                    // F39: dimensi unpacked SEMUA (pertama + lanjutan `[a][b]`)
                    // → array_dims [d0, d1, ...]; total depth = produk.
                    let dims = |ar: &Range| {
                        if ar.msb >= ar.lsb {
                            ar.msb - ar.lsb + 1
                        } else {
                            ar.lsb - ar.msb + 1
                        }
                    };
                    let mut all_dims: Vec<usize> = vec![dims(ar)];
                    for (ext_r, ext_sz) in &var.extra_unpacked_dims {
                        if let Some(r) = ext_r {
                            all_dims.push(dims(r));
                        } else if let Some(sz) = ext_sz {
                            match const_eval_params(sz, &effective_params) {
                                Ok(n) if n > 0 => all_dims.push(n as usize),
                                _ => {} // ukuran tak ter-resolve → dim diabaikan
                            }
                        }
                    }
                    let total_depth: usize = all_dims.iter().product();
                    let total_width = elem_width * total_depth;
                    if std::env::var("DBG_DECL").is_ok() {
                        eprintln!(
                            "DBG-DECL array name={} elem_w={} dims={:?} depth={} width={}",
                            var.name.as_str(), elem_width, all_dims, total_depth, total_width
                        );
                    }
                    let sid = get_or_create_signal(
                        var.name,
                        total_width,
                        kind.clone(),
                        net_type,
                        &mut signals,
                        &mut signal_map,
                        &mut next_id,
                        total_depth,
                        elem_width,
                        total_width - 1,
                        0,
                        decl_is_2state,
                        is_signed_type(&decl.dtype),
                    );
                    let sig = &mut signals[sid];
                    sig.is_2state = decl_is_2state;
                    let elem_init = if kind == SignalKind::Wire {
                        LogicVec::fill(LogicVal::Z, elem_width)
                    } else {
                        LogicVec::new(elem_width)
                    };
                    // `LogicVec::new` me-clamp lebar > 1M bit jadi 1 bit (guard
                    // OOM untuk lebar bogus). Memory SRAM asli OpenTitan
                    // (`sram_ctrl` dgn ECC: 65536 x 76 = 4.98M bit) sah dan
                    // jauh lebih besar dari 1M — pakai `fill` (tanpa clamp)
                    // untuk full_init agar init array tidak out-of-bounds.
                    let mut full_init = if kind == SignalKind::Wire {
                        LogicVec::fill(LogicVal::Z, total_width)
                    } else {
                        LogicVec::fill(LogicVal::X, total_width)
                    };
                    let init_n = full_init.bits.len();
                    // Loop bound STORAGE-consistent: `elem_width`/`total_depth`
                    // (telah di-reject bila absurd oleh guard E3012, tapi jalur
                    // lain bisa lolos) TIDAK boleh memandu iterasi — storage
                    // `full_init` di-clamp oleh LogicVec (≤2^27). Sebelumnya
                    // loop 256 × 2^31 = 5.5e11 iterasi → hang (temuan fuzzer).
                    let elems_covered = if elem_width == 0 {
                        0
                    } else {
                        init_n.div_ceil(elem_width).min(total_depth)
                    };
                    for i in 0..elems_covered {
                        let j_span = elem_width.min(elem_init.bits.len());
                        for (j, bit) in elem_init.bits[..j_span].iter().copied().enumerate() {
                            let idx = i * elem_width + j;
                            if idx < init_n {
                                full_init.bits[idx] = bit;
                            }
                        }
                    }
                    sig.init_val = full_init;
                    // Populate array_dims for unpacked arrays (F39: semua dim).
                    sig.array_dims = all_dims.clone();
                    if let Some(ref class) = class_name {
                        sig.class_name = Some(Symbol::intern(class));
                        if class == "__mailbox" {
                            sig.is_mailbox = true;
                        }
                        if class == "__semaphore" {
                            sig.is_semaphore = true;
                        }
                    }
                    // Compute packed dimension widths for multi-dim packed arrays
                    if !var.extra_packed_dims.is_empty() {
                        let first_width = if let Some(er) = &var.expr_range {
                            resolve_expr_range(er, &effective_params).map(|r| r.width())
                        } else if let Some(r) = &var.range {
                            Ok(r.width())
                        } else {
                            Ok(1usize)
                        };
                        if let Ok(fw) = first_width {
                            let mut pd = vec![fw];
                            for (extra_er, _) in &var.extra_packed_dims {
                                if let Ok(or) = resolve_expr_range(extra_er, &effective_params) {
                                    pd.push(or.width());
                                }
                            }
                            sig.packed_dims = pd;
                        }
                    } else if let DataType::UserDefined(tn) = &decl.dtype {
                        // Deklarasi bertipe typedef multi-dimensi (`box_t x;`
                        // dengan `typedef logic [4:0][4:0][W-1:0] box_t`) —
                        // salin packed dims dari typedef agar `x[0][0][z]`
                        // di-resolve sebagai elemen bukan bit tunggal.
                        if let Some((range, extras)) = self.typedef_dims.get(tn) {
                            let mut pd = Vec::new();
                            if let Some(er) = range {
                                if let Ok(r) = resolve_expr_range(er, &effective_params) {
                                    pd.push(r.width());
                                }
                            }
                            for extra_er in extras {
                                if let Ok(or) = resolve_expr_range(extra_er, &effective_params) {
                                    pd.push(or.width());
                                }
                            }
                            if !pd.is_empty() {
                                sig.packed_dims = pd;
                            }
                        }
                    }
                } else {
                    if std::env::var("DBG_DECL").is_ok() {
                        eprintln!("DBG-DECL scalar name={} elem_w={} array_range={:?}", var.name.as_str(), elem_width, var.array_range.is_some());
                    }
                    let sid = get_or_create_signal(
                        var.name,
                        elem_width,
                        kind,
                        net_type,
                        &mut signals,
                        &mut signal_map,
                        &mut next_id,
                        1,
                        elem_width,
                        d_msb,
                        d_lsb,
                        decl_is_2state,
                        is_signed_type(&decl.dtype),
                    );
                    let sig = &mut signals[sid];
                    // Associative array (`int m [string];` / `[*]`): tak punya
                    // dimensi — storage-nya HashMap di engine (`assoc_data`).
                    // PRA-FIX `is_associative` hardcoded false di cabang ini →
                    // signal diperlakukan array 1-elemen, sehingga `m["k"] = v`
                    // menulis di luar rentang ( hilang) dan `m["k"]` baca 0.
                    if var.is_associative {
                        sig.is_associative = true;
                        sig.array_depth = 0;
                        sig.array_dims = vec![];
                        sig.width = elem_width;
                        sig.elem_width = elem_width;
                        sig.msb = elem_width.saturating_sub(1);
                        sig.lsb = 0;
                    }
                    if let Some(class) = &class_name {
                        sig.class_name = Some(Symbol::intern(class));
                        if class == "__mailbox" {
                            sig.is_mailbox = true;
                        }
                        if class == "__semaphore" {
                            sig.is_semaphore = true;
                        }
                    }
                    sig.is_2state = decl_is_2state;
                    // Compute packed dimension widths for multi-dim packed arrays
                    if !var.extra_packed_dims.is_empty() {
                        if let Some(er) = &var.expr_range {
                            if let Ok(r) = resolve_expr_range(er, &effective_params) {
                                let mut pd = vec![r.width()];
                                for (extra_er, _) in &var.extra_packed_dims {
                                    if let Ok(or) = resolve_expr_range(extra_er, &effective_params)
                                    {
                                        pd.push(or.width());
                                    }
                                }
                                sig.packed_dims = pd;
                            }
                        }
                    } else if let DataType::UserDefined(tn) = &decl.dtype {
                        // Deklarasi bertipe typedef multi-dimensi (`box_t x;`)
                        // — salin packed dims dari typedef (sama seperti cabang
                        // unpacked array di atas).
                        if let Some((range, extras)) = self.typedef_dims.get(tn) {
                            let mut pd = Vec::new();
                            if let Some(er) = range {
                                if let Ok(r) = resolve_expr_range(er, &effective_params) {
                                    pd.push(r.width());
                                }
                            }
                            for extra_er in extras {
                                if let Ok(or) = resolve_expr_range(extra_er, &effective_params) {
                                    pd.push(or.width());
                                }
                            }
                            if !pd.is_empty() {
                                sig.packed_dims = pd;
                            }
                        }
                    }
                }
                // Compute struct/union field offsets for member access
                if let Some(&sid) = signal_map.get(&var.name) {
                    let sig = &mut signals[sid];
                    match &decl.dtype {
                        DataType::StructType { members } | DataType::UnionType { members } => {
                            match &decl.dtype {
                                DataType::UnionType { members } => {
                                    for m in members {
                                        sig.struct_fields.push(self.struct_field_from_member(m, 0));
                                    }
                                }
                                _ => {
                                    let mut offset = 0usize;
                                    let members_rev: Vec<_> = members.iter().rev().collect();
                                    for m in &members_rev {
                                        sig.struct_fields
                                            .push(self.struct_field_from_member(m, offset));
                                        offset += m.range.as_ref().map(|r| r.width()).unwrap_or(1);
                                    }
                                    sig.struct_fields.reverse();
                                }
                            }
                        }
                        DataType::UserDefined(name) => {
                            if let Some(fields) = self.lookup_struct_fields(name.as_str()) {
                                if !fields.is_empty() {
                                    sig.struct_fields = fields;
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        step_ck("after decls loop", &step_t0);

        // ── Localparam ARRAY (`localparam logic [63:0] RC [24] = '{...}`) ──
        // Parser membuang unpacked dims `[24]` dan menyimpan default `'{...}`
        // sebagai `Expr::Concat` multi-elemen. Array ini TIDAK boleh masuk
        // param_vals sebagai skalar (param_util.rs mencegahnya) — daftarkan
        // sebagai signal const array agar `RC[rnd]` di-resolve sebagai
        // ArrayIndex lebar elem_width (bukan bit-select 1-bit yang kemudian
        // gagal dengan "range select out of bounds"). Didaftarkan SEBELUM
        // proses item (assign/always/initial) karena `assign out = RC[rnd]...`
        // harus bisa me-resolve RC saat elaborasi.
        //
        // CATATAN: parser menyimpan `ModuleItem::Param` (termasuk localparam
        // body) di `module.params`, bukan `module.items`. Iterasi kedua sumber:
        // `module.params` (deklarasi module) + `expanded_items` (param dari
        // hasil generate block).
        let mut param_array_srcs: Vec<&ParamDecl> = Vec::new();
        param_array_srcs.extend(module.params.iter());
        for item in &expanded_items {
            if let ModuleItem::Param(p) = item {
                param_array_srcs.push(p);
            }
        }
        for p in param_array_srcs {
            if std::env::var("MIVON_DBG_LP").is_ok() {
                eprintln!(
                    "[DBG-LP] {} is_lp={} in_sigmap={} default={:?}",
                    p.name.as_str(),
                    p.is_localparam,
                    signal_map.contains_key(&p.name),
                    p.default
                        .as_ref()
                        .map(|e| format!("{:?}", e).chars().take(80).collect::<String>())
                );
            }
            if !p.is_localparam || signal_map.contains_key(&p.name) {
                continue;
            }
            if std::env::var("MIVON_DEBUG_PARAMARR").is_ok() {
                eprintln!(
                    "[DBG-PARAMARR] {} default={:?}",
                    p.name.as_str(),
                    p.default
                        .as_ref()
                        .map(|e| format!("{:?}", e).chars().take(120).collect::<String>())
                );
            }
            if let Some(Expr::Concat(elems)) = &p.default {
                if elems.len() > 1 {
                    // 2D array (`PiRotate [5][5] = '{ '{...}, '{...} }`):
                    // elemen default adalah Concat bertingkat. Daftarkan sebagai
                    // array 1D datar (depth = total elemen skalar, elem_width =
                    // lebar skalar) DAN daftarkan key ter-flatten `name[r][c]`
                    // di param_vals agar `PiRotate[x][y]` dengan index konstanta
                    // di-fold saat elaborasi (const_eval BitSelect nested).
                    let is_2d = elems.iter().all(|e| matches!(e, Expr::Concat(_)));
                    let mut flat_elems: Vec<&Expr> = Vec::new();
                    if is_2d {
                        for e in elems.iter() {
                            if let Expr::Concat(row) = e {
                                flat_elems.extend(row.iter());
                            }
                        }
                    } else {
                        flat_elems.extend(elems.iter());
                    }
                    let elem_width = if let Some((msb, lsb)) = &p.range {
                        match (
                            const_eval_params(msb, &effective_params),
                            const_eval_params(lsb, &effective_params),
                        ) {
                            (Ok(m), Ok(l)) => (m.max(l) - m.min(l)) as usize + 1,
                            _ => 1,
                        }
                    } else if matches!(p.dtype, Some(DataType::Int) | Some(DataType::Integer)) {
                        // `localparam int PiRotate [5][5]` — tanpa range, elemen
                        // adalah int 32-bit.
                        32
                    } else {
                        1
                    };
                    let depth = flat_elems.len();
                    let total_width = elem_width * depth;
                    // Daftarkan key ter-flatten untuk const-fold: `name[i]`
                    // (1D) atau `name[r][c]` (2D). Ini membuat `PiRotate[x][y]`
                    // jadi Const saat elaborasi — index array tidak lagi
                    // dievaluasi runtime dengan width 1.
                    for (fi, e) in flat_elems.iter().enumerate() {
                        if let Ok(v) = const_eval_params(e, &effective_params) {
                            if is_2d {
                                let cols = flat_elems.len() / elems.len();
                                let r = fi / cols;
                                let c = fi % cols;
                                let key = format!("{}[{}][{}]", p.name.as_str(), r, c);
                                effective_params.insert(Symbol::intern(&key), v);
                                self.param_vals.insert(Symbol::intern(&key), v);
                            } else {
                                let key = format!("{}[{}]", p.name.as_str(), fi);
                                effective_params.insert(Symbol::intern(&key), v);
                                self.param_vals.insert(Symbol::intern(&key), v);
                            }
                        }
                        // Elemen struct (`'{offset: 8'd0, size: 8'd64}`):
                        // daftarkan member keys `name[i].field` → nilai field.
                        // Ini membuat `arr[i].field` (mis. di generate body)
                        // ter-fold via const_eval MemberAccess.
                        if let Expr::StructLit { members } = e {
                            for m in members {
                                if let StructLitMember::Named(fname, fexpr) = m {
                                    if let Ok(fv) = const_eval_params(fexpr, &effective_params) {
                                        let key = format!(
                                            "{}[{}].{}",
                                            p.name.as_str(),
                                            fi,
                                            fname.as_str()
                                        );
                                        effective_params.insert(Symbol::intern(&key), fv);
                                        self.param_vals.insert(Symbol::intern(&key), fv);
                                    }
                                }
                            }
                        }
                    }
                    let sid = get_or_create_signal(
                        p.name,
                        total_width,
                        SignalKind::Reg,
                        NetType::Wire,
                        &mut signals,
                        &mut signal_map,
                        &mut next_id,
                        depth,
                        elem_width,
                        total_width.saturating_sub(1),
                        0,
                        false,
                        false,
                    );
                    let sig = &mut signals[sid];
                    sig.is_const = true;
                    let mut init = LogicVec::new(total_width);
                    for (i, e) in flat_elems.iter().enumerate() {
                        if let Ok(v) = const_eval_params(e, &effective_params) {
                            let ev = LogicVec::from_u64(v as u64, elem_width);
                            let base = i * elem_width;
                            // Bounds storage-consistent (init/ev di-clamp LogicVec):
                            // hindari hang/panic index-OOB utk elem_width absurd.
                            let j_span = elem_width
                                .min(init.bits.len().saturating_sub(base))
                                .min(ev.bits.len());
                            for j in 0..j_span {
                                if base + j < total_width {
                                    init.bits[base + j] = ev.bits[j];
                                }
                            }
                        }
                    }
                    sig.init_val = init;
                }
            }
        }

        step_ck("after localparam array", &step_t0);

        // Process module items
        let mut proc_counter = 0usize;
        // Pre-pass: daftarkan loop variable dari `for` loops sebagai signal
        // sintetis 32-bit. Loop var bukan signal module; runtime LoopFor
        // (fallback saat unroll gagal) tetap butuh signal untuk di-resolve.
        {
            let mut loop_vars: Vec<Symbol> = Vec::new();
            for item in &expanded_items {
                match item {
                    ModuleItem::Always(a) => {
                        super::util::loop_unroll::collect_loop_var_names(&a.stmts, &mut loop_vars)
                    }
                    ModuleItem::Initial(i) | ModuleItem::Final(i) => {
                        super::util::loop_unroll::collect_loop_var_names(&i.stmts, &mut loop_vars)
                    }
                    ModuleItem::Func(f) => {
                        super::util::loop_unroll::collect_loop_var_names(&f.stmts, &mut loop_vars)
                    }
                    _ => {}
                }
            }
            for v in &loop_vars {
                if !signal_map.contains_key(v) {
                    let sid = next_id;
                    next_id += 1;
                    signal_map.insert(*v, sid);
                    signals.push(SignalInfo {
                        name: *v,
                        width: 32,
                        kind: SignalKind::Reg,
                        net_type: NetType::Wire,
                        multi_driver: false,
                        init_val: LogicVec::new(32),
                        array_depth: 1,
                        elem_width: 32,
                        array_dims: vec![],
                        class_name: None,
                        is_string: false,
                        is_mailbox: false,
                        is_semaphore: false,
                        is_real: false,
                        is_2state: true,
                        is_dynamic: false,
                        is_queue: false,
                        is_associative: false,
                        is_signed: true,
                        is_const: false,
                        msb: 31,
                        lsb: 0,
                        struct_fields: vec![],
                        packed_dims: vec![],
                        delay_rise: None,
                        delay_fall: None,
                        iface_type: None,
                        iface_modport: None,
                    });
                }
            }
        }
        // PRE-PASS implicit net: Verilog-2001 mengizinkan ident TAK dikenal
        // dalam koneksi port OUTPUT/INOUT instance menjadi implicit wire.
        // Koneksi input yang MENGONSUMSI ident tersebut boleh muncul LEBIH
        // DULU di source (chip_earlgrey `scanmode`, chip_darjeeling
        // `es_rng_fips`): `.scanmode_i(scanmode)` di baris 421 sebelum
        // `.dft_scan_md_o(scanmode)` di baris 1014). Tanpa pre-pass, konsumsi
        // input gagal resolve → E2001. Scan SEMUA instance dulu (lebar 1-bit,
        // di-upgrade di `implicit_declare_port_idents` saat koneksi output
        // diproses) — helper idempoten, jadi tidak dobel.
        for pre_item in &expanded_items {
            let ModuleItem::Instance(pre_inst) = pre_item else {
                continue;
            };
            let pre_target = self
                .module_idx
                .get(&pre_inst.module_name)
                .and_then(|&i| self.design.modules.get(i));
            let Some(pre_tm) = pre_target else {
                continue;
            };
            for (i, conn) in pre_inst.port_conns.iter().enumerate() {
                let out_like: Option<(bool, &Expr)> = match conn {
                    PortConnection::Positional(expr) => pre_tm.ports.get(i).map(|p| {
                        (
                            matches!(p.direction, PortDirection::Output | PortDirection::Inout),
                            expr,
                        )
                    }),
                    PortConnection::Named { port, expr } => {
                        pre_tm.ports.iter().find(|p| p.name == *port).map(|p| {
                            (
                                matches!(p.direction, PortDirection::Output | PortDirection::Inout),
                                expr,
                            )
                        })
                    }
                    PortConnection::Unconnected { .. } => None,
                };
                if let Some((true, e)) = out_like {
                    self.implicit_declare_port_idents(
                        e,
                        &mut signal_map,
                        &mut signals,
                        &mut next_id,
                    );
                }
            }
        }

        for item in &expanded_items {
            let item_kind = match item {
                ModuleItem::Always(_) => "always",
                ModuleItem::Initial(_) => "initial",
                ModuleItem::Final(_) => "final",
                ModuleItem::Assign(_) => "assign",
                ModuleItem::Instance(_) => "instance",
                ModuleItem::Func(_) => "func",
                ModuleItem::Typedef(_) => "typedef",
                ModuleItem::Decl(_) => "decl",
                _ => "other",
            };
            let item_t0 = std::time::Instant::now();
            let item_ck = |kind: &str, t0: &std::time::Instant| {
                let e = t0.elapsed();
                if dbg_step && e.as_micros() > 200_000 {
                    eprintln!(
                        "[DBG-STEP] {}: item<{}> in {}us",
                        module.name.as_str(),
                        kind,
                        e.as_micros()
                    );
                }
            };
            match item {
                // LANG-04/11/12/13: module-level concurrent assertion property
                // boolean — ubah jadi always block ber-clock (dari clock_event
                // assertion) agar engine mengevaluasi tiap edge clock.
                ModuleItem::PropertyAssert(stmt) => {
                    let clock_event = match stmt.as_ref() {
                        Stmt::Assert { clock_event, .. }
                        | Stmt::Assume { clock_event, .. }
                        | Stmt::Cover { clock_event, .. }
                        | Stmt::PropertySeq { clock_event, .. } => clock_event.clone(),
                        _ => None,
                    };
                    if let Some(ce) = clock_event {
                        let ev = match &ce {
                            mivon_ast::types::ClockEvent::Posedge(s) => {
                                SensitivityEvent::PosEdge(Expr::Ident {
                                    name: *s,
                                    line: 0,
                                    col: 0,
                                })
                            }
                            mivon_ast::types::ClockEvent::Negedge(s) => {
                                SensitivityEvent::NegEdge(Expr::Ident {
                                    name: *s,
                                    line: 0,
                                    col: 0,
                                })
                            }
                            mivon_ast::types::ClockEvent::Edge(s) => {
                                SensitivityEvent::Level(Expr::Ident {
                                    name: *s,
                                    line: 0,
                                    col: 0,
                                })
                            }
                        };
                        let always = AlwaysBlock {
                            kind: AlwaysKind::Always,
                            sensitivity: Some(SensitivityList { events: vec![ev] }),
                            stmts: vec![stmt.as_ref().clone()],
                        };
                        match self.elaborate_always(&always, &signal_map, &signals) {
                            Ok(process) => processes.push(process),
                            Err(e) => {
                                let mut diag = e.to_diagnostic();
                                if !self.is_current_module_reachable() || self.is_fragment_module()
                                {
                                    diag.level = DiagLevel::Warning;
                                }
                                self.diag_sink.push(diag);
                            }
                        }
                    } else {
                        // Tanpa clock event — evaluasi sebagai proses initial
                        // sekali (bentuk langka; fallback aman).
                        let name = format_sym(b"initial_prop_", proc_counter);
                        proc_counter += 1;
                        *self.current_proc_name.borrow_mut() = Some(name);
                        let body_res = self.elaborate_stmt_block(
                            std::slice::from_ref(stmt.as_ref()),
                            &signal_map,
                            known_modules,
                            &signals,
                        );
                        *self.current_proc_name.borrow_mut() = None;
                        match body_res {
                            Ok(body) => processes.push(Process::Initial { name, body }),
                            Err(e) => {
                                let mut diag = e.to_diagnostic();
                                if !self.is_current_module_reachable() || self.is_fragment_module()
                                {
                                    diag.level = DiagLevel::Warning;
                                }
                                self.diag_sink.push(diag);
                            }
                        }
                    }
                }
                ModuleItem::Always(always) => {
                    match self.elaborate_always(always, &signal_map, &signals) {
                        Ok(process) => {
                            processes.push(process);
                        }
                        Err(e) => {
                            // Process ini di-skip agar module tetap terdaftar dengan
                            // signal-nya, TAPI error dipertahankan level aslinya
                            // (Error). Error di satu tempat bersifat GLOBAL: gate di
                            // main.rs memblokir simulasi & VCD sampai semua bersih.
                            // F38: module unreachable → downgrade ke warning.
                            let mut diag = e.to_diagnostic();
                            if !self.is_current_module_reachable() || self.is_fragment_module() {
                                diag.level = DiagLevel::Warning;
                            }
                            self.diag_sink.push(diag);
                        }
                    }
                    item_ck(item_kind, &item_t0);
                }
                ModuleItem::Initial(initial) => {
                    // SIM-29: set nama proses SEBELUM elaborate agar stmt_lines
                    // tercatat dengan key yang SAMA dengan record_line_hit.
                    let name = format_sym(b"initial_", proc_counter);
                    proc_counter += 1;
                    *self.current_proc_name.borrow_mut() = Some(name);
                    let body_res = self.elaborate_stmt_block(
                        &initial.stmts,
                        &signal_map,
                        known_modules,
                        &signals,
                    );
                    *self.current_proc_name.borrow_mut() = None;
                    match body_res {
                        Ok(body) => {
                            processes.push(Process::Initial { name, body });
                        }
                        Err(e) => {
                            // Error GLOBAL (lihat komentar di Always).
                            let mut diag = e.to_diagnostic();
                            if !self.is_current_module_reachable() || self.is_fragment_module() {
                                diag.level = DiagLevel::Warning;
                            }
                            self.diag_sink.push(diag);
                        }
                    }
                    item_ck(item_kind, &item_t0);
                }
                ModuleItem::Final(final_block) => {
                    // SIM-29: set nama proses SEBELUM elaborate.
                    let name = format_sym(b"final_", proc_counter);
                    proc_counter += 1;
                    *self.current_proc_name.borrow_mut() = Some(name);
                    let body_res = self.elaborate_stmt_block(
                        &final_block.stmts,
                        &signal_map,
                        known_modules,
                        &signals,
                    );
                    *self.current_proc_name.borrow_mut() = None;
                    match body_res {
                        Ok(body) => {
                            processes.push(Process::Final { name, body });
                        }
                        Err(e) => {
                            // Error GLOBAL (lihat komentar di Always).
                            let mut diag = e.to_diagnostic();
                            if !self.is_current_module_reachable() || self.is_fragment_module() {
                                diag.level = DiagLevel::Warning;
                            }
                            self.diag_sink.push(diag);
                        }
                    }
                    item_ck(item_kind, &item_t0);
                }
                ModuleItem::Assign(assign) => {
                    // Undeclared identifier (LHS atau RHS) → implicit net
                    // (semantik Verilog; pola generated code OpenTitan seperti
                    // `assign tl_reg_h2d = tl_i;` dan `assign tl_o_pre =
                    // tl_reg_d2h;` di mana net `tl_reg_d2h` tak terdeklarasi
                    // dulu dikoneksikan ke reg block yang di-optimalkan).
                    // Lebar net LHS diambil dari lebar RHS; net RHS = 1-bit.
                    let mut implicit_nets: Vec<(Symbol, usize, usize)> = Vec::new();
                    if let Expr::Ident { name, line, col } = &assign.lhs {
                        if !signal_map.contains_key(name) {
                            implicit_nets.push((*name, *line, *col));
                        }
                    }
                    collect_implicit_net_idents(
                        &assign.rhs,
                        &signal_map,
                        &self.param_vals,
                        &self.pkg_param_ctx,
                        &mut implicit_nets,
                    );
                    for (name, line, col) in implicit_nets {
                        if signal_map.contains_key(&name) {
                            continue;
                        }
                        let width = if matches!(&assign.lhs, Expr::Ident { name: ln, .. } if *ln == name)
                        {
                            super::util::width::compute_expr_width(
                                &assign.rhs,
                                &signal_map,
                                &signals,
                                &self.param_vals,
                                &self.package_symbols,
                            )
                            .unwrap_or(1)
                            .max(1)
                        } else {
                            1
                        };
                        let sid = next_id;
                        next_id += 1;
                        signal_map.insert(name, sid);
                        signals.push(SignalInfo {
                            name,
                            width,
                            kind: SignalKind::Wire,
                            net_type: NetType::Wire,
                            multi_driver: false,
                            init_val: LogicVec::fill(LogicVal::Z, width),
                            array_depth: 1,
                            elem_width: width,
                            array_dims: vec![],
                            class_name: None,
                            is_string: false,
                            is_mailbox: false,
                            is_semaphore: false,
                            is_real: false,
                            is_2state: false,
                            is_dynamic: false,
                            is_queue: false,
                            is_associative: false,
                            is_signed: false,
                            is_const: false,
                            msb: width - 1,
                            lsb: 0,
                            struct_fields: vec![],
                            packed_dims: vec![],
                            delay_rise: None,
                            delay_fall: None,
                            iface_type: None,
                            iface_modport: None,
                        });
                        self.elab_warn_at(
                            DiagCode::UndefinedSignal,
                            format!(
                                "signal '{}' not declared; creating implicit net (width {})",
                                name, width
                            ),
                            line,
                            col,
                        );
                    }
                    // Convert to a combinational process
                    let lhs_result = self.elaborate_lvalue(&assign.lhs, &signal_map, &signals);
                    let rhs_result = self.elaborate_expr(&assign.rhs, &signal_map, &signals);
                    match (lhs_result, rhs_result) {
                        (Ok(lhs), Ok(mut rhs)) => {
                            // Propagasi lebar konteks LHS → operand
                            // context-determined RHS (LRM §11.8.1).
                            let lhs_w = crate::elaborator::stmt::lvalue_width(&lhs, &signals);
                            if lhs_w > 0 {
                                // Whole-RHS konstanta → fold langsung pada
                                // lebar konteks. JANGAN fold bila RHS
                                // mereferensikan param SIGNED: const_eval
                                // memperlakukan Ident sbg unsigned sehingga
                                // `assign y = (N<0)` (N=-2) salah jadi 0
                                // (probe p19).
                                if expr_refs_signed_param(&assign.rhs, &self.param_is_signed) {
                                    propagate_context_width(&mut rhs, lhs_w, &signals);
                                } else if let Some(c) =
                                    try_fold_const_at_width(&assign.rhs, &self.param_vals, lhs_w)
                                {
                                    rhs = c;
                                } else {
                                    propagate_context_width(&mut rhs, lhs_w, &signals);
                                }
                            }
                            let stmts = vec![IrStmt::BlockingAssign {
                                lhs,
                                rhs,
                                delay: None,
                            }];
                            let sensitivity = collect_sensitivity(&assign.rhs, &signal_map)
                                .into_iter()
                                .map(SignalSensitivity::whole)
                                .collect();
                            processes.push(Process::Combinational {
                                name: format_sym(b"assign_", proc_counter),
                                sensitivity,
                                body: stmts,
                            });
                            proc_counter += 1;
                        }
                        (Err(e), _) | (_, Err(e)) => {
                            // Assign ini di-skip agar module tetap terdaftar.
                            // Downgrade errors in fragment modules (implicit
                            // unit wrappers for include snippets) — their
                            // signals come from the enclosing context, not the
                            // fragment scope (mirrors parse-level fragment
                            // downgrade and `is_current_module_reachable`).
                            let mut diag = e.to_diagnostic();
                            if self.is_fragment_module() {
                                diag.level = DiagLevel::Warning;
                            }
                            self.diag_sink.push(diag);
                        }
                    }
                    item_ck(item_kind, &item_t0);
                }
                ModuleItem::Typedef(td) => {
                    // Already collected in pre-pass; register for UserDefined resolution
                    let width = self.typedef_map.get(&td.name).copied().unwrap_or_else(|| {
                        self.resolve_typedef_width_dims(
                            &td.dtype,
                            td.range.as_ref(),
                            &td.extra_packed_dims,
                            &self.param_vals,
                        )
                    });
                    self.typedef_map.insert(td.name, width);
                    self.typedef_dims
                        .insert(td.name, (td.range.clone(), td.extra_packed_dims.clone()));
                    // Store struct/union field info for member access
                    match &td.dtype {
                        DataType::StructType { members } | DataType::UnionType { members } => {
                            let mut fields = Vec::new();
                            match &td.dtype {
                                DataType::UnionType { members } => {
                                    for m in members {
                                        fields.push(self.struct_field_from_member(m, 0));
                                    }
                                }
                                _ => {
                                    let mut offset = 0usize;
                                    let members_rev: Vec<_> = members.iter().rev().collect();
                                    for m in &members_rev {
                                        fields.push(self.struct_field_from_member(m, offset));
                                        offset += m.range.as_ref().map(|r| r.width()).unwrap_or(1);
                                    }
                                    fields.reverse();
                                }
                            }
                            self.typedef_field_map.insert(td.name, fields);
                        }
                        _ => {}
                    }
                }
                ModuleItem::Instance(inst) => {
                    // Check if this is a UDP instance
                    let udp_match = self
                        .design
                        .udp_defs
                        .iter()
                        .find(|u| u.name == inst.module_name)
                        .cloned();

                    if let Some(udp) = udp_match {
                        // UDP instance: create combinational process with table lookup
                        let mut sig_ids = Vec::new();
                        for conn in &inst.port_conns {
                            let expr = match conn {
                                PortConnection::Positional(e) => e,
                                PortConnection::Named { expr, .. } => expr,
                                // Port UDP tak terhubung — lewati.
                                PortConnection::Unconnected { .. } => continue,
                            };
                            let sid = self.instance_port_expr_to_signal(
                                expr,
                                &signal_map,
                                &mut signals,
                                &mut next_id,
                                &mut processes,
                                &format!("{}.udp", inst.instance_name),
                                false,
                            )?;
                            sig_ids.push(sid);
                        }
                        if sig_ids.len() < 2 {
                            return Err(self.elab_diag(
                                DiagCode::ParamMismatch,
                                format!(
                                    "UDP '{}' requires at least 2 ports (1 output + 1+ inputs)",
                                    udp.name
                                ),
                            ));
                        }
                        let out_id = sig_ids[0];
                        let in_ids: Vec<SignalId> = sig_ids[1..].to_vec();
                        let mut in_exprs: Vec<IrExpr> =
                            in_ids.iter().map(|id| IrExpr::Signal(*id, 0)).collect();
                        // For sequential UDP, add output state as last arg (state feedback)
                        if udp.is_sequential {
                            in_exprs.push(IrExpr::Signal(out_id, 0));
                        }
                        let mut sensitivity: Vec<SignalSensitivity> = in_ids
                            .iter()
                            .map(|&id| SignalSensitivity::whole(id))
                            .collect();
                        if udp.is_sequential {
                            sensitivity.push(SignalSensitivity::whole(out_id));
                        }
                        let process = Process::Combinational {
                            name: Symbol::intern(&format!(
                                "udp_{}_{}",
                                udp.name, inst.instance_name
                            )),
                            sensitivity: sensitivity.clone(),
                            body: vec![IrStmt::BlockingAssign {
                                lhs: IrLValue::Signal(out_id, 0),
                                rhs: IrExpr::UdpLookup {
                                    udp_name: udp.name,
                                    args: in_exprs,
                                },
                                delay: None,
                            }],
                        };
                        processes.push(process);
                        // Handle initial output for sequential UDP
                        if let Some(ref init_sym) = udp.initial_output {
                            let init_val = match init_sym {
                                UdpSymbol::Zero => LogicVec::fill(LogicVal::Zero, 1),
                                UdpSymbol::One => LogicVec::fill(LogicVal::One, 1),
                                _ => LogicVec::fill(LogicVal::X, 1),
                            };
                            processes.push(Process::Initial {
                                name: Symbol::intern(&format!(
                                    "udp_init_{}_{}",
                                    udp.name, inst.instance_name
                                )),
                                body: vec![IrStmt::BlockingAssign {
                                    lhs: IrLValue::Signal(out_id, 0),
                                    rhs: IrExpr::Const(init_val),
                                    delay: None,
                                }],
                            });
                        }
                    } else if let Some(cd) = self.checker_decls.get(&inst.module_name).cloned() {
                        // LANG-10: checker instance — bind port (positional/
                        // named) ke signal, lalu drive assertion property di
                        // body checker sebagai always block ber-clock (pola
                        // sama dengan ModuleItem::PropertyAssert module-level).
                        let mut bind_map = signal_map.clone();
                        for (i, conn) in inst.port_conns.iter().enumerate() {
                            let (pname, expr) = match conn {
                                PortConnection::Positional(e) => {
                                    (cd.ports.get(i).copied(), Some(e))
                                }
                                PortConnection::Named { port, expr } => (Some(*port), Some(expr)),
                                PortConnection::Unconnected { .. } => (None, None),
                            };
                            if let (Some(pname), Some(expr)) = (pname, expr) {
                                let sid = self.instance_port_expr_to_signal(
                                    expr,
                                    &signal_map,
                                    &mut signals,
                                    &mut next_id,
                                    &mut processes,
                                    &format!("{}.{}", inst.instance_name, pname.as_str()),
                                    false,
                                )?;
                                bind_map.insert(pname, sid);
                            }
                        }
                        // Drive assertion property items di body checker.
                        for item in &cd.items {
                            let ModuleItem::PropertyAssert(stmt) = item else {
                                continue;
                            };
                            let clock_event = match stmt.as_ref() {
                                Stmt::Assert { clock_event, .. }
                                | Stmt::Assume { clock_event, .. }
                                | Stmt::Cover { clock_event, .. } => clock_event.clone(),
                                _ => None,
                            };
                            if let Some(ce) = clock_event {
                                let ev = match &ce {
                                    mivon_ast::types::ClockEvent::Posedge(s) => {
                                        SensitivityEvent::PosEdge(Expr::Ident {
                                            name: *s,
                                            line: 0,
                                            col: 0,
                                        })
                                    }
                                    mivon_ast::types::ClockEvent::Negedge(s) => {
                                        SensitivityEvent::NegEdge(Expr::Ident {
                                            name: *s,
                                            line: 0,
                                            col: 0,
                                        })
                                    }
                                    mivon_ast::types::ClockEvent::Edge(s) => {
                                        SensitivityEvent::Level(Expr::Ident {
                                            name: *s,
                                            line: 0,
                                            col: 0,
                                        })
                                    }
                                };
                                let always = AlwaysBlock {
                                    kind: AlwaysKind::Always,
                                    sensitivity: Some(SensitivityList { events: vec![ev] }),
                                    stmts: vec![stmt.as_ref().clone()],
                                };
                                match self.elaborate_always(&always, &bind_map, &signals) {
                                    Ok(process) => processes.push(process),
                                    Err(e) => {
                                        let mut diag = e.to_diagnostic();
                                        if !self.is_current_module_reachable()
                                            || self.is_fragment_module()
                                        {
                                            diag.level = DiagLevel::Warning;
                                        }
                                        self.diag_sink.push(diag);
                                    }
                                }
                            }
                        }
                    } else {
                        // Regular module instance
                        if std::env::var("DBG_FLATTEN_PARAM").is_ok() {
                            eprintln!(
                                "[DBG-INST] name={} mod={} param_assigns={} port_conns={}",
                                inst.instance_name,
                                inst.module_name,
                                inst.param_assigns.len(),
                                inst.port_conns.len()
                            );
                        }
                        let mut port_map = HashMap::new();
                        // Look up target module to get port order for positional connections
                        let target_module: Option<&Module> = self
                            .module_idx
                            .get(&inst.module_name)
                            .and_then(|&i| self.design.modules.get(i));
                        for (i, conn) in inst.port_conns.iter().enumerate() {
                            match conn {
                                PortConnection::Positional(expr) => {
                                    if let Some(tm) = target_module {
                                        if let Some(port) = tm.ports.get(i) {
                                            // Verilog-2001 implicit net: ident tak
                                            // dikenal dalam koneksi OUTPUT/INOUT port
                                            // jadi wire 1-bit (prim_diff_encode
                                            // `diff_n_buf`, chip_* `scanmode`/
                                            // `es_rng_fips`). Input port TETAP error
                                            // (sesuai LRM & test regresi).
                                            if matches!(
                                                port.direction,
                                                PortDirection::Output | PortDirection::Inout
                                            ) {
                                                self.implicit_declare_port_idents(
                                                    expr,
                                                    &mut signal_map,
                                                    &mut signals,
                                                    &mut next_id,
                                                );
                                            }
                                            let sig_id = self.instance_port_expr_to_signal(
                                                expr,
                                                &signal_map,
                                                &mut signals,
                                                &mut next_id,
                                                &mut processes,
                                                &format!("{}.{}", inst.instance_name, port.name),
                                                matches!(
                                                    port.direction,
                                                    PortDirection::Output | PortDirection::Inout
                                                ),
                                            )?;
                                            port_map.insert(port.name, sig_id);
                                        }
                                    }
                                }
                                PortConnection::Named { port, .. }
                                    if *port == Symbol::intern("*") =>
                                {
                                    // `.*` sentinel (IEEE 1800 §23.2.2.3): sambung
                                    // tiap port target_module ke ident bernama sama
                                    // di scope pemanggil. Port yg sudah terhubung
                                    // eksplisit (campuran `.a(x), .*`) dipertahankan.
                                    // Input tanpa signal bernama sama → skip
                                    // (unconnected, tanpa error — konservatif).
                                    if let Some(tm) = target_module {
                                        let star_ports: Vec<(Symbol, bool)> = tm
                                            .ports
                                            .iter()
                                            .map(|p| {
                                                (
                                                    p.name,
                                                    matches!(
                                                        p.direction,
                                                        PortDirection::Output
                                                            | PortDirection::Inout
                                                    ),
                                                )
                                            })
                                            .collect();
                                        for (pname, is_output_like) in star_ports {
                                            if port_map.contains_key(&pname) {
                                                continue;
                                            }
                                            let ident = Expr::Ident {
                                                name: pname,
                                                line: 0,
                                                col: 0,
                                            };
                                            if !is_output_like && !signal_map.contains_key(&pname) {
                                                continue; // input tanpa pasangan → unconnected
                                            }
                                            if is_output_like {
                                                self.implicit_declare_port_idents(
                                                    &ident,
                                                    &mut signal_map,
                                                    &mut signals,
                                                    &mut next_id,
                                                );
                                            }
                                            let sig_id = self.instance_port_expr_to_signal(
                                                &ident,
                                                &signal_map,
                                                &mut signals,
                                                &mut next_id,
                                                &mut processes,
                                                &format!("{}.{}", inst.instance_name, pname),
                                                is_output_like,
                                            )?;
                                            port_map.insert(pname, sig_id);
                                        }
                                    }
                                }
                                PortConnection::Named { port, expr } => {
                                    // Implicit net hanya untuk output/inout port
                                    // (aturan Verilog-2001).
                                    let is_output_like = target_module
                                        .and_then(|tm| tm.ports.iter().find(|p| p.name == *port))
                                        .map(|p| {
                                            matches!(
                                                p.direction,
                                                PortDirection::Output | PortDirection::Inout
                                            )
                                        })
                                        .unwrap_or(false);
                                    if is_output_like {
                                        self.implicit_declare_port_idents(
                                            expr,
                                            &mut signal_map,
                                            &mut signals,
                                            &mut next_id,
                                        );
                                    }
                                    let sig_id = self.instance_port_expr_to_signal(
                                        expr,
                                        &signal_map,
                                        &mut signals,
                                        &mut next_id,
                                        &mut processes,
                                        &format!("{}.{}", inst.instance_name, port),
                                        is_output_like,
                                    )?;
                                    port_map.insert(*port, sig_id);
                                }
                                // Port tak terhubung (`.port()`): TIDAK buat
                                // stub/port_assign (bukan literal 0!) — port
                                // child tetap internal: output didorong child,
                                // input mengambang Z/X. port_map tanpa entry →
                                // flatten mempertahankan signal port child.
                                PortConnection::Unconnected { .. } => {}
                            }
                        }
                        // Resolve parameter overrides to integer values.
                        // Override BUKAN ekspresi konstan (mis. sinyal) adalah
                        // ilegal LRM — error di titik override (bukan tebak 0
                        // diam-diam seperti sebelumnya).
                        let mut param_map = HashMap::new();
                        for (pname, pexpr) in &inst.param_assigns {
                            let val = match const_eval_with_params(pexpr, &effective_params) {
                                Ok(v) => v,
                                Err(_) => {
                                    let (l, c) = expr_location(pexpr);
                                    return Err(self.elab_diag_at(
                                        DiagCode::ParamMismatch,
                                        format!(
                                            "override parameter '{pname}' bukan ekspresi konstan \
                                             pada instance '{}' — nilai parameter harus dapat \
                                             dievaluasi saat elaborasi",
                                            inst.instance_name.as_str()
                                        ),
                                        l,
                                        c,
                                    ));
                                }
                            };
                            param_map.insert(*pname, val);
                            // Override STRUCT package (`Info = PartInfoDefault`,
                            // `Info = PartInfo[k]`): `const_eval_with_params`
                            // mengubah override struct menjadi skalar 0 (flatten
                            // struct→0) sehingga member keys `Info.<field>`
                            // HILANG dan generate if `Info.integrity` di child
                            // gagal const-eval (error E3001/E3003 "member access
                            // not allowed" — pola otp_ctrl_part_buf/unbuf).
                            // Salin member keys dari index struct package
                            // (pkg_struct_ref_index) ke param_map agar child
                            // menerima `Info.integrity` dst. sebagai skalar.
                            if let Some(fields) =
                                self.struct_override_fields(pexpr, &effective_params)
                            {
                                for f in &fields {
                                    if let Some(fname) = f.name {
                                        if let CVal::Scalar(v) = f.val {
                                            param_map.insert(
                                                Symbol::intern(&format!(
                                                    "{}.{}",
                                                    pname.as_str(),
                                                    fname.as_str()
                                                )),
                                                v,
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        let mut type_param_map: HashMap<Symbol, usize> = HashMap::new();
                        for (pname, ta) in &inst.type_param_assigns {
                            // Override type param `#(.T(Word16))` — lebar dari
                            // UserDefined harus di-RESOLVE ke typedef
                            // (`resolve_type_width`), bukan `dt.width()` yang
                            // selalu 1 utk UserDefined. Override literal
                            // (`#(.T(logic[15:0]))`) memakai range yang
                            // disimpan parser (F32: sebelumnya dibuang → 1).
                            // Gagal resolve → fallback dt.width().
                            let w = if let Some((msb, lsb)) = &ta.range {
                                match (
                                    const_eval_with_params(msb, &effective_params),
                                    const_eval_with_params(lsb, &effective_params),
                                ) {
                                    (Ok(m), Ok(l)) => m.abs_diff(l) as usize + 1,
                                    _ => self
                                        .resolve_type_width(&ta.dtype)
                                        .unwrap_or_else(|_| ta.dtype.width()),
                                }
                            } else {
                                self.resolve_type_width(&ta.dtype)
                                    .unwrap_or_else(|_| ta.dtype.width())
                            };
                            type_param_map.insert(*pname, w);
                        }
                        // Re-bucket: override type param yang ter-parse sebagai
                        // VALUE (`#(.T(Wide16))` — Ident bukan type-token) →
                        // pindahkan ke type_param_map bila pname adalah type
                        // param target & nilainya me-resolve sebagai tipe.
                        // Tanpa ini override typedef tak pernah berlaku
                        // (T tetap default). Value param tak tersentuh
                        // (pname-nya bukan type param).
                        if let Some(tm) = target_module {
                            for p in &tm.params {
                                if !p.is_type_param
                                    || type_param_map.contains_key(&p.name)
                                {
                                    continue;
                                }
                                let type_name = match inst.param_assigns.get(&p.name) {
                                    Some(Expr::Ident { name, .. }) => {
                                        Some(name.as_str().to_string())
                                    }
                                    Some(Expr::ScopedIdent { package, item, .. }) => {
                                        Some(format!(
                                            "{}::{}",
                                            package.as_str(),
                                            item.as_str()
                                        ))
                                    }
                                    _ => None,
                                };
                                if let Some(tn) = type_name {
                                    if let Some(w) = self.resolve_cast_name_width(&tn) {
                                        type_param_map.insert(p.name, w);
                                        param_map.remove(&p.name);
                                    }
                                }
                            }
                        }

                        if let Some(range) = &inst.range {
                            let msb = const_eval_with_params(&range.msb, &effective_params)
                                .map_err(|e| {
                                    self.elab_diag_at(
                                        DiagCode::SimulationError,
                                        format!("instance range bound evaluation failed: {}", e),
                                        inst.line,
                                        inst.col,
                                    )
                                })?;
                            let lsb = const_eval_with_params(&range.lsb, &effective_params)
                                .map_err(|e| {
                                    self.elab_diag_at(
                                        DiagCode::SimulationError,
                                        format!("instance range bound evaluation failed: {}", e),
                                        inst.line,
                                        inst.col,
                                    )
                                })?;
                            let (start, end) = if msb >= lsb { (lsb, msb) } else { (msb, lsb) };
                            let pm = std::sync::Arc::new(port_map);
                            let pam = std::sync::Arc::new(param_map);
                            let tpam = std::sync::Arc::new(type_param_map);
                            for idx in start..=end {
                                let inst_name = format!("{}[{}]", inst.instance_name, idx);
                                sub_instances.push(IrInstance {
                                    module_name: inst.module_name,
                                    instance_name: Symbol::intern(&inst_name),
                                    port_map: pm.clone(),
                                    param_map: pam.clone(),
                                    type_param_map: tpam.clone(),
                                    line: inst.line,
                                    col: inst.col,
                                    attrs: inst.attrs.clone(),
                                });
                            }
                        } else {
                            sub_instances.push(IrInstance {
                                module_name: inst.module_name,
                                instance_name: inst.instance_name,
                                port_map: std::sync::Arc::new(port_map),
                                param_map: std::sync::Arc::new(param_map),
                                type_param_map: std::sync::Arc::new(type_param_map),
                                line: inst.line,
                                col: inst.col,
                                attrs: inst.attrs.clone(),
                            });
                        }
                    }
                    item_ck(item_kind, &item_t0);
                }
                ModuleItem::VirtualInterface {
                    iface_type,
                    modport,
                    vif_name,
                } => {
                    // Create a signal for the virtual interface variable
                    let sid = next_id;
                    next_id += 1;
                    signal_map.insert(*vif_name, sid);
                    signals.push(SignalInfo {
                        name: *vif_name,
                        width: 64,
                        kind: SignalKind::Reg,
                        net_type: NetType::Wire,
                        multi_driver: false,
                        init_val: LogicVec::new(64),
                        array_depth: 1,
                        elem_width: 64,
                        array_dims: vec![],
                        class_name: Some(*iface_type),
                        is_string: false,
                        is_mailbox: false,
                        is_semaphore: false,
                        is_real: false,
                        is_2state: true,
                        is_dynamic: false,
                        is_queue: false,
                        is_associative: false,
                        is_signed: false,
                        is_const: false,
                        msb: 63,
                        lsb: 0,
                        struct_fields: vec![],
                        packed_dims: vec![],
                        delay_rise: None,
                        delay_fall: None,
                        iface_type: Some(*iface_type),
                        iface_modport: *modport,
                    });
                }
                ModuleItem::Gate(gate) => {
                    if gate.ports.len() < 2 {
                        return Err(self.elab_diag(
                            DiagCode::ParamMismatch,
                            format!(
                                "gate requires at least 2 ports (gate type: {:?}, got {} ports)",
                                gate.gate_type,
                                gate.ports.len()
                            ),
                        ));
                    }
                    // Port output harus signal; port input boleh ekspresi arbitrer
                    // (mis. `buf b0 (out, a && b);` — ekspresi di-elaborate ke IR).
                    let (out_exprs, in_src): (Vec<&Expr>, Vec<&Expr>) = match gate.gate_type {
                        GateType::And
                        | GateType::Or
                        | GateType::Nand
                        | GateType::Nor
                        | GateType::Xor
                        | GateType::Xnor => {
                            (vec![&gate.ports[0]], gate.ports[1..].iter().collect())
                        }
                        GateType::Buf | GateType::Not => {
                            let (outs, inputs) = gate.ports.split_at(gate.ports.len() - 1);
                            (outs.iter().collect(), inputs.iter().collect())
                        }
                    };
                    let mut out_ids = Vec::with_capacity(out_exprs.len());
                    for port in &out_exprs {
                        let out_id = match port {
                            Expr::Ident { name, line, col } => {
                                if let Some(&sid) = signal_map.get(name) {
                                    sid
                                } else {
                                    // Undeclared gate output → implicit 1-bit net
                                    // (semantik SystemVerilog: identifier tanpa
                                    // deklarasi menjadi net 1-bit).
                                    self.elab_warn_at(
                                        DiagCode::UndefinedSignal,
                                        format!(
                                            "signal '{}' not declared for gate; creating implicit 1-bit net",
                                            name
                                        ),
                                        *line,
                                        *col,
                                    );
                                    let sid = next_id;
                                    next_id += 1;
                                    signal_map.insert(*name, sid);
                                    signals.push(SignalInfo {
                                        name: *name,
                                        width: 1,
                                        kind: SignalKind::Wire,
                                        net_type: NetType::Wire,
                                        multi_driver: false,
                                        init_val: LogicVec::fill(LogicVal::Z, 1),
                                        array_depth: 1,
                                        elem_width: 1,
                                        array_dims: vec![],
                                        class_name: None,
                                        is_string: false,
                                        is_mailbox: false,
                                        is_semaphore: false,
                                        is_real: false,
                                        is_2state: false,
                                        is_dynamic: false,
                                        is_queue: false,
                                        is_associative: false,
                                        is_signed: false,
                                        is_const: false,
                                        msb: 0,
                                        lsb: 0,
                                        struct_fields: vec![],
                                        packed_dims: vec![],
                                        delay_rise: None,
                                        delay_fall: None,
                                        iface_type: None,
                                        iface_modport: None,
                                    });
                                    sid
                                }
                            }
                            _ => {
                                return Err(self.elab_diag_at(DiagCode::InstanceNotFound, format!(
                                    "gate output port must be a simple signal (port expression: {:?})",
                                    port
                                ), expr_location(port).0, expr_location(port).1))
                            }
                        };
                        out_ids.push(out_id);
                    }
                    let in_exprs: Vec<IrExpr> = in_src
                        .iter()
                        .map(|p| self.elaborate_expr(p, &signal_map, &signals))
                        .collect::<Result<_, _>>()?;
                    let mut gate_sens: Vec<SignalSensitivity> = Vec::new();
                    for p in &in_src {
                        for id in collect_sensitivity(p, &signal_map) {
                            gate_sens.push(SignalSensitivity::whole(id));
                        }
                    }
                    let gate_expr = build_gate_expr(&gate.gate_type, &in_exprs);
                    for &out_id in &out_ids {
                        let process = Process::Combinational {
                            name: Symbol::intern(&format!("gate_{}", out_id)),
                            sensitivity: gate_sens.clone(),
                            body: vec![IrStmt::BlockingAssign {
                                lhs: IrLValue::Signal(out_id, 0),
                                rhs: gate_expr.clone(),
                                delay: None,
                            }],
                        };
                        processes.push(process);
                    }
                }
                _ => {}
            }
        }

        // F40: initializer port ANSI (`output reg [7:0] b = 8'h2A;`) →
        // Process::Initial, setara deklarasi `reg b = 8'h2A;`.
        // Urutan WAJIB didahulukan: inisialisasi variabel SV terjadi pada
        // fase inisialisasi SEBELUM aktivitas prosedural t=0 (LRM 1800
        // §4.3.2 + §6.8). Sebelumnya proses ini di-push setelah `initial`
        // user → `output st = 0; initial st = 1;` berakhir `st=0` (init
        // menimpa assignment user di delta yang sama).
        let mut port_init_procs: Vec<Process> = Vec::with_capacity(port_inits.len());
        for (name, init_expr) in &port_inits {
            let lhs = self.elaborate_lvalue(
                &Expr::Ident {
                    name: *name,
                    line: 0,
                    col: 0,
                },
                &signal_map,
                &signals,
            )?;
            let rhs = self.elaborate_expr(init_expr, &signal_map, &signals)?;
            port_init_procs.push(Process::Initial {
                name: format_sym(b"port_init_", proc_counter),
                body: vec![IrStmt::BlockingAssign {
                    lhs,
                    rhs,
                    delay: None,
                }],
            });
            proc_counter += 1;
        }
        if !port_init_procs.is_empty() {
            port_init_procs.append(&mut processes);
            processes = port_init_procs;
        }

        // Process declaration initializers (wire a = 1; reg b = 0; etc.)
        for decl in &all_decls {
            for var in &decl.names {
                if let Some(init_expr) = &var.expr {
                    let lhs = self.elaborate_lvalue(
                        &Expr::Ident {
                            name: var.name,
                            line: 0,
                            col: 0,
                        },
                        &signal_map,
                        &signals,
                    )?;

                    // Unpacked array literal init — `rom [0:3] = '{e0, e1, e2, e3}`.
                    // Concat membangun value MSB-first `{e0, e1, e2, e3}` tapi storage
                    // unpacked punya element 0 di bit TERENDAH → fold ke Const lalu
                    // write utuh menghasilkan array TERBALIK (bug: rom[0]=e3). Sama
                    // dengan statement assign (stmt.rs:805): decompose ke per-elemen
                    // assign `rom[i] = ei;` di level IR.
                    let mut per_elem_stmts: Vec<IrStmt> = Vec::new();
                    if let (Some(sid), Expr::Concat(elems)) = (lvalue_signal_id(&lhs), init_expr) {
                        if let Some(sig) = signals.get(sid) {
                            if sig.array_depth > 1 {
                                // F39: multi-dimensi — nested concat
                                // `'{'{1,2},'{3,4}}` di-flatten row-major ke
                                // elemen flat [e0..eN] (array_dims penuh).
                                // Daftar datar `'{1,2,3,4}` (len == total_depth)
                                // juga diterima — flat index == elemen row-major.
                                let flat = flatten_array_init(elems, &sig.array_dims);
                                if let Some(flat) = flat {
                                    for (i, elem) in flat.iter().enumerate() {
                                        let ir_elem =
                                            self.elaborate_expr(elem, &signal_map, &signals)?;
                                        per_elem_stmts.push(IrStmt::BlockingAssign {
                                            lhs: IrLValue::ArrayIndex {
                                                sig_id: sid,
                                                index: Box::new(IrExpr::Const(LogicVec::from_u64(
                                                    i as u64, 32,
                                                ))),
                                                elem_width: sig.elem_width.max(1),
                                            },
                                            rhs: ir_elem,
                                            delay: None,
                                        });
                                    }
                                } else if elems.len() == sig.array_depth {
                                    // single-dim fallback (list datar)
                                    for (i, elem) in elems.iter().enumerate() {
                                        let ir_elem =
                                            self.elaborate_expr(elem, &signal_map, &signals)?;
                                        per_elem_stmts.push(IrStmt::BlockingAssign {
                                            lhs: IrLValue::ArrayIndex {
                                                sig_id: sid,
                                                index: Box::new(IrExpr::Const(LogicVec::from_u64(
                                                    i as u64, 32,
                                                ))),
                                                elem_width: sig.elem_width.max(1),
                                            },
                                            rhs: ir_elem,
                                            delay: None,
                                        });
                                    }
                                }
                            } else if elems.len() == sig.array_depth {
                                for (i, elem) in elems.iter().enumerate() {
                                    let ir_elem =
                                        self.elaborate_expr(elem, &signal_map, &signals)?;
                                    per_elem_stmts.push(IrStmt::BlockingAssign {
                                        lhs: IrLValue::ArrayIndex {
                                            sig_id: sid,
                                            index: Box::new(IrExpr::Const(LogicVec::from_u64(
                                                i as u64, 32,
                                            ))),
                                            elem_width: sig.elem_width.max(1),
                                        },
                                        rhs: ir_elem,
                                        delay: None,
                                    });
                                }
                            }
                        }
                    }
                    if !per_elem_stmts.is_empty() {
                        let proc_name = format_sym(b"decl_init_", proc_counter);
                        processes.push(Process::Initial {
                            name: proc_name,
                            body: per_elem_stmts,
                        });
                        proc_counter += 1;
                        continue;
                    }

                    let mut rhs = self.elaborate_expr(init_expr, &signal_map, &signals)?;
                    // Lebar konteks untuk initializer deklarasi (LRM §11.8.1).
                    {
                        let lhs_w = lvalue_signal_id(&lhs)
                            .and_then(|sid| signals.get(sid))
                            .map(|s| s.width)
                            .unwrap_or(0);
                        if lhs_w > 0 {
                            // JANGAN fold RHS yang mereferensikan param SIGNED
                            // (const_eval → Ident unsigned; `logic y = (N<0)`
                            // N=-2 salah jadi 0 — probe p19).
                            if expr_refs_signed_param(init_expr, &self.param_is_signed) {
                                propagate_context_width(&mut rhs, lhs_w, &signals);
                            } else if let Some(c) =
                                try_fold_const_at_width(init_expr, &self.param_vals, lhs_w)
                            {
                                rhs = c;
                            } else {
                                propagate_context_width(&mut rhs, lhs_w, &signals);
                            }
                        }
                    }
                    if decl.kind.is_net() {
                        let sensitivity = collect_sensitivity(init_expr, &signal_map)
                            .into_iter()
                            .map(SignalSensitivity::whole)
                            .collect();
                        processes.push(Process::Combinational {
                            name: format_sym(b"decl_assign_", proc_counter),
                            sensitivity,
                            body: vec![IrStmt::BlockingAssign {
                                lhs,
                                rhs,
                                delay: None,
                            }],
                        });
                        proc_counter += 1;
                    } else {
                        processes.push(Process::Initial {
                            name: format_sym(b"decl_init_", proc_counter),
                            body: vec![IrStmt::BlockingAssign {
                                lhs,
                                rhs,
                                delay: None,
                            }],
                        });
                        proc_counter += 1;
                    }
                }
            }
        }

        step_ck("after items loop", &step_t0);

        if std::env::var("DBG_STMT").is_ok() {
            stmt::stmt_dbg_dump(module.name.as_str());
        }

        Ok(IrModule {
            name: module.name,
            signals,
            inputs,
            outputs,
            inouts,
            processes,
            sub_instances,
        })
    }
}

/// Traverse statements secara rekursif dan kumpulkan deklarasi procedural lokal
/// (`int index_x1;` di dalam always/initial block). Parser menyimpannya sebagai
/// `Stmt::NamedBlock` dengan `decls` — variabel ini wajib terdaftar di
/// signal_map agar referensi di dalam loop yang di-unroll bisa di-resolve.
pub(crate) fn collect_procedural_decls(stmts: &[Stmt], out: &mut Vec<Decl>) {
    for s in stmts {
        match s {
            Stmt::NamedBlock { stmts, decls, .. } => {
                out.extend(decls.iter().cloned());
                collect_procedural_decls(stmts, out);
            }
            Stmt::Block { stmts } => collect_procedural_decls(stmts, out),
            Stmt::IfElse {
                true_branch,
                false_branch,
                ..
            }
            | Stmt::UniqueIf {
                true_branch,
                false_branch,
                ..
            }
            | Stmt::PriorityIf {
                true_branch,
                false_branch,
                ..
            } => {
                collect_procedural_decls(std::slice::from_ref(true_branch), out);
                if let Some(fb) = false_branch {
                    collect_procedural_decls(std::slice::from_ref(fb), out);
                }
            }
            Stmt::Case { items, default, .. }
            | Stmt::CaseX { items, default, .. }
            | Stmt::CaseZ { items, default, .. }
            | Stmt::StmtCase { items, default, .. }
            | Stmt::UniqueCase { items, default, .. }
            | Stmt::PriorityCase { items, default, .. }
            | Stmt::Unique0Case { items, default, .. }
            | Stmt::CaseInside { items, default, .. } => {
                for item in items {
                    collect_procedural_decls(std::slice::from_ref(&item.stmt), out);
                }
                if let Some(d) = default {
                    collect_procedural_decls(std::slice::from_ref(d), out);
                }
            }
            Stmt::LoopForever { stmts } | Stmt::LoopWhile { stmts, .. } => {
                collect_procedural_decls(stmts, out);
            }
            Stmt::DoWhile { stmts, .. } => collect_procedural_decls(stmts, out),
            Stmt::LoopFor { stmts, .. } => collect_procedural_decls(stmts, out),
            Stmt::Repeat { stmts, .. } => collect_procedural_decls(stmts, out),
            Stmt::Delay { stmt, .. } => collect_procedural_decls(std::slice::from_ref(stmt), out),
            Stmt::Wait { stmt: Some(s), .. } => {
                collect_procedural_decls(std::slice::from_ref(s), out);
            }
            Stmt::EventControl { stmt: Some(s), .. } => {
                collect_procedural_decls(std::slice::from_ref(s), out);
            }
            Stmt::ForeachLoop { stmts, .. } => collect_procedural_decls(stmts, out),
            Stmt::Assert {
                pass_stmt,
                fail_stmt,
                ..
            }
            | Stmt::Assume {
                pass_stmt,
                fail_stmt,
                ..
            }
            | Stmt::Expect {
                pass_stmt,
                fail_stmt,
                ..
            } => {
                if let Some(p) = pass_stmt {
                    collect_procedural_decls(std::slice::from_ref(p), out);
                }
                if let Some(f) = fail_stmt {
                    collect_procedural_decls(std::slice::from_ref(f), out);
                }
            }
            Stmt::Cover {
                pass_stmt: Some(p), ..
            } => {
                collect_procedural_decls(std::slice::from_ref(p), out);
            }
            Stmt::WaitOrder {
                fail_stmt: Some(f), ..
            } => {
                collect_procedural_decls(std::slice::from_ref(f), out);
            }
            Stmt::Fork { processes, .. } => {
                for p in processes {
                    collect_procedural_decls(std::slice::from_ref(p), out);
                }
            }
            Stmt::RandCase { items } => {
                for item in items {
                    collect_procedural_decls(std::slice::from_ref(&item.stmt), out);
                }
            }
            Stmt::RandSequence { productions } => {
                for prod in productions {
                    for item in &prod.items {
                        collect_procedural_decls(std::slice::from_ref(&item.value), out);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Cek apakah default param berbentuk ident yang mereferensikan konstanta
/// STRUCT package — ada di index `pkg_struct_ref_index` (base name → fields,
/// dibangun sekali dari key `pkg::name.<field>` di pkg_const_scalars).
/// Contoh: `parameter part_info_t Info = PartInfoDefault;` — `PartInfoDefault`
/// adalah parameter struct di `otp_ctrl_part_pkg`.
fn ident_refs_pkg_struct(expr: &Expr, index: &HashMap<Symbol, Vec<SField>>) -> bool {
    let base = match expr {
        Expr::Ident { name, .. } => name.as_str(),
        Expr::ScopedIdent { item, .. } => item.as_str(),
        _ => return false,
    };
    index.contains_key(&Symbol::intern(base))
}

/// Ambil fields struct untuk default param berbentuk ident yang
/// mereferensikan konstanta struct package — dari index package (O(1)).
fn pkg_struct_fields_for_ref(
    expr: &Expr,
    index: &HashMap<Symbol, Vec<SField>>,
) -> Option<Vec<SField>> {
    let base = match expr {
        Expr::Ident { name, .. } => name.as_str(),
        Expr::ScopedIdent { item, .. } => item.as_str(),
        _ => return None,
    };
    index.get(&Symbol::intern(base)).cloned()
}
