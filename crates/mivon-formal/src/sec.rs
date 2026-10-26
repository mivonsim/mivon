//! COMP-16 tahap 1 — Sequential Equivalence Checking (kombinasional).
//!
//! Membandingkan dua netlist kombinasional yang diberikan sebagai ekspresi
//! bit-vector atas input bernama. Miter dibangun dengan variabel Z3 yang SAMA
//! untuk input bernama sama, lalu setiap output dibandingkan: bila ada model
//! yang membuat output berbeda, desain TIDAK ekuivalen dan model itu menjadi
//! counterexample. Semua output unsat = ekuivalen.
//!
//! Batasan tahap ini (jujur): hanya kombinasional (tanpa state/clock/register),
//! lebar efektif <= 64 bit, lebar ekspresi = natural (maksimum operand; konstanta
//! 64 bit) lalu disesuaikan ke lebar deklarasi output — BELUM context width SV,
//! himpunan input & output kedua desain harus identik,
//! dan operator terbatas pada yang ada di [`SecExpr`]. Lowering dari IR/netlist
//! nyata serta SEC sekuensial (unrolling) adalah tahap berikutnya — item
//! COMP-16 tetap PARTIAL sampai itu ada.

use std::collections::HashMap;

/// Ekspresi kombinasional untuk SEC.
#[derive(Debug, Clone, PartialEq)]
pub enum SecExpr {
    /// Input bernama (lebar diambil dari [`SecDesign::inputs`]).
    Input(String),
    /// Konstanta (lebar 64 bit, nol-ekstensi saat digabung).
    Const(u64),
    Not(Box<SecExpr>),
    And(Box<SecExpr>, Box<SecExpr>),
    Or(Box<SecExpr>, Box<SecExpr>),
    Xor(Box<SecExpr>, Box<SecExpr>),
    Add(Box<SecExpr>, Box<SecExpr>),
    Sub(Box<SecExpr>, Box<SecExpr>),
    Mul(Box<SecExpr>, Box<SecExpr>),
    /// Perbandingan kesamaan, hasil 1 bit.
    Eq(Box<SecExpr>, Box<SecExpr>),
    /// `Mux(sel, then, else)` — sel != 0 memilih `then`.
    Mux(Box<SecExpr>, Box<SecExpr>, Box<SecExpr>),
}

/// Satu desain kombinasional: daftar input (nama, lebar) dan output (nama,
/// ekspresi).
#[derive(Debug, Clone, Default)]
pub struct SecDesign {
    pub inputs: Vec<(String, u32)>,
    /// (nama, lebar, ekspresi). Lebar dideklarasikan supaya literal/konstanta
    /// tidak memaksa lebar 64 dan perbandingan tetap SV-faithful.
    pub outputs: Vec<(String, u32, SecExpr)>,
}

/// Hasil pemeriksaan ekuivalensi.
#[derive(Debug, Clone, PartialEq)]
pub enum SecOutcome {
    /// Tidak ada input yang membedakan output mana pun.
    Equivalent,
    /// Ada input yang membedakan `output`; nilai input ada di counterexample.
    NotEquivalent {
        output: String,
        counterexample: Vec<(String, u64)>,
    },
    /// Di luar cakupan tahap ini (input/output tidak cocok, simbol tak
    /// dikenal, lebar > 64, dsb).
    Unsupported(String),
}

