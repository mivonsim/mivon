//! FEAT-24: Standard VIP for industry protocols (USB, PCIe, DDR, Ethernet).
//! Stage 3 (lanjut dari stage 1 stub + stage 2 DDR).
//!
//! Model library VIP standar untuk protokol industri (stage 1: model data
//! + register/enable). Stage 2 menambahkan DDR command checker (decode
//! JEDEC 3-bit + validasi reserved). Stage 3 menambahkan decoder + validasi
//! enkoding perintah USB/PCIe/Ethernet (decode + flag reserved/invalid).
//! Belum di-wire ke engine/VIP runtime — alat diagnostik mandiri.
//! Sisa open: runtime VIP assertion library (di-drive saat simulasi, bukan
//! post-sim decode).

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

/// Pelanggaran protokol generik pada deretan token (dipakai USB/PCIe/Ethernet).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VipViolation {
    /// Indeks dalam deretan token.
    pub index: usize,
    /// Enkoding yang melanggar (lebar tergantung protokol).
    pub code: u32,
    /// Jenis pelanggaran, mis. `USB-INVALID-PID` / `PCIE-RESERVED-TYPE` /
    /// `ETH-INVALID-ETYPE`.
    pub kind: &'static str,
}

// ───────────────────────── USB (FEAT-24 stage 3) ─────────────────────────
/// Decode USB Packet ID (PID) 4-bit ke nama menurut USB 2.0 (field PID 4-bit;
/// encoding asli menggunakan komplemen bitwise 8-bit, di sini hanya field
/// 4-bit yang dipakai). Dari 16 nilai, `0b1000` adalah SPLIT, `0b1100` adalah
/// PRE/ERR (low/full-speed PRE, high-speed ERR), `0b0100` adalah PING, dan
/// `0b0000` adalah RESERVED (tidak dipakai). Mengembalikan `None` untuk
/// reserved. Bit di luar 4 LSB diabaikan.
pub fn decode_usb_pid(pid4: u8) -> Option<&'static str> {
    match pid4 & 0b1111 {
        0b0001 => Some("OUT"),
        0b1001 => Some("IN"),
        0b0101 => Some("SOF"),
        0b1101 => Some("SETUP"),
        0b0011 => Some("DATA0"),
        0b1011 => Some("DATA1"),
        0b0111 => Some("DATA2"),
        0b1111 => Some("MDATA"),
        0b0010 => Some("ACK"),
        0b1010 => Some("NAK"),
        0b1110 => Some("STALL"),
        0b0110 => Some("NYET"),
        0b0000 => None, // RESERVED (USB 2.0)
        0b0100 => Some("PING"),
        0b1000 => Some("SPLIT"),
        0b1100 => Some("PRE/ERR"),
        _ => None, // impossible setelah mask 4-bit; dianggap reserved
    }
}

/// Validasi deretan PID USB: flag `0b0000` (reserved) sebagai
/// `USB-INVALID-PID`.
pub fn check_usb_pids(pids: &[u8]) -> Vec<VipViolation> {
    let mut out = Vec::new();
    for (i, &p) in pids.iter().enumerate() {
        if decode_usb_pid(p).is_none() {
            out.push(VipViolation {
                index: i,
                code: (p & 0b1111) as u32,
                kind: "USB-INVALID-PID",
            });
        }
    }
    out
}

// ───────────────────────── PCIe (FEAT-24 stage 3) ────────────────────────
/// Decode PCIe TLP Type (5-bit, `type & 0x1F`) ke nama untuk subset TLP
/// umum yang didokumentasikan. Bit ke-4 (0x10) menandakan header 4DW bagi
/// Memory/IO/Cfg/Completion; untuk Msg (0x10/0x11) bit tersebut adalah
/// bagian dari encoding Msg. Nilai di luar subset dikenal dianggap
/// reserved/unsupported -> `None`.
///
/// Ini STUB FEAT-24: bukan decoder TLP lengkap (tidak mem-parsing Fmt,
/// length, address, atau atribut). Hanya mengklasifikasi field Type.
pub fn decode_pcie_tlp_type(t: u8) -> Option<&'static str> {
    match t & 0b11111 {
        // 3DW
        0x00 => Some("MRd"),
        0x01 => Some("MRdLk"),
        0x02 => Some("IORd"),
        0x03 => Some("IOWr"),
        0x04 => Some("CfgRd0"),
        0x05 => Some("CfgWr0"),
        0x06 => Some("CfgRd1"),
        0x07 => Some("CfgWr1"),
        0x0A => Some("Cpl"),
        0x0B => Some("CplD"),
        0x0E => Some("CplLk"),
        0x0F => Some("CplLkD"),
        0x10 => Some("Msg"),
        0x11 => Some("MsgD"),
        // 4DW (bit 0x10 set pada Memory/IO/Cfg/Completion)
        0x20 => Some("MRd"),
        0x21 => Some("MRdLk"),
        0x22 => Some("IORd"),
        0x23 => Some("IOWr"),
        0x24 => Some("CfgRd0"),
        0x25 => Some("CfgWr0"),
        0x26 => Some("CfgRd1"),
        0x27 => Some("CfgWr1"),
        0x2A => Some("Cpl"),
        0x2B => Some("CplD"),
        0x2E => Some("CplLk"),
        0x2F => Some("CplLkD"),
        _ => None, // reserved/unsupported (mis. 0x08,0x09,0x0C,0x0D,0x12..0x1F,
                   // 0x28,0x29,0x2C,0x2D,0x30..0x3F)
    }
}

