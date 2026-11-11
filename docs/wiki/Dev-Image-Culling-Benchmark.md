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
itself. The series are indexed on a backend of this build and graded with each
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

## Open: what the benchmark found and this did not fix

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
- **Focus behind the subject is not named** (`subject_soft`: found 71 %, named
  0 %), and the frame-wide reading cannot tell a shallow depth of field from a
  miss: 21 % of `background_soft` frames are nominated. That is what the subject
  pass is for.
- **Camera shake is rarely named** `motion_blur` (2 %): the anisotropy
  threshold almost never fires on a real streak.
- **Wildlife and landscape find few rejects** (recall 14 % and 33 %): the
  `default` preset rejects a soft single only, and its blur threshold sits
  below most of the blurred frames there.
