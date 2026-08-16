# Help: AI Edit Photos *(beta)*

> **Beta feature.** AI Edit generates generally good results but can occasionally produce edits that are too aggressive or miss the mark on specific scenes. Always use **Review each proposed edit** when running on photos you haven't tested this feature on before, and be ready to skip or undo individual edits. Feedback and bug reports via GitHub Issues are very welcome.

The **AI Edit Photos** workflow generates a structured Lightroom develop recipe for each photo and can apply it directly — without leaving Lightroom Classic.

## How to start

`Library → Plug-in Extras → AI Edit Photos...`

---

## What it generates

For each photo the AI produces a develop recipe that may include:

- **Global adjustments** — exposure, white balance, highlights, shadows, whites, blacks, contrast, clarity, vibrance, saturation, HSL color shifts, tone curve, sharpening, noise reduction.
- **Local masks** — subject, sky, or background masks with separate adjustments. Masks are only added when materially beneficial.

The recipe is applied via the Lightroom SDK. No raw pixel editing happens outside Lightroom; all results are reversible via Lightroom's Edit History.

---

## What the frame allows

A model that cannot see how the light actually falls will happily add contrast
to a scene that already has too much of it. So before a recipe reaches your
photo, the backend measures the image itself — how hard the light is, how many
stops of dynamic range there are, how much of the frame is specular highlight,
how far the shadows are already clipped — and works out how much contrast,
clarity, shadow lift and whites this particular frame can take. Values above
that ceiling are pulled back down.

The review dialog shows why, in plain sentences: hard midday light means no
extra contrast, flat overcast light means there is room for it, an already
clipped sky means the whites stay where they are. When the recipe stayed inside
the budget anyway, nothing is shown — there is no point reporting a limit that
never bit.

The judgement changes with the file type: raw files still hold detail behind
clipped highlights, JPEGs do not, so the same blown sky earns a stricter budget
on a JPEG. The plugin tells the backend which it is, since the photo is
exported to JPEG before upload and the original encoding is otherwise
invisible from the server side.

---

## Dialog options

### Scope

- **Selected photos only** — processes only the photos you have selected in the Library grid.
- **Current view** — all photos in the currently visible folder or collection.

### AI Model

Choose which LLM to use. The list is loaded from the backend at runtime and only shows providers that are configured and reachable. See [Help: Choosing AI Model](Help-Choosing-AI-Model).

### Overall look

Selects the editing style preset injected into the AI prompt:

| Preset | Description |
|---|---|
| General - Natural Professional | Balanced contrast, realistic color, clean detail. Default. |
| General - Moody Dramatic | Deeper shadows, restrained saturation, cinematic tonal separation. |
| Landscape - Cinematic | Controlled dynamic range, subtle color contrast, tasteful depth. |
| Landscape - Vibrant Natural | Clear tonal separation, protected highlights, controlled saturation. |
| Portrait - Skin Safe | Gentle contrast, natural texture, flattering highlights. |
| Portrait - Editorial | Clean skin tones, polished midtone contrast, soft highlight roll-off. |
| Wedding - Soft Airy | Bright mids, warm-neutral white balance, gentle contrast. |
| Wedding - Rich Filmic | Subtle warm skin tones, gentle black-point lift, cinematic color depth. |
| Real Estate - Bright Neutral | Bright neutral interiors, straight tonal balance, minimal stylization. |
| Commercial - Clean Product | Neutral white balance, crisp detail, true-to-product colors. |
| Street - Punchy Documentary | Decisive contrast, neutral color fidelity, clear subject separation. |
| Custom | Write your own style instruction in the text field. |

### Style strength

Controls how aggressively the preset style is applied, from subtle correction to full stylized look. Range: 0–100%. Default: 50%.

### Composition / crop

Whether the AI may suggest a crop:
- **No crop** — never adjusts framing.
- **Subtle crop** — only if clearly beneficial for composition.
- **Aggressive crop** — freely crops to improve composition.

Default: Subtle crop.

### Creative Controls

Each checkbox switches off a whole family of adjustments, so you can let the AI
work on tone but keep your own color, or the other way round:

- Adjust white balance
- Adjust basic tone (exposure/contrast/highlights/shadows/whites/blacks)
- Adjust presence (texture/clarity/dehaze)
- Adjust color mix (vibrance/saturation/HSL)
- Do color grading
- Use tone curve, and within it Use point curve
- Adjust detail, Adjust effects, Adjust lens corrections
- Include local masks

Anything you switch off is stripped from the recipe before it reaches your
photo, whatever the model proposed.

### Review each proposed edit before applying it

When enabled, a **review dialog** opens for each photo before the edit is applied. You see:

- The proposed develop values, plus which engine produced them and how confident it was.
- Any guardrail explanations — what the frame allowed, and what got capped.
- Options to **Apply** or **Skip**.

**Recommended for first use.** Disable only after you've validated the results for your shooting style.

> **No before/after preview yet.** The review dialog lists values; it does not render a comparison. A rendered before/after is planned. Until then, Lightroom's own History panel is the fastest way to judge a result — every run is a single undo step named *Apply AI Lightroom develop settings*.

### Per-photo instruction

If **Allow per-photo instructions** is enabled, a free-text field opens *before* the edit is generated — for example "Make the sky more dramatic" or "Reduce noise in shadows". A checkbox carries the same instruction to all following photos.

Note this happens before generation, not after: there is currently no way to re-run a single photo from the review dialog with a changed instruction. Skip the photo and run it again to do that.

---

## Training the AI on your style

If you want AI Edit to match your personal editing style, use **Save Edits as AI Training Examples** to feed your existing edits back to the AI as few-shot examples. See [Help: Train from Edits](Help-Train-From-Edits).

---

## Tips for best results

- Start with **Review each proposed edit** enabled — review the first batch before applying to hundreds of photos.
- Choose a preset that matches your genre (portrait, landscape, wedding, etc.) rather than using *Custom* initially.
- **Style strength 40–60%** is a good starting range. Higher values push the style harder; lower values stay close to a neutral technical correction.
- If results are inconsistent, try a higher-quality model (e.g. `gemini-2.5-pro` or `gpt-5.4-pro`).
- Use **Save Edits as AI Training Examples** after a manual editing session to teach the AI your preferences.
