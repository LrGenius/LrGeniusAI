# Culling evaluation fixtures

Labelled groups that `lrg_analysis::eval` scores, driven by
`cargo run --release -p lrg-analysis --example cull_eval -- testdata/cull_eval`.

```bash
# score the current configuration
cargo run --release -p lrg-analysis --example cull_eval -- testdata/cull_eval

# ...and against ranking as it was before the signal-quality pass
cargo run --release -p lrg-analysis --example cull_eval -- testdata/cull_eval --compare

# attribute a delta to one change
cargo run --release -p lrg-analysis --example cull_eval -- testdata/cull_eval \
  --ablate sharpness-peak     # or: relative | iqa | sets
```

## What is in here, and what it is not

**These four fixtures are synthetic, and they were written by the same person
who wrote the code they score.** They encode the specific failure modes named in
[Dev-Image-Culling-Signal-Analysis.md](../../../docs/wiki/Dev-Image-Culling-Signal-Analysis.md)
— shallow depth of field read as softness, brackets culled, an absolute exposure
target applied to low-key work, long-lens frames where only the subject is sharp.
They are therefore **regression tests for known bugs**, and a good score on them
means those specific bugs have not come back. It does **not** mean culling is
accurate on real photographs, and no number produced from this directory should
be quoted as if it did.

The thing that would make it a benchmark is real labelled shoots, which nobody
has contributed yet. Adding one is the highest-value contribution to culling
quality available; see below.

## Adding a real fixture

**The easy way — from Lightroom.** *Library → Plug-in Extras → Export Culling
Fixture…* does all of this for you:

1. Cull a shoot by hand, as you normally would. Reject flag for frames you would
   delete, pick flag for each keeper, star ratings to order the rest.
2. Run the task, pick a preset and a filename, and it writes a fixture here.

It reads your flags and ratings as labels and pulls the metrics straight from the
backend, so nothing has to be transcribed. Tick *"I stacked these photos myself"*
only if you used Lightroom stacks to mark which frames belong together —
otherwise the group boundaries come from the backend's own output, and the
exporter sets `groups_are_authoritative: false` so the harness skips the grouping
metrics rather than scoring the grouper against itself.

**By hand**, if you would rather not use the plugin:

1. Cull the shoot and note, per burst: which frame you kept, the order of the
   rest, and which you would delete.
2. Index the shoot (`tasks=cull` is enough) and pull the stored inputs:
   ```bash
   curl -s localhost:19819/cull -H 'content-type: application/json' \
     -d '{"photo_ids": ["…"], "include_stored_metadata": true}' > raw.json
   ```
   Use each `photos[].stored_metadata` block verbatim as the entry's
   `metadata` — **not** `photos[].metrics`. The two look similar and are not:
   `metrics` is what ranking *concluded* (short names, derived values, preset
   weights already applied), `stored_metadata` is what it *read*. Only the
   latter can reproduce a run.
3. Write one JSON file per shoot in this directory, shaped like the existing
   ones — the schema is `lrg_analysis::eval::EvalFixture`:

   ```json
   {
     "name": "wedding-2026-05-reception",
     "notes": "Real. Canon R5, 35mm f/1.4, candlelight only.",
     "culling_preset": "event",
     "time_delta_seconds": 2,
     "groups": [
       {
         "group_id": "g1",
         "photos": [
           {"photo_id": "…", "capture_time": 1747000000.0,
            "phash": "a5a5a5a5a5a5a5a5",
            "metadata": {"cull_sharpness": 0.61, "…": 0},
            "rank": 1},
           {"photo_id": "…", "…": 0, "rank": 2, "reject": true}
         ]
       }
     ]
   }
   ```

4. For a bracket, focus stack or panorama, set `"intentional_set": "bracket"`
   (or `"focus_stack"` / `"panorama"`) on the group and leave the per-photo
   `rank`/`reject` labels off. Every frame is wanted, so there is nothing to
   order and nothing to reject.

**Label the ranking, not the metrics.** The fixture carries whatever the backend
computed; your job is only to say which frame you would have kept. A fixture
where the labels were adjusted to match the output measures nothing.

Photo ids are opaque strings here — nothing resolves them against a catalog — so
a fixture can be shared without shipping the photographs.

## Reading the numbers

| metric | what a bad score means |
|---|---|
| winner top-1 | the pick collection is showing the wrong frame |
| NDCG | the ordering is roughly right even when the winner is wrong |
| reject precision | frames are being offered for deletion that shouldn't be — the failure users notice |
| reject recall | culling is being timid; costs time, not trust |
| set preservation | **a bracket or stack was torn apart.** Should always be 100% |
| set recognition | the set survived by luck rather than on purpose (see below) |
| grouping P/R | the frames being compared are the wrong set; every number above is unreliable |

There is deliberately no combined score. Reject precision and recall trade
against each other directly, and collapsing them would hide the trade.

### Why set preservation and set recognition are separate

Preservation asks "was any frame of the set nominated for deletion". Recognition
asks "did the set land in one group that was flagged keep-all".

They came apart the first time these fixtures were run against the *old*
ranking, which scored a perfect 100% preservation — not because it protected
anything, but because it could not group the bracket frames at all, and a
singleton group is never nominated for rejection. A metric that certifies the
bug it was written to catch is worse than no metric. Preservation is still the
harm measure and must stay at 100%; recognition is the one that says the
protection is real.

## Known open finding

`event.json` scores 50% reject precision: group-relative exposure normalisation
widens the score spread inside a group, which makes `reject_score_delta` fire on
the third frame of a three-frame low-key burst that was labelled "keep". That is
a real cost of the relative pass, not a fixture bug, and it is left visible
rather than tuned away — `--ablate relative` shows both sides of it.
