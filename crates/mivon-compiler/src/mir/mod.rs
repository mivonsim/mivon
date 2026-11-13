//! Mid-Level IR — simulation-optimized intermediate representation.

#[cfg(feature = "jit")]
pub mod jit;
pub mod lower;
#[allow(clippy::module_inception)]
pub mod mir;
pub mod opt;
pub mod ssa;

pub use lower::lower_module;
pub use mir::*;
pub use opt::{optimize_module, optimize_process};

// MirJitCompiler fallback when jit feature is disabled
#[cfg(feature = "jit")]
pub use jit::MirJitCompiler;

/// MirJitCompiler fallback — digunakan saat `jit` feature tidak aktif.
/// Semua method no-op, engine tetap berfungsi tanpa compiled-code simulation.
#[cfg(not(feature = "jit"))]
pub struct MirJitCompiler;

/// CompiledMirProcess fallback — dummy type untuk mir.jit module when feature is off.
#[cfg(not(feature = "jit"))]
pub struct CompiledMirProcess {
    pub code_ptr: *const u8,
}

#[cfg(not(feature = "jit"))]
impl MirJitCompiler {
    pub fn new() -> Option<Self> {
        None
    }
    pub fn compile_process(
        &mut self,
        _process: &crate::mir::MirProcess,
        _n_sigs: usize,
    ) -> Option<CompiledMirProcess> {
        None
    }
    /// Call a compiled process. Stub — no-op saat `jit` feature tidak aktif.
    ///
    /// # Safety
    /// Implementasi nyata (feature `jit`) mengharuskan `_code_ptr` menunjuk
    /// entry point process yang valid hasil kompilasi. Stub ini tidak membaca
    /// memory sama sekali; safety requirement dipertahankan agar signature
    /// identik dengan jalur `jit` (callers tetap harus menjamin validity).
    pub unsafe fn call_process(_code_ptr: *const u8, _signals: &[u64], _out: &mut [u64]) {}
}
