//! User settings: a small JSON file, written atomically.
//!
//! Not in SQLite. Settings are tiny, read once at boot, and being able to open
//! the file in a text editor to see exactly which folders the app is allowed to
//! touch is worth more than storage uniformity.
//!
//! Every field carries `#[serde(default)]` so an older file, a newer file, or a
//! partially hand-edited one still loads instead of resetting the user's
//! configuration to defaults.

use crate::core::model::Category;
use crate::core::walk::WalkOptions;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const SETTINGS_FILE: &str = "settings.json";
/// 2: the scan depth default rose from 8 to 20.
/// 3: added `excludedPaths` and `swipe`.
const CURRENT_SCHEMA: u32 = 3;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ScanSettings {
    pub max_depth: usize,
    pub include_hidden: bool,
    pub sniff_extensionless: bool,
}

impl Default for ScanSettings {
    fn default() -> Self {
        Self {
            max_depth: 20,
            include_hidden: false,
            sniff_extensionless: true,
        }
    }
}

/// The depth default before schema 2. Used to recognise a settings file that
/// still carries the old default so it can be raised; a value the user chose
/// themselves is left alone.
const LEGACY_MAX_DEPTH: usize = 8;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HashSettings {
    /// Concurrent file reads while hashing (Phase 3).
    ///
    /// Not auto-detected: parallel reads are a large win on NVMe and a loss on
    /// a spinning disk, and reliable cross-platform drive-type detection is not
    /// worth the complexity. A conservative default plus a user dial is the
    /// honest trade.
    pub concurrency: usize,
    pub head_bytes: usize,
    /// Byte-compare the survivors of a hash group before allowing deletion.
    /// Off by default: a BLAKE3 collision is a ~2^-128 event.
    pub verify_bytewise: bool,
}

impl Default for HashSettings {
    fn default() -> Self {
        Self {
            concurrency: 6,
            head_bytes: 64 * 1024,
            verify_bytewise: false,
        }
    }
}

/// Which files Swipe Clean puts in front of you.
///
/// Reviewing one file at a time is only worth it for files you can actually
/// judge at a glance, and which those are is personal: a photographer wants
/// every JPEG and CR2, someone clearing an archive of installers wants
/// neither. So it is a setting rather than a built-in list.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SwipeSettings {
    /// Category keys eligible for the queue. **Order is review order**, so the
    /// first category listed is what you are shown first.
    pub categories: Vec<String>,
    /// When non-empty, only these extensions (lowercase, no dot) qualify -
    /// a narrower filter applied on top of `categories`.
    pub extensions: Vec<String>,
    /// Anything smaller than this is skipped. Deciding about a 4 KB file one
    /// swipe at a time is not time well spent.
    pub min_bytes: u64,
}

impl Default for SwipeSettings {
    fn default() -> Self {
        Self {
            // Documents are deliberately absent: a Downloads folder usually
            // holds hundreds of small PDFs and receipts, and burying the
            // photos and installers behind them makes the queue a chore.
            // They are one checkbox away for anyone who wants them.
            categories: vec![
                "images".into(),
                "videos".into(),
                "archives".into(),
                "audio".into(),
                "installers".into(),
            ],
            extensions: Vec::new(),
            min_bytes: 0,
        }
    }
}

impl SwipeSettings {
    /// Review priority for a file, or `None` if it does not belong in the
    /// queue at all. Lower sorts first.
    pub fn priority(&self, category: Category, size: u64, ext: Option<&str>) -> Option<u8> {
        if size < self.min_bytes {
            return None;
        }
        if !self.extensions.is_empty() {
            let ext = ext?;
            if !self.extensions.iter().any(|e| e == ext) {
                return None;
            }
        }
        let position = self
            .categories
            .iter()
            .position(|key| key == category.key())?;
        // More categories than a u8 can rank is not reachable - there are nine
        // - but saturating keeps it honest rather than wrapping to 0.
        Some(position.min(u8::MAX as usize) as u8)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub schema_version: u32,
    pub theme: String,
    /// Scan roots the user has explicitly approved. This is also the allowlist
    /// the safety guard checks every operation against (Phase 4), which is why
    /// it is persisted rather than inferred.
    pub roots: Vec<String>,
    pub scan: ScanSettings,
    pub hash: HashSettings,
    pub stale_days: i64,
    pub ignore: Vec<String>,
    /// Folders Cleanup must never read or modify, even when they sit inside an
    /// approved root. Matched by whole path, not by name, so one specific
    /// `Unity` folder can be excluded without hiding every folder called
    /// "Unity" everywhere.
    pub excluded_paths: Vec<String>,
    pub swipe: SwipeSettings,
    /// Category key -> destination folder name, used by the organizer.
    pub organize_folders: BTreeMap<String, String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA,
            theme: "dark".into(),
            roots: Vec::new(),
            scan: ScanSettings::default(),
            hash: HashSettings::default(),
            stale_days: 180,
            excluded_paths: Vec::new(),
            swipe: SwipeSettings::default(),
            ignore: vec![
                ".ds_store".into(),
                "thumbs.db".into(),
                "desktop.ini".into(),
                "._*".into(),
            ],
            organize_folders: Category::ALL
                .into_iter()
                .map(|c| (c.key().to_string(), c.default_folder().to_string()))
                .collect(),
        }
    }
}

