//! Plugin/Extension API internal untuk Mivon GUI TUI.
//!
//! Plugin memungkinkan menambahkan custom command dan panel ke GUI tanpa
//! mengubah kode inti. Plugin terdaftar secara internal (bukan shared library
//! eksternal) melalui trait object, sehingga aman dari sisi borrow checker
//! dan dapat di-unit-test.
//!
//! Contoh pengguna:
//!   let plugin = MyPlugin::new();
//!   state.plugin_registry.register(Box::new(plugin));

use ratatui::{
    layout::Rect,
    style::Style,
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use super::state::GuiState;

/// Setiap plugin wajib implementasi trait ini.
pub trait Plugin: Send + Sync {
    /// Nama unik plugin (tampilkan di daftar plugin).
    fn name(&self) -> &str;
    /// Versi plugin.
    fn version(&self) -> &str {
        "0.1.0"
    }
    /// Deskripsi singkat plugin.
    fn description(&self) -> &str {
        ""
    }
    /// Jalankan perintah plugin. Mengembalikan pesan log (jika ada) agar caller
    /// dapat memanggil state.log setelah borrow registry dilepaskan.
    fn exec(&self, _cmd: &str) -> Option<String> {
        None
    }
    /// Render panel plugin (jika plugin memiliki panel visual).
    fn render(&self, _f: &mut Frame, _area: Rect, _state: &GuiState) {
        // Default: tidak ada panel.
    }
    /// Apakah plugin memiliki panel visual yang ingin ditampilkan di bottom tab.
    fn has_panel(&self) -> bool {
        false
    }
}

/// Wrapper untuk plugin trait object (karena Plugin tidak object-safe karena
/// generic params tidak ada, maka ini aman).
pub struct BoxedPlugin {
    pub name: String,
    pub version: String,
    pub description: String,
    pub inner: Box<dyn Plugin>,
}

impl BoxedPlugin {
    pub fn new<P: Plugin + 'static>(plugin: P) -> Self {
        BoxedPlugin {
            name: plugin.name().to_string(),
            version: plugin.version().to_string(),
            description: plugin.description().to_string(),
            inner: Box::new(plugin),
        }
    }
}

/// Registry plugin: menyimpan daftar plugin yang terdaftar.
#[derive(Default)]
pub struct PluginRegistry {
    pub plugins: Vec<BoxedPlugin>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Daftarkan plugin baru.
    pub fn register(&mut self, plugin: BoxedPlugin) {
        self.plugins.push(plugin);
    }

    /// Cari plugin berdasarkan nama.
    pub fn find(&self, name: &str) -> Option<&BoxedPlugin> {
        self.plugins.iter().find(|p| p.name == name)
    }

    /// Jalankan perintah pada plugin bernama `name`. Mengembalikan pesan log
    /// (jika ada) agar caller dapat memanggil state.log setelah borrow
    /// registry dilepaskan.
    pub fn exec(&mut self, name: &str, cmd: &str) -> Option<String> {
        if let Some(p) = self.plugins.iter_mut().find(|p| p.name == name) {
            return p.inner.exec(cmd);
        }
        None
    }

    /// Jumlah plugin terdaftar.
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }
}

/// Contoh plugin bawaan: "Hello" plugin yang menampilkan pesan sambutan.
pub struct HelloPlugin;

impl Plugin for HelloPlugin {
    fn name(&self) -> &str {
        "hello"
    }
    fn version(&self) -> &str {
        "1.0.0"
    }
    fn description(&self) -> &str {
        "Plugin contoh: menampilkan pesan sambutan"
    }
    fn exec(&self, cmd: &str) -> Option<String> {
        if cmd == "hello" {
            return Some("👋 Halo dari plugin hello!".to_string());
        }
        None
    }
}

/// Contoh plugin bawaan: "Stats" plugin yang menampilkan statistik GUI.
pub struct StatsPlugin;

impl Plugin for StatsPlugin {
    fn name(&self) -> &str {
        "stats"
    }
    fn version(&self) -> &str {
        "1.0.0"
    }
    fn description(&self) -> &str {
        "Plugin contoh: tampilkan statistik GUI"
    }
    fn exec(&self, _cmd: &str) -> Option<String> {
        Some(
            "📊 Stats: <file_count> file terbuka, <diag_count> diagnostics, split=<split>"
                .to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn test_state() -> GuiState {
        let (tx, rx) = channel();
        GuiState::new(tx, rx)
    }

    #[test]
    fn plugin_register_and_find() {
        let mut reg = PluginRegistry::new();
        assert!(reg.is_empty());
        reg.register(BoxedPlugin::new(HelloPlugin));
        assert_eq!(reg.len(), 1);
        assert!(reg.find("hello").is_some());
        assert!(reg.find("nonexistent").is_none());
    }

    #[test]
    fn plugin_exec_hello() {
        let mut reg = PluginRegistry::new();
        reg.register(BoxedPlugin::new(HelloPlugin));
        assert_eq!(
            reg.exec("hello", "hello"),
            Some("👋 Halo dari plugin hello!".to_string())
        );
        assert_eq!(reg.exec("hello", "unknown"), None);
        assert_eq!(reg.exec("nonexistent", "hello"), None);
    }

    #[test]
    fn plugin_stats() {
        let mut reg = PluginRegistry::new();
        reg.register(BoxedPlugin::new(StatsPlugin));
        assert_eq!(
            reg.exec("stats", "stats"),
            Some("📊 Stats: <file_count> file terbuka, <diag_count> diagnostics, split=<split>"
                .to_string())
        );
    }
}
