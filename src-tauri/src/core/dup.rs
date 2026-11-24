//! Duplicate detection: four escalating stages, cheapest test first.
//!
//! 1. **Exact size.** A size group of one cannot contain a duplicate, and is
//!    discarded having read **zero bytes**. On a real Downloads folder this
//!    removes 85-95% of files immediately.
//! 2. **Filesystem identity.** Members sharing `(volume, file_id)` are one
//!    file reached twice - a hardlink - not two copies. Collapsing them is what
//!    stops the app promising space that deleting a link would never free.
//! 3. **Head hash.** BLAKE3 over the first 64 KiB: one block read per file.
//!    Different files of equal size almost never share a prefix that long, so
//!    this eliminates nearly all remaining false candidates.
//! 4. **Full hash**, streamed and cached, only for what survived.
//!
//! The pipeline never holds the scan index: `collect_candidates` takes a brief
//! read lock and copies out what it needs, so a long dedupe run cannot block
//! the file list the user is browsing.

use crate::core::db::{CacheEntry, CacheKey, Db};
use crate::core::hash;
use crate::core::model::{EntryFlags, FileIdent, ScanIndex};
use crate::core::progress::CancelToken;

use parking_lot::Mutex;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct DupOptions {
    pub head_bytes: usize,
    /// Concurrent file reads. Not auto-detected: parallel reads are a large
    /// win on NVMe and a loss on a spinning disk, and reliable cross-platform
    /// drive-type detection is not worth the complexity.
    pub concurrency: usize,
    /// Byte-compare each group before reporting it. Off by default; a BLAKE3
    /// collision is a ~2^-128 event.
    pub verify_bytewise: bool,
}

