use crate::simulator::engine::SimulationEngine;
use mivon_ast::*;
use mivon_core::diagnostics::DiagCode;
use mivon_core::error::SimError;
use mivon_ir::*;
use std::fmt::Write as _;

pub fn map_ast_binary_op(op: &BinaryOp) -> Result<BinaryIrOp, String> {
    match op {
        BinaryOp::Add => Ok(BinaryIrOp::Add),
        BinaryOp::Sub => Ok(BinaryIrOp::Sub),
        BinaryOp::Mul => Ok(BinaryIrOp::Mul),
        BinaryOp::Div => Ok(BinaryIrOp::Div),
        BinaryOp::Mod => Ok(BinaryIrOp::Mod),
        BinaryOp::Power => Ok(BinaryIrOp::Power),
        BinaryOp::Eq => Ok(BinaryIrOp::Eq),
        BinaryOp::Neq => Ok(BinaryIrOp::Neq),
        BinaryOp::CaseEq => Ok(BinaryIrOp::CaseEq),
        BinaryOp::CaseNeq => Ok(BinaryIrOp::CaseNeq),
        BinaryOp::EqWild => Ok(BinaryIrOp::Eq),
        BinaryOp::NeqWild => Ok(BinaryIrOp::Neq),
        BinaryOp::Lt => Ok(BinaryIrOp::Lt),
        BinaryOp::Le => Ok(BinaryIrOp::Le),
        BinaryOp::Gt => Ok(BinaryIrOp::Gt),
        BinaryOp::Ge => Ok(BinaryIrOp::Ge),
        BinaryOp::BitAnd => Ok(BinaryIrOp::BitAnd),
        BinaryOp::BitOr => Ok(BinaryIrOp::BitOr),
        BinaryOp::BitXor => Ok(BinaryIrOp::BitXor),
        BinaryOp::BitXnor => Ok(BinaryIrOp::BitXnor),
        BinaryOp::Shl => Ok(BinaryIrOp::Shl),
        BinaryOp::Shr => Ok(BinaryIrOp::Shr),
        BinaryOp::Sshl => Ok(BinaryIrOp::Sshl),
        BinaryOp::Sshr => Ok(BinaryIrOp::Sshr),
        BinaryOp::LogicalAnd => Ok(BinaryIrOp::LogicalAnd),
        BinaryOp::LogicalOr => Ok(BinaryIrOp::LogicalOr),
    }
}

pub fn map_ast_unary_op(op: &UnaryOp) -> Result<UnaryIrOp, String> {
    match op {
        UnaryOp::Plus => Ok(UnaryIrOp::Plus),
        UnaryOp::Minus => Ok(UnaryIrOp::Minus),
        UnaryOp::BitNot => Ok(UnaryIrOp::BitNot),
        UnaryOp::Not => Ok(UnaryIrOp::Not),
        UnaryOp::ReductionAnd => Ok(UnaryIrOp::RedAnd),
        UnaryOp::ReductionNand => Ok(UnaryIrOp::RedNand),
        UnaryOp::ReductionOr => Ok(UnaryIrOp::RedOr),
        UnaryOp::ReductionNor => Ok(UnaryIrOp::RedNor),
        UnaryOp::ReductionXor => Ok(UnaryIrOp::RedXor),
        UnaryOp::ReductionXnor => Ok(UnaryIrOp::RedXnor),
    }
}

pub fn extract_signal_deps(expr: &IrExpr) -> Vec<SignalId> {
    let mut deps = Vec::new();
    extract_signal_deps_inner(expr, &mut deps);
    deps
}

