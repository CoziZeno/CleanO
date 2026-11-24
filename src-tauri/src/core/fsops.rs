//! The only code in this project that modifies a user's files.
//!
//! Every function here takes the approved-root list and runs it through
//! `safety::guard_*` before touching anything. There is no bypass, and no
//! `remove_file` on a user's file anywhere in the codebase - deletion always
//! goes to the OS Trash, which means the user can always get it back.
//!
//! Three rules, each of which exists because of a specific way this goes wrong:
//!
//! * **Never overwrite.** If the destination name is taken, a non-colliding
//!   one is chosen and reported. No code path passes an existing destination
//!   to `rename`.
//! * **Cross-volume moves copy, verify, then trash the source** - never
//!   `remove_file`. A move that half-succeeded must still leave the data
//!   recoverable.
//! * **Refuse rather than permanently delete.** The Windows Recycle Bin does
//!   not exist on network shares or most removable drives: "delete" there is
//!   forever. We check first and refuse, instead of silently destroying data.

use crate::core::identity;
use crate::core::safety::{self, SafetyError};
use crate::core::suggest::resolve_destination;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum OpError {
    Safety(SafetyError),
    /// The volume has no Recycle Bin / Trash, so deleting would be permanent.
    NoRecycleBin { path: String, reason: String },
    /// Windows: the file is open in another program.
    InUse { path: String },
    Io { path: String, message: String },
    /// A cross-volume copy did not produce an identical-size file.
    VerificationFailed { path: String },
    /// A user-supplied name that cannot be used.
    InvalidName(String),
    /// Renaming to a name already in use. Refused rather than auto-renamed.
    NameTaken(String),
}

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpError::Safety(err) => write!(f, "{err}"),
            OpError::NoRecycleBin { path, reason } => write!(
                f,
                "{path} is on a volume with no Recycle Bin ({reason}). Deleting there would be \
                 permanent and cannot be undone, so Cleanup will not do it."
            ),
            OpError::InUse { path } => {
                write!(f, "{path} is open in another program and cannot be moved.")
            }
            OpError::Io { path, message } => write!(f, "{path}: {message}"),
            OpError::VerificationFailed { path } => write!(
                f,
                "{path} was copied but the copy did not match. The original has been left alone."
            ),
            OpError::InvalidName(reason) => write!(f, "{reason}"),
            OpError::NameTaken(name) => {
                write!(f, "\"{name}\" is already taken in that folder.")
            }
        }
    }
}

impl std::error::Error for OpError {}

impl From<SafetyError> for OpError {
    fn from(err: SafetyError) -> Self {
        OpError::Safety(err)
    }
}

fn io_error(path: &Path, err: &std::io::Error) -> OpError {
    let shown = identity::shorten_for_display(path.as_os_str());
    // ERROR_SHARING_VIOLATION (32) / ERROR_LOCK_VIOLATION (33). Windows
    // refuses to move a file another process has open; macOS does not. This
    // is a normal outcome there, not a bug, and deserves a plain explanation.
    #[cfg(windows)]
    if matches!(err.raw_os_error(), Some(32) | Some(33)) {
        return OpError::InUse { path: shown };
    }
    OpError::Io {
        path: shown,
        message: err.to_string(),
    }
}

#[derive(Debug, Clone)]
pub struct MoveOutcome {
    pub from: PathBuf,
    pub to: PathBuf,
    /// Set when the obvious name was taken and a different one was used.
    pub renamed_to: Option<String>,
    /// True when the move crossed volumes and therefore copied rather than
    /// renamed.
    pub cross_volume: bool,
}

