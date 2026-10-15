// Seed 50: specify timing checks — area tipis (specparam + $setup/$hold
// jarang tersentuh mutasi; path specify elaborator/engine).
//
// Marker ASRT_*=<val> utk differential iverilog (oracle_icarus).

`timescale 1ns/1ps
module specify_probe (
  input  logic clk,
  input  logic d,
  output logic q
);
  specparam tSU = 2, tH = 1;

  always @(posedge clk) q <= d;

  specify
    specparam tpd = 3;
    (clk => q) = tpd;
    $setup(d, posedge clk, tSU);
    $hold(posedge clk, d, tH);
  endspecify
endmodule

module tb;
  logic clk = 0;
  logic d = 0;
  logic q;

  specify_probe dut (.clk(clk), .d(d), .q(q));

  always #5 clk = ~clk;

  initial begin
    $display("ASRT_START specify");
    d = 1;
    #12;
    if (q !== 1'b1) $error("ASRT_Q_BAD=<%b>", q);
    else $display("ASRT_Q=<1>");
    #10;
    $display("ASRT_END specify");
    $finish;
  end
endmodule
