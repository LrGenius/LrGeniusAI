"""Stratified, reproducible sample of a sidecar corpus (input: ``scan_corpus.py`` output).

The full corpus is large and dominated by a few folders and one process version.
The structural inventory (``parse_xmp.py inventory``) runs on a sample instead,
built in two steps:

1. **Guaranteed rare shapes** first: element-form files, files written by an
   older Camera Raw major version than the newest one seen, snapshots
   (``crss:SavedSettings``), retouch, filter lists (Denoise and friends), crops.
2. **Proportional fill** by ``(group, process version, has masks)`` with a
   minimum per stratum, so small strata are still represented.

Selection is random with a fixed seed over a sorted list, so the same corpus
always yields the same sample.

Output: ``sample_files.json`` in ``--out-dir``, a sorted list of paths. It holds
paths from your own disk. **Never commit it.**

Example::

    uv run sample.py --meta "$OUT/corpus_meta.json" --size 3000 --out-dir "$OUT"

``$OUT`` is a folder outside the repository; the tool refuses to write inside it.
"""

from __future__ import annotations

import argparse
import json
import random
import sys
from collections import Counter, defaultdict
from pathlib import Path

from outpath import require_outside_repo


def major(version: str) -> int:
    try:
        return int(version.split(".")[0])
    except ValueError:
        return -1


def stratified_sample(meta: list[dict], size: int, seed: int, min_per_stratum: int) -> list[str]:
    rng = random.Random(seed)
    meta = sorted(meta, key=lambda m: m["p"])
    chosen: dict[str, dict] = {}

    def take(pool: list[dict], n: int) -> None:
        pool = [m for m in pool if m["p"] not in chosen]
        for m in rng.sample(pool, min(n, len(pool))):
            chosen[m["p"]] = m

    newest = max((major(m["ver"]) for m in meta), default=-1)
    take([m for m in meta if m["elem"]], 10)
    take([m for m in meta if major(m["ver"]) != newest], 200)
    take([m for m in meta if m["saved"]], 30)
    take([m for m in meta if m["retouch"]], 80)
    take([m for m in meta if m["filterlist"]], 80)
    take([m for m in meta if m["hascrop"]], 100)

    strata: defaultdict[tuple, list[dict]] = defaultdict(list)
    for m in meta:
        strata[(m["group"], m["pv"], m["masks"])].append(m)
    budget = max(0, size - len(chosen))
    total = max(1, len(meta))
    for _key, pool in sorted(strata.items()):
        take(pool, max(min_per_stratum, round(budget * len(pool) / total)))
    return sorted(chosen)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Stratified sidecar sample.")
    ap.add_argument("--meta", required=True, help="corpus_meta.json from scan_corpus.py")
    ap.add_argument("--size", type=int, default=3000, help="target sample size (default 3000)")
    ap.add_argument("--seed", type=int, default=42, help="random seed (default 42)")
    ap.add_argument("--min-per-stratum", type=int, default=15, help="minimum files per stratum (default 15)")
    ap.add_argument("--out-dir", required=True)
    args = ap.parse_args(argv)
    require_outside_repo(args.out_dir)

    meta = json.loads(Path(args.meta).read_text(encoding="utf-8"))
    files = stratified_sample(meta, args.size, args.seed, args.min_per_stratum)
    by_path = {m["p"]: m for m in meta}
    picked = [by_path[p] for p in files]
    out = Path(args.out_dir)
    out.mkdir(parents=True, exist_ok=True)
    (out / "sample_files.json").write_text(json.dumps(files, indent=0), encoding="utf-8")

    print(f"{len(files)} files sampled from {len(meta)}")
    groups = Counter(m["group"] for m in picked)
    print(f"{len(groups)} groups; largest", dict(groups.most_common(10)))
    print("masks", sum(m["masks"] for m in picked), "pv", dict(Counter(m["pv"] for m in picked)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
