//! Check — unit tests (dipindah dari check.rs saat pemecahan struktur).
//! Di-include oleh `check/mod.rs` (`#[cfg(test)] mod tests;`).

use super::*;
use crate::parser::parse;

fn check_src(src: &str) -> Result<(), MvError> {
    let file = parse(src)?;
    check(&file)
}

#[test]
fn ok_interface() {
    // F26: interface sehat + port module bertipe interface + seq clock
    // field interface
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
    check_src(src).expect("interface sehat harus lolos check");
}

#[test]
fn e2001_interface_modport_ref_unknown() {
    let src = "interface bad {\n sig a : bit\n modport m { in a, nope } }\n";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2001"), "msg: {}", e.msg);
    assert!(e.msg.contains("nope"));
}

#[test]
fn e2007_duplicate_interface() {
    let src = "interface x {}\ninterface x {}\n";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2007"), "msg: {}", e.msg);
    assert!(e.msg.contains("interface"));
}

#[test]
fn ok_interface_port_type_known() {
    let src = "interface bus {}\nmodule m {\n in b : bus\n out y : bit\n comb { y = 0 } }\n";
    check_src(src).expect("interface sebagai tipe port harus lolos");
}

#[test]
fn ok_counter_like() {
    let src = r#"
module counter #(WIDTH = 8, MOD = 100) {
    in  clk, rst_n : bit
    in  enable     : bit
    out count      : logic[WIDTH-1:0]
    out done       : bit
    seq(clk, rst_n) {
        if (!rst_n) {
            count <= '0
        } else if (enable) {
            count <= (count == MOD-1) ? '0 : count + 1
            done <= 1
        } else {
            done <= 0
        }
    }
}
"#;
    check_src(src).expect("counter harus lolos check");
}

#[test]
fn ok_traffic_like_enum_and_struct() {
    let src = r#"
package pkt {
    enum State { RED, GREEN, YELLOW }
    type Addr = logic[15:0]
    packed struct Packet {
        valid : bit,
        addr  : Addr,
        data  : logic[31:0]
    }
}
module traffic {
    use pkt::*
    in  clk, rst_n : bit
    out state      : State
    out p          : Packet
    reg state      : State
    seq(clk, rst_n) {
        if (!rst_n) {
            state <= RED
        } else {
            state <= GREEN
        }
    }
    comb {
        p.valid = 1
        p.addr  = 8'h00
        p.data  = 32'd0
    }
}
"#;
    check_src(src).expect("enum/struct harus lolos check");
}

#[test]
fn ok_widening_allowed() {
    let src = "module m {\n in a : logic[3:0]\n out y : logic[7:0]\n comb { y = a } }";
    check_src(src).expect("widening harus diizinkan");
}

#[test]
fn ok_testbench_drives_input() {
    let src = "module tb {\n in clk : bit\n in d   : bit\n initial { clk = 0\n d = 1 } }";
    check_src(src).expect("testbench boleh drive input");
}

#[test]
fn ok_system_task_in_initial() {
    // `assert property` = CONCURRENT assertion → module item (LRM 1800 §14),
    // bukan statement di dalam `initial`.
    let src = "module tb {\n in clk : bit\n in count : logic[7:0]\n initial {\n $display(\"mulai\")\n $finish\n }\n assert property (@(posedge clk) count == $past(count) + 1)\n }\n";
    check_src(src).expect("system task bukan sinyal");
}

#[test]
fn ok_assert_property_skipped() {
    let src = "module m {\n in clk : bit\n assert property (@(posedge clk) some_signal_apa_saja == 1)\n }\n";
    check_src(src).expect("assert property body dilewati");
}

#[test]
fn ok_assume_immediate_and_property() {
    // `assume` mirror `assert`: sinyal dikenal lolos, property dilewati.
    let src = "module m {\n sig a : bit\n initial {\n assume (a == 0) $info(\"ok\") else $error(\"bad\")\n }\n assume property (@(posedge clk) a |-> b)\n }\n";
    check_src(src).expect("assume harus lolos");
}

#[test]
fn e2001_in_assume_cond() {
    // Kondisi `assume` ikut validasi E2001 seperti `assert`.
    let src = "module m {\n sig a : bit\n initial {\n assume (nope == 0) $info(\"ok\")\n }\n }\n";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2001"), "msg: {}", e.msg);
}

#[test]
fn ok_cover_immediate_and_property() {
    // `cover` mirror `assert` (tanpa `else`): sinyal dikenal lolos.
    let src = "module m {\n sig a : bit\n initial {\n cover (a == 0) $info(\"ok\")\n }\n cover property (@(posedge clk) a |-> b)\n }\n";
    check_src(src).expect("cover harus lolos");
}

#[test]
fn e2001_in_cover_cond() {
    // Kondisi `cover` ikut validasi E2001 seperti `assert`.
    let src = "module m {\n sig a : bit\n initial {\n cover (nope == 0) $info(\"ok\")\n }\n }\n";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2001"), "msg: {}", e.msg);
}

#[test]
fn ok_uvm_known_types_in_decls() {
    // F72: tipe UVM bawaan sah di field/var tanpa definisi lokal —
    // testbench UVM tak lagi butuh `--no-check` untuk deklarasi.
    let src = "class my_test extends uvm_test {\n field count : uint\n task run_phase() {\n var seqr : uvm_sequencer\n var item : uvm_sequence_item\n seqr.start_item(item)\n }\n}\n";
    check_src(src).expect("tipe UVM harus lolos");
}

#[test]
fn e2005_still_fires_for_unknown_non_uvm_type() {
    // Allowlist presisi: typo / tipe asing tetap E2005.
    let src = "class c {\n task t() {\n var x : uvm_sequncer\n }\n}\n";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2005"), "msg: {}", e.msg);
    assert!(e.msg.contains("uvm_sequncer"), "msg: {}", e.msg);
}

