//! Regresi localparam lokal dalam body generate-for (fuzzer 2026-09-29).
//!
//! Dua cacat yang saling menutupi:
//! 1. `collect_scope_locals` tak mengumpulkan `ModuleItem::Param` + tak
//!    rekursif ke cabang If → rename scope dilewati → SEMUA iterasi share
//!    nama `Pa` → tabrakan key param_vals (nilai iterasi pertama menang) →
//!    index localparam salah utk iterasi kedua+ (repro t7/t9).
//! 2. Saat rename disertakan, nama `label[cur]` kolisi ANTAR iterasi loop
//!    LUAR saat nested (lv=0 & lv=1 dua-duanya punya `go[0]`) → fix:
//!    `fold_localparams` — const-fold localparam → literal setelah
//!    substitusi genvar (repro t3d/t3b).
//!
//! Golden: iverilog `-g2012` (dikutip sebagai expected literal).

use mivon_api::compile_str;

/// Node loop dgn localparam Pa/C0/C1 (repro t7/t9 — single-level).
const GEN_LP_SINGLE: &str = r#"
module gp_single;
  logic [6:0] vld;
  logic [3:0] valid_i;
  assign vld[3] = valid_i[0];
  assign vld[4] = valid_i[1];
  assign vld[5] = valid_i[2];
  assign vld[6] = valid_i[3];
  for (genvar offset = 0; offset < 2; offset++) begin : go
    localparam int Pa = 1 + offset;
    localparam int C0 = 3 + 2*offset;
    localparam int C1 = 4 + 2*offset;
    assign vld[Pa] = vld[C0] | vld[C1];
  end
  assign vld[0] = vld[1] | vld[2];
  initial begin
    valid_i = 4'b1111;
    #1 $display("R vld=%b", vld);
    $finish;
  end
endmodule
"#;

/// Nested 2 level (repro t3d/t3b — scope kolisi antar iterasi luar).
const GEN_LP_NESTED: &str = r#"
module gp_nested;
  logic [6:0] vld;
  logic [3:0] valid_i;
  assign vld[3] = valid_i[0];
  assign vld[4] = valid_i[1];
  assign vld[5] = valid_i[2];
  assign vld[6] = valid_i[3];
  for (genvar lv = 0; lv < 2; lv++) begin : gl
    for (genvar offset = 0; offset < 2**lv; offset++) begin : go
      localparam int Pa = (2**lv)-1 + offset;
      localparam int C0 = (2**(lv+1))-1 + 2*offset;
      localparam int C1 = C0 + 1;
      assign vld[Pa] = vld[C0] | vld[C1];
    end
  end
  initial begin
    valid_i = 4'b1111;
    #1 $display("S vld=%b", vld);
    $finish;
  end
endmodule
"#;

/// Jalankan source, kembalikan nilai `vld` sbg string bit MSB-first.
fn run_vld(src: &str) -> String {
    let design = compile_str(src).expect("harus compile");
    let mut engine = mivon_simulator::simulator::SimulationEngine::new(design, 10);
    engine.run().expect("sim harus jalan");
    let idx = engine
        .design
        .top
        .signals
        .iter()
        .position(|s| s.name == "vld")
        .expect("vld harus ada");
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

#[test]
fn generate_localparam_single_level_index_correct() {
    let v = run_vld(GEN_LP_SINGLE);
    assert_eq!(
        v, "1111111",
        "index localparam iterasi kedua salah (tabrakan nama Pa) — golden iverilog 1111111: {v}"
    );
}

#[test]
fn generate_localparam_nested_scope_no_collision() {
    let v = run_vld(GEN_LP_NESTED);
    assert_eq!(
        v, "1111111",
        "nama scope kolisi antar iterasi loop luar (nested) — golden iverilog 1111111: {v}"
    );
}
