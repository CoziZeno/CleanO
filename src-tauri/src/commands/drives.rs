//! Drive commands.
//!
//! Listing drives is read-only. Approving one as a scan root is not, which is
//! why it is a separate command with its own validation rather than a special
//! case inside `add_root`: `safety::validate_root` refuses volume roots on
//! purpose, and that refusal should only be lifted by a path the user walked
//! down deliberately.

use crate::core::drives::{self, Drive};
use crate::core::settings::Settings;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub fn list_drives() -> Vec<Drive> {
    drives::list()
}

/// Approve a whole drive as a scan root.
///
/// The UI confirms this with the user first, and the walk still refuses to
/// descend into system and application folders, so "scan this drive" means
/// the user's own files on it - not the operating system.
#[tauri::command]
pub fn add_drive_root(state: State<'_, Arc<AppState>>, path: String) -> AppResult<Settings> {
    let validated = drives::validate_drive_root(&PathBuf::from(path))?;
    {
        let mut settings = state.settings.write();
        settings.add_root(&validated);
    }
    state
        .save_settings()
        .map_err(|err| AppError::Io(format!("could not write settings: {err}")))?;
    Ok(state.settings.read().clone())
}
