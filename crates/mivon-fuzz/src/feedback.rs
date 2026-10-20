//! feedback — umpan balik coverage (evolusioner lite).
//!
//! Tiap sim sukses menyumbang coverage keys; key BARU menandai input
//! menarik → disimpan content-addressed ke `.mivon-fuzz-state/corpus/`
//! dan ikut dimuat sebagai seed kampanye berikutnya (Corpus::load).

#![cfg(feature = "dev")]

use std::collections::BTreeSet;
use std::path::PathBuf;

/// Batas ukuran sumber menarik yang disimpan (anti-blowup).
pub const INTERESTING_MAX_BYTES: usize = 256 * 1024;

fn state_dir() -> PathBuf {
    PathBuf::from(".mivon-fuzz-state")
}

pub fn keys_path() -> PathBuf {
    state_dir().join("coverage-keys.txt")
}

/// Direktori seed hasil pertumbuhan (dimuat `Corpus::load`).
pub fn grow_corpus_dir() -> PathBuf {
    state_dir().join("corpus")
}

fn fingerprint(s: &str) -> u64 {
    let mut h: u64 = 0xCBF29CE484222325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001B3);
    }
    h
}

/// Ledger coverage keys persisten lintas kampanye.
pub struct CoverageLedger {
    path: PathBuf,
    seen: BTreeSet<String>,
    new_marks: usize,
}

impl CoverageLedger {
    pub fn load() -> Self {
        Self::load_from(keys_path())
    }

