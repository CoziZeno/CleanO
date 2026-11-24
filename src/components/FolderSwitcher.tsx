import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { IconFolder } from "./icons";
import { useScanStore } from "@/state/scanStore";
import { useSettingsStore } from "@/state/settingsStore";

/**
 * Switching which folder you are looking at is a top-level action, not a
 * setting. It lives in the toolbar, lists the folders already approved, and
 * can add a new one without a detour through Settings.
 */
export function FolderSwitcher() {
  const { summary, platform, status, startScan } = useScanStore();
  const settings = useSettingsStore((state) => state.settings);
  const load = useSettingsStore((state) => state.load);
  const addRoot = useSettingsStore((state) => state.addRoot);

  const [open_, setOpen] = useState(false);
  const wrapper = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!settings) void load();
  }, [settings, load]);

  // Close on an outside click or Escape, like any other menu.
  useEffect(() => {
    if (!open_) return;
    const onClick = (event: MouseEvent) => {
      if (!wrapper.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onClick);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onClick);
      document.removeEventListener("keydown", onKey);
    };
  }, [open_]);

  const roots = settings?.roots ?? [];
  const current = summary?.root ?? null;
  const busy = status === "scanning";

  const pick = async () => {
    setOpen(false);
    const picked = await open({
      directory: true,
      multiple: false,
      title: "Choose a folder to scan",
      defaultPath: platform?.homeDir ?? undefined,
    });
    if (typeof picked !== "string") return;
    await addRoot(picked);
    await startScan(picked);
  };

  const label = summary?.rootDisplay ?? "Choose a folder";

  return (
    <div className="switcher" ref={wrapper}>
      <button
        className="switcher-trigger"
        onClick={() => setOpen((value) => !value)}
        title={current ?? "Choose a folder to scan"}
        aria-haspopup="menu"
        aria-expanded={open_}
      >
        <IconFolder size={14} />
        <span className="switcher-label">{label}</span>
        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <path
            d="m6 9 6 6 6-6"
            stroke="currentColor"
            strokeWidth="2.4"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      </button>

      {open_ ? (
        <div className="switcher-menu" role="menu">
          {roots.length > 0 ? (
            <>
              <div className="switcher-heading">Approved folders</div>
              {roots.map((root) => (
                <button
                  key={root}
                  className={`switcher-item ${root === current ? "active" : ""}`}
                  disabled={busy}
                  onClick={() => {
                    setOpen(false);
                    void startScan(root);
                  }}
                  title={root}
                >
                  <IconFolder size={13} />
                  <span className="switcher-item-path">{root}</span>
                </button>
              ))}
              <div className="switcher-divider" />
            </>
          ) : null}

          {platform?.downloadsDir && !roots.includes(platform.downloadsDir) ? (
            <button
              className="switcher-item"
              disabled={busy}
              onClick={() => {
                setOpen(false);
                void startScan(platform.downloadsDir);
              }}
            >
              <IconFolder size={13} />
              <span className="switcher-item-path">Downloads</span>
            </button>
          ) : null}

          <button className="switcher-item" onClick={() => void pick()} disabled={busy}>
            <IconFolder size={13} />
            <span className="switcher-item-path">Choose another folder…</span>
          </button>
        </div>
      ) : null}
    </div>
  );
}
