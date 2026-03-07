# Help: Analyze and Index

> Migrated from `lrgenius.com/help` and curated for repo docs.  
> Screenshot references were intentionally removed.

## Start the task

In Lightroom Classic:

- `Library -> Plug-in Extras -> Analyze & Index photos`

## Scope options

- `New or unprocessed photos`
- `All photos in catalog`
- `Current view` (folder/collection currently open)
- `Selected photos only`

## Processing options

- `Regenerate all data (overwrite existing)`
  - If disabled, only missing data is generated.
- `Import metadata from catalog before indexing`
  - Imports existing Lightroom metadata into backend DB before AI generation.
- `Generate AI metadata`
  - Generates `keywords`, `title`, `caption`, `alt_text` (based on your toggles).
- `Create search embeddings`
  - Required for semantic search.

## Metadata behavior and structure

- `Use top-level keyword`
  - Places generated keywords under one top-level keyword.
- `Use keyword structure from Lightroom catalog`
  - Uses existing keyword hierarchy from catalog as category source.
  - Use carefully for large/complex hierarchies.
- `Use hierarchical keywords`
  - Enables category-based keyword output.

## Extra context options

- `Show Photo Context Dialog`
- `Folder names`
- `Capture Date/Time`
- `Existing Keywords`
- `GPS coordinates`

These fields are passed as additional context for metadata generation.

## Related tasks

- `Retrieve metadata from backend`
  - Re-apply generated metadata if it was not saved directly to catalog.
- `Import metadata from catalog`
  - Sync Lightroom metadata into backend DB.
