# Culling: the public benchmark

Every culling threshold in this project was a hypothesis: the fixtures under
`server-rs/testdata/cull_eval` are synthetic and were written by the person who
wrote the code they score, and no hand-culled shoot has been contributed. The
public benchmark is the measurement in between. It needs no hand cull, because
it makes the defects itself and therefore knows the right answer.

What the other tools judge, and what this benchmark is for, is in
[Dev-Image-Culling-Competitive-Analysis.md](Dev-Image-Culling-Competitive-Analysis.md).

## How it is made

`server-rs/scripts/cull_bench.py` takes 120 quality images from Wikimedia
Commons — 24 each for event, portrait, sport, wildlife and landscape, free
licences only (`testdata/cull_bench/sources.toml` names each photograph, its
author and its licence) — and turns each into a series, as if shot in a burst:

| Frame | Made by (at 3000 px) | The right answer |
|---|---|---|
| `original` | nothing | may win; never "blurred" |
| `background_soft` | background blurred, subject sharp: a shallow depth of field | may win; **must survive any cull** |
| `panning` | background streaked sideways, subject sharp (sport, wildlife) | may win; **must survive any cull** |
| `blur_mild` | sigma 1.5 px all over | worse; deleting it is optional |
| `under` / `over` | −1 / +1.3 EV in linear light | worse; optional; named as such |
| `noise` | grain, about ISO 12 800 | worse; optional |
| `blur_strong` | sigma 6.6 px all over | **reject**, named `blurred` |
| `motion` | an 18 px streak of camera shake | **reject**, named `motion_blur` |
| `eyes_soft` | only the eyes blurred: focus on the nose | **reject**, named `eyes_out_of_focus` |
| `subject_soft` | only the subject blurred: focus behind it | **reject**, named as focus on the subject |

Where the subject and the eyes are comes from torchvision's Mask R-CNN and
Keypoint R-CNN (eyes only on faces turned to the camera), never from the models
the backend ships, so the benchmark does not grade a model by its agreement with
itself. The subject is the people of a photograph, the animals in wildlife, and
in sport the athletes with what carries them (a horse, a bicycle touching
them). A photograph with a person as large as one of the subject's left out of
it carries no subject defect, and `eyes_soft` blurs only the subject's eyes:
either way the defect would be focus on one of two people, which is no clear
answer. The series are indexed on a backend of this build and graded with each
genre's preset; the fixtures hold the measurements, packed embeddings and
labels, and no pixels. `testdata/cull_bench/README.md` says how to make it
again; `cargo test -p lrg-analysis --test cull_bench_fixtures` holds ranking to
its level in CI.

**What it does not measure.** Taste and the moment: every frame of a series
shows the same moment. Real misses: a Gaussian blur is not a lens out of focus.
And grouping, beyond each series ending up as one group (it does, 100 %).

## Reading the numbers

`cargo run --release -p lrg-analysis --example cull_eval -- testdata/cull_bench/fixtures --by-defect`

- **winner top-1** — a frame that may win (`original`, `background_soft`,
  `panning`) has the best score of its series.
- **pair order** — of every two frames with different ranks, the better one
  scores higher.
- **reject precision / recall** — over the frames labelled reject or not;
  frames whose deletion is optional count for neither.
- **must-keep lost** — shallow depth of field and pans nominated for deletion.
  The error a photographer forgives least.
- **reason named / wrong reason** — the explanation names the defect, or
  misnames a frame (`blurred` on a sharp subject).

## First run, and what changed

The first run (2026-11-10, backend at `c910424`), and the ranking after the
changes it led to, on the same labels
(`--ablate benchmark-calibration` reproduces the left column):

| | before | after |
|---|---|---|
| winner top-1 | 31.7 % | 33.3 % |
| pair order | 77.1 % | 75.9 % |
| reject precision | 82.3 % | 87.7 % |
| reject recall | 70.0 % | 68.7 % |
| must-keep lost | 25.4 % | 16.9 % |
| reason named | 46.1 % | 49.9 % |
| wrong reason | 13.4 % | 10.3 % |

What the run found, by defect (after):

| defect | frames | won the series | nominated | found | must-keep kept | ordered below the best | named |
|---|---|---|---|---|---|---|---|
| original | 120 | 26 | 12 % | | | | |
| background_soft | 94 | 10 | 21 % | | 79 % | | |
| panning | 48 | 4 | 8 % | | 92 % | | |
| blur_mild | 120 | 0 | 49 % | | | 84 % | |
| under | 120 | 14 | 33 % | | | 79 % | 40 % |
| over | 120 | 5 | 52 % | | | 84 % | 86 % |
| noise | 120 | **58** | 8 % | | | **27 %** | |
| blur_strong | 120 | 0 | 72 % | 72 % | | 100 % | 96 % |
| motion | 120 | 1 | 58 % | 58 % | | 90 % | 2 % |
| eyes_soft | 59 | 1 | 81 % | 81 % | | 83 % | 81 % |
| subject_soft | 94 | 1 | 71 % | 71 % | | 83 % | 0 % |

