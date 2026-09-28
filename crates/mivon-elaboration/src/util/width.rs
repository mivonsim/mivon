//! ──────────────────────────────────────────────────────────────────────────────
//! CATATAN: File ini adalah bagian dari pemisahan util.rs (SRP Refactoring).
//! Tanggung jawab: Expression width computation.
//!
//! Fungsi:
//!   - compute_expr_width() — hitung lebar expression (dalam bit)
//!
//! ──────────────────────────────────────────────────────────────────────────────

use std::collections::HashMap;

use mivon_ast::types::const_eval_with_params;
use mivon_ast::*;
use mivon_core::intern::Symbol;
use mivon_ir::*;

/// Hitung lebar (width) expression dalam bit.
/// Mendukung: Ident, Value, FuncCall, Paren, UnaryOp, BinaryOp,
/// Concat, Replicate, TernaryOp, RangeSelect, BitSelect, PartSelect,
/// MemberAccess, Cast.
pub fn compute_expr_width(
    expr: &Expr,
    signal_map: &HashMap<Symbol, SignalId>,
    signals: &[SignalInfo],
    param_vals: &HashMap<Symbol, i64>,
    package_symbols: &HashMap<Symbol, HashMap<Symbol, PackageItem>>,
) -> Result<usize, String> {
    match expr {
        Expr::Ident { name, .. } => {
            if let Some(sig_id) = signal_map.get(name) {
                // SignalInfo.width ALREADY includes unpacked array depth
                // (total_width = elem_width * depth di-set saat elaborasi),
                // jadi jangan kalikan lagi dgn array_depth (double-count).
                let info = &signals[*sig_id];
                Ok(info.width)
            } else if let Some(&val) = param_vals.get(name) {
                let abs = val.unsigned_abs();
                Ok(if val == 0 {
                    1
                } else {
                    64 - (abs.leading_zeros() as usize)
                }
                .max(1))
            } else if let Some(w) = resolve_typedef_ident_width(name, package_symbols) {
                Ok(w)
            } else {
                Err(format!("cannot determine width of '{}'", name))
            }
        }
        Expr::Value(v) => match v {
            Value::Binary { width, .. } => Ok(width.unwrap_or(1)),
            Value::Hex { width, .. } => Ok(width.unwrap_or(1)),
            Value::Octal { width, .. } => Ok(width.unwrap_or(1)),
            Value::Decimal(_) => Ok(32),
            Value::Real(_) => Ok(64),
        },
        Expr::FillLit(_) => Ok(1),
        // Struct assignment pattern — lebar dijumlahkan dari member (pola
        // posisional bernilai utuh) atau 1 (perilaku lama: pola bernama jadi
        // FillLit 0). Caller biasanya memakai lebar TARGET, jadi fallback 1
        // aman.
        Expr::StructLit { members } => {
            let mut w = 0usize;
            let mut all_pos = true;
            for m in members {
                match m {
                    mivon_ast::expr::StructLitMember::Positional(e) => {
                        if let Ok(mw) =
                            compute_expr_width(e, signal_map, signals, param_vals, package_symbols)
                        {
                            w += mw;
                        }
                    }
                    mivon_ast::expr::StructLitMember::Named(_, e)
                    | mivon_ast::expr::StructLitMember::Default(e) => {
                        all_pos = false;
                        let _ = e;
                    }
                }
            }
            if all_pos && w > 0 {
                Ok(w)
            } else {
                Ok(1)
            }
        }
        Expr::FuncCall { name, args, .. } if name == "$bits" || name == "$size" => {
            if let Some(arg) = args.first() {
                compute_expr_width(arg, signal_map, signals, param_vals, package_symbols)
            } else {
                Err("$bits requires one argument".to_string())
            }
        }
        Expr::FuncCall { name, .. } => {
            if let Some(width) = param_vals.get(name) {
                let abs = width.unsigned_abs();
                Ok(if *width == 0 {
                    1
                } else {
                    64 - (abs.leading_zeros() as usize)
                }
                .max(1))
            } else if let Some((pkg_name, func_name)) = name.split_once("::") {
                let pkg_sym = Symbol::intern(pkg_name);
                let func_sym = Symbol::intern(func_name);
                if let Some(pkg) = package_symbols.get(&pkg_sym) {
                    if let Some(PackageItem::Function(f)) = pkg.get(&func_sym) {
                        let ret_width = if let Some(r) = &f.range {
                            if let (Ok(msb), Ok(lsb)) = (
                                const_eval_with_params(&r.msb, param_vals),
                                const_eval_with_params(&r.lsb, param_vals),
                            ) {
                                (msb.abs_diff(lsb).saturating_add(1)) as usize
                            } else {
                                1
                            }
                        } else if let Some(rt) = &f.return_type {
                            // Return type berupa typedef package (mis.
                            // `function automatic mubi4_t mubi4_or_hi(...)`)
                            // — resolve width dari typedef.
                            resolve_dtype_width(rt, package_symbols)
                        } else {
                            1
                        };
                        Ok(ret_width)
                    } else {
                        Err(format!("cannot determine width of function '{}'", name))
                    }
                } else {
                    Err(format!("cannot determine width of function '{}'", name))
                }
            } else {
                // Fungsi plain-name (dipanggil via `import pkg::*`), mis.
                // `mubi4_or_hi(...)`, `lc_tx_test_true_strict(...)` — cari di
                // SEMUA package agar return width ter-resolve. Sebelumnya
                // langsung error → module di-skip ("width computation failed
                // for port ... cannot determine width of function").
                let func_sym = Symbol::intern(name.as_str());
                for pkg in package_symbols.values() {
                    if let Some(PackageItem::Function(f)) = pkg.get(&func_sym) {
                        let ret_width = if let Some(r) = &f.range {
                            if let (Ok(msb), Ok(lsb)) = (
                                const_eval_with_params(&r.msb, param_vals),
                                const_eval_with_params(&r.lsb, param_vals),
                            ) {
                                (msb.abs_diff(lsb).saturating_add(1)) as usize
                            } else {
                                1
                            }
                        } else if let Some(rt) = &f.return_type {
                            resolve_dtype_width(rt, package_symbols)
                        } else {
                            1
                        };
                        if ret_width > 0 {
                            return Ok(ret_width);
                        }
                    }
                }
                Err(format!("cannot determine width of function '{}'", name))
            }
        }
        Expr::Paren(inner) => {
            compute_expr_width(inner, signal_map, signals, param_vals, package_symbols)
        }
        Expr::UnaryOp { op, expr: inner } => match op {
            UnaryOp::ReductionAnd
            | UnaryOp::ReductionNand
            | UnaryOp::ReductionOr
            | UnaryOp::ReductionNor
            | UnaryOp::ReductionXor
            | UnaryOp::ReductionXnor
            | UnaryOp::Not => Ok(1),
            _ => compute_expr_width(inner, signal_map, signals, param_vals, package_symbols),
        },
        Expr::BinaryOp { lhs, rhs, .. } => {
            let lw = compute_expr_width(lhs, signal_map, signals, param_vals, package_symbols)?;
            let rw = compute_expr_width(rhs, signal_map, signals, param_vals, package_symbols)?;
            Ok(lw.max(rw))
        }
        Expr::Concat(items) => {
            let mut total: usize = 0;
            for item in items {
                total = total.saturating_add(compute_expr_width(
                    item,
                    signal_map,
                    signals,
                    param_vals,
                    package_symbols,
                )?);
            }
            Ok(total)
        }
        Expr::Replicate { count, expr: inner } => {
            let c = const_eval_with_params(count, param_vals).unwrap_or(1) as usize;
            let w = compute_expr_width(inner, signal_map, signals, param_vals, package_symbols)?;
            Ok(c.saturating_mul(w))
        }
        Expr::TernaryOp {
            true_expr,
            false_expr,
            ..
        } => {
            let tw =
                compute_expr_width(true_expr, signal_map, signals, param_vals, package_symbols)?;
            let fw =
                compute_expr_width(false_expr, signal_map, signals, param_vals, package_symbols)?;
            Ok(tw.max(fw))
        }
        Expr::RangeSelect { msb, lsb, .. } => {
            if let (Ok(m), Ok(l)) = (
                const_eval_with_params(msb, param_vals),
                const_eval_with_params(lsb, param_vals),
            ) {
                Ok((m.abs_diff(l) + 1) as usize)
            } else {
                Err("dynamic range select width not computable at compile time".to_string())
            }
        }
        Expr::BitSelect { expr: inner, .. } => {
            // `sig[idx]`: single-bit select UNTUK skalar/array 1D. Untuk packed
            // array multidimensi (`logic [1:0][3:0] mubis`), `mubis[0]` memilih
            // SATU ELEMENT (lebar = width/packed_dims[0]), bukan 1 bit.
            if let Expr::Ident { name, .. } = inner.as_ref() {
                if let Some(&sig_id) = signal_map.get(name) {
                    let sig = &signals[sig_id];
                    if sig.packed_dims.len() > 1 && sig.packed_dims[0] > 0 {
                        return Ok((sig.width / sig.packed_dims[0]).max(1));
                    }
                    if sig.array_depth > 1 {
                        return Ok(sig.elem_width.max(1));
                    }
                }
            }
            Ok(1)
        }
        Expr::PartSelect { width, .. } => {
            Ok(const_eval_with_params(width, param_vals).unwrap_or(1) as usize)
        }
        Expr::MemberAccess { obj, field } => {
            // Bug menyamar sbg warning (WR0102): dulu fallback
            // `compute_expr_width(obj)` mengembalikan lebar OBJEK INDUK —
            // untuk `hw2reg.intr_state.done_ch0.d` hasilnya lebar
            // `hw2reg.intr_state`, bukan field `d` → synth wire port melebar,
            // flatten cuma warning → simulasi jalan dgn lebar salah
            // (diferensial antar engine, kasus fuzzer bug_0793).
            // Kini ikuti rantai sampai field LEAF; bila chain tak resolve,
            // perilaku lama tetap sbg fallback.
            if let Some(f) = resolve_member_chain(obj, field, signal_map, signals) {
                return Ok(f.width.max(1));
            }
            compute_expr_width(obj, signal_map, signals, param_vals, package_symbols)
        }
        // Cast dengan width dari ekspresi: `size'(expr)` (mis. `$clog2(N)'(x)`)
        // — lebar cast = lebar ekspresi width-nya.
        Expr::CastWidth { width, .. } => {
            if let Ok(v) = const_eval_with_params(width, param_vals) {
                return Ok(v.unsigned_abs().max(1) as usize);
            }
            compute_expr_width(width, signal_map, signals, param_vals, package_symbols)
        }
        Expr::Cast { dtype, .. } => match crate::util::parse_type_spec_str(dtype.as_str()) {
            Some(dt) => match dt {
                DataType::UserDefined(name) => {
                    // 1) Parameter/konstanta: nilai = width (mis. `Width'(...)`).
                    if let Some(&v) = param_vals.get(&name) {
                        return Ok(v as usize);
                    }
                    // 2) Package param di-import (nama polos) — mis.
                    //    `MuBi4Width'(x)` dari `import prim_mubi_pkg::*`.
                    if let Some(v) = resolve_pkg_param_width(&name, package_symbols) {
                        return Ok(v as usize);
                    }
                    // 3) Typedef package (mis. `mubi4_t'(...)`) — resolve width
                    //    dari typedef di semua package.
                    Ok(resolve_dtype_width(&dt, package_symbols))
                }
                _ => Ok(dt.width()),
            },
            // dtype bukan base type — bisa jadi parameter/typedef package
            // (mis. `MuBi4Width'(x)`, `k'(x)`). Perlakukan sebagai identifier.
            None => {
                let name = Symbol::intern(dtype.as_str());
                if let Some(&v) = param_vals.get(&name) {
                    return Ok(v as usize);
                }
                if let Some(v) = resolve_pkg_param_width(&name, package_symbols) {
                    return Ok(v as usize);
                }
                Ok(resolve_dtype_width(
                    &DataType::UserDefined(name),
                    package_symbols,
                ))
            }
        },
        Expr::MethodCall { .. } | Expr::StreamingConcat { .. } | Expr::Dist { .. } => {
            Err("width not computable for this expression type".to_string())
        }
        Expr::ScopedIdent { package, item, .. } => {
            // Enum member / konstanta package yang di-flatten ke param_vals
            // sebagai qualified `pkg::item` (build_pkg_param_ctx). Contoh nyata
            // OpenTitan: `prim_mubi_pkg::MuBi4False` sebagai argumen port.
            let qualified = Symbol::intern(&format!("{}::{}", package.as_str(), item.as_str()));
            if let Some(&val) = param_vals.get(&qualified) {
                let abs = val.unsigned_abs();
                return Ok(if val == 0 {
                    1
                } else {
                    64 - (abs.leading_zeros() as usize)
                }
                .max(1));
            }
            // Package parameter dengan default: `pkg::PARAM`.
            if let Some(pkg_items) = package_symbols.get(package) {
                if let Some(PackageItem::Param(p)) = pkg_items.get(item) {
                    if let Some(expr) = &p.default {
                        if let Ok(val) = const_eval_with_params(expr, param_vals) {
                            let abs = val.unsigned_abs();
                            return Ok(if val == 0 {
                                1
                            } else {
                                64 - (abs.leading_zeros() as usize)
                            }
                            .max(1));
                        }
                    }
                }
                // Package typedef: `pkg::TypeName` di `$bits(pkg::TypeName)`
                // (mis. `$bits(prim_mubi_pkg::mubi4_t)`). Resolve lebar dari
                // typedef (enum base / struct fields / user-defined).
                if let Some(PackageItem::Typedef(td)) = pkg_items.get(item) {
                    let w = resolve_dtype_width(&td.dtype, package_symbols);
                    if w > 0 {
                        return Ok(w);
                    }
                }
            }
            Err(format!(
                "cannot determine width of '{}.{}' at compile time",
                package, item
            ))
        }
        _ => Err("cannot determine width of expression".to_string()),
    }
}

