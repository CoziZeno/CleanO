/**
 * TypeScript mirror of `src-tauri/src/core/dto.rs`.
 *
 * This is the one place where the two languages can drift, which is why it is a
 * single file with nothing else in it: when a Rust DTO changes, exactly one
 * file needs updating and the compiler points at every consumer.
 */

export type Category =
  | "images"
  | "videos"
  | "audio"
  | "documents"
  | "archives"
  | "installers"
  | "development"
  | "temporary"
  | "misc";

export const CATEGORIES: Category[] = [
  "images",
  "videos",
  "audio",
  "documents",
  "archives",
  "installers",
  "development",
  "temporary",
  "misc",
];

export const CATEGORY_LABELS: Record<Category, string> = {
  images: "Images",
  videos: "Videos",
  audio: "Audio",
  documents: "Documents",
  archives: "Archives",
  installers: "Installers",
  development: "Development",
  temporary: "Temporary",
  misc: "Other",
};

/** Mirrors `EntryFlags` in `core/model.rs`. Keep the bit values in step. */
export const Flag = {
  HIDDEN: 1 << 0,
  SYMLINK: 1 << 1,
  READONLY: 1 << 2,
  CLOUD_PLACEHOLDER: 1 << 3,
  IN_PROGRESS: 1 << 4,
  SYSTEM_PROTECTED: 1 << 5,
  APP_BUNDLE: 1 << 6,
  REPARSE: 1 << 7,
  NOISE: 1 << 8,
} as const;

/** Must equal `EntryFlags::UNSAFE_TO_TOUCH`. */
export const UNSAFE_TO_TOUCH =
  Flag.SYMLINK |
  Flag.CLOUD_PLACEHOLDER |
  Flag.IN_PROGRESS |
  Flag.SYSTEM_PROTECTED |
  Flag.APP_BUNDLE |
  Flag.REPARSE;

export const hasFlag = (flags: number, bit: number): boolean => (flags & bit) !== 0;
export const isSafeToTouch = (flags: number): boolean => (flags & UNSAFE_TO_TOUCH) === 0;

/**
 * Why an entry cannot be acted on, for the UI to show instead of silently
 * disabling a row. Order matters: the most consequential reason wins.
 */
export function untouchableReason(flags: number): string | null {
  if (hasFlag(flags, Flag.CLOUD_PLACEHOLDER))
    return "Stored in the cloud — reading it would download the whole file";
  if (hasFlag(flags, Flag.IN_PROGRESS)) return "Still being downloaded";
  if (hasFlag(flags, Flag.SYSTEM_PROTECTED)) return "System file — Cleanup never touches these";
  if (hasFlag(flags, Flag.APP_BUNDLE)) return "Application bundle — treated as one item";
  if (hasFlag(flags, Flag.SYMLINK)) return "Shortcut or symlink, not a real file";
  if (hasFlag(flags, Flag.REPARSE)) return "Junction or reparse point, not a real file";
  return null;
}

export interface FileRow {
  id: number;
  name: string;
  path: string;
  parent: string;
  ext: string | null;
  size: number;
  created: number | null;
  modified: number | null;
  accessed: number | null;
  category: Category;
  flags: number;
}

export interface CategoryStat {
  category: Category;
  key: string;
  count: number;
  bytes: number;
}

export interface FlaggedCounts {
  inProgress: number;
  cloudPlaceholders: number;
  symlinks: number;
  temporary: number;
  hidden: number;
}

export interface EmptyFolders {
  count: number;
  sample: string[];
}

/**
 * Candidates from size grouping alone — zero disk reads.
 * `reclaimableBytesUpperBound` is a ceiling, not a promise; confirming it needs
 * hashing (Phase 3).
 */
export interface DuplicateCandidates {
  groups: number;
  files: number;
  reclaimableBytesUpperBound: number;
}

export interface StaleStats {
  days: number;
  count: number;
  bytes: number;
  /** False on Windows, where this came from modification time instead. */
  fromAccessTime: boolean;
}

