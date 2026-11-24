import type { Route } from "@/app/nav";
import { Banner } from "@/components/Banner";
import { LifetimeCard } from "@/components/Reward";
import { IconCloud, IconDuplicates, IconFolder, IconSparkle, IconStorage } from "@/components/icons";
import { CATEGORY_LABELS, type Category, type QueryRequest } from "@/ipc/types";
import { CATEGORY_COLORS } from "@/lib/categories";
import { formatBytes, formatCount, formatDuration, formatRelative, plural } from "@/lib/format";
import { useScanStore } from "@/state/scanStore";

type Jump = (route: Route, preset?: Partial<QueryRequest>) => void;

const DAY = 86_400;

export function OverviewView({ onJump }: { onJump: Jump }) {
  const { status, summary, platform, scanErrors, startScan } = useScanStore();

  if (status === "scanning") return <ScanningHero />;

  if (!summary) {
    return (
      <div className="view">
        <div className="scan-hero">
          <span className="brand-mark" style={{ width: 38, height: 38, borderRadius: 10 }}>
            <IconSparkle size={19} />
          </span>
          <div className="scan-hero-title">Let's look at your Downloads folder</div>
          <p className="scan-hero-body">
            Cleanup reads file names, sizes and dates — nothing is opened, moved or deleted.
            Everything stays on this computer.
          </p>
          <button className="btn btn-primary btn-lg" onClick={() => void startScan()}>
            Scan {platform?.downloadsDir ? "Downloads" : "a folder"}
          </button>
          {platform?.downloadsDir ? (
            <span className="dim selectable" style={{ fontSize: 11.5 }}>
              {platform.downloadsDir}
            </span>
          ) : (
            <span className="dim" style={{ fontSize: 11.5 }}>
              Pick a folder in Settings to get started.
            </span>
          )}
        </div>
      </div>
    );
  }

  const staleBytes = summary.stale.bytes;
  const dupes = summary.duplicateCandidates;
  const topCategory = summary.byCategory.find((c) => c.count > 0);

  return (
    <div className="view">
      {summary.cancelled ? (
        <Banner
          tone="warn"
          title="This scan was cancelled"
          text="The numbers below cover only what had been read before you stopped it."
        />
      ) : null}

      {summary.truncated ? (
        <Banner
          tone="warn"
          title="Some folders were not fully explored"
          text="The scan hit its folder-depth limit, so these totals are a lower bound. You can raise the limit in Settings."
        />
      ) : null}

      {summary.errorCount > 0 ? (
        <Banner
          tone="warn"
          title={`${formatCount(summary.errorCount)} ${plural(summary.errorCount, "item")} could not be read`}
          text={
            scanErrors[0]
              ? `For example: ${scanErrors[0].path} — ${scanErrors[0].message}`
              : "These were skipped. The scan completed otherwise."
          }
          hint={
            platform?.privacyPrompts
              ? "On macOS this is usually a privacy permission. System Settings › Privacy & Security › Files and Folders."
              : null
          }
        />
      ) : null}

      {/* What you have already dealt with, before the list of what is left. */}
      <LifetimeCard />

      <div className="stat-grid">
        <Stat
          label="Files"
          value={formatCount(summary.totalFiles)}
          foot={`in ${formatCount(summary.totalDirs)} ${plural(summary.totalDirs, "folder")}`}
        />
        <Stat
          label="Total size"
          value={formatBytes(summary.totalBytes)}
          foot={topCategory ? `mostly ${CATEGORY_LABELS[topCategory.category].toLowerCase()}` : undefined}
        />
        <Stat
          label="Possible duplicates"
          value={formatCount(dupes.files)}
          foot={
            dupes.files > 0
              ? `up to ${formatBytes(dupes.reclaimableBytesUpperBound)} — needs checking`
              : "none found by size"
          }
        />
        <Stat
          label="Empty folders"
          value={formatCount(summary.emptyFolders.count)}
          foot={summary.emptyFolders.count > 0 ? "nothing useful inside" : "none"}
        />
      </div>

      <div className="grid-2">
        <div className="card">
          <div className="card-head">
            <h2>What's in there</h2>
            <span className="card-sub">by size</span>
          </div>
          <CategoryBar
            segments={summary.byCategory
              .filter((c) => c.bytes > 0)
              .map((c) => ({ category: c.category, bytes: c.bytes }))}
            total={summary.totalBytes}
          />
          <div className="legend">
            {summary.byCategory
              .filter((c) => c.count > 0)
              .map((stat) => (
                <button
                  key={stat.key}
                  className="legend-item"
                  onClick={() => onJump("downloads", { categories: [stat.category] })}
                  title={`Show ${CATEGORY_LABELS[stat.category]} in Downloads`}
                >
                  <span
                    className="swatch"
                    style={{ background: CATEGORY_COLORS[stat.category] }}
                  />
                  <span className="legend-name">{CATEGORY_LABELS[stat.category]}</span>
                  <span className="legend-value">{formatBytes(stat.bytes)}</span>
                </button>
              ))}
          </div>
        </div>

        <div className="card">
          <div className="card-head">
            <h2>Worth a look</h2>
            <span className="card-sub">with the reason, not a verdict</span>
          </div>
          <div className="signal-list">
            {dupes.files > 0 ? (
              <Signal
                icon={<IconDuplicates size={14} />}
                title={`${formatCount(dupes.files)} files share an exact size`}
                why={`Across ${formatCount(dupes.groups)} ${plural(dupes.groups, "group")}. Same size is a hint, not proof — comparing contents comes next.`}
                value={`≤ ${formatBytes(dupes.reclaimableBytesUpperBound)}`}
              />
            ) : null}

            {summary.largestFiles.length > 0 && summary.largestFiles[0] ? (
              <Signal
                icon={<IconStorage size={14} />}
                title="Your largest files"
                why={`The biggest is ${summary.largestFiles[0].name} at ${formatBytes(summary.largestFiles[0].size)}.`}
                value={formatBytes(
                  summary.largestFiles.reduce((sum, row) => sum + row.size, 0),
                )}
                onClick={() =>
                  onJump("downloads", { sort: "size", descending: true, minSize: 1024 ** 2 })
                }
              />
            ) : null}

            {summary.stale.count > 0 ? (
              <Signal
                icon={<IconFolder size={14} />}
                title={`${formatCount(summary.stale.count)} files untouched for ${summary.stale.days}+ days`}
                why={
                  summary.stale.fromAccessTime
                    ? "Based on when they were last opened. Age alone is not a reason to delete anything."
                    : "Based on when they were last changed — Windows does not track file opens. Age alone is not a reason to delete anything."
                }
                value={formatBytes(staleBytes)}
                onClick={() =>
                  onJump("downloads", {
                    modifiedBefore: Math.floor(Date.now() / 1000) - summary.stale.days * DAY,
                    sort: "size",
                    descending: true,
                  })
                }
              />
            ) : null}

            {summary.flagged.temporary > 0 ? (
              <Signal
                icon={<IconSparkle size={14} />}
                title={`${formatCount(summary.flagged.temporary)} temporary ${plural(summary.flagged.temporary, "file")}`}
                why="Backups, .tmp and leftover partial files. Still worth a glance before anything happens to them."
                value=""
                onClick={() => onJump("downloads", { categories: ["temporary"] })}
              />
            ) : null}

            {summary.flagged.cloudPlaceholders > 0 ? (
              <Signal
                icon={<IconCloud size={14} />}
                title={`${formatCount(summary.flagged.cloudPlaceholders)} files live in the cloud`}
                why="OneDrive or iCloud placeholders. Cleanup will not read or move them — doing so would download them in full."
                value=""
                warn
              />
            ) : null}

            {summary.flagged.inProgress > 0 ? (
              <Signal
                icon={<IconSparkle size={14} />}
                title={`${formatCount(summary.flagged.inProgress)} downloads still in progress`}
                why="Excluded from everything until they finish, because something is still writing to them."
                value=""
                warn
              />
            ) : null}

            {dupes.files === 0 &&
            summary.stale.count === 0 &&
            summary.flagged.temporary === 0 &&
            summary.emptyFolders.count === 0 ? (
              <div className="signal">
                <span className="signal-icon">
                  <IconSparkle size={14} />
                </span>
                <div className="signal-body">
                  <div className="signal-title">Nothing stands out</div>
                  <div className="signal-why">
                    No same-size files, no stale files, no leftovers. This folder is in good shape.
                  </div>
                </div>
              </div>
            ) : null}
          </div>
        </div>
      </div>

      <div className="resultbar" style={{ padding: 0 }}>
        <span className="dim">
          Scanned {formatRelative(summary.scannedAt)} in {formatDuration(summary.durationMs)}
          {summary.flagged.hidden > 0
            ? ` · ${formatCount(summary.flagged.hidden)} hidden items not shown`
            : ""}
        </span>
        <button className="btn btn-ghost" onClick={() => void startScan()}>
          Rescan
        </button>
      </div>
    </div>
  );
}

