//! Serialisasi payload AST/IR dan versi skemanya.
//!
//! Bincode positional: bila struct AST/IR berubah, versi WAJIB naik agar node
//! lama ditolak (bukan salah-decode diam-diam).

use mivon_ast::Design;

/// Versi format serialisasi AST.
pub const AST_FORMAT_VERSION: u64 = 10;

/// Versi format serialisasi IR hasil elaborasi.
pub const IR_FORMAT_VERSION: u64 = 4;

/// Serialisasi `Design` → bytes biner.
pub fn serialize_design(design: &Design) -> Result<Vec<u8>, String> {
    bincode::serialize(design).map_err(|e| format!("node_cache AST serialize: {}", e))
}

/// Deserialisasi `Design`; `None` bila corrupt/format tak dikenal.
pub fn deserialize_design(bytes: &[u8]) -> Option<Design> {
    bincode::deserialize(bytes).ok()
}

/// Serialisasi `IrDesign` → bytes biner.
pub fn serialize_ir(ir: &mivon_ir::IrDesign) -> Result<Vec<u8>, String> {
    bincode::serialize(ir).map_err(|e| format!("node_cache IR serialize: {}", e))
}

/// Deserialisasi `IrDesign`; `None` bila corrupt/format tak dikenal.
pub fn deserialize_ir(bytes: &[u8]) -> Option<mivon_ir::IrDesign> {
    bincode::deserialize(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn garbage_design_is_rejected() {
        assert!(deserialize_design(&[]).is_none());
        assert!(deserialize_design(&[0xDE, 0xAD, 0xBE, 0xEF]).is_none());
    }

    #[test]
    fn garbage_ir_is_rejected() {
        assert!(deserialize_ir(&[]).is_none());
        assert!(deserialize_ir(&[0xDE, 0xAD, 0xBE, 0xEF]).is_none());
    }
}
