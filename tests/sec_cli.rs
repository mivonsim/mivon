//! E2E wiring test untuk `--formal-sec` (COMP-16 tahap 3).
//!
//! Test unit di `src/formal_sec.rs` hanya memanggil `run_sec`/`compile_design`
//! langsung; ia TIDAK bisa menangkap flag yang diabaikan diam-diam oleh
//! pipeline. File ini menjalankan binary sungguhan supaya wiring (guard
//! recovery, desain kosong, paritas legacy/fast, propagasi define) terkunci:
//! gate CI yang hijau tanpa membandingkan adalah bug paling berbahaya di sini.

#![cfg(feature = "formal")]

use std::path::PathBuf;
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mivon")
}

/// Direktori kerja + MICD terisolasi per test (cache MICD bisa menipu: AST
/// lama di-restore sehingga verdict terlihat benar padahal flag berubah).
struct Sandbox {
    dir: PathBuf,
}

impl Sandbox {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "mivon_sec_it_{}_{}_{:?}",
            tag,
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("buat direktori sandbox");
        // Marker root proyek: pipeline (legacy) berhenti menaikkan include
        // search path di sini, jadi sandbox paralel di /tmp tidak saling
        // "menemukan" header satu sama lain.
        std::fs::create_dir_all(dir.join(".mivon")).expect("marker .mivon");
        Sandbox { dir }
    }

    fn write(&self, name: &str, src: &str) -> String {
        let path = self.dir.join(name);
        std::fs::write(&path, src).expect("tulis fixture");
        path.to_string_lossy().into_owned()
    }

    /// Jalankan binary di dalam sandbox; kembalikan (exit code, stdout+stderr).
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

const AND: &str =
    "module m(input logic [7:0] a, b, output logic [7:0] y);\n  assign y = a & b;\nendmodule\n";
const AND_SWAPPED: &str =
    "module m(input logic [7:0] a, b, output logic [7:0] y);\n  assign y = b & a;\nendmodule\n";
const OR: &str =
    "module m(input logic [7:0] a, b, output logic [7:0] y);\n  assign y = a | b;\nendmodule\n";

#[test]
fn equivalent_designs_exit_zero() {
    let sb = Sandbox::new("eq");
    let a = sb.write("a.sv", AND);
    let b = sb.write("b.sv", AND_SWAPPED);
    let (code, out) = sb.run(&[&a, "--formal-sec", &b]);
    assert_eq!(code, 0, "harus exit 0 (EQUIVALENT): {out}");
    assert!(out.contains("EQUIVALENT"), "{out}");
}

#[test]
fn not_equivalent_designs_exit_60() {
    let sb = Sandbox::new("neq");
    let a = sb.write("a.sv", AND);
    let c = sb.write("c.sv", OR);
    let (code, out) = sb.run(&[&a, "--formal-sec", &c]);
    assert_eq!(code, 60, "harus exit 60 (RT7001): {out}");
    assert!(out.contains("RT7001"), "{out}");
    assert!(out.contains("NOT EQUIVALENT"), "{out}");
}

/// Desain tanpa module (hanya package) TIDAK boleh hijau: dulu exit 0 di kedua
/// pipeline karena jalur analisis kembali lebih awal dari blok SEC.
#[test]
fn packages_only_design_is_rejected_in_both_pipelines() {
    let sb = Sandbox::new("pkg");
    let pkg = sb.write(
        "p.sv",
        "package p;\n  typedef logic [7:0] byte_t;\nendpackage\n",
    );
    let a = sb.write("a.sv", AND);
    for extra in [None, Some("--fast")] {
        let mut args = vec![pkg.as_str(), "--formal-sec", a.as_str()];
        if let Some(f) = extra {
            args.push(f);
        }
        let (code, out) = sb.run(&args);
        assert_eq!(code, 10, "{args:?} harus E1008/10: {out}");
        assert!(out.contains("E1008"), "{out}");
    }
}

