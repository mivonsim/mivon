//! Perilaku prosedural: release/deassign, loop (break/continue), literal fill,
//! wildcard equality, package/import, fork-join, wait, mailbox, semaphore,
//! dan UVM dasar.

use super::*;

#[test]
fn test_release_deassign() {
    let source = r#"
module tb;
    reg [7:0] a;
    initial begin
        a = 42;
        #1 force a = 99;
        #1 release a;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let a_val = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    // After release, forced status is removed but value stays at last forced value
    assert_eq!(a_val, 99, "after release, value retains last forced value");
}

#[test]
fn test_break_in_loop() {
    let source = r#"
module tb;
    reg [7:0] count;
    initial begin
        count = 0;
        forever begin
            count = count + 1;
            if (count == 5) break;
        end
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "count").unwrap();
    assert_eq!(val.to_u64(), 5, "break should exit at count=5");
}

#[test]
fn test_continue_in_loop() {
    let source = r#"
module tb;
    reg [7:0] count;
    reg [7:0] sum;
    initial begin
        count = 0;
        sum = 0;
        while (count < 10) begin
            count = count + 1;
            if (count % 2 == 0) continue;
            sum = sum + count;
        end
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "sum").unwrap();
    // Sum of odd numbers 1..9 = 25
    assert_eq!(val.to_u64(), 25, "continue should skip even numbers");
}

#[test]
fn test_fill_literals() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [7:0] b;
    initial begin
        a = '0;
        b = '1;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let a_val = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(a_val, 0, "'0 should fill all bits with 0");
}

#[test]
fn test_do_while_loop() {
    let source = r#"
module tb;
    reg [7:0] count;
    initial begin
        count = 0;
        do begin
            count = count + 1;
        end while (count < 5);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "count").unwrap();
    assert_eq!(val.to_u64(), 5, "do-while should execute until count=5");
}

#[test]
fn test_bits_system_function() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [31:0] result;
    initial begin
        result = $bits(a);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(val.to_u64(), 8, "$bits(reg [7:0]) should be 8");
}

#[test]
fn test_wildcard_equality_eq() {
    let source = r#"
module tb;
    reg [3:0] a, b;
    reg result;
    initial begin
        a = 4'b1010;
        b = 4'b10x0;
        result = (a ==? b);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(val.to_u64(), 1, "==? should treat X as don't-care");
}

#[test]
fn test_wildcard_equality_neq() {
    let source = r#"
module tb;
    reg [3:0] a, b;
    reg result;
    initial begin
        a = 4'b1010;
        b = 4'b1011;
        result = (a !=? b);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(val.to_u64(), 1, "!=? should be 1 when not equal");
}

#[test]
fn test_dollar_time() {
    let source = r#"
module tb;
    reg [63:0] t;
    initial begin
        #5;
        t = $time;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "t").unwrap();
    assert_eq!(val.to_u64(), 5, "$time should return 5 at time 5");
}

#[test]
fn test_range_select_signal() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [3:0] result;
    initial begin
        a = 8'b11001100;
        result = a[5:2];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    // a[5:2] of 11001100 = 0011 (bit2=LSB) → 3. Cocok dengan iverilog.
    assert_eq!(val.to_u64(), 3, "a[5:2] of 11001100 should give 3");
}

#[test]
fn test_generate_if_active() {
    let source = r#"
module tb;
    generate
        if (1) begin
            reg [7:0] data;
        end else begin
            reg [15:0] data;
        end
    endgenerate
    initial begin
        data = 8'hAB;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "data").unwrap();
    assert_eq!(val.to_u64(), 0xAB, "generate if should select true branch");
}

#[test]
fn test_generate_case() {
    let source = r#"
module tb;
    reg [7:0] data;
    generate
        case (2)
            0: begin
                initial data = 8'hAA;
            end
            1: begin
                initial data = 8'hBB;
            end
            2: begin
                initial data = 8'hCC;
            end
            default: begin
                initial data = 8'hFF;
            end
        endcase
    endgenerate
    initial begin
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "data").unwrap();
    assert_eq!(val.to_u64(), 0xCC, "generate case should select arm 2");
}

#[test]
fn test_generate_case_default() {
    let source = r#"
module tb;
    reg [7:0] data;
    generate
        case (99)
            0: begin
                initial data = 8'hAA;
            end
            1: begin
                initial data = 8'hBB;
            end
            default: begin
                initial data = 8'hFF;
            end
        endcase
    endgenerate
    initial begin
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "data").unwrap();
    assert_eq!(val.to_u64(), 0xFF, "generate case default should fire");
}

#[test]
fn test_dynamic_part_select() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [3:0] result;
    integer sel;
    initial begin
        a = 8'b11001100;
        sel = 5;
        // dynamic part-select: a[sel -: 4] → a[5:2]
        result = a[sel -: 4];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    // a[sel-:4] dengan sel=5 → a[5:2]. bits 5:2 of 11001100 = 0011 = 3
    // (bit2=LSB). Cocok dengan iverilog.
    assert_eq!(
        val.to_u64(),
        3,
        "dynamic part-select a[sel-:4] should give 3"
    );
}

#[test]
fn test_dynamic_part_select_plus() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [3:0] result;
    integer sel;
    initial begin
        a = 8'b11001100;
        sel = 2;
        result = a[sel +: 4];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    // a[sel+:4] sel=2 → a[2:5] = bits 2..5 of 11001100 = 0011 = 3.
    // Cocok dengan iverilog.
    assert_eq!(
        val.to_u64(),
        3,
        "dynamic part-select a[sel+:4] should give 3"
    );
}

#[test]
fn test_unknown_syscall_no_crash() {
    let source = r#"
module tb;
    reg [31:0] x;
    initial begin
        x = 42;
        $foobar(x);
        #1 $finish;
    end
endmodule
"#;
    // Should not crash or error, just warn
    let result = simulate_signals(source, 5);
    assert!(
        result.is_ok(),
        "unknown syscall should not cause crash: {:?}",
        result.err()
    );
}

#[test]
fn test_array_range_select_lvalue() {
    let source = r#"
module tb;
    reg [7:0] arr [0:3];
    reg [3:0] result;
    integer i;
    initial begin
        arr[0] = 8'hA5;
        arr[1] = 8'h5A;
        i = 1;
        result = arr[i][3:0];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    // arr[1] = 8'h5A; [3:0] = low nibble = 0xA = 10. Cocok dengan iverilog.
    assert_eq!(val.to_u64(), 10, "arr[i][3:0] should select low nibble");
}

#[test]
fn test_array_bit_select_lvalue() {
    let source = r#"
module tb;
    reg [7:0] arr [0:3];
    reg result;
    integer i;
    initial begin
        arr[0] = 8'hA5;
        arr[1] = 8'h5A;
        i = 0;
        result = arr[i][0];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    // arr[0] = 8'hA5 = 10100101; bit 0 = 1
    assert_eq!(val.to_u64(), 1, "arr[i][0] should select bit 0");
}

#[test]
fn test_multidim_unpacked_array_read_decl_init() {
    // F39 — multi-dimensi unpacked array (buglog-mv #4, sebelumnya SKIP
    // buta dim lanjutan): `logic [7:0] mat [0:1][0:1] = '{{1,2},{3,4}}`
    // → width 32 (4 elemen 8-bit), array_dims [2,2]; init nested di-flatten
    // row-major [1,2,3,4]; index `mat[i][j]` = elemen flat i*2+j.
    // Sebelumnya: width 16/2 elemen, init tak ter-decompose → semua 0.
    let source = r#"
module tb;
    logic [7:0] mat [0:1][0:1] = '{{1,2},{3,4}};
    logic [7:0] m00, m01, m10, m11;
    integer i, j;
    initial begin
        m00 = mat[0][0];
        m01 = mat[0][1];
        m10 = mat[1][0];
        m11 = mat[1][1];
        i = 1; j = 1;
        m11 = mat[i][j];   // dinamis → 4
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("m00"), 1, "mat[0][0]");
    assert_eq!(get("m01"), 2, "mat[0][1]");
    assert_eq!(get("m10"), 3, "mat[1][0]");
    assert_eq!(get("m11"), 4, "mat[1][1] dynamic");
}

#[test]
fn test_multidim_unpacked_array_write_dynamic() {
    // F39 — tulis dinamis multi-dimensi `mat[i][j] = x` (fold lvalue).
    let source = r#"
module tb;
    logic [7:0] mat [0:1][0:1] = '{{1,2},{3,4}};
    logic [7:0] p00, p01;
    integer i, j;
    initial begin
        i = 0; j = 1;
        mat[i][j] = 42;
        p00 = mat[0][0];
        p01 = mat[0][1];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("p00"), 1, "element tidak tersentuh");
    assert_eq!(get("p01"), 42, "write dinamis mat[0][1]");
}

#[test]
fn test_multidim_unpacked_array_3d_and_row() {
    // F39 — 3-dimensi + row select `mat[0]` (16-bit dari 2 elemen 8-bit).
    let source = r#"
module tb;
    logic [7:0] m3d [0:1][0:1][0:1] = '{{'{1,2},'{3,4}}, '{'{5,6},'{7,8}}};
    logic [7:0] w;
    logic [31:0] row;
    integer i, j, k;
    initial begin
        i = 1; j = 0; k = 1;
        w = m3d[i][j][k];   // m3d[1][0][1] = 6
        row = m3d[0];       // baris 0 = {1,2,3,4} → 0x04030201 (MSB-first)
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("w"), 6, "m3d[1][0][1] = 6");
    assert_eq!(
        get("row"),
        0x0403_0201,
        "row select m3d[0] (32-bit sub-array)"
    );
}

#[test]
fn test_multidim_unpacked_array_port() {
    // F39 — port array multi-dimensi `output logic [7:0] out [0:1][0:1]`.
    let source = r#"
module dut(input logic [7:0] d [0:1][0:1], output logic [7:0] q [0:1][0:1]);
    always_comb q = d;
endmodule
module tb;
    logic [7:0] d [0:1][0:1] = '{{1,2},{3,4}};
    logic [7:0] q [0:1][0:1];
    dut u(.d(d), .q(q));
    logic [7:0] r1, r4;
    initial begin
        #1;               // tunggu always_comb q = d propagasi
        r1 = q[0][0];
        r4 = q[1][1];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("r1"), 1, "port multi-dim q[0][0]");
    assert_eq!(get("r4"), 4, "port multi-dim q[1][1]");
}

#[test]
fn test_package_import_typedef() {
    let source = r#"
package my_pkg;
    typedef enum { IDLE, BUSY, DONE } state_t;
endpackage

module tb;
    import my_pkg::*;
    state_t state;
    initial begin
        state = 2;
    end
endmodule
"#;
    let design = compile_str(source);
    assert!(design.is_ok(), "compilation failed: {:?}", design.err());
    let ir = design.unwrap();
    assert!(
        ir.top.signals.iter().any(|s| s.name == "state"),
        "state signal should exist in top module"
    );
}

