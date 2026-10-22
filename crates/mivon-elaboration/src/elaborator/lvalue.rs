//! Elaborasi lvalue: dari AST ekspresi kiri-tugas ke IR assignment target
//! (signal, bit/part select, member struct, elemen array), plus resolusi
//! chain member access (`a.b.c`) ke offset field struct.
//!
//! Dipisah dari `stmt.rs` karena bagian ini ~1400 baris dan punya tanggung
//! jawab sendiri: memetakan "tempat menulis" ke (SignalId, offset) yang
//! dipahami engine.

use super::super::util::*;
use super::stmt::{describe_lvalue_expr, select_lv_bit, select_lv_range, ChainStep};
use super::Elaborator;
use mivon_ast::types::const_eval_with_params;
use mivon_ast::*;
use mivon_core::diagnostics::diagnostic::DiagCode;
use mivon_core::diagnostics::suggest::suggest_name;
use mivon_core::error::SimError;
use mivon_core::intern::Symbol;
use mivon_ir::*;
use std::collections::HashMap;

impl Elaborator {
    pub(crate) fn elaborate_lvalue(
        &self,
        expr: &Expr,
        signal_map: &HashMap<Symbol, SignalId>,
        signals: &[SignalInfo],
    ) -> Result<IrLValue, SimError> {
        match expr {
            Expr::Ident { name, line, col } => {
                let sig_id = signal_map.get(name).ok_or_else(|| {
                    let candidates: Vec<&str> = signal_map.keys().map(|s| s.as_str()).collect();
                    let hint = suggest_name(name.as_str(), candidates.into_iter())
                        .map(|(s, _)| format!(" — did you mean '{}'?", s))
                        .unwrap_or_default();
                    self.elab_diag_at(
                        DiagCode::UndefinedSignal,
                        format!("signal '{}' not found{}", name, hint),
                        *line,
                        *col,
                    )
                })?;
                Ok(IrLValue::Signal(*sig_id, 0))
            }
            Expr::RangeSelect {
                expr: inner,
                msb,
                lsb,
            } => {
                let inner_lv = self.elaborate_lvalue(inner, signal_map, signals)?;
                // Bound range yang gagal di-const-eval (mis. member access
                // struct, param hilang) → fallback 0 + warning, bukan error
                // yang mematikan modul.
                let msb_c = match const_eval_params(msb, &self.param_vals) {
                    Ok(v) => v.max(0) as usize,
                    Err(e) => {
                        let (l, c) = crate::util::generate::expr_location(msb);
                        self.elab_warn_at(
                            DiagCode::SimulationError,
                            format!("cannot evaluate lvalue range bound ({}), fallback 0", e),
                            l,
                            c,
                        );
                        0
                    }
                };
                let lsb_c = match const_eval_params(lsb, &self.param_vals) {
                    Ok(v) => v.max(0) as usize,
                    Err(e) => {
                        let (l, c) = crate::util::generate::expr_location(lsb);
                        self.elab_warn_at(
                            DiagCode::SimulationError,
                            format!("cannot evaluate lvalue range bound ({}), fallback 0", e),
                            l,
                            c,
                        );
                        0
                    }
                };
                match inner_lv {
                    IrLValue::Signal(sid, _) => {
                        // Indeks deklarasi (`a[hi:lo] = …`) → POSISI bit fisik
                        // (rentang menaik `[0:3]` / lsb != 0 `[7:4]`).
                        let sig = &signals[sid];
                        let msb_p = index_to_bitpos(msb_c, sig.msb, sig.lsb);
                        let lsb_p = index_to_bitpos(lsb_c, sig.msb, sig.lsb);
                        Ok(IrLValue::RangeSelect(sid, msb_p, lsb_p))
                    }
                    IrLValue::RangeSelect(sid, outer_msb, outer_lsb) => {
                        let outer_start = if outer_msb > outer_lsb {
                            outer_lsb
                        } else {
                            outer_msb
                        };
                        let inner_start = outer_start + if msb_c > lsb_c { lsb_c } else { msb_c };
                        let inner_end = outer_start + if msb_c > lsb_c { msb_c } else { lsb_c };
                        Ok(IrLValue::RangeSelect(sid, inner_end, inner_start))
                    }
                    IrLValue::ArrayIndex {
                        sig_id,
                        index,
                        elem_width,
                    } => Ok(IrLValue::ArrayRangeSelect {
                        sig_id,
                        index,
                        elem_width,
                        msb: msb_c,
                        lsb: lsb_c,
                    }),
                    // Range select di atas bit select (`sig[a][b][msb:lsb]` —
                    // packed multidimensi setelah unroll). Akumulasi offset bit.
                    IrLValue::BitSelect(sid, base_bit) => {
                        let start = base_bit + msb_c.min(lsb_c);
                        let end = base_bit + msb_c.max(lsb_c);
                        Ok(IrLValue::RangeSelect(sid, end, start))
                    }
                    // Range-select di atas ArrayRangeSelect (`mem[i][15:8][2:0]`).
                    IrLValue::ArrayRangeSelect { sig_id, index, elem_width, msb, lsb } => {
                        let inner_min = msb.min(lsb);
                        let sub_min = msb_c.min(lsb_c);
                        let sub_max = msb_c.max(lsb_c);
                        Ok(IrLValue::ArrayRangeSelect {
                            sig_id,
                            index,
                            elem_width,
                            msb: inner_min + sub_max,
                            lsb: inner_min + sub_min,
                        })
                    }
                    // Range-select di atas ArrayBitSelect (`mem[i][j][2:0]`).
                    IrLValue::ArrayBitSelect { sig_id, index, elem_width, bit } => {
                        let sub_min = msb_c.min(lsb_c);
                        let sub_count = msb_c.max(lsb_c) - msb_c.min(lsb_c) + 1;
                        match *bit {
                            IrExpr::Const(lv) => {
                                let b = lv.to_u64() as usize;
                                Ok(IrLValue::ArrayRangeSelect {
                                    sig_id,
                                    index,
                                    elem_width,
                                    msb: b + msb_c.max(lsb_c),
                                    lsb: b + msb_c.min(lsb_c),
                                })
                            }
                            other => {
                                let elem_off = IrExpr::BinaryOp(
                                    BinaryIrOp::Mul,
                                    index.clone(),
                                    Box::new(IrExpr::Const(LogicVec::from_u64(elem_width as u64, 32))),
                                );
                                let base = IrExpr::BinaryOp(
                                    BinaryIrOp::Add,
                                    Box::new(elem_off),
                                    Box::new(other),
                                );
                                let new_base = IrExpr::BinaryOp(
                                    BinaryIrOp::Add,
                                    Box::new(base),
                                    Box::new(IrExpr::Const(LogicVec::from_u64(sub_min as u64, 32))),
                                );
                                Ok(IrLValue::ExprPartSelect {
                                    sig_id,
                                    base: Box::new(new_base),
                                    width: sub_count,
                                })
                            }
                        }
                    }
                    // Range-select di atas ExprPartSelect dinamis (`sig[b+:8][2:0]`).
                    IrLValue::ExprPartSelect { sig_id, base, width: _ } => {
                        let sub_min = msb_c.min(lsb_c);
                        let sub_count = msb_c.max(lsb_c) - msb_c.min(lsb_c) + 1;
                        let new_base = IrExpr::BinaryOp(
                            BinaryIrOp::Add,
                            base,
                            Box::new(IrExpr::Const(LogicVec::from_u64(sub_min as u64, 32))),
                        );
                        Ok(IrLValue::ExprPartSelect {
                            sig_id,
                            base: Box::new(new_base),
                            width: sub_count,
                        })
                    }
                    // Range-select di atas lvalue hierarkis (`u.data[3:0]`,
                    // `sif.sd_out[7:4]`): nama baru resolve saat write, jadi
                    // dekomposisi ke concat HierRefIndex (MSB-first) — lebar
                    // elemen (bit vs word) dihitung engine dari SignalInfo.
                    IrLValue::HierRef(name) => {
                        let start = msb_c.min(lsb_c);
                        let count = msb_c.max(lsb_c) - start + 1;
                        let mut parts: Vec<IrLValue> = Vec::with_capacity(count);
                        for k in (0..count).rev() {
                            parts.push(IrLValue::HierRefIndex {
                                name,
                                index: Box::new(IrExpr::Const(LogicVec::from_u64(
                                    (start + k) as u64,
                                    64,
                                ))),
                            });
                        }
                        Ok(IrLValue::Concat(parts))
                    }
                    IrLValue::HierRefIndex { name, index } => {
                        let start = msb_c.min(lsb_c);
                        let count = msb_c.max(lsb_c) - start + 1;
                        let mut parts: Vec<IrLValue> = Vec::with_capacity(count);
                        for k in (0..count).rev() {
                            let idx = if start + k == 0 {
                                (*index).clone()
                            } else {
                                IrExpr::BinaryOp(
                                    BinaryIrOp::Add,
                                    index.clone(),
                                    Box::new(IrExpr::Const(LogicVec::from_u64(
                                        (start + k) as u64,
                                        64,
                                    ))),
                                )
                            };
                            parts.push(IrLValue::HierRefIndex {
                                name,
                                index: Box::new(idx),
                            });
                        }
                        Ok(IrLValue::Concat(parts))
                    }
                    _ => {
                        // F45: nested range select tak ter-model (hier
                        // interface / array dinamis) — degrade warning + write
                        // no-op, bukan hard error yang memblokir modul.
                        let (l, c) = expr_location(expr);
                        self.elab_warn_at(
                            DiagCode::NotImplemented,
                            format!(
                                "nested range select lvalue tidak di-resolve statis (obj={:?}) — write diabaikan",
                                expr
                            ),
                            l,
                            c,
                        );
                        Ok(IrLValue::ObjectField {
                            sig_id: 0,
                            field: Symbol::intern("__nested_range_ignored"),
                        })
                    }
                }
            }
            Expr::BitSelect {
                expr: inner,
                index: bs_index,
            } => {
                let inner_lv = self.elaborate_lvalue(inner, signal_map, signals)?;
                match inner_lv {
IrLValue::VifField { .. } => Err(self.elab_diag_at(
                            DiagCode::NotImplemented,
                            "select pada field virtual interface tidak dapat di-resolve statis",
                            expr_location(expr).0,
                            expr_location(expr).1,
                        )),
                    IrLValue::Signal(sid, _) => {
                        let sig = &signals[sid];
                        // Check for multi-dim packed array: packed_dims.len() > 1
                        if sig.packed_dims.len() > 1 {
                            let outer_elem_width = sig.width / sig.packed_dims[0];
                            if let Ok(idx) = const_eval_params(bs_index, &self.param_vals) {
                                let idx = idx as usize;
                                let lsb = idx * outer_elem_width;
                                let msb = lsb + outer_elem_width - 1;
                                Ok(IrLValue::RangeSelect(sid, msb, lsb))
                            } else {
                                let index_expr =
                                    self.elaborate_expr(bs_index, signal_map, signals)?;
                                Ok(IrLValue::ArrayIndex {
                                    sig_id: sid,
                                    index: Box::new(index_expr),
                                    elem_width: outer_elem_width,
                                })
                            }
                        } else if sig.array_depth > 1
                            || sig.is_dynamic
                            || sig.is_queue
                            || sig.is_associative
                        {
                            // `$` = indeks terakhir (mirror jalur baca).
                            let index_expr = if Self::is_dollar_index(bs_index)
                                && (sig.is_dynamic || sig.is_queue)
                            {
                                Self::dollar_last_index(sid, sig.width)
                            } else {
                                self.elaborate_expr(bs_index, signal_map, signals)?
                            };
                            // F39: multi-dim unpacked — index pertama memilih
                            // ROW: lebar = elem_width × Π dims[1..] (bit offset
                            // i × row_w terbaca/tertulis utuh oleh engine).
                            let ew = if sig.array_dims.len() > 1 {
                                sig.array_dims[1..]
                                    .iter()
                                    .product::<usize>()
                                    .saturating_mul(sig.elem_width)
                            } else {
                                sig.elem_width
                            };
                            Ok(IrLValue::ArrayIndex {
                                sig_id: sid,
                                index: Box::new(index_expr),
                                elem_width: ew,
                            })
                        } else if sig.is_string {
                            // Tulis byte string `s[i] = v` (mirror jalur baca:
                            // engine kalikan index × elem_width sendiri).
                            if let Ok(idx) = const_eval_params(bs_index, &self.param_vals) {
                                let idx = idx.max(0) as usize;
                                let lsb = idx.saturating_mul(8);
                                Ok(IrLValue::RangeSelect(sid, lsb + 7, lsb))
                            } else {
                                let index_expr =
                                    self.elaborate_expr(bs_index, signal_map, signals)?;
                                Ok(IrLValue::ArrayIndex {
                                    sig_id: sid,
                                    index: Box::new(index_expr),
                                    elem_width: 8,
                                })
                            }
                        } else if let Ok(idx) = const_eval_params(bs_index, &self.param_vals) {
                            // Indeks deklarasi → posisi bit fisik (rentang
                            // menaik `[0:3]` / lsb != 0 `[7:4]`). Indeks negatif
                            // → usize besar → posisi di luar lebar → X.
                            let pos = index_to_bitpos(idx as usize, sig.msb, sig.lsb);
                            Ok(IrLValue::BitSelect(sid, pos))
                        } else {
                            // Index dinamis pada sinyal FLAT → bit-select runtime
                            // (mirror jalur baca). Bukan ArrayIndex: elem_width
                            // sinyal flat = lebar penuh → menulis selebar sinyal
                            // per index (offset i×width) → sinyal melebar/korup.
                            // Indeks deklarasi → posisi bit fisik (rentang menaik
                            // `[0:3]` / lsb != 0 `[7:4]`).
                            let index_expr = self.elaborate_expr(bs_index, signal_map, signals)?;
                            let base_expr = decl_index_to_pos_expr(index_expr, sig.msb, sig.lsb);
                            Ok(IrLValue::ExprPartSelect {
                                sig_id: sid,
                                base: Box::new(base_expr),
                                width: 1,
                            })
                        }
                    }
                    IrLValue::RangeSelect(sid, outer_msb, outer_lsb) => {
                        if let Ok(idx) = const_eval_params(bs_index, &self.param_vals) {
                            let base = if outer_msb > outer_lsb {
                                outer_lsb
                            } else {
                                outer_msb
                            };
                            Ok(IrLValue::BitSelect(sid, base + idx as usize))
                        } else {
                            let index_expr = self.elaborate_expr(bs_index, signal_map, signals)?;
                            Ok(IrLValue::ArrayIndex {
                                sig_id: sid,
                                index: Box::new(index_expr),
                                elem_width: outer_msb.max(outer_lsb) - outer_msb.min(outer_lsb) + 1,
                            })
                        }
                    }
                    IrLValue::ArrayIndex {
                        sig_id,
                        index,
                        elem_width,
                    } => {
                        // F39: index berantai pada array unpacked multi-dimensi
                        // `mat[i][j] = x` — fold ke satu ArrayIndex (index
                        // gabungan unit elemen, lihat jalur baca expr.rs).
                        let mut folded: Option<IrLValue> = None;
                        if let Some(sig) = signals.get(sig_id) {
                            let dims = &sig.array_dims;
                            let ew = sig.elem_width;
                            if dims.len() > 1 && elem_width > ew && ew > 0 {
                                let remaining = elem_width / ew;
                                let mut c = dims.len();
                                let mut prod = 1usize;
                                while c > 0 {
                                    c -= 1;
                                    prod *= dims[c];
                                    if prod == remaining {
                                        break;
                                    }
                                }
                                if prod == remaining && c < dims.len() {
                                    let new_ew = elem_width / dims[c];
                                    let idx_expr =
                                        self.elaborate_expr(bs_index, signal_map, signals)?;
                                    let combined = IrExpr::BinaryOp(
                                        BinaryIrOp::Add,
                                        Box::new(IrExpr::BinaryOp(
                                            BinaryIrOp::Mul,
                                            Box::new((*index).clone()),
                                            Box::new(IrExpr::Const(LogicVec::from_u64(
                                                dims[c] as u64,
                                                32,
                                            ))),
                                        )),
                                        Box::new(idx_expr),
                                    );
                                    folded = Some(IrLValue::ArrayIndex {
                                        sig_id,
                                        index: Box::new(combined),
                                        elem_width: new_ew,
                                    });
                                }
                            }
                        }
                        if let Some(lv) = folded {
                            return Ok(lv);
                        }
                        if let Ok(idx) = const_eval_params(bs_index, &self.param_vals) {
                            Ok(IrLValue::ArrayBitSelect {
                                sig_id,
                                index,
                                elem_width,
                                bit: Box::new(IrExpr::Const(LogicVec::from_u64(idx as u64, 64))),
                            })
                        } else {
                            // `arr[i][j]` dengan j runtime (packed multidimensi,
                            // mis. `seeds_q[seed_idx][rd_idx]` di
                            // flash_ctrl_lcmgr) — bit dievaluasi saat write di
                            // engine (offset = i * elem_width + j).
                            let bit_expr = self.elaborate_expr(bs_index, signal_map, signals)?;
                            Ok(IrLValue::ArrayBitSelect {
                                sig_id,
                                index,
                                elem_width,
                                bit: Box::new(bit_expr),
                            })
                        }
                    }
                    // Bit select di atas lvalue hierarkis (`sif.sd_out[i]`):
                    // nama belum ter-resolve di elaborator — simpan index,
                    // engine menghitung offset saat write dari SignalInfo.
                    IrLValue::HierRef(name) | IrLValue::HierRefIndex { name, .. } => {
                        let index_expr = self.elaborate_expr(bs_index, signal_map, signals)?;
                        Ok(IrLValue::HierRefIndex {
                            name,
                            index: Box::new(index_expr),
                        })
                    }
                    // Bit select di atas field object yang sudah ter-degrade
                    // (`resets_o.rst_por_io_div2_n[DomainMainSel]` di rstmgr —
                    // struct port tak ter-resolve → ObjectField no-op). Index
                    // di atas ObjectField tidak bisa di-offset statis; pertahan-
                    // kan degrade: warning + ObjectField (write tetap diabaikan
                    // engine) agar modul TIDAK di-skip oleh "nested bit select".
                    IrLValue::ObjectField { sig_id, field } => {
                        let (l, c) = expr_location(expr);
                        self.elab_warn_at(
                            DiagCode::NotImplemented,
                            format!(
                                "bit select pada field object '{}' yang tidak ter-resolve — write diabaikan",
                                field.as_str()
                            ),
                            l,
                            c,
                        );
                        Ok(IrLValue::ObjectField { sig_id, field })
                    }
                    // Bit select di atas bit select (`sig[a][b]` — packed
                    // multidimensi setelah unroll, mis. `result[0][0][0]`).
                    // Akumulasi offset bit.
                    IrLValue::BitSelect(sid, inner_bit) => {
                        if let Ok(idx) = const_eval_params(bs_index, &self.param_vals) {
                            Ok(IrLValue::BitSelect(sid, inner_bit + idx as usize))
                        } else {
                            let index_expr = self.elaborate_expr(bs_index, signal_map, signals)?;
                            Ok(IrLValue::ArrayBitSelect {
                                sig_id: sid,
                                index: Box::new(index_expr),
                                elem_width: 1,
                                bit: Box::new(IrExpr::Const(LogicVec::from_u64(
                                    inner_bit as u64,
                                    64,
                                ))),
                            })
                        }
                    }
                    // Bit-select di atas ArrayRangeSelect (`mem[i][7:0][k]`):
                    // `k` relatif thd base elemen slice (offset = min(msb,lsb)).
                    IrLValue::ArrayRangeSelect { sig_id, index, elem_width, msb, lsb } => {
                        let inner_base = msb.min(lsb);
                        if let Ok(idx) = const_eval_params(bs_index, &self.param_vals) {
                            Ok(IrLValue::ArrayBitSelect {
                                sig_id,
                                index,
                                elem_width,
                                bit: Box::new(IrExpr::Const(LogicVec::from_u64(
                                    (inner_base + idx as usize) as u64,
                                    64,
                                ))),
                            })
                        } else {
                            let index_expr = self.elaborate_expr(bs_index, signal_map, signals)?;
                            let bit_expr = IrExpr::BinaryOp(
                                BinaryIrOp::Add,
                                Box::new(IrExpr::Const(LogicVec::from_u64(inner_base as u64, 64))),
                                Box::new(index_expr),
                            );
                            Ok(IrLValue::ArrayBitSelect {
                                sig_id,
                                index,
                                elem_width,
                                bit: Box::new(bit_expr),
                            })
                        }
                    }
                    // Bit-select di atas ArrayBitSelect (`arr[i][j][k]`):
                    // akumulasi offset bit dalam elemen.
                    IrLValue::ArrayBitSelect { sig_id, index, elem_width, bit } => {
                        if let Ok(idx) = const_eval_params(bs_index, &self.param_vals) {
                            let new_bit = match *bit {
                                IrExpr::Const(lv) => IrExpr::Const(LogicVec::from_u64(
                                    lv.to_u64().wrapping_add(idx as u64),
                                    64,
                                )),
                                other => IrExpr::BinaryOp(
                                    BinaryIrOp::Add,
                                    Box::new(other),
                                    Box::new(IrExpr::Const(LogicVec::from_u64(idx as u64, 64))),
                                ),
                            };
                            Ok(IrLValue::ArrayBitSelect {
                                sig_id,
                                index,
                                elem_width,
                                bit: Box::new(new_bit),
                            })
                        } else {
                            let index_expr = self.elaborate_expr(bs_index, signal_map, signals)?;
                            let new_bit = IrExpr::BinaryOp(BinaryIrOp::Add, bit, Box::new(index_expr));
                            Ok(IrLValue::ArrayBitSelect {
                                sig_id,
                                index,
                                elem_width,
                                bit: Box::new(new_bit),
                            })
                        }
                    }
                    // Bit-select di atas ExprPartSelect dinamis (`sig[base+:w][k]`).
                    IrLValue::ExprPartSelect { sig_id, base, width: _ } => {
                        if let Ok(idx) = const_eval_params(bs_index, &self.param_vals) {
                            let new_base = IrExpr::BinaryOp(
                                BinaryIrOp::Add,
                                base,
                                Box::new(IrExpr::Const(LogicVec::from_u64(idx as u64, 64))),
                            );
                            Ok(IrLValue::ExprPartSelect {
                                sig_id,
                                base: Box::new(new_base),
                                width: 1,
                            })
                        } else {
                            let index_expr = self.elaborate_expr(bs_index, signal_map, signals)?;
                            let new_base = IrExpr::BinaryOp(BinaryIrOp::Add, base, Box::new(index_expr));
                            Ok(IrLValue::ExprPartSelect {
                                sig_id,
                                base: Box::new(new_base),
                                width: 1,
                            })
                        }
                    }
                    // Bit-select di atas concat lvalue (`{a,b}[k]`): decompose
                    // statis ke bit fisik pada part yang memuat k (dari LSB).
                    IrLValue::Concat(parts) => {
                        if let Ok(idx) = const_eval_params(bs_index, &self.param_vals) {
                            let lv = IrLValue::Concat(parts);
                            match select_lv_bit(&lv, idx as usize, signals) {
                                Some(sel) => Ok(sel),
                                None => Err(self.elab_diag_at(
                                    DiagCode::NotImplemented,
                                    "concat bit-select index out of range",
                                    expr_location(expr).0,
                                    expr_location(expr).1,
                                )),
                            }
                        } else {
                            Err(self.elab_diag_at(
                                DiagCode::NotImplemented,
                                "dynamic bit-select on concat lvalue",
                                expr_location(expr).0,
                                expr_location(expr).1,
                            ))
                        }
                    }
                }
            }
            Expr::PartSelect {
                expr: inner,
                base,
                width,
            } => {
                let inner_lv = self.elaborate_lvalue(inner, signal_map, signals)?;
                let base_r = const_eval_params(base, &self.param_vals);
                let width_r = const_eval_params(width, &self.param_vals);
                match (base_r, width_r) {
                    (Ok(b), Ok(w)) => {
                        let (base_c, width_c) = (b as usize, w as usize);
                        match inner_lv {
IrLValue::VifField { .. } => Err(self.elab_diag_at(
                            DiagCode::NotImplemented,
                            "select pada field virtual interface tidak dapat di-resolve statis",
                            expr_location(expr).0,
                            expr_location(expr).1,
                        )),
                            IrLValue::Signal(sid, _) => {
                                let sig = &signals[sid];
                                if width_c > 0 {
                                    // `sig[base +: width] = …` menulis indeks
                                    // base..base+width-1. MSB = indeks pertama
                                    // dalam urutan deklarasi (menurun →
                                    // base+width-1, menaik → base). Konversi
                                    // indeks → posisi bit fisik (lsb != 0 /
                                    // rentang menaik).
                                    let hi_idx = base_c + width_c - 1;
                                    let (msb_idx, lsb_idx) = if sig.msb >= sig.lsb {
                                        (hi_idx, base_c)
                                    } else {
                                        (base_c, hi_idx)
                                    };
                                    let pos_msb = index_to_bitpos(msb_idx, sig.msb, sig.lsb);
                                    let pos_lsb = index_to_bitpos(lsb_idx, sig.msb, sig.lsb);
                                    Ok(IrLValue::RangeSelect(sid, pos_msb, pos_lsb))
                                } else {
                                    let pos = index_to_bitpos(base_c, sig.msb, sig.lsb);
                                    Ok(IrLValue::RangeSelect(sid, pos, pos))
                                }
                            }
                            IrLValue::RangeSelect(sid, outer_msb, outer_lsb) => {
                                let outer_base = if outer_msb > outer_lsb {
                                    outer_lsb
                                } else {
                                    outer_msb
                                };
                                let new_base = outer_base + base_c;
                                if width_c > 0 {
                                    Ok(IrLValue::RangeSelect(sid, new_base + width_c - 1, new_base))
                                } else {
                                    Ok(IrLValue::RangeSelect(sid, new_base, new_base))
                                }
                            }
                            IrLValue::ArrayIndex {
                                sig_id,
                                index,
                                elem_width,
                            } => {
                                if width_c > 0 {
                                    Ok(IrLValue::ArrayRangeSelect {
                                        sig_id,
                                        index,
                                        elem_width,
                                        msb: base_c + width_c - 1,
                                        lsb: base_c,
                                    })
                                } else {
                                    Ok(IrLValue::ArrayRangeSelect {
                                        sig_id,
                                        index,
                                        elem_width,
                                        msb: base_c,
                                        lsb: base_c,
                                    })
                                }
                            }
                            // Part-select di atas BitSelect (`sig[p][base+:w]`).
                            IrLValue::BitSelect(sid, p) => {
                                let hi = base_c + width_c - 1;
                                Ok(IrLValue::RangeSelect(sid, p + hi, p + base_c))
                            }
                            // Part-select di atas ArrayBitSelect (`arr[i][j][base+:w]`).
                            // physical = idx*elem_width + bit + base_c .. + (w-1).
                            IrLValue::ArrayBitSelect { sig_id, index, elem_width, bit } => {
                                let elem_off = IrExpr::BinaryOp(
                                    BinaryIrOp::Mul,
                                    index.clone(),
                                    Box::new(IrExpr::Const(LogicVec::from_u64(elem_width as u64, 32))),
                                );
                                let total = IrExpr::BinaryOp(
                                    BinaryIrOp::Add,
                                    Box::new(IrExpr::BinaryOp(
                                        BinaryIrOp::Add,
                                        Box::new(elem_off),
                                        bit,
                                    )),
                                    Box::new(IrExpr::Const(LogicVec::from_u64(base_c as u64, 32))),
                                );
                                Ok(IrLValue::ExprPartSelect {
                                    sig_id,
                                    base: Box::new(total),
                                    width: width_c,
                                })
                            }
                            // Part-select di atas ArrayRangeSelect (`mem[i][7:0][base+:w]`).
                            IrLValue::ArrayRangeSelect { sig_id, index, elem_width, msb, lsb } => {
                                let inner_base = msb.min(lsb);
                                let hi = base_c + width_c - 1;
                                Ok(IrLValue::ArrayRangeSelect {
                                    sig_id,
                                    index,
                                    elem_width,
                                    msb: inner_base + hi,
                                    lsb: inner_base + base_c,
                                })
                            }
                            // Part-select di atas ExprPartSelect (`sig[b+:w2][base+:w]`).
                            IrLValue::ExprPartSelect { sig_id, base, width: _ } => {
                                let new_base = IrExpr::BinaryOp(
                                    BinaryIrOp::Add,
                                    base,
                                    Box::new(IrExpr::Const(LogicVec::from_u64(base_c as u64, 64))),
                                );
                                Ok(IrLValue::ExprPartSelect {
                                    sig_id,
                                    base: Box::new(new_base),
                                    width: width_c,
                                })
                            }
                            // Part-select di atas concat lvalue (`{a,b}[base+:w]`):
                            // decompose ke sub-range per part.
                            IrLValue::Concat(parts) => {
                                let lv = IrLValue::Concat(parts);
                                match select_lv_range(&lv, base_c, width_c, signals) {
                                    Some(sel) => Ok(sel),
                                    None => Err(self.elab_diag_at(
                                        DiagCode::NotImplemented,
                                        "concat part-select out of range",
                                        expr_location(expr).0,
                                        expr_location(expr).1,
                                    )),
                                }
                            }
                            IrLValue::ObjectField { sig_id, field } => {
                                let (l, c) = expr_location(expr);
                                self.elab_warn_at(
                                    DiagCode::NotImplemented,
                                    format!(
                                        "part-select pada field object '{}' yang tidak ter-resolve — write diabaikan",
                                        field.as_str()
                                    ),
                                    l,
                                    c,
                                );
                                Ok(IrLValue::ObjectField { sig_id, field })
                            }
                            IrLValue::HierRef(name) | IrLValue::HierRefIndex { name, .. } => {
                                // Part-select pada referensi hierarkis. width == 1
                                // tetap memakai HierRefIndex (bit/word select);
                                // width > 1 didekomposisi menjadi concat lvalue
                                // MSB-first — tiap elemen memakai HierRefIndex
                                // sehingga lebar elemen (bit vs word array)
                                // di-resolve engine dari SignalInfo.
                                if width_c == 1 {
                                    let index_expr =
                                        self.elaborate_expr(base, signal_map, signals)?;
                                    Ok(IrLValue::HierRefIndex {
                                        name,
                                        index: Box::new(index_expr),
                                    })
                                } else {
                                    let base_ir =
                                        self.elaborate_expr(base, signal_map, signals)?;
                                    let mut parts: Vec<IrLValue> =
                                        Vec::with_capacity(width_c);
                                    for k in (0..width_c).rev() {
                                        let idx = if k == 0 {
                                            base_ir.clone()
                                        } else {
                                            IrExpr::BinaryOp(
                                                BinaryIrOp::Add,
                                                Box::new(base_ir.clone()),
                                                Box::new(IrExpr::Const(
                                                    LogicVec::from_u64(k as u64, 64),
                                                )),
                                            )
                                        };
                                        parts.push(IrLValue::HierRefIndex {
                                            name,
                                            index: Box::new(idx),
                                        });
                                    }
                                    Ok(IrLValue::Concat(parts))
                                }
                            }
                        }
                    }
                    _ => {
                        // `sig[base +: width]` dengan base runtime. Simpan base
                        // sebagai ekspresi (dievaluasi saat write) dan width yang
                        // di-resolve best-effort (param/konstan biasanya).
                        let base_ir = self.elaborate_expr(base, signal_map, signals)?;
                        let width_c: usize = match const_eval_params(width, &self.param_vals) {
                            Ok(w) => (w.max(1)) as usize,
                            Err(_) => {
                                let _ = self.elaborate_expr(width, signal_map, signals);
                                // IrLValue::ExprPartSelect.width statis (tak ada
                                // varian dinamis seperti rvalue) — width sinyal
                                // hanya bisa ditebak 1. Warning eksplisit agar
                                // tak silent-wrong (pola buglog #33).
                                let (wl, wc) = expr_location(width);
                                self.elab_warn_at(
                                    DiagCode::NotImplemented,
                                    "lebar part-select pada lvalue tidak konstan — \
                                     dipakai 1 (tulis IR tak mendukung width dinamis)",
                                    wl,
                                    wc,
                                );
                                compute_expr_width(
                                    width,
                                    signal_map,
                                    signals,
                                    &self.param_vals,
                                    &self.package_symbols,
                                )
                                .unwrap_or(1)
                                .max(1)
                            }
                        };
                        match inner_lv {
IrLValue::VifField { .. } => Err(self.elab_diag_at(
                            DiagCode::NotImplemented,
                            "select pada field virtual interface tidak dapat di-resolve statis",
                            expr_location(expr).0,
                            expr_location(expr).1,
                        )),
                            IrLValue::Signal(sid, _) => Ok(IrLValue::ExprPartSelect {
                                sig_id: sid,
                                base: Box::new(base_ir),
                                width: width_c,
                            }),
                            IrLValue::RangeSelect(sid, outer_msb, outer_lsb) => {
                                let offset = outer_msb.min(outer_lsb);
                                let base_adj = IrExpr::BinaryOp(
                                    BinaryIrOp::Add,
                                    Box::new(base_ir),
                                    Box::new(IrExpr::Const(LogicVec::from_u64(offset as u64, 32))),
                                );
                                Ok(IrLValue::ExprPartSelect {
                                    sig_id: sid,
                                    base: Box::new(base_adj),
                                    width: width_c,
                                })
                            }
                            IrLValue::ArrayIndex {
                                sig_id,
                                index,
                                elem_width,
                            } => {
                                let idx_expr = IrExpr::BinaryOp(
                                    BinaryIrOp::Mul,
                                    index,
                                    Box::new(IrExpr::Const(LogicVec::from_u64(
                                        elem_width as u64,
                                        32,
                                    ))),
                                );
                                let base_adj = IrExpr::BinaryOp(
                                    BinaryIrOp::Add,
                                    Box::new(base_ir),
                                    Box::new(idx_expr),
                                );
                                Ok(IrLValue::ExprPartSelect {
                                    sig_id,
                                    base: Box::new(base_adj),
                                    width: width_c,
                                })
                            }
                            // Lvalue hierarkis / field object yang sudah ter-degrade
                            // Lvalue hierarkis dengan base DINAMIS
                            // (`u.data[base +: 4]`): dekomposisi ke concat
                            // HierRefIndex, offset dievaluasi saat write.
                            IrLValue::HierRef(name) => {
                                let mut parts: Vec<IrLValue> =
                                    Vec::with_capacity(width_c);
                                for k in (0..width_c).rev() {
                                    let idx = if k == 0 {
                                        base_ir.clone()
                                    } else {
                                        IrExpr::BinaryOp(
                                            BinaryIrOp::Add,
                                            Box::new(base_ir.clone()),
                                            Box::new(IrExpr::Const(LogicVec::from_u64(
                                                k as u64,
                                                64,
                                            ))),
                                        )
                                    };
                                    parts.push(IrLValue::HierRefIndex {
                                        name,
                                        index: Box::new(idx),
                                    });
                                }
                                Ok(IrLValue::Concat(parts))
                            }
                            IrLValue::HierRefIndex { name, index } => {
                                let mut parts: Vec<IrLValue> =
                                    Vec::with_capacity(width_c);
                                for k in (0..width_c).rev() {
                                    let idx = if k == 0 {
                                        (*index).clone()
                                    } else {
                                        IrExpr::BinaryOp(
                                            BinaryIrOp::Add,
                                            index.clone(),
                                            Box::new(IrExpr::Const(LogicVec::from_u64(
                                                k as u64,
                                                64,
                                            ))),
                                        )
                                    };
                                    parts.push(IrLValue::HierRefIndex {
                                        name,
                                        index: Box::new(idx),
                                    });
                                }
                                Ok(IrLValue::Concat(parts))
                            }
                            IrLValue::ObjectField { sig_id, field } => {
                                let (l, c) = expr_location(expr);
                                self.elab_warn_at(
                                    DiagCode::NotImplemented,
                                    format!(
                                        "dynamic part-select pada field object '{}' — write diabaikan",
                                        field.as_str()
                                    ),
                                    l,
                                    c,
                                );
                                Ok(IrLValue::ObjectField { sig_id, field })
                            }
                            // Part-select dinamis di atas BitSelect (`sig[p][base+:w]`, base runtime).
                            IrLValue::BitSelect(sid, p) => {
                                let off = IrExpr::Const(LogicVec::from_u64(p as u64, 32));
                                let base_adj = IrExpr::BinaryOp(
                                    BinaryIrOp::Add,
                                    Box::new(base_ir),
                                    Box::new(off),
                                );
                                Ok(IrLValue::ExprPartSelect {
                                    sig_id: sid,
                                    base: Box::new(base_adj),
                                    width: width_c,
                                })
                            }
                            // Part-select dinamis di atas ArrayBitSelect (`arr[i][j][base+:w]`).
                            IrLValue::ArrayBitSelect { sig_id, index, elem_width, bit } => {
                                let elem_off = IrExpr::BinaryOp(
                                    BinaryIrOp::Mul,
                                    index.clone(),
                                    Box::new(IrExpr::Const(LogicVec::from_u64(elem_width as u64, 32))),
                                );
                                let bit_off = match *bit {
                                    IrExpr::Const(lv) => IrExpr::Const(LogicVec::from_u64(lv.to_u64(), 32)),
                                    other => other,
                                };
                                let total = IrExpr::BinaryOp(
                                    BinaryIrOp::Add,
                                    Box::new(IrExpr::BinaryOp(
                                        BinaryIrOp::Add,
                                        Box::new(elem_off),
                                        Box::new(bit_off),
                                    )),
                                    Box::new(base_ir),
                                );
                                Ok(IrLValue::ExprPartSelect {
                                    sig_id,
                                    base: Box::new(total),
                                    width: width_c,
                                })
                            }
                            // Part-select dinamis di atas ArrayRangeSelect.
                            IrLValue::ArrayRangeSelect { sig_id, index, elem_width, msb, lsb } => {
                                let inner_base = msb.min(lsb) as u64;
                                let elem_off = IrExpr::BinaryOp(
                                    BinaryIrOp::Mul,
                                    index.clone(),
                                    Box::new(IrExpr::Const(LogicVec::from_u64(elem_width as u64, 32))),
                                );
                                let total = IrExpr::BinaryOp(
                                    BinaryIrOp::Add,
                                    Box::new(IrExpr::BinaryOp(
                                        BinaryIrOp::Add,
                                        Box::new(elem_off),
                                        Box::new(IrExpr::Const(LogicVec::from_u64(inner_base, 32))),
                                    )),
                                    Box::new(base_ir),
                                );
                                Ok(IrLValue::ExprPartSelect {
                                    sig_id,
                                    base: Box::new(total),
                                    width: width_c,
                                })
                            }
                            // Part-select dinamis di atas ExprPartSelect.
                            IrLValue::ExprPartSelect { sig_id, base, width: _ } => {
                                let total = IrExpr::BinaryOp(
                                    BinaryIrOp::Add,
                                    base,
                                    Box::new(base_ir),
                                );
                                Ok(IrLValue::ExprPartSelect {
                                    sig_id,
                                    base: Box::new(total),
                                    width: width_c,
                                })
                            }
                            IrLValue::Concat(_) => Err(self.elab_diag_at(
                                DiagCode::NotImplemented,
                                "dynamic part-select on concat lvalue requires constant base",
                                expr_location(expr).0,
                                expr_location(expr).1,
                            )),
                        }
                    }
                }
            }
            Expr::Concat(exprs) => {
                let parts: Result<Vec<IrLValue>, SimError> = exprs
                    .iter()
                    .map(|e| self.elaborate_lvalue(e, signal_map, signals))
                    .collect();
                Ok(IrLValue::Concat(parts?))
            }
            Expr::MethodCall { .. } => Err(self.elab_diag_at(
                DiagCode::NotImplemented,
                "method calls cannot be used as lvalues",
                expr_location(expr).0,
                expr_location(expr).1,
            )),
            Expr::MemberAccess { obj, field } => {
                // Try struct/union field write
                let hier_name = Self::build_hier_name(obj, field.as_str());
                if std::env::var("MIVON_DBG_HIER").is_ok() && !hier_name.is_empty() {
                    let in_sigmap = signal_map.contains_key(&Symbol::intern(&hier_name));
                    let in_signals = signals.iter().any(|s| s.name.as_str() == hier_name);
                    eprintln!(
                        "[DBG-HIER] lvalue hier_name='{}' sigmap={} signals={} obj={:?}",
                        hier_name, in_sigmap, in_signals, obj
                    );
                }
                if let Some(&sig_id) = signal_map.get(&Symbol::intern(&hier_name)) {
                    return Ok(IrLValue::Signal(sig_id, 0));
                }
                // F27: port interface (`bus_if b` — iface_type di-set, class_name
                // None) → field write lewat hier path (`b.data = x`). Dikompil
                // sebagai IrLValue::HierRef agar engine menulis ke signal flatten
                // instance interface yang sama dengan tb (via hier_signal_map).
                if let Some((base_name, _)) =
                    Self::collect_member_chain(obj, *field, &self.param_vals)
                {
                    if let Some(&base_sid) = signal_map.get(&Symbol::intern(&base_name)) {
                        let base_info = &signals[base_sid];
                        if base_info.iface_type.is_some()
                            && base_info.class_name.is_none()
                            && !hier_name.is_empty()
                        {
                            return Ok(IrLValue::HierRef(Symbol::intern(&hier_name)));
                        }
                    }
                }
                // Nested member access lvalue (`hw2reg.val.d = x`): kumpulkan
                // chain field (dari luar ke dalam), resolve base signal, lalu
                // akumulasi offset berjenjang via struct_fields + typedef_field_map.
                if let Some((base_name, chain)) =
                    Self::collect_member_chain(obj, *field, &self.param_vals)
                {
                    if let Some(&base_sid) = signal_map.get(&Symbol::intern(&base_name)) {
                        let base_info = &signals[base_sid];
                        if !base_info.struct_fields.is_empty() {
                            if let Some((msb, lsb)) =
                                self.resolve_struct_chain(base_sid, &chain, signals)
                            {
                                return Ok(IrLValue::RangeSelect(base_sid, msb, lsb));
                            }
                        }
                    }
                }
                match self.elaborate_expr(obj, signal_map, signals) {
                    Ok(IrExpr::Signal(sig_id, _)) => {
                        let sig_info = &signals[sig_id];
                        // LANG-44: VARIABLE virtual interface (bukan port
                        // interface) - vif.field ditulis lewat binding runtime.
                        if sig_info.iface_type.is_some()
                            && sig_info.class_name == sig_info.iface_type
                            && !matches!(
                                sig_info.kind,
                                mivon_ir::SignalKind::Input
                                    | mivon_ir::SignalKind::Output
                                    | mivon_ir::SignalKind::Inout
                            )
                        {
                            return Ok(IrLValue::VifField {
                                vif: sig_info.name,
                                field: *field,
                                width: 0,
                            });
                        }
                        if !sig_info.struct_fields.is_empty() {
                            if let Some(f) =
                                sig_info.struct_fields.iter().find(|f| f.name == *field)
                            {
                                let lsb = f.offset;
                                let msb = f.offset + f.width - 1;
                                return Ok(IrLValue::RangeSelect(sig_id, lsb, msb));
                            }
                            // Field tidak ditemukan — mungkin struct dari package yang
                            // belum fully resolved. Emit warning dan fallback ke
                            // ObjectField agar elaborasi tidak gagal total.
                            self.elab_warn_at(
                                DiagCode::ModuleNotFound,
                                format!("field '{}' not found in struct type", field),
                                expr_location(expr).0,
                                expr_location(expr).1,
                            );
                            return Ok(IrLValue::ObjectField {
                                sig_id,
                                field: *field,
                            });
                        }
                        // Class object handle: obj = signal berisi obj id → field.
                        if sig_info.class_name.is_some() {
                            return Ok(IrLValue::ObjectField {
                                sig_id,
                                field: *field,
                            });
                        }
                        // Signal NON-struct dijadikan target member access.
                        // Port SKALAR + field access tak pernah bisa resolve
                        // (flatten tak menciptakan `port.field` dari scalar —
                        // temuan fuzz bug_0395 `register_o.hartid`): elab ERROR
                        // ber-lokasi di sini, bukan warning + runtime error
                        // tanpa lokasi. Interface (iface_type) tetap defer
                        // warning+HierRef (resolve post-flatten legitimate).
                        if sig_info.iface_type.is_none()
                            && matches!(
                                sig_info.kind,
                                SignalKind::Input | SignalKind::Output | SignalKind::Inout
                            )
                        {
                            let (l, c) = expr_location(expr);
                            return Err(self.elab_diag_at(
                                DiagCode::TypeMismatch,
                                format!(
                                    "field '{}' tidak ada pada port scalar '{}' (tipe non-struct)",
                                    field.as_str(),
                                    sig_info.name.as_str()
                                ),
                                l,
                                c,
                            ));
                        }
                        // Biasanya interface instance (`jtag_mst.tdo`, `tif.miso`)
                        // atau sinyal testbench yang field-nya tidak ter-model.
                        // Degrade: warning + HierRef (engine resolve nama saat
                        // write) bila nama hierarkis tersedia, selain itu
                        // ObjectField (no-op aman di engine untuk non-object).
                        self.elab_warn_at(
                            DiagCode::ModuleNotFound,
                            format!(
                                "member access pada signal non-struct '{:?}.{}' — write tidak penuh (interface/hier fallback)",
                                obj,
                                field.as_str()
                            ),
                            expr_location(expr).0,
                            expr_location(expr).1,
                        );
                        let hn = Self::build_hier_name(obj, field.as_str());
                        if !hn.is_empty() {
                            return Ok(IrLValue::HierRef(Symbol::intern(&hn)));
                        }
                        Ok(IrLValue::ObjectField {
                            sig_id,
                            field: *field,
                        })
                    }
                    // obj TIDAK ter-resolve sebagai signal: instance interface
                    // (`sif.csb`) atau path instance (`u_dut.u_padring.cio_*`).
                    // Statement module di-elaborate SEBELUM flatten_instances,
                    // jadi nama hierarkis belum ada di signal_map/signals saat
                    // itu. Simpan sebagai HierRef — engine resolve ke flattened
                    // signal list saat write (mekanisme sama dengan
                    // IrExpr::HierRef untuk read).
                    _ => {
                        let hier_name = Self::build_hier_name(obj, field.as_str());
                        if !hier_name.is_empty() {
                            let base_is_signal =
                                Self::collect_member_chain(obj, *field, &self.param_vals)
                                    .map(|(base, _)| {
                                        signal_map.contains_key(&Symbol::intern(&base))
                                    })
                                    .unwrap_or(false);
                            if !base_is_signal {
                                return Ok(IrLValue::HierRef(Symbol::intern(&hier_name)));
                            }
                        }
                        // Member access lvalue yang TIDAK bisa di-resolve statis
                        // (base index dinamis — mis. `key_slots_d[slot].key[j]`
                        // di keymgr_dpe, atau path instance). Degrade: warning +
                        // HierRef bila nama hierarkis tersedia, selain itu
                        // ObjectField (no-op aman) agar modul tidak di-skip.
                        let (l, c) = expr_location(expr);
                        self.elab_warn_at(
                            DiagCode::NotImplemented,
                            format!(
                                "member access lvalue tidak dapat di-resolve statis (obj={:?}.{}) — write diabaikan",
                                obj,
                                field.as_str()
                            ),
                            l,
                            c,
                        );
                        let hn = Self::build_hier_name(obj, field.as_str());
                        if !hn.is_empty() {
                            return Ok(IrLValue::HierRef(Symbol::intern(&hn)));
                        }
                        if let Some((base_name, _)) =
                            Self::collect_member_chain(obj, *field, &self.param_vals)
                        {
                            if let Some(&base_sid) = signal_map.get(&Symbol::intern(&base_name)) {
                                return Ok(IrLValue::ObjectField {
                                    sig_id: base_sid,
                                    field: *field,
                                });
                            }
                        }
                        Ok(IrLValue::ObjectField {
                            sig_id: 0,
                            field: *field,
                        })
                    }
                }
            }
            _ => {
                if std::env::var("MIVON_DBG_LVALUE").is_ok() {
                    eprintln!(
                        "[DBG-LVALUE] invalid lvalue expr at {}:{}: {:?}",
                        expr_location(expr).0,
                        expr_location(expr).1,
                        expr
                    );
                }
                Err(self.elab_diag_at(
                    DiagCode::InvalidSyntax,
                    format!("invalid lvalue expression: {}", describe_lvalue_expr(expr)),
                    expr_location(expr).0,
                    expr_location(expr).1,
                ))
            }
        }
    }

