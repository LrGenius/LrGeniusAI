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
| **Near duplicates (phash)** | Visually nearly identical images — same scene, slightly different exposure, crop, or retouch. Useful for finding exact or near-exact duplicates. |
| **Similar content (CLIP)** | Semantically similar images — same subject, style, or scene even if taken at a different time or place. Useful for finding shots "like this one". |

### Search scope

- **All indexed photos** — searches the entire backend database.
- **Current view** — restricts the search to the currently open folder or collection.
- **Selected photos** — searches within the current selection.

### Max results

How many similar photos to return. Default: 100.

### Similarity strictness (near-duplicate mode)

Controls how closely two images must match to be included:

- **Strict** — only very close matches (nearly identical images).
- **Normal** — balanced default.
- **Loose** — broader matches, useful for finding related variations.

---

## Tips

- Use **Near duplicates (phash)** to find forgotten duplicates before deleting or archiving a shoot.
- Use **Similar content (CLIP)** to build collections of photos sharing a visual theme — useful for portfolio editing or creating consistent series.
- Photos must be indexed with **Create search embeddings** enabled for CLIP mode to work. Near-duplicate (phash) mode also benefits from indexing but uses perceptual hash comparison.
- Set the Lightroom collection sort order to **Custom Order** after the results collection is created to see the best matches at the top.
