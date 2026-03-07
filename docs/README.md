# Documentation Setup

This repository uses a docs-as-code workflow for GitHub Wiki publishing.

## Source of truth

- Wiki source pages live in `docs/wiki/`
- Any file ending with `.md` in that folder is published to the GitHub Wiki
- `docs/wiki/Home.md` becomes the wiki home page
- Additional generated pages are created from repository READMEs:
  - `Project-README.md` from `/README.md`
  - `Plugin-README.md` from `/plugin/README.md`
  - `Server-README.md` from `/server/README.md`

## Automated publishing

The workflow `.github/workflows/publish-wiki.yml` publishes docs to the repository wiki:

- Triggered on push to `main` when docs or README files change
- Can also be started manually via `workflow_dispatch`

## Local test

You can run the publisher script manually if you have push access:

```bash
bash scripts/publish-wiki.sh
```

To only regenerate README-derived wiki pages:

```bash
bash scripts/build-wiki-pages.sh
```

Required env variables:

- `GITHUB_REPOSITORY` (for example `LrGenius/LrGeniusAI`)
- `GITHUB_TOKEN` with write access to the wiki
