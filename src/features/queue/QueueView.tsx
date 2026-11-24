import { useCallback, useEffect, useState } from "react";
import { Banner } from "@/components/Banner";
import { IconClose, IconQueue, IconSparkle } from "@/components/icons";
import { CleanupCelebration } from "@/components/Reward";
import { Thumb } from "@/components/Thumb";
import { useThumbStore } from "@/state/thumbStore";
import { api } from "@/ipc/client";
import { CATEGORY_LABELS, type FileRow, type OperationRow } from "@/ipc/types";
import { formatBytes, formatCount, formatRelative, plural } from "@/lib/format";
import { useQueueStore } from "@/state/queueStore";
import { useScanStore } from "@/state/scanStore";

export function QueueView() {
  const { generation } = useScanStore();
  const {
    selected,
    operations,
    busy,
    error,
    lastOperation,
    lastUndo,
    remove,
    clear,
    loadOperations,
    trashSelected,
    undo,
    dismissError,
    dismissLast,
  } = useQueueStore();

  const [rows, setRows] = useState<FileRow[]>([]);
  const [confirming, setConfirming] = useState(false);

  // Resolve the selected ids to real rows, so the paths shown are the ones
  // Rust will act on rather than anything the UI has been holding.
  const loadRows = useCallback(async () => {
    const ids = Array.from(selected);
    if (ids.length === 0) {
      setRows([]);
      return;
    }
    try {
      const resolved = await api.getRows(ids);
      setRows(resolved);
      // This is the screen where "which file is this?" matters most.
      useThumbStore.getState().request(resolved);
    } catch {
      setRows([]);
    }
  }, [selected]);

  useEffect(() => {
    void loadRows();
  }, [loadRows, generation]);

  useEffect(() => {
    void loadOperations();
  }, [loadOperations]);

  const totalBytes = rows.reduce((sum, row) => sum + row.size, 0);

  return (
    <div className="view">
      {error ? (
        <Banner
          tone="danger"
          title="That could not be done"
          text={error.message}
          hint={error.hint}
          onDismiss={dismissError}
        />
      ) : null}

      {lastOperation ? (
        <CleanupCelebration operation={lastOperation} onDismiss={dismissLast} />
      ) : null}

      {lastUndo ? (
        <Banner
          tone={lastUndo.manualRestoreRequired ? "warn" : "info"}
          title={
            lastUndo.manualRestoreRequired
              ? "These files are in the Recycle Bin"
              : `${formatCount(lastUndo.restored)} ${plural(lastUndo.restored, "file")} put back`
          }
          text={
            lastUndo.manualRestoreRequired
              ? "Cleanup does not restore from the Recycle Bin, because that cannot be done reliably and the bin may have been emptied. Open it and restore these yourself: " +
                lastUndo.problems.slice(0, 3).join(", ") +
                (lastUndo.problems.length > 3
                  ? ` and ${lastUndo.problems.length - 3} more`
                  : "")
              : lastUndo.failed > 0
                ? lastUndo.problems[0]
                : undefined
          }
          onDismiss={dismissLast}
        />
      ) : null}

      <div className="card">
        <div className="card-head">
          <h2>Selected for cleanup</h2>
          <span className="card-sub">
            {rows.length > 0
              ? `${formatCount(rows.length)} ${plural(rows.length, "file")} · ${formatBytes(totalBytes)}`
              : "nothing selected"}
          </span>
        </div>

        {rows.length === 0 ? (
          <div className="empty" style={{ padding: "32px 12px" }}>
            <span className="signal-icon" style={{ width: 32, height: 32, borderRadius: 9 }}>
              <IconQueue size={16} />
            </span>
            <div className="empty-title">Nothing queued</div>
            <p className="empty-body">
              Tick files on the Downloads or Duplicates screens and they will collect here. You
              see every path before anything happens, and files go to the Recycle Bin — never a
              permanent delete.
            </p>
          </div>
        ) : (
          <>
            <div className="plain-list">
              {rows.map((row) => (
                <div className="plain-row" key={row.id}>
                  <Thumb row={row} />
                  <div className="file-meta">
                    <span className="file-name selectable" title={row.path}>
                      {row.name}
                    </span>
                    {/* The full path, because this is the screen where it matters. */}
                    <span className="file-sub selectable">{row.path}</span>
                  </div>
                  <span className="dim" style={{ fontSize: 11 }}>
                    {CATEGORY_LABELS[row.category]}
                  </span>
                  <span className="legend-value" style={{ minWidth: 72, textAlign: "right" }}>
                    {formatBytes(row.size)}
                  </span>
                  <button
                    className="btn-ghost"
                    title="Remove from the queue"
                    aria-label={`Remove ${row.name} from the queue`}
                    onClick={() => remove([row.id])}
                  >
                    <IconClose size={13} />
                  </button>
                </div>
              ))}
            </div>

            <div className="queue-actions">
              <button className="btn btn-ghost" onClick={clear} disabled={busy}>
                Clear selection
              </button>
              {confirming ? (
                <>
                  <span className="muted" style={{ fontSize: 12.5 }}>
                    Move {formatCount(rows.length)} {plural(rows.length, "file")} to the Recycle
                    Bin?
                  </span>
                  <button className="btn" onClick={() => setConfirming(false)} disabled={busy}>
                    Cancel
                  </button>
                  <button
                    className="btn btn-primary"
                    disabled={busy}
                    onClick={() => {
                      setConfirming(false);
                      void trashSelected();
                    }}
                  >
                    {busy ? "Working…" : "Yes, move to Recycle Bin"}
                  </button>
                </>
              ) : (
                <button
                  className="btn btn-primary"
                  onClick={() => setConfirming(true)}
                  disabled={busy || rows.length === 0}
                >
                  Move to Recycle Bin — {formatBytes(totalBytes)}
                </button>
              )}
            </div>
          </>
        )}
      </div>

      <div className="card">
        <div className="card-head">
          <h2>History</h2>
          <span className="card-sub">every operation, with what it did</span>
        </div>
        {operations.length === 0 ? (
          <p className="setting-help">Nothing has been moved or deleted yet.</p>
        ) : (
          <div className="plain-list">
            {operations.map((operation) => (
              <OperationLine
                key={operation.id}
                operation={operation}
                busy={busy}
                onUndo={() => void undo(operation.id)}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

function OperationLine({
  operation,
  busy,
  onUndo,
}: {
  operation: OperationRow;
  busy: boolean;
  onUndo: () => void;
}) {
  const [open, setOpen] = useState(false);
  const tone =
    operation.state === "failed"
      ? "var(--danger)"
      : operation.state === "partial"
        ? "var(--warn)"
        : operation.state === "undone"
          ? "var(--text-dim)"
          : "var(--ok)";

  return (
    <div className="op-row">
      <div className="op-head">
        <span className="cat-dot" style={{ background: tone }} />
        <div className="file-meta">
          <span className="file-name">{operation.note}</span>
          <span className="file-sub">
            {formatRelative(operation.createdAt)} · {operation.state}
            {operation.failed > 0 ? ` · ${formatCount(operation.failed)} failed` : ""}
            {operation.bytes > 0 ? ` · ${formatBytes(operation.bytes)}` : ""}
          </span>
        </div>
        <button className="btn-ghost" onClick={() => setOpen((v) => !v)}>
          <span className="dim" style={{ fontSize: 11.5 }}>
            {open ? "Hide" : "Details"}
          </span>
        </button>
        {operation.undoable ? (
          <button className="btn" onClick={onUndo} disabled={busy}>
            Undo
          </button>
        ) : (
          <span className="tag">{operation.state === "undone" ? "undone" : "no undo"}</span>
        )}
      </div>

      {open ? (
        <div className="op-items">
          {operation.items.map((item, index) => (
            <div className="op-item" key={`${item.fromPath}-${index}`}>
              <span className={`op-item-state ${item.state}`}>{item.state}</span>
              <span className="file-name selectable" title={item.fromPath}>
                {item.name}
              </span>
              {item.toPath ? (
                <>
                  <span className="plan-arrow">→</span>
                  <span className="plan-target selectable" title={item.toPath}>
                    {item.toPath}
                  </span>
                </>
              ) : null}
              {item.error ? <span className="tag tag-warn">{item.error}</span> : null}
            </div>
          ))}
        </div>
      ) : null}
    </div>
  );
}

/** Shown on the Overview when a previous run died mid-operation. */
export function RecoveryBanner() {
  const { recoveryNotice, loadRecoveryNotice, dismissRecoveryNotice } = useQueueStore();
  useEffect(() => {
    void loadRecoveryNotice();
  }, [loadRecoveryNotice]);

  if (!recoveryNotice) return null;
  return (
    <Banner
      tone="warn"
      title="Cleanup did not shut down cleanly last time"
      text={recoveryNotice}
      onDismiss={dismissRecoveryNotice}
    />
  );
}

export function QueueBadge() {
  const count = useQueueStore((state) => state.selected.size);
  if (count === 0) return null;
  return (
    <span className="queue-badge">
      <IconSparkle size={10} />
      {count}
    </span>
  );
}