/// Move a file into `to_dir`, creating that directory if needed.
///
/// Returns where it actually landed. Never overwrites.
pub fn move_into(
    from: &Path,
    to_dir: &Path,
    approved: &safety::Scope,
) -> Result<MoveOutcome, OpError> {
    let source = safety::guard_source(from, approved)?;

    // Create the destination folder first so it can be canonicalised and
    // guarded like any other path.
    let dir = safety::guard_new_dir(to_dir, approved)?;
    if !dir.exists() {
        std::fs::create_dir_all(identity::for_io(&dir)).map_err(|e| io_error(&dir, &e))?;
    }
    let dir = safety::guard_source(&dir, approved)?;

    let name = source
        .file_name()
        .ok_or_else(|| OpError::Io {
            path: identity::shorten_for_display(source.as_os_str()),
            message: "the file has no name".into(),
        })?
        .to_os_string();

    let (target, renamed_to) = resolve_destination(&dir, &name);
    let target = safety::guard_destination(&target, approved)?;

    // Moving a file onto itself is a no-op, not an error.
    if target == source {
        return Ok(MoveOutcome {
            from: source.clone(),
            to: source,
            renamed_to: None,
            cross_volume: false,
        });
    }

    match std::fs::rename(identity::for_io(&source), identity::for_io(&target)) {
        Ok(()) => Ok(MoveOutcome {
            from: source,
            to: target,
            renamed_to,
            cross_volume: false,
        }),
        Err(err) if is_cross_device(&err) => {
            copy_verify_trash(&source, &target, approved)?;
            Ok(MoveOutcome {
                from: source,
                to: target,
                renamed_to,
                cross_volume: true,
            })
        }
        Err(err) => Err(io_error(&source, &err)),
    }
}

/// Cross-volume move: copy, verify the size, then send the source to the Trash.
///
/// The source is trashed rather than deleted so that even a wrong verification
/// leaves the data recoverable.
fn copy_verify_trash(source: &Path, target: &Path, approved: &safety::Scope) -> Result<(), OpError> {
    let expected = std::fs::metadata(identity::for_io(source))
        .map_err(|e| io_error(source, &e))?
        .len();

    std::fs::copy(identity::for_io(source), identity::for_io(target))
        .map_err(|e| io_error(source, &e))?;

    let copied = match std::fs::metadata(identity::for_io(target)) {
        Ok(meta) => meta.len(),
        Err(err) => {
            let _ = std::fs::remove_file(identity::for_io(target));
            return Err(io_error(target, &err));
        }
    };

    if copied != expected {
        // Remove the *copy* we just made - our own partial artefact, never the
        // user's data - and leave the original untouched.
        let _ = std::fs::remove_file(identity::for_io(target));
        return Err(OpError::VerificationFailed {
            path: identity::shorten_for_display(source.as_os_str()),
        });
    }

    to_trash(source, approved)
}

#[cfg(unix)]
fn is_cross_device(err: &std::io::Error) -> bool {
    err.raw_os_error() == Some(18) // EXDEV
}

#[cfg(windows)]
fn is_cross_device(err: &std::io::Error) -> bool {
    // ERROR_NOT_SAME_DEVICE
    err.raw_os_error() == Some(17)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrashAvailability {
    Available,
    Unavailable(String),
}

/// Can this path be sent to a Recycle Bin / Trash at all?
///
/// This is the check that stops the app quietly destroying data. On Windows,
/// network shares and most removable drives have no Recycle Bin, and a delete
/// there is immediate and permanent.
pub fn trash_availability(path: &Path) -> TrashAvailability {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;

        const DRIVE_UNKNOWN: u32 = 0;
        const DRIVE_NO_ROOT_DIR: u32 = 1;
        const DRIVE_REMOVABLE: u32 = 2;
        const DRIVE_REMOTE: u32 = 4;
        const DRIVE_CDROM: u32 = 5;
        const DRIVE_RAMDISK: u32 = 6;

        // GetDriveTypeW wants a root like "C:\". A UNC path has no drive
        // letter at all, which is itself the answer.
        let text = path.to_string_lossy().replace('/', "\\");
        if text.starts_with("\\\\") {
            return TrashAvailability::Unavailable("it is on a network share".into());
        }
        let Some(colon) = text.find(':') else {
            return TrashAvailability::Unavailable("its drive could not be identified".into());
        };
        let root = format!("{}:\\", &text[..colon]);
        let wide: Vec<u16> = std::ffi::OsStr::new(&root)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: `wide` is a NUL-terminated UTF-16 string alive for the call.
        let kind = unsafe { GetDriveTypeW(wide.as_ptr()) };

        return match kind {
            DRIVE_REMOTE => TrashAvailability::Unavailable("it is on a network drive".into()),
            DRIVE_REMOVABLE => TrashAvailability::Unavailable(
                "it is on a removable drive, where Windows deletes permanently".into(),
            ),
            DRIVE_CDROM => TrashAvailability::Unavailable("it is on read-only media".into()),
            DRIVE_RAMDISK => TrashAvailability::Unavailable("it is on a RAM disk".into()),
            DRIVE_UNKNOWN | DRIVE_NO_ROOT_DIR => {
                TrashAvailability::Unavailable("its drive type is unknown".into())
            }
            _ => TrashAvailability::Available,
        };
    }

    #[cfg(not(windows))]
    {
        let _ = path;
        // macOS keeps a per-volume `.Trashes`; the crate reports failure if it
        // cannot use it, and that error is surfaced rather than worked around.
        TrashAvailability::Available
    }
}