function ScanningHero() {
  const { progress, cancel } = useScanStore();
  return (
    <div className="view">
      <div className="scan-hero">
        <div className="scan-hero-title">Reading your folder…</div>
        <div className="scan-live">
          <div className="progress">
            {/* No percentage: discovering the total is the scan. */}
            <div className="progress-indeterminate" />
          </div>
          <div className="scan-live-counts">
            <span>
              <strong>{formatCount(progress?.files ?? 0)}</strong>{" "}
              <span className="muted">files</span>
            </span>
            <span>
              <strong>{formatCount(progress?.dirs ?? 0)}</strong>{" "}
              <span className="muted">folders</span>
            </span>
            <span>
              <strong>{formatBytes(progress?.bytes ?? 0)}</strong>
            </span>
          </div>
          <div className="scan-live-path">{progress?.current ?? ""}</div>
        </div>
        <button className="btn" onClick={() => void cancel()}>
          Stop <span className="kbd">Esc</span>
        </button>
      </div>
    </div>
  );
}

function Stat({ label, value, foot }: { label: string; value: string; foot?: string }) {
  return (
    <div className="stat">
      <div className="stat-label">{label}</div>
      <div className="stat-value">{value}</div>
      {foot ? <div className="stat-foot">{foot}</div> : null}
    </div>
  );
}

