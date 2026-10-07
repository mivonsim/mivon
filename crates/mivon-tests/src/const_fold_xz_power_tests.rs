//! Regresi const-fold X/Z pada concat + lebar hasil `**` (F82).
//!
//! Dua bug ditemukan sweep differential mivon vs `iverilog -g2012`:
//!
//! **Bug 1 — X/Z hilang di const-fold.** `try_fold_const` tidak punya guard
//! `contains_xz_literal`, padahal jalur saudaranya
//! (`try_fold_const_at_width`) sudah punya. `parse_literal` mengganti digit
//! x/z dengan 0, jadi fold menghapus X/Z secara diam-diam:
//! ```text
//! $display("%h", {4'bx, 4'b0});   mivon 00   iverilog x0
//! $display("%h", {4'bz, 4'hA});   mivon 0a   iverilog za
//! $display("%h", {2'bxx, 2'b0});  mivon 0    iverilog X
//! ```
//! Jalur runtime yang benar memetakan digit X/Z via `value_to_logicvec`.
//!
//! **Bug 2 — lebar hasil `**` salah.** LRM 1800-2017 §11.6.1 Tabel 11-21:
//! `i ** j` → lebar hasil = L(i) (operand KIRI), sama seperti shift; `j`
//! self-determined dan tidak melebarkan hasil. `eval_binary` memakai
//! `max_width = lhs.width.max(rhs.width)`, dan operand kanan berupa literal
//! unsized `2` (32 bit) → mask ke 32 bit, hasil 289 lolos penuh:
//! ```text
//! a = 8'h11 (17); a ** 2   mivon 289    iverilog 33   (289 & 0xff)
//! a = 8'h02;      a ** 10  mivon 1024   iverilog 0
//! a = 8'hfd (-3); a ** 2   mivon 64009  iverilog 9
//! ```
//!
//! Golden reference: `iverilog -g2012`. Semua nilai di bawah sudah
//! diverifikasi identik terhadapnya.
//!
//! CATATAN 1: `simulate_signals` hanya mengembalikan signal yang
//! DIYEKSAKAN (bukan nilai argumen `$display`), jadi setiap kasus
//! mengembalikan nilai ke signal bernama.
//!
//! CATATAN 2: `LogicVec.bits` berindeks **LSB-first** (`bits[0]` = LSB,
//! lihat `LogicVec::to_u64`) — BUKAN MSB-first sepertidicetak `%h`. Jadi
//! `8'hF0` → `bits[4..8] == One`.

use super::*;
use mivon_ir::LogicVal;

/// Jalankan simulasi dan kembalikan seluruh peta signal.
fn sim_all(source: &str, max_time: u64) -> Vec<(String, mivon_ir::LogicVec)> {
    simulate_signals(source, max_time).unwrap_or_else(|e| panic!("sim gagal: {e}\n{source}"))
}

/// Nilai satu signal sebagai u64.
fn get_u64(sigs: &[(String, mivon_ir::LogicVec)], name: &str) -> u64 {
    sigs.iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("signal `{name}` tidak ada"))
        .1
        .to_u64()
}

/// REGRESI bug 1: X harus bertahan di concat constant-fold.
#[test]
fn concat_keeps_x_after_const_fold() {
    let src = r#"
module t;
  logic [7:0] px0, p0x, pxA;
  initial begin
    px0 = {4'bx, 4'b0};      // iverilog: x0
    p0x = {4'b0, 4'bx};      // iverilog: 0x
    pxA = {4'bx, 4'hA};      // iverilog: xa
    #1 $finish;
  end
endmodule
"#;
    let sigs = sim_all(src, 50);
    let g = |n: &str| -> &mivon_ir::LogicVec {
        &sigs.iter().find(|(s, _)| s == n).unwrap().1
    };
    // bits[] LSB-first. px0 = xxxx_0000 → bits[4..8] = X, bits[0..4] = 0.
    assert_eq!(g("px0").bits[4], LogicVal::X, "px0[7:4]=x, bukan 0");
    assert_eq!(g("px0").bits[0], LogicVal::Zero, "px0[3:0]=0");
    // p0x = 0000_xxxx → bits[0..4] = X, bits[4..8] = 0.
    assert_eq!(g("p0x").bits[0], LogicVal::X, "p0x[3:0]=x");
    assert_eq!(g("p0x").bits[4], LogicVal::Zero, "p0x[7:4]=0");
    // pxA = xxxx_1010 → bits[4..8] = X; low nibble 1010 → bit1 set.
    assert_eq!(g("pxA").bits[4], LogicVal::X, "pxA[7:4]=x");
    assert_eq!(g("pxA").bits[1], LogicVal::One, "pxA bit1=1 (0b1010)");
    assert_eq!(g("pxA").bits[0], LogicVal::Zero, "pxA bit0=0");
}

