"""Dump the backend's ``edit_training`` LanceDB table to JSON.

The table lives in the backend's Lance store next to the database: for
``--db-path <dir>/lrgenius.db`` that is ``<dir>/lrgenius-lance/edit_training.lance``.
It holds one row per "Train from edits" example: the photo's full
``getDevelopSettings()`` blob (as the JSON string the plugin's ``JSON.lua``
produced) plus photo metadata. ``extract_fixtures.py`` reads this dump.

Needs the optional ``lance`` dependency group (``pylance``)::

    uv run --group lance dump_training.py <dir>/lrgenius-lance/edit_training.lance "$OUT/training_rows.json"

``$OUT`` is a folder outside the repository; the tool refuses to write inside it
and creates the file readable by you only (mode 0600).

The output contains photo ids, file names, capture times and camera data from
your catalog. **It is private - never commit it.** Only the scrubbed files that
``extract_fixtures.py`` writes may go into the repository.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

from outpath import require_outside_repo


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Dump the edit_training LanceDB table to JSON.")
    ap.add_argument("dataset", help="path to the edit_training.lance dataset directory")
    ap.add_argument("out", help="output JSON file outside the repository (a list of row objects)")
    ap.add_argument("--show-schema", action="store_true", help="print the table schema")
    args = ap.parse_args(argv)
    require_outside_repo(args.out)

    try:
        import lance  # imported here so the rest of the project stays stdlib-only
    except ImportError:
        print("pylance is not installed: run with `uv run --group lance dump_training.py ...`", file=sys.stderr)
        return 2

    if not Path(args.dataset).is_dir():
        print(f"not a dataset directory: {args.dataset}", file=sys.stderr)
        return 2
    ds = lance.dataset(args.dataset)
    if args.show_schema:
        print(ds.schema)
    # The embedding column is large and not needed for fixtures.
    cols = [f.name for f in ds.schema if f.name != "vector"]
    rows = ds.to_table(columns=cols).to_pylist()
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    # Private data: create the file readable by the owner only.
    fd = os.open(out, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    os.fchmod(fd, 0o600)  # an existing file keeps its old mode otherwise
    with os.fdopen(fd, "w", encoding="utf-8") as fh:
        fh.write(json.dumps(rows, default=str))
    print(f"{len(rows)} rows -> {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
