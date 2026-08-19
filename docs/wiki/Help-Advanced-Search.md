# Help: Advanced Search

Advanced Search finds photos by description rather than by filename or keyword —
"red sports car in front of a garage" instead of remembering which shoot it was
in. Results land in a new Lightroom collection.

## Open advanced search

In Lightroom Classic:

- `Library → Plug-in Extras → Advanced Search...`

## What the search can see

A text search only finds photos that carry a SigLIP embedding, so a
half-indexed catalog answers "no results" for a subject that is sitting right
there. The dialog says so before you type: if the search model is not
downloaded, or some of the catalog has no search index yet, an orange note at
the top gives the count and what to run to fix it.

The check is two cheap lookups (`/db/stats` plus the catalog's photo count) and
runs every time the dialog opens. It is a note, not a gate — the search still
runs across whatever *is* indexed.

## Search term

Free text. Describe what is in the photo, not how it was shot:

- `red sports car in front of a garage`
- `sunset over mountains with orange sky`
- `two people laughing at a wooden table`

## Search scope

- **All photos** — everything indexed in the backend
- **Current view** — the folder or collection currently open
- **Selected photos** — the current selection only

## Tuning

### Relevance strictness (0–100)

How aggressively weak matches are dropped. `0` turns filtering off and returns
the full ranked list; `50` is moderate; `100` keeps only strong matches. The
filter looks for the point where result scores fall off a cliff and cuts there,
so a strict setting on a query with many good matches still returns many
results.

### Max results (50–1000)

Upper bound on how many photos end up in the collection, across both halves of
the search. Default 100.

## Search in

At least one of these has to be on:

- **Semantic (SigLIP / local AI)** — matches the meaning of the query against
  the image embedding. Requires the photos to have been indexed with
  *Create search embeddings*, and the SigLIP model to be downloaded.
- **Metadata** — plain substring matching against the AI-generated fields.
  Each field can be switched on separately: **Keywords**, **Caption**,
  **Title**, **Alt text**. Photos need AI metadata for this to find anything.

Semantic matches are ranked by similarity and come first; metadata matches fill
whatever is left of **Max results**.

> Semantic search over Vertex AI embeddings still exists in the backend but was
> removed from the plugin UI in August 2026. Existing `VERTEX_TABLE` rows are
> kept and no longer queried.

## Results

The plugin creates a collection under the **Search Results** collection set,
named after the query and scope.

**Set the collection sort order to `Custom Order`** — that is the only order in
which Lightroom shows the best matches at the top. Any other sort order throws
the ranking away.
