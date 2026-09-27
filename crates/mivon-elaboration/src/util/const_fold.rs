//! ──────────────────────────────────────────────────────────────────────────────
//! CATATAN: File ini adalah bagian dari pemisahan util.rs (SRP Refactoring).
//! Tanggung jawab: Constant folding & value-to-LogicVec conversion.
//!
//! Fungsi:
//!   - const_eval_params()   — evaluasi konstanta dengan parameter
//!   - try_fold_const()      — coba fold expression ke IrExpr::Const
//!   - value_to_logicvec()   — konversi Value AST ke LogicVec IR
//!
//! ──────────────────────────────────────────────────────────────────────────────

use std::collections::HashMap;

use mivon_ast::types::const_eval_with_params;
use mivon_ast::*;
use mivon_core::intern::Symbol;
use mivon_ir::*;

/// Evaluasi expression konstanta dengan parameter yang diberikan.
/// Delegasi ke const_eval_with_params dari ast/types.
pub fn const_eval_params(expr: &Expr, params: &HashMap<Symbol, i64>) -> Result<i64, String> {
    const_eval_with_params(expr, params)
}

/// Fold SELURUH RHS konstanta langsung pada lebar konteks target.
///
/// Masalah yang diselesaikan: fold bertingkat menghasilkan Const pada lebar
/// self-determined sub-ekspresi (mis. `-((cmp))` → 1 bit), sehingga negasi
/// terlanjur dihitung di lebar sempit sebelum zero-extension ke target —
/// `wire [31:0] y = -((-(32'h...) >= !(32'h...)))` memberi 1 alih-alih
/// ffffffff (ditemukan fuzzer seed=59498908; LRM §11.8.1: operator
/// context-determined dievaluasi pada lebar konteks).
///
/// Evaluasi nilai via `const_eval_with_params` (kini unsigned-correct untuk
/// perbandingan), lalu simpan sebagai Const selebar `ctx` (zero/truncate —
/// assignment ke target melakukan hal yang sama). None bila bukan konstanta
/// murni atau ctx == 0.
pub fn try_fold_const_at_width(
    expr: &Expr,
    params: &HashMap<Symbol, i64>,
    ctx: usize,
) -> Option<IrExpr> {
    if ctx == 0 {
        return None;
    }
    // Konstanta selebar LHS RAKSASA (array memori flatten / ctx 45M-1e9 bit):
    // `LogicVec::from_u64(val, ctx)` materialisasi penuh → puncak heap elab
    // (heaptrack AetherX: 134M/3 calls via apply_lhs_context_width). Fold
    // hanya optimisasi — di-skip: ekspresi tetap, engine menangani lebar saat
    // runtime (lazy part).
    if ctx >= mivon_core::LogicVec::LAZY_ZERO_THRESHOLD {
        return None;
    }
    let val = const_eval_with_params(expr, params).ok()?;
    // Literal >64-bit: const_eval i64 memotong bit tinggi — jangan fold
    // (wide_fuzz seed=11).
    if contains_wide_literal(expr) {
        return None;
    }
    // String tidak boleh di-fold sebagai bit-pattern (lihat try_fold_const).
    if matches!(expr, Expr::String(_)) {
        return None;
    }
    // Fill literal (`'0`, `'1`, `'x`, `'z`) context-determined: nilai i64
    // (`FillLit => 0`) kehilangan semantik fill — `'1` harus jadi all-ones
    // selebar LHS di runtime, bukan 0.
    if matches!(expr, Expr::FillLit(_)) {
        return None;
    }
    // Literal yang memuat digit X/Z tak terrepresentasi di i64
    // (`parse_literal` mengganti x/z → 0) — fold di sini menghilangkan X/Z
    // (mis. `b = 4'b10xz` diam-diam jadi 4'b1000). Biarkan jalur
    // `value_to_logicvec` yang memetakan digit X/Z dengan benar.
    if contains_xz_literal(expr) {
        return None;
    }
    // System function ($bits/$size/$clog2/…) BUKAN konstanta nilai-argumen:
    // `const_eval_with_params($bits(X)` mengembalikan NILAI X (mis. 220),
    // bukan jumlah bit — memakai nilai itu sebagai hasil fold merusak semantik
    // (regresi `$bits(COEFFS)` = 220 padahal 128). Biarkan arm SysFunc di
    // elaborate_expr yang menangani dengan benar.
    if contains_sysfunc(expr) {
        return None;
    }
    // Replikasi `{N{e}}`: konversi i64 kehilangan LEBAR POLA operan
    // (approximation `63.min(n)` salah utk pola multi-bit — ditemukan fuzzer:
    // `{7{16'hca3b}}` ter-fold jadi sampah). Fold hanya aman via
    // IrExpr::Replicate yang dirender elaborate_expr dan dievaluasi engine.
    if matches!(expr, Expr::Replicate { .. }) {
        return None;
    }
    // Ekspresi yang memuat `~x` / unary `-x` TIDAK boleh di-fold pada jalur
    // ini: inversi/negasi context-determined dievaluasi const_eval pada
    // lebar ≥32 lalu dibandingkan/dipotong di sini → nilai salah untuk
    // konteks sempit (ditemukan fuzzer seed=15107620: `(2'b11 > ~(2'b10))`
    // harusnya 1). Jalur runtime + propagasi konteks yang benar.
    if contains_ctx_sensitive_unary(expr) {
        return None;
    }
    // Rantai shift (Shl/Shr/Sshl/Sshr) TIDAK boleh di-fold: const_eval
    // menghitung pada i64 penuh TANPA masking intermediate ke lebar bit.
    // `(4'sh4 << 1) >>> 4'hd` → i64: 8>>13=0, tapi 4-bit: 4'b1000 >>> 13
    // → sign-fill → 4'b1111=0xf (shift_chain_fuzz). Biarkan engine yang
    // menangani width masking per-step.
    if contains_shift_op(expr) {
        return None;
    }
    let masked = (val as u64)
        & if ctx >= 64 {
            u64::MAX
        } else {
            (1u64 << ctx) - 1
        };
    // Preserve signedness: ekspresi signed menghasilkan IrExpr::Signed
    // (bukan Const) agar engine tahu `>>>` = arithmetic (sign-fill).
    // Tanpa ini `((4'sh4 << 1) >>> 4'hd)` salah jadi 0 padahal 0xf
    // (shift_chain_fuzz — is_signed_expr(Const) = false).
    let lv = LogicVec::from_u64(masked, ctx);
    Some(if expr_is_signed(expr) {
        IrExpr::Signed(Box::new(IrExpr::Const(lv)))
    } else {
        IrExpr::Const(lv)
    })
}

