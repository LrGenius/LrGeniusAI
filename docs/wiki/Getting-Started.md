# Getting Started

Welcome to LrGeniusAI! This guide will walk you through setting up the plugin, indexing your first batch of photos, and starting your AI-powered Lightroom workflow.

## 1. Install Plugin and Server

To begin, you must install both the Lightroom Classic plugin frontend and the backend server (`geniusai-server`, a single Rust binary). These components communicate locally to process your images without freezing the Lightroom UI. 
Please refer to the high-level installation instructions on the [root `README.md`](Dev-Project-README) or the detailed steps in the [`plugin/README.md`](Dev-Plugin-README).

### ⚠️ Bypassing Security Warnings (Unsigned Installers)

Because LrGeniusAI is an open-source project and the current installers are not code-signed, your operating system will likely flag them as "untrusted" or "malicious". This is a standard security precaution for any third-party software that has not been notarized by Microsoft or Apple.

#### Windows (SmartScreen)
When you run the installer or the backend `.cmd` file, you may see a "Windows protected your PC" dialog.
1. Click **More info**.
2. Click **Run anyway**.

#### macOS (Gatekeeper)
When you try to open the `.pkg` installer or the backend binary:
1. **Right-click** (or Control-click) the file in Finder.
2. Select **Open** from the menu.
3. In the dialog that appears, click **Open** again.
4. If it still fails, go to `System Settings -> Privacy & Security`, scroll down to the "Security" section, and click **Open Anyway**.

---

## 2. Configure Plugin

Once installed, open the **Lightroom Plug-in Manager** (`File -> Plug-in Manager`) and locate LrGeniusAI. Here you need to:
- **Set the Backend Server URL:** This defaults to `http://127.0.0.1:19819` but if you're running the backend on a different machine (e.g. via Docker), update the address here.
- **Configure Provider/API Keys:** If you plan to use cloud providers like OpenAI or Google Gemini, enter your API keys. For external local servers like Ollama or LM Studio, ensure their respective base URLs are correctly configured.
- ~~**Set Vertex AI Details:** If using Google Cloud's Vertex AI, provide your project ID and preferred location.~~ **Removed** — the Vertex AI fields no longer exist in the Plug-in Manager (see [section 7](#7-vertex-ai-login--removed)).

*Having trouble? Refer to the [Troubleshooting](Troubleshooting) guide for connectivity and API issues.*

### Download the on-device AI models

Some features do not use a cloud model at all and instead run their own model on
your machine: smart photo search, and species identification for animals,
plants and fungi. Those model files are not part of the installer, so fetch them
once before you index anything.

In the same settings dialog, scroll to **On-device AI models** and click
**Download AI models**. One button fetches everything that is missing — there is
nothing to choose per feature — and the per-model indicators next to it turn
green as each family lands. Expect roughly 3 GB in total on a fresh install.

The same button appears in the first-run setup wizard. It is also how you finish
setting up after an upgrade: families already on disk are skipped, so an
existing installation that only had the search model downloads just the species
model.

Until a model is on disk, the feature that needs it stays greyed out in the
Analyze & Index dialog rather than failing at run time.

### Prefer to stay fully local? (no API key, no extra app)

The backend can run vision models itself. In the same settings dialog, scroll to
the **Local AI Model** sections:

- **Local AI Model — MLX** on macOS.
- **Local AI Model (no external app)** — the built-in llama.cpp engine — on
  Windows.

Each platform ships exactly one of them, so you will only ever see the section
that applies to your machine.

Pick a model (start with **Gemma 4 E4B**), click **Download**, and wait for the
**Installed** line to list it. It then appears in the **AI Model** dropdown of
every task as `mlx: …` or `llamacpp: …`. Full guide:
[Local AI Models](Help-Local-AI-Models).

## 3. Index Photos

