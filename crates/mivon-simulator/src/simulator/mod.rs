pub mod arena;
pub mod checkpoint;
pub mod cosim;
pub mod distributed;

/// Debug mode simulasi via env var `DBG_SIM` (analog `DBG_ELAB` untuk
/// elaborator). Di-aktifkan saat `DBG_SIM` di-set (nilai apa pun); nilai
/// `DBG_SIM=1` saja menampilkan proses. `DBG_SIM=2` menambah detail per-delta.
/// Cache OnceLock — nol overhead saat tidak dipakai.
pub fn dbg_sim_level() -> u8 {
    use std::sync::OnceLock;
    static LEVEL: OnceLock<u8> = OnceLock::new();
    *LEVEL.get_or_init(|| {
        std::env::var("DBG_SIM")
            .map(|v| v.parse::<u8>().unwrap_or(1))
            .unwrap_or(0)
    })
}

#[macro_export]
/// Cetak baris debug simulasi `[DBG-SIM]` bila `DBG_SIM` aktif pada level
/// yang diminta. Pemakaian: `dbg_sim!(1, "t={} pid={}", t, pid)`.
macro_rules! dbg_sim {
    ($lvl:expr, $($arg:tt)*) => {
        if ($crate::simulator::dbg_sim_level() as u32) >= ($lvl as u32) {
            eprintln!("[DBG-SIM] {}", format!($($arg)*));
        }
    };
}
pub mod coverage_db;
#[cfg(feature = "dpi")]
pub mod dpi;
pub mod engine;
pub mod group_snapshot;
pub mod guard;
pub mod jit;
#[cfg(feature = "jit")]
pub mod jit_cranelift;
#[cfg(feature = "jit")]
pub mod jit_eval;
pub mod liberty;
pub mod packed;
pub mod packed_eval;
pub mod parallel;
pub mod sdf;
pub mod signal_history;
pub mod simd_packed;
pub mod state;
pub mod types;
pub mod upf;
pub mod util;
pub mod value;

pub use engine::*;
pub use jit::*;
#[cfg(feature = "jit")]
pub use jit_cranelift::*;
#[cfg(feature = "jit")]
pub use jit_eval::*;
pub use packed::*;
pub use sdf::*;
pub use state::*;
pub use types::*;
pub use util::*;
pub use value::*;

/// JIT Evaluator fallback — digunakan saat `jit` feature tidak aktif.
/// Semua method no-op, engine tetap berfungsi tanpa native compilation.
#[cfg(not(feature = "jit"))]
pub struct JITEvaluator;

#[cfg(not(feature = "jit"))]
impl JITEvaluator {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self
    }
    pub fn stats(&self) -> (u64, u64, f64) {
        (0, 0, 0.0)
    }
    pub fn is_available(&self) -> bool {
        false
    }
    pub fn compiled_count(&self) -> usize {
        0
    }
    pub fn cache_hit_rate(&self) -> f64 {
        0.0
    }
    pub fn eval_binary(
        &mut self,
        _op: &mivon_ir::BinaryIrOp,
        _lhs: &mivon_ir::LogicVec,
        _rhs: &mivon_ir::LogicVec,
    ) -> Option<mivon_ir::LogicVec> {
        None
    }
    pub fn eval_unary(
        &mut self,
        _op: &mivon_ir::UnaryIrOp,
        _val: &mivon_ir::LogicVec,
    ) -> Option<mivon_ir::LogicVec> {
        None
    }
    pub fn eval_expression(
        &mut self,
        _expr: &mivon_ir::IrExpr,
        _signal_values: &[u64],
        _result_width: usize,
    ) -> Option<mivon_ir::LogicVec> {
        None
    }
}
