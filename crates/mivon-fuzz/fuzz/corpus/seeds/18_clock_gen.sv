// Seed 18: initial + delays + $display + clock gen pattern
// NOTE differential-hygiene: `timescale SAMA dgn TB (lihat 21_fork.sv).
`timescale 1ns/1ps
module clock_gen (
  output logic clk,
  output logic [3:0] count
);
  initial begin
    clk = 0;
    forever #5 clk = ~clk;
  end

  initial begin
    count = 0;
    repeat (16) begin
      @(posedge clk);
      count = count + 1;
    end
    #10;
    $display("count = %0d", count);
    $finish;
  end
endmodule