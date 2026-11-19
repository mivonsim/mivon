# MIVON EDA ECOSYSTEM & GUI GAP ANALYSIS

**Tanggal:** 19 Nov 2026  
**Versi Mivon:** 0.3.0 (workspace `crates/`, binary `mivon`, GUI `mivon gui`/ratatui TUI)  
**Sumber bukti:** inspeksi sumber kode Aktual (crates/, src/, Cargo.toml, tools.md, gui.md, tes workspace)  
**Status build/tes:** `cargo build --workspace` lolos; `cargo test --workspace` 0 gagal, 0 panic

---

## 1. EXECUTIVE SUMMARY

Mivon adalah simulator RTL SystemVerilog berbasis Rust dengan pipeline: preprocessor → lexer → parser → AST → elaborator → IR → simulator engine → VCD/FST. Terdiri dari 20+ crate workspace (`mivon-parser`, `mivon-elaboration`, `mivon-simulator`, `mivon-compiler`, `mivon-gui`, `mivon-tools`, `mivon-env`, `mivon-node-cache`, dll.).

### Kesalahan besar dari audit ini

1. **GUI bukan GUI grafis.** `mivon-gui` adalah **ratatui TUI terminal** (tanpa GPU, tanpa window, tanpa docking grafis). `gui.md` adalah dokumen konsep/keinginan, bukan implementasi. Banyak fitur yang tercantum di `gui.md` (semantic highlighting, go-to-definition nyata, refactoring, snippets, hover, signature help, multi-cursor, diff editor, split editor) **tidak ada** di implementasi TUI. Editor TUI saat ini adalah viewer teks sederhana (mode Normal/Insert, undo/redo cap 100 snapshot, scroll) tanpa syntax highlighting semantik.

2. **Tidak ada model project terpadu yang terpercaya.** GUI membuka direktori dan memindai `.sv`/`.svh` secara heuristik; tidak ada project file `.mivon` bawaan GUI dengan konfigurasi compile/simulasi tersimpan, tidak ada multiple targets/configurations, tidak ada project import/export/cloning terstruktur. Compile dilakukan per-daerah file yang dipindai, tanpa manajemen dependensi antar project.

3. **Simulasi GUI adalah "run sekali hingga selesai".** Tidak ada pause/resume/step/reset/restart dari UI (hanya F5 run + F5 stop + F7 compile). Debugger hanya breakpoints/watchpoints sebagai string teks (bp cycle N / bp change sig / wp sig), tanpa conditional breakpoint nyata, call stack, variable inspector, source-to-signal mapping, atau reverse debugging meski engine pun snapshot state.

4. **Waveform viewer sangat primitif.** Render karakter terminal (█/░/▧/▦), satu sel per interval waktu, tanpa zoom/pan/kurzor/time measurement yang layak, tanpa bus expansion/collapse, tanpa trigger navigation, tanpa overlay multi-run yang berfungsi nyata (hanya `prev_waveform` disimpan, render compare belum ada).

5. **Tidak ada fitur EDA profesional standar:** formal verification terintegrasi GUI, synthesis/timing analysis, lint bawaan GUI yang terpadu dengan editor, coverage source-level highlighting, regression manager, test generation otomatis, fuzzing GUI, CI/CD integrasi, plugin/extension API, multi-window, session restoration yang kaya.

6. **Arsitektur monoputih GUI.** `GuiState` (~830 baris) menumpuk semua state (project, editor, compile, sim, waveform, debug, LSP, terminal, command palette, wizard). Tight coupling antara backend compile/sim dan state UI. Tidak ada job scheduler terpadu, background job framework terpisah, atau event system standar di luar channel mpsc sederhana.

7. **Tidak cocok untuk skala SoC.** Tidak ada data virtualization/lazy loading untuk waveform besar, tidak ada batasan ukuran dump yang terenkapselasi di GUI (hardcoded 256MB di backend), tidak ada indeks sinyal, tidak ada pencarian lintas file yang berindeks, tidak ada incremental update UI saat file berubah.

### Veredik siap pakai

Mivon **belum siap** untuk mendukung alur kerja pengembangan HDL profesional sepenuhnya. Ia berfungsi sebagai simulator CLI untuk desain kecil-menengah (counter, adder, hierarki 3-level, picorv32 RISC-V berhasil disimulasikan). Sebagai EDA environment profesional (GUI terintegrasi, debugging, waveform, project management, tool ecosystem) — **belum ada**.

---

## 2. ARSITEKTUR SAAT INI (VERIFIED)

Lapisan: `src/main.rs` + `src/cli.rs` (binary-only). Semua logika di `crates/`.

| Crate | Tanggung jawab | Ukuran (kira-iran) |
|-------|---------------|-------------------|
| `mivon-parser` | preprocessor, lexer (Pratt), parser | ~3624 LOC lib |
| `mivon-ast` | expr, stmt, types, const_eval, inline | eks |
| `mivon-elaboration` | AST→IR, signal collection, type resolution, loop unroll, const folding | 5886 LOC mod.rs |
| `mivon-ir` | IrStmt, IrExpr, LogicVec | eks |
| `mivon-compiler` | CompileSession, MICD cache, discovery | 3592 LOC |
| `mivon-simulator` | engine, scheduler, debugger, waveform VCD/FST, sdf, parallel, jit | besar |
| `mivon-gui` | ratatui TUI, editor, panels, state, LSP client | eks |
| `mivon-tools` | 11+ CLI tool (minspect, mlint, melab, msim, mcov, mwave, mfmt, mgen, mprof, mcheck, mbench, synth, dll.) | banyak file |
| `mivon-env` | GlobalEnv, 12 konteks (config, workspace, runtime, compiler, database, dll.) | eks |
| `mivon-diag` | DiagLevel, DiagCode, Diagnostic, DiagSink, MICD | pusat diagnos |
| `mivon-node-cache` | filesystem-native cache (BLAKE3 content-addressed) | objek/nodes/graphs/refs/transactions |

GUI layout (app.rs): Toolbar atas (3 baris), Sidebar kiri (30%), Editor tengah (70%), Bottom panel (10 baris). Tidak ada panel docking yang bisa diubah ukurannya secara interaktif (splitter hanya bottom_height).

