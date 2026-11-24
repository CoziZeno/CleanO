import { create } from "zustand";
import { api, IpcError, toIpcError } from "@/ipc/client";
import type { OperationRow, OrganizeMode, UndoOutcome } from "@/ipc/types";
import { useScanStore } from "./scanStore";

interface QueueStore {
  /** File ids picked for cleanup, from any screen. */
  selected: Set<number>;
  operations: OperationRow[];
  busy: boolean;
  error: IpcError | null;
  lastOperation: OperationRow | null;
  lastUndo: UndoOutcome | null;
  recoveryNotice: string | null;

  toggle: (id: number) => void;
  /** Range select, for Shift-click. */
  setRange: (ids: number[], selected: boolean) => void;
  add: (ids: number[]) => void;
  remove: (ids: number[]) => void;
  clear: () => void;
  isSelected: (id: number) => boolean;

  loadOperations: () => Promise<void>;
  loadRecoveryNotice: () => Promise<void>;
  dismissRecoveryNotice: () => void;
  trashSelected: () => Promise<void>;
  applyOrganize: (ids?: number[], mode?: OrganizeMode) => Promise<void>;
  moveTo: (destination: string) => Promise<void>;
  undo: (operationId: number) => Promise<void>;
  dismissError: () => void;
  dismissLast: () => void;
}

export const useQueueStore = create<QueueStore>((set, get) => ({
  selected: new Set<number>(),
  operations: [],
  busy: false,
  error: null,
  lastOperation: null,
  lastUndo: null,
  recoveryNotice: null,

  toggle: (id) =>
    set((state) => {
      const next = new Set(state.selected);
      if (!next.delete(id)) next.add(id);
      return { selected: next };
    }),

  setRange: (ids, selected) =>
    set((state) => {
      const next = new Set(state.selected);
      for (const id of ids) {
        if (selected) next.add(id);
        else next.delete(id);
      }
      return { selected: next };
    }),

  add: (ids) =>
    set((state) => {
      const next = new Set(state.selected);
      for (const id of ids) next.add(id);
      return { selected: next };
    }),

  remove: (ids) =>
    set((state) => {
      const next = new Set(state.selected);
      for (const id of ids) next.delete(id);
      return { selected: next };
    }),

  clear: () => set({ selected: new Set<number>() }),

  isSelected: (id) => get().selected.has(id),

  loadOperations: async () => {
    try {
      set({ operations: await api.listOperations(25) });
    } catch (err) {
      set({ error: toIpcError(err) });
    }
  },

  loadRecoveryNotice: async () => {
    try {
      const notice = await api.takeRecoveryNotice();
      if (notice) set({ recoveryNotice: notice });
    } catch {
      // Not worth surfacing on top of whatever else went wrong.
    }
  },

  dismissRecoveryNotice: () => set({ recoveryNotice: null }),

  trashSelected: async () => {
    const ids = Array.from(get().selected);
    if (ids.length === 0 || get().busy) return;
    set({ busy: true, error: null, lastUndo: null });
    try {
      const operation = await api.trashFiles(ids);
      set({ lastOperation: operation, selected: new Set<number>() });
      await get().loadOperations();
      // The index still describes files that are now in the Recycle Bin.
      // Rescanning is what stops deleted files lingering in the lists.
      await useScanStore.getState().startScan();
    } catch (err) {
      set({ error: toIpcError(err) });
    } finally {
      set({ busy: false });
    }
  },

  applyOrganize: async (ids, mode = "type") => {
    if (get().busy) return;
    set({ busy: true, error: null, lastUndo: null });
    try {
      const operation = await api.applyOrganize(ids, mode);
      set({ lastOperation: operation });
      await get().loadOperations();
    } catch (err) {
      set({ error: toIpcError(err) });
    } finally {
      set({ busy: false });
    }
  },

  moveTo: async (destination) => {
    const ids = Array.from(get().selected);
    if (ids.length === 0 || get().busy) return;
    set({ busy: true, error: null, lastUndo: null });
    try {
      const operation = await api.moveFilesTo(ids, destination);
      set({ lastOperation: operation, selected: new Set<number>() });
      await get().loadOperations();
      // Those files are no longer where the index says they are.
      await useScanStore.getState().startScan();
    } catch (err) {
      set({ error: toIpcError(err) });
    } finally {
      set({ busy: false });
    }
  },

  undo: async (operationId) => {
    if (get().busy) return;
    set({ busy: true, error: null });
    try {
      const outcome = await api.undoOperation(operationId);
      set({ lastUndo: outcome, lastOperation: null });
      await get().loadOperations();
      // Files have moved back, so the index is stale again.
      if (outcome.restored > 0) await useScanStore.getState().startScan();
    } catch (err) {
      set({ error: toIpcError(err) });
    } finally {
      set({ busy: false });
    }
  },

  dismissError: () => set({ error: null }),
  dismissLast: () => set({ lastOperation: null, lastUndo: null }),
}));
