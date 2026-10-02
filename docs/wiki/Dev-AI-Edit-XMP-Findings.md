# AI Edit via XMP — Findings and Experiments

> **Status: research done 2026-09-26; step 1 in progress (1d/1f changed AI
> Edit's style path).**
> The page records what Camera Raw's XMP (`crs:`) can express, how that maps
> onto the Lightroom SDK, and which questions only Lightroom itself can answer.
> Those questions are what *Help → Plug-in Extras → Developer: Run
> Develop-Settings Experiments…* runs (see [below](#running-the-experiments)).
> If the XMP idea comes back, start here rather than re-running the
> investigation.
>
> Work has started: the first building blocks, a native model of Lightroom's
> develop settings (key registry, typed model, readers for the Lua form and
> for XMP), are the `lrg-develop` crate. The XMP reader parses Lightroom's
> bundled presets and the current state of that sidecar corpus with no
> unknown key (counts and method in
> [Native Develop Model](Dev-Develop-Model#checked-against-real-files)). The
> crate also writes develop presets and builds semantic, gradient and
> luminance-range corrections
> ([Writing XMP presets](Dev-Develop-Model#writing-xmp-presets),
> [Building corrections](Dev-Develop-Model#building-corrections)). Rebuilt
> with those builders, Adobe's adaptive presets match Adobe's corrections
> field for field once both go through the preset writer (names and sync ids
> apart); see Native Develop Model for the fields the writer holds back
> (LocalHue, LocalGrain, point colour, digests) and the lower
> `CompatibleVersion` in 10 of 38. Nothing calls the preset writer yet; AI
> Edit's style path already uses the crate (white balance in 1d, more
> sliders in 1f — see [Native Develop Model](Dev-Develop-Model)).

## The question

AI Edit applies a recipe of global sliders through `photo:applyDevelopSettings`
and adds masks through `LrDevelopController` UI automation. The proposal was to
write `.xmp` develop settings instead, so AI Edit could use *every* develop
feature. There were three ways to deliver them: the current pipeline, a develop
preset, or an XMP per photo. The concern that came with it: the image AI Edit
analyses is a downscaled, cropped export, so mask coordinates predicted on it do
not match the full-size photo.

## Short answer

- **XMP is the right data model, but usually not the right transport.** The
  `crs:` key space is the same table `photo:getDevelopSettings()` returns and
  `applyDevelopSettings()` / `addDevelopPresetForPlugin()` accept. That includes
  masks, as `MaskGroupBasedCorrections → CorrectionMasks`. The mapping is
  mechanical:
  - drop the `crs:` prefix
  - `rdf:Seq` becomes an array, a nested `rdf:Description` becomes a table
  - point curves become flat number arrays
- **What AI Edit lacks is in its recipe schema, not in the SDK.** Missing today:
  the full colour grading, profiles as a `Look`, B&W mix, calibration, lens
  profile, lens blur, every AI mask type (people parts, landscape classes),
  linear/radial/range masks. This is proven for the global sliders. New masks
  written through the SDK are so far shown only by third-party plugins, and a
  new AI mask through `applyDevelopSettings` is exactly what E2 tests.
- **The resolution concern disappears for semantic AI masks.** Subject, sky,
  background, people parts and landscape classes are stored without geometry.
  Lightroom computes them itself at about 2880 px.
- **The frame of reference is the real problem.** Geometric masks and the crop
  live in a different frame from the export. The mapping is exact and
  computable, except under lens warp and Upright.
- **The training data already holds masks.** 497 of 1,294 stored training
  examples carry the full `getDevelopSettings()` table *with* masks, so a style
  engine could learn masks from the user's own edits without anyone re-saving.
  Masks are also the norm in real edits: on the corpus below, 83 % of photos
  edited in 2026 have one, and across all years 79 % of masked photos use a
  subject mask.

## Evidence base

- The 1,045 presets bundled with Lightroom Classic 15.5.1, 38 of them Adobe
  "Adaptive" presets with AI masks.
- A corpus of 12,338 edited photos: LrC sidecars written by LrC 10.2–15.5.1,
  Canon 3:2 sensors, one photographer.
- A read-only copy of that catalog: `Adobe_imageDevelopSettings`,
  `croppedWidth/Height` and orientation.
- The Lightroom Classic SDK 15.3 API reference.
- Third-party plugins that write masks: jasondentler/photo-workflow-mcp and
  Wyyyyuu/lightroom-style.
- darktable's `src/develop/lightroom.c`, ExifTool's `crs` tag table, and Adobe
  community threads.

## Masks in XMP

```
crs:MaskGroupBasedCorrections   rdf:Seq — one entry per mask in the Masks panel
  What="Correction", CorrectionAmount (1 = 100 %, 0..2), CorrectionActive,
  CorrectionName, CorrectionSyncID (32 hex), Local* sliders
  crs:CorrectionMasks            rdf:Seq — the mask's components, applied in order
    What, MaskActive, MaskName, MaskBlendMode, MaskInverted, MaskSyncID, MaskValue, ...
```

### Combining components

| Encoding | Meaning |
|---|---|
| `MaskBlendMode=0` | add |
| `MaskBlendMode=1` + `MaskValue=0` | subtract |
| `MaskBlendMode=1` + `MaskInverted=true` | intersect (A ∩ B = A − ¬B; 105 corpus cases) |

For a radial gradient, `Flipped` is always `!MaskInverted`. For a range mask,
`CorrectionRangeMask.Invert` is always equal to `MaskInverted` (every range
mask in the sidecar corpus, luminance and colour, intersections included), so
an intersected luminance range carries `Invert="true"`; the builders set both
mirrors themselves.

### Mask types by robustness

| Class | `What` | Depends on frame/resolution? |
|---|---|---|
| Semantic AI | `Mask/Image` + `MaskSubType` / `MaskSubCategoryID` | **No.** Only the intent is stored; Lightroom recomputes it. This is the only kind Adobe's presets contain. |
| Select Object | `Mask/Image` subtype 0 + `crs:Gesture` (a 4-point polygon or brush strokes) | The hint geometry is frame-dependent; Lightroom refines it. |
| Geometric | `Mask/Gradient` (`ZeroX/Y`, `FullX/Y`), `Mask/CircularGradient` (`Top/Left/Bottom/Right`, `Angle`, `Feather`) | Frame-dependent, see below. |
| Brush | `Mask/Aggregate` → `Mask/Paint` (`Dabs`: `d x y`, `M x y`, `r R`, `f F`) | Frame-dependent. Strokes sometimes live only in `.acr` / the catalog (`MaskBrushTable`). |
| Range | `Mask/RangeMask` + `CorrectionRangeMask` (`Type` 2 = luminance `LumRange`, 1 = colour) | Luminance: no. Colour: photo-specific samples. |

Computed AI mask bitmaps never live in modern XMP. They sit in a sibling
`<basename>.acr` (8-bit TIFF, JPEG-XL) keyed by `MaskDigest`, or in the catalog's
`.lrcat-data`. They cannot be written by us.

### `MaskSubType` / `MaskSubCategoryID`

| SubType | Category | Meaning | Evidence |
|---|---|---|---|
| 1 | – | Subject | Adobe presets + 5,149 corpus masks |
| 2 | – | Sky | Adobe presets + corpus |
| 0 | 22 | Background | corpus (603), user presets; not in Adobe presets |
| 0 | – + `Gesture` | Select Object | corpus (99) |
| 3 | 2–9, 11, 12 | People part, preset form (reportedly all people, unconfirmed) | every Adobe portrait preset |
| 0 | 2–9, 11, 12 | People part of one specific person (created in the UI) | corpus |
| 0 | 50001–50008 | Landscape classes | Adobe landscape presets |
| 0 | 20036 | Whole person | 4 corpus masks, meaning unconfirmed |

People-part categories:

| Id | Part |
|---|---|
| 2 | Face skin |
| 3 | Iris & pupil |
| 4 | Body skin |
| 5 | Hair |
| 6 | Lips |
| 7 | Facial hair |
| 8 | Sclera |
| 9 | Eyebrows |
| 11 | Clothes |
| 12 | Teeth |

Landscape categories:

| Id | Class |
|---|---|
| 50001 | Architecture |
| 50002 | Mountains |
| 50003 | Artificial ground |
| 50004 | Natural ground |
| 50005 | Vegetation |
| 50006 | Sky (≠ subtype 2) |
| 50007 | Water |
| 50008 | Snow |

### Local slider scaling

- `LocalExposure2012` is stored as **EV/4**: ±1 means ±4 EV.
- The other signed sliders are UI/100: `Contrast2012`, `Highlights2012`,
  `Shadows2012`, `Whites2012`, `Blacks2012`, `Clarity2012`, `Dehaze`, `Texture`,
  `Saturation`, `Temperature`, `Tint`, `Sharpness`.
- `LocalToningHue` is in degrees.
- The tone keys carry `2012`. `LocalTemperature`, `LocalTint`, `LocalTexture`,
  `LocalDehaze` and `LocalSaturation` do not.

A preset-safe AI mask looks like Adobe's own: the definition only, with
`ReferencePoint="0.500000 0.500000"`, `ErrorReason="0"`, and none of `MaskDigest`,
`InputDigest`, `FullMaskSize`, `WholeImageArea`, `Origin`, `CorrectionID`, `MaskID`.

## Coordinates and the resolution problem

Verified on the corpus and the catalog:

- **Every geometric value lives in one frame.** This covers crop, gradients,
  radial, brush dabs, AI `ReferencePoint`s and even LrC's own `mwg-rs` face
  regions. The frame is the **uncropped image in sensor orientation** (before
  `tiff:Orientation`), normalised **per axis** (x/W, y/H). A pixel circle is
  therefore a normalised ellipse.
  - `croppedWidth = (R−L)·W_sensor` holds for orientation 8 in 123/123 cases,
    for orientation 6 in 10/10 and for orientation 1 in 271/271. The display-frame
    hypothesis never matches.
- **The crop is a rotated rectangle.** `CropLeft/Top` and `CropRight/Bottom` are
  opposite corners of a rectangle rotated by +`CropAngle` in sensor *pixel* space.
  This reproduces the catalog's cropped size to ≤2.5 px on 570/570 angled crops.
- **Brush `Radius` is normalised by the sensor width**, which is the long side
  for every camera in the corpus. Dabs are spaced at 0.30 × radius.
- **AI mask canvases are always about 2880×1920 in sensor orientation**,
  independent of the crop and of the size of any export.

The AI Edit export is none of that. It is rendered (default 3072 px long edge,
`Defaults.lua`) *after* lens correction, Upright, crop, crop angle and
orientation. Today the plugin sends the backend no dimensions, crop or
orientation.

**Export pixel → XMP mask coordinate** (without warp), given the develop
settings `CropLeft/Top/Right/Bottom = L/T/R/B`, `CropAngle = θ`, orientation and
sensor-oriented `W, H`:

1. Normalise the export pixel: `x = (i+0.5)/w_export`, `y = (j+0.5)/h_export`.
2. Undo the orientation to get crop-local `(u, v)`:

   | Orientation | `(u, v)` |
   |---|---|
   | 1 | `(x, y)` |
   | 3 | `(1−x, 1−y)` |
   | 6 | `(y, 1−x)` |
   | 8 | `(1−y, x)` |

   Mirrored orientations flip the sense of θ; they are untested.
3. Compute the crop rectangle in sensor pixels:
   - `Δx = (R−L)·W`, `Δy = (B−T)·H`
   - `w_c = cos θ·Δx + sin θ·Δy`, `h_c = −sin θ·Δx + cos θ·Δy`
4. Place the point: `P = (L·W, T·H) + u·w_c·(cos θ, sin θ) + v·h_c·(−sin θ, cos θ)`.
5. The mask coordinate is `(P.x / W, P.y / H)`.

Lengths scale uniformly by `w_c / w_export`. With an active lens profile the
residual is a few pixels. With Upright or manual transforms it is unknown, so
geometric masks should not be generated there.

**Strategy:**

1. Prefer semantic AI masks. They are resolution-independent and the only
   preset-safe kind.
2. Per photo only: object boxes, people `ReferencePoint`s, and gradient/radial
   masks through the mapping above.
3. Never generate brush strokes.
4. Never put geometry or a crop into a shared preset.

## Ways into Lightroom

| Path | Coverage | Batch | Undo | Catalog safety | Verdict |
|---|---|---|---|---|---|
| `photo:applyDevelopSettings(tbl, name, flattenAuto)` | all flat keys; new gradient/radial/brush/range masks proven by third-party plugins; **new AI masks unproven** (E2) | one write gate, no module switch | one step | good | primary path |
| `LrApplication.addDevelopPresetForPlugin` + `applyDevelopPreset(p, _PLUGIN, amount, updateAI)` (updateAI: 15.3+) | same table; AI masks incl. sub-categories proven by photo-workflow-mcp; masks reportedly added, not replaced (third-party report, LrC 11; E4 tests it) | AI compute per photo | one step | hidden presets, **no delete API**; deleting the file right after applying is E4f | path for AI masks |
| `.xmp` preset file | full XMP incl. `SupportsAmount` flags | no SDK import; the user imports via *Presets → Import* (visible immediately), a file dropped into the folder needs a restart | – | LrC normalises on import and silently drops invalid keys | export artifact, not an apply path |
| `.xmp` sidecar + *Read Metadata from Files* | full XMP | **no documented SDK call**; undocumented `photo:readMetadata()` is unverified | replaces **all** metadata | auto-write-XMP can clobber the file; JPEG/DNG means rewriting originals; virtual copies have no XMP | expert export only |
| `LrDevelopController` (today) | AI types without sub-category or geometry | module switch, sleeps | many steps | ok | fallback for LrC < 15.3 |

"Write an `.xmp` preset, import it, apply it, delete it" has no SDK form as
such: there is no call to import a preset file and none to delete a preset.
Its programmatic equivalent is the plugin-preset row:
`addDevelopPresetForPlugin` writes the preset file, `applyDevelopPreset`
applies it, and `LrFileUtils.delete` removes the file at the path
`preset:getFile()` returns. Whether the edit survives that, and what Lightroom
still lists until a restart, is E4f.

## Bugs found in the current code

- AI Edit writes and trains on the develop key **`Temp`**, which occurs in 0 of
  17,814 catalog rows. The real keys are `Temperature`/`Tint` (raw) and
  `IncrementalTemperature`/`IncrementalTint` (non-raw). White balance is most
  likely never learnt and never applied. E1 shows whether Lightroom rejects or
  ignores `Temp`. *Fixed in AI Edit (step 1, PR 1d):* the style engine now
  reads the white balance typed from the real keys and sends it as
  `edit.white_balance` (see [Dev-Develop-Model](Dev-Develop-Model) and
  `POST /v1/edit/style` in [Dev-Backend-API](Dev-Backend-API)); only the
  develop experiments still write `Temp`, on purpose.
- The dormant LLM path has several problems:
  - `EnableLensCorrections` (the panel toggle) is written where the profile
    toggle `LensProfileEnable` is meant.
  - The profile is written as `CameraProfile="Adobe Color"`. In current
    Lightroom that is a `Look` on top of `CameraProfile="Adobe Standard"`.
    Importing an `.xmp` preset silently drops that value (verified); that
    `applyDevelopSettings` does the same is likely but unverified. E11 tests
    the correct form, a `Look` on top of an `Adobe Standard` base profile.
  - The crop maths assumes the export frame.
- Reported by the research, not yet reproduced: the mask fallback in
  `DevelopEditManager` writes a `Correction` sub-table with `local_*` keys in UI
  units. That shape does not exist, so the fallback is a no-op that reports
  success.

## Proposed modes

1. **Apply directly** (replaces today's pipeline). Apply the full native table
   through `applyDevelopSettings` in one write gate and one history step. AI
   masks go through a plugin preset with `updateAI=true`, or through
   `updateAISettings()` if E2 shows that works. UI automation stays only for
   LrC < 15.3.
2. **As preset.** The backend renders an `.xmp` preset containing only semantic
   masks, with no crop, geometry or lens identity. The user imports it through
   the Presets panel. It is shareable and gets the amount slider.
3. **XMP per photo.** Only as an explicit "export a sidecar for ACR or other
   tools" for raw masters, with warnings, because of the metadata replacement
   and auto-write clobbering above.

Wherever the XMP is produced, it should come from one native-table model in the
backend. That model needs a whitelist with ranges, a mask builder, and a
denylist: `FilterList`, `RetouchAreas`, digests, `CorrectionID`/`MaskID`,
`Crop*` in presets, and camera-specific looks.

## Running the experiments

*Help → Plug-in Extras → Developer: Run Develop-Settings Experiments…*
(`TaskDevelopExperiments.lua`; the pure helpers are in `DevelopExperiments.lua`).

**Use a throwaway test catalog.**
- Import copies of three or four photos (the task takes at most four):
  - a raw rotated in camera (portrait) whose crop is straightened or changes
    the aspect ratio. Without that, E13 can only answer "ambiguous";
  - a raw that shows a person and sky. The sky and hair masks in E2 find
    nothing on other photos;
  - optionally a raw with strong lens distortion;
  - a JPEG.

  E13's full readback gives Lightroom's defaults only for photos without
  develop edits (its verdict says which photos have edits), so leave at least
  one raw and the JPEG untouched after import, with *Raw Defaults* set to
  *Adobe Default*. E11 needs a raw. No develop preset is needed: E11b takes its
  complete `Look` from a photo, best from a second raw with a different
  profile. Lightroom's bundled presets carry Looks only as stubs
  (`Stubbed="true"`, no `Parameters`), so they cannot supply one.
- Turn *Automatically write changes into XMP* off in that catalog.
- Select the photos, then run the task from any module. It switches to the
  Library module on purpose, because E2 asks whether masks can be added outside
  Develop.

| Id | Question | What it does |
|---|---|---|
| E13 | Which orientation do `getRawMetadata('dimensions')` / `'croppedDimensions'` use, and what does `orientation` look like? What does a photo's full develop-settings table hold? | Read-only. Logs metadata, the full list of raw-metadata keys, and the develop settings. Fits the crop model under every width/height interpretation, and says "ambiguous" when two interpretations fit equally. Also records every top-level `getDevelopSettings()` key: scalars verbatim, tables by shape (small all-scalar tables such as tone curves with their values), `Look` as name/UUID/amount plus whether `Parameters` exist, masks as in E2. Whether the photo has develop edits comes from Lightroom's own *Has Adjustments* search (narrowed to the file name, and trusted only when the name search finds the photo), plus `editCount` and `lastEditTime`. On an unedited raw and JPEG this is Lightroom's raw and non-raw default table; the verdict says "has develop edits - not a default table" when it is not, and names the white-balance family. The full dump is in the JSON. |
| E1 | Does `applyDevelopSettings` accept `Temp`? Does a white-balance mode on its own make Lightroom recompute temperature and tint? | Uses a fresh virtual copy per variant (`LrG Exp E1a` … `E1g`), so each starts from the master's white balance, and diffs the white-balance keys. Raw: `{Temp}`, `{Temperature}`, `{WhiteBalance="Custom", Temperature, Tint}`, `{WhiteBalance="Custom", Temp}`, then mode only: `{WhiteBalance="Daylight"}`, `{WhiteBalance="Auto"}` and `{WhiteBalance="Auto"}` with `optFlattenAutoNow=true`. Non-raw: `{Temp}`, `{IncrementalTemperature/Tint}`, `{Temperature}`, `{WhiteBalance="Custom", Temp}` and the two Auto variants (non-raw files have no Daylight). The `Custom`+`Temp` variant keeps a no-op under "As Shot" from being read as a rejected key. Each mode-only copy is first put on a distinctive Custom white balance in its own history step (raw `Temperature=3000, Tint=40`; non-raw `IncrementalTemperature=-40, IncrementalTint=40`), so a recomputation always shows as a change. After the mode is written, the copy is read back right away and then every 0.5 s for up to 15 s, stopping at the first change of `Temperature`/`Tint` (raw) or `IncrementalTemperature`/`IncrementalTint` (non-raw). The verdict says "recomputed ... after N s" or "not recomputed within N s". A flattened Auto that reads back as Custom with new values is reported as flattened, not as a rejected mode. Raw or not is decided by the photo's white-balance family, so a DNG converted from a JPEG counts as non-raw. |
| E11 | Does an Adobe Raw profile `Look` transfer through `applyDevelopSettings`, and in which form? | Raw photos only. Non-raw photos get a "skipped" verdict: E11 tests Adobe Raw Looks, which need a raw file, and creative Looks on non-raw files are not tested. The base profile is the master's when it is an `Adobe Standard` variant (e.g. `Adobe Standard v2`), otherwise `Adobe Standard`, so only the Look changes. `LrG Exp E11a`: a stub in the form Lightroom's presets carry it, `{Name, UUID, Amount=1, Stubbed=true}`, using the first of Adobe Vivid, Adobe Landscape and Adobe Color whose name differs from the photo's current Look. `LrG Exp E11c`: the same stub without `Stubbed`, so the difference between the two forms is visible. `LrG Exp E11b`: a complete `Look` with `Parameters`, taken at run time from, in this order: another selected photo's Look (not an Adobe Adaptive Look, and a camera-restricted Look only from a photo of the same camera model); the Look the E11a copy read back, if Lightroom filled in the stub; the photo's own Look, written onto a copy that was first switched to a stubbed different Look in an `LrGenius E11b prepare` step (inconclusive if that switch did not take); and, as a last resort, an installed develop preset whose Look has `Parameters`. The source goes into the step data. Nothing of a Look is stored in the plugin or the report beyond name, UUID, flags and key counts. Reads back `CameraProfile`, `Look.Name`, `Look.UUID` and whether `Parameters` exist, and says honoured / ignored / changed / error. If the preset scan runs, presets it could not read are counted and named in the skip reason, and a scan in which every read failed is a failed step. |
| E2 | Can `applyDevelopSettings` add a new AI mask, when is it computed, and how long does it take? | On an `LrG Exp E2` copy: adds a subject mask (+1 EV), then waits up to 30 s for Lightroom to compute it unasked. The first detection in a session loads the model, so a short wait would be misleading. If nothing happened, calls `photo:updateAISettings()` and polls for up to 60 s for `MaskDigest` / `ErrorReason`, recording `needsUpdateAISettings` and `isAvailableForEditing` along the way. The E2c verdict and step data carry how long the `updateAISettings()` call itself took, any wait for write access before it, and how long from the start of the call until the mask reached a final state. The time is marked as a lower bound when Lightroom was already computing: the photo was locked before the update could start, or at some point during the unasked wait. A correction whose AI tool disappears is reported as such and is not counted as a ready mask. A run-level line lists the timings per photo and marks the first measured photo, whose time may include loading the model. Then adds sky, background and hair masks in one call, updates once and polls each. "Computed, nothing found" means the definition was accepted but the photo has no such content. |
| E4 | How do plugin presets behave, and are preset masks merged with or replacing the photo's? | Adds a preset twice under one name with different content, and records the uuid, file path, file contents and preset count. On an `LrG Exp E4` copy it first seeds a linear gradient via `applyDevelopSettings`, then applies the preset with `updateAI=true`; whether the gradient survives answers merge vs replace. It applies the preset again and counts the corrections carrying its sync id. Then it applies two amount presets, one without and one with the `SupportsAmount` flags, at 50 and 200, and reads back the before and after values. Last, E4f: creates "LrGenius Experiment E4f" (`Contrast2012=25` plus a subject mask), applies it with `updateAI=true` to an `LrG Exp E4f` copy and reads it back, then deletes the file `preset:getFile()` reports with `LrFileUtils.delete`. It deletes only a file (never a directory) whose name is the preset's name, alone or followed by a suffix starting with a non-alphanumeric character ("E4f 2"), that is not another E4 preset's file, and that sits next to the other E4 preset files or inside a `Plugin Develop Presets` folder (`DevelopExperiments.presetFileDeletable`); anything else is refused and reported. After the delete it records whether `getDevelopPresetsForPlugin` still lists the preset (by name and uuid), whether `preset:getSetting()` still works, whether the first copy kept its contrast and mask, and whether applying the deleted preset object to a second copy (`LrG Exp E4f after delete`) works, fails or does nothing. It then re-adds the same name, records whether and where a file appears, and deletes that file too. Before the add it records every preset of that name already listed, with whether its file exists. After the re-add it also records whether the new preset holds the new settings (`Contrast2012=15`) or the deleted one's, and how many presets of that name are listed. The verdict says whether "apply then delete" is viable within the session, judged only on what the apply changed: a preset with no visible effect is inconclusive, and a subject mask that was still pending at the delete limits the verdict to the settings. Restart behaviour is a manual check: plugin presets never appear in the Develop presets panel, so it looks in the `Plugin Develop Presets` folder; running E4 again after the restart reports the presets of that name still listed, split into "file missing" (kept somewhere other than the file), "file present" (left over, or written back at quit) and none. The final dialog always shows the E4f outcome, past the per-experiment cap. |

**Output.**
- A Markdown report plus the same data as JSON next to it, written where you
  choose. If a `.json` of that name already exists, a numbered name is used
  instead; the final dialog shows both paths. The report holds the verdicts, a checklist of things the SDK cannot read back (history
  step names, what a mask covers, AI progress dialogs), and every step's raw
  readback. Long step data (E13's full develop-settings readback) is cut off
  in the Markdown; the JSON has it in full.
- A failed step is recorded, not hidden. An error is often the answer itself
  (e.g. `Temp` rejected).

**Cleanup.**
- Delete the `LrG Exp …` virtual copies (E1a–E1g, E11a–E11c, E2, E4, the
  E4 amount copies, and E4f's `LrG Exp E4f` and `LrG Exp E4f after delete`).
- E4's plugin presets ("LrGenius Experiment E4", "… E4 Amount", "… E4 Amount
  Flagged") cannot be deleted through the SDK. They are files in Lightroom's
  preset folder, outside the catalog and shared by every catalog, so deleting
  the test catalog does not remove them. Their paths are in the final dialog
  and in the report header; delete those files by hand. A later run records how
  many presets of that name already existed.
- E4f deletes its own preset files. The list in the final dialog and the report
  header holds only files that still exist, so an E4f file shows up there only
  when E4f stopped before deleting it (no virtual copy, the photo stayed
  locked, the apply or its readback failed, or the run was canceled), when the
  delete failed or was refused, or when applying the deleted preset wrote the
  file back.

### What the answers change

The Lua writer (`lrg_develop::lua::write`, PR 1g) represents every
alternative E1, E2, E4 and E11 measure as a `LuaOptions` switch and decides
none of them; the provisional choices are `LuaOptions::PROVISIONAL`. Which
field each experiment bears on, what the experiments *as built* can settle
about it, the evidence so far, each field's status and the steps after a
run are in one table:
[Dev: Native Develop Model — Options and what decides them](Dev-Develop-Model#options-and-what-decides-them).
Read its "Settles" column before marking a field decided: for several
fields the experiments apply only the provisional form, so a working run
confirms it without measuring the alternative, and one field (`int_flag_as`)
no experiment touches at all.
The plugin spec `plugin/spec/native_wire_format_spec.lua` checks the
goldens only for rules that hold whatever the experiments answer and
accepts both forms of each provisional encoding, and the Rust tests check
every single flip, so flipping a field needs a re-bless, not a test change
([Wire goldens](Dev-Develop-Model#wire-goldens)).

### Still open after these experiments

Write-side forms the experiments as built do not measure (the Lua writer
holds each as a `LuaOptions` alternative; see the table linked above):

- Whether the 0/1 flag keys (`LensProfileEnable`, `AutoLateralCA`,
  `CropConstrainToWarp`, `HDREditMode`) take booleans, or need numbers: no
  experiment writes one (`int_flag_as`). Cheap evidence: read back photos
  the LLM path already applied `AutoLateralCA` to as a boolean.
- Mask enums as numeric strings (`mask_enum_as = String`): E2/E4 write
  numbers only.
- A mask correction without `EnableMaskGroupBasedCorrections`
  (`panel_switches = None`, the provisional form), and every other panel
  switch (`PANEL_SWITCHES`, unverified).
- A correction with only its own `Local*` keys (`local_form = Sparse`), an
  AI mask without `MaskVersion`/`ReferencePoint`/`ErrorReason`
  (`mask_form = Minimal`), and any luminance range applied through
  `applyDevelopSettings`.
- A non-raw `{WhiteBalance = "Custom", IncrementalTemperature,
  IncrementalTint}` (`wb_custom_with_numbers` on non-raw files), and a named
  mode other than `Daylight`/`Auto` alone (`wb_mode_only`).
- A camera-restricted or an Adobe Adaptive `Look`, and whether an adaptive
  one computes its `AILook` after `updateAISettings()`.
- Applying a wire golden unchanged (decoded with JSON.lua): the experiments
  apply hand-built tables, never the writer's own output. Needed before the
  goldens are frozen.

Other open questions:

- `CircularGradient.Angle` convention and the direction of `Zero` vs `Full`.
- Whether masks sit before or after lens correction and Upright.
- Mirrored orientations and non-3:2 sensors.
- Whether a digest-less AI mask in a *sidecar* is recomputed on *Read Metadata*.
- Whether LrC accepts a self-built Denoise `Table_` blob.
- Import normalisation of generated `.xmp` presets.
- Whether subtype-3 people parts apply to every person in the photo.
