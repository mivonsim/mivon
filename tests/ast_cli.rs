//! E2E wiring test untuk `--ast`.
//!
//! Regresi: jalur legacy (`src/run_legacy.rs`) tidak punya cabang `print_ast`
//! (hanya `run_fast` yang mencetak AST, dan jalur itu justru dimatikan saat
//! `--ast` aktif). Akibatnya `mivon --ast file.sv` menjalankan elaborasi +
//! simulasi penuh dan **tidak pernah** mencetak AST.
//!
//! Test ini mengunci dua hal sekaligus:
//! 1. AST benar-benar dicetak dan nama modul/sinyal terbaca (bukan `Symbol(n)`).
//! 2. Simulasi TIDAK dijalankan — marker runtime `SIM_MARKER_42` (hasil
//!    `$display("SIM_MARKER_%0d", 42)`) harus absen, meski literal formatnya
//!    ikut muncul di dump AST.

use std::path::PathBuf;
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mivon")
}

struct Sandbox {
    dir: PathBuf,
}

impl Sandbox {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "mivon_ast_it_{}_{}_{:?}",
            tag,
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("buat direktori sandbox");
        // Marker root proyek: pipeline legacy berhenti menaikkan include search
        // path di sini, jadi sandbox paralel di /tmp tidak saling menemukan.
        std::fs::create_dir_all(dir.join(".mivon")).expect("marker .mivon");
        Sandbox { dir }
    }

    fn write(&self, name: &str, src: &str) -> String {
        let path = self.dir.join(name);
        std::fs::write(&path, src).expect("tulis fixture");
        path.to_string_lossy().into_owned()
    }

    fn run(&self, args: &[&str]) -> (i32, String) {
        let out = Command::new(bin())
            .args(args)
            .current_dir(&self.dir)
            .env("MIVON_MICD_DIR", self.dir.join("micd"))
            .output()
            .expect("jalankan binary mivon");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.code().unwrap_or(-1), text)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const PROBE: &str = r#"module ast_probe(input logic clk, output logic [7:0] q);
  initial begin
    $display("SIM_MARKER_%0d", 42);
    #1 $finish;
  end
endmodule
"#;

#[test]
fn ast_flag_prints_design_and_skips_simulation() {
    let sb = Sandbox::new("dump");
    let src = sb.write("probe.sv", PROBE);

    let (code, text) = sb.run(&["--ast", &src]);
    assert_eq!(code, 0, "exit code harus 0, output:\n{}", text);

    // (1) AST tercetak, dan nama terbaca (Symbol Debug = string).
    assert!(
        text.contains("Design {"),
        "AST tidak dicetak oleh --ast. Output:\n{}",
        text
    );
    assert!(
        text.contains("Symbol(\"ast_probe\")"),
        "nama modul tidak terbaca di dump AST (Symbol index mentah?). Output:\n{}",
        text
    );
    assert!(
        text.contains("ports:"),
        "dump AST tidak memuat field ports. Output:\n{}",
        text
    );

    // (2) Simulasi tidak dijalankan.
    assert!(
        !text.contains("SIM_MARKER_42"),
        "--ast masih menjalankan simulasi (marker runtime muncul). Output:\n{}",
        text
    );
    assert!(
        !text.contains("Simulation completed"),
        "--ast masih menjalankan simulasi. Output:\n{}",
        text
    );
}

/// Flag simulasi yang tidak bermakna tanpa simulasi harus diberi tahu
/// (bukan diabaikan diam-diam seperti perilaku lama).
#[test]
fn ast_flag_warns_about_ignored_simulation_flags() {
    let sb = Sandbox::new("warn");
    let src = sb.write("probe.sv", PROBE);

    let (code, text) = sb.run(&["--ast", "--debug", "--print-state", &src]);
    assert_eq!(code, 0, "exit code harus 0, output:\n{}", text);
    assert!(
        text.contains("--ast tidak menjalankan simulasi"),
        "tidak ada peringatan untuk flag simulasi yang diabaikan. Output:\n{}",
        text
    );
    assert!(
        text.contains("--debug") && text.contains("--print-state"),
        "peringatan tidak menyebut flag yang diabaikan. Output:\n{}",
        text
    );
    assert!(
        text.contains("Design {"),
        "AST tetap harus dicetak. Output:\n{}",
        text
    );
}

/// Tanpa `--ast`, simulasi tetap berjalan — memastikan marker test di atas
/// benar-benar diskriminatif (bukan karena `$display` tak pernah jalan).
#[test]
fn without_ast_flag_simulation_runs() {
    let sb = Sandbox::new("sim");
    let src = sb.write("probe.sv", PROBE);

    let (code, text) = sb.run(&[&src]);
    assert_eq!(code, 0, "exit code harus 0, output:\n{}", text);
    assert!(
        text.contains("SIM_MARKER_42"),
        "simulasi normal tidak menjalankan $display. Output:\n{}",
        text
    );
    assert!(
        !text.contains("Design {"),
        "tanpa --ast tidak boleh ada dump AST. Output:\n{}",
        text
    );
}
