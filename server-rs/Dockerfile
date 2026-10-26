# geniusai-server (Rust) — for local or remote operation as a container
# Build: docker build -t geniusai-server -f server-rs/Dockerfile server-rs
# Run:   docker run -p 19819:19819 -v /path/to/data:/data -v /path/to/models:/models \
#          -e GENIUSAI_HOST=0.0.0.0 geniusai-server
#
# Model files: "Download AI models" in the plug-in's Plug-in Manager fetches
# them into /models, at the paths set below (or mount a /models you filled
# yourself). Until they are there the server starts fine, but indexing and
# face detection fail.
#
# Local AI model (llama.cpp, in-process): not built in by default. Pick a
# GPU backend with --build-arg LLAMACPP=...
#   vulkan  AMD, Intel and NVIDIA GPUs through Vulkan (amd64 and arm64)
#   cuda    NVIDIA GPUs through CUDA (amd64 only; faster on NVIDIA than Vulkan)
# Both run on the CPU when the container is given no GPU. Hand it one with
#   AMD/Intel, Vulkan:     docker run --device /dev/dri ...
#   NVIDIA, Vulkan or CUDA: docker run --gpus all ...  (NVIDIA Container Toolkit)
# Downloaded models land in /models/llm.

# The local-model variant: none, vulkan or cuda.
ARG LLAMACPP=none
# For LLAMACPP=cuda. A 12.x line on purpose: CUDA 13 drops the Maxwell, Pascal
# and Volta kernels (see crates/lrg-llama/Cargo.toml). The image only starts on
# a host whose NVIDIA driver supports this CUDA version (560+ for 12.6).
ARG CUDA_VERSION=12.6.3
# Ubuntu 24.04, because the ONNX Runtime libraries ort-sys links need glibc
# 2.38+ (see the trixie note below); 22.04 has 2.35.
ARG UBUNTU_VERSION=24.04

FROM rust:1-trixie AS rust-toolchain

# --- Build environment, one per LLAMACPP value -----------------------------

