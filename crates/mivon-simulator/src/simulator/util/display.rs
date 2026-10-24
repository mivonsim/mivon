//! Format `$display`/`$monitor`/`$sformatf` dan severity task.
//!
//! Inti formatter (`format_display_core`, `format_display_core_scoped`) plus
//! method engine yang memakainya (`sscanf_impl`, `format_severity_message`,
//! `emit_severity`). Dipisah dari `util.rs` supaya file induk fokus ke
//! utilitas sinyal/nilai.

use super::*;

#[cfg(test)]
pub(super) fn format_display_core(
    fmt_str: &str,
    value_args: impl Iterator<Item = (LogicVec, bool, bool)>,
    sim_time: u64,
    timeformat: &TimeFormat,
) -> String {
    format_display_core_scoped(fmt_str, value_args, sim_time, timeformat, "")
}

/// Seperti format_display_core, tetapi membawa scope (nama hierarki
/// instance aktif) untuk mendukung %m (dan %l). scope kosong bila
/// tidak tersedia.
pub(super) fn format_display_core_scoped(
    fmt_str: &str,
    value_args: impl Iterator<Item = (LogicVec, bool, bool)>,
    sim_time: u64,
    timeformat: &TimeFormat,
    scope: &str,
) -> String {
    let mut value_args = value_args;
    let mut result = String::with_capacity(fmt_str.len() + 8 * 16);
    let mut chars = fmt_str.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            let mut zero_fill = false;
            let mut width = 0usize;
            // `%.Nf` — presisi desimal utk `%f`/`%e`/`%g` (LRM 1800
            // §21.2.1.4). Default 6. `%0.Nf` = zero-fill + presisi N.
            let mut precision: Option<usize> = None;
            // Flag `-` = left-justify (`%-5d`, `%-10s`).
            let mut left_align = false;
            while let Some(&next) = chars.peek() {
                if next == '-' {
                    left_align = true;
                    chars.next();
                } else {
                    break;
                }
            }
            if chars.peek() == Some(&'0') {
                zero_fill = true;
                chars.next();
            }
            while let Some(&next) = chars.peek() {
                if next.is_ascii_digit() {
                    width = width * 10 + next.to_digit(10).unwrap() as usize;
                    chars.next();
                } else {
                    break;
                }
            }
            if chars.peek() == Some(&'.') {
                chars.next();
                let mut p = 0usize;
                while let Some(&next) = chars.peek() {
                    if next.is_ascii_digit() {
                        p = p * 10 + next.to_digit(10).unwrap() as usize;
                        chars.next();
                    } else {
                        break;
                    }
                }
                precision = Some(p);
            }
            let precision = precision.unwrap_or(6);
            // Simpan specifier terakhir agar arm gabungan (`'h' | 'x' |
            // 'X'`) bisa membedakan upper/lower.
            let spec = chars.next();
            let last_spec_upper = matches!(spec, Some('H') | Some('X'));
            match spec {
                Some('o') | Some('O') => {
                    if let Some((val, _, is_real)) = value_args.next() {
                        let val = fmt_arg_as_int(&val, is_real);
                        // `%o` octal — leading '0' HANYA dibuang bila
                        // zero-fill `%0o` (IEEE 1800 §21.2.1.3).
                        let vw = val.width;
                        let ndigits = vw.div_ceil(3);
                        let mut s = String::new();
                        let mut started = false;
                        for i in (0..ndigits).rev() {
                            let mut tri = 0u8;
                            let mut has_x = false;
                            let mut has_z = false;
                            for j in 0..3 {
                                let bi = i * 3 + j;
                                if bi >= vw {
                                    continue;
                                }
                                match val.bits[bi] {
                                    LogicVal::One => tri |= 1 << j,
                                    LogicVal::X => has_x = true,
                                    LogicVal::Z => has_z = true,
                                    LogicVal::Zero => {}
                                }
                            }
                            if !started && tri == 0 && !has_x && !has_z && i > 0 && zero_fill {
                                continue;
                            }
                            started = true;
                            if has_x {
                                // semua-X → 'x'; campuran dengan bit known
                                // → 'X' (konvensi iverilog/VCS).
                                let partial = (i * 3..i * 3 + 3).any(|bi| {
                                    val.bits
                                        .get(bi)
                                        .map(|b| *b != LogicVal::X && *b != LogicVal::Z)
                                        .unwrap_or(false)
                                });
                                s.push(if partial { 'X' } else { 'x' });
                            } else if has_z {
                                let partial = (i * 3..i * 3 + 3).any(|bi| {
                                    val.bits
                                        .get(bi)
                                        .map(|b| *b != LogicVal::X && *b != LogicVal::Z)
                                        .unwrap_or(false)
                                });
                                s.push(if partial { 'Z' } else { 'z' });
                            } else {
                                let tri_u32: u32 = tri as u32;
                                s.push(char::from_digit(tri_u32, 8).unwrap_or('0'));
                            }
                        }
                        if !started {
                            s.push('0'); // nilai "0"
                        }
                        if width > s.len() {
                            let pad = if zero_fill { '0' } else { ' ' };
                            for _ in 0..(width - s.len()) {
                                result.push(pad);
                            }
                        }
                        result.push_str(&s);
                    }
                }
                Some('d') | Some('D') | Some('i') | Some('I') => {
                    if let Some((val, is_signed, is_real)) = value_args.next() {
                        // Real → integer bulat (LRM §21.2.1.4).
                        let val = fmt_arg_as_int(&val, is_real);
                        let is_signed = is_signed || is_real;
                        // Default field width = lebar representasi maksimum
                        // tipe (LRM 1800 Tabel 21-3) — plain `%d` di-right-justify
                        // ke field itu (iverilog: `%d` dari `integer 1` →
                        // "          1"). Width eksplisit menang; `%0d` = minimal.
                        let width = if width > 0 {
                            width
                        } else if zero_fill {
                            0
                        } else {
                            default_dec_field_width(val.width, is_signed)
                        };
                        // DESIMAL utk nilai UNKNOWN = huruf, BUKAN angka 0
                        // (X→0 = silent coercion menyesatkan — fuzzer
                        // verify_bad_0039: `OUT_RST_BAD=<0>` padahal
                        // out=X). Golden iverilog: semua-X → `x`,
                        // semua-Z → `z`, partially-unknown → `X`;
                        // space-pad utk `%Nd` (bukan zero-pad).
                        let all_pred = |p: &LogicVal| matches!(p, LogicVal::X);
                        let all_z = val.bits.iter().all(|b| *b == LogicVal::Z);
                        let any_unknown = val
                            .bits
                            .iter()
                            .any(|b| matches!(b, LogicVal::X | LogicVal::Z));
                        let unknown_char = if any_unknown {
                            if all_z {
                                // semua-Z → 'z'; semua-X → 'x'; campuran →
                                // huruf kapital sesuai unknowns yang ada
                                // (konvensi iverilog/VCS: `x`/`z` untuk
                                // unknown seragam, `X`/`Z` untuk parsial).
                                Some('z')
                            } else if val.bits.iter().all(&all_pred) {
                                Some('x')
                            } else if val.bits.iter().any(|b| matches!(b, LogicVal::X)) {
                                Some('X')
                            } else {
                                Some('Z')
                            }
                        } else {
                            None
                        };
                        if let Some(ch) = unknown_char {
                            // Space-pad: `%4d` utk x → "   x"; `%0d` tanpa pad.
                            for _ in 1..width {
                                result.push(' ');
                            }
                            result.push(ch);
                        } else if is_signed && val.width <= 64 {
                            // Signed: cetak dua-complement sebagai negatif
                            // (mis. int -5 = 0xFFFFFFFB → "-5").
                            let n = val.to_i64();
                            let s = i64_digits_str(n);
                            push_padded(&mut result, &s, width, zero_fill, left_align);
                        } else {
                            let n = val.to_u64();
                            let s = u64_digits_str(n);
                            push_padded(&mut result, &s, width, zero_fill, left_align);
                        }
                    }
                }
                Some('b') | Some('B') | Some('z') | Some('Z') | Some('p') | Some('P') => {
                    if let Some((val, _, is_real)) = value_args.next() {
                        let val = fmt_arg_as_int(&val, is_real);
                        // `%0b` membuang leading '0'; `%b` plain mencetak
                        // FULL width nilai (IEEE 1800 §21.2.1.3).
                        let mut chars_out: Vec<char> = Vec::new();
                        if zero_fill {
                            let mut seen_nonzero = false;
                            for bit in val.bits.iter().rev() {
                                if *bit == LogicVal::Zero && !seen_nonzero {
                                    continue;
                                }
                                seen_nonzero = true;
                                chars_out.push(match bit {
                                    LogicVal::Zero => '0',
                                    LogicVal::One => '1',
                                    LogicVal::X => 'x',
                                    LogicVal::Z => 'z',
                                });
                            }
                            if !seen_nonzero {
                                chars_out.push('0'); // nilai "0"
                            }
                        } else if val.bits.is_empty() {
                            chars_out.push('0');
                        } else {
                            for bit in val.bits.iter().rev() {
                                chars_out.push(match bit {
                                    LogicVal::Zero => '0',
                                    LogicVal::One => '1',
                                    LogicVal::X => 'x',
                                    LogicVal::Z => 'z',
                                });
                            }
                        }
                        let out_len = chars_out.len();
                        if width > out_len {
                            let pad = if zero_fill { '0' } else { ' ' };
                            for _ in 0..(width - out_len) {
                                result.push(pad);
                            }
                        }
                        result.extend(chars_out);
                    }
                }
                Some('h') | Some('H') | Some('x') | Some('X') => {
                    // `%h`/`%x` = hex lower, `%H`/`%X` = hex upper
                    // (LRM 1800 §21.2.1.3).
                    let upper = last_spec_upper;
                    if let Some((val, _, is_real)) = value_args.next() {
                        // Real → integer bulat utk specifier integer (LRM §21.2.1.4).
                        let val = fmt_arg_as_int(&val, is_real);
                        // Format per-nibble dari pola bit — X/Z-aware.
                        // (Bug render: to_u64() memetakan X/Z → 0 sehingga
                        // `$display("%h", 8'hxx)` mencetak "0" — user
                        // debugging propagasi X melihat nilai seolah known.
                        // Kini: nibble ber-X → 'x', ber-Z (tanpa X) → 'z',
                        // else digit hex. Berlaku juga >64-bit.)
                        let vw = val.width;
                        let ndigits = vw.div_ceil(4);
                        let mut s = String::new();
                        let mut started = false;
                        for i in (0..ndigits).rev() {
                            let mut nib = 0u8;
                            let mut has_x = false;
                            let mut has_z = false;
                            for j in 0..4 {
                                let bi = i * 4 + j;
                                if bi >= vw {
                                    continue;
                                }
                                match val.bits[bi] {
                                    LogicVal::One => nib |= 1 << j,
                                    LogicVal::X => has_x = true,
                                    LogicVal::Z => has_z = true,
                                    LogicVal::Zero => {}
                                }
                            }
                            // Skip leading zero nibble HANYA saat `%0h` trims
                            // (IEEE 1800 §21.2.1.3) — plain `%h` mencetak
                            // full width (iverilog differential: 8'h05 → "05").
                            if zero_fill && !started && nib == 0 && !has_x && !has_z && i > 0 {
                                continue;
                            }
                            started = true;
                            if has_x {
                                // semua-X → 'x'; campuran X+known → 'X'
                                // (konvensi iverilog/VCS, LRM §21.2.1.3).
                                let partial = val.bits.iter().enumerate().any(|(bi, b)| {
                                    (i * 4..i * 4 + 4).contains(&bi)
                                        && *b != LogicVal::X
                                        && *b != LogicVal::Z
                                });
                                s.push(if partial || upper { 'X' } else { 'x' });
                            } else if has_z {
                                let partial = val.bits.iter().enumerate().any(|(bi, b)| {
                                    (i * 4..i * 4 + 4).contains(&bi)
                                        && *b != LogicVal::X
                                        && *b != LogicVal::Z
                                });
                                s.push(if partial || upper { 'Z' } else { 'z' });
                            } else {
                                let digit = char::from_digit(nib as u32, 16).unwrap_or('0');
                                s.push(if upper {
                                    digit.to_ascii_uppercase()
                                } else {
                                    digit
                                });
                            }
                        }
                        if !started {
                            s.push('0');
                        }
                        push_padded(&mut result, &s, width, zero_fill, left_align);
                    }
                }
                Some('f') | Some('F') => {
                    if let Some((val, _, is_real)) = value_args.next() {
                        // `%f` default precision 6 (IEEE 1800 §21.2.1.4);
                        // `%.Nf`/`%0.Nf` → N digit presisi.
                        let prec = precision.min(20);
                        let s = format!("{:.*}", prec, fmt_arg_as_real(&val, is_real, false));
                        push_padded(&mut result, &s, width, zero_fill, left_align);
                    }
                }
                Some('g') | Some('G') => {
                    // `%g` — representasi pendek (C printf): ekspon bila
                    // eksponen di luar [-4, presisi), else desimal dengan
                    // trailing nol dibuang (LRM 1800 §21.2.1.4).
                    if let Some((val, _, is_real)) = value_args.next() {
                        let v = fmt_arg_as_real(&val, is_real, false);
                        let prec = precision.min(20);
                        let exp = if v == 0.0 {
                            0
                        } else {
                            v.abs().log10().floor() as i32
                        };
                        let s = if exp < -4 || exp >= prec as i32 {
                            normalize_exp(&format!("{:.*e}", prec.saturating_sub(1), v))
                        } else {
                            let decimals = (prec as i32 - 1 - exp).max(0) as usize;
                            let mut s = format!("{:.*}", decimals, v);
                            if s.contains('.') {
                                while s.ends_with('0') {
                                    s.pop();
                                }
                                if s.ends_with('.') {
                                    s.pop();
                                }
                            }
                            s
                        };
                        let s = if matches!(spec, Some('G')) {
                            s.to_uppercase()
                        } else {
                            s
                        };
                        push_padded(&mut result, &s, width, zero_fill, left_align);
                    }
                }
                Some('e') | Some('E') => {
                    if let Some((val, _, is_real)) = value_args.next() {
                        let prec = precision.min(20);
                        let s = normalize_exp(&format!(
                            "{:.*e}",
                            prec,
                            fmt_arg_as_real(&val, is_real, false)
                        ));
                        let s = if matches!(spec, Some('E')) {
                            s.to_uppercase()
                        } else {
                            s
                        };
                        push_padded(&mut result, &s, width, zero_fill, left_align);
                    }
                }
                Some('t') | Some('T') => {
                    // %t: format time using $timeformat settings (IEEE 1800).
                    // Sim time advances 1 unit per step; base unit seeded from
                    // design `timescale (default 1ns = 10^-9 s).
                    let t = value_args
                        .next()
                        .map(|(v, _, _)| v.to_u64() as f64)
                        .unwrap_or(sim_time as f64);
                    // Skala relatif terhadap basis sim-time, bukan hardcode -9.
                    // saturating_sub mencegah underflow i64 (panic di debug)
                    // jika user memanggil $timeformat dengan units ekstrem.
                    let scale =
                        10f64.powi(timeformat.base_units.saturating_sub(timeformat.units) as i32);
                    let scaled = t * scale;
                    let precision = timeformat.precision.clamp(0, 20) as usize;
                    let mut s = format!("{:.*}", precision, scaled);
                    // Clamp min_field_width utk cegah alokasi " ".repeat(huge).
                    let min_width = timeformat.min_field_width.min(128);
                    if s.len() < min_width {
                        s = format!("{}{}", " ".repeat(min_width - s.len()), s);
                    }
                    s.push_str(&timeformat.suffix);
                    push_padded(&mut result, &s, width, zero_fill, left_align);
                }
                Some('s') | Some('S') => {
                    if let Some((val, _, is_real)) = value_args.next() {
                        let val = fmt_arg_as_int(&val, is_real);
                        let s = logicvec_to_string(&val);
                        push_padded(&mut result, &s, width, zero_fill, left_align);
                    }
                }
                Some('c') | Some('C') => {
                    // `%c` — karakter dari 8 bit pertama nilai (LRM
                    // 1800 §21.2.1.2). Nilai 0 → NUL (tidak dicetak).
                    if let Some((val, _, is_real)) = value_args.next() {
                        let val = fmt_arg_as_int(&val, is_real);
                        let code = val.to_u64() as u32;
                        if let Some(ch) = char::from_u32(code) {
                            push_padded(&mut result, &ch.to_string(), width, zero_fill, left_align);
                        }
                    }
                }
                Some('u') | Some('U') => {
                    // %u — 2-state unformatted (binary, x/z -> 0).
                    if let Some((val, _, is_real)) = value_args.next() {
                        let val = fmt_arg_as_int(&val, is_real);
                        let mut chars_out: Vec<char> = Vec::new();
                        if zero_fill {
                            let mut seen_nonzero = false;
                            for bit in val.bits.iter().rev() {
                                if matches!(bit, LogicVal::Zero) && !seen_nonzero {
                                    continue;
                                }
                                seen_nonzero = true;
                                chars_out.push(if matches!(bit, LogicVal::One) {
                                    '1'
                                } else {
                                    '0'
                                });
                            }
                            if !seen_nonzero {
                                chars_out.push('0');
                            }
                        } else if val.bits.is_empty() {
                            chars_out.push('0');
                        } else {
                            for bit in val.bits.iter().rev() {
                                chars_out.push(if matches!(bit, LogicVal::One) {
                                    '1'
                                } else {
                                    '0'
                                });
                            }
                        }
                        let out_len = chars_out.len();
                        if width > out_len {
                            let pad = if zero_fill { '0' } else { ' ' };
                            for _ in 0..(width - out_len) {
                                result.push(pad);
                            }
                        }
                        result.extend(chars_out);
                    }
                }
                Some('m') | Some('M') | Some('l') | Some('L') => {
                    // %m/%l — nama hierarki scope aktif (library ~ scope).
                    push_padded(&mut result, scope, width, zero_fill, left_align);
                }
                Some('v') | Some('V') => {
                    // %v — strength; mivon belum melacak strength per-net,
                    // gunakan default Strong dan konsumsi argumennya.
                    if value_args.next().is_some() {
                        push_padded(&mut result, "Strong", width, zero_fill, left_align);
                    }
                }
                Some('%') => {
                    // %% — literal persen (LRM 1800 §21.2.1.2).
                    result.push('%');
                }
                Some(c2) => {
                    result.push('%');
                    if zero_fill {
                        result.push('0');
                    }
                    if width > 0 {
                        let _ = write!(result, "{}", width);
                    }
                    result.push(c2);
                }
                None => {
                    result.push('%');
                }
            }
        } else if c == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some(c2) => {
                    result.push('\\');
                    result.push(c2);
                }
                None => result.push('\\'),
            }
        } else {
            result.push(c);
        }
    }
    result
}

