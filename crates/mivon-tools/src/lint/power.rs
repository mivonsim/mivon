//! Power optimization helpers for `mlint`.

use std::collections::HashMap;

use mivon_core::intern::Symbol;

use super::Finding;

pub(crate) fn find_power_patterns(
    scope: &str,
    declared: &HashMap<Symbol, usize>,
    out: &mut Vec<Finding>,
) {
    for (name, &width) in declared {
        if width > 8 {
            let n = name.as_str().to_lowercase();
            if n.contains("vdd") || n.contains("vss") || n.contains("gnd") || n.contains("supply") {
                continue;
            }
            out.push(Finding {
                module: scope.to_string(),
                check: "power",
                severity: "I",
                message: format!(
                    "signal '{}' ({} bit) — pertimbangkan power domain isolation untuk sinyal lebar",
                    name.as_str(),
                    width
                ),
            });
        }
    }
}
