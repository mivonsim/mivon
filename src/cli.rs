//! CLI argument definitions for Mivon RTL Simulator.
//! Separated from main.rs for clarity.

use clap::Parser as ClapParser;
use clap::Subcommand;

/// Subcommands `mivon`.
#[derive(Subcommand, Clone)]
pub enum MivonCmd {
    /// Bersihkan database MICD (.mivon/database)
    Clean,

    // ── Tools terminal (tools.md) ──
    /// minspect — inspeksi struktur project (stats/modules/hierarchy/...)
    #[command(alias = "minspect")]
    Inspect(MinspectArgs),
    /// mlint — static RTL linter (unused signal, width, latch, loop, FSM)
    #[command(alias = "mlint")]
    Lint(MlintArgs),
    /// melab — standalone elaborator (parameter resolve, generate, hierarchy)
    #[command(alias = "melab")]
    Elab(MelabArgs),
    /// msim — simulator (VCD/FST/wave/assertion/coverage)
    #[command(alias = "msim")]
    Sim(MsimArgs),
    /// mcov — coverage analyzer → coverage.json / coverage.html
    #[command(alias = "mcov")]
    Cov(McovArgs),
    /// mwave — wave utility (merge/export/filter VCD)
    #[command(alias = "mwave")]
    Wave(MwaveArgs),
    /// mfmt — formatter Verilog/SystemVerilog
    #[command(alias = "mfmt")]
    Fmt(MfmtArgs),
    /// mgen — generate SystemVerilog (.sv/.svh) dari Mivon HDL (.mv/.mvh)
    #[command(alias = "mgen")]
    Gen(MgenArgs),
    /// mprof — performance profiler pipeline (lexer→parser→elab→sim)
    #[command(alias = "mprof")]
    Prof(MprofArgs),
    /// mcheck — project health checker (missing file, circular include, deps)
    #[command(alias = "mcheck")]
    Check(McheckArgs),
    /// mbench — benchmark tool (compile speed, memori, CPU, throughput)
    #[command(alias = "mbench")]
    Bench(MbenchArgs),
    /// synth — Mivon synthesis (RTL → SIR → netlist gate-level, SYNTHESIS.md)
    /// Nama lama: `msynth` (alias)
    #[command(name = "synth", alias = "msynth")]
    Synth(SynthArgs),
    /// mupdate — auto-update Mivon dari release resmi (manifest + checksum +
    /// rollback). `mivon update check` untuk deteksi, `mivon update` untuk
    /// memasang.
    #[command(alias = "mupdate")]
    Update(UpdateArgs),

    // ── Emulator (EMULATOR.md) — Hardware-Software Emulator ──
    /// emu — Mivon emulator; R0: MHIR extraction (device/register/clock/
    /// reset/memory + back-pointer) dan memory map dump
    #[command(name = "emu")]
    Emu(EmuArgs),

    // ── Additional tools (belum ada di CLI sebelumnya) ──
    /// mbatch — batch simulation runner (parallel/sequential jobs)
    #[command(alias = "mbatch")]
    Batch(MbatchArgs),
    /// mmemcheck — memory profiling (valgrind/heaptrack integration)
    #[command(alias = "mmemcheck")]
    Memcheck(MmemcheckArgs),
    /// mtbgen — automated testbench generation from module ports
    #[command(alias = "mtbgen")]
    Tbgen(MtbgenArgs),
    /// mwaiver — lint/formal violation waiver management
    #[command(alias = "mwaiver")]
    Waiver(MwaiverArgs),
    /// mvault — RTL secure vault (file locking, integrity, access control)
    #[command(alias = "mvault")]
    Vault(MvaultArgs),
    /// mipxact — IP-XACT (IEEE 1685) XML component packaging
    #[command(alias = "mipxact")]
    Ipxact(MipxactArgs),
    /// mdesign-repo — versioned design storage repository
    #[command(alias = "mdesign-repo")]
    DesignRepo(MdesignRepoArgs),
    /// mproject — multi-project workspace management
    #[command(alias = "mproject")]
    Project(MprojectArgs),
    /// msdc — SDC timing constraints parser
    #[command(alias = "msdc")]
    Sdc(MsdcArgs),
    /// mequiv — sequential equivalence checking
    #[command(alias = "mequiv")]
    EquivCheck(MequivCheckArgs),
    /// mregression — regression test management and analytics
    #[command(alias = "mregression")]
    Regression(MregressionArgs),
    /// meco — engineering change order (ECO) tracking
    #[command(alias = "meco")]
    Eco(MecoArgs),
    /// mcov-closure — coverage closure analytics
    #[command(alias = "mcov-closure")]
    CovClosure(McovClosureArgs),
}

/// minspect — Mivon Inspect.
#[derive(clap::Args, Clone)]
pub struct MinspectArgs {
    /// Target input: file .sv, direktori, atau file list .f
    /// Subcommand output (stats/modules/hierarchy/...) boleh diletakkan
    /// di posisi pertama (mis. `minspect stats rtl/`).
    #[arg(required = true)]
    pub targets: Vec<String>,

    /// Add include search path
    #[arg(short = 'I', long = "incdir", num_args = 1)]
    pub incdirs: Vec<String>,

    /// Define preprocessor macro
    #[arg(short = 'D', long = "define", num_args = 1)]
    pub defines: Vec<String>,

    /// Top module name
    #[arg(short = 't', long = "top")]
    pub top: Option<String>,

    /// Output sebagai JSON
    #[arg(long)]
    pub json: bool,
}

/// mlint — Static RTL Linter.
#[derive(clap::Args, Clone)]
pub struct MlintArgs {
    /// Target input: file .sv, direktori, atau file list .f
    #[arg(required = true)]
    pub targets: Vec<String>,

    /// Tambahkan include search path
    #[arg(short = 'I', long = "incdir", num_args = 1)]
    pub incdirs: Vec<String>,

    /// Define preprocessor macro
    #[arg(short = 'D', long = "define", num_args = 1)]
    pub defines: Vec<String>,

    /// Aktifkan semua check
    #[arg(long)]
    pub all: bool,

    /// Check: unused signal
    #[arg(long)]
    pub unused: bool,

    /// Check: width mismatch
    #[arg(long)]
    pub width: bool,

    /// Check: latch detection
    #[arg(long)]
    pub latch: bool,

    /// Check: combinational loop
    #[arg(long)]
    pub loop_check: bool,

    /// Check: FSM state register
    #[arg(long)]
    pub fsm: bool,

    /// Check: case analysis (parallel_case/full_case + missing default)
    #[arg(long)]
    pub case_analysis: bool,

    /// Check: clock gating inference (always_ff with if-gate pattern)
    #[arg(long)]
    pub clock_gating: bool,

    /// Check: power optimization (UPF power domain analysis)
    #[arg(long)]
    pub power: bool,

    /// Check: memory inference (RAM/ROM pattern detection)
    #[arg(long)]
    pub memory: bool,

    /// Check: gate-level optimization (redundant assignment detection)
    #[arg(long)]
    pub gate_opt: bool,

    /// Suppress output sukses
    #[arg(short = 'q', long)]
    pub quiet: bool,
}

/// melab — Standalone Elaborator.
#[derive(clap::Args, Clone)]
pub struct MelabArgs {
    /// Input file .sv (bisa lebih dari satu)
    #[arg(required = true)]
    pub files: Vec<String>,

