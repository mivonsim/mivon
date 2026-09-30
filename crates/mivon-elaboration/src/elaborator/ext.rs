//! ──────────────────────────────────────────────────────────────────────────────
//! CATATAN: File ini adalah bagian dari pemisahan elaborator.rs (SRP Refactoring).
//! Tanggung jawab: Covergroup, DPI imports, multi-driver signal detection.
//!
//! Fungsi:
//!   - elaborate_covergroups()         — elaborate covergroup definitions
//!   - elaborate_dpi_imports()         — elaborate DPI import declarations
//!   - detect_multi_driver_signals()   — deteksi signal multi-driver
//!   - collect_driven_signals()        — kumpulkan signal yang di-driven (static)
//!
//! ──────────────────────────────────────────────────────────────────────────────

use std::collections::{HashMap, HashSet};

use super::Elaborator;
use mivon_ast::{Expr, ModuleItem};
use mivon_core::error::SimError;
use mivon_core::intern::Symbol;
use mivon_ir::*;

impl Elaborator {
    /// Elaborate covergroup definitions — top module di-elaborate penuh;
    /// covergroup di module LAIN dilewati dgn WARNING (transparan).
    ///
    /// Dulu tanpa catatan apa pun: covergroup DUT (ketika testbench jadi
    /// top) tak pernah masuk `design.covergroups` → mcov senyap `0/0`
    /// (bug menyamar: report tampak jalan, covergroup hilang diam-diam).
    /// Resolve penuh utk module non-top tak bisa memakai `signal_map` top
    /// (ident DUT tak ada di scope top → E2001 palsu, atau salah resolve ke
    /// signal top bernama sama → nilai coverage salah senyap) — maka
    /// dilewati dgn warning, bukan di-resolve paksa.
    pub(crate) fn elaborate_covergroups(
        &self,
        top_name: &str,
        signal_map: &HashMap<Symbol, SignalId>,
        signals: &[SignalInfo],
    ) -> Result<Vec<IrCovergroup>, SimError> {
        let mut covergroups = Vec::new();
        // Top module dulu (urutan laporan tetap stabil), lalu module lain
        // (utk warning transparansi — lihat doc di atas).
        let mut modules: Vec<&mivon_ast::Module> = Vec::new();
        if let Some(m) = self.design.modules.iter().find(|m| m.name == top_name) {
            modules.push(m);
        }
        modules.extend(self.design.modules.iter().filter(|m| m.name != top_name));
        for top_module in modules {
            for item in &top_module.items {
                if let ModuleItem::Covergroup(cg) = item {
                    if top_module.name != top_name {
                        self.elab_warn_at(
                            mivon_core::diagnostics::DiagCode::NotImplemented,
                            format!(
                                "covergroup '{}' di module '{}' tidak dielaborate — saat ini \
                                 coverage covergroup hanya utk top '{}' (tidak dihitung, BUKAN \
                                 hilang senyap)",
                                cg.name, top_module.name, top_name
                            ),
                            0,
                            0,
                        );
                        continue;
                    }
                    // Formal arguments kini di-parse parser (CovergroupDecl
                    // .formals), tapi binding nilai saat sample(a,b,c) belum
                    // diimplementasi → coverpoint yang merujuk formal tak
                    // bisa dievaluasi. Skip dengan warning JUJUR (bukan
                    // error jauh E2001 "signal not found" di baris coverpoint,
                    // dan bukan coverage palsu).
                    if !cg.formals.is_empty() {
                        self.elab_warn_at(
                            mivon_core::diagnostics::DiagCode::NotImplemented,
                            format!(
                                "covergroup '{}' punya formal arguments — binding sample() \
                                 belum didukung, covergroup tidak dielaborasi (coverage tidak \
                                 dihitung, BUKAN hilang senyap)",
                                cg.name
                            ),
                            0,
                            0,
                        );
                        continue;
                    }
                    let mut ir_cps = Vec::new();
                    for cp in &cg.coverpoints {
                        let ir_expr = self.elaborate_expr(&cp.expr, signal_map, signals)?;
                        // VERIF-30: turunkan bin eksplisit (bins/illegal_bins/
                        // ignore_bins) — sebelumnya di-parse parser tapi di-drop
                        // di sini sehingga sampler hanya memakai auto-binning.
                        let mut ir_bins = Vec::new();
                        for b in &cp.bins {
                            let mut ranges = Vec::new();
                            for r in &b.range_list {
                                let low = self.elaborate_expr(&r.low, signal_map, signals)?;
                                let high = match &r.high {
                                    Some(h) => Some(self.elaborate_expr(h, signal_map, signals)?),
                                    None => None,
                                };
                                ranges.push(IrBinRange { low, high });
                            }
                            // VERIF-31: transition bins — turunkan tiap sekuens nilai.
                            let mut ir_transitions = Vec::new();
                            for seq in &b.transitions {
                                let mut ir_seq = Vec::new();
                                for v in seq {
                                    ir_seq.push(self.elaborate_expr(v, signal_map, signals)?);
                                }
                                ir_transitions.push(ir_seq);
                            }
                            ir_bins.push(IrBin {
                                name: b.name,
                                ranges,
                                transitions: ir_transitions,
                                bin_type: b.bin_type.clone(),
                            });
                        }
                        ir_cps.push(IrCoverpoint {
                            name: cp.name,
                            expr: ir_expr,
                            bins: ir_bins,
                        });
                    }
                    let ir_crosses = cg
                        .crosses
                        .iter()
                        .map(|c| IrCross {
                            name: c.name,
                            coverpoints: c.coverpoints.clone(),
                        })
                        .collect();
                    // IMPLICIT SAMPLING (§19.8): turunkan clocking event ke
                    // SignalId — engine sample otomatis tiap edge.
                    let (event_signal, event_posedge) = match &cg.clocking_event {
                        Some(ce) => {
                            let sid = match &ce.expr {
                                Expr::Ident { name, .. } => signal_map.get(name).copied(),
                                other => {
                                    // Event non-Ident (mis. `@(posedge a[0])`) —
                                    // belum didukung; tanpa resolve, covergroup
                                    // jatuh ke sample() eksplisit saja (bukan
                                    // senyap-total: ada warning).
                                    let _ = other;
                                    None
                                }
                            };
                            if sid.is_none() {
                                self.elab_warn_at(
                                    mivon_core::diagnostics::DiagCode::NotImplemented,
                                    format!(
                                        "covergroup '{}': clocking event tidak bisa di-resolve — \
             implicit sampling (@edge) nonaktif; pakai sample() eksplisit",
                                        cg.name
                                    ),
                                    0,
                                    0,
                                );
                            }
                            (sid, ce.posedge)
                        }
                        None => (None, true),
                    };
                    covergroups.push(IrCovergroup {
                        name: cg.name,
                        coverpoints: ir_cps,
                        crosses: ir_crosses,
                        // VERIF-28: type_option.weight default 1; per_instance default false.
                        weight: cg.weight.unwrap_or(1),
                        per_instance: cg.per_instance,
                        event_signal,
                        event_posedge,
                    });
                }
            }
        }
        Ok(covergroups)
    }

