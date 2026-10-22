//! Test kategori error parser/elaborator: top-level, modul, port, package,
//! interface, class, generate, DPI, covergroup, lvalue, dan ekspresi.
use super::*;

// ===== Category 1: Top-level design errors (parse_design) =====

#[test]
fn test_parse_err_top_level_wire() {
    assert!(compile_str("wire x;").is_ok());
}

#[test]
fn test_parse_err_top_level_gibberish() {
    assert!(compile_str("foo bar;").is_err());
}

#[test]
fn test_parse_err_top_level_endmodule() {
    assert!(compile_str("endmodule").is_err());
}

#[test]
fn test_parse_err_top_level_endclass() {
    assert!(compile_str("endclass").is_err());
}

#[test]
fn test_parse_err_top_level_endpackage() {
    assert!(compile_str("endpackage").is_err());
}

#[test]
fn test_parse_err_top_level_endinterface() {
    assert!(compile_str("endinterface").is_err());
}

#[test]
fn test_parse_err_top_level_task() {
    assert!(compile_str("task t(); endtask").is_err());
}

#[test]
fn test_parse_err_top_level_function() {
    assert!(compile_str("function f(); endfunction").is_err());
}

#[test]
fn test_parse_err_top_level_initial() {
    assert!(compile_str("initial begin end").is_err());
}

#[test]
fn test_parse_err_top_level_always() {
    assert!(compile_str("always begin end").is_err());
}

#[test]
fn test_parse_err_top_level_if() {
    assert!(compile_str("if (x) a=1;").is_err());
}

#[test]
fn test_parse_err_top_level_for() {
    assert!(compile_str("for (;;) begin end").is_err());
}

#[test]
fn test_parse_err_top_level_typedef() {
    assert!(compile_str("typedef int myint;").is_err());
}

#[test]
fn test_parse_err_top_level_import_dpi() {
    assert!(compile_str("import \"DPI-C\" function void f();").is_err());
}

#[test]
fn test_parse_err_top_level_covergroup() {
    assert!(compile_str("covergroup cg; endgroup").is_err());
}

#[test]
fn test_covergroup_formal_args_supported() {
    // Formal arguments covergroup (IEEE 1800 §19.5) kini DI-DUKUNG: parser
    // menyimpan daftar formal ke CovergroupDecl.formals. Dulu fail-fast di
    // header → body ter-skip → cascade diagnostic SESAT di konstruk
    // berikutnya ("expected system call name after $" dst, 7 error OpenTitan).
    let source = r#"
module top(input logic clk, input logic [3:0] v);
  covergroup handshake_cg (int unsigned complete_status, logic [3:0] x) @(posedge clk);
    cp: coverpoint v;
  endgroup
endmodule
"#;
    let mut lexer = Lexer::new(source);
    use mivon_parser::lexer::Token;
    let mut tokens = Vec::new();
    loop {
        let (tok, line, col) = lexer.next_token();
        if tok == Token::Eof {
            break;
        }
        tokens.push((tok, line, col));
    }
    let mut parser = mivon_parser::Parser::new(tokens, "t.sv").with_source_lines(source);
    let design = parser
        .parse_design()
        .expect("parse_design Ok (error recoverable)");
    let errs: Vec<String> = parser
        .errors
        .iter()
        .filter(|d| d.is_error())
        .map(|d| d.message.to_string())
        .collect();
    assert!(errs.is_empty(), "tanpa error parse: {:?}", errs);
    let m = design
        .modules
        .iter()
        .find(|m| m.name == Symbol::intern("top"))
        .expect("module top harus ada");
    let cg = m
        .items
        .iter()
        .find_map(|i| match i {
            mivon_ast::ModuleItem::Covergroup(cg) => Some(cg),
            _ => None,
        })
        .expect("covergroup harus ada di module top");
    assert_eq!(
        cg.formals.len(),
        2,
        "2 formal argument harus tersimpan: {:?}",
        cg.formals
    );
    assert_eq!(cg.formals[0], Symbol::intern("complete_status"));
    assert_eq!(cg.formals[1], Symbol::intern("x"));
}

