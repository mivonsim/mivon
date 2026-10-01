use super::super::SimulationEngine;
use crate::simulator::types::*;
use crate::simulator::util::*;
use crate::simulator::value::*;
use mivon_ast::*;
use mivon_core::diagnostics::DiagCode;
use mivon_core::error::SimError;
use mivon_core::Symbol;
use mivon_ir::*;
use std::collections::HashMap;

impl SimulationEngine {
    pub(crate) fn evaluate_ast_expr(&mut self, expr: &Expr) -> Result<LogicVec, SimError> {
        match expr {
            Expr::Value(v) => {
                match v {
                    Value::Decimal(i) => Ok(LogicVec::from_u64(*i as u64, 32)),
                    Value::Binary { bits, .. } => LogicVec::from_bin(bits)
                        .map_err(|e| SimError::with_diag(DiagCode::DpiError, e)),
                    Value::Hex { bits, .. } => LogicVec::from_hex(bits)
                        .map_err(|e| SimError::with_diag(DiagCode::DpiError, e)),
                    Value::Octal { bits, .. } => LogicVec::from_hex(bits)
                        .map_err(|e| SimError::with_diag(DiagCode::DpiError, e)),
                    Value::Real(r) => Ok(LogicVec::from_u64(r.to_bits(), 64)),
                }
            }
            Expr::Ident { name, line, col } => {
                // F20: catat posisi source terakhir yang diketahui agar
                // warning/error runtime selalu punya file:line:col.
                self.set_cur_src_pos(*line, *col);
                if name == "this" {
                    if let Some(obj_id) = self.current_this {
                        return Ok(LogicVec::from_u64(obj_id as u64, 64));
                    } else {
                        return Err(self.diag_error_at(
                            DiagCode::NullHandle,
                            "'this' used outside of class method",
                            *line,
                            *col,
                        ));
                    }
                }
                // F18: `uvm_test_top` — handle global ke root test UVM (jalur
                // AST, sama seperti jalur IR di eval/expr.rs SysFunc).
                if name == "uvm_test_top" {
                    return Ok(LogicVec::from_u64(
                        self.root_test_obj_id.unwrap_or(0) as u64,
                        64,
                    ));
                }
                // LANG-40: ident `let` class tanpa parameter — evaluasi body.
                if let Some(ld) = self.class_let_decl(name).cloned() {
                    if ld.params.is_empty() {
                        return self.evaluate_ast_expr(&ld.expr);
                    }
                }
                if let Some(local) = self.get_local(name.as_str()) {
                    return Ok(local);
                }
                if let Some(obj_id) = self.current_this {
                    if let Some(obj) = self.state.get_object(obj_id) {
                        if let Some(val) = obj.fields.get(name) {
                            return Ok(val.clone());
                        }
                    }
                }
                if let Some(sig_id) = self.find_signal(name.as_str()) {
                    return Ok(self.state.read_signal(sig_id).clone());
                }
                let ctx = self
                    .current_this
                    .map(|id| format!("obj_id={}", id))
                    .unwrap_or_else(|| "no current_this".to_string());
                // Identifier method-context yang tak bisa di-resolve (mis. field
                // class yang belum di-assign / null handle pada kode UVM) di-
                // perlakukan sebagai warning + null default agar simulasi tetap
                // berjalan, bukan mematikan seluruh run.
                self.diag_warn_at(
                    DiagCode::NullHandle,
                    format!(
                        "cannot resolve identifier '{}' in method context ({}); using null default",
                        name, ctx
                    ),
                    *line,
                    *col,
                );
                Ok(LogicVec::from_u64(0, 64))
            }
            Expr::BinaryOp { op, lhs, rhs } => {
                let lval = self.evaluate_ast_expr(lhs)?;
                let rval = self.evaluate_ast_expr(rhs)?;
                let ir_op = map_ast_binary_op(op)?;
                Ok(eval_binary(ir_op, &lval, &rval))
            }
            Expr::UnaryOp { op, expr: inner } => {
                let val = self.evaluate_ast_expr(inner)?;
                let ir_op = map_ast_unary_op(op)?;
                Ok(eval_unary(ir_op, &val))
            }
            Expr::Concat(parts) => {
                let mut result = LogicVec::new(0);
                for p in parts.iter().rev() {
                    let part = self.evaluate_ast_expr(p)?;
                    result = result.extend(&part);
                }
                Ok(result)
            }
            Expr::Replicate { count, expr: inner } => {
                let count_val = self.evaluate_ast_expr(count)?;
                let n = count_val.to_u64() as usize;
                let val = self.evaluate_ast_expr(inner)?;
                let mut result = LogicVec::new(0);
                for _ in 0..n {
                    result = result.extend(&val);
                }
                Ok(result)
            }
            Expr::TernaryOp {
                cond,
                true_expr,
                false_expr,
            } => {
                let cval = self.evaluate_ast_expr(cond)?;
                let tv = self.evaluate_ast_expr(true_expr)?;
                let fv = self.evaluate_ast_expr(false_expr)?;
                let w = tv.width.max(fv.width);
                let tv = if tv.width < w { tv.resize(w) } else { tv };
                let fv = if fv.width < w { fv.resize(w) } else { fv };
                match cval.to_bool() {
                    Some(true) => Ok(tv),
                    Some(false) => Ok(fv),
                    // IEEE 1800-2017 §11.4.11 — kondisi unknown/z → kombinasi
                    // bitwise (sama dgn jalur IR IrExpr::Cond di eval/expr.rs;
                    // perbaiki keduanya agar konsisten).
                    None => {
                        let mut bits = Vec::with_capacity(w);
                        for i in 0..w {
                            let out = match (tv.bits.get(i), fv.bits.get(i)) {
                                (Some(LogicVal::Zero), Some(LogicVal::Zero)) => LogicVal::Zero,
                                (Some(LogicVal::One), Some(LogicVal::One)) => LogicVal::One,
                                _ => LogicVal::X,
                            };
                            bits.push(out);
                        }
                        Ok(LogicVec { bits, width: w })
                    }
                }
            }
            Expr::FuncCall { name, args, .. } if name == "new" => {
                // F17: alokasi object — class di-resolve di situs assignment
                // (resolve_new_class_hint) karena evaluator ekspresi tidak tahu
                // tipe LHS. Di sini (konteks non-assignment) class kosong.
                let arg_vals: Vec<LogicVec> = args
                    .iter()
                    .map(|a| self.evaluate_ast_expr(a))
                    .collect::<Result<_, _>>()?;
                self.allocate_new_object(None, &arg_vals)
            }
            Expr::FuncCall { name, args, .. } if name.ends_with("::new") => {
                let raw_name = name.strip_suffix("::new").unwrap().to_string();
                let is_builtin = matches!(
                    raw_name.as_str(),
                    "uvm_object"
                        | "uvm_component"
                        | "uvm_sequence_item"
                        | "uvm_sequence"
                        | "uvm_sequencer"
                        | "uvm_driver"
                        | "uvm_monitor"
                        | "uvm_scoreboard"
                        | "uvm_analysis_port"
                        | "uvm_analysis_imp"
                        | "uvm_analysis_export"
                        | "uvm_subscriber"
                        | "uvm_tlm_fifo"
                        | "uvm_test"
                        | "uvm_report_object"
                        | "uvm_factory"
                        | "uvm_resource_db"
                );
                let effective = if is_builtin {
                    format!("__{}", raw_name)
                } else {
                    raw_name.clone()
                };
                let effective = self
                    .factory_type_overrides
                    .get::<str>(effective.as_str())
                    .unwrap_or(&effective)
                    .clone();
                let obj_id = self.state.alloc_object(Symbol::intern(&effective));
                let arg_vals: Vec<LogicVec> = args
                    .iter()
                    .map(|a| self.evaluate_ast_expr(a))
                    .collect::<Result<_, _>>()?;
                // Initialize built-in data
                if is_builtin {
                    if raw_name == "uvm_analysis_port" {
                        let pname = if !arg_vals.is_empty() {
                            logicvec_to_string(&arg_vals[0])
                        } else {
                            String::new()
                        };
                        self.uvm_analysis_port_data.insert(
                            obj_id,
                            UvmAnalysisPortData {
                                connections: Vec::new(),
                                name: pname.clone(),
                            },
                        );
                        self.uvm_object_data
                            .entry(obj_id)
                            .or_insert_with(|| UvmObjectData { name: pname });
                    } else if raw_name == "uvm_analysis_imp" {
                        let pname = if !arg_vals.is_empty() {
                            logicvec_to_string(&arg_vals[0])
                        } else {
                            String::new()
                        };
                        let parent_obj = arg_vals.get(1).map(|a| a.to_u64() as ObjId).unwrap_or(0);
                        self.uvm_analysis_imp_data.insert(
                            obj_id,
                            UvmAnalysisImpData {
                                parent: if parent_obj != 0 {
                                    Some(parent_obj)
                                } else {
                                    None
                                },
                                name: pname.clone(),
                            },
                        );
                        self.uvm_object_data
                            .entry(obj_id)
                            .or_insert_with(|| UvmObjectData { name: pname });
                    }
                }
                // F21-F24: builtin UVM punya `methods: vec![]` (no "new" di
                // extends chain) — tapi `new` wajib di-dispatch ke
                // execute_uvm_*_method utk meng-insert data / auto-buat child.
                if self.uvm_needs_new_dispatch(&effective) {
                    self.execute_method(obj_id, "new", &arg_vals)?;
                }
                Ok(LogicVec::from_u64(obj_id as u64, 64))
            }
            Expr::FuncCall { name, args, .. } if name == "uvm_config_db::set" => {
                let arg_vals: Vec<LogicVec> = args
                    .iter()
                    .map(|a| self.evaluate_ast_expr(a))
                    .collect::<Result<_, _>>()?;
                let inst_name = if arg_vals.len() > 1 {
                    logicvec_to_string(&arg_vals[1])
                } else {
                    String::new()
                };
                let field_name = if arg_vals.len() > 2 {
                    logicvec_to_string(&arg_vals[2])
                } else {
                    String::new()
                };
                let value = if arg_vals.len() > 3 {
                    arg_vals[3].clone()
                } else {
                    LogicVec::new(1)
                };
                self.uvm_config_db_data
                    .insert((inst_name.clone(), field_name.clone()), value);
                // VERIF-06: wait_modified ter-blokir menunggu key ini → bangun.
                self.config_db_release_waiters(&inst_name, &field_name)?;
                Ok(LogicVec::from_u64(1, 1))
            }
            // VERIF-06: uvm_config_db::exists(inst, field) → 1/0 (non-blocking).
            Expr::FuncCall { name, args, .. } if name == "uvm_config_db::exists" => {
                let arg_vals: Vec<LogicVec> = args
                    .iter()
                    .map(|a| self.evaluate_ast_expr(a))
                    .collect::<Result<_, _>>()?;
                let mut inst_name = if arg_vals.len() > 1 {
                    logicvec_to_string(&arg_vals[1])
                } else {
                    String::new()
                };
                let field_name = if arg_vals.len() > 2 {
                    logicvec_to_string(&arg_vals[2])
                } else {
                    String::new()
                };
                if inst_name.is_empty() {
                    if let Some(oid) = self.current_this {
                        inst_name = self.uvm_object_full_path(oid);
                    }
                }
                Ok(LogicVec::from_u64(
                    if self.config_db_exists(&inst_name, &field_name) {
                        1
                    } else {
                        0
                    },
                    1,
                ))
            }
            // VERIF-06: uvm_config_db::wait_modified(inst, field) — BLOCKING;
            // di-intercept block.rs (waiter keyed by (inst,field), release oleh
            // set). Di sini (konteks non-blocking/ekspresi) cek kondisi terkini.
            Expr::FuncCall { name, args, .. } if name == "uvm_config_db::wait_modified" => {
                let arg_vals: Vec<LogicVec> = args
                    .iter()
                    .map(|a| self.evaluate_ast_expr(a))
                    .collect::<Result<_, _>>()?;
                let inst_name = if arg_vals.len() > 1 {
                    logicvec_to_string(&arg_vals[1])
                } else {
                    String::new()
                };
                let field_name = if arg_vals.len() > 2 {
                    logicvec_to_string(&arg_vals[2])
                } else {
                    String::new()
                };
                Ok(LogicVec::from_u64(
                    if self.config_db_exists(&inst_name, &field_name) {
                        1
                    } else {
                        0
                    },
                    1,
                ))
            }
            Expr::FuncCall { name, args, .. } if name == "uvm_config_db::get" => {
                let arg_vals: Vec<LogicVec> = args
                    .iter()
                    .map(|a| self.evaluate_ast_expr(a))
                    .collect::<Result<_, _>>()?;
                let mut inst_name = if arg_vals.len() > 1 {
                    logicvec_to_string(&arg_vals[1])
                } else {
                    String::new()
                };
                let field_name = if arg_vals.len() > 2 {
                    logicvec_to_string(&arg_vals[2])
                } else {
                    String::new()
                };
                // F19: inst_path kosong (`get(this, "", ...)`) → resolve ke
                // path hierarki penuh objek saat ini.
                if inst_name.is_empty() {
                    if let Some(oid) = self.current_this {
                        inst_name = self.uvm_object_full_path(oid);
                    }
                }
                // F19: exact match menang, lalu wildcard paling spesifik.
                let stored = self.config_db_find(&inst_name, &field_name);
                if let Some(val) = stored {
                    if let Some(last_arg) = args.get(3) {
                        match last_arg {
                            Expr::Ident { name: var, .. } => {
                                self.write_local_or_field(var.as_str(), val.clone())?;
                            }
                            Expr::MemberAccess { obj, field } => {
                                let obj_val = self.evaluate_ast_expr(obj)?;
                                let obj_id = obj_val.to_u64() as ObjId;
                                if let Some(obj) = self.state.get_object_mut(obj_id) {
                                    obj.fields.insert(*field, val.clone());
                                }
                            }
                            _ => {}
                        }
                    }
                    Ok(LogicVec::from_u64(1, 1))
                } else {
                    Ok(LogicVec::from_u64(0, 1))
                }
            }
            Expr::FuncCall { name, args, .. } if name == "uvm_resource_db::set" => {
                let arg_vals: Vec<LogicVec> = args
                    .iter()
                    .map(|a| self.evaluate_ast_expr(a))
                    .collect::<Result<_, _>>()?;
                let scope = if !arg_vals.is_empty() {
                    logicvec_to_string(&arg_vals[0])
                } else {
                    String::new()
                };
                let rname = if arg_vals.len() > 1 {
                    logicvec_to_string(&arg_vals[1])
                } else {
                    String::new()
                };
                let value = if arg_vals.len() > 2 {
                    arg_vals[2].clone()
                } else {
                    LogicVec::new(1)
                };
                self.uvm_resource_db_data.insert((scope, rname), value);
                Ok(LogicVec::from_u64(1, 1))
            }
            Expr::FuncCall { name, args, .. }
                if name == "uvm_resource_db::get" || name == "uvm_resource_db::read_by_name" =>
            {
                // VERIF-07: lookup exact dulu, lalu wildcard scope paling
                // spesifik — `set("*.env", ...)` terbaca `get("tb.env", ...)`.
                let arg_vals: Vec<LogicVec> = args
                    .iter()
                    .map(|a| self.evaluate_ast_expr(a))
                    .collect::<Result<_, _>>()?;
                let scope = if !arg_vals.is_empty() {
                    logicvec_to_string(&arg_vals[0])
                } else {
                    String::new()
                };
                let rname = if arg_vals.len() > 1 {
                    logicvec_to_string(&arg_vals[1])
                } else {
                    String::new()
                };
                let stored = self.resource_db_find(&scope, &rname);
                if let Some(val) = stored {
                    if let Some(last_arg) = args.get(2) {
                        match last_arg {
                            Expr::Ident { name: var, .. } => {
                                self.write_local_or_field(var.as_str(), val.clone())?;
                            }
                            Expr::MemberAccess { obj, field } => {
                                let obj_val = self.evaluate_ast_expr(obj)?;
                                let obj_id = obj_val.to_u64() as ObjId;
                                if let Some(obj) = self.state.get_object_mut(obj_id) {
                                    obj.fields.insert(*field, val.clone());
                                }
                            }
                            _ => {}
                        }
                    }
                    Ok(LogicVec::from_u64(1, 1))
                } else {
                    Ok(LogicVec::from_u64(0, 1))
                }
            }
            Expr::FuncCall { name, args, .. } if name == "uvm_resource_db::exists" => {
                let arg_vals: Vec<LogicVec> = args
                    .iter()
                    .map(|a| self.evaluate_ast_expr(a))
                    .collect::<Result<_, _>>()?;
                let scope = if !arg_vals.is_empty() {
                    logicvec_to_string(&arg_vals[0])
                } else {
                    String::new()
                };
                let rname = if arg_vals.len() > 1 {
                    logicvec_to_string(&arg_vals[1])
                } else {
                    String::new()
                };
                Ok(LogicVec::from_u64(
                    if self.resource_db_exists(&scope, &rname) {
                        1
                    } else {
                        0
                    },
                    1,
                ))
            }
            Expr::FuncCall { name, args, .. } if name == "uvm_resource_db::write_by_name" => {
                // VERIF-07: alias set dgn arg ke-4 rw access type (diabaikan).
                let arg_vals: Vec<LogicVec> = args
                    .iter()
                    .map(|a| self.evaluate_ast_expr(a))
                    .collect::<Result<_, _>>()?;
                let scope = if !arg_vals.is_empty() {
                    logicvec_to_string(&arg_vals[0])
                } else {
                    String::new()
                };
                let rname = if arg_vals.len() > 1 {
                    logicvec_to_string(&arg_vals[1])
                } else {
                    String::new()
                };
                let value = if arg_vals.len() > 2 {
                    arg_vals[2].clone()
                } else {
                    LogicVec::new(1)
                };
                self.uvm_resource_db_data.insert((scope, rname), value);
                Ok(LogicVec::from_u64(1, 1))
            }
            Expr::FuncCall { name, args, .. }
                if name == "uvm_factory::set_type_override_by_type" =>
            {
                let arg_vals: Vec<LogicVec> = args
                    .iter()
                    .map(|a| self.evaluate_ast_expr(a))
                    .collect::<Result<_, _>>()?;
                let orig = if !arg_vals.is_empty() {
                    logicvec_to_string(&arg_vals[0])
                } else {
                    String::new()
                };
                let override_type = if arg_vals.len() > 1 {
                    logicvec_to_string(&arg_vals[1])
                } else {
                    String::new()
                };
                self.factory_type_overrides.insert(orig, override_type);
                Ok(LogicVec::from_u64(1, 1))
            }
            Expr::FuncCall {
                name,
                args,
                line,
                col,
                ..
            } => {
                // LANG-40: panggilan `let name(args)` di class — substitusi
                // parameter dengan argumen lalu evaluasi body.
                if let Some(ld) = self.class_let_decl(name).cloned() {
                    if !ld.params.is_empty() && ld.params.len() == args.len() {
                        let map: HashMap<Symbol, &Expr> = ld
                            .params
                            .iter()
                            .zip(args.iter())
                            .map(|(p, a)| (*p, a))
                            .collect();
                        let body =
                            mivon_ast::inline_util::substitute_let_args(ld.expr.clone(), &map);
                        return self.evaluate_ast_expr(&body);
                    }
                }
                // F20: catat posisi call agar warning runtime punya file:line:col.
                self.set_cur_src_pos(*line, *col);
                let arg_vals: Vec<LogicVec> = args
                    .iter()
                    .map(|a| self.evaluate_ast_expr(a))
                    .collect::<Result<_, _>>()?;
                if name == "$clog2" {
                    if let Some(arg) = args.first() {
                        let val = self.evaluate_ast_expr(arg)?;
                        let n = val.to_u64();
                        if n <= 1 {
                            return Ok(LogicVec::from_u64(0, 32));
                        }
                        let msb = (64 - n.leading_zeros()) as u64;
                        let result = if n.is_power_of_two() { msb - 1 } else { msb };
                        return Ok(LogicVec::from_u64(result, 32));
                    }
                }
                if name == "run_test" {
                    // F18: run_test("name") dari body method class (jalur AST).
                    // Guard uvm_phases_started mencegah eksekusi ganda.
                    let test_name = args
                        .first()
                        .map(|a| self.evaluate_ast_expr(a))
                        .transpose()?
                        .map(|v| logicvec_to_string(&v))
                        .unwrap_or_default();
                    self.run_uvm_test(&test_name)?;
                    return Ok(LogicVec::from_u64(1, 1));
                }
                // VERIF-04: uvm_root::run_test("name") — varian class-method,
                // sama dgn bare run_test (F18).
                if name == "uvm_root::run_test" {
                    let test_name = args
                        .first()
                        .map(|a| self.evaluate_ast_expr(a))
                        .transpose()?
                        .map(|v| logicvec_to_string(&v))
                        .unwrap_or_default();
                    self.run_uvm_test(&test_name)?;
                    return Ok(LogicVec::from_u64(1, 1));
                }
                // VERIF-04: uvm_root::get() — singleton handle.
                if name == "uvm_root::get" {
                    let obj_id = match self.uvm_root_id {
                        Some(id) => id,
                        None => {
                            let id = self.state.alloc_object(Symbol::intern("uvm_root"));
                            self.uvm_root_id = Some(id);
                            id
                        }
                    };
                    return Ok(LogicVec::from_u64(obj_id as u64, 64));
                }
                // VERIF-04: uvm_root::get_top() — komponen top (uvm_test_top).
                if name == "uvm_root::get_top" {
                    return Ok(LogicVec::from_u64(
                        self.root_test_obj_id.unwrap_or(0) as u64,
                        64,
                    ));
                }
                // VERIF-18: uvm_tr_database — singleton db + stream/record query.
                if name == "uvm_tr_database::get_db" {
                    let obj_id = match self.uvm_tr_db_id {
                        Some(id) => id,
                        None => {
                            let id = self.state.alloc_object(Symbol::intern("uvm_tr_database"));
                            self.uvm_tr_db_id = Some(id);
                            id
                        }
                    };
                    return Ok(LogicVec::from_u64(obj_id as u64, 64));
                }
                if name == "uvm_tr_database::get_stream" {
                    let stream_name = args
                        .first()
                        .map(|a| self.evaluate_ast_expr(a))
                        .transpose()?
                        .map(|v| logicvec_to_string(&v))
                        .unwrap_or_default();
                    let id = self.tr_stream_get(&stream_name);
                    return Ok(LogicVec::from_u64(id as u64, 64));
                }
                if name == "uvm_tr_database::get_tr_count" {
                    return Ok(LogicVec::from_u64(self.tr_records.len() as u64, 64));
                }
                if name == "uvm_tr_database::set_stream" {
                    let stream_name = args
                        .first()
                        .map(|a| self.evaluate_ast_expr(a))
                        .transpose()?
                        .map(|v| logicvec_to_string(&v))
                        .unwrap_or_default();
                    self.tr_stream_get(&stream_name);
                    self.tr_db_default_stream = Some(stream_name);
                    return Ok(LogicVec::from_u64(0, 64));
                }
                if name == "$sformatf" {
                    // F17: $sformatf di body method class — format via
                    // format_display_ast (argumen AST, field class ter-resolve).
                    let msg = self.format_display_ast(args);
                    let mut bits = Vec::with_capacity(msg.len() * 8);
                    for c in msg.chars() {
                        let byte = c as u8;
                        for i in 0..8 {
                            bits.push(if (byte >> i) & 1 == 1 {
                                LogicVal::One
                            } else {
                                LogicVal::Zero
                            });
                        }
                    }
                    return Ok(LogicVec {
                        width: bits.len(),
                        bits,
                    });
                }
                // Pemanggilan method class tanpa `this.` prefix — dispatch ke
                // object saat ini bila method ada di hierarki class-nya.
                if let Some(obj_id) = self.current_this {
                    if let Some(obj) = self.state.get_object(obj_id) {
                        if !obj.class_name.is_empty()
                            && self
                                .find_method_quiet(obj.class_name.as_str(), name.as_str())
                                .is_some()
                        {
                            return self.execute_method(obj_id, name.as_str(), &arg_vals);
                        }
                    }
                }
                // F18: method builtin UVM dipanggil tanpa `this.` di body task
                // komponen (pola driver `get_next_item(it)` / `item_done()`).
                // Dispatch ke builtin handler via execute_method (hanya di
                // jalur fallback — override user tetap menang di atas).
                // get_next_item bersifat output by-ref: item id hasil di-tulis
                // balik ke argumen pertama bila berupa local/field (pola UVM
                // `task get_next_item(output RSP item)`).
                if matches!(
                    name.as_str(),
                    "get_next_item"
                        | "try_next_item"
                        | "item_done"
                        | "get_response"
                        | "put_response"
                        | "start_item"
                        | "finish_item"
                        | "set_sequencer"
                        | "get_sequencer"
                ) {
                    if let Some(obj_id) = self.current_this {
                        if let Some(obj) = self.state.get_object(obj_id) {
                            let cn = obj.class_name.as_str();
                            if std::env::var("DBG_UVM").is_ok()
                                && matches!(
                                    name.as_str(),
                                    "start_item" | "finish_item" | "set_sequencer"
                                )
                            {
                                eprintln!(
                                    "[DBG-UVM] fallback dispatch {} cn={} drv={} seqr={} seq={}",
                                    name,
                                    cn,
                                    self.is_uvm_driver_hierarchy(cn),
                                    self.is_uvm_sequencer_hierarchy(cn),
                                    self.is_uvm_sequence_hierarchy(cn)
                                );
                            }
                            if self.is_uvm_driver_hierarchy(cn)
                                || self.is_uvm_sequencer_hierarchy(cn)
                                || self.is_uvm_monitor_hierarchy(cn)
                                || (self.is_uvm_sequence_hierarchy(cn)
                                    && matches!(
                                        name.as_str(),
                                        "start_item"
                                            | "finish_item"
                                            | "set_sequencer"
                                            | "get_sequencer"
                                    ))
                            {
                                let r = self.execute_method(obj_id, name.as_str(), &arg_vals)?;
                                if std::env::var("DBG_UVM").is_ok() {
                                    eprintln!(
                                        "[DBG-UVM] {} on {} -> r={} width={}",
                                        name,
                                        cn,
                                        r.to_u64(),
                                        r.width
                                    );
                                }
                                if matches!(name.as_str(), "get_next_item" | "try_next_item") {
                                    if let Some(Expr::Ident { name: var, .. }) = args.first() {
                                        self.write_local_or_field(var.as_str(), r.clone())?;
                                    }
                                }
                                return Ok(r);
                            }
                        }
                    }
                }
                // F16: uvm_report_* dipanggil tanpa `this.` prefix di body
                // method (pola standar UVM `uvm_report_info(id, msg, verb)`).
                // HANYA di jalur fallback (setelah hierarchy lookup gagal,
                // sama dengan pola method.rs:151 "only intercept if class
                // doesn't override") — override user tetap menang. Dispatch ke
                // report builtin → emit_severity (counter + fatal_hit).
                if matches!(
                    name.as_str(),
                    "uvm_report_info"
                        | "uvm_report_warning"
                        | "uvm_report_error"
                        | "uvm_report_fatal"
                ) {
                    if let Some(obj_id) = self.current_this {
                        return self.execute_uvm_report_object_method(
                            obj_id,
                            name.as_str(),
                            &arg_vals,
                        );
                    }
                }
                // F35: function module-level (recursive, tidak di-inline) —
                // dispatch ke helper runtime yang sama dengan jalur IR. Tanpa
                // ini, pemanggilan REKURSIF di dalam body function (yang
                // dieksekusi via AST eval) jatuh ke fallback RT9003 di bawah
                // → hasil 0 (bug siluman: fact(5) = 0 padahal 120).
                if self.design.module_functions.contains_key(name) {
                    return self.execute_module_function_call(name, &arg_vals);
                }
                // F20: gunakan posisi call terakhir yang diketahui (bukan 0,0)
                // agar warning punya file:line:col.
                let (w_line, w_col) = self.cur_src_pos();
                self.diag_warn_at(
                    DiagCode::NotImplemented,
                    format!(
                        "unknown function '{}' in method context; using null default",
                        name
                    ),
                    w_line,
                    w_col,
                );
                Ok(LogicVec::from_u64(0, 64))
            }
            Expr::MethodCall {
                obj,
                method,
                args,
                with_clause,
            } => {
                if let Expr::Ident { name: s, .. } = obj.as_ref() {
                    if s == "super" {
                        let arg_vals: Vec<LogicVec> = args
                            .iter()
                            .map(|a| self.evaluate_ast_expr(a))
                            .collect::<Result<_, _>>()?;
                        return self.execute_super_method(method.as_str(), &arg_vals);
                    }
                }
                let obj_val = self.evaluate_ast_expr(obj)?;
                let obj_id = obj_val.to_u64() as ObjId;
                let arg_vals: Vec<LogicVec> = args
                    .iter()
                    .map(|a| self.evaluate_ast_expr(a))
                    .collect::<Result<_, _>>()?;
                // F17: randomize() with {...} di jalur AST (body method class) —
                // inline constraint AST ditangani solver via
                // execute_randomize_ast_with (evaluate_ast_expr + current_this
                // → field class bisa diakses), bukan execute_method yang
                // mengabaikan with_clause.
                if method == "randomize" && with_clause.is_some() {
                    let class_name = self
                        .state
                        .get_object(obj_id)
                        .map(|o| o.class_name)
                        .unwrap_or_default();
                    return self.execute_randomize_ast_with(
                        obj_id,
                        class_name.as_str(),
                        with_clause.as_deref(),
                    );
                }
                self.execute_method(obj_id, method.as_str(), &arg_vals)
            }
            Expr::MemberAccess { obj, field } => {
                // Try hierarchical signal reference first
                let hier_name = Self::build_hier_name(obj, field.as_str());
                if let Some(sig_id) = self.find_signal(&hier_name) {
                    return Ok(self.state.read_signal(sig_id).clone());
                }
                // Fall back to object field access (class objects)
                let obj_val = self.evaluate_ast_expr(obj)?;
                let obj_id = obj_val.to_u64() as ObjId;
                // Handle OOB (stale/garbage id dari signal handle yang belum
                // ter-alloc di run ini) → null default, bukan error — konsisten
                // dgn handle null (id 0) yang sudah graceful. Hard error di
                // sini menghentikan seluruh sim (E9001) utk bug handle.
                let Some(obj_data) = self.state.get_object(obj_id) else {
                    return Ok(LogicVec::new(1));
                };
                Ok(obj_data
                    .fields
                    .get(field)
                    .cloned()
                    .unwrap_or_else(|| LogicVec::new(1)))
            }
            Expr::BitSelect { expr: inner, index } => {
                let val = self.evaluate_ast_expr(inner)?;
                let idx_val = self.evaluate_ast_expr(index)?;
                let i = idx_val.to_u64() as usize;
                // Check if this is an array field access (extract element, not bit)
                if let Some(elem_width) = self.get_field_elem_width(inner) {
                    let start = i.saturating_mul(elem_width);
                    let end = start.saturating_add(elem_width).min(val.width);
                    // Guard OOB (index X/negatif → huge): return X elemen.
                    if start >= end || start >= val.bits.len() {
                        return Ok(LogicVec::fill(LogicVal::X, elem_width));
                    }
                    let mut bits = val.bits[start..end].to_vec();
                    if bits.len() < elem_width {
                        bits.resize(elem_width, LogicVal::X);
                    }
                    Ok(LogicVec {
                        width: bits.len(),
                        bits,
                    })
                } else {
                    let bit = val.bits.get(i).copied().unwrap_or(LogicVal::X);
                    Ok(LogicVec {
                        bits: vec![bit],
                        width: 1,
                    })
                }
            }
            Expr::RangeSelect {
                expr: inner,
                msb,
                lsb,
            } => {
                let val = self.evaluate_ast_expr(inner)?;
                let msb_val = self.evaluate_ast_expr(msb)?;
                let lsb_val = self.evaluate_ast_expr(lsb)?;
                let m = msb_val.to_u64() as usize;
                let l = lsb_val.to_u64() as usize;
                let (start, end) = if m > l { (l, m) } else { (m, l) };
                // Guard OOB (nilai msb/lsb dari ekspresi yang nilainya X/negatif
                // bisa jadi huge): return X, jangan panic slice.
                let n = val.bits.len();
                if start >= n || end >= n || start > end {
                    let w = (end.saturating_sub(start).saturating_add(1)).max(1);
                    return Ok(LogicVec::fill(LogicVal::X, w));
                }
                let bits = val.bits[start..=end].to_vec();
                Ok(LogicVec {
                    width: bits.len(),
                    bits,
                })
            }
            Expr::PartSelect {
                expr: inner,
                base,
                width,
            } => {
                let val = self.evaluate_ast_expr(inner)?;
                let base_val = self.evaluate_ast_expr(base)?;
                let width_val = self.evaluate_ast_expr(width)?;
                let b = base_val.to_u64() as usize;
                let w = width_val.to_u64() as usize;
                let in_bounds = b <= val.width && w > 0 && b.saturating_add(w) <= val.width;
                if in_bounds {
                    let bits = val.bits[b..b + w].to_vec();
                    Ok(LogicVec { width: w, bits })
                } else if w == 0 {
                    Ok(LogicVec::from_u64(0, 1))
                } else {
                    // Out-of-range part-select → X (LRM §11.5.1), bukan error.
                    Ok(LogicVec::fill(LogicVal::X, w.min(val.width.max(1))))
                }
            }
            Expr::Paren(inner) => self.evaluate_ast_expr(inner),
            Expr::String(s) => {
                let mut bits = Vec::with_capacity(s.len() * 8);
                for c in s.chars() {
                    let byte = c as u8;
                    for i in 0..8 {
                        bits.push(if (byte >> i) & 1 == 1 {
                            LogicVal::One
                        } else {
                            LogicVal::Zero
                        });
                    }
                }
                Ok(LogicVec {
                    width: bits.len(),
                    bits,
                })
            }
            Expr::Null => Ok(LogicVec::from_u64(0, 64)),
            Expr::FillLit(v) => Ok(LogicVec::fill(*v, 1)),
            Expr::Inside {
                expr: inner,
                range_list,
            } => {
                let val = self.evaluate_ast_expr(inner)?;
                for item in range_list {
                    // Range `[lo:hi]` di-parse SV sebagai RangeSelect dengan
                    // base 0 (`Expr::Value(Decimal(0))`) — evaluasi sebagai
                    // rentang, bukan bit-select. SV memakai notasi descending
                    // `[msb:lsb]` (`[1:10]` → msb=1, lsb=10) jadi ambil min/max.
                    // Base selain 0 = member select asli (`inside {x[7:0]}`)
                    // → evaluasi sebagai nilai tunggal, bukan rentang.
                    if let Expr::RangeSelect {
                        expr: base,
                        msb,
                        lsb,
                        ..
                    } = item
                    {
                        if matches!(base.as_ref(), Expr::Value(Value::Decimal(0))) {
                            let a = self.evaluate_ast_expr(msb)?.to_u64();
                            let b = self.evaluate_ast_expr(lsb)?.to_u64();
                            let (lo, hi) = (a.min(b), a.max(b));
                            let v = val.to_u64();
                            if v >= lo && v <= hi {
                                return Ok(LogicVec::from_u64(1, 1));
                            }
                            continue;
                        }
                    }
                    let item_val = self.evaluate_ast_expr(item)?;
                    // Normalisasi lebar sebelum case_eq: `addr[7:0]` (8-bit)
                    // vs literal desimal (32-bit) — bits tidak sama walau
                    // nilai sama tanpa resize.
                    let w = val.width.max(item_val.width);
                    let eq = val.resize(w).case_eq(&item_val.resize(w));
                    if eq == LogicVec::from_u64(1, 1) {
                        return Ok(LogicVec::from_u64(1, 1));
                    }
                }
                Ok(LogicVec::from_u64(0, 1))
            }
            Expr::StreamingConcat {
                op,
                slice_size,
                slices,
            } => {
                let mut vals = Vec::new();
                for sl in slices {
                    vals.push(self.evaluate_ast_expr(sl)?);
                }
                let all_bits: Vec<LogicVal> =
                    vals.iter().flat_map(|v| v.bits.iter().copied()).collect();
                let slen = if let Some(ss_expr) = slice_size {
                    let ss_val = self.evaluate_ast_expr(ss_expr)?;
                    let n = ss_val.to_u64() as usize;
                    if n == 0 {
                        return Err(SimError::with_diag(
                            DiagCode::MemoryOutOfBounds,
                            "streaming slice size must be > 0",
                        ));
                    }
                    n
                } else {
                    1
                };
                let mut result = Vec::new();
                if op == ">>" {
                    for chunk in all_bits.chunks(slen).rev() {
                        result.extend(chunk.iter().rev());
                    }
                } else {
                    for chunk in all_bits.chunks(slen).rev() {
                        result.extend(chunk.iter());
                    }
                }
                Ok(LogicVec {
                    width: result.len(),
                    bits: result,
                })
            }
            Expr::Dist { expr: inner, items } => {
                let inner_val = self.evaluate_ast_expr(inner)?;
                let ir_items = items
                    .iter()
                    .map(|di| match di {
                        mivon_ast::DistItem::Value(e, mivon_ast::DistWeight::Item(w)) => {
                            let ev = self
                                .evaluate_ast_expr(e)
                                .unwrap_or(LogicVec::from_u64(0, 32));
                            mivon_ir::IrDistItem {
                                range_lo: Some(ev.to_u64() as i64),
                                range_hi: Some(ev.to_u64() as i64),
                                weight_type: mivon_ir::DistWeightType::Item,
                                weight: *w as i64,
                            }
                        }
                        mivon_ast::DistItem::Value(e, mivon_ast::DistWeight::Range(w)) => {
                            let ev = self
                                .evaluate_ast_expr(e)
                                .unwrap_or(LogicVec::from_u64(0, 32));
                            mivon_ir::IrDistItem {
                                range_lo: Some(ev.to_u64() as i64),
                                range_hi: Some(ev.to_u64() as i64),
                                weight_type: mivon_ir::DistWeightType::Range,
                                weight: *w as i64,
                            }
                        }
                        mivon_ast::DistItem::Range(lo, hi, mivon_ast::DistWeight::Item(w)) => {
                            let lo_v = self.evaluate_ast_expr(lo).ok().map(|v| v.to_u64() as i64);
                            let hi_v = self.evaluate_ast_expr(hi).ok().map(|v| v.to_u64() as i64);
                            mivon_ir::IrDistItem {
                                range_lo: lo_v,
                                range_hi: hi_v,
                                weight_type: mivon_ir::DistWeightType::Item,
                                weight: *w as i64,
                            }
                        }
                        mivon_ast::DistItem::Range(lo, hi, mivon_ast::DistWeight::Range(w)) => {
                            let lo_v = self.evaluate_ast_expr(lo).ok().map(|v| v.to_u64() as i64);
                            let hi_v = self.evaluate_ast_expr(hi).ok().map(|v| v.to_u64() as i64);
                            mivon_ir::IrDistItem {
                                range_lo: lo_v,
                                range_hi: hi_v,
                                weight_type: mivon_ir::DistWeightType::Range,
                                weight: *w as i64,
                            }
                        }
                    })
                    .collect::<Vec<_>>();
                Ok(self.evaluate_expr(&IrExpr::Dist {
                    expr: Box::new(IrExpr::Const(inner_val)),
                    items: ir_items,
                })?)
            }
            Expr::Cast { dtype, expr: inner } => {
                let val = self.evaluate_ast_expr(inner)?;
                let cast_width = match mivon_elaboration::util::parse_type_spec_str(dtype.as_str())
                {
                    Some(_) => {
                        // For AST path, compute width from type string
                        match dtype.as_str() {
                            "bit" | "logic" => 1,
                            "byte" => 8,
                            "shortint" => 16,
                            "int" | "integer" => 32,
                            "longint" | "time" => 64,
                            "real" | "realtime" => 64,
                            _ => val.width,
                        }
                    }
                    None => val.width,
                };
                Ok(val.resize(cast_width))
            }
            Expr::CastWidth { width, expr: inner } => {
                let val = self.evaluate_ast_expr(inner)?;
                let w = self.evaluate_ast_expr(width)?.to_u64().max(1) as usize;
                Ok(val.resize(w))
            }
            Expr::ScopedIdent {
                package,
                item,
                line,
                col,
            } => {
                // F20: catat posisi source agar warning di bawah punya lokasi.
                self.set_cur_src_pos(*line, *col);
                let qname = Symbol::intern(&format!("{}::{}", package.as_str(), item.as_str()));
                if let Some(&val) = self.design.pkg_scoped_consts.get(&qname) {
                    return Ok(LogicVec::from_u64(val as u64, 32));
                }
                self.diag_warn_at(
                    DiagCode::DpiError,
                    format!(
                        "scoped identifier '{}.{}' not resolved at runtime; using null default",
                        package, item
                    ),
                    *line,
                    *col,
                );
                Ok(LogicVec::from_u64(0, 32))
            }
            // Struct assignment pattern di konteks runtime (inisialisasi var
            // struct / class field). Nilai utuh tidak bisa di-pack tanpa layout
            // typedef; evaluasi 0 (perilaku lama: pola bernama → FillLit 0).
            Expr::StructLit { .. } => Ok(LogicVec::from_u64(0, 32)),
        }
    }

