//! Pass `ConstFold` — constant folding + identity boolean (SYNTHESIS.md §6).
//!
//! Dua aturan per node:
//! 1. **Semua operand konstanta** (2-state) → hasil dihitung → node diganti
//!    konstanta (`values[output] = Const`).
//! 2. **Identity / double inversion** → node diganti salah satu operand
//!    (`values[output] = values[operand]` clone).
//!
//! `Buffer` selalu di-alias ke input-nya (transparan). 4-state (X/Z) TIDAK
//! di-fold (dilewati) — jangan disalah-fold.

use mivon_core::LogicVec;
use mivon_sir::{SirModule, SirNodeKind, SirValue, ValueId};

use crate::opt::{alias_content, mask, resolve_const, u64_known};
use crate::pass::{SynthContext, SynthPass};

/// Pass: constant folding + identity boolean.
pub struct ConstFold;

impl SynthPass for ConstFold {
    fn name(&self) -> &'static str {
        "const_fold"
    }

    fn run(&mut self, ctx: &mut SynthContext) -> Result<usize, mivon_core::error::SimError> {
        let mut changed = 0usize;
        let nids: Vec<usize> = (0..ctx.module.nodes.len()).collect();
        for nid in nids {
            if !crate::opt::node_alive(&ctx.module, nid) {
                continue;
            }
            let kind = ctx.module.nodes[nid].kind.clone();
            let inputs = ctx.module.nodes[nid].inputs.clone();
            let width = ctx.module.nodes[nid].width;
            let out = ctx.module.nodes[nid].output;

            if let Some(lv) = fold_node(&ctx.module, &kind, &inputs, width) {
                ctx.module.values[out] = SirValue::Const(lv);
                changed += 1;
            } else if let Some(content) = identity_node(&ctx.module, &kind, &inputs, width) {
                ctx.module.values[out] = content;
                changed += 1;
            }
        }
        Ok(changed)
    }
}

/// Fold node bila SEMUA operand konstanta 2-state.
fn fold_node(
    m: &SirModule,
    kind: &SirNodeKind,
    inputs: &[ValueId],
    width: usize,
) -> Option<LogicVec> {
    let consts: Vec<LogicVec> = inputs
        .iter()
        .map(|v| resolve_const(m, *v))
        .collect::<Option<_>>()?;
    let vals: Vec<Option<u64>> = consts.iter().map(u64_known).collect();
    if vals.iter().any(|v| v.is_none()) {
        return None; // ada X/Z → jangan fold
    }
    let vals: Vec<u64> = vals.into_iter().map(|v| v.unwrap()).collect();
    let msk = mask(width);

    let r = match kind {
        SirNodeKind::And => Some(vals[0] & vals[1]),
        SirNodeKind::Or => Some(vals[0] | vals[1]),
        SirNodeKind::Xor => Some(vals[0] ^ vals[1]),
        SirNodeKind::Not => Some(!vals[0] & msk),
        SirNodeKind::Add => Some(vals[0].wrapping_add(vals[1]) & msk),
        SirNodeKind::Sub => Some(vals[0].wrapping_sub(vals[1]) & msk),
        SirNodeKind::Mul => Some(vals[0].wrapping_mul(vals[1]) & msk),
        SirNodeKind::Div => (vals[1] != 0).then(|| (vals[0] / vals[1]) & msk),
        SirNodeKind::Mod => (vals[1] != 0).then(|| (vals[0] % vals[1]) & msk),
        SirNodeKind::Shl => Some(shift_left(vals[0], vals[1], msk)),
        SirNodeKind::Shr => {
            // Guard shift >= 64 (UB di release tanpa guard).
            if vals[1] >= 64 {
                Some(0)
            } else {
                Some((vals[0] >> vals[1]) & msk)
            }
        }
        SirNodeKind::Sar => {
            // Sign-extend DARI lebar node (bukan bit 63): 0x80 (8-bit) >> 1
            // aritmetik = 0xC0, bukan 0x40.
            let sign = (vals[0] >> width.saturating_sub(1)) & 1;
            let ext = if sign == 1 { vals[0] | !msk } else { vals[0] };
            let sa = (ext as i64) >> vals[1].min(63);
            Some((sa as u64) & msk)
        }
        SirNodeKind::Eq | SirNodeKind::Ne => Some(rel(vals[0], vals[1], kind)),
        SirNodeKind::Lt => Some((vals[0] < vals[1]) as u64),
        SirNodeKind::Le => Some((vals[0] <= vals[1]) as u64),
        SirNodeKind::Gt => Some((vals[0] > vals[1]) as u64),
        SirNodeKind::Ge => Some((vals[0] >= vals[1]) as u64),
        SirNodeKind::ReduceAnd => Some((vals[0] == msk) as u64),
        SirNodeKind::ReduceOr => Some((vals[0] != 0) as u64),
        SirNodeKind::ReduceXor => Some(vals[0].count_ones() as u64 & 1),
        SirNodeKind::Concat => {
            // Guard overflow: total concat > 64 bit tidak bisa diwakili u64
            // → skip fold (bukan hasil terpotong).
            let total: usize = inputs.iter().map(|id| m.value_width(*id)).sum();
            if total > 64 {
                return None;
            }
            let mut acc = 0u64;
            let mut shift = 0usize;
            for (v, in_id) in vals.iter().zip(inputs.iter()) {
                acc |= v << shift;
                shift += m.value_width(*in_id);
            }
            Some(acc)
        }
        SirNodeKind::Slice { msb, lsb } => {
            let w = msb.saturating_sub(*lsb) + 1;
            // Guard lsb >= 64: semua bit tergeser keluar → 0 (dan hindari UB
            // shift overflow).
            let r = if *lsb >= 64 {
                0
            } else {
                (vals[0] >> lsb) & mask(w)
            };
            Some(r)
        }
        // Mux: inputs = [sel, t, f]. sel==0 → f; else → t.
        SirNodeKind::Mux => {
            if vals[0] == 0 {
                Some(vals[2] & msk)
            } else {
                Some(vals[1] & msk)
            }
        }
        // Buffer/TriState: fold node tidak berlaku (Buffer di-alias, TriState skip).
        SirNodeKind::Buffer | SirNodeKind::TriState => None,
        // Tidak didukung folding: _ => None
    };
    r.map(|v| LogicVec::from_u64(v, width.max(1)))
}

