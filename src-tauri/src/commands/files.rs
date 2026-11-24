//! File manager operations.
//!
//! Everything here goes through the same guard and the same journal as the
//! cleanup operations - a rename from a context menu is no less destructive
//! than one from a plan, so it gets the same treatment.
//!
//! Opening a file is the one exception: it mutates nothing, so it is not
//! journalled. It still passes the guard, because "open" hands a path to the
//! operating system and that path had better be one we were allowed to touch.

use crate::core::dto::{FileRow, OperationRow};
use crate::core::journal::{self, OpKind, OpState};
use crate::core::{dto, fsops, identity, safety};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;

use super::cleanup::{approved_roots, load_row, resolve_targets, settle};

/// Absolute path for an entry, guarded. Used for opening and for copy-path.
#[tauri::command]
pub fn file_path(state: State<'_, Arc<AppState>>, id: u32) -> AppResult<String> {
    let index = state.index.read();
    let index = index.as_ref().ok_or(AppError::NoScan)?;
    let entry = index
        .entry(id)
        .ok_or_else(|| AppError::Invalid(format!("no file with id {id}")))?;
    let path = index.path_of(entry);
    let approved = approved_roots(&state);
    let guarded = safety::guard_source(&path, &approved)?;
    Ok(guarded.to_string_lossy().to_string())
}

/// Rename in place. Refuses a name already in use rather than inventing one.
#[tauri::command]
pub async fn rename_file(
    state: State<'_, Arc<AppState>>,
    id: u32,
    new_name: String,
) -> AppResult<OperationRow> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (root, targets) = resolve_targets(&state, &[id])?;
        let target = targets
            .into_iter()
            .next()
            .ok_or_else(|| AppError::Invalid("that file cannot be renamed".into()))?;

        let db = state
            .db
            .as_ref()
            .ok_or_else(|| AppError::Io("no journal is available".into()))?;
        let approved = approved_roots(&state);

        let begun = journal::begin(
            db,
            OpKind::Organize,
            &root,
            &format!("Renamed to {}", new_name.trim()),
            &[journal::PlannedItem {
                from: target.path.clone(),
                to: None,
                size: target.size,
            }],
        )
        .map_err(|e| AppError::Io(format!("could not write the journal: {e}")))?;
        let _ = journal::set_state(db, begun.operation_id, OpState::Running);

        let outcome = fsops::rename(&target.path, &new_name, &approved);
        match &outcome {
            Ok(to) => {
                let _ = journal::item_done(db, begun.item_ids[0], Some(to));
                let _ = journal::set_state(db, begun.operation_id, OpState::Done);
            }
            Err(err) => {
                let _ = journal::item_failed(db, begun.item_ids[0], &err.to_string());
                let _ = journal::set_state(db, begun.operation_id, OpState::Failed);
            }
        }
        // Surface the real reason rather than a generic "operation failed".
        outcome.map_err(|err| AppError::Invalid(err.to_string()))?;
        load_row(&state, begun.operation_id)
    })
    .await
    .map_err(|err| AppError::Io(format!("the operation thread failed: {err}")))?
}

/// Copy files into a folder. Journalled, though a copy is not undoable by
/// moving anything back - the record is there so the user can see what ran.
#[tauri::command]
pub async fn copy_files_to(
    state: State<'_, Arc<AppState>>,
    ids: Vec<u32>,
    destination: String,
) -> AppResult<OperationRow> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (root, targets) = resolve_targets(&state, &ids)?;
        if targets.is_empty() {
            return Err(AppError::Invalid(
                "none of the selected files can be copied".into(),
            ));
        }
        let to_dir = PathBuf::from(&destination);
        let approved = approved_roots(&state);
        safety::guard_new_dir(&to_dir, &approved)?;

        let db = state
            .db
            .as_ref()
            .ok_or_else(|| AppError::Io("no journal is available".into()))?;

        let planned: Vec<journal::PlannedItem> = targets
            .iter()
            .map(|t| journal::PlannedItem {
                from: t.path.clone(),
                to: t.path.file_name().map(|name| to_dir.join(name)),
                size: t.size,
            })
            .collect();

        let shown = identity::shorten_for_display(to_dir.as_os_str());
        let begun = journal::begin(
            db,
            OpKind::Organize,
            &root,
            &format!("Copied {} files to {shown}", planned.len()),
            &planned,
        )
        .map_err(|e| AppError::Io(format!("could not write the journal: {e}")))?;
        let _ = journal::set_state(db, begun.operation_id, OpState::Running);

        let mut done = 0usize;
        for (target, &item_id) in targets.iter().zip(&begun.item_ids) {
            match fsops::copy_into(&target.path, &to_dir, &approved) {
                Ok(outcome) => {
                    done += 1;
                    let _ = journal::item_done(db, item_id, Some(&outcome.to));
                }
                Err(err) => {
                    let _ = journal::item_failed(db, item_id, &err.to_string());
                }
            }
        }
        // A copy leaves the originals in place, so undoing it would mean
        // deleting the copies - not something to do automatically.
        let _ = journal::set_state(db, begun.operation_id, settle(done, targets.len()));
        load_row(&state, begun.operation_id)
    })
    .await
    .map_err(|err| AppError::Io(format!("the operation thread failed: {err}")))?
}

