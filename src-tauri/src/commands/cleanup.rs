//! Operations that change the filesystem, and undo.
//!
//! The shape of every one of these is the same, and it is the shape that makes
//! them recoverable:
//!
//! 1. Resolve ids to real paths from the index - never from a string the UI
//!    sent back.
//! 2. Write the whole plan to the journal and **commit it** - still nothing
//!    touched.
//! 3. Execute item by item, recording each outcome as it happens. One failure
//!    does not abort the batch.
//! 4. Settle the operation's state to the truth: done, partial or failed.
//!
//! They run on a blocking worker rather than the UI thread, so trashing a few
//! thousand files cannot freeze the window.

use crate::core::dto::{LifetimeStats, OperationItemRow, OperationRow, UndoOutcome};
use crate::core::identity;
use crate::core::journal::{self, ItemState, OpKind, OpState};
use crate::core::{fsops, suggest, text};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;

/// One file to act on: its id, and the path resolved from the index.
///
/// Shared with `commands::files` so every operation resolves ids the same
/// way - from the index, never from a path the UI sent back.
pub(crate) struct Target {
    pub path: PathBuf,
    pub size: u64,
}

pub(crate) fn resolve_targets(
    state: &AppState,
    ids: &[u32],
) -> AppResult<(PathBuf, Vec<Target>)> {
    let index = state.index.read();
    let index = index.as_ref().ok_or(AppError::NoScan)?;
    let mut targets = Vec::with_capacity(ids.len());
    for &id in ids {
        let Some(entry) = index.entry(id) else { continue };
        // Links, cloud placeholders and part-downloads never get operated on,
        // whatever the UI thinks it selected.
        if !entry.flags.is_safe_to_touch() {
            continue;
        }
        targets.push(Target {
            path: index.path_of(entry),
            size: entry.size,
        });
    }
    Ok((index.root.clone(), targets))
}

/// The folders every operation is checked against: the approved roots, minus
/// any folder the user excluded.
pub(crate) fn approved_roots(state: &AppState) -> crate::core::safety::Scope {
    state.settings.read().scope()
}

/// Send files to the Recycle Bin / Trash.
#[tauri::command]
pub async fn trash_files(
    state: State<'_, Arc<AppState>>,
    ids: Vec<u32>,
) -> AppResult<OperationRow> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || execute_trash(&state, &ids))
        .await
        .map_err(|err| AppError::Io(format!("the operation thread failed: {err}")))?
}

fn execute_trash(state: &AppState, ids: &[u32]) -> AppResult<OperationRow> {
    let (root, targets) = resolve_targets(state, ids)?;
    if targets.is_empty() {
        return Err(AppError::Invalid(
            "none of the selected files can be acted on".into(),
        ));
    }
    let db = state.db.as_ref().ok_or_else(|| {
        AppError::Io("no journal is available, so this cannot be done safely".into())
    })?;
    let roots = approved_roots(state);

    // Refuse the whole batch if any file sits on a volume with no Recycle Bin.
    // Doing half the job and permanently destroying the rest is the worst
    // possible outcome, so this check happens before the journal is written.
    for target in &targets {
        if let fsops::TrashAvailability::Unavailable(reason) =
            fsops::trash_availability(&target.path)
        {
            return Err(AppError::Invalid(format!(
                "{} is on a volume with no Recycle Bin ({reason}). Deleting there would be \
                 permanent, so nothing was done.",
                identity::shorten_for_display(target.path.as_os_str())
            )));
        }
    }

    let planned: Vec<journal::PlannedItem> = targets
        .iter()
        .map(|t| journal::PlannedItem {
            from: t.path.clone(),
            to: None,
            size: t.size,
        })
        .collect();

    let begun = journal::begin(
        db,
        OpKind::Trash,
        &root,
        &format!("Moved {} files to the Trash", planned.len()),
        &planned,
    )
    .map_err(|e| AppError::Io(format!("could not write the journal: {e}")))?;
    let _ = journal::set_state(db, begun.operation_id, OpState::Running);

    let mut done = 0u64;
    for (target, &item_id) in targets.iter().zip(&begun.item_ids) {
        match fsops::to_trash(&target.path, &roots) {
            Ok(()) => {
                done += 1;
                let _ = journal::item_done(db, item_id, None);
            }
            Err(err) => {
                let _ = journal::item_failed(db, item_id, &err.to_string());
            }
        }
    }

    let state_after = settle(done as usize, targets.len());
    let _ = journal::set_state(db, begun.operation_id, state_after);
    load_row(state, begun.operation_id)
}

