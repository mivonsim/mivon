//! Packed 4-state logic representation — SIMD-ready bit-packing.
//!
//! # Encoding
//!
//! Each 4-state cell (X, Z, 0, 1) dikodekan dalam **2 bit** menggunakan
//! dua bitmask `u64`: `known` dan `value`.
//!
//! | State | known | value | Keterangan               |
//! |-------|-------|-------|--------------------------|
//! |   X   |   0   |   0   | Unknown                  |
//! |   Z   |   0   |   1   | High-impedance           |
//! |   0   |   1   |   0   | Logic 0                  |
//! |   1   |   1   |   1   | Logic 1                  |
//!
//! # SIMD Benefit
//!
//! Operasi bitwise (AND, OR, XOR, NOT) menggunakan **2-3 instruksi CPU**
//! per chunk, tanpa branching, tanpa loop per-bit. Untuk sinyal ≤64 bit,
//! semua operasi adalah u64 bitwise langsung — 100% CPU pipeline friendly.
//!
//! Catatan: AND dan OR membutuhkan logika 4-state khusus (0/1 dominance),
//! sehingga sedikit lebih kompleks dari XOR/NOT yang bisa bitmask langsung.

use mivon_ir::{BinaryIrOp, LogicVal, LogicVec};

/// Number of cells per chunk (u64 = 64 bits).
const CELLS_PER_CHUNK: usize = 64;

/// Packed 4-state logic vector.
///
/// Setiap chunk adalah `(known_mask: u64, value_mask: u64)`.
/// - `known_mask`: bit ke-i = 1 jika nilai diketahui (0/1), 0 jika X/Z.
/// - `value_mask`: bit ke-i = nilai aktual (hanya valid jika known=1).
///
/// Untuk sinyal >64 bit, chunks berisi Vec<(u64, u64)>.
/// Untuk sinyal ≤64 bit, hanya chunks[0] yang digunakan.
///
/// # 4-State Truth Tables
///
/// AND: 0 mendominasi (a&0=0 regardless of b). X/Z with anything non-zero → X.
/// OR:  1 mendominasi (a|1=1 regardless of b). X/Z with anything non-zero → X.
/// XOR: X/Z jika salah satu X/Z. Otherwise normal XOR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedLogicVec {
    /// Chunks of (known, value) — each chunk covers 64 cells.
    chunks: Vec<(u64, u64)>,
    /// Total number of cells (bits).
    width: usize,
}

impl PackedLogicVec {
    // ─── Construction ───

    /// Create a new packed vector initialized to X (unknown).
    pub fn new(width: usize) -> Self {
        let w = width.max(1);
        let num_chunks = w.div_ceil(CELLS_PER_CHUNK);
        // X = known:0, value:0
        let chunks = vec![(0u64, 0u64); num_chunks];
        PackedLogicVec { chunks, width: w }
    }

    /// Create from a single bit value with given width.
    pub fn fill(val: LogicVal, width: usize) -> Self {
        let w = width.max(1);
        let num_chunks = w.div_ceil(CELLS_PER_CHUNK);
        let mut chunks = Vec::with_capacity(num_chunks);
        for c in 0..num_chunks {
            let chunk_width = (w - c * CELLS_PER_CHUNK).min(CELLS_PER_CHUNK);
            let mask = if chunk_width >= 64 {
                !0u64
            } else {
                (1u64 << chunk_width) - 1
            };
            let (known, value) = match val {
                LogicVal::X => (0u64, 0u64),
                LogicVal::Z => (0u64, mask),    // all bits = Z
                LogicVal::Zero => (mask, 0u64), // all bits = 0 (known)
                LogicVal::One => (mask, mask),  // all bits = 1 (known)
            };
            chunks.push((known, value));
        }
        PackedLogicVec { chunks, width: w }
    }

    /// Create from a u64 value with given width (0/1 bits only).
    /// Untuk sinyal >64 bit, hanya lower 64 bit yang diisi; sisanya X.
    pub fn from_u64(val: u64, width: usize) -> Self {
        let w = width.max(1);
        let num_chunks = w.div_ceil(CELLS_PER_CHUNK);
        let mut chunks = Vec::with_capacity(num_chunks);
        for c in 0..num_chunks {
            let bit_offset = c * CELLS_PER_CHUNK;
            let chunk_width = (w - bit_offset).min(CELLS_PER_CHUNK);
            let mask = if chunk_width >= 64 {
                !0u64
            } else {
                (1u64 << chunk_width) - 1
            };
            let shifted = if bit_offset < 64 {
                val >> bit_offset
            } else {
                0
            };
            let value = shifted & mask;
            let known = mask; // All bits are known 0/1
            chunks.push((known, value));
        }
        PackedLogicVec { chunks, width: w }
    }

