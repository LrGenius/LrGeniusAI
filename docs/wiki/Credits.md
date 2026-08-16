# Credits & Dependencies

LrGeniusAI is made possible by these amazing open-source projects and AI frameworks.

## Core Backend Dependencies
- **InsightFace**: State-of-the-art face analysis and recognition; the `buffalo_l` models power face detection and recognition. [GitHub](https://github.com/deepinsight/insightface)
- **SigLIP2 / OpenCLIP**: The vision-language models behind semantic search. [GitHub](https://github.com/mlfoundations/open_clip)
- **ONNX Runtime**: Cross-platform, high performance ML inferencing, used via the `ort` crate for SigLIP2 and InsightFace. [Website](https://onnxruntime.ai/)
- **LanceDB**: Embedded vector database storing embeddings and metadata. [Website](https://lancedb.com/)
- **axum / tokio**: Async HTTP server and runtime for `geniusai-server`. [GitHub](https://github.com/tokio-rs/axum)
- **Hugging Face Tokenizers**: Fast tokenizers used for the SigLIP2 text tower. [GitHub](https://github.com/huggingface/tokenizers)

## Local Inference Engines
- **llama.cpp**: Runs GGUF vision models in-process on Windows (Vulkan), via the `llama-cpp-2` bindings. [GitHub](https://github.com/ggml-org/llama.cpp)
- **llguidance**: Turns JSON Schemas into decode-time constraints, so local models return valid structured output. [GitHub](https://github.com/guidance-ai/llguidance)
- **MLX & mlx-swift-lm**: Apple's array framework and its language-model package, powering the `lrgenius-mlx` sidecar on Apple silicon. [GitHub](https://github.com/ml-explore/mlx-swift-lm)
- **Gemma, Qwen-VL and SmolVLM**: The open-weights vision models offered in the built-in model catalogs.

## AI Model Providers & Interfaces
- **Google Gemini**: Large language models for multimodal analysis.
- ~~**Google Vertex AI**: Enterprise AI platform for training and deploying models.~~ *(removed from the plugin in August 2026; the backend client code remains but is unused.)*
- **OpenAI / ChatGPT**: Advanced conversational AI and embeddings.
- **Ollama**: Run open-source large language models locally. [Website](https://ollama.ai/)
- **LM Studio**: Discover, download, and run local LLMs. [Website](https://lmstudio.ai/)

## Utilities & SDKs
- **JSON.lua**: A complete JSON encoder/decoder in Lua by Jeffrey Friedl.
- **Adobe Lightroom Classic SDK**: The Lua API the plugin frontend is built on.
- **Hugging Face Hub**: Distribution for the downloadable local models.

---
Developed by **Bastian Machek (LrGenius / Fokuspunk)** and **AI agents**.
