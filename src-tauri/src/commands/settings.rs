//! Settings commands.

use crate::core::safety;
use crate::core::settings::Settings;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub fn get_settings(state: State<'_, Arc<AppState>>) -> Settings {
    state.settings.read().clone()
}

/// Replace settings wholesale, after clamping anything out of range.
#[tauri::command]
pub fn save_settings(state: State<'_, Arc<AppState>>, settings: Settings) -> AppResult<Settings> {
    let sanitized = settings.sanitized();
    *state.settings.write() = sanitized.clone();
    state
        .save_settings()
        .map_err(|err| AppError::Io(format!("could not write settings: {err}")))?;
    Ok(sanitized)
}

/// A warning about the settings file, consumed once.
///
/// Taking rather than reading it means a corrupt-file notice is shown exactly
/// once per launch instead of nagging on every navigation.
#[tauri::command]
pub fn take_settings_warning(state: State<'_, Arc<AppState>>) -> Option<String> {
    state.settings_warning.lock().take()
}

/// Validate and record a folder the user picked, without starting a scan.
///
/// The same guard a scan uses, so a folder that cannot be scanned is rejected
/// here with the same explanation rather than being accepted and failing later.
#[tauri::command]
pub fn add_root(state: State<'_, Arc<AppState>>, path: String) -> AppResult<Settings> {
    let validated = safety::validate_root(&PathBuf::from(path))?;
    {
        let mut settings = state.settings.write();
        settings.add_root(&validated);
    }
    state
        .save_settings()
        .map_err(|err| AppError::Io(format!("could not write settings: {err}")))?;
    Ok(state.settings.read().clone())
}

/// Withdraw approval for a folder.
///
/// Phase 4's mutation guard checks the approved-roots list, so removing a root
/// here also removes the app's permission to modify anything inside it.
#[tauri::command]
pub fn remove_root(state: State<'_, Arc<AppState>>, path: String) -> AppResult<Settings> {
    {
        let mut settings = state.settings.write();
        settings.roots.retain(|r| r != &path);
    }
    state
        .save_settings()
        .map_err(|err| AppError::Io(format!("could not write settings: {err}")))?;
    Ok(state.settings.read().clone())
}

/// Add a folder Cleanup must never read or modify.
///
/// The path is resolved before being stored, so what lands in settings is the
/// real folder rather than whatever route the picker returned. A path that
/// does not resolve to a folder is rejected now, when the user can see why,
/// rather than being silently ignored every time the guard runs.
#[tauri::command]
pub fn add_excluded_path(state: State<'_, Arc<AppState>>, path: String) -> AppResult<Settings> {
    let requested = PathBuf::from(&path);
    let canonical = std::fs::canonicalize(crate::core::identity::for_io(&requested))
        .map(crate::core::identity::strip_verbatim)
        .map_err(|err| {
            AppError::Invalid(format!(
                "{} could not be resolved ({err}), so it cannot be excluded",
                requested.display()
            ))
        })?;
    if !canonical.is_dir() {
        return Err(AppError::Invalid(format!(
            "{} is not a folder",
            canonical.display()
        )));
    }

    {
        let mut settings = state.settings.write();
        settings.exclude_path(&canonical);
    }
    state
        .save_settings()
        .map_err(|err| AppError::Io(format!("could not write settings: {err}")))?;
    Ok(state.settings.read().clone())
}

/// Stop excluding a folder. Takes the stored string verbatim, so a folder on a
/// drive that is currently disconnected can still be removed from the list.
#[tauri::command]
pub fn remove_excluded_path(state: State<'_, Arc<AppState>>, path: String) -> AppResult<Settings> {
    {
        let mut settings = state.settings.write();
        settings.unexclude_path(&path);
    }
    state
        .save_settings()
        .map_err(|err| AppError::Io(format!("could not write settings: {err}")))?;
    Ok(state.settings.read().clone())
}
