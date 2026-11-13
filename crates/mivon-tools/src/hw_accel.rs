//! FEAT-20: Hardware acceleration co-simulation foundation.
//! Stage 1 stub (High, open).
//!
//! Model akselerasi perangkat keras (FPGA/ASIC co-simulation) untuk
//! menjalankan bagian desain di accelerator keras. Integrasi accelerator
//! nyata (PCIe, DPI, Xilinx/Intel FPGA) BELUM ada — stage 1 adalah model
//! data + tes yang mendokumentasikan gap.
//! Belum di-wire ke engine/accelerator.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Akselerator keras yang tersedia.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HwAccelTarget {
    Fpga,
    Asic,
    Gpu,
    Placeholder,
}

/// Status satu akselerator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HwAccelStatus {
    pub target: HwAccelTarget,
    pub bitstream_loaded: bool,
    pub pcie_connected: bool,
    pub clock_mhz: Option<u32>,
}

/// Manajer akselerasi keras (in-memory, stage 1).
#[derive(Debug, Clone, Default)]
pub struct HwAccelManager {
    accelerators: Arc<Mutex<HashMap<String, HwAccelStatus>>>,
}

impl HwAccelManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, name: &str, target: HwAccelTarget) -> bool {
        let mut acc = self.accelerators.lock().unwrap();
        if acc.contains_key(name) {
            return false;
        }
        acc.insert(
            name.to_string(),
            HwAccelStatus {
                target,
                bitstream_loaded: false,
                pcie_connected: false,
                clock_mhz: None,
            },
        );
        true
    }

    pub fn set_bitstream(&self, name: &str, loaded: bool) -> bool {
        let mut acc = self.accelerators.lock().unwrap();
        match acc.get_mut(name) {
            Some(s) => {
                s.bitstream_loaded = loaded;
                true
            }
            None => false,
        }
    }

    pub fn set_pcie(&self, name: &str, connected: bool) -> bool {
        let mut acc = self.accelerators.lock().unwrap();
        match acc.get_mut(name) {
            Some(s) => {
                s.pcie_connected = connected;
                true
            }
            None => false,
        }
    }

    pub fn set_clock(&self, name: &str, mhz: u32) -> bool {
        let mut acc = self.accelerators.lock().unwrap();
        match acc.get_mut(name) {
            Some(s) => {
                s.clock_mhz = Some(mhz);
                true
            }
            None => false,
        }
    }

    pub fn count(&self) -> usize {
        self.accelerators.lock().unwrap().len()
    }

    pub fn is_ready(&self, name: &str) -> bool {
        self.accelerators
            .lock()
            .unwrap()
            .get(name)
            .map(|s| s.bitstream_loaded && s.pcie_connected && s.clock_mhz.is_some())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_configure_accelerator() {
        let mgr = HwAccelManager::new();
        assert!(mgr.register("fpga1", HwAccelTarget::Fpga));
        assert!(!mgr.register("fpga1", HwAccelTarget::Asic)); // duplikat
        assert!(!mgr.is_ready("fpga1"));
        assert!(mgr.set_bitstream("fpga1", true));
        assert!(mgr.set_pcie("fpga1", true));
        assert!(mgr.set_clock("fpga1", 250));
        assert!(mgr.is_ready("fpga1"));
        assert_eq!(mgr.count(), 1);
    }

    #[test]
    fn unknown_accelerator_rejected() {
        let mgr = HwAccelManager::new();
        assert!(!mgr.set_bitstream("nope", true));
        assert!(!mgr.set_pcie("nope", true));
        assert!(!mgr.set_clock("nope", 1));
        assert_eq!(mgr.count(), 0);
    }

    #[test]
    fn placeholder_target_roundtrip() {
        let t = HwAccelTarget::Placeholder;
        assert!(matches!(t, HwAccelTarget::Placeholder));
    }

    #[test]
    fn empty_manager() {
        let mgr = HwAccelManager::new();
        assert_eq!(mgr.count(), 0);
        assert!(!mgr.is_ready("any"));
    }
}
