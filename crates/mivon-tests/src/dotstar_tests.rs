//! Regresi implicit port connection `.*` (IEEE 1800 §23.2.2.3).
//!
//! Dulu parser MELEWATI `.*` total (`advance; continue`) → `port_conns`
//! kosong → SEMUA port instance tak terhubung (input mengambang X, output
//! tak ter-drive) tanpa error apa pun — fuzzer: `sub dut (.*)` → output
//! `y=q=X` padahal stimulus benar.

use mivon_api::compile_str;

const DOTSTAR_SRC: &str = r#"
module ds_sub (input logic [3:0] a, output logic [3:0] b);
  assign b = ~a;
endmodule
module ds_top;
  logic [3:0] a;
  logic [3:0] b;
  ds_sub u1 (.*); // implicit: a↔a, b↔b
  initial begin
    a = 4'h5;
    #1 $display("DS b=%h", b);
    $finish;
  end
endmodule
"#;

fn run_ds() -> String {
    let design = compile_str(DOTSTAR_SRC).expect("harus compile");
    let mut engine = mivon_simulator::simulator::SimulationEngine::new(design, 10);
    engine.run().expect("sim harus jalan");
    let idx = engine
        .design
        .top
        .signals
        .iter()
        .position(|s| s.name == "b")
        .expect("b ada");
    engine
        .state
        .read_signal(idx)
        .bits
        .iter()
        .rev()
        .map(|bit| match bit {
            mivon_ir::LogicVal::Zero => '0',
            mivon_ir::LogicVal::One => '1',
            mivon_ir::LogicVal::X => 'x',
            mivon_ir::LogicVal::Z => 'z',
        })
        .collect()
}

/// `.*` wajib menghubungkan port ke signal bernama sama — dulu semua port
/// mengambang (output X tanpa error).
#[test]
fn dot_star_connects_ports_by_name() {
    let v = run_ds();
    assert_eq!(
        v, "1010",
        "port connection `.*` tak terhubung (dulu output X): ~4'h5 = a, dapat {v}"
    );
}

/// Regresi collect_sensitivity utk `Expr::Cast` (`logic'(a > b)`).
///
/// Dulu Cast jatuh ke `_ => vec![]` → sensitivity KOSONG senyap → assign
/// bergantung cast tak re-evaluate saat operand berubah (fuzzer: sel tree
/// prim_max_tree stale pasca update mx → `2/1` vs LRM `0/5`).
const CAST_SENS_SRC: &str = r#"
module cast_sens;
  logic [7:0] a, b;
  logic sel;
  assign sel = a > 0 && logic'(b > a);
  initial begin
    a = 8'd5; b = 8'd1;
    #1 $display("CS1 sel=%b (expect 1)", sel);
    b = 8'd9;
    #1 $display("CS2 sel=%b (expect 1)", sel);
    b = 8'd0;
    #1 $display("CS3 sel=%b (expect 0)", sel);
    $finish;
  end
endmodule
"#;

/// Cast dlm expr assign harus ikut sensitivity (re-evaluate saat operand
/// berubah) — dulu sel stale: perubahan `b` tak menggerakkan `sel`.
#[test]
fn cast_expression_participates_in_sensitivity() {
    // Observasi via sinyal final: setelah b=0 → sel harus 0.
    let design = compile_str(CAST_SENS_SRC).expect("harus compile");
    let mut engine = mivon_simulator::simulator::SimulationEngine::new(design, 10);
    engine.run().expect("sim harus jalan");
    let idx = engine
        .design
        .top
        .signals
        .iter()
        .position(|s| s.name == "sel")
        .expect("sel ada");
    let v = engine.state.read_signal(idx);
    let final_sel = v.bits.first().copied().unwrap_or(mivon_ir::LogicVal::X);
    assert_eq!(
        final_sel,
        mivon_ir::LogicVal::Zero,
        "sel tak re-evaluate setelah operand berubah (sensitivity Cast kosong) — \
         akhir simulasi harus 0 (b=8'd0)"
    );
}
