// TB 53_asc_range — ascending-range cross-sim differential reference.
`timescale 1ns/1ps
module tb_asc_range;
  logic clk = 0;
  logic rst_n = 0;
  logic [0:3] out_asc;

  asc_range_demo dut (.clk(clk), .rst_n(rst_n), .out_asc(out_asc));

  always #5 clk = ~clk;

  initial begin
    $display("ASRT_START tb_asc_range");
    #20;
    rst_n = 1;
    repeat (4) @(posedge clk);
    @(negedge clk);
    $display("ASRT_ASC=<%0h>", out_asc);
    assert (out_asc !== 'x) else $error("ASRT_ASC_X_BAD=<%0h>", out_asc);
    $display("ASRT_END tb_asc_range");
    $finish;
  end
endmodule
