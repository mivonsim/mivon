//! Assignment pattern struct: `p = '{hi: 4'hA, lo: 4'h5}` (LRM 1800
//! §7.9 struct assignment pattern).
//!
//! Sebelumnya pola bernama di runtime di-evaluate jadi `FillLit(0)`
//! (lihat `elaborate_expr` arm `Expr::StructLit`) karena layout typedef tak
//! tersedia di level ekspresi. Akibatnya `p = '{hi:4'hA, lo:4'h5}` menghasilkan
//! `p = 0` — silent wrong (padahal member access `p.hi` berfungsi penuh).
//!
//! Fix: di level STATEMENT kita tahu LHS-nya struct, jadi layout
//! (`StructFieldInfo`: offset+width tiap field) tersedia. Pola di-pack menjadi
//! `IrExpr::Concat` MSB-first sesuai offset field, tiap member di-cast ke
//! lebar field-nya, bit celah di-isi nol, dan member yang tidak disebut
//! bernilai 0 (default SV).
//!
//! 1 file = 1 tanggung jawab (packing layout + call site statement).

use mivon_ast::expr::{Expr, StructLitMember};
use mivon_core::error::SimError;
use mivon_core::intern::Symbol;
use mivon_ir::{IrExpr, IrLValue, LogicVec, SignalId, SignalInfo, StructFieldInfo};
use std::collections::HashMap;

use super::Elaborator;

/// Batas rekursi struct bersarang di dalam pola (anti siklus typedef).
const MAX_DEPTH: usize = 16;

impl Elaborator {
    /// Kalau `rhs` adalah struct assignment pattern dan LHS-nya struct dengan
    /// layout diketahui, pack menjadi `IrExpr`. `Ok(None)` = bukan pola atau
    /// layout tak diketahui → pemanggil jatuh ke `elaborate_expr` (perilaku lama).
    pub(crate) fn elaborate_struct_pattern(
        &self,
        ir_lhs: &IrLValue,
        rhs: &Expr,
        signal_map: &HashMap<Symbol, SignalId>,
        signals: &[SignalInfo],
    ) -> Result<Option<IrExpr>, SimError> {
        let Expr::StructLit { members } = rhs else {
            return Ok(None);
        };
        let Some(fields) = lvalue_struct_fields(ir_lhs, signals) else {
            return Ok(None);
        };
        Ok(Some(self.pack_struct_pattern(
            members,
            &fields,
            signal_map,
            signals,
            0,
        )?))
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
                parts.push(IrExpr::Const(LogicVec::from_u64(0, cursor - (offset + width))));
            }
            parts.push(val);
            cursor = offset;
        }
        if cursor > 0 {
            parts.push(IrExpr::Const(LogicVec::from_u64(0, cursor)));
        }
        Ok(match parts.len() {
            0 => IrExpr::Const(LogicVec::from_u64(0, total_width.max(1))),
            1 => parts.pop().unwrap_or(IrExpr::Const(LogicVec::from_u64(0, 1))),
            _ => IrExpr::Concat(parts),
        })
    }
}

/// Layout struct dari lvalue — `SignalInfo.struct_fields` (dipakai juga untuk
/// member access). Lvalue non-signal (bit/part select struct) belum punya
/// layout di elaborasi → `None` (perilaku lama).
fn lvalue_struct_fields(
    ir_lhs: &IrLValue,
    signals: &[SignalInfo],
) -> Option<Vec<StructFieldInfo>> {
    let IrLValue::Signal(sid, _) = ir_lhs else {
        return None;
    };
    let sig = signals.get(*sid)?;
    if sig.struct_fields.is_empty() {
        return None;
    }
    Some(sig.struct_fields.clone())
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
