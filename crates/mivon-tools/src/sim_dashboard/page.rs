//! Render halaman HTML dashboard (FEAT-03).
//!
//! Tabel di-render di server (progressive enhancement: halaman tetap berguna
//! tanpa JS) dan di-refresh di klien lewat `/api/v1/sessions`. Semua teks
//! dinamis di-escape — nama user / file berasal dari input klien.

use crate::sim_server::{SessionStatus, SimSession};

/// Escape teks untuk konteks HTML (body maupun nilai atribut).
pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

fn status_badge(status: &SessionStatus) -> (&'static str, String) {
    match status {
        SessionStatus::Running => ("running", "Running".to_string()),
        SessionStatus::Queued => ("queued", "Queued".to_string()),
        SessionStatus::Completed => ("done", "Completed".to_string()),
        SessionStatus::Cancelled => ("cancelled", "Cancelled".to_string()),
        SessionStatus::Failed(reason) => ("failed", format!("Failed: {}", reason)),
    }
}

fn cancellable(status: &SessionStatus) -> bool {
    matches!(status, SessionStatus::Running | SessionStatus::Queued)
}

fn render_row(s: &SimSession) -> String {
    let (cls, label) = status_badge(&s.status);
    let files = if s.source_files.is_empty() {
        "-".to_string()
    } else {
        escape_html(&s.source_files.join(", "))
    };
    let action = if cancellable(&s.status) {
        format!(
            "<button class=\"cancel\" data-id=\"{}\">Cancel</button>",
            escape_html(&s.id)
        )
    } else {
        String::new()
    };
    format!(
        "<tr><td class=\"mono\">{}</td><td>{}</td><td><span class=\"badge {}\">{}</span></td><td class=\"mono\">{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
        escape_html(&s.id),
        escape_html(&s.user),
        cls,
        escape_html(&label),
        files,
        s.max_time,
        s.elapsed_cycles,
        action,
    )
}

/// Halaman HTML lengkap untuk snapshot sesi saat ini.
pub fn render_page(sessions: &[SimSession]) -> String {
    let rows = if sessions.is_empty() {
        "<tr><td colspan=\"7\" class=\"empty\">Belum ada sesi.</td></tr>".to_string()
    } else {
        sessions.iter().map(render_row).collect::<Vec<_>>().join("\n")
    };
    let summary = format!(
        "{} sesi — {} running, {} queued, {} selesai, {} dibatalkan",
        sessions.len(),
        sessions.iter().filter(|s| matches!(s.status, SessionStatus::Running)).count(),
        sessions.iter().filter(|s| matches!(s.status, SessionStatus::Queued)).count(),
        sessions.iter().filter(|s| matches!(s.status, SessionStatus::Completed)).count(),
        sessions.iter().filter(|s| matches!(s.status, SessionStatus::Cancelled)).count(),
    );
    PAGE.replace("<!--ROWS-->", &rows)
        .replace("<!--SUMMARY-->", &escape_html(&summary))
}