/// File header/macro-only: kode harus SAMA di legacy dan --fast (dulu 10 vs 30).
#[test]
fn header_only_design_is_rejected_identically() {
    let sb = Sandbox::new("hdr");
    let hdr = sb.write("h.sv", "`define FOO 1\n// tidak ada module\n");
    let a = sb.write("a.sv", AND);
    for extra in [None, Some("--fast")] {
        let mut args = vec![hdr.as_str(), "--formal-sec", a.as_str()];
        if let Some(f) = extra {
            args.push(f);
        }
        let (code, out) = sb.run(&args);
        assert_eq!(code, 10, "{args:?} harus E1008/10: {out}");
        assert!(out.contains("E1008"), "{out}");
    }
}

/// Top tidak unik (mode recovery) tidak boleh dipakai sebagai gate, termasuk
/// dengan --force-sim.
#[test]
fn recovery_mode_is_rejected_in_both_pipelines() {
    let sb = Sandbox::new("rec");
    let two = sb.write(
        "two.sv",
        "module m(input logic a, output logic y);\n  assign y = a;\nendmodule\n\
         module n(input logic b, output logic z);\n  assign z = b;\nendmodule\n",
    );
    let a = sb.write("a.sv", AND);
    for extra in [None, Some("--fast")] {
        for force in [false, true] {
            let mut args = vec![two.as_str(), "--formal-sec", a.as_str()];
            if let Some(f) = extra {
                args.push(f);
            }
            if force {
                args.push("--force-sim");
            }
            let (code, out) = sb.run(&args);
            assert_eq!(code, 10, "{args:?} harus ditolak: {out}");
            assert!(out.contains("E1008"), "{out}");
        }
    }
}

/// `include yang hilang TIDAK boleh lolos sebagai EQUIVALENT: pipeline hanya
/// memberi warning preprocessor, jadi desain terlihat "lengkap" padahal header
/// (mis. `ifdef WIDE) belum ada. Diuji di KEDUA sisi (desain utama dan desain
/// kedua) karena jalur cache-nya berbeda.
#[test]
fn missing_include_is_rejected_on_both_sides() {
    const IFDEF: &str = "`include \"defs.svh\"\n\
         module m(input logic [7:0] a, b, output logic [7:0] y);\n\
         `ifdef WIDE\n  assign y = a | b;\n`else\n  assign y = a & b;\n`endif\nendmodule\n";

    for extra in [None, Some("--fast")] {
        // (A) include hilang di desain KEDUA.
        let sb = Sandbox::new("inc2");
        let a = sb.write("a.sv", AND);
        let b = sb.write("b.sv", IFDEF);
        let mut args = vec![a.as_str(), "--formal-sec", b.as_str()];
        if let Some(f) = extra {
            args.push(f);
        }
        let (code, out) = sb.run(&args);
        assert_eq!(code, 10, "include hilang (desain kedua) {args:?}: {out}");
        assert!(out.contains("E1008"), "{out}");
        // Header muncul: WIDE aktif -> a|b vs a&b -> NOT EQUIVALENT.
        std::fs::write(sb.dir.join("defs.svh"), "`define WIDE 1\n").expect("tulis header");
        let (code, out) = sb.run(&args);
        assert_eq!(code, 60, "setelah header ada {args:?}: {out}");

        // (B) include hilang di desain UTAMA (jalur preprocessor + cache MICD).
        let sb = Sandbox::new("inc1");
        let p = sb.write("p.sv", IFDEF);
        let q = sb.write("q.sv", AND);
        let mut args = vec![p.as_str(), "--formal-sec", q.as_str()];
        if let Some(f) = extra {
            args.push(f);
        }
        let (code, out) = sb.run(&args);
        assert_eq!(code, 10, "include hilang (desain utama) {args:?}: {out}");
        assert!(out.contains("E1008"), "{out}");
        // Run yang ditolak tidak boleh meninggalkan cache yang membuat header
        // baru diabaikan.
        std::fs::write(sb.dir.join("defs.svh"), "`define WIDE 1\n").expect("tulis header");
        let (code, out) = sb.run(&args);
        assert_eq!(code, 60, "utama, setelah header ada {args:?}: {out}");
    }
}

