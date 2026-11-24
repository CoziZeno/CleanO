import { useEffect, useRef, useState } from "react";

/**
 * A single-field dialog for renaming and naming new folders.
 *
 * `window.prompt` is blocked in the webview and looks like a browser anyway.
 * This selects the filename stem on open, the way Explorer and Finder do, so
 * typing replaces the name and keeps the extension.
 */
export function PromptDialog({
  title,
  label,
  initial,
  confirmLabel = "Save",
  selectStem = false,
  error,
  busy,
  onConfirm,
  onCancel,
}: {
  title: string;
  label: string;
  initial: string;
  confirmLabel?: string;
  /** Preselect up to the last dot, so the extension survives typing. */
  selectStem?: boolean;
  error?: string | null;
  busy?: boolean;
  onConfirm: (value: string) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = useState(initial);
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const field = input.current;
    if (!field) return;
    field.focus();
    const dot = initial.lastIndexOf(".");
    if (selectStem && dot > 0) field.setSelectionRange(0, dot);
    else field.select();
  }, [initial, selectStem]);

  const submit = () => {
    const trimmed = value.trim();
    if (trimmed.length > 0 && !busy) onConfirm(trimmed);
  };

  return (
    <div className="overlay" role="dialog" aria-modal="true" aria-label={title}>
      <button className="overlay-backdrop" aria-label="Cancel" onClick={onCancel} />
      <div className="overlay-panel" style={{ width: "min(420px, calc(100vw - 48px))" }}>
        <h2 style={{ marginBottom: 10 }}>{title}</h2>
        <label className="dialog-label" htmlFor="prompt-input">
          {label}
        </label>
        <input
          id="prompt-input"
          ref={input}
          className="dialog-input"
          value={value}
          spellCheck={false}
          disabled={busy}
          onChange={(event) => setValue(event.target.value)}
          onKeyDown={(event) => {
            // Stop the app-level shortcuts from seeing these keys.
            event.stopPropagation();
            if (event.key === "Enter") submit();
            if (event.key === "Escape") onCancel();
          }}
        />
        {error ? <div className="dialog-error">{error}</div> : null}
        <div className="dialog-actions">
          <button className="btn" onClick={onCancel} disabled={busy}>
            Cancel
          </button>
          <button
            className="btn btn-primary"
            onClick={submit}
            disabled={busy || value.trim().length === 0}
          >
            {busy ? "Working…" : confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
