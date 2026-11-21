//! JIT Compiler untuk ekspresi evaluasi.
//!
//! Menggunakan monomorphization + caching untuk kompilasi ekspresi
//! ke fungsi native. Cranelift integration direncanakan untuk versi
//! selanjutnya.
//!
//! Arsitektur:
//! - `CompiledExpr` menyimpan hasil kompilasi ekspresi
//! - `JITCache` meng-cache ekspresi yang sudah dikompilasi
//! - `JITCompiler` adalah entry point utama

use std::collections::HashMap;
use std::sync::Mutex;

/// Hasil kompilasi sebuah ekspresi.
#[derive(Clone)]
pub struct CompiledExpr {
    /// Nama ekspresi (untuk debugging)
    pub name: String,
    /// Fungsi yang dikompilasi (Arc<dyn Fn> — bisa capture variable)
    pub eval_fn: std::sync::Arc<dyn Fn(&[u64], &[u64]) -> u64 + Send + Sync>,
    /// Jumlah argumen yang dibutuhkan
    pub arg_count: usize,
    /// Ukuran output (dalam bit)
    pub width: usize,
    /// Hit counter
    pub hit_count: u64,
}

// Manual Debug implementation since Arc<dyn Fn> doesn't implement Debug
impl std::fmt::Debug for CompiledExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledExpr")
            .field("name", &self.name)
            .field("arg_count", &self.arg_count)
            .field("width", &self.width)
            .field("hit_count", &self.hit_count)
            .finish()
    }
}

/// Cache ekspresi yang sudah dikompilasi.
pub struct JITCache {
    compiled: Mutex<HashMap<u64, CompiledExpr>>,
    hits: Mutex<u64>,
    misses: Mutex<u64>,
}

impl JITCache {
    pub fn new() -> Self {
        JITCache {
            compiled: Mutex::new(HashMap::new()),
            hits: Mutex::new(0),
            misses: Mutex::new(0),
        }
    }

    /// Cari ekspresi di cache berdasarkan hash.
    pub fn lookup(&self, hash: u64) -> Option<CompiledExpr> {
        let cache = self.compiled.lock().unwrap();
        if let Some(expr) = cache.get(&hash) {
            let mut expr = expr.clone();
            expr.hit_count += 1;
            let mut hits = self.hits.lock().unwrap();
            *hits += 1;
            // Update cache entry with new hit count
            drop(cache);
            let mut cache = self.compiled.lock().unwrap();
            cache.insert(hash, expr.clone());
            Some(expr)
        } else {
            let mut misses = self.misses.lock().unwrap();
            *misses += 1;
            None
        }
    }

    /// Insert compiled expression ke cache.
    pub fn insert(&self, hash: u64, expr: CompiledExpr) {
        let mut cache = self.compiled.lock().unwrap();
        cache.insert(hash, expr);
    }

    /// Hit rate.
    pub fn hit_rate(&self) -> f64 {
        let hits = *self.hits.lock().unwrap();
        let misses = *self.misses.lock().unwrap();
        let total = hits + misses;
        if total == 0 {
            0.0
        } else {
            hits as f64 / total as f64
        }
    }

    /// Number of cached entries.
    pub fn len(&self) -> usize {
        self.compiled.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clear cache.
    pub fn clear(&self) {
        self.compiled.lock().unwrap().clear();
    }
}

impl Default for JITCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Built-in evaluator functions yang bisa dipanggil oleh JIT compiler.
pub mod intrinsics {
    /// Binary addition: a + b
    pub fn add(a: u64, b: u64) -> u64 {
        a.wrapping_add(b)
    }

    /// Binary subtraction: a - b
    pub fn sub(a: u64, b: u64) -> u64 {
        a.wrapping_sub(b)
    }

    /// Bitwise AND: a & b
    pub fn bit_and(a: u64, b: u64) -> u64 {
        a & b
    }

    /// Bitwise OR: a | b
    pub fn bit_or(a: u64, b: u64) -> u64 {
        a | b
    }

    /// Bitwise XOR: a ^ b
    pub fn bit_xor(a: u64, b: u64) -> u64 {
        a ^ b
    }

    /// Arithmetic: a * b
    pub fn mul(a: u64, b: u64) -> u64 {
        a.wrapping_mul(b)
    }

