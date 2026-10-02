use super::Elaborator;
use mivon_ast::*;
use mivon_core::diagnostics::diagnostic::DiagCode;
use mivon_core::error::SimError;
use mivon_core::intern::Symbol;
use mivon_ir::*;
use std::collections::HashMap;

impl Elaborator {
    /// Signature deterministik untuk cache IR instance ber-parameter: module
    /// name + param override (sorted) + type param override (sorted). Dua
    /// instance dengan override identik menghasilkan IR yang identik, jadi
    /// elaborasi berat hanya perlu dilakukan sekali per signature unik.
    fn param_ir_signature(
        &self,
        module_name: Symbol,
        param_map: &HashMap<Symbol, i64>,
        type_param_map: &HashMap<Symbol, usize>,
    ) -> (Symbol, u64) {
        use mivon_core::checksum::{combine_checksum, compute_checksum, compute_str_checksum};
        let mut h = compute_str_checksum(module_name.as_str());
        let mut keys: Vec<&Symbol> = param_map.keys().collect();
        keys.sort_by_key(|k| k.as_str());
        for k in keys {
            h = combine_checksum(h, compute_str_checksum(k.as_str()));
            h = combine_checksum(h, compute_checksum(&param_map[k].to_le_bytes()));
        }
        let mut tkeys: Vec<&Symbol> = type_param_map.keys().collect();
        tkeys.sort_by_key(|k| k.as_str());
        for k in tkeys {
            h = combine_checksum(h, compute_str_checksum(k.as_str()));
            h = combine_checksum(
                h,
                compute_checksum(&(type_param_map[k] as u64).to_le_bytes()),
            );
        }
        (module_name, h)
    }

    pub(crate) fn flatten_instances(
        &mut self,
        top: &mut IrModule,
    ) -> Result<HashMap<Symbol, SignalId>, SimError> {
        let module_index: HashMap<Symbol, usize> = self
            .design
            .modules
            .iter()
            .enumerate()
            .map(|(i, m)| (m.name, i))
            .collect();
        let known_mods: Vec<Symbol> = self.design.modules.iter().map(|m| m.name).collect();
        // ── Perbaikan global (hierarchy tree): simpan instance top ──
        // `flatten_instances_inner` memakai `std::mem::take(&mut top.sub_instances)`
        // dan TIDAK pernah mengembalikannya → `IrDesign.top.sub_instances`
        // SELALU kosong setelah elaborasi. Akibatnya melab `--tree`, hierarchy
        // tree debugger, dan outline GUI tidak pernah menampilkan anak top
        // (hanya root), dan distributed partitioner selalu jatuh ke jalur
        // single-partition (instance partitioning mati). Clone daftar asli
        // SEBELUM flatten dan kembalikan setelah selesai — hierarki asli
        // (instance + line/col) tetap tersedia di IrDesign.top. Biaya: satu
        // clone list instance top (kecil).
        let saved_instances = top.sub_instances.clone();
        let mut chain = Vec::new();
        let mut map = self.flatten_instances_inner(top, &mut chain, &module_index, &known_mods)?;
        // F28 post-pass: proses job alias hier port interface SETELAH semua
        // instance ter-flatten — tidak bergantung pada urutan sub_instances
        // (instance interface boleh muncul setelah child module di AST).
        // Kembalikan daftar instance asli top (lihat komentar di atas).
        top.sub_instances = saved_instances;
        let jobs = std::mem::take(&mut *self.iface_alias_jobs.borrow_mut());
        for (port_name, iface_name, inst_path) in jobs {
            let Some(iface) = self.design.interfaces.iter().find(|i| i.name == iface_name) else {
                continue;
            };
            for d in &iface.decls {
                for n in &d.names {
                    let child_key = format!("{}.{}", port_name, n.name);
                    let parent_key = format!("{}.{}", inst_path, n.name);
                    if let Some(field_sid) = top
                        .signals
                        .iter()
                        .position(|s| s.name.as_str() == parent_key)
                    {
                        map.insert(Symbol::intern(&child_key), field_sid);
                    }
                }
            }
        }
        Ok(map)
    }

