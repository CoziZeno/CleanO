import { useEffect, useMemo, useState } from "react";
import { api, toIpcError, type IpcError } from "@/ipc/client";
import { CATEGORY_LABELS, type FileRow, type TreemapItem, type TreemapNode } from "@/ipc/types";
import { CATEGORY_COLORS } from "@/lib/categories";
import { formatBytes, formatCount, plural } from "@/lib/format";
import { useFileOpsStore } from "@/state/fileOpsStore";
import { Banner } from "@/components/Banner";
import { IconFolder } from "@/components/icons";

interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
  item: TreemapItem;
}

/**
 * Squarified treemap layout (Bruls, Huizing & van Wijk).
 *
 * The naive "slice and dice" alternative produces slivers — a 2px × 400px
 * rectangle is impossible to read or click. Squarifying keeps each rectangle
 * close to square by filling one row at a time and stopping that row the
 * moment adding another item would make its worst aspect ratio worse.
 *
 * Items must arrive sorted largest-first.
 */
function squarify(items: TreemapItem[], width: number, height: number): Rect[] {
  const total = items.reduce((sum, item) => sum + item.bytes, 0);
  if (total <= 0 || width <= 0 || height <= 0) return [];

  const out: Rect[] = [];
  // Area in pixels per byte, so a row's pixel area is comparable to its bytes.
  const scale = (width * height) / total;

  let x = 0;
  let y = 0;
  let remainingWidth = width;
  let remainingHeight = height;
  let index = 0;

  /** Worst aspect ratio in `row` if laid along a side of length `side`. */
  const worst = (row: number[], side: number, rowSum: number): number => {
    if (rowSum <= 0 || side <= 0) return Infinity;
    const max = row[0] ?? 0;
    const min = row[row.length - 1] ?? 0;
    const side2 = side * side;
    const sum2 = rowSum * rowSum;
    return Math.max((side2 * max) / sum2, sum2 / (side2 * min));
  };

  while (index < items.length) {
    const side = Math.min(remainingWidth, remainingHeight);
    const row: number[] = [];
    let rowSum = 0;
    let start = index;

    while (index < items.length) {
      const area = (items[index]?.bytes ?? 0) * scale;
      // A zero-byte item has no area to lay out; include it in the row so it
      // is still drawn (as a hairline) rather than silently dropped.
      const nextRow = [...row, area];
      const nextSum = rowSum + area;
      if (row.length > 0 && worst(nextRow, side, nextSum) > worst(row, side, rowSum)) break;
      row.push(area);
      rowSum = nextSum;
      index += 1;
    }

    if (row.length === 0) break;

    // Lay the row along the shorter side, which is what keeps shapes square.
    const horizontal = remainingWidth >= remainingHeight;
    const thickness = rowSum / side;
    let offset = 0;

    for (let i = 0; i < row.length; i += 1) {
      const item = items[start + i];
      if (!item) continue;
      const length = (row[i] ?? 0) / thickness;
      out.push(
        horizontal
          ? { x, y: y + offset, width: thickness, height: length, item }
          : { x: x + offset, y, width: length, height: thickness, item },
      );
      offset += length;
    }

    if (horizontal) {
      x += thickness;
      remainingWidth -= thickness;
    } else {
      y += thickness;
      remainingHeight -= thickness;
    }
    start = index;
  }

  return out;
}

/** Folders read as one neutral family so files' category colours stand out. */
function fillFor(item: TreemapItem): string {
  if (item.isRemainder) return "var(--surface-hover)";
  if (item.isDir) return "var(--accent-soft)";
  return CATEGORY_COLORS[item.category];
}

interface TreemapProps {
  /** Bumped by the caller when a new scan lands, to force a reload. */
  generation: number;
  onContextMenu: (x: number, y: number, row: FileRow) => void;
}

