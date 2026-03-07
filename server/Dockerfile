# geniusai-server – für lokalen oder Remote-Betrieb als Container
# Build: docker build -t geniusai-server .
# Run:   docker run -p 19819:19819 -v /pfad/zu/daten:/data -e GENIUSAI_HOST=0.0.0.0 geniusai-server

FROM python:3.12-slim

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
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

# App (src) + lokales open_clip
COPY src /app/src
COPY open_clip /app/open_clip

# Damit beim Start "config" und "open_clip" gefunden werden
ENV PYTHONPATH=/app:/app/src

# Remote-Zugriff: Server auf allen Interfaces binden
ENV GENIUSAI_HOST=0.0.0.0
ENV GENIUSAI_PORT=19819

# Modell-Caches (open_clip/Hugging Face + InsightFace) – Volume mounten, damit Downloads persistent sind
ENV HF_HOME=/models/huggingface
ENV INSIGHTFACE_ROOT=/models/insightface
ENV CLOUDSDK_CONFIG=/root/.config/gcloud

# ChromaDB-Daten persistent (von außen mounten)
VOLUME /data
VOLUME /models

EXPOSE 19819

# DB-Pfad per Volume; bei Bedarf überschreiben: docker run ... geniusai-server --db-path /anderer/pfad
CMD ["python", "/app/src/geniusai_server.py", "--db-path", "/data/db"]