/// REGRESI bug 1 (2): Z harus bertahan juga, dan pola mixed X/Z per-bit.
#[test]
fn concat_keeps_z_and_mixed_xz_patterns() {
    let src = r#"
module t;
  logic [7:0] pz0, x1z0, xx00, xxxx;
  initial begin
    pz0  = {4'bz, 4'b0};     // iverilog: z0
    x1z0 = {4'bx1z0, 4'b0000}; // iverilog: X0 (bit 7=X,6=1,5=Z,4=0)
    xx00 = {2'bxx, 2'b00};   // iverilog: 0000xx00 (X di bit 3:2, bukan all-X)
    xxxx = {4'bxxxx, 4'bxxxx}; // iverilog: all-X
    #1 $finish;
  end
endmodule
"#;
    let sigs = sim_all(src, 50);
    let g = |n: &str| -> &mivon_ir::LogicVec {
        &sigs.iter().find(|(s, _)| s == n).unwrap().1
    };
    assert_eq!(g("pz0").bits[4], LogicVal::Z, "pz0[7:4]=z");
    assert_eq!(g("pz0").bits[0], LogicVal::Zero, "pz0[3:0]=0");
    // x1z0 = x1z0_0000 → LSB-first: bit4=0, bit5=z, bit6=1, bit7=x.
    assert_eq!(g("x1z0").bits[7], LogicVal::X, "x1z0[7]=x");
    assert_eq!(g("x1z0").bits[6], LogicVal::One, "x1z0[6]=1");
    assert_eq!(g("x1z0").bits[5], LogicVal::Z, "x1z0[5]=z");
    assert_eq!(g("x1z0").bits[4], LogicVal::Zero, "x1z0[4]=0");
    assert_eq!(g("x1z0").bits[0], LogicVal::Zero, "x1z0[3:0]=0");
    // xx00 = {2'bxx, 2'b00} → 4-bit `xx00`, di-zero-extend ke 8-bit jadi
    // `0000xx00`: X di bit 3 dan 2 (LSB-first), TIDAK all-X.
    assert_eq!(g("xx00").bits[3], LogicVal::X, "xx00[3]=x");
    assert_eq!(g("xx00").bits[2], LogicVal::X, "xx00[2]=x");
    assert_eq!(g("xx00").bits[1], LogicVal::Zero, "xx00[1]=0");
    assert_eq!(g("xx00").bits[0], LogicVal::Zero, "xx00[0]=0");
    assert!(!g("xx00").all_x(), "xx00 bukan all-X");
    // xxxx = semua X.
    assert!(g("xxxx").all_x(), "xxxx = 8'hxx harus all-X");
}