    pub(crate) fn find_signal(&self, name: &str) -> Option<usize> {
        // PERF-16: cache lookup dibangun lazy SEKALI. Design top.signals dan
        // hier_signal_map STATIS setelah elaborasi → map aman di-cache.
        // Sebelumnya: scan linear O(S) per akses hierarkis (HierRef /
        // HierRefIndex) — O(N²) pada desain besar (lihat perf/REPORT).
        let map = self.signal_lookup.get_or_init(|| {
            let mut m: std::collections::HashMap<Symbol, usize> =
                self.design.hier_signal_map.clone();
            for (i, s) in self.design.top.signals.iter().enumerate() {
                m.entry(s.name).or_insert(i);
            }
            m
        });
        map.get(&Symbol::intern(name)).copied()
    }

    pub(crate) fn build_hier_name(obj: &Expr, field: &str) -> String {
        match obj {
            Expr::Ident { name: prefix, .. } => format!("{}.{}", prefix, field),
            Expr::MemberAccess {
                obj: inner,
                field: inner_field,
            } => {
                format!(
                    "{}.{}",
                    Self::build_hier_name(inner, inner_field.as_str()),
                    field
                )
            }
            _ => String::new(),
        }
    }

    /// F17: alokasi object class + panggil constructor (bila ada). Dipakai
    /// `x = new(...)` di jalur AST (body method class). `class = None` →
    /// objek tanpa class_name (perilaku lama untuk konteks non-assignment).
    pub(crate) fn allocate_new_object(
        &mut self,
        class: Option<Symbol>,
        arg_vals: &[LogicVec],
    ) -> Result<LogicVec, SimError> {
        if std::env::var("DBG_UVM").is_ok() {
            eprintln!(
                "[DBG-UVM] allocate_new_object class={:?} nargs={}",
                class.map(|s| s.to_string()),
                arg_vals.len()
            );
        }
        let raw = class
            .unwrap_or_else(|| Symbol::intern(""))
            .as_str()
            .to_string();
        let effective = self
            .factory_type_overrides
            .get(&raw)
            .cloned()
            .unwrap_or(raw);
        let obj_id = self.state.alloc_object(Symbol::intern(&effective));
        // F19: inisialisasi field class ke default 0 (semantics SV — semua
        // member di-zero-initialize). Tanpa ini, baca field yang belum pernah
        // di-assign (mis. `if (got_cnt == 0)` sebelum `got_cnt = ...` di
        // run_phase driver) memunculkan warning RT0001 + null default.
        if let Some(cls) = self.design.classes.get(&Symbol::intern(&effective)) {
            if let Some(obj) = self.state.get_object_mut(obj_id) {
                for field in &cls.fields {
                    obj.fields
                        .entry(field.name)
                        .or_insert_with(|| LogicVec::from_u64(0, field.width.max(1)));
                }
            }
        }
        // F21-F24: builtin UVM (`methods: vec![]`) — `new` tetap di-dispatch
        // agar data di-insert / child internal dibuat. Tanpa ini, objek TANPA
        // `new` override → data tak pernah dibuat → handle null / senyap.
        if !effective.is_empty() && self.uvm_needs_new_dispatch(&effective) {
            self.execute_method(obj_id, "new", arg_vals)?;
        }
        Ok(LogicVec::from_u64(obj_id as u64, 64))
    }