    /// Kumpulkan chain member access `a.b.c` → (nama signal base, urutan step
    /// dari luar ke dalam). Contoh: `hw2reg.val.d` → ("hw2reg", [val, d]);
    /// `reg2hw.key_share0[0].qe` → ("reg2hw", [key_share0, Index(0), qe]).
    /// Index konstanta (genvar sudah di-substitute saat generate expansion),
    /// dievaluasi via `const_eval_with_params` agar ekspresi seperti `31-i`
    /// (sudah menjadi `31-0`, `31-1`, …) ikut ter-fold.
    pub(crate) fn collect_member_chain(
        obj: &Expr,
        leaf_field: Symbol,
        param_vals: &std::collections::HashMap<Symbol, i64>,
    ) -> Option<(String, Vec<ChainStep>)> {
        let mut chain = vec![ChainStep::Field(leaf_field)];
        let mut cur = obj;
        loop {
            match cur {
                Expr::MemberAccess { obj: inner, field } => {
                    chain.push(ChainStep::Field(*field));
                    cur = inner;
                }
                Expr::BitSelect { expr: inner, index } => {
                    if let Ok(idx) = const_eval_with_params(index, param_vals) {
                        chain.push(ChainStep::Index(idx));
                        cur = inner;
                    } else {
                        return None;
                    }
                }
                Expr::Ident { name, .. } => {
                    let base = name.as_str().to_string();
                    chain.reverse(); // [base_step, ..., leaf_step]
                    return Some((base, chain));
                }
                _ => return None,
            }
        }
    }

