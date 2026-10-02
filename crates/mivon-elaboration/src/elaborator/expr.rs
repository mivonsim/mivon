use super::super::util::*;
use super::stmt::{expr_is_real_ty, is_real_type_name, strip_parens};
use super::Elaborator;
use super::BUILTIN_UVM_CLASSES;
use mivon_ast::types::{const_eval_simple, const_eval_with_params};
use mivon_ast::*;
use mivon_core::diagnostics::diagnostic::DiagCode;
use mivon_core::diagnostics::suggest::suggest_name;
use mivon_core::error::SimError;
use std::collections::HashMap;

use mivon_core::intern::Symbol;
use mivon_ir::*;

impl Elaborator {
    /// F36: apakah `name` adalah nama instance (module atau interface) di dalam
    /// module/interface yang sedang di-elaborasi. Dipakai untuk method call /
    /// referensi receiver yang tidak terdaftar sebagai signal (interface
    /// instance seperti `clk_if.set_period_ps(...)` atau `sck_clk.set_active()`)
    /// agar tidak menjadi E2001 "signal not found".
    /// Coba fold konstanta + catat statistik optimasi (db.md "6. optimize/",
    /// "10. expression/"). Wrapper `try_fold_const` yang menambah sampel
    /// (teks ekspresi, nilai) ke `opt_stats` bila berhasil.
    fn try_fold_const(&self, expr: &Expr) -> Result<Option<IrExpr>, String> {
        let folded = super::super::util::try_fold_const(expr, &self.param_vals)?;
        if let Some(IrExpr::Const(lv)) = &folded {
            self.opt_stats
                .record_const_fold(format!("{:?}", expr), lv.to_u64() as i64);
        }
        Ok(folded)
    }

    pub(crate) fn is_current_module_instance(&self, name: &Symbol) -> bool {
        let Some(cur) = self.current_module else {
            return false;
        };
        let items = self
            .design
            .modules
            .iter()
            .find(|m| m.name == cur)
            .map(|m| m.items.as_slice())
            .or_else(|| {
                self.design
                    .interfaces
                    .iter()
                    .find(|i| i.name == cur)
                    .map(|i| i.items.as_slice())
            });
        let Some(items) = items else {
            return false;
        };
        items
            .iter()
            .any(|item| matches!(item, ModuleItem::Instance(inst) if inst.instance_name == *name))
    }

    pub(crate) fn build_hier_name(obj: &Expr, field: &str) -> String {
        match obj {
            Expr::Ident { name: prefix, .. } => format!("{}.{}", prefix, field),
            Expr::MemberAccess {
                obj: inner,
                field: inner_field,
            } => {
                format!(
                    "{}.{}",
                    Self::build_hier_name(inner, inner_field.as_str()),
                    field
                )
            }
            _ => String::new(),
        }
    }