Debugger engine (types.rs): `DebugMode` {Normal, Debug, DeepDebug}, `Breakpoint` {Cycle, SignalEq, SignalNeq, SignalChange, Module}, `Watchpoint` {Signal, MemAddr}, `StateSnapshot` (time, signals, next_signals, changed), `EventRegion` 13 wilayah IEEE. Reverse debug tersedia di engine (deep-debug), TIDAK terpandu GUI.

Tes: 3414 tes lolos (24 skip) via `cargo nextest run --workspace --features dev` (dokumentasi AGENTS.md). Workspace test saat ini: 0 gagal.

---

## 3. KEEMAMPUAN TERVERIFIKASI (SUDAH ADA, BUKAN HIPOTETIS)

Dibuat dari inspeksi kode langsung:

- Lexer/parser SystemVerilog (module, interface, package, program, class, import pkg::*)
- Elaborator: parameter override, generate block, hierarki, constant folding (`$clog2`, `$bits`, `$size`, dll.)
- Simulator event-driven: 13 region IEEE, NBA commit, multi-driver resolution, inout port, force/release/deassign
- Waveform: VCD writer + parser, FST writer (wavefst), glitch detection window
- Debugger: breakpoint (cycle/signal change/signal eq/module), watchpoint, step_cycle, pause, state snapshot, reverse step, signal history (max 100k/sinyal)
- Coverage: line, toggle, branch, expression, FSM, covergroup (engine + coverage_stats)
- Assertion: assert/assume/cover, assertion_stats (pass/fail per line:col)
- CLI tools: 11+ tool (minspect, mlint, melab, msim, mcov, mwave, mfmt, mgen, mprof, mcheck, mbench, synth, batch, regression, tbgen, waiver, vault, ipxact, design-repo, project, sdc, equiv-check, eco, dashboard, web, license, bugtrack, requirements, tcl)
- LSP server (`mivon --lsp`, tower-lsp, stdio JSON-RPC) — GUI pakai sebagai subprocess
- node_cache: content-addressed filesystem cache (incremental compile, restore AST)
- `.mivon` project file dengan section `[foreign]` (vhpi/pli/dpi libs)
- DPI-C, VHPI, PLI foreign library loading (libloading, optional)
- Formal: Z3 BMC, k-induction, connectivity check, SEC equivalence (fitur `formal`)
- Emulator: MHIR, RV32 interpreter, x86 boot ISO (fitur terpisah)
- Synthesis: SYN check, RTL→SIR, netlist `.mvnet`, tech mapping, timing/area (fitur `synth`)

Belum divalidasi sebagai fitur GUI nyata: sebagian besar di atas hanya tersedia via CLI, tidak via TUI.

---

## 3. GUI FEATURE GAP ANALYSIS (DETAIL)

### 3.1 Main application interface — IMPLEMENTASI TIDAK LENGKAP

| Aspek | Status | Bukti |
|-------|--------|-------|
| Main window structure | Implemented (TUI) | app.rs: layout toolbar(3) + main(horizontal sidebar/editor) + bottom(10) |
| Layout management | Partial | Constraint::Percentage(30/70) fixed; tidak bisa diubah interaktif |
| Panel docking | Missing | Tidak ada dockable/undockable panel |
| Panel resizing | Partial | Hanya `bottom_height` via splitter handle; sidebar fixed 30% |
| Panel visibility | Partial | `show_sidebar`, `show_bottom` boolean toggle saja |
| Workspace customization | Missing | Tidak bisa menyimpan/mengubah layout workspace |
| Multiple workspaces | Missing | Hanya satu project_root |
| Split views | Missing | Tidak ada split editor |
| Tab management | Partial | open_files dengan active_file index, tanpa tab bar visual nyata |
| Multi-window support | Missing | Satu terminal TUI saja |
| Window state restoration | Partial | workspace.json (project_root, recent, bottom_height, sidebar state) |
| Navigation | Partial | Sidebar project tree, search, sidebar tab (Tab/BackTab) |
| Command execution | Partial | Command palette (Ctrl+Shift+P) + F5/F7/F(global) |
| Context menus | Missing | Tidak ada context menu (TUI ratatui tanpa mouse menu) |
| Keyboard shortcuts | Partial | Ctrl+Q/S/O/G, Ctrl+B, F5, F7, Ctrl+Shift+P, Tab, BackTab, arrow |
| Command palette | Implemented | command_palette.rs: 18 aksi nyata, filter, navigasi, execute_selected |
| Quick actions | Missing | Tidak ada quick action toolbar kontekstual |
| Toolbars | Partial | toolbar.rs (satu bar atas) |
| Status bar | Partial | statusbar.rs (ada, butuh inspeksi detail) |
| Breadcrumb navigation | Missing | Tidak ada breadcrumb |
| Notifications | Partial | `state.log()` console, tidak ada notifikasi sistem terpisah |
| Progress indicators | Missing | Tidak ada progress bar untuk compile/sim panjang |
| Modal dialogs | Partial | Command palette modal (Clear) |
| Non-modal dialogs | Missing | Tidak ada dialog non-modal |

**Masalah utama:** Interaksi terlalu banyak langkah (mis. untuk membuka file: Ctrl+O → pilih direktori → sidebar ↑/↓ → Enter). Tidak ada pencarian "search everywhere" terpadu.

### 3.2 HDL editor — PRIMITIF

