// Seed 51: disable + fork/join_any — area tipis (kontrol konkurren,
// terminasi branch; rawan hang/false-hang di oracle).
//
// Marker ASRT_*=<val> utk differential iverilog (oracle_icarus).

`timescale 1ns/1ps
module tb;
  logic clk = 0;
  logic [3:0] cnt = 0;
  logic done = 0;

  always #5 clk = ~clk;

  initial begin
    $display("ASRT_START disable_fork");
    fork : workers
      begin
        repeat (4) @(posedge clk);
        cnt = 4'd7;
      end
      begin
        repeat (2) @(posedge clk);
        disable workers;
      end
    join_any
    #1;
    if (cnt !== 4'd0) $error("ASRT_CNT_BAD=<%0d>", cnt);
    else $display("ASRT_CNT=<0>");
    done = 1;
    $display("ASRT_DONE=<1>");
    $finish;
  end
endmodule
