# mivon — RTL Simulator untuk SystemVerilog

## Aturan 
1 file = 1 tanggung jawab tidak boleh lebih dari 1
Rust-based SystemVerilog simulator. Pipeline: preprocessor → lexer → parser → AST → elaborator → IR → simulation engine → VCD output.
 semua col dan line wajib 100% akuran dengan lokasi masalah 

### 🚫 LARANGAN TOTAL: SCRIPT UNTUK MEMODIFIKASI PROJECT
**DILARANG TOTAL, TANPA TOLERANSI** — dilarang menggunakan script apa pun (Python, Perl, Ruby, Bash/sed/awk mass-edit, `python -c` one-liner, dan sejenisnya) untuk mengubah/memodifikasi file di project ini.

**Berlaku untuk SEMUA agent dan subagent yang bekerja di project ini, tanpa terkecuali.**

# Aturan Modifikasi Project

## 1. Prinsip utama

Semua perubahan pada source code project harus dilakukan secara **terkontrol, terukur, dan dapat diverifikasi**.

Agent tidak boleh melakukan perubahan massal hanya demi mempercepat pekerjaan. Keamanan correctness lebih penting daripada kecepatan editing.

Prioritas:

1. Jangan merusak kode yang sudah bekerja.
2. Ubah sekecil mungkin.
3. Verifikasi konteks sebelum mengubah.
4. Build/test setelah perubahan yang berisiko.
5. Jangan melakukan perubahan otomatis apabila struktur kode belum dipahami.

---

## 2. Perubahan manual adalah default

Untuk perubahan yang menyentuh:

- ownership / borrowing Rust
- trait
- generic
- lifetime
- `Deref` / `DerefMut`
- `Copy` / `Clone`
- `Arc` / `Rc`
- `Option` / `Result`
- macro
- parser / lexer
- AST
- semantic analysis
- elaboration
- evaluator / simulator
- concurrency / parallel evaluation
- unsafe code
- API publik

**WAJIB menggunakan perubahan manual yang terkontrol**, misalnya:

- `str_replace`
- `write_file`
- editor
- patch kecil yang secara eksplisit menargetkan lokasi tertentu

Perubahan harus dilakukan **satu logical change pada satu waktu**.

---

## 3. Automated edit hanya diperbolehkan untuk perubahan LOW-RISK

Script atau tool otomatis boleh digunakan hanya apabila perubahan memenuhi seluruh kondisi berikut:

- pola perubahan sangat jelas;
- tidak mengubah semantic Rust;
- tidak menyentuh ownership/borrowing;
- tidak mengubah tipe;
- tidak mengubah control flow;
- tidak mengubah API;
- tidak mengubah macro;
- tidak melakukan transformasi struktural;
- target file dan jumlah perubahan dapat diverifikasi sebelum eksekusi.

Contoh yang masih dapat dianggap LOW-RISK:

- perubahan literal/string tertentu;
- pembaruan komentar;
- perubahan metadata sederhana;
- perubahan konfigurasi yang formatnya sudah diketahui;
- perubahan nama pada file/data non-source-code apabila tidak memengaruhi referensi kode;
- regenerasi file yang memang secara eksplisit merupakan generated artifact dan memiliki generator resmi.

Namun, **LOW-RISK tetap harus diverifikasi sebelum dan sesudah perubahan**.

---

## 4. Automated source-code transformation dilarang secara default

Agent **tidak boleh secara default** menggunakan:

- `sed -i`
- `awk` untuk menulis source
- Python script yang menulis source
- Perl replacement
- shell mass-replacement
- regex mass-edit
- codemod
- AST transformation otomatis
- script `/tmp` yang hasilnya kemudian disalin ke project
- tool otomatis lain yang melakukan perubahan source-code secara massal

Larangan ini berlaku terutama terhadap perubahan yang dapat memengaruhi:

- `&T` vs `T`
- `&mut T` vs `T`
- `*x`
- `.clone()`
- ownership
- borrowing
- lifetime
- trait resolution
- generic type
- pattern matching
- `async` / `await`
- concurrency
- unsafe code

---

## 5. Formatter dan fixer otomatis

