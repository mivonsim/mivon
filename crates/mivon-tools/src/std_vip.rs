//! FEAT-24: Standard VIP for industry protocols (USB, PCIe, DDR, Ethernet).
//! Stage 2 (lanjut dari stage 1 stub).
//!
//! Model library VIP standar untuk protokol industri (stage 1: model data
//! + register/enable). Stage 2 menambahkan checker VIP DDR tahap 1: decode
//! enkoding perintah bus 3-bit (JEDEC) + validasi enkoding reserved.
//! Belum di-wire ke engine/VIP runtime — alat diagnostik mandiri.
//! Sisa open: checker USB/PCIe/Ethernet + runtime VIP assertion library.

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

/// Decode nilai bus perintah DDR (3-bit, LSB-aligned) ke nama perintah
/// menurut konvensi JEDEC DDR3/4. Mengembalikan `None` untuk enkoding
/// reserved (`110`/`111`) — basis bagi checker VIP DDR.
pub fn decode_ddr_command(cmd3: u8) -> Option<&'static str> {
    match cmd3 & 0b111 {
        0b000 => Some("NOP"),
        0b001 => Some("ACT"),
        0b010 => Some("RD"),
        0b011 => Some("WR"),
        0b100 => Some("PRE"),
        0b101 => Some("REF"),
        // 0b110 / 0b111: reserved (invalid)
        _ => None,
    }
}

/// Satu pelanggaran pada deretan perintah DDR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DdrViolation {
    /// Indeks dalam deretan perintah.
    pub index: usize,
    /// Enkoding 3-bit yang melanggar.
    pub cmd: u8,
    /// Jenis pelanggaran (saat ini: `DDR-INVALID-CMD`).
    pub kind: &'static str,
}

/// Validasi deretan perintah DDR (tiap elemen adalah enkoding 3-bit).
/// Flag setiap enkoding reserved (`110`/`111`) sebagai `DDR-INVALID-CMD`.
///
/// Ini checker VIP DDR tahap 1 (FEAT-24): validasi enkoding perintah.
/// Aturan lebih kaya (ACT tanpa PRE, tumpang-tindih bank) menyusul di
/// stage lanjut.
pub fn check_ddr_commands(cmds: &[u8]) -> Vec<DdrViolation> {
    let mut out = Vec::new();
    for (i, &c) in cmds.iter().enumerate() {
        if decode_ddr_command(c).is_none() {
            out.push(DdrViolation {
                index: i,
                cmd: c & 0b111,
                kind: "DDR-INVALID-CMD",
            });
        }
    }
    out
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

    #[test]
    fn ddr_command_decode_known() {
        // 6 perintah valid menurut JEDEC; bit tinggi diabaikan (LSB-aligned 3-bit).
        assert_eq!(decode_ddr_command(0b000), Some("NOP"));
        assert_eq!(decode_ddr_command(0b001), Some("ACT"));
        assert_eq!(decode_ddr_command(0b010), Some("RD"));
        assert_eq!(decode_ddr_command(0b011), Some("WR"));
        assert_eq!(decode_ddr_command(0b100), Some("PRE"));
        assert_eq!(decode_ddr_command(0b101), Some("REF"));
        // bit di luar 3 LSB tidak memengaruhi hasil.
        assert_eq!(decode_ddr_command(0b001 | 0b1000), Some("ACT"));
    }

    #[test]
    fn ddr_command_decode_reserved_invalid() {
        // Enkoding reserved 110/111 wajib None (basis checker).
        assert_eq!(decode_ddr_command(0b110), None);
        assert_eq!(decode_ddr_command(0b111), None);
        assert_eq!(decode_ddr_command(0b110 | 0b1000), None);
    }

    #[test]
    fn ddr_check_flags_reserved_only() {
        let cmds = [
            0b000, // NOP
            0b010, // RD
            0b110, // reserved -> violation
            0b101, // REF
            0b111, // reserved -> violation
            0b001, // ACT
        ];
        let v = check_ddr_commands(&cmds);
        // Hanya 2 enkoding reserved yang dilaporkan.
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].index, 2);
        assert_eq!(v[0].cmd, 0b110);
        assert_eq!(v[0].kind, "DDR-INVALID-CMD");
        assert_eq!(v[1].index, 4);
        assert_eq!(v[1].cmd, 0b111);
        assert_eq!(v[1].kind, "DDR-INVALID-CMD");
    }

    #[test]
    fn ddr_check_clean() {
        let cmds = [0b000, 0b001, 0b010, 0b011, 0b100, 0b101];
        let v = check_ddr_commands(&cmds);
        assert!(
            v.is_empty(),
            "tidak boleh ada pelanggaran untuk perintah valid"
        );
    }
}
