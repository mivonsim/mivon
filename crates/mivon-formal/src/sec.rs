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
//! dan operator terbatas pada yang ada di [`SecExpr`]. Ekstraksi dari
//! [`mivon_ir::IrModule`] tersedia lewat [`design_from_module`] untuk subset
//! kombinasional datar; SEC sekuensial (unrolling state per siklus) dan
//! front-end CLI belum ada — item COMP-16 tetap PARTIAL sampai itu ada.

use std::collections::HashMap;

/// Ekspresi kombinasional untuk SEC.
#[derive(Debug, Clone, PartialEq)]
pub enum SecExpr {
    /// Input bernama (lebar diambil dari [`SecDesign::inputs`]).
    Input(String),
    /// Konstanta beserta lebar operandnya (lebar literal IR).
    Const(u64, u32),
    /// Sesuaikan lebar seperti assignment SV: ekstensi (sign bila operand
    /// signed, selain itu nol) atau potong bit tinggi.
    Resize(Box<SecExpr>, u32),
    /// Nilai bertanda: bit-pattern sama dengan operand, tetapi pelebaran lebar
    /// (Resize/widen/fit) memakai sign-extension seperti SV.
    Signed(Box<SecExpr>),
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
        let va =
            match eval(expr_a, &env, &widths).and_then(|(v, w)| fit_width(v, w, *width, expr_a)) {
                Ok(v) => v,
                Err(e) => return SecOutcome::Unsupported(format!("output {name}: {e}")),
            };
        let vb =
            match eval(expr_b, &env, &widths).and_then(|(v, w)| fit_width(v, w, *width, expr_b)) {
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
                    .and_then(|(v, w)| fit_width(v, w, *width, expr_a))
                    .expect("sudah divalidasi");
                let vb = eval(expr_b, &env, &widths)
                    .and_then(|(v, w)| fit_width(v, w, *width, expr_b))
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
        // `Cast` dari elaborator = penyesuaian lebar. Aman hanya bila operand
        // sinyal/konstanta (ekstensi/pemotongan murni). Cast pada ekspresi
        // aritmetika berarti context width SV (carry ke bit lebih tinggi),
        // yang belum dimodelkan -> tolak, jangan tebak.
        IrExpr::Cast { width, expr } => {
            if !matches!(**expr, IrExpr::Signal(..) | IrExpr::Const(_)) {
                return Err(
                    "IrExpr Cast pada ekspresi non-sinyal — context width SV belum dimodelkan SEC"
                        .to_string(),
                );
            }
            if *width == 0 || *width > 64 {
                return Err(format!("lebar Cast {width} tidak didukung (1..=64)"));
            }
            Ok(SecExpr::Resize(
                Box::new(lower(expr, sig_names)?),
                *width as u32,
            ))
        }
        // `Signed(expr)` = nilai bertanda. Bit-pattern tetap, tetapi pelebaran
        // lebar memakai sign-extension (lihat is_signed/widen/Resize/fit_width).
        // Sinyal yang dideklarasikan signed tetap ditolak di design_from_module,
        // karena pembacaan tanpa pembungkus `Signed` tidak membawa tanda.
        IrExpr::Signed(inner) => Ok(SecExpr::Signed(Box::new(lower(inner, sig_names)?))),
        _ => Err("IrExpr varian ini tidak didukung SEC".to_string()),
    }
}

/// Ekstraksi tahap 2 (COMP-16): bangun [`SecDesign`] dari
/// [`mivon_ir::IrModule`] kombinasional.
///
/// Didukung: proses `Combinational`/`CombReactive` yang isinya hanya
/// `BlockingAssign` ke sinyal utuh (`IrLValue::Signal`). Sinyal internal
/// disubstitusi transitif DAN di-resize ke lebar deklarasinya (assignment SV
/// memotong/menambah lebar; tanpa itu `logic [7:0] t = a & b` dengan operand
/// 16 bit akan dibandingkan 16 bit — silent false Equivalent).
///
/// Ditolak dengan pesan eksplisit (bukan ditebak): statement selain
/// `BlockingAssign` (If/Case/Loop/Delay/...), assignment ke seleksi bit/array,
/// driver ganda, driver pada input, output atau sinyal internal tanpa driver,
/// loop kombinasional, sinyal `signed` (sign-extension belum dimodelkan),
/// sinyal `inout` (tri-state), instance sub-modul (hierarkis), proses
/// sekuensial/initial/final yang menulis output, dan referensi maju pada proses
/// `always @(...)` dengan sensitivity eksplisit (berarti latch). Initializer
/// konstanta (`output logic y = 1'b0`, `reg x = 0`) dilewati karena hanya nilai
/// t=0 dan tidak memengaruhi perilaku kombinasional tunak.
pub fn design_from_module(module: &mivon_ir::IrModule) -> Result<SecDesign, String> {
    use mivon_ir::Process;

    // Instance sub-modul = konektivitas hierarkis; menelusurinya belum
    // didukung (FORMAL-13). Kalau diabaikan, output yang sebenarnya digerakkan
    // sub-modul akan tampak "tanpa driver" dan SEC bisa salah menilai.
    if !module.sub_instances.is_empty() {
        return Err(format!(
            "modul {} punya {} instance sub-modul — SEC hierarkis belum didukung",
            module.name.as_str(),
            module.sub_instances.len()
        ));
    }
    if !module.inouts.is_empty() {
        return Err(format!(
            "modul {} punya sinyal inout — SEC tri-state belum didukung",
            module.name.as_str()
        ));
    }

    let mut ex = Extract {
        eq: HashMap::new(),
        names: module
            .signals
            .iter()
            .map(|s| s.name.as_str().to_string())
            .collect(),
        widths: module.signals.iter().map(|s| s.width as u32).collect(),
        inputs: module.inputs.clone(),
    };

    for p in &module.processes {
        match p {
            Process::CombReactive { body, .. } => ex.collect(body)?,
            Process::Combinational {
                sensitivity, body, ..
            } => {
                if !sensitivity_covers_reads(body, sensitivity) {
                    return Err(format!(
                        "proses {} membaca sinyal di luar sensitivity list — kemungkinan latch, ekstraksi SEC menolaknya",
                        p.name()
                    ));
                }
                ex.collect(body)?;
            }
            Process::Initial { body, .. } if is_constant_init(body) => {}
            Process::Sequential { body, .. }
            | Process::Initial { body, .. }
            | Process::Final { body, .. }
            | Process::AlwaysWithDelay { body, .. } => {
                if writes_any(body, &module.outputs) {
                    return Err(format!(
                        "proses {} menulis output — SEC hanya untuk logika kombinasional",
                        p.name()
                    ));
                }
            }
        }
    }

    // Input tidak boleh punya persamaan: kalau dibiarkan, sinyal input akan
    // disubstitusi dan hilang dari miter, sehingga SEC bisa melaporkan
    // Equivalent untuk desain yang sebenarnya berbeda.
    for &id in &module.inputs {
        if ex.eq.contains_key(&id) {
            return Err(format!("sinyal input {} ditulis oleh proses", ex.name(id)?));
        }
    }

    // Signedness belum dimodelkan: `logic signed [7:0] s` yang di-assign ke
    // target lebih lebar harus sign-extend, sedangkan model SEC zero-extend.
    let mut referenced: Vec<usize> = module.inputs.clone();
    referenced.extend(module.outputs.iter().copied());
    referenced.extend(ex.eq.keys().copied());
    for rhs in ex.eq.values() {
        collect_refs(rhs, &mut referenced);
    }
    for id in referenced {
        if module.signals.get(id).is_some_and(|s| s.is_signed) {
            return Err(format!(
                "sinyal {} dideklarasikan signed — sign-extension belum didukung SEC",
                ex.name(id)?
            ));
        }
    }

    let mut inputs = Vec::with_capacity(module.inputs.len());
    for &id in &module.inputs {
        inputs.push((ex.name(id)?.to_string(), ex.width(id)));
    }

    let mut outputs = Vec::with_capacity(module.outputs.len());
    for &id in &module.outputs {
        let name = ex.name(id)?;
        if !ex.eq.contains_key(&id) {
            return Err(format!("output {name} tidak punya driver kombinasional"));
        }
        let mut stack = Vec::new();
        let expr = ex.resolve(id, &mut stack)?;
        outputs.push((name.to_string(), ex.width(id), expr));
    }

    Ok(SecDesign { inputs, outputs })
}

/// Nama proses untuk pesan error.
trait ProcessName {
    fn name(&self) -> String;
}

impl ProcessName for mivon_ir::Process {
    fn name(&self) -> String {
        match self {
            mivon_ir::Process::Combinational { name, .. }
            | mivon_ir::Process::CombReactive { name, .. }
            | mivon_ir::Process::Sequential { name, .. }
            | mivon_ir::Process::Initial { name, .. }
            | mivon_ir::Process::Final { name, .. }
            | mivon_ir::Process::AlwaysWithDelay { name, .. } => name.as_str().to_string(),
        }
    }
}

/// Konteks ekstraksi: persamaan sinyal + tabel nama/lebar + daftar input.
struct Extract<'a> {
    eq: HashMap<usize, &'a mivon_ir::IrExpr>,
    names: Vec<String>,
    widths: Vec<u32>,
    inputs: Vec<usize>,
}

