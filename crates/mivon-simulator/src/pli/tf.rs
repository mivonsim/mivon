//! PLI tf — Task/Function interface (IEEE 1364 PLI 1.0).
//!
//! Memungkinkan library C mengakses argumen system task/function:
//! `tf_getp(tfinst, n)` membaca argumen ke-n, `tf_putp` menulis balik,
//! `tf_gettime` membaca waktu sim, dll. Argumen disimpan dalam registry
//! per-instance (keyed `tfinst` u32) — di-isi engine saat system task
//! terdaftar dieksekusi, dibaca library C.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

/// Nilai argumen PLI — `Vec<u8>` mewakili bit vector (LSB-first), `i64`
/// nilai integer, `String` untuk string.
#[derive(Debug, Clone, PartialEq)]
pub enum PliArg {
    Int(i64),
    BitVec(Vec<u8>),
    Str(String),
    Real(f64),
}

fn tf_registry() -> &'static Mutex<HashMap<u32, Vec<PliArg>>> {
    static R: OnceLock<Mutex<HashMap<u32, Vec<PliArg>>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_TFINST: AtomicU32 = AtomicU32::new(1);

/// Buat instance task/function baru + simpan argumen. Kembalikan `tfinst`.
pub fn tf_create_instance(args: Vec<PliArg>) -> u32 {
    let id = NEXT_TFINST.fetch_add(1, Ordering::SeqCst);
    tf_registry().lock().unwrap().insert(id, args);
    id
}

/// Hapus instance (dipanggil engine setelah task selesai).
pub fn tf_free_instance(tfinst: u32) {
    tf_registry().lock().unwrap().remove(&tfinst);
}

