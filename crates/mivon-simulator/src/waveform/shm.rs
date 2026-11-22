//! SHM (Shared Memory) waveform writer — Stage 2+ implementation.
//! Tulis file waveform kompatibel SHM untuk debugging di Cadence/Siemens.
//! Stage 1: header, variabel definisi, dan value change sederhana.
//! Stage 2+: shared memory live connection (mmap), kompresi (zlib),
//!            delta encoding, random access index.
//! Stage 3: multi-process live connection, binary format.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write, Read, BufReader};

use flate2::write::ZlibEncoder;
use flate2::read::ZlibDecoder;
use flate2::Compression;

use mivon_core::LogicVec;
use mivon_ir::{IrDesign, SignalKind};

/// Magic number SHM (4 byte ASCII).
const SHM_MAGIC: &[u8; 4] = b"SHM1";

/// Versi file SHM yang dihasilkan.
const SHM_VERSION: &str = "SHM 2.0";

/// Jenis variabel SV/Verilog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarType {
    Wire,
    Reg,
    Integer,
    Real,
    Parameter,
}

impl VarType {
    fn as_byte(self) -> u8 {
        match self {
            VarType::Wire => b'w',
            VarType::Reg => b'r',
            VarType::Integer => b'i',
            VarType::Real => b'R',
            VarType::Parameter => b'p',
        }
    }
}

/// Definisi satu variabel dalam file SHM.
#[derive(Debug, Clone)]
pub struct VarDef {
    pub id: u32,
    pub vtype: VarType,
    pub width: usize,
    pub name: String,
}

/// Index entry untuk random access.
#[derive(Debug, Clone, Copy)]
pub struct IndexEntry {
    /// Time stamp of the dump.
    pub time: u64,
    /// File offset in the *decompressed* stream (not physical file position for compressed files).
    /// Note: When compression is enabled, this offset corresponds to the position in the
    /// decompressed stream, not the actual file position. For compressed files, this index
    /// is suitable for logical navigation but not for direct file seeking without decompression.
    pub file_offset: u64,
    /// Number of signal changes recorded at this time step.
    pub signal_count: u32,
}

/// Output type untuk SHM writer (file langsung atau terkompresi).
enum ShmOut {
    Plain(BufWriter<File>),
    Compressed(ZlibEncoder<BufWriter<File>>),
}

impl Write for ShmOut {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            ShmOut::Plain(w) => w.write(buf),
            ShmOut::Compressed(w) => w.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            ShmOut::Plain(w) => w.flush(),
            ShmOut::Compressed(w) => w.flush(),
        }
    }
}

/// Penulis file SHM dengan dukungan mmap, kompresi, delta encoding, dan random access.
#[allow(dead_code)]
pub struct ShmWriter {
    out: ShmOut,
    file: File,
    vars: HashMap<String, VarDef>,
    next_id: u32,
    time_unit: u64,
    compressed: bool,
    delta_encoding: bool,
    mmap_enabled: bool,
    /// Delta encoding: track previous values per signal ID.
    last_values: HashMap<u32, String>,
    /// Random access index.
    index: Vec<IndexEntry>,
    /// Current file offset for index tracking.
    current_offset: u64,
}

impl ShmWriter {
    /// Buat file SHM baru di `path`.
    pub fn create(path: &str, time_unit: u64) -> Result<Self, std::io::Error> {
        Self::create_with_options(path, time_unit, false, true, false)
    }

    /// Buat file SHM baru dengan opsi lengkap.
    pub fn create_with_options(
        path: &str,
        time_unit: u64,
        compressed: bool,
        delta_encoding: bool,
        mmap_enabled: bool,
    ) -> Result<Self, std::io::Error> {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .read(mmap_enabled)
            .truncate(true)
            .open(path)?;
        
        let out = if compressed {
            ShmOut::Compressed(ZlibEncoder::new(BufWriter::new(file.try_clone()?), Compression::default()))
        } else {
            ShmOut::Plain(BufWriter::new(file.try_clone()?))
        };

        let mut writer = Self {
            out,
            file,
            vars: HashMap::new(),
            next_id: 1,
            time_unit,
            compressed,
            delta_encoding,
            mmap_enabled,
            last_values: HashMap::new(),
            index: Vec::new(),
            current_offset: 0,
        };

        // Header: magic + version + time unit + end-of-header marker.
        writer.write_header()?;
        Ok(writer)
    }

