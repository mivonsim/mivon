//! Inferensi primitif RTL (SYNTHESIS.md §5.3) + pembangunan netlist pra-map.
//!
//! Fase S1:
//! - Port top-level → `Port` (+ IO count).
//! - `Process::Sequential` → satu `DffR`/`DffRE` per signal yang di-assign
//!   non-blocking (reset value dari `ResetInfo.value`, clock enable dari `iff`).
//! - Ekspresi kombinasional → estimasi LUT/carry/DSP (heuristik S1; mapping
//!   penuh di S2).
//! - Clock & reset → net bertanda + `Bufg`.
//! - Array besar → estimasi BRAM/ROM (inferensi penuh di S3).

use mivon_core::intern::Symbol;
use mivon_ir::{ClockEdge, IrDesign, IrExpr, IrLValue, IrStmt, LogicVec, Process, SignalId};
use std::collections::HashSet;

use crate::netlist::{CellKind, DeviceKind, Netlist, Port, PortDir};

/// Opsi synthesis (S1: subset kecil — device saja).
#[derive(Debug, Clone)]
pub struct SynthOpts {
    pub device: DeviceKind,
}

impl Default for SynthOpts {
    fn default() -> Self {
        SynthOpts {
            device: DeviceKind::FpgaX7,
        }
    }
}