    /// Elaborate DPI import declarations from all modules.
    /// Apakah design mendeklarasikan DPI import (`import "DPI-C" ...`).
    /// Dipakai untuk memutuskan apakah function tak dikenal di-degrade ke
    /// stub DPI (konteks C eksternal nyata, mis. OpenTitan) ATAU jadi hard
    /// error E3001 (design tanpa DPI sama sekali — regresi test
    /// `test_elab_err_func_not_found_*`).
    pub(crate) fn design_has_dpi_imports(&self) -> bool {
        !self.dpi_import_names.is_empty()
    }

    pub(crate) fn elaborate_dpi_imports(&self) -> Result<Vec<IrDpiImport>, SimError> {
        let mut dpi_imports = Vec::new();
        for module in &self.design.modules {
            for item in &module.items {
                if let ModuleItem::DpiImport(dpi) = item {
                    let return_width = dpi.return_type.as_ref().map(|dt| dt.width()).unwrap_or(1);
                    let arg_widths: Vec<usize> = dpi.args.iter().map(|a| a.dtype.width()).collect();
                    dpi_imports.push(IrDpiImport {
                        name: dpi.name,
                        return_width,
                        arg_widths,
                        is_task: dpi.is_task,
                    });
                }
            }
        }
        Ok(dpi_imports)
    }