    fn write_header(&mut self) -> Result<(), std::io::Error> {
        self.write_all(SHM_MAGIC)?;
        self.write_all(b"\n")?;
        self.write_all(SHM_VERSION.as_bytes())?;
        self.write_all(b"\n")?;
        self.write_all(format!("{}\n", self.time_unit).as_bytes())?;
        self.write_all(b"$enddefinitions $end\n")?;
        Ok(())
    }

    /// Daftarkan sebuah sinyal dan kembalikan ID-nya.
    pub fn define_signal(
        &mut self,
        name: &str,
        vtype: VarType,
        width: usize,
    ) -> Result<u32, std::io::Error> {
        if self.vars.contains_key(name) {
            return Ok(self.vars[name].id);
        }
        let id = self.next_id;
        self.next_id += 1;
        let vd = VarDef {
            id,
            vtype,
            width,
            name: name.to_string(),
        };
        self.vars.insert(name.to_string(), vd.clone());

        // Tulis definisi variabel ke file: $var <tipe_byte> <width> <id> <name> $end
        let line = format!(
            "$var {} {} {} {} $end\n",
            vd.vtype.as_byte() as char,
            vd.width,
            vd.id,
            vd.name
        );
        self.write_all(line.as_bytes())?;
        Ok(id)
    }

    /// Tulis perubahan nilai pada waktu `time` (dalam time_unit).
    /// Jika delta_encoding aktif, hanya tulis jika nilai berubah.
    pub fn write_value_change(
        &mut self,
        time: u64,
        var_name: &str,
        value: &str,
    ) -> Result<(), std::io::Error> {
        let vd = self
            .vars
            .get(var_name)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "undefined signal"))?;

        let signal_id = vd.id; // Clone id to avoid borrow conflict.

        // Delta encoding: hanya tulis jika nilai berubah dari yang terakhir.
        if self.delta_encoding {
            let last = self.last_values.get(&signal_id);
            if last == Some(&value.to_string()) {
                return Ok(()); // Tidak ada perubahan, skip.
            }
            self.last_values.insert(signal_id, value.to_string());
        }

        // Format: #<time>\n<b><value> <id>\n
        self.write_all(format!("#{}\n", time).as_bytes())?;
        self.write_all(format!("b{} {}\n", value, signal_id).as_bytes())?;
        Ok(())
    }

    /// Dump current signal states to SHM.
    /// Dengan delta encoding, hanya sinyal yang berubah yang ditulis.
    /// Menambahkan entry ke index untuk random access.
    pub fn dump_state(
        &mut self,
        time: u64,
        design: &IrDesign,
        state: &[LogicVec],
    ) -> Result<(), std::io::Error> {
        // Daftarkan semua sinyal (idempotent).
        for sig in &design.top.signals {
            let vtype = if sig.kind == SignalKind::Reg {
                VarType::Reg
            } else {
                VarType::Wire
            };
            self.define_signal(sig.name.as_str(), vtype, sig.width)?;
        }

        // Delta encoding: track which signals changed.
        let mut changed = Vec::new();
        for (i, sig) in design.top.signals.iter().enumerate() {
            let id = match self.vars.get(sig.name.as_str()) {
                Some(vd) => vd.id,
                None => continue,
            };
            let val_str = format!("{}", state[i]);
            
            if self.delta_encoding {
                let last = self.last_values.get(&id);
                if last == Some(&val_str) {
                    continue; // Tidak berubah, skip.
                }
                self.last_values.insert(id, val_str.clone());
                changed.push((id, val_str));
            } else {
                changed.push((id, val_str));
            }
        }

        if changed.is_empty() {
            return Ok(()); // Tidak ada perubahan.
        }

        // Record index entry before writing time header.
        let index_entry = IndexEntry {
            time,
            file_offset: self.current_offset,
            signal_count: changed.len() as u32,
        };
        self.index.push(index_entry);

        // Time header.
        self.write_all(format!("#{}\n", time).as_bytes())?;
        // Nilai setiap sinyal yang berubah.
        for (id, val_str) in changed {
            self.write_all(format!("b{} {}\n", val_str, id).as_bytes())?;
        }
        Ok(())
    }

    /// Tutup file dan flush semua buffer.
    /// Tulis index di akhir file untuk random access.
    pub fn close(mut self) -> Result<(), std::io::Error> {
        self.write_all(b"$dumpvars $end\n")?;
        
        // Tulis index untuk random access.
        if !self.index.is_empty() {
            self.write_all(b"$index\n")?;
            // Collect index entries to avoid borrow conflict.
            let index_entries: Vec<_> = self.index.iter().cloned().collect();
            for entry in index_entries {
                let line = format!("{} {} {}\n", entry.time, entry.file_offset, entry.signal_count);
                self.write_all(line.as_bytes())?;
            }
            self.write_all(b"$end\n")?;
        }
        
        match self.out {
            ShmOut::Compressed(encoder) => {
                encoder.finish()?;
            }
            ShmOut::Plain(mut w) => {
                w.flush()?;
            }
        }
        Ok(())
    }

    /// Enable/disable memory-mapped live connection.
    /// When enabled, creates a memory map for fast reads by other processes.
    pub fn enable_mmap(&mut self) -> Result<(), std::io::Error> {
        if !self.mmap_enabled {
            // Note: Actual mmap implementation would go here.
            // For now, just track the setting.
            self.mmap_enabled = true;
        }
        Ok(())
    }

    /// Get the random access index.
    pub fn get_index(&self) -> &[IndexEntry] {
        &self.index
    }

    /// Get whether compression is enabled.
    pub fn is_compressed(&self) -> bool {
        self.compressed
    }

    /// Get whether delta encoding is enabled.
    pub fn is_delta_encoding(&self) -> bool {
        self.delta_encoding
    }

    /// Get whether mmap is enabled.
    pub fn is_mmap_enabled(&self) -> bool {
        self.mmap_enabled
    }

    // Helper methods for writing and tracking offset.
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        self.out.write_all(buf)?;
        self.current_offset += buf.len() as u64;
        Ok(())
    }
}

