# CLEANUP — Architecture (Phase 1)

> Status: design only. No application code written yet.
> Target: Windows 10/11 + macOS 12+ (Apple Silicon & Intel).

---

## 0. Executive summary

| Decision | Choice | One-line reason |
|---|---|---|
| Shell | **Tauri 2** | ~8 MB binary, ~80 MB RAM, and the filesystem work belongs in a real systems language. |
| Core logic | **Rust** (`src-tauri/src/core/`, zero Tauri deps) | Parallel walk + hashing without blocking the UI; unit-testable without a GUI. |
| UI | **React 18 + TypeScript + Vite** | Fast HMR; virtualised lists are a solved problem here. |
| Styling | **Plain CSS + custom properties** | No Tailwind/CSS-in-JS runtime. OS webviews differ, so I want to control exactly what ships. |
| Client state | **Zustand** | ~1 KB, no boilerplate, no context re-render storms. |
| Persistence | **SQLite (`rusqlite`, bundled)** — hash cache + undo journal only | Both must survive a crash. Everything else is re-derivable by rescanning. |
| Settings | **`settings.json`**, atomic write | Tiny, read once at boot, user-inspectable. |
| Hashing | **BLAKE3**, head-hash then full-hash, cached | 3–10× SHA-256, SIMD, still cryptographic. |
| Deletion | **Native Trash only** (`trash` crate) | Permanent deletion is not implemented anywhere in the codebase. |

### The one thing I would push back on

Most of the brief is sound. The part I would argue with is the framing of **"safe to delete"**.
A file manager cannot know intent. So this architecture never produces a boolean
`safe_to_delete`. It produces a **`CleanupReason`** — a structured justification plus a
confidence band — and the UI renders the reason, never a verdict.

Concretely: **age alone never generates a suggestion.** An old file surfaces only when age is
combined with at least one independent signal, e.g. *"installer, 8 months old, **and** a newer
build of the same product is present in this folder"*. This costs us a flashier
"Reclaim 14 GB!" headline. It is the right trade for a tool pointed at someone's personal files.

### Prerequisite gap found on this machine

I checked before recommending the stack:

```
node     v24.20.0   OK
npm      11.19.0    OK
WebView2 runtime    OK   (required by Tauri on Windows)
cargo / rustc       MISSING
MSVC build tools    MISSING  (no Visual Studio Installer present)
```

Tauri needs both of the missing items. One-time install, roughly 3–6 GB on disk:

1. **MSVC C++ Build Tools** — install `Microsoft.VisualStudio.2022.BuildTools` and select
   *Desktop development with C++*.
2. **Rust** — install `Rustlang.Rustup`, then `rustup default stable-msvc`.

If that install is unacceptable, the honest alternative is Electron plus a Node worker-thread
scanner. I do **not** recommend it: ~150 MB bundle, ~400 MB RAM, and the scanner would still
need a native addon for file identity and hardlink detection — so you compile native code
either way, with worse ergonomics. **Recommendation: install the Rust toolchain, use Tauri 2.**

---

## 1. Why this stack, critically

**Where Rust actually wins — and where it does not.**

It does *not* win at hashing throughput as much as is usually claimed: Node's OpenSSL-backed
SHA-256 is hardware-accelerated, and disk I/O dominates either way. The real wins are:

1. **Directory traversal.** A parallel walk (`jwalk` over `rayon`) across 100k entries beats
   recursive `readdir` by 3–10×, and — more importantly — never touches the JS event loop. In
   Electron, a naive scan janks the UI; avoiding that means workers plus structured cloning,
   which is its own tax.
2. **Keeping the index out of the UI.** 100k `FileEntry` records live in Rust memory (~20 MB
   with interned parent paths). The webview never receives the full set — it requests windows.
   This is the single most important performance decision in the app, and it is the natural
   shape in Tauri's model.
3. **Platform identity and edge cases.** `volume_serial_number` + `file_index` on Windows,
   `st_dev`/`st_ino` on macOS, cloud-placeholder reparse attributes, Windows long paths. All
   first-class in Rust std + `windows-sys`; all painful from Node without an addon.
4. **Memory ceiling.** Streaming 1 MiB chunks through BLAKE3 with bounded concurrency gives a
   predictable memory profile under load. Easy to get wrong in a GC'd runtime.