    /// F17: resolve tipe class deklarasi local (`my_item it;` → `my_item`)
    /// pada method yang sedang berjalan, dari `decls` method
    /// (`DataType::UserDefined`). Dipakai `it = new(...)` untuk mengisi
    /// class_name object — tanpanya randomize()/get_type_name() gagal.
    /// F17: evaluasi RHS assignment — `x = new(...)` di-resolve class dari
    /// tipe deklarasi LHS lalu alokasi object + constructor; selain itu
    /// evaluasi ekspresi biasa. Helper BERSAMA untuk `evaluate_ast_stmt`
    /// (function body) dan loop statement block.rs (task body) agar class
    /// object selalu terisi (tanpa ini randomize()/get_type_name() gagal
    /// "unknown class").
    pub(crate) fn eval_ast_assign_rhs(
        &mut self,
        rhs: &Expr,
        lhs: &Expr,
    ) -> Result<LogicVec, SimError> {
        if let Expr::FuncCall { name, args, .. } = rhs {
            if name == "new" {
                let class = match lhs {
                    Expr::Ident { name, .. } => self.resolve_new_class_hint(name.as_str()),
                    _ => None,
                };
                let arg_vals: Vec<LogicVec> = args
                    .iter()
                    .map(|a| self.evaluate_ast_expr(a))
                    .collect::<Result<_, _>>()?;
                return self.allocate_new_object(class, &arg_vals);
            }
        }
        self.evaluate_ast_expr(rhs)
    }

