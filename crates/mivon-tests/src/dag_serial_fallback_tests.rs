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