/// Cek apakah AST expression bernilai signed (LRM §6.11 / §11.8.2).
fn expr_is_signed(e: &Expr) -> bool {
    match e {
        Expr::Value(Value::Decimal(_)) => true,
        Expr::Value(Value::Binary { is_signed, .. })
        | Expr::Value(Value::Hex { is_signed, .. })
        | Expr::Value(Value::Octal { is_signed, .. }) => *is_signed,
        Expr::Paren(inner) => expr_is_signed(inner),
        Expr::UnaryOp { op, expr: inner } => match op {
            UnaryOp::Not
            | UnaryOp::ReductionAnd
            | UnaryOp::ReductionNand
            | UnaryOp::ReductionOr
            | UnaryOp::ReductionNor
            | UnaryOp::ReductionXor
            | UnaryOp::ReductionXnor => false,
            _ => expr_is_signed(inner),
        },
        Expr::BinaryOp { op, lhs, rhs } => match op {
            BinaryOp::Shl | BinaryOp::Shr | BinaryOp::Sshl | BinaryOp::Sshr => expr_is_signed(lhs),
            BinaryOp::Eq
            | BinaryOp::Neq
            | BinaryOp::Lt
            | BinaryOp::Le
            | BinaryOp::Gt
            | BinaryOp::Ge
            | BinaryOp::LogicalAnd
            | BinaryOp::LogicalOr => false,
            _ => expr_is_signed(lhs) && expr_is_signed(rhs),
        },
        _ => false,
    }
}

/// Deteksi pemanggilan system function (`$bits`, `$size`, …) di subtree —
/// nilai konstanta i64 tidak merepresentasikan hasilnya.
fn contains_sysfunc(e: &Expr) -> bool {
    match e {
        Expr::FuncCall { name, args, .. } if name.starts_with("$") => true,
        Expr::FuncCall { args, .. } => args.iter().any(contains_sysfunc),
        Expr::Paren(inner) => contains_sysfunc(inner),
        Expr::UnaryOp { expr: inner, .. } => contains_sysfunc(inner),
        Expr::BinaryOp { lhs, rhs, .. } => contains_sysfunc(lhs) || contains_sysfunc(rhs),
        Expr::Concat(elems) => elems.iter().any(contains_sysfunc),
        Expr::Replicate { count, expr: inner } => {
            contains_sysfunc(count) || contains_sysfunc(inner)
        }
        Expr::TernaryOp {
            cond,
            true_expr,
            false_expr,
        } => contains_sysfunc(cond) || contains_sysfunc(true_expr) || contains_sysfunc(false_expr),
        _ => false,
    }
}

