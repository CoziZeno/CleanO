import { useCallback, useEffect, useRef, useState } from "react";
import { Banner } from "@/components/Banner";
import { IconQueue, IconReveal, IconSparkle } from "@/components/icons";
import { api, toIpcError, type IpcError } from "@/ipc/client";
import {
  CATEGORY_LABELS,
  type FileRow,
  type Settings,
  type SwipeDecision,
  type SwipeProgress,
} from "@/ipc/types";
import { CATEGORY_COLORS, isPreviewable } from "@/lib/categories";
import { formatAbsolute, formatBytes, formatCount, formatRelative } from "@/lib/format";
import type { Route } from "@/app/nav";
import { useFileOpsStore } from "@/state/fileOpsStore";
import { useScanStore } from "@/state/scanStore";
import { useSettingsStore } from "@/state/settingsStore";
import { useThumbStore } from "@/state/thumbStore";

/** Horizontal distance before a drag counts as a swipe. */
const SWIPE_THRESHOLD = 110;

/**
 * Plain-language summary of what Swipe Clean is currently showing.
 *
 * Worth the few lines: without it an empty queue is indistinguishable from a
 * clean folder, and a narrow extension filter set weeks ago is invisible.
 */
function describeFilter(settings: Settings | null): string {
  if (!settings) return "your chosen formats";
  const { categories, extensions, minBytes } = settings.swipe;
  if (categories.length === 0) return "nothing (no kinds are selected)";

  const kinds = categories.map((category) => CATEGORY_LABELS[category].toLowerCase());
  const kindText =
    kinds.length === 1
      ? kinds[0]
      : `${kinds.slice(0, -1).join(", ")} and ${kinds[kinds.length - 1]}`;

  const parts = [kindText];
  if (extensions.length > 0) parts.push(`limited to ${extensions.join(", ")}`);
  if (minBytes > 0) parts.push(`over ${formatBytes(minBytes)}`);
  return parts.join(", ");
}

type Outgoing = "kept" | "trashed" | null;

