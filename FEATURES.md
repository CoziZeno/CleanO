# Cleanup — every feature, and how to test it

A checklist you can work through. Everything listed here is implemented and
working; anything deliberately absent is in the last section so you don't go
hunting for it.

**Nothing moves or deletes without you confirming it.** There is no
permanent-delete code path in the project — deletion always goes to the
Recycle Bin, and moves are undoable.

---

## 0. Quick Clean

Top of the home screen. Measures the well-known junk caches as soon as you
open the app, so the button already says how much there is to free.

**This is the only part of Cleanup that deletes permanently.** Caches do not
go to the Recycle Bin — sending a hundred thousand temp files there would free
no space at all, which is the entire point. The folders and their exact sizes
are on screen before anything happens, and the confirmation says so plainly.

The target list is **closed and hardcoded**. Every path is resolved from the
operating system's own environment, and only a target *key* crosses the IPC
boundary — never a path. That is what makes an exception to the system-folder
denylist acceptable here: it is not a general bypass, it is a fixed list of
folders whose purpose is to be disposable.

Ticked by default (no downside):

- **Your temporary files** — `%TEMP%`
- **System temporary files** — `%SystemRoot%\Temp` *(administrator)*
- **Windows Update downloads** — already-installed update packages *(administrator)*
- **Delivery Optimization cache** *(administrator)*
- **Crash dumps** and **Error reports**

Off by default, each with its downside stated:

- **Recycle Bin** — often the biggest single win, but emptying it is permanent
  and it also discards anything Cleanup trashed, so those operations can no
  longer be undone
- **Prefetch** — your next few app launches are slower while Windows rebuilds it
- **Chrome / Edge / Brave cache** — default profile only; close the browser
  first or most of it is locked. Cookies and passwords are never touched, so
  you stay signed in
- **Thumbnail cache** — Explorer mostly keeps these locked, so expect them to
  be skipped

Rules it follows:

- The target folder itself is never removed, only its contents.
- Links are never followed. A junction inside a temp folder is removed as a
  link, never traversed into.
- A file written in the last minute is left alone — something is still using it.
- A file that cannot be deleted is skipped and counted, never reported as
  freed. "347 in use, left alone" is the honest answer for `%TEMP%`.
- Without administrator rights the admin targets report what was refused
  rather than silently contributing nothing.

To test: note the total, click through, and check the freed figure against
what Explorer says about the drive. Run it twice — the second run should find
almost nothing.

## 1. Scanning

| Feature | How to see it |
|---|---|
| Finds your Downloads folder automatically | Overview → **Scan Downloads**. The path under the button is what it found. |
| Switch folders from the toolbar | The folder button at the top right lists everything you've approved, plus **Choose another folder…**. No trip through Settings. |
| Depth 20 by default | Covers unpacked archives and project trees. If your settings file still had the old 8 and you never changed it, it is raised automatically; a depth you chose yourself is left alone. |
| Live progress | Watch the counts and the folder path during a scan. |
| Cancel mid-scan | Press **Esc** or **Stop**. Partial results are kept, with a banner saying so. |
| Never freezes | Type, switch tabs and navigate while a scan runs. |
| Unreadable files don't abort it | If anything is locked or permission-denied, you get a count and an example, not a failure. |
| Depth limit | Settings → *Folder depth*. A deep tree shows a "not fully explored" banner. |

**Guards you can verify.** Symlinks and Windows junctions are listed but never
followed into. A junction pointing back up its own tree will not loop. macOS
`.app` bundles count as one item, not ten thousand. `.DS_Store`, `Thumbs.db`
and `desktop.ini` are filtered out entirely.

**Detected and flagged, never acted on:**
- Cloud placeholders (OneDrive / iCloud) — reading one would download the whole
  file, so they are counted and left alone
- In-progress downloads (`.crdownload`, `.part`, `.download`, and anything
  modified in the last 30 seconds)

---

## 2. Categorisation

Nine types — **Images, Videos, Audio, Documents, Archives, Installers,
Development, Temporary, Other** — from roughly 400 extensions.

Files with **no extension** get classified by reading their first 32 bytes.
Drop an extensionless PDF or PNG in the folder and it will still land in the
right category. (Settings → *Identify files with no extension* turns this off.)

---

## 3. Overview