export interface ScanSummary {
  scanId: number;
  root: string;
  rootDisplay: string;
  scannedAt: number;
  durationMs: number;
  truncated: boolean;
  cancelled: boolean;
  totalFiles: number;
  totalDirs: number;
  totalBytes: number;
  byCategory: CategoryStat[];
  largestFiles: FileRow[];
  oldestFiles: FileRow[];
  recentFiles: FileRow[];
  emptyFolders: EmptyFolders;
  duplicateCandidates: DuplicateCandidates;
  stale: StaleStats;
  flagged: FlaggedCounts;
  errorCount: number;
  accessTimeReliable: boolean;
}

export type SortKey = "name" | "size" | "modified" | "created" | "category" | "extension";

export interface QueryRequest {
  categories: Category[];
  text: string | null;
  ext: string | null;
  minSize: number | null;
  maxSize: number | null;
  modifiedAfter: number | null;
  modifiedBefore: number | null;
  parentPrefix: string | null;
  requireFlags: number;
  excludeFlags: number;
  sort: SortKey;
  descending: boolean;
  offset: number;
  limit: number;
}

export interface QueryResponse {
  total: number;
  bytes: number;
  rows: FileRow[];
}

export interface PlatformInfo {
  os: string;
  downloadsDir: string | null;
  homeDir: string | null;
  accessTimeReliable: boolean;
  privacyPrompts: boolean;
}

export type ScanStatus = "idle" | "scanning" | "ready" | "failed";

export interface ScanState {
  status: ScanStatus;
  scanId: number | null;
  root: string | null;
}

export interface ScanProgress {
  scanId: number;
  files: number;
  dirs: number;
  bytes: number;
  errors: number;
  current: string;
}

export type ScanErrorKind = "permissionDenied" | "notFound" | "accessRestricted" | "other";

export interface ScanErrorEntry {
  path: string;
  kind: ScanErrorKind;
  message: string;
}

/** Mirrors the `AppError` serialisation in `src-tauri/src/error.rs`. */
export interface AppErrorShape {
  code: string;
  message: string;
  hint: string | null;
}

export interface ScanSettings {
  maxDepth: number;
  includeHidden: boolean;
  sniffExtensionless: boolean;
}

export interface HashSettings {
  concurrency: number;
  headBytes: number;
  verifyBytewise: boolean;
}

/**
 * Which files Swipe Clean puts in front of you. `categories` is in review
 * order — the first entry is shown first. `extensions`, when non-empty, is a
 * filter applied *within* those categories, not an addition to them.
 */
export interface SwipeSettings {
  categories: Category[];
  extensions: string[];
  minBytes: number;
}

export interface Settings {
  schemaVersion: number;
  theme: "dark" | "light";
  roots: string[];
  scan: ScanSettings;
  hash: HashSettings;
  staleDays: number;
  ignore: string[];
  /**
   * Folders Cleanup must never read or modify, matched by whole path. Distinct
   * from `ignore`, which matches file *names* anywhere in the tree.
   */
  excludedPaths: string[];
  swipe: SwipeSettings;
  organizeFolders: Record<string, string>;
}

// ------------------------------------------------------- storage analyzer ---

export interface FolderRow {
  path: string;
  display: string;
  /** Recursive: this folder and everything beneath it. */
  bytes: number;
  files: number;
  depth: number;
}

export interface StorageReport {
  totalBytes: number;
  totalFiles: number;
  largestFolders: FolderRow[];
  largestFiles: FileRow[];
  staleFiles: FileRow[];
  temporaryFiles: FileRow[];
  byCategory: CategoryStat[];
  emptyFolders: EmptyFolders;
  stale: StaleStats;
  truncated: boolean;
}

// ------------------------------------------------------------ duplicates ---

export interface DuplicateGroup {
  /** First 12 hex characters of the BLAKE3 digest. */
  hash: string;
  size: number;
  /** Freed only if every copy but one goes. */
  reclaimable: number;
  keepId: number;
  /** Why this copy was suggested. A suggestion, never a selection. */
  keepReason: string;
  /** `keepId` first. */
  files: FileRow[];
}

