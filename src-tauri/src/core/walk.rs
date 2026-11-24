//! Parallel metadata walk.
//!
//! Reads no file contents, with one bounded exception: a 32-byte header read
//! for files that have no extension at all (see `category::categorize_from_magic`),
//! and only when the entry passed `EntryFlags::is_safe_to_touch()`.
//!
//! The guards live *inside* the traversal, which is why this is hand-written
//! rather than layered on top of a walker crate:
//!
//! * Symlinks and Windows reparse points are recorded but never traversed.
//! * Directory identity `(volume, inode)` is tracked, so junctions, macOS
//!   firmlinks and bind mounts cannot produce an infinite walk or a tree that
//!   appears twice.
//! * A depth cap makes a pathological tree slow, not fatal, and sets
//!   `truncated` so the UI can say the totals are a lower bound.
//! * macOS `*.app` bundles are a single entry. A bundle is a directory with
//!   tens of thousands of files inside it; traversing one floods the index and
//!   would let a later "organize" pass tear an application apart.
//! * A read error on one directory is recorded and the walk continues.

use crate::core::category;
use crate::core::identity;
use crate::core::model::{
    now_unix_secs, to_unix_secs, Category, DirRecord, EntryFlags, FileEntry, PathIntern, ScanError,
    ScanErrorKind, ScanIndex,
};
use crate::core::progress::{CancelToken, ProgressSink, ProgressSnapshot};
use crate::core::text;

use parking_lot::Mutex;
use rayon::prelude::*;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct WalkOptions {
    /// Levels below the root. The cap exists so a pathological tree degrades
    /// instead of hanging, not to keep the scan small - 20 covers essentially
    /// any real folder, including unpacked archives and project trees.
    pub max_depth: usize,
    pub include_hidden: bool,
    /// Patterns applied to the folded name. Supports a single leading or
    /// trailing `*`.
    pub ignore: Vec<String>,
    /// Read 32 bytes to classify files with no extension.
    pub sniff_extensionless: bool,
    /// A file modified this recently is assumed to still be being written.
    pub in_progress_grace_secs: i64,
    /// Folders to leave alone entirely: never opened, never listed, never
    /// descended into. **Canonical paths**, because that is what the walk
    /// compares against - see `Shared::is_excluded`.
    pub excluded: Vec<PathBuf>,
}

impl Default for WalkOptions {
    fn default() -> Self {
        Self {
            max_depth: 20,
            include_hidden: false,
            ignore: vec![
                ".ds_store".into(),
                "thumbs.db".into(),
                "desktop.ini".into(),
                "._*".into(),
            ],
            sniff_extensionless: true,
            // Short on purpose. This is a backstop for download managers that
            // write straight to the final filename instead of using a `.part`
            // extension; too long a window starts excluding files the user
            // just finished downloading from the very analysis they opened the
            // app to see.
            in_progress_grace_secs: 30,
            excluded: Vec::new(),
        }
    }
}

const TICK_INTERVAL: Duration = Duration::from_millis(100);
/// Hard ceiling so a mistakenly chosen root (a whole drive) cannot exhaust
/// memory. Reaching it sets `truncated`.
const MAX_ENTRIES: usize = 2_000_000;
/// Depth cap for summing a macOS app bundle's size.
const BUNDLE_SUM_MAX_DEPTH: usize = 12;

struct Shared<'a> {
    opts: &'a WalkOptions,
    cancel: &'a CancelToken,
    sink: &'a dyn ProgressSink,
    scan_id: u64,
    now: i64,

    files: Mutex<Vec<FileEntry>>,
    dirs: Mutex<Vec<DirRecord>>,
    parents: Mutex<PathIntern>,
    errors: Mutex<Vec<ScanError>>,
    /// Canonical paths of directories already walked. See `identity::dir_key`.
    visited: Mutex<HashSet<PathBuf>>,
    last_tick: Mutex<Instant>,

    n_files: AtomicU64,
    n_dirs: AtomicU64,
    n_bytes: AtomicU64,
    n_errors: AtomicU64,
    truncated: AtomicBool,
}

impl<'a> Shared<'a> {
    fn intern(&self, path: &OsStr) -> u32 {
        self.parents.lock().intern(path)
    }

