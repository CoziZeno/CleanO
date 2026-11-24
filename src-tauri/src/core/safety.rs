//! Path safety.
//!
//! In Phase 2 this validates scan roots. Phase 4 adds the mutation guard
//! (`guard()`), and both use the same denylist, so an operation can never be
//! permitted against a path a scan would have refused to open.
//!
//! Rules, in order, because order is the whole point:
//!
//! 1. Canonicalise **first**. Checking a raw path for forbidden prefixes is how
//!    traversal bugs happen: `~/Downloads/evil/../../../System` only looks safe
//!    before resolution, and a symlink looks safe even after textual analysis.
//! 2. Reject volume roots. Scanning `C:\` or `/` is "scan my whole computer",
//!    which must be a deliberate, separate act - not something a stray click
//!    in a folder picker can start.
//! 3. Reject the system denylist. The app has no business in OS or application
//!    directories, and a user cannot grant it that business by accident.

use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SafetyError {
    NotFound(String),
    NotADirectory(String),
    /// A symlink, junction or other reparse point. Never operated on, because
    /// its target can be anywhere - including outside every approved root.
    NotAFile(String),
    VolumeRoot(String),
    SystemPath(String),
    OutsideApprovedRoots(String),
    /// Inside a folder the user told Cleanup to leave alone.
    Excluded(String),
    Unreadable { path: String, reason: String },
}

/// Where the app is allowed to act: the folders the user approved, minus the
/// folders they told it to keep out of.
///
/// Exclusion cannot be expressed by editing the approved list - "all of
/// `E:\Projects` except `E:\Projects\Unity`" is not a set of prefixes - so it
/// is carried alongside and checked by the same guard. Both lists hold
/// **canonical** paths, because a prefix comparison against anything else is
/// exactly the bug the guard exists to prevent.
#[derive(Clone, Debug, Default)]
pub struct Scope {
    approved: Vec<PathBuf>,
    excluded: Vec<PathBuf>,
}

impl Scope {
    pub fn new(approved: Vec<PathBuf>, excluded: Vec<PathBuf>) -> Self {
        Self { approved, excluded }
    }

    /// A scope with nothing excluded.
    pub fn of(approved: Vec<PathBuf>) -> Self {
        Self {
            approved,
            excluded: Vec::new(),
        }
    }

    pub fn approved(&self) -> &[PathBuf] {
        &self.approved
    }

    pub fn excluded(&self) -> &[PathBuf] {
        &self.excluded
    }

    /// Is this canonical path the excluded folder itself, or inside it?
    ///
    /// `starts_with` compares whole components, so excluding `E:\Unity` does
    /// not also exclude `E:\Unity-Backup`.
    pub fn is_excluded(&self, canonical: &Path) -> bool {
        self.excluded.iter().any(|dir| canonical.starts_with(dir))
    }
}

impl std::fmt::Display for SafetyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SafetyError::NotFound(p) => write!(f, "{p} does not exist"),
            SafetyError::NotADirectory(p) => write!(f, "{p} is not a folder"),
            SafetyError::NotAFile(p) => write!(
                f,
                "{p} is a shortcut or link rather than a real file, so Cleanup will not touch it."
            ),
            SafetyError::VolumeRoot(p) => write!(
                f,
                "{p} is the root of a drive. Scanning a whole drive is not \
                 something Cleanup will start from a folder picker - choose a \
                 folder inside it instead."
            ),
            SafetyError::SystemPath(p) => write!(
                f,
                "{p} is a system or application folder. Cleanup never reads or \
                 modifies these."
            ),
            SafetyError::OutsideApprovedRoots(p) => {
                write!(f, "{p} is outside every folder you have approved")
            }
            SafetyError::Excluded(p) => write!(
                f,
                "{p} is inside a folder you told Cleanup to leave alone, so                  nothing there will be touched."
            ),
            SafetyError::Unreadable { path, reason } => {
                write!(f, "{path} could not be opened: {reason}")
            }
        }
    }
}

