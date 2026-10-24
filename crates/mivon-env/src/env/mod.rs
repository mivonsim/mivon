//! Enterprise Context Architecture (desain 5 — doc/env.md).
//!
//! `GlobalEnv` adalah root object yang menampung service Mivon sebagai
//! Context terpisah (Config, Workspace, Runtime, Compiler, ...). Tidak ada
//! satu Env raksasa dengan ratusan field — tiap context memiliki tanggung
//! jawab sendiri dan dependency satu arah:
//!
//! ```text
//! Config → Workspace → Runtime → Compiler → Cache/Database/Diagnostics/Telemetry
//!        → Verification → Simulation
//! ```
//!
//! GlobalEnv tidak berisi logika compiler; ia hanya menyimpan service
//! dan menyediakan akses seragam ke mereka.

pub mod cache;
pub mod compiler;
pub mod config;
pub mod database;
pub use mivon_diag::envdiag as diagnostics;
pub mod global;
pub mod plugins;
pub mod runtime;
pub mod security;
pub mod simulation;
pub mod telemetry;
pub mod verification;
pub mod workspace;

pub use cache::{ArtifactPaths, CacheContext, Fingerprint, IncrementalHandle};
pub use compiler::{CompilerContext, HirHandle, OptimizeLevel};
pub use config::{ConfigContext, ConfigSource, EnvCliOptions};
pub use database::DatabaseContext;
pub use diagnostics::DiagnosticsContext;
pub use global::{for_cli, shutdown, startup, startup_with, BuildInfo, GlobalEnv};
pub use plugins::{PluginContext, PluginManagerHandle, PluginRegistry, SandboxPolicy};
pub use runtime::{CpuInfo, MemoryInfo, RuntimeContext, ThreadPoolHandle};
pub use security::{FileAccessPolicy, PermissionSet, SecurityContext};
pub use simulation::{SimulationContext, SimulationKernel, WaveformOptions};
pub use telemetry::{Metrics, PhaseTimings, ProfilerHandle, TelemetryContext, TraceHandle};
pub use verification::{LintChecks, SemanticStatus, VerificationContext, XPropMode};
pub use workspace::{Filelist, IncludeDirs, ProjectFile, WorkspaceContext};
