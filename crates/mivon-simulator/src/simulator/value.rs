use crate::simulator::types::XPropagationMode;
use mivon_ir::{BinaryIrOp, LogicVal, LogicVec, UnaryIrOp};
use std::cell::RefCell;

thread_local! {
    /// Thread-local X-propagation mode.
    static XPROP_MODE: RefCell<XPropagationMode> = const { RefCell::new(XPropagationMode::Pessimistic) };
}

/// Set X-propagation mode for current thread.
pub fn set_xprop_mode(mode: XPropagationMode) {
    XPROP_MODE.with(|cell| *cell.borrow_mut() = mode);
}

/// Get current X-propagation mode.
pub fn get_xprop_mode() -> XPropagationMode {
    XPROP_MODE.with(|cell| *cell.borrow())
}

/// Convert LogicVec to u64 bitmasks for SIMD-style operations.
/// Returns (known_mask, value_bits) where:
/// - known_mask: bit i = 1 if bit i is known (0/1), 0 if unknown (X/Z)
/// - value_bits: actual value of known bits (only meaningful where known_mask=1)
fn to_bitmasks(lv: &LogicVec) -> (u64, u64) {
    let mut known = 0u64;
    let mut value = 0u64;
    // Defensif: bit yang hilang (bits.len() < width) dianggap X (LRM:
    // bit tak terinisialisasi = unknown). Mencegah panic index-out-of-bounds
    // di jalur eval manapun bila nilai korup (width>0, bits kosong) bocor.
    for i in 0..lv.width.min(64) {
        match lv.bits.get(i).copied().unwrap_or(LogicVal::X) {
            LogicVal::Zero => {
                known |= 1 << i;
            }
            LogicVal::One => {
                known |= 1 << i;
                value |= 1 << i;
            }
            LogicVal::X | LogicVal::Z => {}
        }
    }
    (known, value)
}

/// Convert u64 bitmasks back to LogicVec.
fn from_bitmasks(known: u64, value: u64, width: usize) -> LogicVec {
    let _w = width.min(64);
    let mut bits = Vec::with_capacity(width);
    for i in 0..width {
        if i < 64 {
            if (known >> i) & 1 == 1 {
                bits.push(if (value >> i) & 1 == 1 {
                    LogicVal::One
                } else {
                    LogicVal::Zero
                });
            } else {
                bits.push(LogicVal::X);
            }
        } else {
            bits.push(LogicVal::X); // >64 bits default to X
        }
    }
    LogicVec { bits, width }
}

/// Optimized check for X/Z using u64 bitmasks (O(1) for width ≤ 64).
fn has_xz(val: &LogicVec) -> bool {
    if val.width <= 64 {
        let (known, _) = to_bitmasks(val);
        let mask = if val.width == 64 {
            !0u64
        } else {
            (1u64 << val.width) - 1
        };
        known != mask
    } else {
        val.bits
            .iter()
            .any(|b| *b == LogicVal::X || *b == LogicVal::Z)
    }
}

/// Optimized check for any X/Z in a (known, value) pair.
#[allow(dead_code)]
fn has_xz_u64(known: u64, width: usize) -> bool {
    let mask = if width == 64 {
        !0u64
    } else {
        (1u64 << width) - 1
    };
    known != mask
}

/// Evaluate a unary operation on a logic vector
pub fn eval_unary(op: UnaryIrOp, val: &LogicVec) -> LogicVec {
    match op {
        UnaryIrOp::Plus => val.clone(),
        UnaryIrOp::Minus => {
            if val.width <= 64 && !has_xz(val) {
                // Fast path: u64 two's complement negation
                let v = val.to_u64();
                let neg = (!v).wrapping_add(1); // two's complement: ~v + 1
                LogicVec::from_u64(neg, val.width)
            } else {
                // Slow path: per-bit
                let mut result = LogicVec::new(val.width);
                for (i, b) in val.bits.iter().enumerate() {
                    result.bits[i] = match b {
                        LogicVal::Zero => LogicVal::One,
                        LogicVal::One => LogicVal::Zero,
                        LogicVal::X => LogicVal::X,
                        LogicVal::Z => LogicVal::X,
                    };
                }
                let mut carry = true;
                for b in result.bits.iter_mut() {
                    if carry {
                        match b {
                            LogicVal::Zero => {
                                *b = LogicVal::One;
                                carry = false;
                            }
                            LogicVal::One => {
                                *b = LogicVal::Zero;
                            }
                            LogicVal::X => {
                                carry = false;
                            }
                            LogicVal::Z => {
                                *b = LogicVal::X;
                                carry = false;
                            }
                        }
                    }
                }
                result
            }
        }
        UnaryIrOp::Not => {
            // Logical not: skalarisasi LRM — ada X → hasil X (dulu
            // unwrap_or(false) membuat `!x` = 1; xprop_fuzz seed=38).
            let truthy = val.to_bool();
            match truthy {
                Some(t) => LogicVec::from_u64(if t { 0 } else { 1 }, 1),
                None => LogicVec {
                    bits: vec![LogicVal::X],
                    width: 1,
                },
            }
        }
        UnaryIrOp::BitNot => {
            if val.width <= 64 {
                // Fast path: u64 bitmasks
                let (known, value) = to_bitmasks(val);
                let _mask = if val.width == 64 {
                    !0u64
                } else {
                    (1u64 << val.width) - 1
                };
                // Known bits stay known, value is inverted
                // X/Z bits remain unknown
                from_bitmasks(known, !value, val.width)
            } else {
                // Slow path: per-bit
                let mut result = LogicVec::new(val.width);
                for (i, b) in val.bits.iter().enumerate() {
                    result.bits[i] = match b {
                        LogicVal::Zero => LogicVal::One,
                        LogicVal::One => LogicVal::Zero,
                        LogicVal::X => LogicVal::X,
                        LogicVal::Z => LogicVal::X,
                    };
                }
                result
            }
        }
        UnaryIrOp::RedAnd => {
            // LRM Tabel 11-19: 0 MENDOMINASI reduction AND — satu bit 0
            // yang diketahui menghasilkan 0 walau bit lain X/Z. Dulu
            // fast path langsung X bila ada X/Z (ditemukan guided_fuzz
            // seed=111666772; emas + Icarus: 0).
            if val.width <= 64 {
                // Fast path: u64 bitmasks
                let (known, value) = to_bitmasks(val);
                let mask = if val.width == 64 {
                    !0u64
                } else {
                    (1u64 << val.width) - 1
                };
                let known_zeros = !value & known & mask;
                if known_zeros != 0 {
                    // Ada bit 0 yang diketahui → hasil pasti 0
                    LogicVec::from_u64(0, 1)
                } else if known != mask {
                    // Ada X/Z tanpa bit 0 yang diketahui → X
                    LogicVec {
                        bits: vec![LogicVal::X],
                        width: 1,
                    }
                } else {
                    // All bits known: check if any is 0
                    let all_ones = value & mask == mask;
                    LogicVec::from_u64(if all_ones { 1 } else { 0 }, 1)
                }
            } else {
                // Slow path: per-bit
                let mut result = LogicVal::One;
                for b in &val.bits {
                    match b {
                        LogicVal::Zero => {
                            result = LogicVal::Zero;
                            break;
                        }
                        LogicVal::X | LogicVal::Z => {
                            result = LogicVal::X;
                        }
                        _ => {}
                    }
                }
                LogicVec {
                    bits: vec![result],
                    width: 1,
                }
            }
        }
        UnaryIrOp::RedNand => {
            let and = eval_unary(UnaryIrOp::RedAnd, val);
            eval_unary(UnaryIrOp::BitNot, &and)
        }
        UnaryIrOp::RedOr => {
            // LRM Tabel 11-19: 1 MENDOMINASI reduction OR — satu bit 1
            // yang diketahui menghasilkan 1 walau bit lain X/Z.
            if val.width <= 64 {
                // Fast path: u64 bitmasks
                let (known, value) = to_bitmasks(val);
                let mask = if val.width == 64 {
                    !0u64
                } else {
                    (1u64 << val.width) - 1
                };
                let known_ones = value & known & mask;
                if known_ones != 0 {
                    // Ada bit 1 yang diketahui → hasil pasti 1
                    LogicVec::from_u64(1, 1)
                } else if known != mask {
                    // Ada X/Z tanpa bit 1 yang diketahui → X
                    LogicVec {
                        bits: vec![LogicVal::X],
                        width: 1,
                    }
                } else {
                    // All bits known: check if any is 1
                    let any_one = value & mask != 0;
                    LogicVec::from_u64(if any_one { 1 } else { 0 }, 1)
                }
            } else {
                // Slow path: per-bit
                let mut result = LogicVal::Zero;
                for b in &val.bits {
                    match b {
                        LogicVal::One => {
                            result = LogicVal::One;
                            break;
                        }
                        LogicVal::X | LogicVal::Z => {
                            result = LogicVal::X;
                        }
                        _ => {}
                    }
                }
                LogicVec {
                    bits: vec![result],
                    width: 1,
                }
            }
        }
        UnaryIrOp::RedNor => {
            let or = eval_unary(UnaryIrOp::RedOr, val);
            eval_unary(UnaryIrOp::BitNot, &or)
        }
        UnaryIrOp::RedXor => {
            if val.width <= 64 {
                // Fast path: u64 bitmasks
                let (known, value) = to_bitmasks(val);
                let mask = if val.width == 64 {
                    !0u64
                } else {
                    (1u64 << val.width) - 1
                };
                if known != mask {
                    // Some bits are X/Z → result is X
                    LogicVec {
                        bits: vec![LogicVal::X],
                        width: 1,
                    }
                } else {
                    // XOR of all bits: parity of 1s
                    let ones = (value & mask).count_ones();
                    LogicVec::from_u64(if ones % 2 == 1 { 1 } else { 0 }, 1)
                }
            } else {
                // Slow path: per-bit
                let mut result = LogicVal::Zero;
                for b in &val.bits {
                    match b {
                        LogicVal::One => {
                            result = match result {
                                LogicVal::Zero => LogicVal::One,
                                LogicVal::One => LogicVal::Zero,
                                LogicVal::X => LogicVal::X,
                                LogicVal::Z => LogicVal::X,
                            };
                        }
                        LogicVal::X | LogicVal::Z => {
                            result = LogicVal::X;
                        }
                        _ => {}
                    }
                }
                LogicVec {
                    bits: vec![result],
                    width: 1,
                }
            }
        }
        UnaryIrOp::RedXnor => {
            let xor = eval_unary(UnaryIrOp::RedXor, val);
            eval_unary(UnaryIrOp::BitNot, &xor)
        }
    }
}

