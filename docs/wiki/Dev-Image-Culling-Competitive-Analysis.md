# Culling: what the other tools judge

What the established culling tools evaluate, how they choose, and where their
users say they fail — researched on 2026-11-10 to decide what culling here has
to do to match them, and where it can do better. The plan that came out of it
is in [Dev-Image-Culling-Implementation-Plan.md](Dev-Image-Culling-Implementation-Plan.md#matching-the-established-tools);
the benchmark that measures it in [Dev-Image-Culling-Benchmark.md](Dev-Image-Culling-Benchmark.md).

Most of what follows comes from the vendors' own help pages and marketing.
Independent tests are few, and none of the vendors publishes an accuracy figure
with a method someone else could repeat. Treat every number a vendor gives as a
claim.

## The tools

| Tool | Runs | What it judges | How it chooses | Learns from the user |
|---|---|---|---|---|
| **Lightroom Classic Assisted Culling** (GA June 2026, 15.4) | locally | Subject sharpness, eye sharpness, eyes open (with "can't tell"), exposure issues; reject reasons *Documents* and *Misfires*. Scores per face since GA | Thresholds on 0–100 sliders, per photo. Stacks by time or similarity, but does not pick the best of a stack | no |
| **Aftershoot** | locally | Blur (lenient / moderate / strict), closed eyes (open / partial / closed), duplicates, "key faces", a 1–100 image score; nine genres (sports judges the body, not the face) | At least one keeper per duplicate group; *Cull to Target* (10–60 % or an exact count) "weighing quality, coverage and variety"; *Highlights* | yes, from the user's corrections and final picks; mechanism not documented |
| **Narrative Select** | locally | Per face: focus out of 10, eyes and expression (green / yellow / red); main, secondary and background faces (background ones are not judged). Without faces: sharpness against the rest of the scene | Five tiers relative to the *scene*: Best in scene … Undesirable; Ruthless / Balanced / Cautious. No target count. Never rejects on its own | no |
| **FilterPixel DeepCull** | cloud (previews) | Blur, exposure, blinks; per genre up to ten criteria (emotion, group attention, key moments, composition…) with a score and a reason in words | At least one per burst; "Magic Number" target | yes ("memory-based") |
| **Imagen** | cloud | Focus, exposure, unintended blinks with a kiss exception, duplicates | Best per group; target count or share | not for culling |
| **Excire Search / Foto** | locally | Sharpness, face sharpness, eye sharpness, eyes open, smile, aesthetics | Top N or N % per group by criteria the user picks; groups by time, similarity, people or content | no |
| **Evoto** | — | Eyes open / half / closed / blurry, expression (big smile / smile / neutral), face focus (in / soft / blurry) | A total limit plus min/max per cluster; groups by time, duplicates or faces | — |

### Lightroom's Assisted Culling, in its users' words

It is the tool every Lightroom user already has, so its failures are the bar.
From Adobe's feedback threads (early access from October 2025, GA in June 2026):

- closed and half-closed eyes are missed, and frames with clearly shut eyes are
  called "likely open";
- sunglasses count as sharp, open eyes;
- people in the background count as part of the subject, so a sharp foreground
  person is rejected for "subject focus";
- every panning shot is rejected although the subject is sharp, and full-body
  shots fail the subject focus test;
- birds: "the bird needs to be in focus, not the tree";
- dark clothing and moody light read as underexposed, white scenes as overexposed;
- the focus score is not linear (slider at 20: 9 rejects, at 30: 387), and the
  same photos get different results on another run;
- it judges every frame on its own: "its absolute, rather than relative nature",
  and there is no best-of-burst.

### What none of them does well

- **Animals.** Lightroom is "tuned for people"; Excire has no wildlife features;
  Narrative added a "key element" for objects and animals in 2026 without saying
  how it is found. Nobody judges an animal's eye.
- **Repeatable accuracy numbers.** FilterPixel's comparison pages give 94.7 %
  keepers and 96.1 % rejects on "2,740 wedding images" and three other figures
  for the same thing elsewhere.
- **Taste.** "What no tool does reliably is judge taste" (PhotoWorkout, 2026).
  Aftershoot and FilterPixel claim to learn it; neither says how.

## Where culling here stands

Already at the level of the best of them, by design:

- **Relative to the burst**, not against absolute thresholds — the complaint
  users make most about Lightroom. Eye focus, blink, occlusion and exposure are
  all judged against the group's best frame
  ([Dev-Image-Culling-Implementation-Plan.md](Dev-Image-Culling-Implementation-Plan.md)).
- **Eye focus on the full-size original**, the sharper eye counting, the weakest
  prominent face deciding a group shot; **eye state** from MediaPipe with an
  explicit *unknown*; **looking down** is not a blink; **a laugh or a kiss** is
  not a blink.
- **An explanation for every frame**, in words, which only FilterPixel offers too.
- **Brackets, focus stacks and panoramas** are recognised and never culled.
  None of the others mentions them.
- **Local**, like Lightroom, Aftershoot, Narrative and Excire.

Missing, in the order the plan takes them:

1. **A measurement.** Every threshold is a hypothesis; there was no benchmark.
   *Done: [the public benchmark](Dev-Image-Culling-Benchmark.md).*
2. **The subject without a face** — sport, wildlife, products. Sharpness is the
   sharpest of 8×8 tiles, which can be the background. *Done, with Lightroom's
   two complaints above answered: only faces on the subject count, and a pan is
   named as one ([numbers](Dev-Image-Culling-Benchmark.md#the-subject-pass)).*
3. **Choosing by genre** — the genre is stored for every photo, but the user
   still picks the preset. *Done: `auto` chooses per burst
   ([numbers](Dev-Image-Culling-Benchmark.md#the-preset-from-the-genre)).*
4. **People who matter** (Aftershoot's key faces, Narrative's main faces) and
   small faces in a crowd. *Done: key people, named in the explanation, and a
   second look for small faces
   ([numbers](Dev-Image-Culling-Benchmark.md#people-key-people-and-small-faces)).*
5. **Scenes, a target count and highlights** across a shoot. *Done
   ([numbers](Dev-Image-Culling-Benchmark.md#scenes-a-target-and-highlights)).*
6. **Animal eyes.**
7. **Learning from the user** — a Lightroom catalog holds years of the
   photographer's flags and stars, which no stand-alone tool can read. *Done:
   learned locally, and used only where cross-validation shows it agrees with
   the photographer more often than the default ranking.*
8. Optionally, **a vision model for close calls**, with its reason in words.
   *Measured with a local model and not better than the ranking; not shipped.*

## Sources

- Adobe: feedback threads for Assisted Culling in Lightroom Classic,
  [early access](https://community.adobe.com/questions-675/early-access-assisted-culling-lrclassic-983825)
  and [general availability](https://community.adobe.com/questions-675/p-assisted-culling-feedback-thread-lrc-1627467);
  [Paper Arts, "Lightroom's New Culling Features"](https://paperarts.substack.com/p/lightrooms-new-culling-features);
  [Fstoppers](https://fstoppers.com/lightroom/use-lightroom-ai-find-sharp-eyes-open-keepers-seconds-719139)
- Aftershoot: [culling](https://aftershoot.com/culling/),
  [genres](https://support.aftershoot.com/en/articles/10570203-aftershoot-culling-genres),
  [preferences](https://support.aftershoot.com/en/articles/6508163-setting-your-ai-automated-culling-preferences-in-aftershoot),
  [blur settings](https://support.aftershoot.com/en/articles/6508135-get-the-correct-blur-settings),
  [Cull to Target](https://support.aftershoot.com/en/articles/11533911-cull-to-target-with-aftershoot),
  [technical answers](https://support.aftershoot.com/en/articles/10601968-technical-answers-about-aftershoot),
  [changelog](https://aftershoot.com/changelog/)
- Narrative: [face and focus assessments](https://help.narrative.so/en/articles/7337369-face-and-focus-assessments),
  [First Pass](https://help.narrative.so/en/articles/7337372-ai-first-pass-image-assessments),
  [scenes](https://help.narrative.so/en/articles/8119179-use-scenes-and-rank-ordering-in-narrative),
  [photos without faces](https://help.narrative.so/en/articles/17659545-how-does-narrative-assess-photos-without-faces-or-with-creative-blur),
  [focus score caveats](https://help.narrative.so/en/articles/7337398-why-does-an-out-of-focus-image-have-a-high-focus-assessment-score)
- FilterPixel: [DeepCull](https://filterpixel.com/deepcull),
  [memory-based culling](https://filterpixel.com/blog/introducing-deepcull-memory-based-culling),
  [comparison](https://filterpixel.com/best-ai-photo-culling-software),
  [SLR Lounge launch article](https://www.slrlounge.com/filterpixel-ai-photo-culling-deep-cull/)
- Imagen: [culling](https://imagen-ai.com/culling/),
  [culling guide](https://imagen-ai.com/valuable-tips/photo-culling-guide)
- Excire: [Lightroom AI culling vs. Excire Search](https://excire.com/lightroom-classic-ai-culling-vs-excire-search-plugin/),
  [best culling software](https://excire.com/en/best-culling-software/)
- Evoto: [culling & organization](https://support.evoto.ai/?p=13367)
- Independent overview: [PhotoWorkout, 2026](https://www.photoworkout.com/?p=223718)
- Research: Chang et al., [Automatic Triage for a Photo Series](https://pixl.cs.princeton.edu/pubs/Chang_2016_ATF/index.php),
  SIGGRAPH 2016 (15,545 photos in 5,953 series with human preferences; the
  dataset's host no longer resolves); Huang et al.,
  [Series Photo Selection via Multi-view Graph Learning](https://arxiv.org/abs/2203.09736)
  (72–75 % pair accuracy on that dataset)
