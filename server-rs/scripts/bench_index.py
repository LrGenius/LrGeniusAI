#!/usr/bin/env python3
"""Controlled benchmark for the /index_by_reference pipeline.

Why this exists
---------------
Timing a live indexing run from the server log is not interpretable.
Per-photo latency is dominated by the source file's size (decode cost),
so any two windows of a real run differ for reasons that have nothing to
do with whatever is being tested. A 6x "improvement" was once read off
two such windows; it was 14 MB photos vs 31 MB photos.

So this harness:

  * holds file size constant by bucketing photos and sampling per bucket,
  * reports every result per size bucket with an n, so size can never
    silently explain a difference,
  * issues a unique photo_id per iteration, because process_batch's
    delta-check (`need_embedding`) skips already-indexed photos and would
    otherwise measure no-ops,
  * runs its own server on its own port and db-path, so it never touches
    a live install.

Standard library only; no dependency on the uv project in this directory.

Examples
--------
Is the external volume the problem?  (the store moves, the photos don't)

    ./bench_index.py --photos /Volumes/EXTERN/Lightroom/Photos \\
        --db-path /Volumes/EXTERN/bench.db \\
        --compare-db-path ~/bench.db

Does a long-running process degrade?

    ./bench_index.py --photos <dir> --warmup 200

Is the released binary slower than a local build?

    ./bench_index.py --photos <dir> \\
        --binary ../target/release/geniusai-server \\
        --compare-binary /Applications/LrGeniusAI/Server/lrgenius-server
"""

from __future__ import annotations

import argparse
import json
import os
import random
import shutil
import statistics
import subprocess
import sys
import time
import urllib.error
import urllib.request
import uuid
from dataclasses import dataclass, field
from pathlib import Path

RAW_EXTS = {
    ".jpg", ".jpeg", ".png", ".heic", ".heif", ".tif", ".tiff",
    ".dng", ".cr2", ".cr3", ".nef", ".arw", ".orf", ".raf", ".rw2",
}

# Size buckets in MB. Latency tracks decode cost, which tracks file size,
# so comparisons are only meaningful within a bucket.
BUCKETS_MB = [(0, 8), (8, 16), (16, 24), (24, 32), (32, 48), (48, 1 << 20)]


def bucket_of(size_bytes: int) -> str:
    mb = size_bytes / 1e6
    for lo, hi in BUCKETS_MB:
        if lo <= mb < hi:
            return f"{lo}-{hi}MB" if hi < (1 << 20) else f"{lo}MB+"
    return "?"


@dataclass
class Sample:
    path: str
    size: int
    seconds: float
    ok: bool


@dataclass
class Result:
    label: str
    samples: list[Sample] = field(default_factory=list)

    def by_bucket(self) -> dict[str, list[float]]:
        return self._group(lambda s: bucket_of(s.size))

    def by_format(self) -> dict[str, list[float]]:
        return self._group(lambda s: Path(s.path).suffix.lower())

    def by_format_and_bucket(self) -> dict[str, list[float]]:
        return self._group(lambda s: f"{Path(s.path).suffix.lower()} {bucket_of(s.size)}")

    def _group(self, key) -> dict[str, list[float]]:
        out: dict[str, list[float]] = {}
        for s in self.samples:
            if s.ok:
                out.setdefault(key(s), []).append(s.seconds)
        return out


def discover_photos(roots: list[Path], per_bucket: int, seed: int) -> list[Path]:
    """Collect photos and sample `per_bucket` from each size bucket."""
    buckets: dict[str, list[Path]] = {}
    for root in roots:
        if root.is_file():
            files = [root]
        else:
            files = (p for p in root.rglob("*") if p.suffix.lower() in RAW_EXTS)
        for p in files:
            try:
                size = p.stat().st_size
            except OSError:
                continue
            if size < 1024:
                continue
            buckets.setdefault(bucket_of(size), []).append(p)

    rng = random.Random(seed)
    chosen: list[Path] = []
    for name in sorted(buckets):
        pool = buckets[name]
        rng.shuffle(pool)
        take = pool[:per_bucket]
        chosen.extend(take)
        print(f"  bucket {name:>10}: {len(pool):5d} available, using {len(take)}")
    rng.shuffle(chosen)
    return chosen


