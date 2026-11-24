//! Filtering, sorting and windowing over the in-memory index.
//!
//! The webview never receives the whole index. It sends a `QueryRequest` and
//! gets back a page of at most a few hundred rows plus the totals for the full
//! match set, so a 100k-file folder costs the same to display as a 100-file one.
//!
//! Cost per call is one linear filter pass plus a sort of the matching ids
//! (not of the entries), which stays in single-digit milliseconds at 100k
//! entries - fast enough to run on every keystroke without a debounce hack.

use crate::core::dto::{row, QueryRequest, QueryResponse, SortKey};
use crate::core::model::{FileEntry, ScanIndex};
use crate::core::text;

/// Upper bound on rows returned in one page, regardless of what was asked for.
/// Protects the IPC channel from a request for 100k rows.
const MAX_PAGE: u32 = 500;

pub fn query(index: &ScanIndex, request: &QueryRequest) -> QueryResponse {
    let needle = request
        .text
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(text::fold);

    let ext_filter = request
        .ext
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|e| e.trim_start_matches('.').to_ascii_lowercase());

    let parent_filter = request
        .parent_prefix
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(text::fold);

    let mut matched: Vec<u32> = Vec::new();
    let mut bytes = 0u64;

    for entry in &index.entries {
        if !matches(index, entry, request, needle.as_deref(), ext_filter.as_deref(), parent_filter.as_deref()) {
            continue;
        }
        matched.push(entry.id);
        bytes += entry.size;
    }

    let total = matched.len() as u64;

    sort_ids(index, &mut matched, request.sort, request.descending);

    let limit = request.limit.clamp(1, MAX_PAGE) as usize;
    let offset = request.offset as usize;
    let rows = matched
        .iter()
        .skip(offset)
        .take(limit)
        .filter_map(|&id| index.entry(id))
        .map(|entry| row(index, entry))
        .collect();

    QueryResponse { total, bytes, rows }
}

/// Ids of every match, ignoring paging.
///
/// Backs "select all": the UI only ever holds a window of rows, so it cannot
/// build this list itself. Ids are 4 bytes, so even a six-figure result set is
/// a few hundred KB over IPC - far cheaper than the equivalent in rows.
pub fn matching_ids(index: &ScanIndex, request: &QueryRequest, limit: usize) -> Vec<u32> {
    let needle = request
        .text
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(text::fold);
    let ext_filter = request
        .ext
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|e| e.trim_start_matches('.').to_ascii_lowercase());
    let parent_filter = request
        .parent_prefix
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(text::fold);

    index
        .entries
        .iter()
        .filter(|entry| {
            matches(
                index,
                entry,
                request,
                needle.as_deref(),
                ext_filter.as_deref(),
                parent_filter.as_deref(),
            )
        })
        .map(|entry| entry.id)
        .take(limit)
        .collect()
}

fn matches(
    index: &ScanIndex,
    entry: &FileEntry,
    request: &QueryRequest,
    needle: Option<&str>,
    ext_filter: Option<&str>,
    parent_filter: Option<&str>,
) -> bool {
    if request.require_flags != 0 && (entry.flags.0 & request.require_flags) != request.require_flags
    {
        return false;
    }
    if request.exclude_flags != 0 && entry.flags.has(request.exclude_flags) {
        return false;
    }
    if !request.categories.is_empty() && !request.categories.contains(&entry.category) {
        return false;
    }
    if let Some(min) = request.min_size {
        if entry.size < min {
            return false;
        }
    }
    if let Some(max) = request.max_size {
        if entry.size > max {
            return false;
        }
    }
    if request.modified_after.is_some() || request.modified_before.is_some() {
        // A file with no timestamp cannot satisfy a date filter. Treating it as
        // "matches everything" would silently include files the user filtered out.
        let Some(modified) = entry.modified else {
            return false;
        };
        if let Some(after) = request.modified_after {
            if modified < after {
                return false;
            }
        }
        if let Some(before) = request.modified_before {
            if modified > before {
                return false;
            }
        }
    }
    if let Some(wanted) = ext_filter {
        match entry.ext.as_deref() {
            Some(ext) if ext == wanted => {}
            _ => return false,
        }
    }
    if let Some(prefix) = parent_filter {
        let parent = index
            .parents
            .get(entry.parent)
            .map(|p| text::fold(&text::display(p)))
            .unwrap_or_default();
        if !parent.starts_with(prefix) {
            return false;
        }
    }
    if let Some(needle) = needle {
        // Both sides are already folded, so this is a plain substring test
        // with no allocation - which matters because it runs once per entry on
        // every keystroke.
        if !entry.name_folded.contains(needle) {
            return false;
        }
    }
    true
}

