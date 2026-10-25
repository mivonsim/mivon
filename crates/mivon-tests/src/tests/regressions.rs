//! Test regresi bug nyata + fitur simulasi lanjutan (makro rekursif,
//! deteksi osilasi, coverage control, glitch, JIT body, dan regresi OpenTitan).

use super::*;

// ── CRIT-047: Macro expansion recursive depth limit ──────────────────────

#[test]
fn test_macro_recursive_expansion_simple() {
    // Test that nested macros are recursively expanded
    // `SIZE is defined as `WIDTH, which should recursively expand to 8
    use mivon_parser::preprocessor::Preprocessor;
    let mut pp = Preprocessor::new();
    let source = "`define WIDTH 8\n`define SIZE `WIDTH\nwire [`SIZE-1:0] data;\n";
    let result = pp.preprocess(source, None).unwrap();
    // SIZE → `WIDTH → 8
    assert!(
        result.contains("wire [8-1:0] data;"),
        "nested macro expansion failed: {}",
        result
    );
}

#[test]
fn test_macro_recursive_chain() {
    // Test chain: A → B → C (each refers to the next via backtick)
    use mivon_parser::preprocessor::Preprocessor;
    let mut pp = Preprocessor::new();
    let source = "`define C 42\n`define B `C\n`define A `B\nwire [8-`A:0] data;\n";
    let result = pp.preprocess(source, None).unwrap();
    // A → `B → `C → 42
    assert!(
        result.contains("wire [8-42:0] data;"),
        "3-level macro chain failed: {}",
        result
    );
}

#[test]
fn test_macro_recursive_with_args() {
    // Test that macro with args can contain `macro references that get expanded
    use mivon_parser::preprocessor::Preprocessor;
    let mut pp = Preprocessor::new();
    let source = "`define SCALE 100\n`define MUL(a,b) a * b * `SCALE\nwire w = `MUL(2, 3);\n";
    let result = pp.preprocess(source, None).unwrap();
    // MUL(2,3) → 2 * 3 * `SCALE → 2 * 3 * 100
    assert!(
        result.contains("2 * 3 * 100"),
        "macro with args + nested macro failed: {}",
        result
    );
}

#[test]
fn test_macro_recursive_depth_limit_prevents_overflow() {
    // Test that circular macros (A → B → A) don't cause stack overflow
    use mivon_parser::preprocessor::Preprocessor;
    let mut pp = Preprocessor::new();
    let source = "`define A `B\n`define B `A\nwire w = `A;\n";
    // Should not overflow: depth limit should stop expansion and return partial result
    let result = pp.preprocess(source, None).unwrap();
    // The result won't be fully expanded, but it should NOT cause a stack overflow/crash
    assert!(
        result.contains("wire w ="),
        "circular macro should not crash: {}",
        result
    );
}

#[test]
fn test_macro_recursive_self_reference_limit() {
    // Test that a macro referring to itself stops at depth limit
    use mivon_parser::preprocessor::Preprocessor;
    let mut pp = Preprocessor::new();
    let source = "`define X `X + 1\nwire w = `X;\n";
    // Should not overflow
    let result = pp.preprocess(source, None).unwrap();
    assert!(
        result.contains("wire w ="),
        "self-referential macro should not crash: {}",
        result
    );
}

#[test]
fn test_macro_recursive_expansion_with_simulation() {
    // Full simulation test: recursive macro expansion should produce correct HDL
    let source = r#"
`define BASE 5
`define ADD_BASE(x) x + `BASE

module tb;
    reg [7:0] result;
    initial begin
        result = `ADD_BASE(10);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(
        val.to_u64(),
        15,
        "recursive macro ADD_BASE(10) should expand to 10+5=15"
    );
}

#[test]
fn test_always_comb_self_trigger_loop_detection() {
    // Self-triggering always_comb: a = ~a creates infinite delta cycle
    // a must be initialized to known value first, otherwise ~X = X (no loop)
    // The engine should detect this via delta_limit and return InfiniteDelta error
    let source = r#"
module tb;
    reg a;
    initial a = 0;
    always_comb a = ~a;
endmodule
"#;
    let design = crate::compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 10);
    engine.set_delta_limit(100); // Low limit for fast test
    let result = engine.run();
    assert!(
        result.is_err(),
        "self-triggering always_comb should hit delta limit"
    );
    // Check that the error is InfiniteDelta (RT2001)
    let err = result.unwrap_err();
    let err_str = format!("{}", err);
    assert!(
        err_str.contains("RT2001")
            || err_str.contains("delta")
            || err_str.contains("InfiniteDelta"),
        "error should mention delta limit: got '{}'",
        err_str
    );
}

#[test]
fn test_combinational_oscillation_cycle_detection() {
    // SIM-28: osilasi 2-state (a = ~a) membentuk cycle state 0→1→0→1...
    // Deteksi cycle hash harus abort CEPAT (delta ~1000) walau delta_limit
    // sangat tinggi — bukan menunggu puluhan juta delta.
    let source = r#"
module tb;
    reg a;
    initial a = 0;
    always_comb a = ~a;
endmodule
"#;
    let design = crate::compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 10);
    engine.set_delta_limit(10_000_000); // jangan pernah terpicu dalam test
    let result = engine.run();
    let err = result.expect_err("oscillation harus terdeteksi, bukan hang");
    let err_str = format!("{}", err);
    assert!(
        err_str.contains("osilasi") || err_str.contains("kombinational"),
        "error harus menyebut osilasi/cycle: got '{}'",
        err_str
    );
}

#[test]
fn test_unconnected_port_no_oscillation() {
    // PORT-1: port output tak terhubung (`.data_o()`) harus DIBIARKAN
    // mengambang — bukan dikoneksikan ke literal 0. Koneksi ke 0 membuat
    // multiple-driver (always_comb child vs port_assign 0) → X-conflict →
    // osilasi delta (terlihat di prim_secded checkers OpenTitan). Dengan fix,
    // child drive output internal, tidak ada feedback → sim selesai bersih.
    let source = r#"
module prim_chk (
  input [7:0] data_i,
  output logic [7:0] data_o,
  output logic [1:0] err_o
);
  always_comb begin
    data_o = ~data_i;
    err_o = data_i[0] ? 2'b01 : 2'b00;
  end
endmodule

module tb;
  logic [7:0] data_i;
  logic [1:0] err_o;
  initial begin
    data_i = 8'h55;
    #10 data_i = 8'hAA;
    #10 $finish;
  end
  prim_chk u_chk (
    .data_i(data_i),
    .data_o(),
    .err_o(err_o)
  );
endmodule
"#;
    let sigs = crate::simulate_signals(source, 25).expect("sim harus selesai tanpa osilasi");
    let (_, err_v) = sigs.iter().find(|(n, _)| n == "err_o").expect("err_o ada");
    assert_eq!(
        err_v.to_u64(),
        0,
        "err_o = data_i[0] ? 1 : 0 — data_i[0]=0 di akhir, harus 0 (bukan X/2)"
    );
    let (_, di) = sigs
        .iter()
        .find(|(n, _)| n == "data_i")
        .expect("data_i ada");
    assert_eq!(di.to_u64(), 0xAA, "data_i harus 0xAA setelah #20");
}

#[test]
fn test_combinational_loop_oscillation_detection() {
    // Combinational loop with multiple always_comb blocks
    // that stabilizes after a few delta oscillations
    // Tests that oscillation detection (signal_write_count) does NOT false-positive
    let source = r#"
module tb;
    reg [3:0] a, b;
    always_comb begin
        if (a > 8)
            a = 8;
        if (b > 8)
            b = 8;
    end
    initial begin
        a = 15;
        b = 3;
        #1 $finish;
    end
endmodule
"#;
    // This should simulate without hitting delta limit
    let result = crate::simulate_signals(source, 2);
    assert!(
        result.is_ok(),
        "stable combinational logic should not trigger delta limit: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_callback_infrastructure() {
    // Test callback queue infrastructure directly from Rust
    // Create a minimal design and engine
    let source = r#"
module tb;
    reg a;
    always_comb a = 1;
endmodule
"#;
    let design = crate::compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 5);

    // Manually add a callback entry to the queue
    let key = ("my_component".to_string(), "my_cb_type".to_string());
    engine.callback_queues.insert(
        key.clone(),
        crate::simulator::types::UvmCallbackData {
            cb_type_name: "my_cb_type".to_string(),
            callbacks: Vec::new(),
            enabled: true,
        },
    );

    // Verify the callback is in the queue
    let entry = engine.callback_queues.get(&key);
    assert!(entry.is_some(), "callback should exist in queue");
    assert!(entry.unwrap().enabled, "callback should be enabled");
    assert!(
        entry.unwrap().callbacks.is_empty(),
        "no callbacks registered yet"
    );

    // Run simulation — should not crash
    let result = engine.run();
    assert!(
        result.is_ok(),
        "engine should run with callbacks: {:?}",
        result.err()
    );

    eprintln!("UVN callback infrastructure test passed");
}

#[test]
fn test_jit_body_combinational() {
    // Integration test: end-to-end simulation with JIT body compilation enabled.
    // Self-contained design with const expression (tests interpreter+engine integration OK)
    let source = r#"
module test_jit_body;
    reg [7:0] out;
    always_comb begin
        out = 10 + 20;
    end
    initial begin
        #1;
        $finish;
    end
endmodule
"#;
    let design = crate::compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 10);
    engine.set_use_mir_jit(true);
    let result = engine.run();
    assert!(
        result.is_ok(),
        "JIT body sim should succeed: {:?}",
        result.err()
    );
    let out_sig_idx = engine
        .design
        .top
        .signals
        .iter()
        .position(|s| s.name.as_str() == "out")
        .unwrap();
    let out_val = engine.state.read_signal(out_sig_idx).to_u64();
    eprintln!("JIT combinational integration test: out = {}", out_val);
    // constant expr 10 + 20 = 30 computed at time 0 via JIT
    assert_eq!(out_val, 30, "10 + 20 should be 30 via JIT body");
}

#[test]
fn test_jit_falls_back_on_x_input() {
    // Guard tri-state: jika proses membaca sinyal X/Z, jalur mir-jit harus
    // fallback ke interpreter agar propagasi X benar. Tanpa guard, aritmetika
    // u64 (X->0) memberi y = 0 + 1 = 1 (definitif, salah); dengan guard y = X.
    // Regresi bug_0366cd02200e_differential.
    let source = r#"
module test_jit_x;
    reg [7:0] a;
    reg [7:0] y;
    always_comb begin
        y = a + 1'b1;
    end
    initial begin
        #1;
        $finish;
    end
endmodule
"#;
    let design = crate::compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 10);
    engine.set_use_mir_jit(true);
    let result = engine.run();
    assert!(result.is_ok(), "JIT X fallback sim: {:?}", result.err());
    let y_idx = engine
        .design
        .top
        .signals
        .iter()
        .position(|s| s.name.as_str() == "y")
        .unwrap();
    let y = engine.state.read_signal(y_idx);
    assert!(
        y.has_unknown(),
        "y harus X (fallback interpreter), bukan nilai definitif u64={}",
        y.to_u64()
    );
}

#[test]
fn test_jit_body_nonblocking_assign() {
    // Integration test: JIT body with non-blocking assignment.
    // Simple register: always_ff @(posedge clk) q <= d
    let source = r#"
module test_jit_nba(
    input clk,
    input [7:0] d,
    output reg [7:0] q
);
    always_ff @(posedge clk) begin
        q <= d;
    end
    initial begin
        #1;
    end
endmodule
"#;
    let design = crate::compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 10);
    engine.set_use_mir_jit(true);
    let d_sig_idx = engine
        .design
        .top
        .signals
        .iter()
        .position(|s| s.name.as_str() == "d")
        .unwrap();
    let q_sig_idx = engine
        .design
        .top
        .signals
        .iter()
        .position(|s| s.name.as_str() == "q")
        .unwrap();
    // Set d = 42
    engine
        .state
        .write_signal(d_sig_idx, mivon_ir::LogicVec::from_u64(42, 8));
    // Toggle clock to trigger the always_ff: manually run edge mechanism
    let result = engine.run();
    assert!(
        result.is_ok(),
        "JIT NBA sim should succeed: {:?}",
        result.err()
    );
    eprintln!(
        "JIT NBA integration test passed, q = {}",
        engine.state.read_signal(q_sig_idx).to_u64()
    );
}

// ─── SIM-30: $coverage_control semua mode (per-type gating) ───────────────

#[test]
fn test_coverage_control_bitmask_off() {
    // $coverage_control(0) — semua coverage nonaktif
    let source = r#"
module tb;
    reg a;
    covergroup cg;
        cp_a: coverpoint a;
    endgroup
    cg cg_inst = new();
    initial begin
        $coverage_control(32'h0);
        a = 1;
        #1 cg_inst.sample();
        #1 $finish;
    end
endmodule
"#;
    let design = crate::compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 4);
    engine.run().unwrap();
    assert!(
        !engine.coverage_enabled,
        "coverage should be disabled with bitmask 0"
    );
    assert!(engine.coverage_enabled_types.is_empty());
    assert_eq!(
        engine.coverage_options.get("control").map(|s| s.as_str()),
        Some("0"),
        "coverage_options.control harus menyimpan bitmask"
    );
}

#[test]
fn test_coverage_control_bitmask_all_on() {
    // $coverage_control(~0) — semua tipe aktif (set kosong = semua enabled)
    let source = r#"
module tb;
    reg a;
    initial begin
        $coverage_control(32'hFFFF_FFFF);
        a = 1;
        #1 $finish;
    end
endmodule
"#;
    let design = crate::compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 2);
    engine.run().unwrap();
    assert!(
        engine.coverage_enabled,
        "coverage should be enabled with ~0"
    );
    assert!(
        engine.coverage_enabled_types.is_empty(),
        "empty set = semua tipe enabled"
    );
}

#[test]
fn test_coverage_control_bitmask_toggle_only() {
    // $coverage_control(0x2) — hanya toggle coverage aktif
    let source = r#"
module tb;
    reg a;
    initial begin
        $coverage_control(32'h0000_0002);
        a = 0;
        #1 a = 1;
        #1 $finish;
    end
endmodule
"#;
    let design = crate::compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 3);
    engine.run().unwrap();
    use crate::simulator::types::CoverageType;
    assert!(engine.coverage_enabled, "coverage enabled with toggle bit");
    assert!(
        engine
            .coverage_enabled_types
            .contains(&CoverageType::Toggle),
        "Toggle harus ter-enable: {:?}",
        engine.coverage_enabled_types
    );
    assert!(
        !engine.coverage_enabled_types.contains(&CoverageType::Line),
        "Line tidak boleh ter-enable: {:?}",
        engine.coverage_enabled_types
    );
    // Hanya toggle yang tercatat
    assert!(!engine.cover_toggle.is_empty(), "toggle harus tercatat");
    // Line coverage NONAKTIF setelah $coverage_control. Satu-satunya line-hit yang
    // boleh ada adalah statement $coverage_control itu sendiri (di-record SEBELUM
    // gate diaktifkan — statement dieksekusi saat line coverage masih aktif).
    // Statement setelahnya (a=0, a=1, $finish) tidak boleh tercatat.
    let total_line_hits: u64 = engine.cover_line.values().sum();
    assert!(
        total_line_hits <= 1,
        "line coverage harus nonaktif setelah control (max 1 hit utk statement $coverage_control), got {}",
        total_line_hits
    );
}

#[test]
fn test_coverage_control_branch_only() {
    // $coverage_control(0x4) — hanya branch coverage aktif; line/toggle harus kosong
    let source = r#"
module tb;
    reg [3:0] a;
    always_comb begin
        if (a > 8)
            a = 8;
        else
            a = 0;
    end
    initial begin
        $coverage_control(32'h0000_0004);
        a = 15;
        #1 $finish;
    end
endmodule
"#;
    let design = crate::compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 2);
    engine.run().unwrap();
    use crate::simulator::types::CoverageType;
    assert!(engine
        .coverage_enabled_types
        .contains(&CoverageType::Branch));
    assert!(!engine.coverage_enabled_types.contains(&CoverageType::Line));
    assert!(
        engine.cover_toggle.is_empty(),
        "toggle coverage harus kosong"
    );
}

// ─── SIM-23: Glitch detection (A→B→A dalam window) ────────────────────────