class Server:
    """An isolated server instance: own port, own db-path, own process."""

    def __init__(self, binary: Path, db_path: Path, port: int, debug: bool = False,
                 extra_env: dict[str, str] | None = None):
        self.binary = binary
        self.db_path = db_path
        self.port = port
        self.debug = debug
        self.extra_env = extra_env or {}
        self.proc: subprocess.Popen | None = None

    @property
    def base(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    def __enter__(self) -> "Server":
        env = dict(os.environ, GENIUSAI_PORT=str(self.port), **self.extra_env)
        cmd = [str(self.binary), "--db-path", str(self.db_path)]
        if self.debug:
            cmd.append("--debug")
        self.proc = subprocess.Popen(
            cmd, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
        )
        deadline = time.time() + 120
        while time.time() < deadline:
            if self.proc.poll() is not None:
                raise RuntimeError(f"server exited early (rc={self.proc.returncode})")
            try:
                with urllib.request.urlopen(f"{self.base}/health", timeout=2) as r:
                    json.loads(r.read())
                    return self
            except Exception:
                time.sleep(0.5)
        raise RuntimeError("server did not become healthy within 120s")

    def __exit__(self, *exc) -> None:
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=30)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait(timeout=10)

    def index(self, path: Path, photo_id: str, timeout: float) -> tuple[float, bool]:
        body = {
            "images": [{"path": str(path), "photo_id": photo_id}],
            # --db-path on the CLI only sets the log location; the store is
            # bound by the auto_bind_db_path middleware from this field.
            "db_path": str(self.db_path),
            "catalog_id": "bench",
            # Exercise decode + SigLIP + faces. No LLM tasks, so no
            # provider or API key is needed and nothing leaves the machine.
            "tasks": ["embeddings", "faces"],
            "regenerate_metadata": "false",
            "generate_keywords": "false",
            "generate_caption": "false",
            "generate_title": "false",
            "generate_alt_text": "false",
        }
        req = urllib.request.Request(
            f"{self.base}/index_by_reference",
            data=json.dumps(body).encode(),
            headers={"Content-Type": "application/json"},
        )
        start = time.perf_counter()
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                payload = json.loads(r.read())
            elapsed = time.perf_counter() - start
            ok = not payload.get("error") and payload.get("failures", 0) == 0
            return elapsed, ok
        except (urllib.error.URLError, TimeoutError, json.JSONDecodeError):
            return time.perf_counter() - start, False


def run(server: Server, photos: list[Path], warmup: int, timeout: float, label: str) -> Result:
    res = Result(label=label)
    total = len(photos)
    for i, p in enumerate(photos):
        # Unique id per iteration, else the delta-check skips the work.
        pid = f"bench:{uuid.uuid4()}"
        secs, ok = server.index(p, pid, timeout)
        phase = "warmup" if i < warmup else "measure"
        if phase == "measure":
            try:
                size = p.stat().st_size
            except OSError:
                continue
            res.samples.append(Sample(str(p), size, secs, ok))
        done = i + 1
        if done % 10 == 0 or done == total:
            print(f"\r  [{label}] {done}/{total} ({phase})", end="", flush=True)
    print()
    return res


def run_interleaved(
    sa: Server, sb: Server, photos: list[Path], warmup: int, timeout: float,
    labels: tuple[str, str], seed: int,
) -> tuple[Result, Result]:
    """Alternate A and B on each photo, with the order coin-flipped per photo.

    Running A fully and then B is not sound here: B re-reads the same files
    A just pulled into the page cache, and any thermal or background drift
    over the run lands entirely on one side. Interleaving splits both
    effects evenly, and the per-photo coin flip removes the residual
    first/second advantage within each pair.
    """
    ra, rb = Result(label=labels[0]), Result(label=labels[1])
    rng = random.Random(seed)
    total = len(photos)
    for i, p in enumerate(photos):
        pair = [(sa, ra), (sb, rb)]
        if rng.random() < 0.5:
            pair.reverse()
        for server, res in pair:
            secs, ok = server.index(p, f"bench:{uuid.uuid4()}", timeout)
            if i >= warmup:
                try:
                    size = p.stat().st_size
                except OSError:
                    continue
                res.samples.append(Sample(str(p), size, secs, ok))
        done = i + 1
        if done % 10 == 0 or done == total:
            phase = "warmup" if i < warmup else "measure"
            print(f"\r  [A|B interleaved] {done}/{total} ({phase})", end="", flush=True)
    print()
    return ra, rb


def summarize(res: Result) -> None:
    ok = [s for s in res.samples if s.ok]
    bad = len(res.samples) - len(ok)
    print(f"\n=== {res.label} ===  n={len(ok)} measured" + (f", {bad} FAILED" if bad else ""))
    # Both groupings are printed because it is not obvious which one drives
    # latency. File size is only a proxy for decode cost; container format
    # (JPEG vs RAW) can matter far more. Reading only one table is how you
    # end up attributing a difference to the wrong variable.
    for title, groups in (
        ("by size", res.by_bucket()),
        ("by format", res.by_format()),
        ("by format x size", res.by_format_and_bucket()),
    ):
        print(f"\n  -- {title} --")
        print(f"  {'group':>18}  {'n':>4}  {'median':>8}  {'p95':>8}  {'mean':>8}")
        for name, vals in sorted(groups.items()):
            v = sorted(vals)
            p95 = v[min(len(v) - 1, int(len(v) * 0.95))]
            flag = "" if len(v) >= 5 else "   (n too small)"
            print(
                f"  {name:>18}  {len(v):4d}  {statistics.median(v):7.3f}s "
                f"{p95:7.3f}s {statistics.fmean(v):7.3f}s{flag}"
            )


