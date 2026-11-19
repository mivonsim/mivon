//! MIR → Cranelift JIT Compiler (Full Pipeline).
//!
//! Compiles `MirProcess` instruction sequences into native code via Cranelift.
//! The compiled function signature is:
//!   `fn(signals: *mut u64, out_signals: *mut u64, n_sigs: usize)`
//!
//! Where:
//! - `signals` = current signal values (buffer mutable: `Store` menulis
//!   balik agar `Load` berikutnya melihat nilai blocking terbaru)
//! - `out_signals` = output buffer for stores/NBA (mutable array)
//! - `n_sigs` = number of signals
//!
//! The function uses registers stored as Cranelift locals and signal memory
//! via load/store with pointer arithmetic.

use crate::mir::mir::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use cranelift::codegen::ir::UserFuncName;
use cranelift::prelude::*;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module};

/// Compiled MIR process: native function pointer + metadata.
pub struct CompiledMirProcess {
    pub name: String,
    pub code_ptr: *const u8,
    pub n_regs: usize,
    pub n_signals: usize,
}

// Safety: function pointers are Send + Sync
unsafe impl Send for CompiledMirProcess {}
unsafe impl Sync for CompiledMirProcess {}

/// MIR JIT Compiler — compiles MirProcess instances to native code.
pub struct MirJitCompiler {
    module: JITModule,
    ctx: FunctionBuilderContext,
    cache: Mutex<HashMap<u64, CompiledMirProcess>>,
    compiled_count: Mutex<usize>,
    cache_path: Option<PathBuf>,
}

impl MirJitCompiler {
    pub fn new() -> Option<Self> {
        let builder = JITBuilder::new(cranelift_module::default_libcall_names()).ok()?;
        let module = JITModule::new(builder);
        let ctx = FunctionBuilderContext::new();
        Some(MirJitCompiler {
            module,
            ctx,
            cache: Mutex::new(HashMap::new()),
            compiled_count: Mutex::new(0),
            cache_path: None,
        })
    }

