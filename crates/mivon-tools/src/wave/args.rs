//! Enum [`WaveArgs`] (argumen CLI mwave) + dispatcher [`run`] ke
//! submodul subkomandonya.

use mivon_core::error::SimError;

use super::compare::compare;
use super::debug::debug;
use super::decode::decode;
use super::export::export;
use super::filter::filter;
use super::get::get;
use super::merge::merge;
use super::search::search;
use super::stats::stats;
use super::tree::tree;

/// Opsi mwave.
pub enum WaveArgs {
    Merge {
        inputs: Vec<String>,
        output: Option<String>,
    },
    Export {
        input: String,
        format: String,
        output: Option<String>,
    },
    Filter {
        input: String,
        signals: Vec<String>,
        output: Option<String>,
    },
    Compare {
        a: String,
        b: String,
    },
    Search {
        input: String,
        patterns: Vec<String>,
    },
    Tree {
        input: String,
    },
    Stats {
        input: String,
    },
    Get {
        /// VCD input
        input: String,
        /// Sinyal yang di-query (koma/space terpisah, dukung * dan ?)
        signals: Vec<String>,
        /// Nilai pada waktu T (random access — perubahan terakhir ≤ T)
        at: Option<u64>,
        /// Rentang waktu t1:t2 (semua perubahan dalam [t1, t2])
        range: Option<(u64, u64)>,
    },
    Decode {
        /// VCD input
        input: String,
        /// Protokol bus: apb (default) | axi4lite | ahb
        proto: String,
    },
    /// Debug manifest (FEAT-15): VCD -> JSON probe-point map.
    Debug {
        /// VCD input
        input: String,
        /// Output JSON (default: <input>.debug.json)
        output: Option<String>,
    },
}

/// Jalankan mwave.
pub fn run(args: &WaveArgs) -> Result<(), SimError> {
    match args {
        WaveArgs::Merge { inputs, output } => merge(inputs, output.as_deref()),
        WaveArgs::Export {
            input,
            format,
            output,
        } => export(input, format, output.as_deref()),
        WaveArgs::Filter {
            input,
            signals,
            output,
        } => filter(input, signals, output.as_deref()),
        WaveArgs::Compare { a, b } => compare(a, b),
        WaveArgs::Search { input, patterns } => search(input, patterns),
        WaveArgs::Tree { input } => tree(input),
        WaveArgs::Stats { input } => stats(input),
        WaveArgs::Get {
            input,
            signals,
            at,
            range,
        } => get(input, signals, *at, *range),
        WaveArgs::Decode { input, proto } => decode(input, proto),
        WaveArgs::Debug { input, output } => debug(input, output.as_deref()),
    }
}
