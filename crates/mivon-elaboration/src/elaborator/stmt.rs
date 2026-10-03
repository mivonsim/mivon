use super::super::util::*;
use super::Elaborator;
use mivon_ast::types::const_eval_with_params;
use mivon_ast::*;
use mivon_core::diagnostics::diagnostic::DiagCode;
use mivon_core::diagnostics::suggest::suggest_name;
use mivon_core::error::SimError;
use mivon_core::intern::Symbol;
use mivon_ir::*;
use std::collections::HashMap;

/// Extract SignalId from IrLValue, if it's a simple signal reference.
/// Apakah ekspresi memuat literal biner/oktal/heksadesimal dengan digit
/// `x`/`z`/`?` (unknown)? Dipakai untuk MENGAWANGI const-fold `case`: nilai
/// 4-state tak bisa di-folder ke i64 tanpa kehilangan informasi unknown
/// (LRM 1800 §12.5).
pub(crate) fn literal_has_unknown_bits(expr: &Expr) -> bool {
    match expr {
        Expr::Value(Value::Binary { bits, .. })
        | Expr::Value(Value::Hex { bits, .. })
        | Expr::Value(Value::Octal { bits, .. }) => bits
            .chars()
            .any(|c| matches!(c, 'x' | 'X' | 'z' | 'Z' | '?')),
        Expr::Paren(inner) => literal_has_unknown_bits(inner),
        _ => false,
    }
}

/// Apakah nama tipe (untuk cast `T'(x)`) menyatakan real/realtime?
pub(crate) fn is_real_type_name(name: &str) -> bool {
    matches!(name, "real" | "realtime" | "shortreal")
}

/// Buang lapisan `Paren` — untuk mengenali literal real di balik `(1.5)`.
pub(crate) fn strip_parens(mut e: &Expr) -> &Expr {
    while let Expr::Paren(inner) = e {
        e = inner;
    }
    e
}

/// Apakah ekspresi AST bertipe real? (sinyal real / literal real / `$itor` /
/// cast real) — dipakai untuk memilih jalur konversi cast real↔integer.
pub(crate) fn expr_is_real_ty(
    expr: &Expr,
    signal_map: &HashMap<Symbol, SignalId>,
    signals: &[SignalInfo],
) -> bool {
    match expr {
        Expr::Value(Value::Real(_)) => true,
        Expr::Paren(inner) => expr_is_real_ty(inner, signal_map, signals),
        Expr::Cast { dtype, .. } => is_real_type_name(dtype.as_str()),
        Expr::Ident { name, .. } if name.as_str() == "$itor" => true,
        Expr::FuncCall { name, .. } if name.as_str() == "$itor" => true,
        Expr::Ident { name, .. } => signal_map
            .get(name)
            .and_then(|sid| signals.get(*sid))
            .map(|s| s.is_real)
            .unwrap_or(false),
        _ => false,
    }
}

/// Apakah ekspresi IR bertipe real? (sinyal real, `RealConst`, `$itor`,
/// atau hasil operasi yang salah satu operand real) — dipakai untuk konversi
/// implisit real→integer saat assignment (LRM 1800 §6.24.3).
pub(crate) fn ir_expr_is_real_ty(e: &IrExpr, signals: &[SignalInfo]) -> bool {
    match e {
        IrExpr::RealConst(_) => true,
        IrExpr::Signal(id, _) => signals.get(*id).map(|s| s.is_real).unwrap_or(false),
        IrExpr::SysFunc { name, .. } => matches!(name.as_str(), "$itor" | "$bitstoreal"),
        IrExpr::Cast { expr, .. } | IrExpr::Signed(expr) => ir_expr_is_real_ty(expr, signals),
        IrExpr::UnaryOp(_, inner)
        | IrExpr::ExprBitSelect(inner, _)
        | IrExpr::ExprRangeSelect(inner, ..) => ir_expr_is_real_ty(inner, signals),
        IrExpr::BinaryOp(op, a, b) => {
            // Hanya operator ARITMETIKA yang mempertahankan tipe real.
            // Perbandingan/logical menghasilkan 1-bit (0/1) — membungkusnya
            // dengan konversi real→integer membuat `gt = (a > b)` jadi 0.
            if !matches!(
                op,
                BinaryIrOp::Add
                    | BinaryIrOp::Sub
                    | BinaryIrOp::Mul
                    | BinaryIrOp::Div
                    | BinaryIrOp::Mod
                    | BinaryIrOp::Power
            ) {
                return false;
            }
            ir_expr_is_real_ty(a, signals) || ir_expr_is_real_ty(b, signals)
        }
        IrExpr::Cond(_, a, b) => ir_expr_is_real_ty(a, signals) || ir_expr_is_real_ty(b, signals),
        _ => false,
    }
}

pub(crate) fn lvalue_signal_id(lv: &IrLValue) -> Option<SignalId> {
    match lv {
        IrLValue::Signal(id, _) => Some(*id),
        IrLValue::RangeSelect(id, _, _) => Some(*id),
        IrLValue::BitSelect(id, _) => Some(*id),
        IrLValue::ArrayIndex { sig_id, .. } => Some(*sig_id),
        IrLValue::ArrayRangeSelect { sig_id, .. } => Some(*sig_id),
        IrLValue::ArrayBitSelect { sig_id, .. } => Some(*sig_id),
        IrLValue::ExprPartSelect { sig_id, .. } => Some(*sig_id),
        IrLValue::ObjectField { sig_id, .. } => Some(*sig_id),
        IrLValue::HierRef(_) | IrLValue::HierRefIndex { .. } => None,
        IrLValue::Concat(items) => items.first().and_then(lvalue_signal_id),
    }
}

/// Lebar total sebuah lvalue (untuk context-width RHS).
/// Concat = jumlah lebar semua part (LRM §11.8.1 concat lvalue context size).
pub(crate) fn lvalue_width(lv: &IrLValue, signals: &[SignalInfo]) -> usize {
    match lv {
        IrLValue::RangeSelect(_, hi, lo) => hi.saturating_sub(*lo).saturating_add(1),
        IrLValue::Concat(items) => items
            .iter()
            .map(|p| {
                lvalue_signal_id(p)
                    .and_then(|sid| signals.get(sid))
                    .map(|s| s.width)
                    .unwrap_or(0)
            })
            .sum(),
        _ => lvalue_signal_id(lv)
            .and_then(|sid| signals.get(sid))
            .map(|s| s.width)
            .unwrap_or(0),
    }
}

/// Ambil nilai konstanta dari IrExpr bila berupa literal — dipakai untuk
/// part-select width yang tidak ter-fold saat elaborasi (base dinamis seperti
/// `sig[i*32 +: 32]` → width tetap `Const(32)`).
fn ir_const_u64(e: &IrExpr) -> Option<u64> {
    match e {
        IrExpr::Const(lv) => Some(lv.to_u64()),
        _ => None,
    }
}

/// Deskripsi singkat ekspresi untuk pesan "invalid lvalue" — tanpa Debug repr
/// (`String("x")` bocor ke user). Menampilkan bentuk yang bisa dibaca manusia.
fn describe_lvalue_expr(expr: &Expr) -> String {
    match expr {
        Expr::String(s) => format!("string literal \"{s}\""),
        Expr::Value(v) => format!("literal {v:?}"),
        Expr::FillLit(_) => "fill literal ('0/'1/'x/'z)".to_string(),
        Expr::Ident { name, .. } => name.as_str().to_string(),
        Expr::UnaryOp { op, .. } => format!("{op:?} expression"),
        Expr::Concat(_) => "concatenation {…}".to_string(),
        Expr::Replicate { .. } => "replication {n{…}}".to_string(),
        Expr::TernaryOp { .. } => "ternary ?: expression".to_string(),
        Expr::BinaryOp { op, .. } => format!("{op:?} expression"),
        Expr::FuncCall { .. } => "function call".to_string(),
        Expr::MethodCall { .. } => "method call".to_string(),
        Expr::Null => "null".to_string(),
        _ => "expression".to_string(),
    }
}

/// Apakah konstanta (u64) muat dalam `w` bit — unsigned ATAU sebagai nilai
/// negatif two's complement (sign bit di lebar asli konstanta). Dipakai untuk
/// context sizing literal unsized: `cnt + 1` (1 default 32-bit) seharusnya
/// selebar operan lain (4-bit), bukan membuat ekspresi jadi 32-bit.
fn const_fits_in(lv: &LogicVec, w: usize) -> bool {
    if w == 0 {
        return true;
    }
    if w >= 64 {
        return true;
    }
    let raw = lv.to_u64();
    if raw < (1u64 << w) {
        return true;
    }
    // Nilai negatif: sign bit terpasang di lebar asli konstanta → interpretasi
    // signed (mis. -1 = 0xFFFFFFFF) muat dalam rentang signed `w` bit.
    if lv.width >= 2 && lv.width <= 64 {
        let sign_bit = 1u64 << (lv.width - 1);
        if raw & sign_bit != 0 {
            let mask = if lv.width >= 64 {
                u64::MAX
            } else {
                (1u64 << lv.width) - 1
            };
            let signed = (raw | !mask) as i64;
            let min = -(1i64 << (w - 1));
            let max = (1i64 << (w - 1)) - 1;
            if signed >= min && signed <= max {
                return true;
            }
        }
    }
    false
}

/// Lebar operan dalam konteks operasi biner: konstanta unsized (width >= 32,
/// literal default SV) yang nilainya muat di lebar operan lain → mengambil
/// lebar operan lain (context-determined sizing). Mencegah false-positive
/// `cnt <= cnt + 1` (rhs dihitung 32 padahal idiom umum 4-bit counter).
///
/// `own_w` = lebar `e` yang SUDAH dihitung pemanggil. Untuk operand
/// non-konstanta hasilnya persis `own_w` — sehingga subtree tidak pernah
/// di-traverse ulang. Sebelumnya di sini memanggil `expr_approx_width(e)`
/// lagi, yang membuat tiap `BinaryOp`/`Cond` menelusuri kedua operand dua
/// kali → eksponensial O(2^n) pada rantai ekspresi dalam.
fn context_width(e: &IrExpr, own_w: usize, other_w: usize) -> usize {
    if let IrExpr::Const(lv) = e {
        if lv.width >= 32 && const_fits_in(lv, other_w) {
            return other_w;
        }
        return lv.width;
    }
    own_w
}

/// Compute approximate width of an IrExpr at elaboration time (best-effort).
fn expr_approx_width(expr: &IrExpr, signals: &[SignalInfo]) -> usize {
    match expr {
        IrExpr::Const(lv) => lv.width,
        IrExpr::FillLit(_) => 1,
        IrExpr::Signal(id, _) => signals.get(*id).map(|s| s.width).unwrap_or(1),
        IrExpr::RangeSelect(_, hi, lo) => hi.saturating_sub(*lo).saturating_add(1),
        IrExpr::BitSelect(_, _) => 1,
        IrExpr::ExprRangeSelect(_, hi, lo) => hi.saturating_sub(*lo).saturating_add(1),
        IrExpr::ExprBitSelect(_, _) => 1,
        // Part-select dinamis (`sig[base +: width]`): lebar = argumen `width`
        // (biasanya konstanta/param yang sudah di-fold).
        IrExpr::ExprPartSelect(_, _, width) => ir_const_u64(width).map(|w| w as usize).unwrap_or(1),
        IrExpr::ArrayIndex { elem_width, .. } => *elem_width,
        IrExpr::Concat(items) => {
            // Saturasi: item dengan lebar tak-sah (mis. part-select hi meluap
            // dari underflow width konstanta, `IDW-STIDW` di top_pkg unresolved)
            // berujung usize::MAX → `.sum()` panic "attempt to add with overflow"
            // (ditemukan mivon-fuzz: panic pada concat tlul_socket `shifted_id`).
            // lebar di sini HANYA perkiraan utk konteks sizing — jangan panic.
            items.iter().fold(0usize, |acc, e| {
                acc.saturating_add(expr_approx_width(e, signals))
            })
        }
        IrExpr::Replicate(n, inner) => n.saturating_mul(expr_approx_width(inner, signals)),
        // Unary logika & reduksi menghasilkan 1 bit (`!x`, `&x`, `|x`, `^x`);
        // sisanya (aritmetika, bitwise) selebar operand.
        IrExpr::UnaryOp(op, inner) => match op {
            UnaryIrOp::Not
            | UnaryIrOp::RedAnd
            | UnaryIrOp::RedNand
            | UnaryIrOp::RedOr
            | UnaryIrOp::RedNor
            | UnaryIrOp::RedXor
            | UnaryIrOp::RedXnor => 1,
            _ => expr_approx_width(inner, signals),
        },
        IrExpr::BinaryOp(op, a, b) => {
            let wa = expr_approx_width(a, signals);
            let wb = expr_approx_width(b, signals);
            // Perbandingan (`==`, `<`, ...) & logika (`&&`, `||`) → 1 bit;
            // shift → selebar lhs; aritmetika/bitwise → max operand.
            match op {
                BinaryIrOp::Eq
                | BinaryIrOp::Neq
                | BinaryIrOp::CaseEq
                | BinaryIrOp::CaseNeq
                | BinaryIrOp::EqWild
                | BinaryIrOp::NeqWild
                | BinaryIrOp::Lt
                | BinaryIrOp::Le
                | BinaryIrOp::Gt
                | BinaryIrOp::Ge
                | BinaryIrOp::LogicalAnd
                | BinaryIrOp::LogicalOr => 1,
                // Shift: SV result width = left operand width (LRM §11.4.10)
                BinaryIrOp::Shl | BinaryIrOp::Shr | BinaryIrOp::Sshl | BinaryIrOp::Sshr => wa,
                _ => context_width(a, wa, wb).max(context_width(b, wb, wa)),
            }
        }
        IrExpr::Cond(_, a, b) => {
            let wa = expr_approx_width(a, signals);
            let wb = expr_approx_width(b, signals);
            context_width(a, wa, wb).max(context_width(b, wb, wa))
        }
        IrExpr::Signed(inner) => expr_approx_width(inner, signals),
        IrExpr::Cast { width, .. } => *width,
        IrExpr::String(s) => s.len() * 8,
        IrExpr::DpiCall { return_width, .. } => *return_width,
        IrExpr::StreamingConcat { slices, .. } => {
            slices.iter().map(|e| expr_approx_width(e, signals)).sum()
        }
        IrExpr::Inside { .. } => 1,
        // Fungsi matematika real → 64-bit (bit-pattern f64, sama `$itor`).
        // `$rtoi` → 32-bit integer.
        IrExpr::SysFunc { name, .. }
            if matches!(
                name.as_str(),
                "$itor" | "$bitstoreal" | "$ln" | "$log10" | "$exp" | "$sqrt" | "$pow"
                    | "$floor" | "$ceil" | "$round" | "$sin" | "$cos" | "$tan"
                    | "$asin" | "$acos" | "$atan" | "$atan2" | "$hypot" | "$sinh"
                    | "$cosh" | "$tanh" | "$asinh" | "$acosh" | "$atanh"
            ) =>
        {
            64
        }
        IrExpr::SysFunc { name, .. } if name.as_str() == "$rtoi" => 32,
        _ => 1,
    }
}

/// Signedness ekspresi IR (LRM §11.8.2 Tabel 11-21) — cermin
/// `mivon_simulator::simulator::util::is_signed_expr` untuk dipakai saat
/// propagasi konteks lebar di elaborator:
/// - perbandingan & logical → SELALU unsigned;
/// - shift → mengikuti operan kiri saja;
/// - operator lain → signed bila KEDUA operan signed.
pub(crate) fn expr_ir_is_signed(e: &IrExpr, signals: &[SignalInfo]) -> bool {
    match e {
        IrExpr::Signed(_) => true,
        IrExpr::Signal(id, _) | IrExpr::BitSelect(id, _) | IrExpr::RangeSelect(id, ..) => {
            signals.get(*id).map(|s| s.is_signed).unwrap_or(false)
        }
        IrExpr::ArrayIndex { sig_id, .. } => {
            signals.get(*sig_id).map(|s| s.is_signed).unwrap_or(false)
        }
        IrExpr::BinaryOp(op, l, r) => {
            if matches!(
                op,
                BinaryIrOp::Eq
                    | BinaryIrOp::Neq
                    | BinaryIrOp::CaseEq
                    | BinaryIrOp::CaseNeq
                    | BinaryIrOp::EqWild
                    | BinaryIrOp::NeqWild
                    | BinaryIrOp::Lt
                    | BinaryIrOp::Le
                    | BinaryIrOp::Gt
                    | BinaryIrOp::Ge
                    | BinaryIrOp::LogicalAnd
                    | BinaryIrOp::LogicalOr
            ) {
                return false;
            }
            if matches!(
                op,
                BinaryIrOp::Shl | BinaryIrOp::Shr | BinaryIrOp::Sshl | BinaryIrOp::Sshr
            ) {
                return expr_ir_is_signed(l, signals);
            }
            expr_ir_is_signed(l, signals) && expr_ir_is_signed(r, signals)
        }
        IrExpr::UnaryOp(_, inner) => expr_ir_is_signed(inner, signals),
        IrExpr::Cond(_, t, f) => expr_ir_is_signed(t, signals) || expr_ir_is_signed(f, signals),
        _ => false,
    }
}

