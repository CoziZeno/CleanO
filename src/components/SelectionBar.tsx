import { useCallback, useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "@/ipc/client";
import { formatBytes, formatCount, plural } from "@/lib/format";
import { useQueueStore } from "@/state/queueStore";
import { IconClose, IconQueue } from "./icons";

/**
 * A bar that appears whenever anything is selected, on every screen.
 *
 * Without it, picking files on Downloads or Duplicates means navigating to
 * the queue to do anything with them. This keeps the action where the
 * selection was made, while still showing the count and total first - and the
 * confirmation step is unchanged, because this is the destructive one.
 */
export function SelectionBar({ onReview }: { onReview: () => void }) {
  const { selected, busy, clear, trashSelected, moveTo } = useQueueStore();
  const [bytes, setBytes] = useState(0);
  const [confirming, setConfirming] = useState(false);

  const count = selected.size;

  const measure = useCallback(async () => {
    const ids = Array.from(selected);
    if (ids.length === 0) {
      setBytes(0);
      return;
    }
    try {
      const rows = await api.getRows(ids);
      setBytes(rows.reduce((sum, row) => sum + row.size, 0));
    } catch {
      setBytes(0);
    }
  }, [selected]);

  useEffect(() => {
    void measure();
  }, [measure]);

  // A changed selection invalidates a pending confirmation - otherwise you
  // could confirm one set and delete another.
  useEffect(() => {
    setConfirming(false);
  }, [count]);

  if (count === 0) return null;

  return (
    <div className="selbar" role="region" aria-label="Selected files">
      <span className="selbar-count">
        <IconQueue size={13} />
        {formatCount(count)} {plural(count, "file")}
        {bytes > 0 ? <span className="dim"> · {formatBytes(bytes)}</span> : null}
      </span>

      <span className="plan-spacer" />

      {confirming ? (
        <>
          <span className="muted" style={{ fontSize: 12.5 }}>
            Move {formatCount(count)} to the Recycle Bin?
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
        <>
          <button className="btn btn-ghost" onClick={onReview}>
            Review
          </button>
          {/* Not everything is rubbish. Sometimes the answer is "put this
              somewhere", and that deserves equal billing with deleting. */}
          <button
            className="btn"
            disabled={busy}
            onClick={async () => {
              const picked = await open({
                directory: true,
                multiple: false,
                title: `Move ${count} ${plural(count, "file")} to…`,
              });
              if (typeof picked === "string") await moveTo(picked);
            }}
          >
            Move to folder…
          </button>
          <button className="btn btn-primary" onClick={() => setConfirming(true)} disabled={busy}>
            Move to Recycle Bin
          </button>
          <button className="btn-ghost" onClick={clear} aria-label="Clear selection" title="Clear selection">
            <IconClose size={14} />
          </button>
        </>
      )}
    </div>
  );
}
