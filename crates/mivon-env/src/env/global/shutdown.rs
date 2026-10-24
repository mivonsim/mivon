use crate::env::global::GlobalEnv;
use std::sync::Arc;

/// Lifecycle shutdown (doc/env.md): membuang resource milik context.
///
/// Urutan kebalikan dari startup. Semua operasi best-effort — shutdown
/// tidak boleh gagal (hasil dibuang, log ke stderr bila ada masalah).
pub fn shutdown(env: &mut GlobalEnv) {
    // Telemetry: tutup dengan jejak akhir.
    env.telemetry
        .trace("shutdown", &format!("uptime={:?}", env.uptime()));

    // Database: simpan perubahan (best-effort; hanya bila refcount = 1).
    if let Some(db) = Arc::get_mut(&mut env.database) {
        if let Err(e) = db.save() {
            eprintln!("[env] database save warning: {}", e);
        }
    }

    // Diagnostics: drain tersimpan (untuk laporan akhir caller).
    let _ = env.diagnostics.drain();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shutdown_no_panic() {
        let mut env = crate::env::global::startup::startup().expect("startup gagal");
        shutdown(&mut env); // harus berjalan tanpa panic
    }
}
