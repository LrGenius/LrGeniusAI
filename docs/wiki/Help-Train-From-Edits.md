# Help: Save Edits as AI Training Examples

The **Save Edits as AI Training Examples** workflow lets you teach LrGeniusAI your personal editing style. Your current Lightroom develop settings for selected photos are saved to the backend as few-shot examples. The next time you run **AI Edit Photos**, these examples are injected into the AI prompt as context, so the AI produces results closer to your style.

## How to use it

`Library → Plug-in Extras → Save Edits as AI Training Examples...`

1. Edit a set of photos in Lightroom's Develop module the way you like them.
2. Select those photos in the Library grid.
3. Open *Save Edits as AI Training Examples* from the menu.
4. In the dialog, choose the scope and optionally set a **style label** (e.g.
   "Wedding summer 2025") and a **description** of the style. Both are
   optional.
5. Confirm.

The plugin reads the current develop settings from Lightroom and sends them to the backend, where they are stored as labeled training examples.

---

## Dialog options

### Scope

- **Selected photos only** — reads develop settings from your current selection.
- **Current view** — uses all photos in the currently visible folder or collection.
- **Entire catalog** — reads develop settings from all photos in the catalog.

### Style label (optional)

A short name for this set of training examples (e.g. `wedding_airy`, `street_bw`, `landscape_dramatic`). Use the same label when saving multiple sessions of a consistent style.

### Description (optional)

A free-text summary of what this edit style represents. Helps the AI understand when to apply it.

---

## How it affects AI Edit

Training examples feed two different mechanisms:

**As few-shot context for the LLM.** When you run **AI Edit Photos** with a
cloud or local model, the backend retrieves the stored examples most similar to
the photo being edited and puts their actual develop values into the prompt, so
the model has your numbers to anchor on rather than only the style preset.

**As the Style Engine.** The backend can also produce an edit with no LLM at
all: it finds the closest training examples by image similarity, re-scores them
on exposure, scene type and time of day, and interpolates their develop
settings. This needs at least **five** training examples before it will return
anything, and it gets noticeably better with more.

Both paths match examples by image similarity, so a training photo has to have
been indexed with **Enable smart photo search** for it to be findable. Saving an
example for a photo without an embedding still works, but that example will
never be retrieved — the plugin shows a warning when this happens.

The same applies to the photo being edited: if it is not indexed, there is no
embedding to compare your saved edits against. AI Edit then falls back to the
plain style preset and says so in the result, rather than silently producing a
generic edit.

---

## Tips

- Save at least 5 well-edited photos per style — that is the Style Engine's minimum — and 10–20 for good results. More diverse examples generalize better.
- Use consistent labels per shooting genre: one label for weddings, one for portraits, one for landscapes — rather than mixing everything.
- Re-run the workflow after editing sessions to keep examples fresh and representative.
- Training examples do not replace the **Overall look** preset — they augment it. Combining a matching preset with few-shot examples gives the best results.
