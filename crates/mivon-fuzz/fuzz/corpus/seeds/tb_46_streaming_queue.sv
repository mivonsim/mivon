// TB 46_streaming_queue — streaming operator {<<{}}/{>>{}} + queue methods.
// Stimulus: push/pop dgn data berbeda; marker ASRT utk verify.
`timescale 1ns/1ps
module tb_stream_queue;
  logic clk = 0;
  logic rst_n = 0;
  logic [7:0] par_in = 8'h00;
  logic push = 0, pop = 0;
  logic [7:0] par_out;

  stream_queue #(.N(8)) dut (
    .clk(clk), .rst_n(rst_n), .par_in(par_in),
    .push(push), .pop(pop), .par_out(par_out)
  );

  always #5 clk = ~clk;

  initial begin
    $display("ASRT_START tb_stream_queue");
    rst_n = 0;
    @(negedge clk);
    rst_n = 1;
    // push 2 elemen (rev_bytes: 8'hA5 → byte-reverse tetap A5 utk 1 byte utuh;
    // N=8 = 1 byte → rev = id) — queue menampung hasil reverse.
    @(negedge clk);
    par_in = 8'hA5; push = 1;
    @(negedge clk);
    par_in = 8'h3C; push = 1;
    @(negedge clk);
    push = 0;
    // par_out = q[0] bila ada (FIFO head)
    @(negedge clk);
    $display("ASRT_HEAD=%02h", par_out);
    // pop head → head berikutnya / fallback rev_bits(input)
    pop = 1;
    @(negedge clk);
    pop = 0;
    @(negedge clk);
    $display("ASRT_AFTER_POP=%02h", par_out);
    $display("ASRT_END tb_stream_queue");
    $finish;
  end
endmodule