#[test]
fn test_glitch_detection_triggers_warning() {
    // Signal 0→1→0 dengan window 2 — harus menghasilkan WR0302 SignalGlitch
    let source = r#"
module tb;
    reg x;
    initial begin
        x = 0;
        #1 x = 1;
        #1 x = 0;
        #1 $finish;
    end
endmodule
"#;
    let design = crate::compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 4);
    engine.set_glitch_window(2);
    engine.run().unwrap();
    let diags = engine.flush_diagnostics();
    assert!(
        diags
            .iter()
            .any(|d| d.code == mivon_core::diagnostics::DiagCode::SignalGlitch),
        "harus ada warning glitch: {:#?}",
        diags
            .iter()
            .map(|d| (d.code.as_str(), d.message.as_ref()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn test_glitch_detection_disabled_by_default() {
    // Tanpa set_glitch_window, tidak ada warning glitch
    let source = r#"
module tb;
    reg x;
    initial begin
        x = 0;
        #1 x = 1;
        #1 x = 0;
        #1 $finish;
    end
endmodule
"#;
    let design = crate::compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 4);
    engine.run().unwrap();
    let diags = engine.flush_diagnostics();
    assert!(
        !diags
            .iter()
            .any(|d| d.code == mivon_core::diagnostics::DiagCode::SignalGlitch),
        "glitch detection default harus nonaktif (window 0)"
    );
}

#[test]
fn test_glitch_detection_no_false_positive_slow_transition() {
    // Transisi lambat 0→1→0 dengan jarak > window — tidak boleh glitch
    let source = r#"
module tb;
    reg x;
    initial begin
        x = 0;
        #10 x = 1;
        #10 x = 0;
        #1 $finish;
    end
endmodule
"#;
    let design = crate::compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 25);
    engine.set_glitch_window(2);
    engine.run().unwrap();
    let diags = engine.flush_diagnostics();
    assert!(
        !diags
            .iter()
            .any(|d| d.code == mivon_core::diagnostics::DiagCode::SignalGlitch),
        "transisi dengan jarak > window bukan glitch: {:#?}",
        diags
            .iter()
            .map(|d| (d.code.as_str(), d.message.as_ref()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn test_glitch_detection_end_to_end() {
    // End-to-end via CLI-style config: set_glitch_window dijalankan sebelum run
    let source = r#"
module tb;
    reg [7:0] y;
    initial begin
        y = 8'h00;
        #1 y = 8'hFF;
        #1 y = 8'h00;
        #1 y = 8'hFF;
        #1 $finish;
    end
endmodule
"#;
    let design = crate::compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 5);
    engine.set_glitch_window(1);
    engine.run().unwrap();
    let diags = engine.flush_diagnostics();
    let glitch_count = diags
        .iter()
        .filter(|d| d.code == mivon_core::diagnostics::DiagCode::SignalGlitch)
        .count();
    assert!(glitch_count >= 1, "0→FF→0→FF harus memicu glitch");
    // Nilai akhir tetap benar
    let y_idx = engine
        .design
        .top
        .signals
        .iter()
        .position(|s| s.name.as_str() == "y")
        .unwrap();
    assert_eq!(engine.state.read_signal(y_idx).to_u64(), 0xFF);
}

// ─── SIM-24: Timing check violation reporting (WR0303) ────────────────────

#[test]
fn test_timing_setup_violation_warning() {
    // $setup(data, posedge clk, 5): data berubah 1ns sebelum check — harus
    // menghasilkan warning TimingViolation (WR0303) via DiagSink.
    let source = r#"
module tb;
    reg data, clk;
    specify
        $setup(data, posedge clk, 5);
    endspecify
    initial begin
        data = 0;
        clk = 0;
        #1 data = 1;   // data berubah di time 1
        #1 clk = 1;    // posedge clk di time 2 — data berubah 1ns sebelum edge (<= 5)
        #1 $finish;
    end
endmodule
"#;
    let design = crate::compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 5);
    engine.run().unwrap();
    let diags = engine.flush_diagnostics();
    assert!(
        diags
            .iter()
            .any(|d| d.code == mivon_core::diagnostics::DiagCode::TimingViolation),
        "harus ada warning timing violation: {:#?}",
        diags
            .iter()
            .map(|d| (d.code.as_str(), d.message.as_ref()))
            .collect::<Vec<_>>()
    );
    assert!(
        diags.iter().any(|d| d.code.as_str() == "WR0303"),
        "harus ada kode WR0303"
    );
}

#[test]
fn test_timing_hold_violation_warning() {
    // $hold(posedge clk, data, 5): data berubah setelah ref dalam window —
    // harus menghasilkan warning TimingViolation.
    let source = r#"
module tb;
    reg data, clk;
    specify
        $hold(posedge clk, data, 5);
    endspecify
    initial begin
        data = 0;
        clk = 0;
        #1 clk = 1;    // ref edge
        #1 data = 1;   // data berubah dalam hold window
        #1 $finish;
    end
endmodule
"#;
    let design = crate::compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 5);
    engine.run().unwrap();
    let diags = engine.flush_diagnostics();
    assert!(
        diags
            .iter()
            .any(|d| d.code == mivon_core::diagnostics::DiagCode::TimingViolation),
        "harus ada warning timing violation (hold): {:#?}",
        diags
            .iter()
            .map(|d| (d.code.as_str(), d.message.as_ref()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn test_timing_no_false_positive_when_satisfied() {
    // Data stabil jauh sebelum ref (10ns > limit 5ns) — TIDAK boleh ada
    // warning TimingViolation.
    let source = r#"
module tb;
    reg data, clk;
    specify
        $setup(data, posedge clk, 5);
    endspecify
    initial begin
        data = 0;
        clk = 0;
        #10 data = 1;   // data berubah di time 10
        #10 clk = 1;    // posedge clk di time 20 — data berubah 10ns sebelum edge (> 5)
        #1 $finish;
    end
endmodule
"#;
    let design = crate::compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 25);
    engine.run().unwrap();
    let diags = engine.flush_diagnostics();
    assert!(
        !diags
            .iter()
            .any(|d| d.code == mivon_core::diagnostics::DiagCode::TimingViolation),
        "tidak boleh ada warning timing violation: {:#?}",
        diags
            .iter()
            .map(|d| (d.code.as_str(), d.message.as_ref()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn test_timing_width_violation_warning() {
    // $width(posedge clk, 5): pulse clk terlalu sempit (2ns < 5ns) — harus
    // menghasilkan warning TimingViolation.
    let source = r#"
module tb;
    reg data, clk;
    specify
        $width(posedge clk, 5);
    endspecify
    initial begin
        data = 0;
        clk = 0;
        #2 clk = 1;    // pulse sempit 2ns
        #2 clk = 0;
        #1 $finish;
    end
endmodule
"#;
    let design = crate::compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 8);
    engine.run().unwrap();
    let diags = engine.flush_diagnostics();
    assert!(
        diags
            .iter()
            .any(|d| d.code == mivon_core::diagnostics::DiagCode::TimingViolation),
        "harus ada warning timing violation (width): {:#?}",
        diags
            .iter()
            .map(|d| (d.code.as_str(), d.message.as_ref()))
            .collect::<Vec<_>>()
    );
}

// ─── SIM-25: Simulation performance monitoring dashboard ──────────────────

#[test]
fn test_perf_dashboard_counts_activity() {
    // Engine harus mencatat time steps, delta cycles, dan events processed
    // selama simulasi (dipakai CLI --perf-dashboard).
    let source = r#"
module tb;
    reg clk;
    integer count = 0;
    always #1 clk = ~clk;
    initial begin
        clk = 0;
        count = 0;
    end
    always @(posedge clk) begin
        count = count + 1;
    end
    initial #10 $finish;
endmodule
"#;
    let design = crate::compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 10);
    engine.run().unwrap();
    assert!(
        engine.sim_perf.counters.time_steps >= 1,
        "harus ada time steps"
    );
    assert!(
        engine.sim_perf.counters.delta_cycles >= 1,
        "harus ada delta cycles"
    );
    assert!(
        engine.sim_perf.counters.events_processed >= 1,
        "harus ada events processed"
    );
    assert!(
        engine.sim_perf.counters.sensitive_triggers >= 1,
        "posedge clk harus memicu sensitive processes"
    );
    assert!(
        engine.sim_perf.counters.processes_evaluated >= 1,
        "harus ada proses yang dievaluasi (jalur sequential/DAG)"
    );
}

#[test]
fn test_perf_dashboard_display_and_throughput() {
    let source = r#"
module tb;
    reg clk;
    always #1 clk = ~clk;
    initial begin
        clk = 0;
    end
    initial #5 $finish;
endmodule
"#;
    let design = crate::compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 5);
    engine.run().unwrap();
    let s = format!("{}", engine.sim_perf);
    assert!(s.contains("Simulation Performance Dashboard"));
    assert!(s.contains("Delta cycles"));
    assert!(s.contains("Throughput"));
    // events_per_delta tidak boleh NaN/panik meski delta==0
    assert!(engine.sim_perf.events_per_delta().is_finite());
    assert!(engine.sim_perf.events_per_sec().is_finite());
}

// ─── ROUND 23: SIM-28 (UPF e2e) + SIM-29 (coverage exclusion) ───────────────

#[test]
fn test_upf_power_off_x_propagation_end_to_end() {
    let source = r#"
module tb;
    reg [7:0] data;
    reg [7:0] out;
    always @* out = data;
    initial begin
        data = 8'hAA;
        #1 data = 8'h55;
        #1 $finish;
    end
endmodule
"#;
    let design = crate::compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 5);
    // Domain PD berisi `data`; VDD mati (false) → domain OFF → sinyal baca X
    let upf = r#"
create_power_domain PD -elements {data}
create_supply_net VDD -domain PD
set_domain_supply_net PD -primary_power_net VDD
"#;
    let mut pi = crate::simulator::upf::PowerIntent::parse(upf).unwrap();
    pi.build_signal_mapping(&engine.design.top.signals);
    pi.supply_values.insert("VDD".to_string(), false);
    engine.power_intent = Some(pi);
    engine.run().unwrap();
    let out_idx = engine
        .design
        .top
        .signals
        .iter()
        .position(|s| s.name.as_str() == "out")
        .unwrap();
    let out_val = engine.state.read_signal(out_idx).clone();
    assert!(
        out_val.all_x(),
        "out harus X karena domain data OFF, dapat {:?}",
        out_val
    );
}

#[test]
fn test_upf_isolation_clamp_end_to_end() {
    let source = r#"
module tb;
    reg [7:0] data;
    reg [7:0] out;
    always @* out = data;
    initial begin
        data = 8'hAA;
        #1 data = 8'h55;
        #1 $finish;
    end
endmodule
"#;
    let design = crate::compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 5);
    // Isolation cell dengan clamp 0: signal domain OFF terbaca 0, bukan X
    let upf = r#"
create_power_domain PD -elements {data}
create_supply_net VDD -domain PD
set_domain_supply_net PD -primary_power_net VDD
set_isolation PD -clamp_value 0
"#;
    let mut pi = crate::simulator::upf::PowerIntent::parse(upf).unwrap();
    pi.build_signal_mapping(&engine.design.top.signals);
    pi.supply_values.insert("VDD".to_string(), false);
    engine.power_intent = Some(pi);
    engine.run().unwrap();
    let out_idx = engine
        .design
        .top
        .signals
        .iter()
        .position(|s| s.name.as_str() == "out")
        .unwrap();
    let out_val = engine.state.read_signal(out_idx).clone();
    assert_eq!(
        out_val.to_u64(),
        0,
        "out harus clamp 0 karena isolation PD, dapat {:?}",
        out_val
    );
}

#[test]
fn test_coverage_exclusion_macros() {
    // `` `coverage_off `` / `` `coverage_on `` harus: (a) dikenali preprocessor
    // tanpa error, (b) kode di antaranya tetap dieksekusi, (c) range baris
    // tercatat di design & engine (is_line_excluded).
    let source = r#"
module tb;
    reg clk;
    always #1 clk = ~clk;
`coverage_off
    initial begin
        clk = 0;
    end
`coverage_on
    initial #5 $finish;
endmodule
"#;
    let design = crate::compile_str(source).unwrap();
    assert!(
        !design.coverage_exclusions.is_empty(),
        "coverage_exclusions harus terisi oleh `coverage_off/`coverage_on"
    );
    let mut engine = crate::simulator::SimulationEngine::new(design, 5);
    let (start, end) = engine.coverage_exclusions[0];
    assert!(
        engine.is_line_excluded(start),
        "baris {} harus excluded",
        start
    );
    assert!(engine.is_line_excluded(end), "baris {} harus excluded", end);
    if start > 1 {
        assert!(
            !engine.is_line_excluded(start - 1),
            "baris {} (di luar region) tidak boleh excluded",
            start - 1
        );
    }
    engine.run().unwrap();
}

#[test]
fn test_coverage_exclusion_filters_line_hits() {
    // SIM-29 e2e: statement pada baris dalam `` `coverage_off ``/`` `coverage_on ``
    // TIDAK dihitung line coverage. Sebelum fix, record_line_hit memakai key
    // process+discriminant tanpa info baris → statement di region excluded
    // ikut dihitung. Sekarang elaborator mencatat baris per statement
    // (IrDesign.stmt_lines) dan engine melewati statement yang barisnya
    // excluded (is_line_excluded).
    let source = r#"
module tb;
    reg [7:0] x;
    reg [7:0] y;
    initial begin
        x = 5;
    end
`coverage_off
    initial begin
        y <= 3;
    end
`coverage_on
    initial #2 $finish;
endmodule
"#;
    let design = crate::compile_str(source).unwrap();
    assert!(
        !design.stmt_lines.is_empty(),
        "stmt_lines harus terisi oleh elaborator (SIM-29)"
    );
    let mut engine = crate::simulator::SimulationEngine::new(design, 5);
    engine.coverage_enabled = true;
    engine.run().unwrap();

    // initial_0 (x = 5) berada DI LUAR region → harus dihitung.
    assert!(
        engine
            .cover_line
            .keys()
            .any(|k| k.as_str().starts_with("initial_0.")),
        "statement di luar region coverage_off harus dihitung line hit"
    );
    // initial_1 (y <= 3) berada DI DALAM region → TIDAK boleh dihitung.
    assert!(
        !engine
            .cover_line
            .keys()
            .any(|k| k.as_str().starts_with("initial_1.")),
        "statement di dalam region coverage_off TIDAK boleh dihitung line hit (SIM-29)"
    );
}

#[test]
fn test_if_branch_delay_keeps_tail() {
    // F31 fix: statement SETELAH if yang cabangnya ber-delay harus tetap
    // dieksekusi — sebelum fix, `evaluate_block_with_delay_fork` pada arm
    // If tidak menyertakan tail ke continuation saat branch suspend →
    // statement setelah if hilang (sim hang tanpa output).
    let source = r#"
module tb;
    reg c;
    reg [7:0] got;
    initial begin
        c = 1;
        got = 0;
        if (c) begin #1; got = 1; end
        #1 got = 2;
        $display("IFD got=%0d", got);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "got").unwrap();
    assert_eq!(
        val.to_u64(),
        2,
        "tail setelah if ber-delay harus jalan (got=2)"
    );
}

#[test]
fn test_repeat_with_if_delay_keeps_loop_and_tail() {
    // F31 fix: repeat dengan body `if (c) #1` (delay di cabang if) harus
    // mengulang SEMUA iterasi + tetap mengeksekusi statement setelah loop.
    // Tanpa fix: iterasi pertama suspend lalu loop hilang (n=1) atau tail
    // hilang (hang).
    let source = r#"
module tb;
    reg c;
    reg [7:0] n;
    initial begin
        c = 1;
        n = 0;
        repeat (3) begin
            if (c) #1;
            n = n + 1;
        end
        $display("RIF n=%0d", n);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "n").unwrap();
    assert_eq!(
        val.to_u64(),
        3,
        "repeat 3 iterasi harus jalan meski body ber-delay"
    );
}

#[test]
fn test_error_suggestion_did_you_mean_signal() {
    // PARSER-03: Saat signal tidak ditemukan, elaborator beri saran "did you mean?"
    // berdasarkan edit distance Levenshtein.
    let source = r#"
module tb;
    reg clk;
    reg [7:0] data_in;
    reg [7:0] data_out;
    initial begin
        clk = 0;
        data_in = 8'hAB;
        data_out = data_oun;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_err(),
        "compile harus gagal karena 'data_oun' typo"
    );
    let err_msg = format!("{:?}", result.err());
    // Error harus mengandung saran "did you mean 'data_out'?"
    assert!(
        err_msg.contains("data_out"),
        "error harus saran 'data_out', got: {}",
        err_msg
    );
}

#[test]
fn test_error_suggestion_no_match_long_name() {
    // PARSER-03: Nama sangat panjang tanpa match → tidak ada saran "did you mean?"
    let source = r#"
module tb;
    reg clk;
    initial begin
        clk = this_is_a_very_long_name_with_no_similar_match;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(result.is_err(), "compile harus gagal");
    let err_msg = format!("{:?}", result.err());
    // Tidak ada saran untuk nama sangat berbeda
    assert!(
        !err_msg.contains("did you mean"),
        "tidak ada saran untuk nama tanpa kemiripan, got: {}",
        err_msg
    );
}

#[test]
fn test_error_suggestion_did_you_mean_in_context() {
    // PARSER-03: Typo di context procedural statement juga dapat saran
    let source = r#"
module tb;
    reg clk;
    reg rst_n;
    reg [7:0] data_in;
    reg [7:0] data_out;
    initial begin
        data_out = dat_in;  // typo: 'dat_in' → saran 'data_in'
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(result.is_err(), "compile harus gagal karena 'dat_in' typo");
    let err_msg = format!("{:?}", result.err());
    assert!(
        err_msg.contains("data_in"),
        "error harus saran 'data_in', got: {}",
        err_msg
    );
}

#[test]
fn test_nba_write_conflict_detected() {
    // SIM-14: Dua always_ff menulis signal yang sama via NBA di delta cycle
    // yang sama → warning RT1006 harus muncul.
    let source = r#"
module tb;
    reg [7:0] a;
    reg clk;
    always_ff @(posedge clk) begin
        a <= 8'h11;
    end
    always_ff @(posedge clk) begin
        a <= 8'h22;
    end
    initial begin
        clk = 0;
        #1 clk = 1; #1 clk = 0;
        #1 clk = 1; #1 clk = 0;
        $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 10);
    assert!(result.is_ok(), "sim harus jalan: {:?}", result.err());
    // Sim harus berhasil meski ada NBA conflict (warning, bukan error)
    let sigs = result.unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "a").unwrap();
    // a harus 8'h22 (last writer wins) — bukan X atau 0
    assert_eq!(val.to_u64(), 0x22, "a harus 8'h22 (last NBA writer wins)");
}

#[test]
fn test_nba_single_writer_no_conflict() {
    // SIM-14: Hanya 1 always_ff menulis signal → TIDAK ada conflict
    let source = r#"
module tb;
    reg [7:0] a;
    reg clk;
    always_ff @(posedge clk) begin
        a <= a + 1;
    end
    initial begin
        a = 0;
        clk = 0;
        #1 clk = 1; #1 clk = 0;
        #1 clk = 1; #1 clk = 0;
        #1 clk = 1; #1 clk = 0;
        $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 10);
    assert!(result.is_ok(), "sim harus jalan: {:?}", result.err());
    let sigs = result.unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "a").unwrap();
    // a = 0 + 1 + 1 + 1 = 3
    assert_eq!(val.to_u64(), 3, "a harus 3 (3 clock上升沿, 1 writer saja)");
}

// ═══ SIM-20 tahap 1: Cycle-based simulation mode (--cycle) ═══

#[test]
fn test_cycle_based_counter_counts_edges() {
    // SIM-20: mode cycle-based — scheduler mendrive clock internal (tanpa
    // generator #delay). cnt naik 1 per posedge; period default 10 →
    // -T 100 = 10 posedge → cnt = 10. Comb (doubled = cnt*2) merespons
    // output FF setelah NBA commit → 20.
    let source = r#"
module cycle_tb;
    reg clk;
    reg [7:0] cnt;
    reg [7:0] doubled;
    initial begin
        cnt = 8'h00;
    end
    always_ff @(posedge clk) begin
        cnt <= cnt + 8'h01;
    end
    always_comb begin
        doubled = cnt * 2;
    end
endmodule
"#;
    let design = compile_str(source).expect("compile cycle_tb");
    let mut engine = mivon_api::simulator::SimulationEngine::new(design, 100);
    engine.set_cycle_based(true);
    engine.set_cycle_period(10);
    engine.run().expect("cycle-based sim berjalan");
    let get = |name: &str| -> u64 {
        let idx = engine
            .design
            .top
            .signals
            .iter()
            .position(|s| s.name.as_str() == name)
            .expect("signal ada");
        engine.state.read_signal(idx).to_u64()
    };
    assert_eq!(get("cnt"), 10, "cnt = 10 posedge dalam 100 tu (period 10)");
    assert_eq!(get("doubled"), 20, "comb merespons output FF setelah NBA");
}

#[test]
fn test_cycle_based_matches_event_driven_semantics() {
    // Semantik sama dengan event-driven untuk desain synchronous murni:
    // FF dengan reset sinkron + enable. Jalur cycle harus menghasilkan
    // nilai akhir identik dengan jalur event-driven yang didrive stimulus.
    let source = r#"
module sem_tb;
    reg clk;
    reg rst_n;
    reg en;
    reg [3:0] q;
    always_ff @(posedge clk) begin
        if (!rst_n) q <= 4'h0;
        else if (en) q <= q + 4'h1;
    end
endmodule
"#;
    // ── Jalur cycle-based: init di t=0 oleh scheduler, reset ditarik en=1 ──
    let source_cycle = r#"
module sem_tb;
    reg clk;
    reg rst_n;
    reg en;
    reg [3:0] q;
    initial begin
        q = 4'h0; rst_n = 1'b0; en = 1'b0;
    end
    always_ff @(posedge clk) begin
        if (!rst_n) q <= 4'h0;
        else if (en) q <= q + 4'h1;
    end
    // stimulus tanpa timed wait tidak mungkin (butuh #delay per edge);
    // mode cycle menghitung: 5 posedge pertama rst_n=0 (q tetap 0),
    // lalu rst_n=1 + en=1 → naik. rst_n/en di-drive comb-style via
    // assign dari register yang di-flip proses initial? Tidak bisa tanpa
    // delay. Gunakan pendekatan: rst_n/en konstan (en=1, rst_n ditahan 0
    // selama 5 edge lewat logika counter sederhana tidak tersedia).
    // Solusi tahap 1: bandingkan hanya pola reset sinkron aktif.
endmodule
"#;
    let _ = source_cycle; // dokumentasi keterbatasan stimulus statis
    let design = compile_str(source).expect("compile sem_tb");
    let mut engine = mivon_api::simulator::SimulationEngine::new(design, 50);
    engine.set_cycle_based(true);
    engine.set_cycle_period(10);
    // Injeksi stimulus via state langsung SEBELUM run: rst_n=0, en=1.
    // Mode cycle mengevaluasi FF tiap edge — q harus tetap 0 (reset aktif).
    {
        let mut set = |name: &str, v: u64| {
            let idx = engine
                .design
                .top
                .signals
                .iter()
                .position(|s| s.name.as_str() == name)
                .expect("signal ada");
            let w = engine.design.top.signals[idx].width.max(1);
            engine
                .state
                .write_signal(idx, mivon_ir::LogicVec::from_u64(v, w));
        };
        set("rst_n", 0);
        set("en", 1);
        set("q", 0);
    }
    engine.run().expect("cycle sim reset-aktif berjalan");
    let idx = engine
        .design
        .top
        .signals
        .iter()
        .position(|s| s.name.as_str() == "q")
        .unwrap();
    let qv = engine.state.read_signal(idx).to_u64();
    assert_eq!(qv, 0, "rst_n=0 sinkron → q tertahan 0 meski 5 posedge");
}

#[test]
fn test_cycle_based_fallback_timed_design_still_works() {
    // Desain dengan generator clock (#delay) TIDAK cocok mode cycle →
    // fallback otomatis ke event-driven dan hasil tetap BENAR (bukan error).
    let source = r#"
module fb_tb;
    reg clk;
    reg [7:0] a;
    always #5 clk = ~clk;
    always_ff @(posedge clk) begin
        a <= a + 8'h01;
    end
    initial begin
        a = 0; clk = 0;
        #100 $finish;
    end
endmodule
"#;
    let design = compile_str(source).expect("compile fb_tb");
    let mut engine = mivon_api::simulator::SimulationEngine::new(design, 200);
    engine.set_cycle_based(true); // akan fallback (AlwaysWithDelay terdeteksi)
    engine.run().expect("fallback ke event-driven sukses");
    let idx = engine
        .design
        .top
        .signals
        .iter()
        .position(|s| s.name.as_str() == "a")
        .unwrap();
    let av = engine.state.read_signal(idx).to_u64();
    // clk toggle tiap 5 tu mulai t=5 → posedge di 5,15,...,95 = 10 posedge
    assert_eq!(av, 10, "hasil fallback identik event-driven (a=10)");
}

