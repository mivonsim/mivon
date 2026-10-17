//! Assignment pattern packing: `lhs = '{...}` (LRM 1800 §7.9/§10.9).
//!
//! Dua bentuk agregat di-pack ke `IrExpr` di level STATEMENT (saat LHS diketahui):
//!
//! - **Struct**: `p = '{hi: 4'hA, lo: 4'h5}` — layout `StructFieldInfo`
//!   (offset+width tiap field) di-pack `IrExpr::Concat` MSB-first.
//! - **Queue/dynamic array**: `q = '{a}` / `q = '{1,2,3}` — tiap elemen
//!   di-cast ke `elem_width`, digabung MSB-first (elemen 0 = bit terendah,
//!   sinkron storage queue engine yang menaruh elemen i di `i*elem_width`).
//!
//! Sebelumnya pola bernama di runtime di-evaluate jadi `FillLit(0)`
//! (lihat `elaborate_expr` arm `Expr::StructLit`) karena layout typedef tak
//! tersedia di level ekspresi. Akibatnya `p = '{hi:4'hA, lo:4'h5}` menghasilkan
//! `p = 0`, dan cast-to-queue `Q'(a)` → `'{ a }` menghasilkan lebar 1
//! (queue terbaca kosong/X) — silent wrong.
//!
//! 1 file = 1 tanggung jawab (packing assignment-pattern + call site statement).

use mivon_ast::expr::{Expr, StructLitMember};
use mivon_core::error::SimError;
use mivon_core::intern::Symbol;
use mivon_ir::{IrExpr, IrLValue, LogicVec, SignalId, SignalInfo, StructFieldInfo};
use std::collections::HashMap;

use super::Elaborator;

/// Batas rekursi struct bersarang di dalam pola (anti siklus typedef).
const MAX_DEPTH: usize = 16;

impl Elaborator {
    /// Kalau `rhs` adalah assignment pattern `'{...}` dan LHS-nya agregat
    /// (struct ber-layout, atau queue/dynamic array), pack menjadi `IrExpr`.
    /// `Ok(None)` = bukan pola atau layout tak diketahui → pemanggil jatuh ke
    /// `elaborate_expr` (perilaku lama).
    pub(crate) fn elaborate_struct_pattern(
        &self,
        ir_lhs: &IrLValue,
        rhs: &Expr,
        signal_map: &HashMap<Symbol, SignalId>,
        signals: &[SignalInfo],
    ) -> Result<Option<IrExpr>, SimError> {
        let IrLValue::Signal(sid, _) = ir_lhs else {
            return Ok(None);
        };
        let Some(sig) = signals.get(*sid) else {
            return Ok(None);
        };
        // Queue/dynamic array: assignment pattern `'{a,b,c}` di-desugar parser
        // menjadi `Expr::Concat` (semua posisional) atau tetap `StructLit`
        // (ada named/default). Keduanya = daftar ELEMEN queue berurutan;
        // elemen 0 di bit terendah (storage engine `i*elem_width`).
        // Associative array punya storage HashMap → dilewati.
        if (sig.is_queue || sig.is_dynamic) && !sig.is_associative {
            let elems: Vec<&Expr> = match rhs {
                Expr::StructLit { members } => members
                    .iter()
                    .map(|m| match m {
                        StructLitMember::Positional(e)
                        | StructLitMember::Named(_, e)
                        | StructLitMember::Default(e) => e,
                    })
                    .collect(),
                Expr::Concat(es) => es.iter().collect(),
                _ => Vec::new(),
            };
            if !elems.is_empty() {
                return Ok(Some(self.pack_array_pattern(
                    &elems,
                    sig.elem_width.max(1),
                    signal_map,
                    signals,
                )?));
            }
            return Ok(None);
        }
        let Expr::StructLit { members } = rhs else {
            return Ok(None);
        };
        if !sig.struct_fields.is_empty() {
            return Ok(Some(self.pack_struct_pattern(
                members,
                &sig.struct_fields.clone(),
                signal_map,
                signals,
                0,
            )?));
        }
        Ok(None)
    }