    pub(crate) fn elaborate_expr(
        &self,
        expr: &Expr,
        signal_map: &HashMap<Symbol, SignalId>,
        signals: &[SignalInfo],
    ) -> Result<IrExpr, SimError> {
        // Statistik cache pipeline (db.md "10. expression/"): jumlah evaluasi
        // ekspresi. Dipanggil untuk setiap sub-ekspresi — di-akumulasi di
        // `opt_stats` (Cell) dan di-snapshot ke cache setelah elaborasi.
        self.opt_stats.record_expr_eval();
        match expr {
            Expr::Ident { name, .. } if name == "this" => Ok(IrExpr::This),
            Expr::Value(v) => {
                let lv = value_to_logicvec(v);
                // ROUND 36 (LRM §6.8.1 + §11.8.2): literal desimal UNSIZED
                // (`5`, `2`) bersifat SIGNED di SV. Tanpa ini `a < 0` dan
                // `a / 2` (a int) bergantung pada aturan `||` yang salah utk
                // operand unsigned (`a < 8'hFF` jadi signed padahal LRM
                // bilang unsigned). Sized literal tetap unsigned kecuali
                // suffix `s` (is_signed: true).
                let is_signed = matches!(v, Value::Decimal(_))
                    || matches!(
                        v,
                        Value::Binary {
                            is_signed: true,
                            ..
                        } | Value::Hex {
                            is_signed: true,
                            ..
                        } | Value::Octal {
                            is_signed: true,
                            ..
                        }
                    );
                if matches!(v, Value::Real(_)) {
                    // Literal real → `RealConst` (penanda tipe real; nilai
                    // bit identik Const 64-bit). Tanpa penanda, `%f` dari
                    // `1.5` membaca bit-pattern sebagai integer (4.6e18).
                    return Ok(IrExpr::RealConst(lv));
                }
                if is_signed {
                    Ok(IrExpr::Signed(Box::new(IrExpr::Const(lv))))
                } else {
                    Ok(IrExpr::Const(lv))
                }
            }
            Expr::FillLit(val) => Ok(IrExpr::FillLit(*val)),
            Expr::Ident { name, line, col } => {
                if name.starts_with("$") {
                    return Ok(IrExpr::SysFunc {
                        name: *name,
                        args: vec![],
                        line: *line,
                        col: *col,
                    });
                }
                // F18: `uvm_test_top` — handle global ke root test UVM (dibuat
                // oleh run_test/execute_phases). Di-resolve di evaluator ke
                // root_test_obj_id; 0 (null) bila tidak ada test.
                if name.as_str() == "uvm_test_top" {
                    return Ok(IrExpr::SysFunc {
                        name: *name,
                        args: vec![],
                        line: *line,
                        col: *col,
                    });
                }
                // LANG-40: ident yang merupakan `let` tanpa parameter —
                // elaborate body let (alias ekspresi, IEEE 1800-2017 §11.12.2).
                if let Some(ld) = self.let_decls.get(name) {
                    if ld.params.is_empty() {
                        return self.elaborate_expr(&ld.expr, signal_map, signals);
                    }
                }
                // Check if this ident is a parameter (from param_vals or effective_params)
                if let Some(&val) = self.param_vals.get(name) {
                    return Ok(super::const_or_signed(
                        LogicVec::from_u64(val as u64, 64),
                        self.param_is_signed.contains(name),
                    ));
                }
                // Enum member plain dari package (di-build oleh build_pkg_param_ctx
                // dengan nilai sequential). Case label seperti `DmaXfer1BperTxn:`
                // atau `MuBi4True:` di-resolve ke konstanta, bukan E2001.
                if let Some(&val) = self.pkg_param_ctx.get(name) {
                    return Ok(IrExpr::Const(LogicVec::from_u64(val as u64, 64)));
                }
                let sig_id = signal_map.get(name).ok_or_else(|| {
                    let mut candidates: Vec<&str> = signal_map.keys().map(|s| s.as_str()).collect();
                    for k in self.param_vals.keys() {
                        candidates.push(k.as_str());
                    }
                    for k in self.pkg_param_ctx.keys() {
                        candidates.push(k.as_str());
                    }
                    let hint = suggest_name(name.as_str(), candidates.into_iter())
                        .map(|(s, _)| format!(" — did you mean '{}'?", s))
                        .unwrap_or_default();
                    self.elab_diag_at(
                        DiagCode::UndefinedSignal,
                        format!("signal '{}' not found{}", name, hint),
                        *line,
                        *col,
                    )
                })?;
                Ok(IrExpr::Signal(*sig_id, 0))
            }
            Expr::ScopedIdent {
                package,
                item,
                line,
                col,
            } => {
                // Enum member atau konstanta lain yang sudah di-flatten ke
                // param_vals sebagai qualified name `pkg::member` oleh
                // build_pkg_param_ctx (enum member TIDAK terdaftar sebagai
                // PackageItem, jadi tidak ada di package_symbols).
                let qualified = Symbol::intern(&format!("{}::{}", package.as_str(), item.as_str()));
                if let Some(&val) = self.param_vals.get(&qualified) {
                    return Ok(IrExpr::Const(LogicVec::from_u64(val as u64, 64)));
                }
                // Context package global (build_pkg_param_ctx) — qualified
                // `pkg::member` untuk enum member & konstanta package. Contoh
                // nyata OpenTitan DV: `dv_utils_pkg::Device` (member enum
                // `if_mode_e`) dipakai di `push_pull_if`.
                if let Some(&val) = self.pkg_param_ctx.get(&qualified) {
                    return Ok(IrExpr::Const(LogicVec::from_u64(val as u64, 64)));
                }
                if let Some(pkg_items) = self.package_symbols.get(package) {
                    if let Some(pkg_item) = pkg_items.get(item) {
                        match pkg_item {
                            PackageItem::Param(p) => {
                                if let Some(expr) = &p.default {
                                    if let Ok(val) = const_eval_with_params(expr, &self.param_vals)
                                    {
                                        // Signed bila tipe param signed (sama
                                        // dgn jalur plain param di atas).
                                        let psign = super::param_decl_is_signed(p);
                                        return Ok(super::const_or_signed(
                                            LogicVec::from_u64(val as u64, 64),
                                            psign,
                                        ));
                                    }
                                }
                                return Err(self.elab_diag_at(
                                    DiagCode::ParamMismatch,
                                    format!("package param '{}.{}' has no default", package, item),
                                    *line,
                                    *col,
                                ));
                            }
                            PackageItem::Typedef(td) => {
                                // `pkg::TypeName` dipakai sebagai ekspresi (mis. cast atau
                                // type-param). Kembalikan lebar typedef sebagai konstanta
                                // sehingga ekspresi yang memakai hasil ini masih valid.
                                let w = self.resolve_typedef_width_dims(
                                    &td.dtype,
                                    td.range.as_ref(),
                                    &td.extra_packed_dims,
                                    &self.param_vals,
                                );
                                return Ok(IrExpr::Const(LogicVec::from_u64(w as u64, 32)));
                            }
                            // Package variable (mis. `sec_cm_pkg::sec_cm_if_proxy_q`
                            // — queue of class handles di DV). Dipakai sebagai
                            // receiver method (`pkg::var.push_back(...)`) atau
                            // referensi objek. Bukan konstanta — kembalikan
                            // handle 0 (null) + warning agar elaborasi interface
                            // DV tidak gagal total; runtime method-call pada
                            // objek 0 menjadi no-op.
                            PackageItem::Decl(_) => {
                                self.elab_warn_at(
                                    DiagCode::ModuleNotFound,
                                    format!(
                                        "package variable '{}.{}' treated as null handle (0) in expression context",
                                        package, item
                                    ),
                                    *line,
                                    *col,
                                );
                                return Ok(IrExpr::Const(LogicVec::from_u64(0, 64)));
                            }
                            _ => {
                                return Err(self.elab_diag_at(
                                    DiagCode::ModuleNotFound,
                                    format!(
                                        "'{}' is not a constant in package '{}'",
                                        item, package
                                    ),
                                    *line,
                                    *col,
                                ))
                            }
                        }
                    }
                }
                Err(self.elab_diag_at(
                    DiagCode::ModuleNotFound,
                    format!("'{}' not found in package '{}'", item, package),
                    *line,
                    *col,
                ))
            }
            Expr::RangeSelect {
                expr: inner,
                msb,
                lsb,
            } => {
                // Const-fold array/param range select (e.g. `pkg::ARR[3:0]`)
                if let Some(folded) = self.try_fold_const(expr)? {
                    return Ok(folded);
                }
                let inner_expr = self.elaborate_expr(inner, signal_map, signals)?;
                if let (Ok(msb_c), Ok(lsb_c)) = (
                    const_eval_params(msb, &self.param_vals),
                    const_eval_params(lsb, &self.param_vals),
                ) {
                    let msb_c = msb_c.max(0) as usize;
                    let lsb_c = lsb_c.max(0) as usize;
                    if let IrExpr::Signal(sid, _) = &inner_expr {
                        Ok(IrExpr::RangeSelect(*sid, msb_c, lsb_c))
                    } else {
                        Ok(IrExpr::ExprRangeSelect(Box::new(inner_expr), msb_c, lsb_c))
                    }
                } else {
                    // Fallback width-aware: bound memakai `$bits(typedef)` dsb yang
                    // tidak bisa const-eval skalar tapi lebar typenya diketahui.
                    let msb_v = width::eval_width_aware_param(
                        msb,
                        signal_map,
                        signals,
                        &self.param_vals,
                        &self.package_symbols,
                    );
                    let lsb_v = width::eval_width_aware_param(
                        lsb,
                        signal_map,
                        signals,
                        &self.param_vals,
                        &self.package_symbols,
                    );
                    match (msb_v, lsb_v) {
                        (Some(m), Some(l)) => {
                            let (msb_c, lsb_c) = (m.max(0) as usize, l.max(0) as usize);
                            if let IrExpr::Signal(sid, _) = &inner_expr {
                                Ok(IrExpr::RangeSelect(*sid, msb_c, lsb_c))
                            } else {
                                Ok(IrExpr::ExprRangeSelect(Box::new(inner_expr), msb_c, lsb_c))
                            }
                        }
                        _ => Err(self.elab_diag_at(
                            DiagCode::ModuleNotFound,
                            "dynamic range select not supported",
                            expr_location(expr).0,
                            expr_location(expr).1,
                        )),
                    }
                }
            }
            Expr::BitSelect { expr: inner, index } => {
                // Const-fold array/param element select (e.g. `pkg::ARR[2]` via
                // flattened param_vals key `ARR[2]`). Falls through if not constant.
                if let Some(folded) = self.try_fold_const(expr)? {
                    return Ok(folded);
                }
                let inner_expr = self.elaborate_expr(inner, signal_map, signals)?;
                // Array param package dengan index dinamis/konstan yang tidak
                // ter-fold: `pkg::ARR[idx]` harus memilih ELEMEN (lebar elem_w),
                // bukan bit tunggal. Tanpa ini `PRESENT_SBOX4[x[k*4 +: 4]]`
                // menjadi part-select lebar 1 → nilai sbox salah + WR0102 palsu.
                if let Some((arr_const, elem_w)) = self.pkg_array_param_element(inner, &inner_expr)
                {
                    let index_expr = self.elaborate_expr(index, signal_map, signals)?;
                    let base_expr = IrExpr::BinaryOp(
                        BinaryIrOp::Mul,
                        Box::new(index_expr),
                        Box::new(IrExpr::Const(LogicVec::from_u64(elem_w as u64, 32))),
                    );
                    return Ok(IrExpr::ExprPartSelect(
                        Box::new(arr_const),
                        Box::new(base_expr),
                        Box::new(IrExpr::Const(LogicVec::from_u64(elem_w as u64, 32))),
                    ));
                }
                if let IrExpr::Signal(sid, _) = &inner_expr {
                    let sig = &signals[*sid];
                    // Multi-dim packed array: `mem[i]` memilih ELEMEN (sub-array),
                    // bukan bit tunggal — `logic [3:0][7:0] mem; mem[2]` = 8 bit
                    // (bit 23:16), bukan bit 2. Jalur tulis (elaborate_lvalue)
                    // sudah benar; jalur baca dulu BitSelect 1-bit → `mem[k]`
                    // jadi 0/1 (probe t05b: flat benar tapi mem[2]=0).
                    // Lebar elemen = width / packed_dims[0] (dim terluar).
                    if sig.packed_dims.len() > 1 {
                        let elem_w =
                            sub_elem_width_from_packed(signals, *sid, sig.width).unwrap_or(1);
                        if elem_w > 1 {
                            if let Ok(idx) = const_eval_params(index, &self.param_vals) {
                                let lsb = (idx as usize).saturating_mul(elem_w);
                                let msb = lsb + elem_w - 1;
                                Ok(IrExpr::RangeSelect(*sid, msb, lsb))
                            } else {
                                let index_expr = self.elaborate_expr(index, signal_map, signals)?;
                                let base_expr = IrExpr::BinaryOp(
                                    BinaryIrOp::Mul,
                                    Box::new(index_expr),
                                    Box::new(IrExpr::Const(LogicVec::from_u64(elem_w as u64, 32))),
                                );
                                Ok(IrExpr::ExprPartSelect(
                                    Box::new(IrExpr::Signal(*sid, sig.width)),
                                    Box::new(base_expr),
                                    Box::new(IrExpr::Const(LogicVec::from_u64(elem_w as u64, 32))),
                                ))
                            }
                        } else if let Ok(idx) = const_eval_params(index, &self.param_vals) {
                            Ok(IrExpr::BitSelect(*sid, idx as usize))
                        } else {
                            let index_expr = self.elaborate_expr(index, signal_map, signals)?;
                            Ok(IrExpr::ExprPartSelect(
                                Box::new(IrExpr::Signal(*sid, sig.width)),
                                Box::new(index_expr),
                                Box::new(IrExpr::Const(LogicVec::from_u64(1, 32))),
                            ))
                        }
                    } else if sig.array_depth > 1
                        || sig.is_dynamic
                        || sig.is_queue
                        || sig.is_associative
                    {
                        let index_expr = self.elaborate_expr(index, signal_map, signals)?;
                        // F39: multi-dim unpacked — index pertama memilih ROW
                        // (sub-array): lebar = elem_width × Π dims[1..].
                        // ArrayIndex engine: start = idx × elem_width → row
                        // terbaca utuh (bit offset i × row_w).
                        let ew = if sig.array_dims.len() > 1 {
                            sig.array_dims[1..]
                                .iter()
                                .product::<usize>()
                                .saturating_mul(sig.elem_width)
                        } else {
                            sig.elem_width
                        };
                        Ok(IrExpr::ArrayIndex {
                            sig_id: *sid,
                            index: Box::new(index_expr),
                            elem_width: ew,
                        })
                    } else if let Ok(idx) = const_eval_params(index, &self.param_vals) {
                        Ok(IrExpr::BitSelect(*sid, idx as usize))
                    } else {
                        // Index DINAMIS pada sinyal flat → bit-select runtime,
                        // BUKAN ArrayIndex (elem_width sinyal flat = lebar
                        // penuh → ArrayIndex membaca selebar sinyal per index).
                        let index_expr = self.elaborate_expr(index, signal_map, signals)?;
                        Ok(IrExpr::ExprPartSelect(
                            Box::new(IrExpr::Signal(*sid, sig.width)),
                            Box::new(index_expr),
                            Box::new(IrExpr::Const(LogicVec::from_u64(1, 32))),
                        ))
                    }
                } else if let IrExpr::RangeSelect(sid, outer_msb, outer_lsb) = &inner_expr {
                    // Bit-select bertingkat pada chunk packed multi-dimensi:
                    // `state[0]` → RangeSelect(319:0) (320-bit), lalu `[0]`
                    // memilih ELEMEN (64-bit), bukan bit tunggal. Hitung lebar
                    // sub-elemen dari packed_dims.
                    let chunk_w = outer_msb.abs_diff(*outer_lsb) + 1;
                    let elem_w = sub_elem_width_from_packed(signals, *sid, chunk_w);
                    if let Some(elem_w) = elem_w {
                        if let Ok(idx) = const_eval_params(index, &self.param_vals) {
                            let base = (*outer_msb).min(*outer_lsb);
                            let lsb = base + idx as usize * elem_w;
                            let msb = lsb + elem_w - 1;
                            Ok(IrExpr::RangeSelect(*sid, msb, lsb))
                        } else {
                            let index_expr = self.elaborate_expr(index, signal_map, signals)?;
                            let base_expr = IrExpr::BinaryOp(
                                BinaryIrOp::Mul,
                                Box::new(index_expr),
                                Box::new(IrExpr::Const(LogicVec::from_u64(elem_w as u64, 32))),
                            );
                            Ok(IrExpr::ExprPartSelect(
                                Box::new(IrExpr::RangeSelect(*sid, *outer_msb, *outer_lsb)),
                                Box::new(base_expr),
                                Box::new(IrExpr::Const(LogicVec::from_u64(elem_w as u64, 32))),
                            ))
                        }
                    } else if let Ok(idx) = const_eval_params(index, &self.param_vals) {
                        Ok(IrExpr::ExprBitSelect(Box::new(inner_expr), idx as usize))
                    } else {
                        let index_expr = self.elaborate_expr(index, signal_map, signals)?;
                        Ok(IrExpr::ExprPartSelect(
                            Box::new(inner_expr),
                            Box::new(index_expr),
                            Box::new(IrExpr::Const(LogicVec::from_u64(1, 32))),
                        ))
                    }
                } else if let IrExpr::ArrayIndex {
                    sig_id,
                    index: inner_idx,
                    elem_width: inner_ew,
                } = &inner_expr
                {
                    // F39: index berantai pada array unpacked multi-dimensi
                    // `mat[i][j]` — fold ke SATU ArrayIndex dengan index
                    // gabungan (unit elemen relatif thd elem_width baru).
                    // inner membaca row selebar inner_ew = ew × Π dims[c..];
                    // index saat ini mengkonsumsi dims[c]:
                    //   combined_index = inner_idx × dims[c] + current
                    //   new_ew         = inner_ew / dims[c]
                    // Engine ArrayIndex: start = idx × ew → bit offset
                    // (i×row_w + j×elem_w) benar.
                    let mut folded: Option<(SignalId, IrExpr, usize)> = None;
                    if let Some(sig) = signals.get(*sig_id) {
                        let dims = &sig.array_dims;
                        let ew = sig.elem_width;
                        if dims.len() > 1 && *inner_ew > ew && ew > 0 {
                            let remaining = *inner_ew / ew;
                            let mut c = dims.len();
                            let mut prod = 1usize;
                            while c > 0 {
                                c -= 1;
                                prod *= dims[c];
                                if prod == remaining {
                                    break;
                                }
                            }
                            if prod == remaining && c < dims.len() {
                                let new_ew = *inner_ew / dims[c];
                                let idx_expr = self.elaborate_expr(index, signal_map, signals)?;
                                let combined = IrExpr::BinaryOp(
                                    BinaryIrOp::Add,
                                    Box::new(IrExpr::BinaryOp(
                                        BinaryIrOp::Mul,
                                        Box::new((**inner_idx).clone()),
                                        Box::new(IrExpr::Const(LogicVec::from_u64(
                                            dims[c] as u64,
                                            32,
                                        ))),
                                    )),
                                    Box::new(idx_expr),
                                );
                                folded = Some((*sig_id, combined, new_ew));
                            }
                        }
                    }
                    match folded {
                        Some((sid2, combined, new_ew)) => Ok(IrExpr::ArrayIndex {
                            sig_id: sid2,
                            index: Box::new(combined),
                            elem_width: new_ew,
                        }),
                        None => {
                            // Bukan fold valid (`mat[i][j]` dgn semua dim habis
                            // = bit-select; bukan multi-dim) — perilaku lama.
                            if let Ok(idx) = const_eval_params(index, &self.param_vals) {
                                Ok(IrExpr::ExprBitSelect(
                                    Box::new(inner_expr.clone()),
                                    idx as usize,
                                ))
                            } else {
                                let index_expr = self.elaborate_expr(index, signal_map, signals)?;
                                Ok(IrExpr::ExprPartSelect(
                                    Box::new(inner_expr.clone()),
                                    Box::new(index_expr),
                                    Box::new(IrExpr::Const(LogicVec::from_u64(1, 32))),
                                ))
                            }
                        }
                    }
                } else if let Ok(idx) = const_eval_params(index, &self.param_vals) {
                    Ok(IrExpr::ExprBitSelect(Box::new(inner_expr), idx as usize))
                } else {
                    // Index dinamis pada ekspresi non-signal (mis. hasil
                    // `c[idx_x][idx_z]` dengan `idx_z` variabel lokal). Simulator
                    // mengevaluasi `ExprPartSelect` dengan base dinamis, jadi
                    // bit-select dinamis cukup diterjemahkan ke part-select
                    // lebar 1 dengan base = index.
                    let index_expr = self.elaborate_expr(index, signal_map, signals)?;
                    Ok(IrExpr::ExprPartSelect(
                        Box::new(inner_expr),
                        Box::new(index_expr),
                        Box::new(IrExpr::Const(LogicVec::from_u64(1, 32))),
                    ))
                }
            }
            Expr::Concat(exprs) => {
                if let Some(folded) = self.try_fold_const(expr)? {
                    return Ok(folded);
                }
                let parts: Result<Vec<IrExpr>, SimError> = exprs
                    .iter()
                    .map(|e| self.elaborate_expr(e, signal_map, signals))
                    .collect();
                Ok(IrExpr::Concat(parts?))
            }
            Expr::Replicate { count, expr: inner } => {
                if let Some(folded) = self.try_fold_const(expr)? {
                    return Ok(folded);
                }
                let c = const_eval_params(count, &self.param_vals).map_err(|e| {
                    let (l, c) = expr_location(count);
                    self.elab_diag_at(
                        DiagCode::SimulationError,
                        format!("cannot evaluate replication count: {}", e),
                        l,
                        c,
                    )
                })? as usize;
                let inner_expr = self.elaborate_expr(inner, signal_map, signals)?;
                Ok(IrExpr::Replicate(c, Box::new(inner_expr)))
            }
            Expr::UnaryOp { op, expr: inner } => {
                if let Some(folded) = self.try_fold_const(expr)? {
                    return Ok(folded);
                }
                let inner_expr = self.elaborate_expr(inner, signal_map, signals)?;
                let ir_op = map_unary_op(op)?;
                Ok(IrExpr::UnaryOp(ir_op, Box::new(inner_expr)))
            }
            Expr::BinaryOp { op, lhs, rhs } => {
                if let Some(folded) = self.try_fold_const(expr)? {
                    return Ok(folded);
                }
                // REAL literal fold: `1.5 + 2.25` — eval biner hanya kenal
                // is_real utk operand SIGNAL; literal+literal jatuh jalur int
                // → NaN. Fold real di sini (bug #6).
                if let (Some(a), Some(b)) = (real_literal(lhs), real_literal(rhs)) {
                    if let Some(v) = fold_binary_real(op, a, b) {
                        return Ok(v);
                    }
                }
                let lhs_expr = self.elaborate_expr(lhs, signal_map, signals)?;
                let rhs_expr = self.elaborate_expr(rhs, signal_map, signals)?;
                let ir_op = map_binary_op(op)?;
                Ok(IrExpr::BinaryOp(
                    ir_op,
                    Box::new(lhs_expr),
                    Box::new(rhs_expr),
                ))
            }
            Expr::TernaryOp {
                cond,
                true_expr,
                false_expr,
            } => {
                if let Some(folded) = self.try_fold_const(expr)? {
                    return Ok(folded);
                }
                let ir_cond = self.elaborate_expr(cond, signal_map, signals)?;
                let ir_true = self.elaborate_expr(true_expr, signal_map, signals)?;
                let ir_false = self.elaborate_expr(false_expr, signal_map, signals)?;
                Ok(IrExpr::Cond(
                    Box::new(ir_cond),
                    Box::new(ir_true),
                    Box::new(ir_false),
                ))
            }
            Expr::PartSelect {
                expr: inner,
                base,
                width,
            } => {
                let inner_expr = self.elaborate_expr(inner, signal_map, signals)?;
                if let IrExpr::Signal(sid, _) = &inner_expr {
                    let sig_width = signals[*sid].width;
                    if let (Ok(base_c), Ok(width_c)) = (
                        const_eval_params(base, &self.param_vals),
                        const_eval_params(width, &self.param_vals),
                    ) {
                        let base = base_c as usize;
                        let width = width_c as usize;
                        if width > 0 {
                            // Arbitrary slice statis `sig[base+:width]` / `sig[msb:lsb]`:
                            // bila dalam batas → RangeSelect biasa.
                            // bila OOB → tetap RangeSelect (bukan ExprPartSelect).
                            // RangeSelect evaluator sudah menerapkan §11.5.1 dengan benar:
                            // bit dalam batas = nilai asli, bit di luar batas = X.
                            // Dulu ExprPartSelect dipakai sebagai anti-wrap guard, tapi
                            // evaluator ExprPartSelect menghitung index dari base ke atas
                            // (bukan LSB→MSB), sehingga arbitrary slice statis seperti
                            // `a[3:0]` pada a 2-bit menghasilkan XXXX alih-alih xx01.
                            // Bug ditemukan fuzzer EMI (dead-code mempengaruhi hasil).
                            let msb = base.checked_add(width - 1).unwrap_or(base);
                            if msb < sig_width {
                                Ok(IrExpr::RangeSelect(*sid, msb, base))
                            } else {
                                // Out-of-range: RangeSelect dengan msb OOB.
                                // Evaluator akan isi bit dalam batas + X untuk OOB.
                                Ok(IrExpr::RangeSelect(*sid, msb, base))
                            }
                        } else {
                            Ok(IrExpr::RangeSelect(*sid, base.min(sig_width), base))
                        }
                    } else {
                        let base_expr = self.elaborate_expr(base, signal_map, signals)?;
                        let width_expr = self.elaborate_expr(width, signal_map, signals)?;
                        Ok(IrExpr::ExprPartSelect(
                            Box::new(inner_expr),
                            Box::new(base_expr),
                            Box::new(width_expr),
                        ))
                    }
                } else if let (Ok(base_c), Ok(width_c)) = (
                    const_eval_params(base, &self.param_vals),
                    const_eval_params(width, &self.param_vals),
                ) {
                    let base = base_c as usize;
                    let width = width_c as usize;
                    if width > 0 {
                        // Anti-wrap seperti cabang signal di atas.
                        match base.checked_add(width - 1) {
                            Some(msb) => {
                                Ok(IrExpr::ExprRangeSelect(Box::new(inner_expr), msb, base))
                            }
                            None => Ok(IrExpr::ExprPartSelect(
                                Box::new(inner_expr),
                                Box::new(IrExpr::Const(LogicVec::from_u64(base_c as u64, 64))),
                                Box::new(IrExpr::Const(LogicVec::from_u64(width_c as u64, 32))),
                            )),
                        }
                    } else {
                        Ok(IrExpr::ExprRangeSelect(Box::new(inner_expr), base, base))
                    }
                } else {
                    let base_expr = self.elaborate_expr(base, signal_map, signals)?;
                    let width_expr = self.elaborate_expr(width, signal_map, signals)?;
                    Ok(IrExpr::ExprPartSelect(
                        Box::new(inner_expr),
                        Box::new(base_expr),
                        Box::new(width_expr),
                    ))
                }
            }
            Expr::Paren(inner) => self.elaborate_expr(inner, signal_map, signals),
            // `i++`/`++i` dalam ekspresi: baca nilai sekarang, tulis balik
            // ±1 ke lvalue yang sama saat runtime (LRM 1800 §11.4.1).
            Expr::IncDec {
                expr: inner,
                inc,
                pre,
            } => {
                let read = self.elaborate_expr(inner, signal_map, signals)?;
                let lv = self.elaborate_lvalue(inner, signal_map, signals)?;
                Ok(IrExpr::IncDec {
                    read: Box::new(read),
                    lv: Box::new(lv),
                    inc: *inc,
                    pre: *pre,
                })
            }
            Expr::FuncCall {
                name,
                args,
                line,
                col,
                ..
            } if name.starts_with("$") => {
                self.elaborate_sys_func_call(name, args, line, col, signal_map, signals)
            }
            _ => self.elaborate_expr_tail(expr, signal_map, signals),
        }
    }

