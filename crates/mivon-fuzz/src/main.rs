//! mivon-fuzz CLI (dev-only, butuh --features dev).
//!
//! Subcommand: run, replay, triage, minimize, report, help.

use mivon_fuzz::{
    bugs_dir, default_corpus_dir, minimize, run, triage, CaseResult, Category, FuzzConfig, Target,
};

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("help");

    let code = match cmd {
        "run" => cmd_run(&args[2..]),
        "replay" => cmd_replay(&args[2..]),
        "judge" => cmd_judge(&args[2..]),
        "triage" => cmd_triage(&args[2..]),
        "minimize" => cmd_minimize(&args[2..]),
        "report" => cmd_report(&args[2..]),
        "verify" => cmd_verify(&args[2..]),
        "regress" => cmd_regress(&args[2..]),
        "help" | "-h" | "--help" => {
            print_help();
            0
        }
        other => {
            eprintln!("subcommand tidak dikenal: {other}\n");
            print_help();
            1
        }
    };
    std::process::exit(code);
}

fn print_help() {
    println!(
        r#"mivon-fuzz {version} — fuzzer berbasis corpus project nyata (cva6/openc910/opentitan)

USAGE:
  mivon-fuzz run     [--target <t>] [-c <n>] [--seed <n>] [--timeout <ms>] [--corpus <dir>] [--no-save] [--no-verify] [--verify-cases <n>] [--verify-seed <n>] [--jobs <n>]
                     # --jobs: 0/absen = auto (cores/2 clamp 1-8), 1 = serial, N = worker pool
  mivon-fuzz replay  <file.sv> [--target <t>] [--timeout <ms>]
  mivon-fuzz judge   <file.sv> [--timeout <ms>]   # nilai LRM IEEE 1800 (lrm_judge)
  mivon-fuzz triage  <dir>
  mivon-fuzz minimize <file.sv> [--target <t>] [--timeout <ms>]
  mivon-fuzz verify  [<corpus-dir>] [--timeout <ms>]      # differential vs iverilog atas seed corpus
  mivon-fuzz verify  --cases <n> [--seed <n>] [--timeout]  # differential vs iverilog atas hasil MUTASI
  mivon-fuzz regress [--dir <bugs>] [--out <file.rs>]      # generate auto-regression tests (1 bug = 1 test)
  mivon-fuzz report  [<dir>]
  mivon-fuzz help

TARGETS: all | lexer | parser | elab | sim | fmt | cli | preproc | mv | vcd | sdf | micd | synth | astdiff | judge
Default: all (2000 cases/target). Corpus default: {corpus}
Bug output: {bugs}
"#,
        version = mivon_fuzz::VERSION,
        corpus = default_corpus_dir().display(),
        bugs = bugs_dir().display(),
    );
}

fn parse_target(s: &str) -> Option<Target> {
    Target::from_token(s)
}

/// Cari nilai flag di args (mis. "--seed" → value), hapus dari slice.
fn take_flag(args: &[String], flag: &str) -> (Option<String>, Vec<String>) {
    let mut val = None;
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == flag && i + 1 < args.len() {
            val = Some(args[i + 1].clone());
            i += 2;
        } else {
            rest.push(args[i].clone());
            i += 1;
        }
    }
    (val, rest)
}

/// Parse u64 — dukung hex `0x`/`0X` prefix (default fuzz seed hex dipakai
/// kampanye). Falls back ke decimal biasa.
fn parse_u64(s: &str) -> Option<u64> {
    s.strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .and_then(|h| u64::from_str_radix(h, 16).ok())
        .or_else(|| s.parse().ok())
}