Changed, each with its own evidence:

- **Blown highlights cost** (`clip_penalty_weight` 0.2, `clip_penalty_scale`
  0.05). The exposure term asks how close the mean sits to mid-grey, which the
  frame a stop and a third brighter answered better than the original while
  6–14 % of it was blown white: it took the pick in 21 of 120 series. Clipping
  beyond the least clipped frame of the group now costs, and is named
  (`highlights_clipped`): 21 → 5. Pair order falls 1.2 points with it, since a
  clipped frame now also ranks below some of the blurred ones — which is
  arguable either way, and what the label "keepable but worse" got wrong.
- **Wider face margins** (`relative_defect_margin` 0.10 → 0.14,
  `relative_occlusion_margin` 0.10 → 0.20). The face state model's lids and the
  occlusion proxy read up to 0.1 and 0.2 apart between a frame and the same
  frame a stop brighter, so originals were "blinks" and "occluded" against
  copies of themselves: five of the seven sport originals nominated for
  deletion were "occluded". 0.20 is also what the real baptism shoot measured.
- **Blur against the group** (`relative_blur_rule` on), the second candidate
  from that shoot: precision +0.5, must-keep lost −0.8, recall unchanged.

None of these replaces a hand cull. They are where two independent sources —
one real shoot, one benchmark — point the same way.

## The subject pass

