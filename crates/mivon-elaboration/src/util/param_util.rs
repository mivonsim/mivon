//! ──────────────────────────────────────────────────────────────────────────────
//! CATATAN: File ini adalah bagian dari pemisahan util.rs (SRP Refactoring).
//! Tanggung jawab: Parameter resolution utilities.
//!
//! Fungsi:
//!   - collect_body_params()    — kumpulkan parameter dari body module
//!   - resolve_param_values_fn() — resolusi nilai parameter dengan override
//!
//! ──────────────────────────────────────────────────────────────────────────────

use std::collections::HashMap;

use mivon_ast::const_eval_ext::{eval_cval_full, eval_param_default_full, CVal, SField};
use mivon_ast::types::const_eval_with_params;
use mivon_ast::types::string_to_i64;
use mivon_ast::types::PackageItem;
use mivon_ast::*;
use mivon_core::intern::Symbol;

use super::const_eval_params;

/// ELAB-13: lebar bit deklarasi param. Urutan prioritas: range eksplisit
/// (parameter [7:0] P), tipe eksplisit (int/byte/...), lalu lebar literal
/// default (8'd1). None = tidak diketahui (pemakai memakai 64 bit).
pub(crate) fn param_width_of(p: &ParamDecl, params: &HashMap<Symbol, i64>) -> Option<usize> {
    if let Some((msb, lsb)) = &p.range {
        let (m, l) = match (
            const_eval_params(msb, params),
            const_eval_params(lsb, params),
        ) {
            (Ok(m), Ok(l)) => (m, l),
            _ => return None,
        };
        let w = (m.max(l).saturating_sub(m.min(l)) as usize).saturating_add(1);
        return (w <= 64).then_some(w);
    }
    if let Some(dt) = &p.dtype {
        if let Some(w) = dtype_width(dt) {
            return (w <= 64).then_some(w);
        }
        // Tipe eksplisit yang lebarnya tidak diketahui (typedef/enum/signed
        // tanpa range) -> jangan menebak dari literal default (bisa menyempit).
        return None;
    }
    let declared = match p.default.as_ref().and_then(default_literal_width) {
        Some(w) => w,
        None => return None,
    };
    // Lebar dari literal default hanya inferensi: override (`#(.P(16'hABCD))`)
    // membawa tipe sendiri, dan lebarnya TIDAK ikut di peta i64. Untuk param
    // untyped yang signedness-nya hanya inferensi, lebar deklarasi hanya boleh
    // mengikat bila nilai efektif memang berasal dari default itu (tanpa
    // override); kalau tidak, nilai override bisa ter-sign-extend salah.
    if param_decl_signed(p) {
        // Bandingkan nilai default SETELAH truncate+sign-extend ke lebar
        // deklarasinya: `parameter signed P8 = 8'hFF` punya default mentah
        // 255 tapi nilai efektif -1 (ELAB-13 (11)). Tanpa normalisasi ini
        // guard menganggapnya override → lebar hilang → generate-if
        // mengevaluasi P8 < 0 sebagai unsigned (cabang salah).
        let default_val = p
            .default
            .as_ref()
            .and_then(|e| const_eval_with_params(e, &HashMap::new()).ok())
            .map(|dv| truncate_sign(dv, declared));
        match (default_val, params.get(&p.name).copied()) {
            (Some(dv), Some(v)) if dv == v => {}
            _ => return None,
        }
        return (declared <= 64).then_some(declared);
    }
    let w = match params.get(&p.name) {
        // Lebar tipe NILAI efektif (override / default). Nilai negatif
        // bertipe `int` minimal 32 bit — `parameter unsigned U = -1` tetap
        // 32 bit (0xFFFFFFFF), bukan 64 bit hasil sign-extend mentah.
        // Tanpa aturan ini `U == 32'hFFFFFFFF` salah jadi 0 (ELAB-13 (11)).
        Some(&v) => declared.max(value_type_width(v)),
        None => declared,
    };
    (w <= 64).then_some(w)
}

/// ELAB-13: lebar self-determined literal default param. Literal tak berlebar
/// (desimal maupun basis) = 32 bit — LRM 6.20.2: parameter tanpa range/tipe
/// mengambil tipe nilainya, desimal tak berlebar = `int` (32 bit signed),
/// literal basis tak berlebar = 32 bit unsigned. Unary +/- dan tanda kurung
/// tidak mengubah lebar.
fn default_literal_width(e: &Expr) -> Option<usize> {
    match e {
        Expr::Value(Value::Binary { width, .. })
        | Expr::Value(Value::Hex { width, .. })
        | Expr::Value(Value::Octal { width, .. }) => Some(width.unwrap_or(32)),
        Expr::Value(Value::Decimal(v)) => {
            // LRM 5.7.1: literal desimal tak berlebar minimal 32 bit dan
            // MELEBAR bila nilainya tidak muat (iverilog: 4000000000 -> 33
            // bit, 10^12 -> 41 bit). Konstanta 32 memotong nilai >= 2^31.
            Some(signed_bits(*v).max(32))
        }
        Expr::UnaryOp {
            op: UnaryOp::Minus | UnaryOp::Plus,
            expr,
        } => default_literal_width(expr),
        Expr::Paren(inner) => default_literal_width(inner),
        _ => None,
    }
}

