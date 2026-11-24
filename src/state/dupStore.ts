import { create } from "zustand";
import { api, IpcError, toIpcError } from "@/ipc/client";
import { onDedupeDone, onDedupeFailed, onDedupeProgress } from "@/ipc/events";
import type { DedupeProgress, DedupeStatus, DuplicateReport } from "@/ipc/types";

interface DupStore {
  status: DedupeStatus;
  report: DuplicateReport | null;
  progress: DedupeProgress | null;
  error: IpcError | null;
  /** The scan these results describe, so a new scan can invalidate them. */
  scanId: number | null;

  start: () => Promise<void>;
  cancel: () => Promise<void>;
  /** Pick up a report that completed while the user was on another screen. */
  hydrate: () => Promise<void>;
  reset: () => void;
  dismissError: () => void;
}

/** Registered once per window, not in an effect — see the note in scanStore. */
let subscribed = false;

async function subscribe(): Promise<void> {
  if (subscribed) return;
  subscribed = true;

  await onDedupeProgress((progress) => {
    if (useDupStore.getState().status !== "running") return;
    useDupStore.setState({ progress });
  });

  await onDedupeDone((report) => {
    useDupStore.setState({
      status: "ready",
      report,
      scanId: report.scanId,
      progress: null,
      error: null,
    });
  });

  await onDedupeFailed((shape) => {
    useDupStore.setState({
      status: "failed",
      progress: null,
      error: new IpcError(shape),
    });
  });
}

export const useDupStore = create<DupStore>((set, get) => ({
  status: "idle",
  report: null,
  progress: null,
  error: null,
  scanId: null,

  start: async () => {
    if (get().status === "running") return;
    await subscribe();
    set({
      status: "running",
      error: null,
      report: null,
      progress: {
        scanId: 0,
        stage: "grouping",
        filesDone: 0,
        filesTotal: 0,
        bytesDone: 0,
        bytesTotal: 0,
      },
    });
    try {
      await api.startDedupe();
    } catch (err) {
      set({ status: "failed", progress: null, error: toIpcError(err) });
    }
  },

  cancel: async () => {
    try {
      await api.cancelDedupe();
    } catch (err) {
      set({ error: toIpcError(err) });
    }
  },

  hydrate: async () => {
    await subscribe();
    if (get().report || get().status === "running") return;
    try {
      const status = await api.dedupeStatus();
      if (status === "ready") {
        const report = await api.getDuplicates();
        set({ status: "ready", report, scanId: report.scanId });
      } else {
        set({ status });
      }
    } catch (err) {
      const ipc = toIpcError(err);
      // "Not run yet" and "from an older scan" are both just the idle state.
      if (ipc.code === "noDuplicateScan" || ipc.code === "staleDuplicates" || ipc.code === "noScan") {
        set({ status: "idle", report: null, scanId: null });
      } else {
        set({ error: ipc });
      }
    }
  },

  reset: () =>
    set({ status: "idle", report: null, progress: null, error: null, scanId: null }),

  dismissError: () => set({ error: null }),
}));