    /// Convert from a traditional LogicVec (Vec<LogicVal> based).
    pub fn from_logicvec(lv: &LogicVec) -> Self {
        let w = lv.width.max(1);
        let num_chunks = w.div_ceil(CELLS_PER_CHUNK);
        let mut chunks = vec![(0u64, 0u64); num_chunks];
        #[allow(clippy::needless_range_loop)]
        for i in 0..lv.width {
            let (k, v) = match lv.bits[i] {
                LogicVal::X => (0u64, 0u64),
                LogicVal::Z => (0u64, 1u64),
                LogicVal::Zero => (1u64, 0u64),
                LogicVal::One => (1u64, 1u64),
            };
            let (chunk_idx, bit) = (i / CELLS_PER_CHUNK, i % CELLS_PER_CHUNK);
            chunks[chunk_idx].0 |= k << bit;
            chunks[chunk_idx].1 |= v << bit;
        }
        PackedLogicVec { chunks, width: w }
    }

    /// Convert back to a traditional LogicVec.
    pub fn to_logicvec(&self) -> LogicVec {
        let mut bits = Vec::with_capacity(self.width);
        for i in 0..self.width {
            let (chunk_idx, bit) = (i / CELLS_PER_CHUNK, i % CELLS_PER_CHUNK);
            let (known, value) = self.chunks[chunk_idx];
            let k = (known >> bit) & 1;
            let v = (value >> bit) & 1;
            bits.push(match (k, v) {
                (0, 0) => LogicVal::X,
                (0, 1) => LogicVal::Z,
                (1, 0) => LogicVal::Zero,
                (1, 1) => LogicVal::One,
                _ => LogicVal::X,
            });
        }
        let width = self.width;
        LogicVec { bits, width }
    }

    // ─── Mask Helpers ───

    /// Get the active bit mask for the last chunk.
    fn last_chunk_mask(&self) -> u64 {
        if self.width == 0 {
            return 0;
        }
        let last_bit = self.width % CELLS_PER_CHUNK;
        if last_bit == 0 {
            !0u64
        } else {
            (1u64 << last_bit) - 1
        }
    }

    /// Apply width mask to all chunks.
    fn apply_mask(&self) -> Vec<(u64, u64)> {
        let last_idx = self.chunks.len().saturating_sub(1);
        let mask = self.last_chunk_mask();
        self.chunks
            .iter()
            .enumerate()
            .map(|(i, &(k, v))| {
                if i == last_idx && mask != !0u64 {
                    (k & mask, v & mask)
                } else {
                    (k, v)
                }
            })
            .collect()
    }

    /// Get a single cell as LogicVal.
    fn get_cell(&self, i: usize) -> LogicVal {
        if i >= self.width {
            return LogicVal::X;
        }
        let (chunk_idx, bit) = (i / CELLS_PER_CHUNK, i % CELLS_PER_CHUNK);
        let (known, value) = self.chunks[chunk_idx];
        let k = (known >> bit) & 1;
        let v = (value >> bit) & 1;
        match (k, v) {
            (0, 0) => LogicVal::X,
            (0, 1) => LogicVal::Z,
            (1, 0) => LogicVal::Zero,
            (1, 1) => LogicVal::One,
            _ => LogicVal::X,
        }
    }

    // ─── Accessors ───

    /// Get the width in cells (bits).
    pub fn width(&self) -> usize {
        self.width
    }

    /// Get the number of chunks.
    pub fn num_chunks(&self) -> usize {
        self.chunks.len()
    }

    /// Get raw chunk data: slice of (known, value) pairs.
    pub fn chunks(&self) -> &[(u64, u64)] {
        &self.chunks
    }

    /// Check if all cells are X.
    pub fn all_x(&self) -> bool {
        let masked = self.apply_mask();
        masked
            .iter()
            .all(|&(known, value)| known == 0 && value == 0)
    }

    /// Check if all cells are Z.
    pub fn all_z(&self) -> bool {
        let masked = self.apply_mask();
        if masked.is_empty() {
            return true;
        }
        // For Z, known=0 and value=1 for each cell
        // So known must be 0, and value must have all 1s within the active width
        let last_idx = masked.len() - 1;
        masked.iter().enumerate().all(|(i, &(known, value))| {
            if i == last_idx {
                // Last chunk: all bits must be Z within the active width
                // Z = known:0, value:1 for each cell
                known == 0 && value == self.last_chunk_mask()
            } else {
                known == 0 && value == !0u64
            }
        })
    }

    /// Convert to u64 (only known bits; X/Z bits become 0).
    pub fn to_u64(&self) -> u64 {
        if self.chunks.is_empty() {
            return 0;
        }
        let (known, value) = self.chunks[0];
        // Only return known 0/1 bits, masked to active width
        let chunk_width = self.width.min(CELLS_PER_CHUNK);
        let width_mask = if chunk_width >= 64 {
            !0u64
        } else {
            (1u64 << chunk_width) - 1
        };
        (value & known) & width_mask
    }

