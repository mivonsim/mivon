use crate::simulator::types::CoverageType;
use crate::simulator::util::*;
/// Coverage tracking and reporting for SimulationEngine.
/// Manages covergroup sampling, coverage reporting, and UCIS XML export.
use mivon_core::diagnostics::DiagCode;
use mivon_core::error::SimError;
use mivon_core::Symbol;
use mivon_ir::*;
use std::collections::{HashMap, HashSet};

use super::SeqCovStats;
use super::SimulationEngine;

/// Batas jumlah default bin per coverpoint/cross. Nilai unik di luar cap tetap
/// dihitung sebagai sample (hits/total), tapi tidak membuat bin baru — mencegah
/// pertumbuhan tak terbatas untuk coverpoint lebar dengan nilai acak.
pub(crate) const MAX_DEFAULT_BINS: usize = 4096;

/// PERF-16: Pre-compute a constant-bin lookup map for fast O(1) bin matching.
/// Returns (const_map, has_non_const) — const_map maps value → (bin_name, bin_type),
/// has_non_const indicates if any bin has non-constant ranges (needs linear scan).
fn build_const_bin_map(
    bins: &[IrBin],
) -> (HashMap<u64, (Symbol, mivon_ast::types::BinType)>, bool) {
    let mut const_map: HashMap<u64, (Symbol, mivon_ast::types::BinType)> = HashMap::new();
    let mut has_non_const = false;

    for bin in bins {
        // Skip transition bins — they need prev value, can't be pre-computed.
        if !bin.transitions.is_empty() {
            has_non_const = true;
            continue;
        }
        // Check if ALL ranges are constant (IrExpr::Const).
        let all_const = bin.ranges.iter().all(|rng| {
            matches!(rng.low, IrExpr::Const(_))
                && rng
                    .high
                    .as_ref()
                    .is_none_or(|h| matches!(h, IrExpr::Const(_)))
        });
        if all_const && !bin.ranges.is_empty() {
            for rng in &bin.ranges {
                if let IrExpr::Const(low_lv) = &rng.low {
                    let low = low_lv.to_u64();
                    let high = match &rng.high {
                        Some(IrExpr::Const(h)) => h.to_u64(),
                        None => low,
                        _ => {
                            has_non_const = true;
                            continue;
                        }
                    };
                    for v in low..=high {
                        const_map.insert(v, (bin.name, bin.bin_type.clone()));
                    }
                }
            }
        } else {
            has_non_const = true;
        }
    }
    (const_map, has_non_const)
}

/// Check if a value matches a wildcard bin pattern (supports ? and * wildcards).
#[allow(dead_code)]
fn wildcard_match(value: u64, pattern: &str) -> bool {
    let val_str = format!("{}", value);
    let p = pattern.trim();

    let pat_chars: Vec<char> = p.chars().collect();
    let val_chars: Vec<char> = val_str.chars().collect();

    let vlen = val_chars.len();
    let plen = pat_chars.len();
    let mut dp = vec![vec![false; plen + 1]; vlen + 1];
    dp[0][0] = true;

    for j in 1..=plen {
        if pat_chars[j - 1] == '*' {
            dp[0][j] = dp[0][j - 1];
        }
    }

    for i in 1..=vlen {
        for j in 1..=plen {
            if pat_chars[j - 1] == '*' {
                dp[i][j] = dp[i - 1][j] || dp[i][j - 1];
            } else if pat_chars[j - 1] == '?' || pat_chars[j - 1] == val_chars[i - 1] {
                dp[i][j] = dp[i - 1][j - 1];
            }
        }
    }
    dp[vlen][plen]
}

impl SimulationEngine {
    // ─── Coverage Control ($coverage_control, SIM-30) ───────────────

    /// `$load_coverage_db(path)` — muat database coverage biner (format MCDB,
    /// ditulis `mcov`/CoverageDatabase::save_to_file) dan MERGE ke counter
    /// live engine (line/branch/toggle/FSM/covergroup dijumlahkan).
    /// Return jumlah item coverage yang di-merge. Error I/O / format
    /// dikembalikan sebagai Err (pemanggil: warning IoError, sim lanjut).
    pub(crate) fn load_coverage_db_merge(&mut self, path: &str) -> Result<usize, String> {
        let db = crate::simulator::coverage_db::CoverageDatabase::load_from_path(path)?;
        let mut merged = 0usize;
        // Line: jumlahkan hits per key (key SAMA dengan cover_line).
        for (key, hits) in &db.line_hits {
            *self.cover_line.entry(*key).or_insert(0) += hits;
            merged += 1;
        }
        // Branch: jumlahkan per (key, label).
        for (key, branches) in &db.branch_data {
            let entry = self.cover_branches.entry(*key).or_default();
            for (label, count) in branches {
                *entry.entry(*label).or_insert(0) += count;
                merged += 1;
            }
        }
        // Toggle: union transisi (engine simpan set — count db dibuang,
        // konsisten dgn record_toggle yg juga hanya mencatat keberadaan).
        for (sig_id, tentry) in &db.toggle_data {
            let entry = self.cover_toggle.entry(*sig_id).or_default();
            for t in tentry.transitions.keys() {
                entry.insert(*t);
                merged += 1;
            }
        }
        // FSM: union state (engine simpan set).
        for (sig_id, states) in &db.fsm_data {
            let entry = self.cover_fsm.entry(*sig_id).or_default();
            for s in states.keys() {
                entry.insert(*s);
                merged += 1;
            }
        }
        // Covergroup: agregat ke key `cg.cp` (pola merge_from_engine —
        // sum_key membaca key agregat ATAU per-instance `cg.i<id>.cp`).
        for (cg_name, cg_entry) in &db.covergroups {
            for cp in &cg_entry.coverpoints {
                let key = Symbol::intern(&format!("{}.{}", cg_name, cp.name));
                *self.cover_total.entry(key).or_insert(0) += cp.total;
                *self.cover_hits.entry(key).or_insert(0) += cp.hits;
                let bins = self.cover_bins.entry(key).or_default();
                for (bk, bv) in &cp.bins {
                    *bins.entry(*bk).or_insert(0) += bv;
                    merged += 1;
                }
                merged += 1;
            }
            for cross in &cg_entry.crosses {
                let key = Symbol::intern(&format!("{}.{}", cg_name, cross.name));
                *self.cover_total.entry(key).or_insert(0) += cross.total;
                *self.cover_hits.entry(key).or_insert(0) += cross.hits;
                let bins = self.cover_bins.entry(key).or_default();
                for (bk, bv) in &cross.bins {
                    *bins.entry(*bk).or_insert(0) += bv;
                    merged += 1;
                }
                merged += 1;
            }
        }
        Ok(merged)
    }