fn rel(a: u64, b: u64, kind: &SirNodeKind) -> u64 {
    match kind {
        SirNodeKind::Eq => (a == b) as u64,
        SirNodeKind::Ne => (a != b) as u64,
        _ => 0,
    }
}

fn shift_left(a: u64, b: u64, msk: u64) -> u64 {
    if b >= 64 {
        0
    } else {
        (a << b) & msk
    }
}

/// Identity / double inversion → konten nilai pengganti.
fn identity_node(
    m: &SirModule,
    kind: &SirNodeKind,
    inputs: &[ValueId],
    width: usize,
) -> Option<SirValue> {
    let msk = mask(width);
    match kind {
        // Buffer transparan.
        SirNodeKind::Buffer => Some(alias_content(m, inputs[0])),
        // ~~a → a
        SirNodeKind::Not => {
            if let Some(SirValue::Node(inner)) = m.values.get(inputs[0]) {
                if m.nodes[*inner].kind == SirNodeKind::Not {
                    return Some(alias_content(m, m.nodes[*inner].inputs[0]));
                }
            }
            None
        }
        SirNodeKind::And => {
            let c = operand_const(m, inputs);
            match c {
                Some(0) => Some(SirValue::Const(LogicVec::from_u64(0, width.max(1)))),
                Some(c) if c == msk => other_operand(m, inputs).map(|v| alias_content(m, v)),
                _ => same_operand(inputs).map(|v| alias_content(m, v)),
            }
        }
        SirNodeKind::Or => {
            let c = operand_const(m, inputs);
            match c {
                Some(0) => other_operand(m, inputs).map(|v| alias_content(m, v)),
                Some(c) if c == msk => Some(SirValue::Const(LogicVec::from_u64(msk, width.max(1)))),
                _ => same_operand(inputs).map(|v| alias_content(m, v)),
            }
        }
        SirNodeKind::Xor => {
            let c = operand_const(m, inputs);
            match c {
                Some(0) => other_operand(m, inputs).map(|v| alias_content(m, v)),
                _ => same_operand(inputs)
                    .map(|_| SirValue::Const(LogicVec::from_u64(0, width.max(1)))),
            }
        }
        SirNodeKind::Mux => {
            // mux(c, t, t) → t
            if inputs[1] == inputs[2] {
                return Some(alias_content(m, inputs[1]));
            }
            // mux(0, t, f) → f ; mux(1, t, f) → t ; mux(_, t, f) non-const → none
            let c = resolve_const(m, inputs[0]).and_then(|lv| u64_known(&lv));
            match c {
                Some(0) => Some(alias_content(m, inputs[2])),
                Some(_) => Some(alias_content(m, inputs[1])),
                None => None,
            }
        }
        _ => None,
    }
}

