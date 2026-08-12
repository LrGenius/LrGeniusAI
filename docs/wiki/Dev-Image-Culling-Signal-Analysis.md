# Culling: Signal Quality and Genre Analysis

Reference material behind the culling performance and correctness pass. The
*what changed and what is outstanding* view lives in
[Dev-Image-Culling-Implementation-Plan.md](Dev-Image-Culling-Implementation-Plan.md);
this page is the reasoning underneath it — which signals cap ranking accuracy
today, and what "best frame" actually means across photographic genres.

Line references point at the state of the code when this was written; treat
them as signposts rather than exact addresses.

## Where the quality ceiling is

### Sharpness is global, and that is the wrong question — FIXED (region), OUTSTANDING (eye crop)
`metrics.rs:196-211` takes Laplacian variance over the whole 512 px frame.

- **Shallow DOF is punished.** An f/1.4 portrait or a 400 mm wildlife frame has a razor-sharp
  subject and smooth background; global variance is *lower* than a mediocre f/8 shot. The
  `sports` preset's only answer is lowering a threshold (`culling_config.rs:218`).
- **Content, not focus, dominates.** Busy foliage beats a clean minimalist frame regardless of
  whether anything is in focus.
- **The decisive detail is invisible at 512 px.** Eye-sharp vs. nose-sharp — the most common
  portrait cull decision there is — is not resolvable at that scale.

**Fix:** compute sharpness on **regions**, at native resolution.
1. Tile the working image; keep `max`-over-tiles ("is anything in focus?") and the location of
   the sharp region alongside the global mean. The pair distinguishes "soft frame" from
   "correctly shallow".
2. Where faces exist, measure sharpness on a **native-resolution eye-region crop**, not the
   2048 px working image. Face boxes are already available from SCRFD.
3. Add a **motion-blur direction** estimate (anisotropy of the gradient orientation histogram)
   so directional camera shake is separable from defocus and from intentional panning.

> **Status.** Points 1 and 3 shipped. Tiling accumulates per-tile Laplacian variance in the
> existing pass, giving `cull_sharpness_peak`, `cull_focus_concentration` and
> `cull_sharp_region_{x,y}`; ranking blends peak into effective sharpness at
> `sharpness_peak_weight` (0.70 `sports`, 0.55 `portrait`, 0.25 `street`). Point 3 uses the
> structure tensor rather than an orientation histogram — the coherence
> `√((Jxx−Jyy)² + 4Jxy²)/(Jxx+Jyy)` is the same quantity for two axes and costs three
> accumulators. It is **not scored**, only used to label a soft frame `motion_blur` instead of
> `blurred`, because directional *content* (architecture, horizons, rain) raises it identically.
> Point 2 — the native-resolution eye crop — is still outstanding.

### Eye-openness: the aggregation is inverted for group shots — FIXED
`face_aggregate.rs:68-135` rolls multiple faces up as `eye_openness = max`,
`blink_penalty = min`. In a ten-person group shot where nine people blink and one has eyes open,
the photo scores **maximum** eye-openness and **minimum** blink penalty. That is exactly
backwards, and group shots are the highest-value case in event work.

**Fix:** aggregate eyes-open as a **face-size-weighted min** (worst prominent face decides),
keep `max` only for prominence. Currently nothing is weighted by face size at all.

### Blink detection itself is a ≤8 px gradient — OUTSTANDING
`face_quality.rs:56-104`: mean absolute *vertical* gradient in a patch of radius
`clamp(min(w,h) × 0.08, 2, 8)` around SCRFD's two eye keypoints.

- The `EYE_PATCH_RADIUS_MAX: 8` clamp means the window **stops scaling with face size** — on a
  headshot where the face is 1200 px, the analysis window is a 17×17 px sliver.
- SCRFD runs on a **640×640 letterbox** (`scrfd.rs:15`), so keypoints carry several pixels of
  error when scaled back, and small faces in group shots go undetected.
- Eyelashes, eyeliner, glasses frames and dark eyebrows all produce strong vertical gradients on
  a *closed* eye; squinting in sunlight reads as closed.