impl<'a> Extract<'a> {
    fn name(&self, id: usize) -> Result<&str, String> {
        self.names
            .get(id)
            .map(String::as_str)
            .ok_or_else(|| format!("SignalId {id} di luar tabel sinyal"))
    }

    fn width(&self, id: usize) -> u32 {
        self.widths.get(id).copied().unwrap_or(0)
    }

    /// Kumpulkan persamaan `signal = rhs` dari body proses kombinasional.
    ///
    /// Urutan assignment TIDAK dipakai sebagai urutan eksekusi: nilai tunak
    /// proses kombinasional adalah fixed point persamaan. Prasyaratnya
    /// sensitivity list lengkap (dicek `sensitivity_covers_reads`); kalau tidak,
    /// proses itu latch dan model persamaan simultan akan salah.
    fn collect(&mut self, body: &'a [mivon_ir::IrStmt]) -> Result<(), String> {
        use mivon_ir::{IrLValue, IrStmt};

        for stmt in body {
            match stmt {
                IrStmt::BlockingAssign {
                    lhs: IrLValue::Signal(id, _),
                    rhs,
                    delay: None,
                } => {
                    let name = self.name(*id)?.to_string();
                    if self.eq.insert(*id, rhs).is_some() {
                        return Err(format!("sinyal {name} ditulis lebih dari sekali"));
                    }
                }
                IrStmt::BlockingAssign { delay: Some(_), .. } => {
                    return Err("BlockingAssign berdelay tidak didukung SEC".to_string())
                }
                IrStmt::BlockingAssign { .. } => {
                    return Err("assignment ke seleksi bit/array tidak didukung SEC".to_string())
                }
                other => {
                    return Err(format!(
                        "statement {other:?} belum didukung ekstraksi SEC (hanya BlockingAssign)"
                    ))
                }
            }
        }
        Ok(())
    }

    /// Ekspresi SEC untuk sinyal: persamaan (jika ada) lalu substitusi
    /// rekursif, diakhiri resize ke lebar deklarasi sinyal itu.
    fn resolve(&self, id: usize, stack: &mut Vec<usize>) -> Result<SecExpr, String> {
        let name = self.name(id)?;
        let Some(rhs) = self.eq.get(&id) else {
            if self.inputs.contains(&id) {
                return Ok(SecExpr::Input(name.to_string()));
            }
            return Err(format!("sinyal {name} tidak punya driver kombinasional"));
        };
        if stack.contains(&id) {
            return Err(format!("loop kombinasional terdeteksi pada sinyal {name}"));
        }
        stack.push(id);
        // pop() harus tetap jalan walau lowering/substitusi error (stack dipakai
        // ulang bila resolve dimemoized nanti).
        let result =
            expr_from_ir(rhs, &self.names).and_then(|lowered| self.substitute(&lowered, stack));
        stack.pop();
        let out = result?;
        let w = self.width(id);
        let natural = natural_width(&out, &self.names, &self.widths);
        if natural < w && !widen_safe(&out, is_signed(&out)) {
            return Err(format!(
                "assignment ke {name} butuh context width SV (hasil signed aritmetika di-sign-extend) — belum dimodelkan SEC"
            ));
        }
        // Assignment ke sinyal unsigned menghapus tanda RHS, jadi bungkus
        // Resize juga saat lebar sama (kalau tidak, penanda Signed bocor ke
        // pembacaan berikutnya dan melebar salah).
        if natural == w && !is_signed(&out) {
            Ok(out)
        } else {
            Ok(SecExpr::Resize(Box::new(out), w))
        }
    }

    /// Ganti setiap `Input(nama)` yang punya persamaan dengan ekspresinya.
    fn substitute(&self, expr: &SecExpr, stack: &mut Vec<usize>) -> Result<SecExpr, String> {
        let mut recur = |e: &Box<SecExpr>| self.substitute(e, stack);
        Ok(match expr {
            SecExpr::Input(name) => match self.names.iter().position(|n| n == name) {
                // Nama yang bukan input wajib punya persamaan; resolve() yang
                // memutuskan (dan melaporkan sinyal internal tanpa driver).
                Some(id) if !self.inputs.contains(&id) => self.resolve(id, stack)?,
                _ => expr.clone(),
            },
            SecExpr::Const(..) => expr.clone(),
            SecExpr::Resize(a, w) => SecExpr::Resize(Box::new(recur(a)?), *w),
            SecExpr::Signed(a) => SecExpr::Signed(Box::new(recur(a)?)),
            SecExpr::Not(a) => SecExpr::Not(Box::new(recur(a)?)),
            SecExpr::Neg(a) => SecExpr::Neg(Box::new(recur(a)?)),
            SecExpr::And(a, b) => SecExpr::And(Box::new(recur(a)?), Box::new(recur(b)?)),
            SecExpr::Or(a, b) => SecExpr::Or(Box::new(recur(a)?), Box::new(recur(b)?)),
            SecExpr::Xor(a, b) => SecExpr::Xor(Box::new(recur(a)?), Box::new(recur(b)?)),
            SecExpr::Add(a, b) => SecExpr::Add(Box::new(recur(a)?), Box::new(recur(b)?)),
            SecExpr::Sub(a, b) => SecExpr::Sub(Box::new(recur(a)?), Box::new(recur(b)?)),
            SecExpr::Mul(a, b) => SecExpr::Mul(Box::new(recur(a)?), Box::new(recur(b)?)),
            SecExpr::Eq(a, b) => SecExpr::Eq(Box::new(recur(a)?), Box::new(recur(b)?)),
            SecExpr::Mux(s, t, f) => SecExpr::Mux(
                Box::new(recur(s)?),
                Box::new(recur(t)?),
                Box::new(recur(f)?),
            ),
        })
    }
}

/// Initializer konstanta (port/deklarasi): hanya nilai t=0, bukan state.
fn is_constant_init(body: &[mivon_ir::IrStmt]) -> bool {
    use mivon_ir::IrStmt;
    !body.is_empty()
        && body.iter().all(|s| {
            matches!(
                s,
                IrStmt::BlockingAssign {
                    rhs,
                    delay: None,
                    ..
                } if is_const_rhs(rhs)
            )
        })
}

/// Apakah ekspresi ini konstanta murni (boleh dibungkus `Signed`/`Cast`).
///
/// Bentuk nyata dari elaborator: literal desimal `= 0` menjadi
/// `Signed(Const(0,32))`, bukan `Const` telanjang.
fn is_const_rhs(expr: &mivon_ir::IrExpr) -> bool {
    use mivon_ir::IrExpr;
    match expr {
        IrExpr::Const(_) | IrExpr::FillLit(_) => true,
        IrExpr::Signed(inner) | IrExpr::Cast { expr: inner, .. } => is_const_rhs(inner),
        _ => false,
    }
}

/// Apakah sensitivity list proses `always @(...)` menutup semua sinyal yang
/// dibaca body.
///
/// Sensitivity kosong dianggap lengkap (wildcard/`always @(*)` — elaborator
/// mengisinya dari read-set). Sensitivity parsial (`@(a[0])`) tidak bisa
/// diverifikasi terhadap read-set per-bit, jadi dianggap TIDAK menutup.
/// Kalau ada read di luar daftar, proses menyimpan nilai lama = latch, dan
/// model persamaan simultan SEC akan memberi hasil yang salah.
fn sensitivity_covers_reads(
    body: &[mivon_ir::IrStmt],
    sens: &[mivon_ir::SignalSensitivity],
) -> bool {
    use mivon_ir::IrStmt;
    if sens.is_empty() {
        return true;
    }
    let mut reads = Vec::new();
    for s in body {
        if let IrStmt::BlockingAssign { rhs, .. } = s {
            collect_refs(rhs, &mut reads);
        }
    }
    reads.iter().all(|id| {
        sens.iter()
            .any(|e| e.sig_id == *id && e.msb.is_none() && e.lsb.is_none())
    })
}

/// Kumpulkan semua SignalId yang dibaca sebuah ekspresi.
fn collect_refs(expr: &mivon_ir::IrExpr, out: &mut Vec<usize>) {
    use mivon_ir::IrExpr as E;
    match expr {
        E::Signal(id, _) | E::RangeSelect(id, _, _) | E::BitSelect(id, _) => out.push(*id),
        E::ArrayIndex { sig_id, index, .. } => {
            out.push(*sig_id);
            collect_refs(index, out);
        }
        E::ExprRangeSelect(a, _, _)
        | E::ExprBitSelect(a, _)
        | E::Signed(a)
        | E::Cast { expr: a, .. }
        | E::Replicate(_, a)
        | E::UnaryOp(_, a)
        | E::MemberAccess { obj: a, .. }
        | E::Dist { expr: a, .. }
        | E::IncDec { read: a, .. } => collect_refs(a, out),
        E::ExprPartSelect(a, b, c) => {
            collect_refs(a, out);
            collect_refs(b, out);
            collect_refs(c, out);
        }
        E::BinaryOp(_, a, b) => {
            collect_refs(a, out);
            collect_refs(b, out);
        }
        E::Cond(a, b, c) => {
            collect_refs(a, out);
            collect_refs(b, out);
            collect_refs(c, out);
        }
        E::Concat(items) | E::StreamingConcat { slices: items, .. } => {
            for e in items {
                collect_refs(e, out);
            }
        }
        E::SysFunc { args, .. }
        | E::NewCall { args, .. }
        | E::DpiCall { args, .. }
        | E::FuncCall { args, .. }
        | E::UdpLookup { args, .. } => {
            for e in args {
                collect_refs(e, out);
            }
        }
        E::MethodCall {
            obj,
            args,
            with_clause,
            ..
        } => {
            collect_refs(obj, out);
            for e in args {
                collect_refs(e, out);
            }
            if let Some(w) = with_clause {
                collect_refs(w, out);
            }
        }
        E::Inside { expr, list } => {
            collect_refs(expr, out);
            for e in list {
                collect_refs(e, out);
            }
        }
        E::InsideRange { expr, lo, hi } => {
            collect_refs(expr, out);
            collect_refs(lo, out);
            collect_refs(hi, out);
        }
        E::Const(_)
        | E::FillLit(_)
        | E::String(_)
        | E::This
        | E::HierRef(_)
        | E::VifBinding { .. }
        | E::VirtualIfaceAccess { .. }
        | E::RealConst(_) => {}
    }
}