/// Apply the organization plan, optionally for a subset of files.
#[tauri::command]
pub async fn apply_organize(
    state: State<'_, Arc<AppState>>,
    ids: Option<Vec<u32>>,
    mode: Option<suggest::OrganizeMode>,
) -> AppResult<OperationRow> {
    let state = state.inner().clone();
    let mode = mode.unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || execute_organize(&state, ids, mode))
        .await
        .map_err(|err| AppError::Io(format!("the operation thread failed: {err}")))?
}

/// Move chosen files into a folder the user picked.
///
/// The counterpart to deleting: sometimes the answer is "put this somewhere",
/// not "get rid of it". Same guard, same journal, same undo - the destination
/// simply comes from a folder picker instead of a rule.
#[tauri::command]
pub async fn move_files_to(
    state: State<'_, Arc<AppState>>,
    ids: Vec<u32>,
    destination: String,
) -> AppResult<OperationRow> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || execute_move_to(&state, &ids, &destination))
        .await
        .map_err(|err| AppError::Io(format!("the operation thread failed: {err}")))?
}

fn execute_move_to(
    state: &AppState,
    ids: &[u32],
    destination: &str,
) -> AppResult<OperationRow> {
    let (root, targets) = resolve_targets(state, ids)?;
    if targets.is_empty() {
        return Err(AppError::Invalid(
            "none of the selected files can be moved".into(),
        ));
    }

    let to_dir = PathBuf::from(destination);
    // The destination has to be inside an approved folder, exactly like any
    // other write. Picking a folder in a dialog does not grant access to it.
    let approved = approved_roots(state);
    crate::core::safety::guard_new_dir(&to_dir, &approved)?;

    let db = state.db.as_ref().ok_or_else(|| {
        AppError::Io("no journal is available, so this cannot be done safely".into())
    })?;

    let planned: Vec<journal::PlannedItem> = targets
        .iter()
        .map(|target| journal::PlannedItem {
            from: target.path.clone(),
            to: target
                .path
                .file_name()
                .map(|name| to_dir.join(name))
                .or_else(|| Some(to_dir.clone())),
            size: target.size,
        })
        .collect();

    let shown = identity::shorten_for_display(to_dir.as_os_str());
    let begun = journal::begin(
        db,
        OpKind::Organize,
        &root,
        &format!("Moved {} files to {shown}", planned.len()),
        &planned,
    )
    .map_err(|e| AppError::Io(format!("could not write the journal: {e}")))?;
    let _ = journal::set_state(db, begun.operation_id, OpState::Running);

    let mut done = 0usize;
    for (target, &item_id) in targets.iter().zip(&begun.item_ids) {
        match fsops::move_into(&target.path, &to_dir, &approved) {
            Ok(outcome) => {
                done += 1;
                let _ = journal::item_done(db, item_id, Some(&outcome.to));
            }
            Err(err) => {
                let _ = journal::item_failed(db, item_id, &err.to_string());
            }
        }
    }

    let state_after = settle(done, targets.len());
    let _ = journal::set_state(db, begun.operation_id, state_after);
    load_row(state, begun.operation_id)
}

