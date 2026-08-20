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
out of a real complaint about a soccer series.

1. **A real eye-state classifier.** `blink_penalty` is still exactly
   `1 - eye_openness`, and the underlying signal is a mean vertical gradient in
   a patch clamped to ≤8px radius that stops scaling with face size. YuNet's
   single keypoint per eye cannot support an eye-aspect-ratio measure, so this
   needs a small ONNX eye-state model shipped as a new asset.
2. **A noise estimator that does not compete with sharpness.** The current one
   is a 3×3 box-blur residual, so real detail reads as grain: measured on two
   frames of one scene differing only in focus, it scores the *sharp* frame
   0.082 and the blurred one 0.012. That is why noise was deliberately left out
   of the group-relative pass (see below) — normalising a signal that points the
   wrong way only amplifies it.
3. **Native-resolution eye-region sharpness.** Tiled sharpness fixed the
   shallow-depth-of-field failure, but eye-vs-nose focus — the most common
   portrait cull decision there is — still is not resolvable, because face
   sharpness is measured on the 2048px working image rather than a
   native-resolution crop around the eye.
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
- **CLIP-IQA aesthetics** (`lrg-ml::clip_iqa`). Five antonym prompt pairs
  embedded once per server lifetime; every photo is then a handful of dot
  products against the SigLIP2 vector already in the database. Blended against
  the old heuristic at `aesthetic_iqa_weight` (0.8). Degrades silently to the
  heuristic when there is no embedding or no reachable text tower.
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
| `sports` | `action` | 0.30 | ball visible, feet off the ground, mid-play vs between plays |
| `street` | `candid` | 0.32 | unposed vs posed, people reacting to each other |
| `event` | `candid` | 0.22 | the same, weighted lower — event work still has to be deliverable |
| `portrait` | `expression` | 0.20 | warm genuine smile vs flat awkward expression |
| `default` | — | 0 | spans every genre; no one question is right for all of them |

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
passes it through. `0.0` switches the axis off, `0.5` lets it lead. The shipped
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
did before.

**It has not been validated on real photographs.** That is not an oversight: the
prompts are a hypothesis, and this repository has now twice shipped a
plausible-sounding signal that was wrong. Which is why the other half of this
work is the fixture exporter.

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
