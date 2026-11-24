import { SHORTCUTS } from "@/app/shortcuts";
import { IconClose } from "./icons";

/**
 * The `?` overlay. Reads its contents from the same list the key handler uses,
 * so a shortcut cannot be added without appearing here.
 */
export function ShortcutsOverlay({ onClose }: { onClose: () => void }) {
  return (
    <div className="overlay" role="dialog" aria-modal="true" aria-label="Keyboard shortcuts">
      {/* Clicking the backdrop closes, same as Escape. */}
      <button className="overlay-backdrop" aria-label="Close" onClick={onClose} />
      <div className="overlay-panel">
        <div className="card-head" style={{ marginBottom: 10 }}>
          <h2>Keyboard shortcuts</h2>
          <button className="btn-ghost" onClick={onClose} aria-label="Close">
            <IconClose size={14} />
          </button>
        </div>
        <div className="shortcut-list">
          {SHORTCUTS.map((shortcut) => (
            <div className="shortcut-row" key={shortcut.description}>
              <span className="shortcut-keys">
                {shortcut.keys.map((key, index) =>
                  key === "–" ? (
                    <span className="dim" key={index}>
                      –
                    </span>
                  ) : (
                    <span className="kbd" key={index}>
                      {key}
                    </span>
                  ),
                )}
              </span>
              <span className="muted">{shortcut.description}</span>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