    /// Shift left: a << b
    pub fn shl(a: u64, b: u64) -> u64 {
        a.wrapping_shl(b as u32)
    }

    /// Shift right (logical): a >> b
    pub fn shr(a: u64, b: u64) -> u64 {
        a.wrapping_shr(b as u32)
    }

    /// Equality: a == b
    pub fn eq(a: u64, b: u64) -> u64 {
        if a == b {
            1
        } else {
            0
        }
    }

    /// Less-than: a < b
    pub fn lt(a: u64, b: u64) -> u64 {
        if a < b {
            1
        } else {
            0
        }
    }
}

/// JIT Compiler — mengkompilasi ekspresi menjadi fungsi native.
pub struct JITCompiler {
    compiled_count: usize,
    cache: JITCache,
}

impl JITCompiler {
    pub fn new() -> Result<Self, String> {
        Ok(JITCompiler {
            compiled_count: 0,
            cache: JITCache::new(),
        })
    }

    /// Compile a binary expression into a native function.
    /// Returns a compiled expression descriptor.
    pub fn compile_binary(
        &mut self,
        op: &str,
        lhs: &CompiledExpr,
        rhs: &CompiledExpr,
        width: usize,
    ) -> CompiledExpr {
        // Generate a hash from the expression
        let hash = self.compute_hash(op, lhs, rhs, width);

        // Check cache
        if let Some(cached) = self.cache.lookup(hash) {
            return cached;
        }

        // Select the appropriate function (as Arc-wrapped closure)
        let eval_fn: std::sync::Arc<dyn Fn(&[u64], &[u64]) -> u64 + Send + Sync> = match op {
            "+" => std::sync::Arc::new(|args, _| intrinsics::add(args[0], args[1])),
            "-" => std::sync::Arc::new(|args, _| intrinsics::sub(args[0], args[1])),
            "*" => std::sync::Arc::new(|args, _| intrinsics::mul(args[0], args[1])),
            "&" => std::sync::Arc::new(|args, _| intrinsics::bit_and(args[0], args[1])),
            "|" => std::sync::Arc::new(|args, _| intrinsics::bit_or(args[0], args[1])),
            "^" => std::sync::Arc::new(|args, _| intrinsics::bit_xor(args[0], args[1])),
            "==" => std::sync::Arc::new(|args, _| intrinsics::eq(args[0], args[1])),
            "<" => std::sync::Arc::new(|args, _| intrinsics::lt(args[0], args[1])),
            "<<" => std::sync::Arc::new(|args, _| intrinsics::shl(args[0], args[1])),
            ">>" => std::sync::Arc::new(|args, _| intrinsics::shr(args[0], args[1])),
            _ => std::sync::Arc::new(|_, _| 0), // fallback
        };

        let expr = CompiledExpr {
            name: format!("{}_{}_{}", op, lhs.name, rhs.name),
            eval_fn,
            arg_count: 2,
            width,
            hit_count: 0,
        };

        self.compiled_count += 1;
        self.cache.insert(hash, expr.clone());
        expr
    }

    /// Compile a unary expression.
    pub fn compile_unary(
        &mut self,
        op: &str,
        operand: &CompiledExpr,
        width: usize,
    ) -> CompiledExpr {
        let dummy = CompiledExpr {
            name: String::new(),
            eval_fn: std::sync::Arc::new(|_, _| 0),
            arg_count: 0,
            width: 0,
            hit_count: 0,
        };
        let hash = self.compute_hash(op, operand, &dummy, width);

        if let Some(cached) = self.cache.lookup(hash) {
            return cached;
        }

        let eval_fn: std::sync::Arc<dyn Fn(&[u64], &[u64]) -> u64 + Send + Sync> = match op {
            "~" => std::sync::Arc::new(|args, _| !args[0]),
            "-" => std::sync::Arc::new(|args, _| args[0].wrapping_neg()),
            "!" => std::sync::Arc::new(|args, _| if args[0] == 0 { 1 } else { 0 }),
            _ => std::sync::Arc::new(|args, _| args[0]),
        };

        let expr = CompiledExpr {
            name: format!("{}_{}", op, operand.name),
            eval_fn,
            arg_count: 1,
            width,
            hit_count: 0,
        };

        self.compiled_count += 1;
        self.cache.insert(hash, expr.clone());
        expr
    }

