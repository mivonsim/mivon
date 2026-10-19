//! Clock Domain Analysis — Cycle-Based Simulation Fusion.
//!
//! # Clock-Gated Fusion Strategy
//!
//! 1. **Analisis**: Identifikasi clock signal + edge untuk setiap Sequential process.
//!    Grouping by `(clock_signal_id, edge_type)`.
//!
//! 2. **Fusion**: Untuk setiap clock domain, kumpulkan:
//!    - Sequential processes yang triggered oleh clock edge
//!    - Combinational processes yang membaca sinyal output dari sequential processes
//!
//! 3. **Evaluasi**: Saat clock edge terdeteksi, evaluasi SEMUA process dalam domain
//!    secara berurutan (sequential → combinational → repeat sampai stabil).
//!    Event queue overhead di-skip karena semua process dalam domain sinkronus.
//!
//! 4. **Safety**: Processes yang mengandung delay, event control, atau system calls
//!    TIDAK boleh di-fuse — mereka tetap pakai event-driven evaluation.

use mivon_ir::*;
use std::collections::{HashMap, HashSet};

// ─── Types ───

/// Clock edge type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClockEdgeType {
    PosEdge,
    NegEdge,
}

/// Sebuah clock domain — grup process sinkronus yang di-fuse.
///
/// Semua process dalam domain ini triggered oleh clock edge yang sama.
/// Kombinasi sequential + follower combinational processes dievaluasi
/// sebagai satu unit fused, tanpa event queue overhead.
#[derive(Debug, Clone)]
pub struct ClockDomain {
    /// Clock signal ID.
    pub clock_signal: SignalId,
    /// Edge type (posedge/negedge).
    pub edge: ClockEdgeType,
    /// Sequential process IDs dalam domain ini.
    pub sequential_processes: Vec<usize>,
    /// Combinational process IDs yang mengikuti sequential processes.
    /// Process ini membaca sinyal yang ditulis oleh sequential processes
    /// dalam domain ini.
    pub follower_processes: Vec<usize>,
    /// Total processes dalam domain ini.
    pub total_processes: usize,
}

/// Hasil analisis clock domain untuk seluruh desain.
#[derive(Debug, Clone)]
pub struct ClockDomainAnalysis {
    /// Semua clock domain yang terdeteksi.
    pub domains: Vec<ClockDomain>,
    /// Set of process IDs yang sudah di-fuse (skip di event loop).
    pub fused_processes: HashSet<usize>,
}