fn cmd_run(args: &[String]) -> i32 {
    let (target_s, rest) = take_flag(args, "--target");
    let (cases_s, rest) = take_flag(&rest, "-c");
    let (seed_s, rest) = take_flag(&rest, "--seed");
    let (timeout_s, rest) = take_flag(&rest, "--timeout");
    let (corpus_s, rest) = take_flag(&rest, "--corpus");
    let (verify_cases_s, rest) = take_flag(&rest, "--verify-cases");
    let (verify_seed_s, rest) = take_flag(&rest, "--verify-seed");
    let (jobs_s, rest) = take_flag(&rest, "--jobs");
    let no_save = rest.iter().any(|a| a == "--no-save");
    // Auto-verify vs iverilog (differential eksternal) bisa dimatikan:
    // `--no-verify`.
    let no_verify = rest.iter().any(|a| a == "--no-verify");

    let target = target_s
        .as_deref()
        .and_then(parse_target)
        .unwrap_or(Target::All);
    let cases = cases_s
        .and_then(|s| s.parse().ok())
        .unwrap_or(FuzzConfig::default().cases);
    let seed = seed_s
        .and_then(|s| parse_u64(&s))
        .unwrap_or(FuzzConfig::default().seed);
    let timeout = timeout_s
        .and_then(|s| s.parse().ok())
        .unwrap_or(FuzzConfig::default().timeout_ms);
    let corpus_dir = corpus_s.map(PathBuf::from);
    // `--jobs N`: 0/absen = auto (cores/2 clamp 1-8), 1 = serial jalur lama.
    let jobs: usize = jobs_s
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let cfg = FuzzConfig {
        target,
        cases,
        seed,
        timeout_ms: timeout,
        corpus_dir,
        save_bugs: !no_save,
        jobs,
    };

    eprintln!(
        "KAMPANYE: target={} cases={} seed={:#x} timeout={}ms jobs={}",
        target.as_str(),
        cases,
        seed,
        timeout,
        if jobs == 0 { "auto".to_string() } else { jobs.to_string() }
    );
    let report = run(cfg);
    let text = triage::render_report(&report.bugs);
    println!("{text}");

    // Ringkasan per target
    for s in &report.summaries {
        println!("{}", mivon_fuzz::summary_line(s));
    }
    if !report.bugs.is_empty() {
        let p = triage::save_report(&report.bugs);
        if let Ok(p) = p {
            eprintln!("laporan: {}", p.display());
        }
    }

    // ── AUTO-VERIFY vs iverilog (differential EKSTERNAL) ──
    // Bagian DEFAULT tiap kampanye (dulu subcommand terpisah yang jarang
    // dijalankan → bug semantik "salah tapi konsisten antar jalur" tak
    // ketahuan — kasus prim_max_tree harus golden manual). 50 kasus mutasi
    // murah; dilewati bila iverilog tak terpasang atau `--no-verify`.
    //
    // Seed verify ROTASI tiap kampanye (time-based) — sebelumnya seed tetap
    // `0x1CA_2026` membuat 50 kasus yang PERSIS SAMA diulang setiap
    // kampanye, nol eksplorasi baru. Override via `--verify-seed` /
    // `--verify-cases` (reproducible bila diinginkan).
    let verify_cases: usize = verify_cases_s
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);
    let verify_seed: u64 = verify_seed_s
        .as_deref()
        .and_then(parse_u64)
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0x1CA_2026)
        });
    let mut verify_failed = false;
    if no_verify {
        eprintln!("verify: dilewati (--no-verify)");
    } else if !mivon_fuzz::oracle_icarus::iverilog_available() {
        eprintln!("verify: iverilog tak terpasang — differential eksternal dilewati");
    } else {
        eprintln!(
            "verify otomatis vs iverilog ({} kasus mutasi, seed {:#x})...",
            verify_cases, verify_seed
        );
        // Budget 20s: compile kasus mutasi besar terukur 4-5s (+sim+icarus)
        // → 8s menghasilkan MIVON-BUG hang PALSU (3-4 per kampanye).
        verify_failed = verify_mutated(verify_cases, verify_seed, 20000) != 0;
    }

    if report.bugs.is_empty() && !verify_failed {
        0
    } else {
        1
    }
}

fn cmd_replay(args: &[String]) -> i32 {
    let (target_s, rest) = take_flag(args, "--target");
    let (timeout_s, rest) = take_flag(&rest, "--timeout");
    let detail = rest.iter().any(|a| a == "--detail");
    let rest: Vec<String> = rest.into_iter().filter(|a| a != "--detail").collect();
    let file = rest.first().cloned().unwrap_or_default();
    if file.is_empty() {
        eprintln!("usage: mivon-fuzz replay <file.sv> [--target <t>] [--timeout <ms>] [--detail]");
        return 1;
    }
    let source = match std::fs::read_to_string(&file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("gagal baca {file}: {e}");
            return 1;
        }
    };
    let target = target_s
        .as_deref()
        .and_then(parse_target)
        .unwrap_or(Target::Simulator);
    let timeout = timeout_s
        .and_then(|s| s.parse().ok())
        .unwrap_or(FuzzConfig::default().timeout_ms);

    // --detail: dumpt bukti per-sinyal (validate::collect) — isolasi akar
    // differential/x-stuck tanpa menebak.
    if detail {
        let ev = mivon_fuzz::validate::collect(&source, 1000);
        eprintln!("EVIDENCE: {}", ev.summary());
        for d in &ev.diff_details {
            println!("DIFF: {d}");
        }
    }

    eprintln!("REPLAY: {file} -> target={}", target.as_str());
    let result = mivon_fuzz::oracle::evaluate(target, &source, timeout);
    println!("category: {}", result.category.label());
    println!("oracle:   {}", result.oracle);
    println!("detail:   {}", result.detail.lines().next().unwrap_or(""));
    if result.category.is_bug() {
        1
    } else {
        0
    }
}

