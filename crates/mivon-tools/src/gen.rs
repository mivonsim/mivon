//! `mgen` — Generator SystemVerilog dari Mivon HDL (.mv / .mvh).
//!
//! Membaca file `.mv` (desain → `.sv` + `.svh`) atau `.mvh` (header →
//! `.svh` saja, F43), me-transpile (MIVON-HDL.md). Output deterministik —
//! bisa di-commit ke repo dan di-`--check` di CI.
//!
//! Mode:
//! - default : tulis `<base>.sv` + `<base>.svh` di direktori input (atau `-o`);
//!   file `.mvh` hanya menghasilkan `<base>.svh`
//! - `--stdout` : print `.sv` ke stdout (debug; `.mvh` → print `.svh`)
//! - `--check`  : verifikasi output up-to-date — exit 1 bila beda (CI)
//! - `--svh-only` / `--sv-only` : hanya satu file output

use std::path::{Path, PathBuf};

use mivon_core::error::SimError;
use mivon_mv as mv;

/// Apakah path sumber Mivon HDL (`.mv` desain / `.mvh` header, F43)?
pub fn is_mv_source(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("mv" | "mvh")
    )
}

/// Opsi mgen.
pub struct GenArgs<'a> {
    pub targets: &'a [String],
    pub output: Option<String>,
    pub stdout: bool,
    pub check: bool,
    pub svh_only: bool,
    pub sv_only: bool,
    pub no_check: bool,
    /// Bungkus typedef level file dalam package ini (MIVON-HDL.md §11).
    pub package: Option<String>,
    pub verbose: bool,
}

fn diag(msg: impl Into<String>) -> SimError {
    SimError::with_diag(mivon_core::diagnostics::DiagCode::InvalidSyntax, msg)
}

