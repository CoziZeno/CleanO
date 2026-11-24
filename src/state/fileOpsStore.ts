import { create } from "zustand";
import { open } from "@tauri-apps/plugin-dialog";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { api, toIpcError, type IpcError } from "@/ipc/client";
import type { FileRow } from "@/ipc/types";
import { useQueueStore } from "./queueStore";
import { useScanStore } from "./scanStore";

/** A file awaiting a rename, or a pending "new folder" prompt. */
export type Prompt =
  | { kind: "rename"; row: FileRow }
  | { kind: "newFolder"; ids: number[] }
  | null;

interface FileOpsStore {
  prompt: Prompt;
  promptError: string | null;
  busy: boolean;
  error: IpcError | null;
  /** Last operation's one-line result, for a transient toast. */
  notice: string | null;

  openFile: (row: FileRow) => Promise<void>;
  revealFile: (row: FileRow) => Promise<void>;
  copyPath: (row: FileRow) => Promise<void>;
  moveTo: (ids: number[]) => Promise<void>;
  copyTo: (ids: number[]) => Promise<void>;
  trash: (ids: number[]) => Promise<void>;

  startRename: (row: FileRow) => void;
  startNewFolder: (ids: number[]) => void;
  submitPrompt: (value: string) => Promise<void>;
  cancelPrompt: () => void;

  dismissError: () => void;
  dismissNotice: () => void;
}

/** A rescan is the only way the index learns that files moved. */
async function refresh() {
  await useScanStore.getState().startScan();
}

export const useFileOpsStore = create<FileOpsStore>((set, get) => ({
  prompt: null,
  promptError: null,
  busy: false,
  error: null,
  notice: null,

  openFile: async (row) => {
    try {
      // The path is re-resolved and re-guarded in Rust rather than trusting
      // the display string the UI is holding.
      await openPath(await api.filePath(row.id));
    } catch (err) {
      set({ error: toIpcError(err) });
    }
  },

  revealFile: async (row) => {
    try {
      await revealItemInDir(await api.filePath(row.id));
    } catch (err) {
      set({ error: toIpcError(err) });
    }
  },

  copyPath: async (row) => {
    try {
      const path = await api.filePath(row.id);
      await navigator.clipboard.writeText(path);
      set({ notice: "Path copied" });
    } catch (err) {
      set({ error: toIpcError(err) });
    }
  },

  moveTo: async (ids) => {
    if (ids.length === 0 || get().busy) return;
    const picked = await open({ directory: true, multiple: false, title: "Move to…" });
    if (typeof picked !== "string") return;
    set({ busy: true, error: null });
    try {
      const operation = await api.moveFilesTo(ids, picked);
      set({ notice: `${operation.done} moved${operation.failed ? `, ${operation.failed} failed` : ""}` });
      useQueueStore.getState().remove(ids);
      await useQueueStore.getState().loadOperations();
      await refresh();
    } catch (err) {
      set({ error: toIpcError(err) });
    } finally {
      set({ busy: false });
    }
  },

  copyTo: async (ids) => {
    if (ids.length === 0 || get().busy) return;
    const picked = await open({ directory: true, multiple: false, title: "Copy to…" });
    if (typeof picked !== "string") return;
    set({ busy: true, error: null });
    try {
      const operation = await api.copyFilesTo(ids, picked);
      set({ notice: `${operation.done} copied${operation.failed ? `, ${operation.failed} failed` : ""}` });
      await useQueueStore.getState().loadOperations();
      // Copies land in the destination; if that is inside the scanned folder
      // the index needs to learn about them.
      await refresh();
    } catch (err) {
      set({ error: toIpcError(err) });
    } finally {
      set({ busy: false });
    }
  },

  trash: async (ids) => {
    if (ids.length === 0) return;
    useQueueStore.getState().add(ids);
    await useQueueStore.getState().trashSelected();
  },

  startRename: (row) => set({ prompt: { kind: "rename", row }, promptError: null }),
  startNewFolder: (ids) => set({ prompt: { kind: "newFolder", ids }, promptError: null }),
  cancelPrompt: () => set({ prompt: null, promptError: null }),

  submitPrompt: async (value) => {
    const prompt = get().prompt;
    if (!prompt) return;
    set({ busy: true, promptError: null });
    try {
      if (prompt.kind === "rename") {
        await api.renameFile(prompt.row.id, value);
        set({ prompt: null, notice: `Renamed to ${value}` });
      } else {
        const operation = await api.createFolderAndMove(prompt.ids, value);
        set({
          prompt: null,
          notice: `${operation.done} moved into ${value}${
            operation.failed ? `, ${operation.failed} failed` : ""
          }`,
        });
        useQueueStore.getState().remove(prompt.ids);
      }
      await useQueueStore.getState().loadOperations();
      await refresh();
    } catch (err) {
      // Keep the dialog open with the reason — a name clash should be
      // correctable without retyping everything.
      set({ promptError: toIpcError(err).message });
    } finally {
      set({ busy: false });
    }
  },

  dismissError: () => set({ error: null }),
  dismissNotice: () => set({ notice: null }),
}));