/// JUDGE: nilai satu file terhadap hakim LRM IEEE 1800 (lrm_judge +
/// registry penuh). Exit 1 bila verdict layak-simpan (violation/internal
/// failure), 0 bila tidak.
fn cmd_judge(args: &[String]) -> i32 {
    let (timeout_s, rest) = take_flag(args, "--timeout");
    let file = rest.first().cloned().unwrap_or_default();
    if file.is_empty() {
        eprintln!("usage: mivon-fuzz judge <file.sv> [--timeout <ms>]");
        return 1;
    }
    let source = match std::fs::read_to_string(&file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("gagal baca {file}: {e}");
            return 1;
        }
    };
    let timeout = timeout_s
        .and_then(|s| s.parse().ok())
        .unwrap_or(FuzzConfig::default().timeout_ms);

    eprintln!("JUDGE: {file} (aturan LRM penuh)");
    let report = mivon_fuzz::judge::judge_single(&source, timeout);
    print!("{}", mivon_fuzz::judge::render_report(&report));
    if report.should_save() {
        1
    } else {
        0
    }
}

fn cmd_triage(args: &[String]) -> i32 {
    let dir = args
        .first()
        .cloned()
        .unwrap_or_else(|| bugs_dir().to_string_lossy().to_string());

    let mut results = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("sv") {
                continue;
            }
            // butuh pasangan .txt metadata
            let txt = p.with_extension("txt");
            let meta = std::fs::read_to_string(&txt).unwrap_or_default();
            let category = meta
                .lines()
                .find(|l| l.starts_with("category: "))
                .and_then(|l| l.split(": ").nth(1))
                .and_then(Category::from_label)
                .unwrap_or(Category::Panic);

            let source = std::fs::read_to_string(&p).unwrap_or_default();
            results.push(CaseResult {
                target: Target::Parser,
                category,
                oracle: "manual",
                detail: meta
                    .lines()
                    .find(|l| l.starts_with("detail: "))
                    .unwrap_or("")
                    .to_string(),
                source,
                cov_keys: Vec::new(),
            });
        }
    } else {
        eprintln!("dir tidak ada: {dir}");
        return 1;
    }

    println!("{}", triage::render_report(&results));
    0
}

fn cmd_minimize(args: &[String]) -> i32 {
    let (target_s, rest) = take_flag(args, "--target");
    let (timeout_s, rest) = take_flag(&rest, "--timeout");
    let file = rest.first().cloned().unwrap_or_default();
    if file.is_empty() {
        eprintln!("usage: mivon-fuzz minimize <file.sv> [--target <t>] [--timeout <ms>]");
        return 1;
    }
    let source = match std::fs::read_to_string(&file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("gagal baca {file}: {e}");
            return 1;
        }
    };
    let target = target_s
        .as_deref()
        .and_then(parse_target)
        .unwrap_or(Target::Simulator);
    let timeout = timeout_s
        .and_then(|s| s.parse().ok())
        .unwrap_or(FuzzConfig::default().timeout_ms);

    // Predikat: bug ter-reproduksi (bukan Ok/CleanError)
    let predicate = |s: &str| {
        let r = mivon_fuzz::oracle::evaluate(target, s, timeout);
        r.category.is_bug()
    };

    eprintln!(
        "MINIMIZE: {file} ({} bytes) target={}",
        source.len(),
        target.as_str()
    );
    if !predicate(&source) {
        eprintln!("source asli TIDAK reproducible — tidak bisa minimize");
        return 1;
    }

    let min = minimize::ddmin(&source, &predicate);
    eprintln!("minimized: {} -> {} bytes", source.len(), min.len());
    println!("{min}");

    let out = file.replace(".sv", "_min.sv");
    let _ = std::fs::write(&out, &min);
    eprintln!("tersimpan: {out}");
    0
}