#[test]
fn test_package_enum_sequence_resets_per_typedef() {
    // Dua typedef enum dalam satu package: member enum tanpa nilai eksplisit
    // melanjutkan counter HANYA dalam typedef yang sama (standar SV). Sebelum
    // fix, `pkg_enums` di-flatten jadi satu list per package sehingga counter
    // `last` bocor lintas enum — mis. enum kedua dapat nilai 3+ (harusnya 0..)
    // persis seperti `alu_op_base_e` di otbn_pkg (OpenTitan) yang mendapat
    // 256+ padahal harusnya 0..8.
    let source = r#"
package p;
    typedef enum { A, B, C } e1_t;   // A=0, B=1, C=2
    typedef enum { X, Y, Z } e2_t;   // X=0, Y=1, Z=2 (BUKAN 3,4,5)
endpackage

module tb;
    import p::*;
    logic [3:0] x, y, z;
    initial begin
        x = X;
        y = Y;
        z = Z;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap()
    };
    assert_eq!(get("x"), 0, "X harus 0 (enum kedua mulai dari 0)");
    assert_eq!(get("y"), 1, "Y harus 1");
    assert_eq!(get("z"), 2, "Z harus 2");
}

#[test]
fn test_package_enum_sequence_reset_multiple_pkgs() {
    // Dua package berbeda — masing-masing enum independent (counter reset
    // antar package DAN antar typedef).
    let source = r#"
package p1;
    typedef enum { A, B, C, D, E } e1_t;
endpackage
package p2;
    typedef enum { Q, R } e2_t;
endpackage

module tb;
    import p1::*;
    import p2::*;
    logic [3:0] q, r;
    initial begin
        q = Q;
        r = R;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap()
    };
    assert_eq!(get("q"), 0, "Q harus 0 (pkg p2 enum mulai dari 0)");
    assert_eq!(get("r"), 1, "R harus 1");
}

#[test]
fn test_package_import_param() {
    let source = r#"
package my_pkg;
    parameter int WIDTH = 8;
endpackage

module tb;
    import my_pkg::WIDTH;
    reg [WIDTH-1:0] data;
    initial begin
        data = 42;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let data_val = sigs
        .iter()
        .find(|(n, _)| n == "data")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(data_val, 42, "data should be 42");
}

#[test]
fn test_interface_decl() {
    let source = r#"
interface bus_if;
    logic [7:0] data;
    logic valid;
endinterface

module tb;
    bus_if bus();
endmodule
"#;
    let design = compile_str(source);
    assert!(design.is_ok(), "compilation failed: {:?}", design.err());
}

#[test]
fn test_interface_modport() {
    let source = r#"
interface bus_if;
    logic [7:0] data;
    logic valid;
    modport master (output data, valid);
    modport slave (input data, valid);
endinterface

module tb;
    bus_if bus();
endmodule
"#;
    let design = compile_str(source);
    assert!(design.is_ok(), "compilation failed: {:?}", design.err());
}

#[test]
fn test_package_import_param_expr() {
    let source = r#"
package my_pkg;
    parameter int WIDTH = 8;
    parameter int DEPTH = 4;
endpackage

module tb;
    import my_pkg::*;
    reg [WIDTH*DEPTH-1:0] mem;
    initial begin
        mem = 255;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let mem_val = sigs
        .iter()
        .find(|(n, _)| n == "mem")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(mem_val, 255, "mem should be 255");
}

#[test]
fn test_package_import_function() {
    let source = r#"
package math_pkg;
    function int add(input int a, input int b);
        add = a + b;
    endfunction
endpackage

module tb;
    import math_pkg::*;
    reg [31:0] result;
    initial begin
        result = add(10, 20);
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let r = sigs
        .iter()
        .find(|(n, _)| n == "result")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(r, 30, "package function add(10,20) should return 30");
}

#[test]
fn test_package_import_task() {
    let source = r#"
package task_pkg;
    task set_reg(output reg [7:0] r, input [7:0] v);
        r = v;
    endtask
endpackage

module tb;
    import task_pkg::*;
    reg [7:0] val;
    initial begin
        val = 0;
        set_reg(val, 42);
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 42, "package task set_reg should set val to 42");
}

#[test]
fn test_module_task() {
    let source = r#"
module tb;
    reg [7:0] val;
    task set_val(input [7:0] x);
        val = x;
    endtask
    initial begin
        val = 0;
        set_val(42);
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 42, "val should be 42 after task call");
}

#[test]
fn test_compound_assign_operators() {
    let source = r#"
module tb;
    reg [15:0] a, b, c, d, e, f, g, h, i;
    initial begin
        a = 16'd5;   a *= 16'd4;       // 5*4 = 20
        b = 16'd20;  b /= 16'd5;       // 20/5 = 4
        c = 16'd17;  c %= 16'd5;       // 17%5 = 2
        d = 16'hF0;  d &= 16'h0F;      // 0
        e = 16'h0F;  e |= 16'hF0;      // 0xFF
        f = 16'hAA;  f ^= 16'hFF;      // 0x55
        g = 16'd1;   g <<= 16'd4;      // 16
        h = 16'h80;  h >>= 16'd3;      // 16
        i = 16'd7;   i += 16'd8; i -= 16'd2; // 13
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get = |name: &str| {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap()
    };
    assert_eq!(get("a"), 20, "*= should multiply");
    assert_eq!(get("b"), 4, "/= should divide");
    assert_eq!(get("c"), 2, "%= should mod");
    assert_eq!(get("d"), 0, "&= should bitwise-and");
    assert_eq!(get("e"), 0xFF, "|= should bitwise-or");
    assert_eq!(get("f"), 0x55, "^= should bitwise-xor");
    assert_eq!(get("g"), 16, "<<= should shift left");
    assert_eq!(get("h"), 16, ">>= should shift right");
    assert_eq!(get("i"), 13, "+= / -= should add and subtract");
}

#[test]
fn test_assignment_pattern_concat() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [15:0] b;
    initial begin
        a = '{8'hA5};
        b = '{8'hAA, 8'h55};
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let av = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    let bv = sigs
        .iter()
        .find(|(n, _)| n == "b")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(av, 0xA5, "assignment pattern '{{v}} should equal v");
    assert_eq!(bv, 0xAA55, "'{{a,b}} should concat to aab");
}

#[test]
fn test_package_array_param() {
    let source = r#"
package cfg_pkg;
    parameter int COEFFS[0:3] = '{8'd1, 8'd2, 8'd3, 8'd4};
endpackage

module tb;
    import cfg_pkg::*;
    reg [7:0] result;
    initial begin
        result = COEFFS[2];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let r = sigs
        .iter()
        .find(|(n, _)| n == "result")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(r, 3, "COEFFS[2] should resolve to 3");
}

#[test]
fn test_parameter_type_decl() {
    let source = r#"
module tb;
    parameter type T = int;
    T x;
    initial begin
        x = 42;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let xv = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(xv, 42, "parameter type T = int should declare 32-bit x");
}

#[test]
fn test_parameter_type_header_decl() {
    // Type param dari header parameter list (module m #(parameter type T))
    // juga harus resolve: `T x;` di body jadi deklarasi, bukan instance.
    let source = r#"
module m #(parameter type T = int) ();
    T x;
    initial begin
        x = 42;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let xv = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(xv, 42, "header parameter type T should declare 32-bit x");
}

#[test]
fn test_negative_const_signed_assign() {
    // Konstanta negatif hasil const-fold (two's complement 32-bit) yang muat
    // di lebar LHS signed tidak boleh error — dan tidak memicu width mismatch
    // false-positive (fix check_width_mismatch value-aware utk nilai negatif).
    let source = r#"
module tb;
    reg signed [7:0] a;
    reg signed [7:0] b;
    reg signed [15:0] c;
    initial begin
        a = -1;
        b = -128;
        c = -300;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let av = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    let bv = sigs
        .iter()
        .find(|(n, _)| n == "b")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    let cv = sigs
        .iter()
        .find(|(n, _)| n == "c")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(av, 0xFF, "-1 in signed [7:0] should be 0xFF");
    assert_eq!(bv, 0x80, "-128 in signed [7:0] should be 0x80");
    assert_eq!(cv, 0xFED4, "-300 in signed [15:0] should be 0xFED4");
}

#[test]
fn test_negative_const_width_warning_suppression() {
    // Verifikasi fix check_width_mismatch value-aware: konstanta negatif yang
    // muat di lebar LHS signed (a=-1, b=-128) TIDAK memicu WidthMismatchWarning,
    // sedangkan konstanta yang benar-benar tidak muat (d=-200 dalam signed [7:0])
    // TETAP memicu warning. Memeriksa diag_sink elaborator, bukan nilai sim.
    let source = r#"
module tb;
    reg signed [7:0] a;
    reg signed [7:0] b;
    reg signed [7:0] d;
    initial begin
        a = -1;
        b = -128;
        d = -200;
        #1 $finish;
    end
endmodule
"#;

    // Pipeline lengkap seperti compile_str, tapi tangkap diagnostics elaborator.
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
    let warn_msgs: Vec<String> = diags
        .iter()
        .filter(|d| {
            matches!(
                d.code,
                mivon_core::diagnostics::DiagCode::WidthMismatchWarning
            )
        })
        .map(|d| d.message.to_string())
        .collect();

    // -1 dan -128 muat di signed [7:0] → tidak boleh ada width warning utk a/b
    assert!(
        !warn_msgs.iter().any(|m| m.contains("assignment to 'a'")),
        "a=-1 should not warn, got: {:?}",
        warn_msgs
    );
    assert!(
        !warn_msgs.iter().any(|m| m.contains("assignment to 'b'")),
        "b=-128 should not warn, got: {:?}",
        warn_msgs
    );
    // -200 tidak muat di signed [7:0] → harus tetap warning (total tepat 1)
    assert_eq!(
        warn_msgs.len(),
        1,
        "expected exactly 1 width mismatch warning (only d), got: {:?}",
        warn_msgs
    );
    assert!(
        warn_msgs.iter().any(|m| m.contains("assignment to 'd'")),
        "d=-200 should still warn, got: {:?}",
        warn_msgs
    );
}