    /// Create a compiled expression for a constant value.
    pub fn compile_const(&mut self, value: u64, width: usize) -> CompiledExpr {
        let eval_fn: std::sync::Arc<dyn Fn(&[u64], &[u64]) -> u64 + Send + Sync> =
            std::sync::Arc::new(move |_, _| value);
        let expr = CompiledExpr {
            name: format!("const_{}", value),
            eval_fn,
            arg_count: 0,
            width,
            hit_count: 0,
        };
        expr
    }

    fn compute_hash(&self, op: &str, lhs: &CompiledExpr, rhs: &CompiledExpr, width: usize) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        op.hash(&mut hasher);
        lhs.name.hash(&mut hasher);
        rhs.name.hash(&mut hasher);
        width.hash(&mut hasher);
        hasher.finish()
    }

    pub fn compiled_count(&self) -> usize {
        self.compiled_count
    }

    /// Get cache statistics.
    pub fn cache_stats(&self) -> (usize, f64) {
        (self.cache.len(), self.cache.hit_rate())
    }
}

impl Default for JITCompiler {
    fn default() -> Self {
        Self::new().expect("JITCompiler::new failed")
    }
}

// ─── JIT-01 Stage 1: Statement-level JIT infrastructure ───
// CompiledStmt + JITStmtCache + JITStmtCompiler mengizinkan kompilasi
// statement (blocking assign tanpa delay) ke native function dengan
// side-effect (tulis signal). Stage 1: infrastructure + simple blocking assign.
// Stage 2+: if/case/loop/delay statement compilation.

/// Hasil kompilasi sebuah statement.
#[derive(Clone)]
pub struct CompiledStmt {
    /// Nama statement (debugging)
    pub name: String,
    /// Fungsi eksekusi: baca input signal values, tulis output signal values,
    /// kembalikan true bila ada perubahan.
    pub exec_fn: std::sync::Arc<
        dyn Fn(&[u64], &mut [u64]) -> bool + Send + Sync,
    >,
    /// Signal ID yang dibaca (input)
    pub read_ids: Vec<usize>,
    /// Signal ID yang ditulis (output)
    pub write_ids: Vec<usize>,
    /// Hit counter
    pub hit_count: u64,
}

impl std::fmt::Debug for CompiledStmt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledStmt")
            .field("name", &self.name)
            .field("read_ids", &self.read_ids)
            .field("write_ids", &self.write_ids)
            .field("hit_count", &self.hit_count)
            .finish()
    }
}

/// Cache statement yang sudah dikompilasi.
pub struct JITStmtCache {
    compiled: std::sync::Mutex<std::collections::HashMap<u64, CompiledStmt>>,
    hits: std::sync::Mutex<u64>,
    misses: std::sync::Mutex<u64>,
}

impl JITStmtCache {
    pub fn new() -> Self {
        JITStmtCache {
            compiled: std::sync::Mutex::new(std::collections::HashMap::new()),
            hits: std::sync::Mutex::new(0),
            misses: std::sync::Mutex::new(0),
        }
    }

    pub fn lookup(&self, hash: u64) -> Option<CompiledStmt> {
        let cache = self.compiled.lock().unwrap();
        if let Some(stmt) = cache.get(&hash) {
            let mut stmt = stmt.clone();
            stmt.hit_count += 1;
            let mut hits = self.hits.lock().unwrap();
            *hits += 1;
            drop(cache);
            let mut cache = self.compiled.lock().unwrap();
            cache.insert(hash, stmt.clone());
            Some(stmt)
        } else {
            let mut misses = self.misses.lock().unwrap();
            *misses += 1;
            None
        }
    }

    pub fn insert(&self, hash: u64, stmt: CompiledStmt) {
        let mut cache = self.compiled.lock().unwrap();
        cache.insert(hash, stmt);
    }

    pub fn len(&self) -> usize {
        self.compiled.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&self) {
        self.compiled.lock().unwrap().clear();
    }

    /// Hit rate.
    pub fn hit_rate(&self) -> f64 {
        let hits = *self.hits.lock().unwrap();
        let misses = *self.misses.lock().unwrap();
        let total = hits + misses;
        if total == 0 {
            0.0
        } else {
            hits as f64 / total as f64
        }
    }
}

impl Default for JITStmtCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Compiler untuk statement-level JIT.
pub struct JITStmtCompiler {
    compiled_count: usize,
    cache: JITStmtCache,
}