/// VERIFY: differential reference vs iverilog atas seed corpus ATAU hasil
/// mutasi (mode `--cases N`).
///
/// Mode seed (default): untuk setiap seed module `NN_name.sv` yang punya
/// pasangan `tb_NN_name.sv`, concat module+tb → run `oracle_icarus::evaluate`
/// → klasifikasi: MATCH (hasil mivon == iverilog), MISMATCH (semantic
/// divergence), REF-N/A (iverilog cannot compile), MIVON-BUG.
///
/// Mode mutasi (`--cases N`): ambil N seed nyata dari corpus (termasuk
/// project real), mutasi 0-2x, icarus-compare — differential EKSTERNAL atas
/// kasus fuzz, bukan hanya seed bersih. Ini area yang belum tersentuh: sim
/// oracle hanya membuktikan konsistensi INTERNAL (O4/O5), bukan kebenaran
/// vs reference independen.
fn cmd_verify(args: &[String]) -> i32 {
    let (timeout_s, rest) = take_flag(args, "--timeout");
    let (cases_s, rest) = take_flag(&rest, "--cases");
    let (seed_s, rest) = take_flag(&rest, "--seed");
    let timeout = timeout_s
        .and_then(|s| s.parse().ok())
        .unwrap_or(FuzzConfig::default().timeout_ms);

    if let Some(cases_s) = cases_s {
        let cases: usize = cases_s.parse().unwrap_or(200);
        let seed: u64 = seed_s.and_then(|s| parse_u64(&s)).unwrap_or(0x1CA_2026);
        return verify_mutated(cases, seed, timeout);
    }

    let dir = rest
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(default_corpus_dir);

    eprintln!(
        "VERIFY vs iverilog: corpus={} timeout={}ms",
        dir.display(),
        timeout
    );

    let mut seed_sources: Vec<(String, String)> = Vec::new(); // (name, combined)
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let mut names: Vec<String> = entries
            .flatten()
            .map(|e| e.path().to_string_lossy().to_string())
            .collect();
        names.sort();
        let mut tbs: Vec<String> = names
            .iter()
            .filter(|n| n.contains("tb_") && n.ends_with(".sv"))
            .cloned()
            .collect();
        // Harus sort deterministik
        tbs.sort();
        for tb in &tbs {
            // tb_<N>_<name>.sv → module <N>_<name>.sv (ding kursa tb_ prefix)
            let fname = tb.split("/").last().unwrap_or(tb).to_string();
            let module_name = fname.strip_prefix("tb_").unwrap_or_default().to_string();
            let module_path = dir.join(&module_name);
            if !module_path.exists() {
                eprintln!("  ~ skip {fname}: module pasangan {module_name} tidak ada");
                continue;
            }
            let module_src = std::fs::read_to_string(&module_path).unwrap_or_default();
            let tb_src = std::fs::read_to_string(tb).unwrap_or_default();
            if module_src.is_empty() || tb_src.is_empty() {
                continue;
            }
            seed_sources.push((fname.clone(), format!("{}\n{}", module_src, tb_src)));
        }
    } else {
        eprintln!("corpus dir tidak ada: {}", dir.display());
        return 1;
    }

    eprintln!("{} seed pair (module+tb) ditemui\n", seed_sources.len());

    let mut n_match = 0usize;
    let mut n_mismatch = 0usize;
    let mut n_n_a = 0usize;
    for (name, combined) in &seed_sources {
        let r = mivon_fuzz::oracle_icarus::evaluate_icarus(combined, timeout);
        match r.verdict {
            mivon_fuzz::oracle_icarus::Verdict::Match => {
                eprintln!("  [MATCH  ] {name}: {}", r.detail);
                n_match += 1;
            }
            mivon_fuzz::oracle_icarus::Verdict::Mismatch => {
                eprintln!(
                    "  [MISMATCH] {name}:\n    {}",
                    r.detail.lines().collect::<Vec<_>>().join("\n    ")
                );
                n_mismatch += 1;
            }
            mivon_fuzz::oracle_icarus::Verdict::RefUnavailable => {
                eprintln!("  [REF-N/A] {name}: {}", r.detail);
                n_n_a += 1;
            }
            mivon_fuzz::oracle_icarus::Verdict::MivonBug => {
                eprintln!("  [MIVON-BUG] {name}: {}", r.detail);
                n_mismatch += 1;
            }
        }
    }

    println!(
        "\nVERIFY SUMARRY: match={} mismatch={} ref-n/a={} total={}",
        n_match,
        n_mismatch,
        n_n_a,
        seed_sources.len()
    );
    if n_mismatch == 0 {
        0
    } else {
        1
    }
}