    /// Resolve chain member access struct (`a.b.c.d`) menjadi offset [lsb..msb]
    /// terhadap signal dasar `base_sid`. Memakai struct_fields base signal +
    /// typedef_field_map untuk step berjenjang. Mengembalikan (msb, lsb) bila
    /// seluruh step ter-resolve; None bila tidak (fallback jalur HierRef/
    /// MemberAccess runtime).
    pub(crate) fn resolve_struct_chain(
        &self,
        base_sid: SignalId,
        chain: &[ChainStep],
        signals: &[SignalInfo],
    ) -> Option<(usize, usize)> {
        let base_info = &signals[base_sid];
        if base_info.struct_fields.is_empty() {
            return None;
        }
        let mut offset = 0usize;
        let mut width = 1usize;
        let mut cur_fields: Option<Vec<StructFieldInfo>> = Some(base_info.struct_fields.clone());
        let mut last_field: Option<StructFieldInfo> = None;
        let mut ok = true;
        for (i, step) in chain.iter().enumerate() {
            match step {
                ChainStep::Index(idx) => {
                    let elem_width = if let Some(f) = &last_field {
                        f.type_name
                            .as_ref()
                            .and_then(|tn| self.lookup_struct_fields(tn.as_str()))
                            .map(|fs| fs.iter().map(|sf| sf.width).sum::<usize>().max(1))
                            .unwrap_or(1)
                    } else {
                        base_info.elem_width.max(1)
                    };
                    offset = offset.saturating_add((*idx as usize).saturating_mul(elem_width));
                    if let Some(f) = &last_field {
                        if let Some(tn) = &f.type_name {
                            cur_fields = self.lookup_struct_fields(tn.as_str());
                        }
                    }
                    last_field = None;
                }
                ChainStep::Field(fname) => {
                    let fields = match &cur_fields {
                        Some(fs) => fs.clone(),
                        None => {
                            ok = false;
                            break;
                        }
                    };
                    let found = fields.iter().find(|f| f.name == *fname);
                    match found {
                        Some(f) => {
                            offset += f.offset;
                            width = f.width;
                            last_field = Some(f.clone());
                            if i + 1 < chain.len() {
                                let mut nxt = f
                                    .type_name
                                    .as_ref()
                                    .and_then(|tn| self.lookup_struct_fields(tn.as_str()));
                                if nxt.is_none() {
                                    nxt = Some(f.sub_fields.clone());
                                }
                                cur_fields = nxt;
                            }
                        }
                        None => {
                            ok = false;
                            break;
                        }
                    }
                }
            }
        }
        if ok {
            Some((offset + width - 1, offset))
        } else {
            None
        }
    }

