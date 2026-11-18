//! MIR Optimizations — peephole and dataflow optimizations on MIR instructions.
//!
//! Pass pipeline (run to fixed point):
//! 1. constant_fold        — fold binary ops on constant operands
//! 2. copy_propagate       — fold unary ops with constant operand
//! 3. dead_store_eliminate — remove stores to registers never read (conservative: within basic blocks only)
//! 4. constant_branch_fold — replace branches on constant condition with Jump
//! 5. strength_reduce      — replace Mul/Div by power-of-2 with Shl/Shr
//! 6. remove_nops          — sweep NOP instructions
//! 7. sign_ext_eliminate   — eliminate redundant sign-extension patterns (COMP-08)
//! 8. xz_propagate         — fold operations with known-zero/X operands (COMP-09)

use super::mir::{MirBinOp, MirInstr, MirModule, MirSignal};
use std::collections::{HashMap, HashSet};

/// Optimize a MIR module (peephole + dataflow).
pub fn optimize_module(module: &mut MirModule) {
    for process in &mut module.processes {
        optimize_process(&mut process.instrs);
    }
    eliminate_unused_signals(module);
}

/// Eliminasi sinyal MIR yang tidak pernah dibaca/ditulis oleh instruksi
/// manapun (Load/Store/NonBlocking). Setelah menghapus sinyal tidak terpakai,
/// perbarui `signal_map` dan semua referensi `signal` id dalam instruksi agar
/// indeks tetap konsisten.
fn eliminate_unused_signals(module: &mut MirModule) -> bool {
    // Kumpulkan semua sinyal yang digunakan oleh instruksi.
    let mut used: HashSet<usize> = HashSet::new();
    for process in &module.processes {
        for instr in &process.instrs {
            match instr {
                MirInstr::Load { signal, .. } => {
                    used.insert(*signal);
                }
                MirInstr::Store { signal, .. } => {
                    used.insert(*signal);
                }
                MirInstr::NonBlocking { signal, .. } => {
                    used.insert(*signal);
                }
                _ => {}
            }
        }
    }

    // Bangun mapping indeks lama -> indeks baru untuk sinyal yang digunakan,
    // urutan dipertahankan.
    let mut old_to_new: HashMap<usize, usize> = HashMap::new();
    let mut kept: Vec<MirSignal> = Vec::new();
    for (old_idx, sig) in module.signals.iter().enumerate() {
        if used.contains(&old_idx) {
            let new_idx = kept.len();
            old_to_new.insert(old_idx, new_idx);
            kept.push(sig.clone());
        }
    }

    let removed = module.signals.len() - kept.len();
    if removed == 0 {
        return false;
    }

    // Ganti sinyal dan bangun ulang signal_map.
    module.signals = kept;
    module.signal_map = module
        .signals
        .iter()
        .enumerate()
        .map(|(new_idx, sig)| (sig.name, new_idx))
        .collect();

    // Perbarui semua referensi signal id dalam instruksi.
    for process in &mut module.processes {
        for instr in &mut process.instrs {
            match instr {
                MirInstr::Load { signal, .. } => {
                    if let Some(&new_idx) = old_to_new.get(signal) {
                        *signal = new_idx;
                    }
                }
                MirInstr::Store { signal, .. } => {
                    if let Some(&new_idx) = old_to_new.get(signal) {
                        *signal = new_idx;
                    }
                }
                MirInstr::NonBlocking { signal, .. } => {
                    if let Some(&new_idx) = old_to_new.get(signal) {
                        *signal = new_idx;
                    }
                }
                _ => {}
            }
        }
    }

    true
}

/// Optimize a list of MIR instructions to fixed point.
pub fn optimize_process(instrs: &mut Vec<MirInstr>) {
    let mut changed = true;
    while changed {
        changed = false;
        changed |= constant_fold(instrs);
        changed |= copy_propagate(instrs);
        changed |= dead_store_eliminate(instrs);
        changed |= constant_branch_fold(instrs);
        changed |= common_subexpr_eliminate(instrs);
        changed |= licm(instrs);
        changed |= strength_reduce(instrs);
        changed |= sign_ext_eliminate(instrs);
        changed |= xz_propagate(instrs);
        changed |= remove_nops(instrs);
    }
}

// ── Helper: Find Constant Value ──

/// Try to find the constant value loaded into a register by scanning backwards.
fn find_const_value(instrs: &[MirInstr], reg: usize) -> Option<u64> {
    for instr in instrs.iter().rev() {
        match instr {
            MirInstr::Const { dest, value, .. } if *dest == reg => return Some(*value),
            MirInstr::Load { dest, .. } if *dest == reg => return None,
            MirInstr::Binary { dest, .. } if *dest == reg => return None,
            MirInstr::Unary { dest, .. } if *dest == reg => return None,
            _ => {}
        }
    }
    None
}

// ── Pass 1: Constant Folding ──

/// Fold binary operations where both operands are constants.
fn constant_fold(instrs: &mut [MirInstr]) -> bool {
    let mut changed = false;
    let mut i = 0;
    while i < instrs.len() {
        if let MirInstr::Binary {
            op,
            dest,
            lhs,
            rhs,
            width,
        } = &instrs[i]
        {
            let lhs_val = find_const_value(instrs, *lhs);
            let rhs_val = find_const_value(instrs, *rhs);

            if let (Some(lv), Some(rv)) = (lhs_val, rhs_val) {
                let result = match op {
                    MirBinOp::Add => lv.wrapping_add(rv),
                    MirBinOp::Sub => lv.wrapping_sub(rv),
                    MirBinOp::Mul => lv.wrapping_mul(rv),
                    MirBinOp::Div => lv.checked_div(rv).unwrap_or(0),
                    MirBinOp::And => lv & rv,
                    MirBinOp::Or => lv | rv,
                    MirBinOp::Xor => lv ^ rv,
                    MirBinOp::Eq => (lv == rv) as u64,
                    MirBinOp::Ne => (lv != rv) as u64,
                    MirBinOp::Lt => (lv < rv) as u64,
                    MirBinOp::Le => (lv <= rv) as u64,
                    MirBinOp::Gt => (lv > rv) as u64,
                    MirBinOp::Ge => (lv >= rv) as u64,
                    MirBinOp::Shl => lv << rv.min(63),
                    MirBinOp::Shr => lv >> rv.min(63),
                    MirBinOp::Mod => {
                        if rv != 0 {
                            lv % rv
                        } else {
                            0
                        }
                    }
                    MirBinOp::LogicalAnd => (lv != 0 && rv != 0) as u64,
                    MirBinOp::LogicalOr => (lv != 0 || rv != 0) as u64,
                };
                instrs[i] = MirInstr::Const {
                    dest: *dest,
                    value: result,
                    width: *width,
                };
                changed = true;
            }
        }
        i += 1;
    }
    changed
}

// ── Pass 2: Copy Propagation (Unary Constant Fold) ──

/// Fold unary operations where the operand is constant.
/// Also replaces identity ops (x | 0, x & all-ones) with the operand.
fn copy_propagate(instrs: &mut [MirInstr]) -> bool {
    let mut changed = false;
    // Pre-scan: collect constant values from Const instructions
    let const_map: HashMap<usize, u64> = instrs
        .iter()
        .filter_map(|i| match i {
            MirInstr::Const { dest, value, .. } => Some((*dest, *value)),
            _ => None,
        })
        .collect();

    for instr in instrs.iter_mut() {
        if let MirInstr::Unary {
            op,
            dest,
            operand,
            width,
        } = instr
        {
            // If operand is constant, fold: !C, -C
            if let Some(&ov) = const_map.get(operand) {
                let raw_result = match op {
                    super::mir::MirUnOp::Not => !ov,
                    super::mir::MirUnOp::Neg => ov.wrapping_neg(),
                };
                let mask = if *width < 64 {
                    (1u64 << *width) - 1
                } else {
                    u64::MAX
                };
                *instr = MirInstr::Const {
                    dest: *dest,
                    value: raw_result & mask,
                    width: *width,
                };
                changed = true;
            }
        }
        // Note: Binary identity ops (x|0, x&all-ones) are intentionally
        // NOT folded here — the subsequent constant_fold pass in the
        // fixed-point pipeline handles them when the constant operand
        // is detected via find_const_value.
    }
    changed
}

