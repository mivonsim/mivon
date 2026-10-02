//! ──────────────────────────────────────────────────────────────────────────────
//! CATATAN: File ini adalah bagian dari pemisahan util.rs (SRP Refactoring).
//! Tanggung jawab: Loop unrolling & variable substitution (genvar, loop vars).
//!
//! Fungsi:
//!   - try_unroll_for_loop()          — unroll for loop dengan konstanta
//!   - substitute_loop_var_in_stmts() — substitusi loop var di statements
//!   - substitute_loop_var_in_stmt()  — substitusi loop var di satu statement
//!   - substitute_loop_var_in_expr()  — substitusi loop var di expression
//!   - substitute_sensitivity_event() — substitusi loop var di sensitivity event
//!
//! ──────────────────────────────────────────────────────────────────────────────

use std::collections::HashMap;

use mivon_ast::types::const_eval_with_params;
use mivon_ast::*;
use mivon_core::intern::Symbol;
use mivon_ir::*;

/// Coba unroll for loop dengan nilai parameter yang diketahui.
/// Mengembalikan Some(vec) jika berhasil di-unroll, None jika tidak bisa.
pub fn try_unroll_for_loop<'a, F>(
    init: Option<&'a Stmt>,
    cond: Option<&'a Expr>,
    step: Option<&'a Stmt>,
    stmts: &[Stmt],
    elaborate_body: &F,
    params: &HashMap<Symbol, i64>,
) -> Result<Option<Vec<IrStmt>>, String>
where
    F: Fn(&[Stmt], &str, i64) -> Result<Vec<IrStmt>, String>,
{
    // ── Enum-iterasi fallback (prim_sparse_fsm_flop::is_undefined_state) ──
    // `for (int i = 0, StateEnumT t = t.first(); i < t.num(); i += 1, t = t.next())`
    // — enum METHODS `.first()/.num()/.next()` pada loop var bertipe
    // `parameter type` (per-instance enum tidak di-resolve mivon). Loop ini
    // hanya menandai signal assertion/FIP (`unused_err_o`), BUKAN datapath.
    // Deteksi struktural (tanpa mengevaluasi t):
    let is_enum_iter = cond.as_ref().is_some_and(|c| expr_uses_enum_method(c))
        || step.as_ref().is_some_and(|s| stmt_uses_enum_method(s))
        || init.as_ref().is_some_and(|s| stmt_uses_enum_method(s));
    if is_enum_iter {
        // Elide loop — body dihilangkan; loop var enum tidak dapat di-resolve
        // (E2001 't' not found). `is_defined` tetap 0 → `~is_defined`=1 →
        // `unused_err_o`=1 (spurious tapi NON-FATAL utk sim RTL; datapath
        // tidak terpengaruh). Caller mencatat warning degradasi.
        return Ok(Some(Vec::new()));
    }

    let (var_name, init_val) = match init {
        Some(Stmt::BlockingAssign {
            lhs: Expr::Ident { name, .. },
            rhs,
            ..
        }) => (*name, const_eval_with_params(rhs, params)?),
        _ => return Ok(None),
    };

    // Step: `i = i + inc` (Add) atau `i = i - inc` (Sub). Pola OpenTitan juga
    // memakai `i--` (loop menurun) — sebelumnya hanya Add yang didukung,
    // sehingga `for (int i = 14; i >= 0; i--)` jatuh ke fallback runtime dan
    // loop var `i` tidak ada di signal_map → error E2001 'i' not found.
    let step_fn: Box<dyn Fn(i64) -> Result<i64, String>> = match step {
        Some(Stmt::BlockingAssign {
            lhs: Expr::Ident { name: n, .. },
            rhs,
            ..
        }) if *n == var_name => match rhs {
            Expr::BinaryOp {
                op: BinaryOp::Add,
                lhs,
                rhs,
            } => {
                if let Expr::Ident { name: n2, .. } = lhs.as_ref() {
                    if n2 == &var_name {
                        let inc = const_eval_with_params(rhs, params)?;
                        Box::new(move |v| Ok(v + inc))
                    } else {
                        return Ok(None);
                    }
                } else if let Expr::Ident { name: n2, .. } = rhs.as_ref() {
                    if n2 == &var_name {
                        let inc = const_eval_with_params(lhs, params)?;
                        Box::new(move |v| Ok(v + inc))
                    } else {
                        return Ok(None);
                    }
                } else {
                    return Ok(None);
                }
            }
            Expr::BinaryOp {
                op: BinaryOp::Sub,
                lhs,
                rhs,
            } => {
                if let Expr::Ident { name: n2, .. } = lhs.as_ref() {
                    if n2 == &var_name {
                        let inc = const_eval_with_params(rhs, params)?;
                        Box::new(move |v| Ok(v - inc))
                    } else {
                        return Ok(None);
                    }
                } else {
                    return Ok(None);
                }
            }
            _ => return Ok(None),
        },
        _ => return Ok(None),
    };

    // Condition: `i < limit`, `i <= limit` (loop naik) atau
    // `i > limit`, `i >= limit` (loop turun).
    let (cmp_op, limit) = match cond {
        Some(Expr::BinaryOp { op, lhs, rhs }) => match lhs.as_ref() {
            Expr::Ident { name: n, .. } if *n == var_name => {
                (op.clone(), const_eval_with_params(rhs, params)?)
            }
            _ => return Ok(None),
        },
        _ => return Ok(None),
    };

    // Break/Continue KEHILANGAN semantiknya bila body di-unroll menjadi
    // kode lurus: `continue` di iterasi ke-k menjadi "hentikan SEMUA
    // iterasi berikutnya" karena FlowControl menyebar ke seluruh blok
    // hasil-unroll (fuzz loop_fuzz varian C/D: rc salah 4→2, 2→0).
    // Loop dengan kontrol ini tidak di-unroll — fallback runtime
    // LoopFor menangani keduanya dengan benar.
    if stmts_contain_break_continue(stmts) {
        return Ok(None);
    }

    let mut all_stmts = Vec::new();
    let mut ivar = init_val;
    // Jaring pengaman: maks 100k iterasi unroll — loop yang lebih besar
    // tidak di-unroll (fallback runtime loop, di mana MAX_LOOP_ITER engine
    // menghentikan loop tak berujung). Unroll 1M+ iterasi membuang waktu
    // elaborasi untuk loop besar yang biasanya bukan generate.
    let mut guard = 0usize;
    loop {
        guard += 1;
        // Batas unroll TURUN (4096 → 256): loop lebih besar dijalankan
        // RUNTIME di engine (LoopFor) — unroll 2000+ stmt memperbesar
        // IR (OpenTitan 3GB) & memperlambat elab/sim (bench: elab demo
        // 3 modul 789ms utk ITER=2000; runtime loop jauh lebih murah).
        // 256 iterasi sudah menutupi pola real (generate/unroll kecil).
        if guard > 256 {
            return Ok(None);
        }
        let keep = match cmp_op {
            BinaryOp::Lt => ivar < limit,
            BinaryOp::Le => ivar <= limit,
            BinaryOp::Gt => ivar > limit,
            BinaryOp::Ge => ivar >= limit,
            _ => return Ok(None),
        };
        if !keep {
            break;
        }
        let body = elaborate_body(stmts, var_name.as_str(), ivar)?;
        all_stmts.extend(body);
        ivar = step_fn(ivar)?;
    }

    Ok(Some(all_stmts))
}

