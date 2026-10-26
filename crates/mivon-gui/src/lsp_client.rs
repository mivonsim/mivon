//! LSP client — GUI terhubung ke LSP SERVER Mivon sendiri (`mivon --lsp`,
//! mivon-env::lsp berbasis tower-lsp) via JSON-RPC/stdio.
//!
//! - Server di-spawn sebagai subprocess; streaming stdout di-parse per frame
//!   `Content-Length: N` (JSON-RPC).
//! - Request (hover/definition) diberi id oleh UI; reply tidak ber-id 1
//!   diteruskan ke UI yang memetakan id → pending (hover/goto).
//! - Diagnostics (`textDocument/publishDiagnostics`) diteruskan ke UI sebagai
//!   `GuiEvent::LspDiagnostics`.
//!
//! Satu tanggung jawab: transport LSP. Dekode semantik (Hover/Definition/
//! Diagnostic) dilakukan di sisi UI (app.rs) memakai lsp-types.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};

use serde_json::Value;

use crate::state::{DiagEntry, DiagLevel, GuiEvent};

/// Perintah UI → client LSP (dikirim via channel; thread output menulis ke
/// stdin server).
#[derive(Debug)]
pub enum LspCmd {
    /// Spawn server + handshake (initialize/id=1, initialized). Wajib pertama.
    Start {
        bin: PathBuf,
        workspace: PathBuf,
    },
    /// Teks dokumen sinkron penuh (didOpen/didChange/didClose).
    DidOpen {
        path: PathBuf,
        text: String,
    },
    DidChange {
        path: PathBuf,
        text: String,
    },
    DidClose {
        path: PathBuf,
    },
    /// Request hover/definition — id dipakai UI utk memetakan reply.
    Hover {
        id: u64,
        path: PathBuf,
        line: u32,
        character: u32,
    },
    Goto {
        id: u64,
        path: PathBuf,
        line: u32,
        character: u32,
    },
    /// LSP-25: completion dari server (simbol lintas file), bukan hanya
    /// kandidat lokal AST + keyword.
    Completion {
        id: u64,
        path: PathBuf,
        line: u32,
        character: u32,
    },
    /// Matikan server (shutdown + exit + kill).
    Stop,
}

/// Handle client: kirim via `tx`, terima event via `rx`.
pub struct LspClient {
    pub tx: Sender<LspCmd>,
    pub rx: Receiver<GuiEvent>,
}

fn uri(path: &PathBuf) -> String {
    lsp_types::Url::from_file_path(path)
        .map(|u| u.to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

fn pos(line: u32, character: u32) -> Value {
    serde_json::json!({ "line": line, "character": character })
}

fn writer_send(w: &mut ChildStdin, msg: &str) {
    let _ = write!(w, "{}\r\n", msg);
    let _ = w.flush();
}

/// Parameter permintaan completion: posisi caret + triggerKind 1 (invoked)
/// supaya server boleh mengembalikan daftar penuh; UI yang memfilter prefix.
fn completion_params(path: &PathBuf, line: u32, character: u32) -> Value {
    serde_json::json!({
        "textDocument": { "uri": uri(path) },
        "position": pos(line, character),
        "context": { "triggerKind": 1 },
    })
}

fn request(w: &mut ChildStdin, id: u64, method: &str, params: Value) {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    let body = body.to_string();
    writer_send(
        w,
        &format!("Content-Length: {}\r\n\r\n{}", body.len(), body),
    );
}

fn notify(w: &mut ChildStdin, method: &str, params: Value) {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    });
    let body = body.to_string();
    writer_send(
        w,
        &format!("Content-Length: {}\r\n\r\n{}", body.len(), body),
    );
}

/// Path binary LSP server: env `MIVON_LSP_BIN`, fallback binary `mivon` di
/// direktori yang sama dengan executable GUI (target/debug).
pub fn lsp_binary_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MIVON_LSP_BIN") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let bin = dir.join("mivon");
    if bin.is_file() {
        Some(bin)
    } else {
        None
    }
}

