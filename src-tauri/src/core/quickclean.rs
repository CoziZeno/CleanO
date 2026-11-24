//! Quick Clean: the well-known junk caches Windows and macOS leave behind.
//!
//! This is the one part of the app that deletes permanently and the one part
//! that touches system folders, so it is also the most tightly constrained.
//!
//! **The target list is closed and hardcoded.** Every path comes from
//! `targets()`, resolved from the operating system's own environment, and the
//! only thing that crosses the IPC boundary is a `TargetKind` - never a path.
//! `clean()` re-resolves the directory from the kind it was given, so there is
//! no argument anyone could pass that makes this delete somewhere else. That
//! is what makes an exception to the denylist acceptable here: it is not a
//! general-purpose bypass, it is nine specific folders whose entire purpose is
//! to be disposable.
//!
//! The other rules:
//!
//! * **The target directory is never removed**, only its contents. Deleting
//!   `%TEMP%` itself breaks programs that expect it to exist.
//! * **Links are never followed.** A reparse point inside a temp folder could
//!   point anywhere; it is removed as a link, never traversed.
//! * **Files written in the last minute are left alone.** Something is still
//!   using them.
//! * **A file that cannot be deleted is skipped and counted**, never reported
//!   as freed. Most of these folders hold files that are open, and the honest
//!   answer is "347 in use, left alone".
//! * Measuring and deleting are separate calls, so the user sees exact
//!   folders and sizes before anything happens.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Depth cap for both measuring and cleaning. Temp trees are shallow; this
/// stops a pathological one from taking the process with it.
const MAX_DEPTH: usize = 16;

/// A file touched this recently is assumed to be in use.
const IN_USE_GRACE_SECS: u64 = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TargetKind {
    UserTemp,
    WindowsTemp,
    Prefetch,
    WindowsUpdateCache,
    DeliveryOptimization,
    CrashDumps,
    ErrorReports,
    ThumbnailCache,
    RecycleBin,
    ChromeCache,
    EdgeCache,
    BraveCache,
    MacUserCaches,
    MacUserLogs,
}

impl TargetKind {
    pub fn key(self) -> &'static str {
        match self {
            TargetKind::UserTemp => "userTemp",
            TargetKind::WindowsTemp => "windowsTemp",
            TargetKind::Prefetch => "prefetch",
            TargetKind::WindowsUpdateCache => "windowsUpdateCache",
            TargetKind::DeliveryOptimization => "deliveryOptimization",
            TargetKind::CrashDumps => "crashDumps",
            TargetKind::ErrorReports => "errorReports",
            TargetKind::ThumbnailCache => "thumbnailCache",
            TargetKind::RecycleBin => "recycleBin",
            TargetKind::ChromeCache => "chromeCache",
            TargetKind::EdgeCache => "edgeCache",
            TargetKind::BraveCache => "braveCache",
            TargetKind::MacUserCaches => "macUserCaches",
            TargetKind::MacUserLogs => "macUserLogs",
        }
    }
}

/// What inside the directory is eligible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    /// Everything inside it. The directory itself is kept.
    Contents,
    /// Only files whose lowercased name matches one of these patterns.
    /// Used where a folder holds disposable caches *and* things to keep.
    Matching(&'static [&'static str]),
    /// Not a directory walk at all - see `recycle_bin`.
    RecycleBin,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub kind: TargetKind,
    pub key: &'static str,
    pub title: &'static str,
    /// What this is, and what clearing it costs. Shown to the user verbatim.
    pub description: &'static str,
    pub path: String,
    /// Usually needs administrator rights; without them most of it is skipped.
    pub needs_admin: bool,
    /// Ticked by default. Off for anything with a real downside.
    pub default_on: bool,
    /// The downside, when there is one. `None` means genuinely free.
    pub caveat: Option<&'static str>,
}

/// What a target currently holds.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Measurement {
    #[serde(flatten)]
    pub target: Target,
    pub bytes: u64,
    pub files: u64,
    /// The folder does not exist on this machine, so there is nothing to do.
    pub missing: bool,
    /// The folder exists but could not be read, usually for want of
    /// administrator rights. Its size is unknown rather than zero.
    pub unreadable: bool,
}