Tool seperti:

- `cargo fix`
- `cargo clippy --fix`
- formatter otomatis
- automated refactoring

**tidak boleh digunakan untuk memodifikasi source-code secara otomatis** kecuali user memberikan izin eksplisit untuk penggunaan tool tersebut pada perubahan yang sedang dikerjakan.

Perintah read-only tetap diperbolehkan.

Contoh:

```bash
cargo check
cargo test
cargo test --no-run
cargo clippy
cargo fmt --check
```

Sedangkan tool yang melakukan rewrite source harus dianggap sebagai **write operation** dan memerlukan izin sesuai aturan di atas.

---

## 6. Read-only analysis selalu diperbolehkan

Agent boleh menggunakan tool/script untuk membaca dan menganalisis project.

Contoh:

```bash
grep
rg
awk
sed
find
git diff
git status
cargo check
cargo test
cargo metadata
```

Dengan syarat command tersebut **tidak menulis atau memodifikasi file project**.

Read-only analysis justru dianjurkan sebelum melakukan perubahan.

---

## 7. Sebelum mengubah file

Agent harus terlebih dahulu:

1. Membaca konteks kode yang relevan.
2. Menentukan fungsi/module yang terdampak.
3. Mencari semua reference penting jika perubahan menyentuh API atau tipe.
4. Memahami tipe yang terlibat.
5. Menentukan risiko perubahan.
6. Menentukan file yang benar-benar perlu diubah.

Jangan melakukan replacement hanya berdasarkan satu baris yang ditemukan oleh `grep`.

---

## 8. Perubahan harus sekecil mungkin

Gunakan prinsip:

> **Minimal Patch**

Jangan memperbaiki 20 hal sekaligus apabila masalah sebenarnya hanya membutuhkan perubahan 3 baris.

Setiap perubahan harus memiliki tujuan yang jelas.

Hindari:

- refactor sambil memperbaiki bug;
- rename besar sambil memperbaiki type error;
- formatting seluruh repository;
- cleanup kode yang tidak berhubungan;
- perubahan arsitektur dalam patch bug kecil.

---

## 9. Verifikasi setelah perubahan

Setelah perubahan source-code:

1. Periksa `git diff`.
2. Pastikan hanya file yang diharapkan berubah.
3. Jalankan pemeriksaan/build yang relevan.
4. Jalankan test yang berkaitan.
5. Jika perubahan menyentuh subsystem penting, lakukan test yang lebih luas.

Jika jumlah error meningkat drastis setelah perubahan:

> **STOP. Jangan melakukan patch berikutnya secara membabi buta.**

Kembali ke `git diff`, identifikasi perubahan yang menyebabkan regresi, lalu perbaiki secara manual.

---

## 10. Perubahan massal membutuhkan izin khusus

Jika agent menemukan bahwa perubahan massal benar-benar diperlukan, agent **tidak boleh langsung menjalankannya**.

Agent harus terlebih dahulu menjelaskan:

- file yang akan terkena;
- jumlah perubahan;
- pola perubahan;
- alasan perubahan massal diperlukan;
- risiko terhadap type/borrow/API;
- metode yang akan digunakan;
- cara rollback;
- verifikasi setelah perubahan.

Tanpa izin eksplisit dari user, gunakan pendekatan manual atau patch kecil.

---

## 11. Git sebagai safety boundary

Sebelum perubahan besar:

```bash
git status
git diff
```

Setelah perubahan:

```bash
git diff
git status
```

Agent harus memastikan tidak ada perubahan tidak sengaja pada file lain.

Jika repository memiliki perubahan user yang belum di-commit, **jangan menghapus, reset, checkout, atau overwrite perubahan tersebut** tanpa izin eksplisit.

Dilarang menggunakan operasi destruktif seperti:

```bash
git reset --hard
git checkout -- .
git clean -fd
```

tanpa izin eksplisit dari user.

---

## 12. Tingkat risiko perubahan

Gunakan klasifikasi berikut:

### LOW
Contoh:

- komentar;
- string;
- metadata;
- konfigurasi sederhana;
- perubahan satu literal yang tidak memengaruhi tipe/control flow.

