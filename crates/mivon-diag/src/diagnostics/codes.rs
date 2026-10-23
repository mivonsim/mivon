//! Error codes — centralized error code definitions.
//!
//! Kategori kode:
//! - E1xxx: Parse errors
//! - E2xxx: Semantic errors
//! - E3xxx: Elaboration errors
//! - RT0xxx: Memory runtime errors
//! - RT1xxx: Signal runtime errors
//! - RT2xxx: Scheduler runtime errors
//! - RT3xxx: Event runtime errors
//! - RT4xxx: Module runtime errors
//! - RT5xxx: Interface runtime errors
//! - RT6xxx: Clock runtime errors
//! - RT7xxx: Assertion runtime errors
//! - RT8xxx: DPI runtime errors
//! - RT9xxx: Internal runtime errors
//! - WRxxxx: Warnings
//! - E9xxx: Legacy runtime errors

pub use super::diagnostic::DiagCode;

/// All parse error codes (E1xxx)
pub const PARSE_ERRORS: &[(DiagCode, &str)] = &[
    (DiagCode::UnexpectedToken, "unexpected token"),
    (DiagCode::ExpectedToken, "expected token"),
    (DiagCode::ExpectedSemi, "expected ';'"),
    (DiagCode::UnclosedBlock, "unclosed block"),
    (DiagCode::InvalidSyntax, "invalid syntax"),
];

/// All semantic error codes (E2xxx)
pub const SEMANTIC_ERRORS: &[(DiagCode, &str)] = &[
    (DiagCode::UndefinedSignal, "undefined signal"),
    (DiagCode::TypeMismatch, "type mismatch"),
    (DiagCode::WidthMismatch, "width mismatch"),
    (DiagCode::UndefinedVariable, "undefined variable"),
];

/// All elaboration error codes (E3xxx)
pub const ELAB_ERRORS: &[(DiagCode, &str)] = &[
    (DiagCode::ModuleNotFound, "module not found"),
    (DiagCode::CircularDependency, "circular dependency"),
    (DiagCode::ParamMismatch, "parameter mismatch"),
    (DiagCode::InstanceNotFound, "instance not found"),
    (
        DiagCode::TopResolutionFailed,
        "unable to determine top-level design",
    ),
    (
        DiagCode::MultipleCandidateTops,
        "multiple candidate top modules",
    ),
    (DiagCode::MissingRootModule, "missing root module"),
    (
        DiagCode::UnresolvedInstantiation,
        "unresolved instantiation",
    ),
    (DiagCode::CircularHierarchy, "circular hierarchy"),
    (DiagCode::ExcludedByFilelist, "excluded by filelist"),
    (DiagCode::DuplicateDeclaration, "duplicate definition"),
];

/// All runtime memory error codes (RT0xxx)
pub const RUNTIME_MEMORY_ERRORS: &[(DiagCode, &str)] = &[
    (DiagCode::NullHandle, "null handle access"),
    (DiagCode::InvalidReference, "invalid object reference"),
    (DiagCode::NullInterface, "null interface dereference"),
    (DiagCode::MemoryOutOfBounds, "memory out of bounds"),
    (DiagCode::MailboxError, "mailbox access error"),
];

/// All runtime signal error codes (RT1xxx)
pub const RUNTIME_SIGNAL_ERRORS: &[(DiagCode, &str)] = &[
    (DiagCode::SignalUnknown, "signal entered unknown state (X)"),
    (DiagCode::SignalFloating, "floating signal detected"),
    (DiagCode::SignalContention, "driver contention"),
    (DiagCode::SignalWidthMismatch, "signal width mismatch"),
    (DiagCode::SignalUninitialized, "uninitialized signal read"),
];

