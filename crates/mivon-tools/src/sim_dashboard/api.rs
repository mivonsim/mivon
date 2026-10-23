//! Handler HTTP dashboard — menerjemahkan request menjadi operasi SimServer.
//!
//! Kontrak:
//! - `GET /api/v1/sessions[?user=]` → 200 `{count, queue_depth, sessions[]}`
//! - `POST /api/v1/sessions?user=&files=&max_time=` → 201 `{id, user, max_time}`
//!   (400 parameter tidak valid, 429 kuota/kapasitas user habis)
//! - `POST /api/v1/cancel?id=` → 200 (400 tanpa id, 404 tak dikenal,
//!   409 sudah terminal)
//! - `GET /api/v1/stats` → 200 hitungan per status + ringkasan server

use std::sync::Arc;

use crate::rest_api::{parse_query, ApiResponse, RestServer};
use crate::sim_server::{SessionStatus, SimServer, SimSession};

use super::page::render_page;

/// `max_time` default bila klien tidak mengirimkannya.
const DEFAULT_MAX_TIME: u64 = 1000;

pub(crate) fn register(rest: &mut RestServer, server: Arc<SimServer>) {
    let s = server.clone();
    rest.add_route("GET", "/", move |_, _| {
        ApiResponse::html(&render_page(&s.list_all())).to_http_response()
    });

    let s = server.clone();
    rest.add_route("GET", "/api/v1/sessions", move |_, path| {
        sessions_response(&s, path)
    });

    let s = server.clone();
    rest.add_route("POST", "/api/v1/sessions", move |_, path| {
        create_response(&s, path)
    });

    let s = server.clone();
    rest.add_route("POST", "/api/v1/cancel", move |_, path| {
        cancel_response(&s, path)
    });

    rest.add_route("GET", "/api/v1/stats", move |_, _| {
        stats_response(&server)
    });
}

/// Urutkan sesi agar output deterministik (HashMap tidak berurutan).
fn sorted(mut sessions: Vec<SimSession>) -> Vec<SimSession> {
    sessions.sort_by(|a, b| a.created_at.cmp(&b.created_at).then_with(|| a.id.cmp(&b.id)));
    sessions
}

/// Pisahkan daftar file `a.sv,b.sv` — buang entri kosong.
fn split_files(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|f| f.trim())
        .filter(|f| !f.is_empty())
        .map(|f| f.to_string())
        .collect()
}

fn sessions_response(server: &SimServer, path: &str) -> String {
    let q = parse_query(path);
    let user = q.get("user").map(|u| u.trim()).filter(|u| !u.is_empty());
    let sessions = match user {
        Some(user) => sorted(server.list_user_sessions(user)),
        None => server.list_all(),
    };
    let body = serde_json::json!({
        "count": sessions.len(),
        "queue_depth": server.queue_depth(),
        "sessions": sessions,
    });
    ApiResponse::json(&body.to_string()).to_http_response()
}

fn create_response(server: &SimServer, path: &str) -> String {
    let q = parse_query(path);
    let user = q.get("user").map(|u| u.trim()).unwrap_or("");
    if user.is_empty() {
        return ApiResponse::error(400, "parameter 'user' wajib diisi").to_http_response();
    }
    let max_time = match q.get("max_time").map(|m| m.trim()) {
        None | Some("") => DEFAULT_MAX_TIME,
        Some(raw) => match raw.parse::<u64>() {
            Ok(v) if v > 0 => v,
            _ => {
                return ApiResponse::error(400, "parameter 'max_time' harus bilangan bulat > 0")
                    .to_http_response()
            }
        },
    };
    let files = q.get("files").map(|f| split_files(f)).unwrap_or_default();

    match server.submit(user, files, max_time) {
        Ok(id) => {
            let body = serde_json::json!({
                "id": id,
                "user": user,
                "max_time": max_time,
            });
            ApiResponse::created(&body.to_string()).to_http_response()
        }
        Err(e) => ApiResponse::error(429, &e).to_http_response(),
    }
}

fn cancel_response(server: &SimServer, path: &str) -> String {
    let q = parse_query(path);
    let id = q.get("id").map(|i| i.trim()).unwrap_or("");
    if id.is_empty() {
        return ApiResponse::error(400, "parameter 'id' wajib diisi").to_http_response();
    }
    if server.get_session(id).is_none() {
        return ApiResponse::error(404, "session tidak ditemukan").to_http_response();
    }
    match server.cancel(id) {
        Ok(()) => {
            let body = serde_json::json!({ "id": id, "status": "cancelled" });
            ApiResponse::json(&body.to_string()).to_http_response()
        }
        Err(e) => ApiResponse::error(409, &e).to_http_response(),
    }
}

fn stats_response(server: &SimServer) -> String {
    let sessions = server.list_all();
    let count = |pred: fn(&SessionStatus) -> bool| sessions.iter().filter(|s| pred(&s.status)).count();
    let body = serde_json::json!({
        "total": sessions.len(),
        "running": count(|s| matches!(s, SessionStatus::Running)),
        "queued": count(|s| matches!(s, SessionStatus::Queued)),
        "completed": count(|s| matches!(s, SessionStatus::Completed)),
        "failed": count(|s| matches!(s, SessionStatus::Failed(_))),
        "cancelled": count(|s| matches!(s, SessionStatus::Cancelled)),
        "queue_depth": server.queue_depth(),
        "summary": server.summary(),
    });
    ApiResponse::json(&body.to_string()).to_http_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_files_trims_and_drops_empty() {
        assert_eq!(split_files(" a.sv , ,b.sv, "), vec!["a.sv", "b.sv"]);
        assert!(split_files("   ").is_empty());
    }

    #[test]
    fn sorted_is_stable_by_created_at_then_id() {
        let mk = |id: &str, created_at: u64| SimSession {
            id: id.to_string(),
            user: "u".to_string(),
            status: SessionStatus::Running,
            source_files: vec![],
            max_time: 1,
            elapsed_cycles: 0,
            created_at,
        };
        let out = sorted(vec![mk("b", 2), mk("c", 1), mk("a", 2)]);
        let ids: Vec<&str> = out.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["c", "a", "b"]);
    }
}