    /// Arm ekor `elaborate_expr` (FuncCall non-sistem, String, MethodCall,
    /// MemberAccess, Inside, dst.). Dipisah dari `elaborate_expr` supaya
    /// binggkai stack tiap fungsi tetap kecil: bingkai versi monolitik
    /// ~163 KB pada build debug membuat rekursi ekspresi bersarang
    /// melebihi stack thread 2 MB (stack overflow, metamorphic fuzz).
    fn elaborate_expr_tail(
        &self,
        expr: &Expr,
        signal_map: &HashMap<Symbol, SignalId>,
        signals: &[SignalInfo],
    ) -> Result<IrExpr, SimError> {
        match expr {
            Expr::FuncCall {
                name,
                args,
                line,
                col,
                ..
            } if name.starts_with("$") => {
                self.elaborate_sys_func_call(name, args, line, col, signal_map, signals)
            }
            _ => self.elaborate_expr_rest(expr, signal_map, signals),
        }
    }

    /// Elaborasi pemanggilan fungsi sistem (`$display`, `$clog2`, ...).
    /// Dipisah agar bingkai stack tiap fungsi tetap kecil — bingkai versi
    /// monolitik ~163 KB pada build debug membuat rekursi ekspresi bersarang
    /// melebihi stack thread 2 MB (stack overflow, metamorphic fuzz).
    fn elaborate_sys_func_call(
        &self,
        name: &Symbol,
        args: &[Expr],
        line: &usize,
        col: &usize,
        signal_map: &HashMap<Symbol, SignalId>,
        signals: &[SignalInfo],
    ) -> Result<IrExpr, SimError> {
        match name.as_str() {
            "$signed" => {
                if args.len() != 1 {
                    return Err(self.elab_diag(
                        DiagCode::ParamMismatch,
                        "$signed requires exactly one argument",
                    ));
                }
                let inner = self.elaborate_expr(&args[0], signal_map, signals)?;
                Ok(IrExpr::Signed(Box::new(inner)))
            }
            "$unsigned" => {
                if args.len() != 1 {
                    return Err(self.elab_diag(
                        DiagCode::ParamMismatch,
                        "$unsigned requires exactly one argument",
                    ));
                }
                // LRM §6.24.3: $unsigned(x) memperlakukan x sebagai UNSIGNED
                // (ekstensi nol saat penugasan/lebar-konteks, perbandingan &
                // shift logis). Tidak ada varian IrExpr "Unsigned" — gunakan
                // ekivalensi LRM §11.8.1: concat satu-elemen `{x}` bernilai x
                // dengan signedness UNSIGNED (is_signed_expr(Concat) = false).
                // Dulu di-emit sebagai x polos → `$unsigned(sn)` (signed
                // signal) tetap signed → sign-extend (probe q07: 0xFFFFFFFC
                // padahal harus 0xFC).
                let inner = self.elaborate_expr(&args[0], signal_map, signals)?;
                Ok(IrExpr::Concat(vec![inner]))
            }
            "$clog2" => {
                if let Some(arg) = args.first() {
                    match const_eval_params(arg, &self.param_vals) {
                        Ok(val) => {
                            if val <= 1 {
                                return Ok(IrExpr::Const(LogicVec::from_u64(0, 32)));
                            }
                            let n = val as u64;
                            let msb = (64 - n.leading_zeros()) as u64;
                            let result = if n.is_power_of_two() { msb - 1 } else { msb };
                            Ok(IrExpr::Const(LogicVec::from_u64(result, 32)))
                        }
                        Err(e) => {
                            // F40: argumen $clog2 runtime (sinyal / ekspresi
                            // non-konstan seperti `$countones(wstrb)`) — emit
                            // SysFunc runtime, bukan hard error. Engine
                            // mengevaluasi $clog2 di runtime.
                            let (l, c) = expr_location(arg);
                            let _ = e;
                            let ir_arg = self.elaborate_expr(arg, signal_map, signals)?;
                            Ok(IrExpr::SysFunc {
                                name: Symbol::intern("$clog2"),
                                args: vec![ir_arg],
                                line: l,
                                col: c,
                            })
                        }
                    }
                } else {
                    Err(self.elab_diag(DiagCode::ParamMismatch, "$clog2 requires one argument"))
                }
            }
            "$bits" => {
                if let Some(arg) = args.first() {
                    // Signal: SignalInfo.width ALREADY includes unpacked array
                    // depth (total_width = elem_width * depth di-set saat elaborasi).
                    // Jangan kalikan lagi dgn array_depth — itu double-count.
                    let width = resolve_expr_signal(arg, signal_map)
                        .map(|sig_id| {
                            let info = &signals[sig_id];
                            info.width
                        })
                        .or_else(|| self.try_array_param_bits(arg))
                        .or_else(|| {
                            // Typedef (termasuk lokal module): self.typedef_map
                            // dihitung PENUH (range + packed dims + struct fields
                            // via resolve_typedef_width_dims) saat pemrosesan
                            // ModuleItem::Typedef. JANGAN pakai compute_expr_width
                            // utk nama typedef — arm param-nya menghitung
                            // min-bits NILAI (byte_t=8 → 4) & resolve_typedef_
                            // ident_width mengabaikan td.range (logic [7:0] → 1)
                            // → $bits(byte_t)=4 / $bits(pkt_t)=4 (probe q13/q14).
                            match arg {
                                Expr::Ident { name, .. } => self.typedef_map.get(name).copied(),
                                _ => None,
                            }
                        })
                        .or_else(|| {
                            compute_expr_width(
                                arg,
                                signal_map,
                                signals,
                                &self.param_vals,
                                &self.package_symbols,
                            )
                            .ok()
                        })
                        .ok_or_else(|| {
                            self.elab_diag(
                                DiagCode::ModuleNotFound,
                                "$bits argument must resolve to a signal or computable expression",
                            )
                        })?;
                    Ok(IrExpr::Const(LogicVec::from_u64(width as u64, 32)))
                } else {
                    Err(self.elab_diag(DiagCode::ParamMismatch, "$bits requires one argument"))
                }
            }
            "$high" => {
                if let Some(arg) = args.first() {
                    let sig_id = resolve_expr_signal(arg, signal_map).ok_or_else(|| {
                        self.elab_diag(
                            DiagCode::ModuleNotFound,
                            "$high argument must resolve to a signal",
                        )
                    })?;
                    let info = &signals[sig_id];
                    let high = info.msb.max(info.lsb);
                    Ok(IrExpr::Const(LogicVec::from_u64(high as u64, 32)))
                } else {
                    Err(self.elab_diag(DiagCode::ParamMismatch, "$high requires one argument"))
                }
            }
            "$low" => {
                if let Some(arg) = args.first() {
                    let sig_id = resolve_expr_signal(arg, signal_map).ok_or_else(|| {
                        self.elab_diag(
                            DiagCode::ModuleNotFound,
                            "$low argument must resolve to a signal",
                        )
                    })?;
                    let info = &signals[sig_id];
                    let low = info.msb.min(info.lsb);
                    Ok(IrExpr::Const(LogicVec::from_u64(low as u64, 32)))
                } else {
                    Err(self.elab_diag(DiagCode::ParamMismatch, "$low requires one argument"))
                }
            }
            "$left" => {
                if let Some(arg) = args.first() {
                    let sig_id = resolve_expr_signal(arg, signal_map).ok_or_else(|| {
                        self.elab_diag(
                            DiagCode::ModuleNotFound,
                            "$left argument must resolve to a signal",
                        )
                    })?;
                    let info = &signals[sig_id];
                    Ok(IrExpr::Const(LogicVec::from_u64(info.msb as u64, 32)))
                } else {
                    Err(self.elab_diag(DiagCode::ParamMismatch, "$left requires one argument"))
                }
            }
            "$right" => {
                if let Some(arg) = args.first() {
                    let sig_id = resolve_expr_signal(arg, signal_map).ok_or_else(|| {
                        self.elab_diag(
                            DiagCode::ModuleNotFound,
                            "$right argument must resolve to a signal",
                        )
                    })?;
                    let info = &signals[sig_id];
                    Ok(IrExpr::Const(LogicVec::from_u64(info.lsb as u64, 32)))
                } else {
                    Err(self.elab_diag(DiagCode::ParamMismatch, "$right requires one argument"))
                }
            }
            "$size" => {
                if let Some(arg) = args.first() {
                    let sig_id = resolve_expr_signal(arg, signal_map).ok_or_else(|| {
                        self.elab_diag(
                            DiagCode::ModuleNotFound,
                            "$size argument must resolve to a signal",
                        )
                    })?;
                    let info = &signals[sig_id];
                    // $size mengembalikan jumlah elemen pada dimensi pertama.
                    // Prioritas: unpacked array (array_depth) > packed
                    // multi-dimensi (packed_dims[0]) > lebar total.
                    let size = if info.array_depth > 1 {
                        info.array_depth
                    } else if info.packed_dims.len() > 1 {
                        info.packed_dims[0]
                    } else {
                        info.width
                    };
                    Ok(IrExpr::Const(LogicVec::from_u64(size as u64, 32)))
                } else {
                    Err(self.elab_diag(DiagCode::ParamMismatch, "$size requires one argument"))
                }
            }
            "$countones" => {
                if let Some(arg) = args.first() {
                    let ir_arg = self.elaborate_expr(arg, signal_map, signals)?;
                    if let Ok(val) = const_eval_params(arg, &self.param_vals) {
                        let count = (0..64).filter(|i| (val >> i) & 1 == 1).count() as u64;
                        Ok(IrExpr::Const(LogicVec::from_u64(count, 32)))
                    } else {
                        Ok(IrExpr::SysFunc {
                            name: Symbol::intern("$countones"),
                            args: vec![ir_arg],
                            line: *line,
                            col: *col,
                        })
                    }
                } else {
                    Err(self.elab_diag(DiagCode::ParamMismatch, "$countones requires one argument"))
                }
            }
            "$countbits" => {
                if let Some(arg) = args.first() {
                    let ir_arg = self.elaborate_expr(arg, signal_map, signals)?;
                    if let Ok(val) = const_eval_params(arg, &self.param_vals) {
                        // $countbits counts non-zero bits (1, X, Z)
                        let count = (0..64).filter(|i| ((val >> i) & 1) != 0).count() as u64;
                        Ok(IrExpr::Const(LogicVec::from_u64(count, 32)))
                    } else {
                        Ok(IrExpr::SysFunc {
                            name: Symbol::intern("$countbits"),
                            args: vec![ir_arg],
                            line: *line,
                            col: *col,
                        })
                    }
                } else {
                    Err(self.elab_diag(DiagCode::ParamMismatch, "$countbits requires one argument"))
                }
            }
            "$dimensions" => {
                if let Some(arg) = args.first() {
                    // $dimensions returns the number of dimensions of an array
                    // For now, try to resolve from signal info at elaboration time
                    let ir_arg = self.elaborate_expr(arg, signal_map, signals)?;
                    if let Some(sig_id) = resolve_expr_signal(arg, signal_map) {
                        let info = &signals[sig_id];
                        // Packed dimensions + unpacked dimensions
                        let packed_dims = info.packed_dims.len();
                        let unpacked_dims = info.array_dims.len();
                        let dims = packed_dims + unpacked_dims;
                        Ok(IrExpr::Const(LogicVec::from_u64(dims as u64, 32)))
                    } else {
                        Ok(IrExpr::SysFunc {
                            name: Symbol::intern("$dimensions"),
                            args: vec![ir_arg],
                            line: *line,
                            col: *col,
                        })
                    }
                } else {
                    Err(self
                        .elab_diag(DiagCode::ParamMismatch, "$dimensions requires one argument"))
                }
            }
            "$onehot" => {
                if let Some(arg) = args.first() {
                    let ir_arg = self.elaborate_expr(arg, signal_map, signals)?;
                    if let Ok(val) = const_eval_params(arg, &self.param_vals) {
                        let ones = (0..64).filter(|i| (val >> i) & 1 == 1).count();
                        Ok(IrExpr::Const(LogicVec::from_u64(
                            if ones == 1 { 1 } else { 0 },
                            1,
                        )))
                    } else {
                        Ok(IrExpr::SysFunc {
                            name: Symbol::intern("$onehot"),
                            args: vec![ir_arg],
                            line: *line,
                            col: *col,
                        })
                    }
                } else {
                    Err(self.elab_diag(DiagCode::ParamMismatch, "$onehot requires one argument"))
                }
            }
            "$isunknown" => {
                if let Some(arg) = args.first() {
                    let ir_arg = self.elaborate_expr(arg, signal_map, signals)?;
                    if let Ok(val) = const_eval_params(arg, &self.param_vals) {
                        let has_xz = val as u8 >= 0xFE;
                        Ok(IrExpr::Const(LogicVec::from_u64(
                            if has_xz { 1 } else { 0 },
                            1,
                        )))
                    } else {
                        Ok(IrExpr::SysFunc {
                            name: Symbol::intern("$isunknown"),
                            args: vec![ir_arg],
                            line: *line,
                            col: *col,
                        })
                    }
                } else {
                    Err(self.elab_diag(DiagCode::ParamMismatch, "$isunknown requires one argument"))
                }
            }
            "$onehot0" => {
                if let Some(arg) = args.first() {
                    let ir_arg = self.elaborate_expr(arg, signal_map, signals)?;
                    if let Ok(val) = const_eval_params(arg, &self.param_vals) {
                        let ones = (0..64).filter(|i| (val >> i) & 1 == 1).count();
                        Ok(IrExpr::Const(LogicVec::from_u64(
                            if ones <= 1 { 1 } else { 0 },
                            1,
                        )))
                    } else {
                        Ok(IrExpr::SysFunc {
                            name: Symbol::intern("$onehot0"),
                            args: vec![ir_arg],
                            line: *line,
                            col: *col,
                        })
                    }
                } else {
                    Err(self.elab_diag(DiagCode::ParamMismatch, "$onehot0 requires one argument"))
                }
            }
            "$cast" => {
                if args.len() >= 2 {
                    let ir_dest = self.elaborate_expr(&args[0], signal_map, signals)?;
                    let ir_src = self.elaborate_expr(&args[1], signal_map, signals)?;
                    Ok(IrExpr::SysFunc {
                        name: Symbol::intern("$cast"),
                        args: vec![ir_dest, ir_src],
                        line: *line,
                        col: *col,
                    })
                } else {
                    Err(self.elab_diag(DiagCode::ParamMismatch, "$cast requires two arguments"))
                }
            }
            "$typename" => {
                if let Some(arg) = args.first() {
                    let type_name = self.resolve_typename_for_expr(arg, signal_map, signals);
                    Ok(IrExpr::Const(LogicVec::from_string(&type_name)))
                } else {
                    Err(self.elab_diag(DiagCode::ParamMismatch, "$typename requires one argument"))
                }
            }
            _ => {
                let ir_args: Result<Vec<IrExpr>, SimError> = args
                    .iter()
                    .map(|a| self.elaborate_expr(a, signal_map, signals))
                    .collect();
                Ok(IrExpr::SysFunc {
                    name: *name,
                    args: ir_args?,
                    line: *line,
                    col: *col,
                })
            }
        }
    }

