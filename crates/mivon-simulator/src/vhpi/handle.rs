//! VHPI Handle System — opaque u64 handle + object registry.
//!
//! Arsitektur masukan user poin 6: library eksternal tidak pernah menerima
//! pointer internal Mivon. `VhpiHandle` adalah `u64` id; object Mivon
//! disimpan di registry (leak-safe, di-clear di end-of-simulation).

use mivon_core::intern::Symbol;
use mivon_ir::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::sync::OnceLock;

/// Jenis object VHPI.
#[derive(Debug, Clone)]
pub enum VhpiObjectKind {
    Null,
    Module(usize, Symbol),
    Signal(SignalId, usize),
    Port(SignalId, usize),
    Process(usize),
    Scope(String),
    Time(u64),
    Iterator {
        items: Vec<VhpiHandle>,
        cursor: usize,
    },
}

#[derive(Debug, Clone)]
pub struct VhpiObject {
    pub kind: VhpiObjectKind,
    pub id: u64,
}

// VhpiObject berisi handle raw pointer (VhpiHandle) — aman karena hanya
// dipakai di belakang Mutex / di FFI boundary dengan locking yang benar.
unsafe impl Send for VhpiObject {}
unsafe impl Sync for VhpiObject {}

impl VhpiObject {
    pub fn new(kind: VhpiObjectKind) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        VhpiObject {
            kind,
            id: NEXT_ID.fetch_add(1, Ordering::SeqCst),
        }
    }
}

/// Handle VHPI — opaque (u64 id), bukan pointer.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VhpiHandle {
    pub ptr: *mut std::ffi::c_void,
}

impl VhpiHandle {
    pub const NULL: VhpiHandle = VhpiHandle {
        ptr: std::ptr::null_mut(),
    };
    pub fn is_null(&self) -> bool {
        self.ptr.is_null()
    }
    pub fn is_valid(&self) -> bool {
        !self.ptr.is_null()
    }
}

fn vhpi_objects() -> &'static Mutex<std::collections::HashMap<u64, VhpiObject>> {
    static MAP: OnceLock<Mutex<std::collections::HashMap<u64, VhpiObject>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

pub(crate) fn register_object(kind: VhpiObjectKind) -> VhpiHandle {
    let obj = VhpiObject::new(kind);
    let id = obj.id;
    vhpi_objects().lock().unwrap().insert(id, obj);
    VhpiHandle {
        ptr: id as *mut std::ffi::c_void,
    }
}

/// Ekspos registrasi object untuk test e2e (mivon-tests) — pola sama dgn
/// vpi handle test. Produksi memakai vhpi_handle_by_name / iterator.
pub fn register_object_for_test(kind: VhpiObjectKind) -> VhpiHandle {
    register_object(kind)
}

pub(crate) fn lookup(handle: VhpiHandle) -> Option<VhpiObject> {
    if handle.is_null() {
        return None;
    }
    let id = handle.ptr as u64;
    vhpi_objects().lock().unwrap().get(&id).cloned()
}

/// vhpi_release_handle(handle) — bebaskan object.
pub fn vhpi_release_handle(handle: VhpiHandle) -> i32 {
    if handle.is_null() {
        return 0;
    }
    let id = handle.ptr as u64;
    let mut reg = vhpi_objects().lock().unwrap();
    if reg.remove(&id).is_some() {
        1
    } else {
        0
    }
}

/// Bersihkan semua (end of simulation).
pub(crate) fn vhpi_clear_all_objects() {
    vhpi_objects().lock().unwrap().clear();
}

/// Akses registry untuk iterator scan (mutate cursor). Dipakai iterator.rs.
pub(crate) fn vhpi_objects_for_scan(
) -> std::sync::MutexGuard<'static, std::collections::HashMap<u64, VhpiObject>> {
    vhpi_objects().lock().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    // FIX flake (ROUND 106): lock bersama lintas modul vhpi — registry
    // global bisa di-clear oleh test modul lain saat paralel.
    use crate::vhpi::VHPI_TEST_LOCK as TEST_LOCK;

    #[test]
    fn test_vhpi_handle_registry_roundtrip() {
        let _g = TEST_LOCK.lock().unwrap();
        vhpi_clear_all_objects();
        let h = register_object(VhpiObjectKind::Signal(0, 0));
        assert!(h.is_valid());
        let obj = lookup(h).expect("object ada");
        assert!(matches!(obj.kind, VhpiObjectKind::Signal(0, _)));
        assert_eq!(vhpi_release_handle(h), 1, "release sukses");
        assert!(lookup(h).is_none());
        assert_eq!(vhpi_release_handle(h), 0, "release ganda → 0");
        vhpi_clear_all_objects();
    }

    #[test]
    fn test_vhpi_handle_null_semantics() {
        // FIX flake: register_object menyentuh registry global — tanpa lock,
        // clear_all test paralel (registry_roundtrip/clear_all) menghapus
        // entry di tengah → panic menular (poison CACHE) ke test sim lain
        // (teramati: 6 test gagal serentak saat cargo test --workspace).
        let _g = TEST_LOCK.lock().unwrap();
        assert!(VhpiHandle::NULL.is_null());
        assert!(!VhpiHandle::NULL.is_valid());
        let h = register_object(VhpiObjectKind::Scope("s".to_string()));
        assert!(!h.is_null());
        assert_eq!(vhpi_release_handle(VhpiHandle::NULL), 0);
        vhpi_clear_all_objects();
    }

    #[test]
    fn test_vhpi_clear_all() {
        let _g = TEST_LOCK.lock().unwrap();
        vhpi_clear_all_objects();
        for _ in 0..4 {
            let _ = register_object(VhpiObjectKind::Process(0));
        }
        assert_eq!(vhpi_objects().lock().unwrap().len(), 4);
        vhpi_clear_all_objects();
        assert_eq!(vhpi_objects().lock().unwrap().len(), 0);
    }
}
