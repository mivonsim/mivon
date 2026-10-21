//! ELAB-09: `defparam` (IEEE 1800-2017 §23.11, warisan Verilog-2001).
//!
//! Dukungan penuh: lexer `defparam` keyword → parser (compilation-unit scope
//! + module body, path hierarki dgn indeks array `u_a[0]`) → AST
//! `ModuleItem::DefParam` → collect di `elaborate()` → apply saat flatten
//! (merge ke `param_map` SEBELUM signature → child re-elaborasi dgn override).
//!
//! Golden perilaku: iverilog `-g2012` (defparam menimpa override `#()`,
//! menarget instance path per-individu termasuk elemen instance array).

use super::*;

fn sim(source: &str, max_time: u64) -> Vec<(String, mivon_ir::LogicVec)> {
    simulate_signals(source, max_time)
        .unwrap_or_else(|e| panic!("sim gagal: {e}\n{source}"))
}

fn get_u64(sigs: &[(String, mivon_ir::LogicVec)], name: &str) -> u64 {
    sigs.iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("signal tidak ada: {name}"))
        .1
        .to_u64()
}

/// Modul child: param W mengontrol nilai output — observasi langsung.
const CHILD: &str = r#"
module child #(parameter W = 4);
  logic [15:0] val;
  assign val = W;
endmodule
"#;

/// Unit-scope: `defparam top.u1.W = 8;` — hanya u1 yang berubah, u0 default.
#[test]
fn defparam_unit_scope_overrides_one_instance() {
    let source = format!(
        "{CHILD}
module top;
  child u0();
  child u1();
  defparam top.u1.W = 8;
  initial #1 $finish;
endmodule
"
    );
    let sigs = sim(&source, 5);
    assert_eq!(get_u64(&sigs, "u0.val"), 4, "u0 tetap default W=4");
    assert_eq!(get_u64(&sigs, "u1.val"), 8, "u1 di-override defparam W=8");
}

/// Module body: `defparam u1.W = 8;` relatif terhadap scope module top.
#[test]
fn defparam_module_body_relative_path() {
    let source = format!(
        "{CHILD}
module top;
  child u0();
  child u1();
  defparam u1.W = 9;
  initial #1 $finish;
endmodule
"
    );
    let sigs = sim(&source, 5);
    assert_eq!(get_u64(&sigs, "u0.val"), 4, "u0 tetap default");
    assert_eq!(get_u64(&sigs, "u1.val"), 9, "module-body defparam relatif");
}

/// Instance array: `defparam top.u_a[1].W = 8;` — hanya ELEMEN [1] yang
/// berubah; [0] tetap default (per-instance path, ELAB-09 inti).
#[test]
fn defparam_instance_array_per_element() {
    let source = format!(
        "{CHILD}
module top;
  child u_a [1:0]();
  defparam top.u_a[1].W = 8;
  defparam top.u_a[0].W = 6;
  initial #1 $finish;
endmodule
"
    );
    let sigs = sim(&source, 5);
    assert_eq!(get_u64(&sigs, "u_a[0].val"), 6, "elemen [0] W=6");
    assert_eq!(get_u64(&sigs, "u_a[1].val"), 8, "elemen [1] W=8");
}

/// Path nested: unit-scope `top.u_sub.u_grand.W` — defparam menembus
/// lebih dari satu level instance.
#[test]
fn defparam_nested_instance_path() {
    let source = format!(
        "{CHILD}
module mid;
  child u_grand();
endmodule
module top;
  mid u_sub();
  child u_flat();
  defparam top.u_sub.u_grand.W = 12;
  initial #1 $finish;
endmodule
"
    );
    let sigs = sim(&source, 5);
    assert_eq!(get_u64(&sigs, "u_flat.val"), 4, "sibling tak terpengaruh");
    assert_eq!(
        get_u64(&sigs, "u_sub.u_grand.val"),
        12,
        "nested path tembus 2 level"
    );
}

/// Nilai non-trivial: ekspresi konstan `4 + 4` di-const-eval saat flatten.
#[test]
fn defparam_value_const_expression() {
    let source = format!(
        "{CHILD}
module top;
  child u0();
  defparam top.u0.W = 4 + 4;
  initial #1 $finish;
endmodule
"
    );
    let sigs = sim(&source, 5);
    assert_eq!(get_u64(&sigs, "u0.val"), 8, "const-eval 4+4 = 8");
}

/// Defparam menimpa override `#()` instance (LRM: defparam override terpisah
/// dari port/param binding; iverilog memberi prioritas defparam).
#[test]
fn defparam_wins_over_hash_override() {
    let source = format!(
        "{CHILD}
module top;
  child u0 #(.W(3))();
  defparam top.u0.W = 7;
  initial #1 $finish;
endmodule
"
    );
    let sigs = sim(&source, 5);
    assert_eq!(get_u64(&sigs, "u0.val"), 7, "defparam menang atas #()");
}

/// Target tak ada / param tak ada → WARNING (bukan error): compile tetap Ok.
#[test]
fn defparam_bad_target_is_warning_not_error() {
    let source = format!(
        "{CHILD}
module top;
  child u0();
  defparam top.nope.W = 8;
  defparam top.u0.MISSING = 8;
  initial #1 $finish;
endmodule
"
    );
    // Tidak panic: kedua defparam bermasalah hanya warning.
    let sigs = sim(&source, 5);
    assert_eq!(get_u64(&sigs, "u0.val"), 4, "u0 tetap default (tak ter-override)");
}

/// Defparam di file setelah endmodule (compilation-unit scope, second pass).
#[test]
fn defparam_after_endmodule() {
    let source = format!(
        "{CHILD}
module top;
  child u0();
  initial #1 $finish;
endmodule
defparam top.u0.W = 10;
"
    );
    let sigs = sim(&source, 5);
    assert_eq!(get_u64(&sigs, "u0.val"), 10, "defparam setelah endmodule");
}
