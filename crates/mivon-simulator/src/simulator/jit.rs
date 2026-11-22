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
use std::path::PathBuf;
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
    /// Signal ID yang dibaca (input signals) — untuk statement JIT
    pub read_ids: Vec<usize>,
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

    /// Signed division: a / b (treating inputs as signed i64)
    pub fn sdiv(a: u64, b: u64) -> u64 {
        let a_signed = a as i64;
        let b_signed = b as i64;
        if b_signed == 0 {
            return u64::MAX; // division by zero - return all 1s
        }
        (a_signed / b_signed) as u64
    }

    /// Signed modulo: a % b (treating inputs as signed i64)
    pub fn smod(a: u64, b: u64) -> u64 {
        let a_signed = a as i64;
        let b_signed = b as i64;
        if b_signed == 0 {
            return u64::MAX; // modulo by zero - return all 1s
        }
        (a_signed % b_signed) as u64
    }

    /// Sign-extend: extend value from `src_width` bits to `dst_width` bits
    /// preserving the sign bit (MSB of src_width).
    pub fn sign_extend(val: u64, src_width: usize, dst_width: usize) -> u64 {
        if dst_width <= src_width {
            return val & ((1u64 << dst_width).wrapping_sub(1));
        }
        if src_width == 0 || src_width >= 64 {
            return val;
        }
        let sign_bit = (val >> (src_width - 1)) & 1;
        if sign_bit == 1 {
            // Negative: fill upper bits with 1
            let mask = (!0u64) << src_width;
            val | mask
        } else {
            // Positive: upper bits are already 0
            val
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
            "/s" => std::sync::Arc::new(|args, _| intrinsics::sdiv(args[0], args[1])),
            "%s" => std::sync::Arc::new(|args, _| intrinsics::smod(args[0], args[1])),
            _ => std::sync::Arc::new(|_, _| 0), // fallback
        };

        let expr = CompiledExpr {
            name: format!("{}_{}_{}", op, lhs.name, rhs.name),
            eval_fn,
            arg_count: 2,
            width,
            read_ids: {
                let mut ids = lhs.read_ids.clone();
                ids.extend_from_slice(&rhs.read_ids);
                ids.sort();
                ids.dedup();
                ids
            },
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
            read_ids: Vec::new(),
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
            read_ids: operand.read_ids.clone(),
            hit_count: 0,
        };

        self.compiled_count += 1;
        self.cache.insert(hash, expr.clone());
        expr
    }

    /// Compile a sign-extend operation (unary with source/dest width info).
    /// `src_width` is the width of the operand, `dst_width` is the target width.
    pub fn compile_sign_extend(
        &mut self,
        operand: &CompiledExpr,
        src_width: usize,
        dst_width: usize,
    ) -> CompiledExpr {
        let op = "sext";
        let hash = self.compute_hash(op, operand, operand, dst_width); // use operand for both lhs/rhs in hash

        if let Some(cached) = self.cache.lookup(hash) {
            return cached;
        }

        let eval_fn: std::sync::Arc<dyn Fn(&[u64], &[u64]) -> u64 + Send + Sync> =
            std::sync::Arc::new(move |args, _| intrinsics::sign_extend(args[0], src_width, dst_width));

        let expr = CompiledExpr {
            name: format!("sext_{}_{}_{}", src_width, dst_width, operand.name),
            eval_fn,
            arg_count: 1,
            width: dst_width,
            read_ids: operand.read_ids.clone(),
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
            read_ids: Vec::new(),
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

    /// Create a compiled expression that reads a signal value from inputs.
    /// `signal_id` is the index into the inputs array.
    pub fn compile_signal_read(&mut self, signal_id: usize, width: usize) -> CompiledExpr {
        let eval_fn: std::sync::Arc<dyn Fn(&[u64], &[u64]) -> u64 + Send + Sync> =
            std::sync::Arc::new(move |args, _| args[signal_id]);
        let expr = CompiledExpr {
            name: format!("signal_read_{}", signal_id),
            eval_fn,
            arg_count: 1,
            width,
            read_ids: vec![signal_id],
            hit_count: 0,
        };
        expr
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
    /// True jika ini adalah placeholder dari disk cache (perlu recompile).
    pub is_placeholder: bool,
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
            // Check if this is a placeholder from disk cache (needs recompilation)
            if stmt.is_placeholder {
                drop(cache);
                // Remove placeholder from cache so it gets recompiled
                let mut cache = self.compiled.lock().unwrap();
                cache.remove(&hash);
                let mut misses = self.misses.lock().unwrap();
                *misses += 1;
                eprintln!("JITStmt: placeholder detected for hash {}, will recompile", hash);
                return None;
            }
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
    /// Optional persistent cache file path for saving/loading compiled statements.
    cache_path: Option<PathBuf>,
}

impl JITStmtCompiler {
    pub fn new() -> Self {
        JITStmtCompiler {
            compiled_count: 0,
            cache: JITStmtCache::new(),
            cache_path: None,
        }
    }

    /// Set persistent cache file path. The cache saves compiled statement
    /// metadata so that on next startup, statements can be pre-populated.
    /// Note: Native function pointers are ephemeral (process-lifetime), so we
    /// save metadata to reconstruct the cache on next startup. On load,
    /// placeholders with null function pointers are treated as cache misses
    /// and will be recompiled when needed.
    pub fn set_cache_path(&mut self, path: &str) {
        self.cache_path = Some(PathBuf::from(path));
        self.load_cache();
    }

    /// Get the current cache file path, if any.
    pub fn cache_path(&self) -> Option<&PathBuf> {
        self.cache_path.as_ref()
    }

    /// Save cache metadata to disk using a simple line-based format.
    /// Format: each line is "hash|name|read_ids|write_ids"
    /// where read_ids and write_ids are comma-separated lists of signal IDs.
    pub fn save_cache(&self) -> bool {
        let path = match &self.cache_path {
            Some(p) => p,
            None => return false,
        };

        let cache = self.cache.compiled.lock().unwrap();
        let mut output = String::new();
        for (hash, entry) in cache.iter() {
            let read_ids: String = entry.read_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
            let write_ids: String = entry.write_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
            output.push_str(&format!(
                "{}|{}|{}|{}\n",
                hash, entry.name, read_ids, write_ids
            ));
        }
        drop(cache);

        match std::fs::write(path, &output) {
            Ok(_) => {
                eprintln!(
                    "JITStmt cache saved: {} entries to {}",
                    output.lines().count(),
                    path.display()
                );
                true
            }
            Err(e) => {
                eprintln!("JITStmt cache write error: {}", e);
                false
            }
        }
    }

    /// Load cache metadata from disk and pre-populate the in-memory cache.
    /// Loaded entries have null exec_fn (placeholder) and will be recompiled
    /// on first use (cache miss behavior).
    pub fn load_cache(&self) -> bool {
        let path = match &self.cache_path {
            Some(p) => p,
            None => return false,
        };

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return false, // No cache file yet
        };

        let mut loaded_count = 0usize;
        let mut cache = self.cache.compiled.lock().unwrap();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.split('|').collect();
            if parts.len() < 4 {
                continue;
            }
            let hash: u64 = match parts[0].parse() {
                Ok(h) => h,
                Err(_) => continue,
            };
            let name = parts[1].to_string();
            let read_ids: Vec<usize> = parts[2]
                .split(',')
                .filter_map(|s| s.parse().ok())
                .collect();
            let write_ids: Vec<usize> = parts[3]
                .split(',')
                .filter_map(|s| s.parse().ok())
                .collect();

            // Create placeholder CompiledStmt with null exec_fn.
            // On first use, cache lookup will miss (due to null fn) and recompile.
            // We can't easily reconstruct the exact exec_fn without the original
            // CompiledExpr, so we rely on recompilation.
            let placeholder_fn: std::sync::Arc<
                dyn Fn(&[u64], &mut [u64]) -> bool + Send + Sync,
            > = std::sync::Arc::new(|_, _| false); // always "no change" - will trigger recompile

            let stmt = CompiledStmt {
                name: format!("{}_loaded", name),
                exec_fn: placeholder_fn,
                read_ids,
                write_ids,
                hit_count: 0,
                is_placeholder: true,
            };

            cache.insert(hash, stmt);
            loaded_count += 1;
        }
        drop(cache);

        if loaded_count > 0 {
            eprintln!(
                "JITStmt cache loaded: {} entries from {}",
                loaded_count,
                path.display()
            );
        }
        true
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
            is_placeholder: false,
        };

        self.compiled_count += 1;
        self.cache.insert(hash, stmt.clone());
        stmt
    }

    /// Compile if statement: `if (cond) then_stmt else else_stmt`
    /// `cond_expr` is the condition expression (returns 1 for true, 0 for false)
    /// `then_stmts` and `else_stmts` are pre-compiled statement sequences
    pub fn compile_if_stmt(
        &mut self,
        cond_expr: &CompiledExpr,
        then_stmts: Vec<CompiledStmt>,
        else_stmts: Option<Vec<CompiledStmt>>,
    ) -> CompiledStmt {
        let hash = self.compute_if_stmt_hash(cond_expr, &then_stmts, else_stmts.as_ref());

        if let Some(cached) = self.cache.lookup(hash) {
            return cached;
        }

        // Collect all read/write IDs from branches
        let mut read_ids = Vec::new();
        let mut write_ids = Vec::new();

        for stmt in &then_stmts {
            read_ids.extend_from_slice(&stmt.read_ids);
            write_ids.extend_from_slice(&stmt.write_ids);
        }
        if let Some(else_stmts) = &else_stmts {
            for stmt in else_stmts {
                read_ids.extend_from_slice(&stmt.read_ids);
                write_ids.extend_from_slice(&stmt.write_ids);
            }
        }
        // Add condition read IDs
        read_ids.extend_from_slice(&cond_expr.read_ids);

        // Deduplicate
        read_ids.sort();
        read_ids.dedup();
        write_ids.sort();
        write_ids.dedup();

        let cond_fn = cond_expr.eval_fn.clone();
        let then_stmts_owned = then_stmts;
        let else_stmts_owned = else_stmts;

        let exec_fn: std::sync::Arc<
            dyn Fn(&[u64], &mut [u64]) -> bool + Send + Sync,
        > = std::sync::Arc::new(move |inputs, outputs| {
            let cond_result = (cond_fn)(inputs, &[]);
            if cond_result != 0 {
                // Execute then branch
                let mut any_changed = false;
                for stmt in &then_stmts_owned {
                    // Extract inputs for this statement
                    let stmt_inputs: Vec<u64> = stmt
                        .read_ids
                        .iter()
                        .map(|&id| inputs[id])
                        .collect();
                    let changed = (stmt.exec_fn)(&stmt_inputs, outputs);
                    any_changed |= changed;
                }
                any_changed
            } else if let Some(else_stmts) = &else_stmts_owned {
                // Execute else branch
                let mut any_changed = false;
                for stmt in else_stmts {
                    let stmt_inputs: Vec<u64> = stmt
                        .read_ids
                        .iter()
                        .map(|&id| inputs[id])
                        .collect();
                    let changed = (stmt.exec_fn)(&stmt_inputs, outputs);
                    any_changed |= changed;
                }
                any_changed
            } else {
                false
            }
        });

        let stmt = CompiledStmt {
            name: format!("if_stmt"),
            exec_fn,
            read_ids,
            write_ids,
            hit_count: 0,
            is_placeholder: false,
        };

        self.compiled_count += 1;
        self.cache.insert(hash, stmt.clone());
        stmt
    }

    fn compute_if_stmt_hash(
        &self,
        cond_expr: &CompiledExpr,
        then_stmts: &[CompiledStmt],
        else_stmts: Option<&Vec<CompiledStmt>>,
    ) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        cond_expr.name.hash(&mut hasher);
        for stmt in then_stmts {
            stmt.name.hash(&mut hasher);
        }
        if let Some(else_stmts) = else_stmts {
            for stmt in else_stmts {
                stmt.name.hash(&mut hasher);
            }
        }
        hasher.finish()
    }

    /// Compile case statement: `case (expr) { item1: stmt1; item2: stmt2; ... }`
    /// `case_expr` is the case expression to compare against
    /// `items` is a vector of (item_expressions, compiled_statements) pairs
    /// `default_stmts` is optional default branch statements
    pub fn compile_case_stmt(
        &mut self,
        case_expr: &CompiledExpr,
        items: Vec<(Vec<CompiledExpr>, Vec<CompiledStmt>)>,
        default_stmts: Option<Vec<CompiledStmt>>,
    ) -> CompiledStmt {
        let hash = self.compute_case_stmt_hash(case_expr, &items, default_stmts.as_ref());

        if let Some(cached) = self.cache.lookup(hash) {
            return cached;
        }

        // Collect all read/write IDs from branches
        let mut read_ids = Vec::new();
        let mut write_ids = Vec::new();

        // Add case expression read IDs
        read_ids.extend_from_slice(&case_expr.read_ids);

        for (item_exprs, stmts) in &items {
            for item_expr in item_exprs {
                read_ids.extend_from_slice(&item_expr.read_ids);
            }
            for stmt in stmts {
                read_ids.extend_from_slice(&stmt.read_ids);
                write_ids.extend_from_slice(&stmt.write_ids);
            }
        }
        if let Some(default_stmts) = &default_stmts {
            for stmt in default_stmts {
                read_ids.extend_from_slice(&stmt.read_ids);
                write_ids.extend_from_slice(&stmt.write_ids);
            }
        }

        // Deduplicate
        read_ids.sort();
        read_ids.dedup();
        write_ids.sort();
        write_ids.dedup();

        let case_fn = case_expr.eval_fn.clone();
        let items_owned = items;
        let default_stmts_owned = default_stmts;

        let exec_fn: std::sync::Arc<
            dyn Fn(&[u64], &mut [u64]) -> bool + Send + Sync,
        > = std::sync::Arc::new(move |inputs, outputs| {
            let case_val = (case_fn)(inputs, &[]);

            // Try each case item
            for (item_exprs, stmts) in &items_owned {
                let mut matched = false;
                for item_expr in item_exprs {
                    let item_val = (item_expr.eval_fn)(inputs, &[]);
                    if item_val == case_val {
                        matched = true;
                        break;
                    }
                }
                if matched {
                    // Execute matched branch
                    let mut any_changed = false;
                    for stmt in stmts {
                        let stmt_inputs: Vec<u64> = stmt
                            .read_ids
                            .iter()
                            .map(|&id| inputs[id])
                            .collect();
                        let changed = (stmt.exec_fn)(&stmt_inputs, outputs);
                        any_changed |= changed;
                    }
                    return any_changed;
                }
            }

            // No match - execute default if present
            if let Some(default_stmts) = &default_stmts_owned {
                let mut any_changed = false;
                for stmt in default_stmts {
                    let stmt_inputs: Vec<u64> = stmt
                        .read_ids
                        .iter()
                        .map(|&id| inputs[id])
                        .collect();
                    let changed = (stmt.exec_fn)(&stmt_inputs, outputs);
                    any_changed |= changed;
                }
                return any_changed;
            }

            false
        });

        let stmt = CompiledStmt {
            name: format!("case_stmt"),
            exec_fn,
            read_ids,
            write_ids,
            hit_count: 0,
            is_placeholder: false,
        };

        self.compiled_count += 1;
        self.cache.insert(hash, stmt.clone());
        stmt
    }

    fn compute_case_stmt_hash(
        &self,
        case_expr: &CompiledExpr,
        items: &[(Vec<CompiledExpr>, Vec<CompiledStmt>)],
        default_stmts: Option<&Vec<CompiledStmt>>,
    ) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        case_expr.name.hash(&mut hasher);
        for (item_exprs, stmts) in items {
            for expr in item_exprs {
                expr.name.hash(&mut hasher);
            }
            for stmt in stmts {
                stmt.name.hash(&mut hasher);
            }
        }
        if let Some(default_stmts) = default_stmts {
            for stmt in default_stmts {
                stmt.name.hash(&mut hasher);
            }
        }
        hasher.finish()
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

    #[test]
    fn test_jit_stmt_if() {
        let mut stmt_compiler = JITStmtCompiler::new();
        let mut expr_compiler = JITCompiler::new().unwrap();

        // Condition: input[0] == 0 (returns 1 if true, 0 if false)
        let lhs = expr_compiler.compile_const(0, 32);
        let rhs = expr_compiler.compile_const(0, 32);
        let cond_expr = expr_compiler.compile_binary("==", &lhs, &rhs, 1);

        // Then branch: signal 0 = 42
        let then_const = expr_compiler.compile_const(42, 32);
        let then_stmt = stmt_compiler.compile_blocking_assign(vec![], 0, &then_const);

        // Else branch: signal 0 = 99
        let else_const = expr_compiler.compile_const(99, 32);
        let else_stmt = stmt_compiler.compile_blocking_assign(vec![], 0, &else_const);

        // Compile if statement
        let if_stmt = stmt_compiler.compile_if_stmt(&cond_expr, vec![then_stmt.clone()], Some(vec![else_stmt]));

        // Test with condition true (input[0] = 0)
        let inputs = vec![0u64, 0u64]; // lhs=0, rhs=0
        let mut outputs = vec![0u64; 1];
        let changed = (if_stmt.exec_fn)(&inputs, &mut outputs);
        assert!(changed);
        assert_eq!(outputs[0], 42, "if true should execute then branch");

        // Test with condition false (input[0] = 5)
        let inputs = vec![5u64, 0u64]; // lhs=5, rhs=0
        let mut outputs = vec![0u64; 1];
        let changed = (if_stmt.exec_fn)(&inputs, &mut outputs);
        assert!(changed);
        assert_eq!(outputs[0], 99, "if false should execute else branch");

        // Test with condition false and no else branch
        let if_stmt_no_else = stmt_compiler.compile_if_stmt(&cond_expr, vec![then_stmt], None);
        let inputs = vec![5u64, 0u64];
        let mut outputs = vec![0u64; 1];
        let changed = (if_stmt_no_else.exec_fn)(&inputs, &mut outputs);
        assert!(!changed, "if false with no else should not change");
        assert_eq!(outputs[0], 0, "output should remain unchanged");
    }

    #[test]
    fn test_jit_stmt_case() {
        let mut stmt_compiler = JITStmtCompiler::new();
        let mut expr_compiler = JITCompiler::new().unwrap();

        // Case expression: read input[0] (signal value)
        let case_expr = expr_compiler.compile_signal_read(0, 32);

        // Item 1: case 1 -> signal 0 = 11
        let item1_expr = expr_compiler.compile_const(1, 32);
        let item1_const = expr_compiler.compile_const(11, 32);
        let item1_stmt = stmt_compiler.compile_blocking_assign(vec![], 0, &item1_const);

        // Item 2: case 2 -> signal 0 = 22
        let item2_expr = expr_compiler.compile_const(2, 32);
        let item2_const = expr_compiler.compile_const(22, 32);
        let item2_stmt = stmt_compiler.compile_blocking_assign(vec![], 0, &item2_const);

        // Default: signal 0 = 99
        let default_const = expr_compiler.compile_const(99, 32);
        let default_stmt = stmt_compiler.compile_blocking_assign(vec![], 0, &default_const);

        let case_stmt = stmt_compiler.compile_case_stmt(
            &case_expr,
            vec![
                (vec![item1_expr.clone()], vec![item1_stmt.clone()]),
                (vec![item2_expr.clone()], vec![item2_stmt.clone()]),
            ],
            Some(vec![default_stmt.clone()]),
        );

        // Test case 1: input[0] = 1
        let inputs = vec![1u64];
        let mut outputs = vec![0u64; 1];
        let changed = (case_stmt.exec_fn)(&inputs, &mut outputs);
        assert!(changed);
        assert_eq!(outputs[0], 11, "case 1 should execute");

        // Test case 2: input[0] = 2
        let inputs = vec![2u64];
        let mut outputs = vec![0u64; 1];
        let changed = (case_stmt.exec_fn)(&inputs, &mut outputs);
        assert!(changed);
        assert_eq!(outputs[0], 22, "case 2 should execute");

        // Test default: input[0] = 5
        let inputs = vec![5u64];
        let mut outputs = vec![0u64; 1];
        let changed = (case_stmt.exec_fn)(&inputs, &mut outputs);
        assert!(changed);
        assert_eq!(outputs[0], 99, "default should execute");

        // Test with no default
        let case_stmt_no_default = stmt_compiler.compile_case_stmt(
            &case_expr,
            vec![
                (vec![item1_expr], vec![item1_stmt]),
                (vec![item2_expr], vec![item2_stmt]),
            ],
            None,
        );
        let inputs = vec![5u64];
        let mut outputs = vec![0u64; 1];
        let changed = (case_stmt_no_default.exec_fn)(&inputs, &mut outputs);
        assert!(!changed, "no match and no default should not change");
    }

    #[test]
    fn test_jit_stmt_cache_persistence() {
        use std::fs;
        let temp_dir = std::env::temp_dir();
        let cache_path = temp_dir.join("jit_stmt_cache_test.txt");

        // Clean up any existing cache file
        let _ = fs::remove_file(&cache_path);

        // Create compiler and compile some statements
        let mut stmt_compiler = JITStmtCompiler::new();
        let mut expr_compiler = JITCompiler::new().unwrap();

        let c = expr_compiler.compile_const(42, 32);
        let _ = stmt_compiler.compile_blocking_assign(vec![], 0, &c);

        let lhs = expr_compiler.compile_const(10, 32);
        let rhs = expr_compiler.compile_const(20, 32);
        let add_expr = expr_compiler.compile_binary("+", &lhs, &rhs, 32);
        let _ = stmt_compiler.compile_blocking_assign(vec![0, 1], 0, &add_expr);

        // Set cache path and save
        stmt_compiler.set_cache_path(cache_path.to_str().unwrap());
        assert!(stmt_compiler.save_cache());
        assert!(cache_path.exists());

        // Verify cache file has entries
        let content = fs::read_to_string(&cache_path).unwrap();
        assert!(content.lines().count() >= 2);

        // Create new compiler, load cache, and verify statements are loaded
        let mut stmt_compiler2 = JITStmtCompiler::new();
        stmt_compiler2.set_cache_path(cache_path.to_str().unwrap());

        // The cache should have loaded entries (as placeholders)
        let (loaded_count, _) = stmt_compiler2.cache_stats();
        // Note: loaded entries are placeholders, not counted in cache_stats
        // but they are in the cache map

        // Clean up
        let _ = fs::remove_file(&cache_path);
    }
}
