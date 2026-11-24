//! Measure the scan pipeline against a real directory.
//!
//!   cargo run --release --example bench --no-default-features -- <path>
//!
//! Reports each stage separately, because "the scan is slow" is not actionable
//! and "the summary takes 400 ms because it sorts the whole index three times"
//! is. Needs no Tauri: it runs `core` directly, which is the point of keeping
//! `core` free of it.

use cleanup_lib::core::analyze;
use cleanup_lib::core::dto::{QueryRequest, SortKey};
use cleanup_lib::core::model::now_unix_secs;
use cleanup_lib::core::progress::{CancelToken, NoopSink};
use cleanup_lib::core::query;
use cleanup_lib::core::walk::{scan, WalkOptions};
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let root = match std::env::args().nth(1) {
        Some(path) => PathBuf::from(path),
        None => {
            eprintln!("usage: bench <folder>");
            std::process::exit(2);
        }
    };

    let options = WalkOptions::default();
    let cancel = CancelToken::new();

    let started = Instant::now();
    let index = match scan(&root, 1, &options, &cancel, &NoopSink) {
        Ok(index) => index,
        Err(err) => {
            eprintln!("scan failed: {err}");
            std::process::exit(1);
        }
    };
    let walk_ms = started.elapsed().as_millis();

    println!("walk              {walk_ms:>7} ms   {} files, {} dirs, {} errors",
        index.entries.len(), index.dirs.len(), index.errors.len());
    println!("                              {:.1} MB indexed",
        index.total_bytes() as f64 / 1024.0 / 1024.0);
    // Without this the file count looks like data loss rather than the depth
    // cap doing its job, which is exactly the confusion a benchmark should not
    // create.
    if index.truncated {
        println!("                              TRUNCATED at depth {} - counts are a lower bound",
            options.max_depth);
    }

    let t = Instant::now();
    let summary = analyze::summarize(&index, 180, now_unix_secs());
    println!("summarize         {:>7} ms   {} categories, {} dup candidates",
        t.elapsed().as_millis(), summary.by_category.len(), summary.duplicate_candidates.files);

    let t = Instant::now();
    let folders = analyze::folder_rollup(&index, 20);
    println!("folder rollup     {:>7} ms   {} folders ranked", t.elapsed().as_millis(), folders.len());

    // The query the UI runs on every keystroke. This is the one that has to
    // stay in single-digit milliseconds or typing feels laggy.
    let request = QueryRequest {
        text: Some("a".into()),
        sort: SortKey::Size,
        descending: true,
        limit: 200,
        ..QueryRequest::default()
    };
    let t = Instant::now();
    let mut matched = 0;
    for _ in 0..10 {
        matched = query::query(&index, &request).total;
    }
    println!("query (x10)       {:>7} ms   {matched} matches each", t.elapsed().as_millis());

    let request = QueryRequest {
        sort: SortKey::Name,
        limit: 200,
        ..QueryRequest::default()
    };
    let t = Instant::now();
    for _ in 0..10 {
        query::query(&index, &request);
    }
    println!("sort by name (x10){:>7} ms", t.elapsed().as_millis());

    let t = Instant::now();
    let candidates = analyze::duplicate_candidates(&index);
    println!("dup candidates    {:>7} ms   {} groups, {} files",
        t.elapsed().as_millis(), candidates.groups, candidates.files);

    println!("\ntotal to first paint: {} ms", walk_ms);
}