const PAGE: &str = r#"<!DOCTYPE html>
<html lang="id">
<head>
<meta charset="utf-8">
<title>Mivon Simulation Control</title>
<style>
  body { font-family: system-ui, sans-serif; margin: 0; background: #0f1115; color: #e6e6e6; }
  header { padding: 16px 24px; background: #171a21; border-bottom: 1px solid #2a2f3a; }
  h1 { font-size: 18px; margin: 0 0 4px; }
  .sub { color: #9aa4b2; font-size: 13px; }
  main { padding: 24px; max-width: 1100px; }
  form { display: flex; gap: 8px; flex-wrap: wrap; margin-bottom: 20px; }
  input { background: #171a21; color: #e6e6e6; border: 1px solid #2a2f3a; padding: 8px; border-radius: 4px; }
  button { background: #2d6cdf; color: #fff; border: 0; padding: 8px 14px; border-radius: 4px; cursor: pointer; }
  button.cancel { background: #8a2f2f; padding: 4px 8px; font-size: 12px; }
  table { width: 100%; border-collapse: collapse; font-size: 13px; }
  th, td { text-align: left; padding: 8px; border-bottom: 1px solid #2a2f3a; }
  th { color: #9aa4b2; font-weight: 600; }
  .mono { font-family: ui-monospace, monospace; }
  .empty { color: #9aa4b2; text-align: center; padding: 24px; }
  .badge { padding: 2px 8px; border-radius: 10px; font-size: 12px; }
  .badge.running { background: #1f3d6b; }
  .badge.queued { background: #4a3a12; }
  .badge.done { background: #1d4a2b; }
  .badge.failed { background: #5a1f1f; }
  .badge.cancelled { background: #333842; }
  #status { margin-left: 12px; color: #9aa4b2; font-size: 13px; }
</style>
</head>
<body>
<header>
  <h1>Mivon Simulation Control</h1>
  <div class="sub" id="summary"><!--SUMMARY--></div>
</header>
<main>
  <form id="submit">
    <input id="user" placeholder="user" required>
    <input id="files" placeholder="file1.sv,file2.sv" size="34">
    <input id="max_time" placeholder="max_time" value="1000" size="8">
    <button type="submit">Submit</button>
    <span id="status"></span>
  </form>
  <table>
    <thead><tr><th>ID</th><th>User</th><th>Status</th><th>Files</th><th>max_time</th><th>cycles</th><th></th></tr></thead>
    <tbody id="sessions">
<!--ROWS-->
    </tbody>
  </table>
</main>
<script>
const tbody = document.getElementById('sessions');
const summary = document.getElementById('summary');
const statusEl = document.getElementById('status');

function badgeClass(status) {
  if (status === 'Running' || status === 'Queued' || status === 'Completed' || status === 'Cancelled') {
    return status === 'Completed' ? 'done' : status.toLowerCase();
  }
  return 'failed';
}
function badgeText(status) {
  if (typeof status === 'string') return status;
  if (status && status.Failed !== undefined) return 'Failed: ' + status.Failed;
  return 'Unknown';
}
function cell(text, mono) {
  const td = document.createElement('td');
  td.textContent = text;
  if (mono) td.className = 'mono';
  return td;
}
function renderRow(s) {
  const tr = document.createElement('tr');
  tr.appendChild(cell(s.id, true));
  tr.appendChild(cell(s.user, false));
  const td = document.createElement('td');
  const span = document.createElement('span');
  span.className = 'badge ' + badgeClass(s.status);
  span.textContent = badgeText(s.status);
  td.appendChild(span);
  tr.appendChild(td);
  tr.appendChild(cell(s.source_files.length ? s.source_files.join(', ') : '-', true));
  tr.appendChild(cell(String(s.max_time), false));
  tr.appendChild(cell(String(s.elapsed_cycles), false));
  const action = document.createElement('td');
  if (s.status === 'Running' || s.status === 'Queued') {
    const btn = document.createElement('button');
    btn.className = 'cancel';
    btn.textContent = 'Cancel';
    btn.dataset.id = s.id;
    action.appendChild(btn);
  }
  tr.appendChild(action);
  return tr;
}
async function refresh() {
  try {
    const res = await fetch('/api/v1/sessions');
    const data = await res.json();
    tbody.replaceChildren();
    if (!data.sessions.length) {
      const tr = document.createElement('tr');
      const td = document.createElement('td');
      td.colSpan = 7;
      td.className = 'empty';
      td.textContent = 'Belum ada sesi.';
      tr.appendChild(td);
      tbody.appendChild(tr);
    } else {
      data.sessions.forEach(s => tbody.appendChild(renderRow(s)));
    }
    const active = data.sessions.filter(s => s.status === 'Running' || s.status === 'Queued').length;
    summary.textContent = data.sessions.length + ' sesi — ' + active + ' aktif, antrean ' + data.queue_depth;
  } catch (e) {
    statusEl.textContent = 'gagal memuat: ' + e;
  }
}
document.getElementById('submit').addEventListener('submit', async (ev) => {
  ev.preventDefault();
  const user = document.getElementById('user').value.trim();
  const files = document.getElementById('files').value.trim();
  const maxTime = document.getElementById('max_time').value.trim();
  const q = new URLSearchParams({ user, files, max_time: maxTime });
  const res = await fetch('/api/v1/sessions?' + q.toString(), { method: 'POST' });
  if (!res.ok) {
    const err = await res.json().catch(() => ({ error: res.statusText }));
    statusEl.textContent = 'gagal: ' + err.error;
    return;
  }
  statusEl.textContent = 'sesi dibuat';
  refresh();
});
tbody.addEventListener('click', async (ev) => {
  const btn = ev.target.closest('button.cancel');
  if (!btn) return;
  const res = await fetch('/api/v1/cancel?id=' + encodeURIComponent(btn.dataset.id), { method: 'POST' });
  if (!res.ok) {
    const err = await res.json().catch(() => ({ error: res.statusText }));
    statusEl.textContent = 'gagal cancel: ' + err.error;
  }
  refresh();
});
refresh();
setInterval(refresh, 2000);
</script>
</body>
</html>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn session(user: &str, files: &[&str], status: SessionStatus) -> SimSession {
        SimSession {
            id: format!("sim_{}_1", user),
            user: user.to_string(),
            status,
            source_files: files.iter().map(|f| f.to_string()).collect(),
            max_time: 1000,
            elapsed_cycles: 0,
            created_at: 0,
        }
    }

    #[test]
    fn page_has_form_and_table() {
        let html = render_page(&[]);
        assert!(html.contains("Mivon Simulation Control"));
        assert!(html.contains("id=\"submit\""));
        assert!(html.contains("Belum ada sesi."));
        assert!(html.contains("setInterval(refresh, 2000)"));
    }

    #[test]
    fn page_lists_session_and_status() {
        let html = render_page(&[session("alice", &["a.sv", "b.sv"], SessionStatus::Running)]);
        assert!(html.contains("sim_alice_1"));
        assert!(html.contains("a.sv, b.sv"));
        assert!(html.contains("badge running"));
        assert!(html.contains("data-id=\"sim_alice_1\""));
    }

    #[test]
    fn page_escapes_user_and_files() {
        let evil = session(
            "<script>alert(1)</script>",
            &["a\"><img src=x onerror=alert(2)>.sv"],
            SessionStatus::Failed("<b>boom</b>".to_string()),
        );
        let html = render_page(&[evil]);
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!html.contains("<img src=x"));
        assert!(html.contains("Failed: &lt;b&gt;boom&lt;/b&gt;"));
    }

    #[test]
    fn terminal_status_has_no_cancel_button() {
        let html = render_page(&[
            session("alice", &[], SessionStatus::Completed),
            session("bob", &[], SessionStatus::Cancelled),
        ]);
        assert!(!html.contains("class=\"cancel\""));
    }

    #[test]
    fn escape_html_covers_all_specials() {
        assert_eq!(
            escape_html("&<>\"'"),
            "&amp;&lt;&gt;&quot;&#39;"
        );
    }
}
