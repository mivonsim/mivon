// Seed 44: covergroup sampling (coverage pipeline mcov — target baru fuzz)
// CATATAN: formal args covergroup `cg (input logic v, ...)` BELUM didukung
// (parser melewatkan daftar arg — temuan seed ini); pakai signal module
// langsung sebagai coverpoint expression (pola didukung penuh).
module cover_pipeline (
  input logic       clk,
  input logic       rst_n,
  input logic       req_valid,
  input logic       req_write,
  input logic [1:0] req_len,
  output logic      ready
);
  covergroup cg_req @ (posedge clk);
    cp_valid: coverpoint req_valid {
      bins zero = {0};
      bins one  = {1};
    }
    cp_write: coverpoint req_write;
    cp_len:   coverpoint req_len {
      bins small = {0, 1};
      bins large = {2, 3};
    }
    cp_cross: cross cp_valid, cp_write, cp_len;
  endgroup

  cg_req cg = new;

  always_ff @(posedge clk or negedge rst_n) begin
    if (!rst_n) begin
      ready <= 1'b0;
    end else begin
      ready <= req_valid;
      if (req_valid) begin
        cg.sample();
      end
    end
  end
endmodule
