//! Storage analyzer command.
//!
//! Everything here is derived from the in-memory index, so this performs no
//! disk I/O at all - which is why it can be called on every navigation to the
//! Storage screen without a loading state.

use crate::core::analyze;
use crate::core::dto::StorageReport;
use crate::core::model::now_unix_secs;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub fn get_storage_report(state: State<'_, Arc<AppState>>) -> AppResult<StorageReport> {
    let index = state.index.read();
    let index = index.as_ref().ok_or(AppError::NoScan)?;
    let stale_days = state.settings.read().stale_days;
    Ok(analyze::storage_report(index, stale_days, now_unix_secs()))
}

/// One level of the storage map. `path` is `None` for the scan root.
///
/// Derived from the in-memory index like the rest of this module, so drilling
/// into a folder costs no disk I/O and needs no loading state.
#[tauri::command]
pub fn get_treemap(
    state: State<'_, Arc<AppState>>,
    path: Option<String>,
) -> AppResult<crate::core::dto::TreemapNode> {
    let index = state.index.read();
    let index = index.as_ref().ok_or(AppError::NoScan)?;
    let dir = path.as_deref().map(std::path::Path::new);
    Ok(analyze::treemap(index, dir))
}
