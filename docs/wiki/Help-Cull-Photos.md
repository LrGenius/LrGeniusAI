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
     own window: 1 s for Default, Portrait and Street, 2 s for Event and
     Wildlife, 3 s for Sports — under the Auto preset, the window of the
     preset most of the selection calls for.
   - **Culling preset** – tunes thresholds and weights:
     `Auto (each burst by its genre)`, `Default (sharpness, exposure, faces)`,
     `Portrait (faces and expression)`, `Street (candid moments)`,
     `Event (people, emotion, moments)`, `Sports (action and emotion)`,
     `Wildlife (animals, the subject sharp)`. Every preset except Default also
     judges the moment; see below.
   - **Auto** is the default for a new setup: each burst is judged with the
     preset its photos' genre calls for (Portrait, Event, Sports, Wildlife or
     Default), so a wedding day with a football match in the afternoon is
     culled with both. The completion message says what it chose. Photos the
     AI has called "street" are judged as an event: the Street preset
     deliberately ignores focus, and that should be your choice, not a guess.
     On the [public benchmark](Dev-Image-Culling-Benchmark.md) Auto ranked at
     least as well as the preset chosen by hand for every series. Choose a
     preset yourself when you know better — it then applies to everything.
   - **Cull strength** – how many near-misses become reject candidates.
     `Gentle` keeps more alternates, `Strict` aims for one keeper per burst,
     `Balanced` is the preset's own margin. It only moves the score margin:
     a blurred frame, a blink or a blocked face is rejected at the same
     threshold whatever you choose.
   - **Keep** – `The pick of every burst` (the default), `A number of
     photos` or `A share of the photos (%)`. A number or a share chooses
     across the whole selection, the way a gallery is delivered: the shoot is
     split into scenes (bursts close in time and alike), the best photo of
     every scene comes first, then the best of the rest that are unlike what is
     already chosen, so the speeches do not crowd out the dancing. The chosen
     photos are the picks — none of some bursts, several of others. A photo
     with a flaw is chosen only when its scene has nothing better, and the
     culling warning says so.
   - **Highlights** – with a number or a share, this many of the best chosen
     photos, kept different from each other, go into a `Highlights`
     collection: a sneak peek. 0 for none.
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
- **Wildlife** asks about behaviour: a bird in flight, an animal feeding,
  running, looking back at the camera, two animals interacting.

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

## Focus on the eyes

For photos of people, the question most culls turn on is whether the eyes are
sharp — or whether the focus landed on the nose, an ear or the hair. That cannot
be answered on the reduced copy the analysis otherwise works with: there, an eye
in a half-length portrait is a dozen pixels. So for every photo with faces, the
backend measures focus a second time on the **full-size original**, around each
eye and over the face as a whole.

- **The sharper eye counts.** With a shallow depth of field the near eye is sharp
  and the far one is not, which is how it should be.
- **Sharp eyes overrule a soft-looking frame.** A small face against something dark
  or out of focus can make the whole frame measure soft. Where the eyes were measured
  sharp, the frame is not called blurred.
- **In a couple or group shot, the least sharp prominent face decides**: the
  frame is only sharp when everyone in it is. Small faces in the background do
  not count.
- **Too small to judge is not the same as soft.** When the eyes are too small in
  the photo to measure (less than about 40 pixels apart in the original), they
  are left out and the face as a whole stands in for them.
- **Frames are compared with each other.** A frame is reported as "the eyes are
  not sharp" only when a sharper frame of the same burst exists, and it becomes a
  reject candidate only when it is clearly behind that frame. A single photo is
  never rejected for its eye focus alone.
- The explanation shows **eye focus N/10 within the group**: 10 is the frame
  with the sharpest eyes of its group, and each point below it is a step
  softer. It is a comparison, not an absolute grade — how sharp a photo is at
  the pixel level depends on the camera, the lens, the ISO and how large the
  face is, as much as on the focus. A photo with no other measured frame in its
  group gets no number.

**Portrait** weights eye focus most and holds it to the highest bar, **Default**
and **Event** less, **Sports** least (the athlete as a whole has to be sharp,
and motion softens faces a little), and **Street** not at all, because the
moment counts there, not the technique.

