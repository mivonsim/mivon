//! FEAT-06: Tcl scripting interface (subset Tcl 8.6) + bridge simulasi.
//!
//! Audit menandai FEAT-06 "Tcl scripting interface" belum ada. Seluruh tool
//! EDA komersial memakai Tcl untuk otomasi flow/regression, jadi modul ini
//! menyediakan interpreter Tcl subset yang cukup untuk skrip orkestrasi:
//!
//! - substitusi `$var`, `${var}`, `[cmd]`, escape backslash;
//! - word braced `{...}` (tanpa substitusi) dan quoted `"..."`;
//! - perintah inti: `set puts incr append lappend unset expr if while for
//!   foreach proc return break continue list lindex llength lrange string info
//!   catch error eval source exit`;
//! - bridge simulasi: `mivon compile|simulate|get|time|signals|reset`.
//!
//! 1 file = 1 tanggung jawab: `mod.rs` = interpreter + perintah inti,
//! `expr.rs` = evaluator ekspresi, `bridge.rs` = perintah simulator.

mod bridge;
mod expr;

use std::collections::HashMap;
use std::fmt;

use mivon_ir::IrDesign;

/// Batas iterasi loop agar skrip salah tidak menggantung CLI.
const MAX_LOOP: usize = 10_000_000;
/// Batas kedalaman pemanggilan perintah/proc (proteksi stack).
const MAX_DEPTH: usize = 64;

/// Error/flow-control interpreter.
#[derive(Debug, Clone, PartialEq)]
pub enum TclError {
    /// Error skrip biasa (bisa ditangkap `catch`).
    Msg(String),
    /// `return` dari body proc.
    Return(String),
    /// `break` keluar loop.
    Break,
    /// `continue` lanjut iterasi.
    Continue,
    /// `exit ?code?` — keluar dari proses CLI.
    Exit(i32),
}

impl fmt::Display for TclError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TclError::Msg(m) => write!(f, "{}", m),
            TclError::Return(_) => write!(f, "invoked \"return\" outside of a procedure"),
            TclError::Break => write!(f, "invoked \"break\" outside of a loop"),
            TclError::Continue => write!(f, "invoked \"continue\" outside of a loop"),
            TclError::Exit(code) => write!(f, "exit {}", code),
        }
    }
}

impl TclError {
    /// Pesan yang bisa dicetak ke pengguna (tanpa prefix flow-control).
    pub fn message(&self) -> String {
        self.to_string()
    }
}

impl From<String> for TclError {
    fn from(s: String) -> Self {
        TclError::Msg(s)
    }
}

impl From<&str> for TclError {
    fn from(s: &str) -> Self {
        TclError::Msg(s.to_string())
    }
}

/// Hasil eksekusi skrip/perintah Tcl.
pub type TclResult = Result<String, TclError>;

#[derive(Debug, Clone)]
struct Word {
    raw: String,
    braced: bool,
}

#[derive(Debug, Clone)]
struct ProcDef {
    params: Vec<String>,
    variadic: bool,
    body: String,
}

/// Interpreter Tcl + state bridge simulasi.
pub struct Interp {
    vars: HashMap<String, String>,
    procs: HashMap<String, ProcDef>,
    output: Vec<String>,
    exit_code: Option<i32>,
    depth: usize,
    pub(crate) design: Option<IrDesign>,
    pub(crate) signals: HashMap<String, mivon_core::LogicVec>,
    pub(crate) sim_time: u64,
}

impl Default for Interp {
    fn default() -> Self {
        Self::new()
    }
}

impl Interp {
    pub fn new() -> Self {
        Interp {
            vars: HashMap::new(),
            procs: HashMap::new(),
            output: Vec::new(),
            exit_code: None,
            depth: 0,
            design: None,
            signals: HashMap::new(),
            sim_time: 0,
        }
    }

    /// Baris keluaran `puts` (urut sesuai eksekusi).
    pub fn output(&self) -> &[String] {
        &self.output
    }

    /// Kode `exit` bila skrip memanggilnya.
    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    pub fn set_var(&mut self, name: &str, value: impl Into<String>) {
        self.vars.insert(name.to_string(), value.into());
    }

    pub fn get_var(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(|s| s.as_str())
    }