    /// Detect signals driven by multiple processes (multi-driver).
    pub(crate) fn detect_multi_driver_signals(&self, top: &mut IrModule) -> Result<(), SimError> {
        let mut driver_count: Vec<usize> = vec![0; top.signals.len()];
        for process in &top.processes {
            match process {
                Process::Combinational { body, .. }
                | Process::CombReactive { body, .. }
                | Process::Sequential { body, .. } => {
                    let mut driven = HashSet::new();
                    Self::collect_driven_signals(body, &mut driven);
                    for id in driven {
                        if id < driver_count.len() {
                            driver_count[id] += 1;
                        }
                    }
                }
                _ => {}
            }
        }
        for (id, count) in driver_count.iter().enumerate() {
            if *count > 1 {
                if let Some(sig) = top.signals.get_mut(id) {
                    if sig.kind == SignalKind::Wire
                        || sig.kind == SignalKind::Reg
                        || sig.kind == SignalKind::Inout
                    {
                        sig.multi_driver = true;
                    }
                }
            }
        }
        Ok(())
    }

    /// Static method: collect signal IDs that are driven (assigned) in IR statements.
    fn collect_driven_signals(stmts: &[IrStmt], driven: &mut HashSet<usize>) {
        for stmt in stmts {
            match stmt {
                IrStmt::BlockingAssign { lhs, .. } | IrStmt::NonBlockingAssign { lhs, .. } => {
                    Self::lv_driven(lhs, driven);
                }
                IrStmt::Block { stmts: body } | IrStmt::NamedBlock { stmts: body, .. } => {
                    Self::collect_driven_signals(body, driven);
                }
                IrStmt::If {
                    true_branch,
                    false_branch,
                    ..
                } => {
                    Self::collect_driven_signals(true_branch, driven);
                    Self::collect_driven_signals(false_branch, driven);
                }
                IrStmt::Case { items, default, .. } => {
                    for item in items {
                        Self::collect_driven_signals(&item.body, driven);
                    }
                    Self::collect_driven_signals(default, driven);
                }
                IrStmt::LoopFor { init, body, .. } => {
                    if let Some(init) = init {
                        Self::collect_driven_signals(&[init.as_ref().clone()], driven);
                    }
                    Self::collect_driven_signals(body, driven);
                }
                IrStmt::LoopWhile { body, .. }
                | IrStmt::LoopDoWhile { body, .. }
                | IrStmt::Repeat { body, .. } => {
                    Self::collect_driven_signals(body, driven);
                }
                IrStmt::Delay { body, .. } | IrStmt::Wait { body, .. } => {
                    Self::collect_driven_signals(body, driven);
                }
                _ => {}
            }
        }
    }

    /// Extract signal ID yang di-drive sebuah lvalue (termasuk part-select,
    /// bit-select, array index, concat — `ff1_pred_10_9[3:0]` = RangeSelect).
    /// Sebelumnya hanya Signal → multi-driver tak terdeteksi utk part-select →
    /// resolver net tak diaktifkan → last-write-wins race antar driver
    /// (fuzzer nondeterminism OpenC910 ct_fadd_close_s0_h).
    fn lv_driven(lv: &IrLValue, driven: &mut HashSet<usize>) {
        match lv {
            IrLValue::Signal(id, _)
            | IrLValue::RangeSelect(id, _, _)
            | IrLValue::BitSelect(id, _)
            | IrLValue::ExprPartSelect { sig_id: id, .. }
            | IrLValue::ArrayIndex { sig_id: id, .. }
            | IrLValue::ArrayRangeSelect { sig_id: id, .. }
            | IrLValue::ArrayBitSelect { sig_id: id, .. } => {
                driven.insert(*id);
            }
            IrLValue::Concat(items) => {
                for it in items {
                    Self::lv_driven(it, driven);
                }
            }
            IrLValue::ObjectField { sig_id, .. } => {
                driven.insert(*sig_id);
            }
            _ => {}
        }
    }
}
