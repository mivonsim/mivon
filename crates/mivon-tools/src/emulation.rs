//! FEAT-19: Emulation/prototyping integration foundation.
//! Stage 1 stub (High, open).
//!
//! Model target emulation/prototyping (FPGA/emulator) untuk menjalankan
//! desain SV di perangkat keras prototipe. Integrasi emulator nyata
//! (JTAG/USB, bitstream, pmod) BELUM ada — stage 1 adalah model data +
//! tes yang mendokumentasikan gap.
//! Belum di-wire ke emulator/server.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Target emulasi/prototyping yang didukung.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EmuTarget {
    FpgaXilinx,
    FpgaAltera,
    EmulatorQemu,
    Placeholder,
}

/// Status satu target emulasi.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmuTargetStatus {
    pub target: EmuTarget,
    pub bitstream_loaded: bool,
    pub jtag_connected: bool,
    pub clock_hz: Option<u64>,
}

/// Manajer emulasi/prototyping (in-memory, stage 1).
#[derive(Debug, Clone, Default)]
pub struct EmuManager {
    targets: Arc<Mutex<HashMap<String, EmuTargetStatus>>>,
}

impl EmuManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Daftarkan target emulasi baru.
    pub fn register_target(&self, name: &str, target: EmuTarget) -> bool {
        let mut targets = self.targets.lock().unwrap();
        if targets.contains_key(name) {
            return false;
        }
        targets.insert(
            name.to_string(),
            EmuTargetStatus {
                target,
                bitstream_loaded: false,
                jtag_connected: false,
                clock_hz: None,
            },
        );
        true
    }

    pub fn set_bitstream(&self, name: &str, loaded: bool) -> bool {
        let mut targets = self.targets.lock().unwrap();
        match targets.get_mut(name) {
            Some(s) => {
                s.bitstream_loaded = loaded;
                true
            }
            None => false,
        }
    }

    pub fn set_jtag(&self, name: &str, connected: bool) -> bool {
        let mut targets = self.targets.lock().unwrap();
        match targets.get_mut(name) {
            Some(s) => {
                s.jtag_connected = connected;
                true
            }
            None => false,
        }
    }

    pub fn set_clock(&self, name: &str, hz: u64) -> bool {
        let mut targets = self.targets.lock().unwrap();
        match targets.get_mut(name) {
            Some(s) => {
                s.clock_hz = Some(hz);
                true
            }
            None => false,
        }
    }

    pub fn target_count(&self) -> usize {
        self.targets.lock().unwrap().len()
    }

    pub fn is_ready(&self, name: &str) -> bool {
        self.targets
            .lock()
            .unwrap()
            .get(name)
            .map(|s| s.bitstream_loaded && s.jtag_connected && s.clock_hz.is_some())
            .unwrap_or(false)
    }

    /// Daftar nama semua target terdaftar.
    pub fn list_targets(&self) -> Vec<String> {
        self.targets.lock().unwrap().keys().cloned().collect()
    }

    /// Ambil klon status target menurut nama.
    pub fn get_target(&self, name: &str) -> Option<EmuTargetStatus> {
        self.targets.lock().unwrap().get(name).cloned()
    }

    /// Hapus target menurut nama.
    pub fn remove_target(&self, name: &str) -> bool {
        self.targets.lock().unwrap().remove(name).is_some()
    }

    /// Hapus semua target.
    pub fn clear_targets(&self) {
        self.targets.lock().unwrap().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_configure_target() {
        let mgr = EmuManager::new();
        assert!(mgr.register_target("fpga1", EmuTarget::FpgaXilinx));
        assert!(!mgr.register_target("fpga1", EmuTarget::FpgaAltera)); // duplikat
        assert!(!mgr.is_ready("fpga1")); // belum siap
        assert!(mgr.set_bitstream("fpga1", true));
        assert!(mgr.set_jtag("fpga1", true));
        assert!(mgr.set_clock("fpga1", 100_000_000));
        assert!(mgr.is_ready("fpga1"));
        assert_eq!(mgr.target_count(), 1);
    }

    #[test]
    fn unknown_target_rejected() {
        let mgr = EmuManager::new();
        assert!(!mgr.set_bitstream("nope", true));
        assert!(!mgr.set_jtag("nope", true));
        assert!(!mgr.set_clock("nope", 1));
        assert_eq!(mgr.target_count(), 0);
    }

    #[test]
    fn placeholder_target_roundtrip() {
        let t = EmuTarget::Placeholder;
        assert!(matches!(t, EmuTarget::Placeholder));
    }

    #[test]
    fn empty_manager() {
        let mgr = EmuManager::new();
        assert!(mgr.target_count() == 0);
        assert!(!mgr.is_ready("any"));
    }

    #[test]
    fn list_targets_tracks_all() {
        let mgr = EmuManager::new();
        assert!(mgr.list_targets().is_empty());
        mgr.register_target("fpga1", EmuTarget::FpgaXilinx);
        mgr.register_target("emu1", EmuTarget::EmulatorQemu);
        let mut list = mgr.list_targets();
        list.sort();
        assert_eq!(
            list,
            vec!["emu1".to_string(), "fpga1".to_string()]
        );
        assert_eq!(mgr.target_count(), 2);
    }

    #[test]
    fn get_and_remove_target() {
        let mgr = EmuManager::new();
        mgr.register_target("fpga1", EmuTarget::FpgaXilinx);
        let t = mgr.get_target("fpga1").unwrap();
        assert!(matches!(t.target, EmuTarget::FpgaXilinx));
        assert!(!t.bitstream_loaded);
        assert!(mgr.get_target("nope").is_none());
        assert!(mgr.remove_target("fpga1"));
        assert!(mgr.get_target("fpga1").is_none());
        assert!(!mgr.remove_target("fpga1"));
        assert_eq!(mgr.target_count(), 0);
    }

    #[test]
    fn clear_targets_empties_manager() {
        let mgr = EmuManager::new();
        mgr.register_target("fpga1", EmuTarget::FpgaXilinx);
        mgr.register_target("emu1", EmuTarget::EmulatorQemu);
        assert_eq!(mgr.target_count(), 2);
        mgr.clear_targets();
        assert!(mgr.list_targets().is_empty());
        assert_eq!(mgr.target_count(), 0);
    }
}
