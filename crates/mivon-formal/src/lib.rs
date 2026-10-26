//! Formal Verification Engine — IEEE 1800-2012 Assertion Checking.
//!
//! Integrates Z3 SMT solver for:
//! - Bounded Model Checking (BMC): unroll transition relation up to bound k
//! - Induction-based proof: base case + inductive step for simple properties
//! - SAT/SMT bridge: SystemVerilog expressions → Z3 bit-vector formulas
//! - Equivalence checking (COMP-16 tahap 1, kombinasional): `sec.rs`
//!
//! Uses z3 0.20 thread-local context — constructors take no `&Context` argument.
//! Usage: `cargo run -- --formal --formal-bound 10 test.sv`

pub mod abstract_interp;
pub mod bmc;
pub mod connectivity;
pub mod sat;
pub mod sec;
pub mod xprop;

#[cfg(test)]
pub mod tests;

/// Result of a formal property check.
#[derive(Debug, Clone, PartialEq)]
pub enum FormalResult {
    /// Property holds for all states up to bound k (BMC pass)
    Pass,
    /// Counterexample found at given depth
    Counterexample(u64),
    /// Induction proof successful at induction depth k
    /// (property holds for ALL depths — unbounded proof)
    InductiveProof(u64),
    /// Formal check inconclusive (timeout or undecidable)
    Unknown,
    /// Error during formal check
    Error(String),
}

/// Configuration for formal verification.
#[derive(Debug, Clone)]
pub struct FormalConfig {
    /// Maximum unrolling bound for BMC
    pub bound: u64,
    /// Timeout for each solver query (seconds)
    pub timeout: u64,
    /// Enable induction-based proof (requires BMC base case)
    pub induction: bool,
    /// Maximum induction depth k to try (k-induction iterates k = 1..max_k;
    /// property yang tidak terbukti di k sering terbukti di k lebih dalam)
    pub max_k: u64,
    /// Only check assertions (ignore cover/property)
    pub assert_only: bool,
}

impl Default for FormalConfig {
    fn default() -> Self {
        FormalConfig {
            bound: 20,
            timeout: 30,
            induction: false,
            max_k: 8,
            assert_only: true,
        }
    }
}

/// The formal verification engine, wrapping Z3 solver.
///
/// Uses z3's thread-local context — all constructors access
/// `Context::thread_local()` automatically.
pub struct FormalEngine {
    pub config: FormalConfig,
    pub solver: Option<z3::Solver>,
}

impl FormalEngine {
    pub fn new(config: FormalConfig) -> Self {
        FormalEngine {
            config,
            solver: None,
        }
    }

    /// Initialize Z3 solver with timeout from config.
    pub fn init(&mut self) {
        let solv = z3::Solver::new();
        if self.config.timeout > 0 {
            let mut params = z3::Params::new();
            params.set_u32("timeout", self.config.timeout as u32 * 1000);
            solv.set_params(&params);
        }
        self.solver = Some(solv);
    }

    /// Check if Z3 is initialized and available.
    pub fn is_available(&self) -> bool {
        self.solver.is_some()
    }

    /// Zero-extend narrower operand to match wider one for Z3 BV operations.
    /// Returns (a, b) with equal widths.
    pub fn zero_extend_match(
        &self,
        a: &z3::ast::BV,
        b: &z3::ast::BV,
    ) -> (z3::ast::BV, z3::ast::BV) {
        let a_size = a.get_size();
        let b_size = b.get_size();
        if a_size == b_size {
            (a.clone(), b.clone())
        } else if a_size > b_size {
            (a.clone(), b.zero_ext(a_size - b_size))
        } else {
            (a.zero_ext(b_size - a_size), b.clone())
        }
    }

    /// Reset solver (keep thread-local context for incremental solving).
    pub fn reset(&mut self) {
        self.solver = Some(z3::Solver::new());
        // Re-apply timeout
        if self.config.timeout > 0 {
            if let Some(ref solv) = self.solver {
                let mut params = z3::Params::new();
                params.set_u32("timeout", self.config.timeout as u32 * 1000);
                solv.set_params(&params);
            }
        }
    }
}
