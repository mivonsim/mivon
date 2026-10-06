//! System call handler untuk blocking evaluation.
//! Diekstrak dari block.rs — 1 file = 1 tanggung jawab.
//!
//! Menangani semua $system tasks seperti $display, $fopen, $readmemh, dll.

use super::super::SimulationEngine;
use crate::simulator::util::*;
use crate::waveform::VcdWriter;
use mivon_core::diagnostics::DiagCode;
use mivon_core::error::SimError;
use mivon_core::Symbol;
use mivon_ir::*;
use rand::Rng;
use rand::SeedableRng;
use std::io::Write;

impl SimulationEngine {
    /// Handle a SysCall statement during block evaluation (with delay/fork context).
    /// Returns Ok(true) if the statement was handled (continue to next), Ok(false) if
    /// the statement yielded (needs rescheduling).
    pub(crate) fn evaluate_syscall(
        &mut self,
        name: &str,
        ir_args: &[IrExpr],
        _fork_id: Option<usize>,
        _stmts: &[IrStmt],
        _i: usize,
    ) -> Result<bool, SimError> {
        // VERIF-07: UVM DB call sebagai bare statement (uvm_config_db::set /
        // uvm_resource_db::set/get/exists/write_by_name/read_by_name, uvm_
        // cmdline_processor::*) — dispatch lewat execute_uvm_db_stmt. Tanpa
        // ini set-as-statement di-eliminasi elaborator (sebelumnya map tidak
        // pernah terisi).
        if name.starts_with("uvm_config_db::")
            || name.starts_with("uvm_resource_db::")
            || name.starts_with("uvm_cmdline_processor::")
            || name.starts_with("uvm_root::")
            || name.starts_with("uvm_tr_database::")
        {
            self.execute_uvm_db_stmt(name, ir_args)?;
            return Ok(true);
        }
        if self.evaluate_lang_syscall(name, ir_args)? {
            return Ok(true);
        }
        if name == "display" || name == "write" {
            let msg = self.format_display(ir_args);
            if name == "display" {
                println!("{}", msg);
            } else {
                print!("{}", msg);
            }
        } else if name == "run_test" {
            // F18: run_test("name") dari statement (initial/program block).
            // Tanpa special-case di elaborator, statement ini di-eliminasi
            // sebagai side-effect-free; di sini efeknya dijalankan.
            let arg_vals: Vec<LogicVec> = ir_args
                .iter()
                .map(|a| self.evaluate_expr(a).unwrap_or(LogicVec::from_u64(0, 32)))
                .collect();
            let test_name = arg_vals.first().map(logicvec_to_string).unwrap_or_default();
            self.run_uvm_test(&test_name)?;
        } else if name == "strobe" {
            self.strobe_events.push(ir_args.to_vec());
        } else if name == "fstrobe" {
            let handle = ir_args
                .first()
                .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u32));
            if let Some(h) = handle {
                self.fstrobe_events.push((h, ir_args[1..].to_vec()));
            }
        } else if name == "fmonitor" {
            let handle = ir_args
                .first()
                .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u32));
            if let Some(h) = handle {
                let vals: Vec<LogicVec> = ir_args[1..]
                    .iter()
                    .map(|a| self.evaluate_expr(a).unwrap_or(LogicVec::from_u64(0, 32)))
                    .collect();
                self.fmonitor_map.insert(h, (ir_args[1..].to_vec(), vals));
            }
        } else if name == "monitor" {
            let vals: Vec<LogicVec> = ir_args
                .iter()
                .map(|a| self.evaluate_expr(a).unwrap_or(LogicVec::from_u64(0, 32)))
                .collect();
            self.monitor_args = Some(ir_args.to_vec());
            self.monitor_last_values = Some(vals);
        } else if name == "readmemh" {
            let file = ir_args.first().ok_or_else(|| {
                self.diag_error(
                    DiagCode::DpiError,
                    "$readmemh requires at least a filename argument",
                )
            })?;
            let file_str = match file {
                IrExpr::String(s) => s.clone(),
                // Bug-2: filename via VARIABEL string (`string f; $readmemh(f,m)`).
                IrExpr::Signal(sid, _) => {
                    let ok = self
                        .design
                        .top
                        .signals
                        .get(*sid)
                        .map(|si| si.is_string)
                        .unwrap_or(false);
                    if ok {
                        let lv = self.state.read_signal(*sid);
                        crate::simulator::util::logicvec_to_string(lv)
                    } else {
                        return Err(self.diag_error(
                            DiagCode::DpiError,
                            "$readmemh first argument must be a string (filename)",
                        ));
                    }
                }
                _ => {
                    return Err(self.diag_error(
                        DiagCode::DpiError,
                        "$readmemh first argument must be a string (filename)",
                    ));
                }
            };
            let sig_id = ir_args
                .get(1)
                .and_then(|a| {
                    if let IrExpr::Signal(id, _) = a {
                        Some(*id)
                    } else {
                        None
                    }
                })
                .ok_or_else(|| {
                    self.diag_error(
                        DiagCode::DpiError,
                        "$readmemh requires a signal/memory argument",
                    )
                })?;
            let sig_info = self.design.top.signals.get(sig_id).ok_or_else(|| {
                self.diag_error(DiagCode::DpiError, "$readmemh: signal not found")
            })?;
            let elem_w = sig_info.elem_width.max(1);
            let max_words = sig_info.width / elem_w;
            let data = read_hex_file(&file_str, elem_w, max_words, None, None)?;
            let mut all_bits = Vec::new();
            for d in &data {
                all_bits.extend(d.bits.iter().cloned());
            }
            let packed = LogicVec {
                bits: all_bits,
                width: data.len() * elem_w,
            };
            self.state.write_signal(sig_id, packed);
        } else if name == "readmemb" {
            let file = ir_args.first().ok_or_else(|| {
                self.diag_error(
                    DiagCode::DpiError,
                    "$readmemb requires at least a filename argument",
                )
            })?;
            let file_str = match file {
                IrExpr::String(s) => s.clone(),
                IrExpr::Signal(sid, _) => {
                    let ok = self
                        .design
                        .top
                        .signals
                        .get(*sid)
                        .map(|si| si.is_string)
                        .unwrap_or(false);
                    if ok {
                        let lv = self.state.read_signal(*sid);
                        crate::simulator::util::logicvec_to_string(lv)
                    } else {
                        return Err(self.diag_error(
                            DiagCode::DpiError,
                            "$readmemb first argument must be a string (filename)",
                        ));
                    }
                }
                _ => {
                    return Err(self.diag_error(
                        DiagCode::DpiError,
                        "$readmemb first argument must be a string (filename)",
                    ));
                }
            };
            let sig_id = ir_args
                .get(1)
                .and_then(|a| {
                    if let IrExpr::Signal(id, _) = a {
                        Some(*id)
                    } else {
                        None
                    }
                })
                .ok_or_else(|| {
                    self.diag_error(
                        DiagCode::DpiError,
                        "$readmemb requires a signal/memory argument",
                    )
                })?;
            let sig_info = self.design.top.signals.get(sig_id).ok_or_else(|| {
                self.diag_error(DiagCode::DpiError, "$readmemb: signal not found")
            })?;
            let elem_w = sig_info.elem_width.max(1);
            let max_words = sig_info.width / elem_w;
            let data = read_bin_file(&file_str, elem_w, max_words, None, None)?;
            let mut all_bits = Vec::new();
            for d in &data {
                all_bits.extend(d.bits.iter().cloned());
            }
            let packed = LogicVec {
                bits: all_bits,
                width: data.len() * elem_w,
            };
            self.state.write_signal(sig_id, packed);
        } else if name == "random" {
            self.rand_call_count += 1;
            if let Some(seed_arg) = ir_args.get(1) {
                if let Ok(seed_val) = self.evaluate_expr(seed_arg) {
                    let seed = seed_val.to_u64();
                    self.rng = rand::rngs::StdRng::seed_from_u64(seed);
                    self.rand_seed = seed;
                }
            }
            let val: i32 = self.rng.gen();
            let sig_id = ir_args.first().and_then(|a| {
                if let IrExpr::Signal(id, _) = a {
                    Some(*id)
                } else {
                    None
                }
            });
            if let Some(sid) = sig_id {
                self.state
                    .write_signal(sid, LogicVec::from_u64(val as u64, 32));
            }
        } else if name == "srandom" {
            // $srandom as system task: sets seed for $random, does NOT increment rand_call_count
            // Args: seed (ir_args[0]) - no target signal for $srandom
            if let Some(seed_arg) = ir_args.first() {
                if let Ok(seed_val) = self.evaluate_expr(seed_arg) {
                    let seed = seed_val.to_u64();
                    self.rng = rand::rngs::StdRng::seed_from_u64(seed);
                    self.rand_seed = seed;
                }
            }
        } else if name == "urandom_seed" {
            // $urandom_seed as system task: sets seed for $urandom/$urandom_range,
            // increments rand_call_count. Sebelumnya hanya ada di
            // evaluate_syscall_stmt (jalur statement lanjutan) — statement
            // `$urandom_seed(777);` di initial block masuk ke evaluate_syscall
            // (jalur statement awal, block.rs) → seed TIDAK pernah diterapkan
            // (stream $urandom identik utk seed berbeda) + warning RT9003
            // "unknown system call ignored". LANG-21.
            if let Some(seed_arg) = ir_args.first() {
                if let Ok(seed_val) = self.evaluate_expr(seed_arg) {
                    let seed = seed_val.to_u64();
                    self.rng = rand::rngs::StdRng::seed_from_u64(seed);
                    self.rand_seed = seed;
                    self.rand_call_count += 1;
                }
            }
        } else if name == "urandom" {
            self.rand_call_count += 1;
            let val: u32 = self.rng.gen();
            let sig_id = ir_args.first().and_then(|a| {
                if let IrExpr::Signal(id, _) = a {
                    Some(*id)
                } else {
                    None
                }
            });
            if let Some(sid) = sig_id {
                self.state
                    .write_signal(sid, LogicVec::from_u64(val as u64, 32));
            }
        } else if name == "urandom_range" {
            self.rand_call_count += 1;
            let args_eval: Vec<LogicVec> = ir_args
                .iter()
                .map(|a| self.evaluate_expr(a).unwrap_or(LogicVec::from_u64(0, 32)))
                .collect();
            let maxval = args_eval.first().map(|v| v.to_u64()).unwrap_or(0);
            let minval = args_eval.get(1).map(|v| v.to_u64()).unwrap_or(0);
            let val = if maxval <= minval {
                minval
            } else {
                let range = maxval - minval + 1;
                if range <= 1 {
                    minval
                } else {
                    minval + (self.rng.gen::<u64>() % range)
                }
            };
            let sig_id = ir_args.first().and_then(|a| {
                if let IrExpr::Signal(id, _) = a {
                    Some(*id)
                } else {
                    None
                }
            });
            if let Some(sid) = sig_id {
                self.state.write_signal(sid, LogicVec::from_u64(val, 32));
            }
        } else if name == "dumpfile" {
            if let Some(IrExpr::String(fname)) = ir_args.first() {
                let path = fname.clone();
                let design = &self.design;
                let state = &self.state.signals;
                if let Some(ref mut vcd) = self.vcd {
                    let _ = vcd.reopen(&path, design, state);
                } else {
                    match VcdWriter::new(&path, design) {
                        Ok(v) => self.vcd = Some(v),
                        // F20: via DiagSink agar error punya file:line:col.
                        Err(e) => {
                            let _ = self.diag_error_at(
                                mivon_core::diagnostics::DiagCode::WaveformError,
                                format!("VCD: cannot create '{}': {}", path, e),
                                self.cur_src_pos().0,
                                self.cur_src_pos().1,
                            );
                        }
                    }
                }
            }
        } else if name == "dumpall" {
            if let Some(ref mut vcd) = self.vcd {
                vcd.write_time_header(self.state.time)?;
                let design = &self.design;
                let state = &self.state.signals;
                vcd.dump_all(design, state)?;
            }
        } else if name == "dumplimit" {
            if let Some(limit) = ir_args
                .first()
                .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64()))
            {
                if let Some(ref mut vcd) = self.vcd {
                    vcd.max_dump_size = Some(limit);
                }
            }
        } else if name == "dumpvars" || name == "dumpon" {
            if let Some(ref mut vcd) = self.vcd {
                vcd.enabled = true;
            }
        } else if name == "dumpoff" {
            if let Some(ref mut vcd) = self.vcd {
                vcd.enabled = false;
            }
        } else if name == "fopen" {
            let fname = ir_args.first().and_then(|a| {
                if let IrExpr::String(s) = a {
                    Some(s.clone())
                } else {
                    None
                }
            });
            if let Some(fname) = fname {
                let mode = ir_args.get(1).and_then(|a| {
                    if let IrExpr::String(s) = a {
                        Some(s.as_str())
                    } else {
                        None
                    }
                });
                let open_result = match mode {
                    Some("r") | Some("rb") => std::fs::File::open(&fname),
                    _ => std::fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .create(true)
                        .truncate(true)
                        .open(&fname),
                };
                match open_result {
                    Ok(f) => {
                        let handle = self.next_file_handle;
                        self.next_file_handle += 1;
                        self.file_handles.insert(handle, f);
                        self.file_read_pos.insert(handle, 0);
                        let sig_id = ir_args.get(1).and_then(|a| {
                            if let IrExpr::Signal(id, _) = a {
                                Some(*id)
                            } else {
                                None
                            }
                        });
                        if let Some(sid) = sig_id {
                            self.state
                                .write_signal(sid, LogicVec::from_u64(handle as u64, 32));
                        }
                    }
                    Err(_) => {
                        let sig_id = ir_args.get(1).and_then(|a| {
                            if let IrExpr::Signal(id, _) = a {
                                Some(*id)
                            } else {
                                None
                            }
                        });
                        if let Some(sid) = sig_id {
                            self.state.write_signal(sid, LogicVec::from_u64(0, 32));
                        }
                    }
                }
            }
        } else if name == "fdisplay" {
            let handle = ir_args
                .first()
                .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u32));
            if let Some(h) = handle {
                let msg = self.format_display(&ir_args[1..]);
                if let Some(f) = self.file_handles.get_mut(&h) {
                    // LRM: $fdisplay = $fwrite + newline (VCS/iverilog patuh;
                    // mivon sebelumnya concat tanpa '\n' — artefak differential).
                    let _ = writeln!(f, "{}", msg);
                }
            }
        } else if name == "fwrite" {
            let handle = ir_args
                .first()
                .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u32));
            if let Some(h) = handle {
                let msg = self.format_display(&ir_args[1..]);
                if let Some(f) = self.file_handles.get_mut(&h) {
                    let _ = write!(f, "{}", msg);
                }
            }
        } else if name == "fscanf" {
            let handle = ir_args
                .first()
                .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u32));
            if let Some(h) = handle {
                if let Some(f) = self.file_handles.get_mut(&h) {
                    use std::io::{Read, Seek};
                    let read_pos = self.file_read_pos.entry(h).or_insert(0);
                    f.seek(std::io::SeekFrom::Start(*read_pos)).ok();
                    let mut content = String::new();
                    let _bytes_read = f.read_to_string(&mut content).unwrap_or(0);
                    *read_pos = f.stream_position().unwrap_or(0);
                    let fmt = ir_args.get(1).and_then(|a| {
                        if let IrExpr::String(s) = a {
                            Some(s.clone())
                        } else {
                            None
                        }
                    });
                    if let Some(ref fmt_str) = fmt {
                        let tokens: Vec<&str> = content.split_whitespace().collect();
                        let mut ti = 0;
                        let mut ai = 0;
                        let mut chars = fmt_str.chars().peekable();
                        while let Some(c) = chars.next() {
                            if c == '%' {
                                if let Some(spec) = chars.next() {
                                    if spec == 'd' || spec == 'h' || spec == 'b' {
                                        if let Some(tok) = tokens.get(ti) {
                                            if let Ok(val) = if spec == 'h' {
                                                i64::from_str_radix(tok, 16)
                                            } else if spec == 'b' {
                                                i64::from_str_radix(tok, 2)
                                            } else {
                                                tok.parse::<i64>()
                                            } {
                                                let out_idx = 2 + ai;
                                                if let Some(IrExpr::Signal(sid, _)) =
                                                    ir_args.get(out_idx)
                                                {
                                                    self.state.write_signal(
                                                        *sid,
                                                        LogicVec::from_u64(val as u64, 32),
                                                    );
                                                }
                                                ai += 1;
                                            }
                                        }
                                        ti += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        } else if name == "fread" {
            let target = ir_args.first().and_then(|a| {
                if let IrExpr::Signal(id, _) = a {
                    Some(*id)
                } else {
                    None
                }
            });
            let src = ir_args.get(1);
            let data = if let Some(IrExpr::String(fname)) = src {
                std::fs::read(fname).ok()
            } else if let Some(arg) = src {
                let handle = self
                    .evaluate_expr(arg)
                    .ok()
                    .map(|v| v.to_u64() as u32)
                    .unwrap_or(0);
                if handle > 0 {
                    use std::io::Read;
                    self.file_handles.get_mut(&handle).and_then(|f| {
                        let mut buf = Vec::new();
                        f.read_to_end(&mut buf).ok().map(|_| buf)
                    })
                } else {
                    None
                }
            } else {
                None
            };
            if let (Some(sid), Some(bytes)) = (target, data) {
                let mut bits = Vec::with_capacity(bytes.len() * 8);
                for byte in bytes {
                    for i in 0..8 {
                        bits.push(if (byte >> i) & 1 == 1 {
                            LogicVal::One
                        } else {
                            LogicVal::Zero
                        });
                    }
                }
                self.state.write_signal(
                    sid,
                    LogicVec {
                        width: bits.len(),
                        bits,
                    },
                );
            }
        } else if name == "fclose" {
            let handle = ir_args
                .first()
                .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u32));
            if let Some(h) = handle {
                self.file_handles.remove(&h);
            }
        } else if name == "fflush" {
            let handle = ir_args
                .first()
                .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u32));
            if let Some(h) = handle {
                if let Some(f) = self.file_handles.get_mut(&h) {
                    let _ = f.flush();
                }
            }
        } else if name == "fseek" {
            let handle = ir_args
                .first()
                .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u32));
            let offset = ir_args
                .get(1)
                .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as i64));
            let op = ir_args
                .get(2)
                .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64()));
            if let (Some(h), Some(off)) = (handle, offset) {
                if let Some(f) = self.file_handles.get_mut(&h) {
                    use std::io::{Seek, SeekFrom};
                    let seek_from = match op {
                        Some(1) => SeekFrom::Current(off),
                        Some(2) => SeekFrom::End(off),
                        _ => SeekFrom::Start(off as u64),
                    };
                    let _ = f.seek(seek_from);
                    if let Ok(pos) = f.stream_position() {
                        self.file_read_pos.insert(h, pos);
                    }
                }
            }
        } else if name == "__dpi_stmt" {
            if let Some(arg) = ir_args.first() {
                self.evaluate_expr(arg)?;
            }
        } else if name == "value$plusargs" {
            let pattern = ir_args
                .first()
                .and_then(|a| self.evaluate_expr(a).ok())
                .map(|v| logicvec_to_string(&v))
                .unwrap_or_default();
            let plusarg_name = pattern
                .split('%')
                .next()
                .unwrap_or(&pattern)
                .trim_end_matches('=');
            let plusargs = self.plusargs.clone();
            for (key, val) in &plusargs {
                if key == plusarg_name {
                    if let Some(var_arg) = ir_args.get(1) {
                        let num = if let Some(hex) =
                            val.strip_prefix("0x").or_else(|| val.strip_prefix("0X"))
                        {
                            u64::from_str_radix(hex, 16).unwrap_or(0)
                        } else {
                            val.parse::<u64>().unwrap_or(0)
                        };
                        if std::env::var("DBG_PA").is_ok() {
                            eprintln!(
                                "[PA] pattern='{}' name='{}' val='{}' has_sig={}",
                                pattern,
                                plusarg_name,
                                val,
                                matches!(ir_args.get(1), Some(IrExpr::Signal(..)))
                            );
                        }
                        // Utk `%s`: tulis STRING ke string-signal (bukan nol).
                        let sig_id = if let IrExpr::Signal(id, _) = var_arg {
                            *id
                        } else {
                            break;
                        };
                        let is_str = self
                            .design
                            .top
                            .signals
                            .get(sig_id)
                            .map(|si| si.is_string)
                            .unwrap_or(false);
                        if std::env::var("DBG_PA").is_ok() {
                            eprintln!("[PA] sig={} is_str={}", sig_id, is_str);
                        }
                        if is_str {
                            self.state.write_signal(
                                sig_id,
                                crate::simulator::util::string_to_logicvec(val),
                            );
                        } else {
                            self.state.write_signal(sig_id, LogicVec::from_u64(num, 32));
                        }
                    }
                    break;
                }
            }
        } else if name == "asserton" {
            self.assert_off_all = false;
        } else if name == "assertoff" {
            self.assert_off_all = true;
            if let Some(scope_arg) = ir_args.first() {
                if let Ok(scope_val) = self.evaluate_expr(scope_arg) {
                    let scope_name = logicvec_to_string(&scope_val);
                    self.assert_modules_off.insert(Symbol::intern(&scope_name));
                }
            }
        } else if name == "assertkill" {
            self.assert_kill_all = true;
            self.assert_off_all = true;
            if let Some(scope_arg) = ir_args.first() {
                if let Ok(scope_val) = self.evaluate_expr(scope_arg) {
                    let scope_name = logicvec_to_string(&scope_val);
                    self.assert_modules_off.insert(Symbol::intern(&scope_name));
                }
            }
        } else if name == "assertpasson" || name == "assertfailon" || name == "assertnonvacuouson" {
            // Stubs — acknowledge but no-op
        } else if name == "isunbounded" {
            if let Some(IrExpr::Signal(id, _)) = ir_args.first() {
                self.state.write_signal(*id, LogicVec::from_u64(0, 1));
            }
        } else if name == "coverage_control" {
            if let Some(arg) = ir_args.first() {
                if let Ok(val) = self.evaluate_expr(arg) {
                    let bitmask = val.to_u64();
                    self.apply_coverage_control(bitmask);
                }
            }
        } else if name == "coverage_get" {
            let mut total = 0u64;
            let mut hit = 0u64;
            for cg in &self.design.covergroups {
                for cp in &cg.coverpoints {
                    let key = format!("{}.{}", cg.name, cp.name);
                    let key_sym = Symbol::intern(&key);
                    if let Some(t) = self.cover_total.get(&key_sym) {
                        total += t;
                    }
                    if let Some(h) = self.cover_hits.get(&key_sym) {
                        hit += h;
                    }
                }
            }
            let pct = if total > 0 {
                (hit as f64 / total as f64) * 100.0
            } else {
                0.0
            };
            if let Some(IrExpr::Signal(id, _)) = ir_args.first() {
                self.state
                    .write_signal(*id, LogicVec::from_u64(pct as u64, 64));
            }
        } else if name == "coverage_save" {
            let path = ir_args
                .first()
                .and_then(|a| {
                    if let IrExpr::String(s) = a {
                        Some(s.clone())
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| "coverage.ucis".to_string());
            let _ = self.export_coverage_ucis(&path);
        } else if name == "coverage_model" {
            let cg_name = ir_args.get(1).and_then(|a| {
                if let IrExpr::String(s) = a {
                    Some(s.clone())
                } else {
                    None
                }
            });
            let handle: u32 = if let Some(ref name) = cg_name {
                let exists = self
                    .design
                    .covergroups
                    .iter()
                    .any(|cg| cg.name.as_str() == name.as_str());
                if exists {
                    if let Some((&h, _)) = self
                        .coverage_model_handles
                        .iter()
                        .find(|(_, n)| n.as_str() == name.as_str())
                    {
                        h as u32
                    } else {
                        let h = self.next_coverage_model_handle;
                        self.next_coverage_model_handle += 1;
                        self.coverage_model_handles.insert(h, Symbol::intern(name));
                        h as u32
                    }
                } else {
                    self.emit_warning(
                        mivon_core::diagnostics::DiagCode::NotImplemented,
                        format!("$coverage_model: covergroup '{}' not found", name),
                    );
                    0
                }
            } else if let Some(first_cg) = self.design.covergroups.first() {
                if let Some((&h, _)) = self
                    .coverage_model_handles
                    .iter()
                    .find(|(_, n)| n.as_str() == first_cg.name.as_str())
                {
                    h as u32
                } else {
                    let h = self.next_coverage_model_handle;
                    self.next_coverage_model_handle += 1;
                    self.coverage_model_handles.insert(h, first_cg.name);
                    h as u32
                }
            } else {
                0
            };
            if let Some(IrExpr::Signal(id, _)) = ir_args.first() {
                self.state
                    .write_signal(*id, LogicVec::from_u64(handle as u64, 32));
            }
        } else if name == "load_coverage_db" {
            // `$load_coverage_db(path)` — muat MCDB lalu merge ke counter
            // live (line/branch/toggle/FSM/covergroup). Gagal load =
            // warning IoError (sim LANJUT, pola $fopen yg gagal = handle 0).
            let path = ir_args.first().and_then(|a| match a {
                IrExpr::String(s) => Some(s.clone()),
                IrExpr::Signal(sid, _) => {
                    let is_str = self
                        .design
                        .top
                        .signals
                        .get(*sid)
                        .map(|si| si.is_string)
                        .unwrap_or(false);
                    if is_str {
                        let lv = self.state.read_signal(*sid);
                        Some(crate::simulator::util::logicvec_to_string(lv))
                    } else {
                        None
                    }
                }
                _ => None,
            });
            match path {
                Some(p) => {
                    // Sukses = silent merge (seperti VCS — tak ada output).
                    // Gagal = warning IoError, sim LANJUT.
                    if let Err(e) = self.load_coverage_db_merge(&p) {
                        self.emit_warning(
                            mivon_core::diagnostics::DiagCode::IoError,
                            format!("$load_coverage_db: {}", e),
                        );
                    }
                }
                None => {
                    self.emit_warning(
                        mivon_core::diagnostics::DiagCode::IoError,
                        "$load_coverage_db requires a filename argument",
                    );
                }
            }
        } else if name == "swrite" || name == "sformat" {
            if let Some(IrExpr::Signal(out_id, _)) = ir_args.first() {
                let format_args = &ir_args[1..];
                let mut msg = self.format_display(format_args);
                if name == "swrite" {
                    msg.push('\n');
                }
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
                self.state.write_signal(
                    *out_id,
                    LogicVec {
                        width: bits.len(),
                        bits,
                    },
                );
            }
        } else if name == "sscanf" {
            if let Some(input_arg) = ir_args.first() {
                let input_str = if let IrExpr::String(s) = input_arg {
                    s.clone()
                } else if let Ok(val) = self.evaluate_expr(input_arg) {
                    logicvec_to_string(&val)
                } else {
                    String::new()
                };
                let fmt = ir_args.get(1).and_then(|a| {
                    if let IrExpr::String(s) = a {
                        Some(s.clone())
                    } else {
                        None
                    }
                });
                if let Some(ref fmt_str) = fmt {
                    let tokens: Vec<&str> = input_str.split_whitespace().collect();
                    let mut ti = 0;
                    let mut ai = 0;
                    let mut chars = fmt_str.chars().peekable();
                    while let Some(c) = chars.next() {
                        if c == '%' {
                            if let Some(spec) = chars.next() {
                                if spec == 'd' || spec == 'h' || spec == 'b' || spec == 'o' {
                                    if let Some(tok) = tokens.get(ti) {
                                        let radix = if spec == 'h' {
                                            16
                                        } else if spec == 'o' {
                                            8
                                        } else if spec == 'b' {
                                            2
                                        } else {
                                            10
                                        };
                                        if let Ok(val) = i64::from_str_radix(tok, radix) {
                                            if let Some(IrExpr::Signal(sid, _)) =
                                                ir_args.get(2 + ai)
                                            {
                                                self.state.write_signal(
                                                    *sid,
                                                    LogicVec::from_u64(val as u64, 32),
                                                );
                                            }
                                            ai += 1;
                                        }
                                    }
                                    ti += 1;
                                } else if spec == 's' {
                                    if let Some(IrExpr::Signal(sid, _)) = ir_args.get(2 + ai) {
                                        let s = tokens[ti..].join(" ");
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
                                        self.state.write_signal(
                                            *sid,
                                            LogicVec {
                                                width: bits.len(),
                                                bits,
                                            },
                                        );
                                    }
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        } else if name == "test$plusargs" {
            // $test$plusargs in statement context — return value ignored
        } else if name == "coverage_merge" {
            // Stub
        } else {
            // Try VPI registered system task/function (names need $ prefix)
            let vpi_name = format!("${}", name);
            if crate::vpi::systf::call_registered_systf(&vpi_name, false)
                || crate::vpi::systf::call_registered_systf(&vpi_name, true)
            {
                // Handled by VPI
            } else {
                self.emit_warning(
                    mivon_core::diagnostics::DiagCode::NotImplemented,
                    format!("unknown system call '{}' ignored", name),
                );
            }
        }
        Ok(true)
    }

    /// Handler bersama syscall bahasa (LANG-xx): $timeformat, $printtimescale,
    /// $scope/$showscopes, $deposit/$assign, $get_randcount/$get_randstate,
    /// $sdf_annotate. Dipanggil dari evaluate_syscall dan evaluate_syscall_stmt.
    /// Returns Ok(true) jika syscall ditangani di sini (synced, tidak yield),
    /// Ok(false) jika bukan tanggung jawab handler ini.
    fn evaluate_lang_syscall(&mut self, name: &str, ir_args: &[IrExpr]) -> Result<bool, SimError> {
        match name {
            "info" | "warning" | "error" | "fatal" => {
                // $info / $warning / $error / $fatal — severity system tasks
                // (LRM 1800 §20.2). Argumen diformat persis seperti $display.
                // $fatal menghentikan simulasi (setara $finish(1)); yang lain
                // hanya mencetak dan simulasi berlanjut.
                let msg = self.format_severity_message(ir_args);
                self.emit_severity(name, &msg);
                Ok(true)
            }
            "timeformat" => {
                // $timeformat(units, precision, suffix, min_width) — IEEE 1800
                let mut units = -9i64;
                let mut precision = 0i64;
                let mut suffix = String::new();
                let mut min_width = 0usize;
                if let Some(a) = ir_args.first() {
                    if let Ok(v) = self.evaluate_expr(a) {
                        units = v.to_u64() as i64;
                    }
                }
                if let Some(a) = ir_args.get(1) {
                    if let Ok(v) = self.evaluate_expr(a) {
                        precision = v.to_u64() as i64;
                    }
                }
                if let Some(IrExpr::String(s)) = ir_args.get(2) {
                    suffix = s.clone();
                }
                if let Some(a) = ir_args.get(3) {
                    if let Ok(v) = self.evaluate_expr(a) {
                        min_width = v.to_u64() as usize;
                    }
                }
                self.state.timeformat = crate::simulator::types::TimeFormat {
                    units,
                    precision,
                    suffix,
                    min_field_width: min_width,
                    base_units: self.state.timeformat.base_units,
                };
                Ok(true)
            }
            "printtimescale" => {
                // $printtimescale[(module)] — format LRM: "Time scale of (scope) is 1ns / 1ps"
                let ts = self
                    .design
                    .timescale
                    .clone()
                    .unwrap_or_else(|| ("1ns".to_string(), "1ps".to_string()));
                let scope = self
                    .current_scope_name
                    .clone()
                    .unwrap_or_else(|| self.design.top.name.to_string());
                println!("Time scale of ({}) is {} / {}", scope, ts.0, ts.1);
                Ok(true)
            }
            "showscopes" => {
                // $showscopes — print all hierarchical scopes (module names)
                let mut scopes: Vec<String> =
                    self.design.modules.keys().map(|s| s.to_string()).collect();
                scopes.sort();
                for s in &scopes {
                    println!("{}", s);
                }
                Ok(true)
            }
            // `$time;` / `$realtime;` sebagai STATEMENT (nilai dibuang).
            // Parser kini menerima bentuk ini (sebelumnya ditolak "expected
            // system call name after $" karena `time`/`realtime` keyword SV →
            // Token::Time/RealTime, bukan Token::Ident). Tanpa lengan di sini
            // bentuk yang sah-sah itu jatuh ke warning `unknown system call`
            // RT9003 — warning palsu pada source yang legal.
            //
            // IEEE 1800-2017 §20.7: keduanya system FUNCTION yang mengembalikan
            // waktu simulasi saat ini; sebagai statement nilainya dibuang,
            // jadi cukup dievaluasi lalu diabaikan (jalur ekspresi sudah
            // mengembalikan nilainya ke pemanggil).
            "time" | "realtime" => Ok(true),
            "scope" => {
                // $scope(name) — set current scope for $showscopes
                if let Some(a) = ir_args.first() {
                    if let Ok(v) = self.evaluate_expr(a) {
                        self.current_scope_name = Some(logicvec_to_string(&v));
                    }
                }
                Ok(true)
            }
            "deposit" => {
                // $deposit(sig, value) — deposit sekali, tidak persistent override.
                // Driver berikutnya tetap bisa menimpa (beda dgn $assign).
                if let (Some(IrExpr::Signal(id, _)), Some(val_arg)) =
                    (ir_args.first(), ir_args.get(1))
                {
                    let val = self.evaluate_expr(val_arg)?;
                    self.state.write_signal(*id, val);
                }
                Ok(true)
            }
            "assign" => {
                // $assign(sig, value) — procedural continuous assignment.
                // Override nilai signal; write berikutnya ditekan sampai $deassign
                // (dicek di write_lvalue via forced_signals).
                if let (Some(IrExpr::Signal(id, _)), Some(val_arg)) =
                    (ir_args.first(), ir_args.get(1))
                {
                    // Simpan nilai SEBELUM $assign (hanya untuk wire)
                    if !self.forced_signals.contains(id) {
                        let is_wire = self
                            .design
                            .top
                            .signals
                            .get(*id)
                            .map(|s| {
                                matches!(
                                    s.kind,
                                    mivon_ir::SignalKind::Wire | mivon_ir::SignalKind::Logic
                                )
                            })
                            .unwrap_or(false);
                        if is_wire {
                            if let Some(sig) = self.state.signals.get(*id) {
                                self.pre_force_values.insert(*id, sig.clone());
                            }
                        }
                    }
                    let val = self.evaluate_expr(val_arg)?;
                    self.state.write_signal(*id, val);
                    self.forced_signals.insert(*id);
                }
                Ok(true)
            }
            "deassign" => {
                if let Some(IrExpr::Signal(id, _)) = ir_args.first() {
                    self.forced_signals.remove(id);
                    // Restore nilai asli setelah $deassign
                    if let Some(saved) = self.pre_force_values.remove(id) {
                        if let Some(sig) = self.state.signals.get_mut(*id) {
                            *sig = saved;
                        }
                    }
                }
                Ok(true)
            }
            "get_randcount" => {
                if let Some(IrExpr::Signal(id, _)) = ir_args.first() {
                    self.state
                        .write_signal(*id, LogicVec::from_u64(self.rand_call_count, 32));
                }
                Ok(true)
            }
            "get_randstate" => {
                // Konsisten dengan expression-form di expr.rs (64-bit).
                if let Some(IrExpr::Signal(id, _)) = ir_args.first() {
                    self.state
                        .write_signal(*id, LogicVec::from_u64(self.rand_seed, 64));
                }
                Ok(true)
            }
            "sdf_annotate" => {
                // $sdf_annotate("file.sdf") — runtime SDF annotation
                if let Some(IrExpr::String(path)) = ir_args.first() {
                    let sdf_data =
                        crate::simulator::sdf::SdfData::parse_file(path).map_err(|e| {
                            self.diag_error(
                                DiagCode::DpiError,
                                format!("$sdf_annotate: SDF parse failed: {}", e),
                            )
                        })?;
                    self.annotate_sdf(&sdf_data)?;
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Handle a SysCall statement during evaluate_stmt_block (no delay/fork context).
    /// Simpler version that doesn't handle continuation-related syscalls.
    pub(crate) fn evaluate_syscall_stmt(
        &mut self,
        name: &str,
        ir_args: &[IrExpr],
    ) -> Result<(), SimError> {
        // VERIF-07: UVM DB call sebagai bare statement di jalur statement-block
        // (dipakai initial/always block biasa + CLI run) — dispatch sama dgn
        // evaluate_syscall. Tanpa ini `uvm_resource_db::set(...)` statement
        // masuk ke sini dan jatuh ke no-op (map tidak pernah terisi).
        if name.starts_with("uvm_config_db::")
            || name.starts_with("uvm_resource_db::")
            || name.starts_with("uvm_cmdline_processor::")
            || name.starts_with("uvm_root::")
            || name.starts_with("uvm_tr_database::")
        {
            self.execute_uvm_db_stmt(name, ir_args)?;
            return Ok(());
        }
        if self.evaluate_lang_syscall(name, ir_args)? {
            return Ok(());
        }
        if name == "display" || name == "write" {
            let msg = self.format_display(ir_args);
            if name == "display" {
                println!("{}", msg);
            } else {
                print!("{}", msg);
            }
        } else if name == "run_test" {
            // F18: run_test("name") dari statement (initial/program block).
            // Tanpa special-case di elaborator, statement ini di-eliminasi
            // sebagai side-effect-free; di sini efeknya dijalankan.
            let arg_vals: Vec<LogicVec> = ir_args
                .iter()
                .map(|a| self.evaluate_expr(a).unwrap_or(LogicVec::from_u64(0, 32)))
                .collect();
            let test_name = arg_vals.first().map(logicvec_to_string).unwrap_or_default();
            self.run_uvm_test(&test_name)?;
        } else if name == "strobe" {
            self.strobe_events.push(ir_args.to_vec());
        } else if name == "fstrobe" {
            let handle = ir_args
                .first()
                .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u32));
            if let Some(h) = handle {
                self.fstrobe_events.push((h, ir_args[1..].to_vec()));
            }
        } else if name == "fmonitor" {
            let handle = ir_args
                .first()
                .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u32));
            if let Some(h) = handle {
                let vals: Vec<LogicVec> = ir_args[1..]
                    .iter()
                    .map(|a| self.evaluate_expr(a).unwrap_or(LogicVec::from_u64(0, 32)))
                    .collect();
                self.fmonitor_map.insert(h, (ir_args[1..].to_vec(), vals));
            }
        } else if name == "monitor" {
            let vals: Vec<LogicVec> = ir_args
                .iter()
                .map(|a| self.evaluate_expr(a).unwrap_or(LogicVec::from_u64(0, 32)))
                .collect();
            self.monitor_args = Some(ir_args.to_vec());
            self.monitor_last_values = Some(vals);
        } else if name == "urandom" {
            self.rand_call_count += 1;
            let val: u32 = self.rng.gen();
            let sig_id = ir_args.first().and_then(|a| {
                if let IrExpr::Signal(id, _) = a {
                    Some(*id)
                } else {
                    None
                }
            });
            if let Some(sid) = sig_id {
                self.state
                    .write_signal(sid, LogicVec::from_u64(val as u64, 32));
            }
        } else if name == "urandom_range" {
            self.rand_call_count += 1;
            let args_eval: Vec<LogicVec> = ir_args
                .iter()
                .map(|a| self.evaluate_expr(a).unwrap_or(LogicVec::from_u64(0, 32)))
                .collect();
            let maxval = args_eval.first().map(|v| v.to_u64()).unwrap_or(0);
            let minval = args_eval.get(1).map(|v| v.to_u64()).unwrap_or(0);
            let val = if maxval <= minval {
                minval
            } else {
                let range = maxval - minval + 1;
                if range <= 1 {
                    minval
                } else {
                    minval + (self.rng.gen::<u64>() % range)
                }
            };
            let sig_id = ir_args.first().and_then(|a| {
                if let IrExpr::Signal(id, _) = a {
                    Some(*id)
                } else {
                    None
                }
            });
            if let Some(sid) = sig_id {
                self.state.write_signal(sid, LogicVec::from_u64(val, 32));
            }
        } else if name == "random" {
            self.rand_call_count += 1;
            if let Some(seed_arg) = ir_args.get(1) {
                if let Ok(seed_val) = self.evaluate_expr(seed_arg) {
                    let seed = seed_val.to_u64();
                    self.rng = rand::rngs::StdRng::seed_from_u64(seed);
                    self.rand_seed = seed;
                }
            }
            let val: i32 = self.rng.gen();
            let sig_id = ir_args.first().and_then(|a| {
                if let IrExpr::Signal(id, _) = a {
                    Some(*id)
                } else {
                    None
                }
            });
            if let Some(sid) = sig_id {
                self.state
                    .write_signal(sid, LogicVec::from_u64(val as u64, 32));
            }
        } else if name == "srandom" {
            // $srandom as system task: sets seed for $random, does NOT increment rand_call_count
            // Args: seed (ir_args[0]) - no target signal for $srandom
            if let Some(seed_arg) = ir_args.first() {
                if let Ok(seed_val) = self.evaluate_expr(seed_arg) {
                    let seed = seed_val.to_u64();
                    self.rng = rand::rngs::StdRng::seed_from_u64(seed);
                    self.rand_seed = seed;
                }
            }
        } else if name == "urandom_seed" {
            // $urandom_seed as system task: sets seed for $urandom/$urandom_range, increments rand_call_count
            // Args: seed (ir_args[0]) - no target signal for $urandom_seed
            if let Some(seed_arg) = ir_args.first() {
                if let Ok(seed_val) = self.evaluate_expr(seed_arg) {
                    let seed = seed_val.to_u64();
                    self.rng = rand::rngs::StdRng::seed_from_u64(seed);
                    self.rand_seed = seed;
                    self.rand_call_count += 1;
                }
            }
        } else if name == "dumpfile" {
            if let Some(IrExpr::String(fname)) = ir_args.first() {
                let path = fname.clone();
                let design = &self.design;
                let state = &self.state.signals;
                if let Some(ref mut vcd) = self.vcd {
                    let _ = vcd.reopen(&path, design, state);
                } else {
                    match VcdWriter::new(&path, design) {
                        Ok(v) => self.vcd = Some(v),
                        // F20: via DiagSink agar error punya file:line:col.
                        Err(e) => {
                            let _ = self.diag_error_at(
                                mivon_core::diagnostics::DiagCode::WaveformError,
                                format!("VCD: cannot create '{}': {}", path, e),
                                self.cur_src_pos().0,
                                self.cur_src_pos().1,
                            );
                        }
                    }
                }
            }
        } else if name == "dumpall" {
            if let Some(ref mut vcd) = self.vcd {
                vcd.write_time_header(self.state.time)?;
                let design = &self.design;
                let state = &self.state.signals;
                vcd.dump_all(design, state)?;
            }
        } else if name == "dumplimit" {
            if let Some(limit) = ir_args
                .first()
                .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64()))
            {
                if let Some(ref mut vcd) = self.vcd {
                    vcd.max_dump_size = Some(limit);
                }
            }
        } else if name == "dumpvars" || name == "dumpon" {
            if let Some(ref mut vcd) = self.vcd {
                vcd.enabled = true;
            }
        } else if name == "dumpoff" {
            if let Some(ref mut vcd) = self.vcd {
                vcd.enabled = false;
            }
        } else if name == "fopen" {
            let fname = ir_args.first().and_then(|a| {
                if let IrExpr::String(s) = a {
                    Some(s.clone())
                } else {
                    None
                }
            });
            if let Some(fname) = fname {
                let mode = ir_args.get(1).and_then(|a| {
                    if let IrExpr::String(s) = a {
                        Some(s.as_str())
                    } else {
                        None
                    }
                });
                let open_result = match mode {
                    Some("r") | Some("rb") => std::fs::File::open(&fname),
                    _ => std::fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .create(true)
                        .truncate(true)
                        .open(&fname),
                };
                match open_result {
                    Ok(f) => {
                        let handle = self.next_file_handle;
                        self.next_file_handle += 1;
                        self.file_handles.insert(handle, f);
                        self.file_read_pos.insert(handle, 0);
                        let sig_id = ir_args.get(1).and_then(|a| {
                            if let IrExpr::Signal(id, _) = a {
                                Some(*id)
                            } else {
                                None
                            }
                        });
                        if let Some(sid) = sig_id {
                            self.state
                                .write_signal(sid, LogicVec::from_u64(handle as u64, 32));
                        }
                    }
                    Err(_) => {
                        let sig_id = ir_args.get(1).and_then(|a| {
                            if let IrExpr::Signal(id, _) = a {
                                Some(*id)
                            } else {
                                None
                            }
                        });
                        if let Some(sid) = sig_id {
                            self.state.write_signal(sid, LogicVec::from_u64(0, 32));
                        }
                    }
                }
            }
        } else if name == "fdisplay" {
            let handle = ir_args
                .first()
                .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u32));
            if let Some(h) = handle {
                let msg = self.format_display(&ir_args[1..]);
                if let Some(f) = self.file_handles.get_mut(&h) {
                    // LRM: $fdisplay = $fwrite + newline (VCS/iverilog patuh;
                    // mivon sebelumnya concat tanpa '\n' — artefak differential).
                    let _ = writeln!(f, "{}", msg);
                }
            }
        } else if name == "fwrite" {
            let handle = ir_args
                .first()
                .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u32));
            if let Some(h) = handle {
                let msg = self.format_display(&ir_args[1..]);
                if let Some(f) = self.file_handles.get_mut(&h) {
                    let _ = write!(f, "{}", msg);
                }
            }
        } else if name == "fscanf" {
            let handle = ir_args
                .first()
                .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u32));
            if let Some(h) = handle {
                if let Some(f) = self.file_handles.get_mut(&h) {
                    use std::io::{Read, Seek};
                    let read_pos = self.file_read_pos.entry(h).or_insert(0);
                    f.seek(std::io::SeekFrom::Start(*read_pos)).ok();
                    let mut content = String::new();
                    let _bytes_read = f.read_to_string(&mut content).unwrap_or(0);
                    *read_pos = f.stream_position().unwrap_or(0);
                    let fmt = ir_args.get(1).and_then(|a| {
                        if let IrExpr::String(s) = a {
                            Some(s.clone())
                        } else {
                            None
                        }
                    });
                    if let Some(ref fmt_str) = fmt {
                        let tokens: Vec<&str> = content.split_whitespace().collect();
                        let mut ti = 0;
                        let mut ai = 0;
                        let mut chars = fmt_str.chars().peekable();
                        while let Some(c) = chars.next() {
                            if c == '%' {
                                if let Some(spec) = chars.next() {
                                    if spec == 'd' || spec == 'h' || spec == 'b' {
                                        if let Some(tok) = tokens.get(ti) {
                                            if let Ok(val) = if spec == 'h' {
                                                i64::from_str_radix(tok, 16)
                                            } else if spec == 'b' {
                                                i64::from_str_radix(tok, 2)
                                            } else {
                                                tok.parse::<i64>()
                                            } {
                                                let out_idx = 2 + ai;
                                                if let Some(IrExpr::Signal(sid, _)) =
                                                    ir_args.get(out_idx)
                                                {
                                                    self.state.write_signal(
                                                        *sid,
                                                        LogicVec::from_u64(val as u64, 32),
                                                    );
                                                }
                                                ai += 1;
                                            }
                                        }
                                        ti += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        } else if name == "fread" {
            let target = ir_args.first().and_then(|a| {
                if let IrExpr::Signal(id, _) = a {
                    Some(*id)
                } else {
                    None
                }
            });
            let src = ir_args.get(1);
            let data = if let Some(IrExpr::String(fname)) = src {
                std::fs::read(fname).ok()
            } else if let Some(arg) = src {
                let handle = self
                    .evaluate_expr(arg)
                    .ok()
                    .map(|v| v.to_u64() as u32)
                    .unwrap_or(0);
                if handle > 0 {
                    use std::io::Read;
                    self.file_handles.get_mut(&handle).and_then(|f| {
                        let mut buf = Vec::new();
                        f.read_to_end(&mut buf).ok().map(|_| buf)
                    })
                } else {
                    None
                }
            } else {
                None
            };
            if let (Some(sid), Some(bytes)) = (target, data) {
                let mut bits = Vec::with_capacity(bytes.len() * 8);
                for byte in bytes {
                    for i in 0..8 {
                        bits.push(if (byte >> i) & 1 == 1 {
                            LogicVal::One
                        } else {
                            LogicVal::Zero
                        });
                    }
                }
                self.state.write_signal(
                    sid,
                    LogicVec {
                        width: bits.len(),
                        bits,
                    },
                );
            }
        } else if name == "fclose" {
            let handle = ir_args
                .first()
                .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u32));
            if let Some(h) = handle {
                self.file_handles.remove(&h);
            }
        } else if name == "fflush" {
            let handle = ir_args
                .first()
                .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u32));
            if let Some(h) = handle {
                if let Some(f) = self.file_handles.get_mut(&h) {
                    let _ = f.flush();
                }
            }
        } else if name == "fseek" {
            let handle = ir_args
                .first()
                .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as u32));
            let offset = ir_args
                .get(1)
                .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64() as i64));
            let op = ir_args
                .get(2)
                .and_then(|a| self.evaluate_expr(a).ok().map(|v| v.to_u64()));
            if let (Some(h), Some(off)) = (handle, offset) {
                if let Some(f) = self.file_handles.get_mut(&h) {
                    use std::io::{Seek, SeekFrom};
                    let seek_from = match op {
                        Some(1) => SeekFrom::Current(off),
                        Some(2) => SeekFrom::End(off),
                        _ => SeekFrom::Start(off as u64),
                    };
                    let _ = f.seek(seek_from);
                    if let Ok(pos) = f.stream_position() {
                        self.file_read_pos.insert(h, pos);
                    }
                }
            }
        } else if name == "__dpi_stmt" {
            if let Some(arg) = ir_args.first() {
                self.evaluate_expr(arg)?;
            }
        } else if name == "value$plusargs" {
            let pattern = ir_args
                .first()
                .and_then(|a| self.evaluate_expr(a).ok())
                .map(|v| logicvec_to_string(&v))
                .unwrap_or_default();
            let plusarg_name = pattern
                .split('%')
                .next()
                .unwrap_or(&pattern)
                .trim_end_matches('=');
            let plusargs = self.plusargs.clone();
            for (key, val) in &plusargs {
                if key == plusarg_name {
                    if let Some(var_arg) = ir_args.get(1) {
                        let num = if let Some(hex) =
                            val.strip_prefix("0x").or_else(|| val.strip_prefix("0X"))
                        {
                            u64::from_str_radix(hex, 16).unwrap_or(0)
                        } else {
                            val.parse::<u64>().unwrap_or(0)
                        };
                        if std::env::var("DBG_PA").is_ok() {
                            eprintln!(
                                "[PA] pattern='{}' name='{}' val='{}' has_sig={}",
                                pattern,
                                plusarg_name,
                                val,
                                matches!(ir_args.get(1), Some(IrExpr::Signal(..)))
                            );
                        }
                        // Utk `%s`: tulis STRING ke string-signal (bukan nol).
                        let sig_id = if let IrExpr::Signal(id, _) = var_arg {
                            *id
                        } else {
                            break;
                        };
                        let is_str = self
                            .design
                            .top
                            .signals
                            .get(sig_id)
                            .map(|si| si.is_string)
                            .unwrap_or(false);
                        if std::env::var("DBG_PA").is_ok() {
                            eprintln!("[PA] sig={} is_str={}", sig_id, is_str);
                        }
                        if is_str {
                            self.state.write_signal(
                                sig_id,
                                crate::simulator::util::string_to_logicvec(val),
                            );
                        } else {
                            self.state.write_signal(sig_id, LogicVec::from_u64(num, 32));
                        }
                    }
                    break;
                }
            }
        } else if name == "asserton" {
            self.assert_off_all = false;
        } else if name == "assertoff" {
            self.assert_off_all = true;
            if let Some(scope_arg) = ir_args.first() {
                if let Ok(scope_val) = self.evaluate_expr(scope_arg) {
                    let scope_name = logicvec_to_string(&scope_val);
                    self.assert_modules_off.insert(Symbol::intern(&scope_name));
                }
            }
        } else if name == "assertkill" {
            self.assert_kill_all = true;
            self.assert_off_all = true;
            if let Some(scope_arg) = ir_args.first() {
                if let Ok(scope_val) = self.evaluate_expr(scope_arg) {
                    let scope_name = logicvec_to_string(&scope_val);
                    self.assert_modules_off.insert(Symbol::intern(&scope_name));
                }
            }
        } else if name == "assertpasson" || name == "assertfailon" || name == "assertnonvacuouson" {
            // Stubs
        } else if name == "isunbounded" {
            if let Some(IrExpr::Signal(id, _)) = ir_args.first() {
                self.state.write_signal(*id, LogicVec::from_u64(0, 1));
            }
        } else if name == "coverage_control" {
            if let Some(arg) = ir_args.first() {
                if let Ok(val) = self.evaluate_expr(arg) {
                    let bitmask = val.to_u64();
                    self.apply_coverage_control(bitmask);
                }
            }
        } else if name == "coverage_get" {
            let mut total = 0u64;
            let mut hit = 0u64;
            for cg in &self.design.covergroups {
                for cp in &cg.coverpoints {
                    let key = format!("{}.{}", cg.name, cp.name);
                    let key_sym = Symbol::intern(&key);
                    if let Some(t) = self.cover_total.get(&key_sym) {
                        total += t;
                    }
                    if let Some(h) = self.cover_hits.get(&key_sym) {
                        hit += h;
                    }
                }
            }
            let pct = if total > 0 {
                (hit as f64 / total as f64) * 100.0
            } else {
                0.0
            };
            if let Some(IrExpr::Signal(id, _)) = ir_args.first() {
                self.state
                    .write_signal(*id, LogicVec::from_u64(pct as u64, 64));
            }
        } else if name == "coverage_save" {
            let path = ir_args
                .first()
                .and_then(|a| {
                    if let IrExpr::String(s) = a {
                        Some(s.clone())
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| "coverage.ucis".to_string());
            let _ = self.export_coverage_ucis(&path);
        } else if name == "coverage_model" {
            let cg_name = ir_args.get(1).and_then(|a| {
                if let IrExpr::String(s) = a {
                    Some(s.clone())
                } else {
                    None
                }
            });
            let handle: u32 = if let Some(ref name) = cg_name {
                let exists = self
                    .design
                    .covergroups
                    .iter()
                    .any(|cg| cg.name.as_str() == name.as_str());
                if exists {
                    if let Some((&h, _)) = self
                        .coverage_model_handles
                        .iter()
                        .find(|(_, n)| n.as_str() == name.as_str())
                    {
                        h as u32
                    } else {
                        let h = self.next_coverage_model_handle;
                        self.next_coverage_model_handle += 1;
                        self.coverage_model_handles.insert(h, Symbol::intern(name));
                        h as u32
                    }
                } else {
                    self.emit_warning(
                        mivon_core::diagnostics::DiagCode::NotImplemented,
                        format!("$coverage_model: covergroup '{}' not found", name),
                    );
                    0
                }
            } else if let Some(first_cg) = self.design.covergroups.first() {
                if let Some((&h, _)) = self
                    .coverage_model_handles
                    .iter()
                    .find(|(_, n)| n.as_str() == first_cg.name.as_str())
                {
                    h as u32
                } else {
                    let h = self.next_coverage_model_handle;
                    self.next_coverage_model_handle += 1;
                    self.coverage_model_handles.insert(h, first_cg.name);
                    h as u32
                }
            } else {
                0
            };
            if let Some(IrExpr::Signal(id, _)) = ir_args.first() {
                self.state
                    .write_signal(*id, LogicVec::from_u64(handle as u64, 32));
            }
        } else if name == "load_coverage_db" {
            // `$load_coverage_db(path)` — muat MCDB lalu merge ke counter
            // live (line/branch/toggle/FSM/covergroup). Gagal load =
            // warning IoError (sim LANJUT, pola $fopen yg gagal = handle 0).
            let path = ir_args.first().and_then(|a| match a {
                IrExpr::String(s) => Some(s.clone()),
                IrExpr::Signal(sid, _) => {
                    let is_str = self
                        .design
                        .top
                        .signals
                        .get(*sid)
                        .map(|si| si.is_string)
                        .unwrap_or(false);
                    if is_str {
                        let lv = self.state.read_signal(*sid);
                        Some(crate::simulator::util::logicvec_to_string(lv))
                    } else {
                        None
                    }
                }
                _ => None,
            });
            match path {
                Some(p) => {
                    // Sukses = silent merge (seperti VCS — tak ada output).
                    // Gagal = warning IoError, sim LANJUT.
                    if let Err(e) = self.load_coverage_db_merge(&p) {
                        self.emit_warning(
                            mivon_core::diagnostics::DiagCode::IoError,
                            format!("$load_coverage_db: {}", e),
                        );
                    }
                }
                None => {
                    self.emit_warning(
                        mivon_core::diagnostics::DiagCode::IoError,
                        "$load_coverage_db requires a filename argument",
                    );
                }
            }
        } else if name == "swrite" || name == "sformat" {
            if let Some(IrExpr::Signal(out_id, _)) = ir_args.first() {
                let format_args = &ir_args[1..];
                let mut msg = self.format_display(format_args);
                if name == "swrite" {
                    msg.push('\n');
                }
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
                self.state.write_signal(
                    *out_id,
                    LogicVec {
                        width: bits.len(),
                        bits,
                    },
                );
            }
        } else if name == "sscanf" {
            if let Some(input_arg) = ir_args.first() {
                let input_str = if let IrExpr::String(s) = input_arg {
                    s.clone()
                } else if let Ok(val) = self.evaluate_expr(input_arg) {
                    logicvec_to_string(&val)
                } else {
                    String::new()
                };
                let fmt = ir_args.get(1).and_then(|a| {
                    if let IrExpr::String(s) = a {
                        Some(s.clone())
                    } else {
                        None
                    }
                });
                if let Some(ref fmt_str) = fmt {
                    let tokens: Vec<&str> = input_str.split_whitespace().collect();
                    let mut ti = 0;
                    let mut ai = 0;
                    let mut chars = fmt_str.chars().peekable();
                    while let Some(c) = chars.next() {
                        if c == '%' {
                            if let Some(spec) = chars.next() {
                                if spec == 'd' || spec == 'h' || spec == 'b' || spec == 'o' {
                                    if let Some(tok) = tokens.get(ti) {
                                        let radix = if spec == 'h' {
                                            16
                                        } else if spec == 'o' {
                                            8
                                        } else if spec == 'b' {
                                            2
                                        } else {
                                            10
                                        };
                                        if let Ok(val) = i64::from_str_radix(tok, radix) {
                                            if let Some(IrExpr::Signal(sid, _)) =
                                                ir_args.get(2 + ai)
                                            {
                                                self.state.write_signal(
                                                    *sid,
                                                    LogicVec::from_u64(val as u64, 32),
                                                );
                                            }
                                            ai += 1;
                                        }
                                    }
                                    ti += 1;
                                } else if spec == 's' {
                                    if let Some(IrExpr::Signal(sid, _)) = ir_args.get(2 + ai) {
                                        let s = tokens[ti..].join(" ");
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
                                        self.state.write_signal(
                                            *sid,
                                            LogicVec {
                                                width: bits.len(),
                                                bits,
                                            },
                                        );
                                    }
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        } else if name == "test$plusargs" {
            // $test$plusargs in statement context — return value ignored
        } else if name == "coverage_merge" {
            // Stub
        } else {
            // Try VPI registered system task/function (names need $ prefix)
            let vpi_name = format!("${}", name);
            if !crate::vpi::systf::call_registered_systf(&vpi_name, false)
                && !crate::vpi::systf::call_registered_systf(&vpi_name, true)
            {
                self.emit_warning(
                    mivon_core::diagnostics::DiagCode::NotImplemented,
                    format!("unknown system call '{}' ignored", name),
                );
            }
        }
        Ok(())
    }
}
