// TB 44_covergroup — coverage sampling cross-sim differential reference.
// Stimulus coverpoint: valid/write/len melewati semua bins + cross.
`timescale 1ns/1ps
module tb_cover_pipeline;
  logic clk = 0;
  logic rst_n = 0;
  logic req_valid = 0, req_write = 0;
  logic [1:0] req_len = 0;
  logic ready;

  cover_pipeline dut (
    .clk(clk), .rst_n(rst_n),
    .req_valid(req_valid), .req_write(req_write), .req_len(req_len),
    .ready(ready)
  );

  always #5 clk = ~clk;

  initial begin
    $display("ASRT_START tb_cover_pipeline");
    rst_n = 0;
    @(negedge clk);
    rst_n = 1;
    // cp_len bins: small (0,1) + large (2,3); cp_write + cp_valid.
    @(negedge clk);
    req_valid = 1; req_write = 0; req_len = 0;
    @(negedge clk);
    req_len = 1;
    @(negedge clk);
    req_write = 1; req_len = 2;
    @(negedge clk);
    req_len = 3;
    @(negedge clk);
    req_valid = 0; req_write = 0; req_len = 0;
    @(negedge clk);
    $display("ASRT_READY=%0d", ready);
    $display("ASRT_END tb_cover_pipeline");
    $finish;
  end
endmodule
