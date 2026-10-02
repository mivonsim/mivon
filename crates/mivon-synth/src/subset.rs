//! Analisis sintesizability (SYNTHESIS.md §5.2) — SYN-1..9.
//!
//! Menelusuri `IrDesign` (proses + statement + signal) dan melaporkan konstruk
//! yang tidak bisa disintesis, dengan kode diagnostic `SYN-n`. Hasilnya berupa
//! daftar issue + skor sintesizability per modul (estimasi persentase).

use mivon_core::intern::Symbol;
use mivon_ir::{IrDesign, IrExpr, IrLValue, IrModule, IrStmt, Process, SignalInfo};

/// Severity issue SYN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SynSeverity {
    Error,
    Warning,
}

impl SynSeverity {
    pub fn name(&self) -> &'static str {
        match self {
            SynSeverity::Error => "error",
            SynSeverity::Warning => "warning",
        }
    }
}

/// Satu issue sintesizability.
#[derive(Debug, Clone, PartialEq)]
pub struct SynIssue {
    /// Kode `SYN-1`..`SYN-9`.
    pub code: &'static str,
    pub severity: SynSeverity,
    pub module: Symbol,
    pub message: String,
}

/// Hasil pengecekan per modul + agregat.
#[derive(Debug, Clone, PartialEq)]
pub struct SynCheck {
    pub issues: Vec<SynIssue>,
    /// (nama modul, error, warning, skor 0..100).
    pub per_module: Vec<(Symbol, usize, usize, f64)>,
}

impl SynCheck {
    pub fn error_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity == SynSeverity::Error)
            .count()
    }

    pub fn warning_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity == SynSeverity::Warning)
            .count()
    }

    /// Agregat skor seluruh design (rata-rata modul).
    pub fn overall_score(&self) -> f64 {
        if self.per_module.is_empty() {
            return 0.0;
        }
        let sum: f64 = self.per_module.iter().map(|(_, _, _, s)| s).sum();
        sum / self.per_module.len() as f64
    }
}

/// Jalankan analisis sintesizability pada seluruh design.
pub fn check(ir: &IrDesign) -> SynCheck {
    let mut ctx = Ctx {
        issues: Vec::new(),
        per_module: Vec::new(),
    };
    let mut seen = std::collections::HashSet::new();
    ctx.check_module(&ir.top, &mut seen);
    let mut names: Vec<Symbol> = ir.modules.keys().copied().collect();
    names.sort_by_key(|s| s.as_str().to_string());
    for name in names {
        if let Some(m) = ir.modules.get(&name) {
            ctx.check_module(m, &mut seen);
        }
    }
    SynCheck {
        issues: ctx.issues,
        per_module: ctx.per_module,
    }
}

struct Ctx {
    issues: Vec<SynIssue>,
    per_module: Vec<(Symbol, usize, usize, f64)>,
}

impl Ctx {
    fn push(&mut self, module: Symbol, code: &'static str, severity: SynSeverity, message: String) {
        self.issues.push(SynIssue {
            code,
            severity,
            module,
            message,
        });
    }

    /// Periksa satu modul: signal-level + proses-level, lalu hitung skor.
    fn check_module(&mut self, m: &IrModule, seen: &mut std::collections::HashSet<Symbol>) {
        if !seen.insert(m.name) {
            return;
        }
        let mut err = 0usize;
        let mut warn = 0usize;

        for s in &m.signals {
            self.check_signal(s, m.name, &mut err, &mut warn);
        }
        for p in &m.processes {
            self.check_process(p, m.name, &mut err, &mut warn);
        }

        let score = (100.0 - err as f64 * 10.0 - warn as f64 * 2.0).clamp(0.0, 100.0);
        self.per_module.push((m.name, err, warn, score));
    }