    /// Is this directory one the user told us to stay out of?
    ///
    /// Takes the canonical path, which the walk already has from `dir_key`, so
    /// a junction pointing into an excluded folder is excluded too - the thing
    /// a name- or text-based check would miss. `starts_with` compares whole
    /// components, so excluding `E:\Unity` leaves `E:\Unity-Backup` alone.
    fn is_excluded(&self, canonical: &Path) -> bool {
        self.opts
            .excluded
            .iter()
            .any(|dir| canonical.starts_with(dir))
    }

    fn is_ignored(&self, folded_name: &str) -> bool {
        self.opts
            .ignore
            .iter()
            .any(|p| text::matches_pattern(folded_name, p))
    }

    /// Returns false when this directory has already been walked.
    fn mark_visited(&self, key: PathBuf) -> bool {
        self.visited.lock().insert(key)
    }

    fn push_error(&self, path: &Path, err: &std::io::Error) {
        self.n_errors.fetch_add(1, Ordering::Relaxed);
        let kind = match err.kind() {
            std::io::ErrorKind::PermissionDenied => {
                // On macOS a TCC denial also surfaces as PermissionDenied; the
                // command layer distinguishes the two by checking whether the
                // path is a protected user folder.
                ScanErrorKind::PermissionDenied
            }
            std::io::ErrorKind::NotFound => ScanErrorKind::NotFound,
            _ => ScanErrorKind::Other,
        };
        let mut errors = self.errors.lock();
        // Keep the list bounded: 2000 distinct failures is already more than a
        // UI can usefully show, and the count is tracked separately.
        if errors.len() < 2000 {
            errors.push(ScanError {
                path: identity::shorten_for_display(path.as_os_str()),
                kind,
                message: err.to_string(),
            });
        }
    }

    fn maybe_tick(&self, current: &Path) {
        let mut last = self.last_tick.lock();
        if last.elapsed() < TICK_INTERVAL {
            return;
        }
        *last = Instant::now();
        drop(last);
        self.sink.tick(ProgressSnapshot {
            scan_id: self.scan_id,
            files: self.n_files.load(Ordering::Relaxed),
            dirs: self.n_dirs.load(Ordering::Relaxed),
            bytes: self.n_bytes.load(Ordering::Relaxed),
            errors: self.n_errors.load(Ordering::Relaxed),
            current: identity::shorten_for_display(current.as_os_str()),
        });
    }

    fn over_entry_limit(&self) -> bool {
        if self.n_files.load(Ordering::Relaxed) as usize >= MAX_ENTRIES {
            self.truncated.store(true, Ordering::Relaxed);
            return true;
        }
        false
    }

    fn should_stop(&self) -> bool {
        self.cancel.is_cancelled() || self.over_entry_limit()
    }
}

