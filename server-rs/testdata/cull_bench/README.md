# Public culling benchmark

Free-licence photographs from Wikimedia Commons, each turned into a series with
defects made on purpose, so that the right answer is known without anyone
culling by hand. `scripts/cull_bench.py` builds it; `cull_eval` scores it:

```bash
cargo run --release -p lrg-analysis --example cull_eval -- testdata/cull_bench/fixtures --by-defect
```

The background, the baseline and what changed since are in
[Dev-Image-Culling-Benchmark.md](../../../docs/wiki/Dev-Image-Culling-Benchmark.md).

## What is in here

- `sources.toml` — every photograph: its Commons page, licence, author, and why
  it was taken for its genre. The photographs themselves are not in the
  repository.
- `fixtures/<genre>.json` — one fixture per genre, in the format of
  `lrg_analysis::eval::EvalFixture`: what the backend measured on every frame
  (`metadata`), its packed embedding (`embedding_q8`), and the labels.

## A series

Each photograph becomes one group, frames 0.3 s apart in a fixed but arbitrary
order. Not every photograph can carry every frame: an eye defect needs a face
turned to the camera with the eyes at least 45 px apart, a subject defect a
subject mask covering 1.5–65 % of the frame, a pan a subject in sport or
wildlife.

| `defect` | What was done (at 3000 px on the long edge) | Label |
|---|---|---|
| `original` | nothing | rank 1; must not be called blurred or out of focus |
| `background_soft` | background blurred (sigma 12 px), subject left sharp: a shallow depth of field | rank 1, **must keep**; must not be called blurred |
| `panning` | background streaked sideways (75 px), subject left sharp | rank 1, **must keep**; must not be called blurred |
| `blur_mild` | whole frame, sigma 1.5 px | rank 2, deleting it is optional |
| `under` / `over` | −1 EV / +1.3 EV in linear light | rank 2, optional; should be named `underexposed` / `overexposed` |
| `noise` | grain, about ISO 12 800 | rank 2, optional |
| `blur_strong` | whole frame, sigma 6.6 px | rank 3, **reject**; `blurred` or `motion_blur` |
| `motion` | camera shake, an 18 px streak at a random angle | rank 3, **reject**; `motion_blur` |
| `eyes_soft` | only the eyes blurred (sigma 6 % of the eye distance): focus on the nose or the ear | rank 3, **reject**; `eyes_out_of_focus` |
| `subject_soft` | only the subject blurred (sigma 4.5 px): focus on the background | rank 3, **reject**; `subject_out_of_focus` or `focus_on_background` |

Rank 1 frames may all win; the score must put each better rank above each
worse one (*pair order*). "Optional" (`reject_optional`) means a reject
candidate there is neither right nor wrong: it is a matter of cull strength,
since a stop darker than its twin is worse but not broken.

## What it does not measure

- **Taste and the moment.** Every frame of a series shows the same moment, so
  the moment scores are equal across it by construction. Which of two sharp
  frames is the better picture still takes a hand-culled shoot
  (*Export Culling Fixture* in Lightroom, `testdata/cull_eval`).
- **Real misses.** A Gaussian blur is not a lens out of focus, and camera shake
  is not a straight line. The defects are close enough to separate the cases
  culling has to tell apart, not to stand in for every photograph.
- **Grouping.** The series are made to be grouped; grouping is only checked
  in that each one ends up in one group.

## Making it again

```bash
cd server-rs/scripts
uv run python cull_bench.py select --per-genre 24      # only to change the photographs
uv run python cull_bench.py make
GENIUSAI_PORT=19830 ../target/release/geniusai-server --db-path ~/.cache/lrgenius/cull-bench/db/lrgenius.db &
uv run python cull_bench.py run --url http://127.0.0.1:19830
```

`run --reindex` measures every frame again, after a change to what indexing
stores; without it, frames the backend already has are only graded again.
The subject masks and eye positions that place the defects come from
torchvision's Mask R-CNN and Keypoint R-CNN, never from the models the backend
ships, so the benchmark does not grade a model by its agreement with itself.