export function Treemap({ generation, onContextMenu }: TreemapProps) {
  const [node, setNode] = useState<TreemapNode | null>(null);
  const [path, setPath] = useState<string | undefined>(undefined);
  const [error, setError] = useState<IpcError | null>(null);
  const [hovered, setHovered] = useState<TreemapItem | null>(null);
  const [size, setSize] = useState<{ width: number; height: number } | null>(null);
  // A callback ref, not useRef: the element this measures does not exist on
  // the first render (the component shows a spinner until the first node
  // arrives), so an effect that reads a ref on mount finds null and never
  // runs again — which left the map blank.
  const [box, setBox] = useState<HTMLDivElement | null>(null);
  const ops = useFileOpsStore();

  // A new scan invalidates the path we had drilled into.
  useEffect(() => {
    setPath(undefined);
  }, [generation]);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const next = await api.getTreemap(path);
        if (!cancelled) {
          setNode(next);
          setError(null);
        }
      } catch (err) {
        if (!cancelled) {
          const ipc = toIpcError(err);
          setError(ipc.code === "noScan" ? null : ipc);
          setNode(null);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [path, generation]);

  // The layout needs real pixels, so it waits for the element to be measured
  // rather than guessing and reflowing.
  useEffect(() => {
    if (!box) return;
    // Measure immediately as well as on resize: ResizeObserver does fire once
    // on observe, but taking the size here means the first paint already has
    // it instead of waiting a frame.
    const measure = (width: number, height: number) => {
      if (width > 0 && height > 0) setSize({ width, height });
    };
    measure(box.clientWidth, box.clientHeight);
    const observer = new ResizeObserver((entries) => {
      const rect = entries[0]?.contentRect;
      if (rect) measure(rect.width, rect.height);
    });
    observer.observe(box);
    return () => observer.disconnect();
  }, [box]);

  const rects = useMemo(() => {
    if (!node || !size) return [];
    return squarify(node.items, size.width, size.height);
  }, [node, size]);

  const openItem = async (item: TreemapItem) => {
    if (item.isRemainder) return;
    if (item.isDir) {
      setPath(item.path);
      return;
    }
    if (item.id === null) return;
    const row = await rowFor(item.id);
    if (row) void ops.openFile(row);
  };

  if (error) {
    return (
      <Banner
        tone="danger"
        title="Could not build the storage map"
        text={error.message}
        hint={error.hint}
      />
    );
  }

  if (!node) {
    return (
      <div className="empty">
        <span className="spinner" />
      </div>
    );
  }

  return (
    <div className="treemap-wrap">
      <div className="treemap-bar">
        <div className="treemap-trail">
          {node.trail.map((step, index) => {
            const last = index === node.trail.length - 1;
            return (
              <span key={step.path} className="treemap-crumb">
                {index > 0 ? <span className="treemap-sep">/</span> : null}
                <button
                  className={`btn-ghost treemap-crumb-btn ${last ? "current" : ""}`}
                  disabled={last}
                  onClick={() => setPath(index === 0 ? undefined : step.path)}
                  title={step.path}
                >
                  {index === 0 ? <IconFolder size={12} /> : null}
                  {step.display}
                </button>
              </span>
            );
          })}
        </div>
        <span className="treemap-total muted">
          {formatBytes(node.bytes)} · {formatCount(node.files)} {plural(node.files, "file")}
        </span>
      </div>

      <div className="treemap" ref={setBox}>
        {node.items.length === 0 ? (
          <div className="treemap-empty muted">This folder holds nothing that was indexed.</div>
        ) : (
          rects.map((rect) => {
            // Sub-pixel rectangles get no label; drawing one would overflow
            // the box and sit on top of its neighbours.
            const roomy = rect.width > 54 && rect.height > 26;
            return (
              <button
                key={`${rect.item.path}:${rect.item.id ?? "d"}`}
                className={`treemap-cell ${rect.item.isDir ? "is-dir" : ""} ${
                  rect.item.isRemainder ? "is-remainder" : ""
                }`}
                style={{
                  left: rect.x,
                  top: rect.y,
                  width: Math.max(rect.width - 2, 1),
                  height: Math.max(rect.height - 2, 1),
                  background: fillFor(rect.item),
                }}
                onMouseEnter={() => setHovered(rect.item)}
                onMouseLeave={() => setHovered((current) => (current === rect.item ? null : current))}
                onClick={() => void openItem(rect.item)}
                onContextMenu={(event) => {
                  if (rect.item.isDir || rect.item.id === null) return;
                  event.preventDefault();
                  const { clientX, clientY, } = event;
                  void rowFor(rect.item.id).then((row) => {
                    if (row) onContextMenu(clientX, clientY, row);
                  });
                }}
                title={`${rect.item.name} — ${formatBytes(rect.item.bytes)}`}
                aria-label={`${rect.item.name}, ${formatBytes(rect.item.bytes)}${
                  rect.item.isDir ? ", folder" : ""
                }`}
              >
                {roomy ? (
                  <span className="treemap-label">
                    <span className="treemap-name">{rect.item.name}</span>
                    <span className="treemap-size">{formatBytes(rect.item.bytes)}</span>
                  </span>
                ) : null}
              </button>
            );
          })
        )}
      </div>

      <div className="treemap-foot">
        {hovered ? (
          <span className="treemap-hover">
            <span className="cat-dot" style={{ background: fillFor(hovered) }} />
            <span className="treemap-hover-name selectable">{hovered.name}</span>
            <span className="muted">
              {formatBytes(hovered.bytes)}
              {hovered.isDir
                ? ` · ${formatCount(hovered.files)} ${plural(hovered.files, "file")} · click to open`
                : hovered.isRemainder
                  ? " · too small to draw individually"
                  : ` · ${CATEGORY_LABELS[hovered.category]}`}
            </span>
          </span>
        ) : (
          <span className="muted">
            Each rectangle is sized by what it uses. Click a folder to go in
            {node.remainderCount > 0
              ? `; ${formatCount(node.remainderCount)} items too small to draw are grouped together.`
              : "."}
          </span>
        )}
      </div>
    </div>
  );
}

/**
 * Resolve a file id to a row.
 *
 * The map holds ids, not rows, because sending a full row for every rectangle
 * would multiply the payload for data most of them never need.
 */
async function rowFor(id: number): Promise<FileRow | null> {
  try {
    const rows = await api.rowsFor([id]);
    return rows[0] ?? null;
  } catch {
    return null;
  }
}