    /// Sisa arm ekor `elaborate_expr`: FuncCall non-sistem ("new", method /
    /// user / DPI), String, MethodCall, MemberAccess, Null, Inside,
    /// StreamingConcat, Dist, Cast, StructLit.
    fn elaborate_expr_rest(
        &self,
        expr: &Expr,
        signal_map: &HashMap<Symbol, SignalId>,
        signals: &[SignalInfo],
    ) -> Result<IrExpr, SimError> {
        match expr {
            Expr::FuncCall { name, args, .. } if name == "new" => {
                let ir_args: Result<Vec<IrExpr>, SimError> = args
                    .iter()
                    .map(|a| self.elaborate_expr(a, signal_map, signals))
                    .collect();
                Ok(IrExpr::NewCall {
                    class_name: Symbol::intern(""),
                    args: ir_args?,
                })
            }
            Expr::String(s) => Ok(IrExpr::String(s.clone())),
            Expr::MethodCall {
                obj,
                method,
                args,
                with_clause,
            } => {
                // F36: receiver berupa instance (interface instance seperti
                // `clk_if.set_period_ps(...)`) — tidak terdaftar sebagai signal
                // karena instance interface di-flatten sebagai sub-module.
                // Emit MethodCall dengan obj HierRef(inst); engine resolve dan
                // no-op bila instance tidak punya method signal.
                if let Expr::Ident { name, .. } = obj.as_ref() {
                    if self.is_current_module_instance(name) {
                        let ir_args: Result<Vec<IrExpr>, SimError> = args
                            .iter()
                            .map(|a| self.elaborate_expr(a, signal_map, signals))
                            .collect();
                        let ir_with = match with_clause {
                            Some(wc) => {
                                Some(Box::new(self.elaborate_expr(wc, signal_map, signals)?))
                            }
                            None => None,
                        };
                        return Ok(IrExpr::MethodCall {
                            obj: Box::new(IrExpr::HierRef(*name)),
                            method: *method,
                            args: ir_args?,
                            with_clause: ir_with,
                        });
                    }
                }
                let ir_obj = self.elaborate_expr(obj, signal_map, signals)?;
                let ir_args: Result<Vec<IrExpr>, SimError> = args
                    .iter()
                    .map(|a| self.elaborate_expr(a, signal_map, signals))
                    .collect();
                let ir_with = match with_clause {
                    Some(wc) => Some(Box::new(self.elaborate_expr(wc, signal_map, signals)?)),
                    None => None,
                };
                Ok(IrExpr::MethodCall {
                    obj: Box::new(ir_obj),
                    method: *method,
                    args: ir_args?,
                    with_clause: ir_with,
                })
            }
            Expr::MemberAccess { obj, field } => {
                // Try to resolve as hierarchical signal reference first
                let hier_name = Self::build_hier_name(obj, field.as_str());
                if !hier_name.is_empty() {
                    if let Some(&sig_id) = signal_map.get(&Symbol::intern(&hier_name)) {
                        return Ok(IrExpr::Signal(sig_id, 0));
                    }
                }
                // Nested struct member access (`hw2reg.phy_status.init_wip.de`):
                // resolve offset PENUH via chain (konsisten dgn lvalue path).
                // Tanpa ini `a.b.c` rvalue jadi MemberAccess { obj:
                // RangeSelect(...) } yang runtime salah di-interpretasi sebagai
                // object handle (E9001 object id OOB → sim hang di time 0).
                if let Some((base_name, chain)) =
                    Self::collect_member_chain(obj, *field, &self.param_vals)
                {
                    if let Some(&base_sid) = signal_map.get(&Symbol::intern(&base_name)) {
                        if let Some((msb, lsb)) =
                            self.resolve_struct_chain(base_sid, &chain, signals)
                        {
                            return Ok(IrExpr::RangeSelect(base_sid, msb, lsb));
                        }
                    }
                }
                // Try struct/union member access: resolve obj signal, check struct_fields
                match self.elaborate_expr(obj, signal_map, signals) {
                    Ok(IrExpr::Signal(sig_id, _)) => {
                        let sig_info = &signals[sig_id];
                        // F27: port interface (`bus_if b` — iface_type di-set,
                        // class_name None; vif variabel pakai class_name) →
                        // field diakses via hier path `b.clk` yang di-resolve
                        // engine via hier_signal_map setelah flatten. Bukan
                        // VirtualIfaceAccess (itu untuk vif yang di-bind runtime).
                        if sig_info.iface_type.is_some()
                            && sig_info.class_name.is_none()
                            && !hier_name.is_empty()
                        {
                            return Ok(IrExpr::HierRef(Symbol::intern(&hier_name)));
                        }
                        // Check if this is a virtual interface variable
                        if let Some(ref iface_type) = sig_info.iface_type {
                            // Look up the interface definition to find field width
                            let field_width = if let Some(iface) = self
                                .design
                                .interfaces
                                .iter()
                                .find(|i| i.name == *iface_type)
                            {
                                if let Some(d) = iface
                                    .decls
                                    .iter()
                                    .find(|d| d.names.iter().any(|n| n.name == *field))
                                {
                                    let var = d.names.iter().find(|n| n.name == *field).unwrap();
                                    var.resolved_width(&HashMap::new()).unwrap_or(1)
                                } else {
                                    1
                                }
                            } else {
                                1
                            };
                            return Ok(IrExpr::VirtualIfaceAccess {
                                vif_name: sig_info.name,
                                field: *field,
                                field_width,
                            });
                        }
                        if !sig_info.struct_fields.is_empty() {
                            if let Some(f) =
                                sig_info.struct_fields.iter().find(|f| f.name == *field)
                            {
                                let lsb = f.offset;
                                let msb = f.offset + f.width - 1;
                                return Ok(IrExpr::RangeSelect(sig_id, lsb, msb));
                            }
                            // Field tidak ditemukan di struct — mungkin struct dari package
                            // yang belum fully resolved. Emit warning dan fallback ke
                            // MemberAccess runtime agar elaborasi tidak gagal.
                            let (fl, fc) = expr_location(obj);
                            self.elab_warn_at(
                                DiagCode::ModuleNotFound,
                                format!(
                                    "field '{}' not found in struct type (width {})",
                                    field, sig_info.width
                                ),
                                fl,
                                fc,
                            );
                            return Ok(IrExpr::MemberAccess {
                                obj: Box::new(IrExpr::Signal(sig_id, 0)),
                                field: *field,
                            });
                        }
                        Ok(IrExpr::MemberAccess {
                            obj: Box::new(IrExpr::Signal(sig_id, 0)),
                            field: *field,
                        })
                    }
                    Ok(IrExpr::HierRef(base_name)) => {
                        // Nested hier: inner already returned HierRef("u1.u"),
                        // combine with field → HierRef("u1.u.v").
                        let combined = format!("{}.{}", base_name.as_str(), field.as_str());
                        Ok(IrExpr::HierRef(Symbol::intern(&combined)))
                    }
                    Ok(ir_obj) => Ok(IrExpr::MemberAccess {
                        obj: Box::new(ir_obj),
                        field: *field,
                    }),
                    Err(_) => {
                        // If obj can't be elaborated (e.g., instance name), emit a HierRef
                        // that the engine can resolve at runtime using the flattened signal list.
                        // Guard: `build_hier_name` kembali "" utk obj non-Ident/MemberAccess
                        // (mis. `arr[i].field`, `ns[0].field` dgn indeks tak-resolved) →
                        // emit `HierRef("")` → engine kalah `signal '' not found` (RT0001
                        // nama KOSONG — diagnostic tak berguna, ditemukan mivon-fuzz dari
                        // concat `tl_h_i[i].a_source[...]` opentitan). Fallback ke nama
                        // field agar pesan error memuat sinyal yang sebenarnya.
                        if hier_name.is_empty() {
                            return Ok(IrExpr::HierRef(*field));
                        }
                        Ok(IrExpr::HierRef(Symbol::intern(&hier_name)))
                    }
                }
            }
            Expr::Null => Ok(IrExpr::Const(LogicVec::from_u64(0, 64))),
            Expr::Inside {
                expr: inner,
                range_list,
            } => {
                let inner_ir = self.elaborate_expr(inner, signal_map, signals)?;
                let mut list_ir = Vec::with_capacity(range_list.len());
                for item in range_list {
                    // Range inside `{[a:b]}` — parser menyisipkan RangeSelect
                    // dengan base literal 0 sebagai penanda rentang. Untuk
                    // operan runtime, emit IrExpr::InsideRange agar engine
                    // memeriksa `lo <= val <= hi` (bukan bit-slice).
                    if let Expr::RangeSelect {
                        expr: base,
                        msb,
                        lsb,
                    } = item
                    {
                        if matches!(base.as_ref(), Expr::Value(Value::Decimal(0))) {
                            // `inside {[a:b]}`: a adalah batas BAWAH, b batas atas
                            // (sintaks range inside, bukan bit-slice). Parser
                            // menyimpan msb=a, lsb=b.
                            let lo = self.elaborate_expr(msb, signal_map, signals)?;
                            let hi = self.elaborate_expr(lsb, signal_map, signals)?;
                            list_ir.push(IrExpr::InsideRange {
                                expr: Box::new(inner_ir.clone()),
                                lo: Box::new(lo),
                                hi: Box::new(hi),
                            });
                            continue;
                        }
                    }
                    list_ir.push(self.elaborate_expr(item, signal_map, signals)?);
                }
                Ok(IrExpr::Inside {
                    expr: Box::new(inner_ir),
                    list: list_ir,
                })
            }
            Expr::StreamingConcat {
                op,
                slice_size,
                slices,
            } => {
                let mut ir_slices = Vec::new();
                for sl in slices {
                    ir_slices.push(self.elaborate_expr(sl, signal_map, signals)?);
                }
                let ir_slice_size = if let Some(ss) = slice_size {
                    match const_eval_params(ss, &self.param_vals) {
                        Ok(v) if v > 0 => Some(v as usize),
                        Ok(_) => {
                            return Err(self.elab_diag(
                                DiagCode::ParamMismatch,
                                "streaming slice_size must be > 0",
                            ))
                        }
                        Err(_) => {
                            return Err(self.elab_diag(
                                DiagCode::ParamMismatch,
                                "slice_size must be a constant expression",
                            ))
                        }
                    }
                } else {
                    None
                };
                Ok(IrExpr::StreamingConcat {
                    op: op.clone(),
                    slice_size: ir_slice_size,
                    slices: ir_slices,
                })
            }
            Expr::Dist { expr: inner, items } => {
                let inner_ir = self.elaborate_expr(inner, signal_map, signals)?;
                let ir_items = items
                    .iter()
                    .map(|di| match di {
                        mivon_ast::DistItem::Value(e, mivon_ast::DistWeight::Item(w)) => {
                            let ev = self
                                .elaborate_expr(e, signal_map, signals)
                                .unwrap_or(IrExpr::Const(LogicVec::from_u64(0, 32)));
                            let lo = if let IrExpr::Const(ref lv) = ev {
                                Some(lv.to_u64() as i64)
                            } else {
                                None
                            };
                            mivon_ir::IrDistItem {
                                range_lo: lo,
                                range_hi: lo,
                                weight_type: mivon_ir::DistWeightType::Item,
                                weight: *w as i64,
                            }
                        }
                        mivon_ast::DistItem::Value(e, mivon_ast::DistWeight::Range(w)) => {
                            let ev = self
                                .elaborate_expr(e, signal_map, signals)
                                .unwrap_or(IrExpr::Const(LogicVec::from_u64(0, 32)));
                            let lo = if let IrExpr::Const(ref lv) = ev {
                                Some(lv.to_u64() as i64)
                            } else {
                                None
                            };
                            mivon_ir::IrDistItem {
                                range_lo: lo,
                                range_hi: lo,
                                weight_type: mivon_ir::DistWeightType::Range,
                                weight: *w as i64,
                            }
                        }
                        mivon_ast::DistItem::Range(lo, hi, mivon_ast::DistWeight::Item(w)) => {
                            let lo_v = const_eval_with_params(lo, &self.param_vals).ok();
                            let hi_v = const_eval_with_params(hi, &self.param_vals).ok();
                            mivon_ir::IrDistItem {
                                range_lo: lo_v,
                                range_hi: hi_v,
                                weight_type: mivon_ir::DistWeightType::Item,
                                weight: *w as i64,
                            }
                        }
                        mivon_ast::DistItem::Range(lo, hi, mivon_ast::DistWeight::Range(w)) => {
                            let lo_v = const_eval_with_params(lo, &self.param_vals).ok();
                            let hi_v = const_eval_with_params(hi, &self.param_vals).ok();
                            mivon_ir::IrDistItem {
                                range_lo: lo_v,
                                range_hi: hi_v,
                                weight_type: mivon_ir::DistWeightType::Range,
                                weight: *w as i64,
                            }
                        }
                    })
                    .collect::<Vec<_>>();
                Ok(IrExpr::Dist {
                    expr: Box::new(inner_ir),
                    items: ir_items,
                })
            }
            Expr::Cast { dtype, expr: inner } => {
                let inner_ir = self.elaborate_expr(inner, signal_map, signals)?;
                let cast_width = match parse_type_spec_str(dtype.as_str()) {
                    Some(dt) => self.resolve_type_width(&dt).unwrap_or(1),
                    // Identifier (parameter/typedef package) — resolve width
                    // agar runtime tidak resize ke 1 bit (data loss). Mis.
                    // `MuBi4Width'(x)` dari `import prim_mubi_pkg::*`.
                    None => self.resolve_cast_name_width(dtype.as_str()).unwrap_or(1),
                };
                // ── Cast real↔integer (LRM 1800 §6.24) ──
                // IR `Cast` hanya menyimpan LEBAR (tidak ada info tipe), sehingga
                // konversi real↔int tak bisa-INFER di runtime: `int'(3.99)`
                // sebelumnya memotong bit-pattern f64 (515396076, bukan 4).
                // Rantai lewat sysfunc: real→`$rtoi` (bulat, ties away from
                // zero), int→`$itor` (f64 64-bit). Constant real di-fold di sini.
                let target_is_real = is_real_type_name(dtype.as_str());
                let inner_is_real = expr_is_real_ty(inner, signal_map, signals);
                if target_is_real && !inner_is_real {
                    return Ok(IrExpr::SysFunc {
                        name: Symbol::intern("$itor"),
                        args: vec![inner_ir],
                        line: 0,
                        col: 0,
                    });
                }
                if !target_is_real && inner_is_real {
                    if let Expr::Value(Value::Real(f)) = strip_parens(inner) {
                        // Constant real → bulat saat elaborasi ( Deterministik ).
                        return Ok(IrExpr::Const(LogicVec::from_u64(
                            if f.is_finite() { f.round() as i64 as u64 } else { 0 },
                            cast_width.max(32),
                        )));
                    }
                    return Ok(IrExpr::SysFunc {
                        name: Symbol::intern("$rtoi"),
                        args: vec![inner_ir],
                        line: 0,
                        col: 0,
                    });
                }
                Ok(IrExpr::Cast {
                    width: cast_width,
                    expr: Box::new(inner_ir),
                })
            }
            // Cast dengan width dari ekspresi: `size'(expr)` (mis. `$clog2(N)'(x)`).
            Expr::CastWidth { width, expr: inner } => {
                let inner_ir = self.elaborate_expr(inner, signal_map, signals)?;
                let cast_width = match const_eval_with_params(width, &self.param_vals) {
                    Ok(w) => (w.max(1)) as usize,
                    Err(_) => {
                        // Width tidak konstanta-folding — fallback 1-bit agar runtime
                        // tidak crash; elaborasi width ekspresi tetap dilakukan agar
                        // referensi signal/param valid.
                        let _ = self.elaborate_expr(width, signal_map, signals);
                        1
                    }
                };
                Ok(IrExpr::Cast {
                    width: cast_width,
                    expr: Box::new(inner_ir),
                })
            }
            Expr::FuncCall {
                name,
                args,
                line,
                col,
                ..
            } if name.starts_with("process::") => {
                let ir_args: Result<Vec<IrExpr>, SimError> = args
                    .iter()
                    .map(|a| self.elaborate_expr(a, signal_map, signals))
                    .collect();
                Ok(IrExpr::SysFunc {
                    name: *name,
                    args: ir_args?,
                    line: *line,
                    col: *col,
                })
            }
            Expr::FuncCall { name, args, .. }
                if name.ends_with("::new")
                    && (self
                        .design
                        .classes
                        .iter()
                        .any(|c| *name == format!("{}::new", c.name))
                        || BUILTIN_UVM_CLASSES
                            .iter()
                            .any(|c| *name == format!("{}::new", c))
                        || name.contains("#")) =>
            {
                let raw_name = name.strip_suffix("::new").unwrap().to_string();
                let class_name = if let Some(hash_pos) = raw_name.find('#') {
                    let base = &raw_name[..hash_pos];
                    let type_spec = &raw_name[hash_pos + 1..];
                    let specialized = format!("{}__param_{}", base, type_spec.replace(',', "_"));
                    let exists_in_design =
                        self.design.classes.iter().any(|c| c.name == specialized);
                    let exists_in_spec = self
                        .specialized_classes
                        .borrow()
                        .iter()
                        .any(|c| c.name == specialized);
                    if !exists_in_design && !exists_in_spec {
                        let orig = self.design.classes.iter().find(|c| c.name == base).cloned();
                        if let Some(mut spec) = orig {
                            let tp_name = spec.type_params.first().map(|tp| tp.name);
                            spec.name = Symbol::intern(&specialized);
                            if let Some(ref param_name) = tp_name {
                                let type_dt = parse_type_spec_str(type_spec);
                                if let Some(ref dt) = type_dt {
                                    spec = substitute_class_types(spec, param_name.as_str(), dt);
                                }
                            }
                            self.specialized_classes.borrow_mut().push(spec);
                        }
                    }
                    Symbol::intern(&specialized)
                } else if BUILTIN_UVM_CLASSES.contains(&raw_name.as_str()) {
                    Symbol::intern(&format!("__{}", raw_name))
                } else {
                    Symbol::intern(&raw_name)
                };
                let ir_args: Result<Vec<IrExpr>, SimError> = args
                    .iter()
                    .map(|a| self.elaborate_expr(a, signal_map, signals))
                    .collect();
                Ok(IrExpr::NewCall {
                    class_name,
                    args: ir_args?,
                })
            }
            Expr::FuncCall {
                name,
                args,
                line,
                col,
                ..
            } if name == "uvm_factory::set_type_override_by_type" => {
                let ir_args: Result<Vec<IrExpr>, SimError> = args
                    .iter()
                    .map(|a| self.elaborate_expr(a, signal_map, signals))
                    .collect();
                Ok(IrExpr::SysFunc {
                    name: *name,
                    args: ir_args?,
                    line: *line,
                    col: *col,
                })
            }
            Expr::FuncCall {
                name,
                args,
                line,
                col,
                ..
            } if name == "uvm_config_db::set"
                || name == "uvm_config_db::get"
                || name == "uvm_resource_db::set"
                || name == "uvm_resource_db::get"
                || name == "uvm_resource_db::exists"
                || name == "uvm_resource_db::read_by_name"
                || name == "uvm_resource_db::write_by_name"
                || name == "uvm_cmdline_processor::get"
                || name == "uvm_root::get"
                || name == "uvm_root::get_top"
                || name == "uvm_root::run_test"
                || name == "uvm_tr_database::get_db"
                || name == "uvm_tr_database::get_stream"
                || name == "uvm_tr_database::get_tr_count"
                || name == "uvm_tr_database::set_stream" =>
            {
                let ir_args: Result<Vec<IrExpr>, SimError> = args
                    .iter()
                    .map(|a| self.elaborate_expr(a, signal_map, signals))
                    .collect();
                // Use SysFunc variant for engine dispatch
                Ok(IrExpr::SysFunc {
                    name: *name,
                    args: ir_args?,
                    line: *line,
                    col: *col,
                })
            }
            Expr::FuncCall {
                name,
                args,
                line,
                col,
            } if name != "new" && name.contains("::") => {
                let fl = if *line > 0 {
                    *line
                } else {
                    expr_location(expr).0
                };
                let fc = if *col > 0 {
                    *col
                } else {
                    expr_location(expr).1
                };
                self.elaborate_package_func_call(name.as_str(), args, signal_map, signals, fl, fc)
            }
            Expr::FuncCall {
                name,
                args,
                line,
                col,
                ..
            } if name == "run_test" => {
                // F18: run_test → IrExpr::SysFunc — dispatch engine menciptakan
                // objek test & menjalankan fase UVM (run_uvm_test). Jalur
                // statement terpisah di stmt.rs (IrStmt::SysCall).
                let ir_args: Result<Vec<IrExpr>, SimError> = args
                    .iter()
                    .map(|a| self.elaborate_expr(a, signal_map, signals))
                    .collect();
                Ok(IrExpr::SysFunc {
                    name: *name,
                    args: ir_args?,
                    line: *line,
                    col: *col,
                })
            }
            Expr::FuncCall { name, args, .. } if name != "new" => {
                // LANG-40: panggilan `let name(args)` — substitusi parameter
                // formal dengan argumen, lalu elaborate body.
                if let Some(ld) = self.let_decls.get(name) {
                    if !ld.params.is_empty() && ld.params.len() == args.len() {
                        let map: HashMap<Symbol, &Expr> = ld
                            .params
                            .iter()
                            .zip(args.iter())
                            .map(|(p, a)| (*p, a))
                            .collect();
                        let body =
                            mivon_ast::inline_util::substitute_let_args(ld.expr.clone(), &map);
                        return self.elaborate_expr(&body, signal_map, signals);
                    }
                }
                if std::env::var("DBG_PKG").is_ok() && name.as_str() == "aes_circ_byte_shift" {
                    let func_exists = self.design.modules.iter().any(|m| {
                        m.items
                            .iter()
                            .any(|mi| matches!(mi, ModuleItem::Func(fd) if fd.name == *name))
                    });
                    eprintln!(
                        "DBG-PKG: aes_circ_byte_shift module={:?} func_exists={} import_sets={:?}",
                        self.current_module,
                        func_exists,
                        self.collect_import_sets()
                            .iter()
                            .map(|(p, i)| format!("{}::{}", p.as_str(), i.as_str()))
                            .collect::<Vec<_>>()
                    );
                }
                let is_dpi = self.dpi_import_names.contains(name);
                if is_dpi {
                    let ir_args: Result<Vec<IrExpr>, SimError> = args
                        .iter()
                        .map(|a| self.elaborate_expr(a, signal_map, signals))
                        .collect();
                    let return_width = self
                        .design
                        .modules
                        .iter()
                        .flat_map(|m| m.items.iter())
                        .filter_map(|item| {
                            if let ModuleItem::DpiImport(d) = item {
                                Some(d)
                            } else {
                                None
                            }
                        })
                        .find(|d| d.name == *name)
                        .and_then(|d| d.return_type.as_ref())
                        .map(|dt| dt.width())
                        .unwrap_or(32);
                    Ok(IrExpr::DpiCall {
                        name: *name,
                        args: ir_args?,
                        return_width,
                    })
                } else {
                    // Check if this is a module-level function (recursive, not inlined)
                    let func_exists = self.design.modules.iter().any(|m| {
                        m.items
                            .iter()
                            .any(|mi| matches!(mi, ModuleItem::Func(fd) if fd.name == *name))
                    });
                    if func_exists {
                        let ir_args: Result<Vec<IrExpr>, SimError> = args
                            .iter()
                            .map(|a| self.elaborate_expr(a, signal_map, signals))
                            .collect();
                        return Ok(IrExpr::FuncCall {
                            func_name: *name,
                            args: ir_args?,
                        });
                    }
                    // Plain-name package function via import (pkg::* / pkg::item)
                    let (fl, fc) = expr_location(expr);
                    if let Some(ir) = self.elaborate_imported_package_func_call(
                        name.as_str(),
                        args,
                        signal_map,
                        signals,
                        fl,
                        fc,
                    )? {
                        return Ok(ir);
                    }
                    // Fungsi eksternal/DPI yang TIDAK terdaftar (mis.
                    // `riscv_cosim_step`/`otbn_model_*` dari .svh yang tidak
                    // ter-include, fungsi UVM helper) → warning + stub
                    // DpiCall (engine return 0) agar elaborasi tidak gagal.
                    // Perbaikan global: fungsi C eksternal tidak bisa
                    // dieksekusi mivon — degrade ke stub dengan warning
                    // eksplisit, bukan hard error yang mematikan modul.
                    // HANYA bila design mendeklarasikan DPI import (konteks
                    // C eksternal nyata); tanpa DPI import, function tak
                    // dikenal tetap hard error (E3001) — regresi test
                    // `test_elab_err_func_not_found_*`.
                    if !self.design_has_dpi_imports() {
                        return Err(self.elab_diag_at(
                            DiagCode::ModuleNotFound,
                            format!("function '{}' not found", name),
                            expr_location(expr).0,
                            expr_location(expr).1,
                        ));
                    }
                    self.elab_warn_at(
                        DiagCode::ModuleNotFound,
                        format!(
                            "function '{}' not found (not a DPI import) — treated as external DPI stub (returns 0)",
                            name
                        ),
                        expr_location(expr).0,
                        expr_location(expr).1,
                    );
                    let ir_args: Result<Vec<IrExpr>, SimError> = args
                        .iter()
                        .map(|a| self.elaborate_expr(a, signal_map, signals))
                        .collect();
                    Ok(IrExpr::DpiCall {
                        name: *name,
                        args: ir_args?,
                        return_width: 32,
                    })
                }
            }
            // Struct assignment pattern sebagai nilai utuh (inisialisasi var
            // struct / koneksi port). Tanpa layout typedef nilai tidak bisa
            // di-pack — evaluasi zero-fill (perilaku lama: pola bernama
            // di-discard menjadi FillLit 0). Member access tetap di-const-eval
            // benar lewat arm MemberAccess + key param_vals.
            Expr::StructLit { .. } => Ok(IrExpr::FillLit(mivon_ir::LogicVal::Zero)),
            _ => Err(self.elab_diag_at(
                DiagCode::ModuleNotFound,
                "expression type not yet supported".to_string(),
                expr_location(expr).0,
                expr_location(expr).1,
            )),
        }
    }