**Where the original comes from.** With the backend on this computer it is the
photo file itself — for a raw file, the full-size JPEG the camera embedded in it
(some cameras embed only a smaller one, which is then what is measured). With
the backend on another computer it is the JPEG the plugin sends, at the
**Export size** set in Plug-in Manager (3072 pixels by default).

Photos prepared for culling before this existed have no eye-focus reading. The
culling dialog offers to prepare them again; if you skip that, the bursts they
are in are ranked without eye focus, and the culling warning says how many
photos that affects.

## Focus on the subject

A frame can look soft for two opposite reasons. The focus missed: the subject
is soft and, often, the background behind it sharp. Or the photographer meant
it: a shallow depth of field, or a pan that streaks the background while the
athlete or the bird stays sharp. Sharpness measured over the whole frame cannot
tell the two apart, and that is the frame it most often gets wrong — it calls
the keeper blurred, or keeps the one whose focus landed on the wall behind.

So culling first finds what the photo is of: a person, an animal, a vehicle or
a thing, with its outline (an AI model, part of **Download AI models**, about
120 MB). It then measures focus on the subject and on the background
separately.

- **A sharp subject settles it.** When the subject reads sharp, a soft
  background is a choice and the frame is never called blurred for it. A pan is
  named as such ("a pan: the subject sharp, the background streaked").
- **Focus behind the subject is named.** A frame whose subject is clearly
  softer than in another frame of the burst says "the subject is not sharp",
  or "focus landed behind it" when the background is the sharper part, and
  becomes a reject candidate when it is clearly behind. A single photo is
  never rejected for it alone.
- **Faces come first.** For a person whose eyes could be measured, the eyes
  decide, as before; the subject answers for everything faces cannot — an
  athlete seen from the side, an animal, a car.
- **Only faces on the subject count.** A second person soft in the background
  of a portrait, or a spectator behind the athlete, is out of focus on purpose
  and no longer makes the keeper a reject. When the subject is an animal or a
  thing — a dog, a cake — and no face is on it, the people beside it are more
  likely the point, and every face counts.
- **What carries the subject belongs to it.** A rider and the horse, a cyclist
  and the bicycle are one subject, so neither is ever taken for the background
  the other stands against.
- **Wildlife** leans on the subject most, and judges the moment by behaviour:
  in flight, feeding, running, looking back. The animal's eye is not judged
  yet.

Photos prepared before this existed have no subject reading. The culling
dialog offers to prepare them again; if you skip that, the bursts they are in
are judged on frame-wide sharpness, and the culling warning says how many
photos that affects.

## Eyes open, smile and gaze

Whether the eyes are open is judged by an AI model made for exactly that:
Google's MediaPipe face mesh and expression model, which places 478 points on
each face and reads 52 expressions from them — among them how far each eye is
closed, the smile and where the eyes look. It replaces an earlier rough
estimate that, on the small faces of a ceremony, called almost every frame a
blink.

- **Open, half closed, closed — or unknown.** Each prominent face gets one of
  these. A face turned far away, barely visible or too small in the photo is
  *unknown*: it is never called a blink, and it does not count for or against
  the frame.
- **One blink spoils the group.** The most closed eyes among the people that
  matter in the frame decide, so a group shot with one person blinking is not
  "eyes open". One shut eye counts as well, even when the other is open. Small
  faces in the background do not count.
- **Compared with the burst.** A frame becomes a reject candidate for closed
  eyes only when another frame of the same burst has them open. Half-closed
  eyes (a squint into the sun, a mid-blink) are named in the explanation but
  never make a frame a reject candidate. The model cannot always tell eyes
  narrowed to a slit from eyes shut, so a hard squint can read as closed, and
  sunglasses can too. Comparing with the burst keeps either from costing a frame
  when the whole burst looks the same.
- **A laugh is not a blink.** A broad smile with the eyes shut counts as a
  laugh, like a kiss already did.
- **Looking down is not a blink.** Lids down on a lowered head, a child looking at
  the candles say, read much like shut eyes. Such a frame says "looking down" and
  is never a reject candidate for it. Someone really asleep with the head down gets
  the same note, which costs a warning and no photo.
