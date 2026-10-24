//! Regresi fallback SERIAL jalur DAG-parallel utk ekspresi tak didukung.
//!
//! Evaluator parallel (`evaluate_expr_simple`) dulu punya fallback senyap
//! `_ => Ok(LogicVec::new(32))` = 32-bit X utk variant tanpa arm
//! (FuncCall/DpiCall/SysFunc/...) → `assign mem = {pkg::f(), ...}` hasil
//! default/`-P` = 0 (DPI stub return 0) tapi `--parallel` = X →
//! differential antar engine (fuzzer sim bug_0508). Kini evaluator
//! parallel mengembalikan Err → core.rs men-fallback layer ke jalur serial
//! (jalur referensi) — hasil wajib identik.

use mivon_api::compile_str;

/// Persis repro fuzzer bug_0508: package function ABEN → DPI stub
/// returns 0 di jalur serial; tanpa fallback, DAG menghasilkan X.
const DAG_DPI_SRC: &str = r#"
module dpi_assign (
    input  logic         clk_i,
    input  logic         req_i,
    input  logic [31:0]  addr_i,
    output logic [31:0]  rdata_o
);
    localparam int RomSize = 2;
    logic [RomSize-1:0][31:0] mem;
    assign mem = {
        absent_pkg::jalr(5'h0, 5'h1, 32'h1c00_0080),
        absent_pkg::lui(5'h1, 32'h1c00_0080)
    };
    logic [$clog2(RomSize)-1:0] addr_q;
    assign rdata_o = (addr_q < RomSize) ? mem[addr_q] : '0;
    always_ff @(posedge clk_i) begin
        if (req_i) begin
            addr_q <= addr_i[$clog2(RomSize)-1+3:2];
        end
    end
endmodule
"#;

/// Jalankan design dgn flag DAG, kembalikan nilai `rdata_o` sbg string bit
/// MSB-first (pola sama parallel_emi_tests).
fn run_rdata(use_dag: bool) -> String {
    let design = compile_str(DAG_DPI_SRC).expect("design harus compile");
    let mut engine = mivon_simulator::simulator::SimulationEngine::new(design, 50);
    engine.use_dag_parallel = use_dag;
    engine.run().expect("sim harus jalan");
    let idx = engine
        .design
        .top
        .signals
        .iter()
        .position(|s| s.name == "rdata_o")
        .expect("rdata_o harus ada");
    engine
        .state
        .read_signal(idx)
        .bits
        .iter()
        .rev()
        .map(|b| match b {
            mivon_ir::LogicVal::Zero => '0',
            mivon_ir::LogicVal::One => '1',
            mivon_ir::LogicVal::X => 'x',
            mivon_ir::LogicVal::Z => 'z',
        })
        .collect()
}

/// Jalur `--parallel` wajib identik dgn default utk assign berisi
/// function-call package (ekspresi tanpa arm di evaluator parallel).
#[test]
fn dag_parallel_serial_fallback_matches_default() {
    let default_v = run_rdata(false);
    let dag_v = run_rdata(true);
    assert_eq!(
        default_v, dag_v,
        "DAG-parallel beda dgn default (fallback serial tidak bekerja): \
         default={default_v} dag={dag_v}"
    );
    // DPI stub returns 0 → rdata_o ter-drive 0 (bukan X senyap).
    assert!(
        !dag_v.contains('x'),
        "rdata_o mengandung X di jalur DAG — evaluator parallel fallback ke X \
         senyap lagi: {dag_v}"
    );
}

/// Baca nilai sinyal flat (post-flatten) sbg string bit MSB→LSB utk
/// design dari source `src` dgn flag DAG on/off.
fn run_signal_bits(src: &str, name: &str, use_dag: bool) -> String {
    let design = compile_str(src).expect("design harus compile");
    let mut engine = mivon_simulator::simulator::SimulationEngine::new(design, 100);
    engine.use_dag_parallel = use_dag;
    engine.run().expect("sim harus jalan");
    let idx = engine
        .design
        .top
        .signals
        .iter()
        .position(|s| s.name == name)
        .unwrap_or_else(|| panic!("sinyal '{name}' harus ada di design flat"));
    engine
        .state
        .read_signal(idx)
        .bits
        .iter()
        .rev()
        .map(|b| match b {
            mivon_ir::LogicVal::Zero => '0',
            mivon_ir::LogicVal::One => '1',
            mivon_ir::LogicVal::X => 'x',
            mivon_ir::LogicVal::Z => 'z',
        })
        .collect()
}

/// MINIMAL REPRO (fuzz bug_0cb12a3dc00b / bug_63d353b8c78f, kampanye
/// sim 2026-10-19): submodule pasif (`assign out = m`, m masih X,
/// tanpa stimulus) + 2 part-select assign ke `q` di parent. Semantik
/// LRM: out = m(X) → dout X → kedua slice q = X; bit58 TIDAK ter-drive
/// → tetap Z. Fix dag (commit cbf7608) = nilai PINNED persis (bukan
/// sekadar identik): default = dag = "z" (bit58) + "x"×58. Kalau remake
/// silent shift kebalik arah (mis. patch drop → all-z), test ini tetap
/// kena.
const PASSIVE_CHAIN_SRC: &str = r#"
module mem_sub(out);
  output [28:0] out;
  reg [28:0] m;
  assign out = m;
endmodule

module top;
  wire [28:0] dout_lo;
  wire [28:0] dout_hi;
  wire [58:0] q;
  mem_sub s_lo(.out(dout_lo));
  mem_sub s_hi(.out(dout_hi));
  assign q[57:29] = dout_hi;
  assign q[28:0]  = dout_lo;
endmodule
"#;

#[test]
fn dag_vs_default_passive_slice_chain_identical() {
    let def = run_signal_bits(PASSIVE_CHAIN_SRC, "dout_lo", false);
    let dag = run_signal_bits(PASSIVE_CHAIN_SRC, "dout_lo", true);
    assert_eq!(
        def, dag,
        "dout_lo beda antar jalur: default={def} dag={dag}"
    );
    // PINNED — bukan sekadar identik: bit58 undriven z, 58 bit cover q = x.
    let expect = format!("z{}", "x".repeat(58));
    assert_eq!(def, "x".repeat(29), "dout_lo harus X semua (reg masih X)");
    let qd = run_signal_bits(PASSIVE_CHAIN_SRC, "q", false);
    let qg = run_signal_bits(PASSIVE_CHAIN_SRC, "q", true);
    assert_eq!(qd, expect, "q default beda pin: {qd}");
    assert_eq!(qg, expect, "q dag beda pin: {qg}");
}
