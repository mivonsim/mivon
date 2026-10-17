//! Regresi parser untuk konstruk yang gagal saat compile CVA6
//! (`mivon --filelist cva6/cva6.f`). Ketiga kasus di bawah dulunya
//! memunculkan `error[E1002]` (parse error) yang membatalkan simulasi.
//!
//! Tiap bug diuji secara terisolasi via parse-only (`Parser` mentah) agar
//! test tidak bergantung pada dukungan elaborasi/simulator untuk konstruk
//! bersangkutan — fokus murni pada kebenaran parse.

use super::*;
use mivon_parser::lexer::Token;

fn lex_and_parse(src: &str) -> (mivon_ast::Design, Vec<mivon_core::diagnostics::Diagnostic>) {
    let mut lexer = Lexer::new(src);
    let mut tokens = Vec::new();
    loop {
        let (tok, line, col) = lexer.next_token();
        if tok == Token::Eof {
            break;
        }
        tokens.push((tok, line, col));
    }
    let file_line_map = lexer.file_line_map.clone();
    let mut parser = Parser::new(tokens, "<test>")
        .with_source_lines(src)
        .with_file_line_map(file_line_map);
    let design = parser
        .parse_design()
        .expect("parse (recovery) harus sukses");
    (design, parser.errors)
}

fn assert_no_parse_errors(src: &str) {
    let (_design, errors) = lex_and_parse(src);
    let errs: Vec<_> = errors.iter().filter(|d| d.is_error()).collect();
    assert!(
        errs.is_empty(),
        "parse errors tidak diharapkan:\n{:?}",
        errs
    );
}

/// Bug-1 (cva6): dimensi array range `[msb:lsb]` tidak didukung —
/// `expected RBrack, found Colon` pada `type name [1:0];` dan multi-dimensi
/// `type name [NR-1:0][M-1:0]`. Sekarang di-parse sebagai unpacked range.
#[test]
fn test_cva6_array_range_dimension() {
    let src = r#"
module top;
  typedef logic [7:0] mem_resp_t;
  mem_resp_t mem_resp_read_arb [1:0];
  logic bht_q [3:0][2:0];
  logic [7:0] req [NumPorts-2:0];
  initial begin
    mem_resp_read_arb[0] = 8'h11;
    mem_resp_read_arb[1] = 8'h22;
    bht_q[0][0] = 1'b1;
    req[0] = 8'h0;
  end
  initial #10 $finish;
endmodule
"#;
    assert_no_parse_errors(src);
}

/// Bug-2 (cva6): keyword `none` bentrok dengan identifier — gagal di
/// deklarasi `logic none, load, ...` dan member access `.none`. Sekarang
/// `none`/`some` diterima sebagai identifier di posisi identifier (nama
/// deklarasi + field struct), sesuai pemakaian aktual di cva6
/// (`fus_busy[1].none`, `logic none, load, store, ...`).
#[test]
fn test_cva6_keyword_none_as_identifier() {
    let src = r#"
module top;
  typedef struct packed { logic v; } fus_busy_t;
  fus_busy_t fus_busy [1:0];
  logic none, load, store, alu, some;
  initial begin
    fus_busy[1].none = 1'b1;
    fus_busy[0].some = 1'b0;
    load = 1'b1;
  end
  initial #1 $finish;
endmodule
"#;
    assert_no_parse_errors(src);
}

/// Bug-3 (cva6): parameter override bertipe `#(.CVA6Cfg(CVA6Cfg))` antara
/// nama tipe dan nama variabel pada deklarasi class/struct ter-parameter —
/// `expected expression, found Dot`. Sekarang `#(...)` dilewati (skip) sebelum
/// nama variabel di-parse.
#[test]
fn test_cva6_typed_param_override() {
    let src = r#"
module top;
  typedef logic [7:0] instr_trace_item;
  function void f;
    automatic instr_trace_item #(.CVA6Cfg(8)) iti = 8'h5;
    automatic instr_trace_item #(.CVA6Cfg(8), .bp_resolve_t(8)) iti2 = 8'ha;
  endfunction
  initial #1 $finish;
endmodule
"#;
    assert_no_parse_errors(src);
}

/// Bug-4 (cva6): nama member struct/union ber-koma berbagi tipe SAMA
/// (`logic none, load, store, ...;`) di-body struct/union tidak didukung —
/// gagal "expected type in struct/union member". Sekarang tiap nama
/// didaftarkan sebagai member tersendiri (LRM 1800 §7.2).
#[test]
fn test_cva6_struct_comma_members() {
    let src = r#"
module top;
  typedef struct packed {
    logic none, load, store, alu, alu2, ctrl_flow, mult, csr, fpu, fpu_vec, cvxif, accel, aes;
    logic [31:0] pc, npc;
  } fu_t;
  initial #1 $finish;
endmodule
"#;
    assert_no_parse_errors(src);
}

/// Bug-5 (cva6): `assign a=b, c.d=e, ...;` multi-assignment koma (LRM §10.3)
/// — assignment ke-2+ jatuh ke E1005 "skipping unknown construct" sesat.
/// Sekarang semua pasangan di-parse (cva6 hpdcache_to_l15, mem_to_axi_*).
#[test]
fn test_cva6_multi_assign_comma() {
    let src = r#"
module top;
  logic a, b, c;
  typedef struct packed { logic [7:0] x; logic [7:0] y; } req_t;
  req_t r;
  assign a = b, r.x = c, r.y = a;
  initial #1 $finish;
endmodule
"#;
    let (design, errors) = lex_and_parse(src);
    assert_no_parse_errors(src);
    // 3 assign harus jadi 3 ModuleItem::Assign (1 langsung + 2 antrean).
    let assigns = design.modules[0]
        .items
        .iter()
        .filter(|i| matches!(i, mivon_ast::ModuleItem::Assign(_)))
        .count();
    assert_eq!(assigns, 3, "multi-assign koma harus hasilkan 3 assign");
    let sesat: Vec<_> = errors
        .iter()
        .filter(|d| format!("{}", d).contains("skipping unknown construct"))
        .collect();
    assert!(
        sesat.is_empty(),
        "diag sesat E1005 tidak boleh muncul: {:?}",
        sesat
    );
}

/// Bug-6 (cva6): struct anonim dgn packed-dim PARAM setelah body
/// (`} [NUM_RULES-1:0] rules [NUM_SLAVE]`, `} [TLB_ENTRIES-1:0] tags_q`)
/// jatuh ke E1005 sesat "skipping unknown construct: 'NUM_RULES'".
/// Sekarang packed-dim di-parse (LRM §7.2).
#[test]
fn test_cva6_struct_packed_param_dim() {
    let src = r#"
module top #(parameter int NUM_RULES = 4, parameter int NUM_SLAVE = 2) ();
  struct packed {
    logic enabled;
    logic [7:0] mask;
  } [NUM_RULES-1:0] rules [NUM_SLAVE];
  struct packed {
    logic [3:0] vpn;
  } [2-1:0] tags_q, tags_n;
  initial #1 $finish;
endmodule
"#;
    let (_design, errors) = lex_and_parse(src);
    assert_no_parse_errors(src);
    let sesat: Vec<_> = errors
        .iter()
        .filter(|d| format!("{}", d).contains("skipping unknown construct"))
        .collect();
    assert!(
        sesat.is_empty(),
        "diag sesat E1005 tidak boleh muncul: {:?}",
        sesat
    );
}