#[test]
fn e2001_still_fires_for_undeclared_signal_in_uvm_call() {
    // E2001 tetap: argumen tak dikenal di call UVM ditolak (bukan
    // dilemahkan) — contoh harus deklarasikan item-nya.
    let src = "class c extends uvm_test {\n task t() {\n var seqr : uvm_sequencer\n seqr.start_item(item)\n }\n}\n";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2001"), "msg: {}", e.msg);
}

#[test]
fn ok_case_inside_values_and_ranges() {
    // Label nilai + rentang + default lolos; sinyal dikenal.
    let src = "module m {\n sig x : logic[7:0]\n sig y : logic[7:0]\n comb {\n case (x) inside {\n 0 : { y = 0 }\n [1:10], x : { y = 1 }\n default : { y = 2 }\n }\n }\n}\n";
    check_src(src).expect("case inside harus lolos");
}

#[test]
fn e2013_in_case_inside_value_dup() {
    // Label NILAI duplikat → E2013 (rentang dilewati konservatif).
    let src = "module m {\n sig x : logic[7:0]\n sig y : bit\n comb {\n case (x) inside {\n 5 : { y = 1 }\n 5 : { y = 0 }\n }\n }\n}\n";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2013"), "msg: {}", e.msg);
    assert!(e.msg.contains("inside"), "msg: {}", e.msg);
}

#[test]
fn ok_unique_if_branches_checked() {
    // Kondisi + cabang `unique if` ikut validasi seperti `if` biasa.
    let src = "module m {\n sig a : bit\n sig y : bit\n comb {\n unique if (nope) { y = 1 } else { y = 0 }\n }\n}\n";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2001"), "msg: {}", e.msg);
    let ok = "module m {\n sig a : bit\n sig y : bit\n comb {\n priority if (a) { y = 1 } else { y = 0 }\n }\n}\n";
    check_src(ok).expect("priority if harus lolos");
}

#[test]
fn ok_package_const_in_enum_width() {
    let src = "package p {\n const N = 4\n enum(N) Color { RED, GREEN } }\nmodule m { in clk : bit\n out c : p::Color\n comb { c = RED } }";
    check_src(src).expect("const package boleh dipakai di lebar enum");
}

#[test]
fn e2001_in_program_block() {
    let src = "program p {\n in clk : bit\n initial { foo = 1 } }\n";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2001"), "msg: {}", e.msg);
    assert!(e.msg.contains("foo"));
}

#[test]
fn ok_program_drives_input_in_initial() {
    let src = "program p {\n in clk : bit\n initial { clk = 0 } }\n";
    check_src(src).expect("program initial boleh drive input");
}

#[test]
fn e2003_drive_input_with_final_still_error() {
    let src = "module m {\n in clk : bit\n out y : bit\n comb { clk = 0 }\n final { y = clk } }";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2003"), "msg: {}", e.msg);
}

#[test]
fn ok_func_locals() {
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
"#;
    check_src(src).expect("func dengan local harus lolos");
}

#[test]
fn e2001_undefined_signal() {
    let src = "module m {\n in clk : bit\n out y : bit\n comb { y = foo } }";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2001"), "msg: {}", e.msg);
    assert!(e.msg.contains("foo"));
}

#[test]
fn e2005_unknown_type() {
    let src = "module m {\n in clk : bit\n out y : Foo }";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2005"), "msg: {}", e.msg);
    assert!(e.msg.contains("Foo"));
}

#[test]
fn e2007_duplicate_signal() {
    let src = "module m {\n sig a : bit\n sig a : bit }";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2007"), "msg: {}", e.msg);
}

#[test]
fn e2004_nba_outside_seq() {
    let src = "module m {\n in clk : bit\n out y : bit\n comb { y <= clk } }";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2004"), "msg: {}", e.msg);
}

#[test]
fn e2004_blocking_in_seq() {
    let src = "module m {\n in clk : bit\n out y : bit\n seq(clk) { y = clk } }";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2004"), "msg: {}", e.msg);
}

#[test]
fn e2003_drive_input() {
    let src = "module m {\n in clk : bit\n out y : bit\n comb { clk = 0 } }";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2003"), "msg: {}", e.msg);
    assert!(e.msg.contains("clk"));
}

#[test]
fn e2002_width() {
    let src = "module m {\n in a : logic[7:0]\n out y : logic[3:0]\n comb { y = a } }";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2002"), "msg: {}", e.msg);
}

#[test]
fn e2006_overflow() {
    let src = "module m {\n in clk : bit\n out y : logic[7:0]\n comb { y = 8'h1FF } }";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2006"), "msg: {}", e.msg);
}

#[test]
fn e2001_struct_field_unknown() {
    let src = r#"
package pkt {
    packed struct Packet { valid : bit, addr : logic[7:0] }
}
module m {
    use pkt::*
    in clk : bit
    out p  : Packet
    comb {
        p.nonexistent = 1
    }
}
"#;
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2001"), "msg: {}", e.msg);
    assert!(e.msg.contains("nonexistent"));
}

#[test]
fn e2007_duplicate_enum_member() {
    let src = "package p { enum E { A, A } }";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2007"), "msg: {}", e.msg);
}

#[test]
fn ok_class_plain() {
    let src = r#"
class counter_model {
    field value : uint
    constraint c { value > 0 }
    func new() {
        value = 0
    }
    task tick() {
        value = value + 1
    }
}
"#;
    check_src(src).expect("class mandiri harus lolos check");
}

#[test]
fn ok_class_this_super_method_call() {
    let src = r#"
class c extends base {
    field x : uint
    func new() {
        super.new()
        this.x = 0
    }
    func f() {
        self_helper(x)
    }
}
"#;
    check_src(src).expect("this/super/method-call harus lolos");
}

#[test]
fn e2001_in_class_method() {
    let src = "class c {\n field x : uint\n func f() {\n y = 1\n }\n}";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2001"), "msg: {}", e.msg);
    assert!(e.msg.contains("y"));
}

#[test]
fn e2007_duplicate_class_field() {
    let src = "class c {\n field x : uint\n field x : uint\n}";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2007"), "msg: {}", e.msg);
}