    /// Terapkan bitmask `$coverage_control(control)` (IEEE 1800-2017 §20.13.2).
    /// Bit-0=line, bit-1=toggle, bit-2=branch, bit-3=FSM, bit-4=covergroup.
    /// Nilai 0 → semua nonaktif; nilai ~0 (semua bit set) → semua aktif.
    pub(crate) fn apply_coverage_control(&mut self, bitmask: u64) {
        self.coverage_options
            .insert("control".to_string(), bitmask.to_string());
        self.coverage_enabled_types.clear();
        const ALL_TYPES: u64 = 0x1F; // 5 tipe coverage yang didukung
        if bitmask == 0 {
            self.coverage_enabled = false;
        } else if bitmask == u64::MAX || (bitmask & ALL_TYPES) == ALL_TYPES {
            // Semua tipe aktif: set kosong berarti semua enabled (sesuai komentar field)
            self.coverage_enabled = true;
        } else {
            self.coverage_enabled = true;
            if bitmask & 0x1 != 0 {
                self.coverage_enabled_types.insert(CoverageType::Line);
            }
            if bitmask & 0x2 != 0 {
                self.coverage_enabled_types.insert(CoverageType::Toggle);
            }
            if bitmask & 0x4 != 0 {
                self.coverage_enabled_types.insert(CoverageType::Branch);
            }
            if bitmask & 0x8 != 0 {
                self.coverage_enabled_types.insert(CoverageType::Fsm);
            }
            if bitmask & 0x10 != 0 {
                self.coverage_enabled_types.insert(CoverageType::Covergroup);
            }
        }
    }

    /// Apakah tipe coverage tertentu aktif saat ini?
    /// Set kosong (coverage_enabled_types) berarti semua tipe aktif.
    fn coverage_type_enabled(&self, t: CoverageType) -> bool {
        self.coverage_enabled
            && (self.coverage_enabled_types.is_empty() || self.coverage_enabled_types.contains(&t))
    }

    // ─── Line Coverage ─────────────────────────────────────────────

    /// Apakah baris (1-based, koordinat output preprocessed) berada dalam
    /// region `` `coverage_off ``/`` `coverage_on `` sehingga line coverage-nya
    /// harus di-exclude (SIM-29).
    pub fn is_line_excluded(&self, line: usize) -> bool {
        self.coverage_exclusions
            .iter()
            .any(|(s, e)| line >= *s && line <= *e)
    }

    /// VERIF-27: catat hasil evaluasi assertion (pass/fail) ke assertion_stats
    /// — keyed by (line, col). Dipanggil dari handler Assert/Expect/Assume
    /// (block.rs, 2 jalur) dan concurrent sequence completion (sequence.rs).
    pub(crate) fn record_assertion(&mut self, line: usize, col: usize, ok: bool) {
        if line == 0 {
            return;
        }
        let e = self.assertion_stats.entry((line, col)).or_insert((0, 0));
        if ok {
            e.0 += 1;
        } else {
            e.1 += 1;
        }
    }

    /// VERIF-32: catat bahwa sebuah concurrent assertion sequence attempt
    /// DIMULAI pada posisi (line, col). Dipanggil dari 2 situs push
    /// SequenceAttempt di block.rs.
    pub(crate) fn record_sequence_attempt(&mut self, line: usize, col: usize) {
        if line == 0 {
            return;
        }
        self.sequence_coverage
            .entry((line, col))
            .or_default()
            .attempts += 1;
    }

    /// VERIF-32: catat hasil completion sequence attempt — matched (property
    /// terpenuhi) atau failed (timeout max_cycles tanpa match).
    pub(crate) fn record_sequence_result(&mut self, line: usize, col: usize, ok: bool) {
        if line == 0 {
            return;
        }
        let e = self.sequence_coverage.entry((line, col)).or_default();
        if ok {
            e.matched += 1;
        } else {
            e.failed += 1;
        }
    }

    /// Record that a source line was executed.
    pub(crate) fn record_line_hit(&mut self, stmt: &IrStmt, process_name: &str) {
        if !self.coverage_type_enabled(CoverageType::Line) {
            return;
        }
        let key = Symbol::intern(&format!(
            "{}.{:?}",
            process_name,
            std::mem::discriminant(stmt)
        ));
        // SIM-29: statement pada baris dalam `` `coverage_off ``/`` `coverage_on ``
        // TIDAK dihitung line coverage-nya. Baris statement di-lookup dari
        // side-table `stmt_lines` (key SAMA dengan cover_line) — di-populate
        // elaborator dari AST (expr_location). Statement tanpa baris
        // (line 0 / tidak dicatat) tetap dihitung.
        if let Some(&line) = self.stmt_lines.get(&key) {
            if self.is_line_excluded(line) {
                return;
            }
        }
        *self.cover_line.entry(key).or_insert(0) += 1;
    }

    /// Create a unique branch key for a conditional statement.
    #[allow(dead_code)]
    fn branch_key(process_name: &str, branch_type: &str, idx: usize) -> Symbol {
        Symbol::intern(&format!("{}.{}#{}", process_name, branch_type, idx))
    }

    /// Record a branch being taken (branch coverage).
    pub(crate) fn record_branch_hit(&mut self, branch_key: Symbol, label: &str) {
        if !self.coverage_type_enabled(CoverageType::Branch) {
            return;
        }
        let branches = self.cover_branches.entry(branch_key).or_default();
        *branches.entry(Symbol::intern(label)).or_insert(0) += 1;
    }

    // ─── Toggle Coverage ───────────────────────────────────────────

    /// Record a signal toggle (transition between logic values).
    pub(crate) fn record_toggle(&mut self, sig_id: usize, old_val: &LogicVec, new_val: &LogicVec) {
        if !self.coverage_type_enabled(CoverageType::Toggle) {
            return;
        }
        let toggles = self.cover_toggle.entry(sig_id).or_default();
        for i in 0..old_val.width.min(new_val.width).min(64) {
            let old_bit = old_val.bits.get(i).copied().unwrap_or(LogicVal::X);
            let new_bit = new_val.bits.get(i).copied().unwrap_or(LogicVal::X);
            if old_bit != new_bit {
                toggles.insert((old_bit, new_bit));
            }
        }
    }

    // ─── FSM Coverage ──────────────────────────────────────────────

    /// Record a signal value for FSM state analysis.
    pub(crate) fn record_fsm_value(&mut self, sig_id: usize, val: &LogicVec) {
        if !self.coverage_type_enabled(CoverageType::Fsm) {
            return;
        }
        let uval = val.to_u64();
        self.cover_fsm.entry(sig_id).or_default().insert(uval);
    }

    // ─── Reporting ─────────────────────────────────────────────────

    /// Print line coverage report.
    fn report_line_coverage(&self) {
        if self.cover_line.is_empty() {
            return;
        }
        eprintln!("\n=== Line Coverage ===");
        let mut sorted: Vec<(&str, u64)> = self
            .cover_line
            .iter()
            .map(|(k, v)| (k.as_str(), *v))
            .collect();
        sorted.sort_by_key(|a| std::cmp::Reverse(a.1));
        for (key, hits) in sorted.iter().take(20) {
            eprintln!("  {}: {} hits", key, hits);
        }
        eprintln!("  ({} total line items)", self.cover_line.len());
    }