/// What actually happened.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub key: String,
    /// Only counts files that were really removed.
    pub freed_bytes: u64,
    pub removed_files: u64,
    pub removed_dirs: u64,
    /// Open, locked, or written in the last minute.
    pub skipped_in_use: u64,
    /// Refused by the operating system - usually needs administrator rights.
    pub skipped_denied: u64,
    /// A representative failure, for the UI to show rather than a bare count.
    pub example_error: Option<String>,
}

/// Every target on this platform, whether or not it currently exists.
pub fn targets() -> Vec<Target> {
    imp::targets()
}

/// Look up a target by the key the UI sent back. Returns `None` for anything
/// that is not in the closed list, which is the whole point.
pub fn target_for(key: &str) -> Option<Target> {
    targets().into_iter().find(|t| t.key == key)
}

/// Measure every target. Read-only.
pub fn measure_all() -> Vec<Measurement> {
    targets().into_iter().map(measure).collect()
}

pub fn measure(target: Target) -> Measurement {
    if matches!(scope_of(target.kind), Scope::RecycleBin) {
        let (bytes, files, ok) = imp::recycle_bin_size();
        return Measurement {
            bytes,
            files,
            missing: false,
            unreadable: !ok,
            target,
        };
    }

    let path = PathBuf::from(&target.path);
    if target.path.is_empty() || !path.is_dir() {
        return Measurement {
            bytes: 0,
            files: 0,
            missing: true,
            unreadable: false,
            target,
        };
    }

    let mut bytes = 0u64;
    let mut files = 0u64;
    let mut denied = false;
    sum_dir(
        &path,
        scope_of(target.kind),
        0,
        true,
        &mut bytes,
        &mut files,
        &mut denied,
    );

    Measurement {
        bytes,
        files,
        missing: false,
        // Nothing readable *and* something refused us is "unknown", not
        // "empty". Showing 0 for a folder we could not open would be a lie.
        unreadable: denied && files == 0,
        target,
    }
}

/// Delete the contents of one target.
///
/// `key` is looked up against the closed list; an unknown key does nothing.
pub fn clean(key: &str) -> Outcome {
    let Some(target) = target_for(key) else {
        return Outcome {
            key: key.to_string(),
            ..Outcome::default()
        };
    };

    let mut outcome = Outcome {
        key: key.to_string(),
        ..Outcome::default()
    };

    if matches!(scope_of(target.kind), Scope::RecycleBin) {
        imp::empty_recycle_bin(&mut outcome);
        return outcome;
    }

    let path = PathBuf::from(&target.path);
    if target.path.is_empty() || !path.is_dir() {
        return outcome;
    }

    clean_dir(&path, scope_of(target.kind), 0, true, &mut outcome);
    outcome
}

fn scope_of(kind: TargetKind) -> Scope {
    match kind {
        TargetKind::RecycleBin => Scope::RecycleBin,
        // The Explorer folder also holds real settings; only the caches go.
        TargetKind::ThumbnailCache => {
            Scope::Matching(&["thumbcache_*.db", "iconcache_*.db", "thumbcache_*", "iconcache_*"])
        }
        _ => Scope::Contents,
    }
}

/// Does this entry qualify under the target's scope?
///
/// Patterns only apply at the top level: a folder matched at the top is taken
/// whole, and inside it everything counts.
fn eligible(scope: Scope, top_level: bool, folded_name: &str) -> bool {
    match scope {
        Scope::Contents | Scope::RecycleBin => true,
        Scope::Matching(patterns) => {
            !top_level
                || patterns
                    .iter()
                    .any(|p| crate::core::text::matches_pattern(folded_name, p))
        }
    }
}

fn is_link(md: &std::fs::Metadata) -> bool {
    if md.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        return md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    }
    #[cfg(not(windows))]
    false
}

fn recently_written(md: &std::fs::Metadata) -> bool {
    md.modified()
        .ok()
        .and_then(|m| SystemTime::now().duration_since(m).ok())
        .map(|age| age.as_secs() < IN_USE_GRACE_SECS)
        .unwrap_or(false)
}