    fn flatten_instances_inner(
        &mut self,
        top: &mut IrModule,
        chain: &mut Vec<Symbol>,
        module_index: &HashMap<Symbol, usize>,
        known_mods: &[Symbol],
    ) -> Result<HashMap<Symbol, SignalId>, SimError> {
        let mut hier_signal_map: HashMap<Symbol, SignalId> = HashMap::new();
        let instances = std::mem::take(&mut top.sub_instances);
        for inst in &instances {
            // Cycle detection: chain berisi nama module yang sedang di-flatten.
            // Instantiation cycle tidak legal di SV — deteksi dini mencegah
            // rekursi tak terbatas / duplikasi kerja (anti-leak memori).
            if chain.contains(&inst.module_name) {
                return Err(self.elab_diag_at(DiagCode::ModuleNotFound, format!(
                    "instantiation cycle detected: module '{}' is instantiated from within its own hierarchy (instance '{}')",
                    inst.module_name, inst.instance_name
                ), inst.line, inst.col));
            }

            // Clone AST Module HANYA bila instance memakai parameter custom /
            // type parameter (harus di-elaborasi ulang). Kasus default-param
            // (paling umum) langsung memakai IR hasil elaborasi — tanpa clone
            // AST penuh per instance (anti-peak O(depth × AST)).
            let inst_module = module_index
                .get(&inst.module_name)
                .map(|&i| &self.design.modules[i]);
            let needs_custom_params =
                inst_module.is_some_and(|m| !m.params.is_empty()) && !inst.param_map.is_empty();
            let needs_type_params = !inst.type_param_map.is_empty();
            let mut child = if needs_custom_params || needs_type_params {
                // Cache IR per signature (module + override). Ribuan instance
                // dengan override identik → elaborasi hanya SEKALI per
                // signature, bukan per instance (bottleneck flatten di desain
                // besar: ~62k-ctx resolve + AST clone + elaborasi penuh per
                // instance).
                let sig = self.param_ir_signature(
                    inst.module_name,
                    &inst.param_map,
                    &inst.type_param_map,
                );
                let cached = self.param_ir_cache.get(&sig).cloned();
                match cached {
                    Some(ir) => ir,
                    None => {
                        let ast_module: Module = match inst_module {
                            Some(m) => m.clone(),
                            None => match self
                                .design
                                .interfaces
                                .iter()
                                .find(|i| i.name == inst.module_name)
                            {
                                Some(iface) => Module {
                                    name: iface.name,
                                    ports: vec![],
                                    params: vec![],
                                    decls: iface.decls.clone(),
                                    items: vec![],
                                },
                                None => {
                                    return Err(self.elab_diag_at(
                                        DiagCode::ModuleNotFound,
                                        format!(
                                            "module or interface '{}' not found for instance '{}'",
                                            inst.module_name, inst.instance_name
                                        ),
                                        inst.line,
                                        inst.col,
                                    ))
                                }
                            },
                        };
                        let param_vals = self.resolve_param_values(&ast_module, &inst.param_map)?;
                        let ir = self.elaborate_module_with_params_and_type(
                            &ast_module,
                            known_mods,
                            &param_vals,
                            &inst.type_param_map,
                        )?;
                        // Bounded cache: 512 entry cukup untuk duplikasi
                        // parameter umum; sisa (signature unik) dielaborasi
                        // sekali saja tanpa menyimpan.
                        if self.param_ir_cache.len() >= 512 {
                            self.param_ir_cache.clear();
                        }
                        self.param_ir_cache.insert(sig, ir.clone());
                        ir
                    }
                }
            } else {
                // Use pre-elaborated module (default params) — move langsung,
                // tanpa klone AST.
                match self.modules.get(&inst.module_name) {
                    Some(ir) => {
                        if ir.sub_instances.is_empty() {
                            // Leaf module (no sub-instances): skip expensive full
                            // IrModule clone. Child is only read during translation
                            // (signals, processes) — never mutated. flatten_instances_inner
                            // is a no-op for leaf modules (std::mem::take returns empty
                            // vec, loop body never executes).
                            self.translate_child_into_parent(ir, inst, top, &mut hier_signal_map)?;
                            continue;
                        }
                        ir.clone()
                    }
                    None => {
                        return Err(self.elab_diag_at(
                            DiagCode::ModuleNotFound,
                            format!(
                                "module or interface '{}' not found for instance '{}'",
                                inst.module_name, inst.instance_name
                            ),
                            inst.line,
                            inst.col,
                        ))
                    }
                }
            };

            // Recursively flatten child's own instances
            chain.push(inst.module_name);
            let child_hier_map =
                self.flatten_instances_inner(&mut child, chain, module_index, known_mods)?;
            chain.pop();
            hier_signal_map.extend(child_hier_map);

            self.translate_child_into_parent(&child, inst, top, &mut hier_signal_map)?;
        }
        Ok(hier_signal_map)
    }