/// Deteksi literal based (Binary/Hex/Octal) yang memuat digit `x`/`z` —
/// nilainya tidak bisa direpresentasikan sebagai i64.
fn contains_xz_literal(e: &Expr) -> bool {
    let lit_has_xz = |bits: &str| {
        bits.chars()
            .any(|c| matches!(c, 'x' | 'X' | 'z' | 'Z' | '?'))
    };
    match e {
        Expr::Value(Value::Binary { bits, .. }) | Expr::Value(Value::Octal { bits, .. }) => {
            lit_has_xz(bits)
        }
        Expr::Value(Value::Hex { bits, .. }) => lit_has_xz(bits),
        Expr::FillLit(_) => true,
        Expr::Paren(inner) => contains_xz_literal(inner),
        Expr::UnaryOp { expr: inner, .. } => contains_xz_literal(inner),
        Expr::BinaryOp { lhs, rhs, .. } => contains_xz_literal(lhs) || contains_xz_literal(rhs),
        Expr::Concat(elems) => elems.iter().any(contains_xz_literal),
        Expr::Replicate { expr: inner, .. } => contains_xz_literal(inner),
        Expr::TernaryOp {
            cond,
            true_expr,
            false_expr,
        } => {
            contains_xz_literal(cond)
                || contains_xz_literal(true_expr)
                || contains_xz_literal(false_expr)
        }
        _ => false,
    }
}

/// Deteksi subtree `~x` / unary `-x` (context-determined, sensitif lebar).
/// Apakah subtree memuat literal berpola >64-bit? Aritmetika konstan di
/// `const_eval_with_params` berjalan pada i64 — fold subtree semacam itu
/// memotong bit tinggi diam-diam (ditemukan wide_fuzz seed=11:
/// `(96'sh… * 96'sh…) | …` ter-fold salah 32 bit atas). Jalur runtime
/// (evaluator u128) menangani dengan benar → jangan fold.
fn contains_wide_literal(e: &Expr) -> bool {
    match e {
        Expr::Value(Value::Binary { bits, .. }) => bits.len() > 64,
        Expr::Value(Value::Hex { bits, .. }) => bits.len() * 4 > 64,
        Expr::Value(Value::Octal { bits, .. }) => bits.len() * 3 > 64,
        Expr::Paren(inner) => contains_wide_literal(inner),
        Expr::BinaryOp { lhs, rhs, .. } => contains_wide_literal(lhs) || contains_wide_literal(rhs),
        Expr::UnaryOp { expr: inner, .. } => contains_wide_literal(inner),
        Expr::TernaryOp {
            cond,
            true_expr,
            false_expr,
        } => {
            contains_wide_literal(cond)
                || contains_wide_literal(true_expr)
                || contains_wide_literal(false_expr)
        }
        Expr::Concat(elems) => elems.iter().any(contains_wide_literal),
        Expr::Replicate { expr: inner, .. } => contains_wide_literal(inner),
        _ => false,
    }
}

/// Deteksi ekspresi yang mengandung operator shift (<<, >>, <<<, >>>).
/// Shift harus dievaluasi di engine dengan width masking per-step, bukan
/// di const_eval i64 penuh (ditemukan shift_chain_fuzz: rantai shift
/// menghasilkan 0 alih-alih all-ones karena intermediate tidak di-mask).
fn contains_shift_op(e: &Expr) -> bool {
    match e {
        Expr::BinaryOp {
            op: BinaryOp::Shl | BinaryOp::Shr | BinaryOp::Sshl | BinaryOp::Sshr,
            ..
        } => true,
        Expr::BinaryOp { lhs, rhs, .. } => contains_shift_op(lhs) || contains_shift_op(rhs),
        Expr::Paren(inner) => contains_shift_op(inner),
        Expr::UnaryOp { expr: inner, .. } => contains_shift_op(inner),
        Expr::TernaryOp {
            cond,
            true_expr,
            false_expr,
        } => {
            contains_shift_op(cond) || contains_shift_op(true_expr) || contains_shift_op(false_expr)
        }
        Expr::Concat(elems) => elems.iter().any(contains_shift_op),
        Expr::Replicate { count, expr: inner } => {
            contains_shift_op(count) || contains_shift_op(inner)
        }
        _ => false,
    }
}

