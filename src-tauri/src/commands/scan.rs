//! Scan lifecycle commands.
//!
//! These are adapters, not logic: resolve and validate input, hand off to
//! `core`, serialise the result. The walk itself runs on a dedicated thread so
//! the IPC call returns immediately with a scan id and the UI stays live.

use crate::core::dto::{PlatformInfo, ScanState, ScanStatus, ScanSummary};
use crate::core::model::{now_unix_secs, ScanError};
use crate::core::progress::{CancelToken, ProgressSink, ProgressSnapshot};
use crate::core::{analyze, identity, safety, walk};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

pub const EVENT_PROGRESS: &str = "scan:progress";
pub const EVENT_DONE: &str = "scan:done";
pub const EVENT_FAILED: &str = "scan:failed";

/// Bridges `core`'s Tauri-free progress trait onto the event system.
struct EventSink {
    app: AppHandle,
}

impl ProgressSink for EventSink {
    fn tick(&self, snapshot: ProgressSnapshot) {
        // A dropped progress event is not worth failing a scan over: the next
        // tick, and the final one, carry absolute counts rather than deltas.
        let _ = self.app.emit(EVENT_PROGRESS, snapshot);
    }
}

#[tauri::command]
pub fn platform_info() -> PlatformInfo {
    PlatformInfo {
        os: std::env::consts::OS,
        // `dirs` uses SHGetKnownFolderPath(FOLDERID_Downloads) on Windows
        // rather than building %USERPROFILE%\Downloads, which matters because
        // the folder is relocatable through the registry.
        downloads_dir: dirs::download_dir().map(|p| p.to_string_lossy().to_string()),
        home_dir: dirs::home_dir().map(|p| p.to_string_lossy().to_string()),
        access_time_reliable: identity::access_time_reliable(),
        privacy_prompts: cfg!(target_os = "macos"),
    }
}

#[tauri::command]
pub fn scan_state(state: State<'_, Arc<AppState>>) -> ScanState {
    state.scan_state()
}

/// Begin a scan. Returns the new scan id immediately; results arrive as
/// `scan:progress` ticks and then one `scan:done` or `scan:failed`.
#[tauri::command]
pub fn start_scan(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    root: Option<String>,
) -> AppResult<u64> {
    let state = state.inner().clone();

    let requested = match root.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => default_root(&state)?,
    };

    // Validated before anything is locked or spawned, so a rejected root leaves
    // no trace in the runtime.
    let validated = safety::validate_root(&requested)?;

    let (scan_id, cancel, options) = {
        let mut runtime = state.runtime.lock();
        if runtime.active.is_some() {
            return Err(AppError::ScanInProgress);
        }
        runtime.next_id += 1;
        let scan_id = runtime.next_id;
        let cancel = CancelToken::new();
        runtime.active = Some((scan_id, cancel.clone()));
        runtime.status = ScanStatus::Scanning;
        runtime.root = Some(identity::shorten_for_display(validated.as_os_str()));
        let options = state.settings.read().walk_options();
        (scan_id, cancel, options)
    };

    // Scanning a folder is the user approving it. Recording that here is what
    // gives Phase 4's mutation guard an allowlist to check against.
    {
        let mut settings = state.settings.write();
        settings.add_root(&validated);
    }
    if let Err(err) = state.save_settings() {
        // Not fatal: the scan can proceed, the approval just will not persist
        // across a restart. Worth knowing about, not worth blocking on.
        eprintln!("cleanup: could not save settings: {err}");
    }

    let stale_days = state.settings.read().stale_days;
    let thread_app = app.clone();
    let thread_state = state.clone();
    let scan_root = validated.clone();

    std::thread::Builder::new()
        .name(format!("cleanup-scan-{scan_id}"))
        .spawn(move || {
            let sink = EventSink {
                app: thread_app.clone(),
            };
            let outcome = walk::scan(&scan_root, scan_id, &options, &cancel, &sink);

            match outcome {
                Ok(index) => {
                    let summary = analyze::summarize(&index, stale_days, now_unix_secs());
                    // A cancelled scan still yields a usable partial index;
                    // throwing it away would be wasteful and surprising.
                    *thread_state.index.write() = Some(index);
                    // Duplicate results index into the previous scan, so they
                    // now point at the wrong files. Drop them.
                    thread_state.invalidate_duplicates();
                    {
                        let mut runtime = thread_state.runtime.lock();
                        runtime.active = None;
                        runtime.status = ScanStatus::Ready;
                    }
                    let _ = thread_app.emit(EVENT_DONE, summary);
                }
                Err(err) => {
                    let app_error = AppError::from_io(&err, &scan_root);
                    {
                        let mut runtime = thread_state.runtime.lock();
                        runtime.active = None;
                        runtime.status = ScanStatus::Failed;
                    }
                    let _ = thread_app.emit(EVENT_FAILED, app_error);
                }
            }
        })
        .map_err(|err| AppError::Io(format!("could not start the scan thread: {err}")))?;

    Ok(scan_id)
}

/// Request cancellation. Returns whether a scan was actually running.
///
/// The walk checks the token once per directory and once per entry, so a cancel
/// takes effect within one `read_dir` batch rather than at the end of a subtree.
#[tauri::command]
pub fn cancel_scan(state: State<'_, Arc<AppState>>) -> bool {
    let runtime = state.runtime.lock();
    match runtime.active.as_ref() {
        Some((_, token)) => {
            token.cancel();
            true
        }
        None => false,
    }
}

#[tauri::command]
pub fn get_summary(state: State<'_, Arc<AppState>>) -> AppResult<ScanSummary> {
    let index = state.index.read();
    let index = index.as_ref().ok_or(AppError::NoScan)?;
    let stale_days = state.settings.read().stale_days;
    Ok(analyze::summarize(index, stale_days, now_unix_secs()))
}

/// Paths the scan could not read. Bounded, because a UI cannot usefully show
/// thousands; `ScanSummary::error_count` carries the true total.
#[tauri::command]
pub fn get_scan_errors(
    state: State<'_, Arc<AppState>>,
    limit: Option<u32>,
) -> AppResult<Vec<ScanError>> {
    let index = state.index.read();
    let index = index.as_ref().ok_or(AppError::NoScan)?;
    let limit = limit.unwrap_or(100).clamp(1, 1000) as usize;
    Ok(index.errors.iter().take(limit).cloned().collect())
}

/// The folder to scan when the user has not named one: whatever they scanned
/// last, else the platform's Downloads folder.
fn default_root(state: &AppState) -> AppResult<PathBuf> {
    if let Some(last) = state.settings.read().roots.last() {
        let path = PathBuf::from(last);
        if path.is_dir() {
            return Ok(path);
        }
    }
    dirs::download_dir().ok_or(AppError::NoDownloadsFolder)
}