Step 1 of [the plan](Dev-Image-Culling-Implementation-Plan.md#matching-the-established-tools)
(2026-11-11): the subject found by RF-DETR-Seg, focus on it and on the
background, faces counted only on the subject, and noise compared between
frames of equal sharpness. Measured on a benchmark made again for it (the sport
subject and the two rules above: 1115 frames, from 1135), so the left column is
ranking without these rules on the same measurements
(`--ablate subject`), the right one with them:

| | without | with |
|---|---|---|
| winner top-1 | 31.7 % | 73.3 % |
| pair order | 78.6 % | 85.9 % |
| reject precision | 90.7 % | 93.0 % |
| reject recall | 68.4 % | 69.2 % |
| must-keep lost | 12.1 % | 9.1 % |
| reason named | 50.4 % | 62.3 % |
| wrong reason | 5.2 % | 1.2 % |

The faces-on-the-subject rule is in the measurements of both columns, since
only re-indexing could take it back; against the first run it took must-keep
lost from 16.9 % to 12.1 % on its own.

By defect (with):

| defect | frames | won the series | nominated | found | must-keep kept | ordered below the best | named |
|---|---|---|---|---|---|---|---|
| original | 120 | 45 | 7 % | | | | |
| background_soft | 87 | 30 | 10 % | | 90 % | | |
| panning | 45 | 13 | 7 % | | 93 % | | |
| blur_mild | 120 | 0 | 49 % | | | 94 % | |
| under | 120 | 15 | 30 % | | | 82 % | 41 % |
| over | 120 | 12 | 49 % | | | 86 % | 87 % |
| noise | 120 | **3** | 8 % | | | 89 % | |
| blur_strong | 120 | 0 | 72 % | 72 % | | 100 % | 96 % |
| motion | 120 | 1 | 60 % | 60 % | | 97 % | 1 % |
| eyes_soft | 56 | 1 | 80 % | 80 % | | 86 % | 80 % |
| subject_soft | 87 | 0 | 70 % | 70 % | | 98 % | **85 %** |

What decided it, each with its own evidence:

- **Noisy twins** (`noise_twin_ratio` 1.6, `noise_twin_sharpness_delta` 0.02,
  `noise_twin_penalty` 0.15). The noisy copy took the series 56 times in 120,
  now 3. The penalty orders it below its twin and never makes it a reject
  candidate on its own: it alone pushed pans, whose streaked background keeps
  more grain than a twin blurred all round, past the reject line. A delta of
  0.03 paired a pan with its own soft-background twin (0.023 apart); grain moved
  frame-wide sharpness by 0.005–0.02.
- **The subject in the score** (`subject_focus_weight` 0.75; 1.0 for sports and
  wildlife). Pair order 76.6 % at 0, 82.9 % at 0.75. At 1.0 a landscape's
  "subject" took over and cost reject precision, while wildlife lost no keeper
  and nominated only rejects (12 of 12), and sport put 80 % of pairs in order.
- **Named for what it is** (`reason_subject_focus_threshold` 0.5): focus behind
  the subject is named 85 % of the time, from 0 %, with no frame misnamed for
  it. 0.45 named 79 %.
- **Faces on the subject only.** A soft bystander behind the subject, or a
  spectator's face the soft background hid, cost keepers in event and sport.
  For a person only the faces on them count; for an animal or a vehicle the
  faces on it, or every face when none is (three portraits had a pet as the
  larger subject); beside a thing every face.
- **The rider and the horse are one subject**, in the backend and the
  benchmark alike: blurring around the rider alone made the horse background.

`relative_subject_margin`, `reason_background_sharper_margin` and
`reason_panning_streak` changed nothing on the benchmark over their whole
range: they stay where they were set, untested.

## The preset from the genre

Step 2 (2026-11-11): `culling_preset: "auto"` ranks each series with the
preset its photos' genre calls for, without being told which genre the series
was picked for (`cull_eval --preset auto`; the fixtures carry the `genre` the
backend read). Same measurements, three ways of choosing:

| | `default` for all | chosen by hand per genre | `auto` |
|---|---|---|---|
| winner top-1 | 75.0 % | 73.3 % | 77.5 % |
| pair order | 84.6 % | 85.9 % | 86.2 % |
| reject precision | 92.4 % | 93.0 % | 93.3 % |
| reject recall | 70.0 % | 69.2 % | 69.2 % |
| must-keep lost | 12.1 % | 9.1 % | 8.3 % |
| reason named | 62.3 % | 62.3 % | 62.1 % |

The genre model calls many photographs of people outdoors "street": 13 of 24
portraits and 12 of 24 events. Ranked with the `street` preset, which leaves
focus unjudged on purpose, `auto` found 63.4 % of the rejects and only 48 % of
the eyes that missed focus. A guessed "street" is therefore ranked as an event;
the `street` preset stays the photographer's own choice. Mapped to `default`
instead, recall was the same and the winner 75.8 %; to `portrait`, 9.8 % of the
keepers were lost.

`auto` matched the genre the series was picked for in 80 of 120 series before
that mapping (wildlife 22, sport 17, portrait 10, event 7, landscape 24); the
rest went to a preset that ranks them as well. A fixture's prompt scores come
from the preset it was exported under, so where `auto` chooses another, that
group is ranked on the other preset's weights but the exported prompts'
answers. On this benchmark that changes little: every frame of a series shows
the same moment, so the answers are near equal within one.

CI holds `auto` at or above the hand-chosen presets on winner, pairs, both
reject numbers and keepers (`auto_ranks_as_well_as_the_preset_chosen_by_hand`).

## People: key people and small faces

Step 3 (2026-11-11), in two parts.

**Small faces.** The detector looks at the whole frame once, at 640 px, and
misses faces under about 20 px there — 64 px on the 2048 px working copy. A
second look at four overlapping tiles finds them. On the benchmark's 120
photographs (`lrg-ml`'s `tile_faces_probe` example) it found 176 → 224 faces
in the events, 84 → 133 in sport (39 of them the crowd of one match), 34 → 37
in the portraits, 2 → 3 in wildlife and none in the landscapes. Every one of
the 101 added faces was looked at: about 92 are faces, mostly spectators and
the back of a group; the rest are the back of a head, a shoulder, an arm, a
horse's head. Before tile faces were limited to the size the first look
misses, the larger ones were as often pots, an accordion or a horse's flank as
people, whatever the detector's confidence (0.61–0.88 either way).

On the ranking, re-indexed, against the measurements of step 2:

| | before | tiles | tiles, and a missed face on a sharp subject kept |
|---|---|---|---|
| winner top-1 | 73.3 % | 73.3 % | 73.3 % |
| pair order | 85.9 % | 85.9 % | 85.9 % |
| reject precision | 93.0 % | 92.5 % | 92.8 % |
| reject recall | 69.2 % | 70.8 % | 70.8 % |
| must-keep lost | 9.1 % | 9.8 % | 9.1 % |

Found by defect: eyes that missed focus 80 → 82 %, blur 72 → 74 %, focus behind
the subject 70 → 72 %. The one keeper the tiles cost was the frame where a
soft background hid the small face the athlete's other frames now had; a frame
whose face was not found beside siblings whose was, with its subject in focus,
now ranks below them but is no reject on the score margin alone.

**Key people.** Not measured: no public collection of photographs says whom
the photographer cared about, and a benchmark series is one photograph, not a
shoot. The rule (named in People, or in a fifth of the photos with faces) is a
first guess, and a contract test holds what it does: a guest standing larger in
front no longer hides the bride's blink, and the explanation says "Anna: eyes
closed".

## Scenes, a target and highlights

Step 4 (2026-11-11): a number or a share of photos chosen across a shoot, and
highlights among them (`lrg_analysis::selection`). The benchmark's series lie
an hour apart, so each is a scene; it can say whether the choice takes every
series and its best frame first, not whether a gallery is well told.
`cull_eval --preset auto --target N --highlights 5`:

| target | series covered | best frame of its series | labelled reject | highlights the best of their series |
|---|---|---|---|---|
| 120 (one per series) | 100 % | 76.7 % | 0.8 % | 72 % (5 per genre) |
| 240 (two per series) | 100 % | 63.6 % | 2.9 % | 72 % |
| 10 % | 93.3 % | 78.8 % | 0.9 % | 72 % |

One per series takes the same frame as the pick wherever the pick is right
(winner top-1 77.5 %). The second frame of a series is often a worse copy,
since the benchmark has no second moment to offer: 2.9 % of the chosen were
labelled rejects the ranking had not nominated. Two choices came from this
table: a cost of 0.1 per place a frame ranks below its group's pick when filling
(best frame 56.9 → 63.6 %, rejects 3.3 → 2.9 %; 0.2 took more rejects again),
and highlights take of each group only its best-ranked chosen frame — the
first version made the grainier twin of a chosen frame a highlight. Scenes made of several bursts,
the novelty term and the people bonus are not exercised here; their weights
are a first version.

## A learned culling style

Step 6 (2026-11-11) learns from the photographer's own flags and stars and
proves itself by cross-validation on them; there is no public data for it, and
it is measured on each photographer's own decisions every time it learns. As a
check that it learns at all, the benchmark's labels were given to
`POST /v1/cull/taste` as if they were a photographer's (the best frames picked,
the clear defects rejected): from 2557 decisions in 89 series it agreed with
them 91.6 % of the time on held-out series, the default ranking 87.3 %.
The other 31 series had been taken for focus stacks, which no decision comes
from — see below.

## Open: what the benchmark found and this did not fix

- **A face found in one frame and not the next.** Where the face detector
  finds the subject's face in one exposure of a burst only (a dark frame lifted
  a stop, a soft background that changed the context), the frames without it
  are "faceless" beside it and rank below it. Since step 3 such a frame is kept
  when its subject is in focus; the two keepers of a very dark match photograph
  still go, as underexposed.