impl Default for DupOptions {
    fn default() -> Self {
        Self {
            head_bytes: hash::DEFAULT_HEAD_BYTES,
            concurrency: 6,
            verify_bytewise: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DupStage {
    Grouping,
    HeadHashing,
    FullHashing,
    Verifying,
    Done,
}

impl DupStage {
    pub fn key(self) -> &'static str {
        match self {
            DupStage::Grouping => "grouping",
            DupStage::HeadHashing => "headHashing",
            DupStage::FullHashing => "fullHashing",
            DupStage::Verifying => "verifying",
            DupStage::Done => "done",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DupProgress {
    pub stage: DupStage,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

pub trait DupSink: Send + Sync {
    fn tick(&self, progress: DupProgress);
}

pub struct NoopDupSink;

impl DupSink for NoopDupSink {
    fn tick(&self, _progress: DupProgress) {}
}

/// Everything the pipeline needs about one file, copied out of the index.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub id: u32,
    pub size: u64,
    pub path: PathBuf,
    pub name_folded: String,
    /// Loose directly in the scan root, rather than filed in a subfolder.
    pub loose_in_root: bool,
    pub created: Option<i64>,
    /// Identity from the walk: present on Unix, absent on Windows. Stage 4
    /// fills the gap from the open handle.
    pub ident: Option<FileIdent>,
}

/// A candidate awaiting the full pass, carrying the modification time read
/// from its open handle so the cache key describes the file we actually hashed.
struct Pending {
    candidate: Candidate,
    mtime_ns: i64,
}

#[derive(Clone, Debug)]
pub struct DupGroup {
    pub hash_hex: String,
    pub size: u64,
    /// Distinct files, `keep` first.
    pub ids: Vec<u32>,
    pub keep: u32,
    pub keep_reason: String,
    /// Freed only if every copy but one goes.
    pub reclaimable: u64,
}

#[derive(Clone, Debug, Default)]
pub struct DupStats {
    pub considered: u64,
    pub skipped_unsafe: u64,
    pub eliminated_by_size: u64,
    pub collapsed_hardlinks: u64,
    pub eliminated_by_head: u64,
    pub head_hashed: u64,
    pub full_hashed: u64,
    pub cache_hits: u64,
    pub bytes_read: u64,
    pub duplicate_files: u64,
    pub reclaimable: u64,
    pub verified_bytewise: bool,
    pub cancelled: bool,
    pub duration_ms: u64,
    pub errors: Vec<String>,
}

pub struct DupReport {
    pub groups: Vec<DupGroup>,
    pub stats: DupStats,
}

/// Copy out what the pipeline needs, under a brief read lock.
pub fn collect_candidates(index: &ScanIndex) -> (Vec<Candidate>, u64) {
    let mut skipped = 0u64;
    let mut out = Vec::new();

    for entry in &index.entries {
        // Zero-byte files are all byte-identical to each other, which is true
        // and useless. Reporting 400 mutual duplicates of nothing is noise.
        if entry.size == 0 {
            continue;
        }
        // In-progress downloads, cloud placeholders, symlinks, app bundles.
        // Hashing these is either impossible or actively harmful.
        if !entry.flags.is_safe_to_touch() {
            if entry.flags.has(EntryFlags::CLOUD_PLACEHOLDER | EntryFlags::IN_PROGRESS) {
                skipped += 1;
            }
            continue;
        }

        let path = index.path_of(entry);
        let loose_in_root = path.parent() == Some(index.root.as_path());
        out.push(Candidate {
            id: entry.id,
            size: entry.size,
            name_folded: entry.name_folded.to_string(),
            path,
            loose_in_root,
            created: entry.created.or(entry.modified),
            ident: entry.ident,
        });
    }

    (out, skipped)
}

pub fn find_duplicates(
    candidates: Vec<Candidate>,
    db: Option<&Db>,
    options: &DupOptions,
    cancel: &CancelToken,
    sink: &dyn DupSink,
) -> DupReport {
    let started = Instant::now();
    let mut stats = DupStats {
        considered: candidates.len() as u64,
        verified_bytewise: options.verify_bytewise,
        ..DupStats::default()
    };

    let pool = match rayon::ThreadPoolBuilder::new()
        .num_threads(options.concurrency.clamp(1, 64))
        .build()
    {
        Ok(pool) => pool,
        Err(err) => {
            stats.errors.push(format!("could not start hashing threads: {err}"));
            stats.duration_ms = started.elapsed().as_millis() as u64;
            return DupReport {
                groups: Vec::new(),
                stats,
            };
        }
    };

    stats.skipped_unsafe = 0; // set by the caller via collect_candidates

    // ---- Stage 1: group by exact size. Zero reads. --------------------------
    sink.tick(DupProgress {
        stage: DupStage::Grouping,
        files_done: 0,
        files_total: candidates.len() as u64,
        bytes_done: 0,
        bytes_total: 0,
    });

    let mut by_size: HashMap<u64, Vec<Candidate>> = HashMap::new();
    for candidate in candidates {
        by_size.entry(candidate.size).or_default().push(candidate);
    }

    let mut survivors: Vec<Vec<Candidate>> = Vec::new();
    for (_, group) in by_size {
        if group.len() < 2 {
            stats.eliminated_by_size += group.len() as u64;
            continue;
        }
        // ---- Stage 2: collapse known hardlinks. -----------------------------
        let (distinct, collapsed) = collapse_by_identity(group);
        stats.collapsed_hardlinks += collapsed;
        if distinct.len() < 2 {
            stats.eliminated_by_size += distinct.len() as u64;
            continue;
        }
        survivors.push(distinct);
    }

    if cancel.is_cancelled() {
        stats.cancelled = true;
        stats.duration_ms = started.elapsed().as_millis() as u64;
        return DupReport {
            groups: Vec::new(),
            stats,
        };
    }

    // ---- Stage 3: head hash. One block read per file. -----------------------
    let head_total: u64 = survivors.iter().map(|g| g.len() as u64).sum();
    let head_bytes_total: u64 = survivors
        .iter()
        .flatten()
        .map(|c| c.size.min(options.head_bytes as u64))
        .sum();

    let done = AtomicU64::new(0);
    let bytes = AtomicU64::new(0);
    let errors = Mutex::new(Vec::<String>::new());
    let ticker = Ticker::new();

    let head_results: Vec<Vec<(Candidate, Option<hash::Hashed>)>> = pool.install(|| {
        survivors
            .into_par_iter()
            .map(|group| {
                group
                    .into_par_iter()
                    .map(|candidate| {
                        if cancel.is_cancelled() {
                            return (candidate, None);
                        }
                        let result = hash::head(&candidate.path, options.head_bytes);
                        done.fetch_add(1, Ordering::Relaxed);
                        bytes.fetch_add(
                            candidate.size.min(options.head_bytes as u64),
                            Ordering::Relaxed,
                        );
                        ticker.maybe(sink, || DupProgress {
                            stage: DupStage::HeadHashing,
                            files_done: done.load(Ordering::Relaxed),
                            files_total: head_total,
                            bytes_done: bytes.load(Ordering::Relaxed),
                            bytes_total: head_bytes_total,
                        });
                        match result {
                            Ok(hashed) => (candidate, Some(hashed)),
                            Err(err) => {
                                push_error(&errors, &candidate.path, &err);
                                (candidate, None)
                            }
                        }
                    })
                    .collect()
            })
            .collect()
    });

    stats.head_hashed = done.load(Ordering::Relaxed);
    stats.bytes_read += bytes.load(Ordering::Relaxed);

    if cancel.is_cancelled() {
        stats.cancelled = true;
        stats.errors = errors.into_inner();
        stats.duration_ms = started.elapsed().as_millis() as u64;
        return DupReport {
            groups: Vec::new(),
            stats,
        };
    }

    // Sub-group by head digest. A file whose head covered the whole file is
    // already fully hashed and needs no second pass.
    let mut needs_full: Vec<Pending> = Vec::new();
    let mut complete: Vec<(Candidate, [u8; 32], Option<FileIdent>)> = Vec::new();

    for group in head_results {
        let mut by_head: HashMap<[u8; 32], Vec<(Candidate, hash::Hashed)>> = HashMap::new();
        for (candidate, hashed) in group {
            match hashed {
                Some(hashed) => by_head.entry(hashed.hash).or_default().push((candidate, hashed)),
                None => {} // unreadable or cancelled; already recorded
            }
        }
        for (_, members) in by_head {
            if members.len() < 2 {
                stats.eliminated_by_head += members.len() as u64;
                continue;
            }
            for (mut candidate, hashed) in members {
                // Learn the file's identity here.
                //
                // This is load-bearing, not tidying: the cache is keyed on
                // identity, and the metadata walk cannot supply it on Windows
                // (std's accessors are unstable, and the stable call costs a
                // handle per file). The head stage has already opened the
                // file, so this is where it becomes available on every
                // platform - and without it the cache lookup below would have
                // no key to look up, so Windows would silently never get a
                // cache hit.
                if candidate.ident.is_none() {
                    candidate.ident = hashed.ident;
                }
                if hashed.covers_whole_file {
                    complete.push((candidate, hashed.hash, hashed.ident));
                } else {
                    needs_full.push(Pending {
                        candidate,
                        mtime_ns: hashed.mtime_ns,
                    });
                }
            }
        }
    }

    // ---- Stage 4: full hash, cache first. -----------------------------------
    //
    // Cache access is batched: one read transaction here, one write at the
    // end, and no database contact at all from the hashing threads.
    let mut cached: Vec<Option<[u8; 32]>> = vec![None; needs_full.len()];
    if let Some(db) = db {
        let keys: Vec<Option<CacheKey>> = needs_full
            .iter()
            .map(|pending| {
                pending.candidate.ident.map(|ident| CacheKey {
                    ident,
                    size: pending.candidate.size,
                    mtime_ns: pending.mtime_ns,
                })
            })
            .collect();
        let present: Vec<CacheKey> = keys.iter().flatten().copied().collect();
        if !present.is_empty() {
            let found = db.lookup(&present);
            let mut cursor = 0usize;
            for (slot, key) in cached.iter_mut().zip(&keys) {
                if key.is_some() {
                    *slot = found.get(cursor).copied().flatten();
                    cursor += 1;
                }
            }
        }
    }

    let full_total = needs_full.len() as u64;
    let full_bytes_total: u64 = needs_full
        .iter()
        .zip(&cached)
        .filter(|(_, hit)| hit.is_none())
        .map(|(pending, _)| pending.candidate.size)
        .sum();
    let full_done = AtomicU64::new(0);
    let full_bytes = AtomicU64::new(0);
    let cache_hits = AtomicU64::new(0);

    let full_results: Vec<(Candidate, Option<[u8; 32]>, Option<FileIdent>, Option<CacheEntry>)> =
        pool.install(|| {
            needs_full
                .into_par_iter()
                .zip(cached)
                .map(|(Pending { candidate, .. }, hit)| {
                    if let Some(hash) = hit {
                        cache_hits.fetch_add(1, Ordering::Relaxed);
                        full_done.fetch_add(1, Ordering::Relaxed);
                        return (candidate, Some(hash), None, None);
                    }
                    if cancel.is_cancelled() {
                        return (candidate, None, None, None);
                    }
                    let outcome = hash::full(&candidate.path, cancel);
                    full_done.fetch_add(1, Ordering::Relaxed);
                    full_bytes.fetch_add(candidate.size, Ordering::Relaxed);
                    ticker.maybe(sink, || DupProgress {
                        stage: DupStage::FullHashing,
                        files_done: full_done.load(Ordering::Relaxed),
                        files_total: full_total,
                        bytes_done: full_bytes.load(Ordering::Relaxed),
                        bytes_total: full_bytes_total,
                    });
                    match outcome {
                        Ok(Some(hashed)) => {
                            let entry = hashed.ident.map(|ident| CacheEntry {
                                key: CacheKey {
                                    ident,
                                    size: hashed.size,
                                    mtime_ns: hashed.mtime_ns,
                                },
                                hash: hashed.hash,
                            });
                            (candidate, Some(hashed.hash), hashed.ident, entry)
                        }
                        // Cancelled mid-file: unknown, never "different".
                        Ok(None) => (candidate, None, None, None),
                        Err(err) => {
                            push_error(&errors, &candidate.path, &err);
                            (candidate, None, None, None)
                        }
                    }
                })
                .collect()
        });

    stats.cache_hits = cache_hits.load(Ordering::Relaxed);
    stats.full_hashed = full_done.load(Ordering::Relaxed).saturating_sub(stats.cache_hits);
    stats.bytes_read += full_bytes.load(Ordering::Relaxed);

    if let Some(db) = db {
        let to_store: Vec<CacheEntry> = full_results
            .iter()
            .filter_map(|(_, _, _, entry)| *entry)
            .collect();
        if let Err(err) = db.store(&to_store) {
            errors
                .lock()
                .push(format!("could not write the hash cache: {err}"));
        }
    }

    // ---- Final grouping -----------------------------------------------------
    let mut by_hash: HashMap<[u8; 32], Vec<Candidate>> = HashMap::new();
    for (candidate, hash, ident, _) in full_results {
        if let Some(hash) = hash {
            // Identity learned from the open handle - on Windows this is the
            // only place it is available, and stage 2 could not use it.
            let mut candidate = candidate;
            if candidate.ident.is_none() {
                candidate.ident = ident;
            }
            by_hash.entry(hash).or_default().push(candidate);
        }
    }
    for (candidate, hash, ident) in complete {
        let mut candidate = candidate;
        if candidate.ident.is_none() {
            candidate.ident = ident;
        }
        by_hash.entry(hash).or_default().push(candidate);
    }

    let mut groups: Vec<DupGroup> = Vec::new();
    for (digest, members) in by_hash {
        // Collapse again: identity may only now be known.
        let (mut members, collapsed) = collapse_by_identity(members);
        stats.collapsed_hardlinks += collapsed;
        if members.len() < 2 {
            continue;
        }

        if options.verify_bytewise {
            sink.tick(DupProgress {
                stage: DupStage::Verifying,
                files_done: 0,
                files_total: members.len() as u64,
                bytes_done: 0,
                bytes_total: 0,
            });
            members = verify_group(members, cancel, &errors);
            if members.len() < 2 {
                continue;
            }
        }

        let (keep, reason) = rank_keep(&members);
        // Deterministic order, keep first, so the UI never reshuffles.
        members.sort_by(|a, b| {
            (a.id != keep)
                .cmp(&(b.id != keep))
                .then_with(|| a.path.cmp(&b.path))
        });

        let reclaimable = digest_reclaimable(&members);
        stats.duplicate_files += members.len() as u64;
        stats.reclaimable += reclaimable;

        groups.push(DupGroup {
            hash_hex: hex32(&digest),
            size: members.first().map(|c| c.size).unwrap_or(0),
            ids: members.iter().map(|c| c.id).collect(),
            keep,
            keep_reason: reason,
            reclaimable,
        });
    }

    // Biggest win first - the order a user wants to work through.
    groups.sort_by(|a, b| {
        b.reclaimable
            .cmp(&a.reclaimable)
            .then_with(|| b.size.cmp(&a.size))
            .then_with(|| a.hash_hex.cmp(&b.hash_hex))
    });

    stats.cancelled = cancel.is_cancelled();
    stats.errors = {
        let mut list = errors.into_inner();
        list.truncate(200);
        list
    };
    stats.duration_ms = started.elapsed().as_millis() as u64;

    sink.tick(DupProgress {
        stage: DupStage::Done,
        files_done: stats.duplicate_files,
        files_total: stats.duplicate_files,
        bytes_done: stats.bytes_read,
        bytes_total: stats.bytes_read,
    });

    DupReport { groups, stats }
}

fn digest_reclaimable(members: &[Candidate]) -> u64 {
    let size = members.first().map(|c| c.size).unwrap_or(0);
    size.saturating_mul(members.len().saturating_sub(1) as u64)
}

/// Keep one entry per `(volume, file_id)`. Returns the survivors and how many
/// were folded away.
fn collapse_by_identity(members: Vec<Candidate>) -> (Vec<Candidate>, u64) {
    let mut seen: HashMap<(u64, u64), ()> = HashMap::new();
    let mut out = Vec::with_capacity(members.len());
    let mut collapsed = 0u64;

    for candidate in members {
        match candidate.ident {
            Some(ident) => {
                if seen.insert((ident.volume, ident.file_id), ()).is_none() {
                    out.push(candidate);
                } else {
                    collapsed += 1;
                }
            }
            // Unknown identity: keep it. Over-counting a candidate is
            // harmless - the user still sees both paths - whereas dropping one
            // would hide a real duplicate.
            None => out.push(candidate),
        }
    }

    (out, collapsed)
}

/// Byte-compare every member against the first, for `verifyBytewise`.
fn verify_group(
    members: Vec<Candidate>,
    cancel: &CancelToken,
    errors: &Mutex<Vec<String>>,
) -> Vec<Candidate> {
    let Some(reference) = members.first().cloned() else {
        return members;
    };
    let mut out = vec![reference.clone()];
    for candidate in members.into_iter().skip(1) {
        match hash::verify_identical(&reference.path, &candidate.path, cancel) {
            Ok(true) => out.push(candidate),
            Ok(false) => {}
            Err(err) => {
                push_error(errors, &candidate.path, &err);
            }
        }
    }
    out
}

/// Recommend which copy to keep - a *suggestion* with a stated reason, never a
/// selection. Nothing is pre-ticked for deletion anywhere in the UI.
///
/// Signals, in order of weight:
/// 1. A name with no copy marker beats `report (1).pdf` or `report - Copy.pdf`.
/// 2. A file deliberately filed in a subfolder beats one loose in the root.
/// 3. The oldest beats the rest, as most likely the original.
/// 4. Path order, purely so the result is deterministic.
fn rank_keep(members: &[Candidate]) -> (u32, String) {
    let oldest = members.iter().filter_map(|c| c.created).min();

    let mut best: Option<(i32, &Candidate, &'static str)> = None;
    for candidate in members {
        let marker = copy_marker(&candidate.name_folded);
        let mut score = 0i32;
        let mut reason = "Chosen so the result is stable - the copies are otherwise identical";

        if marker.is_none() {
            score += 40;
            reason = "Its name has no \"copy\" or \"(1)\" marker";
        }
        if !candidate.loose_in_root {
            score += 25;
            if marker.is_none() {
                reason = "Filed in a subfolder rather than left loose, and its name has no copy marker";
            } else {
                reason = "Filed in a subfolder rather than left loose in the scanned folder";
            }
        }
        if oldest.is_some() && candidate.created == oldest {
            score += 10;
            if marker.is_some() && candidate.loose_in_root {
                reason = "The oldest of the group, so most likely the original";
            }
        }

        let better = match best {
            None => true,
            Some((best_score, best_candidate, _)) => {
                score > best_score
                    || (score == best_score && candidate.path < best_candidate.path)
            }
        };
        if better {
            best = Some((score, candidate, reason));
        }
    }

    match best {
        Some((_, candidate, reason)) => (candidate.id, reason.to_string()),
        None => (0, String::new()),
    }
}

/// Detect the copy markers the major platforms and browsers actually produce.
///
/// Deliberately narrow: a loose `contains("copy")` would flag
/// `copywriting-notes.pdf`, and a wrong keep-suggestion is worse than none.
pub fn copy_marker(name_folded: &str) -> Option<&'static str> {
    let stem = name_folded
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(name_folded);

    if stem.ends_with(" - copy") || stem.contains(" - copy (") {
        return Some("Windows \"- Copy\" suffix");
    }
    if stem.ends_with(" copy") {
        return Some("macOS \" copy\" suffix");
    }
    // "file copy 2", "file copy 10"
    if let Some(rest) = stem.rsplit_once(" copy ").map(|(_, rest)| rest) {
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            return Some("macOS \" copy N\" suffix");
        }
    }
    if stem.starts_with("copy of ") {
        return Some("\"Copy of\" prefix");
    }
    if stem.contains("(another copy)") || stem.contains("(duplicate)") {
        return Some("Linux duplicate marker");
    }
    // Browser style: "report (1)", "report(2)"
    if let Some(open) = stem.rfind('(') {
        if stem.ends_with(')') {
            let inner = &stem[open + 1..stem.len() - 1];
            if !inner.is_empty() && inner.chars().all(|c| c.is_ascii_digit()) {
                return Some("browser \"(N)\" suffix");
            }
        }
    }
    None
}

fn push_error(errors: &Mutex<Vec<String>>, path: &Path, err: &std::io::Error) {
    let mut list = errors.lock();
    if list.len() < 200 {
        list.push(format!(
            "{}: {err}",
            crate::core::identity::shorten_for_display(path.as_os_str())
        ));
    }
}

fn hex32(bytes: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Throttles progress events to roughly ten a second from any number of
/// threads. Emitting one per file is how an IPC-bridged UI ends up slower than
/// the disk it is reporting on.
struct Ticker {
    last: Mutex<Instant>,
}

impl Ticker {
    fn new() -> Self {
        Ticker {
            last: Mutex::new(
                Instant::now()
                    .checked_sub(std::time::Duration::from_millis(200))
                    .unwrap_or_else(Instant::now),
            ),
        }
    }

    fn maybe<F>(&self, sink: &dyn DupSink, build: F)
    where
        F: FnOnce() -> DupProgress,
    {
        let mut last = self.last.lock();
        if last.elapsed() < std::time::Duration::from_millis(100) {
            return;
        }
        *last = Instant::now();
        drop(last);
        sink.tick(build());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::progress::NoopSink;
    use crate::core::walk::{scan, WalkOptions};

    struct Temp(PathBuf);

    impl Temp {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "cleanup-dup-{tag}-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Temp(dir)
        }
        fn write(&self, rel: &str, bytes: &[u8]) {
            let path = self.0.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, bytes).unwrap();
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn index_of(t: &Temp) -> ScanIndex {
        let options = WalkOptions {
            in_progress_grace_secs: 0,
            ..WalkOptions::default()
        };
        scan(&t.0, 1, &options, &CancelToken::new(), &NoopSink).unwrap()
    }

    fn run(t: &Temp, options: DupOptions) -> DupReport {
        let index = index_of(t);
        let (candidates, _) = collect_candidates(&index);
        find_duplicates(
            candidates,
            None,
            &options,
            &CancelToken::new(),
            &NoopDupSink,
        )
    }

    #[test]
    fn finds_identical_files_with_different_names() {
        let t = Temp::new("basic");
        t.write("report.pdf", b"identical payload here");
        t.write("report-final.pdf", b"identical payload here");
        t.write("unrelated.txt", b"something else entirely");

        let report = run(&t, DupOptions::default());
        assert_eq!(report.groups.len(), 1);
        let group = &report.groups[0];
        assert_eq!(group.ids.len(), 2);
        assert_eq!(group.reclaimable, 22);
        assert_eq!(report.stats.duplicate_files, 2);
    }

    #[test]
    fn same_size_different_content_is_not_a_duplicate() {
        let t = Temp::new("samesize");
        t.write("a.bin", b"AAAAAAAAAA");
        t.write("b.bin", b"BBBBBBBBBB");
        let report = run(&t, DupOptions::default());
        assert!(report.groups.is_empty(), "equal size is a hint, not proof");
    }

    #[test]
    fn unique_sizes_are_eliminated_without_reading_anything() {
        let t = Temp::new("sizes");
        t.write("a.bin", &[1u8; 10]);
        t.write("b.bin", &[2u8; 20]);
        t.write("c.bin", &[3u8; 30]);

        let report = run(&t, DupOptions::default());
        assert!(report.groups.is_empty());
        assert_eq!(report.stats.eliminated_by_size, 3);
        assert_eq!(report.stats.head_hashed, 0, "nothing should be opened");
        assert_eq!(report.stats.bytes_read, 0);
    }

    #[test]
    fn files_sharing_a_prefix_need_the_full_pass_and_are_told_apart() {
        let t = Temp::new("prefix");
        let mut a = vec![4u8; 200_000];
        let mut b = a.clone();
        a[199_999] = 1;
        b[199_999] = 2;
        t.write("a.bin", &a);
        t.write("b.bin", &b);

        let report = run(&t, DupOptions::default());
        assert!(report.groups.is_empty());
        assert_eq!(report.stats.head_hashed, 2);
        assert_eq!(report.stats.full_hashed, 2, "the head stage could not decide");
    }

    #[test]
    fn small_files_skip_the_full_pass_entirely() {
        let t = Temp::new("small");
        t.write("a.bin", b"tiny and identical");
        t.write("b.bin", b"tiny and identical");

        let report = run(&t, DupOptions::default());
        assert_eq!(report.groups.len(), 1);
        assert_eq!(
            report.stats.full_hashed, 0,
            "the head read already covered every byte"
        );
    }

    #[test]
    fn three_copies_report_two_as_reclaimable() {
        let t = Temp::new("three");
        for name in ["one.bin", "two.bin", "three.bin"] {
            t.write(name, b"same content in all three");
        }
        let report = run(&t, DupOptions::default());
        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.groups[0].ids.len(), 3);
        assert_eq!(report.groups[0].reclaimable, 25 * 2);
    }

    #[test]
    fn hardlinks_are_collapsed_rather_than_offered_as_savings() {
        let t = Temp::new("hardlink");
        t.write("original.bin", b"shared bytes");
        if std::fs::hard_link(t.0.join("original.bin"), t.0.join("link.bin")).is_err() {
            return;
        }
        let report = run(&t, DupOptions::default());
        assert!(
            report.groups.is_empty(),
            "one file at two paths is not two copies to delete"
        );
        assert!(report.stats.collapsed_hardlinks >= 1);
    }

    #[test]
    fn zero_byte_files_are_left_out() {
        let t = Temp::new("zero");
        t.write("a.txt", b"");
        t.write("b.txt", b"");
        let report = run(&t, DupOptions::default());
        assert!(report.groups.is_empty());
        assert_eq!(report.stats.considered, 0);
    }

    #[test]
    fn the_keep_suggestion_avoids_the_copy_marked_name() {
        let t = Temp::new("keep-name");
        t.write("invoice.pdf", b"the invoice contents");
        t.write("invoice (1).pdf", b"the invoice contents");

        let index = index_of(&t);
        let (candidates, _) = collect_candidates(&index);
        let report = find_duplicates(
            candidates,
            None,
            &DupOptions::default(),
            &CancelToken::new(),
            &NoopDupSink,
        );

        let group = &report.groups[0];
        let kept = index.entry(group.keep).unwrap();
        assert_eq!(crate::core::text::display(&kept.name), "invoice.pdf");
        assert!(!group.keep_reason.is_empty());
        // Keep is listed first so the UI never has to hunt for it.
        assert_eq!(group.ids[0], group.keep);
    }

    #[test]
    fn the_keep_suggestion_prefers_a_filed_copy_over_a_loose_one() {
        let t = Temp::new("keep-filed");
        t.write("photo.jpg", b"image bytes right here");
        t.write("Pictures/photo.jpg", b"image bytes right here");

        let index = index_of(&t);
        let (candidates, _) = collect_candidates(&index);
        let report = find_duplicates(
            candidates,
            None,
            &DupOptions::default(),
            &CancelToken::new(),
            &NoopDupSink,
        );

        let kept = index.path_of(index.entry(report.groups[0].keep).unwrap());
        assert!(
            kept.parent().unwrap().ends_with("Pictures"),
            "the deliberately filed copy should be the one kept, got {kept:?}"
        );
    }

    #[test]
    fn copy_markers_are_recognised_without_false_positives() {
        assert!(copy_marker("report (1).pdf").is_some());
        assert!(copy_marker("report - copy.pdf").is_some());
        assert!(copy_marker("report copy.pdf").is_some());
        assert!(copy_marker("report copy 2.pdf").is_some());
        assert!(copy_marker("copy of report.pdf").is_some());

        // Must not fire on ordinary names that merely contain the word.
        assert!(copy_marker("copywriting-notes.pdf").is_none());
        assert!(copy_marker("photocopier manual.pdf").is_none());
        assert!(copy_marker("report.pdf").is_none());
        assert!(copy_marker("budget (final).xlsx").is_none());
    }

    #[test]
    fn bytewise_verification_keeps_genuine_duplicates() {
        let t = Temp::new("verify");
        t.write("a.bin", &[8u8; 150_000]);
        t.write("b.bin", &[8u8; 150_000]);

        let report = run(
            &t,
            DupOptions {
                verify_bytewise: true,
                ..DupOptions::default()
            },
        );
        assert_eq!(report.groups.len(), 1);
        assert!(report.stats.verified_bytewise);
    }

    #[test]
    fn cancellation_yields_no_groups_and_says_so() {
        let t = Temp::new("cancel");
        t.write("a.bin", &[1u8; 100_000]);
        t.write("b.bin", &[1u8; 100_000]);

        let index = index_of(&t);
        let (candidates, _) = collect_candidates(&index);
        let cancel = CancelToken::new();
        cancel.cancel();
        let report = find_duplicates(
            candidates,
            None,
            &DupOptions::default(),
            &cancel,
            &NoopDupSink,
        );
        assert!(report.groups.is_empty());
        assert!(report.stats.cancelled);
    }

    #[test]
    fn the_cache_removes_the_second_runs_reads() {
        let t = Temp::new("cache");
        // Larger than the head window, so a full pass is genuinely required.
        let payload = vec![3u8; 200_000];
        t.write("a.bin", &payload);
        t.write("b.bin", &payload);

        let db = Db::open_in_memory().unwrap();
        let index = index_of(&t);

        let (first, _) = collect_candidates(&index);
        let first_run = find_duplicates(
            first,
            Some(&db),
            &DupOptions::default(),
            &CancelToken::new(),
            &NoopDupSink,
        );
        assert_eq!(first_run.groups.len(), 1);
        assert_eq!(first_run.stats.full_hashed, 2);
        assert_eq!(first_run.stats.cache_hits, 0);

        let (second, _) = collect_candidates(&index);
        let second_run = find_duplicates(
            second,
            Some(&db),
            &DupOptions::default(),
            &CancelToken::new(),
            &NoopDupSink,
        );
        assert_eq!(second_run.groups.len(), 1, "same answer");
        assert_eq!(second_run.stats.cache_hits, 2, "served from the cache");
        assert_eq!(second_run.stats.full_hashed, 0, "nothing re-read in full");
    }

    #[test]
    fn groups_are_ordered_by_what_they_would_free() {
        let t = Temp::new("order");
        t.write("big-a.bin", &[1u8; 90_000]);
        t.write("big-b.bin", &[1u8; 90_000]);
        t.write("small-a.bin", &[2u8; 1_000]);
        t.write("small-b.bin", &[2u8; 1_000]);

        let report = run(&t, DupOptions::default());
        assert_eq!(report.groups.len(), 2);
        assert!(report.groups[0].reclaimable > report.groups[1].reclaimable);
    }
}