#[test]
fn e2007_duplicate_class() {
    let src = "class a {}\nclass a {}\n";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2007"), "msg: {}", e.msg);
}

// ── F9: multi-file (konteks gabungan lintas file) ──

#[test]
fn f9_check_many_cross_file_package() {
    let types = parse(
        "package types_pkg {\n type Addr = logic[15:0]\n enum State { IDLE, RUN }\n}\nmodule types_dummy {\n in clk : bit\n}\n",
    )
    .unwrap();
    let counter = parse(
        "module counter {\n use types_pkg::*\n in clk, rst_n : bit\n out addr : Addr\n out st : State\n seq(clk, rst_n) {\n if (!rst_n) {\n addr <= '0\n st <= IDLE\n } else {\n addr <= addr + 1\n st <= RUN\n }\n }\n}\n",
    )
    .unwrap();

    let e = check(&counter).unwrap_err();
    assert!(e.msg.contains("E2005"), "msg: {}", e.msg);

    check_many(&[&types, &counter]).expect("multi-file harus lolos check");
}

#[test]
fn f9_check_many_error_index_and_message() {
    let good = parse("module a {\n in clk : bit\n}\n").unwrap();
    let bad = parse("module b {\n in clk : bit\n out y : bit\n comb { y = nope }\n}\n").unwrap();
    let (idx, e) = check_many(&[&good, &bad]).unwrap_err();
    assert_eq!(idx, 1);
    assert!(e.msg.contains("E2001"), "msg: {}", e.msg);
    assert!(e.msg.contains("nope"));
}

#[test]
fn f9_check_many_const_visible_across_files() {
    let a = parse("package cfg {\n const N = 4\n}\n").unwrap();
    let b = parse(
        "package p {\n enum(N) Color { RED, GREEN }\n}\nmodule m {\n in clk : bit\n out c : p::Color\n comb { c = RED }\n}\n",
    )
    .unwrap();
    check_many(&[&a, &b]).expect("konstanta antar-file harus terlihat");
}

#[test]
fn f9_check_many_duplicate_package_cross_file() {
    let a = parse("package pkt {\n type Addr = logic[7:0]\n}\n").unwrap();
    let b = parse("package pkt {\n type Data = logic[7:0]\n}\n").unwrap();
    let (idx, e) = check_many(&[&a, &b]).unwrap_err();
    assert_eq!(idx, 1);
    assert!(e.msg.contains("E2007"), "msg: {}", e.msg);
    assert!(e.msg.contains("pkt"));
}

#[test]
fn f9_check_many_duplicate_class_cross_file() {
    let a = parse("class foo {\n field x : uint\n}\n").unwrap();
    let b = parse("class foo {\n field y : uint\n}\n").unwrap();
    let e = check_many(&[&a, &b]).unwrap_err();
    assert!(e.1.msg.contains("E2007"), "msg: {}", e.1.msg);
    assert!(e.1.msg.contains("foo"));
}

// ── F11: error type-check BERPOSISI (line:col) ──

#[test]
fn f11_e2001_undefined_signal_position() {
    let src = "module m {\n    in clk : bit\n    out y : bit\n    comb { y = foo }\n}\n";
    let e = check_src(src).unwrap_err();
    assert_eq!(e.line, 4, "line: {e:?}");
    assert_eq!(e.col, 16, "col: {e:?}");
    assert!(e.msg.contains("E2001"));
}

#[test]
fn f11_e2002_width_position() {
    let src = "module m {\n    in a : logic[7:0]\n    out y : logic[3:0]\n    comb { y = a }\n}\n";
    let e = check_src(src).unwrap_err();
    assert_eq!(e.line, 4, "line: {e:?}");
    assert_eq!(e.col, 12, "col: {e:?}");
    assert!(e.msg.contains("E2002"));
}

#[test]
fn f11_e2003_drive_input_position() {
    let src = "module m {\n    in clk : bit\n    out y : bit\n    comb { clk = 0 }\n}\n";
    let e = check_src(src).unwrap_err();
    assert_eq!(e.line, 4);
    assert_eq!(e.col, 12);
    assert!(e.msg.contains("E2003"));
}

#[test]
fn f11_e2004_nba_outside_seq_position() {
    let src = "module m {\n    in clk : bit\n    out y : bit\n    comb { y <= clk }\n}\n";
    let e = check_src(src).unwrap_err();
    assert_eq!(e.line, 4);
    assert_eq!(e.col, 12);
    assert!(e.msg.contains("E2004"));
}

#[test]
fn f11_e2005_unknown_type_position() {
    let src = "module m {\n    in clk : bit\n    out y : Foo\n}\n";
    let e = check_src(src).unwrap_err();
    assert_eq!(e.line, 3, "line: {e:?}");
    assert_eq!(e.col, 13, "col: {e:?}");
    assert!(e.msg.contains("E2005"));
}

#[test]
fn f11_e2006_overflow_position() {
    let src = "module m {\n    in clk : bit\n    out y : logic[7:0]\n    comb { y = 8'h1FF }\n}\n";
    let e = check_src(src).unwrap_err();
    assert_eq!(e.line, 4, "line: {e:?}");
    assert_eq!(e.col, 16, "col: {e:?}");
    assert!(e.msg.contains("E2006"));
}

#[test]
fn f11_e2007_duplicate_signal_position() {
    let src = "module m {\n    sig a : bit\n    sig a : bit\n}\n";
    let e = check_src(src).unwrap_err();
    assert_eq!(e.line, 3, "line: {e:?}");
    assert_eq!(e.col, 5, "col: {e:?}");
    assert!(e.msg.contains("E2007"));
}

#[test]
fn f11_e2001_clock_undefined_position() {
    let src = "module m {\n    in clk : bit\n    out y : bit\n    seq(nope) { y <= 0 }\n}\n";
    let e = check_src(src).unwrap_err();
    assert_eq!(e.line, 4, "line: {e:?}");
    assert!(e.msg.contains("E2001"));
    assert!(e.msg.contains("nope"));
}