/// All runtime scheduler error codes (RT2xxx)
pub const RUNTIME_SCHEDULER_ERRORS: &[(DiagCode, &str)] = &[
    (DiagCode::InfiniteDelta, "infinite delta cycle"),
    (DiagCode::SchedulerDeadlock, "scheduler deadlock"),
    (DiagCode::SimulationTimeout, "simulation timeout"),
    (DiagCode::ForkError, "process fork error"),
    (DiagCode::MaxDeltaExceeded, "max delta cycles exceeded"),
];

/// All runtime event error codes (RT3xxx)
pub const RUNTIME_EVENT_ERRORS: &[(DiagCode, &str)] = &[
    (DiagCode::EventTimeout, "event wait timeout"),
    (DiagCode::EventOrderViolation, "event order violation"),
    (DiagCode::EventCreateFailed, "event creation failed"),
];

/// All runtime module error codes (RT4xxx)
pub const RUNTIME_MODULE_ERRORS: &[(DiagCode, &str)] = &[
    (DiagCode::ModuleInstantiation, "module instantiation error"),
    (DiagCode::ModuleBindError, "module binding error"),
];

/// All runtime interface error codes (RT5xxx)
pub const RUNTIME_INTERFACE_ERRORS: &[(DiagCode, &str)] = &[
    (DiagCode::InterfaceNull, "null interface handle"),
    (DiagCode::InterfaceConnect, "interface connection error"),
];

/// All runtime clock error codes (RT6xxx)
pub const RUNTIME_CLOCK_ERRORS: &[(DiagCode, &str)] = &[
    (DiagCode::ClockPeriodViolation, "clock period violation"),
    (DiagCode::ClockGenError, "clock generation error"),
];

/// All runtime assertion error codes (RT7xxx)
pub const RUNTIME_ASSERTION_ERRORS: &[(DiagCode, &str)] = &[
    (DiagCode::AssertionFailed, "assertion failed"),
    (
        DiagCode::AssertionImmediateFailed,
        "immediate assertion failed",
    ),
    (DiagCode::CoverProperty, "cover property"),
    (DiagCode::AssertionDisableError, "assertion disable error"),
];

/// All runtime DPI error codes (RT8xxx)
pub const RUNTIME_DPI_ERRORS: &[(DiagCode, &str)] = &[
    (DiagCode::DpiError, "DPI function error"),
    (DiagCode::DpiImportNotFound, "DPI import not found"),
    (DiagCode::DpiScopeError, "DPI scope error"),
];

/// All runtime internal error codes (RT9xxx)
pub const RUNTIME_INTERNAL_ERRORS: &[(DiagCode, &str)] = &[
    (DiagCode::InternalError, "internal simulator error"),
    (DiagCode::Unreachable, "unreachable code reached"),
    (DiagCode::NotImplemented, "not yet implemented"),
];

/// All infrastructure error codes (E0xxx)
pub const INFRASTRUCTURE_ERRORS: &[(DiagCode, &str)] = &[
    (DiagCode::PreprocessorError, "preprocessor error"),
    (DiagCode::DebuggerError, "debugger error"),
    (DiagCode::IoError, "I/O error"),
];

/// All legacy runtime error codes (E9xxx)
pub const LEGACY_RUNTIME_ERRORS: &[(DiagCode, &str)] = &[
    (DiagCode::SimulationError, "simulation error"),
    (DiagCode::OutOfBounds, "out of bounds"),
    (DiagCode::RuntimeTypeMismatch, "runtime type mismatch"),
];

/// Other codes
pub const OTHER_CODES: &[(DiagCode, &str)] = &[(DiagCode::WaveformError, "waveform error")];

/// All warning codes (WRxxxx)
pub const WARNING_CODES: &[(DiagCode, &str)] = &[
    (DiagCode::UninitializedRegister, "uninitialized register"),
    (DiagCode::WidthMismatchWarning, "width mismatch"),
    (DiagCode::UnusedSignal, "unused signal"),
    (DiagCode::ClockNeverToggles, "clock never toggles"),
    (
        DiagCode::ResetPermanentlyAsserted,
        "reset permanently asserted",
    ),
    (DiagCode::CombinationalLoop, "possible combinational loop"),
    (DiagCode::SignalGlitch, "signal glitch detected"),
    (DiagCode::TimingViolation, "timing check violation"),
    (DiagCode::SlowSimulation, "slow simulation region"),
];

