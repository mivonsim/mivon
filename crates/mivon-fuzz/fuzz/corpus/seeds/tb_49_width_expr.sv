// TB 49_width_expr — width-sensitive arithmetic dengan marker ASRT
// (cross-sim differential reference vs iverilog -g2012).
//
// Setiap stimulus menghasilkan marker `ASRT_<nama>=<<nilai>>` — wrapper
// `<<>>` WAJIB: `oracle_icarus::find_marker_tokens` hanya mengekstrak token
// bila `ASRT_...=` diikuti `>` di baris yang sama. Tanpa wrapper, kedua sisi
// punya marker kosong → `Verdict::Match` vakum selamanya.
// Semua nilai dipilih agar perbedaan truncation TERLIHAT: mis.
// `op8 = 17` → `17**2` = 289, yang di 8 bit jadi 33. Kalau lebar salah,
// hasilnya 289.

`timescale 1ns/1ps
module tb_width_expr;
  logic [7:0]  op8;
  logic [15:0] op16;
  logic        en;
  logic [31:0] pow_res, mul_res, add_res;
  logic [7:0]  trunc_res;

  width_sensitive_probe dut (.op8(op8), .op16(op16), .en(en),
                            .pow_res(pow_res), .mul_res(mul_res),
                            .add_res(add_res), .trunc_res(trunc_res));

  initial begin
    $display("ASRT_START tb_width_expr");

    // en=0 → semua output 0.
    en = 1'b0; op8 = 8'd17; op16 = 16'd200;
    #1 $display("ASRT_IDLE=<%h>", pow_res ^ mul_res ^ add_res ^ {24'h0, trunc_res});

    // op8 = 17: 17**2 = 289, truncate ke 8 bit = 33 (0x21).
    // Kalau lebar salah (mask ikut 32 bit) → trunc_res = 0x121 (289).
    en = 1'b1; op8 = 8'd17; op16 = 16'd200;
    #1 $display("ASRT_TRUNC17=<%0d>", trunc_res);

    // op8 = 2: 2**3 = 8 (muat), pow_res = {24'h0,8} ^ {16'h0,200**3 truncated}.
    en = 1'b1; op8 = 8'd2; op16 = 16'd3;
    #1 $display("ASRT_TRUNC2=<%0d>", trunc_res);

    // op8 = 3: 3**2 = 9 (muat di 8 bit).
    en = 1'b1; op8 = 8'd3; op16 = 16'd4;
    #1 $display("ASRT_TRUNC3=<%0d>", trunc_res);

    // Perkalian 16-bit: 200*200 = 40000 (muat di 16 bit, tapi konteks 32).
    en = 1'b1; op8 = 8'd5; op16 = 16'd200;
    #1 $display("ASRT_MUL200=<%0d>", mul_res);

    // Perkalian yang overflow 16-bit: 255*255 = 65025 (muat 17 bit).
    en = 1'b1; op8 = 8'd1; op16 = 16'd255;
    #1 $display("ASRT_MUL255=<%0d>", mul_res);

    // Penambahan dengan literal unsized, truncate ke 8 bit:
    // op8 = 255 + 1 = 256 → 0 (8-bit wrap).
    en = 1'b1; op8 = 8'd255; op16 = 16'd1;
    #1 $display("ASRT_ADDWRAP=<%0d>", add_res);

    // Pen Addition yang muat: 100 + 1 = 101.
    en = 1'b1; op8 = 8'd100; op16 = 16'd1;
    #1 $display("ASRT_ADDFIT=<%0d>", add_res);

    $display("ASRT_END tb_width_expr");
    $finish;
  end
endmodule