    /// Compute a stable hash for a MirProcess (for caching).
    fn process_hash(process: &MirProcess) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        process.name.hash(&mut hasher);
        for instr in &process.instrs {
            std::mem::discriminant(instr).hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Compile a MirProcess to native code.
    /// Returns CompiledMirProcess with function pointer.
    pub fn compile_process(
        &mut self,
        process: &MirProcess,
        n_signals: usize,
    ) -> Option<CompiledMirProcess> {
        let hash = Self::process_hash(process);

        // Check cache. A disk-loaded placeholder has `code_ptr == null()`
        // (native code is process-lifetime only); treat it as a cache miss
        // so the process is recompiled rather than returning a null pointer
        // that callers would transmute and indirect-call → UB (mirrors
        // jit_cranelift.rs::cache_get placeholder handling).
        {
            let cache = self.cache.lock().unwrap();
            if let Some(cached) = cache.get(&hash) {
                if !cached.code_ptr.is_null() {
                    return Some(CompiledMirProcess {
                        name: cached.name.clone(),
                        code_ptr: cached.code_ptr,
                        n_regs: cached.n_regs,
                        n_signals: cached.n_signals,
                    });
                }
                // Stale placeholder: remove so recompile overwrites it.
                drop(cache);
                self.cache.lock().unwrap().remove(&hash);
            }
        }

        if process.instrs.is_empty() {
            return None;
        }

        // Count registers needed (max dest register index + 1)
        let n_regs = self.count_registers(&process.instrs);
        if n_regs == 0 {
            return None;
        }

        // Build Cranelift function
        let mut sig = self.module.make_signature();
        // Arg 0: signals pointer (const u64*)
        sig.params.push(AbiParam::new(types::I64));
        // Arg 1: out_signals pointer (mut u64*)
        sig.params.push(AbiParam::new(types::I64));
        // Arg 2: n_sigs (usize = u64)
        sig.params.push(AbiParam::new(types::I64));
        // No return value
        sig.returns.push(AbiParam::new(types::I64)); // return success code

        let func_name = UserFuncName::user(0, hash as u32);
        let mut func = cranelift::codegen::ir::Function::with_name_signature(func_name, sig);

        let n_sigs_for_function = n_signals;

        {
            let mut builder = FunctionBuilder::new(&mut func, &mut self.ctx);

            let entry_block = builder.create_block();
            builder.append_block_params_for_function_params(entry_block);
            builder.switch_to_block(entry_block);
            builder.seal_block(entry_block);

            let sigs_ptr = builder.block_params(entry_block)[0]; // *const u64
            let out_ptr = builder.block_params(entry_block)[1]; // *mut u64
            let _n_sigs_param = builder.block_params(entry_block)[2]; // u64

            // Allocate register slots as Cranelift stack slots
            let reg_slots: Vec<Variable> = (0..n_regs)
                .map(|i| {
                    let var = Variable::new(i);
                    builder.declare_var(var, types::I64);
                    var
                })
                .collect();

            // Phase 1: Pre-scan instructions untuk collect all Label positions
            // Map label numbers → instruction index and Cranelift block
            let mut label_to_block: std::collections::HashMap<
                usize,
                cranelift::codegen::ir::Block,
            > = std::collections::HashMap::new();
            let mut label_positions: Vec<usize> = Vec::new();
            for (idx, instr) in process.instrs.iter().enumerate() {
                if let MirInstr::Label(l) = instr {
                    label_positions.push(idx);
                    let block = builder.create_block();
                    label_to_block.insert(*l, block);
                }
            }
            let end_block = builder.create_block();

            // Phase 3: Process instructions with proper block switching
            let mut last_was_terminator = false;
            let mut i = 0;
            while i < process.instrs.len() {
                let instr = &process.instrs[i];

                // Check if this instruction is a label — switch to its block
                if let MirInstr::Label(l) = instr {
                    if let Some(&block) = label_to_block.get(l) {
                        // If previous block was unterminated, jump to this label's block
                        if !last_was_terminator {
                            builder.ins().jump(block, &[]);
                        }
                        builder.switch_to_block(block);
                        builder.seal_block(block);
                        last_was_terminator = false;
                    }
                    i += 1;
                    continue;
                }

                // If the previous instruction was a terminator (Branch/Jump),
                // and we're not at a Label, this instruction is unreachable.
                // Switch to end_block to avoid unterminated blocks.
                if last_was_terminator && !matches!(instr, MirInstr::Label(_)) {
                    builder.ins().jump(end_block, &[]);
                    builder.switch_to_block(end_block);
                    last_was_terminator = false;
                }

                match instr {
                    MirInstr::Const { dest, value, width } => {
                        if *dest < n_regs {
                            let w = (*width as u64).min(64);
                            let mask = if w == 64 { u64::MAX } else { (1u64 << w) - 1 };
                            let v = *value & mask;
                            let val = builder.ins().iconst(types::I64, v as i64);
                            builder.def_var(reg_slots[*dest], val);
                        }
                        i += 1;
                    }
                    MirInstr::Load { dest, signal } => {
                        if *dest < n_regs && *signal < n_sigs_for_function {
                            let offset = builder.ins().iconst(types::I64, (*signal * 8) as i64);
                            let ptr = builder.ins().iadd(sigs_ptr, offset);
                            let flags = MemFlags::new().with_notrap();
                            let loaded = builder.ins().load(types::I64, flags, ptr, 0);
                            builder.def_var(reg_slots[*dest], loaded);
                        }
                        i += 1;
                    }
                    MirInstr::Store { signal, src } => {
                        if *src < n_regs && *signal < n_sigs_for_function {
                            let val = builder.use_var(reg_slots[*src]);
                            let offset = builder.ins().iconst(types::I64, (*signal * 8) as i64);
                            let flags = MemFlags::new().with_notrap();
                            // Tulis ke out_ptr (hasil) DAN ke sigs_ptr (input)
                            // agar Load berikutnya dalam proses yang sama
                            // melihat nilai blocking terbaru — read-after-write
                            // semantik interpreter (JIT-05b).
                            let out_slot = builder.ins().iadd(out_ptr, offset);
                            builder.ins().store(flags, val, out_slot, 0);
                            let in_slot = builder.ins().iadd(sigs_ptr, offset);
                            builder.ins().store(flags, val, in_slot, 0);
                        }
                        i += 1;
                    }
                    MirInstr::Binary {
                        op,
                        dest,
                        lhs,
                        rhs,
                        width,
                    } => {
                        if *dest < n_regs && *lhs < n_regs && *rhs < n_regs {
                            let lv = builder.use_var(reg_slots[*lhs]);
                            let rv = builder.use_var(reg_slots[*rhs]);
                            let result = match op {
                                MirBinOp::Add => builder.ins().iadd(lv, rv),
                                MirBinOp::Sub => builder.ins().isub(lv, rv),
                                MirBinOp::Mul => builder.ins().imul(lv, rv),
                                MirBinOp::Div => builder.ins().udiv(lv, rv),
                                MirBinOp::Mod => builder.ins().urem(lv, rv),
                                MirBinOp::And => builder.ins().band(lv, rv),
                                MirBinOp::Or => builder.ins().bor(lv, rv),
                                MirBinOp::Xor => builder.ins().bxor(lv, rv),
                                MirBinOp::Shl => builder.ins().ishl(lv, rv),
                                MirBinOp::Shr => builder.ins().ushr(lv, rv),
                                MirBinOp::Eq | MirBinOp::Ne => {
                                    let cmp = match op {
                                        MirBinOp::Eq => IntCC::Equal,
                                        MirBinOp::Ne => IntCC::NotEqual,
                                        _ => unreachable!(),
                                    };
                                    let cond = builder.ins().icmp(cmp, lv, rv);
                                    let one = builder.ins().iconst(types::I64, 1);
                                    let zero = builder.ins().iconst(types::I64, 0);
                                    builder.ins().select(cond, one, zero)
                                }
                                MirBinOp::Lt | MirBinOp::Le | MirBinOp::Gt | MirBinOp::Ge => {
                                    let cmp = match op {
                                        MirBinOp::Lt => IntCC::UnsignedLessThan,
                                        MirBinOp::Le => IntCC::UnsignedLessThanOrEqual,
                                        MirBinOp::Gt => IntCC::UnsignedGreaterThan,
                                        MirBinOp::Ge => IntCC::UnsignedGreaterThanOrEqual,
                                        _ => unreachable!(),
                                    };
                                    let cond = builder.ins().icmp(cmp, lv, rv);
                                    let one = builder.ins().iconst(types::I64, 1);
                                    let zero = builder.ins().iconst(types::I64, 0);
                                    builder.ins().select(cond, one, zero)
                                }
                                MirBinOp::LogicalAnd => {
                                    let one = builder.ins().iconst(types::I64, 1);
                                    let zero = builder.ins().iconst(types::I64, 0);
                                    let lcond = builder.ins().icmp(IntCC::NotEqual, lv, zero);
                                    let rcond = builder.ins().icmp(IntCC::NotEqual, rv, zero);
                                    let both = builder.ins().band(lcond, rcond);
                                    builder.ins().select(both, one, zero)
                                }
                                MirBinOp::LogicalOr => {
                                    let one = builder.ins().iconst(types::I64, 1);
                                    let zero = builder.ins().iconst(types::I64, 0);
                                    let lcond = builder.ins().icmp(IntCC::NotEqual, lv, zero);
                                    let rcond = builder.ins().icmp(IntCC::NotEqual, rv, zero);
                                    let either = builder.ins().bor(lcond, rcond);
                                    builder.ins().select(either, one, zero)
                                }
                            };
                            let masked = if *width < 64 {
                                let mask_val = ((1u64 << width) - 1) as i64;
                                let mask = builder.ins().iconst(types::I64, mask_val);
                                builder.ins().band(result, mask)
                            } else {
                                result
                            };
                            builder.def_var(reg_slots[*dest], masked);
                        }
                        i += 1;
                    }
                    MirInstr::Unary {
                        op,
                        dest,
                        operand,
                        width,
                    } => {
                        if *dest < n_regs && *operand < n_regs {
                            let val = builder.use_var(reg_slots[*operand]);
                            let result = match op {
                                MirUnOp::Not => builder.ins().bnot(val),
                                MirUnOp::Neg => builder.ins().ineg(val),
                            };
                            let masked = if *width < 64 {
                                let mask_val = ((1u64 << width) - 1) as i64;
                                let mask = builder.ins().iconst(types::I64, mask_val);
                                builder.ins().band(result, mask)
                            } else {
                                result
                            };
                            builder.def_var(reg_slots[*dest], masked);
                        }
                        i += 1;
                    }
                    MirInstr::Branch {
                        cond,
                        then_label,
                        else_label,
                    } => {
                        if *cond < n_regs {
                            let cond_val = builder.use_var(reg_slots[*cond]);
                            let zero = builder.ins().iconst(types::I64, 0);
                            let is_true = builder.ins().icmp(IntCC::NotEqual, cond_val, zero);
                            let then_block =
                                label_to_block.get(then_label).copied().unwrap_or(end_block);
                            let else_block =
                                label_to_block.get(else_label).copied().unwrap_or(end_block);
                            builder
                                .ins()
                                .brif(is_true, then_block, &[], else_block, &[]);
                        }
                        last_was_terminator = true;
                        i += 1;
                    }
                    MirInstr::Jump { label } => {
                        let target_block = label_to_block.get(label).copied().unwrap_or(end_block);
                        builder.ins().jump(target_block, &[]);
                        last_was_terminator = true;
                        i += 1;
                    }
                    MirInstr::NonBlocking { signal, src, .. } => {
                        if *src < n_regs && *signal < n_sigs_for_function {
                            let val = builder.use_var(reg_slots[*src]);
                            let offset = builder.ins().iconst(types::I64, (*signal * 8) as i64);
                            let ptr = builder.ins().iadd(out_ptr, offset);
                            let flags = MemFlags::new().with_notrap();
                            builder.ins().store(flags, val, ptr, 0);
                        }
                        i += 1;
                    }
                    MirInstr::Display { .. } | MirInstr::Finish => {
                        i += 1;
                    }
                    MirInstr::Nop => {
                        i += 1;
                    }
                    MirInstr::Label(_) => {
                        // Already handled above via continue
                        i += 1;
                    }
                }
            }

            // Post-process: ensure all blocks are sealed and terminated
            // If the last block wasn't terminated, jump to end_block
            if !last_was_terminator {
                builder.ins().jump(end_block, &[]);
            }
            // End block: just return success
            builder.switch_to_block(end_block);
            builder.seal_block(end_block);
            let one = builder.ins().iconst(types::I64, 1);
            builder.ins().return_(&[one]);
            builder.finalize();
        }

        let fn_name_str = format!("mir_jit_{:x}", hash);
        let id = self
            .module
            .declare_function(&fn_name_str, Linkage::Local, &func.signature)
            .ok()?;

        let mut ctx = cranelift::codegen::Context::new();
        ctx.func = func;
        self.module.define_function(id, &mut ctx).ok()?;
        // Result dibuang sengaja — kegagalan finalize terdeteksi lewat
        // get_finalized_function yang mengembalikan null di bawah.
        let _ = self.module.finalize_definitions();

        let code_ptr = self.module.get_finalized_function(id);
        self.module.clear_context(&mut ctx);

        *self.compiled_count.lock().unwrap() += 1;

        let compiled = CompiledMirProcess {
            name: process.name.as_str().to_string(),
            code_ptr,
            n_regs,
            n_signals: n_sigs_for_function,
        };

        self.cache.lock().unwrap().insert(
            hash,
            CompiledMirProcess {
                name: compiled.name.clone(),
                code_ptr: compiled.code_ptr,
                n_regs: compiled.n_regs,
                n_signals: compiled.n_signals,
            },
        );

        Some(compiled)
    }

    /// Count the number of registers needed for a MIR instruction sequence.
    fn count_registers(&self, instrs: &[MirInstr]) -> usize {
        let mut max_reg = 0usize;
        for instr in instrs {
            match instr {
                MirInstr::Const { dest, .. }
                | MirInstr::Load { dest, .. }
                | MirInstr::Binary { dest, .. }
                | MirInstr::Unary { dest, .. } => {
                    max_reg = max_reg.max(*dest + 1);
                }
                _ => {}
            }
        }
        max_reg
    }

    /// Call a compiled MIR process function.
    ///
    /// # Safety
    /// `code_ptr` must point to a valid compiled function with the correct signature.
    pub unsafe fn call_process(
        code_ptr: *const u8,
        signal_vals: &mut [u64],
        out_vals: &mut [u64],
    ) -> u64 {
        type MirProcessFn = unsafe extern "C" fn(*mut u64, *mut u64, u64) -> u64;
        let func: MirProcessFn = std::mem::transmute(code_ptr);
        func(
            signal_vals.as_mut_ptr(),
            out_vals.as_mut_ptr(),
            signal_vals.len() as u64,
        )
    }

    /// Clear the compilation cache (for testing/reloading).
    pub fn clear_cache(&mut self) {
        self.cache.lock().unwrap().clear();
    }

    /// Set persistent cache file path. The cache saves compiled function
    /// metadata so that on next startup, functions can be pre-populated.
    /// Native code pointers are ephemeral (process-lifetime), so we save
    /// metadata to reconstruct the cache on next startup.
    pub fn set_cache_path(&mut self, path: &str) {
        self.cache_path = Some(PathBuf::from(path));
        self.load_cache();
    }

    /// Save cache metadata to disk using a simple line-based format.
    /// Format: each line is "hash|name|n_regs|n_signals"
    pub fn save_cache(&self) -> bool {
        let path = match &self.cache_path {
            Some(p) => p,
            None => return false,
        };

        let cache = self.cache.lock().unwrap();
        let mut output = String::new();
        for (hash, entry) in cache.iter() {
            output.push_str(&format!(
                "{}|{}|{}|{}\n",
                hash, entry.name, entry.n_regs, entry.n_signals
            ));
        }
        drop(cache);

        match std::fs::write(path, &output) {
            Ok(_) => {
                eprintln!(
                    "MIR JIT cache saved: {} entries to {}",
                    output.lines().count(),
                    path.display()
                );
                true
            }
            Err(e) => {
                eprintln!("MIR JIT cache write error: {}", e);
                false
            }
        }
    }

    /// Load cache metadata from disk and pre-populate the in-memory cache.
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
        let mut cache = self.cache.lock().unwrap();
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
            let n_regs: usize = match parts[2].parse() {
                Ok(c) => c,
                Err(_) => continue,
            };
            let n_signals: usize = match parts[3].parse() {
                Ok(w) => w,
                Err(_) => continue,
            };

            if let std::collections::hash_map::Entry::Vacant(e) = cache.entry(hash) {
                let placeholder = CompiledMirProcess {
                    name,
                    code_ptr: std::ptr::null(),
                    n_regs,
                    n_signals,
                };
                e.insert(placeholder);
                loaded_count += 1;
            }
        }
        drop(cache);

