use mivon_core::diagnostics::diagnostic::{DiagCode, Diagnostic};
use mivon_core::error::SimError;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

const MAX_INCLUDE_DEPTH: usize = 64;

/// Maximum macro expansion depth to prevent infinite recursion.
const MAX_MACRO_EXPANSION_DEPTH: usize = 64;
/// Batas panjang baris hasil ekspansi. Ekspansi eksponensial (`define A `A `A`)
/// menggandakan ukuran tiap level — depth cap saja tidak cukup karena pohon
/// pemanggilan tetap 2^depth sebelum tersentuh. Tanpa batas ini: hang/OOM.
const MAX_MACRO_EXPANSION_LEN: usize = 1 << 20;

struct CondFrame {
    taking_branch: bool,
    branch_taken: bool,
}

/// Nama direktif kondisional — saat muncul DI DALAM body macro (bukan awal
/// baris file), backtick harus dipertahankan agar post-processing
/// (`fold_expanded_conditionals`) bisa mengevaluasinya.
fn is_cond_directive(name: &str) -> bool {
    matches!(name, "ifdef" | "ifndef" | "elsif" | "else" | "endif")
}

#[derive(Clone)]
struct MacroDef {
    value: String,
    params: Vec<String>,
    /// Nilai default per param SV (`` `define M(a, b = 2) `` → b default "2").
    /// Kosong bila param tak punya default.
    defaults: Vec<String>,
}

#[derive(Clone)]
pub struct Preprocessor {
    defines: HashMap<String, MacroDef>,
    search_paths: Vec<PathBuf>,
    warned_includes: HashSet<String>,
    include_stack: Vec<PathBuf>,
    include_set: std::collections::HashSet<PathBuf>,
    /// Path file yang sedang diproses — untuk `` `line `` RESTORE setelah
    /// include (tanpa ini token setelah include salah-label file include).
    /// Di-set caller (compile_session), default None = label "<string>".
    pub cur_path: Option<String>,
    pub quiet: bool,
    pub timescale: Option<(String, String)>, // (unit, precision)
    pub warnings: Vec<Diagnostic>,
    /// Semua file include yang pernah di-resolve (transitif) selama pemrosesan
    /// file ini. Dipakai MICD untuk memverifikasi header tidak berubah sebelum
    /// me-reuse AST/preprocessed cache (koreksi correctness).
    pub resolved_includes: std::collections::HashSet<PathBuf>,
    /// Line ranges (start, end) inklusif, 1-based, dalam koordinat output
    /// preprocessed, yang di-exclude dari coverage oleh `` `coverage_off ``
    /// ... `` `coverage_on `` (IEEE 1800 simulation control directives).
    pub coverage_exclusions: Vec<(usize, usize)>,
    /// Segmen `` `timescale `` per baris OUTPUT preprocessed (F84).
    ///
    /// LRM 1800-2017 §19.8: `` `timescale `` berlaku untuk SEMUA module yang
    /// SETELAH directive, dan berubah setiap ada directive baru — bukan
    /// properti design global. Design satu file bisa mencampur beberapa
    /// satuan (mis. RTL inti `1ns/1ps` + testbench `1us/1ns`), dan `#5` di
    /// masing-masing berarti 5ns vs 5us.
    ///
    /// Dulu `timescale` tunggal dipakai untuk seluruh design, sehingga
    /// `#5` di module `1us` diperlakukan 5 tick basis `1ns` (atau sebaliknya)
    /// → hasil simulasi menyimpang diam-diam vs `iverilog`.
    ///
    /// Isi: `(baris_output_1based_dimulai_berlaku, (unit, precision))`,
    /// terurut menaik. Baris sebelum entri pertama = belum ada directive.
    pub timescale_segments: Vec<(usize, (String, String))>,
}

impl Preprocessor {
    pub fn new() -> Self {
        Self {
            defines: HashMap::new(),
            search_paths: Vec::new(),
            warned_includes: HashSet::new(),
            include_stack: Vec::new(),
            include_set: std::collections::HashSet::new(),
            quiet: false,
            timescale: None,
            warnings: Vec::new(),
            resolved_includes: std::collections::HashSet::new(),
            coverage_exclusions: Vec::new(),
            timescale_segments: Vec::new(),
            cur_path: None,
        }
    }

    /// Timescale yang berlaku pada baris OUTPUT tertentu (1-based).
    ///
    /// Entri terakhir dengan `from_line <= line` menang; `None` kalau baris
    /// itu belum didahului directive `` `timescale `` (LRM §19.8 — module
    /// sebelum directive pertama memakai default simulator).
    pub fn timescale_at_line(&self, line: usize) -> Option<&(String, String)> {
        self.timescale_segments
            .iter()
            .rev()
            .find(|(from, _)| *from <= line)
            .map(|(_, ts)| ts)
    }

    /// Gabung segmen dari beberapa file menjadi satu daftar terurut (F84).
    ///
    /// `compile_files` memproses tiap file terpisah lalu menyambung source-nya
    /// dengan penanda `` `line ``. Segmen per file harus digeser sesuai
    /// offset baris file tersebut agar lookup di source gabungan tetap benar.
    pub fn merge_timescale_segments(
        segments: &[(usize, (String, String))],
        line_offset: usize,
        out: &mut Vec<(usize, (String, String))>,
    ) {
        for (from, ts) in segments {
            out.push((from + line_offset, ts.clone()));
        }
    }

    pub fn max_macro_expansion_depth(&self) -> usize {
        MAX_MACRO_EXPANSION_DEPTH
    }

    pub fn define(&mut self, name: &str, value: &str) {
        self.defines.insert(
            name.to_string(),
            MacroDef {
                value: value.to_string(),
                params: Vec::new(),
                defaults: Vec::new(),
            },
        );
    }

    pub fn add_search_path(&mut self, path: &str) {
        self.search_paths.push(PathBuf::from(path));
    }

    pub fn preprocess_file(&mut self, filename: &str) -> Result<String, SimError> {
        // Label `` `line `` RESTORE utk include: path file induk.
        self.cur_path = Some(filename.to_string());
        let path = Path::new(filename);
        let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let source = fs::read_to_string(filename)
            .map_err(|e| SimError::preprocessor(format!("cannot read '{}': {}", filename, e)))?;
        let processed = self.preprocess(&source, Some(&dir))?;
        Ok(processed)
    }

