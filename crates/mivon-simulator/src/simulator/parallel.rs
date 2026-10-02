use crate::simulator::packed::PackedLogicVec;
use crate::simulator::packed_eval::{eval_binary_packed, is_packable_binary_op};
use crate::simulator::util::{is_signed_expr, sanitize_for_2state, string_to_logicvec};
use crate::simulator::value::*;
use mivon_core::error::SimError;
use mivon_ir::{
    BinaryIrOp, CaseType, IrExpr, IrLValue, IrStmt, LogicVal, LogicVec, SignalId, SignalInfo,
};
use std::sync::Arc;

// Semantik packed-eval untuk jalur paralel (BUG FIX fuZZ): evaluator paralel
// (SIM-28) memakai `eval_binary` (value.rs, pessimistic) sedangkan jalur
// serial memakai `eval_binary_packed` (tabel LRM) saat `use_packed_eval` —
// hasil beda utk X/Z (`x & 0`: packed=0, pessimistic=x). Menambah dead-code
// (EMI) bisa menggeser jumlah proses comb melewati ambang paralel →
// mismatch EMI palsu. Solusi: flag scoped thread-local — paralel meneruskan
// semantik `use_packed_eval` dari engine, konsisten dgn serial.
thread_local! {
    static PACKED_EVAL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Jalankan `f` dengan semantik packed-eval aktif (scoped, thread-local).
pub fn with_packed_eval<T>(use_packed: bool, f: impl FnOnce() -> T) -> T {
    let prev = PACKED_EVAL.with(|c| c.replace(use_packed));
    let r = f();
    PACKED_EVAL.with(|c| c.set(prev));
    r
}

#[inline]
fn packed_eval_enabled() -> bool {
    PACKED_EVAL.with(|c| c.get())
}

/// Pandangan sinyal untuk evaluasi paralel (SIM-28): base array + peta id
/// global→lokal + overlay tulis per-process. Per-process setup = O(0) (tanpa
/// clone seluruh sinyal). Baca: overlay dulu, lalu base via id_map. Tulis:
/// ke overlay (copy-on-write). Sinyal sintetis (Foreach) memakai id >=
/// id_map.len() agar tidak pernah bentrok dengan id sinyal global.
pub struct SignalView<'a> {
    base: &'a [Arc<LogicVec>],
    id_map: &'a [Option<usize>],
    overlay: &'a mut std::collections::HashMap<usize, Arc<LogicVec>>,
    next_synth: usize,
}

impl<'a> SignalView<'a> {
    pub fn new(
        base: &'a [Arc<LogicVec>],
        id_map: &'a [Option<usize>],
        overlay: &'a mut std::collections::HashMap<usize, Arc<LogicVec>>,
    ) -> Self {
        SignalView {
            base,
            id_map,
            overlay,
            next_synth: id_map.len(),
        }
    }

    #[inline]
    pub fn get(&self, id: usize) -> Option<&Arc<LogicVec>> {
        if let Some(v) = self.overlay.get(&id) {
            return Some(v);
        }
        if id < self.id_map.len() {
            match self.id_map[id] {
                Some(i) => self.base.get(i),
                None => None,
            }
        } else {
            None
        }
    }

    /// Tulis sinyal ke overlay local process (copy-on-write).
    #[inline]
    pub fn set(&mut self, id: usize, val: Arc<LogicVec>) {
        self.overlay.insert(id, val);
    }

    /// Id sintetis berikutnya untuk Foreach (>= id_map.len()).
    #[inline]
    pub fn len(&self) -> usize {
        self.next_synth
    }

    /// Apakah belum ada signal sintetis.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.overlay.is_empty()
    }

    /// Push sinyal sintetis (Foreach index variable) ke overlay.
    #[inline]
    pub fn push(&mut self, val: Arc<LogicVec>) {
        self.overlay.insert(self.next_synth, val);
        self.next_synth += 1;
    }
}

/// Configuration for parallel execution
#[derive(Debug, Clone, Copy)]
pub struct ParallelConfig {
    /// Number of worker threads (0 = auto-detect)
    pub num_threads: usize,
    /// Enable parallel process evaluation
    pub parallel_processes: bool,
    /// Enable parallel signal snapshot
    pub parallel_snapshot: bool,
    /// Minimum number of processes before parallelizing
    pub min_processes_parallel: usize,
    /// Minimum number of signals before parallelizing
    pub min_signals_parallel: usize,
}

impl Default for ParallelConfig {
    fn default() -> Self {
        let num_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        ParallelConfig {
            num_threads,
            parallel_processes: true,
            parallel_snapshot: true,
            min_processes_parallel: 4,
            min_signals_parallel: 64,
        }
    }
}

