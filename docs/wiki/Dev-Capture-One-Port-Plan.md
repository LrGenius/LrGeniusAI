# Capture One Port — Findings and Plan

> **Status: not being pursued. Decision taken 2026-09-10.** The page is kept
> because the expensive part of this work was finding out what Capture One can
> and cannot do, and that answer does not change often. If the question comes
> back, start here rather than re-running the investigation.
>
> A partial implementation snapshot was kept as well, under
> [`capture-one/`](../../capture-one/README.md) — most usefully a **reconstructed
> SDK header** recovered from Objective-C runtime metadata, since the official
> SDK is not publicly available. See that README for what exists and what does
> not; nothing in it has ever run against Capture One.

## Why this exists

The question was: build a Capture One plugin for Windows and macOS equivalent
to the Lightroom plugin, reusing the existing Rust backend.

The short answer is that Capture One has no mechanism equivalent to a Lightroom
plugin, and that macOS and Capture One on Windows are so far apart in what they
allow that they would have been two different products sharing a backend.

## Key finding

**The official Capture One Plugin SDK cannot do the one thing the product
needs.** It is a file-in/file-out interface: a plugin receives file paths, does
something, and reports a status. It has no access to the catalog, to metadata,
to keywords, to the selection, or to a window of its own. Writing an AI-generated
keyword back onto a photo — the core of `Analyze & Index` — is outside its
contract entirely.

What replaces "plugin" is **"companion process with its own UI"**, and the
control direction inverts: today Lightroom calls the backend; here the backend
would hold the UI and call the host.

The second finding is that **macOS and Windows are not the same product**:

| | macOS | Windows |
|---|---|---|
| Automation interface | AppleScript/JXA, deep and actively maintained | none — no scripting, no COM, no CLI |
| Write keywords, rating, IPTC | native | XMP sidecar + a user-triggered sync step |
| Develop settings | native, ~90 scriptable parameters | `.costyle` file the user applies by hand |
| Albums, selection, variants | native | not reachable |
| Menu entry without the SDK | yes, via the Scripts menu | no |

## Verified first-hand

Capture One 16.8.5.30 ("Capture One Studio (Trial)") is installed on the
development Mac and was running during this investigation. Everything in this
section was checked against the installed application, not taken from
documentation.

| Fact | How it was verified |
|---|---|
| Version 16.8.5.30, bundle id `com.captureone.captureone16` | `Info.plist` |
| AppleScript is enabled and answers | `osascript -e 'tell application "Capture One" to return {app version, name of current document, kind of current document, count of (get selected variants)}'` → `Capture One Studio (Trial) 16.8.5, Capture One Catalog, catalog, 0` |
| Scripting dictionary is large: 2212 lines, 2 suites, ~48 classes | `Contents/Resources/CaptureOne.sdef`, parsed with ElementTree |
| `.coplugin` is a **ZIP** containing a macOS bundle — `Contents/{Info.plist, MacOS/<exe>, Resources/, _CodeSignature/}` | unpacked the installed `COHeliconFocusPlugin.coplugin` |
| Plugin binaries link `@rpath/Frameworks/CaptureOnePlugins.framework` and declare `NSPrincipalClass` | `otool -L` + `plutil -p` on that plugin |
| Plugins are hosted **out-of-process over XPC** | `COPluginHostApple.xpc` / `COPluginHostIntel.xpc` in the app bundle |
| A 2022-era SDK 1.0 plugin still loads under 16.8.5 | file date on the installed Helicon plugin |
| The Scripts menu folder exists: `~/Library/Scripts/Capture One Scripts/` with `Background Scripts/` and `Disabled Scripts/` | filesystem |

### What the plugin API actually exposes

Extracted from the symbols in `CaptureOnePlugins.framework`:

```
COPluginBase, COPluginAction, COPluginTask, COFileHandlingPluginTask
COPluginActionResult
  COPluginActionOpenWithResult      (+ ...Status)
  COPluginActionPublishResult       (+ ...URL, ...Message)
  COPluginActionImageResult         (+ ...Images)
  COPluginActionColorProfilingResult(+ ...ColorProfiles)
COPluginTaskExecutingDocumentType   (Catalog | Session)
COSupportedFileFormats, COTaskDestinationFolder, COTaskTemporaryFolder
Settings UI, declarative only:
  COSettingsBase, COSettingsElement, COSettingsElementsGroup, COSettingsItemsGroup,
  COSettingsBoolItem, COSettingsButtonItem, COSettingsFileItem, COSettingsLabelItem,
  COSettingsListItem, COSettingsListOption, COSettingsMultipleListItem, COSettingsTextItem
Keys: COPluginActionDisplayNameKey, COPluginActionIdentifierKey,
      COFileHandlingPluginTaskFilesKey
```