function CategoryBar({
  segments,
  total,
}: {
  segments: { category: Category; bytes: number }[];
  total: number;
}) {
  if (total === 0) return <div className="bar" />;
  return (
    <div className="bar">
      {segments.map((segment) => (
        <div
          key={segment.category}
          className="bar-seg"
          style={{
            width: `${Math.max((segment.bytes / total) * 100, 0.5)}%`,
            background: CATEGORY_COLORS[segment.category],
          }}
          title={`${CATEGORY_LABELS[segment.category]} — ${formatBytes(segment.bytes)}`}
        />
      ))}
    </div>
  );
}

function Signal({
  icon,
  title,
  why,
  value,
  onClick,
  warn,
}: {
  icon: JSX.Element;
  title: string;
  why: string;
  value: string;
  onClick?: () => void;
  warn?: boolean;
}) {
  const body = (
    <>
      <span className={`signal-icon ${warn ? "warn" : ""}`}>{icon}</span>
      <div className="signal-body">
        <div className="signal-title">{title}</div>
        <div className="signal-why">{why}</div>
      </div>
      {value ? <span className="signal-value">{value}</span> : null}
    </>
  );

  // Only rendered as a button when there is somewhere to go. A clickable-looking
  // row that does nothing is worse than a plain one.
  return onClick ? (
    <button className="signal actionable" onClick={onClick}>
      {body}
    </button>
  ) : (
    <div className="signal">{body}</div>
  );
}
