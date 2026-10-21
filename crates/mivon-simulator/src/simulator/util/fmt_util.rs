//! Helper format angka untuk `$display`/`$write` (util.rs).
//!
//! Dipisah dari `util.rs`: konversi/representasi angka (desimal, hex,
//! padding, eksponen) tanpa menyentuh state engine.

use mivon_ast::*;
use mivon_ir::LogicVec;

/// Signedness ekspresi di jalur AST (body method class): hanya
/// `-<literal desimal>` (unary minus pada unsized integer literal = signed
/// 32-bit, IEEE 1800 §6.8.1) yang dapat dipastikan signed tanpa info tipe
/// field/local. Kasus lain dianggap unsigned (field class tidak menyimpan
/// signedness — keterbatasan dicatat).
pub(super) fn ast_expr_is_signed(expr: &Expr) -> bool {
    match expr {
        Expr::UnaryOp {
            op: UnaryOp::Minus,
            expr: inner,
        } => matches!(inner.as_ref(), Expr::Value(Value::Decimal(_))),
        _ => false,
    }
}

/// Jumlah karakter `%d` signed: digit abs + 1 untuk tanda '-' (0 → 1).
/// String representasi desimal bertanda (dipakai arm `%d`).
pub(super) fn i64_digits_str(n: i64) -> String {
    n.to_string()
}

/// String representasi desimal tak bertanda (dipakai arm `%d`).
pub(super) fn u64_digits_str(n: u64) -> String {
    n.to_string()
}

/// Jumlah digit desimal dari `u128` (1 untuk 0).
pub(super) fn digits_of_u128(mut n: u128) -> usize {
    let mut d = 1usize;
    while n >= 10 {
        n /= 10;
        d += 1;
    }
    d
}

/// Default field width arm `%d` tanpa width eksplisit (LRM 1800 Tabel 21-3):
/// nilai di-right-justify dalam field selebar representasi MAKSIMUM tipe —
/// unsigned `w` bit → digit(2^w − 1), signed `w` bit → 1 (tanda) + digit(2^(w−1)).
/// `%0d` (zero-fill) = tanpa padding — jadi lebarnya 0.
pub(super) fn default_dec_field_width(width: usize, signed: bool) -> usize {
    if width == 0 {
        return 1;
    }
    // Batasi 128 bit agar tidak meledakkan (2^128 masih muat u128).
    let w = width.min(120);
    let mut pow2: u128 = 1;
    for _ in 0..w {
        pow2 = pow2.saturating_mul(2);
    }
    if signed {
        // magnitude maks = 2^(w-1); field = digit + 1 utk tanda '-'
        let mag = pow2 >> 1;
        digits_of_u128(mag) + 1
    } else {
        digits_of_u128(pow2 - 1)
    }
}

/// Jumlah digit hex dari u64 (1 untuk 0) — hindari format! alloc di %h.
#[allow(dead_code)] // util: dipakai tool/feature selanjutnya
pub(super) fn u64_hex_digits(mut n: u64) -> usize {
    let mut d = 1usize;
    while n >= 16 {
        n /= 16;
        d += 1;
    }
    d
}

pub fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Konversi nilai argumen format untuk specifier INTEGER: real (bit-pattern
/// f64) → integer 32-bit hasil pembulatan (LRM 1800 §21.2.1.4: `%d` dari real
/// dicetak sebagai integer). Tanpa ini `$display("%d", real_var)` mencetak
/// bit-pattern mentah (mis. 4620580627691444634 untuk 7.9).
pub(super) fn fmt_arg_as_int(val: &LogicVec, is_real: bool) -> LogicVec {
    if !is_real {
        return val.clone();
    }
    let f = f64::from_bits(val.to_u64());
    if !f.is_finite() {
        return LogicVec::from_u64(0, 32);
    }
    LogicVec::from_u64(f.round() as i64 as u64, 32)
}

/// Konversi nilai argumen format untuk specifier REAL: integer → f64
/// (bit-pattern 64-bit). Tanpa ini `$display("%f", 3)` membaca integer 3
/// sebagai bit-pattern f64 yang nonsense.
pub(super) fn fmt_arg_as_real(val: &LogicVec, is_real: bool, is_signed: bool) -> f64 {
    if is_real {
        return f64::from_bits(val.to_u64());
    }
    if is_signed {
        val.to_i64() as f64
    } else {
        val.to_u64() as f64
    }
}

/// Dorong string hasil format ke output dengan padding `width` (space default,
/// `'0'` bila zero-fill `%0`, rata kiri bila flag `-`).
pub(super) fn push_padded(out: &mut String, s: &str, width: usize, zero_fill: bool, left_align: bool) {
    if width > s.len() {
        let extra = width - s.len();
        if left_align {
            out.push_str(s);
            for _ in 0..extra {
                out.push(' ');
            }
            return;
        }
        let pad = if zero_fill { '0' } else { ' ' };
        for _ in 0..extra {
            out.push(pad);
        }
    }
    out.push_str(s);
}

/// Normalisasi notasi eksponen Rust → C printf / IEEE 1800 §21.2.1.4:
/// `3.14e0` → `3.14e+00` (tanda eksplisit, minimal 2 digit eksponen).
pub(super) fn normalize_exp(s: &str) -> String {
    let Some((mant, exp)) = s.split_once(['e', 'E']) else {
        return s.to_string();
    };
    let (sign, digits) = match exp.strip_prefix('-') {
        Some(d) => ("-", d),
        None => ("+", exp.strip_prefix('+').unwrap_or(exp)),
    };
    if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
        let d = if digits.len() < 2 {
            format!("0{digits}")
        } else {
            digits.to_string()
        };
        format!("{mant}e{sign}{d}")
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regresi pemisahan: lebar field default desimal (LRM Tabel 21-3).
    #[test]
    fn dec_field_width_matches_lrm() {
        // unsigned 8 bit: 255 → 3 digit
        assert_eq!(default_dec_field_width(8, false), 3);
        // signed 8 bit: -128 → 4 karakter (tanda + 3 digit)
        assert_eq!(default_dec_field_width(8, true), 4);
        // width 0 = minimal 1
        assert_eq!(default_dec_field_width(0, false), 1);
    }

    /// Eksponen dinormalisasi ke format C printf (tanda + 2 digit).
    #[test]
    fn exponent_normalised_to_printf() {
        assert_eq!(normalize_exp("3.14e0"), "3.14e+00");
        assert_eq!(normalize_exp("1e-5"), "1e-05");
        assert_eq!(normalize_exp("2.5"), "2.5");
    }
}
