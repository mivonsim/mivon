//! Directed mutation (mutasi TERARAH setelah mutasi acak).
//!
//! Mutasi acak (mutator.rs) menyebar luas tapi dangkal. Directed mutation
//! menyerang pola yang SECARA HISTORIS memicu bug mivon:
//! - blok tidak ditutup (`begin` tanpa `end`) — parser recovery / hang
//! - keyword end duplikat — stack/dedent salah
//! - operator dibalik (`==`→`===`, `-`→`+`) — semantic fold
//! - literal lebar diubah — width resolution / concat
//! - paren tidak seimbang — parser hanabalance / EOF lokasi
//! - fill literal (`'0`→`'x`) — 4-state propagation
//! - sensitivity `@(*)` vs `@(posedge)` — scheduler
//! - add/remove delay — event scheduling
//!
//! Dipasang SETELAH mutasi acak (0-4x) di run_single: 30% kasus mendapat
//! 1 directed mutation — meninggikan probabilitas menemukan bug struktural.

use crate::Rng;

pub struct DirectedMutator<'r> {
    rng: &'r mut Rng,
}

impl<'r> DirectedMutator<'r> {
    pub fn new(rng: &'r mut Rng) -> Self {
        Self { rng }
    }

    /// Terapkan 1 mutation terarah. Operator aman char-boundary.
    pub fn mutate(&mut self, source: &str) -> String {
        if source.is_empty() {
            return String::new();
        }
        match self.rng.below(14) {
            0 => self.del_end_keyword(source),
            1 => self.dup_end_keyword(source),
            2 => self.swap_operator(source),
            3 => self.widen_literal(source),
            4 => self.unbalance_paren(source),
            5 => self.flip_fill_lit(source),
            6 => self.sensitivity_posedge(source),
            7 => self.add_delay(source),
            8 => self.dup_begin(source),
            9 => self.swap_edge(source),
            10 => self.swap_assign_kind(source),
            11 => self.remove_semi(source),
            // ── Corner LRM deterministik (area rawan hasil fuzz) ──
            12 => self.force_ascending_range(source),
            13 => self.force_decl_init_zero(source),
            _ => self.remove_semi(source),
        }
    }

    /// Hapus satu keyword `end`/`endmodule`/`endcase` (blok tak tertutup).
    fn del_end_keyword(&mut self, source: &str) -> String {
        let ends = ["endmodule", "endcase", "endclass", "endfunction", "endtask"];
        let target = self.rng.pick(&ends);
        let out = remove_first(source, target);
        if out == source {
            // Target tidak ada — fallback: hapus `end` (bentuk umum).
            remove_first(source, "end")
        } else {
            out
        }
    }

    /// Duplikasi keyword `endmodule`/`end` (extra close).
    fn dup_end_keyword(&mut self, source: &str) -> String {
        let ends = ["endmodule", "endcase", "end"];
        let target = self.rng.pick(&ends);
        insert_after_first(source, target, target)
    }

    /// Balik operator: `==`→`===`, `!==`→`!=`, `+`→`-`, `<=`(assign)→`==`.
    fn swap_operator(&mut self, source: &str) -> String {
        let pairs = [
            ("==", "==="),
            ("!==", "!="),
            (" <= ", " == "),
            (" + ", " - "),
            (" / ", " * "),
            (" & ", " | "),
        ];
        let (from, to) = self.rng.pick(&pairs);
        replace_all(source, from, to)
    }

