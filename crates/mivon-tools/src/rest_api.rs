//! FEAT-04: REST API — Basic HTTP server skeleton for automation.
//!
//! Provides a minimal HTTP server using only std::net for exposing
//! mivon tools via REST endpoints. No external dependencies needed.
//!
//! Endpoints:
//! - GET /health — health check
//! - GET /api/v1/status — server status
//! - POST /api/v1/compile — compile source
//! - POST /api/v1/simulate — run simulation
//! - GET /api/v1/modules — list modules
//!
//! Dashboard (FEAT-03) menambahkan route-nya lewat `add_route`.
//!
//! Batas keamanan yang dipasang di layer transport:
//! - satu thread per koneksi (klien lambat tidak memblokir accept loop),
//! - read timeout 5 detik + batas 8 KiB header,
//! - panic di handler ditangkap → 500, listener tetap hidup,
//! - POST dengan `Origin` lintas-host ditolak 403 (proteksi CSRF dasar
//!   untuk server yang bind ke localhost),
//! - semua respons `Cache-Control: no-store` (data sesi selalu segar).
//!
//! Usage:
//! ```no_run
//! use mivon_tools::rest_api::RestServer;
//! let server = RestServer::new("127.0.0.1:8080");
//! server.start().unwrap();
//! ```

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

type Handler = Arc<dyn Fn(&str, &str) -> String + Send + Sync>;

/// REST API server.
pub struct RestServer {
    addr: String,
    routes: Arc<HashMap<String, Handler>>,
}

/// API response.
#[derive(Debug, Clone)]
pub struct ApiResponse {
    pub status: u16,
    pub body: String,
    pub content_type: String,
}

impl ApiResponse {
    pub fn ok(body: &str) -> Self {
        ApiResponse {
            status: 200,
            body: body.to_string(),
            content_type: "application/json".to_string(),
        }
    }

    pub fn json(body: &str) -> Self {
        ApiResponse {
            status: 200,
            body: body.to_string(),
            content_type: "application/json".to_string(),
        }
    }

    /// Respons HTML (dashboard).
    pub fn html(body: &str) -> Self {
        ApiResponse {
            status: 200,
            body: body.to_string(),
            content_type: "text/html; charset=utf-8".to_string(),
        }
    }

    /// 201 Created (submit sesi dashboard).
    pub fn created(body: &str) -> Self {
        ApiResponse {
            status: 201,
            body: body.to_string(),
            content_type: "application/json".to_string(),
        }
    }

    /// Error JSON — pesan di-escape agar body selalu JSON valid walau pesan
    /// berisi kutip/backslash (mis. pesan kuota dari SimServer).
    pub fn error(status: u16, body: &str) -> Self {
        ApiResponse {
            status,
            body: serde_json::json!({ "error": body }).to_string(),
            content_type: "application/json".to_string(),
        }
    }

    pub fn not_found() -> Self {
        Self::error(404, "not found")
    }

    pub fn to_http_response(&self) -> String {
        let status_text = match self.status {
            200 => "OK",
            201 => "Created",
            400 => "Bad Request",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            409 => "Conflict",
            429 => "Too Many Requests",
            500 => "Internal Server Error",
            503 => "Service Unavailable",
            _ => "Unknown",
        };
        format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{}",
            self.status,
            status_text,
            self.content_type,
            self.body.len(),
            self.body,
        )
    }
}

