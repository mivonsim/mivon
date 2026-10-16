// Seed 54: declaration initializer + edge at t=0 — area inisialisasi LRM §6.8
// (temuan fuzz verify_bad_0586: initializer X->0 memicu negedge palsu).
`timescale 1ns/1ps
module decl_init_demo (
  input  logic clk, rst_n,
  output logic q
);
  logic d = 1'b0;
  always_ff @(negedge clk or negedge rst_n) begin
    if (!rst_n) q <= 1'b0;
    else q <= d;
  end
endmodule
