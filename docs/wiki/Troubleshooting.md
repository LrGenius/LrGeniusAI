# Troubleshooting

> Tips and solutions for common issues when running LrGeniusAI.

## GUI Error Reporting

Recent versions of LrGeniusAI have transitioned from silent, log-file-based error tracking to transparent, actionable GUI error dialogs directly within Lightroom. This means that if something fails behind the scenes—whether during batch indexing or AI editing—you will be notified immediately.

### Task Completion Summaries

When running bulk operations like "Analyze & Index photos" or "AI Edit photos", you may process hundreds of images at once. If any single image fails (e.g., due to an API timeout), the overall task will continue. At the end of the batch process, a **Task Completion Dialog** will systematically aggregate and display a summary of:
- Successfully processed photos
- Photos that encountered errors

A detailed report of the errors will be attached to the dialog, enabling you to identify exactly which images need to be re-run and why they failed without scouring backend logs.

## Common Issues

### 0. Installer & Security Warnings (SmartScreen / Gatekeeper)

Since LrGeniusAI installers are not currently code-signed, your OS may block them.

- **Symptom**: "Windows protected your PC" or "LrGeniusAI.pkg cannot be opened because it is from an unidentified developer".
- **Resolution**: Refer to the [Getting Started](Getting-Started#%E2%9A%A0%EF%B8%8F-bypassing-security-warnings-unsigned-installers) guide for OS-specific bypass steps. This is a one-time requirement during installation.

### 1. Missing OpenCLIP Model

When you first launch the backend, it may need to download the selected OpenCLIP model to generate vector embeddings for semantic search. If the model hasn't finished downloading or could not be downloaded (e.g., due to network issues):

- **Symptom**: The backend starts, but semantic search or indexing immediately fails.
- **Resolution**: Lightroom will now display a specific warning dialog indicating that the OpenCLIP model is missing. Check your internet connection and ensure the backend server has write permissions to download the models into its cache directory. The `/status` endpoint monitors whether the specific model is successfully cached locally.

### 2. Backend Server Connection Failed

Lightroom communicates with the `geniusai-server` over a local network port (default: 8000).

- **Symptom**: "Cannot connect to server" or "Connection Refused" errors.
- **Resolution**: 
  1. Open `File -> Plug-in Manager -> LrGeniusAI -> Backend Server`.
  2. Verify the server URL `http://127.0.0.1:8000` matches where your server is running.
  3. Ensure the server terminal/console hasn't crashed.

### 3. Invalid API Keys

If you're using cloud providers (Gemini, ChatGPT, Vertex AI), authentication failures are hard-blocked by the providers.

- **Symptom**: "Unauthorized" or 401/403 HTTP errors reported in the Lightroom Error Dialog.
- **Resolution**: Double-check your API keys in the Lightroom Plug-in Manager. Make sure they haven't expired and have sufficient billing quotas attached.

### 4. Local Model Timeout

Local AI models (via LM Studio or Ollama) can take significantly longer to process than cloud APIs, particularly on machines without powerful GPUs.

- **Symptom**: Lightroom displays a timeout error during image analysis.
- **Resolution**: Ensure your local LLM server is actually running and the model is loaded into memory before starting the batch job in Lightroom. You may also need to process smaller batches of photos at a time.
