//! ──────────────────────────────────────────────────────────────────────────────
//! Extended constant evaluation untuk parameter package.
//! Mendukung nilai skalar, array posisional `'{...}` (di-parse sebagai Concat),
//! dan inlining fungsi package (loop/if/return) untuk default parameter.
//! Hasil: map kualifikasi `pkg::name` → skalar / array elemen.
//! ──────────────────────────────────────────────────────────────────────────────

use std::collections::HashMap;

use crate::const_eval::{sized_width, string_to_i64};
use crate::expr::{BinaryOp, Expr, StructLitMember, UnaryOp, Value};
use crate::stmt::Stmt;
use crate::types::{FunctionDecl, PackageItem};
use mivon_core::intern::Symbol;

#[derive(Debug, Clone, PartialEq)]
pub enum CVal {
    Scalar(i64),
    Array(Vec<CVal>),
    /// Nilai struct (assignment pattern bernama). `name=None` untuk anggota
    /// posisional; `is_default=true` untuk `default: value` (fallback field
    /// yang tidak disebutkan). Dipakai agar `pkg::arr[i].field` bisa di-const
    /// eval untuk array-of-struct (mis. `otp_ctrl_part_pkg::PartInfo`).
    Struct(Vec<SField>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SField {
    pub name: Option<Symbol>,
    pub val: CVal,
    pub is_default: bool,
}

impl SField {
    pub fn named(name: Symbol, val: CVal) -> Self {
        SField {
            name: Some(name),
            val,
            is_default: false,
        }
    }
    pub fn default(val: CVal) -> Self {
        SField {
            name: None,
            val,
            is_default: true,
        }
    }
}

pub type Scalars = HashMap<Symbol, i64>;
pub type Arrays = HashMap<Symbol, Vec<i64>>;

pub struct PkgCtx<'a> {
    pub scalars: &'a Scalars,
    pub arrays: &'a Arrays,
    pub package_symbols: &'a HashMap<Symbol, HashMap<Symbol, PackageItem>>,
    /// Nilai struct localparam/parameter (assignment pattern) agar
    /// `name.field` bisa di-const-eval — mis. `localparam info_t Info = '{...};`
    /// lalu `localparam int W = Info.size;` (pola `Info.size` di
    /// prim_generic_flash_bank / otp_ctrl_part_buf OpenTitan).
    pub structs: &'a HashMap<Symbol, Vec<SField>>,
}

fn parse_base(s: &str, radix: u32) -> Result<CVal, String> {
    // Nilai ≥ 2^63 (konstanta kriptografi 64-bit OpenTitan) tidak muat di
    // i64 — parse sebagai u64 lalu wrap bit-preserving (lihat
    // const_eval::parse_literal).
    let cleaned = s.replace(['x', 'z'], "0").replace('_', "");
    u64::from_str_radix(&cleaned, radix)
        .map(|v| CVal::Scalar(v as i64))
        .map_err(|_| format!("bad {} literal", radix))
}

pub fn flatten(a: &[CVal]) -> Vec<i64> {
    let mut out = Vec::new();
    for v in a {
        match v {
            CVal::Scalar(s) => out.push(*s),
            CVal::Array(inner) => out.extend(flatten(inner)),
            // Struct tanpa layout: representasi nilai utuh tidak bisa di-pack
            // di sini (butuh lebar field dari typedef). Nilai 0 (tanpa
            // regresi — pola bernama sebelumnya di-discard jadi 0). Member
            // access tetap benar lewat key `name[i].field` terdaftar.
            CVal::Struct(_) => out.push(0),
        }
    }
    out
}

pub fn scalar(v: CVal) -> Result<i64, String> {
    match v {
        CVal::Scalar(s) => Ok(s),
        CVal::Array(_) => Err("expected scalar constant".to_string()),
        CVal::Struct(_) => Err("expected scalar constant (got struct)".to_string()),
    }
}

fn apply_bin(op: BinaryOp, l: i64, r: i64) -> Result<i64, String> {
    Ok(match op {
        BinaryOp::Add => l.wrapping_add(r),
        BinaryOp::Sub => l.wrapping_sub(r),
        BinaryOp::Mul => l.wrapping_mul(r),
        BinaryOp::Div => {
            if r == 0 {
                return Err("division by zero".to_string());
            }
            l / r
        }
        BinaryOp::Mod => {
            if r == 0 {
                return Err("modulo by zero".to_string());
            }
            l % r
        }
        BinaryOp::Power => {
            // Modular biner mod 2^64 (dulu `l.pow(r as u32)`: truncation
            // eksponen + panic overflow saat debug).
            let mut b = l as u64;
            let mut acc: u64 = 1;
            let mut e = r as u64;
            while e > 0 {
                if e & 1 == 1 {
                    acc = acc.wrapping_mul(b);
                }
                e >>= 1;
                if e > 0 {
                    b = b.wrapping_mul(b);
                }
            }
            acc as i64
        }
        BinaryOp::Eq => (l == r) as i64,
        BinaryOp::Neq => (l != r) as i64,
        BinaryOp::Lt => (l < r) as i64,
        BinaryOp::Le => (l <= r) as i64,
        BinaryOp::Gt => (l > r) as i64,
        BinaryOp::Ge => (l >= r) as i64,
        BinaryOp::LogicalAnd => (l != 0 && r != 0) as i64,
        BinaryOp::LogicalOr => (l != 0 || r != 0) as i64,
        BinaryOp::BitAnd => l & r,
        BinaryOp::BitOr => l | r,
        BinaryOp::BitXor => l ^ r,
        BinaryOp::BitXnor => !(l ^ r),
        BinaryOp::Shl | BinaryOp::Sshl => l.wrapping_shl((r & 63) as u32),
        BinaryOp::Shr => l.wrapping_shr((r & 63) as u32),
        BinaryOp::Sshr => l >> (r & 63),
        _ => return Err(format!("unsupported const binary op {:?}", op)),
    })
}

fn clog2(v: i64) -> i64 {
    if v <= 1 {
        return 0;
    }
    let n = v as u64;
    let msb = (64 - n.leading_zeros()) as i64;
    if n.is_power_of_two() {
        msb - 1
    } else {
        msb
    }
}

/// Evaluasi ekspresi konstanta. `cur_pkg` menandai package tempat expr berada
/// sehingga referensi plain-name di-resolve ke `pkg::name` dulu.
pub fn eval_expr(e: &Expr, ctx: &PkgCtx, cur_pkg: Option<&str>) -> Result<CVal, String> {
    match e {
        Expr::Value(Value::Decimal(n)) => Ok(CVal::Scalar(*n)),
        Expr::Value(Value::Binary { bits, .. }) => parse_base(bits, 2),
        Expr::Value(Value::Hex { bits, .. }) => parse_base(bits, 16),
        Expr::Value(Value::Octal { bits, .. }) => parse_base(bits, 8),
        Expr::String(s) => Ok(CVal::Scalar(string_to_i64(s))),
        Expr::FillLit(_) | Expr::Null => Ok(CVal::Scalar(0)),
        Expr::Ident { name, .. } => {
            if let Some(pkg) = cur_pkg {
                let q = Symbol::intern(&format!("{}::{}", pkg, name.as_str()));
                if let Some(&v) = ctx.scalars.get(&q) {
                    return Ok(CVal::Scalar(v));
                }
                if let Some(a) = ctx.arrays.get(&q) {
                    return Ok(CVal::Array(a.iter().map(|&x| CVal::Scalar(x)).collect()));
                }
                if let Some(s) = ctx.structs.get(&q) {
                    return Ok(CVal::Struct(s.clone()));
                }
            }
            if let Some(&v) = ctx.scalars.get(name) {
                Ok(CVal::Scalar(v))
            } else if let Some(a) = ctx.arrays.get(name) {
                Ok(CVal::Array(a.iter().map(|&x| CVal::Scalar(x)).collect()))
            } else if let Some(s) = ctx.structs.get(name) {
                Ok(CVal::Struct(s.clone()))
            } else if name.as_str() == "1" {
                Ok(CVal::Scalar(1))
            } else {
                Err(format!("'{}' not found", name.as_str()))
            }
        }
        Expr::ScopedIdent { package, item, .. } => {
            let qname = Symbol::intern(&format!("{}::{}", package.as_str(), item.as_str()));
            if let Some(&v) = ctx.scalars.get(&qname) {
                Ok(CVal::Scalar(v))
            } else if let Some(a) = ctx.arrays.get(&qname) {
                Ok(CVal::Array(a.iter().map(|&x| CVal::Scalar(x)).collect()))
            } else if let Some(s) = ctx.structs.get(&qname) {
                Ok(CVal::Struct(s.clone()))
            } else {
                Err(format!("'{}' not found", qname.as_str()))
            }
        }
        Expr::Paren(inner) => eval_expr(inner, ctx, cur_pkg),
        Expr::UnaryOp { op, expr } => {
            let v = scalar(eval_expr(expr, ctx, cur_pkg)?)?;
            Ok(CVal::Scalar(match op {
                UnaryOp::Minus => v.wrapping_neg(),
                UnaryOp::Plus => v,
                UnaryOp::BitNot => {
                    // SV §11.8.1: `~` context-determined — evaluasi pada
                    // lebar ≥ 32, penugasan yang memotong (ekivalen untuk
                    // semua konteks; konsisten dgn const_eval_with_params).
                    match sized_width(expr) {
                        Some(w) if w > 0 && w < 64 => {
                            let ew = w.max(32);
                            let mask = (1u64 << ew) - 1;
                            (!(v as u64) & mask) as i64
                        }
                        _ => !v,
                    }
                }
                UnaryOp::Not => (v == 0) as i64,
                UnaryOp::ReductionAnd => (v == -1) as i64,
                UnaryOp::ReductionNand => (v != -1) as i64,
                UnaryOp::ReductionOr => (v != 0) as i64,
                UnaryOp::ReductionNor => (v == 0) as i64,
                UnaryOp::ReductionXor | UnaryOp::ReductionXnor => (v.count_ones() % 2) as i64,
            }))
        }
        Expr::BinaryOp { op, lhs, rhs } => {
            let l = scalar(eval_expr(lhs, ctx, cur_pkg)?)?;
            let r = scalar(eval_expr(rhs, ctx, cur_pkg)?)?;
            Ok(CVal::Scalar(apply_bin(op.clone(), l, r)?))
        }
        Expr::TernaryOp {
            cond,
            true_expr,
            false_expr,
        } => {
            let c = scalar(eval_expr(cond, ctx, cur_pkg)?)?;
            if c != 0 {
                eval_expr(true_expr, ctx, cur_pkg)
            } else {
                eval_expr(false_expr, ctx, cur_pkg)
            }
        }
        Expr::BitSelect { expr, index } => {
            let idx = scalar(eval_expr(index, ctx, cur_pkg)?)?;
            match eval_expr(expr, ctx, cur_pkg)? {
                CVal::Scalar(v) => Ok(CVal::Scalar(if (0..64).contains(&idx) {
                    (v >> idx) & 1
                } else {
                    0
                })),
                CVal::Array(a) => {
                    if idx >= 0 && (idx as usize) < a.len() {
                        Ok(a[idx as usize].clone())
                    } else {
                        Err("array index out of range".to_string())
                    }
                }
                CVal::Struct(_) => Err("bit select on struct constant".to_string()),
            }
        }
        Expr::RangeSelect { expr, msb, lsb } => {
            let m = scalar(eval_expr(msb, ctx, cur_pkg)?)?;
            let l = scalar(eval_expr(lsb, ctx, cur_pkg)?)?;
            match eval_expr(expr, ctx, cur_pkg)? {
                CVal::Array(a) => {
                    if l >= 0 && m >= l && (m as usize) < a.len() {
                        Ok(CVal::Array(a[(l as usize)..=(m as usize)].to_vec()))
                    } else {
                        Err("array range out of bounds".to_string())
                    }
                }
                CVal::Scalar(v) => {
                    let width = (m - l + 1) as usize;
                    if width >= 64 {
                        Ok(CVal::Scalar(v >> l))
                    } else {
                        Ok(CVal::Scalar((v >> l) & (((1u64 << width) - 1) as i64)))
                    }
                }
                CVal::Struct(_) => Err("range select on struct constant".to_string()),
            }
        }
        // Indexed part-select `[base +: width]` (sama dengan const_eval.rs —
        // arah `+:` diasumsikan). Mendukung skalar & array.
        Expr::PartSelect { expr, base, width } => {
            let b = scalar(eval_expr(base, ctx, cur_pkg)?)?;
            let w = scalar(eval_expr(width, ctx, cur_pkg)?)?;
            if w <= 0 {
                return Err("part-select width must be positive".to_string());
            }
            match eval_expr(expr, ctx, cur_pkg)? {
                CVal::Scalar(v) => {
                    let width = w as usize;
                    if !(0..64).contains(&b) {
                        return Ok(CVal::Scalar(0));
                    }
                    if width >= 64 {
                        Ok(CVal::Scalar(v >> b))
                    } else {
                        Ok(CVal::Scalar((v >> b) & (((1u64 << width) - 1) as i64)))
                    }
                }
                CVal::Array(a) => {
                    if b >= 0 && (b as usize + w as usize) <= a.len() {
                        Ok(CVal::Array(
                            a[(b as usize)..(b as usize + w as usize)].to_vec(),
                        ))
                    } else {
                        Err("part-select out of bounds".to_string())
                    }
                }
                CVal::Struct(_) => Err("part select on struct constant".to_string()),
            }
        }
        Expr::Concat(items) => {
            let mut out = Vec::new();
            for it in items {
                out.push(eval_expr(it, ctx, cur_pkg)?);
            }
            Ok(CVal::Array(out))
        }
        Expr::Replicate { count, expr } => {
            let c = scalar(eval_expr(count, ctx, cur_pkg)?)?;
            if !(0..=1_000_000).contains(&c) {
                return Err("bad replication count".to_string());
            }
            let inner = eval_expr(expr, ctx, cur_pkg)?;
            Ok(CVal::Array(vec![inner; c as usize]))
        }
        Expr::Cast { expr, .. } => eval_expr(expr, ctx, cur_pkg),
        Expr::CastWidth { expr, .. } => eval_expr(expr, ctx, cur_pkg),
        Expr::FuncCall { name, args, .. } => eval_func(name.as_str(), args, ctx, cur_pkg),
        // Struct assignment pattern `'{name: v, default: d, ...}` — evaluasi
        // setiap member jadi CVal::Struct agar `arr[i].field` bisa diekstrak.
        Expr::StructLit { members } => {
            let mut fields = Vec::new();
            for m in members {
                match m {
                    StructLitMember::Named(name, e) => {
                        fields.push(SField::named(*name, eval_expr(e, ctx, cur_pkg)?));
                    }
                    StructLitMember::Positional(e) => fields.push(SField {
                        name: None,
                        val: eval_expr(e, ctx, cur_pkg)?,
                        is_default: false,
                    }),
                    StructLitMember::Default(e) => {
                        fields.push(SField::default(eval_expr(e, ctx, cur_pkg)?));
                    }
                }
            }
            Ok(CVal::Struct(fields))
        }
        Expr::MemberAccess { obj, field } => match eval_expr(obj, ctx, cur_pkg)? {
            CVal::Struct(fields) => {
                for f in &fields {
                    if f.name
                        .as_ref()
                        .map(|n| n.as_str() == field.as_str())
                        .unwrap_or(false)
                    {
                        return Ok(f.val.clone());
                    }
                }
                // Fallback `default: value` untuk field yang tidak disebutkan.
                for f in &fields {
                    if f.is_default {
                        return Ok(f.val.clone());
                    }
                }
                Err(format!(
                    "field '{}' not found in struct constant",
                    field.as_str()
                ))
            }
            _ => Err("member access on non-struct constant".to_string()),
        },
        _ => Err("unsupported const expr".to_string()),
    }
}

fn eval_func(
    name: &str,
    args: &[Expr],
    ctx: &PkgCtx,
    cur_pkg: Option<&str>,
) -> Result<CVal, String> {
    let first_arg = |i: usize| -> Result<i64, String> {
        args.get(i)
            .ok_or_else(|| format!("{} needs {} args", name, i + 1))
            .and_then(|a| eval_expr(a, ctx, cur_pkg).and_then(scalar))
    };
    match name {
        "$clog2" => Ok(CVal::Scalar(clog2(first_arg(0)?))),
        "$bits" => {
            let Some(arg) = args.first() else {
                return Err("$bits needs 1 arg".to_string());
            };
            // Prioritas: konstanta biasa (skalar/array) → evaluasi ekspresi.
            if let Ok(v) = eval_expr(arg, ctx, cur_pkg) {
                return Ok(v);
            }
            // `$bits(typedef)` — argumen adalah nama tipe (ident atau
            // `pkg::type`). Hitung lebar typedef dari package_symbols,
            // termasuk nested struct/typedef dan range eksplisit.
            let type_name: Option<(Option<&str>, &str)> = match arg {
                Expr::Ident { name, .. } => Some((None, name.as_str())),
                Expr::ScopedIdent { package, item, .. } => {
                    Some((Some(package.as_str()), item.as_str()))
                }
                _ => None,
            };
            if let Some((pkg_opt, type_name)) = type_name {
                if let Some(w) = resolve_typedef_bits(ctx, pkg_opt, type_name, 0) {
                    return Ok(CVal::Scalar(w as i64));
                }
            }
            Err("'$bits' argument cannot be evaluated (not a constant or known type)".to_string())
        }
        "$size" | "$left" | "$right" | "$low" | "$high" => args
            .first()
            .ok_or_else(|| format!("{} needs 1 arg", name))
            .and_then(|a| eval_expr(a, ctx, cur_pkg)),
        "vbits" => {
            let v = first_arg(0)?;
            Ok(CVal::Scalar(if v == 1 { 1 } else { clog2(v) }))
        }
        "ceil_div" => {
            let a = first_arg(0)?;
            let b = first_arg(1)?;
            if b == 0 {
                return Err("division by zero in ceil_div".to_string());
            }
            Ok(CVal::Scalar(if a % b != 0 { a / b + 1 } else { a / b }))
        }
        _ => {
            let Some((_pkg, func)) = find_package_func(ctx.package_symbols, name) else {
                return Err(format!("function '{}' not found", name));
            };
            eval_function_body(func, args, ctx, cur_pkg)
        }
    }
}

/// Hitung lebar (dalam bit) sebuah typedef package, dengan rekursi untuk
/// nested `UserDefined` (typedef merujuk typedef lain) dan anonymous struct.
/// `pkg_opt`: package wajib untuk nama scoped; None = cari di semua package.
/// Guard depth mencegah rekursi tak hingga (typedef sirkular).
fn resolve_typedef_bits(
    ctx: &PkgCtx,
    pkg_opt: Option<&str>,
    type_name: &str,
    depth: usize,
) -> Option<usize> {
    if depth > 32 {
        return None;
    }
    let package_symbols = ctx.package_symbols;
    // Cari typedef: scoped (pkg::type) atau plain di semua package.
    // Package disimpan sebagai String milik sendiri agar bebas dari lifetime
    // borrow pkg_opt / iterator.
    let (td, typedef_pkg): (&crate::types::TypedefDecl, String) = if let Some(pkg) = pkg_opt {
        {
            let td = package_symbols
                .get(&Symbol::intern(pkg))
                .and_then(|items| lookup_pkg_typedef(items, type_name))?;
            (td, pkg.to_string())
        }
    } else {
        match package_symbols.iter().find_map(|(pkg, items)| {
            lookup_pkg_typedef(items, type_name).map(|td| (td, pkg.as_str().to_string()))
        }) {
            Some((td, pkg)) => (td, pkg),
            None => return None,
        }
    };
    // Range eksplisit `typedef logic [W-1:0] name;` menang atas width default.
    // eval_expr memakai `cur_pkg` = package typedef AGAR batas range yang
    // mereferensikan parameter package plain-name (`logic [SramKeyWidth-1:0]`
    // di otp_ctrl_pkg) ter-resolve ke `pkg::SramKeyWidth` dari ctx.scalars.
    // Tanpa ini `$bits(otp_ctrl_pkg::sram_key_t)` (alias logic [..]) jatuh ke
    // `dtype.width()=1` → TotalAnchorWidth = $bits(..)+$bits(..) = 2 (harusnya
    // 256) → lebar instance/port salah (E3012 di desain besar).
    if let Some(er) = &td.range {
        let pkg_ctx = Some(typedef_pkg.as_str());
        if let (Ok(msb), Ok(lsb)) = (
            eval_expr(&er.msb, ctx, pkg_ctx).and_then(scalar),
            eval_expr(&er.lsb, ctx, pkg_ctx).and_then(scalar),
        ) {
            if msb >= 0 && lsb >= 0 {
                return Some(msb.abs_diff(lsb) as usize + 1);
            }
        }
    }
    Some(typedef_dtype_bits(
        ctx,
        &td.dtype,
        Some(typedef_pkg.as_str()),
        depth,
    ))
}

/// Lebar bit dari DataType dengan resolve `UserDefined` ke typedef lain.
/// Range member struct dievaluasi via `eval_expr` dengan `cur_pkg` = package
/// typedef agar parameter package (mis. `logic [KeyLen-1:0] key;` di mana
/// KeyLen = `pkg::KeyLen`) ter-resolve dari ctx.scalars.
fn typedef_dtype_bits(
    ctx: &PkgCtx,
    dtype: &crate::types::DataType,
    typedef_pkg: Option<&str>,
    depth: usize,
) -> usize {
    use crate::types::DataType;
    let member_width = |m: &crate::types::StructMember| -> usize {
        // StructMember.range sudah `Range` (msb/lsb usize ter-resolve saat
        // parse); pakai langsung. Kalau belum ter-resolve (range memakai
        // parameter package, mis. `logic [KeyLen-1:0] key`), evaluasi
        // expr_range dengan konteks package typedef.
        if let Some(r) = &m.range {
            return r.width();
        }
        if let Some(er) = &m.expr_range {
            if let (Ok(msb), Ok(lsb)) = (
                eval_expr(&er.msb, ctx, typedef_pkg).and_then(scalar),
                eval_expr(&er.lsb, ctx, typedef_pkg).and_then(scalar),
            ) {
                if msb >= 0 && lsb >= 0 {
                    return msb.abs_diff(lsb) as usize + 1;
                }
            }
        }
        typedef_dtype_bits(ctx, &m.dtype, typedef_pkg, depth + 1)
    };
    match dtype {
        DataType::UserDefined(name) => {
            // Coba resolve sebagai typedef package (plain-name, semua package).
            resolve_typedef_bits(ctx, None, name.as_str(), depth + 1).unwrap_or(64)
        }
        DataType::Signed(inner) => typedef_dtype_bits(ctx, inner, typedef_pkg, depth),
        DataType::StructType { members } => members.iter().map(&member_width).sum(),
        DataType::UnionType { members } => members.iter().map(member_width).max().unwrap_or(1),
        _ => dtype.width(),
    }
}

/// Ambil `TypedefDecl` dari peta item package berdasarkan nama.
fn lookup_pkg_typedef<'a>(
    pkg_items: &'a HashMap<Symbol, PackageItem>,
    name: &str,
) -> Option<&'a crate::types::TypedefDecl> {
    pkg_items.get(&Symbol::intern(name)).and_then(|item| {
        if let PackageItem::Typedef(td) = item {
            Some(td)
        } else {
            None
        }
    })
}

