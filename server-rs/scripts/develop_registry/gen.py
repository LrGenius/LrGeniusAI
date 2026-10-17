"""Render the crs key registry (``spec.py``) as Markdown, with observed statistics.

Reads one or two inventories from ``parse_xmp.py inventory``:

* ``--inventory``       - the stratified sample (plus presets); it supplies the
  "Observed" column (n, min / p5 / p95 / max, bool spelling, container shape).
* ``--full-inventory``  - optional, the whole corpus; used only where the sample
  has no data (marked "full corpus only") and for the coverage check.

Writes to ``--out-dir``:

* ``registry_tables.md`` - sections 3-7 of the registry (global keys, preset
  header, correction, mask tool, nested structs). With ``--head``/``--tail`` the
  given Markdown files are wrapped around the tables instead, as ``registry.md``.
* ``counts.json``        - keys and rows per policy class and level.

The "Observed" column never shows raw string values by default: they can be file
names, lens or camera models or your own mask names. Only keys whose values are
Adobe enum labels (``SAMPLE_KEYS``) show their top values; every other string
key shows "N distinct values". ``--private-samples`` prints the raw top values
for all keys - for your own inspection only; such output must never be
committed or used to seed ``table.rs``.

The public Rust table is seeded from ``spec.py``'s rows (``import spec``), never
from the rendered Markdown's "Observed" column.

It then prints every ``(level, key)`` that the inventory saw but no ``spec.py`` row
covers. That list must be empty before the registry is used to seed the Rust
table; ``--strict`` turns a non-empty list into a failing exit code.

Example::

    uv run gen.py --inventory "$OUT/inventory.json" --out-dir "$OUT"

``$OUT`` is a folder outside the repository; the tool refuses to write inside it.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path

import spec
from outpath import require_outside_repo

FILTER_LEVELS = [
    "nested:Filters",
    "nested:Images",
    "nested:ImageGroup",
    "nested:Alpha",
    "nested:ColorVariations",
    "nested:BlackLevels",
    "nested:Linearization",
    "nested:FilterList",
]
# String-valued keys whose values are Adobe enum labels, safe to show as samples.
SAMPLE_KEYS = frozenset(
    {
        "What",
        "WhiteBalance",
        "ProcessVersion",
        "Version",
        "ToneCurveName",
        "ToneCurveName2012",
        "LensProfileSetup",
        "Method",
        "SourceState",
        "sourceState",
        "SpotType",
        "spotType",
        "fill_method",
        "pm_source_type",
        "PresetType",
        "ConvertToGrayscale",
        "CropConstrainToWarp",
        "Orientation",
        "orientation",
    }
)
COLUMNS = (
    "Key",
    "Type",
    "UI range / unit",
    "Stored \u2194 UI",
    "Default raw / non-raw",
    "Observed: n; min / p5 / p95 / max",
    "PV / version",
    "Policy",
    "Notes",
)


def index(inv: dict) -> dict[str, dict[str, dict]]:
    out: defaultdict[str, dict[str, dict]] = defaultdict(dict)
    for v in inv["keys"].values():
        out[v["level"]][v["key"]] = v
    return out


def fmt(x: float | None) -> str:
    if x is None:
        return ""
    if abs(x) >= 1e5:
        return "%d" % x
    return ("%.4f" % x).rstrip("0").rstrip(".") if x != int(x) else "%d" % x


class Generator:
    def __init__(self, sample: dict, full: dict, private_samples: bool = False) -> None:
        self.private_samples = private_samples
        self.sources = ((index(sample), ""), (index(full), " (full corpus only)"))
        self.full_index = self.sources[1][0]
        self.policy_keys: Counter[str] = Counter()
        self.policy_by_level: defaultdict[str, Counter[str]] = defaultdict(Counter)
        self.policy_rows: Counter[str] = Counter()
        self.gated = 0
        self.covered: defaultdict[str, set[str]] = defaultdict(set)

    # -- groups whose member lists come from the data ------------------------

    def patched_nested(self) -> list[tuple]:
        fi = self.full_index
        filters_keys = sorted({k for lvl in FILTER_LEVELS for k in fi.get(lvl, {})} - {"GenAIInfo"}) + [
            "IsSignalForDelete"
        ]
        pm_levels = ("nested:RetouchArea", "nested:RemoveAreas", "nested:pm_patch_variations")
        pm_keys = sorted({k for lvl in pm_levels for k in fi.get(lvl, {}) if k.startswith("pm_")})
        retouch_mask_keys = sorted(fi.get("mask-tool(retouch)", {}).keys())
        out = []
        for lvl, title, rows in spec.NESTED:
            new = []
            for r in rows:
                r = list(r)
                if r[0].startswith("pm_*"):
                    r[0] = "pm_* (content-aware / generative patch state, %d keys)" % len(pm_keys)
                    r[8] = pm_keys
                elif r[0].startswith("Filters payload"):
                    r[0] = "Filters payload (%d keys)" % len(filters_keys)
                    r[8] = filters_keys
                    r[7] = ", ".join(filters_keys) + ". IsSignalForDelete seen only inside applied-Preset/Parameters."
                elif r[0].startswith("GenAIInfo"):
                    r[8] = ["GenAIInfo", "Name", "UUID", "SoftwareAgent", "Timestamp", "ParameterPairs"]
                elif r[0].startswith("Mask/Ellipse"):
                    r[0] = "Masks items (Mask/Ellipse heal shapes, Mask/Paint strokes; %d keys)" % len(
                        retouch_mask_keys
                    )
                    r[8] = retouch_mask_keys
                    r[7] = ", ".join(retouch_mask_keys) + "."
                new.append(tuple(r))
            out.append((lvl, title, new))
        return out

    # -- the "Observed" column -------------------------------------------------

    def observed(self, levels: list[str], members: list[str]) -> str:
        """Observed statistics from the first inventory that has data."""
        for idx, tag in self.sources:
            rows = [(lvl, idx.get(lvl, {}).get(m)) for lvl in levels for m in members]
            rows = [(lvl, v) for lvl, v in rows if v]
            if not rows:
                continue
            if len(members) > 1:
                found = len({v["key"] for _, v in rows})
                nmax = max(v["n"] for _, v in rows)
                mins = [v["min"] for _, v in rows if v["min"] is not None and not v["bools"]]
                maxs = [v["max"] for _, v in rows if v["max"] is not None and not v["bools"]]
                rng = ""
                if mins and all(v["num_n"] for _, v in rows):
                    rng = "; range across keys %s .. %s" % (fmt(min(mins)), fmt(max(maxs)))
                return "%d/%d keys seen, up to n=%d per key%s%s" % (found, len(members), nmax, rng, tag)
            lvl, v = rows[0]
            k = v["kinds"]
            sc = k.get("sidecar", 0)
            pr = sum(c for kk, c in k.items() if kk != "sidecar")
            head = "n=%d sc + %d pr" % (sc, pr)
            if lvl != levels[0]:
                head += " [in %s]" % lvl.replace("nested:", "")
            return "%s; %s%s" % (head, self._body(v), tag)
        return "not observed"

    def _samples(self, key: str, values: list[str], width: int) -> str | None:
        """Quoted sample values, or ``None`` when this key's strings must not be shown."""
        if not (self.private_samples or key in SAMPLE_KEYS):
            return None
        return "; ".join('"%s"' % t[:width] for t in values)

    def _body(self, v: dict) -> str:
        if v["bools"]:
            return ", ".join("%s %d" % (b, c) for b, c in sorted(v["bools"].items(), key=lambda x: -x[1]))
        n_values = max(1, v["n"] - sum(v["containers"].values()))
        if v["num_n"] and v["num_n"] >= 0.8 * n_values and not v["containers"]:
            return "%s / %s / %s / %s" % (fmt(v["min"]), fmt(v["p5"]), fmt(v["p95"]), fmt(v["max"]))
        if v["containers"] or v["item_fmt"]:
            if "Alt" in v["containers"]:
                tops = [t[0] for t in v["top"] if t[0]][:3]
                if not tops:
                    return "Alt x-default (empty)"
                shown = self._samples(v["key"], tops, 30)
                return "Alt x-default: " + (shown if shown is not None else "%d distinct values" % v["distinct"])
            if v["item_fmt"]:
                return "Seq; items %s" % ", ".join(v["item_fmt"])
            return "struct" if "struct" in v["forms"] else "Seq"
        if "struct" in v["forms"]:
            return "struct"
        tops = [t[0] for t in v["top"] if not t[0].startswith("[item]")][:3]
        non_empty = [t for t in tops if t]
        if not non_empty:
            return 'always ""'
        if v["key"] in ("UUID", "CorrectionSyncID", "MaskSyncID") or all(
            re.match(r"^[0-9A-F]{32}$", t) for t in non_empty
        ):
            return "32-hex, %d distinct" % v["distinct"]
        shown = self._samples(v["key"], tops, 34)
        return shown if shown is not None else "string, %d distinct values" % v["distinct"]

    # -- tables ---------------------------------------------------------------

    def _count(self, level_name: str, policy: str, members: list[str]) -> None:
        p = policy.replace("\u2020", "")
        n = max(1, len(members))
        self.policy_keys[p] += n
        self.policy_by_level[level_name][p] += n
        self.policy_rows[p] += 1
        if "\u2020" in policy:
            self.gated += n

    def table(self, rows: list[tuple], levels: list[str], level_name: str) -> str:
        out = ["| " + " | ".join(COLUMNS) + " |", "|" + "---|" * len(COLUMNS)]
        for r in rows:
            key, typ, ui, sc, dflt, pv, pol, notes = r[:8]
            grouped = len(r) > 8
            members = r[8] if grouped else [key]
            for m in members:
                for lvl in levels:
                    self.covered[lvl].add(m)
            self._count(level_name, pol, members)
            obs = self.observed(levels, members) if members else "see global keys"
            cells = [key if grouped else "`%s`" % key, typ, ui, sc, dflt, obs, pv, "**%s**" % pol, notes]
            out.append("| " + " | ".join(c.replace("|", "\\|").replace("\n", " ") for c in cells) + " |")
        return "\n".join(out)

    def render(self) -> str:
        parts = ["## 3. Global keys (top level of the develop description)\n"]
        for i, (title, rows) in enumerate(spec.G):
            parts.append("### 3.%d %s\n" % (i + 1, title))
            parts.append(self.table(rows, ["global", "look-parameters"], "global") + "\n")

        parts.append("## 4. Preset header (preset and profile files only)\n")
        parts.append(self.table(spec.HEADER, ["header", "header(look)", "global"], "header") + "\n")

        parts.append("## 5. Correction level (one `MaskGroupBasedCorrections` item)\n")
        parts.append(self.table(spec.CORR, ["correction"], "correction") + "\n")

        parts.append("## 6. Mask-tool level (one `CorrectionMasks` item)\n")
        for i, (title, rows) in enumerate(spec.MASK):
            parts.append("### 6.%d %s\n" % (i + 1, title))
            parts.append(self.table(rows, ["mask-tool", "nested:Gesture"], "mask-tool") + "\n")

        parts.append("## 7. Nested structures\n")
        extra_levels = {
            "nested:RetouchArea": [
                "nested:RetouchArea",
                "nested:RemoveAreas",
                "mask-tool(retouch)",
                "nested:pm_patch_variations",
            ],
            "nested:Filters": FILTER_LEVELS + ["nested:GenAIInfo"],
            "nested:CorrectionRangeMask": ["nested:CorrectionRangeMask", "nested:AreaModels"],
            "nested:Gesture": ["nested:Gesture", "nested:Points"],
        }
        for i, (lvl, title, rows) in enumerate(self.patched_nested()):
            parts.append("### 7.%d %s\n" % (i + 1, title))
            levels = extra_levels.get(lvl, [lvl])
            parts.append(self.table(rows, levels, lvl.replace("nested:", "struct:")) + "\n")
        return "\n".join(parts)

    def uncovered(self) -> list[tuple[str, str]]:
        """Keys the full inventory saw that no spec row covers."""
        skip_levels = {"header(look)", "look-parameters"}
        out = []
        for lvl, keys in self.full_index.items():
            if lvl in skip_levels:
                continue
            out.extend((lvl, k) for k in keys if k not in self.covered.get(lvl, set()))
        return sorted(out)

    def counts(self) -> dict:
        return {
            "keys": dict(self.policy_keys),
            "rows": dict(self.policy_rows),
            "by_level": {lvl: dict(c) for lvl, c in self.policy_by_level.items()},
            "gated": self.gated,
        }


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Render the crs key registry as Markdown.")
    ap.add_argument("--inventory", required=True, help="inventory.json of the sample (+ presets)")
    ap.add_argument("--full-inventory", help="inventory of the whole corpus (default: --inventory)")
    ap.add_argument("--out-dir", required=True)
    ap.add_argument("--head", help="Markdown to put before the tables (writes registry.md)")
    ap.add_argument("--tail", help="Markdown to put after the tables (writes registry.md)")
    ap.add_argument("--strict", action="store_true", help="exit 1 when an observed key has no spec row")
    ap.add_argument(
        "--private-samples",
        action="store_true",
        help="show raw top string values for every key (private output: file names, lenses, cameras, mask names)",
    )
    args = ap.parse_args(argv)
    require_outside_repo(args.out_dir)

    sample = json.loads(Path(args.inventory).read_text(encoding="utf-8"))
    full = json.loads(Path(args.full_inventory).read_text(encoding="utf-8")) if args.full_inventory else sample
    gen = Generator(sample, full, private_samples=args.private_samples)
    tables = gen.render()

    out = Path(args.out_dir)
    out.mkdir(parents=True, exist_ok=True)
    if args.head or args.tail:
        head = Path(args.head).read_text(encoding="utf-8") if args.head else ""
        tail = Path(args.tail).read_text(encoding="utf-8") if args.tail else ""
        (out / "registry.md").write_text("\n".join(p for p in (head, tables, tail) if p), encoding="utf-8")
    else:
        (out / "registry_tables.md").write_text(tables, encoding="utf-8")
    counts = gen.counts()
    (out / "counts.json").write_text(json.dumps(counts, indent=1), encoding="utf-8")

    unc = gen.uncovered()
    print("UNCOVERED", unc)
    print("POLICY (keys)", counts["keys"], "total", sum(counts["keys"].values()))
    print("POLICY (rows)", counts["rows"], "total", sum(counts["rows"].values()))
    print("GATED", counts["gated"])
    for lvl, c in counts["by_level"].items():
        print(" ", lvl, c)
    return 1 if unc and args.strict else 0


if __name__ == "__main__":
    sys.exit(main())
