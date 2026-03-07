# Plugin Guide

## Main documentation

The plugin documentation is maintained in:

- `plugin/README.md`

## Covers

- Analyze and Index
- Advanced Search
- Metadata import/retrieval
- People and face workflows
- Vertex AI and gcloud setup
- Migration from legacy UUID IDs to `photo_id`

## Upgrade note

`photo_id` migration is a required one-time step for existing databases from older releases.

Use:

- Plugin Manager -> Backend Server -> **Migrate existing DB IDs to photo_id**
