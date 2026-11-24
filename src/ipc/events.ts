/**
 * Typed wrappers over the Rust event stream.
 *
 * Progress carries absolute counts, not deltas, so a dropped event is harmless
 * and the UI never needs to reconcile. Every payload includes `scanId`, which
 * lets a listener discard ticks from a scan the user has already abandoned.
 */

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppErrorShape,
  DedupeProgress,
  DuplicateReport,
  ScanProgress,
  ScanSummary,
} from "./types";

export const SCAN_PROGRESS = "scan:progress";
export const SCAN_DONE = "scan:done";
export const SCAN_FAILED = "scan:failed";

export const DEDUPE_PROGRESS = "dedupe:progress";
export const DEDUPE_DONE = "dedupe:done";
export const DEDUPE_FAILED = "dedupe:failed";

export function onScanProgress(handler: (progress: ScanProgress) => void): Promise<UnlistenFn> {
  return listen<ScanProgress>(SCAN_PROGRESS, (event) => handler(event.payload));
}

export function onScanDone(handler: (summary: ScanSummary) => void): Promise<UnlistenFn> {
  return listen<ScanSummary>(SCAN_DONE, (event) => handler(event.payload));
}

export function onScanFailed(handler: (error: AppErrorShape) => void): Promise<UnlistenFn> {
  return listen<AppErrorShape>(SCAN_FAILED, (event) => handler(event.payload));
}

export function onDedupeProgress(
  handler: (progress: DedupeProgress) => void,
): Promise<UnlistenFn> {
  return listen<DedupeProgress>(DEDUPE_PROGRESS, (event) => handler(event.payload));
}

export function onDedupeDone(handler: (report: DuplicateReport) => void): Promise<UnlistenFn> {
  return listen<DuplicateReport>(DEDUPE_DONE, (event) => handler(event.payload));
}

export function onDedupeFailed(handler: (error: AppErrorShape) => void): Promise<UnlistenFn> {
  return listen<AppErrorShape>(DEDUPE_FAILED, (event) => handler(event.payload));
}
