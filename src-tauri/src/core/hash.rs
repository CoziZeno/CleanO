//! Content hashing.
//!
//! Two shapes, because the dedupe pipeline escalates:
//!
//! * `head` - BLAKE3 over the first N bytes. One block read, used to eliminate
//!   same-size files that are not actually the same. Different files of
//!   identical size almost never share a 64 KiB prefix.
//! * `full` - BLAKE3 over everything, streamed in 1 MiB chunks so memory stays
//!   bounded at `concurrency x CHUNK` regardless of file size, and cancellable
//!   between chunks so stopping a run on a 40 GB file takes milliseconds.
//!
//! BLAKE3 rather than SHA-256: three to ten times the throughput thanks to
//! SIMD, while still being a 256-bit cryptographic digest. A collision is a
//! ~2^-128 event, so "equal size and equal hash" is treated as identical
//! without further qualification. Users who want absolute certainty can turn on
//! `hash.verifyBytewise`, which byte-compares a group before it can be acted
//! on; `verify_identical` below implements that.

use crate::core::model::FileIdent;
use crate::core::progress::CancelToken;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::UNIX_EPOCH;

/// Streaming chunk size. Large enough to keep the disk busy, small enough that
/// cancellation is responsive and memory per worker is predictable.
const CHUNK: usize = 1024 * 1024;

pub const DEFAULT_HEAD_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hashed {
    pub hash: [u8; 32],
    /// Size as seen on the open handle, not from the earlier scan.
    pub size: u64,
    pub mtime_ns: i64,
    pub ident: Option<FileIdent>,
    /// True when the digest covers every byte, so it is safe to cache as a
    /// full-content hash. A `head` result on a file larger than the window is
    /// a prefix digest and must never be stored as one.
    pub covers_whole_file: bool,
}

/// BLAKE3 over the first `head_bytes` bytes.
///
/// When the file is no larger than the window, the result covers every byte and
/// `covers_whole_file` is set - which lets the pipeline skip the full pass
/// entirely for small files, and cache the result as a real content hash.
pub fn head(path: &Path, head_bytes: usize) -> io::Result<Hashed> {
    let file = File::open(crate::core::identity::for_io(path))?;
    let meta = file.metadata()?;
    let size = meta.len();
    let window = head_bytes.max(1);

    let mut hasher = blake3::Hasher::new();
    let mut remaining = window.min(usize::try_from(size).unwrap_or(usize::MAX));
    let mut buffer = vec![0u8; CHUNK.min(window.max(1))];
    let mut reader = file;
    let mut read_total: u64 = 0;

    while remaining > 0 {
        let want = remaining.min(buffer.len());
        let got = reader.read(&mut buffer[..want])?;
        if got == 0 {
            break;
        }
        hasher.update(&buffer[..got]);
        read_total += got as u64;
        remaining -= got;
    }

    Ok(Hashed {
        hash: *hasher.finalize().as_bytes(),
        size,
        mtime_ns: mtime_nanos(&meta),
        ident: identity_of(&reader),
        covers_whole_file: read_total >= size,
    })
}

/// BLAKE3 over the whole file, streamed and cancellable.
///
/// Returns `Ok(None)` when cancelled, which the caller must treat as "unknown",
/// never as "different". Fails if the file changes size while being read: a
/// digest of a moving target is worse than no digest, because it would be
/// cached and later believed.
pub fn full(path: &Path, cancel: &CancelToken) -> io::Result<Option<Hashed>> {
    let mut file = File::open(crate::core::identity::for_io(path))?;
    let meta = file.metadata()?;
    let size_before = meta.len();
    let ident = identity_of(&file);

    file.seek(SeekFrom::Start(0))?;

    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; CHUNK];
    let mut total: u64 = 0;

    loop {
        if cancel.is_cancelled() {
            return Ok(None);
        }
        let got = file.read(&mut buffer)?;
        if got == 0 {
            break;
        }
        hasher.update(&buffer[..got]);
        total += got as u64;
    }

    // Re-stat the same handle. If something wrote to the file while we were
    // reading it, the digest describes neither the old nor the new contents.
    let after = file.metadata()?;
    if after.len() != size_before || total != size_before {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "file changed while it was being read",
        ));
    }

    Ok(Some(Hashed {
        hash: *hasher.finalize().as_bytes(),
        size: size_before,
        mtime_ns: mtime_nanos(&after),
        ident,
        covers_whole_file: true,
    }))
}

