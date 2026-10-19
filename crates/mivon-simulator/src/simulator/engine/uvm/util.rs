use super::super::SimulationEngine;
use mivon_ast::*;
use mivon_core::diagnostics::DiagCode;
use mivon_core::error::SimError;
use mivon_core::Symbol;
use mivon_ir::*;
use std::collections::HashMap;

/// LANG-55: pembanding kunci associative array — urutan lexicographic pada
/// bit-kanonik (width asc, lalu bit-string MSB→LSB). HashMap engine unordered,
/// sedangkan LRM §7.10.6 mensyaratkan iterator `first/next/last/prev`
/// ascending — tanpa sort ini urutan rawak antar-run.
pub(crate) fn assoc_key_cmp(a: &LogicVec, b: &LogicVec) -> std::cmp::Ordering {
    a.width.cmp(&b.width).then_with(|| {
        let sa = format!("{}", a);
        let sb = format!("{}", b);
        sa.cmp(&sb)
    })
}

impl SimulationEngine {
    pub(crate) fn execute_method_body(
        &mut self,
        obj_id: Option<ObjId>,
        method_def: &IrClassMethod,
        args: &[LogicVec],
        method: &str,
    ) -> Result<LogicVec, SimError> {
        // DEBUG trace method (env-gated): 1,2M+ baris spam saat korpus UVM
        // (opentitan) — kampanye besar kewalahan I/O. Hanya aktif eksplisit.
        if std::env::var("MIVON_METHOD_TRACE").is_ok() {
            eprintln!(
                "METHOD {} task={} obj={:?}",
                method, method_def.is_task, obj_id
            );
        }
        let old_this = self.current_this;
        if let Some(oid) = obj_id {
            self.current_this = Some(oid);
        }

        let mut local_signals: HashMap<Symbol, LogicVec> = HashMap::new();
        for (i, port) in method_def.ports.iter().enumerate() {
            let port_width = port.resolved_width(&HashMap::new()).unwrap_or(1);
            let val = if i < args.len() {
                // F18: jangan resize port 1-bit — `string name` / handle class
                // (`uvm_component parent`) di-resolve width 1, resize(1)
                // menghancurkan string & obj id (super.new(name, parent)
                // kehilangan argumen). Vector eksplisit ([7:0]) tetap resize.
                if port_width > 1 {
                    args[i].resize(port_width)
                } else {
                    args[i].clone()
                }
            } else {
                LogicVec::new(port_width)
            };
            local_signals.insert(port.name, val);
        }

        for decl in &method_def.decls {
            for dv in &decl.names {
                let w = dv.resolved_width(&HashMap::new()).unwrap_or(1);
                local_signals.insert(dv.name, LogicVec::new(w));
            }
        }

        let depth = self.method_locals.len();
        self.method_locals.push(local_signals);

        self.ast_loop_iters = 0;

        let old_method = self.current_method;
        self.current_method = Some(Symbol::intern(method));

        if !method_def.stmts.is_empty() {
            if method_def.is_task {
                // F26: fork id aktif (dari branch fork IR/AST) diteruskan ke
                // evaluasi task body — continuation resume (ContinueAstBlock)
                // membawa fork_id ini → event.rs fork_decrement saat selesai.
                // Sebelumnya selalu None → task di dalam `fork ... join`
                // (module initial) suspend dgn fork_id None → resume tak
                // pernah decrement → join selesai premature. `task_suspended`
                // memberitahu fork arm utk TIDAK decrement di titik ini
                // (resume yang decrement — cegah double-decrement).
                let completed = self
                    .evaluate_ast_block_with_delay_fork(&method_def.stmts, self.active_fork_id)?;
                if std::env::var("DBG_UVM").is_ok() {
                    eprintln!(
                        "[DBG-F26] task '{}' fid={:?} completed={} nstmts={}",
                        method,
                        self.active_fork_id,
                        completed,
                        method_def.stmts.len()
                    );
                }
                if !completed {
                    // Task suspended by delay — keep scope & context alive for
                    // continuation. F26 fix review: flag `task_suspended` HANYA
                    // di-set saat task berjalan di dalam branch fork
                    // (active_fork_id terisi) — di luar fork flag tidak relevan
                    // dan tidak boleh mengotori fork_branch_end berikutnya.
                    if self.active_fork_id.is_some() {
                        self.task_suspended = true;
                    }
                    // Propagasi suspensi ke pemanggil (counter — blok AST yang
                    // memanggil method ini berhenti, bukan lanjut buta; hang
                    // bug_0029).
                    self.method_suspend_count = self.method_suspend_count.wrapping_add(1);
                    self.current_method = old_method;
                    return Ok(LogicVec::new(0));
                }
            } else {
                let body = Stmt::Block {
                    stmts: method_def.stmts.clone(),
                };
                self.evaluate_ast_stmt(&body)?;
            }
        }

        // F35: `return` di body method menandai ast_return_pending (stop-blok
        // lintas nested). Clear di sini (wrapper method terluar) agar flag
        // tidak bocor ke evaluasi blok lain setelah method selesai.
        self.ast_return_pending = false;

        let return_val = if method_def.is_task {
            LogicVec::new(0) // tasks return void
        } else {
            self.get_local(method).unwrap_or_else(|| LogicVec::new(1))
        };

        self.current_method = old_method;
        self.method_locals.truncate(depth);
        self.current_this = old_this;
        Ok(return_val)
    }

