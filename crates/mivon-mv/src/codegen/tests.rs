//! Codegen — unit tests (dipindah dari codegen.rs saat pemecahan struktur).
//! Di-include oleh `codegen/mod.rs` (`#[cfg(test)] mod tests;`).

use super::*;
use crate::parser::parse;

#[test]
fn codegen_counter() {
    let src = r#"
module counter #(WIDTH = 8) {
    in  clk, rst_n : bit
    in  enable     : bit
    out count      : logic[WIDTH-1:0]
    seq(clk, rst_n) {
        if (!rst_n) {
            count <= '0
        } else if (enable) {
            count <= count + 1
        }
    }
}
"#;
    let file = parse(src).unwrap();
    let out = generate(&file, "counter");
    assert!(out.sv.contains("module counter"));
    assert!(out.sv.contains("parameter WIDTH = 8"));
    assert!(out.sv.contains("input  bit clk"));
    assert!(out.sv.contains("output logic [WIDTH - 1:0] count"));
    assert!(out
        .sv
        .contains("always_ff @(posedge clk or negedge rst_n) begin"));
    assert!(out.sv.contains("count <= '0;"));
    assert!(out.sv.contains("count <= count + 1;"));
    assert!(out.sv.contains("endmodule"));
}

#[test]
fn codegen_traffic() {
    let src = r#"
package traffic_pkg {
    enum State { RED, GREEN, YELLOW }
}

module traffic #(GREEN_T = 30, YELLOW_T = 5) {
    use traffic_pkg::*
    in  clk, rst_n : bit
    out state      : State
    out red, green, yellow : bit

    reg state : State
    reg timer : uint[8]

    seq(clk, rst_n) {
        if (!rst_n) {
            state <= RED
            timer <= 0
        } else {
            case (state) {
                RED: {
                    state <= GREEN
                    timer <= 0
                }
                default: {
                    state <= RED
                }
            }
        }
    }

    comb {
        red    = (state == RED)
        green  = (state == GREEN)
        yellow = (state == YELLOW)
    }
}
"#;
    let file = parse(src).unwrap();
    let out = generate(&file, "traffic");
    assert!(out.svh.contains("package traffic_pkg;"));
    assert!(out
        .svh
        .contains("typedef enum logic [1:0] { RED, GREEN, YELLOW } State;"));
    assert!(out.svh.contains("`endif"));
    assert!(out.sv.contains("`include \"traffic.svh\""));
    assert!(out.sv.contains("import traffic_pkg::*;"));
    assert!(out.sv.contains("output State state"));
    assert!(out.sv.contains("case (state)"));
    assert!(out.sv.contains("default: begin"));
    assert!(!out.sv.contains("State state;"));
}

#[test]
fn codegen_instance_and_generate() {
    let src = r#"
module shiftreg #(N : int = 8) {
    in clk : bit
    in d   : bit
    out q  : logic[N-1:0]
    seq(clk) {
        q[0] <= d
    }
    for i in 1..N {
        seq(clk) {
            q[i] <= q[i-1]
        }
    }
    inst counter_small u_small (.clk, .rst_n, .count(q[3:0]))
}
"#;
    let file = parse(src).unwrap();
    let out = generate(&file, "shiftreg");
    assert!(out.sv.contains("parameter int N = 8"));
    assert!(out
        .sv
        .contains("for (genvar i = 1; i < N; i = i + 1) begin : gen_i"));
    assert!(out.sv.contains("q[i] <= q[i - 1];"));
    assert!(out.sv.contains("counter_small u_small ("));
    assert!(out.sv.contains(".clk   (clk),"));
    assert!(out.sv.contains(".rst_n (rst_n),"));
    assert!(out.sv.contains(".count (q[3:0])"));
}

#[test]
fn codegen_types() {
    let src = r#"
type Addr = logic[15:0]
packed struct Packet {
    valid : bit,
    addr  : Addr,
    data  : logic[31:0]
}
enum(3) Color { RED = 0, GREEN = 2, BLUE = 4 }
"#;
    let file = parse(src).unwrap();
    let out = generate(&file, "types");
    assert!(out.svh.contains("typedef logic [15:0] Addr;"));
    assert!(out.svh.contains("typedef struct packed {"));
    assert!(out
        .svh
        .contains("typedef enum logic [2:0] { RED = 0, GREEN = 2, BLUE = 4 } Color;"));
}