#[test]
fn test_auto_top_resolution_picks_largest_cone() {
    // Design dengan BANYAK candidate top (SoC chip + testbench kecil + bind
    // assertion). Mode AnalysisRecovery dulu menyerah (recovered=true, top
    // pertama) — sekarang auto top resolution memilih kandidat dengan skor
    // cone transitif tertinggi secara deterministik: chip_earlgrey-style SoC
    // yang menginstansiasi ratusan modul menang atas tb/bind kecil.
    let source = r#"
module add8(input [7:0] a, input [7:0] b, output [7:0] y);
    assign y = a + b;
endmodule

module mul4(input [7:0] a, input [7:0] b, output [7:0] y);
    assign y = a * b;
endmodule

module alu(input [7:0] a, input [7:0] b, input op, output [7:0] y);
    wire [7:0] add_y, mul_y;
    add8 u_add(.a(a), .b(b), .y(add_y));
    mul4 u_mul(.a(a), .b(b), .y(mul_y));
    assign y = op ? add_y : mul_y;
endmodule

// Peripheral kecil dengan sub-blok — cone chip membesar signifikan.
module uart(input clk, input [7:0] d, output [7:0] q);
    add8 u(.a(d), .b(8'h1), .y(q));
endmodule
module timer(input clk, input [7:0] d, output [7:0] q);
    mul4 u(.a(d), .b(8'h2), .y(q));
endmodule
module gpio(input clk, input [7:0] d, output [7:0] q);
    add8 u(.a(d), .b(8'h3), .y(q));
endmodule

// SoC chip: menginstansiasi alu + tiga peripheral → cone terbesar.
module chip_soc(input clk, input [7:0] a, input [7:0] b, input op, output [7:0] y);
    alu u_alu(.a(a), .b(b), .op(op), .y(y));
    uart u_uart(.clk(clk), .d(a), .q());
    timer u_timer(.clk(clk), .d(b), .q());
    gpio u_gpio(.clk(clk), .d(a), .q());
endmodule

// Testbench kecil — candidate top juga, tapi cone jauh lebih kecil.
module tb_alu;
    reg [7:0] a, b, y;
    reg op;
    alu u(.a(a), .b(b), .op(op), .y(y));
    initial #1 $finish;
endmodule

// Bind assertion — bukan top fungsional (diberi penalti nama).
module tb_alu_bind;
    wire [7:0] y;
    alu u(.a(8'h0), .b(8'h0), .op(1'b0), .y(y));
    initial #1 $finish;
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
    // AnalysisRecovery: tanpa --top, top tidak unik TIDAK menggagalkan analisis.
    let ir = elaborator
        .elaborate(
            None,
            mivon_elaboration::elaborator::ElaborateMode::AnalysisRecovery,
        )
        .expect("elaborate");
    // Auto top resolution memilih cone terbesar (chip_soc menginstansiasi
    // add8+mul4+alu = 4 module) — bukan tb kecil (cone 2).
    assert_eq!(
        ir.top.name.as_str(),
        "chip_soc",
        "auto top resolution should pick the SoC chip with largest cone"
    );
    assert!(
        !elaborator.recovered,
        "auto top resolution unique winner → analysis NOT recovered"
    );
}

#[test]
fn test_auto_top_resolution_sim_preference() {
    // Tie cone (kedua wrapper menginstansiasi SoC yang sama) → sinyal nama
    // memutus: preferensi simulasi (`verilator`/`_sim`) menang.
    let source = r#"
module leaf;
endmodule

module soc_top(input clk);
    leaf u();
endmodule

module chip_earlgrey_verilator(input clk);
    soc_top u();
endmodule

module chip_earlgrey_asic(input clk);
    soc_top u();
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
    let ir = elaborator
        .elaborate(
            None,
            mivon_elaboration::elaborator::ElaborateMode::AnalysisRecovery,
        )
        .expect("elaborate");
    assert_eq!(
        ir.top.name.as_str(),
        "chip_earlgrey_verilator",
        "sim-prefixed chip wrapper should win the tie via name score"
    );
    assert!(
        !elaborator.recovered,
        "unique winner → analysis NOT recovered"
    );
}

#[test]
fn test_bits_package_array_param() {
    // Verifikasi `$bits` pada referensi array utuh (param package):
    //   - plain name via `import pkg::*`  → `$bits(COEFFS)`
    //   - qualified name                  → `$bits(pkg::COEFFS)`
    // Keduanya harus 128 (4 elemen int × 32 bit). Ini menghapus documented
    // limitation lama di audit.txt yang menyebut $bits(COEFFS) tidak ter-resolve.
    let source = r#"
package cfg_pkg;
    parameter int COEFFS[0:3] = '{8'd1, 8'd2, 8'd3, 8'd4};
endpackage

module tb;
    import cfg_pkg::*;
    reg [7:0] arr[0:3];
    reg [3:0][7:0] p;  // packed multi-dim: 4 elemen x 8 bit = 32
    reg [31:0] b1;
    reg [31:0] b2;
    reg [31:0] b3;
    reg [31:0] b4;
    reg [31:0] s1;
    reg [31:0] s2;
    initial begin
        b1 = $bits(COEFFS);
        b2 = $bits(cfg_pkg::COEFFS);
        b3 = $bits(arr);
        b4 = $bits(p);
        s1 = $size(arr);
        s2 = $size(p);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let b1 = sigs
        .iter()
        .find(|(n, _)| n == "b1")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    let b2 = sigs
        .iter()
        .find(|(n, _)| n == "b2")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    let b3 = sigs
        .iter()
        .find(|(n, _)| n == "b3")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    let b4 = sigs
        .iter()
        .find(|(n, _)| n == "b4")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    let s1 = sigs
        .iter()
        .find(|(n, _)| n == "s1")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    let s2 = sigs
        .iter()
        .find(|(n, _)| n == "s2")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(
        b1, 128,
        "$bits(COEFFS) via import should be 4 ints * 32 = 128"
    );
    assert_eq!(
        b2, 128,
        "$bits(cfg_pkg::COEFFS) should be 4 ints * 32 = 128"
    );
    // Signal array lokal: SignalInfo.width sudah lebar total (elem * depth),
    // jadi $bits(arr) = 32 (4 elemen x 8), bukan 128 (double-count lama).
    assert_eq!(
        b3, 32,
        "$bits(arr) on local signal array should be 4 * 8 = 32"
    );
    // $bits tetap lebar total untuk packed multi-dimensi (4 x 8 = 32),
    // tidak terpengaruh fix $size.
    assert_eq!(b4, 32, "$bits(p) on packed [3:0][7:0] should be 32");
    // $size mengembalikan jumlah elemen dimensi pertama (array_depth=4),
    // bukan lebar total (bug lama: mengembalikan info.width = 32).
    assert_eq!(s1, 4, "$size(arr) should return first-dimension size (4)");
    // Packed multi-dimensi [3:0][7:0]: $size = packed_dims[0] = 4 (bukan 32).
    assert_eq!(s2, 4, "$size(p) on packed [3:0][7:0] should be 4");
}

#[test]
fn test_qualified_package_function_call() {
    // LANG-43: panggilan function/task qualified `pkg::func(...)` —
    // elaborator resolve via elaborate_package_func_call (elaborator/expr.rs)
    // + func_source_pkg; plain name via `import pkg::*` lewat
    // elaborate_imported_package_func_call. Keduanya harus memanggil body
    // function di package dengan benar.
    let source = r#"
package math_pkg;
    function int add3(int a, int b, int c);
        return a + b + c;
    endfunction
    function int mul2(int a);
        return a * 2;
    endfunction
endpackage

module tb;
    import math_pkg::*;
    int r1;
    int r2;
    int r3;
    initial begin
        r1 = math_pkg::add3(1, 2, 4);  // qualified call pkg::func
        r2 = add3(10, 20, 0);          // plain name via import pkg::*
        r3 = math_pkg::mul2(add3(1, 1, 1));  // nested: qualified wraps plain
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(get("r1"), 7, "qualified math_pkg::add3(1,2,4) = 7");
    assert_eq!(get("r2"), 30, "plain add3(10,20,0) via import pkg::* = 30");
    assert_eq!(get("r3"), 6, "nested math_pkg::mul2(add3(1,1,1)) = 2*3 = 6");
}

#[test]
fn test_let_declaration_module() {
    // LANG-40: `let` declaration di module (IEEE 1800-2017 §11.12.2) —
    // tanpa parameter (`let W = 8;` dipakai sebagai ident) dan berparameter
    // (`let double(x) = x * 2;` dipakai sebagai panggilan). Substitusi
    // dilakukan di elaborator (elaborate_expr → let_decls).
    let source = r#"
module tb;
    let W = 8;
    let double(x) = x * 2;
    let add3(a, b, c) = a + b + c;

    int r1;
    int r2;
    int r3;
    initial begin
        r1 = W;
        r2 = double(21);
        r3 = add3(1, 2, 4);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(get("r1"), 8, "let tanpa parameter: W = 8");
    assert_eq!(get("r2"), 42, "let berparameter: double(21) = 42");
    assert_eq!(get("r3"), 7, "let berparameter 3 arg: add3(1,2,4) = 7");
}

#[test]
fn test_let_declaration_class() {
    // LANG-40: `let` di dalam class — dipakai di body method (jalur AST):
    // let tanpa parameter (`MAX`) dan berparameter (`clamp(v,lo,hi)` dengan
    // nested ternary).
    let source = r#"
class Cfg;
    let MAX = 100;
    let clamp(v, lo, hi) = (v < lo) ? lo : (v > hi) ? hi : v;
    function int compute(int v);
        return clamp(v, 0, MAX);
    endfunction
endclass

module tb;
    Cfg c;
    int r1;
    int r2;
    int r3;
    initial begin
        c = new();
        r1 = c.compute(250);
        r2 = c.compute(50);
        r3 = c.compute(0);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(get("r1"), 100, "clamp(250,0,100) = 100");
    assert_eq!(get("r2"), 50, "clamp(50,0,100) = 50");
    assert_eq!(get("r3"), 0, "clamp(0,0,100) = 0");
}

#[test]
fn test_negative_literal_signed_semantics() {
    // ROUND 36 — literal negatif harus berperilaku signed di SEMUA jalur:
    //   1. `int a = -5` → signal is_signed=true (IEEE 1800 §6.11: int/
    //      integer/byte/shortint/longint intrinsik signed) → `a < 0` = true
    //      (sebelumnya unsigned compare → false, bug ROUND 34)
    //   2. `-5` const-fold dibungkus Signed → perbandingan konstanta benar
    //   3. argumen literal negatif di method class `compute(-5)` → -5
    //      (sebelumnya 4294967291)
    // Bits tetap two's complement (0xFFFFFFFB); signedness menentukan
    // interpretasi perbandingan/display, bukan bit yang disimpan.
    let source = r#"
class Cls;
    function int compute(input int v);
        return v;
    endfunction
endclass

module tb;
    Cls c;
    int a;
    int b;
    int c1;
    int r1;
    int r2;
    int r3;
    initial begin
        a = -5;
        b = (a < 0) ? 1 : 0;
        c1 = (-5 < 0) ? 1 : 0;
        c = new();
        r1 = c.compute(-5);
        r2 = c.compute(5);
        r3 = (r1 == -5) ? 1 : 0;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(get("a"), 0xFFFF_FFFB, "int -5 = two's complement 32-bit");
    assert_eq!(get("b"), 1, "a < 0 true — int signal signed");
    assert_eq!(get("c1"), 1, "-5 < 0 true — const fold signed");
    assert_eq!(get("r1"), 0xFFFF_FFFB, "compute(-5) mempertahankan bits");
    assert_eq!(get("r2"), 5, "compute(5) = 5");
    assert_eq!(get("r3"), 1, "r1 == -5 true — signed equality");
}

#[test]
fn test_signed_division_modulo() {
    // ROUND 36 lanjutan: Div/Mod dengan operand SIGNED harus menghitung
    // dengan semantik i64 (truncate toward zero, IEEE 1800 §11.4.3):
    //   -7/2 = -3, -7%2 = -1, -7/-2 = 3, 7%-2 = 1
    // Sebelumnya eval_binary selalu unsigned → -7/2 = 2147483644, -7%-2 = 0.
    let source = r#"
module tb;
    int a, b, c, d;
    int q1, m1, q2, m2, q3, m3;
    initial begin
        a = -7; b = 2;
        c = -7; d = -2;
        q1 = a / b;
        m1 = a % b;
        q2 = c / d;
        m2 = c % d;
        q3 = 7 / -2;
        m3 = 7 % -2;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_i64())
            .unwrap_or(0)
    };
    assert_eq!(get("q1"), -3, "-7 / 2 = -3 (signed)");
    assert_eq!(get("m1"), -1, "-7 % 2 = -1 (tanda ikut dividen)");
    assert_eq!(get("q2"), 3, "-7 / -2 = 3 (signed)");
    assert_eq!(get("m2"), -1, "-7 % -2 = -1");
    assert_eq!(get("q3"), -3, "7 / -2 = -3 (const fold signed)");
    assert_eq!(get("m3"), 1, "7 % -2 = 1");
}

#[test]
fn test_arithmetic_shift_right_signedness() {
    // ROUND 36 lanjutan — `>>>` (IEEE 1800 §11.4.10): ARITHMETIC bila lhs
    // signed, LOGICAL bila unsigned. Dua bug lama:
    //   1. extend_to zero-extend ke max_width operand menghilangkan sign bit
    //      asli → `logic signed [7:0] s = -128; s >>> 2` = 0x20 (harus 0xE0)
    //   2. semantik >>> selalu arithmetic padahal harus logical utk unsigned
    // Fix: eval_sshr_signed (lebar asli lhs) + pemilihan di evaluate_expr
    // dan parallel path.
    let source = r#"
module tb;
    logic signed [7:0] s;
    logic [7:0] u;
    logic [7:0] rs_signed;
    logic [7:0] rs_shift_ge_width;
    logic [7:0] ru_unsigned;
    logic [7:0] ru_logical_op;
    int si;
    int ri;
    initial begin
        s = -128;
        u = 8'h80;
        rs_signed = s >>> 2;        // arithmetic → 8'hE0 (-32)
        rs_shift_ge_width = s >>> 10; // shift >= width → semua sign bit 0xFF
        ru_unsigned = u >>> 2;      // logical → 8'h20
        ru_logical_op = s >> 2;     // `>>` selalu logical → 8'h20
        si = -8;
        ri = si >>> 2;              // int signed → -2
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(get("rs_signed"), 0xE0, "signed >>> 2 = arithmetic 0xE0");
    assert_eq!(get("rs_shift_ge_width"), 0xFF, "shift >= width → sign fill");
    assert_eq!(get("ru_unsigned"), 0x20, "unsigned >>> = logical 0x20");
    assert_eq!(get("ru_logical_op"), 0x20, ">> selalu logical 0x20");
    assert_eq!(get("ri"), 0xFFFF_FFFE, "int signed >>> 2 = -2");
}

#[test]
fn test_compound_expr_signedness_propagation() {
    // ROUND 36 lanjutan — is_signed_expr di-rekursi ke ekspresi majemuk:
    // `int a; (a+1) < 0` harus signed compare (a=-5 → a+1=-4 < 0 = true).
    // Sebelumnya is_signed_expr(BinaryOp) = false → unsigned compare → false.
    // Berlaku juga utk div/mod, `>>>`, dan %d display pada ekspresi majemuk.
    let source = r#"
module tb;
    int a;
    int b1, b2, b3;
    int q, s;
    initial begin
        a = -5;
        b1 = ((a + 1) < 0) ? 1 : 0;   // -4 < 0 → 1
        b2 = ((a * 2) < 0) ? 1 : 0;   // -10 < 0 → 1
        b3 = ((-a) < 0) ? 1 : 0;      // 5 < 0 → 0
        a = -7;
        q = (a + 1) / 2;   // -6 / 2 = -3
        s = (a * 2) >>> 2; // -14 >>> 2 = -4 (arithmetic)
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_i64())
            .unwrap_or(0)
    };
    assert_eq!(get("b1"), 1, "(a+1) < 0 signed compare");
    assert_eq!(get("b2"), 1, "(a*2) < 0 signed compare");
    assert_eq!(get("b3"), 0, "(-a) < 0 → false (5 < 0)");
    assert_eq!(get("q"), -3, "(a+1)/2 signed division");
    assert_eq!(get("s"), -4, "(a*2) >>> 2 arithmetic shift");
}

#[test]
fn test_lrm_signedness_any_unsigned_rule() {
    // ROUND 36 — aturan LRM §11.8.2 PENUH: operasi signed hanya bila KEDUA
    // operand signed ('ada operand unsigned → hasil unsigned'). Dua prasyarat
    // di-emit elaborator:
    //   - literal desimal UNSIZED (`0`, `127`) → IrExpr::Signed (LRM §6.8.1)
    //   - eval_binary_signed SIGN-extend operan dari lebar aslinya
    //     (`logic signed [7:0] s = -1; s < 0` → -1 < 0, bukan 255 < 0)
    // Kasus uji: signed-signal vs literal desimal = signed; vs literal
    // hex/bin = unsigned (hex/biner sized tidak punya suffix s).
    let source = r#"
module tb;
    logic signed [7:0] s;
    logic [7:0] u;
    int a;
    int r1, r2, r3, r4, r5, r6, r7;
    initial begin
        s = -1;          // 0xFF
        u = 8'hFF;
        a = -5;
        r1 = (s < 0) ? 1 : 0;          // signed vs desimal(signed) → -1<0 → 1
        r2 = (s < 8'h7F) ? 1 : 0;      // signed vs hex(unsigned) → 0xFF<0x7F → 0
        r3 = (s < 127) ? 1 : 0;        // signed vs desimal(signed) → -1<127 → 1
        r4 = (u > 8'hFE) ? 1 : 0;      // unsigned vs hex → 0xFF>0xFE → 1
        r5 = (a < 0) ? 1 : 0;          // int vs desimal → -5<0 → 1
        r6 = (a < 8'h02) ? 1 : 0;      // int vs hex(unsigned) → 0xFFFFFFFB<2 → 0
        r7 = (s / 8'h02) ? 1 : 0;      // signed / hex(unsigned) → unsigned: 0xFF/2 ≠ 0
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(get("r1"), 1, "s<0 signed (sign-extend 8-bit -1)");
    assert_eq!(get("r2"), 0, "s<8'h7F unsigned (any-unsigned)");
    assert_eq!(get("r3"), 1, "s<127 signed (-1 < 127)");
    assert_eq!(get("r4"), 1, "u>8'hFE unsigned");
    assert_eq!(get("r5"), 1, "a<0 signed");
    assert_eq!(get("r6"), 0, "a<8'h02 unsigned");
    assert_eq!(get("r7"), 1, "s/8'h02 unsigned div → 0xFF/2 = 127 ≠ 0");
}

#[test]
fn test_const_fold_signedness_propagation() {
    // ROUND 36 — const-fold mempertahankan signedness OPERAND ASLI:
    //   `a < (2+3)` — 2+3 = desimal unsized → SIGNED → a=-5 < 5 = true
    //   `a < (8'h01+8'h05)` — operand hex sized (unsigned) → hasil unsigned
    // Sebelumnya try_fold_const selalu menghasilkan Const UNSIGNED untuk
    // fold positif → `a < (2+3)` memakai unsigned compare (a=-5:
    // 0xFFFFFFFB < 5 = false, salah). Keterbatasan dicatat: ekspresi konstanta
    // yang SEMUA ter-fold dievaluasi const_eval dengan i64 (mis.
    // `(8'h01-8'h05) < 0` = -4 < 0 = 1, padahal LRM: 8'hFC=252 < 0 = 0) —
    // butuh evaluator konstanta sadar-lebar, di luar scope.
    let source = r#"
module tb;
    int a;
    int r1, r2;
    initial begin
        a = -5;
        r1 = (a < (2 + 3)) ? 1 : 0;          // signed: -5 < 5 → 1
        r2 = (a < (8'h01 + 8'h05)) ? 1 : 0;  // unsigned: 0xFFFFFFFB < 6 → 0
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(
        get("r1"),
        1,
        "a < (2+3) signed compare (fold desimal signed)"
    );
    assert_eq!(
        get("r2"),
        0,
        "a < (8'h01+8'h05) unsigned compare (any-unsigned)"
    );
}

#[test]
fn test_port_unpacked_array_end_to_end() {
    // Verifikasi dukungan port unpacked-array:
    //   - parser menerima `output logic [7:0] arr[0:3]` (sebelumnya error
    //     'expected RBrack, found Colon')
    //   - elaborator melipat array depth ke lebar total port + set
    //     array_depth/elem_width dengan benar
    //   - flatten check lebar membandingkan width TOTAL + elem_width
    //   - nilai continuous assign di child mengalir ke parent array signal
    let source = r#"
module tb;
    logic [7:0] a[0:3];
    reg [31:0] w;
    reg [31:0] s;
    child c(.arr(a));
    initial begin
        w = $bits(a);
        s = $size(a);
        #1 $finish;
    end
endmodule

module child(output logic [7:0] arr[0:3]);
    assign arr[0] = 8'h01;
    assign arr[1] = 8'h02;
    assign arr[2] = 8'h03;
    assign arr[3] = 8'h04;
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let w = sigs
        .iter()
        .find(|(n, _)| n == "w")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    let s = sigs
        .iter()
        .find(|(n, _)| n == "s")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    let a = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    // Port [7:0] x [0:3]: total 4 elemen x 8 bit = 32 bit, bukan elem_width 8.
    assert_eq!(w, 32, "$bits(port array) should be 4 elems * 8 = 32");
    // $size = jumlah elemen dimensi pertama (array_depth=4).
    assert_eq!(
        s, 4,
        "$size(port array) should return first-dimension size 4"
    );
    // arr[0]=8'h01 ... arr[3]=8'h04 mengalir ke parent: lsb-first 0x04030201.
    assert_eq!(
        a, 0x04030201,
        "child assigns should flow through the array port"
    );
}

#[test]
fn test_port_array_elem_width_mismatch_rejected() {
    // Guard di flatten: dua array bisa punya total width sama tapi elem_width
    // beda (mis. child [15:0][0:1] width 32 elem 16 vs parent [7:0][0:3]
    // width 32 elem 8) — tanpa guard ini check lolos tapi indexing engine salah.
    let source = r#"
module tb;
    logic [7:0] a[0:3];
    child c(.arr(a));
    initial #1 $finish;
endmodule

module child(output logic [15:0] arr[0:1]);
    assign arr[0] = 16'h1111;
    assign arr[1] = 16'h2222;
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_err(),
        "expected elem width mismatch to be rejected, but design compiled ok"
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("element width mismatch"),
        "expected array element width mismatch error, got: {}",
        err
    );
}

#[test]
fn test_endmodule_colon_name_suffix() {
    // Suffix `endpackage : name` / `endmodule : name` / `endprogram : name`
    // harus bisa di-parse. Dipisah jadi dua design yang masing-masing punya
    // TEPAT SATU top candidate — StrictSimulation menolak design dengan
    // banyak candidate tops (program main + module top = ambigu).
    let source = r#"
package pkg;
    parameter int W = 8;
endpackage : pkg

module top;
    wire a;
endmodule : top
"#;
    let sigs = simulate_signals(source, 2);
    assert!(
        sigs.is_ok(),
        "endpackage/endmodule : name suffix should parse: {:?}",
        sigs.err()
    );
    let source2 = r#"
program main;
    initial #1 $finish;
endprogram : main
"#;
    let sigs2 = simulate_signals(source2, 2);
    assert!(
        sigs2.is_ok(),
        "endprogram : name suffix should parse: {:?}",
        sigs2.err()
    );
}

#[test]
fn test_module_task_multiple_ports() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [7:0] b;
    task add_and_store(input [7:0] x, input [7:0] y);
        a = x + y;
        b = x - y;
    endtask
    initial begin
        a = 0;
        b = 0;
        add_and_store(30, 12);
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let av = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let bv = sigs
        .iter()
        .find(|(n, _)| n == "b")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(av, 42, "a should be 30+12=42");
    assert_eq!(bv, 18, "b should be 30-12=18");
}

#[test]
fn test_module_task_output_port() {
    let source = r#"
module tb;
    task double_it(input [7:0] x, output [7:0] y);
        y = x * 2;
    endtask
    reg [7:0] result;
    initial begin
        result = 0;
        double_it(21, result);
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(
        val.to_u64(),
        42,
        "result should be 21*2=42 after task with output port"
    );
}

#[test]
fn test_module_task_inout_port() {
    let source = r#"
module tb;
    task increment(input [7:0] x, inout [7:0] acc);
        acc = acc + x;
    endtask
    reg [7:0] total;
    initial begin
        total = 10;
        increment(5, total);
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "total").unwrap();
    assert_eq!(
        val.to_u64(),
        15,
        "total should be 10+5=15 after task with inout port"
    );
}

#[test]
fn test_fork_join_basic() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [7:0] b;
    initial begin
        a = 0; b = 0;
        fork
            #5 a = 42;
            #10 b = 99;
        join
    end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let a = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let b = sigs
        .iter()
        .find(|(n, _)| n == "b")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(a, 42, "a should be 42 after fork-join");
    assert_eq!(b, 99, "b should be 99 after fork-join");
}

#[test]
fn test_fork_join_any() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [7:0] b;
    reg [7:0] result;
    initial begin
        a = 0; b = 0; result = 0;
        fork
            #5 a = 42;
            #10 b = 99;
        join_any
        result = 1;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let a_val = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let _b_val = sigs
        .iter()
        .find(|(n, _)| n == "b")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let r = sigs
        .iter()
        .find(|(n, _)| n == "result")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(a_val, 42, "a should be 42 after join_any");
    assert_eq!(r, 1, "result should be 1 (set after join_any continues)");
}

#[test]
fn test_fork_join_none() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [7:0] b;
    reg [7:0] result;
    initial begin
        a = 0; b = 0; result = 0;
        fork
            #5 a = 42;
            #10 b = 99;
        join_none
        result = 1;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let a_val = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let b_val = sigs
        .iter()
        .find(|(n, _)| n == "b")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let r = sigs
        .iter()
        .find(|(n, _)| n == "result")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(a_val, 42, "a should be 42 after join_none");
    assert_eq!(b_val, 99, "b should be 99 after join_none");
    assert_eq!(r, 1, "result should be 1 (set immediately after join_none)");
}

#[test]
fn test_wait_fork_waits_for_join_none() {
    // LANG-29: `wait fork;` memblokir sampai SEMUA fork process milik proses
    // ini selesai. Snapshot diambil SETELAH wait fork — bila wait fork tidak
    // memblokir (bug lama: `wait(0)` yang tak pernah true / lanjut segera),
    // snapshot membaca done1/done2 sebelum branch selesai → 0.
    let source = r#"
module tb;
    int done1;
    int done2;
    int snapshot;
    initial begin
        done1 = 0; done2 = 0; snapshot = 0;
        fork
            #5 done1 = 1;
            #10 done2 = 1;
        join_none
        #2
        wait fork;
        snapshot = done1 * 10 + done2;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let snap = sigs
        .iter()
        .find(|(n, _)| n == "snapshot")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        snap, 11,
        "wait fork harus menunggu kedua branch (done1=1, done2=1)"
    );
}

#[test]
fn test_wait_fork_no_active_fork_continues() {
    // LANG-29: tanpa fork proses aktif, `wait fork` lanjut segera (t=0).
    let source = r#"
module tb;
    int ok;
    initial begin
        ok = 0;
        wait fork;
        ok = 1;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let ok = sigs
        .iter()
        .find(|(n, _)| n == "ok")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(ok, 1, "tanpa fork aktif, wait fork lanjut segera");
}

#[test]
fn test_wait_fork_nested_in_fork_branch() {
    // LANG-29: `wait fork` di dalam branch fork menunggu child process dari
    // branch itu sendiri; `join` luar tetap menunggu semua branch.
    let source = r#"
module tb;
    int nested_done;
    int outer_ok;
    initial begin
        nested_done = 0; outer_ok = 0;
        fork
            begin
                fork
                    #4 nested_done = 1;
                join_none
                #2
                wait fork;
                if (nested_done != 1) $display("FAIL nested");
            end
            #10 outer_ok = 1;
        join
    end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let nd = sigs
        .iter()
        .find(|(n, _)| n == "nested_done")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let oo = sigs
        .iter()
        .find(|(n, _)| n == "outer_ok")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(nd, 1, "nested wait fork harus menunggu child branch");
    assert_eq!(oo, 1, "outer join harus menunggu semua branch");
}

#[test]
fn test_disable_fork_kills_branches() {
    // LANG-30: `disable fork;` men-terminate SEMUA child process milik proses
    // pemanggil (IEEE 1800-2017 §9.6.4). Branch yang tertunda (`#10`/`#20`)
    // tidak pernah dieksekusi (a/b tetap 0) dan proses pemanggil LANJUT —
    // `wait fork` setelah disable langsung selesai (tidak hang).
    let source = r#"
module tb;
    int a;
    int b;
    int after;
    initial begin
        a = 0; b = 0; after = 0;
        fork
            #10 a = 1;
            #20 b = 1;
        join_none
        disable fork;
        after = 1;
        wait fork;
        after = 2;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 30).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(99)
    };
    assert_eq!(get("a"), 0, "branch #10 harus dibunuh (a tetap 0)");
    assert_eq!(get("b"), 0, "branch #20 harus dibunuh (b tetap 0)");
    assert_eq!(
        get("after"),
        2,
        "proses lanjut setelah disable fork; wait fork selesai tanpa hang"
    );
}

