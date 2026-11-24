import { useCallback, useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Banner } from "@/components/Banner";
import { DrivePicker } from "@/components/DrivePicker";
import { IconArrowDown, IconArrowUp, IconClose, IconFolder } from "@/components/icons";
import { api } from "@/ipc/client";
import { CATEGORIES, CATEGORY_LABELS, type Category } from "@/ipc/types";
import { CATEGORY_COLORS } from "@/lib/categories";
import { formatBytes, formatCount } from "@/lib/format";
import { useScanStore } from "@/state/scanStore";
import { useSettingsStore } from "@/state/settingsStore";
import { useThumbStore } from "@/state/thumbStore";

export function SettingsView() {
  const {
    settings,
    saving,
    error,
    load,
    update,
    addRoot,
    removeRoot,
    excludePath,
    unexcludePath,
    dismissError,
  } = useSettingsStore();
  const { platform, startScan, status } = useScanStore();
  const [cacheCount, setCacheCount] = useState<number | null>(null);
  const [thumbUsage, setThumbUsage] = useState<[number, number] | null>(null);
  const previews = useThumbStore((state) => state.enabled);
  const setPreviews = useThumbStore((state) => state.setEnabled);

  const refreshThumbUsage = useCallback(async () => {
    try {
      setThumbUsage(await api.thumbCacheUsage());
    } catch {
      setThumbUsage(null);
    }
  }, []);

  useEffect(() => {
    void refreshThumbUsage();
  }, [refreshThumbUsage]);

  useEffect(() => {
    if (!settings) void load();
  }, [settings, load]);

  const refreshCache = useCallback(async () => {
    try {
      setCacheCount(await api.hashCacheCount());
    } catch {
      setCacheCount(null);
    }
  }, []);

  useEffect(() => {
    void refreshCache();
  }, [refreshCache]);

  if (!settings) {
    return (
      <div className="empty">
        <span className="spinner" />
      </div>
    );
  }

  const pickFolder = async () => {
    const picked = await open({
      directory: true,
      multiple: false,
      title: "Choose a folder Cleanup may scan",
      defaultPath: platform?.homeDir ?? undefined,
    });
    if (typeof picked === "string") await addRoot(picked);
  };

  const pickExcluded = async () => {
    const picked = await open({
      directory: true,
      multiple: false,
      title: "Choose a folder Cleanup must never touch",
      defaultPath: platform?.homeDir ?? undefined,
    });
    if (typeof picked === "string") await excludePath(picked);
  };

  const swipe = settings.swipe;
  const unused = CATEGORIES.filter((category) => !swipe.categories.includes(category));

  // Membership and order are one list, because the order *is* the review
  // order — so checking a box appends rather than sorting.
  const addSwipeCategory = (category: Category) => {
    void update({ swipe: { ...swipe, categories: [...swipe.categories, category] } });
  };

  const removeSwipeCategory = (category: Category) => {
    void update({
      swipe: { ...swipe, categories: swipe.categories.filter((c) => c !== category) },
    });
  };

  const moveSwipeCategory = (index: number, delta: number) => {
    const next = [...swipe.categories];
    const target = index + delta;
    const moving = next[index];
    const displaced = next[target];
    // Both reads are bounds-checked rather than asserted, so a stale index
    // from a re-render cannot write `undefined` into the list.
    if (moving === undefined || displaced === undefined) return;
    next[index] = displaced;
    next[target] = moving;
    void update({ swipe: { ...swipe, categories: next } });
  };

  return (
    <div className="view">
      {error ? (
        <Banner
          tone="danger"
          title="That folder cannot be used"
          text={error.message}
          hint={error.hint}
          onDismiss={dismissError}
        />
      ) : null}

      <div className="card">
        <div className="card-head">
          <h2>Drives on this computer</h2>
          <span className="card-sub">Scanning one is a deliberate choice</span>
        </div>

        <p className="setting-help" style={{ marginBottom: 12 }}>
          Cleanup will not scan a whole drive from the folder picker — that has to be chosen here,
          by name. System and application folders are skipped either way.
        </p>

        <DrivePicker />
      </div>

      <div className="card">
        <div className="card-head">
          <h2>Folders Cleanup may read</h2>
          <span className="card-sub">{saving ? "Saving…" : "Nothing else is ever touched"}</span>
        </div>

        <p className="setting-help" style={{ marginBottom: 12 }}>
          This list is the whole of Cleanup's access. Scanning a folder adds it here, and removing
          it withdraws permission — including the permission to move or delete anything inside it.
          System and application folders are always refused.
        </p>

        <div className="root-list">
          {settings.roots.length === 0 ? (
            <div className="root-item dim">No folders approved yet.</div>
          ) : (
            settings.roots.map((root) => (
              <div className="root-item" key={root}>
                <IconFolder size={14} />
                <span className="root-path selectable" title={root}>
                  {root}
                </span>
                <button
                  className="btn"
                  disabled={status === "scanning"}
                  onClick={() => void startScan(root)}
                >
                  Scan
                </button>
                <button
                  className="btn-ghost"
                  onClick={() => void removeRoot(root)}
                  aria-label={`Remove ${root}`}
                  title="Withdraw access"
                >
                  <IconClose size={14} />
                </button>
              </div>
            ))
          )}
        </div>

        <div style={{ marginTop: 12 }}>
          <button className="btn" onClick={() => void pickFolder()}>
            Add a folder…
          </button>
        </div>
      </div>

      <div className="card">
        <div className="card-head">
          <h2>Folders Cleanup must never touch</h2>
          <span className="card-sub">Stronger than the list above</span>
        </div>

        <p className="setting-help" style={{ marginBottom: 12 }}>
          A folder listed here is skipped entirely: never scanned, never indexed, and refused by
          every move, rename and delete — even when it sits inside a folder you approved above. Use
          it for folders whose contents look like clutter but are not, such as a Unity project, a
          game engine cache, or a node_modules tree.
        </p>

        <div className="root-list">
          {settings.excludedPaths.length === 0 ? (
            <div className="root-item dim">Nothing excluded.</div>
          ) : (
            settings.excludedPaths.map((path) => (
              <div className="root-item" key={path}>
                <IconFolder size={14} />
                <span className="root-path selectable" title={path}>
                  {path}
                </span>
                <button
                  className="btn-ghost"
                  onClick={() => void unexcludePath(path)}
                  aria-label={`Stop excluding ${path}`}
                  title="Stop excluding this folder"
                >
                  <IconClose size={14} />
                </button>
              </div>
            ))
          )}
        </div>

        <div style={{ marginTop: 12 }}>
          <button className="btn" onClick={() => void pickExcluded()}>
            Exclude a folder…
          </button>
        </div>

        <p className="setting-help" style={{ marginTop: 12 }}>
          Takes effect on the next scan. Files already indexed from a folder you just excluded stay
          listed until then, but every operation on one is refused.
        </p>
      </div>

      <div className="card">
        <div className="card-head">
          <h2>Swipe Clean</h2>
          <span className="card-sub">What you get asked about</span>
        </div>

        <p className="setting-help" style={{ marginBottom: 12 }}>
          Reviewing files one at a time is only worth it for files you can judge at a glance, and
          which those are is personal. Pick the kinds you want to see — this order is the order you
          are shown them in.
        </p>

        <div className="swipe-order">
          {swipe.categories.length === 0 ? (
            <div className="root-item dim">
              Nothing selected, so Swipe Clean has nothing to show you.
            </div>
          ) : (
            swipe.categories.map((category, index) => (
              <div className="swipe-order-item" key={category}>
                <span className="swipe-rank">{index + 1}</span>
                <span className="cat-dot" style={{ background: CATEGORY_COLORS[category] }} />
                <span className="swipe-order-label">{CATEGORY_LABELS[category]}</span>
                <button
                  className="btn-ghost"
                  onClick={() => moveSwipeCategory(index, -1)}
                  disabled={index === 0}
                  aria-label={`Show ${CATEGORY_LABELS[category]} earlier`}
                  title="Show earlier"
                >
                  <IconArrowUp size={12} />
                </button>
                <button
                  className="btn-ghost"
                  onClick={() => moveSwipeCategory(index, 1)}
                  disabled={index === swipe.categories.length - 1}
                  aria-label={`Show ${CATEGORY_LABELS[category]} later`}
                  title="Show later"
                >
                  <IconArrowDown size={12} />
                </button>
                <button
                  className="btn-ghost"
                  onClick={() => removeSwipeCategory(category)}
                  aria-label={`Leave ${CATEGORY_LABELS[category]} out of Swipe Clean`}
                  title="Remove"
                >
                  <IconClose size={12} />
                </button>
              </div>
            ))
          )}
        </div>

        {unused.length > 0 ? (
          <div className="swipe-add">
            <span className="muted">Add</span>
            {unused.map((category) => (
              <button className="chip" key={category} onClick={() => addSwipeCategory(category)}>
                <span className="cat-dot" style={{ background: CATEGORY_COLORS[category] }} />
                {CATEGORY_LABELS[category]}
              </button>
            ))}
          </div>
        ) : null}

        <ExtensionFilter
          value={swipe.extensions}
          onChange={(extensions) => void update({ swipe: { ...swipe, extensions } })}
        />

        <div className="setting">
          <div className="setting-body">
            <div className="setting-label">Skip anything smaller than</div>
            <div className="setting-help">
              Deciding about a 4 KB file one swipe at a time is not time well spent. Set this to 0
              to see everything.
            </div>
          </div>
          <div className="setting-control">
            <input
              className="num"
              type="number"
              min={0}
              max={4096}
              value={Math.round(swipe.minBytes / (1024 * 1024))}
              onChange={(event) =>
                void update({
                  swipe: {
                    ...swipe,
                    minBytes: Math.max(0, Number(event.target.value) || 0) * 1024 * 1024,
                  },
                })
              }
            />
            <span className="muted">MB</span>
          </div>
        </div>
      </div>

      <div className="card">
        <div className="card-head">
          <h2>Scanning</h2>
        </div>

        <div className="setting">
          <div className="setting-body">
            <div className="setting-label">Folder depth</div>
            <div className="setting-help">
              How many levels below the chosen folder to explore. A Downloads folder is usually
              shallow; the limit stops a pathological tree from taking forever.
            </div>
          </div>
          <div className="setting-control">
            <input
              className="num"
              type="number"
              min={1}
              max={64}
              value={settings.scan.maxDepth}
              onChange={(event) =>
                void update({
                  scan: { ...settings.scan, maxDepth: Number(event.target.value) || 1 },
                })
              }
            />
          </div>
        </div>

        <Toggle
          label="Include hidden files"
          help="Dot-files on macOS, hidden-attribute files on Windows. They are still counted either way, just not listed."
          value={settings.scan.includeHidden}
          onChange={(includeHidden) => void update({ scan: { ...settings.scan, includeHidden } })}
        />

        <Toggle
          label="Identify files with no extension"
          help="Reads the first 32 bytes of extension-less files to work out what they are. The only time a scan opens a file at all, and never for cloud placeholders or part-downloads."
          value={settings.scan.sniffExtensionless}
          onChange={(sniffExtensionless) =>
            void update({ scan: { ...settings.scan, sniffExtensionless } })
          }
        />

        <div className="setting">
          <div className="setting-body">
            <div className="setting-label">Call a file stale after</div>
            <div className="setting-help">
              {platform?.accessTimeReliable
                ? "Counted from when the file was last opened."
                : "Counted from when the file last changed — Windows does not record file opens by default."}{" "}
              Age on its own is never treated as a reason to delete something.
            </div>
          </div>
          <div className="setting-control">
            <input
              className="num"
              type="number"
              min={1}
              max={3650}
              value={settings.staleDays}
              onChange={(event) =>
                void update({ staleDays: Number(event.target.value) || 1 })
              }
            />
            <span className="muted">days</span>
          </div>
        </div>
      </div>

      <div className="card">
        <div className="card-head">
          <h2>Comparing file contents</h2>
          <span className="card-sub">used by the duplicate check</span>
        </div>

        <div className="setting">
          <div className="setting-body">
            <div className="setting-label">Files read at once</div>
            <div className="setting-help">
              Reading several files in parallel is much faster on an SSD and{" "}
              <em>slower</em> on a spinning disk, where the drive head has to seek between them.
              There is no reliable way to tell the two apart across platforms, so this is a dial
              rather than a guess. Lower it if duplicate checks feel slow on an external drive.
            </div>
          </div>
          <div className="setting-control">
            <input
              className="num"
              type="number"
              min={1}
              max={64}
              value={settings.hash.concurrency}
              onChange={(event) =>
                void update({
                  hash: { ...settings.hash, concurrency: Number(event.target.value) || 1 },
                })
              }
            />
          </div>
        </div>

        <div className="setting">
          <div className="setting-body">
            <div className="setting-label">First chunk compared</div>
            <div className="setting-help">
              Files of the same size are compared by their opening bytes before anything is read
              in full. A larger window rules out more non-duplicates early; a smaller one reads
              less per file. {formatBytes(settings.hash.headBytes)} is a good default.
            </div>
          </div>
          <div className="setting-control">
            <input
              className="num"
              type="number"
              min={4}
              max={4096}
              step={4}
              value={Math.round(settings.hash.headBytes / 1024)}
              onChange={(event) =>
                void update({
                  hash: {
                    ...settings.hash,
                    headBytes: Math.max(4, Number(event.target.value) || 64) * 1024,
                  },
                })
              }
            />
            <span className="muted">KB</span>
          </div>
        </div>

        <Toggle
          label="Also compare byte for byte"
          help="After two files produce the same BLAKE3 hash, read both again and compare every byte. A hash collision is a roughly 1-in-2^128 event, so this is off by default and costs a second full read of everything it checks. Turn it on if you want certainty rather than near-certainty."
          value={settings.hash.verifyBytewise}
          onChange={(verifyBytewise) =>
            void update({ hash: { ...settings.hash, verifyBytewise } })
          }
        />

        <div className="setting">
          <div className="setting-body">
            <div className="setting-label">Remembered comparisons</div>
            <div className="setting-help">
              {cacheCount === null
                ? "The cache could not be read."
                : `${formatCount(cacheCount)} file ${cacheCount === 1 ? "hash is" : "hashes are"} remembered, so repeat checks read nothing.`}{" "}
              Entries are keyed to each file's identity and are discarded automatically if the
              file changes. Clearing only costs time on the next check.
            </div>
          </div>
          <div className="setting-control">
            <button
              className="btn"
              disabled={!cacheCount}
              onClick={async () => {
                await api.clearHashCache();
                await refreshCache();
              }}
            >
              Clear
            </button>
          </div>
        </div>
      </div>

      <div className="card">
        <div className="card-head">
          <h2>Folder names</h2>
          <span className="card-sub">used when sorting by type</span>
        </div>
        <p className="setting-help" style={{ marginBottom: 12 }}>
          Rename these to whatever you would have called them. Sorting by date
          or extension builds its folder names from the files themselves, so
          these only apply to sorting by type.
        </p>
        <div className="folder-names">
          {CATEGORIES.map((category) => (
            <label className="folder-name-row" key={category}>
              <span
                className="swatch"
                style={{ background: CATEGORY_COLORS[category] }}
              />
              <span className="folder-name-label">{CATEGORY_LABELS[category]}</span>
              <input
                className="num folder-name-input"
                value={settings.organizeFolders[category] ?? ""}
                onChange={(event) => {
                  const next = { ...settings.organizeFolders, [category]: event.target.value };
                  void update({ organizeFolders: next });
                }}
                spellCheck={false}
              />
            </label>
          ))}
        </div>
      </div>

      <div className="card">
        <div className="card-head">
          <h2>Previews</h2>
        </div>

        <Toggle
          label="Show image previews"
          help="Thumbnails for PNG, JPEG, GIF, BMP, WebP, TIFF and ICO files, generated only for rows actually on screen and remembered afterwards. Files stored in the cloud are never opened for a preview, because reading one would download it in full. HEIC is not supported — iPhone photos need a decoder that would mean a native dependency."
          value={previews}
          onChange={(value) => {
            setPreviews(value);
            useThumbStore.getState().setEnabled(value);
          }}
        />

        <div className="setting">
          <div className="setting-body">
            <div className="setting-label">Remembered previews</div>
            <div className="setting-help">
              {thumbUsage
                ? `${formatCount(thumbUsage[0])} ${thumbUsage[0] === 1 ? "preview" : "previews"} taking ${formatBytes(thumbUsage[1])}.`
                : "None stored yet."}{" "}
              Clearing only costs time the next time you scroll past them.
            </div>
          </div>
          <div className="setting-control">
            <button
              className="btn"
              disabled={!thumbUsage || thumbUsage[0] === 0}
              onClick={async () => {
                await api.clearThumbCache();
                useThumbStore.getState().clear();
                await refreshThumbUsage();
              }}
            >
              Clear
            </button>
          </div>
        </div>
      </div>

      <div className="card">
        <div className="card-head">
          <h2>Appearance</h2>
        </div>
        <Toggle
          label="Dark theme"
          help="Follows the window chrome. Form controls and scrollbars come from the operating system's webview, so the light variant exists to keep those looking right."
          value={settings.theme === "dark"}
          onChange={(dark) => void update({ theme: dark ? "dark" : "light" })}
        />
      </div>

      <div className="card">
        <div className="card-head">
          <h2>Privacy</h2>
        </div>
        <p className="setting-help">
          Every byte of analysis happens on this computer. Cleanup has no network code at all —
          there is no HTTP client in its dependencies, which you can check in{" "}
          <code className="selectable">src-tauri/Cargo.toml</code>. The window itself holds no
          filesystem permissions; each read goes through a Rust command that checks the folder list
          above first.
        </p>
      </div>
    </div>
  );
}

