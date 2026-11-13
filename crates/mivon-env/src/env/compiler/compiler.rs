use crate::env::config::ConfigContext;
use crate::env::workspace::WorkspaceContext;
use mivon_ast::Design;
use mivon_compiler::frontend::compile_session::{CompileSession, SessionConfig, SessionTiming};
use mivon_compiler::frontend::module_index::ModuleIndex;
use mivon_core::error::SimError;
use mivon_elaboration::ElaborateMode;
use mivon_ir::IrDesign;

/// CompilerContext — pintu pipeline compile+elaborate.
///
/// Compiler TIDAK tahu database/GUI/logger; cukup bertanya ke context
/// (config, workspace) untuk sumber/pengaturan.
pub struct CompilerContext {
    session: CompileSession,
    elab_mode: ElaborateMode,
    /// Jumlah file sumber yang terdeteksi (dari workspace).
    pub source_count: usize,
}

impl CompilerContext {
    /// Bangun session dari workspace + config. Tidak langsung compile.
    pub fn new(workspace: &WorkspaceContext, config: &ConfigContext) -> Self {
        let sources = workspace.discover_sources();
        let source_count = sources.len();
        let sc = SessionConfig {
            sources,
            incdirs: workspace.incdirs().dirs().to_vec(),
            defines: workspace.defines().to_vec(),
            top_module: None,
            use_fast_lexer: true,
            ..Default::default()
        };
        let elab_mode = match config.elab_mode() {
            Some(m) if m.eq_ignore_ascii_case("analysisrecovery") => {
                ElaborateMode::AnalysisRecovery
            }
            _ => ElaborateMode::StrictSimulation,
        };
        CompilerContext {
            session: CompileSession::new(sc),
            elab_mode,
            source_count,
        }
    }

    pub fn elab_mode(&self) -> ElaborateMode {
        self.elab_mode
    }

    pub fn set_elab_mode(&mut self, mode: ElaborateMode) {
        self.elab_mode = mode;
    }

    /// Parse semua file (paralel, dengan node_cache bila di-attach) → merged Design.
    pub fn compile(&mut self) -> Result<(Design, &ModuleIndex), SimError> {
        self.session.compile()
    }

    /// Parse + elaborate → (Design, IrDesign).
    pub fn compile_and_elaborate(
        &mut self,
        top: Option<&str>,
    ) -> Result<(Design, IrDesign), SimError> {
        let mode = self.elab_mode;
        let (design, ir, _len) = self.session.compile_and_elaborate_with_mode(top, mode)?;
        Ok((design, ir))
    }

    pub fn timing(&self) -> &SessionTiming {
        &self.session.timing
    }

    pub fn module_index(&self) -> &ModuleIndex {
        &self.session.module_index
    }

    /// Akses session mentah (untuk cache attach/save oleh DatabaseContext).
    pub fn session_mut(&mut self) -> &mut CompileSession {
        &mut self.session
    }

    pub fn session_ref(&self) -> &CompileSession {
        &self.session
    }

    /// Aktifkan profiler pada session.
    pub fn enable_profiling(&mut self) {
        self.session.enable_profiling();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mivon_core::config::MivonConfig;

    #[test]
    fn test_compiler_context_builds_session() {
        let cfg = ConfigContext::new(MivonConfig::default());
        let ws = WorkspaceContext::open_in(std::path::Path::new("."));
        let cc = CompilerContext::new(&ws, &cfg);
        assert_eq!(cc.elab_mode(), ElaborateMode::StrictSimulation);
    }

    #[test]
    fn test_elab_mode_from_config() {
        let mut m = MivonConfig::default();
        m.elaborate.mode = Some("AnalysisRecovery".into());
        let cfg = ConfigContext::new(m);
        let ws = WorkspaceContext::open_in(std::path::Path::new("."));
        let cc = CompilerContext::new(&ws, &cfg);
        assert_eq!(cc.elab_mode(), ElaborateMode::AnalysisRecovery);
    }
}