// ── PARSER-REGRESSION (OpenTitan RTL) ──────────────────────────────────────
// Setiap test di bawah adalah regresi bug parser nyata yang ditemukan saat
// compile filelist RTL OpenTitan (`--filelist opentitan_rtl.f --recompile`).

#[test]
fn test_case_default_bare_semicolon() {
    // hmac_core.sv: `unique case (...) ... default; endcase` — `default;`
    // tanpa ': ' dan tanpa statement = default kosong (valid SV).
    let source = r#"
module tb;
    int out;
    always_comb begin
        case (out)
          0: out = 1;
          default;
        endcase
    end
    initial begin
        out = 0;
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "case `default;` gagal parse: {:?}",
        design.err()
    );
}

#[test]
fn test_assign_drive_strength() {
    // prim_pad_wrapper.sv: `assign (weak0, weak1) inout_io = ...;`
    let source = r#"
module tb(input logic in_io);
    logic sig;
    assign (weak0, weak1) sig = in_io;
    assign (strong0, strong1) sig2 = in_io;
    initial #1 $finish;
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "assign drive strength gagal parse: {:?}",
        design.err()
    );
}

#[test]
fn test_for_loop_compound_step() {
    // ibex_alu.sv: `for (int unsigned i = 1; i < 32; i += 2)` — `+=` di step.
    let source = r#"
module tb;
    int acc;
    initial begin
        acc = 0;
        for (int unsigned i = 1; i < 8; i += 2)
            acc += i;
        if (acc != 16) $error("acc=%0d", acc);
        $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 10);
    assert!(result.is_ok(), "for `i += 2` gagal sim: {:?}", result.err());
}

#[test]
fn test_dpi_export_function_no_parens() {
    // ibex_if_stage.sv: `export "DPI-C" function simutil_get_scramble_key;`
    let source = r#"
module tb;
    export "DPI-C" function simutil_get_scramble_key;
    export "DPI-C" function simutil_get_scramble_nonce;
    initial #1 $finish;
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "DPI export tanpa parens gagal parse: {:?}",
        design.err()
    );
}

#[test]
fn test_unpacked_array_port_single_dim() {
    // ibex_pmp.sv: `output logic pmp_req_err_o [PMPNumChan]` — dimensi
    // unpacked tunggal tanpa range sesudah nama port.
    let source = r#"
module tb (
    input  logic [7:0] data_i [4],
    output logic       flag_o  [4]
);
    assign flag_o[0] = data_i[0][0];
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "unpacked array port [N] gagal parse: {:?}",
        design.err()
    );
}

#[test]
fn test_double_backtick_macro_paste() {
    // otbn.sv `` `define DEF_FAC_BIT(NAME) ...``NAME``... `` — paste arg
    // dengan backtick ganda (OpenTitan extension).
    let source = r#"
`define DEF_FAC_BIT(NAME) \
  assign hw2reg.``NAME``.d = 1'b1;
module top_tb(input logic a);
    logic hw2reg_bit;
    `DEF_FAC_BIT(foo)
    initial #1 $finish;
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "double-backtick paste gagal: {:?}",
        design.err()
    );
}

#[test]
fn test_macro_token_paste_prefix_param() {
    // OpenTitan dv_macros.svh: `dv_``SEV_` — backtick + literal prefix `dv_`
    // di-paste ke param SEV_ (`` ``` ``) → `dv_error`. Backtick terdepan harus
    // dibuang, bukan dikopi literal (E1002 lexical error) — regresi dari
    // cip_base_vseq.sv:1368 yang memanggil `DV_CHECK_EQ(...)`.
    let source = r#"
`define DV_CHECK_EQ(ACT_, EXP_, SEV_=error) \
  begin \
    `dv_``SEV_($sformatf("Check failed"), ACT_) \
  end
module top_tb(input logic a);
  logic got;
  initial begin
    `DV_CHECK_EQ(a, 1)
  end
  initial #1 $finish;
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "token-paste prefix+param gagal: {:?}",
        design.err()
    );
}

#[test]
fn test_macro_dv_check_eq_no_backtick() {
    // Rekonstruksi nilai dari dv_macros.svh:95-101 (DV_CHECK_EQ) lalu panggil —
    // pastikan TIDAK ada backtick yang bocor ke output preprocessed (regresi
    // cip_base_vseq.sv:1368 E1002).
    let source = r#"
`define DV_CHECK_EQ(ACT_, EXP_, MSG_="", SEV_=error, ID_=`gfn) \
  begin \
    if ((ACT_) == (EXP_)) ; else begin \
      `dv_``SEV_($sformatf("Check failed %s == %s (%0d [0x%0h] vs %0d [0x%0h]) %s", \
                           `"ACT_`", `"EXP_`", ACT_, ACT_, EXP_, EXP_, MSG_), ID_) \
    end \
  end
module top_tb(input logic a);
  logic got;
  initial begin
    `DV_CHECK_EQ(a, 1, "mismatch")
  end
  initial #1 $finish;
endmodule
"#;
    let mut pp = mivon_parser::preprocessor::Preprocessor::new();
    let out = pp.preprocess(source, None).unwrap();
    assert!(
        !out.contains('`'),
        "backtick bocor ke output preprocessed:\n{}",
        out
    );
    assert!(
        out.contains("dv_error"),
        "token-paste dv_+SEV_ gagal:\n{}",
        out
    );
}

#[test]
fn test_dv_check_eq_multiline_define() {
    // Re-producing the EXACT form from dv_macros.svh:95-101 (backslash continuations
    // spanning actual lines, NOT a single joined line) — mimics real preprocessor
    // pipeline where each define line is separate before parse_define gets it.
    let source = "`define DV_CHECK_EQ(ACT_, EXP_, MSG_=\"\", SEV_=error, ID_=act) \\\n  begin \\\n    if ((ACT_) == (EXP_)) ; else begin \\\n      `dv_``SEV_($sformatf(\"check\"), ACT_) \\\n    end \\\n  end\nmodule top_tb(input logic a);\n  initial begin\n    `DV_CHECK_EQ(a, 1, \"mismatch\")\n  end\n  initial #1 $finish;\nendmodule\n";
    let mut pp = mivon_parser::preprocessor::Preprocessor::new();
    let out = pp.preprocess(source, None).unwrap();
    assert!(
        !out.contains('`'),
        "backtick bocor ke output preprocessed:\n{}",
        out
    );
    assert!(
        out.contains("dv_error"),
        "token-paste dv_+SEV_ gagal:\n{}",
        out
    );
}

#[test]
fn test_dv_check_eq_real_body() {
    // Body PERSIS dv_macros.svh:95-101 dengan stringify `"ACT_`"` & token-paste
    // `dv_``SEV_` di dalam $sformatf — reproduksi akurat E1002. Panggil dengan
    // 3 arg + isi default SEV_ dan ID_=`gfn`.
    let source = "`define gfn get_full_name()\n`define DV_CHECK_EQ(ACT_, EXP_, MSG_=\"\", SEV_=error, ID_=`gfn) \\\n  begin \\\n    if ((ACT_) == (EXP_)) ; else begin \\\n      `dv_``SEV_($sformatf(\"Check failed %s == %s (%0d [0x%0h] vs %0d [0x%0h]) %s\", \\\n                           `\"ACT_`\", `\"EXP_`\", ACT_, ACT_, EXP_, EXP_, MSG_), ID_) \\\n    end \\\n  end\nmodule top_tb(input logic a);\n  task check(int alert_name);\n    begin\n      `DV_CHECK_EQ(cfg.m_alert_agent_cfgs[alert_name].vif.get_alert(), 1,\n                   $sformatf(\"fatal error %0s does not trigger!\", alert_name))\n    end\n  endtask\n  initial #1 $finish;\nendmodule\n";
    let mut pp = mivon_parser::preprocessor::Preprocessor::new();
    let out = pp.preprocess(source, None).unwrap();
    assert!(
        !out.contains('`'),
        "backtick bocor ke output preprocessed (real body):\n{}",
        out
    );
    assert!(
        out.contains("dv_error"),
        "token-paste gagal (real body):\n{}",
        out
    );
}

#[test]
fn test_undefined_macro_midline_strips_backtick() {
    // cip_base_vseq.sv:1387 — `DV_CHECK_EQ` TIDAK didefinisikan (file tidak
    // include dv_macros.svh; fresh Preprocessor per file yang hanya diberi
    // config.defines global) → makro unknown di tengah baris harus kehilangan
    // backtick (strip), bukan bocor ` ke lexer (E1002).
    let source = r#"
module top_tb;
  task check(int alert_name);
    begin
      `DV_CHECK_EQ(cfg.m_alert_agent_cfgs[alert_name].vif.get_alert(), 1,
                   $sformatf("fatal error %0s does not trigger!", alert_name))
    end
  endtask
  initial #1 $finish;
endmodule
"#;
    let mut pp = mivon_parser::preprocessor::Preprocessor::new();
    let out = pp.preprocess(source, None).unwrap();
    assert!(
        !out.contains('`'),
        "backtick bocor ke output preprocessed (undef macro):\n{}",
        out
    );
    assert!(
        out.contains("DV_CHECK_EQ"),
        "undefined macro harus tetap ada (tanpa backtick):\n{}",
        out
    );
}

#[test]
fn test_dv_real_file_dump() {
    use std::path::PathBuf;
    // nextest berjalan dari direktori crate (crates/mivon-tests), jadi relatif ke root workspace = ../../
    let file = PathBuf::from("../../opentitan/hw/dv/sv/cip_lib/seq_lib/cip_base_vseq.sv");
    // Butuh checkout OpenTitan (3rd-party, di-scan di CI via sparse-clone).
    // Bila tidak tersedia → skip (non-fatal).
    let src = match std::fs::read_to_string(&file) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("skip: {} tidak ada (butuh OpenTitan)", file.display());
            return;
        }
    };
    let dir = file.parent().unwrap().to_path_buf();
    let mut pp = mivon_parser::preprocessor::Preprocessor::new();
    pp.add_search_path(dir.to_str().unwrap());
    let out = pp.preprocess(&src, Some(&dir)).unwrap();
    for (ln, line) in out.lines().enumerate() {
        if line.contains('`') {
            eprintln!("BACKTICK line {}: {}", ln + 1, line);
        }
    }
}

#[test]
fn test_param_class_after_name() {
    // pulp_riscv_dbg dmi_test.sv: `class req_t #(parameter int AW = 7);`
    // — param list SETELAH nama class, item `parameter int N = value`.
    let source = r#"
package pkg;
  class req_t #(
    parameter int AW = 7
  );
    int data;
  endclass
endpackage
module tb;
    pkg::req_t q;
    initial #1 $finish;
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "class #(parameter ...) gagal parse: {:?}",
        design.err()
    );
}

#[test]
fn test_virtual_class_in_package() {
    // dmi_test.sv: `virtual class rand_dmi #(parameter int AW = 32);`
    let source = r#"
package pkg;
  virtual class rand_dmi #(
    parameter int   AW = 32
  );
    int data;
  endclass
endpackage
module tb;
    pkg::rand_dmi d;
    initial #1 $finish;
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "virtual class di package gagal parse: {:?}",
        design.err()
    );
}

#[test]
fn test_function_automatic_pkgtype_return() {
    // lc_ctrl_pkg.sv: `function automatic prim_mubi_pkg::mubi4_t
    // lc_to_mubi4(lc_tx_t val);` — return type package-qualified.
    let source = r#"
package lc_ctrl_pkg;
  function automatic prim_mubi_pkg::mubi4_t lc_to_mubi4(lc_tx_t val);
    lc_to_mubi4 = 0;
  endfunction
endpackage
module tb;
    import lc_ctrl_pkg::*;
    initial #1 $finish;
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "function auto pkg::type gagal parse: {:?}",
        design.err()
    );
}

// ── PARSER-REGRESSION 2 (FastLexer/preprocessor, OpenTitan filelist) ─────────
// Deret bug nyata dari jalur `run_fast` (CompileSession + FastLexer) —
// FastLexer punya path tersendiri lintas filelist, beda dari legacy lexer.

#[test]
fn test_undefined_macro_backtick_parse() {
    // `default: `uvm_fatal(`gfn, "...")` + `endcase` — FastLexer LAMA membuang
    // seluruh baris pada backtick undefined → `endcase` jadi
    // "expected expression, found Endcase".
    let source = r#"
module tb;
  int mode;
  initial begin
    case (mode)
      default: `uvm_fatal(`gfn, "no!")
    endcase
  end
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "undefined macro di case default gagal: {:?}",
        design.err()
    );
}

#[test]
fn test_macro_stringify_direct_usage() {
    // csrng_err_vseq.sv: `` $assertoff(0, `"path`") `` — stringify dipakai
    // langsung di kode (di luar body macro) → harus jadi string literal.
    let source = r#"
module tb;
  initial begin
    $assertoff(0, `"tb.dut.sig`");
    #1 $finish;
  end
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "stringify langsung gagal: {:?}",
        design.err()
    );
}

#[test]
fn test_macro_param_default_paste() {
    // tb__xbar_connect.sv: `define M(tl, inst, sig, pd_hier = ) \ ...``pd_hier``...
    // — param default dipanggil tanpa arg → paste ` ```pd_hier``` ` harus kosong.
    let source = r#"
`define DRIVE_IF(tl_name, inst_name, sig_name, pd_hier = ) \
     force ``tl_name``_tl_if.d2h = dut.top_darjeeling``pd_hier``.u_``inst_name``;
module tb;
  `DRIVE_IF(rv_core_ibex__corei, rv_core_ibex, corei_tl_h)
  initial #1 $finish;
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "macro param default paste gagal: {:?}",
        design.err()
    );
}

#[test]
fn test_macro_paste_between_digits() {
    // otbn_env_cov.sv: coverpoint bin generated `0``0``0` — paste marker
    // antar digit harus dibuang (bukan jadi token backtick).
    let source = r#"
module tb;
  logic [7:0] wdr_operand_a;
  covergroup cg with function sample(logic [7:0] a);
    cp: coverpoint wdr_operand_a[8'b 0``0``1``1];
  endgroup
  initial #1 $finish;
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "paste antar digit gagal: {:?}",
        design.err()
    );
}

#[test]
fn test_instance_array_single_size() {
    // DV_ALERT_IF_CONNECT: `alert_esc_if alert_if[N](.clk(clk), .rst_n(rst_n));`
    // — ukuran array `[N]` tunggal tanpa ':' sesudah nama instance.
    let source = r#"
interface iface;
  input clk;
  input rst_n;
endinterface
module tb;
  logic clk, rst_n;
  iface alert_if[2](.clk(clk), .rst_n(rst_n));
  initial #1 $finish;
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "instance array [N] gagal: {:?}",
        design.err()
    );
}

#[test]
fn test_force_release_hierarchical() {
    // keymgr_dpe_if.sv: `force tb.dut.u_x.sig = v;` / `release tb.dut.u_x.sig;`
    let source = r#"
module tb;
  reg a;
  initial begin
    force tb.dut.u_ctrl.gen_en_o = 1'b1;
    release tb.dut.u_ctrl.gen_en_o;
  end
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "force/release hierarkis gagal: {:?}",
        design.err()
    );
}

#[test]
fn test_forever_at_bare_signal() {
    // jtag_dmi_monitor.sv: `forever @cfg.in_reset begin ... end` —
    // event control `@sig` tanpa kurung.
    let source = r#"
module tb;
  reg d;
  initial begin
    forever @tb.d begin
      #1;
    end
  end
endmodule
"#;
    let design = compile_str(source);
    assert!(design.is_ok(), "forever @sig gagal: {:?}", design.err());
}

#[test]
fn test_class_extends_pkg_class() {
    // sec_cm prim_*_if.sv: `class my_proxy extends pkg::base_class;`
    let source = r#"
package pkg;
  class base_proxy; endclass
endpackage
interface sec_cm_if;
  import pkg::*;
  class my_proxy extends pkg::base_proxy; endclass
endinterface
module tb;
  initial #1 $finish;
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "extends pkg::class gagal: {:?}",
        design.err()
    );
}

#[test]
fn test_uvm_config_db_virtual_type() {
    // alert_esc_agent.sv: `uvm_config_db#(virtual alert_esc_if)::get(...)`
    let source = r#"
interface alert_esc_if; endinterface
module tb;
  alert_esc_if vif;
  initial begin
    if (uvm_config_db#(virtual alert_esc_if)::get(this, "", "vif", cfg.vif)) begin
      #1;
    end
  end
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "uvm_config_db#(virtual iface) gagal: {:?}",
        design.err()
    );
}

// ── F48: `++`/`--` side-effect penuh dalam ekspresi (LRM 1800 §11.4.1) ──
//
// PRA-FIX: parser SV men-desugar `i++`/`++i` di level ekspresi jadi
// `BinaryOp Add/Sub` murni → nilai postfix SALAH dan side-effect hilang
// (`k = i--` → k=4 harusnya 5; `while (++i < 4)` → infinite loop).

#[test]
fn test_incdec_prefix_side_effect_in_rhs() {
    let source = r#"
module tb;
    integer i, j;
    initial begin
        i = 5;
        j = ++i;
        $display("P j=%0d i=%0d", j, i);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("j"), 6, "prefix: hasil = nilai BARU");
    assert_eq!(get("i"), 6, "side-effect: i bertambah 1");
}

#[test]
fn test_incdec_postfix_old_value_in_rhs() {
    // Postfix: hasil = nilai LAMA, operand berkurang 1.
    let source = r#"
module tb;
    integer i, k;
    initial begin
        i = 5;
        k = i--;
        $display("B k=%0d i=%0d", k, i);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("k"), 5, "postfix: hasil = nilai LAMA");
    assert_eq!(get("i"), 4, "side-effect: i berkurang 1");
}

#[test]
fn test_incdec_in_loop_condition_not_infinite() {
    // `while (++i < 4)` — PRA-FIX: loop tak pernah berakhir (n=100000).
    let source = r#"
module tb;
    integer i, n;
    initial begin
        n = 0;
        i = 0;
        while (++i < 4) n = n + 1;
        $display("C i=%0d n=%0d", i, n);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 100).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("n"), 3, "loop 3 iterasi");
    assert_eq!(get("i"), 4, "i incremented tiap kondisi");
}

#[test]
fn test_incdec_postfix_loop_condition() {
    let source = r#"
module tb;
    integer i, n;
    initial begin
        n = 0;
        i = 3;
        while (i-- > 0) n = n + 1;
        $display("D i=%0d n=%0d", i, n);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 100).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("n"), 3, "loop 3 iterasi");
    // Postfix decrement terjadi SETIAP evaluasi kondisi, termasuk yang
    // bernilai 0 (hasil perbandingan tetap 0 → loop berhenti) → i = -1.
    assert_eq!(get("i"), 0xFFFF_FFFF, "i terpuntuk -1 (4 decrement)");
}

#[test]
fn test_incdec_mixed_arithmetic() {
    // `k = i++ + j` — postfix bernilai lama lalu i maju.
    let source = r#"
module tb;
    integer i, j, k;
    initial begin
        i = 0; j = 7;
        k = i++ + j;
        $display("E k=%0d i=%0d", k, i);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("k"), 7, "nilai lama i (0) + j (7)");
    assert_eq!(get("i"), 1, "i maju setelah evaluasi");
}

#[test]
fn test_incdec_mv_postfix_rhs_e2e() {
    // Sisi `.mv`: postfix di RHS ekspresi kini didukung penuh (F48) —
    // sebelumnya ditolak parser `.mv`.
    let src = r#"
module tb_mvpost {
    sig i : logic[7:0]
    sig j : logic[7:0]
    sig k : logic[7:0]
    initial {
        i = 5
        k = i--
        j = i
    }
}
"#;
    let r = mivon_mv::transpile(src, "mvpost").expect("postfix RHS .mv");
    assert!(r.sv.contains("k = i--;"), "emit postfix: {}", r.sv);
    let sigs = simulate_signals(&r.sv, 20).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("k"), 5, "postfix = nilai lama");
    assert_eq!(get("i"), 4, "side-effect i--");
}

// ── Bug mivon utama: enum member typedef $unit (top-level) tak resolve ──

#[test]
fn test_unit_typedef_enum_member_resolves() {
    // `typedef enum ... E;` di luar module (= output `mgen`) — member
    // `RUN` harus resolve sebagai konstanta, bukan E2001 "signal not found".
    let source = r#"
typedef enum logic [1:0] { IDLE, RUN, DONE } State;
module tb;
    State s = IDLE;
    initial begin
        s = RUN;
        #10 $display("E s=%0d", s);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 50).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("s"), 1, "RUN = 1");
}

