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

A develop recipe of **global adjustments** averaged from the matching
examples:

- **Basic:** exposure, contrast, highlights, shadows, whites, blacks, texture,
  clarity, dehaze, vibrance, saturation.
- **Tone curve:** the four parametric sliders (highlights, lights, darks,
  shadows) and the three region splits.
- **Color mixer (HSL):** hue, saturation and luminance of all eight colors.
- **Color grading:** hue and saturation of the shadows and highlights, and the
  balance. Tints are averaged as colors (a red at 350° and one at 10° average
  to red, not cyan), and a tint you set at zero saturation does not count.
  When your matching examples tint the shadows (or highlights) in clearly
  different colors, the tints partly cancel: that zone is toned down towards
  neutral (fully opposite tints leave no toning at all), and the end-of-run
  summary says so. Toning from black-and-white examples is not carried over,
  so a sepia black-and-white example does not tint a color photo. Midtones,
  global grading, luminance and blending are not carried over yet.
- **Detail:** sharpening amount, radius, detail and masking; luminance noise
  reduction with its detail and contrast; color noise reduction with its detail
  and smoothness.
- **Effects:** post-crop vignette (amount, midpoint, roundness, feather,
  highlights) and grain (amount, size, roughness).

White balance is decided separately; see *How white balance is carried over*
below. Point curves, lens corrections, the profile and crop are not carried
over.

A setting only some of the matching examples carry (a black-and-white example
has no color mixer, for instance) is averaged over the ones that have it, so it
is not pulled towards zero. Examples you saved before this release take part
with all of these settings without being saved again.

The color mixer, color grading, curve splits and the detail and effects
settings above came with a backend update alone: the plug-in already knew how
to apply them, so you do not need a new plug-in version to get them.

These values replace the photo's own settings in those panels, including a
color mixer, color grading, curve splits or detail and effects settings it
already had (earlier versions left those alone), usually with values near 0
when your examples did not use a panel. To keep an edited photo's own look,
turn on *Apply the edit to a new virtual copy*. The review dialog lists
`hsl`, `color_grading` and `tone_curve` whenever they are sent, even when every
value in them is 0.

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
from the server side. It asks the photo's own develop settings rather than the
file extension, so a DNG converted from a JPEG counts as a JPEG here — which is
how Lightroom treats its white balance.

A white balance that does not fit the photo is left out, never squeezed onto the
other scale: the photo keeps its own, and the run's summary says why.

### How white balance is carried over

White balance is not averaged like the sliders. Up to twenty of your matching
examples vote on *how* you set it, weighted by how well each one matches:

- **Mostly As Shot:** the photo keeps its own white balance. That is what your
  examples do, so nothing is reported.
- **Mostly Custom:** the temperature and tint of your best-matching Custom
  examples of the same file type as the photo (up to three) are applied — but
  only when at least two of them exist and their temperatures are within 500 K
  of each other. If they disagree, or there is only one, the photo keeps its
  own and the run's summary says so. A JPEG or TIFF photo is the exception,
  see below.
- **Mostly Auto or a preset such as Daylight:** not carried over yet; the
  summary says so. (Whether Lightroom recalculates the temperature when only
  the mode is set is still being tested.)
- **A JPEG or TIFF photo when Custom wins:** training examples are saved from
  raw and DNG files only, so their Kelvin temperature means nothing on a JPEG.
  The photo keeps its own white balance, with a note in the end-of-run
  warnings. With an As Shot habit a JPEG simply keeps its own, with no note.
  (The one exception is an example saved from a DNG converted from a JPEG: it
  has the JPEG kind of white balance and can pass it on to JPEG photos.)

These thresholds are a first setting and may be tuned. In this release white
balance transfer cannot be switched off separately; undo it in Lightroom's
Edit History if a photo should keep its own.

---

## Warnings at the end of a run

Anything that did not go as planned — a white balance left out, a low
style-match confidence, a recipe that could not be applied — is listed in the
dialog at the end of the run. A cause that hit many photos is listed once with a
count, for example *"White balance was not transferred … (12 photos)"*, so one
run-wide reason does not push every other report out of view. The five that
affected the most photos are shown; the dialog says how many more there were,
and the plug-in log has all of them.

With **review before apply** on, the review dialog lists the same notes for the
photo in front of you, so you see why a white balance was left out before you
decide.

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