**The genuine cost of Tauri 2, stated plainly.** The webview is the OS's, not Chromium's:
WebView2 on Windows, WKWebView on macOS. They diverge on newer CSS and on scroll/overscroll
behaviour, and WKWebView has historically been fussier about `position: sticky` inside
transformed containers and about scrollbar styling. Mitigations are in the plan, not deferred:
conservative CSS (no layout that depends on `:has()`, no container queries on the critical
path), one virtualised-list implementation used by every view, and cross-platform visual
verification as an explicit Phase 5 task.

**Dependencies, deliberately short.**

*Rust:* `tauri` (+ `dialog`, `opener` plugins), `serde`, `serde_json`, `jwalk`, `rayon`,
`blake3`, `rusqlite` (bundled), `trash`, `dirs`, `thiserror`, `parking_lot`,
`unicode-normalization`, `windows-sys` (Windows only).
No ORM, no extra async runtime, no image/thumbnail crates in the MVP.

*TypeScript:* `react`, `react-dom`, `zustand`, `@tauri-apps/api`, `@tanstack/react-virtual`,
`vite`, `typescript`.
No UI kit, no chart library (storage bars are flexbox plus a little SVG), no router (six
screens need a nav enum, not a router), no date library.

---

## 2. Process and module architecture

```
+---------------------- WebView (React / TS) -----------------------+
|  features/  overview . downloads . duplicates . storage .         |
|             organize . queue . settings                           |
|  state/     zustand stores (summary, page cache, selection, queue)|
|  ipc/       the ONLY module allowed to call invoke() / listen()   |
+-------------------------------+-----------------------------------+
                   invoke(cmd)  |  events: scan:progress, hash:progress,
                   JSON         |          op:progress, op:done
+------------------------------v------------------------------------+
|  commands/   thin adapters: deserialize -> validate -> core -> DTO|
|              (no business logic lives here)                       |
+-------------------------------------------------------------------+
|  core/       ZERO Tauri dependencies - plain Rust, `cargo test`    |
|   walk  identity  hash  dup  analyze  suggest  safety  fsops      |
|   category  journal  db  model                                    |
+-------------------------------------------------------------------+
|  state.rs    AppState: RwLock<ScanIndex>, cancel tokens, DB conn  |
+-------------------------------------------------------------------+
```

Two rules make this hold up:

- **`core/` must never import `tauri`.** It takes a `&dyn ProgressSink` and a `&CancelToken`
  instead of emitting events itself. That is what makes the scanner, the duplicate grouper, the
  path guard and the undo journal testable against temp directories with no window and no
  webview. Every safety-critical rule in section 6 is a unit test, not a code review comment.

  This is enforced by the compiler, not by convention: `tauri` and the plugins sit behind a
  default-on `app` feature, and the test suite runs with `--no-default-features`. If anything in
  `core/` ever reaches for a Tauri type, the test build stops compiling.
- **`ipc/` is the only TS module that touches `invoke`.** Features consume typed hooks. When a
  Rust DTO changes, exactly one file breaks.

---

## 3. Project folder structure