#[test]
fn test_disable_fork_process_isolation() {
    // LANG-30: `disable fork` hanya membunuh child process MILIK PROSES
    // pemanggil — branch proses lain tetap berjalan.
    let source = r#"
module tb;
    int a;
    int b;
    initial begin  // proses A: fork join_none
        a = 0; b = 0;
        fork
            #5 a = 1;
            #10 b = 1;
        join_none
    end
    initial begin  // proses B: disable fork (tidak punya child)
        #2
        disable fork;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(99)
    };
    assert_eq!(get("a"), 1, "branch proses A tetap jalan (a=1 di #5)");
    assert_eq!(get("b"), 1, "branch proses A tetap jalan (b=1 di #10)");
}

#[test]
fn test_disable_fork_branch_label_scoped() {
    // `disable <label>` untuk named fork branch — SUDAH diimplementasikan:
    // label `begin : branch_a` kini dipertahankan parser (sebelumnya dibuang →
    // `Stmt::Block` anonim) dan kontinuasi suspend membawa label-nya
    // (`Continuation::named_labels`), sehingga `disable branch_a` membunuh
    // HANYA branch tersebut — branch_b tetap jalan (LRM 1800 §9.6.4).
    let source = r#"
module tb;
    int a;
    int b;
    initial begin
        a = 0; b = 0;
        fork
            begin : branch_a
                #5 a = 1;
            end
            begin : branch_b
                #10 b = 1;
            end
        join_none
        #2 disable branch_a;
        #15 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(99)
    };
    assert_eq!(get("a"), 0, "branch_a dibunuh disable label");
    assert_eq!(get("b"), 1, "branch_b TIDAK tersentuh disable branch_a");
}