/// Lebar natural ekspresi SEC (aturan yang sama dengan `eval`: maksimum
/// operand, `Eq` 1 bit, `Resize` lebar target).
fn natural_width(expr: &SecExpr, names: &[String], widths: &[u32]) -> u32 {
    match expr {
        SecExpr::Input(n) => names
            .iter()
            .position(|x| x == n)
            .and_then(|i| widths.get(i).copied())
            .unwrap_or(1),
        SecExpr::Const(_, w) => *w,
        SecExpr::Resize(_, w) => *w,
        SecExpr::Signed(a) | SecExpr::Not(a) | SecExpr::Neg(a) => natural_width(a, names, widths),
        SecExpr::And(a, b)
        | SecExpr::Or(a, b)
        | SecExpr::Xor(a, b)
        | SecExpr::Add(a, b)
        | SecExpr::Sub(a, b)
        | SecExpr::Mul(a, b) => {
            natural_width(a, names, widths).max(natural_width(b, names, widths))
        }
        SecExpr::Eq(..) => 1,
        SecExpr::Mux(_, t, f) => {
            natural_width(t, names, widths).max(natural_width(f, names, widths))
        }
    }
}

/// Apakah lvalue ini bisa menulis salah satu sinyal target.
///
/// `HierRef`/`HierRefIndex`/`VifField` tidak bisa dipetakan ke SignalId saat
/// elaborasi, jadi dianggap BISA menulis target (konservatif: lebih baik
/// menolak desain ber-state daripada meloloskan model yang salah).
fn lvalue_may_write(lv: &mivon_ir::IrLValue, targets: &[usize]) -> bool {
    use mivon_ir::IrLValue as L;
    match lv {
        L::Signal(id, _) | L::RangeSelect(id, _, _) | L::BitSelect(id, _) => targets.contains(id),
        L::ArrayIndex { sig_id, .. }
        | L::ArrayRangeSelect { sig_id, .. }
        | L::ArrayBitSelect { sig_id, .. }
        | L::ExprPartSelect { sig_id, .. }
        | L::ObjectField { sig_id, .. } => targets.contains(sig_id),
        L::Concat(parts) => parts.iter().any(|p| lvalue_may_write(p, targets)),
        L::HierRef(_) | L::HierRefIndex { .. } | L::VifField { .. } => true,
    }
}

/// Apakah body menulis salah satu sinyal target (untuk menolak desain ber-state).
fn writes_any(body: &[mivon_ir::IrStmt], targets: &[usize]) -> bool {
    use mivon_ir::IrStmt;
    body.iter().any(|s| match s {
        IrStmt::BlockingAssign { lhs, .. } | IrStmt::NonBlockingAssign { lhs, .. } => {
            lvalue_may_write(lhs, targets)
        }
        IrStmt::Force { lvalue, .. } | IrStmt::Release { lvalue } | IrStmt::Deassign { lvalue } => {
            lvalue_may_write(lvalue, targets)
        }
        IrStmt::Block { stmts } | IrStmt::NamedBlock { stmts, .. } => writes_any(stmts, targets),
        IrStmt::If {
            true_branch,
            false_branch,
            ..
        } => writes_any(true_branch, targets) || writes_any(false_branch, targets),
        IrStmt::Case { items, default, .. } => {
            items.iter().any(|it| writes_any(&it.body, targets)) || writes_any(default, targets)
        }
        IrStmt::LoopFor {
            init, step, body, ..
        } => {
            init.as_deref()
                .is_some_and(|s| writes_any(std::slice::from_ref(s), targets))
                || step
                    .as_deref()
                    .is_some_and(|s| writes_any(std::slice::from_ref(s), targets))
                || writes_any(body, targets)
        }
        IrStmt::LoopWhile { body, .. }
        | IrStmt::LoopDoWhile { body, .. }
        | IrStmt::Repeat { body, .. }
        | IrStmt::Foreach { body, .. }
        | IrStmt::Delay { body, .. }
        | IrStmt::Wait { body, .. }
        | IrStmt::EventControl { body, .. } => writes_any(body, targets),
        IrStmt::Fork { processes, .. } => processes.iter().any(|p| writes_any(p, targets)),
        IrStmt::Assert {
            pass_stmt,
            fail_stmt,
            ..
        }
        | IrStmt::Assume {
            pass_stmt,
            fail_stmt,
            ..
        }
        | IrStmt::Expect {
            pass_stmt,
            fail_stmt,
            ..
        } => writes_any(pass_stmt, targets) || writes_any(fail_stmt, targets),
        IrStmt::Cover { pass_stmt, .. } => writes_any(pass_stmt, targets),
        IrStmt::RandCase { items } => items.iter().any(|(_, b)| writes_any(b, targets)),
        IrStmt::RandSequence { productions } => productions
            .iter()
            .any(|(_, arms)| arms.iter().any(|(_, b)| writes_any(b, targets))),
        IrStmt::WaitOrder { failure_stmts, .. } => writes_any(failure_stmts, targets),
        // $readmem* menulis isi memori (array signal) — konservatif.
        IrStmt::SysCall { name, .. } => matches!(name.as_str(), "$readmemh" | "$readmemb"),
        _ => false,
    })
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
        SecExpr::Resize(inner, w) => {
            if *w == 0 || *w > 64 {
                return Err(format!("lebar resize {w} tidak didukung (1..=64)"));
            }
            let (v, vw) = eval(inner, env, widths)?;
            if vw == *w {
                Ok((v, *w))
            } else if vw < *w {
                if !widen_safe(inner, is_signed(inner)) {
                    return Err(
                        "pelebaran nilai signed hasil aritmetika butuh context width SV — belum dimodelkan SEC"
                            .to_string(),
                    );
                }
                let ext = if is_signed(inner) {
                    v.sign_ext(*w - vw)
                } else {
                    v.zero_ext(*w - vw)
                };
                Ok((ext, *w))
            } else {
                Ok((v.extract(*w - 1, 0), *w))
            }
        }
        // Nilai bertanda: bit-pattern sama; pelebaran ditangani Resize/widen/fit.
        SecExpr::Signed(inner) => eval(inner, env, widths),
        SecExpr::Not(inner) => {
            let (v, w) = eval(inner, env, widths)?;
            Ok((v.bvnot(), w))
        }
        SecExpr::And(l, r) | SecExpr::Or(l, r) | SecExpr::Xor(l, r) => {
            let (lv, lw) = eval(l, env, widths)?;
            let (rv, rw) = eval(r, env, widths)?;
            let (lv, rv, w) = widen(lv, lw, rv, rw, l, r)?;
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
            let (lv, rv, w) = widen(lv, lw, rv, rw, l, r)?;
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
            let (lv, rv, _) = widen(lv, lw, rv, rw, l, r)?;
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
            let (tv, fv, w) = widen(tv, tw, fv, fw, t, f)?;
            Ok((cond.ite(&tv, &fv), w))
        }
    }
}

/// Samakan lebar dua BV (lebar hasil = maksimum).
///
/// SV §11.6.1: bila KEDUA operand signed, keduanya sign-extend; bila salah
/// satu unsigned, keduanya diperlakukan unsigned (zero-extend).
fn widen(
    lv: z3::ast::BV,
    lw: u32,
    rv: z3::ast::BV,
    rw: u32,
    l: &SecExpr,
    r: &SecExpr,
) -> Result<(z3::ast::BV, z3::ast::BV, u32), String> {
    let w = lw.max(rw);
    // Melebarkan operand berarti menghitungnya pada lebar lebih besar; hanya
    // sah bila operasi komutatif dengan ekstensi (lihat widen_safe).
    let both_signed = is_signed(l) && is_signed(r);
    if lw != rw && (!widen_safe(l, both_signed) || !widen_safe(r, both_signed)) {
        return Err(
            "operand dilebarkan ke lebar operand lain (context width SV) — belum dimodelkan SEC"
                .to_string(),
        );
    }
    let ext = |v: z3::ast::BV, from: u32| {
        if from == w {
            v
        } else if both_signed {
            v.sign_ext(w - from)
        } else {
            v.zero_ext(w - from)
        }
    };
    Ok((ext(lv, lw), ext(rv, rw), w))
}