- **Four stat cards**: files, total size, possible duplicates, empty folders
- **What's in there** — size breakdown bar; click any legend row to jump to
  Downloads filtered to that type
- **Worth a look** — findings that always state *why*, never a verdict:
  - files sharing an exact size (a hint, not proof)
  - your largest files
  - files untouched for N+ days
  - temporary and leftover files
  - cloud files and in-progress downloads (informational, not actionable)
- Banners for a cancelled scan, a truncated scan, unreadable items, and a
  previous crash

Age alone never produces a recommendation — it is always paired with another
signal.

---

## 3b. Progress worth feeling

A cleanup tool that only shows what's *left* is exhausting. So:

- **After every cleanup** — an animated count of what you just freed, with a
  note if it pushed you past a milestone (100 MB, 1 GB, 5 GB, 10 GB…)
- **On the Overview** — a running total of everything cleared, with how far to
  the next milestone

Both come from the operation journal, so they're real. **Undo something and
the total goes back down** — a number you didn't earn would be worthless.

## 3c. Refreshing

The index is rebuilt automatically after a delete, an organize, or an undo, so
files you just removed stop appearing. You can also rescan any time with **R**
or the Rescan button.

## 3d. Image previews

Thumbnails appear in the file list, in duplicate groups and in the cleanup
queue, so you can tell what something is without opening it.

- **PNG, JPEG, GIF, BMP, WebP, TIFF, ICO.** Not HEIC — iPhone photos need a
  HEIF decoder, which would mean a native dependency.
- **Only decoded for rows on screen**, a couple of dozen at a time. Scrolling
  past 10,000 files does not decode 10,000 images.
- **Remembered afterwards**, keyed to each file's identity, so scrolling back
  is instant and a rename doesn't throw the work away.
- **Cloud files are never opened** for a preview — that would download them in
  full. Hover the placeholder and it tells you why.
- Transparency is preserved on a checkerboard rather than flattened onto an
  invented background.
- Turn them off, and clear the stored previews, in Settings → Previews.

Worth checking: duplicate groups of images, where picking which copy to keep
by filename alone is the thing that's genuinely hard.

## 3e. Camera RAW previews

CR2, NEF, ARW, DNG, RAF, SRW and PEF show previews in the file list, Swipe
Clean and the storage map.

The sensor data is **not** decoded - that would mean a demosaicing pipeline and
a colour profile per camera body. These formats are TIFF containers that
already hold a full-size JPEG preview, put there by the camera for its own
screen, and that is what gets extracted. It is how every fast RAW viewer works,
and it is instant.

CR3 is absent: it is an ISO media container, not TIFF, so the same parser has
nothing to extract. A RAW file whose camera wrote no preview reports that there
is none rather than showing a guess.

To test: put a CR2 next to a JPEG in a folder and scan. Both should show a
thumbnail, and the CR2 should be categorised as an image.

## 4. Downloads

| Feature | Notes |
|---|---|
| Virtualised table | Handles 100k files; only ~200 rows are fetched at a time |
| Search | Case-insensitive, and handles accented names across Windows/macOS encodings |
| Type chips | With live counts |
| Size filter | Any / >1 MB / >10 MB / >100 MB / >1 GB |
| Age filter | Last 7 / 30 days, older than 6 months / a year |
| Sort | Name, Type, Modified, Size — click a header, click again to reverse |
| Reveal | Opens the file's folder in Explorer/Finder |
| Tick to queue | Checkbox per row; untouchable files are disabled with the reason on hover |
| **Shift-click** | Selects a range, like any file manager |
| **Select all N** | In the results bar — selects every match across the whole filter, not just the loaded page, and skips files that can't be touched |
| **Delete from anywhere** | A bar appears at the bottom whenever something is selected, with the count, total size and a direct **Move to Recycle Bin** — no need to visit the queue |

**Organize preview** sits above the table when you have loose files. Expand a
folder to see every `file → Images/file` line, including the exact rename a
collision would force. **Sort N files** applies it.

Four ways to sort, switchable in the preview:

| Mode | Produces |
|---|---|
| **By type** | `Images/`, `Documents/`, `Installers/`… |
| **By year** | `2024/`, `2025/` |
| **By month** | `2024/March/` |
| **By extension** | `PNG/`, `PDF/`, `ZIP/` |

