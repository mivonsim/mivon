use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use super::Diagnostic;

/// Kapasitas channel queue diag. Bounded (bukan unbounded): loop yang memuntahkan
/// warning berulang (mis. `forever` yang memanggil null-handle) tidak boleh
/// menumpuk diagnostic tanpa batas → anti-leak memori (gejala: RSS/swap membengkak
/// puluhan GB selama sim).
const DIAG_QUEUE_CAP: usize = 10_000;
/// Batas jumlah diagnostic yang dipertahankan di `collected` setelah flush.
/// Normal (desain sehat) jauh di bawah ini; hanya jaring pengaman.
const DIAG_COLLECTED_CAP: usize = 100_000;

/// Thread-safe diagnostic collection menggunakan crossbeam channel (bounded).
pub struct DiagSink {
    /// MPSC channel untuk cross-thread diagnostics
    sender: crossbeam::channel::Sender<Diagnostic>,
    receiver: crossbeam::channel::Receiver<Diagnostic>,
    /// Collected diagnostics (after flush)
    collected: Mutex<Vec<Diagnostic>>,
    /// Total diagnostics pushed (atomic counter)
    pub total_pushed: AtomicUsize,
    /// Diagnostics yang dibuang karena channel penuh (untuk observability).
    pub dropped: AtomicUsize,
}

impl DiagSink {
    pub fn new() -> Self {
        let (sender, receiver) = crossbeam::channel::bounded(DIAG_QUEUE_CAP);
        DiagSink {
            sender,
            receiver,
            collected: Mutex::new(Vec::new()),
            total_pushed: AtomicUsize::new(0),
            dropped: AtomicUsize::new(0),
        }
    }

    /// Push a diagnostic (non-blocking, lock-free fast path).
    /// Channel bounded: bila penuh, diagnostic DIBUANG (dihitung di `dropped`)
    /// daripada menumpuk memori tanpa batas.
    /// Setiap diagnostic juga diteruskan ke Global Diagnostic Engine
    /// (`crate::diagnostics::global`) — agregator global lintas komponen.
    pub fn push(&self, diag: Diagnostic) {
        self.total_pushed.fetch_add(1, Ordering::Relaxed);
        super::global::diag_global().report(diag.clone());
        if self.sender.try_send(diag).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Flush all pending diagnostics into collected vec.
    /// `collected` di-cap — di luar cap, entry terlama dibuang (anti-leak).
    pub fn flush(&self) {
        while let Ok(diag) = self.receiver.try_recv() {
            let mut collected = self.collected.lock().unwrap();
            collected.push(diag);
            if collected.len() > DIAG_COLLECTED_CAP {
                let excess = collected.len() - DIAG_COLLECTED_CAP;
                collected.drain(..excess);
            }
        }
    }

    /// Get all collected diagnostics (flush first).
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        self.flush();
        let mut all = self.collected.lock().unwrap().clone();
        // Sort by file, then by position
        all.sort_by(|a, b| {
            let file_a = a.spans.first().map(|s| s.file.index());
            let file_b = b.spans.first().map(|s| s.file.index());
            file_a.cmp(&file_b).then_with(|| {
                let pos_a = a.spans.first().map(|s| s.start).unwrap_or(0);
                let pos_b = b.spans.first().map(|s| s.start).unwrap_or(0);
                pos_a.cmp(&pos_b)
            })
        });
        all
    }

    /// Get count of errors (not warnings/notes).
    pub fn error_count(&self) -> usize {
        self.flush();
        self.collected
            .lock()
            .unwrap()
            .iter()
            .filter(|d| d.is_error())
            .count()
    }

    /// Are there any errors?
    pub fn has_errors(&self) -> bool {
        self.error_count() > 0
    }

    /// Clear all collected diagnostics.
    pub fn clear(&self) {
        self.collected.lock().unwrap().clear();
    }
}

impl Default for DiagSink {
    fn default() -> Self {
        Self::new()
    }
}