/// Bangun netlist pra-map dari IrDesign.
pub fn infer_netlist(ir: &IrDesign, opts: &SynthOpts) -> Netlist {
    let top = &ir.top;
    let mut nl = Netlist::new(top.name, opts.device.clone());

    // ── Sinyal → net ──
    // SignalId = index di `top.signals`. Buat net untuk semua, tandai port IO.
    let mut signal_net: Vec<Option<crate::netlist::NetId>> = vec![None; top.signals.len()];
    for (id, s) in top.signals.iter().enumerate() {
        let name = s.name;
        let net_id = nl.add_net(name, s.width.max(1));
        signal_net[id] = Some(net_id);
    }

    // ── Port top-level ──
    for id in &top.inputs {
        let s = &top.signals[*id];
        let net_id = signal_net[*id].expect("net port input");
        nl.nets[net_id].is_io = true;
        nl.ports.push(Port {
            name: s.name,
            dir: PortDir::Input,
            width: s.width.max(1),
        });
        nl.stats.io_count += 1;
    }
    for id in &top.outputs {
        let s = &top.signals[*id];
        let net_id = signal_net[*id].expect("net port output");
        nl.nets[net_id].is_io = true;
        nl.ports.push(Port {
            name: s.name,
            dir: PortDir::Output,
            width: s.width.max(1),
        });
        nl.stats.io_count += 1;
    }
    for id in &top.inouts {
        let s = &top.signals[*id];
        let net_id = signal_net[*id].expect("net port inout");
        nl.nets[net_id].is_io = true;
        nl.ports.push(Port {
            name: s.name,
            dir: PortDir::Inout,
            width: s.width.max(1),
        });
        nl.stats.io_count += 1;
    }

    // ── Proses ──
    // Kumpulkan nama signal yang di-drive sequential (FF) — termasuk dari
    // sub-modul? S1: hanya modul top (flatten) yang di-infer; sub-modul
    // direpresentasikan sebagai `PassThrough` (referensi — S2 memperluas).
    let mut driven_sequential: HashSet<SignalId> = HashSet::new();
    let mut clocks: HashSet<SignalId> = HashSet::new();
    let mut resets: HashSet<SignalId> = HashSet::new();
    let mut logic_nodes = 0usize;
    let mut carry_nodes = 0usize;
    let mut dsp_nodes = 0usize;
    let mut fsm_hint = 0usize;

    for p in &top.processes {
        match p {
            Process::Sequential {
                clock,
                reset,
                iff,
                body,
                ..
            } => {
                nl.stats.process_count += 1;
                // Clock net → Bufg (satu per clock).
                match clock {
                    ClockEdge::PosEdge(id) | ClockEdge::NegEdge(id) => {
                        if clocks.insert(*id) {
                            if let Some(net) = signal_net[*id] {
                                nl.nets[net].is_clock = true;
                            }
                        }
                    }
                    ClockEdge::PosEdgeHier(_) | ClockEdge::NegEdgeHier(_) => {}
                }
                // Reset net.
                if let Some(r) = reset {
                    resets.insert(r.signal);
                    if let Some(net) = signal_net[r.signal] {
                        nl.nets[net].is_reset = true;
                    }
                }
                // FF inference: cari semua NonBlockingAssign ke signal.
                let ff_targets = collect_ff_targets(body);
                for (sig_id, _width) in ff_targets {
                    if driven_sequential.insert(sig_id) {
                        let s = &top.signals[sig_id];
                        // Q = net register itu sendiri (output register berasal
                        // dari Q FF — benar secara semantik).
                        let q_net = signal_net[sig_id].expect("FF q-net");
                        // Enable (`iff`): S1 hanya mendukung enable berupa signal
                        // tunggal. Ekspresi kompleks jatuh ke FF tanpa enable
                        // (konservatif — tidak membuat net mengambang).
                        let ce_net = match iff {
                            Some(IrExpr::Signal(id, _)) => signal_net[*id],
                            _ => None,
                        };
                        let kind = match (reset, ce_net) {
                            (Some(r), Some(_)) => CellKind::DffRE {
                                reset_value: logicvec_bits(&r.value),
                            },
                            (Some(r), None) => CellKind::DffR {
                                reset_value: logicvec_bits(&r.value),
                            },
                            (None, Some(_)) => CellKind::DffE,
                            (None, None) => CellKind::Dff,
                        };
                        let clk_net = match clock {
                            ClockEdge::PosEdge(id) | ClockEdge::NegEdge(id) => {
                                signal_net[*id].unwrap_or(0)
                            }
                            // Clock hierarkis tidak ter-resolve di S1 — FF tetap
                            // dibuat tanpa koneksi clock (di-refine di S2).
                            _ => 0,
                        };
                        // D-logic: net `<name>_d`, akan di-drive oleh teknologi
                        // mapping S2. Di S1 D mengambang (belum ada logic).
                        let d_net = nl.add_net(
                            Symbol::intern(&format!("{}_d", s.name.as_str())),
                            s.width.max(1),
                        );
                        let inst = crate::netlist::ff_instance(
                            Symbol::intern(&format!("ff_{}", s.name.as_str())),
                            kind.clone(),
                            clk_net,
                            d_net,
                            q_net,
                            reset.as_ref().map(|r| {
                                (signal_net[r.signal].unwrap_or(0), logicvec_bits(&r.value))
                            }),
                            ce_net,
                        );
                        nl.add_instance(inst);
                        nl.stats.ff_count += 1;
                    }
                }
                // FSM hint: case selector yang merupakan signal reg.
                fsm_hint += count_fsm_cases(body);
                // D-logic: ekspresi kombinasional yang memberi makan D FF juga
                // dihitung sebagai node logika (adder/komparator di dalam proses
                // sequential). `estimate_combinational` sudah menangani
                // NonBlockingAssign RHS + if/case/block.
                let (l, c, d) = estimate_combinational(body);
                logic_nodes += l;
                carry_nodes += c;
                dsp_nodes += d;
            }
            Process::Combinational { body, .. } | Process::CombReactive { body, .. } => {
                nl.stats.process_count += 1;
                let (l, c, d) = estimate_combinational(body);
                logic_nodes += l;
                carry_nodes += c;
                dsp_nodes += d;
            }
            Process::Initial { .. } | Process::Final { .. } | Process::AlwaysWithDelay { .. } => {
                // Testbench — diabaikan (SYN check sudah melaporkan).
            }
        }
    }

    nl.stats.logic_nodes = logic_nodes;
    nl.stats.carry4_count = carry_nodes;
    nl.stats.dsp_count = dsp_nodes;
    nl.stats.fsm_count = fsm_hint;

    // Bufg per clock (net is_clock).
    for net in nl.nets.iter() {
        if net.is_clock {
            nl.stats.bufg_count += 1;
        }
    }

    // ── Estimasi LUT (heuristik S1) ──
    // 1 LUT per node logika dasar + 1 LUT per op carry (generate). Untuk S1,
    // node dihitung per operator (bukan per bit) — over-estimasi konservatif
    // yang di-refine di S2 (technology mapping nyata). CARRY4 dihitung
    // terpisah sebagai unit chain.
    nl.stats.lut_count = logic_nodes + carry_nodes;

    // ── Estimasi memori (heuristik S1 — inferensi penuh S3) ──
    for s in &top.signals {
        if s.array_depth > 0 {
            let total_bits = s.array_depth.saturating_mul(s.elem_width.max(1));
            if total_bits >= 512 && s.elem_width >= 8 {
                // ROM bila ada init; estimasi konservatif sebagai RAM.
                nl.stats.mem_bits += total_bits;
                if !s.init_val.all_x() {
                    nl.stats.rom_count += 1;
                } else {
                    nl.stats.bram_count += 1;
                }
            }
        }
    }

    // ── src_map: signal → (file, 0, 0) — S1 tanpa posisi baris/kolom. ──
    let file = ir.source_file.clone().unwrap_or_else(|| "<unknown>".into());
    for s in &top.signals {
        nl.src_map.insert(s.name, (file.clone(), 0, 0));
    }

    nl
}