- **The smile counts in Portrait and Event.** Between otherwise equal frames,
  the one where everyone smiles more wins, and its explanation says
  "best smile in group".
- **The mouth, in Portrait and Event.** A frame with the mouth caught open mid-word
  says so when another frame of the burst has it closed. A note only; a broad
  smile or a laugh does not count.
- **The light on the face.** The explanation names a face darker than in another
  frame of the burst, harder light across the face than in another frame, and skin
  that is blown out. Not at **Street** or **Sports**. These are notes and move no
  score. A face is only ever compared with itself in another frame, never with a
  standard, because skin differs by several stops between people.
- **Looking away means from the lens.** **Portrait** notes when someone looks
  away from the camera. Head and eyes count together: a head turned to the side
  with the eyes back on the camera is looking at it, and a head turned with the
  eyes following it is not. It is a note, never a reason to reject.

Like focus, the face is read on the full-size original where there is one.
The model is part of **Download AI models** (about 7 MB). Without it culling
uses the old rough estimate for blinks and says so in a culling warning, with
the fix; photos prepared before the model was downloaded are offered for
preparing again once it is there.

## Key people

Tick **Judge the key people first** in the dialog, and a frame is judged by the
people who matter in the shoot rather than by whoever is largest in it:

- **Who counts:** everyone you have named in **People** who is in these photos,
  and anyone who is in at least a fifth of the selected photos that have faces
  (and in three at least) — the couple at a wedding, the child at a birthday.
  Faces you have not sorted in People yet are grouped among the selection's
  photos for this, so it works on a shoot you have never opened People for.
- **What changes:** in a burst that shows one of them, only their faces count —
  however small. The bride's blink counts though a guest stands larger in front
  of her, and a frame where she turned away has no face that matters. Bursts
  without a key person are judged as before.
- **Named in the explanation:** "Anna: eyes closed", "Anna: the eyes are not
  sharp". The completion message lists the key people it found.

It needs face recognition, which the fast culling preparation leaves out:
with the option on, **Prepare now** also recognises faces, which takes
longer. Photos that have not been through it are counted in the culling
warning. The rule of who counts is a first guess: no public collection of
photographs says who mattered to the photographer, so it is not measured.

**Small faces.** Faces are looked for twice: once in the whole photo, and
again in four overlapping quarters of it, which finds faces down to about a
third of the size — the people in the back of a group, the spectators of a
match. Such a face counts for culling only; it is too small to be recognised,
so it never appears in People. A small face is not found in every frame alike:
a frame where it was missed ranks below the frames that show it, but when its
subject is sharp it is not suggested for rejection for that.

## Your culling style

Every photographer culls a little differently: one always takes the broader
smile, another the sharper frame, a third the stronger moment. **Library →
Plug-in Extras → Learn My Culling Style...** learns yours from photos you
culled yourself — select a few shoots where you set flags, rejects or stars
and start it.

- **What it learns from:** inside each burst, two frames you marked
  differently — a pick over an unflagged frame over a reject, more stars over
  fewer — are one decision. Marks a cull set and you left alone are not
  counted; ones you changed are, and say the most.