Dates come from when the file was created, falling back to when it changed;
anything with no usable date goes to `Undated/` rather than being silently
skipped. Folder names for sorting by type are editable in Settings.

**Move to folder…** — on the selection bar, on any screen. Pick files, pick a
destination, and they move there. Same guard, same journal, same undo as
everything else: the destination still has to be inside a folder you approved,
so picking one in a dialog does not grant access to it.

---

## 5. Duplicates

Compares *contents*, not names. Four stages, cheapest first:

1. Exact size — groups of one are discarded having read **zero bytes**
2. Filesystem identity — hardlinks collapse instead of being offered as savings
3. First 64 KB hash — one block read per file
4. Full BLAKE3 hash — only for what survives

**To test:** copy a file, rename the copy, run the check. Then run it again —
the second run should report everything served from the cache and nothing
re-read.

Each group shows how many copies, size each, what's recoverable, and a
**suggested keep with its reason** (no copy marker in the name / filed in a
subfolder / oldest). Nothing is pre-ticked. **Pick extra copies** selects
everything except the suggested keep.

Settings → *Also compare byte for byte* adds a full re-read and comparison on
top of the hash, if you want certainty over near-certainty.

---

## 6. Storage

- **Largest folders**, rolled up recursively with proportional bars
- **Files worth a look** — segmented: Largest / Untouched / Temporary
- **By type** — count and size per category
- **Empty folders** — including ones holding nothing but system clutter

On Windows it tells you "untouched" is measured from last *change*, because
Windows doesn't record when files are opened.

---

## 6b. Installed Apps

Everything installed on the machine, from the system's own records — name,
version, publisher, install date and the size the installer claimed.

Search it, sort by name / size / install date, and **Refresh** to re-read.

**Cleanup never uninstalls anything.** This is read-only; removing software is
the operating system's job.

### Installers you already have

The reason this screen exists. Installers in your scanned folder are matched
against what's installed, so instead of *"this setup.exe is 8 months old"* you
get:

> **Firefox Setup 121.0.exe** — you have Mozilla Firefox 133.0 installed, this
> installer is 121.0.

That's a fact rather than a guess about age. Versions compare numerically, so
1.10 is correctly newer than 1.9 — getting that backwards would recommend
deleting the *newer* installer.

**It refuses rather than guesses.** If a filename matches two products equally
well on the same words, those words identify a vendor rather than a product, so
nothing is claimed — `microsoft-setup.exe` will not tell you it's Edge. A
missed match costs you nothing; a wrong one would tell you that you already
have software you don't.

Nothing is ticked for you, and both names are shown so you can check the
pairing.

*Windows reads the registry; MSIX/Store apps are not covered. macOS reads
`/Applications` — written but never run on a Mac.*

## 6c. Storage map (treemap)

Storage -> **Storage map**. Every rectangle is sized by the space it uses, so a
single huge file is a single huge box.

- Folders are outlined in the accent colour; files take their category colour,
  the same one used everywhere else in the app.
- **Click a folder** to go inside it. The breadcrumb above the map walks back
  out; the first crumb is the scan root.
- **Hover** anything for its name, size and what it is, in the line under the
  map.
- **Right-click a file** for the same menu as the file list - open, reveal,
  move, rename, trash.
- Items too small to draw are grouped into one grey "N smaller items"
  rectangle rather than dropped, so a level always adds up to the folder's
  real size.

To test: scan a folder with a deep tree. Check that the biggest box matches
the biggest thing in Largest folders, drill two levels down and back out via
the breadcrumb, and confirm the rectangle sizes look proportional.

## 6d. Drives

Settings -> **Drives on this computer**, and on the Storage screen before any
scan. Lists every attached drive with its label, type, and how much of it is
used.

- Scanning a whole drive is **not** available from the folder picker - the
  safety guard refuses volume roots there on purpose. It has to be chosen here
  by name, and it asks for confirmation first.
- The walk skips system and application folders (`C:\Windows`, Program Files,
  your AppData) even when scanning a whole drive, so "scan this drive" means
  your own files on it.
- A drive with no Recycle Bin says so, because deleting there is permanent.
- A drive whose size cannot be read (empty optical drive, unreachable share)
  says "size unavailable" rather than showing 0.

To test: open Settings, confirm your drives and their used/free figures match
File Explorer. Scan one, then check that nothing under `C:\Windows` appears in
the results.

