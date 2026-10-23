//! Pre-pass desain sebelum elaborasi per-module: konteks parameter paket,
//! skala delay, checker/defparam, dedup module, bind, import paket, inline
//! fungsi, ekspansi generate, deteksi module mati, dan index nama module.
//!
//! Dipisah dari `elaborator/mod.rs` supaya `elaborate()` tinggal memanggil
//! satu fase persiapan lalu menjalankan pass inkremental.

use super::*;

impl Elaborator {
    /// Jalankan seluruh pre-pass desain dan kembalikan himpunan module yang
    /// reachable dari `top_module` (dipakai untuk pruning di pass berikutnya).
    ///
    /// `elab_t0` hanya dipakai instrumentasi `DBG_ELAB` agar durasi yang
    /// dilaporkan tetap dihitung dari awal `elaborate()`.
    pub(super) fn prepare_design(
        &mut self,
        top_module: Option<&str>,
        elab_t0: std::time::Instant,
    ) -> Result<std::collections::HashSet<Symbol>, SimError> {
        // Build global package param context ONCE — dipakai bersama oleh semua
        // module. Sebelumnya dihitung ulang per-modul (rescan semua package +
        // fixed-point 64 iterasi) yang menjadi bottleneck di desain besar.
        self.build_pkg_param_ctx();

        // F84 (LRM 1800-2017 §19.8): hitung faktor skala delay per module.
        // Delay module `1us` = 1000 tick bila basis design `1ns`.
        self.compute_module_delay_scales();

        // LANG-41: checker di compilation-unit scope terlihat oleh semua
        // module (IEEE 1800-2017 §3.2). Seed dulu sebelum checker per-module.
        for item in &self.design.unit_items {
            if let ModuleItem::Checker(cd) = item {
                self.checker_decls
                    .entry(cd.name)
                    .or_insert_with(|| cd.clone());
            }
        }

        // ELAB-09: kumpulkan semua `defparam` — compilation-unit scope
        // (path absolut dari top) + module body (path relatif scope module).
        // Diterapkan saat flatten per instance path; di sini hanya kumpul.
        for item in &self.design.unit_items {
            if let ModuleItem::DefParam(dp) = item {
                self.defparams.push(DefParamEntry {
                    scope_module: None,
                    path: dp.path.clone(),
                    param: dp.param,
                    value: dp.value.clone(),
                    line: dp.line,
                    col: dp.col,
                });
            }
        }
        for module in &self.design.modules {
            for item in &module.items {
                if let ModuleItem::DefParam(dp) = item {
                    self.defparams.push(DefParamEntry {
                        scope_module: Some(module.name),
                        path: dp.path.clone(),
                        param: dp.param,
                        value: dp.value.clone(),
                        line: dp.line,
                        col: dp.col,
                    });
                }
            }
        }

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
                    topo::collect_instance_names(&self.design.modules[idx].items, &mut insts);
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
                        // Var tersintesis — tanpa posisi source.
                        line: 0,
                        col: 0,
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
                    self.pristine_param_modules.insert(m.name, m.clone());
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
                array_structs: &self.pkg_array_structs,
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
                    &self.pkg_const_arrays,
                    &self.package_symbols,
                    &self.pkg_struct_ref_index,
                    &self.pkg_array_structs,
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
        if std::env::var("DBG_PRISTINE").is_ok() {
            let mut names: Vec<&str> = self
                .pristine_param_modules
                .keys()
                .map(|s| s.as_str())
                .collect();
            names.sort_unstable();
            eprintln!(
                "[DBG-PRISTINE] n={} has_lsu={} has_mmu={} has_tlb={} names={:?}",
                names.len(),
                self.pristine_param_modules
                    .contains_key(&Symbol::intern("load_store_unit")),
                self.pristine_param_modules
                    .contains_key(&Symbol::intern("cva6_mmu")),
                self.pristine_param_modules
                    .contains_key(&Symbol::intern("cva6_tlb")),
                names,
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
                    topo::collect_instance_names(&m.items, &mut insts);
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
                let mut insts = Vec::new();
                if let Some(module) = module_map.get(&name) {
                    topo::collect_instance_names(&module.items, &mut insts);
                }
                // Pristine (belum ter-expand) module — cabang generate yang
                // hilang saat expansion param-default tetap dihitung agar
                // module spt `ras` tidak salah di-prune (lihat komentar
                // compute_reachable_set).
                if let Some(pristine) = self.pristine_param_modules.get(&name) {
                    topo::collect_instance_names(&pristine.items, &mut insts);
                }
                for mn in insts {
                    if all_names.contains(&mn) && !reachable.contains(&mn) {
                        reachable.insert(mn);
                        queue.push_back(mn);
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
                        array_structs: &self.pkg_array_structs,
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
                            &self.pkg_const_arrays,
                            &self.package_symbols,
                            &self.pkg_struct_ref_index,
                            &self.pkg_array_structs,
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
        Ok(reachable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(s: &str) -> Symbol {
        Symbol::intern(s)
    }

    fn module_with_insts(name: &str, insts: &[&str]) -> Module {
        Module {
            name: sym(name),
            line: 0,
            col: 0,
            ports: vec![],
            params: vec![],
            decls: vec![],
            items: insts
                .iter()
                .map(|m| {
                    ModuleItem::Instance(ModuleInstance {
                        module_name: sym(m),
                        instance_name: sym("u"),
                        range: None,
                        param_assigns: HashMap::new(),
                        type_param_assigns: HashMap::new(),
                        port_conns: vec![],
                        line: 0,
                        col: 0,
                        attrs: vec![],
                    })
                })
                .collect(),
            timescale: None,
        }
    }

    /// Regresi pemisahan modul: pre-pass harus mengembalikan cone reachable
    /// dari top (top + anak) dan mengeluarkan module mati, sekaligus mengisi
    /// index nama module yang dipakai pass berikutnya.
    #[test]
    fn prepare_design_returns_reachable_cone() {
        let design = Design {
            modules: vec![
                module_with_insts("top", &["child"]),
                module_with_insts("child", &[]),
                module_with_insts("dead", &[]),
            ],
            ..Default::default()
        };
        let mut elab = Elaborator::new(design);
        let reachable = elab
            .prepare_design(Some("top"), std::time::Instant::now())
            .expect("pre-pass harus berhasil");

        assert!(reachable.contains(&sym("top")), "{reachable:?}");
        assert!(reachable.contains(&sym("child")), "{reachable:?}");
        assert!(!reachable.contains(&sym("dead")), "{reachable:?}");
        // module_idx dibangun SETELAH drop non-cone (L712-715): hanya top +
        // child yang sisa; dead tidak terindeks dan tidak dalam reachable.
        assert_eq!(elab.module_idx.len(), 2);
        assert!(!elab.module_idx.contains_key(&sym("dead")));
        assert_eq!(elab.module_idx.get(&sym("top")), Some(&0));
        assert_eq!(elab.module_idx.get(&sym("child")), Some(&1));
    }
}
