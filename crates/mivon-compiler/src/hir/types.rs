//! Lazy Type Resolution System.
//!
//! Tipe di-resolve hanya saat pertama kali diakses (`resolve_type()`).
//! Hasil di-cache di `DashMap` untuk thread-safe concurrent access.
//!
//! TypeSystem mengkonversi AST `DataType` ke HIR `HirType`, termasuk
//! width resolution untuk typedef, enum, struct, dan user-defined types.

use std::sync::Arc;

use dashmap::DashMap;

use super::hir::*;
use mivon_ast::types::*;
use mivon_core::intern::Symbol;

/// Lazy type resolution system.
///
/// Cache: (module_name, type_name) → Arc<HirType>
/// Module-scoped agar lookup aman — tiap modul punya namespace sendiri.
pub struct TypeSystem {
    /// Primary type cache: (module, type_name) → resolved HirType
    type_cache: DashMap<(Symbol, Symbol), Arc<HirType>>,
    /// Package-scoped type cache: (package, type_name) → resolved HirType
    pkg_type_cache: DashMap<(Symbol, Symbol), Arc<HirType>>,
    /// Reference to package symbol table for cross-module type resolution
    package_symbols: Option<Arc<DashMap<Symbol, DashMap<Symbol, PackageItem>>>>,
}

impl TypeSystem {
    pub fn new() -> Self {
        TypeSystem {
            type_cache: DashMap::new(),
            pkg_type_cache: DashMap::new(),
            package_symbols: None,
        }
    }

    /// Set package symbols reference for cross-module type resolution.
    pub fn with_package_symbols(
        mut self,
        symbols: Arc<DashMap<Symbol, DashMap<Symbol, PackageItem>>>,
    ) -> Self {
        self.package_symbols = Some(symbols);
        self
    }

    /// Resolve a type name to HirType, lazily.
    ///
    /// Jika sudah di-cache, return clone. Jika belum, resolve dan cache.
    pub fn resolve_type(&self, module: Symbol, type_name: Symbol) -> HirType {
        // 1. Check cache
        let key = (module, type_name);
        if let Some(t) = self.type_cache.get(&key) {
            return t.as_ref().clone();
        }

        // 2. Check if it's being resolved concurrently (simple retry)
        // DashMap handles this internally — entry API will block if another
        // thread is writing to the same key.

        // 3. Resolve and cache
        let hir_type = self.resolve_type_impl(module, type_name);
        self.type_cache.insert(key, Arc::new(hir_type.clone()));
        hir_type
    }

    /// Resolve a type in a package scope.
    pub fn resolve_package_type(&self, pkg: Symbol, type_name: Symbol) -> Option<HirType> {
        let key = (pkg, type_name);
        if let Some(t) = self.pkg_type_cache.get(&key) {
            return Some(t.as_ref().clone());
        }

        // Try to resolve from package symbols
        if let Some(ref pkg_symbols) = self.package_symbols {
            if let Some(pkg_items) = pkg_symbols.get(&pkg) {
                if let Some(item) = pkg_items.get(&type_name) {
                    if let PackageItem::Typedef(td) = item.value() {
                        let hir_type = self.ast_dtype_to_hir(&td.dtype, pkg);
                        self.pkg_type_cache.insert(key, Arc::new(hir_type.clone()));
                        return Some(hir_type);
                    }
                }
            }
        }
        None
    }

    /// Actual type resolution logic — called once per (module, type_name).
    fn resolve_type_impl(&self, module: Symbol, type_name: Symbol) -> HirType {
        // Try built-in types
        if let Some(builtin) = Self::builtin_type(type_name) {
            return builtin;
        }

        // Try package-scoped types (look up all packages)
        if let Some(ref pkg_symbols) = self.package_symbols {
            for entry in pkg_symbols.iter() {
                let pkg_name = *entry.key();
                let pkg_items = entry.value();
                if let Some(item) = pkg_items.get(&type_name) {
                    if let PackageItem::Typedef(td) = item.value() {
                        let hir_type = self.ast_dtype_to_hir(&td.dtype, pkg_name);
                        self.pkg_type_cache
                            .insert((pkg_name, type_name), Arc::new(hir_type.clone()));
                        return hir_type;
                    }
                }
            }
        }

        // Not found — default to 32-bit (consistent with Elaborator behavior)
        eprintln!(
            "  ** WARNING: type '{}' not found in module '{}' (defaulting to 32-bit)",
            type_name, module
        );
        HirType::BitVec { width: 32 }
    }