/// Propagasi lebar konteks (LRM §11.8.1 "context-determined") ke dalam tree
/// IR RHS assignment. Engine evaluasi bottom-up tanpa info target; operand
/// dari operator context-determined (unary ±/~, aritmetika, bitwise, dan
/// operand kiri shift) harus di-zero-extend SEBELUM dievaluasi — kalau tidak
/// `y = -((cmp <<< 0))` dihitung di lebar 1 bit dan hasilnya salah.
///
/// Mekanisme: node yang self-determined-nya lebih sempit dari konteks tapi
/// berada di posisi context-determined dibungkus `Cast{width}` (engine
/// `resize` = zero-extend unsigned). Node `Signed` tidak pernah dibungkus.
///
/// Ditemukan fuzzer terarah (seed 57554764 / 59498908), divalidasi
/// differential vs Icarus.
pub(crate) fn propagate_context_width(e: &mut IrExpr, ctx: usize, signals: &[SignalInfo]) -> usize {
    // Bantu: bungkus node dengan Cast{width: target} bila self-width < target.
    fn wrap_cast(e: &mut IrExpr, target: usize, signals: &[SignalInfo]) -> usize {
        let w = expr_approx_width(e, signals);
        if target > w && !matches!(e, IrExpr::Signed(_) | IrExpr::FillLit(_)) {
            let inner = std::mem::replace(e, IrExpr::FillLit(mivon_core::logic::LogicVal::X));
            *e = IrExpr::Cast {
                width: target,
                expr: Box::new(inner),
            };
            return target;
        }
        w
    }

    match e {
        IrExpr::Const(lv) => {
            if ctx > lv.width && lv.width <= 64 && ctx <= 64 {
                // JANGAN lewat `to_u64()`: bit X/Z literal ikut hilang
                // (X/Z → 0) saat Const dilebarkan konteks assign
                // (`assign y = 1'bx` dgn y [1:0] menjadi 2'b00 padahal
                // harus 2'b0x — ditemukan mivon-fuzz). Zero-extend per-bit,
                // pertahankan bit X/Z yang sudah ada.
                let mut bits = lv.bits.clone();
                bits.resize(ctx, LogicVal::Zero);
                *lv = LogicVec { bits, width: ctx };
            }
            lv.width
        }
        IrExpr::FillLit(_) | IrExpr::String(_) | IrExpr::Signed(_) => expr_approx_width(e, signals),
        IrExpr::Signal(..) | IrExpr::RangeSelect(..) | IrExpr::BitSelect(..) => {
            // Sinyal sudah terbaca selebar deklarasinya; cukup kembalikan.
            expr_approx_width(e, signals)
        }
        IrExpr::UnaryOp(op, inner) => match op {
            UnaryIrOp::Not
            | UnaryIrOp::RedAnd
            | UnaryIrOp::RedNand
            | UnaryIrOp::RedOr
            | UnaryIrOp::RedNor
            | UnaryIrOp::RedXor
            | UnaryIrOp::RedXnor => {
                let w = expr_approx_width(inner, signals);
                propagate_context_width(inner, w, signals);
                // Self-determined 1-bit; pembungkusan jadi tugas parent.
                wrap_cast(e, ctx, signals)
            }
            // Minus/BitNot/Plus context-determined → turunkan konteks,
            // lalu pastikan OPERAN selebar konteks di runtime (Cast),
            // karena hasil unary = lebar operan pasca-extension.
            _ => {
                let wi = propagate_context_width(inner, ctx.max(1), signals);
                if wi < ctx {
                    match inner.as_mut() {
                        IrExpr::FillLit(_) | IrExpr::Signed(_) => {}
                        _ => {
                            let old = std::mem::replace(
                                inner.as_mut(),
                                IrExpr::FillLit(mivon_core::logic::LogicVal::X),
                            );
                            **inner = IrExpr::Cast {
                                width: ctx,
                                expr: Box::new(old),
                            };
                        }
                    }
                    return ctx;
                }
                wi
            }
        },
        IrExpr::BinaryOp(op, a, b) => {
            let wa0 = expr_approx_width(a, signals);
            let wb0 = expr_approx_width(b, signals);
            match op {
                BinaryIrOp::Eq
                | BinaryIrOp::Neq
                | BinaryIrOp::CaseEq
                | BinaryIrOp::CaseNeq
                | BinaryIrOp::EqWild
                | BinaryIrOp::NeqWild
                | BinaryIrOp::Lt
                | BinaryIrOp::Le
                | BinaryIrOp::Gt
                | BinaryIrOp::Ge
                | BinaryIrOp::LogicalAnd
                | BinaryIrOp::LogicalOr => {
                    // LRM Table 11-21: comparison operands are
                    // context-determined to max of BOTH operand widths.
                    // Assignment context does NOT flow into operands —
                    // only the operands size each other. E.g.
                    // `~(1'b1) < 65'hX`: ~(1'b1) sized to max(1,65)=65
                    // but `2-bit_expr < 2-bit_expr` stays 2-bit.
                    // Bug: using ctx.max(wb0) leaked assignment width
                    // into shift, widening ~(2'b10) from 2 to 8/32.
                    let cmp_ctx = wb0.max(wa0);
                    let _wa1 = propagate_context_width(a, cmp_ctx, signals);
                    let _wb1 = propagate_context_width(b, cmp_ctx, signals);
                    wrap_cast(e, ctx, signals)
                }
                BinaryIrOp::Shl | BinaryIrOp::Shr | BinaryIrOp::Sshl => {
                    // RHS shift self-determined — TIDAK menyumbang konteks
                    // untuk lhs (literal unsized 8 = "32-bit" jangan
                    // menggelembungkan lebar inner-shift; LRM §11.8.1 hasil
                    // shift = lebar operan kiri). xprop_fuzz seed=39.
                    let _wb = propagate_context_width(b, wb0, signals);
                    let wa1 = propagate_context_width(a, ctx.max(wa0), signals);
                    if wa1 >= ctx {
                        wa1
                    } else {
                        wrap_cast(a, ctx, signals);
                        ctx
                    }
                }
                BinaryIrOp::Sshr => {
                    // >>> ARITHMETIC bila lhs signed (IEEE 1800 §11.4.10).
                    // lhs SIGNED TIDAK boleh di-cast: wrap_cast zero-extend
                    // merusak sign bit (`logic signed [7:0] s = -128;
                    // rs = s >>> 2` salah jadi 0x20 padahal 0xE0) dan
                    // is_signed_expr(Cast) = false memaksa jalur logical.
                    // Runtime menangani sign-fill ke lebar konteks
                    // (evaluate_expr_ctx + eval_sshr_signed pada lebar asli
                    // lhs). lhs UNSIGNED → >>> identik logis dan lhs tetap
                    // context-determined (§11.8.1): propagasi konteks +
                    // wrap_cast seperti Shl/Shr, kalau tidak sub-ekspresi
                    // `-(cmp)` / `~x` dihitung pada lebar self-determined
                    // sempit lalu hasilnya salah (ditemukan fuzzer
                    // signed_fuzz seed=125; emas + Icarus: -(1) selebar
                    // konteks = all-ones).
                    let _wb = propagate_context_width(b, wb0, signals);
                    if expr_ir_is_signed(a, signals) {
                        wa0
                    } else {
                        let wa1 = propagate_context_width(a, ctx.max(wa0), signals);
                        if wa1 >= ctx {
                            wa1
                        } else {
                            wrap_cast(a, ctx, signals);
                            ctx
                        }
                    }
                }
                _ => {
                    // Aritmetika/bitwise: kedua operand context-determined,
                    // hasil = max(kedua operand, KONTEKS) — LRM §11.8.1.
                    let wa1 = propagate_context_width(a, ctx.max(wb0), signals);
                    let wb1 = propagate_context_width(b, ctx.max(wa1), signals);
                    let w = wa1.max(wb1).max(ctx);
                    // Operand yang tetap lebih sempit dari lebar operasi
                    // WAJIB dibungkus Cast SEBELUM operasi — propagate saja
                    // tidak cukup untuk operand Signal/BitSelect (arm-nya
                    // tidak pernah membungkus cast), sehingga op bitwise
                    // jalan pada lebar sempit lalu hasilnya di-extend
                    // terlambat (`b ^~ b[8]` dgn b 16-bit, konteks 32 →
                    // salah; divalidasi Icarus, guided_fuzz seed=850564).
                    if wa1 < w {
                        wrap_cast(a, w, signals);
                    }
                    if wb1 < w {
                        wrap_cast(b, w, signals);
                    }
                    w
                }
            }
        }
        IrExpr::Cond(_, ta, fa) => {
            let wt = propagate_context_width(ta, ctx, signals);
            let wf = propagate_context_width(fa, ctx, signals);
            wt.max(wf)
        }
        IrExpr::Inside { expr, list } => {
            // LRM §11.4.13 + §11.8.1: lhs dan tiap item `inside`
            // context-determined — operan saling di-extend ke max(konteks,
            // lebar lhs, lebar item terbesar). Tanpa ini `~(b[15]) inside
            // {a[2]}` dievaluasi 1-bit self-determined (salah; divalidasi
            // differential vs Icarus, metamorphic fuzz seed=300922).
            let mut wm = propagate_context_width(expr, ctx.max(1), signals);
            for item in list.iter_mut() {
                let wi = propagate_context_width(item, ctx.max(wm), signals);
                wm = wm.max(wi);
            }
            let _ = propagate_context_width(expr, ctx.max(wm), signals);
            // Hasil inside 1-bit; pembungkusan ke konteks tugas parent.
            wrap_cast(e, ctx, signals)
        }
        IrExpr::Replicate(_count, inner) => {
            // Replikasi SELF-DETERMINED (LRM §11.8.1) — lebar konteks luar
            // TIDAK masuk. Tapi tetap harus DESCEND ke body dengan konteks
            // lebar body sendiri: op context-determined bersarang (perbandingan,
            // unary ±/~) saling di-size antar operan di dalam body. Tanpa
            // descend, `~x > y` di dalam `{N{...}}` dievaluasi pada lebar
            // sempit `x` (ditemukan guided_fuzz seed=17861824; emas +
            // Icarus: operan comparison di-extend ke lebar operan terlebar).
            let wi0 = expr_approx_width(inner, signals).max(1);
            let _wi = propagate_context_width(inner, wi0, signals);
            expr_approx_width(e, signals)
        }
        IrExpr::Concat(items) => {
            // Elemen concat SELF-DETERMINED (LRM §11.8.1) — konteks luar
            // TIDAK boleh masuk (wrap_cast ke konteks melipatgandakan lebar
            // elemen → concat ter-truncate; regression `{(b<a), !(...)}`
            // golden 0x3 mivon 0x1). Descend hanya dengan lebar elemen
            // sendiri agar op context-determined INTERNAL saling di-size.
            for it in items.iter_mut() {
                let wi = expr_approx_width(it, signals).max(1);
                let _ = propagate_context_width(it, wi, signals);
            }
            expr_approx_width(e, signals)
        }
        _ => expr_approx_width(e, signals),
    }
}

/// LRM §12.5: ekspresi `case` dan SELURUH label di-size mutual ke lebar
/// terlebar SEBELUM evaluasi — op context-determined bersarang pada
/// selector (mis. `~(^(...))`) mengubah NILAI berdasarkan lebar akhir.
/// Runtime memang melakukan zero-extension saat perbandingan, tapi itu
/// terlambat: extension pasca-evaluasi tidak mengubah nilai op yang sudah
/// dihitung pada lebar sempit (`~0` 1-bit = 1, padahal `~0` selebar item =
/// all-ones). Ditemukan fuzz case (seed=139372796, divalidasi Icarus).
pub(crate) fn apply_case_context_width(
    ir_expr: &mut IrExpr,
    items: &mut [IrCaseItem],
    signals: &[SignalInfo],
) {
    let mut wm = expr_approx_width(ir_expr, signals);
    for item in items.iter() {
        for l in &item.labels {
            wm = wm.max(expr_approx_width(l, signals));
        }
    }
    propagate_context_width(ir_expr, wm, signals);
    for item in items.iter_mut() {
        for l in item.labels.iter_mut() {
            let _ = propagate_context_width(l, wm, signals);
        }
    }
}

/// Check signedness mismatch between LHS and RHS at elaboration.
fn check_signed_mismatch(lhs_signal_id: Option<SignalId>, rhs: &IrExpr, signals: &[SignalInfo]) {
    let Some(sid) = lhs_signal_id else { return };
    let Some(lhs_sig) = signals.get(sid) else {
        return;
    };
    let is_rhs_signed = matches!(rhs, IrExpr::Signed(_));
    if lhs_sig.is_signed && !is_rhs_signed {
        // Only warn when RHS could be determined at compile time
    }
}

impl Elaborator {
    /// Terapkan lebar konteks LHS ke RHS assignment (LRM §11.8.1):
    /// whole-RHS konstanta di-fold langsung pada lebar LHS; sisanya dapat
    /// propagasi konteks untuk operand context-determined.
    fn apply_lhs_context_width(
        &self,
        ir_lhs: &IrLValue,
        rhs_ast: &Expr,
        ir_rhs: &mut IrExpr,
        _signal_map: &HashMap<Symbol, SignalId>,
        signals: &[SignalInfo],
    ) {
        let lhs_w = match ir_lhs {
            IrLValue::RangeSelect(_, hi, lo) => hi.saturating_sub(*lo).saturating_add(1),
            _ => lvalue_signal_id(ir_lhs)
                .and_then(|sid| signals.get(sid))
                .map(|s| s.width)
                .unwrap_or(0),
        };
        if lhs_w == 0 {
            return;
        }
        // Jangan fold RHS yang mereferensikan param SIGNED: try_fold_const_at_width
        // → const_eval memperlakukan Ident sbg unsigned, sehingga `lt = (N < 0)`
        // (N=localparam int -2) salah ter-fold jadi 0 (probe test_edge_signed_param_negative).
        if super::expr_refs_signed_param(rhs_ast, &self.param_is_signed) {
            propagate_context_width(ir_rhs, lhs_w, signals);
            return;
        }
        if let Some(c) = try_fold_const_at_width(rhs_ast, &self.param_vals, lhs_w) {
            *ir_rhs = c;
        } else {
            propagate_context_width(ir_rhs, lhs_w, signals);
        }
    }

    /// Check width mismatch between LHS signal and RHS expression at elaboration.
    /// Lebar LHS dihitung dari bentuk `IrLValue` aktual (bukan width penuh
    /// signal) — mis. `RangeSelect(sid, msb, lsb)` lebarnya `msb-lsb+1`,
    /// `BitSelect` = 1. Tanpa ini `result[0][0][W-1:0] = ...` (64-bit)
    /// memicu false-positive "lhs=1600" karena memakai width signal penuh.
    fn check_width_mismatch(
        &self,
        lhs: &IrLValue,
        rhs: &IrExpr,
        signals: &[SignalInfo],
        line: usize,
        col: usize,
    ) {
        // Fill literal (`'0`, `'1`, `'x`, `'z`) menyesuaikan lebar konteks
        // LHS (dipanjangkan ke lebar target saat assign) — selalu kompatibel
        // dengan lebar apa pun, jadi jangan pernah warning.
        if matches!(rhs, IrExpr::FillLit(_)) {
            return;
        }
        let Some(sid) = lvalue_signal_id(lhs) else {
            return;
        };
        let Some(lhs_sig) = signals.get(sid) else {
            return;
        };
        // Signal sintetis dari inlining function (temp `__func_*`) atau tanpa
        // lokasi source (line==0): lebar temp best-effort dan runtime selalu
        // menyesuaikan lebar LHS saat assign — warning di sini false-positive.
        if line == 0 || lhs_sig.name.as_str().starts_with("__func_") {
            return;
        }
        let lhs_w = match lhs {
            IrLValue::RangeSelect(_, msb, lsb) => msb.saturating_sub(*lsb).saturating_add(1),
            IrLValue::BitSelect(_, _) => 1,
            IrLValue::ArrayIndex { elem_width, .. } => *elem_width,
            IrLValue::ArrayBitSelect { elem_width, .. } => *elem_width,
            // `mem[i][msb:lsb]` / `rf[i][0+:W]`: lebar seleksi = msb-lsb+1
            // (bukan elemen penuh) — `.max(elem_width)` memicu false-positive
            // mis. `rf[i][0+:ExtWLEN/2]` (lhs dihitung 312, harusnya 156).
            IrLValue::ArrayRangeSelect { msb, lsb, .. } => {
                msb.saturating_sub(*lsb).saturating_add(1)
            }
            _ => lhs_sig.width,
        };
        let rhs_w = expr_approx_width(rhs, signals);
        if lhs_w != rhs_w && rhs_w > 0 {
            // Jangan warning bila RHS konstanta yang nilainya muat di lebar
            // LHS (mis. `result = COEFFS[2]` di mana COEFFS[2] = 3 dalam
            // reg [7:0]; atau `reg signed [7:0] a; a = -1;` — `-1` berlebar
            // 32 sebagai ekspresi tapi nilainya muat). Nilai diekstrak dari
            // Const langsung maupun ekspresi konstanta sederhana
            // (`-1` = UnaryOp(Minus, Const), `-x*2+1`, dsb.) — konstanta
            // negatif hasil fold ROUND 36 dibungkus Signed.
            if let Some(v) = Self::ir_const_value(rhs) {
                let fits = if lhs_sig.is_signed && lhs_w > 0 && lhs_w < 64 {
                    let min = -(1i64 << (lhs_w - 1));
                    let max = (1i64 << (lhs_w - 1)) - 1;
                    v >= min && v <= max
                } else if lhs_w < 64 {
                    v >= 0 && (v as u64) < (1u64 << lhs_w)
                } else {
                    true
                };
                if fits {
                    return;
                }
            }
            self.elab_warn_at(
                DiagCode::WidthMismatchWarning,
                format!(
                    "width mismatch in assignment to '{}' (lhs={}, rhs={})",
                    lhs_sig.name, lhs_w, rhs_w
                ),
                line,
                col,
            );
        }
    }