→ Automated edit dapat digunakan secara terbatas setelah verifikasi.

### MEDIUM
Contoh:

- perubahan beberapa fungsi;
- rename identifier;
- perubahan API internal;
- perubahan struktur data;
- perubahan parser rule sederhana.

→ Gunakan patch kecil/manual. Automated edit hanya jika pola benar-benar deterministik dan telah diverifikasi.

### HIGH
Contoh:

- ownership;
- borrowing;
- lifetime;
- trait;
- generic;
- macro;
- parser/AST;
- evaluator;
- simulator;
- concurrency;
- unsafe;
- perubahan arsitektur.

→ **Manual edit wajib.**

### CRITICAL
Contoh:

- perubahan yang menyentuh core execution engine;
- parallel evaluation;
- memory model;
- simulator semantics;
- parser/semantic correctness;
- perubahan yang berpotensi menghasilkan silent miscompilation atau incorrect simulation.

→ Manual edit + verifikasi bertahap + test khusus wajib.

---

## 13. Jangan menganggap "compile error" sebagai satu-satunya indikator

Source-code yang berhasil compile belum tentu benar.

Untuk project seperti simulator/parser/compiler, agent harus memperhatikan:

- regression test;
- semantic correctness;
- behavioral correctness;
- differential testing;
- fuzzing;
- deterministic output;
- race/concurrency behavior;
- waveform/output correctness;
- perubahan performa yang tidak diharapkan.

Jika perubahan membuat build berhasil tetapi behavior berubah tanpa alasan yang jelas, perubahan dianggap **belum tervalidasi**.

---

## 14. Aturan untuk agent dan subagent

Semua aturan ini berlaku sama terhadap:

- agent utama;
- subagent;
- autonomous coding agent;
- tool invocation;
- script yang dibuat agent;
- command yang dijalankan agent.

Subagent tidak boleh menggunakan celah dengan cara membuat script sendiri untuk melakukan perubahan yang dilarang oleh aturan utama.

---

## 15. Prinsip akhir

**Kecepatan bukan alasan untuk mengorbankan correctness.**

Gunakan automated tooling untuk **menganalisis sebanyak mungkin**, tetapi gunakan perubahan otomatis hanya ketika risikonya rendah dan dapat diverifikasi.

Untuk kode inti Rust yang sensitif terhadap ownership, type system, parser, evaluator, simulator, dan concurrency:

> **Understand → Inspect → Minimal Patch → Diff → Build/Test → Verify**

Jangan:

> **Search → Replace Everything → Cargo Check → Panic**

## Build & Test

```shell
cargo build
cargo test                    # all unit tests (workspace: in-module + crates/mivon-tests)
cargo test -p mivon-tests     # test suite terpadu (ex src/tests + edge_tests + debug_lexer)
cargo test <test_name>        # single test (no --lib needed if unique)
```

No CI, no lint, no typecheck shortcuts. Just `cargo test`. 1634 tests pass.

## CLI Tools (`crates/mivon-tools/`, subcommand `mivon <tool>`)

11 tool terminal dari tools.md — satu file per tool (aturan 1 file = 1 tanggung
jawab). Pindah dari `src/tools/` ke crate `mivon-tools` (migrasi monorepo crate
11); `mivon::tools` re-export via lib.rs.

11 tool terminal dari tools.md — satu file per tool (aturan 1 file = 1 tanggung jawab):

