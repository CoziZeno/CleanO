import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { FileRow } from "@/ipc/types";
import { Banner } from "@/components/Banner";
import { FileTable } from "@/components/FileTable";
import { IconClose, IconSearch } from "@/components/icons";
import { api } from "@/ipc/client";
import {
  CATEGORIES,
  CATEGORY_LABELS,
  UNSAFE_TO_TOUCH,
  emptyQuery,
  type Category,
  type QueryRequest,
  type SortKey,
} from "@/ipc/types";
import { useQueueStore } from "@/state/queueStore";
import { AGE_PRESETS, SIZE_PRESETS, formatBytes, formatCount, plural } from "@/lib/format";
import { useScanStore } from "@/state/scanStore";
import { OrganizePanel } from "./OrganizePanel";
import { usePagedQuery } from "./usePagedQuery";

const DAY = 86_400;

interface DownloadsViewProps {
  /** A filter handed over from Overview. Applied when `presetNonce` changes. */
  preset: Partial<QueryRequest> | null;
  presetNonce: number;
  searchRef: React.RefObject<HTMLInputElement>;
  onContextMenu: (x: number, y: number, row: FileRow) => void;
  /** Last row the pointer or menu touched, for keyboard actions. */
  onFocusRow: (row: FileRow) => void;
}

