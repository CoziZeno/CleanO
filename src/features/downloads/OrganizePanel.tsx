import { useCallback, useEffect, useState } from "react";
import { IconFolder, IconSparkle } from "@/components/icons";
import { api, toIpcError } from "@/ipc/client";
import {
  CATEGORY_LABELS,
  ORGANIZE_MODES,
  type OrganizeMode,
  type OrganizePlan,
} from "@/ipc/types";
import { CATEGORY_COLORS } from "@/lib/categories";
import { formatBytes, formatCount, plural } from "@/lib/format";
import { useQueueStore } from "@/state/queueStore";
import { useScanStore } from "@/state/scanStore";

/**
 * Preview of how loose files would be sorted into folders.
 *
 * Read-only on purpose. The destinations shown are real and fully resolved —
 * including the renames a name collision would force — so the user can see
 * exactly what applying this would do. Applying it, with the journal and undo
 * that make it safe, is Phase 4; until then there is no button here that
 * pretends otherwise.
 */
export function OrganizePanel() {
  const { generation, startScan } = useScanStore();
  const applyOrganize = useQueueStore((state) => state.applyOrganize);
  const busy = useQueueStore((state) => state.busy);
  const [plan, setPlan] = useState<OrganizePlan | null>(null);
  const [open, setOpen] = useState(false);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [mode, setMode] = useState<OrganizeMode>("type");

  const load = useCallback(async () => {
    try {
      setPlan(await api.getOrganizePlan(mode));
    } catch (err) {
      // A missing scan is the normal starting state; nothing to report.
      if (toIpcError(err).code !== "noScan") setPlan(null);
    }
  }, [mode]);

  useEffect(() => {
    void load();
  }, [load, generation]);

  if (!plan || plan.totalFiles === 0) return null;

  return (
    <div className="plan-panel">
      <button className="plan-head" onClick={() => setOpen((value) => !value)}>
        <span className="signal-icon">
          <IconSparkle size={14} />
        </span>
        <div className="signal-body">
          <div className="signal-title">
            {formatCount(plan.totalFiles)} loose {plural(plan.totalFiles, "file")} could be sorted
            into {plan.groups.length} {plural(plan.groups.length, "folder")}
          </div>
          <div className="signal-why">
            {formatBytes(plan.totalBytes)} in total
            {plan.collisions > 0
              ? ` · ${formatCount(plan.collisions)} would be renamed to avoid overwriting something`
              : ""}
            {plan.skipped > 0 ? ` · ${formatCount(plan.skipped)} left alone` : ""}
          </div>
        </div>
        <span className="dim" style={{ fontSize: 11.5 }}>
          {open ? "Hide" : "Preview"}
        </span>
      </button>

      {open ? (
        <div className="plan-body">
          <div className="plan-modes">
            <span className="dim" style={{ fontSize: 11.5 }}>
              Sort
            </span>
            <div className="segmented">
              {ORGANIZE_MODES.map((option) => (
                <button
                  key={option.mode}
                  className={`seg ${mode === option.mode ? "active" : ""}`}
                  title={option.help}
                  onClick={() => {
                    setMode(option.mode);
                    setExpanded(null);
                    setConfirming(false);
                  }}
                >
                  {option.label}
                </button>
              ))}
            </div>
          </div>

          {plan.groups.map((group) => {
            const isOpen = expanded === group.destination;
            return (
              <div className="plan-group" key={group.destination}>
                <button
                  className="plan-group-head"
                  onClick={() => setExpanded(isOpen ? null : group.destination)}
                >
                  <span
                    className="cat-dot"
                    style={{ background: CATEGORY_COLORS[group.category] }}
                  />
                  <IconFolder size={13} />
                  <span className="plan-dest">{group.destination}/</span>
                  <span className="dim" style={{ fontSize: 11.5 }}>
                    {CATEGORY_LABELS[group.category]}
                  </span>
                  <span className="plan-spacer" />
                  <span className="legend-value">
                    {formatCount(group.files)} {plural(group.files, "file")} ·{" "}
                    {formatBytes(group.bytes)}
                  </span>
                </button>

                {isOpen ? (
                  <div className="plan-items">
                    {group.items.map((item) => (
                      <div className="plan-item" key={item.id}>
                        <span className="file-name selectable" title={item.fromPath}>
                          {item.name}
                        </span>
                        <span className="plan-arrow">→</span>
                        <span className="plan-target selectable" title={item.toPath}>
                          {group.destination}/{item.resolvedName ?? item.name}
                        </span>
                        {item.collides ? (
                          <span className="tag tag-warn">renamed, nothing overwritten</span>
                        ) : null}
                      </div>
                    ))}
                  </div>
                ) : null}
              </div>
            );
          })}

          <div className="plan-apply">
            <p className="plan-note" style={{ margin: 0, border: "none", padding: 0 }}>
              Nothing has moved yet. Every move is written to the journal first and can be undone
              from the Cleanup Queue, and nothing is ever overwritten.
            </p>
            {confirming ? (
              <>
                <button className="btn" onClick={() => setConfirming(false)} disabled={busy}>
                  Cancel
                </button>
                <button
                  className="btn btn-primary"
                  disabled={busy}
                  onClick={async () => {
                    setConfirming(false);
                    await applyOrganize(undefined, mode);
                    // The index now describes a folder that has changed, so
                    // rescan rather than showing stale rows.
                    await startScan();
                  }}
                >
                  {busy ? "Moving…" : `Yes, move ${formatCount(plan.totalFiles)}`}
                </button>
              </>
            ) : (
              <button
                className="btn btn-primary"
                onClick={() => setConfirming(true)}
                disabled={busy}
              >
                Sort {formatCount(plan.totalFiles)} {plural(plan.totalFiles, "file")}
              </button>
            )}
          </div>
        </div>
      ) : null}
    </div>
  );
}