#[test]
fn codegen_func_task() {
    let src = r#"
func clog2(x : int) -> int {
    var r : int = 0
    var n : int = x - 1
    while (n > 0) {
        r = r + 1
        n = n >> 1
    }
    return r
}
task send(data : logic[7:0], out ok : bit) {
    #10
    ok = 1
}
"#;
    let file = parse(src).unwrap();
    let out = generate(&file, "util");
    assert!(out.sv.contains("function int clog2(input int x);"));
    assert!(out.sv.contains("int r = 0;"));
    assert!(out.sv.contains("return r;"));
    assert!(out
        .sv
        .contains("task send(inout logic [7:0] data, output bit ok);"));
    assert!(out.sv.contains("#10 ok = 1;"));
    assert!(!out.sv.contains("#10 begin"));
}

#[test]
fn codegen_assert_property() {
    let src = r#"
module m {
    in clk, enable : bit
    in count       : logic[7:0]
    initial {
        assert property (@(posedge clk) enable |-> count == $past(count) + 1)
    }
}
"#;
    let file = parse(src).unwrap();
    let out = generate(&file, "m");
    assert!(
        out.sv
            .contains("assert property (@(posedge clk) enable |-> count == $past(count) + 1);"),
        "sv: {}",
        out.sv
    );
}

#[test]
fn codegen_testbench() {
    let src = r#"
module counter #(WIDTH = 8) {
    in  clk, rst_n : bit
    in  enable     : bit
    out count      : logic[WIDTH-1:0]
    seq(clk, rst_n) {
        if (!rst_n) {
            count <= '0
        } else if (enable) {
            count <= count + 1
        }
    }
}

module tb_counter {
    in clk, rst_n : bit
    in enable     : bit
    in count      : logic[7:0]

    initial {
        $display("tb_counter: mulai")
        clk = 0
        rst_n = 0
        #20
        rst_n = 1
        enable = 1
        repeat (300) @(posedge clk)
        assert (count > 0) $info("counter ok") else $fatal("counter stuck")
        $finish
    }

    initial {
        forever #5 clk = ~clk
    }
}
"#;
    let file = parse(src).unwrap();
    let out = generate(&file, "tb_counter");
    assert!(out.sv.contains("$display(\"tb_counter: mulai\");"));
    assert!(out.sv.contains("$finish;"));
    assert!(out.sv.contains("forever #5 clk = ~clk;"));
    assert!(out
        .sv
        .contains("assert (count > 0) $info(\"counter ok\") else $fatal(\"counter stuck\");"));
}

#[test]
fn codegen_program() {
    let src = r#"
program test_runner {
    in clk : bit
    initial {
        run_test("my_test")
    }
}
"#;
    let file = parse(src).unwrap();
    assert_eq!(file.programs.len(), 1);
    let out = generate(&file, "tb");
    assert!(out.sv.contains("program test_runner ("));
    assert!(out.sv.contains("input  bit clk"));
    assert!(out.sv.contains("initial begin"));
    assert!(out.sv.contains("run_test(\"my_test\");"));
    assert!(out.sv.contains("endprogram"));
}

#[test]
fn codegen_class_uvm() {
    let src = r#"
class my_test extends uvm_test {
    field count     : uint
    rand field seed : uint
    constraint c { seed > 10, seed < 200 }
    func new(name : string) {
        super.new(name)
    }
    func build_phase() {
        uvm_config_db::set(this, "*.agent", "count", count)
    }
    task run_phase() {
        var seqr : uvm_sequencer
        seqr.start_item(item)
        seqr.finish_item(item)
        #100
    }
}
"#;
    let file = parse(src).unwrap();
    let out = generate(&file, "my_test");
    assert!(out.sv.contains("class my_test extends uvm_test;"));
    assert!(out.sv.contains("logic [31:0] count;"));
    assert!(out.sv.contains("rand logic [31:0] seed;"));
    assert!(out.sv.contains("constraint c {"));
    assert!(out.sv.contains("seed > 10;"));
    assert!(out.sv.contains("seed < 200;"));
    assert!(out.sv.contains("function new(input string name);"));
    assert!(out.sv.contains("super.new(name);"));
    assert!(out
        .sv
        .contains("uvm_config_db::set(this, \"*.agent\", \"count\", count);"));
    assert!(out.sv.contains("function void build_phase();"));
    assert!(out.sv.contains("task run_phase();"));
    assert!(out.sv.contains("uvm_sequencer seqr;"));
    assert!(out.sv.contains("seqr.start_item(item);"));
    assert!(out.sv.contains("seqr.finish_item(item);"));
    assert!(out.sv.contains("#100;"));
    assert!(out.sv.contains("endclass"));
}

