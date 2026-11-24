import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { NAV_ITEMS, type Route } from "@/app/nav";
import { useShortcuts } from "@/app/shortcuts";
import { Banner } from "@/components/Banner";
import { ContextMenu, type MenuState } from "@/components/ContextMenu";
import { FolderSwitcher } from "@/components/FolderSwitcher";
import { IconRefresh, IconSearch } from "@/components/icons";
import { PromptDialog } from "@/components/PromptDialog";
import { SelectionBar } from "@/components/SelectionBar";
import { ShortcutsOverlay } from "@/components/ShortcutsOverlay";
import { Sidebar } from "@/components/Sidebar";
import { AppsView } from "@/features/apps/AppsView";
import { DownloadsView } from "@/features/downloads/DownloadsView";
import { DuplicatesView } from "@/features/duplicates/DuplicatesView";
import { QuickFind } from "@/features/quickfind/QuickFind";
import { QueueView, RecoveryBanner } from "@/features/queue/QueueView";
import { SessionsView } from "@/features/sessions/SessionsView";
import { SettingsView } from "@/features/settings/SettingsView";
import { StorageView } from "@/features/storage/StorageView";
import { SwipeView } from "@/features/swipe/SwipeView";
import type { FileRow, QueryRequest } from "@/ipc/types";
import { formatCount } from "@/lib/format";
import { useFileOpsStore } from "@/state/fileOpsStore";
import { useQueueStore } from "@/state/queueStore";
import { useScanStore } from "@/state/scanStore";
import { useSettingsStore } from "@/state/settingsStore";
import { useThumbStore } from "@/state/thumbStore";

