//! Thumbnail generation.
//!
//! This is the one part of the app that decodes arbitrary, untrusted file
//! contents, so the guards matter more than the feature:
//!
//! * **Decode limits.** A 64000x64000 PNG is a few hundred KB on disk and
//!   ~16 GB decoded. Limits are set before decoding, not after, so a
//!   decompression bomb fails rather than taking the process with it.
//! * **A size ceiling.** Files beyond it are skipped outright - no thumbnail
//!   is worth minutes of decoding.
//! * **The caller checks the flags.** Cloud placeholders are never passed in:
//!   reading one downloads the whole file over the user's connection, which is
//!   exactly the behaviour this app exists to avoid.

use image::{DynamicImage, ImageFormat, ImageReader};
use std::io::Cursor;
use std::path::Path;

/// Longest edge of a generated thumbnail. Small on purpose: these are
/// recognition aids in a list, not a gallery, and every byte crosses IPC.
pub const DEFAULT_MAX_PX: u32 = 128;

/// Files larger than this are not decoded. A 200 MB TIFF is not worth the wait
/// for a 128px preview.
pub const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;

/// Ceiling on decoded pixel data, regardless of the file's size on disk.
const MAX_DECODE_ALLOC: u64 = 192 * 1024 * 1024;
const MAX_DECODE_EDGE: u32 = 24_000;

const JPEG_QUALITY: u8 = 78;

#[derive(Debug)]
pub enum ThumbError {
    /// Not an image, or a format this build does not decode.
    Unsupported,
    TooLarge(u64),
    Io(String),
    Decode(String),
}

impl std::fmt::Display for ThumbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThumbError::Unsupported => write!(f, "no preview available for this format"),
            ThumbError::TooLarge(bytes) => write!(f, "too large to preview ({bytes} bytes)"),
            ThumbError::Io(message) => write!(f, "{message}"),
            ThumbError::Decode(message) => write!(f, "could not be read as an image: {message}"),
        }
    }
}

pub struct Thumb {
    pub data: Vec<u8>,
    pub mime: &'static str,
    pub width: u32,
    pub height: u32,
}

/// Can this extension plausibly be decoded? Cheap pre-filter so the caller can
/// avoid opening files that will certainly fail.
pub fn is_previewable_ext(ext: Option<&str>) -> bool {
    match ext {
        Some(
            "png" | "jpg" | "jpeg" | "jpe" | "jfif" | "gif" | "bmp" | "ico" | "tif" | "tiff"
            | "webp" | "qoi",
        ) => true,
        // Camera RAW, previewed from the JPEG the camera embedded. See `raw`.
        Some(other) => crate::core::raw::is_raw_ext(other),
        None => false,
    }
}

/// Decode `path` and produce a thumbnail no larger than `max_px` on its
/// longest edge.
pub fn generate(path: &Path, max_px: u32) -> Result<Thumb, ThumbError> {
    let metadata = std::fs::metadata(crate::core::identity::for_io(path))
        .map_err(|err| ThumbError::Io(err.to_string()))?;
    if metadata.len() > MAX_SOURCE_BYTES {
        return Err(ThumbError::TooLarge(metadata.len()));
    }
    if metadata.len() == 0 {
        return Err(ThumbError::Unsupported);
    }

    // Camera RAW has to be handled before the generic decoder gets a look in.
    // A CR2 or DNG *is* a TIFF as far as format sniffing is concerned, so the
    // decoder would happily open one and then choke on the undemosaiced
    // sensor data inside. The embedded JPEG preview is the right source.
    let raw_ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase());
    if let Some(ext) = raw_ext.as_deref() {
        if crate::core::raw::is_raw_ext(ext) {
            return match crate::core::raw::extract_preview(path) {
                Some(jpeg) => decode_in_memory(&jpeg).and_then(|img| encode(img, max_px.clamp(32, 512))),
                // Some bodies and some converters write RAW with no preview at
                // all. There is nothing to show, and saying so is better than
                // handing the sensor data to a decoder that cannot read it.
                None => Err(ThumbError::Unsupported),
            };
        }
    }

    let reader = ImageReader::open(crate::core::identity::for_io(path))
        .map_err(|err| ThumbError::Io(err.to_string()))?
        .with_guessed_format()
        .map_err(|err| ThumbError::Io(err.to_string()))?;

    if reader.format().is_none() {
        return Err(ThumbError::Unsupported);
    }

    // Limits go on before `decode`, which is the only point at which they can
    // prevent the allocation rather than merely report it.
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    limits.max_image_width = Some(MAX_DECODE_EDGE);
    limits.max_image_height = Some(MAX_DECODE_EDGE);

    let mut reader = reader;
    reader.limits(limits);

    let decoded = reader
        .decode()
        .map_err(|err| ThumbError::Decode(err.to_string()))?;

    encode(decoded, max_px.clamp(32, 512))
}

