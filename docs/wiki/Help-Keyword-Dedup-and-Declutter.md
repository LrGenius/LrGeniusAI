# Help: Keyword Deduplication and De-Clutter

LrGeniusAI offers two complementary keyword management features to keep your Lightroom catalog clean:

- **Deduplicate Keyword Synonyms** — an interactive workflow to find and merge near-duplicate or synonym keywords that already exist in your catalog.
- **Auto De-Clutter during Indexing** — an automatic step inside Analyze & Index Photos that prevents newly AI-generated keywords from creating near-duplicates of what's already in your catalog.

---

## Deduplicate Keyword Synonyms

### What it does

Scans your existing Lightroom keywords using AI semantic similarity to find groups of keywords that mean the same thing (e.g. `Car` / `Automobile` / `Vehicle`). For each group it picks one canonical name and lets you merge the others into it.

This is a **catalog-modifying operation**. Back up your catalog before running it.

### How to run it

`Library -> Plug-in Extras -> Deduplicate Keyword Synonyms...`

The workflow has five steps:

#### Step 1 — Configuration

- **AI Model** — choose which LLM validates the similarity clusters. Options: ChatGPT, Gemini, Ollama, LM Studio. If no key/server is configured, the task falls back to CLIP-only (no LLM validation).
- **Matching Strictness** — slider from 0.70 to 0.98.
  - Lower values (0.70) produce more suggestions and may include false positives.
  - Higher values (0.98) are conservative; fewer but more certain matches.
  - Default: 0.85. Your last-used value is remembered.
- **Backup confirmation** — you must tick the checkbox confirming you have a backup before you can continue.

#### Step 2 — Select keyword branches

A list of all top-level keywords in your catalog is shown with checkboxes. Select the branches you want to scan.

Only **leaf keywords** (keywords with no children) are considered for deduplication. Category keywords with sub-keywords are never touched.

#### Step 3 — AI clustering (background)

For each selected branch the plugin:

1. Generates CLIP/SigLIP semantic embeddings for all leaf keyword names.
2. Builds a similarity matrix and groups keywords using **complete-linkage clustering**: a keyword joins a cluster only if it is above the threshold with *every* existing member. This prevents false chains where A≈B and B≈C but A is unrelated to C.
3. Optionally submits candidate clusters to the selected LLM for refinement. The LLM is instructed to:
   - Keep only true synonyms (e.g. `Car` / `Automobile`).
   - Split groups that contain related-but-distinct concepts (e.g. `Cat` vs `Kitten`).
   - Put the clearest, most common name first — this becomes the canonical name.

If the LLM call fails, the CLIP-only clusters are used as a fallback.

#### Step 4 — Preview and approve

A list of all proposed merge pairs is shown:

```
"Automobile"  →  "Car"
"Vehicle"     →  "Car"
"Pic"         →  "Photo"
```

Deselect any pair you want to keep separate. When satisfied, click Merge.

There is also a **Sync backend** checkbox (recommended on). When checked, the backend's ChromaDB metadata is updated so that semantic search and future AI operations reflect the merged keywords.

#### Step 5 — Execution

For each approved pair:

1. All photos tagged with the duplicate keyword are re-tagged with the canonical keyword.
2. The duplicate keyword tag is removed from those photos.
3. The now-empty duplicate keyword remains in the catalog keyword list with 0 photos. To purge it, use Lightroom's built-in `Metadata → Purge Unused Keywords`.

A summary is shown at the end: how many keywords were merged, how many pairs were skipped, and how many backend records were updated.

---

## Auto De-Clutter during Indexing

### What it does

When **Analyze & Index Photos** generates new keywords, the de-clutter step compares those new keywords against what's already in your catalog. If a new keyword is semantically near-identical to an existing one, it is replaced by the existing one before anything is written to Lightroom.

This means: if your catalog already has `Car` and the AI generates `Automobile`, the photo receives `Car`, not a second keyword entry.

### When it runs

Automatically, as part of the **Apply Metadata** phase inside Analyze & Index Photos — no separate action needed. It only runs when new keywords are being written.

### What the validation dialog shows

If any replacements were made, the **keyword validation dialog** shows a two-column view:

| Generated (AI) | | De-cluttered (final) |
|---|---|---|
| Automobile | → | Car |
| Vehicle | → | Car |
| Tree | | Tree |

- Left column: what the AI originally produced.
- Right column: what will actually be written to Lightroom (editable).
- An arrow is shown only when the name was changed.
- A label below the list shows the total number of merges applied.

You can edit any entry in the right column before confirming.

### How the canonical name is chosen

Existing catalog keywords always win over newly generated ones. If the AI generates a keyword that already exists in the catalog (case-insensitively), the existing form is used as-is. If multiple new keywords cluster together and none already exists in the catalog, the first member of the cluster is used as the canonical name.

---

## Tips

- **Run Deduplicate Keyword Synonyms after a large initial indexing run.** A freshly indexed catalog often has synonym sprawl that is easiest to clean up in one batch before you build more keyword structure on top of it.
- **Use a higher strictness (0.90+) with LLM validation** for precise taxonomies (wildlife, botany, places). Use lower strictness (0.75–0.85) with CLIP-only if you want a fast first pass and are happy to review more suggestions manually.
- **De-clutter is CLIP-only** (no LLM call) to keep indexing latency low. For finer control, run the interactive Deduplicate task afterwards.
- **Back up your catalog** before running the interactive deduplication. The merge step modifies keyword assignments across potentially thousands of photos and cannot be undone automatically.
- After a merge run, use `Metadata → Purge Unused Keywords` in Lightroom to remove the now-empty duplicate entries from the keyword list.

---

## Preferences persisted across runs

| Preference | Description |
|---|---|
| Matching Strictness | Last-used slider value (0.70–0.98). |
| AI Model | Last-selected LLM provider and model. |