impl ClockDomainAnalysis {
    /// Analyze all processes and build clock domains.
    ///
    /// # Algorithm
    ///
    /// 1. Kumpulkan semua Sequential process, group by (clock, edge)
    /// 2. Untuk setiap group, cari Combinational/CombReactive processes
    ///    yang membaca sinyal output dari group tersebut (follower processes)
    /// 3. Simpan fused_processes set untuk skip di event loop
    pub fn analyze(design: &IrDesign) -> Self {
        let num_processes = design.top.processes.len();
        let processes = &design.top.processes;
        let mut domains: Vec<ClockDomain> = Vec::new();
        let mut fused_processes: HashSet<usize> = HashSet::new();

        if num_processes == 0 {
            return ClockDomainAnalysis {
                domains: Vec::new(),
                fused_processes: HashSet::new(),
            };
        }

        // Phase 1: Group Sequential processes by (clock_signal, edge)
        let mut clock_groups: HashMap<(SignalId, ClockEdgeType), Vec<usize>> = HashMap::new();

        for (pid, process) in processes.iter().enumerate() {
            if let Process::Sequential { clock, .. } = process {
                let key = match clock {
                    ClockEdge::PosEdge(sig_id) => (*sig_id, ClockEdgeType::PosEdge),
                    ClockEdge::NegEdge(sig_id) => (*sig_id, ClockEdgeType::NegEdge),
                    // F27: clock hierarkis (`b.clk` via port interface) — resolve
                    // path Symbol via hier_signal_map (design sudah di-flatten)
                    // agar domain fusion mengelompokkan clock interface yang
                    // SAMA; jangan pakai key tunggal (bisa salah-fuse).
                    ClockEdge::PosEdgeHier(s) => (
                        design.hier_signal_map.get(s).copied().unwrap_or_else(|| {
                            let mut h = 0x9e37_79b9usize;
                            for b in s.as_str().bytes() {
                                h = h.wrapping_mul(31).wrapping_add(b as usize);
                            }
                            h
                        }),
                        ClockEdgeType::PosEdge,
                    ),
                    ClockEdge::NegEdgeHier(s) => (
                        design.hier_signal_map.get(s).copied().unwrap_or_else(|| {
                            let mut h = 0x9e37_79b9usize;
                            for b in s.as_str().bytes() {
                                h = h.wrapping_mul(31).wrapping_add(b as usize);
                            }
                            h
                        }),
                        ClockEdgeType::NegEdge,
                    ),
                };
                clock_groups.entry(key).or_default().push(pid);
            }
        }

        if clock_groups.is_empty() {
            return ClockDomainAnalysis {
                domains: Vec::new(),
                fused_processes: HashSet::new(),
            };
        }

        // Phase 2: For each clock group, find follower combinational processes.
        // A combinational process is a follower if it reads ANY signal
        // that is written by a sequential process in this clock group.
        let seq_writes: Vec<HashSet<SignalId>> = processes.iter().map(get_process_writes).collect();

        for ((clock_signal, edge), seq_pids) in clock_groups {
            // Collect ALL signals written by sequential processes in this domain
            let mut domain_writes: HashSet<SignalId> = HashSet::new();
            for &pid in &seq_pids {
                if let Some(writes) = seq_writes.get(pid) {
                    domain_writes.extend(writes);
                }
            }

            // Find combinational processes that read any of these signals
            let follower_pids: Vec<usize> = processes
                .iter()
                .enumerate()
                .filter(|(pid, p)| {
                    !seq_pids.contains(pid)
                        && matches!(
                            p,
                            Process::Combinational { .. } | Process::CombReactive { .. }
                        )
                })
                .filter(|(_, p)| {
                    let reads = get_process_reads(p);
                    reads.iter().any(|sig| domain_writes.contains(sig))
                })
                .map(|(pid, _)| pid)
                .collect();

            // Mark all processes in this domain as fused
            for &pid in &seq_pids {
                fused_processes.insert(pid);
            }
            for &pid in &follower_pids {
                fused_processes.insert(pid);
            }

            let total_processes = seq_pids.len() + follower_pids.len();

            domains.push(ClockDomain {
                clock_signal,
                edge,
                sequential_processes: seq_pids,
                follower_processes: follower_pids,
                total_processes,
            });
        }

        ClockDomainAnalysis {
            domains,
            fused_processes,
        }
    }

    /// Number of clock domains found.
    pub fn num_domains(&self) -> usize {
        self.domains.len()
    }

    /// Number of processes fused into clock domains.
    pub fn num_fused_processes(&self) -> usize {
        self.fused_processes.len()
    }

    /// Total processes across all domains.
    pub fn total_fused_processes(&self) -> usize {
        self.domains.iter().map(|d| d.total_processes).sum()
    }
}

// ─── Signal Access Helpers (reuse from sim_dag.rs) ───

fn get_process_writes(process: &Process) -> HashSet<SignalId> {
    let mut writes = HashSet::new();
    match process {
        Process::Combinational { body, .. }
        | Process::CombReactive { body, .. }
        | Process::Initial { body, .. }
        | Process::Final { body, .. }
        | Process::AlwaysWithDelay { body, .. }
        | Process::Sequential { body, .. } => {
            collect_stmt_writes(body, &mut writes);
        }
    }
    writes
}

fn get_process_reads(process: &Process) -> HashSet<SignalId> {
    let mut reads = HashSet::new();
    match process {
        Process::Combinational {
            sensitivity, body, ..
        }
        | Process::CombReactive {
            sensitivity, body, ..
        } => {
            for s in sensitivity {
                reads.insert(s.sig_id);
            }
            collect_stmt_reads(body, &mut reads);
        }
        Process::Sequential { clock, body, .. } => {
            match clock {
                ClockEdge::PosEdge(sid) | ClockEdge::NegEdge(sid) => {
                    reads.insert(*sid);
                }
                // F27: clock hierarkis — belum dikenal saat analisis domain.
                ClockEdge::PosEdgeHier(_) | ClockEdge::NegEdgeHier(_) => {}
            }
            collect_stmt_reads(body, &mut reads);
        }
        Process::Initial { body, .. }
        | Process::Final { body, .. }
        | Process::AlwaysWithDelay { body, .. } => {
            collect_stmt_reads(body, &mut reads);
        }
    }
    reads
}

