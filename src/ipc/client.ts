/**
 * The only module in the frontend that calls `invoke`.
 *
 * Features consume `api`, never Tauri directly. When a Rust command changes
 * shape, this file is the single thing that breaks, and the compiler then points
 * at every caller.
 */

import { invoke } from "@tauri-apps/api/core";
import type {
  AppErrorShape,
  DedupeStatus,
  DuplicateReport,
  FileRow,
  InstalledApp,
  InstallerMatchRow,
  LifetimeStats,
  OperationRow,
  OrganizeMode,
  OrganizePlan,
  PlatformInfo,
  QueryRequest,
  QueryResponse,
  ScanErrorEntry,
  ScanState,
  ScanSummary,
  QuickFindResponse,
  Drive,
  QuickCleanMeasurement,
  QuickCleanOutcome,
  Settings,
  TreemapNode,
  StorageReport,
  SwipeDecision,
  SwipeQueue,
  ThumbResult,
  UndoOutcome,
} from "./types";

/**
 * A failure that came back from Rust, carrying its machine-readable `code` and
 * the recovery `hint` separately from the diagnosis.
 */
export class IpcError extends Error {
  readonly code: string;
  readonly hint: string | null;

  constructor(shape: AppErrorShape) {
    super(shape.message);
    this.name = "IpcError";
    this.code = shape.code;
    this.hint = shape.hint;
  }
}

function isErrorShape(value: unknown): value is AppErrorShape {
  return (
    typeof value === "object" &&
    value !== null &&
    "code" in value &&
    "message" in value &&
    typeof (value as AppErrorShape).message === "string"
  );
}

/** Normalise anything a rejected invoke can throw into one error type. */
export function toIpcError(err: unknown): IpcError {
  if (err instanceof IpcError) return err;
  if (isErrorShape(err)) return new IpcError(err);
  if (typeof err === "string") return new IpcError({ code: "unknown", message: err, hint: null });
  return new IpcError({
    code: "unknown",
    message: err instanceof Error ? err.message : "Something went wrong.",
    hint: null,
  });
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (err) {
    throw toIpcError(err);
  }
}

