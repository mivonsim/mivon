//! Control flow evaluation methods untuk IR statement blocks.
//! Diekstrak dari block.rs — 1 file = 1 tanggung jawab.
//!
//! Menangani: Block, NamedBlock, If, Case, LoopFor, LoopWhile,
//! LoopDoWhile, Repeat, Foreach, Break, Continue.

use super::super::SimulationEngine;
use super::super::MAX_LOOP_ITER;
use crate::simulator::types::*;
use mivon_core::error::SimError;
use mivon_core::Symbol;
use mivon_ir::*;
use std::collections::HashMap;

impl SimulationEngine {
    // ─── Block / NamedBlock ─────────────────────────────────────────

    /// Evaluate a Block statement with delay/fork support.
    pub(crate) fn evaluate_block_fork(
        &mut self,
        inner: &[IrStmt],
        fork_id: Option<usize>,
    ) -> Result<bool, SimError> {
        self.evaluate_block_with_delay_fork(inner, fork_id)
    }

    /// Evaluate a Block statement without delay/fork (stmt context).
    pub(crate) fn evaluate_block_stmt(&mut self, inner: &[IrStmt]) -> Result<(), SimError> {
        self.evaluate_stmt_block(inner)
    }

    /// Evaluate a NamedBlock with delay/fork support.
    pub(crate) fn evaluate_named_block_fork(
        &mut self,
        name: Symbol,
        inner: &[IrStmt],
        fork_id: Option<usize>,
    ) -> Result<bool, SimError> {
        if self.disable_pending == Some(name) {
            self.disable_pending = None;
            return Ok(true);
        }
        // F47: target `disable_cross` baru mencapai bloknya (label belum
        // ada di stack saat `disable` dieksekusi di branch lain).
        if self.consume_disable_cross_for(name) {
            return Ok(true);
        }
        let old = self.disable_pending.take();
        // Daftar label aktif agar `disable <label>` dari branch lain bisa
        // menyAPA kontinuasi yang dijadwalkan saat blok ini suspend (lihat
        // `Continuation::named_labels`).
        self.known_named_labels.insert(name);
        self.active_named_labels.push(name);
        let completed = self.evaluate_block_with_delay_fork(inner, fork_id);
        self.active_named_labels.pop();
        let completed = completed?;
        if let Some(ref n) = self.disable_pending {
            if *n == name {
                self.disable_pending = None;
            }
        }
        self.disable_pending = self.disable_pending.take().or(old);
        Ok(completed)
    }

    /// Evaluate a NamedBlock without delay/fork (stmt context).
    pub(crate) fn evaluate_named_block_stmt(
        &mut self,
        name: Symbol,
        inner: &[IrStmt],
    ) -> Result<(), SimError> {
        if self.disable_pending == Some(name) {
            self.disable_pending = None;
            return Ok(());
        }
        if self.consume_disable_cross_for(name) {
            return Ok(());
        }
        let old = self.disable_pending.take();
        self.known_named_labels.insert(name);
        self.active_named_labels.push(name);
        let res = self.evaluate_stmt_block(inner);
        self.active_named_labels.pop();
        res?;
        if let Some(ref n) = self.disable_pending {
            if *n == name {
                self.disable_pending = None;
            }
        }
        self.disable_pending = self.disable_pending.take().or(old);
        Ok(())
    }

    // ─── If ─────────────────────────────────────────────────────────

    /// Evaluate an If statement with delay/fork support.
    pub(crate) fn evaluate_if_fork(
        &mut self,
        cond: &IrExpr,
        then_stmts: &[IrStmt],
        else_stmts: &[IrStmt],
        fork_id: Option<usize>,
    ) -> Result<bool, SimError> {
        let cond_val = self.evaluate_expr(cond)?;
        self.cover_branch_counter += 1;
        let branch_key = Symbol::intern(&format!(
            "{}.if_fork#{}",
            self.current_process_name.as_deref().unwrap_or("?"),
            self.cover_branch_counter
        ));
        if cond_val.to_bool().unwrap_or(false) {
            self.record_branch_hit(branch_key, "true");
            self.evaluate_block_with_delay_fork(then_stmts, fork_id)
        } else if !else_stmts.is_empty() {
            self.record_branch_hit(branch_key, "false");
            self.evaluate_block_with_delay_fork(else_stmts, fork_id)
        } else {
            self.record_branch_hit(branch_key, "false_no_else");
            Ok(true)
        }
    }