impl std::error::Error for SafetyError {}

#[cfg(windows)]
const SYSTEM_SEGMENTS: &[&str] = &[
    r"\windows",
    r"\program files",
    r"\program files (x86)",
    r"\programdata",
    r"\$recycle.bin",
    r"\system volume information",
    r"\recovery",
    r"\appdata\local\microsoft\windows",
    r"\appdata\local\packages",
    r"\appdata\locallow",
];

#[cfg(target_os = "macos")]
const SYSTEM_SEGMENTS: &[&str] = &[
    "/system",
    "/library",
    "/usr",
    "/bin",
    "/sbin",
    "/private",
    "/applications",
    "/cores",
    "/opt",
    "/volumes/macintosh hd/system",
    "/library/caches",
];

#[cfg(all(unix, not(target_os = "macos")))]
const SYSTEM_SEGMENTS: &[&str] = &[
    "/usr", "/bin", "/sbin", "/lib", "/lib64", "/etc", "/boot", "/proc", "/sys", "/dev", "/run",
    "/var",
];

/// Paths relative to the user's home that are off limits even though the home
/// directory itself is a legitimate place to scan.
#[cfg(target_os = "macos")]
const HOME_RELATIVE_DENY: &[&str] = &["library"];

#[cfg(windows)]
const HOME_RELATIVE_DENY: &[&str] = &["appdata"];

#[cfg(all(unix, not(target_os = "macos")))]
const HOME_RELATIVE_DENY: &[&str] = &[".config", ".local/share", ".cache"];

/// Carve-outs from `HOME_RELATIVE_DENY`, checked first.
///
/// `AppData` holds application state and is denied wholesale - except the
/// user's own temp folder, which is a legitimate thing to want cleaned up and
/// is where Windows puts `std::env::temp_dir()`. Denying it would make a
/// cleanup tool refuse one of the folders people most want cleaned.
#[cfg(windows)]
const HOME_RELATIVE_ALLOW: &[&str] = &["appdata\\local\\temp"];

#[cfg(target_os = "macos")]
const HOME_RELATIVE_ALLOW: &[&str] = &[];

#[cfg(all(unix, not(target_os = "macos")))]
const HOME_RELATIVE_ALLOW: &[&str] = &[];

/// True when `path` is, or is inside, a directory the app must never touch.
///
/// Expects an already-canonicalised path.
pub fn is_system_path(path: &Path) -> bool {
    let text = path.to_string_lossy().to_ascii_lowercase();
    let normalised = if cfg!(windows) {
        text.replace('/', "\\")
    } else {
        text
    };

    // Strip the drive specifier on Windows so `C:\Windows` and `D:\Windows`
    // are both caught by a single `\windows` rule.
    let tail = if cfg!(windows) {
        match normalised.find(':') {
            Some(idx) => normalised[idx + 1..].to_string(),
            None => normalised.clone(),
        }
    } else {
        normalised.clone()
    };

    let sep = if cfg!(windows) { '\\' } else { '/' };
    for deny in SYSTEM_SEGMENTS {
        if tail == *deny || tail.starts_with(&format!("{deny}{sep}")) {
            return true;
        }
    }

    if let Some(home) = dirs::home_dir() {
        let home_text = home.to_string_lossy().to_ascii_lowercase();
        let home_text = if cfg!(windows) {
            home_text.replace('/', "\\")
        } else {
            home_text
        };
        if let Some(rest) = normalised.strip_prefix(&home_text) {
            let rest = rest.trim_start_matches(sep);
            // Exceptions win over the denylist.
            for allow in HOME_RELATIVE_ALLOW {
                let allow = allow.replace('/', &sep.to_string());
                if rest == allow || rest.starts_with(&format!("{allow}{sep}")) {
                    return false;
                }
            }
            for deny in HOME_RELATIVE_DENY {
                let deny = deny.replace('/', &sep.to_string());
                if rest == deny || rest.starts_with(&format!("{deny}{sep}")) {
                    return true;
                }
            }
        }
    }

    false
}