    fn elaborate_package_func_call(
        &self,
        name: &str,
        args: &[Expr],
        signal_map: &HashMap<Symbol, SignalId>,
        signals: &[SignalInfo],
        line: usize,
        col: usize,
    ) -> Result<IrExpr, SimError> {
        let (pkg_name, func_name) = name.split_once("::").ok_or_else(|| {
            self.elab_diag_at(
                DiagCode::ModuleNotFound,
                format!("invalid function name '{}'", name),
                line,
                col,
            )
        })?;
        self.elaborate_package_func(pkg_name, func_name, args, signal_map, signals, line, col)
    }

    /// Kumpulkan set import aktif: import $unit (`design.unit_imports`) + import
    /// di body module saat ini (`ModuleItem::Import`). Dipakai oleh resolusi
    /// plain-name package (fungsi & konstanta/array).
    fn collect_import_sets(&self) -> Vec<(Symbol, Symbol)> {
        let mut import_sets: Vec<(Symbol, Symbol)> = self.design.unit_imports.clone();
        if let Some(mod_name) = self.current_module {
            if let Some(module) = self.design.modules.iter().find(|m| m.name == mod_name) {
                for item in &module.items {
                    if let ModuleItem::Import {
                        package,
                        item: import_item,
                    } = item
                    {
                        import_sets.push((*package, *import_item));
                    }
                }
            }
        }
        import_sets
    }

