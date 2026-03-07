# Getting Started

## 1. Install plugin and server

Follow installation in:

- root `README.md`
- `plugin/README.md`

## 2. Configure plugin

In Lightroom Plug-in Manager:

- set backend server URL
- configure provider/API keys
- set Vertex project/location if needed

## 3. Index photos

Run:

- `Library -> Plug-in Extras -> Analyze & Index Photos`

Then use:

- Advanced Search
- People
- Retrieve Metadata

## 4. Vertex AI login

Use gcloud ADC on the server host:

```bash
gcloud init
gcloud config set project YOUR_PROJECT_ID
gcloud auth application-default login
```

## 5. Imported help pages

Curated pages migrated from `lrgenius.com/help`:

- [Help: Analyze and Index](Help-Analyze-and-Index)
- [Help: Advanced Search](Help-Advanced-Search)
- [Help: Choosing AI Model](Help-Choosing-AI-Model)
- [Help: Ollama Setup](Help-Ollama-Setup)
- [Help: LM Studio Setup](Help-LM-Studio-Setup)
