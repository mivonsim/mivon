//! Lazy Elaborator — on-demand module elaboration.
//!
//! Module hanya di-elaborate saat diminta. Hasil di-cache supaya
//! tidak perlu re-elaborate saat parameter atau dependency tidak berubah.

use std::sync::Arc;

use dashmap::DashMap;

use super::builder::HirBuilder;
use super::hir::HirModule;
use super::types::TypeSystem;
use mivon_core::intern::Symbol;

/// Lazy elaboration engine — on-demand module elaboration.
pub struct LazyElaborator {
    /// Cache of elaborated modules (name → HIR)
    elaborated: DashMap<Symbol, Arc<HirModule>>,
    /// Currently being elaborated (prevent double-elaboration)
    in_progress: DashSet<Symbol>,
    /// Builder for AST → HIR conversion
    builder: parking_lot::Mutex<HirBuilder>,
    /// Shared type system (lazy type resolution)
    type_system: Arc<TypeSystem>,
}

// Re-export DashSet since it's not in scope
use dashmap::DashSet;

impl LazyElaborator {
    pub fn new() -> Self {
        let type_system = Arc::new(TypeSystem::new());
        let builder = HirBuilder::with_type_system(type_system.clone());
        LazyElaborator {
            elaborated: DashMap::new(),
            in_progress: DashSet::new(),
            builder: parking_lot::Mutex::new(builder),
            type_system,
        }
    }

    /// Get the shared type system.
    pub fn type_system(&self) -> &TypeSystem {
        &self.type_system
    }

    /// Get an Arc reference to the shared type system.
    pub fn shared_type_system(&self) -> Arc<TypeSystem> {
        self.type_system.clone()
    }

    /// Elaborate a module on-demand. Returns cached version if available.
    pub fn elaborate(&self, name: Symbol) -> Option<Arc<HirModule>> {
        // 1. Check cache
        if let Some(module) = self.elaborated.get(&name) {
            return Some(module.clone());
        }

        // 2. Already being elaborated (by another thread)
        if self.in_progress.contains(&name) {
            // Wait and retry
            return self.elaborated.get(&name).map(|m| m.clone());
        }

        // Not found — caller must provide AST data and call elaborate_with_data
        None
    }

    /// Elaborate a module with provided data.
    pub fn elaborate_with_data(
        &self,
        name: Symbol,
        params: Vec<super::hir::HirParam>,
        signals: Vec<super::hir::HirSignal>,
        stmts: Vec<super::hir::HirStmt>,
    ) -> Arc<HirModule> {
        // Check cache first
        if let Some(module) = self.elaborated.get(&name) {
            return module.clone();
        }

        // Mark as in-progress
        self.in_progress.insert(name);

        // Build HIR
        let module = {
            let mut builder = self.builder.lock();
            builder.build_module(name, params, signals, stmts)
        };

        let module = Arc::new(module);

        // Cache and unmark
        self.elaborated.insert(name, module.clone());
        self.in_progress.remove(&name);

        module
    }

    /// ELAB-02 stage 2: on-demand module elaboration convenience.
    /// Returns cached elaborated module if already elaborated, otherwise None
    /// (caller must supply AST data via elaborate_with_data).
    pub fn elaborate_module(&self, name: Symbol) -> Option<Arc<HirModule>> {
        self.elaborate(name)
    }

    /// Check if a module has been elaborated.
    pub fn is_elaborated(&self, name: Symbol) -> bool {
        self.elaborated.contains_key(&name)
    }

    /// Get all elaborated module names.
    pub fn module_names(&self) -> Vec<Symbol> {
        self.elaborated.iter().map(|entry| *entry.key()).collect()
    }

    /// Number of elaborated modules.
    pub fn len(&self) -> usize {
        self.elaborated.len()
    }

    /// Is empty.
    pub fn is_empty(&self) -> bool {
        self.elaborated.is_empty()
    }

    /// Invalidate all cached modules.
    pub fn invalidate_all(&self) {
        self.elaborated.clear();
    }

    /// Invalidate a specific module.
    pub fn invalidate(&self, name: Symbol) {
        self.elaborated.remove(&name);
    }
}

impl Default for LazyElaborator {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lazy_elab_basic() {
        let elab = LazyElaborator::new();
        let name = Symbol::intern("lazy_mod");

        assert!(!elab.is_elaborated(name));
        assert!(elab.elaborate(name).is_none());

        elab.elaborate_with_data(name, vec![], vec![], vec![]);
        assert!(elab.is_elaborated(name));
        assert!(elab.elaborate(name).is_some());
    }

    #[test]
    fn test_lazy_elab_module_convenience() {
        let elab = LazyElaborator::new();
        let name = Symbol::intern("mod_a");

        // Not elaborated yet -> None.
        assert!(elab.elaborate_module(name).is_none());

        // After elaborate_with_data, convenience wrapper returns Some.
        elab.elaborate_with_data(name, vec![], vec![], vec![]);
        assert!(elab.elaborate_module(name).is_some());
        assert_eq!(elab.elaborate_module(name).unwrap().name, name);
    }

    #[test]
    fn test_lazy_elab_invalidate() {
        let elab = LazyElaborator::new();
        let name = Symbol::intern("inv_mod");

        elab.elaborate_with_data(name, vec![], vec![], vec![]);
        assert!(elab.is_elaborated(name));

        elab.invalidate(name);
        assert!(!elab.is_elaborated(name));
    }

    #[test]
    fn test_lazy_elab_module_names() {
        let elab = LazyElaborator::new();
        let a = Symbol::intern("mod_a");
        let b = Symbol::intern("mod_b");
        elab.elaborate_with_data(a, vec![], vec![], vec![]);
        elab.elaborate_with_data(b, vec![], vec![], vec![]);
        let names = elab.module_names();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&a));
        assert!(names.contains(&b));
        assert_eq!(elab.len(), 2);
        assert!(!elab.is_empty());
    }

    #[test]
    fn test_lazy_elab_invalidate_all() {
        let elab = LazyElaborator::new();
        elab.elaborate_with_data(Symbol::intern("x"), vec![], vec![], vec![]);
        elab.elaborate_with_data(Symbol::intern("y"), vec![], vec![], vec![]);
        assert_eq!(elab.len(), 2);
        elab.invalidate_all();
        assert!(elab.is_empty());
        assert_eq!(elab.len(), 0);
        assert!(!elab.is_elaborated(Symbol::intern("x")));
    }

    #[test]
    fn test_lazy_elab_shared_type_system() {
        let elab = LazyElaborator::new();
        let ts1 = elab.shared_type_system();
        let ts2 = elab.shared_type_system();
        assert!(Arc::ptr_eq(&ts1, &ts2));
        // type_system() selalu mengembalikan referensi valid (tidak pernah None).
        let _ = elab.type_system();
    }
}