```
cleanup/
├─ package.json  tsconfig.json  vite.config.ts  index.html
├─ ARCHITECTURE.md
├─ src/
│  ├─ main.tsx
│  ├─ App.tsx                     # shell: sidebar + active view + command palette
│  ├─ app/
│  │  ├─ nav.ts                   # Route enum, nav metadata
│  │  ├─ shortcuts.ts             # global key handling
│  │  └─ theme.css                # design tokens (color, spacing, type scale)
│  ├─ ipc/
│  │  ├─ types.ts                 # mirrored Rust DTOs (the one place drift can occur)
│  │  ├─ client.ts                # typed invoke wrappers
│  │  └─ events.ts                # typed listen wrappers + teardown
│  ├─ state/
│  │  ├─ scanStore.ts             # status, summary, progress, windowed page cache
│  │  ├─ selectionStore.ts        # selected file ids, shared across views
│  │  ├─ queueStore.ts            # cleanup queue + pending plans
│  │  └─ settingsStore.ts
│  ├─ features/
│  │  ├─ overview/                # ScanCard, SignalList, StorageRing
│  │  ├─ downloads/               # FileTable (virtualised), FilterBar
│  │  ├─ duplicates/              # GroupList, KeepPicker
│  │  ├─ storage/                 # LargestFiles, FolderRollup, TypeBreakdown
│  │  ├─ organize/                # PlanPreview, DestinationEditor
│  │  ├─ queue/                   # QueueList, ConfirmSheet
│  │  └─ settings/
│  ├─ components/                 # Button, Checkbox, Dialog, VirtualList, Progress…
│  └─ lib/                        # formatBytes, formatRelativeDate, fuzzyMatch
└─ src-tauri/
   ├─ Cargo.toml  tauri.conf.json  build.rs
   ├─ capabilities/default.json    # Tauri 2 permission allowlist
   ├─ icons/
   └─ src/
      ├─ main.rs                   # entry; wires plugins + AppState
      ├─ lib.rs
      ├─ error.rs                  # AppError -> serialisable ErrorDto
      ├─ state.rs
      ├─ commands/
      │  ├─ mod.rs  scan.rs  query.rs  duplicates.rs
      │  └─ organize.rs  cleanup.rs  settings.rs
      └─ core/
         ├─ mod.rs
         ├─ model.rs               # FileEntry, Category, ScanSummary, Plan…
         ├─ category.rs            # extension table + magic-byte fallback
         ├─ walk.rs                # parallel walk, loop & symlink guards
         ├─ identity.rs            # file id, hidden, cloud placeholder (per-OS cfg)
         ├─ hash.rs                # head hash, full hash, cache lookup
         ├─ dup.rs                 # size -> head -> full grouping; keep-ranking
         ├─ analyze.rs             # largest files/folders, stale, empty dirs
         ├─ suggest.rs             # organize plan; CleanupReason generation
         ├─ safety.rs              # root allowlist, system denylist, canonicalisation
         ├─ fsops.rs               # move / trash, never-overwrite, cross-volume copy
         ├─ journal.rs             # operation log + undo
         └─ db.rs                  # schema, migrations
```

`core/` is flat, one concern per file. No `utils.rs`, no `helpers/`.

---

## 4. Data model

### 4.1 In-memory (the hot path)

```rust
pub struct ScanIndex {
    pub scan_id: u64,              // bumped per scan; stale events are dropped by the UI
    pub root: PathBuf,
    pub entries: Vec<FileEntry>,   // id == index into this Vec
    pub dirs: Vec<DirEntryRec>,    // for folder roll-up
    pub parents: Vec<Arc<str>>,    // interned parent paths
    pub errors: Vec<ScanError>,
    pub summary: ScanSummary,
    pub finished_at: Option<SystemTime>,
}

pub struct FileEntry {
    pub id: u32,
    pub parent: u32,               // index into ScanIndex::parents
    pub name: Box<str>,            // display form, NFC-normalised
    pub ext: Option<Box<str>>,     // lowercased, no dot
    pub size: u64,
    pub created: Option<i64>,      // unix seconds; None where the OS has no value
    pub modified: i64,
    pub accessed: Option<i64>,     // unreliable on Windows; see §7
    pub category: Category,
    pub flags: EntryFlags,         // bitflags
    pub ident: Option<FileIdent>,  // (volume, file_id) for hardlink / identity checks
    pub head_hash: Option<[u8; 32]>,
    pub full_hash: Option<[u8; 32]>,
}

bitflags! {
    pub struct EntryFlags: u16 {
        const HIDDEN            = 1 << 0;
        const SYMLINK           = 1 << 1;
        const READONLY          = 1 << 2;
        const CLOUD_PLACEHOLDER = 1 << 3;  // never hash, never move - see §7
        const IN_PROGRESS       = 1 << 4;  // .crdownload/.part/.download, or mtime < 60s
        const SYSTEM_PROTECTED  = 1 << 5;  // matched the denylist; ops refused
        const APP_BUNDLE        = 1 << 6;  // macOS *.app - one unit, not traversed
    }
}

pub enum Category {
    Images, Videos, Audio, Documents, Archives,
    Installers, Development, Temporary, Misc,
}
```

Why `name` is `Box<str>` and the parent is interned: paths dominate memory. 100k files in a
Downloads tree share a few hundred parent directories. Storing each full path would cost
~12 MB in duplicated prefixes; interning drops the index to roughly 20 MB total, which
comfortably satisfies the "avoid excessive memory consumption" requirement.

