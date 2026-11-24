import { useCallback, useEffect, useMemo, useState } from "react";
import { Banner } from "@/components/Banner";
import { Checkbox } from "@/components/Checkbox";
import { IconAlert, IconRefresh, IconSparkle } from "@/components/icons";
import { api, toIpcError, type IpcError } from "@/ipc/client";
import type { QuickCleanMeasurement, QuickCleanOutcome } from "@/ipc/types";
import { formatBytes, formatCount, plural } from "@/lib/format";

type Phase = "scanning" | "ready" | "confirming" | "cleaning" | "done";

/**
 * Quick Clean: the junk caches Windows leaves behind.
 *
 * This is the only part of Cleanup that deletes permanently, because routing
 * a hundred thousand temp files through the Recycle Bin would free no space
 * at all — which is the whole point. So the trade is made explicitly: the
 * exact folders and sizes are on screen before anything happens, nothing with
 * a real downside is ticked by default, and the confirmation says the word
 * "permanently".
 *
 * It measures on mount so that by the time anyone looks, the button already
 * says how much there is to free.
 */
export function QuickClean() {
  const [items, setItems] = useState<QuickCleanMeasurement[]>([]);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [phase, setPhase] = useState<Phase>("scanning");
  const [results, setResults] = useState<QuickCleanOutcome[] | null>(null);
  const [error, setError] = useState<IpcError | null>(null);
  const [expanded, setExpanded] = useState(false);

  const scan = useCallback(async () => {
    setPhase("scanning");
    setResults(null);
    try {
      const measured = await api.quickCleanScan();
      setItems(measured);
      setPicked(
        new Set(
          measured
            // Only tick what is both safe and actually has something in it.
            .filter((item) => item.defaultOn && !item.missing && item.bytes > 0)
            .map((item) => item.key),
        ),
      );
      setPhase("ready");
      setError(null);
    } catch (err) {
      setError(toIpcError(err));
      setPhase("ready");
    }
  }, []);

  useEffect(() => {
    void scan();
  }, [scan]);

  const total = useMemo(
    () =>
      items
        .filter((item) => picked.has(item.key))
        .reduce((sum, item) => sum + item.bytes, 0),
    [items, picked],
  );

  const freed = useMemo(
    () => (results ?? []).reduce((sum, outcome) => sum + outcome.freedBytes, 0),
    [results],
  );

  const run = async () => {
    setPhase("cleaning");
    try {
      const outcomes = await api.quickCleanRun([...picked]);
      setResults(outcomes);
      setPhase("done");
      setError(null);
      // Re-measure so the numbers on screen are what is really left, not what
      // we assumed we removed.
      const measured = await api.quickCleanScan();
      setItems(measured);
    } catch (err) {
      setError(toIpcError(err));
      setPhase("ready");
    }
  };

  const toggle = (key: string) => {
    setPicked((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  // Anything not worth showing a row for: folders this machine does not have.
  const visible = items.filter((item) => !item.missing);
  const skippedForAdmin = (results ?? []).reduce(
    (sum, outcome) => sum + outcome.skippedDenied,
    0,
  );
  const skippedInUse = (results ?? []).reduce((sum, outcome) => sum + outcome.skippedInUse, 0);

  return (
    <div className="card quick-clean">
      <div className="card-head">
        <h2>Quick Clean</h2>
        <span className="card-sub">Caches Windows can rebuild</span>
      </div>

      {error ? (
        <Banner
          tone="danger"
          title="Quick Clean ran into a problem"
          text={error.message}
          hint={error.hint}
          onDismiss={() => setError(null)}
        />
      ) : null}

      <div className="qc-top">
        <span className="qc-icon">
          <IconSparkle size={18} />
        </span>

        <div className="qc-headline">
          {phase === "scanning" ? (
            <>
              <div className="qc-amount dim">Looking…</div>
              <div className="qc-sub muted">Measuring the temporary folders.</div>
            </>
          ) : phase === "done" ? (
            <>
              <div className="qc-amount">{formatBytes(freed)} freed</div>
              <div className="qc-sub muted">
                {skippedInUse > 0 || skippedForAdmin > 0
                  ? [
                      skippedInUse > 0 ? `${formatCount(skippedInUse)} in use` : null,
                      skippedForAdmin > 0
                        ? `${formatCount(skippedForAdmin)} needed administrator`
                        : null,
                    ]
                      .filter(Boolean)
                      .join(", ") + " — left alone."
                  : "Everything selected was removed."}
              </div>
            </>
          ) : total > 0 ? (
            <>
              <div className="qc-amount">{formatBytes(total)}</div>
              <div className="qc-sub muted">
                in {formatCount(picked.size)} {plural(picked.size, "place")} that can go.
              </div>
            </>
          ) : (
            <>
              <div className="qc-amount dim">Nothing to clear</div>
              <div className="qc-sub muted">
                These folders are already empty, or nothing is selected.
              </div>
            </>
          )}
        </div>

        {phase === "done" ? (
          <button className="btn" onClick={() => void scan()}>
            <IconRefresh size={13} /> Check again
          </button>
        ) : (
          <button
            className="btn-primary qc-go"
            disabled={phase !== "ready" || total === 0}
            onClick={() => setPhase("confirming")}
          >
            {phase === "cleaning" ? "Cleaning…" : `Free ${formatBytes(total)}`}
          </button>
        )}
      </div>

      {phase === "confirming" ? (
        <div className="qc-confirm">
          <div className="qc-confirm-title">
            <IconAlert size={14} /> This deletes permanently
          </div>
          <p className="setting-help">
            These are caches, not documents, so they do not go to the Recycle Bin — sending a
            hundred thousand temp files there would free no space at all. Windows rebuilds
            everything here as it needs it. Anything currently in use is left alone.
          </p>
          <p className="setting-help">
            About to clear <strong>{formatBytes(total)}</strong> from{" "}
            {formatCount(picked.size)} {plural(picked.size, "folder")}.
          </p>
          <div className="qc-confirm-actions">
            <button className="btn-primary" onClick={() => void run()}>
              Delete {formatBytes(total)} permanently
            </button>
            <button className="btn" onClick={() => setPhase("ready")}>
              Cancel
            </button>
          </div>
        </div>
      ) : null}

      <button
        className="btn-ghost qc-toggle"
        onClick={() => setExpanded((v) => !v)}
        aria-expanded={expanded}
      >
        {expanded ? "Hide" : "Show"} what will be cleared ({formatCount(visible.length)}{" "}
        {plural(visible.length, "place")})
      </button>

      {expanded ? (
        <div className="qc-list">
          {visible.map((item) => {
            const outcome = (results ?? []).find((o) => o.key === item.key);
            return (
              <div className="qc-item" key={item.key}>
                <Checkbox
                  checked={picked.has(item.key)}
                  onChange={() => toggle(item.key)}
                  label={`Include ${item.title}`}
                  disabledReason={phase === "cleaning" ? "Cleaning…" : undefined}
                />

                <div className="qc-item-body">
                  <div className="qc-item-head">
                    <span className="qc-item-title">{item.title}</span>
                    <span className="qc-item-size">
                      {item.unreadable ? (
                        // Not "0 bytes": we could not look inside.
                        <span className="muted">size unknown</span>
                      ) : (
                        formatBytes(item.bytes)
                      )}
                    </span>
                  </div>

                  <div className="qc-item-desc muted">{item.description}</div>

                  {item.path ? (
                    <code className="qc-item-path selectable" title={item.path}>
                      {item.path}
                    </code>
                  ) : null}

                  {item.caveat ? (
                    <div className="qc-item-caveat">
                      <IconAlert size={11} /> {item.caveat}
                    </div>
                  ) : null}

                  {item.needsAdmin ? (
                    <div className="qc-item-note muted">
                      Most of this needs Cleanup to be running as administrator; without that it
                      is skipped rather than failing.
                    </div>
                  ) : null}

                  {outcome ? (
                    <div className="qc-item-result">
                      {outcome.freedBytes > 0
                        ? `Freed ${formatBytes(outcome.freedBytes)} (${formatCount(
                            outcome.removedFiles,
                          )} ${plural(outcome.removedFiles, "file")})`
                        : "Nothing removed"}
                      {outcome.skippedDenied > 0
                        ? ` · ${formatCount(outcome.skippedDenied)} refused`
                        : ""}
                      {outcome.skippedInUse > 0
                        ? ` · ${formatCount(outcome.skippedInUse)} in use`
                        : ""}
                      {outcome.exampleError ? (
                        <div className="qc-item-note muted selectable">
                          {outcome.exampleError}
                        </div>
                      ) : null}
                    </div>
                  ) : null}
                </div>
              </div>
            );
          })}
        </div>
      ) : null}
    </div>
  );
}