    pub fn preprocess(
        &mut self,
        source: &str,
        current_dir: Option<&PathBuf>,
    ) -> Result<String, SimError> {
        let lines: Vec<&str> = source.lines().collect();
        let mut output = String::new();
        let mut i = 0;
        let mut cond_stack: Vec<CondFrame> = vec![];
        let mut emitting = true;
        // SIM-29: tracking region `` `coverage_off `` ... `` `coverage_on ``
        let mut cov_start: Option<usize> = None;

        while i < lines.len() {
            let mut raw_line = lines[i].to_string();
            loop {
                // `\` di AKHIR baris hanya line-continuation bila berada di
                // LUAR comment/string. File OpenTitan (aon_osc.sv) menaruh `\`
                // di dalam `// ...` comment untuk MENGAKHIRI komentar (baris
                // berikutnya `` `endif `` tetap directive nyata) — perlakuan
                // `\`-in-comment sebagai continuation (sebelumnya) menelan
                // `` `endif `` sehingga ifdef tak seimbang dan module hilang.
                // KECUALI kita sedang di dalam body `` `define ``: body define
                // SV boleh memuat baris KOMENTAR yang berakhiran `\` (mis.
                // ASSERT_INIT_NET prim_assert: baris `// When a net is
                // assigned... \`). Menerapkan rule aon_osc di sini memotong
                // body define → sisa baris BOCOK ke output sebagai kode →
                // direct-error parser ("skipping Hash" / expected-*; ratusan
                // file RTL/DV OpenTitan). Dalam body define, per LRM 1800
                // §22.5.1 backslash-newline tetap di-delete.
                let in_define_body = raw_line.trim_start().starts_with("`define");
                if !in_define_body && !trailing_backslash_is_continuation(&raw_line) {
                    break;
                }
                let te = raw_line.trim_end();
                if !te.ends_with('\\') && !te.ends_with("\\\r") {
                    break;
                }
                if te.ends_with('\r') {
                    raw_line.pop();
                }
                raw_line = raw_line.trim_end().to_string();
                raw_line.pop(); // remove trailing \
                i += 1;
                if i < lines.len() {
                    // LRM 1800 §22.5.1: `\` + <newline> di-DELETE. Baris yang
                    // digabung tidak boleh memutus komentar (case aon_osc.sv:
                    // `// ... input\` + `` `endif `` — jika joined dengan
                    // newline, `` `endif `` menjadi directive nyata dan ifdef
                    // jadi tak seimbang → module hilang).
                    //
                    // Di dalam body `` `define `` newline DI-PERTAHANKAN:
                    // body define boleh memuat baris KOMENTAR yang berakhiran
                    // `\` (mis. WAIT_FOR_FORCED_SIGNAL / ASSERT_INIT_NET /
                    // DV_SPINWAIT_EXIT). Kalau `\`-continuation meratakan
                    // komentar garis-tunggal jadi satu baris panjang, `` // ``
                    // pertama menelan SISA BARIS (termasuk `@(...)`/`end`)
                    // → "expected expression, found EndTask" dsb. Newline
                    // menjaga struktur baris; token-paste antar baris define
                    // (`foo\` + `bar` → token terpisah) praktis tidak dipakai
                    // SV nyata (paste memakai `` ` ``, bukan `\`).
                    if in_define_body {
                        raw_line.push('\n');
                    }
                    raw_line.push_str(lines[i]);
                } else {
                    break;
                }
            }
            // Macro invocation dapat menjalar beberapa baris tanpa `\` di baris
            // pertama (OpenTitan menulis `ASSERT_STATIC_IN_PACKAGE(Check,
            //     (a == b))` — paren argumen belum tertutup di baris pertama).
            // Gabungkan baris berikutnya selama parens invokasi belum seimbang.
            if unbalanced_macro_call(&raw_line) {
                while i + 1 < lines.len() && unbalanced_macro_call(&raw_line) {
                    i += 1;
                    raw_line.push('\n');
                    raw_line.push_str(lines[i]);
                }
            }
            let trimmed = raw_line.trim();

            if !trimmed.starts_with('`') {
                if emitting {
                    let expanded = self.expand_inline_macros(&raw_line);
                    // Post-ekspansi: fold direktif kondisional yang muncul di
                    // DALAM body macro (`` `ifdef/`else/`endif `` ASSERT_ERROR
                    // prim_assert). Tanpa fold: direktif jadi teks polos → parser
                    // rusak (~586 file RTL OpenTitan, direct in macro).
                    let folded = self.fold_expanded_conditionals(&expanded);
                    output.push_str(&folded);
                    output.push('\n');
                    // Body macro multi-baris (with `\` kontinuasi) menambah
                    // baris FISIK → nilai line token berikut drift. Re-sync
                    // ke baris file induk berikut (i+2, 1-based).
                    if folded.contains('\n') {
                        if let Some(ref p) = self.cur_path {
                            output.push_str(&format!("`line {} \"{}\"\n", i + 2, p));
                        }
                    }
                }
                i += 1;
                continue;
            }

            let directive = &trimmed[1..];
            let (cmd, rest) = self.split_directive(directive);
            // Baris yang diawali backtick bisa berupa DIRECTIVE (`` `ifdef ``,
            // `` `include ``) ATAU invokasi macro di awal baris (`` `uvm_info(...) ``
            // — pola umum UVM). Kalau nama setelah backtick adalah macro yang
            // sudah didefinisikan, ini INVOKASI — expand seperti baris biasa,
            // bukan directive (sebelumnya di-skip diam-diam → macro UVM & macro
            // statement lain di awal baris tidak pernah dieksekusi).
            if emitting && self.defines.contains_key(cmd) {
                let expanded = self.expand_inline_macros(&raw_line);
                let folded = self.fold_expanded_conditionals(&expanded);
                output.push_str(&folded);
                output.push('\n');
                // Sama — macro invocation multi-baris: re-sync physical line.
                if folded.contains('\n') {
                    if let Some(ref p) = self.cur_path {
                        output.push_str(&format!("`line {} \"{}\"\n", i + 2, p));
                    }
                }
                i += 1;
                continue;
            }

            match cmd {
                "include" => {
                    if emitting {
                        if rest.trim().starts_with('`') {
                            // Not actually an include — misparsed due to nested backtick
                            i += 1;
                            continue;
                        }
                        let inc_path = match self.parse_include_path(rest) {
                            Ok(p) => p,
                            Err(e) => {
                                if !self.quiet {
                                    self.warnings.push(Diagnostic::warning(
                                        DiagCode::InvalidSyntax,
                                        format!("{}", e),
                                    ));
                                }
                                i += 1;
                                continue;
                            }
                        };
                        match self.resolve_path(&inc_path, current_dir) {
                            Ok(resolved) => {
                                if self.include_stack.len() >= MAX_INCLUDE_DEPTH {
                                    return Err(SimError::preprocessor(format!(
                                        "include depth exceeded ({}) — possible circular include for '{}'",
                                        MAX_INCLUDE_DEPTH, inc_path
                                    )));
                                }
                                if self.include_set.contains(&resolved) {
                                    return Err(SimError::preprocessor(format!(
                                        "circular include detected: '{}' already in include stack",
                                        inc_path
                                    )));
                                }
                                self.include_stack.push(resolved.clone());
                                self.include_set.insert(resolved.clone());
                                self.resolved_includes.insert(resolved.clone());
                                self.warned_includes.remove(&inc_path);
                                // Auto-aktifkan `UVM bila design menyertakan
                                // macro-macros UVM/DV (dv_macros.svh /
                                // uvm_macros.svh). mivon punya infra UVM
                                // built-in (uvm_info/warning/error/fatal +
                                // factory via engine dispatch); tanpa `UVM,
                                // `` `gfn `` di dv_macros.svh jatuh ke cabang
                                // `$sformatf("%m")` → `X.`gfn` me-expand jadi
                                // `X.$sformatf(...)` — member-access invalid
                                // (E1002 "expected identifier, found Dollar").
                                // Dengan `UVM → `` `gfn `` = get_full_name() →
                                // `X.get_full_name()` valid. Scoped: hanya
                                // file yang include uvm/dv macros; RTL murni
                                // tanpa UVM tidak terpengaruh.
                                if !self.defines.contains_key("UVM") {
                                    let base = resolved
                                        .file_name()
                                        .map(|b| b.to_string_lossy().into_owned())
                                        .unwrap_or_default();
                                    if base.starts_with("uvm_macros") || base == "dv_macros.svh" {
                                        self.defines.insert(
                                            "UVM".to_string(),
                                            MacroDef {
                                                value: String::new(),
                                                params: Vec::new(),
                                                defaults: Vec::new(),
                                            },
                                        );
                                    }
                                }
                                let inc_result = (|| -> Result<(), SimError> {
                                    let inc_source =
                                        fs::read_to_string(&resolved).map_err(|e| {
                                            SimError::preprocessor(format!(
                                                "cannot read include '{}': {}",
                                                resolved.display(),
                                                e
                                            ))
                                        })?;
                                    let inc_dir = resolved.parent().map(|p| p.to_path_buf());
                                    output
                                        .push_str(&format!("`line 1 \"{}\"\n", resolved.display()));
                                    // cur_path = include utk nested include di dalamnya.
                                    let outer_path = self.cur_path.clone();
                                    self.cur_path = Some(resolved.display().to_string());
                                    let processed =
                                        self.preprocess(&inc_source, inc_dir.as_ref())?;
                                    output.push_str(&processed);
                                    if !processed.ends_with('\n') {
                                        output.push('\n');
                                    }
                                    // RESTORE `` `line {i+2} "outer" ``: token
                                    // berikutnya milik file INDUK. Include ada di
                                    // baris i+1 (1-based) → baris berikut = i+2.
                                    if let Some(ref outer) = outer_path {
                                        output.push_str(&format!(
                                            "`line {} \"{}\"\n",
                                            i + 2,
                                            outer
                                        ));
                                    }
                                    self.cur_path = outer_path;
                                    Ok(())
                                })();
                                let popped = self.include_stack.pop();
                                if let Some(p) = popped {
                                    self.include_set.remove(&p);
                                }
                                if let Err(e) = inc_result {
                                    if !self.quiet {
                                        self.warnings.push(Diagnostic::warning(
                                            DiagCode::InvalidSyntax,
                                            format!("{}", e),
                                        ));
                                    }
                                }
                            }
                            Err(e) => {
                                if !self.quiet && self.warned_includes.insert(inc_path.clone()) {
                                    self.warnings.push(Diagnostic::warning(
                                        DiagCode::InvalidSyntax,
                                        format!("{}", e),
                                    ));
                                }
                            }
                        }
                    }
                }
                "define" => {
                    if emitting {
                        self.parse_define(rest);
                    }
                }
                "undef" => {
                    if emitting {
                        let name = rest.trim();
                        if !name.is_empty() {
                            self.defines.remove(name);
                        }
                    }
                }
                "ifdef" => {
                    let defined = self.eval_ifdef_expr(rest.trim());
                    cond_stack.push(CondFrame {
                        taking_branch: defined,
                        branch_taken: defined,
                    });
                    emitting = cond_stack.iter().all(|f| f.taking_branch);
                }
                "ifndef" => {
                    let defined = self.eval_ifdef_expr(rest.trim());
                    cond_stack.push(CondFrame {
                        taking_branch: !defined,
                        branch_taken: !defined,
                    });
                    emitting = cond_stack.iter().all(|f| f.taking_branch);
                }
                "elsif" => {
                    let frame = cond_stack.last_mut().ok_or_else(|| {
                        SimError::preprocessor(format!(
                            "{}:{}:{}: line {}: `elsif without matching `ifdef/`ifndef",
                            self.cur_path.as_deref().unwrap_or("<string>"),
                            i + 1,
                            1,
                            i + 1
                        ))
                    })?;
                    if frame.branch_taken {
                        frame.taking_branch = false;
                    } else {
                        let defined = self.eval_ifdef_expr(rest.trim());
                        if defined {
                            frame.taking_branch = true;
                            frame.branch_taken = true;
                        }
                    }
                    emitting = cond_stack.iter().all(|f| f.taking_branch);
                }
                "else" => {
                    let frame = cond_stack.last_mut().ok_or_else(|| {
                        SimError::preprocessor(format!(
                            "{}:{}:{}: line {}: `else without matching `ifdef/`ifndef",
                            self.cur_path.as_deref().unwrap_or("<string>"),
                            i + 1,
                            1,
                            i + 1
                        ))
                    })?;
                    if frame.branch_taken {
                        frame.taking_branch = false;
                    } else {
                        frame.taking_branch = true;
                        frame.branch_taken = true;
                    }
                    emitting = cond_stack.iter().all(|f| f.taking_branch);
                }
                "endif" => {
                    cond_stack.pop().ok_or_else(|| {
                        SimError::preprocessor(format!(
                            "{}:{}:{}: line {}: `endif without matching `ifdef/`ifndef",
                            self.cur_path.as_deref().unwrap_or("<string>"),
                            i + 1,
                            1,
                            i + 1
                        ))
                    })?;
                    emitting = cond_stack.iter().all(|f| f.taking_branch);
                }
                "line" => {
                    if emitting {
                        output.push_str(&raw_line);
                        output.push('\n');
                    }
                }
                "timescale" => {
                    // `timescale 1ns / 1ps — parse, store sebagai fallback
                    // global DAN catat SEGMEN per baris output (LRM §19.8:
                    // berlaku ke module sesudah directive, berubah tiap
                    // directive baru — lihat `timescale_segments`).
                    let ts = rest.trim();
                    let parsed = if let Some(slash_pos) = ts.find('/') {
                        let unit = ts[..slash_pos].trim().to_string();
                        let prec = ts[slash_pos + 1..].trim().to_string();
                        Some((unit, prec))
                    } else if !ts.is_empty() {
                        Some((ts.to_string(), String::new()))
                    } else {
                        None
                    };
                    if let Some(p) = parsed {
                        self.timescale = Some(p.clone());
                        // Directive ini TIDAK ikut output (sudah dikonsumsi), jadi
                        // baris output berikutnya = baris pertama yang
                        // terpengaruh directive ini.
                        let from_line = output.lines().count() + 1;
                        // Directive di dalam `ifdef` yang TIDAK diemitting
                        // tak berlaku — hanya directive yang lolos
                        // preprocessing yang punya efek (sama dengan
                        // `timescale` global yang juga hanya di-set di
                        // cabang ini).
                        if emitting {
                            self.timescale_segments.push((from_line, p));
                        }
                    }
                }
                "coverage_off" => {
                    if emitting && cov_start.is_none() {
                        // Baris output berikutnya (1-based) jadi awal region exclude.
                        cov_start = Some(output.lines().count() + 1);
                    }
                }
                "coverage_on" => {
                    if emitting {
                        if let Some(start) = cov_start.take() {
                            let end = output.lines().count();
                            if start <= end {
                                self.coverage_exclusions.push((start, end));
                            }
                        }
                    }
                }
                "default_nettype" => {
                    // `default_nettype wire|none|... — track for implicit net declarations
                    // Currently tracked but not enforced in elaborated
                }
                "celldefine"
                | "endcelldefine"
                | "unconnected_drive"
                | "nounconnected_drive"
                | "pragma"
                | "assert"
                | "debug"
                | "resetall"
                | "PICORV32_REGS" => {
                    // Standard or tool-specific Verilog directives that we ignore
                }
                "FORMAL_KEEP" => {
                    // Yosys formal attribute — emit the rest as Verilog declaration
                    if emitting {
                        output.push_str(rest);
                        output.push('\n');
                    }
                }
                _ if emitting
                    && (rest.trim_start().starts_with('(')
                        || rest.trim_start().starts_with('.')
                        || rest.trim_start().starts_with('[')) =>
                {
                    // Unknown backtick macro at line start (mis.
                    // `` `uvm_error(...) `` / `` `uvm_info(...) `` saat uvm_macros
                    // tidak ter-dedefine; `` `CR.rf_wdata_fwd_wb `` / `` `RF.reg[i] `` —
                    // referensi macro scope/array di formal checker yg definisinya
                    // ada di file lain, tidak di-substitusi). Ini BUKAN directive —
                    // strip backtick dan expan baris statement biasa, supaya
                    // `CR.rf_wdata_fwd_wb` tetap parse-able dan baris tidak
                    // di-skip (sebelumnya baris `? \n `CR.x : `RF.y;` DI-DROP →
                    // "expected expression, found End/Assign" palsu).
                    let expanded = self.expand_inline_macros(&raw_line);
                    output.push_str(&expanded);
                    output.push('\n');
                }
                _ => {
                    // Unknown backtick directive — skip silently
                }
            }

            i += 1;
        }

        // Tutup region coverage_off yang belum ditutup di akhir file
        if let Some(start) = cov_start.take() {
            let end = output.lines().count();
            if start <= end {
                self.coverage_exclusions.push((start, end));
            }
        }

        if !cond_stack.is_empty() {
            let any_taking = cond_stack.iter().any(|f| f.taking_branch);
            if !self.quiet && any_taking && cond_stack.len() <= 3 {
                // Show the current file name if available from the source
                let file_hint = current_dir
                    .as_ref()
                    .and_then(|d| d.file_name())
                    .map(|n| format!(" in '{}'", n.to_string_lossy()))
                    .unwrap_or_default();
                self.warnings.push(Diagnostic::warning(
                    DiagCode::InvalidSyntax,
                    format!(
                        "{} open `ifdef/`ifndef block(s) at end of file{} (auto-closed)",
                        cond_stack.len(),
                        file_hint
                    ),
                ));
            }
            // Auto-close remaining conditionals so they don't corrupt subsequent files
            while let Some(_frame) = cond_stack.pop() {
                // Frame automatically dropped
            }
        }

        Ok(output)
    }

