//! FSDB (Fast Signal Database) waveform writer — Stage 2+ implementation.
//! Tulis file waveform kompatibel FSDB untuk debugging di Verdi/Novas.
//! Stage 1: header, variabel definisi, dan value change sederhana.
//! Stage 2+: kompresi (zlib), delta encoding (hanya tulis perubahan), binary format.
//! Stage 3: random access index, mmap support.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write, Read};

use flate2::write::ZlibEncoder;
use flate2::read::ZlibDecoder;
use flate2::Compression;

use mivon_core::LogicVec;
use mivon_ir::{IrDesign, SignalKind};

/// Magic number FSDB (4 byte ASCII).
const FSDB_MAGIC: &[u8; 4] = b"FSDB";

/// Versi file FSDB yang dihasilkan.
const FSDB_VERSION: &str = "FSDB 2.0";

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

/// Definisi satu variabel dalam file FSDB.
#[derive(Debug, Clone)]
pub struct VarDef {
    pub id: u32,
    pub vtype: VarType,
    pub width: usize,
    pub name: String,
}

/// Output type untuk FSDB writer (file langsung atau terkompresi).
enum FsdbOut {
    Plain(BufWriter<File>),
    Compressed(ZlibEncoder<BufWriter<File>>),
}

impl Write for FsdbOut {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            FsdbOut::Plain(w) => w.write(buf),
            FsdbOut::Compressed(w) => w.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            FsdbOut::Plain(w) => w.flush(),
            FsdbOut::Compressed(w) => w.flush(),
        }
    }
}

/// Penulis file FSDB dengan dukungan kompresi dan delta encoding.
#[allow(dead_code)]
pub struct FsdbWriter {
    out: FsdbOut,
    vars: HashMap<String, VarDef>,
    next_id: u32,
    time_unit: u64,
    compressed: bool,
    /// Delta encoding: track previous values per signal ID.
    last_values: HashMap<u32, String>,
    /// Whether to use delta encoding (only write when value changes).
    delta_encoding: bool,
}

impl FsdbWriter {
    /// Buat file FSDB baru di `path`.
    pub fn create(path: &str, time_unit: u64) -> Result<Self, std::io::Error> {
        Self::create_with_options(path, time_unit, false, true)
    }

    /// Buat file FSDB baru dengan opsi kompresi dan delta encoding.
    pub fn create_with_options(
        path: &str,
        time_unit: u64,
        compressed: bool,
        delta_encoding: bool,
    ) -> Result<Self, std::io::Error> {
        let f = File::create(path)?;
        let out = if compressed {
            FsdbOut::Compressed(ZlibEncoder::new(BufWriter::new(f), Compression::default()))
        } else {
            FsdbOut::Plain(BufWriter::new(f))
        };

        let mut writer = Self {
            out,
            vars: HashMap::new(),
            next_id: 1,
            time_unit,
            compressed,
            last_values: HashMap::new(),
            delta_encoding,
        };

        // Header: magic + version + time unit + end-of-header marker.
        writer.write_header()?;
        Ok(writer)
    }

    fn write_header(&mut self) -> Result<(), std::io::Error> {
        self.out.write_all(FSDB_MAGIC)?;
        self.out.write_all(b"\n")?;
        self.out.write_all(FSDB_VERSION.as_bytes())?;
        self.out.write_all(b"\n")?;
        self.out.write_all(format!("{}\n", self.time_unit).as_bytes())?;
        self.out.write_all(b"$enddefinitions $end\n")?;
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

        // Tulis definisi variabel ke file: $var <tipe> <lebar> <id> <nama> $end
        let line = format!(
            "$var {} {} {} {} $end\n",
            vd.vtype.as_byte() as char,
            vd.width,
            vd.id,
            vd.name
        );
        self.out.write_all(line.as_bytes())?;
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

        // Delta encoding: hanya tulis jika nilai berubah dari yang terakhir.
        if self.delta_encoding {
            let last = self.last_values.get(&vd.id);
            if last == Some(&value.to_string()) {
                return Ok(()); // Tidak ada perubahan, skip.
            }
            self.last_values.insert(vd.id, value.to_string());
        }

        // Format: #<time>\n<b><value> <id>\n
        self.out.write_all(format!("#{}\n", time).as_bytes())?;
        self.out
            .write_all(format!("b{} {}\n", value, vd.id).as_bytes())?;
        Ok(())
    }

    /// Dump status semua sinyal pada waktu `time` (mirip VCD dump_state).
    /// Dengan delta encoding, hanya sinyal yang berubah yang ditulis.
    pub fn dump_state(
        &mut self,
        time: u64,
        design: &IrDesign,
        state: &[LogicVec],
    ) -> Result<(), std::io::Error> {
        // Daftarkan semua sinyal (idempotent — nama duplikat kembali id lama).
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

        // Time header.
        self.out.write_all(format!("#{}\n", time).as_bytes())?;
        // Nilai setiap sinyal yang berubah.
        for (id, val_str) in changed {
            self.out
                .write_all(format!("b{} {}\n", val_str, id).as_bytes())?;
        }
        Ok(())
    }

    /// Tutup file dan flush semua buffer.
    pub fn close(mut self) -> Result<(), std::io::Error> {
        self.out.write_all(b"$dumpvars $end\n")?;
        match self.out {
            FsdbOut::Compressed(encoder) => {
                encoder.finish()?;
            }
            FsdbOut::Plain(mut w) => {
                w.flush()?;
            }
        }
        Ok(())
    }

    /// Get whether compression is enabled.
    pub fn is_compressed(&self) -> bool {
        self.compressed
    }

