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
//! memakai lebar literal IR) lalu disesuaikan ke lebar deklarasi output — BELUM
//! context width SV,
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
    /// Konstanta beserta lebar operandnya (lebar literal IR).
    Const(u64, u32),
    Not(Box<SecExpr>),
    And(Box<SecExpr>, Box<SecExpr>),
    Or(Box<SecExpr>, Box<SecExpr>),
    Xor(Box<SecExpr>, Box<SecExpr>),
    Add(Box<SecExpr>, Box<SecExpr>),
    /// Negasi aritmetika dua-komplemen pada lebar operand (SV unary minus).
    Neg(Box<SecExpr>),
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

/// Lowering tahap 1 dari `IrExpr` ke [`SecExpr`] (kombinasional).
///
/// `sig_names` adalah tabel nama sinyal (index = `SignalId`) dari hasil
/// elaborasi; sinyal yang namanya tidak ada ditolak. Hanya subset aman yang
/// dipetakan: Const (tanpa X/Z, <= 64 bit, LEBARNYA ikut dibawa), Signal,
/// BitNot, Minus (jadi Neg, lebar operand dipertahankan), Add/Sub/Mul,
/// BitAnd/BitOr/BitXor, Eq/Neq/CaseEq/CaseNeq, LogicalAnd/LogicalOr
/// (dibandingkan ke nol dulu, seperti && / ||), dan Cond. Sisanya
/// (Div/Mod/Shift/Concat/select/ArrayIndex/SysFunc, Signed, Cast/context
/// width, Not logika, FillLit, konstanta X/Z) -> Err, supaya SEC tidak
/// diam-diam salah. Lebar
/// konstanta harus ikut karena `SecExpr` memakai lebar natural (maksimum
/// operand): konstanta 64 bit akan melebarkan seluruh ekspresi dan membuat
/// `~(a + 8'd1) == 8'hF0` selalu 0. Prasyarat: literal sudah dilebarkan ke
/// lebar konteks oleh elaborator; lebar konteks SV belum dimodelkan di sini.
///
/// Prasyarat (tidak bisa dicek dari signature): tidak ada sinyal `signed`
/// (sign-extension belum dimodelkan) dan lebar di `SecDesign::inputs` harus
/// sama dengan lebar sinyal di IR.
pub fn expr_from_ir(expr: &mivon_ir::IrExpr, sig_names: &[String]) -> Result<SecExpr, String> {
    validate_sig_names(sig_names)?;
    lower(expr, sig_names)
}

/// Tabel nama sinyal harus unik dan tidak memuat nama kosong; kalau tidak,
/// dua SignalId berbeda bisa diam-diam menjadi satu variabel Z3.
fn validate_sig_names(sig_names: &[String]) -> Result<(), String> {
    let mut seen = std::collections::HashSet::with_capacity(sig_names.len());
    for name in sig_names {
        if name.is_empty() {
            return Err("tabel nama sinyal memuat nama kosong".to_string());
        }
        if !seen.insert(name.as_str()) {
            return Err(format!("nama sinyal ganda di tabel: {name}"));
        }
    }
    Ok(())
}

