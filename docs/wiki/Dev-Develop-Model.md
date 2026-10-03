# Dev: Native Develop Model (`lrg-develop`)

> **Status: steps 1a and 1c (1d/1f use it).** The crate has the key
> registry, the typed model, the policy filter and the readers for the Lua
> table form and for XMP (sidecars, develop presets, profiles). The XMP
> writer and mask builders (1b) and the Lua writer (1g) follow; this page
> grows with them. Background and the research behind it:
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
| `xmp::read` | an XMP sidecar, preset or profile → `XmpDocument` (kind, header, model, warnings, skipped subtrees) |
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

The table was seeded once from `server-rs/scripts/develop_registry/spec.py`
by `gen_table_rs.py` (same folder; it imports `spec.py` only, never the
rendered Markdown, an inventory or the training dump). **`table.rs` is the
source of truth since.** Adding or changing a key is editing a row there by
hand, then refreshing the review snapshot:

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
write `Temp` on purpose, are allowlisted). Because it guards a plugin file, CI
also runs it in the unfiltered `format-lint-rust` job of `lint-format.yml`, not
only in `server-rs-tests.yml`, which skips plugin-only PRs.

Defaults: the non-raw column is `Unverified` for every key with a fixed raw
default, because every training example so far is a raw file; a non-raw
readback fills those in. `NoDefault` in that column means there is no fixed
default for either file kind (as-shot white balance, digests, `Look`), which a
readback does not change.

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
- **Look.** The profile is taken whole from its source (name, UUID, amount,
  camera restriction, and the rest including `Parameters` verbatim). It is
  never rebuilt from a profile name. An empty `CameraModelRestriction` means
  "no restriction" and stays in the rest as it was.
- **White balance.** `WbSetting { mode, family, temperature, tint }`. The family
  (`Temperature`/`Tint` for raw, `IncrementalTemperature`/`IncrementalTint`
  for non-raw) comes from the keys the settings carry, via the registry's file
  scope, not from the file extension: a DNG converted from a JPEG uses the
  non-raw family. The white-balance *policy* (what transfers) belongs to the
  style engine (`blend_white_balance`, see `POST /v1/edit/style` in
  [Dev-Backend-API](Dev-Backend-API)).
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
brushes NEVER). A camera-restricted `Look` only goes to that camera.
`ApplyPhoto` also drops keys newer than the photo's process version and keys
of the other file kind.

The verdict is per key at every depth: a kept structure is rebuilt from the
fields that pass at their own registry rows, and each removed field is
reported with its full path (`LensBlur.SampledArea`,
`MaskGroupBasedCorrections[0].CorrectionMasks[1].CorrectionRangeMask.LuminanceDepthSampleInfo`).
So a shared `LensBlur` keeps `Active` and the look sliders but not its sampled
area or focal range, and a luminance range loses its eyedropper sample. The
`Look` is the one exception, taken whole: its `Parameters` are the profile
definition. A range mask's `SampleType` is UNKNOWN (default 0) and is dropped
without a report; the XMP writer must emit Adobe's fixed range-mask form
(`Version` 3, `SampleType` 0) itself.

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
- **`tests/registry_snapshot.rs`**: the snapshot above.
- **`tests/support/`**: helpers the local sweeps share (mask tool and
  combination counts, the registry-range check, warning paths without
  indices), all reducing a model to key names and counts.
- **`tests/training_dump_local.rs`** — local only: parses every row of the
  maintainer's private training dump and requires zero parse errors, zero
  warnings of any kind and every number within its key's registry range,
  printing counts per warning kind, mask tool, combination and out-of-range
  key (never values or photo ids; failing rows are named by position):

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
- **`tests/xmp_goldens_local.rs`** — local only: the XMP reader over files
  that can never be in the repository or CI, one test and one gate family
  per source:

  | Variable | Family | Parses | Must hold |
  |---|---|---|---|
  | `LRG_LRC_PRESETS_DIR` | `lrc-presets` | every `*.xmp` below it (Lightroom's `Contents/Resources/Settings`) | 0 parse errors, 0 `UnknownKey`, no warning but `DuplicateKey Group`, 0 keys kept whole; presets and profiles only |
  | `LRG_ACR_PRESETS_DIR` | `acr-presets` | the presets and profiles among the `*.xmp` below it (Camera Raw's user folder: its presets and profiles) | as the bundle; keys kept whole only printed (a user preset may have a localized name) |
  | `LRG_XMP_CORPUS_DIR` | `xmp-corpus` | the sidecars below it with `crs:ProcessVersion`; `LRG_XMP_CORPUS_SAMPLE=N` takes a fixed-seed sample of N | 0 parse errors, no warning at all, 0 keys kept whole, every file a `Sidecar` |

  Every sweep also fails on an unrecognised mask combination and on a number
  outside its key's registry range. The corpus walk skips `* [conflicted].xmp`
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

  The full corpus above (28,680 files) takes about 20 s in a debug build.
  Gating works as for the training dump: an unset variable skips, the
  families are local-only (`all` leaves them out, only naming one makes its
  absence a failure), and a set variable that is not a directory, or a
  directory without a single file to check, fails.

  A new unknown key found here goes into the registry as a hand-written row
  in `registry/table.rs`, followed by
  `LRG_BLESS=1 cargo test -p lrg-develop --test registry_snapshot` (see
  [Maintaining the registry](#maintaining-the-registry)), never into an
  allow-list.