Why `accessed` is `Option`: Windows disables last-access-time updates by default
(`NtfsDisableLastAccessUpdate`), so "not accessed in a year" is a lie on most Windows
machines. The analyzer uses `modified` there and the UI labels it honestly. This is exactly
the kind of silent cross-platform wrongness the brief asked me to avoid.

### 4.2 Persisted (SQLite — `cleanup.db`)

Only two things genuinely need to persist. Everything else is cheaper to recompute.

```sql
-- 1. Hash cache. Keyed on file IDENTITY, not path, so a rename or a move
--    performed by this app does not invalidate an expensive hash.
--    volume_id and file_id hold u64 values reinterpreted as i64, because
--    SQLite has no unsigned type; they are opaque keys, never ordered.
CREATE TABLE hash_cache (
  volume_id   INTEGER NOT NULL,
  file_id     INTEGER NOT NULL,
  size        INTEGER NOT NULL,
  mtime_ns    INTEGER NOT NULL,
  full_hash   BLOB NOT NULL,      -- 32 bytes, BLAKE3
  computed_at INTEGER NOT NULL,
  PRIMARY KEY (volume_id, file_id)
) WITHOUT ROWID;
-- A hit is trusted only if size AND mtime_ns both still match.

-- 2. Undo journal. Written BEFORE any filesystem mutation (see §6).
CREATE TABLE operations (
  id          INTEGER PRIMARY KEY,
  kind        TEXT NOT NULL,      -- 'move' | 'trash' | 'mkdir'
  root        TEXT NOT NULL,      -- the allowlisted root this op was scoped to
  created_at  INTEGER NOT NULL,
  state       TEXT NOT NULL,      -- 'planned'|'running'|'done'|'partial'|'undone'
  undoable    INTEGER NOT NULL,
  note        TEXT
);

CREATE TABLE operation_items (
  id            INTEGER PRIMARY KEY,
  operation_id  INTEGER NOT NULL REFERENCES operations(id),
  from_path     TEXT NOT NULL,
  to_path       TEXT,             -- final path actually used (collisions renamed)
  size          INTEGER NOT NULL,
  state         TEXT NOT NULL,    -- 'pending'|'done'|'failed'|'reverted'
  error         TEXT
);
CREATE INDEX idx_items_op ON operation_items(operation_id);

CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);  -- schema_version
```

`PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;` for the journal writes — correctness over
throughput, since the volume of writes is trivial compared to the cost of losing an undo record.

What is deliberately **not** persisted: the scan index, duplicate groups, suggestions. A
rescan with a warm hash cache is fast, and a stale snapshot of someone's filesystem is a
safety hazard — it invites the app to act on files that have since changed.

### 4.3 Settings (`settings.json`)

```jsonc
{
  "schemaVersion": 1,
  "theme": "dark",
  "roots": ["<downloads>"],            // explicitly approved scan roots
  "scan": { "maxDepth": 8, "followSymlinks": false, "includeHidden": false },
  "hash": { "concurrency": 6, "headBytes": 65536, "verifyBytewise": false },
  "stale": { "days": 180 },
  "organize": { "folders": { "Images": "Images", "Videos": "Videos", "…": "…" } },
  "ignore": ["desktop.ini", ".DS_Store", "._*", "Thumbs.db"]
}
```

Written atomically (temp file + rename) so a crash mid-write cannot corrupt it.
`followSymlinks` exists as a setting but is **ignored for write operations** — see §6.

---

## 5. How scanning and duplicate detection work

### 5.1 Phase A — metadata walk (always; no file contents read)

Parallel walk with `jwalk`, `follow_links(false)`, `rayon` thread pool.

Per entry: name, extension (lowercased), size, timestamps, `is_dir`, `is_symlink`,
platform file identity, platform attribute flags, category.

Guards, all enforced in `walk.rs`:

- **Symlinks are never followed.** They are recorded as entries and excluded from hashing,
  organizing and deletion.
- **Loop protection**: a visited set of `(volume_id, file_id)` for directories. Even with
  symlinks off, Windows junctions, macOS firmlinks (`/System/Volumes/Data`) and bind mounts can
  produce cycles.
- **Depth cap** (default 8). A Downloads folder is shallow; the cap exists to make a
  pathological tree non-fatal.
- **macOS `*.app` bundles are single units.** A bundle is a directory with tens of thousands of
  entries inside. Traversing it floods the index and, worse, lets the organizer shred an
  application. Treated as one entry, flagged `APP_BUNDLE`.
