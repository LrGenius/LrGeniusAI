# Help: Cull Photos *(beta)*

> **Beta feature.** Culling results are useful as a starting point but should be treated as suggestions, not final decisions. Always review Picks and Reject Candidates before deleting anything. No photos are ever deleted automatically.

## What the culling workflow does

The **Cull Photos** workflow groups similar photos (bursts and near-duplicates), ranks them using technical and face-aware metrics, and creates Lightroom collections so you can quickly review:

- `Picks` – the best usable photo per group (a group where nothing is usable,
  such as a single blurred frame, has no pick)
- `Alternates` – reasonable alternatives you might still keep
- `Reject Candidates` – clearly weaker shots
- optional `Duplicates / Near Duplicates`
- `Brackets / Stacks / Panoramas (keep all)` – see below

No photos are deleted automatically. All results are non-destructive and shown via collections.

### Sets that must not be culled

An exposure bracket, a focus stack and a panorama all look like a burst to a
grouper: same framing, seconds apart, near-identical. But there is no keeper to
pick — every frame is part of one picture, and rejecting four of five destroys
the shot. The backend detects these sets and switches ranking off for them: no
winner, no reject candidates, and the whole group goes to the
`Brackets / Stacks / Panoramas (keep all)` collection instead.

Detection is deliberately cautious and needs corroborating evidence from more
than one signal, so it can miss a set rather than misread an ordinary burst.
Exposure-bracket detection reads the exposure compensation value from your
photos, so it works best on frames shot with AEB.

## Prerequisites

- The backend server is running and reachable from Lightroom.
- The photos have been processed with **Analyze & Index Photos** so the backend has embeddings and culling metrics for them.

## How to run culling

1. In Lightroom Classic, select the photos you want to cull **or** switch to a filtered view (for example a single shoot or folder).
2. Open the menu:  
   `Library → Plug-in Extras → Cull Similar Photos...`.
3. Choose:
   - **Apply to** – `Selected photos only` or `Current view`.
   - **Burst time window (seconds)** – how far apart two frames may be and
     still count as the same burst. `Auto (from preset)` uses the preset's
     own window: 1 s for Default, Portrait and Street, 2 s for Event, 3 s for
     Sports.
   - **Culling preset** – tunes thresholds and weights:
     `Default (sharpness, exposure, faces)`, `Portrait (faces and expression)`,
     `Street (candid moments)`, `Event (people, emotion, moments)`,
     `Sports (action and emotion)`. Every preset except Default also judges
     the moment; see below.
   - **Cull strength** – how many near-misses become reject candidates.
     `Gentle` keeps more alternates, `Strict` aims for one keeper per burst,
     `Balanced` is the preset's own margin. It only moves the score margin:
     a blurred frame, a blink or a blocked face is rejected at the same
     threshold whatever you choose.
   - **Create 'Duplicates / Near Duplicates' collection** – on by default.
   - **Lightroom flags and ratings** – see the next section.
4. Start the task and wait until the progress dialog completes.

## Flags, stars and labels in Lightroom

Collections are good for review; flags are what you cull *with*. The
**Lightroom flags and ratings** box writes the result the way you would by
hand, so the Library filter bar, the flag shortcuts and
**Photo → Delete Rejected Photos** all work on it directly:

- **Flag picks (P)** – the best usable frame of every group gets the Pick
  flag. On by default.
- **Flag reject candidates as rejected (X)** – reject candidates get the
  Reject flag. Off by default, because that flag is one menu command away
  from deleting files; turn it on once you trust the preset on your work.
  Nothing is ever deleted by the plugin.
- **Star ratings** – picks 3 stars, alternates 2 stars. Reject candidates
  get no stars; the flag is their mark. Off by default.
- **Purple label on "look before deleting" rejects** – a reject candidate
  that had its group's strongest moment but a technical flaw is labelled
  purple, so it stands out in the grid before you delete rejects. On by
  default.
- **Also overwrite flags, stars and labels I set myself** – off by default.
  The plugin remembers what each cull wrote on a photo (the *Culling wrote*
  field in the Metadata panel), so it can tell its own marks from yours. Marks
  an earlier cull wrote are always brought up to date with the new run — an
  earlier pick that is now an alternate loses its flag, an earlier "look
  before deleting" label goes — while a flag, rating or label you set
  yourself is left alone and counted in the completion dialog. Turn this on
  only to re-cull from scratch over your own marks.

Everything the run writes is one history step, so **Cmd/Ctrl+Z** takes it all
back. Frames of an exposure bracket, focus stack or panorama are never flagged
or rated.

The result collections are filled in review order: `Picks` and `Alternates`
best first, `Reject Candidates` worst first. Set the collection's sort to
**Custom Order** to see it.

The plugin calls the backend culling endpoint, which:

- groups photos into similarity clusters (`single`, `burst`, `near_duplicate`,
  and the keep-all types `bracket`, `focus_stack`, `panorama`)
- scores each image per group
- selects winners, alternates, and reject candidates — except in keep-all groups

## Emotion and the moment

Sharpness alone picks the wrong frame from a burst surprisingly often: the
sharpest frame is the one before the goal, the kiss with the eyes shut looks
like a blink, the laugh is a little soft. So every preset except Default also
asks what the photo shows:

- **Sports** asks about action (is the ball at the foot, is someone in the air)
  and about emotion (celebrating, protesting, despair).
- **Event** asks about candid moments and about emotion (laughing, crying with
  joy, hugging, guests reacting).
- **Portrait** asks about expression, **Street** about candid, unposed moments.

The stronger of the two answers counts, not their average: the goal
celebration has no ball in it, and the shot on goal has nobody cheering yet.

The decision then works the way a photographer culls: **first usable, then the
moment.** Among the frames that are sharp enough, have the eyes open and the
face clear, the strongest moment wins, even over a slightly sharper frame where
nothing happens. A frame with a flaw no edit can repair (blur, unintended
closed eyes, a blocked face) never beats a usable one, however strong its
moment. Exposure is not on that list, because Lightroom lifts a stop without a
trace.

**Emotion beats the blink.** At Event and Portrait, closed eyes in a kiss or a
burst of laughter are recognised and not treated as a blink.

**Singles and whole bursts can be rejected.** A single photo used to be a pick
whatever its quality. Now a clearly blurred single is a reject candidate, and
so is every frame of a burst that is blurred throughout. Exposure alone never
rejects a single, since a dark concert photo is often meant to be dark.

**A blurred strong moment is never deleted unseen.** When the frame with the
strongest moment of its group is a reject candidate for a technical flaw, and
its moment is clearly stronger than the pick's, its explanation says so ("strongest moment in the group, but it has a technical
flaw — look before deleting"), and the completion message counts these photos.

The moment is read from the image analysis (the SigLIP2 embedding). When you
cull with a moment preset, the preparation step therefore computes it for any
photo that does not have it yet. That takes longer than the culling signals
alone, but no AI descriptions are generated. If the moment could not be judged,
for some photos or for all of them, the plugin tells you why and what to do.

> The emotion questions and weights are a first version and are still being
> measured against hand-culled shoots. If a preset keeps picking the wrong
> frame, export a culling fixture (`Library → Plug-in Extras → Export Culling
> Fixture...`) from a shoot you culled by hand and share it.

## Result collections in Lightroom

For each culling run, the plugin creates a new collection set:

- **Name:** `Culling Results @ <timestamp>`
- **Contents:**
  - `Picks`
  - `Alternates`
  - `Reject Candidates`
  - optional `Duplicates / Near Duplicates`
  - `Brackets / Stacks / Panoramas (keep all)` – only when the run found such a
    set

After creation, Lightroom automatically switches to the **Picks** collection inside that set so you can start your review immediately.

You can safely rename or move these collections later; they are standard Lightroom collections.

### When the run finishes

The completion dialog counts the groups, picks, alternates, reject candidates
and near-duplicate groups, and names the preset used. When flags, stars or
labels were written it says how many of each, and how many were left alone
because you had set them yourself. Two more lines appear only when they
apply:

- how many groups were recognised as brackets, stacks or panoramas, and that
  every frame in them was kept
- **Not culled:** how many of the selected photos are missing from the
  collections, and why — the plugin could not compute a photo ID for them, or
  the backend grouped photos that are no longer in this catalog

If the backend reports a problem with the run itself, a **Culling warning**
dialog shows it before the collections are created.

## Understanding scores and explanations

For each photo, the backend stores culling-related fields such as:

- group information: `cull_group_id`, `cull_group_type`, `cull_group_rank`, `cull_group_winner`
- technical scores: `cull_sharpness`, `cull_sharpness_peak`, `cull_exposure`,
  `cull_highlight_clip`, `cull_shadow_clip`, `cull_noise`, `cull_noise_flat`
  (noise measured only where the frame is smooth, so fine detail no longer
  reads as grain; ranking prefers it to `cull_noise`, and photos indexed
  before it existed rank on `cull_noise` until re-indexed),
  `cull_motion_anisotropy` (camera shake vs. subject motion),
  `cull_sharp_region_x` / `cull_sharp_region_y` (where in the frame the
  sharpness actually sits, so a sharp background does not win over a sharp
  subject)
- face scores: `cull_face_score`, `cull_face_count`, `cull_face_sharpness`,
  `cull_face_prominence`, `cull_face_visibility`, `cull_eye_openness`,
  `cull_blink_penalty`, `cull_occlusion`
