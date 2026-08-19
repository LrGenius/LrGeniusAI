# Help: Find Similar Images

The **Find Similar Images** workflow lets you select one photo and find other visually or perceptually similar photos across your indexed catalog. Results are placed into a new Lightroom collection for easy review.

## How to use it

`Library → Plug-in Extras → Find Similar Images...`

1. Select a **single photo** in the Library grid.
2. Open *Find Similar Images* from the menu.
3. Choose your search options in the dialog.
4. The plugin queries the backend and creates a new Lightroom collection with the results, sorted by similarity.

---

## Search options

### Find by

| Mode | What it finds |
|---|---|
| **Near duplicates (visual match)** | Visually nearly identical images — same scene, slightly different crop or retouch. Useful for finding exact or near-exact duplicates. Uses a perceptual hash. |
| **Similar content (AI)** | Semantically similar images — same subject, style, or scene even if taken at a different time or place. Useful for finding shots "like this one". Uses the SigLIP embedding. |

> A perceptual hash compares how the picture *looks*, so it does not survive a
> large exposure change: the same frame at +2 EV reads as a different image.
> Use **Similar content (AI)** to find other frames of the same scene across a
> bracket.

### Search in

- **All indexed photos** — searches the entire backend database.
- **Current view** — restricts the search to the currently open folder or collection.

### Max results

How many similar photos to return. Default: 100.

### Similarity (near-duplicate mode only)

Controls how closely two images must match to be included:

- **Strict (near duplicates)** — only very close matches.
- **Normal** — balanced default.
- **Loose (more variety)** — broader matches, useful for finding related variations.

---

## Tips

- Use **Near duplicates** to find forgotten duplicates before deleting or archiving a shoot.
- Use **Similar content (AI)** to build collections of photos sharing a visual theme — useful for portfolio editing or creating consistent series.
- Photos must be indexed with **Enable smart photo search** for the AI mode to work. Near-duplicate mode needs indexing too, but compares perceptual hashes rather than embeddings.
- Set the Lightroom collection sort order to **Custom Order** after the results collection is created to see the best matches at the top.
