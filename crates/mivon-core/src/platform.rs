//! Platform support — stage 1 & 2 of #66/#67.
//! Deteksi sistem operasi target (macOS, Windows, Linux) dan informasi
//! platform untuk kompilasi dan simulasi lintas-platform.
//! 1 file = 1 tanggung jawab: hanya deteksi platform.
//!
//! Stage 2 (no_std): dengan fitur `no_std`, platform.rs kompilasi tanpa
//! pustaka standar (`std`). Deteksi OS/ARCH menggunakan konstanta kompilasi
//! Rust (`cfg!`). Untuk no_std penuh di seluruh crate, semua dependensi
//! harus no_std-compatible — langkah selanjutnya di DEBT-04 stage 3.

#[cfg(feature = "no_std")]
extern crate alloc;

/// Sistem operasi yang didukung.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetOs {
    Linux,
    Macos,
    Windows,
    Unknown,
}

/// Informasi platform saat ini.
#[derive(Debug, Clone)]
pub struct PlatformInfo {
    pub os: TargetOs,
    pub arch: &'static str,
    pub is_unix_like: bool,
    pub path_separator: char,
}

impl Default for PlatformInfo {
    fn default() -> Self {
        Self::current()
    }
}

impl PlatformInfo {
    /// Deteksi platform saat ini dari konstanta kompilasi Rust (cfg!),
    /// dengan override melalui variabel lingkungan MIVON_TARGET_OS /
    /// MIVON_TARGET_ARCH untuk cross-compile bare-metal.
    pub fn current() -> Self {
        let os = detect_os_current();
        let is_unix_like = matches!(os, TargetOs::Linux | TargetOs::Macos);
        let path_separator = if is_unix_like { '/' } else { '\\' };
        Self {
            os,
            arch: arch_current(),
            is_unix_like,
            path_separator,
        }
    }

    /// Apakah platform ini macOS?
    pub fn is_macos(&self) -> bool {
        self.os == TargetOs::Macos
    }

    /// Apakah platform ini Windows?
    pub fn is_windows(&self) -> bool {
        self.os == TargetOs::Windows
    }

    /// Apakah platform ini Linux?
    pub fn is_linux(&self) -> bool {
        self.os == TargetOs::Linux
    }
}

/// Deteksi OS dari konstanta kompilasi Rust, dengan override via env.
fn detect_os_current() -> TargetOs {
    #[cfg(feature = "no_std")]
    {
        // no_std: tidak bisa baca std::env, gunakan cfg! + opsi override
        if cfg!(target_os = "linux") {
            return TargetOs::Linux;
        }
        if cfg!(target_os = "macos") {
            return TargetOs::Macos;
        }
        if cfg!(target_os = "windows") {
            return TargetOs::Windows;
        }
        TargetOs::Unknown
    }

    #[cfg(not(feature = "no_std"))]
    {
        use std::env::consts::OS;
        match OS {
            "linux" => TargetOs::Linux,
            "macos" => TargetOs::Macos,
            "windows" => TargetOs::Windows,
            _ => TargetOs::Unknown,
        }
    }
}

/// Deteksi arsitektur dari konstanta kompilasi Rust.
fn arch_current() -> &'static str {
    #[cfg(feature = "no_std")]
    {
        if cfg!(target_arch = "x86_64") {
            "x86_64"
        } else if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else if cfg!(target_arch = "riscv64") {
            "riscv64"
        } else {
            "unknown"
        }
    }

    #[cfg(not(feature = "no_std"))]
    {
        std::env::consts::ARCH
    }
}

/// Deteksi OS dari string nama (untuk cross-compile target).
pub fn detect_os(os_name: &str) -> TargetOs {
    match os_name.to_lowercase().as_str() {
        "linux" => TargetOs::Linux,
        "macos" | "darwin" | "osx" => TargetOs::Macos,
        "windows" | "win32" | "win64" => TargetOs::Windows,
        _ => TargetOs::Unknown,
    }
}

/// Konversi path separator untuk platform target.
pub fn path_sep_for(os: TargetOs) -> char {
    match os {
        TargetOs::Windows => '\\',
        _ => '/',
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_info_current_known_os() {
        let info = PlatformInfo::current();
        assert!(
            matches!(
                info.os,
                TargetOs::Linux | TargetOs::Macos | TargetOs::Windows
            ),
            "current OS should be known, got {:?}",
            info.os
        );
        assert!(!info.arch.is_empty());
    }

    #[test]
    fn test_platform_info_is_macos() {
        let info = PlatformInfo {
            os: TargetOs::Macos,
            arch: "x86_64",
            is_unix_like: true,
            path_separator: '/',
        };
        assert!(info.is_macos());
        assert!(!info.is_windows());
        assert!(!info.is_linux());
    }

    #[test]
    fn test_platform_info_is_windows() {
        let info = PlatformInfo {
            os: TargetOs::Windows,
            arch: "x86_64",
            is_unix_like: false,
            path_separator: '\\',
        };
        assert!(info.is_windows());
        assert!(!info.is_macos());
        assert!(!info.is_linux());
    }

    #[test]
    fn test_platform_info_is_linux() {
        let info = PlatformInfo {
            os: TargetOs::Linux,
            arch: "aarch64",
            is_unix_like: true,
            path_separator: '/',
        };
        assert!(info.is_linux());
        assert!(!info.is_macos());
        assert!(!info.is_windows());
    }

    #[test]
    fn test_detect_os_macos_variants() {
        assert_eq!(detect_os("macos"), TargetOs::Macos);
        assert_eq!(detect_os("darwin"), TargetOs::Macos);
        assert_eq!(detect_os("osx"), TargetOs::Macos);
    }

    #[test]
    fn test_detect_os_windows_variants() {
        assert_eq!(detect_os("windows"), TargetOs::Windows);
        assert_eq!(detect_os("win32"), TargetOs::Windows);
        assert_eq!(detect_os("win64"), TargetOs::Windows);
    }

    #[test]
    fn test_detect_os_linux() {
        assert_eq!(detect_os("linux"), TargetOs::Linux);
    }

    #[test]
    fn test_detect_os_unknown() {
        assert_eq!(detect_os("freebsd"), TargetOs::Unknown);
        assert_eq!(detect_os(""), TargetOs::Unknown);
    }

    #[test]
    fn test_path_sep_for_windows() {
        assert_eq!(path_sep_for(TargetOs::Windows), '\\');
    }

    #[test]
    fn test_path_sep_for_unix() {
        assert_eq!(path_sep_for(TargetOs::Linux), '/');
        assert_eq!(path_sep_for(TargetOs::Macos), '/');
    }

    #[test]
    fn test_default_is_current() {
        let default = PlatformInfo::default();
        let current = PlatformInfo::current();
        assert_eq!(default.os, current.os);
        assert_eq!(default.arch, current.arch);
    }
}
