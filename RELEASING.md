# Releasing Cleanup

## The one thing that constrains everything

**macOS cannot be cross-compiled.** Not from Windows, not from Linux, not with
any flag. Building a `.app` or `.dmg` requires Xcode and the macOS SDK, which
only run on macOS. This is Apple's constraint, not Tauri's, and there is no way
around it.

So there are exactly two ways to get a macOS build:

1. A macOS runner in CI — already set up in `.github/workflows/release.yml`.
   This is the recommended path and needs no Mac of your own.
2. A Mac you have access to, running `npm run tauri build` directly.

The Windows build has no such constraint and is produced locally.

## Cutting a release

```bash
git tag v1.0.0
git push origin v1.0.0
```

That triggers `.github/workflows/release.yml`, which:

1. Runs the core test suite and type-checks the frontend on Linux (cheap gate).
2. Builds on `windows-latest` and `macos-14` in parallel.
3. Creates a **draft** GitHub Release with the installers attached.

The release is a draft on purpose — nothing is downloadable until you open the
Releases page and publish it. Read the notes and check the files first.

To test a build without tagging: Actions → Release → Run workflow. Same build,
no release; installers are attached to the run as artifacts.

Keep the tag and the version in sync. Three files carry the version number:

- `package.json`
- `src-tauri/Cargo.toml`
- `src-tauri/tauri.conf.json`

## What gets produced

| Platform | File | Notes |
| --- | --- | --- |
| Windows 10/11 x64 | `Cleanup_1.0.0_x64-setup.exe` | NSIS. Installs per-user, no admin prompt. |
| Windows 10/11 x64 | `Cleanup_1.0.0_x64_en-US.msi` | For Group Policy / managed deployment. |
| macOS 11+ | `Cleanup_1.0.0_universal.dmg` | Universal: one file for Apple Silicon and Intel. |

## Building Windows locally

```bash
npm ci
npm run tauri build
```

Output lands in `src-tauri/target/release/bundle/`. On this machine the build
uses `CARGO_TARGET_DIR=C:/ct2` to stay clear of Windows' `MAX_PATH` limit —
without it the build scripts fail with a confusing `os error 2`.

## Code signing

The builds are currently **unsigned**. They work, but both operating systems
warn the person installing them, which costs you a meaningful share of anyone
who was going to try the app.

### What an unsigned build looks like to someone downloading it

- **Windows** — SmartScreen shows a full-screen "Windows protected your PC".
  The Run button is hidden behind **More info**. This warning fades over time
  as more people install without incident, but a brand-new unsigned binary gets
  it every time.
- **macOS** — Gatekeeper refuses to open it on a double-click. The workaround
  is right-click → Open → Open, once. Users who do not know this will assume
  the app is broken.

### Fixing it on Windows

You need an **OV or EV code-signing certificate** (roughly $200–500/year from
Sectigo, DigiCert, SSL.com and others). EV clears SmartScreen immediately; OV
builds reputation over time.

Two routes:

- **Azure Trusted Signing** — cheapest and easiest now (about $10/month, no
  hardware token). Set `bundle.windows.signCommand` in `tauri.conf.json` to
  call `azuresigntool`.
- **A certificate file** — import the `.pfx` into the runner's certificate
  store in a CI step, then set `bundle.windows.certificateThumbprint`.

Never commit the certificate. `.gitignore` already blocks `*.pfx`, `*.p12`
and `*.key`.

### Fixing it on macOS

You need an **Apple Developer Program membership** ($99/year) and a
"Developer ID Application" certificate.

**Two steps, not one.** Add the six secrets in Settings → Secrets and variables
→ Actions, *and* uncomment the `APPLE_*` lines in
`.github/workflows/release.yml`. They are commented out on purpose: GitHub
substitutes an empty string for a secret that does not exist rather than
leaving the variable unset, and Tauri reads "`APPLE_CERTIFICATE` is defined"
as "sign this build". An empty value therefore fails the bundle step with
`SecKeychainItemImport: Unable to decode the provided data`, which is a
confusing way to learn you have no certificate.

The secrets:

| Secret | What it is |
| --- | --- |
| `APPLE_CERTIFICATE` | Base64 of your Developer ID `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | Password for that `.p12` |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_ID` | Your Apple ID email |
| `APPLE_PASSWORD` | An app-specific password, not your real one |
| `APPLE_TEAM_ID` | Your 10-character team ID |

Notarisation adds a few minutes to the build. Once it passes, the app opens on
a normal double-click.

## Before you publish v1.0.0

- [ ] **Decide the licence.** `LICENSE` currently says "all rights reserved",
      which grants nothing away and is the safe default. If you want this to be
      open source, replace it with MIT or Apache-2.0 before the first public
      release — relicensing after people have copies is awkward.
- [ ] **Set the publisher name.** `bundle.publisher` and `bundle.copyright` in
      `tauri.conf.json` both say "Cleanup". Whatever you put there is what
      Windows shows in Programs and Features and what macOS shows in Get Info.
      It should be your name or your company's.
- [ ] **Consider the bundle identifier.** `dev.cleanup.app` is fine, but
      convention is a domain you control, reversed. Changing it after release
      makes existing installs look like a different app.
- [ ] **Run the macOS build at least once** before tagging, via the manual
      workflow. See the warning below.

## The macOS caveat, stated plainly

**It compiles.** As of 24 Nov 2026 the whole crate builds clean as a universal
binary on a `macos-15` runner — every macOS-specific path (TCC permission
handling, `.app` bundle treatment, Trash, NFD filename normalisation,
`Info.plist` parsing, the `/Volumes` drive listing) compiled on the first
attempt with no errors. That was the big unknown, and it is settled.

**It has still never been run.** Compiling proves the types line up, not that
the app works. Nobody has opened it on a Mac, granted it a TCC prompt, or
watched it move a file to the Trash. The paths most likely to be wrong are the
ones the compiler cannot check: whether the permission prompts appear at the
right moment, whether `.app` bundles are treated as single items rather than
folders, and whether NFD/NFC filename handling survives a round trip on APFS.

Someone needs to install it on a Mac and work through `FEATURES.md` before
macOS is advertised as supported.

## Auto-update

Not built. Tauri's updater needs a signing key pair and somewhere to host a
`latest.json` manifest; releases are manual for now. If you want it later,
`tauri signer generate` makes the key and the updater plugin reads a static
JSON file — GitHub Releases can host it.
