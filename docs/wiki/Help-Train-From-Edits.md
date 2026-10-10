# Help: Save Edits as AI Training Examples

The **Save Edits as AI Training Examples** workflow lets you teach LrGeniusAI your personal editing style. Your current Lightroom develop settings for selected photos are saved to the backend as labeled examples. They are what **AI Edit Photos** builds its recipes from — without them AI Edit has nothing to work with and refuses to run, so this workflow is the prerequisite for that one, not an optional refinement of it.

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

Only **raw and DNG** files are saved. JPEGs, TIFFs and other formats in the chosen scope are skipped, and if nothing is left the workflow says so.

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

Training examples are the entire input to **AI Edit Photos**. The backend finds
the closest examples by image similarity, re-scores them on exposure, scene type
and time of day, and interpolates their develop settings into a recipe for the
photo in front of it. No language model is involved.

That has two consequences worth knowing:

- **Below five examples, AI Edit will not run.** It stops in the plugin with a
  pointer back to this workflow. Five is the floor; the style profile is
  reported as *Building up* below ten and *Active* from fifty, and the match
  quality follows that curve.
- **Variety matters as much as volume.** Matching is per photo, so a profile
  built only from bright studio work has little to offer a night shot. Train on
  the kinds of photos you actually want edited.

Matching runs on image similarity, so a training photo has to have been indexed
with **Enable smart photo search** for it to be findable. Saving an example for
a photo without an embedding still works, but that example will never be
retrieved — the plugin shows a warning when this happens.

The same applies to the photo being edited: if it is not indexed, there is no
embedding to compare your saved edits against. The backend then falls back to
scoring recent examples on exposure, scene and time of day alone, which is a
weaker match — the review dialog reports the lower confidence rather than
hiding it.

## Raw and non-raw examples are kept apart

Only for white balance, and only because Lightroom forces the issue: its
temperature slider is Kelvin on a raw file and a relative −100..+100 offset on a
JPEG. Averaging the two would produce a number that means nothing on either
scale — and it would land on a real photo.

"File type" here means the kind of white balance Lightroom stores for a photo,
read from its develop settings, not the file extension. Since this workflow
saves raw and DNG files only, nearly every example is the raw kind. The
exception is a DNG converted from a JPEG: Lightroom keeps the JPEG kind of white
balance for it, and so does the backend.

When the backend carries a white balance over, it uses only the examples whose
file type matches the photo you are editing. For a JPEG or TIFF photo that
usually means none qualify: it keeps its own white balance, and the run's
summary tells you why. Everything else — exposure, contrast, presence, colour,
curves — means the same on both, and is blended from all matched examples as
usual.

Examples you saved earlier are read again from their stored develop settings,
so they take part with the right file type — and now with their white balance —
without being saved again. The exception is an example still on a process
version older than PV2012: it contributes nothing, and AI Edit's end-of-run
warnings say so when such an example was among the matches. Update it in the
Develop module and save it again.

Saving an example reports the same kind of problem right away — an old process
version, or develop settings that carry both kinds of white balance at once, in
which case its white balance is not learned. The summary at the end lists each
cause once, with the number of examples it hit.

What is learned from an example's white balance is how you set it — As Shot,
Auto, a preset or Custom — and, for Custom only, its temperature and tint. The
Kelvin an As Shot or Auto example happens to have belongs to that photo's light
and is never copied. See [Help: AI Edit](Help-AI-Edit) for how the examples'
choices decide what a photo gets.

---

## Tips

- Save at least 5 well-edited photos per style — that is the Style Engine's minimum — and 10–20 for good results. More diverse examples generalize better.
- Use consistent labels per shooting genre: one label for weddings, one for portraits, one for landscapes — rather than mixing everything.
- Re-run the workflow after editing sessions to keep examples fresh and representative.
- Training examples are not one input among several — they are the only one. AI Edit has no look presets, no style-strength slider and no prompt to fall back on, so whatever the examples say is what the recipe does.
