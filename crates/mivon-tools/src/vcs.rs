//! ENT-39: integrasi revision control — daftar file sumber yang berubah
//! menurut VCS untuk dipakai sebagai file list inkremental.
//!
//! Dukungan: git (porcelain -z, untracked lengkap, subdirektori) dan svn.
//! Perforce DIDETEKSI (.p4config) tetapi konversi depot path -> lokal belum
//! didukung: pemanggilan mengembalikan error yang menjelaskan langkah manual
//! (lebih baik daripada mencetak path depot yang tidak bisa dibuka mivon).
//!
//! Output dirancang pipe-friendly: satu path per baris, komentar diawali '#'
//! (kompatibel file list .f).

use mivon_core::error::SimError;
use std::path::{Path, PathBuf};
use std::process::Command;

/// VCS yang dikenali.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcsKind {
    Git,
    Svn,
    Perforce,
    None,
}

impl VcsKind {
    pub fn name(self) -> &'static str {
        match self {
            VcsKind::Git => "git",
            VcsKind::Svn => "svn",
            VcsKind::Perforce => "perforce",
            VcsKind::None => "none",
        }
    }
}

/// Path absolut TANPA me-resolve symlink (beda dengan canonicalize).
///
/// Dua alasan: (1) root relatif seperti "." punya parent "" (= cwd) sehingga
/// walk-up berhenti dan VCS di atas cwd tidak terdeteksi — path harus
/// diabsolutkan dulu; (2) nama file yang dilaporkan VCS tidak boleh berubah
/// menjadi target symlink.
fn absolutize(p: &Path) -> PathBuf {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    // std::path::absolute membiarkan komponen ".." apa adanya; rapikan agar
    // root seperti ".." tidak menghasilkan path berawalan "../" di output.
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Deteksi VCS dengan menelusuri root lalu parent (maks 64 level) —
/// penanda .git/.svn/.p4config. Env P4CONFIG saja TIDAK dianggap bukti
/// (bisa diset global di mesin tanpa workspace Perforce).
pub fn detect(root: &Path) -> VcsKind {
    let start = absolutize(root);
    if let Some(kind) = detect_from(&start) {
        return kind;
    }
    // root bisa berupa symlink ke SUBdirektori repo (mis. ~/rtl ->
    // /proj/rtl): walk-up dari nama link tidak melihat marker, jadi ulangi
    // dari path fisiknya. canonicalize hanya untuk deteksi — display_path
    // tetap memakai absolutize agar nama symlink tidak berubah.
    if let Ok(real) = std::fs::canonicalize(&start) {
        if real != start {
            if let Some(kind) = detect_from(&real) {
                return kind;
            }
        }
    }
    VcsKind::None
}

/// Walk-up dari path yang sudah absolut (maks 64 level) mencari penanda
/// .git/.svn/.p4config.
fn detect_from(start: &Path) -> Option<VcsKind> {
    let mut cur: Option<&Path> = Some(start);
    for _ in 0..64 {
        let Some(dir) = cur else { break };
        if dir.join(".git").exists() {
            return Some(VcsKind::Git);
        }
        if dir.join(".svn").exists() {
            return Some(VcsKind::Svn);
        }
        if dir.join(".p4config").exists() {
            return Some(VcsKind::Perforce);
        }
        cur = dir.parent();
    }
    None
}

/// Apakah path berekstensi sumber/daftar file yang relevan.
pub fn is_source_path(path: &str) -> bool {
    // .f/.mivon sengaja TIDAK termasuk: output tool ini dipakai SEBAGAI
    // file list, jadi file list/proyek yang berubah tidak boleh ikut
    // tercetak (kalau ikut, redirect ke dalam root memuat dirinya sendiri
    // dan -f gagal membacanya sebagai sumber).
    const EXTS: [&str; 7] = ["sv", "svh", "v", "vh", "svi", "mv", "mvh"];
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Parse output git status --porcelain -z (NUL-separated).
///
/// - rename/copy: entri berisi "XY path-baru" lalu entri path-lama; path
///   lama dikonsumsi dan diabaikan;
/// - status berisi 'D' (deleted) dilewati — file list tidak boleh memuat
///   path yang sudah tidak ada;
/// - path non-ASCII/newline aman saat parsing karena -z tidak mengutip
///   (newline tetap tidak bisa direpresentasikan pada output satu path per
///   baris; lihat render_changed).
pub fn parse_git_status(out: &str) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    let mut it = out.split('\0');
    while let Some(entry) = it.next() {
        if entry.len() < 4 {
            continue;
        }
        let xy = &entry[..2];
        let renamed = xy.starts_with('R') || xy.starts_with('C');
        if renamed {
            // Entri berikutnya adalah path lama.
            let _ = it.next();
        }
        if xy.contains('D') {
            continue;
        }
        let path = &entry[3..];
        if path.is_empty() {
            continue;
        }
        v.push(path.to_string());
    }
    v.sort();
    v.dedup();
    v
}

/// Parse output svn status: blok 7 kolom status lalu path. Dilewati: tak
/// berversi ('?'), external ('X'), dihapus ('D') dan hilang ('!').
pub fn parse_svn_status(out: &str) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for line in out.lines() {
        let Some(status) = line.chars().next() else {
            continue;
        };
        if matches!(status, '?' | 'X' | ' ' | 'D' | '!') {
            continue;
        }
        // Kolom status svn = 7 karakter (item/props/lock/history/switched/
        // lock-token/tree-conflict); "A  +    path" punya flag '+' di kolom 4.
        let bytes = line.as_bytes();
        let mut idx = 0usize;
        while idx < bytes.len()
            && idx < 7
            && (bytes[idx] == b' ' || b"MADRCX!?~+LKS".contains(&bytes[idx]))
        {
            idx += 1;
        }
        let path = line[idx..].trim_start();
        if path.is_empty() {
            continue;
        }
        v.push(path.to_string());
    }
    v.sort();
    v.dedup();
    v
}

