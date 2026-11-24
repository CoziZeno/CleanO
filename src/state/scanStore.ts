import { create } from "zustand";
import { api, IpcError, toIpcError } from "@/ipc/client";
import { onScanDone, onScanFailed, onScanProgress } from "@/ipc/events";
import type {
  PlatformInfo,
  ScanErrorEntry,
  ScanProgress,
  ScanStatus,
  ScanSummary,
} from "@/ipc/types";

interface ScanStore {
  ready: boolean;
  status: ScanStatus;
  summary: ScanSummary | null;
  progress: ScanProgress | null;
  error: IpcError | null;
  warning: string | null;
  platform: PlatformInfo | null;
  scanErrors: ScanErrorEntry[];
  /** Bumped whenever a new scan lands, so views refetch their queries. */
  generation: number;

  init: () => Promise<void>;
  startScan: (root?: string | null) => Promise<void>;
  cancel: () => Promise<void>;
  dismissError: () => void;
  dismissWarning: () => void;
}

/**
 * Event subscriptions are registered once for the lifetime of the window.
 *
 * Deliberately not in a React effect: with StrictMode double-invoking effects
 * in development, that pattern subscribes twice and double-counts progress.
 */
let subscribed = false;

async function subscribe(): Promise<void> {
  if (subscribed) return;
  subscribed = true;

  await onScanProgress((progress) => {
    const { status } = useScanStore.getState();
    // Discard ticks that arrive after the scan already finished or failed.
    if (status !== "scanning") return;
    useScanStore.setState({ progress });
  });

  await onScanDone(async (summary) => {
    useScanStore.setState((state) => ({
      status: "ready",
      summary,
      progress: null,
      error: null,
      generation: state.generation + 1,
    }));
    if (summary.errorCount > 0) {
      try {
        useScanStore.setState({ scanErrors: await api.getScanErrors(100) });
      } catch {
        // The scan itself succeeded; failing to fetch the error list is not
        // worth surfacing on top of it.
      }
    } else {
      useScanStore.setState({ scanErrors: [] });
    }
  });

  await onScanFailed((shape) => {
    useScanStore.setState({
      status: "failed",
      progress: null,
      error: new IpcError(shape),
    });
  });
}

export const useScanStore = create<ScanStore>((set, get) => ({
  ready: false,
  status: "idle",
  summary: null,
  progress: null,
  error: null,
  warning: null,
  platform: null,
  scanErrors: [],
  generation: 0,

  init: async () => {
    if (get().ready) return;
    await subscribe();
    try {
      const [platform, warning, state] = await Promise.all([
        api.platformInfo(),
        api.takeSettingsWarning(),
        api.scanState(),
      ]);
      set({ platform, warning, status: state.status, ready: true });

      // A scan can already be complete if the window was reloaded during
      // development; pick the result up rather than showing an empty shell.
      if (state.status === "ready") {
        try {
          const summary = await api.getSummary();
          set((prev) => ({ summary, generation: prev.generation + 1 }));
        } catch {
          set({ status: "idle" });
        }
      }
    } catch (err) {
      set({ error: toIpcError(err), ready: true });
    }
  },

  startScan: async (root) => {
    if (get().status === "scanning") return;
    set({
      status: "scanning",
      error: null,
      progress: { scanId: 0, files: 0, dirs: 0, bytes: 0, errors: 0, current: "" },
    });
    try {
      await api.startScan(root ?? null);
    } catch (err) {
      set({ status: "failed", progress: null, error: toIpcError(err) });
    }
  },

  cancel: async () => {
    try {
      await api.cancelScan();
    } catch (err) {
      set({ error: toIpcError(err) });
    }
  },

  dismissError: () => set({ error: null }),
  dismissWarning: () => set({ warning: null }),
}));
