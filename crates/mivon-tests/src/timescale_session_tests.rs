//! REGRESI F84 (CLI) — `timescale` per-module harus berlaku di jalur
//! CompileSession (dipakai CLI `mivon <file>`), bukan hanya jalur API.
//!
//! Test API (`mivon-tests/src/timescale_tests.rs`) membuktikan elaborasi
//! benar; test ini menutup jalur yang berbeda: CompileSession meng-inline
//! `timescale` global sendiri lewat `extend_design_move`, dan bisa salah
//! memilih satuan bila beberapa directive ada di file berbeda.

use mivon_compiler::frontend::compile_session::{CompileSession, SessionConfig};
use mivon_ir::{IrModule, IrStmt, Process};

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

/// Nama direktori unik per test (bisMDBIRestore antar test mengotori basis).
fn dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("mivon_f84_{tag}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn elaborate(src: &str, tag: &str) -> mivon_ir::IrDesign {
    let d = dir(tag);
    let f = d.join("design.sv");
    std::fs::write(&f, src).unwrap();
    let mut cfg = SessionConfig::default();
    cfg.sources = vec![f];
    let mut session = CompileSession::new(cfg);
    let (_design, ir, _n) = session
        .compile_and_elaborate(None)
        .expect("CompileSession compile_and_elaborate");
    ir
}

/// Dua directive dalam SATU file: `1ns` lalu `1us`.
#[test]
fn session_two_timescales_in_one_file() {
    let src = r#"
`timescale 1ns/1ps
module m_fast (output logic [7:0] o);
  initial begin o = 8'd0; #5 o = 8'd5; end
endmodule

`timescale 1us/1ns
module tb;
  logic [7:0] a;
  m_fast uf (.o(a));
  initial begin
    #4 $display("R %0d", a);
    $finish;
  end
endmodule
"#;
    let ir = elaborate(src, "one_file");
    let mut v = Vec::new();
    delays_of(&ir.top, &mut v);
    assert!(
        v.contains(&4000),
        "delay module 1us (#4) = 4000 tick basis 1ns; dapat {v:?}"
    );
    assert!(
        v.contains(&5),
        "delay module 1ns (#5) = 5 tick; dapat {v:?}"
    );
}

/// Directive berbeda di DUA FILE terpisah — `extend_design_move` menyatukan
/// `design.timescale` dan harus memilih satuan TERHALUS, bukan yang terakhir.
#[test]
fn session_timescale_across_two_files() {
    let d = dir("two_files");
    // File A: `1us` (lebih kasar) + top yang meng-instansiasi modul file B.
    let fa = d.join("a_slow.sv");
    std::fs::write(
        &fa,
        "`timescale 1us/1ns\n\
         module tb; logic [7:0] a;\n\
           m_fast uf (.o(a));\n\
           initial begin #4 $display(\"R %0d\", a); $finish; end\n\
         endmodule\n",
    )
    .unwrap();
    // File B: `1ns` (lebih halus) — diurutkan SETELAH a_slow.
    let fb = d.join("b_fast.sv");
    std::fs::write(
        &fb,
        "`timescale 1ns/1ps\nmodule m_fast (output logic [7:0] o);\n\
         initial begin o = 8'd0; #5 o = 8'd5; end\nendmodule\n",
    )
    .unwrap();

    let mut cfg = SessionConfig::default();
    cfg.sources = vec![fa, fb];
    let mut session = CompileSession::new(cfg);
    let (_design, ir, _n) = session
        .compile_and_elaborate(None)
        .expect("CompileSession dua file");
    let mut v = Vec::new();
    delays_of(&ir.top, &mut v);
    assert!(
        v.contains(&4000),
        "delay module 1us tetap 4000 tick meski file `1ns` menyusul; dapat {v:?}"
    );
    assert!(v.contains(&5), "delay module 1ns tetap 5 tick; dapat {v:?}");
}