    /// Translate child module's signals and processes into parent — extracted
    /// from flatten_instances_inner. Takes `&IrModule` (reference) so leaf
    /// modules (no sub_instances) skip the expensive full IrModule clone.
    fn translate_child_into_parent(
        &self,
        child: &IrModule,
        inst: &IrInstance,
        top: &mut IrModule,
        hier_signal_map: &mut HashMap<Symbol, SignalId>,
    ) -> Result<(), SimError> {
        // Build signal remapping: child_signal_id -> parent_signal_id
        let mut sig_remap: Vec<Option<SignalId>> = vec![None; child.signals.len()];
        let mut next_parent_id = top.signals.len();

        // Map port connections
        for (port_name, &parent_sig) in inst.port_map.iter() {
            if let Some(child_sig) = child.signals.iter().position(|s| s.name == *port_name) {
                let child_sig_info = &child.signals[child_sig];
                let parent_sig_info = &top.signals[parent_sig];
                let child_width = child_sig_info.width;
                // Bandingkan lebar yang relevan: bila child port adalah
                // unpacked array (array_depth > 1), bandingkan lebar TOTAL
                // (parent.width sudah termasuk array depth). Bila child port
                // scalar, bandingkan elem_width parent — ini menjaga kompat
                // dengan pola lama (array signal → scalar port) yang tetap
                // diterima.
                let parent_width = if child_sig_info.array_depth > 1 {
                    parent_sig_info.width
                } else {
                    parent_sig_info.elem_width
                };
                if child_width != parent_width {
                    // SV LRM: koneksi signal dengan lebar berbeda ke port adalah
                    // legal — implisit zero-extension / truncation saat sim. Jadi
                    // cukup warning (WR0102), bukan error yang memblokir design.
                    self.elab_warn_at(
                            DiagCode::WidthMismatchWarning,
                            format!(
                                "port width mismatch on instance '{}': port '{}' expects width {}, connected signal '{}' has width {}",
                                inst.instance_name, port_name, child_width,
                                parent_sig_info.name, parent_width
                            ),
                            inst.line,
                            inst.col,
                        );
                }
                // Untuk port unpacked-array, pastikan lebar ELEMEN juga cocok.
                // Dua kasus bisa punya total width sama tapi elemen beda
                // (mis. [15:0][0:1] vs [7:0][0:3]) — tanpa guard ini check lolos
                // namun indexing array di engine salah.
                if child_sig_info.array_depth > 1
                    && child_sig_info.elem_width != parent_sig_info.elem_width
                {
                    // Array of STRUCT: lebar elemen di-resolve per-module dan
                    // bergantung konteks param — untuk struct yang sama bisa
                    // menghasilkan angka berbeda antar modul (mis. 1 vs 12
                    // untuk tl_d2h_t[2] di OpenTitan). Check ini jadi false-
                    // positive yang memblokir design valid → downgrade ke
                    // warning (lebar total sudah diperiksa di atas).
                    let is_struct_typed = !child_sig_info.struct_fields.is_empty()
                        || !parent_sig_info.struct_fields.is_empty();
                    if is_struct_typed {
                        self.elab_warn_at(
                                DiagCode::WidthMismatchWarning,
                                format!(
                                    "port array element width mismatch on instance '{}': port '{}' expects element width {}, connected signal '{}' has element width {} (struct-typed; ignored)",
                                    inst.instance_name, port_name, child_sig_info.elem_width,
                                    parent_sig_info.name, parent_sig_info.elem_width
                                ),
                                inst.line,
                                inst.col,
                            );
                    } else {
                        return Err(self.elab_diag_at(DiagCode::ParamMismatch, format!(
                                "port array element width mismatch on instance '{}': port '{}' expects element width {}, connected signal '{}' has element width {}",
                                inst.instance_name, port_name, child_sig_info.elem_width,
                                parent_sig_info.name, parent_sig_info.elem_width
                            ), inst.line, inst.col));
                    }
                }
                // Port type checking: inout must connect to tri. Downgrade
                // ke warning (bukan error): pola Verilator/DV umum —
                // `chip_earlgrey_verilator` mengkoneksikan pad inout ke
                // signal `logic`, dan port inout TANPA `tri` (top_earlgrey
                // `inout flash_test_voltage_h_io`) ter-resolve ke Wire.
                // Check keras di sini memblokir desain valid (E3003 palsu).
                if child.signals[child_sig].kind == SignalKind::Inout
                    && top.signals[parent_sig].net_type != NetType::Tri
                {
                    self.elab_warn_at(
                            DiagCode::ParamMismatch,
                            format!(
                                "port type mismatch on instance '{}': inout port '{}' connects to '{}' with net type {:?} (expected tri; treated as net)",
                                inst.instance_name, port_name,
                                top.signals[parent_sig].name,
                                top.signals[parent_sig].net_type
                            ),
                            inst.line,
                            inst.col,
                        );
                }
                sig_remap[child_sig] = Some(parent_sig);
                // Add hierarchical alias: inst_name.port_name -> parent signal ID
                hier_signal_map.insert(
                    Symbol::intern(&format!("{}.{}", inst.instance_name, port_name)),
                    parent_sig,
                );
                // F28: port interface (`axi_if`) dikoneksikan ke instance
                // interface di parent (handle `__iface_<inst>`) — akses
                // field `axi_if.<f>` di child harus resolve ke signal
                // flatten `inst.<f>`. Kumpulkan JOB (diproses post-pass di
                // flatten_instances setelah SEMUA instance ter-flatten)
                // agar tidak bergantung urutan AST: instance interface
                // boleh muncul setelah child module.
                if child_sig_info.iface_type.is_some() {
                    let iface_name = child_sig_info
                        .iface_type
                        .as_ref()
                        .map(|t| t.as_str().split('.').next().unwrap_or(t.as_str()));
                    // Instance path disimpan di class_name handle (F28) —
                    // nama signal handle berbasis hint (`__iface_inst_port`)
                    // tidak memuat nama instance. class_name handle interface
                    // tidak dipakai sebagai class object (hanya metadata).
                    if let (Some(iface_name), Some(inst_path)) =
                        (iface_name, top.signals[parent_sig].class_name)
                    {
                        self.iface_alias_jobs.borrow_mut().push((
                            *port_name,
                            Symbol::intern(iface_name),
                            inst_path,
                        ));
                    }
                }
            }
        }

        // Allocate parent signal IDs for unmapped child signals (internal signals)
        for (i, sig) in child.signals.iter().enumerate() {
            if sig_remap[i].is_none() {
                let new_id = next_parent_id;
                next_parent_id += 1;
                sig_remap[i] = Some(new_id);
                top.signals.push(SignalInfo {
                    name: Symbol::intern(&format!("{}.{}", inst.instance_name, sig.name)),
                    width: sig.width,
                    kind: sig.kind.clone(),
                    net_type: sig.net_type,
                    multi_driver: sig.multi_driver,
                    init_val: sig.init_val.clone(),
                    array_depth: sig.array_depth,
                    elem_width: sig.elem_width,
                    array_dims: sig.array_dims.clone(),
                    class_name: sig.class_name,
                    is_string: sig.is_string,
                    is_mailbox: sig.is_mailbox,
                    is_semaphore: sig.is_semaphore,
                    is_real: sig.is_real,
                    is_2state: sig.is_2state,
                    is_dynamic: sig.is_dynamic,
                    is_queue: sig.is_queue,
                    is_associative: sig.is_associative,
                    is_signed: sig.is_signed,
                    is_const: sig.is_const,
                    msb: sig.msb,
                    lsb: sig.lsb,
                    struct_fields: sig.struct_fields.clone(),
                    packed_dims: sig.packed_dims.clone(),
                    delay_rise: sig.delay_rise,
                    delay_fall: sig.delay_fall,
                    iface_type: None,
                    iface_modport: None,
                });
                // Also add to hier_signal_map: internal signals already have the right name in flat list
                hier_signal_map.insert(
                    Symbol::intern(&format!("{}.{}", inst.instance_name, sig.name)),
                    new_id,
                );
            }
        }

        let map_sig = |child_id: SignalId| -> SignalId {
            sig_remap.get(child_id).and_then(|&v| v).unwrap_or(child_id)
        };

        for process in &child.processes {
            let translated = self.translate_process(process, &map_sig)?;
            top.processes.push(translated);
        }
        Ok(())
    }

