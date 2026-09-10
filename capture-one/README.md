# Capture One — abandoned experiment

**Status: not a product, not built, not shipped.** This directory is a partial
snapshot of an attempt to port the plugin to Capture One, stopped on 2026-09-10
part-way through the first implementation pass. It is kept as a record, because
the research behind it was expensive and the reconstructed SDK header below is
not trivially reproducible.

Read [`docs/wiki/Dev-Capture-One-Port-Plan.md`](../docs/wiki/Dev-Capture-One-Port-Plan.md)
first. It has the findings, the feasibility matrix for macOS vs. Windows, the
architecture this code was heading towards, and the open questions. This file
only describes what is actually on disk.

## Why it stopped

Two reasons, in order of weight:

1. Capture One's plugin SDK cannot write keywords, metadata, or anything else
   into the catalog. It hands a plugin a list of file paths and takes back a
   status. The core of `Analyze & Index` is outside its contract, so a
   Capture One "plugin" would really have been a companion application with its
   own window, plus a shim to launch it.
2. macOS and Windows would have been two different products. On macOS
   AppleScript covers nearly the whole Lightroom feature surface; on Windows
   there is no automation interface at all and the only way back into the
   catalog is an XMP sidecar the user has to tell Capture One to read.

Then the project decided not to pursue Capture One. That decision is the actual
reason this stopped where it did.

## What is here

| Path | State |
|---|---|
| `coplugin-macos/Headers/CaptureOnePlugins.h` | **The interesting part.** A reconstructed interface for the SDK, recovered from Objective-C runtime metadata in the shipped `CaptureOnePlugins.framework` and in a real third-party plugin. Selector names and type encodings are exact; what could not be recovered from metadata (enum names and values leave no trace in a binary) is marked `SDK-UNVERIFIED`. Verified against Capture One 16.8.5.30. |
| `coplugin-macos/Sources/LrGeniusAIPlugin.swift` | The `.coplugin` shim: collect the selected paths, hand them to the backend, mirror progress, surface errors. Compiles against the reconstructed header. |
| `coplugin-macos/Info.plist` | `CFBundlePackageType=BNDL`, `NSPrincipalClass`, the `COPlugin*` keys — mirrors what a real installed plugin declares. |
| `coplugin-macos/scripts/build.sh`, `notarize.sh` | Build and the manual notarisation dance (Xcode cannot notarise a bundle; `stapler staple` goes on the bundle, not the ZIP). |
| `coplugin-windows/LrGeniusAI.Plugin/Core/*.cs` | Eight support classes — HTTP handoff, fallback launcher, hand-rolled JSON (net472 has no `System.Text.Json`), photo id, sidecar naming rules, run report. |
| `scripts-macos/analyze_selection.applescript` | Scripts-menu entry point. Compiles cleanly with `osacompile`. |

## What is missing

The run was stopped mid-flight, so this is genuinely incomplete:

- `bridge/c1_bridge.js` — the JXA bridge, the piece that would have done all the
  actual catalog work on macOS. Never written.
- `cli/` — `lrgenius-c1`, the companion process with the `CatalogHost` trait,
  the `OsaHost`/`XmpHost` implementations and the XMP sidecar writer. Never written.
- `coplugin-windows/` — no `Plugin.cs` entry class, no `.csproj`, no
  `manifest.xml`, no packaging scripts. Only the support classes exist.
- `scripts-macos/` — only one of the three entry points, and no installer.
- Per-directory READMEs.

## Two caveats before anyone picks this up

**Nothing here has run against Capture One.** The macOS binaries built, and the
AppleScript compiles, but no write path was ever executed — the catalog open on
the development machine was empty. Every Windows statement is derived from
documentation and two public plugin repositories, never reproduced on Windows.

**`build/` and `dist/` are gitignored, and one reason is legal.** `build.sh`
stages a copy of Capture One's own `CaptureOnePlugins.framework` so the compiler
has something to link against. That binary is Phase One's property, taken out of
the locally installed app. It must not be committed or redistributed. The
reconstructed header in `Headers/` is our own work and is fine; the framework
binary is not.

The official SDK is behind a developer-portal signup at captureone.com, was last
released as 1.0.1 in March 2022, and has no public repository. Getting hold of
it legitimately is the first step for anyone continuing this, and the only way
to get a Windows build at all.
