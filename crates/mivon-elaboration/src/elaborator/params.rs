//! Package/param resolution — extract dari mod.rs untuk Arsitektur-06
//! (elaborator monolithic → submodule).
//!
//! Context parameter package global ($unit + qualified + plain) dan
//! resolve nilai param module. Seluruh method di sini adalah
//! `impl Elaborator`; call site tak berubah.

use std::collections::HashMap;

use mivon_ast::types::const_eval_with_params;
use mivon_ast::*;
use mivon_core::diagnostics::diagnostic::DiagCode;
use mivon_core::error::SimError;
use mivon_core::intern::Symbol;

use super::super::util::param_util::{resolve_param_values_with_ctx, PkgFullCtx};
use super::types::{eval_enum_member_value, package_overlay};
use super::Elaborator;

impl Elaborator {
    pub(crate) fn resolve_param_values(
        &self,
        module: &Module,
        instance_overrides: &HashMap<Symbol, i64>,
    ) -> Result<HashMap<Symbol, i64>, SimError> {
        let ctx = self.collect_package_param_ctx(module);
        // Evaluasi penuh: $bits(typedef) & inlining fungsi package untuk default
        // param — evaluator skalar sederhana tidak punya akses package_symbols.
        // `structs` = index struct package GLOBAL (lihat komentar di jalur
        // generate expansion) agar default `Info = PartInfoDefault` membawa
        // member keys `Info.size` dst. ke param_vals.
        let pkg_full = PkgFullCtx {
            scalars: &self.pkg_const_scalars,
            arrays: &self.pkg_const_arrays,
            package_symbols: &self.package_symbols,
            structs: &self.pkg_struct_ref_index,
            array_structs: &self.pkg_array_structs,
        };
        resolve_param_values_with_ctx(module, instance_overrides, &ctx, Some(&pkg_full)).map_err(
            |e| {
                self.elab_diag_at(
                    DiagCode::ParamMismatch,
                    e,
                    // Posisi deklarasi module (1-based).
                    module.line,
                    module.col,
                )
            },
        )
    }

