//! FORMAL-13 tahap 2b: connectivity check LINTAS INSTANCE/HIERARKI.
//!
//! Verifikasi (ROUND 88): connectivity BFS di flattened top SUDAH
//! menembus hierarki tanpa perubahan analisis — flatten meng-ALIAS port
//! child ke signal parent (`sig_remap[child_sig] = parent_sig`) dan
//! meng-merge semua child signals (`inst.sig`) + child processes ke
//! `top` dengan id di-remap. Jadi jalur combinational lintas port = jalur
//! biasa di graf BFS top (port tak menambah node terpisah).
//!
//! Sisa gap terdokumentasi (bukan diuji lolos di sini): query dgn nama
//! hierarki DALAM (mis. `u0.u1.a`) belum resolve — `hier_signal_map`
//! berisi id id-space child yang tidak ikut di-remap saat merge (dan
//! key tanpa prefix penuh → ambigu utk instance berulang). Dukungan
//! alias full-path = tahap lanjut di elaborator (risiko tinggi).

use mivon_api::compile_str;
use mivon_api::formal::connectivity::check_connectivity;
use mivon_ir::IrDesign;

/// Desain berhierarki 2 level: top → mid(u0) → leaf(u1).
/// Port di-alias (bukan node terpisah) → jalur p → q = p → u0.w → q.
const HIER_SRC: &str = r#"
module leaf(input logic a, input logic [1:0] ctl, output logic y);
  always_comb begin
    case (ctl)
      2'b00: y = a;
      default: y = 1'b0;
    endcase
  end
endmodule

module mid(input logic s, output logic z);
  logic w;
  leaf u1(.a(s), .ctl(2'b00), .y(w));
  always_comb z = w;
endmodule

module top;
  logic p, q;
  mid u0(.s(p), .z(q));
endmodule
"#;

fn hier_design() -> IrDesign {
    compile_str(HIER_SRC).unwrap_or_else(|e| panic!("compile gagal: {e}"))
}

/// Jalur combinational menembus 2 level instance (top → u0 → u1 → u0 →
/// top): port = alias flatten, child processes = bagian graf top.
#[test]
fn connectivity_crosses_two_instance_levels() {
    let design = hier_design();
    let r = check_connectivity(&design, &[("p".into(), "q".into())]);
    assert!(r[0].error.is_none(), "error: {:?}", r[0].error);
    assert!(
        r[0].connected,
        "p → q harus menembus instance u0 + u1 (flatten alias port)"
    );
    assert_eq!(r[0].path_len, Some(2), "jalur p → u0.w → q = 2 hop");
    assert_eq!(
        r[0].path,
        vec!["p".to_string(), "u0.w".to_string(), "q".to_string()],
        "path rekonstruksi memakai nama flatten internal"
    );
}

/// Sinyal internal instance bisa di-query dgn nama flatten `u0.w`.
#[test]
fn connectivity_flat_internal_name_queryable() {
    let design = hier_design();
    let r = check_connectivity(&design, &[("p".into(), "u0.w".into())]);
    assert!(r[0].error.is_none(), "error: {:?}", r[0].error);
    assert!(r[0].connected, "p → u0.w lewat port alias u1.a/s");
    assert_eq!(r[0].path_len, Some(1));
}

/// Arah komposisional dihormati lintas instance: q TIDAK mencapai p
/// (tak ada jalur balik — port output bukan alias dua arah).
#[test]
fn connectivity_directional_across_instances() {
    let design = hier_design();
    let r = check_connectivity(&design, &[("q".into(), "p".into())]);
    assert!(r[0].error.is_none(), "error: {:?}", r[0].error);
    assert!(
        !r[0].connected,
        "q → p tidak boleh terhubung (combinational satu arah)"
    );
}

/// Instance dengan logika di dalamnya: jalur menembus instance DAN
/// logic parent sesudahnya (3 assignment berturut lintas boundary).
#[test]
fn connectivity_path_through_instance_then_parent_logic() {
    let src = r#"
module inner(input logic x, output logic y);
  logic t;
  always_comb t = x & x;
  always_comb y = t;
endmodule

module top;
  logic a, b, c;
  inner u_i(.x(a), .y(b));
  always_comb c = b | 1'b0;
endmodule
"#;
    let design = compile_str(src).expect("compile gagal");
    let r = check_connectivity(&design, &[("a".into(), "c".into())]);
    assert!(r[0].error.is_none(), "error: {:?}", r[0].error);
    assert!(
        r[0].connected,
        "a → (u_i: x→t→y) → b → c harus terhubung lintas instance"
    );
    // a → b (alias u_i.y) via t: 2 hop dalam instance; b → c: 1 hop parent.
    assert_eq!(r[0].path_len, Some(3), "path: {:?}", r[0].path);
    assert_eq!(r[0].path.first().map(|s| s.as_str()), Some("a"));
    assert_eq!(r[0].path.last().map(|s| s.as_str()), Some("c"));
}

/// Sinyal tak dikenal → error (bukan false "tidak terhubung").
#[test]
fn connectivity_unknown_signal_is_error() {
    let design = hier_design();
    let r = check_connectivity(&design, &[("p".into(), "no_such".into())]);
    assert!(
        r[0].error.as_deref().is_some_and(|e| e.contains("no_such")),
        "harusnya error sinyal tak dikenal, dapat {:?}",
        r[0].error
    );
    assert!(!r[0].connected);
}

/// Dua instance sibling — jalur melalui SATU instance tak bocor ke
/// instance lain (instance isolation pada flattened graph).
#[test]
fn connectivity_sibling_instances_isolated() {
    let src = r#"
module leaf(input logic a, output logic y);
  always_comb y = a;
endmodule

module top;
  // Catatan: `cross` adalah keyword SV (coverage) — jangan dipakai sbg nama.
  logic s1, r1, s2, r2, merged;
  leaf u_a(.a(s1), .y(r1));
  leaf u_b(.a(s2), .y(r2));
  always_comb merged = r1 & r2;
endmodule
"#;
    let design = compile_str(src).expect("compile gagal");
    let r = check_connectivity(
        &design,
        &[
            ("s1".into(), "merged".into()),
            ("s2".into(), "merged".into()),
            ("s1".into(), "r2".into()),
        ],
    );
    assert!(r[0].connected, "s1 → r1 → merged");
    assert!(r[1].connected, "s2 → r2 → merged");
    assert!(
        !r[2].connected,
        "s1 tak boleh mencapai r2 (instance berbeda, alias tak silang)"
    );
}
