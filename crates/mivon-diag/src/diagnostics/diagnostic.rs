//! Diagnostic — structured error/warning reporting ala Cargo/Rust.
//!
//! Setiap diagnostic memiliki: level, code, message, spans, notes, hints,
//! explanation, suggestion, runtime context, dan source snippet.
//! Thread-safe via MPSC channel (DiagSink).
//!
//! Format keluaran:
//!
//! ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
//!
//! error[RT0001]: Null handle access
//!
//! ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
//!
//!    ┌─ uart.sv:52:14
//!    │
//! 52 │ fifo.push(data);
//!    │      ^^^^ Handle "fifo" is null
//!    │
//!    = module : uart_top
//!    = process: write_task
//!    = time   : 125 ns
//!    = thread : worker-2
//!
//! Explanation
//!
//! The FIFO handle was never initialized before simulation started.
//!
//! Help
//!
//! Initialize the interface before using it.

use std::fmt;
pub use super::fixit::FixItHint;

// ─── Severity ───

/// Severity level untuk diagnostic HDL — fatal, error, warning, note, dll.
pub use super::level::DiagLevel;

// ─── Diagnostic Code ───

/// Error code untuk diagnostic.
///
/// Kategori kode:
/// - E1xxx: Parse errors
/// - E2xxx: Semantic errors
/// - E3xxx: Elaboration errors
/// - RT0xxx: Memory runtime errors
/// - RT1xxx: Signal runtime errors
/// - RT2xxx: Scheduler runtime errors
/// - RT3xxx: Event runtime errors
/// - RT4xxx: Module runtime errors
/// - RT5xxx: Interface runtime errors
/// - RT6xxx: Clock runtime errors
/// - RT7xxx: Assertion runtime errors
/// - RT8xxx: DPI runtime errors
/// - RT9xxx: Internal runtime errors
/// - WRxxxx: Warnings
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagCode {
    // ── Parse errors: E1xxx ──
    UnexpectedToken,
    ExpectedToken,
    ExpectedSemi,
    UnclosedBlock,
    InvalidSyntax,

    // ── Semantic errors: E2xxx ──
    UndefinedSignal,
    TypeMismatch,
    WidthMismatch,
    UndefinedVariable,

    // ── Elaboration errors: E3xxx ──
    ModuleNotFound,
    CircularDependency,
    ParamMismatch,
    InstanceNotFound,
    TopResolutionFailed,
    /// Multiple candidate top modules found (E3006)
    MultipleCandidateTops,
    /// Missing root module (E3007)
    MissingRootModule,
    /// Unresolved instantiation in hierarchy (E3008)
    UnresolvedInstantiation,
    /// Circular hierarchy detected (E3009)
    CircularHierarchy,
    /// Module excluded by filelist (E3010)
    ExcludedByFilelist,
    /// Module/interface/package defined more than once (E3011)
    DuplicateDeclaration,
    /// Packed vector width exceeds implementation limit (E3012) — input
    /// mendeklarasikan sinyal/vektor lebih lebar dari batas aman; menolak
    /// dengan diag bersih daripada hang/OOM/silent-clamp.
    PackedWidthTooLarge,
    /// Multiple continuous assignments drive one VARIABLE (E3013) — ilegal
    /// (LRM §10.5); jalur engine dapat memberi hasil berbeda (temuan fuzz
    /// bug differential d2919857: sda_o=1 di default vs x di DAG).
    MultipleContinuousDriver,

    // ── Runtime Memory: RT0xxx ──
    /// Null handle access (RT0001)
    NullHandle,
    /// Invalid object reference (RT0002)
    InvalidReference,
    /// Null interface dereference (RT0003)
    NullInterface,
    /// Out of bounds memory access (RT0004)
    MemoryOutOfBounds,
    /// Mailbox access error (RT0005)
    MailboxError,

    // ── Runtime Signal: RT1xxx ──
    /// Signal entered unknown state (X) (RT1001)
    SignalUnknown,
    /// Floating signal detected (RT1002)
    SignalFloating,
    /// Driver contention (RT1003)
    SignalContention,
    /// NBA write conflict — two processes write same signal via NBA in same delta (RT1006)
    NbaWriteConflict,
    /// Signal width mismatch at runtime (RT1004)
    SignalWidthMismatch,
    /// Uninitialized signal read (RT1005)
    SignalUninitialized,

    // ── Runtime Scheduler: RT2xxx ──
    /// Infinite delta cycle (RT2001)
    InfiniteDelta,
    /// Scheduler deadlock (RT2002)
    SchedulerDeadlock,
    /// Simulation timeout (RT2003)
    SimulationTimeout,
    /// Process fork error (RT2004)
    ForkError,
    /// Max delta cycles exceeded (RT2005)
    MaxDeltaExceeded,

    // ── Runtime Event: RT3xxx ──
    /// Event wait timeout (RT3001)
    EventTimeout,
    /// Event order violation (RT3002)
    EventOrderViolation,
    /// Event creation failed (RT3003)
    EventCreateFailed,

    // ── Runtime Module: RT4xxx ──
    /// Module instantiation error (RT4001)
    ModuleInstantiation,
    /// Module binding error (RT4002)
    ModuleBindError,

    // ── Runtime Interface: RT5xxx ──
    /// Null interface handle (RT5001)
    InterfaceNull,
    /// Interface connection error (RT5002)
    InterfaceConnect,

    // ── Runtime Clock: RT6xxx ──
    /// Clock period violation (RT6001)
    ClockPeriodViolation,
    /// Clock generation error (RT6002)
    ClockGenError,

    // ── Runtime Assertion: RT7xxx ──
    /// Concurrent assertion failed (RT7001)
    AssertionFailed,
    /// Immediate assertion failed (RT7002)
    AssertionImmediateFailed,
    /// Cover property hit count (RT7003)
    CoverProperty,
    /// Assertion disable error (RT7004)
    AssertionDisableError,

    // ── Runtime DPI: RT8xxx ──
    /// DPI function error (RT8001)
    DpiError,
    /// DPI import not found (RT8002)
    DpiImportNotFound,
    /// DPI scope error (RT8003)
    DpiScopeError,

    // ── Runtime Internal: RT9xxx ──
    /// Internal simulator error (RT9001)
    InternalError,
    /// Unreachable code reached (RT9002)
    Unreachable,
    /// Not yet implemented (RT9003)
    NotImplemented,
    /// Resource limit exceeded (RT9004) — simulasi melebihi batas memori
    /// yang dikonfigurasi (`MIVON_SIM_MEM_LIMIT_MB`), dicegah oleh
    /// `SimResourceGuard` sebelum kernel OOM-kill.
    ResourceLimit,

    // ── Legacy Runtime (E9xxx) ──
    /// General simulation error (E9001)
    SimulationError,
    /// Out of bounds (E9002)
    OutOfBounds,
    /// Type mismatch at runtime (E9003)
    RuntimeTypeMismatch,

    // ── Infrastructure Errors (E0xxx) ──
    /// Preprocessor/IO error (E0101)
    PreprocessorError,
    /// Debugger error (E0003)
    DebuggerError,
    /// I/O operation error (E0004)
    IoError,

    // ── Other Codes ──
    /// Waveform error (W0001)
    WaveformError,

    // ── Warnings: WRxxxx ──
    /// Uninitialized register (WR0014)
    UninitializedRegister,
    /// Width mismatch warning (WR0102)
    WidthMismatchWarning,
    /// Unused signal (WR0104)
    UnusedSignal,
    /// Clock never toggles (WR0202)
    ClockNeverToggles,
    /// Reset permanently asserted (WR0203)
    ResetPermanentlyAsserted,
    /// Possible combinational loop (WR0301)
    CombinationalLoop,
    /// Signal glitch detected (WR0302)
    SignalGlitch,
    /// Timing check violation (WR0303)
    TimingViolation,
    /// Slow simulation region (WR0402)
    SlowSimulation,
}