#[test]
fn f11_e2001_struct_field_position() {
    let src = "package pkt {\n    packed struct P { valid : bit }\n}\nmodule m {\n    use pkt::*\n    in clk : bit\n    out p : P\n    comb { p.nonexistent = 1 }\n}\n";
    let e = check_src(src).unwrap_err();
    assert_eq!(e.line, 8, "line: {e:?}");
    assert!(e.msg.contains("E2001"));
    assert!(e.msg.contains("nonexistent"));
}

#[test]
fn f11_cross_file_duplicate_has_position() {
    let a = parse("package pkt {\n type A = logic[7:0]\n}\n").unwrap();
    let b = parse("package pkt {\n type B = logic[7:0]\n}\n").unwrap();
    let (idx, e) = check_many(&[&a, &b]).unwrap_err();
    assert_eq!(idx, 1);
    assert_eq!(e.line, 1, "line: {e:?}");
    assert_eq!(e.col, 9, "col: {e:?}");
    assert!(e.msg.contains("E2007"));
}

// ── F12: constraint lanjutan (inside/dist/if-else/solve) ──

#[test]
fn f12_ok_constraint_advanced() {
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
    check_src(src).expect("constraint lanjutan harus lolos check");
}

#[test]
fn f12_constraint_solve_unknown_var() {
    let src = "class c {\n    rand field x : uint\n    constraint c1 {\n        solve nope before x\n    }\n}\n";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2001"), "msg: {}", e.msg);
    assert!(e.msg.contains("nope"));
    assert_eq!(e.line, 4, "line: {e:?}");
}

#[test]
fn f12_constraint_inside_unknown_signal() {
    let src = "class c {\n    rand field x : uint\n    constraint c1 {\n        w inside {[1:10]}\n    }\n}\n";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2001"), "msg: {}", e.msg);
    assert!(e.msg.contains("w"));
}

#[test]
fn f12_constraint_dist_unknown_weight_signal() {
    // bobot dist memakai sinyal tak dikenal → E2001
    let src = "class c {\n    rand field x : uint\n    constraint c1 {\n        x dist { 1 := w }\n    }\n}\n";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2001"), "msg: {}", e.msg);
    assert!(e.msg.contains("w"));
}

#[test]
fn f11_e2007_duplicate_module_in_file() {
    let src = "module a {\n    in clk : bit\n}\nmodule a {\n    in clk : bit\n}\n";
    let e = check_src(src).unwrap_err();
    assert_eq!(e.line, 4, "line: {e:?}");
    assert_eq!(e.col, 8, "col: {e:?}");
    assert!(e.msg.contains("E2007"));
    assert!(e.msg.contains("module 'a'"));
}

#[test]
fn f11_e2007_duplicate_module_cross_file() {
    let a = parse("module top {\n    in clk : bit\n}\n").unwrap();
    let b = parse("module top {\n    in clk : bit\n}\n").unwrap();
    let (idx, e) = check_many(&[&a, &b]).unwrap_err();
    assert_eq!(idx, 1);
    assert_eq!(e.line, 1, "line: {e:?}");
    assert_eq!(e.col, 8, "col: {e:?}");
    assert!(e.msg.contains("E2007"));
}

#[test]
fn f11_e2007_module_vs_program_collision() {
    let src = "module tb {\n    in clk : bit\n}\nprogram tb {\n    in clk : bit\n}\n";
    let e = check_src(src).unwrap_err();
    assert_eq!(e.line, 4, "line: {e:?}");
    assert!(e.msg.contains("E2007"));
}

// ── F29: validasi instansiasi + koneksi port ──

#[test]
fn f29_inst_named_port_not_exist() {
    let src = "module foo {\n    in a : bit\n    out b : bit\n}\nmodule tb {\n    sig x : bit\n    sig y : bit\n    inst foo u (.a(x), .bogus(y))\n}\n";
    let e = check_src(src).unwrap_err();
    assert_eq!(e.line, 8, "line: {e:?}");
    assert!(e.msg.contains("E2001"), "msg: {}", e.msg);
    assert!(e.msg.contains("bogus"), "msg: {}", e.msg);
    assert!(e.msg.contains("module 'foo'"), "msg: {}", e.msg);
}

#[test]
fn f29_inst_positional_too_many() {
    let src = "module foo {\n    in a : bit\n}\nmodule tb {\n    sig x : bit\n    sig y : bit\n    inst foo u (x, y)\n}\n";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2001"), "msg: {}", e.msg);
    assert!(
        e.msg.contains("terlalu banyak koneksi positional"),
        "msg: {}",
        e.msg
    );
}

#[test]
fn f29_inst_external_module_skipped() {
    let src = "module tb {\n    sig x : bit\n    inst foo3 u (.a(x))\n}\n";
    check_src(src).expect("module eksternal harus dilewati, bukan error");
}

#[test]
fn f29_inst_interface_port_check() {
    let src = "interface axi_lite {\n    in clk : bit\n}\nmodule tb {\n    sig x : bit\n    inst axi_lite bus (.bogus(x))\n}\n";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2001"), "msg: {}", e.msg);
    assert!(e.msg.contains("interface 'axi_lite'"), "msg: {}", e.msg);
}

#[test]
fn f29_inst_ok_named_and_positional() {
    let src = "module foo {\n    in a : bit\n    out b : bit\n}\nmodule tb {\n    sig x : bit\n    sig y : bit\n    inst foo u (x, .b(y))\n}\n";
    check_src(src).expect("koneksi valid harus lolos");
}

// ── F29 fix review ──

#[test]
fn f29_review_duplicate_port_connection() {
    let src = "module foo {\n    in a : bit\n}\nmodule tb {\n    sig x : bit\n    sig y : bit\n    inst foo u (.a(x), .a(y))\n}\n";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2007"), "msg: {}", e.msg);
    assert!(e.msg.contains("dikoneksikan dua kali"), "msg: {}", e.msg);
}