fn lower(expr: &mivon_ir::IrExpr, sig_names: &[String]) -> Result<SecExpr, String> {
    use mivon_ir::{BinaryIrOp, IrExpr, UnaryIrOp};
    match expr {
        IrExpr::Const(lv) => {
            if lv.has_unknown() {
                return Err(format!("konstanta dengan X/Z tidak didukung: {lv:?}"));
            }
            if lv.width > 64 {
                return Err(format!("konstanta {} bit > 64 tidak didukung", lv.width));
            }
            Ok(SecExpr::Const(lv.to_u64(), lv.width as u32))
        }
        // FillLit butuh lebar konteks (belum dimodelkan) -> tolak, jangan tebak.
        IrExpr::FillLit(_) => Err("IrExpr FillLit butuh lebar konteks SEC".to_string()),
        IrExpr::Signal(id, _) => {
            let name = sig_names
                .get(*id)
                .ok_or_else(|| format!("SignalId {id} di luar tabel nama"))?;
            Ok(SecExpr::Input(name.clone()))
        }
        IrExpr::UnaryOp(op @ (UnaryIrOp::BitNot | UnaryIrOp::Minus), inner) => {
            let inner = Box::new(lower(inner, sig_names)?);
            Ok(if matches!(op, UnaryIrOp::BitNot) {
                SecExpr::Not(inner)
            } else {
                SecExpr::Neg(inner)
            })
        }
        IrExpr::UnaryOp(op, _) => Err(format!("UnaryIrOp {op:?} tidak didukung SEC")),
        IrExpr::BinaryOp(op, l, r) => {
            let l = Box::new(lower(l, sig_names)?);
            let r = Box::new(lower(r, sig_names)?);
            Ok(match op {
                BinaryIrOp::Add => SecExpr::Add(l, r),
                BinaryIrOp::Sub => SecExpr::Sub(l, r),
                BinaryIrOp::Mul => SecExpr::Mul(l, r),
                BinaryIrOp::BitAnd => SecExpr::And(l, r),
                BinaryIrOp::BitOr => SecExpr::Or(l, r),
                // Logika (&&/||) = bandingkan ke nol dulu, bukan bitwise.
                BinaryIrOp::LogicalAnd => {
                    let lz = SecExpr::Not(Box::new(SecExpr::Eq(l, Box::new(SecExpr::Const(0, 1)))));
                    let rz = SecExpr::Not(Box::new(SecExpr::Eq(r, Box::new(SecExpr::Const(0, 1)))));
                    SecExpr::And(Box::new(lz), Box::new(rz))
                }
                BinaryIrOp::LogicalOr => {
                    let lz = SecExpr::Not(Box::new(SecExpr::Eq(l, Box::new(SecExpr::Const(0, 1)))));
                    let rz = SecExpr::Not(Box::new(SecExpr::Eq(r, Box::new(SecExpr::Const(0, 1)))));
                    SecExpr::Or(Box::new(lz), Box::new(rz))
                }
                BinaryIrOp::BitXor => SecExpr::Xor(l, r),
                // SEC 2-state: X/Z ditolak, jadi CaseEq == Eq di sini.
                BinaryIrOp::Eq | BinaryIrOp::CaseEq => SecExpr::Eq(l, r),
                BinaryIrOp::Neq | BinaryIrOp::CaseNeq => SecExpr::Not(Box::new(SecExpr::Eq(l, r))),
                other => return Err(format!("BinaryIrOp {other:?} tidak didukung SEC")),
            })
        }
        IrExpr::Cond(c, t, f) => Ok(SecExpr::Mux(
            Box::new(lower(c, sig_names)?),
            Box::new(lower(t, sig_names)?),
            Box::new(lower(f, sig_names)?),
        )),
        // Sign-extension belum dimodelkan; menelan Signed akan salah pada
        // Eq/Add beda lebar, jadi tolak.
        IrExpr::Signed(_) => Err("IrExpr Signed belum didukung SEC".to_string()),
        _ => Err("IrExpr varian ini tidak didukung SEC".to_string()),
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
        SecExpr::Const(v, w) => {
            if *w == 0 || *w > 64 {
                return Err(format!("lebar konstanta {w} tidak didukung (1..=64)"));
            }
            Ok((z3::ast::BV::from_u64(*v, *w), *w))
        }
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
        SecExpr::Neg(inner) => {
            let (v, w) = eval(inner, env, widths)?;
            let zero = z3::ast::BV::from_u64(0, w);
            Ok((zero.bvsub(&v), w))
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
/// (maksimum operand, konstanta memakai lebar literal IR), BUKAN context
/// width SV — jadi
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
    /// Ekspresi "operand bukan nol" (seperti konversi && / ||).
    fn nonzero(s: SecExpr) -> SecExpr {
        SecExpr::Not(Box::new(SecExpr::Eq(
            Box::new(s),
            Box::new(SecExpr::Const(0, 1)),
        )))
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
                ("y2".to_string(), 1, SecExpr::Const(0, 1)),
            ],
        };
        let b = SecDesign {
            inputs: vec![("a".to_string(), 1)],
            outputs: vec![
                ("y1".to_string(), 1, SecExpr::Const(0, 1)),
                ("y2".to_string(), 1, SecExpr::Const(0, 1)),
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
        let lhs = SecExpr::Eq(Box::new(input("x")), Box::new(SecExpr::Const(3, 8)));
        let rhs = SecExpr::Eq(Box::new(SecExpr::Const(3, 8)), Box::new(input("x")));
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
            SecExpr::Eq(Box::new(input("x")), Box::new(SecExpr::Const(3, 8))),
        );
        let b = design(&[("x", 8)], "hit", 1, SecExpr::Const(1, 1));
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

    #[test]
    fn lowering_maps_supported_ir() {
        use mivon_ir::{BinaryIrOp, IrExpr};
        let names = vec!["a".to_string(), "b".to_string()];
        let ir = IrExpr::BinaryOp(
            BinaryIrOp::BitAnd,
            Box::new(IrExpr::Signal(0, 1)),
            Box::new(IrExpr::Signal(1, 1)),
        );
        let lowered = expr_from_ir(&ir, &names).expect("BitAnd didukung");
        assert_eq!(lowered, and(input("a"), input("b")));

        // Neq dipetakan ke NOT(Eq).
        let neq = IrExpr::BinaryOp(
            BinaryIrOp::Neq,
            Box::new(IrExpr::Signal(0, 1)),
            Box::new(IrExpr::Signal(1, 1)),
        );
        let lowered = expr_from_ir(&neq, &names).expect("Neq didukung");
        assert_eq!(
            lowered,
            SecExpr::Not(Box::new(SecExpr::Eq(
                Box::new(input("a")),
                Box::new(input("b"))
            )))
        );

        // Add/Sub/Mul dipetakan apa adanya (bukan ditukar/diratakan).
        let bin = |op: BinaryIrOp| {
            IrExpr::BinaryOp(
                op,
                Box::new(IrExpr::Signal(0, 8)),
                Box::new(IrExpr::Signal(1, 8)),
            )
        };
        let ab = || (Box::new(input("a")), Box::new(input("b")));
        assert_eq!(
            expr_from_ir(&bin(BinaryIrOp::Add), &names),
            Ok(SecExpr::Add(ab().0, ab().1))
        );
        assert_eq!(
            expr_from_ir(&bin(BinaryIrOp::Sub), &names),
            Ok(SecExpr::Sub(ab().0, ab().1))
        );
        assert_eq!(
            expr_from_ir(&bin(BinaryIrOp::Mul), &names),
            Ok(SecExpr::Mul(ab().0, ab().1))
        );
        assert_eq!(
            expr_from_ir(&bin(BinaryIrOp::BitOr), &names),
            Ok(SecExpr::Or(ab().0, ab().1))
        );
        assert_eq!(
            expr_from_ir(&bin(BinaryIrOp::BitXor), &names),
            Ok(SecExpr::Xor(ab().0, ab().1))
        );

        // Cond -> Mux(sel, then, else), urutan cabang tidak boleh tertukar.
        let cond = IrExpr::Cond(
            Box::new(IrExpr::Signal(0, 1)),
            Box::new(IrExpr::Signal(1, 8)),
            Box::new(IrExpr::Const(mivon_core::LogicVec::from_u64(0, 8))),
        );
        assert_eq!(
            expr_from_ir(&cond, &names),
            Ok(SecExpr::Mux(
                Box::new(input("a")),
                Box::new(input("b")),
                Box::new(SecExpr::Const(0, 8))
            ))
        );
    }

    #[test]
    fn lowering_rejects_unsupported_ir() {
        use mivon_ir::{BinaryIrOp, IrExpr};
        let names = vec!["a".to_string()];
        let div = IrExpr::BinaryOp(
            BinaryIrOp::Div,
            Box::new(IrExpr::Signal(0, 1)),
            Box::new(IrExpr::Const(mivon_core::LogicVec::from_u64(1, 1))),
        );
        match expr_from_ir(&div, &names) {
            Err(msg) => assert!(msg.contains("tidak didukung"), "{msg}"),
            Ok(e) => panic!("Div harus ditolak, dapat {e:?}"),
        }
        // SignalId di luar tabel nama juga ditolak.
        match expr_from_ir(&IrExpr::Signal(7, 1), &names) {
            Err(msg) => assert!(msg.contains("di luar tabel"), "{msg}"),
            Ok(e) => panic!("SignalId asing harus ditolak, dapat {e:?}"),
        }
        // Signed belum dimodelkan (sign-extension) -> tolak, jangan diam.
        match expr_from_ir(&IrExpr::Signed(Box::new(IrExpr::Signal(0, 8))), &names) {
            Err(msg) => assert!(msg.contains("Signed"), "{msg}"),
            Ok(e) => panic!("Signed harus ditolak, dapat {e:?}"),
        }
        // Konstanta > 64 bit ditolak (tidak boleh truncate diam-diam).
        let wide = IrExpr::Const(mivon_core::LogicVec::from_u64(1, 65));
        match expr_from_ir(&wide, &names) {
            Err(msg) => assert!(msg.contains("> 64"), "{msg}"),
            Ok(e) => panic!("konstanta 65 bit harus ditolak, dapat {e:?}"),
        }
    }

    #[test]
    fn logical_and_is_not_bitwise_and() {
        use mivon_ir::{BinaryIrOp, IrExpr};
        let names = vec!["a".to_string(), "b".to_string()];
        let logical = IrExpr::BinaryOp(
            BinaryIrOp::LogicalAnd,
            Box::new(IrExpr::Signal(0, 8)),
            Box::new(IrExpr::Signal(1, 8)),
        );
        let lowered = expr_from_ir(&logical, &names).expect("LogicalAnd didukung");
        // a=2, b=1: 2 && 1 = 1, tetapi 2 & 1 = 0 -> harus NotEquivalent.
        let a = design(&[("a", 8), ("b", 8)], "y", 1, lowered);
        let b = design(&[("a", 8), ("b", 8)], "y", 1, and(input("a"), input("b")));
        assert!(matches!(
            check_equivalence(&a, &b),
            SecOutcome::NotEquivalent { .. }
        ));
    }

    #[test]
    fn logical_or_is_not_bitwise_or() {
        use mivon_ir::{BinaryIrOp, IrExpr};
        let names = vec!["a".to_string(), "b".to_string()];
        let logical = IrExpr::BinaryOp(
            BinaryIrOp::LogicalOr,
            Box::new(IrExpr::Signal(0, 8)),
            Box::new(IrExpr::Signal(1, 8)),
        );
        let lowered = expr_from_ir(&logical, &names).expect("LogicalOr didukung");
        // a=2, b=0: 2 || 0 = 1, tetapi 2 | 0 = 2 -> harus NotEquivalent.
        let a = design(&[("a", 8), ("b", 8)], "y", 1, lowered);
        let b = design(&[("a", 8), ("b", 8)], "y", 1, or(input("a"), input("b")));
        assert!(matches!(
            check_equivalence(&a, &b),
            SecOutcome::NotEquivalent { .. }
        ));
    }

    #[test]
    fn logical_lowering_matches_zero_compare() {
        use mivon_ir::{BinaryIrOp, IrExpr};
        let names = vec!["a".to_string(), "b".to_string()];
        let sig = || {
            (
                Box::new(IrExpr::Signal(0, 8)),
                Box::new(IrExpr::Signal(1, 8)),
            )
        };
        let and_ir = IrExpr::BinaryOp(BinaryIrOp::LogicalAnd, sig().0, sig().1);
        let or_ir = IrExpr::BinaryOp(BinaryIrOp::LogicalOr, sig().0, sig().1);

        let lowered_and = expr_from_ir(&and_ir, &names).expect("LogicalAnd didukung");
        let lowered_or = expr_from_ir(&or_ir, &names).expect("LogicalOr didukung");

        // Formula yang benar: bandingkan tiap operand ke nol lalu AND/OR 1-bit.
        let want_and = and(nonzero(input("a")), nonzero(input("b")));
        let want_or = or(nonzero(input("a")), nonzero(input("b")));
        assert_eq!(lowered_and, want_and);
        assert_eq!(lowered_or, want_or);

        // Polaritas: && tidak boleh menjadi OR dari kedua predikat, dan ||
        // tidak boleh menjadi AND. Kalau lowering tertukar, ini gagal.
        let inputs = [("a", 8), ("b", 8)];
        let and_design = design(&inputs, "y", 1, lowered_and);
        let or_design = design(&inputs, "y", 1, lowered_or);
        assert_eq!(
            check_equivalence(&and_design, &design(&inputs, "y", 1, want_and.clone())),
            SecOutcome::Equivalent
        );
        assert_eq!(
            check_equivalence(&or_design, &design(&inputs, "y", 1, want_or.clone())),
            SecOutcome::Equivalent
        );
        assert!(matches!(
            check_equivalence(&and_design, &design(&inputs, "y", 1, want_or.clone())),
            SecOutcome::NotEquivalent { .. }
        ));
        assert!(matches!(
            check_equivalence(&or_design, &design(&inputs, "y", 1, want_and)),
            SecOutcome::NotEquivalent { .. }
        ));
    }

    #[test]
    fn literal_width_is_preserved_through_lowering() {
        use mivon_ir::{BinaryIrOp, IrExpr, UnaryIrOp};
        let names = vec!["a".to_string()];
        let const8 = |v: u64| Box::new(IrExpr::Const(mivon_core::LogicVec::from_u64(v, 8)));

        // ~8'h0F harus tetap 8 bit (bukan 64 bit).
        let not_const = IrExpr::UnaryOp(UnaryIrOp::BitNot, const8(0x0F));
        assert_eq!(
            expr_from_ir(&not_const, &names),
            Ok(SecExpr::Not(Box::new(SecExpr::Const(0x0F, 8))))
        );

        // Regresi lebar-konstanta: ~(a + 8'h01) == 8'hF0 hanya benar bila
        // konstanta tetap 8 bit. Kalau konstanta menjadi 64 bit, Add melebar
        // ke 64 bit, komplemen jadi 0xFFFF_FFFF_FFFF_FFF0 dan perbandingan
        // selalu 0 (false Equivalent).
        let ir = IrExpr::BinaryOp(
            BinaryIrOp::Eq,
            Box::new(IrExpr::UnaryOp(
                UnaryIrOp::BitNot,
                Box::new(IrExpr::BinaryOp(
                    BinaryIrOp::Add,
                    Box::new(IrExpr::Signal(0, 8)),
                    const8(1),
                )),
            )),
            const8(0xF0),
        );
        let lowered = expr_from_ir(&ir, &names).expect("Eq didukung");
        assert_eq!(
            lowered,
            SecExpr::Eq(
                Box::new(SecExpr::Not(Box::new(SecExpr::Add(
                    Box::new(input("a")),
                    Box::new(SecExpr::Const(1, 8)),
                )))),
                Box::new(SecExpr::Const(0xF0, 8)),
            )
        );
        // Ekuivalen dengan "a == 8'h0E" (bukan konstanta 0).
        let inputs = [("a", 8)];
        let got = design(&inputs, "y", 1, lowered);
        let want = design(
            &inputs,
            "y",
            1,
            SecExpr::Eq(Box::new(input("a")), Box::new(SecExpr::Const(0x0E, 8))),
        );
        assert_eq!(check_equivalence(&got, &want), SecOutcome::Equivalent);
        let never = design(&inputs, "y", 1, SecExpr::Const(0, 1));
        assert!(matches!(
            check_equivalence(&got, &never),
            SecOutcome::NotEquivalent { .. }
        ));
    }

    #[test]
    fn fill_lit_is_rejected() {
        use mivon_ir::IrExpr;
        let names = vec!["a".to_string()];
        match expr_from_ir(&IrExpr::FillLit(mivon_core::LogicVal::Zero), &names) {
            Err(msg) => assert!(msg.contains("FillLit"), "{msg}"),
            Ok(e) => panic!("FillLit butuh lebar konteks, harus ditolak: {e:?}"),
        }
    }

    #[test]
    fn duplicate_signal_name_is_rejected() {
        use mivon_ir::IrExpr;
        let dup = vec!["a".to_string(), "a".to_string()];
        match expr_from_ir(&IrExpr::Signal(0, 1), &dup) {
            Err(msg) => assert!(msg.contains("ganda"), "{msg}"),
            Ok(e) => panic!("nama sinyal ganda harus ditolak, dapat {e:?}"),
        }
        let empty = vec![String::new()];
        match expr_from_ir(&IrExpr::Signal(0, 1), &empty) {
            Err(msg) => assert!(msg.contains("kosong"), "{msg}"),
            Ok(e) => panic!("nama sinyal kosong harus ditolak, dapat {e:?}"),
        }
    }
    #[test]
    fn lowering_minus_preserves_operand_width() {
        use mivon_ir::{IrExpr, UnaryIrOp};
        let names = vec!["a".to_string()];
        let ir = IrExpr::UnaryOp(UnaryIrOp::Minus, Box::new(IrExpr::Signal(0, 8)));
        let lowered = expr_from_ir(&ir, &names).expect("Minus didukung");
        assert_eq!(lowered, SecExpr::Neg(Box::new(input("a"))));
        // Model NATURAL-WIDTH tahap 1: Neg memakai lebar operand (-8'h01 =
        // 0xFF) lalu zero-extend ke lebar deklarasi -> 0x00FF di lebar 16.
        // LRM memakai context width (16 bit -> 0xFFFF), jadi 0xFFFF di sini
        // adalah hasil model Sub 64 bit yang memang berbeda; lihat batasan di
        // doc modul. Asersi ini mengunci MODEL (harus diubah saat context
        // width diimplementasikan) sekaligus menangkap mutasi 'Neg dievaluasi
        // 64 bit', yang tidak terlihat pada lebar 8.
        let a = design(&[("a", 8)], "y", 16, lowered.clone());
        let b = design(
            &[("a", 8)],
            "y",
            16,
            SecExpr::Sub(Box::new(SecExpr::Const(0, 64)), Box::new(input("a"))),
        );
        match check_equivalence(&a, &b) {
            SecOutcome::NotEquivalent {
                output,
                counterexample,
            } => {
                assert_eq!(output, "y");
                // Setiap a != 0 menghasilkan selisih (0x00FF vs 0xFFFF dst).
                assert!(
                    counterexample.iter().any(|(n, v)| n == "a" && *v != 0),
                    "counterexample harus memuat a != 0: {counterexample:?}"
                );
            }
            other => panic!("Neg lebar-8 tidak boleh sama dengan sub 64 bit: {other:?}"),
        }
        // Pada lebar deklarasi 8 keduanya memang sama (0xFF) — di sini model
        // natural-width kebetulan sama dengan LRM.
        let a8 = design(&[("a", 8)], "y", 8, lowered.clone());
        let b8 = design(
            &[("a", 8)],
            "y",
            8,
            SecExpr::Sub(Box::new(SecExpr::Const(0, 64)), Box::new(input("a"))),
        );
        assert_eq!(check_equivalence(&a8, &b8), SecOutcome::Equivalent);
    }
}