fn execute_organize(
    state: &AppState,
    ids: Option<Vec<u32>>,
    mode: suggest::OrganizeMode,
) -> AppResult<OperationRow> {
    // Rebuild the plan now rather than trusting one the UI is holding: the
    // folder may have changed since it was shown.
    let (root, moves) = {
        let index = state.index.read();
        let index = index.as_ref().ok_or(AppError::NoScan)?;
        let settings = state.settings.read();
        let plan = suggest::organize_plan(index, &settings, mode);

        let wanted: Option<std::collections::HashSet<u32>> =
            ids.map(|list| list.into_iter().collect());

        let mut moves = Vec::new();
        for group in &plan.groups {
            for item in &group.items {
                if wanted.as_ref().is_some_and(|set| !set.contains(&item.id)) {
                    continue;
                }
                let Some(entry) = index.entry(item.id) else { continue };
                if !entry.flags.is_safe_to_touch() {
                    continue;
                }
                // A nested destination like "2024/March" is joined component
                // by component, so the separator is the platform's own.
                let target_dir = group
                    .destination
                    .split('/')
                    .fold(index.root.clone(), |path, part| path.join(part));
                moves.push((index.path_of(entry), target_dir, entry.size));
            }
        }
        (index.root.clone(), moves)
    };

    if moves.is_empty() {
        return Err(AppError::Invalid("there is nothing to organize".into()));
    }

    let db = state.db.as_ref().ok_or_else(|| {
        AppError::Io("no journal is available, so this cannot be done safely".into())
    })?;
    let roots = approved_roots(state);

    let planned: Vec<journal::PlannedItem> = moves
        .iter()
        .map(|(from, to_dir, size)| journal::PlannedItem {
            from: from.clone(),
            to: from
                .file_name()
                .map(|name| to_dir.join(name))
                .or_else(|| Some(to_dir.clone())),
            size: *size,
        })
        .collect();

    let begun = journal::begin(
        db,
        OpKind::Organize,
        &root,
        &format!("Sorted {} files into folders", planned.len()),
        &planned,
    )
    .map_err(|e| AppError::Io(format!("could not write the journal: {e}")))?;
    let _ = journal::set_state(db, begun.operation_id, OpState::Running);

    let mut done = 0usize;
    for ((from, to_dir, _), &item_id) in moves.iter().zip(&begun.item_ids) {
        match fsops::move_into(from, to_dir, &roots) {
            Ok(outcome) => {
                done += 1;
                // Record where it actually landed, which may differ from the
                // plan if a collision forced a rename.
                let _ = journal::item_done(db, item_id, Some(&outcome.to));
            }
            Err(err) => {
                let _ = journal::item_failed(db, item_id, &err.to_string());
            }
        }
    }

    let state_after = settle(done, moves.len());
    let _ = journal::set_state(db, begun.operation_id, state_after);
    load_row(state, begun.operation_id)
}

/// Reverse an operation.
///
/// Moves are reversed exactly. Trash is **not**: restoring from the Recycle Bin
/// programmatically is not reliable across Windows configurations, and the bin
/// may have been emptied. Rather than promise an undo that silently fails, the
/// original paths are reported so the user can restore them themselves.
#[tauri::command]
pub async fn undo_operation(
    state: State<'_, Arc<AppState>>,
    operation_id: i64,
) -> AppResult<UndoOutcome> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || execute_undo(&state, operation_id))
        .await
        .map_err(|err| AppError::Io(format!("the operation thread failed: {err}")))?
}

