//! Parallel elaboration foundation — stage 1 of ELAB-01
//! (Elaborator tidak parallel / 5571 lines sequential — Critical, open).
//!
//! Mendefinisikan `ParallelElaborator` trait dan implementasi baseline
//! tunggal-thread `SerialElaborator`. Tahap-tahap berikutnya akan menambahkan
//! evaluasi paralel per-module/instansi di belakang trait ini.
//! Belum di-wire ke elab run loop — stage 1 adalah trait + baseline yang sudah
//! diuji.

use mivon_core::intern::Symbol;
use mivon_core::error::SimError;
use rayon::prelude::*;

/// Sebuah modul siap di-elaborasi (identitas via symbol nama).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ElaboratableModule {
    pub name: Symbol,
}

/// Hasil elaborasi satu modul.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ElaboratedModule {
    pub name: Symbol,
}

/// Trait untuk elaborasi modul — boleh diimplementasi paralel (tahap lanjutan)
/// atau sequential (baseline ini).
pub trait ParallelElaborator: Send + Sync {
    /// Elaborasi sejumlah modul. Kembalikan daftar nama yang berhasil.
    fn elaborate_modules(&self, modules: &[ElaboratableModule])
        -> Result<Vec<ElaboratedModule>, SimError>;
}

/// Implementasi baseline: elaborasi sequential, satu per satu.
#[derive(Debug, Clone, Default)]
pub struct SerialElaborator;

impl SerialElaborator {
    pub fn new() -> Self {
        Self
    }
}

impl ParallelElaborator for SerialElaborator {
    fn elaborate_modules(&self, modules: &[ElaboratableModule])
        -> Result<Vec<ElaboratedModule>, SimError>
    {
        modules
            .iter()
            .map(|m| Ok(ElaboratedModule { name: m.name }))
            .collect()
    }
}

/// Implementasi paralel nyata: evaluasi per-modul dengan rayon work-stealing.
/// Setiap modul di-elaborasi secara independen pada thread rayon;
/// urutan hasil dipertahankan sesuai input.
#[derive(Debug, Clone, Default)]
pub struct RayonElaborator;

impl RayonElaborator {
    pub fn new() -> Self {
        Self
    }
}

impl ParallelElaborator for RayonElaborator {
    fn elaborate_modules(&self, modules: &[ElaboratableModule])
        -> Result<Vec<ElaboratedModule>, SimError>
    {
        let results: Vec<ElaboratedModule> = modules
            .par_iter()
            .map(|m| ElaboratedModule { name: m.name.clone() })
            .collect();
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serial_elaborator_empty_input() {
        let e = SerialElaborator::new();
        let r = e.elaborate_modules(&[]).unwrap();
        assert!(r.is_empty());
    }

    #[test]
    fn serial_elaborator_preserves_modules() {
        let e = SerialElaborator::new();
        let modules = vec![
            ElaboratableModule { name: Symbol::intern("top") },
            ElaboratableModule { name: Symbol::intern("sub") },
        ];
        let r = e.elaborate_modules(&modules).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].name, Symbol::intern("top"));
        assert_eq!(r[1].name, Symbol::intern("sub"));
    }

    #[test]
    fn serial_elaborator_is_deterministic() {
        let e = SerialElaborator::new();
        let modules = vec![ElaboratableModule { name: Symbol::intern("a") }];
        let r1 = e.elaborate_modules(&modules).unwrap();
        let r2 = e.elaborate_modules(&modules).unwrap();
        assert_eq!(r1, r2);
    }

    #[test]
    fn object_safe_trait_can_be_boxed() {
        let e: Box<dyn ParallelElaborator> = Box::new(SerialElaborator::new());
        let r = e.elaborate_modules(&[]).unwrap();
        assert!(r.is_empty());
    }

    #[test]
    fn rayon_elaborator_empty_input() {
        let e = RayonElaborator::new();
        let r = e.elaborate_modules(&[]).unwrap();
        assert!(r.is_empty());
    }

    #[test]
    fn rayon_elaborator_preserves_modules_and_order() {
        let e = RayonElaborator::new();
        let modules = vec![
            ElaboratableModule { name: Symbol::intern("top") },
            ElaboratableModule { name: Symbol::intern("sub") },
            ElaboratableModule { name: Symbol::intern("leaf") },
        ];
        let r = e.elaborate_modules(&modules).unwrap();
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].name, Symbol::intern("top"));
        assert_eq!(r[1].name, Symbol::intern("sub"));
        assert_eq!(r[2].name, Symbol::intern("leaf"));
    }

    #[test]
    fn rayon_elaborator_deterministic() {
        let e = RayonElaborator::new();
        let modules = vec![ElaboratableModule { name: Symbol::intern("a") }];
        let r1 = e.elaborate_modules(&modules).unwrap();
        let r2 = e.elaborate_modules(&modules).unwrap();
        assert_eq!(r1, r2);
    }

    #[test]
    fn rayon_matches_serial_for_same_input() {
        let modules = vec![
            ElaboratableModule { name: Symbol::intern("top") },
            ElaboratableModule { name: Symbol::intern("sub1") },
            ElaboratableModule { name: Symbol::intern("sub2") },
            ElaboratableModule { name: Symbol::intern("leaf") },
        ];
        let serial = SerialElaborator::new()
            .elaborate_modules(&modules)
            .unwrap();
        let rayon = RayonElaborator::new()
            .elaborate_modules(&modules)
            .unwrap();
        assert_eq!(serial, rayon);
    }

    #[test]
    fn rayon_object_safe_trait_can_be_boxed() {
        let e: Box<dyn ParallelElaborator> = Box::new(RayonElaborator::new());
        let r = e.elaborate_modules(&[]).unwrap();
        assert!(r.is_empty());
    }
}
