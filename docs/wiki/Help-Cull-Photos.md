# Help: Cull Photos *(beta)*

> **Beta feature.** Culling results are useful as a starting point but should be treated as suggestions, not final decisions. Always review Picks and Reject Candidates before deleting anything. No photos are ever deleted automatically.

## What the culling workflow does

The **Cull Photos** workflow groups similar photos (bursts and near-duplicates), ranks them using technical and face-aware metrics, and creates Lightroom collections so you can quickly review:

- `Picks` – best candidates per group
- `Alternates` – reasonable alternatives you might still keep
- `Reject Candidates` – clearly weaker shots
- optional `Duplicates / Near Duplicates`

No photos are deleted automatically. All results are non-destructive and shown via collections.

## Prerequisites

- The backend server is running and reachable from Lightroom.
- The photos have been processed with **Analyze & Index Photos** so the backend has embeddings and culling metrics for them.

## How to run culling

1. In Lightroom Classic, select the photos you want to cull **or** switch to a filtered view (for example a single shoot or folder).
2. Open the menu:  
   `Library -> Plug-in Extras -> Cull Similar Photos`.
3. Choose:
   - **Scope** – `Selected photos` or `Current view`.
   - **Preset** – for example `default` or `sports` (tunes thresholds and weights).
4. Start the task and wait until the progress dialog completes.

The plugin calls the backend culling endpoint, which:

- groups photos into similarity clusters (`single`, `burst`, `near_duplicate`)
- scores each image per group
- selects winners, alternates, and reject candidates

## Result collections in Lightroom

For each culling run, the plugin creates a new collection set:

- **Name:** `Culling Results @ <timestamp>`
- **Contents:**
  - `Picks`
  - `Alternates`
  - `Reject Candidates`
  - optional `Duplicates / Near Duplicates`

After creation, Lightroom automatically switches to the **Picks** collection inside that set so you can start your review immediately.

You can safely rename or move these collections later; they are standard Lightroom collections.

## Understanding scores and explanations

For each photo, the backend stores culling-related fields such as:

- group information: `cull_group_id`, `cull_group_type`, `cull_group_rank`, `cull_group_winner`
- scores: `cull_score`, `cull_sharpness`, `cull_face_score`, `cull_eye_openness`, `cull_blink_penalty`, etc.
- explanations: `cull_reason_codes`, `cull_explanation`

The plugin writes a subset of these values into plugin-specific metadata fields on each photo so they can be inspected or used for diagnostics.

Typical reason codes include:

- `sharpest_in_group`
- `blurred`
- `underexposed` / `overexposed`
- `best_face_quality` / `weak_face_quality`
- `eyes_open_best`
- `possible_blink`
- `near_duplicate_weaker`

These help explain why a specific frame was chosen as a pick or flagged as a reject candidate.

## Tips for best results

- Run culling after you have narrowed down an initial selection for a shoot (for example by folder or date range).
- Use **Analyze & Index Photos** with face detection enabled if you want face-aware ranking (eyes open, sharpness, occlusion).
- Start with conservative presets (`default`) and treat the result as a review aid, not an automatic delete list.