- aesthetic scores: `cull_aesthetic` (a contrast-and-colour heuristic) and the
  photo's **Aesthetic score** from the AestheticSigLIP model, stored at
  indexing time (`aesthetic_score`) and read by culling as `cull_aesthetic_model`
- moment scores, also computed at cull time: `cull_semantic_iqa` (action,
  expression or candid moment, depending on the preset), `cull_emotion_iqa`
  (Sports and Event) and `cull_eyes_closed_intent_iqa` (kiss or laugh, Event
  and Portrait)
- explanations: `cull_reason_codes`, `cull_explanation`

The plugin writes only the result of culling into the photo's metadata fields:
*Culling decision*, *Culling score*, *Culling rank*, *Culling group ID* and
*Culling explanation*. The sub-scores above (sharpness, exposure, noise, the
face scores, the reason codes and the group type and winner) stay in the
backend: they were never read back in Lightroom, and writing twenty fields per
photo made every cull run touch every photo's last-edit time. The explanation
carries the reason codes in words. Photos culled before this change keep the
values their old fields held, but those fields are no longer shown or updated.

Typical reason codes include:

- `sharpest_in_group`
- `blurred`
- `underexposed` / `overexposed`
- `best_face_quality` / `weak_face_quality`
- `eyes_open_best`
- `possible_blink`
- `possible_occlusion`
- `no_face_detected_in_group`
- `near_duplicate_weaker`
- `peak_action` / `best_expression` / `strongest_moment` / `strongest_emotion`
  – the moment decided the pick
- `eyes_closed_intentional` – eyes closed in a kiss or a laugh, not counted as a
  blink
- `strong_moment_but_unusable` – the group's strongest moment, but a technical
  flaw put it among the reject candidates; look before deleting
- `bracket_frame_kept` / `focus_stack_frame_kept` / `panorama_frame_kept`

These help explain why a specific frame was chosen as a pick or flagged as a reject candidate.

## "Some culling signals are missing"

When the selected photos have no culling data yet, the plugin offers to prepare
them — a fast pass that computes only what culling reads. It never runs a
language model, so when the backend is on this machine it hands over a group of
photos at a time and they are read and measured across several CPU cores rather
than one after another. If a signal could not be computed, the plugin says so
before the culling run starts instead of quietly grading photos without it. The usual cause is that the on-device models are not
downloaded yet: open **File → Plug-in Manager → LrGeniusAI** and press
**Download AI models**, then run culling again. Culling still works in the
meantime, but face-aware ranking (eyes open, sharpness, occlusion) is inactive,
and with a moment preset the moment is not judged either.

The same applies after the run: when the moment could not be judged for some
photos because they have no image analysis yet, the culling warning names how
many and what to do. Photos without it are ranked on sharpness, exposure and
faces only, and so is every photo in the same group, so that the frames of one
burst are always compared on the same terms.

The moment and emotion questions and the kiss-or-blink check need the AI
search model once, to prepare a set of reference phrases; those are then kept
in LrGeniusAI's database, so later culling runs never load the model for them.
If the model was not there for that first step, one culling warning names every
score that could not be computed, says that ranking ran on the technical
measurements alone, and how to fix it.

**The aesthetic score** is separate: every photo gets one when it is indexed (or
prepared for culling) by the *Photo quality scoring* model, a second download in
the same **Download AI models** button, and culling reads it from there. It is
one input among several (sharpness, exposure and faces weigh more) and it only
orders frames *within* a group, so a photo that scores low is not rejected for
that alone. Photos indexed before it existed, or scored by an older version of
the model, have none; the culling warning then says how many and that they were
ranked on a simpler contrast-and-colour estimate instead, which can compare
unevenly with scored frames in the same group. Run **Analyze & Index Photos** on
them to fill it in.

## Capture time and grouping

Burst grouping runs on each photo's capture time. The fast preparation pass
sends it from the catalog, and the backend reads it from the file's EXIF when
the catalog has none. Photos prepared by an older version without it group
badly (whole sequences of similar frames collapse into one group with one
pick), so the plugin offers to prepare them again once; a photo that has no
capture time anywhere is remembered as such and not asked about again.

Rejects for a blink, a blocked face or a weak face are judged against the
other frames of the group as well as against fixed thresholds: a frame is only
a blink when its eyes read clearly less open than the best frame of its burst.
On small faces the eye estimate reads "closed" on every frame, so without this
every frame but the pick was a reject. The same treatment for blur exists in
the backend but is switched off until it has been checked against hand-culled
shoots of several genres.

## Tips for best results

- Run culling after you have narrowed down an initial selection for a shoot (for example by folder or date range).
- Use **Analyze & Index Photos** with face detection enabled if you want face-aware ranking (eyes open, sharpness, occlusion).
- Start with conservative presets (`default`) and treat the result as a review aid, not an automatic delete list.