#[test]
fn f29_review_module_interface_name_collision() {
    let src = "interface foo {\n    in clk : bit\n}\nmodule foo {\n    in a : bit\n}\n";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2007"), "msg: {}", e.msg);
    assert!(e.msg.contains("bentrok"), "msg: {}", e.msg);
}

#[test]
fn f30_inst_cross_file_port_check() {
    let a = parse("module foo {\n    in a : bit\n    out b : bit\n}\n").unwrap();
    let b = parse(
        "module tb {\n    sig x : bit\n    sig y : bit\n    inst foo u (.a(x), .nope(y))\n}\n",
    )
    .unwrap();
    let (idx, e) = check_many(&[&a, &b]).unwrap_err();
    assert_eq!(idx, 1);
    assert!(e.msg.contains("E2001"), "msg: {}", e.msg);
    assert!(e.msg.contains("nope"), "msg: {}", e.msg);
    assert!(e.msg.contains("module 'foo'"), "msg: {}", e.msg);
}

#[test]
fn f30_inst_cross_file_ok() {
    let a = parse("module foo {\n    in a : bit\n    out b : bit\n}\n").unwrap();
    let b = parse("module tb {\n    sig x : bit\n    sig y : bit\n    inst foo u (x, .b(y))\n}\n")
        .unwrap();
    check_many(&[&a, &b]).expect("koneksi lintas-file valid harus lolos");
}

// ── F31: parameter module + validasi override ──

#[test]
fn f31_param_override_unknown() {
    let src = "module counter_w #(W = 4) {\n    in  clk : bit\n    out count : logic[W-1:0]\n}\nmodule tb {\n    sig x : bit\n    inst counter_w u #(.NOPE(4)) (.clk(x), .count(x))\n}\n";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2001"), "msg: {}", e.msg);
    assert!(e.msg.contains("NOPE"), "msg: {}", e.msg);
    assert!(e.msg.contains("module 'counter_w'"), "msg: {}", e.msg);
}

#[test]
fn f31_param_override_duplicate() {
    let src = "module counter_w #(W = 4) {\n    in  clk : bit\n    out count : logic[W-1:0]\n}\nmodule tb {\n    sig x : bit\n    inst counter_w u #(.W(4), .W(8)) (.clk(x), .count(x))\n}\n";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2007"), "msg: {}", e.msg);
    assert!(e.msg.contains("di-override dua kali"), "msg: {}", e.msg);
}

#[test]
fn f31_param_override_ok() {
    let src = "module counter_w #(W = 4) {\n    in  clk : bit\n    out count : logic[W-1:0]\n}\nmodule tb {\n    sig x : bit\n    sig y : logic[7:0]\n    inst counter_w u #(.W(8)) (.clk(x), .count(y))\n}\n";
    check_src(src).expect("override parameter valid harus lolos");
}

#[test]
fn f31_param_override_external_module_skipped() {
    let src = "module tb {\n    sig x : bit\n    sig y : bit\n    inst foo3 u #(.NOPE(4)) (.a(x), .b(y))\n}\n";
    check_src(src).expect("module eksternal harus dilewati, bukan error");
}

// ── F32: type parameter (`T : type = logic[7:0]`) ──

#[test]
fn f32_type_param_decl_ok() {
    let src =
        "module m #(T : type = logic[7:0]) {\n    in  d : T\n    out q : T\n    sig x : T\n}\n";
    check_src(src).expect("type param + sinyal bertipe T harus lolos");
}

#[test]
fn f32_type_param_type_kw_syntax() {
    let src = "module m #(type T = logic[15:0]) {\n    sig x : T\n}\n";
    check_src(src).expect("sintaks `type T = ...` harus lolos");
}

#[test]
fn f32_type_param_bad_default_type() {
    let src = "module m #(T : type = UnknownTy) {\n    sig x : T\n}\n";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2005"), "msg: {}", e.msg);
    assert!(e.msg.contains("UnknownTy"), "msg: {}", e.msg);
}

#[test]
fn f32_type_param_override_type_ok() {
    let src = "type Word16 = logic[15:0]\nmodule m #(T : type = logic[7:0]) {\n    in  d : T\n    out q : T\n}\nmodule tb {\n    sig d16 : logic[15:0]\n    sig q16 : logic[15:0]\n    inst m u #(.T(Word16)) (.d(d16), .q(q16))\n}\n";
    check_src(src).expect("override type param dgn tipe dikenal harus lolos");
}

#[test]
fn f32_type_param_override_non_type_err() {
    let src = "module m #(T : type = logic[7:0]) {\n    in  d : T\n    out q : T\n}\nmodule tb {\n    sig d8 : logic[7:0]\n    sig q8 : logic[7:0]\n    inst m u #(.T(8'd4)) (.d(d8), .q(q8))\n}\n";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2005"), "msg: {}", e.msg);
    assert!(e.msg.contains("harus nama tipe"), "msg: {}", e.msg);
}

#[test]
fn f32_type_param_override_unknown_type_err() {
    let src = "module m #(T : type = logic[7:0]) {\n    in  d : T\n    out q : T\n}\nmodule tb {\n    sig d8 : logic[7:0]\n    sig q8 : logic[7:0]\n    inst m u #(.T(NopeTy)) (.d(d8), .q(q8))\n}\n";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2005"), "msg: {}", e.msg);
    assert!(e.msg.contains("NopeTy"), "msg: {}", e.msg);
}

#[test]
fn f32_inst_external_type_param_like_skipped() {
    let src = "type Word16 = logic[15:0]\nmodule tb {\n    sig d16 : logic[15:0]\n    sig q16 : logic[15:0]\n    inst ext_mod u #(.T(Word16)) (.d(d16), .q(q16))\n}\n";
    check_src(src).expect("override type param eksternal harus dilewati");
}

// ── F32 fix review ──

