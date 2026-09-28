"""Count how nested ``crs:`` structs are spelled in XMP.

The same struct can be serialized three ways, and a parser has to accept all of
them:

* ``parseType=Resource``   - ``<crs:X rdf:parseType="Resource"><crs:A>..</crs:A></crs:X>``
* ``Description-child``    - ``<crs:X><rdf:Description crs:A=".."/></crs:X>``
* ``attrs-on-element``     - ``<crs:X crs:A=".."/>`` (only when no field is itself complex)

For every struct-valued property (``Key``) and every struct item in a Seq
(``Key[]``) this prints the form and how often it occurs, and whether the struct
has complex children (``complex=True``). Alt containers (localised strings) are
not structs and are skipped.

Inputs are any mix of folders (scanned recursively) and ``--sidecar-list`` files.
Sidecar rules apply to the list: ``[conflicted]`` copies and files without
``crs:ProcessVersion`` are skipped.

Example::

    uv run structform.py --lrc-presets "$LRG_LRC_PRESETS_DIR" --sidecar-list "$OUT/sample_files.json"

``$OUT`` is a folder outside the repository; ``--out`` refuses a path inside it.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import xml.etree.ElementTree as ET
from collections import Counter
from pathlib import Path

from outpath import require_outside_repo
from parse_xmp import CRS, RDF, is_conflicted, is_lrc_sidecar, iter_xmp_files, load_sidecar_list

CRS_T = "{%s}" % CRS
RDF_T = "{%s}" % RDF


def _complex_children(node: ET.Element) -> list[ET.Element]:
    return [c for c in node if c.tag.startswith(CRS_T)]


def form_of(el: ET.Element) -> str:
    """Serialization form of a struct held by a property element or ``rdf:li``."""
    if el.attrib.get(RDF_T + "parseType") == "Resource":
        return "parseType=Resource"
    d = el.find(RDF_T + "Description")
    if d is not None:
        return "Description-child(complex=%s)" % bool(_complex_children(d))
    if any(k.startswith(CRS_T) for k in el.attrib):
        return "attrs-on-element(complex=%s)" % bool(_complex_children(el))
    return "other"


def count_forms(files: list[Path | str], counter: Counter, errors: list[str] | None = None) -> int:
    """Count struct forms into ``counter``; returns files parsed, appends failures to ``errors``."""
    parsed = 0
    for p in files:
        try:
            root = ET.parse(p).getroot()
        except (ET.ParseError, OSError) as e:
            if errors is not None:
                errors.append(f"{p}: {e}")
            continue
        parsed += 1
        for el in root.iter():
            if not el.tag.startswith(CRS_T):
                continue
            key = el.tag[len(CRS_T) :]
            seq = el.find(RDF_T + "Seq")
            if seq is not None:
                for li in seq.findall(RDF_T + "li"):
                    if len(li) or any(k.startswith(CRS_T) for k in li.attrib):
                        counter[(key + "[]", form_of(li))] += 1
            elif (len(el) or any(k.startswith(CRS_T) for k in el.attrib)) and el.find(RDF_T + "Alt") is None:
                counter[(key, form_of(el))] += 1
    return parsed


def sidecars_from_list(path: str) -> list[str]:
    out = []
    for p in load_sidecar_list(path):
        if is_conflicted(p):
            continue
        try:
            if is_lrc_sidecar(Path(p).read_bytes()):
                out.append(p)
        except OSError:
            continue
    return out


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Count struct serialization forms in XMP files.")
    ap.add_argument(
        "--lrc-presets",
        default=os.environ.get("LRG_LRC_PRESETS_DIR"),
        help="LrC's bundled Settings folder (default: $LRG_LRC_PRESETS_DIR)",
    )
    ap.add_argument("--dir", action="append", default=[], help="further folder to scan (repeatable)")
    ap.add_argument("--sidecar-list", help="JSON list of sidecar paths (sample.py output)")
    ap.add_argument("--out", help="also write the counts as JSON to this file")
    args = ap.parse_args(argv)
    if args.out:
        require_outside_repo(args.out)

    files: list[Path | str] = []
    for d in ([args.lrc_presets] if args.lrc_presets else []) + args.dir:
        files.extend(iter_xmp_files(d))
    if args.sidecar_list:
        files.extend(sidecars_from_list(args.sidecar_list))
    if not files:
        print("no input: give --lrc-presets, --dir or --sidecar-list", file=sys.stderr)
        return 2

    counter: Counter = Counter()
    errors: list[str] = []
    parsed = count_forms(files, counter, errors)
    for (k, f), n in sorted(counter.items()):
        print("%-32s %-40s %d" % (k, f, n))
    print(f"-- {parsed} files parsed, {len(errors)} could not be parsed")
    for e in errors[:20]:
        print("   parse error: " + e, file=sys.stderr)
    if len(errors) > 20:
        print(f"   ... and {len(errors) - 20} more", file=sys.stderr)
    if args.out:
        rows = [{"key": k, "form": f, "n": n} for (k, f), n in sorted(counter.items())]
        Path(args.out).write_text(json.dumps(rows, indent=1), encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
