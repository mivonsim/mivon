//! Regresi: propagasi field struct-parameter melalui override instance.
//!
//! CVA6 mendeklarasikan `parameter cva6_cfg_t CVA6Cfg = config_pkg::cva6_cfg_empty`
//! lalu meneruskannya lintas hierarki (`#(.CVA6Cfg(CVA6Cfg))`); submodule memakai
//! `CVA6Cfg.VpnLen`/`CVA6Cfg.PtLevels` di bound range-select
//! (`lu_vaddr_i[12 + (CVA6Cfg.VpnLen / CVA6Cfg.PtLevels) - 1 : 16]` di
//! `cva6_tlb.sv`). Field struct-parameter yang dilewatkan sebagai override
//! **Ident** (`#(.CVA6Cfg(CVA6Cfg))`) HARUS di-flatten ke `param_vals` child
//! (sebagai `CVA6Cfg.VpnLen` dst.) agar member access `param.field` bisa
//! di-const-eval — sebelumnya hanya nama utuh yang disalin (placeholder 0),
//! sehingga `CVA6Cfg.VpnLen` tak ter-resolve → "dynamic range select not
//! supported" (E3001) / E3003 param-tidak-konstan.
//!
//! Test ini memverifikasi rantai override `.CVA6Cfg(cfg_top)` → `.CFG(CVA6Cfg)`
//! mempropagasikan field struct sehingga range-select di child berhasil
//! di-fold (tanpa error elaborasi).

use super::*;

#[test]
fn test_struct_param_override_field_propagation() {
    let src = r#"
typedef struct packed {
    logic [7:0] A;
    logic [7:0] B;
} cfg_t;

module child #(parameter cfg_t CFG = '{A:8'h3, B:8'h5});
    logic [31:0] x;
    wire [CFG.A-1:0] y;
    assign y = x[CFG.A-1:0];
    initial #10 $finish;
endmodule

module mid #(parameter cfg_t CVA6Cfg = '{A:8'h4, B:8'h6});
    child #(.CFG(CVA6Cfg)) c();
endmodule

module top;
    mid #(.CVA6Cfg(cfg_top)) t();
    parameter cfg_t cfg_top = '{A:8'h7, B:8'h8};
endmodule
"#;
    let result = simulate_signals(src, 20);
    assert!(
        result.is_ok(),
        "propagasi field struct-parameter via override Ident harus sukses tanpa \
         'dynamic range select not supported': {:?}",
        result.err()
    );
}

/// Sama seperti di atas namun dengan referensi struct-package sebagai default
/// param (`CVA6Cfg = config_pkg::cva6_cfg_empty`, pola persis cva6_mmu.sv).
/// Memastikan flattening default struct-package + propagasi override berlapis
/// tetap menghasilkan field yang ter-resolve di submodule.
#[test]
fn test_struct_param_pkg_default_override_chain() {
    let src = r#"
package pp;
  typedef struct packed { logic [7:0] A; logic [7:0] B; } cfg_t;
  localparam cfg_t CFG_EMPTY = '{A:8'h4, B:8'h6};
endpackage

module child #(parameter pp::cfg_t CFG = pp::CFG_EMPTY);
    logic [31:0] x;
    wire [CFG.A-1:0] y;
    assign y = x[CFG.A-1:0];
    initial #10 $finish;
endmodule

module mid #(parameter pp::cfg_t CVA6Cfg = pp::CFG_EMPTY);
    child #(.CFG(CVA6Cfg)) c();
endmodule

module top;
    mid #(.CVA6Cfg(cfg_top)) t();
    parameter pp::cfg_t cfg_top = '{A:8'h7, B:8'h8};
endmodule
"#;
    let result = simulate_signals(src, 20);
    assert!(
        result.is_ok(),
        "default struct-package + override chain harus sukses: {:?}",
        result.err()
    );
}

/// Override struct-LITERAL bercast (`#(.CFG(pp::cfg_t'{A:27,B:3}))`, pola
/// `CVA6Cfg` di cva6_tlb): `const_eval_with_params` melipat StructLit→0
/// sehingga key `CFG.A`/`CFG.B` hilang dan range-select ber-Div
/// (`x[12+(CFG.A/CFG.B)-1:16]`) gagal E3001 "dynamic range select not
/// supported" sesat. Sekarang tiap member Named di-flatten ke
/// `pname.field` saat override (elab mod.rs), tanpa error elaborasi.
#[test]
fn test_struct_param_cast_literal_override_flatten() {
    let src = r#"
package pp;
  typedef struct packed { int unsigned A; int unsigned B; } cfg_t;
endpackage

module child #(parameter pp::cfg_t CFG = pp::cfg_t'(0));
    logic [63:0] x;
    wire w;
    assign w = x[12 + (CFG.A / CFG.B) - 1 : 16] == 5'h0;
    initial #10 $finish;
endmodule

module top;
    child #(.CFG(pp::cfg_t'{A:27, B:3})) c();
endmodule
"#;
    let result = simulate_signals(src, 20);
    assert!(
        result.is_ok(),
        "override struct-literal bercast harus flatten field tanpa E3001: {:?}",
        result.err()
    );
}

/// FuncCall package murni di dimensi deklarasi (`avoid_neg(N-1)`, pola
/// `pmpcfg_t [avoid_neg(CVA6Cfg.NrPMPEntries-1):0]` di cva6.sv): evaluator
/// skalar menolak FuncCall ("cannot evaluate function call ... in constant
/// context") → E3003 ParamMismatch yang mematikan elaborasi module.
/// Sekarang lebar range di-resolve via evaluator-penuh (inline body fungsi)
/// sebagai fallback di `range_width_aware` (elab types.rs) — keterbatasan
/// "not supported" menjadi full support. Nilai override (`N=8` → `[7:0]`,
/// `v='1` → 255) membuktikan field dipakai, bukan default.
#[test]
fn test_struct_param_funcall_in_decl_dim() {
    let src = r#"
package mpkg;
  typedef struct packed { int unsigned N; int unsigned M; } cfg_t;
  function automatic int unsigned avoid_neg(int n);
    return (n < 0) ? 0 : n;
  endfunction
endpackage

module child #(parameter mpkg::cfg_t CFG = mpkg::cfg_t'(0))
  (output logic [avoid_neg(CFG.N-1):0] o);
    assign o = '1;
endmodule

module top;
    wire [7:0] vtop;
    child #(.CFG(mpkg::cfg_t'{N:8, M:2})) c(.o(vtop));
    initial #20 $finish;
endmodule
"#;
    let result = simulate_signals(src, 20);
    assert!(
        result.is_ok(),
        "FuncCall di dimensi deklarasi harus ter-resolve tanpa E3003: {:?}",
        result.err()
    );
    let sigs = result.unwrap();
    let v = sigs.iter().find(|(s, _)| s == "vtop").unwrap();
    assert_eq!(v.1.width, 8, "override N=8 -> port [7:0]");
    assert_eq!(v.1.to_u64(), 255, "override N=8 -> o='1 harus 255");
}