- **The occlusion proxy reads up to 0.25 apart** between a frame and its
  soft-background or darker twin, and the darkest frame sets the group's bar:
  4 keepers. The benchmark has no occluded frames to weigh a wider margin
  against, so it stays at the 0.20 the real shoot measured.
- **The face state reads a soft-background twin differently** — eyes a little
  more closed, the head turned a little further — and the twin falls behind
  on the score: 3 keepers.
- **Who is the subject among people** is a guess the backend and the benchmark
  sometimes make differently (a footballer in front that Mask R-CNN missed, a
  face the backend counts and the benchmark blurred): 3 keepers.
- **Wildlife finds few rejects** (recall 14 %), as before, now with no keeper
  lost: the blur threshold sits below most of the blurred frames there.
- **Camera shake is rarely named** `motion_blur` (1 %), as before.

## Open after the first run

- **Noise reads as detail.** The noisy frame takes the series 58 times in 120,
  and in portraits 19 of 24. Grain lifts every focus measure: face sharpness
  +0.19, eye focus +0.06 to +0.09 against the clean original, which is then
  even called `eyes_out_of_focus` beside it. Pre-smoothing the eye patch does
  not cure it (0.8 px: +0.06; 1.2 px: +0.03, still ahead every time), and
  judging noise against the group rewards blur, which removes noise too. Within
  one burst at one ISO it does not arise; between a clean frame and a twin at
  three stops more ISO it decides. The fix needs a noise measure at full working
  resolution, compared only between frames of equal sharpness — with the
  subject pass, which needs a new index pass anyway.
  *Fixed by the subject pass: the noisy copy wins 3 series in 120.*
- **Focus behind the subject is not named** (`subject_soft`: found 71 %, named
  0 %), and the frame-wide reading cannot tell a shallow depth of field from a
  miss: 21 % of `background_soft` frames are nominated. That is what the subject
  pass is for. *Named 85 %, 10 % nominated, after it.*
- **Camera shake is rarely named** `motion_blur` (2 %): the anisotropy
  threshold almost never fires on a real streak.
- **Wildlife and landscape find few rejects** (recall 14 % and 33 %): the
  `default` preset rejects a soft single only, and its blur threshold sits
  below most of the blurred frames there.