## 7. Cleanup Queue

- Collects what you tick anywhere in the app (count shows in the sidebar)
- Shows the **full path** of every file and the total size
- **Move to Recycle Bin** behind a two-step confirmation
- Remove individual items, or clear the lot
- **History** of every operation: what it did, how many succeeded or failed,
  bytes involved — expandable to per-file detail with errors
- **Undo** per operation

**Honest limitation:** undo of a *trash* operation won't restore from the
Recycle Bin, because that can't be done reliably and the bin may have been
emptied. It lists the original paths and tells you to restore them yourself.
Undo of a *move* is exact, and refuses if something has reoccupied the spot.

**Crash recovery:** kill the app mid-operation and relaunch — you get a banner
explaining what had already happened, and the history shows the truth.

---

## 8. Settings

- **Folders Cleanup may read** — the whole of its access. Removing one
  withdraws permission to modify anything inside it.
- Scan: folder depth, include hidden files, identify extensionless files
- Stale threshold in days
- Comparing contents: files read at once, first chunk size, byte-for-byte verify
- Hash cache: how many are remembered, and a Clear button
- Dark / light theme

---

## 8b. Excluding folders

Settings -> **Folders Cleanup must never touch**.

A folder listed here is skipped entirely: never scanned, never indexed, and
refused by every move, rename and delete - even when it sits inside a folder
you approved. It is matched by whole path, so excluding one `Unity` folder on
E: does not hide every folder called Unity anywhere, and excluding `E:\Unity`
leaves `E:\Unity-Backup` alone.

This is different from the `ignore` list in `settings.json`, which matches
file *names* anywhere in the tree.

To test: exclude a folder, rescan, and confirm nothing from it appears and its
bytes are not counted. Then try an operation on a file that was indexed before
you excluded it - it should be refused.

## 8c. Choosing formats for Swipe Clean

Settings -> **Swipe Clean**.

- Pick which kinds of file reach the queue. The list order is the order you
  are shown them in; use the arrows to reorder.
- **Only these formats** narrows further by extension (`cr2, nef, jpg`). It
  filters *within* the kinds you picked - it cannot add one you left out.
- **Skip anything smaller than** keeps trivial files out of a one-at-a-time
  review.
- Documents are off by default: a Downloads folder is usually full of small
  PDFs, and burying the photos behind them makes the queue a chore. One click
  adds them.

The Swipe screen shows the active filter at the top with a Change link, and an
empty queue says whether everything was reviewed or nothing matched the
filter.

To test: set the filter to images only with `cr2` as the extension, then open
Swipe Clean and confirm only CR2 files appear.

## 9. Keyboard

| Key | Does |
|---|---|
| `1`–`6` | Jump to a section |
| `/` | Search |
| `R` | Scan again |
| `Ctrl+Z` | Undo the last operation |
| `?` | Shortcut list |
| `Esc` | Stop a scan, close the overlay, leave the search box |

---

## 10. Safety, if you want to try to break it

- Add a folder, scan it, then **remove it in Settings** — operations on files
  inside it are now refused
- Point it at `C:\Windows` or a drive root — refused, with an explanation
- A symlink inside an approved folder — listed, never moved or trashed
- Organize twice onto the same name — the second becomes `name (2).ext`;
  nothing is ever overwritten
- A network or removable drive — trashing is **refused** rather than
  permanently deleting, because those volumes have no Recycle Bin

There is no network code in the app at all — you can check `src-tauri/Cargo.toml`
for the absence of any HTTP client. The window itself holds zero filesystem
permissions; every read goes through a Rust command that checks your approved
folder list first.

---

## Deliberately not built

So you don't test for them:

- **Near-duplicate / similar image matching** — only exact duplicates
- **Scheduled or automatic organisation**
- **Background monitoring / tray mode**
- **Custom organisation rules** — the category→folder names are editable in
  `settings.json`, but there's no rule builder
- **Clipboard image detection**
- **Exportable reports**
- **Auto-update**

## Not verified

- **macOS.** Every Mac-specific path is written — TCC permission prompts,
  `.app` bundles, Trash, NFD filenames — but none has been compiled or run on a
  Mac. Treat that half as unproven.
- **Code signing / notarisation.** The installer will trip SmartScreen on
  another machine.