/// Periksa ekuivalensi kombinasional dua desain.
pub fn check_equivalence(a: &SecDesign, b: &SecDesign) -> SecOutcome {
    // 1. Himpunan input harus identik (nama + lebar, tanpa duplikat).
    let widths = match input_widths(a) {
        Ok(w) => w,
        Err(e) => return SecOutcome::Unsupported(e),
    };
    let widths_b = match input_widths(b) {
        Ok(w) => w,
        Err(e) => return SecOutcome::Unsupported(e),
    };
    if widths != widths_b {
        return SecOutcome::Unsupported("himpunan/lebar input kedua desain berbeda".to_string());
    }

    // 2. Himpunan output harus identik (urutan bebas).
    let mut out_names_a: Vec<&str> = a.outputs.iter().map(|(n, _, _)| n.as_str()).collect();
    let mut out_names_b: Vec<&str> = b.outputs.iter().map(|(n, _, _)| n.as_str()).collect();
    out_names_a.sort_unstable();
    out_names_b.sort_unstable();
    if out_names_a != out_names_b {
        return SecOutcome::Unsupported("himpunan output kedua desain berbeda".to_string());
    }
    if out_names_a.windows(2).any(|w| w[0] == w[1]) {
        return SecOutcome::Unsupported("nama output duplikat".to_string());
    }

    // 3. Variabel Z3 dibagi antar desain (nama input sama = variabel sama).
    let mut env: HashMap<&str, z3::ast::BV> = HashMap::new();
    for (name, width) in &a.inputs {
        env.insert(
            name.as_str(),
            z3::ast::BV::new_const(format!("sec_{name}"), *width),
        );
    }

    // Desain tanpa output: ekuivalen secara vacuous (tidak ada yang dibandingkan).
    if a.outputs.is_empty() {
        return SecOutcome::Equivalent;
    }

    let solver = z3::Solver::new();
    // Batas waktu supaya instance sulit tidak menggantung pemanggil.
    let mut params = z3::Params::new();
    params.set_u32("timeout", 10_000);
    solver.set_params(&params);

    // Miter = DISJUNGSI dari semua ketidaksamaan output. Assert per-output
    // (konjungsi) salah: dua output yang masing-masing berbeda pada kondisi
    // BERBEDA akan tampak ekuivalen karena tidak ada model yang membuat
    // keduanya berbeda sekaligus.
    let mut diffs: Vec<z3::ast::Bool> = Vec::new();
    for (name, width, expr_a) in &a.outputs {
        // Himpunan output sudah divalidasi identik, jadi pasangan selalu ada.
        let Some((_, width_b, expr_b)) = b.outputs.iter().find(|(n, _, _)| n == name) else {
            return SecOutcome::Unsupported(format!("output {name} tidak ada di desain B"));
        };
        if width != width_b {
            return SecOutcome::Unsupported(format!(
                "output {name}: lebar deklarasi berbeda ({width} vs {width_b})"
            ));
        }
        let va = match eval(expr_a, &env, &widths).and_then(|(v, w)| fit_width(v, w, *width)) {
            Ok(v) => v,
            Err(e) => return SecOutcome::Unsupported(format!("output {name}: {e}")),
        };
        let vb = match eval(expr_b, &env, &widths).and_then(|(v, w)| fit_width(v, w, *width)) {
            Ok(v) => v,
            Err(e) => return SecOutcome::Unsupported(format!("output {name}: {e}")),
        };
        diffs.push(va.eq(&vb).not());
    }
    let diff_refs: Vec<&z3::ast::Bool> = diffs.iter().collect();
    solver.assert(&z3::ast::Bool::or(&diff_refs));

    match solver.check() {
        z3::SatResult::Unsat => SecOutcome::Equivalent,
        z3::SatResult::Unknown => {
            SecOutcome::Unsupported("solver inconclusive/timeout".to_string())
        }
        z3::SatResult::Sat => {
            let Some(model) = solver.get_model() else {
                return SecOutcome::Unsupported("model tidak tersedia".to_string());
            };
            // Nilai input dari model; jangan fabrikasi 0 bila evaluasi gagal.
            let mut counterexample = Vec::new();
            for (name, _) in &a.inputs {
                let var = env.get(name.as_str()).expect("input terdaftar");
                let Some(value) = model.eval(var, true).and_then(|v| v.as_u64()) else {
                    return SecOutcome::Unsupported(format!("model tidak memberi nilai {name}"));
                };
                counterexample.push((name.clone(), value));
            }
            // Output mana yang berbeda di bawah model ini (disjungsi di atas
            // menjamin minimal satu ada).
            let mut failing = None;
            for (name, width, expr_a) in &a.outputs {
                let (_, _, expr_b) = b
                    .outputs
                    .iter()
                    .find(|(n, _, _)| n == name)
                    .expect("himpunan output identik");
                let va = eval(expr_a, &env, &widths)
                    .and_then(|(v, w)| fit_width(v, w, *width))
                    .expect("sudah divalidasi");
                let vb = eval(expr_b, &env, &widths)
                    .and_then(|(v, w)| fit_width(v, w, *width))
                    .expect("sudah divalidasi");
                let Some(av) = model.eval(&va, true).and_then(|v| v.as_u64()) else {
                    return SecOutcome::Unsupported(format!("model tidak mengevaluasi {name}"));
                };
                let Some(bv) = model.eval(&vb, true).and_then(|v| v.as_u64()) else {
                    return SecOutcome::Unsupported(format!("model tidak mengevaluasi {name}"));
                };
                if av != bv {
                    failing = Some(name.clone());
                    break;
                }
            }
            let Some(output) = failing else {
                return SecOutcome::Unsupported(
                    "solver sat tetapi tidak ada output yang berbeda".to_string(),
                );
            };
            SecOutcome::NotEquivalent {
                output,
                counterexample,
            }
        }
    }
}

