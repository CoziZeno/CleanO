//! Quick Clean commands.
//!
//! Both are thin: the closed target list, the measuring and the deleting all
//! live in `core::quickclean`. What crosses this boundary is a target *key*,
//! never a path, and an unknown key resolves to nothing.

use crate::core::quickclean::{self, Measurement, Outcome};
use crate::error::{AppError, AppResult};
use std::sync::Arc;
use tauri::State;

use crate::state::AppState;

/// Measure every target. Read-only; nothing is deleted.
///
/// Walking a large `%TEMP%` can take a second or two, so it runs off the UI
/// thread.
#[tauri::command]
pub async fn quick_clean_scan(_state: State<'_, Arc<AppState>>) -> AppResult<Vec<Measurement>> {
    tauri::async_runtime::spawn_blocking(quickclean::measure_all)
        .await
        .map_err(|err| AppError::Io(format!("the scan thread failed: {err}")))
}

/// Delete the contents of the named targets.
///
/// **This is permanent.** These folders hold caches, not documents, and
/// routing hundreds of thousands of temp files through the Recycle Bin would
/// free no space at all - which is the entire point of the feature. The UI
/// confirms it in those words before calling this.
///
/// Each target reports separately, so a folder that needed administrator
/// rights shows as skipped rather than quietly contributing nothing.
#[tauri::command]
pub async fn quick_clean_run(
    _state: State<'_, Arc<AppState>>,
    keys: Vec<String>,
) -> AppResult<Vec<Outcome>> {
    tauri::async_runtime::spawn_blocking(move || {
        keys.iter().map(|key| quickclean::clean(key)).collect()
    })
    .await
    .map_err(|err| AppError::Io(format!("the cleanup thread failed: {err}")))
}
