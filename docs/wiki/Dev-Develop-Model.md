# Dev: Native Develop Model (`lrg-develop`)

> **Status: steps 1a, 1c, 1b and 1g (1d/1f use it; 1b's manual import test in
> the installed Lightroom Classic is pending, see [Hand test](#hand-test)).** The crate has the key
> registry, the typed model, the policy filter, the readers for the Lua table
> form and for XMP (sidecars, develop presets, profiles), the develop-preset
> writer, the mask builders and the Lua writer. The Lua writer's forms are
> **provisional** until experiments E1/E2/E4/E11 are run (see [Writing the
> Lua form](#writing-the-lua-form-provisional)). Background and the research behind it:
> [AI Edit via XMP — Findings and Experiments](Dev-AI-Edit-XMP-Findings).

`server-rs/crates/lrg-develop` holds the facts and shapes of Lightroom
Classic's develop settings: which keys exist, what they mean, how they are
stored, and what may be learned, shared or written. It is a **leaf crate** —
no image, network or async dependencies — so `cargo test -p lrg-develop` runs
in seconds without an ONNX or LanceDB build.

What it deliberately does *not* hold: learning and blending (majority votes,
circular means, thresholds) stay in `lrg-analysis::style_engine`, and the LLM
edit-recipe schema stays in `lrg-providers::edit_recipe`.

Who uses it today:

- `lrg-analysis::training` reads a training example's `develop_settings` blob
  with `lua::read`, learns the registry keys that have a `global.*` recipe
  alias, and keeps the white balance as a typed `WbSetting`
  (`CANONICAL_VERSION` marks the stored form). A black-and-white example
  (`ConvertToGrayscale`) teaches no `color_grading_*`/`hsl_*` keys: the recipe
  does not carry the B&W switch, so its toning would reach colour photos.
- `lrg-analysis::style_engine` rounds blends to each key's registry precision,
  blends each `CircularHue` key with its saturation as a mean colour vector
  (hue = angle, saturation = length), and decides the white balance per key
  family.
- `lrg-api` (`routes/training.rs`, `routes/style_edit.rs`) stores and re-reads
  that canonical form.
- `lrg-providers` only in a test: every range the LLM recipe schema declares
  for a registry key must equal the key's UI range.

## Layout

| Module | What |
|---|---|
| `registry` | One `KeySpec` per develop key (`table.rs`), name families (`patterns.rs`), UI ↔ stored scaling (`ui.rs`, the only place), int/real conversion (`coerce.rs`, the only place) |
| `model` | `DevelopSettings`, `Value`/`Finite`/`Opaque`, corrections and masks (`correction.rs`), white balance (`whitebalance.rs`), the policy filter (`policy.rs`) |
| `lua::read` | `getDevelopSettings()` as JSON.lua encodes it → model + warnings |
| `lua::write` | model → the table `applyDevelopSettings()` takes, as JSON for JSON.lua (`LuaMode::Apply`), or a lossless test rewrite (`LuaMode::TestRoundTrip`, feature `test-roundtrip`); the experiment-decided forms are `LuaOptions` |
| `xmp::read` | an XMP sidecar, preset or profile → `XmpDocument` (kind, header, model, warnings, skipped subtrees) |
| `xmp::write` | model → a develop preset (`WriteMode::Preset`), or a lossless test rewrite (`WriteMode::TestRoundTrip`, feature `test-roundtrip`) |
| `xmp::format` | how the writer spells numbers, booleans, curves, ids, versions; XML escaping |
| `build` | `CorrectionBuilder`: new corrections from UI values and semantic, gradient and luminance-range components, with deterministic sync ids |
| `reader` (crate-private) | what both readers share: typing through the registry, closed sets and ids, curve points, file kind, process-version check, assembly |
| `parse` | `ParseError` (input unusable) and `ParseWarning` (one value, reading went on) |

## The key registry

Every key is one row: name, level (global, preset header, correction, mask
tool, or a field of a nested structure), value kind, range **in the stored
unit**, UI scaling, XMP number format, defaults for raw and non-raw files,
policy class, frame scope, file kind (raw only / non-raw only / both), where
it occurs (Lua, XMP, both, or not yet observed), minimum process version and
recipe alias. Lookup
is by `(level, name)`, because names such as `Version` mean different things
on different levels. Families that are never modelled one key at a time are
patterns: `Table_<md5>` and `BrushTable_<md5>` blobs, `pm_*` patch state,
`UprightTransform_N`/`UprightFourSegments_N`, and everything under
`FilterList`.

### Maintaining the registry

The table was seeded from `server-rs/scripts/develop_registry/spec.py`
by `gen_table_rs.py` (same folder; it imports `spec.py` and reads one
committed data file, the non-raw readback in
`server-rs/testdata/develop/defaults/`; never the rendered Markdown, an
inventory or the training dump). **`table.rs` is the source of truth.**
Adding or changing a key is editing a row there by hand, then refreshing the
review snapshot:

```bash
cd server-rs
LRG_BLESS=1 cargo test -p lrg-develop --test registry_snapshot
```

`server-rs/testdata/develop/registry_snapshot.json` is generated from the
table and the patterns and compared on every run, so every registry change is
a readable diff in review. Never edit the snapshot by hand.

Re-running the generator **overwrites hand edits** to `table.rs`. Do it only
when `spec.py` itself changed, and review the diff:

```bash
cd server-rs/scripts/develop_registry
python3 gen_table_rs.py --out ../../crates/lrg-develop/src/registry/table.rs --counts
```

A new key needs: its level (the same name can mean different things on
different levels, e.g. `Version`), the value kind, the range in the **stored**
unit, the UI scale if it is a slider, raw and non-raw defaults (`Unverified`
if unknown, `NoDefault` if Lightroom fills it per photo or only writes it when
a feature is used), the policy class
(`UNKNOWN` until someone has checked what it does), frame scope, file scope,
where it occurs and the first process version that has it. The invariants
(unique `(level, name)`, ranges contain defaults, `+` signs only on global
keys, raw-only keys absent for non-raw files, unobserved keys only UNKNOWN or
COMPUTED, ...) are unit tests in `registry/tests.rs` and catch many
half-filled rows. `Temp` is not a key: the plugin and the style engine's alias
table used to read and write it by mistake (see *Bugs found in the current
code* in [Dev-AI-Edit-XMP-Findings](Dev-AI-Edit-XMP-Findings)); it resolves to
unknown, and a test pins that. `tests/no_temp_key.rs` keeps it from coming back
as a key literal in `server-rs/crates/*/src` and the plugin's
`DevelopEditManager.lua` (comments excluded; the develop experiments, which
write `Temp` on purpose, are allowlisted, and so is the one constant the Lua
writer refuses it with, `RETIRED_TEMP_KEY` in `src/lua/write.rs`, via
`ALLOWED_LINES`, matched on `/`-joined paths so the Windows release job
agrees). Because it guards a plugin file, CI
also runs it in the unfiltered `format-lint-rust` job of `lint-format.yml`, not
only in `server-rs-tests.yml`, which skips plugin-only PRs.

Defaults: every training example is a raw file, so the non-raw column comes
from a readback instead. Experiment E13 read Lightroom's full develop table
for one non-raw photo without develop edits (Lightroom Classic 15.6, process
version 15.4, a virtual copy of a TIFF; [Findings: Results](Dev-AI-Edit-XMP-Findings#results-first-run-2026-10-03)),
committed scrubbed as `server-rs/testdata/develop/defaults/non_raw_lrc15.6.json`
(key -> value, plus the Lightroom version, process version and file kind).
`gen_table_rs.py` gives every global row the text leaves `Unverified` for
non-raw files the value read back, if the readback has the key (145 rows),
and stops when a readback contradicts the non-raw default `spec.py` states.
It holds back the keys in `NON_RAW_READBACK_NOT_CONSTANT`: `HDRMaxValue` read
2.3 (raw 4.0) on that one photo and may be per image, so its non-raw default
stays `Unverified`. `registry/tests.rs` (`non_raw_defaults_come_from_the_readback`)
requires every non-raw `Value` to match the file and no row the readback
covers to be left `Unverified`, except the held-back keys. Most equal the raw
default; the ones that differ are `Sharpness` 0 (raw 40),
`ColorNoiseReduction` 0 (raw 25), `CameraProfile` "Embedded" (raw "Adobe
Standard") and `IncrementalTemperature`/`IncrementalTint` 0 (absent for raw).
The policy filter reads this column: a non-raw source's removed value equal
to it is dropped without a report (`non_raw_defaults_are_removed_without_a_report`
in `model/policy.rs`). One virtual copy is thin evidence for a table: the
report's own check (a freshly imported JPEG and raw for the default tables)
is still open. A key the readback does not have stays `Unverified`:
absence can mean "written only when used", so no key was marked `Absent` on
this evidence. `NoDefault` means there is no fixed default for either file
kind (as-shot white balance, digests, `Look`, the Upright solver state),
which a readback does not change (19 readback keys kept it). Hand edits to
`table.rs` are mirrored in `spec.py`, so the generator reproduces the table
and a later readback (a raw default table, another Lightroom version) is one
regeneration plus a reviewed snapshot diff.

### Policy classes

| Class | Learn/blend | Shared preset | Apply to one photo | Test round trip |
|---|---|---|---|---|
| LEARN | yes | yes | yes | yes |
| LEARN† (gated) | yes, behind the gate | if the gate holds for every target | if the gate holds for the photo | yes |
| PHOTO | never averaged | no | yes, with the photo's own context | yes |
| COMPUTED | no | no | no | yes |
| NEVER | no | no | no | yes |
| META, global (`ProcessVersion`, `Version`, `Enable*`, ...) | no | no: set by the writer | no: set by the writer | yes |
| META, nested (correction, mask and structure bookkeeping) | no | yes | yes | yes |
| UNKNOWN | no | no | no | yes |

Gates: `RawOnly`/`NonRawOnly` (white balance), `FileKindDefault` (defaults
differ between raw and non-raw), `OnlyValues` (only some values transfer),
`CameraRestricted` (profiles), and the learning-side gates `Categorical`,
`CircularHue`, `DependsOn`, `ByMaskType`, `NeedsAiUpdate`.

`CircularHue(<key>)` names the saturation that weighs the angle
(`SplitToningShadowHue` → `SplitToningShadowSaturation`, `LocalToningHue` →
`LocalToningSaturation`, same level): the style engine treats each example as
a colour vector of that length and averages the vectors by score (see
`POST /v1/edit/style` in
[Dev-Backend-API](Dev-Backend-API)). A registry test checks that the named key
exists on the same level and cannot be negative.

### Recipe aliases

`recipe_alias` is the edit-recipe field a global key is learned and sent as
(`global.exposure`, `global.hsl.red.hue`, `global.color_grading.shadows.hue`,
`global.tone_curve.shadow_split`). Since step 1f every field the installed
plugin's `DevelopEditManager` applies has one: 65 keys (the basic and presence
sliders, detail, vignette, grain, the parametric curve with its splits, the 24
HSL values, colour grading shadows/highlights hue and saturation plus balance).
Deliberately without an alias: white balance (typed, `WbSetting`), colour
grading midtones/global/luminance/blending (installed plugins warn and drop
them), point curves, lens, crop, `Look`. Tests pin that aliases are unique,
only on learnable global keys, never a prefix of one another, and that the LLM
schema declares the registry's UI range for each (`edit_recipe.rs`). The
aliases live in `gen_table_rs.py`'s `ALIAS` table and are regenerated into
`table.rs` with the rest; adding one means bumping `CANONICAL_VERSION` in
`lrg-analysis::training`, so stored examples are re-read from their blob.

`CameraRestricted` on `CameraProfile` depends on the value: `Adobe Standard`
and the symbolic `Default Color`/`Default Monochrome`/`Default Profile` exist
for every camera and go everywhere (`PORTABLE_PROFILES`); any other name is
camera-specific and never goes into a shared preset. For one photo
(`ApplyPhoto`) it passes, because the model knows neither the example's camera
nor the photo's make: until the target carries the make, the caller must only
apply settings from the same make.

### Units

Lua tables and XMP files both carry the **stored** unit:
`LocalExposure2012` is EV / 4; the other signed `Local*` sliders,
`LocalToningSaturation` and `CorrectionAmount` are UI / 100;
`LocalToningHue` and globals are UI units. The model always holds the stored
unit; `registry::to_ui`/`from_ui` are the only conversion. A builder or blend
uses `from_ui_value`, which returns the model value of the key's kind
(`Value::Int` for `Contrast2012`, as the readers produce), so integer rounding
stays in one place. Keys whose scale is unverified (`LocalHue`, `LocalGrain`)
cannot be converted, so nothing can write them from UI values.

## The model

`DevelopSettings` holds global values in a map keyed by registry id (so the
order is the same in every build, whatever `serde_json` features the
workspace unifies). Invariant, enforced by `insert` (the map is private): only
global keys, and never `MaskGroupBasedCorrections` or `Look`, which are the
typed fields `corrections` and `look`. Everything the registry does not know
is kept verbatim in `opaque`, never dropped (from a Lua table sorted by key
name, so every build agrees).

Typed views exist only where logic hangs:

- **Corrections and masks.** A `Correction` has its adjustments (`local`, stored
  unit), its components in evaluation order, and everything else in `extra`.
  Components are recognised **by structure, never by name** (names are
  localised):

  | `MaskTool` | Recognised from |
  |---|---|
  | `Semantic::Subject` / `Sky` | `Mask/Image`, `MaskSubType` 1 / 2 |
  | `Semantic::Background` | `Mask/Image`, subtype 0, category 22 |
  | `Semantic::PeoplePart(part)` | subtype 3, a people-part category (preset form) |
  | `Semantic::Landscape(class)` | subtype 0, category 50001–50008 |
  | `Semantic::PersonPartAt { part, point }` | subtype 0, a people-part category, a `ReferencePoint` (one person; photo-specific) |
  | `Linear` | `Mask/Gradient` |
  | `Radial` | `Mask/CircularGradient` with `Angle` 0 |
  | `LuminanceRange` | `Mask/RangeMask`, `CorrectionRangeMask.Type` 2 |
  | `Opaque { what }` | anything else: brush aggregates, Select Object, colour/depth ranges, rotated radial gradients — kept for the round trip only |

  How a component combines with the ones before it:

  | `Combine` | `MaskBlendMode` | `MaskValue` | `MaskInverted` |
  |---|---|---|---|
  | `Add { inverted }` | 0 | 1 | `inverted` |
  | `Subtract` | 1 | 0 | false |
  | `Intersect` | 1 | 0 | true (A ∩ B = A − ¬B) |

  Any other combination stays as it is (`Combine::Unrecognised`, with a
  warning).
- **Look.** The profile is read whole from its source (name, UUID, amount,
  camera restriction, and the rest including `Parameters` verbatim). It is
  never rebuilt from a profile name. An empty `CameraModelRestriction` means
  "no restriction" and stays in the rest as it was. What of it leaves the
  model depends on the target (see [Policy filter and frame
  scope](#policy-filter-and-frame-scope)).
- **White balance.** `WbSetting { mode, family, temperature, tint }`. The family
  (`Temperature`/`Tint` for raw, `IncrementalTemperature`/`IncrementalTint`
  for non-raw) comes from the keys the settings carry, via the registry's file
  scope, not from the file extension: a DNG converted from a JPEG uses the
  non-raw family. The white-balance *policy* (what transfers) belongs to the
  style engine (`blend_white_balance`, see `POST /v1/edit/style` in
  [Dev-Backend-API](Dev-Backend-API)); what a preset may carry is the policy
  filter's: the numbers only next to a `Custom` (or absent) `WhiteBalance`,
  never an `As Shot` or `Auto` result turned into a custom white balance
  (see [Policy filter and frame scope](#policy-filter-and-frame-scope)).
- **Process version.** `process_version()`; PV2012 (`"6.7"`) is the lowest
  version whose settings are learned.

`Finite` is the float for everything that can reach a writer: it cannot be NaN
or infinite (JSON.lua and `serde_json` would both write `null`), and `-0.0` is
stored as `0.0`.

### Policy filter and frame scope

`DevelopSettings::filtered(target)` keeps what may go to a `Target`
(`Preset { mixed_file_kinds }`, `ApplyPhoto { file_kind, process_version,
camera }`, `TestRoundTrip`) and returns every removed value that differs from
its default as `Skipped { path, reason }`, so a caller can say what did not
transfer. A correction goes or stays whole, decided by its most restrictive
component (semantic masks, except a person part at a reference point, and
luminance ranges are LEARN; gradients, Select Object and colour ranges PHOTO;
brushes NEVER). A camera-restricted `Look` only goes to that camera. In a
preset, white-balance numbers (`Temperature`, `Tint`, `Incremental*`) go only
with a custom white balance: next to any other `WhiteBalance` mode (`As
Shot`, `Auto`, `Daylight`, ...) they are what that mode resolved to for the
source photo, so they are dropped and reported as
`SkipReason::WhiteBalanceMode(mode)` instead of pinning one photo's white
balance on every photo (`WhiteBalance` itself is PHOTO; writing `As Shot`
into a preset is a later policy change).
`ApplyPhoto` also drops keys newer than the photo's process version and keys
of the other file kind. A single-kind preset (`Preset { mixed_file_kinds:
false }`) is for photos of the settings' own file kind: it drops the other
kind's keys, and when the settings carry both white-balance families with no
known file kind (the readers warn `ConflictingFileKind`) it drops both
families, since either would make the preset contradict itself. A mask
component whose `MaskBlendMode`/`MaskValue`/`MaskInverted` form no known
combination (`Combine::Unrecognised`) is UNKNOWN, so its correction reaches
neither a preset nor a photo: what it does to the mask is not known. Both
rules came out of the writer's round-trip tests (a preset written from such
settings read back with a warning).

The verdict is per key at every depth: a kept structure is rebuilt from the
fields that pass at their own registry rows, and each removed field is
reported with its full path (`LensBlur.SampledArea`,
`MaskGroupBasedCorrections[0].CorrectionMasks[1].CorrectionRangeMask.LuminanceDepthSampleInfo`).
So a shared `LensBlur` keeps `Active` and the look sliders but not its sampled
area or focal range, and a luminance range loses its eyedropper sample. A
registry value the reader kept whole (`Value::Opaque`: an `rdf:Bag`,
qualifiers, a Lua table of an unexpected shape) reaches neither a preset nor
a photo and is always reported (`SkipReason::Opaque`). The `Look` goes to one
photo whole (its profile record as the source has it) and to a preset by
reference: `Name`, `UUID`, `Amount` (and the META `Stubbed` when present),
without its COMPUTED fields — `Parameters` (the profile definition, whose
`Table_*` blobs a preset never carries), `Group`, `Cluster`, `Copyright`,
`SortName`, `Supports*`, `isAdobeAdaptive`. Those are copies Lightroom takes
from its profile library, none of Adobe's 118 bundled presets with a `Look`
has `Parameters`, and they are dropped without a report (nothing the user
could act on). Adobe's presets themselves use either the name alone or a
stub (`Stubbed="true"` with the profile's `Group`, `Cluster`, `Copyright`
and `Supports*`); the writer's form, `Name`/`UUID`/`Amount`, is untested in
Lightroom until the E11 experiment. A range mask's `SampleType` is UNKNOWN
(default 0) and is dropped without a report; the XMP writer emits Adobe's
fixed range-mask form (`Version` 3, `SampleType` 0) itself (see [Preset
mode](#preset-mode)).

Global META values (`ProcessVersion`, `Version`, `CompatibleVersion`,
`ToneCurveName2012`, `HasCrop`, the `Enable*` panel switches, ...) never leave
for a preset or a photo, and are not reported: they describe the example, and
the writer sets them from the target's context. Copying an example's
`ProcessVersion` would silently change the target photo's process version.
Nested META (a correction's name and sync id, a range mask's `Version`) stays
for now.

`split_by_frame_scope()` sorts values into shareable, per-frame-adjusted,
per-frame-only and not-applicable parts — the basis for sharing one recipe
across a series later.

## Reading the Lua form

`lua::from_lua_str(text, hint)` / `from_lua_value(&json, hint)` read the
`develop_settings` blob the plugin sends (JSON.lua's encoding of
`photo:getDevelopSettings()`):

- a top-level `[]` is the empty table; an empty table (`[]` or `{}`) as the
  value of any key, at any depth, means the key is absent (`LensBlur`,
  `AILook`, `FilterList`, ...), and an empty table as an element of a list of
  objects (corrections, masks, structures) is no element, all without a
  warning;
- an integer where a real is expected is fine; a non-integer on an integer key
  is rounded half-to-even and warned about;
- booleans and 0/1 are accepted on bool and 0/1-flag keys and normalised;
- a known key with the wrong shape (`null` in an array, a mixed array, text for
  a number) is kept verbatim with a `WrongType` warning and never written;
- an unknown key is kept verbatim with an `UnknownKey` warning — never an
  error; pattern-family keys are kept verbatim without a warning;
- the file kind comes from the white-balance keys; a caller hint (`is_raw`)
  that contradicts them is warned about and loses;
- a process version below 6.7 is warned about (such settings are not learned);
- keys are read in name order, so opaque entries and warnings come out in the
  same order whatever `serde_json` features the build unifies.

`ParseError` is only for input that is unusable as a whole: invalid JSON,
more than 4 MiB, or a top level that is not a table. Warnings are a list with
a path such as `MaskGroupBasedCorrections[2].CorrectionMasks[0].MaskSubType`.

## Reading XMP

`xmp::parse(bytes)` reads a sidecar, a develop preset or a profile into an
`XmpDocument { kind, header, develop, warnings, skipped_subtrees }`. It lands
in **the same model the Lua reader produces**: the same key ids, value
variants, correction and mask classification, `Look` and white-balance family.
Both readers only walk their own syntax; typing (through `KeySpec::coerce`),
the checks on closed sets and ids, curve points, the file kind and the
process-version check live once in the crate-private `reader` module, and the
correction/mask classification in `model::correction`.
`tests/xmp_lua_equivalence.rs` holds the two readers to that. What differs by
design, and what those fixtures therefore avoid or strip:

- global keys of only one format (`Presence::XmpOnly`/`LuaOnly`);
- `PerFormat` keys (`PointColors`, `RetouchInfo`): a `StructList` from Lua, a
  `StrList` from XMP;
- `ValueKind::Any` and `Settings` keys (`RGBTables`, `ProfileGainTableMap`,
  `Look.Parameters`, ...): `Opaque::Json` from Lua, `Opaque::Xmp` from XMP;
- struct-, correction- and mask-level Lua-only keys (`CorrectionID`,
  `CorrectionReferenceX/Y`, `MaskID`, `Look.isAdobeAdaptive`);
- Lightroom writes `PointColors` into every sidecar as a sentinel item
  (19 × `-1`) where `getDevelopSettings()` returns `[]`, so an XMP model
  reports one point colour that the Lua model does not.

Pairing real training rows with their sidecars (September 2026, key names
only) found no key present in both formats with a different value variant,
and identical mask tools and combine sequences. It also found `Presence`
rows the data contradicts, a follow-up for the registry (not changed yet):
seen only in XMP although marked `Both` — `Version`, `CompatibleVersion`,
`CameraProfileDigest`, `OverrideLookVignette`, `ColorVariance`,
`AutoToneDigest`, `AutoToneDigestNoSat`, `ToggleStyle*`, `AllowFilters`,
`RangeMaskMapInfo`, `Upright*DependentDigest`; seen only in Lua — the PV2010
keys (`Exposure`, `Contrast`, `Brightness`, `Shadows`, `FillLight`,
`HighlightRecovery`), the `Auto*` flags and `ExtendedToneCurvePV2012*`.

Two passes. The first turns RDF into the XMP data model (`XmpNode`), whatever
the syntax: attribute form and element form, structures as a nested
`rdf:Description`, as attributes on the property element or with
`rdf:parseType="Resource"`, `rdf:Seq`/`rdf:Bag`/`rdf:Alt` with empty items,
`xml:lang` and qualifiers (`rdf:value`, as an element or an attribute). Every
`rdf:Description` under every outermost `rdf:RDF` is merged in document
order. Top-level properties outside `crs:` are counted and never built. A
property element that mixes forms RDF does not allow (a container next to
other elements or field attributes, something other than `rdf:li` in a
container) is read as a plain structure of everything it holds, so the
second pass reports it (`WrongType`/`UnknownKey`) and keeps it whole; text
next to elements, which the data model cannot hold, is dropped with one
`MalformedRdf` warning per top-level property (the first place, and how many
more). Namespace URIs are shared (`Arc<str>`), not copied per field, so a
long URI cannot multiply into gigabytes. The second pass types the `crs:`
properties through the registry:

- the namespace is matched by URI (`http://ns.adobe.com/camera-raw-settings/1.0/`),
  never by the `crs:` prefix;
- top-level names resolve at the global level first, then as preset header
  keys (`Baseline` is a header key though it looks like a setting);
- `True`/`true`/`False`/`false` are all booleans, `"+15"` is 15, `"1.000000"`
  is 1.0; a non-integer on an integer key is rounded half-to-even with a
  warning, as in the Lua reader; ids are checked for 32 hex digits;
- global curves (`"x, y"`) and local curves (`"x,y"`) both become
  `Value::Curve` — the same value the Lua reader makes of a flat number list
  and of `"x,y"` strings;
- an `rdf:Alt` with only `x-default` is `Value::Alt`. One with further
  languages, an `rdf:Bag` or `rdf:Alt` where the registry expects an
  `rdf:Seq`, and any value with qualifiers are kept whole under their key as
  `Opaque::Xmp` (lossless, no warning: valid XMP the model does not
  interpret; `XmpNode::alt_default` still finds the default language). Each
  such key is counted in `skipped_subtrees.kept_whole`, so the lost typing
  is visible; the local sweeps require 0 for Lightroom's own files;
- `crss:` (snapshots) is skipped entirely; `Look.Parameters` (a profile
  definition) and `Preset.Parameters` (the last applied preset) are kept
  opaque and never evaluated; properties in other namespaces (`dc:`, `xmp:`,
  `exif:`, ...) are not part of the model. `skipped_subtrees` counts each;
- an unknown `crs:` key, or a foreign property inside a develop structure, is
  kept verbatim (with its namespace) with an `UnknownKey` warning; a known key
  of the wrong shape with `WrongType`; a key that appears twice at one level
  (Adobe's own bundle has profiles with `Group` twice in one description)
  keeps the first occurrence — typed, or opaque if it did not type — and
  keeps later ones verbatim with `DuplicateKey`.

The kind: a **sidecar** has `crs:ProcessVersion` and no `crs:PresetType`; a
**preset** has `PresetType="Normal"`, a **profile** `"Look"` (its settings are
the profile definition); anything else is `NotDevelop`, which is not an error.
The header's common fields are typed (`PresetHeader`: type, UUID, the `x-default`
names, cluster, camera restriction, the four `Supports*` flags); every other
header key stays in `PresetHeader::rest`.

`ParseError` only when the file is unusable as a whole: more than
`MAX_XMP_BYTES` (4 MiB, the sidecar limit of `lrg-imaging`), not UTF-8 after
an optional byte-order mark, not well-formed XML (a DTD counts as that, and
so do more than 2^18 XML nodes), no `rdf:RDF`, elements nested deeper than
`MAX_XMP_DEPTH` (64) below a description (`TooDeep`; real files nest about
20), or beyond a structural limit (`Limit`): more than `MAX_XMP_ATTRIBUTES`
(1024) attributes on one element, more than `MAX_XMP_NAMESPACES` (256)
namespace declarations, or a namespace URI longer than
`MAX_XMP_NAMESPACE_URI` (1024 bytes); real files stay near 200, 20 and 60.

The depth and structural limits are enforced by a linear, non-recursive
pre-scan of the text before roxmltree sees it. roxmltree's tokenizer recurses
once per element with no depth limit, so without it a deeply nested file
overflows the stack inside the XML parser — an abort that takes down the
whole backend, not a catchable panic — and its duplicate-attribute check and
namespace resolution are quadratic, so a file under 4 MiB could take minutes
to hours. The reader's own recursion checks the depth a second time.
Filtering `* [conflicted].xmp` sync copies is the caller's job, not the
parser's.

### Checked against real files

The fixtures cover the forms; whether the registry covers what Lightroom
actually writes is checked by the local sweeps in `tests/xmp_goldens_local.rs`
(see [Tests](#tests) for how to run them). Last run (September 2026,
Lightroom Classic 15.6):

| Source | Files | Result |
|---|---|---|
| Lightroom Classic's bundled presets and profiles (`Contents/Resources/Settings`) | 1,045 (446 presets, 599 profiles) | 0 parse errors, 0 unknown keys, 0 values outside a registry range; the only warnings are 6 `DuplicateKey Group` |
| A sidecar corpus of 28,680 `.xmp` files | 10,814 Lightroom sidecars (PV 11.0 and 15.4, 5,523 corrections) | 0 parse errors, 0 warnings of any kind |
| Camera Raw's user folder: its presets and profiles | 23 | 0 parse errors, 0 warnings |

The `Group` duplicates are Adobe's: some bundled camera profiles carry
`crs:Group` twice in one description, with different values. The reader keeps
the first and warns; the sweep allows exactly that pair (`ADOBE_ALLOWED`) and
nothing else. Camera Raw's folder also holds its own state in `crs:` form
(`Defaults/Preferences.xmp`, `Defaults/Previous.xmp`, `GPU/…`), with keys no
preset or sidecar carries (`JPEGHandling`, `SubsetRetouch`, …); those are not
develop documents, so that sweep checks only presets and profiles and counts
the rest by kind. No registry row had to be added for any of these sources,
and no registry key arrived in a form the model keeps whole
(`kept_whole` is 0 in all three). The corpus is counted as the non-conflicted
files carrying `crs:ProcessVersion` as of September 2026, so it differs from
the photo count in the research.

October 2026 re-run of the corpus: 28,681 files, 10,815 sidecars parsed
(8,006 at PV 15.4, 2,809 at 11.0; 5,524 corrections). One new sidecar,
written by the Camera Raw 18.7 engine, carried `crs:Glow`, the switch of the
Glow panel whose `GlowRange`/`GlowWarmth`/`GlowSpread`/`GlowStyle` rows
already existed (unobserved); the full sweep failed on it (`UnknownKey
Glow`) until it got its own row (global, `Any`, UNKNOWN, observed in XMP
only so far). With the row the sweep is clean again: 0 parse errors, 0
warnings, nothing kept whole. The other September counts on this page are
from the 10,814-sidecar run.

## Writing XMP presets

`xmp::write(&settings, &WriteMode) -> Result<Written, WriteError>` writes a
develop preset Lightroom Classic imports (Develop → Presets → Import). It is
hand-written and deterministic: the same settings give the same bytes in every
build (registry order for keys, the reader's order for opaque content, never a
`serde_json` map). `Written { xmp, skipped }` returns the document and every
value that did not make it, so a caller can say what was left out.

```rust
let header = PresetHeader::lrgenius(uuid, "LrGenius · Warm evening");
let mut spec = PresetSpec::new(header); // mixed_file_kinds: true, no process version
spec.process_version = Some(ProcessVersion::V6);
let Written { xmp, skipped } = xmp::write(&settings, &WriteMode::Preset(spec))?;
```

### Preset mode

`WriteMode::Preset(PresetSpec { header, mixed_file_kinds, process_version })`
is the only production mode:

1. The settings go through `filtered(Target::Preset { mixed_file_kinds })`
   first; only keys whose policy reaches a shared preset are written, and the
   filter's `Skipped` reports become `Written::skipped`. A mixed preset (the
   default) carries no raw-only or non-raw-only key (`Temperature`, `Tint`,
   `Incremental*`), no crop, geometry mask or lens identity; a single-kind
   one keeps only its own kind's keys, and white-balance numbers only next to
   a `Custom` (or absent) `WhiteBalance`; the `Look` goes by reference,
   without its profile definition (see [Policy filter and frame
   scope](#policy-filter-and-frame-scope)).
2. The writer sets what describes the file rather than the settings:
   - the header: `PresetType="Normal"` (always; `Look` profiles cannot be
     written), `UUID` (required, upper-cased), `Name` (required), `ShortName`,
     `SortName`, `Group` (`LrGeniusAI` from `PresetHeader::lrgenius`) and
     `Description` as `rdf:Alt` with an `x-default` item (empty ones as
     `<rdf:li xml:lang="x-default"/>`), `Cluster`, `CameraModelRestriction`,
     `Copyright`, `ContactInfo` (empty by default), the four `Supports*` flags
     of the header (default `True`), and the values Adobe's newer (Camera
     Raw >= 14.4) presets carry: `SupportsHighDynamicRange`,
     `SupportsNormalDynamicRange`, `SupportsSceneReferred`,
     `SupportsOutputReferred` `True`, `RequiresRGBTables` `False` (163 of the
     446 bundled develop presets have these keys — exactly the ones with
     `SupportsAmount2`, every adaptive preset included; the older 283 have
     none). `RequiresRGBTables` `False` is right because a preset never
     carries an RGB table. `Name` must not be blank
     (`WriteError::MissingHeaderField`). Further META header keys in
     `PresetHeader::rest` (`ShowInPresets`, ...) are written; anything else
     there is skipped and reported;
   - `Version="18.5"` (`TARGET_ENGINE`: the oldest engine targeted,
     Lightroom Classic 15.5.1's Camera Raw 18.5.1; the installed 15.6 is
     newer), `CompatibleVersion` from the feature table below,
     `ProcessVersion` only when `PresetSpec::process_version` is set (the
     settings' own process version describes the example and never reaches a
     preset), `HasSettings="True"`;
   - `WhiteBalance="Custom"` whenever white-balance numbers are written
     (only a custom or absent source mode lets them through), and
     `ToneCurveName2012` (`Linear` or `Custom`) next to a point curve (a
     typed one; a curve kept whole never reaches a preset).
3. It completes the fixed form of Adobe's adaptive presets for what the
   policy filter removes as per-photo or a builder may leave out (present
   values are kept): a correction's `What="Correction"`, `CorrectionAmount`
   1, `CorrectionActive`; a component's `MaskActive`; an AI mask's
   `MaskVersion` 1, `ReferencePoint="0.500000 0.500000"`, `ErrorReason` 0; a
   luminance range's `CorrectionRangeMask` `Version` 3 and `SampleType` 0.
   Builders therefore need not set these. What this form puts back without a
   loss is taken out of `Written::skipped` again, so a caller that shows
   `skipped` to the user (as step 2 will) does not warn about it: an AI
   mask's `ReferencePoint` and `ErrorReason` (Lightroom recomputes both per
   photo; Adobe's presets carry the centre and 0 whatever photo they were
   made on), a luminance range's `SampleType` 0, and `WhiteBalance="Custom"`
   next to its numbers. A people part's (`MaskSubType` 3) `ReferencePoint`
   away from the centre stays reported, because the point may pick the
   person; a non-zero `SampleType` too.
4. A last guard refuses the never-write list (`NEVER_WRITE`) whatever the
   policy says: every `*Digest*` key, `CorrectionID`, `MaskID`,
   `CorrectionReferenceX/Y`, `FullMaskSize`, `WholeImageArea`, `Origin`,
   `ModelVersion`, `MaskBrushTable*`, `Dabs`, `RetouchAreas`, `RemoveAreas`,
   `RetouchInfo`, `RedEyeInfo`, `FilterList`, `AILook`, `DepthMapInfo`,
   `orientation`, `AutoWhiteVersion`, `RawFileName`, `AlreadyApplied`, the
   global `Enable*` switches, and every Lua-only or never-observed key. Pattern
   families (`Table_*`, `pm_*`, ...) are opaque and never reach a preset. A
   unit test pins that the policy filter already removes each of them, so the
   guard only catches a future filter bug. A registry value kept whole
   (`Opaque::Xmp`) is likewise skipped and reported by the writer itself,
   should the filter ever let one through.

Content read from a Lua table (`Opaque::Json`) has no XMP form: the filter
removes it and reports it (a Lua example's `Look.Parameters` is COMPUTED and
goes silently, as from XMP); converting it is step 1g's business.

`CompatibleVersion` (`crs:CompatibleVersion`, packed `major << 24 | minor <<
16`) is the highest row a preset's content needs, capped at the engine, and
absent when no row applies (as in most of Adobe's own presets without masks).
The rows are the lowest values Adobe's bundled presets use for the same
feature (`COMPATIBLE_VERSIONS`):

| Feature | Version | Evidence |
|---|---|---|
| any mask group, subject, sky | 14.0 | adaptive Subject/Sky presets |
| people parts | 15.0 | adaptive Portrait presets |
| background | 15.0 | not in the bundle; Select Background shipped with the people masks — unverified: the hand test only shows that the installed (newer) Lightroom imports it, and an import into 15.x cannot check a minimum |
| landscape classes | 15.3 | adaptive Landscape presets |
| point curves inside a correction | 15.3 | adaptive presets with `MainCurve`/`BlueCurve` |
| `LensBlur` | 16.0 | Blur Background presets |
| global `CurveRefineSaturation` | 17.0 | presets with that curve |
| `LocalPointColors` | 17.4 | adaptive presets with point colour in a mask |

The writer computes the version from the *filtered* settings, so two rows
never apply to a preset it writes: `LocalPointColors` (UNKNOWN) and a person
part at a point (PHOTO, the other half of the people-parts row) never pass
the filter. They stay for `compatible_version` on unfiltered settings.

### Document shape and spelling

One `rdf:Description` in the `crs:` namespace under `x:xmpmeta` with
`x:xmptk="LrGeniusAI <backend version>"` (never an Adobe toolkit string), in
Adobe's layout: header attributes, version stamps, global values,
`HasSettings`, then the `rdf:Alt` names, then `rdf:Seq`s and structures as
elements. Simple values are attributes; a structure of simple values only is
written as attributes on its property element or `rdf:li` (mask components),
one with a complex field as a nested `rdf:Description` (corrections, a range
mask's component). `rdf:parseType="Resource"` and `rdf:Bag` are never
written. Numbers and the rest follow `xmp::format`:

| Rule | Example |
|---|---|
| `NumFmt::Int`: no decimal point | `Temperature` 3569 → `3569` |
| `NumFmt::Fixed(n)`: exactly n decimals | `Exposure2012` 0.5 → `+0.50`, `SharpenRadius` 1 → `+1.0` |
| `NumFmt::Trim6`: up to 6 decimals, trailing zeros trimmed | `LocalExposure2012` 0.0625 → `0.0625`, `CorrectionAmount` 1 → `1` |
| `NumFmt::CompoundFixed6`: `%.6f` each, one space apart | `ReferencePoint` → `0.500000 0.500000`, `LumRange` |
| `+` only on `plus_sign` keys at the global level, only above 0 | `Tint` 6 → `+6`, 0 → `0`, -6 → `-6`; never in a structure |
| never `-0` | -0.001 at 2 decimals → `0.00` |
| booleans by level | `True`/`False` global and header, `true`/`false` in structures |
| curves | global points `x, y`, local points `x,y` |
| ids | 32 upper-case hex digits (`UUID`, `CorrectionSyncID`, `MaskSyncID`) |
| escaping | `&` `<` `>` `"` `'` as entities; tab, line feed, carriage return as character references in attributes (a parser would turn them into spaces), a carriage return also in element text; UTF-8 unchanged; a character XML 1.0 cannot hold is a `WriteError`, never a broken file |

A value whose variant does not fit its key (text for `Exposure2012`) is a
`WriteError::WrongValue`, not a guess.

### Test round trip

`WriteMode::TestRoundTrip(Option<PresetHeader>)` writes everything the model
and the header hold — opaque, COMPUTED, NEVER and UNKNOWN content, `rdf:Bag`s,
language alternatives, qualifiers and foreign namespaces inside structures —
so XMP → model → XMP → model compares equal. Numbers keep their registry
spelling unless it would change the value; then the shortest exact text is
written (Lightroom never writes such a number). It exists only behind the
crate feature `test-roundtrip`, which only `lrg-develop`'s own
dev-dependency on itself switches on (`[dev-dependencies] lrg-develop = {
path = ".", features = ["test-roundtrip"] }`): `cargo tree -p lrg-server -e
features -i lrg-develop` shows only `default`. Any build that compiles
`lrg-develop`'s tests (`cargo test --workspace`, `cargo clippy --workspace
--all-targets`, so CI and pre-commit) unifies the feature into the
`lrg-develop` every crate links, so CI cannot catch another crate naming
`TestRoundTrip`; only the release build would fail. Never name it outside
`lrg-develop`; `WriteMode` is `#[non_exhaustive]`, so a match elsewhere
always needs a wildcard arm. **Never write a sidecar with
it**: the model drops every namespace but `crs:` (`dc:`, `xmp:`, `exif:`,
`tiff:`, `aux:`, `photoshop:`, `xmpMM:`) and `crss:` snapshots, so a rewritten
sidecar would lose the photo's metadata. Writing into an existing sidecar
needs a merge into that document (later step).

Round trip 1 of the plan (XMP → model → XMP → model) runs with this mode in
`tests/xmp_roundtrip.rs` over every committed fixture and writer golden, and
in the local sweeps over Adobe's and the maintainer's files (see
[Tests](#tests)). Last local run (September 2026): all 1,045 bundled presets
and profiles, the 23 presets and profiles of Camera Raw's user folder and all
10,814 corpus sidecars write without an error, read back into an equal model
(header, kind and opaque content included, no key differing), and write again
byte-identically. The same sources written as presets (production mode, mixed
and single-kind: the 446 bundled develop presets, the 4 Camera Raw presets,
all 10,814 sidecars, and the 1,294 training-dump rows) write without an error
and read back as presets with no warning, no key outside the preset policy,
nothing kept whole, and exactly the policy-filtered settings (2.3 million
values compared over the corpus, none missing, changed or extra).

The byte rules against the source (plan §7.4) are part of the same sweeps:
every `crs:` value text of the source (attributes, text-only elements,
`rdf:li` items, outside `crss:` snapshots) is compared with the lossless
output as a multiset per file, and anything but a known variance fails.
Known variances, all keys Adobe spells unusually and rarely: over the
10,814 corpus sidecars the legacy PV2010 `Clarity` (1 value, written with a
`+` the row does not have) and `CropAngle`/`CropBottom`/`CropTop` in two
sidecars that spell them untrimmed at `%.6f` (all never in a preset); in
Adobe's bundle `Look.Amount` (`1.000000` where sidecars and the writer write
`1`, 35 values), legacy `Clarity` and legacy `Exposure` (two decimals, the
row is `Trim6`), `PerspectiveX`/`PerspectiveY` in `Classic/General/Zeroed.xmp`
(`0` against `Fixed(2)`), one `LocalTemperature="0.10"` in an Adobe Sky preset; in
Camera Raw's folder one `LocalShadows2012` with two decimals. The
registry rows keep Lightroom's sidecar spelling. Follow-up for the registry
(round-trip spelling only, both COMPUTED): legacy `Clarity` has no
`plus_sign`, and legacy `Exposure` is `Trim6` where Adobe writes two
decimals.

## Writing the Lua form (provisional)

> **Provisional while fields are open.** Experiments E1, E2, E4 and E11 show
> which forms `applyDevelopSettings` and plugin presets accept on the write
> side ([Findings: Running the experiments](Dev-AI-Edit-XMP-Findings#running-the-experiments)).
> The writer decides none of them: every alternative an experiment measures
> that shapes the table, or the call that applies it, is one `LuaOptions`
> field, the defaults sit in one place, `LuaOptions::PROVISIONAL`, and how
> far the evidence carries each of them is `LuaOptions::EVIDENCE` (the table
> below). The first run (Lightroom 15.6, 2026-10-03, one non-raw TIFF, run
> from the Library module;
> [Findings: Results](Dev-AI-Edit-XMP-Findings#results-first-run-2026-10-03))
> settled the AI-mask forms for subject, sky
> and background and the non-raw mode-only choices (`wb_mode_only`,
> `flatten_auto_now`). `wb_custom_with_numbers` is only supported, because
> `Custom` with the incremental numbers (the writer's non-raw form) was never
> applied, and so is `panel_switches`. People parts, `int_flag_as`,
> `look_form` and the raw side are open, and the wire goldens are not frozen.

`lua::to_lua_value(&settings, &LuaMode, &LuaOptions) -> Result<LuaWritten,
WireError>` writes the table `photo:applyDevelopSettings()` takes, as the
JSON the plugin's JSON.lua decodes into it. `LuaWritten { table, skipped,
call }`: the table (always a JSON object; `is_empty()` when nothing reaches
the photo), every value that did not make it (as `xmp::Written::skipped`),
and `ApplyCall { update_ai_settings, flatten_auto_now }`, the call
arguments that go with the table.

```rust
let photo = PhotoContext { file_kind: Some(FileKind::Raw), process_version: Some(ProcessVersion::V6), camera: None };
let LuaWritten { table, skipped, call } =
    lua::to_lua_value(&settings, &LuaMode::Apply(photo), &LuaOptions::default())?;
```

Two modes, as for XMP:

- `LuaMode::Apply(PhotoContext { file_kind, process_version, camera })` is
  production. The settings go through `filtered(Target::ApplyPhoto { .. })`
  first: PHOTO values pass (the settings are meant for this photo; crop,
  gradients, a person's reference point), gates, file-kind keys, `min_pv`
  and camera-restricted profiles are checked against the photo, and every
  removed non-default value is reported. An unknown `file_kind` gets
  no white-balance numbers and no raw-only or non-raw-only key.
- `LuaMode::Preset(PhotoContext)` is the same table for a plugin preset
  applied to that one photo (`addDevelopPresetForPlugin`, the E4 route),
  plus `SupportsAmount`/`SupportsAmount2` when `preset_amount_flags` asks
  for them. It is the only mode that honours that field, so a preset-route
  answer stored in `PROVISIONAL` cannot leak into `applyDevelopSettings`
  tables.
- `LuaMode::TestRoundTrip` (feature `test-roundtrip`, as for XMP) writes
  everything the model holds, opaque content read from a Lua table
  included; content kept as XMP has no Lua form and is a
  `WireError::NotLua`. Of the options only the encodings apply
  (`mask_enum_as`, `int_flag_as`), and the reader reads each of them back
  (an integer string on a mask enum is read as that integer, without a
  warning), so round trip 2 holds whichever form E2 settles on.

### Rules (plan §4.1, writer rules 1–10)

1. Keys without `crs:`; only what the policy filter lets through for the
   photo, then a never-write guard (`lua_never_written`): the XMP writer's
   `NEVER_WRITE` list (`CorrectionID`, `MaskID`, `CorrectionReferenceX/Y`,
   `FullMaskSize`, `WholeImageArea`, `Origin`, `ModelVersion`, retouch and
   filter payloads, `orientation`, ...), every key with `Digest` in its
   name, `MaskBrushTable*`, the global `Enable*` switches (only
   `panel_switches` writes them) and XMP-only keys. The guard's
   drops are reported. Global META is set by the writer, never copied: no
   `ProcessVersion` (it describes the example), `ToneCurveName2012` derived
   from the curve, `WhiteBalance="Custom"` next to numbers.
2. Integer keys as JSON integers (a real on an integer key goes through
   `KeySpec::coerce`, the one rounding place; a value no integer key can
   take is a `WireError::WrongValue`), real keys as JSON numbers at full
   precision. Every number is a `Finite`, so never `null`.
3. Booleans as JSON booleans; 0/1 flags (`IntFlag`) per `int_flag_as`.
4. Numeric-looking strings stay strings: `ProcessVersion`,
   `ReferencePoint`, `LumRange`, `FocalRange`, `Look.UUID`, local curve
   points. A compound number string keeps its spelling (Lightroom's tables
   mix `0` and `0.170630`) but must be a list of numbers
   (`WireError::NotCompound`); only what the writer builds itself
   (`LumRange` of a luminance range, a person's `ReferencePoint`) is spelled
   `%.6f`. Ids are upper-cased and must be 32 hex digits.
5. Global curves as a flat number list `[x, y, x, y, ...]` (integers where
   they are), at least two points (`WireError::ShortCurve`); local curves
   as a list of `"x,y"` strings.
6. `MaskGroupBasedCorrections` as a list in panel order, `Local*` directly
   on the correction, `CorrectionMasks` never empty. Every correction gets
   `What`, `CorrectionAmount` 1 and `CorrectionActive` when the model has
   none, and (`local_form = AdaptivePreset`) the 23 `Local*` keys of
   Adobe's adaptive presets at 0 where the model has no value
   (`ADAPTIVE_PRESET_LOCALS`, the list `DevelopExperiments.lua` applies,
   pinned against it by a test; its legacy PV2010 keys are written only as
   this 0). Every component gets `MaskActive`; `mask_form` decides the
   rest.
7. Never an empty container: JSON.lua encodes `{}` as `[]`, and an empty
   `MaskGroupBasedCorrections` would delete every mask of the photo. When no
   correction reaches the photo the key is absent; a structure left empty
   by the filter is absent (the reader reads both as "absent" too).
8. No mixed tables, no sparse arrays: no `null`, one JSON type per list.
9. White balance as a family. After the filter only the photo's family is
   left; numbers go with `Custom` or no mode (and get `"Custom"` per
   `wb_custom_with_numbers`); numbers next to any other mode (`As Shot`,
   `Auto`, `Daylight`, ...) are what that mode resolved to for the source
   and are reported as `SkipReason::WhiteBalanceMode`; a named mode
   without numbers is written only with `wb_mode_only` (reported otherwise,
   `As Shot`, the default, silently). `Custom` without numbers (they
   belonged to the other family, or the file kind is unknown) and a mode
   outside the registry's list are always skipped and reported: `Custom`
   alone would pin whatever the photo has. `Temp` is never written:
   `RETIRED_TEMP_KEY` is the one place the name is spelled, refused at any
   depth.
10. The forms the experiments decide are `LuaOptions` (below).

Rules 7–9 are checked once more on the finished table (`check_wire`, public
for the apply layer); a failure is a writer bug and a
`WireError::NotWireSafe`, never a table handed to Lightroom.

The `Look` follows `look_form`. The stub forms carry `Name`, `UUID`,
`Amount` (and `Stubbed`): the profile's COMPUTED fields (`Parameters`,
`Group`, `Supports*`, `CameraModelRestriction`, `isAdobeAdaptive`, ...) are
copies Lightroom fills in from its profile library when it resolves the
stub, so dropping them is not reported (as in a preset). A camera
restriction is not reported either: the policy filter has already kept a
restricted Look to its own camera, so the stub that reaches the photo loses
nothing. An Adobe Adaptive Look (`isAdobeAdaptive`) sets
`ApplyCall::update_ai_settings` (with `ai_update`) in every form, since its
`AILook` state is never written. `LookForm::Full` writes the whole record,
with `Parameters` only when it was read from a Lua table and passes
`check_wire` — the one place kept-verbatim content reaches Lightroom, and
only on request. No experiment applies a camera-restricted or an adaptive
Look (E11 uses Adobe Raw profiles and rejects adaptive ones).

Number spelling is not part of the contract: JSON.lua under Lua 5.1 (the
Lightroom runtime and CI's busted) re-encodes with `%.14g`, a local Lua 5.5
with the shortest exact form. Tests compare decoded values, never encoded
text.

### Options and what decides them

Each field's default and status, as `LuaOptions::PROVISIONAL` and
`LuaOptions::EVIDENCE` hold them (a test requires one evidence row per
field). **Settled** means the first run applied the default and it did what
the writer needs, on Lightroom 15.6 with one non-raw TIFF, run from the
Library module; it says nothing about raw files, other versions or the
Develop module. **Supported**: the evidence points to the default without
settling it: the default was not applied as written, or was applied where it
could make no difference. **Open**: no
experiment has answered it. An alternative no experiment applies stays
unmeasured whatever the run says.

| `LuaOptions` field | Alternatives | Default | Evidence (first run, LrC 15.6, one non-raw TIFF, Library module) | Still open | Status |
|---|---|---|---|---|---|
| `mask_enum_as` | `Number` / `String` (mask enums, `KeySpec::is_mask_enum`: every closed integer set below the global level — `MaskSubType`, `MaskBlendMode`, `ErrorReason`, `CorrectionRangeMask.Type`/`SampleType`, ... — and `MaskSubCategoryID`) | `Number` | E2a–E2d apply mask enums as numbers; the subject, sky and background masks are kept and computed (a hair mask ended `failed`, `ErrorReason` 1). Read back: a number in 655 of 655 masks of the training dump | `String` applied by no experiment (not needed) | settled (E2, non-raw) |
| `int_flag_as` | `Number` (0/1) / `Bool` (`LensProfileEnable`, `AutoLateralCA`, `CropConstrainToWarp`, `HDREditMode`) | `Number` | no experiment writes a flag key; read side only: numbers in 1,294 of 1,294 training rows and in the E13 readback | a write of a flag key, numbers against booleans (cheap evidence: read back photos `DevelopEditManager.lua` applied `AutoLateralCA` to as a boolean) | open |
| `panel_switches` (the plan's `include_panel_switches`) | `None` / `MaskOnly` (`EnableMaskGroupBasedCorrections` next to corrections) / `All` (plus the switch of every panel the table touches, `PANEL_SWITCHES`) | `MaskOnly` (was `None`) | E2 sent the switch, but E13 shows it was already `true` on the source photo (E2's copies inherit it), so the run does not tell `None` from `MaskOnly`; the default is the form E2 applied | `None` against `MaskOnly` (the blocking wire-golden variant, with and without the switch); `All` and the `PANEL_SWITCHES` map are unverified (check them before turning `All` on) | supported (E2, non-raw) |
| `mask_form` | `PresetForm` (an AI mask's `MaskVersion` 1, `ReferencePoint` centre, `ErrorReason` 0; a luminance range's `Version` 3, `SampleType` 0) / `Minimal` | `PresetForm` | E2a: a digest-less subject mask in this form is kept, pending until `updateAISettings()`, then computed; sky and background likewise | people parts: the one applied (E2d hair, `MaskSubType` 3 / `MaskSubCategoryID` 5, the writer's form) ended `failed`, `ErrorReason` 1; form or photo is not known. `Minimal`, and any luminance range, applied by no experiment | settled for subject, sky and background (E2, non-raw) |
| `local_form` | `AdaptivePreset` (the 23 `Local*` keys at 0 unless the model has a value) / `Sparse` (the model's own) | `AdaptivePreset` | E2 applies the zero-filled form, and the corrections work | `Sparse` applied by no experiment | settled (E2, non-raw) |
| `wb_custom_with_numbers` | numbers with `WhiteBalance="Custom"` / alone | `true` | E1b: incremental numbers alone are stored, and leave the mode at "As Shot" in the readback, so the writer keeps writing `Custom` with them; whether As Shot + numbers renders them is the open Basic-panel check. E1d: `"Custom"` is taken as a mode | non-raw: `Custom` + incremental numbers in one call (the writer's non-raw form, never applied); raw: E1b against E1c on a raw file | supported on non-raw; open on raw |
| `wb_mode_only` | a named mode without numbers written / skipped and reported (`Custom` alone is never written) | `false` | E1f/E1g: `"Auto"` alone sets the mode, and the readback showed the previous incremental numbers unchanged within 15 s (the Basic-panel check is open). Never honoured for a non-raw photo: the writer and the style engine refuse a mode alone there before reading the switch. Stays together with the style engine's `EMIT_NAMED_WB_MODES` (off) | raw: E1e (`Daylight`) and Auto on a raw file, where a positive answer turns mode-only on for raw targets only; `"As Shot"` and the other named modes alone are applied by no experiment | settled on non-raw; open on raw |
| `flatten_auto_now` | `ApplyCall::flatten_auto_now` when the table sets `WhiteBalance="Auto"` | `false` | E1f against E1g: `optFlattenAutoNow = true` made no difference to the readback within 15 s | raw (matters only with `wb_mode_only`) | settled on non-raw |
| `ai_update` | `ApplyCall::update_ai_settings` after an AI mask (`Mask/Image`), `LensBlur` or an Adobe Adaptive `Look` | `true` | E2b: without the call the mask is still pending after 30 s in the Library module; E2c: after `photo:updateAISettings()` it is computed (call 0.6 s) | an Adobe Adaptive `Look` (E11 rejects adaptive Looks); people parts (the subType-3 form, on a photo with a visible person) | settled for AI masks (E2, non-raw) |
| `look_form` | `Stub` (`Stubbed = true`) / `BareStub` / `Full` | `Stub` | E11 needs a raw file and did not run; every Look in Adobe's bundled presets is a `Stubbed = true` stub (E11a's form) | E11a/E11b/E11c on a raw file; camera-restricted and adaptive Looks by no experiment | open |
| `preset_amount_flags` | `SupportsAmount`/`SupportsAmount2 = true` in a plugin preset's table (`LuaMode::Preset` only) | `false` | E4e: amount 50 and 200 leave the globals and the mask values at the preset's own, with and without the flags (read back) | the look of the amount copies (manual check); the preset route is not chosen (see below) | settled: no effect in 15.6 |

The default table now equals the one E2 applies for AI masks (preset-form
masks, zero-filled locals, numbers, the mask switch; the switch was already
on in E2's photo). The goldens are still
not frozen: no experiment has applied the writer's own output, see
[Wire goldens](#wire-goldens).

What the experiments measure that is **not** a writer switch, and why:

- **E1a/E1d (`Temp` accepted?)**: accepted without an error and ignored
  (15.6, non-raw). `Temp` is never written; the family keys are the
  documented ones (rule 9).
- **E1c on a non-raw file (`{Temperature = 22}`)**: a cross-family probe.
  Lightroom 15.6 stores the raw key on the non-raw photo as a dead value
  (the mode and the incremental keys stay as they were); the writer never
  writes the other family's keys (rule 9), so the answer changes no table.
- **E2 timing** (update call and mask-ready time): runtime planning for
  step 2, no table shape.
- **E4a/E4b (same-name presets), E4f (apply then delete the file)**: the
  preset route's lifecycle in the apply layer (step 2); the table is the
  same either way.
- **E4c (preset masks merged or replacing)**: replacing (15.6), and the
  preset's `updateAI = true` left its sky mask pending for 60 s. The writer
  writes exactly `settings.corrections`. Keeping the photo's existing masks
  is the apply layer's job on the photo's **raw** tables: the writer drops
  digests (rule 1), so an existing AI mask passed through the model would
  lose its computed state. Step 2 applies masks through
  `applyDevelopSettings` + `updateAISettings()` instead of a preset.
- **E4d (re-apply duplicates or updates by sync id)**: updates in place
  (15.6). Sync ids come from
  the builders (derived per role, see [Building corrections](#building-corrections));
  the writer keeps them.

When an experiment has answered, in one change:

1. change the field in `LuaOptions::PROVISIONAL`, its row in
   `LuaOptions::EVIDENCE` and the pinning tests
   (`the_provisional_choices_are_pinned`, `the_first_run_statuses_are_pinned`);
2. E1 (mode only): flip `wb_mode_only` together with
   `lrg_analysis::style_engine::EMIT_NAMED_WB_MODES` and its const assert
   in `a_named_mode_majority_is_not_sent_while_the_switch_is_off`. A positive
   raw E1e turns mode-only on for raw targets only: both refuse a mode alone
   for a non-raw photo before reading the switch (E1f/E1g), pinned by
   `custom_without_numbers_is_never_written_alone` and
   `a_non_raw_photo_gets_no_mode_alone_whatever_the_switch`;
3. re-bless the wire goldens and run `busted` ([Wire goldens](#wire-goldens));
   no test or reader change is needed (`every_golden_case_is_clean_under_every_option_flip`
   proves that for every single flip);
4. set the field's Status in the table above (and the run's scope: version
   and file kind), and correct the field's rustdoc and the `PROVISIONAL`
   table in `src/lua/write.rs` if the run settled more or less than the
   table says.
   The Findings page, `wire/README.md` and the plugin spec point here
   instead of restating the values.

### Round trips 2 and 3

Plan §7.3, in `tests/support/lua_round.rs` (shared by `tests/lua_roundtrip.rs`
and the local sweeps):

- **Round trip 2**, model → Lua → model (`LuaMode::TestRoundTrip`): the
  same model back for every `Presence::Both`/`LuaOnly` value, opaque Lua
  content and read warnings included, and writing the read-back model again
  gives the same table. A model read from XMP goes in without what has no
  Lua form (`without_xmp_only`): XMP-only keys (`HasSettings`, `HasCrop`,
  ...; a preset header is not part of the model), `PerFormat` values in
  their XMP shape, content kept as XMP. Empty containers need no exception:
  the reader reads them as "absent".
- **Round trip 3**, Lua → model → XMP preset → model → Lua, mixed and
  single-kind preset: the preset's model goes through Lua losslessly and
  without a warning, and every value it shares with the source (filtered
  for the preset, rounded with `Value::quantized` to the preset writer's
  `NumFmt`, then through Lua) is equal. Opaque content is excepted by
  definition (format-specific; a preset never carries it).

| Source | Round trip 2 | Round trip 3 |
|---|---|---|
| committed: Lua fixtures, wire goldens, XMP fixtures (`lua_roundtrip.rs`) | 42 Lua tables under 4 encodings (168), every XMP fixture (22) | 64 sources, 128 presets, 7,374 shared values |
| training dump, 1,294 rows (local) | 311,746 values | 2,588 presets, 301,834 shared values |
| Lightroom bundle, 446 presets (local) | 27,151 values | 892 presets, 48,281 shared values |
| Camera Raw presets, 4 (local) | 310 values | 8 presets, 530 shared values |
| sidecar corpus, all 10,815 (local) | 1,645,988 values | 21,630 presets, 2,267,558 shared values |

All clean (October 2026): no value changed, lost or warned about, every
second write identical.

### Wire goldens

`server-rs/testdata/develop/wire/*.json` are tables the writer produces in
apply mode with the default options (`LuaOptions::PROVISIONAL`; re-blessed
2026-10-03 for `panel_switches = MaskOnly`, which added
`EnableMaskGroupBasedCorrections = true` to the ten goldens with
corrections), one per shape: basic global
sliders, raw Custom white balance, non-raw incremental white balance, global
and parametric tone curves, HSL and colour grading, a subject and a sky
correction, linear and radial gradients, a sky minus subject intersected
with a luminance range, a people part, local point curves on a correction,
the `Look` as a stub and whole, a source whose only correction is a brush
(`MaskGroupBasedCorrections` absent, never `[]`), and five committed
readbacks applied to their own photo: a sky mask, a colour range
(`PointModels`, `ColorAmount`), an area colour range next to AI masks
(`AreaModels`, `ColorRangeMaskAreaSampleInfo`), Select Object polygons
(`Gesture`/`Points`) and the `LensBlur` struct (18 files).
`tests/lua_wire_goldens.rs` writes and compares them, pins each case's
skipped paths and `ApplyCall`, and checks each **file** both ways with the
sweeps' check (`support/lua_apply.rs`): it reads back without a warning,
every value of the filtered settings is in it or reported, and it holds
nothing beyond those and the writer's fixed form. Sources are committed
scrubbed fixtures or models built in the test, with synthetic sync ids; the
files pass both hygiene checks. Their README (`wire/README.md`) says who
reads them and how to re-bless.

The same test file also

- writes every case with each `LuaOptions` field flipped to each of its
  other values in turn, in apply and preset mode, and requires the same
  two-way check to pass (504 tables): flipping a field and re-blessing needs
  no test or reader change on the Rust side;
- generates `wire/key_classes.txt` from the registry (blessed with the
  goldens): each key name with the classes of its values (`number`,
  `integer`, `flag`, `maskenum`, `compound`, ...) or `never`, which the
  plugin spec checks every value against, so the spec's key lists cannot
  drift from the registry or from `NEVER_WRITE`;
- pins `ADAPTIVE_PRESET_LOCALS` against `LOCAL_KEYS` in
  `DevelopExperiments.lua`.

**Not frozen.** The first run settled the AI-mask forms for subject, sky
and background on one non-raw photo, and the default table is now the form
E2 applied; people parts, `panel_switches` (`None` against `MaskOnly`),
`int_flag_as`, `look_form` and the raw side are open. Freezing also needs one thing the
experiments as built do not do: apply the writer's own output. E2 and E4
apply hand-built tables (the AI-mask preset form, all 23 locals,
`EnableMaskGroupBasedCorrections`), never a golden. Blocking item before the
goldens are frozen: an E2 variant that decodes `masks_subject_sky.json`,
`mask_luminance_intersect.json` and `global_basic.json` with JSON.lua and
passes them unchanged to `applyDevelopSettings` (with and without the mask
switch, on a photo where the switch is off). Variants for numeric-string mask enums, `MaskForm::Minimal`,
`LocalForm::Sparse` and booleans on the flag keys would measure the
alternatives no experiment applies yet.

To re-bless after an intended writer change, or after an experiment has
flipped a field of `LuaOptions::PROVISIONAL`:

```bash
cd server-rs
LRG_BLESS=1 cargo test -p lrg-develop --test lua_wire_goldens   # writes the files and key_classes.txt
cargo test -p lrg-develop --test lua_wire_goldens --test lua_roundtrip
cd .. && busted                                                  # the plugin spec
```

The blessing run skips the read-back test (it would race the files being
written), so the second command checks them. A golden no case writes fails
the test: delete a file together with its case.

**The plugin side.** `plugin/spec/native_wire_format_spec.lua` decodes every
golden with the plugin's own `JSON.lua`, under Lua 5.1 in CI as in
Lightroom (`lua-tests.yml`, which also runs when only
`server-rs/testdata/develop/wire/**` changes), and checks what reaches
`applyDevelopSettings`: a top-level table with string keys; real sequences
(keys 1..n) and no empty, mixed, sparse or `null` value (decoded with a
`null` placeholder so a null cannot hide as a missing key); every value of
the Lua type of its key's registry class (`key_classes.txt`): numbers as
numbers, numeric-looking strings as strings, compound strings as lists of
numbers (`ReferencePoint` 2, `LumRange` and `FocalRange` 4; an empty
string is no list), `CorrectionRangeMask.Type` only inside a range mask,
sync ids 32 hex digits, booleans as booleans and never `"true"`/`"false"`;
no key the registry does not know; global curves flat number lists of even
length ≥ 4, local curves lists of `"x,y"` strings; a non-empty
`CorrectionMasks` on every correction; no never-written key at any depth
(the registry's `never`, `*Digest*`, `MaskBrushTable*`), no `Temp`, no
top-level `ProcessVersion`; one white-balance family, numbers only with
`Custom` or no mode; and the same decoded values after the plugin's own
encode and decode. The encodings the experiments bear on are accepted
either way (mask enums as integers or integer strings, 0/1 flags as 0/1 or
booleans): all 504 flipped tables of the Rust test pass the spec under Lua
5.1 and 5.5. The shapes the checks are about (corrections, global and local
curves, compound strings and lists, booleans, flags, mask enums, numbers)
are counted while the goldens are collected, and a check fails when one is
missing, so a re-bless cannot make them pass vacuously; that check does not
depend on the other tests having run. A block of crafted broken tables (a
number as a string, an empty compound string, a wrong count, a mask enum
that is no integer, an unknown key, ...) proves each check catches what it
is for. Two pin tests hold JSON.lua's own behaviour the rules rest on:
`encode({})` is `"[]"`, and a `null` inside an array decodes to a hole.
Values are compared decoded, numbers within `%.14g`, never as encoded text.

## Building corrections

`lrg_develop::build` makes new local corrections: a `CorrectionBuilder` takes
adjustments in UI units and mask components in evaluation order and returns a
`Correction` the preset writer (or, later, the Lua writer) takes as it is.

```rust
let ns = SyncNamespace::lrgenius();
let sky = CorrectionBuilder::new("Sky", &ns)      // CorrectionName "LrGenius · Sky"
    .amount_ui(100.0)?                            // CorrectionAmount 1 (the default)
    .local_ui("LocalHighlights2012", -40.0)?      // stored -0.4
    .local_ui("LocalDehaze", 15.0)?               // stored 0.15
    .add(Semantic::Sky)
    .subtract(Semantic::Subject)
    .intersect(LuminanceRange::highs(0.5, 0.2)?)
    .build()?;                                    // Result<Correction, BuildError>
```

- **Adjustments.** `local_ui(key, ui)` converts through the registry's UI
  scale (`LocalExposure2012` EV / 4, the other signed sliders / 100,
  `LocalToningHue` and `LocalCurveRefineSaturation` as they are);
  `local_curve(key, &[(x, y)])` sets `MainCurve`/`RedCurve`/`GreenCurve`/
  `BlueCurve` (2+ points, x strictly increasing). Out of range is
  `BuildError::Ui(OutOfRange)`, never clamped. Only learnable adjustments with
  a verified UI scale are writable: `LocalHue` and `LocalGrain` (unverified
  scales), `LocalPointColors`/`LocalColorVariance` (UNKNOWN), the legacy
  `LocalExposure` family (COMPUTED) and every bookkeeping key are
  `BuildError::NotWritable`.
- **Components.** `add`, `add_inverted`, `subtract`, `intersect` encode as the
  combination table in [The model](#the-model). `build` refuses a correction
  without components (`Empty`), one whose first component is not added
  (`FirstNotAdd`), one without any adjustment (`NoAdjustments`) and an empty
  role (`EmptyRole`). `MaskValue` is 0 exactly when `MaskBlendMode` is 1. A
  luminance range's `CorrectionRangeMask.Invert` is set to the component's
  `MaskInverted` (not the caller's), as Lightroom writes every range mask —
  an intersected range therefore has `Invert="true"`; a range whose `Invert`
  disagreed could be read as its opposite.
- **Names.** The correction is `"LrGenius · <role>"`; each component gets a
  neutral English `MaskName` close to Adobe's (`Subject`, `Sky`,
  `Background`, `Iris and Pupil`, `Vegetation`, `Linear Gradient`, `Radial
  Gradient`, `Luminance Range`, ...; Adobe's own Subject and Sky presets say
  `Subject 1`/`Sky 1`). Names are display only; nothing reads them back.
- **Sync ids.** `CorrectionSyncID` and every `MaskSyncID` are the first 16
  bytes of SHA-256 over a domain tag and the length-prefixed namespace, role
  and slot (the correction, or component *i*), as 32 upper-case hex digits
  (`SyncNamespace::id`). The same role gets the same ids on every run, so a
  later step can replace a correction rather than stack a second one (whether
  Lightroom replaces or duplicates is experiment E4/E5). Two corrections of one
  preset therefore need different roles: assemble a preset's corrections with
  `build::corrections([builder, ...])`, which builds each and refuses a
  repeated role (`BuildError::DuplicateRole`). The preset writer does not
  check it, because Lightroom's own sidecars repeat `MaskSyncID`s across the
  corrections of one file (133 of 3,465 sidecars with corrections, 12 even
  within one correction). Known-answer tests pin three ids, so a change to
  the domain tag, the length prefixes or the slot texts — which would stop a
  later release from replacing what an earlier one wrote — fails.
- **What is left to the writer.** The builder sets the typed parts only
  (name, id, amount, active, adjustments, tool, combination). The preset
  writer adds `What`, an AI mask's `MaskVersion`/`ReferencePoint`/
  `ErrorReason` and a range mask's `Version` 3/`SampleType` 0 (see [Preset
  mode](#preset-mode)). A radial gradient's `Version` 2 is set by the builder,
  because the reader keeps that field in the component's `extra`.

| Component | Built from | Encoding | Policy |
|---|---|---|---|
| `Semantic::Subject` / `Sky` | — | `Mask/Image`, subtype 1 / 2 | LEARN |
| `Semantic::Background` | — | subtype 0, category 22 | LEARN |
| `Semantic::people_part(PeoplePart)` | `FaceSkin` 2, `IrisAndPupil` 3, `BodySkin` 4, `Hair` 5, `Lips` 6, `FacialHair` 7, `EyeSclera` 8, `Eyebrows` 9, `Clothes` 11, `Teeth` 12 | subtype 3 (preset form; "all people" unconfirmed) | LEARN |
| `Semantic::landscape(LandscapeClass)` | `Architecture` 50001 … `Snow` 50008 | subtype 0 | LEARN |
| `Semantic::person_part_at(part, SensorPoint)` | a part and the person's point | subtype 0 + a real `ReferencePoint` | PHOTO, unverified until E2/E8 |
| `LinearGradient::new(zero, full)` | two different `SensorPoint`s (by the key names no effect at `zero`, full at `full`; the direction is unverified until E6) | `Mask/Gradient` | PHOTO |
| `RadialGradient::new(top, left, bottom, right, feather)` | `top < bottom`, `left < right`, feather 0..=100 | `Mask/CircularGradient`, `Angle` 0, `Midpoint` 50, `Roundness` 0, `Version` 2, `Flipped = !MaskInverted` | PHOTO |
| `LuminanceRange::new([fl, l, h, fh])`, `lows(upto, feather)`, `highs(from, feather)` | `0 ≤ fl ≤ l ≤ h ≤ fh ≤ 1` | `Mask/RangeMask`, `CorrectionRangeMask.Type` 2, `Invert = MaskInverted`; no eyedropper sample (`LuminanceDepthSampleInfo`, which every range in Lightroom's sidecars has) — unverified in a preset until the hand test | LEARN |

A category outside these tables is `BuildError::InvalidCategory` (whole-person
20036 included); `MaskTool::Opaque` (brush, Select Object, colour or depth
range) and a luminance range carrying further `CorrectionRangeMask` fields
(the eyedropper sample) are `NotBuildable`. A PHOTO component makes its whole
correction PHOTO, so the preset writer leaves it out and reports it in
`Written::skipped`.

**Coordinates.** A `SensorPoint { x, y }` (and a radial gradient's `Top`,
`Left`, `Bottom`, `Right`) is normalised per axis in the **uncropped image in
sensor orientation**: `x` 0 is the left edge and 1 the right, `y` 0 the top
and 1 the bottom, as the raw file's sensor lies, before crop and before the
photo's rotation. Values may leave 0..1 (a gradient may start outside the
frame). Builders take `SensorPoint`s and normalised numbers only, never pixel
tuples; converting export or display pixels (crop, rotation, orientation) is
`FrameGeometry`, which comes with its first consumer in step 2. Until then the
gradient builders are only usable with points that already are in this frame
(tests, presets).

**Radial gradients: `Angle` 0 only.** The reader types a radial gradient only
when its `Angle` is 0; a rotated one stays `MaskTool::Opaque` (with every
field, so it still round-trips) until experiment E6 settles the rotation
convention, and the builder has no angle parameter. It also fixes `Midpoint`
50 and `Roundness` 0 and sets `Flipped = !MaskInverted` itself: that is the
relation every radial gradient in the maintainer's sidecar corpus holds
(414 typed ones; the local sweep counts it on every run, next to
`Invert = MaskInverted` for all 207 typed luminance ranges, their `Version`
3 and their `SampleType`: 195 × 0, 12 × 1).

**Parity with Adobe's adaptive presets** (`lightroom_adaptive_presets_rebuild_with_the_builders`
in `tests/xmp_goldens_local.rs`, local only). Every correction of the bundle's
adaptive presets is rebuilt from the parsed model with the builders (the
amount and adjustments through `to_ui` → `amount_ui`/`local_ui`/`local_curve`,
the same components and combinations, a luminance range without its
`Version`/`SampleType`, which the writer adds) and written as a preset. Last
run (September 2026, Lightroom Classic 15.6's bundle): 38 presets, 109
corrections, all rebuilt; read back, they equal Adobe's corrections written by
the same writer in every field (names and sync ids apart, which are ours by
design). Against Adobe's files as they are, the only differing fields are ones
the writer also holds back from Adobe's own corrections: the digests
`LocalInputDigest`/`LocalInputDigestVersion` (36), the legacy PV2010
`LocalBrightness`/`LocalClarity`/`LocalContrast`/`LocalExposure` (109, all at
their default), `LocalCorrectedDepth` (65) and `LocalGrain` (65, unverified
scale, all 0), `LocalHue` (109, unverified scale; one non-zero), and
`LocalPointColors`/`LocalColorVariance` (9, UNKNOWN). The builder refuses
`LocalGrain`, `LocalHue` and `LocalPointColors`/`LocalColorVariance` as
`NotWritable`, which the test counts; `LocalCorrectedDepth`, the digests and
the legacy keys are COMPUTED and never offered to it. The bundle holds only
added AI masks (110 components, all `Add`), so this parity says nothing about
subtract, intersect, luminance ranges, gradients or Select Background; the
corpus sweep and the third hand-test preset cover those forms. `CompatibleVersion` is
compared for the report: equal in 28 presets, ours lower in 10 — 6 carry
point colour (Adobe 17.4, ours 15.3, since we drop it) and 4 are Subject, Sky
or people presets Adobe stamped 15.3 whose only difference from Adobe's own
14.0/15.0 presets of the same content is `LocalCorrectedDepth`/`LocalGrain`
at 0, which we do not write.

### Hand test

The one check no test can do: does Lightroom Classic (15.5.1 or later)
import a written preset and show its masks? It runs on the installed
Lightroom Classic — 15.6 on the maintainer's machine, newer than the 15.5.1
`TARGET_ENGINE` aims at — and the version actually used goes into the
record below and the PR. An import into a newer Lightroom cannot confirm a
`CompatibleVersion` minimum. `tests/builder_presets.rs` builds three
presets with the real builders and writer in preset mode (`ProcessVersion`
15.4):

| File | `CompatibleVersion` | Global | Corrections (Masking panel) |
|---|---|---|---|
| `LrGenius Handtest Subject+Sky.xmp` | 14.0 | Contrast +15 | "LrGenius · Subject" (Subject: Exposure +0.30); "LrGenius · Sky" (Sky: Highlights −40, Dehaze +15) |
| `LrGenius Handtest Landscape+People.xmp` | 15.3 | — | "LrGenius · Vegetation" (Vegetation: Saturation +20); "LrGenius · Eyes" (Iris and Pupil of all people: Saturation +30) |
| `LrGenius Handtest Combinations+Background.xmp` | 15.0 | — | "LrGenius · Bright sky" (Sky, minus Subject, intersected with a luminance range 0.3/0.5/1/1: Highlights −30); "LrGenius · Background" (Background: Exposure −0.50, Saturation −30) |

The third preset carries what no preset of Adobe's does: a subtract, an
intersect (with the range's `Invert="true"` mirroring `MaskInverted`), a
luminance range without the eyedropper sample every range in Lightroom's
sidecars has, and Select Background.

```bash
LRG_HANDTEST_DIR=/some/dir cargo test -p lrg-develop --test builder_presets
```

writes all three there. In Lightroom Classic: Develop → Presets panel → `+` →
Import Presets…, pick the files; they appear in the group "LrGeniusAI".
Apply the first and third to a photo with a person, sky and some bright
background, the second to one with vegetation and to one with **two or more
people** (does the people part select everyone, or one person?). Expected:
the global Contrast moves to +15; the Masks panel lists the named masks,
each with the components the table names (Lightroom computes the masks on
apply, which can take a few seconds; the people and landscape masks may ask
for an update); the mask sliders show the values above; the "Bright sky"
mask covers only the bright sky around the subject. What to note if it
fails: an import error, masks listed but empty, a mask missing, a
component shown inverted (a range selecting the shadows), or a people part
on one person only. Until the run is recorded here, Select Background's
15.0 row, the luminance range and the subtract/intersect encodings in a
preset, and "all people" are unverified. The same presets with synthetic
ids are the byte goldens in `testdata/develop/written/` (see [Fixtures and
hygiene](#fixtures-and-hygiene)).

Record (Lightroom version, date, per preset: imported / masks shown /
values / notes): *pending*.

## Fixtures and hygiene

Test data lives in `server-rs/testdata/develop/` (layout and scrubbing rules
in its README):

- `lua/*.json` listed in `lua/manifest.json` are **generated** by
  `server-rs/scripts/develop_registry/extract_fixtures.py` from the
  maintainer's edits and replaced on every run. Do not edit them.
- Shapes the source data does not have (booleans as 0/1, non-integers on
  integer keys, `null` in arrays, mixed arrays, non-raw white balance, both
  white-balance families, PV2010) are **hand-written** as `lua/hand_*.json`;
  the generator never touches them. Add a new one there, and give it its own
  test in `tests/lua_read.rs` naming the warnings it must produce.
- Any other `*.json` in `lua/` is a hygiene failure.
- `xmp/*.xmp` are **self-authored**, one file per XMP form or develop shape
  (no Adobe file, no copy or excerpt of one, no real sidecar); the list is in
  the folder's README and in `FIXTURES` in `tests/xmp_fixtures.rs`.
- `wire/*.json` are the **Lua writer's wire goldens** (see [Wire
  goldens](#wire-goldens)): not frozen while `LuaOptions` fields are open, regenerated with
  `LRG_BLESS=1 cargo test -p lrg-develop --test lua_wire_goldens`, read by
  Rust and the plugin's busted spec. `wire/key_classes.txt`, generated by
  the same run from the registry, holds key names and value classes only.
- `defaults/non_raw_lrc15.6.json` is E13's readback of one non-raw photo
  without develop edits (a virtual copy of a TIFF; key -> value, plus
  Lightroom version, process version and file kind), the source of the
  registry's non-raw default column (see
  [Maintaining the registry](#maintaining-the-registry)).
- `written/*.xmp` are the **writer's byte goldens**: the three hand-test presets
  exactly as the preset writer emits them, with synthetic ids in place of the
  derived sync ids (a SHA-256 id cannot be told from a catalog's by the
  hygiene check) and the development toolkit string. Regenerate after an
  intended format change with
  `LRG_BLESS=1 cargo test -p lrg-develop --test builder_presets`.

The hygiene rules keep private data out of the repository: no photo ids,
paths, file or image names, dates, real 32-hex ids or GUIDs, profile
definitions or lookup tables (`Look.Parameters` is at most a stub of
`ConvertToGrayscale`/`ProcessVersion`), and `Look` names/UUIDs only from the
verified Adobe profiles or synthetic. The Python check
(`extract_fixtures.py --check-only`, in pre-commit and CI) and the Rust test
`tests/fixture_hygiene.rs` implement the same rules and must stay in step:
both match patterns against whole strings with ASCII digits, and both run the
same crafted files through the file-level rules
(`extract_fixtures.py --self-test`, and
`dispatch_rules_flag_exactly_the_crafted_files` in the Rust test).
`registry_snapshot.json` holds key names and their metadata, no settings, so
it gets the text rules without the table-key substrings, plus the id check.
An XMP file must be well-formed with an `rdf:RDF`, its `rdf:Alt` items need a
valid `xml:lang`, and its data model (`xmp_as_tree`, the same conversion on
both sides) gets the rules of a hand-written JSON file, so a `Look.Parameters`
beyond the stub, a real id or a date in an XMP fixture fails the same way.
The table-key substrings (`Table_`, `LookTable`, `RGBTable`) are forbidden as
text, with one exception on both sides: the preset-header key
`RequiresRGBTables` as a whole word, which Adobe's newer presets (the 163 of
446 bundled ones that carry `SupportsAmount2`, every adaptive preset
included) and every preset the writer produces carry (the writer goldens and
`xmp/preset_header.xmp` hold it). It is removed before the text rules run, so `RGBTable_<md5>` or
`RequiresRGBTablesX` still fail; crafted self-test files pin both cases.

Everything under `server-rs/testdata/develop/` is checked out with LF line
endings on every platform (`.gitattributes`), and the snapshot test compares
content, not line endings, so a Windows checkout passes too.

## Tests

- **Unit tests** next to the code: registry invariants, scaling, coercion,
  `Finite`, the settings invariant, combine/semantic tables, the policy filter,
  the frame split, every reader rule.
- **`tests/lua_read.rs`**: every fixture in `server-rs/testdata/develop/lua/`
  parses without errors or unknown keys; generated fixtures without any
  warning; every shape a fixture's `manifest.json` entry claims is recognised
  in the model; each hand-written `hand_*.json` shape produces exactly its
  expected warnings.
- **`tests/fixture_hygiene.rs`**: mirrors `extract_fixtures.py --check-only`
  (rules in `server-rs/testdata/develop/README.md`): no photo ids, paths, file
  names, dates, real ids, profile definitions or lookup tables in the test
  data.
- **`tests/xmp_fixtures.rs`**: every fixture in `testdata/develop/xmp/`
  parses; only `unknown_keys.xmp` has `UnknownKey` warnings and only it and
  `coercion_warnings.xmp` any warning; only `bag.xmp` (1) and
  `alt_second_language.xmp` (2) keep registry keys whole (`kept_whole`); no
  fixture uses a Lua-only key; each form has its own assertions (attribute
  and element form, and plain and `parseType="Resource"` structures, read
  into equal models). The pre-scan, the limits, mixed-form elements,
  duplicates and `rdf:value` attributes are unit tests in `xmp/read.rs`
  (deep nesting is parsed on a 2 MiB-stack thread).
- **`tests/xmp_lua_equivalence.rs`**: for the curve, mask, look and number
  fixtures, the same settings hand-written in the Lua table form read into an
  equal model. Removed before comparing: global keys of only one format
  (`Presence::XmpOnly`/`LuaOnly`) and `Look.Parameters`, which each reader
  keeps opaque in its own format. The other by-design differences listed
  under [Reading XMP](#reading-xmp) (`PerFormat` and `Any` keys, nested
  Lua-only keys, the `PointColors` sentinel) are kept out of the fixtures
  instead.
- **`xmp::format` and `xmp::write` unit tests**: every spelling rule above,
  the document shape against Adobe's layout, the preset header defaults and
  stamps, policy filtering and the never-write guard (with every
  `NEVER_WRITE` key checked against its policy), the `CompatibleVersion`
  table, errors instead of broken files, byte determinism (twice the same
  bytes, insertion order irrelevant), the `Look` by reference without its
  profile definition or tables, values kept whole never reaching a preset,
  white-balance numbers only with a custom mode, the recomputed AI-mask
  fields not reported, a blank name refused, and — with the `test-roundtrip`
  feature — the lossless mode's own rules (numbers the format would round,
  foreign namespaces and qualifiers, Lua content refused).
- **`tests/xmp_roundtrip.rs`** (plan §7.3): round trip 1 over every fixture in
  `testdata/develop/xmp/` and every golden in `testdata/develop/written/`
  (model, header, kind and opaque content equal; writing the read-back model
  gives the same bytes); the key enumeration — every LEARN and PHOTO key with
  an XMP form, at every level (global, correction, mask component, `Look`,
  `LensBlur`, range-mask, area-model and gesture fields), with its minimum,
  maximum, midpoint, default, a negative value for a signed range, a value
  with more digits than any format keeps, and every member of a closed set,
  survives the lossless mode exactly (789 values), and every LEARN value a
  preset can carry by itself (global values, a correction's adjustments and
  amount, `LensBlur` fields, the `Look` amount) survives preset mode, mixed
  and single-kind, after rounding to its `NumFmt` (`Value::quantized`; 1,073
  checks; a LEARN† key may be held back by its gate — the raw-only and
  non-raw-only white balance, the file-kind-default `Sharpness` and
  `ColorNoiseReduction` in a mixed preset, `LensProfileSetup` outside its
  values — and what is held back is pinned per key, gate and mode; a plain
  LEARN key never is). Mask fields are typed and go through presets in the
  builder tests. And a preset written from every Lua and XMP fixture, mixed
  and single-kind, is byte-deterministic and reads back as a preset with no
  warning, nothing kept whole, no key outside LEARN, LEARN† and META (apart
  from the writer's own fixed values: `WhiteBalance="Custom"`, an AI mask's
  `ReferencePoint`/`ErrorReason`, a range mask's `SampleType`), and exactly
  the policy-filtered settings, each value rounded as written (the writer's
  stamps and fixed form apart). Both checks live in `tests/support/preset.rs`
  and are shared with the local sweeps; a test pins that they catch a lost,
  changed or extra value, a PHOTO key and content kept whole.
- **`build` unit tests**: every semantic variant with its encoding and name,
  unknown categories, the combination table and `MaskValue 0 ⇔ MaskBlendMode
  1`, first-component-added, empty correction, `Flipped = !MaskInverted`,
  a range's `Invert = MaskInverted` under every combination, radial and
  linear checks, luminance-range ordering, UI scaling and the not-writable
  keys, local curves, sync-id determinism, uniqueness per namespace, role and
  slot, and known answers, `corrections` refusing a repeated role; through
  the writer: built semantic corrections read back unchanged from a preset
  (an intersected range with `Invert="true"`), PHOTO corrections are skipped
  and reported, output is byte-deterministic, and (feature `test-roundtrip`)
  every built tool round-trips exactly.
- **`lua::write` unit tests**: one per writer rule — keys and policy,
  the guard against every `NEVER_WRITE` key, computed mask bookkeeping
  never written, JSON types of integers/reals/booleans/flags, coercion and
  wrong variants, numeric-looking strings, compound strings and ids,
  curves (flat, local strings, one point refused), corrections in panel
  order with their fixed form, gradients, no empty containers (no
  corrections, a brush-only source, an emptied structure), `check_wire`
  against `null`, empty and mixed lists and `Temp`, the white-balance
  family (raw, non-raw, unknown kind, other modes, mode only, `Custom`
  never alone, flatten, `Custom` stamp), every `LuaOptions` alternative
  (the adaptive-preset locals and the sparse form, the panel switches
  `MaskOnly` and `All`, the amount flags only in preset mode), the `Look`
  forms, a camera restriction and an adaptive Look's AI update,
  `PANEL_SWITCHES` naming registry keys, read-back and determinism, and the
  provisional choices pinned.
- **`tests/lua_wire_goldens.rs`**: the wire goldens above, every case under
  every single option flip, `key_classes.txt`, and the adaptive-preset
  locals against the experiments' list.
- **`plugin/spec/native_wire_format_spec.lua`** (busted, from the repo
  root): the same goldens decoded by the plugin's JSON.lua (see [Wire
  goldens](#wire-goldens)).
- **`tests/lua_roundtrip.rs`** (plan §7.3, see [Round trips 2 and
  3](#round-trips-2-and-3)): round trip 2 (model → Lua → model, lossless)
  over every Lua fixture and wire golden, warnings included, under all four
  encodings (mask enums as numbers or strings × flags as numbers or
  booleans; 42 tables each), and over every XMP fixture without what has no
  Lua form; round trip 3 (Lua → model → XMP preset → model → Lua, mixed and
  single-kind) over the Lua fixtures, the wire goldens and the XMP
  fixtures, agreeing on every shared value after `Value::quantized` (7,374
  values; a failure names the source); the key enumeration
  (`tests/support/enumerate.rs`, shared with `xmp_roundtrip.rs`) through
  the lossless mode (792 values) and through apply mode for a raw and a
  non-raw photo, where every global, correction, `LensBlur` and
  `Look.Amount` value reaches the photo exactly or is reported or is its
  default (1,299 written), with what is held back pinned per key and file
  kind (the other white-balance family, a mode without numbers,
  `LensProfileSetup` outside its values, the guard's `LensProfileDigest` and
  `orientation`); and the sweeps' apply-mode check over every committed Lua
  and XMP fixture (profiles excluded), with the provisional options and
  every single flip, in apply and preset mode (45 sources, 1,350 tables,
  130,792 values compared, nothing lost, changed or added) — the check that
  covers the mask-tool, range-mask and gesture values the enumeration does
  not place.
- **`tests/builder_presets.rs`**: the three hand-test presets (see [Hand
  test](#hand-test)) build, write without skips and read back unchanged, and
  (with synthetic ids) match their byte goldens in `testdata/develop/written/`;
  `LRG_HANDTEST_DIR` writes them out for Lightroom.
- **`tests/registry_snapshot.rs`**: the snapshot above.
- **`tests/support/`**: helpers the local sweeps share (mask tool and
  combination counts, the mask forms the builders rely on — `Flipped`,
  a range's `Invert`, `Version` and `SampleType` —, the registry-range check,
  warning paths without indices), all reducing a model to key names and
  counts; `support/flat.rs` flattens a model into `path → value` with the
  typed parts expanded back into the XMP keys they stand for, so two models
  are compared key by key; `support/preset.rs` holds the two checks every
  preset-writing test applies (and the quantization round trip 3 uses);
  `support/enumerate.rs` the key enumeration both writers' round trips
  share; `support/lua_apply.rs` the two-way apply-mode check of the Lua
  writer (every filtered value written or reported, nothing added beyond
  the writer's fixed form) and the option flips, shared by the local
  sweeps, `lua_roundtrip.rs` and `lua_wire_goldens.rs`; `support/lua_round.rs` round trips 2 and 3, counted,
  for `lua_roundtrip.rs` and the local sweeps (all included by path from
  the tests that use them).
- **`tests/training_dump_local.rs`** — local only: parses every row of the
  maintainer's private training dump and requires zero parse errors, zero
  warnings of any kind and every number within its key's registry range,
  printing counts per warning kind, mask tool, combination and out-of-range
  key (never values or photo ids; failing rows are named by position). Every
  row is also written as a preset, mixed and single-kind — the production
  path, since learned settings come from Lua — and must pass the same two
  preset checks as the fixtures; and every row goes through the Lua writer:
  round trips 2 and 3 (`tests/support/lua_round.rs`; 311,746 and 301,834
  values) and apply mode for its own photo (`tests/support/lua_apply.rs`: a
  table that passes `check_wire`, reads back without a warning, holds
  every filtered value exactly or reports it, and holds nothing beyond
  those and the writer's fixed form; 1,294 rows, 213,826 values compared,
  none changed, lost or added, October 2026):

  ```bash
  LRG_DEVELOP_TRAINING_DUMP=/path/to/training_rows.json \
      cargo test -p lrg-develop --test training_dump_local -- --nocapture
  ```

  Without the variable it skips through the shared golden gate
  (`crates/lrg-ml/tests/common`) as family `training-dump`, which is
  local-only: `LRG_REQUIRE_GOLDENS=all` leaves it out, and only
  `LRG_REQUIRE_GOLDENS=training-dump` makes a missing dump a failure. Setting
  the variable makes a missing file a failure: a typo or an unmounted volume
  must not pass as a skipped run.
- **`tests/xmp_goldens_local.rs`** — local only: the XMP reader and writer
  over files that can never be in the repository or CI, one test and one
  gate family per source:

  | Variable | Family | Parses | Must hold |
  |---|---|---|---|
  | `LRG_LRC_PRESETS_DIR` | `lrc-presets` | every `*.xmp` below it (Lightroom's `Contents/Resources/Settings`) | 0 parse errors, 0 `UnknownKey`, no warning but `DuplicateKey Group`, 0 keys kept whole; presets and profiles only |
  | `LRG_ACR_PRESETS_DIR` | `acr-presets` | the presets and profiles among the `*.xmp` below it (Camera Raw's user folder: its presets and profiles) | as the bundle; keys kept whole only printed (a user preset may have a localized name) |
  | `LRG_XMP_CORPUS_DIR` | `xmp-corpus` | the sidecars below it with `crs:ProcessVersion`; `LRG_XMP_CORPUS_SAMPLE=N` takes a fixed-seed sample of N | 0 parse errors, no warning at all, 0 keys kept whole, every file a `Sidecar` |

  Every sweep also fails on an unrecognised mask combination, on a mask
  component against a relation the builders rely on (`Flipped` other than
  `!MaskInverted`, a range's `Invert` other than `MaskInverted`, a range
  `Version` other than 3), and on a number outside its key's registry range,
  and writes every parsed document back: losslessly (round trip 1: no write
  error, no key differing after reading back, the same model, header and
  kind, idempotent bytes, and every `crs:` value spelled as the source spelled
  it, apart from a per-sweep allow-list of known Adobe variances with their
  reasons; mismatches are counted per key with indices removed) and as a
  preset, mixed and single-kind (no write error, reads back as a preset with
  no warning, no key outside the preset policy, nothing kept whole, and
  exactly the policy-filtered settings; what the policy holds back is
  printed per mode, key and reason, pattern families folded to
  `Table_*`/`UprightTransform_*`). Every document but a profile also goes
  through the Lua writer in apply mode for its own photo, with the same
  check as the training dump (October 2026, both ways — every filtered
  value written or reported, nothing added beyond the writer's fixed form:
  the 446 bundled presets (24,283 values), the 4 Camera Raw presets (267)
  and all 10,815 corpus sidecars (1,376,155) clean), and, without what has no Lua form, through round trips
  2 and 3 (counts in [Round trips 2 and 3](#round-trips-2-and-3)). A fourth test, on `LRG_LRC_PRESETS_DIR`,
  is the builder parity with Adobe's adaptive presets (see [Building
  corrections](#building-corrections)). The corpus walk skips `* [conflicted].xmp`
  sync copies, darktable's `<name>.<ext>.xmp` sidecars (no `crs:` at all) and
  metadata-only sidecars, and prints how many of each. Output is counts per
  document kind, warning kind, process version, skipped subtree, mask tool
  and combination, plus key names; never a value, and corpus or Camera Raw
  files are named by ordinal only.

  ```bash
  LRG_LRC_PRESETS_DIR="/Applications/Adobe Lightroom Classic/Adobe Lightroom Classic.app/Contents/Resources/Settings" \
  LRG_ACR_PRESETS_DIR="$HOME/Library/Application Support/Adobe/CameraRaw" \
  LRG_XMP_CORPUS_DIR=/path/to/photos \
      cargo test -p lrg-develop --test xmp_goldens_local -- --nocapture
  ```

  The full corpus above (October 2026: 28,681 files, 10,815 parsed) takes
  about 150 s in a debug build with all write-backs and checks;
  `LRG_XMP_CORPUS_SAMPLE=3000` about 25 s.
  Gating works as for the training dump: an unset variable skips, the
  families are local-only (`all` leaves them out, only naming one makes its
  absence a failure), and a set variable that is not a directory, or a
  directory without a single file to check, fails.

  A new unknown key found here goes into the registry as a hand-written row
  in `registry/table.rs`, followed by
  `LRG_BLESS=1 cargo test -p lrg-develop --test registry_snapshot` (see
  [Maintaining the registry](#maintaining-the-registry)), never into an
  allow-list.