/// Evaluasi default localparam secara width-aware: untuk `$bits(expr)` /
/// `$size(expr)` nilai param = lebar expr, di-resolve dari sinyal yang sudah
/// terdaftar (port) via `compute_expr_width`. Dipakai saat const-eval skalar
/// gagal (mis. `localparam int NumInBufBits = $bits({a, b, c});`).
pub fn eval_width_aware_param(
    expr: &Expr,
    signal_map: &HashMap<Symbol, SignalId>,
    signals: &[SignalInfo],
    effective_params: &HashMap<Symbol, i64>,
    package_symbols: &HashMap<Symbol, HashMap<Symbol, PackageItem>>,
) -> Option<i64> {
    match expr {
        Expr::Value(Value::Decimal(n)) => Some(*n),
        Expr::Value(Value::Binary { bits, .. }) => {
            mivon_ast::const_eval::parse_literal(bits, 2).ok()
        }
        Expr::Value(Value::Hex { bits, .. }) => mivon_ast::const_eval::parse_literal(bits, 16).ok(),
        Expr::Value(Value::Octal { bits, .. }) => {
            mivon_ast::const_eval::parse_literal(bits, 8).ok()
        }
        Expr::String(s) => Some(mivon_ast::const_eval::string_to_i64(s)),
        Expr::Ident { name, .. } => effective_params.get(name).copied(),
        Expr::Paren(inner) => eval_width_aware_param(
            inner,
            signal_map,
            signals,
            effective_params,
            package_symbols,
        ),
        Expr::FuncCall { name, args, .. } if name == "$bits" || name == "$size" => {
            let arg = args.first()?;
            compute_expr_width(arg, signal_map, signals, effective_params, package_symbols)
                .ok()
                .map(|w| w as i64)
        }
        Expr::FuncCall { name, args, .. } if name == "$clog2" => {
            let arg = args.first()?;
            let v = eval_width_aware_param(
                arg,
                signal_map,
                signals,
                effective_params,
                package_symbols,
            )?;
            Some(clog2_value(v))
        }
        // $high/$left(sig) = lebar signal - 1; $low/$right(sig) = 0 (untuk
        // range [W-1:0] standar). Dipakai di part-select dinamis yang ternyata
        // konstanta, mis. `be_idx[$high(be_idx):1]` di dm_sba.
        Expr::FuncCall { name, args, .. } if name == "$high" || name == "$left" => {
            let arg = args.first()?;
            let w = compute_expr_width(arg, signal_map, signals, effective_params, package_symbols)
                .ok()?;
            Some((w as i64) - 1)
        }
        Expr::FuncCall { name, .. } if name == "$low" || name == "$right" => Some(0),
        // Fungsi package umum di localparam/generate (konsisten dengan
        // const_eval.rs): vbits, ceil_div, get_synd_width, is_width_valid,
        // bucket_ht_data_width, num_bucket_ht_inst.
        Expr::FuncCall { name, args, .. } if pkg_func_base(name.as_str()) == "vbits" => {
            let v = eval_width_aware_param(
                args.first()?,
                signal_map,
                signals,
                effective_params,
                package_symbols,
            )?;
            Some(if v == 1 { 1 } else { clog2_value(v) })
        }
        Expr::FuncCall { name, args, .. } if pkg_func_base(name.as_str()) == "ceil_div" => {
            let a = eval_width_aware_param(
                args.first()?,
                signal_map,
                signals,
                effective_params,
                package_symbols,
            )?;
            let b = eval_width_aware_param(
                args.get(1)?,
                signal_map,
                signals,
                effective_params,
                package_symbols,
            )?;
            if b == 0 {
                None
            } else {
                Some(if a % b != 0 { a / b + 1 } else { a / b })
            }
        }
        // prim_secded_pkg::get_synd_width(sd_type, width) — tabel konstanta
        // ECC (SecdedHsiao=0, SecdedHamming=1, SecdedInvHsiao=2,
        // SecdedInvHamming=3). Dipakai localparam `EccWidth` di otp_macro.
        Expr::FuncCall { name, args, .. } if pkg_func_base(name.as_str()) == "get_synd_width" => {
            let sd = eval_width_aware_param(
                args.first()?,
                signal_map,
                signals,
                effective_params,
                package_symbols,
            )?;
            let w = eval_width_aware_param(
                args.get(1)?,
                signal_map,
                signals,
                effective_params,
                package_symbols,
            )?;
            Some(match (sd, w) {
                (0, 16) | (2, 16) => 6,
                (0, 22) | (2, 22) => 6,
                (0, 32) | (2, 32) => 7,
                (0, 57) | (2, 57) => 7,
                (0, 64) | (2, 64) => 8,
                (1, 16) | (3, 16) => 6,
                (1, 32) | (3, 32) => 7,
                (1, 64) | (3, 64) => 8,
                (1, 68) | (3, 68) => 8,
                _ => 0,
            })
        }
        Expr::FuncCall { name, args, .. } if pkg_func_base(name.as_str()) == "is_width_valid" => {
            let sd = eval_width_aware_param(
                args.first()?,
                signal_map,
                signals,
                effective_params,
                package_symbols,
            )?;
            let w = eval_width_aware_param(
                args.get(1)?,
                signal_map,
                signals,
                effective_params,
                package_symbols,
            )?;
            Some(match (sd, w) {
                (0, 16)
                | (0, 22)
                | (0, 32)
                | (0, 57)
                | (0, 64)
                | (2, 16)
                | (2, 22)
                | (2, 32)
                | (2, 57)
                | (2, 64)
                | (1, 16)
                | (1, 32)
                | (1, 64)
                | (1, 68)
                | (3, 16)
                | (3, 32)
                | (3, 64)
                | (3, 68) => 1,
                _ => 0,
            })
        }
        Expr::FuncCall { name, args, .. }
            if pkg_func_base(name.as_str()) == "bucket_ht_data_width" =>
        {
            let w = eval_width_aware_param(
                args.first()?,
                signal_map,
                signals,
                effective_params,
                package_symbols,
            )?;
            Some(if w >= 4 { 4 } else { w })
        }
        Expr::FuncCall { name, args, .. }
            if pkg_func_base(name.as_str()) == "num_bucket_ht_inst" =>
        {
            let w = eval_width_aware_param(
                args.first()?,
                signal_map,
                signals,
                effective_params,
                package_symbols,
            )?;
            let b = if w >= 4 { 4 } else { w };
            if b == 0 {
                None
            } else {
                Some(if w % b != 0 { w / b + 1 } else { w / b })
            }
        }
        // OpenTitan otbn_pkg::SecAddRandWidth(w) = 2 * ($clog2(w) * w + 1) —
        // lebar randomness untuk otbn_sec_add / otbn_mask_accelerator
        // (localparam `RandWidth = SecAddRandWidth(Width)`).
        Expr::FuncCall { name, args, .. } if pkg_func_base(name.as_str()) == "SecAddRandWidth" => {
            let w = eval_width_aware_param(
                args.first()?,
                signal_map,
                signals,
                effective_params,
                package_symbols,
            )?;
            let clog = if w <= 1 {
                1
            } else {
                63 - (w as u64).leading_zeros() as i64
            };
            Some(2 * (clog * w + 1))
        }
        Expr::UnaryOp {
            op: UnaryOp::Minus,
            expr: inner,
        } => eval_width_aware_param(
            inner,
            signal_map,
            signals,
            effective_params,
            package_symbols,
        )
        .map(|v| -v),
        Expr::UnaryOp {
            op: UnaryOp::BitNot,
            expr: inner,
        } => eval_width_aware_param(
            inner,
            signal_map,
            signals,
            effective_params,
            package_symbols,
        )
        .map(|v| !v),
        Expr::UnaryOp {
            op: UnaryOp::Not,
            expr: inner,
        } => eval_width_aware_param(
            inner,
            signal_map,
            signals,
            effective_params,
            package_symbols,
        )
        .map(|v| if v == 0 { 1 } else { 0 }),
        Expr::BinaryOp { op, lhs, rhs } => {
            let l = eval_width_aware_param(
                lhs,
                signal_map,
                signals,
                effective_params,
                package_symbols,
            )?;
            let r = eval_width_aware_param(
                rhs,
                signal_map,
                signals,
                effective_params,
                package_symbols,
            )?;
            match op {
                BinaryOp::Add => Some(l.wrapping_add(r)),
                BinaryOp::Sub => Some(l.wrapping_sub(r)),
                BinaryOp::Mul => Some(l.wrapping_mul(r)),
                BinaryOp::Div => {
                    if r == 0 {
                        None
                    } else {
                        Some(l.wrapping_div(r))
                    }
                }
                BinaryOp::Mod => {
                    if r == 0 {
                        None
                    } else {
                        Some(l.wrapping_rem(r))
                    }
                }
                BinaryOp::Power => Some(l.pow(r.clamp(0, 31) as u32)),
                BinaryOp::BitAnd => Some(l & r),
                BinaryOp::BitOr => Some(l | r),
                BinaryOp::BitXor => Some(l ^ r),
                BinaryOp::BitXnor => Some(!(l ^ r)),
                BinaryOp::Shl => Some(l),
                BinaryOp::Shr => Some(l),
                BinaryOp::Sshl => Some(l),
                BinaryOp::Sshr => Some(l),
                BinaryOp::Eq => Some(if l == r { 1 } else { 0 }),
                BinaryOp::Neq => Some(if l != r { 1 } else { 0 }),
                BinaryOp::Lt => Some(if l < r { 1 } else { 0 }),
                BinaryOp::Le => Some(if l <= r { 1 } else { 0 }),
                BinaryOp::Gt => Some(if l > r { 1 } else { 0 }),
                BinaryOp::Ge => Some(if l >= r { 1 } else { 0 }),
                BinaryOp::LogicalAnd => Some(if l != 0 && r != 0 { 1 } else { 0 }),
                BinaryOp::LogicalOr => Some(if l != 0 || r != 0 { 1 } else { 0 }),
                BinaryOp::CaseEq | BinaryOp::CaseNeq | BinaryOp::EqWild | BinaryOp::NeqWild => {
                    Some(if l == r { 1 } else { 0 })
                }
            }
        }
        Expr::TernaryOp {
            cond,
            true_expr,
            false_expr,
        } => {
            let c = eval_width_aware_param(
                cond,
                signal_map,
                signals,
                effective_params,
                package_symbols,
            )?;
            if c != 0 {
                eval_width_aware_param(
                    true_expr,
                    signal_map,
                    signals,
                    effective_params,
                    package_symbols,
                )
            } else {
                eval_width_aware_param(
                    false_expr,
                    signal_map,
                    signals,
                    effective_params,
                    package_symbols,
                )
            }
        }
        Expr::Cast { expr: inner, .. } => eval_width_aware_param(
            inner,
            signal_map,
            signals,
            effective_params,
            package_symbols,
        ),
        // CastWidth `W'(expr)` — NILAI cast adalah nilai expr (truncate ke W bit
        // jarang mengubah nilai konstanta kecil). Konsisten dengan const_eval.rs
        // yang mengembalikan inner; sebelumnya salah mengembalikan LEBAR W.
        Expr::CastWidth { expr: inner, .. } => eval_width_aware_param(
            inner,
            signal_map,
            signals,
            effective_params,
            package_symbols,
        ),
        // Replikasi `{N{expr}}` — nilai = pola diulang N kali. Untuk pola
        // 1-bit (0/1) hasilnya mask of N ones / 0 (pola umum `{W{1'b1}}`
        // untuk mask FullRegMask). Untuk pola lebih lebar: ulangi bit pattern
        // N kali dengan lebar pola = lebar ekspresi.
        Expr::Replicate { count, expr } => {
            let n = eval_width_aware_param(
                count,
                signal_map,
                signals,
                effective_params,
                package_symbols,
            )?;
            let v = eval_width_aware_param(
                expr,
                signal_map,
                signals,
                effective_params,
                package_symbols,
            )?;
            let w =
                compute_expr_width(expr, signal_map, signals, effective_params, package_symbols)
                    .unwrap_or(1)
                    .max(1);
            let n = n.clamp(0, 63) as u32;
            if v == 0 {
                Some(0)
            } else if v == 1 && w == 1 {
                Some((1u64.wrapping_shl(n)).wrapping_sub(1) as i64)
            } else if n == 0 {
                Some(0)
            } else {
                // Ulangi bit-pattern v (lebar w) sebanyak n kali.
                let w = w.min(63) as u32;
                let pattern = (v as u64) & ((1u64 << w).wrapping_sub(1));
                let mut acc: u64 = 0;
                let mut total_w: u32 = 0;
                for _ in 0..n {
                    acc = (acc << w) | pattern;
                    total_w = total_w.saturating_add(w);
                    if total_w >= 63 {
                        break;
                    }
                }
                Some(acc as i64)
            }
        }
        // Concat `{a, b, c}` — elemen MSB→LSB; nilai = gabungan bit tiap
        // elemen dengan lebar elemen (width-aware; fallback 32 untuk ident).
        // Contoh `{4'h0, DataCount}` → DataCount << 0 (nilai tetap benar
        // selama elemen MSB bernilai 0 atau lebar elemen benar).
        Expr::Concat(elems) => {
            let mut acc: u64 = 0;
            let mut shift: u32 = 0;
            for elem in elems.iter().rev() {
                let v = eval_width_aware_param(
                    elem,
                    signal_map,
                    signals,
                    effective_params,
                    package_symbols,
                )?;
                let w = compute_expr_width(
                    elem,
                    signal_map,
                    signals,
                    effective_params,
                    package_symbols,
                )
                .unwrap_or(32)
                .clamp(1, 63) as u32;
                let masked = (v as u64) & ((1u64 << w).wrapping_sub(1));
                acc |= masked.wrapping_shl(shift.min(63));
                shift = shift.saturating_add(w);
                if shift >= 63 {
                    break;
                }
            }
            Some(acc as i64)
        }
        _ => None,
    }
}

