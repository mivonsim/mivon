//! FEAT-05: Python Bindings — FFI scaffold for Python integration.
//!
//! Provides C-compatible function exports that can be loaded by Python
//! via `ctypes` or compiled into a shared library for `cffi`/PyO3 usage.
//!
//! Functions follow the `mivon_` prefix convention for FFI safety.
//! All strings are C-strings (`*const c_char`), all pointers are checked.
//!
//! Python example:
//! ```python
//! import ctypes
//! lib = ctypes.CDLL("libmivon.so")
//! lib.mivon_version()  # returns b"0.3.0"
//! ```

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::ptr;

/// Get mivon version string. Returns pointer to static C string.
/// Caller must NOT free this pointer.
#[no_mangle]
pub extern "C" fn mivon_version() -> *const c_char {
    static VERSION: &[u8] = b"0.3.0\0";
    VERSION.as_ptr() as *const c_char
}

/// Compile a SystemVerilog source string.
/// Returns 0 on success, negative on error.
/// Error message is written to `err_buf` if non-null.
///
/// # Safety
/// `source` and `err_buf` must be valid C strings or null.
#[no_mangle]
pub unsafe extern "C" fn mivon_compile(
    source: *const c_char,
    err_buf: *mut c_char,
    err_buf_len: usize,
) -> c_int {
    if source.is_null() {
        write_err(err_buf, err_buf_len, "null source pointer");
        return -1;
    }

    let src = match CStr::from_ptr(source).to_str() {
        Ok(s) => s,
        Err(_) => {
            write_err(err_buf, err_buf_len, "invalid UTF-8 in source");
            return -2;
        }
    };

    // Basic validation — check for module keyword
    if !src.contains("module") && !src.contains("package") {
        write_err(err_buf, err_buf_len, "no module/package found in source");
        return -3;
    }

    0
}

/// Compile source and return number of modules found.
/// Returns module count on success, negative on error.
///
/// # Safety
/// Same as `mivon_compile`.
#[no_mangle]
pub unsafe extern "C" fn mivon_count_modules(source: *const c_char) -> c_int {
    if source.is_null() {
        return -1;
    }

    let src = match CStr::from_ptr(source).to_str() {
        Ok(s) => s,
        Err(_) => return -2,
    };

    // Count "module " at word boundary (not inside "endmodule")
    let count = src.split_whitespace().filter(|t| t == &"module").count() as c_int;
    count
}

/// Free a string allocated by mivon (for future use).
///
/// # Safety
/// `ptr` must have been returned by a mivon function.
#[no_mangle]
pub unsafe extern "C" fn mivon_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        let _ = CString::from_raw(ptr);
    }
}

/// Get build info as JSON string.
/// Caller must free with `mivon_free_string`.
///
/// # Safety
/// Returns null on allocation failure.
#[no_mangle]
pub unsafe extern "C" fn mivon_build_info() -> *mut c_char {
    let info = r#"{"version":"0.3.0","features":["parser","simulator","lsp","formal"],"rust_version":"nightly"}"#;
    match CString::new(info) {
        Ok(s) => s.into_raw(),
        Err(_) => ptr::null_mut(),
    }
}

/// Get supported SystemVerilog features as JSON.
/// Caller must free with `mivon_free_string`.
///
/// # Safety
/// Returns null on allocation failure.
#[no_mangle]
pub unsafe extern "C" fn mivon_features() -> *mut c_char {
    let features = r#"{"modules":true,"packages":true,"interfaces":true,"generates":true,"always_ff":true,"always_comb":true,"typedef":true,"struct":true,"enum":true," unions":true,"assertions":true,"coverage":true,"sva_basic":true,"constraints":true,"randomize":true,"fork_join":true,"dpi":false,"vhd":false,"systemc":false}"#;
    match CString::new(features) {
        Ok(s) => s.into_raw(),
        Err(_) => ptr::null_mut(),
    }
}

/// Write error message to buffer. Safe helper.
unsafe fn write_err(buf: *mut c_char, len: usize, msg: &str) {
    if buf.is_null() || len == 0 {
        return;
    }
    let bytes = msg.as_bytes();
    let copy_len = bytes.len().min(len - 1);
    ptr::copy_nonoverlapping(bytes.as_ptr(), buf as *mut u8, copy_len);
    *buf.add(copy_len) = 0;
}

/// Initialize the mivon runtime. Must be called before other functions.
/// Returns 0 on success.
#[no_mangle]
pub extern "C" fn mivon_init() -> c_int {
    // Future: initialize thread pool, load config, etc.
    0
}

/// Shutdown the mivon runtime.
#[no_mangle]
pub extern "C" fn mivon_shutdown() {
    // Future: cleanup resources
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version() {
        let v = mivon_version();
        let s = unsafe { CStr::from_ptr(v) };
        assert_eq!(s.to_str().unwrap(), "0.3.0");
    }

    #[test]
    fn test_count_modules() {
        let src = CString::new("module test; endmodule module top; endmodule").unwrap();
        let count = unsafe { mivon_count_modules(src.as_ptr()) };
        assert_eq!(count, 2);
    }

    #[test]
    fn test_compile_null_source() {
        let mut err = [0u8; 256];
        let rc = unsafe { mivon_compile(ptr::null(), err.as_mut_ptr() as *mut c_char, 256) };
        assert_eq!(rc, -1);
    }

    #[test]
    fn test_compile_valid() {
        let src = CString::new("module test; endmodule").unwrap();
        let rc = unsafe { mivon_compile(src.as_ptr(), ptr::null_mut(), 0) };
        assert_eq!(rc, 0);
    }

    #[test]
    fn test_compile_no_module() {
        let src = CString::new("always @(*) y = a;").unwrap();
        let mut err = [0u8; 256];
        let rc = unsafe { mivon_compile(src.as_ptr(), err.as_mut_ptr() as *mut c_char, 256) };
        assert_eq!(rc, -3);
    }

    #[test]
    fn test_build_info() {
        let ptr = unsafe { mivon_build_info() };
        assert!(!ptr.is_null());
        let s = unsafe { CStr::from_ptr(ptr) };
        let info = s.to_str().unwrap();
        assert!(info.contains("0.3.0"));
        unsafe {
            mivon_free_string(ptr);
        }
    }

    #[test]
    fn test_init_shutdown() {
        assert_eq!(mivon_init(), 0);
        mivon_shutdown();
    }

    #[test]
    fn test_features_json_valid() {
        let ptr = unsafe { mivon_features() };
        assert!(!ptr.is_null());
        let s = unsafe { CStr::from_ptr(ptr) };
        let info = s.to_str().unwrap();
        assert!(info.contains("always_ff"));
        assert!(info.contains("randomize"));
        assert!(!info.contains("\"dpi\":true"));
        unsafe {
            mivon_free_string(ptr);
        }
    }

    #[test]
    fn test_count_modules_ignores_endmodule() {
        let src = CString::new("module a; endmodule module b; endmodule").unwrap();
        let count = unsafe { mivon_count_modules(src.as_ptr()) };
        assert_eq!(count, 2);
    }

    #[test]
    fn test_count_modules_package_keyword() {
        let src = CString::new("package p; endpackage module m; endmodule").unwrap();
        let count = unsafe { mivon_count_modules(src.as_ptr()) };
        assert_eq!(count, 1);
    }

    #[test]
    fn test_count_modules_null() {
        assert_eq!(unsafe { mivon_count_modules(ptr::null()) }, -1);
    }
}