#[test]
fn codegen_class_plain() {
    let src = r#"
class counter_model {
    field value : uint
    func new() {
        value = 0
    }
    task tick() {
        value = value + 1
    }
}
"#;
    let file = parse(src).unwrap();
    let out = generate(&file, "model");
    assert!(out.sv.contains("class counter_model;"));
    assert!(out.sv.contains("function new();"));
    assert!(out.sv.contains("task tick();"));
    assert!(out.sv.contains("value = value + 1;"));
    assert!(out.sv.contains("endclass"));
}

#[test]
fn codegen_generate_decl_not_dropped() {
    let src = r#"
module pipe #(N : int = 4) {
    in clk : bit
    in d   : bit
    out q  : logic[N-1:0]
    for i in 1..N {
        reg tmp : bit
        seq(clk) {
            q[i] <= d
        }
    }
}
"#;
    let file = parse(src).unwrap();
    let out = generate(&file, "pipe");
    assert!(out.sv.contains("        bit tmp;"));
    assert!(out.sv.contains("        always_ff @(posedge clk) begin"));
    assert!(out.sv.contains("            q[i] <= d;"));
}

#[test]
fn codegen_constraint_advanced() {
    let src = r#"
class item {
    rand field mode : uint[2]
    rand field addr : uint[8]
    rand field data : uint[8]
    constraint c_adv {
        addr inside {[1:10], 20, 30},
        data dist { 0 := 1, [1:5] :/ 9 },
        if (mode == 1) { addr > 5 } else { addr < 100 },
        solve addr before data
    }
}
"#;
    let file = parse(src).unwrap();
    let out = generate(&file, "item");
    assert!(out.sv.contains("constraint c_adv {"), "sv: {}", out.sv);
    assert!(out.sv.contains("addr inside {[1:10], 20, 30};"));
    assert!(out.sv.contains("data dist {0 := 1, [1:5] :/ 9};"));
    assert!(out.sv.contains("if (mode == 1) {"), "sv: {}", out.sv);
    assert!(out.sv.contains("addr > 5;"));
    assert!(out.sv.contains("} else {"));
    assert!(out.sv.contains("addr < 100;"));
    assert!(out.sv.contains("solve addr before data;"));
}

#[test]
fn codegen_interface() {
    let src = r#"
interface axi_lite {
    in  clk : bit
    sig awaddr  : logic[31:0]
    sig awvalid : bit
    sig awready : bit

    modport slave {
        in  awaddr, awvalid
        out awready
    }
}

module dut {
    in axi_if : axi_lite
    out done  : bit
    sig cnt : logic[3:0]
    seq(axi_if.clk) {
        if (axi_if.awvalid && axi_if.awready) {
            cnt <= cnt + 1
            done <= 1
        }
    }
}
"#;
    let file = parse(src).unwrap();
    let out = generate(&file, "axi");
    assert!(out.svh.contains("interface axi_lite;"));
    assert!(out.svh.contains("logic [31:0] awaddr;"));
    assert!(out
        .svh
        .contains("modport slave (input awaddr, awvalid, output awready);"));
    assert!(out.svh.contains("endinterface"));
    assert!(out.sv.contains("`include \"axi.svh\""));
    assert!(out.sv.contains("module dut ("));
    assert!(out.sv.contains("    axi_lite axi_if,"), "sv: {}", out.sv);
    assert!(out.sv.contains("output bit done"));
    assert!(out.sv.contains("always_ff @(posedge axi_if.clk) begin"));
    assert!(out
        .sv
        .contains("if (axi_if.awvalid && axi_if.awready) begin"));
}

#[test]
fn codegen_interface_only_triggers_svh() {
    let src = "interface bus {\n sig a : logic[7:0]\n modport m { in a }\n}\n";
    let out = generate(&parse(src).unwrap(), "bus");
    assert!(out.svh.contains("interface bus;"), "svh: {}", out.svh);
    assert!(out.svh.contains("`endif"));
    assert!(out.sv.is_empty(), "sv: {}", out.sv);
}

