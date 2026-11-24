# Cleanup

An intelligent, local-only file organizer for Windows and macOS, focused on the
Downloads folder.

Architecture and the reasoning behind it: [ARCHITECTURE.md](ARCHITECTURE.md).

---

## Prerequisites

| Requirement | Why |
|---|---|
| **Node 18+** | Frontend build (Vite). |
| **Rust (stable-msvc on Windows)** | The scanner and all file logic. |
| **MSVC C++ Build Tools** (Windows) | Rust's `msvc` target links with `link.exe`. |
| **Xcode Command Line Tools** (macOS) | `xcode-select --install`. |
| **WebView2 Runtime** (Windows) | Preinstalled on Windows 11 and current Windows 10. |

On Windows, the MSVC install needs an elevated prompt:

```powershell
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--quiet --wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

Then confirm Rust is on the MSVC toolchain:

```bash
rustup default stable-msvc
```

## Running

```bash
npm install
```

```bash
npm run dev
```

The first `npm run dev` compiles the Rust side and takes a few minutes; after
that it is incremental. Vite serves the UI on port 1420 with hot reload, and
`tauri dev` restarts the backend when Rust changes.

## Building a distributable

```bash
npm run build
```

Produces an NSIS installer on Windows and a `.dmg` / `.app` on macOS, in
`src-tauri/target/release/bundle/`.

Shipping to other people additionally needs code signing — an Authenticode
certificate on Windows, and signing plus notarisation on macOS, without which
the app will not open on another Mac. That is Phase 5 work.

## Tests

```bash
cd src-tauri && cargo test --lib --no-default-features
```

`--no-default-features` turns off the `app` feature, which drops Tauri, the
webview and the window. The suite then compiles and runs in a couple of
seconds, and — more usefully — "`core` must never depend on `tauri`" becomes
something the compiler enforces rather than a rule a reviewer has to police.

122 tests, covering the parts that could lose someone's data, against real
temporary directories:

- symlinks recorded but never traversed, and a symlink loop that must not hang
- the depth cap truncating instead of descending forever
- the path guard rejecting volume roots, system directories, and a symlink that
  resolves into one
- `path_of` reconstructing a path that genuinely exists on disk
- in-progress downloads flagged and excluded
- duplicate candidates excluding zero-byte files and hardlinked identities
- filename handling across NFC and NFD so macOS and Windows agree
- settings surviving a corrupt, partial or hand-edited file
- an in-progress download being readable (classify its header) but never
  touchable (move, trash, hash)
- identical contents hashing the same under different names, and one byte of
  difference changing the hash
- files sharing a 64 KB prefix being separated by the full pass, not guessed at
- hardlinked paths reported as one file, never as savings
- the hash cache invalidating on a changed size or modification time
- the second dedupe run reading nothing, served entirely from the cache
- copy markers (`(1)`, `- Copy`, ` copy 2`) detected without firing on
  `copywriting-notes.pdf`
- the organizer never proposing an overwrite, and counting past an already-taken
  `(2)` to the first free name
- the mutation guard refusing a file outside the approved roots, a symlink
  inside one, a destination whose parent is a symlink pointing elsewhere, and a
  `..` that climbs out
- moving a file never overwriting what is already at the destination
- undo putting a file back, and refusing when something reoccupied the spot
- a crash mid-batch leaving an unfinished operation that recovery settles to
  `partial` or `failed` based on what actually landed
- paths surviving the journal byte for byte, including names a lossy text
  column would mangle

```bash
npm run typecheck
```

## Performance

Measured on a real 85k-file tree (100k written, the depth cap stopping at 8
levels), Windows, release build:

| Stage | Time |
|---|---|
| Metadata walk | 1.4 s (85k files, 90 MB, 103 folders) |
| Summary | 13 ms |
| Folder roll-up | 1 ms |
| Search query | 3.7 ms |
| Sort by name | 18 ms |
| Duplicate candidates (size grouping) | 3 ms |

Reproduce it yourself:

```bash
node scripts/make-test-tree.mjs C:/ct-bench 100000
```

```bash
cd src-tauri && cargo run --release --example bench --no-default-features -- C:/ct-bench
```

The benchmark found one real problem. Sorting by name took **164 ms per
query**, because the folded name used for comparison was being recomputed
inside the comparator — two string allocations per comparison, so O(n log n)
allocations. Two fixes took it to 18 ms: decorate-sort-undecorate, and caching
the folded name on each entry (the walk already computes it for the noise
checks, so storing it is nearly free). Search dropped from 6.5 ms to 3.7 ms as
a side effect.

## Troubleshooting

**The window shows "localhost refused to connect"** — the binary was built with
`cargo build` instead of the Tauri CLI. `cargo build --release` is *not* enough:
the dev/production decision is made by the CLI, which sets the environment
`tauri-build` reads. A plain cargo build keeps pointing the window at `devUrl`,
so it needs the Vite dev server running and shows that error without it.

Always build the app with:

```bash
npx tauri build --no-bundle
```

To check which you have, start Vite on 1420 and launch the app: a production
build ignores the port entirely, a dev build connects to it.

**`link.exe not found`** — the MSVC Build Tools step above has not been done, or
was installed without the *Desktop development with C++* workload.

**`could not execute process … (os error 2)` on a build script** — the project
is checked out somewhere deep. Windows still imposes a 260-character path
limit on process creation, and `target/debug/build/<crate>-<hash>/build-script-build`
is long. Either move the checkout nearer the drive root, or point the build
somewhere short:

```bash
CARGO_TARGET_DIR=C:/ct cargo build
```

## Generating icons

`src-tauri/icons/` is generated, with no image dependency:

```bash
node scripts/make-icons.mjs
```

Once there is real artwork at `src-tauri/app-icon.png`, use the official
pipeline instead:

```bash
npm run icons
```

## Layout

```
src/                 React + TypeScript
  app/               nav, keyboard shortcuts, design tokens
  ipc/               the only place that calls invoke() - types mirror Rust DTOs
  state/             zustand stores
  features/          one folder per screen
  components/        shared UI
