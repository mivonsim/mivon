//! lrm_model — tipe machine-readable untuk aturan IEEE 1800 LRM.
//!
//! Prinsip: jangan simpan LRM sebagai teks mentah. Buat semantic model
//! yang dapat dievaluasi secara programatik.
//!
//! Hierarki:
//! - `LrmRule`              — satu aturan LRM yang dapat dievaluasi
//! - `ClauseRef`            — referensi ke klausul standar
//! - `RuleClassification`   — required / undefined / impl-defined / ambiguous
//! - `LrmState`             — state design yang dievaluasi hakim
//! - `LrmValue`             — nilai 4-state LRM canonical

#![cfg(feature = "dev")]

pub mod clause;
pub mod predicate;
pub mod state;
pub mod value;

pub use clause::{ClauseRef, Standard};
pub use predicate::{Requirement, SemanticProp};
pub use state::{
    DesignModel, LrmState, ProcessModel, SchedulerModel, ScopeGraph, SymbolTable, TypeSystem,
};
pub use value::{LogicBit, LrmValue};

/// Identitas satu aturan LRM.
/// Contoh: "SV-TYPE-INT-001", "SV-ELAB-PARAM-003", "SV-SCHED-NBA-001"
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuleId(pub &'static str);

impl RuleId {
    pub fn as_str(&self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for RuleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Kategori domain aturan LRM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuleCategory {
    Syntax,
    Grammar,
    Legality,
    Semantics,
    Typing,
    Elaboration,
    Scheduling,
    Assertion,
    Constraint,
    Coverage,
    SystemFunction,
    Portability,
}

impl RuleCategory {
    pub fn label(self) -> &'static str {
        match self {
            RuleCategory::Syntax => "syntax",
            RuleCategory::Grammar => "grammar",
            RuleCategory::Legality => "legality",
            RuleCategory::Semantics => "semantics",
            RuleCategory::Typing => "typing",
            RuleCategory::Elaboration => "elaboration",
            RuleCategory::Scheduling => "scheduling",
            RuleCategory::Assertion => "assertion",
            RuleCategory::Constraint => "constraint",
            RuleCategory::Coverage => "coverage",
            RuleCategory::SystemFunction => "system_function",
            RuleCategory::Portability => "portability",
        }
    }
}

/// Klasifikasi perilaku menurut LRM.
///
/// Krusial: tidak semua perbedaan Mivon vs expected = bug.
/// LRM sendiri yang menentukan apakah perilaku wajib, tidak terdefinisi,
/// atau bergantung implementasi.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleClassification {
    /// LRM mewajibkan perilaku spesifik. Penyimpangan = violation.
    RequiredBehavior,
    /// LRM menyatakan perilaku tidak terdefinisi. Mivon boleh berbeda.
    UndefinedByLrm,
    /// LRM memberi kebebasan ke implementasi. Boleh berbeda dari simulator lain.
    ImplementationDefined,
    /// LRM ambigu. Perlu interpretasi manusia.
    LrmAmbiguous,
    /// Perilaku bergantung pada tool flags / elaboration context.
    ConditionalBehavior,
}

/// Satu aturan LRM yang dapat dievaluasi secara programatik.
pub struct LrmRule {
    pub id: RuleId,
    pub clause: ClauseRef,
    pub category: RuleCategory,
    pub classification: RuleClassification,

    /// Kapan aturan ini berlaku (predikat atas LrmState).
    pub applies_when: Box<dyn Fn(&LrmState) -> bool + Send + Sync>,

    /// Yang harus dipenuhi oleh implementasi.
    pub requires: Vec<Requirement>,

    /// Deskripsi perilaku yang diharapkan (human-readable).
    pub expected_description: &'static str,
}

impl std::fmt::Debug for LrmRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LrmRule")
            .field("id", &self.id)
            .field("clause", &self.clause)
            .field("category", &self.category)
            .field("classification", &self.classification)
            .field("expected_description", &self.expected_description)
            .finish()
    }
}
