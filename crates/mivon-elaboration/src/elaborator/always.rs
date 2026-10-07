use super::super::util::const_eval_params;
use super::super::util::{detect_sync_reset, infer_comb_sensitivity, resolve_expr_signal};
use super::Elaborator;
use mivon_ast::*;
use mivon_core::diagnostics::diagnostic::DiagCode;
use mivon_core::error::SimError;
use mivon_core::intern::Symbol;
use mivon_ir::*;
use std::collections::HashMap;

impl Elaborator {
    pub(crate) fn elaborate_always(
        &self,
        always: &AlwaysBlock,
        signal_map: &HashMap<Symbol, SignalId>,
        signals: &[SignalInfo],
    ) -> Result<Process, SimError> {
        let name = Symbol::intern(&format!("always_{}", 0));
        // SIM-29: set nama proses agar stmt_lines tercatat dengan key yang
        // SAMA dengan record_line_hit engine (current_process_name).
        *self.current_proc_name.borrow_mut() = Some(name);

        let result = self.elaborate_always_inner(always, signal_map, signals, name);
        *self.current_proc_name.borrow_mut() = None;
        result
    }

    fn elaborate_always_inner(
        &self,
        always: &AlwaysBlock,
        signal_map: &HashMap<Symbol, SignalId>,
        signals: &[SignalInfo],
        name: Symbol,
    ) -> Result<Process, SimError> {
        match always.kind {
            AlwaysKind::AlwaysComb | AlwaysKind::AlwaysLatch => {
                let body = self.elaborate_stmt_block(&always.stmts, signal_map, &[], signals)?;
                let sensitivity = infer_comb_sensitivity(&body)
                    .into_iter()
                    .map(SignalSensitivity::whole)
                    .collect();
                Ok(Process::CombReactive {
                    name,
                    sensitivity,
                    body,
                })
            }
            AlwaysKind::AlwaysFF => {
                let (clock, reset, iff) =
                    self.extract_clock_reset(&always.sensitivity, signal_map, signals)?;
                let body = self.elaborate_stmt_block(&always.stmts, signal_map, &[], signals)?;
                let reset = reset.or_else(|| detect_sync_reset(&body));
                Ok(Process::Sequential {
                    name,
                    clock,
                    reset,
                    body,
                    iff,
                })
            }
            AlwaysKind::Always => {
                // Check if body starts with a delay (always #N pattern)
                if always.sensitivity.is_none() && always.stmts.len() == 1 {
                    if let Stmt::Delay { delay, stmt } = &always.stmts[0] {
                        if let Ok(d) = const_eval_params(delay, &self.param_vals) {
                            let body = self.elaborate_stmt_block(
                                &[stmt.as_ref().clone()],
                                signal_map,
                                &[],
                                signals,
                            )?;
                            // F84 (LRM §19.8): satuan `always #N` milik
                            // module ini, sama seperti `Stmt::Delay` biasa.
                            // `#0` tetap 0 (delta cycle) — konsisten dgn
                            // lengan `Stmt::Delay`.
                            let delay = (d as u64).saturating_mul(self.current_delay_scale());
                            return Ok(Process::AlwaysWithDelay { name, delay, body });
                        }
                    }
                }
                // Check if sensitivity has clock edges -> Sequential process
                if let Some(sl) = &always.sensitivity {
                    if sl.events.iter().any(is_edge_event) {
                        if let Ok((clock, reset, iff)) =
                            self.extract_clock_reset(&always.sensitivity, signal_map, signals)
                        {
                            let body =
                                self.elaborate_stmt_block(&always.stmts, signal_map, &[], signals)?;
                            let reset = reset.or_else(|| detect_sync_reset(&body));
                            return Ok(Process::Sequential {
                                name,
                                clock,
                                reset,
                                body,
                                iff,
                            });
                        } // fall through to combinational
                    }
                }
                let body = self.elaborate_stmt_block(&always.stmts, signal_map, &[], signals)?;
                let sensitivity = match &always.sensitivity {
                    Some(sl) => {
                        let has_wildcard = sl.events.iter().any(|e| match e {
                            SensitivityEvent::Iff { event, .. } => {
                                matches!(event.as_ref(), SensitivityEvent::Wildcard)
                            }
                            SensitivityEvent::Wildcard => true,
                            _ => false,
                        });
                        if has_wildcard {
                            infer_comb_sensitivity(&body)
                                .into_iter()
                                .map(SignalSensitivity::whole)
                                .collect()
                        } else {
                            sl.events
                                .iter()
                                .filter_map(|e| {
                                    let inner = match e {
                                        SensitivityEvent::Iff { event, .. } => event.as_ref(),
                                        other => other,
                                    };
                                    match inner {
                                        SensitivityEvent::Level(expr) => {
                                            resolve_expr_sensitivity(expr, signal_map, signals)
                                        }
                                        _ => None,
                                    }
                                })
                                .collect()
                        }
                    }
                    None => Vec::new(),
                };
                Ok(Process::Combinational {
                    name,
                    sensitivity,
                    body,
                })
            }
        }
    }

