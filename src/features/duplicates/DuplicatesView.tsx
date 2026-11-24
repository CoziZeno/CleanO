import { useEffect, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { Banner } from "@/components/Banner";
import { IconDuplicates, IconReveal, IconSparkle } from "@/components/icons";
import { api } from "@/ipc/client";
import {
  DEDUPE_STAGE_LABELS,
  type DuplicateGroup,
  type DuplicateStats,
  type FileRow,
} from "@/ipc/types";
import { Checkbox } from "@/components/Checkbox";
import { Thumb } from "@/components/Thumb";
import { CATEGORY_COLORS } from "@/lib/categories";
import { useThumbStore } from "@/state/thumbStore";
import { formatBytes, formatCount, formatDuration, formatRelative, plural } from "@/lib/format";
import { useDupStore } from "@/state/dupStore";
import { useQueueStore } from "@/state/queueStore";
import { useScanStore } from "@/state/scanStore";

export function DuplicatesView() {
  const { summary, generation } = useScanStore();
  const { status, report, error, start, hydrate, reset, dismissError } = useDupStore();

  // Results index into one specific scan. A new scan invalidates them, and the
  // backend already refuses to serve them, so drop them here too.
  const [seenGeneration, setSeenGeneration] = useState(generation);
  useEffect(() => {
    if (generation !== seenGeneration) {
      setSeenGeneration(generation);
      reset();
    }
  }, [generation, seenGeneration, reset]);

  useEffect(() => {
    void hydrate();
  }, [hydrate]);

  if (!summary) {
    return (
      <div className="empty">
        <div className="empty-title">Nothing scanned yet</div>
        <p className="empty-body">
          Run a scan from the Overview tab first — duplicates are found by comparing the files it
          indexes.
        </p>
      </div>
    );
  }

  if (status === "running") {
    return <RunningState />;
  }

  if (status !== "ready" || !report) {
    const candidates = summary.duplicateCandidates;
    return (
      <div className="view">
        {error ? (
          <Banner
            tone="danger"
            title="The duplicate check could not finish"
            text={error.message}
            hint={error.hint}
            onDismiss={dismissError}
          />
        ) : null}

        <div className="scan-hero">
          <span className="signal-icon" style={{ width: 38, height: 38, borderRadius: 10 }}>
            <IconDuplicates size={19} />
          </span>
          <div className="scan-hero-title">
            {candidates.files > 0
              ? `${formatCount(candidates.files)} files share an exact size`
              : "No files share a size"}
          </div>
          <p className="scan-hero-body">
            {candidates.files > 0 ? (
              <>
                Same size is a hint, not proof. Comparing contents confirms which are genuinely
                identical — at most{" "}
                <strong>{formatBytes(candidates.reclaimableBytesUpperBound)}</strong> could turn
                out to be recoverable.
              </>
            ) : (
              <>
                Two files can only be identical if they are exactly the same size, and none here
                are. Nothing to compare.
              </>
            )}
          </p>
          {candidates.files > 0 ? (
            <>
              <button className="btn btn-primary btn-lg" onClick={() => void start()}>
                Compare file contents
              </button>
              <span className="dim" style={{ fontSize: 11.5 }}>
                Reads the first 64 KB of each candidate, then the whole file only where that is
                not enough to decide.
              </span>
            </>
          ) : null}
        </div>
      </div>
    );
  }

  const { stats, groups } = report;

  return (
    <div className="view">
      {stats.cancelled ? (
        <Banner
          tone="warn"
          title="The check was stopped early"
          text="Only the groups confirmed before you stopped are shown below."
        />
      ) : null}

      {stats.errorCount > 0 ? (
        <Banner
          tone="warn"
          title={`${formatCount(stats.errorCount)} ${plural(stats.errorCount, "file")} could not be read`}
          text={stats.errors[0] ?? undefined}
        />
      ) : null}

      <Summary stats={stats} />

      {groups.length === 0 ? (
        <div className="empty">
          <span className="signal-icon" style={{ width: 34, height: 34, borderRadius: 9 }}>
            <IconSparkle size={17} />
          </span>
          <div className="empty-title">No duplicates</div>
          <p className="empty-body">
            {formatCount(stats.considered)} files were considered.{" "}
            {stats.eliminatedBySize > 0
              ? `${formatCount(stats.eliminatedBySize)} were ruled out on size alone without being read, `
              : ""}
            and everything that was compared turned out to differ.
          </p>
        </div>
      ) : (
        <div className="signal-list">
          {groups.map((group) => (
            <GroupCard key={group.hash + group.size} group={group} />
          ))}
        </div>
      )}

      <div className="resultbar" style={{ padding: 0 }}>
        <span className="dim">
          Checked in {formatDuration(stats.durationMs)} · read{" "}
          {formatBytes(stats.bytesRead)}
          {stats.cacheHits > 0
            ? ` · ${formatCount(stats.cacheHits)} served from the cache`
            : ""}
          {stats.verifiedBytewise ? " · verified byte for byte" : ""}
        </span>
        <button className="btn btn-ghost" onClick={() => void start()}>
          Check again
        </button>
      </div>
    </div>
  );
}

function RunningState() {
  const { progress, cancel } = useDupStore();
  const stage = progress ? DEDUPE_STAGE_LABELS[progress.stage] : "Starting";
  const pct =
    progress && progress.filesTotal > 0
      ? Math.min(100, Math.round((progress.filesDone / progress.filesTotal) * 100))
      : null;

  return (
    <div className="view">
      <div className="scan-hero">
        <div className="scan-hero-title">{stage}…</div>
        <div className="scan-live">
          <div className="progress">
            {/* Unlike a scan, the totals here are known up front, so a real
                percentage is honest. */}
            {pct === null ? (
              <div className="progress-indeterminate" />
            ) : (
              <div className="progress-fill" style={{ width: `${pct}%` }} />
            )}
          </div>
          <div className="scan-live-counts">
            <span>
              <strong>{formatCount(progress?.filesDone ?? 0)}</strong>
              <span className="muted"> / {formatCount(progress?.filesTotal ?? 0)} files</span>
            </span>
            {progress && progress.bytesTotal > 0 ? (
              <span className="muted">
                {formatBytes(progress.bytesDone)} of {formatBytes(progress.bytesTotal)}
              </span>
            ) : null}
          </div>
        </div>
        <button className="btn" onClick={() => void cancel()}>
          Stop
        </button>
      </div>
    </div>
  );
}

function Summary({ stats }: { stats: DuplicateStats }) {
  return (
    <div className="stat-grid">
      <div className="stat">
        <div className="stat-label">Duplicate groups</div>
        <div className="stat-value">{formatCount(stats.groups)}</div>
        <div className="stat-foot">
          {formatCount(stats.duplicateFiles)} {plural(stats.duplicateFiles, "file")} in total
        </div>
      </div>
      <div className="stat">
        <div className="stat-label">Recoverable</div>
        <div className="stat-value">{formatBytes(stats.reclaimable)}</div>
        <div className="stat-foot">if one copy of each is kept</div>
      </div>
      <div className="stat">
        <div className="stat-label">Ruled out on size</div>
        <div className="stat-value">{formatCount(stats.eliminatedBySize)}</div>
        <div className="stat-foot">never opened at all</div>
      </div>
      <div className="stat">
        <div className="stat-label">Fully compared</div>
        <div className="stat-value">{formatCount(stats.fullHashed)}</div>
        <div className="stat-foot">
          {stats.collapsedHardlinks > 0
            ? `${formatCount(stats.collapsedHardlinks)} hardlinked, not counted`
            : `${formatCount(stats.headHashed)} partly read first`}
        </div>
      </div>
    </div>
  );
}

function GroupCard({ group }: { group: DuplicateGroup }) {
  const requestThumbs = useThumbStore((state) => state.request);
  // Seeing the images is the whole point here: choosing between identical
  // files by filename alone is exactly what people find hard.
  useEffect(() => {
    requestThumbs(group.files);
  }, [group.files, requestThumbs]);

  const selected = useQueueStore((state) => state.selected);
  const toggle = useQueueStore((state) => state.toggle);
  const add = useQueueStore((state) => state.add);
  const remove = useQueueStore((state) => state.remove);

  // Everything except the suggested keep. Selecting the keep as well is
  // possible but never offered in bulk - the point of the suggestion is that
  // one copy survives.
  const extras = group.files.filter((file) => file.id !== group.keepId).map((file) => file.id);
  const allExtrasPicked = extras.length > 0 && extras.every((id) => selected.has(id));

  const reveal = async (row: FileRow) => {
    try {
      await revealItemInDir(await api.resolvePath(row.id));
    } catch {
      // Convenience only.
    }
  };

  return (
    <div className="card group-card">
      <div className="group-head">
        <span
          className="cat-dot"
          style={{ background: CATEGORY_COLORS[group.files[0]?.category ?? "misc"] }}
        />
        <div className="group-head-body">
          <div className="signal-title">
            {group.files.length} identical copies · {formatBytes(group.size)} each
          </div>
          <div className="signal-why">{group.keepReason}</div>
        </div>
        <div className="group-head-value">
          <div className="signal-value">{formatBytes(group.reclaimable)}</div>
          <div className="dim" style={{ fontSize: 10.5 }}>
            recoverable
          </div>
        </div>
        <button
          className="btn"
          onClick={() => (allExtrasPicked ? remove(extras) : add(extras))}
          disabled={extras.length === 0}
        >
          {allExtrasPicked ? "Unpick copies" : "Pick extra copies"}
        </button>
      </div>

      <div className="dup-files">
        {group.files.map((file) => {
          const keep = file.id === group.keepId;
          return (
            <div className={`dup-file ${keep ? "keep" : ""}`} key={file.id}>
              <Checkbox
                checked={selected.has(file.id)}
                onChange={() => toggle(file.id)}
                label={`Select ${file.name} for cleanup`}
              />
              <span className={`keep-badge ${keep ? "on" : ""}`}>
                {keep ? "Suggested keep" : "Copy"}
              </span>
              <Thumb row={file} size={34} />
              <div className="file-meta">
                <span className="file-name selectable" title={file.path}>
                  {file.name}
                </span>
                <span className="file-sub selectable">{file.parent}</span>
              </div>
              <span className="dim" style={{ fontSize: 11 }}>
                {formatRelative(file.modified)}
              </span>
              <button
                className="btn-ghost"
                title="Show in file manager"
                aria-label={`Show ${file.name} in file manager`}
                onClick={() => void reveal(file)}
              >
                <IconReveal size={14} />
              </button>
            </div>
          );
        })}
      </div>

      <p className="group-note">
        Nothing is ticked for you. Whatever you pick collects in the Cleanup Queue, where you see
        every full path before anything happens — and files go to the Recycle Bin, never a
        permanent delete.
      </p>
    </div>
  );
}
