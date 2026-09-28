//! PLI — Programming Language Interface (IEEE 1364 Verilog PLI 1.0/2.0).
//!
//! Adapter ABI-compatible di atas kernel Mivon (arsitektur masukan user
//! poin 2). Dua mekanisme:
//!
//! - **tf** (task/function): akses argumen system task/function dari C
//!   (`tf_getp`, `tf_putp`, `tf_strgetp`, `tf_gettime`, ...).
//! - **acc** (access): navigasi object desain (`acc_handle_signal`,
//!   `acc_fetch_value`, `acc_next`, ...).
//!
//! Library eksternal tetap C ABI — tidak diterjemahkan ke Rust (poin 3).

pub mod acc;
pub mod loader;
pub mod tf;

pub use tf::{plio_error, plio_warning};

/// Lock koordinasi SATU-SATUNYA utk state global PLI (tf registry, acc map)
/// saat di-test. Test `pli::tf` memegang instance lalu meng-assert nilai;
/// test sim mana pun yang mencapai end-of-sim memanggil `pli_cleanup()` →
/// `tf_clear_all()`/`acc_close()` yang mengosongkan map global — tanpa lock
/// bersama ini keduanya race: assertion `tf_getp` gagal → panic saat pegang
/// kunci test → kunci poisoned → SELURUH test tf ikut gagal (flaky workspace
/// yang teramati: 3 test `pli::tf` gagal serentak, run ulang lulus).
/// Hanya `#[cfg(test)]` — nol overhead di build release.
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Ambil `TEST_LOCK` tanpa peduli poisoning (test lain yang panik tak boleh
/// mematikan cleanup). Hanya ada di test build.
#[cfg(test)]
pub(crate) fn test_lock_guard() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
