//! Type resolution functions for the elaborator.
//!
//! Extracted from `mod.rs` as part of Arsitektur-06 to reduce monolithic file size.

use std::collections::HashMap;

use mivon_ast::types::const_eval_with_params;
use mivon_ast::*;
use mivon_core::diagnostics::diagnostic::DiagCode;
use mivon_core::error::SimError;
use mivon_core::intern::Symbol;
use mivon_ir::*;

use super::super::util::width;
use super::Elaborator;
use super::BUILTIN_UVM_CLASSES;

impl Elaborator {
    pub(crate) fn store_typedef_fields(&mut self, name: Symbol, dtype: &DataType) {
        let fields = self.compute_struct_fields(dtype);
        if !fields.is_empty() {
            self.typedef_field_map.insert(name, fields);
        }
    }

    /// Batas kedalaman resolusi struct nested (typedef saling merujuk) —
    /// jaring pengaman dari rekursi `sub_fields` berjenjang.
    const MAX_STRUCT_DEPTH: usize = 16;

    /// Cari struct fields untuk nama tipe — polos (`reg2hw_t`) atau scoped
    /// (`pkg::reg2hw_t`). Prioritas: typedef_field_map (typedef yang sudah
    /// di-store via import/typedef module), lalu package_symbols langsung
    /// (scoped type tanpa import eksplisit).
    pub(crate) fn lookup_struct_fields(&self, type_name: &str) -> Option<Vec<StructFieldInfo>> {
        self.lookup_struct_fields_depth(type_name, 0)
    }

    /// Varian ber-depth untuk dipakai dari `struct_field_from_member_depth`
    /// (mengisi `sub_fields` berjenjang tanpa risiko loop tak berujung).
    pub(crate) fn lookup_struct_fields_depth(
        &self,
        type_name: &str,
        depth: usize,
    ) -> Option<Vec<StructFieldInfo>> {
        if depth >= Self::MAX_STRUCT_DEPTH {
            return None;
        }
        let type_sym = Symbol::intern(type_name);
        // 1. Cek map yang sudah di-store (nama polos & scoped).
        if let Some(f) = self.typedef_field_map.get(&type_sym) {
            if !f.is_empty() {
                return Some(f.clone());
            }
        }
        // 2. Scoped `pkg::type` — cari typedef di package asal.
        if let Some((pkg, t)) = type_name.split_once("::") {
            if let Some(items) = self.package_symbols.get(&Symbol::intern(pkg)) {
                if let Some(PackageItem::Typedef(td)) = items.get(&Symbol::intern(t)) {
                    if matches!(
                        &td.dtype,
                        DataType::StructType { .. } | DataType::UnionType { .. }
                    ) {
                        let fields = self.compute_struct_fields_depth(&td.dtype, depth);
                        if !fields.is_empty() {
                            return Some(fields);
                        }
                    }
                }
            }
            // Key map bisa juga `pkg::type` — cek sekali lagi dengan nama asli.
            if let Some(f) = self.typedef_field_map.get(&type_sym) {
                if !f.is_empty() {
                    return Some(f.clone());
                }
            }
        }
        // 3. Nama polos — cari typedef di SEMUA package (nested struct field
        //    bertipe typedef package lain, mis. `intr_test_reg_t` di dalam
        //    `reg2hw_t` tanpa import eksplisit).
        for items in self.package_symbols.values() {
            if let Some(PackageItem::Typedef(td)) = items.get(&type_sym) {
                if matches!(
                    &td.dtype,
                    DataType::StructType { .. } | DataType::UnionType { .. }
                ) {
                    let fields = self.compute_struct_fields_depth(&td.dtype, depth);
                    if !fields.is_empty() {
                        return Some(fields);
                    }
                }
            }
        }
        None
    }