/// Send a file to the Recycle Bin / Trash.
///
/// Refuses outright when the volume has no Trash, instead of falling back to a
/// permanent delete. That fallback is what most cleanup tools do, and it is
/// how people lose files.
pub fn to_trash(path: &Path, approved: &safety::Scope) -> Result<(), OpError> {
    let target = safety::guard_source(path, approved)?;

    if let TrashAvailability::Unavailable(reason) = trash_availability(&target) {
        return Err(OpError::NoRecycleBin {
            path: identity::shorten_for_display(target.as_os_str()),
            reason,
        });
    }

    trash::delete(&target).map_err(|err| OpError::Io {
        path: identity::shorten_for_display(target.as_os_str()),
        message: err.to_string(),
    })
}

/// Characters Windows forbids in a filename, plus the separators every
/// platform forbids. Rejecting these up front gives a clear message instead of
/// an opaque OS error.
const FORBIDDEN_IN_NAME: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|', '\0'];

/// Validate a user-supplied file or folder name.
///
/// A name is not a path: accepting one with a separator in it would let a
/// rename write somewhere else entirely.
pub fn validate_name(name: &str) -> Result<(), OpError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(OpError::InvalidName("the name cannot be empty".into()));
    }
    if trimmed == "." || trimmed == ".." {
        return Err(OpError::InvalidName(format!("\"{trimmed}\" is not a name")));
    }
    if let Some(bad) = trimmed.chars().find(|c| FORBIDDEN_IN_NAME.contains(c)) {
        return Err(OpError::InvalidName(format!(
            "a name cannot contain {bad:?}"
        )));
    }
    // Windows refuses these whatever the extension, and silently so.
    const RESERVED: &[&str] = &[
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
        "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    let stem = trimmed.split('.').next().unwrap_or(trimmed).to_lowercase();
    if RESERVED.contains(&stem.as_str()) {
        return Err(OpError::InvalidName(format!(
            "\"{trimmed}\" is a name Windows reserves"
        )));
    }
    if trimmed.ends_with('.') || trimmed.ends_with(' ') {
        return Err(OpError::InvalidName(
            "a name cannot end with a space or a dot".into(),
        ));
    }
    Ok(())
}

/// Rename a file in place.
///
/// Unlike a move, a collision here is refused rather than auto-renamed: the
/// user typed a specific name, so quietly giving them a different one would be
/// wrong. They are told and can choose again.
pub fn rename(path: &Path, new_name: &str, approved: &safety::Scope) -> Result<PathBuf, OpError> {
    validate_name(new_name)?;
    let source = safety::guard_source(path, approved)?;
    let parent = source.parent().ok_or_else(|| OpError::Io {
        path: identity::shorten_for_display(source.as_os_str()),
        message: "the file has no containing folder".into(),
    })?;

    let target = safety::guard_destination(&parent.join(new_name.trim()), approved)?;
    if target == source {
        return Ok(source);
    }
    if std::fs::symlink_metadata(identity::for_io(&target)).is_ok() {
        return Err(OpError::NameTaken(new_name.trim().to_string()));
    }

    std::fs::rename(identity::for_io(&source), identity::for_io(&target))
        .map_err(|e| io_error(&source, &e))?;
    Ok(target)
}