| Aspek | Status | Bukti |
|-------|--------|-------|
| Syntax highlighting | Missing | editor.rs: menampilkan teks mentah tanpa warna per token (hanya ikon diagnostic merah/kuning) |
| Semantic highlighting | Missing | Tidak ada |
| Language-aware completion | Missing | LSP completion request ada di state (LspPendingKind::CompletionRequest) tapi **tidak pernah dieksekusi/ditampilkan** di editor (app.rs handle LspReply hanya Hover dan Goto, CompletionRequest diabaikan) |
| Auto-completion | Missing | Lihat di atas |
| Symbol completion | Missing | Tidak ada |
| Module/port/parameter/type/macro completion | Missing | Tidak ada |
| Import/package awareness | Partial | LSP didOpen/didChange dikirim, tapi tidak ada import resolution UI |
| Go to definition | Partial | LSP GotoRequest → editor_set_cursor (app.rs), melalui terminal LSP server |
| Find references | Missing | Tidak ada |
| Rename symbol | Missing | Tidak ada |
| Document outline | Partial | outline.rs (ada panel, belum diinspeksi detail) |
| Symbol hierarchy | Missing | Tidak ada |
| Code folding | Missing | Tidak ada |
| Multiple cursors | Missing | Tidak ada |
| Search/replace | Partial | search.rs (tab Search, text hits per file) |
| Regex search | Missing | Tidak diketahui ada di search panel |
| Cross-file search | Missing | Tidak ada |
| Diagnostics | Partial | problems.rs: daftar diagnostic (compile + LSP), klik → buka file |
| Inline errors | Partial | editor.rs menampilkan ikon ✖/⚠/ℹ di sebelah baris |
| Inline warnings | Partial | Lihat di atas |
| Error navigation | Partial | klik baris diagnostic → open_file + pending_goto |
| Code actions/quick fixes | Partial | apply_quick_fix (RemoveLine, BlockingToNonBlocking), hanya 2 jenis |
| Formatting | Missing di editor | mfmt ada CLI, tidak ada format-on-save di editor GUI |
| Lint integration | Partial | mlint ada CLI; GUI `lint_sources` hanya unused signal + blocking assignment |
| Refactoring | Missing | Tidak ada |
| Snippets | Missing | Tidak ada |
| Documentation tooltips | Partial | LSP hover (lsp_hover popup) |
| Hover information | Partial | LSP hover |
| Signature information | Missing | Tidak ada |
| Large-file performance | Unknown | Tidak ada lazy loading / virtualisasi baris editor |
| Multi-file editing | Partial | open_files vec, active_file, tanpa split |
| Split editor | Missing | Tidak ada |
| Diff | Missing | Tidak ada |
| History/undo-redo | Partial | undo/redo cap 100 snapshot per file (state.rs) |
| Crash recovery | Missing | Tidak ada auto-save/recovery file |

**Kesimpulan editor:** Editor TUI saat ini adalah *text viewer* dengan edit karakter dasar, bukan HDL editor profesional. Tidak ada syntax highlighting SV, tidak ada completion, tidak ada refactoring.

### 3.3 Project & workspace management — TIADA MODEL PROJECT NYATA

| Aspek | Status | Bukti |
|-------|--------|-------|
| Project creation | Missing | Tidak ada wizard buat project di GUI (genwizard hanya generate module/interface) |
| Project templates | Missing | Tidak ada |
| Project configuration | Missing | Tidak ada GUI untuk konfigurasi compile/sim per project |
| File management | Partial | scan_tree + FileNode tree, bookmark, filter kategori |
| Source discovery | Partial | scan_tree rekursif (skip hidden/target/node_modules) |
| Source file ordering | Missing | Tidak ada urutan kompilasi yang bisa diatur user |
| File groups | Missing | FileCat (Rtl/Testbench/Library/Include) heuristik saja |
| Source sets | Missing | Tidak ada |
| Include directories | Partial | CLI incdirs, GUI tidak ada manajemen incdir |
| Package management | Partial | package di-index (compile_info.packages), tidak ada manajemen GUI |
| Macro definitions | Partial | macro_index (scan `define`), tidak ada edit macro di GUI |
| Compile configurations | Missing | Tidak ada konfigurasi compile tersimpan |
| Multiple configurations/targets | Missing | Satu compile per buka project |
| Top-module selection | Partial | pending_top dari CLI --top, tidak ada pemilih top di GUI |
| Testbench management | Missing | FileCat::Testbench heuristik, tidak ada manajemen TB |
| Library management | Missing | Tidak ada |
| External libraries | Missing | Tidak ada GUI |
| Dependency tracking | Partial | dep_graph (module → child instance) dari IrDesign |
| Project validation | Partial | mcheck CLI, tidak ada validasi otomatis saat buka project di GUI |
| Project migration/import/export/cloning | Missing | Tidak ada |
| Project history | Partial | workspace.js recent projects (dedupe, cap) |

**Kesimpulan:** GUI beroperasi pada *individual files/direktori*, bukan project model yang terpercaya. Tidak ada konfigurasi project yang tersimpan dan dipulihkan secara kaya.

### 3.4 Simulation workflow — "RUN SAMPAI SELESAI" SAJA

| Aspek | Status | Bukti |
|-------|--------|-------|
| Simulation configuration | Partial | max_time, top, debug config (breakpoints/watchpoints) di state |
| Top-level selection | Partial | pending_top, tidak ada dropdown pilih top |
| Testbench selection | Missing | Tidak ada |
| File compilation order | Missing | Tidak diatur user |
| Compile arguments | Missing | Tidak ada GUI |
| Simulator options | Missing | Tidak ada GUI (xprop, packed, parallel, dll. hanya CLI) |
| Environment variables | Missing | Tidak ada GUI |
| Runtime configuration | Missing | Tidak ada |
| Run | Implemented | F5 → spawn_sim |
| Run with configuration | Missing | Tidak ada profil run tersimpan |
| Run to completion | Implemented | engine.run() hingga max_time/$finish |
| Run for specific duration | Partial | max_time default 1000 ns |
| Run until condition | Missing | Breakpoint signal eq/change ada tapi tidak ada "run until" terpadu UI |
| Stop | Implemented | cancel_flag + F5 stop |
| Pause | Missing di GUI | Engine pun pause (debugger.rs) tapi GUI tidak ada tombol pause |
| Resume | Missing | Tidak ada |
| Restart | Missing | Tidak ada |
| Reset | Missing | Tidak ada |
| Rerun | Missing | Tidak ada tombol rerun |
| Batch simulation | Missing di GUI | mbatch CLI ada, tidak di GUI |
| Multiple instances | Missing | Tidak ada |
| Parallel execution | Missing di GUI | Tidak ada |
| Simulation history | Missing | Tidak ada riwayat run |
| Result management | Partial | SimInfo disimpan state, diganti setiap run |
| Failure analysis | Missing | Tidak ada analisis kegagalan otomatis |
| Reproducibility | Partial | Tidak ada seed/config simulasi tersimpan |

**Kesimpulan:** Hanya run dan stop yang berfungsi. Pause/resume/reset/restart/rerun tidak ada di GUI meski engine mendukung sebagian.

### 3.5 Waveform & signal analysis — SANGAT PRIMITIF