#[test]
fn f32_review_const_of_type_param() {
    let src = "module m #(T : type = logic[7:0]) {\n    const C = 4\n    sig x : T\n    comb { x = C[0] }\n}\n";
    check_src(src).expect("const dgn tipe type param harus lolos");
}

#[test]
fn f32_review_override_scoped_type() {
    let src = "package p {\n    type Word16 = logic[15:0]\n}\nmodule m #(T : type = logic[7:0]) {\n    in  d : T\n    out q : T\n}\nmodule tb {\n    sig d16 : logic[15:0]\n    sig q16 : logic[15:0]\n    inst m u #(.T(p::Word16)) (.d(d16), .q(q16))\n}\n";
    check_src(src).expect("override scoped type param harus lolos");
}

#[test]
fn f32_review_type_kw_without_default() {
    let src = "module m #(type T) {\n    sig x : T\n}\n";
    check_src(src).expect("type param tanpa default tetap dikenal sbg tipe");
}

// ── F33: type cast `T'(expr)` ──

#[test]
fn f33_cast_ok() {
    let src = "type Word16 = logic[15:0]\nmodule m #(T : type = logic[7:0]) {\n    in  a : logic[15:0]\n    out w : Word16\n    out t : T\n    out b : bit\n    comb {\n        w = Word16'(a)\n        t = T'(a)\n        b = logic'(a[0])\n    }\n}\n";
    check_src(src).expect("cast ke tipe dikenal harus lolos");
}

#[test]
fn f33_cast_unknown_type_err() {
    let src = "module m {\n    in  a : logic[7:0]\n    out q : logic[7:0]\n    comb {\n        q = NopeTy'(a)\n    }\n}\n";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2005"), "msg: {}", e.msg);
    assert!(e.msg.contains("NopeTy"), "msg: {}", e.msg);
}

#[test]
fn f33_cast_width_into_narrow_signal_err() {
    let src = "type Word16 = logic[15:0]\nmodule m {\n    in  a : logic[15:0]\n    out q : logic[7:0]\n    comb {\n        q = Word16'(a)\n    }\n}\n";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2002"), "msg: {}", e.msg);
}

// ── F33 fix review ──

#[test]
fn f33_review_size_cast_via_param_ok() {
    let src = "module m #(WIDTH = 8) {\n    in  a : logic[7:0]\n    out q : logic[7:0]\n    comb {\n        q = WIDTH'(a)\n    }\n}\n";
    check_src(src).expect("size cast via parameter harus lolos");
}

#[test]
fn f33_review_cast_ranged_target_err() {
    let src = "module m {\n    in  a : logic[7:0]\n    out q : logic[7:0]\n    comb {\n        q = logic[7:0]'(a)\n    }\n}\n";
    let e = check_src(src).unwrap_err();
    assert!(
        e.msg.contains("tidak boleh punya range") || e.msg.contains("ekspresi tidak valid"),
        "msg: {}",
        e.msg
    );
}

// ── F45: wait fork / disable fork ──

#[test]
fn f45_wait_disable_fork_ok() {
    let src = "module m {\n sig a : logic[7:0]\n initial {\n fork {\n #10\n a = 1\n } join_none\n wait fork\n disable fork\n }\n}\n";
    check_src(src).expect("wait/disable fork harus lolos");
}

// ── F46: force / release ──

#[test]
fn f46_force_release_ok() {
    let src = "module m {\n sig a : logic[7:0]\n initial {\n force a = 8'd99\n release a\n }\n}\n";
    check_src(src).expect("force/release harus lolos");
}

#[test]
fn f46_force_in_seq_rejected() {
    // force = blocking → di dalam seq ditolak E2004 seperti `=`.
    let src = "module m {\n in clk : bit\n sig q : logic[7:0]\n seq(clk) {\n force q = 1\n }\n}\n";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2004"), "msg: {}", e.msg);
}

// ── E2002 lewat konstanta module + lebar enum sinkron dgn emisi ──────────

#[test]
fn check_enum_width_matches_sv_emission() {
    // Lebar enum yang dilihat checker harus SAMA dengan yang di-emit:
    // 3 anggota → `logic [1:0]` = 2 bit. Sebelumnya checker menghitung
    // `enum_bits(n)` (MSB) sebagai lebar TOTAL, sehingga E2002 tak pernah
    // menyalakan untuk sinyal enum.
    let src = "package p { enum St { A, B, C } }\nmodule m { use p::*\n sig s : St\n sig w : logic[0:0]\n comb { s = A  w = s } }";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2002"), "msg: {}", e.msg);
    // 2 anggota → clog2(2)=1 → `logic [1:0]` = 2 bit juga
    let src2 = "package p2 { enum St2 { A, B } }\nmodule m2 { use p2::*\n sig s : St2\n sig w : logic[0:0]\n comb { s = A  w = s } }";
    assert!(check_src(src2).is_err(), "2 anggota juga 2 bit");
    // lebar eksplisit 4 → muat di 4 bit tanpa error
    let src3 = "package p3 { enum(4) St3 { A, B } }\nmodule m3 { use p3::*\n sig s : St3\n sig w : logic[3:0]\n comb { s = A  w = s } }";
    check_src(src3).expect("enum(4) → 4 bit, muat di 4 bit");
    // …dan tidak muat di 1 bit
    let src4 = "package p4 { enum(4) St4 { A, B } }\nmodule m4 { use p4::*\n sig s : St4\n sig w : logic[0:0]\n comb { s = A  w = s } }";
    assert!(check_src(src4).unwrap_err().msg.contains("E2002"));
}

#[test]
fn check_enum_width_from_package_const() {
    // `enum(W)` dengan W = konstanta package harus dipakai sebagai lebar.
    let src = "package p { const W = 4\n enum(W) St { A, B } }\nmodule m { use p::*\n sig s : St\n sig w : logic[0:0]\n comb { s = A  w = s } }";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2002"), "msg: {}", e.msg);
}

