#!/usr/bin/env python3
"""Verify the Lightroom GUI translation files are in sync — without changing them.

`sync_translations.py` *rewrites* the TranslatedStrings_*.txt files from the
LOC() keys found in the Lua source. This script is its read-only counterpart: it
checks the same invariant and reports drift, so it can run as an edit-time hook
and in CI without mutating anything.

Source of truth: every `LOC("$$$/LrGeniusAI/...")` key used in the Lua plugin
must be present in all three translation files (en, de, fr). Keys present in a
translation file but no longer used in the Lua source are reported as orphans
(warning only — they don't fail the check).

Exit code 0 = in sync, 1 = drift detected (missing keys somewhere).
"""

import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
sys.path.insert(0, REPO)

# Reuse the parsing logic that already handles the UTF-16 encoding quirk.
from sync_translations import extract_loc_keys, load_translated_strings  # noqa: E402

PLUGIN_DIR = os.path.join(REPO, "plugin", "LrGeniusAI.lrdevplugin")
LANGS = ("en", "de", "fr")
MAX_LIST = 50


def _path(lang: str) -> str:
    return os.path.join(PLUGIN_DIR, f"TranslatedStrings_{lang}.txt")


def main() -> int:
    lua_keys = set(extract_loc_keys(PLUGIN_DIR))
    if not lua_keys:
        print("No LOC() keys found in Lua source — nothing to check.")
        return 0

    had_error = False
    for lang in LANGS:
        present = set(load_translated_strings(_path(lang)))
        missing = sorted(lua_keys - present)
        orphan = sorted(present - lua_keys)

        if missing:
            had_error = True
            print(
                f"[{lang}] {len(missing)} key(s) used in Lua but MISSING from "
                f"TranslatedStrings_{lang}.txt:"
            )
            for key in missing[:MAX_LIST]:
                print(f"    {key}")
            if len(missing) > MAX_LIST:
                print(f"    ... and {len(missing) - MAX_LIST} more")

        if orphan:
            print(
                f"[{lang}] {len(orphan)} orphan key(s) in TranslatedStrings_{lang}.txt "
                f"no longer used in Lua (warning):"
            )
            for key in orphan[:MAX_LIST]:
                print(f"    {key}")
            if len(orphan) > MAX_LIST:
                print(f"    ... and {len(orphan) - MAX_LIST} more")

    if had_error:
        print(
            "\nTranslations are out of sync. Run `python sync_translations.py` to "
            "regenerate all three files from the Lua LOC() keys, then translate the "
            "new German/French entries."
        )
        return 1

    print(f"Translations in sync across {'/'.join(LANGS)} ({len(lua_keys)} keys).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
