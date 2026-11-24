//! Aggregation over an in-memory `ScanIndex`.
//!
//! Every number here comes from metadata already in memory - no extra disk
//! reads. A full summary over 100k entries is a handful of linear passes plus
//! three partial sorts.

use crate::core::dto::{
    row, CategoryStat, DuplicateCandidates, EmptyFolders, FlaggedCounts, FileRow, FolderRow,
    ScanSummary, StaleStats, StorageReport, TreemapItem, TreemapNode,
};
use crate::core::identity;
use crate::core::model::{Category, EntryFlags, FileEntry, ScanIndex};
use crate::core::text;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const TOP_N: usize = 12;
const EMPTY_FOLDER_SAMPLE: usize = 8;
const SECONDS_PER_DAY: i64 = 86_400;
/// Rows in each Storage list. Enough to be useful, few enough to stay readable.
const STORAGE_TOP_N: usize = 20;

pub fn summarize(index: &ScanIndex, stale_days: i64, now: i64) -> ScanSummary {
    let mut by_category: HashMap<Category, (u64, u64)> = HashMap::new();
    let mut flagged = FlaggedCounts::default();
    let mut total_bytes = 0u64;

    let access_reliable = identity::access_time_reliable();
    let stale_cutoff = now.saturating_sub(stale_days.max(1) * SECONDS_PER_DAY);
    let mut stale = StaleStats {
        days: stale_days,
        count: 0,
        bytes: 0,
        from_access_time: access_reliable,
    };

    for entry in &index.entries {
        total_bytes += entry.size;
        let slot = by_category.entry(entry.category).or_insert((0, 0));
        slot.0 += 1;
        slot.1 += entry.size;

        if entry.flags.has(EntryFlags::IN_PROGRESS) {
            flagged.in_progress += 1;
        }
        if entry.flags.has(EntryFlags::CLOUD_PLACEHOLDER) {
            flagged.cloud_placeholders += 1;
        }
        if entry.flags.has(EntryFlags::SYMLINK) || entry.flags.has(EntryFlags::REPARSE) {
            flagged.symlinks += 1;
        }
        if entry.flags.has(EntryFlags::HIDDEN) {
            flagged.hidden += 1;
        }
        if entry.category == Category::Temporary {
            flagged.temporary += 1;
        }

        // On Windows last-access time is not maintained, so fall back to
        // modification time rather than reporting a fabricated figure.
        let effective = if access_reliable {
            entry.accessed.or(entry.modified)
        } else {
            entry.modified
        };
        if let Some(ts) = effective {
            if ts < stale_cutoff {
                stale.count += 1;
                stale.bytes += entry.size;
            }
        }
    }

    let mut category_stats: Vec<CategoryStat> = Category::ALL
        .into_iter()
        .map(|category| {
            let (count, bytes) = by_category.get(&category).copied().unwrap_or((0, 0));
            CategoryStat {
                category,
                key: category.key(),
                count,
                bytes,
            }
        })
        .collect();
    category_stats.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(b.count.cmp(&a.count)));

    ScanSummary {
        scan_id: index.scan_id,
        root: text::display(index.root.as_os_str()),
        root_display: identity::shorten_for_display(index.root.as_os_str()),
        scanned_at: index.scanned_at,
        duration_ms: index.duration_ms,
        truncated: index.truncated,
        cancelled: index.cancelled,
        total_files: index.entries.len() as u64,
        total_dirs: index.dirs.len() as u64,
        total_bytes,
        by_category: category_stats,
        largest_files: top_by(index, TOP_N, |a, b| b.size.cmp(&a.size)),
        oldest_files: top_by(index, TOP_N, |a, b| {
            // Unknown timestamps sort last rather than pretending to be 1970.
            sort_option_asc(a.modified, b.modified)
        }),
        recent_files: top_by(index, TOP_N, |a, b| {
            sort_option_desc(a.modified, b.modified)
        }),
        empty_folders: empty_folders(index),
        duplicate_candidates: duplicate_candidates(index),
        stale,
        flagged,
        error_count: index.errors.len() as u64,
        access_time_reliable: access_reliable,
    }
}

