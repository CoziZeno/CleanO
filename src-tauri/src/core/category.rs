//! Extension-to-category mapping, plus a bounded magic-byte fallback.
//!
//! The mapping is a `match` on `&str` rather than a lazily built `HashMap`:
//! the compiler turns it into a length-bucketed jump table, it allocates
//! nothing, and it is trivially readable as a data table.

use crate::core::model::Category;

/// Extensions of files a browser or download manager is still writing.
/// These must never be hashed, moved or trashed - the write is in flight.
const IN_PROGRESS_EXTS: &[&str] = &[
    "crdownload",  // Chrome / Edge / Brave
    "part",        // Firefox, wget
    "partial",     // Edge legacy, IDM
    "download",    // Safari
    "opdownload",  // Opera
    "aria2",       // aria2c control file
    "!ut",         // uTorrent
    "!qb",         // qBittorrent
    "filepart",    // Free Download Manager
];

/// Files that are noise to a human: a folder containing only these reads as
/// empty, and they should never be presented as a cleanup "finding".
const NOISE_NAMES: &[&str] = &[
    ".ds_store",
    "thumbs.db",
    "desktop.ini",
    "icon\r",
    ".localized",
    ".directory",
];

pub fn is_in_progress_ext(ext: &str) -> bool {
    IN_PROGRESS_EXTS.contains(&ext)
}

/// `name_folded` must be the lowercased NFC form from `text::fold`.
pub fn is_noise_name(name_folded: &str) -> bool {
    NOISE_NAMES.contains(&name_folded)
        // AppleDouble resource forks, created whenever macOS writes to a
        // non-HFS volume. Always paired with a real file.
        || (name_folded.starts_with("._") && name_folded.len() > 2)
}

