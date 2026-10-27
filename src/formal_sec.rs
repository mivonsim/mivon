//! SEC lewat CLI (COMP-16 tahap 3 / FORMAL-11): bandingkan dua file desain
//! kombinasional memakai `mivon_formal::sec`.
//!
//! Dipisah dari `formal_run.rs` (BMC + connectivity) karena tanggung jawabnya
//! berbeda: file ini hanya menyiapkan dua `SecDesign`, mencetak verdict, dan
//! menjadi gate CI (exit != 0 saat tidak ekuivalen / tidak didukung).

use std::path::Path;

use mivon_core::diagnostics::DiagCode;
use mivon_core::error::SimError;
use mivon_ir::IrDesign;

use mivon_api::formal::sec::{check_equivalence, design_from_module, SecOutcome};

/// Kompilasi file kedua menjadi IR memakai lingkungan CLI yang SAMA
/// (`--define`, `--incdir`, `--libdir`, `--libfile`, `--top`, lexer).
/// Tanpa propagasi ini, desain kedua bisa di-preprocess berbeda dari desain
/// utama sehingga perbandingan tidak bermakna.
pub(crate) fn compile_design(
    path: &Path,
    cli: &crate::cli::Cli,
    top: Option<&str>,
) -> Result<IrDesign, SimError> {
    let mut cfg = mivon_api::SessionConfig::default();
    cfg.sources = vec![path.to_path_buf()];
    cfg.incdirs = cli.incdirs.iter().map(std::path::PathBuf::from).collect();
    cfg.defines = cli.define_pairs();
    cfg.libdirs = cli.libdirs.iter().map(std::path::PathBuf::from).collect();
    cfg.libfiles = cli.libfiles.iter().map(std::path::PathBuf::from).collect();
    // Modul target (bukan sekadar --top) menentukan top desain kedua: tanpa
    // ini file kedua dengan >1 root gagal E3006 walau modulnya ada.
    cfg.top_module = top.map(str::to_string);
    cfg.auto_incdirs = true;
    cfg.use_fast_lexer = !cli.legacy_lexer;
    compile_design_with_config(cfg)
}

/// Tolak desain kedua bila ada `include yang tidak ter-resolve.
///
/// Pipeline memperlakukan include hilang sebagai warning preprocessor yang
/// tidak pernah sampai ke `parse_errors`, sehingga modul yang seharusnya
/// berbeda bisa terlihat ekuivalen (mis. `ifdef dari header yang hilang).
/// Probe memakai preprocessor sesi (incdir/define/jalur UVM yang sama) supaya
/// tidak ada perbedaan resolusi.
fn probe_includes(
    base: &mivon_parser::preprocessor::Preprocessor,
    paths: &[std::path::PathBuf],
) -> Result<(), SimError> {
    for src in paths {
        if !src.is_file() {
            continue;
        }
        let bytes = std::fs::read(src).map_err(|e| {
            SimError::with_diag(
                DiagCode::UnsupportedForSec,
                format!("desain tidak bisa dibaca ({}): {e}", src.display()),
            )
        })?;
        let text = String::from_utf8_lossy(&bytes);
        let dir = src.parent().map(|p| p.to_path_buf());
        // Clone per file: pipeline juga meng-clone base preprocessor per file,
        // jadi `define dari file lain tidak boleh bocor.
        let mut pp = base.clone();
        if let Err(e) = pp.preprocess(&text, dir.as_ref()) {
            return Err(SimError::with_diag(
                DiagCode::UnsupportedForSec,
                format!("desain gagal di-preprocess ({}): {e}", src.display()),
            ));
        }
        // Deteksi terstruktur (bukan cocok-cocokan teks pesan): preprocessor
        // mencatat setiap include yang gagal di-resolve.
        if let Some(miss) = pp.missing_includes().iter().next() {
            return Err(SimError::with_diag(
                DiagCode::UnsupportedForSec,
                format!(
                    "desain punya include yang tidak ter-resolve ({}): '{}'",
                    src.display(),
                    miss
                ),
            ));
        }
    }
    Ok(())
}

