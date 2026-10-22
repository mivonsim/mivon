//! Lint cache persistence for `mlint`.

use super::LintArgs;
use super::Finding;

pub(crate) fn save_lint_cache(args: &LintArgs, findings: &[Finding]) {
    use mivon_compiler::micd::cache::pipeline::LintPayload;
    use mivon_compiler::micd::cache::CacheCategory;

    let Ok((mut layer, _pid)) = crate::open_cache_layer(args.targets, args.incdirs, args.defines)
    else {
        return;
    };
    let payload = LintPayload {
        findings: findings
            .iter()
            .map(|f| mivon_compiler::micd::cache::pipeline::LintFinding {
                module: f.module.clone(),
                check: f.check.to_string(),
                severity: f.severity.to_string(),
                message: f.message.clone(),
            })
            .collect(),
    };
    if let Ok(bytes) = bincode::serialize(&payload) {
        let _ = layer.put(CacheCategory::Lint, "report", &bytes);
        let _ = layer.save();
    }
}