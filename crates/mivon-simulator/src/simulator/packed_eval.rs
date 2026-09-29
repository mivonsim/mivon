// Simulator submodule: packed logic evaluation functions
// Tanggung jawab: eval_unary_packed, eval_binary_packed, eval_binary_packed_extended

use super::packed::PackedLogicVec;
use mivon_ir::*;
pub fn eval_unary_packed(op: &UnaryIrOp, val: &PackedLogicVec) -> Option<PackedLogicVec> {
    match op {
        UnaryIrOp::Plus => Some(val.clone()),
        UnaryIrOp::BitNot => Some(val.bitwise_not()),
        UnaryIrOp::Not => {
            let b = val.to_bool()?;
            Some(PackedLogicVec::from_u64(if b { 0 } else { 1 }, 1))
        }
        UnaryIrOp::Minus => {
            // Two's complement: ~val + 1
            let not = val.bitwise_not();
            let one = PackedLogicVec::from_u64(1, val.width());
            Some(not.add(&one))
        }
        UnaryIrOp::RedAnd => Some(val.red_and()),
        UnaryIrOp::RedNand => Some(val.red_nand()),
        UnaryIrOp::RedOr => Some(val.red_or()),
        UnaryIrOp::RedNor => Some(val.red_nor()),
        UnaryIrOp::RedXor => Some(val.red_xor()),
        UnaryIrOp::RedXnor => Some(val.red_xnor()),
    }
}

/// Evaluate a binary operation using packed representation.
/// Returns None if the operation cannot be evaluated in packed form.
pub fn eval_binary_packed(
    op: &BinaryIrOp,
    lhs: &PackedLogicVec,
    rhs: &PackedLogicVec,
) -> Option<PackedLogicVec> {
    match op {
        BinaryIrOp::BitAnd => Some(lhs.bitwise_and(rhs)),
        BinaryIrOp::BitOr => Some(lhs.bitwise_or(rhs)),
        BinaryIrOp::BitXor => Some(lhs.bitwise_xor(rhs)),
        BinaryIrOp::BitXnor => Some(lhs.bitwise_xnor(rhs)),
        // Comparison ops (Eq, Neq, CaseEq, etc.) are handled by JIT or interpreted
        // for correct 1-bit width handling and X/Z semantics.
        _ => None,
    }
}
/// Check if a binary operation can be accelerated by packed eval.
/// Only true bitwise ops benefit from SIMD bitmask acceleration.
/// Comparison ops are handled by JIT or interpreted for correct width/XZ semantics.
pub fn is_packable_binary_op(op: &BinaryIrOp) -> bool {
    matches!(
        op,
        BinaryIrOp::BitAnd | BinaryIrOp::BitOr | BinaryIrOp::BitXor | BinaryIrOp::BitXnor
    )
}