    /// Top module name (default: module pertama)
    #[arg(short = 't', long = "top")]
    pub top: Option<String>,

    /// Cetak hierarchy tree
    #[arg(long)]
    pub tree: bool,

    /// Cetak parameter ter-resolve per module
    #[arg(long)]
    pub params: bool,

    /// Cetak sinyal per module
    #[arg(long)]
    pub signals: bool,

    /// SIM-22: analisis reset-domain crossing (RDC)
    #[arg(long = "reset-domain")]
    pub reset_domain: bool,

    /// Baca hasil elaborasi dari cache pipeline (db.md "5. elaborate/",
    /// "16. generate/") tanpa menjalankan elaborator — instance + port
    /// binding + parameter override + proses + net resolution + blok generate.
    #[arg(long = "from-cache")]
    pub from_cache: bool,

    /// Tambahkan include search path (identitas project cache)
    #[arg(short = 'I', long = "incdir", num_args = 1)]
    pub incdirs: Vec<String>,

    /// Define preprocessor macro (identitas project cache)
    #[arg(short = 'D', long = "define", num_args = 1)]
    pub defines: Vec<String>,
}

/// msim — Simulator.
#[derive(clap::Args, Clone)]
pub struct MsimArgs {
    /// Input file .sv (bisa lebih dari satu)
    #[arg(required = true)]
    pub files: Vec<String>,

    /// Maximum simulation time (default: unlimited — run until $finish/$fatal)
    #[arg(short = 'T', long = "max-time", alias = "time", value_name = "NS")]
    pub max_time: Option<u64>,

    /// Force simulation + VCD even when elaboration errors/skipped modules exist
    /// (default: Mivon refuses to simulate until the design elaborates cleanly)
    #[arg(long = "force-sim")]
    pub force_sim: bool,

    /// Top module name
    #[arg(short = 't', long = "top")]
    pub top: Option<String>,

    /// VCD/FST output file
    #[arg(short = 'o', long = "output")]
    pub output: Option<String>,

    /// Tulis FST waveform juga
    #[arg(long)]
    pub fst: bool,

    /// Cetak ringkasan assertion setelah simulasi
    #[arg(long)]
    pub assertions: bool,

    /// Cetak ringkasan coverage setelah simulasi
    #[arg(long)]
    pub coverage: bool,

    /// Tambahkan include search path
    #[arg(short = 'I', long = "incdir", num_args = 1)]
    pub incdirs: Vec<String>,

    /// Define preprocessor macro
    #[arg(short = 'D', long = "define", num_args = 1)]
    pub defines: Vec<String>,
}

/// mcov — Coverage Analyzer.
#[derive(clap::Args, Clone)]
pub struct McovArgs {
    /// Input file .sv (bisa lebih dari satu)
    #[arg(required = true)]
    pub files: Vec<String>,

    /// Maximum simulation time (default: unlimited — run until $finish/$fatal)
    #[arg(short = 'T', long = "max-time", alias = "time", value_name = "NS")]
    pub max_time: Option<u64>,

    /// Force simulation + VCD even when elaboration errors/skipped modules exist
    /// (default: Mivon refuses to simulate until the design elaborates cleanly)
    #[arg(long = "force-sim")]
    pub force_sim: bool,

    /// Top module name
    #[arg(short = 't', long = "top")]
    pub top: Option<String>,

    /// Prefix output (default: <top>). Menghasilkan <prefix>.coverage.json
    /// dan <prefix>.coverage.html
    #[arg(short = 'o', long = "output")]
    pub output: Option<String>,

    /// Tulis coverage.json
    #[arg(long)]
    pub json: bool,

    /// Tulis coverage.html
    #[arg(long)]
    pub html: bool,

    /// Threshold branch coverage (%) — exit error bila di bawah
    #[arg(long)]
    pub threshold: Option<f64>,

    /// Tambahkan include search path
    #[arg(short = 'I', long = "incdir", num_args = 1)]
    pub incdirs: Vec<String>,

    /// Define preprocessor macro
    #[arg(short = 'D', long = "define", num_args = 1)]
    pub defines: Vec<String>,
}

/// mwave — Wave Utility.
#[derive(clap::Args, Clone)]
pub struct MwaveArgs {
    /// Subcommand: merge, export, filter
    #[command(subcommand)]
    pub cmd: MwaveCmd,
}

/// mwave subcommand.
#[derive(Subcommand, Clone)]
pub enum MwaveCmd {
    /// Gabungkan beberapa VCD menjadi satu (offset waktu otomatis kumulatif)
    Merge {
        /// VCD input (2+)
        #[arg(required = true)]
        inputs: Vec<String>,
        /// Output VCD
        #[arg(short = 'o', long = "output")]
        output: Option<String>,
    },
    /// Ekspor VCD ke format lain (csv/txt)
    Export {
        /// VCD input
        #[arg(required = true)]
        input: String,
        /// Format output: csv (default) | txt
        #[arg(short = 'f', long = "format", default_value = "csv")]
        format: String,
        /// Output file
        #[arg(short = 'o', long = "output")]
        output: Option<String>,
    },
    /// Filter subset sinyal dari VCD
    Filter {
        /// VCD input
        #[arg(required = true)]
        input: String,
        /// Sinyal yang dipertahankan (koma/space terpisah)
        #[arg(required = true)]
        signals: Vec<String>,
        /// Output VCD
        #[arg(short = 'o', long = "output")]
        output: Option<String>,
    },
    /// Bandingkan dua VCD (perbedaan nilai per signal)
    Compare {
        /// VCD pertama
        #[arg(required = true)]
        a: String,
        /// VCD kedua
        #[arg(required = true)]
        b: String,
    },
    /// Cari sinyal by pola wildcard (* dan ?)
    Search {
        /// VCD input
        #[arg(required = true)]
        input: String,
        /// Pola (koma/space terpisah, dukung * dan ?)
        #[arg(required = true)]
        patterns: Vec<String>,
    },
    /// Index hierarki scope + sinyal
    Tree {
        /// VCD input
        #[arg(required = true)]
        input: String,
    },
    /// Statistik aktivitas per sinyal (toggle, transitions, activity%)
    Stats {
        /// VCD input
        #[arg(required = true)]
        input: String,
    },
    /// Query nilai sinyal dari VCD — random access (WAV-07)
    Get {
        /// VCD input
        #[arg(required = true)]
        input: String,
        /// Sinyal yang di-query (koma/space terpisah, dukung * dan ?)
        #[arg(required = true)]
        signals: Vec<String>,
        /// Nilai pada waktu T (perubahan terakhir ≤ T)
        #[arg(long)]
        at: Option<u64>,
        /// Rentang waktu t1:t2 (semua perubahan dalam [t1, t2])
        #[arg(long = "range", value_parser = parse_time_range)]
        range: Option<(u64, u64)>,
    },
    /// Decode transaksi protokol bus dari VCD (apb/axi4lite/ahb) — WAV-16
    Decode {
        /// VCD input
        #[arg(required = true)]
        input: String,
        /// Protokol: apb (default) | axi4lite | ahb
        #[arg(long = "proto", default_value = "apb")]
        proto: String,
    },
}