impl DiagCode {
    /// Kembalikan string kode error (misal "RT0001", "WR0102").
    pub fn as_str(&self) -> &'static str {
        match self {
            // Parse
            DiagCode::UnexpectedToken => "E1001",
            DiagCode::ExpectedToken => "E1002",
            DiagCode::ExpectedSemi => "E1003",
            DiagCode::UnclosedBlock => "E1004",
            DiagCode::InvalidSyntax => "E1005",
            // Semantic
            DiagCode::UndefinedSignal => "E2001",
            DiagCode::TypeMismatch => "E2002",
            DiagCode::WidthMismatch => "E2003",
            DiagCode::UndefinedVariable => "E2004",
            // Elaboration
            DiagCode::ModuleNotFound => "E3001",
            DiagCode::CircularDependency => "E3002",
            DiagCode::ParamMismatch => "E3003",
            DiagCode::InstanceNotFound => "E3004",
            DiagCode::TopResolutionFailed => "E3005",
            DiagCode::MultipleCandidateTops => "E3006",
            DiagCode::MissingRootModule => "E3007",
            DiagCode::UnresolvedInstantiation => "E3008",
            DiagCode::CircularHierarchy => "E3009",
            DiagCode::ExcludedByFilelist => "E3010",
            DiagCode::DuplicateDeclaration => "E3011",
            DiagCode::PackedWidthTooLarge => "E3012",
            DiagCode::MultipleContinuousDriver => "E3013",
            // Runtime Memory
            DiagCode::NullHandle => "RT0001",
            DiagCode::InvalidReference => "RT0002",
            DiagCode::NullInterface => "RT0003",
            DiagCode::MemoryOutOfBounds => "RT0004",
            DiagCode::MailboxError => "RT0005",
            // Runtime Signal
            DiagCode::SignalUnknown => "RT1001",
            DiagCode::SignalFloating => "RT1002",
            DiagCode::SignalContention => "RT1003",
            DiagCode::SignalWidthMismatch => "RT1004",
            DiagCode::SignalUninitialized => "RT1005",
            DiagCode::NbaWriteConflict => "RT1006",
            // Runtime Scheduler
            DiagCode::InfiniteDelta => "RT2001",
            DiagCode::SchedulerDeadlock => "RT2002",
            DiagCode::SimulationTimeout => "RT2003",
            DiagCode::ForkError => "RT2004",
            DiagCode::MaxDeltaExceeded => "RT2005",
            // Runtime Event
            DiagCode::EventTimeout => "RT3001",
            DiagCode::EventOrderViolation => "RT3002",
            DiagCode::EventCreateFailed => "RT3003",
            // Runtime Module
            DiagCode::ModuleInstantiation => "RT4001",
            DiagCode::ModuleBindError => "RT4002",
            // Runtime Interface
            DiagCode::InterfaceNull => "RT5001",
            DiagCode::InterfaceConnect => "RT5002",
            // Runtime Clock
            DiagCode::ClockPeriodViolation => "RT6001",
            DiagCode::ClockGenError => "RT6002",
            // Runtime Assertion
            DiagCode::AssertionFailed => "RT7001",
            DiagCode::AssertionImmediateFailed => "RT7002",
            DiagCode::CoverProperty => "RT7003",
            DiagCode::AssertionDisableError => "RT7004",
            // Runtime DPI
            DiagCode::DpiError => "RT8001",
            DiagCode::DpiImportNotFound => "RT8002",
            DiagCode::DpiScopeError => "RT8003",
            // Runtime Internal
            DiagCode::InternalError => "RT9001",
            DiagCode::Unreachable => "RT9002",
            DiagCode::NotImplemented => "RT9003",
            DiagCode::ResourceLimit => "RT9004",
            // Infrastructure
            DiagCode::PreprocessorError => "E0101",
            DiagCode::DebuggerError => "E0003",
            DiagCode::IoError => "E0004",
            // Other
            DiagCode::WaveformError => "W0001",
            // Legacy Runtime
            DiagCode::SimulationError => "E9001",
            DiagCode::OutOfBounds => "E9002",
            DiagCode::RuntimeTypeMismatch => "E9003",
            // Warnings
            DiagCode::UninitializedRegister => "WR0014",
            DiagCode::WidthMismatchWarning => "WR0102",
            DiagCode::UnusedSignal => "WR0104",
            DiagCode::ClockNeverToggles => "WR0202",
            DiagCode::ResetPermanentlyAsserted => "WR0203",
            DiagCode::CombinationalLoop => "WR0301",
            DiagCode::SignalGlitch => "WR0302",
            DiagCode::TimingViolation => "WR0303",
            DiagCode::SlowSimulation => "WR0402",
        }
    }

    /// Kembalikan deskripsi singkat kode error.
    pub fn description(&self) -> &'static str {
        match self {
            // Parse
            DiagCode::UnexpectedToken => "unexpected token",
            DiagCode::ExpectedToken => "expected token",
            DiagCode::ExpectedSemi => "expected ';'",
            DiagCode::UnclosedBlock => "unclosed block",
            DiagCode::InvalidSyntax => "invalid syntax",
            // Semantic
            DiagCode::UndefinedSignal => "undefined signal",
            DiagCode::TypeMismatch => "type mismatch",
            DiagCode::WidthMismatch => "width mismatch",
            DiagCode::UndefinedVariable => "undefined variable",
            // Elaboration
            DiagCode::ModuleNotFound => "module not found",
            DiagCode::CircularDependency => "circular dependency",
            DiagCode::ParamMismatch => "parameter mismatch",
            DiagCode::InstanceNotFound => "instance not found",
            DiagCode::TopResolutionFailed => "unable to determine top-level design",
            DiagCode::MultipleCandidateTops => "multiple candidate top modules",
            DiagCode::MissingRootModule => "missing root module",
            DiagCode::UnresolvedInstantiation => "unresolved instantiation",
            DiagCode::CircularHierarchy => "circular hierarchy",
            DiagCode::ExcludedByFilelist => "excluded by filelist",
            DiagCode::DuplicateDeclaration => "duplicate definition",
            DiagCode::PackedWidthTooLarge => "packed width too large",
            DiagCode::MultipleContinuousDriver => "multiple continuous drivers on variable",
            // Runtime Memory
            DiagCode::NullHandle => "null handle access",
            DiagCode::InvalidReference => "invalid object reference",
            DiagCode::NullInterface => "null interface dereference",
            DiagCode::MemoryOutOfBounds => "memory out of bounds",
            DiagCode::MailboxError => "mailbox access error",
            // Runtime Signal
            DiagCode::SignalUnknown => "signal entered unknown state (X)",
            DiagCode::SignalFloating => "floating signal detected",
            DiagCode::SignalContention => "driver contention",
            DiagCode::NbaWriteConflict => "NBA write conflict",
            DiagCode::SignalWidthMismatch => "signal width mismatch",
            DiagCode::SignalUninitialized => "uninitialized signal read",
            // Runtime Scheduler
            DiagCode::InfiniteDelta => "infinite delta cycle",
            DiagCode::SchedulerDeadlock => "scheduler deadlock",
            DiagCode::SimulationTimeout => "simulation timeout",
            DiagCode::ForkError => "process fork error",
            DiagCode::MaxDeltaExceeded => "max delta cycles exceeded",
            // Runtime Event
            DiagCode::EventTimeout => "event wait timeout",
            DiagCode::EventOrderViolation => "event order violation",
            DiagCode::EventCreateFailed => "event creation failed",
            // Runtime Module
            DiagCode::ModuleInstantiation => "module instantiation error",
            DiagCode::ModuleBindError => "module binding error",
            // Runtime Interface
            DiagCode::InterfaceNull => "null interface handle",
            DiagCode::InterfaceConnect => "interface connection error",
            // Runtime Clock
            DiagCode::ClockPeriodViolation => "clock period violation",
            DiagCode::ClockGenError => "clock generation error",
            // Runtime Assertion
            DiagCode::AssertionFailed => "assertion failed",
            DiagCode::AssertionImmediateFailed => "immediate assertion failed",
            DiagCode::CoverProperty => "cover property",
            DiagCode::AssertionDisableError => "assertion disable error",
            // Runtime DPI
            DiagCode::DpiError => "DPI function error",
            DiagCode::DpiImportNotFound => "DPI import not found",
            DiagCode::DpiScopeError => "DPI scope error",
            // Runtime Internal
            DiagCode::InternalError => "internal simulator error",
            DiagCode::Unreachable => "unreachable code reached",
            DiagCode::NotImplemented => "not yet implemented",
            DiagCode::ResourceLimit => "resource limit exceeded",
            // Infrastructure
            DiagCode::PreprocessorError => "preprocessor error",
            DiagCode::DebuggerError => "debugger error",
            DiagCode::IoError => "I/O error",
            // Other
            DiagCode::WaveformError => "waveform error",
            // Legacy Runtime
            DiagCode::SimulationError => "simulation error",
            DiagCode::OutOfBounds => "out of bounds",
            DiagCode::RuntimeTypeMismatch => "runtime type mismatch",
            // Warnings
            DiagCode::UninitializedRegister => "uninitialized register",
            DiagCode::WidthMismatchWarning => "width mismatch",
            DiagCode::UnusedSignal => "unused signal",
            DiagCode::ClockNeverToggles => "clock never toggles",
            DiagCode::ResetPermanentlyAsserted => "reset permanently asserted",
            DiagCode::CombinationalLoop => "possible combinational loop",
            DiagCode::SignalGlitch => "signal glitch detected",
            DiagCode::TimingViolation => "timing check violation",
            DiagCode::SlowSimulation => "slow simulation region",
        }
    }

    /// Dapatkan penjelasan panjang tentang penyebab error.
    pub fn explanation(&self) -> &'static str {
        match self {
            DiagCode::UnexpectedToken =>
                "The parser encountered a token that does not match any valid production in the grammar at the current position.",
            DiagCode::ExpectedToken =>
                "A specific token was expected at the current position but a different token was found.",
            DiagCode::ExpectedSemi =>
                "A semicolon (';') is required to terminate the current statement but was not found.",
            DiagCode::UnclosedBlock =>
                "A block (begin/end, module/endmodule, etc.) was opened but never closed.",
            DiagCode::InvalidSyntax =>
                "The source code contains syntax that does not conform to the SystemVerilog language specification.",
            DiagCode::UndefinedSignal =>
                "A signal name was referenced but has not been declared in the current scope.",
            DiagCode::TypeMismatch =>
                "An expression's type does not match the expected type for this context.",
            DiagCode::WidthMismatch =>
                "The bit width of an expression does not match the expected width.",
            DiagCode::UndefinedVariable =>
                "A variable was referenced but has not been declared in the current scope.",
            DiagCode::ModuleNotFound =>
                "A module, interface, or package name could not be found in the design hierarchy.",
            DiagCode::CircularDependency =>
                "A circular dependency was detected during elaboration — module A depends on B which depends on A.",
            DiagCode::ParamMismatch =>
                "A parameter value does not match its declaration type or range constraints.",
            DiagCode::InstanceNotFound =>
                "A module instance could not be found in the design hierarchy.",
            DiagCode::TopResolutionFailed =>
                "The simulator could not determine a valid top-level design module to simulate.",
            DiagCode::MultipleCandidateTops =>
                "Multiple top-level modules match the top-resolution criteria — the design hierarchy is ambiguous.",
            DiagCode::MissingRootModule =>
                "The root module referenced by the filelist or --top option does not exist in the design.",
            DiagCode::UnresolvedInstantiation =>
                "An instantiation references a module, interface, or package that could not be resolved in the design hierarchy.",
            DiagCode::CircularHierarchy =>
                "A circular hierarchy was detected — module A instantiates module B which (directly or indirectly) instantiates module A.",
            DiagCode::ExcludedByFilelist =>
                "A required module was excluded by the filelist, so the design hierarchy is incomplete.",
            DiagCode::DuplicateDeclaration =>
                "A module, interface, or package is defined more than once across the source files — duplicates are removed by keeping the most self-contained definition (fewest unresolved instances); a tie keeps the LAST definition in the file list.",
            DiagCode::PackedWidthTooLarge =>
                "A packed vector or unpacked-array element width exceeds the implementation limit, which would make the design impractical to elaborate or simulate (hang/OOM/incorrect truncation).",
            DiagCode::MultipleContinuousDriver =>
                "A variable (logic/reg/integer) is driven by more than one continuous assignment. IEEE 1800 §10.5 allows only one continuous driver per variable; multiple drivers may be resolved differently by different simulators/engine paths.",
            DiagCode::NullHandle =>
                "An object handle was used (method call or member access) but the handle is null.",
            DiagCode::InvalidReference =>
                "An object reference points to an invalid or deallocated object.",
            DiagCode::NullInterface =>
                "A virtual interface was used before being connected to a physical interface.",
            DiagCode::MemoryOutOfBounds =>
                "A memory access attempted to read or write beyond the declared array bounds.",
            DiagCode::MailboxError =>
                "A mailbox operation failed — the mailbox may be empty, full, or not initialized.",
            DiagCode::SignalUnknown =>
                "A signal entered the unknown (X) state, typically due to uninitialized registers or contention.",
            DiagCode::SignalFloating =>
                "A signal is not driven by any driver and has no default value.",
            DiagCode::SignalContention =>
                "Multiple drivers are attempting to drive the same signal with different values.",
            DiagCode::NbaWriteConflict =>
                "Two or more processes scheduled non-blocking assignments to the same signal in the same delta cycle. The last assignment wins, which may indicate a race condition.",
            DiagCode::SignalWidthMismatch =>
                "A signal assignment width does not match the target signal width at runtime.",
            DiagCode::SignalUninitialized =>
                "A signal was read before any value was assigned to it.",
            DiagCode::InfiniteDelta =>
                "Simulation cannot advance past the current time step due to a zero-delay combinational loop.",
            DiagCode::SchedulerDeadlock =>
                "The scheduler detected a deadlock — no processes can proceed.",
            DiagCode::SimulationTimeout =>
                "The simulation exceeded the maximum time limit.",
            DiagCode::ForkError =>
                "A fork/join operation failed to create or manage parallel processes.",
            DiagCode::MaxDeltaExceeded =>
                "The simulation exceeded the maximum number of delta cycles without advancing time.",
            DiagCode::EventTimeout =>
                "An event wait operation timed out before the event was triggered.",
            DiagCode::EventOrderViolation =>
                "Events were triggered or waited in an invalid order.",
            DiagCode::EventCreateFailed =>
                "Failed to create a new event object.",
            DiagCode::ModuleInstantiation =>
                "A module could not be instantiated — check port connections and parameter values.",
            DiagCode::ModuleBindError =>
                "A module binding operation failed — the port or interface could not be connected.",
            DiagCode::InterfaceNull =>
                "A null interface handle was accessed.",
            DiagCode::InterfaceConnect =>
                "An interface connection could not be established.",
            DiagCode::ClockPeriodViolation =>
                "A clock period constraint was violated during simulation.",
            DiagCode::ClockGenError =>
                "Clock generation failed — check clock parameters and dependencies.",
            DiagCode::AssertionFailed =>
                "A concurrent assertion (assert property) evaluated to false.",
            DiagCode::AssertionImmediateFailed =>
                "An immediate assertion (assert) evaluated to false.",
            DiagCode::CoverProperty =>
                "A cover property hit its target count or reported a coverage event.",
            DiagCode::AssertionDisableError =>
                "An assertion disable operation failed.",
            DiagCode::DpiError =>
                "A DPI (Direct Programming Interface) function call encountered an error.",
            DiagCode::DpiImportNotFound =>
                "A DPI imported function could not be found in the loaded shared libraries.",
            DiagCode::DpiScopeError =>
                "A DPI scope operation failed — check that the scope is valid.",
            DiagCode::InternalError =>
                "An internal simulator error occurred — this is a bug in the simulator itself.",
            DiagCode::Unreachable =>
                "The simulator reached code that should be unreachable — this indicates a bug.",
            DiagCode::NotImplemented =>
                "A SystemVerilog feature used in the design is not yet implemented in this simulator.",
            DiagCode::PreprocessorError =>
                "The preprocessor encountered an error while processing `include, `define, or conditional compilation directives.",
            DiagCode::DebuggerError =>
                "The debugger encountered an error during stepping, breakpoints, or reverse debugging.",
            DiagCode::IoError =>
                "An I/O operation failed — the file may not exist, may be unreadable, or the disk may be full.",
            DiagCode::WaveformError =>
                "A waveform operation failed — the VCD/FST file could not be written or read.",
            DiagCode::SimulationError =>
                "A general simulation error occurred — check the message for details.",
            DiagCode::OutOfBounds =>
                "An array or memory access went out of bounds.",
            DiagCode::RuntimeTypeMismatch =>
                "A type mismatch was detected at runtime.",
            DiagCode::UninitializedRegister =>
                "A register was read before being assigned a value (retains the default X).",
            DiagCode::WidthMismatchWarning =>
                "A signal or expression width does not match the expected width.",
            DiagCode::UnusedSignal =>
                "A signal never changed during the simulated time window — it is either unused, held at a constant value, or the run ended before any transition.",
            DiagCode::ClockNeverToggles =>
                "A signal identified as a clock never changed after time 0 — the driving clock module may be unreachable, or the simulation ended before the first edge.",
            DiagCode::ResetPermanentlyAsserted =>
                "A reset signal never changed after time 0 — it may be stuck asserted (polarity rst vs rst_n must be checked manually).",
            DiagCode::CombinationalLoop =>
                "A potential combinational loop was detected in the design.",
            DiagCode::SignalGlitch =>
                "A signal changed value and reverted to its previous value within a very short time window, indicating a glitch or race condition.",
            DiagCode::TimingViolation =>
                "A timing check (setup, hold, width, period, recovery, removal, skew, etc.) was violated during simulation.",
            DiagCode::SlowSimulation =>
                "A simulation region or process is significantly slower than others.",
            DiagCode::ResourceLimit =>
                "The simulation exceeded the configured memory limit and was stopped before the kernel could OOM-kill the process. Reduce the design scope, cap simulation time (-T), or raise MIVON_SIM_MEM_LIMIT_MB if RAM is available.",
        }
    }

    /// Dapatkan saran perbaikan untuk error ini.
    pub fn help(&self) -> &'static str {
        match self {
            DiagCode::UnexpectedToken =>
                "Review the syntax near the indicated position and check for typos or missing operators.",
            DiagCode::ExpectedToken =>
                "Check the code near the indicated position for missing keywords or operators.",
            DiagCode::ExpectedSemi =>
                "Add a semicolon at the end of the statement.",
            DiagCode::UnclosedBlock =>
                "Add the closing keyword (end, endmodule, endinterface, etc.) for the block.",
            DiagCode::InvalidSyntax =>
                "Review the code at the indicated location and fix the reported issue.",
            DiagCode::UndefinedSignal =>
                "Declare the signal before using it, or check for typos in the signal name.",
            DiagCode::TypeMismatch =>
                "Use a type conversion or change the expression type to match the expected type.",
            DiagCode::WidthMismatch =>
                "Adjust the bit width of the expression or use a width extension/truncation.",
            DiagCode::UndefinedVariable =>
                "Declare the variable before using it, or check for typos in the variable name.",
            DiagCode::ModuleNotFound =>
                "Check that the module/package name is spelled correctly and that all source files are included.",
            DiagCode::CircularDependency =>
                "Restructure the design to eliminate circular dependencies between modules.",
            DiagCode::ParamMismatch =>
                "Verify parameter values match the declared types and ranges in the module definition.",
            DiagCode::InstanceNotFound =>
                "Check that the instance name is correct and that the module is properly instantiated.",
            DiagCode::TopResolutionFailed =>
                "Check that the top module name is correct and all its dependencies are resolved.",
            DiagCode::MultipleCandidateTops =>
                "Specify the top module explicitly with --top <name>, or remove duplicate module definitions from the filelist.",
            DiagCode::MissingRootModule =>
                "Verify the top/root module name and ensure the source file that defines it is included in the filelist.",
            DiagCode::UnresolvedInstantiation =>
                "Check that the instantiated module/interface is defined and that its source file is included in the filelist.",
            DiagCode::CircularHierarchy =>
                "Restructure the design to remove recursive instantiation between modules.",
            DiagCode::ExcludedByFilelist =>
                "Add the missing source file to the filelist, or remove the exclusion directive that dropped the module.",
            DiagCode::DuplicateDeclaration =>
                "Remove the duplicate definition, or reorder the file list so the intended definition comes last.",
            DiagCode::PackedWidthTooLarge =>
                "Reduce the vector width (e.g. parameterized width expression) below the limit, or restructure the design into smaller packed vectors.",
            DiagCode::MultipleContinuousDriver =>
                "Keep only one continuous assignment driving the variable: merge the assignments, or declare the signal as a net (wire/tri) if multiple drivers with resolution are intended.",
            DiagCode::NullHandle =>
                "Initialize the object handle with 'new()' before accessing its members.",
            DiagCode::InvalidReference =>
                "Ensure the object reference points to a valid, allocated object.",
            DiagCode::NullInterface =>
                "Connect the virtual interface to a physical interface instance before simulation.",
            DiagCode::MemoryOutOfBounds =>
                "Check array bounds and ensure indices are within the declared range.",
            DiagCode::MailboxError =>
                "Initialize the mailbox with 'new()' and check that it is not full/empty for put/get operations.",
            DiagCode::SignalUnknown =>
                "Initialize all registers and avoid multiple drivers driving the same signal.",
            DiagCode::SignalFloating =>
                "Assign a default value or drive the signal from a valid driver.",
            DiagCode::SignalContention =>
                "Check for multiple drivers on the same signal and resolve the conflict.",
            DiagCode::NbaWriteConflict =>
                "Review the scheduling of non-blocking assignments. If intentional, use a single NBA per signal per delta cycle.",
            DiagCode::SignalWidthMismatch =>
                "Adjust the width of the assignment to match the target signal.",
            DiagCode::SignalUninitialized =>
                "Initialize the signal in the reset block or at declaration before reading it.",
            DiagCode::InfiniteDelta =>
                "Add non-zero delays to break the combinational loop, or use a flip-flop.",
            DiagCode::SchedulerDeadlock =>
                "Check for processes waiting on events that never trigger, or circular wait conditions.",
            DiagCode::SimulationTimeout =>
                "Increase the simulation time limit or check for infinite loops in the design.",
            DiagCode::ForkError =>
                "Check fork/join syntax and ensure all branches are valid processes.",
            DiagCode::MaxDeltaExceeded =>
                "Add delays to combinational logic paths or restructure the design to avoid delta cycles.",
            DiagCode::EventTimeout =>
                "Check that the event is triggered within the expected time frame.",
            DiagCode::EventOrderViolation =>
                "Restructure event trigger/wait sequences to maintain proper ordering.",
            DiagCode::EventCreateFailed =>
                "Check system resources and ensure events are properly initialized.",
            DiagCode::ModuleInstantiation =>
                "Check port connections, parameter values, and module definition for mismatches.",
            DiagCode::ModuleBindError =>
                "Verify the module/interface bind directives and port mapping.",
            DiagCode::InterfaceNull =>
                "Initialize the interface handle before accessing its members.",
            DiagCode::InterfaceConnect =>
                "Check interface connections between modules and verify port matching.",
            DiagCode::ClockPeriodViolation =>
                "Adjust clock generation parameters to meet timing constraints.",
            DiagCode::ClockGenError =>
                "Check clock generation logic and ensure all clock parameters are valid.",
            DiagCode::AssertionFailed =>
                "Review the assertion condition and check if the design behavior is correct.",
            DiagCode::AssertionImmediateFailed =>
                "Review the immediate assertion and fix the violation in the design.",
            DiagCode::CoverProperty =>
                "Review coverage results to ensure all cover properties are hit.",
            DiagCode::AssertionDisableError =>
                "Check assertion disable directives and ensure proper usage.",
            DiagCode::DpiError =>
                "Check DPI function arguments, return types, and shared library compatibility.",
            DiagCode::DpiImportNotFound =>
                "Ensure the shared library is loaded and the function name matches exactly (case-sensitive).",
            DiagCode::DpiScopeError =>
                "Check DPI scope setup and ensure the scope is active before making DPI calls.",
            DiagCode::InternalError =>
                "Please report this bug to the simulator developers with a minimal reproduction test case.",
            DiagCode::Unreachable =>
                "Please report this bug to the simulator developers with the design files.",
            DiagCode::NotImplemented =>
                "Consider using an alternative coding style or wait for the feature to be implemented.",
            DiagCode::PreprocessorError =>
                "Check that all `include files exist, `define macros are properly declared, and the file is readable.",
            DiagCode::DebuggerError =>
                "Check debugger settings, breakpoints, and snapshot configuration.",
            DiagCode::IoError =>
                "Check file paths, permissions, and disk space. Verify the file exists and is accessible.",
            DiagCode::WaveformError =>
                "Check disk space, file path permissions, and waveform configuration.",
            DiagCode::SimulationError =>
                "Review the error message and check the code at the indicated location.",
            DiagCode::OutOfBounds =>
                "Ensure array indices are within the declared bounds before accessing them.",
            DiagCode::RuntimeTypeMismatch =>
                "Use type casting or convert the expression to the expected type.",
            DiagCode::UninitializedRegister =>
                "Assign a reset value to the register, or ignore this warning if X-initialization is intentional (e.g. X-propagation checks).",
            DiagCode::WidthMismatchWarning =>
                "Adjust the bit width to match or use explicit width conversion.",
            DiagCode::UnusedSignal =>
                "If the signal is genuinely unused, remove it; otherwise it simply never changed (constant value) or the simulation ended before any transition.",
            DiagCode::ClockNeverToggles =>
                "Check clock generation, or increase simulation time (-T) so the first clock edge falls inside the run.",
            DiagCode::ResetPermanentlyAsserted =>
                "Check that reset is driven and de-asserts after initialization, and verify polarity (rst vs rst_n).",
            DiagCode::CombinationalLoop =>
                "Add flip-flops or non-zero delays to break the potential combinational loop.",
            DiagCode::SignalGlitch =>
                "Check for race conditions, missing delays, or combinational loops causing narrow pulses on the signal.",
            DiagCode::TimingViolation =>
                "Review the timing constraints in the specify block or SDF annotation, and ensure data transitions respect setup/hold/window requirements.",
            DiagCode::SlowSimulation =>
                "Optimize the identified region or process to improve simulation performance.",
            DiagCode::ResourceLimit =>
                "Run the simulation with a bounded -T time, stream the waveform (--waveform-stream), disable unneeded VCD/FST, or increase MIVON_SIM_MEM_LIMIT_MB if the machine has spare RAM.",
        }
    }

    /// Dapatkan kategori kode error.
    pub fn category(&self) -> &'static str {
        match self {
            DiagCode::UnexpectedToken
            | DiagCode::ExpectedToken
            | DiagCode::ExpectedSemi
            | DiagCode::UnclosedBlock
            | DiagCode::InvalidSyntax => "Parse",
            DiagCode::UndefinedSignal
            | DiagCode::TypeMismatch
            | DiagCode::WidthMismatch
            | DiagCode::UndefinedVariable => "Semantic",
            DiagCode::ModuleNotFound
            | DiagCode::CircularDependency
            | DiagCode::ParamMismatch
            | DiagCode::InstanceNotFound
            | DiagCode::TopResolutionFailed
            | DiagCode::MultipleCandidateTops
            | DiagCode::MissingRootModule
            | DiagCode::UnresolvedInstantiation
            | DiagCode::CircularHierarchy
            | DiagCode::ExcludedByFilelist
            | DiagCode::DuplicateDeclaration
            | DiagCode::PackedWidthTooLarge
            | DiagCode::MultipleContinuousDriver => "Elaboration",
            DiagCode::NullHandle
            | DiagCode::InvalidReference
            | DiagCode::NullInterface
            | DiagCode::MemoryOutOfBounds
            | DiagCode::MailboxError => "Memory",
            DiagCode::SignalUnknown
            | DiagCode::SignalFloating
            | DiagCode::SignalContention
            | DiagCode::NbaWriteConflict
            | DiagCode::SignalWidthMismatch
            | DiagCode::SignalUninitialized => "Signal",
            DiagCode::InfiniteDelta
            | DiagCode::SchedulerDeadlock
            | DiagCode::SimulationTimeout
            | DiagCode::ForkError
            | DiagCode::MaxDeltaExceeded => "Scheduler",
            DiagCode::EventTimeout
            | DiagCode::EventOrderViolation
            | DiagCode::EventCreateFailed => "Event",
            DiagCode::ModuleInstantiation | DiagCode::ModuleBindError => "Module",
            DiagCode::InterfaceNull | DiagCode::InterfaceConnect => "Interface",
            DiagCode::ClockPeriodViolation | DiagCode::ClockGenError => "Clock",
            DiagCode::AssertionFailed
            | DiagCode::AssertionImmediateFailed
            | DiagCode::CoverProperty
            | DiagCode::AssertionDisableError => "Assertion",
            DiagCode::DpiError | DiagCode::DpiImportNotFound | DiagCode::DpiScopeError => "DPI",
            DiagCode::InternalError
            | DiagCode::Unreachable
            | DiagCode::NotImplemented
            | DiagCode::ResourceLimit => "Internal",
            DiagCode::WaveformError => "Waveform",
            DiagCode::PreprocessorError | DiagCode::DebuggerError | DiagCode::IoError => {
                "Infrastructure"
            }
            DiagCode::SimulationError | DiagCode::OutOfBounds | DiagCode::RuntimeTypeMismatch => {
                "Runtime"
            }
            DiagCode::UninitializedRegister
            | DiagCode::WidthMismatchWarning
            | DiagCode::UnusedSignal
            | DiagCode::ClockNeverToggles
            | DiagCode::ResetPermanentlyAsserted
            | DiagCode::CombinationalLoop
            | DiagCode::SignalGlitch
            | DiagCode::TimingViolation
            | DiagCode::SlowSimulation => "Warning",
        }
    }
}