/// Sign-extend `val` ke `width` (isi bit di atas lebar asli dengan msb).
/// `extend_to` selalu ZERO-extend — untuk operasi SIGNED (perbandingan,
/// div/mod) nilai 8-bit 0xFF harus jadi -1 (0xFFFFFFFF), bukan 255.
fn sign_extend_to(val: &LogicVec, width: usize) -> LogicVec {
    if val.width >= width {
        return val.clone();
    }
    let mut bits = val.bits.clone();
    let msb = val.bits.last().copied().unwrap_or(LogicVal::Zero);
    let fill = match msb {
        LogicVal::Zero => LogicVal::Zero,
        LogicVal::One => LogicVal::One,
        LogicVal::X | LogicVal::Z => LogicVal::X,
    };
    bits.resize(width, fill);
    LogicVec { bits, width }
}

/// Evaluate a binary operation on logic vectors
pub fn eval_binary_signed(op: BinaryIrOp, lhs: &LogicVec, rhs: &LogicVec) -> LogicVec {
    let max_width = lhs.width.max(rhs.width);
    // ROUND 36: operan SIGNED di-sign-extend dari lebar ASLI-nya (bukan
    // zero-extend) — `logic signed [7:0] s = -1; s < 0` harus -1 < 0 = true,
    // bukan 255 < 0.
    let lhs_ext = sign_extend_to(lhs, max_width);
    let rhs_ext = sign_extend_to(rhs, max_width);
    match op {
        BinaryIrOp::Add | BinaryIrOp::Sub | BinaryIrOp::Mul => {
            // Aritmetika signed: operan DUA-COMPLEMENT di-ekstensi tanda ke
            // lebar umum (LRM §11.8.1/§11.8.2). Dulu eval_binary zero-extend
            // → `-8'sd4 + 1` (8-bit + 32-bit signed) = 253 padahal -3
            // (probe q07). X → X (pessimistic).
            let l_has_x = lhs_ext
                .bits
                .iter()
                .any(|b| *b == LogicVal::X || *b == LogicVal::Z);
            let r_has_x = rhs_ext
                .bits
                .iter()
                .any(|b| *b == LogicVal::X || *b == LogicVal::Z);
            if l_has_x || r_has_x {
                return LogicVec {
                    bits: vec![LogicVal::X; max_width],
                    width: max_width,
                };
            }
            if max_width > 128 {
                return eval_binary(op, lhs, rhs);
            }
            let l = to_i128_signed(&lhs_ext);
            let r = to_i128_signed(&rhs_ext);
            let s = match op {
                BinaryIrOp::Add => l.wrapping_add(r),
                BinaryIrOp::Sub => l.wrapping_sub(r),
                _ => l.wrapping_mul(r),
            };
            if max_width > 64 {
                from_u128_wide(s as u128, max_width)
            } else {
                LogicVec::from_u64(s as u64, max_width)
            }
        }
        BinaryIrOp::Lt => {
            // >64-bit: to_i64 memotong — pakai i128 (wide_fuzz seed=17:
            // `a >= b` 65-bit salah tanda).
            let l = to_i128_signed(&lhs_ext);
            let r = to_i128_signed(&rhs_ext);
            LogicVec::from_u64(if l < r { 1 } else { 0 }, 1)
        }
        BinaryIrOp::Le => {
            let l = to_i128_signed(&lhs_ext);
            let r = to_i128_signed(&rhs_ext);
            LogicVec::from_u64(if l <= r { 1 } else { 0 }, 1)
        }
        BinaryIrOp::Gt => {
            let l = to_i128_signed(&lhs_ext);
            let r = to_i128_signed(&rhs_ext);
            LogicVec::from_u64(if l > r { 1 } else { 0 }, 1)
        }
        BinaryIrOp::Ge => {
            let l = to_i128_signed(&lhs_ext);
            let r = to_i128_signed(&rhs_ext);
            LogicVec::from_u64(if l >= r { 1 } else { 0 }, 1)
        }
        BinaryIrOp::Div => {
            let l = to_i128_signed(&lhs_ext);
            let r = to_i128_signed(&rhs_ext);
            if r == 0 {
                LogicVec {
                    bits: vec![LogicVal::X; max_width],
                    width: max_width,
                }
            } else {
                // i128 div truncates toward zero (semantik SV §11.4.3).
                let q = l.wrapping_div(r);
                if max_width > 64 && max_width <= 128 {
                    from_u128_wide(q as u128, max_width)
                } else {
                    LogicVec::from_u64(q as u64, max_width)
                }
            }
        }
        BinaryIrOp::Mod => {
            let l = to_i128_signed(&lhs_ext);
            let r = to_i128_signed(&rhs_ext);
            if r == 0 {
                LogicVec {
                    bits: vec![LogicVal::X; max_width],
                    width: max_width,
                }
            } else {
                // i128 rem truncates toward zero (tanda mengikuti dividen,
                // SV §11.4.3): -7 % 2 = -1.
                let m = l.wrapping_rem(r);
                if max_width > 64 && max_width <= 128 {
                    from_u128_wide(m as u128, max_width)
                } else {
                    LogicVec::from_u64(m as u64, max_width)
                }
            }
        }
        BinaryIrOp::Eq | BinaryIrOp::CaseEq => {
            // Perbandingan kesetaraan dua operand SIGNED: bandingkan pola
            // setelah BOTH di-sign-extend ke lebar umum. Zero-extend
            // (`eval_binary`) salah utk `int X = -2; X === -2` (64-bit
            // vs 32-bit): 0xFF..FE vs 0x00000000FFFFFFFE (probe p12f).
            let mode = get_xprop_mode();
            if op == BinaryIrOp::Eq
                && (mode == XPropagationMode::Pessimistic || mode == XPropagationMode::XAnywhere)
                && (has_xz(&lhs_ext) || has_xz(&rhs_ext))
            {
                return LogicVec {
                    bits: vec![LogicVal::X],
                    width: 1,
                };
            }
            let eq = lhs_ext.bits == rhs_ext.bits;
            LogicVec::from_u64(if eq { 1 } else { 0 }, 1)
        }
        BinaryIrOp::Neq | BinaryIrOp::CaseNeq => {
            let mode = get_xprop_mode();
            if op == BinaryIrOp::Neq
                && (mode == XPropagationMode::Pessimistic || mode == XPropagationMode::XAnywhere)
                && (has_xz(&lhs_ext) || has_xz(&rhs_ext))
            {
                return LogicVec {
                    bits: vec![LogicVal::X],
                    width: 1,
                };
            }
            let eq = lhs_ext.bits == rhs_ext.bits;
            LogicVec::from_u64(if eq { 0 } else { 1 }, 1)
        }
        _ => eval_binary(op, lhs, rhs),
    }
}