/// Parser argumen `--range t1:t2` untuk `mwave get`.
fn parse_time_range(s: &str) -> Result<(u64, u64), String> {
    let (a, b) = s
        .split_once(':')
        .ok_or_else(|| format!("format range harus t1:t2, dapat: '{}'", s))?;
    let lo: u64 = a
        .trim()
        .parse()
        .map_err(|_| format!("t1 bukan angka: '{}'", a.trim()))?;
    let hi: u64 = b
        .trim()
        .parse()
        .map_err(|_| format!("t2 bukan angka: '{}'", b.trim()))?;
    if lo > hi {
        return Err(format!("t1 ({}) tidak boleh > t2 ({})", lo, hi));
    }
    Ok((lo, hi))
}

/// mfmt — Formatter.
#[derive(clap::Args, Clone)]
pub struct MfmtArgs {
    /// File .sv untuk diformat
    #[arg(required = true)]
    pub files: Vec<String>,

    /// Tulis kembali ke file (tanpa ini hanya ke stdout)
    #[arg(short = 'i', long)]
    pub inplace: bool,

    /// Lebar indentasi (default: 4)
    #[arg(long, default_value = "4")]
    pub indent: usize,

    /// Periksa saja — report file yang berbeda format (exit 1 bila ada)
    #[arg(long)]
    pub check: bool,
}

/// mgen — Generator SystemVerilog dari Mivon HDL (.mv/.mvh).
#[derive(clap::Args, Clone)]
pub struct MgenArgs {
    /// Input: file .mv/.mvh atau direktori (recursive scan *.mv, *.mvh)
    #[arg(required = true)]
    pub targets: Vec<String>,

    /// Direktori output (default: di samping file input)
    #[arg(short = 'o', long = "output")]
    pub output: Option<String>,

    /// Print hasil ke stdout (debug, satu file saja; .mv → .sv, .mvh → .svh)
    #[arg(long)]
    pub stdout: bool,

    /// Verifikasi output up-to-date — exit 1 bila ada file yang beda (CI)
    #[arg(long)]
    pub check: bool,

    /// Hanya generate .svh
    #[arg(long = "svh-only", alias = "emit-svh-only")]
    pub svh_only: bool,

    /// Lewati type-check (E2001–E2007) — untuk konstruk eksternal yang
    /// belum dipahami checker
    #[arg(long)]
    pub no_check: bool,

    /// Hanya generate .sv
    #[arg(long = "sv-only", alias = "emit-sv-only")]
    pub sv_only: bool,

    /// Bungkus typedef level file dalam package bernama ini (di `.svh`),
    /// lalu module dapat otomatis `import <nama>::*;`
    #[arg(long, value_name = "NAME")]
    pub package: Option<String>,

    /// Report per-file yang diproses
    #[arg(long)]
    pub verbose: bool,
}

/// mprof — Performance Profiler.
#[derive(clap::Args, Clone)]
pub struct MprofArgs {
    /// Target input: file .sv, direktori, atau file list .f
    #[arg(required = true)]
    pub targets: Vec<String>,

    /// Maximum simulation time (default: unlimited — run until $finish/$fatal)
    #[arg(short = 'T', long = "max-time", alias = "time", value_name = "NS")]
    pub max_time: Option<u64>,

    /// Force simulation + VCD even when elaboration errors/skipped modules exist
    /// (default: Mivon refuses to simulate until the design elaborates cleanly)
    #[arg(long = "force-sim")]
    pub force_sim: bool,

    /// Top module name
    #[arg(short = 't', long = "top")]
    pub top: Option<String>,

    /// Baca profil build terakhir dari cache pipeline (db.md "20. profile/") —
    /// tanpa compile/simulasi. Menampilkan bottleneck + rekomendasi.
    #[arg(long = "cached")]
    pub cached: bool,

    /// Tambahkan include search path
    #[arg(short = 'I', long = "incdir", num_args = 1)]
    pub incdirs: Vec<String>,

    /// Define preprocessor macro
    #[arg(short = 'D', long = "define", num_args = 1)]
    pub defines: Vec<String>,
}

/// mcheck — Project Health Checker.
#[derive(clap::Args, Clone)]
pub struct McheckArgs {
    /// Target input: file .sv, direktori, atau file list .f
    #[arg(required = true)]
    pub targets: Vec<String>,

    /// Aktifkan semua check
    #[arg(long)]
    pub all: bool,

    /// Check: missing file (`include / file list)
    #[arg(long)]
    pub missing: bool,

    /// Check: circular include
    #[arg(long)]
    pub circular: bool,

    /// Check: unresolved dependency module
    #[arg(long)]
    pub deps: bool,

    /// Check: module instantiation cycle
    #[arg(long)]
    pub cycles: bool,

    /// Check: inkonsistensi timescale
    #[arg(long)]
    pub timescale: bool,

    /// bandingkan AST file pertama dgn file kedua (structural
    /// diff utk regression) — `mcheck a.sv --ast-diff b.sv`.
    #[arg(long = "ast-diff")]
    pub ast_diff: Option<String>,

    /// ENT-22: Check version compatibility — detect SV features used
    /// vs supported by Mivon. Reports feature usage and support status.
    #[arg(long = "sv-version")]
    pub sv_version: bool,
}

/// mbench — Benchmark Tool.
#[derive(clap::Args, Clone)]
pub struct MbenchArgs {
    /// Target input: file .sv, direktori, atau file list .f
    #[arg(required = true)]
    pub targets: Vec<String>,

    /// Jumlah run (default: 3)
    #[arg(short = 'n', long, default_value = "3")]
    pub runs: usize,

    /// Tambahkan include search path
    #[arg(short = 'I', long = "incdir", num_args = 1)]
    pub incdirs: Vec<String>,

    /// Define preprocessor macro
    #[arg(short = 'D', long = "define", num_args = 1)]
    pub defines: Vec<String>,
}

/// synth — Mivon Synthesis.
#[derive(clap::Args, Clone)]
pub struct SynthArgs {
    /// Target input: file .sv, direktori, atau file list (.f/.mivon)
    #[arg(required = true)]
    pub targets: Vec<String>,

    /// Top module name
    #[arg(short = 't', long = "top")]
    pub top: Option<String>,

    /// Prefix output (default: nama top) → <prefix>.mvnet dst.
    #[arg(short = 'o', long = "output")]
    pub output: Option<String>,

    /// Hanya analisis sintesizability (SYN-1..9), tanpa netlist.
    #[arg(long = "check-only")]
    pub check_only: bool,

    /// Device target: fpga-x7 (default) | generic
    #[arg(long, default_value = "fpga-x7")]
    pub device: String,

    /// Preset pipeline synthesis: generic | fpga (default) | asic | custom.
    /// Phase 2: memilih pipeline pass optimizer (mapping menyusul).
    #[arg(long, default_value = "fpga")]
    pub preset: String,

    /// Tulis netlist `.mvnet`
    #[arg(long = "emit-mvnet")]
    pub emit_mvnet: bool,

    /// Tulis dump SIR (Synthesis Intermediate Representation, node-based)
    /// ke stdout — debugging fase RTL→SIR (SYNTHESIS.md §3).
    #[arg(long = "dump-sir")]
    pub dump_sir: bool,

    /// Tulis dump SIR SETELAH pass optimizer (const fold/DCE/CSE/mux/arith)
    #[arg(long = "dump-sir-opt")]
    pub dump_sir_opt: bool,

    /// Dump netlist generik hasil mapping SIR (netlist.v + .mvnet) ke stdout
    /// — debugging fase SIR→netlist (SYNTHESIS.md §13, phase 3).
    #[arg(long = "dump-netlist")]
    pub dump_netlist: bool,

