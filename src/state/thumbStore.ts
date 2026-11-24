import { create } from "zustand";
import { api } from "@/ipc/client";
import type { FileRow } from "@/ipc/types";
import { isPreviewable } from "@/lib/categories";

interface Entry {
  /** A data URL, or null when there is no preview to show. */
  data: string | null;
  reason: string | null;
}

interface ThumbStore {
  entries: Map<number, Entry>;
  enabled: boolean;
  setEnabled: (enabled: boolean) => void;
  /** Ask for previews of rows now on screen. Already-known ids are skipped. */
  request: (rows: FileRow[]) => void;
  get: (id: number) => Entry | undefined;
  clear: () => void;
}

/**
 * Ids already requested, so a scroll back and forth does not re-ask. Kept
 * outside the store because it is bookkeeping, not state the UI renders.
 */
let inFlight = new Set<number>();
let pending: FileRow[] = [];
let timer: ReturnType<typeof setTimeout> | null = null;

/** Rows per request. Small enough to arrive quickly while scrolling. */
const BATCH = 24;
/** Coalescing window: a fast scroll should ask for where it landed. */
const DEBOUNCE_MS = 90;

async function flush() {
  timer = null;
  const rows = pending;
  pending = [];
  if (rows.length === 0) return;

  const ids = rows.slice(0, BATCH).map((row) => row.id);
  // Anything beyond the batch goes back in the queue rather than being lost.
  if (rows.length > BATCH) {
    pending = rows.slice(BATCH);
    schedule();
  }

  try {
    const results = await api.getThumbnails(ids);
    useThumbStore.setState((state) => {
      const entries = new Map(state.entries);
      for (const result of results) {
        entries.set(result.id, { data: result.data, reason: result.reason });
      }
      return { entries };
    });
  } catch {
    // Leave them unrequested so a later scroll can try again.
    for (const id of ids) inFlight.delete(id);
  }
}

function schedule() {
  if (timer !== null) return;
  timer = setTimeout(() => void flush(), DEBOUNCE_MS);
}

export const useThumbStore = create<ThumbStore>((set, get) => ({
  entries: new Map(),
  enabled: true,

  setEnabled: (enabled) => {
    set({ enabled });
    if (!enabled) get().clear();
  },

  request: (rows) => {
    if (!get().enabled) return;
    const known = get().entries;
    const wanted = rows.filter(
      (row) => isPreviewable(row) && !known.has(row.id) && !inFlight.has(row.id),
    );
    if (wanted.length === 0) return;
    for (const row of wanted) inFlight.add(row.id);
    pending.push(...wanted);
    schedule();
  },

  get: (id) => get().entries.get(id),

  clear: () => {
    inFlight = new Set();
    pending = [];
    set({ entries: new Map() });
  },
}));
