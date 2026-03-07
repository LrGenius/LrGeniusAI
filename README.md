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