// ---------------------------------------------------------------------------
// Simplified expression evaluation for parallel context.
/// This version does NOT need &IrDesign, making it safe to use in rayon closures.
/// It handles the common expression types found in combinational processes.
/// `sig_info` (SignalInfo per signal) dipakai `is_signed_expr` agar
/// perbandingan/div/mod SIGNED konsisten dengan jalur serial (ROUND 36) —
/// parallel eval default-on untuk Process::Combinational.
pub fn evaluate_expr_simple(
    expr: &IrExpr,
    signals: &SignalView,
    sig_info: &[SignalInfo],
) -> Result<LogicVec, SimError> {
    match expr {
        IrExpr::Const(val) => Ok(val.clone()),
        IrExpr::FillLit(val) => Ok(LogicVec::fill(*val, 1)),
        IrExpr::Signal(id, _) => {
            let mut val = signals
                .get(*id)
                .map(|a| (**a).clone())
                .unwrap_or_else(|| LogicVec::new(1));
            // 2-state coercion pada READ — mirror serial eval/expr.rs:207.
            // Sinyal 2-state (`bit`) yang kebetulan berisi X/Z harus dibaca
            // sbg 0; parallel path sebelumnya membaca X mentah → differential
            // vs serial (fuzzer: traffic.mv `bit red` = X di jalur dag).
            sanitize_for_2state(sig_info, *id, &mut val);
            Ok(val)
        }
        IrExpr::RangeSelect(sig_id, msb, lsb) => {
            if let Some(val) = signals.get(*sig_id) {
                let (start, end) = if *msb > *lsb {
                    (*lsb, *msb)
                } else {
                    (*msb, *lsb)
                };
                // LRM 1800 §11.5.1: bit luar batas → X, bit dalam batas
                // tetap NILAI ASLI — identik dengan jalur serial eval/expr.rs.
                // Dulu `end >= n` mem-X-kan SELURUH hasil sehingga part-select
                // sebagian OOB (`a[3:0]` pada `a` 2-bit) = xxxx di jalur
                // parallel vs xx01 di jalur serial → mismatch EMI fuzzer
                // (dead-code menambah proses comb → jalur parallel aktif).
                let n = val.bits.len();
                if start > end || n == 0 {
                    return Ok(LogicVec::fill(LogicVal::X, (end - start + 1).max(1)));
                }
                let w = end - start + 1;
                let mut bits = Vec::with_capacity(w);
                for i in start..=end {
                    bits.push(val.bits.get(i).copied().unwrap_or(LogicVal::X));
                }
                Ok(LogicVec { width: w, bits })
            } else {
                Ok(LogicVec::new(1))
            }
        }
        IrExpr::BitSelect(sig_id, idx) => {
            let val = signals
                .get(*sig_id)
                .map(|a| (**a).clone())
                .unwrap_or_else(|| LogicVec::new(1));
            let bit = val.bits.get(*idx).copied().unwrap_or(LogicVal::X);
            Ok(LogicVec {
                bits: vec![bit],
                width: 1,
            })
        }
        IrExpr::ExprRangeSelect(inner, msb, lsb) => {
            let val = evaluate_expr_simple(inner, signals, sig_info)?;
            let (start, end) = if *msb > *lsb {
                (*lsb, *msb)
            } else {
                (*msb, *lsb)
            };
            // §11.5.1 per-bit, identik dengan jalur serial eval/expr.rs
            // (ExprRangeSelect): guard hanya start>end; bit OOB → X, bit
            // dalam batas tetap nilai asli.
            if start > end {
                return Ok(LogicVec::fill(LogicVal::X, (end - start + 1).max(1)));
            }
            let w = end - start + 1;
            let mut bits = Vec::with_capacity(w);
            for i in start..=end {
                bits.push(val.bits.get(i).copied().unwrap_or(LogicVal::X));
            }
            Ok(LogicVec { width: w, bits })
        }
        IrExpr::ExprBitSelect(inner, idx) => {
            let val = evaluate_expr_simple(inner, signals, sig_info)?;
            let bit = val.bits.get(*idx).copied().unwrap_or(LogicVal::X);
            Ok(LogicVec {
                bits: vec![bit],
                width: 1,
            })
        }
        IrExpr::ExprPartSelect(inner, base_expr, width_expr) => {
            let val = evaluate_expr_simple(inner, signals, sig_info)?;
            let base_w = evaluate_expr_simple(base_expr, signals, sig_info)?.width;
            let base = evaluate_expr_simple(base_expr, signals, sig_info)?.to_u64() as usize;
            let width = evaluate_expr_simple(width_expr, signals, sig_info)?.to_u64() as usize;
            if width == 0 {
                return Ok(LogicVec::new(1));
            }
            // LRM 1800 §11.5.1 + realita Icarus: bit luar batas → x, bit
            // dalam batas tetap nilai asli. Base ter-wrap negatif (hasil
            // rewrite parser `a[b -: ws]` → Const/Sub) direinterpretasi
            // two's-complement pada lebar ekspresi base; base unsigned
            // murni tetap unsigned — selaras dengan evaluator utama
            // engine/eval/expr.rs.
            let neg_ok = matches!(
                base_expr.as_ref(),
                IrExpr::Const(_) | IrExpr::BinaryOp(BinaryIrOp::Sub, ..)
            );
            let neg_base: i128 = if neg_ok
                && base >= val.bits.len()
                && base_w > 0
                && base_w < 64
                && ((base as u64) >> (base_w - 1)) & 1 == 1
            {
                ((base as u64) as i64).wrapping_sub(1i64 << base_w) as i128
            } else {
                base as i128
            };
            let mut bits = Vec::with_capacity(width);
            for k in 0..width {
                let idx = neg_base + k as i128;
                if idx >= 0 {
                    bits.push(val.bits.get(idx as usize).copied().unwrap_or(LogicVal::X));
                } else {
                    bits.push(LogicVal::X);
                }
            }
            Ok(LogicVec {
                width: bits.len(),
                bits,
            })
        }
        IrExpr::ArrayIndex {
            sig_id,
            index,
            elem_width,
        } => {
            let key_val = evaluate_expr_simple(index, signals, sig_info)?;
            let idx = key_val.to_u64() as usize;
            if let Some(array_val) = signals.get(*sig_id) {
                let start = idx * elem_width;
                let end = start + elem_width - 1;
                // SELALU hasilkan elem_width bit — index OOB di-pad X (sama
                // dengan jalur serial eval/expr.rs). Versi lama membatasi loop
                // ke array_val.width sehingga index OOB menghasilkan `bits`
                // KOSONG tapi width=elem_width>0 → panic di value.rs:28
                // (to_bitmasks) saat hasilnya dipakai op berikutnya.
                let mut bits = Vec::with_capacity(*elem_width);
                for i in start..=end {
                    bits.push(array_val.bits.get(i).copied().unwrap_or(LogicVal::X));
                }
                Ok(LogicVec {
                    width: *elem_width,
                    bits,
                })
            } else {
                Ok(LogicVec::new(*elem_width))
            }
        }
        IrExpr::Concat(exprs) => {
            let mut result = LogicVec::new(0);
            for e in exprs.iter().rev() {
                let part = evaluate_expr_simple(e, signals, sig_info)?;
                result = result.extend(&part);
            }
            Ok(result)
        }
        IrExpr::Replicate(count, inner) => {
            let val = evaluate_expr_simple(inner, signals, sig_info)?;
            let mut result = LogicVec::new(0);
            for _ in 0..*count {
                result = result.extend(&val);
            }
            Ok(result)
        }
        IrExpr::UnaryOp(op, inner) => {
            let val = evaluate_expr_simple(inner, signals, sig_info)?;
            Ok(eval_unary(op.clone(), &val))
        }
        IrExpr::BinaryOp(op, lhs, rhs) => {
            let lhs_val = evaluate_expr_simple(lhs, signals, sig_info)?;
            let rhs_val = evaluate_expr_simple(rhs, signals, sig_info)?;
            if matches!(
                op,
                BinaryIrOp::Lt
                    | BinaryIrOp::Le
                    | BinaryIrOp::Gt
                    | BinaryIrOp::Ge
                    | BinaryIrOp::Div
                    | BinaryIrOp::Mod
            ) && (is_signed_expr(lhs.as_ref(), sig_info)
                && is_signed_expr(rhs.as_ref(), sig_info))
            {
                Ok(eval_binary_signed(op.clone(), &lhs_val, &rhs_val))
            } else if matches!(op, BinaryIrOp::Sshr) {
                // `>>>`: arithmetic bila lhs signed, logical bila unsigned
                // (konsisten dengan jalur serial — ROUND 36).
                if is_signed_expr(lhs.as_ref(), sig_info) {
                    Ok(eval_sshr_signed(&lhs_val, &rhs_val))
                } else {
                    Ok(eval_binary(BinaryIrOp::Shr, &lhs_val, &rhs_val))
                }
            } else {
                // BUG FIX (fuZZ): coba packed-eval dulu bila aktif (konsisten
                // dgn jalur serial expr.rs:470) — packed memakai tabel LRM
                // (0&X=0, 1|X=1) sementara eval_binary pessimistic memberi X
                // → hasil paralel & serial berbeda utk input X/Z.
                if packed_eval_enabled() && is_packable_binary_op(op) {
                    let pl = PackedLogicVec::from_logicvec(&lhs_val);
                    let pr = PackedLogicVec::from_logicvec(&rhs_val);
                    if let Some(r) = eval_binary_packed(op, &pl, &pr) {
                        return Ok(r.to_logicvec());
                    }
                }
                Ok(eval_binary(op.clone(), &lhs_val, &rhs_val))
            }
        }
        IrExpr::Cond(cond, true_val, false_val) => {
            let cond_val = evaluate_expr_simple(cond, signals, sig_info)?;
            let tv = evaluate_expr_simple(true_val, signals, sig_info)?;
            let fv = evaluate_expr_simple(false_val, signals, sig_info)?;
            // LRM §11.4.11: hasil selebar max(lebar kedua cabang) —
            // selaras jalur evaluator utama.
            let w = tv.width.max(fv.width);
            let tv = if tv.width < w { tv.resize(w) } else { tv };
            let fv = if fv.width < w { fv.resize(w) } else { fv };
            match cond_val.to_bool() {
                Some(true) => Ok(tv),
                Some(false) => Ok(fv),
                // IEEE 1800-2017 §11.4.11 Tabel 11-22: kondisi unknown/z →
                // hasil kombinasi bitwise (bit kedua-branch sama → nilai itu;
                // berbeda/x/z → X). Sebelumnya jatuh ke cabang else
                // (`to_bool().unwrap_or(false)`) — salah utk operator
                // kondisional: `x ? a : b` harus menghasilkan X, bukan b.
                // Differential fuzzer: RTL real opentitan/openc910 —
                // `dbgon ? x : y` dgn dbgon=X → serial X, parallel cabang
                // else (bug_0096 piu_sysio_jdb_pm, bug_0032 complete_d).
                None => {
                    let mut bits = Vec::with_capacity(w);
                    for i in 0..w {
                        let out = match (tv.bits.get(i), fv.bits.get(i)) {
                            (Some(LogicVal::Zero), Some(LogicVal::Zero)) => LogicVal::Zero,
                            (Some(LogicVal::One), Some(LogicVal::One)) => LogicVal::One,
                            _ => LogicVal::X,
                        };
                        bits.push(out);
                    }
                    Ok(LogicVec { bits, width: w })
                }
            }
        }
        IrExpr::Signed(inner) => evaluate_expr_simple(inner, signals, sig_info),
        IrExpr::String(s) => Ok(string_to_logicvec(s)),
        IrExpr::Cast { width, expr } => {
            let val = evaluate_expr_simple(expr, signals, sig_info)?;
            Ok(val.resize(*width))
        }
        IrExpr::Inside { expr: inner, list } => {
            let val = evaluate_expr_simple(inner, signals, sig_info)?;
            for item in list {
                let item_val = evaluate_expr_simple(item, signals, sig_info)?;
                if val == item_val || val.casex_eq(&item_val) {
                    return Ok(LogicVec::from_u64(1, 1));
                }
            }
            Ok(LogicVec::from_u64(0, 1))
        }
        // HierRef read: nama interface-flatten (`bus.req` → `b.req`).
        // Resolve sama seperti write (parallel.rs HierRef write) —
        // fallback `_` di bawah memberi X 32-bit → slave baca `bus.req`
        // jadi X → beda dari serial (interface differential).
        IrExpr::HierRef(name) => match resolve_hier_signal(name.as_str(), sig_info) {
            Some(id) => Ok(signals
                .get(id)
                .map(|a| (**a).clone())
                .unwrap_or_else(|| LogicVec::new(1))),
            None => Ok(LogicVec::new(32)),
        },
        // MemberAccess pada struct signal yang TIDAK punya objek (field tak
        // ditemukan saat elaborasi — tipe unresolved, mis. `bkdr_loader_pkg::
        // bkdr_req_t` dari OpenTitan prim_rom yang di-mutasi fuzz). Serial
        // engine eval/expr.rs:2046 get_object gagal → LogicVec::new(1) (1-bit
        // X). Sebelumnya DAG jatuh ke `_ => new(32)` = 32-bit SEMUA X →
        // addr_bkdr = X,X,X vs serial X,0,0 → differential (fuzzer sim seed
        // 11 prim_rom). Mirror serial: obj/field tak relevan di path paralel
        // (objek class TIDAK di-parallel-kan — hanya Combinational).
        IrExpr::MemberAccess { .. } => Ok(LogicVec::new(1)),
        IrExpr::MethodCall { .. } => Ok(LogicVec::new(32)),
        // Streaming concat `{<<{...}}` / `{>>{...}}` — mirror serial
        // eval/expr.rs agar DAG-parallel identik (ditemukan mivon-fuzz:
        // `{<<{ {4{1'b1}} }}` = 0f di serial, X di dag → differential).
        IrExpr::StreamingConcat {
            op: _,
            slice_size,
            slices,
        } => {
            let mut vals = Vec::new();
            for sl in slices {
                vals.push(evaluate_expr_simple(sl, signals, sig_info)?);
            }
            let all_bits: Vec<LogicVal> =
                vals.iter().flat_map(|v| v.bits.iter().copied()).collect();
            let slen = slice_size.unwrap_or(1).max(1);
            let mut result = Vec::new();
            // Reverse slice order (arah stream: `<<`/`>>` sama di jalur serial).
            for chunk in all_bits.chunks(slen).rev() {
                result.extend(chunk.iter());
            }
            Ok(LogicVec {
                width: result.len(),
                bits: result,
            })
        }
        // Variant TANPA arm di jalur parallel: FuncCall / DpiCall / SysFunc /
        // NewCall / Dist / UdpLookup / VifBinding / VirtualIfaceAccess /
        // InsideRange (di luar konteks Inside). Dulu jatuh ke `new(32)` =
        // 32-bit X SENYAP → differential DAG vs default (fuzzer bug_0508:
        // `assign mem = {pkg::jalr(...), pkg::lui(...)}` → default 0 (DPI
        // stub return 0) vs dag X×64). Kini Err → pemanggil core.rs
        // fallback-kan layer itu ke evaluasi SERIAL (benar > cepat).
        other => Err(mivon_core::error::SimError::with_diag(
            mivon_core::diagnostics::DiagCode::InternalError,
            format!(
                "ekspresi tak didukung jalur parallel (fallback serial): {}",
                variant_name(other)
            ),
        )),
    }
}

