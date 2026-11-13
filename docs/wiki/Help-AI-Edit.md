# Help: AI Edit Photos *(beta)*

> **Beta feature.** Always use **Review each edit** when running on photos you
> haven't tried this feature on before, and be ready to discard individual edits.
> Feedback and bug reports via GitHub Issues are very welcome.

**AI Edit Photos** edits your photos in Lightroom Classic and applies the result
directly, as ordinary develop settings you can keep working on.

## How to start

`Library → Plug-in Extras → AI Edit Photos...`

AI Edit has two editors; pick one under **Editor**:

- **AI retoucher** *(recommended)* — an AI model edits each photo, looks at what
  Lightroom renders, and corrects it, like a person at the Develop panel. With
  enough saved edits it starts from your own style.
- **My style only** — no AI model: every recipe is a blend of your own saved
  edits on similar photos. Offline and free, but it never looks at the result.

---

## The AI retoucher

### Look, edit, look again

A vision model is poor at guessing slider values for a photo it has never seen
rendered — "+0.4 exposure" means something different on every file and profile —
and good at judging a picture and saying what is wrong with it. So the retoucher
works in rounds:

1. Lightroom renders the photo as it is. The model sees it with **measurements**
   of its tones — brightness percentiles, how much is clipped, the colour cast of
   the neutral tones, saturation — and the values every slider starts from.
2. The model sends a complete edit: white balance, exposure, the tonal range,
   presence, HSL, colour grading, effects, and **AI masks** where global sliders
   would hurt the rest of the photo (a darker, richer sky; a lifted subject; a
   calmer background; smoother skin; brighter eyes).
3. Lightroom applies it, computes the masks and renders the result. The model
   sees the render next to the original, with fresh measurements and what moved
   ("median 38 → 47, clipped highlights 0.4 % → 1.9 %"), and corrects.
4. It stops when another round would not clearly help — or goes back to an
   earlier round that looked better.

Every round is the whole edit measured from where the photo started, so a round
can undo anything the one before did. The measurements keep the model honest
about what the eye misjudges: clipping, levels and casts.

### Your style as the starting point