/// Ekstrak bit integer dari LogicVec (LSB-first), clamp ke u64.
fn logicvec_bits(v: &LogicVec) -> u64 {
    v.to_u64()
}

/// Kumpulkan (signal_id, width) yang di-assign non-blocking di dalam body
/// (rekursif termasuk if/case/block).
fn collect_ff_targets(stmts: &[IrStmt]) -> Vec<(SignalId, usize)> {
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

/// Estimasi jumlah node logika / carry / DSP dalam proses combinational.
fn estimate_combinational(stmts: &[IrStmt]) -> (usize, usize, usize) {
    let mut logic = 0usize;
    let mut carry = 0usize;
    let mut dsp = 0usize;
    for st in stmts {
        match st {
            IrStmt::BlockingAssign { rhs, .. } | IrStmt::NonBlockingAssign { rhs, .. } => {
                count_logic(rhs, &mut logic, &mut carry, &mut dsp);
            }
            IrStmt::Block { stmts, .. } | IrStmt::NamedBlock { stmts, .. } => {
                let (l, c, d) = estimate_combinational(stmts);
                logic += l;
                carry += c;
                dsp += d;
            }
            IrStmt::If {
                true_branch,
                false_branch,
                ..
            } => {
                let (l, c, d) = estimate_combinational(true_branch);
                logic += l;
                carry += c;
                dsp += d;
                let (l, c, d) = estimate_combinational(false_branch);
                logic += l;
                carry += c;
                dsp += d;
            }
            _ => {}
        }
    }
    (logic, carry, dsp)
}

// IrExpr adalah tree (tanpa cycle) — tidak butuh guard rekursi. Dedup global
// justru meng-underestimate: subekspresi identik di statement berbeda
// (mis. `a <= x+y; b <= x+y;`) dihitung sekali padahal 2 node logika.
fn count_logic(e: &IrExpr, logic: &mut usize, carry: &mut usize, dsp: &mut usize) {
    match e {
        IrExpr::UnaryOp(_, inner) => {
            *logic += 1;
            count_logic(inner, logic, carry, dsp);
        }
        IrExpr::BinaryOp(op, a, b) => {
            match op {
                mivon_ir::BinaryIrOp::Add
                | mivon_ir::BinaryIrOp::Sub
                | mivon_ir::BinaryIrOp::Lt
                | mivon_ir::BinaryIrOp::Le
                | mivon_ir::BinaryIrOp::Gt
                | mivon_ir::BinaryIrOp::Ge => *carry += 1,
                mivon_ir::BinaryIrOp::Mul | mivon_ir::BinaryIrOp::Power => *dsp += 1,
                _ => *logic += 1,
            }
            count_logic(a, logic, carry, dsp);
            count_logic(b, logic, carry, dsp);
        }
        IrExpr::Cond(c, a, b) => {
            *logic += 1;
            count_logic(c, logic, carry, dsp);
            count_logic(a, logic, carry, dsp);
            count_logic(b, logic, carry, dsp);
        }
        IrExpr::Concat(parts) => {
            for p in parts {
                count_logic(p, logic, carry, dsp);
            }
        }
        IrExpr::Replicate(_, inner) => count_logic(inner, logic, carry, dsp),
        // RangeSelect/BitSelect membawa SignalId (usize), bukan ekspresi.
        IrExpr::Signed(inner)
        | IrExpr::ExprRangeSelect(inner, _, _)
        | IrExpr::ExprBitSelect(inner, _)
        | IrExpr::Cast { expr: inner, .. } => count_logic(inner, logic, carry, dsp),
        IrExpr::RangeSelect(_, _, _) | IrExpr::BitSelect(_, _) => {}
        IrExpr::ExprPartSelect(base, idx, width) => {
            count_logic(base, logic, carry, dsp);
            count_logic(idx, logic, carry, dsp);
            count_logic(width, logic, carry, dsp);
        }
        IrExpr::ArrayIndex { index, .. } => count_logic(index, logic, carry, dsp),
        _ => {}
    }
}

/// Hitung jumlah `case` yang selector-nya signal (FSM hint).
fn count_fsm_cases(stmts: &[IrStmt]) -> usize {
    let mut n = 0usize;
    walk_case_exprs(stmts, &mut n);
    n
}

fn walk_case_exprs(stmts: &[IrStmt], n: &mut usize) {
    for st in stmts {
        match st {
            IrStmt::Case { expr, items, .. } => {
                if matches!(expr, IrExpr::Signal(_, _)) && items.len() >= 2 {
                    *n += 1;
                }
                for it in items {
                    walk_case_exprs(&it.body, n);
                }
            }
            IrStmt::Block { stmts, .. } | IrStmt::NamedBlock { stmts, .. } => {
                walk_case_exprs(stmts, n)
            }
            IrStmt::If {
                true_branch,
                false_branch,
                ..
            } => {
                walk_case_exprs(true_branch, n);
                walk_case_exprs(false_branch, n);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mivon_ir::{BinaryIrOp, IrModule, SignalInfo, SignalKind, SignalSensitivity};
    use std::collections::HashMap;

    fn counter_ir() -> IrDesign {
        // counter 8-bit sederhana: clk, rst_n, enable in; count out.
        let mut ir = IrDesign {
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
            stmt_lines: std::collections::HashMap::new(),
            net_aliases: std::collections::HashMap::new(),
        };
        use mivon_ir::{SignalInfo, SignalKind};
        let sig = |name: &str, width: usize, kind: SignalKind| SignalInfo {
            name: Symbol::intern(name),
            width,
            kind,
            ..Default::default()
        };
        ir.top.signals.push(sig("clk", 1, SignalKind::Input));
        ir.top.signals.push(sig("rst_n", 1, SignalKind::Input));
        ir.top.signals.push(sig("enable", 1, SignalKind::Input));
        ir.top.signals.push(sig("count", 8, SignalKind::Output));
        ir.top.inputs = vec![0usize, 1, 2];
        ir.top.outputs = vec![3usize];
        // Sequential: count <= (count == 99) ? 0 : count + 1
        let lhs = IrLValue::Signal(3, 8);
        let rhs = IrExpr::Cond(
            Box::new(IrExpr::BinaryOp(
                mivon_ir::BinaryIrOp::Eq,
                Box::new(IrExpr::Signal(3, 8)),
                Box::new(IrExpr::Const(LogicVec::from_u64(99, 8))),
            )),
            Box::new(IrExpr::Const(LogicVec::from_u64(0, 8))),
            Box::new(IrExpr::BinaryOp(
                mivon_ir::BinaryIrOp::Add,
                Box::new(IrExpr::Signal(3, 8)),
                Box::new(IrExpr::Const(LogicVec::from_u64(1, 8))),
            )),
        );
        ir.top.processes.push(Process::Sequential {
            name: Symbol::intern("ff_count"),
            clock: ClockEdge::PosEdge(0),
            reset: Some(mivon_ir::ResetInfo {
                signal: 1,
                polarity: false,
                r#async: true,
                value: LogicVec::from_u64(0, 8),
            }),
            body: vec![IrStmt::NonBlockingAssign {
                lhs,
                rhs,
                delay: None,
            }],
            iff: None,
        });
        ir
    }

    #[test]
    fn infer_counter_ff_and_ports() {
        let ir = counter_ir();
        let opts = SynthOpts::default();
        let nl = infer_netlist(&ir, &opts);
        assert_eq!(nl.ports.len(), 4);
        assert_eq!(nl.stats.io_count, 4);
        assert_eq!(nl.stats.ff_count, 1);
        assert_eq!(nl.ffs().count(), 1);
        // D-logic di proses sequential ikut dihitung: Cond + Eq (2 LUT)
        // + Add (1 carry) → LUT est = 3, CARRY4 = 1.
        assert_eq!(
            nl.stats.lut_count, 3,
            "adder/komparator di proses sequential harus dihitung"
        );
        assert_eq!(nl.stats.carry4_count, 1);
        // clock & reset ditandai
        assert!(nl.nets.iter().any(|n| n.is_clock));
        assert!(nl.nets.iter().any(|n| n.is_reset));
    }

    #[test]
    fn logicvec_bits_roundtrip() {
        let v = LogicVec::from_u64(0b1010, 4);
        assert_eq!(logicvec_bits(&v), 0b1010);
    }

    #[test]
    fn infer_empty_design() {
        let ir = IrDesign {
            top: mivon_ir::IrModule {
                name: Symbol::intern("empty"),
                ..Default::default()
            },
            ..Default::default()
        };
        let opts = SynthOpts::default();
        let nl = infer_netlist(&ir, &opts);
        assert_eq!(nl.ports.len(), 0);
        assert_eq!(nl.nets.len(), 0);
        assert_eq!(nl.stats.io_count, 0);
        assert_eq!(nl.stats.ff_count, 0);
        assert_eq!(nl.stats.lut_count, 0);
    }

    #[test]
    fn infer_combinational_process() {
        let mut ir = IrDesign {
            top: mivon_ir::IrModule {
                name: Symbol::intern("comb_only"),
                signals: vec![
                    SignalInfo {
                        name: Symbol::from("a"),
                        width: 8,
                        kind: SignalKind::Input,
                        ..Default::default()
                    },
                    SignalInfo {
                        name: Symbol::from("y"),
                        width: 8,
                        kind: SignalKind::Output,
                        ..Default::default()
                    },
                ],
                inputs: vec![0],
                outputs: vec![1],
                inouts: vec![],
                processes: vec![Process::CombReactive {
                    name: Symbol::from("comb"),
                    sensitivity: vec![SignalSensitivity::whole(0)],
                    body: vec![IrStmt::BlockingAssign {
                        lhs: IrLValue::Signal(1, 8),
                        rhs: IrExpr::BinaryOp(
                            BinaryIrOp::Add,
                            Box::new(IrExpr::Signal(0, 8)),
                            Box::new(IrExpr::Const(LogicVec::from_u64(1, 8))),
                        ),
                        delay: None,
                    }],
                }],
                sub_instances: vec![],
            },
            modules: HashMap::new(),
            classes: HashMap::new(),
            covergroups: vec![],
            dpi_imports: vec![],
            hier_signal_map: HashMap::new(),
            udp_defs: vec![],
            specify_items: vec![],
            timescale: None,
            source_file: None,
            source_lines: None,
            module_functions: HashMap::new(),
            pkg_scoped_consts: HashMap::new(),
            coverage_exclusions: Vec::new(),
            stmt_lines: HashMap::new(),
            net_aliases: HashMap::new(),
        };
        let opts = SynthOpts::default();
        let nl = infer_netlist(&ir, &opts);
        assert_eq!(nl.ports.len(), 2);
        assert_eq!(nl.stats.io_count, 2);
        assert!(nl.stats.lut_count > 0);
        assert_eq!(nl.stats.ff_count, 0);
    }

    #[test]
    fn infer_multiple_ff() {
        let mut ir = IrDesign {
            top: IrModule {
                name: Symbol::intern("multi_ff"),
                signals: vec![
                    SignalInfo {
                        name: Symbol::from("clk"),
                        width: 1,
                        kind: SignalKind::Input,
                        ..Default::default()
                    },
                    SignalInfo {
                        name: Symbol::from("rst"),
                        width: 1,
                        kind: SignalKind::Input,
                        ..Default::default()
                    },
                    SignalInfo {
                        name: Symbol::from("q1"),
                        width: 8,
                        kind: SignalKind::Output,
                        ..Default::default()
                    },
                    SignalInfo {
                        name: Symbol::from("q2"),
                        width: 8,
                        kind: SignalKind::Output,
                        ..Default::default()
                    },
                ],
                inputs: vec![0, 1],
                outputs: vec![2, 3],
                inouts: vec![],
                processes: vec![
                    Process::Sequential {
                        name: Symbol::from("ff1"),
                        clock: ClockEdge::PosEdge(0),
                        reset: None,
                        body: vec![IrStmt::NonBlockingAssign {
                            lhs: IrLValue::Signal(2, 8),
                            rhs: IrExpr::Const(LogicVec::from_u64(1, 8)),
                            delay: None,
                        }],
                        iff: None,
                    },
                    Process::Sequential {
                        name: Symbol::from("ff2"),
                        clock: ClockEdge::PosEdge(0),
                        reset: None,
                        body: vec![IrStmt::NonBlockingAssign {
                            lhs: IrLValue::Signal(3, 8),
                            rhs: IrExpr::Const(LogicVec::from_u64(2, 8)),
                            delay: None,
                        }],
                        iff: None,
                    },
                ],
                sub_instances: vec![],
            },
            modules: HashMap::new(),
            classes: HashMap::new(),
            covergroups: vec![],
            dpi_imports: vec![],
            hier_signal_map: HashMap::new(),
            udp_defs: vec![],
            specify_items: vec![],
            timescale: None,
            source_file: None,
            source_lines: None,
            module_functions: HashMap::new(),
            pkg_scoped_consts: HashMap::new(),
            coverage_exclusions: Vec::new(),
            stmt_lines: HashMap::new(),
            net_aliases: HashMap::new(),
        };
        let opts = SynthOpts::default();
        let nl = infer_netlist(&ir, &opts);
        assert_eq!(nl.stats.ff_count, 2);
        assert_eq!(nl.stats.io_count, 4);
    }

    #[test]
    fn infer_multiple_clocks() {
        let mut ir = IrDesign {
            top: IrModule {
                name: Symbol::intern("multi_clk"),
                signals: vec![
                    SignalInfo {
                        name: Symbol::from("clk1"),
                        width: 1,
                        kind: SignalKind::Input,
                        ..Default::default()
                    },
                    SignalInfo {
                        name: Symbol::from("clk2"),
                        width: 1,
                        kind: SignalKind::Input,
                        ..Default::default()
                    },
                    SignalInfo {
                        name: Symbol::from("q"),
                        width: 8,
                        kind: SignalKind::Output,
                        ..Default::default()
                    },
                    SignalInfo {
                        name: Symbol::from("q2"),
                        width: 8,
                        kind: SignalKind::Output,
                        ..Default::default()
                    },
                ],
                inputs: vec![0, 1],
                outputs: vec![2, 3],
                inouts: vec![],
                processes: vec![
                    Process::Sequential {
                        name: Symbol::from("ff1"),
                        clock: ClockEdge::PosEdge(0),
                        reset: None,
                        body: vec![IrStmt::NonBlockingAssign {
                            lhs: IrLValue::Signal(2, 8),
                            rhs: IrExpr::Const(LogicVec::from_u64(1, 8)),
                            delay: None,
                        }],
                        iff: None,
                    },
                    Process::Sequential {
                        name: Symbol::from("ff2"),
                        clock: ClockEdge::PosEdge(1),
                        reset: None,
                        body: vec![IrStmt::NonBlockingAssign {
                            lhs: IrLValue::Signal(3, 8),
                            rhs: IrExpr::Const(LogicVec::from_u64(2, 8)),
                            delay: None,
                        }],
                        iff: None,
                    },
                ],
                sub_instances: vec![],
            },
            modules: HashMap::new(),
            classes: HashMap::new(),
            covergroups: vec![],
            dpi_imports: vec![],
            hier_signal_map: HashMap::new(),
            udp_defs: vec![],
            specify_items: vec![],
            timescale: None,
            source_file: None,
            source_lines: None,
            module_functions: HashMap::new(),
            pkg_scoped_consts: HashMap::new(),
            coverage_exclusions: Vec::new(),
            stmt_lines: HashMap::new(),
            net_aliases: HashMap::new(),
        };
        let opts = SynthOpts::default();
        let nl = infer_netlist(&ir, &opts);
        assert_eq!(nl.stats.ff_count, 2);
        assert_eq!(nl.nets.iter().filter(|n| n.is_clock).count(), 2);
    }
}