fn sum_dir(
    dir: &Path,
    scope: Scope,
    depth: usize,
    top_level: bool,
    bytes: &mut u64,
    files: &mut u64,
    denied: &mut bool,
) {
    if depth > MAX_DEPTH {
        return;
    }
    let entries = match std::fs::read_dir(crate::core::identity::for_io(dir)) {
        Ok(entries) => entries,
        Err(err) => {
            if err.kind() == std::io::ErrorKind::PermissionDenied {
                *denied = true;
            }
            return;
        }
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let folded = crate::core::text::fold(&crate::core::text::display(&name));
        if !eligible(scope, top_level, &folded) {
            continue;
        }
        let path = entry.path();
        let Ok(md) = std::fs::symlink_metadata(crate::core::identity::for_io(&path)) else {
            continue;
        };
        // A link occupies almost nothing and is never followed, so it does not
        // contribute its target's size.
        if is_link(&md) {
            *files += 1;
            continue;
        }
        if md.is_dir() {
            sum_dir(&path, scope, depth + 1, false, bytes, files, denied);
        } else {
            *bytes += md.len();
            *files += 1;
        }
    }
}

/// Remove contents depth-first, so a directory is only tried once its children
/// are gone.
fn clean_dir(dir: &Path, scope: Scope, depth: usize, top_level: bool, outcome: &mut Outcome) {
    if depth > MAX_DEPTH {
        return;
    }
    let entries = match std::fs::read_dir(crate::core::identity::for_io(dir)) {
        Ok(entries) => entries,
        Err(err) => {
            note_failure(outcome, dir, &err);
            return;
        }
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let folded = crate::core::text::fold(&crate::core::text::display(&name));
        if !eligible(scope, top_level, &folded) {
            continue;
        }
        let path = entry.path();
        let Ok(md) = std::fs::symlink_metadata(crate::core::identity::for_io(&path)) else {
            continue;
        };

        if is_link(&md) {
            // Removed as a link. `remove_file` unlinks a symlink on Unix and
            // `remove_dir` removes a Windows junction without touching what it
            // points at - neither follows it.
            let removed = if md.is_dir() {
                std::fs::remove_dir(crate::core::identity::for_io(&path))
            } else {
                std::fs::remove_file(crate::core::identity::for_io(&path))
            };
            match removed {
                Ok(()) => outcome.removed_files += 1,
                Err(err) => note_failure(outcome, &path, &err),
            }
            continue;
        }

        if md.is_dir() {
            clean_dir(&path, scope, depth + 1, false, outcome);
            // Only succeeds once empty, which is exactly the condition we
            // want: a directory still holding a locked file stays.
            if std::fs::remove_dir(crate::core::identity::for_io(&path)).is_ok() {
                outcome.removed_dirs += 1;
            }
            continue;
        }

        if recently_written(&md) {
            outcome.skipped_in_use += 1;
            continue;
        }

        let size = md.len();
        match std::fs::remove_file(crate::core::identity::for_io(&path)) {
            // Bytes are counted only after the removal succeeded, so the
            // reported total is what was actually freed.
            Ok(()) => {
                outcome.freed_bytes += size;
                outcome.removed_files += 1;
            }
            Err(err) => note_failure(outcome, &path, &err),
        }
    }
}

fn note_failure(outcome: &mut Outcome, path: &Path, err: &std::io::Error) {
    match err.kind() {
        std::io::ErrorKind::PermissionDenied => outcome.skipped_denied += 1,
        std::io::ErrorKind::NotFound => return,
        _ => outcome.skipped_in_use += 1,
    }
    if outcome.example_error.is_none() {
        outcome.example_error = Some(format!(
            "{}: {err}",
            crate::core::identity::shorten_for_display(path.as_os_str())
        ));
    }
}

#[cfg(windows)]
mod imp {
    use super::{Outcome, Target, TargetKind};
    use std::path::PathBuf;

    fn env_dir(key: &str) -> String {
        std::env::var(key).unwrap_or_default()
    }

    fn under(base: &str, rest: &str) -> String {
        if base.is_empty() {
            return String::new();
        }
        PathBuf::from(base).join(rest).to_string_lossy().to_string()
    }

    /// A Chromium browser's HTTP cache.
    ///
    /// Only the `Cache` folder, never the profile beside it: cookies, saved
    /// passwords, history and extensions live one level up and are none of
    /// this feature's business. Clearing a cache does not sign you out.
    fn chromium(kind: TargetKind, title: &'static str, local: &str, vendor: &str) -> Target {
        Target {
            kind,
            key: kind.key(),
            title,
            description: "Cached pages and images for the browser's default profile. \
                          Sites re-download what they need; you stay signed in.",
            path: under(local, &format!(r"{vendor}\User Data\Default\Cache")),
            needs_admin: false,
            default_on: false,
            caveat: Some(
                "Pages you visit often will load a little slower the first time                  afterwards. Close the browser first, or most of this will be                  locked and skipped. Only the default profile is covered.",
            ),
        }
    }