/// Deteksi `break`/`continue` di dalam body loop (rekursif ke statement
/// bersarang). TIDAK menurun ke loop bersarang — `break`/`continue` di
/// sana terikat pada loop dalam, bukan loop yang akan di-unroll.
pub fn stmts_contain_break_continue(stmts: &[Stmt]) -> bool {
    stmts.iter().any(stmt_contains_break_continue)
}

/// Apakah ekspresi memuat enum METHOD `.first()/.num()/.next()` —
/// penanda loop iterasi enum (`t.num()`, `t.next()`, `t.first()`).
fn expr_uses_enum_method(e: &Expr) -> bool {
    match e {
        Expr::MethodCall { method, args, .. } => {
            let m = method.as_str();
            (m == "first" || m == "num" || m == "next") || args.iter().any(expr_uses_enum_method)
        }
        Expr::BinaryOp { lhs, rhs, .. } => expr_uses_enum_method(lhs) || expr_uses_enum_method(rhs),
        Expr::UnaryOp { expr, .. } => expr_uses_enum_method(expr),
        Expr::Cast { expr, .. } => expr_uses_enum_method(expr),
        Expr::RangeSelect { expr, msb, lsb } => {
            expr_uses_enum_method(expr) || expr_uses_enum_method(msb) || expr_uses_enum_method(lsb)
        }
        Expr::TernaryOp {
            cond,
            true_expr,
            false_expr,
        } => {
            expr_uses_enum_method(cond)
                || expr_uses_enum_method(true_expr)
                || expr_uses_enum_method(false_expr)
        }
        Expr::FuncCall { args, .. } => args.iter().any(expr_uses_enum_method),
        _ => false,
    }
}