    #[allow(dead_code)]
    fn is_emitting(&self, stack: &[CondFrame]) -> bool {
        stack.iter().all(|f| f.taking_branch)
    }

    fn split_directive<'a>(&self, directive: &'a str) -> (&'a str, &'a str) {
        let trimmed = directive.trim_start();
        // Stop-set: whitespace, `(`, `[`, DAN `.`. Tanpa `.`, nama macro yang
        // dirangkai hierarki (`` `CLKMGR_HIER.reg2hw.foo.q ``) dianggap
        // satu nama penuh → `defines.contains_key(cmd)` gagal → baris di-skip
        // diam-diam (macro expansion hilang, cast multi-baris rusak).
        let end = trimmed
            .find(|c: char| c.is_whitespace() || c == '(' || c == '[' || c == '.')
            .unwrap_or(trimmed.len());
        let cmd = &trimmed[..end];
        let rest = trimmed[end..].trim();
        (cmd, rest)
    }

    fn parse_include_path(&self, rest: &str) -> Result<String, SimError> {
        let s = rest.trim();
        if s.starts_with('`') {
            return Err(SimError::preprocessor(format!(
                "include path is a macro reference (not a string literal): {}",
                s
            )));
        }
        if let Some(rest) = s.strip_prefix('"') {
            let end = rest
                .find('"')
                .ok_or_else(|| SimError::preprocessor("unterminated include path"))?;
            Ok(rest[..end].to_string())
        } else if let Some(rest) = s.strip_prefix('<') {
            let end = rest
                .find('>')
                .ok_or_else(|| SimError::preprocessor("unterminated include path"))?;
            Ok(rest[..end].to_string())
        } else {
            Err(SimError::preprocessor(format!(
                "invalid include syntax: {}",
                s
            )))
        }
    }

    fn resolve_path(
        &self,
        inc_path: &str,
        current_dir: Option<&PathBuf>,
    ) -> Result<PathBuf, SimError> {
        if let Some(dir) = current_dir {
            let candidate = dir.join(inc_path);
            if candidate.exists() {
                return Ok(candidate);
            }
        }
        for search_path in &self.search_paths {
            let candidate = search_path.join(inc_path);
            if candidate.exists() {
                return Ok(candidate);
            }
        }
        let candidate = PathBuf::from(inc_path);
        if candidate.exists() {
            return Ok(candidate);
        }
        Err(SimError::preprocessor(format!(
            "include file '{}' not found",
            inc_path
        )))
    }

    fn parse_define(&mut self, rest: &str) {
        let s = rest.trim();
        if s.is_empty() {
            return;
        }

        // A function-like macro requires `NAME(` with no whitespace between
        // the name and `(`.  Object-like macros commonly use a parenthesized
        // replacement with whitespace, e.g. `define HAS_PARITY (expr)`.
        // Penting: cari nama = TOKEN PERTAMA (berhenti di whitespace ATAU `(`).
        // Sebelumnya `s.find('(')` ambil `(` yang PERTAMA di string — salah
        // saat value memuat invokasi bersarang langsung: `define IS_LB
        // `IS_LOAD(3'b001)` → nama jadi "IS_LB `IS_LOAD" + value kosong →
        // `IS_LB tidak pernah ter-expand (riscv-dv encodings.sv, compare_helper
        // top.sv → "expected expression, found Assign" palsu).
        let name_end = s
            .find(|c: char| c == '(' || c.is_whitespace())
            .unwrap_or(s.len());
        let function_open = (s.as_bytes().get(name_end) == Some(&b'(')).then_some(name_end);
        let (name, params, value, defaults) = if let Some(open_paren) = function_open {
            let name = s[..open_paren].trim().to_string();
            // Penutup param-list STRING-AWARE: `define dv_fatal(MSG_,
            // ID_ = $sformatf("%m")) $fatal(...)` — `)` di dalam string/
            // $sformatf("%m") TIDAK penutup; naive .find(')') memotong
            // param-list di `$sformatf("%m")` → body bocor `) $fatal(...)`
            // → `if (_ready) ) $fatal(...)` (sw_logger_if, E1002).
            let close_paren = {
                let b = s.as_bytes();
                let mut d = 0usize;
                let mut in_str = false;
                let mut esc = false;
                let mut end = s.len();
                for (k, &c) in b.iter().enumerate().skip(open_paren) {
                    if in_str {
                        if esc {
                            esc = false;
                        } else if c == b'\\' {
                            esc = true;
                        } else if c == b'"' {
                            in_str = false;
                        }
                        continue;
                    }
                    match c {
                        b'"' => in_str = true,
                        b'(' => d += 1,
                        b')' => {
                            d = d.saturating_sub(1);
                            if d == 0 {
                                end = k;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                end
            };
            let params_str = if open_paren < close_paren && close_paren <= s.len() {
                &s[open_paren + 1..close_paren]
            } else {
                ""
            };
            let mut params: Vec<String> = Vec::new();
            let mut defaults: Vec<String> = Vec::new();
            for p in split_args_string_aware(params_str, 0) {
                let p = p.trim();
                if p.is_empty() {
                    continue;
                }
                match p.find('=') {
                    Some(eq) => {
                        // `` `define M(a, b = 2) `` → param "b", default "2".
                        // Stripping default mencegah param `b = 2` tidak pernah
                        // cocok di body → literal + paste backtick bocor.
                        let name = p[..eq].trim().to_string();
                        let default = p[eq + 1..].trim().to_string();
                        params.push(name);
                        defaults.push(default);
                    }
                    None => {
                        params.push(p.to_string());
                        defaults.push(String::new());
                    }
                }
            }
            let value = if close_paren < s.len() {
                Self::strip_macro_comments(s[close_paren + 1..].trim())
            } else {
                String::new()
            };
            (name, params, value, defaults)
        } else {
            let end = s.find(|c: char| c.is_whitespace()).unwrap_or(s.len());
            let name = s[..end].to_string();
            let value = Self::strip_macro_comments(s[end..].trim());
            (name, Vec::new(), value, Vec::new())
        };

        self.defines.insert(
            name,
            MacroDef {
                value,
                params,
                defaults,
            },
        );
    }

    fn strip_macro_comments(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut chars = text.chars().peekable();
        let mut in_string = false;
        let mut escaped = false;
        while let Some(c) = chars.next() {
            if in_string {
                out.push(c);
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    in_string = false;
                }
                continue;
            }
            if c == '"' {
                in_string = true;
                out.push(c);
            } else if c == '/' && chars.peek() == Some(&'/') {
                chars.next();
                for next in chars.by_ref() {
                    if next == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            } else if c == '/' && chars.peek() == Some(&'*') {
                chars.next();
                let mut prev = '\0';
                for next in chars.by_ref() {
                    if next == '\n' {
                        out.push('\n');
                    }
                    if prev == '*' && next == '/' {
                        break;
                    }
                    prev = next;
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    /// Evaluate `ifdef/`ifndef expression. Supports:
    ///   MACRO                — true if defined
    ///   MACRO_A || MACRO_B  — true if any defined
    ///   MACRO_A && MACRO_B  — true if all defined
    fn eval_ifdef_expr(&self, expr: &str) -> bool {
        let expr = expr.trim();
        if expr.is_empty() {
            return false;
        }
        if let Some((a, b)) = expr.split_once("||") {
            let a = a.trim();
            let b = b.trim();
            let left = if a.contains("&&") {
                a.split("&&").all(|m| self.defines.contains_key(m.trim()))
            } else {
                self.defines.contains_key(a)
            };
            let right = if b.contains("&&") {
                b.split("&&").all(|m| self.defines.contains_key(m.trim()))
            } else {
                self.defines.contains_key(b)
            };
            return left || right;
        }
        if let Some((a, b)) = expr.split_once("&&") {
            return self.defines.contains_key(a.trim()) && self.defines.contains_key(b.trim());
        }
        self.defines.contains_key(expr)
    }

    /// Expand inline macros in a single line (wrapper with depth tracking).
    fn expand_inline_macros(&self, line: &str) -> String {
        self.expand_inline_macros_depth(line, 0)
    }

    /// Fold direktif kondisional (`` `ifdef/`ifndef/`elsif/`else/`endif ``) yang
    /// muncul DI DALAM hasil ekspansi macro — bukan di awal baris file (yang
    /// sudah ditangani loop utama line-based). Body macro SV boleh memuat
    /// direktif (mis. ASSERT_ERROR di prim_assert.sv memuat `` `ifdef UVM ...
    /// `else ... `endif ``); setelah ekspansi, teks direktif menyatu ke baris
    /// panggil. Tanpa fold, direktif jadi teks polos → parser rusak dan blok
    /// ifdef tak pernah ditutup. Stack LOKAL (scope macro): direktif dalam body
    /// macro dievaluasi dengan defines saat ini, independen dari cond_stack
    /// file — sesuai LRM 1800 §22.5.1 (direktif di-scan ulang setelah
    /// substitusi macro).
    fn fold_expanded_conditionals(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut stack: Vec<CondFrame> = Vec::new();
        let mut rest = text;
        while let Some(bt) = rest.find('`') {
            // Cari direktif berikutnya: backtick + nama direktif di token boundary.
            let after = &rest[bt + 1..];
            let name_len = after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .map(char::len_utf8)
                .sum::<usize>();
            let name = &after[..name_len];
            if !is_cond_directive(name) {
                // Bukan direktif kondisional — biarkan (macro lain / literal).
                out.push_str(&rest[..bt + 1]);
                rest = after;
                continue;
            }
            // Segmen sebelum direktif: emisi sesuai state saat ini.
            if stack.iter().all(|f| f.taking_branch) {
                out.push_str(&rest[..bt]);
            }
            let after_dir = &after[name_len..];
            match name {
                "ifdef" | "ifndef" => {
                    // Arg direktif: bila ada newline → satu baris penuh;
                    // tanpa newline (body macro joined) → token pertama.
                    let arg_text = after_dir.trim_start();
                    let (expr, rest_of_line) = match arg_text.find('\n') {
                        Some(nl) => (&arg_text[..nl], &arg_text[nl..]),
                        None => {
                            let end = arg_text
                                .find(|c: char| c.is_whitespace())
                                .unwrap_or(arg_text.len());
                            (&arg_text[..end], &arg_text[end..])
                        }
                    };
                    let defined = self.eval_ifdef_expr(expr.trim());
                    let taking = if name == "ifdef" { defined } else { !defined };
                    stack.push(CondFrame {
                        taking_branch: taking,
                        branch_taken: taking,
                    });
                    rest = rest_of_line;
                }
                "elsif" => {
                    match stack.last_mut() {
                        Some(frame) => {
                            if frame.branch_taken {
                                frame.taking_branch = false;
                                // lanjutkan scan SETELAH direktif — tanpa ini
                                // `rest` tidak pernah maju → infinite loop
                                // (ditemukan mivon-fuzz: `// `elsif FZ` dalam
                                // komentar hang preprocessor).
                                rest = after_dir;
                            } else {
                                let arg_text = after_dir.trim_start();
                                let (expr, rest_of_line) = match arg_text.find('\n') {
                                    Some(nl) => (&arg_text[..nl], &arg_text[nl..]),
                                    None => {
                                        let end = arg_text
                                            .find(|c: char| c.is_whitespace())
                                            .unwrap_or(arg_text.len());
                                        (&arg_text[..end], &arg_text[end..])
                                    }
                                };
                                if self.eval_ifdef_expr(expr.trim()) {
                                    frame.taking_branch = true;
                                    frame.branch_taken = true;
                                }
                                rest = rest_of_line;
                            }
                        }
                        // `elsif tanpa `ifdef dalam teks yang di-fold (mis.
                        // `// `elsif` di komentar biasa) — jangan hang; lewati.
                        None => {
                            rest = after_dir;
                        }
                    }
                }
                "else" => {
                    if let Some(frame) = stack.last_mut() {
                        if frame.branch_taken {
                            frame.taking_branch = false;
                        } else {
                            frame.taking_branch = true;
                            frame.branch_taken = true;
                        }
                    }
                    rest = after_dir;
                }
                "endif" => {
                    stack.pop();
                    rest = after_dir;
                }
                _ => unreachable!("is_cond_directive guard"),
            }
        }
        if stack.iter().all(|f| f.taking_branch) {
            out.push_str(rest);
        }
        out
    }

    /// Expand inline macros with recursive depth tracking.
    /// to prevent infinite recursion from circular macro definitions.
    fn expand_inline_macros_depth(&self, line: &str, depth: usize) -> String {
        if depth >= MAX_MACRO_EXPANSION_DEPTH || line.len() > MAX_MACRO_EXPANSION_LEN {
            return line.to_string();
        }

        let mut result = String::new();
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                // Salin komentar `//...` sampai akhir baris (atau akhir string),
                // lalu lanjutkan scan ke baris berikutnya — JANGAN blind-copy
                // sisa string. Pada string multi-baris hasil penggabungan
                // argumen macro (mis. body `DV_SPINWAIT_EXIT` yang memuat `//`
                // sebelum `` `DV_CHECK_EQ ``), blind-copy sisa membuat backtick
                // macro di baris lanjutan bocor ke output → lexical error E1002.
                let mut j = i;
                while j < bytes.len() && bytes[j] != b'\n' {
                    j += 1;
                }
                if j < bytes.len() {
                    // Salin komentar (tanpa newline); loop lanjut dari '\n'.
                    result.push_str(&line[i..j]);
                    i = j;
                } else {
                    // Baris tunggal / akhir string — salin sisa verbatim.
                    result.push_str(&line[i..]);
                    break;
                }
                continue;
            }
            // Stringify langsung penggunaan (di luar body macro): `` `"X`" `` →
            // `"X"`. Undef yang menghasilkan `` `"path`" `` (mis. pemakaian di
            // $assertoff) harus jadi string literal, bukan backtick sisa.
            if bytes[i] == b'`' && i + 1 < bytes.len() && bytes[i + 1] == b'"' {
                result.push('"');
                i += 2;
                continue;
            }
            if bytes[i] == b'`'
                && i + 1 < bytes.len()
                && (bytes[i + 1].is_ascii_alphabetic() || bytes[i + 1] == b'_')
            {
                i += 1;
                // Token-paste run: `` `NAME` `` / `` ``NAME`` `` — konsumsi SEMUA
                // backtick di depan nama biar tidak ada sisa ` di output
                // (mis. `PARAM_NAME_``_MINSTANDARD` → `TLOW_MINSTANDARD`).
                while i < bytes.len() && bytes[i] == b'`' {
                    i += 1;
                }
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let name = &line[start..i];
                // Direktif kondisional bawaan body macro (`` `ifdef/`else/`endif
                // `` — pola ASSERT_ERROR prim_assert.sv). JANGAN perlakukan
                // sebagai macro tak dikenal: backtick yang di-strip membuat
                // direktif jadi teks polos → modul rusak + ifdef tak tertutup
                // (~586 file RTL OpenTitan: "skipping top-level construct:
                // Hash"). Pertahankan backtick; `fold_expanded_conditionals`
                // yang mengevaluasinya.
                if is_cond_directive(name) {
                    result.push('`');
                    result.push_str(name);
                    continue;
                }
                // `MACRO (args)` — spasi antara nama macro dan `(` sah di SV
                // (`uvm_info (gfn, ...)` pola umum). Tanpa skip, args terdeteksi
                // KOSONG → substitusi body jadi `uvm_report_info(, , )` dan
                // `(gfn, ...)` bocor ke output → "expected RParen, found Comma".
                while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                    i += 1;
                }
                if let Some(mdef) = self.defines.get(name) {
                    // Batas ukuran hasil: ekspansi eksponensial (`define A `A `A`)
                    // menggandakan output tiap level — input kecil selalu, jadi
                    // guard di awal fungsi tidak pernah kena. Cek akumulasi result.
                    if result.len() > MAX_MACRO_EXPANSION_LEN {
                        return result;
                    }
                    if mdef.params.is_empty() {
                        let expanded = self.expand_inline_macros_depth(&mdef.value, depth + 1);
                        result.push_str(&expanded);
                        if result.len() > MAX_MACRO_EXPANSION_LEN {
                            return result;
                        }
                    } else {
                        let args = if i < bytes.len() && bytes[i] == b'(' {
                            let args_start = i + 1;
                            let mut paren_depth = 1;
                            // String-aware: `)`/`(` di dalam string literal TIDAK
                            // menghitung. `dv_fatal("...calling ready()")` —
                            // tanpa ini arg terpotong di `)` dalam string,
                            // sisa `)"` bocor → `if (_ready) ) $fatal(...)`
                            // (sw_logger_if.sv, E1002 expected expression).
                            // Komentar `//...` juga di-skip (arg macro bisa
                            // memuat komentar di baris lanjutan).
                            let mut in_string = false;
                            let mut escaped = false;
                            let mut args_end = args_start;
                            while args_end < bytes.len() && paren_depth > 0 {
                                let c = bytes[args_end];
                                if in_string {
                                    if escaped {
                                        escaped = false;
                                    } else if c == b'\\' {
                                        escaped = true;
                                    } else if c == b'"' {
                                        in_string = false;
                                    }
                                } else if c == b'"' {
                                    in_string = true;
                                } else if c == b'/'
                                    && args_end + 1 < bytes.len()
                                    && bytes[args_end + 1] == b'/'
                                {
                                    // Skip komentar sampai newline.
                                    while args_end < bytes.len() && bytes[args_end] != b'\n' {
                                        args_end += 1;
                                    }
                                    continue;
                                } else if c == b'(' {
                                    paren_depth += 1;
                                } else if c == b')' {
                                    paren_depth -= 1;
                                }
                                args_end += 1;
                            }
                            let args_str = if args_end > args_start {
                                // Baris bisa berakhir tepat setelah '(' (input
                                // truncated) — args_end-1 < args_start = panic.
                                &line[args_start..args_end - 1]
                            } else {
                                ""
                            };
                            i = args_end;
                            self.split_macro_args(args_str, mdef.params.len())
                        } else {
                            Vec::new()
                        };
                        let mut expanded_args: Vec<String> = args
                            .iter()
                            .map(|arg| self.expand_inline_macros_depth(arg, depth + 1))
                            .collect();
                        // Isi arg yang TIDAK dikirim saat pemanggilan dengan
                        // default param-nya (SV): `` `define M(a, b = 2) ``
                        // dipanggil `M(x)` → b diisi "2". Bila tanpa default →
                        // kosong. Tanpa ini param bocor literal/paste backtick
                        // (`top_darjeeling``pd_hier``.u_...`).
                        // Arg KOSONG (`, ,` di pemanggil — pola DV_*}
                        // `DV_CHECK_FATAL(x, , MsgId)`) JUGA memakai default:
                        // LRM 1800 §22.5.1 — argumen kosong = use default.
                        for k in 0..mdef.params.len() {
                            let is_empty = expanded_args
                                .get(k)
                                .map(|s| s.trim().is_empty())
                                .unwrap_or(true);
                            if k >= expanded_args.len() || is_empty {
                                let d = mdef
                                    .defaults
                                    .get(k)
                                    .map(|s| self.expand_inline_macros_depth(s, depth + 1))
                                    .unwrap_or_default();
                                if k >= expanded_args.len() {
                                    expanded_args.push(d);
                                } else {
                                    expanded_args[k] = d;
                                }
                            }
                        }
                        // Substitute parameters with expanded arguments — single pass on bytes
                        let mut expanded = String::with_capacity(mdef.value.len());
                        let val_bytes = mdef.value.as_bytes();
                        let mut pos = 0;
                        while pos < val_bytes.len() {
                            let mut matched = false;
                            // Stringify operator `` `"param`" `` (IEEE 1800) di dalam
                            // body macro → `"arg"`. Tanpa ini ASSERT_ERROR prim_assert
                            // (`PRIM_STRINGIFY(__name)` = `` `"__name`" ``) meninggalkan
                            // backtick literal di output → "expected expression, found
                            // End" saat parse blok `else begin ... end` dari ASSERT_I.
                            if val_bytes[pos] == b'`'
                                && pos + 1 < val_bytes.len()
                                && val_bytes[pos + 1] == b'"'
                            {
                                let qpos = pos + 2;
                                for (param, arg) in mdef.params.iter().zip(expanded_args.iter()) {
                                    if !param.is_empty()
                                        && qpos + param.len() <= val_bytes.len()
                                        && &val_bytes[qpos..qpos + param.len()] == param.as_bytes()
                                    {
                                        let after = qpos + param.len();
                                        if after + 1 < val_bytes.len()
                                            && val_bytes[after] == b'`'
                                            && val_bytes[after + 1] == b'"'
                                        {
                                            // Arg di-quote; ` dan " di dalam arg di-strip
                                            // agar string literal selalu valid.
                                            let cleaned: String = arg
                                                .chars()
                                                .filter(|c| *c != '`' && *c != '"')
                                                .collect();
                                            expanded.push('"');
                                            expanded.push_str(&cleaned);
                                            expanded.push('"');
                                            pos = after + 2;
                                            matched = true;
                                            break;
                                        }
                                    }
                                }
                                if matched {
                                    continue;
                                }
                            }
                            for (param, arg) in mdef.params.iter().zip(expanded_args.iter()) {
                                if !param.is_empty()
                                    && pos + param.len() <= val_bytes.len()
                                    && &val_bytes[pos..pos + param.len()] == param.as_bytes()
                                {
                                    // Token-boundary: param satu huruf (`i`, `j`)
                                    // TIDAK boleh match di dalam kata literal
                                    // (`if`, `begin`, `sim` — bug FORCE_OTP_PART
                                    // _LOCK_WITH_RAND_NON_MUBI_VAL di otp_ctrl_if:
                                    // `if` → `i`+`f` jadi ter-paste). Batas: karakter
                                    // sebelum/selepas param bukan ident-char.
                                    let p_end = pos + param.len();
                                    let before_ok =
                                        pos == 0 || !val_bytes[pos - 1].is_ascii_alphanumeric();
                                    let after_ok = p_end >= val_bytes.len()
                                        || !val_bytes[p_end].is_ascii_alphanumeric();
                                    if before_ok && after_ok {
                                        expanded.push_str(arg);
                                        pos += param.len();
                                        // Paste sufiks: `PARAM_``_SUFFIX` — backtick
                                        // langsung setelah param diikuti alpha/_ →
                                        // buang run, rekatkan ke teks berikutnya
                                        // (I2C_GET_MIN_PARAM: `PARAM_NAME_``_MINSTANDARD`).
                                        // Dot juga: `PARAM``.member` (pola
                                        // ASSERT_IBEX_CORE_ERROR_TRIGGER_ALERT di
                                        // rv_core_ibex autogen) — paste ke dot.
                                        if pos < val_bytes.len() && val_bytes[pos] == b'`' {
                                            let mut r = pos;
                                            while r < val_bytes.len() && val_bytes[r] == b'`' {
                                                r += 1;
                                            }
                                            if r < val_bytes.len()
                                                && (val_bytes[r].is_ascii_alphabetic()
                                                    || val_bytes[r] == b'_'
                                                    || val_bytes[r] == b'.')
                                            {
                                                pos = r;
                                            }
                                        }
                                        matched = true;
                                        break;
                                    }
                                }
                            }
                            // OpenTitan token-paste extension dalam body macro:
                            // `` `NAME` `` / `` ``NAME`` `` (backtick mengapit
                            // param) → strip semua backtick, paste arg polos.
                            // Berbeda dari `` `"p`" `` stringify. Contoh pembangkit
                            // (otbn.sv): `` `define DEF_FAC_BIT(NAME) ...``NAME``... ``
                            if !matched && val_bytes[pos] == b'`' {
                                let mut k = pos;
                                while k < val_bytes.len() && val_bytes[k] == b'`' {
                                    k += 1;
                                }
                                let mut param_matched = false;
                                for (param, arg) in mdef.params.iter().zip(expanded_args.iter()) {
                                    if !param.is_empty()
                                        && k + param.len() <= val_bytes.len()
                                        && &val_bytes[k..k + param.len()] == param.as_bytes()
                                    {
                                        let after = k + param.len();
                                        let mut a2 = after;
                                        while a2 < val_bytes.len() && val_bytes[a2] == b'`' {
                                            a2 += 1;
                                        }
                                        expanded.push_str(arg);
                                        pos = a2;
                                        matched = true;
                                        param_matched = true;
                                        break;
                                    }
                                }
                                // `` `prefix``PARAM `` — run backtick diikuti literal
                                // non-param (PREFIX) yang lalu di-paste ke param
                                // (`` `` value args OpenTitan dv_macros, mis.
                                // `` `dv_``SEV_ `` → `dv_` + SEV_==error → `dv_error`).
                                // Backtick terdepan (sebelum PREFIX) HARUS dibuang,
                                // bukan dikopi literal — kalau tidak bocor ` di output
                                // dan FastLexer memuntahkan lexical error E1002.
                                if !param_matched {
                                    let mut prefix_end = k;
                                    while prefix_end < val_bytes.len()
                                        && (val_bytes[prefix_end].is_ascii_alphanumeric()
                                            || val_bytes[prefix_end] == b'_'
                                            || val_bytes[prefix_end] == b'.')
                                    {
                                        prefix_end += 1;
                                    }
                                    let mut p2 = prefix_end;
                                    while p2 < val_bytes.len() && val_bytes[p2] == b'`' {
                                        p2 += 1;
                                    }
                                    for (param, arg) in mdef.params.iter().zip(expanded_args.iter())
                                    {
                                        if !param.is_empty()
                                            && p2 + param.len() <= val_bytes.len()
                                            && &val_bytes[p2..p2 + param.len()] == param.as_bytes()
                                        {
                                            let after = p2 + param.len();
                                            let mut a2 = after;
                                            while a2 < val_bytes.len() && val_bytes[a2] == b'`' {
                                                a2 += 1;
                                            }
                                            // Prefix sebelum paste bisa memuat macro
                                            // BERTINGKAT yang sudah didefinisikan dan
                                            // harus di-expand, mis. OpenTitan:
                                            // `` `define SPI_HOST_HIER(i) `PD_MAIN_HIER.u_spi_host``i ``
                                            // Prefix = `PD_MAIN_HIER.u_spi_host` — segmen
                                            // pertama `PD_MAIN_HIER` adalah macro.
                                            // Expand segmen tersebut (recursive) lalu
                                            // lanjutkan sisa prefix + arg paste.
                                            let prefix_str = &mdef.value[k..prefix_end];
                                            let mut prefix_out = String::new();
                                            let mut seg_start = 0usize;
                                            while seg_start < prefix_str.len() {
                                                let dot = prefix_str[seg_start..]
                                                    .find('.')
                                                    .map(|d| seg_start + d)
                                                    .unwrap_or(prefix_str.len());
                                                let seg = &prefix_str[seg_start..dot];
                                                // Segmen pertama (ident polos) dapat
                                                // berupa macro reference.
                                                if seg_start == 0 && !seg.is_empty() {
                                                    if let Some(inner) = self.defines.get(seg) {
                                                        let inner_val = if inner.params.is_empty() {
                                                            self.expand_inline_macros_depth(
                                                                &inner.value,
                                                                depth + 1,
                                                            )
                                                        } else {
                                                            seg.to_string()
                                                        };
                                                        prefix_out.push_str(&inner_val);
                                                    } else {
                                                        prefix_out.push_str(seg);
                                                    }
                                                } else {
                                                    prefix_out.push_str(seg);
                                                }
                                                if dot < prefix_str.len() {
                                                    prefix_out.push('.');
                                                }
                                                seg_start = dot + 1;
                                            }
                                            expanded.push_str(&prefix_out);
                                            expanded.push_str(arg);
                                            pos = a2;
                                            matched = true;
                                            break;
                                        }
                                    }
                                }
                            }
                            if !matched {
                                expanded.push(val_bytes[pos] as char);
                                pos += 1;
                            }
                        }
                        let expanded = self.expand_inline_macros_depth(&expanded, depth + 1);
                        result.push_str(&expanded);
                    }
                } else if name == "__FILE__" || name == "__LINE__" {
                    // Token predefined `` `__FILE__ `` / `` `__LINE__ `` (dipakai
                    // ASSERT_ERROR prim_assert di arg $error) — substitusi
                    // placeholder string agar output preprocessed valid.
                    result.push_str("\"<preprocessed>\"");
                } else {
                    // Makro TIDAK dikenal (mis. `` `uvm_fatal `` / `` `gfn `` saat
                    // uvm_macros tidak include) — strip backtick, sisakan nama +
                    // arg polos. Membuang backtick mencegah FastLexer menganggap
                    // baris sebagai directive dan MENGHAPUS seluruh baris
                    // (`default: `uvm_fatal(...)` → endcase jadi
                    // "expected expression, found Endcase"). Hasilnya panggilan
                    // polos `uvm_fatal(...)` yang tetap parse-able.
                    result.push_str(name);
                }
            } else {
                if bytes[i] == b'`'
                    && i + 1 < bytes.len()
                    && (bytes[i + 1].is_ascii_alphanumeric()
                        || bytes[i + 1] == b'_'
                        || bytes[i + 1] == b'`')
                    && i > 0
                    && (bytes[i - 1].is_ascii_alphanumeric()
                        || bytes[i - 1] == b'_'
                        || bytes[i - 1] == b'`')
                {
                    // Paste marker antar token literal (mis. `0``0``0` dari
                    // coverpoint macro generate) — bukan operator SV, buang.
                    // Hanya saat diapit karakter alnum/paste (bukan directive).
                    i += 1;
                } else {
                    result.push(bytes[i] as char);
                    i += 1;
                }
            }
        }
        result
    }

    fn split_macro_args(&self, args_str: &str, expected_count: usize) -> Vec<String> {
        split_args_string_aware(args_str, expected_count)
    }
}

impl Default for Preprocessor {
    fn default() -> Self {
        Self::new()
    }
}

/// String-aware top-level comma splitter, dipakai untuk memisahkan:
/// - argumen invokasi macro (`` `MACRO(a, b, c) ``) — koma di dalam string
///   literal (`"x, y"`), di dalam paren bersarang, ATAU di dalam komentar
///   (`//` / `/* */`) TIDAK memisahkan arg.
/// - daftar parameter `define (LRM 1800 §22.5.1) — default bernilai string
///   yang memuat koma (mis. `A="x,y"`) tetap satu param penuh.
/// Koma yang TIDAK memisahkan juga tetap dipertahankan di token aslinya.
/// KOMENTAR di-skip penuh: arg macro multi-baris (mis. `DV_SPINWAIT_EXIT`
///   dengan `i = 0; // restart the delay, since ...`) membawa koma di dalam
///   komentar — tanpa skip, koma itu memutus argumen → ekspansi korup
///   (arg bergeser: MSG_ dapat nilai EXIT_, error "expected expression,
///   found Wait" di ratusan file DV OpenTitan).
fn split_args_string_aware(args_str: &str, expected_count: usize) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut brace_depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut chars = args_str.chars().peekable();
    while let Some(c) = chars.next() {
        if in_string {
            current.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        // Komentar `//` — skip sampai akhir baris (arg macro multi-baris
        // memuat komentar; koma di dalamnya bukan pemisah arg). Teks komentar
        // DI-BUANG (diganti spasi) — komentar SV tidak punya semantik: arg
        // yang memuat `int_err == 0; // catatan \n }` TIDAK boleh membawa `//`
        // ke hasil substitusi (komentar pada baris hasil ekspansi akan MEMAKAN
        // penutup `}`/`)` di baris yang sama → `with {` tak tertutup → parse
        // desync. Ganti spasi utk menjaga batas token (`a//x b` ≠ ident `ab`).
        if c == '/' && chars.peek() == Some(&'/') {
            chars.next(); // consume second '/'
            for n in chars.by_ref() {
                if n == '\n' {
                    current.push(' ');
                    break;
                }
            }
            continue;
        }
        // Komentar `/* ... */` — skip sampai `*/`. DI-BUANG (ganti spasi).
        if c == '/' && chars.peek() == Some(&'*') {
            chars.next(); // consume '*'
            let mut prev = None;
            for n in chars.by_ref() {
                if prev == Some('*') && n == '/' {
                    break;
                }
                prev = Some(n);
            }
            current.push(' ');
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                current.push(c);
            }
            '(' => {
                depth += 1;
                current.push(c);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(c);
            }
            // Argumen macro bisa memuat `{ ... }` dengan koma di dalamnya
            // (mis. `ASSERT_INIT(N, Width inside {25, 50, 100})` prim_keccak
            // atau constraint dist `{0 :/ 5, [1:10] :/ 5}`). Tanpa depth
            // kurung kurawal, koma dalam `{...}` memecah ARGUMEN → ekspansi
            // korup ("expected RBrace, found RParen" di ratusan assert
            // inside/constraint OpenTitan).
            '{' => {
                brace_depth += 1;
                current.push(c);
            }
            '}' => {
                brace_depth = brace_depth.saturating_sub(1);
                current.push(c);
            }
            ',' if depth == 0 && brace_depth == 0 => {
                args.push(current.trim().to_string());
                current.clear();
            }
            _ => {
                current.push(c);
            }
        }
    }
    let last = current.trim().to_string();
    if !last.is_empty() || args.len() < expected_count {
        args.push(last);
    }
    args
}

/// True jika baris berisi invokasi macro (`` `name(...) ``) yang parennya belum
/// seimbang. Dipakai untuk menggabungkan baris-baris lanjutan argumen macro.
fn unbalanced_macro_call(line: &str) -> bool {
    let mut paren_depth = 0i64;
    let mut macro_open = false;
    // Proses per baris: komentar `//` hanya menghentikan scan PADA baris itu
    // (baris lanjutan yang di-join tetap di-scan). Sebelumnya `break` di
    // komentar menghentikan scan seluruh string join → paren yang belum
    // ditutup di baris 1 membuat join menelan SEMUA baris berikutnya sampai
    // EOF (mis. `ASSERT(...)` multiline yang punya komentar di tengah argumen
    // di prim_diff_decode.sv) — endmodule ikut hilang.
    for l in line.split('\n') {
        let bytes = l.as_bytes();
        let mut in_string = false;
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i];
            if c == b'"' && !in_string {
                in_string = true;
                i += 1;
                continue;
            }
            if c == b'"' && in_string {
                // handle \" escape
                if i > 0 && bytes[i - 1] == b'\\' {
                    i += 1;
                    continue;
                }
                in_string = false;
                i += 1;
                continue;
            }
            if in_string {
                i += 1;
                continue;
            }
            // '//' comment ends scan for THIS line only
            if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                break;
            }
            if c == b'`' {
                macro_open = true;
            } else if c == b'(' {
                paren_depth += 1;
            } else if c == b')' {
                paren_depth -= 1;
            }
            i += 1;
        }
    }
    macro_open && paren_depth > 0
}