impl Settings {
    /// Bring an older settings file forward.
    ///
    /// Only raises values the user never chose. If they deliberately set a
    /// depth of 8, that stays 8 - silently overriding a setting someone picked
    /// is worse than leaving them on an old default.
    fn migrate(&mut self) {
        if self.schema_version < 2 && self.scan.max_depth == LEGACY_MAX_DEPTH {
            self.scan.max_depth = ScanSettings::default().max_depth;
        }
    }

    /// Clamp anything a hand-edited file could set to a harmful value.
    pub fn sanitized(mut self) -> Self {
        self.schema_version = CURRENT_SCHEMA;
        self.scan.max_depth = self.scan.max_depth.clamp(1, 64);
        self.hash.concurrency = self.hash.concurrency.clamp(1, 64);
        self.hash.head_bytes = self.hash.head_bytes.clamp(4 * 1024, 4 * 1024 * 1024);
        self.stale_days = self.stale_days.clamp(1, 3650);
        if self.theme != "light" && self.theme != "dark" {
            self.theme = "dark".into();
        }
        // A hand-edited file could name a category that does not exist, or
        // write extensions with dots and capitals.
        self.swipe
            .categories
            .retain(|key| Category::ALL.iter().any(|c| c.key() == key));
        self.swipe.categories.dedup();
        self.swipe.extensions = self
            .swipe
            .extensions
            .iter()
            .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
            .filter(|e| !e.is_empty())
            .collect();
        self.swipe.extensions.sort();
        self.swipe.extensions.dedup();
        self.excluded_paths.retain(|p| !p.trim().is_empty());
        self.excluded_paths.dedup();

        // Missing folder names would make the organizer produce an empty
        // destination, so fill any gaps from the defaults.
        for category in Category::ALL {
            self.organize_folders
                .entry(category.key().to_string())
                .or_insert_with(|| category.default_folder().to_string());
        }
        self
    }

    pub fn walk_options(&self) -> WalkOptions {
        WalkOptions {
            max_depth: self.scan.max_depth,
            include_hidden: self.scan.include_hidden,
            ignore: self.ignore.iter().map(|s| s.to_ascii_lowercase()).collect(),
            sniff_extensionless: self.scan.sniff_extensionless,
            in_progress_grace_secs: 60,
            excluded: self.excluded_canonical(),
        }
    }

    /// The excluded folders, canonicalised so prefix comparisons are valid.
    ///
    /// A path that cannot be resolved - a folder on a drive that is not
    /// plugged in, or one since deleted - is dropped rather than kept as a
    /// literal string. It cannot match anything the walk or the guard will
    /// see, and keeping an unresolvable path in a safety list invites the
    /// textual-prefix bugs the guard exists to prevent. The user's entry stays
    /// in the settings file, so reconnecting the drive restores it.
    pub fn excluded_canonical(&self) -> Vec<PathBuf> {
        self.excluded_paths
            .iter()
            .filter_map(|raw| {
                std::fs::canonicalize(Path::new(raw))
                    .ok()
                    .map(crate::core::identity::strip_verbatim)
            })
            .collect()
    }

    /// Approved roots minus excluded folders: what every mutation is checked
    /// against.
    pub fn scope(&self) -> crate::core::safety::Scope {
        crate::core::safety::Scope::new(self.root_paths(), self.excluded_canonical())
    }

    /// Record a folder as off limits. Stored as the user gave it; resolution
    /// happens in `excluded_canonical`.
    pub fn exclude_path(&mut self, path: &Path) {
        let value = path.to_string_lossy().to_string();
        if !self.excluded_paths.iter().any(|p| p == &value) {
            self.excluded_paths.push(value);
        }
    }

    pub fn unexclude_path(&mut self, path: &str) {
        self.excluded_paths.retain(|p| p != path);
    }