impl SimulationEngine {
    /// Format pesan severity task ($info/$warning/$error/$fatal): argumen
    /// pertama yang berupa konstanta kecil (finish_number 0–2 per LRM §20.2,
    /// mis. `$fatal(1, "msg")`) di-skip — finish number bukan bagian pesan.
    /// Sisanya diformat persis seperti $display.
    /// Parse ala $sscanf: tulis ke argumen signal, kembalikan jumlah
    /// assignment yang berhasil. Dipakai jalur IR (statement) & AST/ekspresi.
    pub(crate) fn sscanf_impl(&mut self, ir_args: &[IrExpr]) -> usize {
        let input_str = match ir_args.first() {
            Some(IrExpr::String(s)) => s.clone(),
            Some(a) => match self.evaluate_expr(a) {
                Ok(v) => logicvec_to_string(&v),
                Err(_) => return 0,
            },
            None => return 0,
        };
        let fmt = match ir_args.get(1) {
            Some(IrExpr::String(s)) => s.clone(),
            Some(a) => match self.evaluate_expr(a) {
                Ok(v) => logicvec_to_string(&v),
                Err(_) => return 0,
            },
            None => return 0,
        };
        let tokens: Vec<&str> = input_str.split_whitespace().collect();
        let mut ti = 0usize;
        let mut ai = 0usize;
        let mut count = 0usize;
        let mut chars = fmt.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '%' {
                continue;
            }
            let mut spec = None;
            for ch in chars.by_ref() {
                if ch.is_ascii_digit() {
                    continue;
                }
                spec = Some(ch);
                break;
            }
            let spec = match spec {
                Some(s) => s,
                None => break,
            };
            if spec == '%' {
                continue;
            }
            let tok = match tokens.get(ti) {
                Some(t) => *t,
                None => break,
            };
            match spec {
                'd' | 'i' | 'h' | 'x' | 'X' | 'o' | 'b' => {
                    let radix = match spec {
                        'h' | 'x' | 'X' => 16,
                        'o' => 8,
                        'b' => 2,
                        _ => 10,
                    };
                    let cleaned = tok.trim_start_matches("0x").trim_start_matches("0X");
                    if let Ok(val) = i64::from_str_radix(cleaned, radix) {
                        if let Some(IrExpr::Signal(sid, _)) = ir_args.get(2 + ai) {
                            let w = self
                                .design
                                .top
                                .signals
                                .get(*sid)
                                .map(|s| s.width.max(1))
                                .unwrap_or(32);
                            self.state
                                .write_signal(*sid, LogicVec::from_u64(val as u64, w));
                            count += 1;
                        }
                        ai += 1;
                    }
                    ti += 1;
                }
                's' => {
                    if let Some(IrExpr::Signal(sid, _)) = ir_args.get(2 + ai) {
                        let s = tokens[ti..].join(" ");
                        self.state.write_signal(*sid, string_to_logicvec(&s));
                        count += 1;
                    }
                    break;
                }
                'c' => {
                    if let Some(IrExpr::Signal(sid, _)) = ir_args.get(2 + ai) {
                        let b = tok.as_bytes().first().copied().unwrap_or(0);
                        self.state
                            .write_signal(*sid, LogicVec::from_u64(b as u64, 8));
                        count += 1;
                    }
                    ti += 1;
                    ai += 1;
                }
                'f' | 'e' | 'g' => {
                    if let Ok(f) = tok.parse::<f64>() {
                        if let Some(IrExpr::Signal(sid, _)) = ir_args.get(2 + ai) {
                            self.state
                                .write_signal(*sid, LogicVec::from_u64(f.to_bits(), 64));
                            count += 1;
                        }
                        ai += 1;
                    }
                    ti += 1;
                }
                _ => {}
            }
        }
        count
    }

    pub(crate) fn format_severity_message(&mut self, ir_args: &[IrExpr]) -> String {
        let args = match ir_args.first() {
            Some(IrExpr::Const(v)) if v.to_u64() <= 2 => &ir_args[1..],
            _ => ir_args,
        };
        self.format_display(args)
    }

    /// Emit severity system task (F14/F15): cetak pesan, increment counter,
    /// dan untuk `$fatal` set `fatal_hit` + `running=false` (hentikan sim
    /// seketika). Dipakai jalur IR (`evaluate_lang_syscall`) & jalur AST
    /// (`handle_ast_syscall`) agar counter & perilaku fatal tidak drift.
    pub(crate) fn emit_severity(&mut self, name: &str, msg: &str) {
        // F20: lampirkan lokasi source (file:line:col) bila tersedia agar
        // $warning/$error/$fatal selalu menunjuk ke baris pemanggil.
        let loc = self.cur_src_loc_str();
        let suffix = loc.map(|l| format!(" (at {})", l)).unwrap_or_default();
        match name {
            "info" => {
                println!("Info: {}{}", msg, suffix);
                self.sev_info_count += 1;
            }
            "warning" => {
                eprintln!("Warning: {}{}", msg, suffix);
                self.sev_warning_count += 1;
            }
            "error" => {
                eprintln!("Error: {}{}", msg, suffix);
                self.sev_error_count += 1;
            }
            _ => {
                eprintln!("Fatal: {}{}", msg, suffix);
                self.sev_fatal_count += 1;
                self.fatal_hit = true;
                self.running = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Kontrak wrapper: `format_display_core` HARUS sama dengan
    /// `format_display_core_scoped` ber-scope kosong. Kalau salah satu diubah
    /// sendirian, output `$display`/`$monitor` (non-scope) dan `%m` (scoped)
    /// bisa melenceng tanpa ketahuan test format lain.
    #[test]
    fn core_matches_scoped_with_empty_scope() {
        let tf = TimeFormat {
            units: -9,
            precision: 0,
            suffix: "ns".to_string(),
            min_field_width: 0,
            base_units: -9,
        };
        // Tiga nilai: %0d, %h, %b masing-masing mengonsumsi satu argumen.
        let args = || {
            vec![
                (LogicVec::from_u64(42, 32), false, false),
                (LogicVec::from_u64(42, 32), false, false),
                (LogicVec::from_u64(42, 32), false, false),
            ]
            .into_iter()
        };
        let fmt = "%0d|%h|%b|%m";
        let non_scoped = format_display_core(fmt, args(), 7, &tf);
        assert_eq!(
            non_scoped,
            format_display_core_scoped(fmt, args(), 7, &tf, "")
        );
        // Nilai literal (width 32 → %h/%b dipad penuh), %m kosong tanpa scope.
        assert_eq!(non_scoped, "42|0000002a|00000000000000000000000000101010|");
    }
}
