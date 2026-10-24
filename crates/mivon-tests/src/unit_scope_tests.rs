use super::*;

// REGRESI LANG-41: item pada compilation-unit scope (IEEE 1800-2017 §3.2).
//
// Sebelumnya deklarasi di luar module:
//   nettype logic [7:0] mynet;   -> "unknown type 'mynet'"
//   checker ... endchecker       -> instance checker tidak ter-resolve
//   let ... = ...;               -> dibuang parser (hanya module/class)
// Sekarang ketiganya disimpan di Design::unit_items dan di-seed ke elaborator
// untuk SETIAP module.

fn sim(source: &str, max_time: u64) -> Vec<(String, mivon_ir::LogicVec)> {
    simulate_signals(source, max_time).unwrap_or_else(|e| panic!("sim gagal: {e}\n{source}"))
}

fn get_u64(sigs: &[(String, mivon_ir::LogicVec)], name: &str) -> u64 {
    sigs.iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("signal tidak ada: {name}"))
        .1
        .to_u64()
}

#[test]
fn unit_scope_nettype_visible_in_module() {
    let source = r#"
nettype logic [7:0] mynet;

module tb;
    mynet x;
    int v;
    initial begin
        x = 8'hAB;
        v = x;
        #1 $finish;
    end
endmodule
"#;
    let sigs = sim(source, 5);
    assert_eq!(
        get_u64(&sigs, "v"),
        0xAB,
        "nettype unit scope: mynet = logic[7:0], x=0xAB"
    );
}

#[test]
fn unit_scope_nettype_shared_by_two_modules() {
    let source = r#"
nettype logic [3:0] nibble;

module producer(output nibble o);
    initial o = 4'hC;
endmodule

module tb;
    nibble n;
    int v;
    producer u(.o(n));
    initial begin
        #1 v = n;
        #2 $finish;
    end
endmodule
"#;
    let sigs = sim(source, 10);
    assert_eq!(
        get_u64(&sigs, "v"),
        0xC,
        "nettype unit scope terlihat di dua module sekaligus"
    );
}

#[test]
fn unit_scope_checker_instance() {
    let source = r#"
checker unit_chk(input clk, input [3:0] v);
    assert property (@(posedge clk) v <= 3);
endchecker

module tb;
    reg clk;
    reg [3:0] cnt = 0;
    initial begin
        clk = 0;
        forever #5 clk = ~clk;
    end
    always @(posedge clk) cnt <= cnt + 1;
    unit_chk u_chk(.clk(clk), .v(cnt));
    initial #50 $finish;
endmodule
"#;
    let design = compile_str(source).expect("compile checker unit scope");
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    let total_pass: u64 = engine.assertion_stats.values().map(|(p, _)| *p).sum();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert!(
        total_pass >= 3,
        "checker unit scope: assertion pass={total_pass}"
    );
    assert!(
        total_fail >= 1,
        "checker unit scope: assertion fail saat cnt>3={total_fail}"
    );
}

#[test]
fn unit_scope_let_visible_in_module() {
    let source = r#"
let W = 8;
let double(x) = x * 2;

module tb;
    int r1;
    int r2;
    initial begin
        r1 = W;
        r2 = double(21);
        #1 $finish;
    end
endmodule
"#;
    let sigs = sim(source, 5);
    assert_eq!(get_u64(&sigs, "r1"), 8, "let unit scope: W = 8");
    assert_eq!(get_u64(&sigs, "r2"), 42, "let unit scope: double(21) = 42");
}

#[test]
fn unit_scope_let_overridden_by_module() {
    let source = r#"
let W = 8;

module tb;
    let W = 3;
    int r;
    initial begin
        r = W;
        #1 $finish;
    end
endmodule
"#;
    let sigs = sim(source, 5);
    assert_eq!(
        get_u64(&sigs, "r"),
        3,
        "let module menimpa let compilation-unit"
    );
}
