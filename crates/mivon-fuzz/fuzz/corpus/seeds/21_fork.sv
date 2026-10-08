// Seed 21: fork/join + fork/join_any di testbench
// NOTE differential-hygiene: `timescale SAMA dgn TB (mixed-timescale =
// delay DUT jalan di satuan default tool → hasil tak sebanding antar
// simulator), dan TANPA $finish di DUT ($finish DUT di t≈10 mematikan sim
// sebelum TB sampling di t≈30 → marker vakum dua sisi).
`timescale 1ns/1ps
module fork_demo (
  input logic clk,
  output logic [3:0] result
);
  initial begin
    fork
      begin
        @(posedge clk);
        result = 4'h1;
      end
      begin
        @(posedge clk);
        @(posedge clk);
        result = 4'h2;
      end
    join_any
    #5;
    result = 4'hF;
  end
endmodule