export interface DuplicateStats {
  considered: number;
  skippedUnsafe: number;
  eliminatedBySize: number;
  collapsedHardlinks: number;
  eliminatedByHead: number;
  headHashed: number;
  fullHashed: number;
  cacheHits: number;
  bytesRead: number;
  duplicateFiles: number;
  groups: number;
  reclaimable: number;
  verifiedBytewise: boolean;
  cancelled: boolean;
  durationMs: number;
  errorCount: number;
  errors: string[];
}

export interface DuplicateReport {
  scanId: number;
  groups: DuplicateGroup[];
  stats: DuplicateStats;
}

export type DedupeStage = "grouping" | "headHashing" | "fullHashing" | "verifying" | "done";

export const DEDUPE_STAGE_LABELS: Record<DedupeStage, string> = {
  grouping: "Grouping by size",
  headHashing: "Comparing file starts",
  fullHashing: "Comparing full contents",
  verifying: "Checking byte for byte",
  done: "Finished",
};

export interface DedupeProgress {
  scanId: number;
  stage: DedupeStage;
  filesDone: number;
  filesTotal: number;
  bytesDone: number;
  bytesTotal: number;
}

export type DedupeStatus = "idle" | "running" | "ready" | "failed";

// --------------------------------------------------- operations and undo ---

export type OperationState = "planned" | "running" | "done" | "partial" | "failed" | "undone";
export type ItemState = "pending" | "done" | "failed" | "reverted";

export interface OperationItemRow {
  name: string;
  fromPath: string;
  toPath: string | null;
  size: number;
  state: ItemState;
  error: string | null;
}

export interface OperationRow {
  id: number;
  kind: "organize" | "trash";
  state: OperationState;
  createdAt: number;
  note: string;
  root: string;
  done: number;
  failed: number;
  bytes: number;
  undoable: boolean;
  items: OperationItemRow[];
}

// -------------------------------------------------------- installed apps ---

export type AppSource = "machine" | "machine32" | "user" | "bundle";

export interface InstalledApp {
  id: string;
  name: string;
  version: string | null;
  publisher: string | null;
  installedAt: number | null;
  location: string | null;
  /** As reported by the installer — often missing, often approximate. */
  estimatedBytes: number | null;
  source: AppSource;
}

export type VersionVerdict = "older" | "same" | "newer" | "unknown";

export interface InstallerMatch {
  appName: string;
  installedVersion: string | null;
  installerVersion: string | null;
  verdict: VersionVerdict;
}

export interface InstallerMatchRow {
  file: FileRow;
  matched: InstallerMatch;
}

// ------------------------------------------------------------ swipe clean ---

export type SwipeDecision = "kept" | "trashed" | "moved" | "later";

export interface SwipeProgress {
  reviewed: number;
  kept: number;
  trashed: number;
  moved: number;
  later: number;
  remaining: number;
  total: number;
}

export interface SwipeQueue {
  files: FileRow[];
  progress: SwipeProgress;
}

// ------------------------------------------------------------- quick find ---

export interface QuickFindInterpretation {
  /** One line per filter applied, so the UI can show its working. */
  filters: string[];
  text: string | null;
  /** Set when the request needs pixels or document text we cannot read. */
  needsContentUnderstanding: string | null;
}

export interface QuickFindResponse {
  interpretation: QuickFindInterpretation;
  total: number;
  bytes: number;
  rows: FileRow[];
}

export interface ThumbResult {
  id: number;
  /** A ready-to-use data URL, or null when there is no preview. */
  data: string | null;
  width: number;
  height: number;
  /** Why there is no preview, when `data` is null. */
  reason: string | null;
}

/** Running totals of everything tidied, from the operation journal. */
export interface LifetimeStats {
  files: number;
  bytes: number;
  operations: number;
}

export interface UndoOutcome {
  operationId: number;
  restored: number;
  failed: number;
  problems: string[];
  /** Trash operations: the OS owns restoring, so we say so rather than fake it. */
  manualRestoreRequired: boolean;
}

// ------------------------------------------------ organization suggestions ---