/// Periksa include desain UTAMA memakai preprocessor pipeline yang SEBENARNYA
/// (bukan konfigurasi tiruan). Ini penting: kalau probe memakai jalur cari yang
/// berbeda dari kompilasi, hasilnya bisa fail-open (probe lolos, kompilasi
/// gagal resolve) — EQUIVALENT palsu tanpa warning.
pub(crate) fn check_includes(
    pp: &mivon_parser::preprocessor::Preprocessor,
    paths: &[std::path::PathBuf],
) -> Result<(), SimError> {
    probe_includes(pp, paths)
}

/// Bagian yang bisa diuji tanpa `Cli`: kompilasi + elaborasi satu file.
pub(crate) fn compile_design_with_config(
    cfg: mivon_api::SessionConfig,
) -> Result<IrDesign, SimError> {
    // SessionConfig::top_module hanya disimpan, tidak dipakai untuk memilih
    // top: compile_and_elaborate menerima nama top sebagai argumen. Tanpa
    // meneruskannya di sini, file kedua dengan >1 root tetap gagal E3006
    // walau --top diberikan.
    let top = cfg.top_module.clone();
    let mut session = mivon_compiler::frontend::CompileSession::new(cfg);
    // Include yang tidak ter-resolve HANYA jadi warning preprocessor yang
    // dibuang pipeline (tidak masuk parse_errors) — desain dengan header
    // hilang bisa lolos SEC seolah lengkap. Probe semua input (sources +
    // libfiles), bukan hanya file pertama.
    let mut inputs: Vec<std::path::PathBuf> = session.config.sources.clone();
    inputs.extend(session.config.libfiles.iter().cloned());
    let pp = session.create_preprocessor();
    probe_includes(&pp, &inputs)?;
    let (_, ir, _) = session.compile_and_elaborate(top.as_deref())?;
    // CompileSession mengumpulkan parse error tanpa mengembalikan Err, jadi
    // desain yang hanya ter-parse SEBAGIAN bisa sampai ke SEC dan dilaporkan
    // "Equivalent" untuk sisa modul. Gate eksplisit (parse + elaborasi, sama
    // seperti pipeline utama): tanpa ini verdict tidak bermakna.
    let first_err = session
        .parse_errors
        .iter()
        .chain(session.elab_diagnostics.iter())
        .find(|d| d.is_error());
    if let Some(d) = first_err {
        let loc = d
            .source_snippet
            .as_ref()
            .map(|ss| format!(" di {}:{}:{}", ss.file, ss.line, ss.col))
            .unwrap_or_default();
        return Err(SimError::with_diag(
            DiagCode::UnsupportedForSec,
            format!(
                "desain kedua gagal dikompilasi ({}){loc}: {}",
                d.code.as_str(),
                d.message
            ),
        ));
    }
    Ok(ir)
}

/// Nama modul yang dibandingkan: eksplisit (`--top`) atau top desain utama.
pub(crate) fn module_name(primary: &IrDesign, explicit: Option<&str>) -> String {
    explicit
        .map(str::to_string)
        .unwrap_or_else(|| primary.top.name.as_str().to_string())
}

/// Cari modul berdasarkan nama (top lebih dulu, lalu tabel modul).
fn pick<'a>(design: &'a IrDesign, name: &str) -> Option<&'a mivon_ir::IrModule> {
    if design.top.name.as_str() == name {
        return Some(&design.top);
    }
    design
        .modules
        .iter()
        .find(|(sym, _)| sym.as_str() == name)
        .map(|(_, m)| m)
}

/// Modul di luar subset SEC: kode E1008 (exit 10), BUKAN 60, supaya CI bisa
/// membedakan "terbukti tidak ekuivalen" (RT7001) dari "tidak bisa dicek".
/// Bukan E1007 — kode itu berarti "assertion di-skip", yang menyesatkan di
/// sini karena tidak ada verdict sama sekali.
fn not_supported(msg: String) -> SimError {
    SimError::with_diag(DiagCode::UnsupportedForSec, msg)
}

/// Modul target tidak ditemukan di salah satu desain (E3001, exit 30).
fn missing_module(msg: String) -> SimError {
    SimError::with_diag(DiagCode::ModuleNotFound, msg)
}