/// Get all error codes.
pub fn all_codes() -> Vec<(DiagCode, &'static str)> {
    PARSE_ERRORS
        .iter()
        .chain(SEMANTIC_ERRORS.iter())
        .chain(ELAB_ERRORS.iter())
        .chain(RUNTIME_MEMORY_ERRORS.iter())
        .chain(RUNTIME_SIGNAL_ERRORS.iter())
        .chain(RUNTIME_SCHEDULER_ERRORS.iter())
        .chain(RUNTIME_EVENT_ERRORS.iter())
        .chain(RUNTIME_MODULE_ERRORS.iter())
        .chain(RUNTIME_INTERFACE_ERRORS.iter())
        .chain(RUNTIME_CLOCK_ERRORS.iter())
        .chain(RUNTIME_ASSERTION_ERRORS.iter())
        .chain(RUNTIME_DPI_ERRORS.iter())
        .chain(RUNTIME_INTERNAL_ERRORS.iter())
        .chain(LEGACY_RUNTIME_ERRORS.iter())
        .chain(INFRASTRUCTURE_ERRORS.iter())
        .chain(OTHER_CODES.iter())
        .chain(WARNING_CODES.iter())
        .cloned()
        .collect()
}

/// Lookup error code by number string.
pub fn lookup_code(code_str: &str) -> Option<DiagCode> {
    all_codes()
        .iter()
        .find(|(c, _)| c.as_str() == code_str)
        .map(|(c, _)| *c)
}

/// Dapatkan kode-kode untuk kategori tertentu.
pub fn codes_by_category(category: &str) -> Vec<(DiagCode, &'static str)> {
    all_codes()
        .into_iter()
        .filter(|(c, _)| c.category() == category)
        .collect()
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_codes_count() {
        let codes = all_codes();
        // Parse: 5, Semantic: 4, Elab: 11, Memory: 5, Signal: 5, Scheduler: 5,
        // Event: 3, Module: 2, Interface: 2, Clock: 2, Assertion: 4, DPI: 3,
        // Internal: 3, Infrastructure: 3, Legacy: 3, Other: 1, Warnings: 9 = 70 total
        assert_eq!(codes.len(), 70);
    }

    #[test]
    fn test_lookup_code() {
        assert_eq!(lookup_code("E1001"), Some(DiagCode::UnexpectedToken));
        assert_eq!(lookup_code("E3001"), Some(DiagCode::ModuleNotFound));
        // E3005 = top resolution (dahulu prefix aneh "EL3001" di luar skema
        // E3xxx — user tak menemukannya di dokumentasi kode E3xxx)
        assert_eq!(lookup_code("E3005"), Some(DiagCode::TopResolutionFailed));
        assert_eq!(lookup_code("EL3001"), None);
        assert_eq!(lookup_code("RT0001"), Some(DiagCode::NullHandle));
        assert_eq!(lookup_code("RT1001"), Some(DiagCode::SignalUnknown));
        assert_eq!(lookup_code("RT2001"), Some(DiagCode::InfiniteDelta));
        assert_eq!(lookup_code("RT7001"), Some(DiagCode::AssertionFailed));
        assert_eq!(lookup_code("WR0014"), Some(DiagCode::UninitializedRegister));
        assert_eq!(lookup_code("E9999"), None);
        assert_eq!(lookup_code("RT9999"), None);
    }

    #[test]
    fn test_codes_by_category() {
        let memory_codes = codes_by_category("Memory");
        assert_eq!(memory_codes.len(), 5);

        let signal_codes = codes_by_category("Signal");
        assert_eq!(signal_codes.len(), 5);

        let warning_codes = codes_by_category("Warning");
        assert_eq!(warning_codes.len(), 9);
    }
}