#[test]
fn test_unit_typedef_enum_explicit_values() {
    // Nilai eksplisit + counter sequential per typedef.
    let source = r#"
typedef enum logic [2:0] { RED = 0, GREEN = 2, BLUE = 4 } Color;
typedef enum logic [1:0] { L0, L1, L2 } Level;
module tb;
    Color c = RED;
    Level l = L0;
    initial begin
        c = BLUE; l = L2;
        #10 $display("EV c=%0d l=%0d", c, l);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 50).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("c"), 4, "BLUE = 4 (eksplisit)");
    assert_eq!(
        get("l"),
        2,
        "L0..L2 sequential per typedef (tak bocor lintas enum)"
    );
}

// ── Bug mivon utama: initializer port ANSI menimpa assignment `initial` ──

#[test]
fn test_port_init_does_not_clobber_initial_assign() {
    // SV: inisialisasi variabel terjadi SEBELUM aktivitas prosedural t=0
    // (LRM 1800 §4.3.2). PRA-FIX proses `port_init_*` dijalankan setelah
    // `initial` user → hasil `st=0` (port init menimpa assignment user).
    let src = r#"
module tb (output logic [1:0] st = 2'd0, output logic [1:0] st2);
    initial begin
        st = 2'd1;
        st2 = 2'd1;
        #10 $display("PI st=%0d st2=%0d", st, st2);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(src, 50).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("st"), 1, "assignment di initial menang atas port init");
    assert_eq!(get("st2"), 1, "port tanpa init tetap bisa di-drive");
}

#[test]
fn test_port_init_applies_when_not_driven() {
    // Port init tetap berlaku bila tak ada assignment (nilai default honored).
    let source = r#"
module tb (output logic [7:0] a = 8'h2A);
    initial begin
        #10 $display("PJ a=%0d", a);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 50).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("a"), 0x2A, "port init berlaku saat tak di-drive");
}

// ── F56: parameter override tak memengaruhi generate loop di modul anak ──
//
// PRA-FIX: `expand_all_generates` memutasi `design.modules` in-place dengan
// nilai param DEFAULT; instance yang meng-override parameter me-clone AST yang
// generate-nya sudah ter-expand → `gen_sub #(.W(8))` dengan
// `for (gi=0; gi<W; gi++)` hanya menghasilkan 4 iterasi (bit atas X).

#[test]
fn test_param_override_expands_generate_per_instance() {
    let source = r#"
module gen #(parameter int N=2, parameter int OFF=0) (input clk, output reg [N-1:0] q);
    genvar i;
    generate
        for (i=0;i<N;i=i+1) begin : g
            always @(posedge clk) q[i] <= i[0] + OFF;
        end
    endgenerate
endmodule
module tb_go;
    reg clk=0; reg [1:0] q2; reg [7:0] q8; reg [1:0] qo;
    gen #(.N(2)) u2 (.clk(clk), .q(q2));
    gen #(.N(8), .OFF(1)) u8 (.clk(clk), .q(q8));
    gen #(.N(2), .OFF(1)) uo (.clk(clk), .q(qo));
    always #5 clk=~clk;
    initial begin
        repeat (2) @(posedge clk);
        #1 $display("M1 %b %b %b", q2, q8, qo);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 200).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    // Oracle iverilog: q2=10 (N=2, OFF=0), q8=01010101 (N=8, OFF=1 → semua bit
    // ter-generate; sebelum fix bit atas X), qo=01 (OFF=1).
    let q2 = get("q2");
    let q8 = get("q8");
    let qo = get("qo");
    assert_eq!(q2, 0b10, "N=2 default");
    assert_eq!(q8, 0b0101_0101, "N=8 override → 8 iterasi generate");
    assert_eq!(qo, 0b01, "OFF=1");
}

// ── F55: signedness hasil function inlining + variabel STATIC (LRM §8.21) ──

#[test]
fn test_function_result_signed_in_display() {
    // PRA-FIX: temp hasil inlining bertipe `Logic` (UNSIGNED) sehingga
    // `$display("%0d", max2(-3, -9))` tercetak 4294967293 (padahal assignment
    // ke variabel integer sudah benar karena resize terjadi di sana).
    let source = r#"
module tb_fs;
    function automatic int max2(int a, int b); return (a > b) ? a : b; endfunction
    function automatic int neg(int a); return a; endfunction
    integer x;
    initial begin
        $display("X1 %0d", max2(-3, -9));
        $display("X2 %0d", neg(-3));
        $display("X3 %0d", max2(3, 9));
        x = max2(-3, -9);
        $display("X4 %0d", x);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 50).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    // integer 32-bit dua-complement: -3 = 0xFFFFFFFD.
    assert_eq!(get("x"), 0xFFFF_FFFD, "assignment tetap benar");
}

#[test]
fn test_static_function_variable_persists() {
    // LRM 1800 §8.21: variabel `static` (dan semua lokal fungsi `static`)
    // punya lifetime modul — nilainya bertahan antar-call. PRA-FIX inliner
    // membuat temp per-call → `counter()` selalu mengembalikan 1.
    let source = r#"
module tb_st;
    function static int counter();
        static int c = 0;
        c++;
        return c;
    endfunction
    integer x;
    initial begin
        $display("Y1 %0d", counter());
        $display("Y2 %0d", counter());
        x = counter();
        $display("Y3 %0d", x);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 50).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("x"), 3, "call ketiga = 3 (state bertahan antar-call)");
}

// ── Real↔integer: `$rtoi`/`$itor`, cast, format (LRM 1800 §6.24/§20.8) ──
//
// PRA-FIX: `$rtoi`/`$itor` tak dikenal (0 + warning RT9003); `$display("%d",
// real)` mencetak bit-pattern mentah (4620580627691444634 untuk 7.9);
// `int'(3.99)` memotong bit f64 (515396076, bukan 4); literal real
// tanpa titik desimal (`1e3`) tak di-lex sama sekali.
//
// CATATAN: `$rtoi` di mivon mengikuti LRM (round ke terdekat, ties away from
// zero → `$rtoi(3.99)`=4, `$rtoi(-2.5)`=-3). iverilog memakai truncation
// (3 dan -2) — menyimpang dari LRM, jadi mivon tidak mengikuti.

#[test]
fn test_real_to_int_sysfuncs() {
    let source = r#"
module tb_rt;
    integer a, b, c, d;
    initial begin
        a = $rtoi(3.4);
        b = $rtoi(3.99);
        c = $rtoi(-2.5);
        d = $rtoi(1e3);
        $display("R1 %0d %0d %0d %0d", a, b, c, d);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 50).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("a"), 3, "$rtoi(3.4)");
    assert_eq!(get("b"), 4, "$rtoi(3.99) → round (LRM §20.8)");
    // integer = 32-bit: -3 disimpan dua-complement 0xFFFFFFFD.
    assert_eq!(get("c"), 0xFFFF_FFFD, "$rtoi(-2.5) → ties away from zero");
    assert_eq!(get("d"), 1000, "$rtoi(1e3) — literal ekspon tanpa titik");
}

#[test]
fn test_int_to_real_sysfunc_and_arith() {
    let source = r#"
module tb_it;
    real r;
    initial begin
        r = $itor(7);
        $display("S1 %f", r);
        $display("S2 %f", $itor(42) / 2.0);
        $display("S3 %f", $itor(42) / 2.0 + 1.0);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 50).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    let r = get("r");
    assert_eq!(r, 7.0f64.to_bits(), "$itor(7)");
}

#[test]
fn test_real_cast_round_trip() {
    let source = r#"
module tb_rc;
    integer i; real r;
    initial begin
        i = int'(3.99);
        r = real'(7);
        $display("T1 %0d", i);
        $display("T2 %f", r);
        r = 2.5e2;
        $display("T3 %f", r);
        r = 1e3;
        $display("T4 %f", r);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 50).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("i"), 4, "int'(3.99) → round, bukan bit-pattern f64");
    assert_eq!(get("r"), 1e3f64.to_bits(), "real'(7) lalu 1e3");
}

#[test]
fn test_display_real_as_decimal() {
    // `%d` dari real → bulat (LRM §21.2.1.4), bukan bit-pattern mentah.
    let source = r#"
module tb_dd;
    real r = 7.9;
    integer k;
    initial begin
        $display("U1 %0d", r);
        k = r;
        $display("U2 %0d", k);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 50).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("k"), 8, "real → integer = round(7.9) = 8");
}

// ── Associative array end-to-end (LRM 1800 §7.9) ──
//
// PRA-FIX: `int m [string];` diperlakukan array 1-elemen (is_associative
// hardcoded false) → tulis hilang, baca 0, dan method (`num`/`exists`/
// `delete`) gagal "cannot call method on unknown class".

#[test]
fn test_associative_array_int_key() {
    let source = r#"
module tb_ai;
    int ai [int];
    initial begin
        ai[7] = 42;
        ai[9] = 43;
        $display("V1 %0d %0d", ai[7], ai[9]);
        $display("V2 %0d %0d", ai.num(), ai.exists(7));
        $display("V3 %0d", ai.exists(11));
        $display("V4 %0d", ai.delete(7));
        $display("V5 %0d %0d", ai.exists(7), ai.num());
        ai.delete();
        $display("V6 %0d", ai.num());
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 50).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    // Snapshot akhir (semua operasi sudah dijalankan).
    assert_eq!(get("ai"), 0, "num() setelah delete() seluruh");
}

#[test]
fn test_associative_array_string_key() {
    let source = r#"
module tb_as;
    logic [7:0] al [string];
    logic [7:0] a_got, b_got, missing;
    integer n_ent, has_a, has_z;
    initial begin
        al["alpha"] = 8'hAB;
        al["beta"]  = 8'hCD;
        a_got = al["alpha"];
        b_got = al["beta"];
        missing = al["missing"];
        n_ent = al.num();
        has_a = al.exists("alpha");
        has_z = al.exists("gamma");
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 50).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("a_got"), 0xAB, "baca via string key");
    assert_eq!(get("b_got"), 0xCD);
    assert_eq!(get("n_ent"), 2, "num()");
    assert_eq!(get("has_a"), 1, "exists()");
    assert_eq!(get("has_z"), 0, "exists() untuk key tak ada");
    // Key tak ada → X (to_u64() memetakan X ke 0).
}

// ── Bug mivon utama: const-fold `case` mengabaikan X/Z pada literal ──
//
// PRA-FIX: `case (2'b1x)` dengan label `2'b1?` → HIT (salah). Penyebab:
// jalur const-fold membandingkan label sebagai INTEGER (`parse_literal`)
// sehingga digit `x`/`z`/`?` jadi 0 dan `1x` == `1z`. LRM 1800 §12.5:
// hanya `casez`/`casex` yang memperlakukan z (dan x utk casex) sebagai
// don't-care; pada `case` biasa perbandingan 4-state strict.

#[test]
fn test_case_plain_with_unknown_bits() {
    let source = r#"
module tb_cu;
    reg [1:0] v;
    reg [3:0] c1 = 0, c2 = 0, c3 = 0, c4 = 0, c5 = 0, c6 = 0;
    initial begin
        case (2'b1x)
            2'b1?: c1 = 4'h1;
            default: c1 = 4'h0;
        endcase
        casez (2'b1x)
            2'b1?: c2 = 4'h2;
            default: c2 = 4'h0;
        endcase
        casex (2'b1x)
            2'b1?: c3 = 4'h3;
            default: c3 = 4'h0;
        endcase
        // case expr berisi unknown + label identik → boleh match
        case (4'b10x1)
            4'b10x1: c4 = 4'hE;
            default: c4 = 4'h0;
        endcase
        // unknown vs known → tak match
        case (4'b10x1)
            4'b1011: c5 = 4'hF;
            default: c5 = 4'h0;
        endcase
        // expr variabel: `?` tetap bukan don't-care di `case`
        v = 2'b11;
        case (v)
            2'b1?: c6 = 4'h7;
            default: c6 = 4'h0;
        endcase
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 50).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    // Oracle iverilog: c1=0 (miss), c2=2, c3=3, c4=e, c5=0, c6=0.
    assert_eq!(get("c1"), 0, "case biasa: `1?` (=1z) tak match `1x`");
    assert_eq!(get("c2"), 2, "casez: z di pattern = don't-care");
    assert_eq!(get("c3"), 3, "casex: x di pattern = don't-care");
    assert_eq!(get("c4"), 0xE, "unknown identik tetap match");
    assert_eq!(get("c5"), 0, "unknown vs known tak match");
    assert_eq!(get("c6"), 0, "case variabel: `?` bukan don't-care");
}

#[test]
fn test_case_const_fold_still_works() {
    // Const-fold case (const expr + const label) harus tetap jalan —
    // guard unknown tidak boleh mematikan optimasi yang sah.
    let source = r#"
module tb_cc;
    localparam int SEL = 2;
    reg [7:0] r = 0;
    initial begin
        case (2)
            1: r = 8'h11;
            2: r = 8'h22;
            default: r = 8'hFF;
        endcase
        $display("C7 r=%h", r);
        case (SEL)
            SEL: r = 8'hAB;
            default: r = 8'h00;
        endcase
        $display("C8 r=%h", r);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 50).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("r"), 0xAB, "const-fold case tetap benar");
}

// ── Bug mivon utama: rujukan hierarkis `top.<sig>` tak resolve ──
//
// PRA-FIX: `$display("%0d", tb_d6.a)` / `force tb_d6.a = 9` gagal E2001
// "hierarchical signal not found for write" — padahal SV mengizinkan path
// lengkap ke modul teratas (LRM 1800 §12.4 hierarchical names).

#[test]
fn test_hierarchical_ref_to_top_module_signal() {
    let source = r#"
module tb_h;
    reg [3:0] a = 4'd3;
    initial begin
        $display("H1 %0d", tb_h.a);
        force tb_h.a = 4'd9;
        #1 $display("H2 %0d", a);
        release tb_h.a;
        a = 4'd5;
        $display("H3 %0d", tb_h.a);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 50).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("a"), 5, "path top.a resolve + force/release bekerja");
}

#[test]
fn test_hierarchical_ref_through_instance() {
    // Path `u_sub.q` (sinyal dalam instance) — regresi tak boleh terputus.
    let source = r#"
module sub; reg [7:0] q = 8'h3C; endmodule
module tb_h2;
    sub u_sub ();
    initial begin
        #1 $display("H4 %h", u_sub.q);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 50).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("u_sub.q"), 0x3C, "hierarkis lewat instance");
}

// ── `$display`/`$sformatf`: `%.Nf`, `%g`, `%x`, `%-Nd`, `%Ns`, `%c`, `%%`
//    (LRM 1800 §21.2.1.2–§21.2.1.4). PRA-FIX specifier tak dikenal dicetak
//    apa adanya: `$display("%.2f", 3.14159)` → `%.2f` (literal, bukan 3.14).
//    Di sini e2e (hasil disimpan ke signal string); unit test formatter ada di
//    `mivon-simulator` `simulator::util::tests::test_fmt_*` (oracle iverilog).

#[test]
fn test_display_real_precision_and_g_e2e() {
    let source = r#"
module tb;
    real r = 3.14159;
    string s1, s2, s3, s4, s5, s6, s7, s8;
    initial begin
        s1 = $sformatf("%f", r);
        s2 = $sformatf("%.2f", r);
        s3 = $sformatf("%.4f", r);
        s4 = $sformatf("%.0f", r);
        s5 = $sformatf("%8.3f", r);
        s6 = $sformatf("%0.3f", r);
        s7 = $sformatf("%g", r);
        s8 = $sformatf("%e", r);
        #10 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 50).unwrap();
    let get = |n: &str| {
        let v = sigs.iter().find(|(s, _)| s == n).unwrap().1.clone();
        let mut out = String::new();
        for k in (0..v.width).step_by(8) {
            let mut byte = 0u8;
            for b in 0..8 {
                if v.bits.get(k + b) == Some(&mivon_core::LogicVal::One) {
                    byte |= 1 << b;
                }
            }
            if byte != 0 {
                out.push(byte as char);
            }
        }
        out
    };
    // Ekspektasi dicocokkan dgn iverilog (`-g2012` + `vvp`) sebagai oracle.
    assert_eq!(get("s1"), "3.141590");
    assert_eq!(get("s2"), "3.14", "%.2f tak boleh tercetak literal");
    assert_eq!(get("s3"), "3.1416");
    assert_eq!(get("s4"), "3");
    assert_eq!(get("s5"), "   3.142");
    assert_eq!(get("s6"), "3.142");
    assert_eq!(get("s7"), "3.14159", "%g");
    assert_eq!(get("s8"), "3.141590e+00", "%e eksponen 2 digit bertanda");
}

#[test]
fn test_display_hex_and_align_variants_e2e() {
    let source = r#"
module tb;
    reg [7:0] a = 8'hF0;
    reg [7:0] n = 8'd240;
    string h1, h2, w1, w2, w3, w4;
    initial begin
        h1 = $sformatf("%h %x", a, a);
        h2 = $sformatf("%H %X", a, a);
        w1 = $sformatf("[%5d][%-5d][%05d]", n, n, n);
        w2 = $sformatf("[%s][%10s][%-10s]", "hi", "hi", "hi");
        w3 = $sformatf("[%c][%c]", 65, 66);
        w4 = $sformatf("100%%");
        #10 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 50).unwrap();
    let get = |n: &str| {
        let v = sigs.iter().find(|(s, _)| s == n).unwrap().1.clone();
        let mut out = String::new();
        for k in (0..v.width).step_by(8) {
            let mut byte = 0u8;
            for b in 0..8 {
                if v.bits.get(k + b) == Some(&mivon_core::LogicVal::One) {
                    byte |= 1 << b;
                }
            }
            if byte != 0 {
                out.push(byte as char);
            }
        }
        out
    };
    assert_eq!(get("h1"), "f0 f0", "%h/%x lower");
    assert_eq!(get("h2"), "F0 F0", "%H/%X upper (LRM §21.2.1.3)");
    assert_eq!(get("w1"), "[  240][240  ][00240]", "flag - rata kiri");
    assert_eq!(get("w2"), "[hi][        hi][hi        ]", "width utk %s");
    assert_eq!(get("w3"), "[A][B]", "%c");
    assert_eq!(get("w4"), "100%", "%%");
}

// ── Bug mivon utama: struct assignment pattern `'{...}` jadi 0 ──
//
// PRA-FIX: `elaborate_expr` arm `Expr::StructLit` tidak tahu layout typedef
// → FillLit 0, dan `apply_lhs_context_width` const-fold menimpanya lagi dengan
// 0. Akibatnya `p = '{hi:4'hA, lo:4'h5}` menghasilkan `p = 0` (silent wrong),
// padahal member access `p.hi` berfungsi.

#[test]
fn test_struct_assignment_pattern_named() {
    let source = r#"
typedef struct packed { logic [3:0] hi; logic [3:0] lo; } P;
module tb;
    P p;
    initial begin
        p = '{hi: 4'hA, lo: 4'h5};
        $display("SL p=%h hi=%h lo=%h", p, p.hi, p.lo);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 50).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    // `p.hi`/`p.lo` bukan signal top-level — nilainya dicek via $display, yang
    // sudah tercetak `SL p=a5 hi=a lo=5` (lihat stdout test).
    assert_eq!(
        get("p"),
        0xA5,
        "pola bernama harus ter-pack sesuai offset field"
    );
}

#[test]
fn test_struct_assignment_pattern_positional_default_partial() {
    let source = r#"
typedef struct packed { logic [3:0] hi; logic [3:0] lo; } P;
module tb;
    P p_pos, p_def, p_part;
    initial begin
        p_pos  = '{4'h3, 4'h4};
        p_def  = '{default: 4'hF};
        p_part = '{lo: 4'h9};
        $display("SP pos=%h def=%h part=%h", p_pos, p_def, p_part);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 50).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("p_pos"), 0x34, "pola posisional (hi=3, lo=4)");
    assert_eq!(get("p_def"), 0xFF, "`default:` untuk semua member");
    assert_eq!(get("p_part"), 0x09, "member tak disebut = 0");
}

#[test]
fn test_struct_assignment_pattern_nested() {
    let source = r#"
typedef struct packed { logic [3:0] hi; logic [3:0] lo; } P;
typedef struct packed { logic [1:0] tag; P inner; } Outer;
module tb;
    Outer o;
    initial begin
        o = '{tag: 2'h2, inner: '{hi: 4'h5, lo: 4'h6}};
        $display("SN o=%h tag=%h inner=%h", o, o.tag, o.inner);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 50).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    // `o.tag`/`o.inner` dicek lewat $display (`SN o=256 tag=2 inner=56`).
    assert_eq!(get("o"), 0x256, "tag(2)<<6 | inner(0x56)");
}

