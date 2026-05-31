# Help: Save Edits as AI Training Examples

The **Save Edits as AI Training Examples** workflow lets you teach LrGeniusAI your personal editing style. Your current Lightroom develop settings for selected photos are saved to the backend as few-shot examples. The next time you run **AI Edit Photos**, these examples are injected into the AI prompt as context, so the AI produces results closer to your style.

## How to use it

`Library → Plug-in Extras → Save Edits as AI Training Examples...`

1. Edit a set of photos in Lightroom's Develop module the way you like them.
2. Select those photos in the Library grid.
3. Open *Save Edits as AI Training Examples* from the menu.
4. In the dialog, set a **label** (e.g. "Wedding summer 2025") and an optional **description** of the style.
5. Choose the scope and confirm.

The plugin reads the current develop settings from Lightroom and sends them to the backend, where they are stored as labeled training examples.

---

## Dialog options

### Scope

- **Selected photos only** — reads develop settings from your current selection.
- **Current view** — uses all photos in the currently visible folder or collection.
- **Entire catalog** — reads develop settings from all photos in the catalog.

### Label

A short name for this set of training examples (e.g. `wedding_airy`, `street_bw`, `landscape_dramatic`). Use the same label when saving multiple sessions of a consistent style.

### Description (optional)

A free-text summary of what this edit style represents. Helps the AI understand when to apply it.

---

## How it affects AI Edit

When you run **AI Edit Photos**, the backend looks up stored training examples. If relevant examples exist, they are included as few-shot context in the AI prompt — the AI sees your actual develop values as reference points alongside the style preset.

---

## Tips

- Save 5–20 well-edited photos per style for best results. More diverse examples generally improve generalization.
- Use consistent labels per shooting genre: one label for weddings, one for portraits, one for landscapes — rather than mixing everything.
- Re-run the workflow after editing sessions to keep examples fresh and representative.
- Training examples do not replace the **Overall look** preset — they augment it. Combining a matching preset with few-shot examples gives the best results.