    /// Ubah lebar literal masked: `8'hFF` → `16'hFF` (width context stress).
    fn widen_literal(&mut self, source: &str) -> String {
        // Cari pola `N'h` / `N'b` / `N'd` — ganti N menjadi lebar lain.
        let chars: Vec<char> = source.chars().collect();
        let mut idx = 0usize;
        while idx + 2 < chars.len() {
            // pola [digit]+' (b|d|h|o)
            if chars[idx].is_ascii_digit() && chars[idx + 1] == '\'' {
                let mut start = idx;
                while start > 0 && chars[start - 1].is_ascii_digit() {
                    start -= 1;
                }
                if start < idx {
                    let width: String = chars[start..idx].iter().collect();
                    if let Ok(w) = width.parse::<usize>() {
                        let new_w = if w < 64 { w + 4 } else { w / 2 };
                        // Lubang 4x: hanya lakukan pada 1/3 kesempatan.
                        if self.rng.chance(33) {
                            let mut out: String = chars[..start].iter().collect();
                            out.push_str(&new_w.to_string());
                            out.extend(&chars[idx..]);
                            return out;
                        }
                    }
                }
            }
            idx += 1;
        }
        source.to_string()
    }

    /// Hapus satu `)` — paren tak seimbang (parse unbalance / EOF loc).
    fn unbalance_paren(&mut self, source: &str) -> String {
        let chars: Vec<char> = source.chars().collect();
        if chars.is_empty() {
            return String::new();
        }
        // Cari `)` terakhir dalam blok non-sepele.
        let mut candidates: Vec<usize> = Vec::new();
        for (i, c) in chars.iter().enumerate() {
            if *c == ')' && i > 2 {
                candidates.push(i);
            }
        }
        if candidates.is_empty() {
            return source.to_string();
        }
        let at = candidates[self.rng.below(candidates.len())];
        let mut out: String = chars[..at].iter().collect();
        out.extend(&chars[at + 1..]);
        out
    }

    /// Flip fill literal: `'0`→`'x`, `'1`→`'z` (4-state propagation stress).
    fn flip_fill_lit(&mut self, source: &str) -> String {
        let pairs = [("'0", "'x"), ("'1", "'z"), ("'x", "'0"), ("'z", "'1")];
        let (from, to) = self.rng.pick(&pairs);
        replace_all(source, from, to)
    }

    /// Balik edge sensitivity: posedge↔negedge (scheduler edge-triggered;
    /// komplemen sensitivity_posedge yang hanya comb→ff).
    fn swap_edge(&mut self, source: &str) -> String {
        if self.rng.below(2) == 0 {
            if source.contains("posedge") {
                return source.replacen("posedge", "negedge", 1);
            }
            return source.to_string();
        }
        if source.contains("negedge") {
            return source.replacen("negedge", "posedge", 1);
        }
        source.to_string()
    }

    /// Tukar blocking↔non-blocking (`<=`↔`=` spasi, hindari `==`/`===`):
    /// semantik race NBA vs Active region (area sekuensial rapuh).
    fn swap_assign_kind(&mut self, source: &str) -> String {
        if self.rng.below(2) == 0 {
            replace_first(source, " <= ", " = ")
        } else {
            replace_first(source, " = ", " <= ")
        }
    }
    /// Tambah `@(posedge clk)` pada satu always_comb → salah sensitivity.
    fn sensitivity_posedge(&mut self, source: &str) -> String {
        if source.contains("always_comb") {
            source.replace("always_comb", "always_ff @(posedge clk)")
        } else {
            source.to_string()
        }
    }

    /// Sisipkan delay `#5` di awal baris statement (event scheduling).
    fn add_delay(&mut self, source: &str) -> String {
        let mut out = String::new();
        let mut done = false;
        for line in source.lines() {
            let t = line.trim_start();
            if !done && t.starts_with("a") && (t.starts_with("always") || t.starts_with("assign")) {
                // Tambahkan `#1` di baris assign berikutnya? — sederhana:
                // sisipkan `#1;` satu baris sebelum baris ini.
                out.push_str("#1;\n");
                done = true;
            }
            out.push_str(line);
            out.push('\n');
        }
        if done {
            out
        } else {
            source.to_string()
        }
    }

    /// Duplikasi `begin` (blok nested tanpa end → recovery).
    fn dup_begin(&mut self, source: &str) -> String {
        insert_after_first(source, "begin", "begin")
    }