/// True jika `\` di akhir baris adalah line-continuation sungguhan — yaitu
/// berada di LUAR comment (`//`, `/* */`) dan string. `\` yang berada DI
/// DALAM comment/string TIDAK meneruskan baris (baris berikutnya tetap baris
/// terpisah, komentar berakhir di newline). Ini perilaku yang dipakai tool
/// reference (Verilator/VCS) oleh kode OpenTitan: aon_osc.sv menaruh `\`
/// di akhir komentar `// ... input\` supaya `` `endif `` baris berikutnya
/// tetap directive nyata — jika `\`-in-comment dianggap continuation, ifdef
/// menjadi tak seimbang dan module aon_osc hilang dari design.
fn trailing_backslash_is_continuation(line: &str) -> bool {
    let bytes = line.as_bytes();
    // Cari `\` terakhir yang bukan whitespace-trailing.
    let mut end = line.len();
    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    if end == 0 || bytes[end - 1] != b'\\' {
        return false;
    }
    // Scan sampai posisi `\` (exclusive), lacak state comment/string.
    let mut in_block = false;
    let mut in_line = false;
    let mut in_string = false;
    let mut j = 0;
    while j < end {
        let c = bytes[j];
        if in_block {
            if c == b'*' && j + 1 < end && bytes[j + 1] == b'/' {
                in_block = false;
                j += 2;
            } else {
                j += 1;
            }
            continue;
        }
        if in_string {
            if c == b'\\' && j + 1 < end {
                j += 2;
            } else {
                if c == b'"' {
                    in_string = false;
                }
                j += 1;
            }
            continue;
        }
        if c == b'/' && j + 1 < end && bytes[j + 1] == b'/' {
            in_line = true;
            break;
        }
        if c == b'/' && j + 1 < end && bytes[j + 1] == b'*' {
            in_block = true;
            j += 2;
            continue;
        }
        if c == b'"' {
            in_string = true;
        }
        j += 1;
    }
    !in_line && !in_block && !in_string
}