fn collect_stmt_writes(stmts: &[IrStmt], writes: &mut HashSet<SignalId>) {
    for stmt in stmts {
        match stmt {
            IrStmt::Block { stmts: inner } | IrStmt::NamedBlock { stmts: inner, .. } => {
                collect_stmt_writes(inner, writes);
            }
            IrStmt::BlockingAssign { lhs, .. } | IrStmt::NonBlockingAssign { lhs, .. } => {
                lvalue_collect_writes(lhs, writes);
            }
            IrStmt::Force { lvalue, .. }
            | IrStmt::Release { lvalue, .. }
            | IrStmt::Deassign { lvalue, .. } => {
                lvalue_collect_writes(lvalue, writes);
            }
            IrStmt::If {
                true_branch,
                false_branch,
                ..
            } => {
                collect_stmt_writes(true_branch, writes);
                collect_stmt_writes(false_branch, writes);
            }
            IrStmt::Case { items, default, .. } => {
                for item in items {
                    collect_stmt_writes(&item.body, writes);
                }
                collect_stmt_writes(default, writes);
            }
            IrStmt::LoopFor {
                init, step, body, ..
            } => {
                if let Some(s) = init {
                    collect_stmt_writes(&[s.as_ref().clone()], writes);
                }
                if let Some(s) = step {
                    collect_stmt_writes(&[s.as_ref().clone()], writes);
                }
                collect_stmt_writes(body, writes);
            }
            IrStmt::LoopWhile { body, .. }
            | IrStmt::LoopDoWhile { body, .. }
            | IrStmt::Repeat { body, .. }
            | IrStmt::Foreach { body, .. }
            | IrStmt::Delay { body, .. }
            | IrStmt::Wait { body, .. } => {
                collect_stmt_writes(body, writes);
            }
            IrStmt::EventTrigger { sig_id, .. } => {
                writes.insert(*sig_id);
            }
            IrStmt::Fork { processes, .. } => {
                for p in processes {
                    collect_stmt_writes(p, writes);
                }
            }
            IrStmt::Assert {
                pass_stmt,
                fail_stmt,
                ..
            }
            | IrStmt::Assume {
                pass_stmt,
                fail_stmt,
                ..
            }
            | IrStmt::Expect {
                pass_stmt,
                fail_stmt,
                ..
            } => {
                collect_stmt_writes(pass_stmt, writes);
                collect_stmt_writes(fail_stmt, writes);
            }
            IrStmt::Cover { pass_stmt, .. } => {
                collect_stmt_writes(pass_stmt, writes);
            }
            _ => {}
        }
    }
}

fn collect_stmt_reads(stmts: &[IrStmt], reads: &mut HashSet<SignalId>) {
    for stmt in stmts {
        match stmt {
            IrStmt::Block { stmts: inner } | IrStmt::NamedBlock { stmts: inner, .. } => {
                collect_stmt_reads(inner, reads);
            }
            IrStmt::BlockingAssign { rhs, .. } | IrStmt::NonBlockingAssign { rhs, .. } => {
                collect_expr_reads(rhs, reads);
            }
            IrStmt::If {
                cond,
                true_branch,
                false_branch,
                ..
            } => {
                collect_expr_reads(cond, reads);
                collect_stmt_reads(true_branch, reads);
                collect_stmt_reads(false_branch, reads);
            }
            IrStmt::Case {
                expr: case_expr,
                items,
                default,
                ..
            } => {
                collect_expr_reads(case_expr, reads);
                for item in items {
                    for pat in &item.labels {
                        collect_expr_reads(pat, reads);
                    }
                    collect_stmt_reads(&item.body, reads);
                }
                collect_stmt_reads(default, reads);
            }
            IrStmt::LoopFor { cond, body, .. } | IrStmt::LoopWhile { cond, body, .. } => {
                collect_expr_reads(cond, reads);
                collect_stmt_reads(body, reads);
            }
            IrStmt::LoopDoWhile { cond, body, .. } => {
                collect_stmt_reads(body, reads);
                collect_expr_reads(cond, reads);
            }
            IrStmt::Repeat { count, body, .. } => {
                collect_expr_reads(count, reads);
                collect_stmt_reads(body, reads);
            }
            IrStmt::Foreach {
                array_var, body, ..
            } => {
                collect_expr_reads(array_var, reads);
                collect_stmt_reads(body, reads);
            }
            IrStmt::Delay { body, .. } | IrStmt::Wait { cond: _, body, .. } => {
                collect_stmt_reads(body, reads);
            }
            IrStmt::EventControl { sigs, body, .. } => {
                for (sid, _) in sigs {
                    reads.insert(*sid);
                }
                collect_stmt_reads(body, reads);
            }
            IrStmt::Fork { processes, .. } => {
                for p in processes {
                    collect_stmt_reads(p, reads);
                }
            }
            IrStmt::Assert {
                cond,
                pass_stmt,
                fail_stmt,
                disable_iff,
                ..
            }
            | IrStmt::Assume {
                cond,
                pass_stmt,
                fail_stmt,
                disable_iff,
                ..
            } => {
                collect_expr_reads(cond, reads);
                if let Some(di) = disable_iff {
                    collect_expr_reads(di, reads);
                }
                collect_stmt_reads(pass_stmt, reads);
                collect_stmt_reads(fail_stmt, reads);
            }
            IrStmt::Expect {
                cond,
                pass_stmt,
                fail_stmt,
                ..
            } => {
                collect_expr_reads(cond, reads);
                collect_stmt_reads(pass_stmt, reads);
                collect_stmt_reads(fail_stmt, reads);
            }
            IrStmt::Cover {
                cond,
                pass_stmt,
                disable_iff,
                ..
            } => {
                collect_expr_reads(cond, reads);
                if let Some(di) = disable_iff {
                    collect_expr_reads(di, reads);
                }
                collect_stmt_reads(pass_stmt, reads);
            }
            IrStmt::SysCall { args, .. } => {
                for arg in args {
                    collect_expr_reads(arg, reads);
                }
            }
            _ => {}
        }
    }
}