| Aspek | Status | Bukti |
|-------|--------|-------|
| Signal browser | Partial | signals.rs: daftar sinyal dengan nilai hex |
| Hierarchical signal discovery | Missing | Tidak ada hierarki scope dalam browser |
| Signal search | Missing | Tidak ada pencarian sinyal di waveform |
| Add/remove signals | Missing | Semua sinyal top ditampilkan, tidak ada pilihan |
| Signal grouping | Missing | Tidak ada grouping |
| Signal reordering | Missing | Tidak ada |
| Bus expansion/collapse | Missing | Tidak ada |
| Binary/decimal/hex display | Partial | Hex saja (logicvec_to_hex), tanpa biner/desimal bawaan viewer |
| Signed/unsigned | Missing | Tidak ada |
| State decoding | Partial | x/z ada (▧/▦) |
| Analog-style rendering | Missing | Tidak ada |
| Time navigation | Partial | ruler waktu di atas |
| Zoom | Missing | wave_zoom ada di state tapi render tidak menyesuaikan secara bermakna |
| Pan | Missing | Tidak ada |
| Cursor | Missing | Tidak ada |
| Multiple cursors | Missing | Tidak ada |
| Time measurement | Missing | Tidak ada |
| Value measurement | Missing | Tidak ada |
| Signal comparison | Partial | prev_waveform untuk compare, tapi render compare belum ada (wave_compare flag tidak dipakai render) |
| Signal overlays | Missing | Tidak ada |
| Trigger navigation | Missing | Tidak ada |
| Edge detection | Missing | Tidak ada |
| Value transitions | Missing | Tidak ada |
| Event search | Missing | Tidak ada |
| Signal filtering | Missing | wave_hidden (sembunyikan), trace_filter (tab Trace substring) |
| Waveform bookmarks | Missing | Tidak ada |
| Waveform saving | Partial | export_vcd (Command Palette) → mivon_wave.vcd |
| Waveform restoration | Missing | Tidak ada |
| Large waveform datasets | Missing | Tidak ada lazy loading; hardcoded 256MB max_dump_size backend |
| Lazy loading | Missing | Tidak ada |
| VCD/FST format | Partial | VCD ditangkap engine, FST hanya CLI (msim), GUI hanya VCD parse |
| Memory visualization | Missing | Tidak ada |
| Array visualization | Missing | Tidak ada |
| X/Z state visualization | Partial | ▧/▦ karakter |

**Kesimpulan:** Waveform viewer TUI tidak layak untuk debugging RTL kompleks. Hilang hampir semua fitur standar waveform viewer (GTKWave/Surfer).

### 3.6 Debugging & observability — TERBATAS

| Aspek | Status | Bukti |
|-------|--------|-------|
| Error inspection | Partial | Problems tab |
| Compile diagnostics | Partial | CompileDone diag |
| Elaboration diagnostics | Partial | LSP diag + compile diag |
| Simulation diagnostics | Partial | SimDone error |
| Runtime diagnostics | Partial | debug_events |
| Stack traces | Missing | Tidak ada call stack |
| Source location mapping | Partial | Goto definition via LSP |
| Signal tracing | Partial | debug_events + signal_history |
| Process tracing | Missing | Tidak ada |
| Event tracing | Missing | Tidak ada |
| Memory inspection | Missing di GUI | CLI `--mem` ada, GUI tidak |
| Variable inspection | Missing | Tidak ada variable inspector |
| Hierarchy inspection | Partial | Architecture panel (dep_graph) |
| Breakpoints | Partial | breakpoints sebagai string teks (bp cycle/change/signal/module) |
| Conditional breakpoints | Missing | Tidak ada kondisi kompleks |
| Watchpoints | Partial | watchpoints (nama sinyal) |
| Assertion inspection | Partial | Assertions tab (pass/fail per line:col) |
| Assertion failures | Partial | Lihat di atas |
| Coverage inspection | Partial | Coverage tab (ringkasan angka, tidak ada source highlighting) |
| Execution history | Missing | Tidak ada |
| Debug snapshots | Partial | StateSnapshot di engine (deep-debug), tidak di GUI |
| Reproducibility | Missing | Tidak ada |
| Debug sessions | Missing | Tidak ada manajemen session debug |
| Root-cause investigation | Missing | Tidak ada workflow terpadu |

**Kesimpulan:** Debugging hanya breakpoints/watchpoints sederhana via terminal. Tidak cukup untuk debugging kompleks.

---

---

## 4. TOOL ECOSYSTEM AUDIT

### 4.1 Tools yang ada (via CLI `mivon <tool>`)

| Tool | File | Fungsi | Tersedia di GUI? |
|------|------|--------|------------------|
| minspect | inspect.rs | X-ray project (stats/modules/hierarchy/packages/classes/interfaces/parameters/deps/cache) | Tidak |
| mlint | lint/ | Static linter (unused signal, width mismatch, latch, comb loop, FSM, case, clock gating, power, memory, gate_opt) | Partial (lint_sources hanya unused signal + blocking assign) |
| melab | elab.rs | Standalone elaborator (hierarchy tree, params, signals, RDC SIM-22) | Tidak |
| msim | sim.rs | Simulasi (VCD/FST, assertion, coverage) | Tidak (GUI pakai engine langsung, tidak via msim CLI) |
| mcov | cov.rs | Coverage → coverage.json + coverage.html | Tidak |
| mwave | wave.rs | VCD utility (merge/export/filter/compare/search/tree/stats/get/decode/protocol) | Tidak |
| mfmt | fmt.rs | Formatter SV/Verilog (stdout/inplace/check) | Tidak (tidak ada format-on-save di editor) |
| mprof | prof.rs | Profiler pipeline (timing per fase + bottleneck + hint) | Tidak |
| mcheck | check.rs | Health check (missing include, circular include, deps, cycle module, timescale, ast-diff, changed) | Tidak |
| mbench | bench.rs | Benchmark (compile speed, throughput, peak RSS, cache hit) | Tidak |
| mgen | gen.rs | Generator SV dari Mivon HDL (.mv/.mvh) | Tidak |
| synth | synth.rs | Synthesis (SYN check, RTL→SIR, netlist .mvnet, report) | Tidak |
| mbatch | batch.rs | Batch simulation runner (parallel/sequential jobs) | Tidak |
| mtbgen | tbgen.rs | Automated testbench generation from module ports | Tidak |
| mregression | regression.rs | Regression test management | Tidak |
| mwaiver | waiver.rs | Lint/formal violation waiver management | Tidak |
| mdashboard/mweb | verif_dashboard, web | Verification dashboard / web kontrol simulasi | Tidak |

### 4.2 Kapasitas yang HILANG dari ecosystem