#[cfg(test)]
mod tests {
    use super::Preprocessor;

    /// F84 (LRM §19.8): `` `timescale `` berlaku ke module SESUDAH directive
    /// dan berubah tiap directive baru — bukan properti global. Tanpa ini
    /// `#5` di module `1us` ikut diskalakan dengan satuan `1ns`.
    #[test]
    fn timescale_segments_are_per_directive() {
        let src = "`timescale 1ns/1ps\nmodule fast; endmodule\n\
                   `timescale 1us/1ns\nmodule slow; endmodule\n";
        let mut pp = Preprocessor::new();
        pp.preprocess(src, None).unwrap();
        assert_eq!(
            pp.timescale_segments.len(),
            2,
            "dua directive → dua segmen: {:?}",
            pp.timescale_segments
        );
        // Directive tidak ikut output, jadi `module fast` = baris output 1 dan
        // `module slow` = baris output 2.
        assert_eq!(
            pp.timescale_at_line(1).map(|(u, _)| u.as_str()),
            Some("1ns"),
            "module fast ikut 1ns"
        );
        assert_eq!(
            pp.timescale_at_line(2).map(|(u, _)| u.as_str()),
            Some("1us"),
            "module slow ikut 1us"
        );
        // Baris sebelum directive pertama = belum ada directive.
        assert!(
            pp.timescale_segments.iter().all(|(f, _)| *f >= 1),
            "segmen mulai baris output >= 1"
        );
    }