/// Nama variant IrExpr utk pesan error fallback (debug singkat).
fn variant_name(e: &IrExpr) -> &'static str {
    match e {
        IrExpr::Const(_) => "Const",
        IrExpr::FillLit(_) => "FillLit",
        IrExpr::Signal(..) => "Signal",
        IrExpr::RangeSelect(..) => "RangeSelect",
        IrExpr::BitSelect(..) => "BitSelect",
        IrExpr::ExprRangeSelect(..) => "ExprRangeSelect",
        IrExpr::ExprBitSelect(..) => "ExprBitSelect",
        IrExpr::ExprPartSelect(..) => "ExprPartSelect",
        IrExpr::ArrayIndex { .. } => "ArrayIndex",
        IrExpr::Concat(_) => "Concat",
        IrExpr::Replicate(..) => "Replicate",
        IrExpr::UnaryOp(..) => "UnaryOp",
        IrExpr::BinaryOp(..) => "BinaryOp",
        IrExpr::Cond(..) => "Cond",
        IrExpr::Signed(_) => "Signed",
        IrExpr::String(_) => "String",
        IrExpr::SysFunc { .. } => "SysFunc",
        IrExpr::NewCall { .. } => "NewCall",
        IrExpr::MethodCall { .. } => "MethodCall",
        IrExpr::MemberAccess { .. } => "MemberAccess",
        IrExpr::DpiCall { .. } => "DpiCall",
        IrExpr::HierRef(_) => "HierRef",
        IrExpr::Inside { .. } => "Inside",
        IrExpr::InsideRange { .. } => "InsideRange",
        IrExpr::Cast { .. } => "Cast",
        IrExpr::StreamingConcat { .. } => "StreamingConcat",
        IrExpr::Dist { .. } => "Dist",
        IrExpr::UdpLookup { .. } => "UdpLookup",
        IrExpr::FuncCall { .. } => "FuncCall",
        IrExpr::VifBinding { .. } => "VifBinding",
        IrExpr::VirtualIfaceAccess { .. } => "VirtualIfaceAccess",
        IrExpr::This => "This",
        IrExpr::RealConst(_) => "RealConst",
        IrExpr::IncDec { .. } => "IncDec",
    }
}