export function SwipeView({ onJump }: { onJump: (route: Route) => void }) {
  const { summary, generation, startScan } = useScanStore();
  const ops = useFileOpsStore();
  // The filter that decides what reaches this screen at all, so an empty
  // queue can say *why* it is empty instead of claiming everything is done.
  const settings = useSettingsStore((state) => state.settings);
  const loadSettings = useSettingsStore((state) => state.load);
  useEffect(() => {
    if (!settings) void loadSettings();
  }, [settings, loadSettings]);

  const [queue, setQueue] = useState<FileRow[]>([]);
  const [progress, setProgress] = useState<SwipeProgress | null>(null);
  const [cursor, setCursor] = useState(0);
  const [laterMode, setLaterMode] = useState(false);
  const [error, setError] = useState<IpcError | null>(null);
  const [busy, setBusy] = useState(false);
  const [outgoing, setOutgoing] = useState<Outgoing>(null);
  const [drag, setDrag] = useState(0);
  const [lastAction, setLastAction] = useState<{ id: number; decision: SwipeDecision } | null>(
    null,
  );

  const dragStart = useRef<number | null>(null);
  const current = queue[cursor];

  const load = useCallback(
    async (later: boolean) => {
      try {
        const result = await api.swipeQueue(later);
        setQueue(result.files);
        setProgress(result.progress);
        setCursor(0);
        setError(null);
      } catch (err) {
        const ipc = toIpcError(err);
        setError(ipc.code === "noScan" ? null : ipc);
        setQueue([]);
        setProgress(null);
      }
    },
    [],
  );

  useEffect(() => {
    void load(laterMode);
  }, [load, laterMode, generation]);

  // Fetch previews a few ahead, so the next card is ready before you reach it.
  useEffect(() => {
    const ahead = queue.slice(cursor, cursor + 4).filter(isPreviewable);
    if (ahead.length > 0) useThumbStore.getState().request(ahead);
  }, [queue, cursor]);

  const advance = useCallback(() => {
    setOutgoing(null);
    setDrag(0);
    setCursor((index) => index + 1);
  }, []);

  const decide = useCallback(
    async (decision: SwipeDecision) => {
      if (!current || busy) return;
      setBusy(true);
      try {
        if (decision === "trashed") {
          // Trash through the normal journalled path, and only record the
          // decision once it actually succeeded.
          const operation = await api.trashFiles([current.id]);
          if (operation.done === 0) {
            setError(
              toIpcError(
                operation.items[0]?.error ?? "that file could not be moved to the Recycle Bin",
              ),
            );
            setBusy(false);
            return;
          }
        }
        await api.swipeDecide([current.id], decision);
        setLastAction({ id: current.id, decision });
        setProgress((p) =>
          p
            ? {
                ...p,
                reviewed: decision === "later" ? p.reviewed : p.reviewed + 1,
                kept: p.kept + (decision === "kept" ? 1 : 0),
                trashed: p.trashed + (decision === "trashed" ? 1 : 0),
                moved: p.moved + (decision === "moved" ? 1 : 0),
                later: p.later + (decision === "later" ? 1 : 0),
                remaining: Math.max(0, p.remaining - 1),
              }
            : p,
        );
        advance();
      } catch (err) {
        setError(toIpcError(err));
      } finally {
        setBusy(false);
      }
    },
    [current, busy, advance],
  );

  const move = useCallback(async () => {
    if (!current || busy) return;
    setBusy(true);
    try {
      await ops.moveTo([current.id]);
      // moveTo triggers a rescan; only record if the file actually left.
      await api.swipeDecide([current.id], "moved");
      setLastAction({ id: current.id, decision: "moved" });
      advance();
    } catch (err) {
      setError(toIpcError(err));
    } finally {
      setBusy(false);
    }
  }, [current, busy, ops, advance]);

  const undo = useCallback(async () => {
    if (!lastAction) return;
    try {
      await api.swipeForget([lastAction.id]);
      setLastAction(null);
      // Step back to the card, so undo is visible rather than silent.
      setCursor((index) => Math.max(0, index - 1));
      if (lastAction.decision === "trashed" || lastAction.decision === "moved") {
        // The file left the folder; only a rescan can tell us where it is now.
        await startScan();
      }
    } catch (err) {
      setError(toIpcError(err));
    }
  }, [lastAction, startScan]);

  // Keyboard. Deliberately not active while a dialog or text field has focus.
  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (
        target &&
        (target.tagName === "INPUT" ||
          target.tagName === "TEXTAREA" ||
          target.isContentEditable)
      ) {
        return;
      }
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "z") {
        event.preventDefault();
        void undo();
        return;
      }
      if (event.ctrlKey || event.metaKey || event.altKey) return;

      switch (event.key.toLowerCase()) {
        case "a":
          event.preventDefault();
          void decide("kept");
          break;
        case "d":
          event.preventDefault();
          void decide("trashed");
          break;
        case "m":
          event.preventDefault();
          void move();
          break;
        case "s":
          event.preventDefault();
          void decide("later");
          break;
        case "arrowright":
          event.preventDefault();
          setCursor((index) => Math.min(queue.length, index + 1));
          break;
        case "arrowleft":
          event.preventDefault();
          setCursor((index) => Math.max(0, index - 1));
          break;
        case "enter":
          event.preventDefault();
          if (current) void ops.openFile(current);
          break;
        default:
          break;
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [decide, move, undo, queue.length, current, ops]);

  if (!summary) {
    return (
      <div className="empty">
        <div className="empty-title">Nothing scanned yet</div>
        <p className="empty-body">
          Scan a folder and its images, videos, archives and large files will queue up here, one
          at a time.
        </p>
      </div>
    );
  }

  if (error) {
    return (
      <div className="view">
        <Banner tone="danger" title="Something went wrong" text={error.message} hint={error.hint} onDismiss={() => setError(null)} />
      </div>
    );
  }

  return (
    <div className="view swipe">
      <SwipeHeader
        progress={progress}
        laterMode={laterMode}
        onToggleLater={() => setLaterMode((v) => !v)}
        canUndo={lastAction !== null}
        onUndo={() => void undo()}
      />

      <div className="swipe-filter">
        <span className="muted">Showing {describeFilter(settings)}</span>
        <button className="btn-ghost swipe-filter-edit" onClick={() => onJump("settings")}>
          Change
        </button>
      </div>

      {!current ? (
        <div className="empty" style={{ padding: "56px 24px" }}>
          <span className="reward-icon">
            <IconSparkle size={19} />
          </span>
          <div className="empty-title">
            {laterMode
              ? "Nothing waiting for a second look"
              : progress && progress.total === 0
                ? "Nothing matches your format choices"
                : "All reviewed"}
          </div>
          <p className="empty-body">
            {laterMode
              ? "Files you skip with Review Later collect here."
              : progress && progress.total === 0
                ? // Not "all reviewed": the filter let nothing through, which
                  // is a different thing and needs a different answer.
                  settings && settings.swipe.categories.length === 0
                  ? "Swipe Clean has no kinds of file selected, so there is nothing for it to show."
                  : `This folder has nothing matching ${describeFilter(settings)}.`
                : progress && progress.later > 0
                  ? `${formatCount(progress.later)} skipped files are waiting in Review Later.`
                  : "Every file in this folder has been reviewed. Scan again to look for new ones."}
          </p>
          {progress && progress.later > 0 && !laterMode ? (
            <button className="btn" onClick={() => setLaterMode(true)}>
              Review those now
            </button>
          ) : null}
          {progress && progress.total === 0 && !laterMode ? (
            <button className="btn" onClick={() => onJump("settings")}>
              Choose formats
            </button>
          ) : null}
        </div>
      ) : (
        <div className="swipe-stage">
          <SwipeCard
            row={current}
            drag={drag}
            outgoing={outgoing}
            onPointerDown={(x) => {
              dragStart.current = x;
            }}
            onPointerMove={(x) => {
              if (dragStart.current === null) return;
              setDrag(x - dragStart.current);
            }}
            onPointerUp={() => {
              const distance = drag;
              dragStart.current = null;
              if (distance <= -SWIPE_THRESHOLD) {
                setOutgoing("trashed");
                void decide("trashed");
              } else if (distance >= SWIPE_THRESHOLD) {
                setOutgoing("kept");
                void decide("kept");
              } else {
                setDrag(0);
              }
            }}
            onOpen={() => void ops.openFile(current)}
            onReveal={() => void ops.revealFile(current)}
          />

          <div className="swipe-actions">
            <button
              className="btn swipe-btn danger"
              disabled={busy}
              onClick={() => void decide("trashed")}
            >
              Trash <span className="kbd">D</span>
            </button>
            <button className="btn swipe-btn" disabled={busy} onClick={() => void move()}>
              Move <span className="kbd">M</span>
            </button>
            <button
              className="btn swipe-btn"
              disabled={busy}
              onClick={() => void decide("later")}
            >
              Later <span className="kbd">S</span>
            </button>
            <button
              className="btn swipe-btn keep"
              disabled={busy}
              onClick={() => void decide("kept")}
            >
              Keep <span className="kbd">A</span>
            </button>
          </div>

          <p className="swipe-hint">
            Drag the card left to trash, right to keep. Keeping moves nothing — it just stops
            the file being asked about.
          </p>
        </div>
      )}
    </div>
  );
}