    fn translate_process(
        &self,
        process: &Process,
        map_sig: &dyn Fn(SignalId) -> SignalId,
    ) -> Result<Process, SimError> {
        match process {
            Process::Combinational {
                name,
                sensitivity,
                body,
            } => {
                let new_sens = sensitivity
                    .iter()
                    .map(|s| SignalSensitivity {
                        sig_id: map_sig(s.sig_id),
                        msb: s.msb,
                        lsb: s.lsb,
                    })
                    .collect();
                let new_body = self.translate_stmts(body, map_sig)?;
                Ok(Process::Combinational {
                    name: *name,
                    sensitivity: new_sens,
                    body: new_body,
                })
            }
            Process::CombReactive {
                name,
                sensitivity,
                body,
            } => {
                let new_sens = sensitivity
                    .iter()
                    .map(|s| SignalSensitivity {
                        sig_id: map_sig(s.sig_id),
                        msb: s.msb,
                        lsb: s.lsb,
                    })
                    .collect();
                let new_body = self.translate_stmts(body, map_sig)?;
                Ok(Process::CombReactive {
                    name: *name,
                    sensitivity: new_sens,
                    body: new_body,
                })
            }
            Process::Sequential {
                name,
                clock,
                reset,
                body,
                iff,
            } => {
                let new_clock = match clock {
                    ClockEdge::PosEdge(id) => ClockEdge::PosEdge(map_sig(*id)),
                    ClockEdge::NegEdge(id) => ClockEdge::NegEdge(map_sig(*id)),
                    // F27: clock hierarkis (`b.clk` via port interface) — path
                    // Symbol tidak di-remap; di-resolve engine via hier_signal_map.
                    ClockEdge::PosEdgeHier(s) => ClockEdge::PosEdgeHier(*s),
                    ClockEdge::NegEdgeHier(s) => ClockEdge::NegEdgeHier(*s),
                };
                let new_reset = reset.as_ref().map(|r| ResetInfo {
                    signal: map_sig(r.signal),
                    polarity: r.polarity,
                    r#async: r.r#async,
                    value: r.value.clone(),
                });
                let new_body = self.translate_stmts(body, map_sig)?;
                let new_iff = iff.as_ref().map(|ir| self.translate_expr(ir, map_sig));
                Ok(Process::Sequential {
                    name: *name,
                    clock: new_clock,
                    reset: new_reset,
                    body: new_body,
                    iff: new_iff,
                })
            }
            Process::Initial { name, body } => {
                let new_body = self.translate_stmts(body, map_sig)?;
                Ok(Process::Initial {
                    name: *name,
                    body: new_body,
                })
            }
            Process::AlwaysWithDelay { name, delay, body } => {
                let new_body = self.translate_stmts(body, map_sig)?;
                Ok(Process::AlwaysWithDelay {
                    name: *name,
                    delay: *delay,
                    body: new_body,
                })
            }
            Process::Final { name, body } => {
                let new_body = self.translate_stmts(body, map_sig)?;
                Ok(Process::Final {
                    name: *name,
                    body: new_body,
                })
            }
        }
    }

    fn translate_stmts(
        &self,
        stmts: &[IrStmt],
        map_sig: &dyn Fn(SignalId) -> SignalId,
    ) -> Result<Vec<IrStmt>, SimError> {
        stmts
            .iter()
            .map(|s| self.translate_stmt(s, map_sig))
            .collect()
    }