/// ELAB-13/14: lebar deklarasi param TANPA melihat nilai efektif — dipakai
/// untuk menyemai scope lebar const-fold SEBELUM nilai param di-resolve
/// (localparam/generate-if di fase prepass). Override tetap mengikuti lebar
/// deklarasi di sini (caveat (13) ELAB-13).
pub(crate) fn param_static_width(p: &ParamDecl, params: &HashMap<Symbol, i64>) -> Option<usize> {
    if let Some((msb, lsb)) = &p.range {
        let (m, l) = match (
            const_eval_params(msb, params),
            const_eval_params(lsb, params),
        ) {
            (Ok(m), Ok(l)) => (m, l),
            _ => return None,
        };
        let w = (m.max(l).saturating_sub(m.min(l)) as usize).saturating_add(1);
        return (w <= 64).then_some(w);
    }
    if let Some(dt) = &p.dtype {
        return dtype_width(dt).filter(|w| *w <= 64);
    }
    p.default
        .as_ref()
        .and_then(default_literal_width)
        .filter(|w| *w <= 64)
}

/// ELAB-13/14: peta lebar STATIS seluruh param module (header `module.params`
/// + param di body/generate). Dipakai menyemai scope const-fold sebelum nilai
/// param di-resolve. Nilai peta = (lebar, signed) — signedness dibutuhkan
/// perbandingan const-fold (`parameter signed [7:0] S8 == -1`).
pub(crate) fn static_param_widths(
    module: &Module,
    params: &HashMap<Symbol, i64>,
) -> HashMap<Symbol, (usize, bool)> {
    let mut out = HashMap::new();
    for p in module
        .params
        .iter()
        .chain(collect_body_params(module).iter())
    {
        if let Some(w) = param_static_width(p, params) {
            out.insert(p.name, (w, param_decl_signed(p)));
        }
    }
    out
}

/// ELAB-13: jumlah bit minimum agar `v` muat sebagai bilangan bertanda.
fn signed_bits(v: i64) -> usize {
    let magnitude = if v >= 0 { v as u64 } else { !(v as u64) };
    (64 - magnitude.leading_zeros() as usize + 1).min(64)
}

/// ELAB-13: signedness deklarasi param. Range tanpa dtype = unsigned
/// (LRM 6.8/6.20.2); tipe eksplisit menang; sisanya ikut default expression.
pub(crate) fn param_decl_signed(p: &ParamDecl) -> bool {
    // ELAB-13 (11): `parameter signed/unsigned P` tanpa tipe eksplisit.
    if let Some(sig) = p.implicit_sign {
        return sig;
    }
    if let Some(dt) = &p.dtype {
        return mivon_ast::types::is_signed_type(dt);
    }
    if p.range.is_some() {
        return false;
    }
    match &p.default {
        Some(expr) => crate::elaborator::sign::elab_expr_is_signed(expr),
        None => true, // implicit integer
    }
}

/// ELAB-13: lebar minimal untuk merepresentasikan nilai (negatif = 64 bit).
fn value_width(v: i64) -> usize {
    let u = v as u64;
    if u == 0 {
        1
    } else {
        64 - u.leading_zeros() as usize
    }
}

/// ELAB-13: lebar TIPE nilai i64 (untuk override yang lebarnya tidak ikut di
/// peta i64). Non-negatif = bit minimum (literal `16'hABCD` → 16); negatif =
/// literal desimal bertanda, minimal `int` 32 bit (`-1` → 32). Diverifikasi
/// iverilog 12: `parameter P = 8'd1` + `#(.P(16'hABCD))` → 16 bit, `#(.P(-1))`
/// → 32 bit.
fn value_type_width(v: i64) -> usize {
    if v >= 0 {
        value_width(v)
    } else {
        signed_bits(v).max(32)
    }
}

/// ELAB-13 (11): truncate ke `w` bit lalu sign-extend (`w >= 64` = no-op).
fn truncate_sign(v: i64, w: usize) -> i64 {
    if w == 0 || w >= 64 {
        return v;
    }
    let mask = (1u64 << w) - 1;
    let raw = v as u64 & mask;
    if raw & (1u64 << (w - 1)) != 0 {
        (raw | !mask) as i64
    } else {
        raw as i64
    }
}

/// ELAB-13: normalisasi nilai param UNSIGNED ke lebar deklarasinya. Dipakai
/// saat mengisi peta nilai agar SEMUA jalur (const-fold localparam, generate
/// if/for, IR) melihat nilai yang sama: parameter [7:0] D = -1 -> 255.
///
/// Varian ini untuk nilai DEFAULT: lebar deklarasi (termasuk inferensi dari
/// literal default) mengikat, sehingga `parameter signed P8 = 8'hFF` menjadi
/// -1 dan generate-if `P8 < 0` benar (ELAB-13 (11)).
pub(crate) fn normalize_param_value(p: &ParamDecl, v: i64, params: &HashMap<Symbol, i64>) -> i64 {
    match param_static_width(p, params) {
        Some(w) => normalize_with_width(p, v, w),
        None => v,
    }
}

/// ELAB-13: normalisasi nilai OVERRIDE instance. Untuk param TANPA
/// range/tipe, lebar/signedness default TIDAK mengikat: override membawa
/// tipenya sendiri (iverilog 12: `parameter P = 8'sd1` + `#(.P(16'hABCD))`
/// → P 16 bit UNSIGNED, `assign y = P` 32 bit = 0x0000ABCD, bukan
/// 0xFFFFABCD). Range/tipe eksplisit tetap mengikat.
pub(crate) fn normalize_override_value(
    p: &ParamDecl,
    v: i64,
    params: &HashMap<Symbol, i64>,
) -> i64 {
    match param_width_of(p, params) {
        Some(w) => normalize_with_width(p, v, w),
        None => v,
    }
}