    fn check_signal(&mut self, s: &SignalInfo, module: Symbol, err: &mut usize, warn: &mut usize) {
        if s.is_real {
            self.push(
                module,
                "SYN-6",
                SynSeverity::Error,
                format!(
                    "signal '{}' bertipe `real` — tidak bisa disintesis",
                    s.name.as_str()
                ),
            );
            *err += 1;
        }
        if s.is_string || s.is_mailbox || s.is_semaphore {
            self.push(
                module,
                "SYN-6",
                SynSeverity::Error,
                format!(
                    "signal '{}' bertipe non-sintesis (string/mailbox/semaphore)",
                    s.name.as_str()
                ),
            );
            *err += 1;
        }
        if let Some(cls) = &s.class_name {
            self.push(
                module,
                "SYN-5",
                SynSeverity::Error,
                format!(
                    "signal '{}' adalah objek class '{}' — class tidak bisa disintesis",
                    s.name.as_str(),
                    cls.as_str()
                ),
            );
            *err += 1;
        }
        if s.multi_driver {
            self.push(
                module,
                "SYN-7",
                SynSeverity::Warning,
                format!(
                    "signal '{}' multi-driver ({:?}) — resolver di-infer, verifikasi konflik",
                    s.name.as_str(),
                    s.net_type
                ),
            );
            *warn += 1;
        }
    }

    fn check_process(&mut self, p: &Process, module: Symbol, err: &mut usize, warn: &mut usize) {
        match p {
            Process::Initial { body, .. } | Process::Final { body, .. } => {
                let has_state = body.iter().any(stmt_has_state);
                if has_state {
                    self.push(
                        module,
                        "SYN-1",
                        SynSeverity::Error,
                        format!(
                            "proses {:?} mengandung state/delay — hanya sah di testbench, tidak disintesis",
                            process_kind_name(p)
                        ),
                    );
                    *err += 1;
                } else {
                    self.push(
                        module,
                        "SYN-1",
                        SynSeverity::Warning,
                        format!(
                            "proses {:?} diabaikan synthesis (testbench)",
                            process_kind_name(p)
                        ),
                    );
                    *warn += 1;
                }
            }
            Process::Combinational { body, .. } | Process::CombReactive { body, .. } => {
                // SYN-9: latch inference — signal di-assign di if tanpa else.
                let mut assigned = std::collections::HashSet::new();
                let mut maybe_latch = Vec::new();
                collect_latch_candidates(body, &mut assigned, &mut maybe_latch);
                for sig in maybe_latch {
                    self.push(
                        module,
                        "SYN-9",
                        SynSeverity::Warning,
                        format!(
                            "potensi LATCH pada '{}' — if tanpa else di proses combinational",
                            sig
                        ),
                    );
                    *warn += 1;
                }
                walk_stmts(body, &mut |st| {
                    self.stmt_issue(st, module, err, warn);
                    false
                });
            }
            Process::Sequential { body, .. } | Process::AlwaysWithDelay { body, .. } => {
                walk_stmts(body, &mut |st| {
                    self.stmt_issue(st, module, err, warn);
                    false
                });
            }
        }
    }

