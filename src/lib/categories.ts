import { isSafeToTouch, type Category, type FileRow } from "@/ipc/types";

/**
 * Formats the backend can decode. Kept in step with `is_previewable_ext` in
 * `core/thumb.rs`; asking for a preview the backend will refuse just wastes a
 * round trip.
 *
 * HEIC is absent on purpose — iPhone photos need a HEIF decoder, which would
 * mean a native dependency.
 */
const PREVIEWABLE = new Set([
  "png",
  "jpg",
  "jpeg",
  "jpe",
  "jfif",
  "gif",
  "bmp",
  "ico",
  "tif",
  "tiff",
  "webp",
  "qoi",
  // Camera RAW. These are not decoded as RAW — the backend pulls out the
  // full-size JPEG the camera embedded, which is what `core/raw.rs` does.
  // CR3 is absent because it is not a TIFF container, so there is nothing
  // to extract from it.
  "cr2",
  "nef",
  "arw",
  "dng",
  "raf",
  "srw",
  "pef",
]);

/** 64 MB, matching `thumb::MAX_SOURCE_BYTES`. */
const MAX_PREVIEW_BYTES = 64 * 1024 * 1024;

export function isPreviewable(row: FileRow): boolean {
  return (
    row.ext !== null &&
    PREVIEWABLE.has(row.ext) &&
    row.size > 0 &&
    row.size <= MAX_PREVIEW_BYTES &&
    // Cloud placeholders and part-downloads are refused by the backend too;
    // this just avoids the pointless request.
    isSafeToTouch(row.flags)
  );
}

/** Resolves to the `--cat-*` tokens in theme.css, so colours stay in one place. */
export const CATEGORY_COLORS: Record<Category, string> = {
  images: "var(--cat-images)",
  videos: "var(--cat-videos)",
  audio: "var(--cat-audio)",
  documents: "var(--cat-documents)",
  archives: "var(--cat-archives)",
  installers: "var(--cat-installers)",
  development: "var(--cat-development)",
  temporary: "var(--cat-temporary)",
  misc: "var(--cat-misc)",
};