pub fn categorize(ext: Option<&str>) -> Category {
    let Some(ext) = ext else {
        return Category::Misc;
    };
    match ext {
        // ---- Images ----
        "png" | "jpg" | "jpeg" | "jpe" | "jfif" | "webp" | "gif" | "bmp" | "tif" | "tiff"
        | "svg" | "svgz" | "heic" | "heif" | "avif" | "ico" | "icns" | "psd" | "psb" | "ai"
        | "xcf" | "tga" | "exr" | "hdr" | "pbm" | "pgm" | "ppm" | "cr2" | "cr3" | "nef"
        | "arw" | "dng" | "orf" | "rw2" | "raf" | "srw" | "pef" | "x3f" => Category::Images,

        // ---- Videos ----
        // `.ts` and `.mts` are genuinely ambiguous: MPEG transport stream and
        // AVCHD, but also TypeScript. In a Downloads folder the video reading
        // wins by a wide margin, so they live here and not in Development.
        // Being coarse only affects grouping, never a deletion decision.
        "mp4" | "m4v" | "mov" | "mkv" | "avi" | "wmv" | "flv" | "webm" | "mpg" | "mpeg"
        | "mpe" | "m2v" | "mts" | "m2ts" | "ts" | "3gp" | "3g2" | "ogv" | "vob" | "rm"
        | "rmvb" | "asf" | "f4v" | "mxf" | "divx" | "braw" | "r3d" => Category::Videos,

        // ---- Audio ----
        "mp3" | "wav" | "wave" | "flac" | "aac" | "m4a" | "m4b" | "ogg" | "oga" | "opus"
        | "wma" | "aiff" | "aif" | "aifc" | "alac" | "mid" | "midi" | "amr" | "ape" | "wv"
        | "dsf" | "dff" | "caf" | "au" | "ra" => Category::Audio,

        // ---- Documents ----
        "pdf" | "doc" | "docx" | "docm" | "dot" | "dotx" | "odt" | "ott" | "rtf" | "txt"
        | "md" | "markdown" | "rst" | "pages" | "wpd" | "xls" | "xlsx" | "xlsm" | "xlsb"
        | "xlt" | "xltx" | "ods" | "numbers" | "csv" | "tsv" | "ppt" | "pptx" | "pptm"
        | "pot" | "potx" | "odp" | "key" | "epub" | "mobi" | "azw" | "azw3" | "fb2"
        | "djvu" | "djv" | "tex" | "bib" | "chm" | "xps" | "oxps" | "one" | "vsd" | "vsdx"
        | "pub" | "msg" | "eml" | "ics" | "log" => Category::Documents,

        // ---- Archives ----
        "zip" | "zipx" | "rar" | "r00" | "7z" | "tar" | "gz" | "tgz" | "bz2" | "tbz"
        | "tbz2" | "xz" | "txz" | "zst" | "tzst" | "lz" | "lz4" | "lzma" | "lzh" | "lha"
        | "arj" | "cab" | "ace" | "arc" | "sit" | "sitx" | "sea" | "z" | "cpio" | "iso"
        | "img" | "bin" | "mdf" | "nrg" | "jar" | "war" | "ear" | "crx" | "xapk"
        | "obb" => Category::Archives,

        // ---- Installers ----
        // `.exe` is the honest ambiguity here: it may be a portable tool rather
        // than an installer. The category drives *grouping*, never a deletion
        // decision, so being coarse is acceptable; `suggest.rs` (Phase 4) looks
        // at the filename and at whether a newer build exists before it ever
        // recommends anything.
        "exe" | "msi" | "msix" | "msixbundle" | "appx" | "appxbundle" | "msu" | "msp"
        | "dmg" | "pkg" | "mpkg" | "deb" | "rpm" | "appimage" | "apk" | "aab" | "snap"
        | "flatpak" | "flatpakref" | "run" | "vsix" => Category::Installers,

        // ---- Development ----
        "js" | "mjs" | "cjs" | "jsx" | "tsx" | "cts" | "html" | "htm"
        | "xhtml" | "css" | "scss" | "sass" | "less" | "styl" | "vue" | "svelte" | "astro"
        | "json" | "json5" | "jsonc" | "jsonl" | "ndjson" | "yaml" | "yml" | "toml" | "xml"
        | "plist" | "ini" | "cfg" | "conf" | "env" | "properties" | "py" | "pyi" | "pyw"
        | "ipynb" | "rb" | "rake" | "gemspec" | "rs" | "go" | "mod" | "sum" | "java"
        | "class" | "kt" | "kts" | "scala" | "clj" | "cljs" | "swift" | "m" | "mm" | "c"
        | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" | "cs" | "csproj" | "sln"
        | "vb" | "fs" | "fsx" | "fsproj" | "php" | "phtml" | "pl" | "pm" | "sh" | "bash"
        | "zsh" | "fish" | "bat" | "cmd" | "ps1" | "psm1" | "psd1" | "lua" | "r" | "rmd"
        | "jl" | "dart" | "ex" | "exs" | "erl" | "hrl" | "hs" | "lhs" | "ml" | "mli"
        | "nim" | "zig" | "v" | "sql" | "graphql" | "gql" | "proto" | "thrift" | "gradle"
        | "cmake" | "mk" | "makefile" | "dockerfile" | "lock" | "patch" | "diff" | "map"
        | "wasm" | "shader" | "hlsl" | "glsl" | "metal" | "unity" | "prefab" | "asset"
        | "meta" | "o" | "a" | "so" | "dylib" | "dll" | "lib" | "pdb" | "ipa"
        | "editorconfig" => Category::Development,

        // ---- Temporary ----
        // Intentionally narrow. Anything in here is still only ever *surfaced*
        // with a reason; category alone never authorises a delete.
        "tmp" | "temp" | "bak" | "bk" | "old" | "orig" | "swp" | "swo" | "swn" | "dmp"
        | "chk" | "gid" | "~" => Category::Temporary,

        other if is_in_progress_ext(other) => Category::Temporary,

        _ => Category::Misc,
    }
}

/// Number of header bytes `categorize_from_magic` needs.
pub const MAGIC_PROBE_BYTES: usize = 32;