pub fn extract_signal_deps_inner(expr: &IrExpr, deps: &mut Vec<SignalId>) {
    match expr {
        IrExpr::Signal(id, _) => {
            if !deps.contains(id) {
                deps.push(*id);
            }
        }
        IrExpr::RangeSelect(id, _, _)
        | IrExpr::BitSelect(id, _)
        | IrExpr::ArrayIndex { sig_id: id, .. } => {
            if !deps.contains(id) {
                deps.push(*id);
            }
        }
        IrExpr::ExprRangeSelect(e, _, _) | IrExpr::ExprBitSelect(e, _) => {
            extract_signal_deps_inner(e, deps);
        }
        IrExpr::ExprPartSelect(e1, e2, e3) => {
            extract_signal_deps_inner(e1, deps);
            extract_signal_deps_inner(e2, deps);
            extract_signal_deps_inner(e3, deps);
        }
        IrExpr::Concat(exprs) => {
            for e in exprs {
                extract_signal_deps_inner(e, deps);
            }
        }
        IrExpr::Replicate(_, e) => {
            extract_signal_deps_inner(e, deps);
        }
        IrExpr::UnaryOp(_, e) => {
            extract_signal_deps_inner(e, deps);
        }
        IrExpr::BinaryOp(_, e1, e2) => {
            extract_signal_deps_inner(e1, deps);
            extract_signal_deps_inner(e2, deps);
        }
        IrExpr::Cond(c, t, e) => {
            extract_signal_deps_inner(c, deps);
            extract_signal_deps_inner(t, deps);
            extract_signal_deps_inner(e, deps);
        }
        IrExpr::Signed(e) => {
            extract_signal_deps_inner(e, deps);
        }
        IrExpr::MethodCall { obj, args, .. } => {
            extract_signal_deps_inner(obj, deps);
            for a in args {
                extract_signal_deps_inner(a, deps);
            }
        }
        IrExpr::MemberAccess { obj, .. } => {
            extract_signal_deps_inner(obj, deps);
        }
        IrExpr::NewCall { args, .. } => {
            for a in args {
                extract_signal_deps_inner(a, deps);
            }
        }
        IrExpr::SysFunc { args, .. } => {
            for a in args {
                extract_signal_deps_inner(a, deps);
            }
        }
        IrExpr::DpiCall { args, .. } => {
            for a in args {
                extract_signal_deps_inner(a, deps);
            }
        }
        IrExpr::HierRef(_) => {}
        IrExpr::Inside { expr, list } => {
            extract_signal_deps_inner(expr, deps);
            for item in list {
                extract_signal_deps_inner(item, deps);
            }
        }
        IrExpr::InsideRange { expr, lo, hi } => {
            extract_signal_deps_inner(expr, deps);
            extract_signal_deps_inner(lo, deps);
            extract_signal_deps_inner(hi, deps);
        }
        IrExpr::Cast { expr, .. } => {
            extract_signal_deps_inner(expr, deps);
        }
        IrExpr::Dist { expr, .. } => {
            extract_signal_deps_inner(expr, deps);
        }
        IrExpr::StreamingConcat { slices, .. } => {
            for e in slices {
                extract_signal_deps_inner(e, deps);
            }
        }
        IrExpr::UdpLookup { args, .. } => {
            for a in args {
                extract_signal_deps_inner(a, deps);
            }
        }
        IrExpr::VifBinding { .. } => {}
        IrExpr::VirtualIfaceAccess { .. } => {}
        IrExpr::FuncCall { args, .. } => {
            for a in args {
                extract_signal_deps_inner(a, deps);
            }
        }
        IrExpr::Const(_) | IrExpr::FillLit(_) | IrExpr::String(_) | IrExpr::This => {}
    }
}

