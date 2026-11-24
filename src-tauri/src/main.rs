// Hide the console window on Windows release builds. Debug builds keep it, so
// `eprintln!` diagnostics remain visible during development.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Without the `app` feature there is no GUI to start - that build exists
    // so the core can be compiled and tested without Tauri. Gating here keeps
    // `cargo check --all-targets --no-default-features` clean rather than
    // failing on a binary that was never meant to exist in that mode.
    #[cfg(feature = "app")]
    cleanup_lib::run();
}
