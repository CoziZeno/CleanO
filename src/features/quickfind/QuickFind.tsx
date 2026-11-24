import { useCallback, useEffect, useRef, useState } from "react";
import { IconSearch, IconSparkle } from "@/components/icons";
import { Thumb } from "@/components/Thumb";
import { api, toIpcError } from "@/ipc/client";
import type { FileRow, QuickFindInterpretation } from "@/ipc/types";
import { formatBytes, formatCount, formatRelative, plural } from "@/lib/format";
import { useFileOpsStore } from "@/state/fileOpsStore";
import { useQueueStore } from "@/state/queueStore";
import { useThumbStore } from "@/state/thumbStore";

const EXAMPLES = [
  "largest mp4 files",
  "screenshots from last week",
  "pdfs older than a year",
  "zip archives over 500mb",
];

/**
 * Quick Find — describe what you are looking for.
 *
 * The request is parsed into ordinary filters in Rust and run against the
 * in-memory index, so results arrive as you type. No model is involved: most
 * requests are about type, size, date or name, and a filter answers those
 * instantly and correctly. When a request genuinely needs to understand
 * pixels or document text, the panel says so rather than quietly returning
 * something that only looks like an answer.
 */
export function QuickFind({ onClose }: { onClose: () => void }) {
  const [text, setText] = useState("");
  const [rows, setRows] = useState<FileRow[]>([]);
  const [total, setTotal] = useState(0);
  const [bytes, setBytes] = useState(0);
  const [interpretation, setInterpretation] = useState<QuickFindInterpretation | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [active, setActive] = useState(0);
  const input = useRef<HTMLInputElement>(null);

  const ops = useFileOpsStore();
  const addToQueue = useQueueStore((state) => state.add);

  useEffect(() => {
    input.current?.focus();
  }, []);

  const run = useCallback(async (value: string) => {
    if (value.trim().length === 0) {
      setRows([]);
      setTotal(0);
      setInterpretation(null);
      setError(null);
      return;
    }
    try {
      const response = await api.quickFind(value);
      setRows(response.rows);
      setTotal(response.total);
      setBytes(response.bytes);
      setInterpretation(response.interpretation);
      setActive(0);
      setError(null);
      useThumbStore.getState().request(response.rows.slice(0, 20));
    } catch (err) {
      const ipc = toIpcError(err);
      setError(ipc.code === "noScan" ? "Scan a folder first." : ipc.message);
      setRows([]);
      setTotal(0);
    }
  }, []);

  // Debounced so every keystroke does not re-query, though the query itself
  // is only a few milliseconds.
  useEffect(() => {
    const timer = setTimeout(() => void run(text), 120);
    return () => clearTimeout(timer);
  }, [text, run]);

  const chosen = rows[active];

  return (
    <div className="overlay quickfind-overlay" role="dialog" aria-modal="true" aria-label="Quick Find">
      <button className="overlay-backdrop" aria-label="Close" onClick={onClose} />
      <div className="quickfind">
        <div className="quickfind-input">
          <IconSearch size={16} />
          <input
            ref={input}
            value={text}
            placeholder="Describe what you're looking for…"
            spellCheck={false}
            onChange={(event) => setText(event.target.value)}
            onKeyDown={(event) => {
              event.stopPropagation();
              if (event.key === "Escape") onClose();
              if (event.key === "ArrowDown") {
                event.preventDefault();
                setActive((i) => Math.min(rows.length - 1, i + 1));
              }
              if (event.key === "ArrowUp") {
                event.preventDefault();
                setActive((i) => Math.max(0, i - 1));
              }
              if (event.key === "Enter" && chosen) {
                event.preventDefault();
                onClose();
                void ops.openFile(chosen);
              }
            }}
          />
          <span className="kbd">Esc</span>
        </div>

        {interpretation && interpretation.filters.length > 0 ? (
          <div className="quickfind-chips">
            {interpretation.filters.map((filter) => (
              <span className="chip" key={filter}>
                {filter}
              </span>
            ))}
          </div>
        ) : null}

        {interpretation?.needsContentUnderstanding ? (
          <div className="quickfind-note">
            <IconSparkle size={13} />
            <span>
              Finding files by {interpretation.needsContentUnderstanding} needs to read image
              pixels or document text, which Cleanup does not do yet. These results are matched on
              name, type, size and date only.
            </span>
          </div>
        ) : null}

        {error ? <div className="quickfind-note">{error}</div> : null}

        {text.trim().length === 0 ? (
          <div className="quickfind-examples">
            <span className="dim" style={{ fontSize: 11.5 }}>
              Try
            </span>
            {EXAMPLES.map((example) => (
              <button key={example} className="chip" onClick={() => setText(example)}>
                {example}
              </button>
            ))}
          </div>
        ) : (
          <>
            <div className="quickfind-summary">
              {formatCount(total)} {plural(total, "match", "matches")}
              {total > 0 ? ` · ${formatBytes(bytes)}` : ""}
              {total > rows.length ? ` · showing ${rows.length}` : ""}
            </div>
            <div className="quickfind-results">
              {rows.map((row, index) => (
                <button
                  key={row.id}
                  className={`quickfind-row ${index === active ? "active" : ""}`}
                  onMouseEnter={() => setActive(index)}
                  onClick={() => {
                    onClose();
                    void ops.openFile(row);
                  }}
                  onContextMenu={(event) => {
                    event.preventDefault();
                    addToQueue([row.id]);
                  }}
                >
                  <Thumb row={row} size={28} />
                  <div className="file-meta">
                    <span className="file-name">{row.name}</span>
                    <span className="file-sub">{row.parent}</span>
                  </div>
                  <span className="dim" style={{ fontSize: 11 }}>
                    {formatRelative(row.modified)}
                  </span>
                  <span className="legend-value">{formatBytes(row.size)}</span>
                </button>
              ))}
              {rows.length === 0 && !error ? (
                <div className="quickfind-empty">Nothing matched that.</div>
              ) : null}
            </div>
            {chosen ? (
              <div className="quickfind-foot">
                <span className="dim">
                  <span className="kbd">↵</span> open · <span className="kbd">↑↓</span> move ·
                  right-click to queue
                </span>
              </div>
            ) : null}
          </>
        )}
      </div>
    </div>
  );
}
