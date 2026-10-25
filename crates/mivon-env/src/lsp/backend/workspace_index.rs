//! LSP-11 tahap 2: indeks workspace dari DISK (bukan hanya dokumen yang
//! sedang terbuka di editor).
//!
//! Tahap 1 (ROUND 99) hanya mem-flatten outline dokumen terbuka. Akibatnya
//! `workspace/symbol` tidak menemukan simbol di file yang belum pernah
//! dibuka. Modul ini menyediakan pemindaian direktori yang deterministik
//! (BFS level-order + anak terurut), batas keras (jumlah file, kedalaman,
//! ukuran per file, total byte, jumlah DIREKTORI dipindai), dan penggabungan
//! dokumen (dokumen terbuka menang atas versi disk).
//!
//! Caching BELUM ada (masih open di audit): pemindaian dilakukan per
//! permintaan `workspace/symbol` — pemanggil menjalankannya di
//! `spawn_blocking` supaya event loop LSP tidak stall.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

/// Ekstensi sumber yang diindeks.
pub(crate) const SOURCE_EXTS: [&str; 5] = ["sv", "svh", "v", "vh", "svi"];

/// Direktori yang tidak pernah diindeks (build artifact / metadata VCS).
/// Perbandingan case-insensitive (FS macOS/Windows case-insensitive).
pub(crate) const SKIP_DIRS: [&str; 5] = [".git", "target", "node_modules", ".svn", "build"];

/// Batas jumlah file yang diindeks per permintaan.
pub(crate) const MAX_INDEX_FILES: usize = 2000;
/// Batas kedalaman traversal direktori.
pub(crate) const MAX_INDEX_DEPTH: usize = 16;
/// Batas ukuran satu file yang dibaca (byte).
pub(crate) const MAX_INDEX_BYTES: u64 = 1024 * 1024;
/// Batas TOTAL byte yang dibaca per pemindaian (anti-blocking / anti-OOM).
pub(crate) const MAX_INDEX_TOTAL_BYTES: u64 = 32 * 1024 * 1024;
/// Batas jumlah DIREKTORI yang dipindai (pohon direktori raksasa).
/// Dihitung per direktori, bukan per entri: repo dengan banyak file
/// non-sumber tidak boleh "menghabiskan" kuota sebelum traversal selesai.
pub(crate) const MAX_SCAN_DIRS: usize = 5_000;

/// Apakah path berekstensi sumber yang diindeks (case-insensitive).
pub(crate) fn is_source_file(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => {
            let lower = ext.to_ascii_lowercase();
            SOURCE_EXTS.contains(&lower.as_str())
        }
        None => false,
    }
}

/// Apakah nama direktori harus dilewati (case-insensitive).
fn is_skipped_dir(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SKIP_DIRS.contains(&lower.as_str())
}

/// Kumpulkan file sumber di bawah `root` (BFS level-order, deterministik).
///
/// Kenapa BFS (bukan DFS stack LIFO): DFS membuat urutan kunjungan terbalik
/// terhadap hasil `sort()` sehingga saat `max_files` habis, yang terpotong
/// adalah himpunan acak-terbalik — direktori alfabetis awal bisa TIDAK
/// pernah terindeks (review R192 BUG-1). BFS menjamin N file pertama =
/// yang paling dangkal lalu alfabetis.
///
/// Symlink TIDAK di-follow (`DirEntry::file_type`) — mencegah file yang sama
/// terkumpul berkali-kali lewat symlink direktori (review R192 BUG-2).
pub(crate) fn collect_source_files(
    root: &Path,
    max_files: usize,
    max_depth: usize,
) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if !root.is_dir() {
        return out;
    }
    let mut queue: VecDeque<(PathBuf, usize)> = VecDeque::new();
    queue.push_back((root.to_path_buf(), 0));
    let mut dirs_scanned = 0usize;
    // Dedupe file lewat canonical path: symlink ke FILE sumber tetap
    // diindeks (review R192 BUG-5: jangan buang symlink file), tapi file
    // yang sama tidak boleh masuk dua kali.
    let mut seen_files: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    while let Some((dir, depth)) = queue.pop_front() {
        if out.len() >= max_files || dirs_scanned >= MAX_SCAN_DIRS {
            break;
        }
        dirs_scanned += 1;
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut children: Vec<(PathBuf, std::fs::FileType)> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_type().ok().map(|t| (e.path(), t)))
            .collect();
        children.sort_by(|a, b| a.0.cmp(&b.0));
        for (path, ft) in children {
            if out.len() >= max_files {
                break;
            }
            // Symlink yang menuju DIREKTORI dilewati (cegah loop + duplikat
            // lewat alias dir). Symlink ke FILE sumber tetap diindeks.
            if ft.is_symlink() {
                if path.is_dir() {
                    continue;
                }
            } else if ft.is_dir() {
                if depth + 1 > max_depth {
                    continue;
                }
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                if is_skipped_dir(name) {
                    continue;
                }
                queue.push_back((path, depth + 1));
                continue;
            }
            if is_source_file(&path) && path.is_file() {
                let canon = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                if seen_files.insert(canon) {
                    out.push(path);
                }
            }
        }
    }
    // Deterministik: himpunan sudah dibatasi max_files saat traversal (BFS
    // level-order), sort hanya menormalkan urutan keluaran.
    out.sort();
    out
}