    /// Resolve lebar DeclVar dengan fallback width-aware: bila range memakai
    /// `$bits(signal)`/`$size(signal)` (const-eval skalar gagal), hitung dari
    /// lebar sinyal yang sudah terdaftar di signal_map.
    pub(crate) fn var_resolved_width_aware(
        &self,
        var: &DeclVar,
        effective_params: &HashMap<Symbol, i64>,
        signal_map: &HashMap<Symbol, SignalId>,
        signals: &[SignalInfo],
    ) -> Result<usize, String> {
        match var.resolved_width(effective_params) {
            Ok(w) => Ok(w),
            Err(e) => {
                let mut total: usize = 1;
                if let Some(er) = &var.expr_range {
                    total = self
                        .range_width_aware(er, effective_params, signal_map, signals)
                        .map_err(|_| e.clone())?;
                }
                for (er, _) in &var.extra_packed_dims {
                    total = total.saturating_mul(
                        self.range_width_aware(er, effective_params, signal_map, signals)
                            .map_err(|_| e.clone())?,
                    );
                }
                Ok(total)
            }
        }
    }

    /// Resolve lebar satu ExprRange dengan fallback width-aware (`$bits(sig)`).
    pub(crate) fn range_width_aware(
        &self,
        er: &ExprRange,
        effective_params: &HashMap<Symbol, i64>,
        signal_map: &HashMap<Symbol, SignalId>,
        signals: &[SignalInfo],
    ) -> Result<usize, String> {
        if let Ok(r) = resolve_expr_range(er, effective_params) {
            return Ok(r.width());
        }
        let msb = width::eval_width_aware_param(
            &er.msb,
            signal_map,
            signals,
            effective_params,
            &self.package_symbols,
        )
        .ok_or_else(|| "cannot resolve range bound".to_string())?;
        let lsb = width::eval_width_aware_param(
            &er.lsb,
            signal_map,
            signals,
            effective_params,
            &self.package_symbols,
        )
        .ok_or_else(|| "cannot resolve range bound".to_string())?;
        Ok((msb.abs_diff(lsb) + 1) as usize)
    }

    /// Resolve lebar port dengan fallback width-aware (`$bits(pkg::Type)` dsb)
    /// bila const-eval skalar gagal. Port `input logic [$bits(pkg::t)-1:0] x`
    /// memerlukan lebar typedef package yang tidak bisa di-const-eval sebagai
    /// skalar — pakai `range_width_aware` (eval_width_aware_param) sebagai
    /// fallback agar port tetap ter-elaborasi.
    pub(crate) fn port_width_aware(
        &self,
        port: &Port,
        effective_params: &HashMap<Symbol, i64>,
        signal_map: &HashMap<Symbol, SignalId>,
        signals: &[SignalInfo],
    ) -> Result<usize, String> {
        match port.resolved_width(effective_params) {
            Ok(w) => {
                // Port bertipe UserDefined / base type lain TANPA range eksplisit:
                // `Port::resolved_width` hanya menghitung dimensi range (default 1)
                // dan mengabaikan `dtype_name`. Akibatnya port enum (`state_e`,
                // `enum logic [1:0]` → 2 bit), struct (`pair_t`), atau `int`
                // semuanya jadi 1-bit → WR0102 palsu & data truncation. Ambil
                // lebar dari tipe saat tidak ada range yang di-deklarasikan.
                if w == 1
                    && port.range.is_none()
                    && port.expr_range.is_none()
                    && port.extra_packed_dims.is_empty()
                {
                    if let Some(tn) = &port.dtype_name {
                        if let Some(dw) = self.port_base_type_width(tn.as_str()) {
                            return Ok(dw);
                        }
                    }
                }
                Ok(w)
            }
            Err(e) => {
                let mut total: usize = 1;
                if let Some(r) = &port.range {
                    total = r.width();
                } else if let Some(er) = &port.expr_range {
                    total = self
                        .range_width_aware(er, effective_params, signal_map, signals)
                        .map_err(|_| e.clone())?;
                }
                for er in &port.extra_packed_dims {
                    total = total.saturating_mul(
                        self.range_width_aware(er, effective_params, signal_map, signals)
                            .map_err(|_| e.clone())?,
                    );
                }
                Ok(total)
            }
        }
    }

    /// Lebar dtype dengan dukungan parameter type (mis. `parameter type T = int`).
    /// `T x;` di body harus pakai lebar dari `type_param_widths` (bila T adalah
    /// type param), bukan jatuh ke fallback "unknown type".
    pub(crate) fn resolve_dtype_width(
        &self,
        dtype: &DataType,
        type_param_widths: &HashMap<Symbol, usize>,
    ) -> Result<usize, SimError> {
        if let DataType::UserDefined(tn) = dtype {
            if let Some(&tw) = type_param_widths.get(tn) {
                return Ok(tw);
            }
        }
        self.resolve_type_width(dtype)
    }

