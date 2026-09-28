// Seed 46: streaming operator {<<{}} / {>>{}} + queue dynamic array
module stream_queue #(
  parameter int N = 8
) (
  input  logic         clk,
  input  logic         rst_n,
  input  logic [N-1:0] par_in,
  input  logic         push,
  input  logic         pop,
  output logic [N-1:0] par_out
);
  // byte reverse via streaming (LSB-first packing)
  function automatic logic [N-1:0] rev_bytes(logic [N-1:0] v);
    return {<<8{v}};
  endfunction

  function automatic logic [N-1:0] rev_bits(logic [N-1:0] v);
    return {<<{v}};
  endfunction

  // queue sebagai FIFO model
  logic [N-1:0] q[$];
  logic [N-1:0] rev;

  assign rev     = rev_bytes(par_in);
  assign par_out = (q.size() > 0) ? q[0] : rev_bits(par_in);

  always_ff @(posedge clk or negedge rst_n) begin
    if (!rst_n) begin
      q.delete();
    end else begin
      if (push) begin
        q.push_back(rev);
        if (q.size() > 4) begin
          void'(q.pop_front());
        end
      end else if (pop && q.size() > 0) begin
        void'(q.pop_front());
      end
    end
  end
endmodule
