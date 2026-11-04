# Image Culling Implementation Plan

> **Status: a performance and correctness pass has landed on top of the MVP
> described below.** See [Current state](#current-state-after-the-performance--quality-pass)
> at the end of this document for what changed, what the measured numbers are,
> and what is still outstanding. Several items in the original checklist were
> ticked optimistically — in particular the evaluation benchmark set does not
> exist.

## Goal

Build an `Image Culling` workflow that is useful for real photographers, explainable, affordable to run, and compatible with the existing `Lightroom + local backend` architecture.

This plan assumes that the current LLM-based quality scoring remains inactive and is **not** used as the core ranking signal for the MVP.

> **Status note:** The MVP described in this document has been implemented in the backend (`group_and_sort_images` plus culling metrics and presets) and in the Lightroom plugin task `Cull Similar Photos`. The checklist at the end reflects the original implementation plan.

## Product Direction

The first release should not try to answer the vague question "How good is this photo?" with a single expensive model score.

Instead, the workflow should:

1. group similar photos into burst/stack candidates
2. detect obvious rejects using technical and face-aware signals
3. rank images relative to other images in the same group
4. create Lightroom collections for picks, alternates, and reject candidates

## Guiding Principles

- Prefer deterministic, explainable signals over opaque global scores.
- Rank relatively within similar groups, not only globally across the full catalog.
- Keep the hot path local and affordable. Do not use LLMs per image for core culling decisions.
- Store individual culling signals separately so they can be inspected, tuned, and reused.
- Make the first version review-friendly rather than fully automatic.

## MVP Scope

The MVP should support:

- culling within `selected photos` and `current view`
- grouping near-duplicate and burst images
- picking the best image per group
- identifying weak images and reject candidates
- creating Lightroom collections from the result
- showing short reasons for ranking decisions

The MVP should not require:

- genre-specific tuning for every photography niche
- personalized taste learning
- LLM-generated critiques
- fully automatic star ratings across the whole catalog

## Recommended Scoring Model

Use a layered ranking model instead of a single score.

### Layer 1: Grouping

Group photos using a combination of:

- `capture_time`
- `pHash` or another cheap near-duplicate signal
- existing image embeddings
- optional face-count consistency for people-heavy bursts

### Layer 2: Hard reject signals

Compute clear negative indicators such as:

- strong blur or missed focus
- face blur when faces are present
- eyes closed / blink
- severe exposure problems
- obvious occlusion or poor facial visibility

### Layer 3: Best-shot signals

Within each group, reward:

- sharpest main subject
- best face quality
- best expression / eyes-open result
- cleanest exposure
- strongest relative composition or framing

### Layer 4: Optional aesthetic prior

Add a lightweight aesthetic model later as a weak secondary signal:

- `NIMA`
- CLIP-based aesthetic predictor
- newer IQA / aesthetics models after evaluation

This layer should influence ordering, but not override hard technical rejects.

## Data Model

Do not store only one `overall quality` field. Store granular culling signals.

Recommended new backend metadata fields:

- `cull_group_id`
- `cull_group_size`
- `cull_group_rank`
- `cull_group_winner`
- `cull_score`
- `cull_sharpness`
- `cull_face_sharpness`
- `cull_blink_penalty`
- `cull_exposure`
- `cull_noise`
- `cull_occlusion`
- `cull_aesthetic`
- `cull_reject_candidate`
- `cull_reason_codes`
- `cull_explanation`

## Backend Work Plan

### Phase 1: Grouping Foundation

- Implement `group_and_sort_images(...)` in `server/src/services/chroma.py`.
- Add burst grouping logic based on time window plus similarity threshold.
- Support both true duplicates and near-duplicates.
- Return groups in a structure that the plugin can map to Lightroom collections.

Acceptance criteria:

- Similar photos are grouped reliably for common burst sequences.
- Ungrouped photos still return as single-item groups.
- Output is deterministic for the same input set.

### Phase 2: Technical Culling Signals

- Add image-level technical metrics in the backend.
- Start with cheap and robust signals:
  - sharpness / blur
  - exposure sanity
  - highlight / shadow clipping approximation
  - noise estimate
- Store metrics in Chroma metadata or a dedicated culling result structure.

Acceptance criteria:

- Clearly blurred or badly exposed images rank lower than clean alternatives in the same group.
- Metrics can be logged and inspected during tuning.

### Phase 3: Face-Aware Culling

- Reuse existing face detection infrastructure.
- Add face-level quality checks when faces are present:
  - face sharpness
  - eye openness / blink
  - face size / prominence
  - occlusion / poor visibility where feasible
- Aggregate face signals into image-level culling fields.

Acceptance criteria:

- In portrait or group-photo bursts, images with sharper open eyes rank above blink shots.
- Photos without faces fall back cleanly to generic technical ranking.

### Phase 4: Relative Ranking per Group

- Define the first `cull_score` formula.
- Rank only inside each group first.
- Mark the top image as `group winner`.
- Mark bottom images with strong negative signals as `reject candidates`.
- Keep weights configurable in code for fast tuning.

Initial weight suggestion:

- `40%` technical quality
- `35%` face-aware quality when faces exist
- `15%` relative framing / composition proxy
- `10%` aesthetic prior

Acceptance criteria:

- Every group has a stable winner.
- Ranking reasons can be explained from stored sub-scores.

### Phase 5: Plugin Workflow

- Add a dedicated Lightroom task, for example `Cull Similar Photos`.
- Support scopes:
  - selected photos
  - current view
- Provide conservative output options:
  - `Picks`
  - `Alternates`
  - `Reject Candidates`
  - optional `Duplicates / Near Duplicates`
- Create collections and switch the user to the result collection set.

Acceptance criteria:

- A photographer can run culling on a selection and immediately review grouped results in Lightroom.
- No catalog metadata is overwritten unless explicitly requested.

### Phase 6: Explainability and Tuning

- Surface short explanations for each winner or reject.
- Example reason codes:
  - `sharpest_in_group`
  - `eyes_closed`
  - `face_blur`
  - `better_exposure_available`
  - `near_duplicate_weaker`
- Add debug logging or an internal diagnostics mode for score inspection.

Acceptance criteria:

- Ranking decisions are explainable enough to debug and improve.
- Thresholds can be tuned without redesigning the system.

## Plugin / API Changes

Recommended new API endpoint behavior:

- keep `/group_similar` as the technical grouping endpoint
- add a higher-level culling endpoint later, for example `/cull`
- return structured groups, winners, alternates, and reject candidates

Recommended plugin additions:

- new task file for culling workflow
- collection creation helper shared with search / people workflows
- optional review dialog for thresholds and output mode

## Suggested Implementation Order

1. Finish `group_and_sort_images(...)`
2. Add technical metrics and per-group ranking
3. Add Lightroom collection workflow
4. Add face-aware ranking
5. Add explanations and diagnostics
6. Evaluate a small aesthetic model as secondary signal

## Explicit Non-Goals for First Release

- no LLM scoring in the hot path
- no full personalized taste model
- no genre auto-detection dependency
- no mandatory cloud service
- no automatic destructive reject action

## Evaluation Strategy

Before full rollout, create a small internal benchmark set with:

- weddings / events
- portraits
- family / kids
- travel / street

For each set, compare:

- best-shot accuracy within groups
- reject precision
- number of wrong winners
- user trust in explanations
- runtime and cost

## Open Technical Questions

- Which sharpness metric performs best on RAW-derived previews in this pipeline?
- Should blink detection be implemented through landmarks, eye aspect ratio, or a lightweight classifier?
- Should culling results live only in Chroma metadata, or also in Lightroom plugin properties?
- Should the first UX focus on `collection output only`, or also on an in-plugin review dialog?

## Concrete Todo Checklist

- [x] Implement similarity grouping backend in `services/chroma.py`
- [x] Define JSON response schema for grouped culling results
- [x] Add technical image metrics
- [x] Add face-aware culling metrics
- [x] Implement first `cull_score` weighting
- [x] Create Lightroom culling task
- [x] Create collections for picks / alternates / reject candidates
- [x] Add explanation fields and debug output
- [x] Build small benchmark dataset for evaluation *(harness + synthetic
      regression fixtures in `server-rs/testdata/cull_eval/`; real labelled
      shoots still wanted — see Signal-quality pass below)*
- [x] Evaluate optional aesthetic model as secondary signal *(CLIP-IQA over the
      stored SigLIP2 embedding, `lrg-ml/src/clip_iqa.rs`)*

## Branch Start Package

This is the recommended first work package for a dedicated implementation branch.

### Ticket 1: Finish grouping backend

Goal:
Implement the missing grouping foundation so culling can operate on similar-image stacks instead of isolated photos.

Scope:

- implement `group_and_sort_images(...)` in `server/src/services/chroma.py`
- combine `capture_time`, embedding similarity, and a cheap duplicate signal
- return stable grouped results for a provided list of photo IDs
- keep output deterministic and easy to debug

Suggested output shape:

- `group_id`
- `photo_ids`
- `group_type` such as `single`, `burst`, `near_duplicate`
- optional similarity/debug fields

Definition of done:

- the backend groups obvious bursts and near-duplicates reliably
- single photos still come back as one-item groups
- repeated runs produce the same grouping for the same input

### Ticket 2: Add technical culling metrics

Goal:
Create the first cheap, explainable ranking basis without LLMs.

Scope:

- add image-level metrics for:
  - sharpness / blur
  - exposure sanity
  - highlight / shadow clipping approximation
  - noise estimate
- store these metrics in backend metadata or a dedicated culling result payload
- expose the metrics in logs or debug output for tuning

Definition of done:

- clearly blurred or badly exposed images score worse than stronger alternatives
- metrics can be inspected per photo during development

### Ticket 3: Rank photos within each group

Goal:
Turn groups plus technical metrics into a usable first-pass culling result.

Scope:

- define the first `cull_score`
- rank only within each group
- mark `group winner`, `alternates`, and `reject candidates`
- add short reason codes derived from the score components

Suggested first reason codes:

- `sharpest_in_group`
- `blurred`
- `underexposed`
- `overexposed`
- `near_duplicate_weaker`

Definition of done:

- every non-empty group has a stable winner
- weak images can be flagged without deleting anything
- ranking reasons are reproducible and understandable

### Ticket 4: Add Lightroom culling task

Goal:
Make the backend result usable in Lightroom without changing existing metadata workflows.

Scope:

- add a new plugin task such as `Cull Similar Photos`
- support `selected photos` and `current view`
- call the grouping / culling API
- create result collections:
  - `Picks`
  - `Alternates`
  - `Reject Candidates`
  - optional `Duplicates / Near Duplicates`

Definition of done:

- a user can run culling on a selection and immediately inspect the result in collections
- no destructive action happens automatically

### Ticket 5: Add face-aware ranking

Goal:
Improve culling quality for portraits, weddings, events, and family photography.

Scope:

- reuse existing face detection
- add face-level signals:
  - face sharpness
  - eye openness / blink
  - face prominence
  - simple visibility / occlusion heuristics
- fold these into the `cull_score` when faces exist

Definition of done:

- in people-heavy bursts, sharp open-eye shots are preferred over blink shots
- photos without faces still rank correctly using generic signals

## Recommended Branch Order

If implementation happens in a separate branch, use this order:

1. Ticket 1
2. Ticket 2
3. Ticket 3
4. Ticket 4
5. Ticket 5

## Optional Nice-to-Haves After MVP

- lightweight aesthetic model such as `NIMA` or a CLIP-based aesthetic predictor
- user-adjustable presets for `portrait`, `event`, `action`
- in-plugin review dialog for thresholds and debug explanations
- learning from user keep/reject feedback

---

## Current state after the performance & quality pass

### What was actually wrong

**`/cull` was never the bottleneck.** It does no image I/O and no inference —
it reads stored metrics out of `IMAGE_TABLE` and ranks them. The wall was the
mandatory Analyze & Index pass, which computes two things culling never reads:
the SigLIP2 embedding (~316–480 ms/photo, the single largest cost) and LLM
keywords/caption (seconds). A wedding import was 30–60 minutes before culling
could start at all.

**Grouping did not scale.** `group_and_sort_images` compared every pair with a
1152-dimensional f64 cosine and recomputed both L2 norms inside that loop. The
only early-exit required a pair to have *neither* an embedding nor a pHash,
which never happens for indexed photos.

**Most preset knobs were dead code.** Every image and face tunable existed
twice — once in `culling_config`, where presets set it, and again as a private
`const` next to the algorithm, which is what got read. No preset could move any
image or face metric. `grouping.time_window_default_seconds` was dead the same
way, so `event`'s 2s and `sports`'s 3s burst windows did nothing.

### Measured results

Grouping, via `cargo run --release -p lrg-analysis --example bench_grouping`
(synthetic bursts of 8 frames, 30s between bursts):

| photos | before | after | speedup |
|---|---|---|---|
| 500 | 544 ms | 10.8 ms | 50× |
| 2000 | 8.85 s | 45.9 ms | 193× |
| 5000 | 56.1 s | 114.7 ms | 489× |
| 10000 | — | 236 ms | — |

Throughput was collapsing quadratically (919 → 226 → 89 photos/s) and is now
flat at ~43k photos/s. At 5000 photos the old path burned 56s of the plugin's
300s request budget.

Per-photo cull signals, via
`cargo run --release -p lrg-imaging --example bench_cull_metrics`: 39 ms for
`culling_metrics` plus 12 ms for `perceptual_hash` on a 2048px frame. These now
run in parallel across the batch (rayon, inside `spawn_blocking`) instead of
inline in a sequential loop.

### Landed

- **`tasks=cull`** — a cull-only ingest computing pHash, image metrics and face
  quality, skipping the embedding, the LLM, FaceNet and face thumbnails.
  `FacePass::QualityOnly` writes no `FACE_TABLE` rows and leaves `faces_checked`
  unset, so person clustering is untouched and a later `faces` run still does
  the real pass.
- **Sliding-window grouping** — exactly equivalent, not approximate: both edge
  predicates already required the time gap to fall inside the duplicate window,
  and records are already sorted with timed ones as an ascending prefix. L2
  norms hoisted out of the pair loop; a matching pHash short-circuits the
  cosine.
- **Config unification** — `ImageMetricsConfig`/`FaceMetricsConfig` moved to
  `lrg-imaging::cull_config` (below both `lrg-ml` and `lrg-analysis`) with a
  single definition presets actually reach. Note the split: threshold-shaped
  fields are baked into stored sub-scores at index time and need a re-index;
  weight-shaped fields apply at rank time and move per run.
- **Group-shot eye aggregation** — was `max` (best face), so one open pair of
  eyes among nine blinks scored as flawless. Now the worst *prominent* face,
  gated at 25% of the largest face's area so a bystander cannot veto a frame.
- **Pre-flight** — the plugin checks `/index/check-unprocessed` with
  `tasks=cull` and offers to prepare missing photos. Previously unindexed
  photos were dropped silently and the user saw "No groups found".
- **Warning correctness** — the "SigLIP model not loaded" dialog tested model
  *residency in RAM*. SigLIP idle-unloads after 30 minutes, so every cull run on
  an idle server claimed visual grouping was disabled while it worked fine. It
  now inspects stored embeddings.
- **`debug` is opt-in** (`include_debug`) — it was O(k²) floats per group,
  always serialized, never read by the plugin.
- Contract tests for `/cull` and `/group_similar`, which had none.
- Repo tooling: `sync_translations.py` hardcoded an absolute path to one
  developer's machine and wrote UTF-8 over UTF-16 files, so running the
  documented command silently corrupted all three translations for Lightroom.

### Still outstanding

Ordered by value. Items 1, 2, 4, 5, 6 and 10 of the original list are done —
see [Signal-quality pass](#signal-quality-pass) below, and
[Moment scoring](#moment-scoring-for-action-work) for the follow-up that came
out of a real complaint about a soccer series. Item 2 of *this* list (the
noise estimator) is done too — see
[Flat-region noise and the Lightroom workflow](#flat-region-noise-and-the-lightroom-workflow).

1. ~~**A real eye-state classifier.**~~ **Done** — see
   [Eye state with MediaPipe](#eye-state-with-mediapipe). `blink_penalty` was
   exactly `1 - eye_openness`, and the underlying signal a mean vertical
   gradient in a patch clamped to ≤8px radius that stopped scaling with face
   size.
2. **A noise estimator that does not compete with sharpness.** The current one
   is a 3×3 box-blur residual, so real detail reads as grain: measured on two
   frames of one scene differing only in focus, it scores the *sharp* frame
   0.082 and the blurred one 0.012. That is why noise was deliberately left out
   of the group-relative pass (see below) — normalising a signal that points the
   wrong way only amplifies it.
3. ~~**Native-resolution eye-region sharpness.**~~ **Done** — see
   [Eye focus at full resolution](#eye-focus-at-full-resolution). Tiled
   sharpness fixed the shallow-depth-of-field failure, but eye-vs-nose focus —
   the most common portrait cull decision there is — was not resolvable,
   because face sharpness was measured on the 2048px working image rather than
   a native-resolution crop around the eye.
4. **Job/progress/cancellation.** `/cull` is still one blocking request with a
   binary 0→100% progress bar. `JobRegistry` has no progress field, no
   cancellation, and destroys a finished job on first read.
5. **Genre auto-detection.** Zero-shot classification against the existing
   SigLIP2 embeddings is a dot product. More importantly, genre must change
   *which signals are trusted*, not just their weights. The preset system is now
   closer to being able to express that — `sharpness_peak_weight`,
   `relative_normalization_weight` and `sets.enabled` are all trust-shaped
   rather than weight-shaped — but nothing selects a preset automatically.
6. **Hardware acceleration.** CPU is the only default execution provider.
   CoreML is macOS-only and env-gated behind `LRG_ML_EP=coreml`; the face
   sessions have no EP path at all. No CUDA, no DirectML. Both ONNX models are
   batch-size 1 behind a `Mutex`.
7. **Real labelled fixtures.** The evaluation harness exists and runs in CI, but
   every fixture in `server-rs/testdata/cull_eval/` is synthetic and was written
   by the same person who wrote the code it scores. They are regression tests for
   known bugs, not a benchmark. One real hand-culled shoot per genre would be the
   single highest-value contribution to culling quality available; see that
   directory's README for the format.

---

## Signal-quality pass

Covers outstanding items 1, 2, 4, 5, 6 and 10 of the previous list.

### Landed

- **Subject-region sharpness** (`lrg-imaging::metrics`). The frame-wide
  Laplacian variance answers "is this frame busy?", not "is anything in focus?",
  so an f/1.4 portrait or a 400mm wildlife frame scored *below* a mediocre f/8
  frame of foliage. The pass now also accumulates per-tile variance over an 8×8
  grid in the same loop, yielding `cull_sharpness_peak` (sharpest tile),
  `cull_focus_concentration` (how localized the focus is — what separates
  *shallow* from *soft*), `cull_sharp_region_{x,y}` (where it is), and
  `cull_motion_anisotropy` (structure-tensor coherence, which labels a soft
  frame `motion_blur` rather than `blurred`; never scored, because directional
  *content* raises it too). Ranking blends peak into effective sharpness per
  `sharpness_peak_weight` — 0.70 for `sports`, 0.55 for `portrait`, 0.25 for
  `street`.
- **Intentional-set protection** (`lrg-analysis::sets`). Brackets, focus stacks
  and panoramas are detected and ranking's usual output is suppressed: ordering
  still happens so the set has a representative frame, but nothing is ever
  nominated for rejection. Surfaced as `keep_all` / `intentional_set` on each
  group and a dedicated Lightroom collection.
- **Aesthetics** (`lrg-ml::aesthetic`). *Superseded:* this first used five
  CLIP-IQA antonym prompt pairs over the SigLIP2 embedding. It is now the
  AestheticSigLIP score stored with each photo at index time, scaled to 0..1
  and blended against the old heuristic at `aesthetic_model_weight` (0.9). A
  photo without a score from the current export ranks on the heuristic alone,
  and the cull says how many did, once.
- **Group-relative exposure.** The absolute target of mean luminance 0.5 marks
  every frame of a low-key set as badly exposed and then cannot tell them apart.
  Exposure is now blended toward its position in the group's own range
  (`relative_normalization_weight`, 0.65 for `event` and `sports`).
- **`cull_occlusion` measures the image.** It was
  `1 - (0.55·det_score + 0.20·center_proximity + 0.25·eye_openness)` — three
  numbers the caller already had, recombined, so it was collinear with detector
  confidence and `reject_occlusion_threshold` was a second confidence gate. It
  is now a landmark-fit residual against the ArcFace template (a similarity
  transform absorbs position, rotation and scale, so only the face's *shape*
  changing moves it) plus mirror asymmetry on the aligned crop.
- **`cull_technical_score` is re-derived at rank time.** The stored composite is
  frozen at index time under the default config, so no preset weight and no
  later signal could reach it — despite the indexing code's own comment claiming
  otherwise.
- **Evaluation harness** (`lrg-analysis::eval`, `--example cull_eval`, fixtures
  in `server-rs/testdata/cull_eval/`, CI test `cull_eval_fixtures.rs`). Scores
  top-1 winner accuracy, reject precision/recall, NDCG, set preservation, set
  recognition and pair-counting grouping agreement, with `--compare` against
  `CullingConfig::python_parity` and `--ablate` for attributing a delta to one
  knob.
- **`exposure_bias` plumbed** from `LrPhoto`'s `exposureBias` through both index
  routes to `IMAGE_TABLE`, per photo rather than per batch.

### Two things only the live test found

Both were caught by running the real binary against real JPEGs, not by unit
tests, and both changed the design.

**A bracket's frames do not hash alike.** The first version of the detector
required all frames to share a pHash, as evidence the camera had not moved.
Measured against an actual +2 EV frame of an otherwise identical scene, the
Hamming distance to the base frame was **61 of 64 bits** — pushing the histogram
into the highlights flips which DCT coefficients sit above the median, so the
hash comes back near-complemented. The framing gate rejected every bracket it
existed to confirm, and worse, the grouper never put the frames in one group at
all (the embedding is absent on the fast cull path, and pHash was the only other
signal). Fixed by dropping the framing check from bracket detection in favour of
requiring *evenly spaced* exposure stops shot back to back — a pattern only AEB
produces — and by adding an explicit bracket edge to grouping
(`GroupingConfig::bracket_edges`). Focus-stack detection still checks framing,
where the exposure is constant and the hash is meaningful.

**Group-relative noise makes things worse, so it was dropped.** The plan called
for normalising exposure *and* noise against the group. Exposure benefits.
Noise does not: the estimator is a high-pass residual that already ranks a sharp
frame dirtier than a blurred one (0.082 vs 0.012 on two frames of one scene),
and rescaling to the group's range stretched that gap to 0.535 — an eightfold
amplification of a signal pointing the wrong way. The stated justification does
not survive either: a group shot entirely at high ISO is penalised *equally*,
and a constant offset cannot change a within-group ranking. Fixing the estimator
is now outstanding item 2.

### Measured on the fixtures

`cargo run --release -p lrg-analysis --example cull_eval -- testdata/cull_eval --compare`,
current configuration against `python_parity`:

| metric | before | after |
|---|---|---|
| winner top-1 | 66.7% | 100.0% |
| NDCG | 0.974 | 1.000 |
| reject precision | 71.4% | 83.3% |
| reject recall | 100.0% | 100.0% |
| set preservation | 100.0% | 100.0% |
| set recognition | 0.0% | 100.0% |
| grouping recall | 66.7% | 100.0% |

**These fixtures are synthetic and encode the failure modes this pass fixed, so
the table shows that the named bugs are gone — not that culling is accurate on
real photographs.** The 100% preservation in the "before" column is itself
instructive: the old code protected nothing, it simply failed to group the
bracket frames, and singletons are never nominated for rejection. That is why
preservation and recognition are separate numbers.

Reject precision at 83.3% is a known open cost, not an oversight: group-relative
exposure widens the score spread, which makes `reject_score_delta` fire on the
third frame of a three-frame low-key burst labelled "keep". `--ablate relative`
shows both sides of the trade.

---

## Moment scoring for action work

Prompted by a real complaint: on a soccer series, the picks and rejects did not
match the photographer's taste.

### Why that happens

Players are small in frame and YuNet runs on a 640×640 letterbox, so an action
burst usually detects **no faces** — which removes the entire face branch
(expression, eyes, blink) from the score. What is left is:

```
score = (technical + 0.08 × aesthetic) / 1.08
      = 46% sharpness + 32% exposure + 14% low-noise + 7.4% aesthetic
```

Within one burst of one play, exposure and noise barely move. So the winner is
decided almost entirely by which frame carries the most high-frequency detail —
crowd texture, grass, how far the player's limbs are spread. Nothing in that
formula knows whether the ball is at their foot.

The genre table in
[Dev-Image-Culling-Signal-Analysis.md](Dev-Image-Culling-Signal-Analysis.md)
says the sports pick is decided by *peak action*. No signal measured it.

### What shipped

- **`ACTION_PROMPT_PAIRS`** in `lrg-ml/src/clip_iqa.rs`, a second prompt set
  alongside the quality one, asking about visible facts rather than judgements —
  "the ball is clearly visible in the frame", "an athlete with both feet off the
  ground". `PromptSet` keeps the two sets separate because they are weighted
  separately; averaging them would let a pretty frame of nothing outrank a
  slightly softer frame of the goal.
- **`ranking.moment_weight`**, 0.30 for `sports` and **0 everywhere else**. The
  question is meaningless for a portrait, and a signal that fires on everything
  is worse than one switched off. Applied as a convex blend over the quality
  score, so a genuinely unusable frame cannot be promoted just because the ball
  is in it.
- **`peak_action` reason code** on the winner when the moment score is what
  carried it, so a surprising pick explains itself.
- The action pass only runs when the chosen preset would use it — otherwise the
  score would be computed and discarded.

### Extended to the other genres

The same mechanism now covers three axes, one per preset, selected by
`ranking.semantic_prompt_set`:

| preset | axis | weight | what it asks |
|---|---|---|---|
| `sports` | `action` (+ `sport_emotion`) | 0.55 (was 0.30) | ball visible, feet off the ground, mid-play vs between plays |
| `street` | `candid` | 0.32 | unposed vs posed, people reacting to each other |
| `event` | `candid` (+ `event_emotion`) | 0.45 (was 0.22) | the same — event work still has to be deliverable, which the usable gate now guarantees |
| `portrait` | `expression` | 0.20 | warm genuine smile vs flat awkward expression |
| `default` | — | 0 | spans every genre; no one question is right for all of them |

The emotion sets and the new weights are from
[Emotion and the usable gate](#emotion-and-the-usable-gate) below.

`street` carries the highest weight because it is the one genre the analysis is
explicit about: grain and motion blur are legitimate there, and ranking frames
by how tidy they are is close to the opposite of the job.

**One axis per preset, deliberately.** Blending two doubles the text-tower work,
dilutes both, and — with no validated fixture — is exactly the sort of
plausible-sounding over-reach that left a contrast heuristic in charge of
"aesthetics". If a genre needs two, demonstrate it on a fixture first.

**What was deliberately left out**, because CLIP answers it badly and something
else answers it well: sharpness and exposure (measured directly, far better);
eyes open / blink (fine facial detail is these models' weakest point, and this
needs a real classifier — a prompt pair would look like progress while changing
nothing); horizon level and verticals (geometric relations are a known blind
spot); animal eye sharpness (needs detection, not a scene judgement).

### Tuning it from outside

`/cull` accepts `semantic_weight` to override the preset's own, and the plugin
passes it through. (Until the emotion work it only decided whether the prompts
were scored; the ranking itself still used the preset's weight, so a sweep
changed nothing. It now reaches the ranking.) `0.0` switches the axis off, `0.5` lets it lead. The shipped
weights are a guess; this is the fastest way to find out whether the signal
suits a particular photographer's eye without a rebuild. `--ablate semantic` in
`cull_eval` does the same against a fixture.

### Calibrate expectations

CLIP-family models read object presence, coarse pose and affect well, and fine
temporal ordering and geometry poorly. The action axis should reliably separate
*the play* from *between plays*; it should **not** be expected to pick the single
peak frame out of five adjacent ones at 20fps. The expression axis is a
*whole-frame* judgement, so on a group shot it reports the mood of the picture
rather than of any one face. All of them need embeddings, so every axis is inert
on the fast `tasks=cull` path — a catalog prepared that way ranks exactly as it
did before. (Since the emotion work the plugin adds `embeddings` to the prep pass
for every preset that judges the moment, and the backend warns when the moment
could not be judged.)

**It has not been validated on real photographs.** That is not an oversight: the
prompts are a hypothesis, and this repository has now twice shipped a
plausible-sounding signal that was wrong. Which is why the other half of this
work is the fixture exporter.

## Emotion and the usable gate

Prompted by the next complaint: the biggest missing factor is **emotion**, at
sports and at events alike. The concept (four failure modes, phases, open
decisions) lives outside the repository; this is what Phase 1 built. No new
model: everything is answered from the stored SigLIP2 embedding.

### What shipped

- **Two emotion prompt sets**, `SPORT_EMOTION_PROMPT_PAIRS` (celebrating,
  protesting, despair vs calm and neutral) and `EVENT_EMOTION_PROMPT_PAIRS`
  (laughing, crying with joy, hugging, guests reacting vs blank faces), selected
  by `ranking.emotion_prompt_set`. Each pair is a complete question with the
  same subject on both sides, after the organism-set lesson that a partial pair
  only adds noise to the average.
- **The moment is the max, not the mean**, of the genre axis and the emotion
  question. The goal celebration has no ball in it and the shot on goal has
  nobody cheering yet; averaging would score both as half a moment. This is why
  it does not contradict "one axis per preset" above: nothing is diluted, the
  stronger answer stands.
- **All or nothing per group.** A frame without an embedding carries no moment,
  and comparing it with frames that do would rank it on a different question
  from its neighbours. Such a group ranks without the moment, and the response
  says so.
- **The usable gate** (`ranking.usable_gate`): once the moment is deciding,
  every frame without a clear defect ranks above every frame with one — first
  usable, then the moment. A clear defect is one no edit repairs: blur,
  unintended closed eyes, occlusion. Exposure is deliberately excluded (group-
  relative, and Lightroom lifts a stop without a trace), and so is the
  composite face score, which also falls for a face that is merely small. The
  gate is what lets the moment lead: `sports` now weights it 0.55 and `event`
  0.45, where the old convex blend had to stay small because it was the only
  thing stopping a ruined frame from winning.
- **Emotion beats the blink.** `EYES_CLOSED_INTENT_PROMPT_PAIRS` is a gate, not a
  grading axis: above `ranking.eyes_closed_intent_threshold` (0.7 at `event` and
  `portrait`) closed eyes stop counting as a blink — no blink penalty, no blink
  reject. It asks about the scene (a kiss, a burst of laughter), which is coarse
  affect and well within what these models read; it does not try to judge eye
  openness, which the section above rules out for good reason. The eye share
  baked into the stored `cull_face_score` stays.
- **Singles and whole bad series can be rejected**
  (`ranking.reject_blurred_without_alternative`). A single used to be a pick
  unconditionally, and so was rank 1 of any group. Now the pick is the
  best-ranked frame that is not a reject candidate, and at rank 1 (or for a
  single) only blur makes one: exposure is scored against a mid-grey target
  that marks a correctly dark concert frame as underexposed once there is no
  group to compare with, and the blink proxy is too weak to reject a frame
  without a better sibling. `winner_photo_id` is therefore nullable.
- **Reasons in plain language**: `strongest_emotion` when emotion carried the
  pick, `eyes_closed_intentional`, and `strong_moment_but_unusable` on a reject
  candidate that had its group's strongest moment ("look before deleting"),
  by at least `ranking.reason_moment_margin` (0.15) over the pick. The plugin
  counts the last one in its completion message.
- **Warnings that reach the dialog.** `/cull/grade` and `/cull/groups` return a
  `warnings` list (and the joined `warning` for older plugins): photos whose
  moment was not judged, or prompt vectors that were neither in the
  `text_embeddings` table nor obtainable from the text tower — one message for
  every signal that cause took out. The emotion and kiss-gate prompts are
  prepared together (formerly with a quality set, since replaced by the stored
  aesthetic score), in one text-tower pass the first time
  and from the database after that. `apply_prompt_scores` returns `None` for a
  set it could not score instead of 0.
- **The plugin prepares what the preset needs**: `Util.cullPrepTasks` adds
  `embeddings` to the prep pass for every preset but `default`, and the model
  check asks for the image model as well as the face model.

`python_parity()` switches the gate and the single/rank-1 rejects off, so the
Python goldens still pin the port.

### Live check (not a validation)

Run against the real SigLIP2 fp16 towers on six public photos, to catch
prompts that are plainly broken, not to tune anything:

| photo | `sport_emotion` | `action` | `event_emotion` | kiss gate |
|---|---|---|---|---|
| coach screaming on the touchline | **0.72** | 0.42 | 0.42 | 0.60 |
| player mid-shot, calm face | 0.31 | **0.87** | 0.23 | 0.59 |
| couple on a red carpet, neutral | 0.12 | 0.34 | 0.28 | 0.35 |
| studio portrait | 0.37 | 0.42 | 0.45 | 0.59 |

The sport emotion question separates the screaming coach from everything else,
and the max picks the action question for the shot instead: both frames won
their group with the right reason (`strongest_emotion`, sharpest). No photo
here contains a kiss or a laugh, so the gate's 0.35–0.60 only shows it does
not fire on neutral faces; 0.6 to 0.7 is a thin margin. One finding changed the
code: a Gaussian-blurred copy of the player scored 0.95 on action against the
original's 0.87, which would have marked the blurred copy as "strongest moment,
look before deleting". Hence the margin above.

### Not validated yet

Same caveat as the moment axes, and the same remedy: the prompts, the 0.7
threshold and the 0.55/0.45 weights are hypotheses. The next step is a sports
and an event shoot culled by hand, exported with *Export Culling Fixture*, and
scored with `cull_eval` with and without `--ablate semantic`. Phase 2 of the
concept (per-face expression, an eyes-closed classifier, focus on the eye, main
subjects) waits on what that measurement shows.

### Export Culling Fixture

New Lightroom task (`TaskExportCullFixture.lua`, *Library → Plug-in Extras*).
It turns a hand-culled selection into a scoring fixture: reject flags become
`reject`, pick flags and star ratings become the ranking, and the metrics come
from a real `/cull` call with the new `include_stored_metadata` flag.

Two details that matter:

- **`stored_metadata`, not `metrics`.** The ranked `metrics` block is what
  ranking concluded — short names, derived values, preset weights folded in. The
  new block is what it read, under the store's own keys. Only the second
  reproduces a run.
- **`groups_are_authoritative`.** Unless the photographer stacked the photos, the
  group boundaries come from the backend itself, and scoring them would compare
  the grouper against its own answer and report 100%. The harness now skips the
  grouping metrics for such a fixture rather than reporting a flattering number.

No photographs leave the machine — a fixture holds measurements, capture times,
hashes and opaque ids.

## Flat-region noise and the Lightroom workflow

Two gaps closed in one pass: the one signal that was known to point the wrong
way, and the fact that a cull ended in collections rather than in the flags a
photographer actually culls with.

### `cull_noise_flat`

Outstanding item 2 above. The frame-wide estimate is the mean 3×3 box-blur
residual over mid-tone pixels; detail is high-pass energy too, so it scored a
sharp frame 0.082 and the blurred frame of the same scene 0.012 and could
never be normalised against the group. `lrg_imaging::metrics` now also
accumulates the residual only where the **smoothed** image has no gradient
(`noise_flat_gradient_threshold`, 0.02 per pixel), so texture and edges are
left out and what remains is tone plus noise. The gradient is taken on the
smoothed plane rather than the raw one so the noise under test cannot push
its own pixels out of the mask. When less than `noise_flat_min_fraction` of
the interior is flat (a frame that is texture edge to edge) the field equals
`cull_noise`, so a row always carries it.

Stored as `cull_noise_flat`, normalised by `noise_flat_denominator` (0.04,
lower than the frame-wide 0.08 because a residual measured only on smooth
pixels is a smaller number). `rank_group_records` reads it in preference to
`cull_noise` — only when every frame of the group has it, since the two
estimators answer different questions and a group indexed half before and
half after must not compare one frame's grain with another's texture — and
reports the frame-wide value alongside as `noise_frame_wide` so a
disagreement is visible. `cull_noise` itself is unchanged, bit for bit,
so the imaging goldens still pin the Python port. Unit tests cover the three
cases that matter: two frames of one scene differing only in focus score the
same flat noise while the frame-wide value still prefers the soft one; grain
added to either raises it; and a frame with nothing flat falls back.

Rows indexed before this shipped have no `cull_noise_flat` and rank on the
frame-wide number until re-indexed — the same migration story as
`cull_sharpness_peak`.

### `reject_margin_scale`

`/cull/grade` takes `reject_margin_scale`, a factor on the preset's
`reject_score_delta`, clamped to 0.25..4. The presets say what a good frame
of a genre looks like; how many near-misses to keep is the photographer's
call and changes from job to job (one frame per burst for a client gallery,
three for an album edit), so it is a dialog setting — **Cull strength**:
gentle 1.5, balanced 1.0, strict 0.6 — rather than a preset. It moves only the
score-margin rule. The defect rules (blur, blink, occlusion, exposure) fire at
their usual thresholds, because a blink is a blink however gentle the cull.
A contract test pins the ladder on the seeded three-frame burst: 0 rejects at
the preset margin, 1 at half, 2 at a quarter.

### Flags, stars and labels

The plugin can now write the result into the catalog the way a photographer
would by hand, so the filter bar, the flag shortcuts and *Delete Rejected
Photos* work on it: Pick flag on winners (default on), Reject flag on reject
candidates (default off — one menu command from deleting files), 3/2 stars for
picks/alternates (default off), and a purple label on
`strong_moment_but_unusable` rejects (default on). All of it in one
`withWriteAccessDo`, so one undo step.

The piece that makes a re-run safe is `Util.cullFlagPlan`: a pure function
over the backend result, what the catalog holds now (`batchGetRawMetadata`
for `pickStatus`, `rating`, `colorNameForLabel`) and what the last cull wrote
(the `cullWritten` plugin field, `flag=1 rating=3 label=purple`, written in
the same undo step as the marks). A mark equal to what the cull wrote is the
cull's: it is refreshed to this run's decision or cleared when the decision
no longer calls for it, so a re-run is never blocked by its own earlier
output. Any other mark is the photographer's and is left alone and counted —
unless *Also overwrite flags, stars and labels I set myself* is on, which is
the only way a user mark is ever changed. (The first version keyed on "has a
flag" alone, which could not tell the two apart: re-runs were blocked by
their own picks and overwrite mode cleared the user's flags; review caught
it.) `keep_all` groups are never touched. Every `with*WriteAccessDo` return
value is checked, since a catalog that cannot be locked returns `"aborted"`
without throwing. It is specced in `plugin/spec/util_cull_spec.lua`; the
task only applies the plan and reports the counts.

Result collections are now filled in score order (picks best first, rejects
worst first), which Lightroom shows under Custom Order sort, and the burst
window gained an `Auto (from preset)` choice that leaves `time_delta_seconds`
out of the request so the preset windows — dead for a long time, then
restored on the backend — finally reach a cull started from the dialog.

## First real shoot

The first cull of a real shoot — a baptism, 797 NEF frames, Event preset at
the gentle strength — on the day the Lightroom workflow landed. It did what
the synthetic fixtures could not: it showed where the signals meet real
photographs. The numbers, then what changed.

| what the run said | number |
|---|---|
| groups | 155 (one of 123 frames, others of 45, 44, 40, 28) |
| picks / alternates / reject candidates | 103 / 27 / **667** |
| reject candidates with `clear_defect` | 666 of 667 |
| `possible_blink` on rejects | 653 |
| `possible_occlusion` on rejects | 567 |
| frames with faces whose `blink_penalty` exceeded the Event reject threshold | 767 of 769 |
| "focus stacks" found, frames kept | 7, 27 — every one a group of 3 to 10 people |
| `time_span_seconds` of every group | 0 |

**The fast prep path sent no capture time.** `TaskCullPhotos.lua`'s
`indexOptionsFor` forwarded `exposure_bias` and the filename and nothing else,
so every photo it prepared was undated to the backend, the sliding-window
grouper treated the whole selection as the "undated suffix" and matched by
hash and embedding alone, and a ceremony of similar frames chained into one
group of 123 with a single pick. Fixed three ways, so it cannot come back:
the prep path now sends `date_time`/`date_time_unix` like Analyze & Index
(and the single-photo by-reference wrapper forwards them, which it never did),
the backend falls back to the file's EXIF `DateTimeOriginal`
(`lrg_imaging::capture::read_capture_time`, with `SubSecTimeOriginal` for the
fraction — at 20–30 fps a burst shares one whole second — and
`OffsetTimeOriginal` so the value is a real epoch, as `local_hour` assumes)
when the plugin sends nothing,
and `/index/unprocessed` reports a cull-prepared photo without a
`capture_time` as still needing preparation — once, since a photo that has
none anywhere is stamped `capture_time_unknown`.

**The face proxies fire on everything.** Eye openness read 0.07–0.26 across
the whole shoot: the patch it is measured on is clamped to 8 px and the faces
at a ceremony are small, so every frame "blinked", most were "occluded" and
nearly all had a face score under the reject threshold. Each absolute rule was
individually reasonable; together they made every non-pick of every burst a
clear defect. The fix is not a new threshold — no threshold is right for a
proxy that cannot see the eye — but `ranking.relative_defect_rules`: blink,
occlusion and weak-face now also have to be clearly worse than the group's
best frame (`relative_defect_margin`, 0.10) before they count. A proxy that
reads the same on every frame of a burst then rejects nothing, while a frame
that reads clearly worse than its siblings still does. The outstanding
eye-state classifier (item 1 above) remains the real answer; this makes the
proxy honest about what it can and cannot distinguish in the meantime.

The second run, with capture times in place, showed the same shape one level
down: 468 of 770 frames still rejected at gentle, 217 of them `blurred`.
Sharpness inside one burst spans 0.01–0.10 (median 0.03), so an absolute
0.20 threshold rejects a burst whole or not at all; and occlusion moves by
0.1–0.35 inside one group of people (median 0.11) as heads turn, so the
shared 0.10 margin fired in most multi-face groups. Blur therefore joined the
relative rules — a frame with siblings is a blur defect when clearly softer
than the sharpest of them, a single or a burst whose sharpest frame is itself
soft keeps the absolute test, so a burst that missed focus throughout is
still rejected whole — and occlusion got its own margin,
`relative_occlusion_margin`. **Both are held pending calibration**, at the
maintainers' request: `relative_blur_rule` defaults to off and the occlusion
margin to the shared 0.10, so shipped behaviour is that of the second run.
The measured candidates (blur on, occlusion 0.20) gave 415 of 770 at gentle
and are what `cull_eval --ablate` should test once a hand-culled fixture
exists for more than one genre.

**People are not focus stacks.** Static framing plus a wandering sharp region
is how a stack is recognised, and it is also what a group of people shifting
their weight between frames looks like. `SetFrame::faces` now carries
`cull_face_count`, and a group with a face in any frame is never a stack.

The first run's flag writing, undo, respect-existing protection and
collections all behaved as designed: 96 picks flagged (103 picks minus the 7
"stacks", which are never flagged), 49 purple labels, 0 rejects (as
configured), confirmed by reading the catalog. The second run (830 frames,
respect-existing off) grouped sanely — 192 groups, the largest 23 frames,
burst windows of 1–12 s, no "stacks" — rewrote the flags (101 picks set, 7
earlier picks unflagged, 22 labels) and brought rejects from 84% to 61%; the
blur and occlusion changes above are measured against that run's stored
metrics, not yet against a hand cull. **None of these margins has been
fitted to the photographer's own picks.** That is what *Export Culling
Fixture* on this shoot is for, and it is the next step.

### Also fixed on the way past

`sync_translations.py` had two bugs that were corrupting shipped UI strings:

- Its LOC regex used `[^"\']+`, so **every default containing an apostrophe was
  truncated at it** and the truncation was written to all three translation
  files. `"Create 'Duplicates / Near Duplicates' collection"` shipped as
  `"Create "`, along with a dozen others.
- The writer escaped `"` as `\"` and the reader never unescaped, so backslashes
  **accumulated on every run** — one string had reached sixteen.

Both fixed, English re-derived from the Lua source (which is what "sync" should
mean and previously did not), the damaged German and French strings retranslated,
and the round trip is now idempotent.

## Eye focus at full resolution

Outstanding item 3, prompted by the question whether culling checks focus on
faces and eyes at all. It checked faces only coarsely — a Laplacian variance
over the whole face box on the ≤2048 px working copy, 35% of the composite face
score, never a defect of its own — and eyes not at all. The commercial culling
tools all treat the two as separate criteria (Lightroom's own Assisted Culling
has a *subject focus* and an *eye focus* threshold, Excire grades face and eye
sharpness separately, Narrative scores each face's focus out of 10).

### What shipped

- **A focus measure that compares frames, not scenes.**
  `lrg_imaging::focus::blur_effect` is the no-reference blur effect of
  Crété-Roffet et al. (2007), ported from `skimage.measure.blur_effect` and
  pinned against it (`testdata/focus_goldens.json`, made by
  `testdata/make_focus_goldens.py`): re-blur a patch with a box filter and see
  how much of its gradient energy survives. Being a ratio of the patch's own
  energies it depends far less on content and contrast than a Laplacian
  variance (half the contrast moves it by under 0.01), which is what a burst
  comparison needs. Focus is `1 - blur_effect`.
- **Measured on the full-size original.** Detection stays on the working copy,
  so YuNet, the stored boxes and the identity embeddings do not change. The face
  step then decodes the original once more — only for a photo with faces —
  through `convert::decode_oriented`, which shares its decode and orientation
  code with `normalize_image_bytes`, so the working copy is that image scaled by
  one factor and boxes map across exactly (`lrg_ml::faces::refine_focus`).
  By-path the file is read again; an upload is kept until the face step. At most
  two full-size decodes run at once (`FULL_DECODES`).
- **Per face**: `eye_focus` (the sharper eye, a square of 0.5 × the interocular
  distance around each YuNet eye point), `face_focus` (the face box), `eye_px`.
  **Per photo**: `cull_eye_focus`/`cull_face_focus`, the least sharp prominent
  face (same prominence gate as the eye-openness aggregate), absent when none
  could be measured, and `cull_focus_version` so "too small" is told apart from
  "indexed before this existed".
- **Ranking** (`RankingConfig::eye_focus_weight` and the
  `*_eye_focus_threshold`/`*_face_focus_threshold` pairs): eye focus takes a
  share of the face term, scored against the group; `eyes_out_of_focus` (or
  `face_out_of_focus` where the eyes are too small) is a label below the reason
  threshold and a clear defect below the reject threshold — both only with a
  sibling and only `relative_focus_margin` behind the sharpest. A single is never
  rejected for focus. All or nothing per group, like the moment. Portrait 0.55,
  default and event 0.35, sports 0.20, street off.
- **The explanation's `eye focus N/10 within the group`** is relative: 10 for
  the sharpest eyes of the group, one point less per 0.03 below them, and no
  number for a frame with nothing to compare against. See the calibration
  below for why an absolute grade would mislead.
- **Cost**: about 0.24 s more per photo *with faces* for a 24-megapixel JPEG
  original in release (0.21 s to decode it again, 0.03 s to measure the face and
  both eyes; `examples/bench_cull_metrics.rs` on a 4-core container), nothing
  for a photo without faces.
- **Migration**: `/index/unprocessed` re-queues cull-prepared photos with faces
  and no `cull_focus_version`; the cull pass then re-runs the face step even on a
  photo whose faces are checked (writing no identities). `/cull/grade` warns
  about photos still without it.

### Calibration — and its limits

On the reference portrait (`testdata/astronaut.jpg`, interocular distance 45 px)
the eye reads 0.69 sharp, 0.60 / 0.53 / 0.47 / 0.39 after a Gaussian blur of
sigma 1 / 1.5 / 2 / 3; the whole face 0.55, 0.43, 0.36, 0.30, 0.22. Blurring
only the eyes moves the eye reading from 0.69 to 0.53 and the face reading not at
all — the nose-versus-eye case. The same eye on a half-size copy (a 13 px patch)
reads 0.86 sharp and 0.80 at sigma 2: the measure saturates once the re-blur
window covers the eye, which is why `eye_focus_min_iod_px` is 40 and why the
original matters. The absolute level also drifts with patch size (0.69 at 23 px,
0.80 at 15 px), and with how sharp the image is *per pixel*: in the live test,
a 6× enlargement of the same portrait (3072 px, so the plumbing ran on a real
full-size decode) read 0.39 on its sharp eye — sharp at viewing size, soft at
100 %, as a high-resolution sensor can be too. The ranking was right all the
same (sharp eyes first; focus on the nose 0.09, `eyes_out_of_focus` and a
reject; the frame blurred all over `blurred`), because frames of one burst share
camera, lens, ISO and magnification and differ in focus. So the group
comparison carries the decision, the thresholds are a backstop, the display is
relative, and eye patches are capped at 128 px (`eye_focus_max_patch_side`) so
a very large eye is judged at a size where focus, not sensor softness,
dominates. **All thresholds are hypotheses** until a hand-culled
portrait shoot has been scored with `cull_eval --ablate eye-focus`; the synthetic
`portrait.json` group `p3` (eyes / nose / behind the head) only shows the
mechanism works.

### Known gaps

- A raw whose embedded preview is no larger than the working copy (some Fuji,
  Panasonic and older Sony bodies) gains nothing; it is measured at what the file
  offers, silently, since the user cannot change it.
- An upload (remote backend) is the plugin's export at `prefs.exportSize` and
  JPEG quality `prefs.exportQuality` (default 50); block artefacts at low quality
  read as detail.
- High-ISO noise reads as detail too. Within one burst the ISO is the same, so
  the comparison holds; across bursts the absolute numbers do not.
- Eye *state* is still the gradient proxy (outstanding item 1). With the eyes
  now measured at full size, the classifier that replaces it can work on the
  same crops.

## Eye state with MediaPipe

Outstanding item 1, and the second package of the commercial-criteria work:
Lightroom's Assisted Culling has *Eyes Open* with a "Can't Tell" state,
Narrative marks blinks red and squints yellow per face, Aftershoot and Excire
read expressions. The proxy could do none of that — on the first real shoot it
read "closed" on 767 of 769 frames.

### What shipped

- **Models.** MediaPipe Face Mesh V2 (478 landmarks, iris included) and
  Blendshape V2 (52 coefficients), Apache-2.0 per both model cards, whose only
  out-of-scope uses are life-critical decisions and identifying people.
  `scripts/export_face_state.py` takes them from the pinned
  `face_landmarker.task`, converts them with tf2onnx and checks them against
  the TFLite originals (≤ 4e-4 px on landmarks, ≤ 1.3e-6 on blendshapes). Own
  family `face_state` (~7 MB, `face-state-assets-v1`), so face detection never
  depends on it.
- **Pipeline** (`lrg-ml/src/face_state.rs`), with every constant taken from
  MediaPipe's FaceLandmarker sources: a square region 1.5× the face rotated to
  level the eyes, 256 px, `0..1`, border replicated; once from YuNet's box and
  eyes, once more from the mesh's own landmarks (MediaPipe's tracking mode,
  the mode its model card says the mesh is accurate in); 146 landmarks in image
  pixels into the blendshape model. Against Python MediaPipe on the reference
  portrait (`tests/face_state_goldens.rs`): landmarks within ~1% of the
  interocular distance, blink and smile within 0.01; the closed-lid path
  exactly, by moving MediaPipe's own upper-lid landmarks onto the lower ones.
- **Per face**: `eye_closed` (`eyeBlink` of the more closed eye, only the near
  eye from 35° of head turn), `eye_state` open/half/closed/unknown (unknown below
  0.5 presence, beyond 60° of yaw, or with the eyes under 24 px apart), smile,
  squint, `gaze_off_lens` in degrees, head pose (yaw, pitch, roll) and all 52
  blendshapes, so that a later signal need not decode every original again.
  Faces under 64 px are not run — the model card warns against rescaling them.
  Prominent faces only (the aggregation's gate), at most 12, on the full-size
  original from the focus step when there is one.
- **Per photo**: the most closed prominent eyes, the weakest smile, the
  strongest squint on a face not smiling, the furthest gaze among open eyes, and
  `cull_face_state_version`, whose value counts: an older one is analysed again.
- **Ranking**: where the group has the version throughout, the model's reading
  becomes blink penalty and eye openness, judged at its own thresholds (closed
  > 0.55, label > 0.5; sports 0.68/0.6, street 0.72/0.62, portrait 0.5/0.45);
  `eyes_closed`, `eyes_half_closed` (label only), `looking_away` (portrait),
  `best_smile`; a smile of 0.6 or more with the eyes shut is a laugh; smile
  weight 0.15 portrait, 0.10 event. An unknown face is never a blink.
- **Migration**: re-queued by `/index/unprocessed` once the model is on disk;
  `/cull/grade` names the fix that applies, "Prepare now" or "Download AI
  models".

### Calibration — and its limits

There is no photograph with closed eyes under a free license in the
repository, so the closed path is pinned synthetically (lids moved shut:
0.81/0.76) and the open path on the reference portrait (0.04/0.03).

Those two points were a poor guide. Measured with `examples/face_state_probe.rs`
on portraits from Wikimedia Commons (not committed; 285 usable faces to choose the
thresholds, judged by eye, then 292 from a disjoint sample to check them):

| Reading | On the first sample | On the second sample |
|---|---|---|
| Eyes shut or lowered, caught by the first rule (mean ≥ 0.6) | 6 of 22 | about 8 of 24 |
| The same, caught by the more closed eye > 0.55 | 19 of 22 | about 17 of 24 |
| Faces with the eyes open and a reading > 0.5 | 0 of 213 | 1 of 178 portraits |
| Labelled `looking_away` in portraits, old (`eyeLook` > 0.5) → new (> 25°) | | 25–31% → 10–12% |
| The same in Commons' *People looking away* | | 53% → 67% |

- **`eyeBlink` tops out low.** Eyes shut read 0.54–0.78 on the more closed eye and
  0.41–0.72 as a mean. No open eye reached 0.35.
- **`eyeLook*` is the eye inside the head.** People turned 25–30° and looking
  straight at the camera read 0.85–0.99, and a head turned 39° with the eyes
  following it 0.14, so the old reading had it backwards. Added to the head's turn
  at 28° per unit (where the two cancel for people looking back at the lens) it
  separates well: portraits a median 9°, people looking away 31°. Up and down is
  weaker: `eyeLookUp` barely moves when a lowered head looks up at the lens, so
  selfies from above read about 20°. That is why the label sits at 25°. Shut eyes
  abstain: their `eyeLook*` is noise, 31° on one such portrait.
- **The head pose** is a frame of bone landmarks, within about 1° of a full
  Procrustes fit to MediaPipe's canonical face model on yaw and roll and 2° on pitch.
- **`eyeSquint` runs high for everyone** (median 0.43) and rises on a smile, so it
  counts only from 0.78 on a face smiling under 0.3. It fired on no face of the
  second sample; it adds little beyond the lids, and no false alarms.
- **Several coefficients do not move on photographs at all**: `cheekSquint`,
  `noseSneer`, `cheekPuff` and `jawForward` stay at 0.00 even at the 99th percentile,
  and `mouthFrown` and `mouthFunnel` reach about 0.1. Don't build on them: a genuine
  smile cannot be read from `cheekSquint`. `jawOpen`, `mouthPucker`, `browDown` and
  `mouthPress` do move.

What this does not solve: a slit narrowed against the sun overlaps eyes shut (0.5
to 0.65), and neither `eyeSquint` nor the lid gap in the landmarks separates them;
sunglasses read as closed (0.47–0.74); a head lying on its side reads open with the
eyes shut. The relative rules catch the first two inside a burst, since every frame
reads alike there. And portraits from Commons are not a shoot, so **the thresholds
stay hypotheses** until a hand-culled shoot with real blinks has been scored with
`cull_eval --ablate` — the same caveat as every other threshold here.

