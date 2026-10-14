use crate::error::{Error, Result};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Shared progress and cancellation state of a long operation. Cheap to poll from a UI thread.
#[derive(Default)]
pub struct Progress {
    bytes_done: AtomicU64,
    bytes_total: AtomicU64,
    files_done: AtomicU64,
    files_total: AtomicU64,
    cancel: AtomicBool,
    current: Mutex<String>,
}

impl Progress {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_totals(&self, files: u64, bytes: u64) {
        self.files_total.store(files, Ordering::Relaxed);
        self.bytes_total.store(bytes, Ordering::Relaxed);
    }

    pub fn add_bytes(&self, n: u64) {
        self.bytes_done.fetch_add(n, Ordering::Relaxed);
    }

    pub fn add_file(&self) {
        self.files_done.fetch_add(1, Ordering::Relaxed);
    }

    pub fn set_current(&self, name: &str) {
        if let Ok(mut c) = self.current.lock() {
            c.clear();
            c.push_str(name);
        }
    }

    pub fn current(&self) -> String {
        self.current.lock().map(|c| c.clone()).unwrap_or_default()
    }

    pub fn bytes_done(&self) -> u64 {
        self.bytes_done.load(Ordering::Relaxed)
    }

    pub fn bytes_total(&self) -> u64 {
        self.bytes_total.load(Ordering::Relaxed)
    }

    pub fn files_done(&self) -> u64 {
        self.files_done.load(Ordering::Relaxed)
    }

    pub fn files_total(&self) -> u64 {
        self.files_total.load(Ordering::Relaxed)
    }

    /// 0.0 ..= 1.0 by bytes (or by files when no byte total is known).
    pub fn fraction(&self) -> f32 {
        let (done, total) = match self.bytes_total() {
            0 => (self.files_done(), self.files_total()),
            t => (self.bytes_done(), t),
        };
        if total == 0 {
            0.0
        } else {
            (done as f64 / total as f64).min(1.0) as f32
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}