    /// Ekstrak nilai konstanta dari IrExpr sederhana: `Const`/`Signed(Const)`
    /// langsung, atau operasi unary/binary aritmetika di atas konstanta
    /// (`-1`, `~5`, `(2*3-1)`, …). `None` bila ada sinyal/X/Z/op tak didukung.
    fn ir_const_value(e: &IrExpr) -> Option<i64> {
        match e {
            IrExpr::Const(lv) if lv.width <= 64 => {
                let has_xz = lv.bits.iter().any(|b| {
                    matches!(
                        b,
                        mivon_core::logic::LogicVal::X | mivon_core::logic::LogicVal::Z
                    )
                });
                if has_xz {
                    None
                } else {
                    Some(lv.to_i64())
                }
            }
            // Konstanta negatif hasil fold ROUND 36 dibungkus Signed.
            IrExpr::Signed(inner) => Self::ir_const_value(inner),
            IrExpr::UnaryOp(op, inner) => match op {
                UnaryIrOp::Minus => Self::ir_const_value(inner)?.checked_neg(),
                UnaryIrOp::BitNot => {
                    let v = Self::ir_const_value(inner)?;
                    Some(!v)
                }
                UnaryIrOp::Plus => Self::ir_const_value(inner),
                _ => None,
            },
            IrExpr::BinaryOp(op, a, b) => {
                let (l, r) = (Self::ir_const_value(a)?, Self::ir_const_value(b)?);
                match op {
                    BinaryIrOp::Add => l.checked_add(r),
                    BinaryIrOp::Sub => l.checked_sub(r),
                    BinaryIrOp::Mul => l.checked_mul(r),
                    BinaryIrOp::Div if r != 0 => l.checked_div(r),
                    BinaryIrOp::Mod if r != 0 => l.checked_rem(r),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    pub(crate) fn elaborate_stmt_block(
        &self,
        stmts: &[Stmt],
        signal_map: &HashMap<Symbol, SignalId>,
        _known_modules: &[Symbol],
        signals: &[SignalInfo],
    ) -> Result<Vec<IrStmt>, SimError> {
        let mut ir_stmts = Vec::new();
        for stmt in stmts {
            ir_stmts.push(self.elaborate_stmt(stmt, signal_map, _known_modules, signals)?);
        }
        Ok(ir_stmts)
    }

    /// Elaborasi case dengan CaseType tertentu tanpa const-fold (struktur
    /// dipertahankan). Dipakai qualifier unique/unique0/priority (LANG-16/17).
    #[allow(clippy::too_many_arguments)]
    fn elaborate_case_raw(
        &self,
        expr: &Expr,
        items: &[mivon_ast::CaseItem],
        default: &Option<Box<Stmt>>,
        case_type: CaseType,
        signal_map: &HashMap<Symbol, SignalId>,
        known_modules: &[Symbol],
        signals: &[SignalInfo],
    ) -> Result<IrStmt, SimError> {
        let mut ir_expr = self.elaborate_expr(expr, signal_map, signals)?;
        let mut ir_items = Vec::new();
        for item in items {
            let mut labels = Vec::new();
            for label in &item.labels {
                labels.push(self.elaborate_expr(label, signal_map, signals)?);
            }
            let body = match &*item.stmt {
                Stmt::Block { stmts } => {
                    self.elaborate_stmt_block(stmts, signal_map, known_modules, signals)?
                }
                other => self.elaborate_stmt_block(
                    std::slice::from_ref(other),
                    signal_map,
                    known_modules,
                    signals,
                )?,
            };
            ir_items.push(IrCaseItem { labels, body });
        }
        let ir_default = match default {
            Some(d) => vec![self.elaborate_stmt(d, signal_map, known_modules, signals)?],
            None => vec![],
        };
        apply_case_context_width(&mut ir_expr, &mut ir_items, signals);
        Ok(IrStmt::Case {
            case_type,
            expr: ir_expr,
            items: ir_items,
            default: ir_default,
        })
    }

    /// SIM-29: wrapper `elaborate_stmt` — memanggil inner lalu mencatat baris
    /// statement ke `stmt_lines` (key `format!("{}.{:?}", proc, discriminant)`
    /// SAMA dengan `record_line_hit` di engine). Line diekstrak dari AST via
    /// `stmt_source_line` (expr_location); statement tanpa line (0) tidak
    /// dicatat sehingga tidak ikut ter-exclude.
    fn elaborate_stmt(
        &self,
        stmt: &Stmt,
        signal_map: &HashMap<Symbol, SignalId>,
        known_modules: &[Symbol],
        signals: &[SignalInfo],
    ) -> Result<IrStmt, SimError> {
        let ir = self.elaborate_stmt_inner(stmt, signal_map, known_modules, signals)?;
        if let Some(proc) = self.current_proc_name.borrow().as_ref() {
            let line = stmt_source_line(stmt);
            if line > 0 {
                let key = Symbol::intern(&format!("{}.{:?}", proc, std::mem::discriminant(&ir)));
                self.stmt_lines.borrow_mut().entry(key).or_insert(line);
            }
        }
        Ok(ir)
    }

    /// Konst-eval intra-assignment delay (`= #(d) rhs`, `<= #d rhs`) menjadi
    /// `Option<u64>`. Gagal konst-eval → warning + fallback 1 (sama dengan
    /// `Stmt::Delay`): engine hanya mendukung delay konstan.
    fn elab_intra_assign_delay(&self, delay: &Option<mivon_ast::types::Delay>) -> Option<u64> {
        let d = delay.as_ref()?;
        let expr = d.rise.as_ref().or(d.fall.as_ref()).or(d.turnoff.as_ref())?;
        match const_eval_with_params(expr, &self.param_vals) {
            Ok(v) => Some(v.max(0) as u64),
            Err(e) => {
                let (l, c) = crate::util::generate::expr_location(expr);
                self.elab_warn_at(
                    DiagCode::SimulationError,
                    format!("non-constant intra-assignment delay evaluated as 1: {}", e),
                    l,
                    c,
                );
                Some(1)
            }
        }
    }

    fn elaborate_stmt_inner(
        &self,
        stmt: &Stmt,
        signal_map: &HashMap<Symbol, SignalId>,
        known_modules: &[Symbol],
        signals: &[SignalInfo],
    ) -> Result<IrStmt, SimError> {
        // ── Instrumentasi DBG_STMT (profiling, non-persistent): hitung total
        // statement node + waktu per konstruk. Dipakai untuk menemukan
        // bottleneck statement elaboration (bukan bagian dari build). ──
        let dbg_stmt = std::env::var("DBG_STMT").is_ok();
        let stmt_t0 = if dbg_stmt {
            Some(std::time::Instant::now())
        } else {
            None
        };
        let stmt_kind = stmt_kind_name(stmt);
        let out = match stmt {
            Stmt::Block { stmts } => {
                let body = self.elaborate_stmt_block(stmts, signal_map, known_modules, signals)?;
                Ok(IrStmt::Block { stmts: body })
            }
            Stmt::BlockingAssign { lhs, rhs, delay } => {
                let ir_lhs = self.elaborate_lvalue(lhs, signal_map, signals)?;
                // Check if LHS is a virtual interface signal
                let is_vif_lhs = match &ir_lhs {
                    IrLValue::Signal(sid, _) => signals
                        .get(*sid)
                        .map(|s| s.iface_type.is_some())
                        .unwrap_or(false),
                    _ => false,
                };
                let mut ir_rhs = if is_vif_lhs {
                    // For vif binding, RHS might be an instance name (not a signal)
                    match rhs {
                        Expr::Ident { name, .. } if !signal_map.contains_key(name) => {
                            // Vif binding: store instance name for runtime resolution
                            IrExpr::VifBinding {
                                instance_name: *name,
                            }
                        }
                        _ => self.elaborate_expr(rhs, signal_map, signals)?,
                    }
                } else {
                    self.elaborate_expr(rhs, signal_map, signals)?
                };
                // Struct assignment pattern `p = '{hi: 4'hA, lo: 4'h5}` —
                // layout struct hanya diketahui di level statement (LHS),
                // sedangkan arm `Expr::StructLit` di `elaborate_expr` tak bisa
                // me-resolve tipe (jadi FillLit 0 = silent wrong).
                if let Some(ir_pattern) =
                    self.elaborate_struct_pattern(&ir_lhs, rhs, signal_map, signals)?
                {
                    ir_rhs = ir_pattern;
                }
                let struct_pattern = matches!(rhs, Expr::StructLit { .. });
                // Konversi implisit real→integer saat assignment (LRM 1800
                // §6.24.3: nilai real dibulatkan ke integer terdekat ketika
                // disimpan ke variabel integer). PRA-FIX hanya resize bit →
                // `integer k = 7.9` menjadi bit-pattern f64 (2576980378).
                if let Some(sid) = lvalue_signal_id(&ir_lhs) {
                    let lhs_is_real = signals.get(sid).map(|s| s.is_real).unwrap_or(false);
                    if !lhs_is_real && ir_expr_is_real_ty(&ir_rhs, signals) {
                        ir_rhs = IrExpr::SysFunc {
                            name: Symbol::intern("$rtoi"),
                            args: vec![ir_rhs],
                            line: 0,
                            col: 0,
                        };
                    }
                }
                // `apply_lhs_context_width` (lihat arm NonBlockingAssign):
                // pola struct tak boleh dilewatkan const-fold `Expr::StructLit`
                // (tak tahu layout → 0), lebar pola sudah pas.
                // Fill in class name for new() calls from LHS signal info
                if let IrExpr::NewCall {
                    ref mut class_name, ..
                } = ir_rhs
                {
                    if class_name.is_empty() {
                        if let IrLValue::Signal(sid, _) = ir_lhs {
                            if let Some(sig) = signals.get(sid) {
                                if let Some(cn) = &sig.class_name {
                                    *class_name = *cn;
                                }
                            }
                        }
                    }
                }
                let lhs_sid = lvalue_signal_id(&ir_lhs);
                // Propagasi lebar konteks LHS → operand context-determined
                // RHS (LRM §11.8.1) sebelum evaluasi runtime.
                let _at = std::time::Instant::now();
                let _at = std::time::Instant::now();
                if !struct_pattern {
                    self.apply_lhs_context_width(&ir_lhs, rhs, &mut ir_rhs, signal_map, signals);
                }
                self.dbg_apply_us
                    .set(self.dbg_apply_us.get() + _at.elapsed().as_micros() as u64);
                self.dbg_apply_n.set(self.dbg_apply_n.get() + 1);
                self.dbg_apply_us
                    .set(self.dbg_apply_us.get() + _at.elapsed().as_micros() as u64);
                self.dbg_apply_n.set(self.dbg_apply_n.get() + 1);
                let (lhs_line, lhs_col) = expr_location(lhs);
                self.check_width_mismatch(&ir_lhs, &ir_rhs, signals, lhs_line, lhs_col);
                check_signed_mismatch(lhs_sid, &ir_rhs, signals);
                // FIX: Unpacked array literal init — `arr = '{e0, e1, ...}`.
                // Concat membangun value MSB-first `{e0, e1, e2, e3}` tapi
                // unpacked array storage punya element 0 di bit terendah.
                // Decompose di AST level sebelum ir_rhs di-fold jadi Const:
                //   arr[0] = e0; arr[1] = e1; arr[2] = e2; arr[3] = e3;
                if let IrLValue::Signal(sid, _) = &ir_lhs {
                    if let Some(sig) = signals.get(*sid) {
                        if sig.array_depth > 1 {
                            // Check if RHS is AST-level Concat (array literal '{...}')
                            if let Expr::Concat(elems) = rhs {
                                if elems.len() == sig.array_depth {
                                    let mut stmts: Vec<IrStmt> = Vec::new();
                                    for (i, elem) in elems.iter().enumerate() {
                                        let ir_elem =
                                            self.elaborate_expr(elem, signal_map, signals)?;
                                        let idx_expr =
                                            IrExpr::Const(LogicVec::from_u64(i as u64, 32));
                                        let elem_lvalue = IrLValue::ArrayIndex {
                                            sig_id: *sid,
                                            index: Box::new(idx_expr),
                                            elem_width: sig.elem_width.max(1),
                                        };
                                        stmts.push(IrStmt::BlockingAssign {
                                            lhs: elem_lvalue,
                                            rhs: ir_elem,
                                            delay: None,
                                        });
                                    }
                                    return Ok(IrStmt::Block { stmts });
                                }
                            }
                        }
                    }
                }
                Ok(IrStmt::BlockingAssign {
                    lhs: ir_lhs,
                    rhs: ir_rhs,
                    delay: self.elab_intra_assign_delay(delay),
                })
            }
            Stmt::NonBlockingAssign { lhs, rhs, delay } => {
                let ir_lhs = self.elaborate_lvalue(lhs, signal_map, signals)?;
                let ir_is_vif = match &ir_lhs {
                    IrLValue::Signal(sid, _) => signals
                        .get(*sid)
                        .map(|s| s.iface_type.is_some())
                        .unwrap_or(false),
                    _ => false,
                };
                let mut ir_rhs = if ir_is_vif {
                    match rhs {
                        Expr::Ident { name, .. } if !signal_map.contains_key(name) => {
                            IrExpr::VifBinding {
                                instance_name: *name,
                            }
                        }
                        _ => self.elaborate_expr(rhs, signal_map, signals)?,
                    }
                } else {
                    self.elaborate_expr(rhs, signal_map, signals)?
                };
                // Struct assignment pattern — lihat arm BlockingAssign.
                if let Some(ir_pattern) =
                    self.elaborate_struct_pattern(&ir_lhs, rhs, signal_map, signals)?
                {
                    ir_rhs = ir_pattern;
                }
                // Pola struct tak boleh dilewatkan const-fold `Expr::StructLit`
                // (tak tahu layout → 0), lihat arm BlockingAssign.
                let struct_pattern = matches!(rhs, Expr::StructLit { .. });
                if let IrExpr::NewCall {
                    ref mut class_name, ..
                } = ir_rhs
                {
                    if class_name.is_empty() {
                        if let IrLValue::Signal(sid, _) = ir_lhs {
                            if let Some(sig) = signals.get(sid) {
                                if let Some(cn) = &sig.class_name {
                                    *class_name = *cn;
                                }
                            }
                        }
                    }
                }
                let lhs_sid = lvalue_signal_id(&ir_lhs);
                // Propagasi lebar konteks LHS (lihat arm BlockingAssign).
                let _at = std::time::Instant::now();
                if !struct_pattern {
                    self.apply_lhs_context_width(&ir_lhs, rhs, &mut ir_rhs, signal_map, signals);
                }
                self.dbg_apply_us
                    .set(self.dbg_apply_us.get() + _at.elapsed().as_micros() as u64);
                self.dbg_apply_n.set(self.dbg_apply_n.get() + 1);
                let (lhs_line, lhs_col) = expr_location(lhs);
                self.check_width_mismatch(&ir_lhs, &ir_rhs, signals, lhs_line, lhs_col);
                check_signed_mismatch(lhs_sid, &ir_rhs, signals);
                Ok(IrStmt::NonBlockingAssign {
                    lhs: ir_lhs,
                    rhs: ir_rhs,
                    delay: self.elab_intra_assign_delay(delay),
                })
            }
            Stmt::IfElse {
                cond,
                true_branch,
                false_branch,
            } => {
                // Constant-fold condition — if known at compile time, eliminate dead branch.
                // JANGAN fold bila kondisi mereferensikan param SIGNED:
                // const_eval_with_params memperlakukan Ident sbg unsigned
                // sehingga `PD < 0` (PD = localparam int -2) salah ter-fold
                // jadi false; biarkan runtime mengevaluasi dgn IrExpr::Signed
                // (probe p12f/p12g — signedness hilang).
                let can_fold = !super::expr_refs_signed_param(cond, &self.param_is_signed);
                if can_fold {
                    if let Ok(val) = const_eval_with_params(cond, &self.param_vals) {
                        if val != 0 {
                            // Condition is always true — keep only true branch
                            Ok(self.elaborate_stmt(
                                true_branch,
                                signal_map,
                                known_modules,
                                signals,
                            )?)
                        } else {
                            // Condition is always false — keep only false branch
                            match false_branch {
                                Some(fb) => {
                                    self.elaborate_stmt(fb, signal_map, known_modules, signals)
                                }
                                None => Ok(IrStmt::Block { stmts: vec![] }),
                            }
                        }
                    } else {
                        let ir_cond = self.elaborate_expr(cond, signal_map, signals)?;
                        let true_stmt = vec![self.elaborate_stmt(
                            true_branch,
                            signal_map,
                            known_modules,
                            signals,
                        )?];
                        let false_stmt = match false_branch {
                            Some(fb) => {
                                vec![self.elaborate_stmt(fb, signal_map, known_modules, signals)?]
                            }
                            None => vec![],
                        };
                        Ok(IrStmt::If {
                            cond: ir_cond,
                            true_branch: true_stmt,
                            false_branch: false_stmt,
                        })
                    }
                } else {
                    let ir_cond = self.elaborate_expr(cond, signal_map, signals)?;
                    let true_stmt = vec![self.elaborate_stmt(
                        true_branch,
                        signal_map,
                        known_modules,
                        signals,
                    )?];
                    let false_stmt = match false_branch {
                        Some(fb) => {
                            vec![self.elaborate_stmt(fb, signal_map, known_modules, signals)?]
                        }
                        None => vec![],
                    };
                    Ok(IrStmt::If {
                        cond: ir_cond,
                        true_branch: true_stmt,
                        false_branch: false_stmt,
                    })
                }
            }
            Stmt::Case {
                expr,
                items,
                default,
            } => {
                // BUG FIX (case const-fold): `case (1'b1)` / `case (KONST)` dengan
                // label SINYAL (idiom `(* parallel_case *) case (1'b1) sel: ...`)
                // TIDAK boleh di-const-fold — nilai label berubah di runtime.
                // Sebelumnya: case expr konstanta membuat elaborator memilih branch
                // secara STATIS; label sinyal gagal const_eval → tidak match → jatuh
                // ke `default` → cabang yang dipilih sinyal TIDAK PERNAH dieksekusi
                // (mis. picorv32 `decoded_imm` selalu X). Fold hanya aman bila case
                // expr KONSTAN dan SEMUA label KONSTAN.
                let all_labels_const = items.iter().all(|item| {
                    item.labels.iter().all(|l| {
                        (const_eval_with_params(l, &self.param_vals).is_ok()
                            || matches!(l, Expr::Value(_)))
                            && !literal_has_unknown_bits(l)
                    })
                });
                let case_const = const_eval_with_params(expr, &self.param_vals);
                // Digit X/Z/? tak bisa di-fold ke i64 (jadi 0) → `case (2'b1x)`
                // vs label `2'b1?` akan SALAH match bila di-fold. Jalankan
                // runtime (4-state) kalau case expr atau label memuat unknown.
                if case_const.is_err() || !all_labels_const || literal_has_unknown_bits(expr) {
                    // ── Evaluasi RUNTIME: case expr / label memakai sinyal ──
                    let mut ir_expr = self.elaborate_expr(expr, signal_map, signals)?;
                    let mut ir_items = Vec::new();
                    for item in items {
                        let mut labels = Vec::new();
                        for label in &item.labels {
                            labels.push(self.elaborate_expr(label, signal_map, signals)?);
                        }
                        let body = match &*item.stmt {
                            Stmt::Block { stmts } => self.elaborate_stmt_block(
                                stmts,
                                signal_map,
                                known_modules,
                                signals,
                            )?,
                            other => self.elaborate_stmt_block(
                                std::slice::from_ref(other),
                                signal_map,
                                known_modules,
                                signals,
                            )?,
                        };
                        ir_items.push(IrCaseItem { labels, body });
                    }
                    let ir_default = match default {
                        Some(d) => {
                            vec![self.elaborate_stmt(d, signal_map, known_modules, signals)?]
                        }
                        None => vec![],
                    };
                    apply_case_context_width(&mut ir_expr, &mut ir_items, signals);
                    Ok(IrStmt::Case {
                        case_type: CaseType::Normal,
                        expr: ir_expr,
                        items: ir_items,
                        default: ir_default,
                    })
                } else {
                    // ── Fold aman: case expr + semua label konstanta ──
                    // Cari branch pertama yang match (prioritas Verilog).
                    let Ok(case_val) = case_const else {
                        unreachable!("case_const Ok karena is_err() sudah di-guard di atas");
                    };
                    let mut matched_body: Option<&Stmt> = None;
                    for item in items {
                        for label in &item.labels {
                            let label_val = const_eval_with_params(label, &self.param_vals);
                            if let Ok(lv) = label_val {
                                if lv == case_val {
                                    matched_body = Some(&item.stmt);
                                    break;
                                }
                            } else if let Expr::Value(v) = label {
                                let lv = match v {
                                    Value::Decimal(d) => *d,
                                    Value::Hex { bits, .. } => {
                                        mivon_ast::const_eval::parse_literal(
                                            bits.trim_start_matches("0x").trim_start_matches("0X"),
                                            16,
                                        )
                                        .unwrap_or(0)
                                    }
                                    Value::Binary { bits, .. } => {
                                        mivon_ast::const_eval::parse_literal(
                                            bits.trim_start_matches("0b").trim_start_matches("0B"),
                                            2,
                                        )
                                        .unwrap_or(0)
                                    }
                                    Value::Octal { bits, .. } => {
                                        mivon_ast::const_eval::parse_literal(
                                            bits.trim_start_matches("0o").trim_start_matches("0O"),
                                            8,
                                        )
                                        .unwrap_or(0)
                                    }
                                    Value::Real(_) => 0,
                                };
                                if lv == case_val {
                                    matched_body = Some(&item.stmt);
                                    break;
                                }
                            }
                        }
                        if matched_body.is_some() {
                            break;
                        }
                    }
                    match matched_body {
                        Some(body) => self.elaborate_stmt(body, signal_map, known_modules, signals),
                        None => {
                            if let Some(def) = default {
                                self.elaborate_stmt(def, signal_map, known_modules, signals)
                            } else {
                                Ok(IrStmt::Block { stmts: vec![] })
                            }
                        }
                    }
                }
            }
            Stmt::StmtAssign { lhs, rhs } => {
                let ir_lhs = self.elaborate_lvalue(lhs, signal_map, signals)?;
                let mut ir_rhs = self.elaborate_expr(rhs, signal_map, signals)?;
                let _at = std::time::Instant::now();
                self.apply_lhs_context_width(&ir_lhs, rhs, &mut ir_rhs, signal_map, signals);
                self.dbg_apply_us
                    .set(self.dbg_apply_us.get() + _at.elapsed().as_micros() as u64);
                self.dbg_apply_n.set(self.dbg_apply_n.get() + 1);
                Ok(IrStmt::BlockingAssign {
                    lhs: ir_lhs,
                    rhs: ir_rhs,
                    delay: None,
                })
            }
            Stmt::Expr { expr } => {
                match expr {
                    Expr::MethodCall {
                        obj,
                        method,
                        args,
                        with_clause,
                    } => {
                        // F36: receiver instance (interface instance seperti
                        // `clk_if.set_active()`) — emit MethodCallStmt dengan
                        // obj HierRef; engine no-op bila instance tak punya
                        // method tersimulasi.
                        if let Expr::Ident { name, .. } = obj.as_ref() {
                            if self.is_current_module_instance(name) {
                                let ir_args: Vec<IrExpr> = args
                                    .iter()
                                    .map(|a| self.elaborate_expr(a, signal_map, signals))
                                    .collect::<Result<_, _>>()?;
                                let ir_with = match with_clause {
                                    Some(wc) => Some(Box::new(
                                        self.elaborate_expr(wc, signal_map, signals)?,
                                    )),
                                    None => None,
                                };
                                return Ok(IrStmt::MethodCallStmt {
                                    obj: IrExpr::HierRef(*name),
                                    method: *method,
                                    args: ir_args,
                                    with_clause: ir_with,
                                });
                            }
                        }
                        let ir_obj = self.elaborate_expr(obj, signal_map, signals)?;
                        let ir_args: Vec<IrExpr> = args
                            .iter()
                            .map(|a| self.elaborate_expr(a, signal_map, signals))
                            .collect::<Result<_, _>>()?;
                        let ir_with = match with_clause {
                            Some(wc) => {
                                Some(Box::new(self.elaborate_expr(wc, signal_map, signals)?))
                            }
                            None => None,
                        };
                        Ok(IrStmt::MethodCallStmt {
                            obj: ir_obj,
                            method: *method,
                            args: ir_args,
                            with_clause: ir_with,
                        })
                    }
                    Expr::FuncCall {
                        name, line, col, ..
                    } if name.starts_with("$") => {
                        let ir_expr = self.elaborate_expr(expr, signal_map, signals)?;
                        Ok(IrStmt::SysCall {
                            name: Symbol::intern(""),
                            args: vec![ir_expr],
                            line: *line,
                            col: *col,
                        })
                    }
                    Expr::FuncCall {
                        name, line, col, ..
                    } if name.ends_with("::new") => {
                        let ir_expr = self.elaborate_expr(expr, signal_map, signals)?;
                        Ok(IrStmt::SysCall {
                            name: Symbol::intern(""),
                            args: vec![ir_expr],
                            line: *line,
                            col: *col,
                        })
                    }
                    Expr::FuncCall {
                        name,
                        args,
                        line,
                        col,
                        ..
                    } if name == "run_test" => {
                        // F18: run_test("name") adalah statement BER-Efek (bukan
                        // side-effect-free) — tanpa special-case ini ia di-
                        // eliminasi diam-diam oleh arm generic di bawah.
                        let ir_args: Result<Vec<IrExpr>, SimError> = args
                            .iter()
                            .map(|a| self.elaborate_expr(a, signal_map, signals))
                            .collect();
                        Ok(IrStmt::SysCall {
                            name: Symbol::intern("run_test"),
                            args: ir_args?,
                            line: *line,
                            col: *col,
                        })
                    }
                    Expr::FuncCall {
                        name,
                        args,
                        line,
                        col,
                        ..
                    } => {
                        // VERIF-07: UVM DB calls (`uvm_config_db::set`, `uvm_resource_db::set`/
                        // get/exists/write_by_name/read_by_name, uvm_cmdline_processor::*) adalah
                        // statement BER-Efek — jangan eliminasi sebagai side-effect-free.
                        // BUG SEBELUMNYA: `uvm_resource_db::set(...)` sebagai bare statement di
                        // initial/always block di-eliminasi di sini → map tidak pernah terisi →
                        // get selalu 0 (nilai tak pernah tersimpan).
                        if name.starts_with("uvm_config_db::")
                            || name.starts_with("uvm_resource_db::")
                            || name.starts_with("uvm_cmdline_processor::")
                            || name.starts_with("uvm_root::")
                            || name.starts_with("uvm_tr_database::")
                        {
                            let ir_args: Result<Vec<IrExpr>, SimError> = args
                                .iter()
                                .map(|a| self.elaborate_expr(a, signal_map, signals))
                                .collect();
                            return Ok(IrStmt::SysCall {
                                name: *name,
                                args: ir_args?,
                                line: *line,
                                col: *col,
                            });
                        }
                        // Check if this is a DPI function call used as a statement
                        let is_dpi = self.dpi_import_names.contains(name);
                        if is_dpi {
                            let ir_expr = self.elaborate_expr(expr, signal_map, signals)?;
                            Ok(IrStmt::SysCall {
                                name: Symbol::intern("__dpi_stmt"),
                                args: vec![ir_expr],
                                line: *line,
                                col: *col,
                            })
                        } else {
                            // Side-effect-free expression statement — eliminate it
                            Ok(IrStmt::Block { stmts: vec![] })
                        }
                    }
                    _ => {
                        // Side-effect-free expression statement — eliminate it
                        Ok(IrStmt::Block { stmts: vec![] })
                    }
                }
            }
            Stmt::SysCall {
                name,
                args,
                line,
                col,
            } => {
                // F42: syscall waveform dump ($dumpvars/$dumpall/$dumpfile/...) —
                // argumen berupa path hierarkis module (`$dumpvars(0, tb_top)`)
                // yang bukan signal. Toleransi: arg tak ter-resolve menjadi
                // konstanta 0 + warning, bukan E2001 yang memblokir modul.
                let dump_syscalls = [
                    "dumpvars",
                    "dumpall",
                    "dumpoff",
                    "dumpon",
                    "dumpfile",
                    "dumpflush",
                    "dumplimit",
                    "wlfdumpvars",
                    "wlfdumpall",
                    "wlfopen",
                ];
                if dump_syscalls.contains(&name.as_str()) {
                    let ir_args: Vec<IrExpr> = args
                        .iter()
                        .map(|a| match self.elaborate_expr(a, signal_map, signals) {
                            Ok(ir) => ir,
                            Err(e) => {
                                self.elab_warn_at(
                                    DiagCode::ModuleNotFound,
                                    format!(
                                        "$({}) argument not resolvable — treated as 0: {}",
                                        name.as_str(),
                                        e
                                    ),
                                    expr_location(a).0,
                                    expr_location(a).1,
                                );
                                IrExpr::Const(LogicVec::from_u64(0, 32))
                            }
                        })
                        .collect();
                    return Ok(IrStmt::SysCall {
                        name: *name,
                        args: ir_args,
                        line: *line,
                        col: *col,
                    });
                }
                let ir_args: Vec<IrExpr> = args
                    .iter()
                    .map(|a| self.elaborate_expr(a, signal_map, signals))
                    .collect::<Result<_, _>>()?;
                Ok(IrStmt::SysCall {
                    name: *name,
                    args: ir_args,
                    line: *line,
                    col: *col,
                })
            }
            Stmt::SysFinish => Ok(IrStmt::SysFinish),
            Stmt::Null => Ok(IrStmt::Null),
            Stmt::Return(_) => Ok(IrStmt::Null),
            Stmt::EventControl { events, stmt } => {
                if events.is_empty() {
                    return Ok(IrStmt::Null);
                }
                let body = match stmt {
                    Some(s) => vec![self.elaborate_stmt(s, signal_map, known_modules, signals)?],
                    None => vec![],
                };
                // @(*) — wildcard: treat as immediate (block), bukan blocking event.
                if events
                    .iter()
                    .any(|e| matches!(e, SensitivityEvent::Wildcard))
                {
                    return Ok(IrStmt::Block { stmts: body });
                }
                // Dukung multi-event `@(a or b)` / `@(posedge a or posedge b)`:
                // kumpulkan SEMUA (sig_id, edge), bukan hanya event pertama.
                // LANG-27: `iff (cond)` pada event disimpan sebagai guard
                // IrStmt::EventControl.iff — continuation hanya lanjut bila cond true.
                let mut sigs = Vec::with_capacity(events.len());
                let mut iff = None;
                for event in events {
                    let inner = match event {
                        SensitivityEvent::Iff { event, cond } => {
                            let ir_cond = self.elaborate_expr(cond, signal_map, signals)?;
                            if iff.is_none() {
                                iff = Some(ir_cond);
                            }
                            event.as_ref()
                        }
                        other => other,
                    };
                    match inner {
                        SensitivityEvent::PosEdge(expr) => {
                            if let Some(sig_id) = resolve_expr_signal(expr, signal_map) {
                                sigs.push((sig_id, Some(ClockEdge::PosEdge(sig_id))));
                            } else {
                                match self.hier_event_edge(expr, signal_map, signals, true) {
                                    Some((sid, edge)) => sigs.push((sid, Some(edge))),
                                    None => {
                                        // F38: event tak ter-resolve (clocking block
                                        // interface `@(sck_clk.cbn)`, hier path) —
                                        // degrade warning + skip, bukan hard error.
                                        self.elab_warn_at(
                                            DiagCode::NotImplemented,
                                            "cannot resolve signal in @(...) — event skipped"
                                                .to_string(),
                                            expr_location(expr).0,
                                            expr_location(expr).1,
                                        );
                                        continue;
                                    }
                                }
                            }
                        }
                        SensitivityEvent::NegEdge(expr) => {
                            if let Some(sig_id) = resolve_expr_signal(expr, signal_map) {
                                sigs.push((sig_id, Some(ClockEdge::NegEdge(sig_id))));
                            } else {
                                match self.hier_event_edge(expr, signal_map, signals, false) {
                                    Some((sid, edge)) => sigs.push((sid, Some(edge))),
                                    None => {
                                        self.elab_warn_at(
                                            DiagCode::NotImplemented,
                                            "cannot resolve signal in @(...) — event skipped"
                                                .to_string(),
                                            expr_location(expr).0,
                                            expr_location(expr).1,
                                        );
                                        continue;
                                    }
                                }
                            }
                        }
                        SensitivityEvent::Level(expr) => {
                            match resolve_expr_signal(expr, signal_map) {
                                Some(sig_id) => sigs.push((sig_id, None)),
                                None => {
                                    self.elab_warn_at(
                                        DiagCode::NotImplemented,
                                        "cannot resolve signal in @(...) — event skipped"
                                            .to_string(),
                                        expr_location(expr).0,
                                        expr_location(expr).1,
                                    );
                                    continue;
                                }
                            }
                        }
                        SensitivityEvent::Wildcard => unreachable!("handled above"),
                        // Iff sudah dibuka di atas — arm ini tidak akan tercapai.
                        SensitivityEvent::Iff { .. } => {}
                    }
                }
                Ok(IrStmt::EventControl { sigs, body, iff })
            }
            Stmt::EventTrigger { name } => {
                if let Some(sig_id) = signal_map.get(name) {
                    Ok(IrStmt::EventTrigger { sig_id: *sig_id })
                } else {
                    Ok(IrStmt::Null)
                }
            }
            Stmt::Force { lhs, rhs } => {
                let ir_lhs = self.elaborate_lvalue(lhs, signal_map, signals)?;
                let ir_rhs = self.elaborate_expr(rhs, signal_map, signals)?;
                Ok(IrStmt::Force {
                    lvalue: ir_lhs,
                    rhs: ir_rhs,
                })
            }
            Stmt::Release { expr } => {
                let ir_lhs = self.elaborate_lvalue(expr, signal_map, signals)?;
                Ok(IrStmt::Release { lvalue: ir_lhs })
            }
            Stmt::Deassign { expr } => {
                let ir_lhs = self.elaborate_lvalue(expr, signal_map, signals)?;
                Ok(IrStmt::Deassign { lvalue: ir_lhs })
            }
            Stmt::CaseX {
                expr,
                items,
                default,
            } => {
                let mut ir_expr = self.elaborate_expr(expr, signal_map, signals)?;
                let mut ir_items = Vec::new();
                for item in items {
                    let mut labels = Vec::new();
                    for label in &item.labels {
                        labels.push(self.elaborate_expr(label, signal_map, signals)?);
                    }
                    let body = match &*item.stmt {
                        Stmt::Block { stmts } => {
                            self.elaborate_stmt_block(stmts, signal_map, known_modules, signals)?
                        }
                        other => self.elaborate_stmt_block(
                            std::slice::from_ref(other),
                            signal_map,
                            known_modules,
                            signals,
                        )?,
                    };
                    ir_items.push(IrCaseItem { labels, body });
                }
                let ir_default = match default {
                    Some(d) => vec![self.elaborate_stmt(d, signal_map, known_modules, signals)?],
                    None => vec![],
                };
                apply_case_context_width(&mut ir_expr, &mut ir_items, signals);
                Ok(IrStmt::Case {
                    case_type: CaseType::CaseX,
                    expr: ir_expr,
                    items: ir_items,
                    default: ir_default,
                })
            }
            Stmt::CaseZ {
                expr,
                items,
                default,
            } => {
                let mut ir_expr = self.elaborate_expr(expr, signal_map, signals)?;
                let mut ir_items = Vec::new();
                for item in items {
                    let mut labels = Vec::new();
                    for label in &item.labels {
                        labels.push(self.elaborate_expr(label, signal_map, signals)?);
                    }
                    let body = match &*item.stmt {
                        Stmt::Block { stmts } => {
                            self.elaborate_stmt_block(stmts, signal_map, known_modules, signals)?
                        }
                        other => self.elaborate_stmt_block(
                            std::slice::from_ref(other),
                            signal_map,
                            known_modules,
                            signals,
                        )?,
                    };
                    ir_items.push(IrCaseItem { labels, body });
                }
                let ir_default = match default {
                    Some(d) => vec![self.elaborate_stmt(d, signal_map, known_modules, signals)?],
                    None => vec![],
                };
                apply_case_context_width(&mut ir_expr, &mut ir_items, signals);
                Ok(IrStmt::Case {
                    case_type: CaseType::CaseZ,
                    expr: ir_expr,
                    items: ir_items,
                    default: ir_default,
                })
            }
            Stmt::NamedBlock { name, stmts, decls } => {
                let body = self.elaborate_stmt_block(stmts, signal_map, known_modules, signals)?;
                Ok(IrStmt::NamedBlock {
                    name: *name,
                    stmts: body,
                    decls: decls.clone(),
                })
            }
            Stmt::Delay { delay, stmt } => {
                // Delay dinamis (`#(CLK_PERIOD/2)` dengan CLK_PERIOD real
                // signal, bukan parameter) tidak bisa di-const-eval — engine
                // hanya mendukung delay konstan. Jangan gagalkan elaborasi
                // (modul AST oscillator OpenTitan seperti io_osc/sys_osc/rng
                // memakai pola ini): emit warning + fallback delay 1 agar
                // modul tetap bisa dielaborasi dan disimulasikan.
                let d = match const_eval_params(delay, &self.param_vals) {
                    Ok(v) => v as u64,
                    Err(e) => {
                        let (l, c) = crate::util::generate::expr_location(delay);
                        self.elab_warn_at(
                            DiagCode::SimulationError,
                            format!("non-constant delay evaluated as 1: {}", e),
                            l,
                            c,
                        );
                        1
                    }
                };
                let body = vec![self.elaborate_stmt(stmt, signal_map, known_modules, signals)?];
                Ok(IrStmt::Delay { delay: d, body })
            }
            Stmt::Wait { cond, stmt } => {
                let ir_cond = self.elaborate_expr(cond, signal_map, signals)?;
                let body = match stmt {
                    Some(s) => vec![self.elaborate_stmt(s, signal_map, known_modules, signals)?],
                    None => vec![],
                };
                Ok(IrStmt::Wait {
                    cond: ir_cond,
                    body,
                })
            }
            Stmt::WaitFork => Ok(IrStmt::WaitFork),
            Stmt::LoopFor {
                init,
                cond,
                step,
                stmts,
            } => {
                // Try to unroll constant-bounded for loops at elaboration time
                let unroll_result = try_unroll_for_loop(
                    init.as_deref(),
                    cond.as_ref(),
                    step.as_deref(),
                    stmts,
                    &|stmts, var_name, iter_val| {
                        let subst_stmts = substitute_loop_var_in_stmts(stmts, var_name, iter_val);
                        if std::env::var("DBG_LOOP").is_ok() {
                            eprintln!(
                                "[DBG-LOOP] iter {}={} body={:?}",
                                var_name, iter_val, subst_stmts
                            );
                        }
                        self.elaborate_stmt_block(&subst_stmts, signal_map, known_modules, signals)
                            .map_err(|e| e.to_string())
                    },
                    &self.param_vals,
                );
                if std::env::var("DBG_LOOP").is_ok() {
                    match &unroll_result {
                        Ok(Some(u)) => eprintln!("[DBG-LOOP] unrolled {} stmts", u.len()),
                        Ok(None) => eprintln!("[DBG-LOOP] unroll None (fallback runtime)"),
                        Err(e) => eprintln!("[DBG-LOOP] unroll Err: {}", e),
                    }
                }
                if let Ok(Some(unrolled)) = unroll_result {
                    // Statistik cache pipeline (db.md "6. optimize/"): loop
                    // for yang berhasil di-unroll + jumlah statement hasilnya.
                    self.opt_stats.record_loop_unroll(unrolled.len());
                    // Enum-iterasi fallback: `for (... t = t.first(); i <
                    // t.num(); ... t = t.next())` di prim_sparse_fsm_flop —
                    // body di-elide (loop var enum tak ter-resolve; hanya
                    // menandai assertion/FIP `unused_err_o`, bukan datapath).
                    if unrolled.is_empty()
                        && !stmts.is_empty()
                        && crate::util::loop_unroll::is_enum_iteration_loop(
                            init.as_deref(),
                            cond.as_ref(),
                            step.as_deref(),
                        )
                    {
                        let loc = cond
                            .as_ref()
                            .map(crate::util::generate::expr_location)
                            .unwrap_or((0, 0));
                        self.elab_warn_at(
                            DiagCode::SimulationError,
                            "enum-iteration loop (t.first()/t.num()/t.next()) elided — mivon belum men-resolve parameter-type enum per-instance; signal assertion/FIP di-degradasi (non-fatal, datapath tidak terpengaruh)".to_string(),
                            loc.0,
                            loc.1,
                        );
                    }
                    return Ok(IrStmt::Block { stmts: unrolled });
                }
                // Fallback: generate runtime LoopFor. Loop var (`for (int j = 0 ...)`)
                // sudah didaftarkan sebagai signal sintetis oleh pre-pass
                // `ensure_loop_var_signals` di module elaboration (lihat mod.rs),
                // jadi init/cond/step/body bisa di-resolve di sini.
                let ir_init = match init {
                    Some(s) => Some(Box::new(self.elaborate_stmt(
                        s,
                        signal_map,
                        known_modules,
                        signals,
                    )?)),
                    None => None,
                };
                let ir_cond = if let Some(c) = cond {
                    self.elaborate_expr(c, signal_map, signals)?
                } else {
                    IrExpr::Const(LogicVec::from_u64(1, 1))
                };
                let ir_step = match step {
                    Some(s) => Some(Box::new(self.elaborate_stmt(
                        s,
                        signal_map,
                        known_modules,
                        signals,
                    )?)),
                    None => None,
                };
                let ir_body =
                    self.elaborate_stmt_block(stmts, signal_map, known_modules, signals)?;
                Ok(IrStmt::LoopFor {
                    init: ir_init,
                    cond: ir_cond,
                    step: ir_step,
                    body: ir_body,
                })
            }
            Stmt::LoopWhile { cond, stmts } => {
                let ir_cond = self.elaborate_expr(cond, signal_map, signals)?;
                let ir_body =
                    self.elaborate_stmt_block(stmts, signal_map, known_modules, signals)?;
                Ok(IrStmt::LoopWhile {
                    cond: ir_cond,
                    body: ir_body,
                })
            }
            Stmt::DoWhile { cond, stmts } => {
                let ir_cond = self.elaborate_expr(cond, signal_map, signals)?;
                let ir_body =
                    self.elaborate_stmt_block(stmts, signal_map, known_modules, signals)?;
                Ok(IrStmt::LoopDoWhile {
                    cond: ir_cond,
                    body: ir_body,
                })
            }
            Stmt::LoopForever { stmts } => {
                let ir_body =
                    self.elaborate_stmt_block(stmts, signal_map, known_modules, signals)?;
                Ok(IrStmt::LoopWhile {
                    cond: IrExpr::Const(LogicVec::from_u64(1, 1)),
                    body: ir_body,
                })
            }
            Stmt::ForeachLoop {
                array_var,
                index_vars,
                stmts,
            } => {
                // F44: array foreach tak ter-resolve (queue port task yang
                // tidak di-rename saat inline, array milik UVM/class, dll) —
                // degrade warning + skip body, bukan hard error.
                let Some(sig_id) = signal_map.get(array_var) else {
                    let (l, c) = (0, 0);
                    self.elab_warn_at(
                        DiagCode::NotImplemented,
                        format!("array '{}' not found for foreach — loop skipped", array_var),
                        l,
                        c,
                    );
                    return Ok(IrStmt::Null);
                };
                let sig_info = signals.get(*sig_id).ok_or_else(|| {
                    self.elab_diag(
                        DiagCode::ModuleNotFound,
                        format!("signal info not found for '{}'", array_var),
                    )
                })?;
                if sig_info.is_dynamic || sig_info.is_queue {
                    let ir_body =
                        self.elaborate_stmt_block(stmts, signal_map, known_modules, signals)?;
                    let iv = index_vars
                        .first()
                        .cloned()
                        .unwrap_or_else(|| Symbol::intern("i"));
                    Ok(IrStmt::Foreach {
                        array_var: IrExpr::Signal(*sig_id, sig_info.width),
                        index_var: iv,
                        body: ir_body,
                    })
                } else {
                    let n = sig_info.array_depth;
                    if n == 0 {
                        // F44: array scalar / tak dikenal — degrade warning +
                        // skip, bukan hard error (pola foreach DV/task).
                        let (l, c) = (0, 0);
                        self.elab_warn_at(
                            DiagCode::NotImplemented,
                            format!(
                                "'{}' is not an array, cannot use foreach — loop skipped",
                                array_var
                            ),
                            l,
                            c,
                        );
                        return Ok(IrStmt::Null);
                    }
                    let mut all_stmts = Vec::new();
                    let iv = index_vars
                        .first()
                        .cloned()
                        .unwrap_or_else(|| Symbol::intern("i"));
                    for i in 0..n {
                        let subst_stmts =
                            substitute_loop_var_in_stmts(stmts, iv.as_str(), i as i64);
                        all_stmts.extend(self.elaborate_stmt_block(
                            &subst_stmts,
                            signal_map,
                            known_modules,
                            signals,
                        )?);
                    }
                    Ok(IrStmt::Block { stmts: all_stmts })
                }
            }
            Stmt::StmtCase {
                expr,
                items,
                default,
            } => {
                let mut ir_expr = self.elaborate_expr(expr, signal_map, signals)?;
                let mut ir_items = Vec::new();
                for item in items {
                    let mut labels = Vec::new();
                    for label in &item.labels {
                        labels.push(self.elaborate_expr(label, signal_map, signals)?);
                    }
                    let body = match &*item.stmt {
                        Stmt::Block { stmts } => {
                            self.elaborate_stmt_block(stmts, signal_map, known_modules, signals)?
                        }
                        other => self.elaborate_stmt_block(
                            std::slice::from_ref(other),
                            signal_map,
                            known_modules,
                            signals,
                        )?,
                    };
                    ir_items.push(IrCaseItem { labels, body });
                }
                let ir_default = match default {
                    Some(d) => vec![self.elaborate_stmt(d, signal_map, known_modules, signals)?],
                    None => vec![],
                };
                apply_case_context_width(&mut ir_expr, &mut ir_items, signals);
                Ok(IrStmt::Case {
                    case_type: CaseType::Normal,
                    expr: ir_expr,
                    items: ir_items,
                    default: ir_default,
                })
            }
            Stmt::Break => Ok(IrStmt::Break),
            Stmt::Continue => Ok(IrStmt::Continue),
            Stmt::Disable { name } => Ok(IrStmt::Disable { name: *name }),
            Stmt::Repeat { count, stmts } => {
                if let Ok(n) = const_eval_params(count, &self.param_vals) {
                    let mut all = Vec::new();
                    for _ in 0..n {
                        all.extend(self.elaborate_stmt_block(
                            stmts,
                            signal_map,
                            known_modules,
                            signals,
                        )?);
                    }
                    Ok(IrStmt::Block { stmts: all })
                } else {
                    let ir_count = self.elaborate_expr(count, signal_map, signals)?;
                    let ir_body =
                        self.elaborate_stmt_block(stmts, signal_map, known_modules, signals)?;
                    Ok(IrStmt::Repeat {
                        count: ir_count,
                        body: ir_body,
                    })
                }
            }
            // LANG-16/17: qualifier unique/unique0/priority dipertahankan ke IR
            // (CaseType::Unique/Unique0/Priority) — engine menegakkan semantik
            // (warning multiple-match / no-match). Tidak di-const-fold agar
            // struktur case tetap ada untuk pemeriksaan runtime.
            Stmt::UniqueCase {
                expr,
                items,
                default,
                kind,
            }
            | Stmt::PriorityCase {
                expr,
                items,
                default,
                kind,
            }
            | Stmt::Unique0Case {
                expr,
                items,
                default,
                kind,
            } => {
                use mivon_ast::CaseKind;
                let is_unique0 = matches!(stmt, Stmt::Unique0Case { .. });
                let is_unique = matches!(stmt, Stmt::UniqueCase { .. });
                // Qualifier & kind ortogonal (LRM 1800 §12.5): kind X/Z
                // memakai pencocokan wildcard, qualifier untuk warning.
                let ct = match (is_unique, is_unique0, kind) {
                    (true, _, CaseKind::X) => CaseType::UniqueX,
                    (true, _, CaseKind::Z) => CaseType::UniqueZ,
                    (true, _, _) => CaseType::Unique,
                    (_, true, CaseKind::X) => CaseType::Unique0X,
                    (_, true, CaseKind::Z) => CaseType::Unique0Z,
                    (_, true, _) => CaseType::Unique0,
                    (_, _, CaseKind::X) => CaseType::PriorityX,
                    (_, _, CaseKind::Z) => CaseType::PriorityZ,
                    _ => CaseType::Priority,
                };
                self.elaborate_case_raw(
                    expr,
                    items,
                    default,
                    ct,
                    signal_map,
                    known_modules,
                    signals,
                )
            }
            Stmt::CaseInside {
                expr,
                items,
                default,
            } => {
                // `case (x) inside`: label bisa berupa nilai tunggal (equality)
                // atau rentang `[lo:hi]` (di-parse sebagai RangeSelect ber-base 0).
                // Jalankan dulu const-fold dengan pola yang sama seperti Case;
                // jika gagal, buat IrStmt::Case dengan CaseType::Inside.
                if let Ok(case_val) = const_eval_with_params(expr, &self.param_vals) {
                    let mut matched_body: Option<&Stmt> = None;
                    for item in items {
                        for label in &item.labels {
                            if let Some((lo, hi)) = inside_range_bounds(label) {
                                // Label rentang: cocok jika lo <= case_val <= hi.
                                if let (Ok(lo_v), Ok(hi_v)) = (
                                    const_eval_with_params(lo, &self.param_vals),
                                    const_eval_with_params(hi, &self.param_vals),
                                ) {
                                    let (l, h) = (lo_v.min(hi_v), lo_v.max(hi_v));
                                    if case_val >= l && case_val <= h {
                                        matched_body = Some(&item.stmt);
                                        break;
                                    }
                                }
                                continue;
                            }
                            let label_val = const_eval_with_params(label, &self.param_vals);
                            if let Ok(lv) = label_val {
                                if lv == case_val {
                                    matched_body = Some(&item.stmt);
                                    break;
                                }
                            } else if let Expr::Value(v) = label {
                                let lv = value_to_i64(v);
                                if lv == case_val {
                                    matched_body = Some(&item.stmt);
                                    break;
                                }
                            }
                        }
                        if matched_body.is_some() {
                            break;
                        }
                    }
                    match matched_body {
                        Some(body) => self.elaborate_stmt(body, signal_map, known_modules, signals),
                        None => {
                            if let Some(def) = default {
                                self.elaborate_stmt(def, signal_map, known_modules, signals)
                            } else {
                                Ok(IrStmt::Block { stmts: vec![] })
                            }
                        }
                    }
                } else {
                    let mut ir_expr = self.elaborate_expr(expr, signal_map, signals)?;
                    let mut ir_items = Vec::new();
                    for item in items {
                        let mut labels = Vec::new();
                        for label in &item.labels {
                            if let Some((lo, hi)) = inside_range_bounds(label) {
                                // Rentang `[lo:hi]` → IrExpr::InsideRange agar runtime
                                // mencocokkan dengan perbandingan rentang.
                                labels.push(IrExpr::InsideRange {
                                    expr: Box::new(ir_expr.clone()),
                                    lo: Box::new(self.elaborate_expr(lo, signal_map, signals)?),
                                    hi: Box::new(self.elaborate_expr(hi, signal_map, signals)?),
                                });
                            } else {
                                labels.push(self.elaborate_expr(label, signal_map, signals)?);
                            }
                        }
                        let body = match &*item.stmt {
                            Stmt::Block { stmts } => self.elaborate_stmt_block(
                                stmts,
                                signal_map,
                                known_modules,
                                signals,
                            )?,
                            other => self.elaborate_stmt_block(
                                std::slice::from_ref(other),
                                signal_map,
                                known_modules,
                                signals,
                            )?,
                        };
                        ir_items.push(IrCaseItem { labels, body });
                    }
                    let ir_default = match default {
                        Some(d) => {
                            vec![self.elaborate_stmt(d, signal_map, known_modules, signals)?]
                        }
                        None => vec![],
                    };
                    apply_case_context_width(&mut ir_expr, &mut ir_items, signals);
                    Ok(IrStmt::Case {
                        case_type: CaseType::Inside,
                        expr: ir_expr,
                        items: ir_items,
                        default: ir_default,
                    })
                }
            }
            Stmt::UniqueIf {
                cond,
                true_branch,
                false_branch,
            } => self.elaborate_stmt(
                &Stmt::IfElse {
                    cond: cond.clone(),
                    true_branch: true_branch.clone(),
                    false_branch: false_branch.clone(),
                },
                signal_map,
                known_modules,
                signals,
            ),
            Stmt::PriorityIf {
                cond,
                true_branch,
                false_branch,
            } => self.elaborate_stmt(
                &Stmt::IfElse {
                    cond: cond.clone(),
                    true_branch: true_branch.clone(),
                    false_branch: false_branch.clone(),
                },
                signal_map,
                known_modules,
                signals,
            ),
            Stmt::PropertySeq {
                sequence,
                pass_stmt,
                fail_stmt,
                clock_event,
                disable_iff,
            } => {
                // LANG-06: concurrent assertion dengan sequence temporal —
                // terjemahkan AST Sequence → IrSequence, kondisikan dummy true
                // (sequence dievaluasi engine via SequenceAttempt per clock).
                // line/col diambil dari ekspresi pertama sequence (untuk
                // record_assertion / assertion_stats).
                let (a_line, a_col) = sequence_first_loc(sequence);
                let ir_seq = self.elaborate_sequence(sequence, signal_map, signals)?;
                let pass = match pass_stmt {
                    Some(s) => vec![self.elaborate_stmt(s, signal_map, known_modules, signals)?],
                    None => vec![],
                };
                let fail = match fail_stmt {
                    Some(s) => vec![self.elaborate_stmt(s, signal_map, known_modules, signals)?],
                    None => vec![],
                };
                let ir_disable = match disable_iff {
                    Some(e) => Some(Box::new(self.elaborate_expr(e, signal_map, signals)?)),
                    None => None,
                };
                let ir_cond = IrExpr::Const(mivon_ir::LogicVec::from_u64(1, 1));
                Ok(IrStmt::Assert {
                    cond: ir_cond,
                    pass_stmt: pass,
                    fail_stmt: fail,
                    clock_event: clock_event.clone(),
                    disable_iff: ir_disable,
                    sequence: Some(Box::new(ir_seq)),
                    line: a_line,
                    col: a_col,
                })
            }
            Stmt::Assert {
                cond,
                pass_stmt,
                fail_stmt,
                clock_event,
                disable_iff,
            } => {
                let (a_line, a_col) = expr_location(cond);
                let ir_cond = self.elaborate_expr(cond, signal_map, signals)?;
                let pass = match pass_stmt {
                    Some(s) => vec![self.elaborate_stmt(s, signal_map, known_modules, signals)?],
                    None => vec![],
                };
                let fail = match fail_stmt {
                    Some(s) => vec![self.elaborate_stmt(s, signal_map, known_modules, signals)?],
                    None => vec![],
                };
                let ir_disable = match disable_iff {
                    Some(e) => Some(Box::new(self.elaborate_expr(e, signal_map, signals)?)),
                    None => None,
                };
                // ELAB-12: assertion dengan kondisi parameter-dependent yang
                // bisa di-eval saat elab-time (seluruh operand konstanta /
                // parameter) dievaluasi SEKARANG — melaporkan kegagalan lebih
                // awal daripada menunggu simulasi. Hanya assertion tanpa
                // clock_event (immediate, bukan concurrent SVA) yang di-eval;
                // assertion concurrent butuh semantik waktu simulasi.
                if clock_event.is_none() {
                    if let Ok(val) =
                        mivon_ast::types::const_eval_with_params(cond, &self.param_vals)
                    {
                        if val == 0 {
                            self.elab_warn_at(
                                mivon_core::diagnostics::DiagCode::AssertionFailed,
                                format!("elaboration-time assertion failed (condition evaluates to 0 at elaboration)\n  condition: {:?}", cond),
                                a_line,
                                a_col,
                            );
                        }
                    }
                }
                Ok(IrStmt::Assert {
                    cond: ir_cond,
                    pass_stmt: pass,
                    fail_stmt: fail,
                    clock_event: clock_event.clone(),
                    disable_iff: ir_disable,
                    sequence: None,
                    line: a_line,
                    col: a_col,
                })
            }
            Stmt::Assume {
                cond,
                pass_stmt,
                fail_stmt,
                clock_event,
                disable_iff,
            } => {
                let (a_line, a_col) = expr_location(cond);
                let ir_cond = self.elaborate_expr(cond, signal_map, signals)?;
                let pass = match pass_stmt {
                    Some(s) => vec![self.elaborate_stmt(s, signal_map, known_modules, signals)?],
                    None => vec![],
                };
                let fail = match fail_stmt {
                    Some(s) => vec![self.elaborate_stmt(s, signal_map, known_modules, signals)?],
                    None => vec![],
                };
                let ir_disable = match disable_iff {
                    Some(e) => Some(Box::new(self.elaborate_expr(e, signal_map, signals)?)),
                    None => None,
                };
                Ok(IrStmt::Assume {
                    cond: ir_cond,
                    pass_stmt: pass,
                    fail_stmt: fail,
                    clock_event: clock_event.clone(),
                    disable_iff: ir_disable,
                    sequence: None,
                    line: a_line,
                    col: a_col,
                })
            }
            Stmt::Cover {
                cond,
                pass_stmt,
                clock_event,
                disable_iff,
            } => {
                let ir_cond = self.elaborate_expr(cond, signal_map, signals)?;
                let pass = match pass_stmt {
                    Some(s) => vec![self.elaborate_stmt(s, signal_map, known_modules, signals)?],
                    None => vec![],
                };
                let ir_disable = match disable_iff {
                    Some(e) => Some(Box::new(self.elaborate_expr(e, signal_map, signals)?)),
                    None => None,
                };
                Ok(IrStmt::Cover {
                    cond: ir_cond,
                    pass_stmt: pass,
                    clock_event: clock_event.clone(),
                    disable_iff: ir_disable,
                    sequence: None,
                })
            }
            Stmt::Expect {
                cond,
                pass_stmt,
                fail_stmt,
            } => {
                // LANG-14: `expect (property) else stmt` — assertion dalam
                // procedural code (IEEE 1800-2017 §17.16.2). Kondisi
                // dievaluasi seketika saat statement dijangkau (subset
                // immediate, tanpa clock_event/disable_iff di AST Expect).
                let (a_line, a_col) = expr_location(cond);
                let ir_cond = self.elaborate_expr(cond, signal_map, signals)?;
                let pass = match pass_stmt {
                    Some(s) => vec![self.elaborate_stmt(s, signal_map, known_modules, signals)?],
                    None => vec![],
                };
                let fail = match fail_stmt {
                    Some(s) => vec![self.elaborate_stmt(s, signal_map, known_modules, signals)?],
                    None => vec![],
                };
                Ok(IrStmt::Expect {
                    cond: ir_cond,
                    pass_stmt: pass,
                    fail_stmt: fail,
                    line: a_line,
                    col: a_col,
                })
            }
            Stmt::WaitOrder { events, fail_stmt } => {
                let mut sig_ids = Vec::new();
                for name in events {
                    if let Some(idx) = signal_map.get(name) {
                        sig_ids.push(*idx);
                    } else {
                        return Err(self.elab_diag(
                            DiagCode::ModuleNotFound,
                            format!("wait_order: signal '{}' not found", name),
                        ));
                    }
                }
                let failure = match fail_stmt {
                    Some(s) => {
                        vec![self.elaborate_stmt(s, signal_map, known_modules, signals)?]
                    }
                    None => vec![],
                };
                Ok(IrStmt::WaitOrder {
                    events: sig_ids,
                    failure_stmts: failure,
                })
            }
            Stmt::Fork {
                processes,
                join_type,
            } => {
                let mut ir_processes = Vec::new();
                for proc_stmt in processes {
                    let ir = self.elaborate_stmt(proc_stmt, signal_map, known_modules, signals)?;
                    ir_processes.push(vec![ir]);
                }
                let ir_join = match join_type {
                    JoinType::Join => IrJoinType::Join,
                    JoinType::JoinAny => IrJoinType::JoinAny,
                    JoinType::JoinNone => IrJoinType::JoinNone,
                };
                Ok(IrStmt::Fork {
                    processes: ir_processes,
                    join_type: ir_join,
                })
            }
            Stmt::RandCase { items } => {
                let new_items: Result<Vec<(IrExpr, Vec<IrStmt>)>, SimError> = items
                    .iter()
                    .map(|rc| {
                        let weight_expr = IrExpr::Const(LogicVec::from_u64(rc.weight, 32));
                        let body = self.elaborate_stmt_block(
                            &[*rc.stmt.clone()],
                            signal_map,
                            known_modules,
                            signals,
                        )?;
                        Ok((weight_expr, body))
                    })
                    .collect();
                Ok(IrStmt::RandCase { items: new_items? })
            }
            Stmt::RandSequence { productions } => {
                let mut ir_productions = Vec::new();
                for prod in productions {
                    let mut ir_items = Vec::new();
                    for item in &prod.items {
                        let weight_expr = if let Some(w) = item.weight {
                            IrExpr::Const(LogicVec::from_u64(w, 32))
                        } else {
                            IrExpr::Const(LogicVec::from_u64(1, 32))
                        };
                        let body = self.elaborate_stmt_block(
                            &[(*item.value).clone()],
                            signal_map,
                            known_modules,
                            signals,
                        )?;
                        ir_items.push((weight_expr, body));
                    }
                    ir_productions.push((prod.name, ir_items));
                }
                Ok(IrStmt::RandSequence {
                    productions: ir_productions,
                })
            }
        };
        // ── Akhir instrumentasi DBG_STMT: catat waktu per konstruk. ──
        if let Some(t0) = stmt_t0 {
            // AtomicU64/Ordering removed: unused
            thread_local! {
                static DBG_STMT_TIME: std::cell::RefCell<std::collections::HashMap<&'static str, (u64, u64)>> =
                    std::cell::RefCell::new(std::collections::HashMap::new());
            }
            let dt = t0.elapsed().as_nanos() as u64;
            DBG_STMT_TIME.with(|cell| {
                let mut m = cell.borrow_mut();
                let e = m.entry(stmt_kind).or_insert((0, 0));
                e.0 += 1;
                e.1 += dt;
            });
        }
        out
    }

    pub(crate) fn elaborate_lvalue(
        &self,
        expr: &Expr,
        signal_map: &HashMap<Symbol, SignalId>,
        signals: &[SignalInfo],
    ) -> Result<IrLValue, SimError> {
        match expr {
            Expr::Ident { name, line, col } => {
                let sig_id = signal_map.get(name).ok_or_else(|| {
                    let candidates: Vec<&str> = signal_map.keys().map(|s| s.as_str()).collect();
                    let hint = suggest_name(name.as_str(), candidates.into_iter())
                        .map(|(s, _)| format!(" — did you mean '{}'?", s))
                        .unwrap_or_default();
                    self.elab_diag_at(
                        DiagCode::UndefinedSignal,
                        format!("signal '{}' not found{}", name, hint),
                        *line,
                        *col,
                    )
                })?;
                Ok(IrLValue::Signal(*sig_id, 0))
            }
            Expr::RangeSelect {
                expr: inner,
                msb,
                lsb,
            } => {
                let inner_lv = self.elaborate_lvalue(inner, signal_map, signals)?;
                // Bound range yang gagal di-const-eval (mis. member access
                // struct, param hilang) → fallback 0 + warning, bukan error
                // yang mematikan modul.
                let msb_c = match const_eval_params(msb, &self.param_vals) {
                    Ok(v) => v.max(0) as usize,
                    Err(e) => {
                        let (l, c) = crate::util::generate::expr_location(msb);
                        self.elab_warn_at(
                            DiagCode::SimulationError,
                            format!("cannot evaluate lvalue range bound ({}), fallback 0", e),
                            l,
                            c,
                        );
                        0
                    }
                };
                let lsb_c = match const_eval_params(lsb, &self.param_vals) {
                    Ok(v) => v.max(0) as usize,
                    Err(e) => {
                        let (l, c) = crate::util::generate::expr_location(lsb);
                        self.elab_warn_at(
                            DiagCode::SimulationError,
                            format!("cannot evaluate lvalue range bound ({}), fallback 0", e),
                            l,
                            c,
                        );
                        0
                    }
                };
                match inner_lv {
                    IrLValue::Signal(sid, _) => Ok(IrLValue::RangeSelect(sid, msb_c, lsb_c)),
                    IrLValue::RangeSelect(sid, outer_msb, outer_lsb) => {
                        let outer_start = if outer_msb > outer_lsb {
                            outer_lsb
                        } else {
                            outer_msb
                        };
                        let inner_start = outer_start + if msb_c > lsb_c { lsb_c } else { msb_c };
                        let inner_end = outer_start + if msb_c > lsb_c { msb_c } else { lsb_c };
                        Ok(IrLValue::RangeSelect(sid, inner_end, inner_start))
                    }
                    IrLValue::ArrayIndex {
                        sig_id,
                        index,
                        elem_width,
                    } => Ok(IrLValue::ArrayRangeSelect {
                        sig_id,
                        index,
                        elem_width,
                        msb: msb_c,
                        lsb: lsb_c,
                    }),
                    // Range select di atas bit select (`sig[a][b][msb:lsb]` —
                    // packed multidimensi setelah unroll). Akumulasi offset bit.
                    IrLValue::BitSelect(sid, base_bit) => {
                        let start = base_bit + msb_c.min(lsb_c);
                        let end = base_bit + msb_c.max(lsb_c);
                        Ok(IrLValue::RangeSelect(sid, end, start))
                    }
                    _ => {
                        // F45: nested range select tak ter-model (hier
                        // interface / array dinamis) — degrade warning + write
                        // no-op, bukan hard error yang memblokir modul.
                        let (l, c) = expr_location(expr);
                        self.elab_warn_at(
                            DiagCode::NotImplemented,
                            format!(
                                "nested range select lvalue tidak di-resolve statis (obj={:?}) — write diabaikan",
                                expr
                            ),
                            l,
                            c,
                        );
                        Ok(IrLValue::ObjectField {
                            sig_id: 0,
                            field: Symbol::intern("__nested_range_ignored"),
                        })
                    }
                }
            }
            Expr::BitSelect {
                expr: inner,
                index: bs_index,
            } => {
                let inner_lv = self.elaborate_lvalue(inner, signal_map, signals)?;
                match inner_lv {
                    IrLValue::Signal(sid, _) => {
                        let sig = &signals[sid];
                        // Check for multi-dim packed array: packed_dims.len() > 1
                        if sig.packed_dims.len() > 1 {
                            let outer_elem_width = sig.width / sig.packed_dims[0];
                            if let Ok(idx) = const_eval_params(bs_index, &self.param_vals) {
                                let idx = idx as usize;
                                let lsb = idx * outer_elem_width;
                                let msb = lsb + outer_elem_width - 1;
                                Ok(IrLValue::RangeSelect(sid, msb, lsb))
                            } else {
                                let index_expr =
                                    self.elaborate_expr(bs_index, signal_map, signals)?;
                                Ok(IrLValue::ArrayIndex {
                                    sig_id: sid,
                                    index: Box::new(index_expr),
                                    elem_width: outer_elem_width,
                                })
                            }
                        } else if sig.array_depth > 1
                            || sig.is_dynamic
                            || sig.is_queue
                            || sig.is_associative
                        {
                            let index_expr = self.elaborate_expr(bs_index, signal_map, signals)?;
                            // F39: multi-dim unpacked — index pertama memilih
                            // ROW: lebar = elem_width × Π dims[1..] (bit offset
                            // i × row_w terbaca/tertulis utuh oleh engine).
                            let ew = if sig.array_dims.len() > 1 {
                                sig.array_dims[1..]
                                    .iter()
                                    .product::<usize>()
                                    .saturating_mul(sig.elem_width)
                            } else {
                                sig.elem_width
                            };
                            Ok(IrLValue::ArrayIndex {
                                sig_id: sid,
                                index: Box::new(index_expr),
                                elem_width: ew,
                            })
                        } else if let Ok(idx) = const_eval_params(bs_index, &self.param_vals) {
                            Ok(IrLValue::BitSelect(sid, idx as usize))
                        } else {
                            // Dynamic index on a flat signal — treat as array index
                            let index_expr = self.elaborate_expr(bs_index, signal_map, signals)?;
                            Ok(IrLValue::ArrayIndex {
                                sig_id: sid,
                                index: Box::new(index_expr),
                                elem_width: sig.elem_width,
                            })
                        }
                    }
                    IrLValue::RangeSelect(sid, outer_msb, outer_lsb) => {
                        if let Ok(idx) = const_eval_params(bs_index, &self.param_vals) {
                            let base = if outer_msb > outer_lsb {
                                outer_lsb
                            } else {
                                outer_msb
                            };
                            Ok(IrLValue::BitSelect(sid, base + idx as usize))
                        } else {
                            let index_expr = self.elaborate_expr(bs_index, signal_map, signals)?;
                            Ok(IrLValue::ArrayIndex {
                                sig_id: sid,
                                index: Box::new(index_expr),
                                elem_width: outer_msb.max(outer_lsb) - outer_msb.min(outer_lsb) + 1,
                            })
                        }
                    }
                    IrLValue::ArrayIndex {
                        sig_id,
                        index,
                        elem_width,
                    } => {
                        // F39: index berantai pada array unpacked multi-dimensi
                        // `mat[i][j] = x` — fold ke satu ArrayIndex (index
                        // gabungan unit elemen, lihat jalur baca expr.rs).
                        let mut folded: Option<IrLValue> = None;
                        if let Some(sig) = signals.get(sig_id) {
                            let dims = &sig.array_dims;
                            let ew = sig.elem_width;
                            if dims.len() > 1 && elem_width > ew && ew > 0 {
                                let remaining = elem_width / ew;
                                let mut c = dims.len();
                                let mut prod = 1usize;
                                while c > 0 {
                                    c -= 1;
                                    prod *= dims[c];
                                    if prod == remaining {
                                        break;
                                    }
                                }
                                if prod == remaining && c < dims.len() {
                                    let new_ew = elem_width / dims[c];
                                    let idx_expr =
                                        self.elaborate_expr(bs_index, signal_map, signals)?;
                                    let combined = IrExpr::BinaryOp(
                                        BinaryIrOp::Add,
                                        Box::new(IrExpr::BinaryOp(
                                            BinaryIrOp::Mul,
                                            Box::new((*index).clone()),
                                            Box::new(IrExpr::Const(LogicVec::from_u64(
                                                dims[c] as u64,
                                                32,
                                            ))),
                                        )),
                                        Box::new(idx_expr),
                                    );
                                    folded = Some(IrLValue::ArrayIndex {
                                        sig_id,
                                        index: Box::new(combined),
                                        elem_width: new_ew,
                                    });
                                }
                            }
                        }
                        if let Some(lv) = folded {
                            return Ok(lv);
                        }
                        if let Ok(idx) = const_eval_params(bs_index, &self.param_vals) {
                            Ok(IrLValue::ArrayBitSelect {
                                sig_id,
                                index,
                                elem_width,
                                bit: Box::new(IrExpr::Const(LogicVec::from_u64(idx as u64, 64))),
                            })
                        } else {
                            // `arr[i][j]` dengan j runtime (packed multidimensi,
                            // mis. `seeds_q[seed_idx][rd_idx]` di
                            // flash_ctrl_lcmgr) — bit dievaluasi saat write di
                            // engine (offset = i * elem_width + j).
                            let bit_expr = self.elaborate_expr(bs_index, signal_map, signals)?;
                            Ok(IrLValue::ArrayBitSelect {
                                sig_id,
                                index,
                                elem_width,
                                bit: Box::new(bit_expr),
                            })
                        }
                    }
                    // Bit select di atas lvalue hierarkis (`sif.sd_out[i]`):
                    // nama belum ter-resolve di elaborator — simpan index,
                    // engine menghitung offset saat write dari SignalInfo.
                    IrLValue::HierRef(name) | IrLValue::HierRefIndex { name, .. } => {
                        let index_expr = self.elaborate_expr(bs_index, signal_map, signals)?;
                        Ok(IrLValue::HierRefIndex {
                            name,
                            index: Box::new(index_expr),
                        })
                    }
                    // Bit select di atas field object yang sudah ter-degrade
                    // (`resets_o.rst_por_io_div2_n[DomainMainSel]` di rstmgr —
                    // struct port tak ter-resolve → ObjectField no-op). Index
                    // di atas ObjectField tidak bisa di-offset statis; pertahan-
                    // kan degrade: warning + ObjectField (write tetap diabaikan
                    // engine) agar modul TIDAK di-skip oleh "nested bit select".
                    IrLValue::ObjectField { sig_id, field } => {
                        let (l, c) = expr_location(expr);
                        self.elab_warn_at(
                            DiagCode::NotImplemented,
                            format!(
                                "bit select pada field object '{}' yang tidak ter-resolve — write diabaikan",
                                field.as_str()
                            ),
                            l,
                            c,
                        );
                        Ok(IrLValue::ObjectField { sig_id, field })
                    }
                    // Bit select di atas bit select (`sig[a][b]` — packed
                    // multidimensi setelah unroll, mis. `result[0][0][0]`).
                    // Akumulasi offset bit.
                    IrLValue::BitSelect(sid, inner_bit) => {
                        if let Ok(idx) = const_eval_params(bs_index, &self.param_vals) {
                            Ok(IrLValue::BitSelect(sid, inner_bit + idx as usize))
                        } else {
                            let index_expr = self.elaborate_expr(bs_index, signal_map, signals)?;
                            Ok(IrLValue::ArrayBitSelect {
                                sig_id: sid,
                                index: Box::new(index_expr),
                                elem_width: 1,
                                bit: Box::new(IrExpr::Const(LogicVec::from_u64(
                                    inner_bit as u64,
                                    64,
                                ))),
                            })
                        }
                    }
                    _ => Err(self.elab_diag_at(
                        DiagCode::NotImplemented,
                        "nested bit select not supported",
                        expr_location(expr).0,
                        expr_location(expr).1,
                    )),
                }
            }
            Expr::PartSelect {
                expr: inner,
                base,
                width,
            } => {
                let inner_lv = self.elaborate_lvalue(inner, signal_map, signals)?;
                let base_r = const_eval_params(base, &self.param_vals);
                let width_r = const_eval_params(width, &self.param_vals);
                match (base_r, width_r) {
                    (Ok(b), Ok(w)) => {
                        let (base_c, width_c) = (b as usize, w as usize);
                        match inner_lv {
                            IrLValue::Signal(sid, _) => {
                                if width_c > 0 {
                                    Ok(IrLValue::RangeSelect(sid, base_c + width_c - 1, base_c))
                                } else {
                                    Ok(IrLValue::RangeSelect(sid, base_c, base_c))
                                }
                            }
                            IrLValue::RangeSelect(sid, outer_msb, outer_lsb) => {
                                let outer_base = if outer_msb > outer_lsb {
                                    outer_lsb
                                } else {
                                    outer_msb
                                };
                                let new_base = outer_base + base_c;
                                if width_c > 0 {
                                    Ok(IrLValue::RangeSelect(sid, new_base + width_c - 1, new_base))
                                } else {
                                    Ok(IrLValue::RangeSelect(sid, new_base, new_base))
                                }
                            }
                            IrLValue::ArrayIndex {
                                sig_id,
                                index,
                                elem_width,
                            } => {
                                if width_c > 0 {
                                    Ok(IrLValue::ArrayRangeSelect {
                                        sig_id,
                                        index,
                                        elem_width,
                                        msb: base_c + width_c - 1,
                                        lsb: base_c,
                                    })
                                } else {
                                    Ok(IrLValue::ArrayRangeSelect {
                                        sig_id,
                                        index,
                                        elem_width,
                                        msb: base_c,
                                        lsb: base_c,
                                    })
                                }
                            }
                            _ => Err(self.elab_diag_at(
                                DiagCode::NotImplemented,
                                "nested part-select in lvalue not supported",
                                expr_location(expr).0,
                                expr_location(expr).1,
                            )),
                        }
                    }
                    _ => {
                        // Base ATAU width tidak konstanta → part-select dinamis
                        // `sig[base +: width]` dengan base runtime. Simpan base
                        // sebagai ekspresi (dievaluasi saat write) dan width yang
                        // di-resolve best-effort (param/konstan biasanya).
                        let base_ir = self.elaborate_expr(base, signal_map, signals)?;
                        let width_c: usize = match const_eval_params(width, &self.param_vals) {
                            Ok(w) => (w.max(1)) as usize,
                            Err(_) => {
                                let _ = self.elaborate_expr(width, signal_map, signals);
                                compute_expr_width(
                                    width,
                                    signal_map,
                                    signals,
                                    &self.param_vals,
                                    &self.package_symbols,
                                )
                                .unwrap_or(1)
                                .max(1)
                            }
                        };
                        match inner_lv {
                            IrLValue::Signal(sid, _) => Ok(IrLValue::ExprPartSelect {
                                sig_id: sid,
                                base: Box::new(base_ir),
                                width: width_c,
                            }),
                            IrLValue::RangeSelect(sid, outer_msb, outer_lsb) => {
                                let offset = outer_msb.min(outer_lsb);
                                let base_adj = IrExpr::BinaryOp(
                                    BinaryIrOp::Add,
                                    Box::new(base_ir),
                                    Box::new(IrExpr::Const(LogicVec::from_u64(offset as u64, 32))),
                                );
                                Ok(IrLValue::ExprPartSelect {
                                    sig_id: sid,
                                    base: Box::new(base_adj),
                                    width: width_c,
                                })
                            }
                            IrLValue::ArrayIndex {
                                sig_id,
                                index,
                                elem_width,
                            } => {
                                let idx_expr = IrExpr::BinaryOp(
                                    BinaryIrOp::Mul,
                                    index,
                                    Box::new(IrExpr::Const(LogicVec::from_u64(
                                        elem_width as u64,
                                        32,
                                    ))),
                                );
                                let base_adj = IrExpr::BinaryOp(
                                    BinaryIrOp::Add,
                                    Box::new(base_ir),
                                    Box::new(idx_expr),
                                );
                                Ok(IrLValue::ExprPartSelect {
                                    sig_id,
                                    base: Box::new(base_adj),
                                    width: width_c,
                                })
                            }
                            // Lvalue hierarkis / field object yang sudah ter-degrade
                            // (`key_slots_d[slot].key[j][cnt*W +: W]` di
                            // keymgr_dpe_ctrl — member chain dinamis tak bisa
                            // di-offset statis). Pertahankan degrade: warning +
                            // lvalue asal (write diabaikan engine) agar modul
                            // tidak di-skip.
                            IrLValue::HierRef(name) => {
                                let (l, c) = expr_location(expr);
                                self.elab_warn_at(
                                    DiagCode::NotImplemented,
                                    format!(
                                        "dynamic part-select pada lvalue hierarkis '{}' — write diabaikan",
                                        name.as_str()
                                    ),
                                    l,
                                    c,
                                );
                                Ok(IrLValue::HierRef(name))
                            }
                            IrLValue::HierRefIndex { name, index } => {
                                let (l, c) = expr_location(expr);
                                self.elab_warn_at(
                                    DiagCode::NotImplemented,
                                    format!(
                                        "dynamic part-select pada lvalue hierarkis '{}' — write diabaikan",
                                        name.as_str()
                                    ),
                                    l,
                                    c,
                                );
                                Ok(IrLValue::HierRefIndex { name, index })
                            }
                            IrLValue::ObjectField { sig_id, field } => {
                                let (l, c) = expr_location(expr);
                                self.elab_warn_at(
                                    DiagCode::NotImplemented,
                                    format!(
                                        "dynamic part-select pada field object '{}' — write diabaikan",
                                        field.as_str()
                                    ),
                                    l,
                                    c,
                                );
                                Ok(IrLValue::ObjectField { sig_id, field })
                            }
                            _ => Err(self.elab_diag_at(
                                DiagCode::NotImplemented,
                                "nested dynamic part-select in lvalue not supported",
                                expr_location(expr).0,
                                expr_location(expr).1,
                            )),
                        }
                    }
                }
            }
            Expr::Concat(exprs) => {
                let parts: Result<Vec<IrLValue>, SimError> = exprs
                    .iter()
                    .map(|e| self.elaborate_lvalue(e, signal_map, signals))
                    .collect();
                Ok(IrLValue::Concat(parts?))
            }
            Expr::MethodCall { .. } => Err(self.elab_diag_at(
                DiagCode::NotImplemented,
                "method calls cannot be used as lvalues",
                expr_location(expr).0,
                expr_location(expr).1,
            )),
            Expr::MemberAccess { obj, field } => {
                // Try struct/union field write
                let hier_name = Self::build_hier_name(obj, field.as_str());
                if std::env::var("MIVON_DBG_HIER").is_ok() && !hier_name.is_empty() {
                    let in_sigmap = signal_map.contains_key(&Symbol::intern(&hier_name));
                    let in_signals = signals.iter().any(|s| s.name.as_str() == hier_name);
                    eprintln!(
                        "[DBG-HIER] lvalue hier_name='{}' sigmap={} signals={} obj={:?}",
                        hier_name, in_sigmap, in_signals, obj
                    );
                }
                if let Some(&sig_id) = signal_map.get(&Symbol::intern(&hier_name)) {
                    return Ok(IrLValue::Signal(sig_id, 0));
                }
                // F27: port interface (`bus_if b` — iface_type di-set, class_name
                // None) → field write lewat hier path (`b.data = x`). Dikompil
                // sebagai IrLValue::HierRef agar engine menulis ke signal flatten
                // instance interface yang sama dengan tb (via hier_signal_map).
                if let Some((base_name, _)) =
                    Self::collect_member_chain(obj, *field, &self.param_vals)
                {
                    if let Some(&base_sid) = signal_map.get(&Symbol::intern(&base_name)) {
                        let base_info = &signals[base_sid];
                        if base_info.iface_type.is_some()
                            && base_info.class_name.is_none()
                            && !hier_name.is_empty()
                        {
                            return Ok(IrLValue::HierRef(Symbol::intern(&hier_name)));
                        }
                    }
                }
                // Nested member access lvalue (`hw2reg.val.d = x`): kumpulkan
                // chain field (dari luar ke dalam), resolve base signal, lalu
                // akumulasi offset berjenjang via struct_fields + typedef_field_map.
                if let Some((base_name, chain)) =
                    Self::collect_member_chain(obj, *field, &self.param_vals)
                {
                    if let Some(&base_sid) = signal_map.get(&Symbol::intern(&base_name)) {
                        let base_info = &signals[base_sid];
                        if !base_info.struct_fields.is_empty() {
                            if let Some((msb, lsb)) =
                                self.resolve_struct_chain(base_sid, &chain, signals)
                            {
                                return Ok(IrLValue::RangeSelect(base_sid, msb, lsb));
                            }
                        }
                    }
                }
                match self.elaborate_expr(obj, signal_map, signals) {
                    Ok(IrExpr::Signal(sig_id, _)) => {
                        let sig_info = &signals[sig_id];
                        if !sig_info.struct_fields.is_empty() {
                            if let Some(f) =
                                sig_info.struct_fields.iter().find(|f| f.name == *field)
                            {
                                let lsb = f.offset;
                                let msb = f.offset + f.width - 1;
                                return Ok(IrLValue::RangeSelect(sig_id, lsb, msb));
                            }
                            // Field tidak ditemukan — mungkin struct dari package yang
                            // belum fully resolved. Emit warning dan fallback ke
                            // ObjectField agar elaborasi tidak gagal total.
                            self.elab_warn_at(
                                DiagCode::ModuleNotFound,
                                format!("field '{}' not found in struct type", field),
                                expr_location(expr).0,
                                expr_location(expr).1,
                            );
                            return Ok(IrLValue::ObjectField {
                                sig_id,
                                field: *field,
                            });
                        }
                        // Class object handle: obj = signal berisi obj id → field.
                        if sig_info.class_name.is_some() {
                            return Ok(IrLValue::ObjectField {
                                sig_id,
                                field: *field,
                            });
                        }
                        // Signal NON-struct dijadikan target member access —
                        // biasanya interface instance (`jtag_mst.tdo`, `tif.miso`)
                        // atau sinyal testbench yang field-nya tidak ter-model.
                        // Degrade: warning + HierRef (engine resolve nama saat
                        // write) bila nama hierarkis tersedia, selain itu
                        // ObjectField (no-op aman di engine untuk non-object).
                        self.elab_warn_at(
                            DiagCode::ModuleNotFound,
                            format!(
                                "member access pada signal non-struct '{:?}.{}' — write tidak penuh (interface/hier fallback)",
                                obj,
                                field.as_str()
                            ),
                            expr_location(expr).0,
                            expr_location(expr).1,
                        );
                        let hn = Self::build_hier_name(obj, field.as_str());
                        if !hn.is_empty() {
                            return Ok(IrLValue::HierRef(Symbol::intern(&hn)));
                        }
                        Ok(IrLValue::ObjectField {
                            sig_id,
                            field: *field,
                        })
                    }
                    // obj TIDAK ter-resolve sebagai signal: instance interface
                    // (`sif.csb`) atau path instance (`u_dut.u_padring.cio_*`).
                    // Statement module di-elaborate SEBELUM flatten_instances,
                    // jadi nama hierarkis belum ada di signal_map/signals saat
                    // itu. Simpan sebagai HierRef — engine resolve ke flattened
                    // signal list saat write (mekanisme sama dengan
                    // IrExpr::HierRef untuk read).
                    _ => {
                        let hier_name = Self::build_hier_name(obj, field.as_str());
                        if !hier_name.is_empty() {
                            let base_is_signal =
                                Self::collect_member_chain(obj, *field, &self.param_vals)
                                    .map(|(base, _)| {
                                        signal_map.contains_key(&Symbol::intern(&base))
                                    })
                                    .unwrap_or(false);
                            if !base_is_signal {
                                return Ok(IrLValue::HierRef(Symbol::intern(&hier_name)));
                            }
                        }
                        // Member access lvalue yang TIDAK bisa di-resolve statis
                        // (base index dinamis — mis. `key_slots_d[slot].key[j]`
                        // di keymgr_dpe, atau path instance). Degrade: warning +
                        // HierRef bila nama hierarkis tersedia, selain itu
                        // ObjectField (no-op aman) agar modul tidak di-skip.
                        let (l, c) = expr_location(expr);
                        self.elab_warn_at(
                            DiagCode::NotImplemented,
                            format!(
                                "member access lvalue tidak dapat di-resolve statis (obj={:?}.{}) — write diabaikan",
                                obj,
                                field.as_str()
                            ),
                            l,
                            c,
                        );
                        let hn = Self::build_hier_name(obj, field.as_str());
                        if !hn.is_empty() {
                            return Ok(IrLValue::HierRef(Symbol::intern(&hn)));
                        }
                        if let Some((base_name, _)) =
                            Self::collect_member_chain(obj, *field, &self.param_vals)
                        {
                            if let Some(&base_sid) = signal_map.get(&Symbol::intern(&base_name)) {
                                return Ok(IrLValue::ObjectField {
                                    sig_id: base_sid,
                                    field: *field,
                                });
                            }
                        }
                        Ok(IrLValue::ObjectField {
                            sig_id: 0,
                            field: *field,
                        })
                    }
                }
            }
            _ => {
                if std::env::var("MIVON_DBG_LVALUE").is_ok() {
                    eprintln!(
                        "[DBG-LVALUE] invalid lvalue expr at {}:{}: {:?}",
                        expr_location(expr).0,
                        expr_location(expr).1,
                        expr
                    );
                }
                Err(self.elab_diag_at(
                    DiagCode::InvalidSyntax,
                    format!("invalid lvalue expression: {}", describe_lvalue_expr(expr)),
                    expr_location(expr).0,
                    expr_location(expr).1,
                ))
            }
        }
    }

    /// F27: event edge hierarkis (`@(posedge b.clk)` di blok prosedural) —
    /// bangun (base_sid, ClockEdge::*Hier(Symbol)) untuk member access pada
    /// port interface. Engine me-resolve Symbol via hier_signal_map saat
    /// meng-arm EventControl (normalize_event_sigs). None bila base BUKAN port
    /// interface (iface_type tanpa class_name) — pemanggil meneruskan error
    /// "cannot resolve signal" seperti perilaku lama.
    fn hier_event_edge(
        &self,
        expr: &Expr,
        signal_map: &HashMap<Symbol, SignalId>,
        signals: &[SignalInfo],
        is_pos: bool,
    ) -> Option<(SignalId, ClockEdge)> {
        let hier_full = match expr {
            Expr::MemberAccess { obj, field } => Self::build_hier_name(obj, field.as_str()),
            _ => return None,
        };
        if hier_full.is_empty() {
            return None;
        }
        let base_sid = match expr {
            Expr::MemberAccess { obj, .. } => resolve_expr_signal(obj, signal_map),
            _ => None,
        };
        // Verifikasi base: port interface (signal iface_type tanpa class_name)
        // ATAU instance interface di module ini (`bus_if b(); ... @(posedge b.clk)`
        // di module yang sama). Bukan struct field biasa / vif variabel.
        let is_iface_port = base_sid
            .and_then(|sid| signals.get(sid))
            .map(|s| s.iface_type.is_some() && s.class_name.is_none())
            .unwrap_or(false);
        let is_iface_inst = match expr {
            Expr::MemberAccess { obj, .. } => match obj.as_ref() {
                Expr::Ident { name, .. } => self.is_interface_instance(name.as_str()),
                _ => false,
            },
            _ => false,
        };
        if !is_iface_port && !is_iface_inst {
            return None;
        }
        let edge = if is_pos {
            ClockEdge::PosEdgeHier(Symbol::intern(&hier_full))
        } else {
            ClockEdge::NegEdgeHier(Symbol::intern(&hier_full))
        };
        // base_sid placeholder 0 hanya utk sigs tuple — normalize_event_sigs
        // menggantinya dgn SignalId nyata (resolve hier via hier_signal_map).
        Some((base_sid.unwrap_or(0), edge))
    }

    /// Kumpulkan chain member access `a.b.c` → (nama signal base, urutan step
    /// dari luar ke dalam). Contoh: `hw2reg.val.d` → ("hw2reg", [val, d]);
    /// `reg2hw.key_share0[0].qe` → ("reg2hw", [key_share0, Index(0), qe]).
    /// Index konstanta (genvar sudah di-substitute saat generate expansion),
    /// dievaluasi via `const_eval_with_params` agar ekspresi seperti `31-i`
    /// (sudah menjadi `31-0`, `31-1`, …) ikut ter-fold.
    pub(crate) fn collect_member_chain(
        obj: &Expr,
        leaf_field: Symbol,
        param_vals: &std::collections::HashMap<Symbol, i64>,
    ) -> Option<(String, Vec<ChainStep>)> {
        let mut chain = vec![ChainStep::Field(leaf_field)];
        let mut cur = obj;
        loop {
            match cur {
                Expr::MemberAccess { obj: inner, field } => {
                    chain.push(ChainStep::Field(*field));
                    cur = inner;
                }
                Expr::BitSelect { expr: inner, index } => {
                    if let Ok(idx) = const_eval_with_params(index, param_vals) {
                        chain.push(ChainStep::Index(idx));
                        cur = inner;
                    } else {
                        return None;
                    }
                }
                Expr::Ident { name, .. } => {
                    let base = name.as_str().to_string();
                    chain.reverse(); // [base_step, ..., leaf_step]
                    return Some((base, chain));
                }
                _ => return None,
            }
        }
    }

    /// Resolve chain member access struct (`a.b.c.d`) menjadi offset [lsb..msb]
    /// terhadap signal dasar `base_sid`. Memakai struct_fields base signal +
    /// typedef_field_map untuk step berjenjang. Mengembalikan (msb, lsb) bila
    /// seluruh step ter-resolve; None bila tidak (fallback jalur HierRef/
    /// MemberAccess runtime).
    pub(crate) fn resolve_struct_chain(
        &self,
        base_sid: SignalId,
        chain: &[ChainStep],
        signals: &[SignalInfo],
    ) -> Option<(usize, usize)> {
        let base_info = &signals[base_sid];
        if base_info.struct_fields.is_empty() {
            return None;
        }
        let mut offset = 0usize;
        let mut width = 1usize;
        let mut cur_fields: Option<Vec<StructFieldInfo>> = Some(base_info.struct_fields.clone());
        let mut last_field: Option<StructFieldInfo> = None;
        let mut ok = true;
        for (i, step) in chain.iter().enumerate() {
            match step {
                ChainStep::Index(idx) => {
                    let elem_width = if let Some(f) = &last_field {
                        f.type_name
                            .as_ref()
                            .and_then(|tn| self.lookup_struct_fields(tn.as_str()))
                            .map(|fs| fs.iter().map(|sf| sf.width).sum::<usize>().max(1))
                            .unwrap_or(1)
                    } else {
                        base_info.elem_width.max(1)
                    };
                    offset = offset.saturating_add((*idx as usize).saturating_mul(elem_width));
                    if let Some(f) = &last_field {
                        if let Some(tn) = &f.type_name {
                            cur_fields = self.lookup_struct_fields(tn.as_str());
                        }
                    }
                    last_field = None;
                }
                ChainStep::Field(fname) => {
                    let fields = match &cur_fields {
                        Some(fs) => fs.clone(),
                        None => {
                            ok = false;
                            break;
                        }
                    };
                    let found = fields.iter().find(|f| f.name == *fname);
                    match found {
                        Some(f) => {
                            offset += f.offset;
                            width = f.width;
                            last_field = Some(f.clone());
                            if i + 1 < chain.len() {
                                let mut nxt = f
                                    .type_name
                                    .as_ref()
                                    .and_then(|tn| self.lookup_struct_fields(tn.as_str()));
                                if nxt.is_none() {
                                    nxt = Some(f.sub_fields.clone());
                                }
                                cur_fields = nxt;
                            }
                        }
                        None => {
                            ok = false;
                            break;
                        }
                    }
                }
            }
        }
        if ok {
            Some((offset + width - 1, offset))
        } else {
            None
        }
    }

    /// Apakah field TERAKHIR dari chain struct bertipe signed (`int a;`)?
    /// Dipakai pembaca member agar hasilnya signed (LRM 1800 §6.24.1).
    pub(crate) fn struct_chain_leaf_signed(
        &self,
        base_sid: SignalId,
        chain: &[ChainStep],
        signals: &[SignalInfo],
    ) -> bool {
        let base_info = &signals[base_sid];
        let mut cur_fields: Option<Vec<StructFieldInfo>> = if base_info.struct_fields.is_empty() {
            None
        } else {
            Some(base_info.struct_fields.clone())
        };
        let mut signed = false;
        for (i, step) in chain.iter().enumerate() {
            match step {
                ChainStep::Field(fname) => {
                    let Some(fields) = cur_fields.as_ref() else {
                        return false;
                    };
                    let Some(f) = fields.iter().find(|f| f.name == *fname) else {
                        return false;
                    };
                    if i + 1 == chain.len() {
                        return f.is_signed;
                    }
                    let mut nxt = f
                        .type_name
                        .as_ref()
                        .and_then(|tn| self.lookup_struct_fields(tn.as_str()));
                    if nxt.is_none() {
                        nxt = Some(f.sub_fields.clone());
                    }
                    cur_fields = nxt;
                }
                // Elemen array struct: signedness mengikuti field induk.
                ChainStep::Index(_) => {
                    let Some(fields) = cur_fields.as_ref() else {
                        return false;
                    };
                    signed = fields.iter().any(|f| f.is_signed);
                    cur_fields = None;
                }
            }
        }
        signed
    }

    /// LANG-06: terjemahkan AST `Sequence` (SVA temporal) → `IrSequence`
    /// untuk evaluasi ber-clock oleh engine. Struktur 1:1 — Expr dievaluasi
    /// tiap cycle, Delay/DelayRange tunggu N cycle, Concat/Or/And/Repeat
    /// sesuai semantik sequence.
    fn elaborate_sequence(
        &self,
        seq: &mivon_ast::types::Sequence,
        signal_map: &HashMap<Symbol, SignalId>,
        signals: &[SignalInfo],
    ) -> Result<mivon_ir::IrSequence, SimError> {
        use mivon_ast::types::Sequence;
        use mivon_ir::IrSequence;
        Ok(match seq {
            Sequence::Expr(e) => IrSequence::Expr(self.elaborate_expr(e, signal_map, signals)?),
            Sequence::Delay(n) => IrSequence::Delay(*n),
            Sequence::DelayRange(a, b) => IrSequence::DelayRange(*a, *b),
            Sequence::Concat(l, r) => IrSequence::Concat(
                Box::new(self.elaborate_sequence(l, signal_map, signals)?),
                Box::new(self.elaborate_sequence(r, signal_map, signals)?),
            ),
            Sequence::Or(l, r) => IrSequence::Or(
                Box::new(self.elaborate_sequence(l, signal_map, signals)?),
                Box::new(self.elaborate_sequence(r, signal_map, signals)?),
            ),
            Sequence::And(l, r) => IrSequence::And(
                Box::new(self.elaborate_sequence(l, signal_map, signals)?),
                Box::new(self.elaborate_sequence(r, signal_map, signals)?),
            ),
            Sequence::Repeat(s, n) => IrSequence::Repeat(
                Box::new(self.elaborate_sequence(s, signal_map, signals)?),
                *n,
            ),
            Sequence::Implication(ante, cons) => IrSequence::Implication(
                Box::new(self.elaborate_sequence(ante, signal_map, signals)?),
                Box::new(self.elaborate_sequence(cons, signal_map, signals)?),
            ),
        })
    }
}

/// LANG-06: posisi source (line, col) ekspresi pertama dalam sequence
/// temporal — untuk record_assertion (key line:col di assertion_stats).
fn sequence_first_loc(seq: &mivon_ast::types::Sequence) -> (usize, usize) {
    use mivon_ast::types::Sequence;
    match seq {
        Sequence::Expr(e) => crate::util::generate::expr_location(e),
        Sequence::Delay(_) | Sequence::DelayRange(_, _) => (0, 0),
        Sequence::Concat(l, _) | Sequence::Or(l, _) | Sequence::And(l, _) => sequence_first_loc(l),
        Sequence::Repeat(s, _) => sequence_first_loc(s),
        Sequence::Implication(ante, _) => sequence_first_loc(ante),
    }
}

/// Langkah dalam chain member access — field struct atau index array konstanta.
#[derive(Debug, Clone)]
pub(crate) enum ChainStep {
    Field(Symbol),
    Index(i64),
}

/// Deteksi label rentang `[lo:hi]` pada case-inside. Parser merepresentasikan
/// rentang ini sebagai `RangeSelect` dengan base `Value::Decimal(0)` (pola
/// sentinel). Mengembalikan `(lo, hi)` jika cocok.
fn inside_range_bounds(label: &Expr) -> Option<(&Expr, &Expr)> {
    if let Expr::RangeSelect { expr, msb, lsb } = label {
        if matches!(&**expr, Expr::Value(Value::Decimal(0))) {
            return Some((msb, lsb));
        }
    }
    None
}

/// Konversi literal `Expr::Value` menjadi i64 (pola yang sama seperti const-fold
/// case biasa). Non-numerik mengembalikan 0.
fn value_to_i64(v: &Value) -> i64 {
    match v {
        Value::Decimal(d) => *d,
        Value::Hex { bits, .. } => mivon_ast::const_eval::parse_literal(
            bits.trim_start_matches("0x").trim_start_matches("0X"),
            16,
        )
        .unwrap_or(0),
        Value::Binary { bits, .. } => mivon_ast::const_eval::parse_literal(
            bits.trim_start_matches("0b").trim_start_matches("0B"),
            2,
        )
        .unwrap_or(0),
        Value::Octal { bits, .. } => mivon_ast::const_eval::parse_literal(
            bits.trim_start_matches("0o").trim_start_matches("0O"),
            8,
        )
        .unwrap_or(0),
        Value::Real(_) => 0,
    }
}

/// SIM-29: ekstrak baris sumber statement (1-based, koordinat output
/// preprocessed) dari AST. Memakai `expr_location` pada ekspresi utama
/// statement (lhs/cond/expr/delay — yang membawa line/col dari parser).
/// Statement tanpa ekspresi pembawa line mengembalikan 0 → tidak dicatat di
/// `stmt_lines` → tidak ikut ter-exclude (statement kosong/null tidak punya
/// makna line coverage).
fn stmt_source_line(s: &Stmt) -> usize {
    use crate::util::generate::expr_location;
    match s {
        Stmt::SysCall { line, .. } => *line,
        Stmt::Assert { cond, .. }
        | Stmt::Assume { cond, .. }
        | Stmt::Cover { cond, .. }
        | Stmt::Expect { cond, .. } => expr_location(cond).0,
        Stmt::BlockingAssign { lhs, .. }
        | Stmt::NonBlockingAssign { lhs, .. }
        | Stmt::StmtAssign { lhs, .. }
        | Stmt::Force { lhs, .. } => expr_location(lhs).0,
        Stmt::IfElse { cond, .. }
        | Stmt::UniqueIf { cond, .. }
        | Stmt::PriorityIf { cond, .. }
        | Stmt::Wait { cond, .. } => expr_location(cond).0,
        Stmt::Case { expr, .. }
        | Stmt::CaseX { expr, .. }
        | Stmt::CaseZ { expr, .. }
        | Stmt::UniqueCase { expr, .. }
        | Stmt::PriorityCase { expr, .. }
        | Stmt::Unique0Case { expr, .. }
        | Stmt::CaseInside { expr, .. }
        | Stmt::StmtCase { expr, .. } => expr_location(expr).0,
        Stmt::LoopWhile { cond, .. } | Stmt::DoWhile { cond, .. } => expr_location(cond).0,
        Stmt::LoopFor { cond, .. } => cond.as_ref().map(|c| expr_location(c).0).unwrap_or(0),
        Stmt::Repeat { count, .. } => expr_location(count).0,
        Stmt::Delay { delay, .. } => expr_location(delay).0,
        Stmt::EventControl { events, .. } => events
            .first()
            .map(|e| match e {
                mivon_ast::SensitivityEvent::PosEdge(ex)
                | mivon_ast::SensitivityEvent::NegEdge(ex)
                | mivon_ast::SensitivityEvent::Level(ex) => expr_location(ex).0,
                mivon_ast::SensitivityEvent::Wildcard | mivon_ast::SensitivityEvent::Iff { .. } => {
                    0
                }
            })
            .unwrap_or(0),
        Stmt::Return(Some(e)) => expr_location(e).0,
        _ => 0,
    }
}

/// Nama statis konstruk Stmt untuk instrumentasi DBG_STMT.
fn stmt_kind_name(s: &Stmt) -> &'static str {
    match s {
        Stmt::Block { .. } => "block",
        Stmt::NamedBlock { .. } => "named_block",
        Stmt::BlockingAssign { .. } => "blocking_assign",
        Stmt::NonBlockingAssign { .. } => "nonblocking_assign",
        Stmt::StmtAssign { .. } => "stmt_assign",
        Stmt::IfElse { .. } => "if",
        Stmt::Case { .. } => "case",
        Stmt::CaseX { .. } => "casex",
        Stmt::CaseZ { .. } => "casez",
        Stmt::UniqueCase { .. } => "unique_case",
        Stmt::PriorityCase { .. } => "priority_case",
        Stmt::Unique0Case { .. } => "unique0_case",
        Stmt::CaseInside { .. } => "case_inside",
        Stmt::PropertySeq { .. } => "property_seq",
        Stmt::Assert { .. } => "assert",
        Stmt::Assume { .. } => "assume",
        Stmt::Cover { .. } => "cover",
        Stmt::Expect { .. } => "expect",
        Stmt::WaitOrder { .. } => "wait_order",
        Stmt::UniqueIf { .. } => "unique_if",
        Stmt::PriorityIf { .. } => "priority_if",
        Stmt::Delay { .. } => "delay",
        Stmt::EventControl { .. } => "event_control",
        Stmt::EventTrigger { .. } => "event_trigger",
        Stmt::Wait { .. } => "wait",
        Stmt::WaitFork => "wait_fork",
        Stmt::LoopForever { .. } => "forever",
        Stmt::LoopFor { .. } => "for",
        Stmt::LoopWhile { .. } => "while",
        Stmt::DoWhile { .. } => "do_while",
        Stmt::Repeat { .. } => "repeat",
        Stmt::ForeachLoop { .. } => "foreach",
        Stmt::Fork { .. } => "fork",
        Stmt::SysCall { .. } => "syscall",
        Stmt::SysFinish => "sys_finish",
        Stmt::Disable { .. } => "disable",
        Stmt::Force { .. } => "force",
        Stmt::Release { .. } => "release",
        Stmt::Deassign { .. } => "deassign",
        Stmt::Return { .. } => "return",
        Stmt::Null => "null",
        Stmt::Expr { .. } => "expr",
        Stmt::StmtCase { .. } => "stmt_case",
        Stmt::RandCase { .. } => "randcase",
        Stmt::RandSequence { .. } => "randsequence",
        Stmt::Break => "break",
        Stmt::Continue => "continue",
    }
}

/// Dump + reset agregat waktu per konstruk Stmt (DBG_STMT). Dipanggil per
/// module oleh elaborator setelah items loop.
pub(crate) fn stmt_dbg_dump(module: &str) {
    thread_local! {
        static DBG_STMT_TIME: std::cell::RefCell<std::collections::HashMap<&'static str, (u64, u64)>> =
            std::cell::RefCell::new(std::collections::HashMap::new());
    }
    let total = DBG_STMT_TIME.with(|cell| {
        let mut m = cell.borrow_mut();
        let items: Vec<_> = m.drain().collect();
        items
    });
    if total.is_empty() {
        return;
    }
    let sum_ns: u64 = total.iter().map(|(_, (_, t))| *t).sum();
    let sum_n: u64 = total.iter().map(|(_, (n, _))| *n).sum();
    eprintln!(
        "[DBG-STMT] {}: {} stmts in {:.2}s",
        module,
        sum_n,
        sum_ns as f64 / 1e9
    );
    let mut sorted: Vec<_> = total.into_iter().collect();
    sorted.sort_by_key(|(_, (_, t))| std::cmp::Reverse(*t));
    for (k, (n, t)) in sorted.iter().take(10) {
        eprintln!(
            "  {:<18} {:>8} stmts  {:>8.2}s  ({:>5.1}%)",
            k,
            n,
            *t as f64 / 1e9,
            (*t as f64 / sum_ns as f64) * 100.0
        );
    }
}