/// True when the path has no component beyond its prefix/root - `C:\`, `\\srv\share`, `/`.
pub fn is_volume_root(path: &Path) -> bool {
    let mut saw_normal = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => saw_normal = true,
            Component::CurDir | Component::ParentDir => saw_normal = true,
            Component::Prefix(_) | Component::RootDir => {}
        }
    }
    !saw_normal
}

/// Validate a scan root, returning the canonical path to use.
pub fn validate_root(requested: &Path) -> Result<PathBuf, SafetyError> {
    let shown = requested.to_string_lossy().to_string();

    let canonical = match std::fs::canonicalize(requested) {
        Ok(p) => crate::core::identity::strip_verbatim(p),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(SafetyError::NotFound(shown))
        }
        Err(err) => {
            return Err(SafetyError::Unreadable {
                path: shown,
                reason: err.to_string(),
            })
        }
    };

    let metadata = std::fs::symlink_metadata(&canonical).map_err(|err| SafetyError::Unreadable {
        path: canonical.to_string_lossy().to_string(),
        reason: err.to_string(),
    })?;
    if !metadata.is_dir() {
        return Err(SafetyError::NotADirectory(
            canonical.to_string_lossy().to_string(),
        ));
    }

    if is_volume_root(&canonical) {
        return Err(SafetyError::VolumeRoot(
            canonical.to_string_lossy().to_string(),
        ));
    }
    if is_system_path(&canonical) {
        return Err(SafetyError::SystemPath(
            canonical.to_string_lossy().to_string(),
        ));
    }

    Ok(canonical)
}

/// Is `candidate` inside one of the roots the user has approved?
/// Both sides must already be canonical.
pub fn is_within_approved(candidate: &Path, approved: &[PathBuf]) -> bool {
    approved.iter().any(|root| candidate.starts_with(root))
}

/// The mutation guard. **Every** operation that moves or trashes a file passes
/// through here, and there is no second code path.
///
/// Order is the whole point:
///
/// 1. **Canonicalise first.** Checking a raw path for forbidden prefixes is
///    how traversal bugs happen - `Downloads/x/../../../Windows` only looks
///    safe before resolution, and a symlink looks safe even after textual
///    analysis.
/// 2. Reject the path if it *is* a link. We never operate on symlinks or
///    reparse points; the target may be anywhere.
/// 3. Volume roots, then the system denylist.
/// 4. The approved-root allowlist, which the user controls in Settings.
///
/// This runs again at execution time, not only when the plan was built. The
/// filesystem can change between preview and apply, and defence in depth is
/// almost free here.
pub fn guard_source(path: &Path, scope: &Scope) -> Result<PathBuf, SafetyError> {
    let shown = path.to_string_lossy().to_string();

    // lstat first: a symlink must be rejected as itself, before canonicalize
    // resolves it to something that would pass every later check.
    let link_meta = std::fs::symlink_metadata(crate::core::identity::for_io(path)).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            SafetyError::NotFound(shown.clone())
        } else {
            SafetyError::Unreadable {
                path: shown.clone(),
                reason: err.to_string(),
            }
        }
    })?;
    if link_meta.file_type().is_symlink() {
        return Err(SafetyError::NotAFile(shown));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if link_meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(SafetyError::NotAFile(shown));
        }
    }

    let canonical = canonicalize_checked(path)?;
    check_location(&canonical, scope)?;
    Ok(canonical)
}

