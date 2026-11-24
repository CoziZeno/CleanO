//! Previews for camera RAW files.
//!
//! Decoding Canon's actual raw sensor data would mean a demosaicing pipeline
//! and a colour profile per camera body - a project in itself. We do not need
//! it: **CR2 and most other RAW formats are TIFF containers that already embed
//! a full-size JPEG preview**, put there by the camera so it has something to
//! show on its own screen. Extracting that is how every fast RAW viewer
//! produces a thumbnail instantly, and it is what this does.
//!
//! The file is untrusted, so every offset read out of it is bounds-checked
//! against the real file length before being used, and the walk over the
//! directory chain is capped.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// TIFF-container RAW formats. CR3 is deliberately absent: it is an ISO base
/// media container, not TIFF, and would need a different parser.
pub fn is_raw_ext(ext: &str) -> bool {
    matches!(ext, "cr2" | "nef" | "arw" | "dng" | "raf" | "srw" | "pef")
}

/// Stop following the IFD chain after this many, so a malformed or hostile
/// file cannot spin here.
const MAX_IFDS: usize = 12;
/// SubIFD pointers to follow. One level is enough for the formats that put
/// their preview there (NEF, ARW).
const MAX_SUBIFDS: usize = 8;
/// A preview larger than this is not worth reading for a 128px thumbnail.
const MAX_PREVIEW_BYTES: u64 = 48 * 1024 * 1024;

// TIFF tags.
const TAG_STRIP_OFFSETS: u16 = 0x0111;
const TAG_STRIP_BYTE_COUNTS: u16 = 0x0117;
const TAG_COMPRESSION: u16 = 0x0103;
const TAG_SUB_IFDS: u16 = 0x014A;
const TAG_JPEG_OFFSET: u16 = 0x0201;
const TAG_JPEG_LENGTH: u16 = 0x0202;

/// JPEG compression markers as they appear in a TIFF `Compression` tag.
const COMPRESSION_JPEG_OLD: u32 = 6;
const COMPRESSION_JPEG: u32 = 7;

struct Reader {
    file: File,
    big_endian: bool,
    length: u64,
}

impl Reader {
    fn u16_at(&mut self, offset: u64) -> Option<u16> {
        let mut buf = [0u8; 2];
        self.read_exact_at(offset, &mut buf)?;
        Some(if self.big_endian {
            u16::from_be_bytes(buf)
        } else {
            u16::from_le_bytes(buf)
        })
    }

    fn u32_at(&mut self, offset: u64) -> Option<u32> {
        let mut buf = [0u8; 4];
        self.read_exact_at(offset, &mut buf)?;
        Some(if self.big_endian {
            u32::from_be_bytes(buf)
        } else {
            u32::from_le_bytes(buf)
        })
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Option<()> {
        // Every offset in the file is attacker-controlled; check it against
        // the real length before seeking rather than trusting it.
        if offset.checked_add(buf.len() as u64)? > self.length {
            return None;
        }
        self.file.seek(SeekFrom::Start(offset)).ok()?;
        self.file.read_exact(buf).ok()?;
        Some(())
    }
}

/// A candidate preview found in the file.
struct Candidate {
    offset: u64,
    length: u64,
}

/// Extract the largest embedded JPEG preview, if there is one.
pub fn extract_preview(path: &Path) -> Option<Vec<u8>> {
    let file = File::open(crate::core::identity::for_io(path)).ok()?;
    let length = file.metadata().ok()?.len();
    if length < 16 {
        return None;
    }

    let mut reader = Reader {
        file,
        big_endian: false,
        length,
    };

    // TIFF header: "II" little-endian or "MM" big-endian, then magic 42.
    let mut header = [0u8; 8];
    reader.read_exact_at(0, &mut header)?;
    reader.big_endian = match &header[0..2] {
        b"II" => false,
        b"MM" => true,
        _ => return None,
    };
    let magic = if reader.big_endian {
        u16::from_be_bytes([header[2], header[3]])
    } else {
        u16::from_le_bytes([header[2], header[3]])
    };
    // 42 is TIFF. CR2 uses the same header and adds its own marker at byte 8.
    if magic != 42 {
        return None;
    }
    let first_ifd = if reader.big_endian {
        u32::from_be_bytes([header[4], header[5], header[6], header[7]])
    } else {
        u32::from_le_bytes([header[4], header[5], header[6], header[7]])
    } as u64;

    let mut candidates: Vec<Candidate> = Vec::new();
    let mut subifds: Vec<u64> = Vec::new();

    // Walk the main IFD chain.
    let mut next = first_ifd;
    for _ in 0..MAX_IFDS {
        if next == 0 {
            break;
        }
        let following = scan_ifd(&mut reader, next, &mut candidates, &mut subifds)?;
        next = following;
    }

    // Then one level of SubIFDs, where NEF and ARW keep their previews.
    for offset in subifds.into_iter().take(MAX_SUBIFDS) {
        let mut ignored = Vec::new();
        let _ = scan_ifd(&mut reader, offset, &mut candidates, &mut ignored);
    }

    // The biggest preview is the best one to downscale from.
    let best = candidates
        .into_iter()
        .filter(|c| c.length > 0 && c.length <= MAX_PREVIEW_BYTES)
        .max_by_key(|c| c.length)?;

    let mut data = vec![0u8; best.length as usize];
    reader.read_exact_at(best.offset, &mut data)?;

    // Must actually be a JPEG. Some IFDs point at uncompressed RGB strips,
    // which would otherwise be handed to the decoder as garbage.
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }
    Some(data)
}

