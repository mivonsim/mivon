//! LSP-11 tahap 2: indeks workspace dari DISK (bukan hanya dokumen yang
//! sedang terbuka di editor).
//!
//! Tahap 1 (ROUND 99) hanya mem-flatten outline dokumen terbuka. Akibatnya
//! `workspace/symbol` tidak menemukan simbol di file yang belum pernah
//! dibuka. Modul ini menyediakan pemindaian direktori yang deterministik
//! (terurut, dibatasi) plus penggabungan dokumen (dokumen terbuka menang
//! atas versi disk — versi editor lebih baru).
//!
//! Caching BELUM ada (masih open di audit): pemindaian dilakukan per
//! permintaan `workspace/symbol` dengan batas keras jumlah file/ukuran
//! supaya tidak memblokir server pada repo besar.

use std::path::{Path, PathBuf};

/// Ekstensi sumber yang diindeks.
pub(crate) const SOURCE_EXTS: [&str; 5] = ["sv", "svh", "v", "vh", "svi"];

/// Direktori yang tidak pernah diindeks (build artifact / metadata VCS).
pub(crate) const SKIP_DIRS: [&str; 5] = [".git", "target", "node_modules", ".svn", "build"];

/// Batas jumlah file yang diindeks per permintaan.
pub(crate) const MAX_INDEX_FILES: usize = 2000;
/// Batas kedalaman traversal direktori.
pub(crate) const MAX_INDEX_DEPTH: usize = 16;
/// Batas ukuran satu file yang dibaca (byte).
pub(crate) const MAX_INDEX_BYTES: u64 = 1024 * 1024;

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

/// Kumpulkan file sumber di bawah `root` (rekursif, terurut, dibatasi).
///
/// Deterministik: hasil diurutkan leksikografis sehingga urutan simbol pada
/// respons `workspace/symbol` stabil antar pemanggilan (penting untuk test
/// dan untuk klien yang melakukan diff hasil).
pub(crate) fn collect_source_files(
    root: &Path,
    max_files: usize,
    max_depth: usize,
) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if !root.is_dir() {
        return out;
    }
    // DFS eksplisit (hindari rekursi dalam utk pohon dalam).
    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if out.len() >= max_files {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut children: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        children.sort();
        for path in children {
            if out.len() >= max_files {
                break;
            }
            if path.is_dir() {
                if depth + 1 > max_depth {
                    continue;
                }
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                if SKIP_DIRS.contains(&name) {
                    continue;
                }
                stack.push((path, depth + 1));
            } else if is_source_file(&path) {
                out.push(path);
            }
        }
    }
    out.sort();
    out.truncate(max_files);
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
}
