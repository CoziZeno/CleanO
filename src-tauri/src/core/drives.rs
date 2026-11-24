//! The drives attached to this machine.
//!
//! Listing them is read-only and cheap. Scanning one is not: a drive root is
//! the one root `safety::validate_root` deliberately refuses, because "scan my
//! whole computer" must never be something a stray click in a folder picker
//! can start. This module exists so that it can be a deliberate, separate act
//! instead - the user sees the drive, its size, and what scanning it means,
//! and chooses it by name.
//!
//! `free_bytes` and `total_bytes` are `Option` rather than 0: "this drive is
//! empty" and "I could not ask" are different answers, and an empty optical
//! drive or a disconnected network mount gives the second one.

use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DriveKind {
    Fixed,
    Removable,
    Network,
    Optical,
    RamDisk,
    Unknown,
}

impl DriveKind {
    pub fn label(self) -> &'static str {
        match self {
            DriveKind::Fixed => "Internal drive",
            DriveKind::Removable => "Removable drive",
            DriveKind::Network => "Network drive",
            DriveKind::Optical => "Optical drive",
            DriveKind::RamDisk => "RAM disk",
            DriveKind::Unknown => "Drive",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Drive {
    /// The root path to scan, e.g. `C:\` or `/`.
    pub path: String,
    /// Volume label, or the mount point's name when it has none.
    pub label: String,
    pub kind: DriveKind,
    /// `None` when the drive could not be queried - no disc in the drive, a
    /// network mount that is not reachable, a permission failure.
    pub total_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
    /// Holds the running operating system.
    pub is_system: bool,
    /// Whether deleting from here can go to a Recycle Bin / Trash at all.
    /// False means a delete would be immediate and permanent.
    pub has_recycle_bin: bool,
}

impl Drive {
    pub fn used_bytes(&self) -> Option<u64> {
        match (self.total_bytes, self.free_bytes) {
            (Some(total), Some(free)) => Some(total.saturating_sub(free)),
            _ => None,
        }
    }
}

/// Every drive currently attached, in a stable order.
pub fn list() -> Vec<Drive> {
    imp::list()
}

/// Validate a drive root the user explicitly chose.
///
/// This is the deliberate counterpart to `safety::validate_root`, which
/// refuses volume roots outright. It still refuses anything that is *not* a
/// volume root, so it cannot be used as a general-purpose bypass of the
/// denylist: a path inside the drive goes through the normal validation.
pub fn validate_drive_root(requested: &Path) -> Result<PathBuf, crate::core::safety::SafetyError> {
    use crate::core::safety::{self, SafetyError};

    let shown = requested.to_string_lossy().to_string();
    let canonical = std::fs::canonicalize(crate::core::identity::for_io(requested))
        .map(crate::core::identity::strip_verbatim)
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                SafetyError::NotFound(shown.clone())
            } else {
                SafetyError::Unreadable {
                    path: shown.clone(),
                    reason: err.to_string(),
                }
            }
        })?;

    if !canonical.is_dir() {
        return Err(SafetyError::NotADirectory(
            canonical.to_string_lossy().to_string(),
        ));
    }
    if !safety::is_volume_root(&canonical) {
        // Not a drive. Whatever it is, it belongs in the ordinary path - which
        // applies the system denylist to the root itself.
        return safety::validate_root(&canonical);
    }
    Ok(canonical)
}