/// Validasi deretan TLP Type PCIe: flag nilai reserved/unsupported sebagai
/// `PCIE-RESERVED-TYPE`.
pub fn check_pcie_tlp_types(types: &[u8]) -> Vec<VipViolation> {
    let mut out = Vec::new();
    for (i, &t) in types.iter().enumerate() {
        if decode_pcie_tlp_type(t).is_none() {
            out.push(VipViolation {
                index: i,
                code: (t & 0b11111) as u32,
                kind: "PCIE-RESERVED-TYPE",
            });
        }
    }
    out
}

// ──────────────────────── Ethernet (FEAT-24 stage 3) ─────────────────────
/// Decode EtherType 16-bit ke nama protokol umum.
/// `0x0000` dan `0xFFFF` BUKAN EtherType valid (per IEEE 802.3) -> `None`
/// (invalid). Nilai lain yang tidak dikenal juga `None` namun BUKAN error
/// (hanya tidak di-decode).
pub fn decode_ether_type(et: u16) -> Option<&'static str> {
    match et {
        0x0800 => Some("IPv4"),
        0x0806 => Some("ARP"),
        0x86DD => Some("IPv6"),
        0x8100 => Some("VLAN"),
        0x8847 => Some("MPLS"),
        0x88CC => Some("LLDP"),
        0x0000 | 0xFFFF => None, // invalid ethertype
        _ => None,               // unknown (bukan error)
    }
}