/// Cache MICD dari run NON-SEC tidak boleh membuat verdict stale: dulu desain
/// parsial (include hilang) ter-cache, lalu header muncul tetapi AST lama
/// di-restore sehingga SEC tetap EQUIVALENT. SEC kini memaksa full-rebuild.
#[test]
fn cached_partial_design_does_not_mask_new_header() {
    let sb = Sandbox::new("cache");
    let p = sb.write(
        "p.sv",
        "`include \"defs.svh\"\n\
         module m(input logic [7:0] a, b, output logic [7:0] y);\n\
         `ifdef WIDE\n  assign y = a | b;\n`else\n  assign y = a & b;\n`endif\nendmodule\n",
    );
    let q = sb.write("q.sv", AND);

    for extra in [None, Some("--fast")] {
        // Header dihapus tiap iterasi: kalau tidak, iterasi --fast mewarisi
        // header dari iterasi sebelumnya dan asersinya jadi vakum.
        let _ = std::fs::remove_file(sb.dir.join("defs.svh"));
        // 1) Run biasa (tanpa SEC) saat header BELUM ada -> cache desain parsial.
        let mut warm = vec![p.as_str()];
        if let Some(f) = extra {
            warm.push(f);
        }
        let (code, out) = sb.run(&warm);
        assert_eq!(code, 0, "run pemanas harus sukses {warm:?}: {out}");

        // 2) Header muncul; run SEC di direktori MICD yang SAMA.
        std::fs::write(sb.dir.join("defs.svh"), "`define WIDE 1\n").expect("tulis header");
        let mut args = vec![p.as_str(), "--formal-sec", q.as_str()];
        if let Some(f) = extra {
            args.push(f);
        }
        let (code, out) = sb.run(&args);
        assert_eq!(
            code, 60,
            "header baru harus mengubah verdict (bukan restore cache) {args:?}: {out}"
        );
    }
}

/// `-DFOO` (tanpa '=') harus sampai ke KEDUA desain dan memberi verdict yang
/// SAMA di legacy maupun --fast. Dulu run_fast membuang define tanpa '=' dan
/// identitas cache legacy mengabaikannya -> verdict berubah tanpa alasan.
#[test]
fn bare_define_reaches_both_designs_in_both_pipelines() {
    let sb = Sandbox::new("def");
    const IFDEF: &str = "module m(input logic [7:0] a, b, output logic [7:0] y);\n\
         `ifdef FOO\n  assign y = a & b;\n`else\n  assign y = a | b;\n`endif\nendmodule\n";
    let p_ifdef = sb.write("p_ifdef.sv", IFDEF);
    let q_ifdef = sb.write("q_ifdef.sv", IFDEF);
    let fixed_and = sb.write("fixed_and.sv", AND);
    let fixed_and2 = sb.write("fixed_and2.sv", AND);

    for extra in [None, Some("--fast")] {
        // (A) define harus sampai ke desain UTAMA (jalur preprocessor).
        // Urutan penting: run ber-define dulu, lalu TANPA define di direktori
        // MICD yang SAMA — cache yang mengabaikan define me-restore hasil run
        // pertama dan memberi EQUIVALENT palsu.
        let mut with = vec![
            p_ifdef.as_str(),
            "--formal-sec",
            fixed_and.as_str(),
            "-DFOO",
        ];
        let mut without = vec![p_ifdef.as_str(), "--formal-sec", fixed_and.as_str()];
        if let Some(f) = extra {
            with.push(f);
            without.push(f);
        }
        let (code, out) = sb.run(&with);
        assert_eq!(code, 0, "utama+define {with:?}: {out}");
        let (code, out) = sb.run(&without);
        assert_eq!(code, 60, "utama tanpa define {without:?}: {out}");

        // (B) define harus sampai ke desain KEDUA (CompileSession SEC).
        let mut with = vec![
            fixed_and2.as_str(),
            "--formal-sec",
            q_ifdef.as_str(),
            "-DFOO",
        ];
        let mut without = vec![fixed_and2.as_str(), "--formal-sec", q_ifdef.as_str()];
        if let Some(f) = extra {
            with.push(f);
            without.push(f);
        }
        let (code, out) = sb.run(&with);
        assert_eq!(code, 0, "kedua+define {with:?}: {out}");
        let (code, out) = sb.run(&without);
        assert_eq!(code, 60, "kedua tanpa define {without:?}: {out}");
    }
}
