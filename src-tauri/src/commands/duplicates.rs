//! Duplicate detection commands.
//!
//! The pipeline runs on its own thread and never holds the scan index: it takes
//! a snapshot of what it needs up front, so a long hashing run cannot block the
//! file list the user is browsing.

use crate::core::dto::{
    self, DedupeProgress, DedupeStatus, DuplicateGroup, DuplicateReport, DuplicateStats,
};
use crate::core::dup::{self, DupOptions, DupProgress, DupReport, DupSink};
use crate::core::model::ScanIndex;
use crate::core::progress::CancelToken;
use crate::error::{AppError, AppResult};
use crate::state::{AppState, StoredDuplicates};

use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

pub const EVENT_PROGRESS: &str = "dedupe:progress";
pub const EVENT_DONE: &str = "dedupe:done";
pub const EVENT_FAILED: &str = "dedupe:failed";

/// Characters of the digest shown in the UI. Enough to tell groups apart
/// without shipping 64 per row.
const HASH_PREFIX: usize = 12;

struct EventSink {
    app: AppHandle,
    scan_id: u64,
}

impl DupSink for EventSink {
    fn tick(&self, progress: DupProgress) {
        let _ = self.app.emit(
            EVENT_PROGRESS,
            DedupeProgress {
                scan_id: self.scan_id,
                stage: progress.stage.key(),
                files_done: progress.files_done,
                files_total: progress.files_total,
                bytes_done: progress.bytes_done,
                bytes_total: progress.bytes_total,
            },
        );
    }
}

#[tauri::command]
pub fn dedupe_status(state: State<'_, Arc<AppState>>) -> DedupeStatus {
    state.dedupe.lock().status
}

/// Start comparing file contents. Returns the scan id the results will belong
/// to; the report arrives as `dedupe:done`.
#[tauri::command]
pub fn start_dedupe(app: AppHandle, state: State<'_, Arc<AppState>>) -> AppResult<u64> {
    let state = state.inner().clone();

    // Snapshot under a brief read lock, then release it for the whole run.
    let (scan_id, candidates, skipped) = {
        let index = state.index.read();
        let index = index.as_ref().ok_or(AppError::NoScan)?;
        let (candidates, skipped) = dup::collect_candidates(index);
        (index.scan_id, candidates, skipped)
    };

    let cancel = {
        let mut dedupe = state.dedupe.lock();
        if dedupe.cancel.is_some() {
            return Err(AppError::DedupeInProgress);
        }
        let cancel = CancelToken::new();
        dedupe.cancel = Some(cancel.clone());
        dedupe.status = DedupeStatus::Running;
        cancel
    };

    let options = {
        let settings = state.settings.read();
        DupOptions {
            head_bytes: settings.hash.head_bytes,
            concurrency: settings.hash.concurrency,
            verify_bytewise: settings.hash.verify_bytewise,
        }
    };

    let thread_app = app.clone();
    let thread_state = state.clone();

    std::thread::Builder::new()
        .name(format!("cleanup-dedupe-{scan_id}"))
        .spawn(move || {
            let sink = EventSink {
                app: thread_app.clone(),
                scan_id,
            };
            let mut report = dup::find_duplicates(
                candidates,
                thread_state.db.as_ref(),
                &options,
                &cancel,
                &sink,
            );
            report.stats.skipped_unsafe = skipped;

            // Only publish results if the index has not moved under us. A scan
            // that finished during the run invalidates every file id here.
            let still_current = thread_state
                .index
                .read()
                .as_ref()
                .is_some_and(|index| index.scan_id == scan_id);

            if !still_current {
                let mut dedupe = thread_state.dedupe.lock();
                dedupe.cancel = None;
                dedupe.status = DedupeStatus::Idle;
                let _ = thread_app.emit(EVENT_FAILED, AppError::StaleDuplicates);
                return;
            }

            let payload = {
                let index = thread_state.index.read();
                index
                    .as_ref()
                    .map(|index| to_dto(index, scan_id, &report))
                    .unwrap_or_else(|| empty_report(scan_id))
            };

            *thread_state.duplicates.write() = Some(StoredDuplicates { scan_id, report });
            {
                let mut dedupe = thread_state.dedupe.lock();
                dedupe.cancel = None;
                dedupe.status = DedupeStatus::Ready;
            }

            let _ = thread_app.emit(EVENT_DONE, payload);
        })
        .map_err(|err| {
            let mut dedupe = state.dedupe.lock();
            dedupe.cancel = None;
            dedupe.status = DedupeStatus::Failed;
            AppError::Io(format!("could not start the hashing thread: {err}"))
        })?;

    Ok(scan_id)
}

