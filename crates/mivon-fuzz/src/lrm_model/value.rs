//! LrmValue — representasi nilai 4-state LRM canonical.

#![cfg(feature = "dev")]

/// Nilai 4-state LRM — representasi canonical independen dari mivon-ir.
#[derive(Debug, Clone, PartialEq)]
pub enum LrmValue {
    /// Integer 4-state: bit per bit 0/1/X/Z, lebar eksplisit.
    FourState { bits: Vec<LogicBit>, width: u32 },
    Real(f64),
    Str(String),
    /// Nilai tidak terdefinisi oleh LRM untuk kasus ini.
    LrmUndefined,
}

impl LrmValue {
    /// Display singkat untuk bug report.
    pub fn display(&self) -> String {
        match self {
            LrmValue::FourState { bits, width } => {
                // Tampilkan sebagai hex bila semua bit 0/1, 4-state otherwise.
                let has_xz = bits.iter().any(|b| matches!(b, LogicBit::X | LogicBit::Z));
                if has_xz {
                    let s: String = bits.iter().map(|b| b.char()).collect();
                    format!("{}'b{}", width, s)
                } else {
                    let val: u64 = bits.iter().enumerate().fold(0u64, |acc, (i, b)| {
                        let bit_idx = bits.len() - 1 - i;
                        if *b == LogicBit::One && bit_idx < 64 {
                            acc | (1u64 << bit_idx)
                        } else {
                            acc
                        }
                    });
                    format!("{}'h{:X}", width, val)
                }
            }
            LrmValue::Real(f) => format!("{}", f),
            LrmValue::Str(s) => format!("\"{}\"", s),
            LrmValue::LrmUndefined => "<undefined>".to_string(),
        }
    }

    /// Buat LrmValue dari nilai u64 dengan lebar tertentu.
    pub fn from_u64(val: u64, width: u32) -> Self {
        let bits: Vec<LogicBit> = (0..width)
            .rev()
            .map(|i| {
                if i < 64 && (val >> i) & 1 == 1 {
                    LogicBit::One
                } else {
                    LogicBit::Zero
                }
            })
            .collect();
        LrmValue::FourState { bits, width }
    }

    /// Apakah semua bit X.
    pub fn is_all_x(&self) -> bool {
        match self {
            LrmValue::FourState { bits, .. } => bits.iter().all(|b| *b == LogicBit::X),
            _ => false,
        }
    }

    /// Apakah semua bit Z.
    pub fn is_all_z(&self) -> bool {
        match self {
            LrmValue::FourState { bits, .. } => bits.iter().all(|b| *b == LogicBit::Z),
            _ => false,
        }
    }
}

/// Satu bit 4-state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicBit {
    Zero,
    One,
    X,
    Z,
}

impl LogicBit {
    pub fn char(self) -> char {
        match self {
            LogicBit::Zero => '0',
            LogicBit::One => '1',
            LogicBit::X => 'x',
            LogicBit::Z => 'z',
        }
    }
}
