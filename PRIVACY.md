# 🔒 Privacy Policy

**Last Updated:** April 11, 2026

> [!IMPORTANT]
> **LrGeniusAI is local-first by design.**
> Your photos, metadata, and AI-generated data stay on your computer by default. We believe in "Privacy as a Feature," not an afterthought.

---

## 📖 Our Philosophy

LrGeniusAI was built with a simple goal: to provide powerful AI tools for photographers without compromising their privacy. We understand that your photo library is personal, and your creative "DNA" (editing style) is your intellectual property. Our architecture reflects this by prioritizing local processing over cloud-based workflows.

---

## 💻 Local vs. ☁️ Cloud Processing

You have full control over where your data is processed. This is how we handle it:

### 🏠 Local Processing (Default)
When using local models like **Ollama** or **LM Studio**, all analysis, tagging, and semantic indexing happen entirely on your machine. No image data or metadata is transmitted to external servers.

### 🌐 Cloud Processing (Optional)
If you choose to enable cloud providers (OpenAI, Google Gemini, Vertex AI), only the necessary data is sent to these services:
- **Image Content**: Temporary transmission of image pixels or descriptive prompts for analysis.
- **Contextual Hints**: Any manual photo context you provide.
- **API Keys**: Stored locally and sent only to the respective provider.

---

## 📊 Data Collection & Storage

| Data Type | Storage Location | Retention | Why we need it |
| :--- | :--- | :--- | :--- |
| **Photos & Previews** | 🏠 Local Drive | Persistent | To generate AI tags and edits. |
| **Photo Metadata** (EXIF/IPTC) | 🏠 Local SQLite | Persistent | To identify photos and camera profiles. |
| **Search Embeddings** | 🏠 Local ChromaDB | Persistent | To enable semantic "natural language" search. |
| **Face Templates** | 🏠 Local Database | Persistent | To group photos by recognized people. |
| **Style Profile** (DNA) | 🏠 Local Database | Persistent | To learn your editing preferences. |
| **Diagnostic Logs** | 🏠 Local / ☁️ Remote* | Per Issue | To troubleshoot plugin errors. |

*\*Remote logs are only uploaded when you manually initiate a "Diagnostic Report" or "Copy to Desktop" action for support.*

---

## 🛡️ Sensitive Data & Security

> [!NOTE]
> **No Hidden Analytics**: LrGeniusAI does not include background tracking, telemetry, or "usage metrics" that monitor your clicks or workflow without your knowledge.

### 📸 Images and Face Recognition
We use **InsightFace** for local face clustering. These biometric templates are stored in your local backend database and are **never** shared with us or any third party.

### 🔑 API Keys
Your API keys for services like OpenAI or Vertex AI are stored in the Lightroom plugin configuration (on your disk). They are transmitted only to the service provider via encrypted HTTPS requests.

---

## 📋 Third-Party Services

If you utilize cloud features, please refer to the privacy policies of the supported providers:

*   [OpenAI Privacy Policy](https://openai.com/policies/privacy-policy)
*   [Google Cloud (Vertex AI) Privacy Notice](https://cloud.google.com/terms/cloud-privacy-notice)
*   [Ollama (Local)](https://ollama.com/) - *Fully Private*
*   [LM Studio (Local)](https://lmstudio.ai/) - *Fully Private*

---

## 📬 Contact & Control

As the developer, I (Bastian Machek) have no access to your data. If you have questions about how the plugin handles specific workflows, please reach out:

- **Website:** [lrgenius.com/help](http://lrgenius.com/help/)
- **GitHub:** [Report an Issue](https://github.com/LrGenius/LrGeniusAI/issues)

> [!TIP]
> **Tip for Maximum Privacy:** Want 100% data sovereignty? Use **Ollama** for language tasks and **OpenCLIP (Local)** for search indexing. This ensures that zero bytes of your library ever leave your local network.