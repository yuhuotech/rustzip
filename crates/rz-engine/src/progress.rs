use rz_core::{CancelToken, Progress, ProgressSink, Result};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Reports progress to a sink from any number of threads.
///
/// Counters are lock-free; the sink is called at most every [`INTERVAL`] so a
/// UI is not flooded with events, except for the first update and the last.
pub(crate) struct Tracker<'a> {
    sink: Mutex<&'a mut dyn ProgressSink>,
    last: Mutex<Option<Instant>>,
    files_total: u64,
    bytes_total: u64,
    files_done: AtomicU64,
    bytes_done: AtomicU64,
    cancel: &'a CancelToken,
}

const INTERVAL: Duration = Duration::from_millis(50);

impl<'a> Tracker<'a> {
    pub fn new(
        sink: &'a mut dyn ProgressSink,
        cancel: &'a CancelToken,
        files_total: u64,
        bytes_total: u64,
    ) -> Self {
        Self {
            sink: Mutex::new(sink),
            last: Mutex::new(None),
            files_total,
            bytes_total,
            files_done: AtomicU64::new(0),
            bytes_done: AtomicU64::new(0),
            cancel,
        }
    }

    pub fn check(&self) -> Result<()> {
        self.cancel.check()
    }

    pub fn add_bytes(&self, n: u64) {
        self.bytes_done.fetch_add(n, Ordering::Relaxed);
        self.report(false);
    }

    pub fn file_done(&self) {
        self.files_done.fetch_add(1, Ordering::Relaxed);
        self.report(false);
    }

    /// Sends the final, complete state.
    pub fn finish(&self) {
        self.files_done.store(self.files_total, Ordering::Relaxed);
        self.report(true);
    }

    /// Finishes work whose totals were not known until its input stream ended.
    pub fn finish_with_totals(&self, files: u64, bytes: u64) {
        if let Ok(mut sink) = self.sink.lock() {
            sink.update(Progress {
                files_done: files,
                files_total: files,
                bytes_done: bytes,
                bytes_total: bytes,
            });
        }
    }

    fn snapshot(&self) -> Progress {
        Progress {
            files_done: self
                .files_done
                .load(Ordering::Relaxed)
                .min(self.files_total),
            files_total: self.files_total,
            bytes_done: self.bytes_done.load(Ordering::Relaxed),
            bytes_total: self.bytes_total,
        }
    }

    fn report(&self, force: bool) {
        // Another thread is reporting right now: skip rather than wait.
        let Ok(mut last) = self.last.try_lock() else {
            return;
        };
        let due = force || last.is_none_or(|t| t.elapsed() >= INTERVAL);
        if !due {
            return;
        }
        *last = Some(Instant::now());
        drop(last);
        if let Ok(mut sink) = self.sink.lock() {
            sink.update(self.snapshot());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_and_final_updates_always_arrive() {
        let cancel = CancelToken::new();
        let mut seen: Vec<Progress> = Vec::new();
        let mut sink = |p: Progress| seen.push(p);
        {
            let t = Tracker::new(&mut sink, &cancel, 3, 30);
            t.add_bytes(10);
            t.add_bytes(10);
            t.add_bytes(10);
            t.finish();
        }
        assert!(seen.len() >= 2 && seen.len() <= 4);
        assert_eq!(seen[0].bytes_done, 10);
        let last = seen[seen.len() - 1];
        assert_eq!((last.bytes_done, last.files_done), (30, 3));
    }

    #[test]
    fn counters_are_safe_across_threads() {
        let cancel = CancelToken::new();
        let mut sink = rz_core::NoProgress;
        let t = Tracker::new(&mut sink, &cancel, 8, 8000);
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    for _ in 0..1000 {
                        t.add_bytes(1);
                    }
                    t.file_done();
                });
            }
        });
        let p = t.snapshot();
        assert_eq!((p.bytes_done, p.files_done), (8000, 8));
    }
}