/// Konversi LogicVec → u128 (hingga 128 bit; bit di atas width = 0).
/// Dipakai jalur aritmetika lebar (>64 bit) — dulu `to_u64` memotong
/// MSB sehingga `num % -(b)` (divisor 65-bit `2^64+1` → terbaca 1)
/// salah total (ditemukan fuzzer seed=41920606, konfirmasi Icarus).
fn to_u128_wide(lv: &LogicVec) -> u128 {
    let mut v = lv.to_u64() as u128;
    for i in 64..lv.width.min(128) {
        if lv.bits.get(i).copied().unwrap_or(LogicVal::X) == LogicVal::One {
            v |= 1u128 << i;
        }
    }
    v
}

/// Konversi u128 → LogicVec selebar `width` (zero-extend / truncate).
fn from_u128_wide(val: u128, width: usize) -> LogicVec {
    let mut lv = LogicVec::new(width);
    #[allow(clippy::needless_range_loop)]
    for i in 0..width.min(128) {
        lv.bits[i] = if (val >> i) & 1 == 1 {
            LogicVal::One
        } else {
            LogicVal::Zero
        };
    }
    lv
}

/// Sign-extend LogicVec ke i128 (untuk Div/Mod signed >64-bit).
fn to_i128_signed(lv: &LogicVec) -> i128 {
    let v = to_u128_wide(lv);
    if lv.width > 0 && lv.width < 128 {
        let sign_bit = 1u128 << (lv.width - 1);
        if v & sign_bit != 0 {
            return (v | !((1u128 << lv.width) - 1)) as i128;
        }
    }
    v as i128
}

/// Bandingkan dua LogicVec sebagai pola UNSIGNED dari MSB → LSB.
/// Bit di luar lebar operan dianggap Zero. Pemanggil menjamin bebas X/Z
/// (xprop ditangani sebelum masuk sini).
fn cmp_unsigned_bits(l: &LogicVec, r: &LogicVec) -> std::cmp::Ordering {
    let w = l.width.max(r.width);
    for i in (0..w).rev() {
        let lo = l.bits.get(i).copied().unwrap_or(LogicVal::Zero) == LogicVal::One;
        let ro = r.bits.get(i).copied().unwrap_or(LogicVal::Zero) == LogicVal::One;
        if lo != ro {
            return if lo {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Less
            };
        }
    }
    std::cmp::Ordering::Equal
}