    pub fn targets() -> Vec<Target> {
        let local = env_dir("LOCALAPPDATA");
        let windows = {
            let root = env_dir("SystemRoot");
            if root.is_empty() {
                env_dir("windir")
            } else {
                root
            }
        };

        vec![
            Target {
                kind: TargetKind::UserTemp,
                key: TargetKind::UserTemp.key(),
                title: "Your temporary files",
                description:
                    "What %TEMP% has accumulated: installer leftovers, half-finished \
                     extracts, scratch files programs forgot to remove. Usually the \
                     biggest single win.",
                path: env_dir("TEMP"),
                needs_admin: false,
                default_on: true,
                caveat: None,
            },
            Target {
                kind: TargetKind::WindowsTemp,
                key: TargetKind::WindowsTemp.key(),
                title: "System temporary files",
                description: "The same thing, for programs running as the system rather \
                              than as you.",
                path: under(&windows, "Temp"),
                needs_admin: true,
                default_on: true,
                caveat: None,
            },
            Target {
                kind: TargetKind::WindowsUpdateCache,
                key: TargetKind::WindowsUpdateCache.key(),
                title: "Windows Update downloads",
                description: "Update packages already installed. Windows re-downloads \
                              anything it still needs.",
                path: under(&windows, "SoftwareDistribution\\Download"),
                needs_admin: true,
                default_on: true,
                caveat: None,
            },
            Target {
                kind: TargetKind::DeliveryOptimization,
                key: TargetKind::DeliveryOptimization.key(),
                title: "Delivery Optimization cache",
                description: "Update data Windows keeps to share with other PCs on your \
                              network.",
                path: under(&windows, "SoftwareDistribution\\DeliveryOptimization"),
                needs_admin: true,
                default_on: true,
                caveat: None,
            },
            Target {
                kind: TargetKind::CrashDumps,
                key: TargetKind::CrashDumps.key(),
                title: "Crash dumps",
                description: "Memory dumps written when a program crashed. Only useful \
                              to a developer debugging that exact crash.",
                path: under(&local, "CrashDumps"),
                needs_admin: false,
                default_on: true,
                caveat: None,
            },
            Target {
                kind: TargetKind::ErrorReports,
                key: TargetKind::ErrorReports.key(),
                title: "Error reports",
                description: "Queued Windows Error Reporting data.",
                path: under(&local, "Microsoft\\Windows\\WER"),
                needs_admin: false,
                default_on: true,
                caveat: None,
            },
            Target {
                kind: TargetKind::RecycleBin,
                key: TargetKind::RecycleBin.key(),
                title: "Recycle Bin",
                description: "Files you already deleted. This is the space they are \
                              still holding.",
                path: String::new(),
                needs_admin: false,
                default_on: false,
                caveat: Some(
                    "Emptying it is permanent - check there is nothing in there you want \
                     back first. This also discards anything Cleanup trashed, so those \
                     operations can no longer be undone.",
                ),
            },
            Target {
                kind: TargetKind::Prefetch,
                key: TargetKind::Prefetch.key(),
                title: "Prefetch data",
                description: "Windows' record of how programs start, used to launch them \
                              faster.",
                path: under(&windows, "Prefetch"),
                needs_admin: true,
                default_on: false,
                caveat: Some(
                    "Your next few app launches will be slightly slower while Windows \
                     rebuilds this. It is usually only a few hundred MB.",
                ),
            },
            chromium(TargetKind::ChromeCache, "Chrome cache", &local, r"Google\Chrome"),
            chromium(TargetKind::EdgeCache, "Edge cache", &local, r"Microsoft\Edge"),
            chromium(
                TargetKind::BraveCache,
                "Brave cache",
                &local,
                r"BraveSoftware\Brave-Browser",
            ),
            Target {
                kind: TargetKind::ThumbnailCache,
                key: TargetKind::ThumbnailCache.key(),
                title: "Thumbnail cache",
                description: "Explorer's cached picture thumbnails.",
                path: under(&local, "Microsoft\\Windows\\Explorer"),
                needs_admin: false,
                default_on: false,
                caveat: Some(
                    "Explorer rebuilds these as you browse, so folders full of photos \
                     will redraw slowly for a while. Most of these files are usually \
                     locked by Explorer and will be skipped.",
                ),
            },
        ]
    }

