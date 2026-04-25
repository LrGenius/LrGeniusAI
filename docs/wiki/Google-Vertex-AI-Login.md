# ☁️ Google Vertex AI Login (gcloud)

If you want to use Vertex AI with LrGeniusAI, run the login on the machine where `geniusai-server` is running.

## macOS

1. Install Google Cloud CLI (if needed):  
   [https://cloud.google.com/sdk/docs/install](https://cloud.google.com/sdk/docs/install)
2. Open Terminal and run:

```bash
gcloud init
gcloud config set project YOUR_PROJECT_ID
gcloud auth application-default login
```

3. Optional verification:

```bash
gcloud auth application-default print-access-token
```

## Windows (PowerShell)

1. Install Google Cloud CLI (if needed):  
   [https://cloud.google.com/sdk/docs/install](https://cloud.google.com/sdk/docs/install)
2. Open **Google Cloud SDK Shell** (or PowerShell with `gcloud` in PATH) and run:

```powershell
gcloud init
gcloud config set project YOUR_PROJECT_ID
gcloud auth application-default login
```

3. Optional verification:

```powershell
gcloud auth application-default print-access-token
```

## Remote backend with Docker Compose

If your backend runs as a remote Docker container, authenticate inside the container and persist the Google Cloud CLI state with the bind mount in `docker-compose.yml`.

1. Open a shell on the server in the repository root:

```bash
mkdir -p gcloud
docker compose up -d --build
```

2. Set the Vertex project inside the running container:

```bash
docker compose exec geniusai-server gcloud config set project YOUR_PROJECT_ID
```

3. Login for Application Default Credentials (ADC):

```bash
docker compose exec geniusai-server gcloud auth application-default login
```

4. Optional verification:

```bash
docker compose exec geniusai-server gcloud auth application-default print-access-token
```

For headless SSH hosts without a browser, use:

```bash
docker compose exec geniusai-server gcloud auth application-default login --no-browser
```

Then follow the remote bootstrap flow shown by `gcloud` on a second trusted machine that has a browser and Google Cloud CLI installed.

## Notes

- `gcloud auth application-default login` creates local Application Default Credentials (ADC).
- In Docker Compose, the bind mount `./gcloud:/root/.config/gcloud` keeps ADC and the active gcloud project across container restarts and rebuilds.
- Set `Vertex AI Project ID` and `Vertex AI Location` in the Lightroom plugin settings.
- Do not set `GOOGLE_APPLICATION_CREDENTIALS` when you want the container to use ADC created by `gcloud auth application-default login`.
- For headless/server deployments, prefer service-account auth via `GOOGLE_APPLICATION_CREDENTIALS`.