    /// Emisi netlist ke file: <prefix>.netlist.v + <prefix>.netlist.json +
    /// <prefix>.netlist.mvnet (prefix default: nama top).
    #[arg(long = "emit-netlist")]
    pub emit_netlist: bool,

    /// Tech mapping (phase 4): LUT cut + AIG dekomposisi + carry chain →
    /// <prefix>.tech.v/.json/.mvnet + report LUT/CARRY4/FF.
    #[arg(long = "tech-map")]
    pub tech_map: bool,

    /// Tulis report utilisasi ke file (tanpa ini: ke stdout)
    #[arg(long = "report-util")]
    pub report_util: Option<String>,

    /// File constraint `.mcs` (clock period, IO delay, false/multicycle path)
    /// — dipakai `--timing`. Default tanpa file: period 10ns, delay 0.
    #[arg(long = "constraint")]
    pub constraint: Option<String>,

    /// Static timing + area analysis (phase 5): hitung arrival/required/
    /// slack/WNS/TNS + critical path atas netlist (tech bila `--tech-map`,
    /// else generic). Tulis `<prefix>.timing.rpt` + `<prefix>.area.rpt`.
    #[arg(long = "timing")]
    pub timing: bool,

    /// Laporan FSM extraction (deteksi state register + transisi)
    #[arg(long = "fsm-report")]
    pub fsm_report: bool,

    /// Tambahkan include search path
    #[arg(short = 'I', long = "incdir", num_args = 1)]
    pub incdirs: Vec<String>,

    /// Define preprocessor macro
    #[arg(short = 'D', long = "define", num_args = 1)]
    pub defines: Vec<String>,

    /// Suppress output
    #[arg(short = 'q', long)]
    pub quiet: bool,
}

/// mupdate — Mivon auto-update.
#[derive(clap::Args, Clone)]
pub struct UpdateArgs {
    /// Subcommand opsional: `check` (lapor saja) | `update` (pasang)
    #[command(subcommand)]
    pub cmd: Option<UpdateCmd>,

    /// Kanal rilis: stable (default) | beta
    #[arg(long, default_value = "stable")]
    pub channel: String,

    /// Pasang versi spesifik (mis. 0.4.0) dari release resmi v<version>
    #[arg(long)]
    pub version: Option<String>,

    /// Kembalikan ke binary sebelumnya (backup terakhir)
    #[arg(long)]
    pub rollback: bool,

    /// Lewati konfirmasi
    #[arg(short = 'y', long)]
    pub yes: bool,

    /// Override URL manifest — hook uji
    #[arg(long, value_name = "URL", hide = true)]
    pub manifest_url: Option<String>,
}

/// mupdate subcommand.
#[derive(Subcommand, Clone)]
pub enum UpdateCmd {
    /// Cek ketersediaan update (tanpa mengubah apa pun)
    Check,
    /// Pasang update terbaru
    #[command(alias = "apply")]
    Update,
}

/// emu — Mivon emulator (EMULATOR.md). R0: MHIR extraction + memory map dump.
#[derive(clap::Args, Clone)]
pub struct EmuArgs {
    /// Target input: file .sv, direktori, atau file list (.f/.mivon)
    /// (opsional bila `--boot-iso` dipakai — boot ISO tidak butuh RTL)
    pub targets: Vec<String>,

    /// Tambahkan include search path
    #[arg(short = 'I', long = "incdir", num_args = 1)]
    pub incdirs: Vec<String>,

    /// Define preprocessor macro
    #[arg(short = 'D', long = "define", num_args = 1)]
    pub defines: Vec<String>,

    /// Top module name
    #[arg(short = 't', long = "top")]
    pub top: Option<String>,

    /// File konfigurasi emulator TOML (`.meu`) — top/mode/accuracy/cpu/ram/
    /// devices/seed. File TERPISAH dari project `.mivon` (MICD memakai
    /// ekstensi/direktori .mivon — tidak boleh bentrok).
    #[arg(long = "config")]
    pub config: Option<String>,

    /// Cetak MHIR lengkap (device/register/clock/reset/memory + back-pointer)
    #[arg(long)]
    pub dump_mhir: bool,

    /// Cetak memory map (region ber-alamat; tanpa --addr, region TBD)
    #[arg(long)]
    pub dump_memory_map: bool,

    /// Assign alamat region: NAME=BASE:SIZE (hex, bisa diulang).
    /// Cocok dengan nama instance device, nama module device, atau nama
    /// memory — mis. `--addr u_uart=0x10000000:0x1000`.
    #[arg(long = "addr", value_name = "NAME=BASE:SIZE")]
    pub addr: Vec<String>,

    /// Muat kernel ELF ke memory map (butuh `[emu] ram` di project file .mivon).
    /// Cetak entry point + isi region.
    #[arg(long = "load-elf", value_name = "PATH")]
    pub load_elf: Option<String>,

    /// Hex dump memori guest: ADDR:LEN (hex) — mis. `0x80000000:64`.
    #[arg(long = "dump-memory", value_name = "ADDR:LEN")]
    pub dump_memory: Option<String>,

    /// Jalankan CPU dari RTL (.sv/.v) — Direct RTL CPU (EMULATOR.md §7.2
    /// mode 3), BUKAN interpreter Rust. Flag diulang per file (wrapper + core,
    /// mis. `--rtl-cpu rv32_bus_wrapper.sv --rtl-cpu picorv32.v`). RTL wajib
    /// memenuhi kontrak bus picorv32-style: clk/resetn/mem_valid/mem_instr/
    /// mem_addr/mem_wdata/mem_wstrb/mem_ready/mem_rdata/trap.
    #[arg(long = "rtl-cpu", value_name = "FILE", action = clap::ArgAction::Append)]
    pub rtl_cpu: Vec<String>,

    /// Top module CPU RTL (default: rv32_bus_wrapper)
    #[arg(long = "rtl-cpu-top", value_name = "TOP")]
    pub rtl_cpu_top: Option<String>,

    /// Jalankan mesin (CPU RTL + RAM) sampai trap / --max-steps
    #[arg(long)]
    pub run: bool,

    /// Batas langkah instruksi mesin (default 10000)
    #[arg(long = "max-steps", value_name = "N")]
    pub max_steps: Option<u64>,

    /// Boot ISO x86 real-mode (R6): muat MBR ke 0x7c00, jalankan interpreter
    /// x86 real-mode dengan ISO sebagai disk (INT 13h). Jalur boot BIOS:
    /// MBR (ISOLINUX hybrid) → INT 13h → El Torito → GRUB boot.img.
    /// Butuh `ram = { base, size }` di config .meu (minimal 0x0:0x100000).
    #[arg(long = "boot-iso", value_name = "PATH")]
    pub boot_iso: Option<String>,

    /// Tampilkan jendela grafis untuk emulator (framebuffer display).
    /// Mode default: text console 80x25 (VGA text mode).
    /// Format: WIDTHxHEIGHT (default: 80x25) atau `--window` (80x25 default).
    #[arg(long = "window", value_name = "WxH")]
    pub window: Option<String>,

