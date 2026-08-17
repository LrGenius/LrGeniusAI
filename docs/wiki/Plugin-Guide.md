# Plugin Guide

The LrGeniusAI Lightroom Plug-in is the primary frontend for communicating with the AI backend. Through native Lua integrations and Lightroom dialogs, it extends your photography workflow with AI — without freezing the Lightroom interface.

## Main Documentation

For detailed technical usage of the plugin component, please view the [`plugin/README.md`](Dev-Plugin-README).

## All menu items

All actions are available under `Library → Plug-in Extras`:

| Menu item | What it does |
|---|---|
| Analyze & Index Photos... | AI metadata generation + search embeddings, faces, optional species ID |
| AI Edit Photos... | Generate and apply Lightroom develop recipes |
| Advanced Search... | Semantic free-text photo search |
| Cull Similar Photos... | Burst grouping, ranking, Picks/Alternates/Rejects |
| Retrieve Metadata from Backend... | Pull AI-generated metadata back into Lightroom |
| Import Metadata from Catalog... | Sync existing Lightroom metadata into backend |
| People... | Browse face clusters, assign names, open person collections |
| Find Similar Faces... | Find photos with the same face as a selected photo |
| Find Similar Images... | Find near-duplicates or visually similar photos |
| Save Edits as AI Training Examples... | Feed your own edits to the AI as style reference |
| Deduplicate Keyword Synonyms... | Find and merge near-duplicate keywords in your catalog |

## Core workflows

### Analyze and Index
Passes photos to the AI backend to generate keywords, title, caption, and alt text. Simultaneously creates SigLIP2 semantic embeddings so photos can be found via Advanced Search. Optionally detects faces and identifies animal, plant and fungus species on-device (BioCLIP 2), writing the taxonomy to the plugin's metadata fields and, if you ask for it, to a `Species` keyword hierarchy. Configurable scope (selected, current view, entire catalog), metadata toggles, and extra context options (folder name, date, GPS). See [Help: Analyze and Index](Help-Analyze-and-Index).

### AI Edit Photos
For each photo, the backend builds a structured Lightroom develop recipe out of your own saved edits — no language model involved — which the plugin applies via the Lightroom SDK. Needs at least five training examples from *Save Edits as AI Training Examples*. Offers per-photo review and can put the edit on a virtual copy instead of the original. See [Help: AI Edit Photos](Help-AI-Edit).

### Advanced Search
Translates a natural language query into vector embeddings and compares them against your indexed photos. Results are placed into a new Lightroom Collection sorted by relevance. See [Help: Advanced Search](Help-Advanced-Search).

### Cull Similar Photos
Groups time-adjacent photos into bursts, scores each image (sharpness, face quality, eye openness, blink detection), and creates a timestamped Collection Set with Picks, Alternates, and Reject Candidates. See [Help: Cull Photos](Help-Cull-Photos).

### People & Faces
Lists detected face clusters (persons) with thumbnails and photo counts. Lets you assign names and jump directly to a Lightroom collection per person. **Find Similar Faces** finds other photos containing the same face as a selected photo. See [Help: People & Faces](Help-People-Faces).

### Find Similar Images
Finds near-duplicates (perceptual hash) or visually similar photos (CLIP embeddings) for a selected photo. Results go into a new Lightroom collection. See [Help: Find Similar Images](Help-Find-Similar).

### Keyword Management
- **Deduplicate Keyword Synonyms** — interactive workflow to find and merge near-duplicate catalog keywords.
- **Auto De-Clutter** — runs automatically during indexing to prevent the AI from creating synonym keywords of ones already in your catalog.

See [Help: Keyword Deduplication and De-Clutter](Help-Keyword-Dedup-and-Declutter).

### Metadata Import and Retrieval
- **Import Metadata from Catalog** — syncs your existing Lightroom keyword/title/caption data into the backend before AI generation runs.
- **Retrieve Metadata from Backend** — pulls AI-generated metadata back into Lightroom if it was not written during indexing.

### Style Training
**Save Edits as AI Training Examples** reads your current develop settings and stores them on the backend as labeled examples. They are the sole input to AI Edit: the backend matches a photo against them and interpolates their settings into a recipe. Below five examples AI Edit refuses to run. See [Help: Train from Edits](Help-Train-From-Edits).

### Error Management
Batch tasks never fail silently. At the end of a run, a **Task Completion Dialog** aggregates successes and per-photo errors so you can see exactly what failed and why. See [Troubleshooting](Troubleshooting).

## Upgrade: UUID-era databases

If you are upgrading from an older version that stored Lightroom catalog UUIDs as primary IDs,
re-run **Analyze & Index Photos** over the catalog. There is no migration path — the one the
plugin used to offer posted to an endpoint the backend does not serve, and has been removed.