impl fmt::Display for DiagCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ─── Diagnostic Span ───

pub use super::span::{DiagNote, DiagSpan};

pub use super::runtime::RuntimeContext;

pub use super::snippet::SourceSnippet;

pub use super::record::Diagnostic;
pub use super::sink::DiagSink;

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intern::Symbol;

    #[test]
    fn test_diagnostic_basic() {
        let d = Diagnostic::error(DiagCode::UnexpectedToken, "found 'foo' where ';' expected");
        assert!(d.is_error());
        assert_eq!(d.level, DiagLevel::Error);
        assert_eq!(d.code, DiagCode::UnexpectedToken);
    }

    #[test]
    fn test_diagnostic_fatal() {
        let d = Diagnostic::fatal(DiagCode::NullHandle, "null handle access");
        assert!(d.is_error());
        assert!(d.is_fatal());
        assert_eq!(d.level, DiagLevel::Fatal);
    }

    #[test]
    fn test_diagnostic_with_source_snippet() {
        let snippet = SourceSnippet::new("test.sv", 42, 14, "fifo.push(data);")
            .with_label("Handle \"fifo\" is null");
        let d = Diagnostic::error(DiagCode::NullHandle, "null handle access")
            .with_source_snippet(snippet);
        assert!(d.source_snippet.is_some());
        let formatted = d.source_snippet.as_ref().unwrap().format(false);
        assert!(formatted.contains("test.sv:42:14"));
        assert!(formatted.contains("fifo.push(data);"));
        assert!(formatted.contains("^"));
    }

    #[test]
    fn test_diagnostic_with_runtime_context() {
        let ctx = RuntimeContext::new()
            .with_time("125 ns")
            .with_delta(12)
            .with_module("uart_top")
            .with_process("write_task");
        let d =
            Diagnostic::error(DiagCode::NullHandle, "null handle access").with_runtime_context(ctx);
        assert!(d.runtime_context.is_some());
        let formatted = d.runtime_context.as_ref().unwrap().format();
        assert!(formatted.contains("uart_top"));
        assert!(formatted.contains("125 ns"));
        assert!(formatted.contains("write_task"));
    }

    #[test]
    fn test_diagnostic_with_explanation() {
        let d = Diagnostic::error(DiagCode::NullHandle, "null handle access")
            .with_explanation("The FIFO handle was never initialized before simulation started.");
        assert_eq!(
            d.explanation.as_ref().unwrap().as_ref(),
            "The FIFO handle was never initialized before simulation started."
        );
    }

    #[test]
    fn test_diagnostic_with_suggestion() {
        let d = Diagnostic::error(DiagCode::NullHandle, "null handle access")
            .with_suggestion("Initialize fifo before use: fifo = new();");
        assert_eq!(
            d.suggestion.as_ref().unwrap().as_ref(),
            "Initialize fifo before use: fifo = new();"
        );
    }

    #[test]
    fn test_diagnostic_with_span() {
        let file = Symbol::intern("test.sv");
        let d = Diagnostic::error(DiagCode::UndefinedSignal, "signal 'foo' not found")
            .with_span(DiagSpan::new(file, 10, 13).with_label("here"));
        assert_eq!(d.spans.len(), 1);
    }

    #[test]
    fn test_diag_sink_push_flush() {
        let sink = DiagSink::new();
        sink.push(Diagnostic::error(DiagCode::UnexpectedToken, "bad token"));
        sink.push(Diagnostic::warning(
            DiagCode::WidthMismatch,
            "width differs",
        ));

        let diags = sink.diagnostics();
        assert_eq!(diags.len(), 2);
        assert_eq!(sink.error_count(), 1);
    }

    #[test]
    fn test_diag_code_display() {
        assert_eq!(DiagCode::UnexpectedToken.as_str(), "E1001");
        assert_eq!(DiagCode::ModuleNotFound.as_str(), "E3001");
        assert_eq!(DiagCode::NullHandle.as_str(), "RT0001");
        assert_eq!(DiagCode::SignalUnknown.as_str(), "RT1001");
        assert_eq!(DiagCode::InfiniteDelta.as_str(), "RT2001");
        assert_eq!(DiagCode::AssertionFailed.as_str(), "RT7001");
        assert_eq!(DiagCode::UninitializedRegister.as_str(), "WR0014");
        assert_eq!(DiagCode::WidthMismatchWarning.as_str(), "WR0102");
    }

    #[test]
    fn test_diag_level_display() {
        assert_eq!(DiagLevel::Fatal.as_str(), "fatal");
        assert_eq!(DiagLevel::Error.as_str(), "error");
        assert_eq!(DiagLevel::Warning.as_str(), "warning");
        assert_eq!(DiagLevel::Note.as_str(), "note");
        assert_eq!(DiagLevel::Info.as_str(), "info");
        assert_eq!(DiagLevel::Help.as_str(), "help");
        assert_eq!(DiagLevel::Trace.as_str(), "trace");
        assert_eq!(DiagLevel::Debug.as_str(), "debug");
    }

    #[test]
    fn test_diag_code_category() {
        assert_eq!(DiagCode::NullHandle.category(), "Memory");
        assert_eq!(DiagCode::SignalUnknown.category(), "Signal");
        assert_eq!(DiagCode::InfiniteDelta.category(), "Scheduler");
        assert_eq!(DiagCode::AssertionFailed.category(), "Assertion");
        assert_eq!(DiagCode::UninitializedRegister.category(), "Warning");
    }

    #[test]
    fn test_source_snippet_format() {
        let snippet = SourceSnippet::new("cpu_top.sv", 182, 17, "    axi.read(addr);")
            .with_label("interface is null");
        let formatted = snippet.format(false);
        assert!(formatted.contains("cpu_top.sv:182:17"));
        assert!(formatted.contains("axi.read(addr);"));
        // The pointer should be at the right column
        assert!(formatted.contains("^"));
    }

    /// Caret harus mendarat tepat di kolom `snippet.col` pada baris source.
    /// Ukur berbasis KARAKTER (prefix `"     │ "` = 7 char; `│` multi-byte di
    /// UTF-8 sehingga `- 7` berbasis byte akan salah).
    /// Regresi: `for _ in 1..col.saturating_sub(1)` = col-2 spasi membuat
    /// caret meleset 1 kolom ke kiri (tetap lolos `contains("^")`).
    fn caret_col_of(snippet: &SourceSnippet) -> usize {
        let formatted = snippet.format(false);
        let caret_line = formatted
            .lines()
            .find(|l| l.contains('^'))
            .expect("harus ada baris caret");
        caret_line.chars().take_while(|&c| c != '^').count() - 7 + 1
    }

    #[test]
    fn test_source_snippet_caret_col_one() {
        assert_eq!(
            caret_col_of(&SourceSnippet::new("t.sv", 3, 1, "module x;")),
            1
        );
    }

    #[test]
    fn test_source_snippet_caret_col_multi_digit() {
        assert_eq!(
            caret_col_of(&SourceSnippet::new("t.sv", 3, 20, "  assign y = bad + ;")),
            20
        );
    }

    #[test]
    fn test_source_snippet_caret_col_deep() {
        assert_eq!(
            caret_col_of(&SourceSnippet::new("t.sv", 42, 17, "    axi.read(addr);")),
            17
        );
    }

    #[test]
    fn test_runtime_context_format() {
        let ctx = RuntimeContext::new()
            .with_module("cpu_top")
            .with_process("fetch_stage")
            .with_time("125 ns")
            .with_thread("worker-2")
            .with_delta(12)
            .with_seed(129381293);

        let formatted = ctx.format();
        assert!(formatted.contains("module : cpu_top"));
        assert!(formatted.contains("process: fetch_stage"));
        assert!(formatted.contains("time   : 125 ns"));
        assert!(formatted.contains("thread : worker-2"));
        assert!(formatted.contains("delta  : 12"));
        assert!(formatted.contains("seed   : 129381293"));
    }
}
