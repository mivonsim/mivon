use super::*;

// REGRESI LANG-42: deklarasi SVA bernama (IEEE 1800-2017 §16.12).
//
// Sebelumnya `property`/`sequence` hanya di-skip parser (baik di module
// maupun compilation-unit scope) dan `assert property (... p)` yang
// mereferensikan nama gagal resolve (dianggap signal tak dikenal).
// Sekarang deklarasi diparse ke registry parser; pemakaian di-inline dengan
// substitusi parameter formal, dan clock di deklarasi dipakai bila assertion
// tidak punya clock sendiri.

fn run(source: &str, max_time: u64) -> (u64, u64) {
    let design = compile_str(source).expect("compile");
    let mut engine = crate::simulator::SimulationEngine::new(design, max_time);
    let _ = engine.run();
    let pass: u64 = engine.assertion_stats.values().map(|(p, _)| *p).sum();
    let fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    (pass, fail)
}

#[test]
fn perf03_lazy_snapshot_refresh_concurrent_sequence() {
    // PERF-03: lazy signal snapshot must still refresh when a concurrent
    // SVA sequence attempt is pending, otherwise posedge detection falls
    // back to "clk is high" and fires every cycle.
    let source = r#"
module tb;
  reg clk = 0;
  reg a = 0;
  reg b = 0;
  always #5 clk = ~clk;
  assert property (@(posedge clk) a ##1 b);
  initial begin
    #12 a = 1;
    #10 b = 1;
    #40; $finish;
  end
endmodule
"#;
    let (pass, fail) = run(source, 60);
    println!("LAZY_TEST pass={pass} fail={fail}");
    assert!(
        pass >= 1,
        "lazy snapshot: concurrent sequence match: pass={pass} fail={fail}"
    );
}

#[test]
fn named_property_module_scope_boolean() {
    let source = r#"
module tb;
    reg clk = 0;
    reg a = 0;
    property p_a;
        a;
    endproperty
    always #5 clk = ~clk;
    assert property (@(posedge clk) p_a);
    initial begin
        @(posedge clk); a = 1;
        #20 $finish;
    end
endmodule
"#;
    let (pass, fail) = run(source, 60);
    assert!(pass >= 2, "named property boolean: pass={pass}");
    assert!(fail >= 1, "named property boolean: fail={fail}");
}

#[test]
fn named_property_unit_scope_with_decl_clock() {
    let source = r#"
property p_clk;
    @(posedge clk) cnt <= 3;
endproperty

module tb;
    reg clk = 0;
    reg [3:0] cnt = 0;
    always #5 clk = ~clk;
    assert property (p_clk);
    always @(posedge clk) cnt <= cnt + 1;
    initial #50 $finish;
endmodule
"#;
    let (pass, fail) = run(source, 60);
    assert!(pass >= 2, "named property decl clock: pass={pass}");
    assert!(
        fail >= 1,
        "named property decl clock: fail saat cnt>3={fail}"
    );
}

#[test]
fn named_property_parameter_substitution() {
    let source = r#"
property p_lim(v, n);
    v <= n;
endproperty

module tb;
    reg clk = 0;
    reg [3:0] cnt = 0;
    always #5 clk = ~clk;
    assert property (@(posedge clk) p_lim(cnt, 4'd1));
    always @(posedge clk) cnt <= cnt + 1;
    initial #50 $finish;
endmodule
"#;
    let (pass, fail) = run(source, 60);
    assert!(pass >= 1, "substitusi parameter: pass={pass}");
    assert!(
        fail >= 2,
        "substitusi parameter: cnt>1 harus fail, fail={fail}"
    );
}

#[test]
fn named_sequence_block_form_with_args() {
    let source = r#"
sequence s_ab(a, b);
    a ##1 b;
endsequence

module tb;
    reg clk = 0;
    reg a = 0;
    reg b = 0;
    always #5 clk = ~clk;
    assert property (@(posedge clk) s_ab(a, b));
    initial begin
        @(posedge clk); a = 1;
        @(posedge clk); b = 1;
        @(posedge clk); a = 0; b = 0;
        #40 $finish;
    end
endmodule
"#;
    let (pass, fail) = run(source, 80);
    assert!(
        pass >= 1,
        "sequence blok dengan argumen harus match, pass={pass} fail={fail}"
    );
}

#[test]
fn named_sequence_shorthand_form() {
    let source = r#"
sequence s_eq = a ##1 b;

module tb;
    reg clk = 0;
    reg a = 0;
    reg b = 0;
    always #5 clk = ~clk;
    assert property (@(posedge clk) s_eq);
    initial begin
        @(posedge clk); a = 1;
        @(posedge clk); b = 1;
        @(posedge clk); a = 0; b = 0;
        #40 $finish;
    end
endmodule
"#;
    let (pass, _fail) = run(source, 80);
    assert!(
        pass >= 1,
        "sequence bentuk ringkas harus match, pass={pass}"
    );
}