    /// Convert to bool (1-bit interpretation).
    pub fn to_bool(&self) -> Option<bool> {
        if self.width == 0 {
            return Some(false);
        }
        let (known0, value0) = self.chunks[0];
        let chunk_width = self.width.min(CELLS_PER_CHUNK);
        let width_mask = if chunk_width >= 64 {
            !0u64
        } else {
            (1u64 << chunk_width) - 1
        };
        let known = known0 & width_mask;
        let value = value0 & width_mask;

        // All X or Z → return None
        if known == 0 {
            return None;
        }
        // Any bit is 1 (and known) → true
        let any_one = (value & known) != 0;
        Some(any_one)
    }

    /// Resize to new width (truncate or zero-extend).
    pub fn resize(&self, new_width: usize) -> Self {
        if new_width == self.width {
            return self.clone();
        }
        let w = new_width.max(1);
        if w <= self.width {
            // Truncate: just mask the excess bits
            let mut chunks = self.chunks.clone();
            let last_bit = w % CELLS_PER_CHUNK;
            if last_bit > 0 {
                let last_idx = chunks.len() - 1;
                let mask = (1u64 << last_bit) - 1;
                chunks[last_idx].0 &= mask;
                chunks[last_idx].1 &= mask;
            }
            // Remove any chunks beyond the new width
            let new_chunks = w.div_ceil(CELLS_PER_CHUNK);
            chunks.truncate(new_chunks);
            PackedLogicVec { chunks, width: w }
        } else {
            // Extend: zero-extend (fill new bits with known=1, value=0)
            let old_width = self.width;
            let new_chunks = w.div_ceil(CELLS_PER_CHUNK);
            let mut chunks = self.chunks.clone();
            chunks.resize(new_chunks, (0u64, 0u64));
            // Fill the new bits in the last old chunk with zeros
            let old_last_bit = old_width % CELLS_PER_CHUNK;
            if old_last_bit > 0 {
                let last_idx = old_width / CELLS_PER_CHUNK;
                if last_idx < chunks.len() {
                    let old_mask = (1u64 << old_last_bit) - 1;
                    let new_mask = if CELLS_PER_CHUNK >= 64 {
                        !0u64
                    } else {
                        (1u64 << CELLS_PER_CHUNK) - 1
                    };
                    let extend_mask = new_mask & !old_mask;
                    // Set known=1, value=0 for new bits
                    chunks[last_idx].0 |= extend_mask;
                    // value bits already 0
                }
            }
            // New chunks are already (0, 0) = X, need to make them zero
            #[allow(clippy::needless_range_loop)]
            for i in (old_width / CELLS_PER_CHUNK + 1)..new_chunks {
                let chunk_width = (w - i * CELLS_PER_CHUNK).min(CELLS_PER_CHUNK);
                let mask = if chunk_width >= 64 {
                    !0u64
                } else {
                    (1u64 << chunk_width) - 1
                };
                chunks[i] = (mask, 0u64); // known=1, value=0 for all bits
            }
            // Mask the last chunk
            let last_bit = w % CELLS_PER_CHUNK;
            if last_bit > 0 {
                let last_idx = chunks.len() - 1;
                let mask = (1u64 << last_bit) - 1;
                chunks[last_idx].0 &= mask;
                chunks[last_idx].1 &= mask;
            }
            PackedLogicVec { chunks, width: w }
        }
    }

    /// Concatenate two packed vectors.
    pub fn extend(&self, other: &PackedLogicVec) -> Self {
        // Fallback: convert to LogicVec, concat, convert back
        // Optimization: chunk-level bit manipulation bisa ditambahkan nanti
        let mut lv_self = self.to_logicvec();
        let lv_other = other.to_logicvec();
        lv_self.bits.extend(lv_other.bits);
        lv_self.width = self.width + other.width;
        PackedLogicVec::from_logicvec(&lv_self)
    }

    // ─── Bitwise Operations (SIMD-friendly) ───

    /// Bitwise AND — 4-state correct.
    ///
    /// Truth table: 0 mendominasi (a&0=0). X/Z dengan non-zero → X.
    /// Formula per bit:
    /// - result = 0 jika a=0 ATAU b=0
    /// - result = 1 jika a=1 DAN b=1
    /// - result = X otherwise
    pub fn bitwise_and(&self, other: &PackedLogicVec) -> PackedLogicVec {
        let max_width = self.width.max(other.width);
        let self_ext = self.resize(max_width);
        let other_ext = other.resize(max_width);
        let chunks = crate::simulator::simd_packed::simd_and(&self_ext.chunks, &other_ext.chunks);
        PackedLogicVec {
            chunks,
            width: max_width,
        }
    }

    /// Bitwise OR — 4-state correct.
    ///
    /// Truth table: 1 mendominasi (a|1=1). X/Z dengan non-zero → X.
    /// Formula per bit:
    /// - result = 1 jika a=1 ATAU b=1
    /// - result = 0 jika a=0 DAN b=0
    /// - result = X otherwise
    pub fn bitwise_or(&self, other: &PackedLogicVec) -> PackedLogicVec {
        let max_width = self.width.max(other.width);
        let self_ext = self.resize(max_width);
        let other_ext = other.resize(max_width);
        let chunks = crate::simulator::simd_packed::simd_or(&self_ext.chunks, &other_ext.chunks);
        PackedLogicVec {
            chunks,
            width: max_width,
        }
    }

