//! FEAT-23 Stage 2: Runtime bus protocol assertion library.
//!
//! Menghasilkan kode SystemVerilog Assertion (SVA) untuk pemeriksaan protokol
//! bus SECARA RUNTIME (saat simulasi), bukan post-sim VCD decode.
//! Assertion ini bisa di-embed ke testbench atau file verilog untuk
//! pemeriksaan cycle-by-cycle selama simulasi berjalan.
//! 1 file = 1 tanggung jawab: hanya generation kode SVA assertion bus protocol.

use serde::{Deserialize, Serialize};

/// Satu assertion bus protocol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BusAssertion {
    /// Protokol bus (apb | axi4lite | ahb)
    pub protocol: String,
    /// Nama aturan (mis. "apb_pready_required", "axi4lite_awvalid_before_wdata")
    pub rule_name: String,
    /// Kode SVA assertion
    pub sva_code: String,
    /// Tingkat keparahan (error | warning | info)
    pub severity: String,
    /// Deskripsi aturan dalam bahasa manusia
    pub description: String,
}

/// Koleksi assertion bus protocol.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BusAssertionLibrary {
    assertions: Vec<BusAssertion>,
}

impl BusAssertionLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    /// Tambah assertion.
    pub fn add(&mut self, assertion: BusAssertion) {
        self.assertions.push(assertion);
    }

    /// Dapatkan semua assertion untuk protokol tertentu.
    pub fn for_protocol(&self, protocol: &str) -> Vec<&BusAssertion> {
        self.assertions
            .iter()
            .filter(|a| a.protocol == protocol)
            .collect()
    }

    /// Dapatkan semua assertion.
    pub fn all(&self) -> &[BusAssertion] {
        &self.assertions
    }

    /// Jumlah assertion.
    pub fn len(&self) -> usize {
        self.assertions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.assertions.is_empty()
    }
}