impl JITStmtCompiler {
    pub fn new() -> Self {
        JITStmtCompiler {
            compiled_count: 0,
            cache: JITStmtCache::new(),
        }
    }

    /// Compile blocking assignment `signal[write_id] = expr` (tanpa delay).
    /// `read_expr` adalah CompiledExpr untuk RHS yang menerima input signal
    /// values dan mengembalikan u64 hasil.
    pub fn compile_blocking_assign(
        &mut self,
        read_ids: Vec<usize>,
        write_id: usize,
        read_expr: &CompiledExpr,
    ) -> CompiledStmt {
        let hash = self.compute_stmt_hash(&read_ids, write_id, read_expr);

        if let Some(cached) = self.cache.lookup(hash) {
            return cached;
        }

        let eval_fn = read_expr.eval_fn.clone();
        let write_id_c = write_id;

        let exec_fn: std::sync::Arc<
            dyn Fn(&[u64], &mut [u64]) -> bool + Send + Sync,
        > = std::sync::Arc::new(move |inputs, outputs| {
            let result = (eval_fn)(inputs, &[]);
            if outputs.len() > write_id_c {
                outputs[write_id_c] = result;
                true
            } else {
                false
            }
        });

        let stmt = CompiledStmt {
            name: format!("blocking_assign_{}", write_id),
            exec_fn,
            read_ids,
            write_ids: vec![write_id],
            hit_count: 0,
        };

        self.compiled_count += 1;
        self.cache.insert(hash, stmt.clone());
        stmt
    }

    fn compute_stmt_hash(
        &self,
        read_ids: &[usize],
        write_id: usize,
        read_expr: &CompiledExpr,
    ) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        write_id.hash(&mut hasher);
        for &id in read_ids {
            id.hash(&mut hasher);
        }
        read_expr.name.hash(&mut hasher);
        read_expr.width.hash(&mut hasher);
        hasher.finish()
    }

    pub fn compiled_count(&self) -> usize {
        self.compiled_count
    }

    pub fn cache_stats(&self) -> (usize, f64) {
        (self.cache.len(), self.cache.hit_rate())
    }
}

impl Default for JITStmtCompiler {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ───
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jit_compiler_create() {
        let compiler = JITCompiler::new().unwrap();
        assert_eq!(compiler.compiled_count(), 0);
    }

    #[test]
    fn test_jit_const() {
        let mut compiler = JITCompiler::new().unwrap();
        let c = compiler.compile_const(42, 32);
        assert_eq!(c.width, 32);
        assert_eq!((c.eval_fn)(&[], &[]), 42);
    }

    #[test]
    fn test_jit_binary_add() {
        let mut compiler = JITCompiler::new().unwrap();
        let lhs = compiler.compile_const(10, 32);
        let rhs = compiler.compile_const(20, 32);
        let expr = compiler.compile_binary("+", &lhs, &rhs, 32);
        let result = (expr.eval_fn)(&[10, 20], &[]);
        assert_eq!(result, 30, "10 + 20 should be 30");
    }

    #[test]
    fn test_jit_binary_and() {
        let mut compiler = JITCompiler::new().unwrap();
        let lhs = compiler.compile_const(0xFF, 8);
        let rhs = compiler.compile_const(0x0F, 8);
        let expr = compiler.compile_binary("&", &lhs, &rhs, 8);
        let result = (expr.eval_fn)(&[0xFF, 0x0F], &[]);
        assert_eq!(result, 0x0F);
    }

    #[test]
    fn test_jit_binary_eq() {
        let mut compiler = JITCompiler::new().unwrap();
        let lhs = compiler.compile_const(5, 32);
        let rhs = compiler.compile_const(5, 32);
        let expr = compiler.compile_binary("==", &lhs, &rhs, 1);
        let result = (expr.eval_fn)(&[5, 5], &[]);
        assert_eq!(result, 1, "5 == 5 should be 1");
    }

    #[test]
    fn test_jit_binary_lt() {
        let mut compiler = JITCompiler::new().unwrap();
        let lhs = compiler.compile_const(3, 32);
        let rhs = compiler.compile_const(7, 32);
        let expr = compiler.compile_binary("<", &lhs, &rhs, 1);
        let result = (expr.eval_fn)(&[3, 7], &[]);
        assert_eq!(result, 1, "3 < 7 should be 1");
    }