export default function App() {
  const [route, setRoute] = useState<Route>("sessions");
  const [preset, setPreset] = useState<Partial<QueryRequest> | null>(null);
  const [presetNonce, setPresetNonce] = useState(0);
  const [helpOpen, setHelpOpen] = useState(false);
  const [findOpen, setFindOpen] = useState(false);
  const [menu, setMenu] = useState<MenuState | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  /** The row most recently right-clicked or hovered, for keyboard actions. */
  const focusedRow = useRef<FileRow | null>(null);

  const {
    ready,
    status,
    summary,
    error,
    warning,
    generation,
    init,
    startScan,
    cancel,
    dismissError,
    dismissWarning,
  } = useScanStore();
  const settings = useSettingsStore((state) => state.settings);
  const loadSettings = useSettingsStore((state) => state.load);
  const operations = useQueueStore((state) => state.operations);
  const loadOperations = useQueueStore((state) => state.loadOperations);
  const undo = useQueueStore((state) => state.undo);
  const selected = useQueueStore((state) => state.selected);
  const ops = useFileOpsStore();

  useEffect(() => {
    void init();
    void loadSettings();
    void loadOperations();
  }, [init, loadSettings, loadOperations]);

  useEffect(() => {
    useThumbStore.getState().clear();
  }, [generation]);

  useEffect(() => {
    document.documentElement.dataset["theme"] = settings?.theme ?? "dark";
  }, [settings?.theme]);

  // A transient confirmation for file operations, cleared on its own.
  useEffect(() => {
    if (!ops.notice) return;
    const timer = setTimeout(() => ops.dismissNotice(), 2600);
    return () => clearTimeout(timer);
  }, [ops.notice, ops]);

  const jump = useCallback((next: Route, filter?: Partial<QueryRequest>) => {
    if (filter) {
      setPreset(filter);
      setPresetNonce((value) => value + 1);
    }
    setRoute(next);
  }, []);

  const openMenu = useCallback((x: number, y: number, row: FileRow) => {
    focusedRow.current = row;
    // Imported lazily to keep the menu definition beside the file actions.
    void import("@/lib/fileMenu").then(({ buildFileMenu }) => {
      setMenu({ x, y, items: buildFileMenu(row, useQueueStore.getState().selected) });
    });
  }, []);

  const handlers = useMemo(
    () => ({
      onNavigate: setRoute,
      onFocusSearch: () => {
        setRoute("downloads");
        requestAnimationFrame(() => searchRef.current?.focus());
      },
      onEscape: () => {
        if (findOpen) setFindOpen(false);
        else if (menu) setMenu(null);
        else if (helpOpen) setHelpOpen(false);
        else if (ops.prompt) ops.cancelPrompt();
        else if (status === "scanning") void cancel();
        else if (document.activeElement === searchRef.current) searchRef.current?.blur();
      },
      onRescan: () => {
        if (status !== "scanning") void startScan();
      },
      onToggleHelp: () => setHelpOpen((open) => !open),
      onQuickFind: () => setFindOpen(true),
      onUndoLast: () => {
        const latest = operations.find((operation) => operation.undoable);
        if (latest) {
          setRoute("queue");
          void undo(latest.id);
        }
      },
      onSelectAll: () => {
        // Handled inside Files, which knows the current filter.
        window.dispatchEvent(new CustomEvent("cleanup:select-all"));
      },
      onTrashSelection: () => {
        const ids = Array.from(useQueueStore.getState().selected);
        if (ids.length > 0) void ops.trash(ids);
        else if (focusedRow.current) void ops.trash([focusedRow.current.id]);
      },
      onRename: () => {
        if (focusedRow.current) ops.startRename(focusedRow.current);
      },
      onOpen: () => {
        if (focusedRow.current) void ops.openFile(focusedRow.current);
      },
      onCopyPath: () => {
        if (focusedRow.current) void ops.copyPath(focusedRow.current);
      },
    }),
    [findOpen, menu, helpOpen, status, cancel, startScan, operations, undo, ops],
  );

  useShortcuts(handlers);

  const activeItem = NAV_ITEMS.find((item) => item.route === route);
  const flush = route === "downloads";

  return (
    <div className="shell">
      <Sidebar
        active={route}
        onNavigate={setRoute}
        rootLabel={summary?.rootDisplay ?? null}
        fileCount={summary?.totalFiles ?? null}
      />

      <main className="main">
        <header className="topbar">
          <div className="topbar-heading">
            <h1>{activeItem?.label ?? "Cleanup"}</h1>
            {summary ? (
              <div className="topbar-sub">{formatCount(summary.totalFiles)} files indexed</div>
            ) : null}
          </div>

          <button
            className="findbar"
            onClick={() => setFindOpen(true)}
            title="Quick Find (Ctrl+K)"
          >
            <IconSearch size={13} />
            <span>Describe what you're looking for</span>
            <span className="kbd">Ctrl K</span>
          </button>

          <FolderSwitcher />

          <div className="topbar-actions">
            <button
              className="btn btn-ghost"
              onClick={() => setHelpOpen(true)}
              title="Keyboard shortcuts (?)"
              aria-label="Keyboard shortcuts"
            >
              <span className="kbd">?</span>
            </button>
            {status === "scanning" ? (
              <>
                <span className="spinner" />
                <button className="btn" onClick={() => void cancel()}>
                  Stop
                </button>
              </>
            ) : (
              <button
                className={summary ? "btn" : "btn btn-primary"}
                onClick={() => void startScan()}
                title="Rescan (R)"
              >
                <IconRefresh size={13} />
                {summary ? "Rescan" : "Scan"}
              </button>
            )}
          </div>
        </header>

        <div className={`content ${flush ? "flush" : ""}`}>
          {!ready ? (
            <div className="empty">
              <span className="spinner" />
            </div>
          ) : (
            <>
              {warning ? (
                <div style={{ padding: flush ? "16px 20px 0" : "0 0 16px" }}>
                  <Banner
                    tone="warn"
                    title="Your settings file could not be read"
                    text={warning}
                    onDismiss={dismissWarning}
                  />
                </div>
              ) : null}

              {error ? (
                <div style={{ padding: flush ? "16px 20px 0" : "0 0 16px" }}>
                  <Banner
                    tone="danger"
                    title="That scan could not finish"
                    text={error.message}
                    hint={error.hint}
                    onDismiss={dismissError}
                  />
                </div>
              ) : null}

              {ops.error ? (
                <div style={{ padding: flush ? "16px 20px 0" : "0 0 16px" }}>
                  <Banner
                    tone="danger"
                    title="That operation failed"
                    text={ops.error.message}
                    hint={ops.error.hint}
                    onDismiss={ops.dismissError}
                  />
                </div>
              ) : null}

              <RecoveryBanner />

              {route === "sessions" ? <SessionsView onJump={jump} /> : null}
              {route === "swipe" ? <SwipeView onJump={jump} /> : null}
              {route === "downloads" ? (
                <DownloadsView
                  preset={preset}
                  presetNonce={presetNonce}
                  searchRef={searchRef}
                  onContextMenu={openMenu}
                  onFocusRow={(row) => {
                    focusedRow.current = row;
                  }}
                />
              ) : null}
              {route === "duplicates" ? <DuplicatesView /> : null}
              {route === "storage" ? <StorageView onContextMenu={openMenu} /> : null}
              {route === "apps" ? <AppsView /> : null}
              {route === "settings" ? <SettingsView /> : null}
              {route === "queue" ? <QueueView /> : null}
            </>
          )}
        </div>
      </main>

      <SelectionBar onReview={() => setRoute("queue")} />

      {ops.notice ? <div className="toast">{ops.notice}</div> : null}

      <ContextMenu state={menu} onClose={() => setMenu(null)} />

      {ops.prompt?.kind === "rename" ? (
        <PromptDialog
          title="Rename"
          label="New name"
          initial={ops.prompt.row.name}
          confirmLabel="Rename"
          selectStem
          error={ops.promptError}
          busy={ops.busy}
          onConfirm={(value) => void ops.submitPrompt(value)}
          onCancel={ops.cancelPrompt}
        />
      ) : null}

      {ops.prompt?.kind === "newFolder" ? (
        <PromptDialog
          title="New folder"
          label={`Name for the folder holding ${ops.prompt.ids.length} ${
            ops.prompt.ids.length === 1 ? "file" : "files"
          }`}
          initial="New folder"
          confirmLabel="Create and move"
          error={ops.promptError}
          busy={ops.busy}
          onConfirm={(value) => void ops.submitPrompt(value)}
          onCancel={ops.cancelPrompt}
        />
      ) : null}

      {findOpen ? <QuickFind onClose={() => setFindOpen(false)} /> : null}
      {helpOpen ? <ShortcutsOverlay onClose={() => setHelpOpen(false)} /> : null}

      {/* Selection count is shown in the sidebar; this keeps it reactive. */}
      <span hidden>{selected.size}</span>
    </div>
  );
}