/// Walk `root`, returning a complete index.
///
/// Errors only for a root that cannot be opened at all; everything else is
/// recorded in `ScanIndex::errors` and the walk continues.
pub fn scan(
    root: &Path,
    scan_id: u64,
    opts: &WalkOptions,
    cancel: &CancelToken,
    sink: &dyn ProgressSink,
) -> std::io::Result<ScanIndex> {
    let started = Instant::now();

    // Canonicalise up front: this resolves a symlinked root, rejects a
    // nonexistent one, and gives us an absolute path to anchor every later
    // safety check against.
    let canonical = fs::canonicalize(root)?;
    let root = identity::strip_verbatim(canonical);

    let md = fs::symlink_metadata(&root)?;
    if !md.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "scan root is not a directory",
        ));
    }

    // Scanning a folder the user excluded would index every file in it and
    // then refuse every operation on them - confusing, and not what excluding
    // a folder is supposed to mean.
    if opts.excluded.iter().any(|dir| root.starts_with(dir)) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "this folder is on your excluded list, so Cleanup will not scan it",
        ));
    }

    let shared = Shared {
        opts,
        cancel,
        sink,
        scan_id,
        now: now_unix_secs(),
        files: Mutex::new(Vec::new()),
        dirs: Mutex::new(Vec::new()),
        parents: Mutex::new(PathIntern::default()),
        errors: Mutex::new(Vec::new()),
        visited: Mutex::new(HashSet::new()),
        // `checked_sub`, because subtracting from a fresh `Instant` can
        // underflow and panic on a platform whose clock epoch is recent.
        last_tick: Mutex::new(Instant::now().checked_sub(TICK_INTERVAL).unwrap_or_else(Instant::now)),
        n_files: AtomicU64::new(0),
        n_dirs: AtomicU64::new(0),
        n_bytes: AtomicU64::new(0),
        n_errors: AtomicU64::new(0),
        truncated: AtomicBool::new(false),
    };

    if let Some(key) = identity::dir_key(&root) {
        shared.mark_visited(key);
    }

    visit_dir(&shared, &root, 0);

    // Final snapshot so the UI never sits on a stale count.
    sink.tick(ProgressSnapshot {
        scan_id,
        files: shared.n_files.load(Ordering::Relaxed),
        dirs: shared.n_dirs.load(Ordering::Relaxed),
        bytes: shared.n_bytes.load(Ordering::Relaxed),
        errors: shared.n_errors.load(Ordering::Relaxed),
        current: String::new(),
    });

    Ok(ScanIndex {
        scan_id,
        root,
        parents: shared.parents.into_inner(),
        entries: shared.files.into_inner(),
        dirs: shared.dirs.into_inner(),
        errors: shared.errors.into_inner(),
        truncated: shared.truncated.load(Ordering::Relaxed),
        duration_ms: started.elapsed().as_millis() as u64,
        scanned_at: now_unix_secs(),
        cancelled: cancel.is_cancelled(),
    })
}

