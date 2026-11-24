import { create } from "zustand";
import { api, IpcError, toIpcError } from "@/ipc/client";
import type { Settings } from "@/ipc/types";

interface SettingsStore {
  settings: Settings | null;
  saving: boolean;
  error: IpcError | null;

  load: () => Promise<void>;
  /** Patch and persist. Rust clamps and returns the authoritative result. */
  update: (patch: Partial<Settings>) => Promise<void>;
  addRoot: (path: string) => Promise<void>;
  removeRoot: (path: string) => Promise<void>;
  /** Mark a folder as never to be read or modified. */
  excludePath: (path: string) => Promise<void>;
  unexcludePath: (path: string) => Promise<void>;
  dismissError: () => void;
}

export const useSettingsStore = create<SettingsStore>((set, get) => ({
  settings: null,
  saving: false,
  error: null,

  load: async () => {
    try {
      set({ settings: await api.getSettings() });
    } catch (err) {
      set({ error: toIpcError(err) });
    }
  },

  update: async (patch) => {
    const current = get().settings;
    if (!current) return;
    set({ saving: true, error: null });
    try {
      // Rust sanitises, so the returned value - not the optimistic one - is
      // what gets stored. A clamped field then visibly snaps back, which is
      // better than the UI and the backend disagreeing.
      const saved = await api.saveSettings({ ...current, ...patch });
      set({ settings: saved, saving: false });
    } catch (err) {
      set({ error: toIpcError(err), saving: false });
    }
  },

  addRoot: async (path) => {
    set({ saving: true, error: null });
    try {
      set({ settings: await api.addRoot(path), saving: false });
    } catch (err) {
      set({ error: toIpcError(err), saving: false });
    }
  },

  removeRoot: async (path) => {
    set({ saving: true, error: null });
    try {
      set({ settings: await api.removeRoot(path), saving: false });
    } catch (err) {
      set({ error: toIpcError(err), saving: false });
    }
  },

  excludePath: async (path) => {
    set({ saving: true, error: null });
    try {
      set({ settings: await api.addExcludedPath(path), saving: false });
    } catch (err) {
      set({ error: toIpcError(err), saving: false });
    }
  },

  unexcludePath: async (path) => {
    set({ saving: true, error: null });
    try {
      set({ settings: await api.removeExcludedPath(path), saving: false });
    } catch (err) {
      set({ error: toIpcError(err), saving: false });
    }
  },

  dismissError: () => set({ error: null }),
}));