1. **Formal verification GUI** — formal (Z3 BMC, k-induction, SEC) hanya via CLI `--formal`, tidak ada GUI untuk hasil formal.
2. **Synthesis/Timing analysis GUI** — synth hanya CLI, tidak ada visualisasi netlist/timing di GUI.
3. **Lint terintegrasi editor** — mlint hanya CLI; GUI lint sangat terbatas (2 pola saja), tidak ada format-on-save, tidak ada quick-fix beragam.
4. **Coverage source-level** — mcov hanya JSON/HTML ringkasan, tidak ada highlight baris RTL yang tidak tercover di editor.
5. **Regression manager GUI** — mregression CLI ada, tidak ada GUI untuk pilih test/konfigurasi/run/bandingkan hasil.
6. **Test generation GUI** — mtbgen CLI ada, tidak ada GUI wizard generate testbench.
7. **Fuzzing GUI** — mivon-fuzz ada (crates/mivon-fuzz) tetapi tidak ada GUI campaign manager/seed corpus/failure triage.
8. **Waveform analysis tool GUI** — mwave CLI kuat (decode protocol APB/AXI4-Lite/AHB, compare, stats, get), tapi tidak ada yang muncul di GUI waveform viewer.
9. **Plugin/extension API** — mivon-env::plugin ada (konsep) tapi tidak ada extension API terpadu GUI untuk alat pihak ketiga.
10. **CI/CD integration** — tidak ada integrasi GitHub Actions/CI dalam GUI (hanya konsep di gui.md).

### 4.3 Pengelompokan alat (rekomendasi arsitektur)

- **Integrasi langsung ke Mivon (GUI):** compile, elaborate, sim, debug, waveform viewer, problems/diagnostics, project explorer.
- **Native GUI workflow:** lint (mlint), fmt (mfmt), coverage (mcov), signal search (mwave search/get).
- **Standardized adapter:** formal, synthesis, timing, external simulators (Verilator/VCS/Xcelium/Questa) via DPI/VPI/VHPI adapter.
- **External tools (CLI saja):** synth, mbench, mcheck, mbatch, mgen, emulator, formal — cukup via CLI, tidak perlu GUI bawaan.
- **Optional plugin:** differential verification, CI dashboard, collaboration, remote cache.
- **Jangan implementasi:** full GTKWave/Surfer clone (lebih baik impor VCD/FST ke viewer eksternal atau gunakan wavefst), full formal solver bawaan (pakai Z3 sebagai library yang sudah ada).

---

## 5. PERBANDINGAN DENGAN EDA TERJEJAL (EVIDENCE-BASED)

| Tool | Primary purpose | GUI | Debug | Waveform | Project mgmt | Catatan vs Mivon |
|------|----------------|-----|-------|----------|--------------|------------------|
| Siemens Questa | Commercial SV simulation/verification | Penuh (Visualizer, debug) | Lengkap (breakpoint, trace, source) | Lengkap (debug, compare) | Lengkap | Mivon TUI jauh di bawah |
| Synopsys VCS | Commercial SV simulation | CLI-oriented (SimVision terpisah) | Cukup | Via SimVision | CLI/filelist | Mivon lebih sederhana, kurang fitur |
| Cadence Xcelium | Commercial parallel simulation | CLI + SimVision | Cukup | Via SimVision | CLI | Mivon kurang paralel terpadu |
| Cadence SimVision | Waveform/debug viewer | Penuh | Lengkap | Lengkap (probe, compare) | Tidak (viewer) | Mivon waveform TUI sangat primitif |
| Verilator | Open-source lint/sim (C++ compile) | Tidak (CLI) | Tidak | Tidak (VCD via external) | Tidak | Mivon memiliki GUI TUI tapi kurang optimasi |
| GTKWave | Waveform viewer | Desktop (GTK) | Tidak | Lengkap (zoom, cursor, search, filter) | Tidak | Mivon kalah signifikan |
| Surfer | Waveform viewer | Web/desktop (Rust) | Tidak | Lengkap (modern, banyak format) | Tidak | Mivon kalah signifikan |
| OSS CAD Suite | Toolchain paket (Yosys, nextpnr, GUI) | Bervariasi | Terbatas | Via external | Terbatas | Mivon belum mencakup FPGA flow |
| Yosys | Synthesis (RTL→netlist) | CLI | Tidak | Tidak | Tidak | Mivon synth ada (CLI) tapi lebih baru/kurang matang |

**Posisi Mivon:** Kurang dari semua alat di atas dalam hal GUI dan workflow terpadu. Hingga saat ini Mivon lebih dekat ke simulator CLI yang dikemas dengan TUI eksperimental, bukan EDA environment profesional.

---

## 6. FEATURE COMPARISON MATRIX