/// Library assertion bawaan untuk protokol APB, AXI4-Lite, AHB.
pub fn default_library() -> BusAssertionLibrary {
    let mut lib = BusAssertionLibrary::new();

    // ── APB assertions ──
    lib.add(BusAssertion {
        protocol: "apb".to_string(),
        rule_name: "apb_penable_requires_psel".to_string(),
        sva_code: r"assert property (@(posedge pclk) disable iff (presetn)
    $rose(penable) |-> psel)" .to_string(),
        severity: "error".to_string(),
        description: "PENABLE harus diakibatkan oleh PSEL (APB3 §4.2)".to_string(),
    });
    lib.add(BusAssertion {
        protocol: "apb".to_string(),
        rule_name: "apb_pready_required_before_access_done".to_string(),
        sva_code: r"assert property (@(posedge pclk) disable iff (presetn)
    psel & penable & ~pready |=> ##[1:$] pready)" .to_string(),
        severity: "error".to_string(),
        description: "PREADY harus naik sebelum access selesai (APB3 §4.3)".to_string(),
    });
    lib.add(BusAssertion {
        protocol: "apb".to_string(),
        rule_name: "apb_pwrite_stable_during_access".to_string(),
        sva_code: r"assert property (@(posedge pclk) disable iff (presetn)
    psel & penable |=> $stable(pwrite))" .to_string(),
        severity: "warning".to_string(),
        description: "PWRITE harus stabil selama access (APB3 §4.2)".to_string(),
    });
    lib.add(BusAssertion {
        protocol: "apb".to_string(),
        rule_name: "apb_paddr_stable_during_access".to_string(),
        sva_code: r"assert property (@(posedge pclk) disable iff (presetn)
    psel & penable |=> $stable(paddr))" .to_string(),
        severity: "warning".to_string(),
        description: "PADDR harus stabil selama access (APB3 §4.2)".to_string(),
    });

    // ── AXI4-Lite assertions ──
    lib.add(BusAssertion {
        protocol: "axi4lite".to_string(),
        rule_name: "axi4lite_awvalid_before_awready".to_string(),
        sva_code: r"assert property (@(posedge aclk) disable iff (aresetn)
    awvalid |=> ##[1:$] awready)" .to_string(),
        severity: "error".to_string(),
        description: "AWREADY harus naik setelah AWVALID (AXI4-Lite §A4)".to_string(),
    });
    lib.add(BusAssertion {
        protocol: "axi4lite".to_string(),
        rule_name: "axi4lite_wvalid_before_wready".to_string(),
        sva_code: r"assert property (@(posedge aclk) disable iff (aresetn)
    wvalid |=> ##[1:$] wready)" .to_string(),
        severity: "error".to_string(),
        description: "WREADY harus naik setelah WVALID (AXI4-Lite §A4)".to_string(),
    });
    lib.add(BusAssertion {
        protocol: "axi4lite".to_string(),
        rule_name: "axi4lite_bvalid_before_bready".to_string(),
        sva_code: r"assert property (@(posedge aclk) disable iff (aresetn)
    bvalid |=> ##[1:$] bready)" .to_string(),
        severity: "error".to_string(),
        description: "BREADY harus naik setelah BVALID (AXI4-Lite §A4)".to_string(),
    });
    lib.add(BusAssertion {
        protocol: "axi4lite".to_string(),
        rule_name: "axi4lite_arvalid_before_arready".to_string(),
        sva_code: r"assert property (@(posedge aclk) disable iff (aresetn)
    arvalid |=> ##[1:$] arready)" .to_string(),
        severity: "error".to_string(),
        description: "ARREADY harus naik setelah ARVALID (AXI4-Lite §A4)".to_string(),
    });
    lib.add(BusAssertion {
        protocol: "axi4lite".to_string(),
        rule_name: "axi4lite_rvalid_before_rready".to_string(),
        sva_code: r"assert property (@(posedge aclk) disable iff (aresetn)
    rvalid |=> ##[1:$] rready)" .to_string(),
        severity: "error".to_string(),
        description: "RREADY harus naik setelah RVALID (AXI4-Lite §A4)".to_string(),
    });
    lib.add(BusAssertion {
        protocol: "axi4lite".to_string(),
        rule_name: "axi4lite_awaddr_stable_until_awready".to_string(),
        sva_code: r"assert property (@(posedge aclk) disable iff (aresetn)
    awvalid & ~awready |=> $stable(awaddr))" .to_string(),
        severity: "warning".to_string(),
        description: "AWADDR harus stabil sampai AWREADY naik (AXI4-Lite §A4)".to_string(),
    });

    // ── AHB assertions ──
    lib.add(BusAssertion {
        protocol: "ahb".to_string(),
        rule_name: "ahb_htrans_seq_requires_prev_nonseq".to_string(),
        sva_code: r"assert property (@(posedge hclk) disable iff (hresetn)
    htrans == 2'b10 |-> $past(htrans, 1) == 2'b01 || $past(htrans, 1) == 2'b10)" .to_string(),
        severity: "error".to_string(),
        description: "HTRANS=SEQ harus didahului NONSEQ atau SEQ (AHB3 §3.3)".to_string(),
    });
    lib.add(BusAssertion {
        protocol: "ahb".to_string(),
        rule_name: "ahb_hready_required_before_transfer_done".to_string(),
        sva_code: r"assert property (@(posedge hclk) disable iff (hresetn)
    htrans != 2'b00 & ~hready |=> ##[1:$] hready)" .to_string(),
        severity: "error".to_string(),
        description: "HREADY harus naik sebelum transfer selesai (AHB3 §3.3)".to_string(),
    });
    lib.add(BusAssertion {
        protocol: "ahb".to_string(),
        rule_name: "ahb_hwrite_stable_during_burst".to_string(),
        sva_code: r"assert property (@(posedge hclk) disable iff (hresetn)
    htrans != 2'b00 |=> $stable(hwrite))" .to_string(),
        severity: "warning".to_string(),
        description: "HWRITE harus stabil selama burst (AHB3 §3.3)".to_string(),
    });
    lib.add(BusAssertion {
        protocol: "ahb".to_string(),
        rule_name: "ahb_haddr_stable_until_hready".to_string(),
        sva_code: r"assert property (@(posedge hclk) disable iff (hresetn)
    htrans != 2'b00 & ~hready |=> $stable(haddr))" .to_string(),
        severity: "warning".to_string(),
        description: "HADDR harus stabil sampai HREADY naik (AHB3 §3.3)".to_string(),
    });

    lib
}

/// Emit semua assertion sebagai blok SVA dalam satu string.
pub fn emit_all_sva(lib: &BusAssertionLibrary) -> String {
    let mut out = String::new();
    out.push_str("// ── FEAT-23: Runtime Bus Protocol Assertions ──\n");
    for a in lib.all() {
        out.push_str(&format!(
            "// Rule: {} ({})\n// {}\n{}\n\n",
            a.rule_name, a.protocol, a.description, a.sva_code
        ));
    }
    out
}

