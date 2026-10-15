// Seed 52: struct param + localparam kondisional — area tipis (struct-field
// flatten, param-driven width; pola CVA6 nyata).
//
// Mivon-only (iverilog -g2012 tak dukung parameter bertipe struct maupun
// `parameter type`): nilai untuk target elab/sim/mutasi, bukan cross-sim.
// Marker ASRT_* tetap ada utk konsistensi korpus.

`timescale 1ns/1ps
typedef struct packed {
  int unsigned XLEN;
  bit          BIG;
} cfg_t;

module paramtype_user #(
  parameter cfg_t CFG = '{XLEN: 32, BIG: 0}
) (
  input  logic [CFG.XLEN-1:0] a,
  output logic [7:0]           y
);
  localparam int W = CFG.BIG ? 16 : 8;
  assign y = a[W-1:0];
endmodule

module tb;
  logic [31:0] a = 32'hA5A5_00FF;
  logic [7:0]  y;

  paramtype_user #(.CFG('{XLEN: 32, BIG: 0})) dut (.a(a), .y(y));

  initial begin
    $display("ASRT_START paramtype");
    #1;
    if (y !== 8'hFF) $error("ASRT_Y_BAD=<%h>", y);
    else $display("ASRT_Y=<ff>");
    $display("ASRT_END paramtype");
    $finish;
  end
endmodule