    pub(crate) fn resolve_new_class_hint(&self, name: &str) -> Option<Symbol> {
        let obj_id = self.current_this?;
        let class_name = self.state.get_object(obj_id)?.class_name;
        let class_def = self.design.classes.get(&class_name)?;
        // F18: cek class FIELD dulu — pola UVM build_phase membuat komponen
        // ke field (`env = new("env", this)`), bukan local. IrClassField kini
        // menyimpan dtype (lihat ir.rs).
        if std::env::var("DBG_UVM").is_ok() {
            eprintln!(
                "[DBG-UVM] resolve_new_class_hint name={} class={} fields={:?}",
                name,
                class_name,
                class_def
                    .fields
                    .iter()
                    .map(|f| (
                        f.name.to_string(),
                        f.dtype.clone().map(|d| format!("{:?}", d))
                    ))
                    .collect::<Vec<_>>()
            );
        }
        if let Some(f) = class_def.fields.iter().find(|f| f.name.as_str() == name) {
            if let Some(DataType::UserDefined(s)) = &f.dtype {
                return Some(*s);
            }
        }
        let mname = self.current_method?;
        let method_def = class_def.methods.iter().find(|m| m.name == mname)?;
        for d in &method_def.decls {
            for dv in &d.names {
                if dv.name.as_str() == name {
                    if let DataType::UserDefined(s) = &d.dtype {
                        return Some(*s);
                    }
                }
            }
        }
        None
    }

