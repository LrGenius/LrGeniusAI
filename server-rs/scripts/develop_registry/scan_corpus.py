"""Pass 1 over a sidecar corpus: a cheap byte-level scan of every Lightroom sidecar.

For each ``*.xmp`` below the corpus root that is a Lightroom sidecar (contains
``crs:ProcessVersion``; ``* [conflicted].xmp`` copies are skipped) it records the
strata ``sample.py`` needs - top-level folder, writer version, process version,
element vs attribute form, size - and which notable features are present (masks,
retouch, lens blur, point colour, applied-preset record, snapshots, crop, filter
list, legacy local corrections).

It also counts, per ``crs:`` name, how many files mention it anywhere. That is
presence only (a regex over the bytes, nesting ignored); ``parse_xmp.py`` does
the structural inventory.

Outputs (in ``--out-dir``):

* ``corpus_meta.json``      - one object per sidecar (``p`` is its absolute path)
* ``corpus_keyfreq.json``   - ``{crs name: number of files}``

These contain paths from your own disk. **Never commit them.**

Example::

    uv run scan_corpus.py --corpus "$LRG_XMP_CORPUS_DIR" --out-dir "$OUT"

``$OUT`` is a folder outside the repository; the tool refuses to write inside it.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from collections import Counter
from pathlib import Path

from outpath import require_outside_repo
from parse_xmp import is_conflicted, is_lrc_sidecar, iter_xmp_files

KEY_RE = re.compile(rb"crs:([A-Za-z_][A-Za-z0-9_]*)")
VERSION_RE = re.compile(rb'crs:Version(?:="|>)([0-9.]+)')
PV_RE = re.compile(rb'crs:ProcessVersion(?:="|>)([0-9.]+)')


def describe(path: Path, root: Path, data: bytes) -> dict:
    """Strata and feature flags for one sidecar."""
    rel = path.relative_to(root)
    m = VERSION_RE.search(data)
    pv = PV_RE.search(data)
    return {
        "p": str(path),
        # First folder below the corpus root (for a year-per-folder layout this is the year).
        "group": rel.parts[0] if len(rel.parts) > 1 else ".",
        "ver": m.group(1).decode() if m else "?",
        "pv": pv.group(1).decode() if pv else "?",
        "masks": b"MaskGroupBasedCorrections" in data,
        "retouch": b"RetouchAreas" in data,
        "lensblur": b"crs:LensBlur" in data,
        "pointcolors": b"crs:PointColors" in data,
        "preset": b"<crs:Preset" in data,
        "saved": b"crss:SavedSettings" in data,
        "elem": b"<crs:ProcessVersion>" in data,
        "size": len(data),
        "hascrop": b'crs:HasCrop="True"' in data or b"<crs:HasCrop>True" in data,
        "filterlist": b"crs:FilterList" in data,
        "legacy": (
            b"PaintBasedCorrections" in data
            or b"GradientBasedCorrections>" in data
            or b"CircularGradientBasedCorrections" in data
        ),
    }


def scan(root: Path) -> tuple[list[dict], dict[str, int], Counter[str]]:
    meta: list[dict] = []
    keyfreq: Counter[str] = Counter()
    skipped: Counter[str] = Counter()
    for p in iter_xmp_files(root):
        if is_conflicted(p):
            skipped["conflicted"] += 1
            continue
        try:
            data = p.read_bytes()
        except OSError:
            skipped["unreadable"] += 1
            continue
        if not is_lrc_sidecar(data):
            skipped["no-ProcessVersion"] += 1
            continue
        meta.append(describe(p, root, data))
        for k in set(KEY_RE.findall(data)):
            keyfreq[k.decode()] += 1
    return meta, dict(sorted(keyfreq.items())), skipped


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Scan a sidecar corpus (presence + strata).")
    ap.add_argument(
        "--corpus",
        default=os.environ.get("LRG_XMP_CORPUS_DIR"),
        help="root folder of the sidecars (default: $LRG_XMP_CORPUS_DIR)",
    )
    ap.add_argument("--out-dir", required=True, help="where to write corpus_meta.json and corpus_keyfreq.json")
    args = ap.parse_args(argv)
    if not args.corpus or not Path(args.corpus).is_dir():
        print("give --corpus DIR or set LRG_XMP_CORPUS_DIR", file=sys.stderr)
        return 2
    require_outside_repo(args.out_dir)
    root = Path(args.corpus).resolve()
    meta, keyfreq, skipped = scan(root)
    out = Path(args.out_dir)
    out.mkdir(parents=True, exist_ok=True)
    (out / "corpus_meta.json").write_text(json.dumps(meta), encoding="utf-8")
    (out / "corpus_keyfreq.json").write_text(json.dumps(keyfreq, indent=0), encoding="utf-8")

    print(f"{len(meta)} LrC sidecars; {len(keyfreq)} distinct crs names; skipped {dict(skipped)}")
    groups = Counter(m["group"] for m in meta)
    print(f"{len(groups)} groups; largest", dict(groups.most_common(10)))
    print("writer major", dict(Counter(m["ver"].split(".")[0] for m in meta)))
    print("process version", dict(Counter(m["pv"] for m in meta)))
    flags = ("masks", "retouch", "lensblur", "pointcolors", "preset", "saved", "elem", "legacy", "filterlist")
    print(", ".join(f"{f} {sum(m[f] for m in meta)}" for f in flags))
    return 0


if __name__ == "__main__":
    sys.exit(main())
