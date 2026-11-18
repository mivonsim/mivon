//! FSDB (Fast Signal Database) waveform writer — stage 1 of CRIT-033.
//! Tulis file waveform kompatibel FSDB untuk debugging di Verdi/Novas.
//! Stage 1: header, variabel definisi, dan value change sederhana.
//! Tahap lanjutan: kompresi, delta encoding, dan random access.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};

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
struct VarDef {
    id: u32,
    vtype: VarType,
    width: usize,
    name: String,
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

    /// Tutup file dan flush semua buffer.
    pub fn close(mut self) -> Result<(), std::io::Error> {
        self.out.write_all(b"$dumpvars $end\n")?;
        self.out.flush()?;
        Ok(())
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
}
