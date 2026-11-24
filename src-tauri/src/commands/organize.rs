//! Organization suggestions.
//!
//! Read-only in Phase 3. The plan carries real, resolved destination paths -
//! including the renames that collisions would force - so the user can see
//! exactly what applying it would do before anything is able to do it.
//! Applying, with its journal and undo, is Phase 4.

use crate::core::dto::OrganizePlan;
use crate::core::suggest;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub fn get_organize_plan(
    state: State<'_, Arc<AppState>>,
    mode: Option<suggest::OrganizeMode>,
) -> AppResult<OrganizePlan> {
    let index = state.index.read();
    let index = index.as_ref().ok_or(AppError::NoScan)?;
    let settings = state.settings.read();
    Ok(suggest::organize_plan(
        index,
        &settings,
        mode.unwrap_or_default(),
    ))
}
