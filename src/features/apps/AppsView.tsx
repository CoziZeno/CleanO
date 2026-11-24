import { useCallback, useEffect, useMemo, useState } from "react";
import { Banner } from "@/components/Banner";
import { IconClose, IconRefresh, IconSearch, IconSparkle } from "@/components/icons";
import { api, toIpcError, type IpcError } from "@/ipc/client";
import type { InstalledApp, InstallerMatchRow } from "@/ipc/types";
import { formatBytes, formatCount, formatRelative, plural } from "@/lib/format";
import { useScanStore } from "@/state/scanStore";
import { useQueueStore } from "@/state/queueStore";
import { Checkbox } from "@/components/Checkbox";

type SortKey = "name" | "size" | "date";

export function AppsView() {
  const { generation, summary } = useScanStore();
  const [apps, setApps] = useState<InstalledApp[] | null>(null);
  const [redundant, setRedundant] = useState<InstallerMatchRow[]>([]);
  const [error, setError] = useState<IpcError | null>(null);
  const [busy, setBusy] = useState(false);
  const [text, setText] = useState("");
  const [sort, setSort] = useState<SortKey>("name");

  const load = useCallback(async (refresh = false) => {
    setBusy(true);
    try {
      setApps(await api.listInstalledApps(refresh));
      setError(null);
    } catch (err) {
      setError(toIpcError(err));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // Matching needs a scan to have something to match against.
  useEffect(() => {
    void api
      .matchInstallers()
      .then(setRedundant)
      .catch(() => setRedundant([]));
  }, [generation, apps]);

  const filtered = useMemo(() => {
    if (!apps) return [];
    const needle = text.trim().toLowerCase();
    const matched = needle
      ? apps.filter(
          (app) =>
            app.name.toLowerCase().includes(needle) ||
            (app.publisher ?? "").toLowerCase().includes(needle),
        )
      : apps;

    const sorted = [...matched];
    sorted.sort((a, b) => {
      if (sort === "size") return (b.estimatedBytes ?? 0) - (a.estimatedBytes ?? 0);
      if (sort === "date") return (b.installedAt ?? 0) - (a.installedAt ?? 0);
      return a.name.localeCompare(b.name);
    });
    return sorted;
  }, [apps, text, sort]);

  if (error) {
    return (
      <div className="view">
        <Banner
          tone="danger"
          title="Could not read installed software"
          text={error.message}
          hint={error.hint}
        />
      </div>
    );
  }

  if (!apps) {
    return (
      <div className="empty">
        <span className="spinner" />
      </div>
    );
  }

  return (
    <div className="view">
      {redundant.length > 0 ? <RedundantInstallers rows={redundant} /> : null}

      <div className="card">
        <div className="card-head">
          <h2>
            {formatCount(filtered.length)}
            {filtered.length !== apps.length ? ` of ${formatCount(apps.length)}` : ""}{" "}
            {plural(filtered.length, "app")}
          </h2>
          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <label className="search" style={{ height: 28, minWidth: 160 }}>
              <IconSearch size={13} />
              <input
                value={text}
                onChange={(event) => setText(event.target.value)}
                placeholder="Search apps…"
                spellCheck={false}
              />
              {text ? (
                <button className="btn-ghost" onClick={() => setText("")} aria-label="Clear">
                  <IconClose size={12} />
                </button>
              ) : null}
            </label>
            <select
              className="select"
              value={sort}
              onChange={(event) => setSort(event.target.value as SortKey)}
              aria-label="Sort apps"
            >
              <option value="name">Name</option>
              <option value="size">Size</option>
              <option value="date">Installed</option>
            </select>
            <button className="btn" onClick={() => void load(true)} disabled={busy}>
              <IconRefresh size={13} />
              {busy ? "Reading…" : "Refresh"}
            </button>
          </div>
        </div>

        <p className="setting-help" style={{ marginBottom: 10 }}>
          Read from the system's own records. Cleanup never uninstalls anything — use Windows
          Settings or Finder for that. Sizes are what each installer reported and are often
          missing or approximate, so they are not totalled.
        </p>

        {filtered.length === 0 ? (
          <p className="setting-help">Nothing matches that search.</p>
        ) : (
          <div className="plain-list">
            {filtered.map((app) => (
              <div className="plain-row" key={`${app.id}-${app.source}-${app.version ?? ""}`}>
                <div className="file-meta">
                  <span className="file-name selectable">
                    {app.name}
                    {app.version ? <span className="app-version"> {app.version}</span> : null}
                  </span>
                  <span className="file-sub">
                    {app.publisher ?? "Unknown publisher"}
                    {app.installedAt ? ` · installed ${formatRelative(app.installedAt)}` : ""}
                  </span>
                </div>
                <span className="tag">{app.source === "user" ? "This user" : "All users"}</span>
                <span className="legend-value" style={{ minWidth: 72, textAlign: "right" }}>
                  {app.estimatedBytes ? `~${formatBytes(app.estimatedBytes)}` : "—"}
                </span>
              </div>
            ))}
          </div>
        )}
      </div>

      {!summary ? (
        <p className="setting-help">
          Scan a folder and any installers in it will be checked against this list.
        </p>
      ) : null}
    </div>
  );
}

/**
 * The payoff: installers you no longer need, with the evidence.
 *
 * "This installer is old" is a guess. "You have 133 installed and this is 121"
 * is a reason, and it is the one case where an age-based suggestion becomes
 * something a person can act on confidently.
 */
function RedundantInstallers({ rows }: { rows: InstallerMatchRow[] }) {
  const selected = useQueueStore((state) => state.selected);
  const toggle = useQueueStore((state) => state.toggle);
  const add = useQueueStore((state) => state.add);

  const obsolete = rows.filter((row) => row.matched.verdict === "older");
  const sameVersion = rows.filter((row) => row.matched.verdict === "same");
  const worthShowing = [...obsolete, ...sameVersion];
  if (worthShowing.length === 0) return null;

  const total = worthShowing.reduce((sum, row) => sum + row.file.size, 0);

  return (
    <div className="card">
      <div className="card-head">
        <h2>Installers you already have installed</h2>
        <button
          className="btn"
          onClick={() => add(worthShowing.map((row) => row.file.id))}
        >
          Pick all {formatCount(worthShowing.length)} · {formatBytes(total)}
        </button>
      </div>

      <div className="signal-list">
        {worthShowing.map((row) => (
          <div className="signal" key={row.file.id}>
            <Checkbox
              checked={selected.has(row.file.id)}
              onChange={() => toggle(row.file.id)}
              label={`Select ${row.file.name}`}
            />
            <span className="signal-icon">
              <IconSparkle size={14} />
            </span>
            <div className="signal-body">
              <div className="signal-title selectable">{row.file.name}</div>
              <div className="signal-why">
                {row.matched.verdict === "older" ? (
                  <>
                    You have <strong>{row.matched.appName}</strong>{" "}
                    {row.matched.installedVersion} installed — this installer is{" "}
                    {row.matched.installerVersion}.
                  </>
                ) : (
                  <>
                    You have <strong>{row.matched.appName}</strong>{" "}
                    {row.matched.installedVersion} installed, the same version this installs.
                  </>
                )}
              </div>
            </div>
            <span className="signal-value">{formatBytes(row.file.size)}</span>
          </div>
        ))}
      </div>

      <p className="group-note">
        Matched by name, so check the pairing looks right before acting. Nothing is ticked for
        you, and these go to the Recycle Bin like anything else.
      </p>
    </div>
  );
}