pub fn is_signed_expr(expr: &IrExpr, signals: &[SignalInfo]) -> bool {
    match expr {
        IrExpr::Signed(_) => true,
        // Cast wrapper (konteks aritmetik me-resize literal/sinyal signed ke
        // lebar umum via `Cast`) — signedness berasal dari sinyal DALAM
        // (bug #8: `a >>> 1` gagal aritmetik krn lhs = Cast{32, Signal}).
        IrExpr::Cast { expr: inner, .. } => is_signed_expr(inner, signals),
        IrExpr::Signal(id, _) | IrExpr::BitSelect(id, _) | IrExpr::RangeSelect(id, ..) => {
            signals.get(*id).map(|s| s.is_signed).unwrap_or(false)
        }
        IrExpr::ArrayIndex { sig_id, .. } => {
            signals.get(*sig_id).map(|s| s.is_signed).unwrap_or(false)
        }
        // ROUND 36: signedness PROPAGASI untuk ekspresi majemuk. Keputusan
        // operasi di evaluator memakai `&&` (LRM §11.8.2: 'ada operand
        // unsigned → hasil unsigned' → operasi signed hanya bila KEDUA
        // operand signed). Literal desimal unsized (`5`) kini di-emit
        // elaborator sebagai IrExpr::Signed (LRM §6.8.1) agar `a < 0` /
        // `a / 2` tetap signed sedangkan `a < 8'hFF` unsigned.
        IrExpr::BinaryOp(op, lhs, rhs) => {
            // LRM §11.8.1 Tabel 11-21: hasil operator perbandingan & logical
            // SELALU unsigned (1-bit), apa pun signedness operandnya. Tanpa
            // pengecualian ini `(-a <= b) >>> x` menandai lhs shift sbg
            // signed → 1'b1 di-sign-extend jadi all-ones (ditemukan fuzzer
            // signed_fuzz seed=1/30; emas + Icarus: zero-extend).
            if matches!(
                op,
                BinaryIrOp::Eq
                    | BinaryIrOp::Neq
                    | BinaryIrOp::CaseEq
                    | BinaryIrOp::CaseNeq
                    | BinaryIrOp::EqWild
                    | BinaryIrOp::NeqWild
                    | BinaryIrOp::Lt
                    | BinaryIrOp::Le
                    | BinaryIrOp::Gt
                    | BinaryIrOp::Ge
                    | BinaryIrOp::LogicalAnd
                    | BinaryIrOp::LogicalOr
            ) {
                return false;
            }
            // LRM §11.8.2 Tabel 11-21: hasil SHIFT mengikuti signedness
            // OPERAN KIRI saja (rhs self-determined, tidak berpengaruh).
            // Dulu `l && r` — `signed_neg >> (unsigned_cmp)` salah jadi
            // unsigned sehingga induk comparison jalan tanpa tanda
            // (ditemukan fuzzer signed_fuzz seed=111; emas + Icarus:
            // signed).
            if matches!(
                op,
                BinaryIrOp::Shl | BinaryIrOp::Shr | BinaryIrOp::Sshl | BinaryIrOp::Sshr
            ) {
                return is_signed_expr(lhs, signals);
            }
            is_signed_expr(lhs, signals) && is_signed_expr(rhs, signals)
        }
        IrExpr::UnaryOp(op, inner) => {
            match op {
                // Unary minus on unsigned stays unsigned (SV: -unsigned = unsigned)
                // Only signed if inner is already signed
                UnaryIrOp::Minus => is_signed_expr(inner, signals),
                // Other unary ops propagate signedness
                _ => is_signed_expr(inner, signals),
            }
        }
        IrExpr::Cond(_, t, f) => is_signed_expr(t, signals) || is_signed_expr(f, signals),
        IrExpr::ExprRangeSelect(inner, ..) | IrExpr::ExprBitSelect(inner, ..) => {
            is_signed_expr(inner, signals)
        }
        // Concat/Replicate menghasilkan nilai unsigned (LRM §11.8.1); Cast
        // lebar, MemberAccess, Inside, SysFunc, dst. → unsigned (konservatif).
        _ => false,
    }
}

// ─── Display formatting ────────────────────────────────────────────────────

pub fn logicvec_to_string(lv: &LogicVec) -> String {
    let mut s = String::new();
    let mut i = 0;
    // F19: berhenti di byte NUL pertama (C-style) — string_to_logicvec
    // menambahkan null terminator (8 bit 0) di akhir; tanpa pemotongan ini
    // path instance UVM jadi `uvm_test_top\u0000.agent` dan config_db
    // wildcard matching gagal karena suffix `\0` ikut dibandingkan.
    while i + 7 < lv.width {
        let mut byte = 0u8;
        for j in 0..8 {
            if lv.bits[i + j] == LogicVal::One {
                byte |= 1 << j;
            }
        }
        if byte == 0 {
            break;
        }
        s.push(byte as char);
        i += 8;
    }
    // Remaining bits (last partial byte)
    if i < lv.width {
        let mut byte = 0u8;
        for j in 0..(lv.width - i) {
            if lv.bits[i + j] == LogicVal::One {
                byte |= 1 << j;
            }
        }
        if byte != 0 {
            s.push(byte as char);
        }
    }
    s
}

