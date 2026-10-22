//! `mwave` — Wave Utility (direktori, 1 subkomando 1 file).
//!
//! Bukan viewer. Subkomando:
//! - `merge`   — gabungkan beberapa VCD jadi satu (offset waktu kumulatif)
//! - `export`  — VCD → CSV/TXT
//! - `filter`  — pertahankan subset sinyal
//! - `compare` — bandingkan 2 VCD (perbedaan nilai per signal, WAV-09)
//! - `search`  — cari sinyal by pola wildcard (WAV-10)
//! - `tree`    — index hierarki scope + sinyal (WAV-08)
//! - `stats`   — statistik per sinyal: toggle, transitions, aktivitas (WAV-17)
//! - `get`     — random access nilai sinyal: `--at T` / `--range t1:t2` /
//!   timeline penuh (WAV-07)
//! - `decode`  — protokol-aware decode transaksi bus dari VCD: apb /
//!   axi4lite / ahb (WAV-16)
//!
//! Peta modul:
//! - `args`   — enum [`WaveArgs`] + dispatcher [`run`]
//! - `data`   — parser/writer VCD ([`data::parse_vcd`], [`data::write_vcd`],
//!   [`data::VcdData`])
//! - `util`   — helper bersama (err, nama penuh, wildcard, sample, timeline)
//! - satu modul per subkomando: `merge`, `export`, `filter`, `compare`,
//!   `search`, `tree`, `get`, `stats`, `decode`
//! - `tests`  — unit test per subkomando

pub mod args;
pub mod compare;
pub mod data;
pub mod decode;
pub mod export;
pub mod filter;
pub mod get;
pub mod merge;
pub mod search;
pub mod stats;
pub mod tree;
pub mod util;

#[cfg(test)]
mod tests;

/// API publik mwave — dipakai dispatcher CLI (`mivon wave`).
pub use args::{run, WaveArgs};