/// Apakah statement memuat enum method (untuk init/step `t=t.first()`,
/// `t=t.next()`).
fn stmt_uses_enum_method(s: &Stmt) -> bool {
    match s {
        Stmt::BlockingAssign { lhs, rhs, .. } | Stmt::NonBlockingAssign { lhs, rhs, .. } => {
            expr_uses_enum_method(lhs) || expr_uses_enum_method(rhs)
        }
        Stmt::IfElse {
            cond,
            true_branch,
            false_branch,
            ..
        } => {
            expr_uses_enum_method(cond)
                || stmt_uses_enum_method(true_branch)
                || false_branch
                    .as_ref()
                    .is_some_and(|b| stmt_uses_enum_method(b))
        }
        Stmt::Block { stmts, .. } | Stmt::NamedBlock { stmts, .. } => {
            stmts.iter().any(stmt_uses_enum_method)
        }
        _ => false,
    }
}

/// Deteksi pola loop iterasi enum: `for (... t = t.first(); i < t.num();
/// ... t = t.next())` — dipakai caller utk mencatat warning degradasi saat
/// body di-elide.
pub fn is_enum_iteration_loop(
    init: Option<&Stmt>,
    cond: Option<&Expr>,
    step: Option<&Stmt>,
) -> bool {
    cond.map(expr_uses_enum_method).unwrap_or(false)
        || step.map(stmt_uses_enum_method).unwrap_or(false)
        || init.map(stmt_uses_enum_method).unwrap_or(false)
}

fn stmt_contains_break_continue(s: &Stmt) -> bool {
    match s {
        Stmt::Break | Stmt::Continue => true,
        Stmt::Block { stmts, .. }
        | Stmt::LoopForever { stmts }
        | Stmt::NamedBlock { stmts, .. } => stmts_contain_break_continue(stmts),
        Stmt::IfElse {
            true_branch,
            false_branch,
            ..
        } => {
            stmt_contains_break_continue(true_branch)
                || false_branch
                    .as_ref()
                    .is_some_and(|fb| stmt_contains_break_continue(fb))
        }
        // Loop bersarang: break/continue milik loop dalam — jangan turun.
        _ => false,
    }
}

/// Kumpulkan nama loop variable dari SEMUA `for` loop di statements (rekursif).
/// Dipakai pre-pass elaborasi: loop var bukan signal module, tapi runtime
/// LoopFor memakai signal — daftarkan sebagai signal sintetis 32-bit dulu.
pub fn collect_loop_var_names(stmts: &[Stmt], out: &mut Vec<Symbol>) {
    for s in stmts {
        collect_loop_var_names_stmt(s, out);
    }
}

/// Kumpulkan nama var dari `init`/`step` LoopFor — mendukung `Stmt::Block`
/// (for-init/step multi-var `int i=0, j=10; i++, j--` kini dipecah jadi Block
/// assignment; probe x03).
fn collect_loop_init_names(s: Option<&Stmt>, out: &mut Vec<Symbol>) {
    match s {
        Some(Stmt::BlockingAssign {
            lhs: Expr::Ident { name, .. },
            ..
        }) => {
            if !out.contains(name) {
                out.push(*name);
            }
        }
        Some(Stmt::Block { stmts }) => {
            for st in stmts {
                collect_loop_init_names(Some(st), out);
            }
        }
        _ => {}
    }
}