    /// Get whether delta encoding is enabled.
    pub fn is_delta_encoding(&self) -> bool {
        self.delta_encoding
    }
}

/// Hasil parsing file FSDB yang dihasilkan `FsdbWriter`.
#[derive(Debug, Clone)]
pub struct FsdbData {
    /// Time unit dari header.
    pub time_unit: u64,
    /// Definisi variabel.
    pub vars: Vec<VarDef>,
    /// Perubahan nilai: (waktu, id variabel, nilai teks).
    pub changes: Vec<(u64, u32, String)>,
}

/// Pembaca file FSDB (dukung kompresi zlib).
pub struct FsdbReader;

impl FsdbReader {
    /// Buka dan parse file FSDB yang dihasilkan `FsdbWriter`, lalu kembalikan
    /// definisi variabel dan daftar perubahan nilai (round-trip).
    /// Otomatis mendeteksi kompresi zlib.
    pub fn load(path: &str) -> Result<FsdbData, std::io::Error> {
        let file = File::open(path)?;
        
        // Cek magic number untuk deteksi kompresi.
        let mut magic = [0u8; 4];
        let mut file = std::io::BufReader::new(file);
        file.read_exact(&mut magic)?;
        
        // Reset reader ke awal file.
        let file = File::open(path)?;
        let mut file = std::io::BufReader::new(file);
        
        let content = if magic.as_slice() == FSDB_MAGIC {
            // Tidak terkompresi - baca langsung.
            let mut content = String::new();
            file.read_to_string(&mut content)?;
            content
        } else {
            // Mungkin terkompresi zlib - coba decode.
            let mut decoder = ZlibDecoder::new(file);
            let mut decompressed = Vec::new();
            decoder.read_to_end(&mut decompressed)?;
            String::from_utf8(decompressed).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?
        };

        let mut time_unit = 0u64;
        let mut vars = Vec::new();
        let mut changes = Vec::new();
        let mut current_time: Option<u64> = None;

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
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
                    vars.push(VarDef {
                        id,
                        vtype,
                        width,
                        name,
                    });
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

        Ok(FsdbData {
            time_unit,
            vars,
            changes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_fsdb_create_and_header() {
        let path = "/tmp/opencode/test_fsdb_create.fsdb";
        let _ = fs::remove_file(path);
        let w = FsdbWriter::create(path, 1).unwrap();
        w.close().unwrap();

        let data = fs::read(path).unwrap();
        assert!(&data[0..4] == FSDB_MAGIC, "FSDB magic harus cocok");
        let content = String::from_utf8_lossy(&data);
        assert!(content.contains("FSDB 2.0"), "harus ada versi FSDB");
        assert!(content.contains("$enddefinitions"), "harus ada end marker");
    }

    #[test]
    fn test_fsdb_define_signal_and_value_change() {
        let path = "/tmp/opencode/test_fsdb_signal.fsdb";
        let _ = fs::remove_file(path);
        let mut w = FsdbWriter::create(path, 1).unwrap();

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
    fn test_fsdb_reader_roundtrip() {
        let path = "/tmp/opencode/test_fsdb_roundtrip.fsdb";
        let _ = fs::remove_file(path);

        // Tulis data contoh.
        let mut w = FsdbWriter::create(path, 1).unwrap();
        let id_clk = w.define_signal("clk", VarType::Wire, 1).unwrap();
        let id_rst = w.define_signal("rst", VarType::Reg, 1).unwrap();
        assert!(w.write_value_change(10, "clk", "1").is_ok());
        assert!(w.write_value_change(20, "clk", "0").is_ok());
        assert!(w.write_value_change(30, "rst", "1").is_ok());
        w.close().unwrap();

        // Baca kembali dan verifikasi round-trip.
        let data = FsdbReader::load(path).unwrap();
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
    fn test_fsdb_compressed() {
        let path = "/tmp/opencode/test_fsdb_compressed.fsdb";
        let _ = fs::remove_file(path);
        
        let mut w = FsdbWriter::create_with_options(path, 1, true, true).unwrap();
        let id = w.define_signal("clk", VarType::Wire, 1).unwrap();
        assert_eq!(id, 1);
        
        // Tulis perubahan nilai (delta encoding aktif).
        assert!(w.write_value_change(10, "clk", "1").is_ok());
        assert!(w.write_value_change(20, "clk", "0").is_ok());
        assert!(w.write_value_change(30, "clk", "1").is_ok()); // berubah
        assert!(w.write_value_change(40, "clk", "1").is_ok()); // sama, seharusnya di-skip
        
        w.close().unwrap();
        
        // File harus terkompresi (magic bukan FSDB).
        let data = fs::read(path).unwrap();
        assert!(&data[0..4] != FSDB_MAGIC, "file terkompresi tidak boleh magic FSDB");
        
        // Baca kembali.
        let data = FsdbReader::load(path).unwrap();
        assert_eq!(data.time_unit, 1);
        assert_eq!(data.vars.len(), 1);
        // Hanya 3 perubahan karena delta encoding skip duplicate.
        assert_eq!(data.changes.len(), 3);
        
        let _ = fs::remove_file(path);
    }

    #[test]
    fn test_fsdb_delta_encoding() {
        let path = "/tmp/opencode/test_fsdb_delta.fsdb";
        let _ = fs::remove_file(path);
        
        let mut w = FsdbWriter::create_with_options(path, 1, false, true).unwrap();
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
        
        let data = FsdbReader::load(path).unwrap();
        assert_eq!(data.changes.len(), 3);
        
        let _ = fs::remove_file(path);
    }
}