/// Resolve nama hierarkis ke SignalId: exact → suffix `.name` → last-segment
/// unik. Dipakai read & write parallel (interface flatten `bus.req` = `b.req`).
fn resolve_hier_signal(name: &str, sig_info: &[SignalInfo]) -> Option<usize> {
    let id = sig_info.iter().position(|s| s.name.as_str() == name);
    if let Some(id) = id {
        return Some(id);
    }
    let suffix = format!(".{}", name);
    let id = sig_info
        .iter()
        .position(|s| s.name.as_str().ends_with(&suffix));
    if let Some(id) = id {
        return Some(id);
    }
    let last = name.rsplit('.').next().unwrap_or(name);
    if last.is_empty() {
        return None;
    }
    let seg_suffix = format!(".{}", last);
    let id = sig_info
        .iter()
        .position(|s| s.name.as_str().ends_with(&seg_suffix));
    if let Some(id) = id {
        return Some(id);
    }
    // Unik per last-segment.
    let candidates: Vec<usize> = sig_info
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            s.name
                .as_str()
                .rsplit('.')
                .next()
                .map(|seg| seg == last)
                .unwrap_or(false)
        })
        .map(|(i, _)| i)
        .collect();
    if candidates.len() == 1 {
        Some(candidates[0])
    } else {
        None
    }
}

