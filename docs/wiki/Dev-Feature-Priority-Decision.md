# Feature Priority Decision

## Decision

The top feature to build next is `AI Culling Assistant`.

## Why `AI Culling Assistant` comes first

`AI Culling Assistant` has the strongest end-to-end foundation in the current codebase:

- Batch processing already generates quality scores alongside embeddings and metadata.
- The analyze/index workflow already supports scope selection, progress handling, and writing results back into Lightroom.
- Advanced Search already exposes a quality-based result flow (`prettiest` / `ugliest`) and creates Lightroom collections from ranked results.
- Existing metadata fields such as `overall_score`, `composition_score`, `lighting_score`, `motiv_score`, `colors_score`, `emotion_score`, and `quality_critique` are already stored in the backend.

This means the remaining work is mainly productization:

- turn existing quality signals into a dedicated culling workflow
- define stronger ranking and filtering rules
- create clearer Lightroom collection outputs such as picks, rejects, and shortlist candidates
- improve UX around review and confidence

## Why not `Duplicate Finder` first

`Duplicate Finder` is promising, but its core grouping implementation is still missing.

- The API route for similarity grouping already exists.
- The service layer already delegates to Chroma grouping.
- The actual grouping function `group_and_sort_images(...)` is still explicitly not implemented.

That makes it a good second priority, but not the fastest path to a polished user-facing workflow.

## Why not `Shoot/Event Grouping` first

`Shoot/Event Grouping` has useful input signals available today, but it is less product-ready than culling.

- Capture time, folder names, GPS, and AI metadata are already available.
- There is no dedicated event-grouping service, grouping API, or Lightroom workflow for turning these signals into usable event buckets yet.
- The feature still needs the grouping logic and the user-facing interaction model.

Compared with culling, more of the product shape is still undefined.

## Recommended implementation scope

Build `AI Culling Assistant` first with a narrow first release:

1. Implement burst and near-duplicate grouping first.
2. Use explainable technical and face-aware signals as the initial ranking basis.
3. Add a dedicated command that creates Lightroom collections for top picks and weaker candidates.
4. Allow optional thresholds or target counts per selection/view.
5. Keep the first version deterministic and review-friendly before adding a lightweight aesthetic model.

## Priority order

1. `AI Culling Assistant`
2. `Duplicate / Near-Duplicate Finder`
3. `Shoot / Event Grouping`
