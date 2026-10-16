// Seed 55: streaming single-driver + concat/signed — versi VALID dari pola
// bug d291 (streaming ganda = differential dag). Satu driver saja agar
// differential bermakna (mivon == iverilog).
`timescale 1ns/1ps
module stream_single_demo (
  input  logic clk, rst_n,
  output logic [7:0] out
);
  logic [7:0] a;
  always_ff @(posedge clk or negedge rst_n) begin
    if (!rst_n) a <= 8'h00;
    else a <= a + 1'b1;
  end
  assign out = {<<{a}};
endmodule