There is no keyword, metadata, catalog, selection, or window API. The settings
form has no number field, no slider, no image, no table and no web view.

### What AppleScript exposes (macOS only)

This is the part that makes macOS viable. From the `.sdef`:

- `application` — `selected variants`, `primary variant`, `current document`
  (**PRO only**), `progress total units` / `progress completed units` /
  `progress text` (drives Capture One's own progress window), `available styles`,
  `edit all selected variants`, and event hooks including
  `selection changed script` and `processing done script`
- `variant` — writable `rating`, `color tag`, `pick`, `adjustments`, `styles`,
  `crop`, and the full IPTC set (`status title`, `content description`,
  `content headline`, `image city/state/country`, …); `keyword` and `layer` as
  elements. Note `selected` is **read-only** — but see `select` below.
- `image` — `path`, `dimensions`, `EXIF capture date` (writable), camera model,
  ISO, shutter speed, aperture, focal length
- `adjustment settings` — **90 properties**: `exposure`, `contrast`,
  `saturation`, `temperature`, `tint`, `clarity amount`, `dehaze amount`,
  `highlight recovery`, `shadow recovery`, `white recovery`, `black recovery`,
  `vignetting amount`, `sharpening amount`, `noise reduction luminance`,
  `black and white`, `rgb curve`, `color balance *`, …
- commands — `apply keyword`, `sync metadata`, `reload metadata`, `apply style`,
  `apply adjustments` / `copy adjustments`, **`select` / `deselect`** (so the
  selection *can* be set, just not via the `selected` property), `make` (albums),
  `add inside`, `process` (**PRO only**), `create people mask`, `autoadjust`,
  `clone variant`

Roughly: on macOS, AppleScript covers nearly the whole Lightroom feature surface.

## Feasibility matrix

**N** = native · **W** = workaround, with the named cost · **X** = impossible

| Capability the LR plugin relies on | C1 macOS | C1 Windows |
|---|---|---|
| Register a menu entry | **N** — Scripts menu (no SDK needed, supports keyboard shortcuts) *or* `.coplugin` | **W** — `.coplugin` only |
| Read the current selection | **N** | **W** — only the paths handed over when the plugin action fires |
| Read the active collection | **N** | **X** |
| Enumerate the whole catalog | **W** — AppleScript (slow) or read-only SQLite | **W** — read-only SQLite, or the user picks a folder |
| Read EXIF | **N** — or better, the backend decodes the original itself (`lrg-imaging`/`rawler`) | **N** — same |
| Write title / caption | **N** | **W** — XMP + user sync step |
| Assign keywords | **N** | **W** — XMP `dc:subject` + sync |
| Hierarchical keywords | **W** — unverified; C1 forbids `,` and `\|` in keyword names, so LR's `A\|B\|C` convention is out | **W** — `lr:hierarchicalSubject` in the sidecar |
| Rating / color tag | **N** | **W** — XMP `xmp:Rating` / `xmp:Label` |
| Custom per-photo fields (the 40 in `MetadataProvider.lua`) | **X** | **X** |
| Create and fill an album | **N** | **X** |
| Set the selection / jump the view | **N** | **X** |
| Read develop settings | **N** | **W** — parse `.cos` XML or the DB |
| Write develop settings | **N** | **W** — drop a `.costyle`, user applies it |
| AI masks | **W** — `create people mask` is real; subject/background unverified | **X** |
| Virtual copies | **N** — `clone variant` | **X** |
| Own dialog UI | **X** | **X** |
| Progress + cancel | **W** — can drive C1's own progress window | **W** — only while the plugin action runs |
| Undo | **X** | **X** |

## Mechanism ranking

Ordered by how far each one carries, not by elegance.

1. **Companion process with a browser UI (the Rust server itself)** — carries all
   analysis, all UI, all persistence, all provider/model management. Roughly
   80–90 % of the product, identical on both platforms. The pattern already
   exists and ships: `/v1/ui/people` + `ui_bridge.rs`.
2. **AppleScript/JXA via `osascript`** (macOS) — the entire catalog write-back.
   Caveats that matter: it needs TCC Apple-Events permission, which in practice
   forces the companion to be a signed `.app` rather than a bare CLI binary;
   `current document` and `process` are **PRO-gated**; Apple Events are expensive
   per call, so it must be one call per *batch*, never per photo; Capture One
   must be running with a document open.
3. **XMP sidecar + Capture One sync** — the only vendor-sanctioned metadata
   inbound path, and on Windows the only one at all. Carries keywords, rating,
   color label, IPTC core. Three sharp edges: Adobe naming (`IMG_0001.XMP`
   without the original extension) means a RAW+JPEG pair **shares one sidecar**
   and would silently merge; the "Auto Sync Sidecar XMP" preference defaults to
   `None`, so nothing happens until the user changes it to `Load`; `Full Sync`
   makes Capture One write the same file, so concurrent writes lose.
4. **The official `.coplugin` SDK** — carries exactly one thing: the entry point.
   Still unavoidable on Windows. It is effectively frozen: last release 1.0.1 on
   2022-03-14, developer forum inactive since 09/2024, **no public repository and
   no online documentation** — the download is behind a developer-portal signup.
   That signup is the single hard blocker for both `.coplugin` targets.
5. **Reading the catalog/session SQLite** — the full DDL (23 tables) ships as
   plain-text migration scripts inside `DataCore.framework`, so no reverse
   engineering is needed. But the schema migrates on nearly every point release,
   and the widely cited OSS examples describe the retired `ZSTACK` mechanism and
   no longer work on 16.8.
6. **Writing `.costyle`** — officially supported (drop the file in the styles
   folder, no import needed), trivial XML. On Windows the only route to AI Edit.
7. **Writing `.cos` sidecars** (sessions only) — the documented carrier for
   adjustments *and* metadata, but the folder name changes per release
   (`Settings1680` on 16.8.5, verified locally) and each variant has three layer
   rows where writing the wrong one gets overwritten on reload. Not a default path.
8. **Watch folder / export recipe hook** — a trigger only, no way back into the
   catalog. Of little use since we want to analyse originals, not derivatives.
9. **Reading the preview cache** (`.cop`/JPEG-XL, `.cot`/JPEG) — undocumented
   internal format. An optimisation at best; `lrg-imaging` already decodes RAW.
10. **UI automation (AutoHotkey and similar)** — carries nothing reliable.
    Explicitly: do not build this.

Not available: Capture One "Actions" (16.8) are cloud connectors, centrally
administered, Studio-for-Teams/Enterprise only, with no public connector API.

## The architecture that was going to be built

```
User selects in Capture One  →  right-click → Edit With → LrGeniusAI
   → .coplugin (Swift / C#) collects the file paths
   → POST 127.0.0.1:19819/v1/host/handoff  {paths, document_type, host}
   → companion window comes forward, user picks the tasks
   → backend indexes via /v1/index/photos/by-path (reads the originals itself)
   → write-back:
        macOS : osascript -l JavaScript bridge.js   (one call per batch)
        Win   : <basename>.XMP  →  user runs "Load Metadata"
```

Planned artefacts — `✓` exists in `capture-one/`, `✗` never written:

```
capture-one/
  coplugin-macos/Headers/      ✓ reconstructed SDK interface (see below)
  coplugin-macos/Sources/      ✓ Swift shim + Info.plist + build/notarize scripts
  coplugin-windows/…/Core/     ~ eight support classes only; no entry class,
                                 no .csproj, no manifest, no packaging
  scripts-macos/               ~ one of three entry points, no installer
  bridge/c1_bridge.js          ✗ JXA bridge — the piece that would have done all
                                 the real catalog work on macOS
  cli/                         ✗ lrgenius-c1: CatalogHost trait, OsaHost, XmpHost,
                                 XMP sidecar writer
```

### The reconstructed SDK header

Worth knowing about even if Capture One never comes back: because the official
SDK is not publicly downloadable, `coplugin-macos/Headers/CaptureOnePlugins.h`
was rebuilt from Objective-C runtime metadata in two binaries already on the
machine — the shipped `CaptureOnePlugins.framework`, and an installed
third-party plugin. The second one matters: ObjC protocols are emitted into
whichever binary *uses* them, so a real plugin's binary still carries the full
text of `COOpenWithPlugin`, `COEditingPlugin`, `COSettings`, `COFileHandling`,
`COVariantProcessing` and `COActionSettings`, including method type encodings.
Selector names and encodings are therefore exact; enum names and values are not
recoverable from a binary and are marked `SDK-UNVERIFIED`.

The framework binary itself is Phase One's property and is **gitignored** — it is
staged into `build/` from the locally installed app at build time only.

Backend work it would have required (none of it done):

1. **Host-neutral photo identity.** Today's `meta1:` id is an MD5 over eleven
   values, four of which are *formatted Lightroom display strings*
   (`"1/125 s"`, `"f/2.8"`, `"70 mm"`, `"ISO 400"`). No other host reproduces
   those byte-for-byte. The `md5p:` fallback is genuinely file-based but includes
   `mtime`, so it does not survive a backup/restore. Proposed replacement:
   `file1:` = MD5 over `<size>:<first 4 MiB>:<last 4 MiB>`, no mtime.
2. **Separate the training corpus per host.** `POST /v1/edit/training` expects
   Lightroom SDK keys and crosswalks them through `LR_TO_CANONICAL` in
   `lrg-analysis/src/training.rs`. Capture One's controls are named differently
   *and scale differently* — mixing them would corrupt the existing style profile.
3. **Edit-recipe scale and prompt profile.** `temperature` is Kelvin for RAW and
   −100..100 otherwise (a Lightroom convention); `prompts.rs` says "senior
   Lightroom Classic retoucher" verbatim and the schema is `lightroom_edit_recipe`.
4. **Generalise `/update/apply`**, which today patches `.lua` files into the
   `.lrdevplugin` folder via the `plugin_files` manifest key.
5. **`GET /v1/host/capabilities`**, so the UI can explain what a host cannot do
   instead of showing dead buttons.
6. **Decouple the lifecycle** — PID/OK files currently land next to `db_path` in
   the Lightroom catalog folder.
7. **`client_id` for job and action polling.** Both `GET /v1/jobs/{id}` and
   `GET /v1/ui/actions` are *destructive* reads: a finished job is handed out
   exactly once, and the actions queue is drained. Two clients polling in
   parallel would steal each other's results. Invisible today because there is
   only one client.

Untouched: search, similarity, culling, faces, keyword clustering, jobs, model
assets, LLM providers, species links, DB bind/stats/backup, health/logs — all
already host-agnostic. `POST /v1/index/photos/by-path` would in fact have become
the *preferred* path, since the backend decodes originals itself and needs no
export renderer in the host at all.

## Decisions recorded

Taken 2026-09-10, in case this is revisited:

- **Windows scope:** reduced and honestly described. No writing into the Capture
  One SQLite database — schema migrates per point release, no undo, real
  corruption risk, permanent maintenance.
- **Index:** separate from Lightroom for now. Capture One would get its own store
  with a host-neutral `file1:` id. No rebuild of the working Lightroom path, no
  alias migration. Cost: a user switching from Lightroom re-indexes once.
- **Workflow priority:** catalogs before sessions.

## What would never have worked

On both platforms: own dialogs inside Capture One; the ~40 custom per-photo
metadata fields; alt text (Capture One has no such field); clickable
iNaturalist/Wikipedia links on the photo; undo for our changes; a hook that runs
on import; persisting plugin settings in Capture One (the vendor explicitly does
not store them); Lightroom-style `A|B|C` hierarchical keywords.

Additionally on Windows: albums as a result (Search Results, Picks, People,
Duplicates); setting the selection; applying develop settings automatically;
masks; virtual copies; "analyse the whole catalog"; metadata appearing without a
manual sync step.

Additionally on macOS: everything is Pro-gated (`current document` and `process`
are PRO only, so Essentials is largely out); Capture One must be running with a
document open; the user must grant Apple Events permission once, and refusing it
falls the write-back path back to XMP.

## Open verifications, if this is revisited

Points 1–4 cost under two hours on a Mac with a populated catalog.

| # | Question | How | What depends on it |
|---|---|---|---|
| 1 | Can AppleScript create **hierarchical** keywords? | `keyword` has a `parent` property in the sdef — read an existing hierarchy back, then try to create one. `,` and `\|` are forbidden in names. | Whether the core keyword path works natively on macOS or has to go through XMP there too |
| 2 | Does `make new layer … {kind:subject mask}` work? | Run it on an open image, check the Layers panel | Whether AI Edit gets masks on macOS |
| 3 | How fast is a JXA batch over 1000 variants? | Script that sets 1000 ratings, measure | Batch size, or whether write-back needs chunking |
| 4 | Does the developer portal still deliver a working SDK 1.0.1 for **both** platforms? | Sign up at captureone.com developer portal | **The single hard blocker for Windows.** Vendoring `PhaseOne.Plugin.dll` from third-party repos is not licensable |
| 5 | Does XMP sidecar sync work equivalently in **sessions**? | Test session, hand-write a sidecar, "Load Metadata" | Whether the Windows path holds for session users |
| 6 | How does a lower edition behave? | Call `current document` and `process` on Essentials | How much product survives on Essentials |
| 7 | Is Apple Events access granted cleanly to a LaunchAgent-started `.app`? | Prototype, start via LaunchAgent, run `osascript` | Whether the companion must stay an `.app` |
| 8 | The whole Windows path | Reproduce on a real Windows install | **Every Windows statement on this page is derived, not reproduced** |

## Verification status of this page

Everything under "Verified first-hand" was checked against the running
application on 2026-09-10. The AppleScript capability list is read from the
shipped `.sdef` — write operations were **not** executed, because the open
catalog was empty (0 images, 0 variants, 5 collections). All Windows statements
come from documentation, the two public Windows plugin repositories
(PanoCapture, PostCapture) and the `PhaseOne.Plugin.xml` shipped with them; they
are internally consistent but were not reproduced on a Windows machine.