pub fn eval_binary(op: BinaryIrOp, lhs: &LogicVec, rhs: &LogicVec) -> LogicVec {
    let max_width = lhs.width.max(rhs.width);
    let lhs_ext = extend_to(lhs, max_width);
    let rhs_ext = extend_to(rhs, max_width);

    match op {
        BinaryIrOp::Add | BinaryIrOp::Sub => {
            let l_has_x = lhs_ext
                .bits
                .iter()
                .any(|b| *b == LogicVal::X || *b == LogicVal::Z);
            let r_has_x = rhs_ext
                .bits
                .iter()
                .any(|b| *b == LogicVal::X || *b == LogicVal::Z);
            if l_has_x || r_has_x {
                LogicVec {
                    bits: vec![LogicVal::X; max_width],
                    width: max_width,
                }
            } else if max_width > 64 && max_width <= 128 {
                // Jalur lebar: u128 — to_u64 memotong MSB (bug fuzzer
                // seed=41920606).
                let l = to_u128_wide(&lhs_ext);
                let r = to_u128_wide(&rhs_ext);
                let result = match op {
                    BinaryIrOp::Add => l.wrapping_add(r),
                    BinaryIrOp::Sub => l.wrapping_sub(r),
                    _ => unreachable!(),
                };
                from_u128_wide(result, max_width)
            } else {
                let l = lhs_ext.to_u64();
                let r = rhs_ext.to_u64();
                let result = match op {
                    BinaryIrOp::Add => l.wrapping_add(r),
                    BinaryIrOp::Sub => l.wrapping_sub(r),
                    _ => unreachable!(),
                };
                LogicVec::from_u64(result, max_width)
            }
        }
        BinaryIrOp::Mul => {
            let l_has_x = lhs_ext
                .bits
                .iter()
                .any(|b| *b == LogicVal::X || *b == LogicVal::Z);
            let r_has_x = rhs_ext
                .bits
                .iter()
                .any(|b| *b == LogicVal::X || *b == LogicVal::Z);
            if l_has_x || r_has_x {
                LogicVec {
                    bits: vec![LogicVal::X; max_width],
                    width: max_width,
                }
            } else if max_width > 64 && max_width <= 128 {
                let l = to_u128_wide(&lhs_ext);
                let r = to_u128_wide(&rhs_ext);
                from_u128_wide(l.wrapping_mul(r), max_width)
            } else {
                LogicVec::from_u64(lhs_ext.to_u64().wrapping_mul(rhs_ext.to_u64()), max_width)
            }
        }
        BinaryIrOp::Div => {
            let l_has_x = lhs_ext
                .bits
                .iter()
                .any(|b| *b == LogicVal::X || *b == LogicVal::Z);
            let r_has_x = rhs_ext
                .bits
                .iter()
                .any(|b| *b == LogicVal::X || *b == LogicVal::Z);
            if l_has_x || r_has_x {
                LogicVec {
                    bits: vec![LogicVal::X; max_width],
                    width: max_width,
                }
            } else if max_width > 64 && max_width <= 128 {
                let l = to_u128_wide(&lhs_ext);
                let r = to_u128_wide(&rhs_ext);
                match l.checked_div(r) {
                    Some(q) => from_u128_wide(q, max_width),
                    None => LogicVec {
                        bits: vec![LogicVal::X; max_width],
                        width: max_width,
                    },
                }
            } else {
                let l = lhs_ext.to_u64();
                let r = rhs_ext.to_u64();
                match l.checked_div(r) {
                    Some(q) => LogicVec::from_u64(q, max_width),
                    None => LogicVec {
                        bits: vec![LogicVal::X; max_width],
                        width: max_width,
                    },
                }
            }
        }
        BinaryIrOp::Mod => {
            let l_has_x = lhs_ext
                .bits
                .iter()
                .any(|b| *b == LogicVal::X || *b == LogicVal::Z);
            let r_has_x = rhs_ext
                .bits
                .iter()
                .any(|b| *b == LogicVal::X || *b == LogicVal::Z);
            if l_has_x || r_has_x {
                LogicVec {
                    bits: vec![LogicVal::X; max_width],
                    width: max_width,
                }
            } else if max_width > 64 && max_width <= 128 {
                let l = to_u128_wide(&lhs_ext);
                let r = to_u128_wide(&rhs_ext);
                if r == 0 {
                    LogicVec {
                        bits: vec![LogicVal::X; max_width],
                        width: max_width,
                    }
                } else {
                    from_u128_wide(l.wrapping_rem(r), max_width)
                }
            } else {
                let l = lhs_ext.to_u64();
                let r = rhs_ext.to_u64();
                if r == 0 {
                    LogicVec {
                        bits: vec![LogicVal::X; max_width],
                        width: max_width,
                    }
                } else {
                    LogicVec::from_u64(l % r, max_width)
                }
            }
        }
        BinaryIrOp::Power => {
            // LRM §11.6.1 Tabel 11-21: `i ** j` → lebar hasil = L(i) =
            // lebar operand KIRI, sama seperti shift. Amount `j` bersifat
            // self-determined dan TIDAK melebarkan hasil.
            //
            // Bug ditemukan sweep differential vs iverilog -g2012
            // (bug mivon #3 dari fuzzer):
            //     a = 8'h11 (17); a ** 2  →  mivon 289   iverilog 33
            //     a = 8'h02;      a ** 10 →  mivon 1024  iverilog 0
            //     a = -3;         a ** 2  →  mivon 64009 iverilog 9
            // Penyebab: `max_width = lhs.width.max(rhs.width)` dan operand
            // kanan berupa literal unsized `2` yang lebarnya 32 bit →
            // mask ke 32 bit, hasil 289 lolos penuh. LRM: truncate ke 8 bit
            // → 289 & 0xff = 33.
            //
            // `result_width` (bukan `max_width`) dipakai untuk mask DAN
            // lebar keluaran, konsisten dengan lengan `Sshr` di bawah.
            let result_width = lhs.width.max(1);
            let l_has_x = lhs_ext
                .bits
                .iter()
                .any(|b| *b == LogicVal::X || *b == LogicVal::Z);
            let r_has_x = rhs_ext
                .bits
                .iter()
                .any(|b| *b == LogicVal::X || *b == LogicVal::Z);
            if l_has_x || r_has_x {
                LogicVec {
                    bits: vec![LogicVal::X; result_width],
                    width: result_width,
                }
            } else if result_width <= 128 {
                // Square-and-multiply pada u128 — operand SV bisa >64-bit;
                // dulu jatuh ke jalur u64 yang memotong pola 96/128-bit
                // (ditemukan guided_fuzz seed=79912420; emas + Icarus).
                let exp = to_u128_wide(&rhs_ext);
                let m: u128 = if result_width >= 128 {
                    u128::MAX
                } else {
                    (1u128 << result_width) - 1
                };
                let mut base = to_u128_wide(&lhs_ext) & m;
                let mut acc: u128 = 1 & m;
                let mut e = exp;
                while e > 0 {
                    if e & 1 == 1 {
                        acc = acc.wrapping_mul(base) & m;
                    }
                    e >>= 1;
                    if e > 0 {
                        base = base.wrapping_mul(base) & m;
                    }
                }
                from_u128_wide(acc, result_width)
            } else {
                // >128 bit: square-and-multiply generik pada bit LogicVal
                // (perkalian shift-add mod 2^result_width, tanpa X — X sudah
                // ditangani di atas).
                let w = result_width;
                let zero = || vec![LogicVal::Zero; w];
                let mut acc = {
                    let mut v = zero();
                    v[0] = LogicVal::One;
                    v
                };
                let mut base = lhs_ext.bits.clone();
                base.resize(w, LogicVal::Zero);
                let exp_bits: Vec<LogicVal> = rhs_ext.bits.iter().copied().take(w).collect();
                let add_mod = |dst: &mut Vec<LogicVal>, src: &[LogicVal]| {
                    // Operan bebas X (X ditangani di atas) → adder biner murni.
                    let mut carry = false;
                    for i in 0..w {
                        let sum = (dst[i] == LogicVal::One) as u8
                            + (src[i] == LogicVal::One) as u8
                            + u8::from(carry);
                        dst[i] = if sum & 1 == 1 {
                            LogicVal::One
                        } else {
                            LogicVal::Zero
                        };
                        carry = sum >= 2;
                    }
                };
                for i in 0..w {
                    if exp_bits[i] == LogicVal::One {
                        // acc *= base (shifted by i)
                        let mut shifted = vec![LogicVal::Zero; w];
                        shifted[i..w].copy_from_slice(&base[..w - i]);
                        add_mod(&mut acc, &shifted);
                    }
                    // base *= base (square) — hanya perlu bila masih ada
                    // bit eksponen lebih tinggi.
                    if i + 1 < w {
                        let old = base.clone();
                        let mut sq = vec![LogicVal::Zero; w];
                        for j in 0..w {
                            if old[j] == LogicVal::One {
                                let mut shifted = vec![LogicVal::Zero; w];
                                shifted[j..w].copy_from_slice(&old[..w - j]);
                                add_mod(&mut sq, &shifted);
                            }
                        }
                        base = sq;
                    }
                }
                LogicVec {
                    bits: acc,
                    width: w,
                }
            }
        }
        BinaryIrOp::Eq | BinaryIrOp::CaseEq => {
            // `===` (CaseEq) membandingkan pola LITERAL termasuk X/Z
            // (LRM §11.4.5) — TIDAK boleh X-pessimistic. Hanya `==`
            // (Eq) yang mengikuti mode xprop.
            let mode = get_xprop_mode();
            if op == BinaryIrOp::Eq
                && (mode == XPropagationMode::Pessimistic || mode == XPropagationMode::XAnywhere)
                && (has_xz(&lhs_ext) || has_xz(&rhs_ext))
            {
                return LogicVec {
                    bits: vec![LogicVal::X],
                    width: 1,
                };
            }
            let eq = if max_width <= 64 {
                // Fast path: O(1) u64 bitmask comparison
                let (lk, lv) = to_bitmasks(&lhs_ext);
                let (rk, rv) = to_bitmasks(&rhs_ext);
                lk == rk && lv == rv
            } else {
                lhs_ext.bits == rhs_ext.bits
            };
            LogicVec::from_u64(if eq { 1 } else { 0 }, 1)
        }
        BinaryIrOp::Neq | BinaryIrOp::CaseNeq => {
            // `!==` (CaseNeq) = negasi pola literal termasuk X/Z — TIDAK
            // boleh X-pessimistic (`x !== 0` = TRUE, LRM §11.4.5). Dulu
            // digabung dgn `!=` → hasil X (ditemukan guided_fuzz
            // seed=4655880324794; emas + Icarus: 1).
            let mode = get_xprop_mode();
            if op == BinaryIrOp::Neq
                && (mode == XPropagationMode::Pessimistic || mode == XPropagationMode::XAnywhere)
                && (has_xz(&lhs_ext) || has_xz(&rhs_ext))
            {
                return LogicVec {
                    bits: vec![LogicVal::X],
                    width: 1,
                };
            }
            let eq = if max_width <= 64 {
                // Fast path: O(1) u64 bitmask comparison
                let (lk, lv) = to_bitmasks(&lhs_ext);
                let (rk, rv) = to_bitmasks(&rhs_ext);
                lk == rk && lv == rv
            } else {
                lhs_ext.bits == rhs_ext.bits
            };
            LogicVec::from_u64(if eq { 0 } else { 1 }, 1)
        }
        BinaryIrOp::EqWild => {
            let eq = lhs_ext.casex_eq(&rhs_ext);
            LogicVec::from_u64(if eq { 1 } else { 0 }, 1)
        }
        BinaryIrOp::NeqWild => {
            let eq = lhs_ext.casex_eq(&rhs_ext);
            LogicVec::from_u64(if eq { 0 } else { 1 }, 1)
        }
        BinaryIrOp::Lt => {
            let mode = get_xprop_mode();
            if (mode == XPropagationMode::Pessimistic || mode == XPropagationMode::XAnywhere)
                && (has_xz(&lhs_ext) || has_xz(&rhs_ext))
            {
                return LogicVec {
                    bits: vec![LogicVal::X],
                    width: 1,
                };
            }
            // >64-bit: to_u64 memotong MSB → perbandingan salah
            // (ditemukan guided_fuzz seed=79912420; emas + Icarus).
            let lt = if max_width <= 64 {
                lhs_ext.to_u64() < rhs_ext.to_u64()
            } else {
                cmp_unsigned_bits(&lhs_ext, &rhs_ext) == std::cmp::Ordering::Less
            };
            LogicVec::from_u64(if lt { 1 } else { 0 }, 1)
        }
        BinaryIrOp::Le => {
            let mode = get_xprop_mode();
            if (mode == XPropagationMode::Pessimistic || mode == XPropagationMode::XAnywhere)
                && (has_xz(&lhs_ext) || has_xz(&rhs_ext))
            {
                return LogicVec {
                    bits: vec![LogicVal::X],
                    width: 1,
                };
            }
            let le = if max_width <= 64 {
                lhs_ext.to_u64() <= rhs_ext.to_u64()
            } else {
                cmp_unsigned_bits(&lhs_ext, &rhs_ext) != std::cmp::Ordering::Greater
            };
            LogicVec::from_u64(if le { 1 } else { 0 }, 1)
        }
        BinaryIrOp::Gt => {
            let mode = get_xprop_mode();
            if (mode == XPropagationMode::Pessimistic || mode == XPropagationMode::XAnywhere)
                && (has_xz(&lhs_ext) || has_xz(&rhs_ext))
            {
                return LogicVec {
                    bits: vec![LogicVal::X],
                    width: 1,
                };
            }
            let gt = if max_width <= 64 {
                lhs_ext.to_u64() > rhs_ext.to_u64()
            } else {
                cmp_unsigned_bits(&lhs_ext, &rhs_ext) == std::cmp::Ordering::Greater
            };
            LogicVec::from_u64(if gt { 1 } else { 0 }, 1)
        }
        BinaryIrOp::Ge => {
            let mode = get_xprop_mode();
            if (mode == XPropagationMode::Pessimistic || mode == XPropagationMode::XAnywhere)
                && (has_xz(&lhs_ext) || has_xz(&rhs_ext))
            {
                return LogicVec {
                    bits: vec![LogicVal::X],
                    width: 1,
                };
            }
            let ge = if max_width <= 64 {
                lhs_ext.to_u64() >= rhs_ext.to_u64()
            } else {
                cmp_unsigned_bits(&lhs_ext, &rhs_ext) != std::cmp::Ordering::Less
            };
            LogicVec::from_u64(if ge { 1 } else { 0 }, 1)
        }
        BinaryIrOp::BitAnd => {
            let mode = get_xprop_mode();
            if mode == XPropagationMode::Optimistic {
                // Optimistic: 0 & X = 0 (any known bit forces known output)
                bitwise_op(&lhs_ext, &rhs_ext, |a, b| match (a, b) {
                    (LogicVal::Zero, _) | (_, LogicVal::Zero) => LogicVal::Zero,
                    (LogicVal::One, LogicVal::One) => LogicVal::One,
                    _ => LogicVal::X,
                })
            } else {
                // Pessimistic: X menular TAPI identitas dominasi TETAP LRM
                // §11.4.1 Tabel 11-20: `0 & x = 0`, `0 & z = 0` (koreksi Z
                // lama utk fuZZ; koreksi X — sebelumnya `0 & X = X`
                // melanggar LRM → X menular tak terbatas, mis. sel mux
                // prim_max_tree tak pernah konvergen thd input tak-ter-init).
                // X di posisi NON-identitas tetap pessimistis (policy SIM-11:
                // `1 & X = X`, `x & z = X`).
                bitwise_op(&lhs_ext, &rhs_ext, |a, b| match (a, b) {
                    (LogicVal::Zero, LogicVal::X)
                    | (LogicVal::X, LogicVal::Zero)
                    | (LogicVal::Zero, LogicVal::Z)
                    | (LogicVal::Z, LogicVal::Zero) => LogicVal::Zero,
                    (LogicVal::X, _) | (_, LogicVal::X) | (LogicVal::Z, _) | (_, LogicVal::Z) => {
                        LogicVal::X
                    }
                    (LogicVal::One, LogicVal::One) => LogicVal::One,
                    _ => LogicVal::Zero,
                })
            }
        }
        BinaryIrOp::BitOr => {
            let mode = get_xprop_mode();
            if mode == XPropagationMode::Optimistic {
                // Optimistic: 1 | X = 1
                bitwise_op(&lhs_ext, &rhs_ext, |a, b| match (a, b) {
                    (LogicVal::One, _) | (_, LogicVal::One) => LogicVal::One,
                    (LogicVal::Zero, LogicVal::Zero) => LogicVal::Zero,
                    _ => LogicVal::X,
                })
            } else {
                // Pessimistic: X menular TAPI identitas dominasi TETAP LRM
                // §11.4.1 Tabel 11-21: `1 | x = 1`, `1 | z = 1`. X non-
                // identitas tetap pessimistis (SIM-11: `0 | X = X` — juga
                // LRM: tabel `|` memang `0|x=x`).
                bitwise_op(&lhs_ext, &rhs_ext, |a, b| match (a, b) {
                    (LogicVal::One, LogicVal::X)
                    | (LogicVal::X, LogicVal::One)
                    | (LogicVal::One, LogicVal::Z)
                    | (LogicVal::Z, LogicVal::One) => LogicVal::One,
                    (LogicVal::X, _) | (_, LogicVal::X) | (LogicVal::Z, _) | (_, LogicVal::Z) => {
                        LogicVal::X
                    }
                    (LogicVal::One, _) | (_, LogicVal::One) => LogicVal::One,
                    _ => LogicVal::Zero,
                })
            }
        }
        BinaryIrOp::BitXor => bitwise_op(&lhs_ext, &rhs_ext, |a, b| match (a, b) {
            (LogicVal::Zero, LogicVal::Zero) => LogicVal::Zero,
            (LogicVal::One, LogicVal::One) => LogicVal::Zero,
            (LogicVal::Zero, LogicVal::One) => LogicVal::One,
            (LogicVal::One, LogicVal::Zero) => LogicVal::One,
            _ => LogicVal::X,
        }),
        BinaryIrOp::BitXnor => {
            let xor = eval_binary(BinaryIrOp::BitXor, lhs, rhs);
            eval_unary(UnaryIrOp::BitNot, &xor)
        }
        BinaryIrOp::Shl => {
            let shift = rhs_ext.to_u64() as usize;
            let result_width = lhs.width; // LRM §11.8.1: hasil shift = lebar operan kiri; shift amount self-determined TIDAK melebarkan hasil (ekstensi konteks assignment ditangani evaluate_expr_ctx / propagate_context_width — xprop_fuzz seed=39: `(xvec <<< 8)` harus all-zero, emas + Icarus).
                                          // If shift amount has X/Z, result is unknown
            if has_xz(&rhs_ext) {
                return LogicVec {
                    bits: vec![LogicVal::X; result_width],
                    width: result_width,
                };
            }
            if result_width <= 64 && !has_xz(lhs) {
                // Fast path: u64 shift
                let val = lhs.to_u64();
                let shifted = if shift >= result_width {
                    0
                } else {
                    val << shift
                };
                LogicVec::from_u64(shifted, result_width)
            } else {
                // Slow path: per-bit — zero-fill; bit X/Z bergeser normal
                // dan terbuang bila melewati batas (LRM §11.4.10; sama dgn
                // fix Shr — xprop_fuzz seed=39: `(xvec <<< 8) == 0` harus
                // TRUE, bukan X).
                let mut result = lhs.clone();
                if result_width > lhs.width {
                    // Operan kiri di-zero-extend ke result_width (LRM
                    // §11.8.1 context-determined) — tanpa ini indexing
                    // result.bits OOB (ditemukan fuzzer seed=668917811772).
                    result.bits.extend(std::iter::repeat_n(
                        LogicVal::Zero,
                        result_width - lhs.width,
                    ));
                    result.width = result_width;
                }
                if shift > 0 && shift < result_width {
                    for i in (shift..result_width).rev() {
                        // Sumber dibatasi lebar asli lhs (zero-extension
                        // context-determined).
                        result.bits[i] = if i - shift < lhs.width {
                            lhs.bits[i - shift]
                        } else {
                            LogicVal::Zero
                        };
                    }
                    result.bits[..shift].fill(LogicVal::Zero);
                } else if shift >= result_width {
                    // Shift by >= width -> all zeros
                    for bit in result.bits.iter_mut() {
                        *bit = LogicVal::Zero;
                    }
                }
                result
            }
        }
        BinaryIrOp::Shr => {
            let shift = rhs_ext.to_u64() as usize;
            let result_width = lhs.width; // LRM §11.8.1: hasil shift = lebar operan kiri; shift amount self-determined TIDAK melebarkan hasil (ekstensi konteks assignment ditangani evaluate_expr_ctx / propagate_context_width — xprop_fuzz seed=39: `(xvec <<< 8)` harus all-zero, emas + Icarus).
                                          // If shift amount has X/Z, result is unknown
            if has_xz(&rhs_ext) {
                return LogicVec {
                    bits: vec![LogicVal::X; result_width],
                    width: result_width,
                };
            }
            if result_width <= 64 && !has_xz(lhs) {
                // Fast path: u64 shift
                let val = lhs.to_u64();
                let shifted = if shift >= result_width {
                    0
                } else {
                    val >> shift
                };
                LogicVec::from_u64(shifted, result_width)
            } else {
                // Slow path: per-bit — zero-fill logis (LRM §11.4.10): bit
                // X/Z yang bergeser melewati LSB TERBUANG, yang tersisa
                // tetap X di posisi barunya, sisanya zero-fill. Dulu
                // `has_xz(lhs) → all-X` tanpa melihat jumlah shift —
                // shift ≥ lebar harus menghasilkan 0 murni walau operan
                // ber-X (ditemukan guided_fuzz seed=72459868; emas +
                // Icarus: 0).
                let mut result = lhs.clone();
                if result_width > lhs.width {
                    // Operan kiri di-zero-extend ke result_width (LRM
                    // §11.8.1 context-determined) — tanpa ini indexing
                    // result.bits OOB (ditemukan fuzzer seed=668917811772).
                    result.bits.extend(std::iter::repeat_n(
                        LogicVal::Zero,
                        result_width - lhs.width,
                    ));
                    result.width = result_width;
                }
                if shift > 0 && shift < result_width {
                    for i in 0..(result_width - shift) {
                        // Sumber dibatasi lebar asli lhs; sisanya zero-fill
                        // (zero-extension context-determined).
                        result.bits[i] = if i + shift < lhs.width {
                            lhs.bits[i + shift]
                        } else {
                            LogicVal::Zero
                        };
                    }
                    result.bits[result_width - shift..].fill(LogicVal::Zero);
                } else if shift >= result_width {
                    // Shift by >= width -> all zeros
                    for bit in result.bits.iter_mut() {
                        *bit = LogicVal::Zero;
                    }
                }
                result
            }
        }
        BinaryIrOp::Sshl => {
            let shift = rhs_ext.to_u64() as usize;
            let result_width = lhs.width; // LRM §11.8.1: hasil shift = lebar operan kiri; shift amount self-determined TIDAK melebarkan hasil (ekstensi konteks assignment ditangani evaluate_expr_ctx / propagate_context_width — xprop_fuzz seed=39: `(xvec <<< 8)` harus all-zero, emas + Icarus).
                                          // If shift amount has X/Z, result is unknown
            if has_xz(&rhs_ext) {
                return LogicVec {
                    bits: vec![LogicVal::X; result_width],
                    width: result_width,
                };
            }
            if result_width <= 64 && !has_xz(lhs) {
                // Arithmetic shift left is same as logical shift left
                let val = lhs.to_u64();
                let shifted = if shift >= result_width {
                    0
                } else {
                    val << shift
                };
                LogicVec::from_u64(shifted, result_width)
            } else {
                // Slow path: per-bit
                // Shift kiri LOGIS (Sshl ≡ Shl aritmetika): zero-fill —
                // bit X/Z yang tergeser keluar MSB terbuang, yang tersisa
                // tetap X di posisinya. Dulu `has_xz(lhs) → all-X` tanpa
                // melihat jumlah shift (LRM §11.4.10; guided_fuzz
                // seed=72459868; emas + Icarus: shift ≥ lebar = 0).
                let _msb = lhs.bits.last().copied().unwrap_or(LogicVal::Zero);
                let mut result = lhs.clone();
                if result_width > lhs.width {
                    // Operan kiri di-zero-extend ke result_width (LRM
                    // §11.8.1 context-determined) — tanpa ini indexing
                    // result.bits OOB (ditemukan fuzzer seed=668917811772).
                    result.bits.extend(std::iter::repeat_n(
                        LogicVal::Zero,
                        result_width - lhs.width,
                    ));
                    result.width = result_width;
                }
                if shift > 0 && shift < result_width {
                    for _ in 0..shift {
                        for i in (1..result_width).rev() {
                            result.bits[i] = result.bits[i - 1];
                        }
                        result.bits[0] = LogicVal::Zero;
                    }
                } else if shift >= result_width {
                    // Shift by >= width -> all zeros
                    for bit in result.bits.iter_mut() {
                        *bit = LogicVal::Zero;
                    }
                }
                result
            }
        }
        BinaryIrOp::Sshr => {
            let shift = rhs_ext.to_u64() as usize;
            let result_width = lhs.width; // LRM §11.8.1: hasil shift = lebar operan kiri; shift amount self-determined TIDAK melebarkan hasil (ekstensi konteks assignment ditangani evaluate_expr_ctx / propagate_context_width — xprop_fuzz seed=39: `(xvec <<< 8)` harus all-zero, emas + Icarus).
                                          // If shift amount has X/Z, result is unknown
            if has_xz(&rhs_ext) {
                return LogicVec {
                    bits: vec![LogicVal::X; result_width],
                    width: result_width,
                };
            }
            if result_width <= 64 && !has_xz(lhs) {
                // Arithmetic shift right: extend sign bit
                let val = lhs.to_u64();
                let sign_bit = (val >> (result_width - 1)) & 1;
                let shifted = if shift >= result_width {
                    if sign_bit == 1 {
                        !0u64
                    } else {
                        0
                    }
                } else {
                    let shifted_val = val >> shift;
                    if sign_bit == 1 {
                        // Fill high bits with 1s for arithmetic shift
                        shifted_val | (!0u64 << (result_width - shift))
                    } else {
                        shifted_val
                    }
                };
                LogicVec::from_u64(shifted, result_width)
            } else {
                // Slow path: per-bit
                // If lhs has X/Z, shift result is X
                if has_xz(lhs) {
                    return LogicVec {
                        bits: vec![LogicVal::X; result_width],
                        width: result_width,
                    };
                }
                let msb = lhs.bits.last().copied().unwrap_or(LogicVal::Zero);
                let mut result = lhs.clone();
                if result_width > lhs.width {
                    // Operan kiri di-zero-extend ke result_width (LRM
                    // §11.8.1 context-determined) — tanpa ini indexing
                    // result.bits OOB (ditemukan fuzzer seed=668917811772).
                    result.bits.extend(std::iter::repeat_n(
                        LogicVal::Zero,
                        result_width - lhs.width,
                    ));
                    result.width = result_width;
                }
                if shift > 0 && shift < result_width {
                    for _ in 0..shift {
                        for i in 0..(result_width - 1) {
                            result.bits[i] = result.bits[i + 1];
                        }
                        *result.bits.last_mut().unwrap() = msb;
                    }
                } else if shift >= result_width {
                    // Shift by >= width -> fill with sign bit
                    for bit in result.bits.iter_mut() {
                        *bit = msb;
                    }
                }
                result
            }
        }
        BinaryIrOp::LogicalAnd => {
            // Skalarisasi LRM §11.4.4: 0-dominan, ada X → X (dulu
            // unwrap_or(false) memperlakukan X sebagai FALSE —
            // `x && 1` harus x, bukan 0; ditemukan xprop_fuzz seed=0).
            let l = lhs.to_bool();
            let r = rhs.to_bool();
            let v = match (l, r) {
                (Some(false), _) | (_, Some(false)) => LogicVal::Zero,
                (Some(true), Some(true)) => LogicVal::One,
                _ => LogicVal::X,
            };
            LogicVec {
                bits: vec![v],
                width: 1,
            }
        }
        BinaryIrOp::LogicalOr => {
            // 1-dominan; dua-duanya diketahui-false → 0; selain itu X.
            let l = lhs.to_bool();
            let r = rhs.to_bool();
            let v = match (l, r) {
                (Some(true), _) | (_, Some(true)) => LogicVal::One,
                (Some(false), Some(false)) => LogicVal::Zero,
                _ => LogicVal::X,
            };
            LogicVec {
                bits: vec![v],
                width: 1,
            }
        }
    }
}

