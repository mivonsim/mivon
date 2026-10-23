//! Evaluator ekspresi Tcl (subset) untuk FEAT-06.
//!
//! Mendukung integer (desimal/hex/biner), string literal, operator
//! aritmetika `+ - * / %`, perbandingan `== != < <= > >=`, `eq`/`ne`
//! (string), logika `&& || !`, bitwise `& | ^ << >>`, dan tanda kurung.
//!
//! Substitusi `$var`/`[cmd]` dilakukan pemanggil (`Interp::subst`) sebelum
//! teks masuk ke sini — sama seperti Tcl `expr` yang melakukan substitusi
//! sendiri pada argumen braced.

use std::fmt;

/// Nilai hasil evaluasi ekspresi.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Str(String),
}

impl Value {
    pub fn truthy(&self) -> Result<bool, String> {
        match self {
            Value::Int(v) => Ok(*v != 0),
            Value::Str(s) => match s.as_str() {
                "true" | "yes" | "on" => Ok(true),
                "false" | "no" | "off" => Ok(false),
                _ => s
                    .parse::<i64>()
                    .map(|v| v != 0)
                    .map_err(|_| format!("expected boolean value but got \"{}\"", s)),
            },
        }
    }

    pub fn as_int(&self) -> Result<i64, String> {
        match self {
            Value::Int(v) => Ok(*v),
            Value::Str(s) => s
                .trim()
                .parse::<i64>()
                .map_err(|_| format!("expected integer but got \"{}\"", s)),
        }
    }

    pub fn as_string(&self) -> String {
        match self {
            Value::Int(v) => v.to_string(),
            Value::Str(s) => s.clone(),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Or,
    And,
    Eq,
    Ne,
    EqStr,
    NeStr,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Not,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Int(i64),
    Str(String),
    Op(Op),
    LParen,
    RParen,
}

/// Evaluasi ekspresi Tcl. Teks sudah melalui substitusi variabel.
pub fn eval(src: &str) -> Result<Value, String> {
    let toks = tokenize(src)?;
    let mut p = Parser { toks, pos: 0 };
    let v = p.parse_or()?;
    if p.pos != p.toks.len() {
        return Err(format!("unexpected token in expression: \"{}\"", src));
    }
    Ok(v)
}

fn tokenize(src: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '(' {
            out.push(Tok::LParen);
            i += 1;
            continue;
        }
        if c == ')' {
            out.push(Tok::RParen);
            i += 1;
            continue;
        }
        if c == '"' {
            i += 1;
            let mut s = String::new();
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                    s.push(chars[i]);
                } else {
                    s.push(chars[i]);
                }
                i += 1;
            }
            if i >= chars.len() {
                return Err("missing close-quote in expression".to_string());
            }
            i += 1;
            out.push(Tok::Str(s));
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            out.push(Tok::Int(parse_int(&text)?));
            continue;
        }
        // Operator dua karakter lebih dulu.
        if i + 1 < chars.len() {
            let two: String = chars[i..i + 2].iter().collect();
            let op = match two.as_str() {
                "&&" => Some(Op::And),
                "||" => Some(Op::Or),
                "==" => Some(Op::Eq),
                "!=" => Some(Op::Ne),
                "<=" => Some(Op::Le),
                ">=" => Some(Op::Ge),
                "<<" => Some(Op::Shl),
                ">>" => Some(Op::Shr),
                _ => None,
            };
            if let Some(op) = op {
                out.push(Tok::Op(op));
                i += 2;
                continue;
            }
        }
        match c {
            '+' => {
                out.push(Tok::Op(Op::Add));
                i += 1;
            }
            '-' => {
                out.push(Tok::Op(Op::Sub));
                i += 1;
            }
            '*' => {
                out.push(Tok::Op(Op::Mul));
                i += 1;
            }
            '/' => {
                out.push(Tok::Op(Op::Div));
                i += 1;
            }
            '%' => {
                out.push(Tok::Op(Op::Mod));
                i += 1;
            }
            '<' => {
                out.push(Tok::Op(Op::Lt));
                i += 1;
            }
            '>' => {
                out.push(Tok::Op(Op::Gt));
                i += 1;
            }
            '&' => {
                out.push(Tok::Op(Op::BitAnd));
                i += 1;
            }
            '|' => {
                out.push(Tok::Op(Op::BitOr));
                i += 1;
            }
            '^' => {
                out.push(Tok::Op(Op::BitXor));
                i += 1;
            }
            '!' => {
                out.push(Tok::Op(Op::Not));
                i += 1;
            }
            _ if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                match word.as_str() {
                    "eq" => out.push(Tok::Op(Op::EqStr)),
                    "ne" => out.push(Tok::Op(Op::NeStr)),
                    "and" => out.push(Tok::Op(Op::And)),
                    "or" => out.push(Tok::Op(Op::Or)),
                    "not" => out.push(Tok::Op(Op::Not)),
                    "true" => out.push(Tok::Int(1)),
                    "false" => out.push(Tok::Int(0)),
                    _ => out.push(Tok::Str(word)),
                }
            }
            _ => {
                return Err(format!("invalid character '{}' in expression", c));
            }
        }
    }
    Ok(out)
}