/// Content sniffing for files with no usable extension.
///
/// Scope is deliberately tight: the metadata walk reads no file contents, with
/// this one bounded exception - a single 32-byte read for entries that have no
/// extension at all, which in a real Downloads folder is a handful of files.
/// The caller must verify `EntryFlags::is_safe_to_touch()` first, so this never
/// touches a cloud placeholder (which would trigger a multi-gigabyte download)
/// or a partial download.
pub fn categorize_from_magic(head: &[u8]) -> Option<Category> {
    let at = |i: usize, pat: &[u8]| -> bool {
        head.len() >= i + pat.len() && &head[i..i + pat.len()] == pat
    };

    if at(0, b"\x89PNG\r\n\x1a\n") || at(0, b"\xff\xd8\xff") || at(0, b"GIF8")
        || at(0, b"BM") || at(0, b"II*\0") || at(0, b"MM\0*")
        || at(4, b"ftypheic") || at(4, b"ftypavif") || at(0, b"\0\0\x01\0")
    {
        return Some(Category::Images);
    }
    if at(0, b"RIFF") {
        return match () {
            _ if at(8, b"WEBP") => Some(Category::Images),
            _ if at(8, b"WAVE") => Some(Category::Audio),
            _ if at(8, b"AVI ") => Some(Category::Videos),
            _ => None,
        };
    }
    if at(4, b"ftyp") || at(0, b"\x1a\x45\xdf\xa3") || at(0, b"FLV\x01")
        || at(0, b"\0\0\x01\xba") || at(0, b"\0\0\x01\xb3")
    {
        return Some(Category::Videos);
    }
    if at(0, b"ID3") || at(0, b"fLaC") || at(0, b"OggS") || at(0, b"MThd")
        || at(0, b"\xff\xfb") || at(0, b"\xff\xf3") || at(0, b"\xff\xf2")
    {
        return Some(Category::Audio);
    }
    if at(0, b"%PDF") || at(0, b"{\\rtf") || at(0, b"\xd0\xcf\x11\xe0") {
        return Some(Category::Documents);
    }
    if at(0, b"PK\x03\x04") || at(0, b"Rar!") || at(0, b"7z\xbc\xaf\x27\x1c")
        || at(0, b"\x1f\x8b") || at(0, b"BZh") || at(0, b"\xfd7zXZ")
        || at(0, b"(\xb5/\xfd") || at(0, b"\x04\x22\x4d\x18") || at(0, b"ustar")
    {
        return Some(Category::Archives);
    }
    if at(0, b"MZ")
        || at(0, b"\x7fELF")
        || at(0, b"\xcf\xfa\xed\xfe")
        || at(0, b"\xfe\xed\xfa\xcf")
        || at(0, b"\xca\xfe\xba\xbe")
    {
        return Some(Category::Installers);
    }
    if at(0, b"SQLite format 3\0") || at(0, b"\xca\xfe\xba\xbe") {
        return Some(Category::Development);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_mapping() {
        assert_eq!(categorize(Some("png")), Category::Images);
        assert_eq!(categorize(Some("mkv")), Category::Videos);
        assert_eq!(categorize(Some("flac")), Category::Audio);
        assert_eq!(categorize(Some("docx")), Category::Documents);
        assert_eq!(categorize(Some("7z")), Category::Archives);
        assert_eq!(categorize(Some("dmg")), Category::Installers);
        assert_eq!(categorize(Some("tsx")), Category::Development);
        assert_eq!(categorize(Some("crdownload")), Category::Temporary);
        assert_eq!(categorize(Some("qqqq")), Category::Misc);
        assert_eq!(categorize(None), Category::Misc);
    }

    #[test]
    fn in_progress_extensions_are_recognised() {
        for ext in ["crdownload", "part", "download", "opdownload", "!ut"] {
            assert!(is_in_progress_ext(ext), "{ext}");
        }
        assert!(!is_in_progress_ext("zip"));
    }

    #[test]
    fn noise_detection() {
        assert!(is_noise_name(".ds_store"));
        assert!(is_noise_name("thumbs.db"));
        assert!(is_noise_name("._photo.jpg"));
        assert!(!is_noise_name("_photo.jpg"));
        assert!(!is_noise_name("report.pdf"));
    }

    #[test]
    fn magic_fallback() {
        assert_eq!(categorize_from_magic(b"%PDF-1.7"), Some(Category::Documents));
        assert_eq!(
            categorize_from_magic(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"),
            Some(Category::Images)
        );
        assert_eq!(
            categorize_from_magic(b"RIFF\x24\x08\0\0WAVEfmt "),
            Some(Category::Audio)
        );
        assert_eq!(categorize_from_magic(b"MZ\x90\0"), Some(Category::Installers));
        assert_eq!(categorize_from_magic(b"not a known header"), None);
        assert_eq!(categorize_from_magic(b""), None);
    }
}
