// Seed generate_genvar1: Generate blocks + dist + with + for loop
// Bug area: generate expansion correctness, dist evaluation

module gen_example #(parameter int N = 5);
    bit [31:0] mem[0:N-1];
    
    initial begin
        // distribute values secara acak
        for (int i = 0; i < N; i++) begin
            mem[i] = $random;
        end
        $display("mem[0]=%0d", mem[0]);
    end
    
    genvar idx;
    generate
        for (idx = 0; idx < N; idx++) begin : reg_block
            assign sum = mem[idx] + mem[(idx + 1) % N];
        end
    endgenerate
endmodule

module driver_gen(
    input bit clk,
    output bit [31:0] a,
    output bit [31:0] b
);
    initial begin
        a = 32'hDEAD_BEEF;
        b = 32'hC0FFEE;
        forever begin
            @(posedge clk);
            a = a + 1;
            b = b ^ 32'hCAFE_BABE;
        end
    end
endmodule

module compute_sum(
    input bit [31:0] a,
    input bit [31:0] b,
    output reg [31:0] sum
);
    always_comb sum = a + b;
endmodule