fn input_widths(d: &SecDesign) -> Result<HashMap<String, u32>, String> {
    let mut widths = HashMap::new();
    for (name, width) in &d.inputs {
        if *width == 0 || *width > 64 {
            return Err(format!(
                "input {name}: lebar {width} tidak didukung (1..=64)"
            ));
        }
        if widths.insert(name.clone(), *width).is_some() {
            return Err(format!("input duplikat: {name}"));
        }
    }
    Ok(widths)
}

/// Evaluasi ekspresi menjadi BV Z3 + lebarnya.
fn eval(
    expr: &SecExpr,
    env: &HashMap<&str, z3::ast::BV>,
    widths: &HashMap<String, u32>,
) -> Result<(z3::ast::BV, u32), String> {
    match expr {
        SecExpr::Input(name) => {
            let var = env
                .get(name.as_str())
                .ok_or_else(|| format!("input tidak dikenal: {name}"))?;
            let width = *widths
                .get(name)
                .ok_or_else(|| format!("lebar input hilang: {name}"))?;
            Ok((var.clone(), width))
        }
        SecExpr::Const(v) => Ok((z3::ast::BV::from_u64(*v, 64), 64)),
        SecExpr::Not(inner) => {
            let (v, w) = eval(inner, env, widths)?;
            Ok((v.bvnot(), w))
        }
        SecExpr::And(l, r) | SecExpr::Or(l, r) | SecExpr::Xor(l, r) => {
            let (lv, lw) = eval(l, env, widths)?;
            let (rv, rw) = eval(r, env, widths)?;
            let (lv, rv, w) = widen(lv, lw, rv, rw);
            let out = match expr {
                SecExpr::And(..) => lv.bvand(&rv),
                SecExpr::Or(..) => lv.bvor(&rv),
                _ => lv.bvxor(&rv),
            };
            Ok((out, w))
        }
        SecExpr::Add(l, r) | SecExpr::Sub(l, r) | SecExpr::Mul(l, r) => {
            let (lv, lw) = eval(l, env, widths)?;
            let (rv, rw) = eval(r, env, widths)?;
            let (lv, rv, w) = widen(lv, lw, rv, rw);
            let out = match expr {
                SecExpr::Add(..) => lv.bvadd(&rv),
                SecExpr::Sub(..) => lv.bvsub(&rv),
                _ => lv.bvmul(&rv),
            };
            Ok((out, w))
        }
        SecExpr::Eq(l, r) => {
            let (lv, lw) = eval(l, env, widths)?;
            let (rv, rw) = eval(r, env, widths)?;
            let (lv, rv, _) = widen(lv, lw, rv, rw);
            let one = z3::ast::BV::from_u64(1, 1);
            let zero = z3::ast::BV::from_u64(0, 1);
            Ok((lv.eq(&rv).ite(&one, &zero), 1))
        }
        SecExpr::Mux(sel, t, f) => {
            let (sv, sw) = eval(sel, env, widths)?;
            let (tv, tw) = eval(t, env, widths)?;
            let (fv, fw) = eval(f, env, widths)?;
            let zero = z3::ast::BV::from_u64(0, sw);
            let cond = sv.eq(&zero).not();
            let (tv, fv, w) = widen(tv, tw, fv, fw);
            Ok((cond.ite(&tv, &fv), w))
        }
    }
}

