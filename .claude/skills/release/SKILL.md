---
name: release
description: Walks a maintainer through cutting an LrGeniusAI release — pre-flight on main, the release-note sorting of the merged PRs, the v* tag (the tag is the version), watching release.yml build the Windows/macOS installers, the plugin zip and the Docker images, and reviewing the draft release and its update manifest before the maintainer publishes it. Use when asked to release, tag, ship or publish a version, or to check why a release build failed.
---

# Cut a release

**The tag is the version.** Nothing in the repository is bumped by hand: the
checked-in `Info.lua` keeps the placeholder `9.9.9` (build `99991212`), and
CI stamps the real numbers from the tag (`scripts/set_plugin_version.sh`
rewrites `Info.MAJOR/MINOR/REVISION/BUILD`; the backend gets
`LRG_BACKEND_VERSION`, `LRG_BACKEND_RELEASE_TAG` and `LRG_BACKEND_BUILD` at
build time, read by `option_env!` in `lrg-common/src/version.rs`). The plugin
and the backend must then carry the same version: `POST /version/check`
compares them exactly.

Pushing a tag starts a public release build and cannot be taken back quietly.
**Ask the maintainer before pushing it, and never publish the draft
release yourself** — that is their call after reading it.

## 1. Pre-flight

- The commit to release is on `main` and pushed. `validate-tag` in
  `.github/workflows/release.yml` fails a tag that `origin/main` does not
  contain.
- CI on that commit is green (lint-format, lua-tests, server-rs-tests).
- The signing secrets are in place: `SIGNPATH_API_TOKEN` for Windows, and the
  Apple secrets all or none (a partial set fails `validate-tag`).
- The previous tag, for the version number and the notes:
  `git describe --tags --abbrev=0 origin/main`.

## 2. Read what goes into the notes

The release notes are generated from the PRs since the previous tag
(`scripts/generate_release_notes.py`; the rules are under *Release notes* in
`CONTRIBUTING.md`): the PR title becomes the line, and labels or the
`feat:`/`fix:`/`perf:` prefix decide the section. List the merged PRs since
the last tag (`gh pr list --state merged --base main --search "merged:>=<date>"`)
and point out titles that only make sense to a developer, and user-facing PRs
that a `chore:`/`ci:` prefix or label would hide. Fixing a label on GitHub is
enough; nothing in the repository changes.

The tracked `release_notes.md` at the repository root is not this text — CI
writes the real one on the runner. Leave it alone.

## 3. Pick the version and tag

- `vMAJOR.MINOR.PATCH`; a suffix with `-` (`v2.21.0-beta.1`) makes a
  prerelease.
- **Breaking** (the self-updater refuses the update and sends users to the
  full installer) is detected
  from changes to `server-rs/Cargo.toml` / `Cargo.lock` since the previous tag
  (`scripts/generate_update_manifest.py`). If that guess is wrong, say so
  before tagging: `release.yml` can be started by hand with the
  `breaking_changes` input instead.
- With the maintainer's go-ahead:

```bash
git fetch origin && git checkout main && git pull --ff-only
git tag vX.Y.Z && git push origin vX.Y.Z
```

## 4. Watch the build

`gh run watch` on the run `release.yml` started for the tag. The jobs:

| Job | Makes |
|---|---|
| `validate-tag` | the checks from step 1 |
| `build-and-release` | Windows x64 (`--features llamacpp`, Rust 1.99) and macOS arm64 (MLX only, no llamacpp) installers |
| `build-plugin-only` | the plugin zip for a Docker backend |
| `build-docker-images` | the images, through `docker-images.yml` |
| `create-release` | the update manifest, the notes, a **draft** release with the assets |
| `publish-docker-manifest` | `vX.Y.Z` and `latest` (plus `-vulkan`, `-cuda`) once the release exists |

On a failure, read the failing step's log (`gh run view --log-failed`), say
which job and why, and propose a fix as a normal PR to `main`. A fixed release
needs a **new** tag — never move or re-push an existing one.

## 5. Review the draft

Open the draft (`gh release view vX.Y.Z`) and check, then report to the
maintainer:

- the assets: the macOS `.pkg`, the Windows `.exe`, the Docker-backend plugin
  zip, the raw self-update binaries (`update-assets/`) and
  `update-manifest-vX.Y.Z.json`;
- the manifest: `version`, `breaking_changes`, both `platforms` with a sha256
  each, and `plugin_files` — the self-updater (`POST /update/apply`,
  `routes/update.rs`) reads exactly these keys;
- the notes: every user-facing PR under **New**, **Improved** or **Fixed**,
  written for a photographer.

The maintainer edits the text if needed and publishes. Afterwards, the docs
that name a version (installation steps, the wiki home) may need a follow-up
PR — check them with `grep` for the previous version.