    /// Convert AST DataType to HIR HirType.
    ///
    /// Handles width resolution for all type variants.
    pub fn ast_dtype_to_hir(&self, dtype: &DataType, scope: Symbol) -> HirType {
        match dtype {
            DataType::Void => HirType::Void,
            DataType::Bit => HirType::BitVec { width: 1 },
            DataType::Logic => HirType::BitVec { width: 1 },
            DataType::Int => HirType::Int { width: 32 },
            DataType::Integer => HirType::Int { width: 32 },
            DataType::Byte => HirType::Int { width: 8 },
            DataType::Shortint => HirType::Int { width: 16 },
            DataType::Longint => HirType::Int { width: 64 },
            DataType::Time => HirType::UInt { width: 64 },
            DataType::Real | DataType::Realtime => HirType::Real,
            DataType::String => HirType::String,
            DataType::Signed(inner) => {
                let inner_hir = self.ast_dtype_to_hir(inner, scope);
                HirType::Int {
                    width: inner_hir.width(),
                }
            }
            DataType::UserDefined(name) => {
                // Recursive resolution for user-defined types
                self.resolve_type(scope, *name)
            }
            DataType::EnumType { base, members } => {
                let base_hir = base
                    .as_ref()
                    .map(|b| self.ast_dtype_to_hir(b, scope))
                    .unwrap_or(HirType::Int { width: 32 });
                let variants: Vec<(Symbol, Option<u64>)> = members
                    .iter()
                    .map(|(name, opt_expr)| {
                        let val = opt_expr
                            .as_ref()
                            .and_then(|e| const_eval_simple(e).ok().map(|v| v as u64));
                        (*name, val)
                    })
                    .collect();
                HirType::Enum {
                    base: Box::new(base_hir),
                    variants,
                }
            }
            DataType::StructType { members } => {
                let fields: Vec<HirStructField> = members
                    .iter()
                    .map(|m| {
                        let w = m.range.as_ref().map(|r| r.width()).unwrap_or(1);
                        HirStructField {
                            name: m.name,
                            dtype: HirType::BitVec { width: w },
                            width: w,
                        }
                    })
                    .collect();
                HirType::Struct { fields }
            }
            DataType::UnionType { members } => {
                // Union: max width of all members
                let max_width = members
                    .iter()
                    .map(|m| m.range.as_ref().map(|r| r.width()).unwrap_or(1))
                    .max()
                    .unwrap_or(1);
                HirType::BitVec { width: max_width }
            }
        }
    }

    /// Map a built-in type name to HirType.
    fn builtin_type(name: Symbol) -> Option<HirType> {
        match name.as_str() {
            "bit" => Some(HirType::BitVec { width: 1 }),
            "logic" => Some(HirType::BitVec { width: 1 }),
            "int" => Some(HirType::Int { width: 32 }),
            "integer" => Some(HirType::Int { width: 32 }),
            "byte" => Some(HirType::Int { width: 8 }),
            "shortint" => Some(HirType::Int { width: 16 }),
            "longint" => Some(HirType::Int { width: 64 }),
            "time" => Some(HirType::UInt { width: 64 }),
            "real" | "realtime" => Some(HirType::Real),
            "string" => Some(HirType::String),
            "void" => Some(HirType::Void),
            _ => None,
        }
    }

    /// Pre-populate cache with known type definitions from a module or package.
    pub fn register_typedefs(&self, scope: Symbol, typedefs: &[TypedefDecl]) {
        for td in typedefs {
            let hir_type = self.ast_dtype_to_hir(&td.dtype, scope);
            self.type_cache.insert((scope, td.name), Arc::new(hir_type));
        }
    }

