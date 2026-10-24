use super::SimulationEngine;
use mivon_core::error::SimError;
use mivon_ir::*;

impl SimulationEngine {
    /// Evaluator sequence dengan depth-aware: mengevaluasi expression pada
    /// state sinyal yang tepat (historical atau current) berdasarkan posisi
    /// dalam sequence temporal (##N).
    ///
    /// Parameter:
    /// - `seq`: sequence yang akan dievaluasi
    /// - `elapsed`: jumlah posedge yang telah berlalu sejak sub-sequence dimulai
    /// - `depth`: kedalaman historical (0 = current posedge, 1 = 1 posedge lalu)
    ///
    /// Semantics:
    /// - `Expr(expr)`: true jika expr true pada posedge (now - depth), hanya
    ///   saat elapsed == 0 (expr menempati 0 cycle)
    /// - `Delay(n)`: true jika elapsed == n
    /// - `Concat(A, B)`: split pada k, A di (k, depth), B di (elapsed-k, depth-k)
    pub(crate) fn eval_sequence_depth(
        &mut self,
        seq: &IrSequence,
        elapsed: u64,
        depth: u64,
    ) -> Result<bool, SimError> {
        match seq {
            IrSequence::Expr(expr) => {
                if elapsed == 0 {
                    let val = self.evaluate_expr_at_depth(expr, depth)?;
                    Ok(val.to_bool() == Some(true))
                } else {
                    Ok(false)
                }
            }
            IrSequence::Delay(n) => Ok(elapsed == *n),
            IrSequence::DelayRange(min, max) => Ok(elapsed >= *min && elapsed <= *max),
            IrSequence::Concat(a, b) => {
                // A occupies k cycles (from sequence start), B occupies (elapsed-k) cycles
                // A evaluated at depth (same as concat start)
                // B evaluated at depth-k (k cycles into the concat = k posedges later)
                for k in 0..=elapsed {
                    let b_depth = depth.saturating_sub(k);
                    if self.eval_sequence_depth(a, k, depth)?
                        && self.eval_sequence_depth(b, elapsed - k, b_depth)?
                    {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            IrSequence::Or(a, b) => Ok(self.eval_sequence_depth(a, elapsed, depth)?
                || self.eval_sequence_depth(b, elapsed, depth)?),
            IrSequence::And(a, b) => Ok(self.eval_sequence_depth(a, elapsed, depth)?
                && self.eval_sequence_depth(b, elapsed, depth)?),
            IrSequence::Repeat(seq, n) => {
                if *n == 0 {
                    return Ok(true);
                }
                if *n == 1 {
                    return self.eval_sequence_depth(seq, elapsed, depth);
                }
                for k in 0..=elapsed {
                    if self.eval_sequence_depth(seq, k, depth)? {
                        let remaining = IrSequence::Repeat(Box::new((**seq).clone()), n - 1);
                        if self.eval_sequence_depth(
                            &remaining,
                            elapsed - k,
                            depth.saturating_sub(k),
                        )? {
                            return Ok(true);
                        }
                    }
                }
                Ok(false)
            }
            IrSequence::Implication(ante, cons) => {
                // |-> (overlap) dan |=> (non-overlap, di-encode sebagai
                // Concat(Delay(1), cons)) sama-sama berarti: cari cycle k saat
                // antecedent SELESAI, lalu consequent berjalan dari situ.
                // Matching-nya identik dengan Concat(ante, cons).
                for k in 0..=elapsed {
                    let cons_depth = depth.saturating_sub(k);
                    if self.eval_sequence_depth(ante, k, depth)?
                        && self.eval_sequence_depth(cons, elapsed - k, cons_depth)?
                    {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }

    /// Evaluator lama — wrapper untuk backward compatibility.
    #[allow(dead_code)]
    pub(crate) fn eval_sequence_at_cycle(
        &mut self,
        seq: &IrSequence,
        cycles: u64,
    ) -> Result<bool, SimError> {
        self.eval_sequence_depth(seq, cycles, cycles)
    }

    /// Evaluate expression pada kedalaman historical tertentu.
    /// depth=0: current state, depth>0: signal_seq_history[depth].
    fn evaluate_expr_at_depth(&mut self, expr: &IrExpr, depth: u64) -> Result<LogicVec, SimError> {
        if depth == 0 {
            return self.evaluate_expr(expr);
        }
        // Ambil snapshot historis (depth posedges lalu)
        if let Some(snapshot) = self.signal_seq_history.get(depth as usize) {
            // Simpan current signal values, replace dengan historical, evaluate, restore
            let num_sigs = self.state.signals.len();
            let mut saved: Vec<(usize, LogicVec)> = Vec::new();
            for i in 0..num_sigs {
                if let Some(hv) = snapshot.get(i) {
                    let cur = self.state.read_signal(i).clone();
                    if cur != *hv {
                        self.state.write_signal(i, hv.clone());
                        saved.push((i, cur));
                    }
                }
            }
            let result = self.evaluate_expr(expr);
            // Restore
            for (i, old) in saved {
                self.state.write_signal(i, old);
            }
            result
        } else {
            // Tidak cukup history — fallback ke current state
            self.evaluate_expr(expr)
        }
    }

    pub(crate) fn evaluate_sequence_attempts(&mut self) -> Result<(), SimError> {
        if self.sequence_attempts.is_empty() {
            return Ok(());
        }
        // PENTING: posedge detection untuk sequence harus pakai snapshot
        // pre-NBA (preponed_snapshot), bukan signal_snapshot yang sudah
        // ter-refresh ke post-NBA. Tanpa ini, `old==current` → posedge tidak
        // terdeteksi → attempt timeout palsu (selalu pass).
        std::mem::swap(&mut self.signal_snapshot, &mut self.preponed_snapshot);
        let firing_events: Vec<bool> = self
            .sequence_attempts
            .iter()
            .map(|a| self.check_concurrent_clock_event(&a.clock_event))
            .collect();
        std::mem::swap(&mut self.signal_snapshot, &mut self.preponed_snapshot);

        let seqs: Vec<(Box<IrSequence>, u64)> = self
            .sequence_attempts
            .iter()
            .enumerate()
            .filter(|(idx, _)| *idx < firing_events.len() && firing_events[*idx])
            .map(|(_, a)| (a.sequence.clone(), a.cycles))
            .collect();

        let mut results: Vec<bool> = Vec::new();
        let mut ante_hits: Vec<Option<bool>> = Vec::new();
        for (seq, cycles) in &seqs {
            // depth = cycles: sequence dimulai cycles posedge lalu
            let r = self.eval_sequence_depth(seq, *cycles, *cycles)?;
            results.push(r);
            // Lacak apakah antecedent implication selesai pada cycle ini
            // (dipakai untuk vacuous pass saat attempt habis).
            let hit = match &**seq {
                IrSequence::Implication(ante, _) => {
                    Some(self.eval_sequence_depth(ante, *cycles, *cycles)?)
                }
                _ => None,
            };
            ante_hits.push(hit);
        }

        let mut completed = Vec::new();
        let mut result_idx = 0;
        for (idx, attempt) in self.sequence_attempts.iter_mut().enumerate() {
            if idx < firing_events.len() && firing_events[idx] {
                let matched = if result_idx < results.len() {
                    results[result_idx]
                } else {
                    false
                };
                let ante_hit = ante_hits.get(result_idx).copied().flatten();
                result_idx += 1;
                if ante_hit == Some(true) {
                    attempt.ante_matched = Some(true);
                }
                let max_cycles = attempt.sequence.max_cycles().unwrap_or(u64::MAX);
                if matched {
                    completed.push((idx, true));
                } else if attempt.cycles >= max_cycles {
                    // IEEE 1800-2017 §16.12.2: implication yang antecedent-nya
                    // TIDAK PERNAH match adalah vacuous pass, bukan fail.
                    let vacuous = matches!(&*attempt.sequence, IrSequence::Implication(..))
                        && attempt.ante_matched != Some(true);
                    completed.push((idx, vacuous));
                }
                attempt.cycles += 1;
            }
        }

        for (idx, success) in completed.into_iter().rev() {
            // Copy posisi + stmts dalam scope borrow terpisah agar &mut self
            // (record_assertion / evaluate_block_with_delay_fork) valid.
            let (a_line, a_col, stmts) = match self.sequence_attempts.get(idx) {
                Some(a) => (
                    a.line,
                    a.col,
                    if success {
                        a.pass_stmt.clone()
                    } else {
                        a.fail_stmt.clone()
                    },
                ),
                None => (0, 0, Vec::new()),
            };
            // VERIF-27: assertion coverage metrics utk concurrent
            // assertion (sequence) — pass/fail saat attempt selesai.
            self.record_assertion(a_line, a_col, success);
            // VERIF-32: sequence coverage — matched/failed per sequence.
            self.record_sequence_result(a_line, a_col, success);
            if !stmts.is_empty() {
                self.evaluate_block_with_delay_fork(&stmts, None)?;
            }
            self.sequence_attempts.remove(idx);
        }
        Ok(())
    }

    pub(crate) fn check_concurrent_clock_event(&self, ce: &mivon_ast::types::ClockEvent) -> bool {
        let sig_name = match ce {
            mivon_ast::types::ClockEvent::Posedge(s) => s,
            mivon_ast::types::ClockEvent::Negedge(s) => s,
            mivon_ast::types::ClockEvent::Edge(s) => s,
        };
        let sig_id = match self.find_signal(sig_name.as_str()) {
            Some(id) => id,
            None => return true,
        };
        let curr = self.state.read_signal(sig_id);
        match ce {
            mivon_ast::types::ClockEvent::Posedge(_) => {
                if let Some(ref snap) = self.signal_snapshot {
                    let old = snap
                        .get(sig_id)
                        .cloned()
                        .unwrap_or_else(|| LogicVec::new(1));
                    old.to_bool() != Some(true) && curr.to_bool() == Some(true)
                } else {
                    curr.to_bool() == Some(true)
                }
            }
            mivon_ast::types::ClockEvent::Negedge(_) => {
                if let Some(ref snap) = self.signal_snapshot {
                    let old = snap
                        .get(sig_id)
                        .cloned()
                        .unwrap_or_else(|| LogicVec::new(1));
                    old.to_bool() != Some(false) && curr.to_bool() == Some(false)
                } else {
                    curr.to_bool() == Some(false)
                }
            }
            mivon_ast::types::ClockEvent::Edge(_) => {
                if let Some(ref snap) = self.signal_snapshot {
                    let old = snap
                        .get(sig_id)
                        .cloned()
                        .unwrap_or_else(|| LogicVec::new(1));
                    old.to_bool() != curr.to_bool()
                } else {
                    true
                }
            }
        }
    }
}