    pub(crate) fn resolve_type_width(&self, dtype: &DataType) -> Result<usize, SimError> {
        match dtype {
            DataType::UserDefined(name) if name == "__mailbox" || name == "__semaphore" => Ok(64),
            DataType::UserDefined(name) if name == "process" => Ok(64),
            // `chandle` — built-in SV type untuk C pointer (DPI). 64-bit opaque pointer.
            DataType::UserDefined(name) if name == "chandle" => Ok(64),
            DataType::UserDefined(name) if BUILTIN_UVM_CLASSES.contains(&name.as_str()) => Ok(64),
            DataType::UserDefined(name) => {
                if self.design.classes.iter().any(|c| c.name == *name) {
                    return Ok(64);
                }
                if self.design.modules.iter().any(|m| {
                    m.items
                        .iter()
                        .any(|item| matches!(item, ModuleItem::Covergroup(cg) if cg.name == *name))
                }) {
                    return Ok(64);
                }
                // Check package symbols for typedefs
                for pkg_items in self.package_symbols.values() {
                    if let Some(PackageItem::Typedef(td)) = pkg_items.get(name) {
                        let width = self.resolve_typedef_width_dims(
                            &td.dtype,
                            td.range.as_ref(),
                            &td.extra_packed_dims,
                            &self.param_vals,
                        );
                        if width > 0 {
                            return Ok(width);
                        }
                    }
                }
                // Scoped type name `pkg::type` — cari di package yang tepat.
                // Nama disimpan sebagai "pkg::type", sedangkan key package
                // symbols hanya berisi nama tipe tanpa prefix.
                if let Some((pkg, type_name)) = name.as_str().split_once("::") {
                    if let Some(pkg_items) = self.package_symbols.get(&Symbol::intern(pkg)) {
                        if let Some(PackageItem::Typedef(td)) =
                            pkg_items.get(&Symbol::intern(type_name))
                        {
                            let width = self.resolve_typedef_width_dims(
                                &td.dtype,
                                td.range.as_ref(),
                                &td.extra_packed_dims,
                                &self.param_vals,
                            );
                            if width > 0 {
                                return Ok(width);
                            }
                        }
                    }
                }
                // Check in-module typedefs stored in typedef_map
                if let Some(&width) = self.typedef_map.get(name) {
                    return Ok(width);
                }
                // Class handle (UVM dsb.) — tipe VALID, bukan "unknown type".
                // Class di body module/interface dikumpulkan parser ke
                // `design.classes` (contoh: `prim_count_if_proxy` di dalam
                // interface `prim_count_if`). Jangan warn; lebar handle = 64.
                if self.design.classes.iter().any(|c| c.name == *name) {
                    return Ok(64);
                }
                // Type tidak ditemukan — emit warning dan gunakan lebar 1 agar
                // elaborasi tetap berlanjut. Type yang hilang biasanya karena
                // package belum di-import ke scope interface/module ini.
                self.elab_warn_at(
                    DiagCode::UndefinedSignal,
                    format!("unknown type '{}' is not defined in this scope", name),
                    0,
                    0,
                );
                Ok(1)
            }
            DataType::Signed(inner) => self.resolve_type_width(inner),
            _ => Ok(dtype.width()),
        }
    }

    /// Lebar base type port tanpa range eksplisit. `Port::resolved_width`
    /// mengabaikan `dtype_name`, jadi port bertipe enum/struct/`int`/dst
    /// selalu 1-bit. Base type builtin di-map langsung; selain itu delegasi ke
    /// `resolve_cast_name_width` (typedef/param package tanpa warning).
    pub(crate) fn port_base_type_width(&self, name: &str) -> Option<usize> {
        Some(match name {
            "logic" | "wire" | "reg" | "tri" | "tri0" | "tri1" | "wand" | "wor" | "triand"
            | "trior" | "supply0" | "supply1" | "bit" => 1,
            "byte" => 8,
            "shortint" => 16,
            "int" | "integer" => 32,
            "longint" | "time" | "real" | "realtime" => 64,
            _ => return self.resolve_cast_name_width(name),
        })
    }

