//! Subkomando `search` — cari sinyal by pola wildcard (WAV-10).

use mivon_core::error::SimError;

use super::data::parse_vcd;
use super::util::{err, full_name, wildcard_match};

/// Cari sinyal VCD yang cocok dengan pola wildcard (WAV-10). Pola bisa
/// memakai `*` (any run) dan `?` (satu karakter). Cocok terhadap nama
/// polos (`cnt`) maupun nama penuh (`top.cnt`, hierarki). Output: daftar
/// nama + lebar + scope, urutan kemunculan di file.
pub(crate) fn search(input: &str, patterns: &[String]) -> Result<(), SimError> {
    let data = parse_vcd(input)?;
    let pats: Vec<&str> = patterns
        .iter()
        .flat_map(|p| p.split([',', ' ', ';']))
        .filter(|p| !p.is_empty())
        .collect();
    if pats.is_empty() {
        return Err(err("search membutuhkan minimal 1 pola (dukung * dan ?)"));
    }

    let mut found: Vec<(&super::data::VcdSignal, String)> = Vec::new();
    for sig in &data.signals {
        let full = full_name(sig);
        let hit = pats
            .iter()
            .any(|p| wildcard_match(p, &sig.name) || wildcard_match(p, &full));
        if hit {
            found.push((sig, full));
        }
    }

    if found.is_empty() {
        println!("  tidak ada sinyal yang cocok dengan: {}", pats.join(", "));
        return Ok(());
    }
    println!("  {} sinyal cocok dengan: {}", found.len(), pats.join(", "));
    for (sig, full) in &found {
        println!(
            "    {}  width={}  scope={}",
            full,
            sig.width,
            if sig.scope.is_empty() {
                "<top>".to_string()
            } else {
                sig.scope.join(".")
            }
        );
    }
    Ok(())
}