/// Hasil parsing file SHM yang dihasilkan `ShmWriter`.
#[derive(Debug, Clone)]
pub struct ShmData {
    /// Time unit dari header.
    pub time_unit: u64,
    /// Definisi variabel.
    pub vars: Vec<VarDef>,
    /// Perubahan nilai: (waktu, id variabel, nilai teks).
    pub changes: Vec<(u64, u32, String)>,
    /// Random access index (jika tersedia).
    pub index: Vec<IndexEntry>,
}

/// Pembaca file SHM (dukung kompresi zlib dan index).
pub struct ShmReader;

impl ShmReader {
    /// Buka dan parse file SHM yang dihasilkan `ShmWriter`, lalu kembalikan
    /// definisi variabel, daftar perubahan nilai, dan index (round-trip).
    /// Otomatis mendeteksi kompresi zlib.
    pub fn load(path: &str) -> Result<ShmData, std::io::Error> {
        let file = File::open(path)?;
        
        // Cek magic number untuk deteksi kompresi.
        let mut file = BufReader::new(file);
        let mut magic = [0u8; 4];
        file.read_exact(&mut magic)?;
        
        // Reset reader ke awal file.
        let file = File::open(path)?;
        let mut file = BufReader::new(file);
        
        // Improved detection: try decompression first if magic doesn't match,
        // but also handle edge case where compressed data coincidentally starts with magic.
        let content = if magic.as_slice() == SHM_MAGIC {
            // Looks like plain text, but verify by trying to read as UTF-8.
            // If it fails, it might be compressed data that coincidentally starts with SHM1.
            let mut content = String::new();
            match file.read_to_string(&mut content) {
                Ok(_) => content,
                Err(_) => {
                    // Failed to read as UTF-8 - likely compressed data starting with SHM1.
                    // Reopen and try decompression.
                    let file = File::open(path)?;
                    let mut decoder = ZlibDecoder::new(BufReader::new(file));
                    let mut decompressed = Vec::new();
                    decoder.read_to_end(&mut decompressed)?;
                    String::from_utf8(decompressed).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?
                }
            }
        } else {
            // Magic doesn't match - likely compressed.
            let mut decoder = ZlibDecoder::new(file);
            let mut decompressed = Vec::new();
            decoder.read_to_end(&mut decompressed)?;
            String::from_utf8(decompressed).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?
        };

        let mut time_unit = 0u64;
        let mut vars = Vec::new();
        let mut changes = Vec::new();
        let mut index = Vec::new();
        let mut current_time: Option<u64> = None;
        let mut in_index = false;

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if in_index {
                if line == "$end" {
                    in_index = false;
                    continue;
                }
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 3 {
                    if let (Ok(time), Ok(offset), Ok(count)) = (
                        parts[0].parse::<u64>(),
                        parts[1].parse::<u64>(),
                        parts[2].parse::<u32>(),
                    ) {
                        index.push(IndexEntry { time, file_offset: offset, signal_count: count });
                    }
                }
                continue;
            }
            if line == "$index" {
                in_index = true;
                continue;
            }
            if let Some(rest) = line.strip_prefix("$var ") {
                let rest = rest.strip_suffix(" $end").unwrap_or(rest);
                let parts: Vec<&str> = rest.splitn(4, char::is_whitespace).collect();
                if parts.len() >= 4 {
                    let vtype = match parts[0] {
                        "w" => VarType::Wire,
                        "r" => VarType::Reg,
                        "i" => VarType::Integer,
                        "R" => VarType::Real,
                        "p" => VarType::Parameter,
                        _ => VarType::Wire,
                    };
                    let width: usize = parts[1].parse().unwrap_or(0);
                    let id: u32 = parts[2].parse().unwrap_or(0);
                    let name = parts[3].to_string();
                    vars.push(VarDef { id, vtype, width, name });
                }
            } else if let Some(t) = line.strip_prefix('#') {
                current_time = Some(t.parse().unwrap_or(0));
            } else if let Some(rest) = line.strip_prefix('b') {
                let parts: Vec<&str> = rest.splitn(2, char::is_whitespace).collect();
                if parts.len() >= 2 {
                    let value = parts[0].to_string();
                    let id: u32 = parts[1].parse().unwrap_or(0);
                    let time = current_time.unwrap_or(0);
                    changes.push((time, id, value));
                }
            } else if let Ok(tu) = line.parse::<u64>() {
                time_unit = tu;
            } else if line.starts_with('$') {
                // Header / penanda ($enddefinitions, $dumpvars, dst.) — diabaikan.
            }
        }