/// Truncate ke `w` (dilebarkan dulu ke lebar tipe nilai untuk param untyped)
/// lalu sign-extend bila deklarasinya signed.
fn normalize_with_width(p: &ParamDecl, v: i64, mut w: usize) -> i64 {
    // Lebar dari literal default hanya inferensi: nilai yang lebih lebar
    // (override) tidak boleh terpotong. Range/tipe eksplisit mengikat.
    if p.range.is_none() && p.dtype.is_none() {
        w = w.max(value_type_width(v)).min(64);
    }
    if w == 0 || w >= 64 {
        return v;
    }
    let mask = (1u64 << w) - 1;
    let raw = v as u64 & mask;
    if param_decl_signed(p) {
        // Param signed: truncate lalu sign-extend, supaya
        // `parameter signed [7:0] S = 8'hFF` bernilai -1 (bukan +255) di
        // const-fold dan generate-if.
        let sign_bit = 1u64 << (w - 1);
        if raw & sign_bit != 0 {
            return (raw | !mask) as i64;
        }
    }
    raw as i64
}

/// ELAB-13: lebar bit tipe data eksplisit (None = tipe tidak menentukan lebar).
/// `Signed(inner)` mewarisi lebar inner — `parameter signed int SI` = 32 bit,
/// sehingga normalisasi bisa truncate + sign-extend (butir (11)).
fn dtype_width(dt: &DataType) -> Option<usize> {
    match dt {
        DataType::Bit | DataType::Logic => Some(1),
        DataType::Byte => Some(8),
        DataType::Shortint => Some(16),
        DataType::Int | DataType::Integer => Some(32),
        DataType::Longint | DataType::Time => Some(64),
        DataType::Signed(inner) => dtype_width(inner),
        _ => None,
    }
}
/// Kumpulkan parameter dari body module (termasuk yang ada di dalam generate
/// block — `Items`, `if`, `for`, dan `case`, rekursif).
pub fn collect_body_params(module: &Module) -> Vec<ParamDecl> {
    let mut params = Vec::new();
    collect_params_in_items(&module.items, &mut params);
    params
}

