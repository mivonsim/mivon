//! E2E phase 4 — tech mapping (SYNTHESIS.md §5/§12): netlist hasil mapping
//! LUT6/CARRY4/FF disimulasikan engine Mivon dan hasilnya DIBANDINGKAN
//! dengan simulasi RTL asli. Loop verifikasi tertutup (kriteria fase 3/4):
//! `sim netlist = sim RTL`.
//!
//! Flow per kasus: compile RTL → lower ke SIR → pass optimizer → tech_map
//! (LUT cut + AIG decomposition + carry chain) → emit `netlist.v` → sim
//! netlist (dengan testbench yang sama) → bandingkan signal dengan sim RTL.

use std::collections::HashMap;

/// Path absolut contoh RTL di `examples/synth/` (cwd test = crate root).
fn example(name: &str) -> String {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/synth");
    std::fs::read_to_string(std::path::Path::new(dir).join(name))
        .unwrap_or_else(|e| panic!("baca examples/synth/{name}: {e}"))
}

/// Pipeline lengkap RTL → netlist tech-mapped (untuk tes).
fn rtl_to_mapped_netlist(source: &str) -> String {
    let ir = mivon_api::compile_str(source).expect("compile RTL");
    let lower = mivon_sir::lower(&ir);
    assert!(
        lower.skipped.is_empty(),
        "SIR lowering tidak boleh skip: {:?}",
        lower.skipped
    );
    let mut pipeline = mivon_synth::SynthPipeline::with_preset("fpga").expect("preset");
    let (sir_opt, _results) = pipeline.run(lower.module).expect("optimizer SIR");
    let res = mivon_synth::tech_map(&sir_opt, &mivon_tech::FpgaX7Arch);
    assert!(
        res.skipped.is_empty(),
        "tech map tidak boleh skip: {:?}",
        res.skipped
    );
    let check = mivon_netlist::verify_dag(&res.netlist);
    assert!(check.ok, "netlist mapped harus DAG bersih: {:?}", check);
    mivon_netlist::emit_verilog(&res.netlist)
}

/// Hasil mapping langsung (tanpa emit) untuk cek utilisasi.
fn mapped_result(source: &str) -> mivon_synth::TechMapResult {
    let ir = mivon_api::compile_str(source).expect("compile RTL");
    let lower = mivon_sir::lower(&ir);
    let mut pipeline = mivon_synth::SynthPipeline::with_preset("fpga").unwrap();
    let (sir_opt, _) = pipeline.run(lower.module).unwrap();
    mivon_synth::tech_map(&sir_opt, &mivon_tech::FpgaX7Arch)
}

/// Signal final (u64) dari simulasi source (top module).
fn sim_final(source: &str, max_time: u64) -> HashMap<String, u64> {
    let sigs = mivon_api::simulate_signals(source, max_time).expect("simulasi");
    sigs.into_iter()
        .map(|(name, lv)| (name, lv.to_u64()))
        .collect()
}

/// Testbench counter 8-bit — instansiasi `counter`, drive clk/rst/enable,
/// `count` jadi signal top (bisa dibaca hasil sim).
const TB_COUNTER: &str = r#"
module tb_counter();
  logic clk = 0;
  logic rst_n = 0;
  logic enable = 0;
  logic [7:0] count;
  counter dut(.clk(clk), .rst_n(rst_n), .enable(enable), .count(count));
  always #5 clk = ~clk;
  initial begin
    #10 rst_n = 1;
    enable = 1;
    #300;
    $finish;
  end
endmodule
"#;

#[test]
fn tech_mapped_counter_sim_equals_rtl() {
    let rtl = example("counter.sv");
    let netlist_v = rtl_to_mapped_netlist(&rtl);

    let rtl_sim = sim_final(&format!("{rtl}\n{TB_COUNTER}"), 310);
    let net_sim = sim_final(&format!("{netlist_v}\n{TB_COUNTER}"), 310);

    let rtl_count = rtl_sim.get("count").expect("count di RTL");
    let net_count = net_sim.get("count").expect("count di netlist");
    assert_eq!(
        net_count, rtl_count,
        "count netlist != count RTL — netlist={net_count} rtl={rtl_count}"
    );
    assert_eq!(*rtl_count, 30, "30 hitungan setelah reset");
}

/// Testbench alu_opt (kombinasional) — drive a/b, baca y/z.
const TB_ALU: &str = r#"
module tb_alu();
  logic [7:0] a = 8'h35;
  logic [7:0] b = 8'h4A;
  logic [7:0] y, z;
  alu_opt dut(.a(a), .b(b), .y(y), .z(z));
  initial begin
    #10;
    $finish;
  end
endmodule
"#;

#[test]
fn tech_mapped_alu_opt_sim_equals_rtl() {
    let rtl = example("alu_opt.sv");
    let netlist_v = rtl_to_mapped_netlist(&rtl);

    let rtl_sim = sim_final(&format!("{rtl}\n{TB_ALU}"), 10);
    let net_sim = sim_final(&format!("{netlist_v}\n{TB_ALU}"), 10);

    // y = a|b = 0x35|0x4A = 0x7F; z = a + (b<<2) = 53 + 296 = 349 → 8-bit 93.
    let (ry, rz) = (rtl_sim["y"], rtl_sim["z"]);
    let (ny, nz) = (net_sim["y"], net_sim["z"]);
    assert_eq!(ny, ry, "y netlist != y RTL: {ny} vs {ry}");
    assert_eq!(nz, rz, "z netlist != z RTL: {nz} vs {rz}");
    assert_eq!(ry, 127, "y = a|b = 0x7F");
    assert_eq!(rz, 93, "z = a+(b<<2) = 0x5D");
}