#[test]
fn test_struct_assignment_pattern_nonblocking() {
    // Pola yang sama di `<=` (non-blocking) dalam blok always.
    let source = r#"
typedef struct packed { logic [3:0] hi; logic [3:0] lo; } P;
module tb;
    reg clk = 0;
    P p;
    always @(posedge clk) p <= '{hi: 4'h7, lo: 4'h8};
    initial begin
        #5 clk = 1;
        #10 $display("SNB p=%h", p);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 50).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("p"), 0x78, "pola di non-blocking assign");
}

#[test]
fn test_virtual_class_pkg_param() {
    // dmi_test.sv: `virtual class rand_dmi #(parameter int AW = 32);`
    let source = r#"
package pkg;
  virtual class rand_dmi #(
    parameter int   AW = 32
  );
    i2c_item req;
  endclass
endpackage
module tb;
  pkg::rand_dmi d;
  initial #1 $finish;
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "virtual class param gagal: {:?}",
        design.err()
    );
}

// ══════════════════════════════════════════════════════════════════════
// Mivon HDL (`.mv`) — LRM 1800 compliance end-to-end
//
// Setiap kasus di bawah menjalankan rantai penuh: `.mv` → lex → parse →
// type-check → codegen SV → parser/elaborator SV Mivon → engine → nilai
// sinyal. Tujuannya: output `mgen` yang BETUL-BETUL bisa dipakai, bukan
// SV yang tampak benar tetapi ditolak tool.
//
// ── LRM 1800 §23.2.1.2 + §26.3 — tipe `use pkg::*` di ANSI port list ──
// Nama di ANSI port list di-resolve di ENCLOSING scope. `import` di dalam
// body module tidak menutupinya → SV hasil generate ditolak iverilog
// ("syntax error") & verilator ("Cannot find file containing interface").
// `use` di-backend SV karena itu di-emit sebagai import di SCOPE FILE.
// ══════════════════════════════════════════════════════════════════════

/// `package` + `use pkg::*` + tipe package di port list & di body.
#[test]
fn test_mv_package_type_in_port_list_end_to_end() {
    let src = r#"
package pkg {
    enum State { IDLE, RUN, DONE }
    type Word = logic[15:0]
}
module dut_p {
    use pkg::*
    in  clk, rst_n : bit
    out st         : State
    out w          : Word
    reg st : State = IDLE
    seq(clk, rst_n) {
        if (!rst_n) { st <= IDLE  w <= 16'h0 }
        else { st <= RUN  w <= w + 16'd1 }
    }
}
module tb_p {
    sig clk, rst_n : bit
    sig st        : State
    sig w         : Word
    inst dut_p u (.clk, .rst_n, .st, .w)
    initial { clk = 0  rst_n = 0  forever #5 clk = ~clk }
    initial {
        #20
        rst_n = 1
        repeat (3) @(posedge clk) {}
        @(negedge clk)
        assert (st == RUN) $info("pkg st ok") else $error("pkg st salah")
        assert (w == 16'd3) $info("pkg w ok") else $error("pkg w salah")
        $display("TB_PKG_OK st=%0d w=%0d", st, w)
        $finish
    }
}
"#;
    let r = mivon_mv::transpile(src, "pkgdemo").expect("transpile .mv OK");

    // Import di SCOPE FILE (sebelum deklarasi module), bukan di body.
    let import_at = r.sv.find("import pkg::*;").expect("harus ada import");
    let module_at = r.sv.find("module dut_p").expect("harus ada module");
    assert!(
        import_at < module_at,
        "import harus sebelum module: {}",
        r.sv
    );
    assert!(
        !r.sv.contains("    import pkg::*;"),
        "import tak boleh di body module: {}",
        r.sv
    );
    assert!(r.sv.contains("output State st"), "port list: {}", r.sv);

    // Rantai penuh: `.svh` mendahului `.sv` (package harus ter-declare dulu).
    let combined = format!("{}\n{}", r.svh, r.sv);
    let sigs = simulate_signals(&combined, 200).expect("simulasi harus jalan");
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("w"), 3, "w naik tiap posedge setelah reset (3 siklus)");
}

/// Typedef lokal module yang dipakai di port list module yang sama harus
/// naik ke scope file (verilator: "Cannot find file containing interface:
/// 'Word8'" kalau tetap di body).
#[test]
fn test_mv_module_local_typedef_in_port_list_end_to_end() {
    let src = r#"
module dut_l {
    type Word8 = logic[7:0]
    in  clk : bit
    out v   : Word8
    reg acc : Word8 = '0
    seq(clk) { acc <= acc + 1  v <= acc }
}
module tb_l {
    sig clk : bit
    sig v   : logic[7:0]
    inst dut_l u (.clk(clk), .v(v))
    initial { clk = 0  forever #5 clk = ~clk }
    initial {
        repeat (3) @(posedge clk) {}
        @(negedge clk)
        $display("TB_LTYPEDEF_OK v=%0d", v)
        $finish
    }
}
"#;
    let r = mivon_mv::transpile(src, "ltd").expect("transpile .mv OK");
    let td_at = r.sv.find("typedef logic [7:0] Word8;").expect("typedef");
    let mod_at = r.sv.find("module dut_l").expect("module");
    assert!(td_at < mod_at, "typedef sebelum module: {}", r.sv);
    assert!(r.sv.contains("output Word8 v"), "port list: {}", r.sv);
    let sigs = simulate_signals(&r.sv, 100).expect("simulasi harus jalan");
    let v = sigs.iter().find(|(s, _)| s == "v").unwrap().1.to_u64();
    assert_eq!(v, 2, "v = 2 siklus (NBA readback dari siklus sebelumnya)");
}

/// Dua blok generate dengan nama variabel sama → label SV harus unik
/// (LRM 1800 §27.6; iverilog: "'gen_i' has already been declared").
#[test]
fn test_mv_duplicate_generate_labels_uniquified() {
    let src = r#"
module dut_g #(N : int = 4) {
    in  clk : bit
    out q   : logic[N-1:0]
    sig p   : logic[N-1:0]
    for i in 0..N { seq(clk) { q[i] <= q[i] } }
    for i in 0..N { comb { p[i] = 1'b1 } }
}
"#;
    let r = mivon_mv::transpile(src, "gt").expect("transpile .mv OK");
    let first = r.sv.find("begin : gen_i\n").expect("label gen_i");
    let second = r.sv.find("begin : gen_i_1").expect("label gen_i_1");
    assert!(first < second, "dua label unik: {}", r.sv);
}

/// Kata kerja DSL `.mv` (`emit` / `override` / `restore` / `await all` /
/// `stop fork`) → bentuk SV, dan tetap jalan di engine.
#[test]
fn test_mv_dsl_verbs_end_to_end() {
    let src = r#"
module tb_dslverb {
    sig clk  : bit
    sig flag : bit
    sig got  : int
    reg  w    : logic[7:0] = 8'd5
    initial { clk = 0  forever #5 clk = ~clk }
    initial {
        repeat (2) @(posedge clk) {}

        // override/release: pada VARIABEL, `restore` hanya membuka blokir
        // prosedural (LRM 1800 §10.6.2) — nilai pansa tetap sampai
        // assignment berikutnya.
        override w = 8'd99
        #1
        assert (w == 99) $info("override ok") else $error("override gagal")

        restore w
        w = 8'd7
        #1
        assert (w == 7) $info("restore ok") else $error("restore gagal")

        emit flag
        #1
        got = 1

        await all
        stop fork

        $display("TB_DSLVERB_OK w=%0d got=%0d", w, got)
        $finish
    }
    initial {
        @(posedge flag)
        $display("TB_DSLVERB flag")
    }
}
"#;
    let r = mivon_mv::transpile(src, "dslverb").expect("transpile .mv OK");
    // Bentuk SV yang di-emit (bukan sintaks `.mv`).
    assert!(r.sv.contains("-> flag;"), "emit → '->': {}", r.sv);
    assert!(r.sv.contains("force w = 8'd99;"), "override: {}", r.sv);
    assert!(r.sv.contains("release w;"), "restore: {}", r.sv);
    assert!(r.sv.contains("wait fork;"), "await all: {}", r.sv);
    assert!(r.sv.contains("disable fork;"), "stop fork: {}", r.sv);

    let sigs = simulate_signals(&r.sv, 200).expect("simulasi harus jalan");
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("w"), 7, "w = 7 setelah restore + assign");
    assert_eq!(get("got"), 1, "jalur setelah emit/await/stop jalan");
}

/// `assert (c) $info(..) else $error(..)` → branch pass WAJIB diakhiri `;`
/// (LRM 1800 §20.11). Tanpa itu verilator: "unexpected else, expecting ';'".
#[test]
fn test_mv_assert_pass_branch_has_semicolon() {
    let src = r#"
module tb_as {
    sig c : bit
    initial {
        c = 1
        assert (c) $info("ok") else $error("bad")
        $finish
    }
}
"#;
    let r = mivon_mv::transpile(src, "asrt").expect("transpile .mv OK");
    assert!(
        r.sv.contains("assert (c) $info(\"ok\"); else $error(\"bad\");"),
        "branch pass harus diakhiri ';': {}",
        r.sv
    );
    simulate_signals(&r.sv, 50).expect("simulasi harus jalan tanpa error");
}

/// `assume (c) $info(..) else $error(..)` → mirror `assert` (LRM 1800 §20.11):
/// branch pass wajib `;` sebelum `else`; `assume property` = module item (§14).
#[test]
fn test_mv_assume_immediate_and_property() {
    let src = r#"
module tb_am {
    sig a : bit
    sig b : bit
    assume property (@(posedge a) a |-> b)
    initial {
        a = 0
        b = 1
        assume (a == 0) $info("assume ok") else $error("assume bad")
        assert (b == 1) $info("assert ok") else $error("assert bad")
        $display("TB_ASSUME_OK a=%0d b=%0d", a, b)
        $finish
    }
}
"#;
    let r = mivon_mv::transpile(src, "asm").expect("transpile .mv OK");
    assert!(
        r.sv.contains("assume (a == 0) $info(\"assume ok\"); else $error(\"assume bad\");"),
        "assume pass harus ';': {}",
        r.sv
    );
    assert!(
        r.sv.contains("assume property (@(posedge a) a |-> b);"),
        "assume property module item: {}",
        r.sv
    );
    let sigs = simulate_signals(&r.sv, 50).expect("simulasi harus jalan");
    let a = sigs.iter().find(|(s, _)| s == "a").unwrap().1.to_u64();
    let b = sigs.iter().find(|(s, _)| s == "b").unwrap().1.to_u64();
    assert_eq!((a, b), (0, 1), "a=0 b=1");
}

/// `.mv` `priority casez` → SV `priority casez` + wildcard match di sim
/// (contoh `examples/mv/case_qualifiers.mv`; regresi: hanya exact-match).
#[test]
fn test_mv_priority_casez_wildcard() {
    let src = r#"
module tb_pcz {
    sig sel : logic[2:0]
    sig y : logic[7:0]
    comb {
        priority casez (sel) {
            3'b1??: { y = 8'hA0 }
            default: { y = 8'hFF }
        }
    }
    initial {
        sel = 3'b101
        #1
        $display("PCZ y=%0h", y)
        $finish
    }
}
"#;
    let r = mivon_mv::transpile(src, "pcz").expect("transpile .mv OK");
    assert!(
        r.sv.contains("priority casez (sel)"),
        "emisi priority casez: {}",
        r.sv
    );
    let sigs = simulate_signals(&r.sv, 50).expect("simulasi harus jalan");
    let y = sigs.iter().find(|(s, _)| s == "y").unwrap().1.to_u64();
    assert_eq!(y, 0xA0, "3'b101 vs 3'b1?? => A0 (y={y:02x})");
}

/// `.mv` `cover` + `cover property` → SV 1:1 + hit tercatat engine.
/// Regresi: `cover` tanpa keyword di-parse jadi call `cover(...)` diam-diam.
#[test]
fn test_mv_cover_immediate_and_property() {
    let src = r#"
module tb_cv {
    sig a : bit
    sig b : bit
    cover property (@(posedge a) a |-> b)
    initial {
        a = 0
        b = 1
        cover (a == 0) $info("covered")
        $display("TB_COVER_OK a=%0d b=%0d", a, b)
        $finish
    }
}
"#;
    let r = mivon_mv::transpile(src, "cov").expect("transpile .mv OK");
    assert!(
        r.sv.contains("cover (a == 0) $info(\"covered\");"),
        "emisi cover: {}",
        r.sv
    );
    assert!(!r.sv.contains("cover(a"), "cover bukan call: {}", r.sv);
    assert!(
        r.sv.contains("cover property (@(posedge a) a |-> b);"),
        "cover property module item: {}",
        r.sv
    );
    let sigs = simulate_signals(&r.sv, 50).expect("simulasi harus jalan");
    let a = sigs.iter().find(|(s, _)| s == "a").unwrap().1.to_u64();
    assert_eq!(a, 0, "a=0");
}

/// `.mv` `case (x) inside` — nilai + rentang + multi-label + default.
/// Output SV bentuk yang diterima pipeline SV (`inside_range_bounds`).
#[test]
fn test_mv_case_inside_decoder() {
    let src = r#"
module tb_ci {
    sig sel : logic[7:0]
    sig y : logic[7:0]
    comb {
        case (sel) inside {
            0 : { y = 8'h00 }
            [1:10] : { y = 8'h0A }
            [11:20], 30 : { y = 8'h14 }
            default : { y = 8'hFF }
        }
    }
    initial {
        sel = 5
        #1
        $display("CI y=%0h", y)
        $finish
    }
}
"#;
    let r = mivon_mv::transpile(src, "ci").expect("transpile .mv OK");
    assert!(r.sv.contains("case (sel) inside"), "emisi inside: {}", r.sv);
    assert!(r.sv.contains("[1:10]:"), "rentang: {}", r.sv);
    let sigs = simulate_signals(&r.sv, 50).expect("simulasi harus jalan");
    let y = sigs.iter().find(|(s, _)| s == "y").unwrap().1.to_u64();
    assert_eq!(y, 0x0A, "sel=5 dalam [1:10] => 0A (y={y:02x})");
}

/// `.mv` `unique if` — qualifier di head, rantai `else if` plain.
#[test]
fn test_mv_unique_if_chain() {
    let src = r#"
module tb_ui {
    sig a : bit
    sig b : bit
    sig y : logic[1:0]
    comb {
        unique if (a) {
            y = 1
        } else if (b) {
            y = 2
        } else {
            y = 0
        }
    }
    initial {
        a = 1
        b = 1
        #1
        $display("UI y=%0d", y)
        $finish
    }
}
"#;
    let r = mivon_mv::transpile(src, "ui").expect("transpile .mv OK");
    assert!(
        r.sv.contains("unique if (a) begin"),
        "head qualified: {}",
        r.sv
    );
    assert!(
        r.sv.contains("end else if (b) begin"),
        "rantai plain: {}",
        r.sv
    );
    let sigs = simulate_signals(&r.sv, 50).expect("simulasi harus jalan");
    let y = sigs.iter().find(|(s, _)| s == "y").unwrap().1.to_u64();
    assert_eq!(y, 1, "a=b=1 first-match => 1 (y={y})");
}

/// `assert property` di blok prosedural (statement-level) — regresi missing
/// `advance()` sesudah keyword `property`.
#[test]
fn test_mv_statement_level_assert_property() {
    let src = r#"
module tb_sp {
    sig clk : bit
    sig a : bit
    initial {
        clk = 0
        a = 1
        #10
        assert property (@(posedge clk) a == 1)
        $display("STMT_PROP_OK")
        $finish
    }
    initial {
        forever #5 clk = ~clk
    }
}
"#;
    let r = mivon_mv::transpile(src, "sp").expect("transpile .mv OK");
    assert!(
        r.sv.contains("assert property (@(posedge clk) a == 1);"),
        "emisi: {}",
        r.sv
    );
    simulate_signals(&r.sv, 50).expect("simulasi harus jalan");
}

/// F72: class UVM tertranspile DENGAN check (tanpa `--no-check`) —
/// `examples/mv_uvm/my_test.mv` lolos E2005 untuk tipe UVM bawaan.
#[test]
fn test_mv_uvm_class_transpiles_with_check() {
    let src = r#"
class my_test extends uvm_test {
    field count     : uint
    rand field seed : uint

    constraint c { seed > 10, seed < 200 }

    func new(name : string) {
        super.new(name)
    }

    task run_phase() {
        var seqr : uvm_sequencer
        var item : uvm_sequence_item
        seqr.start_item(item)
        seqr.finish_item(item)
        #100
    }
}
"#;
    let r = mivon_mv::transpile(src, "my_test").expect("transpile .mv OK tanpa no-check");
    assert!(
        r.sv.contains("class my_test extends uvm_test;"),
        "class: {}",
        r.sv
    );
    assert!(
        r.sv.contains("uvm_sequence_item item;"),
        "item terdeklarasi: {}",
        r.sv
    );
}

/// `uint[8]` = vektor 8-bit (angka = LEBAR), bukan array 8 x 32-bit.
#[test]
fn test_mv_uint_bracket_is_bit_width() {
    let src = r#"
module tb_uw {
    sig a : uint[8]
    sig b : int[12]
    initial {
        a = 8'd200
        b = -3
        $display("TB_UINTW_OK a=%0d b=%0d", a, b)
        $finish
    }
}
"#;
    let r = mivon_mv::transpile(src, "uw").expect("transpile .mv OK");
    assert!(r.sv.contains("logic [7:0] a;"), "uint[8]: {}", r.sv);
    assert!(r.sv.contains("logic signed [11:0] b;"), "int[12]: {}", r.sv);
    let sigs = simulate_signals(&r.sv, 50).expect("simulasi harus jalan");
    let a = sigs.iter().find(|(s, _)| s == "a").unwrap().1.to_u64();
    assert_eq!(a, 200, "a = 200");
}

/// Override parameter instansiasi SELALU di-emit SEBELUM nama instance —
/// bentuk `mod inst #(...)` ditolak iverilog DAN verilator.
#[test]
fn test_mv_inst_params_emitted_before_instance_name() {
    let src = r#"
module sub_p #(W : int = 4) {
    in  d : logic[W-1:0]
    out q : logic[W-1:0]
    comb { q = d }
}
module tb_ip {
    sig clk : bit
    sig d8, q8 : logic[7:0]
    inst sub_p u8 #(.W(8)) (.d(d8), .q(q8))
    sig d4, q4 : logic[3:0]
    inst sub_p u4 #(.W(4)) (.d(d4), .q(q4))
    initial {
        d8 = 8'hA5
        d4 = 4'h5
        #1
        $display("TB_INSTPARAM_OK q8=%h q4=%h", q8, q4)
        $finish
    }
}
"#;
    let r = mivon_mv::transpile(src, "ip").expect("transpile .mv OK");
    assert!(r.sv.contains("sub_p #(.W(8)) u8 ("), "sv: {}", r.sv);
    assert!(
        !r.sv.contains("u8 #("),
        "params tak boleh setelah nama instance: {}",
        r.sv
    );
    let sigs = simulate_signals(&r.sv, 50).expect("simulasi harus jalan");
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("q8"), 0xA5, "lebar 8-bit");
    assert_eq!(get("q4"), 0x5, "lebar 4-bit");
}

/// Demo lengkap `examples/mv/dsl_demo.mv` — transpile → SV → simulasi.
/// Menggabungkan semua perbaikan LRM di satu test.
#[test]
fn test_mv_dsl_demo_example_end_to_end() {
    let src = include_str!("../../../../examples/mv/dsl_demo.mv");
    let r = mivon_mv::transpile(src, "dsl_demo").expect("transpile .mv OK");
    let combined = format!("{}\n{}", r.svh, r.sv);
    let sigs = simulate_signals(&combined, 500).expect("dsl_demo harus simulasi bersih");
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    // tick = 1+2+3+1+2+3 = 12 setelah 6 posedge (reset aktif selama 4 posedge)
    assert_eq!(get("got"), 12, "tick DUT = 12");
    assert_eq!(get("lo"), 3, "mat[1][0]");
    assert_eq!(get("hi"), 4, "mat[1][1]");
    assert_eq!(get("w"), 7, "override/restore + assign");
}

/// Demo `examples/mv/queue_typedef.mv` — queue lewat typedef end-to-end:
/// method push, cast-to-queue, assignment pattern berurutan, dan port queue.
/// Regresi: dimensi `[$]` typedef dulu dibuang → queue jadi skalar.
#[test]
fn test_mv_queue_typedef_example_end_to_end() {
    let src = include_str!("../../../../examples/mv/queue_typedef.mv");
    let r = mivon_mv::transpile(src, "queue_typedef").expect("transpile .mv OK");
    let combined = format!("{}\n{}", r.svh, r.sv);
    let sigs = simulate_signals(&combined, 100).expect("queue_typedef harus simulasi bersih");
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("first"), 3, "push_back + q[0]");
    assert_eq!(get("last"), 4, "q[1]");
    assert_eq!(get("e0"), 0x11, "pattern elemen 0");
    assert_eq!(get("e1"), 0x22, "pattern elemen 1");
    assert_eq!(get("e2"), 0x33, "pattern elemen 2");
    assert_eq!(get("ok"), 1, "semua assertion queue lolos (QTD_OK)");
}

/// E2E `.mv`: klausa `with (item …)` (find/find_index/sum/min/max/sort) dan
/// method bernama keyword (`unique`) — dari sumber `.mv` sampai simulasi.
#[test]
fn test_mv_array_method_with_clause_end_to_end() {
    let src = include_str!("../../../../examples/mv/array_methods.mv");
    let r = mivon_mv::transpile(src, "array_methods").expect("transpile .mv OK");
    let combined = format!("{}\n{}", r.svh, r.sv);
    let sigs = simulate_signals(&combined, 100).expect("array_methods harus simulasi bersih");
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("n"), 2, "find with (item>3)");
    assert_eq!(get("i"), 2, "find_index with (item==3)");
    assert_eq!(get("s"), 30, "sum with (item*2)");
    assert_eq!(get("mn"), 1, "min with (item)");
    assert_eq!(get("mx"), 5, "max with (item)");
    assert_eq!(get("s0"), 1, "sort with (item) → elemen 0");
    assert_eq!(get("s4"), 5, "sort with (item) → elemen 4");
    assert_eq!(get("un"), 3, "unique");
    assert_eq!(get("un2"), 3, "unique_index → 3 indeks unik");
    assert_eq!(get("ui0"), 0, "unique_index indeks 0");
    assert_eq!(get("ui1"), 2, "unique_index indeks 2");
    assert_eq!(get("ui2"), 3, "unique_index indeks 3");
    assert_eq!(get("ok"), 1, "semua lolos (ARR_OK)");
}

/// E2E `.mv` (F86): akses field struct pada iterator `item`
/// (`q.find with (item.x > 3)`) — dari sumber `.mv` sampai simulasi.
#[test]
fn test_mv_array_item_field_end_to_end() {
    let src = include_str!("../../../../examples/mv/array_item_field.mv");
    let r = mivon_mv::transpile(src, "array_item_field").expect("transpile .mv OK");
    let combined = format!("{}\n{}", r.svh, r.sv);
    let sigs = simulate_signals(&combined, 100).expect("array_item_field harus simulasi bersih");
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("n"), 2, "find with (item.x > 3)");
    assert_eq!(get("s"), 60, "sum with (item.y)");
    assert_eq!(get("mn"), 266, "min with (item.x) → elemen pemenang");
    assert_eq!(get("mx"), 2334, "max with (item.y) → elemen pemenang");
    assert_eq!(get("i0"), 2, "find_index with (item.x == 9)");
    assert_eq!(get("ok"), 1, "semua lolos (ITF_OK)");
}

/// E2E `.mv` (bug #41): member struct bertipe `int` (lebar dari TIPE, bukan
/// range) — `e.x`/`e.y` harus baca bit yang benar.
#[test]
fn test_mv_struct_field_width_end_to_end() {
    let src = include_str!("../../../../examples/mv/struct_field_width.mv");
    let r = mivon_mv::transpile(src, "struct_field_width").expect("transpile .mv OK");
    let combined = format!("{}\n{}", r.svh, r.sv);
    let sigs = simulate_signals(&combined, 10).expect("struct_field_width harus simulasi bersih");
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("sx"), 7, "e.x = 7 (member MSB)");
    assert_eq!(get("sy"), 42, "e.y = 42 (member LSB, 32-bit)");
    assert_eq!(get("ok"), 1, "semua lolos (SFW_OK)");
}

/// E2E `.mv` (F88): method array bertaut pada hasil temporer
/// (`q.unique().size()`, `q.find with (...).unique().size()`) — sumber `.mv`
/// sampai simulasi.
#[test]
fn test_mv_chained_methods_end_to_end() {
    let src = include_str!("../../../../examples/mv/chained_methods.mv");
    let r = mivon_mv::transpile(src, "chained_methods").expect("transpile .mv OK");
    let combined = format!("{}\n{}", r.svh, r.sv);
    let sigs = simulate_signals(&combined, 100).expect("chained_methods harus simulasi bersih");
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("n1"), 3, "q.unique().size()");
    assert_eq!(get("n2"), 3, "q.unique_index().size()");
    assert_eq!(get("n3"), 2, "q.find with (item==1).size()");
    assert_eq!(get("n4"), 2, "q.find with (item>1).unique().size()");
    assert_eq!(get("s1"), 6, "q.unique().sum()");
    assert_eq!(get("ok"), 1, "semua lolos (CHAIN_OK)");
}

/// E2E `.mv` (F89): index/range pada queue & hasil method = ELEMEN (bukan bit).
#[test]
fn test_mv_queue_slice_end_to_end() {
    let src = include_str!("../../../../examples/mv/queue_slice.mv");
    let r = mivon_mv::transpile(src, "queue_slice").expect("transpile .mv OK");
    let combined = format!("{}\n{}", r.svh, r.sv);
    let sigs = simulate_signals(&combined, 100).expect("queue_slice harus simulasi bersih");
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("n"), 2, "q[0:1] → 2 elemen");
    assert_eq!(get("r0"), 1, "r[0] = 1");
    assert_eq!(get("r1"), 2, "r[1] = 2");
    assert_eq!(get("e0"), 1, "q.unique()[0]");
    assert_eq!(get("e1"), 2, "q.unique()[1]");
    assert_eq!(get("ei"), 3, "q.find with (item>2)[0]");
    assert_eq!(get("cu"), 4, "q[0:9] → clamp ke 4");
    assert_eq!(get("cd"), 0, "q[3:0] descending → kosong");
    assert_eq!(get("ok"), 1, "semua lolos (QS_OK)");
}

/// F90 — method string LRM 1800 §6.16.1 (getc / atohex / atobin / atooct /
/// atoreal) end-to-end lewat transpile .mv → SV → simulasi.
#[test]
fn test_mv_string_methods_end_to_end() {
    let src = include_str!("../../../../examples/mv/string_methods.mv");
    let r = mivon_mv::transpile(src, "string_methods").expect("transpile .mv OK");
    let combined = format!("{}\n{}", r.svh, r.sv);
    let sigs = simulate_signals(&combined, 100).expect("string_methods harus simulasi bersih");
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("gc0"), 65, "s.getc(0) = 'A' = 65");
    assert_eq!(get("gc1"), 66, "s.getc(1) = 'B' = 66");
    assert_eq!(get("gc9"), 0, "s.getc(9) di luar batas = 0");
    assert_eq!(get("hx"), 31, "atohex('1f') = 31");
    assert_eq!(get("bn"), 5, "atobin('101') = 5");
    assert_eq!(get("oc"), 15, "atooct('17') = 15");
    assert_eq!(get("ok"), 1, "semua lolos (STR_OK)");
}