    #[test]
    fn test_jit_unary_not() {
        let mut compiler = JITCompiler::new().unwrap();
        let op = compiler.compile_const(0xFF, 8);
        let expr = compiler.compile_unary("~", &op, 8);
        let result = (expr.eval_fn)(&[0xFF], &[]);
        assert_eq!(result, 0xFFFFFFFFFFFFFF00, "~0xFF for u64");
    }

    #[test]
    fn test_jit_cache() {
        let mut compiler = JITCompiler::new().unwrap();
        let lhs = compiler.compile_const(1, 32);
        let rhs = compiler.compile_const(2, 32);
        let _ = compiler.compile_binary("+", &lhs, &rhs, 32);
        let _ = compiler.compile_binary("+", &lhs, &rhs, 32); // should hit cache
        let (cached, hit_rate) = compiler.cache_stats();
        assert!(cached >= 1);
        assert!(hit_rate > 0.0);
    }

    #[test]
    fn test_jit_cache_hit_rate() {
        let mut compiler = JITCompiler::new().unwrap();
        let lhs = compiler.compile_const(10, 32);
        let rhs = compiler.compile_const(20, 32);
        let _ = compiler.compile_binary("+", &lhs, &rhs, 32);
        let _ = compiler.compile_binary("+", &lhs, &rhs, 32);
        let (_, hit_rate) = compiler.cache_stats();
        assert!(hit_rate > 0.0, "cache hit rate should be > 0");
    }

    // ─── JIT-01 Stage 1 tests ───

    #[test]
    fn test_jit_stmt_compiler_create() {
        let compiler = JITStmtCompiler::new();
        assert_eq!(compiler.compiled_count(), 0);
    }

    #[test]
    fn test_jit_stmt_blocking_assign_exec() {
        let mut stmt_compiler = JITStmtCompiler::new();
        let mut expr_compiler = JITCompiler::new().unwrap();

        let c = expr_compiler.compile_const(42, 32);
        let stmt = stmt_compiler.compile_blocking_assign(vec![], 0, &c);

        let inputs = vec![];
        let mut outputs = vec![0u64; 1];
        let changed = (stmt.exec_fn)(&inputs, &mut outputs);
        assert!(changed, "blocking assign should report change");
        assert_eq!(outputs[0], 42, "signal 0 should be 42");
    }

    #[test]
    fn test_jit_stmt_blocking_assign_with_input() {
        let mut stmt_compiler = JITStmtCompiler::new();
        let mut expr_compiler = JITCompiler::new().unwrap();

        let lhs = expr_compiler.compile_const(10, 32);
        let rhs = expr_compiler.compile_const(20, 32);
        let add_expr = expr_compiler.compile_binary("+", &lhs, &rhs, 32);

        // read_ids = [0, 1] (two input signals), write_id = 0
        let stmt = stmt_compiler.compile_blocking_assign(vec![0, 1], 0, &add_expr);

        let inputs = vec![10u64, 20u64];
        let mut outputs = vec![0u64; 1];
        let changed = (stmt.exec_fn)(&inputs, &mut outputs);
        assert!(changed);
        assert_eq!(outputs[0], 30, "10 + 20 should be 30");
    }

    #[test]
    fn test_jit_stmt_cache() {
        let mut stmt_compiler = JITStmtCompiler::new();
        let mut expr_compiler = JITCompiler::new().unwrap();

        let c = expr_compiler.compile_const(5, 32);
        let _ = stmt_compiler.compile_blocking_assign(vec![], 0, &c);
        let _ = stmt_compiler.compile_blocking_assign(vec![], 0, &c); // cache hit

        let (cached, hit_rate) = stmt_compiler.cache_stats();
        assert!(cached >= 1);
        assert!(hit_rate > 0.0);
    }

    #[test]
    fn test_jit_stmt_no_change_when_output_too_small() {
        let mut stmt_compiler = JITStmtCompiler::new();
        let mut expr_compiler = JITCompiler::new().unwrap();

        let c = expr_compiler.compile_const(99, 32);
        let stmt = stmt_compiler.compile_blocking_assign(vec![], 5, &c);

        let inputs = vec![];
        let mut outputs = vec![0u64; 1]; // only 1 slot, write_id=5 is OOB
        let changed = (stmt.exec_fn)(&inputs, &mut outputs);
        assert!(!changed, "should not change when output too small");
    }
}
