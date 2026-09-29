// TB 47_rand_constraint — class rand/constraint/randomize() (jalur
// rejection sampling simulator). Marker ASRT utk verify/combine.
`timescale 1ns/1ps
module tb_pkt_driver;
  logic clk = 0;
  logic rst_n = 0;

  pkt_driver dut (.clk(clk), .rst_n(rst_n));

  always #5 clk = ~clk;

  initial begin
    $display("ASRT_START tb_pkt_driver");
    rst_n = 0;
    @(negedge clk);
    rst_n = 1;
    // randomize() jalan di initial dut (design path) — beri waktu + 2 edge
    // utk always @(posedge clk) menggerakkan addr.
    repeat (4) @(posedge clk);
    $display("ASRT_DRIVER_ALIVE");
    $display("ASRT_END tb_pkt_driver");
    $finish;
  end
endmodule
