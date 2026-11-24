//! Quick Find: turn a sentence into a query.
//!
//! "Find my largest MP4 files from last month" becomes a category filter, an
//! extension filter, a date bound and a sort order - all of which the existing
//! query engine already does, instantly, over the in-memory index.
//!
//! **No model is involved, and that is deliberate.** Most of what people
//! actually ask for is expressible as metadata: a type, a size, a date, a word
//! in the name. Spending a neural network on "show me big videos" would be
//! slower and worse than a filter. What genuinely needs a model - "images with
//! a purple background", "screenshots containing Unity errors" - needs pixels
//! or OCR text, which this layer does not have; those are reported as
//! unsupported rather than quietly answered with something wrong.

use crate::core::dto::{QueryRequest, SortKey};
use crate::core::model::Category;
use serde::Serialize;

const KB: u64 = 1024;
const MB: u64 = 1024 * 1024;
const GB: u64 = 1024 * 1024 * 1024;
const DAY: i64 = 86_400;

/// What the parser understood, so the UI can show its working rather than
/// presenting results as if by magic.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Interpretation {
    /// Human-readable description of each filter applied.
    pub filters: Vec<String>,
    /// Words left over and used as a filename search.
    pub text: Option<String>,
    /// Set when the request asks for something metadata cannot answer.
    pub needs_content_understanding: Option<String>,
}

/// Internal only: `QueryRequest` crosses IPC inbound, so it is deserialisable
/// rather than serialisable. The UI receives the results and the
/// interpretation, not the request it was turned into.
#[derive(Clone, Debug)]
pub struct ParsedQuery {
    pub request: QueryRequest,
    pub interpretation: Interpretation,
}

/// Words that carry no filtering meaning. Stripped before the remainder
/// becomes a filename search, so "find my big videos" searches for nothing
/// rather than for the literal word "find".
const STOP_WORDS: &[&str] = &[
    "find", "show", "me", "my", "all", "the", "a", "an", "of", "with", "that", "those", "these",
    "get", "list", "search", "for", "in", "on", "from", "please", "any", "some", "i", "to",
    "containing", "contains", "about", "related", "files", "file", "stuff", "things", "is",
    "are", "was", "were", "and", "give",
    // Past-tense filler. "that roadtrip video I downloaded" is about the
    // roadtrip, not about downloading - without these the longest-word
    // heuristic picks the verb.
    "downloaded", "download", "saved", "save", "created", "made", "taken", "took", "sent",
    "received", "got", "put", "last", "this", "past", "recently", "called", "named", "like",
];

/// Phrases that need pixels or document text, which this layer cannot read.
const CONTENT_HINTS: &[(&str, &str)] = &[
    ("background", "colour of an image"),
    ("colour", "colour of an image"),
    ("color", "colour of an image"),
    ("looks like", "what an image looks like"),
    ("similar to", "visual similarity"),
    ("contains text", "text inside a file"),
    ("error", "text inside a screenshot"),
    ("screenshot of", "what a screenshot shows"),
    ("inside", "contents of an archive or document"),
];