/// Copy a file into a folder, never overwriting.
pub fn copy_into(
    from: &Path,
    to_dir: &Path,
    approved: &safety::Scope,
) -> Result<MoveOutcome, OpError> {
    let source = safety::guard_source(from, approved)?;
    let dir = safety::guard_new_dir(to_dir, approved)?;
    if !dir.exists() {
        std::fs::create_dir_all(identity::for_io(&dir)).map_err(|e| io_error(&dir, &e))?;
    }
    let dir = safety::guard_source(&dir, approved)?;

    let name = source
        .file_name()
        .ok_or_else(|| OpError::Io {
            path: identity::shorten_for_display(source.as_os_str()),
            message: "the file has no name".into(),
        })?
        .to_os_string();

    let (target, renamed_to) = resolve_destination(&dir, &name);
    let target = safety::guard_destination(&target, approved)?;
    if target == source {
        // Copying a file into its own folder would otherwise silently do
        // nothing; give it the collision-resolved name instead.
        return Err(OpError::Io {
            path: identity::shorten_for_display(source.as_os_str()),
            message: "a file cannot be copied over itself".into(),
        });
    }

    let expected = std::fs::metadata(identity::for_io(&source))
        .map_err(|e| io_error(&source, &e))?
        .len();
    std::fs::copy(identity::for_io(&source), identity::for_io(&target))
        .map_err(|e| io_error(&source, &e))?;

    // Verify, and remove our own partial copy if it is wrong - never the
    // original.
    match std::fs::metadata(identity::for_io(&target)) {
        Ok(meta) if meta.len() == expected => Ok(MoveOutcome {
            from: source,
            to: target,
            renamed_to,
            cross_volume: false,
        }),
        _ => {
            let _ = std::fs::remove_file(identity::for_io(&target));
            Err(OpError::VerificationFailed {
                path: identity::shorten_for_display(source.as_os_str()),
            })
        }
    }
}

/// Create a named folder inside `parent`, returning its path.
pub fn create_named_dir(
    parent: &Path,
    name: &str,
    approved: &safety::Scope,
) -> Result<PathBuf, OpError> {
    validate_name(name)?;
    let parent = safety::guard_source(parent, approved)?;
    let target = safety::guard_destination(&parent.join(name.trim()), approved)?;
    if target.exists() {
        return Err(OpError::NameTaken(name.trim().to_string()));
    }
    std::fs::create_dir_all(identity::for_io(&target)).map_err(|e| io_error(&target, &e))?;
    Ok(target)
}

/// Create a directory. Returns whether it had to be created.
pub fn ensure_dir(path: &Path, approved: &safety::Scope) -> Result<bool, OpError> {
    let target = safety::guard_new_dir(path, approved)?;
    if target.exists() {
        return Ok(false);
    }
    std::fs::create_dir_all(identity::for_io(&target)).map_err(|e| io_error(&target, &e))?;
    Ok(true)
}

/// Reverse a move. Used by undo.
///
/// Refuses rather than forces: if the file is no longer where we put it, or
/// something now occupies the original location, the user is told instead of
/// having a second surprise applied on top of the first.
pub fn move_back(current: &Path, original: &Path, approved: &safety::Scope) -> Result<(), OpError> {
    let source = safety::guard_source(current, approved)?;
    let destination = safety::guard_destination(original, approved)?;

    if destination.exists() {
        return Err(OpError::Io {
            path: identity::shorten_for_display(destination.as_os_str()),
            message: "something is already back at the original location, so undo was skipped"
                .into(),
        });
    }

    if let Some(parent) = destination.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(identity::for_io(parent)).map_err(|e| io_error(parent, &e))?;
        }
    }

    match std::fs::rename(identity::for_io(&source), identity::for_io(&destination)) {
        Ok(()) => Ok(()),
        Err(err) if is_cross_device(&err) => copy_verify_trash(&source, &destination, approved),
        Err(err) => Err(io_error(&source, &err)),
    }
}

