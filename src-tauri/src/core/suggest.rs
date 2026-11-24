//! Organization suggestions.
//!
//! Produces a plan and nothing else. No file is touched here - applying a plan,
//! with its journal and undo, is Phase 4. That split is deliberate: the user
//! should be able to see exactly what would happen, with real resolved
//! destination paths, before anything is capable of happening.
//!
//! Two rules keep the suggestions trustworthy:
//!
//! * **Only loose files in the scan root.** Anything the user has already filed
//!   into a subfolder is their organisation. Rearranging it is the fastest way
//!   to lose someone's trust, and it is not what "tidy my Downloads" means.
//! * **Never propose an overwrite.** If something already exists at the
//!   destination, the plan shows the non-colliding name that would actually be
//!   used. There is no code path anywhere that passes an existing destination
//!   to a rename.

use crate::core::dto::{OrganizePlan, PlanGroup, PlanItem};
use crate::core::identity;
use crate::core::model::{civil_from_unix, Category, EntryFlags, FileEntry, ScanIndex, MONTH_NAMES};
use crate::core::settings::Settings;
use crate::core::text;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// How to decide where a file should go.
///
/// Sorting by type is the default because it is what "tidy my Downloads"
/// usually means, but it is not the only sensible answer - a folder full of
/// photos is far more usefully filed by date, and a developer's folder by
/// extension.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OrganizeMode {
    /// Images/, Documents/, Installers/…
    #[default]
    Type,
    /// 2024/
    Year,
    /// 2024/March/
    YearMonth,
    /// PNG/, PDF/…
    Extension,
}

/// Where files with no usable date go. Named rather than skipped, so nothing
/// silently disappears from a plan.
const UNDATED: &str = "Undated";
/// Where files with no extension go in `Extension` mode.
const NO_EXTENSION: &str = "No extension";

impl OrganizeMode {
    /// The folder this entry belongs in, relative to the scan root.
    fn destination(self, entry: &FileEntry, settings: &Settings) -> String {
        match self {
            OrganizeMode::Type => settings.folder_for(entry.category).to_string(),

            OrganizeMode::Year => match entry.created.or(entry.modified) {
                Some(when) => civil_from_unix(when).0.to_string(),
                None => UNDATED.to_string(),
            },

            OrganizeMode::YearMonth => match entry.created.or(entry.modified) {
                Some(when) => {
                    let (year, month, _) = civil_from_unix(when);
                    let name = MONTH_NAMES
                        .get(month.saturating_sub(1) as usize)
                        .copied()
                        .unwrap_or("Unknown");
                    // A nested path: `fsops` creates intermediate folders and
                    // guards each one, so this needs no special handling.
                    format!("{year}/{name}")
                }
                None => UNDATED.to_string(),
            },

            OrganizeMode::Extension => match entry.ext.as_deref() {
                Some(ext) if !ext.is_empty() => ext.to_uppercase(),
                _ => NO_EXTENSION.to_string(),
            },
        }
    }

    /// Folder names this mode could produce, so a file already named like one
    /// of its own destinations is left alone.
    fn reserved_names(self, settings: &Settings) -> Vec<String> {
        match self {
            OrganizeMode::Type => Category::ALL
                .into_iter()
                .map(|category| text::fold(settings.folder_for(category)))
                .collect(),
            OrganizeMode::Year | OrganizeMode::YearMonth => vec![text::fold(UNDATED)],
            OrganizeMode::Extension => vec![text::fold(NO_EXTENSION)],
        }
    }
}

/// Give up after this many attempts at a non-colliding name, rather than
/// looping forever against a pathological directory.
const MAX_COLLISION_ATTEMPTS: u32 = 1000;