fn extend_to(val: &LogicVec, width: usize) -> LogicVec {
    if val.width >= width {
        val.clone()
    } else {
        // IEEE 1800-2017 §11.8.2: operand UNSIGNED di-extend ZERO-fill —
        // implementasi lama mengisi bit ekstensi dgn X bila msb X/Z sehingga
        // `1'bx | 2'b00` → `xx` (salah; harus `0x`) dan jalur non-packed
        // beda dari packed (`PackedLogicVec::resize` zero-fill) → differential
        // O5 (temuan fuzzer bug_0793, prim_intr_hw). Untuk msb 0/1 pun
        // fungsi ini memang zero-extend (lihat komentar eval_sshr_signed).
        // Sign-extend tetap lewat `sign_extend_to` (jalur aritmetika signed).
        let mut bits = val.bits.clone();
        bits.resize(width, LogicVal::Zero);
        LogicVec { bits, width }
    }
}

/// Arithmetic shift right pada nilai ber-LEBAR ASLINYA (bukan max_width
/// operand). ROUND 36: `a >>> b` (IEEE 1800 §11.4.10) arithmetic bila `a`
/// SIGNED, logical bila unsigned. `extend_to` selalu zero-extend — Sshr lama
/// menghitung sign_bit dari nilai yang sudah di-extend ke max_width sehingga
/// `logic signed [7:0] s = -128; s >>> 2` = 0x20 (harusnya 0xE0). Di sini
/// sign bit diambil dari msb lebar asli lhs; hasil berlebar lhs.width.
pub fn eval_sshr_signed(lhs: &LogicVec, rhs: &LogicVec) -> LogicVec {
    let shift = rhs.to_u64() as usize;
    let lw = lhs.width;
    if lw == 0 {
        return LogicVec::new(0);
    }
    let has_xz = lhs
        .bits
        .iter()
        .any(|b| matches!(b, LogicVal::X | LogicVal::Z));
    if lw <= 64 && !has_xz {
        let val = lhs.to_u64();
        let sign_bit = (val >> (lw - 1)) & 1;
        let shifted = if shift >= 64 {
            if sign_bit == 1 {
                !0u64
            } else {
                0
            }
        } else {
            let sv = val >> shift;
            if sign_bit == 1 {
                // Fill ones dari posisi (lw-shift) ke atas; guard anti
                // shift-overflow (lw=64 & shift=0 dulu panic — signed_fuzz
                // w=64).
                let fill_start = lw.saturating_sub(shift).min(64);
                let fill = match fill_start {
                    0 => !0u64,
                    64 => 0u64,
                    n => (!0u64) << n,
                };
                sv | fill
            } else {
                sv
            }
        };
        LogicVec::from_u64(shifted, lw)
    } else {
        // Slow path per-bit dari lebar asli (sign bit = msb asli).
        let msb = lhs.bits.last().copied().unwrap_or(LogicVal::Zero);
        let mut result = lhs.clone();
        for _ in 0..shift.min(result.width) {
            for i in 0..(result.width - 1) {
                result.bits[i] = result.bits[i + 1];
            }
            *result.bits.last_mut().unwrap() = msb;
        }
        result
    }
}