fn find_package_func<'a>(
    package_symbols: &'a HashMap<Symbol, HashMap<Symbol, PackageItem>>,
    name: &'a str,
) -> Option<(&'a str, &'a FunctionDecl)> {
    if let Some((pkg, func)) = name.split_once("::") {
        let func = package_symbols
            .get(&Symbol::intern(pkg))
            .and_then(|items| items.get(&Symbol::intern(func)))
            .and_then(|item| {
                if let PackageItem::Function(f) = item {
                    Some(f)
                } else {
                    None
                }
            })?;
        return Some((pkg, func));
    }
    for (pkg, items) in package_symbols {
        if let Some(PackageItem::Function(f)) = items.get(&Symbol::intern(name)) {
            return Some((pkg.as_str(), f));
        }
    }
    None
}

thread_local! {
    static FUNC_EVAL_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn eval_function_body(
    func: &FunctionDecl,
    args: &[Expr],
    ctx: &PkgCtx,
    cur_pkg: Option<&str>,
) -> Result<CVal, String> {
    // Guard rekursi: fungsi package yang memanggil dirinya sendiri / saling
    // memanggil tanpa basis (mis. error resolusi di tubuh) tidak boleh
    // membuat stack overflow. Batas kedalaman inlining fungsi package.
    let depth = FUNC_EVAL_DEPTH.with(|c| c.get());
    if depth > 128 {
        return Err(format!(
            "package function recursion too deep in '{}'",
            func.name.as_str()
        ));
    }
    FUNC_EVAL_DEPTH.with(|c| c.set(depth + 1));
    let result = eval_function_body_inner(func, args, ctx, cur_pkg);
    FUNC_EVAL_DEPTH.with(|c| c.set(depth));
    result
}

fn eval_function_body_inner(
    func: &FunctionDecl,
    args: &[Expr],
    ctx: &PkgCtx,
    cur_pkg: Option<&str>,
) -> Result<CVal, String> {
    let mut local_scalars: Scalars = ctx.scalars.clone();
    let mut local_arrays: Arrays = ctx.arrays.clone();
    for (i, port) in func.ports.iter().enumerate() {
        let Some(arg) = args.get(i) else { break };
        match eval_expr(arg, ctx, cur_pkg)? {
            CVal::Scalar(v) => {
                local_scalars.insert(port.name, v);
            }
            CVal::Array(a) => {
                local_arrays.insert(port.name, flatten(&a));
            }
            CVal::Struct(_) => {}
        }
    }
    for decl in &func.decls {
        for var in &decl.names {
            if let Some(init) = &var.expr {
                let local_ctx = PkgCtx {
                    scalars: &local_scalars,
                    arrays: &local_arrays,
                    package_symbols: ctx.package_symbols,
                    structs: ctx.structs,
                };
                if let Ok(v) = eval_expr(init, &local_ctx, cur_pkg) {
                    match v {
                        CVal::Scalar(s) => {
                            local_scalars.insert(var.name, s);
                        }
                        CVal::Array(a) => {
                            local_arrays.insert(var.name, flatten(&a));
                        }
                        CVal::Struct(_) => {}
                    }
                }
            } else if var.array_range.is_some() {
                local_arrays.insert(var.name, Vec::new());
            }
        }
    }
    let mut stmts_scalars = local_scalars;
    let mut stmts_arrays = local_arrays;
    for stmt in &func.stmts {
        if let Some(v) = eval_stmt(
            stmt,
            &mut stmts_scalars,
            &mut stmts_arrays,
            ctx.package_symbols,
            cur_pkg,
        )? {
            return Ok(CVal::Scalar(v));
        }
    }
    Err(format!("function '{}' has no return", func.name.as_str()))
}

fn eval_stmt(
    stmt: &Stmt,
    scalars: &mut Scalars,
    arrays: &mut Arrays,
    package_symbols: &HashMap<Symbol, HashMap<Symbol, PackageItem>>,
    cur_pkg: Option<&str>,
) -> Result<Option<i64>, String> {
    macro_rules! eval_in_ctx {
        ($e:expr) => {{
            let pkg_ctx = PkgCtx {
                scalars: &*scalars,
                arrays: &*arrays,
                package_symbols,
                structs: no_structs(),
            };
            eval_expr($e, &pkg_ctx, cur_pkg)
        }};
    }
    match stmt {
        Stmt::Block { stmts } | Stmt::NamedBlock { stmts, .. } => {
            for s in stmts {
                if let Some(v) = eval_stmt(s, scalars, arrays, package_symbols, cur_pkg)? {
                    return Ok(Some(v));
                }
            }
            Ok(None)
        }
        Stmt::IfElse {
            cond,
            true_branch,
            false_branch,
        } => {
            let c = scalar(eval_in_ctx!(cond)?)?;
            if c != 0 {
                eval_stmt(true_branch, scalars, arrays, package_symbols, cur_pkg)
            } else if let Some(fb) = false_branch {
                eval_stmt(fb, scalars, arrays, package_symbols, cur_pkg)
            } else {
                Ok(None)
            }
        }
        Stmt::LoopFor {
            init,
            cond,
            step,
            stmts,
        } => {
            if let Some(init) = init {
                eval_stmt(init, scalars, arrays, package_symbols, cur_pkg)?;
            }
            let mut guard = 0usize;
            loop {
                if let Some(c) = cond {
                    if scalar(eval_in_ctx!(c)?)? == 0 {
                        break;
                    }
                }
                guard += 1;
                if guard > 1_000_000 {
                    return Err("const loop exceeded iteration limit".to_string());
                }
                for s in stmts {
                    if let Some(v) = eval_stmt(s, scalars, arrays, package_symbols, cur_pkg)? {
                        return Ok(Some(v));
                    }
                }
                if let Some(st) = step {
                    eval_stmt(st, scalars, arrays, package_symbols, cur_pkg)?;
                }
            }
            Ok(None)
        }
        Stmt::LoopWhile { cond, stmts } | Stmt::DoWhile { cond, stmts } => {
            let mut guard = 0usize;
            loop {
                if scalar(eval_in_ctx!(cond)?)? == 0 {
                    break Ok(None);
                }
                guard += 1;
                if guard > 1_000_000 {
                    return Err("const loop exceeded iteration limit".to_string());
                }
                for s in stmts {
                    if let Some(v) = eval_stmt(s, scalars, arrays, package_symbols, cur_pkg)? {
                        return Ok(Some(v));
                    }
                }
            }
        }
        Stmt::Repeat { count, stmts } => {
            let n = scalar(eval_in_ctx!(count)?)?;
            for _ in 0..n {
                for s in stmts {
                    if let Some(v) = eval_stmt(s, scalars, arrays, package_symbols, cur_pkg)? {
                        return Ok(Some(v));
                    }
                }
            }
            Ok(None)
        }
        Stmt::BlockingAssign { lhs, rhs, .. } => {
            let v = eval_in_ctx!(rhs)?;
            match lhs {
                Expr::Ident { name, .. } => match v {
                    CVal::Scalar(s) => {
                        scalars.insert(*name, s);
                    }
                    CVal::Array(a) => {
                        arrays.insert(*name, flatten(&a));
                    }
                    CVal::Struct(_) => {}
                },
                Expr::BitSelect { expr, index } => {
                    if let Expr::Ident { name, .. } = expr.as_ref() {
                        let idx = scalar(eval_in_ctx!(index.as_ref())?)?;
                        if let Some(arr) = arrays.get_mut(name) {
                            if idx >= 0 && (idx as usize) < arr.len() {
                                arr[idx as usize] = scalar(v)?;
                            }
                        }
                    }
                }
                _ => {}
            }
            Ok(None)
        }
        Stmt::Return(Some(e)) => {
            let v = eval_in_ctx!(e)?;
            Ok(Some(scalar(v)?))
        }
        Stmt::Return(None) => Ok(Some(0)),
        Stmt::Null => Ok(None),
        _ => Err("unsupported statement in const function".to_string()),
    }
}

/// Evaluasi semua parameter package (key kualifikasi `pkg::name`).
pub fn eval_package_constants(
    package_symbols: &HashMap<Symbol, HashMap<Symbol, PackageItem>>,
) -> (Scalars, Arrays) {
    let mut scalars: Scalars = HashMap::new();
    let mut arrays: Arrays = HashMap::new();
    // Nilai struct param package yang sudah dievaluasi (key qualified
    // `pkg::name`), agar param LAIN bisa mereferensikannya sebagai struct
    // (`TL_H2D_DEFAULT = '{... TL_A_USER_DEFAULT ...}`), bukan hanya member
    // key skalar `pkg::name.field` di scalars.
    let mut structs: HashMap<Symbol, Vec<SField>> = HashMap::new();
    for _ in 0..256 {
        let mut changed = false;
        for (pkg_name, items) in package_symbols {
            for (name, item) in items {
                let PackageItem::Param(p) = item else {
                    continue;
                };
                if p.is_type_param {
                    continue;
                }
                let Some(default) = &p.default else { continue };
                let qname = Symbol::intern(&format!("{}::{}", pkg_name.as_str(), name.as_str()));
                if scalars.contains_key(&qname)
                    || arrays.contains_key(&qname)
                    || structs.contains_key(&qname)
                {
                    continue;
                }
                let eval_debug = std::env::var("DBG_PKG_EVAL").is_ok();
                let eval_result = {
                    let ctx = PkgCtx {
                        scalars: &scalars,
                        arrays: &arrays,
                        package_symbols,
                        structs: &structs,
                    };
                    eval_expr(default, &ctx, Some(pkg_name.as_str()))
                };
                match eval_result {
                    Ok(CVal::Scalar(v)) => {
                        if eval_debug {
                            eprintln!("[DBG-PKG] {} = scalar {}", qname.as_str(), v);
                        }
                        scalars.insert(qname, v);
                        changed = true;
                    }
                    Ok(CVal::Array(a)) => {
                        if eval_debug {
                            eprintln!("[DBG-PKG] {} = array len {}", qname.as_str(), a.len());
                        }
                        arrays.insert(qname, flatten(&a));
                        // Struct member keys utk array-of-struct: `pkg::name[i].field`
                        // → nilai field skalar. Ini yang membuat `PartInfo[k].offset`
                        // bisa di-const-eval (member access di const_eval.rs cari
                        // key `base.field` di param_vals).
                        for (i, elem) in a.iter().enumerate() {
                            if let CVal::Struct(fields) = elem {
                                if eval_debug {
                                    eprintln!(
                                        "[DBG-PKG]   elem[{}] = struct len {}",
                                        i,
                                        fields.len()
                                    );
                                }
                                for f in fields {
                                    if let Some(fname) = f.name {
                                        if let CVal::Scalar(v) = &f.val {
                                            scalars.insert(
                                                Symbol::intern(&format!(
                                                    "{}::{}[{}].{}",
                                                    pkg_name.as_str(),
                                                    name.as_str(),
                                                    i,
                                                    fname.as_str()
                                                )),
                                                *v,
                                            );
                                        }
                                    }
                                }
                            } else {
                                if eval_debug {
                                    eprintln!("[DBG-PKG]   elem[{}] = NON-STRUCT {:?}", i, elem);
                                }
                            }
                        }
                        changed = true;
                    }
                    Ok(CVal::Struct(fields)) => {
                        if eval_debug {
                            eprintln!("[DBG-PKG] {} = struct len {}", qname.as_str(), fields.len());
                        }
                        // Simpan struct utk referensi silang param package lain.
                        structs.insert(qname, fields.clone());
                        // Single struct param: `pkg::name.field` → nilai field skalar.
                        for f in &fields {
                            if let Some(fname) = f.name {
                                if let CVal::Scalar(v) = &f.val {
                                    scalars.insert(
                                        Symbol::intern(&format!(
                                            "{}::{}.{}",
                                            pkg_name.as_str(),
                                            name.as_str(),
                                            fname.as_str()
                                        )),
                                        *v,
                                    );
                                }
                            }
                        }
                        changed = true;
                    }
                    Err(e) => {
                        if eval_debug {
                            eprintln!("[DBG-PKG] {} = ERROR: {}", qname.as_str(), e);
                        }
                    }
                }
            }
            // Enum member constants package → scalar (qualified + plain-by-context).
            // Ini membuat `import pkg::*` bisa memakai nama member enum (mis.
            // `NumTotalCmdInfo`) sebagai konstanta integer dalam ekspresi parameter.
            // GAGAL eval → lewati (jangan tebak `last`): warning + verifikasi
            // dilakukan terpusat di build_pkg_param_ctx::verify_enum_members.
            for item in items.values() {
                let PackageItem::Typedef(td) = item else {
                    continue;
                };
                let crate::types::DataType::EnumType { members, .. } = &td.dtype else {
                    continue;
                };
                let mut last: Option<i64> = Some(0);
                for (mname, mexpr) in members {
                    let val = match mexpr {
                        Some(e) => {
                            let ctx = PkgCtx {
                                scalars: &scalars,
                                arrays: &arrays,
                                package_symbols,
                                structs: no_structs(),
                            };
                            match eval_expr(e, &ctx, Some(pkg_name.as_str())) {
                                Ok(CVal::Scalar(v)) => Some(v),
                                _ => None,
                            }
                        }
                        None => last,
                    };
                    if let Some(v) = val {
                        let q = Symbol::intern(&format!("{}::{}", pkg_name.as_str(), mname.as_str()));
                        if let std::collections::hash_map::Entry::Vacant(e) = scalars.entry(q) {
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
    (scalars, arrays)
}

/// Flatten hasil evaluasi konstanta ke dalam ctx i64 untuk const_eval_with_params.
/// Skalar → `name`; array → `name[i]` untuk setiap elemen.
pub fn flatten_consts_into_ctx(scalars: &Scalars, arrays: &Arrays, ctx: &mut HashMap<Symbol, i64>) {
    for (name, v) in scalars {
        ctx.insert(*name, *v);
    }
    for (name, elems) in arrays {
        if let Some(&first) = elems.first() {
            ctx.insert(*name, first);
        }
        for (i, v) in elems.iter().enumerate() {
            ctx.insert(Symbol::intern(&format!("{}[{}]", name.as_str(), i)), *v);
        }
    }
}

/// Daftarkan nama plain (via import) untuk konstanta package dalam ctx.
/// Untuk skalar: `name` → nilai. Untuk array: `name[i]` → elemen.
pub fn flatten_imported_consts_into_ctx(
    pkg_name: &str,
    import_item: &str,
    scalars: &Scalars,
    arrays: &Arrays,
    ctx: &mut HashMap<Symbol, i64>,
) {
    let prefix = format!("{}::", pkg_name);
    if import_item == "*" {
        for (qname, v) in scalars {
            if let Some(rest) = qname.as_str().strip_prefix(&prefix) {
                ctx.entry(Symbol::intern(rest)).or_insert(*v);
                // Pertahankan juga bentuk kualifikasi (scoped access
                // `pkg::name[i].field` di ekspresi module).
                ctx.entry(*qname).or_insert(*v);
            }
        }
        for (qname, elems) in arrays {
            if let Some(rest) = qname.as_str().strip_prefix(&prefix) {
                for (i, v) in elems.iter().enumerate() {
                    ctx.insert(Symbol::intern(&format!("{}[{}]", rest, i)), *v);
                    ctx.insert(
                        Symbol::intern(&format!("{}::{}[{}]", pkg_name, rest, i)),
                        *v,
                    );
                }
                if let Some(&first) = elems.first() {
                    ctx.entry(Symbol::intern(rest)).or_insert(first);
                }
            }
        }
    } else {
        let qname = Symbol::intern(&format!("{}{}", prefix, import_item));
        if let Some(&v) = scalars.get(&qname) {
            ctx.entry(Symbol::intern(import_item)).or_insert(v);
        }
        if let Some(elems) = arrays.get(&qname) {
            for (i, v) in elems.iter().enumerate() {
                ctx.insert(Symbol::intern(&format!("{}[{}]", import_item, i)), *v);
            }
            if let Some(&first) = elems.first() {
                ctx.entry(Symbol::intern(import_item)).or_insert(first);
            }
        }
        // Member keys struct utk import item tunggal: `pkg::item[i].field` /
        // `pkg::item.field` → plain `item[i].field` / `item.field`.
        let mp = format!("{}::{}[", pkg_name, import_item);
        let mp2 = format!("{}::{}.", pkg_name, import_item);
        for (qname, v) in scalars {
            let s = qname.as_str();
            if s.starts_with(&mp) || s.starts_with(&mp2) {
                if let Some(rest) = s.strip_prefix(&prefix) {
                    ctx.entry(Symbol::intern(rest)).or_insert(*v);
                }
            }
        }
    }
}

/// Helper kecil: evaluasi default parameter body module (dipakai generate lokal).
pub fn eval_body_param_default(
    expr: &Expr,
    scalars: &Scalars,
    arrays: &Arrays,
    package_symbols: &HashMap<Symbol, HashMap<Symbol, PackageItem>>,
) -> Option<i64> {
    let ctx = PkgCtx {
        scalars,
        arrays,
        package_symbols,
        structs: no_structs(),
    };
    match eval_expr(expr, &ctx, None) {
        Ok(CVal::Scalar(v)) => Some(v),
        Ok(CVal::Array(a)) => flatten(&a).first().copied(),
        _ => None,
    }
}

/// Evaluasi penuh sebuah ekspresi konstanta → `CVal` apa pun (skalar, array,
/// atau struct). `structs` berisi nilai struct localparam/parameter yang sudah
/// dievaluasi sebelumnya (fixed-point) agar `name.field` bisa di-const-eval.
pub fn eval_cval_full(
    expr: &Expr,
    merged: &Scalars,
    arrays: &Arrays,
    package_symbols: &HashMap<Symbol, HashMap<Symbol, PackageItem>>,
    structs: &HashMap<Symbol, Vec<SField>>,
) -> Option<CVal> {
    let ctx = PkgCtx {
        scalars: merged,
        arrays,
        package_symbols,
        structs,
    };
    eval_expr(expr, &ctx, None).ok()
}

/// Evaluasi default param dengan konteks PENUH untuk jalur resolusi
/// localparam header/body module (dipakai `resolve_param_values_with_ctx` di
/// param_util.rs sebagai fallback saat `const_eval_with_params` gagal).
///
/// `merged` = konstanta package (qualified `pkg::name`) + module ctx (plain
/// names — module menang) yang di-maintain pemanggil agar sinkron dengan
/// param_vals yang sedang diakumulasi (module param bisa direferensikan oleh
/// localparam berikutnya). `package_symbols` dipakai untuk resolve
/// `$bits(typedef)` (via `resolve_typedef_bits`) dan inlining fungsi package
/// (mis. `otbn_pkg::SecAddRandWidth`) — dua hal yang tidak bisa dilakukan
/// evaluator skalar sederhana `const_eval_with_params`.
///
/// Mengembalikan `Some(i64)` hanya untuk konstanta SKALAR (kontrak sama
/// dengan `const_eval_with_params`); array/struct → None.
pub fn eval_param_default_full(
    expr: &Expr,
    merged: &Scalars,
    arrays: &Arrays,
    package_symbols: &HashMap<Symbol, HashMap<Symbol, PackageItem>>,
    structs: &HashMap<Symbol, Vec<SField>>,
) -> Option<i64> {
    match eval_cval_full(expr, merged, arrays, package_symbols, structs) {
        Some(CVal::Scalar(v)) => Some(v),
        Some(CVal::Array(a)) => flatten(&a).first().copied(),
        _ => None,
    }
}

/// Map struct kosong statis untuk konteks yang tidak punya struct lokal.
fn no_structs() -> &'static HashMap<Symbol, Vec<SField>> {
    use std::sync::OnceLock;
    static EMPTY: OnceLock<HashMap<Symbol, Vec<SField>>> = OnceLock::new();
    EMPTY.get_or_init(HashMap::new)
}