    fn translate_stmt(
        &self,
        stmt: &IrStmt,
        map_sig: &dyn Fn(SignalId) -> SignalId,
    ) -> Result<IrStmt, SimError> {
        match stmt {
            IrStmt::Block { stmts } => {
                let new = self.translate_stmts(stmts, map_sig)?;
                Ok(IrStmt::Block { stmts: new })
            }
            IrStmt::NamedBlock { name, stmts, decls } => {
                let new = self.translate_stmts(stmts, map_sig)?;
                Ok(IrStmt::NamedBlock {
                    name: *name,
                    stmts: new,
                    decls: decls.clone(),
                })
            }
            IrStmt::BlockingAssign { lhs, rhs, delay } => {
                let new_lhs = self.translate_lvalue(lhs, map_sig);
                let new_rhs = self.translate_expr(rhs, map_sig);
                Ok(IrStmt::BlockingAssign {
                    lhs: new_lhs,
                    rhs: new_rhs,
                    delay: *delay,
                })
            }
            IrStmt::NonBlockingAssign { lhs, rhs, delay } => {
                let new_lhs = self.translate_lvalue(lhs, map_sig);
                let new_rhs = self.translate_expr(rhs, map_sig);
                Ok(IrStmt::NonBlockingAssign {
                    lhs: new_lhs,
                    rhs: new_rhs,
                    delay: *delay,
                })
            }
            IrStmt::If {
                cond,
                true_branch,
                false_branch,
            } => {
                let new_cond = self.translate_expr(cond, map_sig);
                let new_true = self.translate_stmts(true_branch, map_sig)?;
                let new_false = self.translate_stmts(false_branch, map_sig)?;
                Ok(IrStmt::If {
                    cond: new_cond,
                    true_branch: new_true,
                    false_branch: new_false,
                })
            }
            IrStmt::Case {
                case_type,
                expr,
                items,
                default,
            } => {
                let new_expr = self.translate_expr(expr, map_sig);
                let new_items = items
                    .iter()
                    .map(|item| {
                        let labels = item
                            .labels
                            .iter()
                            .map(|l| self.translate_expr(l, map_sig))
                            .collect();
                        let body = self.translate_stmts(&item.body, map_sig)?;
                        Ok(IrCaseItem { labels, body })
                    })
                    .collect::<Result<Vec<_>, SimError>>()?;
                let new_default = self.translate_stmts(default, map_sig)?;
                Ok(IrStmt::Case {
                    case_type: case_type.clone(),
                    expr: new_expr,
                    items: new_items,
                    default: new_default,
                })
            }
            IrStmt::LoopFor {
                init,
                cond,
                step,
                body,
            } => {
                let new_init = init
                    .as_ref()
                    .map(|i| Box::new(self.translate_stmt(i, map_sig).unwrap_or(IrStmt::Null)));
                let new_cond = self.translate_expr(cond, map_sig);
                let new_step = step
                    .as_ref()
                    .map(|s| Box::new(self.translate_stmt(s, map_sig).unwrap_or(IrStmt::Null)));
                let new_body = self.translate_stmts(body, map_sig)?;
                Ok(IrStmt::LoopFor {
                    init: new_init,
                    cond: new_cond,
                    step: new_step,
                    body: new_body,
                })
            }
            IrStmt::LoopWhile { cond, body } => {
                let new_cond = self.translate_expr(cond, map_sig);
                let new_body = self.translate_stmts(body, map_sig)?;
                Ok(IrStmt::LoopWhile {
                    cond: new_cond,
                    body: new_body,
                })
            }
            IrStmt::LoopDoWhile { cond, body } => {
                let new_cond = self.translate_expr(cond, map_sig);
                let new_body = self.translate_stmts(body, map_sig)?;
                Ok(IrStmt::LoopDoWhile {
                    cond: new_cond,
                    body: new_body,
                })
            }
            IrStmt::Repeat { count, body } => {
                let new_count = self.translate_expr(count, map_sig);
                let new_body = self.translate_stmts(body, map_sig)?;
                Ok(IrStmt::Repeat {
                    count: new_count,
                    body: new_body,
                })
            }
            IrStmt::Foreach {
                array_var,
                index_var,
                body,
            } => {
                let new_var = self.translate_expr(array_var, map_sig);
                let new_body = self.translate_stmts(body, map_sig)?;
                Ok(IrStmt::Foreach {
                    array_var: new_var,
                    index_var: *index_var,
                    body: new_body,
                })
            }
            IrStmt::Delay { delay, body } => {
                let new_body = self.translate_stmts(body, map_sig)?;
                Ok(IrStmt::Delay {
                    delay: *delay,
                    body: new_body,
                })
            }
            IrStmt::Wait { cond, body } => {
                let new_cond = self.translate_expr(cond, map_sig);
                let new_body = self.translate_stmts(body, map_sig)?;
                Ok(IrStmt::Wait {
                    cond: new_cond,
                    body: new_body,
                })
            }
            IrStmt::WaitFork => Ok(IrStmt::WaitFork),
            IrStmt::SysCall {
                name,
                args,
                line,
                col,
            } => {
                let new_args = args
                    .iter()
                    .map(|a| self.translate_expr(a, map_sig))
                    .collect();
                Ok(IrStmt::SysCall {
                    name: *name,
                    args: new_args,
                    line: *line,
                    col: *col,
                })
            }
            IrStmt::EventControl { sigs, body, iff } => {
                let new_body = self.translate_stmts(body, map_sig)?;
                let new_sigs = sigs
                    .iter()
                    .map(|(sid, edge)| (map_sig(*sid), edge.clone()))
                    .collect();
                let new_iff = iff.as_ref().map(|ir| self.translate_expr(ir, map_sig));
                Ok(IrStmt::EventControl {
                    sigs: new_sigs,
                    body: new_body,
                    iff: new_iff,
                })
            }
            IrStmt::EventTrigger { sig_id } => Ok(IrStmt::EventTrigger {
                sig_id: map_sig(*sig_id),
            }),
            IrStmt::SysFinish => Ok(IrStmt::SysFinish),
            IrStmt::Null => Ok(IrStmt::Null),
            IrStmt::MethodCallStmt {
                obj,
                method,
                args,
                with_clause,
            } => Ok(IrStmt::MethodCallStmt {
                obj: self.translate_expr(obj, map_sig),
                method: *method,
                args: args
                    .iter()
                    .map(|a| self.translate_expr(a, map_sig))
                    .collect(),
                with_clause: with_clause
                    .as_ref()
                    .map(|wc| Box::new(self.translate_expr(wc, map_sig))),
            }),
            IrStmt::Break => Ok(IrStmt::Break),
            IrStmt::Continue => Ok(IrStmt::Continue),
            IrStmt::Disable { name } => Ok(IrStmt::Disable { name: *name }),
            IrStmt::Force { lvalue, rhs } => Ok(IrStmt::Force {
                lvalue: self.translate_lvalue(lvalue, map_sig),
                rhs: self.translate_expr(rhs, map_sig),
            }),
            IrStmt::Release { lvalue } => Ok(IrStmt::Release {
                lvalue: self.translate_lvalue(lvalue, map_sig),
            }),
            IrStmt::Deassign { lvalue } => Ok(IrStmt::Deassign {
                lvalue: self.translate_lvalue(lvalue, map_sig),
            }),
            IrStmt::Fork {
                processes,
                join_type,
            } => {
                let new_proc = processes
                    .iter()
                    .map(|p| self.translate_stmts(p, map_sig))
                    .collect::<Result<Vec<_>, SimError>>()?;
                Ok(IrStmt::Fork {
                    processes: new_proc,
                    join_type: join_type.clone(),
                })
            }
            IrStmt::Assert {
                cond,
                pass_stmt,
                fail_stmt,
                clock_event,
                disable_iff,
                sequence: _,
                line,
                col,
            } => {
                let new_cond = self.translate_expr(cond, map_sig);
                let new_pass = self.translate_stmts(pass_stmt, map_sig)?;
                let new_fail = self.translate_stmts(fail_stmt, map_sig)?;
                let new_disable = disable_iff
                    .as_ref()
                    .map(|e| Box::new(self.translate_expr(e, map_sig)));
                Ok(IrStmt::Assert {
                    cond: new_cond,
                    pass_stmt: new_pass,
                    fail_stmt: new_fail,
                    clock_event: clock_event.clone(),
                    disable_iff: new_disable,
                    sequence: None,
                    line: *line,
                    col: *col,
                })
            }
            IrStmt::Assume {
                cond,
                pass_stmt,
                fail_stmt,
                clock_event,
                disable_iff,
                sequence: _,
                line,
                col,
            } => {
                let new_cond = self.translate_expr(cond, map_sig);
                let new_pass = self.translate_stmts(pass_stmt, map_sig)?;
                let new_fail = self.translate_stmts(fail_stmt, map_sig)?;
                let new_disable = disable_iff
                    .as_ref()
                    .map(|e| Box::new(self.translate_expr(e, map_sig)));
                Ok(IrStmt::Assume {
                    cond: new_cond,
                    pass_stmt: new_pass,
                    fail_stmt: new_fail,
                    clock_event: clock_event.clone(),
                    disable_iff: new_disable,
                    sequence: None,
                    line: *line,
                    col: *col,
                })
            }
            // LANG-14: expect (procedural assertion) — pass-through sama
            // dengan Assert/Assume (translate cond + pass/fail stmts).
            IrStmt::Expect {
                cond,
                pass_stmt,
                fail_stmt,
                line,
                col,
            } => {
                let new_cond = self.translate_expr(cond, map_sig);
                let new_pass = self.translate_stmts(pass_stmt, map_sig)?;
                let new_fail = self.translate_stmts(fail_stmt, map_sig)?;
                Ok(IrStmt::Expect {
                    cond: new_cond,
                    pass_stmt: new_pass,
                    fail_stmt: new_fail,
                    line: *line,
                    col: *col,
                })
            }
            IrStmt::Cover {
                cond,
                pass_stmt,
                clock_event,
                disable_iff,
                sequence: _,
            } => {
                let new_cond = self.translate_expr(cond, map_sig);
                let new_pass = self.translate_stmts(pass_stmt, map_sig)?;
                let new_disable = disable_iff
                    .as_ref()
                    .map(|e| Box::new(self.translate_expr(e, map_sig)));
                Ok(IrStmt::Cover {
                    cond: new_cond,
                    pass_stmt: new_pass,
                    clock_event: clock_event.clone(),
                    disable_iff: new_disable,
                    sequence: None,
                })
            }
            IrStmt::WaitOrder {
                events,
                failure_stmts,
            } => {
                let new_events = events.iter().map(|id| map_sig(*id)).collect();
                let new_failure = self.translate_stmts(failure_stmts, map_sig)?;
                Ok(IrStmt::WaitOrder {
                    events: new_events,
                    failure_stmts: new_failure,
                })
            }
            IrStmt::RandCase { items } => {
                let new_items: Result<Vec<(IrExpr, Vec<IrStmt>)>, SimError> = items
                    .iter()
                    .map(|(weight_expr, body)| {
                        let new_weight = self.translate_expr(weight_expr, map_sig);
                        let new_body = self.translate_stmts(body, map_sig)?;
                        Ok((new_weight, new_body))
                    })
                    .collect();
                Ok(IrStmt::RandCase { items: new_items? })
            }
            IrStmt::RandSequence { productions } => {
                let new_prods: Result<Vec<(Symbol, Vec<(IrExpr, Vec<IrStmt>)>)>, SimError> =
                    productions
                        .iter()
                        .map(|(name, items)| {
                            let new_items: Vec<(IrExpr, Vec<IrStmt>)> = items
                                .iter()
                                .map(|(weight_expr, body)| {
                                    let new_weight = self.translate_expr(weight_expr, map_sig);
                                    let new_body =
                                        self.translate_stmts(body, map_sig).unwrap_or_default();
                                    (new_weight, new_body)
                                })
                                .collect();
                            Ok((*name, new_items))
                        })
                        .collect();
                Ok(IrStmt::RandSequence {
                    productions: new_prods?,
                })
            }
        }
    }

