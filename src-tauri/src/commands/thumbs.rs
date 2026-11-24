//! Thumbnail commands.
//!
//! The UI asks for previews of the rows it is currently showing, a batch at a
//! time. Three things keep that cheap and safe:
//!
//! * **The flags are checked here**, not in the UI. A cloud placeholder is
//!   never decoded - reading one downloads the whole file over the user's
//!   connection. Nor is a part-downloaded file, whose bytes are still moving.
//! * **The cache is consulted before any decode**, so scrolling back over rows
//!   you have already seen costs a database read rather than a decode.
//! * **Decoding happens on a blocking worker**, several at once, so a slow
//!   image cannot stall the window.

use crate::core::db::{CacheKey, CachedThumb};
use crate::core::model::EntryFlags;
use crate::core::thumb;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

use rayon::prelude::*;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;

/// One row's answer. `data` is a ready-to-use data URL; `reason` explains the
/// absence when there is none, so the UI can say why instead of showing an
/// empty box forever.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThumbResult {
    pub id: u32,
    pub data: Option<String>,
    pub width: u32,
    pub height: u32,
    pub reason: Option<String>,
}

/// What the worker needs, copied out from under the index lock.
struct Job {
    id: u32,
    path: PathBuf,
    key: Option<CacheKey>,
}

#[tauri::command]
pub async fn get_thumbnails(
    state: State<'_, Arc<AppState>>,
    ids: Vec<u32>,
    max_px: Option<u32>,
) -> AppResult<Vec<ThumbResult>> {
    let state = state.inner().clone();
    let max_px = max_px.unwrap_or(thumb::DEFAULT_MAX_PX).clamp(32, 512);
    tauri::async_runtime::spawn_blocking(move || build(&state, ids, max_px))
        .await
        .map_err(|err| AppError::Io(format!("the preview thread failed: {err}")))?
}

fn build(state: &AppState, ids: Vec<u32>, max_px: u32) -> AppResult<Vec<ThumbResult>> {
    // One short read lock: resolve ids to paths and decide what is eligible.
    let (jobs, mut refused) = {
        let index = state.index.read();
        let index = index.as_ref().ok_or(AppError::NoScan)?;

        let mut jobs = Vec::new();
        let mut refused = Vec::new();

        for id in ids {
            let Some(entry) = index.entry(id) else { continue };

            if entry.flags.has(EntryFlags::CLOUD_PLACEHOLDER) {
                refused.push(skip(id, "stored in the cloud, so it was not opened"));
                continue;
            }
            if entry.flags.has(EntryFlags::IN_PROGRESS) {
                refused.push(skip(id, "still downloading"));
                continue;
            }
            if !entry.flags.is_safe_to_read() {
                refused.push(skip(id, "not a readable file"));
                continue;
            }
            if !thumb::is_previewable_ext(entry.ext.as_deref()) {
                refused.push(skip(id, "no preview for this format"));
                continue;
            }
            if entry.size > thumb::MAX_SOURCE_BYTES {
                refused.push(skip(id, "too large to preview"));
                continue;
            }

            jobs.push(Job {
                id,
                path: index.path_of(entry),
                key: entry.ident.map(|ident| CacheKey {
                    ident,
                    size: entry.size,
                    mtime_ns: entry.modified.unwrap_or(0).saturating_mul(1_000_000_000),
                }),
            });
        }
        (jobs, refused)
    };

    // Cache first, single-threaded: SQLite reads are fast and this avoids
    // decoding anything already seen.
    let mut results = Vec::with_capacity(jobs.len() + refused.len());
    let mut to_decode = Vec::new();

    for job in jobs {
        let cached = job
            .key
            .as_ref()
            .and_then(|key| state.db.as_ref().and_then(|db| db.thumb_get(key, max_px)));
        match cached {
            Some(hit) => results.push(to_result(job.id, &hit)),
            None => to_decode.push(job),
        }
    }

    // Decode the misses in parallel.
    let decoded: Vec<(u32, Option<CacheKey>, Result<thumb::Thumb, thumb::ThumbError>)> = to_decode
        .into_par_iter()
        .map(|job| {
            let outcome = thumb::generate(&job.path, max_px);
            (job.id, job.key, outcome)
        })
        .collect();

    for (id, key, outcome) in decoded {
        match outcome {
            Ok(thumb) => {
                let cached = CachedThumb {
                    mime: thumb.mime.to_string(),
                    width: thumb.width,
                    height: thumb.height,
                    data: thumb.data,
                };
                if let (Some(key), Some(db)) = (key.as_ref(), state.db.as_ref()) {
                    // A cache write failure costs a re-decode next time and
                    // nothing else, so it is not worth failing the request.
                    let _ = db.thumb_put(key, max_px, &cached);
                }
                results.push(to_result(id, &cached));
            }
            Err(err) => results.push(skip(id, &err.to_string())),
        }
    }

    results.append(&mut refused);
    Ok(results)
}

fn to_result(id: u32, thumb: &CachedThumb) -> ThumbResult {
    ThumbResult {
        id,
        data: Some(format!(
            "data:{};base64,{}",
            thumb.mime,
            base64(&thumb.data)
        )),
        width: thumb.width,
        height: thumb.height,
        reason: None,
    }
}

fn skip(id: u32, reason: &str) -> ThumbResult {
    ThumbResult {
        id,
        data: None,
        width: 0,
        height: 0,
        reason: Some(reason.to_string()),
    }
}

/// Previews are tens of kilobytes and there are a few dozen per request, so a
/// dependency to encode them is not worth it.
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);

    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(TABLE[(triple >> 18) as usize & 0x3f] as char);
        out.push(TABLE[(triple >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(triple >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[triple as usize & 0x3f] as char
        } else {
            '='
        });
    }
    out
}

/// Size of the preview cache, for Settings.
#[tauri::command]
pub fn thumb_cache_usage(state: State<'_, Arc<AppState>>) -> (u64, u64) {
    state
        .db
        .as_ref()
        .map(|db| db.thumb_usage())
        .unwrap_or((0, 0))
}

#[tauri::command]
pub fn clear_thumb_cache(state: State<'_, Arc<AppState>>) -> AppResult<u64> {
    match state.db.as_ref() {
        Some(db) => db
            .thumb_clear()
            .map(|n| n as u64)
            .map_err(|err| AppError::Io(format!("could not clear the preview cache: {err}"))),
        None => Ok(0),
    }
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_matches_the_known_answers() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_handles_bytes_above_ascii() {
        // Padding and the high-bit table entries are where a hand-rolled
        // encoder usually goes wrong.
        assert_eq!(base64(&[0xff, 0xfe, 0xfd]), "//79");
        assert_eq!(base64(&[0x00, 0x00, 0x00]), "AAAA");
        assert_eq!(base64(&[0xfb, 0xff]), "+/8=");
    }
}
