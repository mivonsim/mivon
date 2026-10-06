use std::collections::HashMap;

use crate::expr::{BinaryOp, Expr, UnaryOp, Value};
use mivon_core::intern::Symbol;

/// Encode a short string as i64 for parameter comparison purposes.
/// Strings up to 8 characters are encoded as little-endian bytes.
pub fn string_to_i64(s: &str) -> i64 {
    let bytes = s.as_bytes();
    let mut val: i64 = 0;
    for (i, &b) in bytes.iter().enumerate().take(8) {
        val |= (b as i64) << (i * 8);
    }
    val
}

/// Nama fungsi dasar untuk fungsi yang dipanggil secara scoped
/// (`pkg::func(...)`). Mengembalikan bagian setelah `::` terakhir agar
/// dispatch fungsi konstan berlaku global untuk package mana pun.
fn base_func_name(name: &str) -> &str {
    name.rsplit_once("::").map(|(_, f)| f).unwrap_or(name)
}

/// Parse literal berbasis angka (`bits` = digit tanpa prefix) menjadi i64
/// dengan bit-pattern dipertahankan. Nilai ≥ 2^63 (mis. `64'hC0AC29B7C97C50DD`
/// yang dipakai konstanta kriptografi OpenTitan) tidak muat di i64 — parse
/// sebagai u64 lalu wrap. Operasi bit (BitSelect/RangeSelect/`& mask`) pada
/// nilai negatif tetap menghasilkan bit asli karena `>>`/`&` bekerja pada
/// representasi two's complement.
pub fn parse_literal(bits: &str, radix: u32) -> Result<i64, String> {
    u64::from_str_radix(&bits.replace(['x', 'z'], "0"), radix)
        .map(|v| v as i64)
        .map_err(|_| "bad literal".to_string())
}

/// Nilai literal B/H/O dengan memperhitungkan lebar & signedness.
///
/// Literal sized-signed (`8'sd130`) adalah POLA w-bit yang ditafsirkan
/// two's-complement: pola 10000010 @ lebar 8 = -126, BUKAN +130. Tanpa
/// ini perbandingan konstan ter-fold salah (`8'sd130 >= 8'sd124` dianggap
/// true padahal -126 >= 124 false — ditemukan fuzzer signed_fuzz
/// seed=115, dikonfirmasi Icarus).
fn parse_sized_literal(
    bits: &str,
    radix: u32,
    width: Option<usize>,
    is_signed: bool,
) -> Result<i64, String> {
    let u = u64::from_str_radix(&bits.replace(['x', 'z'], "0"), radix)
        .map_err(|_| "bad literal".to_string())?;
    if let Some(w) = width {
        if is_signed && w > 0 && w < 64 && ((u >> (w - 1)) & 1) == 1 {
            return Ok((u as i64).wrapping_sub(1i64 << w));
        }
    }
    Ok(u as i64)
}

/// Pola mentah + lebar + signedness literal sized (Paren tembus).
/// `None` untuk unsized/ekspresi lain — pemanggil fallback ke jalur i64.
fn sized_pattern(e: &Expr) -> Option<(u64, u32, bool)> {
    let (bits, radix, width, is_signed) = match e {
        Expr::Paren(inner) => return sized_pattern(inner),
        Expr::Value(Value::Binary {
            bits,
            width,
            is_signed,
        }) => (bits, 2u32, width, is_signed),
        Expr::Value(Value::Hex {
            bits,
            width,
            is_signed,
        }) => (bits, 16, width, is_signed),
        Expr::Value(Value::Octal {
            bits,
            width,
            is_signed,
        }) => (bits, 8, width, is_signed),
        _ => return None,
    };
    let u = u64::from_str_radix(&bits.replace(['x', 'z'], "0"), radix).ok()?;
    Some((u, width.map(|w| w as u32).unwrap_or(0), *is_signed))
}

/// Interpretasi pola p pada lebar `w` bit: signed → two's-complement,
/// unsigned → zero-extend. Dipakai Div/Mod konstan yang HARUS dihitung
/// pada lebar operan bersama (LRM §11.8.1) — pembagian tidak kongruen
/// mod 2^w, beda dari Add/Sub/Mul (signed_fuzz seed=143: `32'sd2710823578
/// % 32'sd2464636924` salah kalau dihitung full-precision).
fn interpret_pattern(p: u64, w: u32, is_signed: bool) -> i64 {
    if w == 0 || w >= 64 {
        return p as i64;
    }
    let mask = if w == 64 { u64::MAX } else { (1u64 << w) - 1 };
    let pat = p & mask;
    if is_signed && ((pat >> (w - 1)) & 1) == 1 {
        (pat as i64).wrapping_sub(1i64 << w)
    } else {
        pat as i64
    }
}

/// Evaluasi Div/Mod dua literal sized dengan semantik lebar bersama.
/// `None` bila ada operan unsized/non-literal (pemanggil pakai fallback).
fn sized_divmod(lhs: &Expr, rhs: &Expr, is_div: bool) -> Option<Result<i64, String>> {
    let (lp, lw, ls) = sized_pattern(lhs)?;
    let (rp, rw, rs) = sized_pattern(rhs)?;
    let w = lw.max(rw);
    let x = interpret_pattern(lp, w, ls);
    let y = interpret_pattern(rp, w, rs);
    if y == 0 {
        return Some(Err("division by zero in constant expression".to_string()));
    }
    if x == i64::MIN && y == -1 {
        return Some(Ok(0));
    }
    Some(Ok(if is_div {
        x.wrapping_div(y)
    } else {
        x.wrapping_rem(y)
    }))
}

pub fn const_eval_simple(expr: &Expr) -> Result<i64, String> {
    match expr {
        Expr::Value(Value::Decimal(n)) => Ok(*n),
        Expr::Value(Value::Binary { bits, .. }) => parse_literal(bits, 2),
        Expr::Value(Value::Hex { bits, .. }) => parse_literal(bits, 16),
        Expr::Value(Value::Octal { bits, .. }) => parse_literal(bits, 8),
        Expr::Ident { name: ref s, .. } if s == "1" => Ok(1),
        Expr::MethodCall { .. } => Err("method calls are not simple constants".to_string()),
        Expr::MemberAccess { .. } => Err("member access is not a simple constant".to_string()),
        Expr::StructLit { .. } => Err("struct literal is not a simple constant".to_string()),
        _ => Err("not a simple constant".to_string()),
    }
}

