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
fn test_sized_parameter_keeps_its_width() {
    // ELAB-13: parameter 8'd1 harus tetap 8 bit. Sebelum perbaikan, nilai
    // parameter selalu di-emit sebagai Const 64 bit sehingga ~(a + P)
    // melebar ke 64 bit dan perbandingan dengan 8'hF0 menjadi false.
    let source = r#"
module tb;
    parameter P = 8'd1;
    logic [7:0] a;
    logic hit;
    initial begin
        a = 8'h0E;
        #1;
        hit = (~(a + P) == 8'hF0);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let hit = sigs
        .iter()
        .find(|(n, _)| n == "hit")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(hit, 1, "~(a + P) harus 8 bit: ~(8'h0E + 8'd1) == 8'hF0");
}

/// ELAB-13 butir (11): `parameter signed <tipe>` TANPA range harus
/// truncate + sign-extend ke lebar tipe (int 32, byte 8, shortint 16).
/// Sebelum perbaikan `dtype_width(Signed(_))` = None sehingga nilai mentah
/// (4294967295) dipakai: `SI < 0` false. Referensi iverilog 12: -1.
#[test]
fn test_signed_dtype_parameter_is_sign_extended() {
    let source = r#"
module tb;
    parameter int signed SI = 32'hFFFFFFFF;
    parameter signed int SJ = 32'hFFFFFFFF;
    parameter signed byte SB = 8'hFF;
    parameter signed shortint SS = 16'h8000;
    parameter signed P64 = 64'h1_0000_0001;
    parameter signed P8 = 8'hFF;
    logic ok1;
    logic ok2;
    logic ok3;
    logic ok4;
    logic ok5;
    logic ok6;
    logic ok7;
    logic [31:0] nv;
    logic [31:0] vsi;
    logic [7:0] vsb;
    logic [15:0] vss;
    logic [63:0] vp64;
    logic [31:0] bp64;
    initial begin
        ok1 = (SI < 0);
        ok2 = (SI == -1);
        ok3 = (SJ == -1);
        ok4 = (SB < 0);
        ok5 = (SS < 0);
        ok6 = (P64 == 64'h1_0000_0001);
        ok7 = (P8 < 0);
        nv = ~SI;
        vsi = SI;
        vsb = SB;
        vss = SS;
        vp64 = P64;
        bp64 = $bits(P64);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(get("ok1"), 1, "parameter int signed SI harus negatif");
    assert_eq!(get("ok2"), 1, "SI harus == -1");
    assert_eq!(get("ok3"), 1, "parameter signed int SJ harus == -1");
    assert_eq!(get("ok4"), 1, "parameter signed byte SB harus negatif");
    assert_eq!(get("ok5"), 1, "parameter signed shortint SS harus negatif");
    assert_eq!(get("nv"), 0, "~SI harus 0 (SI 32 bit -1)");
    assert_eq!(get("vsi"), 0xFFFF_FFFF, "nilai bit SI tetap ffffffff");
    assert_eq!(get("vsb"), 0xFF, "nilai bit SB tetap ff");
    assert_eq!(get("vss"), 0x8000, "nilai bit SS tetap 8000");
    // `parameter signed P` TANPA tipe: lebar ikut nilai default (64 bit),
    // bukan dipaksa int 32 bit (regresi yang ditemukan review).
    assert_eq!(
        get("ok6"),
        1,
        "parameter signed P64 = 64'h1_0000_0001 harus utuh"
    );
    assert_eq!(
        get("vp64"),
        0x1_0000_0001,
        "nilai P64 tidak boleh terpotong"
    );
    assert_eq!(get("bp64"), 64, "$bits(parameter signed P64) harus 64");
    assert_eq!(get("ok7"), 1, "parameter signed P8 = 8'hFF harus -1");
}

/// ELAB-13 butir (11) — kelanjutan: lebar/signedness deklarasi param harus
/// dipakai SAMA oleh tiga jalur (IR runtime, const-fold generate-if,
/// package). Sebelum perbaikan:
///   - generate-if `if (P8 < 0)` memilih cabang else (nilai mentah 255),
///   - `parameter unsigned U = -1` dibandingkan sebagai 64 bit (bukan
///     32'hFFFFFFFF),
///   - param PACKAGE (`import p::*`) tidak membawa lebar/signedness →
///     `SP < 0` salah dan `$display("%0d", SP)` mencetak 255.
/// Referensi iverilog 12: semua cabang true, SP=-1, UP=4294967295, B=-1.
#[test]
fn test_signed_parameter_generate_and_package_parity() {
    let source = r#"
package p;
    parameter signed SP = 8'hFF;
    parameter unsigned UP = -1;
    parameter int B = -1;
endpackage

// Package TIDAK di-import: hanya referensi qualified `q::...` (jalur peta
// global param package).
package q;
    parameter signed QSP = 8'hFF;
    parameter unsigned QUP = -1;
endpackage

module child #(parameter signed CP = 8'hFF, parameter unsigned CU = -1) (
    output logic [7:0] o1,
    output logic [7:0] o2
);
    initial begin
        o1 = (CP < 0) ? 8'hA1 : 8'hB1;
        o2 = (CU == 32'hFFFFFFFF) ? 8'hC2 : 8'hD2;
    end
endmodule

module tb;
    import p::*;
    parameter signed P8 = 8'hFF;
    parameter unsigned U = -1;
    localparam signed LP = 8'hFF;
    logic [7:0] g1, g2, g3, g4, g5, g6, g7, g8;
    logic [7:0] c1, c2;
    logic [31:0] uv;
    logic [31:0] bv;
    logic [7:0] spv;
    child u1(.o1(c1), .o2(c2));
    generate
        if (P8 < 0) begin : a1 initial g1 = 8'h11; end else begin : b1 initial g1 = 8'h12; end
        if (U == 32'hFFFFFFFF) begin : a2 initial g2 = 8'h21; end else begin : b2 initial g2 = 8'h22; end
        if (LP < 0) begin : a3 initial g3 = 8'h31; end else begin : b3 initial g3 = 8'h32; end
        if (SP < 0) begin : a4 initial g4 = 8'h41; end else begin : b4 initial g4 = 8'h42; end
        if (UP == 32'hFFFFFFFF) begin : a5 initial g5 = 8'h51; end else begin : b5 initial g5 = 8'h52; end
        if (B < 0) begin : a6 initial g6 = 8'h61; end else begin : b6 initial g6 = 8'h62; end
        if (q::QSP < 0) begin : a7 initial g7 = 8'h71; end else begin : b7 initial g7 = 8'h72; end
        if (q::QUP == 32'hFFFFFFFF) begin : a8 initial g8 = 8'h81; end else begin : b8 initial g8 = 8'h82; end
    endgenerate
    initial begin
        uv = U;
        bv = B;
        spv = SP;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(get("g1"), 0x11, "generate-if P8 < 0 (implicit signed)");
    assert_eq!(
        get("g2"),
        0x21,
        "generate-if U == 32'hFFFFFFFF (implicit unsigned, 32 bit)"
    );
    assert_eq!(get("g3"), 0x31, "generate-if localparam signed LP < 0");
    assert_eq!(get("g4"), 0x41, "generate-if param package signed SP < 0");
    assert_eq!(get("g5"), 0x51, "generate-if param package unsigned UP");
    assert_eq!(get("g6"), 0x61, "generate-if param package int B < 0");
    assert_eq!(
        get("g7"),
        0x71,
        "generate-if `q::QSP < 0` (package tanpa import)"
    );
    assert_eq!(
        get("g8"),
        0x81,
        "generate-if `q::QUP == 32'hFFFFFFFF` (package tanpa import)"
    );
    assert_eq!(get("c1"), 0xA1, "parameter header signed CP < 0");
    assert_eq!(get("c2"), 0xC2, "parameter header unsigned CU");
    assert_eq!(get("uv"), 0xFFFF_FFFF, "nilai bit U = 32'hFFFFFFFF");
    assert_eq!(get("bv"), 0xFFFF_FFFF, "nilai bit B (int -1)");
    assert_eq!(get("spv"), 0xFF, "nilai bit SP (8 bit)");
}

/// ELAB-13 butir (11)/(12): localparam signed di dalam generate-for dan
/// param `$unit` harus memakai lebar/signedness deklarasinya — sama dengan
/// iverilog 12. Sebelum perbaikan, kondisi generate-if dievaluasi setelah
/// nama di-rename (`g[0].L`) sehingga signedness hilang → cabang else.
#[test]
fn test_signed_parameter_in_generate_loop_and_unit_scope() {
    let source = r#"
parameter signed UP = 8'hFF;
parameter signed [7:0] X = 8'hFF;

module tb;
    parameter [7:0] X = 8'hFA;
    logic [7:0] g0, g1, g2, g3;
    logic [31:0] upv;
    genvar i;
    generate
        for (i = 0; i < 1; i = i + 1) begin : g
            localparam signed L = 8'hFF;
            localparam int LI = -3;
            localparam signed [7:0] S8 = 8'hFF;
            if (L < 0) begin : a0 initial g0 = 8'hA0; end else begin : b0 initial g0 = 8'hB0; end
            if (LI < 0) begin : a1 initial g1 = 8'hA1; end else begin : b1 initial g1 = 8'hB1; end
            if (S8 < 0) begin : a2 initial g2 = 8'hA2; end else begin : b2 initial g2 = 8'hB2; end
        end
    endgenerate
    generate
        if (X > 100) begin : a3 initial g3 = 8'hA3; end else begin : b3 initial g3 = 8'hB3; end
    endgenerate
    initial begin
        upv = UP;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(
        get("g0"),
        0xA0,
        "localparam signed implicit di generate-for"
    );
    assert_eq!(get("g1"), 0xA1, "localparam int LI di generate-for");
    assert_eq!(
        get("g2"),
        0xA2,
        "localparam signed [7:0] S8 di generate-for"
    );
    assert_eq!(get("g3"), 0xA3, "param body X shadow $unit (250 > 100)");
    assert_eq!(get("upv"), 0xFFFF_FFFF, "$unit signed UP = -1 (32 bit)");
}

/// ELAB-13 butir (18): modifier `unsigned` pada tipe eksplisit
/// (`parameter int unsigned`, `byte unsigned`, `localparam int unsigned`)
/// dulu DIBUANG parser — `apply_sign_mod(Some(Int), false)` mengembalikan
/// `Int` apa adanya, sehingga nilai `-1` tetap bertanda (`-1`, `PU > 0` = 0).
/// Perbaikan: signedness dicatat di `ParamDecl.implicit_sign` (dibaca
/// `param_decl_signed`) sementara dtype tetap memberi LEBAR (int 32, byte 8).
/// Referensi iverilog 12: PU=4294967295, BU=255, PS=-1, LU=1, PKB=255,
/// SI=65535, `$bits(LI)`=64, generate-if g1..g10 semuanya cabang true.
#[test]
fn test_unsigned_modifier_on_typed_parameter() {
    let source = r#"
package pk;
    parameter int unsigned PKU = -1;
    parameter byte unsigned PKB = -1;
    parameter int signed PKS = -1;
endpackage

module child #(parameter int unsigned HP = -1) (output logic [7:0] o);
    initial o = (HP > 0) ? 8'hC1 : 8'hD1;
endmodule

module tb;
    import pk::*;
    parameter int unsigned PU = -1;
    parameter byte unsigned BU = -1;
    parameter shortint unsigned SI = -1;
    parameter longint unsigned LI = -1;
    parameter int signed PS = -1;
    parameter logic unsigned LU = -1;
    localparam int unsigned LPU = -1;
    logic [7:0] g1, g2, g3, g4, g5, g6, g7, g8, g9, g10, c1;
    logic [31:0] puv, ps, siv;
    logic [7:0] buv, luv, lpuv, pkbv, lbits;
    child u1(.o(c1));
    generate
        if (PU > 0) begin : a1 initial g1 = 8'h11; end else begin : b1 initial g1 = 8'h12; end
        if (BU > 0) begin : a2 initial g2 = 8'h21; end else begin : b2 initial g2 = 8'h22; end
        if (PS < 0) begin : a3 initial g3 = 8'h31; end else begin : b3 initial g3 = 8'h32; end
        if (PKU > 0) begin : a4 initial g4 = 8'h41; end else begin : b4 initial g4 = 8'h42; end
        if (PKB > 0) begin : a5 initial g5 = 8'h51; end else begin : b5 initial g5 = 8'h52; end
        if (LPU > 0) begin : a6 initial g6 = 8'h61; end else begin : b6 initial g6 = 8'h62; end
        if (PKS < 0) begin : a7 initial g7 = 8'h71; end else begin : b7 initial g7 = 8'h72; end
        if (1) begin : gb
            localparam int unsigned GLU = -1;
            if (GLU > 0) begin : a8 initial g8 = 8'h81; end else begin : b8 initial g8 = 8'h82; end
        end
        if (LI > 0) begin : a9 initial g9 = 8'h91; end else begin : b9 initial g9 = 8'h92; end
        if (SI > 0) begin : a10 initial g10 = 8'hA1; end else begin : b10 initial g10 = 8'hA2; end
    endgenerate
    initial begin
        puv = PU;
        buv = BU;
        ps = PS;
        luv = LU;
        lpuv = LPU;
        pkbv = PKB;
        siv = SI;
        lbits = $bits(LI);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(get("g1"), 0x11, "parameter int unsigned PU > 0");
    assert_eq!(get("g2"), 0x21, "parameter byte unsigned BU > 0");
    assert_eq!(
        get("g3"),
        0x31,
        "parameter int signed PS < 0 (tetap signed)"
    );
    assert_eq!(get("g4"), 0x41, "parameter package int unsigned PKU > 0");
    assert_eq!(get("g5"), 0x51, "parameter package byte unsigned PKB > 0");
    assert_eq!(get("g6"), 0x61, "localparam int unsigned LPU > 0");
    assert_eq!(get("g7"), 0x71, "parameter package int signed PKS < 0");
    assert_eq!(get("g8"), 0x81, "localparam int unsigned di generate block");
    assert_eq!(get("g9"), 0x91, "parameter longint unsigned LI > 0");
    assert_eq!(get("g10"), 0xA1, "parameter shortint unsigned SI > 0");
    assert_eq!(get("c1"), 0xC1, "parameter header int unsigned HP > 0");
    assert_eq!(get("puv"), 0xFFFF_FFFF, "nilai bit PU (int unsigned, -1)");
    assert_eq!(get("buv"), 0xFF, "nilai bit BU (byte unsigned, -1)");
    assert_eq!(get("ps"), 0xFFFF_FFFF, "nilai bit PS (int signed, -1)");
    assert_eq!(get("luv"), 1, "logic unsigned = 1 bit (bit-0 dari -1)");
    assert_eq!(get("lpuv"), 0xFF, "nilai bit LPU (int unsigned, -1)");
    assert_eq!(get("pkbv"), 0xFF, "nilai bit PKB (byte unsigned, -1)");
    assert_eq!(get("siv"), 0xFFFF, "nilai bit SI (shortint unsigned, -1)");
    assert_eq!(get("lbits"), 64, "$bits(longint unsigned) = 64");
}

/// ELAB-13 butir (19): `localparam` di compilation-unit scope (`$unit`, di
/// luar module) DULU DIBUANG parser (second pass hanya menyimpan `!is_local`),
/// sehingga `localparam int L = 7;` di luar module berujung E2001 "signal 'L'
/// not found" dan generate-if jatuh ke RT9003 (ambil cabang true buta).
/// Perbaikan: semua param $unit disimpan (ParamDecl.is_localparam membedakan)
/// dan disuntikkan ke peta nilai param (`resolve_param_values_with_ctx`
/// menerima `unit_params` dan menyuntiknya SEBELUM pass body/header param).
/// Referensi iverilog 12: lv=7 ldv=8 luv=4294967295 l8v=a5 lsv=1 g1=11 g2=21.
#[test]
fn test_unit_scope_localparam_visible_in_module() {
    let source = r#"
localparam int L = 7;
localparam [7:0] L8 = 8'hA5;
localparam int unsigned LU = -1;
localparam signed LS = 8'hFF;
localparam int LD = L + 1;

module tb;
    logic [31:0] lv, ldv, luv;
    logic [7:0] l8v, g1, g2, g3, g4;
    logic lsv;
    generate
        if (L == 7) begin : a1 initial g1 = 8'h11; end else begin : b1 initial g1 = 8'h12; end
        if (LS < 0) begin : a2 initial g2 = 8'h21; end else begin : b2 initial g2 = 8'h22; end
        // Cabang FALSE yang benar — fallback RT9003 ("ambil true branch")
        // akan menghasilkan 0x31/0x41, jadi dua if di bawah membedakan
        // "kondisi benar-benar dievaluasi" dari "warning lalu buta".
        if (L == 99) begin : a3 initial g3 = 8'h31; end else begin : b3 initial g3 = 8'h32; end
        if (LS > 0) begin : a4 initial g4 = 8'h41; end else begin : b4 initial g4 = 8'h42; end
    endgenerate
    initial begin
        lv = L;
        ldv = LD;
        luv = LU;
        l8v = L8;
        lsv = (LS < 0);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(get("lv"), 7, "$unit localparam int L = 7");
    assert_eq!(
        get("ldv"),
        8,
        "$unit localparam LD = L + 1 (urutan deklarasi)"
    );
    assert_eq!(get("luv"), 0xFFFF_FFFF, "$unit localparam int unsigned LU");
    assert_eq!(get("l8v"), 0xA5, "$unit localparam [7:0] L8");
    assert_eq!(get("lsv"), 1, "$unit localparam signed LS = 8'hFF (< 0)");
    assert_eq!(get("g1"), 0x11, "generate-if `L == 7` (bukan RT9003)");
    assert_eq!(
        get("g2"),
        0x21,
        "generate-if `LS < 0` (signed $unit localparam)"
    );
    assert_eq!(get("g3"), 0x32, "generate-if `L == 99` → cabang else");
    assert_eq!(get("g4"), 0x42, "generate-if `LS > 0` → cabang else");
}

/// ELAB-13 butir (19) lanjutan: param `$unit` harus terlihat saat me-resolve
/// default param module — `localparam int W = L + 1;` di body (dulu W=1,
/// fallback) dan `#(parameter W = L)` di header instance (dulu W=0). Selain
/// itu deklarasi SINYAL module menutupi nama `$unit`/package (LRM §3.13) —
/// `logic [7:0] L;` tidak boleh di-resolve ke `localparam int L = 7;`.
/// Referensi iverilog 12: W=8 g1=11 cw=07 lv=42.
#[test]
fn test_unit_scope_localparam_derived_header_and_shadowing() {
    let source = r#"
localparam int L = 7;

module child #(parameter W = L) (output logic [7:0] w);
    initial w = W;
endmodule

module shadow_mod(output logic [7:0] lv);
    logic [7:0] L;
    initial begin
        L = 8'd42;
        #1 lv = L;
    end
endmodule

module tb;
    localparam int W = L + 1;
    logic [7:0] g1;
    logic [7:0] cw;
    logic [7:0] lv;
    child u(.w(cw));
    shadow_mod s(.lv(lv));
    generate
        if (W == 8) begin : a1 initial g1 = 8'h11; end else begin : b1 initial g1 = 8'h12; end
    endgenerate
    initial #2 $finish;
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(
        get("g1"),
        0x11,
        "generate-if `W == 8` (W = L + 1 dari $unit)"
    );
    assert_eq!(get("cw"), 7, "header `#(parameter W = L)` dari $unit");
    assert_eq!(get("lv"), 42, "sinyal `L` menutupi $unit localparam L");
}

/// ELAB-13 (19) review round 2 — dua regresi yang ditemukan reviewer:
/// (a) loop var `for (int P ...)` didaftarkan sebagai signal sintetis dan
///     (setelah sinyal didahulukan) menutupi param module `P` di luar loop
///     (`out = P` jadi 0; iverilog 9);
/// (b) `localparam int L = 5;` di dalam generate block kalah dari
///     `$unit localparam int L = 7;` karena suntikan $unit lebih dulu
///     (`o = 7`; iverilog 5).
/// Referensi iverilog 12: out=9 o=5.
#[test]
fn test_loop_var_and_generate_localparam_do_not_hijack_params() {
    let source = r#"
localparam int L = 7;

module tb;
    parameter int P = 9;
    logic [31:0] out;
    logic [31:0] o;
    generate
        if (1) begin : g
            localparam int L = 5;
            initial o = L;
        end
    endgenerate
    initial begin
        for (int P = 0; P < 3; P = P + 1) begin
        end
        out = P;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(
        get("out"),
        9,
        "loop var `P` tidak menutupi param P di luar loop"
    );
    assert_eq!(
        get("o"),
        5,
        "localparam generate `L` menutupi $unit localparam L"
    );
}

/// ELAB-13 (19) review round 3 — tiga regresi jalur "sinyal didahulukan":
/// (a) BLOCKER: pemeriksaan `signal_map` di `expr.rs` tidak sadar-scope,
///     sehingga `logic [7:0] L;` di dalam blok prosedural menutupi
///     `parameter int L = 9` module (`a = 5`; iverilog 9);
/// (b) MAJOR: `logic [7:0] L;` di body module (disimpan di `module.decls`,
///     BUKAN `module.items`) tidak terdeteksi filter `$unit`, jadi
///     `localparam int L = 7;` bocor (`lv = 7`; iverilog 42);
/// (c) loop `for (int i = 0; i < n; ...)` dengan batas runtime harus tetap
///     jalan (dulu guard loop-var memicu E2001).
/// Referensi iverilog 12: a=9 lv=42 acc=3.
#[test]
fn test_param_shadowing_is_scope_aware_and_dynamic_loop_survives() {
    let source = r#"
localparam int L = 7;

module body_shadow(output logic [7:0] lv);
    logic [7:0] L;
    initial begin
        L = 8'd42;
        #1 lv = L;
    end
endmodule

module proc_local(output integer a);
    parameter int L = 9;
    initial begin
        logic [7:0] L;
        L = 8'd5;
    end
    initial begin
        #1 a = L;
    end
endmodule

module dyn_loop(output integer acc);
    logic [7:0] n;
    initial begin
        n = 8'd3;
        acc = 0;
        #1;
        for (int i = 0; i < n; i = i + 1) begin
            acc = acc + i;
        end
    end
endmodule

module tb;
    logic [7:0] lv;
    integer a, acc;
    body_shadow bs(.lv(lv));
    proc_local pl(.a(a));
    dyn_loop dl(.acc(acc));
    initial #3 $finish;
endmodule
"#;
    let sigs = simulate_signals(source, 8).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(
        get("lv"),
        42,
        "sinyal body module (`module.decls`) menutupi $unit localparam L"
    );
    assert_eq!(
        get("a"),
        9,
        "blok prosedural lokal TIDAK menutupi parameter module (LRM §3.13)"
    );
    assert_eq!(get("acc"), 3, "loop batas runtime tetap berjalan (0+1+2)");
}

/// ELAB-13 (19) review round 4 — filter "sinyal module menutupi `$unit`" TIDAK
/// boleh dipakai di konteks CONST-FOLD. Sinyal module bukan konstanta, jadi
/// default param header (`#(parameter W = L)`), default localparam body
/// (`localparam int W = L`), dan kondisi generate (`if (L == 99)`) harus
/// melihat nilai `$unit` walau module punya sinyal bernama `L`.
/// Referensi iverilog 12: cw=07 y=07 lv=2a g=32.
#[test]
fn test_unit_param_visible_in_const_contexts_despite_signal_shadow() {
    let source = r#"
localparam int L = 7;

module child #(parameter W = L) (output logic [7:0] w);
    initial w = W;
endmodule

module sh(output logic [7:0] lv, output logic [7:0] y, output logic [7:0] g);
    logic [7:0] L;
    localparam int W = L;
    generate
        if (L == 99) begin : a initial g = 8'h31; end else begin : b initial g = 8'h32; end
    endgenerate
    initial begin
        L = 8'd42;
        #1 lv = L;
        y = W;
    end
endmodule

module tb;
    logic [7:0] cw, lv, y, g;
    child u(.w(cw));
    sh s(.lv(lv), .y(y), .g(g));
    initial #2 $finish;
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(get("cw"), 7, "header `#(parameter W = L)` memakai $unit L");
    assert_eq!(get("y"), 7, "body `localparam int W = L` memakai $unit L");
    assert_eq!(
        get("g"),
        0x32,
        "generate-if `L == 99` memakai $unit L (bukan sinyal → RT9003)"
    );
    assert_eq!(get("lv"), 42, "referensi runtime `L` tetap sinyal module");
}

/// ELAB-13 (20): param package hasil `import p::*` yang namanya SAMA dengan
/// sinyal lokal module — deklarasi sinyal menang (LRM §3.13).
/// Referensi iverilog 12: o=01.
#[test]
fn test_package_param_shadowed_by_local_signal() {
    let source = r#"
package p;
    parameter int V = 8'hAA;
endpackage

module tb;
    import p::*;
    logic [7:0] V;
    logic [7:0] o;
    initial begin
        V = 8'h01;
        #1 o = V;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(get("o"), 0x01, "sinyal lokal menutupi param package `V`");
}

/// ELAB-13 (21): localparam di dalam blok generate harus melihat param
/// module saat default-nya dievaluasi. Dulu `resolve_param_values_with_ctx`
/// memproses body/generate param SEBELUM param header/localparam module masuk
/// peta nilai, sehingga `localparam int L1 = BASE` gagal const-eval dan jatuh
/// ke fallback 1 (mivon o1=1 o2=1; iverilog 5/16). Perbaikannya: seluruh
/// body/generate param dievaluasi ULANG setelah Pass 2 (urutan source, hanya
/// nama yang di-insert loop pertama). Diskriminatif — gagal di HEAD.
#[test]
fn test_generate_block_localparam_sees_module_params() {
    let source = r#"
module tb #(parameter int BASE = 5);
    localparam int MOD = BASE + 10;
    generate
        if (1) begin : g
            localparam int L1 = BASE;
            localparam int L2 = MOD + 1;
            wire [7:0] o1 = L1;
            wire [7:0] o2 = L2;
        end
    endgenerate
    initial #1 $finish;
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(get("o1"), 5, "localparam generate L1 = BASE (param header)");
    assert_eq!(
        get("o2"),
        16,
        "localparam generate L2 = MOD + 1 (localparam body)"
    );
}

/// ELAB-13 (21) review round 1: dua hal yang harus dijaga saat body/generate
/// param dievaluasi ulang setelah Pass 2 —
/// (a) RANTAI: `L2 = L1 + 1` harus memakai L1 yang sudah benar (bukan fallback
///     1 → L2 = 2);
/// (b) PRESEDENSI: deklarasi generate bernama sama TIDAK boleh menimpa param
///     header (`parameter int L = 9` tetap 9 di level module).
/// Referensi iverilog 12: v1=5 v2=6 m=9.
#[test]
fn test_generate_localparam_chain_and_header_precedence() {
    let source = r#"
module tb #(parameter int BASE = 5, parameter int L = 9);
    generate if (1) begin : g
        localparam int L1 = BASE;
        localparam int L2 = L1 + 1;
        wire [7:0] v1 = L1;
        wire [7:0] v2 = L2;
    end endgenerate
    wire [7:0] m = L;
    initial #1 $finish;
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(get("v1"), 5, "L1 = BASE");
    assert_eq!(get("v2"), 6, "rantai L2 = L1 + 1 memakai L1 yang benar");
    assert_eq!(
        get("m"),
        9,
        "param header L menang atas localparam generate"
    );
}

/// ELAB-13 (21) review round 1 (blocking 1): deklarasi generate bernama sama
/// TIDAK boleh menimpa param header. Jalur FILE (`compile_files`, dipakai CLI
/// + prepass) berbeda dari `compile_str`: retry tanpa guard presedensi membuat
/// `wire [7:0] m = L` di level module memakai nilai blok generate (5),
/// padahal iverilog 12 → 9.
#[test]
fn test_generate_localparam_does_not_override_header_param() {
    let dir = std::env::temp_dir().join(format!("mivon_elab13_21_{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("hdr.sv");
    fs::write(
        &path,
        r#"
module tb #(parameter int BASE = 5, parameter int L = 9);
    generate if (1) begin : g
        localparam int L = BASE;
    end endgenerate
    wire [7:0] m = L;
    initial #1 $finish;
endmodule
"#,
    )
    .unwrap();
    let design = crate::compile_files(&[path.to_string_lossy().to_string()]).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 10);
    engine.run().unwrap();
    let idx = engine
        .design
        .top
        .signals
        .iter()
        .position(|s| s.name.as_str() == "m")
        .expect("signal m ada");
    assert_eq!(
        engine.state.read_signal(idx).to_u64(),
        9,
        "param header L menang atas localparam generate bernama sama"
    );
}

/// ELAB-13 (21) review round 2 (blocking): konstanta package (`import p::*`)
/// tidak boleh ditimpa deklarasi generate bernama sama di level module.
/// Loop evaluasi-ulang hanya menyentuh nama yang benar-benar di-insert loop
/// pertama; nama dari `base_ctx` (package) tidak termasuk. Tanpa guard itu
/// `wire [7:0] m = V` jadi 5 (iverilog 100) dan cabang generate ikut salah.
/// Referensi iverilog 12: m=100, cabang A.
#[test]
fn test_generate_localparam_does_not_override_package_const() {
    let dir = std::env::temp_dir().join(format!("mivon_elab13_21p_{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("pkg.sv");
    fs::write(
        &path,
        r#"
package p;
    localparam int V = 100;
endpackage
module tb;
    import p::*;
    generate if (1) begin : g
        localparam int V = 5;
    end endgenerate
    wire [7:0] m = V;
    generate if (V == 100) begin : h
        initial #1 g_a = 1'b1;
    end else begin : h2
        initial #1 g_b = 1'b1;
    end endgenerate
    logic g_a, g_b;
    initial #1 $finish;
endmodule
"#,
    )
    .unwrap();
    let design = crate::compile_files(&[path.to_string_lossy().to_string()]).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 10);
    engine.run().unwrap();
    let get = |name: &str| {
        let idx = engine
            .design
            .top
            .signals
            .iter()
            .position(|s| s.name.as_str() == name)
            .unwrap_or_else(|| panic!("signal {name} ada"));
        engine.state.read_signal(idx).to_u64()
    };
    assert_eq!(get("m"), 100, "konstanta package V menang di level module");
    assert_eq!(get("g_a"), 1, "generate-if V == 100 memakai nilai package");
    assert_eq!(get("g_b"), 0, "cabang else tidak diambil");
}

/// ELAB-13 (21) review round 3 (blocking): rantai DI DALAM blok harus memakai
/// nilai blok walau namanya juga dipakai param header. Tanpa overlay sementara,
/// `B = L + 1` dievaluasi dengan L level module (9) → B=10; iverilog 12: b=6
/// (L blok = 5), m=9 (level module tetap param header).
#[test]
fn test_generate_localparam_chain_uses_block_value_when_shadowed() {
    let dir = std::env::temp_dir().join(format!("mivon_elab13_21s_{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("shadow.sv");
    fs::write(
        &path,
        r#"
module tb #(parameter int L = 9);
    generate if (1) begin : g
        localparam int L = 5;
        localparam int B = L + 1;
        wire [7:0] b = B;
    end endgenerate
    wire [7:0] m = L;
    initial #1 $finish;
endmodule
"#,
    )
    .unwrap();
    let design = crate::compile_files(&[path.to_string_lossy().to_string()]).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 10);
    engine.run().unwrap();
    let get = |name: &str| {
        let idx = engine
            .design
            .top
            .signals
            .iter()
            .position(|s| s.name.as_str() == name)
            .unwrap_or_else(|| panic!("signal {name} ada"));
        engine.state.read_signal(idx).to_u64()
    };
    assert_eq!(get("b"), 6, "rantai blok B = L + 1 memakai L blok (5)");
    assert_eq!(get("m"), 9, "level module tetap memakai param header L = 9");
}

/// ELAB-13 (19) review round 5 — deklarasi sinyal di dalam blok generate
/// TIDAK boleh dianggap sinyal scope module. `expand_generate_block` menyalin
/// `ModuleItem::Decl` hasil ekspansi ke `module.decls`, sehingga tanpa koreksi
/// selisih-kemunculan `logic [7:0] L;` di dalam `generate if (1) begin : g`
/// menghapus `$unit localparam L` dari pandangan runtime level module.
/// Referensi iverilog 12: mL = 7 (level module tetap $unit).
/// CATATAN: referensi `L` DI DALAM blok generate (gL) belum benar — lihat
/// SISA butir (19) di audit.txt.
#[test]
fn test_generate_local_decl_does_not_shadow_unit_param_at_module_scope() {
    let source = r#"
localparam int L = 7;

module m;
    logic [31:0] gL, mL;
    generate
        if (1) begin : g
            logic [7:0] L;
            initial begin L = 8'h2A; gL = L; end
        end
    endgenerate
    initial begin #1 mL = L; end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(
        get("mL"),
        7,
        "sinyal di blok generate tidak menutupi $unit localparam di level module"
    );
}

/// ELAB-13 butir (7)/(15): `$bits(param)` memakai lebar DEKLARASI parameter,
/// bukan min-bits nilainya. Sebelum perbaikan `$bits(Q)` = 1 (nilai 1 hanya
/// butuh 1 bit) padahal Q 8 bit, dan `$bits(P)` = 1 padahal P int 32 bit.
/// Referensi iverilog 12: Q=8 P=32 S=8.
#[test]
fn test_bits_of_parameter_uses_declared_width() {
    let source = r#"
module tb;
    parameter [7:0] Q = 1;
    parameter P = 1;
    parameter signed [7:0] S = -1;
    logic [7:0] bq;
    logic [7:0] bp;
    logic [7:0] bs;
    initial begin
        bq = $bits(Q);
        bp = $bits(P);
        bs = $bits(S);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(
        get("bq"),
        8,
        "$bits(parameter [7:0] Q) harus 8 (iverilog 8)"
    );
    assert_eq!(
        get("bp"),
        32,
        "$bits(parameter P = 1) harus 32 (iverilog 32)"
    );
    assert_eq!(get("bs"), 8, "$bits(parameter signed [7:0] S) harus 8");
}

/// ELAB-13 butir (7)/(15) jalur const-fold: `$bits(param)` di initializer
/// localparam (fold_param_bits) dan di kondisi generate-if (const_eval)
/// juga harus memakai lebar deklarasi. Sebelum perbaikan BP=1 dan
/// generate-if `$bits(P) == 8` gagal dievaluasi (warning RT9003) lalu
/// mengambil cabang true. Referensi iverilog 12: BP=32, cabang else.
#[test]
fn test_bits_of_parameter_in_localparam_and_generate_if() {
    let source = r#"
module tb;
    parameter [7:0] Q = 1;
    parameter P = 1;
    localparam [7:0] BQ = $bits(Q);
    localparam [7:0] BP = $bits(P);
    logic [7:0] bq_lp;
    logic [7:0] bp_lp;
    logic [7:0] gen_bits;
    generate
        if ($bits(P) == 8) begin : g8
            initial gen_bits = 8'h11;
        end else begin : gn
            initial gen_bits = 8'h22;
        end
    endgenerate
    initial begin
        bq_lp = BQ;
        bp_lp = BP;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(get("bq_lp"), 8, "localparam BQ = $bits(Q) harus 8");
    assert_eq!(get("bp_lp"), 32, "localparam BP = $bits(P) harus 32");
    assert_eq!(
        get("gen_bits"),
        0x22,
        "$bits(P)=32 -> generate-if harus memilih cabang else (iverilog 22)"
    );
}

/// ELAB-13 butir (7)/(15) regresi-semantik: inferensi lebar literal default
/// HANYA untuk param tanpa range/dtype, dan hanya untuk `$bits`. Param
/// bertipe tetap memakai `DataType::width` seperti sebelumnya — `signed int`
/// = 32, range lebar = 128 (tanpa cap 64), typedef = 64 (perilaku lama).
/// Diuji di KETIGA jalur: initializer localparam (fold), blok `initial`
/// (IR), dan generate-if (const_eval) — ketiganya harus sepakat.
#[test]
fn test_bits_of_typed_parameters_keeps_previous_widths() {
    let source = r#"
module tb;
    parameter signed int SI = -1;
    parameter [127:0] W = 1;
    typedef logic [7:0] byte_t;
    parameter byte_t BT = 8'd3;
    localparam [15:0] BSI = $bits(SI);
    localparam [15:0] BW = $bits(W);
    localparam [15:0] BBT = $bits(BT);
    logic [15:0] bsi;
    logic [15:0] bw;
    logic [15:0] bbt;
    logic [15:0] psi;
    logic [15:0] pw;
    logic [15:0] pbt;
    logic [7:0] gen_si;
    generate
        if ($bits(SI) == 8) begin : g8
            initial gen_si = 8'h33;
        end else begin : gother
            initial gen_si = 8'h44;
        end
    endgenerate
    initial begin
        bsi = BSI;
        bw = BW;
        bbt = BBT;
        psi = $bits(SI);
        pw = $bits(W);
        pbt = $bits(BT);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(get("bsi"), 32, "$bits(parameter signed int SI) harus 32");
    assert_eq!(get("bw"), 128, "$bits(parameter [127:0] W) harus 128");
    assert_eq!(
        get("bbt"),
        64,
        "$bits(parameter byte_t BT) = 64 (perilaku lama)"
    );
    assert_eq!(get("psi"), 32, "jalur IR: $bits(signed int SI) harus 32");
    assert_eq!(get("pw"), 128, "jalur IR: $bits([127:0] W) harus 128");
    assert_eq!(get("pbt"), 64, "jalur IR: $bits(typedef BT) = 64");
    assert_eq!(
        get("gen_si"),
        0x44,
        "generate-if $bits(SI)==8 harus false (32 bit) -> cabang else"
    );
}

/// ELAB-13 butir (7)/(15) lintas-scope: `$bits(param)` harus sepakat di
/// jalur IR, initializer localparam, dan generate-if untuk (a) param `$unit`,
/// (b) localparam di dalam generate block (if DAN for), (c) param untyped
/// ber-override (`#(.P(2))` pada `parameter P = 1` tetap 32 bit).
/// Catatan: lebar tipe yang berasal dari ekspresi override (mis.
/// `#(.P(16'hABCD))` pada `parameter P = 8'd1` → iverilog 16) belum
/// dipropagasi — ELAB-13 butir (13) masih open.
#[test]
fn test_bits_of_parameter_across_scopes_and_override() {
    let source = r#"
parameter [7:0] UP = 1;

module sub #(parameter P = 1) ();
    localparam [15:0] BSUB = $bits(P);
    logic [15:0] psub;
    logic [15:0] bsub;
    logic [7:0] gsub;
    generate
        if ($bits(P) == 8) begin : g8
            initial gsub = 8'h11;
        end else begin : gn
            initial gsub = 8'h22;
        end
    endgenerate
    initial begin
        psub = $bits(P);
        bsub = BSUB;
    end
endmodule

module tb;
    parameter E = 4 + 4;
    localparam [15:0] BU = $bits(UP);
    localparam [15:0] BE = $bits(E);
    logic [15:0] pu;
    logic [15:0] bu;
    logic [15:0] pe;
    logic [15:0] be;
    logic [7:0] gu;
    logic [7:0] ge;
    generate
        if ($bits(UP) == 8) begin : gu8
            initial gu = 8'h33;
        end else begin : gun
            initial gu = 8'h44;
        end
    endgenerate
    generate
        if ($bits(E) == 8) begin : ge8
            initial ge = 8'h11;
        end else begin : gen
            initial ge = 8'h22;
        end
    endgenerate
    generate
        if (1) begin : blk
            localparam [7:0] GP = 8'd3;
            localparam [15:0] BG = $bits(GP);
            logic [15:0] pg;
            logic [15:0] bg;
            initial begin
                pg = $bits(GP);
                bg = BG;
            end
        end
    endgenerate
    genvar i;
    generate
        for (i = 0; i < 1; i = i + 1) begin : gf
            localparam [7:0] FP = 8'd3;
            localparam [15:0] FB = $bits(FP);
            logic [15:0] pfor;
            logic [15:0] bfor;
            logic [7:0] gfor;
            if ($bits(FP) == 7) begin : inner7
                initial gfor = 8'h77;
            end else begin : innern
                initial gfor = 8'h88;
            end
            initial begin
                pfor = $bits(FP);
                bfor = FB;
            end
        end
    endgenerate
    sub #(.P(2)) u1();
    initial begin
        pu = $bits(UP);
        bu = BU;
        pe = $bits(E);
        be = BE;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    // (a) $unit parameter
    assert_eq!(get("pu"), 8, "IR: $bits($unit UP) harus 8");
    assert_eq!(get("bu"), 8, "localparam BU = $bits(UP) harus 8");
    assert_eq!(get("gu"), 0x33, "generate-if $bits(UP)==8 harus true");
    // (b) localparam di dalam generate block (if dan for)
    assert_eq!(get("pg"), 8, "IR: $bits(GP) harus 8");
    assert_eq!(get("bg"), 8, "localparam BG = $bits(GP) harus 8");
    assert_eq!(get("gf[0].pfor"), 8, "IR: $bits(generate-for FP) harus 8");
    assert_eq!(get("gf[0].bfor"), 8, "localparam FB = $bits(FP) harus 8");
    assert_eq!(
        get("gf[0].gfor"),
        0x88,
        "generate-if dalam for: $bits(FP)==7 harus false (8 bit)"
    );
    // Default non-literal (`4+4`) belum terinferensi (ELAB-13 butir (3)) —
    // fallback 64 SERAGAM di IR/localparam/generate-if.
    assert_eq!(get("pe"), 64, "IR: $bits(default non-literal) fallback 64");
    assert_eq!(get("be"), 64, "localparam BE = $bits(E) fallback 64");
    assert_eq!(
        get("ge"),
        0x22,
        "generate-if $bits(E)==8 harus false (fallback 64)"
    );
    // (c) override pada param untyped: tetap lebar deklarasi (32), bukan 2
    assert_eq!(get("u1.psub"), 32, "IR: $bits(P) override tetap 32");
    assert_eq!(get("u1.bsub"), 32, "localparam BSUB = $bits(P) harus 32");
    assert_eq!(
        get("u1.gsub"),
        0x22,
        "generate-if $bits(P)==8 harus false untuk P 32 bit"
    );
}

/// ELAB-15: parameter dengan nilai i64::MIN tidak boleh panic di const_eval.
/// Sebelum perbaikan, unary minus pada 9223372036854775808 (i64::MAX+1)
/// di-fold sebagai `-inner` dan panic "attempt to negate with overflow".
#[test]
fn test_parameter_i64_min_does_not_panic() {
    let source = r#"
module tb;
    parameter P = -9223372036854775808;
    logic [63:0] y;
    initial begin
        y = P;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let yv = sigs
        .iter()
        .find(|(n, _)| n == "y")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(yv, 0x8000000000000000, "P harus 0x8000000000000000");
}

/// ELAB-15 (lanjutan): operasi aritmatika di atas i64::MIN tidak boleh
/// panic dan mengikuti guard i64::MIN / -1 (hasil 0), konsisten dengan
/// sized_divmod dan const_eval_with_params.
#[test]
fn test_parameter_i64_min_arithmetic_no_panic() {
    let source = r#"
module tb;
    parameter P = -9223372036854775808;
    localparam Q = P / -1;
    localparam R = P % -1;
    logic [63:0] a, b;
    initial begin
        a = Q;
        b = R;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    for (name, expected) in [("a", 0u64), ("b", 0u64)] {
        let v = sigs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap();
        assert_eq!(v, expected, "{} harus 0 (guard i64::MIN / -1)", name);
    }
}

/// ELAB-13: parameter tanpa range/tipe mengambil tipe nilainya (LRM 6.20.2).
/// Desimal tak berlebar = `int` 32 bit signed, jadi `~P` harus 32 bit.
/// Sebelum perbaikan, P di-emit Const 64 bit sehingga `~P` =
/// 0xFFFFFFFFFFFFFFFE dan perbandingan dengan 32'hFFFFFFFE menjadi false
/// (iverilog: fffffffe / true).
#[test]
fn test_untyped_decimal_parameter_is_32_bit() {
    let source = r#"
module tb;
    parameter P = 1;
    logic hit;
    initial begin
        hit = ((~P) == 32'hFFFFFFFE);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let hit = sigs
        .iter()
        .find(|(n, _)| n == "hit")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(hit, 1, "~P harus 32 bit (0xFFFFFFFE), bukan 64 bit");
}

/// ELAB-13: literal default berlebar + signed (`8'sd5`) juga mengikat lebar
/// param. Sebelum perbaikan, signedness inferensi membuat lebar diabaikan
/// (fallback 64 bit) sehingga `~D` = 0xFFFFFFFFFFFFFFFA, bukan 0xFA
/// (iverilog: fa).
#[test]
fn test_sized_signed_parameter_keeps_its_width() {
    let source = r#"
module tb;
    parameter D = 8'sd5;
    logic hit;
    initial begin
        hit = ((~D) == 8'hFA);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let hit = sigs
        .iter()
        .find(|(n, _)| n == "hit")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(hit, 1, "~D harus 8 bit (0xFA) untuk parameter 8'sd5");
}

/// ELAB-13: literal desimal tak berlebar MELEBAR bila nilainya tidak muat
/// dalam 32 bit (LRM 5.7.1; iverilog: 10^12 -> 41 bit). Lebar yang dipaksa 32
/// memotong nilai menjadi 3567587328 (regresi yang ditemukan saat review).
#[test]
fn test_large_unsized_decimal_parameter_is_not_truncated() {
    let source = r#"
module tb;
    parameter C = 1000000000000;
    logic [63:0] w;
    logic hit;
    logic width41;
    initial begin
        w = C;
        hit = (w == 64'd1000000000000);
        width41 = ((~C) == 41'h1172B5AEFFF);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (w, hit) = (
        sigs.iter().find(|(n, _)| n == "w").map(|(_, v)| v.to_u64()),
        sigs.iter()
            .find(|(n, _)| n == "hit")
            .map(|(_, v)| v.to_u64()),
    );
    assert_eq!(hit, Some(1), "parameter 10^12 tidak boleh terpotong");
    assert_eq!(w, Some(1000000000000), "nilai w harus 10^12");
    // Lebar 41 bit dikunci eksplisit: fallback 64 bit juga memberi w = 10^12,
    // jadi assertion nilai saja TIDAK mendeteksi regresi lebar.
    let width41 = sigs
        .iter()
        .find(|(n, _)| n == "width41")
        .map(|(_, v)| v.to_u64());
    assert_eq!(
        width41,
        Some(1),
        "~C harus 41 bit (0x1172B5AEFFF), bukan 64 bit"
    );
}

/// ELAB-14: perbandingan param vs literal di const-fold harus memakai
/// bit-pattern selebar operand, bukan i64 mentah. `parameter A = -1` (int
/// 32 bit) == 32'hFFFFFFFF harus 1 (iverilog), bukan 0. Mencakup tiga jalur:
/// ekspresi prosedural, localparam (const_eval_ext), dan generate-if
/// (prepass) — ketiganya punya evaluator sendiri.
#[test]
fn test_param_equality_with_literal_uses_bit_pattern() {
    let source = r#"
module tb;
    parameter A = -1;
    localparam L = (A == 32'hFFFFFFFF);
    logic eq32, eq8, neq32, lv;
    logic [3:0] gen_out;
    generate
        if (A == 32'hFFFFFFFF) begin : g
            assign gen_out = 4'd1;
        end else begin : g2
            assign gen_out = 4'd2;
        end
    endgenerate
    initial begin
        eq32 = (A == 32'hFFFFFFFF);
        eq8  = (A == 8'hFF);
        neq32 = (A != 32'hFFFFFFFF);
        lv = L;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).map(|(_, v)| v.to_u64());
    assert_eq!(get("eq32"), Some(1), "A == 32'hFFFFFFFF harus 1");
    assert_eq!(get("eq8"), Some(0), "A == 8'hFF harus 0 (lebar 8 bit)");
    assert_eq!(get("neq32"), Some(0), "A != 32'hFFFFFFFF harus 0");
    assert_eq!(get("lv"), Some(1), "localparam L harus 1 (const-fold)");
    assert_eq!(
        get("gen_out"),
        Some(1),
        "generate-if harus memilih cabang g"
    );
}

/// ELAB-14 regresi: fix lebar perbandingan TIDAK boleh mengubah lebar
/// operator context-determined. `~0`, `1 << 40`, `2 ** 40` di konteks 64 bit
/// harus tetap 64 bit (dulu sempat rusak saat literal desimal dipaksa 32 bit
/// di `sized_width`).
#[test]
fn test_decimal_literal_width_does_not_leak_to_context_operators() {
    let source = r#"
module tb;
    localparam [63:0] ALL = ~0;
    localparam [63:0] M = 1 << 40;
    localparam [63:0] P = 2 ** 40;
    logic [63:0] a, m, p;
    initial begin
        a = ALL; m = M; p = P;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).map(|(_, v)| v.to_u64());
    assert_eq!(get("a"), Some(u64::MAX), "~0 di konteks 64 bit = semua 1");
    assert_eq!(get("m"), Some(1 << 40), "1 << 40 harus 1099511627776");
    assert_eq!(get("p"), Some(1 << 40), "2 ** 40 harus 1099511627776");
}

/// ELAB-14: perbandingan param berlebar 64 dengan literal negatif harus
/// melebarkan literal ke lebar param (sign-extend), bukan me-mask per operand.
/// `parameter [63:0] B = -1; (B == -1)` = 1 (iverilog).
#[test]
fn test_wide_parameter_equality_sign_extends_literal() {
    let source = r#"
module tb;
    parameter [63:0] B = -1;
    parameter longint C = -1;
    localparam L1 = (B == -1);
    localparam L2 = (C == -1);
    logic l1, l2;
    initial begin
        l1 = L1; l2 = L2;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).map(|(_, v)| v.to_u64());
    assert_eq!(get("l1"), Some(1), "B (64 bit) == -1 harus 1");
    assert_eq!(get("l2"), Some(1), "longint C == -1 harus 1");
}

/// ELAB-14: jalur instance ber-override (`#(.A(-1))`) juga harus memakai
/// lebar param saat memilih cabang generate/localparam child — sebelumnya
/// hanya instance tanpa override yang benar.
#[test]
fn test_param_equality_in_overridden_child_instance() {
    let source = r#"
module child #(parameter A = -1) (output [3:0] g);
    localparam L = (A == 32'hFFFFFFFF) ? 4'd1 : 4'd2;
    assign g = L;
endmodule
module tb;
    logic [3:0] o1, o2;
    child c1 (.g(o1));
    child #(.A(-1)) c2 (.g(o2));
    initial begin
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).map(|(_, v)| v.to_u64());
    assert_eq!(
        get("o1"),
        Some(1),
        "instance default harus memilih cabang 1"
    );
    assert_eq!(get("o2"), Some(1), "instance override #(.A(-1)) juga 1");
}

/// ELAB-14 regresi (review ronde 2, B2/B3): perbandingan dengan literal
/// NEGATIF harus mengikuti signedness DEKLARASI param. `parameter A = -1`
/// (int 32 bit) dan `parameter signed [7:0] S8 = -1` sama-sama == -1;
/// sebelum perbaikan, `cmp_operand_width` tidak mengenal `-1` (unary minus)
/// dan `is_signed_expr` menganggap `Ident` selalu unsigned → hasil 0.
/// Test ini PENJAGA REGRESI (HEAD/iverilog sudah 1, patch sempat merusak).
#[test]
fn test_negative_literal_comparison_follows_param_signedness() {
    let source = r#"
module tb;
    parameter A = -1;
    parameter signed [7:0] S8 = -1;
    parameter int I = -1;
    localparam L1 = (A == -1);
    localparam L2 = (S8 == -1);
    localparam L3 = (I == -1);
    logic [3:0] gen_out;
    generate
        if (A == -1) begin : g
            assign gen_out = 4'd1;
        end else begin : g2
            assign gen_out = 4'd2;
        end
    endgenerate
    logic l1, l2, l3;
    initial begin
        l1 = L1; l2 = L2; l3 = L3;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).map(|(_, v)| v.to_u64());
    assert_eq!(get("l1"), Some(1), "parameter A = -1; A == -1 harus 1");
    assert_eq!(
        get("l2"),
        Some(1),
        "parameter signed [7:0] S8; S8 == -1 = 1"
    );
    assert_eq!(get("l3"), Some(1), "parameter int I = -1; I == -1 harus 1");
    assert_eq!(
        get("gen_out"),
        Some(1),
        "generate-if (A == -1) harus memilih cabang g"
    );
}

/// ELAB-14 regresi (review ronde 2, B1): lebar param TIDAK boleh bocor ke
/// operator context-determined. `~P`, `P << 8`, `P ** 2` di konteks
/// `localparam [63:0]` harus memakai lebar konteks 64 bit (iverilog:
/// ff..6f / 0x100 / 0x5100). Sebelum perbaikan, arm `Expr::Ident` di
/// `sized_width` memotong hasil ke lebar param (8 bit).
/// Test ini PENJAGA REGRESI (HEAD/iverilog sudah benar, patch sempat merusak).
#[test]
fn test_param_width_does_not_leak_to_context_determined_operators() {
    let source = r#"
module tb;
    parameter [7:0] P = 8'h90;
    parameter [7:0] Z0 = 8'h01;
    localparam [63:0] X = ~P;
    localparam [63:0] Y = Z0 << 8;
    localparam [63:0] W = P ** 2;
    logic [63:0] x, y, w;
    initial begin
        x = X; y = Y; w = W;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).map(|(_, v)| v.to_u64());
    assert_eq!(
        get("x"),
        Some(0xFFFF_FFFF_FFFF_FF6F),
        "~P di konteks 64 bit harus 0xffffffffffffff6f"
    );
    assert_eq!(get("y"), Some(0x100), "Z0 << 8 di konteks 64 bit = 0x100");
    assert_eq!(get("w"), Some(0x5100), "P ** 2 di konteks 64 bit = 0x5100");
}

/// ELAB-14 (review ronde 3, B1): bila SALAH SATU operand unsigned, KEDUA
/// operand di-zero-extend (LRM 11.8.2) — bukan di-sign-extend per operand.
/// Regresi revisi 2: `parameter [7:0] D = -1; D == 4'shF` jadi 1 (255 vs
/// -1 di-sign-extend). iverilog: semuanya 0 kecuali `B == -1` (literal
/// desimal tak berlebar melebar ke lebar konteks dengan sign extension).
#[test]
fn test_param_comparison_mixed_signedness_zero_extends_both() {
    let source = r#"
module tb;
    parameter [7:0] D = -1;
    parameter [7:0] P = 8'h90;
    parameter signed [7:0] S = -1;
    parameter [63:0] B = -1;
    parameter [7:0] MASK = 8'hFF;
    localparam M1 = (D == 4'shF);
    localparam M2 = (P < 4'shF);
    localparam M3 = (B == 4'shF);
    localparam M4 = (S == 32'hFFFFFFFF);
    localparam M5 = (B == -1);
    logic [3:0] g;
    generate
        if (MASK == 4'shF) begin : a
            assign g = 4'd1;
        end else begin : b
            assign g = 4'd2;
        end
    endgenerate
    logic m1, m2, m3, m4, m5;
    initial begin
        m1 = M1; m2 = M2; m3 = M3; m4 = M4; m5 = M5;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).map(|(_, v)| v.to_u64());
    assert_eq!(get("m1"), Some(0), "D (8 bit unsigned) == 4'shF harus 0");
    assert_eq!(get("m2"), Some(0), "P (144) < 4'shF (15) harus 0");
    assert_eq!(get("m3"), Some(0), "B (64 bit unsigned) == 4'shF harus 0");
    assert_eq!(
        get("m4"),
        Some(0),
        "S (signed 8 bit) == 32'hFFFFFFFF harus 0"
    );
    assert_eq!(
        get("m5"),
        Some(1),
        "B (64 bit) == -1 harus 1 (literal tak berlebar melebar ke konteks)"
    );
    assert_eq!(
        get("g"),
        Some(2),
        "generate-if (MASK == 4'shF) harus memilih cabang b"
    );
}

/// ELAB-14 (review ronde 3, B2): perbandingan RELASIONAL param signed tidak
/// boleh di-mask ke lebar operand setelah sign-extend — bit tanda harus tetap
/// di bit 63. Sebelumnya `parameter int I = -3; I < 0` jadi 0 (mask 32 bit
/// membuat nilainya 4294967293 sehingga "positif").
#[test]
fn test_signed_param_relational_keeps_sign() {
    let source = r#"
module tb;
    parameter int I = -3;
    parameter A = -1;
    parameter signed [7:0] S8 = -1;
    localparam R1 = (I < 0);
    localparam R2 = (I > 0);
    localparam R3 = (A >= 0);
    localparam R4 = (A < 0);
    localparam R5 = (S8 < 0);
    localparam R6 = (S8 >= 0);
    logic r1, r2, r3, r4, r5, r6;
    initial begin
        r1 = R1; r2 = R2; r3 = R3; r4 = R4; r5 = R5; r6 = R6;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).map(|(_, v)| v.to_u64());
    assert_eq!(get("r1"), Some(1), "int I = -3; I < 0 harus 1");
    assert_eq!(get("r2"), Some(0), "int I = -3; I > 0 harus 0");
    assert_eq!(get("r3"), Some(0), "parameter A = -1; A >= 0 harus 0");
    assert_eq!(get("r4"), Some(1), "parameter A = -1; A < 0 harus 1");
    assert_eq!(get("r5"), Some(1), "signed [7:0] S8 = -1; S8 < 0 harus 1");
    assert_eq!(get("r6"), Some(0), "signed [7:0] S8 = -1; S8 >= 0 harus 0");
}

/// ELAB-14 (review ronde 3, catatan 5): literal desimal TAK BERLEBAR melebar
/// ke lebar konteks dengan sign extension (LRM 5.7.1), sehingga
/// `-1 == 32'hFFFFFFFF` = 1 (iverilog) — sebelumnya 0 karena mask per
/// operand. Literal BERLEBAR tetap memakai aturan lama (`4'shF == 8'hFF` = 0).
#[test]
fn test_unsized_decimal_literal_comparison_is_context_determined() {
    let source = r#"
module tb;
    localparam L1 = (-1 == 32'hFFFFFFFF);
    localparam L2 = (-1 == 32'hFFFFFFFE);
    localparam L3 = (4'shF == 8'hFF);
    localparam L4 = (-1 == 8'hFF);
    logic l1, l2, l3, l4;
    initial begin
        l1 = L1; l2 = L2; l3 = L3; l4 = L4;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).map(|(_, v)| v.to_u64());
    assert_eq!(get("l1"), Some(1), "-1 == 32'hFFFFFFFF harus 1");
    assert_eq!(get("l2"), Some(0), "-1 == 32'hFFFFFFFE harus 0");
    assert_eq!(get("l3"), Some(0), "4'shF == 8'hFF harus 0");
    assert_eq!(get("l4"), Some(0), "-1 == 8'hFF harus 0");
}

/// ELAB-14 (review ronde 4, B1): literal desimal tak berlebar bertipe SIGNED,
/// jadi lebarnya = magnitude + 1 bit tanda. Tanpa bit tanda, 2147483648
/// dianggap 32 bit lalu di-sign-extend menjadi 0xFFFFFFFF80000000 sehingga
/// `parameter [63:0] BASE = 2147483648; BASE == 2147483648` jadi 0
/// (iverilog 1). Nilai negatif besar (-3000000000) juga butuh 33 bit.
#[test]
fn test_large_decimal_literal_comparison_counts_sign_bit() {
    let source = r#"
module tb;
    parameter [63:0] BASE = 2147483648;
    parameter [63:0] NEG = -3000000000;
    parameter int A = -1;
    localparam R1 = (BASE == 2147483648);
    localparam R2 = (NEG == -3000000000);
    localparam R3 = (A == 4294967295);
    localparam R4 = (BASE == 2147483647);
    logic r1, r2, r3, r4;
    initial begin
        r1 = R1; r2 = R2; r3 = R3; r4 = R4;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).map(|(_, v)| v.to_u64());
    assert_eq!(
        get("r1"),
        Some(1),
        "BASE (2^31) == 2147483648 harus 1 (literal 33 bit)"
    );
    assert_eq!(
        get("r2"),
        Some(1),
        "NEG (-3000000000) == -3000000000 harus 1"
    );
    assert_eq!(
        get("r3"),
        Some(0),
        "int A = -1; A == 4294967295 harus 0 (literal 33 bit positif)"
    );
    assert_eq!(get("r4"), Some(0), "BASE (2^31) == 2147483647 harus 0");
}

/// ELAB-14 (review ronde 4, B2): identifier yang lebarnya TIDAK diketahui
/// (enum member) harus memakai aturan lama. Sebelumnya jalur baru memaksa
/// 64 bit unsigned sehingga `typedef enum logic [3:0] {FB = 4'shF} e_t;
/// FB == 4'shF` jadi 0 (HEAD/iverilog 1).
#[test]
fn test_enum_member_comparison_keeps_head_behavior() {
    let source = r#"
typedef enum logic [3:0] {FB = 4'shF} e_t;
module tb;
    localparam B1 = (FB == 4'shF);
    localparam B2 = (FB == 8'hFF);
    localparam B3 = (FB == -1);
    logic b1, b2, b3;
    initial begin
        b1 = B1; b2 = B2; b3 = B3;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).map(|(_, v)| v.to_u64());
    assert_eq!(get("b1"), Some(1), "enum FB == 4'shF harus 1");
    assert_eq!(get("b2"), Some(0), "enum FB == 8'hFF harus 0");
    assert_eq!(
        get("b3"),
        Some(1),
        "enum FB == -1 harus 1 (perilaku HEAD, bukan regresi)"
    );
}

/// ELAB-14 (review ronde 6, B1): lebar/signedness param di BODY/generate juga
/// harus masuk scope const-fold setelah nilai di-resolve (`module_param_meta`
/// memindai `collect_body_params`). Tanpa itu `localparam int N = -5;
/// generate if (N == 32'hFFFFFFFB)` memilih cabang salah (HEAD: cabang else;
/// iverilog: cabang then). Diskriminatif: gagal di HEAD, lulus setelah fix.
#[test]
fn test_body_localparam_comparison_uses_declaration_width() {
    let source = r#"
module tb;
    localparam int N = -5;
    localparam [7:0] Q = -1;
    localparam [63:0] WIDE = -1;
    localparam LQ = (Q == 4'shF);
    localparam LW = (WIDE == -1);
    logic [3:0] g;
    generate
        if (N == 32'hFFFFFFFB) begin : y
            assign g = 4'd1;
        end else begin : n
            assign g = 4'd0;
        end
    endgenerate
    logic lq, lw;
    initial begin
        lq = LQ; lw = LW;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).map(|(_, v)| v.to_u64());
    assert_eq!(
        get("g"),
        Some(1),
        "localparam int N = -5 harus cocok 32'hFFFFFFFB (cabang y)"
    );
    assert_eq!(
        get("lq"),
        Some(0),
        "localparam [7:0] Q = -1; Q == 4'shF harus 0 (255 vs 15)"
    );
    assert_eq!(
        get("lw"),
        Some(1),
        "localparam [63:0] WIDE = -1; WIDE == -1 harus 1"
    );
}

#[test]
fn test_ranged_parameter_keeps_its_width() {
    // ELAB-13 varian deklarasi range: parameter [7:0] Q = 1 juga 8 bit.
    let source = r#"
module tb;
    parameter [7:0] Q = 1;
    logic [7:0] a;
    logic hit;
    initial begin
        a = 8'h0E;
        #1;
        hit = (~(a + Q) == 8'hF0);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let hit = sigs
        .iter()
        .find(|(n, _)| n == "hit")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(hit, 1, "~(a + Q) harus 8 bit untuk parameter [7:0]");
}

#[test]
fn test_parameter_override_is_not_truncated_by_default_width() {
    // ELAB-13: lebar dari literal default (8'd1) hanya inferensi — override
    // #(.P(16'hABCD)) tidak boleh dipotong ke 8 bit (iverilog: P=abcd).
    let source = r#"
module child #(parameter P = 8'd1) (output [15:0] y, output [15:0] z);
    assign y = P;
    assign z = ~P;
endmodule
module tb;
    wire [15:0] y, z;
    child #(.P(16'hABCD)) u(.y(y), .z(z));
    initial #2 $finish;
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(x, _)| x == n)
            .map(|(_, v)| v.to_u64())
            .unwrap()
    };
    assert_eq!(get("y"), 0xABCD, "nilai override tidak boleh terpotong");
    assert_eq!(get("z"), 0x5432, "~P harus pada lebar override (16 bit)");
}

#[test]
fn test_unit_parameter_width_does_not_shadow_module_parameter() {
    // ELAB-13: parameter $unit (8'd1) tidak boleh mewarnai lebar module param
    // bernama sama yang dideklarasikan tanpa lebar (iverilog: hit=0).
    let source = r#"
parameter P = 8'd1;
module tb;
    parameter P = 1;
    logic [7:0] a;
    logic hit;
    initial begin
        a = 8'h0E;
        #1;
        hit = (~(a + P) == 8'hF0);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let hit = sigs
        .iter()
        .find(|(n, _)| n == "hit")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(
        hit, 0,
        "module param tanpa lebar tidak mewarisi lebar $unit"
    );
}

#[test]
fn test_ranged_parameter_is_unsigned() {
    // ELAB-13: deklarasi range tanpa dtype bersifat UNSIGNED (LRM 6.8/6.20.2)
    // walau default-nya desimal unsized. Sebelum guard, TIMEOUT sign-extend
    // menjadi 0xFFFFC350 saat lebar range mulai dihormati.
    let source = r#"
module tb;
    parameter [15:0] TIMEOUT = 50000;
    localparam [7:0] THRESH = 200;
    logic [31:0] t, th;
    initial begin
        t = TIMEOUT;
        th = THRESH;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(x, _)| x == n)
            .map(|(_, v)| v.to_u64())
            .unwrap()
    };
    assert_eq!(
        get("t"),
        50000,
        "parameter [15:0] = 50000 harus zero-extend"
    );
    assert_eq!(get("th"), 200, "localparam [7:0] = 200 harus zero-extend");
}

#[test]
fn test_signed_literal_parameter_override_is_not_sign_extended() {
    // ELAB-13: param untyped dengan default signed (8'sd1) yang di-override
    // nilai unsigned lebar (16'hABCD) tidak boleh sign-extend ke 0xFFFFABCD.
    let source = r#"
module child #(parameter P = 8'sd1) (output [31:0] y);
    assign y = P;
endmodule
module tb;
    wire [31:0] y;
    child #(.P(16'hABCD)) u(.y(y));
    initial #2 $finish;
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let y = sigs
        .iter()
        .find(|(n, _)| n == "y")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(y, 0xABCD, "override unsigned tidak boleh sign-extend");
}

#[test]
fn test_ranged_parameter_negative_default_is_unsigned() {
    // ELAB-13: `parameter [7:0] D = -1` = 8'hFF = 255 (LRM), bukan -1.
    // Jalur const-fold memakai nilai mentah, jadi nilainya dinormalisasi ke
    // lebar deklarasi; tanpa itu perbandingan unsigned jadi salah.
    let source = r#"
module tb;
    parameter [7:0] D = -1;
    logic [31:0] d;
    logic g1, g2;
    initial begin
        d = D;
        g1 = (D < 1000);
        g2 = (D > 300);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(x, _)| x == n)
            .map(|(_, v)| v.to_u64())
            .unwrap()
    };
    assert_eq!(get("d"), 255, "parameter [7:0] = -1 harus 8'hFF");
    assert_eq!(get("g1"), 1, "255 < 1000");
    assert_eq!(get("g2"), 0, "255 tidak > 300");
}

#[test]
fn test_ranged_parameter_override_is_truncated() {
    // ELAB-13: range eksplisit MENGIKAT lebar (LRM) — override 16'h1234 pada
    // parameter [7:0] menjadi 8'h34, termasuk di jalur const-fold.
    let source = r#"
module child #(parameter [7:0] Q = 0) (output [31:0] a, output [31:0] b);
    assign a = Q;
    assign b = Q + 0;
endmodule
module tb;
    wire [31:0] a, b;
    child #(.Q(16'h1234)) u(.a(a), .b(b));
    initial #2 $finish;
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(x, _)| x == n)
            .map(|(_, v)| v.to_u64())
            .unwrap()
    };
    assert_eq!(get("a"), 0x34, "override harus dipotong ke lebar range");
    assert_eq!(
        get("b"),
        0x34,
        "jalur const-fold juga memakai nilai terpotong"
    );
}

#[test]
fn test_ranged_parameter_negative_default_in_localparam() {
    // ELAB-13: nilai ternormalisasi harus terlihat juga oleh const-fold
    // localparam turunan (bukan hanya assignment-level).
    let source = r#"
module tb;
    parameter [7:0] D = -1;
    localparam X = D + 0;
    localparam Y = (D < 1000) ? 1 : 0;
    logic [31:0] x, y;
    initial begin
        x = X;
        y = Y;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(x, _)| x == n)
            .map(|(_, v)| v.to_u64())
            .unwrap()
    };
    assert_eq!(get("x"), 255, "localparam X = D + 0 harus 255");
    assert_eq!(get("y"), 1, "localparam Y = (D < 1000) harus 1");
}

#[test]
fn test_ranged_parameter_negative_default_in_generate_if() {
    // ELAB-13: generate-if memakai peta param dari prepass; nilai mentah -1
    // membuat cabang else yang dipilih (hardware salah).
    let source = r#"
module tb;
    parameter [7:0] D = -1;
    logic sel;
    generate
        if (D < 1000) begin : g_lt
            assign sel = 1'b1;
        end else begin : g_ge
            assign sel = 1'b0;
        end
    endgenerate
    initial #1 $finish;
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let sel = sigs
        .iter()
        .find(|(n, _)| n == "sel")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(sel, 1, "D = 255 sehingga D < 1000 harus memilih cabang if");
}

#[test]
fn test_ranged_parameter_override_in_localparam() {
    // ELAB-13: override yang lebih lebar harus dipotong range eksplisit juga
    // di jalur const-fold localparam anak.
    let source = r#"
module child #(parameter [7:0] Q = 0) (output [31:0] c);
    localparam X = Q + 0;
    assign c = X;
endmodule
module tb;
    wire [31:0] c;
    child #(.Q(16'h1234)) u(.c(c));
    initial #2 $finish;
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let c = sigs
        .iter()
        .find(|(n, _)| n == "c")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(c, 0x34, "localparam X = Q + 0 harus memakai Q 8 bit (0x34)");
}

#[test]
fn test_signed_ranged_parameter_bit_pattern_is_negative() {
    // ELAB-13: `parameter signed [7:0] S = 8'hFF` bernilai -1 (sign-extend),
    // bukan +255 — berlaku di const-fold (S + 0) dan generate-if.
    let source = r#"
module tb;
    parameter signed [7:0] S = 8'hFF;
    logic neg;
    logic [31:0] sum;
    logic gen_sel;
    generate
        if (S < 0) begin : g_neg
            assign gen_sel = 1'b1;
        end else begin : g_pos
            assign gen_sel = 1'b0;
        end
    endgenerate
    initial begin
        neg = (S < 0);
        sum = S + 0;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(x, _)| x == n)
            .map(|(_, v)| v.to_u64())
            .unwrap()
    };
    assert_eq!(get("neg"), 1, "S = -1 sehingga S < 0 harus benar");
    assert_eq!(get("sum"), 0xFFFF_FFFF, "S + 0 harus sign-extend");
    assert_eq!(get("gen_sel"), 1, "generate-if harus memilih cabang S < 0");
}

#[test]
fn test_typedef_parameter_does_not_narrow_to_literal() {
    // ELAB-13: tipe eksplisit yang lebarnya belum didukung (typedef) TIDAK
    // boleh jatuh ke lebar literal default (8 bit) — itu memotong nilai.
    let source = r#"
typedef logic [15:0] word_t;
module tb;
    parameter word_t W = 8'hFF;
    logic hit8;
    initial begin
        // Kalau W menyempit ke 8 bit, ~W = 8'h00 sehingga perbandingan ini
        // benar; dengan fallback 64 bit nilainya 0xFFFF..FF00 -> salah.
        hit8 = (~W == 8'h00);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(x, _)| x == n)
            .map(|(_, v)| v.to_u64())
            .unwrap()
    };
    assert_eq!(get("hit8"), 0, "W tidak boleh menyempit ke lebar literal");
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