/// LUT count sesuai ekspektasi (deliverable phase 4: `alu.sv → LUT count`).
#[test]
fn tech_mapped_alu_opt_lut_count() {
    let rtl = example("alu_opt.sv");
    let res = mapped_result(&rtl);
    // y = a|b → 8 LUT (OR per bit); z = a + (b<<2) → 2 CARRY4 (8-bit, ceil(8/4)).
    // SHL konstanta → wiring murni (tanpa LUT).
    assert_eq!(res.lut_count, 8, "a|b 8-bit → 8 LUT");
    assert_eq!(res.carry4_count, 2, "adder 8-bit → 2 CARRY4 slice");
    assert_eq!(res.ff_count, 0, "alu_opt tanpa FF");
}

/// Utilisasi counter: FF 8, CARRY4 2, EQ count==99 + mux per bit.
#[test]
fn tech_mapped_counter_lut_count() {
    let rtl = example("counter.sv");
    let res = mapped_result(&rtl);
    assert_eq!(res.ff_count, 8, "counter 8-bit → 8 FF bit");
    assert_eq!(res.carry4_count, 2, "count+1 8-bit → 2 CARRY4 slice");
    // Mapping per-bit (belum di-optimasi): EQ count==99 (4 LUT) + mux reset/
    // next (8) + enable (8) + reset async (8) = 28. Fungsional benar (netlist
    // sim == RTL sim), hanya bukan sintesis optimal.
    assert_eq!(res.lut_count, 28, "EQ(4) + next(8) + enable(8) + reset(8)");
}

/// Netlist hasil mapping deterministik: dua run → netlist identik.
#[test]
fn tech_mapped_netlist_is_deterministic() {
    let rtl = example("counter.sv");
    let a = rtl_to_mapped_netlist(&rtl);
    let b = rtl_to_mapped_netlist(&rtl);
    assert_eq!(a, b, "netlist mapped harus deterministik");
}

/// Netlist hasil mapping memuat modul sel (CARRY4 / LUT / FF reset).
#[test]
fn tech_mapped_netlist_verilog_has_cell_modules() {
    let rtl = example("counter.sv");
    let netlist_v = rtl_to_mapped_netlist(&rtl);
    assert!(netlist_v.contains("module CARRY4"), "modul CARRY4");
    assert!(netlist_v.contains("always_comb"), "LUT always_comb");
    assert!(netlist_v.contains("module DFFR_"), "FF reset");
}

/// Smoke: `mivon synth --emit-netlist --tech-map` lewat tools (file tertulis).
#[test]
fn synth_tool_emit_mapped_netlist_smoke() {
    let dir = std::env::temp_dir().join("mivon_synth_p4_smoke");
    std::fs::create_dir_all(&dir).ok();
    let prefix = dir.join("counter_p4");
    let counter_path = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/synth/counter.sv"
    ))
    .to_string_lossy()
    .to_string();
    let args = mivon_api::tools::synth::SynthArgs {
        targets: &[counter_path],
        incdirs: &[],
        defines: &[],
        top: Some("counter"),
        output: Some(prefix.to_string_lossy().to_string()),
        check_only: false,
        device: "generic".to_string(),
        preset: "fpga".to_string(),
        emit_mvnet: false,
        dump_sir: false,
        dump_sir_opt: false,
        dump_netlist: false,
        emit_netlist: true,
        tech_map: true,
        report_util: None,
        constraint: None,
        timing: false,
        fsm_report: false,
        quiet: true,
    };
    mivon_api::tools::synth::run(&args).expect("mivon synth --emit-netlist --tech-map");
    assert!(dir.join("counter_p4.tech.v").exists(), "tech netlist file");
    assert!(
        dir.join("counter_p4.tech.mvnet").exists(),
        "tech mvnet file"
    );
}

/// COMP-11 tahap 3: FSM di SUBMODULE ikut terdeteksi lewat elaborasi nyata
/// (`design.modules` diisi elaborator), bukan hanya top module.
#[test]
fn fsm_extraction_covers_submodule_instances() {
    let src = r#"
module sub_fsm(input logic clk, output logic [1:0] o, output logic [1:0] o2);
    logic [1:0] state;
    always_ff @(posedge clk) begin
        case (state)
            2'd0: state <= 2'd1;
            2'd1: state <= 2'd2;
            default: state <= 2'd0;
        endcase
    end
    // Proses ke-3 (output Moore) — tidak boleh mengganggu deteksi FSM.
    always_comb o2 = state + 2'd1;
    assign o = state;
endmodule

module top(input logic clk, output logic [1:0] o, output logic [1:0] o2);
    sub_fsm u(.clk(clk), .o(o), .o2(o2));
endmodule
"#;
    let ir = mivon_api::compile_str(src).expect("compile RTL hierarki");
    assert!(
        ir.modules
            .contains_key(&mivon_core::Symbol::from("sub_fsm")),
        "elaborator harus menyimpan modul sub_fsm"
    );
    let fsms = mivon_synth::fsm::extract_fsms(&ir);
    let names: Vec<&str> = fsms.iter().map(|f| f.process_name.as_str()).collect();
    assert!(
        names.iter().any(|n| n.starts_with("sub_fsm.")),
        "FSM submodule harus muncul dengan prefiks modul: {names:?}"
    );
    let f = fsms
        .iter()
        .find(|f| f.state_signal == "state")
        .expect("state register submodule");
    assert_eq!(f.states, vec![0, 1, 2], "{:?}", f.states);
    assert_eq!(f.state_width, 2);
}