#[test]
fn codegen_sig_iface_emits_instance() {
    let src = r#"
interface axi_lite {
    sig awaddr : logic[31:0]
}

module tb {
    sig bus : axi_lite
    sig done : bit
    initial {
        bus.awaddr = 32'h10
    }
}
"#;
    let out = generate(&parse(src).unwrap(), "axi");
    assert!(out.sv.contains("axi_lite bus();"), "sv: {}", out.sv);
    assert!(!out.sv.contains("axi_lite bus;"), "sv: {}", out.sv);
    assert!(out.sv.contains("bit done;"));
    assert!(out.sv.contains("bus.awaddr = 32'h10;"));
}

#[test]
fn codegen_cross_file_interface_port_no_dir() {
    let src_a = "interface bus {\n sig a : logic[7:0]\n modport m { in a }\n}\n";
    let src_b = "module dut {\n in b : bus\n out y : bit\n comb { y = 0 }\n}\n";
    let fa = parse(src_a).unwrap();
    let fb = parse(src_b).unwrap();
    let ifaces = vec!["bus"];
    let oa = generate_with_ifaces(&fa, "bus_def", &ifaces);
    assert!(oa.svh.contains("interface bus;"));
    let ob = generate_with_ifaces(&fb, "dut", &ifaces);
    assert!(ob.sv.contains("    bus b,"), "sv: {}", ob.sv);
    assert!(
        !ob.sv.contains("input bus b"),
        "port interface tidak boleh ada arah"
    );
    let oc = generate(&fb, "dut");
    assert!(oc.sv.contains("input  bus b"), "fallback: {}", oc.sv);
}

#[test]
fn codegen_deterministic() {
    let src = "module a { in clk : bit\n out y : logic[7:0]\n comb { y = x + 1 } }";
    let file = parse(src).unwrap();
    let a = generate(&file, "a").sv;
    let b = generate(&file, "a").sv;
    assert_eq!(a, b);
}

#[test]
fn codegen_svh_only_when_shared_defs() {
    let src = "module a { in clk : bit\n out y : logic[7:0]\n comb { y = clk } }";
    let out = generate(&parse(src).unwrap(), "a");
    assert!(
        out.svh.is_empty(),
        "svh harus kosong tanpa shared defs: {}",
        out.svh
    );
    assert!(!out.sv.contains("`include"), "sv tidak boleh include svh");

    let src2 = "package pkg { const N = 4 }\nmodule b { in clk : bit\n out y : logic[3:0]\n comb { y = clk } }";
    let out2 = generate(&parse(src2).unwrap(), "b");
    assert!(
        out2.svh.contains("package pkg;"),
        "svh harus berisi package"
    );
    assert!(
        out2.sv.contains("`include \"b.svh\""),
        "sv harus include svh"
    );

    let src3 =
        "type Addr = logic[15:0]\nmodule c { in a : Addr\n out y : bit\n comb { y = a[0] } }";
    let out3 = generate(&parse(src3).unwrap(), "c");
    assert!(
        out3.svh.contains("typedef logic [15:0] Addr;"),
        "svh harus berisi typedef"
    );
    assert!(out3.sv.contains("`include \"c.svh\""));
}

#[test]
fn codegen_type_param() {
    let src = "module m #(T : type = logic[7:0], N = 2) {\n    in  d : T\n    out q : T\n}\n";
    let out = generate(&parse(src).unwrap(), "m");
    assert!(
        out.sv.contains("parameter type T = logic [7:0]"),
        "sv: {}",
        out.sv
    );
    assert!(out.sv.contains("parameter N = 2"), "sv: {}", out.sv);
    assert!(out.sv.contains("input  T d"), "sv: {}", out.sv);
    assert!(out.sv.contains("output T q"), "sv: {}", out.sv);
}

#[test]
fn codegen_type_param_kw_syntax() {
    let src = "module m #(type T = logic[15:0]) {\n    sig x : T\n}\n";
    let out = generate(&parse(src).unwrap(), "m");
    assert!(
        out.sv.contains("parameter type T = logic [15:0]"),
        "sv: {}",
        out.sv
    );
    assert!(out.sv.contains("T x;"), "sv: {}", out.sv);
}

#[test]
fn codegen_cast() {
    let src = "type Word16 = logic[15:0]\nmodule m {\n    in  a : logic[15:0]\n    out w : Word16\n    out b : bit\n    comb {\n        w = Word16'(a)\n        b = logic'(a[0])\n    }\n}\n";
    let out = generate(&parse(src).unwrap(), "m");
    assert!(out.sv.contains("w = Word16'(a);"), "sv: {}", out.sv);
    assert!(out.sv.contains("b = logic'(a[0]);"), "sv: {}", out.sv);
}