#[test]
fn check_module_const_folded_for_width() {
    // `const C` module harus bisa dipakai sebagai lebar — sebelumnya hanya
    // nama yang terdaftar tanpa nilai sehingga cek lebar mati diam-diam.
    let bad = "module m { in c : bit\n const W = 4\n sig a : logic[W-1:0]\n comb { a = 8'hFF } }";
    let e = check_src(bad).unwrap_err();
    assert!(e.msg.contains("E2002"), "lebar lewat const: {}", e.msg);
    check_src("module m2 { in c : bit\n const W = 4\n sig a : logic[W-1:0]\n comb { a = 4'hF } }")
        .expect("nilai yang muat harus sah");
}

// ── E2010: lvalue tak boleh konstanta (LRM 1800 §6.20) ──────────────────

#[test]
fn check_e2010_cannot_assign_parameter_or_const() {
    let src = "module m #(W = 8) { in c : bit\n sig s : logic[7:0]\n comb { W = 4 } }";
    let e = check_src(src).unwrap_err();
    assert!(e.msg.contains("E2010"), "msg: {}", e.msg);
    let src2 = "module m2 { in c : bit\n const K = 3\n sig s : logic[7:0]\n comb { K = 4 } }";
    let e2 = check_src(src2).unwrap_err();
    assert!(e2.msg.contains("E2010"), "msg: {}", e2.msg);
    // increment pada konstanta juga ditolak
    let e3 = check_src("module m3 { in c : bit\n const K = 3\n sig s : logic[7:0]\n comb { K++ } }")
        .unwrap_err();
    assert!(e3.msg.contains("E2010"), "msg: {}", e3.msg);
    // membaca konstanta tetap sah
    check_src("module m4 #(W = 8) { in c : bit\n sig s : logic[7:0]\n comb { s = W } }")
        .expect("baca konstanta harus sah");
}

// ── E2011: jumlah argumen pemanggilan ──────────────────────────────────

#[test]
fn check_e2011_call_arity() {
    let ok = "func f(a : int, b : int = 2) -> int { return a + b }\nmodule m { in c : bit\n sig r : int\n comb { r = f(1) } }";
    check_src(ok).expect("1 argumen + default harus sah");
    let too_many = "func f(a : int, b : int = 2) -> int { return a + b }\nmodule m { in c : bit\n sig r : int\n comb { r = f(1, 2, 3) } }";
    assert!(check_src(too_many).unwrap_err().msg.contains("E2011"));
    let too_few = "func g(a : int, b : int) -> int { return a + b }\nmodule m { in c : bit\n sig r : int\n comb { r = g(1) } }";
    assert!(check_src(too_few).unwrap_err().msg.contains("E2011"));
    // method class juga
    let bad_m = "class C { func mk(a : int, b : int) -> int { return a + b } }\nmodule mm { in c : bit\n sig r : int\n initial { var o : C\n r = o.mk(1) } }";
    assert!(check_src(bad_m).unwrap_err().msg.contains("E2011"));
}

// ── E2012: perbandingan unsigned yang hasilnya selalu salah ────────────

#[test]
fn check_e2012_unsigned_comparison_always_false() {
    // LRM 1800 §11.8.2: satu operand unsigned → seluruh perbandingan
    // unsigned. `u < 0` untuk `logic[7:0]` selalu false (buglog-mv #5).
    for bad in [
        "u < 0", "u <= -1", "u < 256", "u > 255", "0 > u", "-1 > u", "u >= 256",
    ] {
        let src = format!(
            "module m {{ in c : bit\n sig u : logic[7:0]\n sig y : bit\n comb {{ y = {bad} }} }}"
        );
        let e = check_src(&src).unwrap_err();
        assert!(e.msg.contains("E2012"), "{bad} → E2012, dapat: {}", e.msg);
    }
    // Tidak boleh ada false positive.
    for ok in [
        "u == 0xFF",
        "u >= 255",
        "u <= 255",
        "u < 255",
        "u > 0",
        "u >= 1",
        "u <= 254",
    ] {
        let src = format!(
            "module m {{ in c : bit\n sig u : logic[7:0]\n sig y : bit\n comb {{ y = {ok} }} }}"
        );
        check_src(&src).unwrap_or_else(|e| panic!("{ok} harus sah, dapat: {}", e.msg));
    }
    // signed aman
    check_src("module ms { in c : bit\n sig s : signed logic[7:0]\n sig y : bit\n comb { y = s < 0 } }")
        .expect("signed < 0 harus sah");
    // `int` (signed bawaan) aman
    check_src("module mi { in c : bit\n sig s : int\n sig y : bit\n comb { y = s < 0 } }")
        .expect("int < 0 harus sah");
}

// ── E2013: label case duplikat ─────────────────────────────────────────

#[test]
fn check_e2013_duplicate_case_label() {
    let bad = "module m { in c : bit\n sig s : logic[1:0]\n sig o : logic[7:0]\n comb { case (s) { 2'd0: { o = 1 }  0: { o = 2 }  default: { o = 0 } } } }";
    let e = check_src(bad).unwrap_err();
    assert!(e.msg.contains("E2013"), "msg: {}", e.msg);
    // `8'h0` vs `0` juga dianggap sama
    let bad2 = "module m2 { in c : bit\n sig s : logic[7:0]\n sig o : logic[7:0]\n comb { case (s) { 8'h0: { o = 1 }  8'd0: { o = 2 }  default: { o = 0 } } } }";
    assert!(check_src(bad2).unwrap_err().msg.contains("E2013"));
    // label unik → sah
    check_src("module m3 { in c : bit\n sig s : logic[1:0]\n sig o : logic[7:0]\n comb { case (s) { 2'd0: { o = 1 }  2'd1: { o = 2 }  default: { o = 0 } } } }")
        .expect("label unik harus sah");
}

// ── Posisi pada E2007/E2008/E2009 ──────────────────────────────────────

