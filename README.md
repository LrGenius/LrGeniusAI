<div align="center">
  <h1>🌟 LrGeniusAI</h1>
  <p><b>A smart Lightroom Classic plugin for AI-powered tagging, describing, semantic search, and develop edits.</b></p>
  
  [![Lua](https://img.shields.io/badge/Lua-2C2D72?style=for-the-badge&logo=lua&logoColor=white)]()
  [![Rust](https://img.shields.io/badge/Rust-000000?style=for-the-badge&logo=rust&logoColor=white)]()
  [![Python](https://img.shields.io/badge/Python-3776AB?style=for-the-badge&logo=python&logoColor=white)]()
  [![Website](https://img.shields.io/badge/Website-lrgenius.com-00B2FF?style=for-the-badge)]()
  [![Downloads](https://img.shields.io/github/downloads/LrGenius/LrGeniusAI/total?style=for-the-badge&label=Downloads)](https://github.com/LrGenius/LrGeniusAI/releases)
</div>

---

> **Note:** The lead developer is currently [taking a break from open-source development](https://blog.fokuspunk.de/posts/taking-a-break-from-open-source/). The project remains available, but active maintenance is paused.

---

## 📖 About the Project

**LrGeniusAI** brings the power of modern Large Language Models (LLMs) directly into Adobe Lightroom Classic. It analyzes your photos, automatically generates accurate tags and detailed descriptions, creates AI-guided Lightroom develop edit recipes, and lets you rediscover your library with semantic free-text search using natural language.

Whether you prefer running local models to ensure maximum privacy or want to leverage powerful cloud APIs, LrGeniusAI seamlessly adapts to your photography workflow.

---

## ✨ Core Features

- **🤖 AI-Powered Tagging & Describing:** Uses advanced LLMs to accurately recognize image content, generate metadata, and provide detailed descriptions of your photos.
- **🎛️ AI Lightroom Edit (Develop) *(beta)*:** Generates a structured Lightroom edit recipe per photo (global adjustments and optional masks) and can apply it directly in Develop mode. Includes per-photo review, style presets, style strength, composition/crop mode, and per-photo instruction overrides.
- **🔍 Semantic Free-Text Search (Advanced Search):** Find images naturally through descriptive queries (e.g., *"Red sports car parked in front of a garage"* or *"Sunset over the mountains"*). LrGeniusAI automatically creates a relevance-sorted Collection in Lightroom based on your prompt.
- **📸 Image Culling *(beta)*:** Group similar photos into bursts or near-duplicate stacks, automatically pick the strongest frames, and create Lightroom collections for picks, alternates, reject candidates, and optional duplicates.
- **👥 People & Faces:** Detect and cluster faces, assign names to persons, browse person collections, and find similar faces across your catalog.
- **🔎 Find Similar Images:** Find near-duplicate or visually similar photos for any selected image using perceptual hash or semantic CLIP comparison.
- **☁️ Local & Cloud Models:** Full support for local AI models via **Ollama** and **LM Studio**, as well as integration with cloud providers like **ChatGPT/OpenAI**, **Google Gemini**, and **Vertex AI**.
- **🎨 Customizable Prompts & Temperature Control:** System prompts for the AI can be added, edited, and deleted directly within the Lightroom Plug-In Manager. Use the temperature slider to control whether the AI should be highly creative or strictly consistent.
- **📝 Photo Context (Contextual Info):** Provide manual hints to the AI before analysis (e.g., names of people or specific background details) that aren't immediately obvious from the image itself. This can be done via a popup dialog or directly in Lightroom's metadata panel.
- **🗂️ Keyword Management:** Interactive synonym deduplication and automatic de-clutter during indexing to keep your keyword catalog clean.
- **🎓 Style Training:** Save your own Lightroom edits as AI training examples to teach the AI your personal editing style.
- **🗄️ Custom Backend & Database:** The plugin utilizes a high-performance local server (`geniusai-server`), currently being rewritten from Python to Rust for lower memory overhead. Existing metadata from your Lightroom catalog can easily be imported prior to the first AI analysis.

---

## 🚀 Installation & Getting Started

1. Download the latest release from the [GitHub Releases page](https://github.com/LrGenius/LrGeniusAI/releases).
2. Extract the ZIP file and add the plugin via the **Plug-in Manager** in Lightroom Classic.
3. **Backend Server Setup (First Launch):**
   - The backend starts automatically from Lightroom.
   - **Bypassing Security Warnings:** Because the installers are currently not code-signed, you will see warnings from **Windows SmartScreen** or **macOS Gatekeeper**.
     - **Windows:** Click *More info* -> *Run anyway*.
     - **macOS:** Right-click the `.pkg` -> *Open* -> *Open anyway*.
   - Optional troubleshooting: if you want to start it manually, run `lrgenius-server/lrgenius-server.cmd` on Windows or `lrgenius-server/lrgenius-server` on macOS.
4. Select photos in the library and choose one of the AI actions from **Library -> Plug-in Extras**:
   - **Analyze & Index Photos...** — AI tagging, descriptions, and search index
   - **AI Edit Photos...** *(beta)* — generate and apply Lightroom develop edits
   - **Advanced Search...** — semantic free-text search
   - **Cull Similar Photos...** *(beta)* — burst grouping and auto-ranking
   - **People...** — face clusters and named person collections
   - **Find Similar Images...** — find near-duplicates or visually similar photos
   - **Deduplicate Keyword Synonyms...** — clean up synonym sprawl in your catalog
5. For AI Edit, start with defaults, keep **Review each proposed edit before applying it** enabled, and tune style via **Overall look** + **Style strength**.

*For comprehensive details, model setup guides, and tips, please visit [lrgenius.com/help](http://lrgenius.com/help/).*

---

For detailed instructions on how to use Google Vertex AI, please see our [Google Vertex AI Login Wiki Page](https://github.com/LrGenius/LrGeniusAI/wiki/Google-Vertex-AI-Login).

## ⚖️ License

The LrGeniusAI core, plugin, and backend are released under the **GNU Affero General Public License v3 (AGPL-3.0)**. 

This project is built on the belief that AI tooling for creatives should remain open, transparent, and community-driven. See the [LICENSE](LICENSE) file for the full license text.


## 🛠️ Tech Stack

- **Frontend / Lightroom Plugin:** Lua (Lightroom SDK)
- **Backend / Server:** `geniusai-server` — being rewritten from Python (Flask / Waitress) to Rust (axum) for deterministic memory behavior; both currently live in this repo (`server/` and `server-rs/`) during the transition
- **AI & Embedding:** SigLIP2 via ONNX Runtime (PyTorch/Open-CLIP in the Python backend, `ort` crate in the Rust backend)
- **Identity & Faces:** InsightFace (ONNX)
- **Database:** ChromaDB + SQLite (Python backend) / LanceDB (Rust backend)
- **Supported Interfaces:** Google Gemini, Vertex AI, ChatGPT/OpenAI, Ollama, LM-Studio


---

## 🛠️ Development

For more detailed information on how to contribute, please see our [CONTRIBUTING.md](CONTRIBUTING.md).


## 🤝 Credits

Developed with a passion for photography and IT by:

- **Bastian Machek (LrGenius / Fokuspunk)** – *Creator & Lead Developer*
- **Community** – *Special thanks to all contributors and testers for your valuable input and support.*
- **Various AI agents** - *For the great support in developing this project.*

This project leverages many incredible open-source libraries, including **InsightFace**, **OpenCLIP**, **PyTorch**, **Hugging Face Transformers**, **ChromaDB**, and **Flask**. 

A huge thank you to the open-source community and the developers of the underlying AI frameworks that make this integration possible!