fn visit_dir(sh: &Shared, dir: &Path, depth: u16) {
    if sh.should_stop() {
        return;
    }

    let read_dir = match fs::read_dir(identity::for_io(dir)) {
        Ok(rd) => rd,
        Err(err) => {
            sh.push_error(dir, &err);
            return;
        }
    };

    let dir_path_id = sh.intern(dir.as_os_str());

    let mut batch: Vec<FileEntry> = Vec::new();
    let mut subdirs: Vec<PathBuf> = Vec::new();
    let mut direct_files = 0u32;
    let mut direct_dirs = 0u32;
    let mut direct_bytes = 0u64;
    let mut noise_files = 0u32;
    let mut truncated_here = false;

    for item in read_dir {
        if sh.should_stop() {
            return;
        }

        let dir_entry = match item {
            Ok(e) => e,
            Err(err) => {
                sh.push_error(dir, &err);
                continue;
            }
        };

        let name_os = dir_entry.file_name();
        let folded = text::fold(&text::display(&name_os));
        let path = dir_entry.path();

        // `symlink_metadata` is an lstat: it describes the link itself, so a
        // symlink can never be mistaken for its target.
        let md = match fs::symlink_metadata(identity::for_io(&path)) {
            Ok(m) => m,
            Err(err) => {
                sh.push_error(&path, &err);
                continue;
            }
        };

        let file_type = md.file_type();
        let probe = identity::probe(&folded, &md);
        let mut flags = EntryFlags(probe.flags);
        if file_type.is_symlink() {
            flags.set(EntryFlags::SYMLINK);
        }
        if category::is_noise_name(&folded) {
            flags.set(EntryFlags::NOISE);
        }

        // Noise is counted so that "a folder with only a .DS_Store in it" reads
        // as empty, but it is never listed and never a cleanup finding.
        if flags.has(EntryFlags::NOISE) {
            direct_files += 1;
            noise_files += 1;
            continue;
        }

        if sh.is_ignored(&folded) {
            continue;
        }

        if flags.has(EntryFlags::HIDDEN) && !sh.opts.include_hidden {
            // Still counted, so a folder holding only hidden items is never
            // reported as empty - we just do not list what is inside it.
            if file_type.is_dir() {
                direct_dirs += 1;
            } else {
                direct_files += 1;
            }
            continue;
        }

        // Links come first: a Windows junction reports `is_dir() == true` and
        // `is_symlink() == false`, so checking directory-ness first would walk
        // straight into it.
        if flags.has(EntryFlags::SYMLINK) || flags.has(EntryFlags::REPARSE) {
            direct_files += 1;
            batch.push(FileEntry {
                id: 0,
                parent: dir_path_id,
                name: name_os.into_boxed_os_str(),
                name_folded: folded.clone().into_boxed_str(),
                ext: None,
                size: 0,
                created: md.created().ok().and_then(to_unix_secs),
                modified: md.modified().ok().and_then(to_unix_secs),
                accessed: md.accessed().ok().and_then(to_unix_secs),
                category: Category::Misc,
                flags,
                ident: probe.ident,
                head_hash: None,
                full_hash: None,
            });
            continue;
        }

        if file_type.is_dir() {
            // A macOS application bundle is one thing to a user.
            if cfg!(target_os = "macos") && folded.ends_with(".app") {
                flags.set(EntryFlags::APP_BUNDLE);
                let size = bundle_size(&path, 0);
                direct_files += 1;
                direct_bytes += size;
                batch.push(FileEntry {
                    id: 0,
                    parent: dir_path_id,
                    name: name_os.into_boxed_os_str(),
                    name_folded: folded.clone().into_boxed_str(),
                    ext: Some("app".to_string().into_boxed_str()),
                    size,
                    created: md.created().ok().and_then(to_unix_secs),
                    modified: md.modified().ok().and_then(to_unix_secs),
                    accessed: md.accessed().ok().and_then(to_unix_secs),
                    category: Category::Installers,
                    flags,
                    ident: probe.ident,
                    head_hash: None,
                    full_hash: None,
                });
                continue;
            }

            direct_dirs += 1;

            if depth as usize + 1 > sh.opts.max_depth {
                sh.truncated.store(true, Ordering::Relaxed);
                truncated_here = true;
                continue;
            }

            // Loop and double-visit protection. Without this, a junction or a
            // macOS firmlink makes an entire tree appear twice and every file
            // in it looks like a duplicate of itself.
            //
            // A directory whose canonical path cannot be resolved is skipped:
            // risking an unbounded cycle is worse than missing a folder, and
            // the failure surfaces anyway when `read_dir` reports it.
            let Some(key) = identity::dir_key(&path) else {
                continue;
            };
            // System and application folders are never descended into, even
            // when the root above them was approved. This is what makes
            // scanning a whole drive safe: C:\Windows, Program Files and the
            // user's AppData are skipped by the same denylist that refuses
            // them as a scan root.
            if crate::core::safety::is_system_path(&key) {
                continue;
            }
            if sh.is_excluded(&key) {
                // Counted above as a subfolder, so the parent does not look
                // empty, but nothing inside it is read. This is the one place
                // the exclusion has to hold: the guard would refuse an
                // operation later anyway, but a file that is never indexed can
                // never be offered for one.
                continue;
            }
            if !sh.mark_visited(key) {
                continue;
            }

            subdirs.push(path);
            continue;
        }

        if !file_type.is_file() {
            // FIFO, socket, device node. Not a user file; not our business.
            continue;
        }

        let size = md.len();
        let ext = text::extension_of(&name_os);
        let mut category = category::categorize(ext.as_deref());

        if ext.as_deref().is_some_and(category::is_in_progress_ext) {
            flags.set(EntryFlags::IN_PROGRESS);
        }
        let modified = md.modified().ok().and_then(to_unix_secs);
        if let Some(m) = modified {
            if sh.now.saturating_sub(m) < sh.opts.in_progress_grace_secs {
                // Still being written. Excluded from every suggestion.
                flags.set(EntryFlags::IN_PROGRESS);
            }
        }

        // `is_safe_to_read`, not `is_safe_to_touch`: a part-downloaded file's
        // header is already written, so classifying it is fine. A cloud
        // placeholder is still excluded - reading one hydrates it.
        if ext.is_none() && sh.opts.sniff_extensionless && size > 0 && flags.is_safe_to_read() {
            if let Some(sniffed) = sniff_category(&path) {
                category = sniffed;
            }
        }

        direct_files += 1;
        direct_bytes += size;
        batch.push(FileEntry {
            id: 0,
            parent: dir_path_id,
            name: name_os.into_boxed_os_str(),
            name_folded: folded.into_boxed_str(),
            ext,
            size,
            created: md.created().ok().and_then(to_unix_secs),
            modified,
            accessed: md.accessed().ok().and_then(to_unix_secs),
            category,
            flags,
            ident: probe.ident,
            head_hash: None,
            full_hash: None,
        });
    }

    // Commit this directory's findings in one lock acquisition. Contention is
    // per-directory, not per-file, which is what keeps the walk scaling.
    let pushed = batch.len() as u64;
    if pushed > 0 {
        let mut files = sh.files.lock();
        let base = files.len() as u32;
        for (offset, mut entry) in batch.into_iter().enumerate() {
            entry.id = base + offset as u32;
            files.push(entry);
        }
    }

    sh.dirs.lock().push(DirRecord {
        path: dir_path_id,
        depth,
        direct_files,
        direct_dirs,
        direct_bytes,
        noise_files,
        truncated: truncated_here,
    });

    sh.n_files.fetch_add(pushed, Ordering::Relaxed);
    sh.n_dirs.fetch_add(1, Ordering::Relaxed);
    sh.n_bytes.fetch_add(direct_bytes, Ordering::Relaxed);
    sh.maybe_tick(dir);

    if subdirs.is_empty() || sh.should_stop() {
        return;
    }

    // Recursion inside a rayon parallel iterator is fine: work-stealing keeps
    // every core busy without us managing a queue.
    subdirs
        .into_par_iter()
        .for_each(|child| visit_dir(sh, &child, depth + 1));
}

