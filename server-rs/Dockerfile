# geniusai-server (Rust) — for local or remote operation as a container
# Build: docker build -t geniusai-server -f server-rs/Dockerfile server-rs
# Run:   docker run -p 19819:19819 -v /path/to/data:/data -v /path/to/models:/models \
#          -e GENIUSAI_HOST=0.0.0.0 geniusai-server
#
# Model files: mount /models per the layout below (or trigger POST
# /clip/download/start once a matching GitHub release with model assets
# exists — see server-rs/README.md), or the server starts fine but
# /index and /faces/detect fail until the files are in place.

FROM rust:1-bookworm AS builder

WORKDIR /app

# pkg-config + libssl-dev: openssl-sys (reqwest's native-tls, ureq in ort-sys)
# links against the system OpenSSL rather than vendoring it.
# protobuf-compiler: prost-build (pulled in transitively by lancedb/lance).
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# Baked in via option_env!() at compile time (lrg-common::version) — same
# values the native macOS/Windows/Linux release builds set via $GITHUB_ENV;
# passed here as build-args since this is a separate Docker build context.
ARG LRG_BACKEND_VERSION
ARG LRG_BACKEND_RELEASE_TAG
ARG LRG_BACKEND_BUILD
ENV LRG_BACKEND_VERSION=${LRG_BACKEND_VERSION}
ENV LRG_BACKEND_RELEASE_TAG=${LRG_BACKEND_RELEASE_TAG}
ENV LRG_BACKEND_BUILD=${LRG_BACKEND_BUILD}

COPY . .
RUN cargo build --release -p lrg-server

# `ort`'s `copy-dylibs` feature (on by default) symlinks the downloaded
# ONNX Runtime shared library next to the built binary, pointing back into
# the build cache — that symlink target won't exist in the runtime stage,
# so dereference it into a clean directory here instead of copying it as-is.
RUN mkdir -p /out && \
    cp -L target/release/geniusai-server /out/ && \
    find target/release -maxdepth 1 \( -name '*.so*' -o -name '*.dylib' \) -exec cp -L {} /out/ \;

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /out/ /app/

# The onnxruntime shared library ships next to the binary (see the build
# stage above); it has no rpath pointing here, so the loader needs telling.
ENV LD_LIBRARY_PATH=/app

# Remote access: bind on all interfaces
ENV GENIUSAI_HOST=0.0.0.0
ENV GENIUSAI_PORT=19819

# Model files (SigLIP2 ONNX export + InsightFace buffalo_l) — mount a
# populated /models volume; see server-rs/scripts/export_siglip2_fp16.py
# for how to produce the SigLIP2 files until a signed release asset exists.
ENV LRG_SIGLIP_IMAGE_ONNX=/models/siglip2/siglip2_image_fp16.onnx
ENV LRG_SIGLIP_TEXT_ONNX=/models/siglip2/siglip2_text_fp16.onnx
ENV LRG_SIGLIP_TOKENIZER=/models/siglip2/tokenizer.json
ENV INSIGHTFACE_ROOT=/models/insightface

# Persistent backend data (LanceDB tables, handshake files) and models —
# mount both as volumes. Backend data is also what /db/backup zips up.
VOLUME /data
VOLUME /models

EXPOSE 19819

CMD ["/app/geniusai-server", "--db-path", "/data/db"]
