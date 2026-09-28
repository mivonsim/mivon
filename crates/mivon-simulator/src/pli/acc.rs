//! PLI acc — Access routines (IEEE 1364 PLI 1.0).
//!
//! Navigasi object desain dari library C: `acc_handle_signal` (signal by
//! name), `acc_fetch_value` (baca nilai), `acc_fetch_name`/`acc_fetch_fullname`,
//! `acc_fetch_type`, `acc_next` (iterasi), `acc_initialize`/`acc_close`.
//!
//! Handle acc adalah `u64` id (arsitektur poin 6) — library eksternal tidak
//! pernah menerima pointer internal Mivon.

use mivon_core::intern::Symbol;
use mivon_ir::{LogicVal, LogicVec, SignalId, SignalKind};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// Handle acc — opaque u64.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccHandle {
    pub ptr: *mut std::ffi::c_void,
}

impl AccHandle {
    pub const NULL: AccHandle = AccHandle {
        ptr: std::ptr::null_mut(),
    };
    pub fn is_null(&self) -> bool {
        self.ptr.is_null()
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) enum AccObject {
    Signal(SignalId, usize),
    Port(SignalId, usize),
    Module(usize, Symbol),
    Scope(String),
    Iterator {
        items: Vec<AccHandle>,
        cursor: usize,
    },
    TfArg(u32, i32),
}

// AccObject berisi AccHandle (raw pointer) — hanya dipakai di belakang
// Mutex/FFI boundary dengan locking (pola vpi::types unsafe impl).
unsafe impl Send for AccObject {}
unsafe impl Sync for AccObject {}

fn acc_objects() -> &'static Mutex<HashMap<u64, AccObject>> {
    static MAP: OnceLock<Mutex<HashMap<u64, AccObject>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register(kind: AccObject) -> AccHandle {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    acc_objects().lock().unwrap().insert(id, kind);
    AccHandle {
        ptr: id as *mut std::ffi::c_void,
    }
}

fn lookup(h: AccHandle) -> Option<AccObject> {
    if h.is_null() {
        return None;
    }
    let id = h.ptr as u64;
    acc_objects().lock().unwrap().get(&id).cloned()
}

/// acc_initialize() — inisialisasi access routines. Kembalikan handle scope
/// top (atau NULL bila belum ada engine — dipanggil library saat sim mulai).
pub fn acc_initialize() -> AccHandle {
    super::super::vpi::with_vpi_engine(|engine| {
        register(AccObject::Module(0, engine.design.top.name))
    })
    .unwrap_or(AccHandle::NULL)
}

/// acc_close() — bersihkan semua handle acc.
pub fn acc_close() {
    acc_objects().lock().unwrap().clear();
}

/// acc_handle_signal(name) — signal by (hierarchical) name.
pub fn acc_handle_signal(name: &str) -> AccHandle {
    super::super::vpi::with_vpi_engine(|engine| {
        for (sig_id, sig) in engine.design.top.signals.iter().enumerate() {
            if sig.name.as_str() == name {
                return register(AccObject::Signal(sig_id, 0));
            }
            let full = format!("{}.{}", engine.design.top.name.as_str(), sig.name.as_str());
            if full == name {
                return register(AccObject::Signal(sig_id, 0));
            }
        }
        AccHandle::NULL
    })
    .unwrap_or(AccHandle::NULL)
}

/// acc_handle_tfarg(n) — argumen task/function ke-n sebagai handle signal.
pub fn acc_handle_tfarg(n: i32) -> AccHandle {
    let inst = super::tf::tf_getinstance();
    if inst == 0 {
        return AccHandle::NULL;
    }
    register(AccObject::TfArg(inst, n))
}

/// acc_fetch_value(handle, format, delay) — baca nilai signal.
/// format `'b'` (binary) → string biner, `'d'`/`'h'` → desimal/hex.
pub fn acc_fetch_value(handle: AccHandle, format: u8, _delay: i32) -> String {
    let obj = match lookup(handle) {
        Some(o) => o,
        None => return String::new(),
    };
    match &obj {
        AccObject::Signal(sig_id, _) | AccObject::Port(sig_id, _) => {
            super::super::vpi::with_vpi_engine(|engine| {
                let val = engine.state.read_signal(*sig_id).clone();
                acc_format_value(&val, format)
            })
            .unwrap_or_default()
        }
        AccObject::TfArg(inst, n) => {
            let v = super::tf::tf_getlongp(*inst, *n);
            match format as char {
                'd' => v.to_string(),
                'h' => format!("{:x}", v as u64),
                _ => format!("{:b}", v as u64),
            }
        }
        _ => String::new(),
    }
}

/// Format LogicVec sesuai karakter format acc (`b`/`d`/`h`/`o`).
pub fn acc_format_value(lv: &LogicVec, format: u8) -> String {
    match format as char {
        'd' => lv.to_u64().to_string(),
        'h' => format!("{:x}", lv.to_u64()),
        'o' => format!("{:o}", lv.to_u64()),
        _ => {
            // binary MSB-first
            lv.bits
                .iter()
                .rev()
                .map(|b| match b {
                    LogicVal::Zero => '0',
                    LogicVal::One => '1',
                    LogicVal::X => 'x',
                    LogicVal::Z => 'z',
                })
                .collect()
        }
    }
}

/// acc_fetch_name(handle) — nama local.
pub fn acc_fetch_name(handle: AccHandle) -> String {
    let obj = match lookup(handle) {
        Some(o) => o,
        None => return String::new(),
    };
    match &obj {
        AccObject::Signal(sig_id, _) | AccObject::Port(sig_id, _) => {
            super::super::vpi::with_vpi_engine(|e| {
                e.design
                    .top
                    .signals
                    .get(*sig_id)
                    .map(|s| s.name.to_string())
            })
            .flatten()
            .unwrap_or_default()
        }
        AccObject::Module(_, name) => name.as_str().to_string(),
        AccObject::Scope(name) => name.clone(),
        _ => String::new(),
    }
}

/// acc_fetch_fullname(handle) — nama hierarkis penuh.
pub fn acc_fetch_fullname(handle: AccHandle) -> String {
    let obj = match lookup(handle) {
        Some(o) => o,
        None => return String::new(),
    };
    match &obj {
        AccObject::Signal(sig_id, _) | AccObject::Port(sig_id, _) => {
            super::super::vpi::with_vpi_engine(|e| {
                e.design
                    .top
                    .signals
                    .get(*sig_id)
                    .map(|s| format!("{}.{}", e.design.top.name.as_str(), s.name.as_str()))
            })
            .flatten()
            .unwrap_or_default()
        }
        AccObject::Module(_, name) => name.as_str().to_string(),
        _ => String::new(),
    }
}

/// acc_fetch_type(handle) — jenis object (char code Verilog: n=net, r=reg,
/// p=port, m=module, ...).
pub fn acc_fetch_type(handle: AccHandle) -> u8 {
    let obj = match lookup(handle) {
        Some(o) => o,
        None => return 0,
    };
    match &obj {
        AccObject::Signal(_, _) => b'r', // register/var
        AccObject::Port(_, _) => b'p',
        AccObject::Module(_, _) => b'm',
        AccObject::Scope(_) => b's',
        _ => 0,
    }
}

/// acc_next(type, handle) — iterasi signal modul (accNextSignal).
pub fn acc_next(vpi_type: u8, ref_handle: AccHandle) -> AccHandle {
    let obj = match lookup(ref_handle) {
        Some(o) => o,
        None => return AccHandle::NULL,
    };
    match &obj {
        AccObject::Module(_, _) => super::super::vpi::with_vpi_engine(|engine| {
            let items: Vec<AccHandle> = engine
                .design
                .top
                .signals
                .iter()
                .enumerate()
                .filter(|(_, s)| match vpi_type {
                    b'n' => s.kind == SignalKind::Wire,
                    b'r' => s.kind == SignalKind::Reg || s.kind == SignalKind::Logic,
                    _ => true,
                })
                .map(|(i, _)| register(AccObject::Signal(i, 0)))
                .collect();
            if items.is_empty() {
                return AccHandle::NULL;
            }
            register(AccObject::Iterator { items, cursor: 0 })
        })
        .unwrap_or(AccHandle::NULL),
        AccObject::Iterator { items, cursor } => {
            let idx = *cursor;
            if idx >= items.len() {
                return AccHandle::NULL;
            }
            // update cursor di registry
            if let Some(AccObject::Iterator { cursor: c, .. }) = acc_objects()
                .lock()
                .unwrap()
                .get_mut(&(ref_handle.ptr as u64))
            {
                *c = idx + 1;
            }
            items[idx]
        }
        _ => AccHandle::NULL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_acc_format_value() {
        let lv = LogicVec::from_u64(0b1011, 4);
        assert_eq!(acc_format_value(&lv, b'b'), "1011");
        assert_eq!(acc_format_value(&lv, b'd'), "11");
        assert_eq!(acc_format_value(&lv, b'h'), "b");
        // x/z
        let lvx = LogicVec {
            width: 2,
            bits: vec![LogicVal::X, LogicVal::One],
        };
        assert_eq!(acc_format_value(&lvx, b'b'), "1x");
    }

    #[test]
    fn test_acc_fetch_null_returns_empty() {
        // Murni: NULL handle → kosong/0 tanpa tergantung global engine.
        // (acc_handle_signal/acc_initialize bergantung VPI_ENGINE global yang
        // di-set engine test paralel lain — diuji e2e via simulate_signals.)
        let _g = crate::pli::test_lock_guard();
        acc_close();
        assert_eq!(acc_fetch_name(AccHandle::NULL), "");
        assert_eq!(acc_fetch_fullname(AccHandle::NULL), "");
        assert_eq!(acc_fetch_type(AccHandle::NULL), 0);
        assert_eq!(acc_fetch_value(AccHandle::NULL, b'b', 0), "");
        acc_close();
    }
}