fn collect_expr_reads(expr: &IrExpr, reads: &mut HashSet<SignalId>) {
    match expr {
        IrExpr::Signal(sig_id, _)
        | IrExpr::RangeSelect(sig_id, _, _)
        | IrExpr::BitSelect(sig_id, _) => {
            reads.insert(*sig_id);
        }
        IrExpr::ExprRangeSelect(inner, _, _) | IrExpr::ExprBitSelect(inner, _) => {
            collect_expr_reads(inner, reads);
        }
        IrExpr::ExprPartSelect(inner, base, width) => {
            collect_expr_reads(inner, reads);
            collect_expr_reads(base, reads);
            collect_expr_reads(width, reads);
        }
        IrExpr::ArrayIndex { sig_id, index, .. } => {
            reads.insert(*sig_id);
            collect_expr_reads(index, reads);
        }
        IrExpr::Concat(exprs) | IrExpr::StreamingConcat { slices: exprs, .. } => {
            for e in exprs {
                collect_expr_reads(e, reads);
            }
        }
        IrExpr::Replicate(_, inner) | IrExpr::UnaryOp(_, inner) | IrExpr::Signed(inner) => {
            collect_expr_reads(inner, reads);
        }
        IrExpr::BinaryOp(_, lhs, rhs) | IrExpr::Cond(_, lhs, rhs) => {
            collect_expr_reads(lhs, reads);
            collect_expr_reads(rhs, reads);
        }
        IrExpr::Cast { expr: inner, .. } => {
            collect_expr_reads(inner, reads);
        }
        IrExpr::Inside { expr: inner, list } => {
            collect_expr_reads(inner, reads);
            for item in list {
                collect_expr_reads(item, reads);
            }
        }
        IrExpr::DpiCall { args, .. }
        | IrExpr::SysFunc { args, .. }
        | IrExpr::NewCall { args, .. }
        | IrExpr::FuncCall { args, .. } => {
            for arg in args {
                collect_expr_reads(arg, reads);
            }
        }
        IrExpr::MethodCall {
            obj,
            args,
            with_clause,
            ..
        } => {
            collect_expr_reads(obj, reads);
            for arg in args {
                collect_expr_reads(arg, reads);
            }
            if let Some(wc) = with_clause {
                collect_expr_reads(wc, reads);
            }
        }
        IrExpr::MemberAccess { obj, .. } => {
            collect_expr_reads(obj, reads);
        }
        _ => {}
    }
}