/// Evaluate a block of IR statements against a mutable signal array,
/// collecting writes for later application.
/// This is the parallel-safe version that doesn't need SimulationEngine or IrDesign.
pub fn evaluate_stmt_block_parallel(
    stmts: &[IrStmt],
    signals: &mut SignalView,
    writes: &mut Vec<(SignalId, LogicVec)>,
    sig_info: &[SignalInfo],
) -> Result<(), SimError> {
    for stmt in stmts {
        match stmt {
            IrStmt::Block { stmts: inner } => {
                evaluate_stmt_block_parallel(inner, signals, writes, sig_info)?;
            }
            IrStmt::BlockingAssign { lhs, rhs, delay: _ } => {
                let val = eval_assign_rhs_simple(rhs, lhs, signals, sig_info)?;
                write_lvalue_simple(lhs, val, signals, writes, sig_info)?;
            }
            IrStmt::NonBlockingAssign { lhs, rhs, delay: _ } => {
                let val = eval_assign_rhs_simple(rhs, lhs, signals, sig_info)?;
                write_lvalue_simple(lhs, val, signals, writes, sig_info)?;
            }
            IrStmt::If {
                cond,
                true_branch,
                false_branch,
            } => {
                let cond_val = evaluate_expr_simple(cond, signals, sig_info)?;
                if cond_val.to_bool().unwrap_or(false) {
                    evaluate_stmt_block_parallel(true_branch, signals, writes, sig_info)?;
                } else if !false_branch.is_empty() {
                    evaluate_stmt_block_parallel(false_branch, signals, writes, sig_info)?;
                }
            }
            IrStmt::Case {
                case_type,
                expr: case_expr,
                items,
                default,
            } => {
                let case_val = evaluate_expr_simple(case_expr, signals, sig_info)?;
                let mut matched = false;
                for case_item in items {
                    let mut item_matched = false;
                    for pat in &case_item.labels {
                        let eq = match (case_type, pat) {
                            (CaseType::Inside, IrExpr::InsideRange { lo, hi, .. }) => {
                                let lo_v = evaluate_expr_simple(lo, signals, sig_info)?.to_u64();
                                let hi_v = evaluate_expr_simple(hi, signals, sig_info)?.to_u64();
                                let v = case_val.to_u64();
                                v >= lo_v.min(hi_v) && v <= lo_v.max(hi_v)
                            }
                            _ => {
                                let pat_val = evaluate_expr_simple(pat, signals, sig_info)?;
                                match case_type {
                                    CaseType::CaseX => case_val.casex_eq(&pat_val),
                                    CaseType::CaseZ => case_val.casez_eq(&pat_val),
                                    CaseType::Normal | CaseType::Inside => {
                                        case_val.case_val_eq(&pat_val)
                                    }
                                    CaseType::Unique | CaseType::Unique0 | CaseType::Priority => {
                                        case_val.case_val_eq(&pat_val)
                                    }
                                }
                            }
                        };
                        if eq {
                            evaluate_stmt_block_parallel(
                                &case_item.body,
                                signals,
                                writes,
                                sig_info,
                            )?;
                            item_matched = true;
                            matched = true;
                            break;
                        }
                    }
                    if item_matched {
                        break;
                    }
                }
                if !matched && !default.is_empty() {
                    evaluate_stmt_block_parallel(default, signals, writes, sig_info)?;
                }
            }
            IrStmt::LoopFor {
                init,
                cond,
                step,
                body,
            } => {
                let mut iter_count = 0u64;
                if let Some(init_stmt) = init {
                    let cloned: IrStmt = init_stmt.as_ref().clone();
                    evaluate_stmt_block_parallel(&[cloned], signals, writes, sig_info)?;
                }
                while iter_count < 1_000_000 {
                    let cond_val = evaluate_expr_simple(cond, signals, sig_info)?;
                    if !cond_val.to_bool().unwrap_or(false) {
                        break;
                    }
                    evaluate_stmt_block_parallel(body, signals, writes, sig_info)?;
                    if let Some(step_stmt) = step {
                        let cloned: IrStmt = step_stmt.as_ref().clone();
                        evaluate_stmt_block_parallel(&[cloned], signals, writes, sig_info)?;
                    }
                    iter_count += 1;
                }
            }
            IrStmt::LoopWhile { cond, body } => {
                let mut iter_count = 0u64;
                while iter_count < 1_000_000 {
                    let cond_val = evaluate_expr_simple(cond, signals, sig_info)?;
                    if !cond_val.to_bool().unwrap_or(false) {
                        break;
                    }
                    evaluate_stmt_block_parallel(body, signals, writes, sig_info)?;
                    iter_count += 1;
                }
            }
            IrStmt::LoopDoWhile { cond, body } => {
                let mut iter_count = 0u64;
                loop {
                    evaluate_stmt_block_parallel(body, signals, writes, sig_info)?;
                    iter_count += 1;
                    if iter_count >= 1_000_000 {
                        break;
                    }
                    let cond_val = evaluate_expr_simple(cond, signals, sig_info)?;
                    if !cond_val.to_bool().unwrap_or(false) {
                        break;
                    }
                }
            }
            IrStmt::Repeat { count, body } => {
                let count_val = evaluate_expr_simple(count, signals, sig_info)?;
                let n = count_val.to_u64().min(1_000_000);
                for _ in 0..n {
                    evaluate_stmt_block_parallel(body, signals, writes, sig_info)?;
                }
            }
            IrStmt::Foreach {
                array_var,
                index_var: _,
                body,
            } => {
                let arr_val = evaluate_expr_simple(array_var, signals, sig_info)?;
                let elem_width = match array_var {
                    IrExpr::Signal(_, _) => {
                        // Try to estimate elem_width from signal array structure
                        // For simplicity, assume 1 bit per element if we can't determine
                        1
                    }
                    _ => 1,
                };
                let num_elems = arr_val.width.checked_div(elem_width).unwrap_or(0);
                let idx_sig = signals.len();
                signals.push(Arc::new(LogicVec::from_u64(0, 32)));
                for i in 0..num_elems.min(10_000) {
                    signals.set(idx_sig, Arc::new(LogicVec::from_u64(i as u64, 32)));
                    evaluate_stmt_block_parallel(body, signals, writes, sig_info)?;
                }
            }
            IrStmt::SysCall { .. } | IrStmt::SysFinish | IrStmt::Null => {}
            _ => {
                // Skip unsupported statement types in parallel eval.
                // These will be handled by the sequential fallback path.
                // Types skipped: Force, Release, Deassign, Wait, WaitOrder,
                // NamedBlock, Disable, EventControl, EventTrigger, Fork,
                // Assert, Assume, Cover, RandCase, RandSequence, Return.
            }
        }
    }
    Ok(())
}