    /// Bangun global package param context SEKALI: qualified `pkg::name` untuk semua
    /// parameter package + enum member (plain & qualified) dari semua package,
    /// plus konstanta package ter-evaluasi. Hasil disimpan di `self.pkg_param_ctx`
    /// dan di-clone oleh tiap module (lihat `collect_package_param_ctx`).
    pub(crate) fn build_pkg_param_ctx(&mut self) {
        let mut ctx: HashMap<Symbol, i64> = HashMap::new();
        // Enum member dari typedef $unit (top-level `typedef enum { A, B } E;`
        // di luar package/module) — GENARASI dari `mgen` menaruh typedef level
        // file di `.svh`, jadi pola `st = RUN` setelahnya HARUS resolve.
        // Sebelumnya hanya enum PACKAGE yang di-register → E2001 "signal
        // 'RUN' not found" untuk desain `.mv` hasil generate (dan SV murni).
        // Counter `last` di-reset per typedef (sama seperti enum package).
        // Nilai dihitung di dalam fixpoint di bawah (bersama package) agar
        // referensi-maju/mundur antar-typedef ikut resolve; member yang
        // ekspresinya tak ter-evaluasi dilewati (bukan ditebak) dan
        // dilaporkan pasca-fixpoint oleh verify_enum_members.
        let unit_enums: Vec<Vec<(Symbol, Option<Expr>)>> = self
            .design
            .unit_typedefs
            .iter()
            .filter_map(|td| {
                if let DataType::EnumType { members, .. } = &td.dtype {
                    Some(members.clone())
                } else {
                    None
                }
            })
            .collect();
        // Enum member constants dari package (plain + qualified, sequential).
        // Struktur per-typedef (Vec<Vec<…>>) — member enum tanpa nilai eksplisit
        // melanjutkan counter HANYA dalam typedef yang sama (standar SV).
        // Sebelumnya di-flatten jadi satu list per package sehingga counter
        // `last` bocor lintas enum → mis. `alu_op_base_e` di otbn_pkg mendapat
        // nilai 256+ (harusnya 0..8) karena menumpuk setelah enum lain.
        let pkg_enums: Vec<(Symbol, Vec<Vec<(Symbol, Option<Expr>)>>)> = self
            .package_symbols
            .iter()
            .filter_map(|(pkg_name, items)| {
                let enums: Vec<Vec<(Symbol, Option<Expr>)>> = items
                    .values()
                    .filter_map(|item| {
                        if let PackageItem::Typedef(td) = item {
                            if let DataType::EnumType { members, .. } = &td.dtype {
                                Some(members.clone())
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    })
                    .collect();
                if enums.is_empty() {
                    None
                } else {
                    Some((*pkg_name, enums))
                }
            })
            .collect();
        // Cache param package-sendiri per package (plain name → nilai) dari
        // skalar precompute — fallback forward-ref `enum {F = W}` ke param
        // yang dideklarasikan belakangan (valid SV, elaborasi order-free).
        // Dibangun sekali (nilai statis); dipakai hanya di jalur gagal.
        let mut pkg_param_cache: HashMap<Symbol, Vec<(Symbol, i64)>> = HashMap::new();
        for (k, v) in &self.pkg_const_scalars {
            if let Some((pkg, rest)) = k.as_str().split_once("::") {
                if !rest.contains([':', '.', '[']) {
                    pkg_param_cache
                        .entry(Symbol::intern(pkg))
                        .or_default()
                        .push((Symbol::intern(rest), *v));
                }
            }
        }
        for _ in 0..64 {
            let mut changed = false;
            // Enum $unit di dalam fixpoint (bukan sekali jalan) agar
            // referensi ke package-const maupun antar-typedef resolve.
            // GAGAL eval → lewati (jangan tebak `last`): percobaan ulang
            // iterasi berikut; yang tersisa dilaporkan pasca-fixpoint.
            for members in &unit_enums {
                let mut last: Option<i64> = Some(0);
                for (member_name, member_expr) in members {
                    let val = match member_expr {
                        Some(expr) => eval_enum_member_value(expr, &ctx, None),
                        None => last,
                    };
                    if let Some(v) = val {
                        if !ctx.contains_key(member_name) {
                            ctx.insert(*member_name, v);
                            changed = true;
                        }
                    }
                    last = val.map(|v| v.wrapping_add(1));
                }
            }
            // Qualified names untuk SEMUA package (agar scoped reference resolve)
            for (pkg_name, items) in &self.package_symbols {
                for (name, item) in items {
                    let PackageItem::Param(p) = item else {
                        continue;
                    };
                    let qualified = Symbol::intern(&format!("{}::{}", pkg_name, name));
                    if ctx.contains_key(&qualified) {
                        continue;
                    }
                    if let Some(expr) = &p.default {
                        // Koleksi (struct literal / concat multi-elemen) BUKAN
                        // skalar — const_eval_with_params mengembalikan 0
                        // untuk StructLit; mendaftarkannya menutupi struct
                        // asli di lookup yang mengutamakan scalars (mis. arg
                        // `user_pkg::UCFG` ke fungsi struct jadi Scalar(0)).
                        if matches!(expr, Expr::StructLit { .. }) {
                            continue;
                        }
                        if let Expr::Concat(parts) = expr {
                            if parts.len() > 1 {
                                continue;
                            }
                        }
                        if let Ok(val) = const_eval_with_params(expr, &ctx) {
                            ctx.insert(qualified, val);
                            changed = true;
                        }
                    }
                }
            }
            // Enum member constants di package (plain + qualified, sequential).
            // `last` di-reset per typedef enum (lihat komentar pkg_enums).
            // GAGAL eval → lewati (jangan tebak): retry iterasi berikut.
            // Overlay param package-sendiri (scope LRM menang atas global):
            // dibangun sekali per package per iterasi.
            for (pkg_name, enums) in &pkg_enums {
                let own: Option<&[(Symbol, i64)]> =
                    pkg_param_cache.get(pkg_name).map(|v| v.as_slice());
                let overlay = package_overlay(&ctx, own);
                for members in enums {
                    let mut last: Option<i64> = Some(0);
                    for (member_name, member_expr) in members {
                        let val = match member_expr {
                            Some(expr) => eval_enum_member_value(expr, &ctx, overlay.as_ref()),
                            None => last,
                        };
                        if let Some(v) = val {
                            if !ctx.contains_key(member_name) {
                                ctx.insert(*member_name, v);
                                changed = true;
                            }
                            let qualified =
                                Symbol::intern(&format!("{}::{}", pkg_name, member_name));
                            if let std::collections::hash_map::Entry::Vacant(e) =
                                ctx.entry(qualified)
                            {
                                e.insert(v);
                                changed = true;
                            }
                        }
                        last = val.map(|v| v.wrapping_add(1));
                    }
                }
            }
            if !changed {
                break;
            }
        }
        // Merge konstanta package yang sudah dievaluasi penuh (skalar + array).
        mivon_ast::const_eval_ext::flatten_consts_into_ctx(
            &self.pkg_const_scalars,
            &self.pkg_const_arrays,
            &mut ctx,
        );
        // Verifikasi: member enum eksplisit yang tetap tak ter-evaluasi
        // setelah fixpoint (typo/unknown, bukan forward-ref) dilaporkan —
        // sebelumnya ditebak diam-diam dari counter `last` (silent wrong).
        self.verify_enum_members(&unit_enums, "lingkup $unit", &ctx, None);
        for (pkg_name, enums) in &pkg_enums {
            let own: Option<&[(Symbol, i64)]> = pkg_param_cache.get(pkg_name).map(|v| v.as_slice());
            let overlay = package_overlay(&ctx, own);
            for members in enums {
                self.verify_enum_members(
                    std::slice::from_ref(members),
                    &format!("package '{pkg_name}'"),
                    &ctx,
                    overlay.as_ref(),
                );
            }
        }
        self.pkg_param_ctx = ctx;

        // ── Context $unit imports (unit_imports) — sekali, untuk semua module ──
        // Resolusi plain param names + flatten konstanta untuk import set global.
        let unit_imports: Vec<(Symbol, Symbol)> = self.design.unit_imports.clone();
        let mut unit_ctx: HashMap<Symbol, i64> = self.pkg_param_ctx.clone();
        for _ in 0..64 {
            let mut changed = false;
            for (package, import_item) in &unit_imports {
                let Some(pkg_items) = self.package_symbols.get(package) else {
                    continue;
                };
                let names: Vec<Symbol> = if import_item.as_str() == "*" {
                    pkg_items.keys().copied().collect()
                } else {
                    vec![*import_item]
                };
                for name in names {
                    if let Some(PackageItem::Param(p)) = pkg_items.get(&name) {
                        if unit_ctx.contains_key(&p.name) {
                            continue;
                        }
                        if let Some(expr) = &p.default {
                            // Guard koleksi sama seperti loop qualified di atas
                            // (StructLit → 0 menutupi struct asli).
                            if matches!(expr, Expr::StructLit { .. }) {
                                continue;
                            }
                            if let Expr::Concat(parts) = expr {
                                if parts.len() > 1 {
                                    continue;
                                }
                            }
                            if let Ok(val) = const_eval_with_params(expr, &unit_ctx) {
                                unit_ctx.insert(p.name, val);
                                changed = true;
                            }
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
        for (package, import_item) in &unit_imports {
            mivon_ast::const_eval_ext::flatten_imported_consts_into_ctx(
                package.as_str(),
                import_item.as_str(),
                &self.pkg_const_scalars,
                &self.pkg_const_arrays,
                &mut unit_ctx,
            );
        }
        // Hanya simpan DELTA vs pkg_param_ctx — sebagian besar entri sudah sama
        // (qualified names). Collect per-modul tinggal extend delta kecil,
        // menghindari ~33k insert duplikat untuk tiap module.
        let mut delta: HashMap<Symbol, i64> = HashMap::new();
        for (k, v) in &unit_ctx {
            if self.pkg_param_ctx.get(k) != Some(v) {
                delta.insert(*k, *v);
            }
        }
        // ── Bake unit-import delta ke pkg_param_ctx GLOBAL ──
        // Setiap module menerima extend(unit_import_ctx) yang SAMA di
        // collect_package_param_ctx — baking ke base sekali membuat module
        // TANPA import/enum milik sendiri bisa meminjam base (Cow::Borrowed)
        // tanpa clone 104k-entry per module (kerja berulang terbesar).
        for (k, v) in &delta {
            self.pkg_param_ctx.insert(*k, *v);
        }
        self.unit_import_ctx = delta;

        // ── Plain param per package — sekali, untuk import milik module ──
        // Base: context global yang sudah lengkap (qualified + unit imports).
        let base: HashMap<Symbol, i64> = self.unit_import_ctx.clone();
        let mut plain_map: HashMap<Symbol, HashMap<Symbol, i64>> = HashMap::new();
        for (pkg_name, items) in &self.package_symbols {
            let mut pctx = base.clone();
            for _ in 0..64 {
                let mut changed = false;
                for item in items.values() {
                    let PackageItem::Param(p) = item else {
                        continue;
                    };
                    if pctx.contains_key(&p.name) {
                        continue;
                    }
                    if let Some(expr) = &p.default {
                        // Guard koleksi (lihat loop qualified).
                        if matches!(expr, Expr::StructLit { .. }) {
                            continue;
                        }
                        if let Expr::Concat(parts) = expr {
                            if parts.len() > 1 {
                                continue;
                            }
                        }
                        if let Ok(val) = const_eval_with_params(expr, &pctx) {
                            pctx.insert(p.name, val);
                            changed = true;
                        }
                    }
                }
                if !changed {
                    break;
                }
            }
            let mut plain: HashMap<Symbol, i64> = HashMap::new();
            for (name, item) in items {
                let PackageItem::Param(p) = item else {
                    continue;
                };
                if let Some(&v) = pctx.get(&p.name) {
                    plain.insert(*name, v);
                }
            }
            // Member keys struct / array-of-struct package juga harus terlihat
            // sebagai PLAIN name (`PartInfo[0].offset`, `Info.size`) untuk
            // `import pkg::*` — bukan hanya qualified `pkg::PartInfo[0].offset`.
            // Tanpa ini, generate if `PartInfo[k].offset == 0` di module yang
            // meng-import package gagal const-eval (member access not allowed
            // in constant expression) → error E3001/E3003 palsu di OpenTitan
            // (otp_ctrl / otp_ctrl_part_buf / otp_ctrl_part_unbuf).
            let pkg_prefix = format!("{}::", pkg_name.as_str());
            for (qname, v) in &self.pkg_const_scalars {
                if let Some(rest) = qname.as_str().strip_prefix(&pkg_prefix) {
                    // Hanya member keys (mengandung `.`) — param skalar & enum
                    // member sudah masuk via loop di atas / pkg_param_ctx.
                    if rest.contains('.') {
                        plain.entry(Symbol::intern(rest)).or_insert(*v);
                    }
                }
            }
            plain_map.insert(*pkg_name, plain);
        }
        self.pkg_plain_params = plain_map;
        if std::env::var("DBG_ELAB").is_ok() {
            let gp = Symbol::intern("gpio_env_pkg");
            let gn = Symbol::intern("NUM_GPIOS");
            let plain_n = self.pkg_plain_params.get(&gp).map(|m| m.len()).unwrap_or(0);
            let has_ng = self
                .pkg_plain_params
                .get(&gp)
                .map(|m| m.contains_key(&gn))
                .unwrap_or(false);
            let pkg_has = self.package_symbols.contains_key(&gp);
            eprintln!("[DBG-ELAB]   pkg_plain_params[gpio_env_pkg] len={} has_NUM_GPIOS={} pkg_present={}", plain_n, has_ng, pkg_has);
        }
    }

    /// Kumpulkan nilai parameter package yang terlihat oleh module via
    /// `import pkg::*`/`import pkg::item` (baik di header module maupun $unit).
    /// Dipakai sebagai base context saat evaluasi konstanta (generate limit,
    /// parameter default, dll.) sehingga package parameter bisa di-resolve.
    /// Package param yang di-referensikan secara scoped (`pkg::name`) juga
    /// didaftarkan agar `const_eval_with_params` bisa me-resolve.
    pub(crate) fn collect_package_param_ctx(
        &self,
        module: &Module,
    ) -> std::borrow::Cow<'_, HashMap<Symbol, i64>> {
        // ── Fast path: module TANPA import pkg milik sendiri & tanpa enum
        // modul → base GLOBAL (pkg_param_ctx dengan unit-import delta sudah
        // di-bake saat build) dipinjam TANPA clone. Sebelumnya map 100k+
        // entry di-clone utk SETIAP module × setiap fase (expansion loop +
        // tiap resolve_param_values) = kerja berulang terbesar elaborate. ──
        let mut has_own_imports = false;
        let mut has_own_enums = false;
        for item in &module.items {
            match item {
                ModuleItem::Import { .. } => {
                    has_own_imports = true;
                    if has_own_enums {
                        break;
                    }
                }
                ModuleItem::Typedef(td) => {
                    if matches!(td.dtype, DataType::EnumType { .. }) {
                        has_own_enums = true;
                        if has_own_imports {
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
        if !has_own_imports && !has_own_enums {
            return std::borrow::Cow::Borrowed(&self.pkg_param_ctx);
        }
        let dbg = std::env::var("DBG_ELAB").is_ok();
        let t0 = std::time::Instant::now();
        let mut ctx: HashMap<Symbol, i64> = self.pkg_param_ctx.clone();
        let t1 = std::time::Instant::now();
        if dbg {
            eprintln!(
                "[DBG-ELAB]   collect clone pkg ctx: {}us (pkg_ctx={})",
                t1.duration_since(t0).as_micros(),
                self.pkg_param_ctx.len()
            );
        }
        // Context $unit imports sudah di-precompute (konstan antar-module).
        ctx.extend(self.unit_import_ctx.iter().map(|(k, v)| (*k, *v)));
        let t2 = std::time::Instant::now();
        if dbg {
            eprintln!(
                "[DBG-ELAB]   collect extend unit ctx: {}us (unit_ctx={})",
                t2.duration_since(t1).as_micros(),
                self.unit_import_ctx.len()
            );
        }
        // Import set milik module itu sendiri (di luar $unit).
        let module_imports: Vec<(Symbol, Symbol)> = module
            .items
            .iter()
            .filter_map(|item| {
                if let ModuleItem::Import { package, item } = item {
                    Some((*package, *item))
                } else {
                    None
                }
            })
            .collect();
        // Enum member constants dari typedef di body module (e.g. LastEdnEntry)
        let module_enums: Vec<Vec<(Symbol, Option<Expr>)>> = module
            .items
            .iter()
            .filter_map(|item| {
                if let ModuleItem::Typedef(td) = item {
                    if let DataType::EnumType { members, .. } = &td.dtype {
                        Some(members.clone())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();
        // Fixed-point: modul-local enum member & plain import names bisa
        // mereferensikan package param (dan satu sama lain). Plain param
        // package sudah di-precompute (pkg_plain_params); hanya enum modul yang
        // perlu fixed-point.
        let tf = std::time::Instant::now();
        for _ in 0..64 {
            let mut changed = false;
            // Enum member constants dari body module (sequential values).
            // GAGAL eval → lewati (jangan tebak): retry iterasi berikut.
            for members in &module_enums {
                let mut last: Option<i64> = Some(0);
                for (member_name, member_expr) in members {
                    let val = match member_expr {
                        Some(expr) => const_eval_with_params(expr, &ctx).ok(),
                        None => last,
                    };
                    if let Some(v) = val {
                        if !ctx.contains_key(member_name) {
                            ctx.insert(*member_name, v);
                            changed = true;
                        }
                    }
                    last = val.map(|v| v.wrapping_add(1));
                }
            }
            // Plain names untuk package yang di-import module
            for (package, import_item) in &module_imports {
                let Some(plain) = self.pkg_plain_params.get(package) else {
                    continue;
                };
                if import_item.as_str() == "*" {
                    for (&name, &val) in plain {
                        ctx.entry(name).or_insert(val);
                    }
                } else if let Some(&val) = plain.get(import_item) {
                    ctx.entry(*import_item).or_insert(val);
                }
            }
            if !changed {
                break;
            }
        }
        if dbg && tf.elapsed().as_micros() > 20_000 {
            eprintln!(
                "[DBG-ELAB]   collect fixed-point: {}us (module_imports={}, enums={})",
                tf.elapsed().as_micros(),
                module_imports.len(),
                module_enums.len()
            );
        }
        // Verifikasi di sini SENGAJA dilewati: enum lokal module diverifikasi
        // sekali di elaborate_module_with_params (ctx effective_params lebih
        // lengkap: parameter module + generate). Verifikasi ganda = warning
        // duplikat untuk typedef yang sama.
        // Merge konstanta package yang sudah dievaluasi penuh — hanya untuk
        // import set milik module ($unit sudah ada di unit_import_ctx).
        let tl = std::time::Instant::now();
        for (package, import_item) in &module_imports {
            mivon_ast::const_eval_ext::flatten_imported_consts_into_ctx(
                package.as_str(),
                import_item.as_str(),
                &self.pkg_const_scalars,
                &self.pkg_const_arrays,
                &mut ctx,
            );
        }
        if dbg && module.name.as_str() == "rv_core_ibex_peri" {
            let gn = Symbol::intern("NumRegions");
            let ga = Symbol::intern("NumAlerts");
            eprintln!(
                "[DBG-ELAB]   peri imports={:?}",
                module_imports
                    .iter()
                    .map(|(p, i)| format!("{}::{}", p.as_str(), i.as_str()))
                    .collect::<Vec<_>>()
            );
            eprintln!("[DBG-ELAB]   peri has_pkg_reg={} NumRegions_in_ctx={} NumAlerts_in_ctx={} ctx_len={}", self.package_symbols.contains_key(&Symbol::intern("rv_core_ibex_reg_pkg")), ctx.contains_key(&gn), ctx.contains_key(&ga), ctx.len());
            eprintln!(
                "[DBG-ELAB]   pkg_plain_params has reg_pkg={}",
                self.pkg_plain_params
                    .contains_key(&Symbol::intern("rv_core_ibex_reg_pkg"))
            );
        }
        if dbg && module.name.as_str() == "tb" {
            let gn = Symbol::intern("NUM_GPIOS");
            eprintln!(
                "[DBG-ELAB]   tb imports={:?} NUM_GPIOS_in_ctx={}",
                module_imports
                    .iter()
                    .map(|(p, i)| format!("{}::{}", p.as_str(), i.as_str()))
                    .collect::<Vec<_>>(),
                ctx.contains_key(&gn)
            );
        }
        if dbg && tl.elapsed().as_micros() > 20_000 {
            eprintln!(
                "[DBG-ELAB]   collect flatten-module: {}us",
                tl.elapsed().as_micros()
            );
        }
        if dbg && t0.elapsed().as_micros() > 50_000 {
            eprintln!("[DBG-ELAB]   collect total: {}us", t0.elapsed().as_micros());
        }
        std::borrow::Cow::Owned(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mivon_ast::types::{Design, PackageItem, ParamDecl};

    fn pkg_param(name: &str, val: i64) -> (Symbol, PackageItem) {
        let p = ParamDecl {
            name: Symbol::intern(name),
            dtype: None,
            range: None,
            default: Some(Expr::Value(mivon_ast::expr::Value::Decimal(val))),
            is_localparam: false,
            is_type_param: false,
            type_default: None,
        };
        (Symbol::intern(name), PackageItem::Param(p))
    }

    #[test]
    fn test_build_pkg_param_ctx_qualified() {
        // Satu package `p { param P = 8 }` → ctx memuat `p::P` = 8.
        let mut e = Elaborator::with_source(Design::default(), vec![], String::new());
        let (n, item) = pkg_param("P", 8);
        let mut items = std::collections::HashMap::new();
        items.insert(n, item);
        e.package_symbols.insert(Symbol::intern("p"), items);
        e.build_pkg_param_ctx();
        assert_eq!(
            e.pkg_param_ctx.get(&Symbol::intern("p::P")).copied(),
            Some(8)
        );
    }
}
