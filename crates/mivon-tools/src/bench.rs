//! `mbench` — Benchmark Tool.
//!
//! Ukur: compile speed, memori (peak RSS), CPU time, cache hit, parser
//! throughput — lintas beberapa run.

use std::time::Instant;

use crate::{collect_targets, human_bytes, kv, make_session_config_with_mv, section};
use mivon_compiler::frontend::CompileSession;
use mivon_core::error::SimError;

/// Opsi mbench.
pub struct BenchArgs<'a> {
    pub targets: &'a [String],
    pub incdirs: &'a [String],
    pub defines: &'a [String],
    pub runs: usize,
}

/// Hasil satu run.
struct RunResult {
    total_us: u64,
    parse_us: u64,
    #[allow(dead_code)]
    elab_us: u64,
    processed: usize,
    cached: usize,
}

/// Jalankan mbench.
pub fn run(args: &BenchArgs) -> Result<(), SimError> {
    let files = collect_targets(args.targets)?;
    let runs = args.runs.max(1);

    // F10: `.mv` di-transpile ke buffer inline (svh+sv) agar mbench bisa
    // mengukur performa pipeline Mivon HDL tanpa menulis file ke disk.
    let cfg_template =
        make_session_config_with_mv(files.clone(), args.incdirs, args.defines, None)?;

    // Hitung baris dari buffer inline bila tersedia (file `.mv`) — metrik
    // baris tetap bermakna untuk Mivon HDL; fallback baca disk.
    let total_lines: usize = files
        .iter()
        .map(|f| {
            cfg_template
                .inline_sources
                .get(f)
                .map(|b| String::from_utf8_lossy(b).lines().count())
                .or_else(|| std::fs::read_to_string(f).ok().map(|s| s.lines().count()))
                .unwrap_or(0)
        })
        .sum();

    section("Benchmark");
    kv("files", files.len());
    kv("lines", total_lines);
    kv("runs", runs);

    let mut results: Vec<RunResult> = Vec::new();
    for i in 0..runs {
        let cfg = cfg_template.clone();
        let mut session = CompileSession::new(cfg);

        let start = Instant::now();
        let (_design, _ir, _len) = session.compile_and_elaborate(None)?;
        let total_us = start.elapsed().as_micros() as u64;

        let t = &session.timing;
        results.push(RunResult {
            total_us,
            parse_us: t.parse_us,
            elab_us: t.elab_us,
            processed: t.processed_files,
            cached: t.cached_files,
        });
        println!(
            "  run {}/{}: compile {} µs (parse {} µs, elab {} µs, {} file)",
            i + 1,
            runs,
            total_us,
            t.parse_us,
            t.elab_us,
            t.processed_files
        );
    }

    // ── Agregasi ──
    let min = results.iter().map(|r| r.total_us).min().unwrap_or(0);
    let max = results.iter().map(|r| r.total_us).max().unwrap_or(0);
    let avg = results.iter().map(|r| r.total_us).sum::<u64>() / runs as u64;
    let avg_parse = results.iter().map(|r| r.parse_us).sum::<u64>() / runs as u64;

    section("Result");
    kv("compile min", format!("{} µs", min));
    kv("compile max", format!("{} µs", max));
    kv("compile avg", format!("{} µs", avg));
    kv("parse avg", format!("{} µs", avg_parse));

    // Throughput
    let files_per_sec = if avg > 0 {
        (files.len() as f64 / (avg as f64 / 1000.0)) as u64
    } else {
        0
    };
    let lines_per_sec = if avg > 0 {
        (total_lines as f64 / (avg as f64 / 1000.0)) as u64
    } else {
        0
    };
    kv(
        "throughput",
        format!("{} file/s, {} lines/s", files_per_sec, lines_per_sec),
    );

    // Cache hit
    let processed_total: usize = results.iter().map(|r| r.processed).sum();
    let cached_total: usize = results.iter().map(|r| r.cached).sum();
    kv("files processed", processed_total);
    kv("files cached", cached_total);

    // Memori: peak RSS (VmHWM dari /proc/self/status)
    section("Memory");
    let rss = peak_rss_kb();
    match rss {
        Some(kb) => {
            kv("peak RSS", human_bytes(kb * 1024));
            let mem_per_mb = if avg > 0 {
                (kb * 1024) as f64 / (avg as f64 / 1000.0) / 1_048_576.0
            } else {
                0.0
            };
            kv("rate", format!("{:.2} MB/s", mem_per_mb));
        }
        None => kv("peak RSS", "tidak tersedia"),
    }

    Ok(())
}

/// Peak RSS dari `/proc/self/status` (VmHWM). Linux-only.
fn peak_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            return rest
                .trim()
                .trim_end_matches("kB")
                .trim()
                .parse::<u64>()
                .ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_peak_rss_kb_returns_some_on_linux() {
        let rss = peak_rss_kb();
        assert!(rss.is_some(), "peak RSS should be readable on Linux");
        assert!(rss.unwrap() > 0, "RSS should be > 0 KB");
    }

    #[test]
    fn test_peak_rss_kb_is_reasonable() {
        let rss = peak_rss_kb().unwrap_or(0);
        assert!(rss > 1024, "RSS should exceed 1 MB");
        assert!(rss < 1_048_576, "RSS should be under 1 TB");
    }
}