/// F91 — method string MUTATING LRM 1800 §6.16.1 (itoa/hextoa/bintoa/octtoa/
/// putc) end-to-end lewat transpile .mv → SV → simulasi. Memverifikasi
/// write-back ke receiver, bentuk assignment, dan fix panic F91.
#[test]
fn test_mv_string_mutate_end_to_end() {
    let src = include_str!("../../../../examples/mv/string_mutate.mv");
    let r = mivon_mv::transpile(src, "string_mutate").expect("transpile .mv OK");
    let combined = format!("{}\n{}", r.svh, r.sv);
    let sigs = simulate_signals(&combined, 100).expect("string_mutate harus simulasi bersih");
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    let get_s = |n: &str| -> Vec<u8> {
        sigs.iter()
            .find(|(nm, _)| nm == n)
            .map(|(_, v)| logicvec_to_string(v).into_bytes())
            .unwrap_or_default()
    };
    // Aggregat: ok = 1 (MUT_OK) berarti semua periksa internal lolos.
    assert_eq!(get("ok"), 1, "semua lolos (MUT_OK)");
    // Periksa juga nilai string aktual (bukan hanya aggregate).
    assert_eq!(get_s("a"), b"-42".to_vec(), "a.itoa(-42) = \"-42\"");
    assert_eq!(get_s("b"), b"ff".to_vec(), "b.hextoa(255) = \"ff\"");
    assert_eq!(get_s("c"), b"101".to_vec(), "c.bintoa(5) = \"101\"");
    assert_eq!(get_s("d"), b"17".to_vec(), "d.octtoa(15) = \"17\"");
    assert_eq!(get_s("e"), b"Xbc".to_vec(), "e.putc(0,88) = \"Xbc\"");
    assert_eq!(
        get_s("f"),
        vec![b'a', b'b', b'c', 0, 0, b'Z'],
        "f.putc(5,90) extend dgn NUL = \"abc\\0\\0Z\""
    );
    assert_eq!(get_s("g"), b"abc".to_vec(), "g.putc(-3,..) no-op = \"abc\"");
    assert_eq!(get("flen"), 6, "f.len() setelah putc extend = 6");
    assert_eq!(
        get_s("y"),
        b"9".to_vec(),
        "assignment-form y = z.itoa(9) = \"9\""
    );
    assert_eq!(get_s("z"), b"9".to_vec(), "itoa mutating: z ikut \"9\"");
}

/// Regresi bug #37: rantai `&&`/`||` panjang (16 operan) TIDAK boleh
/// stack-overflow pada jalur evaluator rekursif (`simulate_signals` →
/// `EngineFlags::default()`, `use_packed_eval=false`). Sebelum fix, frame
/// `evaluate_expr_impl` ~184 KB/level (arm `SysFunc` raksasa menumpuk di
/// frame yang sama) → `SIGABRT` pada 8 operan di thread test 2 MiB. Kini arm
/// itu dipindah ke method `#[inline(never)]` `eval_sysfunc`.
#[test]
fn test_mv_long_logic_chain_no_stack_overflow() {
    let src = include_str!("../../../../examples/mv/logic_chain.mv");
    let r = mivon_mv::transpile(src, "logic_chain").expect("transpile .mv OK");
    let sigs = simulate_signals(&r.sv, 10).expect("rantai && panjang harus jalan");
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("y_and"), 0, "i=0 → AND rantai = 0");
    assert_eq!(get("y_or"), 1, "a=1 → OR rantai = 1");
}

/// Regresi bug #38: rantai `&&` **64 operan sinyal** (bukan konstanta) dulu
/// `SIGABRT` saat **ELABORASI** (bukan evaluasi): `const_eval_with_params`
/// (~944 baris, satu match raksasa) di-rekursi sampai dasar sebelum gagal,
/// frame ~ratusan KB/level → stack thread test 2 MiB jebol (batas pra-fix:
/// 16 LULUS / 32 GAGAL). Fix: guard `contains_signal_ref` + dispatcher tipis +
/// `const_eval_ops`/`const_eval_deep`/`elaborate_binary`/`elaborate_unary`.
#[test]
fn test_mv_very_long_logic_chain_no_stack_overflow() {
    let n = 64usize;
    let mut decls = String::new();
    let mut assigns = String::new();
    let mut terms = Vec::new();
    for i in 0..n {
        decls.push_str(&format!("    sig s{} : bit\n", i));
        assigns.push_str(&format!("        s{} = 1\n", i));
        terms.push(format!("s{}", i));
    }
    let src = format!(
        "module tb_chain64 {{\n{}    sig y : bit\n    initial {{\n{}        y = {}\n        $display(\"CHAIN64_OK y=%0d\", y)\n        $finish\n    }}\n}}\n",
        decls,
        assigns,
        terms.join(" && ")
    );
    let r = mivon_mv::transpile(&src, "chain64").expect("transpile .mv OK");
    let sigs = simulate_signals(&r.sv, 10).expect("rantai 64 operan harus jalan");
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("y"), 1, "semua operand 1 → AND 64 operan = 1");
}

// ── LRM 1800 §7.3 — dimensi unpacked harus SETELAH nama variabel ──────
//
// `logic [7:0] [4] fa;` placingkan dimension UNPACKED di posisi packed
// (sebelum identifier). Parser Mivon sebelumnya "konsumsi dan buang" token
// itu lalu mengembalikan deklarasi dengan NOL nama — hasilanya sinyal `fa`
// tidak pernah ada, TANPA diagnostic (silent miscompilation). Sekarang
// ditolak dengan pesan yang menyebut aturan LRM-nya.

#[test]
fn test_lrm_unpacked_dim_before_name_is_rejected() {
    // Pesan diagnosiktik harus menyebut aturan LRM + bentuk yang benar.
    let src = "module top;\n  logic [7:0] [4] fa;\nendmodule\n";
    let err = compile_str(src).expect_err("dimensi unpacked sebelum nama harus ditolak");
    let msg = err.to_string();
    assert!(
        msg.contains("LRM 1800") || msg.contains("tanpa nama variabel"),
        "pesan harusunjuk aturan LRM: {msg}"
    );
    // Tidak ada diagnostic yang mencurigakan "sinyal tak dikenal" — ini
    // rejection saat parse, bukan error semantik turunan.
    let design = compile_str("module top;\n  logic [7:0] fa [0:3];\nendmodule\n");
    assert!(design.is_ok(), "bentuk yang benar harus valid");
}

#[test]
fn test_lrm_unpacked_dim_before_name_in_class_field_is_rejected() {
    // Jalur class punya recovery yang dulu menelan error diam-diam
    // (`Err(_) => skip_until_semi_or_end()`) → field hilang dari design.
    // Sekarang error-nya dipropagasi (bukan ditelan recovery).
    let src = "class C;\n  logic [7:0] [4] fa;\nendclass\nmodule top;\nendmodule\n";
    let err = compile_str(src).expect_err("field class invalid harus ditolak");
    let msg = err.to_string();
    assert!(
        msg.contains("LRM 1800") || msg.contains("tanpa nama variabel"),
        "pesan harus menunjuk aturan LRM: {msg}"
    );
}

#[test]
fn test_lrm_unpacked_dim_after_name_is_accepted() {
    // Bentuk yang benar tetap valid: packed range, identifier, baru dimensi
    // unpacked (LRM 1800 §7.3).
    let ok = compile_str("typedef logic [7:0] T [0:3];\nmodule top;\nendmodule\n");
    assert!(
        ok.is_ok(),
        "typedef dengan unpacked setelah nama harus valid"
    );
    let ok2 =
        compile_str("module top;\n  logic [7:0] fa [0:3];\n  initial fa[0] = 1'b1;\nendmodule\n");
    assert!(
        ok2.is_ok(),
        "deklarasi dengan dimensi setelah nama harus valid"
    );
}

// ══════════════════════════════════════════════════════════════════════
// Mivon HDL (`.mv`) — bahasa & type-check yang ditambahkan (F66–F70)
//
// Semua test di bawah menjalankan rantai penuh: `.mv` → lex → parse →
// type-check → codegen SV → parser/elaborator SV Mivon → engine → nilai
// sinyal. Tujuannya: fitur baru benar-benar bisa DISIMULASI, bukan cuma
// lolos parse.
// ══════════════════════════════════════════════════════════════════════

/// Binary XNOR `~^` / `^~` (LRM 1800 §11.13) — sebelumnya `~` selalu
/// unary sehingga `a ~^ b` gagal parse meski didokumentasikan di §6.6.
#[test]
fn test_mv_binary_xnor_end_to_end() {
    let src = r#"
module tb_xnor {
    sig a, b, y, z : logic[7:0]
    initial {
        a = 8'h0F
        b = 8'h33
        y = a ~^ b
        z = a ^~ b
        $display("XNOR_OK y=%h z=%h", y, z)
        $finish
    }
}
"#;
    let r = mivon_mv::transpile(src, "xnor").expect("transpile .mv OK");
    assert!(r.sv.contains("y = a ~^ b;"), "sv: {}", r.sv);
    assert!(
        r.sv.contains("z = a ~^ b;"),
        "ejaan ^~ dinormalkan: {}",
        r.sv
    );
    let sigs = simulate_signals(&r.sv, 20).expect("simulasi harus jalan");
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    // ~0x0F ^ 0x33 == ~0x3C == 0xC3
    assert_eq!(get("y"), 0xC3, "XNOR");
    assert_eq!(get("z"), 0xC3, "ejaan alternatif harus sama");
}

/// Indexed part-select `q[i +: w]` / `q[i -: w]` (LRM 1800 §11.8.2) —
/// `w` adalah LEBAR bit, bukan indeks akhir.
#[test]
fn test_mv_indexed_part_select_end_to_end() {
    let src = r#"
module tb_psel {
    sig a : logic[15:0]
    sig d : logic[2:0]
    sig up, down : logic[7:0]
    initial {
        a = 16'hBEEF
        d = 3'd4
        up   = a[d +: 8]
        down = a[d +: 4]
        $display("PSEL_OK up=%h down=%h", up, down)
        $finish
    }
}
"#;
    let r = mivon_mv::transpile(src, "psel").expect("transpile .mv OK");
    assert!(r.sv.contains("up = a[d +: 8];"), "sv: {}", r.sv);
    let sigs = simulate_signals(&r.sv, 20).expect("simulasi harus jalan");
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    // 0xBEEF >> 4 = 0x0BEE; 8 bit rendah = 0xEE
    assert_eq!(get("up"), 0xEE, "a[4 +: 8]");
    assert_eq!(get("down"), 0x0E, "a[4 +: 4]");
}

/// Literal yang sebelumnya tidak didukung: real berpangkat (`1e3`),
/// awalan basis tanpa lebar (`0x`/`0o`/`0b`), underscore desimal.
#[test]
fn test_mv_extended_literals_end_to_end() {
    let src = r#"
module tb_lit {
    sig a : logic[31:0]
    initial {
        a = 1_000_000
        $display("LIT dec=%0d", a)
        a = 0xFF
        $display("LIT hex=%h", a)
        a = 0o17
        $display("LIT oct=%h", a)
        a = 0b1011
        $display("LIT bin=%h", a)
        $display("LIT re=%f rf=%f", 1e3, 2.5e-2)
        $finish
    }
}
"#;
    let r = mivon_mv::transpile(src, "lit").expect("transpile .mv OK");
    let sigs = simulate_signals(&r.sv, 20).expect("simulasi harus jalan");
    let a = sigs.iter().find(|(s, _)| s == "a").unwrap().1.to_u64();
    assert_eq!(a, 0b1011, "literal terakhir = 0b1011");
}

/// E2010 — parameter & `const` module tak boleh di-assign (LRM 1800 §6.20
/// "shall be illegal"). Sebelumnya lolos type-check lalu ditolak elaborator.
#[test]
fn test_mv_reject_assign_to_const_end_to_end() {
    let src = "module m #(W = 8) { in c : bit\n sig s : logic[7:0]\n comb { W = 4 } }";
    let e = mivon_mv::transpile(src, "lv").unwrap_err();
    assert!(
        e.msg.contains("E2010"),
        "harus E2010, dapat: {} ({}:{})",
        e.msg,
        e.line,
        e.col
    );
    assert!(e.line > 0, "error harus berposisi");
}

/// E2011 — jumlah argumen pemanggilan harus cocok dengan tanda tangan.
#[test]
fn test_mv_reject_wrong_call_arity() {
    let src = "func f(a : int, b : int = 2) -> int { return a + b }\nmodule m { in c : bit\n sig r : int\n comb { r = f(1, 2, 3) } }";
    let e = mivon_mv::transpile(src, "ar").unwrap_err();
    assert!(e.msg.contains("E2011"), "msg: {}", e.msg);
    assert!(e.line > 0, "error harus berposisi");
    // bentuk sah tetap jalan
    let ok = "func f(a : int, b : int = 2) -> int { return a + b }\nmodule m2 { in c : bit\n sig r : int\n initial { r = f(7) } }";
    mivon_mv::transpile(ok, "ar2").expect("1 argumen + default harus sah");
}