def compare(a: Result, b: Result) -> None:
    """Paired per-bucket comparison. Never compare overall medians: the
    bucket mix differs between runs and will invent differences."""
    print(f"\n=== {a.label}  vs  {b.label} ===")
    print(f"  {'group':>18}  {'n(a)':>5} {'n(b)':>5}  {'median a':>9} {'median b':>9}  {'ratio':>7}")
    # Grouped by format AND size: the strictest pairing available, so a
    # difference in the photo mix cannot masquerade as a real effect.
    ba, bb = a.by_format_and_bucket(), b.by_format_and_bucket()
    for name in sorted(set(ba) | set(bb)):
        va, vb = ba.get(name, []), bb.get(name, [])
        if len(va) < 5 or len(vb) < 5:
            note = "insufficient n" if (va and vb) else "missing in one run"
            print(f"  {name:>18}  {len(va):5d} {len(vb):5d}  {'':>9} {'':>9}  {note}")
            continue
        ma, mb = statistics.median(va), statistics.median(vb)
        print(f"  {name:>18}  {len(va):5d} {len(vb):5d}  {ma:8.3f}s {mb:8.3f}s  {ma / mb:6.2f}x")
    print("\n  Ratios are only meaningful within a bucket and with adequate n.")


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--photos", nargs="+", required=True, type=Path,
                    help="Directories (or files) to sample photos from")
    ap.add_argument("--per-bucket", type=int, default=25,
                    help="Photos to sample per size bucket (default 25)")
    ap.add_argument("--binary", type=Path,
                    default=Path(__file__).resolve().parents[1] / "target/release/geniusai-server")
    ap.add_argument("--db-path", type=Path, required=True,
                    help="Store location for run A (created if absent)")
    ap.add_argument("--compare-binary", type=Path, help="Second binary for an A/B run")
    ap.add_argument("--compare-db-path", type=Path, help="Second store location for an A/B run")
    ap.add_argument("--warmup", type=int, default=10,
                    help="Photos indexed before measuring starts (default 10). "
                         "Raise it to test long-running-process degradation.")
    ap.add_argument("--env", action="append", default=[], metavar="K=V",
                    help="Extra env var for run A (repeatable), e.g. --env LRG_ML_EP=coreml")
    ap.add_argument("--compare-env", action="append", default=[], metavar="K=V",
                    help="Extra env var for run B (repeatable)")
    ap.add_argument("--port", type=int, default=19899)
    ap.add_argument("--timeout", type=float, default=300.0)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--keep-db", action="store_true", help="Do not delete the benchmark stores")
    args = ap.parse_args()

    if not args.binary.is_file():
        print(f"error: binary not found: {args.binary}", file=sys.stderr)
        return 1

    print("Sampling photos:")
    photos = discover_photos(args.photos, args.per_bucket, args.seed)
    if not photos:
        print("error: no photos found", file=sys.stderr)
        return 1
    print(f"  total {len(photos)} photos ({args.warmup} warmup + "
          f"{max(0, len(photos) - args.warmup)} measured)\n")

    def parse_env(pairs: list[str]) -> dict[str, str]:
        out = {}
        for kv in pairs:
            if "=" not in kv:
                raise SystemExit(f"error: --env expects K=V, got {kv!r}")
            k, v = kv.split("=", 1)
            out[k] = v
        return out

    env_a, env_b = parse_env(args.env), parse_env(args.compare_env)

    def tag(binary: Path, db: Path, env: dict[str, str]) -> str:
        suffix = (" " + " ".join(f"{k}={v}" for k, v in sorted(env.items()))) if env else ""
        return f"{binary.name} @ {db}{suffix}"

    ab = bool(args.compare_binary or args.compare_db_path or args.compare_env)
    stores = []
    try:
        if not ab:
            with Server(args.binary, args.db_path, args.port, extra_env=env_a) as s:
                stores.append(args.db_path)
                res_a = run(s, photos, args.warmup, args.timeout, "A")
                res_a.label = tag(args.binary, args.db_path, env_a)
            summarize(res_a)
            return 0

        bin_b = args.compare_binary or args.binary
        # The two arms must never share a store: run B would then see run A's
        # rows and the delta-check would skip the work instead of measuring it.
        db_b = args.compare_db_path or args.db_path.with_name(args.db_path.name + ".b")
        if db_b == args.db_path:
            print("\nerror: --compare-db-path must differ from --db-path, else run B "
                  "reuses run A's store and the delta-check skips the work.",
                  file=sys.stderr)
            return 1
        # Both servers run concurrently so the two arms can be interleaved.
        with Server(args.binary, args.db_path, args.port, extra_env=env_a) as sa, \
                Server(bin_b, db_b, args.port + 1, extra_env=env_b) as sb:
            stores.extend([args.db_path, db_b])
            res_a, res_b = run_interleaved(
                sa, sb, photos, args.warmup, args.timeout,
                (tag(args.binary, args.db_path, env_a), tag(bin_b, db_b, env_b)),
                args.seed,
            )
        summarize(res_a)
        summarize(res_b)
        compare(res_a, res_b)
    finally:
        if not args.keep_db:
            for d in stores:
                shutil.rmtree(d, ignore_errors=True)

    return 0


if __name__ == "__main__":
    sys.exit(main())
