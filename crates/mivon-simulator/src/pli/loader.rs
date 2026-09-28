//! PLI Library Loader (arsitektur masukan user poin 2, 9, 10).
//!
//! Load library PLI → cari entry point (`veriusertfs` PLI table /
//! `vpi_startup`) → ABI validation → registrasi system task/function.
//! Murni adapter: library C ABI tidak diterjemahkan ke Rust (poin 3).

use crate::foreign::loader::AbiInfo;
use std::path::PathBuf;

/// Deskripsi library PLI yang sudah dimuat.
pub struct LoadedPli {
    pub path: PathBuf,
    pub abi: AbiInfo,
    #[cfg(feature = "dpi")]
    pub library: std::sync::Arc<libloading::Library>,
}

impl std::fmt::Debug for LoadedPli {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "LoadedPli {{ path: {:?}, abi: {:?} }}",
            self.path, self.abi
        )
    }
}

/// Cari + muat library PLI.
#[cfg(feature = "dpi")]
pub fn load_pli_library(name: &str) -> Result<LoadedPli, String> {
    use crate::foreign::loader::{current_abi, find_library, load_library};
    let search = crate::foreign::loader::default_search_paths();
    let path = find_library(name, &search)
        .ok_or_else(|| format!("PLI library '{}' not found in search paths", name))?;
    let library = load_library(&path)?;
    Ok(LoadedPli {
        path,
        abi: current_abi(),
        library,
    })
}

#[cfg(not(feature = "dpi"))]
pub fn load_pli_library(name: &str) -> Result<LoadedPli, String> {
    Err(format!(
        "PLI library '{}' loading requires feature \"dpi\"",
        name
    ))
}

/// Cek apakah library mengekspor symbol PLI yang diharapkan:
/// `veriusertfs` (PLI 1.0 table) atau `vpi_startup` (PLI 2.0).
#[cfg(feature = "dpi")]
pub fn has_pli_entry_points(pli: &LoadedPli) -> bool {
    use crate::foreign::loader::resolve_symbol;
    type VoidFn = unsafe extern "C" fn();
    resolve_symbol::<VoidFn>(&pli.library, "veriusertfs").is_some()
        || resolve_symbol::<VoidFn>(&pli.library, "vpi_startup").is_some()
}

#[cfg(not(feature = "dpi"))]
pub fn has_pli_entry_points(_pli: &LoadedPli) -> bool {
    false
}

/// Panggil `vpi_startup` bila ada (PLI 2.0).
#[cfg(feature = "dpi")]
pub fn call_pli_startup(pli: &LoadedPli) -> Result<(), String> {
    use crate::foreign::loader::resolve_symbol;
    type InitFn = unsafe extern "C" fn() -> i32;
    if let Some(init) = resolve_symbol::<InitFn>(&pli.library, "vpi_startup") {
        let rc = unsafe { init() };
        if rc != 0 {
            return Err(format!("vpi_startup (PLI) returned {}", rc));
        }
    }
    Ok(())
}

#[cfg(not(feature = "dpi"))]
pub fn call_pli_startup(_pli: &LoadedPli) -> Result<(), String> {
    Ok(())
}

/// Bersihkan state PLI (end of simulation).
pub fn pli_cleanup() {
    // Satu-satunya titik ambil `pli::TEST_LOCK` (test build): pli_cleanup
    // dipanggil dari end-of-sim test sim mana pun — tanpa kunci ini,
    // `tf_clear_all()` mengosongkan registry di tengah test `pli::tf` yang
    // memegang instance + assert → panic saat pegang kunci → kunci poisoned
    // → 3 test tf gagal serentak (flaky `cargo test --workspace`).
    // Test yang memanggil clear LANGSUNG (tf_clear_all/acc_close) sudah
    // memegang kunci sendiri — jangan kunci ulang di dalam (deadlock).
    #[cfg(test)]
    let _g = crate::pli::test_lock_guard();
    super::tf::tf_clear_all();
    super::acc::acc_close();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foreign::loader::current_abi;

    #[test]
    fn test_load_missing_pli_library_errors() {
        let res = load_pli_library("definitely_missing_pli_lib_54321");
        assert!(res.is_err(), "library imajiner harus error");
    }

    #[test]
    fn test_current_abi() {
        let abi = current_abi();
        assert_eq!(abi.arch, std::env::consts::ARCH);
        assert_eq!(abi.os, std::env::consts::OS);
    }
}
