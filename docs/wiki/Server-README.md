# Server README

> Auto-generated from `server/README.md`. Do not edit this page manually.

# 🖥️ LrGeniusAI - Backend Server

This is the Python-based core of **LrGeniusAI**. It acts as the bridge between Adobe Lightroom Classic (Lua) and various AI models, handling image processing, metadata storage, and high-speed semantic search.

---

## 🛠️ Core Responsibilities

- **📂 Database Management:** Stores image metadata, AI-generated descriptions, and vector embeddings in a local SQLite database to ensure blazing-fast retrieval without re-scanning images.
- **🧠 AI Orchestration:** - Interfaces with **Cloud APIs** (Google Gemini, Vertex AI, OpenAI).
    - Connects to **Local LLMs** via Ollama and LM Studio (enhancing privacy and saving costs).
- **🔎 Semantic Search Engine:** Uses `vertexai` / `SigLip2` to generate image embeddings. This allows users to search their Lightroom catalog using natural language descriptions instead of just keywords.
- **🎭 Face Intelligence:** Provides face detection and recognition capabilities (powered by `Insightface`).
- **⚙️ Metadata Sync:** Handles the import of existing keywords and metadata from Lightroom to build a comprehensive search index.

---

## 🚀 Technical Architecture

The server is built with **Python** and designed to run as a local background process (provided as a standalone `.exe` for Windows or a binary for macOS).

### Key Components:
- **API Framework:** FastAPI / Flask-based REST interface for communication with the Lightroom Lua plugin.
- **Computer Vision:** - `open-clip-torch`: For generating semantic embeddings.
    - `Insightface`: For advanced face detection and recognition.
    - `vertexai`: For even better semantic embeddings.
- **Database:** SQLite (local-first approach).
- **Task Handling:** Efficient processing of batch image analysis.