impl SimulationEngine {
    /// Format `$display`/`$monitor`/`$sformatf` arguments menjadi string.
    ///
    /// Argumen nilai dievaluasi via `evaluate_expr` penuh — sehingga ekspresi
    /// kompleks (cast, binary op, concat, dll.) di argumen `$display` tidak lagi
    /// jatuh ke fallback 0. Format string (jika arg pertama `IrExpr::String`)
    /// diproses per spec: `%0d/%b/%h/%t/%s`, `\n`, `\t`, dll.
    pub(crate) fn format_display(&mut self, ir_args: &[IrExpr]) -> String {
        let (fmt_str, start_idx) = if let Some(IrExpr::String(s)) = ir_args.first() {
            (s.as_str(), 1)
        } else {
            // Tanpa fmt string: tulis langsung ke satu String (no Vec per call).
            let mut out = String::with_capacity(ir_args.len() * 8);
            let mut first = true;
            for arg in ir_args {
                if let Ok(val) = self.evaluate_expr(arg) {
                    if !first {
                        out.push(' ');
                    }
                    first = false;
                    let _ = write!(out, "{}", val);
                }
            }
            return out;
        };

        // Evaluasi arg secara eager ke Vec: borrow &mut self (dari evaluate_expr)
        // harus berakhir sebelum akses self.state di bawah. `evaluate_expr` penuh
        // menangani semua IrExpr (Cast, BinaryOp, Concat, MemberAccess, ...).
        // Signedness per-arg ikut dibawa agar `%d` mencetak negatif untuk
        // ekspresi signed (`int a = -5` → "-5", bukan "4294967291").
        let value_args: Vec<(LogicVec, bool)> = ir_args[start_idx..]
            .iter()
            .filter_map(|a| {
                let signed = is_signed_expr(a, &self.design.top.signals);
                self.evaluate_expr(a).ok().map(|v| (v, signed))
            })
            .collect();
        self.format_display_fmt(fmt_str, value_args.into_iter())
    }

    /// F17: format `$display`/`$sformatf` di jalur AST (body method class) —
    /// argumen dievaluasi via `evaluate_ast_expr` (field class `it.addr` dkk
    /// ter-resolve via current_this), lalu format spec identik dengan jalur IR.
    pub(crate) fn format_display_ast(&mut self, ast_args: &[mivon_ast::Expr]) -> String {
        let (fmt_str, start_idx) = if let Some(mivon_ast::Expr::String(s)) = ast_args.first() {
            (s.as_str(), 1)
        } else {
            let mut out = String::with_capacity(ast_args.len() * 8);
            let mut first = true;
            for arg in ast_args {
                if let Ok(val) = self.evaluate_ast_expr(arg) {
                    if !first {
                        out.push(' ');
                    }
                    first = false;
                    let _ = write!(out, "{}", val);
                }
            }
            return out;
        };
        let value_args: Vec<(LogicVec, bool)> = ast_args[start_idx..]
            .iter()
            .filter_map(|a| {
                let signed = ast_expr_is_signed(a);
                self.evaluate_ast_expr(a).ok().map(|v| (v, signed))
            })
            .collect();
        self.format_display_fmt(fmt_str, value_args.into_iter())
    }