    /// Print toggle coverage report.
    fn report_toggle_coverage(&self) {
        if self.cover_toggle.is_empty() {
            return;
        }
        eprintln!("\n=== Toggle Coverage ===");
        for (sig_id, toggles) in &self.cover_toggle {
            let sig_name = self
                .design
                .top
                .signals
                .get(*sig_id)
                .map(|s| s.name.as_str())
                .unwrap_or("<unknown>");
            eprintln!("  {}: {} transitions", sig_name, toggles.len());
            for (from, to) in toggles.iter() {
                eprintln!("    {:?}→{:?}", from, to);
            }
        }
        let total_signals = self.design.top.signals.len();
        eprintln!(
            "  {} signals with toggles / {} total",
            self.cover_toggle.len(),
            total_signals
        );
    }

    /// Print branch coverage report.
    fn report_branch_coverage(&self) {
        if self.cover_branches.is_empty() {
            return;
        }
        eprintln!("\n=== Branch Coverage ===");
        for (key, branches) in &self.cover_branches {
            eprintln!("  {}:", key.as_str());
            let total: u64 = branches.values().sum();
            for (label, count) in branches {
                eprintln!(
                    "    {}: {} hits ({:.1}%)",
                    label.as_str(),
                    count,
                    if total > 0 {
                        *count as f64 / total as f64 * 100.0
                    } else {
                        0.0
                    }
                );
            }
        }
    }

    /// Print FSM coverage report.
    fn report_fsm_coverage(&self) {
        if self.cover_fsm.is_empty() {
            return;
        }
        eprintln!("\n=== FSM Coverage ===");
        for (sig_id, states) in &self.cover_fsm {
            let sig_name = self
                .design
                .top
                .signals
                .get(*sig_id)
                .map(|s| s.name.as_str())
                .unwrap_or("<unknown>");
            let mut sorted_states: Vec<u64> = states.iter().copied().collect();
            sorted_states.sort();
            eprintln!(
                "  {}: {} states visited: {:?}",
                sig_name,
                states.len(),
                sorted_states
            );
        }
    }

    /// VERIF-27: laporan assertion coverage metrics — pass/fail per assertion
    /// (keyed by line:col). Assertion tanpa pass/fail terlihat jelas.
    fn report_assertion_coverage(&self) {
        if self.assertion_stats.is_empty() {
            return;
        }
        eprintln!("\n=== Assertion Coverage ===");
        let mut entries: Vec<((usize, usize), (u64, u64))> =
            self.assertion_stats.iter().map(|(k, v)| (*k, *v)).collect();
        entries.sort_by_key(|(k, _)| *k);
        for ((line, col), (p, f)) in entries {
            eprintln!("  line {}:{} — {} pass / {} fail", line, col, p, f);
        }
        let total_pass: u64 = self.assertion_stats.values().map(|(p, _)| *p).sum();
        let total_fail: u64 = self.assertion_stats.values().map(|(_, f)| *f).sum();
        eprintln!(
            "  ({} assertions evaluated: {} pass, {} fail)",
            total_pass + total_fail,
            total_pass,
            total_fail
        );
    }

    /// VERIF-32: laporan sequence coverage — attempts/matched/failed per
    /// concurrent assertion sequence (keyed by line:col) + coverage hole
    /// (sequence yang tidak pernah match).
    fn report_sequence_coverage(&self) {
        if self.sequence_coverage.is_empty() {
            return;
        }
        eprintln!("\n=== Sequence Coverage ===");
        let mut entries: Vec<((usize, usize), SeqCovStats)> = self
            .sequence_coverage
            .iter()
            .map(|(k, v)| (*k, *v))
            .collect();
        entries.sort_by_key(|(k, _)| *k);
        for ((line, col), s) in entries {
            let status = if s.matched == 0 {
                " [HOLE: tidak pernah match]"
            } else {
                ""
            };
            eprintln!(
                "  line {}:{} — {} attempts / {} matched / {} failed{}",
                line, col, s.attempts, s.matched, s.failed, status
            );
        }
        let total_attempts: u64 = self.sequence_coverage.values().map(|s| s.attempts).sum();
        let total_matched: u64 = self.sequence_coverage.values().map(|s| s.matched).sum();
        let pct = if total_attempts > 0 {
            (total_matched as f64 / total_attempts as f64) * 100.0
        } else {
            0.0
        };
        eprintln!(
            "  ({} sequences tracked: {} attempts, {} matched, {:.1}% match rate)",
            self.sequence_coverage.len(),
            total_attempts,
            total_matched,
            pct
        );
    }

    /// VERIF-26: coverage gap analysis — daftar item coverage yang TIDAK
    /// pernah kena: covergroup/coverpoint/cross tidak pernah di-sample, bin
    /// eksplisit yang tidak pernah hit, dan sinyal yang tidak pernah toggle.
    pub fn coverage_gaps(&self) -> Vec<String> {
        let mut gaps: Vec<String> = Vec::new();
        for cg in &self.design.covergroups {
            for cp in &cg.coverpoints {
                let key = format!("{}.{}", cg.name, cp.name);
                let (total, _) = self.cg_item_stats(cg, cp.name.as_str());
                if total == 0 {
                    gaps.push(format!("coverpoint '{}' tidak pernah di-sample", key));
                }
                // Bin eksplisit normal yang tidak pernah kena (agregat semua
                // instance — per_instance keys ikut dijumlahkan).
                let prefix = format!("{}.", cg.name.as_str());
                for bin in &cp.bins {
                    if bin.bin_type != mivon_ast::types::BinType::Normal {
                        continue;
                    }
                    let bin_key = format!("{}={}", cp.name, bin.name);
                    let mut hit = 0u64;
                    for (k, bmap) in &self.cover_bins {
                        let s = k.as_str();
                        if s.starts_with(&prefix) && s.ends_with(&format!(".{}", cp.name)) {
                            if let Some(h) = bmap.get(&Symbol::intern(&bin_key)) {
                                hit += h;
                            }
                        }
                    }
                    if hit == 0 {
                        gaps.push(format!(
                            "coverpoint '{}' bin '{}' tidak pernah kena (0 hit)",
                            key, bin.name
                        ));
                    }
                }
            }
            for cross in &cg.crosses {
                let key = format!("{}.{}", cg.name, cross.name);
                let (total, _) = self.cg_item_stats(cg, cross.name.as_str());
                if total == 0 {
                    gaps.push(format!("cross '{}' tidak pernah di-sample", key));
                }
            }
        }
        // VERIF-32: sequence coverage hole — concurrent assertion sequence
        // yang tidak pernah match (semua attempt timeout).
        for ((line, col), s) in &self.sequence_coverage {
            if s.matched == 0 {
                gaps.push(format!(
                    "sequence di line {}:{} tidak pernah match ({} attempts)",
                    line, col, s.attempts
                ));
            }
        }
        // Sinyal yang tidak pernah toggle (hanya saat toggle coverage aktif).
        if self.coverage_type_enabled(CoverageType::Toggle) && !self.cover_toggle.is_empty() {
            let mut never_toggled = 0usize;
            for (idx, s) in self.design.top.signals.iter().enumerate() {
                if s.width == 0 {
                    continue;
                }
                if !self.cover_toggle.contains_key(&idx) {
                    gaps.push(format!("signal '{}' tidak pernah toggle", s.name));
                    never_toggled += 1;
                    // Batasi noise utk design besar — gap utama sudah terlihat.
                    if never_toggled >= 20 {
                        gaps.push(format!(
                            "... ({} sinyal tidak pernah toggle, dibatasi)",
                            never_toggled
                        ));
                        break;
                    }
                }
            }
        }
        gaps
    }