/// Decode bytes already in memory - the embedded preview pulled out of a RAW
/// file - under the same limits as a decode straight from disk.
fn decode_in_memory(bytes: &[u8]) -> Result<DynamicImage, ThumbError> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|err| ThumbError::Io(err.to_string()))?;
    if reader.format().is_none() {
        return Err(ThumbError::Unsupported);
    }
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    limits.max_image_width = Some(MAX_DECODE_EDGE);
    limits.max_image_height = Some(MAX_DECODE_EDGE);
    reader.limits(limits);
    reader
        .decode()
        .map_err(|err| ThumbError::Decode(err.to_string()))
}

fn encode(image: DynamicImage, max_px: u32) -> Result<Thumb, ThumbError> {
    // `thumbnail` fits the image to the box in both directions, which means it
    // will happily *enlarge* something smaller - a 20x10 favicon would come
    // back as a blurry 128x64 that is bigger than the original file. Only
    // scale down.
    //
    // The filter is a box filter: markedly faster than Lanczos and
    // indistinguishable at this size.
    let scaled = if image.width() <= max_px && image.height() <= max_px {
        image
    } else {
        image.thumbnail(max_px, max_px)
    };
    let width = scaled.width();
    let height = scaled.height();

    let has_alpha = scaled.color().has_alpha();
    let mut buffer = Cursor::new(Vec::new());

    if has_alpha {
        // PNG, because flattening transparency onto an invented background
        // misrepresents the file - a logo on white looks like a different image.
        scaled
            .write_to(&mut buffer, ImageFormat::Png)
            .map_err(|err| ThumbError::Decode(err.to_string()))?;
        Ok(Thumb {
            data: buffer.into_inner(),
            mime: "image/png",
            width,
            height,
        })
    } else {
        // JPEG for photographs: roughly a fifth of the PNG size at this
        // resolution, and these all cross an IPC boundary.
        let rgb = scaled.to_rgb8();
        let mut encoder =
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buffer, JPEG_QUALITY);
        encoder
            .encode_image(&rgb)
            .map_err(|err| ThumbError::Decode(err.to_string()))?;
        Ok(Thumb {
            data: buffer.into_inner(),
            mime: "image/jpeg",
            width,
            height,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Temp(PathBuf);

    impl Temp {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "cleanup-thumb-{tag}-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Temp(dir)
        }

        /// A real PNG, written through the same library that will read it.
        fn png(&self, name: &str, w: u32, h: u32, alpha: bool) -> PathBuf {
            let path = self.0.join(name);
            if alpha {
                let buf = image::RgbaImage::from_fn(w, h, |x, y| {
                    image::Rgba([(x % 256) as u8, (y % 256) as u8, 128, 200])
                });
                DynamicImage::ImageRgba8(buf).save(&path).unwrap();
            } else {
                let buf = image::RgbImage::from_fn(w, h, |x, y| {
                    image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
                });
                DynamicImage::ImageRgb8(buf).save(&path).unwrap();
            }
            path
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn makes_a_thumbnail_that_fits_the_box_and_keeps_its_shape() {
        let t = Temp::new("basic");
        let wide = t.png("wide.png", 400, 100, false);

        let thumb = generate(&wide, 128).unwrap();
        assert!(thumb.width <= 128 && thumb.height <= 128);
        assert_eq!(thumb.width, 128, "the long edge should fill the box");
        assert_eq!(thumb.height, 32, "4:1 in, 4:1 out");
        assert!(!thumb.data.is_empty());
    }

    #[test]
    fn transparency_is_preserved_rather_than_flattened() {
        let t = Temp::new("alpha");
        let logo = t.png("logo.png", 64, 64, true);
        let thumb = generate(&logo, 128).unwrap();
        assert_eq!(thumb.mime, "image/png", "alpha must not become a JPEG");
    }

    #[test]
    fn photographs_become_jpeg_and_stay_small() {
        let t = Temp::new("photo");
        let photo = t.png("photo.png", 800, 600, false);
        let thumb = generate(&photo, 128).unwrap();
        assert_eq!(thumb.mime, "image/jpeg");
        assert!(
            thumb.data.len() < 24_000,
            "a 128px preview crossing IPC should be small, was {}",
            thumb.data.len()
        );
    }

    #[test]
    fn an_image_smaller_than_the_box_is_not_enlarged() {
        let t = Temp::new("small");
        let tiny = t.png("tiny.png", 20, 10, false);
        let thumb = generate(&tiny, 128).unwrap();
        assert_eq!((thumb.width, thumb.height), (20, 10));
    }

    #[test]
    fn a_file_that_is_not_an_image_is_rejected_cleanly() {
        let t = Temp::new("notimage");
        let path = t.0.join("notes.txt");
        std::fs::write(&path, b"this is plainly not an image at all").unwrap();
        assert!(matches!(
            generate(&path, 128),
            Err(ThumbError::Unsupported) | Err(ThumbError::Decode(_))
        ));
    }

    #[test]
    fn a_truncated_image_fails_instead_of_panicking() {
        let t = Temp::new("truncated");
        let good = t.png("good.png", 64, 64, false);
        let bytes = std::fs::read(&good).unwrap();
        let broken = t.0.join("broken.png");
        // A real PNG header with the body cut off - the shape of a partial
        // download that slipped past the flags.
        std::fs::write(&broken, &bytes[..bytes.len() / 3]).unwrap();
        assert!(generate(&broken, 128).is_err());
    }

    #[test]
    fn an_empty_file_is_rejected_without_opening_a_decoder() {
        let t = Temp::new("empty");
        let path = t.0.join("empty.png");
        std::fs::write(&path, b"").unwrap();
        assert!(matches!(generate(&path, 128), Err(ThumbError::Unsupported)));
    }

    #[test]
    fn a_missing_file_is_an_error_not_a_panic() {
        let t = Temp::new("missing");
        assert!(matches!(
            generate(&t.0.join("nope.png"), 128),
            Err(ThumbError::Io(_))
        ));
    }

    #[test]
    fn a_cr2_is_previewed_from_its_embedded_jpeg() {
        // End to end: the RAW container goes in, a thumbnail comes out. The
        // sensor data is never decoded - see `core::raw`.
        let t = Temp::new("cr2");
        let jpeg = {
            let img = image::RgbImage::from_fn(600, 400, |x, y| {
                image::Rgb([(x % 255) as u8, (y % 255) as u8, 90])
            });
            let mut out = Cursor::new(Vec::new());
            DynamicImage::ImageRgb8(img)
                .write_to(&mut out, ImageFormat::Jpeg)
                .unwrap();
            out.into_inner()
        };

        // Minimal CR2-shaped TIFF: IFD0 with a JPEG-compressed strip.
        let path = t.0.join("IMG_0042.CR2");
        let mut file: Vec<u8> = Vec::new();
        file.extend_from_slice(b"II");
        file.extend_from_slice(&42u16.to_le_bytes());
        file.extend_from_slice(&16u32.to_le_bytes());
        file.extend_from_slice(b"CR");
        file.push(2);
        file.push(0);
        file.extend_from_slice(&0u32.to_le_bytes());
        let jpeg_at: u32 = 16 + 2 + 3 * 12 + 4;
        file.extend_from_slice(&3u16.to_le_bytes());
        for (tag, kind, count, value) in [
            (0x0103u16, 3u16, 1u32, 6u32),               // Compression = JPEG
            (0x0111u16, 4u16, 1u32, jpeg_at),            // StripOffsets
            (0x0117u16, 4u16, 1u32, jpeg.len() as u32),  // StripByteCounts
        ] {
            file.extend_from_slice(&tag.to_le_bytes());
            file.extend_from_slice(&kind.to_le_bytes());
            file.extend_from_slice(&count.to_le_bytes());
            file.extend_from_slice(&value.to_le_bytes());
        }
        file.extend_from_slice(&0u32.to_le_bytes());
        file.extend_from_slice(&jpeg);
        std::fs::write(&path, &file).unwrap();

        let thumb = generate(&path, 128).expect("a CR2 with a preview must produce a thumbnail");
        assert_eq!((thumb.width, thumb.height), (128, 85));
        assert_eq!(thumb.mime, "image/jpeg");
    }

    #[test]
    fn a_raw_file_with_no_embedded_preview_says_so_instead_of_guessing() {
        let t = Temp::new("cr2-nopreview");
        let path = t.0.join("broken.cr2");
        // A valid TIFF header, but no preview anywhere in it.
        let mut file: Vec<u8> = Vec::new();
        file.extend_from_slice(b"II");
        file.extend_from_slice(&42u16.to_le_bytes());
        file.extend_from_slice(&8u32.to_le_bytes());
        file.extend_from_slice(&0u16.to_le_bytes());
        file.extend_from_slice(&0u32.to_le_bytes());
        std::fs::write(&path, &file).unwrap();

        assert!(matches!(generate(&path, 128), Err(ThumbError::Unsupported)));
    }

    #[test]
    fn the_extension_prefilter_matches_what_can_actually_be_decoded() {
        assert!(is_previewable_ext(Some("png")));
        assert!(is_previewable_ext(Some("jpg")));
        assert!(is_previewable_ext(Some("webp")));
        // Not built in: iPhone photos need a HEIF decoder, which would mean a
        // native dependency. Better to say no than to fail per file.
        assert!(!is_previewable_ext(Some("heic")));
        // Canon RAW, via the camera's own embedded JPEG.
        assert!(is_previewable_ext(Some("cr2")));
        assert!(is_previewable_ext(Some("nef")));
        // CR3 is not a TIFF container, so there is nothing to extract.
        assert!(!is_previewable_ext(Some("cr3")));
        assert!(!is_previewable_ext(Some("pdf")));
        assert!(!is_previewable_ext(Some("mp4")));
        assert!(!is_previewable_ext(None));
    }
}