/// Parse a natural-language request into a query.
pub fn parse(input: &str) -> ParsedQuery {
    let lower = input.to_lowercase();
    let mut request = QueryRequest {
        limit: 200,
        ..QueryRequest::default()
    };
    let mut interpretation = Interpretation::default();
    let mut consumed: Vec<String> = Vec::new();

    // ---- type ----------------------------------------------------------
    let categories: Vec<(&str, Category)> = vec![
        ("screenshot", Category::Images),
        ("image", Category::Images),
        ("photo", Category::Images),
        ("picture", Category::Images),
        ("video", Category::Videos),
        ("movie", Category::Videos),
        ("clip", Category::Videos),
        ("audio", Category::Audio),
        ("music", Category::Audio),
        ("song", Category::Audio),
        ("document", Category::Documents),
        ("doc", Category::Documents),
        ("archive", Category::Archives),
        ("installer", Category::Installers),
        ("setup", Category::Installers),
    ];
    for (word, category) in categories {
        if contains_word(&lower, word) && !request.categories.contains(&category) {
            request.categories.push(category);
            consumed.push(word.to_string());
        }
    }
    if !request.categories.is_empty() {
        let names: Vec<&str> = request.categories.iter().map(|c| c.key()).collect();
        interpretation.filters.push(format!("type: {}", names.join(", ")));
    }

    // ---- extension -----------------------------------------------------
    // Any bare token that looks like a known extension, with or without a dot.
    const EXTENSIONS: &[&str] = &[
        "pdf", "mp4", "mp3", "png", "jpg", "jpeg", "zip", "rar", "7z", "exe", "msi", "docx",
        "xlsx", "pptx", "txt", "csv", "mov", "mkv", "wav", "flac", "gif", "webp", "iso", "dmg",
        "svg", "psd", "apk",
    ];
    for ext in EXTENSIONS {
        if contains_word(&lower, ext) {
            request.ext = Some((*ext).to_string());
            interpretation.filters.push(format!("extension: .{ext}"));
            consumed.push((*ext).to_string());

            // An extension implies its type. "largest mp4 files" is a request
            // for videos, and saying so makes the interpretation readable
            // rather than leaving the user to infer it.
            let implied = crate::core::category::categorize(Some(ext));
            if implied != Category::Misc && !request.categories.contains(&implied) {
                request.categories.push(implied);
                interpretation
                    .filters
                    .push(format!("type: {}", implied.key()));
            }
            break;
        }
    }

    // ---- size ----------------------------------------------------------
    if let Some((min, max, label, words)) = parse_size(&lower) {
        request.min_size = min;
        request.max_size = max;
        interpretation.filters.push(label);
        consumed.extend(words);
    }

    // ---- date ----------------------------------------------------------
    let now = crate::core::model::now_unix_secs();
    if let Some((after, before, label, words)) = parse_dates(&lower, now) {
        request.modified_after = after;
        request.modified_before = before;
        interpretation.filters.push(label);
        consumed.extend(words);
    }

    // ---- sort ----------------------------------------------------------
    if contains_word(&lower, "largest") || contains_word(&lower, "biggest") {
        request.sort = SortKey::Size;
        request.descending = true;
        interpretation.filters.push("largest first".into());
        consumed.extend(["largest".into(), "biggest".into()]);
    } else if contains_word(&lower, "smallest") {
        request.sort = SortKey::Size;
        request.descending = false;
        interpretation.filters.push("smallest first".into());
        consumed.push("smallest".into());
    } else if contains_word(&lower, "newest") || contains_word(&lower, "recent") {
        request.sort = SortKey::Modified;
        request.descending = true;
        interpretation.filters.push("newest first".into());
        consumed.extend(["newest".into(), "recent".into()]);
    } else if contains_word(&lower, "oldest") {
        request.sort = SortKey::Modified;
        request.descending = false;
        interpretation.filters.push("oldest first".into());
        consumed.push("oldest".into());
    }

    // ---- leftovers become a filename search ----------------------------
    let leftover: Vec<String> = lower
        .split(|c: char| !c.is_alphanumeric() && c != '-')
        .filter(|word| !word.is_empty())
        .filter(|word| !STOP_WORDS.contains(word))
        .filter(|word| !consumed.iter().any(|used| used == word))
        // A bare number has almost certainly been consumed by a size or date
        // phrase; searching filenames for "500" would be noise.
        .filter(|word| !word.chars().all(|c| c.is_ascii_digit()))
        .map(String::from)
        .collect();

    if !leftover.is_empty() {
        // The query engine does substring matching, so the most distinctive
        // single word is a better filter than the whole phrase.
        let needle = leftover
            .iter()
            .max_by_key(|word| word.len())
            .cloned()
            .unwrap_or_default();
        if needle.len() >= 3 {
            interpretation.filters.push(format!("name contains \"{needle}\""));
            interpretation.text = Some(needle.clone());
            request.text = Some(needle);
        }
    }

    // ---- honesty about what this layer cannot do -----------------------
    for (hint, what) in CONTENT_HINTS {
        if lower.contains(hint) {
            interpretation.needs_content_understanding = Some((*what).to_string());
            break;
        }
    }

    ParsedQuery {
        request,
        interpretation,
    }
}