    fn translate_lvalue(&self, lv: &IrLValue, map_sig: &dyn Fn(SignalId) -> SignalId) -> IrLValue {
        match lv {
            IrLValue::Signal(id, w) => IrLValue::Signal(map_sig(*id), *w),
            IrLValue::RangeSelect(id, msb, lsb) => IrLValue::RangeSelect(map_sig(*id), *msb, *lsb),
            IrLValue::BitSelect(id, idx) => IrLValue::BitSelect(map_sig(*id), *idx),
            IrLValue::ArrayIndex {
                sig_id,
                index,
                elem_width,
            } => IrLValue::ArrayIndex {
                sig_id: map_sig(*sig_id),
                index: Box::new(self.translate_expr(index, map_sig)),
                elem_width: *elem_width,
            },
            IrLValue::ArrayRangeSelect {
                sig_id,
                index,
                elem_width,
                msb,
                lsb,
            } => IrLValue::ArrayRangeSelect {
                sig_id: map_sig(*sig_id),
                index: Box::new(self.translate_expr(index, map_sig)),
                elem_width: *elem_width,
                msb: *msb,
                lsb: *lsb,
            },
            IrLValue::ArrayBitSelect {
                sig_id,
                index,
                elem_width,
                bit,
            } => IrLValue::ArrayBitSelect {
                sig_id: map_sig(*sig_id),
                index: Box::new(self.translate_expr(index, map_sig)),
                elem_width: *elem_width,
                bit: Box::new(self.translate_expr(bit, map_sig)),
            },
            // Lvalue hierarkis: nama tidak ter-map (tidak ada SignalId);
            // index ekspresi tetap diterjemahkan.
            IrLValue::HierRef(name) => IrLValue::HierRef(*name),
            IrLValue::HierRefIndex { name, index } => IrLValue::HierRefIndex {
                name: *name,
                index: Box::new(self.translate_expr(index, map_sig)),
            },
            IrLValue::ExprPartSelect {
                sig_id,
                base,
                width,
            } => IrLValue::ExprPartSelect {
                sig_id: map_sig(*sig_id),
                base: Box::new(self.translate_expr(base, map_sig)),
                width: *width,
            },
            IrLValue::ObjectField { sig_id, field } => IrLValue::ObjectField {
                sig_id: map_sig(*sig_id),
                field: *field,
            },
            IrLValue::Concat(parts) => IrLValue::Concat(
                parts
                    .iter()
                    .map(|p| self.translate_lvalue(p, map_sig))
                    .collect(),
            ),
        }
    }