# Debian trixie rather than bookworm: the prebuilt ONNX Runtime static libs
# ort-sys downloads reference glibc 2.38's __isoc23_* symbols.
#
# pkg-config + libssl-dev: openssl-sys (reqwest's native-tls, ureq in ort-sys)
# links against the system OpenSSL rather than vendoring it.
# protobuf-compiler: prost-build (pulled in transitively by lancedb/lance).
# libprotobuf-dev: ships the well-known-types .proto files (google/protobuf/empty.proto
# etc.) under /usr/include — protobuf-compiler only provides the protoc binary itself,
# and lance-encoding's build script imports empty.proto.
FROM rust-toolchain AS build-env-none
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    protobuf-compiler \
    libprotobuf-dev \
    && rm -rf /var/lib/apt/lists/*

# llama.cpp is compiled from source: cmake, and libclang for bindgen. Vulkan
# adds the loader to link against, glslc for ggml's compute shaders, and
# SPIRV-Headers, which ggml-vulkan's CMake requires.
FROM build-env-none AS build-env-vulkan
RUN apt-get update && apt-get install -y --no-install-recommends \
    cmake \
    libclang-dev \
    libvulkan-dev \
    glslc \
    spirv-headers \
    && rm -rf /var/lib/apt/lists/*

# nvcc and cuBLAS come with NVIDIA's devel image. That is Ubuntu, so Rust is
# copied in from the official image above: the same toolchain every variant
# builds with, and rustup's toolchains run on any glibc this recent.
FROM nvidia/cuda:${CUDA_VERSION}-devel-ubuntu${UBUNTU_VERSION} AS build-env-cuda
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    ca-certificates \
    pkg-config \
    libssl-dev \
    protobuf-compiler \
    libprotobuf-dev \
    cmake \
    libclang-dev \
    && rm -rf /var/lib/apt/lists/*
COPY --from=rust-toolchain /usr/local/rustup /usr/local/rustup
COPY --from=rust-toolchain /usr/local/cargo /usr/local/cargo
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:${PATH}

# --- Build ------------------------------------------------------------------

FROM build-env-${LLAMACPP} AS builder
ARG LLAMACPP
# For LLAMACPP=cuda: the GPU generations to compile kernels for, as a CMake
# list (e.g. "86-real;89-real" to build faster for one card). Empty keeps
# ggml's default, which covers Maxwell to Hopper and newer cards through PTX.
ARG CUDA_ARCHITECTURES=

WORKDIR /app

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

# On amd64 the GPU backend and the CPU code become modules llama.cpp loads at
# start (`llamacpp-dynamic-backends`): the CUDA module needs the NVIDIA driver,
# which a linked-in backend would require just to start the server, and the
# CPU code comes in one variant per CPU generation instead of the no-AVX
# baseline. arm64 cannot build them that way yet and links them in — see the
# `dynamic-backends` feature in crates/lrg-llama/Cargo.toml.
RUN set -eu; \
    arch="$(dpkg --print-architecture)"; \
    case "$LLAMACPP" in \
        none) features="" ;; \
        vulkan) features="llamacpp-vulkan" ;; \
        cuda) features="llamacpp-cuda" ;; \
        *) echo "LLAMACPP must be none, vulkan or cuda, not \"$LLAMACPP\"" >&2; exit 1 ;; \
    esac; \
    if [ "$LLAMACPP" = cuda ] && [ "$arch" != amd64 ]; then \
        echo "LLAMACPP=cuda builds for amd64 only. On $arch, llama.cpp's CUDA backend would be linked in, and the server would not start without an NVIDIA driver; use LLAMACPP=vulkan." >&2; \
        exit 1; \
    fi; \
    if [ -n "$features" ] && [ "$arch" = amd64 ]; then features="$features,llamacpp-dynamic-backends"; fi; \
    if [ -n "$CUDA_ARCHITECTURES" ]; then export CMAKE_CUDA_ARCHITECTURES="$CUDA_ARCHITECTURES"; fi; \
    echo "Building with features: ${features:-<none>}"; \
    cargo build --release -p lrg-server ${features:+--features "$features"}

# `ort`'s `copy-dylibs` feature (on by default) symlinks the downloaded
# ONNX Runtime shared library next to the built binary, pointing back into
# the build cache — that symlink target won't exist in the runtime stage,
# so dereference it into a clean directory here instead of copying it as-is.
# Broken links are skipped: llama-cpp-sys-2 hard-links its versioned libs'
# unversioned symlinks there (libllama.so -> libllama.so.0), without the files
# they point to.
#
# llama.cpp's own shared libs and backend modules (dynamic-backends builds
# only) are taken from where llama-cpp-sys-2 installed them, symlinks kept,
# so the SONAME the binary asks for (libllama.so.0) resolves. The modules go
# next to the binary, which is where llama.cpp looks for them at start.
RUN set -eu; \
    mkdir -p /out; \
    cp -L target/release/geniusai-server /out/; \
    find target/release -maxdepth 1 -name '*.so*' ! -xtype l -exec cp -L {} /out/ \; ; \
    for dir in target/release/build/llama-cpp-sys-2-*/out/lib \
               target/release/build/llama-cpp-sys-2-*/out/lib64 \
               target/release/build/llama-cpp-sys-2-*/out/backends; do \
        [ -d "$dir" ] || continue; \
        find "$dir" -maxdepth 1 -name 'lib*.so*' -exec cp -P --remove-destination {} /out/ \; ; \
    done; \
    ls -l /out

# --- Runtime, one per LLAMACPP value ---------------------------------------

