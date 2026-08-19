# Credits & Dependencies

LrGeniusAI is made possible by these amazing open-source projects and AI frameworks.

## Core Backend Dependencies
- **YuNet**: Fast, lightweight face detector from the OpenCV Model Zoo; provides the face boxes and five landmarks. [GitHub](https://github.com/opencv/opencv_zoo)
- **FaceNet (facenet-pytorch)**: Inception-ResNet-v1 with VGGFace2 weights; produces the face embeddings that group photos by person. [GitHub](https://github.com/timesler/facenet-pytorch)
- **SigLIP2 / OpenCLIP**: The vision-language models behind semantic search, and the framework both SigLIP2 and BioCLIP 2 are loaded and exported with. [GitHub](https://github.com/mlfoundations/open_clip)
- **ONNX Runtime**: Cross-platform, high performance ML inferencing, used via the `ort` crate for SigLIP2, BioCLIP 2, YuNet and FaceNet. [Website](https://onnxruntime.ai/)
- **LanceDB**: Embedded vector database storing embeddings and metadata. [Website](https://lancedb.com/)
- **axum / tokio**: Async HTTP server and runtime for `geniusai-server`. [GitHub](https://github.com/tokio-rs/axum)
- **Hugging Face Tokenizers**: Fast tokenizers used for the SigLIP2 text tower. [GitHub](https://github.com/huggingface/tokenizers)

## Species Identification

- **BioCLIP 2** (MIT): The vision model behind on-device species identification, trained on the tree of life. [Project](https://imageomics.github.io/bioclip-2/) · [Model](https://huggingface.co/imageomics/bioclip-2) · [Paper](https://arxiv.org/abs/2505.23883)
- **TreeOfLife-200M** (CC0-1.0): The 200-million-image dataset BioCLIP 2 was trained on. Its precomputed text embeddings *are* the taxonomy the plugin classifies against — LrGeniusAI ships a pruned subset of them. [Dataset](https://huggingface.co/datasets/imageomics/TreeOfLife-200M)
- **GBIF Backbone Taxonomy**: The source of the common names shown alongside each scientific name, via TreeOfLife-200M's label files. [DOI 10.15468/39omei](https://doi.org/10.15468/39omei)
- **pybioclip**: The reference implementation whose preprocessing and rank-aggregation semantics `lrg-ml::bioclip` reproduces in Rust. [GitHub](https://github.com/Imageomics/pybioclip)

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