    /// Bitwise XOR — 4-state correct.
    ///
    /// Truth table: X/Z if either input is X/Z, normal XOR otherwise.
    /// Formula per bit:
    /// - known = a.known & b.known
    /// - value = (a.value ^ b.value) & known
    pub fn bitwise_xor(&self, other: &PackedLogicVec) -> PackedLogicVec {
        let max_width = self.width.max(other.width);
        let self_ext = self.resize(max_width);
        let other_ext = other.resize(max_width);
        let chunks = crate::simulator::simd_packed::simd_xor(&self_ext.chunks, &other_ext.chunks);
        PackedLogicVec {
            chunks,
            width: max_width,
        }
    }

    /// Bitwise XNOR = NOT(XOR).
    pub fn bitwise_xnor(&self, other: &PackedLogicVec) -> PackedLogicVec {
        let max_width = self.width.max(other.width);
        let self_ext = self.resize(max_width);
        let other_ext = other.resize(max_width);
        let xor = self_ext.bitwise_xor(&other_ext);
        xor.bitwise_not()
    }

    /// Bitwise NOT — 2 operasi per chunk.
    ///
    /// Formula: known unchanged, value flipped where known.
    /// X→X, Z→Z, 0→1, 1→0
    pub fn bitwise_not(&self) -> PackedLogicVec {
        let chunks = crate::simulator::simd_packed::simd_not(&self.chunks);
        PackedLogicVec {
            chunks,
            width: self.width,
        }
    }

    // ─── Reduction Operations ───

    /// Reduction AND: returns 1 if all bits are 1, 0 if any bit is 0, X otherwise.
    pub fn red_and(&self) -> PackedLogicVec {
        let last_idx = self.chunks.len().saturating_sub(1);
        let last_chunk_full_mask = self.last_chunk_mask();
        // Iterate over original chunks, applying mask for last chunk
        for (i, &(known, value)) in self.chunks.iter().enumerate() {
            // Determine the full mask for this chunk
            let full_mask = if i == last_idx && last_chunk_full_mask != !0u64 {
                last_chunk_full_mask
            } else {
                !0u64
            };
            let km = known & full_mask;
            let vm = value & full_mask;
            // Any known 0 bit → result is 0
            let known_zero = km & !vm;
            if known_zero != 0 {
                return PackedLogicVec::fill(LogicVal::Zero, 1);
            }
            // Any unknown (X/Z) bit in this chunk
            if km != full_mask {
                return PackedLogicVec::fill(LogicVal::X, 1);
            }
        }
        // All bits are known 1
        PackedLogicVec::fill(LogicVal::One, 1)
    }

    /// Reduction OR: returns 1 if any bit is 1, 0 if all bits are 0, X otherwise.
    pub fn red_or(&self) -> PackedLogicVec {
        let last_idx = self.chunks.len().saturating_sub(1);
        let last_chunk_full_mask = self.last_chunk_mask();
        let mut all_known = true;
        for (i, &(known, value)) in self.chunks.iter().enumerate() {
            let full_mask = if i == last_idx && last_chunk_full_mask != !0u64 {
                last_chunk_full_mask
            } else {
                !0u64
            };
            let km = known & full_mask;
            let vm = value & full_mask;
            // Any known 1 bit → result is 1
            let known_one = km & vm;
            if known_one != 0 {
                return PackedLogicVec::fill(LogicVal::One, 1);
            }
            // Any unknown (X/Z) bit
            if km != full_mask {
                all_known = false;
            }
        }
        if all_known {
            PackedLogicVec::fill(LogicVal::Zero, 1)
        } else {
            PackedLogicVec::fill(LogicVal::X, 1)
        }
    }

    /// Reduction XOR: returns 1 if odd number of 1s, 0 if even, X if any X/Z.
    pub fn red_xor(&self) -> PackedLogicVec {
        let last_idx = self.chunks.len().saturating_sub(1);
        let last_chunk_full_mask = self.last_chunk_mask();
        let mut xor_acc = 0u64;
        let mut all_known = true;
        for (i, &(known, value)) in self.chunks.iter().enumerate() {
            let full_mask = if i == last_idx && last_chunk_full_mask != !0u64 {
                last_chunk_full_mask
            } else {
                !0u64
            };
            let km = known & full_mask;
            let vm = value & full_mask;
            if km != full_mask {
                all_known = false;
            }
            xor_acc ^= vm & km;
        }
        if !all_known {
            PackedLogicVec::fill(LogicVal::X, 1)
        } else {
            let parity = xor_acc.count_ones() & 1;
            PackedLogicVec::from_u64(parity as u64, 1)
        }
    }

    /// Reduction NAND.
    pub fn red_nand(&self) -> PackedLogicVec {
        let and = self.red_and();
        and.bitwise_not()
    }

    /// Reduction NOR.
    pub fn red_nor(&self) -> PackedLogicVec {
        let or = self.red_or();
        or.bitwise_not()
    }