    /// Load from `dir/settings.json`.
    ///
    /// A missing file yields defaults. A *corrupt* file also yields defaults but
    /// reports the error, so the caller can tell the user rather than silently
    /// discarding their configuration.
    pub fn load(dir: &Path) -> (Settings, Option<String>) {
        let path = dir.join(SETTINGS_FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<Settings>(&text) {
                Ok(mut settings) => {
                    settings.migrate();
                    (settings.sanitized(), None)
                }
                Err(err) => (
                    Settings::default(),
                    Some(format!(
                        "{SETTINGS_FILE} could not be parsed ({err}); using defaults. \
                         Your file has been left untouched."
                    )),
                ),
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => (Settings::default(), None),
            Err(err) => (
                Settings::default(),
                Some(format!("{SETTINGS_FILE} could not be read ({err})")),
            ),
        }
    }

    /// Write atomically: serialise to a temp file in the same directory, then
    /// rename over the target. A crash mid-write therefore cannot leave a
    /// truncated settings file behind.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let target = dir.join(SETTINGS_FILE);
        let temp = dir.join(format!("{SETTINGS_FILE}.tmp"));
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&temp, json)?;
        // `rename` replaces the destination atomically on both platforms when
        // source and destination are on the same volume, which they are here.
        match std::fs::rename(&temp, &target) {
            Ok(()) => Ok(()),
            Err(err) => {
                let _ = std::fs::remove_file(&temp);
                Err(err)
            }
        }
    }

    pub fn folder_for(&self, category: Category) -> &str {
        self.organize_folders
            .get(category.key())
            .map(String::as_str)
            .unwrap_or_else(|| category.default_folder())
    }

    pub fn add_root(&mut self, root: &Path) {
        let value = root.to_string_lossy().to_string();
        if !self.roots.iter().any(|r| r == &value) {
            self.roots.push(value);
        }
    }

    pub fn root_paths(&self) -> Vec<PathBuf> {
        self.roots.iter().map(PathBuf::from).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("cleanup-settings-{tag}-{unique}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn defaults_cover_every_category() {
        let s = Settings::default();
        for category in Category::ALL {
            assert!(!s.folder_for(category).is_empty(), "{:?}", category);
        }
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = temp_dir("roundtrip");
        let mut original = Settings::default();
        original.stale_days = 42;
        original.scan.include_hidden = true;
        original.add_root(Path::new(if cfg!(windows) { r"C:\Temp" } else { "/tmp" }));
        original.save(&dir).unwrap();

        let (loaded, warning) = Settings::load(&dir);
        assert!(warning.is_none());
        assert_eq!(loaded.stale_days, 42);
        assert!(loaded.scan.include_hidden);
        assert_eq!(loaded.roots.len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        let dir = temp_dir("missing");
        let (loaded, warning) = Settings::load(&dir);
        assert!(warning.is_none());
        assert_eq!(loaded.stale_days, 180);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_corrupt_file_reports_instead_of_silently_resetting() {
        let dir = temp_dir("corrupt");
        std::fs::write(dir.join(SETTINGS_FILE), "{ not json at all").unwrap();
        let (loaded, warning) = Settings::load(&dir);
        assert!(warning.is_some(), "a parse failure must be surfaced");
        assert_eq!(loaded.stale_days, 180);
        // The user's file must still be there for them to inspect.
        assert!(dir.join(SETTINGS_FILE).exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_old_depth_default_is_raised_but_a_chosen_value_is_not() {
        let dir = temp_dir("migrate");

        // A file still carrying the pre-v2 default gets the new one.
        std::fs::write(
            dir.join(SETTINGS_FILE),
            r#"{ "schemaVersion": 1, "scan": { "maxDepth": 8 } }"#,
        )
        .unwrap();
        assert_eq!(Settings::load(&dir).0.scan.max_depth, 20);

        // A depth the user actually chose is left exactly as it is.
        std::fs::write(
            dir.join(SETTINGS_FILE),
            r#"{ "schemaVersion": 1, "scan": { "maxDepth": 3 } }"#,
        )
        .unwrap();
        assert_eq!(Settings::load(&dir).0.scan.max_depth, 3);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_swipe_filter_decides_what_enters_the_queue() {
        let s = SwipeSettings::default();
        // Default order: images first, then video.
        let image = s.priority(Category::Images, 1_000, Some("jpg"));
        let video = s.priority(Category::Videos, 1_000, Some("mp4"));
        assert!(image < video, "images should be reviewed before videos");
        // Documents are off by default.
        assert_eq!(s.priority(Category::Documents, 9_000_000, Some("pdf")), None);
        // So is anything the user did not list.
        assert_eq!(s.priority(Category::Development, 1_000, Some("rs")), None);
    }

    #[test]
    fn reordering_the_categories_reorders_the_queue() {
        let s = SwipeSettings {
            categories: vec!["videos".into(), "images".into()],
            ..SwipeSettings::default()
        };
        assert!(
            s.priority(Category::Videos, 1, Some("mp4"))
                < s.priority(Category::Images, 1, Some("jpg")),
            "the first category listed is shown first"
        );
    }

    #[test]
    fn an_extension_list_narrows_within_the_chosen_categories() {
        let s = SwipeSettings {
            categories: vec!["images".into()],
            extensions: vec!["cr2".into()],
            ..SwipeSettings::default()
        };
        assert!(s.priority(Category::Images, 100, Some("cr2")).is_some());
        assert_eq!(
            s.priority(Category::Images, 100, Some("jpg")),
            None,
            "an extension list is a filter, not an addition"
        );
        // A category that was never selected stays out, extension or not.
        assert_eq!(s.priority(Category::Videos, 100, Some("cr2")), None);
    }

    #[test]
    fn the_minimum_size_keeps_trivial_files_out_of_the_queue() {
        let s = SwipeSettings {
            min_bytes: 1024 * 1024,
            ..SwipeSettings::default()
        };
        assert_eq!(s.priority(Category::Images, 4_000, Some("png")), None);
        assert!(s.priority(Category::Images, 8_000_000, Some("png")).is_some());
    }

    #[test]
    fn swipe_settings_from_a_hand_edited_file_are_cleaned_up() {
        let dir = temp_dir("swipe-clamp");
        std::fs::write(
            dir.join(SETTINGS_FILE),
            r#"{ "swipe": { "categories": ["images", "nonsense"],
                            "extensions": [".CR2", " jpg ", ""] } }"#,
        )
        .unwrap();
        let (loaded, warning) = Settings::load(&dir);
        assert!(warning.is_none());
        assert_eq!(loaded.swipe.categories, vec!["images".to_string()]);
        assert_eq!(
            loaded.swipe.extensions,
            vec!["cr2".to_string(), "jpg".to_string()],
            "extensions are stored lowercase, trimmed and without the dot"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_excluded_folder_survives_a_round_trip_and_reaches_the_guard() {
        let dir = temp_dir("excluded");
        let keep_out = dir.join("Unity");
        std::fs::create_dir_all(&keep_out).unwrap();

        let mut settings = Settings::default();
        settings.add_root(&dir);
        settings.exclude_path(&std::fs::canonicalize(&keep_out).unwrap());
        settings.save(&dir).unwrap();

        let (loaded, _) = Settings::load(&dir);
        assert_eq!(loaded.excluded_paths.len(), 1);
        assert_eq!(
            loaded.excluded_canonical().len(),
            1,
            "a real folder must resolve for the guard to use it"
        );
        assert_eq!(loaded.walk_options().excluded.len(), 1);
        assert!(
            loaded.scope().is_excluded(&loaded.excluded_canonical()[0]),
            "the scope built from settings must refuse the excluded folder"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_excluded_folder_that_no_longer_resolves_is_dropped_not_guessed_at() {
        // A folder on a drive that is not plugged in. Keeping the raw string
        // in a safety list would invite textual prefix matching; the entry
        // stays in the file so reconnecting the drive restores it.
        let mut settings = Settings::default();
        settings
            .excluded_paths
            .push(r"Z:\definitely-not-mounted\Unity".into());
        assert_eq!(settings.excluded_paths.len(), 1);
        assert!(settings.excluded_canonical().is_empty());
    }

    #[test]
    fn hand_edited_extremes_are_clamped() {
        let dir = temp_dir("clamp");
        std::fs::write(
            dir.join(SETTINGS_FILE),
            r#"{ "scan": { "maxDepth": 100000 }, "staleDays": -5, "theme": "neon" }"#,
        )
        .unwrap();
        let (loaded, _) = Settings::load(&dir);
        assert_eq!(loaded.scan.max_depth, 64);
        assert_eq!(loaded.stale_days, 1);
        assert_eq!(loaded.theme, "dark");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn partial_json_keeps_defaults_for_absent_fields() {
        let dir = temp_dir("partial");
        std::fs::write(dir.join(SETTINGS_FILE), r#"{ "staleDays": 30 }"#).unwrap();
        let (loaded, warning) = Settings::load(&dir);
        assert!(warning.is_none());
        assert_eq!(loaded.stale_days, 30);
        assert_eq!(loaded.scan.max_depth, 20);
        assert_eq!(loaded.hash.concurrency, 6);
        std::fs::remove_dir_all(&dir).ok();
    }
}
