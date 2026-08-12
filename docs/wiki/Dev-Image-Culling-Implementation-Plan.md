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
- [ ] Build small benchmark dataset for evaluation *(not present in the repo — see Current state)*
- [ ] Evaluate optional aesthetic model as secondary signal *(still a contrast/colourfulness heuristic)*

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
  quality, skipping the embedding, the LLM, ArcFace and face thumbnails.
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

Ordered by value.

1. **Subject-region sharpness.** Global Laplacian variance on a 512px
   downscale still penalises correct shallow depth of field (sports, wildlife,
   f/1.4 portraits) and cannot resolve eye-vs-nose focus. Needs tile-max plus
   the sharp region's *location*, and a native-resolution crop around the eye
   for portraits.
2. **Bracket / focus-stack / panorama detection.** Nothing detects these, so
   the grouper nominates a winner and marks the rest reject candidates,
   destroying intentional sets. This is the most damaging failure mode for
   landscape, architecture and real-estate work. Needs `exposureBias` and focus
   distance carried through from the plugin at index time.
3. **A real eye-state classifier.** `blink_penalty` is still exactly
   `1 - eye_openness`, and the underlying signal is a mean vertical gradient in
   a patch clamped to ≤8px radius that stops scaling with face size. SCRFD's
   single keypoint per eye cannot support an eye-aspect-ratio measure.
4. **Learned aesthetic / IQA.** `cull_aesthetic` is contrast + colourfulness +
   exposure, which floors muted, low-key and foggy work. A LAION-style linear
   head or CLIP-IQA antonym prompts ride on the SigLIP2 embedding that already
   exists — close to zero marginal cost.
5. **Relative-within-group normalisation.** Exposure targets absolute mean
   luminance 0.5 and the noise estimate is a high-pass residual that rises with
   real detail, so it partly cancels the sharpness reward. Both should be
   scored relative to the group.
6. **`occlusion` does not measure occlusion** — it is a recombination of
   detector confidence, distance from frame centre and eye openness, so it is
   collinear with `det_score`.
7. **Job/progress/cancellation.** `/cull` is still one blocking request with a
   binary 0→100% progress bar. `JobRegistry` has no progress field, no
   cancellation, and destroys a finished job on first read.
8. **Genre auto-detection.** Zero-shot classification against the existing
   SigLIP2 embeddings is a dot product. More importantly, genre must change
   *which signals are trusted*, not just their weights — a weights-only preset
   system cannot express "ignore background sharpness" or "suppress culling for
   bracket sets".
9. **Hardware acceleration.** CPU is the only default execution provider.
   CoreML is macOS-only and env-gated behind `LRG_ML_EP=coreml`; the face
   sessions have no EP path at all. No CUDA, no DirectML. Both ONNX models are
   batch-size 1 behind a `Mutex`.
10. **An evaluation benchmark.** The checklist above ticks "Build small
    benchmark dataset for evaluation" and "Evaluate optional aesthetic model",
    but no such fixture set exists in the repository. Nothing above item 1 can
    be validated as an *improvement* rather than a *change* without it: labelled
    groups per genre, scored on top-1 winner accuracy, reject precision, NDCG,
    and bracket/stack/pano preservation rate.