    /// Cari package yang mengimpor fungsi plain-name (via `import pkg::*`/`import pkg::item`
    /// di $unit atau body module), lalu resolve fungsi package tersebut.
    fn elaborate_imported_package_func_call(
        &self,
        name: &str,
        args: &[Expr],
        signal_map: &HashMap<Symbol, SignalId>,
        signals: &[SignalInfo],
        line: usize,
        col: usize,
    ) -> Result<Option<IrExpr>, SimError> {
        let import_sets = self.collect_import_sets();
        let name_sym = Symbol::intern(name);
        for (package, import_item) in import_sets {
            let Some(pkg_items) = self.package_symbols.get(&package) else {
                continue;
            };
            let matched = if import_item.as_str() == "*" {
                matches!(pkg_items.get(&name_sym), Some(PackageItem::Function(_)))
            } else {
                import_item == name
                    && matches!(pkg_items.get(&name_sym), Some(PackageItem::Function(_)))
            };
            if matched {
                let ir = self.elaborate_package_func(
                    package.as_str(),
                    name,
                    args,
                    signal_map,
                    signals,
                    line,
                    col,
                )?;
                return Ok(Some(ir));
            }
        }
        // Fallback: fungsi plain-name yang dipanggil dari dalam body function
        // package (inline). Simbol di scope package harus ter-resolve dari
        // package asal, bukan hanya dari import module.
        if let Some(inline_pkg) = self.inline_func_pkg.get() {
            if let Some(pkg_items) = self.package_symbols.get(&inline_pkg) {
                if matches!(pkg_items.get(&name_sym), Some(PackageItem::Function(_))) {
                    let ir = self.elaborate_package_func(
                        inline_pkg.as_str(),
                        name,
                        args,
                        signal_map,
                        signals,
                        line,
                        col,
                    )?;
                    return Ok(Some(ir));
                }
            }
        }
        // Fallback 2: fungsi package yang disalin ke body module via `import pkg::func`.
        // Setelah AST inline pass, body function berisi panggilan fungsi saudara plain-name
        // yang berasal dari package yang sama dengan fungsi yang disalin.
        if let Some(mod_name) = self.current_module {
            if let Some(src_pkgs) = self.func_source_pkg.get(&mod_name) {
                let mut seen: Vec<Symbol> = Vec::new();
                for &pkg in src_pkgs.values() {
                    if seen.contains(&pkg) {
                        continue;
                    }
                    seen.push(pkg);
                    if let Some(pkg_items) = self.package_symbols.get(&pkg) {
                        if matches!(pkg_items.get(&name_sym), Some(PackageItem::Function(_))) {
                            let ir = self.elaborate_package_func(
                                pkg.as_str(),
                                name,
                                args,
                                signal_map,
                                signals,
                                line,
                                col,
                            )?;
                            return Ok(Some(ir));
                        }
                    }
                }
            }
        }
        Ok(None)
    }

