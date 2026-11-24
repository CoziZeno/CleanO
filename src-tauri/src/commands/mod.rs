//! IPC command layer.
//!
//! Every command here is thin on purpose: resolve arguments, validate, call
//! into `core`, serialise. No business logic, no filesystem walking, no
//! policy decisions. If a command grows past about twenty lines of anything
//! other than plumbing, that logic belongs in `core` where it can be tested.

pub mod apps;
pub mod cleanup;
pub mod drives;
pub mod files;
pub mod duplicates;
pub mod organize;
pub mod query;
pub mod quickclean;
pub mod scan;
pub mod settings;
pub mod storage;
pub mod swipe;
pub mod thumbs;
