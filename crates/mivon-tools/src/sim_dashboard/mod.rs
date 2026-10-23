//! FEAT-03: Web-based simulation control dashboard.
//!
//! Satu listener HTTP (diatas `RestServer`, FEAT-04) melayani:
//! - `GET /` — halaman kontrol HTML (form submit, tabel sesi, tombol cancel)
//! - `GET /api/v1/sessions[?user=]` — daftar sesi (JSON)
//! - `POST /api/v1/sessions?user=&files=&max_time=` — submit sesi baru
//! - `POST /api/v1/cancel?id=` — batalkan sesi
//! - `GET /api/v1/stats` — hitungan status + ringkasan server
//!
//! Semua state ada di `SimServer` (ENT-18); modul ini hanya transport.
//! Eksekusi simulasi di-inject lewat `SimServer::set_runner` — CLI `mivon
//! mweb` memasang runner nyata (elaborasi + engine), sedangkan test memakai
//! runner palsu. Tanpa runner, sesi tetap Running (mode registri).
//! Satu tanggung jawab per file: `api.rs` (routing/handler) dan `page.rs`
//! (HTML) — `mod.rs` hanya menyatukan keduanya.

mod api;
mod page;

use std::sync::Arc;

use crate::rest_api::RestServer;
use crate::sim_server::{ServerConfig, SimServer};

pub use page::{escape_html, render_page};

/// Dashboard web untuk mengendalikan sesi simulasi.
pub struct WebDashboard {
    server: Arc<SimServer>,
}

impl WebDashboard {
    /// Dashboard dengan server baru sesuai konfigurasi.
    pub fn new(config: ServerConfig) -> Self {
        WebDashboard {
            server: Arc::new(SimServer::new(config)),
        }
    }

    /// Dashboard yang memakai server yang sudah ada (mis. dipakai bersama
    /// dengan worker simulasi lain).
    pub fn from_server(server: Arc<SimServer>) -> Self {
        WebDashboard { server }
    }

    /// Akses server (untuk submit/cancel dari kode pemanggil).
    pub fn server(&self) -> &Arc<SimServer> {
        &self.server
    }

    /// HTML dashboard berisi snapshot sesi saat ini.
    pub fn render_html(&self) -> String {
        render_page(&self.server.list_all())
    }

    /// `RestServer` dengan route default + route dashboard.
    pub fn into_rest_server(&self, addr: &str) -> RestServer {
        let mut rest = RestServer::new(addr);
        api::register(&mut rest, self.server.clone());
        rest
    }