function SwipeHeader({
  progress,
  laterMode,
  onToggleLater,
  canUndo,
  onUndo,
}: {
  progress: SwipeProgress | null;
  laterMode: boolean;
  onToggleLater: () => void;
  canUndo: boolean;
  onUndo: () => void;
}) {
  if (!progress) return null;
  const done = progress.total - progress.remaining;
  const percent = progress.total > 0 ? (done / progress.total) * 100 : 0;

  return (
    <div className="swipe-head">
      <div className="swipe-counts">
        <span>
          <strong>{formatCount(progress.remaining)}</strong> left
        </span>
        <span className="dim">·</span>
        <span className="muted">{formatCount(progress.kept)} kept</span>
        <span className="muted">{formatCount(progress.trashed)} trashed</span>
        <span className="muted">{formatCount(progress.moved)} moved</span>
        <span className="muted">{formatCount(progress.later)} later</span>
      </div>
      <div className="swipe-progress">
        <div className="swipe-progress-fill" style={{ width: `${percent}%` }} />
      </div>
      <div className="swipe-head-actions">
        <button className="btn btn-ghost" onClick={onToggleLater}>
          {laterMode ? "Back to queue" : `Review Later (${formatCount(progress.later)})`}
        </button>
        <button className="btn btn-ghost" onClick={onUndo} disabled={!canUndo}>
          Undo
        </button>
      </div>
    </div>
  );
}