// tf_getinstance() — instance aktif (thread-local, di-set engine).
thread_local! {
    static CURRENT_TFINST: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

pub fn tf_set_current_instance(tfinst: u32) {
    CURRENT_TFINST.with(|c| c.set(tfinst));
}

/// tf_getinstance() — u32 handle instance aktif (0 bila tidak ada).
pub fn tf_getinstance() -> u32 {
    CURRENT_TFINST.with(|c| c.get())
}

/// tf_getp(tfinst, n) — argumen ke-n sebagai i32 (1-based; 0 = tanpa arg).
pub fn tf_getp(tfinst: u32, n: i32) -> i32 {
    let reg = tf_registry().lock().unwrap();
    match reg
        .get(&tfinst)
        .and_then(|args| args.get((n - 1).max(0) as usize))
    {
        Some(PliArg::Int(v)) => *v as i32,
        Some(PliArg::BitVec(bits)) => bits_to_u64(bits) as i32,
        _ => 0,
    }
}

/// tf_getlongp(tfinst, n) — argumen ke-n sebagai i64 (vector lebar).
pub fn tf_getlongp(tfinst: u32, n: i32) -> i64 {
    let reg = tf_registry().lock().unwrap();
    match reg
        .get(&tfinst)
        .and_then(|args| args.get((n - 1).max(0) as usize))
    {
        Some(PliArg::Int(v)) => *v,
        Some(PliArg::BitVec(bits)) => bits_to_u64(bits) as i64,
        _ => 0,
    }
}

/// tf_putp(tfinst, n, value) — tulis balik argumen ke-n (i32).
pub fn tf_putp(tfinst: u32, n: i32, value: i32) -> i32 {
    let mut reg = tf_registry().lock().unwrap();
    if let Some(args) = reg.get_mut(&tfinst) {
        let idx = (n - 1).max(0) as usize;
        if idx < args.len() {
            args[idx] = PliArg::Int(value as i64);
            return 0;
        }
    }
    -1
}

/// tf_putlongp(tfinst, n, value) — tulis balik (i64).
pub fn tf_putlongp(tfinst: u32, n: i32, value: i64) -> i32 {
    let mut reg = tf_registry().lock().unwrap();
    if let Some(args) = reg.get_mut(&tfinst) {
        let idx = (n - 1).max(0) as usize;
        if idx < args.len() {
            args[idx] = PliArg::Int(value);
            return 0;
        }
    }
    -1
}

/// tf_strgetp(tfinst, n) — argumen ke-n sebagai string.
pub fn tf_strgetp(tfinst: u32, n: i32) -> String {
    let reg = tf_registry().lock().unwrap();
    match reg
        .get(&tfinst)
        .and_then(|args| args.get((n - 1).max(0) as usize))
    {
        Some(PliArg::Str(s)) => s.clone(),
        Some(PliArg::Int(v)) => v.to_string(),
        Some(PliArg::BitVec(bits)) => bits_to_u64(bits).to_string(),
        _ => String::new(),
    }
}

/// tf_strputp(tfinst, n, value) — tulis balik argumen sebagai string.
pub fn tf_strputp(tfinst: u32, n: i32, value: &str) -> i32 {
    let mut reg = tf_registry().lock().unwrap();
    if let Some(args) = reg.get_mut(&tfinst) {
        let idx = (n - 1).max(0) as usize;
        if idx < args.len() {
            args[idx] = PliArg::Str(value.to_string());
            return 0;
        }
    }
    -1
}

/// tf_sizep(tfinst, n) — lebar bit argumen ke-n.
pub fn tf_sizep(tfinst: u32, n: i32) -> i32 {
    let reg = tf_registry().lock().unwrap();
    match reg
        .get(&tfinst)
        .and_then(|args| args.get((n - 1).max(0) as usize))
    {
        Some(PliArg::BitVec(bits)) => bits.len() as i32,
        Some(PliArg::Int(_)) => 32,
        _ => 0,
    }
}

// tf_gettime() — waktu sim saat ini (di-set engine tiap cycle).
thread_local! {
    static CURRENT_TIME: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

pub fn tf_set_current_time(t: u64) {
    CURRENT_TIME.with(|c| c.set(t));
}

pub fn tf_gettime() -> u64 {
    CURRENT_TIME.with(|c| c.get())
}

/// Helper: bit vector LSB-first → u64.
pub fn bits_to_u64(bits: &[u8]) -> u64 {
    let mut v = 0u64;
    for (i, b) in bits.iter().enumerate() {
        if *b != 0 && i < 64 {
            v |= 1u64 << i;
        }
    }
    v
}

/// plio_warning / plio_error — output PLI ke stderr/stdout.
pub fn plio_warning(msg: &str) {
    eprintln!("PLI WARNING: {}", msg);
}

pub fn plio_error(msg: &str) {
    eprintln!("PLI ERROR: {}", msg);
}

/// Bersihkan semua instance (end of simulation).
///
/// TEST-SYNC: jangan ambil `pli::TEST_LOCK` di sini (bukan fungsi pub test) —
/// ada test yang memanggil ini DALAM kunci yang sama → deadlock. Koordinasi
/// dilakukan di `pli_cleanup` (satu-satunya pemanggil runtime), lihat
/// pli::TEST_LOCK di mod.rs.
pub fn tf_clear_all() {
    tf_registry().lock().unwrap().clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tf_get_put_roundtrip() {
        let _g = crate::pli::test_lock_guard();
        let inst = tf_create_instance(vec![PliArg::Int(42), PliArg::Str("hello".into())]);
        assert_eq!(tf_getp(inst, 1), 42);
        assert_eq!(tf_getlongp(inst, 1), 42);
        assert_eq!(tf_strgetp(inst, 2), "hello");
        // putp
        assert_eq!(tf_putp(inst, 1, 99), 0);
        assert_eq!(tf_getp(inst, 1), 99);
        assert_eq!(tf_putp(inst, 1, 0), 0); // idempotent
        assert_eq!(tf_strputp(inst, 2, "world"), 0);
        assert_eq!(tf_strgetp(inst, 2), "world");
        // argumen ke-0 / di luar range → 0
        assert_eq!(tf_getp(inst, 0), 0);
        assert_eq!(tf_getp(inst, 99), 0);
        tf_free_instance(inst);
        assert_eq!(tf_getp(inst, 1), 0, "setelah free → 0");
    }

    #[test]
    fn test_tf_bitvec_getp() {
        let _g = crate::pli::test_lock_guard();
        // bits LSB-first: [1,1,0,1] = 0b1011 = 11
        let inst = tf_create_instance(vec![PliArg::BitVec(vec![1, 1, 0, 1])]);
        assert_eq!(tf_getp(inst, 1), 11);
        assert_eq!(tf_sizep(inst, 1), 4);
        assert_eq!(tf_getlongp(inst, 1), 11);
        tf_free_instance(inst);
    }

    #[test]
    fn test_tf_getinstance_and_time() {
        let _g = crate::pli::test_lock_guard();
        tf_set_current_instance(7);
        assert_eq!(tf_getinstance(), 7);
        tf_set_current_time(1234);
        assert_eq!(tf_gettime(), 1234);
        // reset utk test lain
        tf_set_current_instance(0);
        tf_set_current_time(0);
    }

    #[test]
    fn test_bits_to_u64() {
        assert_eq!(bits_to_u64(&[]), 0);
        assert_eq!(bits_to_u64(&[1, 0, 1]), 5); // 0b101
                                                // bit ke-63 → 1<<63
        let mut bits = vec![0u8; 64];
        bits[63] = 1;
        assert_eq!(bits_to_u64(&bits), 1u64 << 63);
    }
}