    pub(crate) fn get_foreach_count(&self, array_var: &str) -> usize {
        if let Some(obj_id) = self.current_this {
            if let Some(obj) = self.state.get_object(obj_id) {
                if let Some(cls) = self.design.classes.get(&obj.class_name) {
                    for field in &cls.fields {
                        if field.name == array_var {
                            return field.array_depth;
                        }
                    }
                }
            }
        }
        1
    }

    pub(crate) fn get_field_elem_width(&self, expr: &Expr) -> Option<usize> {
        match expr {
            Expr::Ident { name, .. } => {
                if let Some(obj_id) = self.current_this {
                    if let Some(obj) = self.state.get_object(obj_id) {
                        if let Some(cls) = self.design.classes.get(&obj.class_name) {
                            for field in &cls.fields {
                                if field.name == *name && field.array_depth > 1 {
                                    return Some(field.elem_width);
                                }
                            }
                        }
                    }
                }
                None
            }
            Expr::MemberAccess { obj, field } => {
                if let Expr::Ident { name: s, .. } = obj.as_ref() {
                    if s == "this" {
                        if let Some(obj_id) = self.current_this {
                            if let Some(obj) = self.state.get_object(obj_id) {
                                if let Some(cls) = self.design.classes.get(&obj.class_name) {
                                    for f in &cls.fields {
                                        if f.name == *field && f.array_depth > 1 {
                                            return Some(f.elem_width);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Probe method di class hierarchy TANPA mencatat diagnostic (murni
    /// kueri — aman dipanggil oleh `.is_ok()`/`.is_some()`). Mengembalikan
    /// method dari walk, atau synthetic no-op untuk phase callback yang
    /// diwarisi dari library class, atau None bila method tidak ada.
    ///
    /// PENTING: tidak push ke diag_sink — pemanggil yang butuh error nyata
    /// (execute_method normal dispatch) harus mencatat sendiri via diag_error.
    pub(crate) fn find_method_quiet(
        &self,
        class_name: &str,
        method: &str,
    ) -> Option<IrClassMethod> {
        let mut current = class_name;
        while let Some(cls) = self.design.classes.get(&Symbol::intern(current)) {
            if let Some(m) = cls.methods.iter().find(|m| m.name == method) {
                return Some(m.clone());
            }
            if let Some(parent) = &cls.extends {
                current = parent.as_str();
            } else {
                break;
            }
        }
        // Hierarchy walk berhenti karena class/parent tidak ada di design
        // (library class yang tidak di-compile — UVM, CIP, DV environment).
        // Jika method adalah PHASE CALLBACK (no-op di base class library),
        // return synthetic method supaya callers tidak gagal RT8001.
        //
        // Contoh: i2c_scoreboard → cip_base_scoreboard → uvm_scoreboard.
        // cip_base_scoreboard tidak di-compile tapi phase callback-nya no-op.
        //
        // HANYA method PHASE CALLBACK yang di-synthesize — method builtin
        // NON-phase (get_next_item/item_done/set_sequencer/dll) tetap None
        // supaya dispatch ke execute_uvm_*_method builtin berjalan.
        const PHASE_CALLBACKS: &[&str] = &[
            "build_phase",
            "connect_phase",
            "end_of_elaboration_phase",
            "start_of_simulation_phase",
            "run_phase",
            "extract_phase",
            "check_phase",
            "report_phase",
            "final_phase",
            "reset_phase",
            "configure_phase",
            "main_phase",
            "shutdown_phase",
        ];
        if PHASE_CALLBACKS.contains(&method)
            && !self.design.classes.contains_key(&Symbol::intern(current))
        {
            return Some(IrClassMethod {
                name: Symbol::intern(method),
                is_task: method == "run_phase",
                virtual_flag: true,
                is_static: false,
                ports: vec![],
                decls: vec![],
                stmts: vec![],
            });
        }
        None
    }

    /// Cari method di class hierarchy; bila tidak ada, catat error ke
    /// diag_sink dan return Err. HANYA untuk pemanggil yang menginginkan
    /// error fatal (normal dispatch method yang benar-benar harus ada).
    pub(crate) fn find_method_in_hierarchy(
        &self,
        class_name: &str,
        method: &str,
    ) -> Result<IrClassMethod, SimError> {
        self.find_method_quiet(class_name, method).ok_or_else(|| {
            self.diag_error(
                DiagCode::DpiError,
                format!(
                    "method '{}' not found in class '{}' or its parents",
                    method, class_name
                ),
            )
        })
    }
}
impl SimulationEngine {
    /// Evaluasi `with` clause sebuah array method untuk satu elemen dan
    /// kembalikan NILAINYA (bukan bool). Dipakai reduction/ordering method
    /// (`sum`/`product`/`and`/`or`/`xor`/`min`/`max`/`unique`/`unique_index`/
    /// `sort`/`rsort`)
    /// di mana clause `with (item.x)` adalah ekspresi bernilai (LRM 1800
    /// §7.12.1). Bila tidak ada clause, kembalikan elemen itu sendiri.
    /// `item` di-bind ke `elem` lewat `method_locals`.
    pub(crate) fn eval_with_value(
        &mut self,
        with_clause: Option<&IrExpr>,
        elem: &LogicVec,
    ) -> Result<LogicVec, SimError> {
        match with_clause {
            Some(wc) => {
                let depth = self.method_locals.len();
                let mut scope = std::collections::HashMap::new();
                scope.insert(Symbol::intern("item"), elem.clone());
                self.method_locals.push(scope);
                let result = self.evaluate_expr(wc);
                self.method_locals.truncate(depth);
                result
            }
            None => Ok(elem.clone()),
        }
    }

    pub(crate) fn check_with_clause(
        &mut self,
        with_clause: Option<&IrExpr>,
        elem: &LogicVec,
    ) -> Result<bool, SimError> {
        match with_clause {
            Some(_) => Ok(self
                .eval_with_value(with_clause, elem)?
                .to_bool()
                .unwrap_or(false)),
            None => Ok(true),
        }
    }

    pub(crate) fn evaluate_array_method(
        &mut self,
        sig_id: SignalId,
        sig: &SignalInfo,
        method: &str,
        args: &[IrExpr],
        with_clause: Option<&IrExpr>,
    ) -> Result<LogicVec, SimError> {
        // Check if this is an associative array method
        if sig.is_associative {
            // Evaluate args first to avoid borrow conflicts with assoc_data access
            let args_eval: Vec<LogicVec> = args
                .iter()
                .map(|a| self.evaluate_expr(a))
                .collect::<Result<Vec<_>, SimError>>()?;
            let assoc_map = self.assoc_data.entry(sig_id).or_default();
            match method {
                "num" => {
                    let n = assoc_map.len();
                    return Ok(LogicVec::from_u64(n as u64, 32));
                }
                "delete" => {
                    if args_eval.is_empty() {
                        assoc_map.clear();
                    } else {
                        assoc_map.remove(&args_eval[0]);
                    }
                    return Ok(LogicVec::new(0));
                }
                "exists" => {
                    let found = assoc_map.contains_key(&args_eval[0]);
                    return Ok(LogicVec::from_u64(if found { 1 } else { 0 }, 1));
                }
                // LANG-55: iterator assoc array (LRM 1800 §7.10.6) —
                // `f = aa.first(idx)` dsb: arg = var index (out-arg). Sukses →
                // var DITULIS kunci target, nilai balik 1; array kosong / var
                // bukan signal → var tak berubah, balik 0.
                "first" | "last" => {
                    let mut keys: Vec<&LogicVec> = assoc_map.keys().collect();
                    keys.sort_by(|a, b| assoc_key_cmp(a, b));
                    let target = match method {
                        "first" => keys.first().map(|k| (*k).clone()),
                        _ => keys.last().map(|k| (*k).clone()),
                    };
                    if let Some(arg) = args.first() {
                        if let Some(idx_key) = target.clone() {
                            if let IrExpr::Signal(id, _) = arg {
                                self.state.write_signal(*id, idx_key);
                                return Ok(LogicVec::from_u64(1, 1));
                            }
                        }
                    }
                    if let Some(idx_key) = target {
                        // Fallback LRM-compat: bila arg bukan signal (mis.
                        // konstanta), balik kunci sebagai result (praktik lama).
                        return Ok(idx_key);
                    }
                    return Ok(LogicVec::from_u64(0, 1));
                }
                "next" => {
                    let mut keys: Vec<&LogicVec> = assoc_map.keys().collect();
                    keys.sort_by(|a, b| assoc_key_cmp(a, b));
                    let cur = match args.first() {
                        Some(IrExpr::Signal(id, _)) => self.state.read_signal(*id).clone(),
                        _ => args_eval.first().cloned().unwrap_or_default(),
                    };
                    // next: kunci pertama yang > cur (sudah ada idx: pasangan
                    // langsung; cur tak ada: smallest key > cur — LRM §7.10.6).
                    if let Some(k) = keys.iter().find(|k| assoc_key_cmp(k, &cur) == std::cmp::Ordering::Greater) {
                        let k = (*k).clone();
                        if let Some(IrExpr::Signal(id, _)) = args.first() {
                            self.state.write_signal(*id, k.clone());
                            return Ok(LogicVec::from_u64(1, 1));
                        }
                        return Ok(k);
                    }
                    return Ok(LogicVec::from_u64(0, 1));
                }
                "prev" => {
                    let mut keys: Vec<&LogicVec> = assoc_map.keys().collect();
                    keys.sort_by(|a, b| assoc_key_cmp(a, b));
                    let cur = match args.first() {
                        Some(IrExpr::Signal(id, _)) => self.state.read_signal(*id).clone(),
                        _ => args_eval.first().cloned().unwrap_or_default(),
                    };
                    // prev: kunci terbesar yang < cur (LRM §7.10.6).
                    if let Some(k) = keys
                        .iter()
                        .rev()
                        .find(|k| assoc_key_cmp(k, &cur) == std::cmp::Ordering::Less)
                    {
                        let k = (*k).clone();
                        if let Some(IrExpr::Signal(id, _)) = args.first() {
                            self.state.write_signal(*id, k.clone());
                            return Ok(LogicVec::from_u64(1, 1));
                        }
                        return Ok(k);
                    }
                    return Ok(LogicVec::from_u64(0, 1));
                }
                _ => {
                    // Fall through to default array methods (like push_back, etc.)
                }
            }
        }
        match method {
            "size" => {
                let lv = self.state.read_signal(sig_id);
                let count = lv.width.checked_div(sig.elem_width).unwrap_or(0);
                Ok(LogicVec::from_u64(count as u64, 32))
            }
            "delete" => {
                if let Some(index_expr) = args.first() {
                    let idx_val = self.evaluate_expr(index_expr)?;
                    let idx = idx_val.to_u64() as usize;
                    let lv = self.state.read_signal(sig_id);
                    let elem_width = sig.elem_width;
                    let count = lv.width.checked_div(elem_width).unwrap_or(0);
                    if idx >= count {
                        return Err(self.diag_error(
                            DiagCode::MemoryOutOfBounds,
                            format!("delete index {} out of range (size {})", idx, count),
                        ));
                    }
                    let before = lv.bits[..idx * elem_width].to_vec();
                    let after = lv.bits[(idx + 1) * elem_width..].to_vec();
                    let mut remaining = Vec::with_capacity(before.len() + after.len());
                    remaining.extend(before);
                    remaining.extend(after);
                    let new_lv = LogicVec {
                        width: remaining.len(),
                        bits: remaining,
                    };
                    self.state.write_signal(sig_id, new_lv);
                    Ok(LogicVec::new(0))
                } else {
                    self.state.write_signal(sig_id, LogicVec::new(0));
                    Ok(LogicVec::new(0))
                }
            }
            "pop_front" => {
                let lv = self.state.read_signal(sig_id);
                let elem_width = sig.elem_width;
                if lv.width < elem_width {
                    return Err(
                        self.diag_error(DiagCode::MemoryOutOfBounds, "pop_front on empty queue")
                    );
                }
                let mut bits = Vec::with_capacity(elem_width);
                for i in 0..elem_width {
                    bits.push(lv.bits.get(i).copied().unwrap_or(LogicVal::X));
                }
                let result = LogicVec {
                    width: elem_width,
                    bits,
                };
                let remaining = LogicVec {
                    width: lv.width - elem_width,
                    bits: lv.bits[elem_width..].to_vec(),
                };
                self.state.write_signal(sig_id, remaining);
                Ok(result)
            }
            "pop_back" => {
                let lv = self.state.read_signal(sig_id);
                let elem_width = sig.elem_width;
                if lv.width < elem_width {
                    return Err(
                        self.diag_error(DiagCode::MemoryOutOfBounds, "pop_back on empty queue")
                    );
                }
                let start = lv.width - elem_width;
                let mut bits = Vec::with_capacity(elem_width);
                for i in start..lv.width {
                    bits.push(lv.bits.get(i).copied().unwrap_or(LogicVal::X));
                }
                let result = LogicVec {
                    width: elem_width,
                    bits,
                };
                let remaining = LogicVec {
                    width: lv.width - elem_width,
                    bits: lv.bits[..start].to_vec(),
                };
                self.state.write_signal(sig_id, remaining);
                Ok(result)
            }
            "push_front" => {
                let arg_val = if let Some(a) = args.first() {
                    self.evaluate_expr(a)?
                } else {
                    return Err(
                        self.diag_error(DiagCode::DpiError, "push_front expects 1 argument")
                    );
                };
                let elem_width = sig.elem_width;
                let padded = if arg_val.width >= elem_width {
                    let bits = arg_val.bits[..elem_width].to_vec();
                    LogicVec {
                        width: elem_width,
                        bits,
                    }
                } else {
                    let mut bits = arg_val.bits.clone();
                    bits.resize(elem_width, LogicVal::X);
                    LogicVec {
                        width: elem_width,
                        bits,
                    }
                };
                let mut existing = self.state.read_signal(sig_id).clone();
                let mut new_bits = Vec::with_capacity(existing.width + elem_width);
                new_bits.extend(padded.bits.iter().copied());
                new_bits.extend(existing.bits.iter().copied());
                existing.bits = new_bits;
                existing.width += elem_width;
                self.state.write_signal(sig_id, existing);
                Ok(LogicVec::new(0))
            }
            "exists" => {
                let index_expr = args.first().ok_or_else(|| {
                    self.diag_error(DiagCode::DpiError, "exists expects 1 argument")
                })?;
                let idx_val = self.evaluate_expr(index_expr)?;
                let idx = idx_val.to_u64() as usize;
                let lv = self.state.read_signal(sig_id);
                let elem_width = sig.elem_width;
                let count = lv.width.checked_div(elem_width).unwrap_or(0);
                Ok(LogicVec::from_u64(if idx < count { 1 } else { 0 }, 1))
            }
            "push_back" => {
                let arg_val = if let Some(a) = args.first() {
                    self.evaluate_expr(a)?
                } else {
                    return Err(self.diag_error(DiagCode::DpiError, "push_back expects 1 argument"));
                };
                let elem_width = sig.elem_width;
                let padded = if arg_val.width >= elem_width {
                    let bits = arg_val.bits[..elem_width].to_vec();
                    LogicVec {
                        width: elem_width,
                        bits,
                    }
                } else {
                    let mut bits = arg_val.bits.clone();
                    bits.resize(elem_width, LogicVal::X);
                    LogicVec {
                        width: elem_width,
                        bits,
                    }
                };
                let mut existing = self.state.read_signal(sig_id).clone();
                existing.bits.extend(padded.bits.iter().copied());
                existing.width += elem_width;
                self.state.write_signal(sig_id, existing);
                Ok(LogicVec::new(0))
            }
            "insert" => {
                if args.len() < 2 {
                    return Err(self.diag_error(
                        DiagCode::DpiError,
                        "insert expects 2 arguments (index, value)",
                    ));
                }
                let idx_val = self.evaluate_expr(&args[0])?;
                let idx = idx_val.to_u64() as usize;
                let arg_val = self.evaluate_expr(&args[1])?;
                let elem_width = sig.elem_width;
                let padded = if arg_val.width >= elem_width {
                    let bits = arg_val.bits[..elem_width].to_vec();
                    LogicVec {
                        width: elem_width,
                        bits,
                    }
                } else {
                    let mut bits = arg_val.bits.clone();
                    bits.resize(elem_width, LogicVal::X);
                    LogicVec {
                        width: elem_width,
                        bits,
                    }
                };
                let mut existing = self.state.read_signal(sig_id).clone();
                let count = existing.width.checked_div(elem_width).unwrap_or(0);
                let pos = idx.min(count);
                let mut new_bits = Vec::with_capacity(existing.width + elem_width);
                new_bits.extend(existing.bits[..pos * elem_width].iter().copied());
                new_bits.extend(padded.bits.iter().copied());
                new_bits.extend(existing.bits[pos * elem_width..].iter().copied());
                existing.bits = new_bits;
                existing.width += elem_width;
                self.state.write_signal(sig_id, existing);
                Ok(LogicVec::new(0))
            }
            "reverse" => {
                let mut lv = self.state.read_signal(sig_id).clone();
                let elem_width = sig.elem_width;
                if elem_width > 0 {
                    let count = lv.width.checked_div(elem_width).unwrap_or(0);
                    let mut new_bits = Vec::with_capacity(lv.width);
                    for i in (0..count).rev() {
                        for j in 0..elem_width {
                            new_bits.push(lv.bits[i * elem_width + j]);
                        }
                    }
                    lv.bits = new_bits;
                }
                self.state.write_signal(sig_id, lv);
                Ok(LogicVec::new(0))
            }
            "sort" => {
                let lv = self.state.read_signal(sig_id).clone();
                let elem_width = sig.elem_width;
                if elem_width > 0 {
                    let count = lv.width.checked_div(elem_width).unwrap_or(0);
                    // Kunci pengurutan = nilai `with (item.x)` bila ada clause
                    // (LRM 1800 §7.12.1), jika tidak = elemen itu sendiri.
                    let mut keyed: Vec<(u64, LogicVec)> = Vec::with_capacity(count);
                    for i in 0..count {
                        let mut bits = Vec::with_capacity(elem_width);
                        for j in 0..elem_width {
                            bits.push(lv.bits[i * elem_width + j]);
                        }
                        let elem = LogicVec {
                            width: elem_width,
                            bits,
                        };
                        let key = self.eval_with_value(with_clause, &elem)?.to_u64();
                        keyed.push((key, elem));
                    }
                    keyed.sort_by_key(|a| a.0);
                    let mut new_bits = Vec::with_capacity(lv.width);
                    for (_, e) in &keyed {
                        new_bits.extend(e.bits.iter().copied());
                    }
                    let sorted = LogicVec {
                        width: lv.width,
                        bits: new_bits,
                    };
                    self.state.write_signal(sig_id, sorted);
                }
                Ok(LogicVec::new(0))
            }
            "rsort" => {
                let lv = self.state.read_signal(sig_id).clone();
                let elem_width = sig.elem_width;
                if elem_width > 0 {
                    let count = lv.width.checked_div(elem_width).unwrap_or(0);
                    let mut keyed: Vec<(u64, LogicVec)> = Vec::with_capacity(count);
                    for i in 0..count {
                        let mut bits = Vec::with_capacity(elem_width);
                        for j in 0..elem_width {
                            bits.push(lv.bits[i * elem_width + j]);
                        }
                        let elem = LogicVec {
                            width: elem_width,
                            bits,
                        };
                        let key = self.eval_with_value(with_clause, &elem)?.to_u64();
                        keyed.push((key, elem));
                    }
                    keyed.sort_by_key(|a| std::cmp::Reverse(a.0));
                    let mut new_bits = Vec::with_capacity(lv.width);
                    for (_, e) in &keyed {
                        new_bits.extend(e.bits.iter().copied());
                    }
                    let sorted = LogicVec {
                        width: lv.width,
                        bits: new_bits,
                    };
                    self.state.write_signal(sig_id, sorted);
                }
                Ok(LogicVec::new(0))
            }
            "shuffle" => {
                let lv = self.state.read_signal(sig_id).clone();
                let elem_width = sig.elem_width;
                if elem_width > 0 {
                    let count = lv.width.checked_div(elem_width).unwrap_or(0);
                    let mut elems: Vec<LogicVec> = (0..count)
                        .map(|i| {
                            let mut bits = Vec::with_capacity(elem_width);
                            for j in 0..elem_width {
                                bits.push(lv.bits[i * elem_width + j]);
                            }
                            LogicVec {
                                width: elem_width,
                                bits,
                            }
                        })
                        .collect();
                    use rand::seq::SliceRandom;
                    elems.shuffle(&mut rand::thread_rng());
                    let mut new_bits = Vec::with_capacity(lv.width);
                    for e in &elems {
                        new_bits.extend(e.bits.iter().copied());
                    }
                    let shuffled = LogicVec {
                        width: lv.width,
                        bits: new_bits,
                    };
                    self.state.write_signal(sig_id, shuffled);
                }
                Ok(LogicVec::new(0))
            }
            // --- Reduction methods ---
            "sum" => {
                let lv = self.state.read_signal(sig_id).clone();
                let elem_width = sig.elem_width;
                if elem_width > 0 {
                    let count = lv.width.checked_div(elem_width).unwrap_or(0);
                    let mut result: u64 = 0;
                    for i in 0..count {
                        let mut bits = Vec::with_capacity(elem_width);
                        for j in 0..elem_width {
                            bits.push(lv.bits[i * elem_width + j]);
                        }
                        let elem = LogicVec {
                            width: elem_width,
                            bits,
                        };
                        let v = self.eval_with_value(with_clause, &elem)?;
                        result = result.wrapping_add(v.to_u64());
                    }
                    Ok(LogicVec::from_u64(result, elem_width.max(32)))
                } else {
                    Ok(LogicVec::new(0))
                }
            }
            "product" => {
                let lv = self.state.read_signal(sig_id).clone();
                let elem_width = sig.elem_width;
                if elem_width > 0 {
                    let count = lv.width.checked_div(elem_width).unwrap_or(0);
                    let mut result: u64 = 1;
                    for i in 0..count {
                        let mut bits = Vec::with_capacity(elem_width);
                        for j in 0..elem_width {
                            bits.push(lv.bits[i * elem_width + j]);
                        }
                        let elem = LogicVec {
                            width: elem_width,
                            bits,
                        };
                        let v = self.eval_with_value(with_clause, &elem)?;
                        result = result.wrapping_mul(v.to_u64());
                    }
                    Ok(LogicVec::from_u64(result, elem_width.max(32)))
                } else {
                    Ok(LogicVec::new(0))
                }
            }
            "and" => {
                let lv = self.state.read_signal(sig_id).clone();
                let elem_width = sig.elem_width;
                if elem_width > 0 && lv.width >= elem_width {
                    let count = lv.width / elem_width;
                    let mut result = LogicVec::fill(LogicVal::One, elem_width);
                    for i in 0..count {
                        let mut bits = Vec::with_capacity(elem_width);
                        for j in 0..elem_width {
                            let idx = i * elem_width + j;
                            bits.push(lv.bits.get(idx).copied().unwrap_or(LogicVal::X));
                        }
                        let elem = LogicVec {
                            width: elem_width,
                            bits,
                        };
                        let v = self.eval_with_value(with_clause, &elem)?;
                        for (j, bit) in v.bits.iter().enumerate().take(elem_width) {
                            if bit == &LogicVal::Zero {
                                result.bits[j] = LogicVal::Zero;
                            }
                        }
                    }
                    Ok(result)
                } else {
                    Ok(LogicVec::fill(LogicVal::One, elem_width.max(1)))
                }
            }
            "or" => {
                let lv = self.state.read_signal(sig_id).clone();
                let elem_width = sig.elem_width;
                if elem_width > 0 && lv.width >= elem_width {
                    let count = lv.width / elem_width;
                    let mut result = LogicVec::fill(LogicVal::Zero, elem_width);
                    for i in 0..count {
                        let mut bits = Vec::with_capacity(elem_width);
                        for j in 0..elem_width {
                            let idx = i * elem_width + j;
                            bits.push(lv.bits.get(idx).copied().unwrap_or(LogicVal::X));
                        }
                        let elem = LogicVec {
                            width: elem_width,
                            bits,
                        };
                        let v = self.eval_with_value(with_clause, &elem)?;
                        for (j, bit) in v.bits.iter().enumerate().take(elem_width) {
                            if bit == &LogicVal::One {
                                result.bits[j] = LogicVal::One;
                            }
                        }
                    }
                    Ok(result)
                } else {
                    Ok(LogicVec::fill(LogicVal::Zero, elem_width.max(1)))
                }
            }
            "xor" => {
                let lv = self.state.read_signal(sig_id).clone();
                let elem_width = sig.elem_width;
                if elem_width > 0 && lv.width >= elem_width {
                    let count = lv.width / elem_width;
                    let mut result = LogicVec::fill(LogicVal::Zero, elem_width);
                    for i in 0..count {
                        let mut bits = Vec::with_capacity(elem_width);
                        for j in 0..elem_width {
                            let idx = i * elem_width + j;
                            bits.push(lv.bits.get(idx).copied().unwrap_or(LogicVal::X));
                        }
                        let elem = LogicVec {
                            width: elem_width,
                            bits,
                        };
                        let v = self.eval_with_value(with_clause, &elem)?;
                        for (j, bit) in v.bits.iter().enumerate().take(elem_width) {
                            if bit == &LogicVal::One {
                                result.bits[j] = match result.bits[j] {
                                    LogicVal::Zero => LogicVal::One,
                                    LogicVal::One => LogicVal::Zero,
                                    other => other,
                                };
                            }
                        }
                    }
                    Ok(result)
                } else {
                    Ok(LogicVec::fill(LogicVal::Zero, elem_width.max(1)))
                }
            }
            // --- Ordering methods ---
            // LRM 1800 §7.12.1: `min`/`max` mengembalikan ELEMEN yang
            // kunci-nya (`with (item…)`, default = elemen itu sendiri)
            // minimum/maksimum — BUKAN nilai kuncinya.
            "min" => {
                let lv = self.state.read_signal(sig_id).clone();
                let elem_width = sig.elem_width;
                if elem_width > 0 && lv.width >= elem_width {
                    let count = lv.width / elem_width;
                    let mut best: Option<(u64, LogicVec)> = None;
                    for i in 0..count {
                        let mut bits = Vec::with_capacity(elem_width);
                        for j in 0..elem_width {
                            bits.push(lv.bits[i * elem_width + j]);
                        }
                        let elem = LogicVec {
                            width: elem_width,
                            bits,
                        };
                        let key = self.eval_with_value(with_clause, &elem)?.to_u64();
                        if best.as_ref().is_none_or(|(k, _)| key < *k) {
                            best = Some((key, elem));
                        }
                    }
                    Ok(best.map(|(_, e)| e).unwrap_or_else(|| LogicVec::new(1)))
                } else {
                    Ok(LogicVec::new(1))
                }
            }
            "max" => {
                let lv = self.state.read_signal(sig_id).clone();
                let elem_width = sig.elem_width;
                if elem_width > 0 && lv.width >= elem_width {
                    let count = lv.width / elem_width;
                    let mut best: Option<(u64, LogicVec)> = None;
                    for i in 0..count {
                        let mut bits = Vec::with_capacity(elem_width);
                        for j in 0..elem_width {
                            bits.push(lv.bits[i * elem_width + j]);
                        }
                        let elem = LogicVec {
                            width: elem_width,
                            bits,
                        };
                        let key = self.eval_with_value(with_clause, &elem)?.to_u64();
                        if best.as_ref().is_none_or(|(k, _)| key > *k) {
                            best = Some((key, elem));
                        }
                    }
                    Ok(best.map(|(_, e)| e).unwrap_or_else(|| LogicVec::new(1)))
                } else {
                    Ok(LogicVec::new(1))
                }
            }
            "unique" => {
                let lv = self.state.read_signal(sig_id).clone();
                let elem_width = sig.elem_width;
                if elem_width > 0 && lv.width >= elem_width {
                    let count = lv.width / elem_width;
                    let mut seen = std::collections::HashSet::new();
                    let mut new_bits = Vec::new();
                    for i in 0..count {
                        let mut bits = Vec::with_capacity(elem_width);
                        for j in 0..elem_width {
                            bits.push(lv.bits[i * elem_width + j]);
                        }
                        let elem = LogicVec {
                            width: elem_width,
                            bits,
                        };
                        let key = self.eval_with_value(with_clause, &elem)?.to_u64();
                        if seen.insert(key) {
                            for j in 0..elem_width {
                                let idx = i * elem_width + j;
                                new_bits.push(lv.bits.get(idx).copied().unwrap_or(LogicVal::X));
                            }
                        }
                    }
                    let result = LogicVec {
                        width: new_bits.len(),
                        bits: new_bits,
                    };
                    // LRM 1800 §7.12.1: `unique` MENGEMBALIKAN queue baru dan
                    // TIDAK mengubah array sumber (beda dgn `sort`/`reverse`
                    // yang in-place). Dulu write_signal(sig_id,..) + return
                    // kosong → `q.unique().size()` gagal (RT0001).
                    return Ok(result);
                }
                Ok(LogicVec::new(0))
            }
            "unique_index" => {
                // LRM 1800 §7.12.1: seperti `unique` tetapi mengembalikan queue
                // INDEKS elemen unik (bukan nilainya); tiap indeks 32-bit.
                // Catatan: kunci dedup via `to_u64()` (sama seperti `unique`) —
                // elemen >64-bit / X-Z bisa ter-truncate; keterbatasan turunan,
                // bukan regresi.
                let lv = self.state.read_signal(sig_id).clone();
                let elem_width = sig.elem_width;
                if elem_width > 0 && lv.width >= elem_width {
                    let count = lv.width / elem_width;
                    let mut seen = std::collections::HashSet::new();
                    let mut new_bits = Vec::new();
                    for i in 0..count {
                        let mut bits = Vec::with_capacity(elem_width);
                        for j in 0..elem_width {
                            bits.push(lv.bits[i * elem_width + j]);
                        }
                        let elem = LogicVec {
                            width: elem_width,
                            bits,
                        };
                        let key = self.eval_with_value(with_clause, &elem)?.to_u64();
                        if seen.insert(key) {
                            let idx_vec = LogicVec::from_u64(i as u64, 32);
                            new_bits.extend(idx_vec.bits.iter());
                        }
                    }
                    return Ok(LogicVec {
                        width: new_bits.len(),
                        bits: new_bits,
                    });
                }
                Ok(LogicVec::new(0))
            }
            // --- Locator methods ---
            "find" | "find_first" | "find_last" => {
                let lv = self.state.read_signal(sig_id).clone();
                let elem_width = sig.elem_width;
                if elem_width > 0 && lv.width >= elem_width {
                    let count = lv.width / elem_width;
                    if with_clause.is_some() {
                        // If with_clause is provided, iterate and find matching elements
                        let mut result_elems: Vec<LogicVec> = Vec::new();
                        if method == "find_last" {
                            for i in (0..count).rev() {
                                let mut bits = Vec::with_capacity(elem_width);
                                for j in 0..elem_width {
                                    bits.push(lv.bits[i * elem_width + j]);
                                }
                                let elem = LogicVec {
                                    width: elem_width,
                                    bits,
                                };
                                if self.check_with_clause(with_clause, &elem)? {
                                    // §7.12.1: `find_last` mengembalikan
                                    // elemen TERAKHIR yang cocok (bukan semua).
                                    result_elems.push(elem);
                                    break;
                                }
                            }
                        } else {
                            for i in 0..count {
                                let mut bits = Vec::with_capacity(elem_width);
                                for j in 0..elem_width {
                                    bits.push(lv.bits[i * elem_width + j]);
                                }
                                let elem = LogicVec {
                                    width: elem_width,
                                    bits,
                                };
                                if self.check_with_clause(with_clause, &elem)? {
                                    result_elems.push(elem);
                                    if method == "find_first" {
                                        break;
                                    }
                                }
                            }
                        }
                        let total_width = result_elems.len() * elem_width;
                        let mut all_bits = Vec::with_capacity(total_width);
                        for e in &result_elems {
                            all_bits.extend(e.bits.iter());
                        }
                        return Ok(LogicVec {
                            width: total_width,
                            bits: all_bits,
                        });
                    }
                    if method == "find_first" && count > 0 {
                        let mut bits = Vec::with_capacity(elem_width);
                        bits.extend(lv.bits.iter().take(elem_width));
                        return Ok(LogicVec {
                            width: elem_width,
                            bits,
                        });
                    }
                    if method == "find_last" && count > 0 {
                        let start = (count - 1) * elem_width;
                        let mut bits = Vec::with_capacity(elem_width);
                        for j in 0..elem_width {
                            bits.push(lv.bits[start + j]);
                        }
                        return Ok(LogicVec {
                            width: elem_width,
                            bits,
                        });
                    }
                    // "find" returns all elements (same as array)
                    return Ok(lv);
                }
                Ok(LogicVec::new(0))
            }
            "find_index" | "find_first_index" | "find_last_index" => {
                let lv = self.state.read_signal(sig_id).clone();
                let elem_width = sig.elem_width;
                if elem_width > 0 && lv.width >= elem_width {
                    let count = lv.width / elem_width;
                    if with_clause.is_some() {
                        let mut indices: Vec<u64> = Vec::new();
                        if method == "find_last_index" {
                            for i in (0..count).rev() {
                                let mut bits = Vec::with_capacity(elem_width);
                                for j in 0..elem_width {
                                    bits.push(lv.bits[i * elem_width + j]);
                                }
                                let elem = LogicVec {
                                    width: elem_width,
                                    bits,
                                };
                                if self.check_with_clause(with_clause, &elem)? {
                                    // §7.12.1: `find_last_index` mengembalikan
                                    // indeks TERAKHIR yang cocok.
                                    indices.push(i as u64);
                                    break;
                                }
                            }
                        } else {
                            for i in 0..count {
                                let mut bits = Vec::with_capacity(elem_width);
                                for j in 0..elem_width {
                                    bits.push(lv.bits[i * elem_width + j]);
                                }
                                let elem = LogicVec {
                                    width: elem_width,
                                    bits,
                                };
                                if self.check_with_clause(with_clause, &elem)? {
                                    indices.push(i as u64);
                                    if method == "find_first_index" {
                                        break;
                                    }
                                }
                            }
                        }
                        let mut bits = Vec::new();
                        for idx in &indices {
                            let idx_vec = LogicVec::from_u64(*idx, 32);
                            bits.extend(idx_vec.bits.iter());
                        }
                        return Ok(LogicVec {
                            width: bits.len(),
                            bits,
                        });
                    }
                    // Return indices as 32-bit values packed into result
                    if method == "find_first_index" && count > 0 {
                        return Ok(LogicVec::from_u64(0, 32));
                    }
                    if method == "find_last_index" && count > 0 {
                        return Ok(LogicVec::from_u64((count - 1) as u64, 32));
                    }
                    // "find_index" returns all indices (0..count) as a packed queue
                    let mut bits = Vec::new();
                    for i in 0..count {
                        let idx_vec = LogicVec::from_u64(i as u64, 32);
                        bits.extend(idx_vec.bits.iter());
                    }
                    return Ok(LogicVec {
                        width: bits.len(),
                        bits,
                    });
                }
                Ok(LogicVec::new(0))
            }
            _ => Err(self.diag_error(
                DiagCode::NotImplemented,
                format!("unknown array/queue method: {}", method),
            )),
        }
    }
}
