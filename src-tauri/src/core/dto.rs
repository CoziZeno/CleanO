//! Serialisable shapes crossing the IPC boundary.
//!
//! Kept separate from `model.rs` on purpose. The model is tuned for memory and
//! for correct I/O (`Box<OsStr>` names, interned parents, 32-byte hashes); the
//! DTOs are tuned for the UI (display strings, NFC-normalised, numbers JSON can
//! represent). Mixing the two is how a display string ends up being used for a
//! filesystem call.
//!
//! The TypeScript mirror of this file is `src/ipc/types.ts`.

use crate::core::identity;
use crate::core::model::{Category, FileEntry, ScanIndex};
use crate::core::text;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRow {
    pub id: u32,
    pub name: String,
    /// Full path, display-normalised. Never send this back for an operation -
    /// operations take the `id` and rebuild the real path in Rust.
    pub path: String,
    /// Parent directory, with the home prefix collapsed to `~`.
    pub parent: String,
    pub ext: Option<String>,
    pub size: u64,
    pub created: Option<i64>,
    pub modified: Option<i64>,
    pub accessed: Option<i64>,
    pub category: Category,
    pub flags: u16,
}

pub fn row(index: &ScanIndex, entry: &FileEntry) -> FileRow {
    let full = index.path_of(entry);
    let parent = index
        .parents
        .get(entry.parent)
        .map(|p| identity::shorten_for_display(p))
        .unwrap_or_default();
    FileRow {
        id: entry.id,
        name: text::display(&entry.name),
        path: text::display(full.as_os_str()),
        parent,
        ext: entry.ext.as_deref().map(str::to_owned),
        size: entry.size,
        created: entry.created,
        modified: entry.modified,
        accessed: entry.accessed,
        category: entry.category,
        flags: entry.flags.0,
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryStat {
    pub category: Category,
    pub key: &'static str,
    pub count: u64,
    pub bytes: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlaggedCounts {
    pub in_progress: u64,
    pub cloud_placeholders: u64,
    pub symlinks: u64,
    pub temporary: u64,
    pub hidden: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmptyFolders {
    pub count: u64,
    /// A handful of example paths, for the UI to show without another call.
    pub sample: Vec<String>,
}

/// Duplicate *candidates*, derived from size grouping alone.
///
/// This costs zero disk reads and is honest about what it is: files that share
/// an exact size and could therefore be duplicates. Confirmation needs hashing,
/// which is Phase 3. `reclaimable_bytes` is an upper bound, and the UI labels
/// it as one - presenting it as a guaranteed saving would be a lie.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateCandidates {
    pub groups: u64,
    pub files: u64,
    pub reclaimable_bytes_upper_bound: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StaleStats {
    pub days: i64,
    pub count: u64,
    pub bytes: u64,
    /// False on Windows, where last-access times are not maintained, so this
    /// was computed from modification time instead.
    pub from_access_time: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSummary {
    pub scan_id: u64,
    pub root: String,
    pub root_display: String,
    pub scanned_at: i64,
    pub duration_ms: u64,
    /// Totals are a lower bound: a depth cap or the entry ceiling was hit.
    pub truncated: bool,
    pub cancelled: bool,

    pub total_files: u64,
    pub total_dirs: u64,
    pub total_bytes: u64,

    pub by_category: Vec<CategoryStat>,
    pub largest_files: Vec<FileRow>,
    pub oldest_files: Vec<FileRow>,
    pub recent_files: Vec<FileRow>,

    pub empty_folders: EmptyFolders,
    pub duplicate_candidates: DuplicateCandidates,
    pub stale: StaleStats,
    pub flagged: FlaggedCounts,

    pub error_count: u64,
    pub access_time_reliable: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortKey {
    Name,
    #[default]
    Size,
    Modified,
    Created,
    Category,
    Extension,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QueryRequest {
    /// Empty or absent means every category.
    pub categories: Vec<Category>,
    /// Case-insensitive substring over the filename.
    pub text: Option<String>,
    pub ext: Option<String>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub modified_after: Option<i64>,
    pub modified_before: Option<i64>,
    /// Display-path prefix, used to drill into one folder.
    pub parent_prefix: Option<String>,
    /// Entry must have all of these flag bits.
    pub require_flags: u16,
    /// Entry must have none of these flag bits.
    pub exclude_flags: u16,
    pub sort: SortKey,
    pub descending: bool,
    pub offset: u32,
    pub limit: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResponse {
    /// Matches across the whole index, not just this page.
    pub total: u64,
    /// Total bytes of every match, so the UI can show "412 files, 8.1 GB"
    /// without fetching them all.
    pub bytes: u64,
    pub rows: Vec<FileRow>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickFindResponse {
    pub interpretation: crate::core::quickfind::Interpretation,
    pub total: u64,
    pub bytes: u64,
    pub rows: Vec<FileRow>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInfo {
    pub os: &'static str,
    pub downloads_dir: Option<String>,
    pub home_dir: Option<String>,
    pub access_time_reliable: bool,
    /// macOS gates ~/Downloads behind a TCC consent prompt; the UI explains
    /// the permission error differently when this is true.
    pub privacy_prompts: bool,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ScanStatus {
    Idle,
    Scanning,
    Ready,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanState {
    pub status: ScanStatus,
    pub scan_id: Option<u64>,
    pub root: Option<String>,
}

// ------------------------------------------------------- storage analyzer ---

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderRow {
    pub path: String,
    /// Home prefix collapsed to `~`, for display.
    pub display: String,
    /// Recursive: this folder and everything beneath it.
    pub bytes: u64,
    pub files: u64,
    /// Levels below the scan root; the root itself is excluded.
    pub depth: u16,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageReport {
    pub total_bytes: u64,
    pub total_files: u64,
    pub largest_folders: Vec<FolderRow>,
    pub largest_files: Vec<FileRow>,
    pub stale_files: Vec<FileRow>,
    pub temporary_files: Vec<FileRow>,
    pub by_category: Vec<CategoryStat>,
    pub empty_folders: EmptyFolders,
    pub stale: StaleStats,
    /// Totals are a lower bound: the depth cap or entry ceiling was reached.
    pub truncated: bool,
}

// ---------------------------------------------------------- duplicates ---

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroup {
    /// First 12 hex characters - enough to identify a group in the UI without
    /// shipping 64 characters per row.
    pub hash: String,
    pub size: u64,
    /// Freed only if every copy but one goes.
    pub reclaimable: u64,
    pub keep_id: u32,
    /// Why this copy was suggested. Shown verbatim; it is a suggestion, and
    /// nothing in the UI is pre-selected for deletion.
    pub keep_reason: String,
    /// `keep_id` first.
    pub files: Vec<FileRow>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateStats {
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
    pub groups: u64,
    pub reclaimable: u64,
    pub verified_bytewise: bool,
    pub cancelled: bool,
    pub duration_ms: u64,
    pub error_count: u64,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateReport {
    pub scan_id: u64,
    pub groups: Vec<DuplicateGroup>,
    pub stats: DuplicateStats,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DedupeProgress {
    pub scan_id: u64,
    pub stage: &'static str,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DedupeStatus {
    Idle,
    Running,
    Ready,
    Failed,
}

// --------------------------------------------------- operations and undo ---

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationItemRow {
    pub name: String,
    pub from_path: String,
    pub to_path: Option<String>,
    pub size: u64,
    pub state: &'static str,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationRow {
    pub id: i64,
    pub kind: &'static str,
    pub state: &'static str,
    pub created_at: i64,
    pub note: String,
    pub root: String,
    pub done: u64,
    pub failed: u64,
    pub bytes: u64,
    /// Whether undo is offered. False once undone, or if nothing landed.
    pub undoable: bool,
    pub items: Vec<OperationItemRow>,
}

/// Running totals, so the user can see what tidying up has actually achieved
/// rather than only ever being shown what is left to do.
#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LifetimeStats {
    pub files: u64,
    pub bytes: u64,
    pub operations: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoOutcome {
    pub operation_id: i64,
    pub restored: u64,
    pub failed: u64,
    /// Items undo could not reverse, each with the reason.
    pub problems: Vec<String>,
    /// True when the operation sent files to the Trash, where restoring is the
    /// operating system's job rather than ours.
    pub manual_restore_required: bool,
}

// ------------------------------------------------ organization suggestions ---

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanItem {
    pub id: u32,
    pub name: String,
    pub from_path: String,
    /// Folder name inside the scan root, e.g. "Images".
    pub destination: String,
    pub to_path: String,
    pub size: u64,
    pub category: Category,
    /// Something already exists at `to_path`, so the move would be renamed
    /// rather than overwrite it. There is no code path that overwrites.
    pub collides: bool,
    /// The name that would actually be used, when `collides` is true.
    pub resolved_name: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanGroup {
    pub destination: String,
    pub category: Category,
    pub files: u64,
    pub bytes: u64,
    pub items: Vec<PlanItem>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizePlan {
    pub scan_id: u64,
    pub root: String,
    pub groups: Vec<PlanGroup>,
    pub total_files: u64,
    pub total_bytes: u64,
    /// Left where they are on purpose - links, placeholders, part-downloads,
    /// or files already filed in a subfolder.
    pub skipped: u64,
    pub collisions: u64,
}

// ------------------------------------------------------------- treemap ---

/// One rectangle in the storage map.
///
/// A folder carries its *recursive* size; a file carries its own. `id` is
/// present only for files, so the map can hand a real target to the same
/// operations the file list uses rather than re-deriving one from a path.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreemapItem {
    pub name: String,
    pub path: String,
    pub bytes: u64,
    pub files: u64,
    pub is_dir: bool,
    pub category: Category,
    /// File id, for files only.
    pub id: Option<u32>,
    /// Set on the aggregate row that stands in for everything too small to
    /// draw, so the UI can label it honestly instead of as a folder.
    pub is_remainder: bool,
}

/// One level of the storage map: the contents of `path`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreemapNode {
    pub path: String,
    pub display: String,
    /// Recursive size of this folder.
    pub bytes: u64,
    pub files: u64,
    /// The folder one level up, or `None` at the scan root.
    pub parent: Option<String>,
    /// Breadcrumb from the scan root down to here, inclusive.
    pub trail: Vec<FolderRow>,
    pub items: Vec<TreemapItem>,
    /// How many items were folded into the remainder row.
    pub remainder_count: u64,
}