fn collect_loop_var_names_stmt(stmt: &Stmt, out: &mut Vec<Symbol>) {
    match stmt {
        Stmt::Block { stmts } | Stmt::LoopForever { stmts } => collect_loop_var_names(stmts, out),
        Stmt::BlockingAssign { .. }
        | Stmt::NonBlockingAssign { .. }
        | Stmt::StmtAssign { .. }
        | Stmt::Null
        | Stmt::SysFinish
        | Stmt::Break
        | Stmt::Continue
        | Stmt::Return(_)
        | Stmt::Expr { .. }
        | Stmt::SysCall { .. }
        | Stmt::EventTrigger { .. }
        | Stmt::Release { .. }
        | Stmt::Deassign { .. }
        | Stmt::Disable { .. }
        | Stmt::Force { .. } => {}
        Stmt::IfElse {
            true_branch,
            false_branch,
            ..
        } => {
            collect_loop_var_names_stmt(true_branch, out);
            if let Some(fb) = false_branch {
                collect_loop_var_names_stmt(fb, out);
            }
        }
        Stmt::Case { items, default, .. }
        | Stmt::CaseX { items, default, .. }
        | Stmt::CaseZ { items, default, .. }
        | Stmt::StmtCase { items, default, .. }
        | Stmt::UniqueCase { items, default, .. }
        | Stmt::PriorityCase { items, default, .. }
        | Stmt::Unique0Case { items, default, .. }
        | Stmt::CaseInside { items, default, .. } => {
            for item in items {
                collect_loop_var_names_stmt(&item.stmt, out);
            }
            if let Some(d) = default {
                collect_loop_var_names_stmt(d, out);
            }
        }
        Stmt::LoopFor {
            init, step, stmts, ..
        } => {
            collect_loop_init_names(init.as_deref(), out);
            if let Some(s) = init {
                collect_loop_var_names_stmt(s, out);
            }
            if let Some(s) = step {
                collect_loop_var_names_stmt(s, out);
            }
            collect_loop_var_names(stmts, out);
        }
        Stmt::LoopWhile { stmts, .. } | Stmt::DoWhile { stmts, .. } => {
            collect_loop_var_names(stmts, out)
        }
        Stmt::Repeat { stmts, .. } => collect_loop_var_names(stmts, out),
        Stmt::Wait { stmt: Some(s), .. } => {
            collect_loop_var_names_stmt(s, out);
        }
        Stmt::EventControl { stmt: Some(s), .. } => {
            collect_loop_var_names_stmt(s, out);
        }
        Stmt::NamedBlock { stmts, .. } => collect_loop_var_names(stmts, out),
        Stmt::ForeachLoop { stmts, .. } => collect_loop_var_names(stmts, out),
        Stmt::RandCase { items } => {
            for item in items {
                collect_loop_var_names_stmt(&item.stmt, out);
            }
        }
        Stmt::Fork { processes, .. } => {
            for p in processes {
                collect_loop_var_names_stmt(p, out);
            }
        }
        Stmt::Delay { stmt, .. } => {
            collect_loop_var_names_stmt(stmt, out);
        }
        _ => {}
    }
}

/// Substitusi loop variable di semua statements.
pub fn substitute_loop_var_in_stmts(stmts: &[Stmt], var_name: &str, value: i64) -> Vec<Stmt> {
    stmts
        .iter()
        .map(|s| substitute_loop_var_in_stmt(s, var_name, value))
        .collect()
}