/// Optimized bitwise operation using u64 bitmasks for width ≤ 64.
/// Falls back to per-bit loop for larger widths.
fn bitwise_op<F>(lhs: &LogicVec, rhs: &LogicVec, op: F) -> LogicVec
where
    F: Fn(LogicVal, LogicVal) -> LogicVal,
{
    let width = lhs.width.max(rhs.width);

    // Fast path: use u64 bitmasks for ≤ 64 bits
    if width <= 64 {
        let mask = if width == 64 {
            !0u64
        } else {
            (1u64 << width) - 1
        };

        // Compute per-bit results using u64 ops
        let mut result_known = 0u64;
        let mut result_value = 0u64;

        // BUG FIX (fuZZ): ambil nilai bit ASLI dari `bits` (4-state penuh)
        // — versi lama memakai bitmasks (known/value) yang meratakan Z→X,
        // sehingga closure tak pernah membedakan Z (0&Z=0, 1|Z=1 —
        // tabel LRM) di jalur fast path. Slow path (>64 bit) sudah benar;
        // bit di luar lebar operand = zero-extension (LRM §11.8.1).
        for i in 0..width {
            let l_val = lhs.bits.get(i).copied().unwrap_or(LogicVal::Zero);
            let r_val = rhs.bits.get(i).copied().unwrap_or(LogicVal::Zero);
            match op(l_val, r_val) {
                LogicVal::Zero => {
                    result_known |= 1 << i;
                }
                LogicVal::One => {
                    result_known |= 1 << i;
                    result_value |= 1 << i;
                }
                LogicVal::X | LogicVal::Z => {}
            }
        }

        return from_bitmasks(result_known & mask, result_value & mask, width);
    }

    // Slow path: per-bit loop for > 64 bits
    let mut bits = Vec::with_capacity(width);
    for i in 0..width {
        let l = lhs.bits.get(i).copied().unwrap_or(LogicVal::Zero);
        let r = rhs.bits.get(i).copied().unwrap_or(LogicVal::Zero);
        bits.push(op(l, r));
    }
    LogicVec { bits, width }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_logicvec_from_u64() {
        let v = LogicVec::from_u64(0b1010, 4);
        assert_eq!(v.to_u64(), 0b1010);
        assert_eq!(format!("{}", v), "1010");
    }

    #[test]
    fn test_bitwise_and() {
        let a = LogicVec::from_u64(0b1100, 4);
        let b = LogicVec::from_u64(0b1010, 4);
        let r = eval_binary(BinaryIrOp::BitAnd, &a, &b);
        assert_eq!(r.to_u64(), 0b1000);
    }

    #[test]
    fn test_bitwise_or() {
        let a = LogicVec::from_u64(0b1100, 4);
        let b = LogicVec::from_u64(0b1010, 4);
        let r = eval_binary(BinaryIrOp::BitOr, &a, &b);
        assert_eq!(r.to_u64(), 0b1110);
    }

    #[test]
    fn test_add() {
        let a = LogicVec::from_u64(5, 8);
        let b = LogicVec::from_u64(3, 8);
        let r = eval_binary(BinaryIrOp::Add, &a, &b);
        assert_eq!(r.to_u64(), 8);
    }

    #[test]
    fn test_bit_not() {
        let a = LogicVec::from_u64(0b1010, 4);
        let r = eval_unary(UnaryIrOp::BitNot, &a);
        assert_eq!(r.to_u64(), 0b0101);
    }

    #[test]
    fn test_logical_not() {
        let a = LogicVec::from_u64(0, 1);
        let r = eval_unary(UnaryIrOp::Not, &a);
        assert_eq!(r.to_u64(), 1);
    }

    // ─── SIM-11: X propagation mode ────────────────────────────────────
    // XPropagationMode menentukan perilaku operator thd X: Optimistic
    // (0 & X = 0, 1 | X = 1), Pessimistic (0 & X = X). Mode thread-local —
    // restore ke default agar test lain tidak terpengaruh.
    fn x_vec() -> LogicVec {
        LogicVec {
            bits: vec![LogicVal::X],
            width: 1,
        }
    }

    #[test]
    fn test_xprop_optimistic_bitand_masks_x() {
        let prev = get_xprop_mode();
        set_xprop_mode(XPropagationMode::Optimistic);
        let r = eval_binary(BinaryIrOp::BitAnd, &LogicVec::from_u64(0, 1), &x_vec());
        set_xprop_mode(prev);
        assert_eq!(r.bits[0], LogicVal::Zero, "optimistic: 0 & X harus 0");
    }

    #[test]
    fn test_xprop_pessimistic_bitand_zero_dominates_x() {
        // LRM §11.4.1 Tabel 11-20: `0 & x = 0` — identitas dominasi WAJIB
        // (dulu `0 & X = X` melanggar LRM → X menular tak terbatas thd
        // input tak-ter-init; fuzzer xmode/pmt). X non-identitas tetap
        // pessimistis: `1 & X = X` (lihat bawah).
        let prev = get_xprop_mode();
        set_xprop_mode(XPropagationMode::Pessimistic);
        let r = eval_binary(BinaryIrOp::BitAnd, &LogicVec::from_u64(0, 1), &x_vec());
        let r1 = eval_binary(BinaryIrOp::BitAnd, &LogicVec::from_u64(1, 1), &x_vec());
        set_xprop_mode(prev);
        assert_eq!(r.bits[0], LogicVal::Zero, "LRM: 0 & x harus 0");
        assert_eq!(
            r1.bits[0],
            LogicVal::X,
            "pessimistic: 1 & X tetap X (non-identitas)"
        );
    }

    #[test]
    fn test_xprop_optimistic_bitor_masks_x() {
        let prev = get_xprop_mode();
        set_xprop_mode(XPropagationMode::Optimistic);
        let r = eval_binary(BinaryIrOp::BitOr, &LogicVec::from_u64(1, 1), &x_vec());
        set_xprop_mode(prev);
        assert_eq!(r.bits[0], LogicVal::One, "optimistic: 1 | X harus 1");
    }

    #[test]
    fn test_xprop_pessimistic_bitor_one_dominates_x() {
        // LRM §11.4.1 Tabel 11-21: `1 | x = 1` — identitas dominasi WAJIB
        // (dulu `1 | X = X` melanggar LRM). X non-identitas tetap
        // pessimistis: `0 | X = X` (juga LRM — tabel `|` `0|x=x`).
        let prev = get_xprop_mode();
        set_xprop_mode(XPropagationMode::Pessimistic);
        let r = eval_binary(BinaryIrOp::BitOr, &LogicVec::from_u64(1, 1), &x_vec());
        let r0 = eval_binary(BinaryIrOp::BitOr, &LogicVec::from_u64(0, 1), &x_vec());
        set_xprop_mode(prev);
        assert_eq!(r.bits[0], LogicVal::One, "LRM: 1 | x harus 1");
        assert_eq!(r0.bits[0], LogicVal::X, "pessimistic/LRM: 0 | X = X");
    }

    fn z_vec() -> LogicVec {
        LogicVec {
            bits: vec![LogicVal::Z],
            width: 1,
        }
    }

    #[test]
    fn test_xprop_pessimistic_bitand_z_dominated() {
        // BUG FIX (fuZZ): `0 & Z = 0` sesuai tabel LRM 4-state — Z tidak
        // boleh mengubah hasil AND dgn 0. Jalur packed sudah benar; jalur
        // nilai (value.rs) sebelumnya memberi X di mode pessimistic.
        let prev = get_xprop_mode();
        set_xprop_mode(XPropagationMode::Pessimistic);
        let r0 = eval_binary(BinaryIrOp::BitAnd, &LogicVec::from_u64(0, 1), &z_vec());
        let r0r = eval_binary(BinaryIrOp::BitAnd, &z_vec(), &LogicVec::from_u64(0, 1));
        // 1 & Z tetap X (tidak didominasi).
        let r1 = eval_binary(BinaryIrOp::BitAnd, &LogicVec::from_u64(1, 1), &z_vec());
        set_xprop_mode(prev);
        assert_eq!(r0.bits[0], LogicVal::Zero, "pessimistic: 0 & Z harus 0");
        assert_eq!(r0r.bits[0], LogicVal::Zero, "pessimistic: Z & 0 harus 0");
        assert_eq!(r1.bits[0], LogicVal::X, "pessimistic: 1 & Z tetap X");
    }

    #[test]
    fn test_xprop_pessimistic_bitor_z_dominated() {
        // BUG FIX (fuZZ): `1 | Z = 1` sesuai tabel LRM 4-state.
        let prev = get_xprop_mode();
        set_xprop_mode(XPropagationMode::Pessimistic);
        let r1 = eval_binary(BinaryIrOp::BitOr, &LogicVec::from_u64(1, 1), &z_vec());
        let r1r = eval_binary(BinaryIrOp::BitOr, &z_vec(), &LogicVec::from_u64(1, 1));
        // 0 | Z tetap X (tidak didominasi).
        let r0 = eval_binary(BinaryIrOp::BitOr, &LogicVec::from_u64(0, 1), &z_vec());
        set_xprop_mode(prev);
        assert_eq!(r1.bits[0], LogicVal::One, "pessimistic: 1 | Z harus 1");
        assert_eq!(r1r.bits[0], LogicVal::One, "pessimistic: Z | 1 harus 1");
        assert_eq!(r0.bits[0], LogicVal::X, "pessimistic: 0 | Z tetap X");
    }

    #[test]
    fn test_xprop_pessimistic_eq_x_returns_x() {
        let prev = get_xprop_mode();
        set_xprop_mode(XPropagationMode::Pessimistic);
        let r = eval_binary(BinaryIrOp::Eq, &LogicVec::from_u64(5, 8), &x_vec());
        set_xprop_mode(prev);
        assert_eq!(r.bits[0], LogicVal::X, "pessimistic: 5 == X harus X");
    }

    #[test]
    fn test_extend_x_operand_is_zero_filled() {
        // REGRESI (fuzzer O5 differential, bug_0793 prim_intr_hw): operand
        // 1-bit X di-extend ke lebar max. IEEE 1800-2017 §11.8.2 unsigned =
        // ZERO-fill → `1'bx | 2'b00` = `2'b0x`. Implementasi lama mengisi
        // bit ekstensi dgn X (msb X) → `2'bxx` dan beda dari jalur packed
        // (`PackedLogicVec::resize` zero-fill) → differential antar engine.
        let prev = get_xprop_mode();
        set_xprop_mode(XPropagationMode::Pessimistic);
        let r = eval_binary(BinaryIrOp::BitOr, &x_vec(), &LogicVec::from_u64(0, 2));
        set_xprop_mode(prev);
        assert_eq!(r.width, 2, "lebar hasil = max(operand)");
        assert_eq!(
            r.bits[1],
            LogicVal::Zero,
            "bit ekstensi harus zero-fill (§11.8.2), bukan X"
        );
        assert_eq!(r.bits[0], LogicVal::X, "bit asli tetap X");
    }

    // ─── Defensif: LogicVec korup (width>0, bits kosong) — PANIC-13 ───
    // Index OOB pada array unpacked pernah menghasilkan `bits` kosong dengan
    // width>0 → panic index-out-of-bounds di to_bitmasks (value.rs:28).
    // Fast path harus memperlakukan bit yang hilang sebagai X.
    fn corrupt_vec(width: usize) -> LogicVec {
        LogicVec {
            bits: Vec::new(),
            width,
        }
    }

    #[test]
    fn test_to_bitmasks_empty_bits_does_not_panic() {
        let (known, _) = to_bitmasks(&corrupt_vec(8));
        assert_eq!(known, 0, "bit hilang = X → known mask kosong");
    }

    #[test]
    fn test_eval_unary_on_empty_bits_does_not_panic() {
        let v = corrupt_vec(8);
        let bn = eval_unary(UnaryIrOp::BitNot, &v);
        assert_eq!(bn.bits[0], LogicVal::X, "BitNot X = X");
        let ra = eval_unary(UnaryIrOp::RedAnd, &v);
        assert_eq!(ra.bits[0], LogicVal::X, "RedAnd X = X");
        let ro = eval_unary(UnaryIrOp::RedOr, &v);
        assert_eq!(ro.bits[0], LogicVal::X, "RedOr X = X");
    }

    #[test]
    fn test_eval_binary_eq_on_empty_bits_does_not_panic() {
        let v = corrupt_vec(4);
        let other = LogicVec::from_u64(0, 4);
        let eq = eval_binary(BinaryIrOp::Eq, &v, &other);
        assert_eq!(eq.bits[0], LogicVal::X, "X == 0 → X (pessimistic)");
        let neq = eval_binary(BinaryIrOp::Neq, &v, &other);
        assert_eq!(neq.bits[0], LogicVal::X, "X != 0 → X (pessimistic)");
    }
}
