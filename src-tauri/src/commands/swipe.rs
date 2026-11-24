//! Swipe Clean: one file at a time.
//!
//! The queue is derived, never stored - it is the current scan minus whatever
//! has already been decided. Decisions *are* stored, keyed on file identity,
//! so closing the app and coming back resumes where you were.
//!
//! "Keep" moves nothing. It records that you looked at the file and chose to
//! keep it, which is the whole point: the app stops asking.

use crate::core::dto::{self, FileRow};
use crate::core::model::FileIdent;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

use serde::Serialize;
use std::sync::Arc;
use tauri::State;

/// How many to hand the UI at once. It shows one at a time, so a few hundred
/// is far more than enough to swipe through without another round trip.
const QUEUE_PAGE: usize = 300;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwipeProgress {
    pub reviewed: u64,
    pub kept: u64,
    pub trashed: u64,
    pub moved: u64,
    pub later: u64,
    /// Still undecided and eligible for review.
    pub remaining: u64,
    /// Total eligible files, decided or not.
    pub total: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwipeQueue {
    pub files: Vec<FileRow>,
    pub progress: SwipeProgress,
}

/// What goes in the queue, and in what order, now comes from the user's
/// settings - see `SwipeSettings`. The default list keeps the old behaviour:
/// images first because they are quickest to judge at a glance, then video and
/// archives, which tend to be the large wins.

/// Files awaiting review, best candidates first.
///
/// `later_only` returns the separate Review Later queue instead.
#[tauri::command]
pub fn swipe_queue(
    state: State<'_, Arc<AppState>>,
    later_only: Option<bool>,
) -> AppResult<SwipeQueue> {
    let later_only = later_only.unwrap_or(false);
    let decisions = state
        .db
        .as_ref()
        .map(|db| db.review_all())
        .unwrap_or_default();

    // Read the filter once: settings cannot change underneath a single queue
    // build, and holding the lock per entry would be pointless contention.
    let filter = state.settings.read().swipe.clone();

    let index = state.index.read();
    let index = index.as_ref().ok_or(AppError::NoScan)?;

    let mut progress = SwipeProgress {
        reviewed: 0,
        kept: 0,
        trashed: 0,
        moved: 0,
        later: 0,
        remaining: 0,
        total: 0,
    };
    let mut candidates: Vec<(u8, u64, u32)> = Vec::new();

    for entry in &index.entries {
        let Some(rank) = filter.priority(entry.category, entry.size, entry.ext.as_deref()) else {
            continue;
        };
        if !entry.flags.is_safe_to_touch() {
            continue;
        }
        progress.total += 1;

        let decision = entry
            .ident
            .and_then(|ident| decisions.get(&(ident.volume, ident.file_id)))
            .map(String::as_str);

        match decision {
            Some("kept") => {
                progress.kept += 1;
                progress.reviewed += 1;
            }
            Some("trashed") => {
                progress.trashed += 1;
                progress.reviewed += 1;
            }
            Some("moved") => {
                progress.moved += 1;
                progress.reviewed += 1;
            }
            Some("later") => {
                progress.later += 1;
                if later_only {
                    candidates.push((rank, entry.size, entry.id));
                }
            }
            _ => {
                progress.remaining += 1;
                if !later_only {
                    candidates.push((rank, entry.size, entry.id));
                }
            }
        }
    }

    // Priority tier first, then biggest within a tier - the files where a
    // decision frees the most space come first.
    candidates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
    candidates.truncate(QUEUE_PAGE);

    Ok(SwipeQueue {
        files: candidates
            .iter()
            .filter_map(|(_, _, id)| index.entry(*id))
            .map(|entry| dto::row(index, entry))
            .collect(),
        progress,
    })
}

/// Record a decision. `keep` and `later` move nothing.
///
/// `trashed` and `moved` are recorded *after* the corresponding operation has
/// already succeeded through the normal journalled path, so this never
/// implies a file was handled when it was not.
#[tauri::command]
pub fn swipe_decide(
    state: State<'_, Arc<AppState>>,
    ids: Vec<u32>,
    decision: String,
) -> AppResult<()> {
    if !matches!(decision.as_str(), "kept" | "later" | "trashed" | "moved") {
        return Err(AppError::Invalid(format!("unknown decision {decision}")));
    }
    let Some(db) = state.db.as_ref() else {
        return Ok(()); // no store, so nothing to remember; swiping still works
    };

    let idents: Vec<FileIdent> = {
        let index = state.index.read();
        let index = index.as_ref().ok_or(AppError::NoScan)?;
        ids.iter()
            .filter_map(|id| index.entry(*id))
            .filter_map(|entry| entry.ident)
            .collect()
    };

    for ident in idents {
        db.review_set(ident, &decision)
            .map_err(|err| AppError::Io(format!("could not save the decision: {err}")))?;
    }
    Ok(())
}

/// Forget decisions, so undoing a swipe puts the file back in the queue.
#[tauri::command]
pub fn swipe_forget(state: State<'_, Arc<AppState>>, ids: Vec<u32>) -> AppResult<()> {
    let Some(db) = state.db.as_ref() else {
        return Ok(());
    };
    let idents: Vec<FileIdent> = {
        let index = state.index.read();
        let index = index.as_ref().ok_or(AppError::NoScan)?;
        ids.iter()
            .filter_map(|id| index.entry(*id))
            .filter_map(|entry| entry.ident)
            .collect()
    };
    for ident in idents {
        let _ = db.review_forget(ident);
    }
    Ok(())
}

/// Start the review over. Only forgets decisions - no file is touched.
#[tauri::command]
pub fn swipe_reset(state: State<'_, Arc<AppState>>) -> AppResult<u64> {
    match state.db.as_ref() {
        Some(db) => db
            .review_clear()
            .map(|n| n as u64)
            .map_err(|err| AppError::Io(format!("could not reset the review: {err}"))),
        None => Ok(0),
    }
}
