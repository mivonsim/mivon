//! SIM-20 tahap 1 — Cycle-based simulation mode (`--cycle`).
//!
//! Mode eksekusi cycle-based ala simulator industri (bukan event-driven
//! penuh): clock didrive internal scheduler, tanpa iterasi delta IEEE 1800.
//! Per setengah periode clock:
//!   1. Drive nilai clk (0↔1) via `write_lvalue` — race detection, history,
//!      dan timing checks tetap jalan (jalur write yang sama dengan RTL).
//!   2. Resume semua continuation `@(posedge/negedge clk)` yang menunggu
//!      (`process_pending_events`) + evaluasi SEMUA proses combinational
//!      berulang sampai fixed-point (state tidak berubah lagi).
//!   3. Di edge: `commit_nba()` lalu settle ulang agar comb bereaksi ke
//!      output FF (semantik NBA read-after-edge benar).
//!
//! SUBSET tahap 1 (jujur):
//! - Desain harus punya ≥1 proses Sequential dan SEMUA FF ber-clock pada
//!   SATU sinyal yang sama (dari sensitivity proses atau EventControl body).
//! - Tidak boleh ada timed wait statis (`#N`, assign ber-delay) di body
//!   proses mana pun — terdeteksi analisis → fallback event-driven.
//! - Proses Initial diperbolehkan tapi hanya yang selesai/menunggu sinyal
//!   di t=0; bila terdeteksi event berjadwal waktu (timed) tersisa setelah
//!   init → error jelas menyuruh jalur event-driven (fallback pre-sim).
//!
//! Yang TIDAK ditangani tahap ini (dokumentasi, bukan klaim): multi-clock,
//! gate primitives dengan delay SDF, `$strobe`/postponed penuh, fork dengan
//! delay (sudah ketahan oleh scan timed), distributed/parallel path.

use super::SimulationEngine;
use crate::simulator::types::EventKind;
use mivon_core::error::SimError;
use mivon_ir::{IrStmt, Process, SignalId};

/// Iterasi settle maksimum per fase — guard kombinational loop.
const MAX_SETTLE_ITERS: usize = 128;

/// Hasil analisis kelayakan mode cycle-based.
pub(crate) struct CyclePlan {
    /// Sinyal clock tunggal yang didrive loop.
    pub clock: SignalId,
    /// Proses combinational / comb-reactive (fixed-point tiap fase).
    pub comb_pids: Vec<usize>,
}

/// Hasil walk body proses: timed wait + kandidat clock edge.
#[derive(Default)]
struct BodyScan {
    has_timed: bool,
    clocks: Vec<SignalId>,
}

