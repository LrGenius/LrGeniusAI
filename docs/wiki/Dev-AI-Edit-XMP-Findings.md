# AI Edit via XMP — Findings and Experiments

> **Status: research done 2026-09-26, nothing in AI Edit has changed yet.**
> The page records what Camera Raw's XMP (`crs:`) can express, how that maps
> onto the Lightroom SDK, and which questions only Lightroom itself can answer.
> Those questions are what *Help → Plug-in Extras → Developer: Run
> Develop-Settings Experiments…* runs (see [below](#running-the-experiments)).
> If the XMP idea comes back, start here rather than re-running the
> investigation.

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

For a radial gradient, `Flipped` is always `!MaskInverted`.

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
| `LrApplication.addDevelopPresetForPlugin` + `applyDevelopPreset(p, _PLUGIN, amount, updateAI)` (updateAI: 15.3+) | same table; AI masks incl. sub-categories proven by photo-workflow-mcp; masks reportedly added, not replaced (third-party report, LrC 11; E4 tests it) | AI compute per photo | one step | hidden presets, **no delete API** (E4) | path for AI masks |
| `.xmp` preset file | full XMP incl. `SupportsAmount` flags | no SDK import; the user imports via *Presets → Import* (visible immediately), a file dropped into the folder needs a restart | – | LrC normalises on import and silently drops invalid keys | export artifact, not an apply path |
| `.xmp` sidecar + *Read Metadata from Files* | full XMP | **no documented SDK call**; undocumented `photo:readMetadata()` is unverified | replaces **all** metadata | auto-write-XMP can clobber the file; JPEG/DNG means rewriting originals; virtual copies have no XMP | expert export only |
| `LrDevelopController` (today) | AI types without sub-category or geometry | module switch, sleeps | many steps | ok | fallback for LrC < 15.3 |

## Bugs found in the current code

- AI Edit writes and trains on the develop key **`Temp`**, which occurs in 0 of
  17,814 catalog rows. The real keys are `Temperature`/`Tint` (raw) and
  `IncrementalTemperature`/`IncrementalTint` (non-raw). White balance is most
  likely never learnt and never applied. E1 shows whether Lightroom rejects or
  ignores `Temp`.
- The dormant LLM path has several problems:
  - `EnableLensCorrections` (the panel toggle) is written where the profile
    toggle `LensProfileEnable` is meant.
  - The profile is written as `CameraProfile="Adobe Color"`. In current
    Lightroom that is a `Look` on top of `CameraProfile="Adobe Standard"`.
    Importing an `.xmp` preset silently drops that value (verified); that
    `applyDevelopSettings` does the same is likely but unverified.
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
- Turn *Automatically write changes into XMP* off in that catalog.
- Select the photos, then run the task from any module. It switches to the
  Library module on purpose, because E2 asks whether masks can be added outside
  Develop.

| Id | Question | What it does |
|---|---|---|
| E13 | Which orientation do `getRawMetadata('dimensions')` / `'croppedDimensions'` use, and what does `orientation` look like? | Read-only. Logs metadata, the full list of raw-metadata keys, and the develop settings. Fits the crop model under every width/height interpretation, and says "ambiguous" when two interpretations fit equally. |
| E1 | Does `applyDevelopSettings` accept `Temp`? | Uses a fresh virtual copy per variant (`LrG Exp E1a` … `E1d`), so each starts from the master's white balance, and diffs the white-balance keys. Raw: `{Temp}`, `{Temperature}`, `{WhiteBalance="Custom", Temperature, Tint}` and `{WhiteBalance="Custom", Temp}`. Non-raw: `{Temp}`, `{IncrementalTemperature/Tint}`, `{Temperature}` and `{WhiteBalance="Custom", Temp}`. The last variant keeps a no-op under "As Shot" from being read as a rejected key. |
| E2 | Can `applyDevelopSettings` add a new AI mask, and when is it computed? | On an `LrG Exp E2` copy: adds a subject mask (+1 EV), then waits up to 30 s for Lightroom to compute it unasked. The first detection in a session loads the model, so a short wait would be misleading. If nothing happened, calls `photo:updateAISettings()` and polls for up to 60 s for `MaskDigest` / `ErrorReason`, recording `needsUpdateAISettings` and `isAvailableForEditing` along the way. Then adds sky, background and hair masks in one call, updates once and polls each. "Computed, nothing found" means the definition was accepted but the photo has no such content. |
| E4 | How do plugin presets behave, and are preset masks merged with or replacing the photo's? | Adds a preset twice under one name with different content, and records the uuid, file path, file contents and preset count. On an `LrG Exp E4` copy it first seeds a linear gradient via `applyDevelopSettings`, then applies the preset with `updateAI=true`; whether the gradient survives answers merge vs replace. It applies the preset again and counts the corrections carrying its sync id. Finally it applies two amount presets, one without and one with the `SupportsAmount` flags, at 50 and 200, and reads back the before and after values. |

**Output.**
- A Markdown report plus the same data as JSON next to it, written where you
  choose. If a `.json` of that name already exists, a numbered name is used
  instead; the final dialog shows both paths. The report holds the verdicts, a checklist of things the SDK cannot read back (history
  step names, what a mask covers, AI progress dialogs), and every step's raw
  readback.
- A failed step is recorded, not hidden. An error is often the answer itself
  (e.g. `Temp` rejected).

**Cleanup.**
- Delete the `LrG Exp …` virtual copies.
- E4's plugin presets ("LrGenius Experiment E4", "… E4 Amount", "… E4 Amount
  Flagged") cannot be deleted through the SDK. They are files in Lightroom's
  preset folder, outside the catalog and shared by every catalog, so deleting
  the test catalog does not remove them. Their paths are in the final dialog
  and in the report header; delete those files by hand. A later run records how
  many presets of that name already existed.

### Still open after these four

- `CircularGradient.Angle` convention and the direction of `Zero` vs `Full`.
- Whether masks sit before or after lens correction and Upright.
- Mirrored orientations and non-3:2 sensors.
- Whether a digest-less AI mask in a *sidecar* is recomputed on *Read Metadata*.
- Whether LrC accepts a self-built Denoise `Table_` blob.
- Import normalisation of generated `.xmp` presets.
- Whether subtype-3 people parts apply to every person in the photo.
- Whether a stubbed `Look` (name and UUID only) works through
  `applyDevelopSettings`.