    /// Hapus satu semicolon (statement merge).
    fn remove_semi(&mut self, source: &str) -> String {
        let chars: Vec<char> = source.chars().collect();
        let semis: Vec<usize> = chars
            .iter()
            .enumerate()
            .filter(|(i, c)| **c == ';' && *i > 5)
            .map(|(i, _)| i)
            .collect();
        if semis.is_empty() {
            return source.to_string();
        }
        let at = semis[self.rng.below(semis.len())];
        let mut out: String = chars[..at].iter().collect();
        out.extend(&chars[at + 1..]);
        out
    }

    /// Paksa SATU deklarasi range menurun `[N:0]` menjadi bentuk menaik
    /// zero-lsb `[0:N]` (atau `[7:0]`→`[0:7]`). Menaik/lsb≠0 adalah area rawan
    /// (posisi bit fisik vs indeks deklarasi, temuan fuzz `verify_bad_0021`).
    /// Tanpa range menurun → no-op.
    fn force_ascending_range(&mut self, source: &str) -> String {
        let chars: Vec<char> = source.chars().collect();
        let mut idx = 0usize;
        while idx < chars.len() {
            if chars[idx] == '[' {
                let mut j = idx + 1;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    j += 1;
                }
                if j > idx + 1 && j < chars.len() && chars[j] == ':' {
                    let mut k = j + 1;
                    while k < chars.len() && chars[k].is_ascii_digit() {
                        k += 1;
                    }
                    if k > j + 1 && k < chars.len() && chars[k] == ']' {
                        let hi: String = chars[idx + 1..j].iter().collect();
                        let lo: String = chars[j + 1..k].iter().collect();
                        // Hanya ubah bila menurun/zero-lsb (bukan sudah menaik).
                        if let (Ok(hi_n), Ok(lo_n)) = (hi.parse::<i64>(), lo.parse::<i64>()) {
                            if hi_n >= lo_n {
                                let rep = if lo_n == 0 {
                                    format!("[0:{hi}]")
                                } else {
                                    // lsb≠0 menaik: [lo:hi] (swap)
                                    format!("[{lo}:{hi}]")
                                };
                                let mut out: String = chars[..idx].iter().collect();
                                out.push_str(&rep);
                                out.extend(&chars[k + 1..]);
                                return out;
                            }
                        }
                    }
                }
            }
            idx += 1;
        }
        source.to_string()
    }

    /// Paksa initializer `= 0` pada SATU deklarasi variabel (deterministik
    /// vs `inject_decl_init` acak). Menyerang edge-sensitivity t=0 (fuzz
    /// `verify_bad_0586`). Lewati deklarasi yg sudah punya `=`.
    fn force_decl_init_zero(&mut self, source: &str) -> String {
        let decl_kw = ["logic", "reg", "bit", "integer", "int", "longint", "shortint", "byte"];
        let mut out = String::new();
        let mut done = false;
        for line in source.lines() {
            if !done {
                let t = line.trim_start();
                let kw = decl_kw.iter().find(|k| kw_at(t, k));
                if let Some(_k) = kw {
                    if !t.contains('=') {
                        if let Some(semi) = line.rfind(';') {
                            let mut l = String::from(&line[..semi]);
                            l.push_str(" = 0");
                            l.push(';');
                            out.push_str(&l);
                            out.push('\n');
                            done = true;
                            continue;
                        }
                    }
                }
            }
            out.push_str(line);
            out.push('\n');
        }
        if done {
            out
        } else {
            source.to_string()
        }
    }
}

// ─── Helpers char-boundary safe ───

/// Cek keyword `kw` di awal teks `t` dengan word-boundary setelahnya.
fn kw_at(t: &str, kw: &str) -> bool {
    if !t.starts_with(kw) {
        return false;
    }
    match t.chars().nth(kw.len()) {
        None => true,
        Some(c) => !(c.is_alphanumeric() || c == '_' || c == '$'),
    }
}