- **Noise filter**: `.DS_Store`, `._*` AppleDouble, `desktop.ini`, `Thumbs.db`.
- **Errors never abort.** A permission error is pushed to `ScanIndex::errors` with its path and
  OS error, the walk continues, and the UI shows "N items could not be read" with a disclosure.

Progress and cancellation:

- `scan:progress` is emitted on a **100 ms throttle** carrying `{scan_id, files, dirs, bytes,
  current_dir}` — never one event per file. Emitting per-file is the classic way to make an
  IPC-bridged UI slower than the disk.
- Cancellation is an `AtomicBool` checked per directory batch. `scan_id` lets the UI discard
  events from a scan it has already abandoned.

### 5.2 Phase B — duplicate detection (lazy; the cheapest test first)

Four escalating stages. Each stage only sees what survived the last one.

1. **Group by exact size.** Any group of one cannot be a duplicate — discarded with
   **zero bytes read**. On a typical Downloads folder this removes 85–95% of files immediately.
2. **Collapse identical identity.** Within a size group, entries sharing `(volume_id, file_id)`
   are the *same file* reached twice (hardlink, junction). Collapsed, not reported as duplicates.
   Zero-byte files are excluded entirely rather than reported as 400 mutual duplicates, which is
   technically true and practically useless.

   **Implementation note, since it bit us:** on Windows the metadata walk cannot supply identity —
   `MetadataExt::volume_serial_number`/`file_index` are still unstable, and the stable
   `GetFileInformationByHandle` costs a file handle per entry, which is far too expensive for a
   walk. So identity is read from the open handle during stage 3, where the file is being opened
   anyway, and the collapse runs again after hashing. This also matters because the cache is keyed
   on identity: without it, Windows would silently never record a cache hit. On Unix `lstat`
   provides `st_dev`/`st_ino` for free, so stage 2 can collapse immediately. Loop protection
   during the walk does not depend on any of this — it keys on the canonical directory path.
3. **Head hash.** BLAKE3 over the first 64 KiB. One block read per file. Sub-groups that fall
   to size one are discarded. This eliminates nearly all remaining false candidates — different
   files of identical size almost never share a 64 KiB prefix.
4. **Full hash**, streamed in 1 MiB chunks, cancellable between chunks, bounded concurrency.
   Consults `hash_cache` on `(volume_id, file_id)` first and trusts the hit only if `size` and
   `mtime_ns` both match.

Never hashed: symlinks, cloud placeholders, in-progress downloads, anything on the denylist.

**Concurrency note, honestly.** Unbounded parallel hashing is a large win on NVMe and a *loss*
on a spinning disk (seek thrash). Reliable cross-platform drive-type detection is not worth the
complexity, so the default is `min(6, cores)` with a user setting, and hashing is scheduled
per-volume so one slow external drive cannot starve the internal one.

**Collision stance.** Equal size plus equal full BLAKE3 is treated as duplicate. A 256-bit
cryptographic digest collision is not a real risk (~2⁻¹²⁸). For users who want absolute
certainty there is an opt-in `verifyBytewise` setting that byte-compares the survivors of a
group before they can be queued. Default off; the UI states which mode produced the result.

**Which copy to keep** — a ranked *suggestion*, each with a visible reason:

| Signal | Preference |
|---|---|
| Location | A copy outside Downloads (i.e. already filed somewhere) wins |
| Name | A copy *without* `(1)`, `copy`, ` - Copy`, `~` wins |
| Age | Oldest `created` wins — most likely the original |
| Path depth | Shallower, more deliberate location wins |

Nothing is pre-selected for deletion. The user selects; the app never ships a screen where
clicking one button trashes files chosen entirely by a heuristic.

### 5.3 Storage analysis

Computed from the in-memory index — no extra disk reads:

- Largest files (partial sort, top N).
- **Folder roll-up**: recursive size per directory, computed bottom-up in one pass over `dirs`.
- Breakdown by category, by count and by bytes.
- Stale: no modification in `stale.days` (**not** presented as a deletion reason on its own).
- Empty directories, and directories containing only noise files — reported separately, since
  "empty" to a user means "nothing I care about".

---

## 6. Safe file operations and undo

This is the part I would want reviewed most carefully, so the rules are explicit and each one
maps to a unit test in `core/safety.rs` or `core/fsops.rs`.

