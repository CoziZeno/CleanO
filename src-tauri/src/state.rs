//! Shared application state.
//!
//! The scan index lives here, in Rust memory, and never crosses to the webview
//! in full. That is the central performance decision of the app: displaying a
//! 100k-file folder costs the same as displaying a 100-file one, because the UI
//! only ever asks for aggregates and for windows of a few hundred rows.

use crate::core::db::Db;
use crate::core::dto::{DedupeStatus, ScanState, ScanStatus};
use crate::core::dup::DupReport;
use crate::core::model::ScanIndex;
use crate::core::progress::CancelToken;
use crate::core::settings::Settings;
use parking_lot::{Mutex, RwLock};
use std::path::PathBuf;

pub const DB_FILE: &str = "cleanup.db";

pub struct ScanRuntime {
    pub next_id: u64,
    /// Present only while a scan is running.
    pub active: Option<(u64, CancelToken)>,
    pub status: ScanStatus,
    pub root: Option<String>,
}

impl Default for ScanRuntime {
    fn default() -> Self {
        Self {
            next_id: 0,
            active: None,
            status: ScanStatus::Idle,
            root: None,
        }
    }
}

#[derive(Default)]
pub struct DedupeRuntime {
    pub status: DedupeStatus,
    /// Present only while a duplicate check is running.
    pub cancel: Option<CancelToken>,
}

impl Default for DedupeStatus {
    fn default() -> Self {
        DedupeStatus::Idle
    }
}

/// Duplicate results, tagged with the scan they describe.
///
/// File ids are indices into one specific scan, so a report from an earlier
/// scan is not stale data to be shown with a caveat - it is wrong data that
/// would point at the wrong files. It is refused instead.
pub struct StoredDuplicates {
    pub scan_id: u64,
    pub report: DupReport,
}

pub struct AppState {
    /// `RwLock` because reads (query, summary) vastly outnumber the single
    /// write at the end of a scan, and a query must never block another query.
    pub index: RwLock<Option<ScanIndex>>,
    pub runtime: Mutex<ScanRuntime>,
    pub dedupe: Mutex<DedupeRuntime>,
    pub duplicates: RwLock<Option<StoredDuplicates>>,
    pub settings: RwLock<Settings>,
    pub config_dir: PathBuf,
    /// The hash cache. `None` only if SQLite could not be opened at all, in
    /// which case the app stays fully usable and simply re-hashes each run.
    pub db: Option<Db>,
    /// Surfaced once on first load if settings.json could not be parsed, so a
    /// corrupt file is visible rather than silently replaced by defaults.
    pub settings_warning: Mutex<Option<String>>,
    /// Surfaced once if the previous run died mid-operation. The user finds
    /// out from us rather than from a half-organised folder.
    pub recovery_notice: Mutex<Option<String>>,
    /// Installed software, read once and reused. It does not change while
    /// someone is tidying a folder, and the registry walk is not free.
    pub apps: RwLock<Option<Vec<crate::core::apps::InstalledApp>>>,
}

impl AppState {
    pub fn new(config_dir: PathBuf) -> Self {
        let (settings, mut warning) = Settings::load(&config_dir);

        // A missing cache costs performance, never correctness, so failing to
        // open it must not stop the app from starting.
        let db = match Db::open(&config_dir.join(DB_FILE)) {
            Ok(db) => Some(db),
            Err(err) => {
                let message = format!(
                    "The hash cache could not be opened ({err}). Duplicate checks will \
                     still work, but results will not be remembered between runs."
                );
                warning = Some(match warning {
                    Some(existing) => format!("{existing} {message}"),
                    None => message,
                });
                None
            }
        };

        // Crash recovery. An operation still marked planned or running means
        // the process died mid-batch; settle it to what the item records
        // actually say, and tell the user once.
        let recovery_notice = db.as_ref().and_then(|db| {
            let stale = crate::core::journal::unfinished(db).unwrap_or_default();
            if stale.is_empty() {
                return None;
            }
            let landed: usize = stale.iter().map(|op| op.done_count()).sum();
            let _ = crate::core::journal::reconcile_unfinished(db);
            Some(format!(
                "Cleanup stopped unexpectedly during {} {}. {} {} had already been handled; \
                 the record of what happened is in the operation history, where you can undo it.",
                stale.len(),
                if stale.len() == 1 { "operation" } else { "operations" },
                landed,
                if landed == 1 { "file" } else { "files" }
            ))
        });

        Self {
            index: RwLock::new(None),
            runtime: Mutex::new(ScanRuntime::default()),
            dedupe: Mutex::new(DedupeRuntime::default()),
            duplicates: RwLock::new(None),
            settings: RwLock::new(settings),
            config_dir,
            db,
            settings_warning: Mutex::new(warning),
            recovery_notice: Mutex::new(recovery_notice),
            apps: RwLock::new(None),
        }
    }

    /// Drop duplicate results. Called whenever a new scan lands, because their
    /// file ids no longer refer to anything.
    pub fn invalidate_duplicates(&self) {
        *self.duplicates.write() = None;
        self.dedupe.lock().status = DedupeStatus::Idle;
    }

    pub fn scan_state(&self) -> ScanState {
        let runtime = self.runtime.lock();
        ScanState {
            status: runtime.status,
            scan_id: runtime.active.as_ref().map(|(id, _)| *id).or_else(|| {
                self.index.read().as_ref().map(|index| index.scan_id)
            }),
            root: runtime.root.clone(),
        }
    }

    /// Persist settings, reporting a write failure rather than dropping it.
    pub fn save_settings(&self) -> std::io::Result<()> {
        let settings = self.settings.read().clone();
        settings.save(&self.config_dir)
    }
}