    /// Jalankan server (blocking sampai proses dihentikan).
    pub fn serve(&self, addr: &str) -> Result<(), String> {
        self.into_rest_server(addr).start()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rest_api::ApiResponse;

    fn dashboard() -> WebDashboard {
        let d = WebDashboard::new(ServerConfig::default());
        d.server()
            .set_runner(std::sync::Arc::new(|_files, _max_time| Ok(1)));
        d
    }

    /// Dashboard dengan runner yang menahan sesi sampai sender di-drop, agar
    /// status Running/Queued deterministik.
    fn dashboard_held() -> (WebDashboard, std::sync::mpsc::Sender<()>) {
        let d = WebDashboard::new(ServerConfig::default());
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
        d.server().set_runner(std::sync::Arc::new(move |_files, _max_time| {
            let guard = rx.lock().unwrap_or_else(|e| e.into_inner());
            guard.recv().map(|_| 1).map_err(|e| e.to_string())
        }));
        (d, tx)
    }

    /// Ambil body dari respons HTTP mentah (setelah header `\r\n\r\n`).
    fn body(resp: &str) -> &str {
        resp.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("")
    }

    fn status(resp: &str) -> u16 {
        resp.split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }

    #[test]
    fn root_serves_html_page() {
        let d = dashboard();
        let resp = d.into_rest_server("127.0.0.1:0").handle_request("GET", "/");
        assert_eq!(status(&resp), 200);
        assert!(resp.contains("Content-Type: text/html; charset=utf-8"));
        assert!(body(&resp).contains("Mivon Simulation Control"));
    }

    #[test]
    fn unknown_route_is_404() {
        let d = dashboard();
        let resp = d
            .into_rest_server("127.0.0.1:0")
            .handle_request("GET", "/tidak-ada");
        assert_eq!(status(&resp), 404);
    }

    #[test]
    fn query_string_does_not_break_routing() {
        let d = dashboard();
        let resp = d
            .into_rest_server("127.0.0.1:0")
            .handle_request("GET", "/api/v1/sessions?user=alice");
        assert_eq!(status(&resp), 200);
        assert!(body(&resp).contains("\"sessions\""));
    }

    #[test]
    fn create_requires_user() {
        let d = dashboard();
        let rest = d.into_rest_server("127.0.0.1:0");
        let resp = rest.handle_request("POST", "/api/v1/sessions");
        assert_eq!(status(&resp), 400);
        assert!(body(&resp).contains("user"));
        let resp = rest.handle_request("POST", "/api/v1/sessions?user=%20");
        assert_eq!(status(&resp), 400);
    }

    #[test]
    fn create_rejects_bad_max_time() {
        let d = dashboard();
        let rest = d.into_rest_server("127.0.0.1:0");
        for bad in ["abc", "0", "-5"] {
            let resp = rest.handle_request(
                "POST",
                &format!("/api/v1/sessions?user=alice&max_time={}", bad),
            );
            assert_eq!(status(&resp), 400, "max_time={} harus ditolak", bad);
        }
    }

    #[test]
    fn create_and_list_roundtrip() {
        let d = dashboard();
        let rest = d.into_rest_server("127.0.0.1:0");
        let resp = rest.handle_request(
            "POST",
            "/api/v1/sessions?user=alice&files=a.sv,b.sv&max_time=250",
        );
        assert_eq!(status(&resp), 201);
        let created: serde_json::Value = serde_json::from_str(body(&resp)).unwrap();
        assert_eq!(created["user"], "alice");
        assert_eq!(created["max_time"], 250);
        let id = created["id"].as_str().unwrap().to_string();
        assert!(id.starts_with("sim_alice_"));

        let list = rest.handle_request("GET", "/api/v1/sessions?user=alice");
        let list_body: serde_json::Value = serde_json::from_str(body(&list)).unwrap();
        assert_eq!(list_body["count"], 1);
        let s = &list_body["sessions"][0];
        assert_eq!(s["id"], id);
        assert_eq!(s["max_time"], 250);
        assert_eq!(s["source_files"][0], "a.sv");
        assert_eq!(s["source_files"][1], "b.sv");
    }

    #[test]
    fn empty_user_query_is_400() {
        let d = dashboard();
        let rest = d.into_rest_server("127.0.0.1:0");
        let resp = rest.handle_request("GET", "/api/v1/sessions?user=");
        assert_eq!(status(&resp), 400);
    }

    #[test]
    fn files_must_be_hdl_and_without_dotdot() {
        let d = dashboard();
        let rest = d.into_rest_server("127.0.0.1:0");
        for bad in ["/etc/passwd", "../../secret.sv", "a.txt"] {
            let resp = rest.handle_request(
                "POST",
                &format!("/api/v1/sessions?user=alice&files={}", bad),
            );
            assert_eq!(status(&resp), 400, "files={} harus ditolak", bad);
        }
        // Filelist .f dan .mivon tetap boleh.
        let ok = rest.handle_request("POST", "/api/v1/sessions?user=alice&files=run.f,top.sv");
        assert_eq!(status(&ok), 201);
    }

    #[test]
    fn capacity_full_is_503() {
        let d = WebDashboard::new(ServerConfig {
            max_sessions_total: 1,
            ..Default::default()
        });
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let _tx = tx; // sender tetap hidup → sesi menggantung di Running
        let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
        d.server()
            .set_runner(std::sync::Arc::new(move |_files, _max_time| {
                let guard = rx.lock().unwrap_or_else(|e| e.into_inner());
                guard.recv().map(|_| 1).map_err(|e| e.to_string())
            }));
        let rest = d.into_rest_server("127.0.0.1:0");
        let first = rest.handle_request("POST", "/api/v1/sessions?user=alice");
        assert_eq!(status(&first), 201);
        let second = rest.handle_request("POST", "/api/v1/sessions?user=bob");
        assert_eq!(status(&second), 503);
    }

    #[test]
    fn runner_executes_session_and_reports_cycles() {
        let d = dashboard();
        d.server()
            .set_runner(std::sync::Arc::new(|_files, max_time| Ok(max_time.min(42))));
        let rest = d.into_rest_server("127.0.0.1:0");
        let resp = rest.handle_request("POST", "/api/v1/sessions?user=alice&max_time=900");
        assert_eq!(status(&resp), 201);
        let id = serde_json::from_str::<serde_json::Value>(body(&resp)).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();

        let mut done = false;
        for _ in 0..200 {
            let list = rest.handle_request("GET", "/api/v1/sessions?user=alice");
            let v: serde_json::Value = serde_json::from_str(body(&list)).unwrap();
            let s = &v["sessions"][0];
            if s["status"] == "Completed" {
                assert_eq!(s["elapsed_time"], 42);
                done = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(done, "sesi tidak selesai: {}", id);
    }

    #[test]
    fn list_filters_by_user() {
        let d = dashboard();
        let rest = d.into_rest_server("127.0.0.1:0");
        rest.handle_request("POST", "/api/v1/sessions?user=alice");
        rest.handle_request("POST", "/api/v1/sessions?user=bob");
        let alice = rest.handle_request("GET", "/api/v1/sessions?user=alice");
        assert!(body(&alice).contains("sim_alice_"));
        assert!(!body(&alice).contains("sim_bob_"));
    }

    #[test]
    fn cancel_unknown_session_is_404() {
        let d = dashboard();
        let rest = d.into_rest_server("127.0.0.1:0");
        let resp = rest.handle_request("POST", "/api/v1/cancel?id=sim_ghost_1");
        assert_eq!(status(&resp), 404);
    }

    #[test]
    fn cancel_twice_is_conflict() {
        let (d, _tx) = dashboard_held();
        let rest = d.into_rest_server("127.0.0.1:0");
        let id = d.server().submit("alice", vec![], 100).unwrap();
        let first = rest.handle_request("POST", &format!("/api/v1/cancel?id={}", id));
        assert_eq!(status(&first), 200);
        let second = rest.handle_request("POST", &format!("/api/v1/cancel?id={}", id));
        assert_eq!(status(&second), 409);
    }

    #[test]
    fn quota_exhausted_is_429() {
        let (d, _tx) = dashboard_held();
        let rest = d.into_rest_server("127.0.0.1:0");
        for _ in 0..4 {
            let resp = rest.handle_request("POST", "/api/v1/sessions?user=alice");
            assert_eq!(status(&resp), 201);
        }
        let resp = rest.handle_request("POST", "/api/v1/sessions?user=alice");
        assert_eq!(status(&resp), 429);
    }

    #[test]
    fn stats_counts_each_status() {
        let (d, _tx) = dashboard_held();
        let rest = d.into_rest_server("127.0.0.1:0");
        let a = d.server().submit("alice", vec![], 100).unwrap();
        d.server().submit("alice", vec![], 100).unwrap();
        rest.handle_request("POST", &format!("/api/v1/cancel?id={}", a));
        let resp = rest.handle_request("GET", "/api/v1/stats");
        let b = body(&resp);
        assert_eq!(status(&resp), 200);
        assert!(b.contains("\"total\":2"), "{}", b);
        assert!(b.contains("\"running\":1"), "{}", b);
        assert!(b.contains("\"cancelled\":1"), "{}", b);
    }

    #[test]
    fn error_body_is_json_escaped() {
        let resp = ApiResponse::error(400, "kutip \" dan \\ backslash").to_http_response();
        assert!(body(&resp).contains("\\\""));
        // Body harus tetap JSON valid.
        let parsed: serde_json::Value = serde_json::from_str(body(&resp)).unwrap();
        assert_eq!(parsed["error"], "kutip \" dan \\ backslash");
    }
}