export function DownloadsView({
  preset,
  presetNonce,
  searchRef,
  onContextMenu,
  onFocusRow,
}: DownloadsViewProps) {
  const { summary, generation, platform, status } = useScanStore();
  const add = useQueueStore((state) => state.add);
  const selectAllRef = useRef<() => void>(() => {});

  const [text, setText] = useState("");
  const [categories, setCategories] = useState<Category[]>([]);
  const [sizeIndex, setSizeIndex] = useState(0);
  const [ageIndex, setAgeIndex] = useState(0);
  const [sort, setSort] = useState<SortKey>("size");
  const [descending, setDescending] = useState(true);

  // Applying a preset from Overview. Keyed on the nonce rather than the object
  // so navigating to the same preset twice still re-applies it.
  const appliedNonce = useRef(0);
  useEffect(() => {
    if (presetNonce === appliedNonce.current || !preset) return;
    appliedNonce.current = presetNonce;
    setText("");
    setCategories(preset.categories ?? []);
    setSort(preset.sort ?? "size");
    setDescending(preset.descending ?? true);
    setSizeIndex(
      preset.minSize ? Math.max(SIZE_PRESETS.findIndex((p) => p.min === preset.minSize), 0) : 0,
    );
    if (preset.modifiedBefore) {
      const days = Math.round((Date.now() / 1000 - preset.modifiedBefore) / DAY);
      const match = AGE_PRESETS.findIndex((p) => p.days !== null && p.days < 0 && -p.days === days);
      setAgeIndex(match >= 0 ? match : 0);
    } else {
      setAgeIndex(0);
    }
  }, [preset, presetNonce]);

  const request = useMemo<QueryRequest>(() => {
    const size = SIZE_PRESETS[sizeIndex];
    const age = AGE_PRESETS[ageIndex];
    const now = Math.floor(Date.now() / 1000);
    let modifiedAfter: number | null = null;
    let modifiedBefore: number | null = null;
    if (age?.days != null) {
      if (age.days > 0) modifiedAfter = now - age.days * DAY;
      else modifiedBefore = now + age.days * DAY;
    }
    return emptyQuery({
      text: text.trim() === "" ? null : text,
      categories,
      minSize: size?.min ?? null,
      modifiedAfter,
      modifiedBefore,
      sort,
      descending,
    });
  }, [text, categories, sizeIndex, ageIndex, sort, descending]);

  const { total, bytes, loading, error, getRow, ensureRange } = usePagedQuery(request, generation);

  // Ctrl+A selects every match for the current filter, not just the loaded
  // page — the app-level handler cannot know the filter, so it asks here.
  selectAllRef.current = useCallback(async () => {
    if (total === 0) return;
    add(
      await api.queryIds({
        ...request,
        excludeFlags: request.excludeFlags | UNSAFE_TO_TOUCH,
      }),
    );
  }, [request, total, add]);

  useEffect(() => {
    const handler = () => void selectAllRef.current();
    window.addEventListener("cleanup:select-all", handler);
    return () => window.removeEventListener("cleanup:select-all", handler);
  }, []);

  const onSort = useCallback(
    (key: SortKey) => {
      if (key === sort) {
        setDescending((value) => !value);
      } else {
        setSort(key);
        // Sensible first direction per column: biggest and newest first,
        // but names A-Z.
        setDescending(key !== "name" && key !== "extension");
      }
    },
    [sort],
  );

  const toggleCategory = (category: Category) => {
    setCategories((current) =>
      current.includes(category)
        ? current.filter((c) => c !== category)
        : [...current, category],
    );
  };

  const hasFilters =
    text.trim() !== "" || categories.length > 0 || sizeIndex !== 0 || ageIndex !== 0;

  const clearFilters = () => {
    setText("");
    setCategories([]);
    setSizeIndex(0);
    setAgeIndex(0);
  };

  if (!summary && status !== "scanning") {
    return (
      <div className="empty">
        <div className="empty-title">Nothing scanned yet</div>
        <p className="empty-body">
          Run a scan from the Overview tab and every file will show up here, searchable and
          sortable.
        </p>
      </div>
    );
  }

  const counts = new Map(summary?.byCategory.map((stat) => [stat.category, stat.count]) ?? []);

  return (
    <>
      <OrganizePanel />

      <div className="filterbar">
        <label className="search">
          <IconSearch size={14} />
          <input
            ref={searchRef}
            value={text}
            onChange={(event) => setText(event.target.value)}
            placeholder="Search file names…"
            spellCheck={false}
            autoComplete="off"
          />
          {text ? (
            <button className="btn-ghost" onClick={() => setText("")} aria-label="Clear search">
              <IconClose size={13} />
            </button>
          ) : null}
        </label>

        <select
          className="select"
          value={sizeIndex}
          onChange={(event) => setSizeIndex(Number(event.target.value))}
          aria-label="Filter by size"
        >
          {SIZE_PRESETS.map((option, index) => (
            <option key={option.label} value={index}>
              {option.label}
            </option>
          ))}
        </select>

        <select
          className="select"
          value={ageIndex}
          onChange={(event) => setAgeIndex(Number(event.target.value))}
          aria-label="Filter by age"
        >
          {AGE_PRESETS.map((option, index) => (
            <option key={option.label} value={index}>
              {option.label}
            </option>
          ))}
        </select>

        {hasFilters ? (
          <button className="btn btn-ghost" onClick={clearFilters}>
            Clear
          </button>
        ) : null}
      </div>

      <div className="chips">
        {CATEGORIES.filter((category) => (counts.get(category) ?? 0) > 0).map((category) => (
          <button
            key={category}
            className={`chip ${categories.includes(category) ? "active" : ""}`}
            onClick={() => toggleCategory(category)}
            aria-pressed={categories.includes(category)}
          >
            {CATEGORY_LABELS[category]}
            <span className="chip-count">{formatCount(counts.get(category) ?? 0)}</span>
          </button>
        ))}
      </div>

      <div className="resultbar">
        <span style={{ display: "flex", alignItems: "center", gap: 10 }}>
          {total > 0 ? (
            <button
              className="btn btn-ghost"
              style={{ height: 24 }}
              title="Select every file matching the current filters"
              onClick={async () => {
                // Untouchable files are excluded server-side, so "select all"
                // never picks something that would then be refused.
                add(
                  await api.queryIds({
                    ...request,
                    excludeFlags: request.excludeFlags | UNSAFE_TO_TOUCH,
                  }),
                );
              }}
            >
              Select all {formatCount(total)}
            </button>
          ) : null}
          {loading && total === 0 ? (
            <span style={{ display: "inline-flex", alignItems: "center", gap: 7 }}>
              <span className="spinner" /> Searching…
            </span>
          ) : (
            <>
              {formatCount(total)} {plural(total, "file")} · {formatBytes(bytes)}
            </>
          )}
        </span>
        {summary?.flagged.hidden ? (
          <span className="dim">
            {formatCount(summary.flagged.hidden)} hidden items excluded — change this in Settings
          </span>
        ) : null}
      </div>

      {error ? (
        <div style={{ padding: "0 20px 12px" }}>
          <Banner tone="danger" title="Could not read the file list" text={error.message} hint={error.hint} />
        </div>
      ) : null}

      {total === 0 && !loading ? (
        <div className="empty">
          <div className="empty-title">No files match</div>
          <p className="empty-body">
            {hasFilters
              ? "Try clearing a filter or searching for something shorter."
              : "This folder came back empty."}
          </p>
        </div>
      ) : (
        <FileTable
          total={total}
          getRow={getRow}
          onRangeChange={ensureRange}
          sort={sort}
          descending={descending}
          onSort={onSort}
          dateLabel={platform?.accessTimeReliable ? "Modified" : "Changed"}
          onContextMenu={onContextMenu}
          onFocusRow={onFocusRow}
        />
      )}
    </>
  );
}