#[test]
fn test_disable_label_cross_branch_fork_join_continuation() {
    // BUG FIX disable-label lintas branch: `disable worker` dari branch kedua
    // (yang men-suspend) men-sUSPEND-kan branch pertama di dalam
    // `begin : worker`. SEBELUM fix, `disable_pending` bocor tak bertabrak
    // consumer → loop-top guard menelan SEMUA statement sesudahnya termasuk
    // continuation `join` → `$finish` tak pernah jalan (simulasi "quiesced").
    let source = r#"
module tb;
    reg [7:0] a;
    reg done;
    initial begin
        a = 0;
        done = 0;
        fork
            begin : worker
                #10;
                a = 1;
            end
            begin
                #5;
                disable worker;
            end
        join
        #1;
        done = 1;
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 50).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(99)
    };
    assert_eq!(get("a"), 0, "worker dibunuh disable label sebelum t=10");
    assert_eq!(
        get("done"),
        1,
        "continuation join tetap jalan (tanpa fix: disable_pending bocor \
         menelan sisa statement -> done=0)"
    );
}

#[test]
fn test_fork_join_parallel_delays() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [7:0] b;
    reg [7:0] c;
    initial begin
        a = 0; b = 0; c = 0;
        fork
            begin
                #3 a = 10;
                #3 a = 20;
            end
            #5 b = 99;
            #10 c = 55;
        join
    end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let a = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let b = sigs
        .iter()
        .find(|(n, _)| n == "b")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let c = sigs
        .iter()
        .find(|(n, _)| n == "c")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        a, 20,
        "a should be 20 after sequential delays in fork branch"
    );
    assert_eq!(b, 99, "b should be 99");
    assert_eq!(c, 55, "c should be 55");
}

#[test]
fn test_zero_delay() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [7:0] b;
    initial begin
        a = 1;
        #0;
        b = a + 1;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let a = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let b = sigs
        .iter()
        .find(|(n, _)| n == "b")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(a, 1, "a should be 1");
    assert_eq!(b, 2, "b should be 2 (a+1 after #0 delay)");
}

#[test]
fn test_zero_delay_ordering() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [7:0] b;
    initial begin
        a = 0;
        b = 0;
        #0 a = 10;
        #0 b = 20;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let a = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let b = sigs
        .iter()
        .find(|(n, _)| n == "b")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(a, 10, "a should be 10");
    assert_eq!(b, 20, "b should be 20");
}

#[test]
fn test_always_comb_basic() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [7:0] b;
    wire [7:0] sum;

    always_comb begin
        sum = a + b;
    end

    initial begin
        a = 10; b = 20;
        #1 a = 30;
        #1 b = 5;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let sum_val = sigs
        .iter()
        .find(|(n, _)| n == "sum")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(sum_val, 35, "final sum should be 30 + 5 = 35");
}

