# Dev: Native Develop Model (`lrg-develop`)

> **Status: step 1a.** The crate has the key registry, the typed model, the
> policy filter and the reader for the Lua table form. The XMP parser (1c),
> the XMP writer and mask builders (1b) and the Lua writer (1g) follow; this
> page grows with them. Background and the research behind it:
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
  (`CANONICAL_VERSION` marks the stored form).
- `lrg-analysis::style_engine` rounds blends to each key's registry precision
  and decides the white balance per key family.
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
- **`tests/registry_snapshot.rs`**: the snapshot above.
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
