use super::super::SequenceAttempt;
use super::super::SimulationEngine;
use crate::simulator::types::*;
use crate::simulator::util::*;
use mivon_core::diagnostics::DiagCode;
use mivon_core::error::SimError;
use mivon_core::Symbol;
use mivon_ir::*;
use rand::Rng;

/// Cap iterasi loop AST (method/task context) untuk mencegah hang ketika loop
/// berisi blocking event (`@(...)`) yang tidak memajukan waktu simulasi.
const AST_LOOP_ITER_CAP: u64 = 100_000;

/// GUARD `#0` churn (BUG FIX fuZZ; requirement eksekusi <10s, ideal <1s):
/// jumlah `#0`-zero-delay SAMA-WAKTU per time step sebelum abort InfiniteDelta.
/// 4096 × ~400µs (debug, dgn kloning body) ≈ <2s — jauh di bawah delta-limit
/// 100k yang baru kena setelah puluhan detik debug. `#0` tunggal (reset idiom)
/// tidak pernah mendekati cap ini.
pub(super) const ZERO_DELAY_EVENT_CAP: u64 = 4096;

impl SimulationEngine {
    /// F47: `disable <label>` (bukan `disable fork`).
    /// - Target ADA di stack aktif → set `disable_pending` + hentikan blok
    ///   saat ini (wrapper `evaluate_named_block_fork` mengonsumsi saat
    ///   unwind, lalu eksekusi lanjut SETELAH blok).
    /// - Target DI LUAR stack (branch lain suspend / belum masuk) → set
    ///   `disable_cross` dan JANGAN hentikan blok pelaku (LRM §9.6.4 —
    ///   pelaku tetap lanjut). Dikonsumsi saat kontinuasi target di-resume
    ///   (`Continuation::named_labels`) atau saat blok target dievaluasi.
    ///
    /// Return true = blok saat ini harus berhenti.
    pub(crate) fn mark_disable_label(&mut self, name: Symbol) -> bool {
        if self.active_named_labels.contains(&name) {
            self.disable_pending = Some(name);
            true
        } else if self.known_named_labels.contains(&name) {
            // Label dikenal design tapi TIDAK di stack proses ini — target
            // di branch/proses lain yang sedang suspend (atau belum masuk).
            self.disable_cross = Some(name);
            false
        } else {
            // Label bukan NamedBlock (dibuang parser / typo) — fallback
            // perilaku lama: bunuh sisa proses pemanggil.
            self.disable_pending = Some(name);
            true
        }
    }

    /// F47: konsumsi `disable_cross` bila target cocok dengan label blok
    /// yang baru dievaluasi (target belum sempat masuk saat disable).
    pub(crate) fn consume_disable_cross_for(&mut self, name: Symbol) -> bool {
        if self.disable_cross == Some(name) {
            self.disable_cross = None;
            true
        } else {
            false
        }
    }

    /// F26: mulai eksekusi branch fork — aktifkan fork id utk task body
    /// (execute_method_body memakainya supaya continuation resume decrement
    /// fork yang benar) + reset flag suspend task utk branch ini.
    pub(crate) fn fork_branch_begin(&mut self, fid: usize) {
        self.active_fork_id = Some(fid);
        self.task_suspended = false;
    }

    /// F26: selesai eksekusi branch fork — decrement HANYA bila branch selesai
    /// langsung (all_consumed) DAN tidak men-suspend task method. Branch yang
    /// men-suspend task (task_suspended) di-decrement oleh resume
    /// (ContinueAstBlock dgn fork_id → event.rs fork_decrement) — tanpa ini,
    /// `fork drv.run(); ...; join` di module initial selesai premature
    /// (F24 limitation: HANDSHAKE tercetak sebelum driver selesai).
    pub(crate) fn fork_branch_end(
        &mut self,
        fid: usize,
        all_consumed: bool,
    ) -> Result<(), SimError> {
        self.active_fork_id = None;
        if std::env::var("DBG_UVM").is_ok() {
            eprintln!(
                "[DBG-F26] branch_end fid={} consumed={} suspended={}",
                fid, all_consumed, self.task_suspended
            );
        }
        if self.task_suspended {
            self.task_suspended = false;
            Ok(())
        } else if all_consumed {
            self.fork_decrement(fid)
        } else {
            Ok(())
        }
    }

    pub(crate) fn evaluate_block_with_delay(&mut self, stmts: &[IrStmt]) -> Result<bool, SimError> {
        self.evaluate_block_with_delay_fork(stmts, None)
    }

    /// Eksekusi `f` dengan `loop_continuation` sementara = tail + old_cont.
    /// F31 fix: saat inner block/if/case mengandung delay/event dan suspend,
    /// continuation yang dibuat delay handler (`later = body + sisa + loop_cont`)
    /// otomatis menyertakan statement SETELAH konstruksi ini (tail) — tanpa
    /// ini statement setelah `begin #1; end $display` hilang (sim hang).
    /// Setelah `f` selesai, loop_continuation di-restore (tanpa efek samping
    /// bila inner tidak suspend).
    fn with_tail_continuation(
        &mut self,
        tail: &[IrStmt],
        f: impl FnOnce(&mut Self) -> Result<bool, SimError>,
    ) -> Result<bool, SimError> {
        let old_loop_cont = self.loop_continuation.clone();
        let mut lc = tail.to_vec();
        if let Some(cont) = &old_loop_cont {
            lc.extend(cont.clone());
        }
        self.loop_continuation = Some(lc);
        let r = f(self);
        self.loop_continuation = old_loop_cont;
        r
    }