#[test]
fn check_return_break_continue_have_position() {
    // Sebelumnya MvError::new(0, 0, "E2008: ...") — tanpa posisi dan tanpa
    // kurung `[E20xx]`, jadi tidak konsisten dengan diagnostic lain.
    let e = check_src("module m { in c : bit\n task t() { return 5 } }").unwrap_err();
    assert!(e.msg.contains("[E2008]"), "kurung kode: {}", e.msg);
    assert!(e.line > 0 && e.col > 0, "harus berposisi: {}:{}", e.line, e.col);
    let e2 = check_src("module m2 { in c : bit\n initial { break } }").unwrap_err();
    assert!(e2.msg.contains("[E2009]"), "kurung kode: {}", e2.msg);
    assert!(e2.line > 0, "harus berposisi: {}", e2.line);
    let e3 = check_src("module m3 { in c : bit\n func f(x : int) -> int { var y : int = 1\n var y : int = 2\n return y } }")
        .unwrap_err();
    assert!(e3.msg.contains("[E2007]"), "kurung kode: {}", e3.msg);
    assert!(e3.line > 0, "harus berposisi: {}", e3.line);
}

// ── E2007 lintas-file untuk function/task level file ───────────────────

#[test]
fn check_duplicate_file_level_func_across_files_is_e2007() {
    // Dua function level file dengan nama sama → dua deklarasi di scope FILE
    // SV yang sama; verilator: "Duplicate declaration of function".
    let a = parse("func dup(v : int) -> int { return v }\nmodule ma { in c : bit }").unwrap();
    let b = parse("func dup(v : int) -> int { return v + 1 }\nmodule mb { in c : bit }").unwrap();
    let e = super::check_many(&[&a, &b]).unwrap_err();
    assert!(e.1.msg.contains("E2007"), "msg: {}", e.1.msg);
    assert!(e.1.msg.contains("dup"), "msg: {}", e.1.msg);
    assert!(e.1.line > 0, "harus berposisi");
    // task juga
    let c = parse("task tw() { }\nmodule mc { in c : bit }").unwrap();
    let d = parse("task tw() { }\nmodule md { in c : bit }").unwrap();
    let e2 = super::check_many(&[&c, &d]).unwrap_err();
    assert!(e2.1.msg.contains("E2007"), "msg: {}", e2.1.msg);
}

#[test]
fn f75_wire_assign_ok() {
    // F75 sehat: wire bit/logic + assign driver valid lolos.
    let src = "module m { in a, b : bit\n out y : bit\n wire w : bit\n assign w = a & b\n assign y = w }";
    check_src(src).expect("wire/assign sehat harus lolos");
    // wire vektor + typedef + init inline
    let src2 = "module m2 { in a : logic[7:0]\n out y : logic[7:0]\n wire w : logic[7:0] = a\n assign y = w }";
    check_src(src2).expect("wire init harus lolos");
}

#[test]
fn f75_wire_bad_type_is_e2005() {
    // wire hanya bit/logic/typedef — int/real/string ditolak E2005.
    let e = check_src("module m { wire x : int\n assign x = 1 }").unwrap_err();
    assert!(e.msg.contains("E2005"), "msg: {}", e.msg);
    let e2 = check_src("module m { wire s : string\n assign s = \"a\" }").unwrap_err();
    assert!(e2.msg.contains("E2005"), "msg: {}", e2.msg);
}

#[test]
fn f75_assign_drive_input_is_e2003() {
    // assign ke input port di DUT → E2003; di TB (ada initial) boleh.
    let e = check_src("module m { in a : bit\n out y : bit\n assign a = y }").unwrap_err();
    assert!(e.msg.contains("E2003"), "msg: {}", e.msg);
    let tb = "module tb { in a : bit\n out y : bit\n assign a = y\n initial { a = 0 } }";
    check_src(tb).expect("TB boleh drive input via assign");
}

#[test]
fn f75_assign_width_mismatch_is_e2002() {
    // RHS 16-bit ke LHS 8-bit → E2002 truncation.
    let e = check_src(
        "module m { in a : logic[15:0]\n out y : logic[7:0]\n wire w : logic[7:0]\n assign w = a\n assign y = w }",
    )
    .unwrap_err();
    assert!(e.msg.contains("E2002"), "msg: {}", e.msg);
}

#[test]
fn f76_all_net_kinds_ok() {
    // F76: semua varian net lolos check (tipe bit/logic).
    let src = "module m { in a : bit\n wand wa : bit\n wor wo : logic[7:0]\n tri tr : bit\n tri0 t0 : bit\n tri1 t1 : bit\n supply0 s0 : bit\n supply1 s1 : bit\n assign wa = a\n assign tr = a }";
    check_src(src).expect("semua net kinds harus lolos");
}

#[test]
fn f78_bind_ok_and_port_param_checked() {
    // F78 sehat: bind ke module dikenal — koneksi/param divalidasi seperti inst.
    let src = "module chk {\n in clk : bit\n in flag : bit\n}\nmodule tb {\n sig clk : bit\n sig flag : bit\n inst chk u (.clk, .flag)\n bind u chk u2 (.clk(clk), .flag)\n}";
    check_src(src).expect("bind sehat harus lolos");
    // port tak ada → E2001 (bila module target dikenali).
    let e = check_src("module chk {\n in clk : bit\n}\nmodule tb {\n sig clk : bit\n bind u chk u2 (.nope(clk))\n}").unwrap_err();
    assert!(e.msg.contains("E2001"), "msg: {}", e.msg);
    // param tak ada → E2001.
    let e2 = check_src("module chk #(W = 1) {\n in clk : bit\n}\nmodule tb {\n sig clk : bit\n bind u chk u2 #(.NOPE(1)) (.clk)\n}").unwrap_err();
    assert!(e2.msg.contains("E2001"), "msg: {}", e2.msg);
    // port ganda → E2007.
    let e3 = check_src("module chk {\n in clk : bit\n}\nmodule tb {\n sig clk : bit\n bind u chk u2 (.clk, .clk(clk))\n}").unwrap_err();
    assert!(e3.msg.contains("E2007"), "msg: {}", e3.msg);
}