/// Konstanta operand bila SALAH SATU operand konstanta (u64 2-state).
fn operand_const(m: &SirModule, inputs: &[ValueId]) -> Option<u64> {
    for v in inputs {
        if let Some(lv) = resolve_const(m, *v) {
            if let Some(u) = u64_known(&lv) {
                return Some(u);
            }
        }
    }
    None
}

/// Operand NON-konstanta (untuk identity `x op c → x`).
fn other_operand(m: &SirModule, inputs: &[ValueId]) -> Option<ValueId> {
    for v in inputs {
        if resolve_const(m, *v).is_none() {
            return Some(*v);
        }
    }
    None
}

/// Kedua operand sama (`x & x → x`, `x | x → x`, `x ^ x → 0`).
fn same_operand(inputs: &[ValueId]) -> Option<ValueId> {
    (inputs.len() == 2 && inputs[0] == inputs[1]).then(|| inputs[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pass::SynthPipeline;
    use mivon_core::intern::Symbol;
    use mivon_sir::{SirModule, SirNodeKind, SirValue};

    fn run_one(m: &mut SirModule, changed: usize) {
        let mut p = SynthPipeline::new();
        p.add(ConstFold);
        let taken = std::mem::replace(m, SirModule::new(Symbol::intern("tmp")));
        let (m2, results) = p.run(taken).unwrap();
        *m = m2;
        assert_eq!(results[0].changed, changed, "changed count");
    }

    #[test]
    fn fold_and_consts() {
        let mut m = SirModule::new(Symbol::intern("top"));
        m.add_value(SirValue::Const(LogicVec::from_u64(0b11, 2))); // 0
        m.add_value(SirValue::Const(LogicVec::from_u64(0b10, 2))); // 1
        let nid = m.add_node(SirNodeKind::And, vec![0, 1], 2);
        let out = m.nodes[nid].output;
        m.add_value(SirValue::Node(nid));
        run_one(&mut m, 1);
        assert_eq!(m.values[out], SirValue::Const(LogicVec::from_u64(0b10, 2)));
    }

    #[test]
    fn identity_and_zero_folds_to_zero() {
        // And(x, 0) → 0 via identity (x = port non-konstan).
        let mut m = SirModule::new(Symbol::intern("top"));
        m.add_value(SirValue::Const(LogicVec::from_u64(0, 8))); // 0 — konstanta 0
        m.add_value(SirValue::Port(0)); // 1 — x (port, non-konstan)
        let nid = m.add_node(SirNodeKind::And, vec![0, 1], 8);
        let out = m.nodes[nid].output;
        m.add_value(SirValue::Node(nid));
        run_one(&mut m, 1);
        assert_eq!(m.values[out], SirValue::Const(LogicVec::from_u64(0, 8)));
    }

    #[test]
    fn double_inversion_aliases() {
        // ~~x → x (x = port non-konstan, bukan jalur fold).
        let mut m = SirModule::new(Symbol::intern("top"));
        m.add_value(SirValue::Port(0)); // 0 — x
        let n1 = m.add_node(SirNodeKind::Not, vec![0], 8);
        let o1 = m.nodes[n1].output;
        m.add_value(SirValue::Node(n1)); // vid 1 = Not(x)
        let n2 = m.add_node(SirNodeKind::Not, vec![1], 8);
        let o2 = m.nodes[n2].output;
        m.add_value(SirValue::Node(n2)); // vid 2 = Not(Not(x))
        run_one(&mut m, 1);
        assert_eq!(m.values[o2], m.values[0], "~~x harus alias x");
        let _ = o1;
    }

    #[test]
    fn sar_sign_extends_from_node_width() {
        // 0x80 (8-bit, msb set) >> 1 aritmetik = 0xC0, bukan 0x40.
        let mut m = SirModule::new(Symbol::intern("top"));
        m.add_value(SirValue::Const(LogicVec::from_u64(0x80, 8))); // 0
        m.add_value(SirValue::Const(LogicVec::from_u64(1, 8))); // 1
        let nid = m.add_node(SirNodeKind::Sar, vec![0, 1], 8);
        let out = m.nodes[nid].output;
        m.add_value(SirValue::Node(nid));
        run_one(&mut m, 1);
        assert_eq!(
            m.values[out],
            SirValue::Const(LogicVec::from_u64(0xC0, 8)),
            "Sar harus sign-extend dari lebar node"
        );
    }

    #[test]
    fn shr_large_amount_folds_zero() {
        // 0xFF >> 70 → 0 (guard shift >= 64, hindari UB).
        let mut m = SirModule::new(Symbol::intern("top"));
        m.add_value(SirValue::Const(LogicVec::from_u64(0xFF, 8))); // 0
        m.add_value(SirValue::Const(LogicVec::from_u64(70, 8))); // 1
        let nid = m.add_node(SirNodeKind::Shr, vec![0, 1], 8);
        let out = m.nodes[nid].output;
        m.add_value(SirValue::Node(nid));
        run_one(&mut m, 1);
        assert_eq!(m.values[out], SirValue::Const(LogicVec::from_u64(0, 8)));
    }

    #[test]
    fn concat_over_64_skips_fold() {
        // Concat total 72 bit > 64 → tidak di-fold (dilewati, bukan hasil
        // terpotong / UB).
        let mut m = SirModule::new(Symbol::intern("top"));
        m.add_value(SirValue::Const(LogicVec::from_u64(0xAB, 8))); // 0
        m.add_value(SirValue::Const(LogicVec::from_u64(0xCD, 8))); // 1
        m.add_value(SirValue::Const(LogicVec::from_u64(0xEF, 8))); // 2
                                                                   // replikasi x3: 8*3*3 = 72 bit — cukup pakai 9 operand 8-bit.
                                                                   // (Simulasikan concat lebar: total = sum lebar = 72.)
        let inputs: Vec<ValueId> = (0..9).map(|_| 0).collect();
        let nid = m.add_node(SirNodeKind::Concat, inputs, 72);
        let out = m.nodes[nid].output;
        m.add_value(SirValue::Node(nid));
        run_one(&mut m, 0);
        assert_eq!(
            m.values[out],
            SirValue::Node(nid),
            "concat > 64 bit harus dilewati (tidak di-fold)"
        );
    }

    #[test]
    fn xor_same_operand_folds_zero() {
        let mut m = SirModule::new(Symbol::intern("top"));
        m.add_value(SirValue::Const(LogicVec::from_u64(7, 4))); // 0
        let nid = m.add_node(SirNodeKind::Xor, vec![0, 0], 4);
        let out = m.nodes[nid].output;
        m.add_value(SirValue::Node(nid));
        run_one(&mut m, 1);
        assert_eq!(m.values[out], SirValue::Const(LogicVec::from_u64(0, 4)));
    }

    #[test]
    fn eq_same_operand_folds_one() {
        // Eq(x, x) → 1.
        let mut m = SirModule::new(Symbol::intern("top"));
        m.add_value(SirValue::Const(LogicVec::from_u64(7, 4))); // 0
        let nid = m.add_node(SirNodeKind::Eq, vec![0, 0], 1);
        let out = m.nodes[nid].output;
        m.add_value(SirValue::Node(nid));
        run_one(&mut m, 1);
        assert_eq!(m.values[out], SirValue::Const(LogicVec::from_u64(1, 1)));
    }

    #[test]
    fn neq_same_operand_folds_zero() {
        // Neq(x, x) → 0.
        let mut m = SirModule::new(Symbol::intern("top"));
        m.add_value(SirValue::Const(LogicVec::from_u64(7, 4))); // 0
        let nid = m.add_node(SirNodeKind::Ne, vec![0, 0], 1);
        let out = m.nodes[nid].output;
        m.add_value(SirValue::Node(nid));
        run_one(&mut m, 1);
        assert_eq!(m.values[out], SirValue::Const(LogicVec::from_u64(0, 1)));
    }

    #[test]
    fn eq_different_operands_folds_zero() {
        // Eq(1, 2) → 0.
        let mut m = SirModule::new(Symbol::intern("top"));
        m.add_value(SirValue::Const(LogicVec::from_u64(1, 1))); // 0
        m.add_value(SirValue::Const(LogicVec::from_u64(2, 1))); // 1
        let nid = m.add_node(SirNodeKind::Eq, vec![0, 1], 1);
        let out = m.nodes[nid].output;
        m.add_value(SirValue::Node(nid));
        run_one(&mut m, 1);
        assert_eq!(m.values[out], SirValue::Const(LogicVec::from_u64(0, 1)));
    }

    #[test]
    fn shl_non_zero_shift_folds() {
        // 0x01 << 2 → 0x04.
        let mut m = SirModule::new(Symbol::intern("top"));
        m.add_value(SirValue::Const(LogicVec::from_u64(0x01, 8))); // 0
        m.add_value(SirValue::Const(LogicVec::from_u64(2, 8))); // 1
        let nid = m.add_node(SirNodeKind::Shl, vec![0, 1], 8);
        let out = m.nodes[nid].output;
        m.add_value(SirValue::Node(nid));
        run_one(&mut m, 1);
        assert_eq!(m.values[out], SirValue::Const(LogicVec::from_u64(0x04, 8)));
    }

    #[test]
    fn shl_shift_zero_identity() {
        // 0x0F << 0 → 0x0F.
        let mut m = SirModule::new(Symbol::intern("top"));
        m.add_value(SirValue::Const(LogicVec::from_u64(0x0F, 8))); // 0
        m.add_value(SirValue::Const(LogicVec::from_u64(0, 8))); // 1
        let nid = m.add_node(SirNodeKind::Shl, vec![0, 1], 8);
        let out = m.nodes[nid].output;
        m.add_value(SirValue::Node(nid));
        run_one(&mut m, 1);
        assert_eq!(m.values[out], SirValue::Const(LogicVec::from_u64(0x0F, 8)));
    }

    #[test]
    fn or_with_zero_aliases() {
        // Or(x, 0) → x (operand non-konstan).
        let mut m = SirModule::new(Symbol::intern("top"));
        m.add_value(SirValue::Port(0)); // x
        m.add_value(SirValue::Const(LogicVec::from_u64(0, 8))); // 0
        let nid = m.add_node(SirNodeKind::Or, vec![0, 1], 8);
        let out = m.nodes[nid].output;
        m.add_value(SirValue::Node(nid));
        run_one(&mut m, 1);
        assert_eq!(m.values[out], m.values[0], "Or(x, 0) harus alias x");
    }

    #[test]
    fn and_with_all_ones_aliases() {
        // And(x, all_ones) → x.
        let mut m = SirModule::new(Symbol::intern("top"));
        m.add_value(SirValue::Port(0)); // x
        m.add_value(SirValue::Const(LogicVec::from_u64(0xFF, 8))); // all ones
        let nid = m.add_node(SirNodeKind::And, vec![0, 1], 8);
        let out = m.nodes[nid].output;
        m.add_value(SirValue::Node(nid));
        run_one(&mut m, 1);
        assert_eq!(m.values[out], m.values[0], "And(x, all_ones) harus alias x");
    }

    #[test]
    fn xor_same_operand_folds_zero_8bit() {
        // Xor(x, x) → 0 pada lebar 8.
        let mut m = SirModule::new(Symbol::intern("top"));
        m.add_value(SirValue::Const(LogicVec::from_u64(0xAB, 8))); // 0
        let nid = m.add_node(SirNodeKind::Xor, vec![0, 0], 8);
        let out = m.nodes[nid].output;
        m.add_value(SirValue::Node(nid));
        run_one(&mut m, 1);
        assert_eq!(m.values[out], SirValue::Const(LogicVec::from_u64(0, 8)));
    }
}
