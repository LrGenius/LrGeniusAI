# Contributing to LrGeniusAI

Welcome! We're excited that you're interested in contributing to **LrGeniusAI**. This project aims to bring powerful AI capabilities to Adobe Lightroom Classic, and your help is vital to making it better for everyone.

By contributing, you agree to abide by the terms of our [LICENSE](LICENSE) (AGPL-3.0).

---

## 🚀 Getting Started

### 1. Fork and Clone
- Fork the repository on GitHub.
- Clone your fork locally:
  ```bash
  git clone https://github.com/YOUR_USERNAME/LrGeniusAI.git
  cd LrGeniusAI
  ```

### 2. Set Up the Development Environment

#### Backend (Rust)
Standard cargo workspace, no extra tooling beyond `rustup`/`cargo`.
```bash
cd server-rs
cargo build --release -p lrg-server
```

The two **local inference engines** are opt-in for exactly this reason — nobody
working on an unrelated crate should pay for their toolchains:

- **llama.cpp** sits behind the `llamacpp` cargo feature (off by default), since
  it compiles llama.cpp from source: needs `cmake` + `libclang`, and the Vulkan
  SDK on Windows. Build with `--features llamacpp`, and keep
  `cargo clippy --workspace --all-targets --features llamacpp` clean when you
  touch that path.
- **MLX** (Apple silicon) needs no cargo feature, but does need the
  `lrgenius-mlx` sidecar built with `xcodebuild` plus the Metal toolchain.

See [`server-rs/README.md`](server-rs/README.md) and
[`native/mlx-sidecar/README.md`](native/mlx-sidecar/README.md) for the exact
commands and the environment variables that point the server at models.

#### Plugin (Lua)
- The plugin code is located in the `plugin/LrGeniusAI.lrdevplugin` directory.
- To test changes, you can link this directory into your Lightroom `Modules` folder or add it via the Lightroom **Plug-in Manager**.

#### Pre-commit Hooks
To ensure code consistency, we use `pre-commit` for automatic formatting and linting.
- Install `pre-commit`: `uv tool install pre-commit` (or `brew install pre-commit`).
- Install the git hooks:
  ```bash
  pre-commit install
  ```
- Now, `stylua` (Lua) and `cargo fmt`/`cargo clippy` (Rust, in `server-rs/`) will run automatically on every commit.

---

## 🛠️ Development Guidelines

### General Rules
- **Error Handling**: All user-facing errors must be surfaced in the Lightroom GUI using `ErrorHandler.handleError`. Avoid silent failures.
- **Logging**:
    - **Plugin**: Use `log:error`, `log:warn`, `log:info`, and `log:trace`.
    - **Backend**: Use the `log` facade (`log::info!`/`warn!`/`error!`).
- **Infrastructure**: Update `Dockerfile` and `docker-compose-*.yml` when changing dependencies or environment requirements.

### Plugin Development (Lua)
- **Asynchronicity**: Long-running operations **must** run in `LrTasks.startAsyncTask`.
- **Yielding**: Use `LrTasks.pcall` instead of native `pcall` to allow for yielding during asynchronous operations.
- **Naming Conventions**: Top-level plugin actions should follow the `Task*.lua` naming convention.
- **Localization**: All GUI strings **must** go through the `LOC` function. The plugin ships no translation files, so the default string inside the `LOC()` call is what users read — write it as finished English.
- **Utilities**: Use `Util.lua` for common logic.
- **Photo Identity**: Use `Util.getGlobalPhotoIdForPhoto` (metadata-based) for cross-catalog consistency.

### Backend Development (Rust)
- **Structure**:
    - Routes: `lrg-api::routes`, one axum `Router` per domain.
    - Business Logic: `lrg-analysis`/`lrg-imaging`/`lrg-ml`; LLM/cloud clients in `lrg-providers`.
- **API Response**: Return structured JSON with `results`, `error`, and `warning` fields.
- **Lifecycle**: PID file + "OK" signal file handshake, handled in `lrg-common`.
- **Formatting**: `cargo fmt` and `cargo clippy --workspace --all-targets` must be clean (zero warnings).

---

## 📖 Documentation
- Wiki pages are located in `docs/wiki/`.
- Changes pushed to `main` automatically update the GitHub Wiki via `.github/workflows/publish-wiki.yml`.
- You can build wiki pages locally using `bash scripts/build-wiki-pages.sh`.

---

## ✅ Testing
- **Smoke Tests**: Run `TaskAutomatedTests.lua` within Lightroom to verify plugin-backend connectivity.
- **Backend Tests**: `cd server-rs && cargo test --workspace`.

---

## 📬 Pull Request Process
1. Create a new branch for your feature or bugfix: `git checkout -b feature/my-cool-feature`.
2. Commit your changes. Ensure pre-commit hooks pass.
3. Push to your fork and open a Pull Request against the `main` branch.
4. Provide a clear description of the changes and how you verified them.
5. Say in one sentence what changes for someone *using* the plugin (or "no user-visible change"). The release notes are generated from PR titles and descriptions — see [Release notes](#-release-notes) below.

---

## 📝 Release notes
Nobody writes the release notes by hand. When a `v*` tag is pushed, the release
workflow runs `scripts/generate_release_notes.py`, which collects every pull
request in the release and sorts it into **New**, **Improved** and **Fixed**
sections for the photographers who install the plugin — with the usual list of
PR titles kept in a collapsed section below.

**Your PR title becomes the release-note line, so write it for a photographer.**
`fix(plugin): apply top-level keyword when hierarchy is disabled` becomes
"Apply top-level keyword when hierarchy is disabled" under **Fixed** — the
`type(scope):` prefix is stripped, nothing else is rewritten. A title that only
makes sense to someone who knows the code produces a note that only makes sense
to them too.

How a PR is sorted:

| Signal | Result |
| --- | --- |
| Label `feature` / `enhancement`, or a `feat:` title | **New** |
| Label `bug` / `fix`, or a `fix:` title | **Fixed** |
| Label `performance` / `improvement`, or a `perf:` title | **Improved** |
| Label `documentation` / `chore` / `ci` / `build` / `test` / `refactor` / `dependencies`, or the matching title prefix | Left out of the user-facing part |
| An internal scope — `fix(ci):`, `chore(deps):`, `fix(release):` | Left out, whatever the type says |
| Anything else | **Other changes** |

Labels win over title prefixes, so a mislabelled prefix can always be corrected
by labelling the PR. Purely internal work is deliberately left out of the
user-facing sections — it still appears in the collapsed technical changelog, so
there is no need to dress it up.

Releases are created as **drafts**, so the generated text can be read and edited
before publishing. To preview the notes for an existing tag:

```bash
GITHUB_TOKEN=<a token with repo read access> \
  python3 scripts/generate_release_notes.py --repo LrGenius/LrGeniusAI --tag v2.20.1 --output -
```

---

Thank you for contributing to LrGeniusAI! 📸✨