fn run_cmd(dir: Option<&Path>, program: &str, args: &[&str]) -> Result<String, SimError> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    if let Some(d) = dir {
        cmd.current_dir(d);
    }
    let out = cmd.output().map_err(|e| {
        SimError::runtime(format!(
            "gagal menjalankan {program}: {e} — pastikan VCS terpasang di PATH"
        ))
    })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(SimError::runtime(format!(
            "{program} gagal ({}): {}",
            out.status,
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Normalisasi separator path VCS. Di Unix backslash adalah karakter sah
/// pada nama file sehingga TIDAK boleh diubah (kalau diubah, file berubah
/// hilang diam-diam karena path hasil normalisasi tidak ada).
fn normalize_sep(p: &str) -> String {
    #[cfg(windows)]
    {
        p.replace('\\', "/")
    }
    #[cfg(not(windows))]
    {
        p.to_string()
    }
}

/// Ubah path relatif-repo menjadi relatif-root (root boleh subdirektori
/// repo). Path di luar root dilewati.
fn rebase_to_root(paths: Vec<String>, repo_root: &Path, root: &Path) -> Vec<String> {
    let repo = std::fs::canonicalize(repo_root).unwrap_or_else(|_| repo_root.to_path_buf());
    let here = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let rel = here.strip_prefix(&repo).ok().map(|p| {
        let s = normalize_sep(&p.to_string_lossy());
        s.trim_end_matches('/').to_string()
    });
    let rel = rel.filter(|r| !r.is_empty());
    let mut out = Vec::new();
    for p in paths {
        let p = normalize_sep(&p);
        match &rel {
            None => out.push(p),
            Some(prefix) => {
                // Cocokkan per komponen ("rtl/" bukan "rtl") agar
                // rtl2.sv tidak dipotong menjadi 2.sv.
                let with_slash = format!("{prefix}/");
                if let Some(rest) = p.strip_prefix(&with_slash) {
                    if !rest.is_empty() {
                        out.push(rest.to_string());
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Daftar file berubah menurut VCS (path relatif terhadap root).
/// VCS tak terdeteksi → (None, []).
pub fn changed_files(root: &Path) -> Result<(VcsKind, Vec<String>), SimError> {
    if !root.is_dir() {
        return Err(SimError::runtime(format!(
            "root bukan direktori: {}",
            root.display()
        )));
    }
    let kind = detect(root);
    let files = match kind {
        VcsKind::Git => {
            let root_str = root
                .to_str()
                .ok_or_else(|| SimError::runtime("path root bukan UTF-8"))?;
            let top = run_cmd(
                None,
                "git",
                &["-C", root_str, "rev-parse", "--show-toplevel"],
            )?;
            let repo_root = PathBuf::from(top.trim());
            let status = run_cmd(
                None,
                "git",
                &[
                    "-C",
                    root_str,
                    "status",
                    "--porcelain",
                    "-z",
                    "--untracked-files=all",
                ],
            )?;
            let files = rebase_to_root(parse_git_status(&status), &repo_root, root);
            files
                .into_iter()
                .filter(|p| root.join(p).exists())
                .collect()
        }
        VcsKind::Svn => {
            let status = run_cmd(Some(root), "svn", &["status", "--", "."])?;
            parse_svn_status(&status)
                .into_iter()
                .filter(|p| root.join(p).exists())
                .collect()
        }
        VcsKind::Perforce => {
            return Err(SimError::runtime(
                "perforce terdeteksi (.p4config) - konversi depot path belum didukung; jalankan perintah p4 opened lalu p4 where manual, atau pakai file list",
            ));
        }
        VcsKind::None => Vec::new(),
    };
    Ok((kind, files))
}

/// Path tampilan untuk file list: relatif terhadap `cwd` bila file berada
/// di bawahnya, selain itu absolut. File list `.f` me-resolve path relatif
/// ke direktori file `.f`, jadi path relatif-root akan salah bila output
/// di-redirect dari luar root (mis. `mcheck rtl --changed > changed.f` di
/// root repo).
fn display_path(root: &Path, rel: &str, cwd: &Path) -> String {
    // absolutize (bukan canonicalize): symlink harus tetap tercetak dengan
    // nama yang dilaporkan VCS, bukan path targetnya.
    let abs = absolutize(root).join(rel);
    match abs.strip_prefix(cwd) {
        Ok(p) if !p.as_os_str().is_empty() => normalize_sep(&p.to_string_lossy()),
        _ => abs.to_string_lossy().to_string(),
    }
}

/// Susun isi file list dari hasil `changed_files`.
///
/// Header memakai ASCII (tanpa em dash) dan satu path sumber per baris.
/// Path yang mengandung newline dilewati — tidak bisa direpresentasikan
/// pada format satu path per baris — dan jumlahnya dikembalikan agar
/// pemanggil bisa memberi peringatan.
pub fn render_changed(kind: VcsKind, files: &[String], root: &Path, cwd: &Path) -> (String, usize) {
    if kind == VcsKind::None {
        return (
            format!(
                "# tidak ada VCS terdeteksi di {} (.git/.svn/.p4config) - pakai file list manual\n",
                root.display()
            ),
            0,
        );
    }
    let sources: Vec<&String> = files.iter().filter(|f| is_source_path(f)).collect();
    let mut text = format!(
        "# VCS: {} - {} file berubah ({} file sumber)\n",
        kind.name(),
        files.len(),
        sources.len()
    );
    let mut skipped = 0usize;
    for f in sources {
        if f.contains('\n') || f.contains('\r') {
            skipped += 1;
            continue;
        }
        text.push_str(&display_path(root, f, cwd));
        text.push('\n');
    }
    (text, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) -> std::process::Output {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git harus tersedia untuk test integrasi");
        assert!(
            out.status.success(),
            "git {:?} gagal: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    #[test]
    fn detect_finds_markers_and_walks_up() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(detect(dir.path()), VcsKind::None);
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        assert_eq!(detect(dir.path()), VcsKind::Git);
        let sub = dir.path().join("rtl/sub");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(detect(&sub), VcsKind::Git, "deteksi harus naik ke parent");

        let svn = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(svn.path().join(".svn")).unwrap();
        assert_eq!(detect(svn.path()), VcsKind::Svn);

        let p4 = tempfile::tempdir().expect("tempdir");
        std::fs::write(p4.path().join(".p4config"), "P4CLIENT=x\n").unwrap();
        assert_eq!(detect(p4.path()), VcsKind::Perforce);
    }

    #[test]
    fn parse_git_status_handles_rename_delete_and_untracked() {
        // -z: "XY path" NUL, rename diikuti path lama sebagai entri terpisah.
        let out = " M rtl/top.sv\0?? rtl/new.sv\0R  rtl/moved c.sv\0old/a b.sv\0D  rtl/gone.sv\0";
        let files = parse_git_status(out);
        assert_eq!(
            files,
            vec![
                "rtl/moved c.sv".to_string(),
                "rtl/new.sv".to_string(),
                "rtl/top.sv".to_string(),
            ],
            "rename ambil path baru, deleted dilewati"
        );
    }

    #[test]
    fn parse_git_status_dedupes_and_ignores_short_entries() {
        assert!(parse_git_status("").is_empty());
        assert!(parse_git_status("M\0").is_empty());
        let files = parse_git_status(" M a.sv\0MM a.sv\0");
        assert_eq!(files, vec!["a.sv".to_string()]);
    }

    #[test]
    fn parse_svn_status_skips_unversioned_external_and_deleted() {
        let out = "M       rtl/top.sv\n?       rtl/scratch.sv\nX       vendor\nA  +    rtl/new.sv\nD       rtl/gone.sv\n!       rtl/missing.sv\n";
        let files = parse_svn_status(out);
        assert_eq!(
            files,
            vec!["rtl/new.sv".to_string(), "rtl/top.sv".to_string()]
        );
    }

    #[test]
    fn is_source_path_filters_by_extension() {
        assert!(is_source_path("rtl/top.sv"));
        assert!(is_source_path("rtl/top.SVH"));
        // .mivon/.f bukan "file sumber" (output dipakai sebagai file list).
        assert!(!is_source_path("proj.mivon"));
        assert!(!is_source_path("list.f"));
        assert!(is_source_path("hdl/core.mv"));
        assert!(is_source_path("hdl/core.mvh"));
        assert!(!is_source_path("README.md"));
        assert!(!is_source_path("rtl"));
    }

    #[test]
    fn parse_svn_status_skips_property_only_lines() {
        // Baris dengan kolom 1 spasi = hanya properti yang berubah (isi file
        // sama) -> tidak perlu di-check ulang.
        let out = " M      rtl/props.sv\nMM      rtl/both.sv\n";
        let files = parse_svn_status(out);
        assert_eq!(files, vec!["rtl/both.sv".to_string()], "{files:?}");
    }

    #[test]
    fn render_changed_without_vcs_header_is_ascii() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (text, skipped) = render_changed(VcsKind::None, &[], dir.path(), dir.path());
        assert_eq!(skipped, 0);
        assert!(text.is_ascii(), "header harus ASCII: {text}");
        assert!(text.contains("file list manual"), "{text}");
    }

    #[test]
    fn render_changed_prints_paths_relative_to_cwd() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        let root = repo.join("rtl");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("top.sv"), "module t; endmodule\n").unwrap();
        let cwd = absolutize(repo);
        let files = vec!["top.sv".to_string(), "baris\nbaru.sv".to_string()];
        let (text, skipped) = render_changed(VcsKind::Git, &files, &root, &cwd);
        assert_eq!(skipped, 1, "path ber-newline dilewati");
        assert!(
            text.contains("# VCS: git - 2 file berubah (2 file sumber)\n"),
            "{text}"
        );
        // Relatif cwd (bukan relatif root) supaya redirect ke root repo
        // menghasilkan path yang bisa di-resolve file list.
        assert!(text.contains("rtl/top.sv\n"), "{text}");
        assert!(!text.contains("baris"), "{text}");
    }

    #[test]
    fn render_changed_filters_non_source_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let files = vec![
            "rtl/top.sv".to_string(),
            "notes.md".to_string(),
            "list.f".to_string(),
            "proj.mivon".to_string(),
        ];
        let (text, _) = render_changed(VcsKind::Git, &files, dir.path(), dir.path());
        assert!(text.contains("4 file berubah (1 file sumber)"), "{text}");
        assert!(text.contains("top.sv"), "{text}");
        assert!(!text.contains("notes.md"), "{text}");
        assert!(!text.contains("list.f"), "{text}");
    }

    #[test]
    fn detect_relative_root_is_absolutized() {
        // Tanpa absolutisasi, parent(".") == "" (= cwd) sehingga walk-up
        // berhenti dan repo di atas cwd tidak terdeteksi.
        let cwd = std::env::current_dir().expect("cwd");
        assert_eq!(detect(Path::new(".")), detect(&cwd));
        let parent = cwd.parent().expect("cwd punya parent");
        assert_eq!(detect(Path::new("..")), detect(parent));
    }

    #[cfg(unix)]
    #[test]
    fn render_changed_keeps_symlink_name_not_target() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        let root = repo.join("rtl");
        let hdl = repo.join("hdl");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&hdl).unwrap();
        std::fs::write(hdl.join("impl.sv"), "module i; endmodule\n").unwrap();
        std::os::unix::fs::symlink(hdl.join("impl.sv"), root.join("link.sv")).unwrap();
        let cwd = absolutize(repo);
        let files = vec!["link.sv".to_string()];
        let (text, _) = render_changed(VcsKind::Git, &files, &root, &cwd);
        assert!(text.contains("rtl/link.sv"), "{text}");
        assert!(!text.contains("hdl/impl.sv"), "{text}");
    }

    #[cfg(unix)]
    #[test]
    fn detect_follows_symlinked_root_to_repo_subdir() {
        // Root symlink ke SUBdirektori repo: walk-up dari nama link tidak
        // melihat .git, jadi deteksi harus mencoba path fisiknya.
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join("rtl")).unwrap();
        std::fs::create_dir(repo.join(".git")).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(repo.join("rtl"), &link).unwrap();
        assert_eq!(detect(&link), VcsKind::Git);
    }

    #[cfg(unix)]
    #[test]
    fn changed_files_accepts_symlinked_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["config", "commit.gpgsign", "false"]);
        std::fs::create_dir(repo.join("rtl")).unwrap();
        std::fs::write(repo.join("rtl/top.sv"), "module t; endmodule\n").unwrap();
        git(&repo, &["add", "."]);
        git(
            &repo,
            &[
                "-c",
                "user.email=test@example.com",
                "-c",
                "user.name=test",
                "commit",
                "-qm",
                "init",
            ],
        );
        std::fs::write(repo.join("rtl/top.sv"), "module t2; endmodule\n").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(repo.join("rtl"), &link).unwrap();

        let (kind, files) = changed_files(&link).expect("root symlink");
        assert_eq!(kind, VcsKind::Git);
        assert_eq!(files, vec!["top.sv".to_string()], "{files:?}");
    }

    #[test]
    fn changed_files_without_vcs_is_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (kind, files) = changed_files(dir.path()).expect("tanpa VCS bukan error");
        assert_eq!(kind, VcsKind::None);
        assert!(files.is_empty());
    }

    #[test]
    fn changed_files_perforce_detected_but_unsupported() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(".p4config"), "P4CLIENT=x\n").unwrap();
        let err = changed_files(dir.path()).expect_err("p4 harus error jelas");
        let msg = format!("{err}");
        assert!(msg.contains("perforce"), "{msg}");
        assert!(msg.contains("p4 where"), "{msg}");
        assert!(!msg.contains("  "), "pesan tanpa celah spasi ganda: {msg}");
        assert!(msg.is_ascii(), "pesan harus ASCII: {msg}");
    }

    #[test]
    fn changed_files_rejects_missing_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = changed_files(&dir.path().join("tidak-ada")).expect_err("harus error");
        assert!(format!("{err}").contains("bukan direktori"), "{err}");
    }

    #[test]
    fn changed_files_git_subdir_prefix_is_component_wise() {
        // rtl2.sv TIDAK boleh terpotong menjadi "2.sv" saat root = rtl/.
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        git(root, &["init", "-q"]);
        git(root, &["config", "commit.gpgsign", "false"]);
        std::fs::create_dir(root.join("rtl")).unwrap();
        std::fs::write(root.join("rtl/top.sv"), "module t; endmodule\n").unwrap();
        std::fs::write(root.join("rtl2.sv"), "module x; endmodule\n").unwrap();
        git(root, &["add", "."]);
        git(
            root,
            &[
                "-c",
                "user.email=test@example.com",
                "-c",
                "user.name=test",
                "commit",
                "-qm",
                "init",
            ],
        );
        std::fs::write(root.join("rtl/top.sv"), "module t2; endmodule\n").unwrap();
        std::fs::write(root.join("rtl2.sv"), "module x2; endmodule\n").unwrap();

        let (_, sub) = changed_files(&root.join("rtl")).expect("subdir");
        assert_eq!(sub, vec!["top.sv".to_string()], "{sub:?}");
    }

    #[test]
    fn changed_files_git_reports_modified_untracked_and_subdir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        git(root, &["init", "-q"]);
        git(root, &["config", "commit.gpgsign", "false"]);
        std::fs::write(root.join("a.sv"), "module a; endmodule\n").unwrap();
        std::fs::write(root.join("notes.md"), "catatan\n").unwrap();
        git(root, &["add", "."]);
        git(
            root,
            &[
                "-c",
                "user.email=test@example.com",
                "-c",
                "user.name=test",
                "commit",
                "-qm",
                "init",
            ],
        );
        std::fs::write(root.join("a.sv"), "module a2; endmodule\n").unwrap();
        std::fs::create_dir(root.join("rtl")).unwrap();
        std::fs::write(root.join("rtl/new.sv"), "module n; endmodule\n").unwrap();

        let (kind, files) = changed_files(root).expect("changed_files");
        assert_eq!(kind, VcsKind::Git);
        assert!(files.contains(&"a.sv".to_string()), "{files:?}");
        assert!(
            files.contains(&"rtl/new.sv".to_string()),
            "untracked dalam direktori baru harus muncul: {files:?}"
        );
        assert!(!files.contains(&"notes.md".to_string()), "{files:?}");

        // Subdirektori: path relatif terhadap root subdir, file di luar root
        // tidak ikut.
        let (_, sub) = changed_files(&root.join("rtl")).expect("subdir");
        assert_eq!(sub, vec!["new.sv".to_string()], "{sub:?}");
    }
}
