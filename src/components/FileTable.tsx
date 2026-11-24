import { useRef } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api } from "@/ipc/client";
import {
  CATEGORY_LABELS,
  isSafeToTouch,
  untouchableReason,
  type FileRow,
  type SortKey,
} from "@/ipc/types";
import { formatAbsolute, formatBytes, formatRelative } from "@/lib/format";
import { useFileOpsStore } from "@/state/fileOpsStore";
import { useQueueStore } from "@/state/queueStore";
import { useThumbStore } from "@/state/thumbStore";
import { Checkbox } from "./Checkbox";
import { Thumb } from "./Thumb";
import { IconArrowDown, IconArrowUp, IconReveal } from "./icons";

interface FileTableProps {
  /** Total matches across the whole index, which may exceed what is loaded. */
  total: number;
  /** Returns the row at an absolute index, or undefined while its page loads. */
  getRow: (index: number) => FileRow | undefined;
  /** Called with the visible index range so the caller can fetch pages. */
  onRangeChange: (start: number, end: number) => void;
  sort: SortKey;
  descending: boolean;
  onSort: (key: SortKey) => void;
  /** Whether the "last modified" column can be trusted on this platform. */
  dateLabel: string;
  /** Opens the row's context menu at a screen position. */
  onContextMenu: (x: number, y: number, row: FileRow) => void;
  /** Reports the row under the pointer, for keyboard actions. */
  onFocusRow: (row: FileRow) => void;
}

const ROW_HEIGHT = 46;

