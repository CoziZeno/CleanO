import { useEffect } from "react";
import { ROUTES, type Route } from "./nav";

interface Handlers {
  onNavigate: (route: Route) => void;
  onFocusSearch: () => void;
  onEscape: () => void;
  onRescan: () => void;
  onToggleHelp: () => void;
  onUndoLast: () => void;
  onQuickFind: () => void;
  onSelectAll: () => void;
  onTrashSelection: () => void;
  onRename: () => void;
  onOpen: () => void;
  onCopyPath: () => void;
}

/**
 * True when the event came from somewhere the user is typing, or from inside
 * a dialog. Shortcuts must never fire while someone is editing a filename -
 * "d" would trash the file they are renaming.
 */
function isTyping(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || target.isContentEditable) {
    return true;
  }
  return target.closest('[role="dialog"]') !== null;
}

const isMac =
  typeof navigator !== "undefined" && navigator.platform.toLowerCase().includes("mac");
const modLabel = isMac ? "⌘" : "Ctrl";

export const SHORTCUTS: { keys: string[]; description: string }[] = [
  { keys: [modLabel, "K"], description: "Quick Find — describe what you want" },
  { keys: ["1", "–", "8"], description: "Jump to a section" },
  { keys: ["/"], description: "Search the file list" },
  { keys: ["R"], description: "Scan again" },
  { keys: [modLabel, "A"], description: "Select everything shown" },
  { keys: ["Del"], description: "Move the selection to the Recycle Bin" },
  { keys: ["Enter"], description: "Open the highlighted file" },
  { keys: ["F2"], description: "Rename" },
  { keys: [modLabel, "C"], description: "Copy the file path" },
  { keys: [modLabel, "Z"], description: "Undo the last operation" },
  { keys: ["?"], description: "Show this list" },
  { keys: ["Esc"], description: "Close, or stop a scan" },
];

/** Swipe Clean's own keys, listed here so they are discoverable too. */
export const SWIPE_SHORTCUTS: { keys: string[]; description: string }[] = [
  { keys: ["A"], description: "Keep — moves nothing" },
  { keys: ["D"], description: "Trash" },
  { keys: ["M"], description: "Move to a folder" },
  { keys: ["S"], description: "Review later" },
  { keys: ["←", "→"], description: "Previous / next file" },
];

export function useShortcuts(handlers: Handlers): void {
  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      const typing = isTyping(event.target);
      const mod = event.ctrlKey || event.metaKey;

      // Escape works everywhere, including inside the search box.
      if (event.key === "Escape") {
        handlers.onEscape();
        return;
      }

      // Modifier combinations that must work even while typing.
      if (mod && event.key.toLowerCase() === "z" && !event.shiftKey) {
        event.preventDefault();
        handlers.onUndoLast();
        return;
      }
      if (mod && event.key.toLowerCase() === "k") {
        event.preventDefault();
        handlers.onQuickFind();
        return;
      }

      if (typing) return;

      if (mod && event.key.toLowerCase() === "a") {
        event.preventDefault();
        handlers.onSelectAll();
        return;
      }
      if (mod && event.key.toLowerCase() === "c") {
        event.preventDefault();
        handlers.onCopyPath();
        return;
      }
      if (mod || event.altKey) return;

      switch (event.key) {
        case "Delete":
        case "Backspace":
          event.preventDefault();
          handlers.onTrashSelection();
          return;
        case "F2":
          event.preventDefault();
          handlers.onRename();
          return;
        case "Enter":
          event.preventDefault();
          handlers.onOpen();
          return;
        case "?":
          event.preventDefault();
          handlers.onToggleHelp();
          return;
        case "/":
          event.preventDefault();
          handlers.onFocusSearch();
          return;
        case "r":
        case "R":
          event.preventDefault();
          handlers.onRescan();
          return;
        default:
          break;
      }

      const digit = Number(event.key);
      if (Number.isInteger(digit) && digit >= 1 && digit <= ROUTES.length) {
        const route = ROUTES[digit - 1];
        if (route) {
          event.preventDefault();
          handlers.onNavigate(route);
        }
      }
    };

    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [handlers]);
}