/// Lebar self-determined literal sized (`2'b11` → 2, `8'hFF` → 8, Paren
/// tembus). `None` untuk unsized/ekspresi lain — pemakai mempertahankan
/// semantik i64 lama.
///
/// Signedness ekspresi (IEEE 1800 §6.11 / §11.8.2): desimal unsized &
/// literal ber-suffix `s` SIGNED; sized B/H/O tanpa `s` UNSIGNED; unary
/// ±/~ mewarisi operand. LRM: perbandingan UNSIGNED bila SALAH SATU operand
/// unsigned. Dipakai arm Lt/Le/Gt/Ge — dulu selalu i64 signed sehingga
/// const-fold `-((-(32'h...) >= !(32'h...)))` salah (ditemukan fuzzer
/// seed=59498908, dikonfirmasi Icarus).
fn is_signed_expr(e: &Expr) -> bool {
    match e {
        Expr::Value(Value::Decimal(_)) => true,
        Expr::Value(Value::Binary { is_signed, .. })
        | Expr::Value(Value::Hex { is_signed, .. })
        | Expr::Value(Value::Octal { is_signed, .. }) => *is_signed,
        Expr::Paren(inner) => is_signed_expr(inner),
        Expr::UnaryOp { op, expr: inner } => match op {
            UnaryOp::Not
            | UnaryOp::ReductionAnd
            | UnaryOp::ReductionNand
            | UnaryOp::ReductionOr
            | UnaryOp::ReductionNor
            | UnaryOp::ReductionXor
            | UnaryOp::ReductionXnor => false,
            _ => is_signed_expr(inner),
        },
        _ => false,
    }
}

/// Bandingkan dua nilai const dengan aturan signedness SV.
fn const_cmp(
    lhs: &Expr,
    rhs: &Expr,
    l: i64,
    r: i64,
    op: impl Fn(std::cmp::Ordering) -> bool,
) -> i64 {
    let ord = if is_signed_expr(lhs) && is_signed_expr(rhs) {
        l.cmp(&r)
    } else {
        // Unsigned: mask ke lebar self-determined literal agar
        // sign-extended i64 tidak merusak perbandingan
        // (16'shdddb <= 16'hffff harus 1, bukan 0 — ditemukan
        // mixed_sign_fuzz seed=87). LRM §11.8.2: operan
        // di-extend sesuai signedness masing-masing SEBELUM
        // dibandingkan.
        let lw = sized_width(lhs).unwrap_or(64).min(64);
        let rw = sized_width(rhs).unwrap_or(64).min(64);
        let lm = if lw >= 64 { u64::MAX } else { (1u64 << lw) - 1 };
        let rm = if rw >= 64 { u64::MAX } else { (1u64 << rw) - 1 };
        ((l as u64) & lm).cmp(&((r as u64) & rm))
    };
    if op(ord) {
        1
    } else {
        0
    }
}
pub fn sized_width(e: &Expr) -> Option<u64> {
    match e {
        Expr::Paren(inner) => sized_width(inner),
        // Replikasi `{N{e}}` → N × lebar self-determined e (IEEE 1800
        // §11.4.12). Tanpa ini concat menghitung lebar elemen replikasi
        // dari bit-length NILAINYA (`{3{3'b000}}` dianggap 1 bit!) sehingga
        // lebar concat salah ({3'b110, {3{3'b000}}} jadi 4 bit, bukan 12 —
        // ditemukan fuzzer seed=2495169615340, konfirmasi Icarus).
        Expr::Replicate { count, expr } => {
            let n = match count.as_ref() {
                Expr::Value(Value::Decimal(n)) if *n > 0 => *n as u64,
                _ => return None,
            };
            Some(n.saturating_mul(sized_width(expr)?))
        }
        Expr::Value(Value::Binary { bits, width, .. }) => {
            Some(width.map(|w| w as u64).unwrap_or(bits.len() as u64))
        }
        Expr::Value(Value::Hex { bits, width, .. }) => {
            Some(width.map(|w| w as u64).unwrap_or(bits.len() as u64 * 4))
        }
        Expr::Value(Value::Octal { bits, width, .. }) => {
            Some(width.map(|w| w as u64).unwrap_or(bits.len() as u64 * 3))
        }
        // Self-determined width (IEEE 1800 §11.8.1):
        // - `!x` & semua reduction → 1 bit.
        // - `~x` / unary ± → lebar operand (ditemukan fuzzer seed=769558:
        //   `~(!(lit))` tanpa width info menghasilkan -1 unmasked yang
        //   merusak concat element width).
        Expr::UnaryOp { op, expr } => match op {
            UnaryOp::Not
            | UnaryOp::ReductionAnd
            | UnaryOp::ReductionNand
            | UnaryOp::ReductionOr
            | UnaryOp::ReductionNor
            | UnaryOp::ReductionXor
            | UnaryOp::ReductionXnor => Some(1),
            UnaryOp::BitNot | UnaryOp::Minus | UnaryOp::Plus => sized_width(expr),
        },
        // Shift: hasil selebar LHS (LRM §11.8.1) — tanpa ini Sshr
        // const_fold tidak tahu lebar intermediate (shift_chain_fuzz).
        // Harus SEBELUM arm `Expr::BinaryOp` umum — kalau sesudahnya, arm
        // umum menelan semua BinaryOp → shift jadi unreachable dan lebar
        // shift tak pernah terisi (bug latent yang di-expose clippy).
        Expr::BinaryOp {
            op: BinaryOp::Shl | BinaryOp::Shr | BinaryOp::Sshl | BinaryOp::Sshr,
            lhs,
            ..
        } => sized_width(lhs),
        // Perbandingan/relasional/logical biner → 1 bit.
        Expr::BinaryOp { op, .. } => matches!(
            op,
            BinaryOp::Eq
                | BinaryOp::Neq
                | BinaryOp::CaseEq
                | BinaryOp::CaseNeq
                | BinaryOp::EqWild
                | BinaryOp::NeqWild
                | BinaryOp::Lt
                | BinaryOp::Le
                | BinaryOp::Gt
                | BinaryOp::Ge
                | BinaryOp::LogicalAnd
                | BinaryOp::LogicalOr
        )
        .then_some(1),
        // Concat → jumlah lebar elemen (None bila ada yang tak diketahui).
        Expr::Concat(elems) => elems
            .iter()
            .map(sized_width)
            .try_fold(0u64, |acc, w| w.map(|w| acc + w)),
        _ => None,
    }
}