/// Samakan lebar dua BV dengan nol-ekstensi (lebar hasil = maksimum).
fn widen(lv: z3::ast::BV, lw: u32, rv: z3::ast::BV, rw: u32) -> (z3::ast::BV, z3::ast::BV, u32) {
    let w = lw.max(rw);
    let lv = if lw < w { lv.zero_ext(w - lw) } else { lv };
    let rv = if rw < w { rv.zero_ext(w - rw) } else { rv };
    (lv, rv, w)
}

/// Sesuaikan ekspresi ke lebar deklarasi output: lebih sempit -> nol-ekstensi,
/// lebih lebar -> truncate. Lebar ekspresi di sini adalah lebar NATURAL
/// (maksimum operand, konstanta 64 bit), BUKAN context width SV — jadi
/// `Add(x8, y8)` yang dideklarasikan 16 bit tidak mendapat carry ke bit 8
/// seperti di SV. Lihat batasan di doc modul.
fn fit_width(v: z3::ast::BV, w: u32, declared: u32) -> Result<z3::ast::BV, String> {
    if declared == 0 || declared > 64 {
        return Err(format!("lebar output {declared} tidak didukung (1..=64)"));
    }
    if w == declared {
        Ok(v)
    } else if w < declared {
        Ok(v.zero_ext(declared - w))
    } else {
        Ok(v.extract(declared - 1, 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(name: &str) -> SecExpr {
        SecExpr::Input(name.to_string())
    }

    fn design(inputs: &[(&str, u32)], output: &str, width: u32, expr: SecExpr) -> SecDesign {
        SecDesign {
            inputs: inputs.iter().map(|(n, w)| (n.to_string(), *w)).collect(),
            outputs: vec![(output.to_string(), width, expr)],
        }
    }

    fn and(l: SecExpr, r: SecExpr) -> SecExpr {
        SecExpr::And(Box::new(l), Box::new(r))
    }
    fn or(l: SecExpr, r: SecExpr) -> SecExpr {
        SecExpr::Or(Box::new(l), Box::new(r))
    }

    #[test]
    fn distributive_forms_are_equivalent() {
        // (a & b) | (a & c)  ==  a & (b | c)
        let lhs = or(and(input("a"), input("b")), and(input("a"), input("c")));
        let rhs = and(input("a"), or(input("b"), input("c")));
        let a = design(&[("a", 1), ("b", 1), ("c", 1)], "y", 1, lhs);
        let b = design(&[("a", 1), ("b", 1), ("c", 1)], "y", 1, rhs);
        assert_eq!(check_equivalence(&a, &b), SecOutcome::Equivalent);
    }

    #[test]
    fn addition_is_commutative_for_8_bits() {
        let a = design(
            &[("x", 8), ("y", 8)],
            "s",
            8,
            SecExpr::Add(Box::new(input("x")), Box::new(input("y"))),
        );
        let b = design(
            &[("x", 8), ("y", 8)],
            "s",
            8,
            SecExpr::Add(Box::new(input("y")), Box::new(input("x"))),
        );
        assert_eq!(check_equivalence(&a, &b), SecOutcome::Equivalent);
    }

    #[test]
    fn mux_matches_boolean_expansion() {
        // Mux(s, t, f) == (s & t) | (~s & f) untuk 1 bit.
        let lhs = SecExpr::Mux(
            Box::new(input("s")),
            Box::new(input("t")),
            Box::new(input("f")),
        );
        let rhs = or(
            and(input("s"), input("t")),
            and(SecExpr::Not(Box::new(input("s"))), input("f")),
        );
        let a = design(&[("s", 1), ("t", 1), ("f", 1)], "y", 1, lhs);
        let b = design(&[("s", 1), ("t", 1), ("f", 1)], "y", 1, rhs);
        assert_eq!(check_equivalence(&a, &b), SecOutcome::Equivalent);
    }

    #[test]
    fn detects_non_equivalence_and_reports_counterexample() {
        // a & b  !=  a | b — model harus memberi a/b yang berbeda.
        let a = design(&[("a", 1), ("b", 1)], "y", 1, and(input("a"), input("b")));
        let b = design(&[("a", 1), ("b", 1)], "y", 1, or(input("a"), input("b")));
        match check_equivalence(&a, &b) {
            SecOutcome::NotEquivalent {
                output,
                counterexample,
            } => {
                assert_eq!(output, "y");
                let values: HashMap<&str, u64> = counterexample
                    .iter()
                    .map(|(n, v)| (n.as_str(), *v))
                    .collect();
                assert_eq!(counterexample.len(), 2, "semua input dilaporkan");
                let av = values["a"];
                let bv = values["b"];
                assert!(av <= 1 && bv <= 1, "nilai 1 bit: {counterexample:?}");
                assert_ne!(av & bv, av | bv, "counterexample harus membedakan output");
            }
            other => panic!("harus NotEquivalent, dapat {other:?}"),
        }
    }

    #[test]
    fn input_width_mismatch_is_unsupported() {
        // 4-bit vs 8-bit adder tidak boleh dianggap ekuivalen.
        let a = design(
            &[("x", 4), ("y", 4)],
            "s",
            8,
            SecExpr::Add(Box::new(input("x")), Box::new(input("y"))),
        );
        let b = design(
            &[("x", 8), ("y", 8)],
            "s",
            8,
            SecExpr::Add(Box::new(input("x")), Box::new(input("y"))),
        );
        match check_equivalence(&a, &b) {
            SecOutcome::Unsupported(msg) => assert!(msg.contains("input"), "{msg}"),
            other => panic!("harus Unsupported, dapat {other:?}"),
        }
    }

    #[test]
    fn output_set_mismatch_is_unsupported() {
        let a = design(&[("a", 1)], "y", 1, input("a"));
        let b = design(&[("a", 1)], "z", 1, input("a"));
        match check_equivalence(&a, &b) {
            SecOutcome::Unsupported(msg) => assert!(msg.contains("output"), "{msg}"),
            other => panic!("harus Unsupported, dapat {other:?}"),
        }
    }

    #[test]
    fn unknown_input_is_unsupported() {
        let a = design(&[("a", 1)], "y", 1, input("ghost"));
        let b = design(&[("a", 1)], "y", 1, input("a"));
        match check_equivalence(&a, &b) {
            SecOutcome::Unsupported(msg) => assert!(msg.contains("tidak dikenal"), "{msg}"),
            other => panic!("harus Unsupported, dapat {other:?}"),
        }
    }

    #[test]
    fn duplicate_input_is_unsupported() {
        let a = SecDesign {
            inputs: vec![("a".to_string(), 1), ("a".to_string(), 2)],
            outputs: vec![("y".to_string(), 1, input("a"))],
        };
        let b = design(&[("a", 1)], "y", 1, input("a"));
        match check_equivalence(&a, &b) {
            SecOutcome::Unsupported(msg) => assert!(msg.contains("duplikat"), "{msg}"),
            other => panic!("harus Unsupported, dapat {other:?}"),
        }
    }

    #[test]
    fn mismatch_in_only_one_output_is_detected() {
        // Miter WAJIB disjungsi: y1 beda hanya saat a=1, y2 selalu sama.
        // Assert per-output (konjungsi) akan salah menyimpulkan Equivalent.
        let a = SecDesign {
            inputs: vec![("a".to_string(), 1)],
            outputs: vec![
                ("y1".to_string(), 1, input("a")),
                ("y2".to_string(), 1, SecExpr::Const(0)),
            ],
        };
        let b = SecDesign {
            inputs: vec![("a".to_string(), 1)],
            outputs: vec![
                ("y1".to_string(), 1, SecExpr::Const(0)),
                ("y2".to_string(), 1, SecExpr::Const(0)),
            ],
        };
        match check_equivalence(&a, &b) {
            SecOutcome::NotEquivalent {
                output,
                counterexample,
            } => {
                assert_eq!(output, "y1");
                assert_eq!(counterexample, vec![("a".to_string(), 1)]);
            }
            other => panic!("harus NotEquivalent pada y1, dapat {other:?}"),
        }
    }

    #[test]
    fn empty_output_designs_are_equivalent() {
        // Tidak ada output = tidak ada yang dibandingkan (vacuous), bukan panic.
        let a = SecDesign {
            inputs: vec![("a".to_string(), 1)],
            outputs: Vec::new(),
        };
        let b = a.clone();
        assert_eq!(check_equivalence(&a, &b), SecOutcome::Equivalent);
    }

    #[test]
    fn comparison_with_constant_is_equivalent() {
        // Eq(x, 3) == Eq(3, x) untuk x 8 bit.
        let lhs = SecExpr::Eq(Box::new(input("x")), Box::new(SecExpr::Const(3)));
        let rhs = SecExpr::Eq(Box::new(SecExpr::Const(3)), Box::new(input("x")));
        let a = design(&[("x", 8)], "hit", 1, lhs);
        let b = design(&[("x", 8)], "hit", 1, rhs);
        assert_eq!(check_equivalence(&a, &b), SecOutcome::Equivalent);
    }

    #[test]
    fn add_is_not_and() {
        // Mengunci semantik operator: Add tidak boleh diperlakukan sebagai And.
        let a = design(
            &[("x", 8), ("y", 8)],
            "s",
            8,
            SecExpr::Add(Box::new(input("x")), Box::new(input("y"))),
        );
        let b = design(
            &[("x", 8), ("y", 8)],
            "s",
            8,
            SecExpr::And(Box::new(input("x")), Box::new(input("y"))),
        );
        assert!(matches!(
            check_equivalence(&a, &b),
            SecOutcome::NotEquivalent { .. }
        ));
    }

    #[test]
    fn eq_is_not_constant_true() {
        // Mengunci Eq: hasilnya bergantung pada input, bukan konstanta 1.
        let a = design(
            &[("x", 8)],
            "hit",
            1,
            SecExpr::Eq(Box::new(input("x")), Box::new(SecExpr::Const(3))),
        );
        let b = design(&[("x", 8)], "hit", 1, SecExpr::Const(1));
        assert!(matches!(
            check_equivalence(&a, &b),
            SecOutcome::NotEquivalent { .. }
        ));
    }

    #[test]
    fn output_width_mismatch_is_unsupported() {
        let a = design(&[("x", 8)], "y", 8, input("x"));
        let b = design(&[("x", 8)], "y", 4, input("x"));
        match check_equivalence(&a, &b) {
            SecOutcome::Unsupported(msg) => assert!(msg.contains("lebar deklarasi"), "{msg}"),
            other => panic!("harus Unsupported, dapat {other:?}"),
        }
    }
}