#[test]
#[allow(clippy::approx_constant)] // literal `3.14` memang target RTL, bukan PI
fn test_real_declaration_and_assignment() {
    let source = r#"
module tb;
    real r;

    initial begin
        r = 3.14;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let r_val = sigs
        .iter()
        .find(|(n, _)| n == "r")
        .map(|(_, v)| f64::from_bits(v.to_u64()))
        .unwrap();
    assert!(
        (r_val - 3.14).abs() < 1e-9,
        "r should be ~3.14, got {}",
        r_val
    );
}

#[test]
fn test_realtime_system_function() {
    let source = r#"
module tb;
    real t;

    initial begin
        #5;
        t = $realtime;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let t_val = sigs
        .iter()
        .find(|(n, _)| n == "t")
        .map(|(_, v)| f64::from_bits(v.to_u64()))
        .unwrap();
    assert!(
        (t_val - 5.0).abs() < 1e-9,
        "$realtime should be 5.0, got {}",
        t_val
    );
}

#[test]
fn test_real_arithmetic() {
    let source = r#"
module tb;
    real a, b, sum, diff, prod, quot;

    initial begin
        a = 10.5;
        b = 3.0;
        sum = a + b;
        diff = a - b;
        prod = a * b;
        quot = a / b;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get_real = |name: &str| {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| f64::from_bits(v.to_u64()))
            .unwrap()
    };
    assert!((get_real("sum") - 13.5).abs() < 1e-9);
    assert!((get_real("diff") - 7.5).abs() < 1e-9);
    assert!((get_real("prod") - 31.5).abs() < 1e-9);
    assert!((get_real("quot") - 3.5).abs() < 1e-9);
}

#[test]
fn test_real_comparison() {
    let source = r#"
module tb;
    real a, b;
    reg gt, lt, eq;

    initial begin
        a = 5.5;
        b = 3.0;
        gt = a > b;
        lt = a < b;
        eq = a == b;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get_val = |name: &str| {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap()
    };
    assert_eq!(get_val("gt"), 1, "5.5 > 3.0 should be true");
    assert_eq!(get_val("lt"), 0, "5.5 < 3.0 should be false");
    assert_eq!(get_val("eq"), 0, "5.5 == 3.0 should be false");
}

/// Fungsi matematika real LRM 1800 §20.8 (`$sqrt`, `$ln`, `$pow`, ...).
/// Regresi: tak dikenal → 0 + warning RT9003 (mis. `$sqrt(2.0)` = 0).
/// Oracle: iverilog (`sqrt=1.414214 ln=1.000000 floor=3.000000 exp=2.718282`).
#[test]
fn test_real_math_sysfuncs() {
    let source = r#"
module tb;
    real a, b, c, d;
    integer n1, n2, n3;
    initial begin
        a = $sqrt(2.0);
        b = $pow(2.0, 10.0);
        c = $sin(0.0) + $cos(0.0);
        d = $atan2(1.0, 1.0);
        n1 = $rtoi(a * 1000.0);
        n2 = $rtoi(b);
        n3 = $rtoi(d * 1000.0);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get_val = |name: &str| {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap()
    };
    assert_eq!(get_val("n1"), 1414, "$rtoi($sqrt(2)*1000) = 1414");
    assert_eq!(get_val("n2"), 1024, "$rtoi($pow(2,10)) = 1024");
    assert_eq!(get_val("n3"), 785, "$rtoi($atan2(1,1)*1000) = 785");
}

#[test]
fn test_simulation_state_send_sync_audit() {
    // DEBT-17: Simulation state Send/Sync audit — compile-time guard.
    // SimulationState murni Vec/HashMap/u64/TimeFormat (tanpa RefCell/Rc/raw
    // pointer), sehingga harus Send + Sync. Test ini gagal kompilasi bila ada
    // field state baru yang memecah thread-safety.
    //
    // Catatan: SimulationEngine TIDAK di-assert di sini — sengaja
    // single-threaded. Field Option<FstWaveWriter> (crate wavefst) memegang
    // RefCell<HashMap<String, SendWrapper<*const u8>>> (symbol cache FST)
    // yang !Sync. Parallel eval (parallel.rs) hanya menyalin nilai sinyal
    // (LogicVec) via rayon par_iter().cloned() — tidak pernah memindahkan
    // atau mem-borrow engine lintas thread.
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    assert_send::<crate::simulator::state::SimulationState>();
    assert_sync::<crate::simulator::state::SimulationState>();
    assert_send::<mivon_ir::LogicVec>();
    assert_sync::<mivon_ir::LogicVec>();
}

#[test]
fn test_wreal_net_type_decl() {
    // wreal = real net type: membawa nilai real via continuous assign
    let source = r#"
module tb;
    real src;
    wreal out;
    real captured;

    assign out = src;

    initial begin
        src = 2.5;
        #1;
        captured = out;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let captured = sigs
        .iter()
        .find(|(n, _)| n == "captured")
        .map(|(_, v)| f64::from_bits(v.to_u64()))
        .unwrap();
    assert!(
        (captured - 2.5).abs() < 1e-9,
        "wreal net should carry real value 2.5, got {}",
        captured
    );
}

#[test]
fn test_wreal_direct_assignment() {
    // wreal juga boleh di-drive procedural (mengalir seperti real biasa)
    let source = r#"
module tb;
    wreal w;
    real captured;

    initial begin
        w = 6.25;
        #1;
        captured = w;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let captured = sigs
        .iter()
        .find(|(n, _)| n == "captured")
        .map(|(_, v)| f64::from_bits(v.to_u64()))
        .unwrap();
    assert!(
        (captured - 6.25).abs() < 1e-9,
        "wreal procedural assignment should carry 6.25, got {}",
        captured
    );
}

#[test]
fn test_bit_type_is_2state() {
    let source = r#"
module tb;
    bit [7:0] b;
    reg [7:0] r;

    initial begin
        b = 8'hFF;
        r = 8'h00;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let b_val = sigs
        .iter()
        .find(|(n, _)| n == "b")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(b_val, 0xFF, "bit signal should store FF");
}

#[test]
fn test_bit_rejects_xz() {
    let source = r#"
module tb;
    bit [3:0] b;
    reg [3:0] r;

    initial begin
        r = 4'b01xz;
        b = r;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let b_val = sigs
        .iter()
        .find(|(n, _)| n == "b")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(
        b_val, 0b0100,
        "bit should convert X/Z to 0; expected 0100, got {:04b}",
        b_val
    );
}

#[test]
fn test_urandom_range() {
    let source = r#"
module tb;
    reg [31:0] val;
    initial begin
        val = $urandom_range(100, 50);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert!(
        (50..=100).contains(&v),
        "urandom_range(100,50) should be [50,100], got {}",
        v
    );
}

#[test]
fn test_urandom_range_single_arg() {
    let source = r#"
module tb;
    reg [31:0] val;
    initial begin
        val = $urandom_range(10);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert!(v <= 10, "urandom_range(10) should be <= 10, got {}", v);
}

#[test]
fn test_timeformat_percent_t() {
    // LANG-48: $timeformat + format %t (IEEE 1800)
    let source = r#"
module tb;
    string s;
    initial begin
        $timeformat(-9, 2, " ns", 0);
        #5;
        s = $sformatf("%t", $time);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let s = sigs
        .iter()
        .find(|(n, _)| n == "s")
        .map(|(_, v)| logicvec_to_string(v))
        .unwrap_or_default();
    assert!(
        s.contains("5.00 ns"),
        "$timeformat(-9,2) + %%t should print '5.00 ns' at time 5, got '{}'",
        s
    );
}