/// Ambil nama fungsi tanpa prefix package (`prim_util_pkg::vbits` → `vbits`).
/// Konsisten dengan `base_func_name` di const_eval.rs (private di sana).
fn pkg_func_base(name: &str) -> &str {
    name.rsplit("::").next().unwrap_or(name)
}

fn clog2_value(v: i64) -> i64 {
    if v <= 1 {
        0
    } else {
        let n = v as u64;
        let msb = (64 - n.leading_zeros()) as i64;
        if n.is_power_of_two() {
            msb - 1
        } else {
            msb
        }
    }
}

/// Hitung lebar DataType dengan resolve typedef package / enum base.
/// Dipakai untuk return type function (`mubi4_t`) dan cast ke typedef
/// (`mubi4_t'(...)`). UserDefined di-resolve lewat `package_symbols`
/// (bukan default 64), EnumType via base-nya.
/// Cari parameter package dengan nama polos (hasil `import pkg::*`) dan
/// evaluasi default-nya. Dipakai untuk cast `MuBi4Width'(...)` di mana
/// `MuBi4Width` adalah `parameter` di package, bukan typedef.
/// Ikuti rantai member access dari akar `obj` sampai `field` terakhir dan
/// kembalikan `StructFieldInfo` field leaf bila resolve penuh berhasil.
///
/// `sub_fields` tiap field (termasuk field bertipe typedef) diisi oleh
/// `Elaborator::struct_field_from_member_depth` (elaborator/types.rs) —
/// free-fn ini tidak punya akses `typedef_field_map`, jadi rantai nested
/// `a.b.c.d` hanya bisa dituruni lewat `sub_fields` yang sudah terisi.
fn resolve_member_chain<'a>(
    obj: &Expr,
    field: &Symbol,
    signal_map: &HashMap<Symbol, SignalId>,
    signals: &'a [SignalInfo],
) -> Option<&'a StructFieldInfo> {
    match obj {
        Expr::Ident { name, .. } => {
            let sig_id = *signal_map.get(name)?;
            signals[sig_id]
                .struct_fields
                .iter()
                .find(|f| f.name == *field)
        }
        Expr::MemberAccess {
            obj: inner,
            field: parent_field,
        } => {
            let parent = resolve_member_chain(inner, parent_field, signal_map, signals)?;
            parent.sub_fields.iter().find(|f| f.name == *field)
        }
        _ => None,
    }
}

