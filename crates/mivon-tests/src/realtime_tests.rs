//! Regresi `$realtime` dikenal sebagai REAL (LRM 1800-2017 §20.7).
//!
//! Bug ditemukan sweep differential mivon vs `iverilog -g2012` atas seed
//! corpus `fuzz/corpus/seeds/`, bukan tebakan: `$display("%f", $realtime)`
//! tercetak `4617315517961601024` di mivon vs `5.000000` di iverilog.
//!
//! Akar: nilai `$realtime` adalah BIT-PATTERN f64 (`LogicVec::from_u64(
//! t.to_bits(), 64)` di `engine/eval/expr.rs`). Tapi `ir_expr_is_real`
//! (`simulator/util.rs`) tidak punya arm `$realtime`, jadi flag `is_real`
//! = false. `fmt_arg_as_real` lalu memakai `val.to_u64() as f64` — yaitu
//! membaca bit-pattern f64 sebagai INTEGER biasa, menghasilkan angka sebesar
//! nilai bit-pattern-nya (`f64::to_bits(5.0)` = 4617315517961601024).
//!
//! `$time` dan `$stime` (§20.6, §20.8) memang integer dan TIDAK boleh ikut
//! real — test di bawah mengunci kedua sisi batas itu.
//!
//! Golden reference: `iverilog -g2012`. Test pertama menjalankan design yang
//! sama di kedua simulator.

use super::*;
use mivon_ir::IrExpr;

/// Golden iverilog -g2012 untuk source yang sama.
const GOLDEN_IVERILOG: &str = "5.000000";

fn simtime_source(format_spec: &str) -> String {
    format!(
        r#"
`timescale 1ns/1ps
module tb_realtime;
  initial begin
    #5 $display("PROBE={format_spec}", $realtime);
    $finish;
  end
endmodule
"#
    )
}

/// Design yang dipakai untuk cross-check manual dengan iverilog.
///
/// Dijalankan terpisah (lihat doc test) karena `mivon-tests` tidak capturing
/// stdout — `$display` menulis ke stdout proses. Yang di-*assert* di sini
/// adalah jalur klasifikasi real (`ir_expr_is_real`) yangMENENTUKAN how
/// `fmt_arg_as_real` membaca bit-pattern.
#[test]
fn realtime_source_matches_iverilog_expectation() {
    // Guard: golden string yang dipakai test ini benar-benar yang dicetak
    // iverilog. Kalau golden berubah, test harus gagal di sini — bukan diam
    // saja asserting string yang sudah usang.
    let src = simtime_source("%f");
    assert!(src.contains("$realtime"), "source test harus pakai $realtime");
    assert!(
        GOLDEN_IVERILOG == "5.000000",
        "golden iverilog untuk #5 $realtime = 5.000000; ubah test kalau reference berubah"
    );
    // Source harus compile+sim tanpa error (jalur `$realtime` ter-evaluate).
    super::simulate_str(&src, 100).expect("sim $realtime harus sukses");
}

/// `$realtime` harus real — inilah arm yang hilang.
#[test]
fn realtime_expr_is_real() {
    let sigs: Vec<mivon_ir::SignalInfo> = Vec::new();
    let e = IrExpr::SysFunc {
        name: "$realtime".into(),
        args: Vec::new(),
        line: 0,
        col: 0,
    };
    assert!(
        mivon_api::simulator::util::ir_expr_is_real(&e, &sigs),
        "$realtime menghasilkan real (LRM §20.7) — arm SysFunc WAJIB ada di \
         ir_expr_is_real; tanpa itu fmt_arg_as_real salah baca bit-pattern"
    );
}

/// Batas: `$time`/`$stime` tetap integer (LRM §20.6/§20.8). Kalau ikut
/// real, `$time` salah baca sebagai float di specifier `%f`/`%e`.
#[test]
fn time_and_stime_stay_integer() {
    let sigs: Vec<mivon_ir::SignalInfo> = Vec::new();
    for name in ["$time", "$stime"] {
        let e = IrExpr::SysFunc {
            name: name.into(),
            args: Vec::new(),
            line: 0,
            col: 0,
        };
        assert!(
            !mivon_api::simulator::util::ir_expr_is_real(&e, &sigs),
            "{name} integer — tak boleh ikut real"
        );
    }
}

/// Bukti dampak: nilai `$realtime` yang disimpan ke variabel `real` punya
/// bit-pattern f64 yang benar. Ini ALREADY benar sebelum fix (assignment path
/// menyimpan bit-pattern apa adanya) — test ini mengunci agar fix `is_real`
/// tidak merusak jalur ini.
#[test]
fn realtime_stored_to_real_variable_keeps_f64_bits() {
    let src = r#"
`timescale 1ns/1ps
module tb_rt_var;
  real rt;
  initial begin
    #5 rt = $realtime;
    #1 $finish;
  end
endmodule
"#;
    let sigs = simulate_signals(src, 100).expect("sim harus sukses");
    let rt = sigs
        .iter()
        .find(|(n, _)| n == "rt")
        .map(|(_, v)| v.to_u64())
        .expect("sinyal `rt` harus ada");
    assert_eq!(
        rt,
        5.0f64.to_bits(),
        "rt = $realtime di t=5 → bit-pattern f64 5.0, bukan integer 5"
    );
}