impl LspClient {
    /// Spawn client: proses server + thread input (baca stdout) + thread
    /// output (tulis stdin). `Start` dikirim segera (handshake pertama).
    pub fn start(bin: PathBuf, workspace: PathBuf) -> LspClient {
        let (tx, rx) = channel::<LspCmd>();
        let (tx_gui, rx_gui) = channel::<GuiEvent>();
        std::thread::Builder::new()
            .name("mivon-lsp-out".into())
            .spawn(move || out_loop(rx, tx_gui))
            .ok();
        let _ = tx.send(LspCmd::Start { bin, workspace });
        LspClient { tx, rx: rx_gui }
    }
}

/// Thread output: terima `LspCmd`, spawn child, tulis pesan ke stdin.
fn out_loop(rx: Receiver<LspCmd>, tx_gui: Sender<GuiEvent>) {
    let mut writer: Option<ChildStdin> = None;
    // Child dipegang langsung (stdout/stdin sudah di-take) — dipakai utk kill
    // di akhir & saat Stop.
    let mut child: Option<Child> = None;
    while let Ok(cmd) = rx.recv() {
        match cmd {
            LspCmd::Start { bin, workspace } => {
                let cwd = workspace
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_default();
                match Command::new(&bin)
                    .arg("--lsp")
                    .current_dir(cwd)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                {
                    Ok(mut c) => {
                        let out = c.stdout.take();
                        let stdin = c.stdin.take();
                        child = Some(c);
                        if let Some(out) = out {
                            let tg = tx_gui.clone();
                            std::thread::Builder::new()
                                .name("mivon-lsp-in".into())
                                .spawn(move || in_loop(out, tg))
                                .ok();
                        }
                        // Ambil stdin SEKALI (menjadi `writer` utk cmds berikut).
                        let mut w = stdin;
                        if let Some(w_ref) = w.as_mut() {
                            // Handshake.
                            let root = lsp_types::Url::from_file_path(&workspace)
                                .map(|u| u.to_string())
                                .unwrap_or_else(|_| String::new());
                            request(
                                w_ref,
                                1,
                                "initialize",
                                serde_json::json!({
                                    "processId": std::process::id(),
                                    "rootUri": root,
                                    "capabilities": {},
                                }),
                            );
                            notify(w_ref, "initialized", serde_json::json!({}));
                            let _ = tx_gui.send(GuiEvent::LspStarted(bin.display().to_string()));
                        }
                        writer = w;
                    }
                    Err(e) => {
                        let _ = tx_gui.send(GuiEvent::LspStopped(format!(
                            "gagal spawn LSP server {}: {}",
                            bin.display(),
                            e
                        )));
                    }
                }
            }
            LspCmd::DidOpen { path, text } => {
                if let Some(w) = writer.as_mut() {
                    notify(
                        w,
                        "textDocument/didOpen",
                        serde_json::json!({
                            "textDocument": {
                                "uri": uri(&path),
                                "languageId": "systemverilog",
                                "version": 1,
                                "text": text,
                            }
                        }),
                    );
                }
            }
            LspCmd::DidChange { path, text } => {
                if let Some(w) = writer.as_mut() {
                    notify(
                        w,
                        "textDocument/didChange",
                        serde_json::json!({
                            "textDocument": { "uri": uri(&path), "version": 2 },
                            "contentChanges": [{ "text": text }],
                        }),
                    );
                }
            }
            LspCmd::DidClose { path } => {
                if let Some(w) = writer.as_mut() {
                    notify(
                        w,
                        "textDocument/didClose",
                        serde_json::json!({
                            "textDocument": { "uri": uri(&path) }
                        }),
                    );
                }
            }
            LspCmd::Hover {
                id,
                path,
                line,
                character,
            } => {
                if let Some(w) = writer.as_mut() {
                    let p = pos(line, character);
                    request(
                        w,
                        id,
                        "textDocument/hover",
                        serde_json::json!({ "textDocument": { "uri": uri(&path) }, "position": p }),
                    );
                }
            }
            LspCmd::Goto {
                id,
                path,
                line,
                character,
            } => {
                if let Some(w) = writer.as_mut() {
                    let p = pos(line, character);
                    request(
                        w,
                        id,
                        "textDocument/definition",
                        serde_json::json!({ "textDocument": { "uri": uri(&path) }, "position": p }),
                    );
                }
            }
            LspCmd::Completion {
                id,
                path,
                line,
                character,
            } => {
                if let Some(w) = writer.as_mut() {
                    request(
                        w,
                        id,
                        "textDocument/completion",
                        completion_params(&path, line, character),
                    );
                }
            }
            LspCmd::Stop => {
                if let Some(w) = writer.as_mut() {
                    request(w, 999, "shutdown", serde_json::json!({}));
                    notify(w, "exit", serde_json::json!({}));
                }
                break;
            }
        }
    }
    // Matikan proses tersisa (juga menutup pipe → thread input selesai).
    if let Some(mut c) = child {
        let _ = c.kill();
        let _ = c.wait();
    }
}