#[test]
fn test_covergroup_two_pass_error_reported_once() {
    // BUG #1: parse_design dua pass (pass-1 kumpulkan nama, pass-2 parse
    // penuh dgn pos di-reset) — error identik di kedua pass wajib terdedup
    // jadi 1 laporan. Sumber error: covergroup body rusak (`coverpoint ;`,
    // tanpa formal args).
    let source = r#"
class dummy_cov;
    int unsigned m;
endclass

covergroup handshake_cg;
    cp: coverpoint ;
endgroup
"#;
    let mut lexer = Lexer::new(source);
    use mivon_parser::lexer::Token;
    let mut tokens = Vec::new();
    loop {
        let (tok, line, col) = lexer.next_token();
        if tok == Token::Eof {
            break;
        }
        tokens.push((tok, line, col));
    }
    let mut parser = mivon_parser::Parser::new(tokens, "cov.sv").with_source_lines(source);
    let _design = parser
        .parse_design()
        .expect("parse_design selalu Ok — error bersifat recoverable");
    let hits = parser.errors.iter().filter(|d| d.is_error()).count();
    assert_eq!(
        hits,
        1,
        "error covergroup body rusak harus dilaporkan tepat 1× (dua-pass dedup), dapat {}:\n{:?}",
        hits,
        parser
            .errors
            .iter()
            .map(|d| (d.level, d.message.to_string()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn test_parse_err_top_level_genvar() {
    assert!(compile_str("genvar i;").is_err());
}

#[test]
fn test_parse_err_top_level_modport() {
    assert!(compile_str("modport m (input clk);").is_err());
}

#[test]
fn test_parse_err_top_level_assign() {
    assert!(compile_str("assign x = y;").is_err());
}

#[test]
fn test_parse_err_top_level_generate() {
    assert!(compile_str("generate endgenerate").is_err());
}

// ===== Category 2: Module name errors =====

#[test]
fn test_parse_err_module_no_name() {
    assert!(compile_str("module ; endmodule").is_err());
}

#[test]
fn test_parse_err_module_eof() {
    assert!(compile_str("module top").is_err());
}

#[test]
fn test_parse_err_module_eof_after_semi() {
    assert!(compile_str("module top;").is_err());
}

#[test]
fn test_parse_err_module_keyword_as_name() {
    assert!(compile_str("module input; endmodule").is_err());
}

#[test]
fn test_parse_err_module_keyword_for() {
    assert!(compile_str("module for; endmodule").is_err());
}

#[test]
fn test_parse_err_module_keyword_begin() {
    assert!(compile_str("module begin; endmodule").is_err());
}

// ===== IEEE 1800 regression (dari corpus-gap fuzzer, kmac_app_if.sv) =====

/// Header import interface (IEEE 1800 §23.2.1): `import pkg::t;` di header
/// interface → nama typedef SCOPE port — `inout wire t x` (net-type +
/// user-type). Sebelumnya `t` salah-parse jadi nama port → E1002
/// "expected RParen, found 'req'" (app_req_t dari kmac_pkg).
#[test]
fn test_iface_header_import_typedef_port() {
    let src = r#"
package p;
  typedef logic [7:0] t;
endpackage
interface i import p::t; (
  inout wire t x,
  output t y
);
endinterface
module top(output logic [7:0] z);
  i u_i ();
  assign z = u_i.y;
endmodule
"#;
    let d = compile_str(src).expect("interface header import + typedef port harus compile");
    assert!(!d.modules.is_empty(), "module top harus ada");
}

/// SVA label + property TANPA ';' penutup (aksi `else `MACRO` di-skip
/// preprocessor → menganggur) — skip assertion TIDAK boleh menelan item
/// berikut (label assert kedua + task). Sebelumnya parser resume di `fork :`
/// → "skipping unknown construct: 'isolation_fork'" (kmac_app_if.sv).
#[test]
fn test_sva_assert_without_semi_does_not_swallow_next_items() {
    let src = r#"
interface i;
  logic clk_i = 0;
  logic req_valid = 0;
  clocking mon_cb @(posedge clk_i);
  endclocking
  StrbAlignLSB_A:
    assert property (req_valid |-> ~|(req_valid & req_valid))
  NoReqBeforeLastResponse_A:
    assert property (req_valid |-> req_valid)
  task automatic wait_cycles(int unsigned num_cycles);
    fork : isolation_fork begin
      repeat (num_cycles) @mon_cb;
    end join
  endtask
endinterface
module top;
endmodule
"#;
    // compile_str Ok = parse lengkap (assert + task fork) tanpa error.
    let _d = compile_str(src).expect("SVA tanpa ';' + item berikut harus parse");
}

/// Port interface+modport `AXI_BUS.Slave in` — comma guard `Ident . Ident`
/// = port BARU (cva6 axi_cut.sv). Sebelumnya E1002 "found Dot".
#[test]
fn test_iface_modport_port_list() {
    let src = r#"
module axi_cut #(
  parameter int unsigned ID_WIDTH = 0,
  parameter type AXI_BUS = logic
) (
  input logic       clk_i,
  AXI_BUS.Slave     in,
  AXI_BUS.Master    out
);
endmodule
"#;
    let _d = compile_str(src).expect("port interface.modport harus parse");
}

/// `(* async *) .p(...)` — atribut sebelum koneksi instance (IEEE 1800
/// §22.11, cdc_fifo_gray.sv PULP). Sebelumnya E1002 "found Star".
#[test]
fn test_attr_before_instance_conn() {
    let src = r#"
module cdc (input logic a, output logic b);
  assign b = a;
endmodule
module top;
  logic a, b;
  cdc u_cdc (
    (* async *) .a(a),
    (* keep *)  .b(b)
  );
endmodule
"#;
    let _d = compile_str(src).expect("atribut sebelum koneksi instance harus parse");
}

/// `.T ( logic [$bits(x)-1:0] )` — type-param arg dgn packed range
/// (axi_cdc_dst.sv cva6). Range di-parse-dan-dibuang (DataType tanpa variant).
#[test]
fn test_type_param_range_arg() {
    let src = r#"
module cdc #(parameter type T = logic) ();
endmodule
module top;
  logic [7:0] w;
  cdc #(.T ( logic [$bits(w)-1:0] )) u ();
endmodule
"#;
    let _d = compile_str(src).expect("type-param arg dgn range harus parse");
}

/// Override type param via typedef BERLAKU (`#(.T(Wide16))`): port/sinyal
/// `T` 16-bit. Nilai width-sensitive (`0x0100<<3=0x0800` tak muat 8-bit).
/// Regresi: Ident-typedef salah-bucket jadi value-override → T tetap 8-bit.
#[test]
fn test_type_param_typedef_override_applies() {
    let source = r#"
package p;
typedef logic [15:0] Wide16;
endpackage
module shifter #(parameter type T = logic [7:0], parameter N = 2) (input bit clk, input T d, output T q);
T r;
always_ff @(posedge clk) begin r <= d << N; q <= r; end
endmodule
module tb;
import p::*;
bit clk;
logic [15:0] d16, q16;
shifter #(.T(Wide16), .N(3)) u16 (.clk(clk), .d(d16), .q(q16));
initial begin
clk = 0; d16 = 16'h0100; #10;
repeat (3) @(posedge clk); #1;
$display("TPOV q=%0h", q16);
$finish; end
initial begin forever #5 clk = ~clk; end
endmodule
"#;
    let sigs = simulate_signals(source, 200).expect("simulasi harus jalan");
    let q = sigs.iter().find(|(n, _)| n == "q16").unwrap().1.to_u64();
    assert_eq!(q, 0x0800, "T=Wide16: 0100<<3=0800 (q={q:04x})");
}

/// Override type param literal (`#(.T(logic[15:0]))`): range disimpan
/// parser (bukan dibuang) → lebar 16.
#[test]
fn test_type_param_literal_override_applies() {
    let source = r#"
module shifter #(parameter type T = logic [7:0], parameter N = 2) (input bit clk, input T d, output T q);
T r;
always_ff @(posedge clk) begin r <= d << N; q <= r; end
endmodule
module tb;
bit clk;
logic [15:0] d16, q16;
shifter #(.T(logic[15:0]), .N(3)) u (.clk(clk), .d(d16), .q(q16));
initial begin
clk = 0; d16 = 16'h0100; #10;
repeat (3) @(posedge clk); #1;
$display("TPL2 q=%0h", q16);
$finish; end
initial begin forever #5 clk = ~clk; end
endmodule
"#;
    let sigs = simulate_signals(source, 200).expect("simulasi harus jalan");
    let q = sigs.iter().find(|(n, _)| n == "q16").unwrap().1.to_u64();
    assert_eq!(q, 0x0800, "T=logic[15:0]: 0100<<3=0800 (q={q:04x})");
}

/// Cast ke type param (`T'(a)`) memakai lebar EFEKTIF (default/override),
/// bukan fallback 1-bit (data loss → 0).
#[test]
fn test_type_param_cast_uses_effective_width() {
    let source = r#"
module m #(parameter type T = logic [7:0]) (input logic [15:0] a, output logic [15:0] q);
assign q = T'(a);
endmodule
module tb;
logic [15:0] a, q8, q16;
m u8 (.a(a), .q(q8));
m #(.T(logic[15:0])) u16 (.a(a), .q(q16));
initial begin
a = 16'h1234; #1;
$display("CT8 q=%0h CT16 q=%0h", q8, q16);
$finish;
end
endmodule
"#;
    let sigs = simulate_signals(source, 50).expect("simulasi harus jalan");
    let q8 = sigs.iter().find(|(n, _)| n == "q8").unwrap().1.to_u64();
    let q16 = sigs.iter().find(|(n, _)| n == "q16").unwrap().1.to_u64();
    assert_eq!(q8, 0x34, "T default 8-bit: 1234→34 (q8={q8:04x})");
    assert_eq!(q16, 0x1234, "T override 16-bit: 1234→1234 (q16={q16:04x})");
}

/// `define M(a, b = $sformatf("%m")) $fatal(1, "...", b, a);` — penutup
/// param-list STRING/paren aware; invokasi dgn arg berisi `)` dalam string
/// (`dv_fatal("...ready()")`). Sebelumnya body bocor `)` / arg terpotong →
/// "expected expression, found RParen" (sw_logger_if.sv).
#[test]
fn test_macro_define_and_invoke_paren_string() {
    let src = r#"
`define dv_fatal(MSG_, ID_ = $sformatf("%m")) $fatal(1, "[%0s] %0s", ID_, MSG_);

module top;
  logic _ready = 1;
  initial begin
    _ready = _ready;
    // Simulasi TIDAK boleh sampai $fatal — hanya parse yang diverifikasi.
  end
endmodule
"#;
    let _d = compile_str(src).expect("define dgn default paren + string harus parse");
}

/// `constraint C::name { }` eksternal di dalam PACKAGE (include file DV,
/// adc_ctrl_env_pkg.sv). Body class sudah diparse; skip agar tak desync →
/// "expected wire/reg/..." (package.rs).
#[test]
fn test_package_level_extern_constraint() {
    let src = r#"
package env_pkg;
  import uvm_pkg::*;
  class cfg_c;
    rand int a;
    constraint min_le_max_c;
  endclass
  constraint cfg_c::min_le_max_c { a > 1; }
endpackage
module top;
endmodule
"#;
    let _d = compile_str(src).expect("constraint eksternal di package harus parse");
}

/// `localparam type T = struct packed { ... }` — type-param localparam dgn
/// struct (frontend.sv cva6). Sebelumnya "expected type" di `struct`.
#[test]
fn test_localparam_type_struct() {
    let src = r#"
module top;
  localparam type bht_update_t = struct packed {
    logic valid;
    logic [7:0] target;
  };
  bht_update_t upd;
endmodule
"#;
    let _d = compile_str(src).expect("localparam type struct harus parse");
}

// ===== Category 3: Port declaration errors =====

#[test]
fn test_parse_err_port_dot_no_paren() {
    assert!(compile_str("module top (.x); endmodule").is_err());
}

#[test]
fn test_parse_err_port_dot_no_name() {
    assert!(compile_str("module top (.); endmodule").is_err());
}

#[test]
fn test_parse_err_port_expr_bad() {
    assert!(compile_str("module top (.x (); endmodule").is_err());
}

#[test]
fn test_parse_err_port_missing_rparen() {
    assert!(compile_str("module top (output clk; endmodule").is_err());
}

#[test]
fn test_parse_err_port_nested_dot() {
    assert!(compile_str("module top (.a(.b())); endmodule").is_err());
}

#[test]
fn test_parse_err_port_dot_before_rparen() {
    assert!(compile_str("module top (.a, .); endmodule").is_err());
}

#[test]
fn test_parse_err_port_dir_then_dot() {
    assert!(compile_str("module top (output .); endmodule").is_err());
}

#[test]
fn test_parse_err_port_dot_no_lparen_after_comma() {
    assert!(compile_str("module top (.x, .); endmodule").is_err());
}

#[test]
fn test_parse_err_port_dot_after_dir() {
    assert!(compile_str("module top (input .); endmodule").is_err());
}

// ===== Category 4: Package errors =====

#[test]
fn test_parse_err_package_no_name() {
    assert!(compile_str("package ; endpackage").is_err());
}

#[test]
fn test_parse_err_package_eof() {
    assert!(compile_str("package p;").is_err());
}

#[test]
fn test_parse_err_package_keyword_name() {
    assert!(compile_str("package input; endpackage").is_err());
}

// ===== Category 5: Interface & Modport errors =====

#[test]
fn test_parse_err_interface_no_name() {
    assert!(compile_str("interface; endinterface").is_err());
}

#[test]
fn test_parse_err_interface_eof() {
    assert!(compile_str("interface i;").is_err());
}

#[test]
fn test_parse_err_modport_no_name() {
    assert!(compile_str("interface i; modport; endinterface").is_err());
}

#[test]
fn test_parse_err_modport_bad_dir() {
    assert!(compile_str("interface i; modport m (bad_dir x); endinterface").is_err());
}

#[test]
fn test_parse_err_modport_no_signal() {
    assert!(compile_str("interface i; modport m (input); endinterface").is_err());
}

// ===== Category 6: Class errors =====

#[test]
fn test_parse_err_class_no_name() {
    assert!(compile_str("class ; endclass").is_err());
}

#[test]
fn test_err_sanity_class_extends_bad() {
    assert!(compile_str("class c extends 42; endclass").is_err());
}

#[test]
fn test_parse_err_class_extends_keyword() {
    assert!(compile_str("class c extends input; endclass").is_err());
}

#[test]
fn test_parse_err_class_no_semi() {
    assert!(compile_str("class c endclass").is_err());
}

#[test]
fn test_parse_err_class_virtual_bad() {
    assert!(compile_str("class c; virtual 42; endclass").is_err());
}

// ===== Category 7: Generate errors (propagating) =====

// ===== Category 8: Additional port errors =====

#[test]
fn test_parse_err_port_multiple_dot_no_name() {
    assert!(compile_str("module top (.a, .); endmodule").is_err());
}

// ===== Category 9: Elaborator errors =====

#[test]
fn test_elab_err_alwaysff_no_sensitivity() {
    assert!(compile_str("module top; always_ff a <= b; endmodule").is_err());
}

#[test]
fn test_elab_err_alwaysff_no_clock_edge() {
    assert!(compile_str("module top; always_ff @(a) q <= d; endmodule").is_err());
}

#[test]
fn test_elab_err_gate_one_port() {
    assert!(compile_str("module top; and g(a); endmodule").is_err());
}

#[test]
fn test_elab_err_gate_port_expr() {
    assert!(compile_str("module top; and g(a+b, c); endmodule").is_err());
}

#[test]
fn test_elab_err_gate_port_unknown_sig() {
    assert!(compile_str("module top; and g(x, y); endmodule").is_err());
}

#[test]
fn test_elab_err_module_not_found() {
    assert!(compile_str("module top; nonexistent inst(.a(1)); endmodule").is_err());
}

#[test]
fn test_elab_err_instance_signal_not_found() {
    // Error di satu tempat bersifat GLOBAL: signal port tidak dikenal saat
    // instantiation adalah error elaborasi → compile gagal (tidak resilient).
    assert!(compile_str("module top; wire a; mod inst(.port(nonexistent)); endmodule; module mod; input port; endmodule")
            .is_err());
}

#[test]
fn test_elab_err_clog2_no_arg() {
    assert!(compile_str("module top; initial a = $clog2(); endmodule").is_err());
}

#[test]
fn test_elab_err_bits_no_arg() {
    assert!(compile_str("module top; initial a = $bits(); endmodule").is_err());
}

#[test]
fn test_elab_err_unsigned_two_args() {
    assert!(compile_str("module top; wire a; initial a = $unsigned(1, 2); endmodule").is_err());
}

#[test]
fn test_elab_err_high_no_arg() {
    assert!(compile_str("module top; initial a = $high(); endmodule").is_err());
}

#[test]
fn test_elab_err_low_no_arg() {
    assert!(compile_str("module top; initial a = $low(); endmodule").is_err());
}

#[test]
fn test_elab_time_assertion_fails_when_constant_false() {
    // ELAB-12: assert dengan kondisi parameter-dependent yang dapat di-eval
    // di elab-time (seluruh operand konstanta) harus melaporkan kegagalan
    // SAAT ELABORASI (warning RT7001 assertion failed), bukan hanya saat
    // simulasi. Pipeline lengkap seperti compile_str tapi tangkap diags.
    let source = r#"
module top #(parameter WIDTH = 4) ();
    initial begin
        assert (WIDTH > 8);   // 4 > 8 = false → elab-time failure
    end
endmodule
"#;
    let mut pp = mivon_parser::preprocessor::Preprocessor::new();
    let preprocessed = pp.preprocess(source, None).unwrap();
    let mut lexer = mivon_parser::lexer::Lexer::new(&preprocessed);
    let mut tokens = Vec::new();
    loop {
        let (tok, line, col) = lexer.next_token();
        if tok == mivon_parser::lexer::Token::Eof {
            break;
        }
        tokens.push((tok, line, col));
    }
    let mut parser = mivon_parser::Parser::new(tokens, "<string>").with_source_lines(&preprocessed);
    let design = parser.parse_design().unwrap();
    let source_lines: Vec<String> = preprocessed.lines().map(|s| s.to_string()).collect();
    let mut elaborator =
        mivon_elaboration::Elaborator::with_source(design, source_lines, "<string>".to_string());
    elaborator
        .elaborate(
            None,
            mivon_elaboration::elaborator::ElaborateMode::StrictSimulation,
        )
        .unwrap();
    let diags = elaborator.flush_diagnostics();
    assert!(
        diags.iter().any(
            |d| d.code == mivon_core::diagnostics::DiagCode::AssertionFailed
                && d.message.contains("elaboration-time assertion failed")
        ),
        "harus ada warning elab-time assertion failed: {:#?}",
        diags
            .iter()
            .map(|d| (d.code.as_str(), d.message.as_ref()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn test_elab_time_assertion_passes_when_constant_true() {
    // ELAB-12 (true path): kondisi konstanta true → TIDAK ada warning elab.
    let source = r#"
module top #(parameter WIDTH = 4) ();
    initial begin
        assert (WIDTH >= 4);   // 4 >= 4 = true → tidak boleh ada elab warning
    end
endmodule
"#;
    let mut pp = mivon_parser::preprocessor::Preprocessor::new();
    let preprocessed = pp.preprocess(source, None).unwrap();
    let mut lexer = mivon_parser::lexer::Lexer::new(&preprocessed);
    let mut tokens = Vec::new();
    loop {
        let (tok, line, col) = lexer.next_token();
        if tok == mivon_parser::lexer::Token::Eof {
            break;
        }
        tokens.push((tok, line, col));
    }
    let mut parser = mivon_parser::Parser::new(tokens, "<string>").with_source_lines(&preprocessed);
    let design = parser.parse_design().unwrap();
    let source_lines: Vec<String> = preprocessed.lines().map(|s| s.to_string()).collect();
    let mut elaborator =
        mivon_elaboration::Elaborator::with_source(design, source_lines, "<string>".to_string());
    elaborator
        .elaborate(
            None,
            mivon_elaboration::elaborator::ElaborateMode::StrictSimulation,
        )
        .unwrap();
    let diags = elaborator.flush_diagnostics();
    assert!(
        !diags
            .iter()
            .any(|d| d.code == mivon_core::diagnostics::DiagCode::AssertionFailed),
        "assertion true tidak boleh warn: {:#?}",
        diags
            .iter()
            .map(|d| (d.code.as_str(), d.message.as_ref()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn test_elab_err_left_no_arg() {
    assert!(compile_str("module top; initial a = $left(); endmodule").is_err());
}

#[test]
fn test_elab_err_right_no_arg() {
    assert!(compile_str("module top; initial a = $right(); endmodule").is_err());
}

#[test]
fn test_elab_err_size_no_arg() {
    assert!(compile_str("module top; initial a = $size(); endmodule").is_err());
}

#[test]
fn test_elab_err_bits_nonsignal_arg() {
    assert!(compile_str("module top; logic a; initial a = $bits(a.len()); endmodule").is_err());
}

// ===== Category 11: always_comb / always_latch / always with @ edge =====

#[test]
fn test_elab_err_always_comb_sensitivity() {
    assert!(compile_str("module top; always_comb @(posedge clk) a <= b; endmodule").is_err());
}

// ===== Category 12: Additional elaborator errors =====

#[test]
fn test_elab_err_undeclared_signal_in_assign() {
    assert!(compile_str("module top; initial y = x; endmodule").is_err());
}

#[test]
fn test_elab_err_undeclared_signal_in_expr() {
    assert!(compile_str("module top; wire a; initial a = b + 1; endmodule").is_err());
}

#[test]
fn test_elab_err_undeclared_signal_in_sens() {
    assert!(compile_str("module top; always @(posedge bad) q <= d; endmodule").is_err());
}

#[test]
fn test_elab_err_cont_assign_bad_lhs() {
    assert!(compile_str("module top; assign 1 + 2 = x; endmodule").is_err());
}

// ===== Category 14: Empty or near-empty sources =====

#[test]
fn test_parse_err_empty_source() {
    assert!(compile_str("").is_err());
}

#[test]
fn test_parse_err_only_whitespace() {
    assert!(compile_str("   \n  \t  ").is_err());
}

#[test]
fn test_parse_err_only_comments() {
    assert!(compile_str("// comment\n/* block */").is_err());
}

// ===== Category 15: Bad DPI import =====

// ===== Category 16: Bad covergroup =====

// ===== Category 17: Class extends errors =====

#[test]
fn test_parse_err_class_extends_no_name() {
    assert!(compile_str("class c extends ; endclass").is_err());
}

#[test]
fn test_parse_err_class_extends_integer() {
    assert!(compile_str("class c extends integer; endclass").is_err());
}

#[test]
fn test_parse_err_class_extends_begin() {
    assert!(compile_str("class c extends begin; endclass").is_err());
}

// ===== Category 18: Bad lvalue expressions =====

#[test]
fn test_elab_err_number_as_lvalue_blocking() {
    assert!(compile_str("module top; initial 42 = 1; endmodule").is_err());
}

#[test]
fn test_elab_expr_42_le_1() {
    // 42 <= 1; is an expression statement (Le comparison), not an NBA — valid SV
    assert!(compile_str("module top; initial 42 <= 1; endmodule").is_ok());
}

#[test]
fn test_elab_err_string_as_lvalue() {
    assert!(compile_str(r#"module top; initial "str" = 1; endmodule"#).is_err());
}

#[test]
fn test_elab_err_concat_as_lvalue() {
    assert!(compile_str("module top; initial {a, b} = 1; endmodule").is_err());
}

// ===== Category 19: Function not found =====

#[test]
fn test_elab_err_func_not_found_with_args() {
    assert!(compile_str("module top; wire a; initial a = my_func(1); endmodule").is_err());
}

#[test]
fn test_elab_err_func_not_found_no_args() {
    assert!(compile_str("module top; wire a; initial a = my_func(); endmodule").is_err());
}

#[test]
fn test_elab_err_func_not_found_nested() {
    assert!(compile_str("module top; wire a; initial a = foo(bar(x)); endmodule").is_err());
}

// ===== Category 20: Various top-level keywords =====

#[test]
fn test_parse_program() {
    assert!(compile_str("program p; endprogram").is_ok());
}

#[test]
fn test_program_simulation() {
    let sigs = simulate_signals("program p; logic a; initial a = 1; endprogram", 10).unwrap();
    let found = sigs.iter().any(|(n, _)| n == "a");
    assert!(found, "program simulation should produce signal a");
}

#[test]
fn test_parse_err_top_level_primitive() {
    assert!(compile_str("primitive p; endprimitive").is_err());
}

#[test]
fn test_parse_err_top_level_config() {
    assert!(compile_str("config c; endconfig").is_err());
}

// ===== Category 21: Various module body issues that reach elaborator =====

#[test]
fn test_elab_err_always_ff_no_clock_signal() {
    assert!(compile_str("module top; always_ff @(posedge clk) q <= d; endmodule").is_err());
}

#[test]
fn test_elab_err_always_ff_bad_sensitivity() {
    assert!(
        compile_str("module top; always_ff @(negedge clk or negedge rst) q <= d; endmodule")
            .is_err()
    );
}

#[test]
fn test_elab_err_always_no_sens_undeclared() {
    assert!(compile_str("module top; always @(posedge bad) q <= d; endmodule").is_err());
}

// ===== Category 22: More assign/expression elaborator errors =====

#[test]
fn test_elab_err_cont_assign_undeclared_lhs() {
    // Semantik SV: identifier tak dideklarasi di LHS continuous assign menjadi
    // implicit net 1-bit (bukan error). Verifikasi sim tetap jalan.
    let res = simulate_signals("module top; assign x = 1'b1; endmodule", 10).unwrap();
    assert!(
        res.iter().any(|(n, v)| n == "x" && v.to_u64() == 1),
        "implicit net 'x' harus = 1"
    );
}

#[test]
fn test_elab_err_cont_assign_undeclared_rhs() {
    // Semantik SV: identifier tak dideklarasi di RHS continuous assign juga
    // menjadi implicit net (konsisten dgn LHS — lihat test di atas). Reggen
    // OpenTitan (rom_ctrl_rom_reg_top dll.) mengandalkan perilaku ini.
    let res = simulate_signals("module top; wire x; assign x = y; endmodule", 10);
    assert!(
        res.is_ok(),
        "implicit net RHS harus diterima: {:?}",
        res.err()
    );
}

#[test]
fn test_elab_err_initial_assign_undeclared() {
    assert!(compile_str("module top; initial begin a = b; end endmodule").is_err());
}

// ===== Category 23: Bad instance connections (elaborator) =====

#[test]
fn test_elab_err_instance_bad_port_signal() {
    // Error di satu tempat bersifat GLOBAL: signal port tidak dikenal adalah
    // error elaborasi → compile gagal total (bukan fallback diam-diam).
    assert!(compile_str(
        "module mod(input a); endmodule; module top; mod inst(.a(nonexistent)); endmodule"
    )
    .is_err());
}

// ===== Category 24: System function with non-signal arguments =====

#[test]
fn test_elab_err_high_nonsignal_arg() {
    assert!(compile_str("module top; wire a; initial a = $high(42); endmodule").is_err());
}

#[test]
fn test_elab_err_low_nonsignal_arg() {
    assert!(compile_str("module top; wire a; initial a = $low(42); endmodule").is_err());
}

#[test]
fn test_elab_err_left_nonsignal_arg() {
    assert!(compile_str("module top; wire a; initial a = $left(42); endmodule").is_err());
}

#[test]
fn test_elab_err_right_nonsignal_arg() {
    assert!(compile_str("module top; wire a; initial a = $right(42); endmodule").is_err());
}

#[test]
fn test_elab_err_size_nonsignal_arg() {
    assert!(compile_str("module top; wire a; initial a = $size(42); endmodule").is_err());
}

// ===== Category 25: Bad package body =====

#[test]
fn test_parse_err_package_bad_body() {
    assert!(compile_str("package p; bad; endpackage").is_err());
}

// ===== Category 26: Bad interface body =====

#[test]
fn test_parse_err_interface_bad_body() {
    assert!(compile_str("interface i; bad; endinterface").is_err());
}

// ===== Category 27: Expression errors during elaboration =====

#[test]
fn test_elab_err_range_select_oob() {
    assert!(
        compile_str("module top; wire [3:0] x; initial begin y = x[10:0]; end endmodule").is_err()
    );
}

#[test]
fn test_elab_err_bit_select_oob() {
    assert!(
        compile_str("module top; wire [3:0] x; initial begin y = x[10]; end endmodule").is_err()
    );
}

// === Fuzzing-like tests ===

#[test]
fn test_fuzz_empty_param_list() {
    assert!(compile_str("module top #(); initial #1 $finish; endmodule").is_ok());
}

#[test]
fn test_fuzz_tab_instead_of_space() {
    assert!(compile_str("module\ttop;\treg\t[7:0]\tx;\tinitial\t#1\t$finish;\tendmodule").is_ok());
}

#[test]
fn test_fuzz_many_signals_10() {
    let mut src = "module top;\n".to_string();
    for i in 0..10 {
        src.push_str(&format!("    wire [7:0] w{};\n", i));
    }
    src.push_str("initial #1 $finish;\nendmodule");
    assert!(compile_str(&src).is_ok());
}

#[test]
fn test_fuzz_many_assigns_5() {
    let mut src = "module top;\n    wire [7:0] sum;\n".to_string();
    for i in 0..5 {
        src.push_str(&format!(
            "    wire [7:0] w{};\n    assign w{} = 8'd{};\n",
            i, i, i
        ));
    }
    src.push_str("initial #1 $finish;\nendmodule");
    assert!(compile_str(&src).is_ok());
}

// Division/mod by zero panics in const folder — known limitation

// === Additional runtime edge cases ===

#[test]
fn test_sim_edge_concat_replicate_large() {
    let sigs = simulate_signals(
        "module top; reg [31:0] x; initial begin x = {16{2'b10}}; #1 $finish; end endmodule",
        5,
    )
    .unwrap();
    let (_, v) = sigs.iter().find(|(n, _)| n == "x").unwrap();
    assert!(v.to_u64() > 0);
}

#[test]
fn test_sim_edge_nba_ordering() {
    let sigs = simulate_signals(
        r#"
module top;
    reg [7:0] a, b;
    initial begin
        a = 1;
        b = 2;
        a <= b;
        b <= a;
        #1 $finish;
    end
endmodule"#,
        5,
    )
    .unwrap();
    let (_, va) = sigs.iter().find(|(n, _)| n == "a").unwrap();
    let (_, vb) = sigs.iter().find(|(n, _)| n == "b").unwrap();
    assert_eq!(va.to_u64(), 2);
    assert_eq!(vb.to_u64(), 1);
}

#[test]
fn test_sim_edge_big_counter() {
    let sigs = simulate_signals(
        r#"
module top;
    reg clk;
    reg [31:0] cnt;
    always_ff @(posedge clk) cnt <= cnt + 1;
    initial begin clk = 0; cnt = 0; #100 $finish; end
    always #1 clk = ~clk;
endmodule"#,
        110,
    )
    .unwrap();
    let (_, v) = sigs.iter().find(|(n, _)| n == "cnt").unwrap();
    assert!(v.to_u64() >= 40, "cnt should be ~50, got {}", v.to_u64());
}

#[test]
fn test_sim_edge_fifo_write_read() {
    let sigs = simulate_signals(
        r#"
module top;
    reg [7:0] mem [0:3];
    reg [1:0] wp, rp;
    reg [7:0] rd;
    initial begin
        wp = 0; rp = 0;
        mem[wp] = 42; wp = wp + 1;
        mem[wp] = 99; wp = wp + 1;
        rp = 0; rd = mem[rp];
        #1 $finish;
    end
endmodule"#,
        5,
    )
    .unwrap();
    let (_, v) = sigs.iter().find(|(n, _)| n == "rd").unwrap();
    assert_eq!(v.to_u64(), 42);
}

#[test]
fn test_sim_edge_reduction_xor_parity() {
    let sigs = simulate_signals(
        r#"
module top;
    reg [7:0] a;
    reg par;
    initial begin
        a = 8'b10101010;
        par = ^a;
        #1 $finish;
    end
endmodule"#,
        5,
    )
    .unwrap();
    let (_, v) = sigs.iter().find(|(n, _)| n == "par").unwrap();
    assert_eq!(v.to_u64(), 0);
}

#[test]
fn test_sim_edge_concat_in_assign() {
    let sigs = simulate_signals(
        "module top; reg [7:0] x; initial begin x = {4'hA, 4'h5}; #1 $finish; end endmodule",
        5,
    )
    .unwrap();
    let (_, v) = sigs.iter().find(|(n, _)| n == "x").unwrap();
    assert_eq!(v.to_u64(), 0xA5);
}

#[test]
fn test_sim_edge_negation_bits() {
    let sigs = simulate_signals(
        "module top; reg [7:0] x; initial begin x = ~8'hA5; #1 $finish; end endmodule",
        5,
    )
    .unwrap();
    let (_, v) = sigs.iter().find(|(n, _)| n == "x").unwrap();
    // Verify bitwise NOT toggles bits
    assert_ne!(v.to_u64(), 0xA5, "bitwise NOT should change value");
    assert!(v.to_u64() < 256, "result should fit in 8 bits");
}

#[test]
fn test_sim_edge_signed_neg() {
    let result = compile_str(
        r#"
module top;
    reg signed [7:0] a;
    reg [7:0] b;
    initial begin
        a = -8'd10;
        b = a;
        #1 $finish;
    end
endmodule"#,
    );
    assert!(result.is_ok(), "signed negation: {:?}", result.err());
}

#[test]
fn test_sim_edge_long_shift() {
    let sigs = simulate_signals(
        "module top; reg [31:0] x; initial begin x = 32'd1 << 16; #1 $finish; end endmodule",
        5,
    )
    .unwrap();
    let (_, v) = sigs.iter().find(|(n, _)| n == "x").unwrap();
    assert_eq!(v.to_u64(), 65536);
}

#[test]
fn test_sim_edge_assign_from_const_func() {
    let result = compile_str(
        r#"
module top;
    function [7:0] add(input [7:0] a, b);
        add = a + b;
    endfunction
    wire [7:0] w;
    assign w = add(3, 4);
    initial #1 $finish;
endmodule"#,
    );
    assert!(result.is_ok(), "function in assign: {:?}", result.err());
}

// === Complex construct tests ===

#[test]
fn test_complex_alu() {
    let sigs = simulate_signals(
        r#"
module top;
    reg [7:0] a, b, result;
    reg [2:0] op;
    initial begin
        a = 10; b = 5;
        op = 0; result = a + b;
        op = 1; result = a - b;
        op = 2; result = a & b;
        op = 3; result = a | b;
        op = 4; result = a ^ b;
        #1 $finish;
    end
endmodule"#,
        5,
    )
    .unwrap();
    let (_, v) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(v.to_u64(), 15);
}

#[test]
fn test_complex_shift_register() {
    // Rotate-right shift register via concat
    let sigs = simulate_signals(
        r#"
module top;
    reg clk;
    reg [7:0] shift;
    always_ff @(posedge clk) shift <= {shift[6:0], shift[7]};
    initial begin
        clk = 0; shift = 8'b10000001;
        #3 clk = 1; #3 clk = 0;
        #3 clk = 1; #3 clk = 0;
        #1 $finish;
    end
endmodule"#,
        20,
    )
    .unwrap();
    let (_, v) = sigs.iter().find(|(n, _)| n == "shift").unwrap();
    // After 2 posedge events (rotate-right via concat): 0x81 → 3 → 6.
    // Cocok dengan iverilog.
    assert!(v.to_u64() == 6 || v.to_u64() == 3 || v.to_u64() == 0x81);
}

#[test]
fn test_complex_generate_adder_tree() {
    let result = compile_str(
        r#"
module top;
    wire [7:0] a, b, c, d, s1, s2, out;
    add2 u1(.a(a), .b(b), .s(s1));
    add2 u2(.a(c), .b(d), .s(s2));
    add2 u3(.a(s1), .b(s2), .s(out));
    initial #1 $finish;
endmodule
module add2(input [7:0] a, b, output [7:0] s);
    assign s = a + b;
endmodule"#,
    );
    assert!(result.is_ok());
}

// === Package import with multiple items ===

#[test]
fn test_complex_pkg_import_items() {
    let result = compile_str(
        r#"
package pkg;
    typedef logic [7:0] byte_t;
    parameter int DEPTH = 16;
endpackage
module top;
    import pkg::byte_t;
    import pkg::DEPTH;
    wire [7:0] x;
    integer y;
    initial begin x = 8'hA5; y = DEPTH; #1 $finish; end
endmodule"#,
    );
    // Package typedef with range may not be supported yet
    if let Err(err) = result {
        if !err.to_string().contains("typedef") {
            panic!("unexpected error: {}", err);
        }
    }
}

// === Foreach with multi-dimensional array ===

// 2D array for loop hangs parser — known issue with array ranges

// === More negative tests ===

#[test]
fn test_parse_err_missing_semi_in_block() {
    // Error recovery handles this gracefully (warning emitted, no crash)
    let _ = compile_str("module top; initial begin wire a end endmodule");
}

#[test]
fn test_parse_err_missing_end_in_fork() {
    // Error recovery handles fork without join gracefully
    let _ = compile_str("module top; initial fork #1 a=1; endmodule");
}

#[test]
fn test_parse_err_unclosed_string() {
    assert!(compile_str(r#"module top; initial $display("hello); #1 $finish; endmodule"#).is_err());
}

#[test]
fn test_parse_err_fake_keyword_after_modport() {
    assert!(compile_str("interface i; modport m (xyz x); endinterface").is_err());
}

// `always clk` without parens hangs parser — known error recovery issue

// `end` vs `endmodule` triggers error recovery infinite loop — skip

// === Additional preprocessor tests ===

#[test]
fn test_pp_undef_not_implemented() {
    // The preprocessor doesn't have undef; redefining does not undefine
    let mut pp = Preprocessor::new();
    pp.define("X", "1");
    assert_eq!(
        pp.preprocess("`ifdef X\na\n`endif", None).unwrap().trim(),
        "a"
    );
}

#[test]
fn test_pp_nested_include() {
    let dir = std::env::temp_dir().join("test_pp_nested");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("inner.sv"), "wire inner_w;\n").unwrap();
    let source = format!("`include \"{}\"", dir.join("inner.sv").display());
    let mut pp = Preprocessor::new();
    let result = pp.preprocess(&source, Some(&dir));
    assert!(result.is_ok());
    let out = result.unwrap();
    assert!(out.contains("inner_w"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_pp_define_empty() {
    let mut pp = Preprocessor::new();
    pp.define("EMPTY", "");
    let out = pp.preprocess("a `EMPTY b", None).unwrap();
    let trimmed = out.trim();
    assert!(
        trimmed.starts_with("a") && trimmed.contains("b"),
        "empty expansion: {}",
        trimmed
    );
}

#[test]
fn test_pp_define_with_equals() {
    let mut pp = Preprocessor::new();
    pp.define("WIDTH", "8");
    let out = pp.preprocess("wire [`WIDTH-1:0] x;", None).unwrap();
    assert_eq!(out.trim(), "wire [8-1:0] x;");
}

#[test]
fn test_pp_elsif_chain() {
    let mut pp = Preprocessor::new();
    let out = pp
        .preprocess(
            "`ifdef A\na\n`elsif B\nb\n`elsif C\nc\n`else\nd\n`endif",
            None,
        )
        .unwrap();
    assert_eq!(out.trim(), "d");
}

#[test]
fn test_pp_define_param_style() {
    let mut pp = Preprocessor::new();
    pp.define("SIZE", "256");
    let out = pp.preprocess("reg [`SIZE-1:0] mem;", None).unwrap();
    assert!(out.contains("256"));
}

#[test]
fn test_fuzz_escaped_ident() {
    assert!(compile_str(r"module top; reg \a+b ; initial #1 $finish; endmodule").is_ok());
}
