// Seed oop_class1: Class inheritance + rand/randc + constraint
// Bug area: class field + constraint evaluation correctness

class base_class;
    rand bit [7:0] data;
    randc logic [3:0] magic;
    constraint c_data { data <= 100; }
    constraint c_magic { magic inside {0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15}; }
endclass

class derived_class extends base_class;
    rand bit en;
    constraint c_en { en -> data > 50; }
endclass

module test_oop_class;
    derived_class obj;
    initial begin
        obj = new();
        repeat (10) begin
            void st = obj.randomize();
            $display("data=%0d magic=%0d en=%0d", obj.data, obj.magic, obj.en);
        end
    end
endmodule