export const api = {
  platformInfo: () => call<PlatformInfo>("platform_info"),
  scanState: () => call<ScanState>("scan_state"),

  /** Returns the new scan id; results arrive as events. */
  startScan: (root?: string | null) => call<number>("start_scan", { root: root ?? null }),
  /** True if a scan was actually running. */
  cancelScan: () => call<boolean>("cancel_scan"),

  getSummary: () => call<ScanSummary>("get_summary"),
  getScanErrors: (limit = 100) => call<ScanErrorEntry[]>("get_scan_errors", { limit }),

  queryFiles: (request: QueryRequest) => call<QueryResponse>("query_files", { request }),
  getRows: (ids: number[]) => call<FileRow[]>("get_rows", { ids }),
  /** Every id matching a filter, for "select all". Capped server-side. */
  queryIds: (request: QueryRequest, limit = 10_000) =>
    call<number[]>("query_ids", { request, limit }),
  /** Read-only: used for "Reveal in Explorer/Finder". */
  resolvePath: (id: number) => call<string>("resolve_path", { id }),

  dedupeStatus: () => call<DedupeStatus>("dedupe_status"),
  /** Returns the scan id the results will belong to; the report arrives as an event. */
  startDedupe: () => call<number>("start_dedupe"),
  cancelDedupe: () => call<boolean>("cancel_dedupe"),
  getDuplicates: () => call<DuplicateReport>("get_duplicates"),
  hashCacheCount: () => call<number>("hash_cache_count"),
  clearHashCache: () => call<number>("clear_hash_cache"),

  getStorageReport: () => call<StorageReport>("get_storage_report"),

  getOrganizePlan: (mode: OrganizeMode = "type") =>
    call<OrganizePlan>("get_organize_plan", { mode }),

  /** Moves files to the Recycle Bin / Trash. Never a permanent delete. */
  trashFiles: (ids: number[]) => call<OperationRow>("trash_files", { ids }),
  /** `ids` omitted means the whole plan. */
  applyOrganize: (ids?: number[], mode: OrganizeMode = "type") =>
    call<OperationRow>("apply_organize", { ids: ids ?? null, mode }),
  /** Move chosen files into a folder the user picked. Journalled and undoable. */
  moveFilesTo: (ids: number[], destination: string) =>
    call<OperationRow>("move_files_to", { ids, destination }),
  undoOperation: (operationId: number) =>
    call<UndoOutcome>("undo_operation", { operationId }),
  listOperations: (limit = 25) => call<OperationRow[]>("list_operations", { limit }),
  takeRecoveryNotice: () => call<string | null>("take_recovery_notice"),
  lifetimeStats: () => call<LifetimeStats>("lifetime_stats"),

  getThumbnails: (ids: number[], maxPx?: number) =>
    call<ThumbResult[]>("get_thumbnails", { ids, maxPx: maxPx ?? null }),
  /** `[count, bytes]` held by cached previews. */
  thumbCacheUsage: () => call<[number, number]>("thumb_cache_usage"),
  clearThumbCache: () => call<number>("clear_thumb_cache"),

  /** Read-only. Cleanup never uninstalls anything. */
  listInstalledApps: (refresh = false) =>
    call<InstalledApp[]>("list_installed_apps", { refresh }),
  /** Installers in the current scan that match installed software. */
  matchInstallers: () => call<InstallerMatchRow[]>("match_installers"),

  // ---- file manager -------------------------------------------------------
  /** Guarded absolute path, for opening or copying to the clipboard. */
  filePath: (id: number) => call<string>("file_path", { id }),
  renameFile: (id: number, newName: string) =>
    call<OperationRow>("rename_file", { id, newName }),
  copyFilesTo: (ids: number[], destination: string) =>
    call<OperationRow>("copy_files_to", { ids, destination }),
  createFolder: (name: string, parent?: string) =>
    call<string>("create_folder", { name, parent: parent ?? null }),
  createFolderAndMove: (ids: number[], name: string, parent?: string) =>
    call<OperationRow>("create_folder_and_move", { ids, name, parent: parent ?? null }),
  rowsFor: (ids: number[]) => call<FileRow[]>("rows_for", { ids }),

  // ---- swipe clean --------------------------------------------------------
  swipeQueue: (laterOnly = false) => call<SwipeQueue>("swipe_queue", { laterOnly }),
  swipeDecide: (ids: number[], decision: SwipeDecision) =>
    call<void>("swipe_decide", { ids, decision }),
  swipeForget: (ids: number[]) => call<void>("swipe_forget", { ids }),
  swipeReset: () => call<number>("swipe_reset"),

  // ---- quick find ---------------------------------------------------------
  quickFind: (text: string) => call<QuickFindResponse>("quick_find", { text }),

  quickCleanScan: () => call<QuickCleanMeasurement[]>("quick_clean_scan"),
  quickCleanRun: (keys: string[]) => call<QuickCleanOutcome[]>("quick_clean_run", { keys }),
  listDrives: () => call<Drive[]>("list_drives"),
  addDriveRoot: (path: string) => call<Settings>("add_drive_root", { path }),
  getTreemap: (path?: string) => call<TreemapNode>("get_treemap", { path: path ?? null }),
  getSettings: () => call<Settings>("get_settings"),
  saveSettings: (settings: Settings) => call<Settings>("save_settings", { settings }),
  takeSettingsWarning: () => call<string | null>("take_settings_warning"),
  addRoot: (path: string) => call<Settings>("add_root", { path }),
  removeRoot: (path: string) => call<Settings>("remove_root", { path }),
  addExcludedPath: (path: string) => call<Settings>("add_excluded_path", { path }),
  removeExcludedPath: (path: string) => call<Settings>("remove_excluded_path", { path }),
};
