//! Auto-regression: ubah temuan `.mivon-fuzz-bugs/*.sv` (+ `.txt`) menjadi
//! regression test — mewujudkan "1 bug = 1 test baru".
//!
//! Alur: `collect_bugs()` baca artefak bug → `emit_rust_tests()` hasilkan modul
//! test Rust (satu `#[test]` per bug) yang mem-replay source via
//! `oracle::evaluate` dan menegaskan kategori bug tercatat TIDAK lagi
//! mereproduksi. Setelah dev memperbaiki bug, test ini menjaga agar tidak
//! regresi.
//!
//! File hasil (default `crates/mivon-fuzz/tests/fuzz_regression.rs`) di-gate
//! `#![cfg(feature = "dev")]` supaya `cargo test --workspace` (tanpa dev) tak
//! ikut mengkompilasinya — crate mivon-fuzz polos tidak punya API publik.

use crate::{Category, Target};
use std::path::Path;

/// Batas ukuran source yg di-embed ke test (sisip sebagai string literal).
/// Di atas ini: skip (biasanya hang/perf raksasa — tak cocok embed, butuh
/// triage manual). 512KB cukup untuk sebagian besar bug semantik/fmt.
pub const EMBED_CAP: usize = 512 * 1024;

/// Timeout replay regression (lebih longgar dari kampanye: source bisa besar).
pub const REGRESS_TIMEOUT_MS: u64 = 20_000;

/// Timeout replay utk artefak besar via fixture — dibatasi (bukan 20s penuh)
/// supaya test hang raksasa tidak menghabiskan menit-menitan.
pub const FIXTURE_TIMEOUT_MS: u64 = 30_000;

/// Batas ukuran fixture yang boleh di-embed `include_str!`. >4MB = kasus
/// perf-class (kompilasi 11-13MB butuh waktu besar, terutama build debug) —
/// tidak praktis sebagai test; diarsipkan sbg komentar skip + butuh minimize
/// manual. Fixture kecil-menengah (<=4MB) tetap 1 bug = 1 test.
pub const FIXTURE_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Fingerprint konten (FNV-1a 64) — deteksi artefak duplikat.
pub fn content_fingerprint(s: &str) -> u64 {
    let mut h: u64 = 0xCBF29CE484222325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001B3);
    }
    h
}

/// Nama file fixture content-addressed: konten identik → satu file (duplikat
/// artefak 11MB tak disalin tiga kali).
pub fn fixture_name(s: &str) -> String {
    format!("{:016x}.sv", content_fingerprint(s))
}

/// Satu entri bug hasil scan direktori artefak.
#[derive(Debug, Clone)]
pub struct BugEntry {
    /// Nama file (tanpa ekstensi), mis. `bug_0000_roundtrip_mismatch`.
    pub name: String,
    /// Target pipeline tempat bug ditemukan.
    pub target: Target,
    /// Kategori bug tercatat (yang harus TIDAK lagi mereproduksi).
    pub category: Category,
    /// Source SV siap-evaluasi.
    pub source: String,
    /// True bila bug ini sengaja di-skip dari suite (lihat `skip_reason`).
    pub skipped: bool,
    /// Alasan skip (kosong bila tidak di-skip).
    pub skip_reason: String,
}

/// Escape string → aman utk string literal Rust (quote/backslash/control char).
/// Unicode cetak dibiarkan utuh (literal Rust menerimanya).
pub fn rust_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// Identifier aman utk nama fungsi test (alfanumerik + `_`).
fn safe_ident(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    // Pastikan prefix huruf/underscore (Rust ident tak boleh angka di awal).
    if out.chars().next().map(|c| c.is_alphanumeric() && c.is_numeric()).unwrap_or(true) {
        out.insert(0, '_');
    }
    out
}