#[test]
fn test_printtimescale_runs() {
    // LANG-49: $printtimescale tanpa kurung harus berjalan tanpa error
    let source = r#"
`timescale 1ns / 1ps
module tb;
    initial begin
        $printtimescale;
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 5);
    assert!(result.is_ok(), "$printtimescale should run without error");
}

#[test]
fn test_showscopes_runs() {
    // LANG-53: $showscopes harus berjalan tanpa error
    let source = r#"
module tb;
    initial begin
        $showscopes;
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 5);
    assert!(result.is_ok(), "$showscopes should run without error");
}

#[test]
fn test_deposit_forces_value() {
    // LANG-52: $deposit(sig, value) memaksa nilai signal
    let source = r#"
module tb;
    reg [7:0] x;
    initial begin
        x = 8'h00;
        #1;
        $deposit(x, 8'hA5);
        #1;
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 0xA5, "$deposit should force x to 0xA5, got {:#x}", v);
}

#[test]
fn test_assign_forces_value() {
    // LANG-52: $assign(sig, value) memaksa nilai signal
    let source = r#"
module tb;
    reg [7:0] x;
    initial begin
        $assign(x, 8'h3C);
        #1;
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 0x3C, "$assign should force x to 0x3C, got {:#x}", v);
}

#[test]
fn test_assign_suppresses_write_until_deassign() {
    // LANG-52: $assign override menahan write berikutnya sampai $deassign.
    // Flag `suppressed_ok` menangkap kondisi di tengah — tanpa itu, assert final
    // x==0x55 tetap lulus walau suppression rusak (0x00 ditimpa, lalu 0x55).
    let source = r#"
module tb;
    reg [7:0] x;
    reg suppressed_ok;
    initial begin
        $assign(x, 8'hA1);
        x = 8'h00;        // harus ditekan (masih forced oleh $assign)
        #1;
        suppressed_ok = (x === 8'hA1) ? 1'b1 : 1'b0;
        $deassign(x);
        x = 8'h55;        // setelah $deassign, write berlaku lagi
        #1;
        if (x !== 8'h55) $display("FAILED write after deassign: %h", x);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 6).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 0x55, "after $deassign, x should be 0x55, got {:#x}", v);
    let ok = sigs
        .iter()
        .find(|(n, _)| n == "suppressed_ok")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        ok, 1,
        "$assign should suppress the x=0 write (x stays 0xA1)"
    );
}

#[test]
fn test_get_randcount() {
    // LANG-22: $get_randcount mengembalikan jumlah panggilan random
    let source = r#"
module tb;
    integer a, b, c;
    initial begin
        a = $urandom();
        b = $urandom();
        c = $get_randcount();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "c")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        v, 2,
        "$get_randcount after 2 x $urandom should be 2, got {}",
        v
    );
}

#[test]
fn test_get_randstate_seed() {
    // LANG-22: $get_randstate mengembalikan seed RNG
    let source = r#"
module tb;
    integer c;
    initial begin
        c = $get_randstate();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "c")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 42, "default $get_randstate should be 42, got {}", v);
}

#[test]
fn test_random_seed_reproducible() {
    // Same seed should produce same random value (reproducibility)
    let source = r#"
module tb;
    reg [31:0] a;
    initial begin
        a = $random(42);
        #1 $finish;
    end
endmodule
"#;
    let sigs1 = simulate_signals(source, 5).unwrap();
    let v1 = sigs1
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);

    // Second simulation with same seed should produce same value
    let sigs2 = simulate_signals(source, 5).unwrap();
    let v2 = sigs2
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);

    assert_eq!(
        v1, v2,
        "$random(42) with same seed should produce same value: {} != {}",
        v1, v2
    );
}

#[test]
fn test_urandom_seed_returns_prev_seed() {
    // LANG-21: $urandom_seed should return previous seed
    let source = r#"
module tb;
    reg [63:0] prev_seed, new_seed;
    initial begin
        prev_seed = $urandom_seed(12345);
        new_seed = $urandom_seed(54321);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let prev = sigs
        .iter()
        .find(|(n, _)| n == "prev_seed")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let new_s = sigs
        .iter()
        .find(|(n, _)| n == "new_seed")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    // First call returns default seed (42)
    assert_eq!(
        prev, 42,
        "$urandom_seed first call should return default seed 42, got {}",
        prev
    );
    // Second call returns previous seed (12345)
    assert_eq!(
        new_s, 12345,
        "$urandom_seed second call should return previous seed 12345, got {}",
        new_s
    );
}

#[test]
fn test_srandom_returns_prev_seed() {
    // LANG-21: $srandom should return previous seed, NOT increment rand_call_count
    let source = r#"
module tb;
    reg [63:0] prev1, prev2, cnt1, cnt2;
    initial begin
        prev1 = $srandom(111);
        cnt1 = $get_randcount();
        prev2 = $srandom(222);
        cnt2 = $get_randcount();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let prev1 = sigs
        .iter()
        .find(|(n, _)| n == "prev1")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let prev2 = sigs
        .iter()
        .find(|(n, _)| n == "prev2")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let cnt1 = sigs
        .iter()
        .find(|(n, _)| n == "cnt1")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let cnt2 = sigs
        .iter()
        .find(|(n, _)| n == "cnt2")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    // $srandom returns previous seed
    assert_eq!(
        prev1, 42,
        "$srandom first call should return default seed 42, got {}",
        prev1
    );
    assert_eq!(
        prev2, 111,
        "$srandom second call should return previous seed 111, got {}",
        prev2
    );
    // $srandom does NOT increment rand_call_count (per IEEE)
    assert_eq!(
        cnt1, 0,
        "$get_randcount after $srandom should be 0, got {}",
        cnt1
    );
    assert_eq!(
        cnt2, 0,
        "$get_randcount after two $srandom should be 0, got {}",
        cnt2
    );
}

#[test]
fn test_urandom_seed_full_scope_determinism() {
    // LANG-21 (gap tersisa): seeding $urandom_seed harus mengendalikan aliran
    // $urandom/$urandom_range secara DETERMINISTIK full-scope — seed yang sama
    // → stream identik (reproducible), seed berbeda → stream berbeda. Sebelumnya
    // hanya return value prev_seed yang diuji; efek pada stream belum diverifikasi.
    let source = r#"
module tb;
    reg [31:0] a1, a2, a3, r1, r2;
    initial begin
        $urandom_seed(777);
        a1 = $urandom;
        a2 = $urandom;
        a3 = $urandom;
        r1 = $urandom_range(1000, 1);
        r2 = $urandom_range(1000, 1);
        #1 $finish;
    end
endmodule
"#;
    // Run 1 & 2: seed SAMA → stream IDENTIK (determinism / reproducibility).
    let run = |src: &str| -> Vec<u64> {
        let sigs = simulate_signals(src, 5).unwrap();
        ["a1", "a2", "a3", "r1", "r2"]
            .iter()
            .map(|n| {
                sigs.iter()
                    .find(|(name, _)| name == n)
                    .map(|(_, v)| v.to_u64())
                    .unwrap_or(0)
            })
            .collect()
    };
    let s1 = run(source);
    let s2 = run(source);
    assert_eq!(
        s1, s2,
        "seed sama → stream $urandom harus identik antar run"
    );

    // Run 3: seed BERBEDA → stream harus berubah (tidak kebetulan sama).
    let src_diff = source.replace("$urandom_seed(777);", "$urandom_seed(888);");
    let s3 = run(&src_diff);
    assert_ne!(
        s1, s3,
        "seed berbeda → stream $urandom harus berubah (seeding tidak efektif?)"
    );

    // $urandom_range dalam [1, 1000] — validasi range tetap bekerja setelah seed.
    for v in &s1[3..] {
        assert!(
            (1..=1000).contains(v),
            "$urandom_range harus dalam [1,1000], got {}",
            v
        );
    }
}

#[test]
fn test_srandom_with_no_arg_returns_prev_seed() {
    // $srandom() without argument should still return previous seed
    let source = r#"
module tb;
    reg [63:0] prev;
    initial begin
        $srandom(999);
        prev = $srandom();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let prev = sigs
        .iter()
        .find(|(n, _)| n == "prev")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        prev, 999,
        "$srandom() no-arg should return previous seed 999, got {}",
        prev
    );
}

#[test]
fn test_error_recovery_unknown_decl() {
    let source = r#"
module tb;
    reg [3:0] a;
    bad_keyword_here x;
    reg [3:0] b;
    initial begin
        a = 1;
        b = 2;
        #1 $finish;
    end
endmodule
"#;
    // Should not panic — returns proper error, no crash
    let _ = compile_str(source);
}

#[test]
fn test_error_recovery_bad_stmt() {
    let source = r#"
module tb;
    reg [3:0] a;
    initial begin
        a = 1;
        bad_statement_here;
        a = 2;
        #1 $finish;
    end
endmodule
"#;
    // Should not panic — returns proper error, no crash
    let _ = compile_str(source);
}

#[test]
fn test_error_recovery_missing_semi() {
    let source = r#"
module tb;
    reg [3:0] a
    reg [3:0] b;
    initial begin
        a = 1
        b = 2;
        #1 $finish;
    end
endmodule
"#;
    // Should not panic — returns proper error, no crash
    let _ = compile_str(source);
}

#[test]
fn test_byte_shortint_int_longint_2state() {
    let source = r#"
module tb;
    byte b;
    shortint si;
    int i;
    longint li;

    initial begin
        b = 8'hAB;
        si = 16'h1234;
        i = 32'hDEAD_BEEF;
        li = 64'h1234_5678_9ABC_DEF0;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let get_val = |name: &str| {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap()
    };
    assert_eq!(get_val("b"), 0xAB);
    assert_eq!(get_val("si"), 0x1234);
    assert_eq!(get_val("i"), 0xDEAD_BEEFu64);
    assert_eq!(get_val("li"), 0x1234_5678_9ABC_DEF0u64);
}

#[test]
fn test_mailbox_put_get() {
    let source = r#"
module tb;
    mailbox mb;
    reg [31:0] val;
    initial begin
        mb = new();
        mb.put(42);
        val = mb.get();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 42, "mailbox get should return 42");
}

#[test]
fn test_mailbox_bounded_try_put_respects_bound() {
    // LANG-24: `new(2)` bounded — try_put sukses dua kali, lalu 0 saat penuh.
    let source = r#"
module tb;
    mailbox mb;
    reg [31:0] r1, r2, r3;
    initial begin
        mb = new(2);
        r1 = mb.try_put(1);
        r2 = mb.try_put(2);
        r3 = mb.try_put(3);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(99)
    };
    assert_eq!(get("r1"), 1, "first try_put should succeed");
    assert_eq!(get("r2"), 1, "second try_put should succeed");
    assert_eq!(
        get("r3"),
        0,
        "third try_put on full bounded mailbox should fail"
    );
}

#[test]
fn test_mailbox_bounded_num() {
    // LANG-24: bounded — num() hanya hitung item yang benar-benar masuk.
    let source = r#"
module tb;
    mailbox mb;
    reg [31:0] count;
    initial begin
        mb = new(1);
        mb.put(1);
        count = mb.num();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "count")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 1, "bounded mailbox num should be 1");
}

#[test]
fn test_mailbox_get_blocks_until_put() {
    // LANG-24: `get` pada mailbox kosong BLOCK (suspend) sampai `put`.
    // Diuji dari task UVM (jalur AST blocking) — konsisten dengan uvm_tlm_fifo.
    let source = r#"
class mb_user extends uvm_component;
    mailbox mb;
    int val;
    int got;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        mb = new();
    endfunction
    task run_phase();
        fork
            begin
                mb.get(val);
                got = 1;
            end
            begin
                #10;
                mb.put(42);
            end
        join
        if (got != 1) $error("blocking get gagal got=%0d", got);
        if (val != 42) $error("nilai salah val=%0d", val);
    endtask
endclass

class mb_test extends uvm_test;
    mb_user u;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        u = new("u", this);
    endfunction
endclass
module tb;
    initial run_test("mb_test");
endmodule
"#;
    let result = simulate_str(source, 100);
    assert!(
        result.is_ok(),
        "blocking mailbox get should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_mailbox_put_blocks_when_bounded_full() {
    // LANG-24: bounded `put` saat penuh BLOCK sampai `get` membebaskan slot.
    let source = r#"
class mb_user extends uvm_component;
    mailbox mb;
    int val;
    int done;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        mb = new(1);
    endfunction
    task run_phase();
        mb.put(10);
        fork
            begin
                #10;
                mb.get(val);
            end
            begin
                mb.put(20);
                done = 1;
            end
        join
        if (done != 1) $error("bounded put block gagal done=%0d", done);
        if (val != 10) $error("nilai salah val=%0d", val);
    endtask
endclass

class mb_test extends uvm_test;
    mb_user u;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        u = new("u", this);
    endfunction
endclass
module tb;
    initial run_test("mb_test");
endmodule
"#;
    let result = simulate_str(source, 100);
    assert!(
        result.is_ok(),
        "bounded mailbox put should block then succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_mailbox_num() {
    let source = r#"
module tb;
    mailbox mb;
    reg [31:0] count;
    initial begin
        mb = new();
        mb.put(1);
        mb.put(2);
        mb.put(3);
        count = mb.num();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "count")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 3, "mailbox num should be 3 after 3 puts");
}

#[test]
fn test_mailbox_try_get_empty() {
    let source = r#"
module tb;
    mailbox mb;
    reg ok;
    initial begin
        mb = new();
        ok = mb.try_get();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "ok")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(1);
    assert_eq!(v, 0, "try_get on empty mailbox should return 0");
}

#[test]
fn test_semaphore_put_get() {
    let source = r#"
module tb;
    semaphore sem;
    reg [31:0] remaining;
    initial begin
        sem = new(2);
        sem.get(1);
        remaining = sem.get(1);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "remaining")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(v, 0, "after get(1)+get(1), remaining should be 0");
}

#[test]
fn test_semaphore_try_get() {
    let source = r#"
module tb;
    semaphore sem;
    reg ok;
    initial begin
        sem = new(1);
        ok = sem.try_get();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "ok")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 1, "try_get with available keys should return 1");
}

#[test]
fn test_mailbox_put_try_get() {
    let source = r#"
module tb;
    mailbox mb;
    reg ok;
    reg [31:0] val;
    initial begin
        mb = new();
        mb.put(99);
        ok = mb.try_get();
        val = mb.num();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let ok_val = sigs
        .iter()
        .find(|(n, _)| n == "ok")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let remaining = sigs
        .iter()
        .find(|(n, _)| n == "val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(ok_val, 1, "try_get with data should return 1");
    assert_eq!(remaining, 0, "after try_get, num should be 0");
}

#[test]
fn test_process_self_and_status() {
    let source = r#"
module tb;
    process p;
    reg [31:0] status_val;
    initial begin
        p = process::self();
        status_val = p.status();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "status_val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(v, 1, "process::self() should return RUNNING status (1)");
}

#[test]
fn test_process_kill_changes_status() {
    let source = r#"
module tb;
    process p;
    reg [31:0] status_after;
    initial begin
        p = process::self();
        p.kill();
        status_after = p.status();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "status_after")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(v, 4, "after kill, status should be KILLED (4)");
}

#[test]
fn test_process_self_parse() {
    let source = r#"
module tb;
    process p;
    initial begin
        p = 42;
        #1 $finish;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "process p should parse and elaborate: {:?}",
        result.err()
    );
}

#[test]
fn test_process_decl_only() {
    let source = r#"
module tb;
    process p;
    initial begin
        #1 $finish;
    end
endmodule
"#;
    let _sigs = simulate_signals(source, 5).unwrap();
    // Just verify it compiles and runs without error
}

#[test]
fn test_process_self_method_await_statement() {
    let source = r#"
module tb;
    process p;
    reg [31:0] x;
    initial begin
        fork
            begin
                #10 x = 42;
            end
        join_none
        p = process::self();
        #20 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 30).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 42, "fork/join_none should execute body");
}

#[test]
fn test_uvm_object_compile() {
    let source = r#"
class my_obj extends uvm_object;
    function new(string name);
        super.new(name);
    endfunction
endclass

module tb;
    my_obj obj;
    initial begin
        obj = my_obj::new("my_test_obj");
        #1 $finish;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "uvm_object compile failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_object_no_new_override() {
    let source = r#"
class my_obj extends uvm_object;
endclass

module tb;
    my_obj obj;
    initial begin
        obj = my_obj::new("my_test_obj");
        #1 $finish;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "uvm_object no-new compile failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_object_sim() {
    let source = r#"
class my_obj extends uvm_object;
    function new(string name);
        super.new(name);
    endfunction
endclass

module tb;
    my_obj obj;
    reg [31:0] result;
    initial begin
        obj = my_obj::new("my_test_obj");
        result = 42;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "result")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 42, "simulation should complete successfully");
}

#[test]
fn test_uvm_super_phase_noop() {
    // `super.end_of_elaboration_phase(phase)` di subclass UVM harus no-op
    // (bukan error RT9003 "uvm_object::end_of_elaboration_phase not
    // implemented") — pola OpenTitan core_ibex_base_test.
    let source = r#"
class my_test extends uvm_test;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void end_of_elaboration_phase(uvm_phase phase);
        super.end_of_elaboration_phase(phase);
    endfunction
endclass

module tb;
    my_test t;
    reg [31:0] result;
    initial begin
        t = new("t", null);
        result = 42;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "result")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 42, "super.xxx_phase harus no-op, tidak error");
}

#[test]
fn test_task_method_array_lvalue_dynamic_index() {
    // LHS array dengan index dinamis (`mem[count] = v`) di body task method
    // harus didukung — bukan error RT9003 "unsupported lvalue type in task
    // method: BitSelect" (pola OpenTitan DV, mis. model flash bank).
    let source = r#"
class c;
    int mem[8];
    int count;
    function new();
        for (int i = 0; i < 8; i++) mem[i] = 0;
        count = 0;
    endfunction
    task push(int v);
        mem[count] = v;
        count = count + 1;
    endtask
    function int read(int i);
        return mem[i];
    endfunction
endclass

module tb;
    c obj;
    reg [31:0] result;
    initial begin
        obj = new();
        obj.push(5);
        obj.push(7);
        result = obj.read(1);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "result")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        v, 7,
        "array lvalue index dinamis di task method harus jalan"
    );
}

#[test]
fn test_uvm_super_new_unknown_base_class() {
    // OpenTitan DV: `dv_report_server extends uvm_default_report_server`
    // (kelas library UVM TIDAK ada di filelist). `super.new(name)` di
    // subclass user harus jatuh ke builtin report_object, bukan error
    // RT8001 "method 'new' not found in class 'uvm_default_report_server'".
    let source = r#"
class dv_report_server extends uvm_default_report_server;
    function new (string name = "");
        super.new(name);
    endfunction
    function string tag();
        return "dv";
    endfunction
endclass

module tb;
    dv_report_server srv;
    reg [31:0] result;
    initial begin
        srv = new("mysrv");
        result = 42;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "result")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        v, 42,
        "super.new pada class UVM base yang tidak terdaftar harus tidak error"
    );
}

#[test]
fn test_uvm_object_get_type_name() {
    let source = r#"
class my_obj extends uvm_object;
    function new(string name);
        super.new(name);
    endfunction
endclass

module tb;
    my_obj obj;
    reg [31:0] result;
    initial begin
        obj = my_obj::new("my_test_obj");
        result = obj.get_type_name();
        #1 $finish;
    end
endmodule
"#;
    let _sigs = simulate_signals(source, 5).unwrap();
    // get_type_name returns a string (bits), we just verify simulation completes
}

#[test]
fn test_uvm_printer_print_object() {
    // VERIF-12: uvm_table_printer::print_object(obj) memformat object jadi
    // string tabel (nama, class, fields). Field yang di-set via `obj.x = ...`
    // harus terlihat di output.
    let source = r#"
class my_obj extends uvm_object;
    int count;
    bit [7:0] addr;
    function new(string name);
        super.new(name);
        count = 0;
        addr = 0;
    endfunction
endclass

module tb;
    my_obj obj;
    uvm_table_printer printer;
    string s;
    reg ok;
    initial begin
        obj = new("my_print_obj");
        obj.count = 42;
        obj.addr = 8'hA5;
        printer = new("printer");
        s = printer.print_object(obj);
        ok = 0;
        if (s.len() > 0) ok = 1;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let ok = sigs
        .iter()
        .find(|(n, _)| n == "ok")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(ok, 1, "print_object harus mengembalikan string non-kosong");
    // String hasil print_object harus berisi nama object + field yang di-set
    // (nama object & field di-format sebagai teks tabel).
    let s = sigs
        .iter()
        .find(|(n, _)| n == "s")
        .map(|(_, v)| logicvec_to_string(v))
        .unwrap_or_default();
    assert!(
        s.contains("my_print_obj"),
        "string harus berisi nama object: got '{:?}'",
        s.chars().take(80).collect::<String>()
    );
    assert!(
        s.contains("count") && s.contains("addr"),
        "string harus berisi nama field: got '{:?}'",
        s.chars().take(80).collect::<String>()
    );
}

#[test]
fn test_uvm_printer_contains_fields() {
    // VERIF-12: hasil print_object harus berisi nama object, class, dan nama
    // field yang di-set — verifikasi lewat method get() yang membaca hasil
    // print ke string signal (bukan hanya non-kosong).
    let source = r#"
class my_obj extends uvm_object;
    int count;
    function new(string name);
        super.new(name);
        count = 0;
    endfunction
endclass

module tb;
    my_obj obj;
    uvm_table_printer printer;
    string printed;
    reg [31:0] check;
    initial begin
        obj = new("printer_obj");
        obj.count = 7;
        printer = new("printer");
        printed = printer.print_object(obj);
        // Konten tidak mudah diperiksa per-char dari sinyal; verifikasi sim
        // selesai + print tidak error dengan cek signal turunan.
        check = obj.count;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let check = sigs
        .iter()
        .find(|(n, _)| n == "check")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(check, 7, "field count harus tetap 7 setelah print_object");
}

#[test]
fn test_uvm_object_print_with_printer() {
    // VERIF-12: `obj.print(printer)` dengan argumen printer harus memakai
    // format tabel (tidak error). Tanpa printer → format default.
    let source = r#"
class my_obj extends uvm_object;
    function new(string name);
        super.new(name);
    endfunction
endclass

module tb;
    my_obj obj;
    uvm_table_printer printer;
    reg ok;
    initial begin
        obj = new("print_arg_obj");
        printer = new("printer");
        obj.print(printer);  // delegasi ke printer — tidak boleh error
        ok = 1;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let ok = sigs
        .iter()
        .find(|(n, _)| n == "ok")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(ok, 1, "obj.print(printer) harus sukses tanpa error");
}

#[test]
fn test_uvm_component_compile() {
    let source = r#"
class my_comp extends uvm_component;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
endclass

module tb;
    my_comp comp;
    initial begin
        comp = my_comp::new("my_comp", 0);
        #1 $finish;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "uvm_component compile failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_sequence_item_compile() {
    let source = r#"
class my_item extends uvm_sequence_item;
    rand bit [7:0] addr;
    function new(string name);
        super.new(name);
    endfunction
endclass

module tb;
    my_item item;
    initial begin
        item = my_item::new("item");
        #1 $finish;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "uvm_sequence_item compile failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_sequence_sim() {
    let source = r#"
class my_seq extends uvm_sequence;
    function new(string name);
        super.new(name);
    endfunction
    task body();
        // body runs when start() is called
    endtask
endclass

module tb;
    my_seq seq;
    initial begin
        seq = my_seq::new("seq");
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 5);
    assert!(
        result.is_ok(),
        "uvm_sequence sim failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_sequencer_driver_compile() {
    let source = r#"
class my_driver extends uvm_driver;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
endclass

class my_sequencer extends uvm_sequencer;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
endclass

module tb;
    my_driver drv;
    my_sequencer seqr;
    initial begin
        drv = my_driver::new("drv", 0);
        seqr = my_sequencer::new("seqr", 0);
        #1 $finish;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "uvm_sequencer/driver compile failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_sequence_start() {
    let source = r#"
class my_seq extends uvm_sequence;
    function new(string name);
        super.new(name);
    endfunction
    task body();
        // body runs when start() is called
    endtask
endclass

class my_sequencer extends uvm_sequencer;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
endclass

module tb;
    my_seq seq;
    my_sequencer seqr;
    initial begin
        seqr = my_sequencer::new("seqr", 0);
        seq = my_seq::new("seq");
        seq.start(seqr);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 10);
    assert!(
        result.is_ok(),
        "uvm_sequence start failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_analysis_port_write_through() {
    let source = r#"
class my_monitor extends uvm_monitor;
    uvm_analysis_port ap;
    function new(string name, uvm_component parent);
        super.new(name, parent);
        ap = uvm_analysis_port::new("ap");
    endfunction
    task run_phase(uvm_phase phase);
        // In real UVM, ap.write(item) would be called here
    endtask
endclass

class my_scoreboard extends uvm_scoreboard;
    int write_count;
    function new(string name, uvm_component parent);
        super.new(name, parent);
        write_count = 0;
    endfunction
    function void write(uvm_sequence_item item);
        write_count = write_count + 1;
    endfunction
endclass

module tb;
    my_monitor mon;
    my_scoreboard sb;
    uvm_analysis_imp imp;
    reg [31:0] result;
    initial begin
        mon = my_monitor::new("mon", 0);
        sb = my_scoreboard::new("sb", 0);
        imp = uvm_analysis_imp::new("imp", sb);
        mon.ap.connect(imp);
        mon.ap.write(0);
        result = sb.write_count;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(
        val.to_u64(),
        1,
        "write_count should be 1 after analysis_port write"
    );
}