fn sort_option_asc(a: Option<i64>, b: Option<i64>) -> std::cmp::Ordering {
    match (a, b) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

fn sort_option_desc(a: Option<i64>, b: Option<i64>) -> std::cmp::Ordering {
    sort_option_asc(b, a)
}

/// Top `n` entries by an ordering.
///
/// Filters only on `is_real_file` - symlinks and reparse points, which are not
/// files and whose size is meaningless here. Cloud placeholders and in-flight
/// downloads are deliberately *kept*: a 4 GB download in progress genuinely is
/// one of the largest things in the folder, and omitting it would misrepresent
/// where the space went. The UI dims those rows and names the reason instead.
fn top_by<F>(index: &ScanIndex, n: usize, compare: F) -> Vec<FileRow>
where
    F: Fn(&crate::core::model::FileEntry, &crate::core::model::FileEntry) -> std::cmp::Ordering,
{
    let mut ids: Vec<u32> = index
        .entries
        .iter()
        .filter(|e| e.flags.is_real_file())
        .map(|e| e.id)
        .collect();

    if ids.len() > n {
        ids.select_nth_unstable_by(n - 1, |&a, &b| {
            compare(&index.entries[a as usize], &index.entries[b as usize])
        });
        ids.truncate(n);
    }
    ids.sort_by(|&a, &b| compare(&index.entries[a as usize], &index.entries[b as usize]));

    ids.iter()
        .filter_map(|&id| index.entry(id))
        .map(|e| row(index, e))
        .collect()
}

/// Recursive size per folder, computed in one pass over the index.
///
/// No extra disk reads: every entry already knows its parent, so the work is
/// accumulating each directory's direct totals into all of its ancestors —
/// O(unique directories x depth), which for a Downloads tree is a few thousand
/// operations.
///
/// The scan root is excluded. It accounts for 100% of the bytes by definition,
/// so listing it tells the user nothing.
pub fn folder_rollup(index: &ScanIndex, top_n: usize) -> Vec<FolderRow> {
    // Direct contents per parent directory.
    let mut direct: HashMap<u32, (u64, u64)> = HashMap::new();
    for entry in &index.entries {
        let slot = direct.entry(entry.parent).or_insert((0, 0));
        slot.0 += entry.size;
        slot.1 += 1;
    }

    // Push each directory's direct totals up through its ancestors.
    let mut totals: HashMap<PathBuf, (u64, u64)> = HashMap::new();
    let root: &Path = index.root.as_path();

    for (parent_id, (bytes, files)) in direct {
        let Some(parent) = index.parents.get(parent_id) else {
            continue;
        };
        let parent_path = PathBuf::from(parent.as_ref());
        if !parent_path.starts_with(root) {
            continue;
        }

        let mut current: Option<&Path> = Some(parent_path.as_path());
        while let Some(dir) = current {
            let slot = totals.entry(dir.to_path_buf()).or_insert((0, 0));
            slot.0 += bytes;
            slot.1 += files;
            if dir == root {
                break;
            }
            current = dir.parent().filter(|p| p.starts_with(root));
        }
    }

    let mut rows: Vec<FolderRow> = totals
        .into_iter()
        .filter(|(path, _)| path.as_path() != root)
        .map(|(path, (bytes, files))| {
            let depth = path
                .strip_prefix(root)
                .map(|rest| rest.components().count() as u16)
                .unwrap_or(0);
            FolderRow {
                path: text::display(path.as_os_str()),
                display: identity::shorten_for_display(path.as_os_str()),
                bytes,
                files,
                depth,
            }
        })
        .collect();

    rows.sort_by(|a, b| {
        b.bytes
            .cmp(&a.bytes)
            .then_with(|| b.files.cmp(&a.files))
            .then_with(|| a.path.cmp(&b.path))
    });
    rows.truncate(top_n);
    rows
}

/// The Storage screen's data. Entirely derived from the in-memory index.
pub fn storage_report(index: &ScanIndex, stale_days: i64, now: i64) -> StorageReport {
    let summary = summarize(index, stale_days, now);
    let access_reliable = identity::access_time_reliable();
    let stale_cutoff = now.saturating_sub(stale_days.max(1) * SECONDS_PER_DAY);

    let is_stale = |entry: &FileEntry| -> bool {
        let effective = if access_reliable {
            entry.accessed.or(entry.modified)
        } else {
            entry.modified
        };
        effective.is_some_and(|ts| ts < stale_cutoff)
    };

    StorageReport {
        total_bytes: summary.total_bytes,
        total_files: summary.total_files,
        largest_folders: folder_rollup(index, STORAGE_TOP_N),
        largest_files: top_matching(index, STORAGE_TOP_N, |_| true),
        stale_files: top_matching(index, STORAGE_TOP_N, is_stale),
        temporary_files: top_matching(index, STORAGE_TOP_N, |e| {
            e.category == Category::Temporary
        }),
        by_category: summary.by_category,
        empty_folders: summary.empty_folders,
        stale: summary.stale,
        truncated: index.truncated,
    }
}

/// Largest `n` real files matching a predicate, biggest first.
fn top_matching<P>(index: &ScanIndex, n: usize, predicate: P) -> Vec<FileRow>
where
    P: Fn(&FileEntry) -> bool,
{
    let mut ids: Vec<u32> = index
        .entries
        .iter()
        .filter(|e| e.flags.is_real_file() && predicate(e))
        .map(|e| e.id)
        .collect();

    ids.sort_by(|&a, &b| {
        let (Some(x), Some(y)) = (index.entry(a), index.entry(b)) else {
            return std::cmp::Ordering::Equal;
        };
        y.size
            .cmp(&x.size)
            .then_with(|| x.name.cmp(&y.name))
    });
    ids.truncate(n);

    ids.iter()
        .filter_map(|&id| index.entry(id))
        .map(|e| row(index, e))
        .collect()
}

fn empty_folders(index: &ScanIndex) -> EmptyFolders {
    let mut count = 0u64;
    let mut sample = Vec::new();
    for dir in &index.dirs {
        if !dir.is_effectively_empty() {
            continue;
        }
        // The scan root itself being empty is information, not a finding.
        let path = index.dir_path(dir);
        if path == index.root {
            continue;
        }
        count += 1;
        if sample.len() < EMPTY_FOLDER_SAMPLE {
            sample.push(identity::shorten_for_display(path.as_os_str()));
        }
    }
    EmptyFolders { count, sample }
}

/// Size-grouping pass. Zero disk reads.
///
/// Excludes, in this order:
/// * zero-byte files - every one is byte-identical to every other, which is
///   true and useless;
/// * anything not safe to touch;
/// * entries sharing a filesystem identity, which are one file reached twice
///   (hardlink, junction, firmlink) rather than two copies.
pub fn duplicate_candidates(index: &ScanIndex) -> DuplicateCandidates {
    let mut by_size: HashMap<u64, Vec<u32>> = HashMap::new();
    for entry in &index.entries {
        if entry.size == 0 || !entry.flags.is_safe_to_touch() {
            continue;
        }
        by_size.entry(entry.size).or_default().push(entry.id);
    }

    let mut result = DuplicateCandidates::default();
    let mut seen_idents: HashSet<(u64, u64)> = HashSet::new();

    for (size, ids) in by_size {
        if ids.len() < 2 {
            continue;
        }

        seen_idents.clear();
        let mut distinct = 0u64;
        for id in ids {
            let Some(entry) = index.entry(id) else {
                continue;
            };
            match entry.ident {
                Some(ident) => {
                    if seen_idents.insert((ident.volume, ident.file_id)) {
                        distinct += 1;
                    }
                }
                // No identity available: count it, because we cannot prove it
                // is the same file. Over-counting a candidate is harmless;
                // under-counting would hide a real duplicate.
                None => distinct += 1,
            }
        }

        if distinct < 2 {
            continue;
        }
        result.groups += 1;
        result.files += distinct;
        result.reclaimable_bytes_upper_bound += size * (distinct - 1);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::progress::{CancelToken, NoopSink};
    use crate::core::walk::{scan, WalkOptions};
    use std::fs;
    use std::path::PathBuf;

    struct TempTree(PathBuf);

    impl TempTree {
        fn new(tag: &str) -> Self {
            let mut p = std::env::temp_dir();
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            p.push(format!("cleanup-analyze-{tag}-{unique}"));
            fs::create_dir_all(&p).unwrap();
            TempTree(p)
        }
        fn file(&self, rel: &str, bytes: &[u8]) {
            let full = self.0.join(rel);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(full, bytes).unwrap();
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn index_of(t: &TempTree) -> ScanIndex {
        // A fixture file is written microseconds before the scan, which the
        // "modified very recently" heuristic would rightly read as an
        // in-flight download. These files are complete, so the grace window is
        // switched off rather than worked around.
        let options = WalkOptions {
            in_progress_grace_secs: 0,
            ..WalkOptions::default()
        };
        scan(&t.0, 1, &options, &CancelToken::new(), &NoopSink).unwrap()
    }

    #[test]
    fn the_treemap_level_adds_up_to_the_folder_it_describes() {
        // The property that matters: a level must account for every byte in
        // the folder, or the map misleads about where the space went.
        let t = TempTree::new("treemap");
        t.file("loose.bin", &[0u8; 1000]);
        t.file("photos/a.png", &[0u8; 2000]);
        t.file("photos/b.png", &[0u8; 3000]);
        t.file("docs/deep/report.pdf", &[0u8; 4000]);
        let index = index_of(&t);

        let node = treemap(&index, None);
        assert_eq!(node.bytes, 10_000);
        assert_eq!(node.files, 4);
        assert_eq!(
            node.items.iter().map(|i| i.bytes).sum::<u64>(),
            node.bytes,
            "the rectangles must account for the whole folder"
        );

        let photos = node
            .items
            .iter()
            .find(|i| i.name == "photos")
            .expect("subfolder must be a rectangle");
        assert!(photos.is_dir);
        assert_eq!(photos.bytes, 5000, "a folder carries its recursive size");
        assert_eq!(photos.files, 2);

        let loose = node
            .items
            .iter()
            .find(|i| i.name == "loose.bin")
            .expect("a file in this folder must be its own rectangle");
        assert!(!loose.is_dir);
        assert!(loose.id.is_some(), "a file rectangle must be actionable");
    }

    #[test]
    fn drilling_into_a_folder_shows_that_folder() {
        let t = TempTree::new("treemap-drill");
        t.file("photos/a.png", &[0u8; 2000]);
        t.file("photos/raw/shot.cr2", &[0u8; 8000]);
        let index = index_of(&t);

        let photos = t.0.join("photos");
        let node = treemap(&index, Some(&photos));
        assert_eq!(node.bytes, 10_000);
        assert!(node.parent.is_some(), "there is a level above this one");
        assert_eq!(
            node.trail.len(),
            2,
            "the breadcrumb runs from the scan root down to here"
        );
        assert!(node.items.iter().any(|i| i.name == "raw" && i.is_dir));
        assert!(node.items.iter().any(|i| i.name == "a.png" && !i.is_dir));
    }

    #[test]
    fn a_path_outside_the_scan_falls_back_to_the_root() {
        // The path comes from the UI, so it is untrusted: it must not be a way
        // to read totals for somewhere that was never scanned.
        let t = TempTree::new("treemap-outside");
        t.file("a.png", &[0u8; 100]);
        let index = index_of(&t);

        let elsewhere = if cfg!(windows) {
            PathBuf::from(r"C:\Windows")
        } else {
            PathBuf::from("/usr")
        };
        let node = treemap(&index, Some(&elsewhere));
        assert_eq!(node.path, text::display(index.root.as_os_str()));
        assert!(node.parent.is_none(), "the root has no level above it");
    }

    #[test]
    fn summary_totals_match_the_tree() {
        let t = TempTree::new("totals");
        t.file("a.png", &[1u8; 500]);
        t.file("b.pdf", &[2u8; 300]);
        let index = index_of(&t);

        let s = summarize(&index, 180, crate::core::model::now_unix_secs());
        assert_eq!(s.total_files, 2);
        assert_eq!(s.total_bytes, 800);
        let images = s
            .by_category
            .iter()
            .find(|c| c.category == Category::Images)
            .unwrap();
        assert_eq!(images.count, 1);
        assert_eq!(images.bytes, 500);
    }

    #[test]
    fn identical_sizes_become_candidates_and_unique_sizes_do_not() {
        let t = TempTree::new("dupes");
        t.file("one.bin", &[7u8; 1024]);
        t.file("two.bin", &[9u8; 1024]); // same size, different content
        t.file("three.bin", &[0u8; 77]); // unique size
        let index = index_of(&t);

        let c = duplicate_candidates(&index);
        assert_eq!(c.groups, 1);
        assert_eq!(c.files, 2);
        // Upper bound only: these two happen to differ, which hashing in
        // Phase 3 will establish.
        assert_eq!(c.reclaimable_bytes_upper_bound, 1024);
    }

    #[test]
    fn zero_byte_files_are_not_duplicate_candidates() {
        let t = TempTree::new("zero");
        t.file("empty1.txt", b"");
        t.file("empty2.txt", b"");
        t.file("empty3.txt", b"");
        let index = index_of(&t);
        assert_eq!(duplicate_candidates(&index).groups, 0);
    }

    #[test]
    fn largest_files_are_ordered_and_bounded() {
        let t = TempTree::new("largest");
        for i in 1..=20u8 {
            t.file(&format!("f{i}.bin"), &vec![i; (i as usize) * 100]);
        }
        let index = index_of(&t);
        let s = summarize(&index, 180, crate::core::model::now_unix_secs());

        assert_eq!(s.largest_files.len(), TOP_N);
        for pair in s.largest_files.windows(2) {
            assert!(pair[0].size >= pair[1].size);
        }
        assert_eq!(s.largest_files[0].size, 2000);
    }

    #[test]
    fn folder_sizes_roll_up_from_the_inside_out() {
        let t = TempTree::new("rollup");
        t.file("media/video/clip.mp4", &[0u8; 5000]);
        t.file("media/image.png", &[0u8; 1000]);
        t.file("docs/notes.txt", &[0u8; 100]);
        let index = index_of(&t);

        let rows = folder_rollup(&index, 10);
        let find = |suffix: &str| {
            rows.iter()
                .find(|r| r.path.replace('\\', "/").ends_with(suffix))
                .unwrap_or_else(|| panic!("{suffix} missing from {rows:?}"))
        };

        // `media` must include its subtree, not just its direct children.
        assert_eq!(find("media").bytes, 6000);
        assert_eq!(find("media").files, 2);
        assert_eq!(find("media/video").bytes, 5000);
        assert_eq!(find("docs").bytes, 100);

        // Biggest first, and the root is never listed.
        assert!(rows[0].bytes >= rows[rows.len() - 1].bytes);
        let root_display = index.root.to_string_lossy().to_string();
        assert!(!rows.iter().any(|r| r.path == root_display));
    }

    #[test]
    fn the_storage_report_lists_the_biggest_things_first() {
        let t = TempTree::new("storage");
        t.file("huge.bin", &[0u8; 9000]);
        t.file("medium.bin", &[0u8; 500]);
        t.file("leftover.tmp", &[0u8; 300]);
        let index = index_of(&t);

        let report = storage_report(&index, 180, crate::core::model::now_unix_secs());
        assert_eq!(report.total_files, 3);
        assert_eq!(report.total_bytes, 9800);
        assert_eq!(report.largest_files[0].name, "huge.bin");
        assert_eq!(report.temporary_files.len(), 1);
        assert_eq!(report.temporary_files[0].name, "leftover.tmp");
    }

    #[test]
    fn empty_folders_are_counted_but_the_root_is_not() {
        let t = TempTree::new("empties");
        fs::create_dir_all(t.0.join("nothing-here")).unwrap();
        fs::create_dir_all(t.0.join("also-nothing")).unwrap();
        t.file("keep/real.txt", b"x");
        let index = index_of(&t);

        let s = summarize(&index, 180, crate::core::model::now_unix_secs());
        assert_eq!(s.empty_folders.count, 2);
        assert_eq!(s.empty_folders.sample.len(), 2);
    }
}

// ------------------------------------------------------------- treemap ---

/// Items drawn at one level. Beyond this, rectangles are smaller than a
/// pixel and the browser is laying out shapes nobody can see or click.
const TREEMAP_ITEMS: usize = 220;

/// Recursive byte totals for every directory under the scan root.
///
/// Shared by `folder_rollup` and the treemap: each entry's bytes are added to
/// its parent and then to every ancestor, which is O(entries x depth).
fn recursive_sizes(index: &ScanIndex) -> HashMap<PathBuf, (u64, u64)> {
    let mut direct: HashMap<u32, (u64, u64)> = HashMap::new();
    for entry in &index.entries {
        let slot = direct.entry(entry.parent).or_insert((0, 0));
        slot.0 += entry.size;
        slot.1 += 1;
    }

    let mut totals: HashMap<PathBuf, (u64, u64)> = HashMap::new();
    let root: &Path = index.root.as_path();

    for (parent_id, (bytes, files)) in direct {
        let Some(parent) = index.parents.get(parent_id) else {
            continue;
        };
        let parent_path = PathBuf::from(parent.as_ref());
        if !parent_path.starts_with(root) {
            continue;
        }
        let mut current: Option<&Path> = Some(parent_path.as_path());
        while let Some(dir) = current {
            let slot = totals.entry(dir.to_path_buf()).or_insert((0, 0));
            slot.0 += bytes;
            slot.1 += files;
            if dir == root {
                break;
            }
            current = dir.parent().filter(|p| p.starts_with(root));
        }
    }
    totals
}

/// One level of the storage map.
///
/// `dir` is the folder to open; `None` means the scan root. Everything is
/// derived from the in-memory index, so drilling in costs no disk I/O.
///
/// Items smaller than the drawable threshold are folded into a single
/// remainder rectangle rather than dropped: the level must still add up to
/// the folder's real size, or the map would quietly lie about where the space
/// went.
pub fn treemap(index: &ScanIndex, dir: Option<&Path>) -> TreemapNode {
    let root: &Path = index.root.as_path();
    let totals = recursive_sizes(index);

    // A path from the UI is untrusted input: it must be inside this scan, or
    // we show the root instead of whatever it asked for.
    let here: &Path = match dir {
        Some(path) if path.starts_with(root) && totals.contains_key(path) => path,
        _ => root,
    };

    let (here_bytes, here_files) = totals.get(here).copied().unwrap_or((0, 0));

    // Immediate subfolders, with their recursive totals.
    let mut items: Vec<TreemapItem> = totals
        .iter()
        .filter(|(path, _)| path.parent() == Some(here))
        .map(|(path, (bytes, files))| TreemapItem {
            name: path
                .file_name()
                .map(|n| text::display(n))
                .unwrap_or_else(|| text::display(path.as_os_str())),
            path: text::display(path.as_os_str()),
            bytes: *bytes,
            files: *files,
            is_dir: true,
            category: Category::Misc,
            id: None,
            is_remainder: false,
        })
        .collect();

    // Files sitting directly in this folder, as their own rectangles - which
    // is the point of a storage map: one huge file shows up as one huge box.
    for entry in &index.entries {
        let Some(parent) = index.parents.get(entry.parent) else {
            continue;
        };
        if Path::new(parent) != here {
            continue;
        }
        items.push(TreemapItem {
            name: text::display(&entry.name),
            path: text::display(index.path_of(entry).as_os_str()),
            bytes: entry.size,
            files: 1,
            is_dir: false,
            category: entry.category,
            id: Some(entry.id),
            is_remainder: false,
        });
    }

    items.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.name.cmp(&b.name)));

    let mut remainder_count = 0u64;
    if items.len() > TREEMAP_ITEMS {
        let tail: Vec<TreemapItem> = items.split_off(TREEMAP_ITEMS);
        remainder_count = tail.len() as u64;
        let bytes: u64 = tail.iter().map(|i| i.bytes).sum();
        let files: u64 = tail.iter().map(|i| i.files).sum();
        if bytes > 0 || files > 0 {
            items.push(TreemapItem {
                name: format!("{remainder_count} smaller items"),
                path: text::display(here.as_os_str()),
                bytes,
                files,
                is_dir: false,
                category: Category::Misc,
                id: None,
                is_remainder: true,
            });
        }
    }

    // Breadcrumb, root first.
    let mut trail: Vec<FolderRow> = Vec::new();
    let mut cursor: Option<&Path> = Some(here);
    while let Some(path) = cursor {
        let (bytes, files) = totals.get(path).copied().unwrap_or((0, 0));
        trail.push(FolderRow {
            path: text::display(path.as_os_str()),
            display: if path == root {
                identity::shorten_for_display(path.as_os_str())
            } else {
                path.file_name()
                    .map(|n| text::display(n))
                    .unwrap_or_else(|| text::display(path.as_os_str()))
            },
            bytes,
            files,
            depth: path
                .strip_prefix(root)
                .map(|rest| rest.components().count() as u16)
                .unwrap_or(0),
        });
        if path == root {
            break;
        }
        cursor = path.parent().filter(|p| p.starts_with(root));
    }
    trail.reverse();

    TreemapNode {
        path: text::display(here.as_os_str()),
        display: identity::shorten_for_display(here.as_os_str()),
        bytes: here_bytes,
        files: here_files,
        parent: if here == root {
            None
        } else {
            here.parent()
                .filter(|p| p.starts_with(root))
                .map(|p| text::display(p.as_os_str()))
        },
        trail,
        items,
        remainder_count,
    }
}