fn contains_ctx_sensitive_unary(e: &Expr) -> bool {
    match e {
        Expr::UnaryOp {
            op: UnaryOp::BitNot | UnaryOp::Minus,
            ..
        } => true,
        // Reduction & `!` juga context-sensitive pada lebar OPERAN: hasil
        // bergantung lebar self-determined operan yang mungkin tak-diketahui
        // struktur (TernaryOp/Ident). Fold di i64 penuh salah —
        // `&(2'b11)` = 1 pada 2 bit tapi 0 pada 64 (ditemukan guided_fuzz
        // seed=47753038; emas + Icarus: jalur runtime yang benar).
        Expr::UnaryOp {
            op:
                UnaryOp::Not
                | UnaryOp::ReductionAnd
                | UnaryOp::ReductionNand
                | UnaryOp::ReductionOr
                | UnaryOp::ReductionNor
                | UnaryOp::ReductionXor
                | UnaryOp::ReductionXnor,
            ..
        } => true,
        Expr::UnaryOp { expr: inner, .. } | Expr::Paren(inner) => {
            contains_ctx_sensitive_unary(inner)
        }
        Expr::BinaryOp { lhs, rhs, .. } => {
            contains_ctx_sensitive_unary(lhs) || contains_ctx_sensitive_unary(rhs)
        }
        Expr::Concat(elems) => elems.iter().any(contains_ctx_sensitive_unary),
        Expr::Replicate { expr: inner, .. } => contains_ctx_sensitive_unary(inner),
        Expr::TernaryOp {
            cond,
            true_expr,
            false_expr,
        } => {
            contains_ctx_sensitive_unary(cond)
                || contains_ctx_sensitive_unary(true_expr)
                || contains_ctx_sensitive_unary(false_expr)
        }
        _ => false,
    }
}

