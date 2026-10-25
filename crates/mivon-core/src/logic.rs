//! Tipe nilai logika 4-state (X/Z/0/1) — fondasi seluruh simulator.
//!
//! Dipindah dari `src/ir/ir.rs` (migrasi monorepo: src/ → crates/) karena
//! dipakai lintas crate (ast `FillLit`, ir, simulator, waveform, hir) dan
//! memutus siklus dependensi ast ↔ ir: `ast` kini memakai `mivon_core::LogicVal`,
//! `mivon-ir` me-reexport dari sini agar `mivon_ir::LogicVec` tetap valid.

use std::cell::RefCell;

/// Signature untuk allocator LogicVec custom (arena-backed).
pub type LogicVecCtor = fn(usize, LogicVal) -> Option<LogicVec>;

thread_local! {
    /// Custom constructor yang di-register oleh SimulationArena (atau None utk heap default).
    static LOGICVEC_CTOR: RefCell<Option<LogicVecCtor>> = const { RefCell::new(None) };
}

/// Register constructor LogicVec custom (mis. alokasi dari arena).
/// Berikan `None` untuk kembali ke heap allocation default.
pub fn set_logicvec_ctor(ctor: Option<LogicVecCtor>) {
    LOGICVEC_CTOR.with(|cell| *cell.borrow_mut() = ctor);
}

