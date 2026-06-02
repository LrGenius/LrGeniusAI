---
name: sync-translations
description: Keep the Lightroom GUI translation files (en/de/fr) in sync with the LOC() keys in the Lua plugin. Use whenever adding, changing, or removing a LOC() string, or when translation drift is suspected.
---

# Translation sync

The plugin localizes every GUI string through `LOC("$$$/LrGeniusAI/Module/Key=Default")`.
Those keys must exist in **all three** translation files:

- `plugin/LrGeniusAI.lrdevplugin/TranslatedStrings_en.txt`
- `plugin/LrGeniusAI.lrdevplugin/TranslatedStrings_de.txt`
- `plugin/LrGeniusAI.lrdevplugin/TranslatedStrings_fr.txt`

If a key is missing from a file, Lightroom shows the raw `$$$/...` key (or the
English fallback) instead of translated text — a silent, shippable bug.

## Critical gotcha

These `.txt` files are **UTF-16 encoded**. Do **not** hand-edit them with tools
that assume UTF-8, and never edit just one of the three. Always go through the
scripts below.

## Workflow when you add or change a LOC() string

1. Make the `LOC(...)` change in the `.lua` source as usual.
2. Regenerate all three files from the Lua keys:
   ```bash
   python sync_translations.py
   ```
   This adds any new keys (English default text is filled in; new de/fr entries
   start from the English value and need translating).
3. Translate the freshly-added German and French entries (don't leave English
   placeholders in `_de`/`_fr`).
4. Verify parity:
   ```bash
   python3 scripts/check_translations.py
   ```
   Exit 0 = in sync. It reports keys missing from any file (error) and orphaned
   keys no longer used in Lua (warning).

## Verifying without changing anything

`scripts/check_translations.py` is read-only and is also wired into the
edit-time lint hook (`.claude/hooks/lint-edited-file.py`) — editing any
`TranslatedStrings_*.txt` triggers a parity check automatically.

## Removing strings

When you delete a `LOC()` usage, its key becomes an orphan. Re-run
`sync_translations.py` to prune it from all three files.