#[cfg(windows)]
mod imp {
    use super::{Drive, DriveKind};
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
    };

    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    const DRIVE_REMOTE: u32 = 4;
    const DRIVE_CDROM: u32 = 5;
    const DRIVE_RAMDISK: u32 = 6;

    fn wide(text: &str) -> Vec<u16> {
        OsStr::new(text).encode_wide().chain(Some(0)).collect()
    }

    pub fn list() -> Vec<Drive> {
        // One bit per drive letter, A at bit 0.
        let mask = unsafe { GetLogicalDrives() };
        if mask == 0 {
            return Vec::new();
        }

        let system_letter = std::env::var("SystemDrive")
            .ok()
            .and_then(|s| s.chars().next())
            .unwrap_or('C')
            .to_ascii_uppercase();

        let mut drives = Vec::new();
        for bit in 0..26u32 {
            if mask & (1 << bit) == 0 {
                continue;
            }
            let letter = (b'A' + bit as u8) as char;
            let root = format!("{letter}:\\");
            let root_w = wide(&root);

            let kind = match unsafe { GetDriveTypeW(root_w.as_ptr()) } {
                DRIVE_FIXED => DriveKind::Fixed,
                DRIVE_REMOVABLE => DriveKind::Removable,
                DRIVE_REMOTE => DriveKind::Network,
                DRIVE_CDROM => DriveKind::Optical,
                DRIVE_RAMDISK => DriveKind::RamDisk,
                // DRIVE_UNKNOWN and DRIVE_NO_ROOT_DIR. The latter means the
                // letter is mapped but there is nothing behind it.
                1 => continue,
                _ => DriveKind::Unknown,
            };

            let (total_bytes, free_bytes) = disk_space(&root_w);
            let label = volume_label(&root_w).unwrap_or_default();
            let label = if label.trim().is_empty() {
                format!("{letter}:")
            } else {
                format!("{label} ({letter}:)")
            };

            drives.push(Drive {
                path: root,
                label,
                kind,
                total_bytes,
                free_bytes,
                is_system: letter == system_letter,
                // Network shares and most removable media have no Recycle Bin;
                // a delete there is immediate. Fixed and RAM disks do.
                has_recycle_bin: matches!(kind, DriveKind::Fixed | DriveKind::RamDisk),
            });
        }
        drives
    }

    fn disk_space(root_w: &[u16]) -> (Option<u64>, Option<u64>) {
        let mut free_to_caller: u64 = 0;
        let mut total: u64 = 0;
        let mut total_free: u64 = 0;
        // Fails for an empty optical drive or an unreachable share, which is
        // information rather than an error - the drive is still listed.
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                root_w.as_ptr(),
                &mut free_to_caller,
                &mut total,
                &mut total_free,
            )
        };
        if ok == 0 {
            (None, None)
        } else {
            // `free_to_caller` honours per-user quotas, which is the number
            // that actually matters to whoever is looking at the screen.
            (Some(total), Some(free_to_caller))
        }
    }

    fn volume_label(root_w: &[u16]) -> Option<String> {
        let mut name = [0u16; 261];
        let ok = unsafe {
            GetVolumeInformationW(
                root_w.as_ptr(),
                name.as_mut_ptr(),
                name.len() as u32,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
            )
        };
        if ok == 0 {
            return None;
        }
        let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        Some(String::from_utf16_lossy(&name[..end]))
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{Drive, DriveKind};
    use std::path::Path;

    /// macOS and Linux.
    ///
    /// Free and total space are reported as `None`: reading them means
    /// `statvfs`, which needs a `libc` dependency this crate does not have.
    /// Listing the volumes is still the useful half - it is what lets someone
    /// pick an external disk to scan - and an absent number is shown as
    /// absent rather than invented.
    pub fn list() -> Vec<Drive> {
        let mut drives = vec![Drive {
            path: "/".into(),
            label: "Macintosh HD".into(),
            kind: DriveKind::Fixed,
            total_bytes: None,
            free_bytes: None,
            is_system: true,
            has_recycle_bin: true,
        }];

        // Mounted volumes. On macOS the system disk also appears here as a
        // symlink to `/`, which would list it twice.
        if let Ok(entries) = std::fs::read_dir("/Volumes") {
            for entry in entries.flatten() {
                let path = entry.path();
                if std::fs::symlink_metadata(&path)
                    .map(|md| md.file_type().is_symlink())
                    .unwrap_or(true)
                {
                    continue;
                }
                let label = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "Volume".into());
                drives.push(Drive {
                    path: path.to_string_lossy().to_string(),
                    label,
                    kind: DriveKind::Removable,
                    total_bytes: None,
                    free_bytes: None,
                    is_system: false,
                    // An external volume gets a .Trashes only if the system
                    // can write one; the trash crate falls back and reports
                    // failure, which is what the operation surfaces.
                    has_recycle_bin: true,
                });
            }
        }

        let _ = Path::new("/");
        drives
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_machine_has_at_least_one_drive() {
        let drives = list();
        assert!(
            !drives.is_empty(),
            "a running machine must have at least one volume"
        );
    }

    #[test]
    fn every_listed_drive_is_a_real_volume_root() {
        for drive in list() {
            let path = PathBuf::from(&drive.path);
            // A listed drive must be something `validate_drive_root` accepts,
            // or the UI would offer a button that cannot work. An optical
            // drive with no disc is allowed to fail - it is listed so the user
            // can see it is there.
            if drive.total_bytes.is_none() {
                continue;
            }
            assert!(
                crate::core::safety::is_volume_root(&path),
                "{} should be a volume root",
                drive.path
            );
        }
    }

    #[test]
    fn used_space_is_derived_only_when_both_numbers_are_known() {
        let unknown = Drive {
            path: "Z:\\".into(),
            label: "Z:".into(),
            kind: DriveKind::Optical,
            total_bytes: None,
            free_bytes: None,
            is_system: false,
            has_recycle_bin: false,
        };
        assert_eq!(unknown.used_bytes(), None, "never invent a number");

        let known = Drive {
            total_bytes: Some(1000),
            free_bytes: Some(250),
            ..unknown.clone()
        };
        assert_eq!(known.used_bytes(), Some(750));
    }

    #[test]
    fn a_folder_that_is_not_a_drive_falls_back_to_normal_validation() {
        // The drive path must not become a way around the denylist.
        let system = if cfg!(windows) {
            PathBuf::from(r"C:\Windows")
        } else {
            PathBuf::from("/usr")
        };
        if system.exists() {
            assert!(
                validate_drive_root(&system).is_err(),
                "a system folder is still refused when asked for as a drive"
            );
        }
    }
}
