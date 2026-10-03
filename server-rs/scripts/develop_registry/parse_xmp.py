"""Generic reader for Camera Raw settings (``crs:``) in XMP files, and the key inventory.

Two uses:

* **Library.** ``iter_xmp_files``, ``is_lrc_sidecar`` and ``Inventory`` are imported
  by the other tools in this directory.
* **CLI.** ``inventory`` walks Lightroom Classic's bundled presets/profiles, optional
  ACR and user preset folders and a list of sidecars, and writes ``inventory.json``:
  per ``(level, key)`` statistics (value range, number formatting, bool spelling,
  container types, which files/kinds it occurs in) plus serialization observations.
  ``show`` prints one line per key of an existing inventory for a quick look.

The walker understands everything Lightroom writes:

* attribute form (``crs:Exposure2012="+0.50"``) and element form
  (``<crs:Exposure2012>+0.50</crs:Exposure2012>``), mixed freely;
* ``rdf:Seq`` / ``rdf:Bag`` / ``rdf:Alt`` containers (``Alt`` reports ``x-default``);
* nested structs written as an ``rdf:Description`` child, as attributes on the
  property element, or with ``rdf:parseType="Resource"``;
* several top-level ``rdf:Description`` elements in one file.

Three subtrees are deliberately *not* inventoried, because they are not the
photo's own settings:

* ``crs:Preset/Parameters`` (a copy of the last applied preset) - only the
  ``Preset`` record's own direct fields are counted;
* ``crs:Look/Parameters`` in a sidecar (Adobe's profile definition, filled in by
  Lightroom from its profile library);
* ``crss:SavedSettings`` (snapshots).

Each skip is counted under ``excluded`` in the output. A profile file's own
top-level keys *are* a profile definition; they are recorded under the level
``look-parameters`` so they never mix with a photo's global keys.

Sidecar rules: ``* [conflicted].xmp`` files (sync-conflict copies) are skipped,
and only files containing ``crs:ProcessVersion`` count as Lightroom sidecars.

Example::

    uv run parse_xmp.py inventory --lrc-presets "$LRG_LRC_PRESETS_DIR" \\
        --sidecar-list "$OUT/sample_files.json" --out "$OUT/inventory.json"
    uv run parse_xmp.py show "$OUT/inventory.json" --levels global,correction

``$OUT`` is a folder outside the repository; the tool refuses to write inside it.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import xml.etree.ElementTree as ET
from collections import Counter, defaultdict
from collections.abc import Iterable, Iterator
from pathlib import Path

from outpath import require_outside_repo

RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
CRS = "http://ns.adobe.com/camera-raw-settings/1.0/"
CRSS = "http://ns.adobe.com/camera-raw-saved-settings/1.0/"
XML = "http://www.w3.org/XML/1998/namespace"

#: Keys that belong to a preset/profile file's header rather than to the settings.
HEADER_KEYS = frozenset(
    {
        "PresetType",
        "Cluster",
        "UUID",
        "SupportsAmount",
        "SupportsAmount2",
        "SupportsColor",
        "SupportsMonochrome",
        "SupportsHighDynamicRange",
        "SupportsNormalDynamicRange",
        "SupportsSceneReferred",
        "SupportsOutputReferred",
        "RequiresRGBTables",
        "CameraModelRestriction",
        "Copyright",
        "ContactInfo",
        "Name",
        "ShortName",
        "SortName",
        "Group",
        "Description",
        "ShowInPresets",
        "ShowInQuickActions",
        "PresetSubtype",
        "Stubbed",
    }
)

NUM_RE = re.compile(r"^[+-]?(\d+\.?\d*|\.\d+)$")
TABLE_RE = re.compile(r"^Table_[0-9A-Fa-f]{32}$")
CURVE_KEYS = frozenset({"MainCurve", "RedCurve", "GreenCurve", "BlueCurve"})

# ---------------------------------------------------------------------------
# File discovery
# ---------------------------------------------------------------------------


def iter_xmp_files(root: str | os.PathLike[str]) -> Iterator[Path]:
    """Yield every ``*.xmp`` below ``root`` in a stable (sorted) order."""
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames.sort()
        for name in sorted(filenames):
            if name.lower().endswith(".xmp") and not name.startswith("._"):
                yield Path(dirpath) / name


def is_conflicted(path: str | os.PathLike[str]) -> bool:
    """True for sync-conflict copies such as ``IMG_0001 [conflicted].xmp``."""
    return "[conflicted]" in Path(path).name


def is_lrc_sidecar(data: bytes) -> bool:
    """True when the file carries develop settings (it has ``crs:ProcessVersion``)."""
    return b"crs:ProcessVersion" in data


def split_tag(tag: str) -> tuple[str, str]:
    """``'{ns}local'`` -> ``('ns', 'local')``."""
    if tag.startswith("{"):
        ns, local = tag[1:].split("}", 1)
        return ns, local
    return "", tag


def has_struct_content(el: ET.Element) -> bool:
    """True when a property element holds a struct rather than a plain value."""
    return (
        any(split_tag(a)[0] == CRS for a in el.attrib)
        or len(el) > 0
        or el.attrib.get(f"{{{RDF}}}parseType") == "Resource"
    )


# ---------------------------------------------------------------------------
# Value classification
# ---------------------------------------------------------------------------


def classify_value(v: str) -> str:
    if v in ("True", "False", "true", "false"):
        return "bool"
    if NUM_RE.match(v):
        return "num"
    return "str"


def item_format(s: str) -> str:
    """Coarse shape of a Seq item (curve points, float lists, dab strings, ...)."""
    s = s.strip()
    if re.match(r"^-?\d+, -?\d+$", s):
        return "int, int"
    if re.match(r"^-?\d+(\.\d+)?, -?\d+(\.\d+)?$", s):
        return "num, num"
    if re.match(r"^(-?\d+\.\d+, ){5,}-?\d+\.\d+$", s):
        return "float-list(%d)" % (s.count(",") + 1)
    if re.match(r"^-?\d+\.\d+$", s):
        return "float"
    if re.match(r"^[a-zA-Z] ", s):
        return "token-string"
    return "other"


# ---------------------------------------------------------------------------
# Inventory
# ---------------------------------------------------------------------------


class Stat:
    """Statistics for one ``(level, key)``."""

    __slots__ = (
        "n",
        "files",
        "lastfile",
        "forms",
        "kinds",
        "nums",
        "strs",
        "plus",
        "pos_noplus",
        "decimals",
        "bools",
        "whats",
        "containers",
        "items",
        "item_fmt",
        "pv",
        "intish",
    )

    def __init__(self) -> None:
        self.n = 0
        self.files = 0
        self.lastfile: str | None = None
        self.forms: Counter[str] = Counter()
        self.kinds: Counter[str] = Counter()
        self.nums: list[float] = []
        self.strs: Counter[str] = Counter()
        self.plus = 0
        self.pos_noplus = 0
        self.decimals: Counter[int] = Counter()
        self.bools: Counter[str] = Counter()
        self.whats: Counter[str] = Counter()
        self.containers: Counter[str] = Counter()
        self.items: Counter[int] = Counter()
        self.item_fmt: Counter[str] = Counter()
        self.pv: Counter[str | None] = Counter()
        self.intish = 0

    def to_json(self, level: str, key: str) -> dict:
        nums = sorted(self.nums)

        def pct(p: float) -> float | None:
            if not nums:
                return None
            i = min(len(nums) - 1, max(0, int(round(p * (len(nums) - 1)))))
            return nums[i]

        return {
            "level": level,
            "key": key,
            "n": self.n,
            "files": self.files,
            "forms": dict(self.forms),
            "kinds": dict(self.kinds),
            "num_n": len(nums),
            "min": pct(0),
            "p5": pct(0.05),
            "p50": pct(0.5),
            "p95": pct(0.95),
            "max": pct(1),
            "plus": self.plus,
            "pos_noplus": self.pos_noplus,
            "decimals": {str(k): v for k, v in self.decimals.items()},
            "bools": dict(self.bools),
            "intish": self.intish,
            "whats": dict(self.whats.most_common(12)),
            "containers": dict(self.containers),
            "items": {str(k): v for k, v in self.items.items()},
            "item_fmt": dict(self.item_fmt),
            "top": self.strs.most_common(12),
            "distinct": len(self.strs),
            "pv": {str(k): v for k, v in self.pv.items()},
        }


def child_level(level: str, key: str) -> str:
    """Level name for the items/struct under ``key`` found at ``level``."""
    if key == "MaskGroupBasedCorrections":
        return "correction"
    if key in (
        "GradientBasedCorrections",
        "CircularGradientBasedCorrections",
        "PaintBasedCorrections",
    ):
        return "correction-legacy"
    if key == "DepthBasedCorrections":
        return "correction-depth"
    if key == "CorrectionMasks":
        return "mask-tool"
    if key == "Masks":
        return "mask-tool(retouch)" if level.startswith("nested:RetouchArea") else "mask-tool"
    if key == "RetouchAreas":
        return "nested:RetouchArea"
    return "nested:" + key


class Inventory:
    """Accumulates per-key statistics over many XMP files."""

    def __init__(self) -> None:
        self.stats: defaultdict[tuple[str, str], Stat] = defaultdict(Stat)
        self.ser: defaultdict[tuple, Counter] = defaultdict(Counter)
        self.excluded: Counter[str] = Counter()
        self.files: Counter[str] = Counter()
        self.skipped: Counter[str] = Counter()
        self.errors: list[tuple[str, str]] = []
        self.what_keys: defaultdict[str, Counter[str]] = defaultdict(Counter)
        self.file_forms: Counter[tuple[str, str]] = Counter()

    # -- recording ---------------------------------------------------------

    def _rec(self, level: str, key: str, value: str | None, form: str, ctx: dict) -> None:
        if TABLE_RE.match(key):
            key = "Table_<md5>"
            if value is not None and len(value) > 60:
                value = "<blob len %d>" % (len(value) // 1000 * 1000)
        st = self.stats[(level, key)]
        st.n += 1
        if st.lastfile != ctx["file"]:
            st.files += 1
            st.lastfile = ctx["file"]
        st.forms[form] += 1
        file_form = "elemfile" if ctx.get("elemform") else "attrfile"
        self.ser[("form", level, key, file_form)][form] += 1
        if value is not None and NUM_RE.match(value) and float(value) == 0:
            if value.startswith("+0"):
                self.ser[("plus-zero", level, key)][value] += 1
            if value.startswith("-0"):
                self.ser[("minus-zero", level, key)][value] += 1
        st.kinds[ctx["kind"]] += 1
        st.pv[ctx.get("pv")] += 1
        if ctx.get("what"):
            st.whats[ctx["what"]] += 1
        if value is None:
            return
        c = classify_value(value)
        if c == "bool":
            st.bools[value] += 1
            self.ser[("bool", level, ctx["kind"])][value] += 1
        elif c == "num":
            f = float(value)
            st.nums.append(f)
            if "." in value:
                st.decimals[len(value.split(".", 1)[1])] += 1
            else:
                st.decimals[0] += 1
                st.intish += 1
            if f > 0:
                if value.startswith("+"):
                    st.plus += 1
                else:
                    st.pos_noplus += 1
        if len(st.strs) < 400 or value in st.strs:
            st.strs[value[:120]] += 1

    @staticmethod
    def _level_for(level: str, key: str, ctx: dict) -> str:
        if level != "global":
            return level
        if ctx["kind"].endswith("profile"):
            return "header(look)" if key in HEADER_KEYS else "look-parameters"
        return "header" if key in HEADER_KEYS else "global"

    # -- walking -----------------------------------------------------------

    @staticmethod
    def _what(node: ET.Element) -> str | None:
        w = node.attrib.get(f"{{{CRS}}}What")
        if w is None:
            e = node.find(f"{{{CRS}}}What")
            if e is not None:
                w = (e.text or "").strip()
        if w is None:
            d = node.find(f"{{{RDF}}}Description")
            if d is not None:
                return Inventory._what(d)
        return w

    def _walk_desc(self, node: ET.Element, level: str, ctx: dict) -> None:
        what = self._what(node)
        if what is not None:
            ctx = dict(ctx, what=what)
        for k, v in node.attrib.items():
            ns, local = split_tag(k)
            if ns != CRS:
                continue
            lvl = self._level_for(level, local, ctx)
            self._rec(lvl, local, v, "attr", ctx)
            if lvl.startswith("mask-tool") and what:
                self.what_keys[what][local] += 1
        for ch in node:
            ns, local = split_tag(ch.tag)
            if ns == RDF and local == "Description":
                self._walk_desc(ch, level, ctx)
            elif ns == CRSS:
                self.excluded["crss:" + local] += 1
            elif ns == CRS:
                self._handle_prop(local, ch, level, ctx)

    def _handle_preset_record(self, el: ET.Element, ctx: dict) -> None:
        """``crs:Preset``: count its own direct fields, never its ``Parameters``."""
        self.excluded["crs:Preset/Parameters"] += 1
        d = el.find(f"{{{RDF}}}Description")
        if d is None:
            d = el
        for k, v in d.attrib.items():
            ns, local = split_tag(k)
            if ns == CRS:
                self._rec("nested:Preset", local, v, "attr", ctx)
        for c in d:
            ns, local = split_tag(c.tag)
            if ns != CRS or local == "Parameters":
                continue
            if c.find(f"{{{RDF}}}Alt") is not None:
                self._rec("nested:Preset", local, None, "alt", ctx)
            elif len(c) == 0:
                self._rec("nested:Preset", local, (c.text or "").strip(), "elem", ctx)

    def _handle_prop(self, key: str, el: ET.Element, level: str, ctx: dict) -> None:
        lvl = self._level_for(level, key, ctx)
        if level == "global" and key == "Preset":
            self._handle_preset_record(el, ctx)
            return
        if level == "nested:Look" and key == "Parameters":
            self.excluded["crs:Look/Parameters"] += 1
            return
        conts = [c for c in el if split_tag(c.tag)[0] == RDF and split_tag(c.tag)[1] in ("Seq", "Bag", "Alt")]
        if conts:
            self._handle_container(key, conts[0], lvl, ctx)
            return
        if has_struct_content(el):
            self._rec(lvl, key, None, "struct", ctx)
            self._walk_desc(el, child_level(lvl, key), dict(ctx, parent=key))
            return
        self._rec(lvl, key, (el.text or "").strip(), "elem", ctx)

    def _handle_container(self, key: str, cont: ET.Element, lvl: str, ctx: dict) -> None:
        ctype = split_tag(cont.tag)[1]
        items = [li for li in cont if split_tag(li.tag) == (RDF, "li")]
        st = self.stats[(lvl, key)]
        st.containers[ctype] += 1
        self.ser[("container", key)][ctype] += 1
        if ctype == "Alt":
            val = None
            langs = []
            for li in items:
                lang = li.attrib.get(f"{{{XML}}}lang", "?")
                langs.append(lang)
                if lang == "x-default":
                    val = li.text or ""
            self.ser[("alt-lang", key)][tuple(sorted(langs))] += 1
            self._rec(lvl, key, val if val is not None else "", "alt", ctx)
            return
        self._rec(lvl, key, None, "seq" if ctype == "Seq" else "bag", ctx)
        sub = child_level(lvl, key)
        for li in items:
            if has_struct_content(li):
                self._walk_desc(li, sub, dict(ctx, parent=key))
                continue
            t = li.text or ""
            st.item_fmt[item_format(t)] += 1
            if len(st.strs) < 400:
                st.strs["[item] " + t[:100]] += 1
            self.ser[("item", key)][item_format(t)] += 1
            if key.startswith("ToneCurve") or key in CURVE_KEYS:
                spacing = "comma-space" if ", " in t else ("comma" if "," in t else "other")
                self.ser[("curve-item-spacing", key)][spacing] += 1
        st.items[len(items)] += 1

    # -- public API ----------------------------------------------------------

    def add_file(self, path: str | os.PathLike[str], kind: str) -> bool:
        """Parse one file. ``kind`` is ``lrc``, ``acr``, ``user`` or ``sidecar``.

        Returns False when the file was skipped (conflict copy, not a sidecar,
        unreadable or unparsable).
        """
        path = str(path)
        if kind == "sidecar" and is_conflicted(path):
            self.skipped["conflicted"] += 1
            return False
        try:
            raw = Path(path).read_bytes()
        except OSError as e:
            self.errors.append((path, str(e)[:100]))
            return False
        if kind == "sidecar" and not is_lrc_sidecar(raw):
            self.skipped["no-ProcessVersion"] += 1
            return False
        try:
            root = ET.fromstring(raw)
        except ET.ParseError as e:
            self.errors.append((path, str(e)[:100]))
            return False
        rdf = root if split_tag(root.tag) == (RDF, "RDF") else root.find(f"{{{RDF}}}RDF")
        if rdf is None:
            self.errors.append((path, "no rdf:RDF"))
            return False
        ptype = re.search(rb'crs:PresetType(?:="|>)([A-Za-z]+)', raw)
        if kind != "sidecar" and ptype and ptype.group(1) == b"Look":
            kind = kind + "-profile"
        pvm = re.search(rb'crs:ProcessVersion(?:="|>)([0-9.]+)', raw)
        elemform = b"<crs:ProcessVersion>" in raw or b"<crs:Version>" in raw
        self.file_forms[(kind, "element-form" if elemform else "attribute-form")] += 1
        self.files[kind] += 1
        ctx = {
            "file": path,
            "kind": kind,
            "pv": pvm.group(1).decode() if pvm else None,
            "elemform": elemform,
        }
        for d in rdf.findall(f"{{{RDF}}}Description"):
            self._walk_desc(d, "global", ctx)
        return True

    def add_files(self, paths: Iterable[str | os.PathLike[str]], kind: str) -> None:
        for p in paths:
            self.add_file(p, kind)

    def to_json(self) -> dict:
        return {
            "files": dict(self.files),
            "skipped": dict(self.skipped),
            "errors": self.errors,
            "excluded": dict(self.excluded),
            "file_forms": {"%s|%s" % k: v for k, v in self.file_forms.items()},
            "keys": {"%s|%s" % (lvl, key): st.to_json(lvl, key) for (lvl, key), st in self.stats.items()},
            "ser": {"|".join(map(str, k)): {str(kk): v for kk, v in c.items()} for k, c in self.ser.items()},
            "what_keys": {w: dict(c) for w, c in self.what_keys.items()},
        }


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def load_sidecar_list(path: str | os.PathLike[str]) -> list[str]:
    """Accept either ``sample.py`` output (a list of paths) or ``scan_corpus.py``'s
    ``corpus_meta.json`` (a list of objects with a ``p`` field)."""
    data = json.loads(Path(path).read_text(encoding="utf-8"))
    return [d["p"] if isinstance(d, dict) else d for d in data]


def cmd_inventory(args: argparse.Namespace) -> int:
    require_outside_repo(args.out)
    inv = Inventory()
    sources = [
        (args.lrc_presets, "lrc"),
        (args.acr_presets, "acr"),
        (args.user_presets, "user"),
    ]
    if not any(d for d, _ in sources) and not args.sidecar_list and not args.sidecars_dir:
        print("nothing to scan: give --lrc-presets and/or --sidecar-list/--sidecars-dir", file=sys.stderr)
        return 2
    for directory, kind in sources:
        if not directory:
            continue
        if not Path(directory).is_dir():
            print(f"not a directory: {directory}", file=sys.stderr)
            return 2
        inv.add_files(iter_xmp_files(directory), kind)
    if args.sidecar_list:
        inv.add_files(load_sidecar_list(args.sidecar_list), "sidecar")
    if args.sidecars_dir:
        inv.add_files(iter_xmp_files(args.sidecars_dir), "sidecar")
    out = inv.to_json()
    out_path = Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(out, indent=0), encoding="utf-8")
    print(f"files {out['files']}; {len(out['keys'])} (level, key) rows; errors {len(out['errors'])}")
    print(f"skipped {out['skipped']}")
    print(f"excluded {out['excluded']}")
    print(f"forms {out['file_forms']}")
    return 1 if out["errors"] and args.strict else 0


def cmd_show(args: argparse.Namespace) -> int:
    d = json.loads(Path(args.inventory).read_text(encoding="utf-8"))
    levels = set(args.levels.split(",")) if args.levels else None

    def f(x: float | None) -> str:
        return "" if x is None else "%g" % x

    for v in sorted(d["keys"].values(), key=lambda v: (v["level"], v["key"])):
        if levels and v["level"] not in levels:
            continue
        k = v["kinds"]
        presets = sum(c for kk, c in k.items() if kk != "sidecar")
        print(
            "%-24s %-30s n=%-5d sidecar=%-5d preset=%-5d | %s | %s %s %s %s %s | +%d/%d dec=%s b=%s | %s %s | top=%s"
            % (
                v["level"][:24],
                v["key"],
                v["n"],
                k.get("sidecar", 0),
                presets,
                ",".join(v["forms"]),
                f(v["min"]),
                f(v["p5"]),
                f(v["p50"]),
                f(v["p95"]),
                f(v["max"]),
                v["plus"],
                v["pos_noplus"],
                v["decimals"] or "",
                v["bools"] or "",
                v["containers"] or "",
                v["item_fmt"] or "",
                [t[0][:36] for t in v["top"][:6]],
            )
        )
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Inventory crs: keys in XMP presets, profiles and sidecars.")
    sub = ap.add_subparsers(dest="cmd", required=True)

    inv = sub.add_parser("inventory", help="build inventory.json")
    inv.add_argument(
        "--lrc-presets",
        default=os.environ.get("LRG_LRC_PRESETS_DIR"),
        help="LrC's bundled Settings folder (default: $LRG_LRC_PRESETS_DIR)",
    )
    inv.add_argument(
        "--acr-presets",
        default=os.environ.get("LRG_ACR_PRESETS_DIR"),
        help="Camera Raw's Settings folder (default: $LRG_ACR_PRESETS_DIR)",
    )
    inv.add_argument("--user-presets", help="a folder of your own imported presets/profiles")
    inv.add_argument("--sidecar-list", help="JSON list of sidecar paths (sample.py or scan_corpus.py output)")
    inv.add_argument("--sidecars-dir", help="scan every sidecar below this folder instead of a list")
    inv.add_argument("--out", required=True, help="output file outside the repository, e.g. $OUT/inventory.json")
    inv.add_argument("--strict", action="store_true", help="exit non-zero if any file failed to parse")
    inv.set_defaults(func=cmd_inventory)

    show = sub.add_parser("show", help="print an inventory as one line per key")
    show.add_argument("inventory")
    show.add_argument("--levels", help="comma-separated levels, e.g. global,correction,mask-tool")
    show.set_defaults(func=cmd_show)

    args = ap.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
