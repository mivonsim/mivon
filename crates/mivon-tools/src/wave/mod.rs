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
pub(crate) mod compare;
pub(crate) mod data;
pub(crate) mod decode;
pub(crate) mod export;
pub(crate) mod filter;
pub(crate) mod get;
pub(crate) mod merge;
pub(crate) mod search;
pub(crate) mod stats;
pub(crate) mod tree;
pub(crate) mod util;

#[cfg(test)]
mod tests;

/// API publik mwave — dipakai dispatcher CLI (`mivon wave`).
pub use args::{run, WaveArgs};
/// Re-export API publik lama (jalur `wave::VcdData` / `wave::*_decode`
/// sebelum pemecahan modul).
pub use data::VcdData;
pub use decode::{DecodedTx, ahb_decode, apb_decode, axi4lite_decode};
