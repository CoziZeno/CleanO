//! Cleanup - intelligent cross-platform desktop file organizer.
//!
//! Layering, from the bottom up:
//!
//! * `core`   - all logic, no Tauri dependency, unit-tested against real temp
//!              directories.
//! * `state`  - the in-memory scan index and scan runtime.
//! * `error`  - the single error shape that crosses IPC.
//! * `commands` - thin adapters the webview calls.
//!
//! The frontend holds **no filesystem permissions at all** (see
//! `capabilities/default.json`). Every read goes through a command, and from
//! Phase 4 every write goes through `core::safety::guard` first.

#[cfg(feature = "app")]
pub mod commands;
pub mod core;
pub mod error;
pub mod state;

#[cfg(feature = "app")]
use state::AppState;
#[cfg(feature = "app")]
use std::sync::Arc;
#[cfg(feature = "app")]
use tauri::Manager;

#[cfg(feature = "app")]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // Per-user config directory. Falling back to a temp subdirectory
            // keeps the app usable (settings just will not persist) rather than
            // refusing to start on an unusual profile setup.
            let config_dir = app
                .path()
                .app_config_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("dev.cleanup.app"));
            if let Err(err) = std::fs::create_dir_all(&config_dir) {
                eprintln!("cleanup: could not create {config_dir:?}: {err}");
            }
            app.manage(Arc::new(AppState::new(config_dir)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::scan::platform_info,
            commands::scan::scan_state,
            commands::scan::start_scan,
            commands::scan::cancel_scan,
            commands::scan::get_summary,
            commands::scan::get_scan_errors,
            commands::query::query_files,
            commands::query::get_rows,
            commands::query::query_ids,
            commands::query::resolve_path,
            commands::duplicates::dedupe_status,
            commands::duplicates::start_dedupe,
            commands::duplicates::cancel_dedupe,
            commands::duplicates::get_duplicates,
            commands::duplicates::hash_cache_count,
            commands::duplicates::clear_hash_cache,
            commands::storage::get_storage_report,
            commands::organize::get_organize_plan,
            commands::cleanup::trash_files,
            commands::cleanup::apply_organize,
            commands::cleanup::move_files_to,
            commands::cleanup::undo_operation,
            commands::cleanup::list_operations,
            commands::cleanup::take_recovery_notice,
            commands::cleanup::lifetime_stats,
            commands::thumbs::get_thumbnails,
            commands::thumbs::thumb_cache_usage,
            commands::thumbs::clear_thumb_cache,
            commands::apps::list_installed_apps,
            commands::apps::match_installers,
            commands::query::quick_find,
            commands::files::file_path,
            commands::files::rename_file,
            commands::files::copy_files_to,
            commands::files::create_folder,
            commands::files::create_folder_and_move,
            commands::files::rows_for,
            commands::swipe::swipe_queue,
            commands::swipe::swipe_decide,
            commands::swipe::swipe_forget,
            commands::swipe::swipe_reset,
            commands::settings::get_settings,
            commands::settings::save_settings,
            commands::settings::take_settings_warning,
            commands::settings::add_root,
            commands::settings::remove_root,
            commands::quickclean::quick_clean_scan,
            commands::quickclean::quick_clean_run,
            commands::drives::list_drives,
            commands::drives::add_drive_root,
            commands::storage::get_treemap,
            commands::settings::add_excluded_path,
            commands::settings::remove_excluded_path,
        ])
        .run(tauri::generate_context!())
        .expect("fatal error while running Cleanup");
}