    /// Directive di cabang `ifdef yang TIDAK diemitting tak berlaku (LRM §19.8:
    /// hanya directive yang lolos preprocessing).
    #[test]
    fn timescale_in_dead_ifdef_branch_is_ignored() {
        let src = "`ifdef NEVER\n`timescale 1us/1ns\n`endif\nmodule m; endmodule\n";
        let mut pp = Preprocessor::new();
        pp.preprocess(src, None).unwrap();
        assert!(
            pp.timescale_segments.is_empty(),
            "directive di cabang mati jangan dicatat: {:?}",
            pp.timescale_segments
        );
    }

    /// Regresi mivon-fuzz (kampanye sim seed 9999, bug_0004 hang uart_tx):
    /// `` `elsif `` tanpa `` `ifdef `` (di sini: di dalam komentar `//`) membuat
    /// `fold_expanded_conditionals` loop selamanya (rest tak pernah maju).
    /// Preprocess harus selesai — bukan hang.
    #[test]
    fn elsif_in_comment_no_hang() {
        let mut pp = Preprocessor::new();
        let out = pp
            .preprocess(
                "module m;\n  // `elsif FZ\n  initial $finish;\nendmodule\n",
                None,
            )
            .unwrap();
        assert!(
            out.contains("initial $finish"),
            "output harus memuat kode modul: {out}"
        );
    }

    /// `elsif setelah `ifdef branch_taken: `rest` harus maju (fix kedua pada
    /// arm yang sama) — preprocess selesai dan cabang kedua benar di-skip.
    #[test]
    fn elsif_after_taken_ifdef_advances() {
        let mut pp = Preprocessor::new();
        pp.preprocess("`define FZ_YES 1\n", None).unwrap();
        let out = pp
            .preprocess(
                "`ifdef FZ_YES\nmodule a; endmodule\n`elsif FZ_NO\nmodule b; endmodule\n`endif\n",
                None,
            )
            .unwrap();
        assert!(
            out.contains("module a"),
            "branch pertama harus ter-emisi: {out}"
        );
        assert!(
            !out.contains("module b"),
            "branch elsif setelah ifdef diambil harus di-skip: {out}"
        );
    }
}