/// VERIFY mode mutasi: N kasus fuzz (corpus penuh + mutasi 0-2x) di-compare
/// vs iverilog. MIVON-BUG/MISMATCH = bug nyata (hasil sim beda dari reference
/// eksternal, atau mivon crash).
fn verify_mutated(cases: usize, seed: u64, timeout_ms: u64) -> i32 {
    use mivon_fuzz::corpus;
    // Fokus ke corpus SELF-CONTAINED (seeds + mv — dirancang utk sim mandiri
    // + punya marker `ASRT_`/`$display`). Project real (cva6/openc910/
    // opentitan) kebanyakan RTL tanpa tb/marker + interdependen → compare
    // iverilog vakum/noise.
    // Path absolut via workspace_root — dulu relatif cwd (verify dari luar
    // workspace → corpus penuh 4698 tanpa marker → 45/50 skip vakum).
    let base_dir = corpus::workspace_root().join("crates/mivon-fuzz/fuzz/corpus/seeds");
    let corpus = if base_dir.exists() {
        corpus::Corpus::load(Some(&base_dir))
    } else {
        corpus::Corpus::load(None)
    };
    if corpus.is_empty() {
        eprintln!("WARNING: corpus kosong");
        return 1;
    }
    let mut rng = mivon_fuzz::Rng::new(seed);
    let mut n_match = 0usize;
    let mut n_mismatch = 0usize;
    let mut n_ref_na = 0usize;
    let mut n_mivonbug = 0usize;
    let mut n_clean = 0usize;

    eprintln!(
        "VERIFY-MUTATED vs iverilog: cases={} seed={:#x} timeout={}ms — corpus {} seeds",
        cases,
        seed,
        timeout_ms,
        corpus.len()
    );

    for i in 0..cases {
        let Some(base_seed) = corpus.random_seed(&mut rng) else {
            break;
        };
        let mut base_source = base_seed.text;
        let is_mv = base_seed.is_mv;
        // Pasangkan tb (`tb_NN_name.sv`) — design tanpa stimulus → marker
        // kosong → compare iverilog vakum (49/50 skip sebelum fix ini).
        if !is_mv {
            base_source = corpus.pair_with_tb(&base_seed.path, &base_source);
        }

        // Mutasi 0-4x (sama dgn kampanye utama) — dulu 0-2x, sinyal lebih
        // dangkal daripada kampanye; differential eksternal justru oracle
        // terkuat, jadi eksplorasi mutasi disamakan.
        let n_mut = rng.below(5);
        if n_mut > 0 {
            let mut mutator = mivon_fuzz::mutator::Mutator::new(&mut rng);
            for _ in 0..n_mut {
                base_source = mutator.mutate(&base_source, &corpus);
            }
        }

        // MV → transpile dulu (transpiled SV lah yang dibandingkan).
        let mut source = if is_mv {
            let base_name = base_seed
                .path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("design")
                .to_string();
            corpus::transpile_mv_string(&base_source, &base_name).unwrap_or_else(|_| {
                "// mv transpile failed\nmodule fz_mv_transpile_err; endmodule".to_string()
            })
        } else {
            base_source
        };

        // Tanpa MARKER ASRT_ → coba injeksi otomatis (differential atas
        // signal top di #1) sebelum menyerah vakum. Gagal injeksi + tanpa
        // $display/$finish → skip murah (tanpa bakar 2× tool run).
        if !source.contains("ASRT_") {
            if let Some(injected) = mivon_fuzz::oracle_icarus::inject_auto_markers(&source) {
                source = injected;
            } else if !source.contains("$display") && !source.contains("$finish") {
                n_clean += 1;
                continue;
            }
        }

        let r = mivon_fuzz::oracle_icarus::evaluate_icarus(&source, timeout_ms);
        match r.verdict {
            mivon_fuzz::oracle_icarus::Verdict::Match => {
                n_match += 1;
            }
            mivon_fuzz::oracle_icarus::Verdict::Mismatch => {
                n_mismatch += 1;
                // Tampilkan 5 baris pertama detail — baris pertama saja
                // ("SEMANTIC MISMATCH vs iverilog:") tak berisi signal apa
                // yang beda (info beda ada di baris berikut).
                let preview: Vec<&str> = r.detail.lines().take(5).collect();
                eprintln!("  [MISMATCH] #{}: {}", i, preview.join("\n    "));
                let _ = std::fs::create_dir_all(mivon_fuzz::bugs_dir());
                let sv_path = mivon_fuzz::bugs_dir().join(format!("verify_bad_{:04}.sv", i));
                let _ = std::fs::write(&sv_path, &source);
                // Metadata .txt wajib: tanpa ini `regress` men-skip artefak
                // ("no metadata") → temuan verify tak pernah jadi test.
                let detail = r.detail.lines().next().unwrap_or("");
                let _ = std::fs::write(
                    sv_path.with_extension("txt"),
                    format!(
                        "oracle: O5-icarus\ncategory: differential\ntarget: sim\nsignature: sim|O5-icarus|differential|{detail}|\ndetail: {detail}\niter: {i}\n",
                    ),
                );
            }
            mivon_fuzz::oracle_icarus::Verdict::RefUnavailable => {
                n_ref_na += 1;
            }
            mivon_fuzz::oracle_icarus::Verdict::MivonBug => {
                // MIVON-BUG asli = panic/abort/hang (mivon crash/corrupt).
                // Verdict::MivonBug dgn category CleanError = mivon menolak
                // source mutasi invalid secara benar → reference N/A dua sisi,
                // BUKAN bug mivon. Filter di sini (oracle_icarus mengklasifikasi
                // semua non-Ok sebagai MivonBug dgn category asli).
                match r.category {
                    mivon_fuzz::Category::Panic
                    | mivon_fuzz::Category::Abort
                    | mivon_fuzz::Category::Hang => {
                        n_mivonbug += 1;
                        eprintln!(
                            "  [MIVON-BUG] #{}: {}",
                            i,
                            r.detail.lines().next().unwrap_or("")
                        );
                        let _ = std::fs::create_dir_all(mivon_fuzz::bugs_dir());
                        let sv_path =
                            mivon_fuzz::bugs_dir().join(format!("verify_bug_{:04}.sv", i));
                        let _ = std::fs::write(&sv_path, &source);
                        // Metadata .txt (lihat catatan verify_bad) — kategori
                        // asli Panic/Abort/Hang tersimpan apa adanya.
                        let detail = r.detail.lines().next().unwrap_or("");
                        let _ = std::fs::write(
                            sv_path.with_extension("txt"),
                            format!(
                                "oracle: O1-no-crash\ncategory: {}\ntarget: sim\nsignature: sim|O1-no-crash|{}|{detail}|\ndetail: {detail}\niter: {i}\n",
                                r.category.label(),
                                r.category.label(),
                            ),
                        );
                    }
                    _ => {
                        n_clean += 1;
                    }
                }
            }
        }
        if (i + 1) % 100 == 0 {
            eprintln!(
                "  [{}/{}] match={} mismatch={} refNA={} mivonBug={}",
                i + 1,
                cases,
                n_match,
                n_mismatch,
                n_ref_na,
                n_mivonbug
            );
        }
    }

    println!(
        "\nVERIFY-MUTATED SUMARRY: match={} mismatch={} ref-n/a={} mivon-bug={} skipped_no_marker={} total={}",
        n_match,
        n_mismatch,
        n_ref_na,
        n_mivonbug,
        n_clean,
        cases
    );
    if n_mismatch + n_mivonbug == 0 {
        0
    } else {
        1
    }
}