| Tool | File | Fungsi |
|------|------|--------|
| `minspect` | `inspect.rs` | X-ray project: `stats`, `modules`, `hierarchy`, `packages`, `classes`, `interfaces`, `parameters`, `deps`. Subcommand boleh di posisi pertama (`minspect stats rtl/`) |
| `mlint` | `lint.rs` | Static linter: unused signal, width mismatch, latch, combinational loop, FSM |
| `melab` | `elab.rs` | Elaborasi saja: hierarchy tree, param, signal top, RDC (`--reset-domain`, SIM-22) |
| `msim` | `sim.rs` | Simulasi: VCD (+FST), ringkasan assertion/coverage |
| `mcov` | `cov.rs` | Coverage → `coverage.json` + `coverage.html` (via CoverageDatabase) |
| `mwave` | `wave.rs` | Utility VCD: `merge` (offset kumulatif), `export` (csv/txt), `filter` (subset sinyal), `compare` (diff 2 VCD per signal), `search` (cari sinyal by wildcard `*`/`?`), `tree` (index hierarki scope), `stats` (toggle/transitions/activity% + stuck detection), `get` (random access nilai sinyal: `--at T` / `--range t1:t2` / timeline penuh, WAV-07), `decode` (protokol bus apb/axi4lite/ahb → daftar transaksi, WAV-16) |
| `mfmt` | `fmt.rs` | Formatter SV/Verilog berbasis lexer (stdout/--inplace/--check) |
| `mprof` | `prof.rs` | Profiler pipeline: timing per fase + bottleneck + hint |
| `mcheck` | `check.rs` | Health check: missing `include, circular include, unresolved deps, cycle module, timescale + `--ast-diff b.sv` (AST differential, PARSER-13) |
| `mbench` | `bench.rs` | Benchmark: compile speed, throughput, peak RSS (VmHWM), cache hit |
| `mgen` | `gen.rs` | **Generator SV dari Mivon HDL (`.mv`/`.mvh`)** — `.mv` → lex → parse → type-check (E2001–E2013) → codegen → `.sv` + `.svh`. `--package/--check/--stdout/--no-check/--svh-only/--sv-only/-o`. Konteks type-check gabungan per direktori; scan rekursif lewat `negative/`, dotdir, prefiks `_`. Alias CLI `mivon gen` |
| `synth` | `synth.rs` | Synthesis (SYNTHESIS.md): SYN check (SYN-1..9), lowering RTL→SIR (`mivon-sir`, `--dump-sir`), inferensi FF, netlist `.mvnet`, report utilisasi. Nama lama `msynth` = alias |

Shared infra di `crates/mivon-tools/src/lib.rs` (ex `src/tools/mod.rs`):
- `collect_targets()` — expand file/direktori/file list
- `open_project()` — CompileSession → merged `Design` (parse saja, cepat, MICD cache)
- `open_elaborated()` — + elaborasi penuh → `IrDesign` (dipakai melab/msim/mcov/mprof)
- `expr_to_string()`, `section()`, `kv()`, `human_bytes()` — output konsisten

Cara kerja: semua tool memakai `CompileSession` (parallel parse + MICD), bukan pipeline legacy. Subcommand di-dispatch di `main.rs` via `dispatch_*()` + `exit_tool()`. CLI args di `src/cli.rs` (`MivonCmd` enum + struct args per tool).

Catatan formatter (`mfmt`): token-based, 1 file = 1 tanggung jawab, tidak pakai `Token::Display` untuk operator (Debug output `Plus` bukan `+`) — pakai `token_text()` manual.

## Pipeline architecture

Target akhir migrasi tercapai: **`src/` hanya berisi `main.rs` + `cli.rs`**
(package `mivon` binary-only). Seluruh logika hidup di crates/; API publik
di `mivon-api` (`crate::compile_str` → `mivon_api::compile_str`).

1. **`src/main.rs`** — CLI entrypoint. Reads `.sv` file(s), concatenates, feeds through lexer → parser → elaborator → engine. Subcommand di-dispatch via `mivon_api::tools::*` (10 tool) + `mivon_api::env`/`mv`/`formal`/`gui`.
2. **`crates/mivon-api/src/lib.rs`** — Public API: `compile_str()`, `simulate_str()`, `simulate_signals()`, `compile_files()`, `run_simulation()`, `compare_asts()` + re-export `mivon_*`. Tests live in `crates/mivon-tests/`.
3. **`crates/mivon-parser/src/`** — `lexer.rs` (tokenizer), `lib.rs` (Parser — Pratt-style top-down operator precedence, ex `src/parser/`), `preprocessor.rs` (`` `ifdef ``/`define`).
4. **`crates/mivon-ast/src/`** — `expr.rs`, `stmt.rs`, `types.rs`, `const_eval.rs`, `inline.rs` (function inlining for `loop_unroll` and `substitute_loop_var`).
5. **`crates/mivon-elaboration/src/elaborator/mod.rs`** — AST → IR, signal collection, type resolution, loop unrolling, constant folding for `$clog2`/`$bits`/`$size`/`$left`/`$right`/`$low`/`$high`.
6. **`crates/mivon-ir/src/ir.rs`** — IR types (`IrStmt`, `IrExpr`, `LogicVec`).
7. **`crates/mivon-simulator/src/simulator/`** — `engine/` (event-driven scheduler), `types.rs` (debug/event/UVM types), `state.rs` (signal storage), `value.rs` (`eval_binary`, `eval_unary`), `sdf.rs` (SDF annotation), `jit.rs` (JIT stubs), `parallel.rs` (parallel eval), `util.rs`.
8. **`crates/mivon-simulator/src/waveform/`** — `vcd.rs` (VCD dump), `fst.rs` (FST waveform via wavefst crate).
9. **`crates/mivon-simulator/src/debugger/mod.rs`** — `Debugger` struct wrapping `SimulationEngine`. Step, breakpoint, watchpoint, timeline, hierarchy tree, reverse debug, memory inspect. 21 unit tests inline.
10. **`uvm_macros.svh`** — UVM macro definitions (info/warning/error/fatal, factory utils).

