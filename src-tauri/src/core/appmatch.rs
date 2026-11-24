//! Matching installers in a folder against software already installed.
//!
//! This turns the weakest cleanup signal into the strongest one. "This
//! installer is eight months old" is a guess about whether you still need it.
//! "You have version 133 installed and this installer is 121" is a fact.
//!
//! **Bias: refuse rather than guess.** A missed match costs the user nothing -
//! the installer simply is not flagged. A wrong match tells someone they
//! already have software they do not, and invites them to delete the installer
//! for it. So the matcher demands a distinctive token in common and will
//! return nothing rather than something plausible.

use crate::core::apps::InstalledApp;
use serde::Serialize;
use std::cmp::Ordering;

/// Words that appear in installer filenames and say nothing about *what* the
/// product is.
const NOISE: &[&str] = &[
    "setup", "installer", "install", "full", "offline", "online", "portable", "latest", "final",
    "release", "stable", "beta", "alpha", "rc", "win", "win32", "win64", "windows", "mac",
    "macos", "osx", "universal", "x64", "x86", "amd64", "arm64", "aarch64", "32bit", "64bit",
    "bit", "free", "trial", "update", "upgrade", "patch", "en", "us", "enus", "multi",
];

/// A token has to be at least this long to carry the match on its own. "vlc"
/// and "7zip" are real products, so this cannot be much higher.
const DISTINCTIVE_LEN: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum VersionVerdict {
    /// The installed version is newer than the installer.
    Older,
    /// Same version as what is installed.
    Same,
    /// The installer is newer - probably an upgrade not yet run.
    Newer,
    /// One side had no usable version, so no comparison was made.
    Unknown,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallerMatch {
    /// The installed product this appears to be.
    pub app_name: String,
    pub installed_version: Option<String>,
    /// Version read out of the filename, if there was one.
    pub installer_version: Option<String>,
    pub verdict: VersionVerdict,
}

/// Split a name into meaningful lowercase tokens.
pub fn tokenize(value: &str) -> Vec<String> {
    value
        .split(|c: char| !c.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(|part| part.to_lowercase())
        .flat_map(split_letters_from_digits)
        .map(strip_noise_affixes)
        .filter(|token| !token.is_empty() && !NOISE.contains(&token.as_str()))
        // A bare number is a version fragment, not a product name.
        .filter(|token| !token.chars().all(|c| c.is_ascii_digit()))
        // A single letter is whatever was left when "x64" was split apart. It
        // carries no meaning and distorts how much of a name a match covers.
        .filter(|token| token.chars().count() > 1)
        .collect()
}

/// Installer names run words together: `ChromeSetup`, `audacitysetup`. Peel a
/// noise word off either end while something meaningful remains.
fn strip_noise_affixes(token: String) -> String {
    let mut current = token;
    loop {
        let mut changed = false;
        for noise in NOISE {
            if current.len() > noise.len() + 2 {
                if let Some(rest) = current.strip_suffix(noise) {
                    current = rest.to_string();
                    changed = true;
                    break;
                }
                if let Some(rest) = current.strip_prefix(noise) {
                    current = rest.to_string();
                    changed = true;
                    break;
                }
            }
        }
        if !changed {
            return current;
        }
    }
}

/// Drop a trailing extension before tokenizing a filename, so `.exe` and
/// `.msi` are not treated as part of the product name.
fn stem(filename: &str) -> &str {
    filename
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(filename)
}

/// "vlc3" and "7zip" mix letters and digits; splitting them makes "7zip"
/// match "7-Zip" while leaving "mp3" alone would not help anyone.
fn split_letters_from_digits(token: String) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut last_was_digit: Option<bool> = None;

    for ch in token.chars() {
        let is_digit = ch.is_ascii_digit();
        if last_was_digit.is_some_and(|was| was != is_digit) && !current.is_empty() {
            parts.push(std::mem::take(&mut current));
        }
        current.push(ch);
        last_was_digit = Some(is_digit);
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

/// Pull a dotted version out of a filename: `Firefox Setup 121.0.exe` -> `121.0`.
///
/// Takes the longest candidate, because `vlc-3.0.20-win64.exe` contains both
/// `3.0.20` and `64`, and the longer one is the version.
pub fn version_from_filename(name: &str) -> Option<String> {
    let stem = name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(name);
    let bytes: Vec<char> = stem.chars().collect();
    let mut best: Option<String> = None;
    let mut index = 0;

    while index < bytes.len() {
        if !bytes[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        let start = index;
        let mut dots = 0;
        while index < bytes.len() {
            if bytes[index].is_ascii_digit() {
                index += 1;
            } else if bytes[index] == '.'
                && index + 1 < bytes.len()
                && bytes[index + 1].is_ascii_digit()
            {
                dots += 1;
                index += 1;
            } else {
                break;
            }
        }
        if dots >= 1 {
            let candidate: String = bytes[start..index].iter().collect();
            if best.as_ref().is_none_or(|current| candidate.len() > current.len()) {
                best = Some(candidate);
            }
        }
    }
    best
}

/// Compare two dotted versions numerically, component by component.
///
/// `1.10` is newer than `1.9`, which string comparison gets backwards - a
/// mistake that would tell someone to delete the newer installer.
pub fn compare_versions(left: &str, right: &str) -> Ordering {
    let parse = |value: &str| -> Vec<u64> {
        value
            .split(['.', '-', '_'])
            .map(|part| {
                let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
                digits.parse::<u64>().unwrap_or(0)
            })
            .collect()
    };

    let a = parse(left);
    let b = parse(right);
    let length = a.len().max(b.len());

    for index in 0..length {
        let left_part = a.get(index).copied().unwrap_or(0);
        let right_part = b.get(index).copied().unwrap_or(0);
        match left_part.cmp(&right_part) {
            Ordering::Equal => continue,
            other => return other,
        }
    }
    Ordering::Equal
}

/// Find the installed app an installer filename refers to, if any.
pub fn match_installer(filename: &str, apps: &[InstalledApp]) -> Option<InstallerMatch> {
    let file_tokens = tokenize(stem(filename));
    if file_tokens.is_empty() {
        return None;
    }

    let mut candidates: Vec<(usize, Vec<String>, &InstalledApp)> = Vec::new();

    for app in apps {
        let app_tokens = tokenize(&app.name);
        if app_tokens.is_empty() {
            continue;
        }

        let shared: Vec<&String> = file_tokens
            .iter()
            .filter(|token| app_tokens.contains(token))
            .collect();
        if shared.is_empty() {
            continue;
        }

        // At least one token in common has to be distinctive. Without this,
        // "Microsoft Update Installer.exe" matches every Microsoft product.
        if !shared.iter().any(|token| token.len() >= DISTINCTIVE_LEN) {
            continue;
        }

        // And the shared tokens must account for most of the app's own name,
        // so "Python 3.12" does not match "Python Launcher" equally well.
        let coverage = shared.len() * 100 / app_tokens.len();
        if coverage < 50 {
            continue;
        }

        let score = shared.iter().map(|token| token.len()).sum::<usize>() * 10 + coverage;
        let mut signature: Vec<String> = shared.iter().map(|token| (*token).clone()).collect();
        signature.sort();
        candidates.push((score, signature, app));
    }

    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    let (top_score, top_signature, app) = candidates.first()?;

    // Ambiguity means refuse. If two different products match equally well on
    // exactly the same words, those words identify a vendor rather than a
    // product - "microsoft-setup.exe" should not claim to be Edge just
    // because Edge came first in the registry.
    let equally_good = candidates
        .iter()
        .filter(|(score, signature, _)| score == top_score && signature == top_signature)
        .count();
    if equally_good > 1 {
        return None;
    }
    let app = *app;
    let installer_version = version_from_filename(filename);
    let verdict = match (installer_version.as_deref(), app.version.as_deref()) {
        (Some(from_file), Some(from_app)) => match compare_versions(from_file, from_app) {
            Ordering::Less => VersionVerdict::Older,
            Ordering::Equal => VersionVerdict::Same,
            Ordering::Greater => VersionVerdict::Newer,
        },
        _ => VersionVerdict::Unknown,
    };

    Some(InstallerMatch {
        app_name: app.name.clone(),
        installed_version: app.version.clone(),
        installer_version,
        verdict,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::apps::AppSource;

    fn app(name: &str, version: Option<&str>) -> InstalledApp {
        InstalledApp {
            id: name.to_string(),
            name: name.to_string(),
            version: version.map(str::to_string),
            publisher: None,
            installed_at: None,
            location: None,
            estimated_bytes: None,
            source: AppSource::Machine,
        }
    }

    #[test]
    fn versions_compare_numerically_not_as_text() {
        // The case that matters: string comparison says "1.9" > "1.10".
        assert_eq!(compare_versions("1.10", "1.9"), Ordering::Greater);
        assert_eq!(compare_versions("121.0", "133.0"), Ordering::Less);
        assert_eq!(compare_versions("1.2.3", "1.2.3"), Ordering::Equal);
        // Missing components are zero, so 1.2 and 1.2.0 are the same release.
        assert_eq!(compare_versions("1.2", "1.2.0"), Ordering::Equal);
        assert_eq!(compare_versions("2.0", "1.9.9"), Ordering::Greater);
    }

    #[test]
    fn versions_are_pulled_out_of_real_installer_names() {
        assert_eq!(
            version_from_filename("Firefox Setup 121.0.exe").as_deref(),
            Some("121.0")
        );
        assert_eq!(
            version_from_filename("vlc-3.0.20-win64.exe").as_deref(),
            Some("3.0.20"),
            "the longest dotted run wins over the arch suffix"
        );
        assert_eq!(
            version_from_filename("python-3.12.1-amd64.exe").as_deref(),
            Some("3.12.1")
        );
        assert_eq!(version_from_filename("setup.exe"), None);
        assert_eq!(version_from_filename("installer-x64.exe"), None);
    }

    #[test]
    fn an_installer_matches_the_product_it_installs() {
        let apps = vec![
            app("Mozilla Firefox (x64 en-US)", Some("133.0")),
            app("Google Chrome", Some("131.0.6778.86")),
            app("7-Zip 23.01 (x64)", Some("23.01")),
        ];

        let found = match_installer("Firefox Setup 121.0.exe", &apps).expect("should match");
        assert!(found.app_name.contains("Firefox"));
        assert_eq!(found.installer_version.as_deref(), Some("121.0"));
        assert_eq!(found.installed_version.as_deref(), Some("133.0"));
        assert_eq!(
            found.verdict,
            VersionVerdict::Older,
            "133 is installed, so a 121 installer is obsolete"
        );
    }

    #[test]
    fn an_installer_newer_than_what_is_installed_is_not_called_obsolete() {
        let apps = vec![app("Google Chrome", Some("120.0.0.0"))];
        let found = match_installer("ChromeSetup-131.0.1.exe", &apps).unwrap();
        assert_eq!(found.verdict, VersionVerdict::Newer);
    }

    #[test]
    fn letters_and_digits_split_so_7zip_matches_7_zip() {
        let apps = vec![app("7-Zip 23.01 (x64)", Some("23.01"))];
        let found = match_installer("7zip-installer.exe", &apps).expect("should match 7-Zip");
        assert!(found.app_name.contains("7-Zip"));
    }

    #[test]
    fn unrelated_installers_do_not_match_anything() {
        let apps = vec![
            app("Mozilla Firefox (x64 en-US)", Some("133.0")),
            app("Google Chrome", Some("131.0")),
        ];
        assert!(match_installer("SomeRandomTool-1.0.exe", &apps).is_none());
        assert!(match_installer("invoice-scan.exe", &apps).is_none());
    }

    #[test]
    fn a_generic_name_does_not_match_every_product_from_one_vendor() {
        // The failure this guards against: "Microsoft" alone matching a dozen
        // Microsoft products and claiming you already have whatever this is.
        let apps = vec![
            app("Microsoft Edge", Some("131.0")),
            app("Microsoft Teams", Some("24.0")),
            app("Microsoft Visual Studio Code", Some("1.95")),
        ];
        let found = match_installer("microsoft-setup-x64.exe", &apps);
        assert!(
            found.is_none(),
            "a vendor name alone is not enough to claim a match, got {found:?}"
        );
    }

    #[test]
    fn noise_words_alone_never_produce_a_match() {
        let apps = vec![app("Setup Factory Runtime", Some("1.0"))];
        assert!(match_installer("setup-installer-x64-win.exe", &apps).is_none());
    }

    #[test]
    fn a_missing_version_on_either_side_is_unknown_not_a_guess() {
        let apps = vec![app("Notepad++", None)];
        let found = match_installer("npp.8.6.Installer.x64.exe", &apps);
        if let Some(found) = found {
            assert_eq!(found.verdict, VersionVerdict::Unknown);
        }

        let apps = vec![app("Audacity", Some("3.4.2"))];
        let found = match_installer("audacity-setup.exe", &apps).unwrap();
        assert_eq!(found.installer_version, None);
        assert_eq!(found.verdict, VersionVerdict::Unknown);
    }

    #[test]
    fn tokenizing_strips_the_words_that_say_nothing() {
        let tokens = tokenize("VLC-Setup-x64-win64-3.0.20.exe");
        assert!(tokens.contains(&"vlc".to_string()));
        assert!(!tokens.iter().any(|t| t == "setup" || t == "x64" || t == "win64"));
    }
}