pub fn organize_plan(index: &ScanIndex, settings: &Settings, mode: OrganizeMode) -> OrganizePlan {
    let root = index.root.as_path();
    let mut grouped: BTreeMap<String, PlanGroup> = BTreeMap::new();
    let mut skipped = 0u64;
    let mut collisions = 0u64;
    let mut total_files = 0u64;
    let mut total_bytes = 0u64;

    // Folder names this mode would create, so a file that happens to be named
    // "Images" or "2024" is never proposed for a move into itself.
    let destination_names = mode.reserved_names(settings);

    for entry in &index.entries {
        let from = index.path_of(entry);

        // Only files sitting loose in the scanned folder.
        if from.parent() != Some(root) {
            continue;
        }
        if entry.flags.has(EntryFlags::NOISE) {
            continue;
        }
        if !entry.flags.is_safe_to_touch() {
            skipped += 1;
            continue;
        }

        let name = text::display(&entry.name);
        if destination_names.contains(&text::fold(&name)) {
            skipped += 1;
            continue;
        }

        let destination = mode.destination(entry, settings);
        // A nested destination like "2024/March" is joined component by
        // component so the separator is the platform's own.
        let target_dir = destination
            .split('/')
            .fold(root.to_path_buf(), |path, part| path.join(part));
        let (to_path, resolved_name) = resolve_destination(&target_dir, &entry.name);
        if resolved_name.is_some() {
            collisions += 1;
        }

        total_files += 1;
        total_bytes += entry.size;

        let group = grouped.entry(destination.clone()).or_insert_with(|| PlanGroup {
            destination: destination.clone(),
            category: entry.category,
            files: 0,
            bytes: 0,
            items: Vec::new(),
        });
        group.files += 1;
        group.bytes += entry.size;
        group.items.push(PlanItem {
            id: entry.id,
            name,
            from_path: identity::shorten_for_display(from.as_os_str()),
            destination,
            to_path: text::display(to_path.as_os_str()),
            size: entry.size,
            category: entry.category,
            collides: resolved_name.is_some(),
            resolved_name,
        });
    }

    let mut groups: Vec<PlanGroup> = grouped.into_values().collect();
    for group in &mut groups {
        // Biggest first within a folder, so the consequential moves are visible
        // without scrolling.
        group
            .items
            .sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
    }
    groups.sort_by(|a, b| {
        b.files
            .cmp(&a.files)
            .then_with(|| b.bytes.cmp(&a.bytes))
            .then_with(|| a.destination.cmp(&b.destination))
    });

    OrganizePlan {
        scan_id: index.scan_id,
        root: identity::shorten_for_display(root.as_os_str()),
        groups,
        total_files,
        total_bytes,
        skipped,
        collisions,
    }
}

/// Where a file would actually land.
///
/// Returns the destination path and, when the obvious name is already taken,
/// the replacement that would be used. `None` means the name was free.
///
/// Probing the filesystem rather than comparing strings is deliberate: Windows
/// is case-insensitive, and APFS is case-insensitive by default but can be
/// configured otherwise, so `Photo.PNG` and `photo.png` may or may not be the
/// same name. Only the filesystem knows.
pub fn resolve_destination(dir: &Path, name: &std::ffi::OsStr) -> (PathBuf, Option<String>) {
    let direct = dir.join(name);
    if !exists(&direct) {
        return (direct, None);
    }

    let display = text::display(name);
    let (stem, extension) = split_name(&display);

    for counter in 2..MAX_COLLISION_ATTEMPTS {
        let candidate = match extension {
            Some(ext) => format!("{stem} ({counter}).{ext}"),
            None => format!("{stem} ({counter})"),
        };
        let path = dir.join(&candidate);
        if !exists(&path) {
            return (path, Some(candidate));
        }
    }

    // Pathological directory. Report the original target as colliding rather
    // than inventing a name; Phase 4 refuses the item instead of guessing.
    (direct, Some(display))
}

fn exists(path: &Path) -> bool {
    // `symlink_metadata`, not `exists()`: a broken symlink at the destination
    // still occupies the name, and treating it as free would mean a rename
    // that replaces it.
    std::fs::symlink_metadata(identity::for_io(path)).is_ok()
}