/// Jalankan mgen.
pub fn run(args: &GenArgs) -> Result<(), SimError> {
    if args.stdout && args.targets.len() != 1 {
        return Err(diag("--stdout hanya untuk satu file .mv/.mvh"));
    }
    if args.stdout && args.check {
        return Err(diag("--stdout tidak bisa digabung dengan --check"));
    }

    let files = collect_mv_files(args.targets)?;
    if files.is_empty() {
        return Err(diag("tidak ada file .mv/.mvh ditemukan"));
    }

    let out_dir: Option<PathBuf> = args.output.as_ref().map(PathBuf::from);
    if let Some(d) = &out_dir {
        if !d.exists() {
            std::fs::create_dir_all(d).map_err(|e| {
                diag(format!(
                    "tidak bisa membuat direktori '{}': {}",
                    d.display(),
                    e
                ))
            })?;
        }
    }

    // ── F9: transpile batch (konteks gabungan lintas file) ──
    // Semua file dibaca dulu, lalu di-transpile BERSAMA — tipe/package dari
    // satu file terlihat oleh file lain (`types.mvh` → `counter.mv`).
    // Flag `header` (F43) menandai sumber `.mvh` → output `.svh` saja.
    let mut items: Vec<mv::MvItem> = Vec::with_capacity(files.len());
    for path in &files {
        let base = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| diag(format!("nama file tidak valid: '{}'", path.display())))?
            .to_string();
        let src = std::fs::read_to_string(path)
            .map_err(|e| diag(format!("{}: {}", path.display(), e)))?;
        let header = path.extension().map(|e| e == "mvh").unwrap_or(false);
        items.push(mv::MvItem {
            src,
            base,
            header,
            package: args.package.clone(),
        });
    }
    let results = transpile_by_directory(&files, &items, args.no_check)?;

    let mut changed_any = false;
    for (path, result) in files.iter().zip(results.iter()) {
        let base = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| diag(format!("nama file tidak valid: '{}'", path.display())))?;
        // Sumber `.mvh` (F43): header-only — satu-satunya output `.svh`.
        let is_header = path.extension().map(|e| e == "mvh").unwrap_or(false);

        // ── --stdout: print .sv (.mvh → .svh, karena .sv memang kosong) ──
        if args.stdout {
            if is_header {
                print!("{}", result.svh);
            } else {
                print!("{}", result.sv);
            }
            continue;
        }

        let svh_path = target_path(&out_dir, path, base, "svh");
        let sv_path = target_path(&out_dir, path, base, "sv");

        // ── --check: bandingkan dengan file existing ──
        // `.svh` kosong = file tidak membutuhkan definisi bersama → tidak ada
        // file yang diharapkan (konsisten dengan generate_svh yang skip).
        if args.check {
            let svh_ok = result.svh.is_empty() || file_matches(&svh_path, &result.svh);
            // F30 fix: .sv kosong (file hanya definisi bersama, tanpa module/
            // program/class/func/task) → tidak ada file yang diharapkan,
            // konsisten dengan skip .svh kosong.
            // F43: sumber `.mvh` tidak pernah menghasilkan `.sv` — abaikan
            // sisi `.sv` (file `.sv` basi bukan urusan header).
            let sv_ok = is_header
                || result.sv.is_empty()
                || file_matches(&sv_path, &result.sv);
            let ok = if args.svh_only {
                svh_ok
            } else if args.sv_only && !is_header {
                sv_ok
            } else {
                svh_ok && sv_ok
            };
            if ok {
                if args.verbose {
                    println!("  ✓ {} — up-to-date", path.display());
                }
            } else {
                println!("  ! {} — perlu regenerate", path.display());
                changed_any = true;
            }
            continue;
        }

        // ── tulis file ──
        // `.svh` kosong (tanpa package/typedef) → jangan tulis file sama sekali.
        if !args.sv_only && !result.svh.is_empty() {
            let changed = write_if_changed(&svh_path, &result.svh)?;
            if changed || args.verbose {
                println!("  generated {}", svh_path.display());
            }
            changed_any |= changed;
        } else if args.svh_only && result.svh.is_empty() && !args.stdout && !args.check {
            println!("  (skip .svh — file tidak punya package/typedef)");
        }
        if is_header {
            // F43: `.mvh` = header — hanya `.svh`. `.sv` tidak pernah ditulis
            // (konten fungsional sudah ditolak E2008 di level .mvh).
            if args.sv_only {
                println!("  (skip .sv — '{}' sumber .mvh header-only)", base);
            }
            continue;
        }
        if !args.svh_only && !result.sv.is_empty() {
            let changed = write_if_changed(&sv_path, &result.sv)?;
            if changed || args.verbose {
                println!("  generated {}", sv_path.display());
            }
            changed_any |= changed;
        } else if !args.svh_only {
            // F30 fix: .sv tanpa konten fungsional tidak ditulis. Bila file
            // .sv LAMA masih ada (dari generate versi sebelumnya) → peringatan
            // agar user menghapusnya manual (`mgen` tidak pernah menghapus).
            if sv_path.exists() {
                println!("  ! obsolete {}.sv terdeteksi — hapus manual (file definisi-only tak lagi ditulis)", base);
            } else if !args.stdout && !args.check {
                println!("  (skip .sv — file tidak punya module/program/class/func/task)");
            }
        }
    }

    if args.check && changed_any {
        return Err(diag(
            "mgen --check: ada file .mv/.mvh yang belum di-generate — jalankan `mivon mgen <file.mv>`",
        ));
    }
    if !args.stdout && !args.check && !args.verbose {
        println!("mgen: {} file .mv/.mvh diproses", files.len());
    }
    Ok(())
}