/// Apakah ekspresi ini bertanda (SV: operand signed dan operator yang
/// mempertahankan signedness).
///
/// `Eq` selalu 1 bit unsigned; `Const`/`Input`/`Resize` unsigned kecuali
/// dibungkus [`SecExpr::Signed`].
fn is_signed(expr: &SecExpr) -> bool {
    match expr {
        SecExpr::Signed(_) => true,
        SecExpr::Not(a) | SecExpr::Neg(a) => is_signed(a),
        SecExpr::And(a, b)
        | SecExpr::Or(a, b)
        | SecExpr::Xor(a, b)
        | SecExpr::Add(a, b)
        | SecExpr::Sub(a, b)
        | SecExpr::Mul(a, b) => is_signed(a) && is_signed(b),
        SecExpr::Mux(_, t, f) => is_signed(t) && is_signed(f),
        SecExpr::Input(_) | SecExpr::Const(..) | SecExpr::Resize(..) | SecExpr::Eq(..) => false,
    }
}

/// Sesuaikan ekspresi ke lebar deklarasi output: lebih sempit -> ekstensi
/// (sign bila ekspresi bertanda, selain itu nol), lebih lebar -> truncate.
/// Lebar ekspresi di sini adalah lebar NATURAL (maksimum operand, konstanta
/// memakai lebar literal IR), BUKAN context width SV — jadi `Add(x8, y8)`
/// yang dideklarasikan 16 bit tidak mendapat carry ke bit 8 seperti di SV.
/// Lihat batasan di doc modul.
fn fit_width(v: z3::ast::BV, w: u32, declared: u32, expr: &SecExpr) -> Result<z3::ast::BV, String> {
    if declared == 0 || declared > 64 {
        return Err(format!("lebar output {declared} tidak didukung (1..=64)"));
    }
    let signed = is_signed(expr);
    if w == declared {
        Ok(v)
    } else if w < declared {
        if !widen_safe(expr, signed) {
            return Err(
                "output signed hasil aritmetika yang dilebarkan butuh context width SV — belum dimodelkan SEC"
                    .to_string(),
            );
        }
        Ok(if signed {
            v.sign_ext(declared - w)
        } else {
            v.zero_ext(declared - w)
        })
    } else {
        Ok(v.extract(declared - 1, 0))
    }
}