/// Request cancellation. The hasher checks between 1 MiB chunks, so this takes
/// effect in milliseconds even on a very large file.
#[tauri::command]
pub fn cancel_dedupe(state: State<'_, Arc<AppState>>) -> bool {
    let dedupe = state.dedupe.lock();
    match dedupe.cancel.as_ref() {
        Some(token) => {
            token.cancel();
            true
        }
        None => false,
    }
}

#[tauri::command]
pub fn get_duplicates(state: State<'_, Arc<AppState>>) -> AppResult<DuplicateReport> {
    let stored = state.duplicates.read();
    let stored = stored.as_ref().ok_or(AppError::NoDuplicateScan)?;

    let index = state.index.read();
    let index = index.as_ref().ok_or(AppError::NoScan)?;
    if index.scan_id != stored.scan_id {
        return Err(AppError::StaleDuplicates);
    }

    Ok(to_dto(index, stored.scan_id, &stored.report))
}

/// How many hashes are remembered, so Settings can show the cache's value
/// rather than asking the user to take it on trust.
#[tauri::command]
pub fn hash_cache_count(state: State<'_, Arc<AppState>>) -> u64 {
    state.db.as_ref().map(|db| db.cached_count()).unwrap_or(0)
}

#[tauri::command]
pub fn clear_hash_cache(state: State<'_, Arc<AppState>>) -> AppResult<u64> {
    match state.db.as_ref() {
        Some(db) => db
            .clear()
            .map(|n| n as u64)
            .map_err(|err| AppError::Io(format!("could not clear the hash cache: {err}"))),
        None => Ok(0),
    }
}

fn empty_report(scan_id: u64) -> DuplicateReport {
    DuplicateReport {
        scan_id,
        groups: Vec::new(),
        stats: DuplicateStats::default(),
    }
}

/// Build the UI payload: resolve ids to rows, shorten digests.
fn to_dto(index: &ScanIndex, scan_id: u64, report: &DupReport) -> DuplicateReport {
    let groups: Vec<DuplicateGroup> = report
        .groups
        .iter()
        .map(|group| DuplicateGroup {
            hash: group.hash_hex.chars().take(HASH_PREFIX).collect(),
            size: group.size,
            reclaimable: group.reclaimable,
            keep_id: group.keep,
            keep_reason: group.keep_reason.clone(),
            files: group
                .ids
                .iter()
                .filter_map(|&id| index.entry(id))
                .map(|entry| dto::row(index, entry))
                .collect(),
        })
        // A group whose files have all vanished from the index is not worth
        // showing; one file left is not a duplicate.
        .filter(|group| group.files.len() > 1)
        .collect();

    let stats = &report.stats;
    DuplicateReport {
        scan_id,
        groups,
        stats: DuplicateStats {
            considered: stats.considered,
            skipped_unsafe: stats.skipped_unsafe,
            eliminated_by_size: stats.eliminated_by_size,
            collapsed_hardlinks: stats.collapsed_hardlinks,
            eliminated_by_head: stats.eliminated_by_head,
            head_hashed: stats.head_hashed,
            full_hashed: stats.full_hashed,
            cache_hits: stats.cache_hits,
            bytes_read: stats.bytes_read,
            duplicate_files: stats.duplicate_files,
            groups: report.groups.len() as u64,
            reclaimable: stats.reclaimable,
            verified_bytewise: stats.verified_bytewise,
            cancelled: stats.cancelled,
            duration_ms: stats.duration_ms,
            error_count: stats.errors.len() as u64,
            errors: stats.errors.iter().take(20).cloned().collect(),
        },
    }
}