/// Emit assertion untuk protokol tertentu sebagai blok SVA.
pub fn emit_protocol_sva(lib: &BusAssertionLibrary, protocol: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("// ── FEAT-23: {} Runtime Assertions ──\n", protocol.to_uppercase()));
    for a in lib.for_protocol(protocol) {
        out.push_str(&format!(
            "// Rule: {}\n// {}\n{}\n\n",
            a.rule_name, a.description, a.sva_code
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_library_has_all_protocols() {
        let lib = default_library();
        assert!(!lib.is_empty());
        assert!(lib.len() >= 12);

        let apb = lib.for_protocol("apb");
        assert!(apb.len() >= 4);

        let axi = lib.for_protocol("axi4lite");
        assert!(axi.len() >= 6);

        let ahb = lib.for_protocol("ahb");
        assert!(ahb.len() >= 4);
    }

    #[test]
    fn default_library_apb_rules() {
        let lib = default_library();
        let rule_names: Vec<&str> = lib
            .for_protocol("apb")
            .iter()
            .map(|a| a.rule_name.as_str())
            .collect();
        assert!(rule_names.contains(&"apb_penable_requires_psel"));
        assert!(rule_names.contains(&"apb_pready_required_before_access_done"));
        assert!(rule_names.contains(&"apb_pwrite_stable_during_access"));
        assert!(rule_names.contains(&"apb_paddr_stable_during_access"));
    }

    #[test]
    fn default_library_axi4lite_rules() {
        let lib = default_library();
        let rule_names: Vec<&str> = lib
            .for_protocol("axi4lite")
            .iter()
            .map(|a| a.rule_name.as_str())
            .collect();
        assert!(rule_names.contains(&"axi4lite_awvalid_before_awready"));
        assert!(rule_names.contains(&"axi4lite_wvalid_before_wready"));
        assert!(rule_names.contains(&"axi4lite_bvalid_before_bready"));
        assert!(rule_names.contains(&"axi4lite_arvalid_before_arready"));
        assert!(rule_names.contains(&"axi4lite_rvalid_before_rready"));
        assert!(rule_names.contains(&"axi4lite_awaddr_stable_until_awready"));
    }

    #[test]
    fn default_library_ahb_rules() {
        let lib = default_library();
        let rule_names: Vec<&str> = lib
            .for_protocol("ahb")
            .iter()
            .map(|a| a.rule_name.as_str())
            .collect();
        assert!(rule_names.contains(&"ahb_htrans_seq_requires_prev_nonseq"));
        assert!(rule_names.contains(&"ahb_hready_required_before_transfer_done"));
        assert!(rule_names.contains(&"ahb_hwrite_stable_during_burst"));
        assert!(rule_names.contains(&"ahb_haddr_stable_until_hready"));
    }

    #[test]
    fn emit_all_sva_contains_rules() {
        let lib = default_library();
        let sva = emit_all_sva(&lib);
        assert!(sva.contains("apb_penable_requires_psel"));
        assert!(sva.contains("axi4lite_awvalid_before_awready"));
        assert!(sva.contains("ahb_htrans_seq_requires_prev_nonseq"));
        assert!(sva.contains("FEAT-23"));
    }

    #[test]
    fn emit_protocol_sva_filters() {
        let lib = default_library();
        let apb_sva = emit_protocol_sva(&lib, "apb");
        assert!(apb_sva.contains("APB"));
        assert!(!apb_sva.contains("axi4lite"));
        assert!(!apb_sva.contains("ahb"));

        let axi_sva = emit_protocol_sva(&lib, "axi4lite");
        assert!(axi_sva.contains("AXI4LITE"));
        assert!(!axi_sva.contains("apb_"));
        assert!(!axi_sva.contains("ahb_"));
    }

    #[test]
    fn assertion_has_required_fields() {
        let lib = default_library();
        for a in lib.all() {
            assert!(!a.protocol.is_empty());
            assert!(!a.rule_name.is_empty());
            assert!(!a.sva_code.is_empty());
            assert!(!a.severity.is_empty());
            assert!(!a.description.is_empty());
            assert!(matches!(a.severity.as_str(), "error" | "warning" | "info"));
        }
    }
}
