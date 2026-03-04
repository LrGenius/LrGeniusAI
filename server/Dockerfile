# geniusai-server – für lokalen oder Remote-Betrieb als Container
# Build: docker build -t geniusai-server .
# Run:   docker run -p 19819:19819 -v /pfad/zu/daten:/data -e GENIUSAI_HOST=0.0.0.0 geniusai-server

FROM python:3.12-slim

WORKDIR /app

# Build-Tools für insightface (Cython/C++-Erweiterungen)
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
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

# ChromaDB-Daten persistent (von außen mounten)
VOLUME /data
VOLUME /models

EXPOSE 19819

# DB-Pfad per Volume; bei Bedarf überschreiben: docker run ... geniusai-server --db-path /anderer/pfad
CMD ["python", "/app/src/geniusai_server.py", "--db-path", "/data/db"]
