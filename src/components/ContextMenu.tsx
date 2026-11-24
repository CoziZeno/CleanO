import { useEffect, useLayoutEffect, useRef, useState } from "react";

export interface MenuItem {
  label: string;
  onSelect: () => void;
  /** Shown right-aligned, e.g. "F2". Not bound here — the view owns the keys. */
  shortcut?: string;
  danger?: boolean;
  disabled?: boolean;
  /** Draw a divider above this item. */
  separatorBefore?: boolean;
}

export interface MenuState {
  x: number;
  y: number;
  items: MenuItem[];
}

/**
 * A real desktop context menu.
 *
 * The browser's own menu ("Reload", "Inspect", "Save image as…") is nonsense
 * inside a desktop app, so it is suppressed globally in `main.tsx` and this
 * takes its place. It flips away from the edges rather than being clipped,
 * closes on Escape, outside click, scroll or resize, and is keyboard
 * navigable.
 */
export function ContextMenu({
  state,
  onClose,
}: {
  state: MenuState | null;
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ x: 0, y: 0 });
  const [active, setActive] = useState(0);

  // Measure after mount, then flip so the menu never runs off-screen.
  useLayoutEffect(() => {
    if (!state || !ref.current) return;
    const rect = ref.current.getBoundingClientRect();
    const margin = 8;
    let x = state.x;
    let y = state.y;
    if (x + rect.width + margin > window.innerWidth) x = Math.max(margin, x - rect.width);
    if (y + rect.height + margin > window.innerHeight) y = Math.max(margin, y - rect.height);
    setPosition({ x, y });
    setActive(state.items.findIndex((item) => !item.disabled));
  }, [state]);

  useEffect(() => {
    if (!state) return;
    const close = () => onClose();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
        return;
      }
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        const step = event.key === "ArrowDown" ? 1 : -1;
        setActive((current) => {
          let next = current;
          for (let i = 0; i < state.items.length; i++) {
            next = (next + step + state.items.length) % state.items.length;
            if (!state.items[next]?.disabled) return next;
          }
          return current;
        });
        return;
      }
      if (event.key === "Enter") {
        event.preventDefault();
        const item = state.items[active];
        if (item && !item.disabled) {
          onClose();
          item.onSelect();
        }
      }
    };

    window.addEventListener("keydown", onKey, true);
    window.addEventListener("resize", close);
    // Capture phase: a scroll inside the virtualised list must close it too.
    window.addEventListener("scroll", close, true);
    window.addEventListener("mousedown", close);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("resize", close);
      window.removeEventListener("scroll", close, true);
      window.removeEventListener("mousedown", close);
    };
  }, [state, onClose, active]);

  if (!state) return null;

  return (
    <div
      className="ctxmenu"
      ref={ref}
      role="menu"
      style={{ left: position.x, top: position.y }}
      onMouseDown={(event) => event.stopPropagation()}
      onContextMenu={(event) => event.preventDefault()}
    >
      {state.items.map((item, index) => (
        <div key={`${item.label}-${index}`}>
          {item.separatorBefore ? <div className="ctxmenu-divider" /> : null}
          <button
            role="menuitem"
            className={`ctxmenu-item ${item.danger ? "danger" : ""} ${
              index === active ? "active" : ""
            }`}
            disabled={item.disabled}
            onMouseEnter={() => setActive(index)}
            onClick={() => {
              onClose();
              item.onSelect();
            }}
          >
            <span className="ctxmenu-label">{item.label}</span>
            {item.shortcut ? <span className="ctxmenu-shortcut">{item.shortcut}</span> : null}
          </button>
        </div>
      ))}
    </div>
  );
}
