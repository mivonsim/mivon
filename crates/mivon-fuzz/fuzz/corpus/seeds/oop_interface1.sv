// Seed oop_interface1: Interface + clocking + bind + connect
// Bug area: connection correctness, modport binding, clocking block

interface intf(input logic clk, input logic rst_n);
    logic [7:0] data_in;
    logic [7:0] data_out;
    
    clocking cb @(posedge clk) {
        default input #1p;
        output data_out;
    }
    
    modport dut (
        input clk, rst_n,
        input data_in,
        output data_out
    );
    
    assign data_out = data_in + 2; // op sederhana
endinterface

module device_intf(intf.if);
    always_ff @(posedge if.clk) begin
        if (!if.rst_n) if.data_out <= 8'h0;
        else          if.data_out <= if.data_in + 2;
    end
endmodule

module tb_intf;
    bit clk = 0, rst_n = 0;
    intf dut_intf (.clk(clk), .rst_n(rst_n));
    device_intf dev_intf (.if(dut_intf));
    
    initial begin
        $display("initial test: interface + clocking");
        #10;
        rst_n = 1;
        #50;
        for (int i = 0; i < 5; i++) begin
            dut_intf.data_in = i;
            #2;
            $display("i=%0d, data_out=%0d", i, dut_intf.data_out);
        end
    end
endmodule