/**
 * The extension filter, edited as free text.
 *
 * Kept as local state while typing so a half-finished word is not parsed into
 * a filter on every keystroke; committed on blur or Enter. The parsed list is
 * shown back underneath, because "cr2, .CR2, raw" becoming `cr2` and `raw` is
 * exactly the kind of thing that should be visible rather than surprising.
 */
function ExtensionFilter({
  value,
  onChange,
}: {
  value: string[];
  onChange: (value: string[]) => void;
}) {
  const [text, setText] = useState(value.join(", "));
  // Re-sync when the backend returns a cleaned-up list, or another control
  // changes it.
  useEffect(() => {
    setText(value.join(", "));
  }, [value]);

  const commit = () => {
    const parsed = Array.from(
      new Set(
        text
          .split(/[,\s]+/)
          .map((token) => token.trim().replace(/^\./, "").toLowerCase())
          .filter((token) => token.length > 0),
      ),
    ).sort();
    // Only save when something actually changed, so a blur after no edit does
    // not write the settings file.
    if (parsed.join(",") !== value.join(",")) onChange(parsed);
    else setText(value.join(", "));
  };

  return (
    <div className="setting">
      <div className="setting-body">
        <div className="setting-label">Only these formats</div>
        <div className="setting-help">
          Optional. Leave empty for everything in the kinds above, or name extensions to narrow it
          further — <code>cr2, nef, jpg</code> for a photo session, say. This filters within the
          kinds you picked; it cannot add a kind you left out.
        </div>
      </div>
      <div className="setting-control">
        <input
          className="text-input"
          type="text"
          placeholder="all formats"
          value={text}
          onChange={(event) => setText(event.target.value)}
          onBlur={commit}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              commit();
            }
          }}
          aria-label="Limit Swipe Clean to these file extensions"
        />
      </div>
    </div>
  );
}

function Toggle({
  label,
  help,
  value,
  onChange,
}: {
  label: string;
  help: string;
  value: boolean;
  onChange: (value: boolean) => void;
}) {
  return (
    <div className="setting">
      <div className="setting-body">
        <div className="setting-label">{label}</div>
        <div className="setting-help">{help}</div>
      </div>
      <div className="setting-control">
        <button
          className={`switch ${value ? "on" : ""}`}
          role="switch"
          aria-checked={value}
          aria-label={label}
          onClick={() => onChange(!value)}
        >
          <span className="switch-knob" />
        </button>
      </div>
    </div>
  );
}
