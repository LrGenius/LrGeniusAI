"""Golden fixtures for BioCLIP 2: real image-tower embeddings from the torch
model on deterministic synthetic images, so the Rust `ort` pipeline can be
verified end-to-end rather than just the exported ONNX graph in isolation.

The point of these is `lrg-ml::bioclip_pre`. Its transform differs from the
SigLIP2 one on four axes at once (224 not 384, squash not center-crop,
bilinear not bicubic, resize in f32 not u8), and getting any of them subtly
wrong yields embeddings that still look fine and classify slightly worse
forever. Comparing against torch's own transform is what catches that.

Run from `server-rs/`:
    uv run --project scripts python testdata/make_bioclip_goldens.py

Note this uses *pybioclip's* transform, not `open_clip`'s. For TreeOfLife
models pybioclip deliberately overrides the checkpoint's own preprocessing
(`BaseClassifier.load_pretrained_model`), and it is that override the shipped
weights are used with in practice.
"""

import json
import os

import numpy as np
import open_clip
import torch
from PIL import Image
from torchvision import transforms

MODEL_STR = "hf-hub:imageomics/bioclip-2"
IMAGE_SIZE = 224

model = open_clip.create_model(MODEL_STR)
model.eval()

# Verbatim from pybioclip's `predict.preprocess_img`.
preprocess = transforms.Compose(
    [
        transforms.ToTensor(),
        transforms.Resize((IMAGE_SIZE, IMAGE_SIZE), antialias=True),
        transforms.Normalize(
            mean=(0.48145466, 0.4578275, 0.40821073),
            std=(0.26862954, 0.26130258, 0.27577711),
        ),
    ]
)


def lcg_bytes(n, seed=42):
    out = np.empty(n, dtype=np.uint8)
    x = seed
    for i in range(n):
        x = (x * 1103515245 + 12345) & 0x7FFFFFFF
        out[i] = (x >> 16) & 0xFF
    return out


def gradient(w, h):
    n = w * h * 3
    vals = (np.arange(n, dtype=np.float64) * 255.0 / (n - 1)).astype(np.uint8)
    return vals.reshape(h, w, 3)


# Three aspect ratios on purpose: 4:3 both ways plus a small one, so a squash
# that accidentally preserves aspect ratio, or a resize that upscales rather
# than downscales, both show up.
images = {
    "gradient_640x480": gradient(640, 480),
    "lcg_noise_800x600": lcg_bytes(800 * 600 * 3).reshape(600, 800, 3),
    "small_320x240": lcg_bytes(320 * 240 * 3, seed=99).reshape(240, 320, 3),
}

out = {"model": MODEL_STR, "images": {}}
with torch.no_grad():
    for name, arr in images.items():
        tensor = preprocess(Image.fromarray(arr, "RGB")).unsqueeze(0)
        embedding = model.encode_image(tensor)[0].float().numpy()
        out["images"][name] = {"embedding": [float(v) for v in embedding]}
        print(f"{name}: {embedding.shape[0]} dims, norm {np.linalg.norm(embedding):.4f}")

dest = os.path.join(os.path.dirname(__file__), "bioclip_goldens.json")
with open(dest, "w") as fh:
    json.dump(out, fh)
print(f"wrote {dest}")
