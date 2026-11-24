//! Platform-specific file identity and attribute probing.
//!
//! Everything here is `cfg`-gated and returns the same `Probe` shape upward, so
//! the walker contains no platform branches at all.
//!
//! Two things in this file prevent real, user-visible damage:
//!
//! 1. **Identity** - `(volume serial, file index)` on Windows, `(st_dev,
//!    st_ino)` on Unix. Windows junctions, macOS firmlinks and hardlinks make
//!    one file appear at two paths. Without identity, a scanner reports every
//!    such file as a duplicate of itself and happily offers to delete "the
//!    copy" - which is the same bytes.
//!
//! 2. **Cloud placeholders** - OneDrive and iCloud Drive files report a size in
//!    their metadata but hold no local data. Opening one triggers a download,
//!    potentially of gigabytes over a metered connection. These are flagged and
//!    excluded from all reads and all operations.

use crate::core::model::{EntryFlags, FileIdent};
use std::ffi::OsStr;
use std::fs::Metadata;
use std::path::{Path, PathBuf};

pub struct Probe {
    pub flags: u16,
    pub ident: Option<FileIdent>,
}

// ---------------------------------------------------------------- Windows ---

#[cfg(windows)]
mod imp {
    use super::*;
    use std::os::windows::fs::MetadataExt;

    // Defined locally rather than taking a `windows-sys` dependency for five
    // integers. Values are from winnt.h and are stable ABI.
    const FILE_ATTRIBUTE_READONLY: u32 = 0x0000_0001;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x0000_0002;
    const FILE_ATTRIBUTE_SYSTEM: u32 = 0x0000_0004;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    /// Content is not local (classic HSM / "online-only").
    const FILE_ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;
    /// OneDrive placeholder: opening the file hydrates it.
    const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
    /// OneDrive placeholder: reading the data hydrates it.
    const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;

    const CLOUD_BITS: u32 = FILE_ATTRIBUTE_OFFLINE
        | FILE_ATTRIBUTE_RECALL_ON_OPEN
        | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS;

    pub fn probe(_name_folded: &str, md: &Metadata) -> Probe {
        let attrs = md.file_attributes();
        let mut flags = 0u16;

        if attrs & FILE_ATTRIBUTE_HIDDEN != 0 {
            flags |= EntryFlags::HIDDEN;
        }
        if attrs & FILE_ATTRIBUTE_READONLY != 0 {
            flags |= EntryFlags::READONLY;
        }
        if attrs & FILE_ATTRIBUTE_SYSTEM != 0 {
            // Treated as hidden for display, and never a cleanup candidate.
            flags |= EntryFlags::HIDDEN | EntryFlags::SYSTEM_PROTECTED;
        }
        if attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            // Junction, mount point, or a symlink std did not classify.
            flags |= EntryFlags::REPARSE;
        }
        if attrs & CLOUD_BITS != 0 {
            flags |= EntryFlags::CLOUD_PLACEHOLDER;
        }

        // No identity here on Windows.
        //
        // `MetadataExt::volume_serial_number` and `file_index` are still
        // unstable (`windows_by_handle`), and the stable alternative -
        // `GetFileInformationByHandle` - needs a `CreateFile` per entry, which
        // is far too expensive to pay for every file during a metadata walk.
        //
        // Nothing is lost, because identity is only needed for *duplicate*
        // detection, to recognise two paths that are one hardlinked file. The
        // hasher (Phase 3) already opens each candidate, so it can read the
        // identity for free exactly where it matters - and that is a far
        // smaller set of files than the whole folder.
        //
        // Loop protection during the walk does not depend on this: it keys on
        // the canonical directory path (`dir_key`), which resolves junctions
        // and firmlinks on both platforms.
        Probe { flags, ident: None }
    }

    /// Windows paths longer than MAX_PATH (260) need the verbatim prefix for
    /// most filesystem calls. Rust's std applies it in some paths but not all,
    /// so operations (Phase 4) go through this.
    pub fn verbatim(path: &std::path::Path) -> std::path::PathBuf {
        use std::path::PathBuf;
        let s = path.as_os_str().to_string_lossy();
        if s.starts_with("\\\\?\\") || s.starts_with("\\\\.\\") {
            return path.to_path_buf();
        }
        if s.len() < 248 || !path.is_absolute() {
            return path.to_path_buf();
        }
        if let Some(stripped) = s.strip_prefix("\\\\") {
            // UNC share: \\server\share -> \\?\UNC\server\share
            return PathBuf::from(format!("\\\\?\\UNC\\{stripped}"));
        }
        PathBuf::from(format!("\\\\?\\{s}"))
    }
}

// ------------------------------------------------------------------- Unix ---

#[cfg(unix)]
mod imp {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    pub fn probe(name_folded: &str, md: &Metadata) -> Probe {
        let mut flags = 0u16;

