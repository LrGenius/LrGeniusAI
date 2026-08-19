<div align="center">
  <h1>🌟 LrGeniusAI</h1>
  <p><b>A smart Lightroom Classic plugin for AI-powered tagging, describing, semantic search, and develop edits.</b></p>
  
  [![Lua](https://img.shields.io/badge/Lua-2C2D72?style=for-the-badge&logo=lua&logoColor=white)]()
  [![Rust](https://img.shields.io/badge/Rust-000000?style=for-the-badge&logo=rust&logoColor=white)]()
  [![Website](https://img.shields.io/badge/Website-lrgenius.com-00B2FF?style=for-the-badge)]()
  [![Downloads](https://img.shields.io/github/downloads/LrGenius/LrGeniusAI/total?style=for-the-badge&label=Downloads)](https://github.com/LrGenius/LrGeniusAI/releases)

  [![ko-fi](https://ko-fi.com/img/githubbutton_sm.svg)](https://ko-fi.com/W7X2240HF4)
</div>


---

## 📖 About the Project

**LrGeniusAI** brings the power of modern Large Language Models (LLMs) directly into Adobe Lightroom Classic. It analyzes your photos, automatically generates accurate tags and detailed descriptions, creates AI-guided Lightroom develop edit recipes, and lets you rediscover your library with semantic free-text search using natural language.

Whether you prefer running local models to ensure maximum privacy or want to leverage powerful cloud APIs, LrGeniusAI seamlessly adapts to your photography workflow.

---

## ✨ Core Features

- **🤖 AI-Powered Tagging & Describing:** Uses advanced LLMs to accurately recognize image content, generate metadata, and provide detailed descriptions of your photos.
- **🎛️ AI Lightroom Edit (Develop) *(beta)*:** Generates a Lightroom edit recipe per photo and applies it in Develop, either to the photo itself or to a new virtual copy. No language model is involved: the recipe is interpolated from the edits you saved yourself (see *Style Training*), so at least five training examples are required. Per-photo review is on by default.
- **🔍 Semantic Free-Text Search (Advanced Search):** Find images naturally through descriptive queries (e.g., *"Red sports car parked in front of a garage"* or *"Sunset over the mountains"*). LrGeniusAI automatically creates a relevance-sorted Collection in Lightroom based on your prompt.
- **📸 Image Culling *(beta)*:** Group similar photos into bursts or near-duplicate stacks, automatically pick the strongest frames, and create Lightroom collections for picks, alternates, reject candidates, and optional duplicates.
- **👥 People & Faces:** Detect and cluster faces, assign names to persons, browse person collections, and find similar faces across your catalog.
- **🦋 Species Identification (on-device):** Identify animals, plants and fungi down to the species with **BioCLIP 2**, running entirely on your machine — no LLM, so no invented binomials. Results land in searchable metadata fields (kingdom → species) and, optionally, as a keyword branch under `Species`. An uncertain call stops at the rank it is sure of rather than guessing. Enable it in *Analyze & Index Photos* after downloading the model in the Plug-in Manager.
- **🔎 Find Similar Images:** Find near-duplicate or visually similar photos for any selected image using perceptual hash or semantic CLIP comparison.
- **🏠 Built-In Local AI (no external app):** The backend runs vision models itself, using whichever engine suits the platform — **MLX** on macOS (Apple silicon, via a small Metal helper process) and **llama.cpp** in-process on Windows (GGUF, Vulkan). Pick a model in the Plug-in Manager, click *Download*, and analysis runs entirely on your machine. Models you already have in LM Studio (or the Hugging Face cache) are picked up without a second copy.
- **☁️ Local & Cloud Models:** Also supports local AI models via **Ollama** and **LM Studio**, as well as integration with cloud providers like **ChatGPT/OpenAI** and **Google Gemini**. (~~**Vertex AI**~~ — *removed*, see below.)
- **🎨 Customizable Prompts & Temperature Control:** System prompts for the AI can be added, edited, and deleted directly within the Lightroom Plug-In Manager. Use the temperature slider to control whether the AI should be highly creative or strictly consistent.
- **📝 Photo Context (Contextual Info):** Provide manual hints to the AI before analysis (e.g., names of people or specific background details) that aren't immediately obvious from the image itself. This can be done via a popup dialog or directly in Lightroom's metadata panel.
- **🗂️ Keyword Management:** Interactive synonym deduplication and automatic de-clutter during indexing to keep your keyword catalog clean.
- **🎓 Style Training:** Save your own Lightroom edits as AI training examples to teach the AI your personal editing style.
- **🗄️ Custom Backend & Database:** The plugin utilizes a high-performance local server (`geniusai-server`), written in Rust for low memory overhead. Existing metadata from your Lightroom catalog can easily be imported prior to the first AI analysis.

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
4. **Pick an AI model.** Either enter a cloud API key in the **Plug-in Manager**, or stay fully local: the **Local AI Model** section offers a curated list of vision models (Gemma 4, Ministral 3, Qwen3-VL, Qwen2.5-VL) — choose one and click *Download*. The section shows the engine your platform ships: **MLX** on macOS, **llama.cpp** on Windows. See [Local AI Models](https://github.com/LrGenius/LrGeniusAI/wiki/Help-Local-AI-Models).
5. Select photos in the library and choose one of the AI actions from **Library -> Plug-in Extras**:
   - **Analyze & Index Photos...** — AI tagging, descriptions, search index, faces, and optional species identification
   - **AI Edit Photos...** *(beta)* — generate and apply Lightroom develop edits learned from your own edits
   - **Advanced Search...** — semantic free-text search
   - **Cull Similar Photos...** *(beta)* — burst grouping and auto-ranking
   - **People...** — face clusters and named person collections
   - **Find Similar Images...** — find near-duplicates or visually similar photos
   - **Deduplicate Keyword Synonyms...** — clean up synonym sprawl in your catalog
6. For AI Edit, first teach it your style: edit a few photos by hand and run **Save Edits as AI Training Examples...**. AI Edit builds every recipe from those examples and needs at least five of them — it calls no LLM. Keep **Review each proposed edit before applying it** enabled while you validate the results.

*For comprehensive details, model setup guides, and tips, please visit [lrgenius.com/help](http://lrgenius.com/help/).*

---

> **⚠️ Google Vertex AI has been removed (August 2026).**
> The Vertex AI controls are gone from the Lightroom plugin: no project ID / location
> settings, no Vertex embeddings during *Analyze & Index*, and no *Semantic (Vertex AI)*
> search option. Existing Vertex embeddings in your database stay untouched but are no
> longer created or queried. The old setup instructions are kept for reference on the
> [Google Vertex AI Login Wiki Page](https://github.com/LrGenius/LrGeniusAI/wiki/Google-Vertex-AI-Login).

## ⚖️ License

The LrGeniusAI core, plugin, and backend are released under the **GNU Affero General Public License v3 (AGPL-3.0)**. 

This project is built on the belief that AI tooling for creatives should remain open, transparent, and community-driven. See the [LICENSE](LICENSE) file for the full license text.


## 🛠️ Tech Stack

- **Frontend / Lightroom Plugin:** Lua (Lightroom SDK)
- **Backend / Server:** `geniusai-server` — Rust (axum) for deterministic memory behavior
- **AI & Embedding:** SigLIP2 via ONNX Runtime (`ort` crate)
- **Identity & Faces:** InsightFace (ONNX)
- **Species:** BioCLIP 2 (ONNX) with a pruned TreeOfLife taxonomy head
- **Local Inference:** an MLX Swift helper (`lrgenius-mlx`) on macOS; llama.cpp compiled into the backend (Vulkan / CPU) on Windows
- **Database:** LanceDB
- **Supported Interfaces:** built-in MLX (macOS), built-in llama.cpp (Windows), Google Gemini, ChatGPT/OpenAI, Ollama, LM-Studio (~~Vertex AI~~ — *removed*)


---

## 🛠️ Development

For more detailed information on how to contribute, please see our [CONTRIBUTING.md](CONTRIBUTING.md).


## 🤝 Credits

Developed with a passion for photography and IT by:

- **Bastian Machek (LrGenius / Fokuspunk)** – *Creator & Lead Developer*
- **Community** – *Special thanks to all contributors and testers for your valuable input and support.*
- **Various AI agents** - *For the great support in developing this project.*

This project leverages many incredible open-source libraries and models, including **InsightFace**, **BioCLIP 2 / TreeOfLife-200M**, **OpenCLIP**, **ONNX Runtime**, **LanceDB**, **llama.cpp**, and **MLX / mlx-swift-lm**. See the [Credits wiki page](https://github.com/LrGenius/LrGeniusAI/wiki/Credits) for the full list and licences. 

A huge thank you to the open-source community and the developers of the underlying AI frameworks that make this integration possible!