fn execute_undo(state: &AppState, operation_id: i64) -> AppResult<UndoOutcome> {
    let db = state.db.as_ref().ok_or_else(|| {
        AppError::Io("no journal is available, so there is nothing to undo from".into())
    })?;
    let record = journal::load(db, operation_id)
        .map_err(|e| AppError::Io(format!("could not read the journal: {e}")))?
        .ok_or_else(|| AppError::Invalid(format!("no operation with id {operation_id}")))?;

    if !record.state.is_reversible() {
        return Err(AppError::Invalid(
            "that operation cannot be undone".to_string(),
        ));
    }

    if record.kind == OpKind::Trash {
        // Honest about the limit instead of pretending.
        let problems: Vec<String> = record
            .items
            .iter()
            .filter(|i| i.state == ItemState::Done)
            .map(|i| identity::shorten_for_display(i.from.as_os_str()))
            .collect();
        return Ok(UndoOutcome {
            operation_id,
            restored: 0,
            failed: 0,
            problems,
            manual_restore_required: true,
        });
    }

    let roots = approved_roots(state);
    let mut restored = 0u64;
    let mut failed = 0u64;
    let mut problems = Vec::new();

    for item in &record.items {
        if item.state != ItemState::Done {
            continue;
        }
        let Some(current) = item.to.as_ref() else {
            continue;
        };
        match fsops::move_back(current, &item.from, &roots) {
            Ok(()) => {
                restored += 1;
                let _ = journal::item_reverted(db, item.id);
            }
            Err(err) => {
                failed += 1;
                if problems.len() < 50 {
                    problems.push(err.to_string());
                }
            }
        }
    }

    let _ = journal::set_state(
        db,
        operation_id,
        if failed == 0 { OpState::Undone } else { OpState::Partial },
    );

    Ok(UndoOutcome {
        operation_id,
        restored,
        failed,
        problems,
        manual_restore_required: false,
    })
}

#[tauri::command]
pub fn list_operations(state: State<'_, Arc<AppState>>, limit: Option<u32>) -> Vec<OperationRow> {
    let Some(db) = state.db.as_ref() else {
        return Vec::new();
    };
    journal::recent(db, limit.unwrap_or(25))
        .map(|records| records.iter().map(to_row).collect())
        .unwrap_or_default()
}

#[tauri::command]
pub fn lifetime_stats(state: State<'_, Arc<AppState>>) -> LifetimeStats {
    let Some(db) = state.db.as_ref() else {
        return LifetimeStats::default();
    };
    journal::lifetime(db)
        .map(|total| LifetimeStats {
            files: total.files,
            bytes: total.bytes,
            operations: total.operations,
        })
        .unwrap_or_default()
}

/// Operations left unfinished by a crash, settled and reported once.
#[tauri::command]
pub fn take_recovery_notice(state: State<'_, Arc<AppState>>) -> Option<String> {
    state.recovery_notice.lock().take()
}

pub(crate) fn settle(done: usize, total: usize) -> OpState {
    if done == 0 {
        OpState::Failed
    } else if done < total {
        OpState::Partial
    } else {
        OpState::Done
    }
}

pub(crate) fn load_row(state: &AppState, operation_id: i64) -> AppResult<OperationRow> {
    let db = state
        .db
        .as_ref()
        .ok_or_else(|| AppError::Io("no journal available".into()))?;
    let record = journal::load(db, operation_id)
        .map_err(|e| AppError::Io(format!("could not read the journal: {e}")))?
        .ok_or_else(|| AppError::Invalid("the operation vanished from the journal".into()))?;
    Ok(to_row(&record))
}

fn to_row(record: &journal::OperationRecord) -> OperationRow {
    OperationRow {
        id: record.id,
        kind: record.kind.key(),
        state: record.state.key(),
        created_at: record.created_at,
        note: record.note.clone(),
        root: identity::shorten_for_display(record.root.as_os_str()),
        done: record.done_count() as u64,
        failed: record.failed_count() as u64,
        bytes: record.bytes(),
        undoable: record.state.is_reversible() && record.done_count() > 0,
        items: record
            .items
            .iter()
            .take(500)
            .map(|item| OperationItemRow {
                name: item
                    .from
                    .file_name()
                    .map(text::display)
                    .unwrap_or_default(),
                from_path: identity::shorten_for_display(item.from.as_os_str()),
                to_path: item
                    .to
                    .as_ref()
                    .map(|p| identity::shorten_for_display(p.as_os_str())),
                size: item.size,
                state: match item.state {
                    ItemState::Pending => "pending",
                    ItemState::Done => "done",
                    ItemState::Failed => "failed",
                    ItemState::Reverted => "reverted",
                },
                error: item.error.clone(),
            })
            .collect(),
    }
}