    /// Seed `argv`/`argc` (argumen CLI setelah nama skrip).
    pub fn set_argv(&mut self, args: &[String]) {
        let list = format_list(args);
        self.vars.insert("argv".to_string(), list);
        self.vars.insert("argc".to_string(), args.len().to_string());
        self.vars.insert("argv0".to_string(), String::new());
    }

    /// Jalankan skrip; `exit` ditangkap dan disimpan di `exit_code`.
    pub fn run(&mut self, script: &str) -> TclResult {
        match self.eval_script(script) {
            Ok(v) => Ok(v),
            Err(TclError::Exit(code)) => {
                self.exit_code = Some(code);
                Ok(String::new())
            }
            Err(e) => Err(e),
        }
    }

    pub(crate) fn eval_script(&mut self, src: &str) -> TclResult {
        let cmds = split_commands(src)?;
        let mut last = String::new();
        for cmd in cmds {
            last = self.eval_command(&cmd)?;
        }
        Ok(last)
    }

    fn eval_command(&mut self, words: &[Word]) -> TclResult {
        if words.is_empty() {
            return Ok(String::new());
        }
        let name = self.word_value(&words[0])?;
        if name.is_empty() {
            return Ok(String::new());
        }
        self.call(&name, &words[1..])
    }

    fn word_value(&mut self, w: &Word) -> TclResult {
        if w.braced {
            Ok(w.raw.clone())
        } else {
            self.subst(&w.raw)
        }
    }

    /// Substitusi `$var`, `${var}`, `[cmd]`, dan escape backslash.
    fn subst(&mut self, raw: &str) -> TclResult {
        let chars: Vec<char> = raw.chars().collect();
        let mut out = String::new();
        let mut i = 0;
        while i < chars.len() {
            match chars[i] {
                '\\' => {
                    if i + 1 >= chars.len() {
                        out.push('\\');
                        i += 1;
                        continue;
                    }
                    let next = chars[i + 1];
                    match next {
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        'r' => out.push('\r'),
                        '\\' => out.push('\\'),
                        '$' => out.push('$'),
                        '[' => out.push('['),
                        ']' => out.push(']'),
                        '{' => out.push('{'),
                        '}' => out.push('}'),
                        ' ' => out.push(' '),
                        ';' => out.push(';'),
                        other => out.push(other),
                    }
                    i += 2;
                }
                '$' => {
                    i += 1;
                    let mut name = String::new();
                    if i < chars.len() && chars[i] == '{' {
                        i += 1;
                        while i < chars.len() && chars[i] != '}' {
                            name.push(chars[i]);
                            i += 1;
                        }
                        if i >= chars.len() {
                            return Err(TclError::Msg(
                                "missing close-brace for variable name".to_string(),
                            ));
                        }
                        i += 1;
                    } else {
                        while i < chars.len() {
                            let c = chars[i];
                            if c.is_alphanumeric() || c == '_' {
                                name.push(c);
                                i += 1;
                            } else if c == ':' && i + 1 < chars.len() && chars[i + 1] == ':' {
                                // Namespace separator `::` saja (bukan ':' tunggal).
                                name.push(':');
                                name.push(':');
                                i += 2;
                            } else {
                                break;
                            }
                        }
                    }
                    if name.is_empty() {
                        out.push('$');
                        continue;
                    }
                    match self.vars.get(&name) {
                        Some(v) => out.push_str(v),
                        None => {
                            return Err(TclError::Msg(format!(
                                "can't read \"{}\": no such variable",
                                name
                            )))
                        }
                    }
                }
                '[' => {
                    let (inner, next) = take_bracket(&chars, i)?;
                    let value = self.eval_script(&inner)?;
                    out.push_str(value.trim_end_matches('\n'));
                    i = next;
                }
                other => {
                    out.push(other);
                    i += 1;
                }
            }
        }
        Ok(out)
    }

    fn eval_body(&mut self, w: &Word) -> TclResult {
        if w.braced {
            self.eval_script(&w.raw)
        } else {
            let script = self.subst(&w.raw)?;
            self.eval_script(&script)
        }
    }

    fn eval_condition(&mut self, w: &Word) -> Result<bool, TclError> {
        let text = self.subst(&w.raw)?;
        let value = expr::eval(&text).map_err(TclError::Msg)?;
        value.truthy().map_err(TclError::Msg)
    }