pub fn const_eval_with_params(
    expr: &Expr,
    param_vals: &HashMap<Symbol, i64>,
) -> Result<i64, String> {
    match expr {
        Expr::Value(Value::Decimal(n)) => Ok(*n),
        Expr::Value(Value::Binary {
            bits,
            width,
            is_signed,
        }) => parse_sized_literal(bits, 2, *width, *is_signed),
        Expr::Value(Value::Hex {
            bits,
            width,
            is_signed,
        }) => parse_sized_literal(bits, 16, *width, *is_signed),
        Expr::Value(Value::Octal {
            bits,
            width,
            is_signed,
        }) => parse_sized_literal(bits, 8, *width, *is_signed),
        Expr::String(s) => Ok(string_to_i64(s)),
        Expr::Ident { name, .. } => {
            if let Some(&val) = param_vals.get(name) {
                Ok(val)
            } else if name == "1" {
                Ok(1)
            } else if name.starts_with("$") {
                Err(format!(
                    "cannot evaluate system function '{}' in constant context",
                    name
                ))
            } else {
                Err(format!("'{}' not found in parameter context", name))
            }
        }
        Expr::UnaryOp {
            op: UnaryOp::Minus,
            expr: inner,
        } => Ok(-const_eval_with_params(inner, param_vals)?),
        Expr::UnaryOp {
            op: UnaryOp::Plus,
            expr: inner,
        } => Ok(const_eval_with_params(inner, param_vals)?),
        Expr::UnaryOp {
            op: UnaryOp::BitNot,
            expr: inner,
        } => {
            let v = const_eval_with_params(inner, param_vals)?;
            // Mask ke lebar self-determined operand (SV §11.4.9). Jangan
            // melebar ke 32: `!(~x)` menguji zero-ness hasil — inversi pada
            // lebar lain mengubah nol-tidak-nolnya (ditemukan fuzzer
            // seed=19238926: `!(~(2'b11)) + 2'b11`). Subtree ber-~/unary --
            // tidak di-fold oleh try_fold_const (guard di const_fold.rs);
            // konteks ditangani jalur runtime + propagasi Cast.
            match sized_width(inner) {
                Some(w) if w > 0 && w < 64 => Ok(!v & (((1u64 << w) - 1) as i64)),
                _ => Ok(!v),
            }
        }
        Expr::BinaryOp {
            op: BinaryOp::Add,
            lhs,
            rhs,
        } => {
            let l = const_eval_with_params(lhs, param_vals)?;
            let r = const_eval_with_params(rhs, param_vals)?;
            Ok(l.wrapping_add(r))
        }
        Expr::BinaryOp {
            op: BinaryOp::Sub,
            lhs,
            rhs,
        } => {
            let l = const_eval_with_params(lhs, param_vals)?;
            let r = const_eval_with_params(rhs, param_vals)?;
            Ok(l.wrapping_sub(r))
        }
        Expr::BinaryOp {
            op: BinaryOp::Mul,
            lhs,
            rhs,
        } => {
            let l = const_eval_with_params(lhs, param_vals)?;
            let r = const_eval_with_params(rhs, param_vals)?;
            Ok(l.wrapping_mul(r))
        }
        Expr::BinaryOp {
            op: BinaryOp::Div,
            lhs,
            rhs,
        } => {
            // Literal sized: operasi pada lebar operan bersama (§11.8.1).
            if let Some(res) = sized_divmod(lhs, rhs, true) {
                return res;
            }
            let l = const_eval_with_params(lhs, param_vals)?;
            let r = const_eval_with_params(rhs, param_vals)?;
            if r == 0 {
                return Err("division by zero in constant expression".to_string());
            }
            // i64::MIN / -1 overflows
            if l == i64::MIN && r == -1 {
                return Ok(0);
            }
            // SV §11.8.2 (any-unsigned): pembagian UNSIGNED bila salah satu
            // operan unsigned (literal besar > i64::MAX = bit-pattern u64,
            // ditemukan fuzzer seed=120172402), SIGNED bila keduanya signed.
            // Sebelumnya `(l as u64)` selalu — `-7 / 3` di-fold jadi
            // 0xFFFF...F9 / 3 = 0x5555...5553 (6148914691236517203) padahal
            // harus -2 (probe p12d, konstanta-folding ≠ runtime).
            let q = if is_signed_expr(lhs) && is_signed_expr(rhs) {
                l.wrapping_div(r)
            } else {
                ((l as u64) / (r as u64)) as i64
            };
            Ok(q)
        }
        Expr::BinaryOp {
            op: BinaryOp::Power,
            lhs,
            rhs,
        } => {
            let base = const_eval_with_params(lhs, param_vals)?;
            let exp = const_eval_with_params(rhs, param_vals)?;
            // Eksponensiasi modular biner mod 2^ow.
            //
            // LRM 1800-2017 §11.6.1 Tabel 11-21: `i ** j` → lebar hasil =
            // L(i) = lebar operand KIRI. Amount `j` self-determined dan TIDAK
            // melebarkan hasil — sama persis aturan shift (`<<`/`>>`).
            //
            // Dulu `ow = max(lebar lhs, lebar rhs)`: eksponen berupa literal
            // unsized (`17 ** 2`, `2` di-size 32 bit) melebarkan mask ke 32
            // bit, sehingga hasil 289 lolos penuh padahal konteksnya 8 bit:
            //     localparam logic [7:0] E = 17 ** 2;
            //     mivon E = 289 (289 & 0xff = 33) iverilog E = 33
            // Sama dengan bug runtime yang sudah diperbaiki di
            // `mivon-simulator/.../value.rs` (`BinaryIrOp::Power`) — kedua
            // jalur harus sepakat, kalau tidak parameter dan ekspresi biasa memberi
            // jawaban berbeda untuk sumber yang sama.
            // memberi jawaban berbeda untuk sumber yang sama.
            //
            // Fallback `.unwrap_or(64)`: kedua operand unsized → LRM memakai
            // lebar operand kiri, tapi `sized_width` tak bisa memastikan
            // (operand non-literal); 64 = i64 penuh, perilaku lama.
            let ow = sized_width(lhs)
                .unwrap_or_else(|| sized_width(rhs).unwrap_or(64))
                .clamp(1, 64) as u32;
            // Mask mod 2^ow — ow=64 → m=MAX (1u64 << 64 overflows).
            let m: u64 = if ow >= 64 { u64::MAX } else { (1u64 << ow) - 1 };
            let mut b = (base as u64) & m;
            let mut acc: u64 = 1 & m;
            let mut e = exp as u64;
            while e > 0 {
                if e & 1 == 1 {
                    acc = acc.wrapping_mul(b) & m;
                }
                e >>= 1;
                if e > 0 {
                    b = b.wrapping_mul(b) & m;
                }
            }
            Ok(acc as i64)
        }
        Expr::BinaryOp {
            op: BinaryOp::Mod,
            lhs,
            rhs,
        } => {
            // Literal sized: operasi pada lebar operan bersama (§11.8.1).
            if let Some(res) = sized_divmod(lhs, rhs, false) {
                return res;
            }
            let l = const_eval_with_params(lhs, param_vals)?;
            let r = const_eval_with_params(rhs, param_vals)?;
            if r == 0 {
                return Err("modulo by zero in constant expression".to_string());
            }
            // i64::MIN % -1 overflows
            if l == i64::MIN && r == -1 {
                return Ok(0);
            }
            // Unsigned semantics bila salah satu operan unsigned (sama dgn
            // Div — lihat catatan di atas); signed bila keduanya signed:
            // `-7 % 3` harus -1, bukan 0 (probe p12d).
            let m = if is_signed_expr(lhs) && is_signed_expr(rhs) {
                l.wrapping_rem(r)
            } else {
                ((l as u64) % (r as u64)) as i64
            };
            Ok(m)
        }
        Expr::BinaryOp {
            op: BinaryOp::Eq,
            lhs,
            rhs,
        } => {
            let l = const_eval_with_params(lhs, param_vals)?;
            let r = const_eval_with_params(rhs, param_vals)?;
            Ok(if l == r { 1 } else { 0 })
        }
        Expr::BinaryOp {
            op: BinaryOp::Neq,
            lhs,
            rhs,
        } => {
            let l = const_eval_with_params(lhs, param_vals)?;
            let r = const_eval_with_params(rhs, param_vals)?;
            Ok(if l != r { 1 } else { 0 })
        }
        Expr::BinaryOp {
            op: BinaryOp::Lt,
            lhs,
            rhs,
        } => {
            let l = const_eval_with_params(lhs, param_vals)?;
            let r = const_eval_with_params(rhs, param_vals)?;
            Ok(const_cmp(lhs, rhs, l, r, |o| o == std::cmp::Ordering::Less))
        }
        Expr::BinaryOp {
            op: BinaryOp::Le,
            lhs,
            rhs,
        } => {
            let l = const_eval_with_params(lhs, param_vals)?;
            let r = const_eval_with_params(rhs, param_vals)?;
            Ok(const_cmp(lhs, rhs, l, r, |o| {
                o != std::cmp::Ordering::Greater
            }))
        }
        Expr::BinaryOp {
            op: BinaryOp::Gt,
            lhs,
            rhs,
        } => {
            let l = const_eval_with_params(lhs, param_vals)?;
            let r = const_eval_with_params(rhs, param_vals)?;
            Ok(const_cmp(lhs, rhs, l, r, |o| {
                o == std::cmp::Ordering::Greater
            }))
        }
        Expr::BinaryOp {
            op: BinaryOp::Ge,
            lhs,
            rhs,
        } => {
            let l = const_eval_with_params(lhs, param_vals)?;
            let r = const_eval_with_params(rhs, param_vals)?;
            Ok(const_cmp(lhs, rhs, l, r, |o| o != std::cmp::Ordering::Less))
        }
        Expr::BinaryOp {
            op: BinaryOp::LogicalAnd,
            lhs,
            rhs,
        } => {
            let l = const_eval_with_params(lhs, param_vals)?;
            let r = const_eval_with_params(rhs, param_vals)?;
            Ok(if l != 0 && r != 0 { 1 } else { 0 })
        }
        Expr::BinaryOp {
            op: BinaryOp::LogicalOr,
            lhs,
            rhs,
        } => {
            let l = const_eval_with_params(lhs, param_vals)?;
            let r = const_eval_with_params(rhs, param_vals)?;
            Ok(if l != 0 || r != 0 { 1 } else { 0 })
        }
        Expr::BinaryOp {
            op: BinaryOp::BitAnd,
            lhs,
            rhs,
        } => {
            Ok(const_eval_with_params(lhs, param_vals)? & const_eval_with_params(rhs, param_vals)?)
        }
        Expr::BinaryOp {
            op: BinaryOp::BitOr,
            lhs,
            rhs,
        } => {
            Ok(const_eval_with_params(lhs, param_vals)? | const_eval_with_params(rhs, param_vals)?)
        }
        Expr::BinaryOp {
            op: BinaryOp::BitXor,
            lhs,
            rhs,
        } => {
            Ok(const_eval_with_params(lhs, param_vals)? ^ const_eval_with_params(rhs, param_vals)?)
        }
        Expr::BinaryOp {
            op: BinaryOp::BitXnor,
            lhs,
            rhs,
        } => {
            Ok(!(const_eval_with_params(lhs, param_vals)?
                ^ const_eval_with_params(rhs, param_vals)?))
        }
        Expr::BinaryOp {
            op: BinaryOp::Shl,
            lhs,
            rhs,
        } => {
            let l = const_eval_with_params(lhs, param_vals)?;
            let r = const_eval_with_params(rhs, param_vals)?;
            // Guard overflow: shift >= 64 → 0; shift negatif (SV: undefined)
            // → tanpa shift (hindari panic debug).
            if !(0..64).contains(&r) {
                Ok(0)
            } else {
                // Mask ke lebar LHS (LRM §11.8.1: hasil shift =
                // lebar operan kiri) — tanpa ini rantai shift salah:
                // `(4'hb << 4'h8) >> 4'h9` = 5 padahal 0 (shift_chain_fuzz).
                let ow = sized_width(lhs).unwrap_or(64).min(64) as u32;
                let m: u64 = if ow >= 64 { u64::MAX } else { (1u64 << ow) - 1 };
                Ok(((l << r) as u64 & m) as i64)
            }
        }
        Expr::BinaryOp {
            op: BinaryOp::Shr,
            lhs,
            rhs,
        } => {
            let l = const_eval_with_params(lhs, param_vals)?;
            let r = const_eval_with_params(rhs, param_vals)?;
            if !(0..64).contains(&r) {
                Ok(0)
            } else {
                let ow = sized_width(lhs).unwrap_or(64).min(64) as u32;
                let m: u64 = if ow >= 64 { u64::MAX } else { (1u64 << ow) - 1 };
                Ok(((l >> r) as u64 & m) as i64)
            }
        }
        Expr::BinaryOp {
            op: BinaryOp::Sshl,
            lhs,
            rhs,
        } => {
            let l = const_eval_with_params(lhs, param_vals)?;
            let r = const_eval_with_params(rhs, param_vals)?;
            if !(0..64).contains(&r) {
                Ok(0)
            } else {
                let ow = sized_width(lhs).unwrap_or(64).min(64) as u32;
                let m: u64 = if ow >= 64 { u64::MAX } else { (1u64 << ow) - 1 };
                Ok(((l << r) as u64 & m) as i64)
            }
        }
        Expr::BinaryOp {
            op: BinaryOp::Sshr,
            lhs,
            rhs,
        } => {
            let l = const_eval_with_params(lhs, param_vals)?;
            let r = const_eval_with_params(rhs, param_vals)?;
            // Mask ke lebar LHS untuk sign-bit check — i64 tidak
            // tahu lebar SV (ditemukan shift_chain_fuzz seed=5:
            // `4'sh8 >>> 13` harus 0xF padahal l=8 positif di i64).
            let ow = sized_width(lhs).unwrap_or(64).min(64) as u32;
            let m: u64 = if ow >= 64 { u64::MAX } else { (1u64 << ow) - 1 };
            let masked = (l as u64) & m;
            let sign_set = ow > 0 && (masked >> (ow - 1)) & 1 == 1;
            if r >= ow as i64 {
                // Shift >= width: sign-fill (LRM §11.4.10).
                Ok(if sign_set { -1 } else { 0 })
            } else if r < 0 {
                Ok(0)
            } else {
                // Arithmetic shift right: sign-fill高位.
                if sign_set {
                    let fill = if ow >= 64 {
                        !0u64
                    } else {
                        !0u64 << (ow - r as u32)
                    };
                    Ok(((masked >> r as u32) | fill) as i64)
                } else {
                    Ok((masked >> r as u32) as i64)
                }
            }
        }
        Expr::BinaryOp {
            op: BinaryOp::CaseEq,
            lhs,
            rhs,
        } => {
            let l = const_eval_with_params(lhs, param_vals)?;
            let r = const_eval_with_params(rhs, param_vals)?;
            Ok(if l == r { 1 } else { 0 })
        }
        Expr::BinaryOp {
            op: BinaryOp::CaseNeq,
            lhs,
            rhs,
        } => {
            let l = const_eval_with_params(lhs, param_vals)?;
            let r = const_eval_with_params(rhs, param_vals)?;
            Ok(if l != r { 1 } else { 0 })
        }
        Expr::BinaryOp {
            op: BinaryOp::EqWild,
            lhs,
            rhs,
        } => {
            let l = const_eval_with_params(lhs, param_vals)?;
            let r = const_eval_with_params(rhs, param_vals)?;
            Ok(if l == r { 1 } else { 0 })
        }
        Expr::BinaryOp {
            op: BinaryOp::NeqWild,
            lhs,
            rhs,
        } => {
            let l = const_eval_with_params(lhs, param_vals)?;
            let r = const_eval_with_params(rhs, param_vals)?;
            Ok(if l != r { 1 } else { 0 })
        }
        Expr::UnaryOp {
            op: UnaryOp::Not,
            expr: inner,
        } => {
            let v = const_eval_with_params(inner, param_vals)?;
            Ok(if v == 0 { 1 } else { 0 })
        }
        Expr::UnaryOp {
            op: UnaryOp::ReductionAnd,
            expr: inner,
        } => {
            let v = const_eval_with_params(inner, param_vals)?;
            // All-ones butuh LEBAR operand — `v == -1` saja tidak cukup:
            // `&(1'b0)` = 0 dan `&(4'b1111)` = 1 (ditemukan fuzzer baru,
            // konfirmasi Icarus). Gunakan lebar literal bila diketahui.
            match sized_width(inner) {
                Some(w) if w > 0 && w < 64 => {
                    let all_ones = ((v as u64) & ((1u64 << w) - 1)) == (1u64 << w) - 1;
                    Ok(all_ones as i64)
                }
                _ => Ok(if v == -1 { 1 } else { 0 }),
            }
        }
        Expr::UnaryOp {
            op: UnaryOp::ReductionNand,
            expr: inner,
        } => {
            let v = const_eval_with_params(inner, param_vals)?;
            match sized_width(inner) {
                Some(w) if w > 0 && w < 64 => {
                    let all_ones = ((v as u64) & ((1u64 << w) - 1)) == (1u64 << w) - 1;
                    Ok((!all_ones) as i64)
                }
                _ => Ok(if v == -1 { 0 } else { 1 }),
            }
        }
        Expr::UnaryOp {
            op: UnaryOp::ReductionOr,
            expr: inner,
        } => {
            let v = const_eval_with_params(inner, param_vals)?;
            Ok(if v == 0 { 0 } else { 1 })
        }
        Expr::UnaryOp {
            op: UnaryOp::ReductionNor,
            expr: inner,
        } => {
            let v = const_eval_with_params(inner, param_vals)?;
            Ok(if v == 0 { 1 } else { 0 })
        }
        Expr::UnaryOp {
            op: UnaryOp::ReductionXor,
            expr: inner,
        } => {
            let v = const_eval_with_params(inner, param_vals)?;
            Ok((v.count_ones() & 1) as i64)
        }
        Expr::UnaryOp {
            op: UnaryOp::ReductionXnor,
            expr: inner,
        } => {
            let v = const_eval_with_params(inner, param_vals)?;
            Ok(1 - (v.count_ones() & 1) as i64)
        }
        Expr::TernaryOp {
            cond,
            true_expr,
            false_expr,
        } => {
            let cond_val = const_eval_with_params(cond, param_vals)?;
            if cond_val != 0 {
                const_eval_with_params(true_expr, param_vals)
            } else {
                const_eval_with_params(false_expr, param_vals)
            }
        }
        Expr::Paren(inner) => const_eval_with_params(inner, param_vals),
        Expr::Cast { expr: inner, .. } => const_eval_with_params(inner, param_vals),
        Expr::CastWidth { expr: inner, .. } => const_eval_with_params(inner, param_vals),
        // Replikasi `{N{expr}}` di constant context: pola direplikasi pada
        // LEBAR SELF-DETERMINED operand (`sized_width`) — DULU lebar pola =
        // COUNT (`63.min(n)`) sehingga `{2{63'h…}}` ter-fold jadi nilai
        // sampah 4-bit dan `a[0] << ({2{…}} << …)` salah cabang (ditemukan
        // fuzzer seed=674226683294, konfirmasi Icarus). Total > 63 bit →
        // Err agar elaborator membangun IR Replicate runtime.
        Expr::Replicate { count, expr } => {
            let n = const_eval_with_params(count, param_vals)?;
            let v = const_eval_with_params(expr, param_vals)?;
            let n = n.max(0) as u64;
            if n == 0 {
                return Ok(0);
            }
            let w = match sized_width(expr) {
                Some(w) if w > 0 => w,
                _ => {
                    // Lebar pola tak diketahui: hanya pola 1-bit (0/1) yang
                    // aman di-approximate.
                    return match v {
                        0 => Ok(0),
                        1 => Ok((1u64 << n.min(63)).wrapping_sub(1) as i64),
                        _ => Err("replication of unknown-width pattern".to_string()),
                    };
                }
            };
            let total = n.saturating_mul(w);
            if total > 63 {
                return Err("replication wider than 63 bits is not constant-foldable".to_string());
            }
            let pat = (v as u64) & ((1u64 << w) - 1);
            let mut acc: u64 = 0;
            for _ in 0..n {
                acc = (acc << w) | pat;
            }
            Ok(acc as i64)
        }
        // Struct literal utuh tidak bisa di-const-eval sebagai skalar tanpa
        // layout typedef. Nilai 0 (perilaku lama — pola bernama sebelumnya
        // di-discard jadi 0) supaya localparam struct tetap terdaftar. Member
        // access (`P.offset`) tetap benar via key `base.field` di atas.
        Expr::StructLit { .. } => Ok(0),
        // Fill literal `'0`/`'1` — nilai 0 untuk konstanta (localparam struct
        // seperti `mac_bignum_contrl_t ControlDefault = '0` di
        // otbn_mac_bignum_fsm, `sha_word64_t ZeroWord = '0` di prim_sha2).
        Expr::FillLit(_) => Ok(0),
        Expr::ScopedIdent { package, item, .. } => {
            let qualified = Symbol::intern(&format!("{}::{}", package, item));
            if let Some(&val) = param_vals.get(&qualified) {
                return Ok(val);
            }
            // Juga coba tanpa package prefix — enum member yang sudah di-flatten
            // ke param_vals tanpa qualified name (mis. dari `import pkg::*`).
            if let Some(&val) = param_vals.get(item) {
                return Ok(val);
            }
            // `pkg::TypeName` (typedef/enum type) dipakai sebagai type-cast argument
            // atau di ekspresi generate — ini bukan nilai integer yang bisa dievaluasi.
            // Kembalikan error yang informatif; generate.rs akan menangani kasus ini
            // sebagai warning (member access / type expression tidak bisa di-const-eval).
            Err(format!("cannot evaluate package parameter '{}'", qualified))
        }
        Expr::MethodCall { .. } => {
            Err("method calls not allowed in constant expression".to_string())
        }
        Expr::MemberAccess { obj, field } => {
            // Struct field lookup via flattened keys `base.field` (mis.
            // `PartInfo[k].offset`, `hw2reg.key.q`) — dipakai generate if /
            // konstanta dengan struct localparam array.
            if let Some(bk) = expr_base_key(obj, param_vals) {
                let key = format!("{}.{}", bk, field.as_str());
                if let Some(&v) = param_vals.get(&Symbol::intern(&key)) {
                    return Ok(v);
                }
                if std::env::var("DBG_MEMBER").is_ok() {
                    eprintln!(
                        "[DBG-MEMBER] key '{}' NOT FOUND (obj={:?} field={} bk={})",
                        key,
                        obj,
                        field.as_str(),
                        bk
                    );
                }
            } else if std::env::var("DBG_MEMBER").is_ok() {
                eprintln!(
                    "[DBG-MEMBER-NO-BASE] obj={:?} field={}",
                    obj,
                    field.as_str()
                );
            }
            Err("member access not allowed in constant expression".to_string())
        }
        Expr::Inside {
            expr: inner,
            range_list,
        } => {
            let val = const_eval_with_params(inner, param_vals)?;
            for item in range_list {
                // Range inside `{[a:b], c}` — parser menyisipkan RangeSelect
                // dengan base literal 0 sebagai penanda rentang [lsb, msb].
                // HANYA base literal 0 yang dimaknai rentang — slice ekspresi
                // user (`inside {y[3:0]}`) tetap dievaluasi sebagai bit-slice
                // agar tidak salah diartikan sebagai rentang.
                if let Expr::RangeSelect {
                    expr: base,
                    msb,
                    lsb,
                } = item
                {
                    if matches!(base.as_ref(), Expr::Value(Value::Decimal(0))) {
                        // `inside {[a:b]}`: msb=a adalah batas BAWAH, lsb=b batas atas.
                        let lo = const_eval_with_params(msb, param_vals)?;
                        let hi = const_eval_with_params(lsb, param_vals)?;
                        if val >= lo && val <= hi {
                            return Ok(1);
                        }
                    } else if const_eval_with_params(item, param_vals)? == val {
                        return Ok(1);
                    }
                } else if const_eval_with_params(item, param_vals)? == val {
                    return Ok(1);
                }
            }
            Ok(0)
        }
        Expr::BitSelect { expr, index } => {
            // Array element lookup via flattened keys `name[idx]` (array params)
            if let Expr::Ident { name, .. } = expr.as_ref() {
                let idx = const_eval_with_params(index, param_vals)?;
                let key = format!("{}[{}]", name.as_str(), idx);
                if let Some(&v) = param_vals.get(&Symbol::intern(&key)) {
                    return Ok(v);
                }
            }
            // 2D array lookup via flattened keys `name[r][c]` (mis. PiRotate [5][5]).
            // `PiRotate[r][c]` → BitSelect(BitSelect(Ident PiRotate, r), c); cari
            // key `PiRotate[r][c]` langsung.
            if let Expr::BitSelect {
                expr: inner,
                index: row_idx,
            } = expr.as_ref()
            {
                if let Expr::Ident { name, .. } = inner.as_ref() {
                    let r = const_eval_with_params(row_idx, param_vals)?;
                    let c = const_eval_with_params(index, param_vals)?;
                    let key = format!("{}[{}][{}]", name.as_str(), r, c);
                    if let Some(&v) = param_vals.get(&Symbol::intern(&key)) {
                        return Ok(v);
                    }
                }
            }
            let base_val = const_eval_with_params(expr, param_vals)?;
            let idx = const_eval_with_params(index, param_vals)?;
            if !(0..64).contains(&idx) {
                return Ok(0);
            }
            Ok((base_val >> idx) & 1)
        }
        Expr::RangeSelect { expr, msb, lsb } => {
            let base_val = const_eval_with_params(expr, param_vals)?;
            let m = const_eval_with_params(msb, param_vals)?;
            let l = const_eval_with_params(lsb, param_vals)?;
            if !(0..64).contains(&l) {
                return Ok(0);
            }
            let width = (m - l + 1) as usize;
            if width >= 64 {
                Ok(base_val >> l)
            } else {
                let mask = ((1u64 << width) - 1) as i64;
                Ok((base_val >> l) & mask)
            }
        }
        // Indexed part-select `[base +: width]` (pola OpenTitan:
        // `localparam bit [AW-1:0] TopAddr = TopAddrInt[0 +: AW];`). Mivon
        // mengasumsikan arah `+:`. Tanpa ini localparam semacam itu gagal
        // di-const-eval → tidak terdaftar → "signal not found" di pemakaian.
        Expr::PartSelect { expr, base, width } => {
            let src = const_eval_with_params(expr, param_vals)?;
            let b = const_eval_with_params(base, param_vals)?;
            let w = const_eval_with_params(width, param_vals)?;
            if w <= 0 {
                return Err("part-select width must be positive".to_string());
            }
            let width = w as usize;
            let lsb = b;
            if !(0..64).contains(&lsb) {
                return Ok(0);
            }
            if width >= 64 {
                Ok(src >> lsb)
            } else {
                let mask = ((1u64 << width) - 1) as i64;
                Ok((src >> lsb) & mask)
            }
        }
        Expr::FuncCall { name, args, .. } if name == "$clog2" => {
            if let Some(arg) = args.first() {
                let v = const_eval_with_params(arg, param_vals)?;
                if v <= 1 {
                    Ok(0)
                } else {
                    let n = v as u64;
                    let msb = (64 - n.leading_zeros()) as i64;
                    if n.is_power_of_two() {
                        Ok(msb - 1)
                    } else {
                        Ok(msb)
                    }
                }
            } else {
                Ok(0)
            }
        }
        // OpenTitan prim_util_pkg::vbits(value) = (value == 1) ? 1 : $clog2(value)
        Expr::FuncCall { name, args, .. } if base_func_name(name.as_str()) == "vbits" => {
            let v = const_eval_with_params(args.first().ok_or("vbits needs 1 arg")?, param_vals)?;
            Ok(if v == 1 {
                1
            } else {
                let n = v as u64;
                let msb = (64 - n.leading_zeros()) as i64;
                if n.is_power_of_two() {
                    msb - 1
                } else {
                    msb
                }
            })
        }
        // OpenTitan prim_util_pkg::ceil_div(a, b) = ceiling division
        Expr::FuncCall { name, args, .. } if base_func_name(name.as_str()) == "ceil_div" => {
            let a =
                const_eval_with_params(args.first().ok_or("ceil_div needs 2 args")?, param_vals)?;
            let b =
                const_eval_with_params(args.get(1).ok_or("ceil_div needs 2 args")?, param_vals)?;
            if b == 0 {
                return Err("division by zero in ceil_div".to_string());
            }
            Ok(if a % b != 0 { a / b + 1 } else { a / b })
        }
        // OpenTitan prim_secded_pkg::get_synd_width(sd_type, width) — lebar
        // syndrome ECC per tipe & lebar data (tabel konstanta; tipe enum:
        // SecdedHsiao=0, SecdedHamming=1, SecdedInvHsiao=2, SecdedInvHamming=3).
        Expr::FuncCall { name, args, .. } if base_func_name(name.as_str()) == "get_synd_width" => {
            let sd = const_eval_with_params(
                args.first().ok_or("get_synd_width needs 2 args")?,
                param_vals,
            )?;
            let w = const_eval_with_params(
                args.get(1).ok_or("get_synd_width needs 2 args")?,
                param_vals,
            )?;
            let synd = match (sd, w) {
                (0, 16) | (2, 16) => 6,
                (0, 22) | (2, 22) => 6,
                (0, 32) | (2, 32) => 7,
                (0, 57) | (2, 57) => 7,
                (0, 64) | (2, 64) => 8,
                (1, 16) | (3, 16) => 6,
                (1, 32) | (3, 32) => 7,
                (1, 64) | (3, 64) => 8,
                (1, 68) | (3, 68) => 8,
                _ => 0,
            };
            Ok(synd)
        }
        // OpenTitan prim_secded_pkg::is_width_valid(sd_type, width) — apakah
        // kombinasi tipe+lebar didukung (tabel konstanta).
        Expr::FuncCall { name, args, .. } if base_func_name(name.as_str()) == "is_width_valid" => {
            let sd = const_eval_with_params(
                args.first().ok_or("is_width_valid needs 2 args")?,
                param_vals,
            )?;
            let w = const_eval_with_params(
                args.get(1).ok_or("is_width_valid needs 2 args")?,
                param_vals,
            )?;
            let valid = match (sd, w) {
                (0, 16)
                | (0, 22)
                | (0, 32)
                | (0, 57)
                | (0, 64)
                | (2, 16)
                | (2, 22)
                | (2, 32)
                | (2, 57)
                | (2, 64)
                | (1, 16)
                | (1, 32)
                | (1, 64)
                | (1, 68)
                | (3, 16)
                | (3, 32)
                | (3, 64)
                | (3, 68) => 1,
                _ => 0,
            };
            Ok(valid)
        }
        // Replikasi `{N{expr}}` — pola umum `{W{1'b1}}` untuk mask / literal
        // konstanta di localparam/param (mis. `{32 - $bits(...) - 1{1'b0}}` di
        // ibex_cs_registers, `{BeWidth{1'b1}}` di dm_mem). Nilai = pola diulang
        // N kali; lebar pola dari literal eksplisit atau bit-length nilai.
        #[allow(unreachable_patterns)]
        Expr::Replicate { count, expr } => {
            let n = const_eval_with_params(count, param_vals)?;
            let v = const_eval_with_params(expr, param_vals)?;
            let n = n.clamp(0, 63) as u32;
            if v == 0 {
                Ok(0)
            } else {
                let w: u32 = match expr.as_ref() {
                    Expr::Value(Value::Hex { bits, width, .. }) => {
                        width.unwrap_or(bits.len() * 4).max(1) as u32
                    }
                    Expr::Value(Value::Binary { bits, width, .. }) => {
                        width.unwrap_or(bits.len()).max(1) as u32
                    }
                    Expr::Value(Value::Octal { bits, width, .. }) => {
                        width.unwrap_or(bits.len() * 3).max(1) as u32
                    }
                    _ => (64u32 - (v as u64).leading_zeros()).max(1),
                };
                let w = w.min(63);
                let pattern = (v as u64) & ((1u64 << w).wrapping_sub(1));
                let mut acc: u64 = 0;
                let mut total_w: u32 = 0;
                for _ in 0..n {
                    acc = (acc << w) | pattern;
                    total_w = total_w.saturating_add(w);
                    if total_w >= 63 {
                        break;
                    }
                }
                Ok(acc as i64)
            }
        }
        // Concat `{a, b, c}` — elemen MSB→LSB. Lebar elemen: eksplisit untuk
        // literal bertipe (`4'h0`), bit-length nilai untuk ident/ekspresi lain
        // (mis. `{4'h0, dm::DataCount}` = 8'h02, bukan 2<<4).
        Expr::Concat(elems) => {
            // String concat ({`"hello", `" `", `"world`"}) harus dievaluasi di
            // runtime (byte per char), bukan di-const-fold sebagai bit-pattern
            // (i64 + bit-width merusak urutan byte). Kembalikan Err agar
            // elaborator menurunkan concat biasa → simulator meng-eval dengan
            // benar.
            if elems.iter().any(|e| matches!(e, Expr::String(_))) {
                return Err("string concat is not a constant expression".to_string());
            }
            let mut acc: u64 = 0;
            let mut shift: u32 = 0;
            for elem in elems.iter().rev() {
                let v = const_eval_with_params(elem, param_vals)?;
                // Lebar elemen: literal sized eksplisit → self-determined
                // width ekspresi (mis. `~(!(x))` = 1 bit) → fallback
                // bit-length nilai.
                let w: u32 = match elem {
                    Expr::Value(Value::Hex { bits, width, .. }) => {
                        width.unwrap_or(bits.len() * 4).max(1) as u32
                    }
                    Expr::Value(Value::Binary { bits, width, .. }) => {
                        width.unwrap_or(bits.len()).max(1) as u32
                    }
                    Expr::Value(Value::Octal { bits, width, .. }) => {
                        width.unwrap_or(bits.len() * 3).max(1) as u32
                    }
                    _ => sized_width(elem)
                        .map(|w| w as u32)
                        .unwrap_or((64u32 - (v as u64).leading_zeros()).max(1)),
                };
                // Elemen/kumulasi melebihi 63 bit → jangan fold diam-diam.
                // Cek PAKAI lebar mentah — dulu `w.min(63)` dipakai sebelum
                // cek sehingga elemen 64-bit lolos lalu kehilangan MSB-nya
                // (ditemukan fuzzer seed=823576: `{64'h…, 64'h…}`).
                // Kembalikan Err agar elaborator membangun IR Concat runtime
                // yang mengevaluasi pada lebar penuh.
                if w == 0 || shift + w > 63 {
                    return Err("concat wider than 63 bits is not constant-foldable".to_string());
                }
                acc |= ((v as u64) & ((1u64 << w).wrapping_sub(1))).wrapping_shl(shift.min(63));
                shift = shift.saturating_add(w);
                if shift >= 63 {
                    break;
                }
            }
            Ok(acc as i64)
        }
        // OpenTitan entropy_src_pkg::bucket_ht_data_width(w) = min(w, 4)
        // (BucketHtDataMaxWidth = 4) — lebar data per bucket health-test.
        Expr::FuncCall { name, args, .. }
            if base_func_name(name.as_str()) == "bucket_ht_data_width" =>
        {
            let w = const_eval_with_params(
                args.first().ok_or("bucket_ht_data_width needs 1 arg")?,
                param_vals,
            )?;
            Ok(if w >= 4 { 4 } else { w })
        }
        // OpenTitan otbn_pkg::SecAddRandWidth(w) = 2 * ($clog2(w) * w + 1) —
        // lebar randomness untuk otbn_sec_add / otbn_mask_accelerator
        // (localparam `RandWidth = SecAddRandWidth(Width)`).
        Expr::FuncCall { name, args, .. } if base_func_name(name.as_str()) == "SecAddRandWidth" => {
            let w = const_eval_with_params(
                args.first().ok_or("SecAddRandWidth needs 1 arg")?,
                param_vals,
            )?;
            let clog = if w <= 1 {
                1
            } else {
                63 - (w as u64).leading_zeros() as i64
            };
            Ok(2 * (clog * w + 1))
        }
        // OpenTitan entropy_src_pkg::num_bucket_ht_inst(w) =
        // ceil_div(w, bucket_ht_data_width(w)) — jumlah instance bucket
        // (dipakai generate for di entropy_src.sv).
        Expr::FuncCall { name, args, .. }
            if base_func_name(name.as_str()) == "num_bucket_ht_inst" =>
        {
            let w = const_eval_with_params(
                args.first().ok_or("num_bucket_ht_inst needs 1 arg")?,
                param_vals,
            )?;
            let b = if w >= 4 { 4 } else { w };
            if b == 0 {
                return Err("division by zero in num_bucket_ht_inst".to_string());
            }
            Ok(if w % b != 0 { w / b + 1 } else { w / b })
        }
        Expr::FuncCall { name, args, .. } if name == "$bits" || name == "$size" => {
            if let Some(arg) = args.first() {
                const_eval_with_params(arg, param_vals)
            } else {
                Ok(0)
            }
        }
        Expr::FuncCall { name, .. } if name.starts_with("$") => Err(format!(
            "cannot evaluate system function '{}' in constant context",
            name
        )),
        _ => Err(format!(
            "non-constant expression in parameter context: {:?}",
            expr
        )),
    }
}

/// Bangun key lookup untuk base sebuah member access: `name` untuk Ident,
/// `name[idx]` untuk BitSelect konstanta, `name[r][c]` untuk BitSelect 2D.
/// Dipakai `const_eval_with_params` pada `Expr::MemberAccess` untuk mencari
/// key ter-flatten `name[idx].field` di param_vals.
fn expr_base_key(
    expr: &Expr,
    param_vals: &std::collections::HashMap<Symbol, i64>,
) -> Option<String> {
    match expr {
        Expr::Ident { name, .. } => Some(name.as_str().to_string()),
        Expr::BitSelect { expr: inner, index } => {
            let base = expr_base_key(inner, param_vals)?;
            let idx = const_eval_with_params(index, param_vals).ok()?;
            Some(format!("{}[{}]", base, idx))
        }
        _ => None,
    }
}
