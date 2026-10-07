//! lrm_rules — database aturan LRM per domain.
//!
//! Setiap modul domain mendaftarkan aturan ke `RuleRegistry`.
//! Registry dibangun sekali saat kampanye dimulai.

#![cfg(feature = "dev")]

pub mod elaboration;
pub mod scheduling;
pub mod semantics;
pub mod syntax;
pub mod system;
pub mod types;

use std::collections::HashMap;
use crate::lrm_model::{LrmRule, LrmState, RuleCategory};

/// Registry semua aturan LRM yang terdaftar.
pub struct RuleRegistry {
    rules: HashMap<&'static str, LrmRule>,
    by_category: HashMap<RuleCategory, Vec<&'static str>>,
}

impl RuleRegistry {
    /// Bangun registry kosong lalu isi semua domain.
    pub fn build() -> Self {
        let mut reg = Self {
            rules: HashMap::new(),
            by_category: HashMap::new(),
        };
        syntax::register(&mut reg);
        semantics::register(&mut reg);
        types::register(&mut reg);
        elaboration::register(&mut reg);
        scheduling::register(&mut reg);
        system::register(&mut reg);
        reg
    }

    pub fn register(&mut self, rule: LrmRule) {
        let cat = rule.category;
        let id = rule.id.0;
        self.by_category.entry(cat).or_default().push(id);
        self.rules.insert(id, rule);
    }

    pub fn by_id(&self, id: &str) -> Option<&LrmRule> {
        self.rules.get(id)
    }

    pub fn for_category(&self, cat: RuleCategory) -> &[&'static str] {
        self.by_category.get(&cat).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Kembalikan semua aturan yang `applies_when(&state)` = true.
    pub fn applicable<'a>(&'a self, state: &LrmState) -> Vec<&'a LrmRule> {
        self.rules
            .values()
            .filter(|r| (r.applies_when)(state))
            .collect()
    }

    pub fn all(&self) -> impl Iterator<Item = &LrmRule> {
        self.rules.values()
    }
}