/// Byte-for-byte comparison, for the opt-in `verifyBytewise` setting.
///
/// Reads both files in lockstep and stops at the first difference, so a
/// mismatch early in two large files costs one block read each.
pub fn verify_identical(a: &Path, b: &Path, cancel: &CancelToken) -> io::Result<bool> {
    let mut left = File::open(crate::core::identity::for_io(a))?;
    let mut right = File::open(crate::core::identity::for_io(b))?;

    if left.metadata()?.len() != right.metadata()?.len() {
        return Ok(false);
    }

    let mut lbuf = vec![0u8; CHUNK];
    let mut rbuf = vec![0u8; CHUNK];

    loop {
        if cancel.is_cancelled() {
            return Ok(false);
        }
        let ln = read_fully(&mut left, &mut lbuf)?;
        let rn = read_fully(&mut right, &mut rbuf)?;
        if ln != rn {
            return Ok(false);
        }
        if ln == 0 {
            return Ok(true);
        }
        if lbuf[..ln] != rbuf[..rn] {
            return Ok(false);
        }
    }
}

/// `Read::read` may return a short read for reasons other than EOF, which
/// would desynchronise a lockstep comparison. This fills the buffer unless the
/// file genuinely ended.
fn read_fully(file: &mut File, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

fn mtime_nanos(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

// ------------------------------------------------------------- identity ---
//
// Read from the handle we already hold. This is the piece the metadata walk
// deliberately skips on Windows, because getting it there costs a file handle
// per entry - but here the handle exists anyway, so it is free, and this is the
// only place it matters: telling two hardlinks apart from two real copies.

#[cfg(windows)]
fn identity_of(file: &File) -> Option<FileIdent> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: `file` is an open handle for the duration of the call, and
    // `info` is a valid, correctly sized, exclusively borrowed output buffer.
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) };
    if ok == 0 {
        return None;
    }
    Some(FileIdent {
        volume: info.dwVolumeSerialNumber as u64,
        file_id: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
    })
}

