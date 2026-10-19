//! Lowering RTL (`IrDesign`) → SIR (SYNTHESIS.md §5 — RTL normalization).
//!
//! Fase 1:
//! - `Process::Sequential` → `SirRegister` (clock/reset/enable diekstrak) +
//!   D-logic (mux chain dari if/case; "no assignment" = hold/RegQ).
//! - `Process::Combinational` / `CombReactive` → DAG node.
//! - Ekspresi → graf node (`BinaryOp` → node, `Cond` → MUX, select → SLICE).
//!
//! Konstruk non-sintesis TIDAK di-lower — `subset` (SYN check) sudah
//! melaporkannya sebelum pipeline ini dipanggil. Konstruk yang masih lolos
//! dicatat di `LowerResult.skipped` (honest: jangan diam-diam salah).

use mivon_core::{LogicVal, LogicVec};
use mivon_ir::{
    BinaryIrOp, ClockEdge, IrDesign, IrExpr, IrLValue, IrStmt, Process, SignalId, UnaryIrOp,
};
use std::collections::{HashMap, HashSet};

use crate::sir::*;

/// Hasil lowering.
#[derive(Debug, Clone)]
pub struct LowerResult {
    pub module: SirModule,
    /// Konstruk yang dilewati (tidak didukung fase 1) — untuk laporan.
    pub skipped: Vec<String>,
}

/// Lower seluruh `IrDesign` (top module) ke SIR.
pub fn lower(ir: &IrDesign) -> LowerResult {
    let mut b = Builder::new(ir);
    b.run();
    LowerResult {
        module: b.module,
        skipped: b.skipped,
    }
}

struct Builder<'a> {
    ir: &'a IrDesign,
    module: SirModule,
    /// ValueId per signal (slot). Placeholder `Const(0)` dibuat upfront,
    /// lalu slot di-resolve saat driver diketahui (Reg / Node / Port).
    signal_value: Vec<Option<ValueId>>,
    reg_ids: HashMap<SignalId, RegisterId>,
    skipped: Vec<String>,
    zero_cache: HashMap<usize, ValueId>,
}

impl<'a> Builder<'a> {
    fn new(ir: &'a IrDesign) -> Self {
        let top = &ir.top;
        let mut b = Builder {
            ir,
            module: SirModule::new(top.name),
            signal_value: vec![None; top.signals.len()],
            reg_ids: HashMap::new(),
            skipped: Vec::new(),
            zero_cache: HashMap::new(),
        };
        b.pass_ports();
        b
    }

    // ── Pass 1: port + wire + slot nilai per signal ──
    fn pass_ports(&mut self) {
        let top = &self.ir.top;
        for id in &top.inputs {
            let p = self.add_port(*id, PortDir::Input);
            // Slot signal = port (resolved langsung).
            let slot = self.signal_value[*id].expect("slot dibuat di add_port");
            self.module.values[slot] = SirValue::Port(p);
        }
        for id in &top.inouts {
            let p = self.add_port(*id, PortDir::Inout);
            let slot = self.signal_value[*id].expect("slot dibuat di add_port");
            self.module.values[slot] = SirValue::Port(p);
        }
        for id in &top.outputs {
            self.add_port(*id, PortDir::Output);
        }
        // Wire internal tanpa driver → tetap placeholder Const(0) (jujur:
        // bila tidak pernah di-drive, laporan menunjukkannya sebagai 0).
        for id in 0..top.signals.len() {
            self.ensure_signal(id);
        }
    }

    /// Buat port untuk signal + wire + slot placeholder.
    fn add_port(&mut self, sig: SignalId, dir: PortDir) -> PortId {
        let s = &self.ir.top.signals[sig];
        let width = s.width.max(1);
        // Slot placeholder dulu (bisa di-resolve setelahnya).
        let slot = self.slot_for(sig, width);
        let port = SirPort {
            name: s.name,
            dir,
            width,
            value: slot,
        };
        // PortId: input/inout → index di `inputs`; output → offset setelah
        // semua input (selaras dengan `value_width` yang men-chain keduanya).
        let pid = match dir {
            PortDir::Input | PortDir::Inout => {
                self.module.inputs.push(port);
                self.module.inputs.len() - 1
            }
            PortDir::Output => {
                self.module.outputs.push(port);
                self.module.inputs.len() + self.module.outputs.len() - 1
            }
        };
        // Wire IO.
        let wid = self.module.add_wire(s.name, width, slot);
        self.module.wires[wid].is_io = true;
        // src_map.
        if let Some(file) = &self.ir.source_file {
            self.module.src_map.insert(s.name, (file.clone(), 0, 0));
        }
        pid
    }

    /// Jamin slot nilai + wire untuk signal.
    fn ensure_signal(&mut self, sig: SignalId) {
        let s = &self.ir.top.signals[sig];
        let width = s.width.max(1);
        let slot = self.slot_for(sig, width);
        if self.module.wires.iter().all(|w| w.name != s.name) {
            let wid = self.module.add_wire(s.name, width, slot);
            if s.kind == mivon_ir::SignalKind::Input
                || s.kind == mivon_ir::SignalKind::Output
                || s.kind == mivon_ir::SignalKind::Inout
            {
                self.module.wires[wid].is_io = true;
            }
        }
    }

    /// Buat (atau ambil) slot nilai untuk signal — placeholder `Const(0)`.
    fn slot_for(&mut self, sig: SignalId, width: usize) -> ValueId {
        if let Some(v) = self.signal_value[sig] {
            return v;
        }
        let slot = self
            .module
            .add_value(SirValue::Const(LogicVec::from_u64(0, width.max(1))));
        self.signal_value[sig] = Some(slot);
        slot
    }

