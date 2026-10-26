# Help: Docker with GPU Access

The backend can run in a Docker container — on a Linux PC, a home server or a
NAS — while Lightroom stays on your Mac or PC. With the `-vulkan` or `-cuda`
image, that container also runs a local AI model (llama.cpp) on its graphics
card. A machine with a GPU then writes the keywords, titles and captions for
a Lightroom computer that has none, with no cloud account.

> [!WARNING]
> Run the backend only on a network you trust. Lightroom talks to it over
> plain HTTP, so read
> [the FAQ](FAQ#can-i-run-the-backend-on-a-different-machine-or-in-docker)
> before you make its port reachable from other computers.

---

## 1. Pick an image

| Image | Local AI model | GPUs | Runs on |
|---|---|---|---|
| `ghcr.io/lrgenius/lrgeniusai:latest` | none | — | amd64, arm64 |
| `ghcr.io/lrgenius/lrgeniusai:latest-vulkan` | llama.cpp | AMD, Intel, NVIDIA | amd64, arm64 |
| `ghcr.io/lrgenius/lrgeniusai:latest-cuda` | llama.cpp | NVIDIA | amd64 |

- **NVIDIA card:** `-cuda`, which is faster on NVIDIA than Vulkan. On an ARM
  machine (arm64) with an NVIDIA GPU, use `-vulkan`: there is no arm64 CUDA
  image.
- **AMD or Intel GPU**, including the integrated graphics in many mini PCs and
  NAS boxes: `-vulkan`.
- **No GPU:** both images still run the model, on the CPU, which is far
  slower. The default image plus a cloud provider or an
  [Other AI server](Help-Other-AI-Server) is usually the better choice there.

`latest` follows the newest release. To stay on one, use its version instead:
`v1.2.3`, `v1.2.3-vulkan`, `v1.2.3-cuda`.

> [!NOTE]
> The `-vulkan`/`-cuda` images and the arm64 image are published from the
> first release after v3.5.2 on. Before that, [build them yourself](#7-build-the-image-yourself).

---

## 2. Prepare the host

### NVIDIA

1. Install the NVIDIA driver. The `-cuda` image needs **driver 560 or newer**
   (it is built with CUDA 12.6); `nvidia-smi` on the host shows the version you
   have.
2. Install the **NVIDIA Container Toolkit**, which hands the GPU to
   containers. On Ubuntu or Debian (other distributions:
   [NVIDIA's install guide](https://docs.nvidia.com/datacenter/cloud-native/container-toolkit/latest/install-guide.html)):

   ```bash
   curl -fsSL https://nvidia.github.io/libnvidia-container/gpgkey \
     | sudo gpg --dearmor -o /usr/share/keyrings/nvidia-container-toolkit-keyring.gpg
   curl -s -L https://nvidia.github.io/libnvidia-container/stable/deb/nvidia-container-toolkit.list \
     | sed 's#deb https://#deb [signed-by=/usr/share/keyrings/nvidia-container-toolkit-keyring.gpg] https://#g' \
     | sudo tee /etc/apt/sources.list.d/nvidia-container-toolkit.list
   sudo apt-get update
   sudo apt-get install -y nvidia-container-toolkit
   sudo nvidia-ctk runtime configure --runtime=docker
   sudo systemctl restart docker
   ```

3. Check that containers see the card:

   ```bash
   sudo docker run --rm --gpus all ubuntu nvidia-smi
   ```

   It should print the same GPU table as `nvidia-smi` on the host.

### AMD or Intel

Nothing to install beyond the GPU driver your Linux distribution already loads
(`amdgpu`, `i915` or `xe`). The Vulkan drivers (Mesa) are in the image; the
container only needs the GPU's device files. Check that they exist:

```bash
ls /dev/dri
# card0  renderD128      <- one renderD12x per GPU
```

If the backend runs in a virtual machine, the GPU has to be passed through to
that VM first; `/dev/dri` inside the VM is what counts.

### What does not work

- **Docker Desktop on a Mac** gives containers no GPU at all. On a Mac, run
  the backend the normal way (the installer): it uses MLX on the Mac's own GPU.
- **Docker Desktop on Windows** passes only NVIDIA GPUs through, via WSL 2,
  which has not been tested with LrGeniusAI. The Windows installer runs the
  same llama.cpp on any GPU, through Vulkan, without Docker.

---

## 3. Start the container

### With Docker Compose

1. Make a folder for the backend, and put
   [`docker-compose-prod.yml`](https://github.com/LrGenius/LrGeniusAI/blob/main/docker-compose-prod.yml)
   from the repository into it.
2. Next to it, create a file named `.env` that names the image:

   ```
   LRGENIUS_IMAGE=ghcr.io/lrgenius/lrgeniusai:latest-cuda
   ```

3. At the end of `docker-compose-prod.yml`, remove the `#` in front of the
   block for your GPU, keeping the indentation:

   NVIDIA (`-cuda`, or `-vulkan` on an NVIDIA card):

   ```yaml
       deploy:
         resources:
           reservations:
             devices:
               - driver: nvidia
                 count: all
                 capabilities: [gpu]
   ```

   AMD or Intel (`-vulkan`):

   ```yaml
       devices:
         - /dev/dri:/dev/dri
   ```

4. Start it:

   ```bash
   docker compose -f docker-compose-prod.yml up -d
   ```

The folder now holds `data/` (the search index, the access token, logs) and
`models/` (every AI model the backend downloads). Both survive updating or
recreating the container.

### With `docker run`

NVIDIA:

```bash
docker run -d --name geniusai-server --restart unless-stopped \
  -p 19819:19819 -v "$PWD/data:/data" -v "$PWD/models:/models" \
  --gpus all \
  ghcr.io/lrgenius/lrgeniusai:latest-cuda
```

AMD or Intel: the same with `--device /dev/dri` in place of `--gpus all`, and
the `latest-vulkan` image.

---

## 4. Connect Lightroom

1. Print the backend's access token on the Docker host:

   ```bash
   docker exec geniusai-server /app/geniusai-server token
   ```

2. In Lightroom, open *File → Plug-in Manager → LrGeniusAI → Backend Server*.
   Set the URL to the Docker host's address, e.g. `http://192.168.1.50:19819`,
   and paste the token under **Access token**.
3. Click **Download AI models** for the search, face and species models. They
   are stored on the Docker host, in `models/`.
4. Further down, the **Local AI Model — llama.cpp** box now belongs to the
   container — on a Mac too, where a backend on the same computer would show
   MLX instead. Pick a model and click **Download**; it goes to `models/llm`
   on the Docker host. [Local AI Models](Help-Local-AI-Models) explains the
   choices.
5. In *Analyze & Index Photos*, choose the model from the **AI Model**
   dropdown. It is listed under *On this Mac* or *On this PC*, as a model on
   this computer would be, although it runs in the container.

---

## 5. Check that the GPU is used

The backend picks its GPU when it loads a model, so analyze one photo first,
then look at the log:

```bash
docker logs geniusai-server 2>&1 | grep 'llama:'
```

- `llama: using <your GPU> via CUDA (… MiB VRAM), out of 2 registered device(s)`
  (or `via Vulkan`): the model runs on the GPU. The CPU always counts as one of
  the registered devices.
- `llama: no GPU backend registered, running on CPU`: the container sees no
  GPU. See [Troubleshooting](#6-troubleshooting).

On NVIDIA, `nvidia-smi` on the host also lists the backend's process and its
memory while a model is loaded.

The first model load on a card other than an RTX 30 or RTX 40 series (or the
A-/L-series of the same generations) takes longer, once: the NVIDIA driver
compiles the GPU code for that card. It keeps the result in `data/cuda-cache`,
so later starts are fast.

---

## 6. Troubleshooting

| Symptom | What to do |
|---|---|
| `running on CPU` with the `-cuda` image | The container was started without the GPU: the `deploy:` block is still commented out, or `--gpus all` is missing. If `docker run --rm --gpus all ubuntu nvidia-smi` fails too, the Container Toolkit is not set up (step 2). |
| The container does not start: `unsatisfied condition: cuda>=12.6` | The NVIDIA driver is older than 560. Update it, or use the `-vulkan` image. |
| `running on CPU` with `-vulkan` on an NVIDIA card | The Container Toolkit mounts NVIDIA's Vulkan driver only for the `graphics` capability. The image asks for it; if you set `NVIDIA_DRIVER_CAPABILITIES` yourself, include `graphics`. With `docker run`, you can also ask explicitly: `--gpus 'all,"capabilities=compute,utility,graphics"'`. |
| `running on CPU` with `-vulkan` on AMD or Intel | `/dev/dri` was not passed to the container, or the host has no render device (`ls /dev/dri`). In a VM, pass the GPU through to the VM first. |
| Compose refuses the GPU block | `capabilities: [gpu]` is required, and the block has to be indented under the service like `ports:` and `volumes:`. |
| A Mac shows the MLX box, not llama.cpp | The Backend Server URL is `localhost` or `127.0.0.1`, which the plug-in takes for a backend on this Mac. Use the Docker host's network address. |
| The model loads, then fails or runs out of memory | It is too large for the card's memory. Pick a smaller model, or lower **Layers on the GPU** in the [advanced settings](Help-Local-AI-Models#3-advanced-settings-llamacpp-only). |
| Analysis fails with "database not initialized" | An older backend that did not open its database at start. Update the image. |

---

## 7. Build the image yourself

```bash
git clone https://github.com/LrGenius/LrGeniusAI.git
cd LrGeniusAI
docker build --build-arg LLAMACPP=cuda -t geniusai-server:cuda -f server-rs/Dockerfile server-rs
```

`LLAMACPP` is `vulkan`, `cuda` or `none` (the default, no local AI model). Or
with the development Compose file, which builds the image itself:

```bash
LLAMACPP=vulkan docker compose -f docker-compose-dev.yml up -d --build
```

Then use `geniusai-server:cuda` (or `geniusai-server:dev` with Compose) where
the steps above say `ghcr.io/lrgenius/lrgeniusai:latest-cuda`.

The CUDA build compiles the GPU code for every NVIDIA generation, which takes a
long time. To build for your card only, add its architecture:

| Card | Add to `docker build` |
|---|---|
| GTX 10 series | `--build-arg CUDA_ARCHITECTURES=61-real` |
| RTX 20 series | `--build-arg CUDA_ARCHITECTURES=75-real` |
| RTX 30 series | `--build-arg CUDA_ARCHITECTURES=86-real` |
| RTX 40 series | `--build-arg CUDA_ARCHITECTURES=89-real` |
| RTX 50 series | `--build-arg CUDA_ARCHITECTURES=120a-real --build-arg CUDA_VERSION=12.8.1` (needs driver 570+) |

An image built this way runs only on that generation. RTX 50 cards also run the
regular image, through the code the driver compiles on the first start.

How the images are put together, and why arm64 has no CUDA image, is in the
[Server README](Dev-Server-README#local-ai-model-in-a-container).