    /// Evaluate an If statement without delay/fork (stmt context).
    pub(crate) fn evaluate_if_stmt(
        &mut self,
        cond: &IrExpr,
        then_stmts: &[IrStmt],
        else_stmts: &[IrStmt],
    ) -> Result<(), SimError> {
        let cond_val = self.evaluate_expr(cond)?;
        self.cover_branch_counter += 1;
        let branch_key = Symbol::intern(&format!(
            "{}.if_stmt#{}",
            self.current_process_name.as_deref().unwrap_or("?"),
            self.cover_branch_counter
        ));
        if cond_val.to_bool().unwrap_or(false) {
            self.record_branch_hit(branch_key, "true");
            self.evaluate_stmt_block(then_stmts)
        } else if !else_stmts.is_empty() {
            self.record_branch_hit(branch_key, "false");
            self.evaluate_stmt_block(else_stmts)
        } else {
            self.record_branch_hit(branch_key, "false_no_else");
            Ok(())
        }
    }

    // ─── Case ────────────────────────────────────────────────────────

    /// Evaluate a Case statement with delay/fork support.
    #[allow(clippy::needless_return)]
    pub(crate) fn evaluate_case_fork(
        &mut self,
        case_type: &CaseType,
        case_expr: &IrExpr,
        items: &[IrCaseItem],
        default: &[IrStmt],
        fork_id: Option<usize>,
    ) -> Result<bool, SimError> {
        let case_val = self.evaluate_expr(case_expr)?;
        self.cover_branch_counter += 1;
        let case_key = Symbol::intern(&format!(
            "{}.case_fork#{}",
            self.current_process_name.as_deref().unwrap_or("?"),
            self.cover_branch_counter
        ));
        let mut matched = false;
        for (item_idx, case_item) in items.iter().enumerate() {
            let mut item_matched = false;
            for pat in &case_item.labels {
                let eq = self.case_label_match(case_type, &case_val, pat)?;
                if eq {
                    self.record_branch_hit(case_key, &format!("item{}_matched", item_idx));
                    if !self.evaluate_block_with_delay_fork(&case_item.body, fork_id)? {
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
        if !matched && !default.is_empty() {
            self.record_branch_hit(case_key, "default");
            if !self.evaluate_block_with_delay_fork(default, fork_id)? {
                return Ok(false);
            }
        }
        if !matched && default.is_empty() {
            self.record_branch_hit(case_key, "nomatch_nodefault");
        }
        Ok(true)
    }

    /// Cek apakah nilai case cocok dengan satu label. Untuk `CaseType::Inside`,
    /// label berupa `IrExpr::InsideRange` dicocokkan dengan rentang inklusif;
    /// label lain memakai equality biasa.
    fn case_label_match(
        &mut self,
        case_type: &CaseType,
        case_val: &LogicVec,
        pat: &IrExpr,
    ) -> Result<bool, SimError> {
        match (case_type, pat) {
            (CaseType::Inside, IrExpr::InsideRange { lo, hi, .. }) => {
                let lo_v = self.evaluate_expr(lo)?.to_u64();
                let hi_v = self.evaluate_expr(hi)?.to_u64();
                let v = case_val.to_u64();
                Ok(v >= lo_v.min(hi_v) && v <= lo_v.max(hi_v))
            }
            _ => {
                let pat_val = self.evaluate_expr(pat)?;
                let eq = match case_type {
                    CaseType::CaseX => case_val.casex_eq(&pat_val),
                    CaseType::CaseZ => case_val.casez_eq(&pat_val),
                    // LRM: case biasa membandingkan dengan zero-extension ke
                    // lebar terbesar (bukan PartialEq width-sensitive).
                    CaseType::Normal | CaseType::Inside => case_val.case_val_eq(&pat_val),
                    CaseType::Unique | CaseType::Unique0 | CaseType::Priority => {
                        case_val.case_val_eq(&pat_val)
                    }
                    // Qualifier + kind ortogonal (LRM 1800 §12.5): combo
                    // memakai pencocokan wildcard X/Z.
                    CaseType::UniqueX | CaseType::Unique0X | CaseType::PriorityX => {
                        case_val.casex_eq(&pat_val)
                    }
                    CaseType::UniqueZ | CaseType::Unique0Z | CaseType::PriorityZ => {
                        case_val.casez_eq(&pat_val)
                    }
                };
                Ok(eq)
            }
        }
    }

    /// Evaluate a Case statement without delay/fork (stmt context).
    pub(crate) fn evaluate_case_stmt(
        &mut self,
        case_type: &CaseType,
        case_expr: &IrExpr,
        items: &[IrCaseItem],
        default: &[IrStmt],
    ) -> Result<(), SimError> {
        let case_val = self.evaluate_expr(case_expr)?;
        self.cover_branch_counter += 1;
        let case_key = Symbol::intern(&format!(
            "{}.case_stmt#{}",
            self.current_process_name.as_deref().unwrap_or("?"),
            self.cover_branch_counter
        ));
        let mut matched = false;
        for (item_idx, case_item) in items.iter().enumerate() {
            let mut item_matched = false;
            for pat in &case_item.labels {
                let eq = self.case_label_match(case_type, &case_val, pat)?;
                if eq {
                    self.record_branch_hit(case_key, &format!("item{}_matched", item_idx));
                    self.evaluate_stmt_block(&case_item.body)?;
                    if self.disable_pending.is_some() {
                        return Ok(());
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
        if !matched && !default.is_empty() {
            self.record_branch_hit(case_key, "default");
            self.evaluate_stmt_block(default)?;
        }
        if !matched && default.is_empty() {
            self.record_branch_hit(case_key, "nomatch_nodefault");
        }
        Ok(())
    }

    // ─── Break / Continue ────────────────────────────────────────────

    /// Evaluate a Break statement with delay/fork support.
    pub(crate) fn evaluate_break_fork(&mut self) -> Result<bool, SimError> {
        self.control_flow = Some(FlowControl::Break);
        Ok(true)
    }

    /// Evaluate a Break statement without delay/fork (stmt context).
    pub(crate) fn evaluate_break_stmt(&mut self) -> Result<(), SimError> {
        self.control_flow = Some(FlowControl::Break);
        Ok(())
    }

    /// Evaluate a Continue statement with delay/fork support.
    pub(crate) fn evaluate_continue_fork(&mut self) -> Result<bool, SimError> {
        self.control_flow = Some(FlowControl::Continue);
        Ok(true)
    }

    /// Evaluate a Continue statement without delay/fork (stmt context).
    pub(crate) fn evaluate_continue_stmt(&mut self) -> Result<(), SimError> {
        self.control_flow = Some(FlowControl::Continue);
        Ok(())
    }

    // ─── LoopFor ─────────────────────────────────────────────────────

    /// Evaluate a for loop with delay/fork support.
    pub(crate) fn evaluate_loop_for_fork(
        &mut self,
        init: &Option<Box<IrStmt>>,
        cond: &IrExpr,
        step: &Option<Box<IrStmt>>,
        body: &[IrStmt],
        fork_id: Option<usize>,
    ) -> Result<bool, SimError> {
        if let Some(init_stmt) = init {
            if !self.evaluate_block_with_delay_fork(&[*init_stmt.clone()], fork_id)? {
                return Ok(false);
            }
        }
        let mut iter_count = 0usize;
        loop {
            if iter_count >= MAX_LOOP_ITER {
                eprintln!(
                    "warning: for loop exceeded {} iterations, breaking",
                    MAX_LOOP_ITER
                );
                break;
            }
            iter_count += 1;
            if self.disable_pending.is_some() {
                break;
            }
            if self.control_flow.is_some() {
                self.control_flow = None;
                break;
            }
            let cond_val = self.evaluate_expr(cond)?;
            if !cond_val.to_bool().unwrap_or(false) {
                break;
            }
            // F31 fix: set loop_continuation agar body/step yang mengandung
            // delay/event dapat di-resume. Sama seperti evaluate_loop_while_fork:
            //   * fase body  → loop_cont = [step, LoopFor, tail]
            //     (suspend di body: resume = sisa body → step → iterasi berikut)
            //   * fase step  → loop_cont = [LoopFor, tail]
            //     (suspend di step: resume = sisa step → iterasi berikut)
            // init=None pada continuation — variabel loop sudah di-inisialisasi
            // saat eksekusi pertama (tidak boleh di-reset saat resume).
            let old_loop_cont = self.loop_continuation.clone();
            let mut lc = Vec::new();
            if let Some(step_stmt) = step {
                lc.push(*step_stmt.clone());
            }
            lc.push(IrStmt::LoopFor {
                init: None,
                cond: cond.clone(),
                step: step.clone(),
                body: body.to_vec(),
            });
            if !self.post_loop_tail.is_empty() {
                lc.extend(self.post_loop_tail.clone());
            }
            // F31: pertahankan continuation outer (lihat komentar LoopWhile).
            if let Some(cont) = &old_loop_cont {
                lc.extend(cont.clone());
            }
            self.loop_continuation = Some(lc);
            let body_completed = self.evaluate_block_with_delay_fork(body, fork_id)?;
            if !body_completed {
                self.loop_continuation = old_loop_cont;
                return Ok(false);
            }
            let cf = self.control_flow.take();
            if cf == Some(FlowControl::Break) {
                self.loop_continuation = old_loop_cont;
                break;
            }
            if self.disable_pending.is_some() {
                self.loop_continuation = old_loop_cont;
                break;
            }
            // Fase step: continuation TANPA step di depan (step sudah berjalan).
            let mut lc2 = vec![IrStmt::LoopFor {
                init: None,
                cond: cond.clone(),
                step: step.clone(),
                body: body.to_vec(),
            }];
            if !self.post_loop_tail.is_empty() {
                lc2.extend(self.post_loop_tail.clone());
            }
            // F31: pertahankan continuation outer juga di fase step.
            if let Some(cont) = &old_loop_cont {
                lc2.extend(cont.clone());
            }
            self.loop_continuation = Some(lc2);
            if let Some(step_stmt) = step {
                if !self.evaluate_block_with_delay_fork(&[*step_stmt.clone()], fork_id)? {
                    self.loop_continuation = old_loop_cont;
                    return Ok(false);
                }
            }
            self.loop_continuation = old_loop_cont;
            if cf == Some(FlowControl::Continue) {
                continue;
            }
        }
        Ok(true)
    }

    /// Evaluate a for loop without delay/fork (stmt context).
    pub(crate) fn evaluate_loop_for_stmt(
        &mut self,
        init: &Option<Box<IrStmt>>,
        cond: &IrExpr,
        step: &Option<Box<IrStmt>>,
        body: &[IrStmt],
    ) -> Result<(), SimError> {
        if let Some(init_stmt) = init {
            self.evaluate_stmt_block(&[*init_stmt.clone()])?;
        }
        let mut iter_count = 0usize;
        loop {
            if iter_count >= MAX_LOOP_ITER {
                eprintln!(
                    "warning: for loop exceeded {} iterations, breaking",
                    MAX_LOOP_ITER
                );
                break;
            }
            iter_count += 1;
            if self.disable_pending.is_some() {
                break;
            }
            if self.control_flow.is_some() {
                self.control_flow = None;
                break;
            }
            let cond_val = self.evaluate_expr(cond)?;
            if !cond_val.to_bool().unwrap_or(false) {
                break;
            }
            self.evaluate_stmt_block(body)?;
            let cf = self.control_flow.take();
            if cf == Some(FlowControl::Continue) {
                if let Some(step_stmt) = step {
                    self.evaluate_stmt_block(&[*step_stmt.clone()])?;
                }
                continue;
            }
            if cf == Some(FlowControl::Break) {
                break;
            }
            if self.disable_pending.is_some() {
                break;
            }
            if let Some(step_stmt) = step {
                self.evaluate_stmt_block(&[*step_stmt.clone()])?;
            }
        }
        Ok(())
    }

    // ─── LoopWhile ───────────────────────────────────────────────────

    /// Evaluate a while loop with delay/fork support (handles loop_continuation).
    pub(crate) fn evaluate_loop_while_fork(
        &mut self,
        cond: &IrExpr,
        body: &[IrStmt],
        fork_id: Option<usize>,
    ) -> Result<bool, SimError> {
        let mut iter_count = 0usize;
        loop {
            if iter_count >= MAX_LOOP_ITER {
                eprintln!(
                    "warning: while loop exceeded {} iterations, breaking",
                    MAX_LOOP_ITER
                );
                break;
            }
            iter_count += 1;
            if self.disable_pending.is_some() {
                break;
            }
            if self.control_flow.is_some() {
                self.control_flow = None;
                break;
            }
            let cond_val = self.evaluate_expr(cond)?;
            if !cond_val.to_bool().unwrap_or(false) {
                break;
            }
            let old_loop_cont = self.loop_continuation.clone();
            let mut lc = vec![IrStmt::LoopWhile {
                cond: cond.clone(),
                body: body.to_vec(),
            }];
            if !self.post_loop_tail.is_empty() {
                lc.extend(self.post_loop_tail.clone());
            }
            // F31: pertahankan continuation outer (mis. tail block via
            // with_tail_continuation) agar statement setelah block tetap jalan
            // setelah loop selesai di-resume.
            if let Some(cont) = &old_loop_cont {
                lc.extend(cont.clone());
            }
            self.loop_continuation = Some(lc);
            let completed = self.evaluate_block_with_delay_fork(body, fork_id)?;
            self.loop_continuation = old_loop_cont;
            if !completed {
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
        Ok(true)
    }

    /// Evaluate a while loop without delay/fork (no loop_continuation needed).
    pub(crate) fn evaluate_loop_while_stmt(
        &mut self,
        cond: &IrExpr,
        body: &[IrStmt],
    ) -> Result<(), SimError> {
        let mut iter_count = 0usize;
        loop {
            if iter_count >= MAX_LOOP_ITER {
                eprintln!(
                    "warning: while loop exceeded {} iterations, breaking",
                    MAX_LOOP_ITER
                );
                break;
            }
            iter_count += 1;
            if self.disable_pending.is_some() {
                break;
            }
            if self.control_flow.is_some() {
                self.control_flow = None;
                break;
            }
            let cond_val = self.evaluate_expr(cond)?;
            if !cond_val.to_bool().unwrap_or(false) {
                break;
            }
            self.evaluate_stmt_block(body)?;
            let cf = self.control_flow.take();
            if cf == Some(FlowControl::Continue) {
                continue;
            }
            if cf == Some(FlowControl::Break) {
                break;
            }
        }
        Ok(())
    }

    // ─── LoopDoWhile ─────────────────────────────────────────────────

    /// Evaluate a do-while loop with delay/fork support.
    pub(crate) fn evaluate_loop_do_while_fork(
        &mut self,
        cond: &IrExpr,
        body: &[IrStmt],
        fork_id: Option<usize>,
    ) -> Result<bool, SimError> {
        let mut iter_count = 0usize;
        loop {
            if iter_count >= MAX_LOOP_ITER {
                eprintln!(
                    "warning: do-while loop exceeded {} iterations, breaking",
                    MAX_LOOP_ITER
                );
                break;
            }
            iter_count += 1;
            if self.disable_pending.is_some() {
                break;
            }
            if self.control_flow.is_some() {
                self.control_flow = None;
                break;
            }
            let old_loop_cont = self.loop_continuation.clone();
            let mut lc = vec![IrStmt::LoopDoWhile {
                cond: cond.clone(),
                body: body.to_vec(),
            }];
            if !self.post_loop_tail.is_empty() {
                lc.extend(self.post_loop_tail.clone());
            }
            // F31: pertahankan continuation outer (lihat komentar LoopWhile).
            if let Some(cont) = &old_loop_cont {
                lc.extend(cont.clone());
            }
            self.loop_continuation = Some(lc);
            let completed = self.evaluate_block_with_delay_fork(body, fork_id)?;
            self.loop_continuation = old_loop_cont;
            if !completed {
                return Ok(false);
            }
            let cf = self.control_flow.take();
            if cf == Some(FlowControl::Continue) {
                continue;
            }
            if cf == Some(FlowControl::Break) {
                break;
            }
            let cond_val = self.evaluate_expr(cond)?;
            if !cond_val.to_bool().unwrap_or(false) {
                break;
            }
        }
        Ok(true)
    }

    /// Evaluate a do-while loop without delay/fork (stmt context).
    pub(crate) fn evaluate_loop_do_while_stmt(
        &mut self,
        cond: &IrExpr,
        body: &[IrStmt],
    ) -> Result<(), SimError> {
        let mut iter_count = 0usize;
        loop {
            if iter_count >= MAX_LOOP_ITER {
                eprintln!(
                    "warning: do-while loop exceeded {} iterations, breaking",
                    MAX_LOOP_ITER
                );
                break;
            }
            iter_count += 1;
            if self.disable_pending.is_some() {
                break;
            }
            if self.control_flow.is_some() {
                self.control_flow = None;
                break;
            }
            self.evaluate_stmt_block(body)?;
            let cf = self.control_flow.take();
            if cf == Some(FlowControl::Continue) {
                continue;
            }
            if cf == Some(FlowControl::Break) {
                break;
            }
            let cond_val = self.evaluate_expr(cond)?;
            if !cond_val.to_bool().unwrap_or(false) {
                break;
            }
        }
        Ok(())
    }

    // ─── Repeat ──────────────────────────────────────────────────────

    /// Evaluate a Repeat loop with delay/fork support.
    /// F31 fix: sama seperti evaluate_loop_while_fork — set `loop_continuation`
    /// berisi sisa iterasi + post_loop_tail SEBELUM mengeksekusi body, sehingga
    /// body yang mengandung delay/event (`repeat (N) @(posedge clk)`) dapat
    /// di-resume dan loop berulang. Sebelumnya body ber-delay hanya dieksekusi
    /// sekali lalu loop hilang (hang / early-exit). Count continuation dibuat
    /// konstanta sisa (bukan ekspresi asli) agar tidak mengulang dari awal.
    pub(crate) fn evaluate_repeat_fork(
        &mut self,
        count: &IrExpr,
        body: &[IrStmt],
        fork_id: Option<usize>,
    ) -> Result<bool, SimError> {
        let count_val = self.evaluate_expr(count)?;
        let n = (count_val.to_u64() as usize).min(MAX_LOOP_ITER);
        for iter in 0..n {
            if self.disable_pending.is_some() {
                break;
            }
            if self.control_flow.is_some() {
                self.control_flow = None;
                break;
            }
            // Sisa iterasi setelah iterasi ini (untuk loop_continuation).
            let remaining = n - iter - 1;
            let old_loop_cont = self.loop_continuation.clone();
            let mut lc = vec![IrStmt::Repeat {
                count: IrExpr::Const(LogicVec::from_u64(remaining as u64, 32)),
                body: body.to_vec(),
            }];
            if !self.post_loop_tail.is_empty() {
                lc.extend(self.post_loop_tail.clone());
            }
            // F31: pertahankan continuation outer (lihat komentar LoopWhile).
            if let Some(cont) = &old_loop_cont {
                lc.extend(cont.clone());
            }
            self.loop_continuation = Some(lc);
            let completed = self.evaluate_block_with_delay_fork(body, fork_id)?;
            self.loop_continuation = old_loop_cont;
            if !completed {
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
        Ok(true)
    }

    /// Evaluate a Repeat loop without delay/fork (stmt context).
    pub(crate) fn evaluate_repeat_stmt(
        &mut self,
        count: &IrExpr,
        body: &[IrStmt],
    ) -> Result<(), SimError> {
        let count_val = self.evaluate_expr(count)?;
        let n = (count_val.to_u64() as usize).min(MAX_LOOP_ITER);
        for _ in 0..n {
            if self.disable_pending.is_some() {
                break;
            }
            if self.control_flow.is_some() {
                self.control_flow = None;
                break;
            }
            self.evaluate_stmt_block(body)?;
            let cf = self.control_flow.take();
            if cf == Some(FlowControl::Continue) {
                continue;
            }
            if cf == Some(FlowControl::Break) {
                break;
            }
        }
        Ok(())
    }

    // ─── Foreach ─────────────────────────────────────────────────────

    /// Evaluate a foreach loop with delay/fork support.
    pub(crate) fn evaluate_foreach_fork(
        &mut self,
        array_var: &IrExpr,
        index_var: &Symbol,
        body: &[IrStmt],
        fork_id: Option<usize>,
    ) -> Result<bool, SimError> {
        let lv = self.evaluate_expr(array_var)?;
        let sig_info = if let IrExpr::Signal(id, _) = array_var {
            self.design.top.signals.get(*id)
        } else {
            None
        };
        let elem_width = sig_info.map(|s| s.elem_width).unwrap_or(1);
        let count = lv.width.checked_div(elem_width).unwrap_or(0);
        for i in 0..count {
            if self.disable_pending.is_some() {
                break;
            }
            if self.control_flow.is_some() {
                self.control_flow = None;
                break;
            }
            let idx_val = LogicVec::from_u64(i as u64, 32);
            let mut scope = HashMap::new();
            scope.insert(*index_var, idx_val);
            let depth = self.method_locals.len();
            self.method_locals.push(scope);
            if !self.evaluate_block_with_delay_fork(body, fork_id)? {
                self.method_locals.truncate(depth);
                return Ok(false);
            }
            self.method_locals.truncate(depth);
            let cf = self.control_flow.take();
            if cf == Some(FlowControl::Continue) {
                continue;
            }
            if cf == Some(FlowControl::Break) {
                break;
            }
        }
        Ok(true)
    }

    /// Evaluate a foreach loop without delay/fork (stmt context).
    pub(crate) fn evaluate_foreach_stmt(
        &mut self,
        array_var: &IrExpr,
        index_var: &Symbol,
        body: &[IrStmt],
    ) -> Result<(), SimError> {
        let lv = self.evaluate_expr(array_var)?;
        let sig_info = if let IrExpr::Signal(id, _) = array_var {
            self.design.top.signals.get(*id)
        } else {
            None
        };
        let elem_width = sig_info.map(|s| s.elem_width).unwrap_or(1);
        let count = lv.width.checked_div(elem_width).unwrap_or(0);
        for i in 0..count {
            if self.disable_pending.is_some() {
                break;
            }
            if self.control_flow.is_some() {
                self.control_flow = None;
                break;
            }
            let idx_val = LogicVec::from_u64(i as u64, 32);
            let mut scope = HashMap::new();
            scope.insert(*index_var, idx_val);
            let depth = self.method_locals.len();
            self.method_locals.push(scope);
            self.evaluate_stmt_block(body)?;
            self.method_locals.truncate(depth);
            let cf = self.control_flow.take();
            if cf == Some(FlowControl::Continue) {
                continue;
            }
            if cf == Some(FlowControl::Break) {
                break;
            }
        }
        Ok(())
    }
}
