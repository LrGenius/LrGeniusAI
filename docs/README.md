# Documentation Setup

This repository uses a docs-as-code workflow for GitHub Wiki publishing.

## Source of truth

- Wiki source pages live in `docs/wiki/`
- Any file ending with `.md` in that folder is published to the GitHub Wiki
- `docs/wiki/Home.md` becomes the wiki home page
- Additional generated pages are created from repository READMEs:
  - `Dev-Project-README.md` from `/README.md`
  - `Dev-Plugin-README.md` from `/plugin/README.md`
  - `Dev-Server-README.md` from `/server-rs/README.md`

## What lands on lrgenius.com

The website ([LrGenius/lrgenius.github.io](https://github.com/LrGenius/lrgenius.github.io))
pulls `docs/wiki/` at build time and publishes **every page except**:

- `Dev-*` — developer documentation, wiki-only
- `Home` — the site has its own `/help` index
- `_*` — wiki special pages (`_Sidebar`, `_Footer`)

So a new user-facing page goes live at `lrgenius.com/help/docs/<lowercase-name>`
by itself, and a page that should *not* be public needs a `Dev-` prefix. Give
each page an `# H1` — the site uses it as the page title. Pages not placed in a
curated group in the site's `src/pages/help/index.astro` are listed under "More
Guides"; moving one into a proper group is a change in that repo.

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
