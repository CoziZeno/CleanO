//! Installed software, and installers that are already redundant.
//!
//! Read-only. Nothing here uninstalls anything - that is the operating
//! system's job, and a tool that cleans a Downloads folder has no business
//! removing software.

use crate::core::appmatch::{self, InstallerMatch};
use crate::core::apps::{self, InstalledApp};
use crate::core::dto::{self, FileRow};
use crate::core::model::Category;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

use serde::Serialize;
use std::sync::Arc;
use tauri::State;

/// An installer in the scanned folder, and the installed product it refers to.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallerMatchRow {
    pub file: FileRow,
    pub matched: InstallerMatch,
}

/// Everything installed, cached after the first read.
///
/// Walking the registry takes long enough to be worth not repeating on every
/// navigation, and installed software does not change while someone is
/// tidying a folder. `refresh` forces a re-read.
#[tauri::command]
pub async fn list_installed_apps(
    state: State<'_, Arc<AppState>>,
    refresh: Option<bool>,
) -> AppResult<Vec<InstalledApp>> {
    let state = state.inner().clone();

    if !refresh.unwrap_or(false) {
        let cached = { state.apps.read().clone() };
        if let Some(cached) = cached {
            return Ok(cached);
        }
    }

    let found = tauri::async_runtime::spawn_blocking(apps::installed)
        .await
        .map_err(|err| AppError::Io(format!("could not read installed software: {err}")))?;

    *state.apps.write() = Some(found.clone());
    Ok(found)
}

/// Installers in the current scan that match software already installed.
///
/// This is what makes an old installer actionable: not "this is eight months
/// old" but "you have version 133 and this is 121".
#[tauri::command]
pub async fn match_installers(
    state: State<'_, Arc<AppState>>,
) -> AppResult<Vec<InstallerMatchRow>> {
    let state = state.inner().clone();

    // Take a copy and release the lock in its own scope. A lock guard held
    // across an `.await` would make this future non-Send, which Tauri rejects
    // - and holding a lock across a suspension point is a deadlock waiting to
    // happen regardless.
    let cached = { state.apps.read().clone() };

    let installed = match cached {
        Some(cached) => cached,
        None => {
            let found = tauri::async_runtime::spawn_blocking(apps::installed)
                .await
                .map_err(|err| AppError::Io(format!("could not read installed software: {err}")))?;
            *state.apps.write() = Some(found.clone());
            found
        }
    };

    let index = state.index.read();
    let index = index.as_ref().ok_or(AppError::NoScan)?;

    let mut rows: Vec<InstallerMatchRow> = index
        .entries
        .iter()
        .filter(|entry| entry.category == Category::Installers && entry.flags.is_safe_to_touch())
        .filter_map(|entry| {
            let name = crate::core::text::display(&entry.name);
            appmatch::match_installer(&name, &installed).map(|matched| InstallerMatchRow {
                file: dto::row(index, entry),
                matched,
            })
        })
        .collect();

    // Biggest first: an obsolete 700 MB installer matters more than a 2 MB one.
    rows.sort_by(|a, b| b.file.size.cmp(&a.file.size));
    Ok(rows)
}
