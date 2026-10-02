# testdata/develop/wire — the Lua writer's wire goldens

> **Provisional — not frozen.** These files change when experiments E1, E2,
> E4 and E11 (`plugin/LrGeniusAI.lrdevplugin/TaskDevelopExperiments.lua`)
> settle the write-side forms `photo:applyDevelopSettings()` accepts. Until
> then they show the provisional choices of `LuaOptions::PROVISIONAL`
> (`server-rs/crates/lrg-develop/src/lua/write.rs`); which choice that is,
> what the experiments as built can settle about it and its status are in
> one table on the wiki page Dev-Develop-Model ("Options and what decides
> them"). `look_full.json` shows the whole-record `Look` alternative.
>
> **Blocking before they are frozen:** the experiments apply hand-built
> tables, never these files. An E2 variant has to decode at least
> `masks_subject_sky.json`, `mask_luminance_intersect.json` and
> `global_basic.json` with JSON.lua and pass them unchanged to
> `applyDevelopSettings` (with and without
> `EnableMaskGroupBasedCorrections`), so the provisional form itself is
> measured.

Each `*.json` is one table exactly as the backend hands it to the plugin:
what JSON.lua decodes into the Lua table `photo:applyDevelopSettings()`
takes. Written by `lrg_develop::lua::to_lua_value` in apply mode, keys
sorted, LF line endings (`.gitattributes`).

## Who reads them

- `crates/lrg-develop/tests/lua_wire_goldens.rs` writes them and compares
  byte for byte, pins what each case skips and how it is applied
  (`ApplyCall`), and checks each file both ways: every value it was written
  from is in it or reported, and it holds nothing else than the writer's
  fixed form. It also writes every case under every single option flip and
  checks those the same way, and generates `key_classes.txt`.
  `tests/lua_roundtrip.rs` round-trips them (model → Lua → model) under
  every encoding.
- `plugin/spec/native_wire_format_spec.lua` decodes them with the plugin's
  `JSON.lua` (Lua 5.1 in CI, like Lightroom) and checks the wire rules on
  the Lua side: real sequences, every value of its key's registry class
  (`key_classes.txt`: numbers are numbers, numeric-looking strings stay
  strings, compound strings hold the right count of numbers, booleans),
  curve lengths, non-empty `CorrectionMasks`, no never-written or unknown
  key, no `Temp`. It compares decoded values, never encoded text: Lua 5.1
  re-encodes numbers with `%.14g`, Lua 5.5 with the shortest exact form.
  It accepts both forms of each provisional encoding (mask enums as
  integers or integer strings, 0/1 flags as 0/1 or booleans), and the Rust
  test checks every single option flip, so a flipped option needs a
  re-bless, not a test change. The `Lua tests` workflow
  (`.github/workflows/lua-tests.yml`) runs when this folder changes.
- Both fixture-hygiene checks (`extract_fixtures.py --check-only` and
  `tests/fixture_hygiene.rs`) cover this folder like every other one.

## Cases

| File | Shape | Source |
|---|---|---|
| `global_basic.json` | basic global sliders, 0/1 flags, a boolean; the example's `ProcessVersion` is not written | hand-written table |
| `wb_raw_custom.json` | raw white balance: `Temperature`/`Tint` with `Custom` | hand-written table |
| `wb_non_raw_incremental.json` | non-raw white balance: `IncrementalTemperature`/`IncrementalTint`, `Custom` stamped | hand-written table |
| `tone_curves.json` | global point curves (flat number lists), parametric curve, derived `ToneCurveName2012` | hand-written table |
| `hsl_color_grading.json` | HSL, colour grading, split toning | hand-written table |
| `masks_subject_sky.json` | a subject and a sky correction | builders |
| `masks_linear_radial.json` | a linear and a radial gradient | builders |
| `mask_luminance_intersect.json` | sky, minus subject, intersected with a luminance range | builders |
| `mask_people_part.json` | a people part for every person (`MaskSubType` 3) | builders |
| `mask_local_curves.json` | local point curves (`MainCurve`, `BlueCurve`) as lists of `"x,y"` strings on a subject correction | hand-written table |
| `look_stub.json` | the `Look` as a stub (provisional form) | `lua/mask_range_color_area.json` |
| `look_full.json` | the same `Look` whole (`LookForm::Full`), stub `Parameters` | `lua/mask_range_color_area.json` |
| `no_masks.json` | a source whose only correction is a brush: `MaskGroupBasedCorrections` absent, never `[]` | hand-written table |
| `fixture_mask_ai_sky.json` | a real (scrubbed) sky-mask readback applied to its own photo: digests, runtime ids and computed mask fields gone | `lua/mask_ai_sky.json` |
| `fixture_mask_range_color.json` | a colour range: `PointModels` (a list of compound strings), `ColorAmount`, `CorrectionRangeMask.Type` 1 | `lua/mask_range_color.json` |
| `fixture_mask_range_color_area.json` | AI masks next to an area colour range (`AreaModels`, `ColorRangeMaskAreaSampleInfo`) | `lua/mask_range_color_area.json` |
| `fixture_mask_select_object_polygon.json` | Select Object polygons (`Gesture`/`Points`), untyped AI masks; `AILook` never written | `lua/mask_select_object_polygon.json` |
| `fixture_lensblur_object.json` | the `LensBlur` struct, which asks for the AI update | `lua/lensblur_object.json` |

`key_classes.txt` is no golden: it is the registry's class of every key
(`number`, `integer`, `flag`, `maskenum`, `compound`, ..., or `never`),
generated by the same test from `lrg-develop`'s registry for the plugin
spec. Key names only.

Nothing here comes from a private source: the inputs are hand-written
tables, models built in the test with synthetic sync ids (twenty zeros and
twelve hex digits; a derived SHA-256 id cannot be told from a catalog's),
and the committed, scrubbed fixtures in `../lua/`.

## Re-blessing

After an intended change to the writer or the registry, or when an
experiment has decided an option (follow the steps on the wiki page first),
regenerate and review the diff:

```bash
cd server-rs
LRG_BLESS=1 cargo test -p lrg-develop --test lua_wire_goldens
```

A file in this folder that no case writes fails the test, so delete a
golden together with its case. The blessing run skips the read-back test
(it would race the files being written); run the command once more without
`LRG_BLESS` to check them. The plugin spec reads whatever is here, so
re-run `busted` from the repository root after re-blessing.