FROM debian:trixie-slim AS runtime-none
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# The Vulkan loader, Mesa's Vulkan drivers for AMD and Intel GPUs, and OpenMP,
# which llama.cpp's CPU code uses. NVIDIA's Vulkan driver comes from the host,
# through the NVIDIA Container Toolkit.
FROM runtime-none AS runtime-vulkan
RUN apt-get update && apt-get install -y --no-install-recommends \
    libgomp1 \
    libvulkan1 \
    mesa-vulkan-drivers \
    && rm -rf /var/lib/apt/lists/*
# The toolkit mounts the NVIDIA Vulkan driver only for the graphics
# capability, and its default is compute and utility.
ENV NVIDIA_DRIVER_CAPABILITIES=compute,utility,graphics

# NVIDIA's base image has the CUDA runtime; ggml-cuda also links cuBLAS. Their
# `runtime` image would add a gigabyte of CUDA libraries nothing here uses.
FROM nvidia/cuda:${CUDA_VERSION}-base-ubuntu${UBUNTU_VERSION} AS runtime-cuda
ARG CUDA_VERSION
RUN set -eu; \
    cuda_mm="$(echo "$CUDA_VERSION" | cut -d. -f1,2 | tr . -)"; \
    apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        libgomp1 \
        "libcublas-${cuda_mm}"; \
    rm -rf /var/lib/apt/lists/*
# Kernels built only as PTX (every GPU but the RTX 30 and 40 series) are
# compiled by the driver on first use. Keep that cache in the data volume, or
# every new container compiles them again.
ENV CUDA_CACHE_PATH=/data/cuda-cache

FROM runtime-${LLAMACPP}

WORKDIR /app
COPY --from=builder /out/ /app/

# The onnxruntime and llama.cpp shared libraries ship next to the binary (see
# the build stage above); it has no rpath pointing here, so the loader needs
# telling. Kept after the base image's own value (the CUDA images set one).
ENV LD_LIBRARY_PATH=/app${LD_LIBRARY_PATH:+:${LD_LIBRARY_PATH}}

# Fail the build, not the first start: every library the server and its
# llama.cpp modules link has to be in the image. All but the NVIDIA driver's
# libcuda.so.1, which the NVIDIA Container Toolkit mounts in at run time.
RUN set -eu; \
    missing=""; \
    for f in /app/geniusai-server /app/*.so*; do \
        [ -e "$f" ] || continue; \
        m="$(ldd "$f" | grep 'not found' | grep -v 'libcuda\.so\.1' || true)"; \
        [ -z "$m" ] || missing="$missing$f:\n$m\n"; \
    done; \
    if [ -n "$missing" ]; then printf 'Libraries missing from the image:\n%b' "$missing" >&2; exit 1; fi; \
    /app/geniusai-server --help >/dev/null

# Remote access: bind on all interfaces
ENV GENIUSAI_HOST=0.0.0.0
ENV GENIUSAI_PORT=19819

# The access token the plug-in needs (`geniusai-server token`) is kept under
# $XDG_CONFIG_HOME/LrGeniusAI: in the /data volume, so it survives the
# container being recreated. Print it with
#   docker exec geniusai-server /app/geniusai-server token
# or set LRG_API_TOKEN to use a fixed one.
ENV XDG_CONFIG_HOME=/data

# Model files (SigLIP2, the aesthetic scorer, the face models and BioCLIP 2
# for species) — all in the /models volume, which is where the plug-in's model
# download puts them. A family left out of this list resolves to the
# container's home directory instead, and a recreated container has to fetch it
# again. To produce them yourself, see server-rs/scripts/export_siglip2_fp16.py
# and server-rs/scripts/export_face_models.py.
ENV LRG_SIGLIP_IMAGE_ONNX=/models/siglip2/siglip2_naflex_image_fp16.onnx
ENV LRG_SIGLIP_TEXT_ONNX=/models/siglip2/siglip2_naflex_text_fp16.onnx
ENV LRG_SIGLIP_TOKENIZER=/models/siglip2/siglip2_naflex_tokenizer.json
ENV LRG_AESTHETIC_ONNX=/models/aesthetic/aesthetic_siglip_fp16.onnx
ENV LRG_YUNET_ONNX=/models/face/yunet_face_detection.onnx
ENV LRG_FACENET_ONNX=/models/face/facenet_vggface2.onnx
ENV LRG_BIOCLIP_IMAGE_ONNX=/models/bioclip/bioclip2_image.onnx
ENV LRG_BIOCLIP_TAXA_BIN=/models/bioclip/bioclip2_taxa.bin
ENV LRG_BIOCLIP_TAXA_JSON=/models/bioclip/bioclip2_taxa.json
# Local AI models the plug-in downloads (llama.cpp images): in the /models
# volume too, instead of the container's home directory, which a recreated
# container starts without.
ENV LRG_LLAMA_MODEL_DIR=/models/llm

# Persistent backend data (LanceDB tables, handshake files) and models —
# mount both as volumes. Backend data is also what /db/backup zips up.
VOLUME /data
VOLUME /models

EXPOSE 19819

# --db-path is the database: the server opens it at start, since the plug-in
# does not bind a remote backend to a catalog.
CMD ["/app/geniusai-server", "--db-path", "/data/db"]
