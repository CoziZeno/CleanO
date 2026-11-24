Cleanup finds what is filling your disk and helps you clear it without
guesswork. Everything runs locally — nothing is uploaded, and nothing is
deleted without your say-so.

### Downloads

| Platform | File |
| --- | --- |
| Windows 10/11 (64-bit) | `Cleanup_1.0.0_x64-setup.exe` |
| macOS 11+ (Apple Silicon and Intel) | `Cleanup_1.0.0_universal.dmg` |

### First launch

These builds are **not code-signed**, so both systems will warn you about an
unknown developer.

- **Windows** — SmartScreen shows "Windows protected your PC". Click **More
  info**, then **Run anyway**.
- **macOS** — right-click (or Control-click) the app in Applications and
  choose **Open**, then confirm. Double-clicking the first time will refuse.
  You only need to do this once.

### What it does

- **Quick Clean** — clears the caches Windows rebuilds anyway, with every
  folder and size shown before anything is deleted.
- **Storage map** — a treemap of where the space actually went. Click to
  drill in.
- **Duplicates** — verified by content hash, not by name or size, so a
  reported saving is a real one.
- **Swipe Clean** — review files one at a time, keep or bin, with your
  decisions remembered between sessions.
- **Drives** — scan a whole drive. System and application folders are always
  skipped.
- **Undo** — every move and delete is journalled to disk before it happens.

### Known limitations

- macOS is built and compiled in CI but has not been run through a manual
  test pass. Treat this release as unproven on macOS.
- Previews cover common image formats plus camera RAW (CR2, NEF, ARW, DNG,
  RAF, SRW, PEF, read from the preview the camera embeds). HEIC and CR3 are
  not supported.
- Quick Clean's browser cache targets cover each browser's default profile
  only.
- Some Quick Clean targets need the app to be running as administrator;
  without it they report what was skipped rather than failing.