| Category | Mivon | Questa | VCS | Xcelium | Verilator | GTKWave | Surfer | Gap | Priority |
|----------|-------|--------|-----|---------|-----------|---------|--------|-----|----------|
| GUI (desktop) | Missing (TUI only) | Implemented | Implemented (SimVision) | Implemented (SimVision) | Missing | Implemented | Implemented | Desktop GUI | P0 |
| HDL editor w/ syntax highlight | Missing | Implemented | Implemented | Implemented | Missing | N/A | N/A | SV semantic editor | P0 |
| Go-to-definition | Partial (LSP) | Implemented | Implemented | Implemented | Missing | N/A | N/A | Lengkapkan LSP editor | P1 |
| Find references | Missing | Implemented | Implemented | Implemented | Missing | N/A | N/A | — | P1 |
| Rename symbol | Missing | Implemented | Implemented | Implemented | Missing | N/A | N/A | — | P2 |
| Code completion | Missing | Implemented | Implemented | Implemented | Missing | N/A | N/A | — | P1 |
| Project model (config tersimpan) | Missing | Implemented | Implemented | Implemented | Missing | N/A | N/A | .mivon project model | P0 |
| Multiple targets/config | Missing | Implemented | Implemented | Implemented | Missing | N/A | N/A | — | P1 |
| Run/Pause/Resume/Reset/Restart | Partial (run/stop only) | Implemented | Implemented | Implemented | Missing | N/A | N/A | Pause/resume/reset | P0 |
| Breakpoint (cycle/signal) | Partial (teks) | Implemented | Implemented | Implemented | Missing | N/A | N/A | Conditional breakpoint | P1 |
| Watchpoint | Partial | Implemented | Implemented | Implemented | Missing | N/A | N/A | — | P1 |
| Call stack | Missing | Implemented | Implemented | Implemented | Missing | N/A | N/A | — | P2 |
| Variable inspector | Missing | Implemented | Implemented | Implemented | Missing | N/A | N/A | — | P1 |
| Reverse debug | Partial (engine only) | Implemented | Limited | Limited | Missing | N/A | N/A | Expose ke GUI | P2 |
| Waveform viewer | Partial (TUI char) | Implemented | Via SimVision | Via SimVision | Missing | Implemented | Implemented | Zoom/cursor/search/bus | P0 |
| VCD/FST support | Partial (VCD GUI, FST CLI) | Implemented | Implemented | Implemented | Implemented (VCD) | Implemented | Implemented | FST di GUI | P1 |
| Signal grouping/bus | Missing | Implemented | Implemented | Implemented | Missing | Implemented | Implemented | — | P1 |
| Coverage source highlight | Missing | Implemented | Implemented | Implemented | Missing | N/A | N/A | — | P2 |
| Regression manager | Missing | Implemented | Implemented | Implemented | Missing | N/A | N/A | — | P2 |
| Formal verification | CLI only | Implemented | Implemented | Implemented | Missing | N/A | N/A | GUI formal | P3 |
| Synthesis/timing | CLI only | Implemented | Implemented | Implemented | Missing | N/A | N/A | GUI synth report | P3 |
| Lint integrated editor | Partial (2 pola) | Implemented | Implemented | Implemented | Implemented | N/A | N/A | Lengkapkan lint | P1 |
| Plugin/extension API | Missing (konsep) | Limited | Limited | Limited | Missing | N/A | N/A | — | P3 |
| CI/CD integration | Missing | Implemented | Implemented | Implemented | Missing | N/A | N/A | — | P3 |

**Status kunci:** Implemented and verified (dicentang via kode), Implemented but unverified, Partially implemented, Experimental, Missing, Not applicable, Unknown. Tidak ada status "good/modern/competitive" yang digunakan.

---

---

## 10. UX PROBLEMS & ENGINEERING FRICTION

| Problem | Root cause | User impact | Engineering impact | Severity | Solusi |
|---------|-----------|-------------|-------------------|----------|--------|
| Terlalu banyak langkah buka file | Tidak ada "search everywhere"; harus Ctrl+O → sidebar → Enter | Tinggi (harian) | Tinggi | P1 | Search everywhere + recent file |
| Tidak bisa pause/resume/reset/restart sim | GUI hanya expose run/stop; engine support sebagian | Tinggi | Tinggi | P0 | Tambah tombol kontrol sim + state |
| Tidak ada syntax highlighting SV | Editor TUI teks mentah | Tinggi | Tinggi | P0 | LSP semantic highlighting + tree-sitter SV |
| Tidak ada completion/go-to-def nyata | LSP completion request diabaikan di editor | Tinggi | Tinggi | P1 | Eksekusi LSP completion + tampilkan popup |
| Waveform tidak bisa zoom/cursor/ukur | Render karakter terminal statis | Tinggi | Tinggi | P0 | Viewer waveform nyata atau impor ke GTKWave/Surfer |
| Tidak ada project config tersimpan | GuiState tidak menyimpan .mivon project | Tinggi | Tinggi | P0 | Project model + .mivon file |
| Context hilang saat switch panel | Tidak ada workspace state yang kaya | Sedang | Sedang | P2 | Session restoration kaya |
| Tidak bisa pilih top module di GUI | pending_top hanya dari CLI | Sedang | Sedang | P1 | Pemilih top module dropdown |
| Error sulit dilacak ke sumber | resolve_assert_file hanya unambiguous | Sedang | Sedang | P2 | Source mapping yang lebih robust |
| Tidak ada call stack/variable inspector | Debugger hanya breakpoint/watchpoint teks | Tinggi | Tinggi | P1 | Variable inspector + call stack |
| Layout tidak bisa disesuaikan | Sidebar fixed 30%, bottom fixed | Sedang | Rendah | P3 | Panel docking/resizing interaktif |

---

## 11. ARSITEKTUR KELEMAHAN

1. **Tight coupling GUI-backend:** `GuiState` menumpuk semua state (project, editor, compile, sim, waveform, debug, LSP, terminal, palette, wizard). Backend `spawn_compile`/`spawn_sim` langsung memutar `GuiState`. Sulit diujikan/piagam.
2. **Tidak ada batas jelas antara compile/simulator dan UI:** Backend GUI (backend.rs) memuat logika lint, dep graph, signal index, parse VCD — yang seharusnya ada di library terpisah.
3. **Operasi berat masih blocking-ish:** Compile/sim di worker tapi state UI diakses langsung dari worker melalui channel; tidak ada job scheduler terpadu.
4. **Tidak ada event system standar:** Hanya channel mpsc `GuiEvent` sederhana, tidak ada event bus untuk komunikasi antar panel.
5. **Tidak ada data virtualization/lazy loading:** Semua sinyal/file dimuat ke memori; tidak skalabel untuk waveform besar.
6. **Tidak ada incremental update UI:** File berubah memerlukan re-scan + re-compile manual.
7. **Komponen yang butuh interface standar:** LSP client, waveform renderer, debugger, project model — masing-masing terpadu dengan GuiState.

---

## 12. PRIORITAS P0–P4

### P0 — Critical (mencegah alur kerja inti)
1. Desktop GUI (bukan hanya TUI) atau integrasi viewer waveform eksternal — Impact tinggi, Frekuensi tinggi, Risiko sedang, Kompleksitas tinggi, Dependency GUI framework, Maintenance tinggi.
2. Syntax highlighting + semantic editor SystemVerilog — Impact tinggi, Frekuensi tinggi, Risiko tinggi, Kompleksitas tinggi, Dependency tree-sitter/LSP, Maintenance tinggi.
3. Run/Pause/Resume/Reset/Restart simulasi di GUI — Impact tinggi, Frekuensi tinggi, Risiko rendah, Kompleksitas sedang, Dependency engine debug mode, Maintenance sedang.
4. Project model .mivon dengan konfigurasi tersimpan — Impact tinggi, Frekuensi tinggi, Risiko sedang, Kompleksitas sedang, Dependency file format, Maintenance sedang.