    /// Simpan snapshot mesin (CPU + seluruh memori guest sparse) ke file
    /// SETELAH run — EMULATOR.md §14 (fase R5). Didukung: interpreter RV32,
    /// x86 (boot ISO). Direct RTL CPU → error sebelum run (belum didukung).
    #[arg(long = "snapshot-save", value_name = "PATH")]
    pub snapshot_save: Option<String>,

    /// Muat snapshot mesin SEBELUM run — lanjut dari state tersimpan
    /// (memory map harus identik: nama/base/size region; blob CPU harus
    /// cocok dengan CPU yang dipakai).
    #[arg(long = "snapshot-load", value_name = "PATH")]
    pub snapshot_load: Option<String>,
}

#[derive(ClapParser, Clone)]
#[command(name = "mivon", about = "RTL Simulator untuk SystemVerilog")]
pub struct Cli {
    /// Subcommand (opsional)
    #[command(subcommand)]
    pub cmd: Option<MivonCmd>,

    /// Input SystemVerilog file(s) — last is top module
    pub files: Vec<String>,

    /// Top module name (default: first module)
    #[arg(short = 't', long = "top")]
    pub top: Option<String>,

    /// Maximum simulation time (default: unlimited — run until $finish/$fatal)
    #[arg(short = 'T', long = "max-time", alias = "time", value_name = "NS")]
    pub max_time: Option<u64>,

    /// Force simulation + VCD even when elaboration errors/skipped modules exist
    /// (default: Mivon refuses to simulate until the design elaborates cleanly)
    #[arg(long = "force-sim")]
    pub force_sim: bool,

    /// VCD/FST output file (default: <module>.vcd)
    #[arg(short = 'o', long = "output")]
    pub output: Option<String>,

    /// Enable waveform streaming to disk (flush after each time step)
    /// Allows external tools to read waveform during simulation.
    #[arg(long = "waveform-stream")]
    pub waveform_stream: bool,

    /// Write VCD via background writer thread (WAV-19) — dump tidak
    /// blocking simulasi; byte dikirim via channel ke thread penulis.
    #[arg(long = "waveform-bg")]
    pub waveform_bg: bool,

    /// WAV-04: Enable gzip compression for VCD output (.vcd.gz)
    #[arg(long = "waveform-gzip")]
    pub waveform_gzip: bool,

    /// Launch the native GUI (egui) — requires --features gui
    #[arg(long = "gui")]
    pub gui: bool,

    /// Add include search path
    #[arg(short = 'I', long = "incdir", num_args = 1)]
    pub incdirs: Vec<String>,

    /// Define preprocessor macro (NAME or NAME=VALUE)
    #[arg(short = 'D', long = "define", num_args = 1)]
    pub defines: Vec<String>,

    /// Read file list from file
    #[arg(short = 'f', long = "filelist")]
    pub filelist: Option<String>,

    /// Jangan tulis VCD waveform (hemat I/O & waktu sim desain besar —
    /// VCD top_earlgrey 57k sinyal × delta tertulis ke disk).
    #[arg(long = "no-vcd")]
    pub no_vcd: bool,

    /// Load configuration from TOML file (configs/*.toml).
    /// Default: configs/compiler.toml bila ada.
    #[arg(long = "config")]
    pub config: Option<String>,

    /// Pass plusarg (NAME=VALUE)
    #[arg(long = "plusarg", num_args = 1)]
    pub plusargs: Vec<String>,

    /// Dump all signal values at each timestep
    #[arg(long = "dump-all")]
    pub dump_all: bool,

    /// Print tokens before parsing
    #[arg(long = "tokens")]
    pub print_tokens: bool,

    /// Print AST after parsing
    #[arg(long = "ast")]
    pub print_ast: bool,

    // ── Debug flags ──
    /// Enable debug mode (pause at breakpoints/watchpoints)
    #[arg(long = "debug")]
    pub debug: bool,

    /// Enable deep debug mode (with snapshot for reverse debugging)
    #[arg(long = "deep-debug")]
    pub deep_debug: bool,

    /// Single-step mode: run one cycle then pause
    #[arg(long = "step")]
    pub step: bool,

    /// Set breakpoint on cycle number
    #[arg(long = "break-cycle")]
    pub break_cycle: Vec<u64>,

    /// Set breakpoint on signal change (NAME)
    #[arg(long = "break-change")]
    pub break_change: Vec<String>,

    /// Set breakpoint on signal equality: NAME=VALUE (hex)
    #[arg(long = "break-eq")]
    pub break_eq: Vec<String>,

    /// Set watchpoint on signal name
    #[arg(long = "watch")]
    pub watch: Vec<String>,

    /// Print hierarchy tree after elaboration
    #[arg(long = "tree")]
    pub print_tree: bool,

    /// Print signal value after simulation
    #[arg(long = "print-signal")]
    pub print_signal: Vec<String>,

    /// Print all signal values after simulation
    #[arg(long = "print-state")]
    pub print_state: bool,

    /// Print timeline for signal after simulation
    #[arg(long = "timeline")]
    pub timeline: Vec<String>,

    /// Inspect memory at address with length
    #[arg(long = "mem", num_args = 2)]
    pub mem: Vec<String>,

    /// Snapshot interval for reverse debug (default: 1000)
    #[arg(long = "snap-interval", default_value = "1000")]
    pub snap_interval: u64,

    /// Print timeline entries count
    #[arg(long = "timeline-len", default_value = "20")]
    pub timeline_len: usize,

    /// Export coverage to UCIS XML file (default: <module>.ucis.xml)
    #[arg(long = "coverage-ucis")]
    pub coverage_ucis: Option<String>,

    /// Library directory to search for missing modules (-y <dir>)
    #[arg(short = 'y', long = "libdir", num_args = 1)]
    pub libdirs: Vec<String>,

    /// Library file containing one or more modules (-v <file>)
    #[arg(short = 'v', long = "libfile", num_args = 1)]
    pub libfiles: Vec<String>,

    /// Suppress preprocessor warnings (missing include files, etc.)
    #[arg(short = 'q', long = "quiet")]
    pub quiet: bool,

    /// X-propagation mode: optimistic, pessimistic, or x-anywhere
    #[arg(long = "xprop", default_value = "pessimistic")]
    pub xprop: String,

    /// Compile-only mode: parse + elaborate, skip simulation & VCD
    #[arg(long = "compile-only")]
    pub compile_only: bool,

    /// Use fast parallel pipeline (CompileSession + FastLexer)
    #[arg(long = "fast")]
    pub fast: bool,

    /// Use legacy lexer (char-based, default with new pipeline)
    #[arg(long = "legacy-lexer")]
    pub legacy_lexer: bool,

    /// Cache stats (show AST/HIR cache hit rates after run)
    #[arg(long = "cache-stats")]
    pub cache_stats: bool,

    /// Tampilkan Global Diagnostic Engine report (semua diagnostic lintas
    /// komponen, coverage posisi + registry error code) setelah run.
    #[arg(long = "gdiag")]
    pub gdiag: bool,

    /// Save checksums to file for change detection across runs
    #[arg(long = "checksum-file")]
    pub checksum_file: Option<String>,

    /// Enable profiling (show phase timings and counters)
    #[arg(long = "profile")]
    pub profile: bool,

    /// Print simulation performance dashboard (delta cycles, events, throughput)
    #[arg(long = "perf-dashboard")]
    pub perf_dashboard: bool,

    /// Force full recompile (ignore cache)
    #[arg(long = "recompile")]
    pub recompile: bool,