function SwipeCard({
  row,
  drag,
  outgoing,
  onPointerDown,
  onPointerMove,
  onPointerUp,
  onOpen,
  onReveal,
}: {
  row: FileRow;
  drag: number;
  outgoing: Outgoing;
  onPointerDown: (x: number) => void;
  onPointerMove: (x: number) => void;
  onPointerUp: () => void;
  onOpen: () => void;
  onReveal: () => void;
}) {
  const entry = useThumbStore((state) => state.entries.get(row.id));
  const canPreview = isPreviewable(row);

  const rotation = drag / 28;
  const intent = drag <= -SWIPE_THRESHOLD ? "trash" : drag >= SWIPE_THRESHOLD ? "keep" : null;

  return (
    <div
      className={`swipe-card ${outgoing ? `out-${outgoing}` : ""}`}
      style={
        outgoing
          ? undefined
          : { transform: `translateX(${drag}px) rotate(${rotation}deg)` }
      }
      onPointerDown={(event) => {
        (event.target as HTMLElement).setPointerCapture?.(event.pointerId);
        onPointerDown(event.clientX);
      }}
      onPointerMove={(event) => onPointerMove(event.clientX)}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
    >
      {intent ? (
        <span className={`swipe-stamp ${intent}`}>{intent === "keep" ? "KEEP" : "TRASH"}</span>
      ) : null}

      <div className="swipe-preview">
        {entry?.data ? (
          <img src={entry.data} alt="" draggable={false} />
        ) : (
          <div className="swipe-preview-fallback">
            <span
              className="swipe-ext"
              style={{ background: CATEGORY_COLORS[row.category] }}
            >
              {row.ext ? row.ext.toUpperCase() : "FILE"}
            </span>
            <span className="dim" style={{ fontSize: 11.5 }}>
              {canPreview ? "Loading preview…" : "No preview for this format"}
            </span>
          </div>
        )}
      </div>

      <div className="swipe-meta">
        <div className="swipe-name selectable" title={row.path}>
          {row.name}
        </div>
        <div className="swipe-facts">
          <span>{formatBytes(row.size)}</span>
          <span className="dim">·</span>
          <span>{CATEGORY_LABELS[row.category]}</span>
          <span className="dim">·</span>
          <span title={formatAbsolute(row.modified)}>{formatRelative(row.modified)}</span>
        </div>
        <div className="swipe-location selectable" title={row.path}>
          {row.parent}
        </div>
        <div className="swipe-card-actions">
          <button className="btn btn-ghost" onClick={onOpen}>
            <IconQueue size={13} /> Open
          </button>
          <button className="btn btn-ghost" onClick={onReveal}>
            <IconReveal size={13} /> Show in folder
          </button>
        </div>
      </div>
    </div>
  );
}