// ── Pass 3: Dead Store Elimination (Conservative: within basic blocks only) ──

/// Eliminate stores to registers that are never read before being overwritten.
///
/// CONSERVATIVE: skips across Branch/Jump/Label boundaries to avoid
/// incorrect elimination across control flow paths.
fn dead_store_eliminate(instrs: &mut Vec<MirInstr>) -> bool {
    let mut last_write: HashMap<usize, usize> = HashMap::new();
    let mut changed = false;

    let mut i = 0;
    while i < instrs.len() {
        // Reset register tracking at control flow boundaries (conservative —
        // prevents incorrect elimination of writes that are needed in other paths)
        match &instrs[i] {
            MirInstr::Branch { .. } | MirInstr::Jump { .. } | MirInstr::Label(_) => {
                last_write.clear(); // reset: don't track across blocks
            }
            _ => {}
        }

        let written_reg = match &instrs[i] {
            MirInstr::Const { dest, .. }
            | MirInstr::Binary { dest, .. }
            | MirInstr::Unary { dest, .. } => Some(*dest),
            _ => None,
        };

        if let Some(reg) = written_reg {
            if let Some(&prev_idx) = last_write.get(&reg) {
                if prev_idx < i {
                    // Check if reg was NOT read between prev_idx+1 and i-1
                    let mut is_dead = true;
                    for ins in &instrs[(prev_idx + 1)..i] {
                        if reads_register(ins, reg) {
                            is_dead = false;
                            break;
                        }
                    }
                    if is_dead && !reads_register(&instrs[i], reg) {
                        instrs[prev_idx] = MirInstr::Nop;
                        changed = true;
                    }
                }
            }
            last_write.insert(reg, i);
        }
        i += 1;
    }

    if changed {
        remove_nops(instrs);
    }
    changed
}

/// Check if an instruction reads a specific register.
fn reads_register(instr: &MirInstr, reg: usize) -> bool {
    match instr {
        MirInstr::Binary { lhs, rhs, .. } => *lhs == reg || *rhs == reg,
        MirInstr::Unary { operand, .. } => *operand == reg,
        MirInstr::Store { src, .. } => *src == reg,
        MirInstr::NonBlocking { src, .. } => *src == reg,
        MirInstr::Branch { cond, .. } => *cond == reg,
        _ => false,
    }
}

// ── Pass 3b: Common Subexpression Elimination (COMP-04, conservative) ──

/// Eliminasi komputasi ulang yang identik dalam straight-line block:
/// `r4 = r2 + r1` setelah `r3 = r2 + r1` → Nop + rewrite semua pemakaian
/// `r4` menjadi `r3` (sampai `r4` ditulis ulang).
///
/// CONSERVATIVE:
/// - Table dibuang pada Branch/Jump/Label (tidak melintasi basic block).
/// - Hanya Binary/Unary murni; Load sinyal tidak di-CSE (bisa berubah).
/// - Operan harus belum ditulis ulang sejak entri di-cache.
fn common_subexpr_eliminate(instrs: &mut Vec<MirInstr>) -> bool {
    #[derive(Hash, PartialEq, Eq, Clone)]
    enum CseKey {
        Bin(String, usize, usize, usize),
        Un(String, usize, usize),
    }

    struct Entry {
        dest: usize,
        def_idx: usize,
        operand_defs: Vec<(usize, usize)>,
    }

    fn is_commutative(op: &str) -> bool {
        matches!(
            op,
            "Add" | "Mul" | "And" | "Or" | "Xor" | "Eq" | "Ne" | "LogicalAnd" | "LogicalOr"
        )
    }

    let mut changed = false;
    // defs: reg → posisi instruksi penulisan terakhir.
    let mut defs: HashMap<usize, usize> = HashMap::new();
    let mut table: HashMap<CseKey, Entry> = HashMap::new();

    for i in 0..instrs.len() {
        // Basic block boundary → reset analisis.
        if matches!(
            instrs[i],
            MirInstr::Branch { .. } | MirInstr::Jump { .. } | MirInstr::Label(_)
        ) {
            defs.clear();
            table.clear();
            continue;
        }

        match &instrs[i] {
            MirInstr::Binary {
                op,
                dest,
                lhs,
                rhs,
                width,
            } => {
                let (a, b) = if is_commutative(&format!("{:?}", op)) {
                    (*lhs.min(rhs), *lhs.max(rhs))
                } else {
                    (*lhs, *rhs)
                };
                let key = CseKey::Bin(format!("{:?}", op), a, b, *width);
                // Cek hit valid: operan tak berubah + dest cache belum
                // ditulis ulang sejak def aslinya.
                if let Some(e) = table.get(&key) {
                    let operands_unchanged =
                        e.operand_defs.iter().all(|(r, d)| defs.get(r) == Some(d));
                    let dest_fresh = defs.get(&e.dest) == Some(&e.def_idx);
                    if operands_unchanged && dest_fresh {
                        let dest = *dest;
                        let cached_dest = e.dest;
                        // Nop-kan komputasi ulang, lalu rewrite semua
                        // pemakaian dest → cached_dest sampai dest ditulis.
                        instrs[i] = MirInstr::Nop;
                        let mut j = i + 1;
                        while j < instrs.len() {
                            if matches!(
                                instrs[j],
                                MirInstr::Branch { .. }
                                    | MirInstr::Jump { .. }
                                    | MirInstr::Label(_)
                            ) {
                                break;
                            }
                            if writes_register(&instrs[j], dest) {
                                break;
                            }
                            rewrite_read_register(&mut instrs[j], dest, cached_dest);
                            j += 1;
                        }
                        defs.insert(dest, i);
                        changed = true;
                        continue;
                    }
                }
                // Cache komputasi ini (simpan posisi def terakhir operan).
                let mut operand_defs = Vec::new();
                for r in [a, b] {
                    operand_defs.push((r, defs.get(&r).copied()));
                }
                let _ = operand_defs;
                // Simpan dengan Option agar "belum pernah didefinisikan"
                // juga terekam (None ≠ ada def baru).
                table.insert(
                    key,
                    Entry {
                        dest: *dest,
                        def_idx: i,
                        operand_defs: [a, b]
                            .iter()
                            .map(|&r| (r, defs.get(&r).copied().unwrap_or(usize::MAX)))
                            .collect(),
                    },
                );
                // Catatan validitas: operand None direkam sebagai usize::MAX,
                // dan cek kesamaan `defs.get(r) == Some(&MAX)` tidak akan cocok
                // bila kemudian didefinisikan → konservatif benar.
                defs.insert(*dest, i);
            }
            MirInstr::Unary {
                op,
                dest,
                operand,
                width,
            } => {
                let key = CseKey::Un(format!("{:?}", op), *operand, *width);
                if let Some(e) = table.get(&key) {
                    let operands_unchanged =
                        e.operand_defs.iter().all(|(r, d)| defs.get(r) == Some(d));
                    let dest_fresh = defs.get(&e.dest) == Some(&e.def_idx);
                    if operands_unchanged && dest_fresh {
                        let dest = *dest;
                        let cached_dest = e.dest;
                        instrs[i] = MirInstr::Nop;
                        let mut j = i + 1;
                        while j < instrs.len() {
                            if matches!(
                                instrs[j],
                                MirInstr::Branch { .. }
                                    | MirInstr::Jump { .. }
                                    | MirInstr::Label(_)
                            ) {
                                break;
                            }
                            if writes_register(&instrs[j], dest) {
                                break;
                            }
                            rewrite_read_register(&mut instrs[j], dest, cached_dest);
                            j += 1;
                        }
                        defs.insert(dest, i);
                        changed = true;
                        continue;
                    }
                }
                let od = vec![(*operand, defs.get(operand).copied().unwrap_or(usize::MAX))];
                table.insert(
                    key,
                    Entry {
                        dest: *dest,
                        def_idx: i,
                        operand_defs: od,
                    },
                );
                defs.insert(*dest, i);
            }
            MirInstr::Const { dest, .. } => {
                defs.insert(*dest, i);
                // Const juga invalidasi entri CSE yang memakai dest sbg
                // operan? Tidak — Const adalah DEFINISI, dan definisi baru
                // mengubah nilai reg → entri dengan operan reg tsb harus
                // divalidasi. defs.update di atas menangani via cek
                // kesamaan posisi. Namun Const tidak masuk table sendiri.
            }
            MirInstr::Load { dest, .. } => {
                defs.insert(*dest, i);
            }
            _ => {}
        }
    }

    if changed {
        remove_nops(instrs);
    }
    changed
}

