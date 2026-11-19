//! FSDB (Fast Signal Database) waveform writer — stage 1 of CRIT-033.
//! Tulis file waveform kompatibel FSDB untuk debugging di Verdi/Novas.
//! Stage 1: header, variabel definisi, dan value change sederhana.
//! Tahap lanjutan: kompresi, delta encoding, dan random access.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};

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

/// Penulis file FSDB.
#[allow(dead_code)]
pub struct FsdbWriter {
    out: BufWriter<File>,
    vars: HashMap<String, VarDef>,
    next_id: u32,
    time_unit: u64, // 1 = 1ns, 10 = 100ps, 100 = 10ps, dst.
}

impl FsdbWriter {
    /// Buat file FSDB baru di `path`.
    pub fn create(path: &str, time_unit: u64) -> Result<Self, std::io::Error> {
        let f = File::create(path)?;
        let mut out = BufWriter::new(f);

        // Header: magic + version + time unit + end-of-header marker.
        out.write_all(FSDB_MAGIC)?;
        out.write_all(b"\n")?;
        out.write_all(FSDB_VERSION.as_bytes())?;
        out.write_all(b"\n")?;
        out.write_all(format!("{}\n", time_unit).as_bytes())?;
        out.write_all(b"$enddefinitions $end\n")?;

        Ok(Self {
            out,
            vars: HashMap::new(),
            next_id: 1,
            time_unit,
        })
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

        // Format: #<time>\n<b><value> <id>\n
        self.out.write_all(format!("#{}\n", time).as_bytes())?;
        self.out
            .write_all(format!("b{} {}\n", value, vd.id).as_bytes())?;
        Ok(())
    }

    /// Dump status semua sinyal pada waktu `time` (mirip VCD dump_state).
    /// Mendaftarkan seluruh sinyal dari design (idempotent via
    /// define_signal), lalu menulis time header dan nilai setiap sinyal.
    /// Indeks sinyal di design.top.signals == indeks nilai di state
    /// (SignalId adalah indeks posisi ke state.signals).
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
            self.define_signal(&sig.name.as_str(), vtype, sig.width)?;
        }
        // Time header.
        self.out.write_all(format!("#{}\n", time).as_bytes())?;
        // Nilai setiap sinyal (indeks signal == indeks state).
        for (i, sig) in design.top.signals.iter().enumerate() {
            let id = match self.vars.get(sig.name.as_str()) {
                Some(vd) => vd.id,
                None => continue,
            };
            let val_str = format!("{}", state[i]);
            self.out
                .write_all(format!("b{} {}\n", val_str, id).as_bytes())?;
        }
        Ok(())
    }

    /// Tutup file dan flush semua buffer.
    pub fn close(mut self) -> Result<(), std::io::Error> {
        self.out.write_all(b"$dumpvars $end\n")?;
        self.out.flush()?;
        Ok(())
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

/// Pembaca file FSDB.
pub struct FsdbReader;

impl FsdbReader {
    /// Buka dan parse file FSDB yang dihasilkan `FsdbWriter`, lalu kembalikan
    /// definisi variabel dan daftar perubahan nilai (round-trip).
    pub fn load(path: &str) -> Result<FsdbData, std::io::Error> {
        let content = std::fs::read_to_string(path)?;
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
                // Format: $var <tipe_byte> <width> <id> <name> $end
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
                // Format: #<time>
                current_time = Some(t.parse().unwrap_or(0));
            } else if let Some(rest) = line.strip_prefix('b') {
                // Format: b<value> <id>
                let parts: Vec<&str> = rest.splitn(2, char::is_whitespace).collect();
                if parts.len() >= 2 {
                    let value = parts[0].to_string();
                    let id: u32 = parts[1].parse().unwrap_or(0);
                    let time = current_time.unwrap_or(0);
                    changes.push((time, id, value));
                }
            } else if let Ok(tu) = line.parse::<u64>() {
                // Baris angka polos: time unit dari header.
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
}
