// Seed 49: ekspresi width-sensitive — gap sweep differential (bug F82:
// lebar hasil `**` salah di runtime DAN const-eval).
//
// Fokus: setiap operator yang lebarnya context-determined. LRM 1800-2017
// §11.6.1 Tabel 11-21 menetapkan lebar hasil per operator; `**` dan
// shift hanya mengikuti lebar operand KIRI, sedangkan `+ - * /` memakai
// max(lebar kiri, kanan). Literal unsized (32 bit) adalah penyebab
// klasik: `8'd17 ** 2` salah jadi 289 kalau mask ikut 32 bit.
//
// Area ini belum punya seed: seeds 12/15/20/33/40/42 sudah punya aritmetika
// dasar tapi tak ada yang menguji `**` + konstanta unsized + truncation
// ke lebar kecil secara bersamaan.

`timescale 1ns/1ps
module width_sensitive_probe (
  input  logic [7:0]  op8,
  input  logic [15:0] op16,
  input  logic        en,
  output logic [31:0] pow_res,
  output logic [31:0] mul_res,
  output logic [31:0] add_res,
  output logic [7:0]  trunc_res
);
  // Pangkat: hasil harus selebar OPERAN KIRI, bukan max dengan literal unsized.
  // `2` adalah literal unsized 32-bit — inilah trigger bug F82.
  wire [7:0]  p8  = op8  ** 2;      // truncate ke 8 bit
  wire [15:0] p16 = op16 ** 3;      // truncate ke 16 bit

  // Perkalian: max(lebar) — kontras dengan pangkat di atas.
  wire [31:0] m32 = op16 * op16;

  // Penambahan dengan literal unsized, lalu truncate ke 8 bit.
  wire [7:0]  t8  = op8 + 32'h1;

  // `always @*` bukan `always_comb`: iverilog 11 memberi warning
  // "constant selects in always_* processes are not currently supported"
  // yang bisa menutupi perbedaan marker asli.
  always @* begin
    pow_res   = 32'h0;
    mul_res   = 32'h0;
    add_res   = 32'h0;
    trunc_res = 8'h0;
    if (en) begin
      pow_res   = {24'h0, p8} ^ {16'h0, p16};
      mul_res   = m32;
      add_res   = t8;
      trunc_res = p8;
    end
  end
endmodule