/// Create a folder inside the scan root (or inside `parent` when given).
#[tauri::command]
pub fn create_folder(
    state: State<'_, Arc<AppState>>,
    name: String,
    parent: Option<String>,
) -> AppResult<String> {
    let parent_path = match parent {
        Some(path) if !path.trim().is_empty() => PathBuf::from(path),
        _ => {
            let index = state.index.read();
            index.as_ref().ok_or(AppError::NoScan)?.root.clone()
        }
    };
    let approved = approved_roots(&state);
    let created = fsops::create_named_dir(&parent_path, &name, &approved)
        .map_err(|err| AppError::Invalid(err.to_string()))?;
    Ok(created.to_string_lossy().to_string())
}

/// Create a folder and move the selection into it, as one journalled action.
#[tauri::command]
pub async fn create_folder_and_move(
    state: State<'_, Arc<AppState>>,
    ids: Vec<u32>,
    name: String,
    parent: Option<String>,
) -> AppResult<OperationRow> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (root, targets) = resolve_targets(&state, &ids)?;
        if targets.is_empty() {
            return Err(AppError::Invalid(
                "none of the selected files can be moved".into(),
            ));
        }
        let parent_path = parent
            .filter(|p| !p.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| root.clone());

        let approved = approved_roots(&state);
        let folder = fsops::create_named_dir(&parent_path, &name, &approved)
            .map_err(|err| AppError::Invalid(err.to_string()))?;

        let db = state
            .db
            .as_ref()
            .ok_or_else(|| AppError::Io("no journal is available".into()))?;

        let planned: Vec<journal::PlannedItem> = targets
            .iter()
            .map(|t| journal::PlannedItem {
                from: t.path.clone(),
                to: t.path.file_name().map(|n| folder.join(n)),
                size: t.size,
            })
            .collect();

        let begun = journal::begin(
            db,
            OpKind::Organize,
            &root,
            &format!("Moved {} files into {}", planned.len(), name.trim()),
            &planned,
        )
        .map_err(|e| AppError::Io(format!("could not write the journal: {e}")))?;
        let _ = journal::set_state(db, begun.operation_id, OpState::Running);

        let mut done = 0usize;
        for (target, &item_id) in targets.iter().zip(&begun.item_ids) {
            match fsops::move_into(&target.path, &folder, &approved) {
                Ok(outcome) => {
                    done += 1;
                    let _ = journal::item_done(db, item_id, Some(&outcome.to));
                }
                Err(err) => {
                    let _ = journal::item_failed(db, item_id, &err.to_string());
                }
            }
        }
        let _ = journal::set_state(db, begun.operation_id, settle(done, targets.len()));
        load_row(&state, begun.operation_id)
    })
    .await
    .map_err(|err| AppError::Io(format!("the operation thread failed: {err}")))?
}

/// Rows for a set of ids, used after an operation to refresh a view without a
/// whole rescan.
#[tauri::command]
pub fn rows_for(state: State<'_, Arc<AppState>>, ids: Vec<u32>) -> AppResult<Vec<FileRow>> {
    let index = state.index.read();
    let index = index.as_ref().ok_or(AppError::NoScan)?;
    Ok(ids
        .into_iter()
        .filter_map(|id| index.entry(id))
        .map(|entry| dto::row(index, entry))
        .collect())
}
