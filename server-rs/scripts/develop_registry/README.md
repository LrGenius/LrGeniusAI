# develop_registry — research and fixture tools for `lrg-develop`

Build-time tools behind the backend's native Lightroom develop model (the
`lrg-develop` crate, step 1 of "AI Edit via XMP"). They answer three questions:

1. **Which `crs:` keys exist, and how does Lightroom write them?** — an inventory
   over Lightroom Classic's bundled presets/profiles and a sample of real
   sidecars (`parse_xmp.py`, `scan_corpus.py`, `sample.py`, `structform.py`).
2. **What goes into the key registry?** — `spec.py` holds the hand-maintained
   rows (type, range, UI scaling, defaults, policy class) and `gen.py` renders
   them as Markdown with observed statistics. This seeds the static Rust table
   once; after that the Rust table is the source of truth.
3. **What do real `getDevelopSettings()` values look like?** — `dump_training.py`
   reads the backend's training table and `extract_fixtures.py` turns a few rows
   into scrubbed test fixtures under `server-rs/testdata/develop/lua/`.

Nothing here ships with the backend. The tools are stdlib-only Python ≥ 3.12;
only `dump_training.py` needs `pylance`, in the opt-in `lance` group.

> **Outputs derived from your own catalog, sidecars or training table are
> private and must never be committed.** That covers `corpus_meta.json`,
> `corpus_keyfreq.json`, `sample_files.json`, `inventory*.json`, the registry
> Markdown and the training dump — they contain paths, file names, photo ids
> and capture data. The **only** exception is what `extract_fixtures.py` writes:
> it scrubs its output, checks it in a private temporary folder and only then
> replaces the fixtures it owns; if the check fails the fixture folder is left
> untouched. Write every other output to a folder outside the repository.

The tools enforce this: they refuse an output path inside the repository
(`outpath.py`), except `extract_fixtures.py` into `server-rs/testdata/develop/`.
A `.gitignore` here also keeps any `*.json` and `out/` in this folder out of
git.

## Inputs

| Variable | Meaning |
|---|---|
| `LRG_LRC_PRESETS_DIR` | Lightroom Classic's bundled settings, e.g. `/Applications/Adobe Lightroom Classic/Adobe Lightroom Classic.app/Contents/Resources/Settings` |
| `LRG_XMP_CORPUS_DIR` | root of a folder tree with Lightroom `.xmp` sidecars |
| `LRG_ACR_PRESETS_DIR` | optional: Camera Raw's settings (`/Library/Application Support/Adobe/CameraRaw/Settings`) |

Every tool also takes these as arguments (`--lrc-presets`, `--corpus`, ...).
Sidecar rules everywhere: `* [conflicted].xmp` copies are skipped, and only
files containing `crs:ProcessVersion` count as Lightroom sidecars.

## Tools

Run from this directory; `OUT` is any folder **outside** the repository.

### `scan_corpus.py` — pass 1 over the sidecars

Byte-level scan: strata (top-level folder, writer version, process version,
element vs attribute form) and feature flags per sidecar, plus how many files
mention each `crs:` name.

```bash
uv run scan_corpus.py --corpus "$LRG_XMP_CORPUS_DIR" --out-dir "$OUT"
```

### `sample.py` — stratified sample

Guarantees the rare shapes (element form, older writers, snapshots, retouch,
filter lists, crops), then fills proportionally by (folder, process version,
masks). Fixed seed, so the same corpus gives the same sample.

```bash
uv run sample.py --meta "$OUT/corpus_meta.json" --size 3000 --out-dir "$OUT"
```

### `parse_xmp.py` — the crs reader and the inventory

A library (`Inventory`, `iter_xmp_files`, `is_lrc_sidecar`, ...) and a CLI.
Handles attribute and element form, `rdf:Seq`/`Bag`/`Alt`, nested
`rdf:Description` and `rdf:parseType="Resource"`. It does not inventory
`crs:Preset/Parameters`, `crs:Look/Parameters` or `crss:SavedSettings` (they are
not the photo's own settings); each skip is counted under `excluded`.

```bash
uv run parse_xmp.py inventory --lrc-presets "$LRG_LRC_PRESETS_DIR" \
    --sidecar-list "$OUT/sample_files.json" --out "$OUT/inventory.json" --strict
# whole corpus, for the coverage check in gen.py:
uv run parse_xmp.py inventory --lrc-presets "$LRG_LRC_PRESETS_DIR" \
    --sidecar-list "$OUT/corpus_meta.json" --out "$OUT/inventory_full.json"
uv run parse_xmp.py show "$OUT/inventory.json" --levels global,correction,mask-tool
```

### `structform.py` — how nested structs are spelled

Counts `parseType=Resource` vs `rdf:Description` child vs attributes on the
element, per struct key. A parser must accept every form listed.

```bash
uv run structform.py --lrc-presets "$LRG_LRC_PRESETS_DIR" --sidecar-list "$OUT/sample_files.json"
```

### `gen.py` + `spec.py` — the registry as Markdown

`spec.py` is the hand-written table; `gen.py` renders it with the "Observed"
column filled from the inventory and prints every observed key that no row
covers (`--strict` makes that fatal). Rows come from local observation only;
nothing is taken from ExifTool or darktable, and keys known only from ExifTool
were deliberately left out. Key names known from Camera Raw but never observed
in Lightroom Classic's output are marked as such and carry no value semantics.

The "Observed" column shows raw string values only for Adobe enum keys
(`SAMPLE_KEYS`); other string keys show "N distinct values", because their
values can be file names, lens or camera models or your own mask names.
`--private-samples` shows everything, for your eyes only. When step 1a seeds
`registry/table.rs`, it imports `spec.py`'s rows, never the rendered Markdown.

```bash
uv run gen.py --inventory "$OUT/inventory.json" --full-inventory "$OUT/inventory_full.json" \
    --out-dir "$OUT" --strict
```

### `dump_training.py` — the training table as JSON

Reads `edit_training.lance` from the backend's Lance store
(`<dir>/lrgenius-lance/` next to `--db-path <dir>/lrgenius.db`).

```bash
OUT=$(mktemp -d)
uv run --group lance dump_training.py ~/path/to/lrgenius-lance/edit_training.lance "$OUT/training_rows.json"
```

### `extract_fixtures.py` — scrubbed fixtures for the repository

Picks a small set of rows (currently 16 plus a synthetic top-level `[]`) that
together cover the shapes the Lua/JSON reader must handle (every mask kind,
intersect/subtract, local curves, `PointColors`, `AILook`, Adobe, adaptive and
camera-restricted Looks, retouch `pm_*` keys, `LensBlur` and `FilterList` as
`[]` and as objects, process versions 11.0 and 15.4, ...), scrubs them and writes
one file per shape plus `manifest.json`. Numbers keep the exact token
`JSON.lua` wrote. The scrubbing rules and the hygiene check are described in the
module docstring and in `server-rs/testdata/develop/README.md`. Hand-written
fixtures live next to the generated ones as `hand_*.json` and are never
touched by a run.

```bash
uv run extract_fixtures.py "$OUT/training_rows.json" --out-dir ../../testdata/develop/lua
# the check alone (stdlib only, also what pre-commit and CI run):
python3 extract_fixtures.py --check-only --out-dir ../../testdata/develop/lua --root ../../testdata/develop
```

## Lint

The pre-commit hook `develop-fixture-hygiene` and the docs-check job in
`lint-format.yml` run the fixture check. No hook runs ruff yet; keep the code
clean by hand:

```bash
ruff format . && ruff check .
```
