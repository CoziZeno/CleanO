/** Display formatting. No dependencies; `Intl` covers everything needed. */

const UNITS = ["B", "KB", "MB", "GB", "TB", "PB"] as const;

/**
 * Binary units with decimal-ish labels, matching what Windows Explorer and
 * Finder's "Get Info" show, so the numbers agree with the rest of the OS.
 */
export function formatBytes(bytes: number, precision?: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes === 0) return "0 B";
  const exponent = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), UNITS.length - 1);
  const value = bytes / 1024 ** exponent;
  const unit = UNITS[exponent] ?? "B";
  const digits = precision ?? (exponent === 0 ? 0 : value < 10 ? 1 : value < 100 ? 1 : 0);
  return `${value.toFixed(digits)} ${unit}`;
}

const numberFormat = new Intl.NumberFormat();

export function formatCount(value: number): string {
  return numberFormat.format(value);
}

export function plural(count: number, singular: string, pluralForm?: string): string {
  return count === 1 ? singular : (pluralForm ?? `${singular}s`);
}

const DAY = 86_400;

/**
 * Relative age, coarse on purpose: "3 months ago" is more useful at a glance
 * than a timestamp, and a null is shown as "unknown" rather than 1970.
 */
export function formatRelative(unixSeconds: number | null): string {
  if (unixSeconds === null || !Number.isFinite(unixSeconds)) return "unknown";
  const now = Date.now() / 1000;
  const seconds = now - unixSeconds;
  if (seconds < 0) return "just now";
  if (seconds < 60) return "just now";
  if (seconds < 3600) {
    const minutes = Math.floor(seconds / 60);
    return `${minutes} ${plural(minutes, "minute")} ago`;
  }
  if (seconds < DAY) {
    const hours = Math.floor(seconds / 3600);
    return `${hours} ${plural(hours, "hour")} ago`;
  }
  const days = Math.floor(seconds / DAY);
  if (days === 1) return "yesterday";
  if (days < 30) return `${days} days ago`;
  const months = Math.floor(days / 30);
  if (months < 12) return `${months} ${plural(months, "month")} ago`;
  const years = Math.floor(days / 365);
  return `${years} ${plural(years, "year")} ago`;
}

const dateFormat = new Intl.DateTimeFormat(undefined, {
  year: "numeric",
  month: "short",
  day: "numeric",
  hour: "2-digit",
  minute: "2-digit",
});

export function formatAbsolute(unixSeconds: number | null): string {
  if (unixSeconds === null || !Number.isFinite(unixSeconds)) return "Unknown";
  return dateFormat.format(new Date(unixSeconds * 1000));
}

export function formatDuration(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)} ms`;
  const seconds = ms / 1000;
  if (seconds < 60) return `${seconds.toFixed(seconds < 10 ? 1 : 0)} s`;
  const minutes = Math.floor(seconds / 60);
  const rest = Math.round(seconds % 60);
  return `${minutes}m ${rest}s`;
}

/** Trim a long path from the left, keeping the end, which is the useful part. */
export function truncatePath(path: string, max = 64): string {
  if (path.length <= max) return path;
  return `…${path.slice(path.length - max + 1)}`;
}

export const SIZE_PRESETS = [
  { label: "Any size", min: null as number | null },
  { label: "> 1 MB", min: 1024 ** 2 },
  { label: "> 10 MB", min: 10 * 1024 ** 2 },
  { label: "> 100 MB", min: 100 * 1024 ** 2 },
  { label: "> 1 GB", min: 1024 ** 3 },
];

export const AGE_PRESETS = [
  { label: "Any age", days: null as number | null },
  { label: "Last 7 days", days: 7 },
  { label: "Last 30 days", days: 30 },
  { label: "Older than 6 months", days: -180 },
  { label: "Older than a year", days: -365 },
];