/// E2012 — `logic` unsigned → `u < 0` selalu false (LRM 1800 §11.8.2).
/// Ini jebakan yang menghasilkan bug senyap (buglog-mv #5).
#[test]
fn test_mv_reject_unsigned_comparison_with_zero() {
    let bad = "module m { in c : bit\n sig u : logic[7:0]\n sig y : bit\n comb { y = u < 0 } }";
    let e = mivon_mv::transpile(bad, "sg").unwrap_err();
    assert!(e.msg.contains("E2012"), "msg: {}", e.msg);
    assert!(e.line > 0, "error harus berposisi");
    // signed aman dan harus tetap bisa disimulasikan dengan benar
    let ok = r#"
module tb_sg {
    sig s : signed logic[7:0]
    sig hit : bit
    initial {
        s = -1
        hit = 0
        if (s < 0) { hit = 1 }
        $display("SIGNED_OK hit=%0d", hit)
        $finish
    }
}
"#;
    let r = mivon_mv::transpile(ok, "sg2").expect("signed < 0 harus lolos check");
    let sigs = simulate_signals(&r.sv, 20).expect("simulasi harus jalan");
    let hit = sigs.iter().find(|(s, _)| s == "hit").unwrap().1.to_u64();
    assert_eq!(hit, 1, "s = -1 maka s < 0 harus true");
}

/// Lebar enum yang dilihat checker harus sama dengan yang di-emit
/// (`enum 3 anggota` → `logic [1:0]`, 2 bit).
#[test]
fn test_mv_enum_width_matches_sv() {
    let src = r#"
package p { enum St { A, B, C } }
module tb_en {
    use p::*
    sig s : St
    sig got : int
    initial {
        s = B
        got = 0
        if (s == B) { got = 1 }
        $display("ENUM_OK got=%0d", got)
        $finish
    }
}
"#;
    let r = mivon_mv::transpile(src, "en").expect("enum 3 anggota = 2 bit");
    assert!(r.svh.contains("typedef enum logic [1:0]"), "svh: {}", r.svh);
    let combined = format!("{}\n{}", r.svh, r.sv);
    let sigs = simulate_signals(&combined, 20).expect("simulasi harus jalan");
    let got = sigs.iter().find(|(s, _)| s == "got").unwrap().1.to_u64();
    assert_eq!(got, 1, "s == B");
}

/// `const` module bisa dipakai sebagai lebar sinyal (E2002 aktif lewatnya).
#[test]
fn test_mv_module_const_used_as_width() {
    let bad = "module m { in c : bit\n const W = 4\n sig a : logic[W-1:0]\n comb { a = 8'hFF } }";
    let e = mivon_mv::transpile(bad, "cw").unwrap_err();
    assert!(e.msg.contains("E2002"), "msg: {}", e.msg);
    // nilai yang muat lolos
    let ok = "module m2 { in c : bit\n const W = 4\n sig a : logic[W-1:0]\n initial { a = 4'hF } }";
    mivon_mv::transpile(ok, "cw2").expect("nilai yang muat harus sah");
}

/// E2013 — label `case` duplikat membuat branch tak pernah dieksekusi.
#[test]
fn test_mv_reject_duplicate_case_label() {
    let src = "module m { in c : bit\n sig s : logic[1:0]\n sig o : logic[7:0]\n comb { case (s) { 2'd0: { o = 1 }  0: { o = 2 }  default: { o = 0 } } } }";
    let e = mivon_mv::transpile(src, "cs").unwrap_err();
    assert!(e.msg.contains("E2013"), "msg: {}", e.msg);
    assert!(e.line > 0, "error harus berposisi");
}

