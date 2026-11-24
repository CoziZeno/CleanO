//! What is installed on this computer.
//!
//! Read-only, always. Nothing here uninstalls anything, and the uninstall
//! command recorded against each entry is shown as information rather than
//! offered as a button - removing software is the operating system's job and
//! a file cleaner has no business doing it.
//!
//! The real payoff is in `appmatch`: knowing what is installed turns "this
//! installer is eight months old" into "you already have version 133, this
//! installer is 121", which is an actual reason rather than a guess about age.

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AppSource {
    /// 64-bit machine-wide registry.
    Machine,
    /// 32-bit machine-wide registry (WOW6432Node).
    Machine32,
    /// Installed for this user only.
    User,
    /// macOS application bundle.
    Bundle,
}

impl AppSource {
    pub fn label(self) -> &'static str {
        match self {
            AppSource::Machine => "All users",
            AppSource::Machine32 => "All users (32-bit)",
            AppSource::User => "This user",
            AppSource::Bundle => "Applications",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledApp {
    /// Registry key name, or bundle identifier. Stable enough to key a row on.
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub publisher: Option<String>,
    /// Unix seconds, midnight UTC of the recorded install date.
    pub installed_at: Option<i64>,
    pub location: Option<String>,
    /// As reported by the installer. Frequently absent, and frequently wrong
    /// when present - shown as "about", never summed into a total.
    pub estimated_bytes: Option<u64>,
    pub source: AppSource,
}

/// Everything we can see, sorted by name.
pub fn installed() -> Vec<InstalledApp> {
    let mut apps = imp::enumerate();

    // The same product often appears in several places (per-user and
    // machine-wide, or 32- and 64-bit views of one installer). Collapse on
    // name plus version so the list reflects what a person would count.
    apps.sort_by(|a, b| {
        fold(&a.name)
            .cmp(&fold(&b.name))
            .then_with(|| a.version.cmp(&b.version))
    });
    apps.dedup_by(|a, b| fold(&a.name) == fold(&b.name) && a.version == b.version);
    apps
}

fn fold(value: &str) -> String {
    value.trim().to_lowercase()
}

/// Registry `InstallDate` is `YYYYMMDD`. Returns midnight UTC of that day.
pub fn parse_install_date(raw: &str) -> Option<i64> {
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() != 8 {
        return None;
    }
    let year: i64 = digits[0..4].parse().ok()?;
    let month: i64 = digits[4..6].parse().ok()?;
    let day: i64 = digits[6..8].parse().ok()?;
    if !(1970..=3000).contains(&year) || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    // Days since the epoch, via the civil-from-days algorithm. Avoids a date
    // dependency for one conversion.
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400)
}

// ---------------------------------------------------------------- Windows ---

#[cfg(windows)]
mod imp {
    use super::{parse_install_date, AppSource, InstalledApp};
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY};
    use winreg::RegKey;

    const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";

    pub fn enumerate() -> Vec<InstalledApp> {
        let mut apps = Vec::new();
        read_into(
            &mut apps,
            RegKey::predef(HKEY_LOCAL_MACHINE),
            KEY_WOW64_64KEY,
            AppSource::Machine,
        );
        read_into(
            &mut apps,
            RegKey::predef(HKEY_LOCAL_MACHINE),
            KEY_WOW64_32KEY,
            AppSource::Machine32,
        );
        read_into(
            &mut apps,
            RegKey::predef(HKEY_CURRENT_USER),
            KEY_WOW64_64KEY,
            AppSource::User,
        );
        apps
    }

    fn read_into(apps: &mut Vec<InstalledApp>, root: RegKey, view: u32, source: AppSource) {
        let Ok(uninstall) = root.open_subkey_with_flags(UNINSTALL, KEY_READ | view) else {
            return;
        };

        for key_name in uninstall.enum_keys().flatten() {
            let Ok(entry) = uninstall.open_subkey_with_flags(&key_name, KEY_READ | view) else {
                continue;
            };

            let name: String = match entry.get_value::<String, _>("DisplayName") {
                Ok(value) if !value.trim().is_empty() => value,
                // No display name means nothing a person would recognise -
                // these are fragments, not applications.
                _ => continue,
            };

            // Windows updates, hotfixes and driver packages all live here.
            // Listing them would bury the actual applications.
            if entry.get_value::<u32, _>("SystemComponent").unwrap_or(0) == 1 {
                continue;
            }
            if entry.get_value::<String, _>("ReleaseType").is_ok_and(|release| {
                matches!(release.as_str(), "Security Update" | "Update" | "Hotfix")
            }) {
                continue;
            }
            if entry.get_value::<String, _>("ParentKeyName").is_ok() {
                continue; // a component of something else already listed
            }

            let optional = |value: &str| -> Option<String> {
                entry
                    .get_value::<String, _>(value)
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            };

            apps.push(InstalledApp {
                id: key_name,
                name: name.trim().to_string(),
                version: optional("DisplayVersion"),
                publisher: optional("Publisher"),
                installed_at: optional("InstallDate")
                    .as_deref()
                    .and_then(parse_install_date),
                location: optional("InstallLocation"),
                // Recorded in KB, and routinely absent or wrong.
                estimated_bytes: entry
                    .get_value::<u32, _>("EstimatedSize")
                    .ok()
                    .filter(|kb| *kb > 0)
                    .map(|kb| kb as u64 * 1024),
                source,
            });
        }
    }
}