fn collect_params_in_items(items: &[ModuleItem], params: &mut Vec<ParamDecl>) {
    for item in items {
        match item {
            ModuleItem::Param(p) => params.push(p.clone()),
            ModuleItem::Generate(gen) => {
                for gi in &gen.items {
                    match gi {
                        GenerateItem::Items(items) => collect_params_in_items(items, params),
                        GenerateItem::If {
                            true_items,
                            false_items,
                            ..
                        } => {
                            collect_params_in_items(true_items, params);
                            collect_params_in_items(false_items, params);
                        }
                        GenerateItem::For { body_items, .. } => {
                            collect_params_in_items(body_items, params);
                        }
                        GenerateItem::Case { items, default, .. } => {
                            for ci in items {
                                collect_params_in_items(&ci.body, params);
                            }
                            if let Some(d) = default {
                                collect_params_in_items(d, params);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Apakah default param berupa koleksi (array literal Concat multi-elemen /
/// struct literal) yang TIDAK boleh didaftarkan sebagai skalar fallback.
/// `eval_param_default` mengembalikan None untuk keduanya; guard ini
/// mencegah fallback 1 meng-klaim nama yang sebenarnya array/struct (yang
/// didaftarkan lewat jalur flatten array / evaluator struct terpisah).
fn param_default_is_collection(e: &Expr) -> bool {
    matches!(e, Expr::Concat(parts) if parts.len() > 1) || matches!(e, Expr::StructLit { .. })
}

/// Konteks package untuk evaluasi PENUH default param: konstanta package
/// ter-evaluasi (qualified `pkg::name` — skalar & array, hasil
/// `eval_package_constants`) dan `package_symbols` (typedef/fungsi package).
/// Dipakai `resolve_param_values_with_ctx` sebagai fallback saat evaluator
/// skalar sederhana gagal — mis. `$bits(typedef)` atau inlining fungsi package
/// seperti `otbn_pkg::SecAddRandWidth`, yang tidak bisa dievaluasi oleh
/// `const_eval_with_params`.
pub struct PkgFullCtx<'a> {
    pub scalars: &'a HashMap<Symbol, i64>,
    pub arrays: &'a HashMap<Symbol, Vec<i64>>,
    pub package_symbols: &'a HashMap<Symbol, HashMap<Symbol, PackageItem>>,
    /// Nilai struct localparam/parameter (assignment pattern) agar
    /// `name.field` bisa di-const-eval (mis. `Info.size`).
    pub structs: &'a HashMap<Symbol, Vec<SField>>,
    /// Array-of-struct package parameter (key qualified `pkg::name` →
    /// vec of struct fields per element). Dipakai saat array indexing
    /// `pkg::ARRAY[idx]` untuk mengembalikan struct utuh.
    pub array_structs: &'a HashMap<Symbol, Vec<Vec<SField>>>,
}

/// Evaluasi default param menjadi `Some(i64)` hanya bila itu konstanta
/// SKALAR. Localparam ARRAY (`localparam logic [63:0] RC [24] = '{...}` —
/// default diparse sebagai `Expr::Concat` multi-elemen) mengembalikan None
/// agar TIDAK masuk param_vals sebagai skalar 0 — kalau masuk, `RC[rnd]`
/// salah di-resolve sebagai bit-select lebar 1 dan `RC[rnd][63:0]` gagal
/// dengan "range select out of bounds: 63:0 on width 1". Array param
/// didaftarkan sebagai signal const array oleh elaborator (lihat
/// elaborate_module_with_params_and_type).
///
/// Jalur cepat `const_eval_with_params` (skalar + $clog2 + vbits + operator),
/// lalu fallback ke evaluator penuh (`$bits(typedef)`, fungsi package, scoped
/// ident) bila konteks package tersedia.
///
/// `existing_vals` berisi konteks skalar penuh: pemanggil menjamin
/// `base_ctx ⊇ pkg_param_ctx ⊇ pkg_const_scalars` (collect_package_param_ctx
/// meng-clone pkg_param_ctx yang sudah meng-flatten konstanta package), jadi
/// `existing_vals` bisa dipakai langsung sebagai `merged` untuk evaluator
/// penuh — TIDAK perlu meng-clone ulang `pkg.scalars` + merge base_ctx per
/// module (bottleneck ~40k+63k insert per module di OpenTitan).
fn eval_param_default(
    e: &Expr,
    existing_vals: &HashMap<Symbol, i64>,
    pkg: Option<&PkgFullCtx>,
    module: &Module,
) -> Option<i64> {
    // `$bits(ParamIdent)` (dan `$size`) harus LEBAR tipe parameter, bukan
    // NILAI konstanta/override-nya. const_eval standard mengevaluasi argumen
    // sebagai nilai → `parameter bit [63:0] ScrNonce = '0` di-override
    // konstanta besar (desain) membuat `$bits(ScrNonce)` = nilai besar
    // (bukan 64) → TotalAnchorWidth raksasa → E3012. Fold dulu di level ini
    // (konteks module tersedia) agar SEMUA jalur eval berikutnya benar.
    let e_folded = fold_param_bits(e, module, existing_vals);
    match &e_folded {
        Expr::Concat(parts) if parts.len() > 1 => None,
        // JANGAN masukkan struct literal ke param_vals sebagai skalar 0.
        // `const_eval_with_params` mengembalikan Ok(0) untuk `Expr::StructLit`
        // (agar array-of-struct lama tetap terdaftar), tapi untuk struct
        // NON-array (mis. `localparam info_t Info = '{size: ..., off: ...};`)
        // nilai 0 itu membuat loop body-param di elaborator melewati `Info`
        // (sudah ada di effective_params) sehingga `struct_vals` tidak pernah
        // diisi dan `Info.size` gagal dengan "member access on non-struct".
        // Serahkan ke evaluator struct (eval_cval_full) di loop body-param.
        Expr::StructLit { .. } => None,
        Expr::String(s) => Some(string_to_i64(s)),
        _ => {
            if let Ok(v) = const_eval_with_params(&e_folded, existing_vals) {
                return Some(v);
            }
            if let Some(p) = pkg {
                return eval_param_default_full(
                    &e_folded,
                    existing_vals,
                    p.arrays,
                    p.package_symbols,
                    p.structs,
                    p.array_structs,
                );
            }
            None
        }
    }
}

/// Lebar satu deklarasi param (ELAB-13 butir (7)/(15)).
///
/// Prioritas: range eksplisit (`[127:0]` → 128, TANPA cap) → lebar dtype
/// (`DataType::width`: `signed int` → 32, typedef → 64) → bila
/// `infer_default` dan tidak ada range/dtype, lebar tipe nilai literal
/// default (`8'd1` → 8, `1` → 32).
fn param_decl_width_one(
    p: &ParamDecl,
    vals: &HashMap<Symbol, i64>,
    infer_default: bool,
) -> Option<usize> {
    // ParamDecl.range = (lo, hi) ekspresi — lebar = |hi - lo| + 1.
    if let Some((lo, hi)) = &p.range {
        if let (Ok(msb), Ok(lsb)) = (
            const_eval_with_params(hi, vals),
            const_eval_with_params(lo, vals),
        ) {
            let w = msb.abs_diff(lsb) as usize + 1;
            if w > 0 {
                return Some(w);
            }
        }
    }
    if let Some(dt) = &p.dtype {
        let w = mivon_ast::types::DataType::width(dt);
        if w > 0 {
            return Some(w);
        }
        // Tipe eksplisit tanpa lebar diketahui: jangan menebak dari literal.
        return None;
    }
    if !infer_default {
        return None;
    }
    // Tanpa range/dtype: lebar tipe nilai default. Literal integer → lebar
    // literal. Default non-literal (`parameter A = 4+4`) belum terinferensi
    // (ELAB-13 butir (3), masih OPEN) → fallback 64 SERAGAM di ketiga jalur
    // (IR/localparam/generate-if), bukan min-bits nilai yang berbeda-beda.
    // Override TIDAK melebarkan lebar deklarasi di sini: tipe param hasil
    // override ditentukan tipe ekspresi override (LRM 6.20.2) yang lebarnya
    // belum dipropagasi ke peta i64 (butir (13), masih OPEN).
    Some(
        p.default
            .as_ref()
            .and_then(default_literal_width)
            .unwrap_or(64),
    )
}

/// Lebar `$bits` satu deklarasi param — dipakai elaborator untuk peta
/// `param_bits_widths` (jalur IR) dan const-fold (generate-if/localparam).
pub(crate) fn param_decl_bits_width(p: &ParamDecl, vals: &HashMap<Symbol, i64>) -> Option<usize> {
    param_decl_width_one(p, vals, true)
}

/// Lebar tipe parameter module (untuk `$bits(ParamIdent)`/`$size(ParamIdent)`).
/// `infer_default` (hanya untuk `$bits`): lihat `param_decl_width_one`.
/// `$size` sengaja TIDAK memakai inferensi default (perilaku lama).
fn param_decl_width(
    module: &Module,
    name: &Symbol,
    vals: &HashMap<Symbol, i64>,
    infer_default: bool,
) -> Option<usize> {
    module
        .params
        .iter()
        .find(|p| p.name == *name)
        .and_then(|p| param_decl_width_one(p, vals, infer_default))
        .or_else(|| {
            collect_body_params(module)
                .iter()
                .find(|p| p.name == *name)
                .and_then(|p| param_decl_width_one(p, vals, infer_default))
        })
}

/// Ganti `$bits(Ident)`/`$size(Ident)` pada ekspresi dengan konstanta = lebar
/// tipe parameter module tsb. Recurse untuk $bits yang muncul di sub-ekspresi
/// (mis. `$bits(A) + $bits(B)`). Ekspresi lain direkursi polos/utuh.
fn fold_param_bits(e: &Expr, module: &Module, vals: &HashMap<Symbol, i64>) -> Expr {
    match e {
        Expr::FuncCall {
            name,
            args,
            line,
            col,
        } if (name.as_str() == "$bits" || name.as_str() == "$size") && args.len() == 1 => {
            if let Expr::Ident { name: ident, .. } = &args[0] {
                // Hanya `$bits` yang menginferensi lebar dari literal default
                // (ELAB-13 (7)/(15)); `$size` mempertahankan perilaku lama.
                let infer_default = name.as_str() == "$bits";
                if let Some(w) = param_decl_width(module, ident, vals, infer_default) {
                    return Expr::Value(mivon_ast::expr::Value::Decimal(w.max(1) as i64));
                }
            }
            Expr::FuncCall {
                name: *name,
                args: args
                    .iter()
                    .map(|a| fold_param_bits(a, module, vals))
                    .collect(),
                line: *line,
                col: *col,
            }
        }
        Expr::BinaryOp { op, lhs, rhs } => Expr::BinaryOp {
            op: op.clone(),
            lhs: Box::new(fold_param_bits(lhs, module, vals)),
            rhs: Box::new(fold_param_bits(rhs, module, vals)),
        },
        Expr::UnaryOp { op, expr } => Expr::UnaryOp {
            op: op.clone(),
            expr: Box::new(fold_param_bits(expr, module, vals)),
        },
        Expr::Paren(inner) => Expr::Paren(Box::new(fold_param_bits(inner, module, vals))),
        Expr::Concat(parts) => Expr::Concat(
            parts
                .iter()
                .map(|p| fold_param_bits(p, module, vals))
                .collect(),
        ),
        _ => e.clone(),
    }
}

/// Nama yang dideklarasikan sebagai SINYAL di scope MODULE (port + deklarasi
/// body, bukan param, dan BUKAN deklarasi di dalam blok generate).
/// Dipakai `Elaborator::set_runtime_param_vals`: deklarasi sinyal module
/// menutupi nama compilation-unit/package (LRM §3.13), sedangkan `signal_map`
/// runtime juga memuat nama sintetis (loop var, net implisit) dan nama blok
/// prosedural yang TIDAK boleh menutupi param — karena itu filter memakai
/// deklarasi AST, bukan `signal_map` (ELAB-13 (19) review round 3).
///
/// PENTING — deklarasi generate TIDAK boleh ikut: `expand_generate_block`
/// menyalin `ModuleItem::Decl` hasil ekspansi ke `module.decls` (supaya
/// signal collection melihatnya) DAN menyisipkannya ke `module.items`.
/// Tanpa koreksi, `logic [7:0] L;` di dalam `generate if (1) begin : g` akan
/// dianggap sinyal scope module dan menghapus `$unit localparam L` dari
/// pandangan runtime — referensi `L` di level module jadi 42, bukan 7
/// (iverilog 7; ELAB-13 (19) review round 5). Deklarasi body asli HANYA ada
/// di `module.decls`, jadi selisih jumlah kemunculan (decls − items)
/// memisahkan keduanya, termasuk saat ada duplikat nama legal antar-blok.
pub(crate) fn module_scope_signal_names(module: &Module) -> std::collections::HashSet<Symbol> {
    use std::collections::{HashMap, HashSet};
    let mut counts: HashMap<Symbol, (usize, usize)> = HashMap::new();
    for d in &module.decls {
        for v in &d.names {
            counts.entry(v.name).or_insert((0, 0)).0 += 1;
        }
    }
    for item in &module.items {
        if let ModuleItem::Decl(d) = item {
            for v in &d.names {
                counts.entry(v.name).or_insert((0, 0)).1 += 1;
            }
        }
    }
    let mut out = HashSet::new();
    for p in &module.ports {
        out.insert(p.name);
    }
    for (name, (decls, from_items)) in counts {
        if decls > from_items {
            out.insert(name);
        }
    }
    out
}

/// Resolusi nilai parameter module dengan dukungan override instance.
/// positional_overrides: parameter posisional berdasarkan index.
/// named_overrides: parameter berdasarkan nama.
pub fn resolve_param_values_fn(
    module: &Module,
    instance_overrides: &HashMap<Symbol, i64>,
) -> Result<HashMap<Symbol, i64>, String> {
    resolve_param_values_with_ctx(module, instance_overrides, &HashMap::new(), None, &[])
}

/// Resolusi nilai parameter module dengan context awal (package params dari import).
/// `base_ctx` berisi nilai yang sudah diketahui sebelumnya (misal parameter package
/// yang di-import ke module) dan dijadikan fallback saat mengevaluasi default param.
///
/// `unit_params` = `design.unit_params` (param/localparam compilation-unit).
/// Nilainya disuntikkan ke `vals` SEBELUM param module di-resolve, sehingga
/// default body/header param yang merujuk nama `$unit`
/// (`localparam int W = L + 1`, `#(parameter W = L)`) melihat nilai aslinya —
/// bukan fallback 1/0 (ELAB-13 (19)). Nama yang sudah ada di `base_ctx` TIDAK
/// ditimpa, jadi param module tetap menang.
///
/// `pkg` (opsional) membuka evaluasi PENUH: `$bits(typedef)`, inlining fungsi
/// package, dan scoped ident `pkg::item` pada default param. Konteks gabungan
/// (konstanta package + module ctx) dibangun SEKALI per module — bukan per
/// param — agar clone konstanta package besar tidak menjadi bottleneck.
pub fn resolve_param_values_with_ctx(
    module: &Module,
    instance_overrides: &HashMap<Symbol, i64>,
    base_ctx: &HashMap<Symbol, i64>,
    pkg: Option<&PkgFullCtx>,
    unit_params: &[ParamDecl],
) -> Result<HashMap<Symbol, i64>, String> {
    let _t0 = std::time::Instant::now();
    let mut vals = base_ctx.clone();
    let _t_clone_vals = _t0.elapsed();
    // ELAB-13 (19): param/localparam compilation-unit ($unit) masuk SEBELUM
    // param module — `localparam int W = L + 1` (body) dan
    // `#(parameter W = L)` (header) harus melihat `L` yang asli. Guard
    // `contains_key` menjaga nilai dari `base_ctx` (param module/package)
    // tidak tertimpa. Urutan deklarasi dipertahankan agar `LD = L + 1` benar.
    //
    // `unit_names` mencatat nama yang berasal dari $unit: deklarasi di module
    // (body/generate) HARUS boleh menimpanya (LRM §3.13 — scope lokal menang),
    // sedangkan guard `contains_key` di loop body param akan melewatinya.
    // Nama dibuang dari himpunan setelah override pertama (lihat loop body
    // param) karena param body di-flatten ke satu map tanpa scope per blok.
    //
    // CATATAN: nilai $unit SELALU disuntik, termasuk nama yang juga dipakai
    // sinyal module. `vals` di sini adalah konteks CONST-FOLD (default param,
    // kondisi generate) — sinyal module bukan konstanta, jadi `#(parameter
    // W = L)` / `if (L == 99)` harus melihat nilai $unit (iverilog: W=7,
    // cabang else). Pemisahan jalur RUNTIME ada di
    // `Elaborator::set_runtime_param_vals` (ELAB-13 (19) review round 4).
    let mut unit_names: std::collections::HashSet<Symbol> = std::collections::HashSet::new();
    for p in unit_params {
        if vals.contains_key(&p.name) {
            continue;
        }
        if let Some(e) = &p.default {
            if let Ok(v) = const_eval_with_params(e, &vals) {
                let v = normalize_param_value(p, v, &vals);
                vals.insert(p.name, v);
                unit_names.insert(p.name);
            }
        }
    }
    // Konteks skalar penuh = `vals` itu sendiri: pemanggil menjamin
    // base_ctx ⊇ pkg_param_ctx ⊇ pkg_const_scalars (collect_package_param_ctx
    // meng-clone pkg_param_ctx yang meng-flatten konstanta package), jadi
    // tidak perlu map `merged` terpisah (dulu clone pkg.scalars + merge
    // base_ctx per module → bottleneck di OpenTitan).
    // Insert ke `vals`.
    macro_rules! insert_val {
        ($k:expr, $v:expr) => {{
            vals.insert($k, $v);
        }};
    }
    let mut positional_overrides: Vec<i64> = Vec::new();
    for (name, val) in instance_overrides {
        if name.starts_with("__param") {
            let idx: usize = name
                .as_str()
                .trim_start_matches("__param")
                .parse()
                .unwrap_or(0);
            if idx >= positional_overrides.len() {
                positional_overrides.resize(idx + 1, 0);
            }
            positional_overrides[idx] = *val;
        }
    }

    // Passthrough member keys struct dari override instance (`Info.integrity`
    // dst. — hasil flatten override struct di elaborate_module_with_params_and_type).
    // Loop param di bawah hanya menyalin override untuk nama param module;
    // key `param.field` (mengandung `.`) harus ikut agar generate if / width
    // member access di child bisa const-eval (pola `Info.integrity` di
    // otp_ctrl_part_buf — tanpa ini gagal "member access not allowed").
    //
    // PENTING — urutan: passthrough HARUS sebelum `register_struct_member_keys`.
    // Override instance (`CVA6Cfg(CVA6Cfg)` dgn config nyata) menang atas
    // default struct module (`cva6_cfg_empty`, semua 0). Bila registrasi
    // default jalan dulu, key `CVA6Cfg.field` = 0 terisi dan passthrough
    // (guard `!contains_key`) ter-skip → child menerima config kosong →
    // pembagian 0/0 (`ICACHE_LINE_WIDTH/AxiDataWidth`) saat elaborasi ulang
    // instance ber-override (mis. `wt_axi_adapter`).
    for (k, v) in instance_overrides {
        if k.as_str().contains('.') && !vals.contains_key(k) {
            insert_val!(*k, *v);
        }
    }

    for param in collect_body_params(module) {
        // Deklarasi body/generate MENIMPA nilai $unit dengan nama sama
        // (`generate if (1) begin : g localparam int L = 5; end` harus 5,
        // bukan nilai $unit). Nilai lama tetap dipakai untuk mengevaluasi
        // default yang merujuk nama lain.
        //
        // Override bersifat SEKALI (`remove`, bukan `contains`): param body
        // di-flatten ke satu map tanpa scope per blok generate, jadi deklarasi
        // kedua dengan nama sama tetap kalah dari yang pertama — perilaku yang
        // sudah ada sebelum dukungan $unit, bukan regresi baru (ELAB-13 (19)
        // review round 3, temuan sibling generate block).
        if !vals.contains_key(&param.name) || unit_names.remove(&param.name) {
            match &param.default {
                Some(e) => {
                    if let Some(v) = eval_param_default(e, &vals, pkg, module) {
                        let v = normalize_param_value(&param, v, &vals);
                        insert_val!(param.name, v);
                    } else if !param_default_is_collection(e) {
                        // Fallback global: param gagal dievaluasi (default
                        // `'x`, `$bits(member)` tak ter-resolve, referensi
                        // package DPI tak ada — mis. `VccPokStrNum` di dalam
                        // generate, `TRANSFER_BYTES_WIDTH = $bits(...)`) tetap
                        // didaftarkan dengan nilai 1 agar generate for-limit /
                        // width resolution tidak gagal "not found in parameter
                        // context". Array/struct literal dikecualikan (jalur
                        // array di bawah / evaluator struct yang menangani).
                        let v = normalize_param_value(&param, 1, &vals);
                        insert_val!(param.name, v);
                    }
                }
                None => {
                    let v = normalize_param_value(&param, 0, &vals);
                    insert_val!(param.name, v);
                }
            }
        }
    }

    // Pass 1: parameter (NON-localparam) — override / nilai default.
    for (i, param) in module.params.iter().enumerate() {
        if param.is_localparam {
            continue;
        }
        // Override membawa tipe sendiri untuk param tanpa range/tipe —
        // normalisasinya berbeda dari nilai default (ELAB-13 (11)/(13)).
        let ovr = if i < positional_overrides.len() {
            Some(positional_overrides[i])
        } else {
            instance_overrides.get(&param.name).copied()
        };
        let val = match ovr {
            Some(v) => v,
            None => match &param.default {
                Some(e) => eval_param_default(e, &vals, pkg, module).unwrap_or(0),
                None => 0,
            },
        };
        let val = match ovr {
            Some(_) => normalize_override_value(param, val, &vals),
            None => normalize_param_value(param, val, &vals),
        };
        insert_val!(param.name, val);
    }

    // ── Member keys struct SEBELUM localparam ──
    // `localparam int W = Cfg.Field;` (pola CVA6 `W = CVA6Cfg.VpnLen /
    // CVA6Cfg.PtLevels`) harus bisa const-eval. Key `Cfg.Field` didaftarkan
    // DI SINI — setelah param non-localparam (yang membawa struct) tapi
    // SEBELUM localparam — agar localparam & array melihatnya. Sebelumnya
    // registrasi terjadi setelah loop param, sehingga localparam yang
    // mereferensikan member struct selalu jatuh ke fallback 1.
    if let Some(p) = pkg {
        register_struct_member_keys(module, &mut vals, p);
    }

    // Pass 2: localparam — SETELAH member key struct terdaftar.
    for param in module.params.iter() {
        if !param.is_localparam {
            continue;
        }
        if let Some(e) = &param.default {
            if let Some(v) = eval_param_default(e, &vals, pkg, module) {
                let v = normalize_param_value(param, v, &vals);
                insert_val!(param.name, v);
            } else if !param_default_is_collection(e) {
                // Fallback global sama seperti body params di atas.
                let v = normalize_param_value(param, 1, &vals);
                insert_val!(param.name, v);
            }
        } else {
            let v = normalize_param_value(param, 0, &vals);
            insert_val!(param.name, v);
        }
    }

    // ── Localparam ARRAY: flatten keys `name[i]` / `name[r][c]` ──
    // Agar ekspresi konstanta `RhoOffset[x][y]`, `RC[rnd]`,
    // `SeedInfoPageSel[idx]` bisa di-fold selama generate expansion
    // (design-level pass), keys array di-flatten ke nilai skalar DI SINI —
    // sebelum expand_all_generates memakai param_vals ini. Dulu flatten
    // hanya dilakukan di elaborate_module (setelah generate expansion),
    // sehingga const_eval gagal dan generate-if/for meleset.
    let mut array_srcs: Vec<(Symbol, Vec<Expr>)> = Vec::new();
    for param in collect_body_params(module) {
        if let Some(Expr::Concat(elems)) = &param.default {
            if elems.len() > 1 {
                array_srcs.push((param.name, elems.clone()));
            }
        }
    }
    for param in &module.params {
        if let Some(Expr::Concat(elems)) = &param.default {
            if elems.len() > 1 && !array_srcs.iter().any(|(n, _)| *n == param.name) {
                array_srcs.push((param.name, elems.clone()));
            }
        }
    }
    for (name, elems) in array_srcs {
        let is_2d = elems.iter().all(|e| matches!(e, Expr::Concat(_)));
        let mut flat_elems: Vec<Expr> = Vec::new();
        if is_2d {
            for e in &elems {
                if let Expr::Concat(row) = e {
                    flat_elems.extend(row.iter().cloned());
                }
            }
        } else {
            flat_elems.extend(elems.iter().cloned());
        }
        for (fi, e) in flat_elems.iter().enumerate() {
            if let Ok(v) = const_eval_with_params(e, &vals) {
                let key = if is_2d {
                    let cols = flat_elems.len() / elems.len();
                    format!("{}[{}][{}]", name.as_str(), fi / cols, fi % cols)
                } else {
                    format!("{}[{}]", name.as_str(), fi)
                };
                insert_val!(Symbol::intern(&key), v);
            }
        }
    }

    // (Blok member key struct dipindah ke `register_struct_member_keys`,
    //  dipanggil SEBELUM evaluasi localparam — lihat di atas. Passthrough
    //  override `param.field` dipindah ke AWAL agar menang atas default.)

    if std::env::var("DBG_ELAB").is_ok() {
        eprintln!(
            "[DBG-RESOLVE] module '{}' total={}us clone_vals={}us",
            module.name.as_str(),
            _t0.elapsed().as_micros(),
            _t_clone_vals.as_micros()
        );
    }
    Ok(vals)
}

/// Daftarkan member key `param.field` untuk setiap param yang nilainya struct
/// ke `vals` — dipanggil `resolve_param_values_with_ctx` SEBELUM localparam
/// dievaluasi.
///
/// `eval_param_default` mengubah default struct menjadi skalar 0 sehingga
/// `Cfg.field` tidak pernah terdaftar; tanpa key itu, localparam/generate
/// yang memakai member struct gagal const-eval (pola CVA6
/// `W = CVA6Cfg.VpnLen / CVA6Cfg.PtLevels` → "dynamic range select not
/// supported", atau otp_ctrl_part_buf `Info.size`). Sumber fields:
/// - ident/scoped ident yang merujuk konstanta struct package,
/// - struct literal lokal module (`localparam user_cfg_t cfg = '{...}`),
/// - FUNCCALL yang mengembalikan struct (`build_config(...)`).
///
/// Konteks struct TUMBUH selama loop (struct lokal yang baru dievaluasi bisa
/// dipakai param berikutnya — `cva6_cfg` memakai `cva6_user_cfg`).
fn register_struct_member_keys(module: &Module, vals: &mut HashMap<Symbol, i64>, p: &PkgFullCtx) {
    let mut struct_defaults: Vec<(Symbol, Expr)> = Vec::new();
    for param in collect_body_params(module) {
        if let Some(e) = &param.default {
            struct_defaults.push((param.name, e.clone()));
        }
    }
    for param in &module.params {
        if let Some(e) = &param.default {
            struct_defaults.push((param.name, e.clone()));
        }
    }
    // `p.structs` kecil (puluhan entri) — clone per-module dapat diabaikan.
    let mut struct_ctx: HashMap<Symbol, Vec<SField>> = p.structs.clone();
    for (pname, e) in struct_defaults {
        let fields: Option<Vec<SField>> = match &e {
            Expr::Ident { name, .. } => struct_ctx.get(&Symbol::intern(name.as_str())).cloned(),
            Expr::ScopedIdent { item, .. } => {
                struct_ctx.get(&Symbol::intern(item.as_str())).cloned()
            }
            Expr::BitSelect { expr: inner, index } => {
                if let Expr::Ident { name, .. } = inner.as_ref() {
                    if let Ok(idx) = const_eval_with_params(index, vals) {
                        struct_ctx
                            .get(&Symbol::intern(&format!("{}[{}]", name.as_str(), idx)))
                            .cloned()
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            Expr::StructLit { .. } => {
                match eval_cval_full(
                    &e,
                    vals,
                    p.arrays,
                    p.package_symbols,
                    &struct_ctx,
                    p.array_structs,
                ) {
                    Some(CVal::Struct(fs)) => Some(fs),
                    _ => None,
                }
            }
            Expr::FuncCall { .. } => {
                match eval_cval_full(
                    &e,
                    vals,
                    p.arrays,
                    p.package_symbols,
                    &struct_ctx,
                    p.array_structs,
                ) {
                    Some(CVal::Struct(fs)) => Some(fs),
                    _ => None,
                }
            }
            _ => None,
        };
        let Some(fields) = fields else { continue };
        // Daftarkan struct hasil agar param berikutnya bisa me-resolve-nya.
        struct_ctx.insert(pname, fields.clone());
        let mut key_created = false;
        for f in &fields {
            if let Some(fname) = f.name {
                if let CVal::Scalar(v) = f.val {
                    let key = format!("{}.{}", pname.as_str(), fname.as_str());
                    let key_sym = Symbol::intern(&key);
                    if let std::collections::hash_map::Entry::Vacant(e) = vals.entry(key_sym) {
                        e.insert(v);
                        key_created = true;
                    }
                }
            }
        }
        if key_created && !vals.contains_key(&pname) {
            // Pastikan nama param terdaftar juga (skalar fallback 0) —
            // key `param.field` butuh `param` ada.
            vals.insert(pname, 0);
        }
    }
}