    fn call(&mut self, name: &str, args: &[Word]) -> TclResult {
        if let Some(proc) = self.procs.get(name).cloned() {
            return self.call_proc(&proc, args);
        }
        match name {
            "set" => {
                let var = self.need(args, 0, "set")?;
                if args.len() > 1 {
                    let value = self.word_value(&args[1])?;
                    self.vars.insert(var, value.clone());
                    Ok(value)
                } else {
                    self.vars.get(&var).cloned().ok_or_else(|| {
                        TclError::Msg(format!("can't read \"{}\": no such variable", var))
                    })
                }
            }
            "puts" => {
                let mut idx = 0;
                let mut newline = true;
                if !args.is_empty() && args[0].raw == "-nonewline" {
                    newline = false;
                    idx = 1;
                }
                // Channel opsional (stdout/stderr) — diabaikan.
                if args.len() - idx > 1 && !args[idx].braced && args[idx].raw.starts_with("std") {
                    idx += 1;
                }
                let text = self.need(args, idx, "puts")?;
                self.output.push(if newline { text } else { text });
                Ok(String::new())
            }
            "incr" => {
                let var = self.need(args, 0, "incr")?;
                let amount = if args.len() > 1 {
                    self.word_value(&args[1])?
                        .parse::<i64>()
                        .map_err(|_| TclError::Msg("expected integer for incr".to_string()))?
                } else {
                    1
                };
                let current = self
                    .vars
                    .get(&var)
                    .ok_or_else(|| {
                        TclError::Msg(format!("can't read \"{}\": no such variable", var))
                    })?
                    .parse::<i64>()
                    .map_err(|_| TclError::Msg(format!("\"{}\" is not an integer", var)))?;
                let next = current + amount;
                self.vars.insert(var, next.to_string());
                Ok(next.to_string())
            }
            "append" => {
                let var = self.need(args, 0, "append")?;
                let mut current = self.vars.get(&var).cloned().unwrap_or_default();
                for w in &args[1..] {
                    current.push_str(&self.word_value(w)?);
                }
                self.vars.insert(var, current.clone());
                Ok(current)
            }
            "lappend" => {
                let var = self.need(args, 0, "lappend")?;
                let mut items = parse_list(&self.vars.get(&var).cloned().unwrap_or_default());
                for w in &args[1..] {
                    items.push(self.word_value(w)?);
                }
                let list = format_list(&items);
                self.vars.insert(var, list.clone());
                Ok(list)
            }
            "unset" => {
                for w in args {
                    let name = self.word_value(w)?;
                    self.vars.remove(&name);
                }
                Ok(String::new())
            }
            "expr" => {
                let mut text = String::new();
                for w in args {
                    if !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(&w.raw);
                }
                let expanded = self.subst(&text)?;
                let value = expr::eval(&expanded).map_err(TclError::Msg)?;
                Ok(value.as_string())
            }
            "if" => self.cmd_if(args),
            "while" => self.cmd_while(args),
            "for" => self.cmd_for(args),
            "foreach" => self.cmd_foreach(args),
            "proc" => self.cmd_proc(args),
            "return" => {
                let value = if args.is_empty() {
                    String::new()
                } else {
                    self.word_value(&args[0])?
                };
                Err(TclError::Return(value))
            }
            "break" => Err(TclError::Break),
            "continue" => Err(TclError::Continue),
            "list" => {
                let mut items = Vec::new();
                for w in args {
                    items.push(self.word_value(w)?);
                }
                Ok(format_list(&items))
            }
            "lindex" => {
                let list = parse_list(&self.need(args, 0, "lindex")?);
                let index_text = self.need(args, 1, "lindex")?;
                let idx = resolve_index(&index_text, list.len());
                match idx {
                    Some(i) => Ok(list.get(i).cloned().unwrap_or_default()),
                    None => Ok(String::new()),
                }
            }
            "llength" => {
                let list = parse_list(&self.need(args, 0, "llength")?);
                Ok(list.len().to_string())
            }
            "lrange" => {
                let list = parse_list(&self.need(args, 0, "lrange")?);
                let first = resolve_index(&self.need(args, 1, "lrange")?, list.len()).unwrap_or(0);
                let last = resolve_index(&self.need(args, 2, "lrange")?, list.len())
                    .unwrap_or(list.len().saturating_sub(1));
                let mut out = Vec::new();
                let mut i = first;
                while i <= last && i < list.len() {
                    out.push(list[i].clone());
                    i += 1;
                }
                Ok(format_list(&out))
            }
            "string" => self.cmd_string(args),
            "info" => self.cmd_info(args),
            "catch" => {
                let script_word = args
                    .first()
                    .ok_or_else(|| TclError::Msg("wrong # args: should be \"catch script ?varName?\"".to_string()))?;
                let var = if args.len() > 1 {
                    Some(self.word_value(&args[1])?)
                } else {
                    None
                };
                let result = self.eval_body(script_word);
                match result {
                    Ok(v) => {
                        if let Some(name) = var {
                            self.vars.insert(name, v);
                        }
                        Ok("0".to_string())
                    }
                    Err(TclError::Msg(m)) => {
                        if let Some(name) = var {
                            self.vars.insert(name, m);
                        }
                        Ok("1".to_string())
                    }
                    Err(TclError::Return(v)) => {
                        if let Some(name) = var {
                            self.vars.insert(name, v);
                        }
                        Ok("2".to_string())
                    }
                    Err(TclError::Break) => {
                        if let Some(name) = var {
                            self.vars.insert(name, "break".to_string());
                        }
                        Ok("3".to_string())
                    }
                    Err(TclError::Continue) => {
                        if let Some(name) = var {
                            self.vars.insert(name, "continue".to_string());
                        }
                        Ok("4".to_string())
                    }
                    Err(e @ TclError::Exit(_)) => Err(e),
                }
            }
            "error" => {
                let msg = self.need(args, 0, "error")?;
                Err(TclError::Msg(msg))
            }
            "eval" => {
                let mut script = String::new();
                for w in args {
                    if !script.is_empty() {
                        script.push(' ');
                    }
                    script.push_str(&self.word_value(w)?);
                }
                self.eval_script(&script)
            }
            "source" => {
                let path = self.need(args, 0, "source")?;
                let src = std::fs::read_to_string(&path).map_err(|e| {
                    TclError::Msg(format!("couldn't read file \"{}\": {}", path, e))
                })?;
                self.eval_script(&src)
            }
            "exit" => {
                let code = if args.is_empty() {
                    0
                } else {
                    self.word_value(&args[0])?
                        .parse::<i32>()
                        .map_err(|_| TclError::Msg("expected integer exit code".to_string()))?
                };
                Err(TclError::Exit(code))
            }
            "mivon" => self.cmd_mivon(args),
            other => Err(TclError::Msg(format!(
                "invalid command name \"{}\"",
                other
            ))),
        }
    }