// ------------------------------------------------------------------ macOS ---

#[cfg(target_os = "macos")]
mod imp {
    use super::{AppSource, InstalledApp};
    use std::path::{Path, PathBuf};

    pub fn enumerate() -> Vec<InstalledApp> {
        let mut apps = Vec::new();
        let mut roots = vec![PathBuf::from("/Applications")];
        if let Some(home) = dirs::home_dir() {
            roots.push(home.join("Applications"));
        }
        for root in roots {
            scan_dir(&root, &mut apps, 0);
        }
        apps
    }

    /// `/Applications` has one level of folders (Utilities, vendor folders),
    /// so a shallow descent finds the rest without walking a whole tree.
    fn scan_dir(dir: &Path, apps: &mut Vec<InstalledApp>, depth: usize) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".app") {
                if let Some(app) = read_bundle(&path, &name) {
                    apps.push(app);
                }
            } else if depth < 1 && path.is_dir() {
                scan_dir(&path, apps, depth + 1);
            }
        }
    }

    fn read_bundle(path: &Path, folder_name: &str) -> Option<InstalledApp> {
        let plist_path = path.join("Contents").join("Info.plist");
        // `plist` handles both the XML and binary formats; most bundles use
        // binary, which is why this is not hand-parsed.
        let value: plist::Value = plist::from_file(&plist_path).ok()?;
        let dict = value.as_dictionary()?;
        let text = |key: &str| -> Option<String> {
            dict.get(key)
                .and_then(|v| v.as_string())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };

        let display = text("CFBundleDisplayName")
            .or_else(|| text("CFBundleName"))
            .unwrap_or_else(|| folder_name.trim_end_matches(".app").to_string());

        Some(InstalledApp {
            id: text("CFBundleIdentifier").unwrap_or_else(|| path.to_string_lossy().to_string()),
            name: display,
            // The marketing version is the one a person recognises;
            // CFBundleVersion is often a build number.
            version: text("CFBundleShortVersionString").or_else(|| text("CFBundleVersion")),
            publisher: None,
            installed_at: std::fs::metadata(path)
                .ok()
                .and_then(|m| m.created().ok())
                .and_then(crate::core::model::to_unix_secs),
            location: Some(path.to_string_lossy().to_string()),
            estimated_bytes: None,
            source: AppSource::Bundle,
        })
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use super::InstalledApp;
    pub fn enumerate() -> Vec<InstalledApp> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_dates_convert_from_the_registry_format() {
        // 1970-01-01 is the epoch itself.
        assert_eq!(parse_install_date("19700101"), Some(0));
        // 2000-01-01 = 10957 days after the epoch.
        assert_eq!(parse_install_date("20000101"), Some(10_957 * 86_400));
        // A leap day, which a naive conversion gets wrong.
        assert_eq!(parse_install_date("20240229"), Some(19_782 * 86_400));
    }

    #[test]
    fn nonsense_dates_are_rejected_rather_than_guessed() {
        assert_eq!(parse_install_date(""), None);
        assert_eq!(parse_install_date("2024"), None);
        assert_eq!(parse_install_date("20241301"), None, "month 13");
        assert_eq!(parse_install_date("20240132"), None, "day 32");
        assert_eq!(parse_install_date("not a date"), None);
    }

    #[cfg(windows)]
    #[test]
    fn finds_software_actually_installed_on_this_machine() {
        let apps = installed();
        // Any real Windows install has entries here. An empty list means the
        // registry read silently failed, which is worth catching.
        assert!(
            !apps.is_empty(),
            "expected to find installed software in the registry"
        );
        assert!(
            apps.iter().all(|app| !app.name.trim().is_empty()),
            "every listed app must have a name a person would recognise"
        );
        // Most entries carry a version; requiring all of them would be wrong,
        // since plenty of installers simply do not record one.
        let with_version = apps.iter().filter(|app| app.version.is_some()).count();
        assert!(
            with_version * 2 > apps.len(),
            "most entries should have a version, got {with_version} of {}",
            apps.len()
        );
    }

    #[cfg(windows)]
    #[test]
    fn the_same_product_is_not_listed_twice() {
        let apps = installed();
        let mut seen = std::collections::HashSet::new();
        for app in &apps {
            let key = (app.name.to_lowercase(), app.version.clone());
            assert!(
                seen.insert(key),
                "duplicate entry for {} {:?}",
                app.name,
                app.version
            );
        }
    }
}