    /// Pre-populate cache with known type definitions from package items.
    pub fn register_package_items(&self, pkg_name: Symbol, items: &DashMap<Symbol, PackageItem>) {
        for entry in items.iter() {
            let name = *entry.key();
            if let PackageItem::Typedef(td) = entry.value() {
                let hir_type = self.ast_dtype_to_hir(&td.dtype, pkg_name);
                self.pkg_type_cache
                    .insert((pkg_name, name), Arc::new(hir_type));
            }
        }
    }

    /// Number of cached type entries.
    pub fn cache_len(&self) -> usize {
        self.type_cache.len() + self.pkg_type_cache.len()
    }

    /// Invalidate all cached types.
    pub fn invalidate_all(&self) {
        self.type_cache.clear();
        self.pkg_type_cache.clear();
    }

    /// Check if a type is already cached for a given module.
    pub fn is_cached(&self, module: Symbol, type_name: Symbol) -> bool {
        self.type_cache.contains_key(&(module, type_name))
            || self.pkg_type_cache.contains_key(&(module, type_name))
    }
}

impl Default for TypeSystem {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_typesystem_basic() {
        let ts = TypeSystem::new();
        let name = Symbol::intern("test_mod");

        // Built-in types resolve directly
        let t = ts.resolve_type(name, Symbol::intern("int"));
        assert_eq!(t, HirType::Int { width: 32 });

        let t = ts.resolve_type(name, Symbol::intern("bit"));
        assert_eq!(t, HirType::BitVec { width: 1 });

        let t = ts.resolve_type(name, Symbol::intern("real"));
        assert_eq!(t, HirType::Real);

        let t = ts.resolve_type(name, Symbol::intern("void"));
        assert_eq!(t, HirType::Void);
    }

    #[test]
    fn test_typesystem_unknown_defaults() {
        let ts = TypeSystem::new();
        let name = Symbol::intern("test_mod");

        // Unknown type defaults to 32-bit (consistent with Elaborator)
        let t = ts.resolve_type(name, Symbol::intern("nonexistent_type"));
        assert_eq!(t, HirType::BitVec { width: 32 });
    }

    #[test]
    fn test_typesystem_cache() {
        let ts = TypeSystem::new();
        let module = Symbol::intern("cache_test");

        assert!(!ts.is_cached(module, Symbol::intern("my_type")));

        ts.resolve_type(module, Symbol::intern("my_type"));
        // After resolution, it should be cached as unknown (1-bit)
        assert!(ts.is_cached(module, Symbol::intern("my_type")));
    }

    #[test]
    fn test_ast_dtype_conversion() {
        let ts = TypeSystem::new();
        let scope = Symbol::intern("test");

        // Basic types
        assert_eq!(
            ts.ast_dtype_to_hir(&DataType::Logic, scope),
            HirType::BitVec { width: 1 }
        );
        assert_eq!(
            ts.ast_dtype_to_hir(&DataType::Int, scope),
            HirType::Int { width: 32 }
        );
        assert_eq!(
            ts.ast_dtype_to_hir(&DataType::Byte, scope),
            HirType::Int { width: 8 }
        );
        assert_eq!(
            ts.ast_dtype_to_hir(&DataType::Longint, scope),
            HirType::Int { width: 64 }
        );
        assert_eq!(ts.ast_dtype_to_hir(&DataType::Real, scope), HirType::Real);
        assert_eq!(
            ts.ast_dtype_to_hir(&DataType::String, scope),
            HirType::String
        );
        assert_eq!(ts.ast_dtype_to_hir(&DataType::Void, scope), HirType::Void);
    }

    #[test]
    fn test_ast_enum_conversion() {
        let ts = TypeSystem::new();
        let scope = Symbol::intern("test");

        let enum_type = DataType::EnumType {
            base: Some(Box::new(DataType::Int)),
            members: vec![
                (
                    Symbol::intern("IDLE"),
                    Some(mivon_ast::expr::Expr::Value(
                        mivon_ast::expr::Value::Decimal(0),
                    )),
                ),
                (
                    Symbol::intern("BUSY"),
                    Some(mivon_ast::expr::Expr::Value(
                        mivon_ast::expr::Value::Decimal(1),
                    )),
                ),
                (Symbol::intern("DONE"), None),
            ],
        };

        let hir = ts.ast_dtype_to_hir(&enum_type, scope);
        match &hir {
            HirType::Enum { base, variants } => {
                assert_eq!(base.width(), 32);
                assert_eq!(variants.len(), 3);
                assert_eq!(variants[0], (Symbol::intern("IDLE"), Some(0)));
                assert_eq!(variants[1], (Symbol::intern("BUSY"), Some(1)));
                assert_eq!(variants[2], (Symbol::intern("DONE"), None));
            }
            _ => panic!("expected Enum type"),
        }
    }