/// Gabungkan dokumen terbuka (editor) dgn dokumen dari disk.
///
/// Dokumen TERBUKA MENANG untuk uri yang sama: isi editor bisa lebih baru
/// daripada file (belum disimpan). Urutan hasil deterministik: dokumen
/// terbuka lebih dulu (urutan asli), lalu dokumen disk yang belum ada.
pub(crate) fn merge_docs(
    open: &[(String, String)],
    disk: &[(String, String)],
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::with_capacity(open.len() + disk.len());
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (uri, text) in open {
        if seen.insert(uri.as_str()) {
            out.push((uri.clone(), text.clone()));
        }
    }
    for (uri, text) in disk {
        if seen.insert(uri.as_str()) {
            out.push((uri.clone(), text.clone()));
        }
    }
    out
}

/// Urutkan dokumen berdasarkan URI (menaik).
///
/// Wajib dipakai sebelum `find_references_in_docs`: sumber dokumen terbuka
/// adalah `HashMap` (urutan iterasi acak) sehingga urutan Location — dan
/// himpunan hasil saat cap tercapai — bisa berbeda antar request
/// (review R193 BUG-1). Untuk `search_workspace_symbols` ini no-op karena
/// hasilnya sudah diurutkan sendiri (uri, line, col).
pub(crate) fn sorted_docs(mut docs: Vec<(String, String)>) -> Vec<(String, String)> {
    docs.sort_by(|a, b| a.0.cmp(&b.0));
    docs
}

/// Root workspace dari parameter `initialize` (root_uri diutamakan,
/// fallback workspace_folders pertama). URI non-file diabaikan.
pub(crate) fn workspace_root_from_params(params: &lsp_types::InitializeParams) -> Option<PathBuf> {
    params
        .root_uri
        .as_ref()
        .and_then(|u| u.to_file_path().ok())
        .or_else(|| {
            params
                .workspace_folders
                .as_ref()
                .and_then(|f| f.first())
                .and_then(|f| f.uri.to_file_path().ok())
        })
}

/// Baca dokumen sumber dari disk di bawah `root` (siap dipakai
/// `search_workspace_symbols`). `skip` = path file yang sudah terbuka di
/// editor (dilewati: isinya sudah ada di cache dokumen dan versinya lebih
/// baru; sekaligus menghemat I/O).
///
/// Batas: ukuran per file (MAX_INDEX_BYTES) dan TOTAL byte
/// (MAX_INDEX_TOTAL_BYTES) — berhenti lebih awal saat total terlampaui.
pub(crate) fn load_disk_docs(root: &Path, skip: &[PathBuf]) -> Vec<(String, String)> {
    load_disk_docs_with_limits(root, skip, MAX_INDEX_BYTES, MAX_INDEX_TOTAL_BYTES)
}