/// Remove a directory, but only if it is empty. Used to undo folder creation.
pub fn remove_empty_dir(path: &Path, approved: &safety::Scope) -> Result<bool, OpError> {
    let target = safety::guard_source(path, approved)?;
    match std::fs::read_dir(identity::for_io(&target)) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                return Ok(false); // someone put something in it; leave it alone
            }
        }
        Err(_) => return Ok(false),
    }
    std::fs::remove_dir(identity::for_io(&target)).map_err(|e| io_error(&target, &e))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);

    impl Temp {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "cleanup-fsops-{tag}-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Temp(std::fs::canonicalize(&dir).map(identity::strip_verbatim).unwrap())
        }
        fn file(&self, rel: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&path, bytes).unwrap();
            path
        }
        fn roots(&self) -> safety::Scope {
            safety::Scope::of(vec![self.0.clone()])
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn moves_a_file_into_a_folder_it_creates() {
        let t = Temp::new("move");
        let from = t.file("photo.png", b"image bytes");

        let outcome = move_into(&from, &t.0.join("Images"), &t.roots()).unwrap();

        assert!(!from.exists(), "the original must be gone");
        assert!(outcome.to.exists(), "the file must be at the destination");
        assert_eq!(outcome.renamed_to, None);
        assert!(!outcome.cross_volume);
        assert_eq!(std::fs::read(&outcome.to).unwrap(), b"image bytes");
    }

    #[test]
    fn never_overwrites_an_existing_file() {
        let t = Temp::new("collide");
        let from = t.file("notes.txt", b"the new one");
        t.file("Documents/notes.txt", b"the original - must survive");

        let outcome = move_into(&from, &t.0.join("Documents"), &t.roots()).unwrap();

        assert_eq!(outcome.renamed_to.as_deref(), Some("notes (2).txt"));
        // The pre-existing file is untouched.
        assert_eq!(
            std::fs::read(t.0.join("Documents/notes.txt")).unwrap(),
            b"the original - must survive"
        );
        assert_eq!(std::fs::read(&outcome.to).unwrap(), b"the new one");
    }

    #[test]
    fn refuses_to_move_a_file_outside_the_approved_roots() {
        let t = Temp::new("inside");
        let other = Temp::new("outside");
        let stray = other.file("stray.txt", b"x");

        let err = move_into(&stray, &t.0.join("Documents"), &t.roots()).unwrap_err();
        assert!(matches!(
            err,
            OpError::Safety(SafetyError::OutsideApprovedRoots(_))
        ));
        assert!(stray.exists(), "the refused file must be untouched");
    }

    #[test]
    fn refuses_a_destination_outside_the_approved_roots() {
        let t = Temp::new("src");
        let other = Temp::new("dest");
        let from = t.file("f.txt", b"x");

        let err = move_into(&from, &other.0.join("Elsewhere"), &t.roots()).unwrap_err();
        assert!(matches!(err, OpError::Safety(_)));
        assert!(from.exists(), "the file must not have moved");
    }

    #[test]
    fn moving_a_file_onto_itself_is_a_no_op() {
        let t = Temp::new("self");
        let from = t.file("Images/a.png", b"x");
        let outcome = move_into(&from, &t.0.join("Images"), &t.roots()).unwrap();
        assert!(outcome.to.exists());
        assert_eq!(std::fs::read(&outcome.to).unwrap(), b"x");
    }

    #[test]
    fn undo_puts_a_moved_file_back() {
        let t = Temp::new("undo");
        let original = t.file("doc.pdf", b"contents");
        let outcome = move_into(&original, &t.0.join("Documents"), &t.roots()).unwrap();
        assert!(!original.exists());

        move_back(&outcome.to, &outcome.from, &t.roots()).unwrap();

        assert!(original.exists(), "the file must be back where it started");
        assert!(!outcome.to.exists());
        assert_eq!(std::fs::read(&original).unwrap(), b"contents");
    }

    #[test]
    fn undo_refuses_when_something_reoccupied_the_original_spot() {
        let t = Temp::new("undo-blocked");
        let original = t.file("doc.pdf", b"first");
        let outcome = move_into(&original, &t.0.join("Documents"), &t.roots()).unwrap();

        // The user downloaded a new file with the same name in the meantime.
        std::fs::write(&original, b"a different file entirely").unwrap();

        let err = move_back(&outcome.to, &outcome.from, &t.roots()).unwrap_err();
        assert!(matches!(err, OpError::Io { .. }));
        // Neither file was clobbered.
        assert_eq!(std::fs::read(&original).unwrap(), b"a different file entirely");
        assert_eq!(std::fs::read(&outcome.to).unwrap(), b"first");
    }

    #[test]
    fn ensure_dir_is_idempotent() {
        let t = Temp::new("mkdir");
        let dir = t.0.join("Archives");
        assert!(ensure_dir(&dir, &t.roots()).unwrap(), "created");
        assert!(!ensure_dir(&dir, &t.roots()).unwrap(), "already there");
        assert!(dir.is_dir());
    }

    #[test]
    fn remove_empty_dir_only_removes_empty_ones() {
        let t = Temp::new("rmdir");
        let empty = t.0.join("Empty");
        std::fs::create_dir_all(&empty).unwrap();
        let full = t.0.join("Full");
        t.file("Full/thing.txt", b"x");

        assert!(remove_empty_dir(&empty, &t.roots()).unwrap());
        assert!(!empty.exists());

        assert!(!remove_empty_dir(&full, &t.roots()).unwrap());
        assert!(full.exists(), "a folder with contents must survive");
    }

    #[test]
    fn the_temp_volume_has_a_working_trash() {
        // Not asserting Available unconditionally: CI and odd volumes exist.
        // What matters is that the probe returns a definite answer with a
        // reason, never a panic or a silent fallback to permanent deletion.
        let t = Temp::new("probe");
        match trash_availability(&t.0) {
            TrashAvailability::Available => {}
            TrashAvailability::Unavailable(reason) => {
                assert!(!reason.is_empty(), "a refusal must explain itself");
            }
        }
    }

    #[test]
    fn trashing_removes_the_file_from_its_folder() {
        let t = Temp::new("trash");
        let file = t.file("junk.txt", b"x");

        match to_trash(&file, &t.roots()) {
            Ok(()) => assert!(!file.exists(), "the file must leave its folder"),
            // A volume without a Recycle Bin must refuse, and leave the file.
            Err(OpError::NoRecycleBin { .. }) => assert!(file.exists()),
            Err(other) => panic!("unexpected failure: {other}"),
        }
    }

    #[test]
    fn trashing_refuses_a_file_outside_the_approved_roots() {
        let t = Temp::new("trash-in");
        let other = Temp::new("trash-out");
        let stray = other.file("precious.txt", b"do not delete me");

        let err = to_trash(&stray, &t.roots()).unwrap_err();
        assert!(matches!(
            err,
            OpError::Safety(SafetyError::OutsideApprovedRoots(_))
        ));
        assert!(stray.exists(), "the file must still be there");
    }

    #[cfg(unix)]
    #[test]
    fn trashing_refuses_a_symlink() {
        let t = Temp::new("trash-link");
        let real = t.file("real.txt", b"important");
        let link = t.0.join("link.txt");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        assert!(matches!(
            to_trash(&link, &t.roots()),
            Err(OpError::Safety(SafetyError::NotAFile(_)))
        ));
        assert!(real.exists(), "the link's target must be untouched");
    }
}