    /// Periksa statement — dipanggil untuk setiap statement (via walker).
    fn stmt_issue(&mut self, st: &IrStmt, module: Symbol, err: &mut usize, warn: &mut usize) {
        match st {
            IrStmt::Delay { .. }
            | IrStmt::Wait { .. }
            | IrStmt::WaitFork
            | IrStmt::EventControl { .. } => {
                self.push(
                    module,
                    "SYN-2",
                    SynSeverity::Error,
                    format!(
                        "timing control ({:?}) tidak bisa disintesis",
                        stmt_kind_name(st)
                    ),
                );
                *err += 1;
            }
            IrStmt::Fork { .. }
            | IrStmt::Force { .. }
            | IrStmt::Release { .. }
            | IrStmt::Deassign { .. }
            | IrStmt::Disable { .. } => {
                self.push(
                    module,
                    "SYN-3",
                    SynSeverity::Error,
                    format!("konstruk non-sintesis: {:?}", stmt_kind_name(st)),
                );
                *err += 1;
            }
            IrStmt::SysCall { name, .. } => {
                self.push(
                    module,
                    "SYN-4",
                    SynSeverity::Warning,
                    format!(
                        "system task {} dibuang saat synthesis (testbench)",
                        name.as_str()
                    ),
                );
                *warn += 1;
            }
            IrStmt::SysFinish => {
                self.push(
                    module,
                    "SYN-4",
                    SynSeverity::Warning,
                    "$finish dibuang saat synthesis (testbench)".into(),
                );
                *warn += 1;
            }
            IrStmt::Assert { .. }
            | IrStmt::Assume { .. }
            | IrStmt::Cover { .. }
            | IrStmt::Expect { .. } => {
                self.push(
                    module,
                    "SYN-4",
                    SynSeverity::Warning,
                    "assertion/cover/expect dibuang saat synthesis".into(),
                );
                *warn += 1;
            }
            IrStmt::LoopWhile { .. } | IrStmt::LoopDoWhile { .. } | IrStmt::Foreach { .. } => {
                self.push(
                    module,
                    "SYN-8",
                    SynSeverity::Error,
                    format!(
                        "loop {:?} non-unrollable — synthesis butuh loop compile-time",
                        stmt_kind_name(st)
                    ),
                );
                *err += 1;
            }
            IrStmt::Repeat { count, .. } => {
                if !expr_is_const(count) {
                    self.push(
                        module,
                        "SYN-8",
                        SynSeverity::Error,
                        "repeat dengan count non-konstan tidak bisa di-unroll".into(),
                    );
                    *err += 1;
                }
            }
            IrStmt::LoopFor { cond, .. } => {
                if !expr_is_const(cond) {
                    self.push(
                        module,
                        "SYN-8",
                        SynSeverity::Error,
                        "for dengan kondisi non-konstan tidak bisa di-unroll".into(),
                    );
                    *err += 1;
                }
            }
            IrStmt::MethodCallStmt { obj, .. } => {
                self.check_expr_for_class(obj, module, err, warn);
            }
            IrStmt::BlockingAssign { rhs, .. } | IrStmt::NonBlockingAssign { rhs, .. } => {
                self.check_expr_for_class(rhs, module, err, warn);
            }
            _ => {}
        }
    }

    /// Deteksi objek class / method call / new di dalam ekspresi (SYN-5).
    fn check_expr_for_class(
        &mut self,
        e: &IrExpr,
        module: Symbol,
        err: &mut usize,
        _warn: &mut usize,
    ) {
        walk_expr(e, &mut |x| match x {
            IrExpr::NewCall { .. } | IrExpr::This | IrExpr::MethodCall { .. } => {
                self.push(
                    module,
                    "SYN-5",
                    SynSeverity::Error,
                    format!(
                        "penggunaan class/method ({:?}) di datapath — tidak bisa disintesis",
                        expr_kind_name(x)
                    ),
                );
                *err += 1;
                true
            }
            _ => false,
        });
    }
}

fn process_kind_name(p: &Process) -> &'static str {
    match p {
        Process::Initial { .. } => "initial",
        Process::Final { .. } => "final",
        Process::Combinational { .. } => "always_comb",
        Process::CombReactive { .. } => "always_comb(reactive)",
        Process::Sequential { .. } => "always_ff",
        Process::AlwaysWithDelay { .. } => "always",
    }
}

fn stmt_kind_name(st: &IrStmt) -> &'static str {
    match st {
        IrStmt::Block { .. } => "block",
        IrStmt::NamedBlock { .. } => "named block",
        IrStmt::BlockingAssign { .. } => "blocking assign",
        IrStmt::NonBlockingAssign { .. } => "non-blocking assign",
        IrStmt::If { .. } => "if",
        IrStmt::Case { .. } => "case",
        IrStmt::LoopFor { .. } => "for",
        IrStmt::LoopWhile { .. } => "while",
        IrStmt::LoopDoWhile { .. } => "do-while",
        IrStmt::Repeat { .. } => "repeat",
        IrStmt::Foreach { .. } => "foreach",
        IrStmt::Delay { .. } => "delay",
        IrStmt::Force { .. } => "force",
        IrStmt::Wait { .. } => "wait",
        IrStmt::SysCall { .. } => "system task",
        IrStmt::SysFinish => "$finish",
        IrStmt::Null => "null",
        IrStmt::EventControl { .. } => "event control",
        IrStmt::EventTrigger { .. } => "event trigger",
        IrStmt::MethodCallStmt { .. } => "method call",
        IrStmt::Break => "break",
        IrStmt::Continue => "continue",
        IrStmt::Disable { .. } => "disable",
        IrStmt::Release { .. } => "release",
        IrStmt::Deassign { .. } => "deassign",
        IrStmt::Fork { .. } => "fork",
        IrStmt::Assert { .. } => "assert",
        IrStmt::Assume { .. } => "assume",
        IrStmt::Cover { .. } => "cover",
        IrStmt::Expect { .. } => "expect",
        IrStmt::WaitOrder { .. } => "wait_order",
        IrStmt::WaitFork => "wait_fork",
        IrStmt::RandCase { .. } => "randcase",
        IrStmt::RandSequence { .. } => "randsequence",
    }
}