/// Simplified assign RHS evaluation (no design reference needed)
fn eval_assign_rhs_simple(
    expr: &IrExpr,
    lhs: &IrLValue,
    signals: &SignalView,
    sig_info: &[SignalInfo],
) -> Result<LogicVec, SimError> {
    if let IrExpr::FillLit(v) = expr {
        let w = get_lvalue_width_simple(lhs, signals, sig_info);
        Ok(LogicVec::fill(*v, w))
    } else if let IrExpr::Signed(inner) = expr {
        let mut val = evaluate_expr_simple(inner, signals, sig_info)?;
        let target_w = get_lvalue_width_simple(lhs, signals, sig_info);
        if val.width < target_w {
            let msb = val.bits.last().copied().unwrap_or(LogicVal::Zero);
            val.bits.resize(target_w, msb);
            val.width = target_w;
        }
        Ok(val)
    } else {
        evaluate_expr_simple(expr, signals, sig_info)
    }
}

/// Get lvalue width (no design reference)
#[allow(clippy::only_used_in_recursion)]
fn get_lvalue_width_simple(
    lvalue: &IrLValue,
    signals: &SignalView,
    sig_info: &[SignalInfo],
) -> usize {
    match lvalue {
        IrLValue::Signal(id, _) => signals.get(*id).map(|s| s.width).unwrap_or(1),
        IrLValue::RangeSelect(_, msb, lsb) => {
            let (lo, hi) = if *msb > *lsb {
                (*lsb, *msb)
            } else {
                (*msb, *lsb)
            };
            hi - lo + 1
        }
        IrLValue::BitSelect(_, _) => 1,
        IrLValue::ArrayIndex { elem_width, .. } => *elem_width,
        IrLValue::ArrayRangeSelect {
            elem_width,
            msb,
            lsb,
            ..
        } => {
            let (lo, hi) = if *msb > *lsb {
                (*lsb, *msb)
            } else {
                (*msb, *lsb)
            };
            (hi - lo + 1) * elem_width
        }
        IrLValue::ArrayBitSelect { elem_width, .. } => *elem_width,
        IrLValue::ExprPartSelect { width, .. } => *width,
        IrLValue::HierRef(_) | IrLValue::HierRefIndex { .. } => 1,
        IrLValue::ObjectField { .. } => 64,
        IrLValue::Concat(items) => items
            .iter()
            .map(|i| get_lvalue_width_simple(i, signals, sig_info))
            .sum(),
    }
}

