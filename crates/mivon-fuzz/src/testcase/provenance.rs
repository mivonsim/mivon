//! TestProvenance — asal-usul dan mutation chain satu testcase.

#![cfg(feature = "dev")]

/// Asal-usul testcase — seed, mutation chain, rule yang ditarget.
#[derive(Debug, Clone, Default)]
pub struct TestProvenance {
    pub seed: u64,
    pub mutation_chain: Vec<MutationRecord>,
    /// ID testcase parent (bila hasil derivasi dari testcase lain).
    pub parent_test: Option<String>,
    /// Aturan LRM spesifik yang ditarget generator.
    pub rule_targeted: Option<String>,
}

impl TestProvenance {
    pub fn from_seed(seed: u64) -> Self {
        Self {
            seed,
            ..Default::default()
        }
    }

    pub fn with_rule(mut self, rule: &'static str) -> Self {
        self.rule_targeted = Some(rule.to_string());
        self
    }

    pub fn add_mutation(&mut self, op: impl Into<String>, detail: impl Into<String>) {
        self.mutation_chain.push(MutationRecord {
            op: op.into(),
            detail: detail.into(),
        });
    }
}

/// Satu langkah mutasi dalam chain.
#[derive(Debug, Clone)]
pub struct MutationRecord {
    /// Nama operasi mutasi — contoh: "replace_keyword", "inject_delay"
    pub op: String,
    /// Detail tambahan — contoh: "from=always_comb to=always_ff"
    pub detail: String,
}