impl RestServer {
    pub fn new(addr: &str) -> Self {
        let mut routes: HashMap<String, Handler> = HashMap::new();

        // Health check
        routes.insert(
            "GET /health".to_string(),
            Arc::new(|_, _| {
                ApiResponse::ok(r#"{"status":"healthy","version":"0.3.0"}"#).to_http_response()
            }),
        );

        // Server status
        routes.insert(
            "GET /api/v1/status".to_string(),
            Arc::new(|_, _| {
                ApiResponse::json(r#"{"server":"mivon","status":"running","uptime":"unknown"}"#)
                    .to_http_response()
            }),
        );

        RestServer {
            addr: addr.to_string(),
            routes: Arc::new(routes),
        }
    }

    /// Add a custom route.
    pub fn add_route<F>(&mut self, method: &str, path: &str, handler: F)
    where
        F: Fn(&str, &str) -> String + Send + Sync + 'static,
    {
        let key = format!("{} {}", method, path);
        Arc::make_mut(&mut self.routes).insert(key, Arc::new(handler));
    }

    /// Dispatch satu request tanpa socket (dipakai test & handler internal).
    ///
    /// Route dicocokkan TANPA query string; handler tetap menerima path penuh
    /// agar bisa mem-parse parameter (`parse_query`). Panic handler tidak
    /// menjatuhkan proses: dikonversi menjadi 500.
    pub fn handle_request(&self, method: &str, path: &str) -> String {
        dispatch(&self.routes, method, path)
    }

    /// Start the server (blocking).
    ///
    /// Satu thread per koneksi; error satu koneksi tidak menghentikan server.
    pub fn start(&self) -> Result<(), String> {
        let listener = TcpListener::bind(&self.addr).map_err(|e| format!("bind failed: {}", e))?;

        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let routes = self.routes.clone();
                    std::thread::spawn(move || {
                        if let Err(e) = handle_connection(routes, stream) {
                            eprintln!("connection error: {}", e);
                        }
                    });
                }
                Err(e) => {
                    eprintln!("accept error: {}", e);
                }
            }
        }
        Ok(())
    }
}

/// Batas total byte header (request line + header) — mencegah klien mengirim
/// header raksasa tanpa akhir.
const MAX_HEADER_BYTES: usize = 8 * 1024;

/// Timeout baca per koneksi.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

fn dispatch(routes: &HashMap<String, Handler>, method: &str, path: &str) -> String {
    let route_path = path.split('?').next().unwrap_or(path);
    let key = format!("{} {}", method, route_path);
    match routes.get(&key) {
        Some(handler) => {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(method, path)));
            match result {
                Ok(resp) => resp,
                Err(_) => ApiResponse::error(500, "internal server error").to_http_response(),
            }
        }
        None => ApiResponse::not_found().to_http_response(),
    }
}

/// Proteksi CSRF dasar: bila request membawa `Origin`, host-nya harus sama
/// dengan header `Host` (server bind lokal). Origin `null` ditolak.
fn origin_allowed(origin: &str, host: &str) -> bool {
    if host.is_empty() || origin == "null" {
        return false;
    }
    origin
        .split_once("://")
        .map(|(_, rest)| rest == host)
        .unwrap_or(false)
}

fn handle_connection(
    routes: Arc<HashMap<String, Handler>>,
    mut stream: TcpStream,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(READ_TIMEOUT))
        .map_err(|e| e.to_string())?;
    let reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut method = String::new();
    let mut path = String::new();
    let mut headers: HashMap<String, String> = HashMap::new();
    let mut total = 0usize;

    for line in reader.lines() {
        let line = line.map_err(|e| e.to_string())?;
        total += line.len() + 2;
        if total > MAX_HEADER_BYTES {
            let resp = ApiResponse::error(400, "header terlalu besar").to_http_response();
            stream
                .write_all(resp.as_bytes())
                .map_err(|e| e.to_string())?;
            return Ok(());
        }
        if line.is_empty() {
            break;
        }
        if line.starts_with("GET ") || line.starts_with("POST ") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                method = parts[0].to_string();
                path = parts[1].to_string();
            }
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }

    // CSRF: POST lintas-origin dari halaman lain ditolak sebelum dispatch.
    if method == "POST" {
        let host = headers.get("host").map(|s| s.as_str()).unwrap_or("");
        if let Some(origin) = headers.get("origin") {
            if !origin_allowed(origin, host) {
                let resp = ApiResponse::error(403, "origin tidak diizinkan").to_http_response();
                stream
                    .write_all(resp.as_bytes())
                    .map_err(|e| e.to_string())?;
                return Ok(());
            }
        }
        if headers.get("sec-fetch-site").map(|s| s.as_str()) == Some("cross-site") {
            let resp = ApiResponse::error(403, "cross-site request ditolak").to_http_response();
            stream
                .write_all(resp.as_bytes())
                .map_err(|e| e.to_string())?;
            return Ok(());
        }
    }

    let response = dispatch(&routes, &method, &path);
    stream
        .write_all(response.as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Parse query string from URL path (percent-decoding + '+' → spasi).
///
/// Form dashboard mengirim lewat URLSearchParams, sehingga koma pada daftar
/// file datang sebagai %2C dan spasi sebagai '+'. Tanpa decode, nilai akan
/// salah (mis. "a%2Cb.sv" dianggap satu nama file).
pub fn parse_query(path: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();
    if let Some(query) = path.split('?').nth(1) {
        for pair in query.split('&') {
            if let Some((key, val)) = pair.split_once('=') {
                params.insert(percent_decode(key), percent_decode(val));
            }
        }
    }
    params
}

/// Decode percent-encoding; urutan '%' yang tidak valid dibiarkan apa adanya.
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                    }
                    None => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Format JSON response dari pasangan key/value (string).