fn lvalue_collect_writes(lvalue: &IrLValue, writes: &mut HashSet<SignalId>) {
    match lvalue {
        IrLValue::Signal(sig_id, _)
        | IrLValue::RangeSelect(sig_id, _, _)
        | IrLValue::BitSelect(sig_id, _)
        | IrLValue::ArrayIndex { sig_id, .. }
        | IrLValue::ArrayRangeSelect { sig_id, .. }
        | IrLValue::ArrayBitSelect { sig_id, .. }
        | IrLValue::ExprPartSelect { sig_id, .. }
        | IrLValue::ObjectField { sig_id, .. } => {
            writes.insert(*sig_id);
        }
        IrLValue::VifField { .. } => {}
        IrLValue::Concat(items) => {
            for item in items {
                lvalue_collect_writes(item, writes);
            }
        }
        // Lvalue hierarkis belum ter-resolve di fase ini.
        IrLValue::HierRef(_) | IrLValue::HierRefIndex { .. } => {}
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use mivon_core::intern::Symbol;

    #[test]
    fn test_empty_design() {
        let design = IrDesign {
            top: IrModule {
                name: Symbol::intern("top"),
                signals: vec![],
                inputs: vec![],
                outputs: vec![],
                inouts: vec![],
                processes: vec![],
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
        let analysis = ClockDomainAnalysis::analyze(&design);
        assert_eq!(analysis.num_domains(), 0);
        assert_eq!(analysis.num_fused_processes(), 0);
    }

    #[test]
    fn test_single_clock_domain() {
        // One Sequential process with combinational follower
        let clk_sig: SignalId = 0;
        let seq_body = vec![IrStmt::BlockingAssign {
            lhs: IrLValue::Signal(1, 8),
            rhs: IrExpr::BinaryOp(
                BinaryIrOp::Add,
                Box::new(IrExpr::Signal(1, 8)),
                Box::new(IrExpr::Const(LogicVec::from_u64(1, 8))),
            ),
            delay: None,
        }];
        let comb_body = vec![IrStmt::BlockingAssign {
            lhs: IrLValue::Signal(2, 8),
            rhs: IrExpr::Signal(1, 8),
            delay: None,
        }];

        let design = IrDesign {
            top: IrModule {
                name: Symbol::intern("top"),
                signals: vec![
                    SignalInfo {
                        name: Symbol::intern("clk"),
                        width: 1,
                        ..default_si()
                    },
                    SignalInfo {
                        name: Symbol::intern("counter"),
                        width: 8,
                        ..default_si()
                    },
                    SignalInfo {
                        name: Symbol::intern("out"),
                        width: 8,
                        ..default_si()
                    },
                ],
                inputs: vec![],
                outputs: vec![],
                inouts: vec![],
                processes: vec![
                    Process::Sequential {
                        name: Symbol::intern("seq"),
                        clock: ClockEdge::PosEdge(clk_sig),
                        reset: None,
                        body: seq_body,
                        iff: None,
                    },
                    Process::Combinational {
                        name: Symbol::intern("comb"),
                        sensitivity: vec![SignalSensitivity::whole(1)],
                        body: comb_body,
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

        let analysis = ClockDomainAnalysis::analyze(&design);
        assert_eq!(analysis.num_domains(), 1, "should find 1 clock domain");
        assert_eq!(analysis.num_fused_processes(), 2, "should fuse 2 processes");

        let domain = &analysis.domains[0];
        assert_eq!(domain.clock_signal, clk_sig);
        assert_eq!(domain.edge, ClockEdgeType::PosEdge);
        assert_eq!(domain.sequential_processes.len(), 1);
        assert_eq!(domain.follower_processes.len(), 1);
        assert_eq!(domain.total_processes, 2);
    }

    #[test]
    fn test_no_combinational_follower() {
        // Sequential process with no combinational follower
        let clk_sig: SignalId = 0;
        let seq_body = vec![IrStmt::BlockingAssign {
            lhs: IrLValue::Signal(1, 8),
            rhs: IrExpr::Const(LogicVec::from_u64(42, 8)),
            delay: None,
        }];

        let design = IrDesign {
            top: IrModule {
                name: Symbol::intern("top"),
                signals: vec![
                    SignalInfo {
                        name: Symbol::intern("clk"),
                        width: 1,
                        ..default_si()
                    },
                    SignalInfo {
                        name: Symbol::intern("q"),
                        width: 8,
                        ..default_si()
                    },
                ],
                inputs: vec![],
                outputs: vec![],
                inouts: vec![],
                processes: vec![Process::Sequential {
                    name: Symbol::intern("seq"),
                    clock: ClockEdge::PosEdge(clk_sig),
                    reset: None,
                    body: seq_body,
                    iff: None,
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

        let analysis = ClockDomainAnalysis::analyze(&design);
        assert_eq!(analysis.num_domains(), 1);
        assert_eq!(analysis.domains[0].follower_processes.len(), 0);
        assert_eq!(analysis.domains[0].total_processes, 1);
    }

    fn default_si() -> SignalInfo {
        SignalInfo {
            name: Symbol::intern(""),
            width: 1,
            kind: SignalKind::Reg,
            net_type: NetType::Wire,
            multi_driver: false,
            init_val: LogicVec::new(1),
            array_depth: 0,
            elem_width: 1,
            array_dims: vec![],
            class_name: None,
            is_string: false,
            is_real: false,
            is_mailbox: false,
            is_semaphore: false,
            is_2state: false,
            is_dynamic: false,
            is_queue: false,
            is_associative: false,
            is_signed: false,
            is_const: false,
            msb: 0,
            lsb: 0,
            struct_fields: vec![],
            packed_dims: vec![],
            delay_rise: None,
            delay_fall: None,
            iface_type: None,
            iface_modport: None,
        }
    }
}
