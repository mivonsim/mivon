//! I/O sumber: pemuatan library foreign ([foreign] di project .mivon) dan
//! pembacaan byte sumber (file disk atau buffer transpile inline).
//!
//! Dipisah dari main.rs — tidak menyentuh state simulasi.

use crate::cli::Cli;
use std::path::{Path, PathBuf};

/// Load library foreign dari bagian `[foreign]` file project .mivon
/// (arsitektur poin 9). Load VHPI/PLI/DPI; error → warning (bukan gagal
/// compile) agar project tetap bisa jalan tanpa library opsional.
pub(crate) fn load_project_foreign_libs(proj: &mivon_api::ProjectFile, cli: &Cli) {
    for lib_path in &proj.vhpi_libs {
        match mivon_api::vhpi::loader::load_vhpi_library(lib_path) {
            Ok(vhpi) => {
                if !cli.quiet {
                    println!(
                        "  [foreign] VHPI library loaded: {} (abi {:?})",
                        vhpi.path.display(),
                        vhpi.abi
                    );
                }
                if let Err(e) = mivon_api::vhpi::loader::call_vhpi_startup(&vhpi) {
                    eprintln!("warning: vhpi_startup '{}': {}", lib_path, e);
                }
            }
            Err(e) => eprintln!(
                "warning: [foreign] failed to load VHPI library '{}': {}",
                lib_path, e
            ),
        }
    }
    for lib_path in &proj.pli_libs {
        match mivon_api::pli::loader::load_pli_library(lib_path) {
            Ok(pli) => {
                if !cli.quiet {
                    println!(
                        "  [foreign] PLI library loaded: {} (abi {:?})",
                        pli.path.display(),
                        pli.abi
                    );
                }
                if let Err(e) = mivon_api::pli::loader::call_pli_startup(&pli) {
                    eprintln!("warning: vpi_startup (PLI) '{}': {}", lib_path, e);
                }
            }
            Err(e) => eprintln!(
                "warning: [foreign] failed to load PLI library '{}': {}",
                lib_path, e
            ),
        }
    }
    #[cfg(feature = "dpi")]
    for lib_path in &proj.dpi_libs {
        use mivon_api::simulator::dpi::DpiEngine;
        let mut eng = DpiEngine::new();
        match eng.load_library(lib_path) {
            Ok(_) => {
                if !cli.quiet {
                    println!("  [foreign] DPI library loaded: {}", lib_path);
                }
            }
            Err(e) => eprintln!(
                "warning: [foreign] failed to load DPI library '{}': {}",
                lib_path, e
            ),
        }
    }
}

/// Apakah source mentah memuat directive ```timescale` (F84).
///
/// Dipakai sebagai gerbang reuse MICD: entry preproc yang tak menyimpan
/// segmen hanya aman bila source-nya memang tak punya directive sama sekali
/// (sehingga satuan global sudah benar). Scan baris demi baris — cukup untuk
/// keputusan cache, tak perlu parser.
pub(crate) fn content_has_timescale(content: &[u8]) -> bool {
    let text = String::from_utf8_lossy(content);
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("//") {
            continue;
        }
        if t.starts_with('`') {
            let rest = t.trim_start_matches('`');
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if name == "timescale" {
                return true;
            }
        }
    }
    false
}

/// Baca byte sumber sebuah file. Untuk file `.mv` (F8) byte berasal dari
/// buffer hasil transpile on-the-fly; untuk file lain langsung dari disk.
pub(crate) fn read_source_bytes(
    path: &Path,
    inline: &std::collections::HashMap<PathBuf, Vec<u8>>,
) -> std::io::Result<Vec<u8>> {
    if let Some(bytes) = inline.get(path) {
        return Ok(bytes.clone());
    }
    std::fs::read(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Directive `timescale` dikenali (termasuk indentasi), komentar diabaikan.
    #[test]
    fn timescale_directive_detected() {
        assert!(content_has_timescale(b"  `timescale 1ns/1ps\nmodule m; endmodule"));
        assert!(content_has_timescale(b"`timescale 10ps/1ps"));
        assert!(!content_has_timescale(b"// `timescale 1ns/1ps\nmodule m; endmodule"));
        assert!(!content_has_timescale(b"module m; endmodule"));
    }

    /// Buffer inline menang atas disk (file yang tidak ada di disk tetap terbaca).
    #[test]
    fn inline_buffer_takes_precedence() {
        let p = PathBuf::from("/nonexistent/mivon/inline.mv");
        let mut inline = std::collections::HashMap::new();
        inline.insert(p.clone(), b"module x; endmodule".to_vec());
        assert_eq!(read_source_bytes(&p, &inline).unwrap(), b"module x; endmodule");
        assert!(read_source_bytes(Path::new("/nonexistent/mivon/none.sv"), &inline).is_err());
    }
}