    /// Use lazy elaboration (HIR-based, on-demand)
    #[arg(long = "lazy")]
    pub lazy: bool,

    /// Use packed 4-state eval (SIMD-ready bitmask ops for bitwise operations)
    #[arg(long = "packed", short = 'P')]
    pub packed: bool,

    /// Use DAG-parallel process evaluation (parallel simulation via rayon)
    #[arg(long = "parallel", short = 'L')]
    pub parallel: bool,

    /// Use cycle-based simulation fusion (clock-gated domain fusion)
    #[arg(long = "cycle-fusion")]
    pub cycle_fusion: bool,

    /// Enable body-level MIR JIT (compiled-code simulation for combinational processes)
    #[arg(long = "jit-body")]
    pub jit_body: bool,

    /// Enable formal verification with Z3 (Bounded Model Checking)
    #[arg(long = "formal")]
    pub formal: bool,

    /// Maximum unrolling bound for BMC (default: 20)
    #[arg(long = "formal-bound", default_value = "20")]
    pub formal_bound: u64,

    /// Cycle-based simulation mode (SIM-20 tahap 1): eksekusi synchronous
    /// murni tanpa event queue — semua comb dieval per cycle, semua FF
    /// commit NBA sekali per edge.
    #[arg(long = "cycle")]
    pub cycle_mode: bool,

    /// Periode clock (unit waktu desain) untuk --cycle (default 10).
    #[arg(long = "cycle-period", default_value = "10")]
    pub cycle_period: u64,

    /// Enable k-induction proof after BMC (FORMAL-03/04) — iterasi k=1..8;
    /// UNSAT di kedalaman k → invariant terbukti untuk SEMUA depth
    #[arg(long = "formal-induction")]
    pub formal_induction: bool,

    /// Connectivity check (FORMAL-13): pasangan sinyal "src,dst" — cek
    /// jalur kombinational dari src ke dst (boleh diulang)
    #[arg(long = "formal-connect")]
    pub formal_connect: Vec<String>,

    /// DPI shared library to load (can be specified multiple times)
    #[arg(long = "dpi-lib", num_args = 1)]
    pub dpi_libs: Vec<String>,

    /// VHPI (IEEE 1076-2008) shared library to load — ABI-compatible adapter
    /// (can be specified multiple times). Loading memakai feature "dpi"
    /// (libloading); tanpa feature → error jelas saat --vhpi dipakai.
    #[arg(long = "vhpi", num_args = 1)]
    pub vhpi_libs: Vec<String>,

    /// PLI (IEEE 1364) shared library to load — ABI-compatible adapter
    /// (can be specified multiple times).
    #[arg(long = "pli", num_args = 1)]
    pub pli_libs: Vec<String>,

    /// Save simulation checkpoint to file (after sim, or at --break-cycle)
    #[arg(long = "save")]
    pub save: Option<String>,

    /// Restore simulation checkpoint from file (overrides initial state)
    #[arg(long = "restore")]
    pub restore: Option<String>,

    /// SIM-18: auto-checkpoint (crash recovery) — simpan state tiap interval
    /// cycle ke file; run yang crash bisa di-resume dari titik terakhir.
    #[arg(long = "auto-checkpoint")]
    pub auto_checkpoint: Option<String>,

    /// Interval (cycle) auto-checkpoint (default 1000)
    #[arg(long = "checkpoint-interval", default_value_t = 1000)]
    pub checkpoint_interval: u64,

    /// Enable signal history disk spill (path to spill file)
    #[arg(long = "signal-history-spill")]
    pub signal_history_spill: Option<String>,

    /// Coverage database file path (UCDB binary format, auto-merge on load)
    #[arg(long = "coverage-ucdb")]
    pub coverage_ucdb: Option<String>,

    /// Run as LSP server (stdin/stdout JSON-RPC transport)
    #[arg(long = "lsp")]
    pub lsp: bool,

    /// SDF annotation file path (Standard Delay Format for gate-level timing)
    #[arg(long = "sdf")]
    pub sdf: Option<String>,

    /// Minimum branch coverage threshold (%). Exit with error if below.
    #[arg(long = "coverage-threshold")]
    pub coverage_threshold: Option<f64>,

    /// Export coverage report as HTML file
    #[arg(long = "coverage-html")]
    pub coverage_html: Option<String>,

    /// Co-simulation port (TCP) for VHDL/SystemVerilog co-simulation bridge
    #[arg(long = "cosim-port")]
    pub cosim_port: Option<u16>,

    /// Comma-separated list of signal names to expose for co-simulation
    #[arg(long = "cosim-signals")]
    pub cosim_signals: Option<String>,

    /// SDF timing mode: min, typ (default), or max
    #[arg(long = "timing-mode", default_value = "typ")]
    pub timing_mode: String,

    /// UPF (Unified Power Format) file for power-aware simulation
    #[arg(long = "upf")]
    pub upf: Option<String>,

    /// Remote cache directory (filesystem-based, shared across CI builds)
    /// Content-addressed: entries stored by checksum under this directory.
    /// Use a shared NFS volume or CI artifact mount for cross-build caching.
    #[arg(long = "cache-remote-dir")]
    pub cache_remote_dir: Option<String>,

    /// Remote cache sync mode: none, manual, read-through, write-through, or read-write
    /// Default: read-write (auto-sync on both miss and insert)
    #[arg(long = "cache-remote-sync", default_value = "read-write")]
    pub cache_remote_sync: String,

    /// Clear all cache (local + remote) before starting
    #[arg(long = "cache-clear")]
    pub cache_clear: bool,

    /// Target directory for build artifacts (precompiled analysis, cache).
    /// Similar to Rust's `target/` — all build output goes here.
    /// Default: `.mivon/database/`. Override for cross-project sharing.
    #[arg(long = "target-dir")]
    pub target_dir: Option<String>,

    /// CSV waveform output file path (signal values as comma-separated values)
    /// Compatible with spreadsheet tools and the built-in HTML viewer.
    #[arg(long = "waveform-csv")]
    pub waveform_csv: Option<String>,

    /// Generate GTKWave save file (.gtkw) for the waveform output
    /// Automatically referenced to VCD/FST path. Pass path or just `--gtkw` for auto-name.
    #[arg(long = "gtkw")]
    pub gtkw: Option<String>,

    /// Write signal statistics report (toggle counts, transitions, activity)
    /// Useful for power estimation and signal activity analysis.
    #[arg(long = "signal-stats")]
    pub signal_stats: Option<String>,

    /// Generate standalone HTML waveform viewer (requires --waveform-csv)
    /// The HTML file includes inline CSS+JS, open in browser to view CSV data.
    #[arg(long = "waveform-html-viewer")]
    pub waveform_html_viewer: Option<String>,

    /// CDC (Clock-Domain Crossing) analysis report file
    /// Detects and reports unsynchronized signal crossings between clock domains.
    #[arg(long = "cdc-report")]
    pub cdc_report: Option<String>,

    /// Run as distributed master node (coordinate partitions)
    #[arg(long = "dist-master")]
    pub dist_master: bool,

    /// Run as distributed slave node (connect to master)
    #[arg(long = "dist-slave")]
    pub dist_slave: bool,

    /// Distributed simulation port (default: 9876)
    #[arg(long = "dist-port", default_value = "9876")]
    pub dist_port: u16,