    fn need(&mut self, args: &[Word], index: usize, cmd: &str) -> TclResult {
        match args.get(index) {
            Some(w) => self.word_value(w),
            None => Err(TclError::Msg(format!(
                "wrong # args: missing argument {} for \"{}\"",
                index + 1,
                cmd
            ))),
        }
    }

    fn call_proc(&mut self, proc: &ProcDef, args: &[Word]) -> TclResult {
        if self.depth >= MAX_DEPTH {
            return Err(TclError::Msg(
                "too many nested calls (infinite recursion?)".to_string(),
            ));
        }
        let mut values = Vec::with_capacity(args.len());
        for w in args {
            values.push(self.word_value(w)?);
        }
        let fixed = if proc.variadic {
            proc.params.len().saturating_sub(1)
        } else {
            proc.params.len()
        };
        if values.len() < fixed || (!proc.variadic && values.len() != fixed) {
            return Err(TclError::Msg(format!(
                "wrong # args: proc expects {} argument(s), got {}",
                fixed,
                values.len()
            )));
        }
        // Simpan nilai lama lalu kembalikan setelah proc selesai (scope lokal).
        let mut saved: Vec<(String, Option<String>)> = Vec::new();
        for (i, name) in proc.params.iter().enumerate() {
            let value = if proc.variadic && i + 1 == proc.params.len() {
                format_list(&values[fixed..])
            } else {
                values[i].clone()
            };
            saved.push((name.clone(), self.vars.get(name).cloned()));
            self.vars.insert(name.clone(), value);
        }
        self.depth += 1;
        let result = self.eval_script(&proc.body);
        self.depth -= 1;
        for (name, old) in saved {
            match old {
                Some(v) => {
                    self.vars.insert(name, v);
                }
                None => {
                    self.vars.remove(&name);
                }
            }
        }
        match result {
            Ok(v) => Ok(v),
            Err(TclError::Return(v)) => Ok(v),
            Err(e) => Err(e),
        }
    }