/// Scan direktori artefak bug → entri (sorted by name, stabil).
///
/// Syarat masuk suite: ada pasangan `.txt` metadata (berisi `target:` &
/// `category:`) DAN `source.len() <= EMBED_CAP`. Tanpa `.txt` atau terlalu
/// besar → `skipped=true` (tetap dikembalikan agar pemanggil bisa melaporkan).
pub fn collect_bugs(dir: &Path) -> Vec<BugEntry> {
    let mut entries: Vec<BugEntry> = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return entries;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("sv") {
            continue;
        }
        let name = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("bug")
            .to_string();
        let Some(source) = std::fs::read_to_string(&p).ok() else {
            continue;
        };
        let txt = p.with_extension("txt");
        let (target, category) = match parse_meta(&txt) {
            Some((t, c)) => (t, c),
            None => {
                entries.push(BugEntry {
                    name,
                    target: Target::All,
                    category: Category::Panic,
                    source,
                    skipped: true,
                    skip_reason: "no metadata .txt (target/category unknown)".to_string(),
                });
                continue;
            }
        };
        let (skipped, reason) = if source.len() > EMBED_CAP {
            (true, format!("source {}B > EMBED_CAP {}B", source.len(), EMBED_CAP))
        } else {
            (false, String::new())
        };
        entries.push(BugEntry {
            name,
            target,
            category,
            source,
            skipped,
            skip_reason: reason,
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

/// Parse `target:` & `category:` dari file metadata `.txt`.
fn parse_meta(txt: &Path) -> Option<(Target, Category)> {
    let content = std::fs::read_to_string(txt).ok()?;
    let mut target = None;
    let mut category = None;
    for line in content.lines() {
        if let Some(v) = line.strip_prefix("target: ") {
            target = Target::from_token(v.trim());
        } else if let Some(v) = line.strip_prefix("category: ") {
            category = Category::from_label(v.trim());
        }
    }
    Some((target?, category?))
}

/// Hasilkan modul test Rust (string) dari kumpulan entri.
///
/// `fixtures`: direktori relatif test file tempat artefak besar sudah
/// di-copy. Entri `!skipped` di-embed inline; entri skip-karena-ukuran yang
/// fixture-nya ada di-emit `include_str!` (1 bug = 1 test TETAP, tanpa
/// skip); entri tanpa metadata tetap komentar. Setiap test mem-replay via
/// `oracle::evaluate` dan menegaskan kategori tercatat tak lagi muncul.
pub fn emit_rust_tests(entries: &[BugEntry], fixtures: &Path) -> String {
    let mut out = String::new();
    out.push_str("// GENERATED by `mivon-fuzz regress` — jangan edit manual.\n");
    out.push_str("// Auto-regression: 1 bug = 1 test. Replay tiap artefak bug dan\n");
    out.push_str("// tegaskan kategori tercatat TIDAK lagi mereproduksi.\n");
    out.push_str("#![cfg(feature = \"dev\")]\n\n");
    out.push_str("use mivon_fuzz::{oracle, Category, Target};\n\n");

    let active: Vec<&BugEntry> = entries.iter().filter(|e| !e.skipped).collect();
    // Entri skip-karena-ukuran yang fixture-nya tersedia → tetap jadi test
    // via `include_str!` (bukan komentar skip).
    let fixture_ready: Vec<&BugEntry> = entries
        .iter()
        .filter(|e| {
            e.skipped
                && e.skip_reason.starts_with("source ")
                && fixtures
                    .join(fixture_name(&e.source))
                    .metadata()
                    .map(|m| m.len() as usize <= FIXTURE_MAX_BYTES)
                    .unwrap_or(false)
        })
        .collect();
    let skipped: Vec<&BugEntry> = entries
        .iter()
        .filter(|e| e.skipped && !fixture_ready.iter().any(|f| f.name == e.name))
        .collect();

    if !skipped.is_empty() {
        out.push_str("// --- SKIPPED (tidak di-embed, butuh triage manual) ---\n");
        for e in &skipped {
            out.push_str(&format!(
                "//   {} [{}] — {}\n",
                e.name, e.target.as_str(), e.skip_reason
            ));
        }
        out.push('\n');
    }

    for e in &active {
        let t = e.target.as_str();
        let c = e.category.label();
        let src = rust_escape(&e.source);
        out.push_str(&format!(
            "#[test]\nfn reg_{}() {{\n    let src = \"{}\";\n",
            safe_ident(&e.name),
            src
        ));
        out.push_str(&format!(
            "    let target = Target::from_token(\"{}\").expect(\"target valid\");\n",
            t
        ));
        out.push_str(&format!(
            "    let expected = Category::from_label(\"{}\").expect(\"category valid\");\n",
            c
        ));
        out.push_str(&format!(
            "    let r = oracle::evaluate(target, src, {});\n",
            REGRESS_TIMEOUT_MS
        ));
        out.push_str(&format!(
            "    assert_ne!(\n        r.category, expected,\n        \"regression: {} reproduced ({{:?}}): {{}}\",\n        r.category, r.detail.lines().next().unwrap_or(\"\")\n    );\n",
            e.name
        ));
        out.push_str("}\n\n");
    }

    // Fixture (artefak besar): `include_str!` — tetap 1 test. Sumber IDENTIK
    // (bug yang sama disimpan dari beberapa kampanye) hanya di-emit SEKALI;
    // duplikat dicatat sebagai komentar.
    let mut seen_fp: Vec<u64> = Vec::new();
    let mut dup_comments: Vec<String> = Vec::new();
    for e in &fixture_ready {
        let fp = content_fingerprint(&e.source);
        if seen_fp.contains(&fp) {
            dup_comments.push(format!("//   {} — duplikat sumber (bug sama, sudah ada test)", e.name));
            continue;
        }
        seen_fp.push(fp);
        let t = e.target.as_str();
        let c = e.category.label();
        out.push_str(&format!(
            "#[test]\nfn reg_{}() {{\n    let src = include_str!(\"fixtures/{}\");\n",
            safe_ident(&e.name),
            fixture_name(&e.source)
        ));
        out.push_str(&format!(
            "    let target = Target::from_token(\"{}\").expect(\"target valid\");\n",
            t
        ));
        out.push_str(&format!(
            "    let expected = Category::from_label(\"{}\").expect(\"category valid\");\n",
            c
        ));
        out.push_str(&format!(
            "    let r = oracle::evaluate(target, src, {});\n",
            FIXTURE_TIMEOUT_MS
        ));
        out.push_str(&format!(
            "    assert_ne!(\n        r.category, expected,\n        \"regression: {} reproduced ({{:?}}): {{}}\",\n        r.category, r.detail.lines().next().unwrap_or(\"\")\n    );\n",
            e.name
        ));
        out.push_str("}\n\n");
    }
    if !dup_comments.is_empty() {
        out.push_str("// --- DUPLIKAT (sumber identik, test sudah ada) ---\n");
        for c in &dup_comments {
            out.push_str(c);
            out.push('\n');
        }
        out.push('\n');
    }

    if active.is_empty() && fixture_ready.is_empty() {
        // Hindari file kosong tak ada test (cargo test tetap OK, tapi beri
        // test penanda agar jelas suite aktif).
        out.push_str(
            "#[test]\nfn reg_no_active_bugs() {\n    // Tidak ada bug aktif ter-embed — suite regresi kosong.\n    assert!(true);\n}\n",
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_escape_handles_quotes_and_control() {
        assert_eq!(rust_escape("a\"b\\c"), "a\\\"b\\\\c");
        assert_eq!(rust_escape("line1\nline2"), "line1\\nline2");
        assert!(!rust_escape("tab\there").contains('\t'), "tab harus \\t");
        // Unicode cetak tetap utuh.
        assert_eq!(rust_escape("modüle é中"), "modüle é中");
    }

    #[test]
    fn safe_ident_replaces_non_alnum() {
        assert_eq!(safe_ident("bug_0000.roundtrip-mismatch"), "bug_0000_roundtrip_mismatch");
        assert!(safe_ident("123abc").starts_with('_'), "awalan angka → prefix _");
    }

    #[test]
    fn collect_bugs_parses_meta_and_skips_oversize() {
        let dir = std::env::temp_dir().join(format!("fzreg_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // Bug kecil dgn metadata lengkap.
        let sv1 = dir.join("bug_x_roundtrip.sv");
        std::fs::write(&sv1, "module m; endmodule").unwrap();
        std::fs::write(
            sv1.with_extension("txt"),
            "oracle: O3-roundtrip\ncategory: roundtrip_mismatch\ntarget: fmt\n",
        )
        .unwrap();
        // Bug tanpa metadata → skip.
        let sv2 = dir.join("bug_y.sv");
        std::fs::write(&sv2, "module n; endmodule").unwrap();
        // Bug raksasa → skip (over cap).
        let sv3 = dir.join("bug_z_hang.sv");
        std::fs::write(&sv3, "x".repeat(EMBED_CAP + 10)).unwrap();
        std::fs::write(
            sv3.with_extension("txt"),
            "oracle: O1-no-crash\ncategory: hang\ntarget: elab\n",
        )
        .unwrap();

        let entries = collect_bugs(&dir);
        assert_eq!(entries.len(), 3);
        let x = entries.iter().find(|e| e.name == "bug_x_roundtrip").unwrap();
        assert!(!x.skipped);
        assert_eq!(x.target, Target::Fmt);
        assert_eq!(x.category, Category::RoundtripMismatch);
        let y = entries.iter().find(|e| e.name == "bug_y").unwrap();
        assert!(y.skipped);
        assert!(y.skip_reason.contains("no metadata"));
        let z = entries.iter().find(|e| e.name == "bug_z_hang").unwrap();
        assert!(z.skipped);
        assert!(z.skip_reason.contains("EMBED_CAP"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn emit_produces_one_test_per_active_bug() {
        let dir = std::env::temp_dir().join(format!("fzemit_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let sv = dir.join("bug_a_fmt.sv");
        std::fs::write(&sv, "module m; endmodule").unwrap();
        std::fs::write(
            sv.with_extension("txt"),
            "category: roundtrip_mismatch\ntarget: fmt\n",
        )
        .unwrap();
        let entries = collect_bugs(&dir);
        let code = emit_rust_tests(&entries, &dir);
        assert!(code.contains("#![cfg(feature = \"dev\")]"));
        assert!(code.contains("fn reg_bug_a_fmt()"));
        assert!(code.contains("Target::from_token(\"fmt\")"));
        assert!(code.contains("Category::from_label(\"roundtrip_mismatch\")"));
        assert!(code.contains("oracle::evaluate"));
        assert!(code.contains("assert_ne!"));
        // Source dg quote harus tergenerate escaped.
        let sv2 = dir.join("bug_b_quote.sv");
        std::fs::write(&sv2, "module \"q\"; endmodule").unwrap();
        std::fs::write(
            sv2.with_extension("txt"),
            "category: panic\ntarget: parser\n",
        )
        .unwrap();
        let entries2 = collect_bugs(&dir);
        let code2 = emit_rust_tests(&entries2, &dir);
        assert!(code2.contains("let src = \"module \\\"q\\\"; endmodule\""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn emit_uses_fixture_for_oversized_when_available() {
        let dir = std::env::temp_dir().join(format!("fzfix_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let fx = dir.join("fixtures");
        std::fs::create_dir_all(&fx).unwrap();
        let big = "x".repeat(EMBED_CAP + 10);
        let sv = dir.join("bug_big_hang.sv");
        std::fs::write(&sv, &big).unwrap();
        std::fs::write(
            sv.with_extension("txt"),
            "category: hang\ntarget: sim\n",
        )
        .unwrap();
        // Fixture belum tersedia → tetap skip (komentar).
        let entries = collect_bugs(&dir);
        let code = emit_rust_tests(&entries, &fx);
        assert!(code.contains("SKIPPED"), "tanpa fixture = skip");
        assert!(!code.contains("include_str!"), "belum ada fixture");
        // Fixture disediakan (content-addressed) → 1 bug tetap 1 test.
        let fx_name = fixture_name(&big);
        std::fs::write(fx.join(&fx_name), &big).unwrap();
        let code2 = emit_rust_tests(&entries, &fx);
        assert!(code2.contains("fn reg_bug_big_hang()"));
        assert!(code2.contains(&format!("include_str!(\"fixtures/{fx_name}\")")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn regress_target_and_category_from_labels_roundtrip() {
        // Kontrak emit: from_token/from_label yg dipakai test valid utk semua
        // target/kategori yg mungkin muncul di artefak.
        for t in Target::ALL {
            assert_eq!(Target::from_token(t.as_str()), Some(*t));
        }
        for c in [
            Category::Panic,
            Category::Abort,
            Category::Hang,
            Category::DiagMissing,
            Category::RoundtripMismatch,
            Category::NonDeterministic,
            Category::GuardBypass,
            Category::Differential,
            Category::HiddenBug,
            Category::LrmViolation,
        ] {
            assert_eq!(Category::from_label(c.label()), Some(c));
        }
    }
}