/// Seperti [`load_disk_docs`] tapi batas byte bisa di-inject (dipakai test
/// untuk mengunci batas TOTAL, bukan hanya batas per file).
pub(crate) fn load_disk_docs_with_limits(
    root: &Path,
    skip: &[PathBuf],
    max_file_bytes: u64,
    max_total_bytes: u64,
) -> Vec<(String, String)> {
    use std::collections::HashSet;
    let skip_set: HashSet<PathBuf> = skip
        .iter()
        .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()))
        .collect();
    let files = collect_source_files(root, MAX_INDEX_FILES, MAX_INDEX_DEPTH);
    let mut out: Vec<(String, String)> = Vec::with_capacity(files.len());
    let mut total_bytes: u64 = 0;
    for path in files {
        let canon = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if skip_set.contains(&canon) {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if meta.len() > max_file_bytes {
            continue;
        }
        if total_bytes + meta.len() > max_total_bytes {
            break;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        total_bytes += text.len() as u64;
        let Ok(url) = lsp_types::Url::from_file_path(&path) else {
            continue;
        };
        out.push((url.as_str().to_string(), text));
    }
    out
}

/// Path file yang sudah terbuka di editor (untuk dilewati saat scan disk).
/// URI non-file / tak valid diabaikan.
pub(crate) fn collect_skip_paths(open: &[(String, String)]) -> Vec<PathBuf> {
    open.iter()
        .filter_map(|(u, _)| lsp_types::Url::parse(u).ok())
        .filter_map(|u| u.to_file_path().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_source_file_matches_sv_extensions() {
        assert!(is_source_file(Path::new("a.sv")));
        assert!(is_source_file(Path::new("a.SV")));
        assert!(is_source_file(Path::new("pkg.svh")));
        assert!(is_source_file(Path::new("a.v")));
        assert!(is_source_file(Path::new("a.vh")));
        assert!(is_source_file(Path::new("a.svi")));
        assert!(!is_source_file(Path::new("a.txt")));
        assert!(!is_source_file(Path::new("a.sv.bak")));
        assert!(!is_source_file(Path::new("Makefile")));
        assert!(!is_source_file(Path::new("a")));
    }

    #[test]
    fn collect_source_files_recurses_sorts_and_skips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("rtl/sub")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("top.sv"), "module top; endmodule\n").unwrap();
        std::fs::write(root.join("rtl/a.sv"), "module a; endmodule\n").unwrap();
        std::fs::write(root.join("rtl/sub/b.svh"), "// hdr\n").unwrap();
        std::fs::write(root.join("rtl/notes.txt"), "bukan sumber\n").unwrap();
        std::fs::write(root.join(".git/ignored.sv"), "module g; endmodule\n").unwrap();
        std::fs::write(root.join("target/gen.sv"), "module t; endmodule\n").unwrap();

        let files = collect_source_files(root, 100, 8);
        let names: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(names, vec!["rtl/a.sv", "rtl/sub/b.svh", "top.sv"]);
    }

    #[test]
    fn collect_source_files_respects_limits() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        for i in 0..5 {
            std::fs::write(root.join(format!("m{i}.sv")), "module m; endmodule\n").unwrap();
        }
        assert_eq!(collect_source_files(root, 3, 8).len(), 3);
        // max_depth=0 → hanya isi root, subdir dilewati.
        std::fs::create_dir_all(root.join("deep")).unwrap();
        std::fs::write(root.join("deep/x.sv"), "module x; endmodule\n").unwrap();
        let shallow = collect_source_files(root, 100, 0);
        assert!(!shallow.iter().any(|p| p.ends_with("deep/x.sv")));
        // Root bukan direktori → kosong (tidak panic).
        assert!(collect_source_files(&root.join("tidak-ada"), 10, 4).is_empty());
    }

    /// Review R192 BUG-1: saat batas file tercapai, yang tersisa WAJIB file
    /// paling dangkal + alfabetis awal — bukan hasil traversal terbalik.
    #[test]
    fn collect_source_files_truncation_prefers_shallow_alphabetical() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        // Direktori alfabetis BELAKANGAN (z*) berisi file lebih banyak —
        // DFS stack LIFO dulu menghabiskan kuota di sini.
        std::fs::create_dir_all(root.join("z_last")).unwrap();
        std::fs::create_dir_all(root.join("a_first")).unwrap();
        for i in 0..4 {
            std::fs::write(
                root.join(format!("z_last/z{i}.sv")),
                "module z; endmodule\n",
            )
            .unwrap();
        }
        std::fs::write(root.join("a_first/a.sv"), "module a; endmodule\n").unwrap();
        std::fs::write(root.join("b.sv"), "module b; endmodule\n").unwrap();

        let files = collect_source_files(root, 2, 8);
        assert_eq!(files.len(), 2);
        let names: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        // Hasil akhir di-sort; yang WAJIB: tidak ada file z_last yang
        // menghabiskan kuota (DFS LIFO dulu justru mengambil z_last).
        assert_eq!(names, vec!["a_first/a.sv", "b.sv"]);
        assert!(
            !names.iter().any(|n| n.starts_with("z_last/")),
            "kuota tidak boleh dihabiskan direktori alfabetis belakangan: {names:?}"
        );
    }

    /// Review R192 BUG-2: symlink direktori tidak boleh menyebabkan file
    /// yang sama terkumpul berulang (kuota habis oleh duplikat).
    #[cfg(unix)]
    #[test]
    fn collect_source_files_does_not_follow_symlinked_dirs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("rtl")).unwrap();
        std::fs::write(root.join("rtl/a.sv"), "module a; endmodule\n").unwrap();
        // rtl/loop -> rtl (self-loop) dan top-level link ke rtl.
        std::os::unix::fs::symlink(root.join("rtl"), root.join("rtl/loop")).unwrap();
        std::os::unix::fs::symlink(root.join("rtl"), root.join("alias")).unwrap();

        let files = collect_source_files(root, 100, 8);
        assert_eq!(files.len(), 1, "symlink tidak boleh diduplikasi: {files:?}");
        assert!(files[0].ends_with("rtl/a.sv"));
    }

    /// Review R192 BUG-5: symlink ke FILE sumber harus tetap terindeks
    /// (mis. file di-vendor/di-share lewat symlink), dan dedupe canonical
    /// mencegah file yang sama muncul dua kali.
    #[cfg(unix)]
    #[test]
    fn collect_source_files_keeps_symlinked_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("shared")).unwrap();
        std::fs::create_dir_all(root.join("rtl")).unwrap();
        std::fs::write(root.join("shared/uart.sv"), "module uart; endmodule\n").unwrap();
        std::os::unix::fs::symlink(root.join("shared/uart.sv"), root.join("rtl/uart.sv")).unwrap();

        let files = collect_source_files(root, 100, 8);
        assert!(
            files.iter().any(|p| p.ends_with("rtl/uart.sv")),
            "symlink ke file sumber harus diindeks: {files:?}"
        );
        // shared/uart.sv + rtl/uart.sv = dua PATH, tapi canonical sama → 1.
        assert_eq!(
            files.len(),
            1,
            "file yang sama tidak boleh dobel: {files:?}"
        );
    }

    /// Review ronde 3 R1/N2: hanya FILE REGULER yang dikumpulkan — dangling
    /// symlink .sv tidak boleh menghabiskan kuota, dan FIFO/device bernama
    /// .sv bisa menggantung read_to_string (dilewati sebelum dibaca).
    #[cfg(unix)]
    #[test]
    fn collect_source_files_skips_non_regular_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("ok.sv"), "module ok; endmodule\n").unwrap();
        std::os::unix::fs::symlink(root.join("tidak-ada.sv"), root.join("dangling.sv")).unwrap();

        let files = collect_source_files(root, 100, 8);
        assert_eq!(files.len(), 1, "{files:?}");
        assert!(files[0].ends_with("ok.sv"));

        // Tanpa file reguler sama sekali → kosong (dangling tidak dihitung).
        std::fs::remove_file(root.join("ok.sv")).unwrap();
        assert!(collect_source_files(root, 100, 8).is_empty());
    }

    #[test]
    fn merge_docs_open_document_wins() {
        let open = vec![
            ("file:///a.sv".to_string(), "OPEN-A".to_string()),
            ("file:///b.sv".to_string(), "OPEN-B".to_string()),
        ];
        let disk = vec![
            ("file:///a.sv".to_string(), "DISK-A".to_string()),
            ("file:///c.sv".to_string(), "DISK-C".to_string()),
        ];
        let merged = merge_docs(&open, &disk);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].1, "OPEN-A");
        assert_eq!(merged[1].1, "OPEN-B");
        assert_eq!(merged[2].0, "file:///c.sv");
        assert_eq!(merged[2].1, "DISK-C");
    }

    /// Review R193 ronde 2 BUG-1: helper produksi wajib MENGURUTKAN — test
    /// ini gagal bila body sorted_docs dihapus (masukan tak terurut).
    #[test]
    fn sorted_docs_orders_by_uri() {
        let docs = vec![
            ("file:///c.sv".to_string(), "c".to_string()),
            ("file:///a.sv".to_string(), "a".to_string()),
            ("file:///b.sv".to_string(), "b".to_string()),
            ("file:///a.sv".to_string(), "a-dup".to_string()),
        ];
        let out = sorted_docs(docs);
        let uris: Vec<&str> = out.iter().map(|d| d.0.as_str()).collect();
        assert_eq!(
            uris,
            vec![
                "file:///a.sv",
                "file:///a.sv",
                "file:///b.sv",
                "file:///c.sv"
            ]
        );
        // Stabil: entri duplikat mempertahankan urutan relatif asli.
        assert_eq!(out[0].1, "a");
        assert_eq!(out[1].1, "a-dup");
        // Idempoten.
        let again = sorted_docs(out.clone());
        assert_eq!(again, out);
        // Kosong / satu elemen aman.
        assert!(sorted_docs(Vec::new()).is_empty());
        assert_eq!(
            sorted_docs(vec![("file:///x.sv".into(), "x".into())]).len(),
            1
        );
    }

    #[test]
    fn workspace_root_prefers_root_uri_then_folders() {
        let mut params = lsp_types::InitializeParams::default();
        assert!(workspace_root_from_params(&params).is_none());

        params.root_uri = Some(lsp_types::Url::parse("file:///work").unwrap());
        assert_eq!(
            workspace_root_from_params(&params),
            Some(PathBuf::from("/work"))
        );

        // root_uri non-file → fallback workspace_folders[0].
        params.root_uri = Some(lsp_types::Url::parse("vscode-remote://host/work").unwrap());
        params.workspace_folders = Some(vec![lsp_types::WorkspaceFolder {
            uri: lsp_types::Url::parse("file:///folders/first").unwrap(),
            name: "first".to_string(),
        }]);
        assert_eq!(
            workspace_root_from_params(&params),
            Some(PathBuf::from("/folders/first"))
        );

        // root_uri file menang atas workspace_folders.
        params.root_uri = Some(lsp_types::Url::parse("file:///root/uri").unwrap());
        assert_eq!(
            workspace_root_from_params(&params),
            Some(PathBuf::from("/root/uri"))
        );
    }

    #[test]
    fn load_disk_docs_skips_open_files_and_caps_total_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("a.sv"), "module a; endmodule\n").unwrap();
        std::fs::write(root.join("b.sv"), "module b; endmodule\n").unwrap();
        // File non-UTF8 → dilewati senyap (tidak panic).
        std::fs::write(root.join("bad.sv"), [0xff, 0xfe, 0x00]).unwrap();

        let all = load_disk_docs(root, &[]);
        assert_eq!(all.len(), 2, "{all:?}");
        assert!(all.iter().all(|(uri, _)| uri.starts_with("file://")));

        // File yang sudah terbuka di editor dilewati (path atau canonical).
        let skip = vec![root.join("a.sv")];
        let rest = load_disk_docs(root, &skip);
        assert_eq!(rest.len(), 1, "{rest:?}");
        assert!(rest[0].1.contains("module b"));

        // File > batas per file dilewati, file lain tetap dibaca.
        let big = "x".repeat(64);
        std::fs::write(root.join("c.sv"), &big).unwrap();
        let after_big = load_disk_docs_with_limits(root, &[], 32, MAX_INDEX_TOTAL_BYTES);
        assert_eq!(after_big.len(), 2, "{after_big:?}");
    }

    /// Review R192 BUG-6: batas TOTAL byte harus benar-benar menghentikan
    /// pembacaan (bukan hanya batas per file). Batas di-inject kecil supaya
    /// bisa diuji tanpa file 32 MiB.
    #[test]
    fn load_disk_docs_stops_at_total_byte_budget() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        for i in 0..4 {
            std::fs::write(root.join(format!("m{i}.sv")), "module m; endmodule\n").unwrap();
        }
        let per_file = std::fs::metadata(root.join("m0.sv")).unwrap().len();
        // Anggaran = 2 file + 1 byte → hanya 2 file yang dibaca.
        let budget = per_file * 2 + 1;
        let docs = load_disk_docs_with_limits(root, &[], 1024, budget);
        assert_eq!(
            docs.len(),
            2,
            "anggaran total byte tidak ditegakkan: {docs:?}"
        );
        // Anggaran pas 2 file (==) → tetap 2 file (cek batas `>` bukan `>=`).
        let docs = load_disk_docs_with_limits(root, &[], 1024, per_file * 2);
        assert_eq!(docs.len(), 2, "{docs:?}");
        // Anggaran 0 → tidak ada file.
        let docs = load_disk_docs_with_limits(root, &[], 1024, 0);
        assert!(docs.is_empty(), "{docs:?}");
    }

    #[test]
    fn collect_skip_paths_filters_non_file_uris() {
        let open = vec![
            ("file:///work/a.sv".to_string(), String::new()),
            ("untitled:Untitled-1".to_string(), String::new()),
            ("vscode-remote://host/work/b.sv".to_string(), String::new()),
        ];
        let skip = collect_skip_paths(&open);
        assert_eq!(skip, vec![PathBuf::from("/work/a.sv")]);
        assert!(collect_skip_paths(&[]).is_empty());
    }
}