## Enterprise Context Architecture (`crates/mivon-env/`)

Desain 5 doc/env.md: `GlobalEnv` root object menampung 12 context (masing-masing `Arc`), bukan satu Env raksasa. Dependency satu arah: `Config → Workspace → Runtime → Compiler → Cache/Database/Diagnostics/Telemetry → Verification → Simulation`. Pindah dari `src/env/` ke crate `mivon-env` (migrasi monorepo crate 10); `mivon_api::env` re-export via mivon-api lib.rs.

- `crates/mivon-env/src/env/global/` — `GlobalEnv` (aggregator + accessor), `startup.rs` (lifecycle `startup()`/`startup_with()`/`for_cli()`), `shutdown.rs`, `build.rs`/`version.rs`.
- `crates/mivon-env/src/env/config/` — `ConfigContext` (wrap `MivonConfig`), `loader.rs` (TOML/JSON), `validator.rs`, `cli.rs` (`EnvCliOptions` — CLI menang), `environment.rs` (`MIVON_*`).
- `crates/mivon-env/src/env/workspace/` — `WorkspaceContext` (`open`/`open_in`), `set_explicit_sources()` untuk seed dari CLI (menghindari scan direktori lambat), `filelist.rs`, `project.rs`, `include.rs`, `search.rs`.
- `crates/mivon-env/src/env/runtime/` — `RuntimeContext` (CPU/memori/threadpool/scheduler), `init(&config)` memakai `config.max_threads()`.
- `crates/mivon-env/src/env/compiler/` — `CompilerContext` (wrap `CompileSession`), helper `preprocess`/`lex`/`parse`/`elaborate`/`merge_all`/`OptimizeLevel`.
- `crates/mivon-env/src/env/database/` — `DatabaseContext` (wrap `MicdDatabase`: symbol/graph/metadata/diag accessor).
- `crates/mivon-env/src/env/{cache,diagnostics,telemetry,plugins,security,verification,simulation}/` — context lain sesuai doc.

Integrasi CLI: `main.rs` bangun env via `mivon::env::for_cli(cfgctx, ws)` (workspace di-seed dari CLI sources), threading ke `run()`/`run_fast()` untuk telemetry/metrics, `mivon::env::shutdown(&mut env)` di akhir. Pipeline compile/sim tidak diubah — env hanya shell orchestrator.

## Key conventions & gotchas

### Operator precedence (parser)
Higher number = tighter binding. `||`(1) < `&&`(2) < `|`(3) < `^`/`~^`(4) < `&`(5) < `<<`/`>>`(6) < `==`/`!=`/`===`(7) < `<`/`<=`/`>`/`>=`(8) < `+`/`-`(9) < `*`/`/`/`%`(10) < `**`(11). **Jangan balik** — higher-number = tighter-binding.

