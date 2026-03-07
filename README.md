<div align="center">
  <h1>🌟 LrGeniusAI</h1>
  <p><b>A smart Lightroom Classic plugin for AI-powered tagging, describing, and semantic image search.</b></p>
  
  [![Lua](https://img.shields.io/badge/Lua-2C2D72?style=for-the-badge&logo=lua&logoColor=white)]()
  [![Python](https://img.shields.io/badge/Python-3776AB?style=for-the-badge&logo=python&logoColor=white)]()
  [![Website](https://img.shields.io/badge/Website-lrgenius.com-00B2FF?style=for-the-badge)]()
</div>

---

## 📖 About the Project

**LrGeniusAI** brings the power of modern Large Language Models (LLMs) directly into Adobe Lightroom Classic. It analyzes your photos, automatically generates accurate tags and detailed descriptions, and lets you rediscover your library with a semantic free-text search using natural language.

Whether you prefer running local models to ensure maximum privacy or want to leverage powerful cloud APIs, LrGeniusAI seamlessly adapts to your photography workflow.

---

## ✨ Core Features

- **🤖 AI-Powered Tagging & Describing:** Uses advanced LLMs to accurately recognize image content, generate metadata, and provide detailed descriptions of your photos.
- **🔍 Semantic Free-Text Search (Advanced Search):** Find images naturally through descriptive queries (e.g., *"Red sports car parked in front of a garage"* or *"Sunset over the mountains"*). LrGeniusAI automatically creates a relevance-sorted Collection in Lightroom based on your prompt.
- **☁️ Local & Cloud Models:** Full support for local AI models via **Ollama** and **LM Studio**, as well as integration with cloud providers like **Google Gemini** and **Vertex AI**.
- **🎨 Customizable Prompts & Temperature Control:** System prompts for the AI can be added, edited, and deleted directly within the Lightroom Plug-In Manager. Use the temperature slider to control whether the AI should be highly creative or strictly consistent.
- **📝 Photo Context (Contextual Info):** Provide manual hints to the AI before analysis (e.g., names of people or specific background details) that aren't immediately obvious from the image itself. This can be done via a popup dialog or directly in Lightroom's metadata panel.
- **🗄️ Custom Python Backend & Database:** The plugin utilizes a high-performance local server (`geniusai-server`). Existing metadata from your Lightroom catalog can easily be imported prior to the first AI analysis.

---

## 🚀 Installation & Getting Started

1. Download the latest release from the [GitHub Releases page](https://github.com/LrGenius/LrGeniusAI/releases).
2. Extract the ZIP file and add the plugin via the **Plug-in Manager** in Lightroom Classic.
3. **Backend Server Setup (First Launch):**
   - **Windows:** Navigate to the `lrgenius-server` folder and run `lrgenius-server.exe`. If a SmartScreen warning appears, click *More info -> Run anyway*.
   - **macOS:** Open the Terminal, navigate to the extracted folder, and run the following commands to bypass Gatekeeper restrictions:
     ```bash
     chmod +x lrgenius-server/lrgenius-server
     xattr -dr com.apple.quarantine lrgenius-server
     ```
4. Select your photos in the library and choose from the menu: **Library -> Plug-in Extras -> Analyze & Index photos**.

*For comprehensive details, model setup guides, and tips, please visit [lrgenius.com/help](http://lrgenius.com/help/).*

---

## ⚠️ Breaking Change: `photo_id` Migration

Recent versions switched the backend identity key from Lightroom catalog UUIDs to file-based `photo_id` values.
The stable ID algorithm was later adjusted to remain stable when metadata is written to files (for example DNG updates).

If you upgrade from an older version, run a one-time migration to keep existing index/search data usable.

### Recommended migration path (from Lightroom)

1. Open `File -> Plug-in Manager`
2. Open LrGeniusAI settings
3. In `Backend Server`, click **Migrate existing DB IDs to photo_id**
4. Wait until the progress dialog is complete

The migration updates all relevant backend collections:

- main image embeddings
- vertex embeddings
- face/person references

### Identity scope note

The current `photo_id` / hash / derived `canonicalId` strategy is more stable than Lightroom catalog UUIDs, but it is still not guaranteed to be 100% cross-catalog safe in every workflow.

Treat backend identity as best-effort and primarily catalog-scoped for now, especially when:

- the same files exist in multiple Lightroom catalogs
- files were duplicated, re-exported, or rewritten outside Lightroom
- the plugin had to fall back to partial file hashes because stable metadata IDs were unavailable

If strict cross-catalog identity is important for your workflow, plan for re-indexing or migration checks when moving photos between catalogs or restoring older databases.

---

## ☁️ Google Vertex AI Login (gcloud)

If you want to use Vertex AI with LrGeniusAI, run the login on the machine where `geniusai-server` is running.

### macOS

1. Install Google Cloud CLI (if needed):  
   [https://cloud.google.com/sdk/docs/install](https://cloud.google.com/sdk/docs/install)
2. Open Terminal and run:

```bash
gcloud init
gcloud config set project YOUR_PROJECT_ID
gcloud auth application-default login
```

3. Optional verification:

```bash
gcloud auth application-default print-access-token
```

### Windows (PowerShell)

1. Install Google Cloud CLI (if needed):  
   [https://cloud.google.com/sdk/docs/install](https://cloud.google.com/sdk/docs/install)
2. Open **Google Cloud SDK Shell** (or PowerShell with `gcloud` in PATH) and run:

```powershell
gcloud init
gcloud config set project YOUR_PROJECT_ID
gcloud auth application-default login
```

3. Optional verification:

```powershell
gcloud auth application-default print-access-token
```

### Notes

- `gcloud auth application-default login` creates local Application Default Credentials (ADC).
- Set `Vertex AI Project ID` and `Vertex AI Location` in the Lightroom plugin settings.
- For headless/server deployments, prefer service-account auth via `GOOGLE_APPLICATION_CREDENTIALS`.

---

## 🛠️ Tech Stack

- **Frontend / Lightroom Plugin:** Lua
- **Backend / Server:** Python (`geniusai-server`)
- **AI & Embedding:** Open-CLIP
- **Supported Interfaces:** Gemini, Vertex AI, Ollama, LM-Studio

---

## 🤝 Credits

Developed with a passion for photography and IT by:

- **Bastian Machek (LrGenius / Fokuspunk)** – *Creator & Lead Developer*
- **AI agents**

A huge thank you to the open-source community and the developers of the underlying AI frameworks that make this integration possible!
