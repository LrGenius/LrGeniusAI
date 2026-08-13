# LrGeniusAI Wiki

Welcome to the project wiki.

---

## User Documentation

### Getting started

- [Getting Started](Getting-Started) — installation, first index run, initial setup
- [FAQ](FAQ) — common questions and quick answers
- [Troubleshooting](Troubleshooting) — connection issues, API errors, missing models

### Feature guides

- [Help: Analyze and Index](Help-Analyze-and-Index) — AI tagging, descriptions, search embeddings
- [Help: AI Edit Photos](Help-AI-Edit) *(beta)* — generate and apply Lightroom develop recipes
- [Help: Advanced Search](Help-Advanced-Search) — semantic free-text search
- [Help: Cull Photos](Help-Cull-Photos) *(beta)* — burst grouping, ranking, Picks/Alternates/Rejects
- [Help: People & Faces](Help-People-Faces) — face clusters, person names, face-based collections
- [Help: Find Similar Images](Help-Find-Similar) — near-duplicate and content-similar search
- [Help: Keyword Deduplication and De-Clutter](Help-Keyword-Dedup-and-Declutter) — synonym merging and auto de-clutter
- [Help: Train from Edits](Help-Train-From-Edits) — save your edits as AI style examples
- [Plugin Guide](Plugin-Guide) — complete menu reference and workflow overview

### AI model setup

- [Help: Choosing AI Model](Help-Choosing-AI-Model) — cloud vs local, model comparison table
- [Help: Local AI Models](Help-Local-AI-Models) — built-in llama.cpp & MLX engines, no external app
- [Help: Ollama Setup](Help-Ollama-Setup)
- [Help: LM Studio Setup](Help-LM-Studio-Setup)
- [Google Vertex AI Login](Google-Vertex-AI-Login)

### Other

- [Credits & Dependencies](Credits)

---

## Developer Documentation

- [Build Environment Setup](Dev-Build-Environment-Setup) — Windows & macOS toolchain setup for `server-rs`, including the `llamacpp` feature and MLX sidecar
- [Backend API Reference](Dev-Backend-API) — all REST endpoints documented
- [Server Guide](Dev-Server-Guide) — backend architecture, database backup, lifecycle
- [Dev: Testing the Update Mechanism](Dev-Testing-Update-Mechanism)
- [Dev: Feature Priority Decision](Dev-Feature-Priority-Decision)
- [Dev: Image Culling Implementation Plan](Dev-Image-Culling-Implementation-Plan)

### Auto-generated from README files

- [Project README](Dev-Project-README)
- [Plugin README](Dev-Plugin-README)
- [Server README](Dev-Server-README)

---

## What is LrGeniusAI?

LrGeniusAI is an AI extension for Lightroom Classic. It runs a local backend server and connects it to the Lightroom plugin to provide:

- **AI metadata generation** — keywords, titles, captions, alt text, via cloud APIs or models the backend runs locally itself
- **AI develop edits** *(beta)* — per-photo Lightroom develop recipes with style presets
- **Semantic free-text search** — find photos by describing them in natural language
- **Image culling** *(beta)* — burst grouping, scoring, Picks/Alternates/Rejects collections
- **Face & person workflows** — face detection, clustering, named person collections
- **Find similar images** — near-duplicate and visually similar search
- **Keyword management** — automatic de-clutter and interactive synonym deduplication
- **Style training** — save your own edits as AI few-shot examples

For project overview and release info, see [Project README](Dev-Project-README).
