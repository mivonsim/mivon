//! Timescale / delay-scale (F84, LRM §19.8) — extract dari mod.rs untuk
//! Arsitektur-06 (elaborator monolithic → submodule).
//!
//! Tick simulasi memakai satuan DASAR (finest) design; delay `#N` diskalakan
//! per-module via `module_delay_scale`. Seluruh method di sini adalah
//! `impl Elaborator` (butuh design/current_module); call site tak berubah.

use std::collections::HashMap;

use mivon_core::intern::Symbol;

use super::Elaborator;

impl Elaborator {
    /// Eksponen basis design = satuan TERHALUS (min) di antara directive
    /// design, module, dan interface. Default -9 (1ns) bila tanpa directive.
    pub(crate) fn design_base_exp(&self) -> i32 {
        let mut exps: Vec<i32> = Vec::new();
        if let Some(e) = self
            .design
            .timescale
            .as_ref()
            .and_then(|(u, _)| crate::util::generate::time_unit_exponent(u))
        {
            exps.push(e);
        }
        for module in &self.design.modules {
            if let Some(e) = module
                .timescale
                .as_ref()
                .and_then(|(u, _)| crate::util::generate::time_unit_exponent(u))
            {
                exps.push(e);
            }
        }
        for iface in &self.design.interfaces {
            if let Some(e) = iface
                .timescale
                .as_ref()
                .and_then(|(u, _)| crate::util::generate::time_unit_exponent(u))
            {
                exps.push(e);
            }
        }
        exps.into_iter().min().unwrap_or(-9)
    }

    /// Faktor skala delay untuk satu satuan `timescale` (F84).
    ///
    /// Dipakai untuk entri yang TIDAK ada di `design.modules` — misalnya
    /// `Interface` yang dielaborasi sebagai `Module` sintetik. Tanpa ini
    /// interface `1us` punya faktor 1 dan delay-nya salah diam-diam.
    pub(crate) fn delay_scale_for_unit(&self, unit: Option<&(String, String)>) -> u64 {
        let Some((u, _)) = unit else {
            return 1;
        };
        let Some(exp) = crate::util::generate::time_unit_exponent(u) else {
            return 1;
        };
        let diff = exp - self.design_base_exp();
        if diff <= 0 {
            1
        } else {
            // Batasi 12 digit supaya satuan raksasa (`1s` vs basis `1fs`) tak
            // overflow u64 dan delay jadi tak masuk akal.
            10u64.checked_pow(diff.min(12) as u32).unwrap_or(1)
        }
    }

    /// Hitung peta `module → faktor skala delay` (F84, LRM §19.8).
    ///
    /// `#N` di module dengan satuan lebih besar dari basis (`1us` pada basis
    /// `1ns`) dikali 10^diff; module pada basis dapat faktor 1. Karena basis =
    /// satuan TERHALUS design, `diff` tak pernah negatif — delay tak pernah
    /// terpotong jadi 0 dan tak ada risiko loop tak berujung.
    ///
    /// Module TANPA directive `timescale` tidak masuk peta (faktor 1). Itu
    /// keputusan produk, bukan LRM: §19.8 menyatakan modul tanpa directive
    /// memakai default simulator, dan `iverilog` memakai `1s` — jauh dari
    /// default mivon `1ns`. Memakai `1ns` menjaga kompatibilitas dengan
    /// ribuan design/test lama yang tak pernah menulis directive.
    pub(crate) fn compute_module_delay_scales(&mut self) {
        let mut scales: HashMap<Symbol, u64> = HashMap::new();
        for module in &self.design.modules {
            let scale = self.delay_scale_for_unit(module.timescale.as_ref());
            if module.timescale.is_some() {
                scales.insert(module.name, scale);
            }
        }
        // Interface ikut dihitung di sini karena `elaborate_module` untuk
        // module sintetiknya berjalan SETELAH fungsi ini; entri yang diinsert
        // nanti di loop interface (lihat pemanggil) tidak akan terpakai kalau
        // basisnya berubah di tengah. Aman: basis design-wide, bukan per-item.
        for iface in &self.design.interfaces {
            let scale = self.delay_scale_for_unit(iface.timescale.as_ref());
            if iface.timescale.is_some() {
                scales.insert(iface.name, scale);
            }
        }
        self.module_delay_scale = scales;
    }

    /// Faktor skala delay untuk module yang sedang dielaborasi (F84).
    pub(crate) fn current_delay_scale(&self) -> u64 {
        self.current_module
            .and_then(|m| self.module_delay_scale.get(&m).copied())
            .unwrap_or(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mivon_ast::types::Design;

    fn elab_ts(ts: Option<(&str, &str)>) -> Elaborator {
        let mut d = Design::default();
        d.timescale = ts.map(|(u, p)| (u.to_string(), p.to_string()));
        Elaborator::with_source(d, vec![], String::new())
    }

    #[test]
    fn test_delay_scale_unit_and_default() {
        let e = elab_ts(Some(("1us", "1ns")));
        // Basis = 1us (-6); 1ns (-9) di bawah basis → faktor 1.
        assert_eq!(e.design_base_exp(), -6);
        assert_eq!(e.delay_scale_for_unit(None), 1);
        assert_eq!(e.delay_scale_for_unit(Some(&("bogus".to_string(), "1ns".to_string()))), 1);
        assert_eq!(e.delay_scale_for_unit(Some(&("1ns".to_string(), "1ps".to_string()))), 1);
        // Tanpa directive: basis default -9.
        assert_eq!(elab_ts(None).design_base_exp(), -9);
        // current tanpa module → 1.
        assert_eq!(e.current_delay_scale(), 1);
    }

    #[test]
    fn test_compute_module_delay_scales() {
        use mivon_ast::types::{Interface, Module};
        // Design: basis 1ns (module b); module a 1us → 1000; tanpa
        // directive tidak masuk peta.
        let mk = |name: &str, ts: Option<(&str, &str)>| Module {
            name: Symbol::intern(name),
            ports: vec![],
            params: vec![],
            decls: vec![],
            items: vec![],
            timescale: ts.map(|(u, p)| (u.to_string(), p.to_string())),
        };
        let mut d = Design::default();
        d.modules = vec![
            mk("a", Some(("1us", "1ns"))),
            mk("b", Some(("1ns", "1ps"))),
            mk("c", None),
        ];
        d.interfaces = vec![Interface {
            name: Symbol::intern("ifc"),
            params: vec![],
            ports: vec![],
            decls: vec![],
            items: vec![],
            modports: vec![],
            timescale: Some(("1us".to_string(), "1ns".to_string())),
        }];
        let mut e = Elaborator::with_source(d, vec![], String::new());
        e.compute_module_delay_scales();
        assert_eq!(e.module_delay_scale.get(&Symbol::intern("a")).copied(), Some(1000));
        assert_eq!(e.module_delay_scale.get(&Symbol::intern("b")).copied(), Some(1));
        assert!(!e.module_delay_scale.contains_key(&Symbol::intern("c")));
        assert_eq!(e.module_delay_scale.get(&Symbol::intern("ifc")).copied(), Some(1000));
    }
}