Tick **Start from my own style** (it needs five saved training examples, see
[My style only](#my-style-only)) and round 0 is your style: AI Edit blends your
saved edits for the photo, applies the blend and shows the model the original
and your style side by side, with your style's values. The model is told to keep
your look — colour, contrast, choices — and to fix only what does not suit this
frame: exposure, white balance, clipping, emphasis, local work. Its edits are
made on top of your style: a setting it does not touch keeps your style's value,
so your colour mix, grading and detail settings survive every round. It can also
decide your style was already right and keep it unchanged.

### Look and your own words

- **Look** sets the direction: *Natural* (a better version of the photo, the
  default), *Bright & airy*, *Moody*, *Cinematic*, *Analog film*,
  *Vivid & punchy* or *Black & white* (converts to monochrome and shapes the tones
  with the black-and-white mix).
- **Your instructions** are free text for the run, e.g. *keep skin warm, no
  vignette*. They win over the look where the two disagree.
- **Edits per photo** caps the rounds (default 3). One round is the old
  single-shot edit; most photos settle in two or three.

### AI masks

With **Use AI masks** the model can use Lightroom's own AI masks: subject, sky,
background, the parts of people (face skin, body skin, hair, lips, teeth, iris
and pupil, eye whites, eyebrows, facial hair, clothes — of everyone in the
photo), and landscape classes (architecture, mountains, vegetation, water,
natural and artificial ground, snow). Lightroom finds the regions itself, at
full resolution, whatever the size of the image the model saw.

The masks are named *LrGenius · Sky*, *LrGenius · Subject*, … in the Masks panel.
A later round, or a later AI Edit run, replaces them instead of stacking new
ones; masks you made yourself are left as they are. A mask that finds nothing
(a sky mask on an indoor photo) is reported back to the model, which then
drops it; if the edit you end up with still has one, the review and the run's
summary say so.

AI masks need **Lightroom Classic 15.3 or later**; on older versions the option
is greyed out and the retoucher edits with global sliders only.

### Which model

Any provider set up in Plug-in Manager works — a cloud model (Google Gemini,
OpenAI, Anthropic Claude), an AI server (Ollama, LM Studio, the Other AI server)
or a model on this computer — as long as it **sees images** and can **call
tools**. The dialog defaults to the model chosen for Analyze & Index.

Editing asks more of a model than tagging. Large cloud models edit noticeably
better than small local ones; a small local model may not manage the tool calls
at all, and the run then says so. Cost and time per photo are one model call
per round, each with the photo at 1536 px plus the latest render, and one
Lightroom render per round — with a cloud model typically a few seconds per
round and a few cents per photo for the larger models; the run's summary lists
the tokens used.

> **Privacy.** With a cloud model, renders of every photo you edit go to that
> provider, as they do for Analyze & Index. Pick a model on this computer to keep
> them here.

### Review, keep or discard

With **Review each edit** on, every finished edit is shown with the photo, the
round that was kept, the model's assessment of each round, what the frame
allowed, and anything that did not reach the photo as asked. **Keep** keeps it,
**Discard** takes the photo back to exactly where it was (sliders and masks),
**Keep all** keeps this one and the rest of the run without asking.

Every round is a step in the photo's Edit History (*AI Edit: round 1*, …), so
you can also step through them there. Tick **Apply the edit to a new virtual
copy** to keep the original's history clean.

---

## My style only

### It edits like you do — and only like you do

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

The upshot: ***My style only* needs training examples to work at all.** With
fewer than five saved examples the task stops before it uploads anything and
points you at *Save Edits as AI Training Examples*. Five is the floor, not the target — the
style profile is reported as *Building up* below ten examples and *Active* from
fifty. See [Help: Train from Edits](Help-Train-From-Edits).

Because the recipe comes from your own edits, there is no prompt, no model
choice, no look and no API key involved. *My style only* works with every cloud
provider switched off.

---

### What it generates

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

### Refine with the aesthetic score *(optional)*

Tick **Refine my style with the aesthetic score** in the options and AI Edit asks
a second opinion before it settles on an edit: it applies the recipe, renders the
photo, and has the aesthetic model score the result. If a gentler version scores
clearly better, it steps the strength down — 100%, 75%, 50%, 25% — and keeps the
best-scoring one.

What the score decides is **how much of your style to keep**, never what the style
is:

- The loop only moves between "no edit" and the recipe as it came back. It never
  goes above 100%, so the limits described in *What the frame allows* below still
  hold.
- A gentler edit has to beat the stronger one by a clear margin to win. On a tie
  the stronger edit — the one closer to your own — stays.
- The scorer is a general model of what photographs look good, not of what *you*
  like. Leave this off if you would rather judge the edit yourself, and keep
  **Review each proposed edit** on to see the recipe before the loop runs.

The cost is a render and a score per strength tried: a few seconds per photo, at
most four tries. Each try is applied to the photo for real, so it shows up in the
photo's Edit History — tick **Apply the edit to a new virtual copy** to keep the
original's history clean. When the loop is done the photo carries only the
strength it chose.

When the finished edit still scores lower than the original, the completion dialog
lists the photo so you can look at it. A photo the scorer could not handle is
edited at full strength and listed too. If the aesthetic model is not downloaded,
the run asks once before it starts: download it from Plug-in Manager, or carry on
without refinement.

---

---

## What the frame allows

Your habitual +25 contrast was learnt on the frames you shot, and this frame may
have nothing in common with them; an AI model, for its part, likes clarity and
contrast more than most photographs do. So before any recipe reaches your photo
— from your style or from the retoucher, every round — the backend measures the
image itself — how hard the light is, how many stops of
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

With **Review each edit** on, the review dialog lists the same notes for the
photo in front of you, so you see why a white balance was left out before you
decide.

---

## Dialog options

### Scope

- **Selected photos only** — processes only the photos you have selected in the Library grid.
- **Current view** — all photos in the currently visible folder or collection.
- **All photos in catalog** — everything.

### Editor

*AI retoucher* or *My style only*, see above. The AI retoucher's settings —
model, look, instructions, edits per photo, your style as the starting point,
AI masks — are greyed out for *My style only*.

### Style profile

Read-only. Shows how many training examples the backend holds and what that
means for the match quality, so you can tell an unconvincing result caused by a
thin style profile from one caused by an unusual photo.

### Review each edit

For the **AI retoucher**: the finished edit is shown to keep or discard, see
[Review, keep or discard](#review-keep-or-discard).

For **My style only**: a review dialog opens for each photo *before* the edit
is applied, with the proposed develop values, how confident the style match was
and which of your saved edits it drew on, and the guardrail explanations — with
the face models downloaded these include the faces in the frame. Options to
**Apply** or **Skip**.

**Recommended for first use.** Disable only after you've validated the results
for your shooting style.

### Apply the edit to a new virtual copy

Off by default. When enabled, each photo gets a virtual copy named *AI Edit*
and the edit goes onto the copy — your original keeps whatever settings it had.

For *My style only* the copy is created *after* the review dialog, so a photo
you skip leaves nothing behind. The AI retoucher edits for real while it works,
so its copy comes first; a copy whose edit you discard stays in the catalog with
the original's settings, and the run's summary says how many. Lightroom does not
let plugins delete photos: remove them with *Photo → Remove Photo*. If Lightroom
refuses to create a copy, the photo is reported as an error and skipped; the
edit is never redirected onto the original.

Two side effects come from Lightroom's own API here: copying works on the
current selection, so your grid selection changes as the run walks through the
photos, and if a photo is not part of the folder or collection you are looking
at, the plugin switches the source to *All Photographs* to reach it.

### Refine my style with the aesthetic score

*My style only*. Off by default. Tries gentler strengths of the recipe and keeps
the one the aesthetic model scores best — see
[Refine with the aesthetic score](#refine-with-the-aesthetic-score-optional)
above. Slower, and each try is a step in the photo's Edit History; pair it with
the virtual copy option to keep the original's history clean.

---

## Tips for best results

- Start with **Review each edit** enabled — review the first batch before applying to hundreds of photos.
- Give the retoucher a look and, where it matters, a sentence of your own: *keep the blue hour cool*, *skin natural, no smoothing*.
- Save your best edits as training examples and tick **Start from my own style**: the retoucher then keeps your look and fixes the frame, which is usually the best of both.
- For *My style only*: train on the kind of photo you are about to edit. The style profile is matched per photo, so a profile built only from bright studio work has little to offer a night shot. A low confidence in the review dialog means the photo did not resemble anything you have trained on — edit it by hand and save that edit as a training example.