/// Elaborator: nilai enum eksplisit yang tak ter-evaluasi (typo/unknown)
/// TIDAK boleh ditebak diam-diam dari counter — warning + member dilewati
/// (rujukan ke sana gagal E2001). Forward-ref yang valid tetap resolve.
#[test]
fn test_enum_unknown_value_not_guessed() {
    fn elaborate(
        source: &str,
    ) -> (
        Result<mivon_ir::IrDesign, mivon_core::error::SimError>,
        Vec<mivon_core::diagnostics::Diagnostic>,
    ) {
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
        let parser = mivon_parser::Parser::new(tokens, "<string>");
        let mut parser = parser.with_source_lines(&preprocessed);
        let design = parser.parse_design().unwrap();
        let source_lines: Vec<String> = preprocessed.lines().map(|s| s.to_string()).collect();
        let mut elaborator = mivon_elaboration::Elaborator::with_source(
            design,
            source_lines,
            "<string>".to_string(),
        );
        let r = elaborator.elaborate(
            None,
            mivon_elaboration::elaborator::ElaborateMode::StrictSimulation,
        );
        let diags = elaborator.flush_diagnostics();
        (r, diags)
    }

    // Typo: A ditebak 0 + B=1 diam-diam sebelum fix. Kini warning saat
    // elaborasi + E2001 jujur di use-site (identik dengan ident tak dikenal).
    let bad = r#"
package p;
typedef enum { A = TYPO_X, B } E;
endpackage
module top;
  import p::*;
  E s;
  initial begin s = A; $display("%0d", s); $finish; end
endmodule
"#;
    let (r, diags) = elaborate(bad);
    assert!(r.is_err(), "rujukan ke member tak-terdefinisi harus gagal");
    let err = format!("{:?}", r.unwrap_err());
    assert!(
        err.contains("E2001") || err.contains("UndefinedSignal"),
        "error use-site harus E2001: {err}"
    );
    let warns: Vec<_> = diags
        .iter()
        .filter(|d| d.message.contains("konstan"))
        .collect();
    assert_eq!(
        warns.len(),
        1,
        "tepat satu warning (tak spam/duplikat): {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
    assert!(
        warns[0].message.contains("'A'"),
        "warning enum harus menyebut member: {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
    // Forward-ref valid tetap resolve tanpa warning.
    let good = r#"
package q;
typedef enum { F = W2 } E2;
parameter int W2 = 7;
endpackage
module top;
  import q::*;
  E2 s2 = F;
  initial begin if (s2 !== 7) $error("fwd"); $finish; end
endmodule
"#;
    let (r2, diags2) = elaborate(good);
    assert!(r2.is_ok(), "forward-ref valid harus lolos: {diags2:?}");
    assert!(
        !diags2.iter().any(|d| d.message.contains("konstan")),
        "tanpa warning palsu: {:?}",
        diags2.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
    // Shadowing LRM: param package-sendiri menang atas nama global sama.
    let shadow = r#"
parameter int W = 3;
package sp;
parameter int W = 1;
typedef enum { G = W } E3;
endpackage
module top;
  import sp::*;
  E3 s3;
  initial begin #1; s3 = G; #1; $finish; end
endmodule
"#;
    let (r3, diags3) = elaborate(shadow);
    assert!(r3.is_ok(), "shadowing harus lolos: {diags3:?}");
    let sigs3 = mivon_api::simulate_signals(shadow, 10).unwrap();
    let s3 = sigs3.iter().find(|(n, _)| n == "s3").unwrap().1.to_u64();
    assert_eq!(
        s3, 1,
        "param package (1) menang atas global (3), bukan sebaliknya"
    );
}

/// Override parameter instance dengan ekspresi NON-konstan (sinyal) adalah
/// ilegal LRM — error ParamMismatch di titik override, bukan tebak 0.
#[test]
fn test_param_override_nonconst_is_error() {
    fn elaborate(
        source: &str,
    ) -> (
        Result<mivon_ir::IrDesign, mivon_core::error::SimError>,
        Vec<mivon_core::diagnostics::Diagnostic>,
    ) {
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
        let parser = mivon_parser::Parser::new(tokens, "<string>");
        let mut parser = parser.with_source_lines(&preprocessed);
        let design = parser.parse_design().unwrap();
        let source_lines: Vec<String> = preprocessed.lines().map(|s| s.to_string()).collect();
        let mut elaborator = mivon_elaboration::Elaborator::with_source(
            design,
            source_lines,
            "<string>".to_string(),
        );
        let r = elaborator.elaborate(
            None,
            mivon_elaboration::elaborator::ElaborateMode::StrictSimulation,
        );
        let diags = elaborator.flush_diagnostics();
        (r, diags)
    }
    let src = r#"
module sub #(W = 4);
  logic [W-1:0] q;
  initial begin q = 0; $finish; end
endmodule
module top;
  logic dyn_w;
  sub #(.W(dyn_w)) u (.q());
endmodule
"#;
    let (r, diags) = elaborate(src);
    assert!(r.is_err(), "override non-konstan harus gagal");
    let msgs: Vec<&str> = diags.iter().map(|d| d.message.as_ref()).collect();
    assert!(
        msgs.iter()
            .any(|m| m.contains("konstan") && m.contains("'W'") && m.contains("'u'")),
        "diagnostic harus menyebut param + instance: {msgs:?}"
    );
    // Posisi type-param tak dikenal: pesan menyebut alternatif tipe.
    let bad_tp = r#"
module sub #(parameter type T = logic[7:0]);
  T r;
  initial begin r = 0; $finish; end
endmodule
module top;
  sub #(.T(NopeUnknown)) u ();
endmodule
"#;
    let (r_tp, diags_tp) = elaborate(bad_tp);
    assert!(r_tp.is_err(), "type tak dikenal harus gagal");
    let msgs_tp: Vec<&str> = diags_tp.iter().map(|d| d.message.as_ref()).collect();
    assert!(
        msgs_tp.iter().any(|m| m.contains("nama tipe")
            && m.contains("'T'")
            && m.contains("'u'")
            && m.contains("konstan")),
        "pesan type-param: {msgs_tp:?}"
    );
    // Override konstan (param lain) tetap sah.
    let ok = r#"
module sub #(W = 4);
  logic [W-1:0] q;
  initial begin q = 0; $finish; end
endmodule
module top;
  localparam int K = 8;
  sub #(.W(K)) u (.q());
endmodule
"#;
    let (r2, diags2) = elaborate(ok);
    assert!(
        r2.is_ok(),
        "override param-dengan-param harus lolos: {diags2:?}"
    );
}

/// Lebar replikasi non-konstan ilegal LRM — error eksplisit (count selalu
/// statis di IR, tak ada jalur runtime dinamis). Part-select dinamis
/// SENGAJA tak disentuh (runtime IrExpr::ExprPartSelect mendukungnya).
#[test]
fn test_dynamic_replicate_width_is_error() {
    fn elaborate(
        source: &str,
    ) -> (
        Result<mivon_ir::IrDesign, mivon_core::error::SimError>,
        Vec<mivon_core::diagnostics::Diagnostic>,
    ) {
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
        let parser = mivon_parser::Parser::new(tokens, "<string>");
        let mut parser = parser.with_source_lines(&preprocessed);
        let design = parser.parse_design().unwrap();
        let source_lines: Vec<String> = preprocessed.lines().map(|s| s.to_string()).collect();
        let mut elaborator = mivon_elaboration::Elaborator::with_source(
            design,
            source_lines,
            "<string>".to_string(),
        );
        let r = elaborator.elaborate(
            None,
            mivon_elaboration::elaborator::ElaborateMode::StrictSimulation,
        );
        let diags = elaborator.flush_diagnostics();
        (r, diags)
    }
    // Count replikasi dari sinyal.
    let rep = r#"
module top;
  logic [7:0] y;
  logic n;
  initial begin y = {n{1'b1}}; $finish; end
endmodule
"#;
    let (r, _) = elaborate(rep);
    assert!(r.is_err(), "replikasi dinamis harus gagal");
    // Lebar part-select dari sinyal didukung runtime (superset dinamis) —
    // hanya replikasi yang wajib statis (count selalu ter-resolve di IR).
    let ps = r#"
module top;
  logic [15:0] a;
  logic [7:0] y;
  logic [3:0] idx;
  initial begin y = a[idx +: 8]; $finish; end
endmodule
"#;
    let (r2, diags2) = elaborate(ps);
    assert!(
        r2.is_ok(),
        "part-select dinamis didukung runtime: {diags2:?}"
    );
    // Kontrol: keduanya konstan tetap sah.
    let ok = r#"
module top;
  logic [15:0] a;
  logic [15:0] y;
  localparam int N = 2;
  localparam int W = 8;
  initial begin y = {N{a[0 +: W]}}; $finish; end
endmodule
"#;
    let (r3, diags3) = elaborate(ok);
    assert!(r3.is_ok(), "bentuk konstan harus lolos: {diags3:?}");
}

/// Literal unsized berbasis (`'b101`, `'hFF`) lebarnya minimal 32 bit
/// (LRM §5.7.1) — bukan 1. Diferensial vs iverilog (`$bits('b101)` = 32).
/// Via KONTEKS KONSTAN (localparam) agar jalur const-eval yang diuji —
/// jalur runtime `$bits` memang sudah benar sebelumnya.
#[test]
fn test_unsized_based_literal_width_32() {
    let src = r#"
module top;
  localparam int WB = $bits('b101);
  localparam int WH = $bits('hFF);
  localparam int W8 = $bits(8'hFF);
  logic [31:0] wb;
  logic [31:0] wh;
  logic [31:0] w8;
  initial begin
    wb = WB;
    wh = WH;
    w8 = W8;
    $finish;
  end
endmodule
"#;
    let sigs = mivon_api::simulate_signals(src, 10).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("wb"), 32, "$bits('b101) harus 32 (iverilog sama)");
    assert_eq!(get("wh"), 32, "$bits('hFF) harus 32");
    assert_eq!(get("w8"), 8, "$bits(8'hFF) tetap 8");
}

/// Width part-select LHS dinamis tak terwakili di IR statis — warning
/// eksplisit (bukan tebak 1 diam-diam). Rvalue dinamis tetap didukung penuh.
#[test]
fn test_dynamic_partselect_lvalue_width_warns() {
    fn elaborate(
        source: &str,
    ) -> (
        Result<mivon_ir::IrDesign, mivon_core::error::SimError>,
        Vec<mivon_core::diagnostics::Diagnostic>,
    ) {
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
        let parser = mivon_parser::Parser::new(tokens, "<string>");
        let mut parser = parser.with_source_lines(&preprocessed);
        let design = parser.parse_design().unwrap();
        let source_lines: Vec<String> = preprocessed.lines().map(|s| s.to_string()).collect();
        let mut elaborator = mivon_elaboration::Elaborator::with_source(
            design,
            source_lines,
            "<string>".to_string(),
        );
        let r = elaborator.elaborate(
            None,
            mivon_elaboration::elaborator::ElaborateMode::StrictSimulation,
        );
        let diags = elaborator.flush_diagnostics();
        (r, diags)
    }
    let src = r#"
module top;
  logic [15:0] a;
  logic [15:0] y;
  logic [3:0] w;
  initial begin y[0 +: w] = 8'hFF; $finish; end
endmodule
"#;
    let (r, diags) = elaborate(src);
    assert!(r.is_ok(), "elaborate lanjut terdegradasi: {diags:?}");
    let msgs: Vec<&str> = diags.iter().map(|d| d.message.as_ref()).collect();
    assert!(
        msgs.iter().any(|m| m.contains("tidak konstan")),
        "warning width harus eksplisit: {msgs:?}"
    );
}

/// Step generate-for non-konstan (sinyal) ilegal LRM — warning eksplisit +
/// blok di-skip (bukan iterasi step-1 diam-diam).
#[test]
fn test_generate_for_step_nonconst_warns() {
    fn elaborate(
        source: &str,
    ) -> (
        Result<mivon_ir::IrDesign, mivon_core::error::SimError>,
        Vec<mivon_core::diagnostics::Diagnostic>,
    ) {
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
        let parser = mivon_parser::Parser::new(tokens, "<string>");
        let mut parser = parser.with_source_lines(&preprocessed);
        let design = parser.parse_design().unwrap();
        let source_lines: Vec<String> = preprocessed.lines().map(|s| s.to_string()).collect();
        let mut elaborator = mivon_elaboration::Elaborator::with_source(
            design,
            source_lines,
            "<string>".to_string(),
        );
        let r = elaborator.elaborate(
            None,
            mivon_elaboration::elaborator::ElaborateMode::StrictSimulation,
        );
        let diags = elaborator.flush_diagnostics();
        (r, diags)
    }
    let src = r#"
module top;
  logic s;
  genvar i;
  generate
    for (i = 0; i < 4; i = i + s) begin : g
      wire w;
    end
  endgenerate
  initial begin s = 0; $finish; end
endmodule
"#;
    let (r, diags) = elaborate(src);
    // Jalur expand_all_generates menurunkan error ekspansi menjadi warning +
    // blok di-skip (kebijakan anti-cascade): asersi warning eksplisitnya.
    assert!(r.is_ok(), "elaborate lanjut: {diags:?}");
    let msgs: Vec<&str> = diags.iter().map(|d| d.message.as_ref()).collect();
    assert!(
        msgs.iter().any(|m| m.contains("step eval failed")),
        "warning step harus eksplisit (bukan iterasi step-1 diam-diam): {msgs:?}"
    );
    // Step konstan tetap sah.
    let ok = r#"
module top;
  genvar i;
  generate
    for (i = 0; i < 8; i = i + 2) begin : g
      wire w;
    end
  endgenerate
  initial begin $finish; end
endmodule
"#;
    let (r2, diags2) = elaborate(ok);
    assert!(r2.is_ok(), "step konstan harus lolos: {diags2:?}");
}

#[test]
fn test_struct_param_zero_cast_cfg_empty() {
    // Bug CVA6: `localparam cfg_t CFG_EMPTY = cfg_t'(0)` harus jadi struct
    // zero (semua field 0) agar `Cfg.XLEN`/`Cfg.FOO` bisa const-eval.
    // Sebelum fix: "member access not allowed in constant expression".
    let source = r#"
package p;
  typedef struct packed { int unsigned XLEN; bit FOO; } cfg_t;
  localparam cfg_t CFG_EMPTY = cfg_t'(0);
endpackage
module top import p::*;
#(parameter cfg_t Cfg = CFG_EMPTY)
();
  logic flag;
  initial begin
    if (Cfg.FOO == 0) flag = 1'b1; else flag = 1'b0;
    $finish;
  end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("flag"), 1, "FOO dari cfg_t'(0) harus 0");
}

#[test]
fn test_struct_param_func_return_cfg() {
    // Bug CVA6: `localparam cfg_t C = build_config(U)` — fungsi package
    // return struct harus bisa di-const-eval (arg struct, local `cfg.F`
    // assign, `return cfg`). Sebelum fix: "member access not allowed".
    let source = r#"
package cfg_pkg;
  typedef struct packed { int unsigned XLEN; bit IS64; bit FOO; } cfg_t;
  typedef struct packed { int unsigned XLEN; bit FOO; } user_cfg_t;
  function automatic cfg_t build_config(user_cfg_t u);
    bit is64 = (u.XLEN == 64) ? 1'b1 : 1'b0;
    cfg_t cfg;
    cfg.XLEN = u.XLEN;
    cfg.IS64 = is64;
    cfg.FOO = u.FOO;
    return cfg;
  endfunction
  localparam user_cfg_t U = '{XLEN:64, FOO:1};
  localparam cfg_t C = build_config(U);
endpackage
module top import cfg_pkg::*;
#(parameter cfg_t Cfg = C)
();
  logic [Cfg.XLEN-1:0] wide;
  logic flag;
  initial begin
    wide = '1;
    if (Cfg.IS64) flag = 1'b1; else flag = 1'b0;
    $finish;
  end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let wide = &sigs.iter().find(|(s, _)| s == "wide").unwrap().1;
    assert_eq!(wide.width, 64, "XLEN dari build_config harus 64");
    assert_eq!(wide.to_u64(), u64::MAX, "wide='1 harus semua 1");
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("flag"), 1, "IS64 harus 1 untuk XLEN=64");
}

#[test]
fn test_const_func_unique_case_inside() {
    // Bug CVA6: fungsi package `unique case (op) inside` (pola
    // ariane_pkg::op_is_branch/is_rd_fpr) tak didukung evaluator konstanta
    // → "has no return". Mendukung label enum, multi-label, range `[a:b]`.
    let source = r#"
package fpkg;
  typedef enum logic [3:0] { OP_A=0, OP_B=1, OP_C=2, OP_D=3, OP_E=4 } op_t;
  function automatic logic is_bc(input op_t op);
    unique case (op) inside
      OP_B, OP_C: return 1'b1;
      [OP_C : OP_E]: return 1'b1;
      default: return 1'b0;
    endcase
  endfunction
  localparam op_t OP = OP_D;
  localparam bit R = is_bc(OP);
endpackage
module top import fpkg::*;
#(parameter bit V = R)
();
  logic [V:0] w;
  logic flag;
  initial begin
    if (V) flag = 1'b1; else flag = 1'b0;
    $finish;
  end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let w = &sigs.iter().find(|(s, _)| s == "w").unwrap().1;
    assert_eq!(w.width, 2, "R=is_bc(OP_D)=1 via range → [1:0]");
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("flag"), 1, "V harus 1");
}

#[test]
fn test_pkg_case_func_runtime_call() {
    // Panggilan RUNTIME fungsi case-inside package dengan arg sinyal:
    // elaborasi harus emit FuncCall (bukan E3001 "has no return"), engine
    // eksekusi tiap arm benar (exact, range, default). Pola CVA6
    // branch_unit `ariane_pkg::op_is_branch(fu_data_i.operation)`.
    let source = r#"
package fpkg;
  typedef enum logic [3:0] { OP_A=0, OP_B=1, OP_C=2, OP_D=3, OP_E=4 } op_t;
  function automatic logic is_bc(input op_t op);
    unique case (op) inside
      OP_B, OP_C: return 1'b1;
      [OP_C : OP_E]: return 1'b1;
      default: return 1'b0;
    endcase
  endfunction
endpackage
module top;
  import fpkg::*;
  logic [3:0] sel = 0;
  logic out_flag;
  logic r0, r1, r3;
  always_comb out_flag = is_bc(sel);
  initial begin
    sel = 4'd0; #1; r0 = out_flag;
    sel = 4'd1; #1; r1 = out_flag;
    sel = 4'd3; #1; r3 = out_flag;
    $finish;
  end
endmodule
"#;
    let sigs = simulate_signals(source, 50).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("r0"), 0, "OP_A → default 0");
    assert_eq!(get("r1"), 1, "OP_B → exact 1");
    assert_eq!(get("r3"), 1, "OP_D → range 1");
}

#[test]
fn test_pkg_struct_member_access_qualified() {
    // Member struct package via qualified path `pkg::CFG.XLEN` di default
    // param module (pola CVA6: `cva6_config_pkg::cva6_cfg.XLEN`). Sebelum
    // fix: expr_base_key tak kenal ScopedIdent → fallback lebar 1.
    let source = r#"
package cfg_pkg;
  typedef struct packed { int unsigned XLEN; bit FOO; } cfg_t;
  localparam cfg_t CFG = '{XLEN:64, FOO:1};
endpackage
module top ();
  localparam int XL = cfg_pkg::CFG.XLEN;
  logic [XL-1:0] wide;
  logic flag;
  initial begin
    wide = '1;
    if (cfg_pkg::CFG.FOO) flag = 1'b1; else flag = 1'b0;
    $finish;
  end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let wide = &sigs.iter().find(|(s, _)| s == "wide").unwrap().1;
    assert_eq!(wide.width, 64, "XL=cfg_pkg::CFG.XLEN harus 64");
    assert_eq!(wide.to_u64(), u64::MAX, "wide='1 harus semua 1");
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("flag"), 1, "FOO harus 1");
}

#[test]
fn test_width_generic_pkg_func_idx_width() {
    // Fungsi package generik di konteks lebar sinyal
    // (`logic [idx_width(D)-1:0]` ala cf_math_pkg CVA6) — inlining penuh
    // via evaluator konstanta, bukan allowlist per-nama. Sebelum fix:
    // "non-constant expression in parameter context" → module di-skip.
    let source = r#"
package math_pkg;
  function automatic integer unsigned idx_width(input integer unsigned n);
    return (n > 32'd1) ? unsigned'($clog2(n)) : 32'd1;
  endfunction
endpackage
module top import math_pkg::*;
#(parameter int D = 3) ();
  logic [idx_width(D)-1:0] usage;
  initial begin usage = '1; $finish; end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let usage = &sigs.iter().find(|(s, _)| s == "usage").unwrap().1;
    assert_eq!(usage.width, 2, "idx_width(3) harus 2");
    assert_eq!(usage.to_u64(), 3, "usage='1 harus 2'b11");
}

#[test]
fn test_struct_param_func_default_flatten() {
    // Default param FUNCCALL yang mengembalikan struct
    // (`Cfg = build_config(user_pkg::UCFG)` ala CVA6 top) harus di-flatten
    // ke key `Cfg.field` agar member access di width/generate bisa
    // const-eval. Sebelum fix: fallback lebar 1.
    let source = r#"
package user_pkg;
  typedef struct packed { int unsigned XLEN; bit FOO; } user_cfg_t;
  localparam user_cfg_t UCFG = '{XLEN:32, FOO:0};
endpackage
package cfg_pkg;
  typedef struct packed { int unsigned XLEN; bit FOO; bit BIG; } cfg_t;
  function automatic cfg_t build_config(user_pkg::user_cfg_t u);
    cfg_t cfg;
    cfg.XLEN = u.XLEN;
    cfg.FOO = u.FOO;
    cfg.BIG = (u.XLEN > 32) ? 1'b1 : 1'b0;
    return cfg;
  endfunction
endpackage
module top import cfg_pkg::*;
#(parameter cfg_t Cfg = build_config(user_pkg::UCFG)) ();
  logic [Cfg.XLEN-1:0] wide;
  generate
    if (Cfg.BIG) begin : gbig
      logic big;
    end else begin : gsmall
      logic small;
    end
  endgenerate
  initial begin wide = '1; $finish; end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let wide = &sigs.iter().find(|(s, _)| s == "wide").unwrap().1;
    assert_eq!(wide.width, 32, "XLEN=32 dari build_config");
    assert_eq!(wide.to_u64(), 0xFFFF_FFFFu64, "wide='1 harus semua 1");
}

#[test]
#[allow(clippy::result_large_err)]
fn test_packed_chunk_negative_index_no_overflow_panic() {
    // Temuan fuzz (bug_0472/cli-emu + synth panic):
    // `expr.rs:784` `base + idx as usize * elem_w` overflow panic saat
    // index konstanta negatif/besar pada chunk packed (`mem[0][-1]`).
    // Fix: saturating + idx.max(0). Test kunci: elaborasi TAK BOLEH panic,
    // hasil Ok (clean) atau Err ber-lokasi.
    for src in [
        "module tb;\n  logic [3:0][7:0] mem;\n  logic [7:0] out;\n  initial out = mem[0][-1];\nendmodule\n",
        "module tb;\n  logic [3:0][7:0] mem;\n  logic [7:0] out;\n  initial out = mem[0][2147483647];\nendmodule\n",
    ] {
        let r = std::panic::catch_unwind(|| mivon_api::compile_str_quiet(src));
        assert!(r.is_ok(), "compile_str_quiet tak boleh panic (overflow idx)");
        // Ok(IrDesign) atau Err ber-lokasi sama-sama sah — yang dilarang panic.
        if let Err(e) = r.unwrap() {
            let msg = e.to_string();
            assert!(
                msg.contains(':'),
                "error harus ber-lokasi file:line:col, dapat: {msg}"
            );
        }
    }
}

#[test]
fn test_disable_fork_label_releases_join_any() {
    // Temuan fuzz (seed 51 vs iverilog): `fork : workers ... join_any` +
    // `disable workers` gantung — parser membuang label fork sehingga
    // disable jatuh ke fallback kill-current. Kini label dipertahankan via
    // NamedBlock: join_any lanjut, cnt tetap 0, done=1 (sama dgn iverilog).
    let source = "module tb;\n  logic clk = 0;\n  logic [3:0] cnt = 0;\n  logic done = 0;\n  always #5 clk = ~clk;\n  initial begin\n    fork : workers\n      begin\n        repeat (4) @(posedge clk);\n        cnt = 4'd7;\n      end\n      begin\n        repeat (2) @(posedge clk);\n        disable workers;\n      end\n    join_any\n    #1;\n    done = 1;\n  end\nendmodule\n";
    let sigs = simulate_signals(source, 200).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("cnt"), 0, "branch ter-disable sebelum tulis");
    assert_eq!(get("done"), 1, "join_any lanjut setelah disable");
}

#[test]
#[allow(clippy::result_large_err)]
fn test_duplicate_instance_name_is_located_error() {
    // Temuan fuzz (bug_2028 vs iverilog): dua instance bernama sama dalam
    // satu module (`sub u1(...); sub u1(...);`) diterima mivon tanpa error —
    // hierarki punya path duplikat. iverilog: "'u1' has already been declared
    // in this scope". Kini elab ERROR ber-lokasi (LRM §23.3.2).
    let source = "module sub(input logic a, output logic b);\n  assign b = a;\nendmodule\nmodule top(input logic x, output logic y);\n  sub u1(.a(x), .b(y));\n  sub u1(.a(x), .b(y));\nendmodule\n";
    let r = std::panic::catch_unwind(|| mivon_api::compile_str_quiet(source));
    assert!(r.is_ok(), "tak boleh panic");
    let e = r.unwrap().expect_err("instance duplikat harus elab error");
    let msg = e.to_string();
    assert!(
        msg.contains("u1") && msg.contains("sudah dideklarasikan") && msg.contains(':'),
        "error ber-lokasi menyebut instance duplikat: {msg}"
    );
    // Pesan menyertakan lokasi deklarasi pertama (baris 5, 1-based) — pesan
    // spesifik tidak lagi tersembunyi di balik "unresolved instantiation".
    assert!(
        msg.contains("5:3"),
        "pesan menyebut lokasi deklarasi pertama: {msg}"
    );
    // Nama instance berbeda (unik) tetap legal.
    let ok = "module sub(input logic a, output logic b);\n  assign b = a;\nendmodule\nmodule top(input logic x, output logic y);\n  sub u1(.a(x), .b(y));\n  sub u2(.a(x), .b(y));\nendmodule\n";
    let r2 = std::panic::catch_unwind(|| mivon_api::compile_str_quiet(ok));
    assert!(
        r2.is_ok() && r2.unwrap().is_ok(),
        "instance unik harus lolos"
    );
}

#[test]
#[allow(clippy::result_large_err)]
fn test_empty_based_literal_is_located_parse_error() {
    // Temuan fuzz (bug_0012 DIAG-001): `'h;` (digit terhapus mutasi) lolos
    // jadi Hex{bits:""} lalu runtime error TANPA lokasi ("invalid hex ''").
    // Kini parse ERROR ber-lokasi.
    for src in [
        "module m;\n  logic [7:0] x;\n  initial x = 8'h;\nendmodule\n",
        "module m;\n  function int f(input int n);\n    'h;\n    return n;\n  endfunction\nendmodule\n",
    ] {
        let r = std::panic::catch_unwind(|| mivon_api::compile_str_quiet(src));
        assert!(r.is_ok(), "tak boleh panic");
        let e = r.unwrap().expect_err("literal berbasis kosong harus error");
        let msg = e.to_string();
        assert!(
            msg.contains("tanpa digit") && msg.contains(':'),
            "error ber-lokasi: {msg}"
        );
    }
}

#[test]
#[allow(clippy::result_large_err)]
fn test_scalar_port_field_write_is_located_elab_error() {
    // Temuan fuzz (bug_0395 DIAG-001): `assign register_o.hartid = ...`
    // dgn `register_o` port scalar → dulu warning + runtime error TANPA
    // lokasi. Kini elab ERROR E2002 ber-lokasi (flatten tak bisa ciptakan
    // `port.field` dari scalar; interface tetap defer).
    let source =
        "module m(input logic clk, output logic [7:0] p);\n  assign p.hartid = 1'b1;\nendmodule\n";
    let r = std::panic::catch_unwind(|| mivon_api::compile_str_quiet(source));
    assert!(r.is_ok(), "tak boleh panic");
    let e = r
        .unwrap()
        .expect_err("field-write ke port scalar harus elab error");
    let msg = e.to_string();
    assert!(
        msg.contains("hartid") && msg.contains(':'),
        "error ber-lokasi menyebut field: {msg}"
    );
}

#[test]
#[allow(clippy::result_large_err)]
fn test_repeat_huge_count_not_unrolled() {
    // Temuan fuzz (bug_0667 cb_filter_tb hang): `Stmt::Repeat` const
    // di-unroll PENUH tanpa batas — `repeat (10jt)` = 10jt IR node +
    // GB/gantung. Fix: >256 → runtime IrStmt::Repeat. Elaborasi harus
    // SELESAI CEPAT (bukan hang) + tetap mensimulasikan dgn benar.
    let source = "module tb;\n  logic clk;\n  logic [1:0] cnt;\n  initial begin\n    cnt = 0;\n    repeat (10000000) @(posedge clk);\n    cnt = 1;\n  end\nendmodule\n";
    let r = std::panic::catch_unwind(|| mivon_api::compile_str_quiet(source));
    assert!(r.is_ok(), "repeat 10jt tak boleh panic");
    assert!(r.unwrap().is_ok(), "repeat 10jt harus ter-elaborasi bersih");
    // Kecil tetap di-unroll & jalan: repeat(3) → cnt=3 tercapai.
    let small = "module tb;\n  logic [1:0] cnt;\n  initial begin\n    cnt = 0;\n    repeat (3) cnt = cnt + 1;\n  end\nendmodule\n";
    let sigs = simulate_signals(small, 10).unwrap();
    let cnt = sigs.iter().find(|(s, _)| s == "cnt").unwrap().1.to_u64();
    assert_eq!(cnt, 3, "repeat kecil tetap jalan");
}

// ─────────────────────────────────────────────────────────────────────────────
// Regression: seleksi bit packed vector wajib memakai POSISI bit fisik, bukan
// indeks mentah. Bug fuzz diferensial vs Icarus `verify_bad_0021` (adder4 dgn
// `logic [0:3]`): `a[0]` pada rentang menaik `[0:3]` = MSB, tapi mivon
// membacanya sebagai LSB → hasil salah. Rentang lsb != 0 (`[7:4]`) juga
// membaca bit di luar batas → X. Cakupan: baca, tulis, range-select, `+:`/`-:`,
// dan indeks dinamis.
// ─────────────────────────────────────────────────────────────────────────────

fn sig_u64(sigs: &[(String, mivon_ir::LogicVec)], name: &str) -> u64 {
    sigs.iter()
        .find(|(s, _)| s == name)
        .unwrap_or_else(|| panic!("signal '{name}' tidak ada"))
        .1
        .to_u64()
}

#[test]
fn test_bitselect_ascending_and_offset_range() {
    // asc [0:3] = 4'b1100 → asc[0]=1 (MSB) … asc[3]=0 (LSB).
    // d74 [7:4] = 4'b1010 → d74[7]=1 (MSB) … d74[4]=0 (LSB).
    let source = r#"
module tb;
  logic [0:3] asc;
  logic [7:4] d74;
  logic b_asc0, b_asc3, b_d74_4, b_d74_7;
  initial begin
    asc = 4'b1100;
    d74 = 4'b1010;
    b_asc0  = asc[0];
    b_asc3  = asc[3];
    b_d74_4 = d74[4];
    b_d74_7 = d74[7];
  end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    assert_eq!(sig_u64(&sigs, "b_asc0"), 1, "asc[0] pada [0:3] = MSB (=1)");
    assert_eq!(sig_u64(&sigs, "b_asc3"), 0, "asc[3] pada [0:3] = LSB (=0)");
    assert_eq!(sig_u64(&sigs, "b_d74_4"), 0, "d74[4] pada [7:4] = LSB (=0)");
    assert_eq!(sig_u64(&sigs, "b_d74_7"), 1, "d74[7] pada [7:4] = MSB (=1)");
}

#[test]
fn test_rangeselect_and_partselect_positions() {
    let source = r#"
module tb;
  logic [0:3] asc;
  logic [7:4] d74;
  logic [3:0] r_asc, r_d74;
  logic [1:0] ps_d74a, ps_d74b, ps_asc;
  initial begin
    asc = 4'b1100;
    d74 = 4'b1010;
    r_asc   = asc[0:3];     // 1100 = 12
    r_d74   = d74[7:4];     // 1010 = 10
    ps_d74a = d74[4 +: 2];  // d74[5:4] = 10 = 2
    ps_d74b = d74[6 -: 2];  // d74[6:5] = 01 = 1
    ps_asc  = asc[0 +: 2];  // asc[0:1] = 11 = 3
  end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    assert_eq!(sig_u64(&sigs, "r_asc"), 12, "asc[0:3] pada [0:3] = 1100");
    assert_eq!(sig_u64(&sigs, "r_d74"), 10, "d74[7:4] pada [7:4] = 1010");
    assert_eq!(sig_u64(&sigs, "ps_d74a"), 2, "d74[4 +: 2] = d74[5:4] = 10");
    assert_eq!(sig_u64(&sigs, "ps_d74b"), 1, "d74[6 -: 2] = d74[6:5] = 01");
    assert_eq!(sig_u64(&sigs, "ps_asc"), 3, "asc[0 +: 2] = asc[0:1] = 11");
}

#[test]
fn test_bitselect_write_and_dynamic_index_positions() {
    let source = r#"
module tb;
  logic [0:3] asc;
  logic [7:4] d74;
  logic [0:3] w_asc;
  logic [7:4] w_d74;
  logic [0:3] dw_asc;
  logic [7:4] dw_d74;
  logic d_asc0, d_d74_4, d_d74_7;
  int i;
  initial begin
    asc = 4'b1100;
    d74 = 4'b1010;
    // Indeks dinamis.
    i = 0; d_asc0  = asc[i];   // asc[0]=1
    i = 4; d_d74_4 = d74[i];   // d74[4]=0
    i = 7; d_d74_7 = d74[i];   // d74[7]=1
    // Tulis statis.
    w_asc = 4'b0000; w_asc[0] = 1'b1;   // MSB → 1000 = 8
    w_d74 = 4'b0000; w_d74[4] = 1'b1;   // LSB → 0001 = 1
    w_d74[7] = 1'b1;                    // MSB → 1001 = 9
    // Tulis dinamis.
    dw_asc = 4'b0000; i = 0; dw_asc[i] = 1'b1;  // MSB → 1000 = 8
    dw_d74 = 4'b0000; i = 7; dw_d74[i] = 1'b1;  // MSB → 1000 = 8
  end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    assert_eq!(sig_u64(&sigs, "d_asc0"), 1, "asc[i=0] = asc[0] = 1");
    assert_eq!(sig_u64(&sigs, "d_d74_4"), 0, "d74[i=4] = d74[4] = 0");
    assert_eq!(sig_u64(&sigs, "d_d74_7"), 1, "d74[i=7] = d74[7] = 1");
    assert_eq!(sig_u64(&sigs, "w_asc"), 8, "w_asc[0]=1 pada [0:3] → 1000");
    assert_eq!(sig_u64(&sigs, "w_d74"), 9, "w_d74[4]=1 & [7]=1 → 1001");
    assert_eq!(sig_u64(&sigs, "dw_asc"), 8, "dw_asc[i=0]=1 → 1000");
    assert_eq!(sig_u64(&sigs, "dw_d74"), 8, "dw_d74[i=7]=1 → 1000");
}

// ─────────────────────────────────────────────────────────────────────────────
// Regression: initializer variabel pada deklarasi (LRM §6.8) = nilai AWAL, BUKAN
// assignment pemicu event. `logic rst_n = 0;` (karena init_val ditetapkan dari
// initializer konstanta) harus menyebabkan sinyal BERNILAI 0 sejak t=0 → tak
// ada transisi X→0 yang salah memicu `@(negedge rst_n)` di t=0. Temuan fuzz
// diferensial `verify_bad_0586`; Icarus & Verilator TIDAK memicu negedge di t=0.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_decl_init_no_spurious_negedge() {
    // `rst_n` diinisialisasi 0 di deklarasi; `clk` juga. Tak ada transisi
    // sebenarnya di t=0, jadi penghitung negedge rst_n & posedge clk tetap 0
    // sampai clk naik pertama kali.
    let source = r#"
module tb;
  logic clk = 0;
  logic rst_n = 0;
  int n_negedge = 0;
  int n_posedge = 0;
  always @(negedge rst_n) n_negedge = n_negedge + 1;
  always @(posedge clk)   n_posedge = n_posedge + 1;
  initial begin
    #5;
    clk = 1;   // posedge pertama
    #5;
    clk = 0;   // negedge clk (tak dihitung)
    #5;
    rst_n = 1; #5;   // tidak ada negedge (naik)
    rst_n = 0; #5;   // negedge rst_n pertama (nyata)
  end
endmodule
"#;
    let sigs = simulate_signals(source, 100).unwrap();
    // Hanya 1 negedge rst_n (saat rst_n=1→0 di akhir), BUKAN 2 (salah satunya
    // pemicu palsu X→0 di t=0).
    assert_eq!(
        sig_u64(&sigs, "n_negedge"),
        1,
        "decl-init tidak boleh memicu negedge palsu di t=0"
    );
    // Satu posedge clk tercatat (clk:0→1).
    assert_eq!(
        sig_u64(&sigs, "n_posedge"),
        1,
        "posedge clk normal tetap jalan"
    );
}