fn parse_int(text: &str) -> Result<i64, String> {
    let t = text.replace('_', "");
    let lower = t.to_ascii_lowercase();
    let parsed = if let Some(rest) = lower.strip_prefix("0x") {
        i64::from_str_radix(rest, 16)
    } else if let Some(rest) = lower.strip_prefix("0b") {
        i64::from_str_radix(rest, 2)
    } else {
        t.parse::<i64>()
    };
    parsed.map_err(|_| format!("invalid integer \"{}\"", text))
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn eat_op(&mut self, op: Op) -> bool {
        if matches!(self.peek(), Some(Tok::Op(o)) if *o == op) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn parse_or(&mut self) -> Result<Value, String> {
        let mut left = self.parse_and()?;
        while self.eat_op(Op::Or) {
            let right = self.parse_and()?;
            let l = left.truthy()?;
            let r = right.truthy()?;
            left = Value::Int(i64::from(l || r));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Value, String> {
        let mut left = self.parse_bit_or()?;
        while self.eat_op(Op::And) {
            let right = self.parse_bit_or()?;
            let l = left.truthy()?;
            let r = right.truthy()?;
            left = Value::Int(i64::from(l && r));
        }
        Ok(left)
    }

    fn parse_bit_or(&mut self) -> Result<Value, String> {
        let mut left = self.parse_bit_xor()?;
        while self.eat_op(Op::BitOr) {
            let right = self.parse_bit_xor()?;
            left = Value::Int(left.as_int()? | right.as_int()?);
        }
        Ok(left)
    }

    fn parse_bit_xor(&mut self) -> Result<Value, String> {
        let mut left = self.parse_bit_and()?;
        while self.eat_op(Op::BitXor) {
            let right = self.parse_bit_and()?;
            left = Value::Int(left.as_int()? ^ right.as_int()?);
        }
        Ok(left)
    }

    fn parse_bit_and(&mut self) -> Result<Value, String> {
        let mut left = self.parse_equality()?;
        while self.eat_op(Op::BitAnd) {
            let right = self.parse_equality()?;
            left = Value::Int(left.as_int()? & right.as_int()?);
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<Value, String> {
        let mut left = self.parse_relational()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op(o)) if matches!(o, Op::Eq | Op::Ne | Op::EqStr | Op::NeStr) => *o,
                _ => break,
            };
            self.pos += 1;
            let right = self.parse_relational()?;
            let equal = match (&left, &right) {
                (Value::Int(a), Value::Int(b)) => a == b,
                _ => left.as_string() == right.as_string(),
            };
            let result = match op {
                Op::Eq | Op::EqStr => equal,
                _ => !equal,
            };
            left = Value::Int(i64::from(result));
        }
        Ok(left)
    }

    fn parse_relational(&mut self) -> Result<Value, String> {
        let mut left = self.parse_shift()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op(o)) if matches!(o, Op::Lt | Op::Le | Op::Gt | Op::Ge) => *o,
                _ => break,
            };
            self.pos += 1;
            let right = self.parse_shift()?;
            let a = left.as_int()?;
            let b = right.as_int()?;
            let result = match op {
                Op::Lt => a < b,
                Op::Le => a <= b,
                Op::Gt => a > b,
                _ => a >= b,
            };
            left = Value::Int(i64::from(result));
        }
        Ok(left)
    }

    fn parse_shift(&mut self) -> Result<Value, String> {
        let mut left = self.parse_additive()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op(o)) if matches!(o, Op::Shl | Op::Shr) => *o,
                _ => break,
            };
            self.pos += 1;
            let right = self.parse_additive()?;
            let a = left.as_int()?;
            let shift = right.as_int()?;
            if !(0..64).contains(&shift) {
                return Err(format!("shift amount {} out of range", shift));
            }
            left = Value::Int(if op == Op::Shl {
                a << shift
            } else {
                a >> shift
            });
        }
        Ok(left)
    }

    fn parse_additive(&mut self) -> Result<Value, String> {
        let mut left = self.parse_multiplicative()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op(o)) if matches!(o, Op::Add | Op::Sub) => *o,
                _ => break,
            };
            self.pos += 1;
            let right = self.parse_multiplicative()?;
            let a = left.as_int()?;
            let b = right.as_int()?;
            left = Value::Int(if op == Op::Add { a + b } else { a - b });
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<Value, String> {
        let mut left = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op(o)) if matches!(o, Op::Mul | Op::Div | Op::Mod) => *o,
                _ => break,
            };
            self.pos += 1;
            let right = self.parse_unary()?;
            let a = left.as_int()?;
            let b = right.as_int()?;
            if (op == Op::Div || op == Op::Mod) && b == 0 {
                return Err("divide by zero".to_string());
            }
            left = Value::Int(match op {
                Op::Mul => a * b,
                Op::Div => a / b,
                _ => a % b,
            });
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Value, String> {
        if self.eat_op(Op::Not) {
            let v = self.parse_unary()?;
            return Ok(Value::Int(i64::from(!v.truthy()?)));
        }
        if self.eat_op(Op::Sub) {
            let v = self.parse_unary()?;
            return Ok(Value::Int(-v.as_int()?));
        }
        if self.eat_op(Op::Add) {
            return self.parse_unary();
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Value, String> {
        match self.peek().cloned() {
            Some(Tok::Int(v)) => {
                self.pos += 1;
                Ok(Value::Int(v))
            }
            Some(Tok::Str(s)) => {
                self.pos += 1;
                Ok(Value::Str(s))
            }
            Some(Tok::LParen) => {
                self.pos += 1;
                let v = self.parse_or()?;
                match self.peek() {
                    Some(Tok::RParen) => {
                        self.pos += 1;
                        Ok(v)
                    }
                    _ => Err("missing close parenthesis".to_string()),
                }
            }
            _ => Err("unexpected end of expression".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(eval("1 + 2 * 3").unwrap(), Value::Int(7));
        assert_eq!(eval("(1 + 2) * 3").unwrap(), Value::Int(9));
        assert_eq!(eval("10 % 4").unwrap(), Value::Int(2));
        assert_eq!(eval("-5 + 2").unwrap(), Value::Int(-3));
    }

    #[test]
    fn comparison_and_logic() {
        assert_eq!(eval("3 > 2 && 1 <= 1").unwrap(), Value::Int(1));
        assert_eq!(eval("!(3 == 4)").unwrap(), Value::Int(1));
        assert_eq!(eval("\"a\" eq \"a\"").unwrap(), Value::Int(1));
        assert_eq!(eval("\"a\" ne \"b\"").unwrap(), Value::Int(1));
    }

    #[test]
    fn bitwise_and_shift() {
        assert_eq!(eval("5 & 3").unwrap(), Value::Int(1));
        assert_eq!(eval("5 | 2").unwrap(), Value::Int(7));
        assert_eq!(eval("5 ^ 1").unwrap(), Value::Int(4));
        assert_eq!(eval("1 << 4").unwrap(), Value::Int(16));
        assert_eq!(eval("32 >> 2").unwrap(), Value::Int(8));
        // Precedence: & lebih kuat dari ==, shift lebih kuat dari relasional.
        assert_eq!(eval("1 | 2 == 3").unwrap(), Value::Int(1));
        assert_eq!(eval("1 << 2 + 1").unwrap(), Value::Int(8));
    }

    #[test]
    fn hex_and_binary_literals() {
        assert_eq!(eval("0x10 + 1").unwrap(), Value::Int(17));
        assert_eq!(eval("0b101").unwrap(), Value::Int(5));
    }

    #[test]
    fn divide_by_zero_is_error() {
        assert!(eval("1 / 0").is_err());
    }
}
