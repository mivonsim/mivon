// Seed 53: ascending range [0:7] + part-select — area rawan posisi bit fisik
// vs indeks deklarasi (temuan fuzz verify_bad_0021, directed force_ascending_range).
`timescale 1ns/1ps
module asc_range_demo (
  input  logic clk, rst_n,
  output logic [0:3] out_asc
);
  logic [0:7] cnt;
  always_ff @(posedge clk or negedge rst_n) begin
    if (!rst_n) cnt <= '0;
    else cnt <= cnt + 1'b1;
  end
  assign out_asc = cnt[0 +: 4];
endmodule