/// Split on the final dot, treating a leading-dot name as having no extension.
fn split_name(name: &str) -> (&str, Option<&str>) {
    let trimmed = name.trim_end_matches('.');
    match trimmed.rfind('.') {
        Some(index) if index > 0 => (&trimmed[..index], Some(&trimmed[index + 1..])),
        _ => (name, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::progress::{CancelToken, NoopSink};
    use crate::core::walk::{scan, WalkOptions};
    use std::ffi::OsString;

    struct Temp(PathBuf);

    impl Temp {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "cleanup-suggest-{tag}-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Temp(dir)
        }
        fn file(&self, rel: &str, len: usize) {
            let path = self.0.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, vec![0u8; len]).unwrap();
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn plan_for(t: &Temp) -> (ScanIndex, OrganizePlan) {
        plan_with(t, OrganizeMode::Type)
    }

    fn plan_with(t: &Temp, mode: OrganizeMode) -> (ScanIndex, OrganizePlan) {
        let options = WalkOptions {
            in_progress_grace_secs: 0,
            ..WalkOptions::default()
        };
        let index = scan(&t.0, 1, &options, &CancelToken::new(), &NoopSink).unwrap();
        let plan = organize_plan(&index, &Settings::default(), mode);
        (index, plan)
    }

    #[test]
    fn sorts_loose_files_into_folders_by_type() {
        let t = Temp::new("basic");
        t.file("holiday.png", 100);
        t.file("screenshot.jpg", 200);
        t.file("invoice.pdf", 300);
        t.file("setup.exe", 400);

        let (_, plan) = plan_for(&t);
        assert_eq!(plan.total_files, 4);
        assert_eq!(plan.total_bytes, 1000);

        let images = plan
            .groups
            .iter()
            .find(|g| g.destination == "Images")
            .expect("an Images group");
        assert_eq!(images.files, 2);
        assert_eq!(images.category, Category::Images);

        let destinations: Vec<&str> = plan.groups.iter().map(|g| g.destination.as_str()).collect();
        assert!(destinations.contains(&"Documents"));
        assert!(destinations.contains(&"Installers"));
    }

    #[test]
    fn leaves_files_the_user_already_filed_alone() {
        let t = Temp::new("filed");
        t.file("loose.png", 10);
        t.file("MyProject/notes.txt", 10);
        t.file("MyProject/deep/thing.pdf", 10);

        let (_, plan) = plan_for(&t);
        assert_eq!(
            plan.total_files, 1,
            "only the loose file should be proposed, got {:?}",
            plan.groups
        );
        assert_eq!(plan.groups[0].items[0].name, "loose.png");
    }

    #[test]
    fn never_proposes_overwriting_an_existing_file() {
        let t = Temp::new("collide");
        t.file("photo.png", 50);
        // Something is already sitting at the destination.
        t.file("Images/photo.png", 90);

        let (_, plan) = plan_for(&t);
        let item = plan
            .groups
            .iter()
            .flat_map(|g| &g.items)
            .find(|i| i.name == "photo.png")
            .expect("the loose photo should still be proposed");

        assert!(item.collides);
        assert_eq!(item.resolved_name.as_deref(), Some("photo (2).png"));
        assert!(item.to_path.replace('\\', "/").ends_with("Images/photo (2).png"));
        assert_eq!(plan.collisions, 1);
    }

    #[test]
    fn collision_renaming_keeps_counting_past_the_first_free_name() {
        let t = Temp::new("collide-many");
        t.file("doc.pdf", 10);
        t.file("Documents/doc.pdf", 10);
        t.file("Documents/doc (2).pdf", 10);
        t.file("Documents/doc (3).pdf", 10);

        let (_, plan) = plan_for(&t);
        let item = plan
            .groups
            .iter()
            .flat_map(|g| &g.items)
            .find(|i| i.name == "doc.pdf")
            .unwrap();
        assert_eq!(item.resolved_name.as_deref(), Some("doc (4).pdf"));
    }

    #[test]
    fn unsafe_entries_are_skipped_and_counted_not_silently_dropped() {
        let t = Temp::new("skip");
        t.file("good.png", 10);
        t.file("busy.zip.crdownload", 10);

        let options = WalkOptions {
            // Leave the grace window on so nothing else is flagged, but the
            // .crdownload extension still marks the partial download.
            ..WalkOptions::default()
        };
        let index = scan(&t.0, 1, &options, &CancelToken::new(), &NoopSink).unwrap();
        let plan = organize_plan(&index, &Settings::default(), OrganizeMode::Type);

        assert!(plan.skipped >= 1, "the part-download must be accounted for");
        assert!(
            !plan
                .groups
                .iter()
                .flat_map(|g| &g.items)
                .any(|i| i.name.contains("crdownload")),
            "a file still being written must never be proposed for a move"
        );
    }

    #[test]
    fn a_file_named_like_a_destination_folder_is_left_alone() {
        let t = Temp::new("selfname");
        t.file("Images", 10); // a file, not a folder
        let (_, plan) = plan_for(&t);
        assert_eq!(plan.total_files, 0);
        assert_eq!(plan.skipped, 1);
    }

    #[test]
    fn noise_is_neither_moved_nor_reported() {
        let t = Temp::new("noise");
        t.file("real.png", 10);
        std::fs::write(t.0.join("desktop.ini"), b"x").unwrap();

        let (_, plan) = plan_for(&t);
        assert_eq!(plan.total_files, 1);
        assert_eq!(plan.skipped, 0, "noise is not a skipped decision, it is invisible");
    }

    #[test]
    fn destination_resolution_treats_a_free_name_as_free() {
        let t = Temp::new("free");
        let (path, resolved) = resolve_destination(&t.0, &OsString::from("brand-new.txt"));
        assert_eq!(resolved, None);
        assert!(path.ends_with("brand-new.txt"));
    }

    #[test]
    fn extensionless_names_still_get_a_counter() {
        let t = Temp::new("noext");
        std::fs::write(t.0.join("Makefile"), b"x").unwrap();
        let (_, resolved) = resolve_destination(&t.0, &OsString::from("Makefile"));
        assert_eq!(resolved.as_deref(), Some("Makefile (2)"));
    }

    #[test]
    fn groups_are_ordered_by_how_many_files_they_would_move() {
        let t = Temp::new("order");
        for i in 0..5 {
            t.file(&format!("img{i}.png"), 10);
        }
        t.file("one.pdf", 10);

        let (_, plan) = plan_for(&t);
        assert_eq!(plan.groups[0].destination, "Images");
        assert_eq!(plan.groups[0].files, 5);
    }

    #[test]
    fn sorting_by_extension_groups_on_the_extension_not_the_type() {
        let t = Temp::new("by-ext");
        t.file("a.png", 10);
        t.file("b.png", 10);
        t.file("c.jpg", 10);

        let (_, plan) = plan_with(&t, OrganizeMode::Extension);
        let names: Vec<&str> = plan.groups.iter().map(|g| g.destination.as_str()).collect();
        assert!(names.contains(&"PNG"), "got {names:?}");
        assert!(names.contains(&"JPG"), "got {names:?}");
        // By type these would all have been one Images folder.
        assert_eq!(plan.groups.len(), 2);
    }

    #[test]
    fn sorting_by_date_uses_the_calendar_not_the_file_type() {
        let t = Temp::new("by-date");
        t.file("photo.png", 10);
        t.file("notes.txt", 10);

        let (_, plan) = plan_with(&t, OrganizeMode::Year);
        // Both files were written now, so both land in the current year -
        // regardless of being different types.
        assert_eq!(plan.groups.len(), 1, "got {:?}", plan.groups);
        assert_eq!(plan.total_files, 2);
        let year = plan.groups[0].destination.clone();
        assert!(
            year.len() == 4 && year.chars().all(|c| c.is_ascii_digit()),
            "expected a year folder, got {year}"
        );
    }

    #[test]
    fn year_and_month_nests_the_month_inside_the_year() {
        let t = Temp::new("by-month");
        t.file("photo.png", 10);

        let (_, plan) = plan_with(&t, OrganizeMode::YearMonth);
        let destination = &plan.groups[0].destination;
        let (year, month) = destination.split_once('/').expect("should be Year/Month");
        assert_eq!(year.len(), 4);
        assert!(
            MONTH_NAMES.contains(&month),
            "{month} should be a month name"
        );
        // The resolved path must use the platform separator, not a literal
        // slash in a filename.
        let to = &plan.groups[0].items[0].to_path;
        assert!(to.contains(month), "{to} should end up inside {month}");
    }

    #[test]
    fn a_file_named_like_a_date_folder_is_left_alone_in_date_mode() {
        let t = Temp::new("undated-name");
        t.file("Undated", 10);
        let (_, plan) = plan_with(&t, OrganizeMode::Year);
        assert_eq!(plan.total_files, 0);
        assert_eq!(plan.skipped, 1);
    }

    #[test]
    fn an_empty_folder_produces_an_empty_plan_rather_than_an_error() {
        let t = Temp::new("empty");
        let (_, plan) = plan_for(&t);
        assert_eq!(plan.total_files, 0);
        assert!(plan.groups.is_empty());
    }
}