### P1 — Essential (harian profesional)
5. Go-to-definition, find references, code completion (LSP lengkap) — Impact tinggi, Frekuensi tinggi, Risiko sedang, Kompleksitas tinggi, Dependency LSP server, Maintenance tinggi.
6. Variable inspector + call stack debugger — Impact tinggi, Frekuensi sedang, Risiko sedang, Kompleksitas sedang, Dependency engine state, Maintenance sedang.
7. Waveform zoom/pan/cursor/time measurement — Impact tinggi, Frekuensi tinggi, Risiko sedang, Kompleksitas tinggi, Dependency rendering, Maintenance tinggi.
8. Lint terintegrasi editor + format-on-save — Impact sedang, Frekuensi tinggi, Risiko rendah, Kompleksitas sedang, Dependency mlint/mfmt, Maintenance sedang.
9. Conditional breakpoint — Impact sedang, Frekuensi sedang, Risiko rendah, Kompleksitas sedang, Dependency engine, Maintenance sedang.
10. Top-module pemilih di GUI — Impact sedang, Frekuensi sedang, Risiko rendah, Kompleksitas rendah, Dependency state, Maintenance rendah.

### P2 — Important
11. Signal grouping/bus expansion/collapse — Impact sedang, Frekuensi sedang, Risiko sedang, Kompleksitas sedang, Dependency waveform model, Maintenance sedang.
12. Coverage source highlighting — Impact sedang, Frekuensi sedang, Risiko sedang, Kompleksitas sedang, Dependency mcov + editor, Maintenance sedang.
13. Regression manager GUI — Impact sedang, Frekuensi rendah, Risiko sedang, Kompleksitas sedang, Dependency mregression, Maintenance sedang.
14. Reverse debug exposed ke GUI — Impact sedang, Frekuensi rendah, Risiko tinggi, Kompleksitas tinggi, Dependency engine snapshot, Maintenance tinggi.
15. Session restoration kaya — Impact sedang, Frekuensi sedang, Risiko rendah, Kompleksitas sedang, Dependency workspace, Maintenance sedang.

### P3 — Advanced
16. Formal verification GUI — Impact rendah, Frekuensi rendah, Risiko tinggi, Kompleksitas tinggi, Dependency Z3/formal, Maintenance tinggi.
17. Synthesis/timing report GUI — Impact rendah, Frekuensi rendah, Risiko sedang, Kompleksitas sedang, Dependency synth, Maintenance sedang.
18. Plugin/extension API — Impact rendah, Frekuensi rendah, Risiko sedang, Kompleksitas tinggi, Dependency API design, Maintenance tinggi.
19. CI/CD integration — Impact rendah, Frekuensi rendah, Risiko rendah, Kompleksitas sedang, Dependency CI, Maintenance sedang.
20. Multi-window / split editor — Impact rendah, Frekuensi rendah, Risiko sedang, Kompleksitas tinggi, Dependency GUI framework, Maintenance tinggi.

### P4 — Optional
21. Theme support, colorblind-safe, high-DPI — Impact rendah, Frekuensi rendah, Risiko rendah, Kompleksitas rendah, Maintenance rendah.
22. Notifications, progress indicators — Impact rendah, Frekuensi rendah, Risiko rendah, Kompleksitas rendah, Maintenance rendah.

---

---

## 13. IMPLEMENTATION ROADMAP

### Phase 1: Foundation — Fix arsitektur, reliability, workflow penting, batasan GUI besar
- Objectives: GUI yang kompilasi & stabil, project model dasar, kontrol simulasi (pause/resume/reset/restart), debugger dasar (variable inspector), waveform viewer yang layak.
- Dependencies: GUI framework (jika beralih dari ratatui), LSP server, engine debug state.
- Acceptance criteria: `cargo build --workspace` lolos, `cargo test --workspace` 0 gagal, GUI bisa buka project, compile, sim, pause/resume, lihat variable, export VCD.
- Test requirements: unit test GUI panels, integration test run/pause/resume.
- Performance: buka project < 5 detik untuk 100 file.
- Exit criteria: P0 tercapai.

### Phase 2: Professional Workflow — Project management, editor integrasi, simulasi, debugging, waveform, tool integrasi
- Objectives: .mivon project file tersimpan, LSP semantic highlighting/completion/go-to-def/references/rename, format-on-save, lint integrasi, waveform zoom/pan/cursor/measurement, coverage source highlight, regression manager GUI, test generation GUI.
- Dependencies: tree-sitter SV / LSP, mcov, mregression, mtbgen.
- Acceptance criteria: semua P1 tercapai; edit→compile→debug→waveform alur tanpa keluar GUI.
- Test requirements: end-to-end test workflow A-F.
- Performance: incremental compile restore > 80%.
- Exit criteria: P1 tercapai.

### Phase 3: Advanced Ecosystem — Analisis lanjutan, automasi, regression, observability, extensibility, large-project
- Objectives: formal GUI, synthesis/timing report GUI, plugin API, CI/CD integration, data virtualization/lazy loading waveform, signal grouping/bus, multi-window/split.
- Dependencies: Z3, synth crate, API design.
- Acceptance criteria: P2 tercapai; skala 1000 file, waveform 50k sinyal render < 2 detik.
- Exit criteria: P2 tercapai.

### Phase 4: Competitive Differentiation — Kapasitas Mivon bisa lebih baik/dari lain
- Objectives: differential verification (multi-engine compare), co-simulation, collaborative debug, reproducible execution environment, session sharing.
- Jangan copy alat lain tanpa alasan engineering yang jelas.
- Exit criteria: P3 tercapai; Mivon beda dari EDA lain melalui integrasi sim+debug+waveform yang terpadu dalam satu binary Rust.

---

## 14. CONCRETE ACCEPTANCE CRITERIA (untuk 0.1 Alpha Enterprise)

1. `cargo build --workspace` dan `cargo test --workspace` 0 gagal.
2. GUI bisa buka project direktori, tampilkan pohon file, buka file, edit, simpan.
3. GUI bisa F7 compile, lihat Problems, klik error → buka file & goto line.
4. GUI bisa F5 run simulasi, pause, resume, stop, reset, restart.
5. GUI debugger: breakpoint (cycle/signal eq/signal change), watchpoint, variable inspector, call stack.
6. GUI waveform: lihat sinyal, zoom, pan, cursor, ukur waktu, export VCD.
7. Editor: syntax highlighting SV, go-to-definition, code completion.
8. Project: .mivon file tersimpan & dipulihkan, top module pemilih, compile configuration tersimpan.
9. Regression manager GUI: pilih test, run, bandingkan hasil.
10. Dokumentasi setiap fitur GUI (bukan hanya gui.md konsep).