    /// Reduction XNOR.
    pub fn red_xnor(&self) -> PackedLogicVec {
        let xor = self.red_xor();
        xor.bitwise_not()
    }

    // ─── Shift Operations ───

    /// Logical shift left.
    pub fn shl(&self, shift: usize) -> PackedLogicVec {
        if shift == 0 || self.width == 0 {
            return self.clone();
        }
        if shift >= self.width {
            return PackedLogicVec::fill(LogicVal::Zero, self.width);
        }
        // Fallback ke LogicVec untuk initial implementation
        let mut lv = self.to_logicvec();
        for i in (shift..self.width).rev() {
            lv.bits[i] = lv.bits[i - shift];
        }
        for b in lv.bits.iter_mut().take(shift.min(self.width)) {
            *b = LogicVal::Zero;
        }
        PackedLogicVec::from_logicvec(&lv)
    }

    /// Logical shift right.
    pub fn shr(&self, shift: usize) -> PackedLogicVec {
        if shift == 0 || self.width == 0 {
            return self.clone();
        }
        if shift >= self.width {
            return PackedLogicVec::fill(LogicVal::Zero, self.width);
        }
        let mut lv = self.to_logicvec();
        for i in 0..(self.width - shift) {
            lv.bits[i] = lv.bits[i + shift];
        }
        for b in lv.bits.iter_mut().skip(self.width - shift) {
            *b = LogicVal::Zero;
        }
        PackedLogicVec::from_logicvec(&lv)
    }

    /// Arithmetic shift left (same as logical for unsigned).
    pub fn sshl(&self, shift: usize) -> PackedLogicVec {
        self.shl(shift)
    }

    /// Arithmetic shift right (sign-extend).
    pub fn sshr(&self, shift: usize) -> PackedLogicVec {
        if shift == 0 || self.width == 0 {
            return self.clone();
        }
        if shift >= self.width {
            let msb = self.get_cell(self.width - 1);
            return PackedLogicVec::fill(msb, self.width);
        }
        let msb = self.get_cell(self.width - 1);
        let mut lv = self.to_logicvec();
        for i in 0..(self.width - shift) {
            lv.bits[i] = lv.bits[i + shift];
        }
        for b in lv.bits.iter_mut().skip(self.width - shift) {
            *b = msb;
        }
        PackedLogicVec::from_logicvec(&lv)
    }

    // ─── Comparison Operations ───

    /// Equality comparison (Verilog === semantics: X/Z must match exactly).
    pub fn eq(&self, other: &PackedLogicVec) -> PackedLogicVec {
        let max_chunks = self.chunks.len().max(other.chunks.len());
        for i in 0..max_chunks {
            let (ak, av) = self.chunks.get(i).copied().unwrap_or((0, 0));
            let (bk, bv) = other.chunks.get(i).copied().unwrap_or((0, 0));
            // For ===: ALL bits (including X/Z) must match exactly
            if ak != bk || av != bv {
                return PackedLogicVec::from_u64(0, 1);
            }
        }
        PackedLogicVec::from_u64(1, 1)
    }

    /// Casex equality: X/Z in the pattern are don't-care.
    pub fn casex_eq(&self, pattern: &PackedLogicVec) -> PackedLogicVec {
        // Gunakan LogicVec fallback untuk correctness
        let val_lv = self.to_logicvec();
        let pat_lv = pattern.to_logicvec();
        let result = val_lv.casex_eq(&pat_lv);
        PackedLogicVec::from_u64(if result { 1 } else { 0 }, 1)
    }

    /// Casez equality: Z in the pattern is don't-care.
    pub fn casez_eq(&self, pattern: &PackedLogicVec) -> PackedLogicVec {
        let val_lv = self.to_logicvec();
        let pat_lv = pattern.to_logicvec();
        let result = val_lv.casez_eq(&pat_lv);
        PackedLogicVec::from_u64(if result { 1 } else { 0 }, 1)
    }

    /// Not-equal.
    pub fn neq(&self, other: &PackedLogicVec) -> PackedLogicVec {
        let eq = self.eq(other);
        eq.bitwise_not()
    }

    // ─── Arithmetic Operations ───

    /// Addition — fallback ke LogicVec karena kompleksitas carry.
    pub fn add(&self, other: &PackedLogicVec) -> PackedLogicVec {
        let lv_self = self.to_logicvec();
        let lv_other = other.to_logicvec();
        let result = crate::simulator::value::eval_binary(BinaryIrOp::Add, &lv_self, &lv_other);
        PackedLogicVec::from_logicvec(&result)
    }

    /// Subtraction.
    pub fn sub(&self, other: &PackedLogicVec) -> PackedLogicVec {
        let lv_self = self.to_logicvec();
        let lv_other = other.to_logicvec();
        let result = crate::simulator::value::eval_binary(BinaryIrOp::Sub, &lv_self, &lv_other);
        PackedLogicVec::from_logicvec(&result)
    }
}

