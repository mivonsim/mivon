//! Scheduler internal node (node_cache.md §1: scheduler bukan subsystem
//! mandiri).
//!
//! Eksekusi node yang independen diparalelkan per batch dengan
//! `std::thread::scope` — tanpa runtime/daemon tambahan. Urutan dependency
//! tetap ditentukan caller (pipeline menyerahkan batch yang sudah siap).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// Jumlah worker default (minimal 1, maksimal 16).
pub fn worker_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 16)
}

/// Jalankan `f` untuk setiap item secara paralel (batch independen).
///
/// Hasil dikumpulkan dengan urutan yang sama seperti input agar deterministik.
pub fn run_parallel<T, R, F>(items: Vec<T>, workers: usize, f: F) -> Vec<R>
where
    T: Send,
    R: Send,
    F: Fn(T) -> R + Sync,
{
    let n = items.len();
    if n == 0 {
        return Vec::new();
    }
    let workers = workers.clamp(1, n);
    if workers == 1 {
        return items.into_iter().map(f).collect();
    }

    let items = Mutex::new(items.into_iter().map(Some).collect::<Vec<_>>());
    let results: Mutex<Vec<Option<R>>> = Mutex::new((0..n).map(|_| None).collect());
    let next = AtomicUsize::new(0);
    let f = &f;

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let idx = next.fetch_add(1, Ordering::Relaxed);
                if idx >= n {
                    break;
                }
                let item = {
                    let mut guard = items.lock().unwrap();
                    guard[idx].take()
                };
                let Some(item) = item else { continue };
                let value = f(item);
                results.lock().unwrap()[idx] = Some(value);
            });
        }
    });

    results
        .into_inner()
        .unwrap()
        .into_iter()
        .map(|v| v.expect("setiap item diproses tepat sekali"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_preserves_order() {
        let out = run_parallel((0..100).collect(), 4, |x| x * 2);
        assert_eq!(out, (0..100).map(|x| x * 2).collect::<Vec<_>>());
    }

    #[test]
    fn empty_input() {
        let out: Vec<u32> = run_parallel(Vec::new(), 4, |x: u32| x);
        assert!(out.is_empty());
    }

    #[test]
    fn single_worker_path() {
        let out = run_parallel(vec![1, 2, 3], 1, |x| x + 1);
        assert_eq!(out, vec![2, 3, 4]);
    }
}
