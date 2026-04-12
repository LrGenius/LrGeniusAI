# Getting Started

Welcome to LrGeniusAI! This guide will walk you through setting up the plugin, indexing your first batch of photos, and starting your AI-powered Lightroom workflow.

## 1. Install Plugin and Server

To begin, you must install both the Lightroom Classic plugin frontend and the Python backend server. These components communicate locally to process your images without freezing the Lightroom UI. 
Please refer to the high-level installation instructions on the [root `README.md`](Project-README) or the detailed steps in the [`plugin/README.md`](Plugin-README).

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
- **Set the Backend Server URL:** This defaults to `http://127.0.0.1:8000` but if you're running the backend on a different machine (e.g. via Docker), update the address here.
- **Configure Provider/API Keys:** If you plan to use cloud providers like OpenAI or Google Gemini, enter your API keys. For local providers like Ollama or LM Studio, ensure their respective base URLs are correctly configured.
- **Set Vertex AI Details:** If using Google Cloud's Vertex AI, provide your project ID and preferred location.

*Having trouble? Refer to the [Troubleshooting](Troubleshooting) guide for connectivity and API issues.*

## 3. Index Photos

Before semantic search or AI-assisted culling can work, the backend needs to process ("index") your photos.
1. Select one or more photos in your Lightroom Library grid.
2. Navigate to `Library -> Plug-in Extras -> Analyze & Index Photos`.
3. The plugin will pass the photos to the backend, generate descriptions, tags, and AI embeddings, and store them.

Once indexing finishes, try out **Advanced Search**, the **People** workflows, or use **Retrieve Metadata** to inject the generated tags straight back into your catalog.

## 4. Run One-Time ID Migration (Upgrade Path)

*Note: This step is only relevant if you are upgrading from an older version of LrGeniusAI.*

If your previous database relied on Lightroom catalog UUIDs, you must migrate to the new `photo_id` system:
1. Open `File -> Plug-in Manager`.
2. Open LrGeniusAI settings.
3. In the `Backend Server` section, click **Migrate existing DB IDs to photo_id**.
4. Wait for the progress dialog to complete. This ensures you do not lose any previously generated metadata or semantic search indexes.

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

## 7. Vertex AI Login

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

## 8. Imported Help Pages

For further reading, we've migrated several curated guides from the project website:
- [Help: Analyze and Index](Help-Analyze-and-Index)
- [Help: Advanced Search](Help-Advanced-Search)
- [Help: Choosing AI Model](Help-Choosing-AI-Model)
- [Help: Ollama Setup](Help-Ollama-Setup)
- [Help: LM Studio Setup](Help-LM-Studio-Setup)
