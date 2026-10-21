//! Signedness ekspresi IR + konversi LogicVec → string.
//!
//! Dipisah dari `util.rs`: dua helper murni (tanpa state engine) yang
//! dipakai formatter `$display` dan evaluator.

use mivon_ir::*;

pub fn is_signed_expr(expr: &IrExpr, signals: &[SignalInfo]) -> bool {
    match expr {
        IrExpr::Signed(_) => true,
        // `$rtoi` menghasilkan integer 32-bit BERTANDA (LRM 1800 §20.8) —
        // tanpa ini `$display("%0d", $rtoi(-2.5))` mencetak 4294967294.
        IrExpr::SysFunc { name, .. } if name.as_str() == "$rtoi" => true,
        // Cast wrapper (konteks aritmetik me-resize literal/sinyal signed ke
        // lebar umum via `Cast`) — signedness berasal dari sinyal DALAM
        // (bug #8: `a >>> 1` gagal aritmetik krn lhs = Cast{32, Signal}).
        IrExpr::Cast { expr: inner, .. } => is_signed_expr(inner, signals),
        IrExpr::Signal(id, _) | IrExpr::BitSelect(id, _) | IrExpr::RangeSelect(id, ..) => {
            signals.get(*id).map(|s| s.is_signed).unwrap_or(false)
        }
        IrExpr::ArrayIndex { sig_id, .. } => {
            signals.get(*sig_id).map(|s| s.is_signed).unwrap_or(false)
        }
        // ROUND 36: signedness PROPAGASI untuk ekspresi majemuk. Keputusan
        // operasi di evaluator memakai `&&` (LRM §11.8.2: 'ada operand
        // unsigned → hasil unsigned' → operasi signed hanya bila KEDUA
        // operand signed). Literal desimal unsized (`5`) kini di-emit
        // elaborator sebagai IrExpr::Signed (LRM §6.8.1) agar `a < 0` /
        // `a / 2` tetap signed sedangkan `a < 8'hFF` unsigned.
        IrExpr::BinaryOp(op, lhs, rhs) => {
            // LRM §11.8.1 Tabel 11-21: hasil operator perbandingan & logical
            // SELALU unsigned (1-bit), apa pun signedness operandnya. Tanpa
            // pengecualian ini `(-a <= b) >>> x` menandai lhs shift sbg
            // signed → 1'b1 di-sign-extend jadi all-ones (ditemukan fuzzer
            // signed_fuzz seed=1/30; emas + Icarus: zero-extend).
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
            // LRM §11.8.2 Tabel 11-21: hasil SHIFT mengikuti signedness
            // OPERAN KIRI saja (rhs self-determined, tidak berpengaruh).
            // Dulu `l && r` — `signed_neg >> (unsigned_cmp)` salah jadi
            // unsigned sehingga induk comparison jalan tanpa tanda
            // (ditemukan fuzzer signed_fuzz seed=111; emas + Icarus:
            // signed).
            if matches!(
                op,
                BinaryIrOp::Shl | BinaryIrOp::Shr | BinaryIrOp::Sshl | BinaryIrOp::Sshr
            ) {
                return is_signed_expr(lhs, signals);
            }
            is_signed_expr(lhs, signals) && is_signed_expr(rhs, signals)
        }
        IrExpr::UnaryOp(op, inner) => {
            match op {
                // Unary minus on unsigned stays unsigned (SV: -unsigned = unsigned)
                // Only signed if inner is already signed
                UnaryIrOp::Minus => is_signed_expr(inner, signals),
                // Other unary ops propagate signedness
                _ => is_signed_expr(inner, signals),
            }
        }
        IrExpr::Cond(_, t, f) => is_signed_expr(t, signals) || is_signed_expr(f, signals),
        IrExpr::ExprRangeSelect(inner, ..) | IrExpr::ExprBitSelect(inner, ..) => {
            is_signed_expr(inner, signals)
        }
        // Concat/Replicate menghasilkan nilai unsigned (LRM §11.8.1); Cast
        // lebar, MemberAccess, Inside, SysFunc, dst. → unsigned (konservatif).
        _ => false,
    }
}

pub fn logicvec_to_string(lv: &LogicVec) -> String {
    let mut s = String::new();
    let mut i = 0;
    // Iterasi per 8-bit (byte) sampai habis atau sisa < 8 bit. Konsumsi
    // penuh tiap byte (`i + 8 <= lv.width`) agar sisa (residual) selalu < 8
    // bit — ini mencegah shift-overflow di blok residual (`1 << j` pada u8
    // hanya valid untuk j < 8).
    while i + 8 <= lv.width {
        let mut byte = 0u8;
        for j in 0..8 {
            if lv.bits[i + j] == LogicVal::One {
                byte |= 1u8 << j;
            }
        }
        // F19: henti di byte NUL PENUTUP (C-style terminator). Hanya byte NUL
        // terakhir yang menghentikan — `string_to_logicvec` selalu mengakhiri
        // string dg NUL tunggal, sehingga ini memotong suffix `\0` (penting
        // agar wildcard config_db UVM cocok). NUL DI TENGAH (mis. hasil
        // `putc(i, c)` dg i > len yang memanjangkan dgn NUL) TETAP dipertahankan
        // sesuai semantik SV `%s` (NUL internal tak terlihat saat display).
        let is_last_byte = i + 8 == lv.width;
        if byte == 0 && is_last_byte {
            break;
        }
        s.push(byte as char);
        i += 8;
    }
    // Sisa < 8 bit (hanya terjadi bila lv.width tak kelipatan 8) — tangani
    // tanpa overflow shift: batasi j ke < 8.
    if i < lv.width {
        let mut byte = 0u8;
        for j in 0..(lv.width - i).min(8) {
            if lv.bits[i + j] == LogicVal::One {
                byte |= 1u8 << j;
            }
        }
        if byte != 0 {
            s.push(byte as char);
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regresi pemisahan modul: byte NUL penutup dipangkas, NUL internal tetap.
    #[test]
    fn logicvec_to_string_trims_trailing_nul_only() {
        let mut bytes = vec![b'h', b'i'];
        bytes.push(0);
        let mut lv = LogicVec::new(bytes.len() * 8);
        for (i, b) in bytes.iter().enumerate() {
            for j in 0..8 {
                if (b >> j) & 1 == 1 {
                    lv.bits[i * 8 + j] = LogicVal::One;
                }
            }
        }
        assert_eq!(logicvec_to_string(&lv), "hi");
    }
}
