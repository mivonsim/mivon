//! Tema tampilan TUI (gelap/terang). Tema dipilih pengguna dan disimpan di
//! `GuiState.theme`; setiap panel membaca warna melalui fungsi helper di sini
//! agar satu tombol toggle meresmasi seluruh tampilan.
//!
//! Saat ini diterapkan pada toolbar + bottom tabs + border panel. Panel lain
//! (editor, waveform, debugger) masih warna default — akan difiturisasi
//! bertahap ke tema penuh.

use ratatui::style::Color;

/// Tema warna yang tersedia.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Theme {
    /// Latar gelap, teks terang (default, nyaman untuk sesi lama).
    #[default]
    Dark,
    /// Latar terang, teks gelap (cocok ruang terang/proyektor).
    Light,
    /// Kontras tinggi untuk aksesibilitas (warna pekat, tebal).
    HighContrast,
}

impl Theme {
    /// Warna latar utama panel.
    pub fn bg(self) -> Color {
        match self {
            Theme::Dark => Color::Black,
            Theme::Light => Color::White,
            Theme::HighContrast => Color::Black,
        }
    }

    /// Warna teks utama.
    pub fn fg(self) -> Color {
        match self {
            Theme::Dark => Color::White,
            Theme::Light => Color::Black,
            Theme::HighContrast => Color::White,
        }
    }

    /// Warna latar area aktif (tab aktif, baris terpilih).
    pub fn active_bg(self) -> Color {
        match self {
            Theme::Dark => Color::Blue,
            Theme::Light => Color::LightBlue,
            Theme::HighContrast => Color::White,
        }
    }

    /// Warna teks pada area aktif.
    pub fn active_fg(self) -> Color {
        match self {
            Theme::Dark => Color::White,
            Theme::Light => Color::Black,
            Theme::HighContrast => Color::Black,
        }
    }

    /// Warna border panel.
    pub fn border(self) -> Color {
        match self {
            Theme::Dark => Color::DarkGray,
            Theme::Light => Color::Gray,
            Theme::HighContrast => Color::White,
        }
    }

    /// Warna highlight (tombol aktif, indikator penting).
    pub fn highlight(self) -> Color {
        match self {
            Theme::Dark => Color::Yellow,
            Theme::Light => Color::Indexed(136), // kuning tua, kontras pada latar terang
            Theme::HighContrast => Color::Yellow,
        }
    }

    /// Ganti ke tema berikutnya (Dark → Light → HighContrast → Dark).
    pub fn next(self) -> Self {
        match self {
            Theme::Dark => Theme::Light,
            Theme::Light => Theme::HighContrast,
            Theme::HighContrast => Theme::Dark,
        }
    }

    /// Label singkat untuk penyimpanan workspace / tampilan.
    pub fn label(self) -> &'static str {
        match self {
            Theme::Dark => "dark",
            Theme::Light => "light",
            Theme::HighContrast => "high-contrast",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_next_cycles() {
        assert_eq!(Theme::Dark.next(), Theme::Light);
        assert_eq!(Theme::Light.next(), Theme::HighContrast);
        assert_eq!(Theme::HighContrast.next(), Theme::Dark);
    }

    #[test]
    fn theme_default_is_dark() {
        assert_eq!(Theme::default(), Theme::Dark);
    }

    #[test]
    fn high_contrast_uses_white_border() {
        assert_eq!(Theme::HighContrast.border(), Color::White);
    }
}