impl std::fmt::Display for PackedLogicVec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for i in (0..self.width).rev() {
            let (chunk_idx, bit) = (i / CELLS_PER_CHUNK, i % CELLS_PER_CHUNK);
            let (known, value) = self.chunks[chunk_idx];
            let k = (known >> bit) & 1;
            let v = (value >> bit) & 1;
            let c = match (k, v) {
                (0, 0) => 'x',
                (0, 1) => 'z',
                (1, 0) => '0',
                (1, 1) => '1',
                _ => '?',
            };
            write!(f, "{}", c)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mivon_ir::{BinaryIrOp, LogicVal, LogicVec};

    fn lv(bits: Vec<LogicVal>) -> LogicVec {
        let width = bits.len();
        LogicVec { bits, width }
    }

    // ─── Construction ───

    #[test]
    fn test_new_all_x() {
        let p = PackedLogicVec::new(8);
        assert_eq!(p.width(), 8);
        assert_eq!(p.num_chunks(), 1);
        assert!(p.all_x());
        assert_eq!(p.to_u64(), 0);
    }

    #[test]
    fn test_fill_states() {
        let w = 8;
        let zero = PackedLogicVec::fill(LogicVal::Zero, w);
        assert!(zero.to_bool() == Some(false));
        assert_eq!(zero.to_u64(), 0);
        assert!(!zero.all_x());
        assert!(!zero.all_z());

        let one = PackedLogicVec::fill(LogicVal::One, w);
        assert!(one.to_bool() == Some(true));
        assert_eq!(one.to_u64(), 0xFF);
        assert!(!one.all_x());
        assert!(!one.all_z());

        let x = PackedLogicVec::fill(LogicVal::X, w);
        assert!(x.to_bool().is_none());
        assert_eq!(x.to_u64(), 0);
        assert!(x.all_x());
        assert!(!x.all_z());

        let z = PackedLogicVec::fill(LogicVal::Z, w);
        assert!(z.to_bool().is_none());
        assert_eq!(z.to_u64(), 0);
        assert!(!z.all_x()); // Z is not X
        assert!(z.all_z());
    }

    #[test]
    fn test_from_u64_roundtrip() {
        let p = PackedLogicVec::from_u64(0xAB, 8);
        assert_eq!(p.to_u64(), 0xAB);
        assert_eq!(format!("{}", p), "10101011");

        // width > 64: lower 64 bits filled, rest known=0 (not X)
        let wide = PackedLogicVec::from_u64(0xFF, 128);
        assert_eq!(wide.to_u64(), 0xFF);
        assert_eq!(wide.width(), 128);
        assert_eq!(wide.num_chunks(), 2);
        // upper chunk is all known-zero: (known=mask, value=0)
        assert_eq!(wide.chunks()[1], (!0u64, 0u64));
    }

    #[test]
    fn test_logicvec_roundtrip() {
        let lv = lv(vec![LogicVal::One, LogicVal::Zero, LogicVal::X, LogicVal::Z]);
        let p = PackedLogicVec::from_logicvec(&lv);
        assert_eq!(p.width(), 4);
        let back = p.to_logicvec();
        assert_eq!(back.bits, lv.bits);
        assert_eq!(back.width, 4);
    }

    // ─── Accessors ───

    #[test]
    fn test_width_num_chunks() {
        let p = PackedLogicVec::new(1);
        assert_eq!(p.num_chunks(), 1);
        let p = PackedLogicVec::new(64);
        assert_eq!(p.num_chunks(), 1);
        let p = PackedLogicVec::new(65);
        assert_eq!(p.num_chunks(), 2);
        let p = PackedLogicVec::new(128);
        assert_eq!(p.num_chunks(), 2);
    }

    #[test]
    fn test_to_bool() {
        assert_eq!(
            PackedLogicVec::fill(LogicVal::One, 8).to_bool(),
            Some(true)
        );
        assert_eq!(
            PackedLogicVec::fill(LogicVal::Zero, 8).to_bool(),
            Some(false)
        );
        assert_eq!(PackedLogicVec::fill(LogicVal::X, 8).to_bool(), None);
        assert_eq!(PackedLogicVec::fill(LogicVal::Z, 8).to_bool(), None);
        // single 1 bit among zeros → true
        let p = PackedLogicVec::from_u64(0x01, 8);
        assert_eq!(p.to_bool(), Some(true));
    }

    // ─── Resize ───

    #[test]
    fn test_resize_truncate() {
        let p = PackedLogicVec::from_u64(0xFF, 8);
        let t = p.resize(4);
        assert_eq!(t.width(), 4);
        assert_eq!(t.to_u64(), 0x0F); // low 4 bits kept
    }

    #[test]
    fn test_resize_extend() {
        let p = PackedLogicVec::from_u64(0x0F, 4);
        let e = p.resize(8);
        assert_eq!(e.width(), 8);
        // zero-extend: low bits kept, high bits known 0
        assert_eq!(e.to_u64(), 0x0F);
        assert_eq!(format!("{}", e), "00001111");
    }