- **`blink_penalty` is literally `1.0 - eye_openness`** (`faces.rs:279`) — it carries no
  independent information, yet `reject_blink_penalty_threshold` is tuned as if it did.

**Fix:** a small eye-state ONNX classifier on a native-resolution eye crop (24×24 or 32×32,
1–3 MB, sub-millisecond). SCRFD's single point per eye cannot support an eye-aspect-ratio
measure, so this genuinely needs a model. Remove `blink_penalty` as a separate field or derive
it from the classifier's confidence.

### `occlusion` does not measure occlusion — FIXED
`face_quality.rs:107`: `1 − (0.55·det_score + 0.20·center_proximity + 0.25·eye_openness)`. It
never looks at the image beyond signals it already has, and `center_proximity` is a
*composition* heuristic — a face near the frame edge is not occluded. The field is ~collinear
with `det_score`, so `reject_occlusion_threshold` is effectively a second detector-confidence
gate. Either replace it with a real measurement or delete it and stop pretending.

> **Status.** Replaced, not deleted. `occlusion_from_crop` combines two measurements that fail
> on different inputs: the **landmark-fit residual** against the ArcFace template (a similarity
> transform absorbs translation, rotation and scale, so only the face's *shape* changing moves
> it — which is what a covered feature does to the detector's estimate) and **mirror asymmetry**
> on the aligned 112 px crop, normalised by the crop's own luminance spread so it is not just
> re-measuring contrast. Neither `det_score` nor `center_proximity` is an input any more.
>
> Two honest caveats. Strong out-of-plane rotation raises the geometry term, and hard side
> lighting raises the symmetry term; the field measures "obstructed *or* turned away", which is
> what the plan calls "occlusion / poor facial visibility" but is not pure occlusion. And a
> `FACE_TABLE` row written before this existed carries no substitute — `face_aggregate` now
> scores a missing value 0 rather than reconstructing it, because a reject nominated from a
> missing field is worse than a reject missed.

### "Aesthetic" is contrast + colorfulness — FIXED
`AESTHETIC_CONTRAST_WEIGHT 0.45 / COLORFULNESS 0.35 / EXPOSURE 0.20` (`metrics.rs:125`). This
rewards punchy saturated frames and floors muted fine-art portraiture, fog, snow, and every
desaturated editorial look.

**Fix — and this is nearly free:** the SigLIP2 embedding is already computed and stored.
- A **LAION-aesthetic-style linear head** over that embedding is a dot product.
- **CLIP-IQA** style antonym prompt pairs ("a sharp photo" vs "a blurry photo", "a
  well-composed photograph" vs "a badly composed photograph") need only `embed_text`, which
  **already batches** (`siglip.rs:197`), computed once and cached.

Both are ~zero marginal cost per photo and replace a heuristic that is actively wrong. They
apply only when embeddings exist, so the fast cull path degrades to technical signals.

> **Status.** The CLIP-IQA route shipped (`lrg-ml/src/clip_iqa.rs`); the LAION head did not,
> because it would mean shipping and versioning another weights file for a signal the prompt
> pairs already provide. Five pairs, embedded once per server lifetime and cached in `AppState`,
> then a two-way softmax over cosine similarity per pair. The **logit scale of 100 is load
> bearing**: raw cosines for a prompt pair differ by a few hundredths, which a plain softmax
> flattens to ~0.5 for every image. Blended against the heuristic at `aesthetic_iqa_weight`
> (0.8) rather than replacing it outright, and skipped silently when there is no embedding or
> the text tower will not load.
>
> Note the prompts deliberately do **not** ask about sharpness or exposure, though the passage
> above suggests "a sharp photo" vs "a blurry photo". Those are measured directly from pixels
> far more reliably than a 1152-dim embedding can judge them, and asking twice only adds a noisy
> second opinion to a question already answered.

### Exposure and noise are absolute, and they fight each other — FIXED (exposure), OUTSTANDING (noise)
`EXPOSURE_TARGET: 0.5`, `EXPOSURE_TOLERANCE: 0.35` (`metrics.rs:117`) penalise low-key
portraits, concert and stage work, night, silhouettes and high-key fashion for being correctly
exposed for their genre. Meanwhile the noise estimate (`metrics.rs:243-266`) is a 3×3 box-blur
residual — a high-pass measure — and fine detail *is* high-pass energy, so a sharper frame
scores as noisier and `0.5·sharpness` partly cancels `0.15·(1−noise)`.

**Fix:** score exposure and noise **relative to the group** (z-score within group, or rank),
not against absolutes. Keep absolute clipping fractions, which are genuinely absolute. For
noise, gate the residual on low-gradient regions only so texture stops registering as grain.

> **Correction after implementing it.** Exposure was normalised against the group and benefits.
> **Noise was not, and must not be.** Relative scoring assumes the metric is at least pointing
> the right way; this one is not. Measured on two frames of one scene differing only in focus,
> the raw estimator scores the *sharp* frame 0.082 and the blurred one 0.012 — so rescaling to
> the group's range stretched the gap to 0.535, amplifying the error eightfold. The stated
> motivation does not hold either: a group shot entirely at high ISO is penalised *equally*, and
> a constant offset cannot change a within-group ranking. Only the second half of the fix above
> — gating the residual on low-gradient regions so texture stops registering as grain — actually
> addresses this, and it remains outstanding.

### Ranking is absolute where it should be relative — FIXED
`rank_group_records` (`grouping.rs:223`) weight-sums absolute 0–1 metrics. Reason codes use
within-group deltas, but the *score* does not. A group shot entirely in dim light gets uniformly
crushed technical scores and the ordering falls to whichever metric happens to retain dynamic
range. Normalise each metric within its group before weighting.

### Intentional multi-frame sets are destroyed — FIXED
Nothing detects HDR brackets, focus stacks or panorama sequences. They are near-identical frames
close in time — precisely the grouper's signature — so it nominates a winner and marks the rest
reject candidates. This is the most damaging single failure mode for landscape, architecture and
real-estate users.

**Fix:** detect before ranking, from EXIF the plugin already has:
- **Bracket**: same scene, monotone exposure ladder, `exposureBias` varying, aperture/ISO
  otherwise consistent → mark `group_type = "bracket"`, suppress winner/reject entirely.
- **Focus stack**: same scene, static framing, sharp-region *location* migrating across frames.
- **Panorama**: sequential frames, consistent exposure, partial content overlap with a
  translational shift.

> **Correction after implementing it.** "Same scene" cannot be verified by pHash for a bracket,
> and the first implementation's attempt to do so rejected every bracket it was meant to confirm.
> Measured against a real +2 EV frame of an otherwise identical scene, the Hamming distance to
> the base frame is **61 of 64 bits**: clipping into the highlights flips which DCT coefficients
> sit above the median, so the hash comes back near-complemented. Changing the exposure is
> exactly what destroys a perceptual hash, which makes it useless on the one input that matters
> here.
>
> Worse, the same failure meant the grouper never put the frames in one group to begin with — on
> the fast `tasks=cull` path there is no embedding, so pHash was its only similarity signal.
> Detection was running on groups that could never contain a bracket.
>
> The replacement evidence is the exposure pattern itself: three or more **evenly spaced** stops
> spanning at least a full EV, shot within a few seconds. Only auto exposure bracketing produces
> that, and the false positive it needs to exclude — a photographer riding the compensation dial
> through a burst — lands on uneven steps. Grouping gained a matching `bracket_edges` rule that
> joins frames close in time whose exposure compensation differs, without asking pHash.
>
> Focus-stack detection *does* still check framing, and correctly: there the exposure is
> constant, so the hash means what it says.

## Dead config that silently did nothing — FIXED

Presets appeared tunable but largely were not. All four of these are now fixed;
kept here because they explain why preset tuning historically had no effect:

- **`ImageMetricsConfig` is entirely dead.** Every field has zero references outside
  `culling_config.rs`; the live values are duplicated as `const`s in `metrics.rs:112-127`. **No
  preset can tune any image metric.**
- **Most of `FaceMetricsConfig` is dead**, duplicated as `const`s in `face_quality.rs:6-13` and
  `face_aggregate.rs:5-15`. Only the five `score_weight_*` are read.
- **`grouping.time_window_default_seconds` is dead** — `derive_grouping_thresholds` takes the
  window from the request only (`grouping.rs:74`). The `event` (=2) and `sports` (=3) preset
  overrides **do nothing**.
- **`grouping.duplicate_distance_auto = 0.05` is dead**, superseded by the
  `min + normalized·span` formula yielding 0.0294 at defaults.

Thread `CullingConfig` through `culling_metrics` and `face_quality` and delete the duplicated
constants. Without this, every genre-tuning change in Part 3 is a no-op.

---

## What "best frame" means, per genre

Presets today are `default / event / portrait / sports / street` (`culling_config.rs:228`),
differ only in ranking weights, and the user picks one by hand.

| Genre | Volume | What decides the pick | What breaks today |
|---|---|---|---|
| **Wedding / event** | 2–5k/day | Every face eyes-open; key-person priority; the moment (kiss, ring, first dance); mixed dim light | `max`-aggregation hides nine blinkers behind one open-eyed face; blink proxy weak; absolute noise penalty misfires at high ISO |
| **Portrait / headshot** | low, high precision | Sharpness **on the eye plane**; catchlight; micro-expression; hair across face; half-blink | 512 px global sharpness cannot resolve eye vs. nose focus; 17 px eye window |
| **Sports / action** | 1–5k bursts | Subject sharp — background *should* be soft; peak action; subject not clipped by frame edge | Global sharpness penalises correct shallow DOF and panning |
| **Wildlife / birds** | large bursts | Animal **eye** sharp + catchlight; wing/limb position; background separation | SCRFD is human-only; no animal-eye detection at all |
| **Landscape / architecture** | low | Corner-to-corner sharpness; horizon level; **brackets/stacks/panos must survive intact** | Intentional sets are culled |
| **Street / documentary** | low burst rate | Moment and gesture over technical; grain and motion blur are legitimate | Technical weights over-penalise the aesthetic |
| **Real estate / product** | tripod-locked repeats | Sharpest of identical frames; verticals; bracket sets | Bracket sets again |
| **Family / kids / pets** | chaotic | Multiple subjects eyes-open; expression; motion-blur tolerance | Same inverted eye aggregation |
| **Concert / low light** | high ISO | Noise is expected — penalise *relatively*; stage clipping is normal | Absolute exposure target and absolute noise penalty both misfire |
| **Astro / night** | low | Star trailing; intentional darkness | Exposure metric inverts the ranking |

### Two structural conclusions

**Genre should be detected, not asked.** SigLIP2 embeddings already exist for indexed photos;
zero-shot classification against a fixed genre prompt set is a dot product against cached text
embeddings — effectively free. Detect per *group*, expose it in the response as
`detected_genre` + confidence, and let the user's explicit preset override it. Fall back to
`default` when no embedding exists (the fast cull path) or confidence is low.

**Genre changes which signals are trusted, not just their weights.** A weights-only preset
system cannot express:
- sports/wildlife: use **subject-region** sharpness, *ignore* background sharpness;
- landscape/real-estate: run bracket/stack/pano detection and **suppress culling** for those
  groups;
- event/family: aggregate eyes-open as size-weighted **min**, not max;
- concert/astro: switch exposure and noise to **relative-only** scoring.

So the preset struct needs *behavioural* fields (which sharpness estimator, which face
aggregation, whether set-detection is active), not only more `f64` weights.

### Learning the user's taste

The strongest differentiator, and the infrastructure precedent already exists
(`TaskTrainFromEdits.lua`, `lrg-analysis/src/training.rs`, the `edit_training` LanceDB table).

Every photographer's catalog is full of free labels: pick flags, star ratings, which frames got
develop edits, which got exported. Train a small pairwise ranking head on
`[SigLIP2 embedding ‖ technical metrics]` over **within-group** comparisons drawn from that
history, and blend it as a weak term. This is what turns "technically correct" picks into
"picks that look like yours". Ship it behind an explicit opt-in with a visible confidence, after
the deterministic layers are solid.

---