/// Substitusi loop variable di satu statement.
pub fn substitute_loop_var_in_stmt(stmt: &Stmt, var_name: &str, value: i64) -> Stmt {
    match stmt {
        Stmt::Block { stmts } => Stmt::Block {
            stmts: substitute_loop_var_in_stmts(stmts, var_name, value),
        },
        Stmt::BlockingAssign { lhs, rhs, delay } => Stmt::BlockingAssign {
            lhs: substitute_loop_var_in_expr(lhs, var_name, value),
            rhs: substitute_loop_var_in_expr(rhs, var_name, value),
            delay: delay.clone(),
        },
        Stmt::NonBlockingAssign { lhs, rhs, delay } => Stmt::NonBlockingAssign {
            lhs: substitute_loop_var_in_expr(lhs, var_name, value),
            rhs: substitute_loop_var_in_expr(rhs, var_name, value),
            delay: delay.clone(),
        },
        Stmt::IfElse {
            cond,
            true_branch,
            false_branch,
        } => Stmt::IfElse {
            cond: substitute_loop_var_in_expr(cond, var_name, value),
            true_branch: Box::new(substitute_loop_var_in_stmt(true_branch, var_name, value)),
            false_branch: false_branch
                .as_ref()
                .map(|fb| Box::new(substitute_loop_var_in_stmt(fb, var_name, value))),
        },
        Stmt::Case {
            expr,
            items,
            default,
        } => Stmt::Case {
            expr: substitute_loop_var_in_expr(expr, var_name, value),
            items: items
                .iter()
                .map(|item| mivon_ast::stmt::CaseItem {
                    labels: item
                        .labels
                        .iter()
                        .map(|l| substitute_loop_var_in_expr(l, var_name, value))
                        .collect(),
                    stmt: Box::new(substitute_loop_var_in_stmt(&item.stmt, var_name, value)),
                })
                .collect(),
            default: default
                .as_ref()
                .map(|d| Box::new(substitute_loop_var_in_stmt(d, var_name, value))),
        },
        Stmt::StmtAssign { lhs, rhs } => Stmt::StmtAssign {
            lhs: substitute_loop_var_in_expr(lhs, var_name, value),
            rhs: substitute_loop_var_in_expr(rhs, var_name, value),
        },
        Stmt::Delay { delay, stmt } => Stmt::Delay {
            delay: substitute_loop_var_in_expr(delay, var_name, value),
            stmt: Box::new(substitute_loop_var_in_stmt(stmt, var_name, value)),
        },
        Stmt::SysCall {
            name,
            args,
            line,
            col,
        } => Stmt::SysCall {
            name: *name,
            args: args
                .iter()
                .map(|a| substitute_loop_var_in_expr(a, var_name, value))
                .collect(),
            line: *line,
            col: *col,
        },
        Stmt::Expr { expr } => Stmt::Expr {
            expr: substitute_loop_var_in_expr(expr, var_name, value),
        },
        Stmt::CaseX {
            expr,
            items,
            default,
        } => Stmt::CaseX {
            expr: substitute_loop_var_in_expr(expr, var_name, value),
            items: items
                .iter()
                .map(|item| mivon_ast::stmt::CaseItem {
                    labels: item
                        .labels
                        .iter()
                        .map(|l| substitute_loop_var_in_expr(l, var_name, value))
                        .collect(),
                    stmt: Box::new(substitute_loop_var_in_stmt(&item.stmt, var_name, value)),
                })
                .collect(),
            default: default
                .as_ref()
                .map(|d| Box::new(substitute_loop_var_in_stmt(d, var_name, value))),
        },
        Stmt::CaseZ {
            expr,
            items,
            default,
        } => Stmt::CaseZ {
            expr: substitute_loop_var_in_expr(expr, var_name, value),
            items: items
                .iter()
                .map(|item| mivon_ast::stmt::CaseItem {
                    labels: item
                        .labels
                        .iter()
                        .map(|l| substitute_loop_var_in_expr(l, var_name, value))
                        .collect(),
                    stmt: Box::new(substitute_loop_var_in_stmt(&item.stmt, var_name, value)),
                })
                .collect(),
            default: default
                .as_ref()
                .map(|d| Box::new(substitute_loop_var_in_stmt(d, var_name, value))),
        },
        // unique/priority case juga harus di-substitute genvar-nya — tanpa ini
        // genvar di body (mis. `mod_no_intg_d[i_word*32+:32]` dalam
        // `unique case` OpenTitan otbn) tidak pernah diganti → error E2001
        // "signal 'i_word' not found" saat elaborasi.
        Stmt::UniqueCase {
            expr,
            items,
            default,
            kind,
        } => Stmt::UniqueCase {
            expr: substitute_loop_var_in_expr(expr, var_name, value),
            items: items
                .iter()
                .map(|item| mivon_ast::stmt::CaseItem {
                    labels: item
                        .labels
                        .iter()
                        .map(|l| substitute_loop_var_in_expr(l, var_name, value))
                        .collect(),
                    stmt: Box::new(substitute_loop_var_in_stmt(&item.stmt, var_name, value)),
                })
                .collect(),
            default: default
                .as_ref()
                .map(|d| Box::new(substitute_loop_var_in_stmt(d, var_name, value))),
            kind: *kind,
        },
        Stmt::PriorityCase {
            expr,
            items,
            default,
            kind,
        } => Stmt::PriorityCase {
            expr: substitute_loop_var_in_expr(expr, var_name, value),
            items: items
                .iter()
                .map(|item| mivon_ast::stmt::CaseItem {
                    labels: item
                        .labels
                        .iter()
                        .map(|l| substitute_loop_var_in_expr(l, var_name, value))
                        .collect(),
                    stmt: Box::new(substitute_loop_var_in_stmt(&item.stmt, var_name, value)),
                })
                .collect(),
            default: default
                .as_ref()
                .map(|d| Box::new(substitute_loop_var_in_stmt(d, var_name, value))),
            kind: *kind,
        },
        Stmt::Unique0Case {
            expr,
            items,
            default,
            kind,
        } => Stmt::Unique0Case {
            expr: substitute_loop_var_in_expr(expr, var_name, value),
            items: items
                .iter()
                .map(|item| mivon_ast::stmt::CaseItem {
                    labels: item
                        .labels
                        .iter()
                        .map(|l| substitute_loop_var_in_expr(l, var_name, value))
                        .collect(),
                    stmt: Box::new(substitute_loop_var_in_stmt(&item.stmt, var_name, value)),
                })
                .collect(),
            default: default
                .as_ref()
                .map(|d| Box::new(substitute_loop_var_in_stmt(d, var_name, value))),
            kind: *kind,
        },
        Stmt::StmtCase {
            expr,
            items,
            default,
        } => Stmt::StmtCase {
            expr: substitute_loop_var_in_expr(expr, var_name, value),
            items: items
                .iter()
                .map(|item| mivon_ast::stmt::CaseItem {
                    labels: item
                        .labels
                        .iter()
                        .map(|l| substitute_loop_var_in_expr(l, var_name, value))
                        .collect(),
                    stmt: Box::new(substitute_loop_var_in_stmt(&item.stmt, var_name, value)),
                })
                .collect(),
            default: default
                .as_ref()
                .map(|d| Box::new(substitute_loop_var_in_stmt(d, var_name, value))),
        },
        Stmt::LoopForever { stmts } => Stmt::LoopForever {
            stmts: substitute_loop_var_in_stmts(stmts, var_name, value),
        },
        Stmt::LoopWhile { cond, stmts } => Stmt::LoopWhile {
            cond: substitute_loop_var_in_expr(cond, var_name, value),
            stmts: substitute_loop_var_in_stmts(stmts, var_name, value),
        },
        Stmt::LoopFor {
            init,
            cond,
            step,
            stmts,
        } => {
            // Shadowing guard: jika loop ini MENDEFINISIKAN loop var dengan
            // nama yang sama dengan `var_name` (mis. genvar `i` di generate-for
            // dan `for (int i = 0; ...)` di dalam body function yang sudah
            // di-inline ke generate body), maka referensi `i` di dalam loop
            // menunjuk ke loop var lokal, BUKAN genvar. Mensubstitusi akan
            // merusak init/step (`i = 0` jadi `0 = 0`) dan menghasilkan lvalue
            // konstanta. Loop var ini men-shadow genvar → jangan substitusi.
            let declares_shadow = init.as_deref().is_some_and(|s| match s {
                Stmt::BlockingAssign {
                    lhs: Expr::Ident { name, .. },
                    ..
                } => name.as_str() == var_name,
                _ => false,
            });
            if declares_shadow {
                return stmt.clone();
            }
            Stmt::LoopFor {
                init: init
                    .as_ref()
                    .map(|s| Box::new(substitute_loop_var_in_stmt(s, var_name, value))),
                cond: cond
                    .as_ref()
                    .map(|c| substitute_loop_var_in_expr(c, var_name, value)),
                step: step
                    .as_ref()
                    .map(|s| Box::new(substitute_loop_var_in_stmt(s, var_name, value))),
                stmts: substitute_loop_var_in_stmts(stmts, var_name, value),
            }
        }
        Stmt::Repeat { count, stmts } => Stmt::Repeat {
            count: substitute_loop_var_in_expr(count, var_name, value),
            stmts: substitute_loop_var_in_stmts(stmts, var_name, value),
        },
        Stmt::Wait { cond, stmt } => Stmt::Wait {
            cond: substitute_loop_var_in_expr(cond, var_name, value),
            stmt: stmt
                .as_ref()
                .map(|s| Box::new(substitute_loop_var_in_stmt(s, var_name, value))),
        },
        Stmt::Disable { name } => Stmt::Disable { name: *name },
        Stmt::Force { lhs, rhs } => Stmt::Force {
            lhs: substitute_loop_var_in_expr(lhs, var_name, value),
            rhs: substitute_loop_var_in_expr(rhs, var_name, value),
        },
        Stmt::Release { expr } => Stmt::Release {
            expr: substitute_loop_var_in_expr(expr, var_name, value),
        },
        Stmt::Deassign { expr } => Stmt::Deassign {
            expr: substitute_loop_var_in_expr(expr, var_name, value),
        },
        Stmt::Return(expr) => Stmt::Return(
            expr.as_ref()
                .map(|e| Box::new(substitute_loop_var_in_expr(e, var_name, value))),
        ),
        Stmt::Null => Stmt::Null,
        Stmt::SysFinish => Stmt::SysFinish,
        Stmt::EventControl { events, stmt } => Stmt::EventControl {
            events: events
                .iter()
                .map(|e| substitute_sensitivity_event(e, var_name, value))
                .collect(),
            stmt: stmt
                .as_ref()
                .map(|s| Box::new(substitute_loop_var_in_stmt(s, var_name, value))),
        },
        Stmt::EventTrigger { name } => Stmt::EventTrigger { name: *name },
        Stmt::ForeachLoop {
            array_var,
            index_vars,
            stmts,
        } => Stmt::ForeachLoop {
            array_var: *array_var,
            index_vars: index_vars.clone(),
            stmts: substitute_loop_var_in_stmts(stmts, var_name, value),
        },
        Stmt::NamedBlock { name, stmts, decls } => Stmt::NamedBlock {
            name: *name,
            stmts: substitute_loop_var_in_stmts(stmts, var_name, value),
            decls: decls.clone(),
        },
        Stmt::RandCase { items } => Stmt::RandCase {
            items: items
                .iter()
                .map(|rc| RandCaseItem {
                    weight: rc.weight,
                    stmt: Box::new(substitute_loop_var_in_stmt(&rc.stmt, var_name, value)),
                })
                .collect(),
        },
        Stmt::Break => Stmt::Break,
        Stmt::Continue => Stmt::Continue,
        Stmt::DoWhile { cond, stmts } => Stmt::DoWhile {
            cond: substitute_loop_var_in_expr(cond, var_name, value),
            stmts: substitute_loop_var_in_stmts(stmts, var_name, value),
        },
        _ => stmt.clone(),
    }
}