fn expr_kind_name(e: &IrExpr) -> &'static str {
    match e {
        IrExpr::NewCall { .. } => "new",
        IrExpr::This => "this",
        IrExpr::MethodCall { .. } => "method call",
        _ => "expr",
    }
}

/// Apakah statement (secara transitif) mengandung state/delay (SYN-1).
fn stmt_has_state(st: &IrStmt) -> bool {
    let mut found = false;
    walk_stmts(std::slice::from_ref(st), &mut |s| match s {
        IrStmt::Delay { .. } | IrStmt::Wait { .. } | IrStmt::EventControl { .. } => {
            found = true;
            true
        }
        IrStmt::LoopWhile { .. }
        | IrStmt::LoopDoWhile { .. }
        | IrStmt::Foreach { .. }
        | IrStmt::Fork { .. } => {
            found = true;
            true
        }
        _ => false,
    });
    found
}

/// Walk semua statement (rekursif termasuk body If/Case/loop).
/// Callback mengembalikan true bila sudah menangani statement sendiri.
pub fn walk_stmts<F>(stmts: &[IrStmt], f: &mut F)
where
    F: FnMut(&IrStmt) -> bool,
{
    for st in stmts {
        if f(st) {
            continue;
        }
        match st {
            IrStmt::Block { stmts } | IrStmt::NamedBlock { stmts, .. } => walk_stmts(stmts, f),
            IrStmt::If {
                true_branch,
                false_branch,
                ..
            } => {
                walk_stmts(true_branch, f);
                walk_stmts(false_branch, f);
            }
            IrStmt::Case { items, default, .. } => {
                for it in items {
                    walk_stmts(&it.body, f);
                }
                walk_stmts(default, f);
            }
            IrStmt::LoopFor { init, body, .. } => {
                if let Some(i) = init {
                    walk_stmts(std::slice::from_ref(i), f);
                }
                walk_stmts(body, f);
            }
            IrStmt::LoopWhile { body, .. }
            | IrStmt::LoopDoWhile { body, .. }
            | IrStmt::Repeat { body, .. }
            | IrStmt::Foreach { body, .. }
            | IrStmt::Delay { body, .. }
            | IrStmt::Wait { body, .. }
            | IrStmt::EventControl { body, .. } => walk_stmts(body, f),
            IrStmt::WaitFork => {}
            IrStmt::Fork { processes, .. } => {
                for p in processes {
                    walk_stmts(p, f);
                }
            }
            IrStmt::WaitOrder { failure_stmts, .. } => walk_stmts(failure_stmts, f),
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
                walk_stmts(pass_stmt, f);
                walk_stmts(fail_stmt, f);
            }
            IrStmt::Cover { pass_stmt, .. } => {
                walk_stmts(pass_stmt, f);
            }
            IrStmt::RandCase { items } => {
                for (_, body) in items {
                    walk_stmts(body, f);
                }
            }
            IrStmt::RandSequence { productions } => {
                for (_, prods) in productions {
                    for (_, body) in prods {
                        walk_stmts(body, f);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Walk semua ekspresi (rekursif termasuk sub-ekspresi).
pub fn walk_expr<F>(e: &IrExpr, f: &mut F)
where
    F: FnMut(&IrExpr) -> bool,
{
    if f(e) {
        return;
    }
    match e {
        // RangeSelect/BitSelect membawa SignalId (usize), bukan ekspresi.
        IrExpr::Signed(inner)
        | IrExpr::ExprRangeSelect(inner, _, _)
        | IrExpr::ExprBitSelect(inner, _) => walk_expr(inner, f),
        IrExpr::RangeSelect(_, _, _) | IrExpr::BitSelect(_, _) => {}
        IrExpr::ExprPartSelect(base, idx, width) => {
            walk_expr(base, f);
            walk_expr(idx, f);
            walk_expr(width, f);
        }
        IrExpr::ArrayIndex { index, .. } => walk_expr(index, f),
        IrExpr::Concat(parts) => {
            for p in parts {
                walk_expr(p, f);
            }
        }
        IrExpr::Replicate(_, inner) => walk_expr(inner, f),
        IrExpr::UnaryOp(_, inner) => walk_expr(inner, f),
        IrExpr::BinaryOp(_, a, b) => {
            walk_expr(a, f);
            walk_expr(b, f);
        }
        IrExpr::Cond(c, a, b) => {
            walk_expr(c, f);
            walk_expr(a, f);
            walk_expr(b, f);
        }
        IrExpr::SysFunc { args, .. }
        | IrExpr::DpiCall { args, .. }
        | IrExpr::FuncCall { args, .. } => {
            for a in args {
                walk_expr(a, f);
            }
        }
        IrExpr::NewCall { args, .. } => {
            for a in args {
                walk_expr(a, f);
            }
        }
        IrExpr::MethodCall { obj, args, .. } => {
            walk_expr(obj, f);
            for a in args {
                walk_expr(a, f);
            }
        }
        IrExpr::MemberAccess { obj, .. } => {
            walk_expr(obj, f);
        }
        IrExpr::Inside { expr, list } => {
            walk_expr(expr, f);
            for l in list {
                walk_expr(l, f);
            }
        }
        IrExpr::InsideRange { expr, lo, hi } => {
            walk_expr(expr, f);
            walk_expr(lo, f);
            walk_expr(hi, f);
        }
        IrExpr::Cast { expr, .. } => walk_expr(expr, f),
        IrExpr::StreamingConcat { slices, .. } => {
            for s in slices {
                walk_expr(s, f);
            }
        }
        IrExpr::Dist { expr, items } => {
            walk_expr(expr, f);
            let _ = items;
        }
        // Side-effect RMW: kunjungi operand baca (untuk analisis dependency).
        IrExpr::IncDec { read, .. } => walk_expr(read, f),
        IrExpr::UdpLookup { args, .. } => {
            for a in args {
                walk_expr(a, f);
            }
        }
        IrExpr::Const(_)
        | IrExpr::FillLit(_)
        | IrExpr::Signal(_, _)
        | IrExpr::String(_)
        | IrExpr::This
        | IrExpr::HierRef(_)
        | IrExpr::VifBinding { .. }
        | IrExpr::VirtualIfaceAccess { .. } => {}
    }
}

/// Apakah ekspresi bernilai konstanta (bisa di-unroll).
pub fn expr_is_const(e: &IrExpr) -> bool {
    match e {
        IrExpr::Const(_) | IrExpr::FillLit(_) => true,
        IrExpr::UnaryOp(_, inner) => expr_is_const(inner),
        IrExpr::BinaryOp(_, a, b) => expr_is_const(a) && expr_is_const(b),
        IrExpr::Cond(c, a, b) => expr_is_const(c) && expr_is_const(a) && expr_is_const(b),
        IrExpr::Concat(parts) => parts.iter().all(expr_is_const),
        IrExpr::Replicate(_, inner) => expr_is_const(inner),
        _ => false,
    }
}

/// Kumpulkan nama signal yang berpotensi latch: di-assign dalam if tanpa else
/// (dan tidak di-assign di semua jalur). Heuristik S1 — deteksi penuh S2+.
fn collect_latch_candidates(
    stmts: &[IrStmt],
    assigned: &mut std::collections::HashSet<String>,
    out: &mut Vec<String>,
) {
    for st in stmts {
        match st {
            IrStmt::BlockingAssign { lhs, .. } | IrStmt::NonBlockingAssign { lhs, .. } => {
                if let Some(name) = lvalue_signal_name(lhs) {
                    assigned.insert(name);
                }
            }
            IrStmt::If {
                true_branch,
                false_branch,
                ..
            } => {
                let mut true_only = std::collections::HashSet::new();
                collect_assigned_names(true_branch, &mut true_only);
                let mut false_only = std::collections::HashSet::new();
                collect_assigned_names(false_branch, &mut false_only);
                for name in true_only {
                    if !false_only.contains(&name)
                        && !assigned.contains(&name)
                        && !out.contains(&name)
                    {
                        out.push(name);
                    }
                }
                collect_latch_candidates(true_branch, &mut t_fresh(), out);
                collect_latch_candidates(false_branch, &mut t_fresh(), out);
            }
            _ => {}
        }
    }
}

fn t_fresh() -> std::collections::HashSet<String> {
    std::collections::HashSet::new()
}

fn collect_assigned_names(stmts: &[IrStmt], out: &mut std::collections::HashSet<String>) {
    for st in stmts {
        match st {
            IrStmt::BlockingAssign { lhs, .. } | IrStmt::NonBlockingAssign { lhs, .. } => {
                if let Some(name) = lvalue_signal_name(lhs) {
                    out.insert(name);
                }
            }
            IrStmt::Block { stmts } | IrStmt::NamedBlock { stmts, .. } => {
                collect_assigned_names(stmts, out)
            }
            IrStmt::If {
                true_branch,
                false_branch,
                ..
            } => {
                collect_assigned_names(true_branch, out);
                collect_assigned_names(false_branch, out);
            }
            IrStmt::Case { items, default, .. } => {
                for it in items {
                    collect_assigned_names(&it.body, out);
                }
                collect_assigned_names(default, out);
            }
            _ => {}
        }
    }
}

fn lvalue_signal_name(lhs: &IrLValue) -> Option<String> {
    match lhs {
        IrLValue::Signal(id, _) => Some(format!("sig#{}", id)),
        IrLValue::BitSelect(id, _) | IrLValue::RangeSelect(id, _, _) => Some(format!("sig#{}", id)),
        IrLValue::ArrayIndex { sig_id, .. } | IrLValue::ArrayBitSelect { sig_id, .. } => {
            Some(format!("sig#{}", sig_id))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_design() -> IrDesign {
        IrDesign {
            top: mivon_ir::IrModule {
                name: Symbol::intern("top"),
                ..Default::default()
            },
            modules: Default::default(),
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
        }
    }

    #[test]
    fn clean_design_has_no_issues() {
        let ir = empty_design();
        let r = check(&ir);
        assert_eq!(r.error_count(), 0);
        assert_eq!(r.warning_count(), 0);
        assert!(r.overall_score() >= 100.0);
    }

    #[test]
    fn initial_block_is_reported() {
        let mut ir = empty_design();
        ir.top.processes.push(Process::Initial {
            name: Symbol::intern("init"),
            body: vec![IrStmt::Null],
        });
        let r = check(&ir);
        assert_eq!(r.error_count(), 0);
        assert!(r.warning_count() >= 1); // SYN-1 warning (testbench)
        assert!(r.issues.iter().any(|i| i.code == "SYN-1"));
    }

    #[test]
    fn delay_in_sequential_is_error() {
        let mut ir = empty_design();
        ir.top.processes.push(Process::Sequential {
            name: Symbol::intern("seq"),
            clock: mivon_ir::ClockEdge::PosEdge(0),
            reset: None,
            body: vec![IrStmt::Delay {
                delay: 5,
                body: vec![IrStmt::Null],
            }],
            iff: None,
        });
        let r = check(&ir);
        assert!(r
            .issues
            .iter()
            .any(|i| i.code == "SYN-2" && i.severity == SynSeverity::Error));
    }

    #[test]
    fn fork_is_error() {
        let mut ir = empty_design();
        ir.top.processes.push(Process::Combinational {
            name: Symbol::intern("comb"),
            sensitivity: vec![],
            body: vec![IrStmt::Fork {
                processes: vec![vec![IrStmt::Null]],
                join_type: mivon_ir::IrJoinType::Join,
            }],
        });
        let r = check(&ir);
        assert!(r.issues.iter().any(|i| i.code == "SYN-3"));
    }
}