    /// Number of partitions for distributed simulation (master)
    #[arg(long = "num-partitions", default_value = "1")]
    pub num_partitions: usize,

    /// Master host for distributed slave
    #[arg(long = "master-host", default_value = "127.0.0.1")]
    pub master_host: String,

    /// Use hierarchical timing wheel for O(1) event scheduling.
    /// Replaces Vec<Vec<RegionEvent>> with a 3-level timing wheel
    /// that eliminates O(E) event filtering per delta cycle.
    #[arg(long = "use-timing-wheel")]
    pub use_timing_wheel: bool,

    /// Glitch detection window (in time units). 0 = disabled (default).
    /// Detects A→B→A pulses where a signal reverts to its previous value
    /// within this window and reports a WR0302 warning.
    #[arg(long = "glitch-window", default_value = "0")]
    pub glitch_window: u64,

    /// (Internal) Mode elaborasi dari config TOML (`[elaborate] mode`).
    /// Tidak di-parse dari CLI — diisi oleh `apply_config_to_cli` di main.rs.
    #[arg(skip)]
    pub config_elab_mode: Option<String>,

    /// Suppress all warnings (WR/RT9xxx codes)
    #[arg(long = "no-warn")]
    pub no_warn: bool,

    /// Suppress specific warning codes (comma-separated, e.g. WR0102,RT9003)
    #[arg(long = "allow", num_args = 1)]
    pub allow_codes: Vec<String>,

    /// Stop after N warnings (default: unlimited)
    #[arg(long = "max-warnings", value_name = "N")]
    pub max_warnings: Option<usize>,
}

// ══════════════════════════════════════════════════════════════════════
// Additional tools — baru ditambahkan ke CLI
// ══════════════════════════════════════════════════════════════════════

/// mbatch — Batch simulation runner.
#[derive(clap::Args, Clone)]
pub struct MbatchArgs {
    /// Subcommand: run, status, summary
    #[command(subcommand)]
    pub cmd: MbatchCmd,
}

/// mbatch subcommand.
#[derive(Subcommand, Clone)]
pub enum MbatchCmd {
    /// Jalankan batch jobs dari file TOML
    Run {
        /// Batch config file (TOML)
        #[arg(required = true)]
        config: String,
    },
    /// Tampilkan status batch terakhir
    Status,
    /// Tampilkan ringkasan batch
    Summary,
}

/// mmemcheck — Memory profiling (valgrind/heaptrack).
#[derive(clap::Args, Clone)]
pub struct MmemcheckArgs {
    /// Tool: valgrind (default) atau heaptrack
    #[arg(long, default_value = "valgrind")]
    pub tool: String,
    /// Binary path untuk di-profile
    #[arg(required = true)]
    pub binary: String,
    /// Arguments untuk binary
    #[arg(last = true)]
    pub args: Vec<String>,
}

/// mtbgen — Testbench generation.
#[derive(clap::Args, Clone)]
pub struct MtbgenArgs {
    /// Module name untuk testbench
    #[arg(short = 'm', long = "module")]
    pub module: Option<String>,
    /// Output file (default: stdout)
    #[arg(short = 'o', long = "output")]
    pub output: Option<String>,
    /// Clock name (default: clk)
    #[arg(long = "clock", default_value = "clk")]
    pub clock_name: String,
    /// Clock period (default: 10)
    #[arg(long = "period", default_value = "10")]
    pub clock_period: u32,
    /// Reset name (default: rst_n)
    #[arg(long = "reset", default_value = "rst_n")]
    pub reset_name: String,
    /// Reset active high (default: active low)
    #[arg(long = "reset-high")]
    pub reset_high: bool,
    /// Simulation time (default: 1000)
    #[arg(long = "sim-time", default_value = "1000")]
    pub sim_time: u32,
    /// Include waveform dump
    #[arg(long = "dump-vcd", default_value = "true")]
    pub dump_vcd: bool,
    /// Include basic checks
    #[arg(long = "checks", default_value = "true")]
    pub include_checks: bool,
    /// Input ports: name=width (comma-separated)
    #[arg(long = "inputs")]
    pub inputs: Option<String>,
    /// Output ports: name=width (comma-separated)
    #[arg(long = "outputs")]
    pub outputs: Option<String>,
}

/// mwaiver — Waiver management.
#[derive(clap::Args, Clone)]
pub struct MwaiverArgs {
    /// Subcommand: add, list, check, export, import
    #[command(subcommand)]
    pub cmd: MwaiverCmd,
}

/// mwaiver subcommand.
#[derive(Subcommand, Clone)]
pub enum MwaiverCmd {
    /// Tambah waiver baru
    Add {
        /// Rule name (e.g. W1001)
        #[arg(short = 'r', long = "rule")]
        rule: String,
        /// File pattern (optional)
        #[arg(short = 'f', long = "file")]
        file_pattern: Option<String>,
        /// Alasan waiver
        #[arg(short = 'R', long = "reason")]
        reason: String,
        /// Owner
        #[arg(short = 'o', long = "owner")]
        owner: String,
    },
    /// List semua waivers
    List {
        /// Filter by rule
        #[arg(short = 'r', long = "rule")]
        rule: Option<String>,
        /// Output JSON
        #[arg(long)]
        json: bool,
    },
    /// Check apakah violation di-waive
    Check {
        /// Rule name
        #[arg(short = 'r', long = "rule")]
        rule: String,
        /// File path (optional)
        #[arg(short = 'f', long = "file")]
        file: Option<String>,
    },
    /// Export waivers ke JSON
    Export {
        /// Output file
        #[arg(short = 'o', long = "output")]
        output: String,
    },
    /// Import waivers dari JSON
    Import {
        /// Input file
        #[arg(short = 'i', long = "input")]
        input: String,
    },
}

/// mvault — RTL Secure Vault.
#[derive(clap::Args, Clone)]
pub struct MvaultArgs {
    /// Subcommand: register, lock, unlock, verify, list, summary
    #[command(subcommand)]
    pub cmd: MvaultCmd,
}

/// mvault subcommand.
#[derive(Subcommand, Clone)]
pub enum MvaultCmd {
    /// Register file ke vault
    Register {
        /// File path
        #[arg(required = true)]
        file: String,
        /// User name
        #[arg(short = 'u', long = "user")]
        user: String,
    },
    /// Lock file untuk exclusive access
    Lock {
        /// File path
        #[arg(required = true)]
        file: String,
        /// User name
        #[arg(short = 'u', long = "user")]
        user: String,
    },
    /// Unlock file
    Unlock {
        /// File path
        #[arg(required = true)]
        file: String,
        /// User name
        #[arg(short = 'u', long = "user")]
        user: String,
    },
    /// Verify file integrity
    Verify {
        /// File path
        #[arg(required = true)]
        file: String,
    },
    /// List semua files di vault
    List,
    /// Tampilkan ringkasan vault
    Summary,
}

/// mipxact — IP-XACT packaging.
#[derive(clap::Args, Clone)]
pub struct MipxactArgs {
    /// Subcommand: generate, summary
    #[command(subcommand)]
    pub cmd: MipxactCmd,
}

