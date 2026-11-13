//! FEAT-24: Standard VIP for industry protocols (USB, PCIe, DDR, Ethernet).
//! Stage 1 stub (Critical, open).
//!
//! Model library VIP standar untuk protokol industri. Implementasi checker
//! VIP nyata (USB/PCIe/DDR/Ethernet) BELUM ada — stage 1 adalah model data
//! + tes yang mendokumentasikan gap. Belum di-wire ke engine/VIP runtime.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Protokol industri yang didukung (placeholder).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VipProtocol {
    Usb,
    Pcie,
    Ddr,
    Ethernet,
    Placeholder,
}

/// Status satu instansi VIP.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VipInstanceStatus {
    pub protocol: VipProtocol,
    pub enabled: bool,
    pub transactions_seen: u64,
}

/// Library VIP standar (in-memory, stage 1).
#[derive(Debug, Clone, Default)]
pub struct StandardVipLibrary {
    instances: Arc<Mutex<HashMap<String, VipInstanceStatus>>>,
}

impl StandardVipLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, name: &str, protocol: VipProtocol) -> bool {
        let mut inst = self.instances.lock().unwrap();
        if inst.contains_key(name) {
            return false;
        }
        inst.insert(
            name.to_string(),
            VipInstanceStatus {
                protocol,
                enabled: false,
                transactions_seen: 0,
            },
        );
        true
    }

    pub fn enable(&self, name: &str) -> bool {
        let mut inst = self.instances.lock().unwrap();
        match inst.get_mut(name) {
            Some(s) => {
                s.enabled = true;
                true
            }
            None => false,
        }
    }

    pub fn record_transaction(&self, name: &str) -> bool {
        let mut inst = self.instances.lock().unwrap();
        match inst.get_mut(name) {
            Some(s) => {
                s.transactions_seen += 1;
                true
            }
            None => false,
        }
    }

    pub fn count(&self) -> usize {
        self.instances.lock().unwrap().len()
    }

    pub fn is_enabled(&self, name: &str) -> bool {
        self.instances
            .lock()
            .unwrap()
            .get(name)
            .map(|s| s.enabled)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_enable_vip() {
        let lib = StandardVipLibrary::new();
        assert!(lib.register("usb0", VipProtocol::Usb));
        assert!(!lib.register("usb0", VipProtocol::Pcie)); // duplikat
        assert!(!lib.is_enabled("usb0"));
        assert!(lib.enable("usb0"));
        assert!(lib.is_enabled("usb0"));
        assert!(lib.record_transaction("usb0"));
        assert_eq!(lib.count(), 1);
    }

    #[test]
    fn unknown_vip_rejected() {
        let lib = StandardVipLibrary::new();
        assert!(!lib.enable("nope"));
        assert!(!lib.record_transaction("nope"));
        assert_eq!(lib.count(), 0);
    }

    #[test]
    fn placeholder_protocol_roundtrip() {
        let p = VipProtocol::Placeholder;
        assert!(matches!(p, VipProtocol::Placeholder));
    }

    #[test]
    fn empty_library() {
        let lib = StandardVipLibrary::new();
        assert_eq!(lib.count(), 0);
        assert!(!lib.is_enabled("any"));
    }
}