    pub(crate) fn evaluate_ast_stmt(&mut self, stmt: &Stmt) -> Result<(), SimError> {
        match stmt {
            Stmt::Block { stmts } => {
                // F35: `return` AST menandai stop-blok (ast_return_pending) —
                // statement setelah return TIDAK boleh dieksekusi (bug:
                // `if (n<=1) return n; return f(n-1)+f(n-2);` → statement
                // kedua tetap jalan → rekursi tak berujung).
                for s in stmts {
                    if self.ast_return_pending {
                        break;
                    }
                    self.evaluate_ast_stmt(s)?;
                }
                Ok(())
            }
            Stmt::BlockingAssign { lhs, rhs, delay: _ } => {
                // F17: `x = new(...)` — resolve class dari tipe LHS (helper
                // bersama dgn loop statement block.rs untuk task body).
                let val = self.eval_ast_assign_rhs(rhs, lhs)?;
                match lhs {
                    Expr::Ident { name, .. } => self.write_local_or_field(name.as_str(), val),
                    Expr::MemberAccess { obj, field } => {
                        let obj_val = self.evaluate_ast_expr(obj)?;
                        let obj_id = obj_val.to_u64() as ObjId;
                        if let Some(obj) = self.state.get_object_mut(obj_id) {
                            obj.fields.insert(*field, val);
                            Ok(())
                        } else {
                            Err(SimError::with_diag(
                                DiagCode::NullHandle,
                                format!("object {} not found for field write", obj_id),
                            ))
                        }
                    }
                    Expr::BitSelect { expr: inner, index } => {
                        let idx_val = self.evaluate_ast_expr(index)?;
                        let idx = idx_val.to_u64() as usize;
                        if let Some(elem_width) = self.get_field_elem_width(inner) {
                            let lhs_val = self.evaluate_ast_expr(inner)?;
                            let mut bits = lhs_val.bits.clone();
                            let start = idx * elem_width;
                            for (j, b) in val.bits.iter().enumerate() {
                                if start + j < bits.len() {
                                    bits[start + j] = *b;
                                }
                            }
                            let new_val = LogicVec {
                                width: bits.len(),
                                bits,
                            };
                            match inner.as_ref() {
                                Expr::Ident { name, .. } => {
                                    self.write_local_or_field(name.as_str(), new_val)?;
                                }
                                Expr::MemberAccess { obj, field } => {
                                    let ov = self.evaluate_ast_expr(obj)?;
                                    let oid = ov.to_u64() as ObjId;
                                    if let Some(o) = self.state.get_object_mut(oid) {
                                        o.fields.insert(*field, new_val);
                                    }
                                }
                                _ => {}
                            }
                            Ok(())
                        } else {
                            let lhs_val = self.evaluate_ast_expr(inner)?;
                            let mut bits = lhs_val.bits.clone();
                            if idx < bits.len() {
                                let bit = val.bits.first().copied().unwrap_or(LogicVal::X);
                                bits[idx] = bit;
                            }
                            let width = bits.len();
                            let new_val = LogicVec { width, bits };
                            match inner.as_ref() {
                                Expr::Ident { name, .. } => {
                                    self.write_local_or_field(name.as_str(), new_val)?;
                                }
                                Expr::MemberAccess { obj, field } => {
                                    let ov = self.evaluate_ast_expr(obj)?;
                                    let oid = ov.to_u64() as ObjId;
                                    if let Some(o) = self.state.get_object_mut(oid) {
                                        o.fields.insert(*field, new_val);
                                    }
                                }
                                _ => {}
                            }
                            Ok(())
                        }
                    }
                    Expr::RangeSelect {
                        expr: inner,
                        msb,
                        lsb,
                    } => {
                        let lhs_val = self.evaluate_ast_expr(inner)?;
                        let msb_val = self.evaluate_ast_expr(msb)?;
                        let lsb_val = self.evaluate_ast_expr(lsb)?;
                        let m = msb_val.to_u64() as usize;
                        let l = lsb_val.to_u64() as usize;
                        let (start, end) = if m > l { (l, m) } else { (m, l) };
                        let range_len = end - start + 1;
                        let mut bits = lhs_val.bits.clone();
                        for j in 0..val.width.min(range_len) {
                            if start + j < bits.len() {
                                bits[start + j] = val.bits[j];
                            }
                        }
                        let new_val = LogicVec {
                            width: bits.len(),
                            bits,
                        };
                        match inner.as_ref() {
                            Expr::Ident { name, .. } => {
                                self.write_local_or_field(name.as_str(), new_val)?;
                            }
                            Expr::MemberAccess { obj, field } => {
                                let ov = self.evaluate_ast_expr(obj)?;
                                let oid = ov.to_u64() as ObjId;
                                if let Some(o) = self.state.get_object_mut(oid) {
                                    o.fields.insert(*field, new_val);
                                }
                            }
                            _ => {}
                        }
                        Ok(())
                    }
                    _ => Err(SimError::with_diag(
                        DiagCode::NotImplemented,
                        format!("unsupported LHS in method: {:?}", lhs),
                    )),
                }
            }
            Stmt::NonBlockingAssign { lhs, rhs, delay: _ } => {
                // F17: sama seperti BlockingAssign — `x <= new(...)` juga perlu
                // class dari tipe LHS.
                let val = self.eval_ast_assign_rhs(rhs, lhs)?;
                match lhs {
                    Expr::Ident { name, .. } => self.write_local_or_field(name.as_str(), val),
                    Expr::MemberAccess { obj, field } => {
                        let obj_val = self.evaluate_ast_expr(obj)?;
                        let obj_id = obj_val.to_u64() as ObjId;
                        if let Some(obj) = self.state.get_object_mut(obj_id) {
                            obj.fields.insert(*field, val);
                            Ok(())
                        } else {
                            Err(SimError::with_diag(
                                DiagCode::NullHandle,
                                format!("object {} not found for field write", obj_id),
                            ))
                        }
                    }
                    Expr::BitSelect { expr: inner, index } => {
                        let idx_val = self.evaluate_ast_expr(index)?;
                        let idx = idx_val.to_u64() as usize;
                        if let Some(elem_width) = self.get_field_elem_width(inner) {
                            let lhs_val = self.evaluate_ast_expr(inner)?;
                            let mut bits = lhs_val.bits.clone();
                            let start = idx * elem_width;
                            for (j, b) in val.bits.iter().enumerate() {
                                if start + j < bits.len() {
                                    bits[start + j] = *b;
                                }
                            }
                            let new_val = LogicVec {
                                width: bits.len(),
                                bits: bits.clone(),
                            };
                            match inner.as_ref() {
                                Expr::Ident { name, .. } => {
                                    self.write_local_or_field(name.as_str(), new_val)?;
                                }
                                Expr::MemberAccess { obj, field } => {
                                    let ov = self.evaluate_ast_expr(obj)?;
                                    let oid = ov.to_u64() as ObjId;
                                    if let Some(o) = self.state.get_object_mut(oid) {
                                        o.fields.insert(*field, new_val);
                                    }
                                }
                                _ => {}
                            }
                            Ok(())
                        } else {
                            let lhs_val = self.evaluate_ast_expr(inner)?;
                            let mut bits = lhs_val.bits.clone();
                            if idx < bits.len() {
                                let bit = val.bits.first().copied().unwrap_or(LogicVal::X);
                                bits[idx] = bit;
                            }
                            let width = bits.len();
                            let new_val = LogicVec { width, bits };
                            match inner.as_ref() {
                                Expr::Ident { name, .. } => {
                                    self.write_local_or_field(name.as_str(), new_val)?;
                                }
                                Expr::MemberAccess { obj, field } => {
                                    let ov = self.evaluate_ast_expr(obj)?;
                                    let oid = ov.to_u64() as ObjId;
                                    if let Some(o) = self.state.get_object_mut(oid) {
                                        o.fields.insert(*field, new_val);
                                    }
                                }
                                _ => {}
                            }
                            Ok(())
                        }
                    }
                    Expr::RangeSelect {
                        expr: inner,
                        msb,
                        lsb,
                    } => {
                        let lhs_val = self.evaluate_ast_expr(inner)?;
                        let msb_val = self.evaluate_ast_expr(msb)?;
                        let lsb_val = self.evaluate_ast_expr(lsb)?;
                        let m = msb_val.to_u64() as usize;
                        let l = lsb_val.to_u64() as usize;
                        let (start, end) = if m > l { (l, m) } else { (m, l) };
                        let range_len = end - start + 1;
                        let mut bits = lhs_val.bits.clone();
                        for j in 0..val.width.min(range_len) {
                            if start + j < bits.len() {
                                bits[start + j] = val.bits[j];
                            }
                        }
                        let new_val = LogicVec {
                            width: bits.len(),
                            bits,
                        };
                        match inner.as_ref() {
                            Expr::Ident { name, .. } => {
                                self.write_local_or_field(name.as_str(), new_val)?;
                            }
                            Expr::MemberAccess { obj, field } => {
                                let ov = self.evaluate_ast_expr(obj)?;
                                let oid = ov.to_u64() as ObjId;
                                if let Some(o) = self.state.get_object_mut(oid) {
                                    o.fields.insert(*field, new_val);
                                }
                            }
                            _ => {}
                        }
                        Ok(())
                    }
                    _ => Err(SimError::with_diag(
                        DiagCode::NotImplemented,
                        format!("unsupported LHS in method: {:?}", lhs),
                    )),
                }
            }
            Stmt::IfElse {
                cond,
                true_branch,
                false_branch,
            } => {
                let cval = self.evaluate_ast_expr(cond)?;
                if cval.to_bool().unwrap_or(false) {
                    self.evaluate_ast_stmt(true_branch)
                } else if let Some(f) = false_branch {
                    self.evaluate_ast_stmt(f)
                } else {
                    Ok(())
                }
            }
            Stmt::Case {
                expr,
                items,
                default,
            } => {
                let case_val = self.evaluate_ast_expr(expr)?;
                let mut matched = false;
                for item in items {
                    for pat in &item.labels {
                        let pat_val = self.evaluate_ast_expr(pat)?;
                        if case_val.case_val_eq(&pat_val) {
                            self.evaluate_ast_stmt(&item.stmt)?;
                            matched = true;
                            break;
                        }
                    }
                    if matched {
                        break;
                    }
                }
                if !matched {
                    if let Some(default_body) = default {
                        self.evaluate_ast_stmt(default_body)?;
                    }
                }
                Ok(())
            }
            Stmt::CaseX {
                expr,
                items,
                default,
            } => {
                let case_val = self.evaluate_ast_expr(expr)?;
                let mut matched = false;
                for item in items {
                    for pat in &item.labels {
                        let pat_val = self.evaluate_ast_expr(pat)?;
                        if case_val.casex_eq(&pat_val) {
                            self.evaluate_ast_stmt(&item.stmt)?;
                            matched = true;
                            break;
                        }
                    }
                    if matched {
                        break;
                    }
                }
                if !matched {
                    if let Some(default_body) = default {
                        self.evaluate_ast_stmt(default_body)?;
                    }
                }
                Ok(())
            }
            Stmt::CaseZ {
                expr,
                items,
                default,
            } => {
                let case_val = self.evaluate_ast_expr(expr)?;
                let mut matched = false;
                for item in items {
                    for pat in &item.labels {
                        let pat_val = self.evaluate_ast_expr(pat)?;
                        if case_val.casez_eq(&pat_val) {
                            self.evaluate_ast_stmt(&item.stmt)?;
                            matched = true;
                            break;
                        }
                    }
                    if matched {
                        break;
                    }
                }
                if !matched {
                    if let Some(default_body) = default {
                        self.evaluate_ast_stmt(default_body)?;
                    }
                }
                Ok(())
            }
            Stmt::StmtCase {
                expr,
                items,
                default,
            } => self.evaluate_ast_stmt(&Stmt::Case {
                expr: expr.clone(),
                items: items.clone(),
                default: default.clone(),
            }),
            Stmt::LoopFor {
                init,
                cond,
                step,
                stmts,
            } => {
                if let Some(init_stmt) = init {
                    self.evaluate_ast_stmt(init_stmt)?;
                }
                while self.disable_pending.is_none()
                    && cond.as_ref().is_none_or(|c| {
                        eprintln!("FOR cond {:?}", c);
                        self.evaluate_ast_expr(c)
                            .ok()
                            .map(|v| v.to_bool().unwrap_or(false))
                            .unwrap_or(false)
                    })
                {
                    for s in stmts {
                        self.evaluate_ast_stmt(s)?;
                        if self.disable_pending.is_some() {
                            break;
                        }
                    }
                    if self.disable_pending.is_some() {
                        break;
                    }
                    if let Some(step_stmt) = step {
                        eprintln!(
                            "FOR step {:?} before {:?}",
                            step_stmt,
                            self.get_local("i").map(|v| v.to_u64())
                        );
                        self.evaluate_ast_stmt(step_stmt)?;
                        eprintln!(
                            "FOR step after {:?}",
                            self.get_local("i").map(|v| v.to_u64())
                        );
                    }
                }
                Ok(())
            }
            Stmt::LoopWhile { cond, stmts } => {
                while self.disable_pending.is_none()
                    && self
                        .evaluate_ast_expr(cond)
                        .ok()
                        .map(|v| v.to_bool().unwrap_or(false))
                        .unwrap_or(false)
                {
                    for s in stmts {
                        self.evaluate_ast_stmt(s)?;
                        if self.disable_pending.is_some() {
                            break;
                        }
                    }
                }
                Ok(())
            }
            Stmt::LoopForever { stmts } => {
                for _ in 0..1_000_000 {
                    if self.disable_pending.is_some() {
                        break;
                    }
                    for s in stmts {
                        self.evaluate_ast_stmt(s)?;
                        if self.disable_pending.is_some() {
                            break;
                        }
                    }
                }
                Ok(())
            }
            Stmt::Repeat { count, stmts } => {
                let count_val = self.evaluate_ast_expr(count)?;
                let n = count_val.to_u64();
                for _ in 0..n {
                    for s in stmts {
                        self.evaluate_ast_stmt(s)?;
                    }
                }
                Ok(())
            }
            Stmt::Expr { expr } => {
                self.evaluate_ast_expr(expr)?;
                Ok(())
            }
            Stmt::SysCall {
                name,
                args,
                line,
                col,
            } => {
                // F20: catat posisi syscall agar warning runtime punya lokasi.
                self.set_cur_src_pos(*line, *col);
                // F18: $display/$info/dst di function body (mis. report_phase)
                // sebelumnya di-skip diam-diam — delegasi ke handler AST.
                self.handle_ast_syscall(name.as_str(), args)
            }
            Stmt::SysFinish => {
                self.running = false;
                Ok(())
            }
            Stmt::Delay { delay: _, stmt } => {
                // In immediate method context, execute delay body immediately
                self.evaluate_ast_stmt(stmt)
            }
            Stmt::Null => Ok(()),
            Stmt::Disable { name } => {
                // F47: self-disable → disable_pending (loop-top berhenti);
                // target di luar stack → disable_cross (pelaku lanjut).
                let _ = self.mark_disable_label(*name);
                Ok(())
            }
            Stmt::ForeachLoop {
                array_var,
                index_vars,
                stmts,
            } => {
                let count = self.get_foreach_count(array_var.as_str());
                let iv = index_vars
                    .first()
                    .cloned()
                    .unwrap_or_else(|| Symbol::intern("i"));
                for i in 0..count {
                    let idx_val = LogicVec::from_u64(i as u64, 32);
                    let mut scope = HashMap::new();
                    scope.insert(iv, idx_val);
                    let depth = self.method_locals.len();
                    self.method_locals.push(scope);
                    for s in stmts {
                        self.evaluate_ast_stmt(s)?;
                    }
                    self.method_locals.truncate(depth);
                }
                Ok(())
            }
            Stmt::Return(Some(expr)) => {
                let val = self.evaluate_ast_expr(expr)?;
                if let Some(ref method) = self.current_method {
                    self.set_local(method.as_str(), val);
                }
                // F35: stop-blok — statement setelah return tidak boleh jalan.
                self.ast_return_pending = true;
                Ok(())
            }
            Stmt::Return(None) => {
                self.ast_return_pending = true;
                Ok(())
            }
            Stmt::StmtAssign { lhs, rhs } => {
                let val = self.evaluate_ast_expr(rhs)?;
                match lhs {
                    Expr::Ident { name, .. } => self.write_local_or_field(name.as_str(), val),
                    Expr::MemberAccess { obj, field } => {
                        let obj_val = self.evaluate_ast_expr(obj)?;
                        let obj_id = obj_val.to_u64() as ObjId;
                        if let Some(obj) = self.state.get_object_mut(obj_id) {
                            obj.fields.insert(*field, val);
                            Ok(())
                        } else {
                            Err(SimError::with_diag(
                                DiagCode::NullHandle,
                                format!("object {} not found for field write", obj_id),
                            ))
                        }
                    }
                    _ => Err(SimError::with_diag(
                        DiagCode::NotImplemented,
                        format!("unsupported LHS in StmtAssign: {:?}", lhs),
                    )),
                }
            }
            _ => Err(SimError::with_diag(
                DiagCode::NotImplemented,
                format!("unsupported statement in method context: {:?}", stmt),
            )),
        }
    }

    /// LANG-40: cari `let` declaration di class milik `current_this`.
    /// Dipakai evaluate_ast_expr untuk resolve ident (let tanpa parameter)
    /// dan FuncCall (let berparameter) di body method/task class.
    fn class_let_decl(&self, name: &Symbol) -> Option<&mivon_ast::types::LetDecl> {
        let obj_id = self.current_this?;
        let class_name = self.state.objects.get(obj_id)?.class_name;
        let class_def = self.design.classes.get(&class_name)?;
        class_def.lets.iter().find(|ld| ld.name == *name)
    }
}