/// Substitusi sensitivity event (posedge/negedge/level) dengan nilai loop var.
pub fn substitute_sensitivity_event(
    event: &SensitivityEvent,
    var_name: &str,
    value: i64,
) -> SensitivityEvent {
    match event {
        SensitivityEvent::PosEdge(e) => {
            SensitivityEvent::PosEdge(substitute_loop_var_in_expr(e, var_name, value))
        }
        SensitivityEvent::NegEdge(e) => {
            SensitivityEvent::NegEdge(substitute_loop_var_in_expr(e, var_name, value))
        }
        SensitivityEvent::Level(e) => {
            SensitivityEvent::Level(substitute_loop_var_in_expr(e, var_name, value))
        }
        SensitivityEvent::Wildcard => SensitivityEvent::Wildcard,
        SensitivityEvent::Iff { event, cond } => SensitivityEvent::Iff {
            event: Box::new(substitute_sensitivity_event(event, var_name, value)),
            cond: substitute_loop_var_in_expr(cond, var_name, value),
        },
    }
}

/// Substitusi loop variable di expression AST.
pub fn substitute_loop_var_in_expr(expr: &Expr, var_name: &str, value: i64) -> Expr {
    match expr {
        Expr::Ident { name, .. } if name == var_name => {
            Expr::Value(mivon_ast::expr::Value::Decimal(value))
        }
        Expr::Ident { .. } => expr.clone(),
        Expr::Value(_) | Expr::String(_) | Expr::Null => expr.clone(),
        Expr::RangeSelect {
            expr: inner,
            msb,
            lsb,
        } => Expr::RangeSelect {
            expr: Box::new(substitute_loop_var_in_expr(inner, var_name, value)),
            msb: Box::new(substitute_loop_var_in_expr(msb, var_name, value)),
            lsb: Box::new(substitute_loop_var_in_expr(lsb, var_name, value)),
        },
        Expr::BitSelect { expr: inner, index } => Expr::BitSelect {
            expr: Box::new(substitute_loop_var_in_expr(inner, var_name, value)),
            index: Box::new(substitute_loop_var_in_expr(index, var_name, value)),
        },
        Expr::PartSelect {
            expr: inner,
            base,
            width,
        } => Expr::PartSelect {
            expr: Box::new(substitute_loop_var_in_expr(inner, var_name, value)),
            base: Box::new(substitute_loop_var_in_expr(base, var_name, value)),
            width: Box::new(substitute_loop_var_in_expr(width, var_name, value)),
        },
        Expr::Concat(exprs) => Expr::Concat(
            exprs
                .iter()
                .map(|e| substitute_loop_var_in_expr(e, var_name, value))
                .collect(),
        ),
        Expr::FuncCall {
            name,
            args,
            line,
            col,
        } => Expr::FuncCall {
            name: *name,
            args: args
                .iter()
                .map(|a| substitute_loop_var_in_expr(a, var_name, value))
                .collect(),
            line: *line,
            col: *col,
        },
        Expr::Replicate { count, expr: inner } => Expr::Replicate {
            count: Box::new(substitute_loop_var_in_expr(count, var_name, value)),
            expr: Box::new(substitute_loop_var_in_expr(inner, var_name, value)),
        },
        Expr::UnaryOp { op, expr: inner } => Expr::UnaryOp {
            op: op.clone(),
            expr: Box::new(substitute_loop_var_in_expr(inner, var_name, value)),
        },
        Expr::BinaryOp { op, lhs, rhs } => Expr::BinaryOp {
            op: op.clone(),
            lhs: Box::new(substitute_loop_var_in_expr(lhs, var_name, value)),
            rhs: Box::new(substitute_loop_var_in_expr(rhs, var_name, value)),
        },
        Expr::TernaryOp {
            cond,
            true_expr,
            false_expr,
        } => Expr::TernaryOp {
            cond: Box::new(substitute_loop_var_in_expr(cond, var_name, value)),
            true_expr: Box::new(substitute_loop_var_in_expr(true_expr, var_name, value)),
            false_expr: Box::new(substitute_loop_var_in_expr(false_expr, var_name, value)),
        },
        Expr::Paren(inner) => Expr::Paren(Box::new(substitute_loop_var_in_expr(
            inner, var_name, value,
        ))),
        Expr::IncDec {
            expr: inner,
            inc,
            pre,
        } => Expr::IncDec {
            expr: Box::new(substitute_loop_var_in_expr(inner, var_name, value)),
            inc: *inc,
            pre: *pre,
        },
        Expr::MethodCall {
            obj,
            method,
            args,
            with_clause,
        } => Expr::MethodCall {
            obj: Box::new(substitute_loop_var_in_expr(obj, var_name, value)),
            method: *method,
            args: args
                .iter()
                .map(|a| substitute_loop_var_in_expr(a, var_name, value))
                .collect(),
            with_clause: with_clause
                .clone()
                .map(|wc| Box::new(substitute_loop_var_in_expr(&wc, var_name, value))),
        },
        Expr::MemberAccess { obj, field } => Expr::MemberAccess {
            obj: Box::new(substitute_loop_var_in_expr(obj, var_name, value)),
            field: *field,
        },
        Expr::FillLit(val) => Expr::FillLit(*val),
        Expr::Inside {
            expr: inner,
            range_list,
        } => Expr::Inside {
            expr: Box::new(substitute_loop_var_in_expr(inner, var_name, value)),
            range_list: range_list
                .iter()
                .map(|e| substitute_loop_var_in_expr(e, var_name, value))
                .collect(),
        },
        Expr::StreamingConcat {
            op,
            slice_size,
            slices,
        } => Expr::StreamingConcat {
            op: op.clone(),
            slice_size: slice_size
                .as_ref()
                .map(|ss| Box::new(substitute_loop_var_in_expr(ss, var_name, value))),
            slices: slices
                .iter()
                .map(|e| substitute_loop_var_in_expr(e, var_name, value))
                .collect(),
        },
        Expr::Dist { expr, items } => Expr::Dist {
            expr: Box::new(substitute_loop_var_in_expr(expr, var_name, value)),
            items: items.clone(),
        },
        Expr::Cast { dtype, expr: inner } => Expr::Cast {
            dtype: *dtype,
            expr: Box::new(substitute_loop_var_in_expr(inner, var_name, value)),
        },
        Expr::CastWidth { width, expr: inner } => Expr::CastWidth {
            width: Box::new(substitute_loop_var_in_expr(width, var_name, value)),
            expr: Box::new(substitute_loop_var_in_expr(inner, var_name, value)),
        },
        Expr::ScopedIdent { package, item, .. } => Expr::ScopedIdent {
            package: *package,
            item: *item,
            line: 0,
            col: 0,
        },
        Expr::StructLit { members } => Expr::StructLit {
            members: members
                .iter()
                .map(|m| match m {
                    mivon_ast::expr::StructLitMember::Named(n, e) => {
                        mivon_ast::expr::StructLitMember::Named(
                            *n,
                            substitute_loop_var_in_expr(e, var_name, value),
                        )
                    }
                    mivon_ast::expr::StructLitMember::Positional(e) => {
                        mivon_ast::expr::StructLitMember::Positional(substitute_loop_var_in_expr(
                            e, var_name, value,
                        ))
                    }
                    mivon_ast::expr::StructLitMember::Default(e) => {
                        mivon_ast::expr::StructLitMember::Default(substitute_loop_var_in_expr(
                            e, var_name, value,
                        ))
                    }
                })
                .collect(),
        },
    }
}