### Loop control flow
`control_flow: Option<FlowControl>` di `SimulationEngine`. Saat check `Continue`/`Break`, gunakan `let cf = self.control_flow.take()` **sekali**, lalu bandingkan `cf` — jangan panggil `take()` dua kali (nilai kedua selalu `None`). Check control_flow di setiap iterasi loop dan di awal setiap statement block.

### Fill literals (`'0`, `'1`, `'x`, `'z`)
Diexpand di `eval_assign_rhs()` (assignment level), bukan di `evaluate_expr()`, karena target width belum diketahui saat expression eval. `LogicVec::fill(val, width)` untuk membuat vector seragam.

### System functions
`$clog2`, `$bits`, `$size`, `$left`, `$right`, `$low`, `$high` dievaluasi di **elaborator** (compile-time) via constant folding. `$clog2` membutuhkan koreksi `is_power_of_two()` (jika power-of-two, hasil = msb - 1).

### `$display` format
`%0d` (zero-padded) **didukung penuh** — format `%0d`, `%0b`, `%0h` bekerja dengan zero-fill padding. Format yang tidak dikenal dicetak literal.

### Test pattern
Test menggunakan `simulate_signals(source, max_time)` yang mengembalikan `Vec<(String, LogicVec)>`. Cari signal dengan `.iter().find(|(n,_)| n == "name")`. Test suite terpadu ada di `crates/mivon-tests/` (ex `src/tests` + `edge_tests` + `debug_lexer`).

### Package support
`package`/`endpackage` + `import pkg::*` / `import pkg::item` di module body. Supports: `Typedef` (enum, struct, union, base) and `Param` (parameter/localparam with optional type keyword). Function/Task imports not yet supported.

### Fork/join support
`fork...join` / `join_any` / `join_none` untuk concurrent execution. Tiap branch berjalan independen, masing-masing dengan delay sendiri. Engine menggunakan `ForkGroup` untuk melacak branch aktif via `Continuation.fork_id`. `join` menunggu semua branch selesai; `join_any` lanjut saat branch pertama selesai; `join_none` lanjut segera. Branch yang berisi delay akan menjadwalkan kerja di masa depan, dan decrement `ForkGroup.remaining` saat semua statement branch habis dikonsumsi (tidak ada lagi delay).

### Constraint & randomize support
`rand`/`randc` modifier in class fields. `constraint name { expr; … }` blocks with relational/equality constraints.
`randomize()` uses rejection sampling (max 100 attempts) — generates random values for `rand` fields,
writes them into the object, and evaluates each constraint expression via `evaluate_ast_expr`.
User-defined `randomize()` methods override the built-in. `rand_fields` and `constraints` stored in
`IrClassDef` (cloned into `execute_randomize` to avoid borrow conflicts).

`.mivon` project file
File proyek mendaftar file `.sv` (satu per baris, `#` untuk komentar). Dibaca via `--filelist`/`-f` flag (sama dengan filelist `.f`). Path relatif terhadap direktori `.mivon`.

## Files
- `crates/mivon-simulator/src/simulator/engine/` — largest file area. Event loop, all statement handlers, loop unrolling, `$display`/`$fopen`/`$urandom`, fork/join tracking, `execute_randomize`, debug hook.
- `crates/mivon-parser/src/lib.rs` — second largest. Operator precedence table at line ~1968.
- `crates/mivon-elaboration/src/elaborator/mod.rs` — AST→IR translation, constant folding, signal resolution, multidimensional packed array support.
- `src/simulator/parallel.rs:448` — Parallel eval framework (rayon-based).
- `src/simulator/sdf.rs:369` — SDF annotation parser + annotator.
- `src/waveform/fst.rs:244` — FST waveform writer via wavefst crate.
- `src/debugger/mod.rs:585` — Debugger struct + 21 unit tests.

## Run
```shell
cargo run -- test/counter.sv              # single file              
cargo run -- test/tb_counter.sv -T 200    # max time
cargo run -- file.sv --ast                # print AST
cargo run -- file.sv --tokens             # print tokens
```

## Debug mode

Activate via `--debug` (basic) or `--deep-debug` (with snapshot/reverse). Debug types (`DebugMode`, `StepMode`, `Breakpoint`, `Watchpoint`) defined in `src/simulator/engine.rs`. `Debugger` wrapper in `src/debugger/mod.rs`.