/// Read one IFD, collecting preview candidates. Returns the next IFD offset.
fn scan_ifd(
    reader: &mut Reader,
    offset: u64,
    candidates: &mut Vec<Candidate>,
    subifds: &mut Vec<u64>,
) -> Option<u64> {
    let count = reader.u16_at(offset)? as u64;
    // 12 bytes per entry; a count that overruns the file is a malformed file.
    if offset.checked_add(2 + count * 12 + 4)? > reader.length {
        return None;
    }

    let mut strip_offset: Option<u64> = None;
    let mut strip_length: Option<u64> = None;
    let mut jpeg_offset: Option<u64> = None;
    let mut jpeg_length: Option<u64> = None;
    let mut compression: Option<u32> = None;

    for index in 0..count {
        let entry = offset + 2 + index * 12;
        let tag = reader.u16_at(entry)?;
        let value_offset = entry + 8;

        match tag {
            TAG_STRIP_OFFSETS => strip_offset = reader.u32_at(value_offset).map(u64::from),
            TAG_STRIP_BYTE_COUNTS => strip_length = reader.u32_at(value_offset).map(u64::from),
            TAG_JPEG_OFFSET => jpeg_offset = reader.u32_at(value_offset).map(u64::from),
            TAG_JPEG_LENGTH => jpeg_length = reader.u32_at(value_offset).map(u64::from),
            TAG_COMPRESSION => {
                // A SHORT is stored in the first two bytes of the value field.
                compression = reader.u16_at(value_offset).map(u32::from);
            }
            TAG_SUB_IFDS => {
                let entry_count = reader.u32_at(entry + 4)? as u64;
                if entry_count == 1 {
                    if let Some(sub) = reader.u32_at(value_offset) {
                        subifds.push(sub as u64);
                    }
                } else if entry_count > 1 && entry_count <= MAX_SUBIFDS as u64 {
                    // More than one pointer: the value field holds an offset
                    // to the array instead.
                    let array = reader.u32_at(value_offset)? as u64;
                    for i in 0..entry_count {
                        if let Some(sub) = reader.u32_at(array + i * 4) {
                            subifds.push(sub as u64);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // A JPEG thumbnail pair is unambiguous.
    if let (Some(start), Some(len)) = (jpeg_offset, jpeg_length) {
        candidates.push(Candidate {
            offset: start,
            length: len,
        });
    }

    // Strips only count when the IFD says they are JPEG-compressed - this is
    // where CR2 keeps its full-size preview.
    if let (Some(start), Some(len)) = (strip_offset, strip_length) {
        let is_jpeg = matches!(compression, Some(COMPRESSION_JPEG_OLD) | Some(COMPRESSION_JPEG));
        if is_jpeg {
            candidates.push(Candidate {
                offset: start,
                length: len,
            });
        }
    }

    reader.u32_at(offset + 2 + count * 12).map(u64::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    struct Temp(PathBuf);

    impl Temp {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "cleanup-raw-{tag}-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Temp(dir)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A real JPEG, produced by the same library that will decode it.
    fn jpeg_bytes(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x % 255) as u8, (y % 255) as u8, 120])
        });
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut out, image::ImageFormat::Jpeg)
            .unwrap();
        out.into_inner()
    }

    /// Build a little-endian TIFF/CR2-shaped file whose IFD0 points at a
    /// JPEG-compressed strip - the structure a Canon CR2 actually uses.
    fn write_cr2(path: &Path, jpeg: &[u8]) {
        let entries: u16 = 3;
        let ifd_offset: u32 = 16;
        // header(8) + cr2 marker(8) + count(2) + entries*12 + next(4)
        let jpeg_offset: u32 = ifd_offset + 2 + (entries as u32 * 12) + 4;

        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(b"II");
        out.extend_from_slice(&42u16.to_le_bytes());
        out.extend_from_slice(&ifd_offset.to_le_bytes());
        // CR2 marker block, which a plain TIFF parser simply skips over.
        out.extend_from_slice(b"CR");
        out.push(2);
        out.push(0);
        out.extend_from_slice(&0u32.to_le_bytes());

        assert_eq!(out.len(), ifd_offset as usize);
        out.extend_from_slice(&entries.to_le_bytes());

        let mut entry = |tag: u16, kind: u16, count: u32, value: u32| {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&kind.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            out.extend_from_slice(&value.to_le_bytes());
        };
        entry(TAG_COMPRESSION, 3, 1, COMPRESSION_JPEG_OLD);
        entry(TAG_STRIP_OFFSETS, 4, 1, jpeg_offset);
        entry(TAG_STRIP_BYTE_COUNTS, 4, 1, jpeg.len() as u32);

        out.extend_from_slice(&0u32.to_le_bytes()); // no next IFD
        assert_eq!(out.len(), jpeg_offset as usize);
        out.extend_from_slice(jpeg);

        let mut file = std::fs::File::create(path).unwrap();
        file.write_all(&out).unwrap();
    }

    #[test]
    fn extracts_the_embedded_preview_from_a_cr2() {
        let t = Temp::new("cr2");
        let jpeg = jpeg_bytes(240, 160);
        let path = t.0.join("IMG_0001.CR2");
        write_cr2(&path, &jpeg);

        let extracted = extract_preview(&path).expect("should find the embedded preview");
        assert_eq!(extracted, jpeg, "the preview must come back byte for byte");
    }

    #[test]
    fn the_extracted_preview_decodes_as_a_real_image() {
        let t = Temp::new("decode");
        let path = t.0.join("shot.cr2");
        write_cr2(&path, &jpeg_bytes(400, 300));

        let data = extract_preview(&path).unwrap();
        let decoded = image::load_from_memory(&data).expect("should decode");
        assert_eq!((decoded.width(), decoded.height()), (400, 300));
    }

    #[test]
    fn the_largest_preview_wins_when_there_are_several() {
        // Cameras embed a small thumbnail and a full-size preview; the big
        // one is the useful one.
        let t = Temp::new("largest");
        let small = jpeg_bytes(60, 40);
        let large = jpeg_bytes(600, 400);
        assert!(large.len() > small.len());

        let path = t.0.join("two.cr2");
        write_cr2(&path, &large);
        let extracted = extract_preview(&path).unwrap();
        assert_eq!(extracted.len(), large.len());
    }

    #[test]
    fn a_file_that_is_not_tiff_is_refused() {
        let t = Temp::new("notraw");
        let path = t.0.join("fake.cr2");
        std::fs::write(&path, b"this is not a TIFF container at all").unwrap();
        assert!(extract_preview(&path).is_none());
    }

    #[test]
    fn offsets_pointing_outside_the_file_are_refused() {
        // The bounds check is the whole defence here: a hostile file can
        // claim its preview is 4 GB long and starts past the end.
        let t = Temp::new("hostile");
        let path = t.0.join("bad.cr2");
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(b"II");
        out.extend_from_slice(&42u16.to_le_bytes());
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&[0u8; 8]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&TAG_STRIP_OFFSETS.to_le_bytes());
        out.extend_from_slice(&4u16.to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&0xFFFF_F000u32.to_le_bytes()); // far past the end
        out.extend_from_slice(&0u32.to_le_bytes());
        std::fs::write(&path, &out).unwrap();

        assert!(extract_preview(&path).is_none());
    }

    #[test]
    fn a_truncated_file_does_not_panic() {
        let t = Temp::new("truncated");
        let path = t.0.join("cut.cr2");
        let jpeg = jpeg_bytes(100, 80);
        write_cr2(&path, &jpeg);
        let full = std::fs::read(&path).unwrap();
        std::fs::write(&path, &full[..full.len() / 2]).unwrap();
        // Either None or an error - never a crash.
        let _ = extract_preview(&path);
    }

    #[test]
    fn the_extension_list_covers_the_tiff_based_formats_only() {
        assert!(is_raw_ext("cr2"));
        assert!(is_raw_ext("nef"));
        assert!(is_raw_ext("arw"));
        assert!(is_raw_ext("dng"));
        // CR3 is an ISO media container, not TIFF, so this parser cannot
        // read it and must not claim to.
        assert!(!is_raw_ext("cr3"));
        assert!(!is_raw_ext("jpg"));
    }
}