/// Guard a path that does not exist yet - a move destination.
///
/// The parent must exist and pass every check; the final component is then
/// appended. Canonicalising the parent is what stops a symlinked destination
/// folder being used to write outside the approved roots.
pub fn guard_destination(path: &Path, scope: &Scope) -> Result<PathBuf, SafetyError> {
    let shown = path.to_string_lossy().to_string();
    let parent = path.parent().ok_or_else(|| SafetyError::NotFound(shown.clone()))?;
    let name = path
        .file_name()
        .ok_or_else(|| SafetyError::NotFound(shown.clone()))?;

    let canonical_parent = canonicalize_checked(parent)?;
    check_location(&canonical_parent, scope)?;

    let full = canonical_parent.join(name);
    // The joined path must still be inside the approved root: a `..` in the
    // final component would otherwise escape.
    check_location(&full, scope)?;
    Ok(full)
}

/// A directory we are about to create. Only the nearest existing ancestor can
/// be canonicalised, so that is what gets checked.
pub fn guard_new_dir(path: &Path, scope: &Scope) -> Result<PathBuf, SafetyError> {
    if path.exists() {
        return guard_source(path, scope);
    }
    guard_destination(path, scope)
}

fn canonicalize_checked(path: &Path) -> Result<PathBuf, SafetyError> {
    let shown = path.to_string_lossy().to_string();
    match std::fs::canonicalize(crate::core::identity::for_io(path)) {
        Ok(p) => Ok(crate::core::identity::strip_verbatim(p)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Err(SafetyError::NotFound(shown)),
        Err(err) => Err(SafetyError::Unreadable {
            path: shown,
            reason: err.to_string(),
        }),
    }
}

fn check_location(path: &Path, scope: &Scope) -> Result<(), SafetyError> {
    if is_volume_root(path) {
        return Err(SafetyError::VolumeRoot(path.to_string_lossy().to_string()));
    }
    if is_system_path(path) {
        return Err(SafetyError::SystemPath(path.to_string_lossy().to_string()));
    }
    // The user's own "never touch this" comes before the allowlist, so a
    // folder they excluded is refused with that reason even when it sits
    // inside a root they approved - which is the whole point of excluding it.
    if scope.is_excluded(path) {
        return Err(SafetyError::Excluded(path.to_string_lossy().to_string()));
    }
    if !is_within_approved(path, scope.approved()) {
        return Err(SafetyError::OutsideApprovedRoots(
            path.to_string_lossy().to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_roots_are_recognised() {
        if cfg!(windows) {
            assert!(is_volume_root(Path::new(r"C:\")));
            assert!(!is_volume_root(Path::new(r"C:\Users")));
        } else {
            assert!(is_volume_root(Path::new("/")));
            assert!(!is_volume_root(Path::new("/home")));
        }
    }

    #[test]
    fn system_directories_are_denied() {
        if cfg!(windows) {
            assert!(is_system_path(Path::new(r"C:\Windows")));
            assert!(is_system_path(Path::new(r"C:\Windows\System32\drivers")));
            assert!(is_system_path(Path::new(r"D:\Program Files\Thing")));
            assert!(is_system_path(Path::new(r"C:\$Recycle.Bin")));
            assert!(!is_system_path(Path::new(r"C:\Users\someone\Downloads")));
        } else {
            assert!(is_system_path(Path::new("/usr/local/bin")));
            assert!(is_system_path(Path::new("/etc")));
            assert!(!is_system_path(Path::new("/home/someone/Downloads")));
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_system_and_application_paths_are_denied() {
        assert!(is_system_path(Path::new("/System/Library")));
        assert!(is_system_path(Path::new("/Applications/Safari.app")));
        assert!(is_system_path(Path::new("/private/var/db")));
    }

    #[test]
    fn the_home_library_or_appdata_is_denied_even_though_home_is_not() {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        assert!(!is_system_path(&home));
        let protected = if cfg!(windows) {
            home.join("AppData").join("Local")
        } else if cfg!(target_os = "macos") {
            home.join("Library").join("Caches")
        } else {
            home.join(".cache")
        };
        assert!(is_system_path(&protected), "{protected:?} must be denied");
    }

    #[cfg(windows)]
    #[test]
    fn the_users_own_temp_folder_is_allowed_despite_living_under_appdata() {
        // AppData is denied wholesale, but Temp is one of the folders people
        // most want cleaned, so it is an explicit carve-out.
        let temp = std::env::temp_dir();
        assert!(
            !is_system_path(&temp),
            "{temp:?} must be usable, it is the user's temp folder"
        );
        // The rest of AppData stays off limits.
        if let Some(home) = dirs::home_dir() {
            assert!(is_system_path(&home.join("AppData").join("Roaming")));
            assert!(is_system_path(&home.join("AppData").join("Local").join("Microsoft")));
        }
    }

    #[test]
    fn a_missing_root_is_rejected_by_name() {
        let missing = std::env::temp_dir().join("cleanup-no-such-root-98765");
        assert_eq!(
            validate_root(&missing),
            Err(SafetyError::NotFound(
                missing.to_string_lossy().to_string()
            ))
        );
    }

    #[test]
    fn a_file_is_not_a_valid_root() {
        let path = std::env::temp_dir().join("cleanup-root-is-a-file.txt");
        std::fs::write(&path, b"x").unwrap();
        assert!(matches!(
            validate_root(&path),
            Err(SafetyError::NotADirectory(_))
        ));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_volume_root_is_rejected() {
        let root = Path::new(if cfg!(windows) { r"C:\" } else { "/" });
        assert!(matches!(
            validate_root(root),
            Err(SafetyError::VolumeRoot(_))
        ));
    }

    #[test]
    fn a_system_directory_is_rejected_as_a_root() {
        let system = Path::new(if cfg!(windows) {
            r"C:\Windows"
        } else if cfg!(target_os = "macos") {
            "/System"
        } else {
            "/etc"
        });
        if !system.exists() {
            return;
        }
        assert!(matches!(
            validate_root(system),
            Err(SafetyError::SystemPath(_))
        ));
    }

    #[test]
    fn approved_root_containment() {
        let base = std::env::temp_dir().join("cleanup-approved");
        let inside = base.join("sub").join("file.txt");
        let outside = std::env::temp_dir().join("cleanup-elsewhere").join("f.txt");
        let approved = vec![base.clone()];
        assert!(is_within_approved(&inside, &approved));
        assert!(!is_within_approved(&outside, &approved));
    }

    // ------------------------------------------------- the mutation guard ---

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cleanup-guard-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(&dir)
            .map(crate::core::identity::strip_verbatim)
            .unwrap_or(dir)
    }

    #[test]
    fn guard_accepts_a_real_file_inside_an_approved_root() {
        let root = scratch("ok");
        let file = root.join("report.pdf");
        std::fs::write(&file, b"x").unwrap();

        assert!(guard_source(&file, &Scope::of(vec![root.clone()])).is_ok());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn guard_refuses_a_file_outside_every_approved_root() {
        let root = scratch("in");
        let other = scratch("out");
        let stray = other.join("not-yours.txt");
        std::fs::write(&stray, b"x").unwrap();

        // The file is perfectly real and readable. It is simply not ours.
        assert!(matches!(
            guard_source(&stray, &Scope::of(vec![root.clone()])),
            Err(SafetyError::OutsideApprovedRoots(_))
        ));
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&other).ok();
    }

    #[test]
    fn guard_refuses_an_empty_allowlist() {
        let root = scratch("noroots");
        let file = root.join("f.txt");
        std::fs::write(&file, b"x").unwrap();
        // No approved roots means no permission to touch anything at all.
        assert!(guard_source(&file, &Scope::default()).is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn guard_refuses_a_destination_that_climbs_out_with_dot_dot() {
        let root = scratch("escape");
        std::fs::create_dir_all(root.join("Images")).unwrap();
        let sneaky = root.join("Images").join("..").join("..").join("escaped.txt");

        assert!(
            guard_destination(&sneaky, &Scope::of(vec![root.clone()])).is_err(),
            "a destination resolving outside the root must be refused"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn guard_accepts_a_destination_that_does_not_exist_yet() {
        let root = scratch("dest");
        std::fs::create_dir_all(root.join("Images")).unwrap();
        let target = root.join("Images").join("brand-new.png");

        let resolved = guard_destination(&target, &Scope::of(vec![root.clone()])).expect("should be allowed");
        assert!(resolved.ends_with("brand-new.png"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn guard_refuses_a_symlink_even_inside_an_approved_root() {
        let root = scratch("link");
        let real = root.join("real.txt");
        std::fs::write(&real, b"x").unwrap();
        let link = root.join("link.txt");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        // Both are inside the approved root, but one is a link and its target
        // could be anywhere.
        assert!(guard_source(&real, &Scope::of(vec![root.clone()])).is_ok());
        assert!(matches!(
            guard_source(&link, &Scope::of(vec![root.clone()])),
            Err(SafetyError::NotAFile(_))
        ));
        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn guard_refuses_a_destination_whose_parent_is_a_symlink_out_of_the_root() {
        let root = scratch("linkdir");
        let outside = scratch("elsewhere");
        std::os::unix::fs::symlink(&outside, root.join("Images")).unwrap();

        // The textual path looks like it is inside the root. It is not.
        let target = root.join("Images").join("payload.txt");
        assert!(matches!(
            guard_destination(&target, &Scope::of(vec![root.clone()])),
            Err(SafetyError::OutsideApprovedRoots(_))
        ));
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&outside).ok();
    }

    #[test]
    fn an_excluded_folder_is_refused_even_inside_an_approved_root() {
        // The case this exists for: a whole drive approved for scanning, with
        // one project folder on it that must never be touched.
        let root = scratch("excluded-root");
        let keep_out = root.join("Unity");
        std::fs::create_dir_all(&keep_out).unwrap();
        let asset = keep_out.join("Library").join("artifacts.db");
        std::fs::create_dir_all(asset.parent().unwrap()).unwrap();
        std::fs::write(&asset, b"do not touch").unwrap();
        let other = root.join("report.pdf");
        std::fs::write(&other, b"fine").unwrap();

        let scope = Scope::new(vec![root.clone()], vec![keep_out.clone()]);

        assert!(
            matches!(guard_source(&asset, &scope), Err(SafetyError::Excluded(_))),
            "a file deep inside an excluded folder must be refused"
        );
        assert!(
            matches!(guard_source(&keep_out, &scope), Err(SafetyError::Excluded(_))),
            "the excluded folder itself must be refused"
        );
        assert!(
            guard_source(&other, &scope).is_ok(),
            "the rest of the approved root stays usable"
        );
        assert!(
            matches!(
                guard_destination(&keep_out.join("new.txt"), &scope),
                Err(SafetyError::Excluded(_))
            ),
            "and nothing may be written into it either"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn excluding_a_folder_does_not_exclude_its_lookalike_sibling() {
        // Prefix matching on strings would make "Unity" also exclude
        // "Unity-Backup". Component matching does not.
        let root = scratch("excluded-sibling");
        let keep_out = root.join("Unity");
        let sibling = root.join("Unity-Backup");
        std::fs::create_dir_all(&keep_out).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        let file = sibling.join("notes.txt");
        std::fs::write(&file, b"keep").unwrap();

        let scope = Scope::new(vec![root.clone()], vec![keep_out]);
        assert!(guard_source(&file, &scope).is_ok());

        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_root_resolves_before_the_denylist_runs() {
        // The attack this prevents: a link whose name looks harmless but which
        // points at a protected directory. Canonicalising first means the
        // denylist sees the real destination.
        let dir = std::env::temp_dir().join(format!(
            "cleanup-symlink-root-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let link = dir.join("innocent-looking-folder");
        std::os::unix::fs::symlink("/usr", &link).unwrap();

        assert!(matches!(validate_root(&link), Err(SafetyError::SystemPath(_))));
        std::fs::remove_dir_all(&dir).ok();
    }
}