### CLI flags
```shell
--debug                   # enable debug mode (pause at breakpoints)
--deep-debug              # enable + snapshots for reverse debug
--step                    # run one cycle then pause
--break-cycle <N>         # break at cycle N
--break-change <NAME>     # break when signal changes
--break-eq NAME=VAL       # break when signal == VAL (hex)
--watch <NAME>            # watchpoint (pause on change)
--timeline <NAME>         # print signal timeline post-sim
--timeline-len <N>        # max timeline entries (default 20)
--print-signal <NAME>     # print signal value post-sim
--print-state             # print all signal values
--tree                    # print hierarchy tree
--mem <ADDR> <LEN>        # memory inspector (hex)
--snap-interval <N>       # snapshot interval (default 1000)
```

### Breakpoint checking
`debug_check()` dipanggil di akhir setiap cycle (sebelum time increment) di `SimulationEngine::run()`. Cycle breakpoint `break cycle N` pause saat `state.time == N`. Signal breakpoint (`SignalEq`/`SignalNeq`/`SignalChange`) diperiksa setiap cycle via `signal_history`. Watchpoints juga diperiksa di sini — jika nilai berubah, engine pause dan event dicatat.

### Reverse debug (deep-debug)
Snapshot `StateSnapshot` disimpan setiap `snapshot_interval` cycle. `reverse_step()` pop snapshot terakhir dan restore state. `reverse_continue(target)` mundur ke snapshot terdekat ≤ target.

### Signal history
Semua signal dicatat di `signal_history: HashMap<String, Vec<(u64, LogicVec)>>` setiap cycle (maks 100k entry per signal). Dipakai oleh timeline, break-change, dan watchpoint.

## MICD — Mivon Incremental Compilation Database

Object database biner (bukan SQL) di `project/.mivon/database/` yang membuat compile
lintas run incremental: file yang tidak berubah tidak di-lex/di-parse/di-verifikasi
ulang. **Terintegrasi otomatis ke `run` dan `run_fast`** — bukan flag tambahan.

Struktur `src/micd/`:
- `format.rs` — format file `MDB1`: header 64B + object table + payload, dibaca via mmap (memmap2). Tulis atomik (temp+rename).
- `metadata.rs` — per-file: content hash (xxh3), mtime, size, status, flags hash, deps.
- `graph.rs` — dependency graph file-level + reverse index → `affected(changed)` transitive closure.
- `ast.rs` — `Design` terserialisasi per file via bincode (AST sudah `serde::Serialize/Deserialize`; `Symbol` diserialisasi sebagai string).
- `mod.rs` — layout Git-style (Opsi B db.md): `VERSION` + `registry.json` + `locks/` di root; payload IMMUTABLE content-addressed di `objects/<pid>/<hash>.ast|.preproc` (dedup antar file identik); index mutable di `state/<pid>/*.mdb`. Layout lama `projects/<pid>/` di-migrasi otomatis.
- `verify.rs` — verification cache keyed by content hash (parse/elab ok, diag counts, timing).
- `diag.rs`, `symbol.rs`, `snapshot.rs` — diagnostic per file, index simbol, snapshot build (build-NNN, rollback, di `state/<pid>/snapshots/`).

Integrasi:
- `CompileSession::attach_micd(MicdDatabase)` — restore `prev_designs`/`prev_checksums`/`prev_combined_sources` untuk file yang content hash-nya cocok → `compile()` melewati lexer+parser.
- `CompileSession::save_micd()` — simpan metadata/graph/ast-objek/preproc-objek/verify/symbol/types + auto-snapshot saat ada perubahan. Dipanggil `main.rs` sekali per build (bukan di `compile()` — agar statistik `changed_files` per-build akurat).
- `run_fast`: MICD penuh (restore AST). `run` (legacy): MICD preprocess cache (reuse combined source, skip preprocessor).
- Root: `.mivon/database` (override `MIVON_MICD_DIR`). `--recompile` melewati MICD (full rebuild). `--cache-clear` menghapus MICD.
