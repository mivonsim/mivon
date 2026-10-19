use super::*;

// REGRESI Tahap 1: nested bit/part-select pada lvalue.
//
// Sebelumnya konstruk berikut memicu hard-error elaborator
// nested bit select not supported / nested part-select in lvalue not supported:
//   mem[i][7:0][k]      bit-select di atas ArrayRangeSelect
//   mem[i][15:8][2:0]   range-select di atas ArrayRangeSelect
//   mem[i][j][k]        bit-select di atas ArrayBitSelect
//   mem[i][j][2:0]      range-select di atas ArrayBitSelect
//   sig[base+:8][k]     bit-select di atas ExprPartSelect
//   sig[base+:8][3:0]   range-select di atas ExprPartSelect
// Sekarang didukung penuh (array unpacked + part-select dinamis).

fn sim(source: &str, max_time: u64) -> Vec<(String, mivon_ir::LogicVec)> {
    simulate_signals(source, max_time)
        .unwrap_or_else(|e| panic!("sim gagal: {e}\n{source}"))
}

fn get_u64(sigs: &[(String, mivon_ir::LogicVec)], name: &str) -> u64 {
    sigs.iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("signal tidak ada: {name}"))
        .1
        .to_u64()
}

fn get_mem_elem(
    sigs: &[(String, mivon_ir::LogicVec)],
    name: &str,
    elem: usize,
    elem_width: usize,
) -> u64 {
    let v = &sigs
        .iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("signal tidak ada: {name}"))
        .1;
    let mut val: u64 = 0;
    for b in 0..elem_width {
        let idx = elem * elem_width + b;
        if idx < v.bits.len() && v.bits[idx] == mivon_core::LogicVal::One {
            val |= 1u64 << b;
        }
    }
    val
}

#[test]
fn nested_bitselect_on_arrayrangeselect() {
    let src = "module top;
  reg [31:0] mem [0:3];
  initial begin
    mem[1] = 32'h0;
    mem[1][7:0][3] = 1'b1;
    $finish;
  end
endmodule";
    let sigs = sim(src, 10);
    assert_eq!(get_mem_elem(&sigs, "mem", 1, 32), 0x8, "mem[1] bit3 harus 1");
}

#[test]
fn nested_rangeselect_on_arrayrangeselect() {
    let src = "module top;
  reg [31:0] mem [0:3];
  initial begin
    mem[2] = 32'h0;
    mem[2][15:8][2:0] = 3'b101;
    $finish;
  end
endmodule";
    let sigs = sim(src, 10);
    assert_eq!(get_mem_elem(&sigs, "mem", 2, 32), 0x500, "mem[2][10:8]=101");
}

#[test]
fn nested_bitselect_on_arraybitselect() {
    let src = "module top;
  reg [31:0] mem [0:3];
  initial begin
    mem[0] = 32'h0;
    mem[0][12][1] = 1'b1;
    $finish;
  end
endmodule";
    let sigs = sim(src, 10);
    assert_eq!(get_mem_elem(&sigs, "mem", 0, 32), 0x2000, "mem[0] bit13 harus 1");
}

#[test]
fn nested_rangeselect_on_arraybitselect() {
    let src = "module top;
  reg [31:0] mem [0:3];
  initial begin
    mem[3] = 32'h0;
    mem[3][20][2:0] = 3'b011;
    $finish;
  end
endmodule";
    let sigs = sim(src, 10);
    assert_eq!(get_mem_elem(&sigs, "mem", 3, 32), 0x300000, "mem[3][22:20]=011");
}

#[test]
fn nested_bitselect_on_exprpartselect() {
    let src = "module top;
  reg [31:0] sig;
  integer b;
  initial begin
    sig = 32'h0;
    b = 8;
    sig[b +: 8][2] = 1'b1;
    $finish;
  end
endmodule";
    let sigs = sim(src, 10);
    assert_eq!(get_u64(&sigs, "sig"), 0x400, "sig[10] harus 1");
}

#[test]
fn nested_rangeselect_on_exprpartselect() {
    let src = "module top;
  reg [31:0] sig;
  integer b;
  initial begin
    sig = 32'h0;
    b = 16;
    sig[b +: 8][3:0] = 4'b1010;
    $finish;
  end
endmodule";
    let sigs = sim(src, 10);
    assert_eq!(get_u64(&sigs, "sig"), 0xA0000, "sig[19:16]=1010");
}

#[test]
fn nested_select_combined() {
    let src = "module top;
  reg [31:0] mem [0:3];
  reg [31:0] sig;
  integer b;
  initial begin
    mem[1] = 32'h0;  mem[1][7:0][3] = 1'b1;
    mem[2] = 32'h0;  mem[2][15:8][2:0] = 3'b101;
    mem[0] = 32'h0;  mem[0][12][1] = 1'b1;
    mem[3] = 32'h0;  mem[3][20][2:0] = 3'b011;
    sig = 32'h0;  b = 8;   sig[b +: 8][2] = 1'b1;
    sig = 32'h0;  b = 16;  sig[b +: 8][3:0] = 4'b1010;
    $finish;
  end
endmodule";
    let sigs = sim(src, 10);
    assert_eq!(get_mem_elem(&sigs, "mem", 1, 32), 0x8);
    assert_eq!(get_mem_elem(&sigs, "mem", 2, 32), 0x500);
    assert_eq!(get_mem_elem(&sigs, "mem", 0, 32), 0x2000);
    assert_eq!(get_mem_elem(&sigs, "mem", 3, 32), 0x300000);
    assert_eq!(get_u64(&sigs, "sig"), 0xA0000);
}

#[test]
fn const_function_break_continue() {
    let src = r#"package P;
  function automatic int find_sum();
    int i;
    int s;
    s = 0;
    for (i = 0; i < 10; i++) begin
      if (i == 5) break;
      if (i % 2 == 1) continue;
      s = s + i;
    end
    return s;
  endfunction
  localparam int R = find_sum();
endpackage
module top(output [7:0] o);
  assign o = P::R;
endmodule"#;
    let sigs = sim(src, 10);
    assert_eq!(get_u64(&sigs, "o"), 6);
}

#[test]
fn nested_bitselect_on_concat_lvalue() {
    let src = r#"module top;
  reg [7:0] a;
  reg [7:0] b;
  initial begin
    a = 8'h00;
    b = 8'h00;
    {a, b}[0] = 1'b1;
    {a, b}[8] = 1'b1;
    $finish;
  end
endmodule"#;
    let sigs = sim(src, 10);
    assert_eq!(get_u64(&sigs, "a"), 1);
    assert_eq!(get_u64(&sigs, "b"), 1);
}

#[test]
fn nested_partselect_on_concat_lvalue() {
    let src = r#"module top;
  reg [7:0] a;
  reg [7:0] b;
  initial begin
    a = 8'h00;
    b = 8'h00;
    {a, b}[0 +: 4] = 4'hF;
    {a, b}[8 +: 4] = 4'hA;
    $finish;
  end
endmodule"#;
    let sigs = sim(src, 10);
    assert_eq!(get_u64(&sigs, "a"), 0x0A);
    assert_eq!(get_u64(&sigs, "b"), 0x0F);
}