    fn cmd_if(&mut self, args: &[Word]) -> TclResult {
        let mut i = 0;
        loop {
            if i >= args.len() {
                return Ok(String::new());
            }
            if args[i].raw == "else" {
                i += 1;
                let body = args
                    .get(i)
                    .ok_or_else(|| TclError::Msg("wrong # args: no script following \"else\"".to_string()))?;
                return self.eval_body(body);
            }
            if args[i].raw == "elseif" {
                i += 1;
            }
            let cond_word = args
                .get(i)
                .ok_or_else(|| TclError::Msg("wrong # args: no expression after \"if\"".to_string()))?;
            let cond = self.eval_condition(cond_word)?;
            i += 1;
            if i < args.len() && args[i].raw == "then" {
                i += 1;
            }
            let body = args
                .get(i)
                .ok_or_else(|| TclError::Msg("wrong # args: no script following expression".to_string()))?;
            if cond {
                return self.eval_body(body);
            }
            i += 1;
        }
    }

    fn cmd_while(&mut self, args: &[Word]) -> TclResult {
        let cond = args
            .first()
            .ok_or_else(|| TclError::Msg("wrong # args: should be \"while test command\"".to_string()))?;
        let body = args
            .get(1)
            .ok_or_else(|| TclError::Msg("wrong # args: should be \"while test command\"".to_string()))?;
        let mut iterations = 0usize;
        while self.eval_condition(cond)? {
            iterations += 1;
            if iterations > MAX_LOOP {
                return Err(TclError::Msg("loop iteration limit exceeded".to_string()));
            }
            match self.eval_body(body) {
                Ok(_) => {}
                Err(TclError::Break) => break,
                Err(TclError::Continue) => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(String::new())
    }

    fn cmd_for(&mut self, args: &[Word]) -> TclResult {
        if args.len() < 4 {
            return Err(TclError::Msg(
                "wrong # args: should be \"for start test next command\"".to_string(),
            ));
        }
        self.eval_body(&args[0])?;
        let mut iterations = 0usize;
        while self.eval_condition(&args[1])? {
            iterations += 1;
            if iterations > MAX_LOOP {
                return Err(TclError::Msg("loop iteration limit exceeded".to_string()));
            }
            match self.eval_body(&args[3]) {
                Ok(_) => {}
                Err(TclError::Break) => break,
                Err(TclError::Continue) => {}
                Err(e) => return Err(e),
            }
            self.eval_body(&args[2])?;
        }
        Ok(String::new())
    }

    fn cmd_foreach(&mut self, args: &[Word]) -> TclResult {
        if args.len() < 3 {
            return Err(TclError::Msg(
                "wrong # args: should be \"foreach varList list body\"".to_string(),
            ));
        }
        let names = parse_list(&self.word_value(&args[0])?);
        if names.is_empty() {
            return Err(TclError::Msg("foreach varList is empty".to_string()));
        }
        let items = parse_list(&self.word_value(&args[1])?);
        let body = &args[2];
        let mut i = 0;
        while i < items.len() {
            for name in &names {
                let value = items.get(i).cloned().unwrap_or_default();
                self.vars.insert(name.clone(), value);
                i += 1;
            }
            match self.eval_body(body) {
                Ok(_) => {}
                Err(TclError::Break) => break,
                Err(TclError::Continue) => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(String::new())
    }

    fn cmd_proc(&mut self, args: &[Word]) -> TclResult {
        if args.len() != 3 {
            return Err(TclError::Msg(
                "wrong # args: should be \"proc name args body\"".to_string(),
            ));
        }
        let name = self.word_value(&args[0])?;
        let param_text = self.word_value(&args[1])?;
        let mut params = parse_list(&param_text);
        let variadic = params.last().map(|p| p == "args").unwrap_or(false);
        if variadic {
            let last = params.len() - 1;
            params[last] = "args".to_string();
        }
        self.procs.insert(
            name,
            ProcDef {
                params,
                variadic,
                body: args[2].raw.clone(),
            },
        );
        Ok(String::new())
    }

    fn cmd_string(&mut self, args: &[Word]) -> TclResult {
        let sub = args
            .first()
            .ok_or_else(|| TclError::Msg("wrong # args for \"string\"".to_string()))
            .map(|w| self.word_value(w))??;
        match sub.as_str() {
            "length" => {
                let s = self.need(args, 1, "string length")?;
                Ok(s.chars().count().to_string())
            }
            "equal" => {
                let a = self.need(args, 1, "string equal")?;
                let b = self.need(args, 2, "string equal")?;
                Ok(if a == b { "1" } else { "0" }.to_string())
            }
            "tolower" => Ok(self.need(args, 1, "string tolower")?.to_lowercase()),
            "toupper" => Ok(self.need(args, 1, "string toupper")?.to_uppercase()),
            "trim" => Ok(self.need(args, 1, "string trim")?.trim().to_string()),
            "index" => {
                let s = self.need(args, 1, "string index")?;
                let idx = self.need(args, 2, "string index")?;
                let chars: Vec<char> = s.chars().collect();
                let i = resolve_index(&idx, chars.len());
                Ok(i.and_then(|i| chars.get(i)).map(|c| c.to_string()).unwrap_or_default())
            }
            "range" => {
                let s = self.need(args, 1, "string range")?;
                let first = self.need(args, 2, "string range")?;
                let last = self.need(args, 3, "string range")?;
                let chars: Vec<char> = s.chars().collect();
                let start = resolve_index(&first, chars.len()).unwrap_or(0);
                let end = resolve_index(&last, chars.len()).unwrap_or(chars.len().saturating_sub(1));
                if start > end || start >= chars.len() {
                    return Ok(String::new());
                }
                let end = end.min(chars.len().saturating_sub(1));
                Ok(chars[start..=end].iter().collect())
            }
            other => Err(TclError::Msg(format!(
                "unknown or ambiguous subcommand \"{}\" for \"string\"",
                other
            ))),
        }
    }

    fn cmd_info(&mut self, args: &[Word]) -> TclResult {
        let sub = args
            .first()
            .ok_or_else(|| TclError::Msg("wrong # args for \"info\"".to_string()))
            .map(|w| self.word_value(w))??;
        match sub.as_str() {
            "exists" => {
                let name = self.need(args, 1, "info exists")?;
                Ok(if self.vars.contains_key(&name) { "1" } else { "0" }.to_string())
            }
            "procs" => {
                let mut names: Vec<String> = self.procs.keys().cloned().collect();
                names.sort();
                Ok(format_list(&names))
            }
            "vars" => {
                let mut names: Vec<String> = self.vars.keys().cloned().collect();
                names.sort();
                Ok(format_list(&names))
            }
            other => Err(TclError::Msg(format!(
                "unknown or ambiguous subcommand \"{}\" for \"info\"",
                other
            ))),
        }
    }
}

/// Pecah skrip menjadi daftar perintah (tiap perintah = daftar word).
fn split_commands(src: &str) -> Result<Vec<Vec<Word>>, TclError> {
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut commands: Vec<Vec<Word>> = Vec::new();
    let mut current: Vec<Word> = Vec::new();
    while i < chars.len() {
        match chars[i] {
            ' ' | '\t' | '\r' => i += 1,
            '\n' | ';' => {
                i += 1;
                if !current.is_empty() {
                    commands.push(std::mem::take(&mut current));
                }
            }
            '#' if current.is_empty() => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '\\' if i + 1 < chars.len() && chars[i + 1] == '\n' => i += 2,
            _ => {
                let (word, next) = parse_word(&chars, i)?;
                current.push(word);
                i = next;
            }
        }
    }
    if !current.is_empty() {
        commands.push(current);
    }
    Ok(commands)
}

fn parse_word(chars: &[char], start: usize) -> Result<(Word, usize), TclError> {
    let mut i = start;
    if chars[i] == '{' {
        i += 1;
        let mut depth = 1usize;
        let mut raw = String::new();
        while i < chars.len() {
            match chars[i] {
                '\\' if i + 1 < chars.len() => {
                    raw.push(chars[i]);
                    raw.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                '{' => {
                    depth += 1;
                    raw.push('{');
                }
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        i += 1;
                        return Ok((Word { raw, braced: true }, i));
                    }
                    raw.push('}');
                }
                c => raw.push(c),
            }
            i += 1;
        }
        return Err(TclError::Msg("missing close-brace".to_string()));
    }
    if chars[i] == '"' {
        i += 1;
        let mut raw = String::new();
        while i < chars.len() {
            match chars[i] {
                '\\' if i + 1 < chars.len() => {
                    raw.push(chars[i]);
                    raw.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                '"' => {
                    i += 1;
                    return Ok((Word { raw, braced: false }, i));
                }
                '[' => {
                    let (inner, next) = take_bracket(chars, i)?;
                    raw.push('[');
                    raw.push_str(&inner);
                    raw.push(']');
                    i = next;
                }
                c => {
                    raw.push(c);
                    i += 1;
                }
            }
        }
        return Err(TclError::Msg("missing close-quote".to_string()));
    }
    let mut raw = String::new();
    while i < chars.len() {
        match chars[i] {
            ' ' | '\t' | '\r' | '\n' | ';' => break,
            '\\' if i + 1 < chars.len() => {
                raw.push(chars[i]);
                raw.push(chars[i + 1]);
                i += 2;
            }
            '[' => {
                let (inner, next) = take_bracket(chars, i)?;
                raw.push('[');
                raw.push_str(&inner);
                raw.push(']');
                i = next;
            }
            c => {
                raw.push(c);
                i += 1;
            }
        }
    }
    Ok((Word { raw, braced: false }, i))
}

/// Ambil isi `[...]` (mulai di posisi `start`) dan indeks setelah `]`.
fn take_bracket(chars: &[char], start: usize) -> Result<(String, usize), TclError> {
    let mut i = start + 1;
    let mut depth = 1usize;
    let mut inner = String::new();
    while i < chars.len() {
        match chars[i] {
            '\\' if i + 1 < chars.len() => {
                inner.push(chars[i]);
                inner.push(chars[i + 1]);
                i += 2;
            }
            '[' => {
                depth += 1;
                inner.push('[');
                i += 1;
            }
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Ok((inner, i + 1));
                }
                inner.push(']');
                i += 1;
            }
            '{' => {
                let (raw, next) = take_braced(chars, i)?;
                inner.push('{');
                inner.push_str(&raw);
                inner.push('}');
                i = next;
            }
            '"' => {
                let (raw, next) = take_quoted(chars, i)?;
                inner.push('"');
                inner.push_str(&raw);
                inner.push('"');
                i = next;
            }
            c => {
                inner.push(c);
                i += 1;
            }
        }
    }
    Err(TclError::Msg("missing close-bracket".to_string()))
}

fn take_braced(chars: &[char], start: usize) -> Result<(String, usize), TclError> {
    let mut i = start + 1;
    let mut depth = 1usize;
    let mut raw = String::new();
    while i < chars.len() {
        match chars[i] {
            '\\' if i + 1 < chars.len() => {
                raw.push(chars[i]);
                raw.push(chars[i + 1]);
                i += 2;
            }
            '{' => {
                depth += 1;
                raw.push('{');
                i += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok((raw, i + 1));
                }
                raw.push('}');
                i += 1;
            }
            c => {
                raw.push(c);
                i += 1;
            }
        }
    }
    Err(TclError::Msg("missing close-brace".to_string()))
}

fn take_quoted(chars: &[char], start: usize) -> Result<(String, usize), TclError> {
    let mut i = start + 1;
    let mut raw = String::new();
    while i < chars.len() {
        match chars[i] {
            '\\' if i + 1 < chars.len() => {
                raw.push(chars[i]);
                raw.push(chars[i + 1]);
                i += 2;
            }
            '"' => return Ok((raw, i + 1)),
            c => {
                raw.push(c);
                i += 1;
            }
        }
    }
    Err(TclError::Msg("missing close-quote".to_string()))
}

/// Parse list Tcl sederhana (braced/quoted/bare).
fn parse_list(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        if chars[i] == '{' {
            if let Ok((raw, next)) = take_braced(&chars, i) {
                out.push(raw);
                i = next;
                continue;
            }
        }
        if chars[i] == '"' {
            if let Ok((raw, next)) = take_quoted(&chars, i) {
                out.push(raw);
                i = next;
                continue;
            }
        }
        let mut item = String::new();
        while i < chars.len() && !chars[i].is_whitespace() {
            if chars[i] == '\\' && i + 1 < chars.len() {
                item.push(chars[i + 1]);
                i += 2;
            } else {
                item.push(chars[i]);
                i += 1;
            }
        }
        out.push(item);
    }
    out
}