    pub(crate) fn evaluate_block_with_delay_fork(
        &mut self,
        stmts: &[IrStmt],
        fork_id: Option<usize>,
    ) -> Result<bool, SimError> {
        for (i, stmt) in stmts.iter().enumerate() {
            // $fatal menghentikan blok seketika (final block tetap jalan
            // setelah $finish/$fatal — hanya flag fatal_hit yang abort).
            if self.fatal_hit {
                return Ok(true);
            }
            if self.disable_pending.is_some() {
                return Ok(true);
            }
            if self.control_flow.is_some() {
                return Ok(true);
            }
            // Line coverage: record every statement execution (also covers fork path)
            if self.coverage_enabled {
                if let Some(pname) = self.current_process_name.clone() {
                    self.record_line_hit(stmt, &pname);
                }
            }
            match stmt {
                IrStmt::Block { stmts: inner } => {
                    // F31: tail statement setelah block harus ikut continuation
                    // saat inner suspend (block ber-delay).
                    if !self.with_tail_continuation(&stmts[i + 1..], |s| {
                        s.evaluate_block_fork(inner, fork_id)
                    })? {
                        return Ok(false);
                    }
                }
                IrStmt::NamedBlock {
                    name, stmts: inner, ..
                } => {
                    // F31: sama seperti Block — tail ikut continuation.
                    if !self.with_tail_continuation(&stmts[i + 1..], |s| {
                        s.evaluate_named_block_fork(*name, inner, fork_id)
                    })? {
                        return Ok(false);
                    }
                }
                IrStmt::If {
                    cond,
                    true_branch: then_stmts,
                    false_branch: else_stmts,
                } => {
                    // F31: cabang if bisa suspend (delay) — tail ikut continuation.
                    if !self.with_tail_continuation(&stmts[i + 1..], |s| {
                        s.evaluate_if_fork(cond, then_stmts, else_stmts, fork_id)
                    })? {
                        return Ok(false);
                    }
                }
                IrStmt::Case {
                    case_type,
                    expr: case_expr,
                    items,
                    default,
                } => {
                    // F31: item case bisa suspend — tail ikut continuation.
                    if !self.with_tail_continuation(&stmts[i + 1..], |s| {
                        s.evaluate_case_fork(case_type, case_expr, items, default, fork_id)
                    })? {
                        return Ok(false);
                    }
                }
                IrStmt::BlockingAssign { lhs, rhs, delay } => {
                    if !self.is_forced(lhs) {
                        let val = self.eval_assign_rhs(rhs, lhs)?;
                        if let Some(d) = delay {
                            if *d > 0 {
                                // Intra-assignment delay `lhs = #d rhs`: RHS
                                // di-sampling SEKARANG, write dilakukan saat
                                // t+d (≡ `#d; lhs = rhs_sampled;`).
                                let delay_t = self.state.time as usize + *d as usize;
                                self.ensure_events(delay_t);
                                let mut later: Vec<IrStmt> = vec![IrStmt::BlockingAssign {
                                    lhs: lhs.clone(),
                                    rhs: IrExpr::Const(val),
                                    delay: None,
                                }];
                                later.extend(stmts[i + 1..].to_vec());
                                if let Some(loop_cont) = &self.loop_continuation {
                                    later.extend(loop_cont.clone());
                                }
                                if !later.is_empty() {
                                    let pid = self.current_process_id;
                                    self.push_event(
                                        delay_t,
                                        RegionEvent {
                                            region: EventRegion::Active,
                                            event: EventKind::ContinueBlock(Continuation {
                                                stmts_to_exec: later,
                                                stmts_remaining: vec![],
                                                fork_id,
                                                process_id: pid,
                                                process_name: self.current_process_name.clone(),
                                                named_labels: self.active_named_labels.clone(),
                                            }),
                                        },
                                    );
                                }
                                return Ok(false);
                            }
                        }
                        self.write_lvalue(lhs, val, true)?;
                    }
                }
                IrStmt::NonBlockingAssign { lhs, rhs, delay } => {
                    if !self.is_forced(lhs) {
                        let val = self.eval_assign_rhs(rhs, lhs)?;
                        if let Some(d) = delay {
                            if *d > 0 {
                                // Intra-assignment delay `lhs <= #d rhs`: RHS
                                // di-sampling SEKARANG, NBA di-queue saat t+d
                                // (≡ `#d; lhs <= rhs_sampled;`).
                                let delay_t = self.state.time as usize + *d as usize;
                                self.ensure_events(delay_t);
                                let mut later: Vec<IrStmt> = vec![IrStmt::NonBlockingAssign {
                                    lhs: lhs.clone(),
                                    rhs: IrExpr::Const(val),
                                    delay: None,
                                }];
                                later.extend(stmts[i + 1..].to_vec());
                                if let Some(loop_cont) = &self.loop_continuation {
                                    later.extend(loop_cont.clone());
                                }
                                if !later.is_empty() {
                                    let pid = self.current_process_id;
                                    self.push_event(
                                        delay_t,
                                        RegionEvent {
                                            region: EventRegion::Active,
                                            event: EventKind::ContinueBlock(Continuation {
                                                stmts_to_exec: later,
                                                stmts_remaining: vec![],
                                                fork_id,
                                                process_id: pid,
                                                process_name: self.current_process_name.clone(),
                                                named_labels: self.active_named_labels.clone(),
                                            }),
                                        },
                                    );
                                }
                                return Ok(false);
                            }
                        }
                        self.push_nba_pending(lhs.clone(), val);
                    }
                }
                IrStmt::Force { lvalue, rhs } => {
                    // LRM §10.6.2: wire kembali ke driver saat release;
                    // reg TETAP di nilai forced. Simpan hanya untuk wire.
                    if let Some(id) = self.signal_id_from_lvalue(lvalue) {
                        if !self.forced_signals.contains(&id) {
                            let is_wire = self
                                .design
                                .top
                                .signals
                                .get(id)
                                .map(|s| {
                                    matches!(
                                        s.kind,
                                        mivon_ir::SignalKind::Wire | mivon_ir::SignalKind::Logic
                                    )
                                })
                                .unwrap_or(false);
                            if is_wire {
                                if let Some(sig) = self.state.signals.get(id) {
                                    self.pre_force_values.insert(id, sig.clone());
                                }
                            }
                        }
                    }
                    let val = self.eval_assign_rhs(rhs, lvalue)?;
                    self.write_lvalue(lvalue, val, true)?;
                    if let Some(id) = self.signal_id_from_lvalue(lvalue) {
                        self.forced_signals.insert(id);
                    }
                }
                IrStmt::Delay { delay, body } => {
                    let delay_val = *delay as usize;
                    let delay_t = self.state.time as usize + delay_val;
                    // ── Guard #0 churn (BUG FIX fuZZ; requirement <10s/<1s) ──
                    // `#0` dalam loop = re-schedule Inactive sambil kloning
                    // body — delta-limit 100k baru kena puluhan detik (debug).
                    // Hitung `#0` SAMA-WAKTU per time step → abort cepat.
                    if delay_val == 0 {
                        if self.state.time != self.zero_delay_seen_t {
                            self.zero_delay_seen_t = self.state.time;
                            self.zero_delay_same_time = 0;
                        }
                        self.zero_delay_same_time += 1;
                        if self.zero_delay_same_time
                            > crate::simulator::engine::scheduler::block::ZERO_DELAY_EVENT_CAP
                        {
                            return Err(SimError::with_diag(
                                DiagCode::InfiniteDelta,
                                format!(
                                    "#0-zero-delay churn: {} zero-delay events tanpa time advance di t={} — kemungkinan loop `#0` (mis. `forever #0 clk = ~clk`). Abort cepat (limit {}).",
                                    self.zero_delay_same_time,
                                    self.state.time,
                                    crate::simulator::engine::scheduler::block::ZERO_DELAY_EVENT_CAP
                                ),
                            ));
                        }
                    }
                    self.ensure_events(delay_t);
                    let mut later: Vec<IrStmt> = body.clone();
                    let remaining: Vec<IrStmt> = stmts[i + 1..].to_vec();
                    later.extend(remaining);
                    if let Some(loop_cont) = &self.loop_continuation {
                        later.extend(loop_cont.clone());
                    }
                    if !later.is_empty() {
                        let region = if delay_val == 0 {
                            EventRegion::Inactive
                        } else {
                            EventRegion::Active
                        };
                        let pid = self.current_process_id;
                        self.push_event(
                            delay_t,
                            RegionEvent {
                                region,
                                event: EventKind::ContinueBlock(Continuation {
                                    stmts_to_exec: later,
                                    stmts_remaining: vec![],
                                    fork_id,
                                    process_id: pid,
                                    process_name: self.current_process_name.clone(),
                                    named_labels: self.active_named_labels.clone(),
                                }),
                            },
                        );
                    }
                    return Ok(false);
                }
                IrStmt::EventControl { sigs, body, iff } => {
                    // Blocking event control `@(a or posedge b)`:
                    // SELALU suspend — tunggu perubahan/edge berikutnya. Event
                    // yang sudah lewat (mis. clk sudah high) TIDAK dihitung.
                    // SATU entry mewakili SATU `@(...)` — fire sekali.
                    let mut later: Vec<IrStmt> = body.clone();
                    later.extend(stmts[i + 1..].to_vec());
                    if let Some(lc) = &self.loop_continuation {
                        later.extend(lc.clone());
                    }
                    // F27: resolve ClockEdge::*Hier (event `@(posedge b.clk)`
                    // lewat port interface) ke SignalId via hier_signal_map.
                    let sigs = self.normalize_event_sigs(sigs);
                    self.pending_events.push(PendingEventControl {
                        sigs,
                        continuation: later,
                        // LANG-27: guard `iff (cond)` — continuation hanya
                        // dilanjutkan bila kondisi benar saat event terpenuhi.
                        iff: iff.clone(),
                        // Branch fork tempat @(...) berada — dipakai decrement
                        // ForkGroup saat resume (cegah join menggantung).
                        fork_id,
                    });
                    return Ok(false);
                }
                IrStmt::EventTrigger { sig_id } => {
                    let val = self.state.read_signal(*sig_id);
                    let toggled = if val.to_bool().unwrap_or(false) {
                        LogicVec::from_u64(0, val.width.max(1))
                    } else {
                        LogicVec::from_u64(1, val.width.max(1))
                    };
                    self.state.write_signal(*sig_id, toggled);
                }
                IrStmt::Disable { name } => {
                    // LANG-30: `disable fork` men-terminate SEMUA child process
                    // milik proses ini (IEEE 1800-2017 §9.6.4) — group di-tandai
                    // disabled, branch tertunda di-skip saat resume. Proses
                    // pemanggil LANJUT. `disable <label>` membunuh blok bernama
                    // (disable_pending → hentikan blok saat ini).
                    if name.as_str() == "fork" {
                        let fids = self.active_fork_groups_for_current_process();
                        for fid in fids {
                            if let Some(g) = self.fork_groups.get_mut(fid) {
                                g.disabled = true;
                            }
                        }
                    } else if self.mark_disable_label(*name) {
                        return Ok(true);
                    }
                }
                IrStmt::Release { lvalue } => {
                    if let Some(id) = self.signal_id_from_lvalue(lvalue) {
                        self.forced_signals.remove(&id);
                        // LRM §10.6.2: wire kembali ke driver asli setelah
                        // release — restore nilai yang tersimpan saat force.
                        if let Some(saved) = self.pre_force_values.remove(&id) {
                            if let Some(sig) = self.state.signals.get_mut(id) {
                                *sig = saved;
                            }
                        }
                    }
                }
                IrStmt::Deassign { lvalue } => {
                    if let Some(id) = self.signal_id_from_lvalue(lvalue) {
                        self.forced_signals.remove(&id);
                        if let Some(saved) = self.pre_force_values.remove(&id) {
                            if let Some(sig) = self.state.signals.get_mut(id) {
                                *sig = saved;
                            }
                        }
                    }
                }
                IrStmt::Wait { cond, body } => {
                    let cond_val = self.evaluate_expr(cond)?;
                    if cond_val.to_bool().unwrap_or(false) {
                        self.evaluate_block_with_delay_fork(body, fork_id)?;
                        if i + 1 < stmts.len() {
                            self.evaluate_block_with_delay_fork(&stmts[i + 1..], fork_id)?;
                        }
                    } else {
                        let deps = extract_signal_deps(cond);
                        if !deps.is_empty() {
                            let later: Vec<IrStmt> = stmts[i..].to_vec();
                            if !later.is_empty() {
                                self.pending_waits.push((deps, later));
                            }
                        }
                    }
                    return Ok(true);
                }
                IrStmt::WaitFork => {
                    // LANG-29: blokir sampai semua fork process milik proses ini
                    // selesai. Tanpa group aktif → lanjut segera.
                    let fids = self.active_fork_groups_for_current_process();
                    if fids.is_empty() {
                        if i + 1 < stmts.len() {
                            self.evaluate_block_with_delay_fork(&stmts[i + 1..], fork_id)?;
                        }
                        return Ok(true);
                    }
                    let later: Vec<IrStmt> = if i + 1 < stmts.len() {
                        stmts[i + 1..].to_vec()
                    } else {
                        Vec::new()
                    };
                    self.pending_wait_forks.push(WaitForkState {
                        fids,
                        continuation: later,
                        ast_continuation: Vec::new(),
                        this: None,
                        method: None,
                        locals: Vec::new(),
                        base_len: 0,
                        process_name: self.current_process_name.clone(),
                    });
                    return Ok(true);
                }
                IrStmt::WaitOrder {
                    events,
                    failure_stmts,
                } => {
                    let remaining = stmts[i + 1..].to_vec();
                    self.pending_wait_orders.push(WaitOrderState {
                        events: events.clone(),
                        expected_idx: 0,
                        continuation: remaining,
                        failure_stmts: failure_stmts.clone(),
                    });
                    return Ok(false);
                }
                IrStmt::RandCase { items } => {
                    let total: u64 = items
                        .iter()
                        .map(|(w_expr, _)| {
                            self.evaluate_expr(w_expr)
                                .unwrap_or(LogicVec::from_u64(1, 32))
                                .to_u64()
                        })
                        .sum();
                    if total > 0 {
                        let r = self.rng.gen::<u64>() % total;
                        let mut cumulative = 0u64;
                        for (w_expr, body) in items {
                            let weight = self
                                .evaluate_expr(w_expr)
                                .unwrap_or(LogicVec::from_u64(1, 32))
                                .to_u64();
                            cumulative += weight;
                            if r < cumulative {
                                let completed =
                                    self.evaluate_block_with_delay_fork(body, fork_id)?;
                                if !completed {
                                    return Ok(false);
                                }
                                break;
                            }
                        }
                    }
                }
                IrStmt::RandSequence { productions } => {
                    if let Some((_, items)) = productions.first() {
                        let total: u64 = items
                            .iter()
                            .map(|(w, _)| {
                                self.evaluate_expr(w)
                                    .unwrap_or(LogicVec::from_u64(1, 32))
                                    .to_u64()
                            })
                            .sum();
                        if total > 0 {
                            let r = self.rng.gen::<u64>() % total;
                            let mut acc = 0u64;
                            for (w, body) in items {
                                acc += self
                                    .evaluate_expr(w)
                                    .unwrap_or(LogicVec::from_u64(1, 32))
                                    .to_u64();
                                if r < acc {
                                    let completed =
                                        self.evaluate_block_with_delay_fork(body, fork_id)?;
                                    if !completed {
                                        return Ok(false);
                                    }
                                    break;
                                }
                            }
                        }
                    }
                }
                IrStmt::SysCall {
                    name,
                    args: ir_args,
                    line,
                    col,
                } => {
                    // F20: catat posisi syscall agar warning runtime punya lokasi.
                    self.set_cur_src_pos(*line, *col);
                    // Handle wrapped $value$plusargs / $test$plusargs from elaborator
                    if name.is_empty() {
                        if let Some(IrExpr::SysFunc {
                            name: fn_name,
                            args: fn_args,
                            ..
                        }) = ir_args.first()
                        {
                            if fn_name == "value$plusargs" {
                                if let Ok(pat_val) = self.evaluate_expr(
                                    fn_args.first().unwrap_or(&IrExpr::Const(LogicVec::new(0))),
                                ) {
                                    let pattern = logicvec_to_string(&pat_val);
                                    let plusarg_name = pattern
                                        .split('%')
                                        .next()
                                        .unwrap_or(&pattern)
                                        .trim_end_matches('=');
                                    let plusargs = self.plusargs.clone();
                                    for (key, val) in &plusargs {
                                        if key == plusarg_name {
                                            if let Some(var_arg) = fn_args.get(1) {
                                                let num = if let Some(hex) = val
                                                    .strip_prefix("0x")
                                                    .or_else(|| val.strip_prefix("0X"))
                                                {
                                                    u64::from_str_radix(hex, 16).unwrap_or(0)
                                                } else {
                                                    val.parse::<u64>().unwrap_or(0)
                                                };
                                                if let IrExpr::Signal(id, _) = var_arg {
                                                    self.state.write_signal(
                                                        *id,
                                                        LogicVec::from_u64(num, 32),
                                                    );
                                                }
                                            }
                                            break;
                                        }
                                    }
                                }
                                continue;
                            }
                            // System function lain (mis. $display/$write) yang
                            // dibungkus elaborator sbg SysCall{name:"", args:[...]}:
                            // dispatch dgn nama asli (tanpa '$') biar tak tercecer.
                            let dispatch_name = fn_name.as_str().trim_start_matches('$');
                            self.evaluate_syscall(dispatch_name, fn_args, fork_id, stmts, i)?;
                            continue;
                        }
                        continue;
                    }
                    self.evaluate_syscall(name.as_str(), ir_args, fork_id, stmts, i)?;
                }
                IrStmt::SysFinish => {
                    // Flush all pending await continuations before stopping
                    for pi in self.process_map.values_mut() {
                        if pi.status == ProcessStatus::Running
                            || pi.status == ProcessStatus::Waiting
                        {
                            pi.status = ProcessStatus::Finished;
                        }
                        pi.await_continuations.clear();
                    }
                    self.running = false;
                    return Ok(true);
                }
                IrStmt::Null => {}
                IrStmt::Assert {
                    cond,
                    pass_stmt,
                    fail_stmt,
                    clock_event,
                    disable_iff,
                    sequence,
                    line,
                    col,
                } => {
                    // F20: posisi assertion utk diagnostic file:line:col.
                    self.set_cur_src_pos(*line, *col);
                    let should_check = match clock_event {
                        Some(ref ce) => self.check_concurrent_clock_event(ce),
                        None => !self.assert_off_all,
                    };
                    if should_check {
                        let disabled = match disable_iff {
                            Some(ref di) => self.evaluate_expr(di)?.to_bool().unwrap_or(false),
                            None => false,
                        };
                        if !disabled && !self.assert_kill_all {
                            if let Some(seq) = &sequence {
                                // Concurrent assertion with temporal sequence: start a new attempt
                                self.sequence_attempts.push(SequenceAttempt {
                                    sequence: seq.clone(),
                                    cycles: 0,
                                    pass_stmt: pass_stmt.clone(),
                                    fail_stmt: fail_stmt.clone(),
                                    clock_event: clock_event.clone().unwrap(),
                                    // VERIF-27: posisi utk assertion stats.
                                    line: *line,
                                    col: *col,
                                    ante_matched: None,
                                });
                                // VERIF-32: sequence coverage — attempt dimulai.
                                self.record_sequence_attempt(*line, *col);
                            } else {
                                // Immediate assertion: evaluate condition now
                                let ok = self.evaluate_expr(cond)?.to_bool().unwrap_or(false);
                                // VERIF-27: assertion coverage metrics.
                                self.record_assertion(*line, *col, ok);
                                if ok {
                                    if !pass_stmt.is_empty() {
                                        self.evaluate_block_with_delay_fork(pass_stmt, fork_id)?;
                                    }
                                } else {
                                    // F20: via DiagSink agar punya file:line:col.
                                    let (a_l, a_c) = self.cur_src_pos();
                                    let _ = self.diag_error_at(
                                        mivon_core::diagnostics::DiagCode::AssertionFailed,
                                        "assertion failed",
                                        a_l,
                                        a_c,
                                    );
                                    if !fail_stmt.is_empty() {
                                        self.evaluate_block_with_delay_fork(fail_stmt, fork_id)?;
                                    }
                                }
                            }
                        }
                    }
                }
                // LANG-14: `expect (cond) else stmt` — assertion dalam
                // procedural code (IEEE 1800-2017 §17.16.2). Kondisi
                // dievaluasi SEKETIKA saat statement dijangkau (subset
                // immediate); false → fail_stmt + report "expect failed".
                // Tidak dipengaruhi $assertoff/$assertkill (berbeda dari
                // assert immediate) — expect selalu dievaluasi.
                IrStmt::Expect {
                    cond,
                    pass_stmt,
                    fail_stmt,
                    line,
                    col,
                } => {
                    self.set_cur_src_pos(*line, *col);
                    let ok = self.evaluate_expr(cond)?.to_bool().unwrap_or(false);
                    // VERIF-27: assertion coverage metrics.
                    self.record_assertion(*line, *col, ok);
                    if ok {
                        if !pass_stmt.is_empty() {
                            self.evaluate_block_with_delay_fork(pass_stmt, fork_id)?;
                        }
                    } else {
                        let (a_l, a_c) = self.cur_src_pos();
                        let _ = self.diag_error_at(
                            mivon_core::diagnostics::DiagCode::AssertionFailed,
                            "expect failed",
                            a_l,
                            a_c,
                        );
                        if !fail_stmt.is_empty() {
                            self.evaluate_block_with_delay_fork(fail_stmt, fork_id)?;
                        }
                    }
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
                    // F20: posisi assumption utk diagnostic file:line:col.
                    self.set_cur_src_pos(*line, *col);
                    let should_check = match clock_event {
                        Some(ref ce) => self.check_concurrent_clock_event(ce),
                        None => !self.assert_off_all,
                    };
                    if should_check {
                        let disabled = match disable_iff {
                            Some(ref di) => self.evaluate_expr(di)?.to_bool().unwrap_or(false),
                            None => false,
                        };
                        if !disabled && !self.assert_kill_all {
                            let ok = self.evaluate_expr(cond)?.to_bool().unwrap_or(false);
                            // VERIF-27: assumption violation = fail metric.
                            self.record_assertion(*line, *col, ok);
                            if ok {
                                if !pass_stmt.is_empty() {
                                    self.evaluate_block_with_delay_fork(pass_stmt, fork_id)?;
                                }
                            } else {
                                // F20: via DiagSink agar punya file:line:col.
                                let (a_l, a_c) = self.cur_src_pos();
                                self.diag_warn_at(
                                    mivon_core::diagnostics::DiagCode::AssertionFailed,
                                    "assumption violated",
                                    a_l,
                                    a_c,
                                );
                                if !fail_stmt.is_empty() {
                                    self.evaluate_block_with_delay_fork(fail_stmt, fork_id)?;
                                }
                            }
                        }
                    }
                }
                IrStmt::Cover {
                    cond,
                    pass_stmt,
                    clock_event,
                    disable_iff,
                    sequence: _,
                } => {
                    let should_check = match clock_event {
                        Some(ref ce) => self.check_concurrent_clock_event(ce),
                        None => !self.assert_off_all,
                    };
                    if should_check {
                        let disabled = match disable_iff {
                            Some(ref di) => self.evaluate_expr(di)?.to_bool().unwrap_or(false),
                            None => false,
                        };
                        if !disabled && !self.assert_kill_all {
                            let ok = self.evaluate_expr(cond)?.to_bool().unwrap_or(false);
                            if ok {
                                // LANG-13: catat hit cover property ke
                                // cover_hits (key line:col) — coverage summary.
                                let (cl, cc) = self.cur_src_pos();
                                let key = format!("cover@{}:{}", cl, cc);
                                let sym = Symbol::intern(&key);
                                *self.cover_hits.entry(sym).or_insert(0) += 1;
                                eprintln!("cover point hit");
                                if !pass_stmt.is_empty() {
                                    self.evaluate_block_with_delay_fork(pass_stmt, fork_id)?;
                                }
                            }
                        }
                    }
                }
                IrStmt::Break => {
                    self.evaluate_break_fork()?;
                    return Ok(true);
                }
                IrStmt::Continue => {
                    self.evaluate_continue_fork()?;
                    return Ok(true);
                }
                IrStmt::LoopFor {
                    init,
                    cond,
                    step,
                    body,
                } => {
                    // F31: set post_loop_tail seperti LoopWhile agar statement
                    // setelah loop tetap dieksekusi setelah resume (body/step
                    // ber-delay). LoopFor di-evaluasi via continuation yang
                    // menyertakan tail — restore di sini agar tidak dobel.
                    let saved_tail = std::mem::take(&mut self.post_loop_tail);
                    self.post_loop_tail = stmts[i + 1..].to_vec();
                    let completed = self.evaluate_loop_for_fork(init, cond, step, body, fork_id)?;
                    if !completed {
                        self.post_loop_tail = saved_tail;
                        return Ok(false);
                    }
                    self.post_loop_tail = saved_tail;
                }
                IrStmt::LoopWhile { cond, body } => {
                    let saved_tail = std::mem::take(&mut self.post_loop_tail);
                    self.post_loop_tail = stmts[i + 1..].to_vec();
                    let completed = self.evaluate_loop_while_fork(cond, body, fork_id)?;
                    if !completed {
                        self.post_loop_tail = saved_tail;
                        return Ok(false);
                    }
                    self.post_loop_tail = saved_tail;
                }
                IrStmt::LoopDoWhile { cond, body } => {
                    let saved_tail = std::mem::take(&mut self.post_loop_tail);
                    self.post_loop_tail = stmts[i + 1..].to_vec();
                    let completed = self.evaluate_loop_do_while_fork(cond, body, fork_id)?;
                    if !completed {
                        self.post_loop_tail = saved_tail;
                        return Ok(false);
                    }
                    self.post_loop_tail = saved_tail;
                }
                IrStmt::Repeat { count, body } => {
                    // F31: set post_loop_tail seperti LoopWhile agar statement
                    // setelah repeat tetap dieksekusi setelah resume (body
                    // ber-delay/event). evaluate_repeat_fork kini menyertakan
                    // tail dalam loop_continuation.
                    let saved_tail = std::mem::take(&mut self.post_loop_tail);
                    self.post_loop_tail = stmts[i + 1..].to_vec();
                    let completed = self.evaluate_repeat_fork(count, body, fork_id)?;
                    if !completed {
                        self.post_loop_tail = saved_tail;
                        return Ok(false);
                    }
                    self.post_loop_tail = saved_tail;
                }
                IrStmt::Foreach {
                    array_var,
                    index_var,
                    body,
                } => {
                    if !self.evaluate_foreach_fork(array_var, index_var, body, fork_id)? {
                        return Ok(false);
                    }
                }
                IrStmt::MethodCallStmt {
                    obj,
                    method,
                    args,
                    with_clause,
                } => {
                    // LANG-33: `obj.<constraint_block>.constraint_mode(0/1)`
                    // sebagai statement — set mode constraint block. Di-
                    // intercept SEBELUM evaluasi obj (field block bukan data
                    // field, evaluasi MemberAccess akan error/no-op).
                    if method.as_str() == "constraint_mode" {
                        if let IrExpr::MemberAccess { obj: inner, field } = obj {
                            let obj_val = self.evaluate_expr(inner)?;
                            let obj_id = obj_val.to_u64() as ObjId;
                            if let Some(arg) = args.first() {
                                let mode = self.evaluate_expr(arg)?.to_u64() != 0;
                                // LANG-32: block STATIC — mode global per-class
                                // (berlaku semua instance, §18.5.10).
                                let class_sym = self
                                    .state
                                    .objects
                                    .get(obj_id)
                                    .map(|o| o.class_name)
                                    .unwrap_or(Symbol::EMPTY);
                                let is_static = self
                                    .design
                                    .classes
                                    .get(&class_sym)
                                    .map(|cd| {
                                        cd.constraints.iter().any(|(bn, st, _)| bn == field && *st)
                                    })
                                    .unwrap_or(false);
                                if is_static {
                                    self.static_constraint_modes
                                        .insert((class_sym, *field), mode);
                                } else {
                                    self.constraint_modes.insert((obj_id, *field), mode);
                                }
                            }
                            continue;
                        }
                    }
                    if let IrExpr::Signal(id, _) = obj {
                        let sig_info = self.design.top.signals.get(*id).cloned();
                        if let Some(ref sig) = sig_info {
                            if sig.is_dynamic || sig.is_queue || sig.is_associative {
                                let _ = self.evaluate_array_method(
                                    *id,
                                    sig,
                                    method.as_str(),
                                    args,
                                    with_clause.as_deref(),
                                )?;
                                continue;
                            }
                            // Auto-create object for class/covergroup variables
                            if let Some(ref cn) = sig.class_name {
                                let is_cg = self.design.covergroups.iter().any(|c| c.name == *cn);
                                if is_cg || self.design.classes.contains_key(cn) {
                                    let obj_val = self.state.read_signal(*id);
                                    let obj_id = obj_val.to_u64() as ObjId;
                                    if obj_id == 0
                                        && !self.state.objects.is_empty()
                                        && self.state.objects[0].class_name.is_empty()
                                    {
                                        let class_for_obj = if is_cg {
                                            format!("__covergroup_{}", cn)
                                        } else {
                                            cn.to_string()
                                        };
                                        let new_id =
                                            self.state.alloc_object(Symbol::intern(&class_for_obj));
                                        self.state.write_signal(
                                            *id,
                                            LogicVec::from_u64(new_id as u64, 64),
                                        );
                                        let arg_vals: Vec<LogicVec> = args
                                            .iter()
                                            .map(|a| self.evaluate_expr(a))
                                            .collect::<Result<_, _>>()?;
                                        self.execute_method(new_id, method.as_str(), &arg_vals)?;
                                        continue;
                                    }
                                }
                            }
                        }
                    }
                    // F36: method call pada instance interface / hier instance
                    // yang tak punya method tersimulasi (receiver HierRef tak
                    // resolve ke signal) → no-op.
                    if let IrExpr::HierRef(name) = obj {
                        if self.find_signal(name.as_str()).is_none() {
                            let _: Vec<LogicVec> = args
                                .iter()
                                .map(|a| self.evaluate_expr(a))
                                .collect::<Result<_, _>>()?;
                            continue;
                        }
                    }
                    let obj_val = self.evaluate_expr(obj)?;
                    let obj_id = obj_val.to_u64() as ObjId;
                    let arg_vals: Vec<LogicVec> = args
                        .iter()
                        .map(|a| self.evaluate_expr(a))
                        .collect::<Result<_, _>>()?;
                    self.execute_method(obj_id, method.as_str(), &arg_vals)?;
                }
                IrStmt::Fork {
                    processes,
                    join_type,
                } => {
                    let remaining: Vec<IrStmt> = stmts[i + 1..].to_vec();
                    let count = processes.len();
                    // JoinNone tidak perlu menyimpan continuation (dieksekusi
                    // sekali di sini) → hindari clone sisa statement.
                    let cont = if matches!(join_type, IrJoinType::JoinNone) {
                        Vec::new()
                    } else {
                        remaining.clone()
                    };
                    let reclaimable = !matches!(join_type, IrJoinType::JoinAny);
                    let fid = self.alloc_fork_group(count, cont, reclaimable);
                    match join_type {
                        IrJoinType::Join => {
                            for p in processes {
                                if p.is_empty() {
                                    self.fork_decrement(fid)?;
                                } else {
                                    self.fork_branch_begin(fid);
                                    let all_consumed =
                                        self.evaluate_block_with_delay_fork(p, Some(fid))?;
                                    self.fork_branch_end(fid, all_consumed)?;
                                }
                            }
                            self.fork_finish(fid)?;
                        }
                        IrJoinType::JoinAny => {
                            self.fork_groups[fid].remaining = 1;
                            let mut any_immediate = false;
                            for p in processes {
                                if p.is_empty() {
                                    any_immediate = true;
                                } else {
                                    self.fork_branch_begin(fid);
                                    let all_consumed =
                                        self.evaluate_block_with_delay_fork(p, Some(fid))?;
                                    if all_consumed && !self.task_suspended {
                                        any_immediate = true;
                                    }
                                    self.fork_branch_end(fid, all_consumed)?;
                                }
                            }
                            if any_immediate {
                                self.fork_decrement(fid)?;
                            }
                            self.fork_finish(fid)?;
                        }
                        IrJoinType::JoinNone => {
                            for p in processes {
                                if p.is_empty() {
                                    self.fork_decrement(fid)?;
                                } else {
                                    self.fork_branch_begin(fid);
                                    let all_consumed =
                                        self.evaluate_block_with_delay_fork(p, Some(fid))?;
                                    self.fork_branch_end(fid, all_consumed)?;
                                }
                            }
                            // continuation join_none dieksekusi sekali sekarang
                            self.fork_groups[fid].fired = true;
                            if !remaining.is_empty() {
                                self.evaluate_block_with_delay_fork(&remaining, None)?;
                            }
                            self.fork_finish(fid)?;
                        }
                    }
                    return Ok(true);
                }
            }
            // Post-statement check: if process::await() was called on a running process,
            // capture remaining statements as await continuation and yield
            if let Some(target_id) = self.pending_await_target.take() {
                let remaining: Vec<IrStmt> = stmts[i + 1..].to_vec();
                let mut cont = remaining;
                if let Some(lc) = &self.loop_continuation {
                    cont.extend(lc.clone());
                }
                if let Some(pi) = self.process_map.get_mut(&target_id) {
                    pi.await_continuations.push(cont);
                }
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(crate) fn evaluate_ast_block_with_delay_fork(
        &mut self,
        stmts: &[mivon_ast::Stmt],
        fork_id: Option<usize>,
    ) -> Result<bool, SimError> {
        for (i, stmt) in stmts.iter().enumerate() {
            // $fatal menghentikan blok seketika (final block tetap jalan
            // setelah $finish/$fatal — hanya flag fatal_hit yang abort).
            if self.fatal_hit {
                return Ok(true);
            }
            if self.disable_pending.is_some() {
                return Ok(true);
            }
            if self.control_flow.is_some() {
                return Ok(true);
            }
            // F35: `return` AST menandai stop-blok lintas nested — iterasi
            // berikutnya (termasuk blok luar setelah if/case) berhenti.
            if self.ast_return_pending {
                return Ok(true);
            }
            match stmt {
                mivon_ast::Stmt::Block { stmts: inner } => {
                    if !self.evaluate_ast_block_with_delay_fork(inner, fork_id)? {
                        return Ok(false);
                    }
                }
                mivon_ast::Stmt::NamedBlock {
                    name,
                    stmts: inner,
                    decls: _,
                } => {
                    if self.disable_pending == Some(*name) {
                        self.disable_pending = None;
                        return Ok(true);
                    }
                    // F47: target disable_cross baru sampai sekarang (label
                    // belum masuk stack saat disable dieksekusi).
                    if self.consume_disable_cross_for(*name) {
                        return Ok(true);
                    }
                    let old = self.disable_pending.take();
                    self.known_named_labels.insert(*name);
                    self.active_named_labels.push(*name);
                    let completed = self.evaluate_ast_block_with_delay_fork(inner, fork_id);
                    self.active_named_labels.pop();
                    let completed = completed?;
                    if let Some(ref n) = self.disable_pending {
                        if *n == *name {
                            self.disable_pending = None;
                        }
                    }
                    self.disable_pending = self.disable_pending.take().or(old);
                    if !completed {
                        return Ok(false);
                    }
                }
                mivon_ast::Stmt::BlockingAssign { lhs, rhs, delay: _ } => {
                    // F17: `x = new(...)` — class di-resolve dari tipe LHS
                    // (helper bersama dgn evaluate_ast_stmt di eval/ast.rs).
                    let val = self.eval_ast_assign_rhs(rhs, lhs)?;
                    self.write_ast_lvalue(lhs, val)?;
                }
                mivon_ast::Stmt::NonBlockingAssign { lhs, rhs, delay: _ } => {
                    // F17: `x <= new(...)` — class dari tipe LHS.
                    let val = self.eval_ast_assign_rhs(rhs, lhs)?;
                    // Convert AST lvalue to IrLValue for nba tracking
                    if let Some(ir_lv) = self.ast_lvalue_to_ir(lhs) {
                        self.push_nba_pending(ir_lv, val);
                    } else {
                        self.write_ast_lvalue(lhs, val)?;
                    }
                }
                mivon_ast::Stmt::IfElse {
                    cond,
                    true_branch,
                    false_branch,
                } => {
                    let cond_val = self.evaluate_ast_expr(cond)?;
                    if cond_val.to_bool().unwrap_or(false) {
                        if !self
                            .evaluate_ast_block_with_delay_fork(&[*true_branch.clone()], fork_id)?
                        {
                            return Ok(false);
                        }
                    } else if let Some(fb) = false_branch {
                        if !self.evaluate_ast_block_with_delay_fork(&[*fb.clone()], fork_id)? {
                            return Ok(false);
                        }
                    }
                }
                mivon_ast::Stmt::Case {
                    expr,
                    items,
                    default,
                } => {
                    let case_val = self.evaluate_ast_expr(expr)?;
                    let mut matched = false;
                    for item in items {
                        let mut item_matched = false;
                        for pat in &item.labels {
                            let pat_val = self.evaluate_ast_expr(pat)?;
                            // LRM: case biasa membandingkan dgn zero-extension
                            // ke lebar terbesar (bukan PartialEq width-sensitive).
                            if case_val.case_val_eq(&pat_val) {
                                if !self.evaluate_ast_block_with_delay_fork(
                                    &[*item.stmt.clone()],
                                    fork_id,
                                )? {
                                    return Ok(false);
                                }
                                if self.disable_pending.is_some() {
                                    return Ok(true);
                                }
                                item_matched = true;
                                matched = true;
                                break;
                            }
                        }
                        if item_matched {
                            break;
                        }
                    }
                    if !matched {
                        if let Some(def) = default {
                            if !self.evaluate_ast_block_with_delay_fork(&[*def.clone()], fork_id)? {
                                return Ok(false);
                            }
                        }
                    }
                }
                mivon_ast::Stmt::CaseX {
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
                                if !self.evaluate_ast_block_with_delay_fork(
                                    &[*item.stmt.clone()],
                                    fork_id,
                                )? {
                                    return Ok(false);
                                }
                                matched = true;
                                break;
                            }
                        }
                        if matched {
                            break;
                        }
                    }
                    if !matched {
                        if let Some(def) = default {
                            if !self.evaluate_ast_block_with_delay_fork(&[*def.clone()], fork_id)? {
                                return Ok(false);
                            }
                        }
                    }
                }
                mivon_ast::Stmt::CaseZ {
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
                                if !self.evaluate_ast_block_with_delay_fork(
                                    &[*item.stmt.clone()],
                                    fork_id,
                                )? {
                                    return Ok(false);
                                }
                                matched = true;
                                break;
                            }
                        }
                        if matched {
                            break;
                        }
                    }
                    if !matched {
                        if let Some(def) = default {
                            if !self.evaluate_ast_block_with_delay_fork(&[*def.clone()], fork_id)? {
                                return Ok(false);
                            }
                        }
                    }
                }
                mivon_ast::Stmt::LoopForever { stmts: inner } => loop {
                    if self.disable_pending.is_some() {
                        break;
                    }
                    if self.control_flow.is_some() {
                        self.control_flow = None;
                        break;
                    }
                    self.ast_loop_iters += 1;
                    if self.ast_loop_iters > AST_LOOP_ITER_CAP {
                        self.emit_warning(
                            mivon_core::diagnostics::diagnostic::DiagCode::NotImplemented,
                            "AST while/forever loop exceeded iteration cap (blocking event in loop without time advance); breaking out to avoid hang",
                        );
                        break;
                    }
                    // F18: ast_loop_continuation agar blok yang suspend (delay /
                    // get_next_item blocking) di dalam forever loop tetap
                    // MENGULANG saat di-resume — sama seperti jalur IR.
                    let old_loop_cont = self.ast_loop_continuation.take();
                    self.ast_loop_continuation = Some(vec![mivon_ast::Stmt::LoopForever {
                        stmts: inner.clone(),
                    }]);
                    let completed = self.evaluate_ast_block_with_delay_fork(inner, fork_id)?;
                    self.ast_loop_continuation = old_loop_cont;
                    if !completed {
                        // F26: body suspend (delay/block) — beri tahu caller
                        // (fork arm / method body) bahwa blok BELUM selesai.
                        // Sebelumnya `break` internal → Ok(true) → fork
                        // decrement premature (join selesai saat task masih
                        // menunggu). Resume via ast_loop_continuation mengulang
                        // loop utuh.
                        return Ok(false);
                    }
                    let cf = self.control_flow.take();
                    if cf == Some(FlowControl::Break) {
                        break;
                    }
                    if cf == Some(FlowControl::Continue) {
                        continue;
                    }
                },
                mivon_ast::Stmt::LoopWhile { cond, stmts: inner } => loop {
                    if self.disable_pending.is_some() {
                        break;
                    }
                    if self.control_flow.is_some() {
                        self.control_flow = None;
                        break;
                    }
                    self.ast_loop_iters += 1;
                    if self.ast_loop_iters > AST_LOOP_ITER_CAP {
                        self.emit_warning(
                            mivon_core::diagnostics::diagnostic::DiagCode::NotImplemented,
                            "AST while loop exceeded iteration cap (blocking event in loop without time advance); breaking out to avoid hang",
                        );
                        break;
                    }
                    let cond_val = self.evaluate_ast_expr(cond)?;
                    if !cond_val.to_bool().unwrap_or(false) {
                        break;
                    }
                    // F18: ast_loop_continuation agar blok yang suspend (delay /
                    // get_next_item blocking) di dalam while loop tetap
                    // MENGULANG saat di-resume — sama seperti jalur IR.
                    let old_loop_cont = self.ast_loop_continuation.take();
                    self.ast_loop_continuation = Some(vec![mivon_ast::Stmt::LoopWhile {
                        cond: cond.clone(),
                        stmts: inner.clone(),
                    }]);
                    let completed = self.evaluate_ast_block_with_delay_fork(inner, fork_id)?;
                    self.ast_loop_continuation = old_loop_cont;
                    if !completed {
                        // F26: body suspend — blok BELUM selesai (lihat komentar
                        // LoopForever). Resume via ast_loop_continuation.
                        return Ok(false);
                    }
                    let cf = self.control_flow.take();
                    if cf == Some(FlowControl::Break) {
                        break;
                    }
                    if cf == Some(FlowControl::Continue) {
                        continue;
                    }
                },
                mivon_ast::Stmt::DoWhile { cond, stmts: inner } => loop {
                    if self.disable_pending.is_some() {
                        break;
                    }
                    if self.control_flow.is_some() {
                        self.control_flow = None;
                        break;
                    }
                    self.ast_loop_iters += 1;
                    if self.ast_loop_iters > AST_LOOP_ITER_CAP {
                        self.emit_warning(
                            mivon_core::diagnostics::diagnostic::DiagCode::NotImplemented,
                            "AST do-while loop exceeded iteration cap (blocking event in loop without time advance); breaking out to avoid hang",
                        );
                        break;
                    }
                    // F19: do-while dengan delay/block — resume lanjut iterasi
                    // berikutnya (continuation mengulang do-while, cond dicek
                    // ulang setelah body — perilaku do-while yang benar).
                    let old_loop_cont = self.ast_loop_continuation.take();
                    self.ast_loop_continuation = Some(vec![mivon_ast::Stmt::DoWhile {
                        cond: cond.clone(),
                        stmts: inner.clone(),
                    }]);
                    let completed = self.evaluate_ast_block_with_delay_fork(inner, fork_id)?;
                    self.ast_loop_continuation = old_loop_cont;
                    if !completed {
                        // F26: body suspend — blok BELUM selesai (lihat komentar
                        // LoopForever). Resume via ast_loop_continuation.
                        return Ok(false);
                    }
                    let cf = self.control_flow.take();
                    if cf == Some(FlowControl::Continue) {
                        continue;
                    }
                    if cf == Some(FlowControl::Break) {
                        break;
                    }
                    let cond_val = self.evaluate_ast_expr(cond)?;
                    if !cond_val.to_bool().unwrap_or(false) {
                        break;
                    }
                },
                mivon_ast::Stmt::LoopFor {
                    init,
                    cond,
                    step,
                    stmts: inner,
                } => {
                    if let Some(init_stmt) = init {
                        if !self
                            .evaluate_ast_block_with_delay_fork(&[*init_stmt.clone()], fork_id)?
                        {
                            // F26: init suspend — blok BELUM selesai.
                            return Ok(false);
                        }
                    }
                    loop {
                        if self.disable_pending.is_some() {
                            break;
                        }
                        if self.control_flow.is_some() {
                            self.control_flow = None;
                            break;
                        }
                        self.ast_loop_iters += 1;
                        if self.ast_loop_iters > AST_LOOP_ITER_CAP {
                            self.emit_warning(
                                mivon_core::diagnostics::diagnostic::DiagCode::NotImplemented,
                                "AST for loop exceeded iteration cap (blocking event in loop without time advance); breaking out to avoid hang",
                            );
                            break;
                        }
                        if let Some(ref c) = cond {
                            let cv = self.evaluate_ast_expr(c)?;
                            if !cv.to_bool().unwrap_or(false) {
                                break;
                            }
                        }
                        // F19: for loop dengan delay/block — continuation
                        // mempertahankan cond/step; init di-skip (variabel loop
                        // sudah di-set) sehingga resume lanjut iterasi berikutnya.
                        let old_loop_cont = self.ast_loop_continuation.take();
                        self.ast_loop_continuation = Some(vec![mivon_ast::Stmt::LoopFor {
                            init: None,
                            cond: cond.clone(),
                            step: step.clone(),
                            stmts: inner.clone(),
                        }]);
                        let completed = self.evaluate_ast_block_with_delay_fork(inner, fork_id)?;
                        self.ast_loop_continuation = old_loop_cont;
                        if !completed {
                            // F26: body suspend — blok BELUM selesai (lihat
                            // komentar LoopForever).
                            return Ok(false);
                        }
                        let cf = self.control_flow.take();
                        if cf == Some(FlowControl::Continue) {
                            if let Some(s) = step {
                                if !self
                                    .evaluate_ast_block_with_delay_fork(&[*s.clone()], fork_id)?
                                {
                                    return Ok(false);
                                }
                            }
                            continue;
                        }
                        if cf == Some(FlowControl::Break) {
                            break;
                        }
                        if self.disable_pending.is_some() {
                            break;
                        }
                        if let Some(s) = step {
                            if !self.evaluate_ast_block_with_delay_fork(&[*s.clone()], fork_id)? {
                                break;
                            }
                        }
                    }
                }
                mivon_ast::Stmt::Repeat {
                    count,
                    stmts: inner,
                } => {
                    let count_val = self.evaluate_ast_expr(count)?;
                    let n = count_val.to_u64() as usize;
                    // F19: repeat dengan delay/block di dalam body — suspensi
                    // harus MELANJUTKAN iterasi berikutnya (bukan mengulang
                    // dari awal). Continuation menyimpan sisa iterasi sebagai
                    // count literal (`repeat (sisa) begin ... end`).
                    let mut remaining_iters = n;
                    loop {
                        if remaining_iters == 0 {
                            break;
                        }
                        if self.disable_pending.is_some() {
                            break;
                        }
                        if self.control_flow.is_some() {
                            self.control_flow = None;
                            break;
                        }
                        self.ast_loop_iters += 1;
                        if self.ast_loop_iters > AST_LOOP_ITER_CAP {
                            self.emit_warning(
                                mivon_core::diagnostics::diagnostic::DiagCode::NotImplemented,
                                "AST repeat loop exceeded iteration cap (blocking event in loop without time advance); breaking out to avoid hang",
                            );
                            break;
                        }
                        remaining_iters -= 1;
                        let old_loop_cont = self.ast_loop_continuation.take();
                        self.ast_loop_continuation = Some(vec![mivon_ast::Stmt::Repeat {
                            count: mivon_ast::Expr::Value(mivon_ast::Value::Decimal(
                                remaining_iters as i64,
                            )),
                            stmts: inner.clone(),
                        }]);
                        let completed = self.evaluate_ast_block_with_delay_fork(inner, fork_id)?;
                        self.ast_loop_continuation = old_loop_cont;
                        if !completed {
                            // F26: body suspend — blok BELUM selesai (lihat
                            // komentar LoopForever).
                            return Ok(false);
                        }
                        let cf = self.control_flow.take();
                        if cf == Some(FlowControl::Continue) {
                            continue;
                        }
                        if cf == Some(FlowControl::Break) {
                            break;
                        }
                    }
                }
                mivon_ast::Stmt::Delay { delay, stmt: body } => {
                    let delay_val = self.evaluate_ast_expr(delay)?;
                    let d = delay_val.to_u64() as usize;
                    let delay_t = self.state.time as usize + d;
                    self.ensure_events(delay_t);
                    let remaining: Vec<mivon_ast::Stmt> = {
                        let mut v = Vec::new();
                        v.push(*body.clone());
                        if i + 1 < stmts.len() {
                            v.extend(stmts[i + 1..].iter().cloned());
                        }
                        // F18: delay di dalam loop AST (forever/while) harus
                        // mengulang saat resume — sama seperti jalur IR
                        // (IrStmt::Delay append loop_continuation) dan
                        // get_next_item blocking.
                        if let Some(lc) = &self.ast_loop_continuation {
                            v.extend(lc.clone());
                        }
                        v
                    };
                    let region = if d == 0 {
                        EventRegion::Inactive
                    } else {
                        EventRegion::Active
                    };
                    self.push_event(
                        delay_t,
                        RegionEvent {
                            region,
                            event: EventKind::ContinueAstBlock(
                                remaining,
                                fork_id,
                                self.current_this,
                                self.current_method,
                            ),
                        },
                    );
                    return Ok(false);
                }
                mivon_ast::Stmt::EventControl { events, stmt: body } => {
                    // Blocking event control di task/method UVM: daftarkan pending
                    // wake-up (dengan konteks method) lalu suspend. Event yang sudah
                    // lewat tidak dihitung. Signal yang tak ter-resolve (mis. vif
                    // tanpa clk di desain) → lanjut segera (tanpa block).
                    if events
                        .iter()
                        .any(|e| matches!(e, mivon_ast::SensitivityEvent::Wildcard))
                    {
                        if let Some(b) = body {
                            self.evaluate_ast_block_with_delay_fork(&[*b.clone()], fork_id)?;
                        }
                        if i + 1 < stmts.len() {
                            self.evaluate_ast_block_with_delay_fork(&stmts[i + 1..], fork_id)?;
                        }
                        return Ok(true);
                    }
                    let mut sigs: Vec<(SignalId, Option<ClockEdge>)> = Vec::new();
                    // LANG-27: guard `iff (cond)` pada event control di task/method.
                    let mut iff_ast: Option<mivon_ast::Expr> = None;
                    for event in events {
                        let inner = match event {
                            mivon_ast::SensitivityEvent::Iff { event, cond } => {
                                iff_ast = Some(cond.clone());
                                event.as_ref()
                            }
                            other => other,
                        };
                        match inner {
                            mivon_ast::SensitivityEvent::PosEdge(expr) => {
                                if let Some(id) = self.find_ast_signal_id(expr) {
                                    sigs.push((id, Some(ClockEdge::PosEdge(id))));
                                }
                            }
                            mivon_ast::SensitivityEvent::NegEdge(expr) => {
                                if let Some(id) = self.find_ast_signal_id(expr) {
                                    sigs.push((id, Some(ClockEdge::NegEdge(id))));
                                }
                            }
                            mivon_ast::SensitivityEvent::Level(expr) => {
                                if let Some(id) = self.find_ast_signal_id(expr) {
                                    sigs.push((id, None));
                                }
                            }
                            mivon_ast::SensitivityEvent::Wildcard => unreachable!("handled above"),
                            mivon_ast::SensitivityEvent::Iff { .. } => {}
                        }
                    }
                    if sigs.is_empty() {
                        return Ok(true);
                    }
                    let mut later: Vec<mivon_ast::Stmt> = Vec::new();
                    if let Some(b) = body {
                        later.push(*b.clone());
                    }
                    later.extend(stmts[i + 1..].iter().cloned());
                    let base_len = self.method_locals.len();
                    self.pending_ast_events.push(PendingAstEventControl {
                        sigs,
                        continuation: later,
                        this: self.current_this,
                        method: self.current_method,
                        locals: self.method_locals.clone(),
                        base_len,
                        iff: iff_ast,
                    });
                    return Ok(false);
                }
                mivon_ast::Stmt::Wait { cond, stmt: body } => {
                    let cond_val = self.evaluate_ast_expr(cond)?;
                    if cond_val.to_bool().unwrap_or(false) {
                        if let Some(b) = body {
                            self.evaluate_ast_block_with_delay_fork(&[*b.clone()], fork_id)?;
                        }
                        if i + 1 < stmts.len() {
                            self.evaluate_ast_block_with_delay_fork(&stmts[i + 1..], fork_id)?;
                        }
                    } else {
                        // Condition not met yet — skip
                        return Ok(true);
                    }
                }
                mivon_ast::Stmt::WaitFork => {
                    // LANG-29 (jalur AST task/method): sama dengan jalur IR —
                    // tunggu group fork milik proses ini, resume via
                    // check_wait_forks dengan konteks method yang disimpan.
                    let fids = self.active_fork_groups_for_current_process();
                    if fids.is_empty() {
                        if i + 1 < stmts.len() {
                            self.evaluate_ast_block_with_delay_fork(&stmts[i + 1..], fork_id)?;
                        }
                        return Ok(true);
                    }
                    let mut later: Vec<mivon_ast::Stmt> = if i + 1 < stmts.len() {
                        stmts[i + 1..].to_vec()
                    } else {
                        Vec::new()
                    };
                    if let Some(lc) = &self.ast_loop_continuation {
                        later.extend(lc.clone());
                    }
                    let base_len = self.method_locals.len();
                    self.pending_wait_forks.push(WaitForkState {
                        fids,
                        continuation: Vec::new(),
                        ast_continuation: later,
                        this: self.current_this,
                        method: self.current_method,
                        locals: self.method_locals.clone(),
                        base_len,
                        process_name: self.current_process_name.clone(),
                    });
                    return Ok(true);
                }
                mivon_ast::Stmt::SysCall {
                    name,
                    args,
                    line,
                    col,
                } => {
                    // F20: catat posisi syscall agar warning runtime punya lokasi.
                    self.set_cur_src_pos(*line, *col);
                    // For task context, delegate to SysCall handler
                    self.handle_ast_syscall(name.as_str(), args)?;
                }
                mivon_ast::Stmt::SysFinish => {
                    self.running = false;
                    return Ok(true);
                }
                mivon_ast::Stmt::Expr { expr } => {
                    // F18/F24: `get_next_item` blocking — driver UVM memakai pola
                    // `forever begin get_next_item(it); ...; item_done(); end`.
                    // Semantics UVM: blokir sampai item tersedia (grant).
                    // F24 upgrade: waiter-based (bukan polling 1ns) keyed by
                    // sequencer (label "get_next_item"), release oleh start_item.
                    // Saat resume, statement get_next_item dieksekusi ULANG →
                    // proceed path: pop + grant + tulis lvalue (sebelumnya
                    // lvalue `it` TIDAK pernah ditulis — item tak sampai driver).
                    if let mivon_ast::Expr::FuncCall {
                        name,
                        args,
                        line: _,
                        col: _,
                    } = expr
                    {
                        let nm = name.as_str();
                        if nm == "get_next_item" || nm == "try_next_item" {
                            if let Some(obj_id) = self.current_this {
                                let seqr = self.uvm_seqr_for(obj_id);
                                if seqr != 0 {
                                    if self.uvm_queue_empty(obj_id) {
                                        // F24 review: `try_next_item` NON-blocking
                                        // (semantik UVM) — kosong → tulis null (0),
                                        // tanpa suspend. Hanya get_next_item yang block.
                                        if nm == "try_next_item" {
                                            if let Some(lhs) = args.first() {
                                                self.write_ast_lvalue(
                                                    lhs,
                                                    LogicVec::from_u64(0, 64),
                                                )?;
                                            }
                                            continue;
                                        }
                                        let mut wait_stmts: Vec<mivon_ast::Stmt> =
                                            vec![mivon_ast::Stmt::Expr { expr: expr.clone() }];
                                        wait_stmts.extend(stmts[i + 1..].to_vec());
                                        if let Some(lc) = &self.ast_loop_continuation {
                                            wait_stmts.extend(lc.clone());
                                        }
                                        self.uvm_seq_try_wait(
                                            seqr,
                                            "get_next_item".to_string(),
                                            wait_stmts,
                                            fork_id,
                                            self.current_this,
                                            self.current_method,
                                        )?;
                                        return Ok(false);
                                    }
                                    // Proceed: pop item + grant + tulis lvalue.
                                    if let Some(item) = self.uvm_seq_pop(seqr) {
                                        if let Some(lhs) = args.first() {
                                            self.write_ast_lvalue(
                                                lhs,
                                                LogicVec::from_u64(item as u64, 64),
                                            )?;
                                        }
                                    }
                                    continue;
                                }
                            }
                        }
                        // F24: `finish_item(it)` FuncCall di body sequence
                        // (pola `start_item(it); finish_item(it);` — tanpa obj).
                        // Block sampai driver item_done utk item tsb (waiter
                        // label "finish_item:{item}"). Saat resume, statement
                        // dieksekusi ulang → done → dikonsumsi.
                        if nm == "finish_item" {
                            if let Some(obj_id) = self.current_this {
                                let cls = self
                                    .state
                                    .get_object(obj_id)
                                    .map(|o| o.class_name)
                                    .unwrap_or_default();
                                if self.is_uvm_sequence_hierarchy(cls.as_str()) {
                                    let seqr = self
                                        .state
                                        .get_object(obj_id)
                                        .and_then(|o| o.fields.get(&Symbol::intern("__sequencer")))
                                        .map(|v| v.to_u64() as ObjId)
                                        .unwrap_or(0);
                                    let item_id = args
                                        .first()
                                        .map(|a| self.evaluate_ast_expr(a))
                                        .transpose()?
                                        .map(|v| v.to_u64() as ObjId)
                                        .unwrap_or(0);
                                    if seqr != 0 && self.uvm_seq_finish_blocks(seqr, item_id) {
                                        let mut wait_stmts: Vec<mivon_ast::Stmt> =
                                            vec![mivon_ast::Stmt::Expr { expr: expr.clone() }];
                                        wait_stmts.extend(stmts[i + 1..].to_vec());
                                        if let Some(lc) = &self.ast_loop_continuation {
                                            wait_stmts.extend(lc.clone());
                                        }
                                        let label = format!("finish_item:{}", item_id);
                                        self.uvm_seq_try_wait(
                                            seqr,
                                            label,
                                            wait_stmts,
                                            fork_id,
                                            self.current_this,
                                            self.current_method,
                                        )?;
                                        return Ok(false);
                                    }
                                    // Item selesai → statement dikonsumsi.
                                    continue;
                                }
                            }
                        }
                        // VERIF-06: `uvm_config_db::wait_modified(inst, field)`
                        // BLOCKING — menunggu `set` berikutnya utk key tsb.
                        // Waiter keyed by (inst, field) di
                        // `uvm_config_db_waiters`; release oleh set (ast.rs/
                        // expr.rs → config_db_release_waiters →
                        // ContinueAstBlock t+1). Saat resume, statement
                        // wait_modified DIULANG → kali ini key sudah ada →
                        // kondisi terpenuhi → dikonsumsi.
                        if nm == "uvm_config_db::wait_modified" {
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
                            // Sudah ada nilai → kondisi terpenuhi seketika.
                            if self.config_db_exists(&inst_name, &field_name) {
                                continue;
                            }
                            let mut wait_stmts: Vec<mivon_ast::Stmt> =
                                vec![mivon_ast::Stmt::Expr { expr: expr.clone() }];
                            wait_stmts.extend(stmts[i + 1..].to_vec());
                            if let Some(lc) = &self.ast_loop_continuation {
                                wait_stmts.extend(lc.clone());
                            }
                            let key = (inst_name, field_name);
                            self.uvm_config_db_waiters.entry(key).or_default().push(
                                crate::simulator::types::UvmSyncWaiter {
                                    continuation: wait_stmts,
                                    fork_id,
                                    this: self.current_this,
                                    method: self.current_method,
                                    wait_label: "wait_modified".to_string(),
                                },
                            );
                            return Ok(false);
                        }
                    }
                    // F21/F23: blocking wait uvm_event/uvm_barrier (`wait_*`)
                    // + blocking put/get/peek uvm_tlm_fifo. Eval obj SEKALI
                    // (sebelumnya arm fifo dan arm wait_* mengevaluasi obj
                    // dua kali utk MethodCall yang cocok). MethodCall lain
                    // (write, start, dll) tidak tersentuh — jalur normal.
                    if let mivon_ast::Expr::MethodCall {
                        obj,
                        method,
                        args,
                        with_clause: _,
                    } = expr
                    {
                        let m = method.as_str();
                        let is_wait = m == "wait_trigger"
                            || m == "wait_on"
                            || m == "wait_for"
                            || m == "wait_for_count";
                        let is_fifo_op = m == "put" || m == "get" || m == "peek";
                        let is_seq_get = m == "get_next_item" || m == "try_next_item";
                        let is_finish = m == "finish_item";
                        if is_wait || is_fifo_op || is_seq_get || is_finish {
                            let obj_val = self.evaluate_ast_expr(obj)?;
                            let eid = obj_val.to_u64() as ObjId;
                            let is_fifo = self
                                .state
                                .get_object(eid)
                                .map(|o| self.is_uvm_tlm_fifo_hierarchy(o.class_name.as_str()))
                                .unwrap_or(false);
                            // ── F23: uvm_tlm_fifo ──
                            // `fifo.get(item);` / `fifo.put(item);` — hanya utk
                            // objek hierarchy fifo (class biasa dgn method
                            // get/put tetap via execute_method). `get`/`peek`
                            // menulis item ke lvalue arg setelah pop.
                            if is_fifo && is_fifo_op {
                                let mut remaining: Vec<mivon_ast::Stmt> = stmts[i + 1..].to_vec();
                                if let Some(lc) = &self.ast_loop_continuation {
                                    remaining.extend(lc.clone());
                                }
                                // Statement get/put ini PREPEND ke continuation
                                // — saat resume dieksekusi ULANG (proceed path:
                                // pop + tulis lvalue utk get; push utk put).
                                // Tanpa ini get yang suspend tak pernah
                                // pop/tulis lvalue (item diambil get berikutnya).
                                let mut wait_stmts: Vec<mivon_ast::Stmt> =
                                    vec![mivon_ast::Stmt::Expr { expr: expr.clone() }];
                                wait_stmts.extend(remaining);
                                let blocked = self.uvm_try_fifo_wait(
                                    eid,
                                    m,
                                    wait_stmts,
                                    fork_id,
                                    self.current_this,
                                    self.current_method,
                                )?;
                                if blocked {
                                    return Ok(false);
                                }
                                match m {
                                    "get" | "peek" => {
                                        let item_id = if m == "get" {
                                            let got = self
                                                .uvm_tlm_fifo_data
                                                .get_mut(&eid)
                                                .and_then(|fd| fd.queue.pop_front());
                                            self.uvm_fifo_release_waiters(eid, false)?;
                                            got
                                        } else {
                                            self.uvm_tlm_fifo_data
                                                .get(&eid)
                                                .and_then(|fd| fd.queue.front().copied())
                                        };
                                        if let Some(it) = item_id {
                                            if let Some(lhs) = args.first() {
                                                self.write_ast_lvalue(
                                                    lhs,
                                                    LogicVec::from_u64(it as u64, 64),
                                                )?;
                                            }
                                        }
                                    }
                                    "put" => {
                                        if let Some(item_arg) = args.first() {
                                            let item = self.evaluate_ast_expr(item_arg)?;
                                            let item_id = item.to_u64() as ObjId;
                                            if let Some(fd) = self.uvm_tlm_fifo_data.get_mut(&eid) {
                                                if fd.queue.len() < fd.capacity {
                                                    fd.queue.push_back(item_id);
                                                }
                                            }
                                            self.uvm_fifo_release_waiters(eid, true)?;
                                        }
                                    }
                                    _ => {}
                                }
                                // Statement wait dikonsumsi (tidak diulang).
                                continue;
                            }
                            // ── LANG-24: mailbox bounded ──
                            // `m.get(x)` / `m.put(x)` pada objek __mailbox.
                            // Bounded mode: put block bila penuh, get block bila
                            // kosong (waiter label "put"/"get", release oleh
                            // get/put yang membebaskan). proceed path: pop/push
                            // + tulis lvalue arg utk get.
                            let is_mailbox = self
                                .state
                                .get_object(eid)
                                .map(|o| o.class_name == "__mailbox")
                                .unwrap_or(false);
                            if is_mailbox && is_fifo_op {
                                let mut remaining: Vec<mivon_ast::Stmt> = stmts[i + 1..].to_vec();
                                if let Some(lc) = &self.ast_loop_continuation {
                                    remaining.extend(lc.clone());
                                }
                                let mut wait_stmts: Vec<mivon_ast::Stmt> =
                                    vec![mivon_ast::Stmt::Expr { expr: expr.clone() }];
                                wait_stmts.extend(remaining);
                                let blocked = self.uvm_try_mailbox_wait(
                                    eid,
                                    m,
                                    wait_stmts,
                                    fork_id,
                                    self.current_this,
                                    self.current_method,
                                )?;
                                if blocked {
                                    return Ok(false);
                                }
                                match m {
                                    "get" => {
                                        let got = self
                                            .mailbox_queues
                                            .get_mut(&eid)
                                            .and_then(|q| q.pop_front());
                                        self.uvm_mailbox_release_waiters(eid, true)?;
                                        if let (Some(val), Some(lhs)) = (got, args.first()) {
                                            self.write_ast_lvalue(lhs, val)?;
                                        }
                                    }
                                    "put" => {
                                        if let Some(item_arg) = args.first() {
                                            let item = self.evaluate_ast_expr(item_arg)?;
                                            self.mailbox_queues
                                                .entry(eid)
                                                .or_default()
                                                .push_back(item);
                                            self.uvm_mailbox_release_waiters(eid, false)?;
                                        }
                                    }
                                    _ => {}
                                }
                                continue;
                            }
                            // ── F24: sequence/sequencer/driver handshake ──
                            // `drv.get_next_item(req)` / `port.get_next_item(req)`
                            // pada obj driver/sequencer/seq_item_port — block
                            // bila queue sequencer kosong (waiter label
                            // "get_next_item", release oleh start_item); proceed:
                            // pop + grant + tulis item ke lvalue arg. Dan
                            // `seq.finish_item(it)` pada obj sequence — block
                            // sampai driver item_done (waiter label
                            // "finish_item:{item}", release oleh item_done).
                            let eid_class = self
                                .state
                                .get_object(eid)
                                .map(|o| o.class_name)
                                .unwrap_or_default();
                            if is_seq_get && self.uvm_seqr_for(eid) != 0 {
                                let seqr = self.uvm_seqr_for(eid);
                                let queue_empty = self
                                    .uvm_sequencer_data
                                    .get(&seqr)
                                    .map(|sd| sd.item_queue.is_empty())
                                    .unwrap_or(true);
                                if queue_empty {
                                    // F24 review: `try_next_item` NON-blocking
                                    // (semantik UVM) — kosong → tulis null (0),
                                    // tanpa suspend. Hanya get_next_item yang block.
                                    if m == "try_next_item" {
                                        if let Some(lhs) = args.first() {
                                            self.write_ast_lvalue(lhs, LogicVec::from_u64(0, 64))?;
                                        }
                                        continue;
                                    }
                                    let mut wait_stmts: Vec<mivon_ast::Stmt> =
                                        vec![mivon_ast::Stmt::Expr { expr: expr.clone() }];
                                    wait_stmts.extend(stmts[i + 1..].to_vec());
                                    if let Some(lc) = &self.ast_loop_continuation {
                                        wait_stmts.extend(lc.clone());
                                    }
                                    self.uvm_seq_try_wait(
                                        seqr,
                                        "get_next_item".to_string(),
                                        wait_stmts,
                                        fork_id,
                                        self.current_this,
                                        self.current_method,
                                    )?;
                                    return Ok(false);
                                }
                                if let Some(item) = self.uvm_seq_pop(seqr) {
                                    if let Some(lhs) = args.first() {
                                        self.write_ast_lvalue(
                                            lhs,
                                            LogicVec::from_u64(item as u64, 64),
                                        )?;
                                    }
                                }
                                continue;
                            }
                            if is_finish && self.is_uvm_sequence_hierarchy(eid_class.as_str()) {
                                let seqr = self
                                    .state
                                    .get_object(eid)
                                    .and_then(|o| o.fields.get(&Symbol::intern("__sequencer")))
                                    .map(|v| v.to_u64() as ObjId)
                                    .unwrap_or(0);
                                let item_id = args
                                    .first()
                                    .map(|a| self.evaluate_ast_expr(a))
                                    .transpose()?
                                    .map(|v| v.to_u64() as ObjId)
                                    .unwrap_or(0);
                                if seqr != 0 && self.uvm_seq_finish_blocks(seqr, item_id) {
                                    let mut wait_stmts: Vec<mivon_ast::Stmt> =
                                        vec![mivon_ast::Stmt::Expr { expr: expr.clone() }];
                                    wait_stmts.extend(stmts[i + 1..].to_vec());
                                    if let Some(lc) = &self.ast_loop_continuation {
                                        wait_stmts.extend(lc.clone());
                                    }
                                    let label = format!("finish_item:{}", item_id);
                                    self.uvm_seq_try_wait(
                                        seqr,
                                        label,
                                        wait_stmts,
                                        fork_id,
                                        self.current_this,
                                        self.current_method,
                                    )?;
                                    return Ok(false);
                                }
                                // Item sudah done → statement dikonsumsi.
                                continue;
                            }
                            // ── F21: uvm_event/uvm_barrier ──
                            // Semantics UVM: blokir sampai event di-trigger /
                            // barrier penuh. uvm_try_wait register waiter +
                            // suspend (side effect count wait_for SEKALI —
                            // statement wait TIDAK diulang). Resume via
                            // uvm_release_waiters → ContinueAstBlock.
                            if is_wait {
                                let arg_vals: Vec<LogicVec> = args
                                    .iter()
                                    .map(|a| self.evaluate_ast_expr(a))
                                    .collect::<Result<_, _>>()?;
                                let mut remaining: Vec<mivon_ast::Stmt> = stmts[i + 1..].to_vec();
                                if let Some(lc) = &self.ast_loop_continuation {
                                    remaining.extend(lc.clone());
                                }
                                let blocked = self.uvm_try_wait(
                                    eid,
                                    m,
                                    &arg_vals,
                                    remaining,
                                    fork_id,
                                    self.current_this,
                                    self.current_method,
                                )?;
                                if blocked {
                                    return Ok(false);
                                }
                                // Kondisi sudah terpenuhi — statement wait
                                // dikonsumsi, lanjut statement berikut.
                                continue;
                            }
                        }
                    }
                    self.evaluate_ast_expr(expr)?;
                }
                mivon_ast::Stmt::Break => {
                    self.control_flow = Some(FlowControl::Break);
                    return Ok(true);
                }
                mivon_ast::Stmt::Continue => {
                    self.control_flow = Some(FlowControl::Continue);
                    return Ok(true);
                }
                mivon_ast::Stmt::Return(Some(expr)) => {
                    // F35: `return expr` menulis nilai ke slot current_method
                    // (`__func_ret` utk module function, nama method utk task)
                    // lalu menandai ast_return_pending agar SELURUH blok
                    // berhenti (bukan cuma blok if — bug: statement setelah
                    // return tetap jalan → rekursi tak berujung). Sebelumnya
                    // no-op: ANSI `return n` di body function tak pernah
                    // menulis `__func_ret` → helper baca slot 0 → return 0.
                    let val = self.evaluate_ast_expr(expr)?;
                    if let Some(ref method) = self.current_method {
                        self.set_local(method.as_str(), val);
                    }
                    self.ast_return_pending = true;
                    return Ok(true);
                }
                mivon_ast::Stmt::Return(None) => {
                    self.ast_return_pending = true;
                    return Ok(true);
                }
                mivon_ast::Stmt::Null => {}
                mivon_ast::Stmt::Force { lhs, rhs } => {
                    let val = self.evaluate_ast_expr(rhs)?;
                    self.write_ast_lvalue(lhs, val)?;
                }
                mivon_ast::Stmt::Release { expr: _ } => {
                    // Release variable — just a no-op in AST context
                }
                mivon_ast::Stmt::EventTrigger { name } => {
                    // Find signal by name and toggle it
                    if let Some(id) = self.find_signal(name.as_str()) {
                        let val = self.state.read_signal(id);
                        let toggled = if val.to_bool().unwrap_or(false) {
                            LogicVec::from_u64(0, val.width.max(1))
                        } else {
                            LogicVec::from_u64(1, val.width.max(1))
                        };
                        self.state.write_signal(id, toggled);
                    }
                }
                mivon_ast::Stmt::Disable { name } => {
                    // LANG-30: `disable fork` (jalur AST task/method) —
                    // terminate child processes milik proses ini, lanjut.
                    if name.as_str() == "fork" {
                        let fids = self.active_fork_groups_for_current_process();
                        for fid in fids {
                            if let Some(g) = self.fork_groups.get_mut(fid) {
                                g.disabled = true;
                            }
                        }
                    } else if self.mark_disable_label(*name) {
                        return Ok(true);
                    }
                }
                mivon_ast::Stmt::Fork {
                    processes,
                    join_type,
                } => {
                    let mut remaining: Vec<mivon_ast::Stmt> = if i + 1 < stmts.len() {
                        stmts[i + 1..].to_vec()
                    } else {
                        Vec::new()
                    };
                    // F21 review: fork di dalam loop AST (forever/while) harus
                    // mengulang saat join selesai — pola sama dengan Delay dan
                    // wait_* blocking (append ast_loop_continuation).
                    if let Some(lc) = &self.ast_loop_continuation {
                        remaining.extend(lc.clone());
                    }
                    let count = processes.len();
                    // F21: fork AST (task/method UVM) kini memakai ForkGroup yang
                    // benar — continuation AST disimpan di `ast_fork_cont`
                    // (ForkGroup.continuation hanya Vec<IrStmt>) dan dieksekusi
                    // di fork_finish saat SEMUA branch selesai. Branch yang
                    // suspend (delay/wait_*) di-resume via
                    // ContinueAstBlock(fork_id) → event.rs fork_decrement.
                    // Sebelumnya remaining dieksekusi LANGSUNG (join selalu
                    // dilewati — salah bila branch menunggu trigger/barrier).
                    let reclaimable = !matches!(join_type, mivon_ast::JoinType::JoinAny);
                    let fid = self.alloc_fork_group(count, Vec::new(), reclaimable);
                    match join_type {
                        mivon_ast::JoinType::Join => {
                            self.ast_fork_cont.insert(fid, remaining);
                            for p in processes {
                                self.fork_branch_begin(fid);
                                let all_consumed = self.evaluate_ast_block_with_delay_fork(
                                    std::slice::from_ref(p),
                                    Some(fid),
                                )?;
                                self.fork_branch_end(fid, all_consumed)?;
                            }
                        }
                        mivon_ast::JoinType::JoinAny => {
                            self.fork_groups[fid].remaining = 1;
                            self.ast_fork_cont.insert(fid, remaining);
                            let mut any_immediate = false;
                            for p in processes {
                                self.fork_branch_begin(fid);
                                let all_consumed = self.evaluate_ast_block_with_delay_fork(
                                    std::slice::from_ref(p),
                                    Some(fid),
                                )?;
                                if all_consumed && !self.task_suspended {
                                    any_immediate = true;
                                }
                                self.fork_branch_end(fid, all_consumed)?;
                            }
                            if any_immediate {
                                self.fork_decrement(fid)?;
                            }
                        }
                        mivon_ast::JoinType::JoinNone => {
                            for p in processes {
                                self.fork_branch_begin(fid);
                                let all_consumed = self.evaluate_ast_block_with_delay_fork(
                                    std::slice::from_ref(p),
                                    Some(fid),
                                )?;
                                self.fork_branch_end(fid, all_consumed)?;
                            }
                            self.fork_groups[fid].fired = true;
                            if !remaining.is_empty() {
                                self.evaluate_ast_block_with_delay_fork(&remaining, None)?;
                            }
                            self.fork_finish(fid)?;
                        }
                    }
                    return Ok(true);
                }
                mivon_ast::Stmt::Assert {
                    cond,
                    pass_stmt,
                    fail_stmt,
                    ..
                } => {
                    let ok = self.evaluate_ast_expr(cond)?.to_bool().unwrap_or(false);
                    if ok {
                        if let Some(ps) = pass_stmt {
                            self.evaluate_ast_block_with_delay_fork(&[*ps.clone()], fork_id)?;
                        }
                    } else {
                        // F20: via DiagSink agar punya file:line:col.
                        let (a_l, a_c) = self.cur_src_pos();
                        let _ = self.diag_error_at(
                            mivon_core::diagnostics::DiagCode::AssertionFailed,
                            "assertion failed",
                            a_l,
                            a_c,
                        );
                        if let Some(fs) = fail_stmt {
                            self.evaluate_ast_block_with_delay_fork(&[*fs.clone()], fork_id)?;
                        }
                    }
                }
                mivon_ast::Stmt::Assume {
                    cond,
                    pass_stmt,
                    fail_stmt,
                    ..
                } => {
                    let ok = self.evaluate_ast_expr(cond)?.to_bool().unwrap_or(false);
                    if ok {
                        if let Some(ps) = pass_stmt {
                            self.evaluate_ast_block_with_delay_fork(&[*ps.clone()], fork_id)?;
                        }
                    } else {
                        // F20: via DiagSink agar punya file:line:col.
                        let (a_l, a_c) = self.cur_src_pos();
                        self.diag_warn_at(
                            mivon_core::diagnostics::DiagCode::AssertionFailed,
                            "assumption violated",
                            a_l,
                            a_c,
                        );
                        if let Some(fs) = fail_stmt {
                            self.evaluate_ast_block_with_delay_fork(&[*fs.clone()], fork_id)?;
                        }
                    }
                }
                mivon_ast::Stmt::Cover {
                    cond, pass_stmt, ..
                } => {
                    let ok = self.evaluate_ast_expr(cond)?.to_bool().unwrap_or(false);
                    if ok {
                        if let Some(ps) = pass_stmt {
                            self.evaluate_ast_block_with_delay_fork(&[*ps.clone()], fork_id)?;
                        }
                    }
                }
                _ => {
                    // Unhandled statement types in task method context
                }
            }
        }
        Ok(true)
    }

    pub(crate) fn evaluate_stmt_block(&mut self, stmts: &[IrStmt]) -> Result<(), SimError> {
        for (i, stmt) in stmts.iter().enumerate() {
            if self.fatal_hit {
                return Ok(());
            }
            if self.disable_pending.is_some() {
                return Ok(());
            }
            if self.control_flow.is_some() {
                return Ok(());
            }
            // Line coverage: record every statement execution
            // Clone pname first to avoid borrow conflict with self.record_line_hit
            if self.coverage_enabled {
                if let Some(pname) = self.current_process_name.clone() {
                    self.record_line_hit(stmt, &pname);
                }
            }
            match stmt {
                IrStmt::BlockingAssign { lhs, rhs, delay } => {
                    if !self.is_forced(lhs) {
                        // Intra-assignment delay di konteks zero-time (fungsi/
                        // task body non-suspend) tidak bisa di-suspend — apply
                        // segera (konstruk ilegal di fungsi; fallback aman).
                        let _ = delay;
                        let val = self.eval_assign_rhs(rhs, lhs)?;
                        self.write_lvalue(lhs, val, true)?;
                    }
                }
                IrStmt::NonBlockingAssign { lhs, rhs, delay } => {
                    if !self.is_forced(lhs) {
                        let _ = delay;
                        let val = self.eval_assign_rhs(rhs, lhs)?;
                        self.push_nba_pending(lhs.clone(), val);
                    }
                }
                IrStmt::Force { lvalue, rhs } => {
                    // LRM §10.6.2: wire → restore; reg → keep forced.
                    if let Some(id) = self.signal_id_from_lvalue(lvalue) {
                        if !self.forced_signals.contains(&id) {
                            let is_wire = self
                                .design
                                .top
                                .signals
                                .get(id)
                                .map(|s| {
                                    matches!(
                                        s.kind,
                                        mivon_ir::SignalKind::Wire | mivon_ir::SignalKind::Logic
                                    )
                                })
                                .unwrap_or(false);
                            if is_wire {
                                if let Some(sig) = self.state.signals.get(id) {
                                    self.pre_force_values.insert(id, sig.clone());
                                }
                            }
                        }
                    }
                    let val = self.eval_assign_rhs(rhs, lvalue)?;
                    self.write_lvalue(lvalue, val, true)?;
                    if let Some(id) = self.signal_id_from_lvalue(lvalue) {
                        self.forced_signals.insert(id);
                    }
                }
                IrStmt::If {
                    cond,
                    true_branch: then_stmts,
                    false_branch: else_stmts,
                } => {
                    self.evaluate_if_stmt(cond, then_stmts, else_stmts)?;
                }
                IrStmt::Block { stmts: inner } => {
                    self.evaluate_block_stmt(inner)?;
                }
                IrStmt::NamedBlock {
                    name, stmts: inner, ..
                } => {
                    self.evaluate_named_block_stmt(*name, inner)?;
                }
                IrStmt::SysCall {
                    name,
                    args: ir_args,
                    line,
                    col,
                } => {
                    // F20: catat posisi syscall agar warning runtime punya lokasi.
                    self.set_cur_src_pos(*line, *col);
                    if name.is_empty() {
                        if let Some(IrExpr::SysFunc {
                            name: fn_name,
                            args: fn_args,
                            ..
                        }) = ir_args.first()
                        {
                            self.evaluate_syscall_stmt(
                                fn_name.as_str().trim_start_matches('$'),
                                fn_args,
                            )?;
                        }
                        continue;
                    }
                    self.evaluate_syscall_stmt(name.as_str(), ir_args)?;
                }
                IrStmt::SysFinish => {
                    self.running = false;
                    return Ok(());
                }
                IrStmt::Case {
                    case_type,
                    expr: case_expr,
                    items,
                    default,
                } => {
                    self.evaluate_case_stmt(case_type, case_expr, items, default)?;
                }
                IrStmt::Null => {}
                IrStmt::Assert {
                    cond,
                    pass_stmt,
                    fail_stmt,
                    clock_event,
                    disable_iff,
                    sequence,
                    line,
                    col,
                } => {
                    // F20: posisi assertion utk diagnostic file:line:col.
                    self.set_cur_src_pos(*line, *col);
                    let should_check = match clock_event {
                        Some(ref ce) => self.check_concurrent_clock_event(ce),
                        None => true,
                    };
                    if should_check {
                        let disabled = match disable_iff {
                            Some(ref di) => self.evaluate_expr(di)?.to_bool().unwrap_or(false),
                            None => false,
                        };
                        if !disabled && !self.assert_kill_all {
                            if let Some(seq) = &sequence {
                                // Concurrent assertion with temporal sequence:
                                // start a new attempt (same as evaluate_block_with_delay_fork).
                                self.sequence_attempts.push(SequenceAttempt {
                                    sequence: seq.clone(),
                                    cycles: 0,
                                    pass_stmt: pass_stmt.clone(),
                                    fail_stmt: fail_stmt.clone(),
                                    clock_event: clock_event.clone().unwrap(),
                                    line: *line,
                                    col: *col,
                                    ante_matched: None,
                                });
                                // VERIF-32: sequence coverage — attempt dimulai.
                                self.record_sequence_attempt(*line, *col);
                            } else {
                                let ok = self.evaluate_expr(cond)?.to_bool().unwrap_or(false);
                                // VERIF-27: assertion coverage metrics.
                                self.record_assertion(*line, *col, ok);
                                if ok {
                                    if !pass_stmt.is_empty() {
                                        self.evaluate_stmt_block(pass_stmt)?;
                                    }
                                } else {
                                    // F20: via DiagSink agar punya file:line:col.
                                    let (a_l, a_c) = self.cur_src_pos();
                                    let _ = self.diag_error_at(
                                        mivon_core::diagnostics::DiagCode::AssertionFailed,
                                        "assertion failed",
                                        a_l,
                                        a_c,
                                    );
                                    if !fail_stmt.is_empty() {
                                        self.evaluate_stmt_block(fail_stmt)?;
                                    }
                                }
                            }
                        }
                    }
                }
                // LANG-14: `expect (cond) else stmt` — jalur evaluate_stmt_block
                // (sama dengan Assert immediate, tanpa assert-off/disable).
                IrStmt::Expect {
                    cond,
                    pass_stmt,
                    fail_stmt,
                    line,
                    col,
                } => {
                    self.set_cur_src_pos(*line, *col);
                    let ok = self.evaluate_expr(cond)?.to_bool().unwrap_or(false);
                    // VERIF-27: assertion coverage metrics.
                    self.record_assertion(*line, *col, ok);
                    if ok {
                        if !pass_stmt.is_empty() {
                            self.evaluate_stmt_block(pass_stmt)?;
                        }
                    } else {
                        let (a_l, a_c) = self.cur_src_pos();
                        let _ = self.diag_error_at(
                            mivon_core::diagnostics::DiagCode::AssertionFailed,
                            "expect failed",
                            a_l,
                            a_c,
                        );
                        if !fail_stmt.is_empty() {
                            self.evaluate_stmt_block(fail_stmt)?;
                        }
                    }
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
                    // F20: posisi assumption utk diagnostic file:line:col.
                    self.set_cur_src_pos(*line, *col);
                    let should_check = match clock_event {
                        Some(ref ce) => self.check_concurrent_clock_event(ce),
                        None => true,
                    };
                    if should_check {
                        let disabled = match disable_iff {
                            Some(ref di) => self.evaluate_expr(di)?.to_bool().unwrap_or(false),
                            None => false,
                        };
                        if !disabled {
                            let ok = self.evaluate_expr(cond)?.to_bool().unwrap_or(false);
                            // VERIF-27: assumption violation = fail metric.
                            self.record_assertion(*line, *col, ok);
                            if ok {
                                if !pass_stmt.is_empty() {
                                    self.evaluate_stmt_block(pass_stmt)?;
                                }
                            } else {
                                // F20: via DiagSink agar punya file:line:col.
                                let (a_l, a_c) = self.cur_src_pos();
                                self.diag_warn_at(
                                    mivon_core::diagnostics::DiagCode::AssertionFailed,
                                    "assumption violated",
                                    a_l,
                                    a_c,
                                );
                                if !fail_stmt.is_empty() {
                                    self.evaluate_stmt_block(fail_stmt)?;
                                }
                            }
                        }
                    }
                }
                IrStmt::Cover {
                    cond,
                    pass_stmt,
                    clock_event,
                    disable_iff,
                    sequence: _,
                } => {
                    let should_check = match clock_event {
                        Some(ref ce) => self.check_concurrent_clock_event(ce),
                        None => true,
                    };
                    if should_check {
                        let disabled = match disable_iff {
                            Some(ref di) => self.evaluate_expr(di)?.to_bool().unwrap_or(false),
                            None => false,
                        };
                        if !disabled {
                            let ok = self.evaluate_expr(cond)?.to_bool().unwrap_or(false);
                            if ok {
                                // LANG-13: catat hit cover property ke
                                // cover_hits (key line:col).
                                let (cl, cc) = self.cur_src_pos();
                                let key = format!("cover@{}:{}", cl, cc);
                                let sym = Symbol::intern(&key);
                                *self.cover_hits.entry(sym).or_insert(0) += 1;
                                eprintln!("cover point hit");
                                if !pass_stmt.is_empty() {
                                    self.evaluate_stmt_block(pass_stmt)?;
                                }
                            }
                        }
                    }
                }
                IrStmt::Break => {
                    self.evaluate_break_stmt()?;
                    return Ok(());
                }
                IrStmt::Continue => {
                    self.evaluate_continue_stmt()?;
                    return Ok(());
                }
                IrStmt::LoopFor {
                    init,
                    cond,
                    step,
                    body,
                } => {
                    self.evaluate_loop_for_stmt(init, cond, step, body)?;
                }
                IrStmt::LoopWhile { cond, body } => {
                    self.evaluate_loop_while_stmt(cond, body)?;
                }
                IrStmt::LoopDoWhile { cond, body } => {
                    self.evaluate_loop_do_while_stmt(cond, body)?;
                }
                IrStmt::Repeat { count, body } => {
                    self.evaluate_repeat_stmt(count, body)?;
                }
                IrStmt::RandCase { items } => {
                    let total: u64 = items
                        .iter()
                        .map(|(w_expr, _)| {
                            self.evaluate_expr(w_expr)
                                .unwrap_or(LogicVec::from_u64(1, 32))
                                .to_u64()
                        })
                        .sum();
                    if total > 0 {
                        let r = self.rng.gen::<u64>() % total;
                        let mut cumulative = 0u64;
                        for (w_expr, body) in items {
                            let weight = self
                                .evaluate_expr(w_expr)
                                .unwrap_or(LogicVec::from_u64(1, 32))
                                .to_u64();
                            cumulative += weight;
                            if r < cumulative {
                                self.evaluate_stmt_block(body)?;
                                break;
                            }
                        }
                    }
                }
                IrStmt::RandSequence { productions } => {
                    if let Some((_, items)) = productions.first() {
                        let total: u64 = items
                            .iter()
                            .map(|(w, _)| {
                                self.evaluate_expr(w)
                                    .unwrap_or(LogicVec::from_u64(1, 32))
                                    .to_u64()
                            })
                            .sum();
                        if total > 0 {
                            let r = self.rng.gen::<u64>() % total;
                            let mut acc = 0u64;
                            for (w, body) in items {
                                acc += self
                                    .evaluate_expr(w)
                                    .unwrap_or(LogicVec::from_u64(1, 32))
                                    .to_u64();
                                if r < acc {
                                    self.evaluate_stmt_block(body)?;
                                    break;
                                }
                            }
                        }
                    }
                }
                IrStmt::Foreach {
                    array_var,
                    index_var,
                    body,
                } => {
                    self.evaluate_foreach_stmt(array_var, index_var, body)?;
                }
                IrStmt::MethodCallStmt {
                    obj,
                    method,
                    args,
                    with_clause,
                } => {
                    // LANG-33: `obj.<constraint_block>.constraint_mode(0/1)`
                    // sebagai statement — set mode constraint block. Di-
                    // intercept SEBELUM evaluasi obj (field block bukan data
                    // field, evaluasi MemberAccess akan error/no-op).
                    if method.as_str() == "constraint_mode" {
                        if let IrExpr::MemberAccess { obj: inner, field } = obj {
                            let obj_val = self.evaluate_expr(inner)?;
                            let obj_id = obj_val.to_u64() as ObjId;
                            if let Some(arg) = args.first() {
                                let mode = self.evaluate_expr(arg)?.to_u64() != 0;
                                // LANG-32: block STATIC — mode global per-class
                                // (berlaku semua instance, §18.5.10).
                                let class_sym = self
                                    .state
                                    .objects
                                    .get(obj_id)
                                    .map(|o| o.class_name)
                                    .unwrap_or(Symbol::EMPTY);
                                let is_static = self
                                    .design
                                    .classes
                                    .get(&class_sym)
                                    .map(|cd| {
                                        cd.constraints.iter().any(|(bn, st, _)| bn == field && *st)
                                    })
                                    .unwrap_or(false);
                                if is_static {
                                    self.static_constraint_modes
                                        .insert((class_sym, *field), mode);
                                } else {
                                    self.constraint_modes.insert((obj_id, *field), mode);
                                }
                            }
                            continue;
                        }
                    }
                    if let IrExpr::Signal(id, _) = obj {
                        let sig_info = self.design.top.signals.get(*id).cloned();
                        if let Some(ref sig) = sig_info {
                            if sig.is_dynamic || sig.is_queue || sig.is_associative {
                                let _ = self.evaluate_array_method(
                                    *id,
                                    sig,
                                    method.as_str(),
                                    args,
                                    with_clause.as_deref(),
                                )?;
                                continue;
                            }
                            if let Some(ref cn) = sig.class_name {
                                let is_cg = self.design.covergroups.iter().any(|c| c.name == *cn);
                                if is_cg || self.design.classes.contains_key(cn) {
                                    let obj_val = self.state.read_signal(*id);
                                    let obj_id = obj_val.to_u64() as ObjId;
                                    if obj_id == 0
                                        && !self.state.objects.is_empty()
                                        && self.state.objects[0].class_name.is_empty()
                                    {
                                        let class_for_obj = if is_cg {
                                            format!("__covergroup_{}", cn)
                                        } else {
                                            cn.to_string()
                                        };
                                        let new_id =
                                            self.state.alloc_object(Symbol::intern(&class_for_obj));
                                        self.state.write_signal(
                                            *id,
                                            LogicVec::from_u64(new_id as u64, 64),
                                        );
                                        let arg_vals: Vec<LogicVec> = args
                                            .iter()
                                            .map(|a| self.evaluate_expr(a))
                                            .collect::<Result<_, _>>()?;
                                        self.execute_method(new_id, method.as_str(), &arg_vals)?;
                                        continue;
                                    }
                                }
                            }
                        }
                    }
                    // F36: method call pada instance interface / hier instance
                    // yang tak punya method tersimulasi (receiver HierRef tak
                    // resolve ke signal) → no-op.
                    if let IrExpr::HierRef(name) = obj {
                        if self.find_signal(name.as_str()).is_none() {
                            let _: Vec<LogicVec> = args
                                .iter()
                                .map(|a| self.evaluate_expr(a))
                                .collect::<Result<_, _>>()?;
                            continue;
                        }
                    }
                    let obj_val = self.evaluate_expr(obj)?;
                    let obj_id = obj_val.to_u64() as ObjId;
                    let arg_vals: Vec<LogicVec> = args
                        .iter()
                        .map(|a| self.evaluate_expr(a))
                        .collect::<Result<_, _>>()?;
                    self.execute_method(obj_id, method.as_str(), &arg_vals)?;
                }
                IrStmt::Delay { delay, body } => {
                    let delay_val = *delay as usize;
                    let delay_t = self.state.time as usize + delay_val;
                    self.ensure_events(delay_t);
                    let mut later: Vec<IrStmt> = body.clone();
                    let remaining: Vec<IrStmt> = stmts[i + 1..].to_vec();
                    later.extend(remaining);
                    if !later.is_empty() {
                        let region = if delay_val == 0 {
                            EventRegion::Inactive
                        } else {
                            EventRegion::Active
                        };
                        let pid = self.current_process_id;
                        self.push_event(
                            delay_t,
                            RegionEvent {
                                region,
                                event: EventKind::ContinueBlock(Continuation {
                                    stmts_to_exec: later,
                                    stmts_remaining: vec![],
                                    fork_id: None,
                                    process_id: pid,
                                    process_name: self.current_process_name.clone(),
                                    named_labels: self.active_named_labels.clone(),
                                }),
                            },
                        );
                    }
                    return Ok(());
                }
                IrStmt::EventControl { sigs, body, iff } => {
                    // Blocking event control (evaluate_stmt_block context):
                    // daftarkan pending wake-up dan berhenti memproses blok ini.
                    // SATU entry mewakili SATU `@(...)` — fire sekali.
                    let mut later: Vec<IrStmt> = body.clone();
                    later.extend(stmts[i + 1..].to_vec());
                    if let Some(lc) = &self.loop_continuation {
                        later.extend(lc.clone());
                    }
                    // F27: resolve ClockEdge::*Hier (event `@(posedge b.clk)`
                    // lewat port interface) ke SignalId via hier_signal_map.
                    let sigs = self.normalize_event_sigs(sigs);
                    self.pending_events.push(PendingEventControl {
                        sigs,
                        continuation: later,
                        // LANG-27: guard `iff (cond)`.
                        iff: iff.clone(),
                        // Konteks tanpa param fork_id eksplisit — pakai aktif.
                        fork_id: self.active_fork_id,
                    });
                    return Ok(());
                }
                IrStmt::EventTrigger { sig_id } => {
                    let val = self.state.read_signal(*sig_id);
                    let toggled = if val.to_bool().unwrap_or(false) {
                        LogicVec::from_u64(0, val.width.max(1))
                    } else {
                        LogicVec::from_u64(1, val.width.max(1))
                    };
                    self.state.write_signal(*sig_id, toggled);
                }
                IrStmt::Wait { cond, body } => {
                    let cond_val = self.evaluate_expr(cond)?;
                    if cond_val.to_bool().unwrap_or(false) {
                        self.evaluate_stmt_block(body)?;
                    }
                }
                IrStmt::WaitFork => {
                    // LANG-29 (konteks evaluate_stmt_block): tanpa group aktif
                    // milik proses ini → lanjut; ada → suspend via wait_forks.
                    let fids = self.active_fork_groups_for_current_process();
                    if fids.is_empty() {
                        if i + 1 < stmts.len() {
                            self.evaluate_stmt_block(&stmts[i + 1..])?;
                        }
                        return Ok(());
                    }
                    let later: Vec<IrStmt> = if i + 1 < stmts.len() {
                        stmts[i + 1..].to_vec()
                    } else {
                        Vec::new()
                    };
                    self.pending_wait_forks.push(WaitForkState {
                        fids,
                        continuation: later,
                        ast_continuation: Vec::new(),
                        this: None,
                        method: None,
                        locals: Vec::new(),
                        base_len: 0,
                        process_name: self.current_process_name.clone(),
                    });
                    return Ok(());
                }
                IrStmt::WaitOrder {
                    events,
                    failure_stmts,
                } => {
                    let continuation: Vec<IrStmt> = stmts[i + 1..].to_vec();
                    self.pending_wait_orders.push(WaitOrderState {
                        events: events.clone(),
                        expected_idx: 0,
                        continuation,
                        failure_stmts: failure_stmts.clone(),
                    });
                    return Ok(());
                }
                IrStmt::Disable { name } => {
                    // LANG-30: `disable fork` (jalur evaluate_stmt_block) —
                    // terminate child processes milik proses ini, lanjut.
                    if name.as_str() == "fork" {
                        let fids = self.active_fork_groups_for_current_process();
                        for fid in fids {
                            if let Some(g) = self.fork_groups.get_mut(fid) {
                                g.disabled = true;
                            }
                        }
                    } else if self.mark_disable_label(*name) {
                        return Ok(());
                    }
                }
                IrStmt::Release { lvalue } => {
                    if let Some(id) = self.signal_id_from_lvalue(lvalue) {
                        self.forced_signals.remove(&id);
                        // LRM §10.6.2: wire kembali ke driver asli setelah
                        // release — restore nilai yang tersimpan saat force.
                        if let Some(saved) = self.pre_force_values.remove(&id) {
                            if let Some(sig) = self.state.signals.get_mut(id) {
                                *sig = saved;
                            }
                        }
                    }
                }
                IrStmt::Deassign { lvalue } => {
                    if let Some(id) = self.signal_id_from_lvalue(lvalue) {
                        self.forced_signals.remove(&id);
                        if let Some(saved) = self.pre_force_values.remove(&id) {
                            if let Some(sig) = self.state.signals.get_mut(id) {
                                *sig = saved;
                            }
                        }
                    }
                }
                IrStmt::Fork {
                    processes,
                    join_type,
                } => {
                    let remaining: Vec<IrStmt> = stmts[i + 1..].to_vec();
                    let count = processes.len();
                    let cont = if matches!(join_type, IrJoinType::JoinNone) {
                        Vec::new()
                    } else {
                        remaining.clone()
                    };
                    let reclaimable = !matches!(join_type, IrJoinType::JoinAny);
                    let fid = self.alloc_fork_group(count, cont, reclaimable);
                    match join_type {
                        IrJoinType::Join => {
                            for p in processes {
                                if p.is_empty() {
                                    self.fork_decrement(fid)?;
                                } else {
                                    self.fork_branch_begin(fid);
                                    let all_consumed =
                                        self.evaluate_block_with_delay_fork(p, Some(fid))?;
                                    self.fork_branch_end(fid, all_consumed)?;
                                }
                            }
                            if self.fork_groups[fid].active
                                && self.fork_groups[fid].remaining == 0
                                && !remaining.is_empty()
                            {
                                self.fork_groups[fid].fired = true;
                                let cont = std::mem::take(&mut self.fork_groups[fid].continuation);
                                self.evaluate_stmt_block(&cont)?;
                                self.retire_fork_group(fid);
                            }
                        }
                        IrJoinType::JoinAny => {
                            self.fork_groups[fid].remaining = 1;
                            let mut any_immediate = false;
                            for p in processes {
                                if p.is_empty() {
                                    any_immediate = true;
                                } else {
                                    self.fork_branch_begin(fid);
                                    let all_consumed =
                                        self.evaluate_block_with_delay_fork(p, Some(fid))?;
                                    if all_consumed && !self.task_suspended {
                                        any_immediate = true;
                                    }
                                    self.fork_branch_end(fid, all_consumed)?;
                                }
                            }
                            if any_immediate {
                                self.fork_decrement(fid)?;
                            }
                            if self.fork_groups[fid].active
                                && self.fork_groups[fid].remaining == 0
                                && !remaining.is_empty()
                            {
                                self.fork_groups[fid].fired = true;
                                let cont = std::mem::take(&mut self.fork_groups[fid].continuation);
                                self.evaluate_stmt_block(&cont)?;
                            }
                            self.fork_groups[fid].continuation.clear();
                        }
                        IrJoinType::JoinNone => {
                            for p in processes {
                                if p.is_empty() {
                                    self.fork_decrement(fid)?;
                                } else {
                                    self.fork_branch_begin(fid);
                                    let all_consumed =
                                        self.evaluate_block_with_delay_fork(p, Some(fid))?;
                                    self.fork_branch_end(fid, all_consumed)?;
                                }
                            }
                            self.fork_groups[fid].fired = true;
                            if !remaining.is_empty() {
                                self.evaluate_stmt_block(&remaining)?;
                            }
                            self.fork_finish(fid)?;
                        }
                    }
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// F18: apakah queue item sequencer (untuk objek `this` saat ini) kosong?
    /// Dipakai `get_next_item` blocking: driver → sequencer (via sequencer_id),
    /// atau sequencer langsung. Selain itu dianggap tidak kosong (bukan UVM
    /// sequencer — biarkan dispatch normal yang menangani).
    fn uvm_queue_empty(&self, obj_id: ObjId) -> bool {
        // Driver: cari sequencer yang terhubung.
        if let Some(dd) = self.uvm_driver_data.get(&obj_id) {
            if let Some(seqr_id) = dd.sequencer_id {
                if let Some(sd) = self.uvm_sequencer_data.get(&seqr_id) {
                    return sd.item_queue.is_empty();
                }
            }
            // Driver tanpa sequencer terhubung → belum ada item → block.
            return true;
        }
        // Sequencer langsung (`seqr.get_next_item(...)`).
        if let Some(sd) = self.uvm_sequencer_data.get(&obj_id) {
            return sd.item_queue.is_empty();
        }
        false
    }
}
