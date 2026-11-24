//! Errors that cross the IPC boundary.
//!
//! Serialised as `{ code, message, hint }` rather than a bare string so the UI
//! can branch on `code` (for example to show the macOS privacy panel) while
//! still having something honest to display if it does not recognise the code.
//! `hint` is the recovery step, kept separate from the diagnosis.

use crate::core::safety::SafetyError;
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

/// `Clone` is required by Tauri's `Emitter`, which may fan an event out to
/// several windows. Every variant holds owned data, so it is free.
#[derive(Debug, Clone, thiserror::Error)]
pub enum AppError {
    #[error("No folder has been scanned yet.")]
    NoScan,

    #[error("A scan is already running.")]
    ScanInProgress,

    #[error("A duplicate check is already running.")]
    DedupeInProgress,

    #[error("No duplicate check has been run for this scan yet.")]
    NoDuplicateScan,

    #[error("These duplicate results are from an earlier scan.")]
    StaleDuplicates,

    #[error("Could not find your Downloads folder.")]
    NoDownloadsFolder,

    #[error("{0}")]
    Safety(#[from] SafetyError),

    #[error("Cleanup is not allowed to read {path}.")]
    PermissionDenied {
        path: String,
        /// macOS gates ~/Downloads behind a consent prompt, so the recovery
        /// step is "grant access in System Settings", not "check the ACL".
        privacy_prompt: bool,
    },

    #[error("{0}")]
    Io(String),

    #[error("{0}")]
    Invalid(String),
}

impl AppError {
    pub fn code(&self) -> &'static str {
        match self {
            AppError::NoScan => "noScan",
            AppError::ScanInProgress => "scanInProgress",
            AppError::DedupeInProgress => "dedupeInProgress",
            AppError::NoDuplicateScan => "noDuplicateScan",
            AppError::StaleDuplicates => "staleDuplicates",
            AppError::NoDownloadsFolder => "noDownloadsFolder",
            AppError::Safety(SafetyError::NotFound(_)) => "rootNotFound",
            AppError::Safety(SafetyError::NotADirectory(_)) => "rootNotADirectory",
            AppError::Safety(SafetyError::NotAFile(_)) => "notAFile",
            AppError::Safety(SafetyError::VolumeRoot(_)) => "rootIsVolume",
            AppError::Safety(SafetyError::SystemPath(_)) => "rootIsSystemPath",
            AppError::Safety(SafetyError::OutsideApprovedRoots(_)) => "outsideApprovedRoots",
            AppError::Safety(SafetyError::Excluded(_)) => "excludedPath",
            AppError::Safety(SafetyError::Unreadable { .. }) => "rootUnreadable",
            AppError::PermissionDenied { .. } => "permissionDenied",
            AppError::Io(_) => "io",
            AppError::Invalid(_) => "invalid",
        }
    }

    pub fn hint(&self) -> Option<String> {
        match self {
            AppError::NoScan => Some("Run a scan first.".into()),
            AppError::ScanInProgress => {
                Some("Cancel the running scan before starting another.".into())
            }
            AppError::DedupeInProgress => {
                Some("Cancel the running check before starting another.".into())
            }
            AppError::NoDuplicateScan => {
                Some("Run the duplicate check to compare file contents.".into())
            }
            AppError::StaleDuplicates => {
                Some("Run the duplicate check again for the current scan.".into())
            }
            AppError::NoDownloadsFolder => Some(
                "Pick a folder manually - your Downloads folder may have been moved or renamed."
                    .into(),
            ),
            AppError::Safety(SafetyError::VolumeRoot(_)) => {
                Some("Choose a folder inside the drive instead.".into())
            }
            AppError::Safety(SafetyError::SystemPath(_)) => {
                Some("Choose a folder in your own home directory.".into())
            }
            AppError::PermissionDenied {
                privacy_prompt: true,
                ..
            } => Some(
                "Open System Settings > Privacy & Security > Files and Folders and allow \
                 Cleanup to access this folder, then scan again."
                    .into(),
            ),
            AppError::PermissionDenied {
                privacy_prompt: false,
                ..
            } => Some("Check that your user account can read this folder.".into()),
            _ => None,
        }
    }

    /// Classify a filesystem error against the path it happened on.
    ///
    /// The macOS TCC case and an ordinary permission problem both arrive as
    /// `PermissionDenied`, so the distinction is made here - by whether the path
    /// is one of the user folders macOS gates - instead of guessing in the UI.
    pub fn from_io(err: &std::io::Error, path: &std::path::Path) -> AppError {
        let shown = crate::core::identity::shorten_for_display(path.as_os_str());
        match err.kind() {
            std::io::ErrorKind::PermissionDenied => AppError::PermissionDenied {
                path: shown,
                privacy_prompt: cfg!(target_os = "macos") && is_tcc_protected(path),
            },
            std::io::ErrorKind::NotFound => {
                AppError::Safety(SafetyError::NotFound(path.to_string_lossy().to_string()))
            }
            _ => AppError::Io(format!("{shown}: {err}")),
        }
    }
}

/// The user folders macOS puts behind a consent prompt.
fn is_tcc_protected(path: &std::path::Path) -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    ["Downloads", "Documents", "Desktop", "Pictures", "Movies", "Music"]
        .iter()
        .any(|folder| path.starts_with(home.join(folder)))
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("AppError", 3)?;
        state.serialize_field("code", self.code())?;
        state.serialize_field("message", &self.to_string())?;
        state.serialize_field("hint", &self.hint())?;
        state.end()
    }
}

pub type AppResult<T> = Result<T, AppError>;