### 6.1 The path guard (`safety.rs`)

Every mutating operation passes through one function. There is no second code path.

```rust
pub fn guard(raw: &Path, roots: &[PathBuf]) -> Result<PathBuf, SafetyError>
```

1. **Canonicalise** (resolving symlinks and `..`) *before* any check. Checking the raw path is
   how directory-traversal bugs happen.
2. **Root allowlist**: the canonical path must be a descendant of a root the user explicitly
   approved. A symlink inside Downloads pointing at `/System` canonicalises outside the root
   and is rejected.
3. **System denylist**, checked on the canonical path:
   - Windows: `C:\Windows`, `Program Files`, `Program Files (x86)`, `ProgramData`,
     `AppData\Local\Microsoft`, `AppData\Local\Packages`, `$Recycle.Bin`, `System Volume Information`,
     any drive root.
   - macOS: `/System`, `/Library`, `/usr`, `/bin`, `/sbin`, `/private`, `/Applications`,
     `~/Library`, any volume root.
4. **Reject** symlinks, cloud placeholders, in-progress downloads, `*.app` bundles.
5. Re-checked **at execution time**, not only at suggestion time. The filesystem can change
   between preview and apply; defence in depth is cheap here.

### 6.2 Move (organize)

- **Never overwrites.** If the destination exists, the app picks `name (2).ext` and records the
  name it actually used. There is no code path that passes an existing destination to a rename.
- **Same volume → `rename`**: instant and effectively atomic. Organizing into a subfolder of
  Downloads is always same-volume, so the common case is fast and safe.
- **Cross volume → copy, verify size, then trash the source.** The source goes to Trash, not
  `unlink`, so a failed verification is still recoverable.
- Only **top-level loose files** are organized by default. The app does not reach into
  subfolders the user already made — that is their organisation, and rearranging it is the
  fastest way to lose a user's trust.
- Per-item failure does not abort the batch. Results are collected; the journal records the
  truth; the UI reports "47 moved, 2 failed" with the reasons.

### 6.3 Trash (cleanup)

- The `trash` crate: `IFileOperation` on Windows (true Recycle Bin), `NSFileManager
  trashItemAtURL:` on macOS. **No `std::fs::remove_file` exists in the codebase for user files.**
- **The trap this avoids:** the Windows Recycle Bin does not exist on every volume. Network
  shares, many removable drives, and volumes with the bin disabled will *permanently delete*
  instead. The app probes the target volume first and, if the Recycle Bin is unavailable,
  **refuses the operation** and says why. It does not silently fall back to permanent deletion.
- macOS equivalent: per-volume `.Trashes`, same probe, same refusal.

### 6.4 Undo

An `Operation` with its `OperationItem`s is written to SQLite and **committed before the first
filesystem call**, in state `planned`. Each item transitions `pending → done | failed`
individually. A crash mid-batch therefore leaves a durable record of exactly what was attempted,
and the app offers recovery on next launch.

| Operation | Undo | Honest limitation |
|---|---|---|
| `mkdir` | Remove, **only if still empty** | None |
| `move` | Move back, only if the destination still exists and the origin is free | If the user moved it again, undo is refused with an explanation — not forced |
| `trash` | Best-effort restore from the OS trash | **Not guaranteed.** See below |

**On trash undo, plainly:** restoring from the Recycle Bin programmatically is awkward and not
reliable across Windows configurations, and the OS may have emptied it. So the app does not
promise restoration. It records every original path, and on undo it attempts a restore; where
it cannot, it shows the exact original paths and opens the Recycle Bin / Trash so the user can
restore manually. Promising an undo that silently fails would be worse than not promising one.

### 6.5 What the app will not do

No permanent deletion, ever. No writes outside an approved root. No whole-disk scan without an
explicit per-root approval. No network calls of any kind — there is no HTTP client in the
dependency list, which makes "all analysis stays local" an auditable property of `Cargo.toml`
rather than a promise. No modification of system or application directories. No action on any
file whose full path was not shown to the user first.

---

## 7. Windows / macOS compatibility problems

Ordered by how likely each one is to cause a real bug.