/// Simple write lvalue (no design reference)
fn write_lvalue_simple(
    lvalue: &IrLValue,
    val: LogicVec,
    signals: &mut SignalView,
    writes: &mut Vec<(SignalId, LogicVec)>,
    sig_info: &[SignalInfo],
) -> Result<(), SimError> {
    match lvalue {
        IrLValue::Signal(id, _) => {
            let target_width = signals.get(*id).map(|s| s.width).unwrap_or(1);
            let resized = if val.width != target_width {
                val.resize(target_width)
            } else {
                val
            };
            // Defensif: normalisasi nilai korup (width>0 tapi bits kosong)
            // agar tidak mencemari state dan memicu panic di jalur eval lain.
            let mut resized = if resized.width > 0 && resized.bits.is_empty() {
                LogicVec::fill(LogicVal::X, resized.width)
            } else {
                resized
            };
            // 2-state coercion pada WRITE — mirror serial lvalue.rs (X/Z→0
            // untuk target `bit`). Parallel path tak pernah sanitize →
            // `bit red = (state==RED)` dgn state=X = X di jalur dag, 0 di
            // serial (fuzzer: traffic.mv).
            sanitize_for_2state(sig_info, *id, &mut resized);
            signals.set(*id, Arc::new(resized.clone()));
            writes.push((*id, resized));
        }
        IrLValue::RangeSelect(sig_id, msb, lsb) => {
            let (start, end) = if *msb > *lsb {
                (*lsb, *msb)
            } else {
                (*msb, *lsb)
            };
            let mut existing = signals
                .get(*sig_id)
                .map(|a| (**a).clone())
                .unwrap_or_else(|| LogicVec::new(1));
            // LRM: `y[msb:lsb] = val` — val bit i → y bit (start+i), TANPA
            // reversal (konsisten serial lvalue.rs:285). Reversal lama
            // (`end - i`) menulis terbalik → `y[2:0] = 0xx` jadi `xx0`
            // (ditemukan fuzzer differential OpenC910 ct_had_dbg_info).
            for i in start..=end.min(existing.width.saturating_sub(1)) {
                let src_idx = (i - start).min(val.bits.len().saturating_sub(1));
                existing.bits[i] = val.bits.get(src_idx).copied().unwrap_or(LogicVal::X);
            }
            sanitize_for_2state(sig_info, *sig_id, &mut existing);
            signals.set(*sig_id, Arc::new(existing.clone()));
            writes.push((*sig_id, existing));
        }
        IrLValue::BitSelect(sig_id, idx) => {
            let mut existing = signals
                .get(*sig_id)
                .map(|a| (**a).clone())
                .unwrap_or_else(|| LogicVec::new(1));
            if *idx < existing.width {
                existing.bits[*idx] = val.bits.first().copied().unwrap_or(LogicVal::X);
            }
            sanitize_for_2state(sig_info, *sig_id, &mut existing);
            signals.set(*sig_id, Arc::new(existing.clone()));
            writes.push((*sig_id, existing));
        }
        IrLValue::ArrayIndex {
            sig_id,
            index,
            elem_width,
        } => {
            let idx_val = evaluate_expr_simple(index, signals, sig_info)?;
            let idx_u64 = idx_val.to_u64() as usize;
            let mut existing = signals
                .get(*sig_id)
                .map(|a| (**a).clone())
                .unwrap_or_else(|| LogicVec::new(1));
            let start = idx_u64 * elem_width;
            for i in 0..*elem_width {
                if start + i < existing.width {
                    existing.bits[start + i] = val.bits.get(i).copied().unwrap_or(LogicVal::X);
                }
            }
            sanitize_for_2state(sig_info, *sig_id, &mut existing);
            signals.set(*sig_id, Arc::new(existing.clone()));
            writes.push((*sig_id, existing));
        }
        IrLValue::ArrayRangeSelect {
            sig_id,
            index,
            elem_width,
            msb,
            lsb,
        } => {
            let idx_val = evaluate_expr_simple(index, signals, sig_info)?;
            let idx = idx_val.to_u64() as usize;
            let base = idx * elem_width;
            let (start, end) = if *msb > *lsb {
                (*lsb, *msb)
            } else {
                (*msb, *lsb)
            };
            let mut existing = signals
                .get(*sig_id)
                .map(|a| (**a).clone())
                .unwrap_or_else(|| LogicVec::new(1));
            // `arr[i][msb:lsb] = val` — val LSB → bit base+start, tanpa
            // reversal (konsisten serial lvalue.rs:397).
            let max_end = base + end.min(existing.width.saturating_sub(1).saturating_sub(base));
            for i in start..=end.min(existing.width.saturating_sub(1).saturating_sub(base)) {
                let src_idx = (i - start).min(val.bits.len().saturating_sub(1));
                let abs = base + i;
                if abs <= max_end && abs < existing.width {
                    existing.bits[abs] = val.bits.get(src_idx).copied().unwrap_or(LogicVal::X);
                }
            }
            sanitize_for_2state(sig_info, *sig_id, &mut existing);
            signals.set(*sig_id, Arc::new(existing.clone()));
            writes.push((*sig_id, existing));
        }
        IrLValue::ArrayBitSelect {
            sig_id,
            index,
            elem_width,
            bit,
        } => {
            let bit_val = evaluate_expr_simple(bit, signals, sig_info)?;
            let idx_val = evaluate_expr_simple(index, signals, sig_info)?;
            let idx = idx_val.to_u64() as usize;
            let abs = idx * elem_width + bit_val.to_u64() as usize;
            let mut existing = signals
                .get(*sig_id)
                .map(|a| (**a).clone())
                .unwrap_or_else(|| LogicVec::new(1));
            if abs < existing.width {
                existing.bits[abs] = val.bits.first().copied().unwrap_or(LogicVal::X);
            }
            sanitize_for_2state(sig_info, *sig_id, &mut existing);
            signals.set(*sig_id, Arc::new(existing.clone()));
            writes.push((*sig_id, existing));
        }
        IrLValue::ExprPartSelect {
            sig_id,
            base,
            width,
        } => {
            let idx_val = evaluate_expr_simple(base, signals, sig_info)?;
            let start = idx_val.to_u64() as usize;
            let mut existing = signals
                .get(*sig_id)
                .map(|a| (**a).clone())
                .unwrap_or_else(|| LogicVec::new(1));
            for i in 0..*width {
                if start + i < existing.width {
                    existing.bits[start + i] = val.bits.get(i).copied().unwrap_or(LogicVal::X);
                }
            }
            sanitize_for_2state(sig_info, *sig_id, &mut existing);
            signals.set(*sig_id, Arc::new(existing.clone()));
            writes.push((*sig_id, existing));
        }
        IrLValue::Concat(items) => {
            // LRM 1800-2017 §10.7: assignment ke concat lvalue — RHS
            // di-zero-extend ke lebar total concat, lalu dibagikan MSB-first
            // (part PERTAMA = bit paling tinggi). Handler lama mengiris
            // LSB-first (offset naik dari 0) → bit terbalik (`{co, s} = a+b`
            // menaruh LSB ke co) DAN mengisi X saat RHS lebih sempit dari
            // total concat (panic logic.rs:97 width=8 bits.len=7).
            let total: usize = items
                .iter()
                .map(|it| get_lvalue_width_simple(it, signals, sig_info))
                .sum();
            let mut bits = val.bits.clone();
            if bits.len() < total {
                bits.resize(total, LogicVal::Zero);
            } else if bits.len() > total {
                bits.truncate(total);
            }
            let mut offset = total;
            for item in items {
                let item_w = get_lvalue_width_simple(item, signals, sig_info);
                offset -= item_w;
                let sub_val = LogicVec {
                    width: item_w,
                    bits: bits[offset..offset + item_w].to_vec(),
                };
                write_lvalue_simple(item, sub_val, signals, writes, sig_info)?;
            }
        }
        // HierRef: nama signal sudah ter-flatten di design (mis. interface
        // field `b.req` → signal `__iface_...`). Cari di sig_info by name,
        // tulis lewat Signal. Tanpa ini DAG-parallel meng-drop interface
        // assign (`bus.req = 1'b1` di master) → b.req tetap X/0 (ditemukan
        // fuzzer differential).
        IrLValue::HierRef(name) => {
            // Nama interface-flatten (`bus.req` signal = `b.req`).
            let Some(id) = resolve_hier_signal(name.as_str(), sig_info) else {
                if std::env::var("MIVON_DBG_HIER").is_ok() {
                    let names: Vec<String> = sig_info
                        .iter()
                        .map(|s| s.name.as_str().to_string())
                        .take(60)
                        .collect();
                    eprintln!(
                        "[DBG-HIER] '{}' not found. signals={}",
                        name.as_str(),
                        names.join(", ")
                    );
                }
                return Err(mivon_core::error::SimError::runtime(format!(
                    "hierarchical signal '{}' not found for write (parallel)",
                    name.as_str()
                )));
            };
            let target_width = signals.get(id).map(|s| s.width).unwrap_or(1);
            let resized = if val.width != target_width {
                val.resize(target_width)
            } else {
                val
            };
            let mut resized = if resized.width > 0 && resized.bits.is_empty() {
                LogicVec::fill(LogicVal::X, resized.width)
            } else {
                resized
            };
            sanitize_for_2state(sig_info, id, &mut resized);
            signals.set(id, Arc::new(resized.clone()));
            writes.push((id, resized));
        }
        _ => {}
    }
    Ok(())
}

/// Parallel signal snapshot: create a copy of all signal values using rayon
pub fn parallel_snapshot(signals: &[LogicVec]) -> Vec<LogicVec> {
    use rayon::prelude::*;
    signals.par_iter().cloned().collect()
}