#[test]
fn codegen_typedef_local_module() {
    // typedef lokal module: `type X` / `enum E` di badan module → emit
    // typedef scope-lokal SV di indent badan (bukan .svh).
    let src = r#"
module m {
    type Word8 = logic[7:0]
    enum Mode { OFF, ON }
    out val : Word8
    comb {
        val = ON
    }
}
"#;
    let out = generate(&parse(src).unwrap(), "m");
    assert!(
        out.sv.contains("typedef logic [7:0] Word8;"),
        "typedef lokal: {}",
        out.sv
    );
    assert!(
        out.sv
            .contains("typedef enum logic [1:0] { OFF, ON } Mode;"),
        "enum lokal: {}",
        out.sv
    );
    assert!(out.svh.is_empty(), "tanpa definisi bersama, svh kosong");
}

#[test]
fn codegen_foreach_array() {
    // `foreach (rom[i]) { rom[i] = i }` → SV foreach (1 & multi index).
    let src = r#"
module m {
    sig rom : logic[8][4]
    sig mat : logic[8][2][2]
    initial {
        foreach (rom[i]) {
            rom[i] = i
        }
        foreach (mat[i][j]) {
            mat[i][j] = 0
        }
    }
}
"#;
    let out = generate(&parse(src).unwrap(), "m");
    assert!(
        out.sv.contains("foreach (rom[i]) begin"),
        "foreach 1-index: {}",
        out.sv
    );
    assert!(
        out.sv.contains("foreach (mat[i][j]) begin"),
        "foreach multi-index: {}",
        out.sv
    );
}

#[test]
fn codegen_inst_positional_param() {
    // Instance param POSITIONAL `inst fifo #(8) u (...)` — nama kosong dari
    // parser → emit `#(8)` (bukan `.8()`); campur `#(8, .DEPTH(4))` valid SV.
    let src = r#"
module top {
    in clk : bit
    out w   : logic[7:0]
    inst fifo u1 #(8) (.clk(clk), .dout(w))
    inst fifo u2 #(8, .DEPTH(4)) (.clk(clk), .dout(w))
}
"#;
    let out = generate(&parse(src).unwrap(), "top");
    assert!(out.sv.contains("fifo u1 #(8) ("), "positional: {}", out.sv);
    assert!(
        out.sv.contains("fifo u2 #(8, .DEPTH(4)) ("),
        "campur positional+named: {}",
        out.sv
    );
}

#[test]
fn codegen_func_named_arg() {
    // Named call arg `f(10, factor = 3)` → SV `.factor(3)` (order bebas).
    let src = r#"
func scale(v : int, factor : int) -> int {
    return v * factor
}
module m {
    out y : logic[31:0]
    comb {
        y = scale(10, factor = 3)
        y = y + scale(factor = 2, v = 5)
    }
}
"#;
    let out = generate(&parse(src).unwrap(), "m");
    assert!(
        out.sv.contains("y = scale(10, .factor(3));"),
        "named arg: {}",
        out.sv
    );
    assert!(
        out.sv.contains("scale(.factor(2), .v(5))"),
        "named order bebas: {}",
        out.sv
    );
}

#[test]
fn codegen_func_default_arg() {
    // Default arg function/task: `b : int = 4` → `input int b = 4`.
    let src = r#"
func scale(v : int, factor : int = 2) -> int {
    return v * factor
}
task send(data : logic[7:0], tag : logic[3:0] = 4'h0) {
    #1
    data = 0
}
"#;
    let out = generate(&parse(src).unwrap(), "util");
    assert!(
        out.sv
            .contains("function int scale(input int v, input int factor = 2);"),
        "func default: {}",
        out.sv
    );
    assert!(
        out.sv
            .contains("task send(inout logic [7:0] data, inout logic [3:0] tag = 4'h0);"),
        "task default: {}",
        out.sv
    );
}

#[test]
fn f45_wait_fork_disable_fork_codegen() {
    // F45: `wait fork;` + `disable fork;` di-emit 1:1 ke SV.
    let src = r#"
module m {
    sig a : logic[7:0]
    initial {
        fork {
            #10
            a = 1
        } join_none
        wait fork
        disable fork
    }
}
"#;
    let out = generate(&parse(src).unwrap(), "m");
    assert!(out.sv.contains("wait fork;"), "wait fork: {}", out.sv);
    assert!(out.sv.contains("disable fork;"), "disable fork: {}", out.sv);
}