        if name_folded.starts_with('.') {
            flags |= EntryFlags::HIDDEN;
        }
        if md.permissions().readonly() {
            flags |= EntryFlags::READONLY;
        }
        // iCloud Drive evicts a file's data and leaves a stub named
        // `.<original name>.icloud`. This is the form that actually appears on
        // disk, so matching it covers the real case. The authoritative check
        // for a materialised-but-dataless file is the `SF_DATALESS` st_flag,
        // which std does not expose; the hasher (Phase 3) performs that check
        // at open time, where it can also react to the kernel's error.
        if name_folded.starts_with('.') && name_folded.ends_with(".icloud") {
            flags |= EntryFlags::CLOUD_PLACEHOLDER;
        }

        Probe {
            flags,
            ident: Some(FileIdent {
                volume: md.dev(),
                file_id: md.ino(),
            }),
        }
    }

    pub fn verbatim(path: &std::path::Path) -> std::path::PathBuf {
        path.to_path_buf()
    }
}

/// Probe attributes and identity from already-obtained `symlink_metadata`.
///
/// `name_folded` is the lowercased NFC name (see `text::fold`); Unix needs it
/// for dot-file and iCloud-stub detection, Windows ignores it.
pub fn probe(name_folded: &str, md: &Metadata) -> Probe {
    imp::probe(name_folded, md)
}

/// Prepare a path for a filesystem call. On Windows this applies the `\\?\`
/// verbatim prefix for long paths; elsewhere it is the identity function.
pub fn for_io(path: &Path) -> PathBuf {
    imp::verbatim(path)
}

/// Drop the `\\?\` verbatim prefix that `canonicalize` adds on Windows.
///
/// Keeping it would leak into every path shown to the user and would break
/// naive prefix comparisons against paths obtained any other way. `for_io`
/// puts it back when a path is actually long enough to need it.
pub fn strip_verbatim(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.as_os_str().to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest.to_string());
        }
    }
    path
}

/// A stable key identifying a directory, for the walk's visited set.
///
/// This is how loop protection works, and it replaces `(volume, inode)`
/// because the Windows accessors for those are still unstable and the stable
/// alternative costs a file handle per entry.
///
/// `canonicalize` resolves symlinks, Windows junctions, macOS firmlinks and
/// bind mounts, so two routes to the same directory produce the same key and
/// the second one is skipped. One syscall per directory, never per file.
pub fn dir_key(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok().map(strip_verbatim)
}

/// Whether the platform maintains last-access timestamps reliably.
///
/// Windows ships with `NtfsDisableLastAccessUpdate` enabled, so `accessed` is
/// typically just a copy of the creation or modification time. Surfacing
/// "not opened in 11 months" from that would be a fabrication, so the UI is
/// told to fall back to the modification time and to label the column honestly.
pub const fn access_time_reliable() -> bool {
    !cfg!(windows)
}

/// Resolve the display name a user would recognise for the home-relative part
/// of a path. Used only for UI labels.
pub fn shorten_for_display(path: &OsStr) -> String {
    let full = crate::core::text::display(path);
    if let Some(home) = dirs::home_dir() {
        let home_str = crate::core::text::display(home.as_os_str());
        if !home_str.is_empty() {
            if let Some(rest) = full.strip_prefix(&home_str) {
                let sep = if cfg!(windows) { '\\' } else { '/' };
                return format!("~{}", rest.strip_prefix(sep).map_or(rest.to_string(), |r| format!("{sep}{r}")));
            }
        }
    }
    full
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dir_key_is_stable_and_resolves_indirection() {
        let dir = std::env::temp_dir();
        let first = dir_key(&dir).expect("temp dir must resolve");
        let second = dir_key(&dir).expect("temp dir must resolve");
        assert_eq!(first, second, "the visited-set key must be stable");

        // Reaching the same directory by a longer route must produce the same
        // key, otherwise the walk's loop protection does not protect anything.
        let roundabout = dir.join("..").join(
            dir.file_name()
                .expect("temp dir must have a final component"),
        );
        assert_eq!(dir_key(&roundabout), Some(first));
    }

    #[test]
    fn dir_key_has_no_verbatim_prefix() {
        let key = dir_key(&std::env::temp_dir()).expect("temp dir must resolve");
        assert!(
            !key.to_string_lossy().starts_with(r"\\?\"),
            "the verbatim prefix must not leak into stored paths: {key:?}"
        );
    }

    #[test]
    fn a_missing_directory_has_no_key() {
        let missing = std::env::temp_dir().join("cleanup-no-such-dir-13579");
        assert_eq!(dir_key(&missing), None);
    }

    #[test]
    fn hidden_detection_matches_the_platform_convention() {
        // Only assert the Unix convention on Unix: on Windows a leading dot
        // carries no meaning and the attribute bit is what counts.
        if cfg!(unix) {
            let md = std::fs::symlink_metadata(std::env::temp_dir()).unwrap();
            assert!(probe(".hidden", &md).flags & EntryFlags::HIDDEN != 0);
            assert!(probe("visible", &md).flags & EntryFlags::HIDDEN == 0);
        }
    }
}