export function FileTable({
  total,
  getRow,
  onRangeChange,
  sort,
  descending,
  onSort,
  dateLabel,
  onContextMenu,
  onFocusRow,
}: FileTableProps) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const selected = useQueueStore((state) => state.selected);
  const toggle = useQueueStore((state) => state.toggle);
  const setRange = useQueueStore((state) => state.setRange);
  const requestThumbs = useThumbStore((state) => state.request);
  // Anchor for Shift-click, the way every file manager behaves.
  const anchor = useRef<number | null>(null);

  const pick = (index: number, id: number, shiftKey: boolean) => {
    if (shiftKey && anchor.current !== null) {
      const from = Math.min(anchor.current, index);
      const to = Math.max(anchor.current, index);
      const ids: number[] = [];
      for (let i = from; i <= to; i++) {
        const row = getRow(i);
        // Rows outside the loaded window cannot be resolved; the range covers
        // what is actually there rather than silently selecting unknowns.
        if (row && isSafeToTouch(row.flags)) ids.push(row.id);
      }
      setRange(ids, !selected.has(id));
      return;
    }
    anchor.current = index;
    toggle(id);
  };

  const virtualizer = useVirtualizer({
    count: total,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 12,
    // Fetching is driven from here rather than from an effect on the rendered
    // items, so a fast scroll requests the page it landed on instead of every
    // page it flew past.
    onChange: (instance) => {
      const items = instance.getVirtualItems();
      const first = items[0];
      const last = items[items.length - 1];
      if (first && last) {
        onRangeChange(first.index, last.index);
        // Previews are only decoded for rows actually on screen. Scrolling
        // past a thousand files must not decode a thousand images.
        const visible = items
          .map((item) => getRow(item.index))
          .filter((row): row is FileRow => row !== undefined);
        requestThumbs(visible);
      }
    },
  });

  const reveal = async (row: FileRow) => {
    try {
      // The path is re-resolved in Rust from the id rather than trusting the
      // display string the UI is holding.
      const path = await api.resolvePath(row.id);
      await revealItemInDir(path);
    } catch {
      // Revealing is a convenience; a missing file or a denied folder is not
      // worth interrupting the user over.
    }
  };

  return (
    <div className="table-wrap">
      <div className="thead">
        <span className="col-pick" />
        <SortHeader
          className="col-name"
          label="Name"
          field="name"
          sort={sort}
          descending={descending}
          onSort={onSort}
        />
        <SortHeader
          className="col-cat"
          label="Type"
          field="category"
          sort={sort}
          descending={descending}
          onSort={onSort}
        />
        <SortHeader
          className="col-date"
          label={dateLabel}
          field="modified"
          sort={sort}
          descending={descending}
          onSort={onSort}
        />
        <SortHeader
          className="col-size"
          label="Size"
          field="size"
          sort={sort}
          descending={descending}
          onSort={onSort}
          align="right"
        />
        <span className="col-actions" />
      </div>

      <div className="tbody" ref={scrollRef}>
        <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
          {virtualizer.getVirtualItems().map((item) => {
            const row = getRow(item.index);
            return (
              <div
                key={item.key}
                className={`row ${row && !isSafeToTouch(row.flags) ? "untouchable" : ""} ${
                  row && selected.has(row.id) ? "picked" : ""
                }`}
                style={{ height: ROW_HEIGHT, transform: `translateY(${item.start}px)` }}
                onMouseEnter={() => {
                  if (row) onFocusRow(row);
                }}
                onDoubleClick={() => {
                  if (row) void useFileOpsStore.getState().openFile(row);
                }}
                onContextMenu={(event) => {
                  if (!row) return;
                  event.preventDefault();
                  // Right-clicking outside the selection targets that row
                  // alone, as every file manager does.
                  if (!selected.has(row.id)) {
                    anchor.current = item.index;
                  }
                  onContextMenu(event.clientX, event.clientY, row);
                }}
              >
                {row ? (
                  <Row
                    row={row}
                    onReveal={reveal}
                    checked={selected.has(row.id)}
                    onToggle={(shiftKey) => pick(item.index, row.id, shiftKey)}
                  />
                ) : (
                  <PlaceholderRow />
                )}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

function Row({
  row,
  onReveal,
  checked,
  onToggle,
}: {
  row: FileRow;
  onReveal: (row: FileRow) => void;
  checked: boolean;
  onToggle: (shiftKey: boolean) => void;
}) {
  const reason = untouchableReason(row.flags);
  return (
    <>
      <div
        className="col-pick"
        onClick={(event) => {
          // Shift-click anywhere in the cell, not just exactly on the box.
          if (event.shiftKey && !reason) {
            event.preventDefault();
            onToggle(true);
          }
        }}
      >
        <Checkbox
          checked={checked}
          onChange={() => onToggle(false)}
          label={`Select ${row.name} for cleanup`}
          disabledReason={reason}
        />
      </div>
      <div className="col-name">
        <Thumb row={row} />
        <div className="file-meta">
          <span className="file-name" title={row.path}>
            {row.name}
          </span>
          <span className="file-sub">
            {reason ? <span className="tag tag-warn">{reason}</span> : row.parent}
          </span>
        </div>
      </div>
      <div className="col-cat">
        <span className="cat-label">{CATEGORY_LABELS[row.category]}</span>
      </div>
      <div className="col-date muted" title={formatAbsolute(row.modified)}>
        {formatRelative(row.modified)}
      </div>
      <div className="col-size">{formatBytes(row.size)}</div>
      <div className="col-actions">
        <button
          className="btn-ghost"
          title="Show in file manager"
          aria-label={`Show ${row.name} in file manager`}
          onClick={() => onReveal(row)}
        >
          <IconReveal size={14} />
        </button>
      </div>
    </>
  );
}

/** Shown while the page covering this index is still in flight. */
function PlaceholderRow() {
  return (
    <>
      <div className="col-pick" />
      <div className="col-name">
        <span className="row-skeleton" />
      </div>
    </>
  );
}

function SortHeader({
  className,
  label,
  field,
  sort,
  descending,
  onSort,
  align,
}: {
  className: string;
  label: string;
  field: SortKey;
  sort: SortKey;
  descending: boolean;
  onSort: (key: SortKey) => void;
  align?: "right";
}) {
  const active = sort === field;
  return (
    <button
      className={`th sortable ${className} ${active ? "active" : ""}`}
      style={align === "right" ? { justifyContent: "flex-end" } : undefined}
      onClick={() => onSort(field)}
      aria-label={`Sort by ${label}`}
    >
      {label}
      {active ? descending ? <IconArrowDown size={11} /> : <IconArrowUp size={11} /> : null}
    </button>
  );
}