fn get_logicvec_ctor() -> Option<LogicVecCtor> {
    LOGICVEC_CTOR.with(|cell| *cell.borrow())
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct LogicVec {
    pub bits: Vec<LogicVal>,
    pub width: usize,
}

/// Alokasi `w` slot bernilai `X` TANPA men-touch halaman (zero page COW).
/// `new_zeroed_slice` memakai allocator zeroed → halaman nol virtual; RSS
/// hanya tumbuh saat slot benar-benar ditulis (sim array memory besar).
/// Safety: `LogicVal` unit enum tanpa padding — bytes nol = discriminant 0
/// (`X`, setelah reorder) — setiap elemen valid; `from_raw_parts` mengambil
/// ownership dari `Box` (drop tunggal, capacity = len).
fn lazy_zeroed(w: usize) -> Vec<LogicVal> {
    let boxed = Box::<[std::mem::MaybeUninit<LogicVal>]>::new_zeroed_slice(w);
    let ptr = Box::into_raw(boxed) as *mut LogicVal;
    // SAFETY: memori zeroed → LogicVal::X (variasi pertama, diskriminant 0)
    // valid utk enum unit tanpa padding/niche — bukan uninit.
    unsafe { Vec::from_raw_parts(ptr, w, w) }
}

impl Default for LogicVec {
    fn default() -> Self {
        LogicVec::new(1)
    }
}

impl LogicVec {
    /// Ambang guard OOM bersama (lihat `new`). Dipakai SEMUA constructor —
    /// literal `4294967295'h0` lewat `fill`/`from_u64` tanpa clamp = alokasi
    /// 4G+ slot (hang/OOM), bukan graceful.
    fn sane_width(width: usize) -> usize {
        if width > (1 << 27) {
            1
        } else {
            width
        }
    }

    /// Width dari sini dialokasikan LAZY-ZERO (`new` / `fill(X)`): halaman
    /// nol COW → RSS hanya halaman yang benar-benar ditulis (array memory
    /// flatten raksasa — cache 65536x32x512 = 1.07e9 entry — tidak OOM).
    /// Threshold 4M bit (~4MB, hitung utk LogicVal 1-byte) jauh di atas
    /// sinyal normal & SRAM realistis; hanya unpacked array besar yang kena.
    pub const LAZY_ZERO_THRESHOLD: usize = 1 << 22;

    pub fn new(width: usize) -> Self {
        // Guard OOM untuk lebar signal BOGUS (hasil width-computation yang
        // salah / overflow param bisa berorder miliaran bit). Ambang 1<<27 bit
        // (~134MB utk LogicVal 4-state) jauh di atas memory SRAM terbesar yang
        // realistis (OpenTitan sram_ctrl dgn ECC: 65536 x 76 ≈ 4.98M bit) tapi
        // tetap membatasi alokasi absurd. Ambang lama 1M bit memotong SRAM
        // legal → full_init 1 bit → index-out-of-bounds saat init array.
        let w = Self::sane_width(width);
        // Array unpacked besar (flatten memory cache, mis. 65536 x 32 x 512 bit
        // ≈ 1.07e9 entry) TIDAK boleh di-materialisasi penuh → OOM di mesin
        // kecil (AetherX). Lazy-zero: halaman nol COW dari alloc_zeroed; RSS
        // hanya halaman yang benar-benar ditulis/diakses selama simulasi.
        // Nilai nol = X (diskriminant 0) — semantik default tak berubah.
        if w >= Self::LAZY_ZERO_THRESHOLD {
            return LogicVec {
                bits: lazy_zeroed(w),
                width: w,
            };
        }
        // Try arena-backed allocation first (zero-deallocation path)
        if let Some(ctor) = get_logicvec_ctor() {
            if let Some(lv) = ctor(w, LogicVal::X) {
                return lv;
            }
        }
        // Fallback: standard heap allocation
        LogicVec {
            bits: vec![LogicVal::X; w],
            width: w,
        }
    }

    pub fn fill(val: LogicVal, width: usize) -> Self {
        let width = Self::sane_width(width);
        // Lazy-zero bila isi seragam X dan lebar besar (default sinyal/memori).
        if val == LogicVal::X && width >= Self::LAZY_ZERO_THRESHOLD {
            return LogicVec {
                bits: lazy_zeroed(width),
                width,
            };
        }
        // Try arena-backed allocation first (zero-deallocation path)
        if let Some(ctor) = get_logicvec_ctor() {
            if let Some(lv) = ctor(width, val) {
                return lv;
            }
        }
        // Fallback: standard heap allocation
        LogicVec {
            bits: vec![val; width],
            width,
        }
    }

    pub fn from_u64(val: u64, width: usize) -> Self {
        let width = Self::sane_width(width);
        // Try arena-backed allocation first (zero-deallocation path)
        if let Some(ctor) = get_logicvec_ctor() {
            if let Some(mut lv) = ctor(width, LogicVal::Zero) {
                for i in 0..lv.width.min(64) {
                    if (val >> i) & 1 == 1 {
                        lv.bits[i] = LogicVal::One;
                    }
                }
                return lv;
            }
        }
        // Fallback: standard heap allocation
        let mut bits = Vec::with_capacity(width);
        for i in 0..width {
            if i < 64 && (val >> i) & 1 == 1 {
                bits.push(LogicVal::One);
            } else {
                bits.push(LogicVal::Zero);
            }
        }
        LogicVec { bits, width }
    }

    pub fn to_u64(&self) -> u64 {
        let mut result = 0u64;
        for i in 0..self.width.min(64) {
            if self.bits.get(i).copied().unwrap_or(LogicVal::X) == LogicVal::One {
                result |= 1 << i;
            }
        }
        result
    }

    pub fn to_i64(&self) -> i64 {
        if self.width == 0 {
            // Lebar 0 (mis. queue kosong): tak ada bit → nilai 0. Tanpa guard
            // ini `1u64 << (width-1)` underflow → panic.
            return 0;
        }
        let uval = self.to_u64();
        if self.width < 64 {
            let mask = 1u64 << (self.width - 1);
            if uval & mask != 0 {
                (uval | (!0u64 << self.width)) as i64
            } else {
                uval as i64
            }
        } else {
            uval as i64
        }
    }

    pub fn to_bool(&self) -> Option<bool> {
        if self.width == 0 {
            return Some(false);
        }
        let any_one = self.bits.contains(&LogicVal::One);
        if any_one {
            // Ada bit 1 yang diketahui → true (dominasi OR-scalarization).
            return Some(true);
        }
        let has_x_or_z = self
            .bits
            .iter()
            .any(|b| *b == LogicVal::X || *b == LogicVal::Z);
        if has_x_or_z {
            // Tanpa 1 tapi ada X/Z → UNKNOWN (LRM §11.4.4). Dulu diam-diam
            // Some(false) sehingga `x && 1` = 0 padahal x (ditemukan
            // xprop_fuzz seed=0). Pemakai kondisional (`unwrap_or(false)`)
            // tetap konsisten: LRM 12.4 — kondisi x → cabang else.
            return None;
        }
        Some(false)
    }

    pub fn resize(&self, new_width: usize) -> Self {
        if new_width <= self.width {
            // Try arena-backed allocation
            if let Some(ctor) = get_logicvec_ctor() {
                if let Some(mut lv) = ctor(new_width, LogicVal::Zero) {
                    lv.bits[..new_width].copy_from_slice(&self.bits[..new_width]);
                    return lv;
                }
            }
            let mut bits = self.bits.clone();
            bits.truncate(new_width);
            return LogicVec {
                bits,
                width: new_width,
            };
        }
        // Try arena-backed allocation
        if let Some(ctor) = get_logicvec_ctor() {
            if let Some(mut lv) = ctor(new_width, LogicVal::Zero) {
                lv.bits[..self.width].copy_from_slice(&self.bits);
                return lv;
            }
        }
        let mut bits = self.bits.clone();
        bits.resize(new_width, LogicVal::Zero);
        LogicVec {
            bits,
            width: new_width,
        }
    }

    pub fn extend(&self, other: &LogicVec) -> Self {
        let new_width = self.width + other.width;
        // Try arena-backed allocation
        if let Some(ctor) = get_logicvec_ctor() {
            if let Some(mut lv) = ctor(new_width, LogicVal::Zero) {
                lv.bits[..self.width].copy_from_slice(&self.bits);
                lv.bits[self.width..].copy_from_slice(&other.bits);
                return lv;
            }
        }
        let mut bits = self.bits.clone();
        bits.extend_from_slice(&other.bits);
        LogicVec {
            bits,
            width: new_width,
        }
    }

    pub fn from_hex(hex_str: &str) -> Result<Self, String> {
        let hex = hex_str.trim_start_matches("0x").trim_start_matches("0X");
        let num_bits = hex.len() * 4;
        let val = u64::from_str_radix(hex, 16)
            .map_err(|e| format!("invalid hex '{}': {}", hex_str, e))?;
        Ok(LogicVec::from_u64(val, num_bits.max(1)))
    }

    pub fn from_bin(bin_str: &str) -> Result<Self, String> {
        let bin = bin_str.trim_start_matches("0b").trim_start_matches("0B");
        let num_bits = bin.len();
        let val = u64::from_str_radix(bin, 2)
            .map_err(|e| format!("invalid binary '{}': {}", bin_str, e))?;
        Ok(LogicVec::from_u64(val, num_bits.max(1)))
    }

    pub fn from_string(s: &str) -> Self {
        let bytes = s.as_bytes();
        let width = bytes.len() * 8;
        // Try arena-backed allocation first
        if let Some(ctor) = get_logicvec_ctor() {
            if let Some(mut lv) = ctor(width, LogicVal::Zero) {
                for (i, &byte) in bytes.iter().enumerate() {
                    for bit in 0..8 {
                        let bit_idx = i * 8 + bit;
                        if bit_idx < lv.width && (byte >> bit) & 1 == 1 {
                            lv.bits[bit_idx] = LogicVal::One;
                        }
                    }
                }
                return lv;
            }
        }
        // Fallback
        let mut bits = Vec::with_capacity(width);
        for &byte in bytes {
            for bit in 0..8 {
                if (byte >> bit) & 1 == 1 {
                    bits.push(LogicVal::One);
                } else {
                    bits.push(LogicVal::Zero);
                }
            }
        }
        LogicVec { bits, width }
    }

    pub fn all_x(&self) -> bool {
        self.bits.iter().all(|b| *b == LogicVal::X)
    }

    pub fn all_z(&self) -> bool {
        self.bits.iter().all(|b| *b == LogicVal::Z)
    }

    /// Ada bit X atau Z? (nilai tri-state tak diketahui). Dipakai guard jalur
    /// JIT yang hanya memodelkan nilai 0/1.
    pub fn has_unknown(&self) -> bool {
        self.bits
            .iter()
            .any(|b| *b == LogicVal::X || *b == LogicVal::Z)
    }

    pub fn casex_eq(&self, other: &LogicVec) -> bool {
        for i in 0..self.width.max(other.width) {
            let val = self.bits.get(i).copied().unwrap_or(LogicVal::Zero);
            let pat = other.bits.get(i).copied().unwrap_or(LogicVal::Zero);
            // In casex: X or Z in the pattern are don't-care (match anything)
            if pat == LogicVal::X || pat == LogicVal::Z {
                continue;
            }
            if val != pat {
                return false;
            }
        }
        true
    }

    pub fn casez_eq(&self, other: &LogicVec) -> bool {
        for i in 0..self.width.max(other.width) {
            let val = self.bits.get(i).copied().unwrap_or(LogicVal::Zero);
            let pat = other.bits.get(i).copied().unwrap_or(LogicVal::Zero);
            // In casez: Z in the pattern is don't-care (match anything)
            if pat == LogicVal::Z {
                continue;
            }
            if val != pat {
                return false;
            }
        }
        true
    }

    /// LRM 1800-2017 §12.5.1: operands case `case` dibandingkan dengan lebar
    /// operand TERLEBAR — yang lebih sempit di-zero-extend dulu. `PartialEq`
    /// (derived) membandingkan `bits` DAN `width` sehingga `6'd1` (width 32
    /// bila lebar literal hilang) tidak pernah cocok dengan signal 6-bit.
    /// Method ini membandingkan NILAI dengan zero-extension ke max width.
    pub fn case_val_eq(&self, other: &LogicVec) -> bool {
        for i in 0..self.width.max(other.width) {
            let val = self.bits.get(i).copied().unwrap_or(LogicVal::Zero);
            let pat = other.bits.get(i).copied().unwrap_or(LogicVal::Zero);
            if val != pat {
                return false;
            }
        }
        true
    }

    pub fn case_eq(&self, other: &LogicVec) -> LogicVec {
        let eq = self.bits == other.bits;
        LogicVec::from_u64(if eq { 1 } else { 0 }, 1)
    }
}

// Urutan varian = urutan discriminant: X=0, Zero=1, One=2, Z=3.
// PENTING: X HARUS 0 — alokasi lazy-zero (See `LogicVec::new`) memetakan
// halaman nol → X (value tak berinitialized di memori) — konsisten semantik.
// Serde memakai nama varian (bukan index) sehingga reorder aman.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum LogicVal {
    X,
    Zero,
    One,
    Z,
}