    #[test]
    fn test_ast_struct_conversion() {
        let ts = TypeSystem::new();
        let scope = Symbol::intern("test");

        let struct_type = DataType::StructType {
            members: vec![
                StructMember {
                    name: Symbol::intern("addr"),
                    dtype: Box::new(DataType::Logic),
                    range: Some(mivon_ast::types::Range { msb: 31, lsb: 0 }),
                    expr_range: None,
                },
                StructMember {
                    name: Symbol::intern("data"),
                    dtype: Box::new(DataType::Logic),
                    range: Some(mivon_ast::types::Range { msb: 7, lsb: 0 }),
                    expr_range: None,
                },
            ],
        };

        let hir = ts.ast_dtype_to_hir(&struct_type, scope);
        match &hir {
            HirType::Struct { fields } => {
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].name, Symbol::intern("addr"));
                assert_eq!(fields[0].width, 32);
                assert_eq!(fields[1].name, Symbol::intern("data"));
                assert_eq!(fields[1].width, 8);
            }
            _ => panic!("expected Struct type"),
        }
    }

    #[test]
    fn test_typesystem_typedef_registration() {
        let ts = TypeSystem::new();
        let scope = Symbol::intern("my_pkg");

        let typedefs = vec![
            TypedefDecl {
                name: Symbol::intern("my_int"),
                dtype: DataType::Int,
                range: None,
                extra_packed_dims: vec![],
                is_queue: false,
                is_dynamic: false,
            },
            TypedefDecl {
                name: Symbol::intern("my_logic"),
                dtype: DataType::Logic,
                range: None,
                extra_packed_dims: vec![],
                is_queue: false,
                is_dynamic: false,
            },
        ];

        ts.register_typedefs(scope, &typedefs);

        assert_eq!(
            ts.resolve_type(scope, Symbol::intern("my_int")),
            HirType::Int { width: 32 }
        );
        assert_eq!(
            ts.resolve_type(scope, Symbol::intern("my_logic")),
            HirType::BitVec { width: 1 }
        );
    }

    #[test]
    fn test_typesystem_invalidate() {
        let ts = TypeSystem::new();
        let scope = Symbol::intern("test");

        ts.resolve_type(scope, Symbol::intern("some_type"));
        assert!(ts.is_cached(scope, Symbol::intern("some_type")));

        ts.invalidate_all();
        assert!(!ts.is_cached(scope, Symbol::intern("some_type")));
    }

    #[test]
    fn test_typesystem_cache_len() {
        let ts = TypeSystem::new();
        let scope = Symbol::intern("test");

        assert_eq!(ts.cache_len(), 0);
        ts.resolve_type(scope, Symbol::intern("type_a"));
        ts.resolve_type(scope, Symbol::intern("type_b"));
        assert_eq!(ts.cache_len(), 2);
    }

    #[test]
    fn test_ast_union_conversion() {
        let ts = TypeSystem::new();
        let scope = Symbol::intern("test");

        let union_type = DataType::UnionType {
            members: vec![
                StructMember {
                    name: Symbol::intern("byte_data"),
                    dtype: Box::new(DataType::Logic),
                    range: Some(mivon_ast::types::Range { msb: 7, lsb: 0 }),
                    expr_range: None,
                },
                StructMember {
                    name: Symbol::intern("word_data"),
                    dtype: Box::new(DataType::Logic),
                    range: Some(mivon_ast::types::Range { msb: 31, lsb: 0 }),
                    expr_range: None,
                },
            ],
        };

        let hir = ts.ast_dtype_to_hir(&union_type, scope);
        // Union width = max member width = 32
        assert_eq!(hir, HirType::BitVec { width: 32 });
    }
}
