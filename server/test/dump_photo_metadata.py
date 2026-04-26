import json
import os
import sys


# Ensure server src directory (with services.chroma, config, etc.) is importable.
_THIS_DIR = os.path.dirname(__file__)
_SERVER_ROOT = os.path.dirname(_THIS_DIR)
_SRC_DIR = os.path.join(_SERVER_ROOT, "src")
if _SRC_DIR not in sys.path:
    sys.path.insert(0, _SRC_DIR)


def _extract_photo_id(argv: list[str]) -> tuple[str | None, list[str]]:
    """
    Extract --photo-id from argv and return (photo_id, cleaned_argv).

    We must remove our own CLI flag before importing modules that use
    argparse globally (e.g. config.py), otherwise their ArgumentParser
    will see an unknown option and exit with an error.
    """
    photo_id = None
    cleaned = [argv[0]] if argv else []

    i = 1
    n = len(argv)
    while i < n:
        arg = argv[i]
        if arg == "--photo-id" and i + 1 < n:
            photo_id = argv[i + 1]
            i += 2
            continue
        cleaned.append(arg)
        i += 1

    return photo_id, cleaned


# Extract our own CLI flag before importing services.chroma/config (which
# parse global arguments on import).
_PHOTO_ID, _NEW_ARGV = _extract_photo_id(sys.argv)
sys.argv = _NEW_ARGV

from services.chroma import get_all_image_ids, get_image  # noqa: E402  (import after argv tweak)


def main() -> int:
    """
    Dump stored data from ChromaDB.

    Usage:
        # Dump a single photo
        python server/test/dump_photo_metadata.py --db-path /path/to/db --photo-id YOUR_PHOTO_ID

        # Dump the whole DB (all photos)
        python server/test/dump_photo_metadata.py --db-path /path/to/db

    Options:
        --db-path  Passed through to the main server config (required).
        --debug    Optional; enables debug logging in config.
        --photo-id Optional; if omitted, all photo IDs are dumped.
    """
    if _PHOTO_ID:
        # Single-photo mode
        record = get_image(_PHOTO_ID)
        if not record or not record.get("ids"):
            print(f"No record found for photo_id={_PHOTO_ID}")
            return 2
        print(json.dumps(record, indent=2, default=str))
        return 0

    # Whole-DB mode: iterate over all IDs and dump their records.
    ids = get_all_image_ids()
    print(f"# Dumping {len(ids)} image record(s) from ChromaDB\n", file=sys.stderr)
    all_records = {}
    for pid in ids:
        rec = get_image(pid)
        all_records[pid] = rec
    print(json.dumps(all_records, indent=2, default=str))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