    /// Apakah field TERAKHIR dari chain struct bertipe signed (`int a;`)?
    /// Dipakai pembaca member agar hasilnya signed (LRM 1800 §6.24.1).
    pub(crate) fn struct_chain_leaf_signed(
        &self,
        base_sid: SignalId,
        chain: &[ChainStep],
        signals: &[SignalInfo],
    ) -> bool {
        let base_info = &signals[base_sid];
        let mut cur_fields: Option<Vec<StructFieldInfo>> = if base_info.struct_fields.is_empty() {
            None
        } else {
            Some(base_info.struct_fields.clone())
        };
        let mut signed = false;
        for (i, step) in chain.iter().enumerate() {
            match step {
                ChainStep::Field(fname) => {
                    let Some(fields) = cur_fields.as_ref() else {
                        return false;
                    };
                    let Some(f) = fields.iter().find(|f| f.name == *fname) else {
                        return false;
                    };
                    if i + 1 == chain.len() {
                        return f.is_signed;
                    }
                    let mut nxt = f
                        .type_name
                        .as_ref()
                        .and_then(|tn| self.lookup_struct_fields(tn.as_str()));
                    if nxt.is_none() {
                        nxt = Some(f.sub_fields.clone());
                    }
                    cur_fields = nxt;
                }
                // Elemen array struct: signedness mengikuti field induk.
                ChainStep::Index(_) => {
                    let Some(fields) = cur_fields.as_ref() else {
                        return false;
                    };
                    signed = fields.iter().any(|f| f.is_signed);
                    cur_fields = None;
                }
            }
        }
        signed
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn ident(name: &str) -> Expr {
        Expr::Ident {
            name: Symbol::intern(name),
            line: 0,
            col: 0,
        }
    }

    /// `collect_member_chain` memetakan objek bertingkat ke (nama base, step
    /// dari luar ke dalam) — inti resolusi lvalue struct/array yang dipindah
    /// ke modul ini.
    #[test]
    fn member_chain_maps_nested_field_and_index() {
        // hw2reg.val.d
        let obj = Expr::MemberAccess {
            obj: Box::new(ident("hw2reg")),
            field: Symbol::intern("val"),
        };
        let (base, steps) =
            Elaborator::collect_member_chain(&obj, Symbol::intern("d"), &HashMap::new())
                .expect("chain field harus resolve");
        assert_eq!(base, "hw2reg");
        assert_eq!(steps.len(), 2);
        match (&steps[0], &steps[1]) {
            (ChainStep::Field(a), ChainStep::Field(b)) => {
                assert_eq!(a.as_str(), "val");
                assert_eq!(b.as_str(), "d");
            }
            other => panic!("urutan step salah: {other:?}"),
        }

        // reg2hw.key_share0[0].qe  (index konstanta ikut jadi ChainStep::Index)
        // Objek yang dioper adalah bagian SEBELUM leaf (tanpa ".qe").
        let obj = Expr::BitSelect {
            expr: Box::new(Expr::MemberAccess {
                obj: Box::new(ident("reg2hw")),
                field: Symbol::intern("key_share0"),
            }),
            index: Box::new(Expr::Value(Value::Decimal(0))),
        };
        let (base, steps) =
            Elaborator::collect_member_chain(&obj, Symbol::intern("qe"), &HashMap::new())
                .expect("chain index harus resolve");
        assert_eq!(base, "reg2hw");
        assert_eq!(steps.len(), 3);
        assert!(matches!(&steps[0], ChainStep::Field(f) if f.as_str() == "key_share0"));
        assert!(matches!(&steps[1], ChainStep::Index(0)));
        assert!(matches!(&steps[2], ChainStep::Field(f) if f.as_str() == "qe"));
    }
}
