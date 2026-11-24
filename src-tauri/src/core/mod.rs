//! Core logic. **This module must never depend on `tauri`.**
//!
//! That single constraint is what makes the scanner, the categoriser, the
//! aggregation, the query engine and the safety rules testable with
//! `cargo test` against real temp directories - no window, no webview, no
//! event loop. Progress and cancellation cross the boundary as the
//! `ProgressSink` trait and `CancelToken` in `progress`, which the command
//! layer implements over an `AppHandle`.
//!
//! Phases 3 and 4 add `hash`, `dup`, `db`, `fsops`, `journal` and `suggest`
//! here, under the same rule; `safety` already holds the denylist they will use.

pub mod analyze;
pub mod appmatch;
pub mod apps;
pub mod category;
pub mod db;
pub mod drives;
pub mod dto;
pub mod dup;
pub mod fsops;
pub mod hash;
pub mod identity;
pub mod journal;
pub mod model;
pub mod progress;
pub mod query;
pub mod quickclean;
pub mod raw;
pub mod quickfind;
pub mod safety;
pub mod settings;
pub mod suggest;
pub mod text;
pub mod thumb;
pub mod walk;
