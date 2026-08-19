# Server Guide

The backend (`geniusai-server`) acts as the brains of LrGeniusAI. It runs locally and handles Large Language Model (LLM) inference, image embedding generation, and vector database management. It's written in Rust (axum, ONNX Runtime, LanceDB) for low and deterministic memory use.

## Main Documentation

For configuration settings, dependency management, and architecture details, refer to [`server-rs/README.md`](../server-rs/README.md) in this repo.

## Key Responsibilities

The backend server is responsible for:
- **Image Indexing:** Offloading heavy ML workloads (SigLIP2/ONNX Runtime inference) away from the Lightroom UI.
- **Semantic Search:** Executing fast, vector-based similarity searches using LanceDB.
- **Metadata Persistence:** Storing tags, face matches, and other AI-generated text alongside the vectors in LanceDB tables.
- **Face & Person APIs:** Processing and matching facial data to build identity maps over time.
- **Model Caching:** Automatically downloading and verifying local storage of the SigLIP2 and face model files to avoid redundant downloads. The `/status` endpoint exposes an `is_model_cached` flag which allows the Lightroom plugin to display warning messages if required assets are missing prior to initiating a task.

## Error Handling & Logic

The API is structured to return robust Error responses. In the event of batch processing failures, endpoints will format exact stack traces and JSON objects detailing which images failed and why (e.g. timeout, invalid model reference, API quota limits). This structured data is intercepted by the plugin to generate user-friendly GUI error reports. 

If you are experiencing unexpected backend behavior:
1. Try parsing the terminal output or log files written to the server's working directory. 
2. Refer to the [Troubleshooting](Troubleshooting) wiki page to debug the server connection.

## Database Backup Workflow

Given the importance of your generated search indexes and AI metadata, the backend exposes a dedicated backup download flow:
- API endpoint: `GET /db/backup`
- Output: A comprehensive ZIP archive containing the complete LanceDB data directory.

**To create a backup via Lightroom:**
Open `File -> Plug-in Manager -> LrGeniusAI -> Backend Server` and click **Download DB backup**.

**When to backup:**
We highly recommend initiating a backup prior to running large one-time DB migrations, moving the server to a new machine, or updating backend dependencies.
