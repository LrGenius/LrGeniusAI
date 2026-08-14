# Build Environment Setup (Windows & macOS)

This page walks through setting up a machine from scratch to build the
`server-rs` backend, including the optional local-LLM backends. For day-to-day
build/test commands once the environment is ready, see the
[Server README](Dev-Server-README) and [CLAUDE.md](../../CLAUDE.md).

There are three layers, each opt-in on top of the last:

1. **Baseline** — builds `geniusai-server` with cloud providers only (Gemini,
   OpenAI, Ollama, LM Studio, and the now-unused Vertex AI client — Vertex AI was
   removed from the plugin UI in August 2026, so nothing requests it). No
   local-model support.
2. **`llamacpp` feature** — adds in-process local inference via llama.cpp.
   Compiles llama.cpp from source, so it needs `cmake` + `libclang` (for
   `bindgen`), and on Windows a GPU backend (Vulkan, not CUDA — see
   `server-rs/crates/lrg-llama/Cargo.toml`).
3. **MLX sidecar** — macOS/Apple silicon only. A separate Swift executable,
   not a cargo feature, so it costs nothing to compile on other platforms.

---

## Windows

### 1. Baseline toolchain

- **Rust**: install via [rustup](https://rustup.rs). Default host triple is
  `x86_64-pc-windows-msvc`.
- **MSVC Build Tools**: rustc on the `-msvc` toolchain needs `link.exe` from
  Visual Studio. Install "Build Tools for Visual Studio" (or full Visual
  Studio) with the **"Desktop development with C++"** workload. Verify with:

  ```powershell
  & "C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe" `
    -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
    -property installationPath
  ```

- **Git** — for cloning and for the pre-commit hooks.

With just this, the default build works:

```powershell
cd server-rs
cargo build --release -p lrg-server
```

### 2. `llamacpp` feature

Needs `cmake`, `libclang`, and the Vulkan SDK.

```powershell
winget install --id Kitware.CMake -e
winget install --id LLVM.LLVM -e
```

`bindgen` (used by llama.cpp's `-sys` crate) finds libclang via the
`LIBCLANG_PATH` environment variable — it does **not** search `PATH`. Set it
to LLVM's `bin` directory, which is where `libclang.dll` lands:

```powershell
[System.Environment]::SetEnvironmentVariable("LIBCLANG_PATH", "C:\Program Files\LLVM\bin", "User")
```

Install the [Vulkan SDK](https://vulkan.lunarg.com/sdk/home#windows) and set
`VULKAN_SDK` (the LunarG installer normally does this for you — verify it, the
build panics without it):

```powershell
[System.Environment]::GetEnvironmentVariable("VULKAN_SDK", "User")
# Expected: something like C:\VulkanSDK\1.4.xxx.x
```

Open a **new** terminal so the updated environment variables are picked up,
then:

```powershell
cd server-rs
cargo build --release -p lrg-server --features llamacpp
cargo clippy --workspace --all-targets --features llamacpp
```

This is a from-source llama.cpp compile plus Vulkan shader compilation, so
expect a noticeably longer cold build than the baseline.

### 3. MLX sidecar

Not applicable — MLX is Apple-silicon only. On Windows, local inference is
llama.cpp-only; `/llm/catalog` will report `mlx.supported: false`.

### 4. Pre-commit hooks

```powershell
winget install --id astral-sh.uv -e
uv tool install pre-commit
pre-commit install
```

`uv` is also what the SigLIP2 export/verify scripts under `server-rs/scripts/`
use (see the [Server README](Dev-Server-README#model-files) if you need to
export those yourself instead of downloading the pre-built ONNX assets).

---

## macOS

### 1. Baseline toolchain

- **Rust**: install via [rustup](https://rustup.rs).
- **Xcode Command Line Tools**: `xcode-select --install`. This is also what
  provides `libclang` for step 2 below — no separate LLVM install needed.
- **Homebrew** for the rest:

  ```bash
  cd server-rs
  cargo build --release -p lrg-server
  ```

### 2. `llamacpp` feature

`libclang` ships with the Xcode CLT; only `cmake` needs installing:

```bash
brew install cmake
cd server-rs
cargo build --release -p lrg-server --features llamacpp
cargo clippy --workspace --all-targets --features llamacpp
```

No GPU-SDK step here — llama.cpp uses Metal on macOS, which is already
available.

### 3. MLX sidecar (Apple silicon only)

This is the second local backend, `mlx`. It is **not** a cargo feature —
`lrg-mlx` only spawns and talks to a helper process over stdio — but that
helper (`lrgenius-mlx`, a Swift executable under `native/mlx-sidecar/`) has to
be built separately, and **`swift build` does not produce a working binary**:
SwiftPM on the command line cannot compile MLX's Metal shaders, and the result
dies on first inference with `Failed to load the default metallib`. Use
`xcodebuild`:

```bash
xcodebuild -downloadComponent MetalToolchain   # ~690 MB, once per machine

cd native/mlx-sidecar
xcodebuild build -scheme lrgenius-mlx -destination 'platform=macOS,arch=arm64' \
  -configuration Release -derivedDataPath .build/xcode \
  -skipPackagePluginValidation -skipMacroValidation

export LRG_MLX_SIDECAR=$PWD/.build/xcode/Build/Products/Release/lrgenius-mlx
```

Without `LRG_MLX_SIDECAR` set (or the binary built), `/llm/catalog` reports
`mlx.supported: false` with a reason naming what's missing. The build also
produces bundles (`mlx-swift_Cmlx.bundle`, tokenizer resource bundles) that
must ship **next to** the executable — `Bundle.module` resolves them relative
to the binary's location, so don't move `lrgenius-mlx` on its own. See
[`native/mlx-sidecar/README.md`](../../native/mlx-sidecar/README.md) for the
full protocol and model layout (an MLX model is a directory, not a GGUF file).

### 4. Pre-commit hooks

```bash
brew install uv
uv tool install pre-commit
pre-commit install
```

---

## Verifying the setup

Regardless of platform, once built:

```bash
cd server-rs
./target/release/geniusai-server --db-path /path/to/lrgenius.db --debug
```

Then in another terminal:

```bash
curl http://127.0.0.1:19819/ping        # {"status":"ok"}
curl http://127.0.0.1:19819/llm/catalog  # check "supported"/"reason" per local backend
```

Note that the Lightroom plugin auto-launches the *installed* binary
(`/Applications/LrGeniusAI/Server/lrgenius-server` on macOS, the equivalent
under Program Files on Windows), not your dev build — `startServer` pings port
19819 first and short-circuits if something already answers. Start your
feature-enabled dev build by hand first and the plugin will talk to that
instead.

## Related

- [Server README](Dev-Server-README) — day-to-day build/test/run commands, model file setup
- [Server Guide](Dev-Server-Guide) — backend architecture and responsibilities
- [Backend API Reference](Dev-Backend-API)
