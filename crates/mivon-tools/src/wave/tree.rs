//! Subkomando `tree` — index hierarki scope + sinyal dari VCD (WAV-08).

use std::collections::BTreeMap;

use mivon_core::error::SimError;

use super::data::parse_vcd;
use super::data::VcdSignal;

/// Index hierarki scope + sinyal dari VCD (WAV-08): kelompokkan signal per
/// scope (dari $scope/$upscope), cetak sebagai pohon dengan indentasi dan
/// lebar tiap signal. Berguna untuk menemukan path lengkap signal sebelum
/// `filter`/`search`.
pub(crate) fn tree(input: &str) -> Result<(), SimError> {
    let data = parse_vcd(input)?;
    // scope → daftar signal (urut kemunculan)
    let mut scopes: BTreeMap<Vec<String>, Vec<&VcdSignal>> = BTreeMap::new();
    for sig in &data.signals {
        scopes.entry(sig.scope.clone()).or_default().push(sig);
    }

    println!(
        "  waveform tree: {} ({} signal, {} scope)",
        input,
        data.signals.len(),
        scopes.len()
    );
    for (scope, sigs) in &scopes {
        // Indentasi scope: 4 + 2 per kedalaman (pohon hierarki).
        let indent = "    ".repeat(scope.len().saturating_add(1));
        let name = if scope.is_empty() {
            "<top>".to_string()
        } else {
            scope.join(".")
        };
        println!("  {} {}", indent, name);
        let sig_indent = "    ".repeat(scope.len().saturating_add(2));
        for sig in sigs {
            println!(
                "  {}{:<8} bits={:<5} {}",
                sig_indent,
                format!("[{}:0]", sig.width.saturating_sub(1)),
                sig.width,
                sig.name
            );
        }
    }
    Ok(())
}