/// Extended packed binary evaluator dengan width extension.
/// Fallback ke LogicVec untuk operasi non-bitwise.
pub fn eval_binary_packed_extended(
    op: &BinaryIrOp,
    lhs: &PackedLogicVec,
    rhs: &PackedLogicVec,
) -> PackedLogicVec {
    if let Some(result) = eval_binary_packed(op, lhs, rhs) {
        return result;
    }
    let lv_lhs = lhs.to_logicvec();
    let lv_rhs = rhs.to_logicvec();
    let result = crate::simulator::value::eval_binary(op.clone(), &lv_lhs, &lv_rhs);
    PackedLogicVec::from_logicvec(&result)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Construction Tests ───

    #[test]
    fn test_new_default_x() {
        let pv = PackedLogicVec::new(8);
        assert_eq!(pv.width(), 8);
        assert!(pv.all_x(), "new PackedLogicVec should be all X");
        assert_eq!(format!("{}", pv), "xxxxxxxx");
    }

    #[test]
    fn test_from_u64() {
        let pv = PackedLogicVec::from_u64(0b1010, 4);
        assert_eq!(format!("{}", pv), "1010");
        assert_eq!(pv.to_u64(), 0b1010);
    }

    #[test]
    fn test_from_u64_wide() {
        // >64 bit signal — hanya lower 64 bit yang terisi
        let pv = PackedLogicVec::from_u64(!0u64, 128);
        assert_eq!(pv.width(), 128);
        assert_eq!(pv.num_chunks(), 2);
        // 64 bit pertama = 1, 64 bit kedua = 0
        let lv = pv.to_logicvec();
        for (i, bit) in lv.bits.iter().take(64).enumerate() {
            assert_eq!(*bit, LogicVal::One, "bit {} should be 1", i);
        }
        for (i, bit) in lv.bits.iter().skip(64).take(64).enumerate() {
            assert_eq!(*bit, LogicVal::Zero, "bit {} should be 0", i + 64);
        }
    }

    #[test]
    fn test_fill_one() {
        let pv = PackedLogicVec::fill(LogicVal::One, 8);
        assert_eq!(format!("{}", pv), "11111111");
        let lv = pv.to_logicvec();
        assert!(lv.bits.iter().all(|b| *b == LogicVal::One));
    }

    #[test]
    fn test_fill_zero() {
        let pv = PackedLogicVec::fill(LogicVal::Zero, 4);
        assert_eq!(format!("{}", pv), "0000");
    }

    #[test]
    fn test_fill_z() {
        let pv = PackedLogicVec::fill(LogicVal::Z, 3);
        assert_eq!(format!("{}", pv), "zzz");
    }

    #[test]
    fn test_fill_single_bit() {
        let pv = PackedLogicVec::fill(LogicVal::One, 1);
        assert_eq!(
            pv.to_u64(),
            1,
            "fill(One, 1).to_u64() should be 1, got {}",
            pv.to_u64()
        );
    }

    #[test]
    fn test_from_logicvec() {
        let lv = LogicVec::from_u64(0b1100, 4);
        let pv = PackedLogicVec::from_logicvec(&lv);
        assert_eq!(format!("{}", pv), "1100");
    }

    #[test]
    fn test_to_logicvec_roundtrip() {
        let original = LogicVec::from_u64(0xDEAD, 16);
        let packed = PackedLogicVec::from_logicvec(&original);
        let recovered = packed.to_logicvec();
        assert_eq!(original, recovered);
    }

    #[test]
    fn test_to_logicvec_with_xz() {
        let lv = LogicVec {
            bits: vec![LogicVal::X, LogicVal::Z, LogicVal::Zero, LogicVal::One],
            width: 4,
        };
        let packed = PackedLogicVec::from_logicvec(&lv);
        let recovered = packed.to_logicvec();
        assert_eq!(lv, recovered);
    }

    // ─── Bitwise Operation Tests ───

    #[test]
    fn test_bitwise_and() {
        let a = PackedLogicVec::from_u64(0b1100, 4);
        let b = PackedLogicVec::from_u64(0b1010, 4);
        let r = a.bitwise_and(&b);
        assert_eq!(r.to_u64(), 0b1000);
        assert_eq!(format!("{}", r), "1000");
    }

    #[test]
    fn test_bitwise_or() {
        let a = PackedLogicVec::from_u64(0b1100, 4);
        let b = PackedLogicVec::from_u64(0b1010, 4);
        let r = a.bitwise_or(&b);
        assert_eq!(r.to_u64(), 0b1110);
    }

    #[test]
    fn test_bitwise_xor() {
        let a = PackedLogicVec::from_u64(0b1100, 4);
        let b = PackedLogicVec::from_u64(0b1010, 4);
        let r = a.bitwise_xor(&b);
        assert_eq!(r.to_u64(), 0b0110);
    }

    #[test]
    fn test_bitwise_not() {
        let a = PackedLogicVec::from_u64(0b1010, 4);
        let r = a.bitwise_not();
        assert_eq!(r.to_u64(), 0b0101);
    }

    #[test]
    fn test_bitwise_xnor() {
        let a = PackedLogicVec::from_u64(0b1100, 4);
        let b = PackedLogicVec::from_u64(0b1010, 4);
        let r = a.bitwise_xnor(&b);
        assert_eq!(r.to_u64(), 0b1001);
    }

    #[test]
    fn test_bitwise_and_x_0() {
        // LRM §11.4.1 Tabel 11-20: `0 & X = 0` — identitas dominasi WAJIB
        // di SEMUA mode (dulu "pessimistic: X dominan" menghasilkan X →
        // melanggar LRM → X menular tak terbatas thd input tak-ter-init,
        // fuzzer xmode/pmt). Sejak koreksi: pessimistic = LRM utk identitas.
        let x_packed = PackedLogicVec::fill(LogicVal::X, 4);
        let zero_packed = PackedLogicVec::fill(LogicVal::Zero, 4);
        let r = x_packed.bitwise_and(&zero_packed);
        let lv = r.to_logicvec();
        assert!(
            lv.bits.iter().all(|b| *b == LogicVal::Zero),
            "LRM: X & 0 should be 0, got {}",
            r
        );
    }

    #[test]
    fn test_bitwise_and_x_0_optimistic() {
        // Moda Optimistic memakai tabel LRM: 0 mendominasi AND → X & 0 = 0.
        use crate::simulator::types::XPropagationMode;
        use crate::simulator::value::set_xprop_mode;
        set_xprop_mode(XPropagationMode::Optimistic);
        let x_packed = PackedLogicVec::fill(LogicVal::X, 4);
        let zero_packed = PackedLogicVec::fill(LogicVal::Zero, 4);
        let r = x_packed.bitwise_and(&zero_packed);
        let lv = r.to_logicvec();
        assert!(
            lv.bits.iter().all(|b| *b == LogicVal::Zero),
            "optimistic: X & 0 should be 0, got {}",
            r
        );
    }

    #[test]
    fn test_bitwise_and_x_1() {
        // X & 1 = X
        let x_packed = PackedLogicVec::fill(LogicVal::X, 4);
        let one_packed = PackedLogicVec::fill(LogicVal::One, 4);
        let r = x_packed.bitwise_and(&one_packed);
        assert!(r.all_x(), "X & 1 should be X, got {}", r);
    }

    #[test]
    fn test_bitwise_or_x_1() {
        // LRM §11.4.1 Tabel 11-21: `1 | X = 1` — identitas dominasi WAJIB
        // di SEMUA mode (dulu "pessimistic: X dominan" → X, melanggar LRM).
        let x_packed = PackedLogicVec::fill(LogicVal::X, 4);
        let one_packed = PackedLogicVec::fill(LogicVal::One, 4);
        let r = x_packed.bitwise_or(&one_packed);
        let lv = r.to_logicvec();
        assert!(
            lv.bits.iter().all(|b| *b == LogicVal::One),
            "LRM: X | 1 should be 1, got {}",
            r
        );
    }

    #[test]
    fn test_bitwise_or_x_1_optimistic() {
        // Moda Optimistic memakai tabel LRM: 1 mendominasi OR → X | 1 = 1.
        use crate::simulator::types::XPropagationMode;
        use crate::simulator::value::set_xprop_mode;
        set_xprop_mode(XPropagationMode::Optimistic);
        let x_packed = PackedLogicVec::fill(LogicVal::X, 4);
        let one_packed = PackedLogicVec::fill(LogicVal::One, 4);
        let r = x_packed.bitwise_or(&one_packed);
        let lv = r.to_logicvec();
        assert!(
            lv.bits.iter().all(|b| *b == LogicVal::One),
            "optimistic: X | 1 should be 1, got {}",
            r
        );
    }

    #[test]
    fn test_bitwise_or_x_0() {
        // X | 0 = X
        let x_packed = PackedLogicVec::fill(LogicVal::X, 4);
        let zero_packed = PackedLogicVec::fill(LogicVal::Zero, 4);
        let r = x_packed.bitwise_or(&zero_packed);
        assert!(r.all_x(), "X | 0 should be X, got {}", r);
    }

    // ─── Reduction Tests ───

    #[test]
    fn test_red_and_all_ones() {
        let pv = PackedLogicVec::fill(LogicVal::One, 8);
        let r = pv.red_and();
        assert_eq!(
            r.to_u64(),
            1,
            "red_and all ones should be 1, got {}",
            r.to_u64()
        );
    }

    #[test]
    fn test_red_and_has_zero() {
        let pv = PackedLogicVec::from_u64(0b1110, 4);
        let r = pv.red_and();
        assert_eq!(r.to_u64(), 0);
    }

    #[test]
    fn test_red_or_all_zeros() {
        let pv = PackedLogicVec::fill(LogicVal::Zero, 8);
        let r = pv.red_or();
        assert_eq!(r.to_u64(), 0);
    }

    #[test]
    fn test_red_or_has_one() {
        let pv = PackedLogicVec::from_u64(0b0010, 4);
        let r = pv.red_or();
        assert_eq!(
            r.to_u64(),
            1,
            "red_or with bit=1 should be 1, got {}",
            r.to_u64()
        );
    }

    #[test]
    fn test_red_xor() {
        let pv = PackedLogicVec::from_u64(0b1010, 4);
        let r = pv.red_xor();
        // 1 xor 0 xor 1 xor 0 = 0
        assert_eq!(r.to_u64(), 0);
    }

    #[test]
    fn test_red_xor_odd() {
        let pv = PackedLogicVec::from_u64(0b1101, 4);
        let r = pv.red_xor();
        // 1 xor 1 xor 0 xor 1 = 1
        assert_eq!(
            r.to_u64(),
            1,
            "red_xor of 1101 should be 1, got {}",
            r.to_u64()
        );
    }

    #[test]
    fn test_red_nand() {
        let pv = PackedLogicVec::fill(LogicVal::One, 8);
        let r = pv.red_nand();
        assert_eq!(r.to_u64(), 0, "nand of all ones should be 0");
    }

    #[test]
    fn test_red_nor() {
        let pv = PackedLogicVec::fill(LogicVal::Zero, 8);
        let r = pv.red_nor();
        assert_eq!(r.to_u64(), 1, "nor of all zeros should be 1");
    }

    #[test]
    fn test_red_xnor() {
        let pv = PackedLogicVec::from_u64(0b1010, 4);
        let r = pv.red_xnor();
        assert_eq!(r.to_u64(), 1, "xnor of 1010 should be 1");
    }

    // ─── Shift Tests ───

    #[test]
    fn test_shl() {
        let pv = PackedLogicVec::from_u64(0b0001, 4);
        let r = pv.shl(2);
        assert_eq!(r.to_u64(), 0b0100);
    }

    #[test]
    fn test_shr() {
        let pv = PackedLogicVec::from_u64(0b1000, 4);
        let r = pv.shr(2);
        assert_eq!(r.to_u64(), 0b0010);
    }

    #[test]
    fn test_sshr_sign_extend() {
        // 4-bit: 1000 = -8 signed, shift right → 1110 = -2
        let pv = PackedLogicVec::from_u64(0b1000, 4);
        let r = pv.sshr(2);
        assert_eq!(format!("{}", r), "1110");
    }

    #[test]
    fn test_shl_full() {
        let pv = PackedLogicVec::from_u64(0b0001, 4);
        let r = pv.shl(4);
        assert_eq!(r.to_u64(), 0);
    }

    // ─── Comparison Tests ───

    #[test]
    fn test_eq_equal() {
        let a = PackedLogicVec::from_u64(0xAB, 8);
        let b = PackedLogicVec::from_u64(0xAB, 8);
        let r = a.eq(&b);
        assert_eq!(r.to_u64(), 1);
    }

    #[test]
    fn test_eq_not_equal() {
        let a = PackedLogicVec::from_u64(0xAB, 8);
        let b = PackedLogicVec::from_u64(0xCD, 8);
        let r = a.eq(&b);
        assert_eq!(r.to_u64(), 0);
    }

    #[test]
    fn test_eq_xz_match() {
        // X===X should be true, Z===Z should be true
        let x1 = PackedLogicVec::fill(LogicVal::X, 4);
        let x2 = PackedLogicVec::fill(LogicVal::X, 4);
        assert_eq!(x1.eq(&x2).to_u64(), 1, "X === X should be 1");

        let z1 = PackedLogicVec::fill(LogicVal::Z, 4);
        let z2 = PackedLogicVec::fill(LogicVal::Z, 4);
        assert_eq!(z1.eq(&z2).to_u64(), 1, "Z === Z should be 1");

        let r = x1.eq(&z1);
        assert_eq!(r.to_u64(), 0, "X === Z should be 0");
    }

    #[test]
    fn test_casex_eq() {
        let a = PackedLogicVec::from_u64(0b1010, 4);
        let pattern = PackedLogicVec::from_u64(0b1010, 4);
        let r = a.casex_eq(&pattern);
        assert_eq!(r.to_u64(), 1, "casex_eq with same value should be 1");
    }

    #[test]
    fn test_casex_eq_dont_care() {
        let a = PackedLogicVec::from_u64(0b1010, 4);
        // Pattern with X don't-care: 1x1x
        let pattern = PackedLogicVec::from_logicvec(&LogicVec {
            bits: vec![LogicVal::X, LogicVal::One, LogicVal::X, LogicVal::One],
            width: 4,
        });
        let r = a.casex_eq(&pattern);
        assert_eq!(r.to_u64(), 1, "casex_eq with X don't-care should be 1");
    }

    #[test]
    fn test_casez_eq() {
        let a = PackedLogicVec::from_u64(0b1010, 4);
        let pattern = PackedLogicVec::from_u64(0b1010, 4);
        let r = a.casez_eq(&pattern);
        assert_eq!(r.to_u64(), 1);
    }

    // ─── Conversion & Roundtrip Tests ───

    #[test]
    fn test_roundtrip_xz() {
        let lv = LogicVec {
            bits: vec![LogicVal::X, LogicVal::Z, LogicVal::X, LogicVal::Z],
            width: 4,
        };
        let packed = PackedLogicVec::from_logicvec(&lv);
        let recovered = packed.to_logicvec();
        assert_eq!(lv, recovered);
    }

    #[test]
    fn test_resize_truncate() {
        let pv = PackedLogicVec::from_u64(0b1111, 8);
        let r = pv.resize(4);
        assert_eq!(r.to_u64(), 0b1111);
        assert_eq!(r.width(), 4);
    }

    #[test]
    fn test_resize_extend() {
        let pv = PackedLogicVec::from_u64(0b1111, 4);
        let r = pv.resize(8);
        // Extend = ZERO-extend (konsisten dgn LogicVec::resize + semantik
        // assignment Verilog: bit baru bernilai 0, known=1).
        assert_eq!(r.to_u64(), 0b1111);
        let lv = r.to_logicvec();
        assert_eq!(lv.bits[4], LogicVal::Zero);
        assert_eq!(lv.bits[7], LogicVal::Zero);
    }

    // ─── Display Tests ───

    #[test]
    fn test_display() {
        let pv = PackedLogicVec::from_logicvec(&LogicVec {
            bits: vec![LogicVal::One, LogicVal::Zero, LogicVal::X, LogicVal::Z],
            width: 4,
        });
        // LV bits[0]=One(LSB), [1]=Zero, [2]=X, [3]=Z(MSB)
        // Display MSB-first: Z X 0 1
        assert_eq!(format!("{}", pv), "zx01");
    }

    #[test]
    fn test_display_all_x() {
        let pv = PackedLogicVec::new(4);
        assert_eq!(format!("{}", pv), "xxxx");
    }

    // ─── Evaluator Tests ───

    #[test]
    fn test_eval_binary_packed_bitand() {
        let a = PackedLogicVec::from_u64(0xFF, 8);
        let b = PackedLogicVec::from_u64(0x0F, 8);
        let r = eval_binary_packed(&BinaryIrOp::BitAnd, &a, &b).unwrap();
        assert_eq!(r.to_u64(), 0x0F);
    }

    #[test]
    fn test_eval_binary_packed_bitor() {
        let a = PackedLogicVec::from_u64(0xF0, 8);
        let b = PackedLogicVec::from_u64(0x0F, 8);
        let r = eval_binary_packed(&BinaryIrOp::BitOr, &a, &b).unwrap();
        assert_eq!(r.to_u64(), 0xFF);
    }

    #[test]
    fn test_eval_unary_packed_bitnot() {
        let a = PackedLogicVec::from_u64(0xFF, 8);
        let r = eval_unary_packed(&UnaryIrOp::BitNot, &a).unwrap();
        assert_eq!(r.to_u64(), 0x00);
    }

    #[test]
    fn test_eval_unary_packed_red_and() {
        let a = PackedLogicVec::fill(LogicVal::One, 8);
        let r = eval_unary_packed(&UnaryIrOp::RedAnd, &a).unwrap();
        assert_eq!(
            r.to_u64(),
            1,
            "red_and all ones via eval should be 1, got {}",
            r.to_u64()
        );
    }

    #[test]
    fn test_eval_unary_packed_red_or() {
        let a = PackedLogicVec::from_u64(0x00, 8);
        let r = eval_unary_packed(&UnaryIrOp::RedOr, &a).unwrap();
        assert_eq!(r.to_u64(), 0);
    }

    #[test]
    fn test_eval_unary_packed_logical_not() {
        let a = PackedLogicVec::from_u64(1, 1);
        let r = eval_unary_packed(&UnaryIrOp::Not, &a).unwrap();
        assert_eq!(r.to_u64(), 0);
    }

    #[test]
    fn test_eval_binary_packed_eq() {
        // Eq is now handled by JIT/interpreted, not packed eval
        let a = PackedLogicVec::from_u64(42, 8);
        let b = PackedLogicVec::from_u64(42, 8);
        let r = eval_binary_packed(&BinaryIrOp::Eq, &a, &b);
        assert!(r.is_none(), "Eq should return None (not packable)");
    }

    #[test]
    fn test_eval_binary_packed_eq_wild() {
        // EqWild is now handled by JIT/interpreted, not packed eval
        let a = PackedLogicVec::from_u64(0b1010, 4);
        let pattern = PackedLogicVec::from_u64(0b1010, 4);
        let r = eval_binary_packed(&BinaryIrOp::EqWild, &a, &pattern);
        assert!(r.is_none(), "EqWild should return None (not packable)");
    }

    // ─── Cross-validation with LogicVec ───

    fn cross_validate_binary(op: BinaryIrOp) {
        let a_lv = LogicVec::from_u64(0xACE, 12);
        let b_lv = LogicVec::from_u64(0xDEF, 12);
        let a_pv = PackedLogicVec::from_logicvec(&a_lv);
        let b_pv = PackedLogicVec::from_logicvec(&b_lv);

        let lv_result = crate::simulator::value::eval_binary(op.clone(), &a_lv, &b_lv);
        let pv_result = eval_binary_packed_extended(&op, &a_pv, &b_pv).to_logicvec();

        assert_eq!(lv_result, pv_result, "Mismatch for op {:?}", op);
    }

    #[test]
    fn test_cross_validate_bitwise_and() {
        cross_validate_binary(BinaryIrOp::BitAnd);
    }

    #[test]
    fn test_cross_validate_bitwise_or() {
        cross_validate_binary(BinaryIrOp::BitOr);
    }

    #[test]
    fn test_cross_validate_bitwise_xor() {
        cross_validate_binary(BinaryIrOp::BitXor);
    }

    #[test]
    fn test_cross_validate_bitwise_xnor() {
        cross_validate_binary(BinaryIrOp::BitXnor);
    }

    /// REGRESI (mivon-fuzz EMI): packed (tabel LRM) dan value.rs pessimistic
    /// tidak sepakat utk `x & 0` / `x | 1` — hasil sim tergantung flag
    /// `--packed` (API default ON, CLI default OFF) → mismatch EMI palsu dan
    /// hasil CLI ≠ API utk source sama. Packed kini menghormati
    /// XPropagationMode; setiap mode harus identik dgn eval_binary.
    #[test]
    fn test_packed_xz_matches_value_across_modes() {
        use crate::simulator::types::XPropagationMode;
        use crate::simulator::value::{get_xprop_mode, set_xprop_mode};
        let prev = get_xprop_mode();
        let x = LogicVec::fill(LogicVal::X, 2);
        let z = LogicVec::fill(LogicVal::Z, 2);
        let ops = [BinaryIrOp::BitAnd, BinaryIrOp::BitOr, BinaryIrOp::BitXor];
        let pairs = [
            (LogicVec::from_u64(0, 2), x.clone()),
            (x.clone(), LogicVec::from_u64(0, 2)),
            (LogicVec::from_u64(1, 2), x.clone()),
            (LogicVec::from_u64(0, 2), z.clone()),
            (LogicVec::from_u64(1, 2), z.clone()),
            (z.clone(), z.clone()),
            (x.clone(), z.clone()),
        ];
        for mode in [
            XPropagationMode::Optimistic,
            XPropagationMode::Pessimistic,
            XPropagationMode::XAnywhere,
        ] {
            set_xprop_mode(mode);
            for (i, (a, b)) in pairs.iter().enumerate() {
                for op in &ops {
                    let lv = crate::simulator::value::eval_binary(op.clone(), a, b);
                    let pv = eval_binary_packed_extended(
                        op,
                        &PackedLogicVec::from_logicvec(a),
                        &PackedLogicVec::from_logicvec(b),
                    )
                    .to_logicvec();
                    assert_eq!(
                        lv, pv,
                        "mismatch op={:?} mode={:?} pair={} (a={} b={})",
                        op, mode, i, a, b
                    );
                }
            }
        }
        set_xprop_mode(prev);
    }

    #[test]
    fn test_cross_validate_eq() {
        cross_validate_binary(BinaryIrOp::Eq);
    }

    #[test]
    fn test_cross_validate_neq() {
        cross_validate_binary(BinaryIrOp::Neq);
    }

    #[test]
    fn test_cross_validate_bitwise_not() {
        let lv = LogicVec::from_u64(0xACE, 12);
        let pv = PackedLogicVec::from_logicvec(&lv);

        let lv_result = crate::simulator::value::eval_unary(UnaryIrOp::BitNot, &lv);
        let pv_result = eval_unary_packed(&UnaryIrOp::BitNot, &pv)
            .unwrap()
            .to_logicvec();

        assert_eq!(lv_result, pv_result);
    }

    #[test]
    fn test_cross_validate_red_and() {
        let lv = LogicVec::from_u64(0xFF, 8);
        let pv = PackedLogicVec::from_logicvec(&lv);

        let lv_result = crate::simulator::value::eval_unary(UnaryIrOp::RedAnd, &lv);
        let pv_result = eval_unary_packed(&UnaryIrOp::RedAnd, &pv)
            .unwrap()
            .to_logicvec();

        assert_eq!(
            lv_result, pv_result,
            "red_and cross: LV={:?}, PV={:?}",
            lv_result, pv_result
        );
    }

    #[test]
    fn test_cross_validate_red_or() {
        let lv = LogicVec::from_u64(0xF0, 8);
        let pv = PackedLogicVec::from_logicvec(&lv);

        let lv_result = crate::simulator::value::eval_unary(UnaryIrOp::RedOr, &lv);
        let pv_result = eval_unary_packed(&UnaryIrOp::RedOr, &pv)
            .unwrap()
            .to_logicvec();

        assert_eq!(lv_result, pv_result);
    }

    #[test]
    fn test_cross_validate_red_xor() {
        let lv = LogicVec::from_u64(0xAA, 8);
        let pv = PackedLogicVec::from_logicvec(&lv);

        let lv_result = crate::simulator::value::eval_unary(UnaryIrOp::RedXor, &lv);
        let pv_result = eval_unary_packed(&UnaryIrOp::RedXor, &pv)
            .unwrap()
            .to_logicvec();

        assert_eq!(lv_result, pv_result);
    }

    #[test]
    fn test_cross_validate_wide_signals() {
        // 128-bit signal
        let lv_a = LogicVec::from_u64(0xABCD_EF01, 128);
        let lv_b = LogicVec::from_u64(0xDEAD_BEEF, 128);
        let pv_a = PackedLogicVec::from_logicvec(&lv_a);
        let pv_b = PackedLogicVec::from_logicvec(&lv_b);

        let lv_result = crate::simulator::value::eval_binary(BinaryIrOp::BitXor, &lv_a, &lv_b);
        let pv_result = pv_a.bitwise_xor(&pv_b).to_logicvec();

        assert_eq!(lv_result.width, pv_result.width);
        assert_eq!(lv_result, pv_result);
    }

    // ─── Edge Cases ───

    #[test]
    fn test_all_x_empty() {
        let pv = PackedLogicVec::new(0);
        assert_eq!(pv.width(), 1);
        assert!(pv.all_x());
    }

    #[test]
    fn test_all_z_check() {
        let pv = PackedLogicVec::fill(LogicVal::Z, 8);
        assert!(pv.all_z(), "fill(Z, 8).all_z() should be true, got {}", pv);
    }

    #[test]
    fn test_to_bool_known() {
        let pv = PackedLogicVec::from_u64(1, 1);
        assert_eq!(pv.to_bool(), Some(true));

        let pv = PackedLogicVec::from_u64(0, 1);
        assert_eq!(pv.to_bool(), Some(false));
    }

    #[test]
    fn test_to_bool_x() {
        let pv = PackedLogicVec::fill(LogicVal::X, 1);
        assert_eq!(pv.to_bool(), None);
    }

    #[test]
    fn test_extend() {
        let a = PackedLogicVec::from_u64(0xFF, 8);
        let b = PackedLogicVec::from_u64(0xAA, 8);
        let r = a.extend(&b);
        assert_eq!(r.width(), 16);
        let lv = r.to_logicvec();
        assert_eq!(lv.bits[0], LogicVal::One); // LSB from a
    }

    #[test]
    fn test_eq_different_widths() {
        let a = PackedLogicVec::from_u64(0xFF, 8);
        let b = PackedLogicVec::from_u64(0x00FF, 16);
        // Different widths should be NOT equal (=== semantics)
        let r = a.eq(&b);
        assert_eq!(r.to_u64(), 0, "diff widths should not be equal");
    }

    #[test]
    fn test_resize_identity() {
        let pv = PackedLogicVec::from_u64(0xABCD, 16);
        let r = pv.resize(16);
        assert_eq!(pv, r);
    }

    #[test]
    fn test_fill_x_then_to_u64() {
        let pv = PackedLogicVec::fill(LogicVal::X, 32);
        assert_eq!(pv.to_u64(), 0, "X fill should give 0 from to_u64");
    }

    // ─── Integration: cross-validate ALL ops via eval_binary_packed ───

    #[test]
    fn test_is_packable_binary_op_true() {
        assert!(is_packable_binary_op(&BinaryIrOp::BitAnd));
        assert!(is_packable_binary_op(&BinaryIrOp::BitOr));
        assert!(is_packable_binary_op(&BinaryIrOp::BitXor));
        assert!(is_packable_binary_op(&BinaryIrOp::BitXnor));
        assert!(!is_packable_binary_op(&BinaryIrOp::Eq)); // Eq now handled by JIT
        assert!(!is_packable_binary_op(&BinaryIrOp::Neq)); // Neq now handled by JIT
    }

    #[test]
    fn test_is_packable_binary_op_false() {
        assert!(!is_packable_binary_op(&BinaryIrOp::Add));
        assert!(!is_packable_binary_op(&BinaryIrOp::Sub));
        assert!(!is_packable_binary_op(&BinaryIrOp::Mul));
        assert!(!is_packable_binary_op(&BinaryIrOp::Div));
        assert!(!is_packable_binary_op(&BinaryIrOp::Shl));
        assert!(!is_packable_binary_op(&BinaryIrOp::Shr));
        assert!(!is_packable_binary_op(&BinaryIrOp::Lt));
        assert!(!is_packable_binary_op(&BinaryIrOp::Gt));
        assert!(!is_packable_binary_op(&BinaryIrOp::LogicalAnd));
        assert!(!is_packable_binary_op(&BinaryIrOp::LogicalOr));
    }

    /// Stress test: verify ALL bitwise ops produce identical results
    /// between classic LogicVec eval and packed eval for random 64-bit values.
    #[test]
    fn test_stress_cross_validate_all_bitwise_ops() {
        use rand::Rng;
        let mut rng = rand::thread_rng();

        let ops = [
            BinaryIrOp::BitAnd,
            BinaryIrOp::BitOr,
            BinaryIrOp::BitXor,
            BinaryIrOp::BitXnor,
        ];

        for _ in 0..100 {
            let a_val: u64 = rng.gen();
            let b_val: u64 = rng.gen();
            let width: usize = rng.gen_range(1..=64);

            let lv_a = LogicVec::from_u64(a_val, width);
            let lv_b = LogicVec::from_u64(b_val, width);
            let pv_a = PackedLogicVec::from_u64(a_val, width);
            let pv_b = PackedLogicVec::from_u64(b_val, width);

            for op in &ops {
                let lv_result = crate::simulator::value::eval_binary(op.clone(), &lv_a, &lv_b);
                let pv_result = eval_binary_packed(op, &pv_a, &pv_b)
                    .expect("packed eval returned None for bitwise op")
                    .to_logicvec();
                assert_eq!(
                    lv_result, pv_result,
                    "Mismatch for op={:?} a=0x{:X} b=0x{:X} width={}",
                    op, a_val, b_val, width
                );
            }
        }
    }

    /// Verify packed eval produces identical results as regular eval for a simple design.
    /// Creates two engines (packed on/off) and compares signal values.
    #[test]
    fn test_simulation_equivalence_packed_vs_regular() {
        let source = r#"
module top;
    reg [7:0] a, b, c_and, c_or, c_xor, c_xnor;
    reg eq_flag, neq_flag;
    initial begin
        a = 8'hA5;
        b = 8'h5A;
        c_and = a & b;
        c_or  = a | b;
        c_xor = a ^ b;
        c_xnor = ~(a ^ b);
        eq_flag = (a == b);
        neq_flag = (a != b);
        #1 $finish;
    end
endmodule
"#;
        use crate::simulator::SimulationEngine;
        use crate::test_util::compile_str;

        let design = compile_str(source).unwrap();

        // Run with regular eval
        let mut engine_regular = SimulationEngine::new(design.clone(), 10);
        engine_regular.use_packed_eval = false;
        engine_regular.run().unwrap();

        // Run with packed eval
        let mut engine_packed = SimulationEngine::new(design, 10);
        engine_packed.use_packed_eval = true;
        engine_packed.run().unwrap();

        // Compare all signal values
        for (i, sig) in engine_regular.design.top.signals.iter().enumerate() {
            let regular_val = engine_regular.state.read_signal(i).clone();
            let packed_val = engine_packed.state.read_signal(i).clone();
            assert_eq!(
                regular_val, packed_val,
                "Signal '{}' mismatch: regular={} packed={}",
                sig.name, regular_val, packed_val
            );
        }
    }

    #[test]
    fn test_all_x_partial_chunk() {
        // 7-bit signal (not aligned to 64)
        let pv = PackedLogicVec::new(7);
        assert!(pv.all_x(), "7-bit new should be all X");
        assert_eq!(pv.width(), 7);
        assert_eq!(format!("{}", pv), "xxxxxxx");
    }
}