export interface PlanItem {
  id: number;
  name: string;
  fromPath: string;
  /** Folder name inside the scan root, e.g. "Images". */
  destination: string;
  toPath: string;
  size: number;
  category: Category;
  /** Something already exists at `toPath`, so the move would be renamed. */
  collides: boolean;
  /** Name that would actually be used, when `collides` is true. */
  resolvedName: string | null;
}

export interface PlanGroup {
  destination: string;
  category: Category;
  files: number;
  bytes: number;
  items: PlanItem[];
}

export type OrganizeMode = "type" | "year" | "yearMonth" | "extension";

export const ORGANIZE_MODES: { mode: OrganizeMode; label: string; help: string }[] = [
  { mode: "type", label: "By type", help: "Images, Documents, Installers…" },
  { mode: "year", label: "By year", help: "2024, 2025…" },
  { mode: "yearMonth", label: "By month", help: "2024 › March" },
  { mode: "extension", label: "By extension", help: "PNG, PDF, ZIP…" },
];

export interface OrganizePlan {
  scanId: number;
  root: string;
  groups: PlanGroup[];
  totalFiles: number;
  totalBytes: number;
  /** Files left where they are, with the reason, so nothing is unexplained. */
  skipped: number;
  collisions: number;
}

export function emptyQuery(overrides: Partial<QueryRequest> = {}): QueryRequest {
  return {
    categories: [],
    text: null,
    ext: null,
    minSize: null,
    maxSize: null,
    modifiedAfter: null,
    modifiedBefore: null,
    parentPrefix: null,
    requireFlags: 0,
    excludeFlags: 0,
    sort: "size",
    descending: true,
    offset: 0,
    limit: 200,
    ...overrides,
  };
}

// ------------------------------------------------------------- drives ---

export type DriveKind = "fixed" | "removable" | "network" | "optical" | "ramDisk" | "unknown";

export const DRIVE_KIND_LABELS: Record<DriveKind, string> = {
  fixed: "Internal drive",
  removable: "Removable drive",
  network: "Network drive",
  optical: "Optical drive",
  ramDisk: "RAM disk",
  unknown: "Drive",
};

export interface Drive {
  /** The root path to scan, e.g. `C:\` or `/`. */
  path: string;
  label: string;
  kind: DriveKind;
  /** `null` when the drive could not be queried — no disc, unreachable share. */
  totalBytes: number | null;
  freeBytes: number | null;
  isSystem: boolean;
  /** False means a delete here would be immediate and permanent. */
  hasRecycleBin: boolean;
}

// ------------------------------------------------------------ treemap ---

export interface TreemapItem {
  name: string;
  path: string;
  /** Recursive for a folder; its own size for a file. */
  bytes: number;
  files: number;
  isDir: boolean;
  category: Category;
  /** Present for files only, so the map can act on a real target. */
  id: number | null;
  /** The aggregate row standing in for everything too small to draw. */
  isRemainder: boolean;
}

export interface TreemapNode {
  path: string;
  display: string;
  bytes: number;
  files: number;
  parent: string | null;
  /** Breadcrumb from the scan root down to here, inclusive. */
  trail: FolderRow[];
  items: TreemapItem[];
  remainderCount: number;
}

// -------------------------------------------------------- quick clean ---

/** One of the well-known junk locations. The list is fixed in Rust. */
export interface QuickCleanTarget {
  key: string;
  title: string;
  description: string;
  /** Empty for the Recycle Bin, which is not a plain folder. */
  path: string;
  needsAdmin: boolean;
  defaultOn: boolean;
  /** The downside of clearing this, when there is one. */
  caveat: string | null;
}

export interface QuickCleanMeasurement extends QuickCleanTarget {
  bytes: number;
  files: number;
  /** This machine does not have the folder at all. */
  missing: boolean;
  /** It exists but could not be read, so `bytes` is unknown, not zero. */
  unreadable: boolean;
}

export interface QuickCleanOutcome {
  key: string;
  /** Only what was really removed. */
  freedBytes: number;
  removedFiles: number;
  removedDirs: number;
  skippedInUse: number;
  skippedDenied: number;
  exampleError: string | null;
}
