//! Fix-it hint — representasi edit kode konkret yang bisa diterapkan otomatis.

/// Fix-it hint — representasi edit kode konkret yang bisa diterapkan otomatis.
#[derive(Debug, Clone)]
pub struct FixItHint {
    /// File yang perlu diedit
    pub file: String,
    /// Baris awal (1-based)
    pub start_line: usize,
    /// Kolom awal (1-based)
    pub start_col: usize,
    /// Baris akhir (1-based)
    pub end_line: usize,
    /// Kolom akhir (1-based)
    pub end_col: usize,
    /// Teks pengganti (kosong = hapus)
    pub replacement: String,
    /// Deskripsi singkat fix-it
    pub description: String,
}

impl FixItHint {
    /// Buat fix-it untuk insert teks di posisi tertentu
    pub fn insert(
        file: impl Into<String>,
        line: usize,
        col: usize,
        text: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        FixItHint {
            file: file.into(),
            start_line: line,
            start_col: col,
            end_line: line,
            end_col: col,
            replacement: text.into(),
            description: description.into(),
        }
    }

    /// Buat fix-it untuk replace rentang teks
    pub fn replace(
        file: impl Into<String>,
        start_line: usize,
        start_col: usize,
        end_line: usize,
        end_col: usize,
        replacement: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        FixItHint {
            file: file.into(),
            start_line,
            start_col,
            end_line,
            end_col,
            replacement: replacement.into(),
            description: description.into(),
        }
    }

    /// Buat fix-it untuk hapus rentang teks
    pub fn delete(
        file: impl Into<String>,
        start_line: usize,
        start_col: usize,
        end_line: usize,
        end_col: usize,
        description: impl Into<String>,
    ) -> Self {
        FixItHint::replace(
            file,
            start_line,
            start_col,
            end_line,
            end_col,
            "",
            description,
        )
    }
}