    // ─── Bitwise ───

    #[test]
    fn test_bitwise_and_or_not() {
        let a = PackedLogicVec::from_u64(0x0F, 8); // 00001111
        let b = PackedLogicVec::from_u64(0xCC, 8); // 11001100
        let and = a.bitwise_and(&b); // 00001100 = 0x0C
        assert_eq!(and.to_u64(), 0x0C);
        let or = a.bitwise_or(&b); // 11001111 = 0xCF
        assert_eq!(or.to_u64(), 0xCF);
        let not_a = a.bitwise_not(); // 11110000 = 0xF0
        assert_eq!(not_a.to_u64(), 0xF0);
    }

    #[test]
    fn test_bitwise_xor_xnor() {
        let a = PackedLogicVec::from_u64(0x0F, 8);
        let b = PackedLogicVec::from_u64(0xCC, 8);
        let xor = a.bitwise_xor(&b); // 11000011 = 0xC3
        assert_eq!(xor.to_u64(), 0xC3);
        let xnor = a.bitwise_xnor(&b); // 00111100 = 0x3C
        assert_eq!(xnor.to_u64(), 0x3C);
    }

    #[test]
    fn test_bitwise_with_x_propagates() {
        // X in either operand → X in result (4-state)
        let lv = lv(vec![LogicVal::X, LogicVal::One]);
        let a = PackedLogicVec::from_logicvec(&lv); // x1
        let b = PackedLogicVec::from_u64(0x01, 2); // 01
        // a & b: bit0: X&1 = X, bit1: 1&0 = 0 → x0
        let r = a.bitwise_and(&b);
        let back = r.to_logicvec();
        assert_eq!(back.bits[0], LogicVal::X);
        assert_eq!(back.bits[1], LogicVal::Zero);
    }

    // ─── Reduction ───

    #[test]
    fn test_red_and() {
        assert_eq!(
            PackedLogicVec::fill(LogicVal::One, 8).red_and().to_bool(),
            Some(true)
        );
        assert_eq!(
            PackedLogicVec::fill(LogicVal::Zero, 8).red_and().to_bool(),
            Some(false)
        );
        assert!(PackedLogicVec::fill(LogicVal::X, 8).red_and().to_bool().is_none());
        // mixed 1 and 0 → 0
        let p = PackedLogicVec::from_u64(0x01, 8); // 00000001
        assert_eq!(p.red_and().to_bool(), Some(false));
    }

    #[test]
    fn test_red_or() {
        assert_eq!(
            PackedLogicVec::fill(LogicVal::Zero, 8).red_or().to_bool(),
            Some(false)
        );
        assert_eq!(
            PackedLogicVec::fill(LogicVal::One, 8).red_or().to_bool(),
            Some(true)
        );
        assert!(PackedLogicVec::fill(LogicVal::X, 8).red_or().to_bool().is_none());
        // only zeros → 0
        let p = PackedLogicVec::from_u64(0x00, 8);
        assert_eq!(p.red_or().to_bool(), Some(false));
        // any one → 1
        let p = PackedLogicVec::from_u64(0x01, 8);
        assert_eq!(p.red_or().to_bool(), Some(true));
    }

    #[test]
    fn test_red_xor() {
        // even number of 1s → 0, odd → 1
        let p = PackedLogicVec::from_u64(0x03, 8); // 00000011 (two 1s)
        assert_eq!(p.red_xor().to_bool(), Some(false));
        let p = PackedLogicVec::from_u64(0x01, 8); // one 1
        assert_eq!(p.red_xor().to_bool(), Some(true));
        // X present → X
        let lv = lv(vec![LogicVal::One, LogicVal::X]);
        assert!(PackedLogicVec::from_logicvec(&lv).red_xor().to_bool().is_none());
    }

    #[test]
    fn test_red_nand_nor_xnor() {
        let all_one = PackedLogicVec::fill(LogicVal::One, 4);
        assert_eq!(all_one.red_nand().to_bool(), Some(false)); // not(1) = 0
        assert_eq!(all_one.red_nor().to_bool(), Some(false));  // not(1) = 0
        assert_eq!(all_one.red_xnor().to_bool(), Some(true));  // not(xor of all 1s even→0) = 1

        let all_zero = PackedLogicVec::fill(LogicVal::Zero, 4);
        assert_eq!(all_zero.red_nand().to_bool(), Some(true)); // not(0) = 1
        assert_eq!(all_zero.red_nor().to_bool(), Some(true));  // not(0) = 1
    }

    // ─── Shift ───

    #[test]
    fn test_shl_shr() {
        let p = PackedLogicVec::from_u64(0x01, 8); // 00000001
        assert_eq!(p.shl(1).to_u64(), 0x02);
        assert_eq!(p.shr(1).to_u64(), 0x00);
        let p = PackedLogicVec::from_u64(0x80, 8); // 10000000
        assert_eq!(p.shr(1).to_u64(), 0x40);
        // shift >= width → zero
        assert_eq!(p.shl(8).to_u64(), 0);
        assert_eq!(p.shr(8).to_u64(), 0);
    }

