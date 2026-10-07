//! testcase — TestCase, TestManifest, dan metadata generator.
//!
//! Setiap testcase bukan hanya file `.sv`. Ia membawa metadata:
//! - aturan LRM yang diserang
//! - fitur SV yang terlibat
//! - asal-usul (seed, mutation chain, parent test)

#![cfg(feature = "dev")]

pub mod provenance;

pub use provenance::{MutationRecord, TestProvenance};

/// Testcase lengkap dengan metadata.
#[derive(Debug, Clone)]
pub struct TestCase {
    /// ID unik — contoh: "FZ-000184"
    pub id: String,
    /// Source SystemVerilog.
    pub source: String,
    pub manifest: TestManifest,
    pub provenance: TestProvenance,
}

impl TestCase {
    pub fn new(id: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            source: source.into(),
            manifest: TestManifest::default(),
            provenance: TestProvenance::default(),
        }
    }

    pub fn with_manifest(mut self, manifest: TestManifest) -> Self {
        self.manifest = manifest;
        self
    }

    pub fn with_provenance(mut self, prov: TestProvenance) -> Self {
        self.provenance = prov;
        self
    }
}

/// Metadata testcase — aturan yang diserang, fitur yang terlibat.
#[derive(Debug, Clone, Default)]
pub struct TestManifest {
    pub test_id: String,
    pub generated_by: GeneratorKind,
    /// ID aturan LRM yang diserang — contoh: ["SV-TYPE-INT-001"]
    pub target_rules: Vec<String>,
    /// Fitur SV yang terlibat — contoh: ["signed_integer", "packed_array"]
    pub features: Vec<String>,
    /// Nomor klausul LRM yang relevan — contoh: ["6.24.1", "11.4.14"]
    pub lrm_clauses: Vec<String>,
}

impl TestManifest {
    pub fn blind(test_id: impl Into<String>) -> Self {
        Self {
            test_id: test_id.into(),
            generated_by: GeneratorKind::BlindStructural,
            ..Default::default()
        }
    }

    pub fn rule_directed(
        test_id: impl Into<String>,
        rules: Vec<&'static str>,
        clauses: Vec<&'static str>,
    ) -> Self {
        Self {
            test_id: test_id.into(),
            generated_by: GeneratorKind::RuleDirected,
            target_rules: rules.iter().map(|s| s.to_string()).collect(),
            lrm_clauses: clauses.iter().map(|s| s.to_string()).collect(),
            features: Vec::new(),
        }
    }
}

/// Jenis generator yang menghasilkan testcase ini.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum GeneratorKind {
    /// Generator mengetahui aturan yang diserang.
    RuleDirected,
    /// Generator acak — hakim menentukan aturan setelah observasi.
    #[default]
    BlindStructural,
    /// Transformasi semantically equivalent (metamorphic testing).
    Metamorphic { transformation: String },
}