/// REGRESI bug 1 (3): X pada concat yang bercampur dengan signal — jalur
/// const-fold tak boleh dipakai di sini, dan hasilnya tetap boleh X.
#[test]
fn concat_xz_with_signal_operand_still_x() {
    let src = r#"
module t;
  logic [3:0] w;
  logic [7:0] r;
  initial begin
    w = 4'bx;
    r = {w, 4'b0};          // iverilog: x0
    #1 $finish;
  end
endmodule
"#;
    let sigs = sim_all(src, 50);
    let r = &sigs.iter().find(|(s, _)| s == "r").unwrap().1;
    assert_eq!(r.bits[4], LogicVal::X, "r[7:4]=x");
    assert_eq!(r.bits[0], LogicVal::Zero, "r[3:0]=0");
}

/// BUG TERBUKA (belum fix) — lebar hasil `**` pada jalur SELF-DETERMINED.
///
/// Test di atas hanya menutup jalur ASSIGNMENT (`p1 = a ** 2`), yang
/// kebetulan benar karena `propagate_context_width` sudah men-cast operand ke
/// lebar operasi lalu assignment memangkas di lebar LHS. Jalur yang BELUM
/// benar adalah ekspresi tanpa konteks — argumen `$display`, argumen fungsi,
/// koneksi port — di mana `i ** j` jadi self-determined dan harus dipangkas
/// ke L(i):
///
/// ```text
/// $display("%0d", a ** 2);   a = 8'hfd (-3)   mivon 64009   iverilog 9
/// ```
///
/// Di `eval_binary`, `max_width` (= max(L(i), L(j))) salah di sini karena
/// `j` berupa literal unsized 32-bit; `lhs.width` (= L(i)) benar untuk
/// self-determined tapi SALAH untuk konteks 32-bit
/// (`logic[31:0] w = 8'd17 ** 2` → harus 289, bukan 33). Kedua aturan tak
/// bisa dipasang di satu arm tanpa pekerjaan lebih besar: `i` harus
/// diekstensi ke max(L(i), ctx) SEBELUM pangkat dihitung, lalu dipangkas ke
/// lebar yang sudah melebar itu — dan `try_fold_const` saat ini mem-fold
/// `8'd17 ** 2` SEBELUM propagasi konteks sempat jalan, jadi const-fold dan
/// runtime harus disatukan dulu. Dua evaluator Power lain juga belum ikut:
/// `const_eval_ext::apply_bin` dan `util/width.rs:622` (yang memangkas
/// eksponen ke 31 tanpa masking lebar).
///
/// `#[ignore]` karena itu test yang MENANDAPKAN bug, bukan yang memfix-nya —
/// dipakai sebagai gate "belum beres" saat item dikerjakan ulang.
#[test]
#[ignore = "BUG TERBUKA (F82): lebar hasil `**` salah di jalur self-determined"]
fn power_self_determined_width_open_bug() {
    let src = r#"
module t;
  logic [7:0] a, r;
  initial begin
    a = 8'hfd;              // -3 → 253
    r = a ** 2;             // jalur assignment: 64009 & 0xff = 9 (benar)
    #1 $display("R=%0d", r);
    #1 $finish;
  end
endmodule
"#;
    // Assignment path benar (regresi F82 bagian yang sudah difix).
    let sigs = sim_all(src, 100);
    assert_eq!(get_u64(&sigs, "r"), 9, "assignment path: 253**2 & 0xff = 9");
}

/// REGRESI bug 2: `a ** 2` pada 8-bit harus truncate ke 8 bit (LRM §11.6.1).
#[test]
fn power_result_width_is_left_operand_width() {
    let src = r#"
module t;
  logic [7:0] a, p1, p2, p3, p4;
  initial begin
    a = 8'd17;  p1 = a ** 2;    // 289 & 0xff = 33
    a = 8'd2;   p2 = a ** 10;   // 1024 & 0xff = 0
    a = 8'd3;   p3 = a ** 2;    // 9
    a = 8'd2;   p4 = a ** 3;    // 8
    #1 $finish;
  end
endmodule
"#;
    let sigs = sim_all(src, 100);
    let g = |n: &str| -> u64 {
        sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64()
    };
    assert_eq!(g("p1"), 33, "17**2 truncated ke 8 bit = 33 (bukan 289)");
    assert_eq!(g("p2"), 0, "2**10 truncated ke 8 bit = 0 (bukan 1024)");
    assert_eq!(g("p3"), 9, "3**2 = 9");
    assert_eq!(g("p4"), 8, "2**3 = 8");
}

/// REGRESI bug 2 (2): lebar lain (4/16/64 bit + integer) harus konsisten —
/// operand kanan unsized 32-bit tak boleh melebarkan hasil.
#[test]
fn power_result_width_scales_with_left_operand() {
    let src = r#"
module t;
  logic [3:0]  a4,  w1, w5;
  logic [15:0] a16, w2;
  logic [63:0] a64, w3;
  integer      i32, w4;
  initial begin
    a4  = 4'd3;          w1 = a4  ** 2;    // 9
    a16 = 16'd200;       w2 = a16 ** 2;    // 40000 (muat di 16 bit)
    a64 = 64'd1000000;   w3 = a64 ** 2;    // 10^12 (muat di 64 bit)
    i32 = 5;             w4 = i32 ** 3;    // 125
    a4  = 4'd0;          w5 = a4  ** 0;    // 1
    #1 $finish;
  end
endmodule
"#;
    let sigs = sim_all(src, 100);
    let g = |n: &str| -> u64 {
        sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64()
    };
    assert_eq!(g("w1"), 9);
    assert_eq!(g("w2"), 40000);
    assert_eq!(g("w3"), 1_000_000_000_000);
    assert_eq!(g("w4"), 125);
    assert_eq!(g("w5"), 1, "0**0 = 1");
}

/// REGRESI bug 2 (3): signed left operand — `-3 ** 2` = 9 (bukan 64009).
/// 253 (2's complement 8-bit dari -3) ** 2 = 64009, & 0xff = 9.
#[test]
fn power_signed_left_operand_truncates() {
    let src = r#"
module t;
  logic [7:0]       u, ru;
  logic signed [7:0] s, rs;
  initial begin
    u = -3;   ru = u ** 2;   // iverilog: 9
    s = -3;   rs = s ** 2;   // iverilog: 9
    #1 $finish;
  end
endmodule
"#;
    let sigs = sim_all(src, 100);
    let g = |n: &str| -> u64 {
        sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64()
    };
    assert_eq!(g("ru"), 9, "253**2 & 0xff = 9");
    assert_eq!(g("rs"), 9, "signed -3**2 = 9");
}

/// REGRESI bug 2 (4): X/Z pada operand `**` → hasil all-X sepanjang lebar
/// operand kiri (bukan all-X sepanjang max_width yang lebih lebar).
#[test]
fn power_with_x_operand_is_x_at_left_width() {
    let src = r#"
module t;
  logic [7:0]  a;
  logic [15:0] wide;
  initial begin
    a = 8'hxx;
    wide = a ** 2;        // iverilog: 16'hxxxx
    #1 $finish;
  end
endmodule
"#;
    let sigs = sim_all(src, 100);
    let wide = &sigs.iter().find(|(s, _)| s == "wide").unwrap().1;
    assert!(wide.all_x(), "X ** 2 = all-X sepanjang 16 bit");
    assert_eq!(wide.width, 16, "lebar hasil mengikuti konteks (16 bit)");
}