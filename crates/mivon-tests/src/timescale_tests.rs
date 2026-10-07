//! REGRESI F84 — `timescale` per-module (LRM 1800-2017 §19.8).
//!
//! Bug: `timescale` diperlakukan sebagai properti GLOBAL design (nilai
//! directive terakhir), padahal LRM §19.8 menetapkan Directive berlaku untuk
//! SEMUA module yang SETELAH directive dan berubah setiap ada directive baru.
//! Akibatnya `#5` di module `1us/1ns` diperlakukan 5 tick basis `1ns`
//! (atau sebaliknya) — hasil simulasi menyimpang diam-diam, tanpa error.
//!
//! Bukti (golden `iverilog -g2012`, semua nilai sudah diverifikasi):
//!
//! ```text
//! `timescale 1ns/1ps
//! module m_fast; initial #5 o = 5; endmodule
//! `timescale 1us/1ns
//! module tb;     initial #4 $display(a); endmodule
//!
//! pada t=4us:  iverilog a = 5   (5ns sudah lewat, 4us belum? tidak — 5ns < 4us)
//!              mivon SEBELUM fix a = 0
//! ```
//!
//! Yang diuji: delay di-IR tiap module sudah diskalakan ke satuan FINEST
//! design (satu tick = 1 unit basis), sehingga `#5` `1us` = 5000 tick dan
//! `#5` `1ns` = 5 tick dalam design yang sama.
//!
//! Design uji ditulis inline (bukan file di `/tmp`) supaya test tidak
//! bergantung file eksternal yang bisa terhapus.

use mivon_api::compile_str;
use mivon_ir::{IrModule, IrStmt, Process};

/// Kumpulkan semua `IrStmt::Delay` dalam satu module.
///
/// CATATAN: setelah flatten, `IrDesign::top` memuat proses SEMUA modul
/// descendant (mivon meng-inline process ke top), jadi daftar di `top`
/// bercampur. Test karena itu memeriksa DAFTAR YANG MENGANDUNG nilai yang
/// diharapkan, bukan urutan persis — atau memakai design tanpa instance.
fn delays_of(m: &IrModule, out: &mut Vec<u64>) {
    for p in &m.processes {
        match p {
            Process::Initial { body, .. } => walk_stmts(body, out),
            Process::AlwaysWithDelay { delay, .. } => out.push(*delay),
            _ => {}
        }
    }
}

fn walk_stmts(stmts: &[IrStmt], out: &mut Vec<u64>) {
    for s in stmts {
        match s {
            IrStmt::Delay { delay, body } => {
                out.push(*delay);
                walk_stmts(body, out);
            }
            IrStmt::Block { stmts } => walk_stmts(stmts, out),
            _ => {}
        }
    }
}

/// Source dua-satuan: modul `1ns` dan modul `1us` dalam file yang sama.
const SRC_MIXED: &str = r#"
`timescale 1ns/1ps
module m_fast (output logic [7:0] o);
  initial begin o = 8'd0; #5 o = 8'd5; end
endmodule

`timescale 1us/1ns
module tb_mixed;
  logic [7:0] a;
  m_fast uf (.o(a));
  initial begin
    #4 $display("R t=%0d fast=%0d", $time, a);
    $finish;
  end
endmodule
"#;

/// Delay module `1us` harus 1000× delay module `1ns` pada basis yang sama.
#[test]
fn timescale_scales_delay_per_module() {
    let design = compile_str(SRC_MIXED).expect("compile");

    let mut fast = Vec::new();
    delays_of(
        design
            .modules
            .values()
            .find(|m| m.name.as_str() == "m_fast")
            .expect("module m_fast"),
        &mut fast,
    );
    let mut tb = Vec::new();
    delays_of(&design.top, &mut tb);

    assert_eq!(fast, vec![5], "delay module 1ns tetap 5 tick");
    assert!(
        tb.contains(&4000),
        "delay module 1us (#4) = 4000 tick basis 1ns harus ada di top \
         (dapat: {tb:?}) — SEBELUM fix nilainya 4, sehingga display terjadi \
         sebelum m_fast selesai"
    );
    assert!(
        !tb.contains(&4),
        "delay 4 TIDAK boleh muncul sebagai nilai `#4` module 1us \
         (dapat: {tb:?})"
    );
}