    #[test]
    fn test_sshr_sign_extend() {
        // MSB = 1 → arithmetic shift right fills with 1
        let p = PackedLogicVec::from_u64(0x80, 8); // 10000000, MSB=1
        let r = p.sshr(1);
        assert_eq!(r.to_u64(), 0xC0); // 11000000
        // MSB = 0 → fills with 0
        let p = PackedLogicVec::from_u64(0x40, 8); // 01000000, MSB=0
        let r = p.sshr(1);
        assert_eq!(r.to_u64(), 0x20); // 00100000
    }

    // ─── Comparison ───

    #[test]
    fn test_eq_neq() {
        let a = PackedLogicVec::from_u64(0xAB, 8);
        let b = PackedLogicVec::from_u64(0xAB, 8);
        let c = PackedLogicVec::from_u64(0xCD, 8);
        assert_eq!(a.eq(&b).to_bool(), Some(true));
        assert_eq!(a.eq(&c).to_bool(), Some(false));
        assert_eq!(a.neq(&c).to_bool(), Some(true));
        assert_eq!(a.neq(&b).to_bool(), Some(false));
    }

    #[test]
    fn test_casex_casez() {
        // casex: pattern X is don't-care.
        // val = [X, Zero, One, Zero], pat = [X, Zero, One, X]
        //   pattern bit0=X (don't-care), bit1=Zero matches, bit2=One matches,
        //   bit3=X (don't-care) → all match → true
        let val = PackedLogicVec::from_logicvec(&lv(vec![
            LogicVal::X, LogicVal::Zero, LogicVal::One, LogicVal::Zero,
        ]));
        let pat = PackedLogicVec::from_logicvec(&lv(vec![
            LogicVal::X, LogicVal::Zero, LogicVal::One, LogicVal::X,
        ]));
        assert_eq!(val.casex_eq(&pat).to_bool(), Some(true));
        // casex false when a non-X pattern bit differs
        let pat2 = PackedLogicVec::from_logicvec(&lv(vec![
            LogicVal::X, LogicVal::One, LogicVal::One, LogicVal::X,
        ])); // bit1=One vs val Zero → differ
        assert_eq!(val.casex_eq(&pat2).to_bool(), Some(false));

        // casez: pattern Z is don't-care.
        // vz = [One, Z], patz = [One, Z] → bit0 matches, bit1 pattern Z don't-care → true
        let vz = PackedLogicVec::from_logicvec(&lv(vec![LogicVal::One, LogicVal::Z]));
        let patz = PackedLogicVec::from_logicvec(&lv(vec![LogicVal::One, LogicVal::Z]));
        assert_eq!(vz.casez_eq(&patz).to_bool(), Some(true));
    }

    // ─── Arithmetic ───

    #[test]
    fn test_add_sub() {
        let a = PackedLogicVec::from_u64(10, 8);
        let b = PackedLogicVec::from_u64(20, 8);
        assert_eq!(a.add(&b).to_u64(), 30);
        assert_eq!(b.sub(&a).to_u64(), 10);
    }

    // ─── Multi-chunk (>64 bit) ───

    #[test]
    fn test_multi_chunk_resize_and_ops() {
        let p = PackedLogicVec::from_u64(u64::MAX, 128);
        assert_eq!(p.width(), 128);
        assert_eq!(p.num_chunks(), 2);
        // lower chunk all ones, upper chunk known-zero (from_u64 fills known=mask)
        assert_eq!(p.chunks()[0], (!0u64, !0u64));
        assert_eq!(p.chunks()[1], (!0u64, 0u64)); // upper: all known 0

        // bitwise_not flips lower chunk to all zeros (known), upper stays known-zero → NOT → all ones
        let not = p.bitwise_not();
        assert_eq!(not.chunks()[0].1, 0u64); // value flipped to 0
        assert_eq!(not.chunks()[0].0, !0u64); // all known
        assert_eq!(not.chunks()[1], (!0u64, !0u64)); // upper: NOT of zeros = all ones
    }

    #[test]
    fn test_extend_concat() {
        let a = PackedLogicVec::from_u64(0xAB, 8);
        let b = PackedLogicVec::from_u64(0xCD, 8);
        let c = a.extend(&b);
        assert_eq!(c.width(), 16);
        // extend: self (a) in LOW bits, other (b) in HIGH bits → low byte = a
        let back = c.to_logicvec();
        assert_eq!(back.bits[0..8], a.to_logicvec().bits);
        assert_eq!(back.bits[8..16], b.to_logicvec().bits);
    }

    // ─── Display ───

    #[test]
    fn test_display() {
        let p = PackedLogicVec::from_u64(0x0A, 8); // 00001010
        let s = format!("{}", p);
        assert_eq!(s, "00001010");

        let pz = PackedLogicVec::fill(LogicVal::Z, 4);
        assert_eq!(format!("{}", pz), "zzzz");
    }
}
