//! Progress reporting and cancellation, defined without any Tauri types.
//!
//! `core` never emits events itself. It takes a `&dyn ProgressSink`, which the
//! command layer implements over an `AppHandle`. That is the single reason the
//! whole scanner can be unit-tested against a temp directory with no window,
//! no webview and no event loop.

use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressSnapshot {
    pub scan_id: u64,
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub errors: u64,
    /// Directory currently being read, already display-normalised.
    pub current: String,
}

pub trait ProgressSink: Send + Sync {
    fn tick(&self, snapshot: ProgressSnapshot);
}

/// Used by tests and by any call site that does not care about progress.
pub struct NoopSink;

impl ProgressSink for NoopSink {
    fn tick(&self, _snapshot: ProgressSnapshot) {}
}

/// Cooperative cancellation.
///
/// Checked once per directory and once per directory entry, which bounds the
/// latency of a cancel to a single `read_dir` batch rather than to a whole
/// subtree.
#[derive(Clone, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Relaxed);
    }

    #[inline]
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }
}