    /// Hitung total bit untuk referensi array utuh (param package) dalam `$bits`,
    /// mis. `$bits(pkg::ARR)` atau `$bits(ARR)` via `import pkg::*`.
    /// Mengembalikan `elem_width * num_elements` bila ter-resolve, selain itu None.
    fn try_array_param_bits(&self, arg: &Expr) -> Option<usize> {
        let candidates: Vec<(Symbol, Symbol)> = match arg {
            Expr::ScopedIdent { package, item, .. } => vec![(*package, *item)],
            Expr::Ident { name, .. } => {
                let mut out = Vec::new();
                for (package, import_item) in self.collect_import_sets() {
                    let Some(pkg_items) = self.package_symbols.get(&package) else {
                        continue;
                    };
                    let matched = if import_item.as_str() == "*" {
                        matches!(pkg_items.get(name), Some(PackageItem::Param(_)))
                    } else {
                        import_item == *name
                            && matches!(pkg_items.get(name), Some(PackageItem::Param(_)))
                    };
                    if matched {
                        out.push((package, *name));
                    }
                }
                out
            }
            _ => return None,
        };

        for (package, item) in candidates {
            let qname = Symbol::intern(&format!("{}::{}", package.as_str(), item.as_str()));
            let Some(elems) = self.pkg_const_arrays.get(&qname) else {
                continue;
            };
            if elems.is_empty() {
                continue;
            }
            let Some(pkg_items) = self.package_symbols.get(&package) else {
                continue;
            };
            let elem_width: usize = match pkg_items.get(&item) {
                Some(PackageItem::Param(p)) => {
                    if let Some((msb, lsb)) = &p.range {
                        match (
                            const_eval_with_params(msb, &self.param_vals),
                            const_eval_with_params(lsb, &self.param_vals),
                        ) {
                            (Ok(m), Ok(l)) => m.abs_diff(l) as usize + 1,
                            _ => p.dtype.as_ref().map(|d| d.width()).unwrap_or(32),
                        }
                    } else {
                        p.dtype.as_ref().map(|d| d.width()).unwrap_or(32)
                    }
                }
                _ => continue,
            };
            return Some(elem_width * elems.len());
        }
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn elaborate_package_func(
        &self,
        pkg_name: &str,
        func_name: &str,
        args: &[Expr],
        signal_map: &HashMap<Symbol, SignalId>,
        signals: &[SignalInfo],
        line: usize,
        col: usize,
    ) -> Result<IrExpr, SimError> {
        let func = match self
            .package_symbols
            .get(&Symbol::intern(pkg_name))
            .and_then(|items| items.get(&Symbol::intern(func_name)))
            .and_then(|item| {
                if let PackageItem::Function(f) = item {
                    Some(f)
                } else {
                    None
                }
            }) {
            Some(f) => f,
            None => {
                // Fungsi package yang tidak ditemukan (mis.
                // `cip_base_pkg::get_rand_lc_tx_val` — helper UVM dari package
                // yang tidak tersedia di design) → warning + stub DpiCall
                // (engine return 0). Perbaikan global: elaborasi tetap lanjut,
                // bukan skip modul.
                self.elab_warn_at(
                    DiagCode::ModuleNotFound,
                    format!(
                        "function '{}' not found in package '{}' — treated as external DPI stub (returns 0)",
                        func_name, pkg_name
                    ),
                    line,
                    col,
                );
                let ir_args: Result<Vec<IrExpr>, SimError> = args
                    .iter()
                    .map(|a| self.elaborate_expr(a, signal_map, signals))
                    .collect();
                return Ok(IrExpr::DpiCall {
                    name: Symbol::intern(&format!("{}::{}", pkg_name, func_name)),
                    args: ir_args?,
                    return_width: 32,
                });
            }
        };

        // Find return expression
        let ret_expr = func
            .stmts
            .iter()
            .find_map(|s| {
                if let Stmt::Return(Some(e)) = s {
                    Some(e.clone())
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                self.elab_diag_at(
                    DiagCode::ModuleNotFound,
                    format!(
                        "function '{}::{}' has no return expression",
                        pkg_name, func_name
                    ),
                    line,
                    col,
                )
            })?;

        // Function body hanya boleh inline bila berisi TEPAT satu statement
        // `return <expr>;`. Fungsi dengan assignment/lokal/loop (mis. mubi4_and
        // yang menulis `out[k]` dalam loop) tidak bisa di-inline sederhana —
        // pakai pemanggilan runtime agar statement body dieksekusi di engine.
        let body_is_trivial =
            func.stmts.len() == 1 && matches!(func.stmts.first(), Some(Stmt::Return(_)));
        if !body_is_trivial {
            let ir_args: Result<Vec<IrExpr>, SimError> = args
                .iter()
                .map(|a| self.elaborate_expr(a, signal_map, signals))
                .collect();
            let qualified = Symbol::intern(&format!("{}::{}", pkg_name, func_name));
            return Ok(IrExpr::FuncCall {
                func_name: qualified,
                args: ir_args?,
            });
        }

        // Substitute formal parameters with actual arguments
        let mut result = *ret_expr;

        // First: resolve package-scoped identifiers (e.g. MuBi4True → constant value)
        let pkg_symbols = self.package_symbols.get(&Symbol::intern(pkg_name));
        if let Some(items) = pkg_symbols {
            // Collect all enum member names and their values from typedefs
            let mut enum_member_values: HashMap<Symbol, Expr> = HashMap::new();
            for item in items.values() {
                if let PackageItem::Typedef(td) = item {
                    if let DataType::EnumType { members, .. } = &td.dtype {
                        for (member_name, member_expr) in members {
                            if let Some(expr) = member_expr {
                                enum_member_values.insert(*member_name, expr.clone());
                            }
                        }
                    }
                }
            }
            for (item_name, item) in items {
                if let PackageItem::Param(p) = item {
                    if let Some(expr) = &p.default {
                        result = Self::substitute_ident_in_expr(
                            result,
                            item_name.as_str(),
                            expr.clone(),
                        );
                    }
                }
            }
            // Substitute enum member names with their constant values
            for (member_name, member_value) in &enum_member_values {
                result = Self::substitute_ident_in_expr(
                    result,
                    member_name.as_str(),
                    member_value.clone(),
                );
            }
        }

        // Then: substitute formal parameters with actual arguments
        for (i, param) in func.ports.iter().enumerate() {
            if let Some(arg) = args.get(i) {
                result = Self::substitute_ident_in_expr(result, param.name.as_str(), arg.clone());
            }
        }

        // Resolve fungsi saudara plain-name di body inline dari package asal
        // (mis. `mubi4_and` di dalam `mubi4_and_hi`), bukan hanya dari import
        // module. Disimpan di Cell sementara lalu di-restore.
        let prev_inline_pkg = self.inline_func_pkg.get();
        self.inline_func_pkg.set(Some(Symbol::intern(pkg_name)));
        let elaborated = self.elaborate_expr(&result, signal_map, signals);
        self.inline_func_pkg.set(prev_inline_pkg);
        elaborated
    }

    fn substitute_ident_in_expr(expr: Expr, target: &str, replacement: Expr) -> Expr {
        match expr {
            Expr::Ident { ref name, .. } if name == target => replacement,
            Expr::Ident { .. } => expr,
            Expr::Value(_) | Expr::String(_) | Expr::Null | Expr::FillLit(_) => expr,
            Expr::BinaryOp { op, lhs, rhs } => Expr::BinaryOp {
                op,
                lhs: Box::new(Self::substitute_ident_in_expr(
                    *lhs,
                    target,
                    replacement.clone(),
                )),
                rhs: Box::new(Self::substitute_ident_in_expr(
                    *rhs,
                    target,
                    replacement.clone(),
                )),
            },
            Expr::UnaryOp { op, expr: inner } => Expr::UnaryOp {
                op,
                expr: Box::new(Self::substitute_ident_in_expr(
                    *inner,
                    target,
                    replacement.clone(),
                )),
            },
            Expr::Paren(inner) => Expr::Paren(Box::new(Self::substitute_ident_in_expr(
                *inner,
                target,
                replacement.clone(),
            ))),
            Expr::IncDec {
                expr: inner,
                inc,
                pre,
            } => Expr::IncDec {
                expr: Box::new(Self::substitute_ident_in_expr(
                    *inner,
                    target,
                    replacement.clone(),
                )),
                inc,
                pre,
            },
            Expr::Concat(exprs) => Expr::Concat(
                exprs
                    .into_iter()
                    .map(|e| Self::substitute_ident_in_expr(e, target, replacement.clone()))
                    .collect(),
            ),
            Expr::Replicate { count, expr: inner } => Expr::Replicate {
                count: Box::new(Self::substitute_ident_in_expr(
                    *count,
                    target,
                    replacement.clone(),
                )),
                expr: Box::new(Self::substitute_ident_in_expr(
                    *inner,
                    target,
                    replacement.clone(),
                )),
            },
            Expr::RangeSelect {
                expr: inner,
                msb,
                lsb,
            } => Expr::RangeSelect {
                expr: Box::new(Self::substitute_ident_in_expr(
                    *inner,
                    target,
                    replacement.clone(),
                )),
                msb: Box::new(Self::substitute_ident_in_expr(
                    *msb,
                    target,
                    replacement.clone(),
                )),
                lsb: Box::new(Self::substitute_ident_in_expr(
                    *lsb,
                    target,
                    replacement.clone(),
                )),
            },
            Expr::BitSelect { expr: inner, index } => Expr::BitSelect {
                expr: Box::new(Self::substitute_ident_in_expr(
                    *inner,
                    target,
                    replacement.clone(),
                )),
                index: Box::new(Self::substitute_ident_in_expr(
                    *index,
                    target,
                    replacement.clone(),
                )),
            },
            Expr::PartSelect {
                expr: inner,
                base,
                width,
            } => Expr::PartSelect {
                expr: Box::new(Self::substitute_ident_in_expr(
                    *inner,
                    target,
                    replacement.clone(),
                )),
                base: Box::new(Self::substitute_ident_in_expr(
                    *base,
                    target,
                    replacement.clone(),
                )),
                width: Box::new(Self::substitute_ident_in_expr(
                    *width,
                    target,
                    replacement.clone(),
                )),
            },
            Expr::ScopedIdent {
                package,
                item,
                line,
                col,
            } => {
                if package == target {
                    match &replacement {
                        Expr::Ident { name, .. } => Expr::ScopedIdent {
                            package: *name,
                            item,
                            line,
                            col,
                        },
                        _ => Expr::ScopedIdent {
                            package,
                            item,
                            line,
                            col,
                        },
                    }
                } else {
                    Expr::ScopedIdent {
                        package,
                        item,
                        line,
                        col,
                    }
                }
            }
            Expr::Cast { dtype, expr: inner } => Expr::Cast {
                expr: Box::new(Self::substitute_ident_in_expr(
                    *inner,
                    target,
                    replacement.clone(),
                )),
                dtype,
            },
            Expr::CastWidth { width, expr: inner } => Expr::CastWidth {
                width: Box::new(Self::substitute_ident_in_expr(
                    *width,
                    target,
                    replacement.clone(),
                )),
                expr: Box::new(Self::substitute_ident_in_expr(*inner, target, replacement)),
            },
            Expr::MemberAccess { obj, field } => Expr::MemberAccess {
                obj: Box::new(Self::substitute_ident_in_expr(
                    *obj,
                    target,
                    replacement.clone(),
                )),
                field,
            },
            Expr::TernaryOp {
                cond,
                true_expr,
                false_expr,
            } => Expr::TernaryOp {
                cond: Box::new(Self::substitute_ident_in_expr(
                    *cond,
                    target,
                    replacement.clone(),
                )),
                true_expr: Box::new(Self::substitute_ident_in_expr(
                    *true_expr,
                    target,
                    replacement.clone(),
                )),
                false_expr: Box::new(Self::substitute_ident_in_expr(
                    *false_expr,
                    target,
                    replacement.clone(),
                )),
            },
            Expr::FuncCall {
                name: n,
                args: a,
                line,
                col,
            } => Expr::FuncCall {
                name: n,
                args: a
                    .into_iter()
                    .map(|e| Self::substitute_ident_in_expr(e, target, replacement.clone()))
                    .collect(),
                line,
                col,
            },
            Expr::MethodCall {
                obj,
                method,
                args,
                with_clause,
            } => Expr::MethodCall {
                obj: Box::new(Self::substitute_ident_in_expr(
                    *obj,
                    target,
                    replacement.clone(),
                )),
                method,
                args: args
                    .into_iter()
                    .map(|e| Self::substitute_ident_in_expr(e, target, replacement.clone()))
                    .collect(),
                with_clause,
            },
            Expr::Inside {
                expr: inner,
                range_list,
            } => Expr::Inside {
                expr: Box::new(Self::substitute_ident_in_expr(
                    *inner,
                    target,
                    replacement.clone(),
                )),
                range_list: range_list
                    .into_iter()
                    .map(|e| Self::substitute_ident_in_expr(e, target, replacement.clone()))
                    .collect(),
            },
            Expr::StreamingConcat {
                op,
                slice_size,
                slices,
            } => Expr::StreamingConcat {
                op,
                slice_size: slice_size.map(|ss| {
                    Box::new(Self::substitute_ident_in_expr(
                        *ss,
                        target,
                        replacement.clone(),
                    ))
                }),
                slices: slices
                    .into_iter()
                    .map(|e| Self::substitute_ident_in_expr(e, target, replacement.clone()))
                    .collect(),
            },
            Expr::Dist { expr: inner, items } => Expr::Dist {
                expr: Box::new(Self::substitute_ident_in_expr(
                    *inner,
                    target,
                    replacement.clone(),
                )),
                items,
            },
            Expr::StructLit { members } => Expr::StructLit {
                members: members
                    .into_iter()
                    .map(|m| match m {
                        mivon_ast::expr::StructLitMember::Named(n, e) => {
                            mivon_ast::expr::StructLitMember::Named(
                                n,
                                Self::substitute_ident_in_expr(e, target, replacement.clone()),
                            )
                        }
                        mivon_ast::expr::StructLitMember::Positional(e) => {
                            mivon_ast::expr::StructLitMember::Positional(
                                Self::substitute_ident_in_expr(e, target, replacement.clone()),
                            )
                        }
                        mivon_ast::expr::StructLitMember::Default(e) => {
                            mivon_ast::expr::StructLitMember::Default(
                                Self::substitute_ident_in_expr(e, target, replacement.clone()),
                            )
                        }
                    })
                    .collect(),
            },
        }
    }

    pub(crate) fn elaborate_expr_to_signal(
        &self,
        expr: &Expr,
        signal_map: &HashMap<Symbol, SignalId>,
    ) -> Result<SignalId, SimError> {
        match expr {
            Expr::Ident { name, line, col } => signal_map
                .get(name)
                .ok_or_else(|| {
                    self.elab_diag_at(
                        DiagCode::UndefinedSignal,
                        format!("signal '{}' not found", name),
                        *line,
                        *col,
                    )
                })
                .copied(),
            Expr::MethodCall { .. } => Err(self.elab_diag(
                DiagCode::ModuleNotFound,
                "method calls cannot resolve to a signal",
            )),
            Expr::MemberAccess { .. } => Err(self.elab_diag(
                DiagCode::ModuleNotFound,
                "member access cannot resolve to a signal",
            )),
            _ => Err(self.elab_diag(
                DiagCode::ModuleNotFound,
                "expected simple signal identifier",
            )),
        }
    }

    /// Create a signal from a port connection expression.
    /// For simple identifiers, resolves directly.
    /// For compound expressions (e.g. ~clk_i), creates an implicit wire + continuous assign.
    ///
    /// `is_output`: arah koneksi. Untuk port OUTPUT/INOUT yang diconnect ke
    /// ekspresi non-trivial (bit/range-select, concat — mis. `.q(s[0])` atau
    /// `.cout (b[72:0])`), synth wire adalah signal port child (didorong child
    /// via flatten). Proses copy harus MENDORONG ekspresi parent dari synth
    /// (`assign <expr> = __port_*`), BUKAN backflow `assign __port_* = <expr>`
    /// yang membaca parent slice yang belum ter-drive (sebelumnya: parent
    /// slice tak pernah dapat nilai child — `s = xxxx`; ditemukan mivon-fuzz).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn instance_port_expr_to_signal(
        &self,
        expr: &Expr,
        signal_map: &HashMap<Symbol, SignalId>,
        signals: &mut Vec<SignalInfo>,
        next_id: &mut SignalId,
        processes: &mut Vec<Process>,
        hint_name: &str,
        is_output: bool,
    ) -> Result<SignalId, SimError> {
        // Try simple signal resolution first
        if let Ok(sid) = self.elaborate_expr_to_signal(expr, signal_map) {
            return Ok(sid);
        }
        // F27: koneksi instance interface (`dut u_dut (.b(b))` di parent — `b`
        // adalah nama instance interface, bukan signal). Buat handle wire
        // 64-bit (lebar = port interface) agar flatten bisa mencocokkan port;
        // field diakses lewat hier_signal_map, handle ini tak pernah dibaca.
        // Juga handle `b.master` (MemberAccess modport): `master u_m(b.master)`
        // — obj `b` interface instance, modport `.master`.
        if let Expr::MemberAccess { obj, .. } = expr {
            if let Expr::Ident { name, .. } = obj.as_ref() {
                if !signal_map.contains_key(name) && self.is_interface_instance(name.as_str()) {
                    let iface_hint = hint_name.replace('.', "_");
                    let sig_name = format!("__iface_{}", iface_hint);
                    if !signal_map.contains_key(&Symbol::intern(&sig_name)) {
                        let class_name = Some(*name);
                        let sid = *next_id;
                        *next_id += 1;
                        signals.push(SignalInfo {
                            name: Symbol::intern(&sig_name),
                            width: 64,
                            kind: SignalKind::Wire,
                            net_type: NetType::Wire,
                            multi_driver: false,
                            init_val: mivon_ir::LogicVec::new(64),
                            array_depth: 1,
                            elem_width: 64,
                            array_dims: vec![],
                            class_name,
                            is_string: false,
                            is_mailbox: false,
                            is_semaphore: false,
                            is_real: false,
                            is_2state: true,
                            is_dynamic: false,
                            is_queue: false,
                            is_associative: false,
                            is_signed: false,
                            is_const: false,
                            msb: 63,
                            lsb: 0,
                            struct_fields: vec![],
                            packed_dims: vec![],
                            delay_rise: None,
                            delay_fall: None,
                            iface_type: None,
                            iface_modport: None,
                        });
                        return Ok(sid);
                    }
                    return signal_map
                        .get(&Symbol::intern(&sig_name))
                        .copied()
                        .ok_or_else(|| {
                            self.elab_diag(
                                mivon_core::diagnostics::diagnostic::DiagCode::UndefinedSignal,
                                format!("interface handle '{}' not found", sig_name),
                            )
                        });
                }
            }
        }
        if let Expr::Ident { name, .. } = expr {
            if !signal_map.contains_key(name) && self.is_interface_instance(name.as_str()) {
                // Nama unik per koneksi (hint = `inst.port`) agar dua koneksi
                // ke instance interface yang sama tidak menabrak nama signal.
                let sig_name = format!(
                    "__iface_{}",
                    if hint_name.is_empty() {
                        name.as_str().to_string()
                    } else {
                        hint_name.replace('.', "_")
                    }
                );
                // F28: simpan NAMA INSTANCE interface di class_name — dipakai
                // flatten utk membuat alias hier `port.<field> -> inst.<field>`
                // (nama port child bisa berbeda dari nama instance parent).
                // class_name handle interface tidak dipakai sebagai object.
                let class_name = Some(Symbol::intern(name.as_str()));
                let sid = *next_id;
                *next_id += 1;
                signals.push(SignalInfo {
                    name: Symbol::intern(&sig_name),
                    width: 64,
                    kind: SignalKind::Wire,
                    net_type: NetType::Wire,
                    multi_driver: false,
                    init_val: mivon_ir::LogicVec::new(64),
                    array_depth: 1,
                    elem_width: 64,
                    array_dims: vec![],
                    class_name,
                    is_string: false,
                    is_mailbox: false,
                    is_semaphore: false,
                    is_real: false,
                    is_2state: true,
                    is_dynamic: false,
                    is_queue: false,
                    is_associative: false,
                    is_signed: false,
                    is_const: false,
                    msb: 63,
                    lsb: 0,
                    struct_fields: vec![],
                    packed_dims: vec![],
                    delay_rise: None,
                    delay_fall: None,
                    iface_type: None,
                    iface_modport: None,
                });
                return Ok(sid);
            }
        }
        // For compound expressions, create an implicit wire
        let width_val = compute_expr_width(
            expr,
            signal_map,
            signals,
            &self.param_vals,
            &self.package_symbols,
        )
        .map_err(|e| {
            self.elab_diag(
                mivon_core::diagnostics::diagnostic::DiagCode::WidthMismatch,
                format!("width computation failed for port '{}': {}", hint_name, e),
            )
        })?;
        let width = if width_val > 0 { width_val } else { 1 };
        if width > 1_000_000 {
            eprintln!(
                "[DBG-WIDTH] port '{}' huge width {} expr={:?} in module {}",
                hint_name,
                width,
                expr,
                self.current_module.map(|s| s.as_str()).unwrap_or("?")
            );
            if let Expr::Concat(items) = expr {
                for it in items {
                    let w = compute_expr_width(
                        it,
                        signal_map,
                        signals,
                        &self.param_vals,
                        &self.package_symbols,
                    )
                    .unwrap_or(0);
                    eprintln!("[DBG-WIDTH]   item {:?} width={}", it, w);
                }
            }
        }
        // Create a unique implicit signal name
        let sig_name = format!("__port_{}", hint_name.replace('.', "_"));
        let sid = *next_id;
        *next_id += 1;
        signals.push(SignalInfo {
            name: Symbol::intern(&sig_name),
            width,
            kind: SignalKind::Wire,
            net_type: NetType::Wire,
            multi_driver: false,
            init_val: mivon_ir::LogicVec::fill(mivon_ir::LogicVal::Z, width),
            array_depth: 1,
            elem_width: width,
            array_dims: vec![],
            class_name: None,
            is_string: false,
            is_mailbox: false,
            is_semaphore: false,
            is_real: false,
            is_2state: false,
            is_dynamic: false,
            is_queue: false,
            is_associative: false,
            is_signed: false,
            is_const: false,
            msb: width - 1,
            lsb: 0,
            struct_fields: vec![],
            packed_dims: vec![],
            delay_rise: None,
            delay_fall: None,
            iface_type: None,
            iface_modport: None,
        });
        // Tambah proses. Arah tergantung port direction:
        // - INPUT: `assign __port_* = <expr>` — ekspresi parent → synth wire
        //   (child membaca nilai tersebut).
        // - OUTPUT/INOUT: `assign <expr-lvalue> = __port_*` — synth wire (yang
        //   didorong child via flatten remap) MENDORONG ekspresi parent.
        //   Tanpa proses ini, slice parent (mis. `s[0]`, `b[72:0]`) TIDAK
        //   pernah menerima nilai child → `sum = xxxx` (ditemukan mivon-fuzz).
        let port_name = Symbol::intern(&format!("port_assign_{}", hint_name.replace('.', "_")));
        if is_output {
            // Jalur lvalue: `expr` yang sama kini menjadi target tulis
            // (bit/range select/concat dari signal parent). Gagal elaborate
            // (mis. index dinamis) → fallback no-op (tulis synth ke sintas
            // sendiri); jangan blokir desain.
            let lv = self
                .elaborate_lvalue(expr, signal_map, signals)
                .unwrap_or(IrLValue::Signal(sid, 0));
            processes.push(Process::Combinational {
                name: port_name,
                sensitivity: vec![SignalSensitivity::whole(sid)],
                body: vec![IrStmt::BlockingAssign {
                    lhs: lv,
                    rhs: IrExpr::Signal(sid, 0),
                    delay: None,
                }],
            });
        } else {
            let ir_expr = self.elaborate_expr(expr, signal_map, signals)?;
            let sensitivity = collect_sensitivity(expr, signal_map)
                .into_iter()
                .map(SignalSensitivity::whole)
                .collect();
            processes.push(Process::Combinational {
                name: port_name,
                sensitivity,
                body: vec![IrStmt::BlockingAssign {
                    lhs: IrLValue::Signal(sid, 0),
                    rhs: ir_expr,
                    delay: None,
                }],
            });
        }
        Ok(sid)
    }

    /// F27: apakah `name` adalah nama instance interface di module yang sedang
    /// dielaborasi (bukan signal). Dipakai koneksi port `.b(b)` agar instance
    /// interface bisa disambungkan ke port interface child module.
    /// Verilog-2001 implicit net: identifier TAK DIKENAL yang dipakai sebagai
    /// koneksi port instance otomatis menjadi wire 1-bit (aturan implicit net
    /// untuk koneksi output/inout). Contoh OpenTitan: `prim_sec_anchor_buf u
    /// (.out_o({diff_n_buf, diff_p_buf}))` — `diff_n_buf`/`diff_p_buf` tidak
    /// dideklarasikan di module tapi sah; pola sama untuk `scanmode` di
    /// chip_earlgrey_cw340 dan `es_rng_fips` di chip_darjeeling_asic. Tidak
    /// menyentuh: konstanta (param_vals/pkg_param_ctx), system `$`, `this`,
    /// `uvm_test_top`, nama instance interface (ditangani terpisah), dan
    /// hier-ref (nama mengandung `.`).
    pub(crate) fn implicit_declare_port_idents(
        &self,
        expr: &Expr,
        signal_map: &mut HashMap<Symbol, SignalId>,
        signals: &mut Vec<SignalInfo>,
        next_id: &mut SignalId,
    ) {
        fn walk(e: &Expr, out: &mut Vec<&str>) {
            match e {
                Expr::Ident { name, .. } => out.push(name.as_str()),
                Expr::MemberAccess { obj, .. } => walk(obj, out),
                Expr::BitSelect { expr, .. }
                | Expr::RangeSelect { expr, .. }
                | Expr::PartSelect { expr, .. }
                | Expr::Paren(expr) => walk(expr, out),
                Expr::Concat(parts) => {
                    for p in parts {
                        walk(p, out);
                    }
                }
                Expr::Replicate { expr, .. } => walk(expr, out),
                Expr::UnaryOp { expr, .. } => walk(expr, out),
                Expr::BinaryOp { lhs, rhs, .. } => {
                    walk(lhs, out);
                    walk(rhs, out);
                }
                Expr::TernaryOp {
                    cond,
                    true_expr,
                    false_expr,
                    ..
                } => {
                    walk(cond, out);
                    walk(true_expr, out);
                    walk(false_expr, out);
                }
                _ => {}
            }
        }
        let mut names = Vec::new();
        walk(expr, &mut names);
        for name in names {
            if name.starts_with('$')
                || name == "this"
                || name == "uvm_test_top"
                || name.contains('.')
                || name.contains('[')
            {
                continue;
            }
            let sym = Symbol::intern(name);
            if signal_map.contains_key(&sym) {
                continue;
            }
            if self.param_vals.contains_key(&sym) || self.pkg_param_ctx.contains_key(&sym) {
                continue;
            }
            if self.is_interface_instance(name) {
                continue;
            }
            let sid = *next_id;
            *next_id += 1;
            signal_map.insert(sym, sid);
            signals.push(SignalInfo {
                name: sym,
                width: 1,
                kind: SignalKind::Wire,
                net_type: NetType::Wire,
                multi_driver: false,
                init_val: mivon_ir::LogicVec::fill(mivon_ir::LogicVal::Z, 1),
                array_depth: 1,
                elem_width: 1,
                array_dims: vec![],
                class_name: None,
                is_string: false,
                is_mailbox: false,
                is_semaphore: false,
                is_real: false,
                is_2state: false,
                is_dynamic: false,
                is_queue: false,
                is_associative: false,
                is_signed: false,
                is_const: false,
                msb: 0,
                lsb: 0,
                struct_fields: vec![],
                packed_dims: vec![],
                delay_rise: None,
                delay_fall: None,
                iface_type: None,
                iface_modport: None,
            });
        }
    }

    pub(crate) fn is_interface_instance(&self, name: &str) -> bool {
        let Some(cur) = self.current_module else {
            return false;
        };
        let sym = Symbol::intern(name);
        let Some(m) = self.design.modules.iter().find(|m| m.name == cur) else {
            return false;
        };
        m.items.iter().any(|it| match it {
            ModuleItem::Instance(inst) => {
                inst.instance_name == sym
                    && self
                        .design
                        .interfaces
                        .iter()
                        .any(|i| i.name == inst.module_name)
            }
            _ => false,
        })
    }

    pub(crate) fn resolve_typedef_width(
        &self,
        dtype: &DataType,
        range: Option<&ExprRange>,
    ) -> usize {
        self.resolve_typedef_width_dims(dtype, range, &[], &self.param_vals)
    }

    /// Resolve lebar typedef, mengalikan semua packed dimensions (range
    /// pertama + `extra_packed_dims`). Untuk `[4:0][4:0][W-1:0]` hasilnya
    /// 5 * 5 * W. `const_eval_params` dipakai agar batas yang memakai
    /// parameter modul (mis. `W`) bisa di-resolve.
    pub(crate) fn resolve_typedef_width_dims(
        &self,
        dtype: &DataType,
        range: Option<&ExprRange>,
        extra_packed_dims: &[ExprRange],
        params: &HashMap<Symbol, i64>,
    ) -> usize {
        let mut total = 1usize;
        let mut any = false;
        let eval = |er: &ExprRange, total: &mut usize, any: &mut bool| {
            let msb = const_eval_params(&er.msb, params).or_else(|_| const_eval_simple(&er.msb));
            let lsb = const_eval_params(&er.lsb, params).or_else(|_| const_eval_simple(&er.lsb));
            if let (Ok(msb), Ok(lsb)) = (msb, lsb) {
                let w = if msb >= lsb {
                    (msb - lsb + 1) as usize
                } else {
                    (lsb - msb + 1) as usize
                };
                if w > 0 {
                    *total *= w;
                    *any = true;
                }
            }
        };
        if let Some(er) = range {
            eval(er, &mut total, &mut any);
        }
        for er in extra_packed_dims {
            eval(er, &mut total, &mut any);
        }
        if any {
            return total;
        }
        match dtype {
            DataType::UserDefined(name) => self.typedef_map.get(name).copied().unwrap_or(64),
            DataType::Signed(inner) => self.resolve_typedef_width(inner, None),
            DataType::StructType { .. } | DataType::UnionType { .. } => {
                // Lebar typedef struct/union harus dihitung dari MEMBER, bukan
                // `dtype.width()` — yang menganggap member tanpa range eksplisit
                // (termasuk nested typedef struct seperti `phy_status_t
                // phy_status;`) sebagai 1-bit. Akibatnya `typedef struct {
                // phy_status_t phy_status; logic [7:0] other; } hw2reg_t;`
                // di-size 1+8=9 (harus 5+8=13) → bit-select member struct
                // bersarang OOB / salah. `compute_struct_fields` menghitung
                // lebar per member via `struct_field_from_member` (range →
                // typedef → nested struct), jadi konsisten dgn struct_fields
                // yang dipakai member access.
                let fields = self.compute_struct_fields(dtype);
                if fields.is_empty() {
                    dtype.width()
                } else if matches!(dtype, DataType::UnionType { .. }) {
                    fields.iter().map(|f| f.width).max().unwrap_or(1)
                } else {
                    fields.iter().map(|f| f.width).sum()
                }
            }
            _ => dtype.width(),
        }
    }

    /// Untuk `pkg::ARR[idx]` dengan `ARR` array param package (tersimpan di
    /// `pkg_const_arrays`) dan index dinamis/konstan yang tidak ter-fold:
    /// bangun ulang konstanta array penuh dari elemen-elemennya + lebar
    /// elemen (`total_width / jumlah_elemen`). Mengembalikan None bila bukan
    /// array param package atau lebarnya tidak bisa ditentukan → fallback ke
    /// bit-select biasa.
    ///
    /// Konteks bug: `parameter logic [15:0][3:0] PRESENT_SBOX4 = {...}` di
    /// prim_cipher_pkg dipakai `PRESENT_SBOX4[x[k*4 +: 4]]`. `pkg::SBOX`
    /// ter-flatten ke ELEMEN PERTAMA di param context, sehingga bit-select
    /// dinamis menghasilkan part-select lebar 1 pada nilai yang salah — sbox
    /// bernilai salah + WR0102 `lhs=4, rhs=1` (496x per desain).
    pub(crate) fn pkg_array_param_element(
        &self,
        inner_ast: &Expr,
        _inner_ir: &IrExpr,
    ) -> Option<(IrExpr, usize)> {
        let Expr::ScopedIdent { package, item, .. } = inner_ast else {
            return None;
        };
        let qualified = Symbol::intern(&format!("{}::{}", package.as_str(), item.as_str()));
        let elems = self.pkg_const_arrays.get(&qualified)?;
        if elems.is_empty() {
            return None;
        }
        // Total width dari default expression param (struktur AST): untuk
        // `{4'h2, 4'h1, ...}` → 64. Nilai hasil eval tidak bisa dipakai
        // (i64 tanpa info lebar; `pkg::name` malah berisi elemen pertama).
        let total_w = (|| {
            let pkg_items = self.package_symbols.get(package)?;
            let mivon_ast::types::PackageItem::Param(p) = pkg_items.get(item)? else {
                return None;
            };
            let default = p.default.as_ref()?;
            const_fold_width(default, &self.param_vals)
        })()?;
        if total_w == 0 || total_w > 64 || total_w % elems.len() != 0 {
            return None;
        }
        let elem_w = total_w / elems.len();
        if elem_w == 0 {
            return None;
        }
        let mask = if elem_w >= 64 {
            u64::MAX
        } else {
            (1u64 << elem_w) - 1
        };
        let mut acc: u64 = 0;
        for (i, v) in elems.iter().enumerate() {
            acc |= (*v as u64 & mask) << (i * elem_w);
        }
        Some((IrExpr::Const(LogicVec::from_u64(acc, total_w)), elem_w))
    }

    fn resolve_typename_for_expr(
        &self,
        expr: &Expr,
        signal_map: &HashMap<Symbol, SignalId>,
        signals: &[SignalInfo],
    ) -> String {
        match expr {
            Expr::Ident { name, .. } => {
                if let Some(sig_id) = signal_map.get(name) {
                    let info = &signals[*sig_id];
                    let type_str = match info.kind {
                        SignalKind::Logic => "logic",
                        SignalKind::Reg => "reg",
                        SignalKind::Wire => "wire",
                        SignalKind::Input => "input",
                        SignalKind::Output => "output",
                        SignalKind::Inout => "inout",
                    };
                    let signed_str = if info.is_signed { " signed" } else { "" };
                    format!("{}{} [{}:{}]", type_str, signed_str, info.msb, info.lsb)
                } else if self.param_vals.contains_key(name) {
                    "parameter".to_string()
                } else {
                    "unknown".to_string()
                }
            }
            Expr::Value(v) => match v {
                mivon_ast::Value::Decimal(_) => "int".to_string(),
                mivon_ast::Value::Real(_) => "real".to_string(),
                mivon_ast::Value::Binary { .. }
                | mivon_ast::Value::Hex { .. }
                | mivon_ast::Value::Octal { .. } => "logic".to_string(),
            },
            Expr::String(_) => "string".to_string(),
            Expr::BitSelect { expr: inner, .. } | Expr::RangeSelect { expr: inner, .. } => {
                self.resolve_typename_for_expr(inner, signal_map, signals)
            }
            Expr::Paren(inner) => self.resolve_typename_for_expr(inner, signal_map, signals),
            Expr::Cast { dtype, .. } => format!("{}", dtype),
            Expr::ScopedIdent { package, item, .. } => {
                format!("{}::{}", package, item)
            }
            Expr::Concat(items) => {
                if items.is_empty() {
                    "logic".to_string()
                } else {
                    let first = self.resolve_typename_for_expr(&items[0], signal_map, signals);
                    format!("{} (concat)", first)
                }
            }
            Expr::Replicate { expr: inner, .. } => {
                self.resolve_typename_for_expr(inner, signal_map, signals)
            }
            _ => "unknown".to_string(),
        }
    }
}

/// Hitung lebar sub-elemen dari sebuah chunk `RangeSelect(sid, msb, lsb)` pada
/// packed array multi-dimensi. Contoh: `state` bertipe `logic [4:0][4:0][W-1:0]`
/// (packed_dims [5,5,W]). `state[0]` → chunk 320-bit (5*W); BitSelect `[0]`
/// pada chunk tersebut memilih ELEMEN (W-bit), bukan bit tunggal. Rumus:
///   chunk_w = sig.width / (packed_dims[0] * ... * packed_dims[k-1])
///   sub-elemen = chunk_w / packed_dims[k]
fn sub_elem_width_from_packed(
    signals: &[mivon_ir::SignalInfo],
    sid: mivon_ir::SignalId,
    chunk_w: usize,
) -> Option<usize> {
    let sig = signals.get(sid)?;
    if sig.packed_dims.is_empty() {
        return None;
    }
    if chunk_w >= sig.width {
        return Some(sig.width / sig.packed_dims[0]);
    }
    let mut acc = 1usize;
    for (k, d) in sig.packed_dims.iter().enumerate() {
        acc *= *d;
        if acc * chunk_w == sig.width {
            if let Some(next) = sig.packed_dims.get(k + 1) {
                return Some(chunk_w / *next);
            }
            return Some(1);
        }
    }
    None
}

// CATATAN: impl DataType { fn width() } dan impl DeclKind { fn default_width() }
// sudah dipindahkan ke src/ast/types.rs karena method-method ini adalah
// bagian dari definisi tipe AST, bukan tanggung jawab elaborator.
// Lihat src/ast/types.rs untuk implementasinya.
//
// CATATAN: parse_type_spec_str() sudah dipindahkan ke src/elaboration/util/type_util.rs
// dan di-re-export via util/mod.rs.

/// Nilai literal REAL pada ekspresi AST (`1.5`) — None bila bukan literal real.
fn real_literal(e: &Expr) -> Option<f64> {
    match e {
        Expr::Value(Value::Real(v)) => Some(*v),
        _ => None,
    }
}

/// Fold binary REAL literal+literal selama elaborasi (bug #6): evaluator biner
/// hanya kenal is_real utk operand SIGNAL; literal murni jatuh jalur integer
/// → NaN. Aritmetik → Const real 64-bit; relasional → Const bool 32-bit.
fn fold_binary_real(op: &mivon_ast::BinaryOp, a: f64, b: f64) -> Option<IrExpr> {
    use mivon_ast::BinaryOp as B;
    let real = |v: f64| IrExpr::Const(LogicVec::from_u64(v.to_bits(), 64));
    let int = |v: bool| IrExpr::Const(LogicVec::from_u64(v as u64, 32));
    match op {
        B::Add => Some(real(a + b)),
        B::Sub => Some(real(a - b)),
        B::Mul => Some(real(a * b)),
        B::Div => Some(real(a / b)),
        B::Mod => Some(real(a % b)),
        B::Power => Some(real(a.powf(b))),
        B::Lt => Some(int(a < b)),
        B::Le => Some(int(a <= b)),
        B::Gt => Some(int(a > b)),
        B::Ge => Some(int(a >= b)),
        B::Eq | B::CaseEq | B::EqWild => Some(int(a == b)),
        B::Neq | B::CaseNeq | B::NeqWild => Some(int(a != b)),
        _ => None,
    }
}
