# geniusai-server – für lokalen oder Remote-Betrieb als Container
# Build: docker build -t geniusai-server .
# Run:   docker run -p 19819:19819 -v /pfad/zu/daten:/data -e GENIUSAI_HOST=0.0.0.0 geniusai-server
#
# API surface is everything under COPY src/ (index, search, clip, faces, import, db, server, …).
# AI Lightroom edit recipes: POST /edit (multipart) and POST /edit_base64 (JSON) – same image,
# no extra Dockerfile COPY beyond src/.

FROM ghcr.io/astral-sh/uv:python3.12-trixie-slim

WORKDIR /app

# Build tools for insightface plus Google Cloud CLI for Vertex ADC login
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    gnupg \
    build-essential \
    && mkdir -p /etc/apt/keyrings \
    && curl -fsSL https://packages.cloud.google.com/apt/doc/apt-key.gpg | gpg --dearmor -o /etc/apt/keyrings/google-cloud.gpg \
    && echo "deb [signed-by=/etc/apt/keyrings/google-cloud.gpg] https://packages.cloud.google.com/apt cloud-sdk main" > /etc/apt/sources.list.d/google-cloud-sdk.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends google-cloud-cli \
    && rm -rf /var/lib/apt/lists/*

# Abhängigkeiten

COPY uv.lock pyproject.toml .
# Disable development dependencies
ENV UV_NO_DEV=1
RUN uv sync --locked

# App source
COPY src /app/src

# Remote-Zugriff: Server auf allen Interfaces binden
ENV GENIUSAI_HOST=0.0.0.0
ENV GENIUSAI_PORT=19819

# Model caches (Hugging Face + InsightFace) – keep persistent via mounted volume
ENV HF_HOME=/models/huggingface
ENV INSIGHTFACE_ROOT=/models/insightface
ENV CLOUDSDK_CONFIG=/root/.config/gcloud

# Persistente Backend-Daten (Chroma/JSON/SQLite) per Volume mounten.
# Diese Dateien werden auch vom /db/backup-Endpunkt gesichert.
VOLUME /data
VOLUME /models

EXPOSE 19819

# DB-Pfad zeigt auf das persistente Backend-Datenverzeichnis unter /data/db.
CMD ["uv", "run", "/app/src/geniusai_server.py", "--db-path", "/data/db"]