/// Perkiraan lebar sebenarnya dari ekspresi konstanta murni (tanpa signal),
/// dihitung dari struktur AST — bukan dari nilai hasil eval.
///
/// Latar belakang: `try_fold_const` mengevaluasi nilai lalu memakai lebar
/// minimal `min_width.max(32)`. Untuk konstanta yang nilainya kecil/0 tapi
/// lebar aslinya lebar (mis. `{384{1'b0}}` → 0, atau `512'h0 ^ 512'h36`),
/// lebar itu salah menjadi 32 — memicu WR0102 palsu seperti
/// `i_pad_256 = {secret_key_i[1023:896], {(BlockSizeSHA256-128){1'b0}}}`
/// (rhs dihitung 160 = 128+32, harusnya 512). Dengan lebar dari AST, hasil
/// fold mempertahankan lebar sebenarnya.
pub(crate) fn const_fold_width(expr: &Expr, params: &HashMap<Symbol, i64>) -> Option<usize> {
    match expr {
        Expr::Value(v) => Some(match v {
            Value::Binary { bits, width, .. } => width.unwrap_or_else(|| bits.len()),
            Value::Hex { bits, width, .. } => width.unwrap_or_else(|| bits.len() * 4),
            Value::Octal { bits, width, .. } => width.unwrap_or_else(|| bits.len() * 3),
            Value::Decimal(n) => {
                let abs = n.unsigned_abs();
                let w = if *n == 0 {
                    1
                } else {
                    64 - abs.leading_zeros() as usize
                };
                w.max(32)
            }
            Value::Real(_) => 64,
        }),
        Expr::FillLit(_) => Some(1),
        Expr::Replicate { count, expr: inner } => {
            let c = const_eval_with_params(count, params).ok()? as usize;
            Some(c.saturating_mul(const_fold_width(inner, params)?))
        }
        Expr::Concat(exprs) => {
            let mut total = 0usize;
            for e in exprs {
                total = total.saturating_add(const_fold_width(e, params)?);
            }
            Some(total)
        }
        Expr::Paren(inner) => const_fold_width(inner, params),
        Expr::CastWidth { width, expr: _ } => {
            let w = const_eval_with_params(width, params).ok()?;
            Some(w.max(1) as usize)
        }
        Expr::String(s) => Some(s.len() * 8),
        Expr::UnaryOp { op, expr: inner } => {
            // Reduction & `!`: hasil SELF-DETERMINED 1-bit (LRM §11.8.1) —
            // tidak membutuhkan lebar operan yang mungkin tak-kethitungan
            // (operan berisi signal). Tanpa ini seluruh subtree lewati
            // (None) dan try_fold_const jatuh ke fallback `.max(32)`:
            // `2'b00 ? ^(x) : 2'b11` ter-fold jadi Const 32-bit 0x3 lalu
            // `&(…)` = 0 padahal 1 (ditemukan guided_fuzz seed=47753038,
            // dikonfirmasi Icarus).
            match op {
                UnaryOp::Not
                | UnaryOp::ReductionAnd
                | UnaryOp::ReductionNand
                | UnaryOp::ReductionOr
                | UnaryOp::ReductionNor
                | UnaryOp::ReductionXor
                | UnaryOp::ReductionXnor => return Some(1),
                _ => {}
            }
            let w = const_fold_width(inner, params)?;
            Some(w)
        }
        Expr::BinaryOp { op, lhs, rhs } => {
            // Perbandingan & logical: hasil SELF-DETERMINED 1-bit (LRM
            // §11.8.2 Tabel 11-21) — tidak bergantung pada keterhitungan
            // lebar operan. Tanpa ini operan non-konst membuat lebar None →
            // fallback `.max(32)` di try_fold_const → hasil fold "1"
            // termaterialisasi sebagai pola 32-bit 0x…001 yang merusak
            // reduction induk (`&(x || 64'h1)` = 0 padahal 1; ditemukan
            // guided_fuzz seed=1688221143996, dikonfirmasi Icarus).
            if matches!(
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
            ) {
                return Some(1);
            }
            let lw = const_fold_width(lhs, params)?;
            let rw = const_fold_width(rhs, params)?;
            let m = lw.max(rw);
            Some(match op {
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
                | BinaryOp::LogicalOr => 1,
                _ => m,
            })
        }
        Expr::TernaryOp {
            cond: _,
            true_expr,
            false_expr,
        } => {
            // Saat const-fold, kondisi diketahui -> hanya cabang yang
            // diambil yang penting. Jika salah satu branch width
            // tidak diketahui (mis. signal Ident -> None), kembalikan
            // None agar `try_fold_const` jatuh ke fallback `.max(32)`.
            // Sebelumnya: `(Some(t), None) => Some(t)` — BUG: `&(1 ? 1'b1 : b)`
            // ter-fold jadi Const(1, w=1) lalu &(1'b1)=1 padahal harusnya
            // Cond evaluator menghitung max(1,2)=2 lalu &(2'b01)=0 (ditemukan
            // guided_fuzz, dikonfirmasi Icarus).
            let tw = const_fold_width(true_expr, params);
            let fw = const_fold_width(false_expr, params);
            match (tw, fw) {
                (Some(t), Some(f)) => Some(t.max(f)),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Coba fold expression konstanta menjadi IrExpr::Const.
/// Mengembalikan Some(Const) jika konstanta bisa dievaluasi, None jika tidak.
pub fn try_fold_const(
    expr: &Expr,
    params: &HashMap<Symbol, i64>,
) -> Result<Option<IrExpr>, String> {
    // Jangan fold subtree yang memuat `~x` / unary `-x`: kedua operator
    // itu context-determined (LRM §11.8.1) dan nilai i64 hasil
    // const_eval tidak membawa lebar — `-(1'b1) == 1'b1` salah ter-fold
    // sebagai -1==1 (ditemukan fuzzer seed=177443644 / seed=197975).
    // Jalur runtime + propagasi konteks (Cast) yang menangani benar.
    if contains_ctx_sensitive_unary(expr) {
        return Ok(None);
    }
    // Jangan fold subtree dengan literal >64-bit: const_eval berjalan di
    // i64 → bit tinggi terpotong diam-diam (wide_fuzz seed=11). Jalur
    // runtime u128 yang benar.
    if contains_wide_literal(expr) {
        return Ok(None);
    }
    // Rantai shift TIDAK boleh di-fold: const_eval menghitung pada i64
    // penuh TANPA masking intermediate ke lebar bit. `(4'sh4 << 1) >>>
    // 4'hd` → i64: 8>>13=0, tapi 4-bit: 4'b1000 >>> 13 → sign-fill
    // → 4'b1111=0xf (shift_chain_fuzz).
    if contains_shift_op(expr) {
        return Ok(None);
    }
    // Replikasi `{N{e}}`: konversi i64 kehilangan LEBAR POLA operan
    // (approximation `63.min(n)` di const_eval_with_params salah utk pola
    // multi-bit). Biarkan IrExpr::Replicate — engine mereplikasi bit-pattern
    // asli dengan benar.
    if matches!(expr, Expr::Replicate { .. }) {
        return Ok(None);
    }
    // Jangan fold relational comparison (Lt/Le/Gt/Ge) untuk ekspresi unsigned
    // karena const_eval_with_params memakai i64 signed arithmetic yang salah
    // untuk unsigned semantics (mis. 187 < -111 sebagai i64 = false, tapi
    // sebagai unsigned 16-bit = 187 < 65425 = true).
    if is_relational_comparison(expr) && !const_expr_is_signed(expr) {
        return Ok(None);
    }

    // Untuk operasi Div/Mod pada ekspresi unsigned, perlu unsigned semantics.
    // const_eval_with_params selalu pakai signed i64. Kalau unsigned,
    // hitung manual dengan u64.
    if !const_expr_is_signed(expr) {
        if let Some(val) = try_fold_const_unsigned(expr, params)? {
            let width = const_fold_width(expr, params).unwrap_or_else(|| {
                let abs = val;
                let min_width = if val == 0 {
                    1
                } else {
                    64 - (abs.leading_zeros() as usize)
                };
                min_width.max(32)
            });
            let lv = LogicVec::from_u64(val, width);
            return Ok(Some(IrExpr::Const(lv)));
        }
    }

    match const_eval_with_params(expr, params) {
        Ok(val) => {
            let width = const_fold_width(expr, params).unwrap_or_else(|| {
                let abs = val as u64;
                let min_width = if val >= 0 {
                    if val == 0 {
                        1
                    } else {
                        64 - (abs.leading_zeros() as usize)
                    }
                } else {
                    64 - (abs.leading_zeros() as usize) + 1
                };
                min_width.max(32)
            });
            let lv = LogicVec::from_u64(val as u64, width);
            // ROUND 36: signedness hasil fold mengikuti SIGNEDNESS OPERAND
            // ASLI (LRM §11.8.2 'ada operand unsigned → hasil unsigned'),
            // bukan sekadar `val < 0` (yang salah utk `8'h01 - 8'h05` =
            // -4 padahal operand unsigned):
            //   - `-5` / `2 + 3` (desimal unsized) → Signed → `a < -5`,
            //     `a < (2+3)` signed compare benar
            //   - `8'h01 - 8'h05` → unsigned → compare unsigned (LRM)
            // Konsisten dengan `Expr::Value` di elaborate_expr.
            if const_expr_is_signed(expr) {
                Ok(Some(IrExpr::Signed(Box::new(IrExpr::Const(lv)))))
            } else {
                Ok(Some(IrExpr::Const(lv)))
            }
        }
        Err(_) => Ok(None),
    }
}

fn try_fold_const_unsigned(
    expr: &Expr,
    params: &HashMap<Symbol, i64>,
) -> Result<Option<u64>, String> {
    match expr {
        Expr::BinaryOp { op, lhs, rhs } => {
            let l = match const_eval_with_params(lhs, params) {
                Ok(v) => v as u64,
                Err(_) => return Ok(None),
            };
            let r = match const_eval_with_params(rhs, params) {
                Ok(v) => v as u64,
                Err(_) => return Ok(None),
            };
            match op {
                BinaryOp::Div => {
                    if r == 0 {
                        return Err("division by zero in constant expression".to_string());
                    }
                    Ok(Some(l / r))
                }
                BinaryOp::Mod => {
                    if r == 0 {
                        return Err("modulo by zero in constant expression".to_string());
                    }
                    Ok(Some(l % r))
                }
                _ => Ok(None),
            }
        }
        _ => Ok(None),
    }
}

fn is_relational_comparison(expr: &Expr) -> bool {
    matches!(expr, Expr::BinaryOp { op, .. } if matches!(op, BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge))
}

/// Signedness ekspresi KONSTAN — signed bila SEMUA literal operand signed
/// (desimal unsized = signed §6.8.1, atau suffix `s`); ada satu literal
/// unsigned (Binary/Hex/Octal tanpa `s`) → hasil unsigned (any-unsigned
/// §11.8.2). `Ident`/param/`$system` dll. tidak dilacak → konservatif
/// unsigned (keterbatasan: `localparam int P=5; a < P` dianggap unsigned).
pub fn const_expr_is_signed(expr: &Expr) -> bool {
    match expr {
        Expr::Value(Value::Decimal(_)) => true,
        Expr::Value(Value::Binary { is_signed, .. })
        | Expr::Value(Value::Hex { is_signed, .. })
        | Expr::Value(Value::Octal { is_signed, .. }) => *is_signed,
        Expr::Paren(inner) => const_expr_is_signed(inner),
        Expr::UnaryOp { op, expr: inner } => {
            match op {
                // Unary minus on unsigned stays unsigned (SV: -unsigned = unsigned)
                // Only signed if inner is a signed decimal literal
                UnaryOp::Minus => matches!(inner.as_ref(), Expr::Value(Value::Decimal(_))),
                // Other unary ops propagate signedness
                _ => const_expr_is_signed(inner),
            }
        }
        Expr::BinaryOp { op, lhs, rhs } => {
            // LRM §11.8.2 Tabel 11-21: hasil perbandingan & logical SELALU
            // unsigned (1-bit), apa pun signedness operandnya. Tanpa ini
            // fold `(4'sd5 < 4'sd7)` menghasilkan `Signed(Const(1))` —
            // engine menandai seluruh sub-tree sebagai signed dan
            // perbandingan induk jalan bertanda padahal mixed-unsigned
            // (ditemukan fuzzer signed_fuzz seed=18/24/84/111).
            if matches!(
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
            ) {
                return false;
            }
            const_expr_is_signed(lhs) && const_expr_is_signed(rhs)
        }
        Expr::TernaryOp {
            true_expr,
            false_expr,
            ..
        } => const_expr_is_signed(true_expr) && const_expr_is_signed(false_expr),
        _ => false,
    }
}

/// Konversi Value AST (Decimal, Binary, Hex, Octal, Real) ke LogicVec IR.
/// LRM 1800-2017 §5.7.1: bila digit paling kiri (MSB) literal bertanda
/// x/z/? — nilai diperluas ke atas dengan x/z (`2'bx` = 2'bxx, `4'hz` =
/// 4'bzzzz) — bukan zero-extend. F30 zero-extend membuat `2'bx` → 2'b0x
/// dan `1'bx` (lebar 1) hilang X-nya saat dilebarkan konteks assign
/// (ditemukan mivon-fuzz: literal X dalam multi-driver assign berubah
/// jadi 0 → hasil sim beda antara jalur serial vs paralel).
fn extend_leftmost_xz(vec: &mut LogicVec, digits: &str, bits_per_digit: usize) {
    let Some(first) = digits.chars().next() else {
        return;
    };
    let fill = match first {
        'z' | 'Z' => LogicVal::Z,
        'x' | 'X' | '?' => LogicVal::X,
        _ => return, // MSB pasti 0/1 → zero-extend (F30, sudah benar)
    };
    let placed = digits.len().saturating_mul(bits_per_digit);
    if placed >= vec.width {
        return;
    }
    for b in vec.bits.iter_mut().skip(placed) {
        *b = fill;
    }
}

/// Konversi `Value` AST ke `LogicVec` IR.
pub fn value_to_logicvec(val: &Value) -> LogicVec {
    match val {
        Value::Decimal(n) => {
            let abs = n.unsigned_abs();
            let min_width = if *n == 0 {
                1
            } else {
                64 - (abs.leading_zeros() as usize)
            };
            let width = min_width.max(32);
            let mut lv = LogicVec::from_u64(abs, width);
            if *n < 0 {
                for b in lv.bits.iter_mut() {
                    *b = match b {
                        LogicVal::Zero => LogicVal::One,
                        LogicVal::One => LogicVal::Zero,
                        _ => LogicVal::X,
                    };
                }
                let mut carry = true;
                for b in lv.bits.iter_mut() {
                    if carry {
                        match b {
                            LogicVal::Zero => {
                                *b = LogicVal::One;
                                carry = false;
                            }
                            LogicVal::One => {
                                *b = LogicVal::Zero;
                            }
                            _ => {}
                        }
                    }
                }
            }
            lv
        }
        Value::Binary { bits, width, .. } => {
            // F30 fix review: filter `_` dulu (konsisten dengan Hex/Octal) —
            // kalau di-loop langsung, enumerate menghitung index underscore
            // sehingga digit berikutnya bergeser (mis. `8'b1_010` → salah).
            let digits: String = bits.chars().filter(|c| *c != '_').collect();
            let w = width.unwrap_or(digits.len());
            // F30 fix: literal sized di-zero-extend — bit di atas digit bernilai
            // 0 (bukan X). Sebelumnya `LogicVec::new(w)` (fill X) membuat
            // `16'h6` = 16'hxxxx...0006 → perbandingan `sig == 16'h6` salah
            // (X di operand → hasil X → false). `from_u64` sudah zero-fill;
            // Binary/Hex/Octal harus konsisten.
            let mut vec = LogicVec::fill(LogicVal::Zero, w);
            // `fill` meng-clamp width absurd → iterasi wajib pakai lebar aktual
            // vec (bukan w mentah), else index-out-of-bounds.
            let vw = vec.width;
            for (i, c) in digits.chars().rev().enumerate() {
                if i >= vw {
                    break;
                }
                vec.bits[i] = match c {
                    '0' => LogicVal::Zero,
                    '1' => LogicVal::One,
                    'x' | 'X' => LogicVal::X,
                    'z' | 'Z' => LogicVal::Z,
                    _ => LogicVal::X,
                };
            }
            extend_leftmost_xz(&mut vec, &digits, 1);
            vec
        }
        Value::Hex { bits, width, .. } => {
            let w = width.unwrap_or(bits.len() * 4);
            let mut vec = LogicVec::fill(LogicVal::Zero, w);
            let vw = vec.width;
            let digits: String = bits.chars().filter(|c| *c != '_').collect();
            for (i, c) in digits.chars().rev().enumerate() {
                // F30 fix: digit x/z eksplisit (mis. `8'hx6`) → bit X/Z,
                // bukan 0 (sebelumnya `to_digit(16)` → None → unwrap_or(0)).
                let val = match c {
                    'x' | 'X' => LogicVal::X,
                    'z' | 'Z' => LogicVal::Z,
                    _ => {
                        let hv = c.to_digit(16).unwrap_or(0);
                        for j in 0..4 {
                            let bit_idx = i * 4 + j;
                            if bit_idx >= vw {
                                break;
                            }
                            vec.bits[bit_idx] = if (hv >> j) & 1 == 1 {
                                LogicVal::One
                            } else {
                                LogicVal::Zero
                            };
                        }
                        continue;
                    }
                };
                for j in 0..4 {
                    let bit_idx = i * 4 + j;
                    if bit_idx >= vw {
                        break;
                    }
                    vec.bits[bit_idx] = val;
                }
            }
            extend_leftmost_xz(&mut vec, &digits, 4);
            vec
        }
        Value::Octal { bits, width, .. } => {
            let w = width.unwrap_or(bits.len() * 3);
            let mut vec = LogicVec::fill(LogicVal::Zero, w);
            let vw = vec.width;
            let digits: String = bits.chars().filter(|c| *c != '_').collect();
            for (i, c) in digits.chars().rev().enumerate() {
                // F30 fix: digit x/z eksplisit di octal → bit X/Z.
                let val = match c {
                    'x' | 'X' => LogicVal::X,
                    'z' | 'Z' => LogicVal::Z,
                    _ => {
                        let ov = c.to_digit(8).unwrap_or(0);
                        for j in 0..3 {
                            let bit_idx = i * 3 + j;
                            if bit_idx >= vw {
                                break;
                            }
                            vec.bits[bit_idx] = if (ov >> j) & 1 == 1 {
                                LogicVal::One
                            } else {
                                LogicVal::Zero
                            };
                        }
                        continue;
                    }
                };
                for j in 0..3 {
                    let bit_idx = i * 3 + j;
                    if bit_idx >= vw {
                        break;
                    }
                    vec.bits[bit_idx] = val;
                }
            }
            extend_leftmost_xz(&mut vec, &digits, 3);
            vec
        }
        Value::Real(r) => LogicVec::from_u64(r.to_bits(), 64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mivon_ast::Value;

    /// F30 fix: literal sized di-zero-extend — bit di atas digit bernilai 0,
    /// BUKAN X. Sebelumnya `16'h6` = 16'hxxxx...0006 sehingga
    /// `signal == 16'h6` selalu false (X di operand → hasil X → display 0).
    #[test]
    fn value_to_logicvec_zero_extends_hex() {
        let lv = value_to_logicvec(&Value::Hex {
            bits: "6".into(),
            width: Some(16),
            is_signed: false,
        });
        assert_eq!(lv.width, 16);
        assert_eq!(lv.to_u64(), 6);
        // bit 4..=15 harus Zero (bukan X)
        assert!(!lv.bits[4..]
            .iter()
            .any(|b| matches!(b, LogicVal::X | LogicVal::Z)));
        assert_eq!(lv.bits[0], LogicVal::Zero);
        assert_eq!(lv.bits[1], LogicVal::One);
        assert_eq!(lv.bits[2], LogicVal::One);
        assert_eq!(lv.bits[3], LogicVal::Zero);
    }

    #[test]
    fn value_to_logicvec_zero_extends_binary_and_octal() {
        let bin = value_to_logicvec(&Value::Binary {
            bits: "110".into(),
            width: Some(8),
            is_signed: false,
        });
        assert_eq!(bin.width, 8);
        assert_eq!(bin.to_u64(), 6);
        assert!(!bin.bits[3..]
            .iter()
            .any(|b| matches!(b, LogicVal::X | LogicVal::Z)));

        let oct = value_to_logicvec(&Value::Octal {
            bits: "6".into(),
            width: Some(9),
            is_signed: false,
        });
        assert_eq!(oct.width, 9);
        assert_eq!(oct.to_u64(), 6);
        assert!(!oct.bits[3..]
            .iter()
            .any(|b| matches!(b, LogicVal::X | LogicVal::Z)));
    }

    /// F30 fix review: underscore di literal binary tidak boleh menggeser
    /// posisi digit (`8'b1_010` = 10, bukan bit[4] terisi karena underscore).
    #[test]
    fn value_to_logicvec_binary_underscore_no_shift() {
        let lv = value_to_logicvec(&Value::Binary {
            bits: "1_010".into(),
            width: Some(8),
            is_signed: false,
        });
        assert_eq!(
            lv.to_u64(),
            0b1010,
            "underscore harus diabaikan: {:?}",
            lv.bits
        );
        assert_eq!(lv.bits[4], LogicVal::Zero); // posisi underscore = 0
    }

    /// F30: digit X/Z eksplisit tetap dipertahankan (tidak di-zero-kan).
    #[test]
    fn value_to_logicvec_keeps_explicit_x() {
        let lv = value_to_logicvec(&Value::Hex {
            bits: "x6".into(),
            width: Some(8),
            is_signed: false,
        });
        assert_eq!(lv.bits[4], LogicVal::X); // digit x eksplisit
        assert_eq!(lv.bits[1], LogicVal::One);
        assert_eq!(lv.bits[2], LogicVal::One);
    }
}