| # | Issue | Handling |
|---|---|---|
| 1 | **Cloud placeholders.** OneDrive (Windows) and iCloud Drive (macOS) report a file size but hold no local data. Reading one triggers a multi-GB download over the user's network. | Detect `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` / `RECALL_ON_OPEN` / `OFFLINE` on Windows, and `.icloud` stubs on macOS. Flag `CLOUD_PLACEHOLDER`: counted in the index, **never hashed, never moved**. Most cleanup tools get this wrong and it is extremely user-hostile. **Known gap:** the macOS `SF_DATALESS` flag, which covers an evicted-but-materialised file, needs a `libc` `lstat` and could not be compiled or tested on this machine, so it is deliberately not implemented rather than shipped unverified. The `.icloud` stub check covers iCloud Drive's actual on-disk representation; close this gap when a Mac is available. |
| 2 | **Downloads folder location.** `%USERPROFILE%\Downloads` is wrong — the folder is relocatable via the registry. | `SHGetKnownFolderPath(FOLDERID_Downloads)` on Windows (which is what the `dirs` crate uses), `$HOME/Downloads` on macOS. Never string-built. If the known folder does not resolve, prompt rather than guess. |
| 3 | **Unicode normalisation.** APFS/HFS+ hand back NFD (`e` + combining acute); Windows stores NFC. The same filename differs byte-wise across platforms, breaking search and name comparison. | `OsString`/`PathBuf` internally; NFC-normalise only for display and for search comparison. Never round-trip a path through a normalised `String` and use it for I/O. |
| 4 | **Non-UTF-8 paths.** Windows paths are UTF-16 and may contain unpaired surrogates; Unix paths are arbitrary bytes. Neither is guaranteed valid UTF-8. | Keep `PathBuf` as the source of truth; lossy-convert for display only. Each entry carries an opaque id, so the UI never needs to send a path string back for an operation. |
| 5 | **macOS TCC privacy.** Reading `~/Downloads` triggers a consent prompt; a denial returns an opaque permission error. Broader scans need Full Disk Access. | `NSDownloadsFolderUsageDescription` in `Info.plist`; detect denial and show a guided panel that opens System Settings → Privacy & Security, instead of an error toast. |
| 6 | **Last-access time.** Windows disables last-access updates by default, so "not opened in a year" is simply false there. | `accessed: Option<i64>`; fall back to `modified` and label the column honestly per platform. |
| 7 | **Recycle Bin availability** (see §6.3) — absent on network/removable volumes, where "delete" means permanent. | Probe, then refuse. Never silently fall back. |
| 8 | **Windows file locking.** A file open in another process cannot be moved or deleted (`ERROR_SHARING_VIOLATION`). macOS permits it. | Expect per-item failure on Windows; surface "in use by another program" rather than a raw OS code. Partial-batch success is a first-class outcome, not an error state. |
| 9 | **Long paths.** Windows `MAX_PATH` is 260 unless long paths are enabled; Rust's std uses the `\\?\` prefix for some operations but not all. | Prefix verbatim paths for filesystem operations; strip the prefix for display. |
| 10 | **Case sensitivity.** Windows is case-insensitive; macOS APFS is case-insensitive *by default* but can be case-sensitive. | Never assume. Collision checks before a move use an actual existence probe, not a string comparison against the index. |
| 11 | **Reparse points, junctions, firmlinks.** Windows junctions and macOS firmlinks make one tree appear in two places — a naive scanner reports every file as a duplicate of itself. | `(volume_id, file_id)` identity collapse in stage 2 of §5.2 handles exactly this. |
| 12 | **App bundles.** `*.app` is a directory on macOS; `.exe`/`.msi` are single files. | `APP_BUNDLE` flag, never traversed, never organized. |
| 13 | **Hidden files.** Windows uses an attribute bit; macOS uses a leading dot *and* a `UF_HIDDEN` flag. | Separate `cfg`-gated implementations in `identity.rs`; one `HIDDEN` flag upward. |
| 14 | **Antivirus.** Defender's real-time scanning can slow a first full hash pass severalfold. | Nothing to fix; keep it off the critical path by hashing lazily, and the progress UI must stay responsive while slow. |
| 15 | Trash plus code signing / notarisation for distribution. | macOS needs hardened runtime, signing and notarisation or the app will not open; Windows wants an Authenticode certificate or SmartScreen warns. Phase 5 task, with real calendar time attached. |

### Top technical risks, and the safer alternative taken

1. **A mass move breaks the user's mental model of their own folder.** → Dry-run preview with
   exact paths, per-file deselect, editable destinations, one-action undo, and no reaching into
   folders the user already organised.