    /// Inti formatter `%d/%b/%h/%s/...` — dipakai jalur IR & AST (F17).
    /// Setiap arg adalah `(LogicVec, is_signed)`; `%d` memakai signedness
    /// untuk mencetak nilai negatif (jalur IR: dari ekspresi; jalur AST: dari
    /// `-<literal>` — lihat `ast_expr_is_signed`).
    fn format_display_fmt(
        &mut self,
        fmt_str: &str,
        value_args: impl Iterator<Item = (LogicVec, bool)>,
    ) -> String {
        let mut value_args = value_args;
        let mut result = String::with_capacity(fmt_str.len() + 8 * 16);
        let mut chars = fmt_str.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '%' {
                let mut zero_fill = false;
                let mut width = 0usize;
                let precision = 6usize;
                // (precision default 6 IEEE 1800 §21.2.1.4; parsing `%.Nf` belum diimplementasi)
                if let Some(&next) = chars.peek() {
                    if next == '0' {
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
                }
                match chars.next() {
                    Some('o') => {
                        if let Some((val, _)) = value_args.next() {
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
                                    s.push('x');
                                } else if has_z {
                                    s.push('z');
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
                    Some('d') => {
                        if let Some((val, is_signed)) = value_args.next() {
                            // DESIMAL utk nilai UNKNOWN = huruf, BUKAN angka 0
                            // (X→0 = silent coercion menyesatkan — fuzzer
                            // verify_bad_0039: `OUT_RST_BAD=<0>` padahal
                            // out=X). Golden iverilog: semua-X → `x`,
                            // semua-Z → `z`, partially-unknown → `X`;
                            // space-pad utk `%Nd` (bukan zero-pad).
                            let all_pred = |p: &LogicVal| matches!(p, LogicVal::X);
                            let all_z = val.bits.iter().all(|b| *b == LogicVal::Z);
                            let any_unknown = val.bits.iter().any(|b| {
                                matches!(b, LogicVal::X | LogicVal::Z)
                            });
                            let unknown_char = if any_unknown {
                                if all_z {
                                    Some('z')
                                } else if val.bits.iter().all(&all_pred) {
                                    Some('x')
                                } else {
                                    Some('X')
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
                                let ndigits = i64_digits(n);
                                if width > ndigits {
                                    let pad = if zero_fill { '0' } else { ' ' };
                                    for _ in 0..(width - ndigits) {
                                        result.push(pad);
                                    }
                                }
                                let _ = write!(result, "{}", n);
                            } else {
                                let n = val.to_u64();
                                let ndigits = u64_digits(n);
                                if width > ndigits {
                                    let pad = if zero_fill { '0' } else { ' ' };
                                    for _ in 0..(width - ndigits) {
                                        result.push(pad);
                                    }
                                }
                                let _ = write!(result, "{}", n);
                            }
                        }
                    }
                    Some('b') => {
                        if let Some((val, _)) = value_args.next() {
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
                    Some('h') => {
                        if let Some((val, _)) = value_args.next() {
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
                                    s.push('x');
                                } else if has_z {
                                    s.push('z');
                                } else {
                                    s.push(char::from_digit(nib as u32, 16).unwrap_or('0'));
                                }
                            }
                            if !started {
                                s.push('0');
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
                    Some('f') => {
                        if let Some((val, _)) = value_args.next() {
                            // `%f` default precision 6 (IEEE 1800 §21.2.1.4);
                            // `%.Nf`/`%0.Nf` → N digit presisi.
                            let prec = precision.min(20);
                            let _ = write!(
                                result,
                                "{:.data$}",
                                f64::from_bits(val.to_u64()),
                                data = prec
                            );
                        }
                    }
                    Some('e') | Some('E') => {
                        if let Some((val, _)) = value_args.next() {
                            let prec = precision.min(20);
                            let _ = write!(
                                result,
                                "{:.data$e}",
                                f64::from_bits(val.to_u64()),
                                data = prec
                            );
                        }
                    }
                    Some('t') => {
                        // %t: format time using $timeformat settings (IEEE 1800).
                        // Sim time advances 1 unit per step; base unit seeded from
                        // design `timescale (default 1ns = 10^-9 s).
                        let t = value_args
                            .next()
                            .map(|(v, _)| v.to_u64() as f64)
                            .unwrap_or(self.state.time as f64);
                        // Skala relatif terhadap basis sim-time, bukan hardcode -9.
                        // saturating_sub mencegah underflow i64 (panic di debug)
                        // jika user memanggil $timeformat dengan units ekstrem.
                        let scale = 10f64.powi(
                            self.state
                                .timeformat
                                .base_units
                                .saturating_sub(self.state.timeformat.units)
                                as i32,
                        );
                        let scaled = t * scale;
                        let precision = self.state.timeformat.precision.clamp(0, 20) as usize;
                        let mut s = format!("{:.*}", precision, scaled);
                        // Clamp min_field_width utk cegah alokasi " ".repeat(huge).
                        let min_width = self.state.timeformat.min_field_width.min(128);
                        if s.len() < min_width {
                            s = format!("{}{}", " ".repeat(min_width - s.len()), s);
                        }
                        s.push_str(&self.state.timeformat.suffix);
                        result.push_str(&s);
                    }
                    Some('s') => {
                        if let Some((val, _)) = value_args.next() {
                            result.push_str(&logicvec_to_string(&val));
                        }
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

    /// Format pesan severity task ($info/$warning/$error/$fatal): argumen
    /// pertama yang berupa konstanta kecil (finish_number 0–2 per LRM §20.2,
    /// mis. `$fatal(1, "msg")`) di-skip — finish number bukan bagian pesan.
    /// Sisanya diformat persis seperti $display.
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

/// Signedness ekspresi di jalur AST (body method class): hanya
/// `-<literal desimal>` (unary minus pada unsized integer literal = signed
/// 32-bit, IEEE 1800 §6.8.1) yang dapat dipastikan signed tanpa info tipe
/// field/local. Kasus lain dianggap unsigned (field class tidak menyimpan
/// signedness — keterbatasan dicatat).
fn ast_expr_is_signed(expr: &Expr) -> bool {
    match expr {
        Expr::UnaryOp {
            op: UnaryOp::Minus,
            expr: inner,
        } => matches!(inner.as_ref(), Expr::Value(Value::Decimal(_))),
        _ => false,
    }
}

/// Jumlah karakter `%d` signed: digit abs + 1 untuk tanda '-' (0 → 1).
fn i64_digits(n: i64) -> usize {
    if n < 0 {
        u64_digits(n.unsigned_abs()) + 1
    } else {
        u64_digits(n as u64)
    }
}

/// Jumlah digit desimal dari u64 (1 untuk 0) — hindari format! alloc di %d.
fn u64_digits(mut n: u64) -> usize {
    let mut d = 1usize;
    while n >= 10 {
        n /= 10;
        d += 1;
    }
    d
}

/// Jumlah digit hex dari u64 (1 untuk 0) — hindari format! alloc di %h.
#[allow(dead_code)] // util: dipakai tool/feature selanjutnya
fn u64_hex_digits(mut n: u64) -> usize {
    let mut d = 1usize;
    while n >= 16 {
        n /= 16;
        d += 1;
    }
    d
}

pub fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ─── Signal utilities ───────────────────────────────────────────────────

pub fn signal_is_2state(signals: &[SignalInfo], id: SignalId) -> bool {
    signals.get(id).map(|s| s.is_2state).unwrap_or(false)
}

pub fn sanitize_for_2state(signals: &[SignalInfo], id: SignalId, val: &mut LogicVec) {
    if !signal_is_2state(signals, id) {
        return;
    }
    for bit in val.bits.iter_mut() {
        if *bit == LogicVal::X || *bit == LogicVal::Z {
            *bit = LogicVal::Zero;
        }
    }
}

pub fn resolve_net_values(net_type: NetType, current: &LogicVec, incoming: &LogicVec) -> LogicVec {
    let width = current.width.max(incoming.width);
    let mut bits = Vec::with_capacity(width);
    for i in 0..width {
        let cur = current.bits.get(i).copied().unwrap_or(LogicVal::Z);
        let inc = incoming.bits.get(i).copied().unwrap_or(LogicVal::Z);
        bits.push(net_type.resolve_bit(cur, inc));
    }
    LogicVec { bits, width }
}

pub fn read_hex_file(
    filename: &str,
    elem_width: usize,
    array_depth: usize,
    start: Option<usize>,
    end: Option<usize>,
) -> Result<Vec<LogicVec>, SimError> {
    let content = std::fs::read_to_string(filename).map_err(|e| {
        SimError::with_diag(
            DiagCode::IoError,
            format!("cannot read {}: {}", filename, e),
        )
    })?;
    let start_addr = start.unwrap_or(0);
    let end_addr = end.unwrap_or(array_depth - 1);
    let len = end_addr - start_addr + 1;
    let mut data = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("//") || line.starts_with('#') {
            continue;
        }
        let val = i64::from_str_radix(line, 16).map_err(|e| {
            SimError::with_diag(
                DiagCode::InvalidSyntax,
                format!("bad hex value '{}': {}", line, e),
            )
        })?;
        data.push(LogicVec::from_u64(val as u64, elem_width));
        if data.len() >= len {
            break;
        }
    }
    Ok(data)
}

pub fn read_bin_file(
    filename: &str,
    elem_width: usize,
    array_depth: usize,
    start: Option<usize>,
    end: Option<usize>,
) -> Result<Vec<LogicVec>, SimError> {
    let content = std::fs::read_to_string(filename).map_err(|e| {
        SimError::with_diag(
            DiagCode::IoError,
            format!("cannot read {}: {}", filename, e),
        )
    })?;
    let start_addr = start.unwrap_or(0);
    let end_addr = end.unwrap_or(array_depth - 1);
    let len = end_addr - start_addr + 1;
    let mut data = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("//") || line.starts_with('#') {
            continue;
        }
        let val = i64::from_str_radix(line, 2).map_err(|e| {
            SimError::with_diag(
                DiagCode::InvalidSyntax,
                format!("bad binary value '{}': {}", line, e),
            )
        })?;
        data.push(LogicVec::from_u64(val as u64, elem_width));
        if data.len() >= len {
            break;
        }
    }
    Ok(data)
}

pub fn string_to_logicvec(s: &str) -> LogicVec {
    let width = s.len() * 8;
    let mut bits = Vec::with_capacity(width);
    for byte in s.bytes() {
        for i in 0..8 {
            bits.push(if (byte >> i) & 1 == 1 {
                LogicVal::One
            } else {
                LogicVal::Zero
            });
        }
    }
    // Add null terminator
    for _ in 0..8 {
        bits.push(LogicVal::Zero);
    }
    LogicVec {
        bits,
        width: width + 8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_engine() -> SimulationEngine {
        let design = crate::test_util::compile_str("module top; endmodule").unwrap();
        SimulationEngine::new(design, 100)
    }

    #[test]
    fn test_format_display_hex_xz_aware() {
        let mut e = test_engine();
        // IEEE 1800 & konvensi simulator: nibble ber-X → 'x', ber-Z (tanpa X)
        // → 'z'. Sebelumnya `$display("%h", x)` mencetak '0' (to_u64 memetakan
        // X→0) — user debugging propagasi X melihat nilai seolah known.
        let x = LogicVec::fill(LogicVal::X, 8);
        assert_eq!(
            e.format_display_fmt("%h", vec![(x, false)].into_iter()),
            "xx"
        );
        // 8'bzzzz_0101 → "z5" (bits[0]=LSB; nibble tinggi bits[4..7]=zzzz)
        let z5 = LogicVec {
            bits: vec![
                LogicVal::One,
                LogicVal::Zero,
                LogicVal::One,
                LogicVal::Zero,
                LogicVal::Z,
                LogicVal::Z,
                LogicVal::Z,
                LogicVal::Z,
            ],
            width: 8,
        };
        assert_eq!(
            e.format_display_fmt("%h", vec![(z5, false)].into_iter()),
            "z5"
        );
        // Known tidak berubah.
        let f0 = LogicVec::from_u64(0xf0, 8);
        assert_eq!(
            e.format_display_fmt("%h", vec![(f0.clone(), false)].into_iter()),
            "f0"
        );
        assert_eq!(
            e.format_display_fmt("%04h", vec![(f0, false)].into_iter()),
            "00f0"
        );
        let z0 = LogicVec::from_u64(0, 8);
        // `%h` plain = lebar nibble penuh nilai (IEEE 1800 §21.2.1.3 +
        // differential iverilog): 8'h00 → "00" (bukan "0"), 4'h0 → "0".
        assert_eq!(
            e.format_display_fmt("%h", vec![(z0, false)].into_iter()),
            "00"
        );
        let z4 = LogicVec::from_u64(0, 4);
        assert_eq!(
            e.format_display_fmt("%h", vec![(z4, false)].into_iter()),
            "0"
        );
    }

    #[test]
    fn test_format_display_decimal_xz_aware() {
        // DESIMAL utk nilai UNKNOWN = huruf, BUKAN angka 0 — X→0 adalah
        // silent coercion menyesatkan (fuzzer verify_bad_0039:
        // `OUT_RST_BAD=<0>` padahal out=X; test f3: mivon P=[1] vs
        // golden iverilog P=[X]). Golden iverilog: semua-X → "x",
        // semua-Z → "z", partially-unknown → "X", `%4d` space-pad.
        let mut e = test_engine();
        let x = LogicVec::fill(LogicVal::X, 4);
        assert_eq!(
            e.format_display_fmt("%0d", vec![(x.clone(), false)].into_iter()),
            "x",
            "semua-X → x"
        );
        assert_eq!(
            e.format_display_fmt("%4d", vec![(x, false)].into_iter()),
            "   x",
            "%4d space-pad (bukan zero-pad) utk letter"
        );
        let z = LogicVec::fill(LogicVal::Z, 4);
        assert_eq!(
            e.format_display_fmt("%0d", vec![(z, false)].into_iter()),
            "z",
            "semua-Z → z"
        );
        // 4'b00x1 (partially unknown) → 'X' (golden iverilog).
        let p = LogicVec {
            bits: vec![LogicVal::One, LogicVal::X, LogicVal::Zero, LogicVal::Zero],
            width: 4,
        };
        assert_eq!(
            e.format_display_fmt("%0d", vec![(p, false)].into_iter()),
            "X",
            "partially-unknown → X"
        );
        // Known tetap desimal utuh.
        let n = LogicVec::from_u64(42, 8);
        assert_eq!(
            e.format_display_fmt("%0d", vec![(n, false)].into_iter()),
            "42"
        );
        let s = LogicVec::from_u64((-5i64) as u64, 32);
        assert_eq!(
            e.format_display_fmt("%0d", vec![(s, true)].into_iter()),
            "-5",
            "signed known tetap negatif"
        );
    }
}