    fn translate_expr(&self, expr: &IrExpr, map_sig: &dyn Fn(SignalId) -> SignalId) -> IrExpr {
        match expr {
            IrExpr::Const(v) => IrExpr::Const(v.clone()),
            IrExpr::FillLit(val) => IrExpr::FillLit(*val),
            IrExpr::Signal(id, w) => IrExpr::Signal(map_sig(*id), *w),
            IrExpr::RangeSelect(id, msb, lsb) => IrExpr::RangeSelect(map_sig(*id), *msb, *lsb),
            IrExpr::BitSelect(id, idx) => IrExpr::BitSelect(map_sig(*id), *idx),
            IrExpr::ArrayIndex {
                sig_id,
                index,
                elem_width,
            } => IrExpr::ArrayIndex {
                sig_id: map_sig(*sig_id),
                index: Box::new(self.translate_expr(index, map_sig)),
                elem_width: *elem_width,
            },
            IrExpr::Concat(exprs) => IrExpr::Concat(
                exprs
                    .iter()
                    .map(|e| self.translate_expr(e, map_sig))
                    .collect(),
            ),
            IrExpr::Replicate(n, inner) => {
                IrExpr::Replicate(*n, Box::new(self.translate_expr(inner, map_sig)))
            }
            IrExpr::UnaryOp(op, inner) => {
                IrExpr::UnaryOp(op.clone(), Box::new(self.translate_expr(inner, map_sig)))
            }
            IrExpr::BinaryOp(op, l, r) => IrExpr::BinaryOp(
                op.clone(),
                Box::new(self.translate_expr(l, map_sig)),
                Box::new(self.translate_expr(r, map_sig)),
            ),
            IrExpr::Cond(c, t, f) => IrExpr::Cond(
                Box::new(self.translate_expr(c, map_sig)),
                Box::new(self.translate_expr(t, map_sig)),
                Box::new(self.translate_expr(f, map_sig)),
            ),
            IrExpr::Signed(inner) => IrExpr::Signed(Box::new(self.translate_expr(inner, map_sig))),
            IrExpr::String(s) => IrExpr::String(s.clone()),
            IrExpr::SysFunc {
                name,
                args,
                line,
                col,
            } => IrExpr::SysFunc {
                name: *name,
                args: args
                    .iter()
                    .map(|a| self.translate_expr(a, map_sig))
                    .collect(),
                line: *line,
                col: *col,
            },
            IrExpr::NewCall { class_name, args } => IrExpr::NewCall {
                class_name: *class_name,
                args: args
                    .iter()
                    .map(|a| self.translate_expr(a, map_sig))
                    .collect(),
            },
            IrExpr::This => IrExpr::This,
            IrExpr::MethodCall {
                obj,
                method,
                args,
                with_clause,
            } => IrExpr::MethodCall {
                obj: Box::new(self.translate_expr(obj, map_sig)),
                method: *method,
                args: args
                    .iter()
                    .map(|a| self.translate_expr(a, map_sig))
                    .collect(),
                with_clause: with_clause
                    .as_ref()
                    .map(|wc| Box::new(self.translate_expr(wc, map_sig))),
            },
            IrExpr::MemberAccess { obj, field } => IrExpr::MemberAccess {
                obj: Box::new(self.translate_expr(obj, map_sig)),
                field: *field,
            },
            IrExpr::ExprRangeSelect(inner, msb, lsb) => {
                IrExpr::ExprRangeSelect(Box::new(self.translate_expr(inner, map_sig)), *msb, *lsb)
            }
            IrExpr::ExprBitSelect(inner, idx) => {
                IrExpr::ExprBitSelect(Box::new(self.translate_expr(inner, map_sig)), *idx)
            }
            IrExpr::ExprPartSelect(inner, base_expr, width_expr) => IrExpr::ExprPartSelect(
                Box::new(self.translate_expr(inner, map_sig)),
                Box::new(self.translate_expr(base_expr, map_sig)),
                Box::new(self.translate_expr(width_expr, map_sig)),
            ),
            IrExpr::DpiCall {
                name,
                args,
                return_width,
            } => IrExpr::DpiCall {
                name: *name,
                args: args
                    .iter()
                    .map(|a| self.translate_expr(a, map_sig))
                    .collect(),
                return_width: *return_width,
            },
            IrExpr::HierRef(name) => IrExpr::HierRef(*name),
            IrExpr::Inside { expr, list } => IrExpr::Inside {
                expr: Box::new(self.translate_expr(expr, map_sig)),
                list: list
                    .iter()
                    .map(|e| self.translate_expr(e, map_sig))
                    .collect(),
            },
            IrExpr::InsideRange { expr, lo, hi } => IrExpr::InsideRange {
                expr: Box::new(self.translate_expr(expr, map_sig)),
                lo: Box::new(self.translate_expr(lo, map_sig)),
                hi: Box::new(self.translate_expr(hi, map_sig)),
            },
            IrExpr::Cast { width, expr } => IrExpr::Cast {
                width: *width,
                expr: Box::new(self.translate_expr(expr, map_sig)),
            },
            IrExpr::StreamingConcat {
                op,
                slice_size,
                slices,
            } => IrExpr::StreamingConcat {
                op: op.clone(),
                slice_size: *slice_size,
                slices: slices
                    .iter()
                    .map(|e| self.translate_expr(e, map_sig))
                    .collect(),
            },
            IrExpr::Dist { expr, items } => IrExpr::Dist {
                expr: Box::new(self.translate_expr(expr, map_sig)),
                items: items.clone(),
            },
            IrExpr::UdpLookup { udp_name, args } => IrExpr::UdpLookup {
                udp_name: *udp_name,
                args: args
                    .iter()
                    .map(|a| self.translate_expr(a, map_sig))
                    .collect(),
            },
            IrExpr::VifBinding { instance_name } => IrExpr::VifBinding {
                instance_name: *instance_name,
            },
            IrExpr::VirtualIfaceAccess {
                vif_name,
                field,
                field_width,
            } => IrExpr::VirtualIfaceAccess {
                vif_name: *vif_name,
                field: *field,
                field_width: *field_width,
            },
            IrExpr::FuncCall { func_name, args } => IrExpr::FuncCall {
                func_name: *func_name,
                args: args
                    .iter()
                    .map(|a| self.translate_expr(a, map_sig))
                    .collect(),
            },
            IrExpr::IncDec { read, lv, inc, pre } => IrExpr::IncDec {
                read: Box::new(self.translate_expr(read, map_sig)),
                lv: Box::new(self.translate_lvalue(lv, map_sig)),
                inc: *inc,
                pre: *pre,
            },
        }
    }
}