/// Validasi deretan EtherType: flag hanya `0x0000`/`0xFFFF` sebagai
/// `ETH-INVALID-ETYPE` (nilai tidak valid secara spesifikasi).
pub fn check_ether_types(ets: &[u16]) -> Vec<VipViolation> {
    let mut out = Vec::new();
    for (i, &e) in ets.iter().enumerate() {
        if decode_ether_type(e).is_none() && (e == 0x0000 || e == 0xFFFF) {
            out.push(VipViolation {
                index: i,
                code: e as u32,
                kind: "ETH-INVALID-ETYPE",
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

    // ── USB (FEAT-24 stage 3) ──
    #[test]
    fn usb_pid_decode_known() {
        assert_eq!(decode_usb_pid(0b0001), Some("OUT"));
        assert_eq!(decode_usb_pid(0b1001), Some("IN"));
        assert_eq!(decode_usb_pid(0b0101), Some("SOF"));
        assert_eq!(decode_usb_pid(0b1101), Some("SETUP"));
        assert_eq!(decode_usb_pid(0b0011), Some("DATA0"));
        assert_eq!(decode_usb_pid(0b1011), Some("DATA1"));
        assert_eq!(decode_usb_pid(0b0010), Some("ACK"));
        assert_eq!(decode_usb_pid(0b1010), Some("NAK"));
        assert_eq!(decode_usb_pid(0b1110), Some("STALL"));
        assert_eq!(decode_usb_pid(0b0110), Some("NYET"));
        // 0b0000 reserved; 0b0100 PING; 0b1000 SPLIT; 0b1100 PRE/ERR.
        assert_eq!(decode_usb_pid(0b0100), Some("PING"));
        assert_eq!(decode_usb_pid(0b1000), Some("SPLIT"));
        assert_eq!(decode_usb_pid(0b1100), Some("PRE/ERR"));
        // bit di luar 4 LSB diabaikan.
        assert_eq!(decode_usb_pid(0b0001 | 0b10000), Some("OUT"));
    }

    #[test]
    fn usb_pid_reserved_is_none() {
        // Hanya 0b0000 yang reserved (USB 2.0).
        assert_eq!(decode_usb_pid(0b0000), None);
        assert_eq!(decode_usb_pid(0b0000 | 0b10000), None);
    }

    #[test]
    fn usb_check_flags_reserved_only() {
        // 0b0000 (reserved) muncul di indeks 1 dan 3; sisanya valid.
        let pids = [0b0001, 0b0000, 0b0011, 0b0000 | 0b10000];
        let v = check_usb_pids(&pids);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].index, 1);
        assert_eq!(v[0].code, 0b0000);
        assert_eq!(v[0].kind, "USB-INVALID-PID");
        assert_eq!(v[1].index, 3);
        assert_eq!(v[1].code, 0b0000);
        assert_eq!(v[1].kind, "USB-INVALID-PID");
    }

    // ── PCIe (FEAT-24 stage 3) ──
    #[test]
    fn pcie_tlp_decode_known() {
        assert_eq!(decode_pcie_tlp_type(0x00), Some("MRd"));
        assert_eq!(decode_pcie_tlp_type(0x0B), Some("CplD"));
        assert_eq!(decode_pcie_tlp_type(0x10), Some("Msg"));
        assert_eq!(decode_pcie_tlp_type(0x11), Some("MsgD"));
        // 4DW variants decode ke nama sama.
        assert_eq!(decode_pcie_tlp_type(0x20), Some("MRd"));
        assert_eq!(decode_pcie_tlp_type(0x2B), Some("CplD"));
        // bit di luar 5 LSB diabaikan.
        assert_eq!(decode_pcie_tlp_type(0x00 | 0b100000), Some("MRd"));
    }

    #[test]
    fn pcie_tlp_reserved_is_none() {
        assert_eq!(decode_pcie_tlp_type(0x08), None);
        assert_eq!(decode_pcie_tlp_type(0x09), None);
        assert_eq!(decode_pcie_tlp_type(0x0C), None);
        assert_eq!(decode_pcie_tlp_type(0x12), None);
        assert_eq!(decode_pcie_tlp_type(0x2C), None);
    }

    #[test]
    fn pcie_check_flags_reserved_only() {
        // 0x2C -> field 5-bit 0x0C (reserved). code menyimpan nilai termask.
        let types = [0x00, 0x08, 0x0B, 0x2C, 0x10];
        let v = check_pcie_tlp_types(&types);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].index, 1);
        assert_eq!(v[0].code, 0x08);
        assert_eq!(v[0].kind, "PCIE-RESERVED-TYPE");
        assert_eq!(v[1].index, 3);
        assert_eq!(v[1].code, 0x0C);
        assert_eq!(v[1].kind, "PCIE-RESERVED-TYPE");
    }

    // ── Ethernet (FEAT-24 stage 3) ──
    #[test]
    fn ether_type_decode_known() {
        assert_eq!(decode_ether_type(0x0800), Some("IPv4"));
        assert_eq!(decode_ether_type(0x0806), Some("ARP"));
        assert_eq!(decode_ether_type(0x86DD), Some("IPv6"));
        assert_eq!(decode_ether_type(0x8100), Some("VLAN"));
        // unknown (bukan invalid) -> None tapi tidak di-flag.
        assert_eq!(decode_ether_type(0x0801), None);
    }

    #[test]
    fn ether_type_invalid_flagged() {
        // 0x0000 dan 0xFFFF adalah ethertype tidak valid -> None + di-flag.
        assert_eq!(decode_ether_type(0x0000), None);
        assert_eq!(decode_ether_type(0xFFFF), None);
        let ets = [0x0800u16, 0x0000, 0x86DD, 0xFFFF];
        let v = check_ether_types(&ets);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].index, 1);
        assert_eq!(v[0].code, 0x0000);
        assert_eq!(v[0].kind, "ETH-INVALID-ETYPE");
        assert_eq!(v[1].index, 3);
        assert_eq!(v[1].code, 0xFFFF);
        assert_eq!(v[1].kind, "ETH-INVALID-ETYPE");
    }

    #[test]
    fn ether_unknown_not_flagged() {
        // ethertype tidak dikenal (valid secara spesifikasi) tidak di-flag.
        let ets = [0x0801u16, 0x8808, 0x0800];
        assert!(check_ether_types(&ets).is_empty());
    }
}