/// Basis global = satuan TERHALUS, bukan directive terakhir.
///
/// `1us` ditulis TERAKHIR di file; bila `design.timescale` ikut mengambilnya,
/// modul `1ns` akan ikut diskalakan 1000× dan `#5`-nya jadi 5000 tick.
#[test]
fn design_timescale_is_finest_not_last() {
    let design = compile_str(SRC_MIXED).expect("compile");
    assert_eq!(
        design.timescale.as_ref().map(|(u, _)| u.as_str()),
        Some("1ns"),
        "design.timescale = satuan TERHALUS (1ns), bukan directive terakhir (1us)"
    );
}

/// Design satu satuan tidak boleh berubah sama sekali (guard regresi).
#[test]
fn single_timescale_design_unchanged() {
    let src = r#"
`timescale 1ns/1ps
module tb_single;
  initial begin
    #5 $finish;
  end
endmodule
"#;
    let design = compile_str(src).expect("compile");
    let mut d = Vec::new();
    delays_of(&design.top, &mut d);
    assert_eq!(d, vec![5], "satu directive → faktor 1, delay tak berubah");
}

/// Tanpa directive sama sekali: default LRM, faktor 1.
#[test]
fn no_timescale_directive_keeps_raw_delay() {
    let src = r#"
module tb_none;
  initial begin
    #7 $finish;
  end
endmodule
"#;
    let design = compile_str(src).expect("compile");
    let mut d = Vec::new();
    delays_of(&design.top, &mut d);
    assert_eq!(d, vec![7], "tanpa directive delay apa adanya");
}

/// `always #N` punya jalur elaborasi TERPISAJ (`Process::AlwaysWithDelay`),
/// jadi diskalannya harus ikut diuji — bukan hanya `initial #N`.
#[test]
fn always_with_delay_also_scaled() {
    // `always #N` punya jalur elaborasi TERPISAJ (`Process::AlwaysWithDelay`),
    // jadi skalanya harus ikut diuji — bukan hanya `initial #N`. Dua submodule
    // dengan satuan berbeda, diinstansiasi dari satu top supaya tak ada
    // ambiguitas top.
    let src = r#"
`timescale 1ns/1ps
module gen_fast (output logic clk);
  initial clk = 0;
  always #5 clk = ~clk;
endmodule

`timescale 1us/1ns
module gen_slow (output logic clk);
  initial clk = 0;
  always #5 clk = ~clk;
endmodule

`timescale 1ns/1ps
module tb;
  logic cf, cs;
  gen_fast uf (.clk(cf));
  gen_slow us (.clk(cs));
  initial begin
    repeat (4) @(posedge cf);
    repeat (4) @(posedge cs);
    $finish;
  end
endmodule
"#;
    let design = compile_str(src).expect("compile");
    let fast = design
        .modules
        .values()
        .find(|m| m.name.as_str() == "gen_fast")
        .expect("module gen_fast");
    let slow = design
        .modules
        .values()
        .find(|m| m.name.as_str() == "gen_slow")
        .expect("module gen_slow");
    let mut fast_d = Vec::new();
    delays_of(fast, &mut fast_d);
    let mut slow_d = Vec::new();
    delays_of(slow, &mut slow_d);

    assert!(
        fast_d.contains(&5),
        "`always #5` module 1ns = 5 tick; dapat {fast_d:?}"
    );
    assert!(
        slow_d.contains(&5000),
        "`always #5` module 1us = 5000 tick; dapat {slow_d:?} — jalur \
         AlwaysWithDelay harus ikut diskalakan"
    );
}

/// `Interface` dielaborasi sebagai `Module` SINTETIK yang tak ada di
/// `design.modules` — tanpa entri peta skala, delay interface `1us` diam-diam
/// memakai faktor 1.
#[test]
fn interface_timescale_scales_too() {
    let src = r#"
`timescale 1us/1ns
interface m_slow_if (input logic clk);
  initial #5 $display("IF");
endinterface

`timescale 1ns/1ps
module tb;
  logic clk = 0;
  m_slow_if u_if (.clk(clk));
  initial begin
    #10 $finish;
  end
endmodule
"#;
    let design = compile_str(src).expect("compile");
    let mut v = Vec::new();
    delays_of(&design.top, &mut v);
    assert!(
        v.contains(&5000),
        "delay di interface `1us` = 5000 tick; dapat {v:?} — interface \
         yang dielaborasi sebagai module sintetik harus punya entri peta skala"
    );
}