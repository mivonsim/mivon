//! AST cache — serialisasi `Design` per file dengan bincode (binary).
//!
//! MICD menyimpan AST tiap file yang sudah di-parse. Pada run berikutnya,
//! jika hash konten file sama, AST di-deserialize langsung → parser di-skip
//! untuk file itu. `Symbol` di-serialisasi sebagai string (bukan u32 index)
//! karena index intern bersifat proses-lokal.

use mivon_ast::Design;

/// Versi format serialisasi AST (increment bila skema AST berubah).
/// Versi format serialisasi AST (increment bila skema AST berubah).
/// Update 4: `CovergroupDecl.clocking_event` → `Option<CovergroupClocking>`
/// (edge disimpan penuh utk implicit sampling §19.8).
pub const AST_FORMAT_VERSION: u64 = 4;

/// Serialisasi `Design` → bytes biner (bincode).
pub fn serialize_design(design: &Design) -> Result<Vec<u8>, String> {
    bincode::serialize(design).map_err(|e| format!("MICD AST serialize: {}", e))
}

/// Deserialisasi `Design` dari bytes. `None` bila format tak dikenal/corrupt
/// (pemanggil fallback ke parse ulang).
pub fn deserialize_design(bytes: &[u8]) -> Option<Design> {
    bincode::deserialize(bytes).ok()
}

/// Versi format serialisasi IR hasil elaborasi (increment bila skema IR
/// berubah — memisahkan skema AST dan IR agar perubahan satu tidak
/// meng-invalidasi yang lain).
/// Update 3: `IrCovergroup` + `event_signal`/`event_posedge` (implicit
/// sampling covergroup §19.8).
pub const IR_FORMAT_VERSION: u64 = 3;

/// Serialisasi `IrDesign` → bytes biner (bincode). Dipakai menyimpan hasil
/// elaborasi penuh ke cache `elaborate/` agar warm run dapat meng-restore IR
/// dan melewati elaborator (db.md "5. elaborate/").
pub fn serialize_ir(ir: &mivon_ir::IrDesign) -> Result<Vec<u8>, String> {
    bincode::serialize(ir).map_err(|e| format!("MICD IR serialize: {}", e))
}

/// Deserialisasi `IrDesign` dari bytes. `None` bila format tak dikenal/corrupt
/// (pemanggil fallback ke elaborasi penuh).
pub fn deserialize_ir(bytes: &[u8]) -> Option<mivon_ir::IrDesign> {
    bincode::deserialize(bytes).ok()
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::compile_session::SessionConfig;
    use crate::frontend::CompileSession;

    /// Resolve path relatif ke root workspace (cwd test = direktori crate).
    fn root_rel(rel: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .unwrap()
            .join(rel)
    }

    #[test]
    fn test_design_roundtrip() {
        let config = SessionConfig {
            sources: vec![root_rel("test/counter.sv")],
            ..Default::default()
        };
        let mut session = CompileSession::new(config);
        let (design, _) = session.compile().unwrap();
        assert!(!design.modules.is_empty());

        let bytes = serialize_design(&design).expect("serialize");
        let restored = deserialize_design(&bytes).expect("deserialize");
        assert_eq!(restored, design, "Design harus round-trip identik");
    }

    #[test]
    fn test_deserialize_garbage_returns_none() {
        assert!(deserialize_design(&[0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01]).is_none());
        assert!(deserialize_design(&[]).is_none());
    }

    #[test]
    fn test_ir_roundtrip() {
        // IrDesign hasil elaborasi harus round-trip identik melalui bincode
        // (db.md "5. elaborate/": IR di-cache agar warm run melewati
        // elaborator). Symbol di-intern ulang sebagai string → struktur sama.
        let config = SessionConfig {
            sources: vec![root_rel("test/counter.sv")],
            ..Default::default()
        };
        let mut session = CompileSession::new(config);
        let (design, _) = session.compile().unwrap();
        let (_, ir, _) = session
            .compile_and_elaborate(Some("counter"))
            .expect("elaborate");
        assert!(!ir.top.processes.is_empty(), "top punya proses");

        let bytes = serialize_ir(&ir).expect("serialize IR");
        let restored = deserialize_ir(&bytes).expect("deserialize IR");
        assert_eq!(restored.top.name, ir.top.name, "top module sama");
        assert_eq!(restored.top.signals.len(), ir.top.signals.len());
        assert_eq!(restored.modules.len(), ir.modules.len());
        assert_eq!(restored.hier_signal_map.len(), ir.hier_signal_map.len());
        let _ = design;
    }

    #[test]
    fn test_deserialize_ir_garbage_returns_none() {
        assert!(deserialize_ir(&[0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01]).is_none());
        assert!(deserialize_ir(&[]).is_none());
    }
}