fn replace_first(source: &str, pat: &str, to: &str) -> String {
    match source.find(pat) {
        Some(pos) => {
            let mut out = String::with_capacity(source.len() + to.len());
            out.push_str(&source[..pos]);
            out.push_str(to);
            out.push_str(&source[pos + pat.len()..]);
            out
        }
        None => source.to_string(),
    }
}

fn remove_first(source: &str, pat: &str) -> String {
    match source.find(pat) {
        Some(pos) => {
            let mut out = String::with_capacity(source.len());
            out.push_str(&source[..pos]);
            out.push_str(&source[pos + pat.len()..]);
            out
        }
        None => source.to_string(),
    }
}

fn insert_after_first(source: &str, pat: &str, insert: &str) -> String {
    match source.find(pat) {
        Some(pos) => {
            let mut out = String::with_capacity(source.len() + insert.len());
            out.push_str(&source[..pos + pat.len()]);
            out.push_str(insert);
            out.push_str(&source[pos + pat.len()..]);
            out
        }
        None => source.to_string(),
    }
}

fn replace_all(source: &str, from: &str, to: &str) -> String {
    if from.is_empty() {
        return source.to_string();
    }
    let mut out = String::with_capacity(source.len() + to.len());
    let mut rest = source;
    while let Some(pos) = rest.find(from) {
        out.push_str(&rest[..pos]);
        out.push_str(to);
        rest = &rest[pos + from.len()..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `swap_edge` membalik posedge↔negedge; tanpa edge → no-op.
    /// `swap_assign_kind` menukar blocking↔non-blocking spasi-aman.
    #[test]
    fn edge_and_assign_swaps() {
        let src = "always @(posedge clk) q <= d;";
        let mut saw_negedge = false;
        let mut saw_noop = false;
        for seed in 0..20u64 {
            let mut rng = crate::Rng::new(seed);
            let mut m = DirectedMutator::new(&mut rng);
            let out = m.swap_edge(src);
            if out.contains("negedge") {
                saw_negedge = true;
            } else {
                assert!(out.contains("posedge"), "seed {seed}: edge hilang: {out}");
                saw_noop = true;
            }
        }
        assert!(saw_negedge, "swap posedge→negedge harus tercapai");
        let _ = saw_noop;
        // `==` tak tersentuh (bukan assignment); kedua arah tercapai.
        let mut saw_eq = false;
        let mut saw_le = false;
        for seed in 0..20u64 {
            let mut rng = crate::Rng::new(seed);
            let mut m = DirectedMutator::new(&mut rng);
            let out = m.swap_assign_kind("if (a == b) q <= d;");
            assert!(out.contains("=="), "seed {seed}: == rusak: {out}");
            if out.contains(" = ") {
                saw_eq = true;
            } else {
                saw_le = true;
            }
        }
        assert!(saw_eq && saw_le, "kedua arah harus tercapai");
    }

    /// `force_ascending_range` ubah `[N:0]`→`[0:N]` (menaik); sudah menaik →
    /// no-op. `force_decl_init_zero` sisipkan `= 0` pada deklarasi pertama.
    #[test]
    fn ascending_range_and_decl_init_zero() {
        let mut rng = crate::Rng::new(7);
        let mut m = DirectedMutator::new(&mut rng);
        let src = "module m;\n  logic [7:0] a;\n  logic b;\nendmodule\n";
        let out = m.force_ascending_range(src);
        assert!(out.contains("[0:7]"), "range jadi menaik: {out}");
        // Sudah menaik → tak berubah.
        assert_eq!(
            m.force_ascending_range("module m; logic [0:3] x; endmodule"),
            "module m; logic [0:3] x; endmodule"
        );
        let mut rng2 = crate::Rng::new(7);
        let mut m2 = DirectedMutator::new(&mut rng2);
        let out2 = m2.force_decl_init_zero(src);
        assert!(out2.contains("= 0"), "decl-init disisipkan: {out2}");
        assert!(!out2.contains("= ="), "tak dobel '=': {out2}");
    }
}
