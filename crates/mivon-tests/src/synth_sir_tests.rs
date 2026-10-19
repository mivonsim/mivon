use super::*;

// REGRESI SYN-10: lowering SIR untuk elemen array (register file / RAM) dan
// deteksi reset sinkron yang tidak boleh salah mengenali enable.
//
// Sebelumnya:
//   - `rdata <= mem[addr]` → const 0 (ArrayIndex tidak di-lower, silent wrong)
//   - `mem[addr] <= wdata` → SELURUH sinyal mem tertimpa RHS (index diabaikan)
//   - `if (en) q <= d`     → dianggap reset sinkron (`rst=en`) di SIR
// Sekarang: read = SHR+MASK, write = read-modify-write (SHL/NOT/AND/OR),
// dan reset hanya dideteksi bila cabang true meng-assign konstanta.

fn ir_of(src: &str) -> mivon_ir::IrDesign {
    compile_str(src).expect("compile + elaborate")
}

const RF_SRC: &str = r#"
module rf(input clk, input [1:0] addr, input [7:0] wdata, input we,
          output reg [7:0] rdata);
    reg [7:0] mem [0:3];
    always @(posedge clk) if (we) mem[addr] <= wdata;
    always @(posedge clk) rdata <= mem[addr];
endmodule
"#;

#[test]
fn array_index_read_lowers_to_shift_mask() {
    let ir = ir_of(RF_SRC);
    let out = mivon_sir::lower::lower(&ir);
    assert!(
        out.skipped.is_empty(),
        "tidak boleh ada konstruk di-skip: {:?}",
        out.skipped
    );
    let kinds: Vec<String> = out
        .module
        .nodes
        .iter()
        .map(|n| format!("{:?}", n.kind))
        .collect();
    assert!(
        kinds.iter().any(|k| k == "Shr"),
        "read elemen array harus memakai SHR: {:?}",
        kinds
    );
    assert!(
        kinds.iter().any(|k| k == "And"),
        "read elemen array harus memakai mask AND: {:?}",
        kinds
    );
}

#[test]
fn array_element_write_is_read_modify_write() {
    let ir = ir_of(RF_SRC);
    let out = mivon_sir::lower::lower(&ir);
    let kinds: Vec<String> = out
        .module
        .nodes
        .iter()
        .map(|n| format!("{:?}", n.kind))
        .collect();
    for need in ["Shl", "Not", "And", "Or"] {
        assert!(
            kinds.iter().any(|k| k == need),
            "write elemen array harus memakai {}: {:?}",
            need,
            kinds
        );
    }
}

#[test]
fn enable_if_is_not_treated_as_sync_reset() {
    let ir = ir_of(RF_SRC);
    let out = mivon_sir::lower::lower(&ir);
    for reg in &out.module.registers {
        assert!(
            reg.reset.is_none(),
            "register '{}' tidak punya reset di source, tapi SIR menginfer {:?}",
            reg.name.as_str(),
            reg.reset
        );
    }
}

#[test]
fn sync_reset_still_detected() {
    let src = r#"
module cnt(input clk, input rst, input [7:0] d, output reg [7:0] q);
    always @(posedge clk) begin
        if (rst) q <= 8'h00;
        else q <= d;
    end
endmodule
"#;
    let ir = ir_of(src);
    let out = mivon_sir::lower::lower(&ir);
    let reg = out
        .module
        .registers
        .iter()
        .find(|r| r.name.as_str() == "q")
        .expect("register q");
    assert!(
        reg.reset.is_some(),
        "pola if (rst) q <= 0; else q <= d; harus terdeteksi reset sinkron"
    );
}

#[test]
fn inside_and_range_lower_to_comparators() {
    let src = r#"
module sel(input [7:0] a, output reg y);
    always @* y = (a inside {8'd1, 8'd5, [8'd10:8'd20]});
endmodule
"#;
    let ir = ir_of(src);
    let out = mivon_sir::lower::lower(&ir);
    assert!(
        out.skipped.is_empty(),
        "inside tidak boleh di-skip: {:?}",
        out.skipped
    );
    let kinds: Vec<String> = out
        .module
        .nodes
        .iter()
        .map(|n| format!("{:?}", n.kind))
        .collect();
    for need in ["Eq", "Ge", "Le", "And", "Or"] {
        assert!(
            kinds.iter().any(|k| k == need),
            "inside harus memakai {}: {:?}",
            need,
            kinds
        );
    }
}

#[test]
fn constant_power_lowers_to_multiply_chain() {
    let src = r#"
module pw(input [3:0] a, output reg [7:0] y);
    always @* y = a ** 2;
endmodule
"#;
    let ir = ir_of(src);
    let out = mivon_sir::lower::lower(&ir);
    assert!(
        out.skipped.is_empty(),
        "a**2 (eksponen konstan) harus di-lower: {:?}",
        out.skipped
    );
    let muls = out
        .module
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, mivon_sir::sir::SirNodeKind::Mul))
        .count();
    assert!(muls >= 2, "a**2 = a*a → minimal 2 MUL, dapat {muls}");
}

#[test]
fn case_inside_range_labels_lowered() {
    let src = r#"
module ci(input [7:0] a, output reg [1:0] y);
    always @* begin
        case (a) inside
            8'd1, [8'd10:8'd20]: y = 2'd1;
            [8'd30:8'd40]: y = 2'd2;
            default: y = 2'd0;
        endcase
    end
endmodule
"#;
    let ir = ir_of(src);
    let out = mivon_sir::lower::lower(&ir);
    assert!(
        out.skipped.is_empty(),
        "case inside tidak boleh di-skip: {:?}",
        out.skipped
    );
    let kinds: Vec<String> = out
        .module
        .nodes
        .iter()
        .map(|n| format!("{:?}", n.kind))
        .collect();
    assert!(
        kinds.iter().any(|k| k == "Ge") && kinds.iter().any(|k| k == "Le"),
        "label rentang harus jadi GE/LE: {:?}",
        kinds
    );
}

#[test]
fn countones_and_onehot_lower_to_adder_tree() {
    let src = r#"
module f(input [7:0] a, output reg [3:0] c, output reg oh, output reg oh0);
    always @* begin
        c   = $countones(a);
        oh  = $onehot(a);
        oh0 = $onehot0(a);
    end
endmodule
"#;
    let ir = ir_of(src);
    let out = mivon_sir::lower::lower(&ir);
    assert!(
        out.skipped.is_empty(),
        "countones/onehot tidak boleh di-skip: {:?}",
        out.skipped
    );
    let adds = out
        .module
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, mivon_sir::sir::SirNodeKind::Add))
        .count();
    assert!(adds >= 7, "$countones 8-bit → adder tree, dapat {adds} ADD");
}