/// REGRESS: ubah artefak `.mivon-fuzz-bugs/*.sv` (+ `.txt`) jadi modul
/// regression test (`#![cfg(feature="dev")]`). Tiap bug → 1 `#[test]`.
/// Default out = `crates/mivon-fuzz/tests/fuzz_regression.rs` (relatif cwd),
/// bisa di-override `--out <file>`. Artefak raksasa / tanpa metadata di-skip
/// (dicatat sbg komentar) — butuh triage manual.
fn cmd_regress(args: &[String]) -> i32 {
    let (dir_s, rest) = take_flag(args, "--dir");
    let (out_s, _rest) = take_flag(&rest, "--out");
    let dir = dir_s
        .map(std::path::PathBuf::from)
        .unwrap_or_else(mivon_fuzz::bugs_dir);
    let out = out_s.unwrap_or_else(|| {
        mivon_fuzz::corpus::workspace_root()
            .join("crates/mivon-fuzz/tests/fuzz_regression.rs")
            .to_string_lossy()
            .to_string()
    });

    if !dir.exists() {
        eprintln!("regress: direktori bug tidak ada: {}", dir.display());
        return 1;
    }

    let entries = mivon_fuzz::regression::collect_bugs(&dir);
    // Fixture untuk artefak > EMBED_CAP: salin ke tests/fixtures/ agar tetap
    // jadi test (`include_str!`) — bukan di-skip sebagai komentar.
    let fixtures = mivon_fuzz::corpus::workspace_root().join("crates/mivon-fuzz/tests/fixtures");
    let mut n_fixture = 0usize;
    // Comment kini penyimpanan tunggal mivon-fuzz::regression::FIXTURE_MAX_BYTES
    // (dulu salinan lokal = 2 mata sumber nilai, review T4 nit 4).
    let mut written: Vec<u64> = Vec::new();
    let fix_cap = mivon_fuzz::regression::FIXTURE_MAX_BYTES;
    for e in entries
        .iter()
        .filter(|e| e.skipped && e.skip_reason.starts_with("source "))
    {
        if e.source.len() > fix_cap {
            continue;
        }
        let fp = mivon_fuzz::regression::content_fingerprint(&e.source);
        if written.contains(&fp) {
            continue;
        }
        let _ = std::fs::create_dir_all(&fixtures);
        if std::fs::write(
            fixtures.join(mivon_fuzz::regression::fixture_name(&e.source)),
            &e.source,
        )
        .is_ok()
        {
            written.push(fp);
            n_fixture += 1;
        }
    }
    let active = entries.iter().filter(|e| !e.skipped).count();
    // Skip sejati = tanpa metadata; entri ukuran-besar tertangani fixture.
    let sized = entries
        .iter()
        .filter(|e| e.skipped && e.skip_reason.starts_with("source "))
        .count();
    let skipped = entries.len() - active - sized;
    // Deteksi LIVE (bug masi reproduksi) → canary assert_eq bukan assert_ne
    // (suite tetap hijau; kantong bug muncul merah saat perilaku berubah).
    eprintln!("regress: deteksi live ({active} entri, replay tiap bug ~s)");
    let live = mivon_fuzz::regression::compute_live(&entries);
    let code = mivon_fuzz::regression::emit_rust_tests(&entries, &fixtures, &live);

    match std::fs::write(&out, &code) {
        Ok(()) => {
            eprintln!(
                "regress: {} entri ({active} aktif, {n_fixture} fixture besar, {skipped} skip, {} live-canary) -> {}",
                entries.len(),
                live.len(),
                out
            );
            // Statistik machine-friendly.
            println!(
                "REGRESS: entries={} active={} fixture={} skipped={} live={}",
                entries.len(),
                active,
                n_fixture,
                skipped,
                live.len()
            );
            0
        }
        Err(e) => {
            eprintln!("regress: gagal tulis {}: {e}", out);
            1
        }
    }
}

fn cmd_report(args: &[String]) -> i32 {
    // Baca semua bug dari bugs dir (pakai metadata .txt), lalu render.
    let mut results: Vec<CaseResult> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(bugs_dir()) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("sv") {
                continue;
            }
            let txt = p.with_extension("txt");
            let meta = std::fs::read_to_string(&txt).unwrap_or_default();
            let category = meta
                .lines()
                .find(|l| l.starts_with("category: "))
                .and_then(|l| l.split(": ").nth(1))
                .and_then(Category::from_label)
                .unwrap_or(Category::Panic);
            let source = std::fs::read_to_string(&p).unwrap_or_default();
            results.push(CaseResult {
                target: Target::Parser,
                category,
                oracle: "manual",
                detail: String::new(),
                source,
                cov_keys: Vec::new(),
            });
        }
    }
    println!("{}", triage::render_report(&results));
    let _ = args;
    0
}
