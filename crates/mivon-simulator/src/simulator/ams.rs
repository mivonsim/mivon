//! Analog/Mixed-Signal (AMS) support — stage 1 of #63.
//! Minimal mixed-signal bridge: analog signal (continuous value),
//! digital-to-analog converter (DAC), analog-to-digital converter (ADC),
//! and a simple RC network solver.
//! 1 file = 1 tanggung jawab: hanya AMS modeling dasar.

use mivon_ir::LogicVec;

/// Tipe sinyal analog (nilai kontinu dalam volt).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnalogSignal {
    /// Nilai tegangan dalam volt.
    pub voltage: f64,
}

impl Default for AnalogSignal {
    fn default() -> Self {
        Self { voltage: 0.0 }
    }
}

/// Konverter digital → analog (DAC).
/// Mengubah LogicVec ke tegangan analog berdasarkan rentang referensi.
pub struct Dac {
    /// Tegangan referensi maksimal (V).
    pub vref: f64,
    /// Lebar data digital (bit).
    pub width: usize,
}

impl Dac {
    pub fn new(vref: f64, width: usize) -> Self {
        Self { vref, width }
    }

    /// Konversi nilai digital ke tegangan analog.
    pub fn convert(&self, digital: &LogicVec) -> AnalogSignal {
        let max_val: u64 = if self.width == 0 { 1 } else { (1u64 << self.width) - 1 };
        let raw = digital.to_u64();
        let clamped = raw.min(max_val);
        let voltage = if max_val == 0 {
            0.0
        } else {
            (clamped as f64 / max_val as f64) * self.vref
        };
        AnalogSignal { voltage }
    }
}

/// Konverter analog → digital (ADC).
/// Mengubah tegangan analog ke nilai digital.
pub struct Adc {
    /// Tegangan referensi maksimal (V).
    pub vref: f64,
    /// Resolusi (bit).
    pub width: usize,
}

impl Adc {
    pub fn new(vref: f64, width: usize) -> Self {
        Self { vref, width }
    }

    /// Konversi tegangan analog ke nilai digital.
    pub fn convert(&self, analog: &AnalogSignal) -> LogicVec {
        let max_val: u64 = if self.width == 0 { 1 } else { (1u64 << self.width) - 1 };
        let clamped_v = analog.voltage.clamp(0.0, self.vref);
        let raw = if self.vref == 0.0 {
            0
        } else {
            ((clamped_v / self.vref) * max_val as f64).round() as u64
        };
        let digital = raw.min(max_val);
        LogicVec::from_u64(digital, self.width.max(1))
    }
}

/// Jaringan RC sederhana untuk simulasi transient dasar.
/// Model: V_out(t+dt) = V_out(t) + dt * (V_in - V_out) / (R * C)
pub struct RcNetwork {
    /// Hambatan (ohm).
    pub r: f64,
    /// Kapasitansi (farad).
    pub c: f64,
    /// Tegangan kapasitor saat ini (V).
    pub v_out: f64,
}

impl Default for RcNetwork {
    fn default() -> Self {
        Self {
            r: 1000.0,
            c: 1e-6,
            v_out: 0.0,
        }
    }
}

impl RcNetwork {
    pub fn new(r: f64, c: f64) -> Self {
        Self { r, c, v_out: 0.0 }
    }

    /// Perbarui tegangan output dengan time step `dt` (detik).
    pub fn step(&mut self, v_in: f64, dt: f64) -> f64 {
        if self.r == 0.0 || self.c == 0.0 {
            self.v_out = v_in;
            return self.v_out;
        }
        let tau = self.r * self.c;
        let alpha = dt / tau;
        self.v_out += alpha * (v_in - self.v_out);
        self.v_out
    }

    /// Baca tegangan output saat ini.
    pub fn voltage(&self) -> f64 {
        self.v_out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dac_convert_full_scale() {
        let dac = Dac::new(3.3, 8);
        let max_digital = LogicVec::from_u64(255, 8);
        let analog = dac.convert(&max_digital);
        assert!((analog.voltage - 3.3).abs() < 1e-6, "full scale should be 3.3V");
    }

    #[test]
    fn test_dac_convert_zero() {
        let dac = Dac::new(3.3, 8);
        let zero = LogicVec::new(8);
        let analog = dac.convert(&zero);
        assert_eq!(analog.voltage, 0.0, "zero should be 0.0V");
    }

    #[test]
    fn test_dac_convert_half_scale() {
        let dac = Dac::new(5.0, 4);
        let mid = LogicVec::from_u64(7, 4); // 7/15 * 5V ≈ 2.33V
        let analog = dac.convert(&mid);
        let expected = (7.0 / 15.0) * 5.0;
        assert!((analog.voltage - expected).abs() < 1e-6);
    }

    #[test]
    fn test_adc_convert_full_scale() {
        let adc = Adc::new(3.3, 8);
        let analog = AnalogSignal { voltage: 3.3 };
        let digital = adc.convert(&analog);
        assert_eq!(digital.to_u64(), 255, "full scale should be 255");
    }

    #[test]
    fn test_adc_convert_zero() {
        let adc = Adc::new(3.3, 8);
        let analog = AnalogSignal { voltage: 0.0 };
        let digital = adc.convert(&analog);
        assert_eq!(digital.to_u64(), 0, "zero should be 0");
    }

    #[test]
    fn test_adc_roundtrip() {
        let adc = Adc::new(5.0, 10);
        let dac = Dac::new(5.0, 10);
        let test_voltage = 2.5;
        let analog = AnalogSignal { voltage: test_voltage };
        let digital = adc.convert(&analog);
        let back = dac.convert(&digital);
        assert!((back.voltage - test_voltage).abs() < 0.01, "roundtrip should be close");
    }

    #[test]
    fn test_rc_network_step() {
        let mut rc = RcNetwork::new(1000.0, 1e-6);
        // V_in = 5V, after many steps should approach 5V
        for _ in 0..10000 {
            rc.step(5.0, 1e-6);
        }
        assert!(
            rc.voltage() > 4.0,
            "RC should charge toward 5V, got {}",
            rc.voltage()
        );
    }

    #[test]
    fn test_rc_network_discharge() {
        let mut rc = RcNetwork::new(1000.0, 1e-6);
        rc.step(5.0, 0.1); // charge
        let charged = rc.voltage();
        assert!(charged > 0.0);
        // Discharge with V_in = 0
        for _ in 0..10000 {
            rc.step(0.0, 1e-6);
        }
        assert!(
            rc.voltage() < charged,
            "RC should discharge, got {} < {}",
            rc.voltage(),
            charged
        );
    }

    #[test]
    fn test_rc_default_values() {
        let rc = RcNetwork::default();
        assert_eq!(rc.r, 1000.0);
        assert_eq!(rc.c, 1e-6);
        assert_eq!(rc.v_out, 0.0);
    }

    #[test]
    fn test_mixed_signal_chain() {
        // Digital → DAC → Analog → RC → ADC → Digital
        let dac = Dac::new(3.3, 8);
        let mut rc = RcNetwork::new(1000.0, 1e-6);
        let adc = Adc::new(3.3, 8);

        let digital_in = LogicVec::from_u64(128, 8); // ~1.65V
        let analog = dac.convert(&digital_in);
        for _ in 0..5000 {
            rc.step(analog.voltage, 1e-6);
        }
        let digital_out = adc.convert(&AnalogSignal { voltage: rc.voltage() });
        assert!(
            (digital_out.to_u64() as i64 - 128i64).abs() < 10,
            "mixed-signal chain should preserve value roughly, got {}",
            digital_out.to_u64()
        );
    }
}
