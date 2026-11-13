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
}