/// Apakah pelebaran ekspresi ini ekuivalen dengan menghitungnya pada lebar
/// konteks (SV §11.8.3).
///
/// Bitwise/Not/Neg/Mux komutatif dengan ekstensi (nol maupun sign), tetapi
/// Add/Sub/Mul TIDAK: carry/borrow melintasi bit yang dilebarkan, sehingga
/// hasil aritmetika tidak boleh dihitung pada lebar natural lalu diekstensi.
/// Berlaku untuk signed DAN unsigned (`y16 = a8 + b8` harus carry ke bit 8).
fn widen_safe(expr: &SecExpr, sign_ext: bool) -> bool {
    match expr {
        SecExpr::Add(..) | SecExpr::Sub(..) | SecExpr::Mul(..) | SecExpr::Neg(_) => false,
        // `~` komutatif dengan sign-extension, TIDAK dengan zero-extension:
        // aman hanya bila ekstensi di situs pemakaian memang sign.
        SecExpr::Not(a) => sign_ext && is_signed(a) && widen_safe(a, sign_ext),
        SecExpr::Signed(a) => widen_safe(a, sign_ext),
        SecExpr::And(a, b) | SecExpr::Or(a, b) | SecExpr::Xor(a, b) => {
            widen_safe(a, sign_ext) && widen_safe(b, sign_ext)
        }
        SecExpr::Mux(_, t, f) => widen_safe(t, sign_ext) && widen_safe(f, sign_ext),
        SecExpr::Resize(a, _) => widen_safe(a, sign_ext),
        SecExpr::Input(_) | SecExpr::Const(..) | SecExpr::Eq(..) => true,
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
        // Signed sekarang DIMODELKAN: dibungkus SecExpr::Signed supaya
        // pelebaran lebar memakai sign-extension (bukan ditolak).
        assert_eq!(
            expr_from_ir(&IrExpr::Signed(Box::new(IrExpr::Signal(0, 8))), &names),
            Ok(SecExpr::Signed(Box::new(input("a"))))
        );
        // Varian IrExpr yang tidak punya padanan SEC tetap ditolak.
        match expr_from_ir(&IrExpr::String("x".to_string()), &names) {
            Err(msg) => assert!(msg.contains("tidak didukung"), "{msg}"),
            Ok(e) => panic!("IrExpr String harus ditolak, dapat {e:?}"),
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
        // Pelebaran hasil Neg DITOLAK (butuh context width SV; negasi nilai
        // minimum juga overflow), jadi desain 16 bit harus Unsupported dan
        // tidak boleh dibandingkan sebagai Equivalent.
        let a = design(&[("a", 8)], "y", 16, lowered.clone());
        let b = design(
            &[("a", 8)],
            "y",
            16,
            SecExpr::Sub(Box::new(SecExpr::Const(0, 64)), Box::new(input("a"))),
        );
        match check_equivalence(&a, &b) {
            SecOutcome::Unsupported(msg) => assert!(msg.contains("context width"), "{msg}"),
            other => panic!("Neg yang dilebarkan harus Unsupported: {other:?}"),
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

    // ── Ekstraksi dari IrModule (COMP-16 tahap 2) ──

    fn sig(name: &str, width: usize) -> mivon_ir::SignalInfo {
        mivon_ir::SignalInfo {
            name: mivon_core::intern::Symbol::intern(name),
            width,
            ..Default::default()
        }
    }

    fn comb_module(
        signals: Vec<mivon_ir::SignalInfo>,
        inputs: Vec<usize>,
        outputs: Vec<usize>,
        stmts: Vec<mivon_ir::IrStmt>,
    ) -> mivon_ir::IrModule {
        mivon_ir::IrModule {
            name: mivon_core::intern::Symbol::intern("m"),
            signals,
            inputs,
            outputs,
            processes: vec![mivon_ir::Process::Combinational {
                name: mivon_core::intern::Symbol::intern("always_comb"),
                sensitivity: Vec::new(),
                body: stmts,
            }],
            ..Default::default()
        }
    }

    /// Ubah proses pertama menjadi `always_comb` (CombReactive): bereaksi
    /// ulang sampai fixed point, jadi urutan assignment tidak memengaruhi.
    fn reactive(mut m: mivon_ir::IrModule) -> mivon_ir::IrModule {
        if let Some(mivon_ir::Process::Combinational { body, .. }) = m.processes.first().cloned() {
            m.processes = vec![mivon_ir::Process::CombReactive {
                name: mivon_core::intern::Symbol::intern("always_comb"),
                sensitivity: Vec::new(),
                body,
            }];
        }
        m
    }

    fn assign(sig_id: usize, width: usize, rhs: mivon_ir::IrExpr) -> mivon_ir::IrStmt {
        mivon_ir::IrStmt::BlockingAssign {
            lhs: mivon_ir::IrLValue::Signal(sig_id, width),
            rhs,
            delay: None,
        }
    }

    fn and_expr(a: usize, b: usize, w: usize) -> mivon_ir::IrExpr {
        mivon_ir::IrExpr::BinaryOp(
            mivon_ir::BinaryIrOp::BitAnd,
            Box::new(mivon_ir::IrExpr::Signal(a, w)),
            Box::new(mivon_ir::IrExpr::Signal(b, w)),
        )
    }

    #[test]
    fn design_from_module_extracts_continuous_logic() {
        let m = comb_module(
            vec![sig("a", 8), sig("b", 8), sig("y", 8)],
            vec![0, 1],
            vec![2],
            vec![assign(2, 8, and_expr(0, 1, 8))],
        );
        let d = design_from_module(&m).expect("ekstraksi harus berhasil");
        assert_eq!(d.inputs, vec![("a".to_string(), 8), ("b".to_string(), 8)]);
        assert_eq!(d.outputs.len(), 1);
        assert_eq!(d.outputs[0].0, "y");
        assert_eq!(d.outputs[0].1, 8);
        assert_eq!(d.outputs[0].2, and(input("a"), input("b")));

        // Modul lain dengan ekspresi berbeda harus NotEquivalent.
        let m2 = comb_module(
            vec![sig("a", 8), sig("b", 8), sig("y", 8)],
            vec![0, 1],
            vec![2],
            vec![assign(2, 8, and_expr(0, 0, 8))],
        );
        let d2 = design_from_module(&m2).unwrap();
        assert!(matches!(
            check_equivalence(&d, &d2),
            SecOutcome::NotEquivalent { .. }
        ));
    }

    #[test]
    fn design_from_module_substitutes_internal_signals() {
        // t = a & b; y = t  ==  y = a & b
        let chained = comb_module(
            vec![sig("a", 8), sig("b", 8), sig("t", 8), sig("y", 8)],
            vec![0, 1],
            vec![3],
            vec![
                assign(2, 8, and_expr(0, 1, 8)),
                assign(3, 8, mivon_ir::IrExpr::Signal(2, 8)),
            ],
        );
        let flat = comb_module(
            vec![sig("a", 8), sig("b", 8), sig("y", 8)],
            vec![0, 1],
            vec![2],
            vec![assign(2, 8, and_expr(0, 1, 8))],
        );
        let a = design_from_module(&chained).unwrap();
        let b = design_from_module(&flat).unwrap();
        assert_eq!(check_equivalence(&a, &b), SecOutcome::Equivalent);
    }

    #[test]
    fn design_from_module_rejects_combinational_loop() {
        // t = y; y = t -> loop. Pakai always_comb supaya guard referensi maju
        // tidak lebih dulu menolak (keduanya sama-sama Err, tapi test ini
        // mengunci deteksi LOOP).
        let m = reactive(comb_module(
            vec![sig("t", 8), sig("y", 8)],
            vec![],
            vec![1],
            vec![
                assign(0, 8, mivon_ir::IrExpr::Signal(1, 8)),
                assign(1, 8, mivon_ir::IrExpr::Signal(0, 8)),
            ],
        ));
        match design_from_module(&m) {
            Err(msg) => assert!(msg.contains("loop kombinasional"), "{msg}"),
            Ok(d) => panic!("loop harus ditolak, dapat {d:?}"),
        }
    }

    #[test]
    fn design_from_module_rejects_unsupported_statements() {
        // If di dalam always_comb belum didukung ekstraksi tahap 2.
        let m = comb_module(
            vec![sig("a", 1), sig("y", 1)],
            vec![0],
            vec![1],
            vec![mivon_ir::IrStmt::If {
                cond: mivon_ir::IrExpr::Signal(0, 1),
                true_branch: vec![assign(
                    1,
                    1,
                    mivon_ir::IrExpr::FillLit(mivon_core::LogicVal::One),
                )],
                false_branch: vec![],
            }],
        );
        match design_from_module(&m) {
            Err(msg) => assert!(msg.contains("belum didukung"), "{msg}"),
            Ok(d) => panic!("If harus ditolak, dapat {d:?}"),
        }
    }

    #[test]
    fn design_from_module_rejects_sequential_output() {
        let mut m = comb_module(
            vec![sig("clk", 1), sig("d", 8), sig("q", 8)],
            vec![1],
            vec![2],
            vec![],
        );
        m.processes = vec![mivon_ir::Process::Sequential {
            name: mivon_core::intern::Symbol::intern("always_ff"),
            clock: mivon_ir::ClockEdge::PosEdge(0),
            reset: None,
            body: vec![mivon_ir::IrStmt::NonBlockingAssign {
                lhs: mivon_ir::IrLValue::Signal(2, 8),
                rhs: mivon_ir::IrExpr::Signal(1, 8),
                delay: None,
            }],
            iff: None,
        }];
        match design_from_module(&m) {
            Err(msg) => assert!(msg.contains("kombinasional"), "{msg}"),
            Ok(d) => panic!("output sekuensial harus ditolak, dapat {d:?}"),
        }
    }

    #[test]
    fn design_from_module_rejects_duplicate_driver() {
        // y ditulis dua kali -> model driver ganda, bukan "yang terakhir menang".
        let m = comb_module(
            vec![sig("a", 8), sig("b", 8), sig("y", 8)],
            vec![0, 1],
            vec![2],
            vec![
                assign(2, 8, and_expr(0, 1, 8)),
                assign(2, 8, and_expr(1, 0, 8)),
            ],
        );
        match design_from_module(&m) {
            Err(msg) => assert!(msg.contains("lebih dari sekali"), "{msg}"),
            Ok(d) => panic!("driver ganda harus ditolak, dapat {d:?}"),
        }
    }

    #[test]
    fn design_from_module_rejects_driver_on_input() {
        // Proses menulis sinyal input: bila diterima, input akan disubstitusi
        // dan hilang dari miter (silent wrong Equivalent).
        let m = comb_module(
            vec![sig("a", 8), sig("b", 8), sig("y", 8)],
            vec![0, 1],
            vec![2],
            vec![
                assign(0, 8, and_expr(0, 1, 8)),
                assign(2, 8, and_expr(0, 1, 8)),
            ],
        );
        match design_from_module(&m) {
            Err(msg) => assert!(msg.contains("input a ditulis"), "{msg}"),
            Ok(d) => panic!("driver pada input harus ditolak, dapat {d:?}"),
        }
    }

    #[test]
    fn design_from_module_rejects_undriven_output() {
        // Output tanpa proses penggerak tidak boleh menjadi variabel bebas Z3.
        let m = comb_module(vec![sig("a", 8), sig("y", 8)], vec![0], vec![1], vec![]);
        match design_from_module(&m) {
            Err(msg) => assert!(msg.contains("tidak punya driver"), "{msg}"),
            Ok(d) => panic!("output tanpa driver harus ditolak, dapat {d:?}"),
        }
    }

    #[test]
    fn design_from_module_rejects_sub_instances() {
        // Konektivitas hierarkis belum ditelusuri (FORMAL-13).
        let mut m = comb_module(
            vec![sig("a", 8), sig("y", 8)],
            vec![0],
            vec![1],
            vec![assign(1, 8, mivon_ir::IrExpr::Signal(0, 8))],
        );
        m.sub_instances = vec![mivon_ir::IrInstance {
            module_name: mivon_core::intern::Symbol::intern("sub"),
            instance_name: mivon_core::intern::Symbol::intern("u0"),
            port_map: Default::default(),
            param_map: Default::default(),
            type_param_map: Default::default(),
            line: 1,
            col: 1,
            attrs: Vec::new(),
        }];
        match design_from_module(&m) {
            Err(msg) => assert!(msg.contains("instance sub-modul"), "{msg}"),
            Ok(d) => panic!("instance sub-modul harus ditolak, dapat {d:?}"),
        }
    }

    #[test]
    fn design_from_module_resizes_narrowing_internal_signal() {
        // logic [7:0] t = a & b (operand 16 bit); y = (t == 8'hFF).
        // Tanpa resize, SEC membandingkan 16 bit: a&b = 16'h01FF akan lolos
        // sebagai "bukan 8'hFF" padahal perangkat keras melihat 8'hFF.
        let m = comb_module(
            vec![sig("a", 16), sig("b", 16), sig("t", 8), sig("y", 1)],
            vec![0, 1],
            vec![3],
            vec![
                assign(2, 8, and_expr(0, 1, 16)),
                assign(
                    3,
                    1,
                    mivon_ir::IrExpr::BinaryOp(
                        mivon_ir::BinaryIrOp::Eq,
                        Box::new(mivon_ir::IrExpr::Signal(2, 8)),
                        Box::new(mivon_ir::IrExpr::Const(mivon_core::LogicVec::from_u64(
                            0xFF, 8,
                        ))),
                    ),
                ),
            ],
        );
        let d = design_from_module(&m).expect("ekstraksi harus berhasil");
        let right = design(
            &[("a", 16), ("b", 16)],
            "y",
            1,
            SecExpr::Eq(
                Box::new(SecExpr::Resize(Box::new(and(input("a"), input("b"))), 8)),
                Box::new(SecExpr::Const(0xFF, 8)),
            ),
        );
        let wrong = design(
            &[("a", 16), ("b", 16)],
            "y",
            1,
            SecExpr::Eq(
                Box::new(and(input("a"), input("b"))),
                Box::new(SecExpr::Const(0xFF, 8)),
            ),
        );
        assert_eq!(check_equivalence(&d, &right), SecOutcome::Equivalent);
        assert!(matches!(
            check_equivalence(&d, &wrong),
            SecOutcome::NotEquivalent { .. }
        ));
    }

    #[test]
    fn design_from_module_resizes_widening_internal_signal() {
        // logic [7:0] t = a (a hanya 4 bit); y = ~t.
        // Model tanpa resize: ~a pada 4 bit -> 0x0?; nyata: ~zero_ext(a) ->
        // 0xF?. Guard ini memastikan Resize juga menangani arah melebar.
        let m = comb_module(
            vec![sig("a", 4), sig("t", 8), sig("y", 8)],
            vec![0],
            vec![2],
            vec![
                assign(1, 8, mivon_ir::IrExpr::Signal(0, 4)),
                assign(
                    2,
                    8,
                    mivon_ir::IrExpr::UnaryOp(
                        mivon_ir::UnaryIrOp::BitNot,
                        Box::new(mivon_ir::IrExpr::Signal(1, 8)),
                    ),
                ),
            ],
        );
        let d = design_from_module(&m).expect("ekstraksi harus berhasil");
        let right = design(
            &[("a", 4)],
            "y",
            8,
            SecExpr::Not(Box::new(SecExpr::Resize(Box::new(input("a")), 8))),
        );
        let wrong = design(&[("a", 4)], "y", 8, SecExpr::Not(Box::new(input("a"))));
        assert_eq!(check_equivalence(&d, &right), SecOutcome::Equivalent);
        // Model tanpa Resize (Not 4 bit lalu dilebarkan) kini ditolak sebagai
        // context width, bukan dilaporkan setara.
        assert!(matches!(
            check_equivalence(&d, &wrong),
            SecOutcome::Unsupported(_)
        ));
    }

    #[test]
    fn design_from_module_rejects_signed_signal() {
        // Sign-extension belum dimodelkan: lebih baik Err daripada zero-extend.
        let mut s = sig("s", 8);
        s.is_signed = true;
        let m = comb_module(
            vec![s, sig("y", 16)],
            vec![0],
            vec![1],
            vec![assign(1, 16, mivon_ir::IrExpr::Signal(0, 8))],
        );
        match design_from_module(&m) {
            Err(msg) => assert!(msg.contains("signed"), "{msg}"),
            Ok(d) => panic!("sinyal signed harus ditolak, dapat {d:?}"),
        }
    }

    /// Set sensitivity list proses pertama (Combinational).
    fn with_sensitivity(mut m: mivon_ir::IrModule, ids: &[usize]) -> mivon_ir::IrModule {
        let sens: Vec<mivon_ir::SignalSensitivity> = ids
            .iter()
            .map(|id| mivon_ir::SignalSensitivity::whole(*id))
            .collect();
        if let Some(mivon_ir::Process::Combinational { body, .. }) = m.processes.first().cloned() {
            m.processes = vec![mivon_ir::Process::Combinational {
                name: mivon_core::intern::Symbol::intern("always_0"),
                sensitivity: sens,
                body,
            }];
        }
        m
    }

    #[test]
    fn design_from_module_rejects_incomplete_sensitivity() {
        // always @(a or b): y = t; t = a & b;  -> t dibaca tapi tidak memicu
        // proses, jadi y menyimpan nilai lama (latch), bukan a&b.
        let m = with_sensitivity(
            comb_module(
                vec![sig("a", 8), sig("b", 8), sig("t", 8), sig("y", 8)],
                vec![0, 1],
                vec![3],
                vec![
                    assign(3, 8, mivon_ir::IrExpr::Signal(2, 8)),
                    assign(2, 8, and_expr(0, 1, 8)),
                ],
            ),
            &[0, 1],
        );
        match design_from_module(&m) {
            Err(msg) => assert!(msg.contains("sensitivity"), "{msg}"),
            Ok(d) => panic!("sensitivity tidak lengkap harus ditolak, dapat {d:?}"),
        }
    }

    #[test]
    fn design_from_module_allows_forward_reference_with_complete_sensitivity() {
        // Sensitivity lengkap ({a,b,t}) = proses bereaksi ulang -> fixed point,
        // sehingga urutan assignment tidak memengaruhi hasil (seperti @(*)).
        let chained = with_sensitivity(
            comb_module(
                vec![sig("a", 8), sig("b", 8), sig("t", 8), sig("y", 8)],
                vec![0, 1],
                vec![3],
                vec![
                    assign(3, 8, mivon_ir::IrExpr::Signal(2, 8)),
                    assign(2, 8, and_expr(0, 1, 8)),
                ],
            ),
            &[0, 1, 2],
        );
        let flat = with_sensitivity(
            comb_module(
                vec![sig("a", 8), sig("b", 8), sig("y", 8)],
                vec![0, 1],
                vec![2],
                vec![assign(2, 8, and_expr(0, 1, 8))],
            ),
            &[0, 1],
        );
        let a = design_from_module(&chained).expect("sensitivity lengkap boleh");
        let b = design_from_module(&flat).unwrap();
        assert_eq!(check_equivalence(&a, &b), SecOutcome::Equivalent);
    }

    #[test]
    fn design_from_module_rejects_latch_across_processes() {
        // Latch lintas proses: proses 1 menulis t, proses 2 membaca t tetapi
        // sensitivity-nya hanya {a} -> y menyimpan nilai lama.
        let mut m = with_sensitivity(
            comb_module(
                vec![sig("a", 8), sig("b", 8), sig("t", 8), sig("y", 8)],
                vec![0, 1],
                vec![3],
                vec![assign(3, 8, mivon_ir::IrExpr::Signal(2, 8))],
            ),
            &[0],
        );
        m.processes.insert(
            0,
            mivon_ir::Process::CombReactive {
                name: mivon_core::intern::Symbol::intern("always_comb"),
                sensitivity: Vec::new(),
                body: vec![assign(2, 8, and_expr(0, 1, 8))],
            },
        );
        match design_from_module(&m) {
            Err(msg) => assert!(msg.contains("sensitivity"), "{msg}"),
            Ok(d) => panic!("latch lintas proses harus ditolak, dapat {d:?}"),
        }
    }

    #[test]
    fn design_from_module_allows_forward_reference_in_always_comb() {
        // always_comb bereaksi ulang -> urutan tidak memengaruhi nilai tunak.
        let chained = reactive(comb_module(
            vec![sig("a", 8), sig("b", 8), sig("t", 8), sig("y", 8)],
            vec![0, 1],
            vec![3],
            vec![
                assign(3, 8, mivon_ir::IrExpr::Signal(2, 8)),
                assign(2, 8, and_expr(0, 1, 8)),
            ],
        ));
        let flat = reactive(comb_module(
            vec![sig("a", 8), sig("b", 8), sig("y", 8)],
            vec![0, 1],
            vec![2],
            vec![assign(2, 8, and_expr(0, 1, 8))],
        ));
        let a = design_from_module(&chained).expect("always_comb boleh referensi maju");
        let b = design_from_module(&flat).unwrap();
        assert_eq!(check_equivalence(&a, &b), SecOutcome::Equivalent);
    }

    #[test]
    fn design_from_module_skips_constant_initializer() {
        // `output logic y = 1'b0` membuat Process::Initial konstanta;
        // itu bukan state dan tidak boleh membuat desain kombinasional ditolak.
        let mut m = comb_module(
            vec![sig("a", 8), sig("y", 8)],
            vec![0],
            vec![1],
            vec![assign(1, 8, mivon_ir::IrExpr::Signal(0, 8))],
        );
        m.processes.insert(
            0,
            mivon_ir::Process::Initial {
                name: mivon_core::intern::Symbol::intern("port_init_0"),
                body: vec![assign(
                    1,
                    8,
                    mivon_ir::IrExpr::Const(mivon_core::LogicVec::from_u64(0, 8)),
                )],
            },
        );
        let d = design_from_module(&m).expect("initializer konstanta harus dilewati");
        assert_eq!(d.outputs[0].2, input("a"));

        // Initial non-konstanta tetap berarti state -> ditolak.
        let mut stateful = comb_module(
            vec![sig("a", 8), sig("y", 8)],
            vec![0],
            vec![1],
            vec![assign(1, 8, mivon_ir::IrExpr::Signal(0, 8))],
        );
        stateful.processes.insert(
            0,
            mivon_ir::Process::Initial {
                name: mivon_core::intern::Symbol::intern("initial_0"),
                body: vec![assign(1, 8, mivon_ir::IrExpr::Signal(0, 8))],
            },
        );
        match design_from_module(&stateful) {
            Err(msg) => assert!(msg.contains("kombinasional"), "{msg}"),
            Ok(d) => panic!("initial non-konstanta harus ditolak, dapat {d:?}"),
        }
    }

    #[test]
    fn design_from_module_rejects_undriven_internal_signal() {
        // t internal tanpa driver: kalau lolos, SEC membuat variabel bebas.
        let m = comb_module(
            vec![sig("a", 8), sig("t", 8), sig("y", 8)],
            vec![0],
            vec![2],
            vec![assign(2, 8, mivon_ir::IrExpr::Signal(1, 8))],
        );
        match design_from_module(&m) {
            Err(msg) => assert!(msg.contains("tidak punya driver"), "{msg}"),
            Ok(d) => panic!("sinyal internal tanpa driver harus ditolak, dapat {d:?}"),
        }
    }

    #[test]
    fn design_from_module_rejects_inout() {
        let mut m = comb_module(
            vec![sig("a", 8), sig("y", 8)],
            vec![0],
            vec![1],
            vec![assign(1, 8, mivon_ir::IrExpr::Signal(0, 8))],
        );
        m.inouts = vec![0];
        match design_from_module(&m) {
            Err(msg) => assert!(msg.contains("inout"), "{msg}"),
            Ok(d) => panic!("inout harus ditolak, dapat {d:?}"),
        }
    }

    #[test]
    fn design_from_module_lowers_cast_from_elaborator() {
        // Bentuk IR nyata `always_comb y = a + 1;`:
        //   Add(Cast{32, a8}, Signed(Const(1,32)))
        // Cast pada sinyal = resize murni; Signed(Const) = bit-pattern yang
        // sama untuk operator unsigned. Keduanya harus diterima.
        let m = comb_module(
            vec![sig("a", 8), sig("y", 8)],
            vec![0],
            vec![1],
            vec![assign(
                1,
                8,
                mivon_ir::IrExpr::BinaryOp(
                    mivon_ir::BinaryIrOp::Add,
                    Box::new(mivon_ir::IrExpr::Cast {
                        width: 32,
                        expr: Box::new(mivon_ir::IrExpr::Signal(0, 8)),
                    }),
                    Box::new(mivon_ir::IrExpr::Signed(Box::new(mivon_ir::IrExpr::Const(
                        mivon_core::LogicVec::from_u64(1, 32),
                    )))),
                ),
            )],
        );
        let d = design_from_module(&m).expect("Cast sinyal + Signed(Const) harus diterima");
        let sum = SecExpr::Add(
            Box::new(SecExpr::Resize(Box::new(input("a")), 32)),
            Box::new(SecExpr::Signed(Box::new(SecExpr::Const(1, 32)))),
        );
        // Lebar natural 32, output 8 bit -> di-resize ke lebar deklarasi.
        assert_eq!(d.outputs[0].2, SecExpr::Resize(Box::new(sum), 8));
    }

    #[test]
    fn design_from_module_rejects_widening_arithmetic_cast() {
        // `logic [15:0] s = a + b` (a,b 8 bit) = Cast{16, Add(a,b)}: SV
        // memakai context width 16 sehingga carry masuk bit 8, model SEC
        // natural-width tidak. Tolak, jangan beri Equivalent palsu.
        let m = comb_module(
            vec![sig("a", 8), sig("b", 8), sig("s", 16)],
            vec![0, 1],
            vec![2],
            vec![assign(
                2,
                16,
                mivon_ir::IrExpr::Cast {
                    width: 16,
                    expr: Box::new(and_expr(0, 1, 8)),
                },
            )],
        );
        match design_from_module(&m) {
            Err(msg) => assert!(msg.contains("context width"), "{msg}"),
            Ok(d) => panic!("Cast pelebar aritmetika harus ditolak, dapat {d:?}"),
        }
    }

    #[test]
    fn design_from_module_sign_extends_signed_signal_expression() {
        // $signed(a) 8 bit di-assign ke output 16 bit: SV sign-extend.
        let m = comb_module(
            vec![sig("a", 8), sig("y", 16)],
            vec![0],
            vec![1],
            vec![assign(
                1,
                16,
                mivon_ir::IrExpr::Signed(Box::new(mivon_ir::IrExpr::Signal(0, 8))),
            )],
        );
        let d = design_from_module(&m).expect("Signed(sinyal) harus dimodelkan");
        assert_eq!(
            d.outputs[0].2,
            SecExpr::Resize(Box::new(SecExpr::Signed(Box::new(input("a")))), 16)
        );
        // Pembanding: ekspresi signed yang di-fit ke 16 bit = sign-extend;
        // sedangkan Input(a) tanpa Signed = zero-extend (berbeda saat a >= 0x80).
        let sign_ext = design(&[("a", 8)], "y", 16, SecExpr::Signed(Box::new(input("a"))));
        let zero_ext = design(&[("a", 8)], "y", 16, input("a"));
        assert_eq!(check_equivalence(&d, &sign_ext), SecOutcome::Equivalent);
        assert!(matches!(
            check_equivalence(&d, &zero_ext),
            SecOutcome::NotEquivalent { .. }
        ));
    }

    #[test]
    fn design_from_module_skips_signed_const_initializer() {
        // `output logic [7:0] y = 0` -> Initial{rhs: Signed(Const(0,32))}.
        let mut m = comb_module(
            vec![sig("a", 8), sig("y", 8)],
            vec![0],
            vec![1],
            vec![assign(1, 8, mivon_ir::IrExpr::Signal(0, 8))],
        );
        m.processes.insert(
            0,
            mivon_ir::Process::Initial {
                name: mivon_core::intern::Symbol::intern("port_init_0"),
                body: vec![assign(
                    1,
                    8,
                    mivon_ir::IrExpr::Signed(Box::new(mivon_ir::IrExpr::Const(
                        mivon_core::LogicVec::from_u64(0, 32),
                    ))),
                )],
            },
        );
        let d = design_from_module(&m).expect("initializer Signed(Const) harus dilewati");
        assert_eq!(d.outputs[0].2, input("a"));
    }

    #[test]
    fn design_from_module_mixed_signed_operand_stays_unsigned() {
        // `localparam byte NEG = -1; y = a + NEG;` -> Add(a16, Signed(Const(0xFF,8))).
        // LRM §11.6.1: bila SALAH SATU operand unsigned, keduanya diperlakukan
        // unsigned -> NEG menjadi 255 (a+255), BUKAN a-1. Model harus sama;
        // memaksa a-1 di sini justru salah.
        let m = comb_module(
            vec![sig("a", 16), sig("y", 16)],
            vec![0],
            vec![1],
            vec![assign(
                1,
                16,
                mivon_ir::IrExpr::BinaryOp(
                    mivon_ir::BinaryIrOp::Add,
                    Box::new(mivon_ir::IrExpr::Signal(0, 16)),
                    Box::new(mivon_ir::IrExpr::Signed(Box::new(mivon_ir::IrExpr::Const(
                        mivon_core::LogicVec::from_u64(0xFF, 8),
                    )))),
                ),
            )],
        );
        let d = design_from_module(&m).expect("Signed(Const) harus dimodelkan");
        let plus_255 = design(
            &[("a", 16)],
            "y",
            16,
            SecExpr::Add(Box::new(input("a")), Box::new(SecExpr::Const(0x00FF, 16))),
        );
        // a - 1 (sign-extend) harus TERBUKTI berbeda dari model SV.
        let minus_one = design(
            &[("a", 16)],
            "y",
            16,
            SecExpr::Add(Box::new(input("a")), Box::new(SecExpr::Const(0xFFFF, 16))),
        );
        assert_eq!(check_equivalence(&d, &plus_255), SecOutcome::Equivalent);
        assert!(matches!(
            check_equivalence(&d, &minus_one),
            SecOutcome::NotEquivalent { .. }
        ));
    }

    #[test]
    fn design_from_module_sign_extends_signed_assignment() {
        // RHS signed yang di-assign ke target lebih lebar: SV sign-extend.
        // `y = NEG` (byte -1) -> 16'hFFFF.
        let m = comb_module(
            vec![sig("y", 16)],
            vec![],
            vec![0],
            vec![assign(
                0,
                16,
                mivon_ir::IrExpr::Signed(Box::new(mivon_ir::IrExpr::Const(
                    mivon_core::LogicVec::from_u64(0xFF, 8),
                ))),
            )],
        );
        let d = design_from_module(&m).expect("Signed(Const) harus dimodelkan");
        let sign_ext = design(&[], "y", 16, SecExpr::Const(0xFFFF, 16));
        let zero_ext = design(&[], "y", 16, SecExpr::Const(0x00FF, 16));
        assert_eq!(check_equivalence(&d, &sign_ext), SecOutcome::Equivalent);
        assert!(matches!(
            check_equivalence(&d, &zero_ext),
            SecOutcome::NotEquivalent { .. }
        ));
    }

    #[test]
    fn design_from_module_accepts_nonnegative_signed_const() {
        // MSB 0: zero-extend == sign-extend, jadi aman diterima.
        let m = comb_module(
            vec![sig("a", 16), sig("y", 16)],
            vec![0],
            vec![1],
            vec![assign(
                1,
                16,
                mivon_ir::IrExpr::BinaryOp(
                    mivon_ir::BinaryIrOp::Add,
                    Box::new(mivon_ir::IrExpr::Signal(0, 16)),
                    Box::new(mivon_ir::IrExpr::Signed(Box::new(mivon_ir::IrExpr::Const(
                        mivon_core::LogicVec::from_u64(0x7F, 8),
                    )))),
                ),
            )],
        );
        let d = design_from_module(&m).expect("konstanta signed non-negatif harus diterima");
        assert_eq!(
            d.outputs[0].2,
            SecExpr::Add(
                Box::new(input("a")),
                Box::new(SecExpr::Signed(Box::new(SecExpr::Const(0x7F, 8)))),
            )
        );
    }

    #[test]
    fn design_from_module_rejects_neg_widening() {
        // `y8 = -4'sd1`: Neg dihitung 4 bit lalu dilebarkan. Negasi tidak
        // komutatif dengan ekstensi (nilai minimum overflow: -4'sd8 -> 8),
        // jadi harus Err context width, bukan ditebak 0xFF/0x0F.
        let m = comb_module(
            vec![sig("y", 8)],
            vec![],
            vec![0],
            vec![assign(
                0,
                8,
                mivon_ir::IrExpr::UnaryOp(
                    mivon_ir::UnaryIrOp::Minus,
                    Box::new(mivon_ir::IrExpr::Signed(Box::new(mivon_ir::IrExpr::Const(
                        mivon_core::LogicVec::from_u64(1, 4),
                    )))),
                ),
            )],
        );
        match design_from_module(&m) {
            Err(msg) => assert!(msg.contains("context width"), "{msg}"),
            Ok(d) => panic!("Neg yang dilebarkan harus ditolak, dapat {d:?}"),
        }
    }

    #[test]
    fn design_from_module_rejects_signed_arithmetic_needing_context_width() {
        // `y = 4'sd7 + 4'sd1`: SV memakai lebar konteks 8 (7+1 = 0x08),
        // sedangkan model natural-width menghitung 4 bit lalu melebar.
        // Sign-extend hasil 4 bit (0xF8) SALAH, jadi harus Err.
        let m = comb_module(
            vec![sig("y", 8)],
            vec![],
            vec![0],
            vec![assign(
                0,
                8,
                mivon_ir::IrExpr::BinaryOp(
                    mivon_ir::BinaryIrOp::Add,
                    Box::new(mivon_ir::IrExpr::Signed(Box::new(mivon_ir::IrExpr::Const(
                        mivon_core::LogicVec::from_u64(7, 4),
                    )))),
                    Box::new(mivon_ir::IrExpr::Signed(Box::new(mivon_ir::IrExpr::Const(
                        mivon_core::LogicVec::from_u64(1, 4),
                    )))),
                ),
            )],
        );
        match design_from_module(&m) {
            Err(msg) => assert!(msg.contains("context width"), "{msg}"),
            Ok(d) => panic!("Add signed yang butuh context width harus ditolak, dapat {d:?}"),
        }
    }

    #[test]
    fn design_from_module_unsigned_signal_strips_signedness() {
        // t (unsigned 8 bit) = $signed(a); y (16 bit) = t.
        // Assignment ke sinyal unsigned menghapus tanda, jadi y zero-extend
        // (0x0080), BUKAN sign-extend (0xFF80).
        let m = comb_module(
            vec![sig("a", 8), sig("t", 8), sig("y", 16)],
            vec![0],
            vec![2],
            vec![
                assign(
                    1,
                    8,
                    mivon_ir::IrExpr::Signed(Box::new(mivon_ir::IrExpr::Signal(0, 8))),
                ),
                assign(2, 16, mivon_ir::IrExpr::Signal(1, 8)),
            ],
        );
        let d = design_from_module(&m).expect("sinyal unsigned harus menghapus tanda");
        let zero_ext = design(&[("a", 8)], "y", 16, input("a"));
        let sign_ext = design(&[("a", 8)], "y", 16, SecExpr::Signed(Box::new(input("a"))));
        assert_eq!(check_equivalence(&d, &zero_ext), SecOutcome::Equivalent);
        assert!(matches!(
            check_equivalence(&d, &sign_ext),
            SecOutcome::NotEquivalent { .. }
        ));
    }

    #[test]
    fn design_from_module_widen_sign_extends_both_signed() {
        // Add(Signed(4'hF), Signed(8'h01)): kedua operand signed -> 4-bit
        // 0xF di-sign-extend ke 0xFF, +1 = 0x00 (8 bit). Zero-extend akan
        // memberi 0x10 — beda, jadi test ini mengunci aturan widen §11.6.1.
        let m = comb_module(
            vec![sig("y", 8)],
            vec![],
            vec![0],
            vec![assign(
                0,
                8,
                mivon_ir::IrExpr::BinaryOp(
                    mivon_ir::BinaryIrOp::Add,
                    Box::new(mivon_ir::IrExpr::Signed(Box::new(mivon_ir::IrExpr::Const(
                        mivon_core::LogicVec::from_u64(0xF, 4),
                    )))),
                    Box::new(mivon_ir::IrExpr::Signed(Box::new(mivon_ir::IrExpr::Const(
                        mivon_core::LogicVec::from_u64(1, 8),
                    )))),
                ),
            )],
        );
        let d = design_from_module(&m).expect("Add signed beda lebar harus dimodelkan");
        let sign_ext = design(&[], "y", 8, SecExpr::Const(0x00, 8));
        let zero_ext = design(&[], "y", 8, SecExpr::Const(0x10, 8));
        assert_eq!(check_equivalence(&d, &sign_ext), SecOutcome::Equivalent);
        assert!(matches!(
            check_equivalence(&d, &zero_ext),
            SecOutcome::NotEquivalent { .. }
        ));
    }

    #[test]
    fn signed_resize_of_arithmetic_is_unsupported() {
        // Desain rakitan tangan: Resize pelebar atas nilai signed hasil Add
        // tidak boleh diam-diam sign-extend (butuh context width) -> Unsupported.
        let sum = SecExpr::Add(
            Box::new(SecExpr::Signed(Box::new(SecExpr::Const(7, 4)))),
            Box::new(SecExpr::Signed(Box::new(SecExpr::Const(1, 4)))),
        );
        let d = design(&[], "y", 8, SecExpr::Resize(Box::new(sum), 8));
        match check_equivalence(&d, &d) {
            SecOutcome::Unsupported(msg) => assert!(msg.contains("context width"), "{msg}"),
            other => panic!("harus Unsupported, dapat {other:?}"),
        }
    }

    #[test]
    fn design_from_module_rejects_unsigned_widening_arithmetic() {
        // `logic [15:0] y = a + b` (a,b 8 bit): SV memakai konteks 16 sehingga
        // carry masuk bit 8. Model natural-width tidak, jadi harus Err.
        let m = comb_module(
            vec![sig("a", 8), sig("b", 8), sig("y", 16)],
            vec![0, 1],
            vec![2],
            vec![assign(
                2,
                16,
                mivon_ir::IrExpr::BinaryOp(
                    mivon_ir::BinaryIrOp::Add,
                    Box::new(mivon_ir::IrExpr::Signal(0, 8)),
                    Box::new(mivon_ir::IrExpr::Signal(1, 8)),
                ),
            )],
        );
        match design_from_module(&m) {
            Err(msg) => assert!(msg.contains("context width"), "{msg}"),
            Ok(d) => panic!("Add unsigned yang melebar harus ditolak, dapat {d:?}"),
        }
    }

    #[test]
    fn design_from_module_allows_unsigned_bitwise_widening() {
        // Bitwise komutatif dengan zero-extension, jadi pelebaran AMAN.
        let m = comb_module(
            vec![sig("a", 8), sig("b", 8), sig("y", 16)],
            vec![0, 1],
            vec![2],
            vec![assign(2, 16, and_expr(0, 1, 8))],
        );
        let d = design_from_module(&m).expect("AND yang melebar harus diterima");
        let expected = design(&[("a", 8), ("b", 8)], "y", 16, and(input("a"), input("b")));
        assert_eq!(check_equivalence(&d, &expected), SecOutcome::Equivalent);
    }

    #[test]
    fn design_from_module_rejects_arithmetic_operand_widened_by_mux() {
        // Mux(s, a8+b8, 16'h0): cabang Add harus dilebarkan ke 16 -> carry
        // hilang bila dihitung 8 bit, jadi widen() menolaknya.
        let m = comb_module(
            vec![sig("s", 1), sig("a", 8), sig("b", 8), sig("y", 16)],
            vec![0, 1, 2],
            vec![3],
            vec![assign(
                3,
                16,
                mivon_ir::IrExpr::Cond(
                    Box::new(mivon_ir::IrExpr::Signal(0, 1)),
                    Box::new(mivon_ir::IrExpr::BinaryOp(
                        mivon_ir::BinaryIrOp::Add,
                        Box::new(mivon_ir::IrExpr::Signal(1, 8)),
                        Box::new(mivon_ir::IrExpr::Signal(2, 8)),
                    )),
                    Box::new(mivon_ir::IrExpr::Const(mivon_core::LogicVec::from_u64(
                        0, 16,
                    ))),
                ),
            )],
        );
        // Ekstraksi bisa berhasil; penolakan terjadi saat evaluasi (widen).
        let d = design_from_module(&m).expect("ekstraksi boleh berhasil");
        match check_equivalence(&d, &d) {
            SecOutcome::Unsupported(msg) => assert!(msg.contains("context width"), "{msg}"),
            other => panic!("harus Unsupported, dapat {other:?}"),
        }
    }

    #[test]
    fn design_from_module_rejects_unsigned_not_widening() {
        // `y16 = ~a8`: SV = ~zero_ext(a) (0xFFxx), model = zero_ext(~a) (0x00xx).
        let m = comb_module(
            vec![sig("a", 8), sig("y", 16)],
            vec![0],
            vec![1],
            vec![assign(
                1,
                16,
                mivon_ir::IrExpr::UnaryOp(
                    mivon_ir::UnaryIrOp::BitNot,
                    Box::new(mivon_ir::IrExpr::Signal(0, 8)),
                ),
            )],
        );
        match design_from_module(&m) {
            Err(msg) => assert!(msg.contains("context width"), "{msg}"),
            Ok(d) => panic!("Not unsigned yang dilebarkan harus ditolak, dapat {d:?}"),
        }
    }

    #[test]
    fn design_from_module_allows_signed_not_widening() {
        // `~` komutatif dengan sign-extension, jadi Not atas nilai signed aman.
        let m = comb_module(
            vec![sig("a", 8), sig("y", 16)],
            vec![0],
            vec![1],
            vec![assign(
                1,
                16,
                mivon_ir::IrExpr::UnaryOp(
                    mivon_ir::UnaryIrOp::BitNot,
                    Box::new(mivon_ir::IrExpr::Signed(Box::new(
                        mivon_ir::IrExpr::Signal(0, 8),
                    ))),
                ),
            )],
        );
        let d = design_from_module(&m).expect("Not signed boleh melebar");
        // a = 0x80 -> sign_ext 0xFF80 -> ~ = 0x007F.
        let expected = design(
            &[("a", 8)],
            "y",
            16,
            SecExpr::Not(Box::new(SecExpr::Signed(Box::new(input("a"))))),
        );
        assert_eq!(check_equivalence(&d, &expected), SecOutcome::Equivalent);
    }

    #[test]
    fn design_from_module_rejects_signed_not_mixed_widening() {
        // `~$signed(a8) & c16`: operand lain unsigned -> ekstensi ZERO, dan
        // `~` tidak komutatif dengan zero-extension (SV: 0xFFF0, model 0x00F0).
        let m = comb_module(
            vec![sig("a", 8), sig("c", 16), sig("y", 16)],
            vec![0, 1],
            vec![2],
            vec![assign(
                2,
                16,
                mivon_ir::IrExpr::BinaryOp(
                    mivon_ir::BinaryIrOp::BitAnd,
                    Box::new(mivon_ir::IrExpr::UnaryOp(
                        mivon_ir::UnaryIrOp::BitNot,
                        Box::new(mivon_ir::IrExpr::Signed(Box::new(
                            mivon_ir::IrExpr::Signal(0, 8),
                        ))),
                    )),
                    Box::new(mivon_ir::IrExpr::Signal(1, 16)),
                ),
            )],
        );
        let d = design_from_module(&m).expect("ekstraksi boleh berhasil");
        match check_equivalence(&d, &d) {
            SecOutcome::Unsupported(msg) => assert!(msg.contains("context width"), "{msg}"),
            other => panic!("Not signed yang di-zero-extend harus ditolak, dapat {other:?}"),
        }
    }
}