/// Whole-word containment, so "doc" does not match "docker".
fn contains_word(haystack: &str, needle: &str) -> bool {
    haystack
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| word == needle || word == format!("{needle}s"))
}

/// `> 1 GB`, `larger than 500mb`, `under 2 gb`, `huge`.
fn parse_size(input: &str) -> Option<(Option<u64>, Option<u64>, String, Vec<String>)> {
    let bigger = ["larger", "bigger", "greater", "over", "above", "more"]
        .iter()
        .any(|word| contains_word(input, word));
    let smaller = ["smaller", "under", "below", "less"]
        .iter()
        .any(|word| contains_word(input, word));

    if let Some((bytes, raw)) = find_size_literal(input) {
        let label = if smaller {
            format!("smaller than {raw}")
        } else {
            format!("larger than {raw}")
        };
        let words = vec![raw];
        return if smaller {
            Some((None, Some(bytes), label, words))
        } else {
            Some((Some(bytes), None, label, words))
        };
    }

    // Vague but common.
    if contains_word(input, "huge") || contains_word(input, "massive") {
        return Some((
            Some(GB),
            None,
            "larger than 1 GB".into(),
            vec!["huge".into(), "massive".into()],
        ));
    }
    if contains_word(input, "large") || contains_word(input, "big") {
        return Some((
            Some(100 * MB),
            None,
            "larger than 100 MB".into(),
            vec!["large".into(), "big".into()],
        ));
    }
    if bigger || smaller {
        return None; // a comparison with nothing to compare against
    }
    None
}