    /// Pack assignment pattern untuk queue/dynamic array → `IrExpr::Concat`
    /// MSB-first: elemen terakhir di posisi MSB, elemen 0 di bit terendah
    /// (storage queue engine menaruh elemen i di `i*elem_width`). Setiap
    /// elemen di-cast ke `elem_width`. Satu elemen → nilai tunggal.
    fn pack_array_pattern(
        &self,
        elems: &[&Expr],
        elem_width: usize,
        signal_map: &HashMap<Symbol, SignalId>,
        signals: &[SignalInfo],
    ) -> Result<IrExpr, SimError> {
        let mut irs: Vec<IrExpr> = Vec::with_capacity(elems.len());
        for e in elems {
            let ir = self.elaborate_expr(e, signal_map, signals)?;
            irs.push(if ir_val_width(&ir) == elem_width {
                ir
            } else {
                IrExpr::Cast {
                    width: elem_width,
                    expr: Box::new(ir),
                }
            });
        }
        // Concat membangun operand PERTAMA sebagai bit TERTINGGI (MSB-first);
        // queue menaruh elemen 0 di bit TERENDAH, jadi urutan dibalik.
        let parts: Vec<IrExpr> = irs.into_iter().rev().collect();
        Ok(if parts.len() == 1 {
            parts.into_iter().next().unwrap()
        } else {
            IrExpr::Concat(parts)
        })
    }

    /// Pack `'{...}` → concat MSB-first sesuai offset field.
    fn pack_struct_pattern(
        &self,
        members: &[StructLitMember],
        fields: &[StructFieldInfo],
        signal_map: &HashMap<Symbol, SignalId>,
        signals: &[SignalInfo],
        depth: usize,
    ) -> Result<IrExpr, SimError> {
        if depth >= MAX_DEPTH {
            return Ok(IrExpr::Const(LogicVec::from_u64(0, 1)));
        }
        let total_width = fields
            .iter()
            .map(|f| f.offset.saturating_add(f.width.max(1)))
            .max()
            .unwrap_or(0);
        // (offset, width, nilai) — lalu diurut MSB-first.
        let mut entries: Vec<(usize, usize, IrExpr)> = Vec::with_capacity(fields.len());
        for f in fields {
            let width = f.width.max(1);
            let named = members.iter().find_map(|m| match m {
                StructLitMember::Named(n, e) if *n == f.name => Some(e),
                _ => None,
            });
            // `default:` berlaku untuk member yang tidak disebut (LRM §7.9).
            let default_expr = members.iter().find_map(|m| match m {
                StructLitMember::Default(e) => Some(e),
                _ => None,
            });
            let value_expr = named.or(default_expr);
            let ir_val = match value_expr {
                Some(e) => {
                    // Nested struct bersarang: `'{inner: '{...}}`.
                    if let Expr::StructLit { members } = e {
                        if !f.sub_fields.is_empty() {
                            self.pack_struct_pattern(
                                members,
                                &f.sub_fields,
                                signal_map,
                                signals,
                                depth + 1,
                            )?
                        } else {
                            self.elaborate_expr(e, signal_map, signals)?
                        }
                    } else {
                        self.elaborate_expr(e, signal_map, signals)?
                    }
                }
                None => IrExpr::Const(LogicVec::from_u64(0, width)),
            };
            // Normalisasi lebar member ke lebar field (SV mensyaratkan sama;
            // truncate alih-alih menggeser seluruh packing).
            let ir_val = if ir_val_width(&ir_val) == width {
                ir_val
            } else {
                IrExpr::Cast {
                    width,
                    expr: Box::new(ir_val),
                }
            };
            entries.push((f.offset, width, ir_val));
        }
        // MSB-first: offset terbesar dulu (descending).
        entries.sort_by_key(|(offset, _, _)| std::cmp::Reverse(*offset));
        let mut parts: Vec<IrExpr> = Vec::with_capacity(entries.len() * 2);
        let mut cursor = total_width;
        for (offset, width, val) in entries {
            if cursor > offset + width {
                parts.push(IrExpr::Const(LogicVec::from_u64(
                    0,
                    cursor - (offset + width),
                )));
            }
            parts.push(val);
            cursor = offset;
        }
        if cursor > 0 {
            parts.push(IrExpr::Const(LogicVec::from_u64(0, cursor)));
        }
        Ok(match parts.len() {
            0 => IrExpr::Const(LogicVec::from_u64(0, total_width.max(1))),
            1 => parts
                .pop()
                .unwrap_or(IrExpr::Const(LogicVec::from_u64(0, 1))),
            _ => IrExpr::Concat(parts),
        })
    }
}

/// Lebar statis IrExpr bila bisa diketahui tanpa evaluasi (Const/Cast/Concat);
/// 0 = tidak diketahui.
fn ir_val_width(e: &IrExpr) -> usize {
    match e {
        IrExpr::Const(v) => v.width,
        IrExpr::Cast { width, .. } => *width,
        IrExpr::Concat(parts) => parts.iter().map(ir_val_width).sum(),
        _ => 0,
    }
}