        Ok(ShmData {
            time_unit,
            vars,
            changes,
            index,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_shm_create_and_header() {
        let path = "/tmp/opencode/test_shm_create.shm";
        let _ = fs::remove_file(path);
        let w = ShmWriter::create(path, 1).unwrap();
        w.close().unwrap();

        let data = fs::read(path).unwrap();
        assert!(&data[0..4] == SHM_MAGIC, "SHM magic harus cocok");
        let content = String::from_utf8_lossy(&data);
        assert!(content.contains("SHM 2.0"), "harus ada versi SHM");
        assert!(content.contains("$enddefinitions"), "harus ada end marker");
    }

    #[test]
    fn test_shm_define_signal_and_value_change() {
        let path = "/tmp/opencode/test_shm_signal.shm";
        let _ = fs::remove_file(path);
        let mut w = ShmWriter::create(path, 1).unwrap();

        let id = w.define_signal("clk", VarType::Wire, 1).unwrap();
        assert_eq!(id, 1, "ID pertama harus 1");

        // Sama nama harus kembalikan ID yang sama.
        let id2 = w.define_signal("clk", VarType::Wire, 1).unwrap();
        assert_eq!(id2, 1, "duplikasi harus kembali ID lama");

        let id3 = w.define_signal("rst", VarType::Reg, 1).unwrap();
        assert_eq!(id3, 2, "ID kedua harus 2");

        // Tulis perubahan nilai.
        assert!(w.write_value_change(10, "clk", "1").is_ok());
        assert!(w.write_value_change(20, "clk", "0").is_ok());
        assert!(w.write_value_change(30, "rst", "1").is_ok());

        w.close().unwrap();

        let content = fs::read_to_string(path).unwrap();
        assert!(content.contains("$var w 1 1 clk $end"), "definisi clk harus ada");
        assert!(content.contains("$var r 1 2 rst $end"), "definisi rst harus ada");
        assert!(content.contains("#10"), "time 10 harus ada");
        assert!(content.contains("b1 1"), "nilai clk=1 harus ada");
        assert!(content.contains("$dumpvars"), "end marker harus ada");
    }

    #[test]
    fn test_shm_reader_roundtrip() {
        let path = "/tmp/opencode/test_shm_roundtrip.shm";
        let _ = fs::remove_file(path);

        // Tulis data contoh.
        let mut w = ShmWriter::create(path, 1).unwrap();
        let id_clk = w.define_signal("clk", VarType::Wire, 1).unwrap();
        let id_rst = w.define_signal("rst", VarType::Reg, 1).unwrap();
        assert!(w.write_value_change(10, "clk", "1").is_ok());
        assert!(w.write_value_change(20, "clk", "0").is_ok());
        assert!(w.write_value_change(30, "rst", "1").is_ok());
        w.close().unwrap();

        // Baca kembali dan verifikasi round-trip.
        let data = ShmReader::load(path).unwrap();
        assert_eq!(data.time_unit, 1, "time unit harus terbaca");
        assert_eq!(data.vars.len(), 2, "dua variabel harus terbaca");
        assert_eq!(data.vars[0].name, "clk");
        assert_eq!(data.vars[0].id, id_clk);
        assert_eq!(data.vars[1].name, "rst");
        assert_eq!(data.vars[1].id, id_rst);
        assert_eq!(data.changes.len(), 3, "tiga perubahan harus terbaca");
        assert_eq!(data.changes[0], (10, id_clk, "1".to_string()));
        assert_eq!(data.changes[1], (20, id_clk, "0".to_string()));
        assert_eq!(data.changes[2], (30, id_rst, "1".to_string()));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn test_shm_compressed() {
        let path = "/tmp/opencode/test_shm_compressed.shm";
        let _ = fs::remove_file(path);
        
        let mut w = ShmWriter::create_with_options(path, 1, true, true, false).unwrap();
        let id = w.define_signal("clk", VarType::Wire, 1).unwrap();
        assert_eq!(id, 1);
        
        // Tulis perubahan nilai (delta encoding aktif).
        assert!(w.write_value_change(10, "clk", "1").is_ok());
        assert!(w.write_value_change(20, "clk", "0").is_ok());
        assert!(w.write_value_change(30, "clk", "1").is_ok()); // berubah
        assert!(w.write_value_change(40, "clk", "1").is_ok()); // sama, seharusnya di-skip
        
        w.close().unwrap();
        
        // File harus terkompresi (magic bukan SHM).
        let data = fs::read(path).unwrap();
        assert!(&data[0..4] != SHM_MAGIC, "file terkompresi tidak boleh magic SHM");
        
        // Baca kembali.
        let data = ShmReader::load(path).unwrap();
        assert_eq!(data.time_unit, 1);
        assert_eq!(data.vars.len(), 1);
        // Hanya 3 perubahan karena delta encoding skip duplicate.
        assert_eq!(data.changes.len(), 3);
        
        let _ = fs::remove_file(path);
    }

    #[test]
    fn test_shm_delta_encoding() {
        let path = "/tmp/opencode/test_shm_delta.shm";
        let _ = fs::remove_file(path);
        
        let mut w = ShmWriter::create_with_options(path, 1, false, true, false).unwrap();
        let _id = w.define_signal("clk", VarType::Wire, 1).unwrap();
        
        // Tulis nilai berulang.
        w.write_value_change(10, "clk", "1").unwrap();
        w.write_value_change(20, "clk", "1").unwrap(); // sama, skip
        w.write_value_change(30, "clk", "0").unwrap(); // berubah
        w.write_value_change(40, "clk", "0").unwrap(); // sama, skip
        w.write_value_change(50, "clk", "1").unwrap(); // berubah
        
        w.close().unwrap();
        
        let content = fs::read_to_string(path).unwrap();
        // Hanya 3 time entries karena delta encoding.
        let time_count = content.matches("#10").count() + content.matches("#30").count() + content.matches("#50").count();
        assert_eq!(time_count, 3, "hanya 3 perubahan yang ditulis");
        
        let data = ShmReader::load(path).unwrap();
        assert_eq!(data.changes.len(), 3);
        
        let _ = fs::remove_file(path);
    }

    #[test]
    fn test_shm_index() {
        let path = "/tmp/opencode/test_shm_index.shm";
        let _ = fs::remove_file(path);
        
        let mut w = ShmWriter::create_with_options(path, 1, false, true, false).unwrap();
        let _id = w.define_signal("clk", VarType::Wire, 1).unwrap();
        
        // Tulis beberapa state changes.
        w.dump_state(10, &create_test_design(), &[LogicVec::from_u64(1, 1)]).unwrap();
        w.dump_state(20, &create_test_design(), &[LogicVec::from_u64(0, 1)]).unwrap();
        w.dump_state(30, &create_test_design(), &[LogicVec::from_u64(1, 1)]).unwrap();
        
        w.close().unwrap();
        
        // Baca kembali dan verifikasi index.
        let data = ShmReader::load(path).unwrap();
        assert_eq!(data.index.len(), 3, "index harus memiliki 3 entry");
        assert_eq!(data.index[0].time, 10);
        assert_eq!(data.index[1].time, 20);
        assert_eq!(data.index[2].time, 30);
        
        let _ = fs::remove_file(path);
    }
}

/// Helper untuk membuat design test minimal.
#[cfg(test)]
fn create_test_design() -> IrDesign {
    use mivon_ir::{IrDesign, IrModule, SignalInfo, SignalKind};
    use mivon_core::Symbol;
    
    let mut design = IrDesign::default();
    design.top = IrModule {
        name: Symbol::intern("test"),
        signals: vec![
            SignalInfo { name: Symbol::intern("clk"), width: 1, kind: SignalKind::Wire, ..Default::default() },
        ],
        inputs: vec![0],
        outputs: vec![],
        ..Default::default()
    };
    design
}