/// Format list Tcl; elemen dengan spasi/karakter khusus dibungkus brace.
fn format_list(items: &[String]) -> String {
    let mut parts = Vec::with_capacity(items.len());
    for item in items {
        if item.is_empty()
            || item
                .chars()
                .any(|c| c.is_whitespace() || matches!(c, '{' | '}' | '"' | '\\' | '$' | '[' | ']' | ';'))
        {
            parts.push(format!("{{{}}}", item));
        } else {
            parts.push(item.clone());
        }
    }
    parts.join(" ")
}

/// Resolusi indeks list: integer, `end`, `end-N`.
fn resolve_index(text: &str, len: usize) -> Option<usize> {
    let t = text.trim();
    if t == "end" {
        return len.checked_sub(1);
    }
    if let Some(rest) = t.strip_prefix("end-") {
        let n: usize = rest.trim().parse().ok()?;
        return len.checked_sub(1 + n);
    }
    t.parse::<usize>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(script: &str) -> (Interp, Result<(), TclError>) {
        let mut interp = Interp::new();
        let result = interp.run(script).map(|_| ());
        (interp, result)
    }

    #[test]
    fn variables_substitution_and_puts() {
        let (interp, result) = run("set x 3\nincr x 4\nputs \"x=$x\"\nputs [expr {$x * 2}]");
        assert!(result.is_ok(), "{:?}", result);
        assert_eq!(interp.output(), &["x=7".to_string(), "14".to_string()]);
    }

    #[test]
    fn if_elseif_else_branches() {
        let (interp, _) = run(
            "set v 5\n\
             if {$v > 10} { puts big } elseif {$v > 3} { puts mid } else { puts small }",
        );
        assert_eq!(interp.output(), &["mid".to_string()]);
    }

    #[test]
    fn while_and_for_loops() {
        let (interp, _) = run(
            "set i 0\nset sum 0\n\
             while {$i < 5} { incr i; incr sum $i }\n\
             set acc 0\nfor {set j 1} {$j <= 3} {incr j} { incr acc $j }\n\
             puts \"$sum/$acc\"",
        );
        assert_eq!(interp.output(), &["15/6".to_string()]);
    }

    #[test]
    fn proc_with_return_and_recursion() {
        let (interp, _) = run(
            "proc fact {n} {\n\
               if {$n <= 1} { return 1 }\n\
               return [expr {$n * [fact [expr {$n - 1}]]}]\n\
             }\n\
             puts [fact 5]",
        );
        assert_eq!(interp.output(), &["120".to_string()]);
    }

    #[test]
    fn list_commands_roundtrip() {
        let (interp, _) = run(
            "set l [list a {b c} d]\n\
             puts [llength $l]\n\
             puts [lindex $l 1]\n\
             puts [lrange $l 0 1]",
        );
        assert_eq!(
            interp.output(),
            &["3".to_string(), "b c".to_string(), "a {b c}".to_string()]
        );
    }

    #[test]
    fn foreach_over_list() {
        let (interp, _) = run(
            "set total 0\nforeach v {1 2 3 4} { incr total $v }\nputs $total",
        );
        assert_eq!(interp.output(), &["10".to_string()]);
    }

    #[test]
    fn catch_captures_error_message() {
        let (interp, _) = run(
            "set code [catch { error boom } msg]\nputs \"$code:$msg\"",
        );
        assert_eq!(interp.output(), &["1:boom".to_string()]);
    }

    #[test]
    fn unknown_command_is_error() {
        let (_interp, result) = run("no_such_command 1 2");
        assert!(matches!(result, Err(TclError::Msg(_))));
    }

    #[test]
    fn break_and_continue_control_loop() {
        let (interp, _) = run(
            "set out {}\n\
             for {set i 0} {$i < 6} {incr i} {\n\
               if {$i == 2} { continue }\n\
               if {$i == 5} { break }\n\
               lappend out $i\n\
             }\n\
             puts $out",
        );
        assert_eq!(interp.output(), &["0 1 3 4".to_string()]);
    }

    #[test]
    fn argv_seeded_from_cli() {
        let mut interp = Interp::new();
        interp.set_argv(&["a.sv".to_string(), "-T".to_string()]);
        let _ = interp.run("puts \"$argc:$argv\"");
        assert_eq!(interp.output(), &["2:a.sv -T".to_string()]);
    }
}