2. **Cloud placeholder hydration** burning bandwidth and disk. → Detect and exclude (§7.1).
3. **Permanent deletion when the Recycle Bin is unavailable.** → Probe and refuse (§6.3).
4. **Trashing an in-progress download.** → `.crdownload`/`.part`/`.download` plus an
   mtime-within-60s check, flagged `IN_PROGRESS` and excluded from every suggestion.
5. **Stale hash cache returning a wrong duplicate verdict.** → Identity-keyed, and a hit is
   only trusted when size *and* mtime both still match.
6. **IPC flooding** with 100k rows. → Index stays in Rust; the UI requests sorted, filtered
   windows of ~200 rows and renders them virtualised.
7. **Over-confident deletion advice.** → `CleanupReason` with a confidence band, never a
   boolean; age alone never qualifies (§0).
8. **Symlink/junction loops** hanging a scan. → Identity visited-set plus a depth cap.

---

## 8. MVP scope

**In — the MVP is the four core screens working end to end, on real files, with undo.**

- App shell, six-item sidebar nav, dark theme, keyboard shortcuts for navigation and selection.
- Downloads detection per platform; add a custom root via the native folder picker.
- Async parallel scan: live progress, cancellation, per-path error reporting.
- Categorisation, `ScanSummary`, Overview screen with the top cleanup signals.
- Downloads screen: virtualised table, sort, filter by category/size/date/extension, text search.
- Duplicates: size → identity → head-hash → full-hash pipeline, grouped, keep-suggestion with
  reason, bulk selection, savings total.
- Storage: largest files, folder roll-up, category breakdown, stale files, empty folders.
- Organize: generated plan, preview, per-file deselect, editable destination, apply, undo.
- Cleanup queue: accumulate from any screen, review with full paths, total bytes, move to Trash,
  undo entry.
- Settings: roots, depth, hash concurrency, stale threshold, folder names, ignore list.
- Persistence: hash cache + undo journal, with crash recovery on launch.
- Tests on the parts that can destroy data: path guard, denylist, collision renaming, undo
  round-trip, symlink-escape rejection, duplicate grouping against fixture trees.

**Out of the MVP — explicitly deferred, and why**

| Deferred | Reason |
|---|---|
| Near-duplicate / perceptual image matching | A whole second engine with a real false-positive rate. Exact duplicates deliver most of the value at zero risk. |
| Thumbnails and image previews | Needs a decode pipeline plus a thumbnail cache; pure polish. |
| Scheduled / automatic organisation | Automating a destructive action before the manual path has earned trust is backwards. |
| Background monitoring | Needs a tray/agent lifecycle on two platforms. |
| Custom rule engine | Ship the fixed category map first and see which rules users actually want. |
| Treemap visualisation | The folder roll-up list answers the question; a treemap is prettier, not clearer. |
| Clipboard image detection | Unrelated to the core loop. |
| Exportable reports | Trivial to add once the data model is stable. |
| Auto-update, signing, notarisation | Phase 5. Real work, but not needed to validate the product. |

**Definition of done for the MVP:** on a real Downloads folder of 10k+ files on both Windows
and macOS — scan completes without freezing the UI and can be cancelled; duplicates are found
without hashing files that cannot be duplicates; an organize plan applies and fully undoes; a
trash operation is recorded and recoverable; nothing outside the approved root is ever written.

---

## 9. Build order from here

| Phase | Deliverable |
|---|---|
| **2** | Shell, nav, `core/walk.rs` + `category.rs` + `identity.rs`, scan commands with progress and cancellation, Overview + Downloads screens. |
| **3** | ✅ `hash.rs`, `dup.rs`, `db.rs` hash cache, `suggest.rs`, folder roll-up; Duplicates + Storage screens; organize preview. |
| **4** | ✅ `fsops.rs`, `journal.rs`, the mutation guard; applying an organize plan, Cleanup queue, trash, undo, crash recovery. |
| **5** | ✅ Shortcuts with a `?` overlay, hash settings and cache controls, benchmark harness, performance pass on a 100k-file tree, NSIS installer. **Not done: macOS verification** — every Mac-specific path is written but has never been compiled or run on a Mac, and signing / notarisation is untouched. |

Phase 2 is the next step and starts with `core/`, because the walk and the type model are what
every later phase depends on.