#[test]
fn f46_force_release_codegen() {
    // F46: `force a = 8'd99;` + `release a;` di-emit 1:1 ke SV.
    let src = r#"
module m {
    sig a : logic[7:0]
    initial {
        force a = 8'd99
        release a
    }
}
"#;
    let out = generate(&parse(src).unwrap(), "m");
    assert!(out.sv.contains("force a = 8'd99;"), "force: {}", out.sv);
    assert!(out.sv.contains("release a;"), "release: {}", out.sv);
}

#[test]
fn f49_mgen_package_wraps_file_typedefs() {
    // `mgen --package <nama>`: typedef level file dibungkus dalam package
    // di `.svh`, dan `.sv` dapat `import <nama>::*;` (MIVON-HDL.md §11).
    let src = r#"
type Addr = logic[15:0]
enum State { IDLE, RUN, DONE }
module tb {
    out a : Addr
    out s : State
    comb { a = 0 s = IDLE }
}
"#;
    let file = parse(src).unwrap();
    let opts = GenOpts {
        package: Some("chip_types"),
    };
    let out = generate_src_ext_opts(&file, "tb", &[], "mv", &opts);
    assert!(
        out.svh.contains("package chip_types;"),
        "package harus dibungkus: {}",
        out.svh
    );
    assert!(
        out.svh.contains("    typedef logic [15:0] Addr;"),
        "typedef di-indent di dalam package: {}",
        out.svh
    );
    assert!(
        out.svh.contains("endpackage"),
        "endpackage: {}",
        out.svh
    );
    assert!(
        out.sv.contains("import chip_types::*;"),
        "sv harus import: {}",
        out.sv
    );
    // Default (tanpa --package) tetap $unit — kompatibilitas output lama.
    let plain = generate(&file, "tb");
    assert!(
        !plain.svh.contains("package chip_types"),
        "default tak boleh membuat package: {}",
        plain.svh
    );
    assert!(!plain.sv.contains("import chip_types"), "default no import");
}

#[test]
fn f49_mgen_package_no_op_without_typedefs() {
    // Tanpa typedef level file, `--package` tak menambah apa pun (tak ada
    // import sia-sia di `.sv`).
    let src = r#"
package p { enum E { A, B } }
module m {
    in clk : bit
    out y : bit
    comb { y = 1 }
}
"#;
    let file = parse(src).unwrap();
    let opts = GenOpts {
        package: Some("chip_types"),
    };
    let out = generate_src_ext_opts(&file, "m", &[], "mv", &opts);
    assert!(out.svh.contains("package p;"), "package sumber tetap: {}", out.svh);
    assert!(!out.svh.contains("package chip_types"), "tak ada typedef → tak dibungkus");
    assert!(!out.sv.contains("import chip_types"), "tak ada import");
}

#[test]
fn f49_port_init_from_reg_same_name() {
    // `out a : Addr` + `reg a : Addr = 16'h2A`: reg di-skip (deklarasi ganda
    // tak sah) TAPI nilai inisialisasi dibawa ke deklarasi port — SV sah
    // (LRM 1800 §6.8.2) dan nilai tidak boleh hilang.
    let src = r#"
type Addr = logic[15:0]
module tb {
    out a : Addr
    reg a : Addr = 16'h2A
    in  clk : bit
    seq(clk) { a <= a + 1 }
}
"#;
    let out = generate(&parse(src).unwrap(), "tb");
    assert!(
        out.sv.contains("output Addr a = 16'h2A"),
        "port harus membawa init: {}",
        out.sv
    );
    // Deklarasi reg ganda TIDAK boleh muncul.
    assert_eq!(
        out.sv.matches("Addr a").count(),
        1,
        "tanpa deklarasi ganda: {}",
        out.sv
    );
}

#[test]
fn f49_input_port_has_no_init() {
    // Input port tak boleh diinisialisasi (SV LRM) — reg init diabaikan.
    let src = r#"
module tb {
    in  d : logic[7:0]
    reg d : logic[7:0] = 8'h5
    out q : logic[7:0]
    comb { q = d }
}
"#;
    let out = generate(&parse(src).unwrap(), "tb");
    assert!(
        out.sv.contains("input  logic [7:0] d"),
        "input port polos: {}",
        out.sv
    );
    assert!(
        !out.sv.contains("d = 8'h5"),
        "input port tak boleh punya init: {}",
        out.sv
    );
}