src-tauri/
  src/core/          all logic, zero Tauri dependencies, unit-tested
  src/commands/      thin IPC adapters
  src/state.rs       the in-memory scan index
```

## What works today (Phases 2–4)

- Platform-correct Downloads detection (`SHGetKnownFolderPath` on Windows, not a
  string-built path), plus any folder you add
- Parallel metadata scan with live progress, cancellation, and per-path error
  reporting that never aborts the walk
- Categorisation into nine types from an extension table, with a 32-byte header
  read for extension-less files
- Overview: totals, size breakdown by type, and cleanup signals that always
  state the reason rather than a verdict
- Downloads: virtualised table over the full index, fetched 200 rows at a time,
  with search, type, size and age filters and sorting
- Settings: approved folders, scan depth, hidden files, stale threshold, theme
- **Duplicates**: the four-stage pipeline — exact size, then filesystem
  identity, then a 64 KB head hash, then a full BLAKE3 hash only for what
  survives — with a keep suggestion that states its reason, and a hash cache
  keyed on file identity so a second run re-reads nothing
- **Storage**: folder sizes rolled up from the inside out, largest files, files
  untouched for a long time, temporary files, breakdown by type, empty folders
- **Organize preview**: how loose files would be sorted into folders, with real
  resolved destinations including the renames a collision would force

- **Cleanup queue**: tick files on any screen, review every full path and the
  total, then move them to the Recycle Bin behind a confirmation
- **Apply an organize plan**, with every move journalled before it happens
- **Undo**, with operation history showing exactly what each run did
- **Crash recovery**: an operation interrupted by a crash is settled on next
  launch and reported, rather than leaving a half-sorted folder unexplained

- **Keyboard shortcuts** with a `?` overlay that reads from the same list the
  key handler uses, so a shortcut cannot exist without being discoverable
- **Hash settings and cache controls** in Settings
- A **benchmark harness** and an **installer**

The one thing still genuinely unproven is macOS: all the Mac-specific code is
written and reasoned about, but has never been compiled or run on a Mac.

## Safety

- There is no permanent-delete code path anywhere in the codebase.
- The webview holds **no filesystem permissions** (see
  `src-tauri/capabilities/default.json`). Every read goes through a Rust command.
- Nothing outside a folder you explicitly approved is ever read or written.
  Scanning a folder is what approves it; removing it in Settings withdraws that.
- Volume roots and system/application directories are refused, after
  canonicalisation, so a symlink cannot be used to escape into one.
- Symlinks, junctions, macOS app bundles, cloud placeholders and in-progress
  downloads are flagged and excluded from any operation.
- OneDrive and iCloud placeholder files are never read — doing so would download
  them in full over the user's connection.
- There is no network code at all. No HTTP client appears in
  `src-tauri/Cargo.toml`, which makes "analysis stays local" checkable rather
  than a promise.