    fn extract_clock_reset(
        &self,
        sensitivity: &Option<SensitivityList>,
        signal_map: &HashMap<Symbol, SignalId>,
        signals: &[SignalInfo],
    ) -> Result<(ClockEdge, Option<ResetInfo>, Option<IrExpr>), SimError> {
        let events = match sensitivity {
            Some(sl) => &sl.events,
            None => {
                return Err(self.elab_diag(
                    DiagCode::ModuleNotFound,
                    "always_ff requires sensitivity list",
                ))
            }
        };

        // Lokasi fallback: event pertama yang punya expr — agar error
        // "no clock edge" render dengan file:line:col (bukan tanpa lokasi).
        let event_loc: (usize, usize) = events
            .iter()
            .find_map(|e| match e {
                SensitivityEvent::Iff { event, .. } => event_loc_of(event),
                other => event_loc_of(other),
            })
            .unwrap_or((0, 0));

        let mut clock_edge = None;
        let mut reset = None;
        // LANG-27: guard `iff (cond)` dari event clock utama.
        let mut iff = None;

        for event in events {
            // Buka bungkus Iff — periksa event asli, simpan kondisi guard.
            let (inner, guard) = match event {
                SensitivityEvent::Iff { event, cond } => (event.as_ref(), Some(cond)),
                other => (other, None),
            };
            match inner {
                SensitivityEvent::PosEdge(expr) | SensitivityEvent::NegEdge(expr) => {
                    let sig_id = resolve_expr_signal(expr, signal_map);
                    let is_pos = matches!(inner, SensitivityEvent::PosEdge(_));
                    if let Some(sid) = sig_id {
                        if clock_edge.is_none() {
                            clock_edge = Some(if is_pos {
                                ClockEdge::PosEdge(sid)
                            } else {
                                ClockEdge::NegEdge(sid)
                            });
                            if let Some(cond) = guard {
                                iff = Some(self.elaborate_expr(cond, signal_map, signals)?);
                            }
                        } else if reset.is_none() {
                            reset = Some(ResetInfo {
                                signal: sid,
                                polarity: is_pos,
                                r#async: true,
                                value: LogicVec::new(1),
                            });
                        }
                    } else {
                        // F27: clock hierarkis via port interface
                        // (`always_ff @(posedge b.clk)`). Member access tidak punya
                        // SignalId saat elaborasi (flatten belum jalan) — pakai
                        // ClockEdge::*Hier(Symbol) yang di-resolve engine via
                        // hier_signal_map saat simulasi. HANYA untuk base port
                        // interface (iface_type tanpa class_name — vif variabel
                        // pakai class_name) agar member access lain yang tak
                        // ter-resolve tetap error seperti sebelumnya (bukan
                        // process senyap yang tak pernah trigger).
                        let hier_full = match expr {
                            Expr::MemberAccess { obj, field } => {
                                Self::build_hier_name(obj, field.as_str())
                            }
                            _ => String::new(),
                        };
                        let base_is_iface = match expr {
                            Expr::MemberAccess { obj, .. } => {
                                resolve_expr_signal(obj, signal_map)
                                    .and_then(|sid| signals.get(sid))
                                    .map(|s| s.iface_type.is_some() && s.class_name.is_none())
                                    .unwrap_or(false)
                                    // Instance interface di module yg sama
                                    // (`bus_if b(); ... posedge b.clk`).
                                    || match obj.as_ref() {
                                        Expr::Ident { name, .. } => {
                                            self.is_interface_instance(name.as_str())
                                        }
                                        _ => false,
                                    }
                            }
                            _ => false,
                        };
                        if base_is_iface && !hier_full.is_empty() && clock_edge.is_none() {
                            clock_edge = Some(if is_pos {
                                ClockEdge::PosEdgeHier(Symbol::intern(&hier_full))
                            } else {
                                ClockEdge::NegEdgeHier(Symbol::intern(&hier_full))
                            });
                            if let Some(cond) = guard {
                                iff = Some(self.elaborate_expr(cond, signal_map, signals)?);
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        let ce = clock_edge.ok_or_else(|| {
            self.elab_diag_at(
                DiagCode::ModuleNotFound,
                "always_ff must have at least one clock edge",
                event_loc.0,
                event_loc.1,
            )
        })?;
        Ok((ce, reset, iff))
    }
}

/// Lokasi event sensitivitas (unwrap Iff), dari expr pembawa.
fn event_loc_of(e: &SensitivityEvent) -> Option<(usize, usize)> {
    match e {
        SensitivityEvent::Iff { event, .. } => event_loc_of(event),
        SensitivityEvent::PosEdge(expr)
        | SensitivityEvent::NegEdge(expr)
        | SensitivityEvent::Level(expr) => {
            let (l, c) = crate::util::generate::expr_location(expr);
            if l > 0 || c > 0 {
                Some((l, c))
            } else {
                None
            }
        }
        SensitivityEvent::Wildcard => None,
    }
}

/// true bila event (termasuk terbungkus `iff`) adalah edge clock.
fn is_edge_event(e: &SensitivityEvent) -> bool {
    match e {
        SensitivityEvent::Iff { event, .. } => is_edge_event(event),
        SensitivityEvent::PosEdge(_) | SensitivityEvent::NegEdge(_) => true,
        _ => false,
    }
}

/// Resolve expression sensitivity list menjadi (signal, bit-range) yang memicu.
/// - `sig`        → seluruh signal (whole).
/// - `sig[k]`     → range element k pada packed array multi-dimensi.
/// - `sig[a:b]`   → range eksplisit.
fn resolve_expr_sensitivity(
    expr: &Expr,
    signal_map: &HashMap<Symbol, SignalId>,
    signals: &[SignalInfo],
) -> Option<SignalSensitivity> {
    match expr {
        Expr::Ident { name, .. } => signal_map.get(name).map(|&id| SignalSensitivity::whole(id)),
        Expr::BitSelect { expr: inner, index } => {
            if let Expr::Ident { name, .. } = inner.as_ref() {
                if let Some(&sid) = signal_map.get(name) {
                    let sig = &signals[sid];
                    if sig.packed_dims.len() > 1 && sig.packed_dims[0] > 0 {
                        if let Ok(idx) = const_eval_params(index, &HashMap::new()) {
                            let ow = sig.width / sig.packed_dims[0];
                            let lo = idx.max(0) as usize * ow;
                            return Some(SignalSensitivity {
                                sig_id: sid,
                                msb: Some(lo + ow - 1),
                                lsb: Some(lo),
                            });
                        }
                    }
                    return Some(SignalSensitivity::whole(sid));
                }
            }
            None
        }
        Expr::RangeSelect {
            expr: inner,
            msb,
            lsb,
        } => {
            if let Expr::Ident { name, .. } = inner.as_ref() {
                if let Some(&sid) = signal_map.get(name) {
                    if let (Ok(m), Ok(l)) = (
                        const_eval_params(msb, &HashMap::new()),
                        const_eval_params(lsb, &HashMap::new()),
                    ) {
                        return Some(SignalSensitivity {
                            sig_id: sid,
                            msb: Some(m.max(0) as usize),
                            lsb: Some(l.max(0) as usize),
                        });
                    }
                    return Some(SignalSensitivity::whole(sid));
                }
            }
            None
        }
        _ => resolve_expr_signal(expr, signal_map).map(SignalSensitivity::whole),
    }
}