    /// Bytes and item count in the Recycle Bin across all drives.
    pub fn recycle_bin_size() -> (u64, u64, bool) {
        use windows_sys::Win32::UI::Shell::{SHQueryRecycleBinW, SHQUERYRBINFO};

        let mut info = SHQUERYRBINFO {
            cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32,
            i64Size: 0,
            i64NumItems: 0,
        };
        // A null path means every drive.
        let hr = unsafe { SHQueryRecycleBinW(std::ptr::null(), &mut info) };
        if hr != 0 {
            return (0, 0, false);
        }
        (info.i64Size as u64, info.i64NumItems as u64, true)
    }

    pub fn empty_recycle_bin(outcome: &mut Outcome) {
        use windows_sys::Win32::UI::Shell::{
            SHEmptyRecycleBinW, SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI, SHERB_NOSOUND,
        };

        // Measure first: the API reports nothing about what it freed, and a
        // number we did not measure is a number we should not print.
        let (before, items, ok) = recycle_bin_size();
        if !ok {
            outcome.example_error = Some("the Recycle Bin could not be read".into());
            return;
        }

        // Our own confirmation already happened, in a dialog that said this is
        // permanent - a second system prompt would be the app not trusting
        // what it just told the user.
        let hr = unsafe {
            SHEmptyRecycleBinW(
                std::ptr::null_mut(),
                std::ptr::null(),
                SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND,
            )
        };
        if hr != 0 {
            outcome.skipped_denied += items;
            outcome.example_error = Some(format!("the Recycle Bin could not be emptied (0x{hr:08x})"));
            return;
        }

        let (after, _, _) = recycle_bin_size();
        outcome.freed_bytes = before.saturating_sub(after);
        outcome.removed_files = items;
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{Outcome, Target, TargetKind};

    fn home(rest: &str) -> String {
        dirs::home_dir()
            .map(|h| h.join(rest).to_string_lossy().to_string())
            .unwrap_or_default()
    }

    pub fn targets() -> Vec<Target> {
        vec![
            Target {
                kind: TargetKind::MacUserCaches,
                key: TargetKind::MacUserCaches.key(),
                title: "Application caches",
                description: "~/Library/Caches. Apps rebuild what they still need.",
                path: home("Library/Caches"),
                needs_admin: false,
                default_on: true,
                caveat: Some(
                    "Some apps will be slower the first time you open them afterwards.",
                ),
            },
            Target {
                kind: TargetKind::MacUserLogs,
                key: TargetKind::MacUserLogs.key(),
                title: "Application logs",
                description: "~/Library/Logs.",
                path: home("Library/Logs"),
                needs_admin: false,
                default_on: true,
                caveat: None,
            },
            Target {
                kind: TargetKind::UserTemp,
                key: TargetKind::UserTemp.key(),
                title: "Temporary files",
                description: "This session's temporary directory.",
                path: std::env::temp_dir().to_string_lossy().to_string(),
                needs_admin: false,
                default_on: true,
                caveat: None,
            },
        ]
    }

    pub fn recycle_bin_size() -> (u64, u64, bool) {
        (0, 0, false)
    }

    pub fn empty_recycle_bin(outcome: &mut Outcome) {
        outcome.example_error =
            Some("emptying the Trash is not implemented on this platform".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);

    impl Temp {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "cleanup-qc-{tag}-{}",
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Temp(dir)
        }
        fn file(&self, rel: &str, bytes: usize) -> PathBuf {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, vec![7u8; bytes]).unwrap();
            path
        }
        /// Backdate past the in-use grace window, so the fixture is not
        /// mistaken for a file something is still writing.
        fn aged(&self, rel: &str, bytes: usize) -> PathBuf {
            let path = self.file(rel, bytes);
            let old = SystemTime::now() - std::time::Duration::from_secs(IN_USE_GRACE_SECS * 10);
            let file = std::fs::File::options().write(true).open(&path).unwrap();
            file.set_modified(old).unwrap();
            path
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fake_target(path: &Path) -> Target {
        Target {
            kind: TargetKind::UserTemp,
            key: "userTemp",
            title: "test",
            description: "test",
            path: path.to_string_lossy().to_string(),
            needs_admin: false,
            default_on: true,
            caveat: None,
        }
    }

    #[test]
    fn every_target_has_a_distinct_key_the_ui_can_send_back() {
        let targets = targets();
        assert!(!targets.is_empty());
        let mut keys: Vec<&str> = targets.iter().map(|t| t.key).collect();
        keys.sort();
        let count = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), count, "keys must be unique");

        for target in targets {
            assert_eq!(
                target_for(target.key).map(|t| t.kind),
                Some(target.kind),
                "every listed target must round-trip through its key"
            );
        }
    }