/// Apakah instruksi MENULIS ke register tertentu?
fn writes_register(instr: &MirInstr, reg: usize) -> bool {
    match instr {
        MirInstr::Const { dest, .. }
        | MirInstr::Binary { dest, .. }
        | MirInstr::Unary { dest, .. } => *dest == reg,
        _ => false,
    }
}

/// Rewrite semua PEMBACAAN `from` menjadi `to` pada satu instruksi.
fn rewrite_read_register(instr: &mut MirInstr, from: usize, to: usize) {
    match instr {
        MirInstr::Binary { lhs, rhs, .. } => {
            if *lhs == from {
                *lhs = to;
            }
            if *rhs == from {
                *rhs = to;
            }
        }
        MirInstr::Unary { operand, .. } => {
            if *operand == from {
                *operand = to;
            }
        }
        MirInstr::Store { src, .. } => {
            if *src == from {
                *src = to;
            }
        }
        MirInstr::NonBlocking { src, .. } => {
            if *src == from {
                *src = to;
            }
        }
        MirInstr::Branch { cond, .. } if *cond == from => {
            *cond = to;
        }
        _ => {}
    }
}

// ── Pass 3c: Loop Invariant Code Motion (COMP-05, conservative) ──

/// Hoist komputasi murni yang invariant keluar dari loop.
///
/// Deteksi loop: Label L ... Jump/Branch yang kembali ke L (backward jump).
/// Sebuah Binary/Unary di body loop adalah INVARIANT bila:
///   1. Semua operand terakhir didefinisikan SEBELUM label loop (di luar),
///   2. Dest TIDAK pernah jadi target Store/NonBlocking dalam body
///      (hasil hanya dipakai komputasi lain / setelah loop).
///
/// CONSERVATIVE: tidak menangani nested loop, tidak menghoist melintasi
/// call/display, dan hanya satu level loop per pass (fixed-point pipeline
/// akan iterasi bila masih ada).
fn licm(instrs: &mut Vec<MirInstr>) -> bool {
    // 1. Cari backward jump → (header_idx, tail_idx).
    let mut loops: Vec<(usize, usize)> = Vec::new();
    for i in 0..instrs.len() {
        match &instrs[i] {
            MirInstr::Jump { label } => {
                if let Some(hdr) = instrs[..i]
                    .iter()
                    .position(|s| matches!(s, MirInstr::Label(l) if l == label))
                {
                    loops.push((hdr, i));
                }
            }
            MirInstr::Branch {
                then_label,
                else_label,
                ..
            } => {
                for lbl in [then_label, else_label] {
                    if let Some(hdr) = instrs[..i]
                        .iter()
                        .position(|s| matches!(s, MirInstr::Label(l) if l == lbl))
                    {
                        if hdr < i {
                            loops.push((hdr, i));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    loops.sort_by_key(|&(h, _)| h);
    loops.dedup();
    if loops.is_empty() {
        return false;
    }

    let mut changed = false;

    // Proses loop dari belakang agar index stabil saat hoist.
    for &(hdr, tail) in loops.iter().rev() {
        // Definisi sebelum header: reg → true.
        let mut outside_defs: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for instr in &instrs[..hdr] {
            match instr {
                MirInstr::Const { dest, .. }
                | MirInstr::Load { dest, .. }
                | MirInstr::Binary { dest, .. }
                | MirInstr::Unary { dest, .. } => {
                    outside_defs.insert(*dest);
                }
                _ => {}
            }
        }

        // Kumpulkan sinyal Store/NonBlocking dalam body (dest yang feed ke
        // sini tidak boleh di-hoist — hasilnya side-effect).
        let mut signal_writes: Vec<usize> = Vec::new();
        for instr in &instrs[hdr..=tail] {
            match instr {
                MirInstr::Store { src, .. } | MirInstr::NonBlocking { src, .. } => {
                    signal_writes.push(*src);
                }
                _ => {}
            }
        }

        // Kandidat hoist: posisi instruksi invariant (urut naik).
        let mut candidates: Vec<usize> = Vec::new();
        #[allow(clippy::needless_range_loop)]
        for i in hdr + 1..tail {
            let (opnds, dest) = match &instrs[i] {
                MirInstr::Binary { lhs, rhs, dest, .. } => (vec![*lhs, *rhs], *dest),
                MirInstr::Unary { operand, dest, .. } => (vec![*operand], *dest),
                _ => continue,
            };
            // Semua operan harus didefinisikan di luar loop.
            if !opnds.iter().all(|r| outside_defs.contains(r)) {
                continue;
            }
            // Dest tidak boleh feed Store/NonBlocking dalam body.
            if signal_writes.contains(&dest) {
                continue;
            }
            candidates.push(i);
        }

        if candidates.is_empty() {
            continue;
        }

        // Hoist: pindahkan instruksi (dari belakang ke depan agar index
        // tetap valid) ke tepat sebelum header Label.
        // Setelah semua move, header Label bergeser; tapi karena kita pakai
        // remove+insert dan proses dari tail→head, index header turun setiap
        // kali. Simpan offset: jumlah hoisted so far.
        let insert_at = hdr; // posisi Label
        for &ci in candidates.iter().rev() {
            let instr = instrs.remove(ci);
            instrs.insert(insert_at, instr);
        }
        changed = true;
    }

    changed
}

// ── Pass 4: Constant Branch Folding ──

/// Replace branches with constant conditions: if cond is always true → Jump,
/// if cond is always false → remove branch.
fn constant_branch_fold(instrs: &mut [MirInstr]) -> bool {
    let mut changed = false;
    let mut i = 0;
    while i < instrs.len() {
        if let MirInstr::Branch {
            cond,
            then_label,
            else_label,
        } = &instrs[i]
        {
            if let Some(val) = find_const_value(instrs, *cond) {
                if val != 0 {
                    instrs[i] = MirInstr::Jump { label: *then_label };
                } else {
                    instrs[i] = MirInstr::Jump { label: *else_label };
                }
                changed = true;
            }
        }
        i += 1;
    }
    changed
}

// ── Pass 5: Strength Reduction ──

/// Replace expensive operations with cheaper ones:
/// - Mul by power-of-2 → Shl
/// - Div by power-of-2 → Shr
/// - Mul by 0 → Const 0
/// - Mul/Div by 1 → NOP (removed by subsequent passes)
fn strength_reduce(instrs: &mut Vec<MirInstr>) -> bool {
    let mut changed = false;
    let mut i = 0;
    while i < instrs.len() {
        // Clone fields before matching to avoid E0502 borrow conflict
        let matched = match &instrs[i] {
            MirInstr::Binary {
                op,
                dest,
                lhs,
                rhs,
                width,
            } => Some((*op, *dest, *lhs, *rhs, *width)),
            _ => None,
        };
        if let Some((op, dest, lhs, rhs, width)) = matched {
            if let Some(rv) = find_const_value(instrs, rhs) {
                match op {
                    MirBinOp::Mul => {
                        if rv == 0 {
                            instrs[i] = MirInstr::Const {
                                dest,
                                value: 0,
                                width,
                            };
                            changed = true;
                        } else if rv == 1 {
                            instrs[i] = MirInstr::Nop;
                            changed = true;
                        } else if rv.is_power_of_two() {
                            let shift = rv.trailing_zeros() as u64;
                            let shift_reg = alloc_temp_reg(instrs);
                            instrs.insert(
                                i,
                                MirInstr::Const {
                                    dest: shift_reg,
                                    value: shift,
                                    width,
                                },
                            );
                            instrs[i + 1] = MirInstr::Binary {
                                op: MirBinOp::Shl,
                                dest,
                                lhs,
                                rhs: shift_reg,
                                width,
                            };
                            changed = true;
                        }
                    }
                    MirBinOp::Div => {
                        if rv == 1 {
                            instrs[i] = MirInstr::Nop;
                            changed = true;
                        } else if rv.is_power_of_two() {
                            let shift = rv.trailing_zeros() as u64;
                            let shift_reg = alloc_temp_reg(instrs);
                            instrs.insert(
                                i,
                                MirInstr::Const {
                                    dest: shift_reg,
                                    value: shift,
                                    width,
                                },
                            );
                            instrs[i + 1] = MirInstr::Binary {
                                op: MirBinOp::Shr,
                                dest,
                                lhs,
                                rhs: shift_reg,
                                width,
                            };
                            changed = true;
                        }
                    }
                    _ => {}
                }
            }
        }
        i += 1;
    }
    changed
}

/// Allocate a temporary register index for strength reduction expansions.
fn alloc_temp_reg(instrs: &[MirInstr]) -> usize {
    let max_reg = instrs
        .iter()
        .filter_map(|i| match i {
            MirInstr::Const { dest, .. }
            | MirInstr::Binary { dest, .. }
            | MirInstr::Unary { dest, .. } => Some(*dest),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    max_reg + 1
}

// ── Pass 6: Remove NOPs ──

fn remove_nops(instrs: &mut Vec<MirInstr>) -> bool {
    let before = instrs.len();
    instrs.retain(|i| !matches!(i, MirInstr::Nop));
    instrs.len() != before
}

// ── Pass 7: Sign Extension Elimination (COMP-08) ──

/// Eliminate redundant shift patterns (COMP-08):
/// - `Shr(x, 0)` / `Shl(x, 0)` → x (identity: shift by 0)
/// - `Shr(x, N)` where N >= width → Const 0 (logical shift right past width)
/// - `Shl(x, N)` where N >= width → Const 0 (logical shift left past width)
///
/// CONSERVATIVE: only operates on constants found via find_const_value.
fn sign_ext_eliminate(instrs: &mut [MirInstr]) -> bool {
    let mut changed = false;
    let mut i = 0;
    while i < instrs.len() {
        let matched = match &instrs[i] {
            MirInstr::Binary {
                op,
                dest,
                lhs,
                rhs,
                width,
            } => Some((*op, *dest, *lhs, *rhs, *width)),
            _ => None,
        };
        if let Some((op, dest, lhs, rhs, width)) = matched {
            if let Some(shift) = find_const_value(instrs, rhs) {
                match op {
                    MirBinOp::Shr | MirBinOp::Shl => {
                        if shift == 0 {
                            // Identity: shift by 0 → just use lhs
                            instrs[i] = MirInstr::Nop;
                            let mut j = i + 1;
                            while j < instrs.len() {
                                if matches!(
                                    instrs[j],
                                    MirInstr::Branch { .. }
                                        | MirInstr::Jump { .. }
                                        | MirInstr::Label(_)
                                ) {
                                    break;
                                }
                                if writes_register(&instrs[j], dest) {
                                    break;
                                }
                                rewrite_read_register(&mut instrs[j], dest, lhs);
                                j += 1;
                            }
                            changed = true;
                            continue;
                        }
                        // Shift >= width → result is always 0
                        if shift >= width as u64 {
                            instrs[i] = MirInstr::Const {
                                dest,
                                value: 0,
                                width,
                            };
                            changed = true;
                        }
                    }
                    _ => {}
                }
            }
        }
        i += 1;
    }
    changed
}

// ── Pass 8: X/Z Constant Propagation (COMP-09) ──

/// Propagate known-zero and identity patterns through operations:
/// - `x & 0` → 0 (AND with zero kills result)
/// - `x | 0` → x (OR with zero is identity)
/// - `x ^ 0` → x (XOR with zero is identity)
/// - `x & all-ones` → x (AND with all-ones is identity)
/// - `x * 0` → 0 (MUL by zero)
/// - `x * 1` → x (MUL by one)
/// - `x + 0` → x (ADD zero)
/// - `x - 0` → x (SUB zero)
/// - `x << 0` / `x >> 0` → x (already handled by sign_ext_eliminate)
///
/// This pass specifically targets patterns left by strength_reduce or
/// constant_fold that operate on power-of-2 masks and bit-field operations
/// common in RTL designs (masking MSB/LSB bits).
fn xz_propagate(instrs: &mut [MirInstr]) -> bool {
    let mut changed = false;
    let mut i = 0;
    while i < instrs.len() {
        let matched = match &instrs[i] {
            MirInstr::Binary {
                op,
                dest,
                lhs,
                rhs,
                width,
            } => Some((*op, *dest, *lhs, *rhs, *width)),
            _ => None,
        };
        if let Some((op, dest, lhs, rhs, width)) = matched {
            let lhs_val = find_const_value(instrs, lhs);
            let rhs_val = find_const_value(instrs, rhs);

            // AND with zero → 0
            if op == MirBinOp::And {
                if Some(0) == lhs_val || Some(0) == rhs_val {
                    instrs[i] = MirInstr::Const {
                        dest,
                        value: 0,
                        width,
                    };
                    changed = true;
                    i += 1;
                    continue;
                }
                // AND with all-ones (mask) → identity on lhs
                if let Some(rv) = rhs_val {
                    let mask = if width < 64 {
                        (1u64 << width) - 1
                    } else {
                        u64::MAX
                    };
                    if rv == mask {
                        instrs[i] = MirInstr::Nop;
                        let mut j = i + 1;
                        while j < instrs.len() {
                            if matches!(
                                instrs[j],
                                MirInstr::Branch { .. }
                                    | MirInstr::Jump { .. }
                                    | MirInstr::Label(_)
                            ) {
                                break;
                            }
                            if writes_register(&instrs[j], dest) {
                                break;
                            }
                            rewrite_read_register(&mut instrs[j], dest, lhs);
                            j += 1;
                        }
                        changed = true;
                        i += 1;
                        continue;
                    }
                }
                if let Some(lv) = lhs_val {
                    let mask = if width < 64 {
                        (1u64 << width) - 1
                    } else {
                        u64::MAX
                    };
                    if lv == mask {
                        instrs[i] = MirInstr::Nop;
                        let mut j = i + 1;
                        while j < instrs.len() {
                            if matches!(
                                instrs[j],
                                MirInstr::Branch { .. }
                                    | MirInstr::Jump { .. }
                                    | MirInstr::Label(_)
                            ) {
                                break;
                            }
                            if writes_register(&instrs[j], dest) {
                                break;
                            }
                            rewrite_read_register(&mut instrs[j], dest, rhs);
                            j += 1;
                        }
                        changed = true;
                        i += 1;
                        continue;
                    }
                }
            }

            // OR with zero → identity
            if op == MirBinOp::Or {
                if Some(0) == lhs_val {
                    instrs[i] = MirInstr::Nop;
                    let mut j = i + 1;
                    while j < instrs.len() {
                        if matches!(
                            instrs[j],
                            MirInstr::Branch { .. } | MirInstr::Jump { .. } | MirInstr::Label(_)
                        ) {
                            break;
                        }
                        if writes_register(&instrs[j], dest) {
                            break;
                        }
                        rewrite_read_register(&mut instrs[j], dest, rhs);
                        j += 1;
                    }
                    changed = true;
                    i += 1;
                    continue;
                }
                if Some(0) == rhs_val {
                    instrs[i] = MirInstr::Nop;
                    let mut j = i + 1;
                    while j < instrs.len() {
                        if matches!(
                            instrs[j],
                            MirInstr::Branch { .. } | MirInstr::Jump { .. } | MirInstr::Label(_)
                        ) {
                            break;
                        }
                        if writes_register(&instrs[j], dest) {
                            break;
                        }
                        rewrite_read_register(&mut instrs[j], dest, lhs);
                        j += 1;
                    }
                    changed = true;
                    i += 1;
                    continue;
                }
            }

            // XOR with zero → identity
            if op == MirBinOp::Xor {
                if Some(0) == lhs_val {
                    instrs[i] = MirInstr::Nop;
                    let mut j = i + 1;
                    while j < instrs.len() {
                        if matches!(
                            instrs[j],
                            MirInstr::Branch { .. } | MirInstr::Jump { .. } | MirInstr::Label(_)
                        ) {
                            break;
                        }
                        if writes_register(&instrs[j], dest) {
                            break;
                        }
                        rewrite_read_register(&mut instrs[j], dest, rhs);
                        j += 1;
                    }
                    changed = true;
                    i += 1;
                    continue;
                }
                if Some(0) == rhs_val {
                    instrs[i] = MirInstr::Nop;
                    let mut j = i + 1;
                    while j < instrs.len() {
                        if matches!(
                            instrs[j],
                            MirInstr::Branch { .. } | MirInstr::Jump { .. } | MirInstr::Label(_)
                        ) {
                            break;
                        }
                        if writes_register(&instrs[j], dest) {
                            break;
                        }
                        rewrite_read_register(&mut instrs[j], dest, lhs);
                        j += 1;
                    }
                    changed = true;
                    i += 1;
                    continue;
                }
            }

            // MUL by zero → 0; MUL by one → identity
            if op == MirBinOp::Mul {
                if Some(0) == lhs_val || Some(0) == rhs_val {
                    instrs[i] = MirInstr::Const {
                        dest,
                        value: 0,
                        width,
                    };
                    changed = true;
                    i += 1;
                    continue;
                }
                if Some(1) == lhs_val {
                    instrs[i] = MirInstr::Nop;
                    let mut j = i + 1;
                    while j < instrs.len() {
                        if matches!(
                            instrs[j],
                            MirInstr::Branch { .. } | MirInstr::Jump { .. } | MirInstr::Label(_)
                        ) {
                            break;
                        }
                        if writes_register(&instrs[j], dest) {
                            break;
                        }
                        rewrite_read_register(&mut instrs[j], dest, rhs);
                        j += 1;
                    }
                    changed = true;
                    i += 1;
                    continue;
                }
                if Some(1) == rhs_val {
                    instrs[i] = MirInstr::Nop;
                    let mut j = i + 1;
                    while j < instrs.len() {
                        if matches!(
                            instrs[j],
                            MirInstr::Branch { .. } | MirInstr::Jump { .. } | MirInstr::Label(_)
                        ) {
                            break;
                        }
                        if writes_register(&instrs[j], dest) {
                            break;
                        }
                        rewrite_read_register(&mut instrs[j], dest, lhs);
                        j += 1;
                    }
                    changed = true;
                    i += 1;
                    continue;
                }
            }

            // ADD with zero → identity; SUB with zero → identity
            if op == MirBinOp::Add {
                if Some(0) == lhs_val {
                    instrs[i] = MirInstr::Nop;
                    let mut j = i + 1;
                    while j < instrs.len() {
                        if matches!(
                            instrs[j],
                            MirInstr::Branch { .. } | MirInstr::Jump { .. } | MirInstr::Label(_)
                        ) {
                            break;
                        }
                        if writes_register(&instrs[j], dest) {
                            break;
                        }
                        rewrite_read_register(&mut instrs[j], dest, rhs);
                        j += 1;
                    }
                    changed = true;
                    i += 1;
                    continue;
                }
                if Some(0) == rhs_val {
                    instrs[i] = MirInstr::Nop;
                    let mut j = i + 1;
                    while j < instrs.len() {
                        if matches!(
                            instrs[j],
                            MirInstr::Branch { .. } | MirInstr::Jump { .. } | MirInstr::Label(_)
                        ) {
                            break;
                        }
                        if writes_register(&instrs[j], dest) {
                            break;
                        }
                        rewrite_read_register(&mut instrs[j], dest, lhs);
                        j += 1;
                    }
                    changed = true;
                    i += 1;
                    continue;
                }
            }
            if op == MirBinOp::Sub && Some(0) == rhs_val {
                instrs[i] = MirInstr::Nop;
                let mut j = i + 1;
                while j < instrs.len() {
                    if matches!(
                        instrs[j],
                        MirInstr::Branch { .. } | MirInstr::Jump { .. } | MirInstr::Label(_)
                    ) {
                        break;
                    }
                    if writes_register(&instrs[j], dest) {
                        break;
                    }
                    rewrite_read_register(&mut instrs[j], dest, lhs);
                    j += 1;
                }
                changed = true;
                i += 1;
                continue;
            }
        }
        i += 1;
    }
    changed
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::mir::*;
    use mivon_core::intern::Symbol;

    #[test]
    fn test_cse_duplicate_binary() {
        // r3 = r2 + r1 ; r4 = r2 + r1 → duplikat; Store memakai r4
        // di-rewrite menjadi r3, Binary kedua jadi Nop.
        let mut instrs = vec![
            MirInstr::Const {
                dest: 0,
                value: 5,
                width: 32,
            },
            MirInstr::Load { dest: 1, signal: 0 },
            MirInstr::Binary {
                op: MirBinOp::Add,
                dest: 2,
                lhs: 1,
                rhs: 0,
                width: 32,
            },
            MirInstr::Binary {
                op: MirBinOp::Add,
                dest: 3,
                lhs: 1,
                rhs: 0,
                width: 32,
            },
            MirInstr::Store { signal: 1, src: 3 },
        ];
        assert!(common_subexpr_eliminate(&mut instrs));
        // Tepat SATU Add tersisa (yang pertama); duplikat jadi Nop+hapus.
        assert_eq!(
            instrs
                .iter()
                .filter(|i| matches!(
                    i,
                    MirInstr::Binary {
                        op: MirBinOp::Add,
                        ..
                    }
                ))
                .count(),
            1,
            "hanya satu Add tersisa: {:?}",
            instrs
        );
        // Store sekarang membaca reg 2 (hasil CSE pertama).
        assert!(instrs
            .iter()
            .any(|i| matches!(i, MirInstr::Store { signal: 1, src: 2 })));
    }

    #[test]
    fn test_cse_commutative_operands() {
        // r2 = r0 + r1 vs r3 = r1 + r0 → komutatif, tetap ter-CSE.
        let mut instrs = vec![
            MirInstr::Load { dest: 0, signal: 0 },
            MirInstr::Load { dest: 1, signal: 1 },
            MirInstr::Binary {
                op: MirBinOp::Add,
                dest: 2,
                lhs: 0,
                rhs: 1,
                width: 32,
            },
            MirInstr::Binary {
                op: MirBinOp::Add,
                dest: 3,
                lhs: 1,
                rhs: 0,
                width: 32,
            },
        ];
        assert!(common_subexpr_eliminate(&mut instrs));
        assert_eq!(
            instrs
                .iter()
                .filter(|i| matches!(
                    i,
                    MirInstr::Binary {
                        op: MirBinOp::Add,
                        ..
                    }
                ))
                .count(),
            1,
            "Add komutatif ter-CSE"
        );
    }

    #[test]
    fn test_cse_not_cross_block_and_not_noncommutative() {
        // Sub non-komutatif dengan operan tertukar TIDAK boleh merge.
        let mut instrs = vec![
            MirInstr::Load { dest: 0, signal: 0 },
            MirInstr::Load { dest: 1, signal: 1 },
            MirInstr::Binary {
                op: MirBinOp::Sub,
                dest: 2,
                lhs: 0,
                rhs: 1,
                width: 32,
            },
            MirInstr::Binary {
                op: MirBinOp::Sub,
                dest: 3,
                lhs: 1,
                rhs: 0,
                width: 32,
            },
        ];
        assert!(
            !common_subexpr_eliminate(&mut instrs),
            "Sub tertukar bukan CSE"
        );

        // Reset di Label: duplikat melintasi label tidak digabung.
        let mut cross = vec![
            MirInstr::Label(0),
            MirInstr::Binary {
                op: MirBinOp::Add,
                dest: 2,
                lhs: 0,
                rhs: 1,
                width: 32,
            },
            MirInstr::Label(1),
            MirInstr::Binary {
                op: MirBinOp::Add,
                dest: 3,
                lhs: 0,
                rhs: 1,
                width: 32,
            },
        ];
        assert!(
            !common_subexpr_eliminate(&mut cross),
            "CSE tidak melintasi Label"
        );
    }

    #[test]
    fn test_constant_fold_add() {
        let mut instrs = vec![
            MirInstr::Const {
                dest: 0,
                value: 3,
                width: 32,
            },
            MirInstr::Const {
                dest: 1,
                value: 4,
                width: 32,
            },
            MirInstr::Binary {
                op: MirBinOp::Add,
                dest: 2,
                lhs: 0,
                rhs: 1,
                width: 32,
            },
        ];
        assert!(constant_fold(&mut instrs));
        assert!(matches!(&instrs[2], MirInstr::Const { value: 7, .. }));
    }

    #[test]
    fn test_remove_nops() {
        let mut instrs = vec![
            MirInstr::Nop,
            MirInstr::Const {
                dest: 0,
                value: 1,
                width: 1,
            },
            MirInstr::Nop,
        ];
        assert!(remove_nops(&mut instrs));
        assert_eq!(instrs.len(), 1);
    }

    #[test]
    fn test_constant_fold_all_ops() {
        let ops: [(MirBinOp, u64, u64, u64); 17] = [
            (MirBinOp::Add, 10, 20, 30),
            (MirBinOp::Sub, 50, 23, 27),
            (MirBinOp::Mul, 6, 7, 42),
            (MirBinOp::Div, 42, 6, 7),
            (MirBinOp::And, 0xFF, 0x0F, 0x0F),
            (MirBinOp::Or, 0xF0, 0x0F, 0xFF),
            (MirBinOp::Xor, 0xFF, 0x0F, 0xF0),
            (MirBinOp::Eq, 5, 5, 1),
            (MirBinOp::Eq, 5, 6, 0),
            (MirBinOp::Ne, 5, 6, 1),
            (MirBinOp::Ne, 5, 5, 0),
            (MirBinOp::Lt, 3, 7, 1),
            (MirBinOp::Lt, 7, 3, 0),
            (MirBinOp::Le, 5, 5, 1),
            (MirBinOp::Gt, 7, 3, 1),
            (MirBinOp::Ge, 5, 5, 1),
            (MirBinOp::Shl, 1, 3, 8),
        ];
        for (i, &(op, a, b, expected)) in ops.iter().enumerate() {
            let mut instrs = vec![
                MirInstr::Const {
                    dest: 0,
                    value: a,
                    width: 32,
                },
                MirInstr::Const {
                    dest: 1,
                    value: b,
                    width: 32,
                },
                MirInstr::Binary {
                    op,
                    dest: 2,
                    lhs: 0,
                    rhs: 1,
                    width: 32,
                },
            ];
            assert!(constant_fold(&mut instrs), "op index {} should fold", i);
            if let MirInstr::Const { value, .. } = &instrs[2] {
                assert_eq!(
                    *value, expected,
                    "op index {}: expected {}, got {}",
                    i, expected, value
                );
            } else {
                panic!("op index {}: not folded", i);
            }
        }
    }

    #[test]
    fn test_dead_store_eliminate_adjacent() {
        // reg 0 written twice — first write dead (overwritten before any read)
        let mut instrs = vec![
            MirInstr::Const {
                dest: 0,
                value: 42,
                width: 32,
            }, // DEAD — overwritten by next
            MirInstr::Const {
                dest: 0,
                value: 7,
                width: 32,
            }, // LIVE — read by Add
            MirInstr::Binary {
                op: MirBinOp::Add,
                dest: 1,
                lhs: 0,
                rhs: 0,
                width: 32,
            },
            MirInstr::Store { signal: 0, src: 1 }, // LIVE — external
        ];
        let changed = dead_store_eliminate(&mut instrs);
        assert!(changed, "dead store should be eliminated");
        assert!(
            instrs
                .iter()
                .any(|i| matches!(i, MirInstr::Const { value: 7, .. })),
            "value 7 should survive"
        );
        assert!(
            instrs.iter().any(|i| matches!(i, MirInstr::Store { .. })),
            "store should survive"
        );
    }

    #[test]
    fn test_dead_store_same_register() {
        // reg 0 written twice — first write dead
        let mut instrs = vec![
            MirInstr::Const {
                dest: 0,
                value: 1,
                width: 8,
            },
            MirInstr::Const {
                dest: 0,
                value: 2,
                width: 8,
            },
            MirInstr::Store { signal: 0, src: 0 },
        ];
        let changed = dead_store_eliminate(&mut instrs);
        assert!(changed);
    }

    #[test]
    fn test_constant_branch_fold_true() {
        let mut instrs = vec![
            MirInstr::Const {
                dest: 0,
                value: 1,
                width: 1,
            },
            MirInstr::Branch {
                cond: 0,
                then_label: 10,
                else_label: 20,
            },
        ];
        assert!(constant_branch_fold(&mut instrs));
        assert!(matches!(instrs[1], MirInstr::Jump { label: 10 }));
    }

    #[test]
    fn test_constant_branch_fold_false() {
        let mut instrs = vec![
            MirInstr::Const {
                dest: 0,
                value: 0,
                width: 1,
            },
            MirInstr::Branch {
                cond: 0,
                then_label: 10,
                else_label: 20,
            },
        ];
        assert!(constant_branch_fold(&mut instrs));
        assert!(matches!(instrs[1], MirInstr::Jump { label: 20 }));
    }

    #[test]
    fn test_strength_reduce_mul_pow2() {
        let mut instrs = vec![
            MirInstr::Const {
                dest: 0,
                value: 5,
                width: 32,
            },
            MirInstr::Const {
                dest: 1,
                value: 8,
                width: 32,
            },
            MirInstr::Binary {
                op: MirBinOp::Mul,
                dest: 2,
                lhs: 0,
                rhs: 1,
                width: 32,
            },
        ];
        assert!(strength_reduce(&mut instrs));
        assert!(instrs.iter().any(|i| matches!(
            i,
            MirInstr::Binary {
                op: MirBinOp::Shl,
                ..
            }
        )));
    }

    #[test]
    fn test_strength_reduce_mul_zero() {
        let mut instrs = vec![
            MirInstr::Const {
                dest: 0,
                value: 42,
                width: 32,
            },
            MirInstr::Const {
                dest: 1,
                value: 0,
                width: 32,
            },
            MirInstr::Binary {
                op: MirBinOp::Mul,
                dest: 2,
                lhs: 0,
                rhs: 1,
                width: 32,
            },
        ];
        assert!(strength_reduce(&mut instrs));
        assert!(matches!(&instrs[2], MirInstr::Const { value: 0, .. }));
    }

    #[test]
    fn test_strength_reduce_div_pow2() {
        let mut instrs = vec![
            MirInstr::Const {
                dest: 0,
                value: 100,
                width: 32,
            },
            MirInstr::Const {
                dest: 1,
                value: 4,
                width: 32,
            },
            MirInstr::Binary {
                op: MirBinOp::Div,
                dest: 2,
                lhs: 0,
                rhs: 1,
                width: 32,
            },
        ];
        assert!(strength_reduce(&mut instrs));
        assert!(instrs.iter().any(|i| matches!(
            i,
            MirInstr::Binary {
                op: MirBinOp::Shr,
                ..
            }
        )));
    }

    #[test]
    fn test_copy_propagate_unary_const() {
        let mut instrs = vec![
            MirInstr::Const {
                dest: 0,
                value: 0xFF,
                width: 8,
            },
            MirInstr::Unary {
                op: super::super::mir::MirUnOp::Not,
                dest: 1,
                operand: 0,
                width: 8,
            },
        ];
        assert!(copy_propagate(&mut instrs));
        assert!(matches!(&instrs[1], MirInstr::Const { value: 0, .. }));
    }

    #[test]
    fn test_optimize_process_full_pipeline() {
        let mut instrs = vec![
            MirInstr::Const {
                dest: 0,
                value: 5,
                width: 32,
            },
            MirInstr::Const {
                dest: 1,
                value: 8,
                width: 32,
            },
            MirInstr::Binary {
                op: MirBinOp::Mul,
                dest: 2,
                lhs: 0,
                rhs: 1,
                width: 32,
            },
            MirInstr::Const {
                dest: 3,
                value: 0,
                width: 1,
            },
            MirInstr::Branch {
                cond: 3,
                then_label: 10,
                else_label: 20,
            },
            MirInstr::Nop,
        ];
        optimize_process(&mut instrs);
        assert!(instrs
            .iter()
            .any(|i| matches!(i, MirInstr::Const { value: 5, .. })));
        assert!(instrs
            .iter()
            .any(|i| matches!(i, MirInstr::Const { value: 40, .. })));
        assert!(instrs.iter().any(|i| matches!(i, MirInstr::Jump { .. })));
        assert!(!instrs.iter().any(|i| matches!(i, MirInstr::Nop)));
    }

    #[test]
    fn test_dead_store_conservative_across_branch() {
        // reg 0 written before branch, used in one path — should NOT be eliminated
        let mut instrs = vec![
            MirInstr::Const {
                dest: 0,
                value: 5,
                width: 8,
            },
            MirInstr::Branch {
                cond: 0,
                then_label: 10,
                else_label: 20,
            },
            MirInstr::Label(10),
            MirInstr::Store { signal: 0, src: 0 },
            MirInstr::Jump { label: 30 },
            MirInstr::Label(20),
            MirInstr::Const {
                dest: 0,
                value: 10,
                width: 8,
            },
            MirInstr::Label(30),
        ];
        // dead_store_eliminate is conservative — shouldn't eliminate const[0] before Branch
        let changed = dead_store_eliminate(&mut instrs);
        // The const at index 0 is across a branch boundary from the next write at index 6
        // So it should NOT be eliminated
        assert!(!changed, "should not eliminate writes across control flow");
    }

    #[test]
    fn test_licm_hoists_invariant_computation() {
        // Loop: Label(0) ... Binary dest=3, lhs=1(sig load), rhs=2(const)
        // Binary invariant (kedua operan didefinisikan di luar loop)
        // → hoist ke sebelum Label(0).
        let mut instrs = vec![
            MirInstr::Const {
                dest: 2,
                value: 10,
                width: 32,
            },
            MirInstr::Load { dest: 1, signal: 0 },
            MirInstr::Label(0),
            MirInstr::Binary {
                op: MirBinOp::Add,
                dest: 3,
                lhs: 1,
                rhs: 2,
                width: 32,
            },
            MirInstr::Load { dest: 4, signal: 1 },
            MirInstr::Store { signal: 1, src: 4 },
            MirInstr::Branch {
                cond: 4,
                then_label: 0,
                else_label: 1,
            },
            MirInstr::Label(1),
        ];
        assert!(licm(&mut instrs));
        let add_pos = instrs
            .iter()
            .position(|i| matches!(i, MirInstr::Binary { .. }))
            .unwrap();
        let label_pos = instrs
            .iter()
            .position(|i| matches!(i, MirInstr::Label(0)))
            .unwrap();
        assert!(
            add_pos < label_pos,
            "Add harus di-hoist keluar loop: {:?}",
            instrs
        );
    }

    #[test]
    fn test_licm_does_not_hoist_variant() {
        // Unary yang operannya didefinisikan DI DALAM loop → tidak boleh
        // di-hoist.
        let mut instrs = vec![
            MirInstr::Label(0),
            MirInstr::Load { dest: 0, signal: 0 },
            MirInstr::Unary {
                op: MirUnOp::Not,
                dest: 1,
                operand: 0,
                width: 32,
            },
            MirInstr::Jump { label: 0 },
        ];
        assert!(!licm(&mut instrs), "variant tidak boleh hoist");
        let unary_pos = instrs
            .iter()
            .position(|i| matches!(i, MirInstr::Unary { .. }))
            .unwrap();
        let label_pos = instrs
            .iter()
            .position(|i| matches!(i, MirInstr::Label(0)))
            .unwrap();
        assert!(unary_pos > label_pos);
    }

    // ── Tests: sign_ext_eliminate (COMP-08) ──

    #[test]
    fn test_sign_ext_eliminate_shr_zero() {
        // Shr(x, 0) → identity: Store memakai x (reg 0), bukan hasil shift.
        let mut instrs = vec![
            MirInstr::Load { dest: 0, signal: 0 },
            MirInstr::Const {
                dest: 1,
                value: 0,
                width: 8,
            },
            MirInstr::Binary {
                op: MirBinOp::Shr,
                dest: 2,
                lhs: 0,
                rhs: 1,
                width: 8,
            },
            MirInstr::Store { signal: 1, src: 2 },
        ];
        assert!(
            sign_ext_eliminate(&mut instrs),
            "Shr(x,0) harus di-eliminasi"
        );
        // Store sekarang membaca reg 0 (lhs asli)
        assert!(
            instrs
                .iter()
                .any(|i| matches!(i, MirInstr::Store { signal: 1, src: 0 })),
            "Store harus membaca lhs: {:?}",
            instrs
        );
    }

    #[test]
    fn test_sign_ext_eliminate_shl_zero() {
        // Shl(x, 0) → identity
        let mut instrs = vec![
            MirInstr::Load { dest: 0, signal: 0 },
            MirInstr::Const {
                dest: 1,
                value: 0,
                width: 8,
            },
            MirInstr::Binary {
                op: MirBinOp::Shl,
                dest: 2,
                lhs: 0,
                rhs: 1,
                width: 8,
            },
            MirInstr::Store { signal: 1, src: 2 },
        ];
        assert!(
            sign_ext_eliminate(&mut instrs),
            "Shl(x,0) harus di-eliminasi"
        );
        assert!(
            instrs
                .iter()
                .any(|i| matches!(i, MirInstr::Store { signal: 1, src: 0 })),
            "Store harus membaca lhs: {:?}",
            instrs
        );
    }

    #[test]
    fn test_sign_ext_eliminate_shl_past_width() {
        // Shl(x, 32) pada width 8 → Const 0 (shift melebihi lebar)
        let mut instrs = vec![
            MirInstr::Load { dest: 0, signal: 0 },
            MirInstr::Const {
                dest: 1,
                value: 32,
                width: 8,
            },
            MirInstr::Binary {
                op: MirBinOp::Shl,
                dest: 2,
                lhs: 0,
                rhs: 1,
                width: 8,
            },
            MirInstr::Store { signal: 1, src: 2 },
        ];
        assert!(sign_ext_eliminate(&mut instrs));
        assert!(
            instrs
                .iter()
                .any(|i| matches!(i, MirInstr::Const { value: 0, .. })),
            "Harus jadi Const 0: {:?}",
            instrs
        );
    }

    #[test]
    fn test_sign_ext_eliminate_shr_past_width() {
        // Shr(x, 32) pada width 8 → Const 0
        let mut instrs = vec![
            MirInstr::Load { dest: 0, signal: 0 },
            MirInstr::Const {
                dest: 1,
                value: 32,
                width: 8,
            },
            MirInstr::Binary {
                op: MirBinOp::Shr,
                dest: 2,
                lhs: 0,
                rhs: 1,
                width: 8,
            },
            MirInstr::Store { signal: 1, src: 2 },
        ];
        assert!(sign_ext_eliminate(&mut instrs));
        assert!(
            instrs
                .iter()
                .any(|i| matches!(i, MirInstr::Const { value: 0, .. })),
            "Harus jadi Const 0: {:?}",
            instrs
        );
    }

    #[test]
    fn test_sign_ext_eliminate_non_constant_no_fold() {
        // Shift by non-constant → no fold
        let mut instrs = vec![
            MirInstr::Load { dest: 0, signal: 0 },
            MirInstr::Load { dest: 1, signal: 1 },
            MirInstr::Binary {
                op: MirBinOp::Shr,
                dest: 2,
                lhs: 0,
                rhs: 1,
                width: 8,
            },
            MirInstr::Store { signal: 2, src: 2 },
        ];
        assert!(
            !sign_ext_eliminate(&mut instrs),
            "non-constant shift tidak boleh fold"
        );
    }

    // ── Tests: xz_propagate (COMP-09) ──

    #[test]
    fn test_xz_propagate_and_zero() {
        // x & 0 → 0
        let mut instrs = vec![
            MirInstr::Load { dest: 0, signal: 0 },
            MirInstr::Const {
                dest: 1,
                value: 0,
                width: 8,
            },
            MirInstr::Binary {
                op: MirBinOp::And,
                dest: 2,
                lhs: 0,
                rhs: 1,
                width: 8,
            },
            MirInstr::Store { signal: 1, src: 2 },
        ];
        assert!(xz_propagate(&mut instrs), "x & 0 harus fold ke 0");
        assert!(
            instrs.iter().any(|i| matches!(
                i,
                MirInstr::Const {
                    value: 0,
                    dest: 2,
                    ..
                }
            )),
            "dest 2 harus jadi Const 0: {:?}",
            instrs
        );
    }

    #[test]
    fn test_xz_propagate_and_all_ones() {
        // x & 0xFF (width=8, mask = all-ones) → x (identity)
        let mut instrs = vec![
            MirInstr::Load { dest: 0, signal: 0 },
            MirInstr::Const {
                dest: 1,
                value: 0xFF,
                width: 8,
            },
            MirInstr::Binary {
                op: MirBinOp::And,
                dest: 2,
                lhs: 0,
                rhs: 1,
                width: 8,
            },
            MirInstr::Store { signal: 1, src: 2 },
        ];
        assert!(xz_propagate(&mut instrs), "x & all-ones harus identity");
        assert!(
            instrs
                .iter()
                .any(|i| matches!(i, MirInstr::Store { signal: 1, src: 0 })),
            "Store harus membaca lhs asli: {:?}",
            instrs
        );
    }

    #[test]
    fn test_xz_propagate_or_zero() {
        // x | 0 → x
        let mut instrs = vec![
            MirInstr::Load { dest: 0, signal: 0 },
            MirInstr::Const {
                dest: 1,
                value: 0,
                width: 8,
            },
            MirInstr::Binary {
                op: MirBinOp::Or,
                dest: 2,
                lhs: 0,
                rhs: 1,
                width: 8,
            },
            MirInstr::Store { signal: 1, src: 2 },
        ];
        assert!(xz_propagate(&mut instrs), "x | 0 harus identity");
        assert!(
            instrs
                .iter()
                .any(|i| matches!(i, MirInstr::Store { signal: 1, src: 0 })),
            "Store harus membaca lhs: {:?}",
            instrs
        );
    }

    #[test]
    fn test_xz_propagate_mul_zero() {
        // x * 0 → 0
        let mut instrs = vec![
            MirInstr::Load { dest: 0, signal: 0 },
            MirInstr::Const {
                dest: 1,
                value: 0,
                width: 8,
            },
            MirInstr::Binary {
                op: MirBinOp::Mul,
                dest: 2,
                lhs: 0,
                rhs: 1,
                width: 8,
            },
            MirInstr::Store { signal: 1, src: 2 },
        ];
        assert!(xz_propagate(&mut instrs), "x * 0 harus fold ke 0");
        assert!(
            instrs.iter().any(|i| matches!(
                i,
                MirInstr::Const {
                    value: 0,
                    dest: 2,
                    ..
                }
            )),
            "dest 2 harus jadi Const 0: {:?}",
            instrs
        );
    }

    #[test]
    fn test_xz_propagate_mul_one() {
        // x * 1 → x
        let mut instrs = vec![
            MirInstr::Load { dest: 0, signal: 0 },
            MirInstr::Const {
                dest: 1,
                value: 1,
                width: 8,
            },
            MirInstr::Binary {
                op: MirBinOp::Mul,
                dest: 2,
                lhs: 0,
                rhs: 1,
                width: 8,
            },
            MirInstr::Store { signal: 1, src: 2 },
        ];
        assert!(xz_propagate(&mut instrs), "x * 1 harus identity");
        assert!(
            instrs
                .iter()
                .any(|i| matches!(i, MirInstr::Store { signal: 1, src: 0 })),
            "Store harus membaca lhs: {:?}",
            instrs
        );
    }

    #[test]
    fn test_xz_propagate_add_zero() {
        // 0 + x → x
        let mut instrs = vec![
            MirInstr::Const {
                dest: 0,
                value: 0,
                width: 8,
            },
            MirInstr::Load { dest: 1, signal: 0 },
            MirInstr::Binary {
                op: MirBinOp::Add,
                dest: 2,
                lhs: 0,
                rhs: 1,
                width: 8,
            },
            MirInstr::Store { signal: 1, src: 2 },
        ];
        assert!(xz_propagate(&mut instrs), "0 + x harus identity");
        assert!(
            instrs
                .iter()
                .any(|i| matches!(i, MirInstr::Store { signal: 1, src: 1 })),
            "Store harus membaca rhs: {:?}",
            instrs
        );
    }

    #[test]
    fn test_xz_propagate_sub_zero() {
        // x - 0 → x
        let mut instrs = vec![
            MirInstr::Load { dest: 0, signal: 0 },
            MirInstr::Const {
                dest: 1,
                value: 0,
                width: 8,
            },
            MirInstr::Binary {
                op: MirBinOp::Sub,
                dest: 2,
                lhs: 0,
                rhs: 1,
                width: 8,
            },
            MirInstr::Store { signal: 1, src: 2 },
        ];
        assert!(xz_propagate(&mut instrs), "x - 0 harus identity");
        assert!(
            instrs
                .iter()
                .any(|i| matches!(i, MirInstr::Store { signal: 1, src: 0 })),
            "Store harus membaca lhs: {:?}",
            instrs
        );
    }

    // ── Tests: eliminate_unused_signals (COMP-02 lanjutan) ──

    #[test]
    fn test_eliminate_unused_signals_removes_unused() {
        // Sinyal 0 (digunakan Load), 1 (tidak digunakan), 2 (digunakan Store).
        let mut module = MirModule::new(Symbol::intern("test"));
        let s_a = module.add_signal(MirSignal {
            name: Symbol::intern("a"),
            width: 8,
            kind: MirSignalKind::Wire,
            initial_value: None,
        });
        let s_b = module.add_signal(MirSignal {
            name: Symbol::intern("b"),
            width: 8,
            kind: MirSignalKind::Wire,
            initial_value: None,
        });
        let s_c = module.add_signal(MirSignal {
            name: Symbol::intern("c"),
            width: 8,
            kind: MirSignalKind::Wire,
            initial_value: None,
        });
        assert_eq!(s_a, 0);
        assert_eq!(s_b, 1);
        assert_eq!(s_c, 2);

        let mut instrs = vec![
            MirInstr::Load { dest: 10, signal: s_a }, // pakai sinyal 0
            MirInstr::Store { signal: s_c, src: 10 },  // pakai sinyal 2
        ];
        module.processes.push(MirProcess {
            name: Symbol::intern("p"),
            sensitivity: MirSensitivity::AlwaysComb,
            instrs,
        });

        assert!(eliminate_unused_signals(&mut module));

        // Sinyal "b" (indeks lama 1) harus hilang, sisa 2 sinyal.
        assert_eq!(module.signals.len(), 2, "sinyal tidak terpakai harus dihapus: {:?}", module.signals);
        assert_eq!(module.signals[0].name, Symbol::intern("a"));
        assert_eq!(module.signals[1].name, Symbol::intern("c"));

        // Referensi instruksi harus diperbarui: signal 0 tetap 0, signal 2 -> 1.
        let instr = &module.processes[0].instrs;
        assert!(
            matches!(instr[0], MirInstr::Load { signal: 0, .. }),
            "Load signal 0 harus tetap 0: {:?}",
            instr[0]
        );
        assert!(
            matches!(instr[1], MirInstr::Store { signal: 1, .. }),
            "Store signal 2 harus jadi 1: {:?}",
            instr[1]
        );

        // signal_map harus konsisten.
        assert_eq!(module.signal_map[&Symbol::intern("a")], 0);
        assert_eq!(module.signal_map[&Symbol::intern("c")], 1);
        assert!(!module.signal_map.contains_key(&Symbol::intern("b")));
    }

    #[test]
    fn test_eliminate_unused_signals_all_used_no_change() {
        let mut module = MirModule::new(Symbol::intern("test"));
        let s_a = module.add_signal(MirSignal {
            name: Symbol::intern("a"),
            width: 8,
            kind: MirSignalKind::Wire,
            initial_value: None,
        });
        module.processes.push(MirProcess {
            name: Symbol::intern("p"),
            sensitivity: MirSensitivity::AlwaysComb,
            instrs: vec![MirInstr::Load { dest: 0, signal: s_a }],
        });

        assert!(!eliminate_unused_signals(&mut module));
        assert_eq!(module.signals.len(), 1);
    }

    #[test]
    fn test_eliminate_unused_signals_all_unused_removed() {
        let mut module = MirModule::new(Symbol::intern("test"));
        module.add_signal(MirSignal {
            name: Symbol::intern("a"),
            width: 8,
            kind: MirSignalKind::Wire,
            initial_value: None,
        });
        module.processes.push(MirProcess {
            name: Symbol::intern("p"),
            sensitivity: MirSensitivity::AlwaysComb,
            instrs: vec![MirInstr::Const {
                dest: 0,
                value: 5,
                width: 8,
            }],
        });

        assert!(eliminate_unused_signals(&mut module));
        assert_eq!(module.signals.len(), 0, "semua sinyal tidak terpakai harus dihapus");
        assert!(module.signal_map.is_empty());
    }
}
