import { useCallback, useEffect, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { Banner } from "@/components/Banner";
import { DrivePicker } from "@/components/DrivePicker";
import { IconFolder, IconReveal } from "@/components/icons";
import { Treemap } from "./Treemap";
import { api, type IpcError, toIpcError } from "@/ipc/client";
import { CATEGORY_LABELS, type FileRow, type FolderRow, type StorageReport } from "@/ipc/types";
import { CATEGORY_COLORS } from "@/lib/categories";
import { formatBytes, formatCount, formatRelative, plural } from "@/lib/format";
import { useScanStore } from "@/state/scanStore";

type ListTab = "largest" | "stale" | "temporary";

export function StorageView({
  onContextMenu,
}: {
  onContextMenu: (x: number, y: number, row: FileRow) => void;
}) {
  const { summary, generation, platform } = useScanStore();
  const [report, setReport] = useState<StorageReport | null>(null);
  const [error, setError] = useState<IpcError | null>(null);
  const [tab, setTab] = useState<ListTab>("largest");

  const load = useCallback(async () => {
    try {
      setReport(await api.getStorageReport());
      setError(null);
    } catch (err) {
      const ipc = toIpcError(err);
      // No scan yet is the normal starting state, not a failure.
      setError(ipc.code === "noScan" ? null : ipc);
      setReport(null);
    }
  }, []);

  // Recomputed from the in-memory index, so this costs no disk I/O.
  useEffect(() => {
    void load();
  }, [load, generation]);

  if (!summary) {
    return (
      <div className="view">
        <div className="empty" style={{ paddingBottom: 8 }}>
          <div className="empty-title">Nothing scanned yet</div>
          <p className="empty-body">
            Scan a folder from the Overview tab, or pick a whole drive below to see where its space
            has gone.
          </p>
        </div>

        <div className="card">
          <div className="card-head">
            <h2>Drives on this computer</h2>
            <span className="card-sub">Pick one to map</span>
          </div>
          <DrivePicker />
        </div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="view">
        <Banner
          tone="danger"
          title="Could not build the storage report"
          text={error.message}
          hint={error.hint}
        />
      </div>
    );
  }

  if (!report) {
    return (
      <div className="empty">
        <span className="spinner" />
      </div>
    );
  }

  const lists: Record<ListTab, { rows: FileRow[]; empty: string }> = {
    largest: { rows: report.largestFiles, empty: "No files were indexed." },
    stale: {
      rows: report.staleFiles,
      empty: `Nothing has gone untouched for ${report.stale.days} days.`,
    },
    temporary: { rows: report.temporaryFiles, empty: "No temporary or leftover files." },
  };
  const active = lists[tab];
  const biggestFolder = report.largestFolders[0]?.bytes ?? 0;

  return (
    <div className="view">
      {report.truncated ? (
        <Banner
          tone="warn"
          title="These totals are a lower bound"
          text="The scan stopped at its folder-depth limit, so some folders were not fully measured. You can raise the limit in Settings."
        />
      ) : null}

      <div className="stat-grid">
        <div className="stat">
          <div className="stat-label">Total</div>
          <div className="stat-value">{formatBytes(report.totalBytes)}</div>
          <div className="stat-foot">
            {formatCount(report.totalFiles)} {plural(report.totalFiles, "file")}
          </div>
        </div>
        <div className="stat">
          <div className="stat-label">Untouched {report.stale.days}+ days</div>
          <div className="stat-value">{formatBytes(report.stale.bytes)}</div>
          <div className="stat-foot">
            {report.stale.fromAccessTime ? "since last opened" : "since last changed"}
          </div>
        </div>
        <div className="stat">
          <div className="stat-label">Temporary files</div>
          <div className="stat-value">
            {formatBytes(report.temporaryFiles.reduce((sum, row) => sum + row.size, 0))}
          </div>
          <div className="stat-foot">
            {formatCount(report.temporaryFiles.length)} shown
          </div>
        </div>
        <div className="stat">
          <div className="stat-label">Empty folders</div>
          <div className="stat-value">{formatCount(report.emptyFolders.count)}</div>
          <div className="stat-foot">nothing useful inside</div>
        </div>
      </div>

      <div className="card">
        <div className="card-head">
          <h2>Storage map</h2>
          <span className="card-sub">Every rectangle sized by what it uses</span>
        </div>
        <Treemap generation={generation} onContextMenu={onContextMenu} />
      </div>

      <div className="card">
        <div className="card-head">
          <h2>Largest folders</h2>
          <span className="card-sub">including everything inside them</span>
        </div>
        {report.largestFolders.length === 0 ? (
          <p className="setting-help">Every file is loose in the scanned folder — no subfolders.</p>
        ) : (
          <div className="folder-list">
            {report.largestFolders.map((folder) => (
              <FolderBar key={folder.path} folder={folder} max={biggestFolder} />
            ))}
          </div>
        )}
      </div>

      <div className="card">
        <div className="card-head">
          <h2>Files worth a look</h2>
          <div className="segmented">
            <SegButton label="Largest" value="largest" tab={tab} onPick={setTab} />
            <SegButton label="Untouched" value="stale" tab={tab} onPick={setTab} />
            <SegButton label="Temporary" value="temporary" tab={tab} onPick={setTab} />
          </div>
        </div>

        {tab === "stale" && !report.stale.fromAccessTime ? (
          <p className="setting-help" style={{ marginBottom: 10 }}>
            {platform?.os === "windows"
              ? "Windows does not record when files are opened, so this is measured from when they last changed."
              : "Measured from when these files last changed."}{" "}
            Age on its own is never a reason to delete something.
          </p>
        ) : null}

        {active.rows.length === 0 ? (
          <p className="setting-help">{active.empty}</p>
        ) : (
          <div className="plain-list">
            {active.rows.map((row) => (
              <FileLine key={row.id} row={row} />
            ))}
          </div>
        )}
      </div>

      <div className="card">
        <div className="card-head">
          <h2>By type</h2>
          <span className="card-sub">count and size</span>
        </div>
        <div className="legend">
          {report.byCategory
            .filter((stat) => stat.count > 0)
            .map((stat) => (
              <div className="legend-item" key={stat.key} style={{ cursor: "default" }}>
                <span className="swatch" style={{ background: CATEGORY_COLORS[stat.category] }} />
                <span className="legend-name">{CATEGORY_LABELS[stat.category]}</span>
                <span className="legend-value">
                  {formatCount(stat.count)} · {formatBytes(stat.bytes)}
                </span>
              </div>
            ))}
        </div>
      </div>

      {report.emptyFolders.count > 0 ? (
        <div className="card">
          <div className="card-head">
            <h2>Empty folders</h2>
            <span className="card-sub">
              {formatCount(report.emptyFolders.count)} found
            </span>
          </div>
          <div className="plain-list">
            {report.emptyFolders.sample.map((path) => (
              <div className="plain-row" key={path}>
                <IconFolder size={14} />
                <span className="file-name selectable" title={path}>
                  {path}
                </span>
              </div>
            ))}
          </div>
          {report.emptyFolders.count > report.emptyFolders.sample.length ? (
            <p className="setting-help" style={{ marginTop: 8 }}>
              and {formatCount(report.emptyFolders.count - report.emptyFolders.sample.length)} more.
            </p>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function SegButton({
  label,
  value,
  tab,
  onPick,
}: {
  label: string;
  value: ListTab;
  tab: ListTab;
  onPick: (value: ListTab) => void;
}) {
  return (
    <button
      className={`seg ${tab === value ? "active" : ""}`}
      onClick={() => onPick(value)}
      aria-pressed={tab === value}
    >
      {label}
    </button>
  );
}

function FolderBar({ folder, max }: { folder: FolderRow; max: number }) {
  const width = max > 0 ? Math.max((folder.bytes / max) * 100, 1.5) : 0;
  return (
    <div className="folder-row">
      <div className="folder-row-top">
        <span className="file-name selectable" title={folder.path}>
          {folder.display}
        </span>
        <span className="legend-value">{formatBytes(folder.bytes)}</span>
      </div>
      <div className="folder-track">
        <div className="folder-fill" style={{ width: `${width}%` }} />
      </div>
      <span className="dim" style={{ fontSize: 10.5 }}>
        {formatCount(folder.files)} {plural(folder.files, "file")}
      </span>
    </div>
  );
}

function FileLine({ row }: { row: FileRow }) {
  const reveal = async () => {
    try {
      await revealItemInDir(await api.resolvePath(row.id));
    } catch {
      // Convenience only.
    }
  };
  return (
    <div className="plain-row">
      <span className="cat-dot" style={{ background: CATEGORY_COLORS[row.category] }} />
      <div className="file-meta">
        <span className="file-name selectable" title={row.path}>
          {row.name}
        </span>
        <span className="file-sub">{row.parent}</span>
      </div>
      <span className="dim" style={{ fontSize: 11 }}>
        {formatRelative(row.modified)}
      </span>
      <span className="legend-value" style={{ minWidth: 72, textAlign: "right" }}>
        {formatBytes(row.size)}
      </span>
      <button
        className="btn-ghost"
        title="Show in file manager"
        aria-label={`Show ${row.name} in file manager`}
        onClick={() => void reveal()}
      >
        <IconReveal size={14} />
      </button>
    </div>
  );
}
