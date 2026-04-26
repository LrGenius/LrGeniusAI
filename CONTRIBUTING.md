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

#### Backend (Python)
We use `uv` for dependency management.
- Install [uv](https://github.com/astral-sh/uv).
- Run the setup script:
  ```bash
  bash scripts/setup-local-uv-env.sh
  ```
- This will create a `.venv`, install dependencies, and set up the environment.

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
- Now, `ruff` (Python) and `stylua` (Lua) will run automatically on every commit.

---

## 🛠️ Development Guidelines

### General Rules
- **Error Handling**: All user-facing errors must be surfaced in the Lightroom GUI using `ErrorHandler.handleError`. Avoid silent failures.
- **Logging**:
    - **Plugin**: Use `log:error`, `log:warn`, `log:info`, and `log:trace`.
    - **Backend**: Use the configured `logger` and include `exc_info=True` for exceptions.
- **Infrastructure**: Update `Dockerfile` and `docker-compose-*.yml` when changing dependencies or environment requirements.

### Plugin Development (Lua)
- **Asynchronicity**: Long-running operations **must** run in `LrTasks.startAsyncTask`.
- **Yielding**: Use `LrTasks.pcall` instead of native `pcall` to allow for yielding during asynchronous operations.
- **Naming Conventions**: Top-level plugin actions should follow the `Task*.lua` naming convention.
- **Localization**: All GUI strings **must** be localized using the `LOC` function. Keep `TranslatedStrings_de.txt` (German) and `TranslatedStrings_fr.txt` (French) synchronized with English.
- **Utilities**: Use `Util.lua` for common logic.
- **Photo Identity**: Use `Util.getGlobalPhotoIdForPhoto` (metadata-based) for cross-catalog consistency.

### Backend Development (Python/Flask)
- **Structure**:
    - Endpoints: Use Flask Blueprints (`routes_*.py`).
    - Business Logic: Keep in the service layer (`service_*.py`).
- **API Response**: Return structured JSON with `results`, `error`, and `warning` fields.
- **Lifecycle**: Respect `server_lifecycle.py` for PID management.
- **Formatting**: Format code with `uv run ruff format` and ensure `server/scripts/lint_format.sh` passes.

---

## 📖 Documentation
- Wiki pages are located in `docs/wiki/`.
- Changes pushed to `main` automatically update the GitHub Wiki via `.github/workflows/publish-wiki.yml`.
- You can build wiki pages locally using `bash scripts/build-wiki-pages.sh`.

---

## ✅ Testing
- **Smoke Tests**: Run `TaskAutomatedTests.lua` within Lightroom to verify plugin-backend connectivity.
- **Backend Tests**: Run tests in `server/test/` using `pytest`.

---

## 📬 Pull Request Process
1. Create a new branch for your feature or bugfix: `git checkout -b feature/my-cool-feature`.
2. Commit your changes. Ensure pre-commit hooks pass.
3. Update `CHANGELOG.md` with a summary of your changes under the `[Unreleased]` section.
4. Push to your fork and open a Pull Request against the `main` branch.
5. Provide a clear description of the changes and how you verified them.

---

## 🌍 Translations
When adding or modifying user-facing strings, you **must** update all three translation files in the plugin directory:
- `TranslatedStrings_en.txt` (English)
- `TranslatedStrings_de.txt` (German)
- `TranslatedStrings_fr.txt` (French)

You can use the `sync_translations.py` script to help maintain consistency.

---

Thank you for contributing to LrGeniusAI! 📸✨