/// Thread input: baca stdout server, parse frame `Content-Length`, klasifikasi
/// pesan, kirim ke UI.
fn in_loop(mut pipe: ChildStdout, tx_gui: Sender<GuiEvent>) {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                while let Some(body) = take_frame(&mut buf) {
                    handle_frame(&body, &tx_gui);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    // EOF/error = server mati (crash/exit). Tanpa notifikasi ini UI tetap
    // mengira LSP siap, request menggantung menumpuk, dan cleanup di app.rs
    // tidak pernah jalan.
    let _ = tx_gui.send(GuiEvent::LspStopped("server berhenti (EOF)".to_string()));
}

/// Ambil satu body JSON-RPC dari buffer frame; `None` bila belum lengkap.
/// Format: `Content-Length: N\r\n\r\n<body N bytes>`. Kembalikan juga ukuran
/// byte yang tersisa.
pub fn take_frame(buf: &mut Vec<u8>) -> Option<String> {
    let raw = buf.as_slice();
    let header_end = find_header_end(raw)?;
    let header = std::str::from_utf8(&raw[..header_end]).ok()?;
    let content_length = header
        .lines()
        .find_map(|l| {
            l.strip_prefix("Content-Length:")
                .or_else(|| l.strip_prefix("Content-Length :"))
        })
        .and_then(|v| v.trim().parse::<usize>().ok())?;
    let body_start = header_end + 4; // \r\n\r\n
    if raw.len() < body_start + content_length {
        return None;
    }
    let body = std::str::from_utf8(&raw[body_start..body_start + content_length])
        .ok()?
        .to_string();
    buf.drain(..body_start + content_length);
    Some(body)
}

fn find_header_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Klasifikasi satu pesan: request-response ber-id>1 → `LspReply`; notifikasi
/// publishDiagnostics → `LspDiagnostics` (di-konversi); lainnya diabaikan.
fn handle_frame(body: &str, tx: &Sender<GuiEvent>) {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return;
    };
    if let Some(id) = v.get("id").and_then(Value::as_u64) {
        if id == 1 || id == 999 {
            return; // handshake/shutdown internal
        }
        let _ = tx.send(GuiEvent::LspReply {
            id,
            value: v.get("result").cloned(),
        });
        return;
    }
    if let Some(method) = v.get("method").and_then(Value::as_str) {
        if method == "textDocument/publishDiagnostics" {
            let params = match v.get("params") {
                Some(p) => p,
                None => return,
            };
            let uri_str = params
                .get("textDocument")
                .and_then(|d| d.get("uri"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let file = lsp_types::Url::parse(uri_str)
                .ok()
                .and_then(|u| u.to_file_path().ok())
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| uri_str.to_string());
            let diags: Vec<DiagEntry> = params
                .get("diagnostics")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|d| diagnostic_to_entry(&file, d))
                        .collect()
                })
                .unwrap_or_default();
            let _ = tx.send(GuiEvent::LspDiagnostics(file, diags));
        }
        // window/logMessage, showMessage, dll — diabaikan (transport issue
        // hanya dari LspStarted/LspStopped).
    }
}