/// Sum a macOS app bundle so its entry carries a real size, without adding its
/// thousands of internal files to the index. Sequential and depth-capped: this
/// is a rare case and not worth parallelising.
fn bundle_size(dir: &Path, depth: usize) -> u64 {
    if depth > BUNDLE_SUM_MAX_DEPTH {
        return 0;
    }
    let Ok(read_dir) = fs::read_dir(identity::for_io(dir)) else {
        return 0;
    };
    let mut total = 0u64;
    for item in read_dir.flatten() {
        let Ok(md) = fs::symlink_metadata(item.path()) else {
            continue;
        };
        let ft = md.file_type();
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            total += bundle_size(&item.path(), depth + 1);
        } else if ft.is_file() {
            total += md.len();
        }
    }
    total
}

/// The walk's only content read: 32 bytes, for files with no extension.
fn sniff_category(path: &Path) -> Option<Category> {
    let mut file = fs::File::open(identity::for_io(path)).ok()?;
    let mut head = [0u8; category::MAGIC_PROBE_BYTES];
    let read = file.read(&mut head).ok()?;
    if read == 0 {
        return None;
    }
    category::categorize_from_magic(&head[..read])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::progress::NoopSink;
    use std::fs::File;
    use std::io::Write as _;

    /// Minimal temp-dir helper. Avoids a `tempfile` dependency for four tests.
    struct TempTree(PathBuf);

    impl TempTree {
        fn new(tag: &str) -> Self {
            let mut p = std::env::temp_dir();
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            p.push(format!("cleanup-test-{tag}-{unique}"));
            fs::create_dir_all(&p).unwrap();
            TempTree(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
        fn file(&self, rel: &str, bytes: &[u8]) -> PathBuf {
            let full = self.0.join(rel);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            let mut f = File::create(&full).unwrap();
            f.write_all(bytes).unwrap();
            full
        }
        fn dir(&self, rel: &str) -> PathBuf {
            let full = self.0.join(rel);
            fs::create_dir_all(&full).unwrap();
            full
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn run(root: &Path, opts: WalkOptions) -> ScanIndex {
        scan(root, 1, &opts, &CancelToken::new(), &NoopSink).unwrap()
    }

    #[test]
    fn categorises_and_sums_a_simple_tree() {
        let t = TempTree::new("simple");
        t.file("photo.png", &[0u8; 100]);
        t.file("movie.mp4", &[0u8; 250]);
        t.file("notes.txt", b"hello");
        t.dir("empty");

        let index = run(t.path(), WalkOptions::default());

        assert_eq!(index.entries.len(), 3);
        assert_eq!(index.total_bytes(), 355);
        let cats: Vec<Category> = {
            let mut c: Vec<Category> = index.entries.iter().map(|e| e.category).collect();
            c.sort();
            c
        };
        assert!(cats.contains(&Category::Images));
        assert!(cats.contains(&Category::Videos));
        assert!(cats.contains(&Category::Documents));
        assert!(!index.truncated);
        assert!(!index.cancelled);
    }

    #[test]
    fn noise_files_make_a_folder_read_as_empty() {
        let t = TempTree::new("noise");
        let sub = t.dir("subfolder");
        File::create(sub.join(".DS_Store")).unwrap();

        let index = run(t.path(), WalkOptions::default());

        // The noise file itself is never listed.
        assert_eq!(index.entries.len(), 0);
        let sub_record = index
            .dirs
            .iter()
            .find(|d| index.dir_path(d).ends_with("subfolder"))
            .expect("subfolder must be recorded");
        assert!(sub_record.is_effectively_empty());
        assert!(!sub_record.is_strictly_empty());
    }

    #[test]
    fn an_excluded_folder_is_never_walked_into() {
        // The case this exists for: one project folder on a drive that must be
        // left completely alone, while the rest of the drive is scanned.
        let t = TempTree::new("excluded");
        t.file("report.pdf", &[0u8; 10]);
        let keep_out = t.dir("Unity");
        t.file("Unity/Library/artifacts.db", &[0u8; 999]);
        t.file("Unity/Assets/sprite.png", &[0u8; 50]);

        let index = run(
            t.path(),
            WalkOptions {
                excluded: vec![fs::canonicalize(&keep_out)
                    .map(identity::strip_verbatim)
                    .unwrap()],
                ..WalkOptions::default()
            },
        );

        let names: Vec<String> = index
            .entries
            .iter()
            .map(|e| text::display(&e.name))
            .collect();
        assert_eq!(names, vec!["report.pdf".to_string()]);
        assert_eq!(
            index.total_bytes(),
            10,
            "bytes inside the excluded folder must not be counted either"
        );
        assert!(
            !index
                .dirs
                .iter()
                .any(|d| index.dir_path(d).ends_with("Library")),
            "nothing inside the excluded folder may be recorded"
        );
    }

    #[test]
    fn excluding_a_folder_leaves_its_lookalike_sibling_alone() {
        let t = TempTree::new("excluded-sibling");
        let keep_out = t.dir("Unity");
        t.file("Unity/inside.txt", b"x");
        t.file("Unity-Backup/kept.txt", b"y");

        let index = run(
            t.path(),
            WalkOptions {
                excluded: vec![fs::canonicalize(&keep_out)
                    .map(identity::strip_verbatim)
                    .unwrap()],
                ..WalkOptions::default()
            },
        );

        let names: Vec<String> = index
            .entries
            .iter()
            .map(|e| text::display(&e.name))
            .collect();
        assert_eq!(names, vec!["kept.txt".to_string()]);
    }

    #[test]
    fn an_excluded_folder_cannot_be_used_as_a_scan_root() {
        let t = TempTree::new("excluded-root");
        let keep_out = t.dir("Unity");
        t.file("Unity/inside.txt", b"x");
        let canonical = fs::canonicalize(&keep_out)
            .map(identity::strip_verbatim)
            .unwrap();

        let outcome = scan(
            &keep_out,
            1,
            &WalkOptions {
                excluded: vec![canonical],
                ..WalkOptions::default()
            },
            &CancelToken::new(),
            &NoopSink,
        );
        match outcome {
            Err(err) => assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput),
            Ok(_) => panic!("scanning an excluded folder must be refused"),
        }
    }

    #[test]
    fn depth_cap_truncates_instead_of_descending_forever() {
        let t = TempTree::new("depth");
        t.file("a/b/c/d/deep.txt", b"x");

        let shallow = run(
            t.path(),
            WalkOptions {
                max_depth: 2,
                ..WalkOptions::default()
            },
        );
        assert!(shallow.truncated);
        assert_eq!(shallow.entries.len(), 0);

        let deep = run(
            t.path(),
            WalkOptions {
                max_depth: 8,
                ..WalkOptions::default()
            },
        );
        assert!(!deep.truncated);
        assert_eq!(deep.entries.len(), 1);
    }

    #[test]
    fn in_progress_downloads_are_flagged() {
        let t = TempTree::new("partial");
        t.file("movie.mp4.crdownload", &[0u8; 10]);

        let index = run(t.path(), WalkOptions::default());
        let entry = &index.entries[0];
        assert!(entry.flags.has(EntryFlags::IN_PROGRESS));
        assert_eq!(entry.category, Category::Temporary);

        // The distinction that matters: never move, trash or hash it, but
        // reading its header to classify it is fine.
        assert!(!entry.flags.is_safe_to_touch());
        assert!(entry.flags.is_safe_to_read());
    }

    #[test]
    fn a_file_finished_seconds_ago_is_not_mistaken_for_a_live_download() {
        let t = TempTree::new("grace");
        t.file("finished.pdf", b"%PDF-1.4");

        // With the grace window off, a complete file written moments ago is
        // just a file. This is the knob the test fixtures use, and it exists
        // because the heuristic is a backstop, not a fact.
        let index = run(
            t.path(),
            WalkOptions {
                in_progress_grace_secs: 0,
                ..WalkOptions::default()
            },
        );
        assert!(index.entries[0].flags.is_safe_to_touch());
    }

    #[test]
    fn extensionless_files_are_classified_by_their_header() {
        let t = TempTree::new("magic");
        t.file("mystery", b"%PDF-1.7 and then some more bytes to read");

        let index = run(t.path(), WalkOptions::default());
        assert_eq!(index.entries[0].category, Category::Documents);

        let without = run(
            t.path(),
            WalkOptions {
                sniff_extensionless: false,
                ..WalkOptions::default()
            },
        );
        assert_eq!(without.entries[0].category, Category::Misc);
    }

    #[test]
    fn cancellation_stops_the_walk() {
        let t = TempTree::new("cancel");
        for i in 0..50 {
            t.file(&format!("f{i}.txt"), b"x");
        }
        let cancel = CancelToken::new();
        cancel.cancel();
        let index = scan(t.path(), 7, &WalkOptions::default(), &cancel, &NoopSink).unwrap();
        assert!(index.cancelled);
        assert_eq!(index.entries.len(), 0);
    }

    #[test]
    fn a_missing_root_is_an_error_not_an_empty_scan() {
        let missing = std::env::temp_dir().join("cleanup-does-not-exist-xyz-123");
        let result = scan(
            &missing,
            1,
            &WalkOptions::default(),
            &CancelToken::new(),
            &NoopSink,
        );
        assert!(result.is_err());
    }

    #[test]
    fn paths_rebuild_to_something_that_actually_exists() {
        let t = TempTree::new("paths");
        t.file("nested/report.pdf", b"%PDF-1.4");

        let index = run(t.path(), WalkOptions::default());
        let entry = &index.entries[0];
        let rebuilt = index.path_of(entry);
        assert!(
            rebuilt.exists(),
            "path_of must reconstruct a usable path, got {rebuilt:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_recorded_but_never_followed() {
        let t = TempTree::new("symlink");
        let target = t.dir("real");
        t.file("real/inside.txt", b"x");
        std::os::unix::fs::symlink(&target, t.path().join("link")).unwrap();

        let index = run(t.path(), WalkOptions::default());

        // inside.txt once (via `real`), plus the link itself as an entry.
        let names: Vec<String> = index
            .entries
            .iter()
            .map(|e| text::display(&e.name))
            .collect();
        assert_eq!(names.iter().filter(|n| *n == "inside.txt").count(), 1);
        let link = index
            .entries
            .iter()
            .find(|e| text::display(&e.name) == "link")
            .expect("the symlink must appear as an entry");
        assert!(link.flags.has(EntryFlags::SYMLINK));
        assert!(!link.flags.is_safe_to_touch());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_loop_does_not_hang_the_walk() {
        let t = TempTree::new("loop");
        let inner = t.dir("a/b");
        // b -> a, a classic self-referential cycle.
        std::os::unix::fs::symlink(t.path().join("a"), inner.join("back")).unwrap();
        t.file("a/b/file.txt", b"x");

        let index = run(t.path(), WalkOptions::default());
        assert_eq!(index.entries.iter().filter(|e| e.size > 0).count(), 1);
    }
}