///
/// Nilai di-escape lewat serde_json — jangan menyusun JSON dengan format!.
pub fn json_object(fields: &[(&str, &str)]) -> String {
    let map: serde_json::Map<String, serde_json::Value> = fields
        .iter()
        .map(|(k, v)| {
            (
                (*k).to_string(),
                serde_json::Value::String((*v).to_string()),
            )
        })
        .collect();
    serde_json::Value::Object(map).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_response_ok() {
        let resp = ApiResponse::ok(r#"{"ok":true}"#);
        assert_eq!(resp.status, 200);
        assert!(resp.to_http_response().contains("200 OK"));
    }

    #[test]
    fn test_api_response_error() {
        let resp = ApiResponse::error(500, "boom");
        assert_eq!(resp.status, 500);
        assert!(resp.body.contains("boom"));
    }

    #[test]
    fn test_not_found() {
        let resp = ApiResponse::not_found();
        assert_eq!(resp.status, 404);
    }

    #[test]
    fn test_parse_query() {
        let q = parse_query("/api/v1/search?q=test&limit=10");
        assert_eq!(q.get("q").unwrap(), "test");
        assert_eq!(q.get("limit").unwrap(), "10");
    }

    #[test]
    fn test_parse_query_decodes_percent_and_plus() {
        let q = parse_query("/api/v1/sessions?user=alice+bob&files=a%2Cb.sv&x=%ZZ");
        assert_eq!(q.get("user").unwrap(), "alice bob");
        assert_eq!(q.get("files").unwrap(), "a,b.sv");
        assert_eq!(q.get("x").unwrap(), "%ZZ");
    }

    #[test]
    fn test_percent_decode_utf8() {
        assert_eq!(percent_decode("caf%C3%A9"), "café");
        assert_eq!(percent_decode("100%"), "100%");
    }

    #[test]
    fn test_json_object() {
        let j = json_object(&[("name", "mivon"), ("version", "0.3.0")]);
        assert!(j.contains(r#""name":"mivon""#));
        assert!(j.contains(r#""version":"0.3.0""#));
    }

    #[test]
    fn json_object_escapes_values() {
        let j = json_object(&[("x", "a\"b\\c")]);
        let parsed: serde_json::Value = serde_json::from_str(&j).unwrap();
        assert_eq!(parsed["x"], "a\"b\\c");
    }

    #[test]
    fn test_http_response_format() {
        let resp = ApiResponse::json(r#"{"status":"ok"}"#);
        let http = resp.to_http_response();
        assert!(http.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(http.contains("Content-Type: application/json"));
        assert!(http.contains("Cache-Control: no-store"));
        assert!(http.ends_with(r#"{"status":"ok"}"#));
    }

    #[test]
    fn test_add_route() {
        let mut server = RestServer::new("127.0.0.1:0");
        server.add_route("GET", "/test", |_, _| {
            ApiResponse::ok("test").to_http_response()
        });
        let key = "GET /test".to_string();
        assert!(server.routes.contains_key(&key));
    }

    #[test]
    fn handler_panic_becomes_500() {
        let mut server = RestServer::new("127.0.0.1:0");
        server.add_route("GET", "/boom", |_, _| panic!("handler rusak"));
        let resp = server.handle_request("GET", "/boom");
        assert!(resp.starts_with("HTTP/1.1 500"));
    }

    #[test]
    fn origin_check_allows_same_host() {
        assert!(origin_allowed("http://127.0.0.1:8090", "127.0.0.1:8090"));
        assert!(!origin_allowed("http://evil.example", "127.0.0.1:8090"));
        assert!(!origin_allowed("null", "127.0.0.1:8090"));
        assert!(!origin_allowed("http://127.0.0.1:8090", ""));
    }
}