/// mipxact subcommand.
#[derive(Subcommand, Clone)]
pub enum MipxactCmd {
    /// Generate IP-XACT XML dari module
    Generate {
        /// Module name
        #[arg(short = 'm', long = "module")]
        module: String,
        /// Vendor name
        #[arg(long = "vendor", default_value = "mivon")]
        vendor: String,
        /// Library name
        #[arg(long = "library", default_value = "rtl")]
        library: String,
        /// Version
        #[arg(long = "version", default_value = "1.0")]
        version: String,
        /// Output file (default: stdout)
        #[arg(short = 'o', long = "output")]
        output: Option<String>,
        /// Input ports: name=width (comma-separated)
        #[arg(long = "inputs")]
        inputs: Option<String>,
        /// Output ports: name=width (comma-separated)
        #[arg(long = "outputs")]
        outputs: Option<String>,
    },
    /// Tampilkan ringkasan IP-XACT
    Summary,
}

/// mdesign-repo — Design repository.
#[derive(clap::Args, Clone)]
pub struct MdesignRepoArgs {
    /// Subcommand: init, commit, log, tag, diff, summary
    #[command(subcommand)]
    pub cmd: MdesignRepoCmd,
}

/// mdesign-repo subcommand.
#[derive(Subcommand, Clone)]
pub enum MdesignRepoCmd {
    /// Initialize repository
    Init {
        /// Repository root directory
        #[arg(default_value = ".")]
        root: String,
    },
    /// Create commit
    Commit {
        /// Author name
        #[arg(short = 'a', long = "author")]
        author: String,
        /// Commit message
        #[arg(short = 'm', long = "message")]
        message: String,
        /// Files to include (space-separated)
        #[arg(required = true)]
        files: Vec<String>,
    },
    /// Show commit history
    Log {
        /// Max entries
        #[arg(short = 'n', long, default_value = "10")]
        max: usize,
    },
    /// Create tag
    Tag {
        /// Tag name
        #[arg(required = true)]
        name: String,
        /// Commit hash
        #[arg(required = true)]
        commit: String,
        /// Description
        #[arg(short = 'd', long = "description")]
        description: String,
    },
    /// Diff between two commits
    Diff {
        /// Commit A hash
        #[arg(required = true)]
        a: String,
        /// Commit B hash
        #[arg(required = true)]
        b: String,
    },
    /// Tampilkan ringkasan repository
    Summary,
}

/// mproject — Multi-project workspace management.
#[derive(clap::Args, Clone)]
pub struct MprojectArgs {
    /// Subcommand: init, add, remove, list, analyze, summary
    #[command(subcommand)]
    pub cmd: MprojectCmd,
}

/// mproject subcommand.
#[derive(Subcommand, Clone)]
pub enum MprojectCmd {
    /// Initialize workspace
    Init {
        /// Workspace root directory
        #[arg(default_value = ".")]
        root: String,
    },
    /// Add project ke workspace
    Add {
        /// Project name
        #[arg(short = 'n', long = "name")]
        name: String,
        /// Project path
        #[arg(short = 'p', long = "path")]
        path: String,
        /// Top module
        #[arg(short = 't', long = "top")]
        top: Option<String>,
        /// Dependencies (comma-separated)
        #[arg(short = 'd', long = "depends")]
        depends: Option<String>,
    },
    /// Remove project dari workspace
    Remove {
        /// Project name
        #[arg(required = true)]
        name: String,
    },
    /// List projects di workspace
    List,
    /// Analyze workspace dependencies
    Analyze,
    /// Tampilkan ringkasan workspace
    Summary,
}

/// msdc — SDC timing constraints parser.
#[derive(clap::Args, Clone)]
pub struct MsdcArgs {
    /// SDC file path
    #[arg(required = true)]
    pub file: String,
    /// Output JSON
    #[arg(long)]
    pub json: bool,
    /// Tampilkan clocks saja
    #[arg(long = "clocks-only")]
    pub clocks_only: bool,
}

/// mequiv — Equivalence checking.
#[derive(clap::Args, Clone)]
pub struct MequivCheckArgs {
    /// Golden values file (JSON)
    #[arg(short = 'g', long = "golden")]
    pub golden: String,
    /// Implementation values file (JSON)
    #[arg(short = 'i', long = "impl")]
    pub impl_file: String,
    /// Signal mapping file (JSON)
    #[arg(short = 'm', long = "mapping")]
    pub mapping: Option<String>,
    /// Method: miter (default), bit-blasting
    #[arg(long, default_value = "miter")]
    pub method: String,
}

/// mregression — Regression test management.
#[derive(clap::Args, Clone)]
pub struct MregressionArgs {
    /// Subcommand: record, summary, flaky, trend
    #[command(subcommand)]
    pub cmd: MregressionCmd,
}

/// mregression subcommand.
#[derive(Subcommand, Clone)]
pub enum MregressionCmd {
    /// Record new regression run
    Record {
        /// Results file (JSON)
        #[arg(short = 'i', long = "input")]
        input: String,
        /// Branch name
        #[arg(short = 'b', long = "branch", default_value = "main")]
        branch: String,
        /// Commit hash
        #[arg(short = 'c', long = "commit")]
        commit: String,
    },
    /// Tampilkan ringkasan regression
    Summary,
    /// Tampilkan flaky tests
    Flaky,
    /// Tampilkan trend regression
    Trend,
}

/// meco — ECO management.
#[derive(clap::Args, Clone)]
pub struct MecoArgs {
    /// Subcommand: create, list, transition, comment, summary
    #[command(subcommand)]
    pub cmd: MecoCmd,
}

/// meco subcommand.
#[derive(Subcommand, Clone)]
pub enum MecoCmd {
    /// Create ECO baru
    Create {
        /// Title
        #[arg(short = 'T', long = "title")]
        title: String,
        /// Description
        #[arg(short = 'd', long = "description")]
        description: String,
        /// Severity: critical, major, minor, cosmetic
        #[arg(short = 's', long = "severity")]
        severity: String,
        /// Author
        #[arg(short = 'a', long = "author")]
        author: String,
    },
    /// List ECOs
    List {
        /// Filter by status
        #[arg(short = 's', long = "status")]
        status: Option<String>,
        /// Filter by severity
        #[arg(short = 'S', long = "severity")]
        severity: Option<String>,
    },
    /// Transition ECO status
    Transition {
        /// ECO ID
        #[arg(required = true)]
        id: String,
        /// New status
        #[arg(required = true)]
        new_status: String,
    },
    /// Add comment ke ECO
    Comment {
        /// ECO ID
        #[arg(required = true)]
        id: String,
        /// Author
        #[arg(short = 'a', long = "author")]
        author: String,
        /// Comment text
        #[arg(short = 'c', long = "comment")]
        text: String,
    },
    /// Tampilkan ringkasan ECOs
    Summary,
}

/// mcov-closure — Coverage closure analytics.
#[derive(clap::Args, Clone)]
pub struct McovClosureArgs {
    /// Subcommand: analyze, critical, uncovered
    #[command(subcommand)]
    pub cmd: McovClosureCmd,
}

/// mcov-closure subcommand.
#[derive(Subcommand, Clone)]
pub enum McovClosureCmd {
    /// Analyze coverage closure
    Analyze {
        /// Coverage data file (JSON)
        #[arg(short = 'i', long = "input")]
        input: String,
    },
    /// Tampilkan critical tests
    Critical {
        /// Coverage data file (JSON)
        #[arg(short = 'i', long = "input")]
        input: String,
    },
    /// Tampilkan uncovered points
    Uncovered {
        /// Coverage data file (JSON)
        #[arg(short = 'i', long = "input")]
        input: String,
    },
}