---

## 15. FEATURES THAT SHOULD NOT BE IMPLEMENTED

1. **Full GTKWave/Surfer clone** — lebih baik impor VCD/FST ke viewer eksternal atau pakai wavefst; tidak perlu buat waveform viewer desktop dari nol.
2. **Full formal solver bawaan** — Z3 sudah ada sebagai library via `mivon-formal`; tidak perlu reimplementasi solver.
3. **Synthesis bawaan yang layak produksi** — Yosys sudah matang; Mivon synth cukup CLI untuk analisis, bukan GUI synthesis production.
4. **Plugin marketplace bawaan** — terlalu awal; cukup exposure API jika ada permintaan.
5. **Fitur "modern" tanpa tujuan engineering** (dark mode, animasi, colaborasi real-time) sebelum inti EDA berfungsi.

---

## 16. FINAL COMPETITIVE READINESS ASSESSMENT

**Q: Apakah Mivon saat ini mampu mendukung alur kerja pengembangan HDL profesional?**
**A:** Tidak. Mivon berfungsi sebagai simulator CLI untuk desain kecil (counter, adder, hierarki 3-level, picorv32 RISC-V berhasil disimulasikan), tetapi GUI-nya adalah ratatui TUI primitif tanpa editor HDL yang layak, tanpa debugging yang cukup, tanpa waveform viewer yang layak, dan tanpa project model terpercaya.

**Q: Apa yang mencegah Mivon menjadi EDA environment serius?**
**A:** (1) GUI bukan desktop dan editor tidak ada syntax highlighting/completion/refactoring, (2) tidak ada project model terpercaya (.mivon konfigurasi tersimpan), (3) simulasi hanya run/stop tanpa pause/resume/reset/restart, (4) waveform viewer primitif (karakter terminal, tidak ada zoom/cursor/measurement), (5) tidak ada debugger yang cukup (tidak ada call stack, variable inspector, conditional breakpoint), (6) tidak ada alat EDA profesional terintegrasi (formal, synthesis, timing, coverage source, regression GUI).

**Q: Fitur GUI paling penting yang hilang?**
**A:** Syntax highlighting editor SV, go-to-definition/completion/references, pause/resume/reset/restart simulasi, waveform zoom/cursor/measurement, variable inspector + call stack, project configuration tersimpan.

**Q: Alat apa yang hilang dari ecosystem?**
**A:** Formal GUI, synthesis/timing GUI, coverage source highlight, regression manager GUI, test generation GUI, fuzzing GUI, plugin API, CI/CD integration.

**Q: Integrasi apa yang hilang?**
**A:** LSP editor yang nyata, waveform viewer yang layak, debugger yang terpadu dengan source, external simulator adapter (Verilator/VCS/Xcelium/Questa), CI/CD.

**Q: Kelemahan arsitektur terbesar?**
**A:** GuiState monoputih yang menumpuk semua state, tight coupling backend-UI, tidak ada job scheduler/event system standar, tidak ada data virtualization/lazy loading.

**Q: Kapasitas apa yang menjadi bottleneck di skala SoC?**
**A:** Tidak ada lazy loading waveform, semua sinyal dimuat memori, tidak ada incremental update UI, tidak ada indeks sinyal, tidak ada batasan ukuran dump yang terenkapselasi di GUI.

**Q: 10 perbaikan terpenting?**
**A:** 1) Desktop GUI/editor SV, 2) syntax highlighting + completion, 3) go-to-def/references, 4) pause/resume/reset/restart, 5) waveform zoom/cursor/measurement, 6) variable inspector + call stack, 7) project .mivon model, 8) lint+format integrasi, 9) coverage source highlight, 10) regression manager GUI.

**Q: 10 perbaikan nilai engineering tertinggi?**
**A:** Sama dengan di atas, ditambah: 11) LSP editor lengkap, 12) data virtualization waveform, 13) external simulator adapter, 14) CI/CD integration.

**Q: Fitur apa yang sia-sia?**
**A:** Full GTKWave/Surfer clone, full formal solver bawaan, synthesis production GUI, plugin marketplace, fitur kosmetik sebelum inti berfungsi.

**Q: Apa yang harus ada sebelum 0.1 Alpha Enterprise?**
**A:** Semua P0 + penerapan acceptance criteria (1-10 di atas).

**Q: Apa yang ditunda setelah rilis?**
**A:** P3 (formal GUI, synth GUI, plugin API, CI/CD, multi-window), P4 (theme, notifikasi).

**Q: Posisi Mivon versus alat terjadwal?**
**A:** Kurang dari semua alat dalam hal GUI dan workflow terpadu. Mivon adalah simulator CLI dengan TUI eksperimental, belum EDA environment profesional.

---

## LAMPIRAN: BUKTI SUMBER KODE

- `crates/mivon-gui/src/app.rs` — layout TUI (toolbar 3 + sidebar 30% + editor 70% + bottom 10), F5/F7 global, command palette.
- `crates/mivon-gui/src/editor.rs` — viewer teks mentah, tidak ada syntax highlighting.
- `crates/mivon-gui/src/panels/waveform.rs` — renderer karakter █/░/▧/▦, tidak ada zoom/cursor/measurement.
- `crates/mivon-gui/src/panels/debugger.rs` — breakpoints/watchpoints sebagai string teks, input via terminal.
- `crates/mivon-gui/src/panels/problems.rs` — daftar diagnostic (compile + LSP), quick fix hanya 2 jenis.
- `crates/mivon-gui/src/state.rs` — GuiState (~830 baris), monoputih semua state; editor undo/redo cap 100.
- `crates/mivon-gui/src/backend.rs` — spawn_compile/spawn_sim, parse_vcd, lint_sources (unused signal + blocking assign).
- `crates/mivon-gui/src/command_palette.rs` — 18 aksi nyata, teruji dengan unit test.
- `crates/mivon-simulator/src/simulator/types.rs` — DebugMode, Breakpoint, Watchpoint, StateSnapshot, EventRegion 13 wilayah IEEE.
- `crates/mivon-simulator/src/debugger/mod.rs` — step_cycle, pause, state snapshot, reverse step.
- `cargo test --workspace` — 0 gagal, 0 panic (verifikasi 19 Nov 2026).