    #[test]
    fn an_unknown_key_cleans_nothing() {
        // The closed list is the safety property: a key that is not in it must
        // not resolve to a path at all.
        assert!(target_for("../../../Windows").is_none());
        assert!(target_for("").is_none());
        let outcome = clean("c:\\windows\\system32");
        assert_eq!(outcome.freed_bytes, 0);
        assert_eq!(outcome.removed_files, 0);
    }

    #[test]
    fn measuring_counts_the_tree_without_changing_it() {
        let t = Temp::new("measure");
        t.aged("a.tmp", 1000);
        t.aged("nested/b.tmp", 2000);

        let m = measure(fake_target(&t.0));
        assert_eq!(m.bytes, 3000);
        assert_eq!(m.files, 2);
        assert!(!m.missing);
        assert!(t.0.join("a.tmp").exists(), "measuring must not delete");
    }

    #[test]
    fn cleaning_empties_the_folder_but_keeps_the_folder_itself() {
        // Removing %TEMP% itself breaks programs that expect it to exist.
        let t = Temp::new("clean");
        t.aged("a.tmp", 1000);
        t.aged("nested/b.tmp", 2000);

        let outcome = clean_at(&t.0);
        assert_eq!(outcome.freed_bytes, 3000);
        assert_eq!(outcome.removed_files, 2);
        assert_eq!(outcome.removed_dirs, 1);
        assert!(t.0.is_dir(), "the target directory must survive");
        assert_eq!(std::fs::read_dir(&t.0).unwrap().count(), 0);
    }

    #[test]
    fn a_file_written_moments_ago_is_left_alone() {
        let t = Temp::new("inuse");
        t.file("fresh.tmp", 500); // not backdated
        t.aged("old.tmp", 500);

        let outcome = clean_at(&t.0);
        assert_eq!(outcome.skipped_in_use, 1);
        assert_eq!(outcome.removed_files, 1);
        assert_eq!(outcome.freed_bytes, 500, "only the removed file counts");
        assert!(t.0.join("fresh.tmp").exists());
        assert!(!t.0.join("old.tmp").exists());
    }

    #[test]
    fn a_missing_folder_is_reported_rather_than_counted_as_empty() {
        let mut target = fake_target(Path::new(""));
        target.path = std::env::temp_dir()
            .join("cleanup-qc-definitely-absent-xyz")
            .to_string_lossy()
            .to_string();
        let m = measure(target);
        assert!(m.missing);
        assert_eq!(m.bytes, 0);
    }

    #[test]
    fn a_pattern_target_leaves_everything_else_alone() {
        // The Explorer folder holds real settings next to the thumbnail cache.
        let t = Temp::new("patterned");
        t.aged("thumbcache_256.db", 900);
        t.aged("settings.dat", 400);

        let mut outcome = Outcome::default();
        clean_dir(
            &t.0,
            scope_of(TargetKind::ThumbnailCache),
            0,
            true,
            &mut outcome,
        );

        assert_eq!(outcome.removed_files, 1);
        assert_eq!(outcome.freed_bytes, 900);
        assert!(
            t.0.join("settings.dat").exists(),
            "a file outside the pattern must not be touched"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_is_removed_without_following_it() {
        let t = Temp::new("link");
        let outside = Temp::new("link-target");
        let kept = outside.aged("precious.txt", 100);
        std::os::unix::fs::symlink(&outside.0, t.0.join("escape")).unwrap();

        let outcome = clean_at(&t.0);
        assert!(!t.0.join("escape").exists(), "the link itself goes");
        assert!(kept.exists(), "what it pointed at must be untouched");
        assert_eq!(outcome.freed_bytes, 0, "a link frees nothing");
    }

    /// `clean` resolves its own path from the closed list, so a test against a
    /// temp directory drives the walk directly.
    fn clean_at(dir: &Path) -> Outcome {
        let mut outcome = Outcome::default();
        clean_dir(dir, Scope::Contents, 0, true, &mut outcome);
        outcome
    }
}