/// Konversi `Diagnostic` JSON → `DiagEntry` (line 0-based → 1-based).
fn diagnostic_to_entry(file: &str, d: &Value) -> Option<DiagEntry> {
    let line = d
        .pointer("/range/start/line")
        .and_then(Value::as_u64)
        .map(|l| l as usize + 1)?;
    let message = d.get("message").and_then(Value::as_str)?.to_string();
    let level = match d.get("severity").and_then(Value::as_u64) {
        Some(1) => DiagLevel::Error,
        Some(2) => DiagLevel::Warning,
        _ => DiagLevel::Info,
    };
    Some(DiagEntry {
        file: file.to_string(),
        line,
        message,
        level,
        fix: None,
    })
}

/// Extrak teks hover (Hover.contents) → string; None bila kosong.
pub fn hover_text(hover: &lsp_types::Hover) -> Option<String> {
    use lsp_types::HoverContents;
    let text = match &hover.contents {
        HoverContents::Scalar(m) => marked_to_text(m),
        HoverContents::Array(arr) => arr
            .iter()
            .map(marked_to_text)
            .collect::<Vec<_>>()
            .join("\n"),
        HoverContents::Markup(m) => m.value.clone(),
    };
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

/// `MarkedString` → teks polos (String langsung, atau LanguageString.value).
fn marked_to_text(m: &lsp_types::MarkedString) -> String {
    match m {
        lsp_types::MarkedString::String(s) => s.clone(),
        lsp_types::MarkedString::LanguageString(ls) => ls.value.clone(),
    }
}

/// Batas kandidat completion dari server (UI memfilter prefix lalu
/// memotong lagi ke 50 untuk popup).
const MAX_COMPLETION_ITEMS: usize = 200;

/// Extrak label dari reply permintaan completion. Server boleh menjawab
/// array CompletionItem langsung atau CompletionList berisi items; label
/// kosong/duplikat dibuang, urutan server dipertahankan.
pub fn completion_items(value: &Value) -> Vec<String> {
    let arr = value
        .as_array()
        .or_else(|| value.get("items").and_then(Value::as_array));
    let Some(arr) = arr else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for item in arr {
        let Some(label) = item.get("label").and_then(Value::as_str) else {
            continue;
        };
        let label = label.trim();
        if label.is_empty() || out.iter().any(|s| s == label) {
            continue;
        }
        out.push(label.to_string());
        if out.len() >= MAX_COMPLETION_ITEMS {
            break;
        }
    }
    out
}

/// Gabungkan kandidat lokal (sudah terfilter prefix) dengan kandidat server.
///
/// Kandidat server difilter prefix di sisi klien: posisi yang dikirim ke
/// server bisa tertinggal dari teks popup (didChange di-debounce) atau caret
/// berada di tengah kata, jadi daftar server tidak boleh dipercaya mentah.
/// Kandidat lokal dipertahankan di depan; duplikat dibuang; hasil dipotong
/// ke cap.
pub fn merge_completion(
    local: &[String],
    server: &[String],
    prefix: &str,
    cap: usize,
) -> Vec<String> {
    let matches = |s: &str| {
        s.get(..prefix.len())
            .map(|h| h.eq_ignore_ascii_case(prefix))
            .unwrap_or(false)
    };
    let mut out: Vec<String> = local.to_vec();
    for item in server {
        if !matches(item) || out.iter().any(|s| s == item) {
            continue;
        }
        out.push(item.clone());
        if out.len() >= cap {
            break;
        }
    }
    out.truncate(cap);
    out
}

/// Extrak target definisi dari reply `textDocument/definition` — lokasi
/// pertama (0-based → line 1-based). None bila kosong.
pub fn definition_target(value: &Value) -> Option<(PathBuf, usize)> {
    let loc = value.get(0)?; // array hanyalah bentuk yang dihasilkan server
    let line0 = loc
        .pointer("/range/start/line")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let uri = loc.get("uri").and_then(Value::as_str)?;
    let path = lsp_types::Url::parse(uri).ok()?.to_file_path().ok()?;
    Some((path, line0 as usize + 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn framing_single_message() {
        let body = r#"{"jsonrpc":"2.0","method":"x","params":{}}"#;
        let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let mut buf: Vec<u8> = frame.as_bytes().to_vec();
        assert_eq!(take_frame(&mut buf).as_deref(), Some(body));
        assert!(buf.is_empty(), "frame habis dikonsumsi");
    }

    #[test]
    fn framing_split_two_messages() {
        let body = r#"{"id":2,"result":null}"#;
        let mut wire: Vec<u8> = Vec::new();
        wire.extend_from_slice(
            format!("Content-Length: {}\r\n\r\n{}", body.len(), body).as_bytes(),
        );
        wire.extend_from_slice(
            format!("Content-Length: {}\r\n\r\n{}", body.len(), body).as_bytes(),
        );
        let mut buf = wire;
        let a = take_frame(&mut buf).expect("msg1");
        let b = take_frame(&mut buf).expect("msg2");
        assert_eq!(a, body);
        assert_eq!(b, body);
        assert!(buf.is_empty());
    }

    #[test]
    fn framing_incomplete_returns_none() {
        let mut buf: Vec<u8> = b"Content-Length: 100\r\n\r\nshort".to_vec();
        assert!(take_frame(&mut buf).is_none());
    }

    #[test]
    fn diagnostics_json_to_entries() {
        let frame = r##"{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{
            "textDocument":{"uri":"file:///tmp/a.sv"},
            "diagnostics":[
                {"range":{"start":{"line":4,"character":0},"end":{"line":4,"character":3}},
                 "severity":1,"message":"syntax error"}
            ]}}"##;
        let v: Value = serde_json::from_str(frame).unwrap();
        // handle_frame via channel
        let (tx, rx) = channel::<GuiEvent>();
        handle_frame(&v.to_string(), &tx);
        match rx.recv().unwrap() {
            GuiEvent::LspDiagnostics(file, diags) => {
                assert_eq!(file, "/tmp/a.sv");
                assert_eq!(diags.len(), 1);
                assert_eq!(diags[0].line, 5); // 0-based → 1-based
                assert_eq!(diags[0].level, DiagLevel::Error);
                assert_eq!(diags[0].message, "syntax error");
            }
            _ => panic!("bukan LspDiagnostics"),
        }
    }

    #[test]
    fn completion_items_accepts_array_and_list() {
        let arr: Value = serde_json::from_str(r#"[{"label":"alpha"},{"label":"beta"}]"#).unwrap();
        assert_eq!(
            completion_items(&arr),
            vec!["alpha".to_string(), "beta".to_string()]
        );
        let list: Value =
            serde_json::from_str(r#"{"isIncomplete":false,"items":[{"label":"gamma"}]}"#).unwrap();
        assert_eq!(completion_items(&list), vec!["gamma".to_string()]);
    }

    #[test]
    fn completion_items_dedupes_and_skips_invalid() {
        let v: Value = serde_json::from_str(
            r#"[{"label":"a"},{"label":"a"},{"label":"  "},{"detail":"x"},{"label":" b "}]"#,
        )
        .unwrap();
        assert_eq!(completion_items(&v), vec!["a".to_string(), "b".to_string()]);
        assert!(completion_items(&Value::Null).is_empty());
        assert!(completion_items(&serde_json::json!({ "items": "x" })).is_empty());
    }

    #[test]
    fn completion_items_caps_at_limit() {
        // Batas 200 adalah kontrak UI (popup memotong lagi ke 50) — dikunci
        // eksplisit supaya mengubah konstanta menggagalkan test.
        assert_eq!(MAX_COMPLETION_ITEMS, 200);
        let arr: Vec<Value> = (0..240)
            .map(|i| serde_json::json!({ "label": format!("s{i}") }))
            .collect();
        let items = completion_items(&Value::Array(arr));
        assert_eq!(items.len(), 200);
        assert_eq!(items[0], "s0");
        assert_eq!(items[199], "s199");
    }

    #[test]
    fn merge_completion_filters_prefix_and_keeps_local_first() {
        let local = vec!["clock".to_string()];
        let server = vec![
            "clock".to_string(),
            "clocking".to_string(),
            "case".to_string(),
        ];
        let out = merge_completion(&local, &server, "clo", 50);
        assert_eq!(out, vec!["clock".to_string(), "clocking".to_string()]);
    }

    #[test]
    fn merge_completion_drops_stale_prefix_items() {
        // Server bisa masih memakai teks lama (didChange di-debounce) —
        // item yang tidak cocok prefix popup wajib dibuang.
        let local = vec!["clk".to_string()];
        let server = vec!["case".to_string(), "class".to_string()];
        let out = merge_completion(&local, &server, "clk", 50);
        assert_eq!(out, vec!["clk".to_string()]);
    }

    #[test]
    fn merge_completion_cap_keeps_local_candidates() {
        let local = vec!["a1".to_string(), "a2".to_string()];
        let server = vec!["a3".to_string()];
        let out = merge_completion(&local, &server, "a", 2);
        assert_eq!(out, vec!["a1".to_string(), "a2".to_string()]);
    }

    #[test]
    fn completion_params_carries_uri_position_and_trigger() {
        let path = PathBuf::from("/tmp/a.sv");
        let expect = lsp_types::Url::from_file_path(&path).unwrap().to_string();
        let p = completion_params(&path, 3, 7);
        assert_eq!(
            p.pointer("/textDocument/uri").and_then(Value::as_str),
            Some(expect.as_str())
        );
        assert_eq!(p.pointer("/position/line").and_then(Value::as_u64), Some(3));
        assert_eq!(
            p.pointer("/position/character").and_then(Value::as_u64),
            Some(7)
        );
        assert_eq!(
            p.pointer("/context/triggerKind").and_then(Value::as_u64),
            Some(1)
        );
    }

    /// E2E: client nyata ↔ server nyata (`mivon --lsp`). DIDOpen file SV
    /// invalid → server harus balas publishDiagnostics dengan error.
    /// Dilewati (bukan gagal) bila binary server belum dibangun.
    #[test]
    fn lsp_end_to_end_with_real_server() {
        let Some(bin) = lsp_binary_path() else {
            eprintln!("SKIP: binary 'mivon' tidak ditemukan (build dulu: cargo build --bin mivon)");
            return;
        };
        let ws = std::env::temp_dir().join(format!("mivon_lsp_e2e_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&ws);
        let file = ws.join("bad.sv");
        let src = "module bad;\n  logic q = ;\nendmodule\n";
        std::fs::write(&file, src).unwrap();

        let client = LspClient::start(bin, ws.clone());
        // Tunggu handshake selesai (server pertama bisa lambat, beri 30s).
        let mut ready = false;
        for _ in 0..30 {
            match client.rx.recv_timeout(Duration::from_secs(1)) {
                Ok(GuiEvent::LspStarted(_)) => {
                    ready = true;
                    break;
                }
                Ok(GuiEvent::LspStopped(reason)) => {
                    panic!("server berhenti sebelum siap: {}", reason);
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        if !ready {
            let _ = std::fs::remove_dir_all(&ws);
            panic!("timeout menunggu LspStarted (30s)");
        }

        client
            .tx
            .send(LspCmd::DidOpen {
                path: file.clone(),
                text: src.to_string(),
            })
            .unwrap();

        // Harus terima diagnostics (SV invalid) dalam 30s.
        let mut got = false;
        for _ in 0..30 {
            match client.rx.recv_timeout(Duration::from_secs(1)) {
                Ok(GuiEvent::LspDiagnostics(f, diags)) => {
                    let expect_path = file.display().to_string();
                    assert_eq!(f, expect_path, "file diagnostics cocok");
                    assert!(!diags.is_empty(), "SV invalid harus punya diagnostic");
                    got = true;
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        client.tx.send(LspCmd::Stop).unwrap();
        let _ = std::fs::remove_dir_all(&ws);
        assert!(
            got,
            "tidak menerima publishDiagnostics dari server dalam 30s"
        );
    }
}