fn resolve_pkg_param_width(
    name: &Symbol,
    package_symbols: &HashMap<Symbol, HashMap<Symbol, PackageItem>>,
) -> Option<i64> {
    for items in package_symbols.values() {
        if let Some(PackageItem::Param(p)) = items.get(name) {
            if let Some(expr) = &p.default {
                if let Ok(v) = const_eval_with_params(expr, &HashMap::new()) {
                    return Some(v);
                }
            }
        }
    }
    None
}

/// Cari typedef package dengan nama polos (hasil `import pkg::*`) dan kembalikan
/// lebarnya. Dipakai untuk `$bits(typedef_name)` di constant/width context.
/// Prioritas: package asli dulu, lalu pseudo-package `__local_typedefs::<mod>`
/// (typedef lokal module yang didaftarkan elaborator per-module) sebagai
/// fallback — sehingga `$bits(typedef_lokal_module)` (mis. struct lokal di
/// ibex_cs_registers / ibex_dummy_instr) ikut ter-resolve.
fn resolve_typedef_ident_width(
    name: &Symbol,
    package_symbols: &HashMap<Symbol, HashMap<Symbol, PackageItem>>,
) -> Option<usize> {
    let find = |items: &HashMap<Symbol, PackageItem>| -> Option<usize> {
        if let Some(PackageItem::Typedef(td)) = items.get(name) {
            let w = resolve_dtype_width(&td.dtype, package_symbols);
            if w > 0 {
                return Some(w);
            }
        }
        None
    };
    // Pass 1: package asli (nama tanpa awalan __local_typedefs::).
    for (pkg, items) in package_symbols {
        if !pkg.as_str().starts_with("__local_typedefs::") {
            if let Some(w) = find(items) {
                return Some(w);
            }
        }
    }
    // Pass 2: typedef lokal module (fallback).
    for (pkg, items) in package_symbols {
        if pkg.as_str().starts_with("__local_typedefs::") {
            if let Some(w) = find(items) {
                return Some(w);
            }
        }
    }
    None
}

fn resolve_dtype_width(
    dt: &DataType,
    package_symbols: &HashMap<Symbol, HashMap<Symbol, PackageItem>>,
) -> usize {
    match dt {
        DataType::UserDefined(name) => {
            for items in package_symbols.values() {
                if let Some(PackageItem::Typedef(td)) = items.get(name) {
                    return resolve_dtype_width(&td.dtype, package_symbols);
                }
            }
            64
        }
        DataType::EnumType { base, .. } => {
            if let Some(b) = base {
                resolve_dtype_width(b, package_symbols)
            } else {
                32
            }
        }
        _ => dt.width(),
    }
}
