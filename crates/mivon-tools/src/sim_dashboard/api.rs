//! Handler HTTP dashboard — menerjemahkan request menjadi operasi SimServer.
//!
//! Kontrak:
//! - `GET /api/v1/sessions[?user=]` → 200 `{count, queue_depth, sessions[]}`
//!   (400 bila parameter `user` ada tapi kosong)
//! - `POST /api/v1/sessions?user=&files=&max_time=` → 201 `{id, user, max_time}`
//!   (400 parameter/file tidak valid, 429 kuota user habis, 503 kapasitas
//!   riwayat server penuh)
//! - `POST /api/v1/cancel?id=` → 200 (400 tanpa id, 404 tak dikenal,
//!   409 sudah terminal)
//! - `GET /api/v1/stats` → 200 hitungan per status + ringkasan server

use std::sync::Arc;

use crate::rest_api::{parse_query, ApiResponse, RestServer};
use crate::sim_server::{CancelError, SessionStatus, SimServer, SubmitError};

use super::page::render_page;

/// `max_time` default bila klien tidak mengirimkannya.
const DEFAULT_MAX_TIME: u64 = 1000;

/// Batas atas `max_time` — mencegah satu request menahan worker sangat lama
/// (tidak ada mekanisme interupsi engine).
const MAX_MAX_TIME: u64 = 10_000_000;

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

/// Pisahkan daftar file `a.sv,b.sv` — buang entri kosong.
fn split_files(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|f| f.trim())
        .filter(|f| !f.is_empty())
        .map(|f| f.to_string())
        .collect()
}

/// Validasi daftar file dari klien.
///
/// Endpoint ini nantinya menyerahkan path ke simulator, jadi boundary harus
/// ketat: ekstensi HDL/filelist, tanpa komponen `..`, dan jumlah terbatas.
fn validate_files(files: &[String]) -> Result<(), String> {
    const MAX_FILES: usize = 64;
    const EXT: [&str; 6] = ["sv", "svh", "v", "vh", "f", "mivon"];
    if files.len() > MAX_FILES {
        return Err(format!("terlalu banyak file (max {})", MAX_FILES));
    }
    for f in files {
        if f.contains("..") {
            return Err(format!("path tidak boleh mengandung '..': {}", f));
        }
        // Path absolut / home / drive-letter ditolak: server tanpa auth tidak
        // boleh bisa membaca file mana pun di mesin (isi bisa bocor lewat
        // pesan Failed).
        if f.starts_with('/') || f.starts_with('\\') || f.starts_with('~') {
            return Err(format!("path harus relatif terhadap direktori kerja: {}", f));
        }
        // Drive-letter Windows (C:) ditolak; nama file sah yang mengandung ':'
        // (mis. "a:b.sv") tetap diizinkan.
        if f.len() >= 2
            && f.as_bytes()[1] == b':'
            && f.as_bytes()[0].is_ascii_alphabetic()
        {
            return Err(format!("path drive-letter tidak diizinkan: {}", f));
        }
        let ext = std::path::Path::new(f)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        if !EXT.contains(&ext.as_str()) {
            return Err(format!("ekstensi file tidak didukung: {}", f));
        }
    }
    Ok(())
}

fn sessions_response(server: &SimServer, path: &str) -> String {
    let q = parse_query(path);
    // Parameter ada tapi kosong = salah ketik, bukan "semua user".
    let user = match q.get("user") {
        Some(raw) => {
            let u = raw.trim();
            if u.is_empty() {
                return ApiResponse::error(400, "parameter 'user' tidak boleh kosong")
                    .to_http_response();
            }
            Some(u)
        }
        None => None,
    };
    // list_* sudah terurut (created_at, nomor urut) — jangan sort ulang
    // dengan tie-break berbeda.
    let sessions = match user {
        Some(user) => server.list_user_sessions(user),
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
    if user.chars().count() > 64 {
        return ApiResponse::error(400, "parameter 'user' maksimal 64 karakter").to_http_response();
    }
    let max_time = match q.get("max_time").map(|m| m.trim()) {
        None | Some("") => DEFAULT_MAX_TIME,
        Some(raw) => match raw.parse::<u64>() {
            Ok(v) if v > 0 && v <= MAX_MAX_TIME => v,
            Ok(v) if v > MAX_MAX_TIME => {
                return ApiResponse::error(
                    400,
                    &format!("parameter 'max_time' maksimal {}", MAX_MAX_TIME),
                )
                .to_http_response()
            }
            _ => {
                return ApiResponse::error(400, "parameter 'max_time' harus bilangan bulat > 0")
                    .to_http_response()
            }
        },
    };
    let files = q.get("files").map(|f| split_files(f)).unwrap_or_default();
    if let Err(e) = validate_files(&files) {
        return ApiResponse::error(400, &e).to_http_response();
    }

    match server.submit(user, files, max_time) {
        Ok(id) => {
            let body = serde_json::json!({
                "id": id,
                "user": user,
                "max_time": max_time,
            });
            ApiResponse::created(&body.to_string()).to_http_response()
        }
        Err(SubmitError::Quota { active, max, .. }) => ApiResponse::error(
            429,
            &format!("kuota sesi user habis ({} aktif, max {})", active, max),
        )
        .to_http_response(),
        Err(SubmitError::Capacity { total, max }) => ApiResponse::error(
            503,
            &format!("kapasitas server penuh ({} sesi, max {})", total, max),
        )
        .to_http_response(),
        Err(SubmitError::NoRunner) => {
            ApiResponse::error(503, "server belum punya runner eksekusi").to_http_response()
        }
    }
}

fn cancel_response(server: &SimServer, path: &str) -> String {
    let q = parse_query(path);
    let id = q.get("id").map(|i| i.trim()).unwrap_or("");
    if id.is_empty() {
        return ApiResponse::error(400, "parameter 'id' wajib diisi").to_http_response();
    }
    match server.cancel(id) {
        Ok(()) => {
            let body = serde_json::json!({ "id": id, "status": "cancelled" });
            ApiResponse::json(&body.to_string()).to_http_response()
        }
        Err(CancelError::NotFound) => {
            ApiResponse::error(404, "session tidak ditemukan").to_http_response()
        }
        Err(CancelError::NotCancellable) => {
            ApiResponse::error(409, "session tidak dalam status yang bisa dibatalkan")
                .to_http_response()
        }
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
    fn absolute_paths_are_rejected() {
        for bad in ["/etc/passwd.sv", "~/x.sv", "C:\\rtl\\a.sv"] {
            assert!(validate_files(&[bad.to_string()]).is_err(), "{}", bad);
        }
        assert!(validate_files(&["rtl/a.sv".to_string()]).is_ok());
    }
}
