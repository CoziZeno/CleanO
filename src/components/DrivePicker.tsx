import { useCallback, useEffect, useState } from "react";
import { api, toIpcError, type IpcError } from "@/ipc/client";
import { DRIVE_KIND_LABELS, type Drive } from "@/ipc/types";
import { formatBytes } from "@/lib/format";
import { useScanStore } from "@/state/scanStore";
import { useSettingsStore } from "@/state/settingsStore";
import { Banner } from "./Banner";
import { IconStorage } from "./icons";

/**
 * The drives on this machine, with a way to scan one.
 *
 * Scanning a whole drive is the one root the safety guard refuses by default,
 * because "scan my whole computer" must never start from a stray click in a
 * folder picker. Here it is deliberate: the drive is named, its size is shown,
 * and the confirmation says what it means before anything happens. The walk
 * still refuses to descend into system and application folders, so this reads
 * the user's own files, not the operating system.
 */
export function DrivePicker({ onScanStarted }: { onScanStarted?: () => void }) {
  const [drives, setDrives] = useState<Drive[] | null>(null);
  const [error, setError] = useState<IpcError | null>(null);
  const [confirming, setConfirming] = useState<Drive | null>(null);
  const [busy, setBusy] = useState(false);
  const { startScan, status } = useScanStore();
  const reloadSettings = useSettingsStore((state) => state.load);

  const load = useCallback(async () => {
    try {
      setDrives(await api.listDrives());
      setError(null);
    } catch (err) {
      setError(toIpcError(err));
      setDrives([]);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const scan = async (drive: Drive) => {
    setBusy(true);
    try {
      // Approve first, then scan. If approval fails the scan never starts,
      // rather than running against a root the guard would later refuse.
      await api.addDriveRoot(drive.path);
      await reloadSettings();
      setConfirming(null);
      await startScan(drive.path);
      onScanStarted?.();
    } catch (err) {
      setError(toIpcError(err));
      setConfirming(null);
    } finally {
      setBusy(false);
    }
  };

  if (!drives) {
    return (
      <div className="empty" style={{ padding: 24 }}>
        <span className="spinner" />
      </div>
    );
  }

  return (
    <>
      {error ? (
        <Banner
          tone="danger"
          title="That drive cannot be used"
          text={error.message}
          hint={error.hint}
          onDismiss={() => setError(null)}
        />
      ) : null}

      <div className="drive-list">
        {drives.length === 0 ? (
          <div className="root-item dim">No drives were found.</div>
        ) : (
          drives.map((drive) => {
            const used =
              drive.totalBytes !== null && drive.freeBytes !== null
                ? drive.totalBytes - drive.freeBytes
                : null;
            const percent =
              used !== null && drive.totalBytes
                ? Math.min(100, Math.round((used / drive.totalBytes) * 100))
                : null;
            return (
              <div className="drive" key={drive.path}>
                <span className="drive-icon">
                  <IconStorage size={15} />
                </span>

                <div className="drive-body">
                  <div className="drive-head">
                    <span className="drive-label">{drive.label}</span>
                    <span className="drive-kind muted">
                      {DRIVE_KIND_LABELS[drive.kind]}
                      {drive.isSystem ? " · system" : ""}
                    </span>
                  </div>

                  {percent !== null && used !== null && drive.totalBytes !== null ? (
                    <>
                      <div className="drive-bar">
                        <span
                          className={`drive-bar-fill ${percent >= 90 ? "full" : ""}`}
                          style={{ width: `${percent}%` }}
                        />
                      </div>
                      <div className="drive-sub muted">
                        {formatBytes(used)} of {formatBytes(drive.totalBytes)} used ·{" "}
                        {formatBytes(drive.totalBytes - used)} free
                      </div>
                    </>
                  ) : (
                    // Not "0 bytes": the drive could not be queried, which is
                    // a different thing and should read as one.
                    <div className="drive-sub muted">
                      Size unavailable — the drive may be empty or unreachable.
                    </div>
                  )}

                  {!drive.hasRecycleBin ? (
                    <div className="drive-sub warn">
                      No Recycle Bin on this drive: deleting here is permanent.
                    </div>
                  ) : null}
                </div>

                <button
                  className="btn"
                  disabled={status === "scanning" || busy || drive.totalBytes === null}
                  onClick={() => setConfirming(drive)}
                >
                  Scan
                </button>
              </div>
            );
          })
        )}
      </div>

      {confirming ? (
        <div className="drive-confirm">
          <div className="drive-confirm-title">Scan all of {confirming.label}?</div>
          <p className="setting-help">
            This indexes every folder on <code className="selectable">{confirming.path}</code> that
            you can read, which on a large drive can take several minutes. System and application
            folders are skipped, and nothing is moved or deleted — scanning only reads file names
            and sizes.
          </p>
          <p className="setting-help">
            It also adds this drive to the folders Cleanup may act in. You can withdraw that at any
            time, or exclude individual folders below.
          </p>
          <div className="drive-confirm-actions">
            <button className="btn-primary" disabled={busy} onClick={() => void scan(confirming)}>
              {busy ? "Starting…" : `Scan ${confirming.path}`}
            </button>
            <button className="btn" disabled={busy} onClick={() => setConfirming(null)}>
              Cancel
            </button>
          </div>
        </div>
      ) : null}
    </>
  );
}