    pub fn load_from(path: PathBuf) -> Self {
        let seen = std::fs::read_to_string(&path)
            .map(|s| {
                s.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        Self {
            path,
            seen,
            new_marks: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }

    pub fn new_marks(&self) -> usize {
        self.new_marks
    }

    /// Tandai seluruh keys; return key yang BARU (belum pernah terlihat).
    pub fn mark_new(&mut self, keys: &[String]) -> Vec<String> {
        let mut fresh = Vec::new();
        for k in keys {
            if self.seen.insert(k.clone()) {
                self.new_marks += 1;
                fresh.push(k.clone());
            }
        }
        fresh
    }

    pub fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let mut body = String::with_capacity(self.seen.len() * 32);
        for k in &self.seen {
            body.push_str(k);
            body.push('\n');
        }
        let tmp = self.path.with_extension("txt.tmp");
        std::fs::write(&tmp, body.as_bytes())?;
        std::fs::rename(&tmp, &self.path)
    }
}

/// Ledger global proses — dimuat SEKALI (bukan per kasus; file bisa besar).
static LEDGER: std::sync::OnceLock<std::sync::Mutex<CoverageLedger>> = std::sync::OnceLock::new();

/// Catat keys satu kasus; return jumlah key BARU. Simpan tiap ~64 key baru.
pub fn record_keys(keys: &[String]) -> usize {
    if keys.is_empty() {
        return 0;
    }
    let m = LEDGER.get_or_init(|| std::sync::Mutex::new(CoverageLedger::load()));
    let Ok(mut l) = m.lock() else {
        return 0;
    };
    let before = l.new_marks();
    let fresh = l.mark_new(keys).len();
    if fresh > 0 && l.new_marks() / 64 != before / 64 {
        let _ = l.save();
    }
    fresh
}

/// Paksa simpan ledger (dipanggil akhir kampanye).
pub fn flush() {
    if let Some(m) = LEDGER.get() {
        if let Ok(l) = m.lock() {
            let _ = l.save();
        }
    }
}

/// Simpan input menarik sbg seed (content-addressed; dobel = no-op).
pub fn save_interesting(source: &str) -> Option<PathBuf> {
    if source.len() > INTERESTING_MAX_BYTES {
        return None;
    }
    let dir = grow_corpus_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("g_{:016x}.sv", fingerprint(source)));
    if path.exists() {
        return Some(path);
    }
    std::fs::write(&path, source).ok()?;
    Some(path)
}

// ── Coverage keys per-target (feed balik corpus evolusi) ─────────────────────
//
// Dulu HANYA jalur sim menyumbang keys (oracle.rs) — corpus tumbuh didorong
// satu sinyal; lexer/parser/elab/fmt semua buta. Sekarang: hasil compile/
// lex in-process juga mencongkir keys → input membuka set IR/token-kind baru
// dianggap menarik → disimpan sbg seed kampanye berikutnya.

/// Cap token per dump — anti besar tanpa batas (debug IR bisa 100k token).
pub const COV_TOKEN_CAP: usize = 400;

/// Token-konstruksi dari output `{:?}`: ident alnum awal-HURUF-BESAR yang
/// menandai nama variant/konstruk (`BlockingAssign`, `IrExpr::Concat`, ...).
/// Murni + deterministik — diuji.
pub fn debug_cov_tokens(debug_str: &str) -> Vec<String> {
    let mut set = std::collections::BTreeSet::new();
    let bytes = debug_str.as_bytes();
    let mut i = 0;
    while i < bytes.len() && set.len() < COV_TOKEN_CAP {
        let b = bytes[i];
        if b.is_ascii_alphabetic() {
            // Ident dimulai huruf besar? Scan sampai non-ident.
            let start = i;
            let mut j = i;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            if bytes[start].is_ascii_uppercase() && j - start >= 4 {
                set.insert(debug_str[start..j].to_string());
            }
            i = j;
        } else {
            i += 1;
        }
    }
    set.into_iter().collect()
}

/// Keys coverage dari IrDesign hasil compile in-process — menutupi kind
/// proses/sinyal/hierarki/class yang dicapai input (top + semua modul).
/// Token ditarik dari union seluruh dump (BTreeSet terurut → deterministik,
/// order iterasi HashMap tak berefek; cap batasi di scanner).
pub fn ir_cov_keys(ir: &mivon_ir::IrDesign) -> Vec<String> {
    use std::fmt::Write as _;
    let mut buf = String::new();
    let mut modules_seen = 0usize;
    buf.push_str(&debug_str_module(&ir.top));
    modules_seen += 1;
    for (_, m) in ir.modules.iter() {
        buf.push_str(&debug_str_module(m));
        modules_seen += 1;
    }
    let _ = write!(buf, "Modules{:?}", modules_seen); // key coarse jumlah modul
    for cls in ir.classes.keys() {
        buf.push_str(&format!("Class{:?}", cls));
    }
    let mut keys = Vec::new();
    for tok in debug_cov_tokens(&buf) {
        keys.push(format!("ir|k|{tok}"));
    }
    keys
}

/// Dump struktur satu module untuk token-kind coverage. Debug penuh IrModule
/// (name/signals/processes/instances) — tidak menarik source_lines (itu di
/// IrDesign, bukan module).
fn debug_str_module(m: &mivon_ir::IrModule) -> String {
    format!("{m:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_marks_new_keys_once() {
        let dir = std::env::temp_dir().join(format!("fzcov_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keys.txt");
        let mut l = CoverageLedger::load_from(path.clone());
        assert_eq!(l.mark_new(&["a".into(), "b".into()]).len(), 2);
        let fresh2 = l.mark_new(&["b".into(), "c".into()]);
        assert_eq!(fresh2, vec!["c".to_string()], "hanya key baru");
        l.save().unwrap();
        let mut l2 = CoverageLedger::load_from(path);
        assert_eq!(l2.len(), 3);
        assert!(
            l2.mark_new(&["a".into()]).is_empty(),
            "persisten antar load"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn debug_cov_tokens_upper_variants_only() {
        let toks = debug_cov_tokens("BlockingAssign { hmac: IrExpr::Concat(..), Other(x) lowercase: keep_out }");
        assert!(toks.iter().any(|t| t == "BlockingAssign"), "{toks:?}");
        assert!(toks.iter().any(|t| t == "Concat"), "{toks:?}");
        assert!(toks.iter().any(|t| t == "IrExpr"), "{toks:?}");
        assert!(!toks.iter().any(|t| t == "hmac"), "huruf kecil = bukan token: {toks:?}");
        assert!(!toks.iter().any(|t| t == "keep"), "huruf kecil lain = bukan token: {toks:?}");
    }

    #[test]
    fn debug_cov_tokens_cap_keeps_sorted_prefix() {
        let mut s = String::new();
        for i in 0..1000 {
            s.push_str(&format!("Big{:04} tiny{} ", i, i));
        }
        let toks = debug_cov_tokens(&s);
        assert_eq!(toks.len(), COV_TOKEN_CAP, "cap ditegakkan");
        // Terurut + deterministik: run kedua sama.
        assert_eq!(toks, debug_cov_tokens(&s));
    }

    /// REGRESI corpus-evolution gap: IR design valid wajib menyumbang keys —
    /// dulu jalur compile TIDAK mem-feed `record_keys` (hanya sim), corpus
    /// tumbuh asimetris.
    #[test]
    fn ir_cov_keys_nonempty_on_small_design() {
        let ir = mivon_api::compile_str_quiet(
            "module kp(input logic a, output logic b);\n  assign b = ~a;\nendmodule\n",
        )
        .expect("compile");
        let keys = ir_cov_keys(&ir);
        assert!(!keys.is_empty(), "harus ada keys");
        assert!(keys.iter().all(|k| k.starts_with("ir|k|")), "{:?}", &keys[..keys.len().min(8)]);
        // Deterministik: two calls sama.
        let ir2 = mivon_api::compile_str_quiet(
            "module kp(input logic a, output logic b);\n  assign b = ~a;\nendmodule\n",
        )
        .expect("compile");
        assert_eq!(keys, ir_cov_keys(&ir2), "keys deterministik antar run");
    }
}