Before semantic search or AI-assisted culling can work, the backend needs to process ("index") your photos.
1. Select one or more photos in your Lightroom Library grid.
2. Navigate to `Library -> Plug-in Extras -> Analyze & Index Photos`.
3. The plugin will pass the photos to the backend, generate descriptions, tags, and AI embeddings, and store them.
4. Optional, and off until you tick it: **Identify animal and plant species**
   runs BioCLIP 2 on-device and writes a taxonomic identification into the
   plugin's metadata panel. Keep its *Only where an animal or plant is detected*
   sub-option on for a general library — it skips the portraits and landscapes
   before the expensive model runs. See
   [Help: Analyze and Index](Help-Analyze-and-Index#species-identification).

Once indexing finishes, try out **Advanced Search**, the **People** workflows, or use **Retrieve Metadata** to inject the generated tags straight back into your catalog. For **AI Edit Photos** *(beta)*, first save a handful of your own edits with **Save Edits as AI Training Examples** — that is what AI Edit builds develop settings from.

## 4. Upgrading From a UUID-Era Database

*Only relevant if you are upgrading from a version of LrGeniusAI that predates
file-based `photo_id` values.*

Those versions keyed everything on Lightroom catalog UUIDs, which the backend
cannot match against your photos any more. There is no migration — the one the
plugin used to offer never worked and has been removed. Run
**Analyze & Index Photos** over the catalog again; photos already indexed under
the current IDs are skipped.

## 5. Run Culling on Similar Photos

After indexing your photos, you can automate the process of picking the best shots from bursts or removing near-duplicates:
1. Select the group of photos you want to cull, or leave it empty to use the current folder view.
2. Open `Library -> Plug-in Extras -> Cull Similar Photos`.
3. Choose a culling preset (e.g., `default` or `sports`) depending on how aggressive you want the AI to be.
4. Wait for the backend to group and analyze your photos. 
5. LrGeniusAI will rapidly create a time-stamped Collection Set in Lightroom containing `Picks`, `Alternates`, `Reject Candidates`, and `Duplicates`. Your view will automatically switch to the `Picks` collection so you can review the best shots right away.

## 6. Create a DB Backup

We highly recommend creating regular backups of your backend data, especially before migrations, moving to a new server, or performing maintenance.
1. Open `File -> Plug-in Manager`.
2. Navigate to `Backend Server` and click **Download DB backup**.
3. Save the resulting `.zip` file somewhere safe. The backup contains the full persistent backend directory including your embeddings and metadata databases.

## 7. Vertex AI Login — REMOVED

> **⚠️ Vertex AI was removed from LrGeniusAI in August 2026.** The plugin no longer exposes
> Vertex AI project/location settings, Vertex embeddings, or *Semantic (Vertex AI)* search,
> so this step is no longer needed. It is kept here for reference only.

For users of Google's Vertex AI, you need to use Google Cloud ADC (Application Default Credentials) on the host running the server.

From your server terminal:
```bash
gcloud init
gcloud config set project YOUR_PROJECT_ID
gcloud auth application-default login
```

If your backend is running in the remote Docker Compose environment:
```bash
mkdir -p gcloud
docker compose up -d --build
docker compose exec geniusai-server gcloud config set project YOUR_PROJECT_ID
docker compose exec geniusai-server gcloud auth application-default login
```

For headless servers without a GUI/browser:
```bash
docker compose exec geniusai-server gcloud auth application-default login --no-browser
```
The `./gcloud:/root/.config/gcloud` bind mount keeps your ADC credentials intact between container restarts.

## 8. Next Steps

- [FAQ](FAQ) — quick answers to common questions
- [Help: Analyze and Index](Help-Analyze-and-Index)
- [Help: AI Edit Photos](Help-AI-Edit) *(beta)*
- [Help: Advanced Search](Help-Advanced-Search)
- [Help: Cull Photos](Help-Cull-Photos) *(beta)*
- [Help: People & Faces](Help-People-Faces)
- [Help: Find Similar Images](Help-Find-Similar)
- [Help: Keyword Deduplication and De-Clutter](Help-Keyword-Dedup-and-Declutter)
- [Help: Choosing AI Model](Help-Choosing-AI-Model)
- [Help: Local AI Models](Help-Local-AI-Models) — built-in llama.cpp and MLX engines
- [Help: Ollama Setup](Help-Ollama-Setup)
- [Help: LM Studio Setup](Help-LM-Studio-Setup)
- [Troubleshooting](Troubleshooting)