/// Transpile seluruh input dengan konteks gabungan **per direktori**.
///
/// Konteks gabungan (F9) dibutuhkan agar `types.mv` mendefinisikan tipe yang
/// dipakai `counter.mv` di direktori yang sama. Tapi kalau seluruh pohon
/// direktori diratakan jadi SATU namespace, dua subdirektori yang sama-sama
/// mendefinisikan `Word16` (pola umum: `examples/mv/cast.mv` dan
/// `examples/mv/type_param.mv`) saling menabrak dengan E2007 — padahal
/// keduanya independen dan keduanya sah. Karena itu pengelompokan per
/// direktori: satu grup = satu konteks `check_many`.
///
/// Hasil tetap sejajar dengan `files` (urutan input dipertahankan).
fn transpile_by_directory(
    files: &[PathBuf],
    items: &[mv::MvItem],
    no_check: bool,
) -> Result<Vec<mv::TranspileResult>, SimError> {
    // Grup indeks input berdasarkan direktori induknya, urutan kemunculan
    // dipertahankan (deterministik — prinsip desain #2).
    let mut groups: Vec<(PathBuf, Vec<usize>)> = Vec::new();
    for (i, path) in files.iter().enumerate() {
        let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        match groups.iter_mut().find(|(d, _)| *d == dir) {
            Some((_, idxs)) => idxs.push(i),
            None => groups.push((dir, vec![i])),
        }
    }

    let mut out: Vec<Option<mv::TranspileResult>> = vec![None; files.len()];
    for (_, idxs) in &groups {
        let sub: Vec<mv::MvItem> = idxs.iter().map(|i| items[*i].clone()).collect();
        let res = if no_check {
            mv::transpile_many_items_no_check(&sub)
        } else {
            mv::transpile_many_items(&sub)
        }
        .map_err(|(k, e)| {
            let i = idxs[k];
            diag(mv::format_error(
                &files[i].display().to_string(),
                &items[i].src,
                &e,
            ))
        })?;
        for (k, i) in idxs.iter().enumerate() {
            out[*i] = Some(res[k].clone());
        }
    }
    // Defensif: setiap input harus punya hasil (jangan zip-truncate diam-diam).
    let mut done: Vec<mv::TranspileResult> = Vec::with_capacity(files.len());
    for (i, r) in out.into_iter().enumerate() {
        match r {
            Some(r) => done.push(r),
            None => {
                return Err(diag(format!(
                    "transpile batch tidak menghasilkan output untuk '{}'",
                    files[i].display()
                )));
            }
        }
    }
    Ok(done)
}

/// Kumpulkan file `.mv`/`.mvh` dari target (file atau direktori recursive).
fn collect_mv_files(targets: &[String]) -> Result<Vec<PathBuf>, SimError> {
    let mut out: Vec<PathBuf> = Vec::new();
    for t in targets {
        let p = Path::new(t);
        if !p.exists() {
            return Err(diag(format!("path tidak ditemukan: '{}'", t)));
        }
        if p.is_dir() {
            collect_dir(p, &mut out)?;
        } else {
            out.push(p.to_path_buf());
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn collect_dir(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), SimError> {
    for entry in std::fs::read_dir(dir).map_err(|e| diag(format!("{}: {}", dir.display(), e)))? {
        let path = entry.map_err(|e| diag(e.to_string()))?.path();
        if path.is_dir() {
            if is_skipped_dir(&path) {
                continue;
            }
            collect_dir(&path, out)?;
        } else if is_mv_source(&path) && !is_skipped_file(&path) {
            out.push(path);
        }
    }
    Ok(())
}

/// Direktori yang dilewati saat scan `.mv` rekursif.
///
/// `negative/` = fixture SENGAJA tidak valid (dipakai test diagnostik) —
/// mem-buildnya akan selalu gagal, jadi tidak boleh ikut `mgen <dir>`.
/// Direktori tersembunyi juga dilewati (konvensi umum tooling).
fn is_skipped_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with('.') || n == "negative")
        .unwrap_or(false)
}

/// File yang dilewati saat scan: nama diawali `_` (konvensi "jangan dipakai",
/// tapi tetap ada di repo) — lihat `examples/mv/negative/`.
fn is_skipped_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with('_'))
        .unwrap_or(false)
}

/// Path output: `-o dir` → dir, selain itu di samping file input.
fn target_path(out_dir: &Option<PathBuf>, input: &Path, base: &str, ext: &str) -> PathBuf {
    match out_dir {
        Some(d) => d.join(format!("{base}.{ext}")),
        None => input.with_file_name(format!("{base}.{ext}")),
    }
}

fn file_matches(path: &Path, content: &str) -> bool {
    std::fs::read_to_string(path)
        .map(|s| s == content)
        .unwrap_or(false)
}

/// Tulis hanya bila konten berubah (deterministik + tidak sentuh mtime bila sama).
fn write_if_changed(path: &Path, content: &str) -> Result<bool, SimError> {
    if file_matches(path, content) {
        return Ok(false);
    }
    std::fs::write(path, content).map_err(|e| diag(format!("{}: {}", path.display(), e)))?;
    Ok(true)
}