fn scan_body(stmts: &[IrStmt], out: &mut BodyScan) {
    if out.has_timed {
        // Tetap lanjut scan clock? Cukup berhenti cepat — has_timed sudah
        // memutuskan fallback. (early-out)
    }
    for s in stmts {
        match s {
            IrStmt::Block { stmts } | IrStmt::NamedBlock { stmts, .. } => scan_body(stmts, out),
            IrStmt::If {
                true_branch,
                false_branch,
                ..
            } => {
                scan_body(true_branch, out);
                scan_body(false_branch, out);
            }
            IrStmt::Case { items, default, .. } => {
                for it in items {
                    scan_body(&it.body, out);
                }
                scan_body(default, out);
            }
            IrStmt::LoopFor {
                init, step, body, ..
            } => {
                if let Some(st) = init {
                    scan_body(std::slice::from_ref(st), out);
                }
                if let Some(st) = step {
                    scan_body(std::slice::from_ref(st), out);
                }
                scan_body(body, out);
            }
            IrStmt::LoopWhile { body, .. } | IrStmt::LoopDoWhile { body, .. } => {
                scan_body(body, out)
            }
            IrStmt::Repeat { body, .. } | IrStmt::Foreach { body, .. } => scan_body(body, out),
            IrStmt::Delay { .. } => out.has_timed = true,
            IrStmt::BlockingAssign { delay, .. } | IrStmt::NonBlockingAssign { delay, .. } => {
                if delay.is_some() {
                    out.has_timed = true;
                }
            }
            IrStmt::EventControl { sigs, body, .. } => {
                for (sid, edge) in sigs {
                    if edge.is_some() {
                        out.clocks.push(*sid);
                    }
                    // Level-sensitive @(sig) aman — resume via pending events.
                }
                scan_body(body, out);
            }
            IrStmt::Wait { body, .. } => scan_body(body, out),
            IrStmt::Fork { processes, .. } => {
                for p in processes {
                    scan_body(p, out);
                }
            }
            IrStmt::Assert {
                pass_stmt,
                fail_stmt,
                ..
            }
            | IrStmt::Assume {
                pass_stmt,
                fail_stmt,
                ..
            }
            | IrStmt::Expect {
                pass_stmt,
                fail_stmt,
                ..
            } => {
                scan_body(pass_stmt, out);
                scan_body(fail_stmt, out);
            }
            IrStmt::Cover { pass_stmt, .. } => scan_body(pass_stmt, out),
            IrStmt::WaitOrder { failure_stmts, .. } => scan_body(failure_stmts, out),
            IrStmt::RandCase { items } => {
                for (_, body) in items {
                    scan_body(body, out);
                }
            }
            IrStmt::RandSequence { productions } => {
                for (_, items) in productions {
                    for (_, body) in items {
                        scan_body(body, out);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Analisis statis kelayakan. Err(reason) → mode tidak cocok (fallback).
pub(crate) fn analyze_plan(engine: &SimulationEngine) -> Result<CyclePlan, String> {
    let procs = &engine.design.top.processes;
    let mut comb_pids = Vec::new();
    let mut ff_count = 0usize;
    let mut clock_opt: Option<SignalId> = None;

    let check_clock = |sid: SignalId, clock: &mut Option<SignalId>| -> Result<(), String> {
        match clock {
            None => *clock = Some(sid),
            Some(c) if *c == sid => {}
            Some(c) => {
                let n1 = engine
                    .design
                    .top
                    .signals
                    .get(*c)
                    .map(|s| s.name.as_str())
                    .unwrap_or("?");
                let n2 = engine
                    .design
                    .top
                    .signals
                    .get(sid)
                    .map(|s| s.name.as_str())
                    .unwrap_or("?");
                return Err(format!(
                    "multi-clock design ('{}' vs '{}') belum didukung mode cycle",
                    n1, n2
                ));
            }
        }
        Ok(())
    };

    for (pid, p) in procs.iter().enumerate() {
        match p {
            Process::Combinational {
                name,
                sensitivity,
                body,
            }
            | Process::CombReactive {
                name,
                sensitivity,
                body,
            } => {
                let mut scan = BodyScan::default();
                scan_body(body, &mut scan);
                if scan.has_timed {
                    return Err(format!(
                        "process '{}' mengandung timed wait (#delay) — tidak cocok mode cycle",
                        name.as_str()
                    ));
                }
                // Comb yang sensitif terhadap clock tetap valid (dievaluasi
                // tiap fase fixed-point). Sensitivity lain diabaikan — kita
                // selalu eval semua comb per sweep.
                let _ = sensitivity;
                comb_pids.push(pid);
            }
            Process::Sequential {
                name, clock, body, ..
            } => {
                let mut scan = BodyScan::default();
                scan_body(body, &mut scan);
                if scan.has_timed {
                    return Err(format!(
                        "process '{}' mengandung timed wait (#delay) — tidak cocok mode cycle",
                        name.as_str()
                    ));
                }
                ff_count += 1;
                // Clock proses FF: field `clock: ClockEdge`. Varian Hier
                // (clock via port interface) belum didukung mode cycle.
                match clock {
                    mivon_ir::ClockEdge::PosEdge(sid) | mivon_ir::ClockEdge::NegEdge(sid) => {
                        check_clock(*sid, &mut clock_opt)?;
                    }
                    other => {
                        return Err(format!(
                            "process '{}' memakai clock hierarkis ({:?}) — tidak cocok mode cycle",
                            name.as_str(),
                            other
                        ));
                    }
                }
                // EventControl edge di body juga kandidat clock (pola
                // always_ff dengan @(posedge clk) di dalam body).
                for sid in &scan.clocks {
                    check_clock(*sid, &mut clock_opt)?;
                }
            }
            Process::AlwaysWithDelay { name, .. } => {
                return Err(format!(
                    "process '{}' adalah always #delay — tidak cocok mode cycle",
                    name.as_str()
                ));
            }
            Process::Initial { name, body } | Process::Final { name, body } => {
                let mut scan = BodyScan::default();
                scan_body(body, &mut scan);
                if scan.has_timed {
                    return Err(format!(
                        "process '{}' mengandung timed wait (#delay) — tidak cocok mode cycle",
                        name.as_str()
                    ));
                }
                for sid in &scan.clocks {
                    check_clock(*sid, &mut clock_opt)?;
                }
            }
        }
    }

    if ff_count == 0 {
        return Err(
            "tidak ada proses Sequential (always_ff) — tidak ada clock untuk didrive".into(),
        );
    }
    let clock = clock_opt.ok_or_else(|| {
        "clock FF tidak terdeteksi (butuh @(posedge/negedge clk) eksplisit)".to_string()
    })?;
    Ok(CyclePlan { clock, comb_pids })
}

/// Hash state seluruh sinyal — deteksi fixed-point settle.
fn hash_signals(engine: &SimulationEngine) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    engine.state.signals.hash(&mut h);
    h.finish()
}

/// Settle satu fase: resume event-control waiter + evaluasi semua comb ke
/// fixed-point. `t` = waktu fase saat ini (untuk konteks evaluasi).
fn settle(
    engine: &mut SimulationEngine,
    plan: &CyclePlan,
    changed_ids: &[SignalId],
    t: usize,
) -> Result<(), SimError> {
    for _ in 0..MAX_SETTLE_ITERS {
        let mut activity = false;

        // Resume `@(clk)` / level waits yang cocok dengan sinyal berubah.
        if !changed_ids.is_empty()
            && !engine.pending_events.is_empty()
            && engine.process_pending_events(changed_ids)?
        {
            activity = true;
        }

        // Drain reactive buffer (hasil evaluasi comb/reactive sebelumnya).
        loop {
            let buffered: Vec<EventKind> = std::mem::take(&mut engine.reactive_events);
            if buffered.is_empty() {
                break;
            }
            activity = true;
            for ev in buffered {
                engine.process_event(ev, t)?;
            }
        }

        // Sweep semua comb process sekali; deteksi perubahan via hash state.
        let before = hash_signals(engine);
        for &pid in &plan.comb_pids {
            engine.process_event(EventKind::EvalProcess(pid), t)?;
        }
        let after = hash_signals(engine);
        if before != after {
            activity = true;
        }

        if !activity {
            return Ok(());
        }
    }
    // Tidak konvergen → combinational loop. Pesan jelas (mode cycle tidak
    // punya delta-limit loop seperti event-driven).
    Err(SimError::with_diag(
        mivon_core::diagnostics::DiagCode::InfiniteDelta,
        format!(
            "cycle-based mode: combinational logic tidak stabil setelah {} iterasi settle di time {} — kemungkinan kombinasional loop. Jalankan ulang tanpa --cycle untuk lokasi delta persisnya.",
            MAX_SETTLE_ITERS, engine.state.time
        ),
    ))
}

/// Refresh snapshot baseline (preponed) sebelum fase — deteksi edge
/// `process_pending_events` membandingkan snapshot vs nilai sekarang.
fn refresh_snapshot(engine: &mut SimulationEngine) {
    let num_sigs = engine.state.signals.len();
    let mut snap = Vec::with_capacity(num_sigs);
    for i in 0..num_sigs {
        snap.push(engine.state.read_signal(i).clone());
    }
    engine.signal_snapshot = Some(std::sync::Arc::new(snap));
}

/// Jalankan loop cycle-based. Return:
/// - `Ok(true)`  → mode selesai dieksekusi penuh (caller skip loop utama).
/// - `Ok(false)` → desain tidak cocok (pesan fallback sudah dicetak) →
///   caller lanjut event-driven biasa.
/// - `Err(e)`    → error runtime nyata.
pub(crate) fn run_cycle_based(engine: &mut SimulationEngine) -> Result<bool, SimError> {
    let plan = match analyze_plan(engine) {
        Ok(p) => p,
        Err(reason) => {
            eprintln!(
                "[mivon] cycle-based mode tidak aktif: {}. Fallback ke event-driven.",
                reason
            );
            return Ok(false);
        }
    };
    let period = engine.cycle_period.max(2);
    let half = (period / 2).max(1);

    let clock_name = engine
        .design
        .top
        .signals
        .get(plan.clock)
        .map(|s| s.name.as_str().to_string())
        .unwrap_or_else(|| format!("#{}", plan.clock));
    eprintln!(
        "[mivon] Cycle-based mode: clock={} period={} ({} ff-clocked, {} comb)",
        clock_name,
        period,
        engine
            .design
            .top
            .processes
            .iter()
            .filter(|p| matches!(p, Process::Sequential { .. }))
            .count(),
        plan.comb_pids.len(),
    );

    // Proses antrean time-0 hasil initialize_time_zero (decl-init, initial
    // blocks, comb awal) — semua same-time karena scan statis sudah menolak
    // timed wait. Bila proses ini menjadwalkan event di waktu MASA DEPAN
    // (timed dinamis yang lolos scan) → fallback SEBELUM output apa pun.
    {
        let mut guard = 0usize;
        loop {
            guard += 1;
            if guard > 100_000 {
                eprintln!(
                    "[mivon] cycle-based mode tidak aktif: antrean time-0 tidak konvergen. Fallback ke event-driven."
                );
                return Ok(false);
            }
            let cur = engine.state.time as usize;
            engine.ensure_events(cur);
            let base = engine.events_base;
            let idx = cur - base;
            if idx >= engine.events.len() || engine.events[idx].is_empty() {
                break;
            }
            let evs: Vec<crate::simulator::types::RegionEvent> =
                std::mem::take(&mut engine.events[idx]);
            for re in evs {
                engine.process_event(re.event, cur)?;
            }
            // Event masa depan terjadwal → desain tidak murni synchronous.
            if engine.events[(idx + 1).min(engine.events.len().saturating_sub(1))..]
                .iter()
                .any(|v| !v.is_empty())
            {
                eprintln!(
                    "[mivon] cycle-based mode tidak aktif: ada event berjadwal waktu dari initial block. Fallback ke event-driven."
                );
                return Ok(false);
            }
        }
    }

    // Driver clk memakai jalur write_lvalue standar — set konteks proses
    // agar race detection tidak salah menuduh (writer konsisten satu "proses").
    engine.current_process_name = Some("<cycle-driver>".to_string());

    let sim_limit = engine.sim_limit;
    let mut high = false;

    while engine.running && sim_limit.allows(engine.state.time) && !engine.is_cancelled() {
        let t = engine.state.time as usize;
        engine.sim_perf.counters.time_steps += 1;
        engine.sim_arena.reset_cycle();

        refresh_snapshot(engine);
        engine.dump_vcd_time()?;
        engine.dump_fst_time()?;

        // ── Fase: drive clk ke nilai berikutnya ──
        let old_clk = engine.state.read_signal(plan.clock).clone();
        let new_val = if high {
            mivon_ir::LogicVec::from_u64(1, old_clk.width.max(1))
        } else {
            mivon_ir::LogicVec::from_u64(0, old_clk.width.max(1))
        };
        high = !high;
        engine.write_lvalue(&mivon_ir::IrLValue::Signal(plan.clock, 0), new_val, true)?;
        let new_clk = engine.state.read_signal(plan.clock).clone();

        // Settle fase pertama (comb bereaksi terhadap level clk baru +
        // stimulus initial yang menunggu sinyal di-resume).
        settle(engine, &plan, &[plan.clock], t)?;

        // ── Edge: trigger FF via jalur standar (iff guard + async reset +
        // cycle-fusion reuse), commit NBA, lalu settle ulang ──
        if old_clk.bits != new_clk.bits {
            engine.sim_perf.counters.sensitive_triggers += 1;
            let changed = vec![(plan.clock, old_clk, new_clk)];
            // Evaluasi proses FF yang terpicu (inline, semantik sama dengan
            // event-driven) + jadwalkan CombReactive.
            engine.trigger_sensitive_processes(&changed, t)?;
            let buffered: Vec<EventKind> = std::mem::take(&mut engine.reactive_events);
            for ev in buffered {
                engine.process_event(ev, t)?;
            }
            // NBA commit di akhir edge region (semantik IEEE: NBA setelah
            // active). FF bodies sudah push ke nba_pending.
            engine.commit_nba();
            // Comb bereaksi ke output FF baru → settle kedua.
            settle(engine, &plan, &[], t)?;
        }

        // Timed event tak terduga (konstruk dinamis yang lolos scan statis)
        // → abort dengan pesan jelas, bukan diam-diam salah hasil.
        if engine.events.iter().any(|v| !v.is_empty()) {
            return Err(SimError::with_diag(
                mivon_core::diagnostics::DiagCode::InternalError,
                "cycle-based mode menemukan event berjadwal waktu di tengah simulasi \
                 (konstruk dinamis/timed). Jalankan ulang tanpa --cycle.",
            ));
        }

        // Coverage toggle/FSM snapshot (pola main loop — capture awal time step).
        // Dipanggil sekali per fase via record_coverage_after_commit di jalur
        // write biasa; snapshot coverage di-refresh di sini agar diff benar.
        let need_cov_snap = engine.coverage_enabled
            && (engine.coverage_enabled_types.is_empty()
                || engine
                    .coverage_enabled_types
                    .contains(&crate::simulator::types::CoverageType::Toggle)
                || engine
                    .coverage_enabled_types
                    .contains(&crate::simulator::types::CoverageType::Fsm));
        if need_cov_snap {
            engine.coverage_snapshot = engine.signal_snapshot.clone();
        }

        engine.state.time += half;
    }
    Ok(true)
}