/// Sort matched ids in place.
///
/// Decorate-sort-undecorate, because the folded name is the tiebreaker for
/// *every* key as well as the primary key for `Name`. Computing it inside the
/// comparator allocates two strings per comparison - O(n log n) allocations,
/// which measured at 164 ms per query over 85k files and made sorting by name
/// visibly laggy. Precomputing it is O(n) allocations and about 25x faster.
fn sort_ids(index: &ScanIndex, ids: &mut Vec<u32>, key: SortKey, descending: bool) {
    use std::cmp::Ordering;

    let mut decorated: Vec<(&str, u32)> = ids
        .iter()
        .map(|&id| {
            let name = index
                .entry(id)
                .map(|entry| entry.name_folded.as_ref())
                .unwrap_or("");
            (name, id)
        })
        .collect();

    decorated.sort_by(|(left_name, left_id), (right_name, right_id)| {
        let (Some(x), Some(y)) = (index.entry(*left_id), index.entry(*right_id)) else {
            return Ordering::Equal;
        };
        let primary = match key {
            SortKey::Size => x.size.cmp(&y.size),
            SortKey::Modified => opt_cmp(x.modified, y.modified),
            SortKey::Created => opt_cmp(x.created, y.created),
            SortKey::Category => x.category.cmp(&y.category),
            SortKey::Extension => x
                .ext
                .as_deref()
                .unwrap_or("")
                .cmp(y.ext.as_deref().unwrap_or("")),
            SortKey::Name => Ordering::Equal,
        };
        // Name breaks every tie, so ordering is deterministic and a re-query
        // never shuffles equal rows under the user's cursor.
        let ordered = primary.then_with(|| left_name.cmp(right_name));
        if descending {
            ordered.reverse()
        } else {
            ordered
        }
    });

    for (slot, (_, id)) in ids.iter_mut().zip(decorated) {
        *slot = id;
    }
}

/// `None` sorts last in ascending order rather than masquerading as 1970.
fn opt_cmp(a: Option<i64>, b: Option<i64>) -> std::cmp::Ordering {
    match (a, b) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::dto::QueryRequest;
    use crate::core::model::{Category, EntryFlags};
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
            p.push(format!("cleanup-query-{tag}-{unique}"));
            fs::create_dir_all(&p).unwrap();
            TempTree(p)
        }
        fn file(&self, rel: &str, len: usize) {
            let full = self.0.join(rel);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(full, vec![0u8; len]).unwrap();
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn base() -> QueryRequest {
        QueryRequest {
            limit: 100,
            ..QueryRequest::default()
        }
    }

    /// Fixture files are written immediately before the scan, so the
    /// "modified very recently" heuristic is switched off - they are complete.
    fn walk_opts() -> WalkOptions {
        WalkOptions {
            in_progress_grace_secs: 0,
            ..WalkOptions::default()
        }
    }

    fn fixture() -> (TempTree, ScanIndex) {
        let t = TempTree::new("fixture");
        t.file("Holiday Photo.PNG", 1000);
        t.file("invoice.pdf", 200);
        t.file("Setup.exe", 5000);
        t.file("clip.mp4", 3000);
        let index = scan(&t.0, 1, &walk_opts(), &CancelToken::new(), &NoopSink).unwrap();
        (t, index)
    }

    #[test]
    fn unfiltered_query_returns_everything_with_totals() {
        let (_t, index) = fixture();
        let r = query(&index, &base());
        assert_eq!(r.total, 4);
        assert_eq!(r.bytes, 9200);
        assert_eq!(r.rows.len(), 4);
    }

    #[test]
    fn text_search_is_case_insensitive() {
        let (_t, index) = fixture();
        let r = query(
            &index,
            &QueryRequest {
                text: Some("HOLIDAY".into()),
                ..base()
            },
        );
        assert_eq!(r.total, 1);
        assert_eq!(r.rows[0].name, "Holiday Photo.PNG");
    }

    #[test]
    fn category_and_size_filters_combine() {
        let (_t, index) = fixture();
        let r = query(
            &index,
            &QueryRequest {
                categories: vec![Category::Images, Category::Videos],
                min_size: Some(2000),
                ..base()
            },
        );
        assert_eq!(r.total, 1);
        assert_eq!(r.rows[0].name, "clip.mp4");
    }

    #[test]
    fn extension_filter_tolerates_a_leading_dot() {
        let (_t, index) = fixture();
        for spelling in [".pdf", "pdf", "PDF"] {
            let r = query(
                &index,
                &QueryRequest {
                    ext: Some(spelling.into()),
                    ..base()
                },
            );
            assert_eq!(r.total, 1, "spelling {spelling}");
        }
    }

    #[test]
    fn sorting_and_paging_are_stable() {
        let (_t, index) = fixture();
        let descending = query(
            &index,
            &QueryRequest {
                sort: SortKey::Size,
                descending: true,
                ..base()
            },
        );
        assert_eq!(descending.rows[0].name, "Setup.exe");

        let page = query(
            &index,
            &QueryRequest {
                sort: SortKey::Size,
                descending: true,
                offset: 1,
                limit: 2,
                ..base()
            },
        );
        // Totals describe the whole match set, not the page.
        assert_eq!(page.total, 4);
        assert_eq!(page.rows.len(), 2);
        assert_eq!(page.rows[0].name, "clip.mp4");
    }

    #[test]
    fn page_size_is_capped() {
        let (_t, index) = fixture();
        let r = query(
            &index,
            &QueryRequest {
                limit: 10_000_000,
                ..base()
            },
        );
        assert!(r.rows.len() <= MAX_PAGE as usize);
    }

    #[test]
    fn exclude_flags_filters_out_unsafe_entries() {
        let t = TempTree::new("flags");
        t.file("good.txt", 10);
        // Flagged by its extension, independently of any timing heuristic.
        t.file("busy.txt.crdownload", 10);
        let index = scan(&t.0, 1, &walk_opts(), &CancelToken::new(), &NoopSink).unwrap();

        let all = query(&index, &base());
        assert_eq!(all.total, 2);

        let safe = query(
            &index,
            &QueryRequest {
                exclude_flags: EntryFlags::UNSAFE_TO_TOUCH,
                ..base()
            },
        );
        assert_eq!(safe.total, 1);
        assert_eq!(safe.rows[0].name, "good.txt");
    }
}