    /// Resolve lebar cast type berupa identifier yang tidak dikenali
    /// `parse_type_spec_str` (hanya base types): parameter modul/package
    /// (mis. `MuBi4Width'(x)` dari `import prim_mubi_pkg::*`) atau typedef
    /// package (mis. `mubi4_t'(x)`). Prioritas: param modul → package param
    /// (default) → typedef package.
    pub(crate) fn resolve_cast_name_width(&self, type_name: &str) -> Option<usize> {
        // 0. Size cast numerik eksplisit (`22'(x)`, `8'(y)`): lebar = angka.
        // Sebelumnya tak ter-resolve → fallback 1 → warning width mismatch
        // palsu (mis. `data_o = 22'(data_i)` dilaporkan rhs=1).
        let digits: String = type_name.chars().filter(|c| *c != '_').collect();
        if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
            return digits.parse::<usize>().ok();
        }
        let name = Symbol::intern(type_name);
        // 1. Parameter modul / konstanta ter-evaluasi.
        if let Some(&v) = self.param_vals.get(&name) {
            return Some(v as usize);
        }
        // 2. Package param di-import (nama polos) — mis. `MuBi4Width`.
        for items in self.package_symbols.values() {
            if let Some(PackageItem::Param(p)) = items.get(&name) {
                if let Some(expr) = &p.default {
                    if let Ok(v) = const_eval_with_params(expr, &self.param_vals) {
                        return Some(v as usize);
                    }
                }
            }
        }
        // 3. Typedef package — mis. `mubi4_t'(x)`. Lebar dihitung dari range
        // typedef (`typedef logic [7:0] tl_dhw_t;` → 8) × packed dims tambahan;
        // `td.dtype` saja hanya berisi base type (Logic → 1).
        for items in self.package_symbols.values() {
            if let Some(PackageItem::Typedef(td)) = items.get(&name) {
                let w = self.resolve_typedef_width_dims(
                    &td.dtype,
                    td.range.as_ref(),
                    &td.extra_packed_dims,
                    &self.param_vals,
                );
                if w > 0 {
                    return Some(w);
                }
            }
        }
        // 4. Qualified package member: `top_pkg::tl_dhw_t'(x)` — package_symbols
        // menyimpan item bare per-package (`pkg → {item → PackageItem}`), jadi
        // nama full-qualified perlu di-split dulu sebelum lookup.
        if let Some(idx) = type_name.find("::") {
            let pkg_sym = Symbol::intern(&type_name[..idx]);
            let item_sym = Symbol::intern(&type_name[idx + 2..]);
            if let Some(items) = self.package_symbols.get(&pkg_sym) {
                if let Some(PackageItem::Param(p)) = items.get(&item_sym) {
                    if let Some(expr) = &p.default {
                        if let Ok(v) = const_eval_with_params(expr, &self.param_vals) {
                            return Some(v as usize);
                        }
                    }
                }
                if let Some(PackageItem::Typedef(td)) = items.get(&item_sym) {
                    let w = self.resolve_typedef_width_dims(
                        &td.dtype,
                        td.range.as_ref(),
                        &td.extra_packed_dims,
                        &self.param_vals,
                    );
                    if w > 0 {
                        return Some(w);
                    }
                }
            }
        }
        // 5. F33: typedef level FILE — delegasi ke resolve_type_width
        // (typedef_map berisi typedef level-file; hasil `mgen` .mv meng-emit
        // typedef ke `.svh`, bukan package). `Word16'(x)` utk
        // `typedef logic [15:0] Word16;` → 16 (sebelumnya None → resize 1 →
        // data loss). F33 fix review: GATE dgn typedef_map — nama yang BUKAN
        // typedef (mis. type param module `T'(x)`) return None TANPA memicu
        // warning "unknown type" dari resolve_type_width (yang emit elab_warn
        // utk nama tak dikenal → warning palsu per `T'(a)`).
        if self.typedef_map.contains_key(&name) {
            if let Ok(w) = self.resolve_type_width(&DataType::UserDefined(name)) {
                return Some(w);
            }
        }
        None
    }

    pub(crate) fn compute_struct_fields(&self, dtype: &DataType) -> Vec<StructFieldInfo> {
        self.compute_struct_fields_depth(dtype, 0)
    }

    /// Varian ber-depth — membatasi rekursi nested typedef (pengaman loop).
    pub(crate) fn compute_struct_fields_depth(
        &self,
        dtype: &DataType,
        depth: usize,
    ) -> Vec<StructFieldInfo> {
        match dtype {
            DataType::UnionType { members } => members
                .iter()
                .map(|m| self.struct_field_from_member_depth(m, 0, depth))
                .collect(),
            DataType::StructType { members } => {
                let mut fields = Vec::new();
                let mut offset = 0usize;
                let members_rev: Vec<_> = members.iter().rev().collect();
                for m in &members_rev {
                    let f = self.struct_field_from_member_depth(m, offset, depth);
                    offset += f.width.max(1);
                    fields.push(f);
                }
                fields.reverse();
                fields
            }
            _ => vec![],
        }
    }

    /// Bangun satu `StructFieldInfo` dari member struct/union. Untuk field
    /// bertipe typedef (`UserDefined`) simpan `type_name` agar chain bisa
    /// resolve lewat typedef_field_map; untuk anonymous struct/union inline
    /// simpan `sub_fields` (dari compute_struct_fields) agar `a.b.c` tetap
    /// bisa di-resolve berjenjang tanpa nama tipe.
    pub(crate) fn struct_field_from_member(
        &self,
        m: &StructMember,
        offset: usize,
    ) -> StructFieldInfo {
        self.struct_field_from_member_depth(m, offset, 0)
    }

    /// Varian ber-depth: `sub_fields` field typedef ikut diisi agar rantai
    /// member access `a.b.c.d` bisa turun ke field LEAF di free-fn
    /// `compute_expr_width` (util/width.rs) yang TIDAK punya akses
    /// `typedef_field_map`. Tanpa ini fallback-nya = lebar objek induk →
    /// lebar port salah, cuma muncul sbg warning WR0102 (bug menyamar
    /// sebagai warning — kasus bug_0793).
    pub(crate) fn struct_field_from_member_depth(
        &self,
        m: &StructMember,
        offset: usize,
        depth: usize,
    ) -> StructFieldInfo {
        // Lebar member: range eksplisit menang; tanpanya resolve lebar typedef
        // (enum 2-bit, mubi4_t 4-bit, struct nested) — bukan 1-bit default.
        // Tanpa ini `phase_e phase` (enum) jadi 1-bit dan offset struct salah.
        let w = if let Some(r) = &m.range {
            r.width()
        } else if let DataType::UserDefined(t) = m.dtype.as_ref() {
            self.resolve_cast_name_width(t.as_str()).unwrap_or(1)
        } else {
            // Member tanpa range eksplisit (`int y;`, `byte x;`, `real r;`)
            // lebarnya dari TIPE, bukan default 1-bit. PRA-FIX default 1 →
            // offset field berikutnya salah dan tulis ke `s.x = 8'hAA` hilang
            // (baca 0) karena targetnya di luar lebar struct.
            self.resolve_type_width(m.dtype.as_ref()).unwrap_or(1)
        };
        let signed_field = is_signed_type(m.dtype.as_ref());
        match m.dtype.as_ref() {
            DataType::UserDefined(t) => StructFieldInfo {
                name: m.name,
                offset,
                width: w,
                is_signed: signed_field,
                type_name: Some(*t),
                sub_fields: if depth < Self::MAX_STRUCT_DEPTH {
                    self.lookup_struct_fields_depth(t.as_str(), depth + 1)
                        .unwrap_or_default()
                } else {
                    vec![]
                },
            },
            DataType::StructType { .. } | DataType::UnionType { .. } => StructFieldInfo {
                name: m.name,
                offset,
                width: w,
                is_signed: false,
                // Anonymous struct/union inline — tidak ada nama tipe untuk
                // lookup typedef_field_map; simpan fields langsung.
                type_name: None,
                sub_fields: self.compute_struct_fields_depth(m.dtype.as_ref(), depth + 1),
            },
            _ => StructFieldInfo {
                name: m.name,
                offset,
                width: w,
                is_signed: signed_field,
                type_name: None,
                sub_fields: vec![],
            },
        }
    }
}
