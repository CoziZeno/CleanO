//! Core data model for a scan.
//!
//! Design notes that matter:
//!
//! * `FileEntry::name` is a `Box<OsStr>`, not a `String`. Windows paths are
//!   UTF-16 and may contain unpaired surrogates; Unix paths are arbitrary
//!   bytes. Neither is guaranteed to be valid UTF-8, and macOS/APFS hands back
//!   NFD-normalised names while Windows stores NFC. Keeping the on-disk form as
//!   the single source of truth means a path we hand to the OS is byte-identical
//!   to what we read, and normalisation happens only at the display boundary.
//!
//! * Parent directories are interned. A Downloads tree with 100k files shares a
//!   few hundred directories; storing a full path per entry would waste ~12 MB
//!   in duplicated prefixes.
//!
//! * `accessed` is an `Option`. Windows disables last-access-time updates by
//!   default (`NtfsDisableLastAccessUpdate`), so "not opened in a year" is
//!   simply false there. The UI is told whether to trust it.

use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Broad file class, derived from the extension (see `category.rs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Images,
    Videos,
    Audio,
    Documents,
    Archives,
    Installers,
    Development,
    Temporary,
    Misc,
}

impl Category {
    pub const ALL: [Category; 9] = [
        Category::Images,
        Category::Videos,
        Category::Audio,
        Category::Documents,
        Category::Archives,
        Category::Installers,
        Category::Development,
        Category::Temporary,
        Category::Misc,
    ];

    /// Stable key used in IPC payloads and as a settings map key.
    pub fn key(self) -> &'static str {
        match self {
            Category::Images => "images",
            Category::Videos => "videos",
            Category::Audio => "audio",
            Category::Documents => "documents",
            Category::Archives => "archives",
            Category::Installers => "installers",
            Category::Development => "development",
            Category::Temporary => "temporary",
            Category::Misc => "misc",
        }
    }

    /// Default destination folder name used by the organizer (Phase 4).
    pub fn default_folder(self) -> &'static str {
        match self {
            Category::Images => "Images",
            Category::Videos => "Videos",
            Category::Audio => "Audio",
            Category::Documents => "Documents",
            Category::Archives => "Archives",
            Category::Installers => "Installers",
            Category::Development => "Development",
            Category::Temporary => "Temporary",
            Category::Misc => "Miscellaneous",
        }
    }

    pub fn from_key(key: &str) -> Option<Category> {
        Category::ALL.into_iter().find(|c| c.key() == key)
    }
}

/// Per-entry bit flags.
///
/// Hand-rolled rather than pulling in `bitflags`: nine bits do not justify a
/// dependency, and a transparent `u16` is the cheapest thing to ship over IPC
/// for a 200-row page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntryFlags(pub u16);

impl EntryFlags {
    pub const HIDDEN: u16 = 1 << 0;
    pub const SYMLINK: u16 = 1 << 1;
    pub const READONLY: u16 = 1 << 2;
    /// OneDrive / iCloud stub: reports a size but holds no local data. Reading
    /// one triggers a network download. Never hash, never move.
    pub const CLOUD_PLACEHOLDER: u16 = 1 << 3;
    /// Browser partial download, or modified within the last minute.
    pub const IN_PROGRESS: u16 = 1 << 4;
    /// Path matched the system denylist; all operations refused.
    pub const SYSTEM_PROTECTED: u16 = 1 << 5;
    /// macOS `*.app` - a directory the user thinks of as one file.
    pub const APP_BUNDLE: u16 = 1 << 6;
    /// Windows reparse point / junction, or any non-regular directory link.
    pub const REPARSE: u16 = 1 << 7;
    /// Known noise (`.DS_Store`, `Thumbs.db`, `desktop.ini`, AppleDouble).
    pub const NOISE: u16 = 1 << 8;

    #[inline]
    pub fn has(self, bits: u16) -> bool {
        self.0 & bits != 0
    }

    #[inline]
    pub fn set(&mut self, bits: u16) {
        self.0 |= bits;
    }

    /// Bits that forbid *mutating* an entry - moving it, trashing it, or
    /// hashing it in full. Checked by the hasher (Phase 3) and by every
    /// operation (Phase 4).
    pub const UNSAFE_TO_TOUCH: u16 = Self::SYMLINK
        | Self::CLOUD_PLACEHOLDER
        | Self::IN_PROGRESS
        | Self::SYSTEM_PROTECTED
        | Self::APP_BUNDLE
        | Self::REPARSE;

    /// Bits that forbid reading an entry's *contents at all*.
    ///
    /// Narrower than `UNSAFE_TO_TOUCH`, and the difference is deliberate:
    /// `IN_PROGRESS` is absent. A partial download must never be moved,
    /// trashed or hashed, because what it contains is still changing - but its
    /// first 32 bytes are already on disk and reading them to work out what
    /// kind of file it is costs nothing and changes nothing. A cloud
    /// placeholder is in both sets, because any read of one downloads the whole
    /// file over the user's connection.
    pub const UNSAFE_TO_READ: u16 =
        Self::SYMLINK | Self::CLOUD_PLACEHOLDER | Self::SYSTEM_PROTECTED | Self::APP_BUNDLE | Self::REPARSE;

    /// Entries that are not really files at all, and so do not belong in a
    /// "largest" or "oldest" list however big or old they look.
    pub const NOT_A_REAL_FILE: u16 = Self::SYMLINK | Self::REPARSE | Self::NOISE;

    #[inline]
    pub fn is_safe_to_touch(self) -> bool {
        !self.has(Self::UNSAFE_TO_TOUCH)
    }

    #[inline]
    pub fn is_safe_to_read(self) -> bool {
        !self.has(Self::UNSAFE_TO_READ)
    }

    #[inline]
    pub fn is_real_file(self) -> bool {
        !self.has(Self::NOT_A_REAL_FILE)
    }
}

/// Filesystem identity: `(volume serial, file index)` on Windows,
/// `(st_dev, st_ino)` on Unix. Two entries with the same identity are the
/// *same file* reached twice - a hardlink, a junction or a firmlink - not a
/// duplicate, and not two things to delete.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileIdent {
    pub volume: u64,
    pub file_id: u64,
}

#[derive(Debug)]
pub struct FileEntry {
    pub id: u32,
    /// Index into `ScanIndex::parents`.
    pub parent: u32,
    /// On-disk file name. Never normalised - see module docs.
    pub name: Box<OsStr>,
    /// Lowercased NFC form of `name`, for search and sorting.
    ///
    /// Stored rather than recomputed: the walk already derives it for the
    /// noise and ignore checks, so keeping it is nearly free, and it removes
    /// an allocation per comparison from every query. At 85k files that was
    /// the difference between a 164 ms sort and a 2 ms one.
    pub name_folded: Box<str>,
    /// Lowercased extension without the dot. ASCII-folded only.
    pub ext: Option<Box<str>>,
    pub size: u64,
    pub created: Option<i64>,
    pub modified: Option<i64>,
    pub accessed: Option<i64>,
    pub category: Category,
    pub flags: EntryFlags,
    pub ident: Option<FileIdent>,
    /// BLAKE3 over the first `hash.headBytes` bytes. Filled in Phase 3.
    pub head_hash: Option<[u8; 32]>,
    /// BLAKE3 over the whole file. Filled in Phase 3.
    pub full_hash: Option<[u8; 32]>,
}

#[derive(Debug)]
pub struct DirRecord {
    /// Index into `ScanIndex::parents` for this directory's own path.
    pub path: u32,
    pub depth: u16,
    pub direct_files: u32,
    pub direct_dirs: u32,
    pub direct_bytes: u64,
    /// Files that are pure noise. A directory containing only these is
    /// "empty" as far as a human is concerned.
    pub noise_files: u32,
    /// True when the depth cap stopped us descending, so counts are partial.
    pub truncated: bool,
}

impl DirRecord {
    /// A user calls a folder empty when there is nothing in it they care about.
    pub fn is_effectively_empty(&self) -> bool {
        !self.truncated && self.direct_dirs == 0 && self.direct_files == self.noise_files
    }

    pub fn is_strictly_empty(&self) -> bool {
        !self.truncated && self.direct_dirs == 0 && self.direct_files == 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScanErrorKind {
    PermissionDenied,
    NotFound,
    /// macOS TCC consent not granted, or Windows access restriction.
    AccessRestricted,
    Other,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanError {
    pub path: String,
    pub kind: ScanErrorKind,
    pub message: String,
}

/// Interning table for directory paths.
#[derive(Default)]
pub struct PathIntern {
    values: Vec<Arc<OsStr>>,
    lookup: std::collections::HashMap<OsString, u32>,
}

impl PathIntern {
    pub fn intern(&mut self, path: &OsStr) -> u32 {
        if let Some(&id) = self.lookup.get(path) {
            return id;
        }
        let id = self.values.len() as u32;
        let owned: OsString = path.to_os_string();
        self.lookup.insert(owned.clone(), id);
        // Via `OsString` rather than `&OsStr`: one allocation per *unique*
        // directory, of which a Downloads tree has a few hundred.
        self.values.push(Arc::from(owned));
        id
    }

    pub fn get(&self, id: u32) -> Option<&Arc<OsStr>> {
        self.values.get(id as usize)
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (u32, &Arc<OsStr>)> {
        self.values.iter().enumerate().map(|(i, v)| (i as u32, v))
    }
}

/// The complete result of one scan. Lives in Rust memory; the webview only
/// ever receives aggregates and windowed pages built from it.
pub struct ScanIndex {
    pub scan_id: u64,
    pub root: PathBuf,
    pub parents: PathIntern,
    pub entries: Vec<FileEntry>,
    pub dirs: Vec<DirRecord>,
    pub errors: Vec<ScanError>,
    /// The depth cap stopped the walk somewhere, so totals are a lower bound.
    pub truncated: bool,
    pub duration_ms: u64,
    pub scanned_at: i64,
    pub cancelled: bool,
}

impl ScanIndex {
    /// Rebuild the real on-disk path for an entry. This is the only correct way
    /// to get a path for I/O - never reconstruct one from a display string.
    pub fn path_of(&self, entry: &FileEntry) -> PathBuf {
        let mut p = match self.parents.get(entry.parent) {
            Some(parent) => PathBuf::from(parent.as_ref()),
            None => PathBuf::new(),
        };
        p.push(entry.name.as_ref());
        p
    }

    pub fn entry(&self, id: u32) -> Option<&FileEntry> {
        self.entries.get(id as usize)
    }

    pub fn dir_path(&self, dir: &DirRecord) -> PathBuf {
        match self.parents.get(dir.path) {
            Some(p) => PathBuf::from(p.as_ref()),
            None => PathBuf::new(),
        }
    }

    pub fn total_bytes(&self) -> u64 {
        self.entries.iter().map(|e| e.size).sum()
    }
}

/// `SystemTime` to unix seconds. Returns `None` for times the platform does not
/// provide (Linux has no birth time on some filesystems) or that predate the
/// epoch, which we treat as "unknown" rather than inventing a value.
pub fn to_unix_secs(t: SystemTime) -> Option<i64> {
    t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs() as i64)
}

pub fn now_unix_secs() -> i64 {
    to_unix_secs(SystemTime::now()).unwrap_or(0)
}

/// Calendar year, month and day (UTC) from unix seconds.
///
/// Howard Hinnant's civil-from-days algorithm. Worth the twelve lines: the
/// alternative is a date dependency for one conversion, and the naive
/// "divide by 365" version gets leap years wrong, which would file a photo
/// taken on the 29th of February under the wrong month.
pub fn civil_from_unix(seconds: i64) -> (i64, u32, u32) {
    // Floor division, so dates before 1970 do not round towards zero.
    let days = seconds.div_euclid(86_400);

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;

    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_dates_round_trip_from_unix_seconds() {
        assert_eq!(civil_from_unix(0), (1970, 1, 1));
        assert_eq!(civil_from_unix(10_957 * 86_400), (2000, 1, 1));
        // A leap day, which the naive version gets wrong.
        assert_eq!(civil_from_unix(19_782 * 86_400), (2024, 2, 29));
        // The last second of a year must not spill into the next one.
        assert_eq!(civil_from_unix(1_735_689_599), (2024, 12, 31));
        assert_eq!(civil_from_unix(1_735_689_600), (2025, 1, 1));
    }

    #[test]
    fn dates_before_the_epoch_do_not_round_the_wrong_way() {
        assert_eq!(civil_from_unix(-1), (1969, 12, 31));
        assert_eq!(civil_from_unix(-86_400), (1969, 12, 31));
    }
}
