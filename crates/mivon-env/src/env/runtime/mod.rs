//! Runtime context — mengatur seluruh resource host: CPU, memori, thread
//! pool, temp directory. Compiler cukup meminta
//! `runtime.thread_pool.spawn(...)` tanpa tahu detail implementasi thread.

pub mod affinity;
pub mod cgroup;
mod cpu;
mod memory;
#[allow(clippy::module_inception)]
mod runtime;
mod threadpool;

pub use cpu::CpuInfo;
pub use memory::MemoryInfo;
pub use runtime::RuntimeContext;
pub use threadpool::ThreadPoolHandle;