- **How it proves itself:** it learns from most of your bursts and is tested
  on the rest, against the default ranking on the same bursts. It reports how
  often each agrees with you ("agrees with your own choices 71 % of the time;
  the default ranking 64 %"). It needs at least 30 decisions, and it is used
  only when it agrees with you more often than the default does.
- **How it is used:** tick **Use my culling style** in Cull Similar Photos. It
  reorders the frames of a burst that are not reject candidates; a blink or a
  blur stays a reject, since that is not a matter of taste. A pick it changed
  says "your culling style prefers it", and the completion message counts them.

It stays on this computer, with the backend's database. Run the task again to
learn from more shoots; its dialog also forgets the learned style.

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
  `cull_blink_penalty`, `cull_occlusion`, and `cull_eye_focus` /
  `cull_face_focus` (focus on the eyes and on the face, measured on the
  full-size original; absent when the faces were too small to judge), and
  `cull_eye_state` / `cull_eye_closed`, `cull_smile`, `cull_eye_squint`,
  `cull_gaze_off_lens`, `cull_mouth_open`, `cull_face_luminance`,
  `cull_face_across_stops`, `cull_face_blown` (from the face model; absent when no
  face could be judged)
- subject: `cull_subject_kind`, `cull_subject_area`, `cull_subject_focus` and
  `cull_background_focus` (focus on the subject and on the rest of the frame),
  `cull_background_streak` (how streaked the background is, high for a pan)
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
- `highlights_clipped` – highlights blown out where a sibling frame kept them
- `noisier_than_sibling` – clearly grainier than a frame of the burst that is
  otherwise as sharp: ranked below it, never a reject candidate for the grain
  alone
- `best_face_quality` / `weak_face_quality`
- `eyes_open_best`
- `eyes_closed` – the eyes are closed, and another frame of the burst has them
  open
- `eyes_half_closed` – half closed or squinting (never a reject on its own)
- `looking_away` – looking away from the camera (Portrait)
- `best_smile` – the broadest smile of the group (Portrait, Event)
- `possible_blink` – the older rough estimate, used when the eye model is not
  downloaded
- `eyes_out_of_focus` – the eyes are clearly softer than in a sharper frame of
  the same burst; the focus landed elsewhere
- `face_out_of_focus` – the same for the whole face, where the eyes were too
  small to measure
- `eyes_sharpest_in_group`
- `subject_out_of_focus` – the subject is clearly softer than in a sharper
  frame of the same burst
- `focus_on_background` – the same, with the background the sharper part:
  focus landed behind the subject
- `subject_sharpest_in_group`
- `panning` – the subject sharp, the background streaked by a pan; never
  counted as blur
- `possible_occlusion`
- `no_face_detected_in_group`
- `near_duplicate_weaker`
- `peak_action` / `best_expression` / `strongest_moment` / `strongest_emotion` /
  `peak_behaviour` – the moment decided the pick
- `eyes_closed_intentional` – eyes closed in a kiss or a laugh, not counted as a
  blink
- `strong_moment_but_unusable` – the group's strongest moment, but a technical
  flaw put it among the reject candidates; look before deleting
- `bracket_frame_kept` / `focus_stack_frame_kept` / `panorama_frame_kept`
- `preferred_by_your_style` – your learned culling style chose this pick

These help explain why a specific frame was chosen as a pick or flagged as a reject candidate.

## "Some culling signals are missing"

When the selected photos have no culling data yet — or were prepared by an
older version that did not measure everything culling now reads, such as the
capture time or eye focus — the plugin offers to prepare them: a fast pass that
computes only what culling reads. It never runs a
language model, so when the backend is on this machine it hands over a group of
photos at a time and they are read and measured across several CPU cores rather
than one after another. If a signal could not be computed, the plugin says so
before the culling run starts instead of quietly grading photos without it. The usual cause is that the on-device models are not
downloaded yet: open **File → Plug-in Manager → LrGeniusAI** and press
**Download AI models**, then run culling again. Culling still works in the
meantime, but face-aware ranking (eyes open, sharpness, occlusion) is inactive
(or, when only the small eye and expression model is missing, blinks are judged
by a rough estimate),
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
every frame but the pick was a reject. Blur is judged the same way: a frame
with siblings is only blurred when it is clearly softer than the sharpest of
them, and a burst that missed focus throughout is still rejected whole.

**Blown highlights cost, a dark frame hardly.** Between two frames of a burst,
the one whose highlights are blown out where its sibling kept them ranks lower
and says so ("highlights blown out where a sibling frame kept them").
Lightroom lifts a stop of shadow without a trace and brings nothing back from
a clipped highlight. A photo that is clipped on purpose, high key say, is
clipped in every frame of its burst and loses nothing.

## Tips for best results

- Run culling after you have narrowed down an initial selection for a shoot (for example by folder or date range).
- Use **Analyze & Index Photos** with face detection enabled if you want face-aware ranking (eyes open, sharpness, occlusion).
- Start with conservative presets (`default`) and treat the result as a review aid, not an automatic delete list.