/// Pull `500mb`, `2 gb`, `700 kb` out of a sentence.
fn find_size_literal(input: &str) -> Option<(u64, String)> {
    let chars: Vec<char> = input.chars().collect();
    let mut index = 0;

    while index < chars.len() {
        if !chars[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        let start = index;
        while index < chars.len() && (chars[index].is_ascii_digit() || chars[index] == '.') {
            index += 1;
        }
        let number: String = chars[start..index].iter().collect();
        let Ok(value) = number.parse::<f64>() else {
            continue;
        };

        // Allow one space between the number and the unit.
        let mut cursor = index;
        if cursor < chars.len() && chars[cursor] == ' ' {
            cursor += 1;
        }
        let rest: String = chars[cursor..].iter().collect();
        let rest = rest.to_lowercase();

        let (multiplier, unit) = if rest.starts_with("gb") || rest.starts_with("gig") {
            (GB, "GB")
        } else if rest.starts_with("mb") || rest.starts_with("meg") {
            (MB, "MB")
        } else if rest.starts_with("kb") {
            (KB, "KB")
        } else {
            continue;
        };

        let bytes = (value * multiplier as f64) as u64;
        return Some((bytes, format!("{number} {unit}")));
    }
    None
}

/// `last month`, `this week`, `older than a year`, `today`.
fn parse_dates(input: &str, now: i64) -> Option<(Option<i64>, Option<i64>, String, Vec<String>)> {
    let older = ["older", "before"].iter().any(|w| contains_word(input, w));

    let span = if contains_word(input, "today") {
        Some((DAY, "today"))
    } else if contains_word(input, "week") {
        Some((7 * DAY, "the last week"))
    } else if contains_word(input, "month") {
        Some((30 * DAY, "the last month"))
    } else if contains_word(input, "year") {
        Some((365 * DAY, "the last year"))
    } else {
        None
    };

    let (seconds, label) = span?;
    let words = vec![
        "today".into(),
        "week".into(),
        "month".into(),
        "year".into(),
        "last".into(),
        "this".into(),
        "past".into(),
        "older".into(),
        "than".into(),
    ];

    if older {
        Some((
            None,
            Some(now - seconds),
            format!("older than {}", label.trim_start_matches("the last ")),
            words,
        ))
    } else {
        Some((Some(now - seconds), None, format!("modified in {label}"), words))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn largest_mp4_files_becomes_a_type_extension_and_sort() {
        let parsed = parse("Find my largest MP4 files");
        assert!(parsed.request.categories.contains(&Category::Videos));
        assert_eq!(parsed.request.ext.as_deref(), Some("mp4"));
        assert!(parsed.request.descending);
        assert!(matches!(parsed.request.sort, SortKey::Size));
        // Nothing meaningful is left, so it must not invent a name search.
        assert_eq!(parsed.request.text, None);
    }

    #[test]
    fn a_size_phrase_becomes_a_size_bound() {
        let parsed = parse("show videos larger than 1 GB");
        assert_eq!(parsed.request.min_size, Some(GB));
        assert_eq!(parsed.request.max_size, None);
        assert!(parsed.request.categories.contains(&Category::Videos));

        let parsed = parse("images under 500mb");
        assert_eq!(parsed.request.max_size, Some(500 * MB));
        assert_eq!(parsed.request.min_size, None);
    }

    #[test]
    fn vague_sizes_still_do_something_sensible() {
        assert_eq!(parse("big videos").request.min_size, Some(100 * MB));
        assert_eq!(parse("huge files").request.min_size, Some(GB));
    }

    #[test]
    fn dates_become_bounds_in_the_right_direction() {
        let now = crate::core::model::now_unix_secs();
        let recent = parse("pdfs from last month");
        let after = recent.request.modified_after.expect("should bound the start");
        assert!((after - (now - 30 * DAY)).abs() < 5);
        assert_eq!(recent.request.modified_before, None);

        // "older than" must bound the other end, or it returns the opposite
        // of what was asked for.
        let old = parse("show me pdfs older than a year");
        assert!(old.request.modified_before.is_some());
        assert_eq!(old.request.modified_after, None);
    }

    #[test]
    fn leftover_words_become_a_filename_search() {
        let parsed = parse("find that roadtrip video I downloaded");
        assert!(parsed.request.categories.contains(&Category::Videos));
        assert_eq!(parsed.request.text.as_deref(), Some("roadtrip"));
    }

    #[test]
    fn filler_words_never_become_a_search_term() {
        // The failure this guards against: searching filenames for "find".
        let parsed = parse("show me all the files");
        assert_eq!(parsed.request.text, None);
        assert!(parsed.request.categories.is_empty());
    }

    #[test]
    fn whole_words_only_so_doc_does_not_match_docker() {
        let parsed = parse("docker");
        assert!(
            !parsed.request.categories.contains(&Category::Documents),
            "\"docker\" is not a request for documents"
        );
    }

    #[test]
    fn requests_needing_pixels_or_text_say_so_instead_of_guessing() {
        for phrase in [
            "images with a purple background",
            "screenshots containing Unity errors",
            "find files similar to this image",
        ] {
            let parsed = parse(phrase);
            assert!(
                parsed.interpretation.needs_content_understanding.is_some(),
                "{phrase} should be flagged as needing content understanding"
            );
        }
        // And an ordinary request must not be flagged.
        assert!(parse("largest mp4 files")
            .interpretation
            .needs_content_understanding
            .is_none());
    }

    #[test]
    fn the_interpretation_explains_every_filter_applied() {
        let parsed = parse("biggest zip archives from last week");
        assert!(!parsed.interpretation.filters.is_empty());
        let joined = parsed.interpretation.filters.join(" | ");
        assert!(joined.contains("archives"), "{joined}");
        assert!(joined.contains("zip"), "{joined}");
        assert!(joined.contains("largest first"), "{joined}");
        assert!(joined.contains("last week"), "{joined}");
    }

    #[test]
    fn an_empty_request_is_harmless() {
        let parsed = parse("");
        assert!(parsed.request.categories.is_empty());
        assert_eq!(parsed.request.text, None);
        assert!(parsed.interpretation.filters.is_empty());
    }
}
