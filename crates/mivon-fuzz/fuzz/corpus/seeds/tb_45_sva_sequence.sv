// TB 45_sva_sequence — sequence/property assertion reference.
// Stimulus: ack dalam TIMEOUT (pass), ack tanpa req (violation), cover.
`timescale 1ns/1ps
module tb_req_ack_sva;
  logic clk = 0;
  logic rst_n = 0;
  logic req = 0, ack = 0;

  req_ack_sva #(.TIMEOUT(16)) dut (
    .clk(clk), .rst_n(rst_n), .req(req), .ack(ack)
  );

  always #5 clk = ~clk;

  initial begin
    $display("ASRT_START tb_req_ack_sva");
    rst_n = 0;
    @(negedge clk);
    rst_n = 1;
    // case pass: req lalu ack di cycle berikutnya
    @(negedge clk);
    req = 1;
    @(negedge clk);
    req = 0; ack = 1;
    @(negedge clk);
    ack = 0;
    $display("ASRT_CASE1=req_then_ack");
    // case cover: req ##[1:3] ack
    @(negedge clk);
    req = 1;
    @(negedge clk);
    req = 0;
    @(negedge clk);
    ack = 1;
    @(negedge clk);
    ack = 0;
    $display("ASRT_CASE2=cover_seq");
    $display("ASRT_END tb_req_ack_sva");
    $finish;
  end
endmodule
