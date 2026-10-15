//! ──────────────────────────────────────────────────────────────────────────────
//! Tanggung jawab: konversi INDEKS deklarasi packed vector → POSISI bit fisik.
//!
//! SystemVerilog mengizinkan rentang packed menurun (`[7:0]`, msb>=lsb) MAUPUN
//! menaik (`[0:7]`, msb<lsb), dan lsb tidak harus 0 (`[7:4]`). Elaborator
//! menyimpan seleksi bit sebagai POSISI (0 = LSB) di IR (`IrExpr::BitSelect`,
//! `IrExpr::RangeSelect`, `IrLValue::*`) karena engine mengindeks `LogicVec`
//! dari LSB. Karena itu setiap `a[i]` / `a[hi:lo]` harus dipetakan dari indeks
//! deklarasi ke posisi sebelum masuk IR.
//!
//! ──────────────────────────────────────────────────────────────────────────────

use mivon_ir::{BinaryIrOp, IrExpr, LogicVec};

/// Petakan indeks deklarasi `idx` (mis. dari `a[idx]`) ke POSISI bit fisik
/// (0 = LSB) pada signal berentang `[msb:lsb]`.
///
/// - Rentang menurun (`msb >= lsb`, mis. `[7:0]`, `[7:4]`): `pos = idx - lsb`.
/// - Rentang menaik (`msb < lsb`, mis. `[0:7]`, `[0:3]`): `pos = lsb - idx`.
///
/// Tanpa konversi ini, `a[idx]` selalu dibaca sebagai bit ke-`idx` dari LSB —
/// benar hanya bila `lsb == 0` dan rentang menurun; salah untuk `[7:4]`
/// (membaca bit di luar batas → X) dan untuk rentang menaik `[0:3]` (bit
/// terbalik). Ditemukan fuzzer diferensial vs Icarus (`verify_bad_0021`).
///
/// Indeks di ATAS rentang (mis. `a[3]` pada `a[1:0]`) menghasilkan posisi
/// `>= width` sehingga engine mengembalikan X sesuai LRM §11.5.1, dan tetap
/// menjaga EKSTENSI range-select (`a[3:0]` pada `a[1:0]` → 4 bit `xx01`, bukan
/// 3 bit). Indeks negatif (dikonversi pemanggil ke `usize` besar) juga jatuh
/// ke jalur OOB ini.
///
/// Catatan: indeks di BAWAH lsb (rentang menurun `lsb != 0`) atau di ATAS lsb
/// (rentang menaik) ter-saturasi ke 0 — deviasi kecil hanya untuk seleksi di
/// luar rentang pada sisi yang jarang dipakai (perilaku OOB tidak dijamin
/// LRM-identik untuk kasus ini).
#[inline]
pub fn index_to_bitpos(idx: usize, msb: usize, lsb: usize) -> usize {
    if msb >= lsb {
        idx.saturating_sub(lsb)
    } else {
        lsb.saturating_sub(idx)
    }
}

/// Bangun ekspresi POSISI bit fisik dari ekspresi indeks DINAMIS `idx_expr`
/// (mis. `a[i]` dengan `i` variabel) pada signal berentang `[msb:lsb]`.
///
/// - Menurun `lsb == 0` → `idx_expr` apa adanya (identitas).
/// - Menurun `lsb != 0` → `idx_expr - lsb`.
/// - Menaik → `lsb - idx_expr`.
///
/// Indeks di luar rentang menghasilkan nilai negatif (two's-complement) yang
/// oleh evaluator `ExprPartSelect` direinterpretasi negatif → X (LRM §11.5.1).
pub fn decl_index_to_pos_expr(idx_expr: IrExpr, msb: usize, lsb: usize) -> IrExpr {
    let lsb_const = |v: usize| IrExpr::Const(LogicVec::from_u64(v as u64, 32));
    if msb >= lsb {
        if lsb == 0 {
            idx_expr
        } else {
            IrExpr::BinaryOp(
                BinaryIrOp::Sub,
                Box::new(idx_expr),
                Box::new(lsb_const(lsb)),
            )
        }
    } else {
        IrExpr::BinaryOp(
            BinaryIrOp::Sub,
            Box::new(lsb_const(lsb)),
            Box::new(idx_expr),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::index_to_bitpos;

    #[test]
    fn descending_zero_lsb_identity() {
        // [7:0]: indeks == posisi (perilaku lama dipertahankan).
        assert_eq!(index_to_bitpos(0, 7, 0), 0);
        assert_eq!(index_to_bitpos(3, 7, 0), 3);
        assert_eq!(index_to_bitpos(7, 7, 0), 7);
        // Di atas rentang → posisi >= width (engine → X), ekstensi terjaga:
        // `a[3:0]` pada `a[1:0]` memakai pos 3 & 0 → 4 bit.
        assert_eq!(index_to_bitpos(8, 7, 0), 8);
        assert_eq!(index_to_bitpos(3, 1, 0), 3);
    }

    #[test]
    fn descending_nonzero_lsb_offsets() {
        // [7:4]: indeks 4 → posisi 0 (LSB), indeks 7 → posisi 3 (MSB).
        assert_eq!(index_to_bitpos(4, 7, 4), 0);
        assert_eq!(index_to_bitpos(5, 7, 4), 1);
        assert_eq!(index_to_bitpos(7, 7, 4), 3);
        // Di atas rentang → posisi >= width (= 4) → X.
        assert_eq!(index_to_bitpos(8, 7, 4), 4);
    }

    #[test]
    fn ascending_reverses() {
        // [0:3]: indeks 0 → posisi 3 (MSB), indeks 3 → posisi 0 (LSB).
        assert_eq!(index_to_bitpos(0, 0, 3), 3);
        assert_eq!(index_to_bitpos(1, 0, 3), 2);
        assert_eq!(index_to_bitpos(3, 0, 3), 0);
        // Di atas rentang (indeks > lsb) → saturasi 0 (deviasi terdokumentasi).
        assert_eq!(index_to_bitpos(9, 0, 3), 0);
    }

    #[test]
    fn single_bit() {
        assert_eq!(index_to_bitpos(0, 0, 0), 0);
        // Bit tunggal di luar indeks 0 → posisi 1 (>= width) → X.
        assert_eq!(index_to_bitpos(1, 0, 0), 1);
    }

    #[test]
    fn dynamic_pos_expr_shape() {
        use super::decl_index_to_pos_expr;
        use mivon_ir::{BinaryIrOp, IrExpr};

        // Menurun lsb == 0 → identitas (tidak dibungkus).
        let id = IrExpr::Signal(7, 8);
        let out = decl_index_to_pos_expr(id.clone(), 7, 0);
        assert!(matches!(out, IrExpr::Signal(7, 8)));

        // Menurun lsb != 0 → `idx - lsb`.
        let out = decl_index_to_pos_expr(id.clone(), 7, 4);
        match out {
            IrExpr::BinaryOp(BinaryIrOp::Sub, _, rhs) => {
                assert!(matches!(*rhs, IrExpr::Const(ref c) if c.to_u64() == 4));
            }
            other => panic!("expected Sub, got {other:?}"),
        }

        // Menaik → `lsb - idx`.
        let out = decl_index_to_pos_expr(id, 0, 3);
        match out {
            IrExpr::BinaryOp(BinaryIrOp::Sub, lhs, _) => {
                assert!(matches!(*lhs, IrExpr::Const(ref c) if c.to_u64() == 3));
            }
            other => panic!("expected Sub, got {other:?}"),
        }
    }
}