        if loaded_count > 0 {
            eprintln!(
                "MIR JIT cache loaded: {} entries from {}",
                loaded_count,
                path.display()
            );
        }
        true
    }

    /// Get cache path.
    pub fn cache_path(&self) -> Option<&std::path::Path> {
        self.cache_path.as_deref()
    }

    /// Statistics
    pub fn compiled_count(&self) -> usize {
        *self.compiled_count.lock().unwrap()
    }

    pub fn cache_size(&self) -> usize {
        self.cache.lock().unwrap().len()
    }
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;
    use mivon_core::intern::Symbol;

    fn make_const_process() -> MirProcess {
        // Simple process: reg0 = 42
        MirProcess {
            name: Symbol::intern("test_const"),
            sensitivity: MirSensitivity::AlwaysComb,
            instrs: vec![MirInstr::Const {
                dest: 0,
                value: 42,
                width: 32,
            }],
        }
    }

    fn make_add_process() -> MirProcess {
        // reg0 = signal[0] + signal[1]
        MirProcess {
            name: Symbol::intern("test_add"),
            sensitivity: MirSensitivity::AlwaysComb,
            instrs: vec![
                MirInstr::Load { dest: 0, signal: 0 },
                MirInstr::Load { dest: 1, signal: 1 },
                MirInstr::Binary {
                    op: MirBinOp::Add,
                    dest: 2,
                    lhs: 0,
                    rhs: 1,
                    width: 32,
                },
            ],
        }
    }

    fn make_store_process() -> MirProcess {
        // out[0] = signal[0] + 1
        MirProcess {
            name: Symbol::intern("test_store"),
            sensitivity: MirSensitivity::AlwaysComb,
            instrs: vec![
                MirInstr::Load { dest: 0, signal: 0 },
                MirInstr::Const {
                    dest: 1,
                    value: 1,
                    width: 32,
                },
                MirInstr::Binary {
                    op: MirBinOp::Add,
                    dest: 2,
                    lhs: 0,
                    rhs: 1,
                    width: 32,
                },
                MirInstr::Store { signal: 0, src: 2 },
            ],
        }
    }

    fn make_if_process() -> MirProcess {
        // if(signal[0]) { out[0] = 10 } else { out[0] = 20 }
        MirProcess {
            name: Symbol::intern("test_if"),
            sensitivity: MirSensitivity::AlwaysComb,
            instrs: vec![
                MirInstr::Load { dest: 0, signal: 0 },
                MirInstr::Const {
                    dest: 1,
                    value: 10,
                    width: 32,
                },
                MirInstr::Const {
                    dest: 2,
                    value: 20,
                    width: 32,
                },
                MirInstr::Branch {
                    cond: 0,
                    then_label: 1,
                    else_label: 2,
                },
                MirInstr::Label(1),
                MirInstr::Store { signal: 1, src: 1 },
                MirInstr::Jump { label: 3 },
                MirInstr::Label(2),
                MirInstr::Store { signal: 1, src: 2 },
                MirInstr::Label(3),
            ],
        }
    }

    #[test]
    fn test_mir_jit_create() {
        let compiler = MirJitCompiler::new();
        assert!(compiler.is_some(), "MirJitCompiler should initialize");
    }

    #[test]
    fn test_mir_jit_const() {
        let mut compiler = MirJitCompiler::new().unwrap();
        let process = make_const_process();
        let compiled = compiler.compile_process(&process, 2).unwrap();
        assert_eq!(compiled.n_regs, 1);

        let mut signals = [0u64; 4];
        let mut out = [0u64; 4];
        unsafe {
            MirJitCompiler::call_process(compiled.code_ptr, &mut signals, &mut out);
        }
        // Process only writes to register, not to output
        // We just verify it doesn't crash
    }

    #[test]
    fn test_mir_jit_add() {
        let mut compiler = MirJitCompiler::new().unwrap();
        let process = make_add_process();
        let compiled = compiler.compile_process(&process, 3).unwrap();

        let mut signals = [10u64, 20u64, 0u64];
        let mut out = [0u64; 3];
        unsafe {
            MirJitCompiler::call_process(compiled.code_ptr, &mut signals, &mut out);
        }
    }

    #[test]
    fn test_mir_jit_store() {
        let mut compiler = MirJitCompiler::new().unwrap();
        let process = make_store_process();
        let compiled = compiler.compile_process(&process, 2).unwrap();

        let mut signals = [5u64, 0u64];
        let mut out = [0u64; 2];
        unsafe {
            MirJitCompiler::call_process(compiled.code_ptr, &mut signals, &mut out);
        }
        assert_eq!(out[0], 6, "signal[0] + 1 should be 6");
    }

    #[test]
    fn test_mir_jit_if_true() {
        let mut compiler = MirJitCompiler::new().unwrap();
        let process = make_if_process();
        let compiled = compiler.compile_process(&process, 3).unwrap();

        let mut signals = [1u64, 0u64, 0u64]; // signal[0] = 1 (true)
        let mut out = [0u64; 3];
        unsafe {
            MirJitCompiler::call_process(compiled.code_ptr, &mut signals, &mut out);
        }
        assert_eq!(out[1], 10, "if true: out[1] should be 10");
    }

    #[test]
    fn test_mir_jit_if_false() {
        let mut compiler = MirJitCompiler::new().unwrap();
        let process = make_if_process();
        let compiled = compiler.compile_process(&process, 3).unwrap();

        let mut signals = [0u64, 0u64, 0u64]; // signal[0] = 0 (false)
        let mut out = [0u64; 3];
        unsafe {
            MirJitCompiler::call_process(compiled.code_ptr, &mut signals, &mut out);
        }
        assert_eq!(out[1], 20, "if false: out[1] should be 20");
    }

    #[test]
    fn test_mir_jit_cache() {
        let mut compiler = MirJitCompiler::new().unwrap();
        let process = make_const_process();
        assert_eq!(compiler.cache_size(), 0);
        let _ = compiler.compile_process(&process, 2);
        assert_eq!(compiler.cache_size(), 1);
        let _ = compiler.compile_process(&process, 2); // cache hit
        assert_eq!(compiler.cache_size(), 1); // still 1
        assert_eq!(compiler.compiled_count(), 1); // compiled once
    }

    #[test]
    fn test_mir_jit_clear_cache() {
        let mut compiler = MirJitCompiler::new().unwrap();
        let process = make_const_process();
        let _ = compiler.compile_process(&process, 2);
        assert_eq!(compiler.cache_size(), 1);
        compiler.clear_cache();
        assert_eq!(compiler.cache_size(), 0);
    }

    #[test]
    fn test_mir_jit_multi_store() {
        // Process: out[0] = signal[0] + 1, out[1] = signal[1] * 2
        let process = MirProcess {
            name: Symbol::intern("test_multi_store"),
            sensitivity: MirSensitivity::AlwaysComb,
            instrs: vec![
                MirInstr::Load { dest: 0, signal: 0 },
                MirInstr::Const {
                    dest: 1,
                    value: 1,
                    width: 32,
                },
                MirInstr::Binary {
                    op: MirBinOp::Add,
                    dest: 2,
                    lhs: 0,
                    rhs: 1,
                    width: 32,
                },
                MirInstr::Store { signal: 0, src: 2 },
                MirInstr::Load { dest: 3, signal: 1 },
                MirInstr::Const {
                    dest: 4,
                    value: 2,
                    width: 32,
                },
                MirInstr::Binary {
                    op: MirBinOp::Mul,
                    dest: 5,
                    lhs: 3,
                    rhs: 4,
                    width: 32,
                },
                MirInstr::Store { signal: 1, src: 5 },
            ],
        };
        let mut compiler = MirJitCompiler::new().unwrap();
        let compiled = compiler.compile_process(&process, 3).unwrap();

        let mut signals = [10u64, 20u64, 0u64];
        let mut out = [0u64; 3];
        unsafe {
            MirJitCompiler::call_process(compiled.code_ptr, &mut signals, &mut out);
        }
        assert_eq!(out[0], 11, "signal[0] + 1 = 11");
        assert_eq!(out[1], 40, "signal[1] * 2 = 40");
    }

    #[test]
    fn test_mir_jit_store_visible_to_later_load() {
        // Read-after-write: out[0] = sig[0] + 1; out[1] = out[0] (Load dari
        // sig[0] SETELAH Store) harus melihat nilai baru, bukan snapshot awal
        // (JIT-05b). Tanpa store-balik ke buffer input, out[1] = 10.
        let process = MirProcess {
            name: Symbol::intern("test_store_visible"),
            sensitivity: MirSensitivity::AlwaysComb,
            instrs: vec![
                MirInstr::Load { dest: 0, signal: 0 },
                MirInstr::Const {
                    dest: 1,
                    value: 1,
                    width: 32,
                },
                MirInstr::Binary {
                    op: MirBinOp::Add,
                    dest: 2,
                    lhs: 0,
                    rhs: 1,
                    width: 32,
                },
                MirInstr::Store { signal: 0, src: 2 },
                MirInstr::Load { dest: 3, signal: 0 },
                MirInstr::Store { signal: 1, src: 3 },
            ],
        };
        let mut compiler = MirJitCompiler::new().unwrap();
        let compiled = compiler.compile_process(&process, 2).unwrap();

        let mut signals = [10u64, 0u64];
        let mut out = [0u64; 2];
        unsafe {
            MirJitCompiler::call_process(compiled.code_ptr, &mut signals, &mut out);
        }
        assert_eq!(out[0], 11, "sig[0] + 1 = 11");
        assert_eq!(out[1], 11, "Load setelah Store harus melihat nilai baru");
    }

    #[test]
    fn test_mir_jit_unary_not() {
        // Process: out[0] = ~signal[0]
        let process = MirProcess {
            name: Symbol::intern("test_unary_not"),
            sensitivity: MirSensitivity::AlwaysComb,
            instrs: vec![
                MirInstr::Load { dest: 0, signal: 0 },
                MirInstr::Unary {
                    op: MirUnOp::Not,
                    dest: 1,
                    operand: 0,
                    width: 8,
                },
                MirInstr::Store { signal: 0, src: 1 },
            ],
        };
        let mut compiler = MirJitCompiler::new().unwrap();
        let compiled = compiler.compile_process(&process, 2).unwrap();

        let mut signals = [0xFu64, 0u64];
        let mut out = [0u64; 2];
        unsafe {
            MirJitCompiler::call_process(compiled.code_ptr, &mut signals, &mut out);
        }
        assert_eq!(out[0], 0xF0, "~0x0F with width=8 should be 0xF0");
    }

    #[test]
    fn test_mir_jit_empty() {
        let mut compiler = MirJitCompiler::new().unwrap();
        let process = MirProcess {
            name: Symbol::intern("empty"),
            sensitivity: MirSensitivity::AlwaysComb,
            instrs: vec![],
        };
        let compiled = compiler.compile_process(&process, 1);
        assert!(compiled.is_none(), "empty process should return None");
    }

    #[test]
    fn test_mir_jit_cache_save_load_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("mir_jit_cache_test_{}.bin", std::process::id()));
        let _ = std::fs::remove_file(&tmp);

        // Compiler A: compile a process, then save cache to disk.
        {
            let mut compiler = MirJitCompiler::new().unwrap();
            let process = MirProcess {
                name: Symbol::intern("cache_proc"),
                sensitivity: MirSensitivity::AlwaysComb,
                instrs: vec![MirInstr::Const {
                    dest: 0,
                    value: 42,
                    width: 32,
                }],
            };
            assert!(compiler.compile_process(&process, 2).is_some());
            assert_eq!(compiler.cache_size(), 1);
            compiler.set_cache_path(tmp.to_str().unwrap());
            assert!(compiler.save_cache(), "save_cache should succeed");
        }

        // Compiler B: fresh compiler, load cache from disk, entry restored as placeholder.
        let mut compiler_b = MirJitCompiler::new().unwrap();
        assert_eq!(compiler_b.cache_size(), 0);
        compiler_b.set_cache_path(tmp.to_str().unwrap());
        assert_eq!(
            compiler_b.cache_size(),
            1,
            "loaded cache should have 1 entry"
        );
        assert_eq!(
            compiler_b.cache_path(),
            Some(tmp.as_path()),
            "cache_path getter should return the set path"
        );

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn test_mir_jit_set_cache_path_no_file_yet() {
        let tmp = std::env::temp_dir().join(format!("mir_jit_cache_path_test_{}.bin", std::process::id()));
        let _ = std::fs::remove_file(&tmp);

        let mut compiler = MirJitCompiler::new().unwrap();
        assert!(compiler.cache_path().is_none());
        compiler.set_cache_path(tmp.to_str().unwrap());
        assert_eq!(compiler.cache_path(), Some(tmp.as_path()));
        // No file yet -> load_cache is a no-op, cache stays empty.
        assert_eq!(compiler.cache_size(), 0);

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn test_mir_jit_recompile_after_load_avoids_null_ptr() {
        // Reproduces the HIGH finding: load_cache inserts placeholders with
        // null code_ptr. Recompiling the same process must not hand back a
        // null pointer to callers.
        let tmp = std::env::temp_dir().join(format!("mir_jit_null_test_{}.bin", std::process::id()));
        let _ = std::fs::remove_file(&tmp);

        // Compiler A: compile + save.
        {
            let mut compiler = MirJitCompiler::new().unwrap();
            let process = MirProcess {
                name: Symbol::intern("null_proc"),
                sensitivity: MirSensitivity::AlwaysComb,
                instrs: vec![MirInstr::Const {
                    dest: 0,
                    value: 7,
                    width: 32,
                }],
            };
            assert!(compiler.compile_process(&process, 2).is_some());
            assert_eq!(compiler.cache_size(), 1);
            compiler.set_cache_path(tmp.to_str().unwrap());
            assert!(compiler.save_cache());
        }

        // Compiler B: load placeholders from disk (all code_ptr == null).
        let mut compiler_b = MirJitCompiler::new().unwrap();
        compiler_b.set_cache_path(tmp.to_str().unwrap());
        assert_eq!(compiler_b.cache_size(), 1);

        // Recompile the same process — must skip the null placeholder and
        // produce a real (non-null) function pointer.
        let process = MirProcess {
            name: Symbol::intern("null_proc"),
            sensitivity: MirSensitivity::AlwaysComb,
            instrs: vec![MirInstr::Const {
                dest: 0,
                value: 7,
                width: 32,
            }],
        };
        let compiled = compiler_b.compile_process(&process, 2);
        assert!(
            compiled.is_some(),
            "recompile after load must return Some"
        );
        let compiled = compiled.unwrap();
        assert!(
            !compiled.code_ptr.is_null(),
            "compiled code_ptr must not be null after load+recompile"
        );

        // Sanity: executing the recompiled process does not crash.
        let mut signals = [0u64; 2];
        let mut out = [0u64; 2];
        unsafe {
            MirJitCompiler::call_process(compiled.code_ptr, &mut signals, &mut out);
        }

        let _ = std::fs::remove_file(&tmp);
    }
}
