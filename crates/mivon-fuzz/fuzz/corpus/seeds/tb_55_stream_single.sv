// TB 55_stream_single — streaming cross-sim differential reference.
`timescale 1ns/1ps
module tb_stream_single;
  logic clk = 0;
  logic rst_n = 0;
  logic [7:0] out;

  stream_single_demo dut (.clk(clk), .rst_n(rst_n), .out(out));

  always #5 clk = ~clk;

  initial begin
    $display("ASRT_START tb_stream_single");
    #20;
    rst_n = 1;
    repeat (4) @(posedge clk);
    @(negedge clk);
    $display("ASRT_STREAM=<%0h>", out);
    assert (out !== 'x) else $error("ASRT_STREAM_X_BAD=<%0h>", out);
    $display("ASRT_END tb_stream_single");
    $finish;
  end
endmodule