    /// Peta coverage EKSEKUSI ber-granular-elemen: satu key per item yang
    /// benar-benar kena (line, branch, toggle, FSM). Dipakai fuzzer
    /// (`mivon-fuzz`) sebagai feedback coverage nyata — menggantikan
    /// sinyal teks-statistik yang gagal membedakan dua eksekusi berbeda.
    /// Deterministik per source (tanpa `$urandom`), sehingga aman dijadikan
    /// kunci novelty fuzzing.
    pub fn coverage_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = Vec::new();
        // Line coverage: key = `proc.discriminant`.
        for k in self.cover_line.keys() {
            keys.push(format!("cov_line:{}", k));
        }
        // Branch coverage: key = branch + label.
        for (bk, labels) in &self.cover_branches {
            for label in labels.keys() {
                keys.push(format!("cov_branch:{}.{}", bk, label));
            }
        }
        // Toggle coverage: nama sinyal + transisi bit (old→new).
        for (sig_id, toggles) in &self.cover_toggle {
            let name = self
                .design
                .top
                .signals
                .get(*sig_id)
                .map(|s| s.name.to_string());
            for (old, new) in toggles {
                let n = name.as_deref().unwrap_or("sig");
                keys.push(format!("cov_toggle:{}:{:?}->{:?}", n, old, new));
            }
        }
        // FSM coverage: nama sinyal + nilai state dikunjungi.
        for (sig_id, vals) in &self.cover_fsm {
            let name = self
                .design
                .top
                .signals
                .get(*sig_id)
                .map(|s| s.name.to_string());
            for v in vals {
                let n = name.as_deref().unwrap_or("sig");
                keys.push(format!("cov_fsm:{}:{:#x}", n, v));
            }
        }
        keys.sort();
        keys
    }

    /// VERIF-26: cetak coverage gap analysis ke stderr (setelah summary).
    pub(crate) fn report_coverage_gaps(&self) {
        let gaps = self.coverage_gaps();
        if gaps.is_empty() {
            eprintln!("\n=== Coverage Gaps ===");
            eprintln!("  (tidak ada gap — semua item coverage kena)");
            return;
        }
        eprintln!("\n=== Coverage Gaps ({} item tidak kena) ===", gaps.len());
        for g in gaps {
            eprintln!("  - {}", g);
        }
    }

    /// Print combined coverage report.
    pub(crate) fn report_full_coverage(&self) {
        if !self.coverage_enabled {
            return;
        }
        eprintln!("\n═══════════════════════════════════════");
        eprintln!("   COVERAGE SUMMARY REPORT");
        eprintln!("═══════════════════════════════════════");

        // Line coverage percentage
        if !self.cover_line.is_empty() {
            let total_line_hits: u64 = self.cover_line.values().sum();
            eprintln!(
                "  Line:     {} unique items, {} total hits",
                self.cover_line.len(),
                total_line_hits
            );
        } else {
            eprintln!("  Line:     (no line data)");
        }

        // Toggle coverage percentage
        if !self.cover_toggle.is_empty() {
            let mut total_signal_bits = 0usize;
            let mut covered_bits = 0usize;
            for toggles in self.cover_toggle.values() {
                let n_transitions = toggles.len();
                // Each bit can transition 0→1, 1→0, 0→X, X→0, etc (max 12 possible transitions per bit)
                covered_bits += n_transitions;
                total_signal_bits += toggles.len().max(1);
            }
            let pct = if total_signal_bits > 0 {
                (covered_bits as f64 / total_signal_bits as f64) * 100.0
            } else {
                0.0
            };
            eprintln!(
                "  Toggle:   {} signals tracked, ~{:.1}% coverage",
                self.cover_toggle.len(),
                pct
            );
        } else {
            eprintln!("  Toggle:   (no toggle data)");
        }

        // Branch coverage percentage
        if !self.cover_branches.is_empty() {
            let mut total_branches = 0u64;
            let mut covered_branches = 0u64;
            for branches in self.cover_branches.values() {
                for count in branches.values() {
                    total_branches += 1;
                    if *count > 0 {
                        covered_branches += 1;
                    }
                }
            }
            let pct = if total_branches > 0 {
                (covered_branches as f64 / total_branches as f64) * 100.0
            } else {
                0.0
            };
            eprintln!(
                "  Branch:   {}/{} branches covered ({:.1}%)",
                covered_branches, total_branches, pct
            );
        } else {
            eprintln!("  Branch:   (no branch data)");
        }

        // FSM coverage percentage
        if !self.cover_fsm.is_empty() {
            let mut total_states = 0usize;
            for states in self.cover_fsm.values() {
                total_states += states.len();
            }
            eprintln!(
                "  FSM:      {} states visited across {} signals",
                total_states,
                self.cover_fsm.len()
            );
        } else {
            eprintln!("  FSM:      (no FSM data)");
        }

        // VERIF-27: assertion coverage metrics (pass/fail per assertion).
        if !self.assertion_stats.is_empty() {
            let total_pass: u64 = self.assertion_stats.values().map(|(p, _)| *p).sum();
            let total_fail: u64 = self.assertion_stats.values().map(|(_, f)| *f).sum();
            let pct = if total_pass + total_fail > 0 {
                (total_pass as f64 / (total_pass + total_fail) as f64) * 100.0
            } else {
                0.0
            };
            eprintln!(
                "  Assert:   {} evaluated ({} pass / {} fail) {:.1}% pass rate",
                total_pass + total_fail,
                total_pass,
                total_fail,
                pct
            );
        } else {
            eprintln!("  Assert:   (no assertion data)");
        }

        eprintln!("═══════════════════════════════════════\n");

        self.report_line_coverage();
        self.report_toggle_coverage();
        self.report_branch_coverage();
        self.report_fsm_coverage();
        self.report_assertion_coverage();
        // VERIF-32: sequence coverage (attempts/matched/failed per sequence).
        self.report_sequence_coverage();
        // VERIF-26: coverage gap analysis — item yang tidak pernah kena.
        self.report_coverage_gaps();
    }

    /// Return coverage statistics as structured data (useful for CLI/JSON output).
    pub fn coverage_stats(&self) -> HashMap<String, f64> {
        let mut stats = HashMap::new();

        // Line coverage
        let line_items = self.cover_line.len() as f64;
        let line_hits: f64 = self.cover_line.values().sum::<u64>() as f64;
        stats.insert("line_items".to_string(), line_items);
        stats.insert("line_total_hits".to_string(), line_hits);

        // Toggle coverage
        let toggle_signals = self.cover_toggle.len() as f64;
        let toggle_transitions: f64 = self.cover_toggle.values().map(|s| s.len() as f64).sum();
        stats.insert("toggle_signals".to_string(), toggle_signals);
        stats.insert("toggle_transitions".to_string(), toggle_transitions);

        // Branch coverage
        let mut total_branches = 0u64;
        let mut covered_branches = 0u64;
        for branches in self.cover_branches.values() {
            for count in branches.values() {
                total_branches += 1;
                if *count > 0 {
                    covered_branches += 1;
                }
            }
        }
        let branch_pct = if total_branches > 0 {
            (covered_branches as f64 / total_branches as f64) * 100.0
        } else {
            0.0
        };
        stats.insert("branch_total".to_string(), total_branches as f64);
        stats.insert("branch_covered".to_string(), covered_branches as f64);
        stats.insert("branch_percent".to_string(), branch_pct);

        // FSM coverage
        let fsm_signals = self.cover_fsm.len() as f64;
        let fsm_states: f64 = self.cover_fsm.values().map(|s| s.len() as f64).sum();
        stats.insert("fsm_signals".to_string(), fsm_signals);
        stats.insert("fsm_states".to_string(), fsm_states);

        // Covergroup coverage (VERIF-28) — dulu HILANG dari ringkasan meski
        // datanya terkumpul (cover_total/cover_hits) dan tersimpan di
        // CoverageDatabase — `mcov` tak pernah menampilkan covergroup
        // (bug menyamar: coverage report tampak lengkap padahal kosong utk
        // covergroup). Key mengikuti pola merge_from_engine: `cg.cp` ATAU
        // per-instance `cg.i<id>.cp`.
        let mut cg_points = 0u64;
        let mut cg_covered = 0u64;
        for cg in &self.design.covergroups {
            for item in cg
                .coverpoints
                .iter()
                .map(|c| c.name)
                .chain(cg.crosses.iter().map(|c| c.name))
            {
                cg_points += 1;
                let full = format!("{}.{}", cg.name, item);
                let prefix = format!("{}.", cg.name);
                let suffix = format!(".{}", item);
                let hit = self.cover_hits.iter().any(|(k, v)| {
                    let s = k.as_str();
                    *v > 0
                        && (s == full.as_str()
                            || (s.starts_with(prefix.as_str()) && s.ends_with(suffix.as_str())))
                });
                if hit {
                    cg_covered += 1;
                }
            }
        }
        stats.insert("covergroup_points".to_string(), cg_points as f64);
        stats.insert("covergroup_covered".to_string(), cg_covered as f64);
        stats.insert(
            "covergroup_percent".to_string(),
            if cg_points > 0 {
                (cg_covered as f64 / cg_points as f64) * 100.0
            } else {
                0.0
            },
        );

        // VERIF-32: sequence coverage (concurrent assertion sequences).
        let seq_attempts: u64 = self.sequence_coverage.values().map(|s| s.attempts).sum();
        let seq_matched: u64 = self.sequence_coverage.values().map(|s| s.matched).sum();
        let seq_failed: u64 = self.sequence_coverage.values().map(|s| s.failed).sum();
        let seq_pct = if seq_attempts > 0 {
            (seq_matched as f64 / seq_attempts as f64) * 100.0
        } else {
            0.0
        };
        stats.insert(
            "sequence_tracked".to_string(),
            self.sequence_coverage.len() as f64,
        );
        stats.insert("sequence_attempts".to_string(), seq_attempts as f64);
        stats.insert("sequence_matched".to_string(), seq_matched as f64);
        stats.insert("sequence_failed".to_string(), seq_failed as f64);
        stats.insert("sequence_match_percent".to_string(), seq_pct);

        stats
    }

    /// Record toggle and FSM coverage after commit_changes.
    /// Called each delta cycle from run() loop.
    pub(crate) fn record_coverage_after_commit(&mut self) {
        if !self.coverage_enabled {
            return;
        }
        // Clone snapshot and current values to avoid double borrow of self.
        // Pakai coverage_snapshot (capture di awal time step) — signal_snapshot
        // di-refresh tiap delta cycle sehingga diff selalu kosong (fix SIM-30).
        let old_vals: Vec<LogicVec> = self.coverage_snapshot.clone().unwrap_or_default();
        for (sig_id, old_val) in old_vals.iter().enumerate() {
            // Read current signal value (now we can borrow self.state immutably)
            let new_val = self.state.read_signal(sig_id).clone();
            if old_val != &new_val {
                // Toggle: record transition — clone values to avoid borrow conflict
                self.record_toggle(sig_id, old_val, &new_val);
                // FSM: record signal value as potential state
                self.record_fsm_value(sig_id, &new_val);
            }
        }
        // ── IMPLICIT SAMPLING (IEEE 1800 §19.8): covergroup `@(edge sig)`
        // sample OTOMATIS tiap edge — tanpa ini covergroup tanpa `sample()`
        // eksplisit = 0 samples senyap (gap sesi 2026-09-28). Snapshot =
        // nilai awal time step → edge klasik (0→1) terdeteksi di sini.
        // Lebar>1 tidak dianggap clock (edge 1-bit saja).
        let mut fire: Vec<std::borrow::Cow<'_, str>> = Vec::new();
        if let Some(old_vals) = self.coverage_snapshot.as_ref() {
            for cg in &self.design.covergroups {
                let Some(sig) = cg.event_signal else { continue };
                if sig >= old_vals.len() {
                    continue;
                }
                let old = &old_vals[sig];
                let new = self.state.read_signal(sig);
                if old.width != 1 || new.width != 1 {
                    continue;
                }
                let is_one =
                    |v: &mivon_ir::LogicVec| v.bits.first() == Some(&mivon_ir::LogicVal::One);
                let is_zero =
                    |v: &mivon_ir::LogicVec| v.bits.first() == Some(&mivon_ir::LogicVal::Zero);
                let edge = if cg.event_posedge {
                    !is_one(old) && is_one(new)
                } else {
                    !is_zero(old) && is_zero(new)
                };
                if edge {
                    fire.push(std::borrow::Cow::Borrowed(cg.name.as_str()));
                }
            }
        }
        for name in fire {
            // Gagal sample tak boleh menghentikan delta cycle — laporkan
            // sebagai warning, bukan error (record_coverage dipanggil run()).
            if let Err(e) = self.sample_covergroup(name.as_ref(), None) {
                self.emit_warning(
                    mivon_core::diagnostics::DiagCode::AssertionFailed,
                    format!("implicit covergroup sample '{}': {}", name, e),
                );
            }
        }

        // ── Trace sampling MID-SIMULATION (opt-in): snapshot sinyal top tiap
        // interval waktu — mivon-fuzz fingerprint mid-sim (bug transient yang
        // pulih sebelum akhir run terlihat; fingerprint nilai-final buta).
        if let Some(iv) = self.trace_interval {
            let t = self.state.time;
            if t.is_multiple_of(iv) && t != self.trace_last_time {
                self.trace_last_time = t;
                let mut parts: Vec<String> = self
                    .design
                    .top
                    .signals
                    .iter()
                    .enumerate()
                    .map(|(i, s)| format!("{}={}", s.name, self.state.read_signal(i)))
                    .collect();
                parts.sort();
                self.trace_snapshots
                    .push(format!("t{}:{}", t, parts.join(",")));
            }
        }
    }

    /// Sample a named covergroup: evaluate coverpoints, update hit counts and bins.
    /// `instance` = obj id instance covergroup (VERIF-28: per-instance tracking
    /// saat `type_option.per_instance = 1`; None/ignored bila merge).
    pub(crate) fn sample_covergroup(
        &mut self,
        cg_name: &str,
        instance: Option<usize>,
    ) -> Result<(), SimError> {
        if !self.coverage_type_enabled(CoverageType::Covergroup) {
            return Ok(());
        }
        let cg = self
            .design
            .covergroups
            .iter()
            .find(|c| c.name == cg_name)
            .cloned();
        if let Some(cg) = cg {
            // VERIF-28: per_instance → key ber-prefix instance (`cg.i<obj>.cp`)
            // sehingga tiap instance punya akumulator terpisah.
            let inst_prefix = if cg.per_instance {
                match instance {
                    Some(id) => format!("{}.i{}", cg.name, id),
                    None => format!("{}.merge", cg.name),
                }
            } else {
                cg.name.as_str().to_string()
            };
            let mut cp_values: HashMap<String, u64> = HashMap::new();
            for cp in &cg.coverpoints {
                let key = format!("{}.{}", inst_prefix, cp.name);
                let key_sym = Symbol::intern(&key);
                let val = self
                    .evaluate_expr(&cp.expr)
                    .unwrap_or(LogicVec::from_u64(0, 32));
                let val_u = val.to_u64();
                cp_values.insert(cp.name.as_str().to_string(), val_u);

                // ── VERIF-30/31: bin eksplisit (bins/illegal_bins/ignore_bins)
                // + transition bins `(a => b)`. Sebelumnya hanya auto-binning
                // default. Semantik IEEE 1800: ignore_bins = nilai yang
                // dikecualikan (tidak dihitung); illegal_bins = nilai yang
                // TIDAK BOLEH muncul (error saat kena); bins normal = dihitung.
                //
                // PERF-16: Pre-computed constant-bin lookup O(1) via HashMap.
                let cp_key = Symbol::intern(&format!("{}.{}", inst_prefix, cp.name));
                let prev_key = cp_key;
                let prev = self.covergroup_prev.get(&prev_key).copied();
                let mut ignored = false;
                let mut illegal = false;
                let mut normal_hit: Option<Symbol> = None;

                // Build const bin map on first access (lazy cache).
                if !cp.bins.is_empty() && !self.covergroup_const_bins.contains_key(&cp_key) {
                    let (cmap, _has_non_const) = build_const_bin_map(&cp.bins);
                    self.covergroup_const_bins.insert(cp_key, cmap);
                }
                // Try O(1) constant-bin lookup first.
                if let Some(cmap) = self.covergroup_const_bins.get(&cp_key) {
                    if let Some((bin_name, bin_type)) = cmap.get(&val_u) {
                        match bin_type {
                            mivon_ast::types::BinType::Ignore => {
                                ignored = true;
                            }
                            mivon_ast::types::BinType::Illegal => {
                                illegal = true;
                            }
                            mivon_ast::types::BinType::Normal => {
                                normal_hit = Some(*bin_name);
                            }
                        }
                    }
                }
                // Fallback: linear scan for non-const bins (transitions + expr ranges).
                if !ignored && !illegal && normal_hit.is_none() {
                    'bins: for bin in &cp.bins {
                        if !bin.transitions.is_empty() {
                            // Transition bin: cocokkan (prev => curr).
                            if let Some(p) = prev {
                                for seq in &bin.transitions {
                                    if seq.len() == 2 {
                                        let v1 = self
                                            .evaluate_expr(&seq[0])
                                            .unwrap_or(LogicVec::from_u64(0, 32))
                                            .to_u64();
                                        let v2 = self
                                            .evaluate_expr(&seq[1])
                                            .unwrap_or(LogicVec::from_u64(0, 32))
                                            .to_u64();
                                        if p == v1 && val_u == v2 {
                                            match bin.bin_type {
                                                mivon_ast::types::BinType::Ignore => {
                                                    ignored = true;
                                                    break 'bins;
                                                }
                                                mivon_ast::types::BinType::Illegal => {
                                                    illegal = true;
                                                    break 'bins;
                                                }
                                                mivon_ast::types::BinType::Normal => {
                                                    normal_hit = Some(bin.name);
                                                    break 'bins;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        } else if !bin.ranges.is_empty() {
                            // Non-const range bins (only if const map missed).
                            for rng in &bin.ranges {
                                let low = self
                                    .evaluate_expr(&rng.low)
                                    .unwrap_or(LogicVec::from_u64(0, 32))
                                    .to_u64();
                                let high = match &rng.high {
                                    Some(h) => self
                                        .evaluate_expr(h)
                                        .unwrap_or(LogicVec::from_u64(0, 32))
                                        .to_u64(),
                                    None => low,
                                };
                                if val_u >= low && val_u <= high {
                                    match bin.bin_type {
                                        mivon_ast::types::BinType::Ignore => {
                                            ignored = true;
                                            break 'bins;
                                        }
                                        mivon_ast::types::BinType::Illegal => {
                                            illegal = true;
                                            break 'bins;
                                        }
                                        mivon_ast::types::BinType::Normal => {
                                            normal_hit = Some(bin.name);
                                            break 'bins;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                // Catat nilai kini untuk transisi berikutnya (setelah semua
                // decision) — berlaku walau sampel ini di-ignore.
                self.covergroup_prev.insert(prev_key, val_u);
                // ignore_bins: sampel dikecualikan — tidak dihitung sama sekali.
                if ignored {
                    continue;
                }
                let total = self.cover_total.entry(key_sym).or_insert(0);
                *total += 1;
                if illegal {
                    // illegal_bins: nilai yang tidak boleh muncul — laporkan.
                    self.emit_warning(
                        mivon_core::diagnostics::DiagCode::AssertionFailed,
                        format!(
                            "coverage illegal_bins hit: coverpoint '{}.{}' value {}",
                            cg.name, cp.name, val_u
                        ),
                    );
                    continue;
                }
                if let Some(bname) = normal_hit {
                    // Bin eksplisit normal: hit dicatat ke bin tersebut.
                    let bin_key = format!("{}={}", cp.name, bname.as_str());
                    let bin_key_sym = Symbol::intern(&bin_key);
                    let bins = self.cover_bins.entry(key_sym).or_default();
                    *bins.entry(bin_key_sym).or_insert(0) += 1;
                } else {
                    // Default bin: record the actual value — cap jumlah bin unik
                    // per coverpoint (anti-leak untuk nilai acak).
                    let bin_key = format!("{}={}", cp.name, val_u);
                    let bin_key_sym = Symbol::intern(&bin_key);
                    let bins = self.cover_bins.entry(key_sym).or_default();
                    if bins.contains_key(&bin_key_sym) {
                        *bins.get_mut(&bin_key_sym).unwrap() += 1;
                    } else if bins.len() < MAX_DEFAULT_BINS {
                        bins.insert(bin_key_sym, 1);
                    }
                }
                let hits = self.cover_hits.entry(key_sym).or_insert(0);
                *hits += 1;
            }
            // Cross coverage
            for cross in &cg.crosses {
                let key = format!("{}.{}", inst_prefix, cross.name);
                let key_sym = Symbol::intern(&key);
                let total = self.cover_total.entry(key_sym).or_insert(0);
                *total += 1;
                let mut parts: Vec<String> = Vec::new();
                for cp_name in &cross.coverpoints {
                    let val = cp_values.get(cp_name.as_str()).copied().unwrap_or(0);
                    parts.push(format!("{}={}", cp_name, val));
                }
                let bin_key = parts.join(" x ");
                let bin_key_sym = Symbol::intern(&bin_key);
                let bins = self.cover_bins.entry(key_sym).or_default();
                if bins.contains_key(&bin_key_sym) {
                    *bins.get_mut(&bin_key_sym).unwrap() += 1;
                } else if bins.len() < MAX_DEFAULT_BINS {
                    bins.insert(bin_key_sym, 1);
                }
                let hits = self.cover_hits.entry(key_sym).or_insert(0);
                *hits += 1;
            }
        }
        Ok(())
    }

    /// Total/hits satu item covergroup (coverpoint ATAU cross) — menjumlahkan
    /// key agregat (`cg.item`) DAN seluruh key per-instance (`cg.i<id>.item`)
    /// saat `type_option.per_instance = 1` (VERIF-28).
    fn cg_item_stats(&self, cg: &IrCovergroup, item: &str) -> (u64, u64) {
        let cg_str = cg.name.as_str();
        let prefix = format!("{}.", cg_str);
        let suffix = format!(".{}", item);
        let full = format!("{}.{}", cg_str, item);
        let mut total = 0u64;
        let mut hits = 0u64;
        for (k, v) in &self.cover_total {
            let s = k.as_str();
            if s == full || (s.starts_with(&prefix) && s.ends_with(&suffix)) {
                total += v;
            }
        }
        for (k, v) in &self.cover_hits {
            let s = k.as_str();
            if s == full || (s.starts_with(&prefix) && s.ends_with(&suffix)) {
                hits += v;
            }
        }
        (total, hits)
    }

    /// VERIF-28: functional coverage keseluruhan — rata-rata tertimbang
    /// (weighted) coverage per covergroup. Bobot = `type_option.weight`
    /// (default 1). Coverage satu covergroup = mean % coverpoint + cross;
    /// item/covergroup yang TIDAK PERNAH di-sample dihitung 0% (coverage
    /// hole — menurunkan metric, sesuai semantik gap analysis). Bila tidak
    /// ada covergroup sama sekali, hasil 0.0.
    pub fn functional_coverage_percent(&self) -> f64 {
        if self.design.covergroups.is_empty() {
            return 0.0;
        }
        let mut weighted_sum = 0.0;
        let mut total_weight = 0.0;
        for cg in &self.design.covergroups {
            let w = cg.weight.max(1) as f64;
            let mut item_sum = 0.0;
            let mut item_count = 0.0;
            for cp in &cg.coverpoints {
                let (total, hits) = self.cg_item_stats(cg, cp.name.as_str());
                item_sum += if total > 0 {
                    hits as f64 / total as f64
                } else {
                    0.0
                };
                item_count += 1.0;
            }
            for cross in &cg.crosses {
                let (total, hits) = self.cg_item_stats(cg, cross.name.as_str());
                item_sum += if total > 0 {
                    hits as f64 / total as f64
                } else {
                    0.0
                };
                item_count += 1.0;
            }
            if item_count > 0.0 {
                weighted_sum += w * (item_sum / item_count);
                total_weight += w;
            }
        }
        if total_weight == 0.0 {
            0.0
        } else {
            (weighted_sum / total_weight) * 100.0
        }
    }

    /// Print coverage report to stderr.
    pub(crate) fn report_coverage(&self) {
        if self.design.covergroups.is_empty() {
            return;
        }
        eprintln!("\n=== Coverage Report ===");
        for cg in &self.design.covergroups {
            eprintln!("Covergroup: {}", cg.name);
            for cp in &cg.coverpoints {
                let (total, hits) = self.cg_item_stats(cg, cp.name.as_str());
                let pct = if total > 0 {
                    (hits as f64 / total as f64) * 100.0
                } else {
                    0.0
                };
                eprintln!(
                    "  {}: {} hits / {} samples ({:.1}%)",
                    cp.name, hits, total, pct
                );
            }
            for cross in &cg.crosses {
                let (total, hits) = self.cg_item_stats(cg, cross.name.as_str());
                let pct = if total > 0 {
                    (hits as f64 / total as f64) * 100.0
                } else {
                    0.0
                };
                eprintln!(
                    "  {} (cross): {} hits / {} samples ({:.1}%)",
                    cross.name, hits, total, pct
                );
            }
            eprintln!(
                "  (type_option.weight = {} | per_instance = {})",
                cg.weight, cg.per_instance
            );
        }
        // VERIF-28: functional coverage keseluruhan tertimbang.
        eprintln!(
            "  Functional: {:.1}% (weighted by type_option.weight)",
            self.functional_coverage_percent()
        );
    }

    /// Export all coverage data to UCIS XML format (IEEE 1800 UCIS schema).
    /// Includes: covergroup, line, toggle, branch, and FSM coverage.
    pub fn export_coverage_ucis(&self, path: &str) -> Result<(), SimError> {
        let mut xml = String::new();
        xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        xml.push_str("<coverageDatabase xmlns=\"urn:ucis:0.1\">\n");
        xml.push_str(&format!(
            "  <design name=\"{}\">\n",
            escape_xml(self.design.top.name.as_str())
        ));

        // ── Covergroup Coverage ──
        if !self.design.covergroups.is_empty() {
            xml.push_str("    <functionalCoverage>\n");
            for cg in &self.design.covergroups {
                xml.push_str(&format!(
                    "      <covergroup name=\"{}\">\n",
                    escape_xml(cg.name.as_str())
                ));

                for cp in &cg.coverpoints {
                    let key = format!("{}.{}", cg.name, cp.name);
                    let key_sym = Symbol::intern(&key);
                    let total = self.cover_total.get(&key_sym).copied().unwrap_or(0);
                    let hits = self.cover_hits.get(&key_sym).copied().unwrap_or(0);
                    let pct = if total > 0 {
                        (hits as f64 / total as f64) * 100.0
                    } else {
                        0.0
                    };

                    xml.push_str(&format!(
                        "        <coverpoint name=\"{}\" total=\"{}\" hits=\"{}\" coverage=\"{:.1}\">\n",
                        escape_xml(cp.name.as_str()), total, hits, pct
                    ));

                    if let Some(bins) = self.cover_bins.get(&key_sym) {
                        for (bin_key, count) in bins.iter() {
                            xml.push_str(&format!(
                                "          <bin name=\"{}\" hits=\"{}\"/>\n",
                                escape_xml(bin_key.as_str()),
                                count
                            ));
                        }
                    }

                    xml.push_str("        </coverpoint>\n");
                }

                for cross in &cg.crosses {
                    let key = format!("{}.{}", cg.name, cross.name);
                    let key_sym = Symbol::intern(&key);
                    let total = self.cover_total.get(&key_sym).copied().unwrap_or(0);
                    let hits = self.cover_hits.get(&key_sym).copied().unwrap_or(0);
                    let pct = if total > 0 {
                        (hits as f64 / total as f64) * 100.0
                    } else {
                        0.0
                    };

                    xml.push_str(&format!(
                        "        <cross name=\"{}\" total=\"{}\" hits=\"{}\" coverage=\"{:.1}\">\n",
                        escape_xml(cross.name.as_str()),
                        total,
                        hits,
                        pct
                    ));

                    if let Some(bins) = self.cover_bins.get(&key_sym) {
                        for (bin_key, count) in bins.iter() {
                            xml.push_str(&format!(
                                "          <bin name=\"{}\" hits=\"{}\"/>\n",
                                escape_xml(bin_key.as_str()),
                                count
                            ));
                        }
                    }

                    xml.push_str("        </cross>\n");
                }

                xml.push_str("      </covergroup>\n");
            }
            xml.push_str("    </functionalCoverage>\n");
        }

        // ── Line Coverage ──
        if !self.cover_line.is_empty() {
            xml.push_str("    <lineCoverage>\n");
            let mut sorted_line: Vec<(&str, &u64)> = self
                .cover_line
                .iter()
                .map(|(k, v)| (k.as_str(), v))
                .collect();
            sorted_line.sort_by(|a, b| b.1.cmp(a.1));
            xml.push_str(&format!(
                "      <summary totalItems=\"{}\" totalHits=\"{}\"/>\n",
                self.cover_line.len(),
                self.cover_line.values().sum::<u64>()
            ));
            for (key, hits) in sorted_line {
                xml.push_str(&format!(
                    "      <lineItem key=\"{}\" hits=\"{}\"/>\n",
                    escape_xml(key),
                    hits
                ));
            }
            xml.push_str("    </lineCoverage>\n");
        }

        // ── Toggle Coverage ──
        if !self.cover_toggle.is_empty() {
            xml.push_str("    <toggleCoverage>\n");
            let mut sorted_toggle: Vec<(&usize, &HashSet<(LogicVal, LogicVal)>)> =
                self.cover_toggle.iter().collect();
            sorted_toggle.sort_by_key(|(id, _)| *id);
            xml.push_str(&format!(
                "      <summary totalSignals=\"{}\"/>\n",
                self.cover_toggle.len()
            ));
            for (sig_id, transitions) in sorted_toggle {
                let sig_name = self
                    .design
                    .top
                    .signals
                    .get(*sig_id)
                    .map(|s| s.name.as_str())
                    .unwrap_or("<unknown>");
                xml.push_str(&format!(
                    "      <signal id=\"{}\" name=\"{}\" transitions=\"{}\">\n",
                    sig_id,
                    escape_xml(sig_name),
                    transitions.len()
                ));
                for (from, to) in transitions.iter() {
                    xml.push_str(&format!(
                        "        <transition from=\"{:?}\" to=\"{:?}\"/>\n",
                        from, to
                    ));
                }
                xml.push_str("      </signal>\n");
            }
            xml.push_str("    </toggleCoverage>\n");
        }

        // ── Branch Coverage ──
        if !self.cover_branches.is_empty() {
            xml.push_str("    <branchCoverage>\n");
            let mut total_br = 0u64;
            let mut covered_br = 0u64;
            for branches in self.cover_branches.values() {
                for count in branches.values() {
                    total_br += 1;
                    if *count > 0 {
                        covered_br += 1;
                    }
                }
            }
            let br_pct = if total_br > 0 {
                (covered_br as f64 / total_br as f64) * 100.0
            } else {
                0.0
            };
            xml.push_str(&format!(
                "      <summary totalBranches=\"{}\" coveredBranches=\"{}\" coverage=\"{:.1}\"/>\n",
                total_br, covered_br, br_pct
            ));
            let mut sorted_branch: Vec<(&str, &HashMap<Symbol, u64>)> = self
                .cover_branches
                .iter()
                .map(|(k, v)| (k.as_str(), v))
                .collect();
            sorted_branch.sort_by(|a, b| a.0.cmp(b.0));
            for (key, branches) in sorted_branch {
                xml.push_str(&format!("      <branchItem key=\"{}\">\n", escape_xml(key)));
                for (label, count) in branches {
                    xml.push_str(&format!(
                        "        <branch label=\"{}\" hits=\"{}\"/>\n",
                        escape_xml(label.as_str()),
                        count
                    ));
                }
                xml.push_str("      </branchItem>\n");
            }
            xml.push_str("    </branchCoverage>\n");
        }

        // ── FSM Coverage ──
        if !self.cover_fsm.is_empty() {
            xml.push_str("    <fsmCoverage>\n");
            xml.push_str(&format!(
                "      <summary totalSignals=\"{}\" totalStates=\"{}\"/>\n",
                self.cover_fsm.len(),
                self.cover_fsm.values().map(|s| s.len()).sum::<usize>()
            ));
            let mut sorted_fsm: Vec<(&usize, &HashSet<u64>)> = self.cover_fsm.iter().collect();
            sorted_fsm.sort_by_key(|(id, _)| *id);
            for (sig_id, states) in sorted_fsm {
                let sig_name = self
                    .design
                    .top
                    .signals
                    .get(*sig_id)
                    .map(|s| s.name.as_str())
                    .unwrap_or("<unknown>");
                let mut sorted_states: Vec<u64> = states.iter().copied().collect();
                sorted_states.sort();
                xml.push_str(&format!(
                    "      <signal id=\"{}\" name=\"{}\" states=\"{}\">\n",
                    sig_id,
                    escape_xml(sig_name),
                    states.len()
                ));
                for state in &sorted_states {
                    xml.push_str(&format!("        <state value=\"{}\"/>\n", state));
                }
                xml.push_str("      </signal>\n");
            }
            xml.push_str("    </fsmCoverage>\n");
        }

        xml.push_str("  </design>\n");
        xml.push_str("</coverageDatabase>\n");

        std::fs::write(path, xml).map_err(|e| {
            SimError::with_diag(
                DiagCode::IoError,
                format!("cannot write UCIS file '{}': {}", path, e),
            )
        })?;

        // Print summary to stderr
        let line_count = self.cover_line.len();
        let toggle_count = self.cover_toggle.len();
        let branch_count = self.cover_branches.len();
        let fsm_count = self.cover_fsm.len();
        let cg_count = self.design.covergroups.len();
        eprintln!(
            "UCIS exported: covergroups={} line={} toggle={} branch={} fsm={}",
            cg_count, line_count, toggle_count, branch_count, fsm_count
        );

        Ok(())
    }
}
