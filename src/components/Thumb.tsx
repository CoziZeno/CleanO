import { useState } from "react";
import type { FileRow } from "@/ipc/types";
import { CATEGORY_COLORS, isPreviewable } from "@/lib/categories";
import { useThumbStore } from "@/state/thumbStore";

/**
 * A file's preview, falling back to its category colour.
 *
 * The fallback is not a placeholder waiting to be replaced - most files are
 * not images and never will have one, so the coloured dot is the real answer
 * for them. Only files that could have a preview get the loading treatment,
 * which keeps the list from shimmering with pending states that will never
 * resolve.
 */
export function Thumb({ row, size = 30 }: { row: FileRow; size?: number }) {
  const entry = useThumbStore((state) => state.entries.get(row.id));
  const enabled = useThumbStore((state) => state.enabled);
  const [broken, setBroken] = useState(false);

  const couldHavePreview = enabled && isPreviewable(row);
  const colour = CATEGORY_COLORS[row.category];

  if (entry?.data && !broken) {
    return (
      <span className="thumb" style={{ width: size, height: size }}>
        <img
          src={entry.data}
          alt=""
          loading="lazy"
          decoding="async"
          onError={() => setBroken(true)}
        />
      </span>
    );
  }

  return (
    <span
      className={`thumb thumb-fallback ${couldHavePreview && !entry ? "pending" : ""}`}
      style={{ width: size, height: size }}
      title={entry?.reason ?? undefined}
    >
      <span className="thumb-dot" style={{ background: colour }} />
    </span>
  );
}
