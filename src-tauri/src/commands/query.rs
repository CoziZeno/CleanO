//! Querying the in-memory index.

use crate::core::dto::{FileRow, QueryRequest, QueryResponse};
use crate::core::{dto, query};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use std::sync::Arc;
use tauri::State;

/// Filter, sort and page over the current scan.
///
/// Holds a read lock for the duration, which is fine: concurrent queries do not
/// block each other, and the only writer is the single hand-off at the end of a
/// scan.
#[tauri::command]
pub fn query_files(
    state: State<'_, Arc<AppState>>,
    request: QueryRequest,
) -> AppResult<QueryResponse> {
    let index = state.index.read();
    let index = index.as_ref().ok_or(AppError::NoScan)?;
    Ok(query::query(index, &request))
}

/// Quick Find: a sentence in, matching files out.
///
/// Parses to an ordinary query and runs it against the in-memory index, so it
/// is as fast as typing in the search box. The interpretation comes back too,
/// so the UI can show what it understood rather than presenting results as if
/// by magic.
#[tauri::command]
pub fn quick_find(
    state: State<'_, Arc<AppState>>,
    text: String,
) -> AppResult<crate::core::dto::QuickFindResponse> {
    let parsed = crate::core::quickfind::parse(&text);
    let index = state.index.read();
    let index = index.as_ref().ok_or(AppError::NoScan)?;
    let response = query::query(index, &parsed.request);
    Ok(crate::core::dto::QuickFindResponse {
        interpretation: parsed.interpretation,
        total: response.total,
        bytes: response.bytes,
        rows: response.rows,
    })
}

/// Every id matching a filter, for "select all".
///
/// Capped: selecting a hundred thousand files in one click is far more likely
/// to be a mistake than an intention, and the cap is reported so the UI can
/// say so rather than silently selecting a subset.
#[tauri::command]
pub fn query_ids(
    state: State<'_, Arc<AppState>>,
    request: QueryRequest,
    limit: Option<u32>,
) -> AppResult<Vec<u32>> {
    let index = state.index.read();
    let index = index.as_ref().ok_or(AppError::NoScan)?;
    let limit = limit.unwrap_or(10_000).clamp(1, 50_000) as usize;
    Ok(query::matching_ids(index, &request, limit))
}

/// Resolve ids to rows, for a selection the UI is holding across views.
#[tauri::command]
pub fn get_rows(state: State<'_, Arc<AppState>>, ids: Vec<u32>) -> AppResult<Vec<FileRow>> {
    let index = state.index.read();
    let index = index.as_ref().ok_or(AppError::NoScan)?;
    Ok(ids
        .into_iter()
        .filter_map(|id| index.entry(id))
        .map(|entry| dto::row(index, entry))
        .collect())
}

/// The real on-disk path for an entry, for "Reveal in Explorer/Finder".
///
/// The UI never sends a path back to Rust for an operation - it sends the id and
/// Rust rebuilds the path. This command exists because the opener plugin runs on
/// the frontend side and needs a string; it is read-only and reveals the file in
/// the OS file manager rather than opening it.
#[tauri::command]
pub fn resolve_path(state: State<'_, Arc<AppState>>, id: u32) -> AppResult<String> {
    let index = state.index.read();
    let index = index.as_ref().ok_or(AppError::NoScan)?;
    let entry = index
        .entry(id)
        .ok_or_else(|| AppError::Invalid(format!("no file with id {id} in the current scan")))?;
    Ok(index.path_of(entry).to_string_lossy().to_string())
}
