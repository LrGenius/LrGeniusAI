# Help: AI Edit Photos *(beta)*

> **Beta feature.** AI Edit generates generally good results but can occasionally produce edits that are too aggressive or miss the mark on specific scenes. Always use **Review each proposed edit** when running on photos you haven't tested this feature on before, and be ready to skip or undo individual edits. Feedback and bug reports via GitHub Issues are very welcome.

The **AI Edit Photos** workflow generates a structured Lightroom develop recipe for each photo and can apply it directly — without leaving Lightroom Classic.

## How to start

`Library → Plug-in Extras → AI Edit Photos...`

---

## It edits like you do — and only like you do

AI Edit does not ask a language model what a good edit looks like. It builds
every recipe from **your own saved edits**: the photo is matched against the
training examples you stored with *Save Edits as AI Training Examples*, and the
closest matches are blended into a recipe.

The match is scored on four things:

| Signal | Weight |
|---|---|
| Visual similarity (the photo's CLIP embedding) | 50% |
| Exposure character — brightness, contrast, warmth | 25% |
| Scene tags | 15% |
| Time of day | 10% |

The upshot: **AI Edit needs training examples to work at all.** With fewer than
five saved examples the task stops before it uploads anything and points you at
*Save Edits as AI Training Examples*. Five is the floor, not the target — the
style profile is reported as *Building up* below ten examples and *Active* from
fifty. See [Help: Train from Edits](Help-Train-From-Edits).

Because the recipe comes from your own edits, there is no prompt, no model
choice, no style preset and no API key involved. AI Edit works with every cloud
provider switched off.

---

## What it generates

A develop recipe of **global adjustments** — exposure, white balance,
highlights, shadows, whites, blacks, contrast, texture, clarity, dehaze,
vibrance, saturation, tone curve, sharpening, noise reduction, vignette, grain
— averaged from the matching examples.

Local masks are not part of a style-engine edit; every recipe carries an empty
mask list.

The recipe is applied via the Lightroom SDK. No raw pixel editing happens
outside Lightroom; all results are reversible via Lightroom's Edit History.

---

## What the frame allows

Your habitual +25 contrast was learnt on the frames you shot, and this frame may
have nothing in common with them. So before a recipe reaches your photo, the
backend measures the image itself — how hard the light is, how many stops of
dynamic range there are, how much of the frame is specular highlight, how far
the shadows are already clipped — and works out how much contrast, clarity,
shadow lift and whites this particular frame can take. Values above that ceiling
are pulled back down.

The review dialog shows why, in plain sentences: hard midday light means no
extra contrast, flat overcast light means there is room for it, an already
clipped sky means the whites stay where they are. When the recipe stayed inside
the budget anyway, nothing is shown — there is no point reporting a limit that
never bit.

The judgement changes with the file type: raw files still hold detail behind
clipped highlights, JPEGs do not, so the same blown sky earns a stricter budget
on a JPEG. The same distinction decides whether a training example's white
balance can be carried over at all — Lightroom's temperature is Kelvin on a raw
file and a relative −100…100 value on everything else, and the two cannot be
averaged together. The plugin tells the backend which it is, since the photo is
exported to JPEG before upload and the original encoding is otherwise invisible
from the server side.

---

## Dialog options

### Scope

- **Selected photos only** — processes only the photos you have selected in the Library grid.
- **Current view** — all photos in the currently visible folder or collection.
- **All photos in catalog** — everything.

### Style profile

Read-only. Shows how many training examples the backend holds and what that
means for the match quality, so you can tell an unconvincing result caused by a
thin style profile from one caused by an unusual photo.

### Review each proposed edit before applying it

When enabled, a **review dialog** opens for each photo before the edit is applied. You see:

- The proposed develop values, plus how confident the style match was and which of your saved edits it drew on.
- Any guardrail explanations — what the frame allowed, and what got capped.
- Options to **Apply** or **Skip**.

**Recommended for first use.** Disable only after you've validated the results for your shooting style.

> **No before/after preview yet.** The review dialog lists values; it does not render a comparison. A rendered before/after is planned. Until then, Lightroom's own History panel is the fastest way to judge a result — every run is a single undo step named *Apply AI Lightroom develop settings*.

### Apply the edit to a new virtual copy

Off by default. When enabled, each photo that is actually going to be edited
gets a virtual copy named *AI Edit* first, and the recipe is applied to that
copy — your original keeps whatever settings it had.

The copy is created *after* the review dialog, so a photo you skip leaves
nothing behind. If Lightroom refuses to create the copy, the photo is reported
as an error and skipped; the edit is never redirected onto the original.

Two side effects come from Lightroom's own API here: copying works on the
current selection, so your grid selection changes as the run walks through the
photos, and if a photo is not part of the folder or collection you are looking
at, the plugin switches the source to *All Photographs* to reach it.

---

## Tips for best results

- Start with **Review each proposed edit** enabled — review the first batch before applying to hundreds of photos.
- Train on the kind of photo you are about to edit. The style profile is matched per photo, so a profile built only from bright studio work has little to offer a night shot.
- Keep feeding it: run **Save Edits as AI Training Examples** after a manual editing session. Match quality improves with the number and variety of examples.
- A low confidence in the review dialog means the photo did not resemble anything you have trained on. That is the moment to edit it by hand — and then save that edit as a training example.

---

## Where the LLM went

Earlier versions offered a second, prompt-driven path: pick a provider and
model, write a system instruction, choose a look preset, and let a vision LLM
propose the develop settings. That path is no longer reachable from the plugin.
The backend still implements it (`POST /v1/edit/recipe`), so it can come back, but the AI
Edit dialog no longer configures it and no run reaches an LLM.

Model choice now only affects [Analyze & Index](Help-Analyze-and-Index) —
tagging, descriptions and keywords.