/// Bandingkan modul `name` pada dua desain.
///
/// `Err` bila modul hilang, ekstraksi gagal (di luar subset SEC), atau hasilnya
/// tidak ekuivalen — semuanya harus menggagalkan CI, dengan pesan berbeda.
pub(crate) fn run_sec(
    primary: &IrDesign,
    secondary: &IrDesign,
    explicit: Option<&str>,
    quiet: bool,
) -> Result<(), SimError> {
    let name = module_name(primary, explicit);
    let a = pick(primary, &name)
        .ok_or_else(|| missing_module(format!("modul '{name}' tidak ada di desain utama")))?;
    let b = pick(secondary, &name)
        .ok_or_else(|| missing_module(format!("modul '{name}' tidak ada di desain kedua")))?;

    let da = design_from_module(a)
        .map_err(|e| not_supported(format!("ekstraksi desain utama gagal: {e}")))?;
    let db = design_from_module(b)
        .map_err(|e| not_supported(format!("ekstraksi desain kedua gagal: {e}")))?;

    // Modul tanpa output: miter kosong selalu Unsat -> "EQUIVALENT (0 output)"
    // yang tidak membuktikan apa pun. Untuk gate CI itu hijau palsu.
    if da.outputs.is_empty() {
        return Err(not_supported(format!(
            "SEC: modul '{name}' tidak punya output — tidak ada yang dibandingkan"
        )));
    }

    if !quiet {
        println!("\n── Equivalence Check (SEC) — modul '{name}' ──");
        let inputs: Vec<String> = da.inputs.iter().map(|(n, w)| format!("{n}[{w}]")).collect();
        let outputs: Vec<String> = da
            .outputs
            .iter()
            .map(|(n, w, _)| format!("{n}[{w}]"))
            .collect();
        println!("  input : {}", inputs.join(", "));
        println!("  output: {}", outputs.join(", "));
    }

    match check_equivalence(&da, &db) {
        SecOutcome::Equivalent => {
            if !quiet {
                println!("  ✓ EQUIVALENT ({} output)", da.outputs.len());
            }
            Ok(())
        }
        SecOutcome::NotEquivalent {
            output,
            counterexample,
        } => {
            let cex: Vec<String> = counterexample
                .iter()
                .map(|(n, v)| format!("{n}={v:#x}"))
                .collect();
            if !quiet {
                println!("  ✗ NOT EQUIVALENT pada output '{output}'");
                println!("    counterexample: {}", cex.join(", "));
            }
            Err(SimError::with_diag(
                DiagCode::AssertionFailed,
                format!(
                    "SEC: modul '{name}' tidak ekuivalen pada output '{output}' ({})",
                    cex.join(", ")
                ),
            ))
        }
        SecOutcome::Unsupported(msg) => {
            if !quiet {
                println!("  ! TIDAK DIDUKUNG: {msg}");
            }
            Err(not_supported(format!(
                "SEC: modul '{name}' tidak dapat dibandingkan: {msg}"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn design(src: &str) -> IrDesign {
        mivon_api::compile_str(src).expect("sumber uji harus compile")
    }

    const AND: &str = r#"
module m(input logic [7:0] a, b, output logic [7:0] y);
  assign y = a & b;
endmodule
"#;

    const AND_SWAPPED: &str = r#"
module m(input logic [7:0] a, b, output logic [7:0] y);
  assign y = b & a;
endmodule
"#;

    const OR: &str = r#"
module m(input logic [7:0] a, b, output logic [7:0] y);
  assign y = a | b;
endmodule
"#;

    #[test]
    fn equivalent_designs_pass() {
        let a = design(AND);
        let b = design(AND_SWAPPED);
        assert!(run_sec(&a, &b, None, true).is_ok());
    }

    #[test]
    fn different_designs_fail_with_output_name() {
        let a = design(AND);
        let b = design(OR);
        let err = run_sec(&a, &b, None, true).expect_err("a&b != a|b");
        let msg = format!("{err}");
        assert!(msg.contains("tidak ekuivalen"), "{msg}");
        assert!(msg.contains('y'), "pesan harus menyebut output y: {msg}");
        // Gate CI: hanya ketidakekuivalenan yang memakai RT7001 (exit 60).
        assert_eq!(err.error_code(), "RT7001", "{msg}");
    }

    #[test]
    fn missing_module_is_reported() {
        let a = design(AND);
        let b = design(OR);
        let err = run_sec(&a, &b, Some("tidak_ada"), true).expect_err("modul asing");
        assert!(format!("{err}").contains("tidak ada di desain utama"));
        // E3001 (exit 30) — beda dari RT7001 supaya CI bisa membedakan
        // "salah nama modul" dari "terbukti tidak ekuivalen".
        assert_eq!(err.error_code(), "E3001");
    }

    #[test]
    fn compile_design_uses_explicit_top_for_multi_root_file() {
        // File kedua dengan dua root: tanpa top eksplisit, elaborasi gagal E3006.
        let dir = std::env::temp_dir().join(format!("mivon_sec_top_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("two_roots.sv");
        std::fs::write(
            &file,
            "module m(input logic a, output logic y);\n  assign y = a;\nendmodule\n\
             module n(input logic b, output logic z);\n  assign z = b;\nendmodule\n",
        )
        .unwrap();

        let mut cfg = mivon_api::SessionConfig::default();
        cfg.sources = vec![file.clone()];
        cfg.top_module = Some("m".to_string());
        let ir = compile_design_with_config(cfg).expect("top eksplisit harus dipakai");
        assert_eq!(ir.top.name.as_str(), "m");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_error_in_second_design_is_rejected() {
        // CompileSession mengumpulkan parse error TANPA Err; tanpa gate ini
        // desain parsial bisa sampai ke SEC dan dilaporkan Equivalent.
        let dir = std::env::temp_dir().join(format!("mivon_sec_parse_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("broken.sv");
        std::fs::write(
            &file,
            "module m(input logic a, output logic y);\n  assign y = a +;\nendmodule\n",
        )
        .unwrap();

        let mut cfg = mivon_api::SessionConfig::default();
        cfg.sources = vec![file];
        let err = compile_design_with_config(cfg).expect_err("parse error harus ditolak");
        let msg = format!("{err}");
        assert!(msg.contains("gagal dikompilasi"), "{msg}");
        assert_eq!(err.error_code(), "E1008", "{msg}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unresolved_include_in_second_design_is_rejected() {
        // Include hilang hanya warning preprocessor (tidak masuk parse_errors);
        // tanpa probe ini modul yang seharusnya beda bisa terlihat ekuivalen.
        let dir = std::env::temp_dir().join(format!("mivon_sec_inc_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("inc.sv");
        std::fs::write(
            &file,
            "`include \"tidak_ada_defs.svh\"\n\
             module m(input logic a, output logic y);\n  assign y = a;\nendmodule\n",
        )
        .unwrap();

        let mut cfg = mivon_api::SessionConfig::default();
        cfg.sources = vec![file];
        let err = compile_design_with_config(cfg).expect_err("include hilang harus ditolak");
        let msg = format!("{err}");
        assert!(msg.contains("include"), "{msg}");
        assert_eq!(err.error_code(), "E1008", "{msg}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn module_without_outputs_is_rejected() {
        // Miter kosong selalu Unsat; "EQUIVALENT (0 output)" bukan bukti.
        let src = "module m(input logic a);\n  logic t;\n  assign t = a;\nendmodule\n";
        let a = design(src);
        let b = design(src);
        let err = run_sec(&a, &b, None, true).expect_err("tanpa output harus ditolak");
        assert!(format!("{err}").contains("tidak punya output"));
        assert_eq!(err.error_code(), "E1008");
    }

    #[test]
    fn unsupported_module_is_rejected() {
        // If di always_comb belum didukung ekstraksi SEC -> harus Err jelas.
        let src = r#"
module m(input logic a, output logic y);
  always_comb begin
    if (a) y = 1'b1; else y = 1'b0;
  end
endmodule
"#;
        let a = design(AND);
        let b = design(src);
        let err = run_sec(&a, &b, None, true).expect_err("If belum didukung");
        let msg = format!("{err}");
        assert!(msg.contains("desain kedua"), "{msg}");
        assert!(msg.contains("belum didukung"), "{msg}");
        // E1008 (exit 10): "tidak bisa dicek", bukan RT7001 (exit 60).
        // Bukan E1007 — kode itu berarti "assertion di-skip" (menyesatkan).
        assert_eq!(err.error_code(), "E1008", "{msg}");
    }
}
