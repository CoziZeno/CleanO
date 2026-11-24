//! The display boundary.
//!
//! Filenames come off disk in a platform-specific form. macOS/APFS returns NFD
//! (`e` + U+0301), Windows returns NFC (U+00E9). The same visible filename
//! therefore has different bytes on the two platforms, which silently breaks
//! substring search and name comparison if you ignore it.
//!
//! Rule enforced by having exactly one conversion point: normalise to NFC when
//! producing a string for display or for matching, and never use that string
//! for I/O.

use std::ffi::OsStr;
use unicode_normalization::UnicodeNormalization;

/// Lossy UTF-8 plus NFC normalisation. For display and for search only.
pub fn display(value: &OsStr) -> String {
    let lossy = value.to_string_lossy();
    if lossy.is_ascii() {
        // Fast path: ASCII is already NFC and the overwhelming majority case.
        return lossy.into_owned();
    }
    lossy.nfc().collect()
}

/// Lowercased NFC form, used for case-insensitive search and name heuristics.
pub fn fold(value: &str) -> String {
    if value.is_ascii() {
        return value.to_ascii_lowercase();
    }
    value.nfc().flat_map(|c| c.to_lowercase()).collect()
}

/// Lowercased extension without the dot, ASCII-folded.
///
/// Deliberately does not treat a leading-dot filename as having an extension:
/// `.gitignore` is a name, not an extension, which is what every file manager
/// shows and what users expect.
pub fn extension_of(name: &OsStr) -> Option<Box<str>> {
    let name = name.to_string_lossy();
    let trimmed = name.trim_end_matches('.');
    let idx = trimmed.rfind('.')?;
    if idx == 0 {
        return None; // dotfile, e.g. ".gitignore"
    }
    let ext = &trimmed[idx + 1..];
    if ext.is_empty() || ext.len() > 24 {
        // A 200-character "extension" is part of the name, not a type.
        return None;
    }
    Some(ext.to_ascii_lowercase().into_boxed_str())
}

/// Case-insensitive substring test straight against an on-disk name.
///
/// `needle_folded` must already have been through `fold`. The ASCII path
/// allocates nothing, which matters because this runs once per entry on every
/// keystroke of the search box - 100k allocations per character would be a
/// needless few milliseconds of jank.
pub fn contains_fold(haystack: &OsStr, needle_folded: &str) -> bool {
    if needle_folded.is_empty() {
        return true;
    }
    let lossy = haystack.to_string_lossy();
    if lossy.is_ascii() && needle_folded.is_ascii() {
        return ascii_contains_ignore_case(lossy.as_bytes(), needle_folded.as_bytes());
    }
    fold(&display(haystack)).contains(needle_folded)
}

/// `needle` is assumed already lowercase.
fn ascii_contains_ignore_case(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.len() > haystack.len() {
        return false;
    }
    let first = needle[0];
    for start in 0..=(haystack.len() - needle.len()) {
        if haystack[start].to_ascii_lowercase() != first {
            continue;
        }
        if haystack[start..start + needle.len()]
            .iter()
            .zip(needle)
            .all(|(h, n)| h.to_ascii_lowercase() == *n)
        {
            return true;
        }
    }
    false
}

/// Case-insensitive glob supporting a single trailing or leading `*`.
///
/// This covers the ignore patterns we actually need (`._*`, `*.tmp`,
/// `desktop.ini`) without taking on a glob dependency.
pub fn matches_pattern(name_folded: &str, pattern: &str) -> bool {
    let pat = pattern.to_ascii_lowercase();
    match (pat.strip_prefix('*'), pat.strip_suffix('*')) {
        (Some(suffix), None) => name_folded.ends_with(suffix),
        (None, Some(prefix)) => name_folded.starts_with(prefix),
        (Some(_), Some(_)) => {
            let inner = pat.trim_matches('*');
            inner.is_empty() || name_folded.contains(inner)
        }
        (None, None) => name_folded == pat,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn extension_basics() {
        let e = |s: &str| extension_of(&OsString::from(s)).map(|b| b.to_string());
        assert_eq!(e("photo.PNG").as_deref(), Some("png"));
        assert_eq!(e("archive.tar.gz").as_deref(), Some("gz"));
        assert_eq!(e("Setup (1).exe").as_deref(), Some("exe"));
        assert_eq!(e(".gitignore"), None);
        assert_eq!(e("Makefile"), None);
        assert_eq!(e("trailing."), None);
    }

    #[test]
    fn nfd_and_nfc_fold_to_the_same_string() {
        // "resume" with an acute accent, composed vs decomposed.
        let nfc = OsString::from("r\u{00e9}sum\u{00e9}.pdf");
        let nfd = OsString::from("re\u{0301}sume\u{0301}.pdf");
        assert_eq!(display(&nfc), display(&nfd));
        assert_eq!(fold(&display(&nfd)), fold(&display(&nfc)));
    }

    #[test]
    fn substring_search_ignores_case_and_handles_nfd() {
        let name = OsString::from("Holiday Photo.PNG");
        assert!(contains_fold(&name, "holiday"));
        assert!(contains_fold(&name, "photo.png"));
        assert!(contains_fold(&name, ""));
        assert!(!contains_fold(&name, "video"));
        // Searching the composed form must find the decomposed filename.
        let nfd = OsString::from("re\u{0301}sume\u{0301}.pdf");
        assert!(contains_fold(&nfd, &fold("résumé")));
    }

    #[test]
    fn patterns() {
        assert!(matches_pattern("._thumb", "._*"));
        assert!(matches_pattern("notes.tmp", "*.tmp"));
        assert!(matches_pattern("desktop.ini", "desktop.ini"));
        assert!(!matches_pattern("desktop.inifile", "desktop.ini"));
    }
}