#[cfg(unix)]
fn identity_of(file: &File) -> Option<FileIdent> {
    use std::os::unix::fs::MetadataExt;
    let meta = file.metadata().ok()?;
    Some(FileIdent {
        volume: meta.dev(),
        file_id: meta.ino(),
    })
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
                "cleanup-hash-{tag}-{}",
                std::time::SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Temp(dir)
        }
        fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            let mut f = File::create(&path).unwrap();
            f.write_all(bytes).unwrap();
            path
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn identical_contents_hash_the_same_regardless_of_name() {
        let t = Temp::new("same");
        let a = t.write("alpha.bin", b"the same bytes exactly");
        let b = t.write("completely-different-name.dat", b"the same bytes exactly");

        let ha = full(&a, &CancelToken::new()).unwrap().unwrap();
        let hb = full(&b, &CancelToken::new()).unwrap().unwrap();
        assert_eq!(ha.hash, hb.hash, "a different filename is not a different file");
    }

    #[test]
    fn one_byte_of_difference_changes_the_hash() {
        let t = Temp::new("diff");
        let a = t.write("a.bin", b"payload A");
        let b = t.write("b.bin", b"payload B");
        let ha = full(&a, &CancelToken::new()).unwrap().unwrap();
        let hb = full(&b, &CancelToken::new()).unwrap().unwrap();
        assert_ne!(ha.hash, hb.hash);
    }

    #[test]
    fn a_small_file_head_hash_covers_the_whole_file() {
        let t = Temp::new("small");
        let path = t.write("tiny.txt", b"only a few bytes");

        let h = head(&path, DEFAULT_HEAD_BYTES).unwrap();
        assert!(
            h.covers_whole_file,
            "a file smaller than the window needs no second pass"
        );

        // And it must agree with the full pass, since that is the basis for
        // skipping the full pass and for caching the result.
        let f = full(&path, &CancelToken::new()).unwrap().unwrap();
        assert_eq!(h.hash, f.hash);
    }

    #[test]
    fn a_large_file_head_hash_is_only_a_prefix() {
        let t = Temp::new("large");
        let mut bytes = vec![1u8; 4096];
        bytes.extend(std::iter::repeat(2u8).take(4096));
        let path = t.write("big.bin", &bytes);

        let h = head(&path, 1024).unwrap();
        assert!(!h.covers_whole_file);
        assert_eq!(h.size, 8192);

        let f = full(&path, &CancelToken::new()).unwrap().unwrap();
        assert_ne!(h.hash, f.hash, "a prefix digest is not a content digest");
    }

    #[test]
    fn files_sharing_a_prefix_are_separated_by_the_full_pass() {
        // This is the case the head stage cannot resolve and must not claim to.
        let t = Temp::new("prefix");
        let mut a = vec![9u8; 2048];
        let mut b = a.clone();
        a.push(0);
        b.push(1);
        let pa = t.write("a.bin", &a);
        let pb = t.write("b.bin", &b);

        assert_eq!(
            head(&pa, 1024).unwrap().hash,
            head(&pb, 1024).unwrap().hash,
            "identical prefixes, so the head stage cannot tell them apart"
        );
        assert_ne!(
            full(&pa, &CancelToken::new()).unwrap().unwrap().hash,
            full(&pb, &CancelToken::new()).unwrap().unwrap().hash,
        );
    }

    #[test]
    fn cancellation_returns_none_rather_than_a_partial_hash() {
        let t = Temp::new("cancel");
        let path = t.write("data.bin", &vec![0u8; 4096]);
        let cancel = CancelToken::new();
        cancel.cancel();
        assert_eq!(full(&path, &cancel).unwrap(), None);
    }

    #[test]
    fn an_empty_file_hashes_without_complaint() {
        let t = Temp::new("empty");
        let path = t.write("nothing.bin", b"");
        let h = full(&path, &CancelToken::new()).unwrap().unwrap();
        assert_eq!(h.size, 0);
        assert!(h.covers_whole_file);
    }

    #[test]
    fn a_missing_file_is_an_error_not_a_hash() {
        let t = Temp::new("missing");
        assert!(full(&t.0.join("nope.bin"), &CancelToken::new()).is_err());
        assert!(head(&t.0.join("nope.bin"), 1024).is_err());
    }

    #[test]
    fn identity_is_available_and_distinguishes_two_files() {
        let t = Temp::new("ident");
        let a = t.write("one.bin", b"x");
        let b = t.write("two.bin", b"x");

        let ia = full(&a, &CancelToken::new()).unwrap().unwrap().ident;
        let ib = full(&b, &CancelToken::new()).unwrap().unwrap().ident;

        // Every platform we support can report this from an open handle. If it
        // ever returns None, hardlink collapse silently stops working, so the
        // guarantee is asserted rather than assumed.
        let ia = ia.expect("identity must be readable from an open handle");
        let ib = ib.expect("identity must be readable from an open handle");
        assert_ne!(ia, ib, "two separate files are two identities");
        assert_eq!(ia.volume, ib.volume, "same volume, same serial");
    }

    #[test]
    fn hardlinked_paths_report_one_identity() {
        let t = Temp::new("hardlink");
        let original = t.write("original.bin", b"shared content");
        let link = t.0.join("link.bin");
        if std::fs::hard_link(&original, &link).is_err() {
            // Some filesystems refuse hardlinks; nothing to assert then.
            return;
        }

        let a = full(&original, &CancelToken::new()).unwrap().unwrap();
        let b = full(&link, &CancelToken::new()).unwrap().unwrap();
        assert_eq!(a.hash, b.hash, "same bytes");
        assert_eq!(
            a.ident, b.ident,
            "one file reached by two paths is not two copies to delete"
        );
    }

    #[test]
    fn bytewise_verification_agrees_with_hashing() {
        let t = Temp::new("verify");
        let a = t.write("a.bin", &vec![5u8; 300_000]);
        let b = t.write("b.bin", &vec![5u8; 300_000]);
        let c = t.write("c.bin", &vec![6u8; 300_000]);

        let cancel = CancelToken::new();
        assert!(verify_identical(&a, &b, &cancel).unwrap());
        assert!(!verify_identical(&a, &c, &cancel).unwrap());
    }

    #[test]
    fn bytewise_verification_rejects_different_lengths_immediately() {
        let t = Temp::new("verify-len");
        let a = t.write("a.bin", b"short");
        let b = t.write("b.bin", b"much longer content");
        assert!(!verify_identical(&a, &b, &CancelToken::new()).unwrap());
    }
}