impl std::fmt::Display for LogicVal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogicVal::Zero => write!(f, "0"),
            LogicVal::One => write!(f, "1"),
            LogicVal::X => write!(f, "x"),
            LogicVal::Z => write!(f, "z"),
        }
    }
}

impl std::fmt::Display for LogicVec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for bit in self.bits.iter().rev() {
            write!(f, "{}", bit)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_logicval_x_is_zero_discriminant() {
        // Reorder enum: X=0 → lazy-zero page == X semantic (default init).
        assert_eq!(LogicVal::X as u8, 0);
        assert_eq!(LogicVal::Zero as u8, 1);
        assert_eq!(LogicVal::One as u8, 2);
        assert_eq!(LogicVal::Z as u8, 3);
    }

    #[test]
    fn test_lazy_zeroed_is_x() {
        let w = LogicVec::LAZY_ZERO_THRESHOLD + 4096;
        let v = LogicVec::new(w);
        assert_eq!(v.width, w);
        assert_eq!(v.bits.len(), w);
        // Sampel — semua entri harus X (zeroed semantics).
        for &i in &[0usize, 1, 12345, w / 2, w - 1024, w - 1] {
            assert_eq!(v.bits[i], LogicVal::X, "bits[{}] harus X (lazy-zero)", i);
        }
        // Tulis + baca ulang (mut path works pada lazy alloc).
        let mut v2 = v;
        v2.bits[42] = LogicVal::One;
        assert_eq!(v2.bits[42], LogicVal::One);
        assert_eq!(v2.bits[41], LogicVal::X);
        // Keseluruhan masih X untuk slot lain (spot check acak).
        assert_eq!(v2.bits[12345], LogicVal::X);
    }

    #[test]
    fn test_small_width_unchanged() {
        let v = LogicVec::new(8);
        assert!(v.bits.iter().all(|b| *b == LogicVal::X));
        let z = LogicVec::fill(LogicVal::Zero, 16);
        assert!(z.bits.iter().all(|b| *b == LogicVal::Zero));
    }
}