    fn run(&mut self) {
        let top = &self.ir.top;
        // ── Pass 2: register (proses sequential) ──
        for p in &top.processes {
            if let Process::Sequential {
                clock,
                reset,
                body,
                iff,
                ..
            } = p
            {
                let targets = collect_nba_targets(body);
                if targets.is_empty() {
                    continue;
                }
                // Kontrol register (clock/reset/enable) — dihitung sekali.
                let clk_val = match clock {
                    ClockEdge::PosEdge(id) | ClockEdge::NegEdge(id) => {
                        self.signal_value[*id].unwrap_or(0)
                    }
                    // Clock hierarkis tak ter-resolve fase 1 → net 0.
                    _ => 0,
                };
                let rst_spec = reset.as_ref().map(|r| ResetSpec {
                    signal: self.signal_value[r.signal].unwrap_or(0),
                    value: r.value.clone(),
                    polarity: r.polarity,
                    r#async: r.r#async,
                });
                let en_val = match iff {
                    Some(IrExpr::Signal(id, _)) => self.signal_value[*id],
                    _ => None,
                };
                // Buat register: Q = slot signal (di-resolve ke Reg). Lebar
                // register diambil dari SignalInfo (lvalue IR bisa membawa
                // lebar tak penuh — pakai lebar signal yang benar).
                for (sig_id, _width) in &targets {
                    if self.reg_ids.contains_key(sig_id) {
                        continue;
                    }
                    let slot = self.signal_value[*sig_id].expect("slot register");
                    let rid = self.module.registers.len();
                    self.module.values[slot] = SirValue::Reg(rid);
                    self.reg_ids.insert(*sig_id, rid);
                    let name = self.ir.top.signals[*sig_id].name;
                    let width = self.sig_width(*sig_id);
                    self.module.registers.push(SirRegister {
                        name,
                        d: slot, // sementara; diisi setelah D-logic
                        q: slot,
                        clock: clk_val,
                        reset: rst_spec.clone(),
                        enable: en_val,
                        width,
                    });
                }
                // D-logic: mux chain dari if/case; "no assignment" = hold (RegQ).
                let holds: HashMap<SignalId, ValueId> = targets
                    .iter()
                    .map(|(s, _)| (*s, self.signal_value[*s].expect("hold slot")))
                    .collect();
                let assigns = self.lower_stmts(body, &holds, &HashMap::new(), true);
                for (sig_id, d) in assigns {
                    if let Some(&rid) = self.reg_ids.get(&sig_id) {
                        self.module.registers[rid].d = d;
                    }
                }
            }
        }

        // ── Pass 3: logika kombinasi (proses comb) ──
        for p in &top.processes {
            match p {
                Process::Combinational { body, .. } | Process::CombReactive { body, .. } => {
                    let assigns = self.lower_stmts(body, &HashMap::new(), &HashMap::new(), false);
                    for (sig_id, v) in assigns {
                        // Resolve slot signal → nilai node.
                        if let Some(slot) = self.signal_value[sig_id] {
                            let resolved = self.module.values[v].clone();
                            self.module.values[slot] = resolved;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // ════════════ D-logic builder ════════════

    /// Lower daftar statement ke peta signal → nilai.
    ///
    /// - `holds`: nilai default per signal bila sebuah branch TIDAK meng-assign
    ///   (sequential: RegQ = hold; comb: kosong → 0).
    /// - `base`: nilai yang sudah di-assign oleh statement SEBELUMNYA (untuk
    ///   semantik "tidak di-assign di branch ini = nilai sebelumnya").
    #[allow(clippy::only_used_in_recursion)]
    fn lower_stmts(
        &mut self,
        stmts: &[IrStmt],
        holds: &HashMap<SignalId, ValueId>,
        base: &HashMap<SignalId, ValueId>,
        is_seq: bool,
    ) -> HashMap<SignalId, ValueId> {
        let mut out = base.clone();
        for st in stmts {
            match st {
                IrStmt::Block { stmts, .. } | IrStmt::NamedBlock { stmts, .. } => {
                    out = self.lower_stmts(stmts, holds, &out, is_seq);
                }
                IrStmt::BlockingAssign { lhs, rhs, .. }
                | IrStmt::NonBlockingAssign { lhs, rhs, .. } => {
                    // Write ke ELEMEN array (`mem[i] <= v`) = read-modify-write:
                    // hanya elemen terpilih berubah. Sebelumnya index diabaikan
                    // dan seluruh sinyal tertimpa RHS (netlist salah diam-diam).
                    let elem: Option<(SignalId, ValueId, usize)> = match lhs {
                        IrLValue::ArrayIndex {
                            sig_id,
                            index,
                            elem_width,
                        } => {
                            let iv = self.lower_expr(index);
                            let ew = (*elem_width).max(1);
                            let off = if ew == 1 {
                                iv
                            } else {
                                let ewv = self.const_val(ew as u64, 32);
                                self.node(SirNodeKind::Mul, vec![iv, ewv], 32)
                            };
                            Some((*sig_id, off, ew))
                        }
                        IrLValue::ArrayBitSelect {
                            sig_id,
                            index,
                            elem_width,
                            bit,
                        } => {
                            let iv = self.lower_expr(index);
                            let ew = (*elem_width).max(1);
                            let ewv = self.const_val(ew as u64, 32);
                            let off = self.node(SirNodeKind::Mul, vec![iv, ewv], 32);
                            let bv = self.lower_expr(bit);
                            let off = self.node(SirNodeKind::Add, vec![off, bv], 32);
                            Some((*sig_id, off, 1))
                        }
                        _ => None,
                    };
                    if let Some((id, off, ew)) = elem {
                        let w = self.sig_width(id);
                        let old = out
                            .get(&id)
                            .copied()
                            .or_else(|| holds.get(&id).copied())
                            .unwrap_or_else(|| self.const_zero(w));
                        let v = self.lower_expr(rhs);
                        let merged = self.lower_elem_write(id, off, ew, v, old);
                        out.insert(id, merged);
                    } else if let Some((id, _w)) = lvalue_signal(lhs) {
                        // Context rule SV: hasil di-fit ke lebar signal.
                        let v = self.lower_expr(rhs);
                        let v = self.fit(v, self.sig_width(id));
                        out.insert(id, v);
                    }
                }
                IrStmt::If {
                    cond,
                    true_branch,
                    false_branch,
                } => {
                    let c = self.lower_expr(cond);
                    let t = self.lower_stmts(true_branch, holds, &out, is_seq);
                    let f = self.lower_stmts(false_branch, holds, &out, is_seq);
                    out = self.merge_branches(c, &t, &f, &out, holds);
                }
                IrStmt::Case {
                    case_type,
                    expr,
                    items,
                    default,
                } => {
                    // `case inside`: label bisa rentang `[lo:hi]` → bandingkan
                    // dengan GE & LE, bukan EQ (label tunggal tetap EQ).
                    let is_inside = matches!(case_type, mivon_ir::CaseType::Inside);
                    let sel = self.lower_expr(expr);
                    let mut cur = self.lower_stmts(default, holds, &out, is_seq);
                    // Priority chain: item terakhir = prioritas terendah.
                    for item in items.iter().rev() {
                        let body = self.lower_stmts(&item.body, holds, &out, is_seq);
                        for label in item.labels.iter().rev() {
                            let eq = if is_inside {
                                self.lower_inside_item(sel, label)
                            } else {
                                let lv = self.lower_expr(label);
                                let (a, b) = self.align_consts(sel, lv);
                                self.node(SirNodeKind::Eq, vec![a, b], 1)
                            };
                            cur = self.merge_branches(eq, &body, &cur, &out, holds);
                        }
                    }
                    out = cur;
                }
                // Null/Break/Continue aman diabaikan (break/continue sudah
                // di-unroll elaborator); statement lain dicatat jujur.
                IrStmt::Null | IrStmt::Break | IrStmt::Continue => {}
                other => self
                    .skipped
                    .push(format!("statement tidak didukung SIR fase 1: {other:?}")),
            }
        }
        out
    }

    /// Gabung dua branch if/case: signal yang di-assign di salah satu branch
    /// di-mux dengan kondisi. Signal yang tidak di-assign di branch mana pun
    /// mempertahankan nilai `base` (sudah di-copy ke out oleh pemanggil).
    fn merge_branches(
        &mut self,
        cond: ValueId,
        t: &HashMap<SignalId, ValueId>,
        f: &HashMap<SignalId, ValueId>,
        base: &HashMap<SignalId, ValueId>,
        holds: &HashMap<SignalId, ValueId>,
    ) -> HashMap<SignalId, ValueId> {
        let keys: HashSet<SignalId> = t.keys().chain(f.keys()).copied().collect();
        let mut merged = base.clone();
        for k in keys {
            let tv = t.get(&k).copied();
            let fv = f.get(&k).copied();
            let base_v = base.get(&k).copied();
            let width = self.sig_width(k);
            // Nilai bila sebuah branch TIDAK meng-assign signal ini, prioritas:
            // 1) nilai dari statement SEBELUMNYA di proses yang sama
            //    (`q <= 0; if (c) q <= 2;` → saat !c, q = 0, BUKAN RegQ), lalu
            // 2) hold (RegQ) untuk FF, lalu 3) 0 untuk comb.
            let fallback = match base_v.or(holds.get(&k).copied()) {
                Some(v) => v,
                None => self.const_zero(width),
            };
            let v = match (tv, fv) {
                (Some(tv), Some(fv)) => self.mux(cond, tv, fv, width),
                (Some(tv), None) => self.mux(cond, tv, fallback, width),
                (None, Some(fv)) => self.mux(cond, fallback, fv, width),
                _ => continue,
            };
            merged.insert(k, v);
        }
        merged
    }

    // ════════════ Ekspresi → graf node ════════════

    fn lower_expr(&mut self, e: &IrExpr) -> ValueId {
        match e {
            IrExpr::Const(lv) => self.module.add_value(SirValue::Const(lv.clone())),
            IrExpr::FillLit(v) => {
                let lv = LogicVec::fill(*v, 1);
                self.module.add_value(SirValue::Const(lv))
            }
            IrExpr::Signal(id, w) => self.signal_value[*id].unwrap_or_else(|| self.const_zero(*w)),
            IrExpr::RangeSelect(id, msb, lsb) => {
                let slot: Option<ValueId> = self.signal_value[*id];
                let base = slot.unwrap_or_else(|| self.const_zero(1));
                self.slice_of(base, *msb, *lsb)
            }
            IrExpr::BitSelect(id, i) => {
                let slot: Option<ValueId> = self.signal_value[*id];
                let base = slot.unwrap_or_else(|| self.const_zero(1));
                self.slice_of(base, *i, *i)
            }
            IrExpr::ExprRangeSelect(inner, msb, lsb) => {
                let v = self.lower_expr(inner);
                self.slice_of(v, *msb, *lsb)
            }
            IrExpr::ExprBitSelect(inner, i) => {
                let v = self.lower_expr(inner);
                self.slice_of(v, *i, *i)
            }
            IrExpr::ExprPartSelect(base, idx, width) => {
                // Fase 1: index konstan saja didukung (dinamis → skipped).
                let base_v = self.lower_expr(base);
                let idx_v = self.lower_expr(idx);
                match self.const_u64(idx_v) {
                    Some(lo) => {
                        let wv = self.lower_expr(width);
                        let w = self.value_width(wv);
                        let lo = lo as usize;
                        let hi = lo + w - 1;
                        self.slice_of(base_v, hi, lo)
                    }
                    None => {
                        self.skipped
                            .push("part-select dinamis tidak di-lower fase 1".into());
                        self.const_zero(1)
                    }
                }
            }
            // Array unpacked read `mem[i]` (register file / RAM read) —
            // index konstan → SLICE statis; index runtime → barrel shift +
            // mask (`(mem >> i*elem_width) & ((1<<elem_width)-1)`).
            IrExpr::ArrayIndex {
                sig_id,
                index,
                elem_width,
            } => {
                let w = self.sig_width(*sig_id);
                let ew = (*elem_width).max(1);
                let base = self.signal_value[*sig_id].unwrap_or_else(|| self.const_zero(w));
                let idx_v = self.lower_expr(index);
                if let Some(i) = self.const_u64(idx_v) {
                    let lo = (i as usize).saturating_mul(ew);
                    if lo + ew <= w {
                        return self.slice_of(base, lo + ew - 1, lo);
                    }
                    return self.const_zero(ew);
                }
                let shift_w = w.max(32);
                let ew_v = self.const_val(ew as u64, shift_w);
                let off = self.node(SirNodeKind::Mul, vec![idx_v, ew_v], shift_w);
                let shifted = self.node(SirNodeKind::Shr, vec![base, off], w);
                let mask_lv = if ew >= 64 {
                    LogicVec::fill(LogicVal::One, ew)
                } else {
                    LogicVec::from_u64((1u64 << ew) - 1, ew)
                };
                let mask = self.module.add_value(SirValue::Const(mask_lv));
                self.node(SirNodeKind::And, vec![shifted, mask], ew)
            }
            IrExpr::Concat(parts) => {
                let vals: Vec<ValueId> = parts.iter().map(|p| self.lower_expr(p)).collect();
                let width: usize = vals.iter().map(|v| self.value_width(*v)).sum();
                self.node(SirNodeKind::Concat, vals, width)
            }
            IrExpr::Replicate(n, inner) => {
                let v = self.lower_expr(inner);
                let w = self.value_width(v);
                let vals = vec![v; *n];
                self.node(SirNodeKind::Concat, vals, w * n)
            }
            IrExpr::UnaryOp(op, inner) => {
                let v = self.lower_expr(inner);
                let w = self.value_width(v);
                match op {
                    UnaryIrOp::Not | UnaryIrOp::BitNot => self.node(SirNodeKind::Not, vec![v], w),
                    UnaryIrOp::Plus => self.node(SirNodeKind::Buffer, vec![v], w),
                    UnaryIrOp::Minus => {
                        let z = self.const_zero(w);
                        self.node(SirNodeKind::Sub, vec![z, v], w)
                    }
                    UnaryIrOp::RedAnd => self.node(SirNodeKind::ReduceAnd, vec![v], 1),
                    UnaryIrOp::RedOr => self.node(SirNodeKind::ReduceOr, vec![v], 1),
                    UnaryIrOp::RedXor => self.node(SirNodeKind::ReduceXor, vec![v], 1),
                    UnaryIrOp::RedNand => {
                        let r = self.node(SirNodeKind::ReduceAnd, vec![v], 1);
                        self.node(SirNodeKind::Not, vec![r], 1)
                    }
                    UnaryIrOp::RedNor => {
                        let r = self.node(SirNodeKind::ReduceOr, vec![v], 1);
                        self.node(SirNodeKind::Not, vec![r], 1)
                    }
                    UnaryIrOp::RedXnor => {
                        let r = self.node(SirNodeKind::ReduceXor, vec![v], 1);
                        self.node(SirNodeKind::Not, vec![r], 1)
                    }
                }
            }
            // System function yang sintesizable.
            IrExpr::SysFunc { name, args, .. } => {
                let n = name.as_str();
                match n {
                    "$unsigned" | "$signed" => args
                        .first()
                        .map(|a| self.lower_expr(a))
                        .unwrap_or_else(|| self.const_zero(1)),
                    "$countones" | "$onehot" | "$onehot0" => {
                        let Some(arg) = args.first() else {
                            return self.const_zero(1);
                        };
                        let v = self.lower_expr(arg);
                        let w = self.value_width(v).max(1);
                        let out_w = (usize::BITS - (w as usize).leading_zeros()) as usize + 1;
                        let mut acc: Option<ValueId> = None;
                        for i in 0..w {
                            let bit = self.slice_of(v, i, i);
                            let bit = self.fit(bit, out_w);
                            acc = Some(match acc {
                                None => bit,
                                Some(a) => self.node(SirNodeKind::Add, vec![a, bit], out_w),
                            });
                        }
                        let cnt = acc.unwrap_or_else(|| self.const_zero(out_w));
                        match n {
                            "$countones" => cnt,
                            "$onehot" => {
                                let one = self.const_val(1, out_w);
                                self.node(SirNodeKind::Eq, vec![cnt, one], 1)
                            }
                            _ => {
                                let one = self.const_val(1, out_w);
                                self.node(SirNodeKind::Le, vec![cnt, one], 1)
                            }
                        }
                    }
                    // 2-state synthesis: tidak ada X, jadi isunknown = 0.
                    "$isunknown" => self.const_zero(1),
                    _ => {
                        self.skipped.push(format!(
                            "system function {} non-sintesis / belum di-lower",
                            n
                        ));
                        self.const_zero(1)
                    }
                }
            }
            // Hierarchical signal read (`u.data`) — resolve lewat hier_signal_map.
            IrExpr::HierRef(name) => {
                if let Some(&sid) = self.ir.hier_signal_map.get(name) {
                    let w = self.sig_width(sid);
                    self.signal_value[sid].unwrap_or_else(|| self.const_zero(w))
                } else {
                    self.skipped.push(format!(
                        "hier ref '{}' tidak ter-resolve (non-sintesis)",
                        name.as_str()
                    ));
                    self.const_zero(1)
                }
            }
            // `x inside {a, b, [lo:hi]}` — OR dari perbandingan (sintesizable).
            IrExpr::Inside { expr, list } => {
                let v = self.lower_expr(expr);
                let mut acc: Option<ValueId> = None;
                for item in list {
                    let cmp = self.lower_inside_item(v, item);
                    acc = Some(match acc {
                        None => cmp,
                        Some(a) => self.node(SirNodeKind::Or, vec![a, cmp], 1),
                    });
                }
                acc.unwrap_or_else(|| self.const_zero(1))
            }
            IrExpr::InsideRange { expr, lo, hi } => {
                let v = self.lower_expr(expr);
                self.lower_range_check(v, lo, hi)
            }
            IrExpr::BinaryOp(op, a, b) => {
                let av = self.lower_expr(a);
                let bv = self.lower_expr(b);
                // Normalisasi lebar konstanta: `count + 1` (8-bit) → konstanta
                // ikut 8-bit, bukan 32-bit default SV.
                let (av, bv) = self.align_consts(av, bv);
                let aw = self.value_width(av);
                let bw = self.value_width(bv);
                // `**` dengan eksponen KONSTAN → rantai MUL (sintesizable).
                // Eksponen non-konstan butuh barrel multiplier — non-sintesis.
                if matches!(op, BinaryIrOp::Power) {
                    if let Some(exp) = self.const_u64(bv) {
                        let mut acc = self.const_val(1, aw);
                        for _ in 0..exp.min(64) {
                            acc = self.node(SirNodeKind::Mul, vec![acc, av], aw);
                        }
                        return acc;
                    }
                    self.skipped
                        .push("** dengan eksponen non-konstan (non-sintesis)".into());
                    return self.const_zero(aw);
                }
                let kind = match op {
                    BinaryIrOp::Add => SirNodeKind::Add,
                    BinaryIrOp::Sub => SirNodeKind::Sub,
                    BinaryIrOp::Mul => SirNodeKind::Mul,
                    BinaryIrOp::Div => SirNodeKind::Div,
                    BinaryIrOp::Mod => SirNodeKind::Mod,
                    BinaryIrOp::Eq | BinaryIrOp::CaseEq | BinaryIrOp::EqWild => SirNodeKind::Eq,
                    BinaryIrOp::Neq | BinaryIrOp::CaseNeq | BinaryIrOp::NeqWild => SirNodeKind::Ne,
                    BinaryIrOp::Lt => SirNodeKind::Lt,
                    BinaryIrOp::Le => SirNodeKind::Le,
                    BinaryIrOp::Gt => SirNodeKind::Gt,
                    BinaryIrOp::Ge => SirNodeKind::Ge,
                    BinaryIrOp::BitAnd | BinaryIrOp::LogicalAnd => SirNodeKind::And,
                    BinaryIrOp::BitOr | BinaryIrOp::LogicalOr => SirNodeKind::Or,
                    BinaryIrOp::BitXor => SirNodeKind::Xor,
                    BinaryIrOp::BitXnor => {
                        let x = self.node(SirNodeKind::Xor, vec![av, bv], aw.max(bw));
                        return self.node(SirNodeKind::Not, vec![x], aw.max(bw));
                    }
                    BinaryIrOp::Shl | BinaryIrOp::Sshl => SirNodeKind::Shl,
                    BinaryIrOp::Shr => SirNodeKind::Shr,
                    BinaryIrOp::Sshr => SirNodeKind::Sar,
                    BinaryIrOp::Power => {
                        self.skipped.push("** (power) tidak di-lower fase 1".into());
                        SirNodeKind::Buffer
                    }
                };
                let width = if matches!(
                    kind,
                    SirNodeKind::Eq
                        | SirNodeKind::Ne
                        | SirNodeKind::Lt
                        | SirNodeKind::Le
                        | SirNodeKind::Gt
                        | SirNodeKind::Ge
                ) {
                    1
                } else {
                    aw.max(bw)
                };
                self.node(kind, vec![av, bv], width)
            }
            IrExpr::Cond(c, t, f) => {
                let cv = self.lower_expr(c);
                let tv = self.lower_expr(t);
                let fv = self.lower_expr(f);
                let (tv, fv) = self.align_consts(tv, fv);
                let w = self.value_width(tv).max(self.value_width(fv));
                self.mux(cv, tv, fv, w)
            }
            IrExpr::Signed(inner) => self.lower_expr(inner),
            // Cast lebar (hasil propagate_context_width): lowering transparan
            // — untuk unsigned, nilai pada lebar lebih sempit identik dengan
            // zero-extension; node konsumen (Add dll.) menentukan lebar hasil.
            IrExpr::Cast { expr: inner, .. } => self.lower_expr(inner),
            _ => {
                self.skipped
                    .push(format!("ekspresi tidak didukung SIR fase 1: {e:?}"));
                self.const_zero(1)
            }
        }
    }

    // ════════════ Helper node ════════════

    fn node(&mut self, kind: SirNodeKind, inputs: Vec<ValueId>, width: usize) -> ValueId {
        let nid = self.module.add_node(kind, inputs, width);
        let vid = self.module.nodes[nid].output;
        self.module.add_value(SirValue::Node(nid));
        vid
    }

    fn mux(&mut self, cond: ValueId, t: ValueId, f: ValueId, width: usize) -> ValueId {
        // Normalisasi konstanta operand mux ke lebar bersama.
        let (t, f) = self.align_consts(t, f);
        self.node(SirNodeKind::Mux, vec![cond, t, f], width)
    }

    /// Fit nilai ke lebar target (context rule SV): truncate / zero-extend.
    fn fit(&mut self, v: ValueId, width: usize) -> ValueId {
        let w = self.value_width(v);
        if w == width || width == 0 {
            return v;
        }
        if w > width {
            return self.slice_of(v, width - 1, 0);
        }
        let z = self.const_zero(width - w);
        self.node(SirNodeKind::Concat, vec![z, v], width)
    }

    /// Normalisasi lebar konstanta bila salah satu operand adalah konstanta
    /// (literal SV default 32-bit → lebar pasangan operand).
    fn align_consts(&mut self, a: ValueId, b: ValueId) -> (ValueId, ValueId) {
        let wa = self.value_width(a);
        let wb = self.value_width(b);
        if wa == wb {
            return (a, b);
        }
        match (&self.module.values[a], &self.module.values[b]) {
            (SirValue::Const(_), _) => (self.resize_const(a, wb), b),
            (_, SirValue::Const(_)) => (a, self.resize_const(b, wa)),
            _ => (a, b),
        }
    }

    /// Emit ulang konstanta dengan lebar tertentu (truncate/zero-extend).
    fn resize_const(&mut self, v: ValueId, width: usize) -> ValueId {
        let lv = match &self.module.values[v] {
            SirValue::Const(lv) => lv,
            _ => return v,
        };
        let mask = if width >= 64 {
            u64::MAX
        } else {
            (1u64 << width) - 1
        };
        let val = lv.to_u64() & mask;
        self.module
            .add_value(SirValue::Const(LogicVec::from_u64(val, width.max(1))))
    }

    fn slice_of(&mut self, base: ValueId, msb: usize, lsb: usize) -> ValueId {
        let w = msb.saturating_sub(lsb) + 1;
        self.node(SirNodeKind::Slice { msb, lsb }, vec![base], w)
    }

    /// Satu item `inside {...}`: nilai tunggal → EQ, rentang → GE & LE.
    fn lower_inside_item(&mut self, v: ValueId, item: &IrExpr) -> ValueId {
        match item {
            IrExpr::InsideRange { lo, hi, .. } => self.lower_range_check(v, lo, hi),
            other => {
                let x = self.lower_expr(other);
                let (a, b) = self.align_consts(v, x);
                self.node(SirNodeKind::Eq, vec![a, b], 1)
            }
        }
    }

    /// `lo <= v <= hi` (inklusif).
    fn lower_range_check(&mut self, v: ValueId, lo: &IrExpr, hi: &IrExpr) -> ValueId {
        let lo_v = self.lower_expr(lo);
        let hi_v = self.lower_expr(hi);
        let (a, b) = self.align_consts(v, lo_v);
        let ge = self.node(SirNodeKind::Ge, vec![a, b], 1);
        let (c, d) = self.align_consts(v, hi_v);
        let le = self.node(SirNodeKind::Le, vec![c, d], 1);
        self.node(SirNodeKind::And, vec![ge, le], 1)
    }

    /// Tulis `ew` bit pada `offset` di dalam sinyal `sig` tanpa mengubah bit
    /// lain: `(old & ~(mask << off)) | ((v & mask) << off)`.
    fn lower_elem_write(
        &mut self,
        sig: SignalId,
        offset: ValueId,
        ew: usize,
        rhs: ValueId,
        old: ValueId,
    ) -> ValueId {
        let w = self.sig_width(sig);
        let v = self.fit(rhs, ew);
        let v_ext = self.fit(v, w);
        let placed = self.node(SirNodeKind::Shl, vec![v_ext, offset], w);
        let mask_lv = if ew >= 64 {
            LogicVec::fill(LogicVal::One, ew)
        } else {
            LogicVec::from_u64((1u64 << ew) - 1, ew)
        };
        let mask = self.module.add_value(SirValue::Const(mask_lv));
        let mask_ext = self.fit(mask, w);
        let shifted = self.node(SirNodeKind::Shl, vec![mask_ext, offset], w);
        let inv = self.node(SirNodeKind::Not, vec![shifted], w);
        let cleared = self.node(SirNodeKind::And, vec![old, inv], w);
        self.node(SirNodeKind::Or, vec![cleared, placed], w)
    }

    /// Konstanta u64 dengan lebar tertentu.
    fn const_val(&mut self, val: u64, width: usize) -> ValueId {
        self.module
            .add_value(SirValue::Const(LogicVec::from_u64(val, width.max(1))))
    }

    /// Konstanta nol (di-cache per lebar).
    fn const_zero(&mut self, width: usize) -> ValueId {
        if let Some(v) = self.zero_cache.get(&width) {
            return *v;
        }
        let v = self
            .module
            .add_value(SirValue::Const(LogicVec::from_u64(0, width.max(1))));
        self.zero_cache.insert(width, v);
        v
    }

    /// Baca konstanta u64 dari sebuah nilai (untuk part-select statis).
    fn const_u64(&self, v: ValueId) -> Option<u64> {
        match &self.module.values[v] {
            SirValue::Const(lv) => Some(lv.to_u64()),
            _ => None,
        }
    }

    fn value_width(&self, v: ValueId) -> usize {
        self.module.value_width(v)
    }

    fn sig_width(&self, sig: SignalId) -> usize {
        self.ir.top.signals[sig].width.max(1)
    }
}

/// Kumpulkan signal yang di-assign non-blocking (rekursif if/case/block).
fn collect_nba_targets(stmts: &[IrStmt]) -> Vec<(SignalId, usize)> {
    let mut out = Vec::new();
    collect_nba(stmts, &mut out);
    out
}

fn collect_nba(stmts: &[IrStmt], out: &mut Vec<(SignalId, usize)>) {
    for st in stmts {
        match st {
            IrStmt::NonBlockingAssign { lhs, .. } => {
                if let Some((id, w)) = lvalue_signal(lhs) {
                    out.push((id, w));
                }
            }
            IrStmt::Block { stmts, .. } | IrStmt::NamedBlock { stmts, .. } => {
                collect_nba(stmts, out)
            }
            IrStmt::If {
                true_branch,
                false_branch,
                ..
            } => {
                collect_nba(true_branch, out);
                collect_nba(false_branch, out);
            }
            IrStmt::Case { items, default, .. } => {
                for it in items {
                    collect_nba(&it.body, out);
                }
                collect_nba(default, out);
            }
            IrStmt::LoopFor { init, body, .. } => {
                if let Some(i) = init {
                    collect_nba(std::slice::from_ref(i), out);
                }
                collect_nba(body, out);
            }
            IrStmt::LoopWhile { body, .. }
            | IrStmt::LoopDoWhile { body, .. }
            | IrStmt::Repeat { body, .. }
            | IrStmt::Foreach { body, .. }
            | IrStmt::Delay { body, .. }
            | IrStmt::Wait { body, .. }
            | IrStmt::EventControl { body, .. } => collect_nba(body, out),
            IrStmt::Fork { processes, .. } => {
                for p in processes {
                    collect_nba(p, out);
                }
            }
            _ => {}
        }
    }
}

fn lvalue_signal(lhs: &IrLValue) -> Option<(SignalId, usize)> {
    match lhs {
        IrLValue::Signal(id, w) => Some((*id, *w)),
        IrLValue::BitSelect(id, _) | IrLValue::RangeSelect(id, _, _) => Some((*id, 1)),
        IrLValue::ArrayIndex { sig_id, .. } | IrLValue::ArrayBitSelect { sig_id, .. } => {
            Some((*sig_id, 1))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mivon_core::intern::Symbol;
    use mivon_ir::{ClockEdge, ResetInfo, SignalKind};

    fn base_design() -> IrDesign {
        IrDesign {
            top: mivon_ir::IrModule {
                name: Symbol::intern("counter"),
                ..Default::default()
            },
            modules: std::collections::HashMap::new(),
            classes: Default::default(),
            covergroups: Vec::new(),
            dpi_imports: Vec::new(),
            hier_signal_map: Default::default(),
            udp_defs: Vec::new(),
            specify_items: Vec::new(),
            timescale: None,
            module_functions: Default::default(),
            source_lines: None,
            source_file: None,
            pkg_scoped_consts: Default::default(),
            coverage_exclusions: Vec::new(),
            stmt_lines: HashMap::new(),
            net_aliases: HashMap::new(),
        }
    }

    /// counter 8-bit: count <= (count==99) ? 0 : count+1, async reset 0.
    fn counter_ir() -> IrDesign {
        let mut ir = base_design();
        let sig = |name: &str, width: usize, kind: SignalKind| {
            use mivon_ir::SignalInfo;
            SignalInfo {
                name: Symbol::intern(name),
                width,
                kind,
                ..Default::default()
            }
        };
        ir.top.signals.push(sig("clk", 1, SignalKind::Input));
        ir.top.signals.push(sig("rst_n", 1, SignalKind::Input));
        ir.top.signals.push(sig("count", 8, SignalKind::Output));
        ir.top.inputs = vec![0, 1];
        ir.top.outputs = vec![2];
        let rhs = IrExpr::Cond(
            Box::new(IrExpr::BinaryOp(
                BinaryIrOp::Eq,
                Box::new(IrExpr::Signal(2, 8)),
                Box::new(IrExpr::Const(LogicVec::from_u64(99, 8))),
            )),
            Box::new(IrExpr::Const(LogicVec::from_u64(0, 8))),
            Box::new(IrExpr::BinaryOp(
                BinaryIrOp::Add,
                Box::new(IrExpr::Signal(2, 8)),
                Box::new(IrExpr::Const(LogicVec::from_u64(1, 8))),
            )),
        );
        ir.top.processes.push(Process::Sequential {
            name: Symbol::intern("ff_count"),
            clock: ClockEdge::PosEdge(0),
            reset: Some(ResetInfo {
                signal: 1,
                polarity: false,
                r#async: true,
                value: LogicVec::from_u64(0, 8),
            }),
            body: vec![IrStmt::NonBlockingAssign {
                lhs: IrLValue::Signal(2, 8),
                rhs,
                delay: None,
            }],
            iff: None,
        });
        ir
    }

    #[test]
    fn lower_counter_to_sir() {
        let ir = counter_ir();
        let out = lower(&ir);
        let m = &out.module;
        assert_eq!(m.name.as_str(), "counter");
        assert_eq!(m.inputs.len(), 2);
        assert_eq!(m.outputs.len(), 1);
        assert_eq!(m.register_count(), 1);

        // Register count: Q = Reg, D = MUX(EQ, 0, ADD(count,1)).
        let reg = &m.registers[0];
        assert_eq!(reg.name.as_str(), "count");
        assert_eq!(reg.width, 8);
        assert!(matches!(m.values[reg.q], SirValue::Reg(0)));
        assert!(matches!(reg.reset, Some(ResetSpec { r#async: true, .. })));

        // D-logic: harus ada node EQ, ADD, MUX.
        let kinds: Vec<&SirNodeKind> = m.nodes.iter().map(|n| &n.kind).collect();
        assert!(kinds.contains(&&SirNodeKind::Eq), "harus ada EQ count==99");
        assert!(kinds.contains(&&SirNodeKind::Add), "harus ada ADD count+1");
        assert!(kinds.contains(&&SirNodeKind::Mux), "harus ada MUX");

        // Baca graph: d = Mux; cek input MUX memuat hasil EQ.
        let d = reg.d;
        assert!(matches!(m.values[d], SirValue::Node(_)));
        let d_node = match m.values[d] {
            SirValue::Node(n) => n,
            _ => panic!("d harus node"),
        };
        assert_eq!(m.nodes[d_node].kind, SirNodeKind::Mux);
    }

    #[test]
    fn lower_sequential_if_hold_semantics() {
        // count <= 1; if (en) count <= count + 1; — dengan reset.
        // Tanpa en: hold (RegQ). Dengan en: ADD.
        let mut ir = base_design();
        let sig = |name: &str, width: usize, kind: SignalKind| {
            use mivon_ir::SignalInfo;
            SignalInfo {
                name: Symbol::intern(name),
                width,
                kind,
                ..Default::default()
            }
        };
        ir.top.signals.push(sig("clk", 1, SignalKind::Input));
        ir.top.signals.push(sig("en", 1, SignalKind::Input));
        ir.top.signals.push(sig("count", 8, SignalKind::Output));
        ir.top.inputs = vec![0, 1];
        ir.top.outputs = vec![2];
        let body = vec![
            IrStmt::NonBlockingAssign {
                lhs: IrLValue::Signal(2, 8),
                rhs: IrExpr::Const(LogicVec::from_u64(1, 8)),
                delay: None,
            },
            IrStmt::If {
                cond: IrExpr::Signal(1, 1),
                true_branch: vec![IrStmt::NonBlockingAssign {
                    lhs: IrLValue::Signal(2, 8),
                    rhs: IrExpr::BinaryOp(
                        BinaryIrOp::Add,
                        Box::new(IrExpr::Signal(2, 8)),
                        Box::new(IrExpr::Const(LogicVec::from_u64(1, 8))),
                    ),
                    delay: None,
                }],
                false_branch: vec![],
            },
        ];
        ir.top.processes.push(Process::Sequential {
            name: Symbol::intern("ff_count"),
            clock: ClockEdge::PosEdge(0),
            reset: None,
            body,
            iff: None,
        });
        let out = lower(&ir);
        assert!(out.skipped.is_empty(), "skip: {:?}", out.skipped);
        let m = &out.module;
        assert_eq!(m.register_count(), 1);
        // D harus MUX(cond=en, ADD, hold=RegQ).
        let d = m.registers[0].d;
        let nid = match m.values[d] {
            SirValue::Node(n) => n,
            _ => panic!("d harus node mux"),
        };
        assert_eq!(m.nodes[nid].kind, SirNodeKind::Mux);
    }

    #[test]
    fn lower_assign_then_conditional_uses_earlier_value() {
        // Regresi review: `q <= 0; if (c) q <= 2;` harus menghasilkan
        // D = Mux(c, 2, 0) — saat !c, q mengambil nilai 0 dari statement
        // sebelumnya, BUKAN hold RegQ.
        let mut ir = base_design();
        let sig = |name: &str, width: usize, kind: SignalKind| {
            use mivon_ir::SignalInfo;
            SignalInfo {
                name: Symbol::intern(name),
                width,
                kind,
                ..Default::default()
            }
        };
        ir.top.signals.push(sig("clk", 1, SignalKind::Input));
        ir.top.signals.push(sig("c", 1, SignalKind::Input));
        ir.top.signals.push(sig("q", 8, SignalKind::Output));
        ir.top.inputs = vec![0, 1];
        ir.top.outputs = vec![2];
        let body = vec![
            IrStmt::NonBlockingAssign {
                lhs: IrLValue::Signal(2, 8),
                rhs: IrExpr::Const(LogicVec::from_u64(0, 8)),
                delay: None,
            },
            IrStmt::If {
                cond: IrExpr::Signal(1, 1),
                true_branch: vec![IrStmt::NonBlockingAssign {
                    lhs: IrLValue::Signal(2, 8),
                    rhs: IrExpr::Const(LogicVec::from_u64(2, 8)),
                    delay: None,
                }],
                false_branch: vec![],
            },
        ];
        ir.top.processes.push(Process::Sequential {
            name: Symbol::intern("p"),
            clock: ClockEdge::PosEdge(0),
            reset: None,
            body,
            iff: None,
        });
        let out = lower(&ir);
        let m = &out.module;
        let d = m.registers[0].d;
        let nid = match m.values[d] {
            SirValue::Node(n) => n,
            _ => panic!("D harus node Mux"),
        };
        assert_eq!(m.nodes[nid].kind, SirNodeKind::Mux);
        let f = m.nodes[nid].inputs[2];
        assert!(
            matches!(m.values[f], SirValue::Const(_)),
            "fallback !c harus nilai statement sebelumnya (Const 0), bukan hold RegQ"
        );
    }

    #[test]
    fn lower_comb_dag() {
        // Combinational: y = a & b; z = y | c
        let mut ir = base_design();
        let sig = |name: &str, width: usize, kind: SignalKind| {
            use mivon_ir::SignalInfo;
            SignalInfo {
                name: Symbol::intern(name),
                width,
                kind,
                ..Default::default()
            }
        };
        ir.top.signals.push(sig("a", 4, SignalKind::Input));
        ir.top.signals.push(sig("b", 4, SignalKind::Input));
        ir.top.signals.push(sig("c", 4, SignalKind::Input));
        ir.top.signals.push(sig("y", 4, SignalKind::Wire));
        ir.top.signals.push(sig("z", 4, SignalKind::Output));
        ir.top.inputs = vec![0, 1, 2];
        ir.top.outputs = vec![4];
        let y_rhs = IrExpr::BinaryOp(
            BinaryIrOp::BitAnd,
            Box::new(IrExpr::Signal(0, 4)),
            Box::new(IrExpr::Signal(1, 4)),
        );
        let z_rhs = IrExpr::BinaryOp(
            BinaryIrOp::BitOr,
            Box::new(IrExpr::Signal(3, 4)),
            Box::new(IrExpr::Signal(2, 4)),
        );
        ir.top.processes.push(Process::Combinational {
            name: Symbol::intern("comb"),
            sensitivity: vec![],
            body: vec![
                IrStmt::BlockingAssign {
                    lhs: IrLValue::Signal(3, 4),
                    rhs: y_rhs,
                    delay: None,
                },
                IrStmt::BlockingAssign {
                    lhs: IrLValue::Signal(4, 4),
                    rhs: z_rhs,
                    delay: None,
                },
            ],
        });
        let out = lower(&ir);
        assert!(out.skipped.is_empty(), "skip: {:?}", out.skipped);
        let m = &out.module;
        assert_eq!(m.node_count(), 2);
        let kinds: Vec<&SirNodeKind> = m.nodes.iter().map(|n| &n.kind).collect();
        assert!(kinds.contains(&&SirNodeKind::And));
        assert!(kinds.contains(&&SirNodeKind::Or));
    }

    #[test]
    fn lower_reports_unsupported_expr() {
        // Panggil API internal lewat design yang memuat method call (skip).
        let mut ir = base_design();
        let sig = |name: &str, width: usize, kind: SignalKind| {
            use mivon_ir::SignalInfo;
            SignalInfo {
                name: Symbol::intern(name),
                width,
                kind,
                ..Default::default()
            }
        };
        ir.top.signals.push(sig("clk", 1, SignalKind::Input));
        ir.top.signals.push(sig("q", 8, SignalKind::Output));
        ir.top.inputs = vec![0];
        ir.top.outputs = vec![1];
        let bad = IrExpr::MethodCall {
            obj: Box::new(IrExpr::Signal(1, 8)),
            method: Symbol::intern("foo"),
            args: vec![],
            with_clause: None,
        };
        ir.top.processes.push(Process::Sequential {
            name: Symbol::intern("p"),
            clock: ClockEdge::PosEdge(0),
            reset: None,
            body: vec![IrStmt::NonBlockingAssign {
                lhs: IrLValue::Signal(1, 8),
                rhs: bad,
                delay: None,
            }],
            iff: None,
        });
        let out = lower(&ir);
        assert!(
            !out.skipped.is_empty(),
            "method call harus dilaporkan skipped"
        );
    }
}
