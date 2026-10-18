"""Goldens for `lrg-ml::naflex_pre`: SigLIP2 NaFlex preprocessing as
transformers runs it, on the same synthetic images `goldens.rs` builds.

Needs no model weights and no network — only `transformers`, `numpy`,
`pillow` and `torch` (the last for the position-embedding resize, which is
`F.interpolate` and nothing else). Run it from the export project:

    uv run --project server-rs/scripts python server-rs/testdata/make_naflex_pre_goldens.py

Uses `Siglip2ImageProcessorPil` on purpose: the Rust port mirrors Pillow's
8-bit resize bit for bit, and the torchvision backend resizes differently.
The full pixel arrays are too large to commit, so each image keeps its patch
grid, checksums over the real patches, and a few raw rows.
"""

import json
from pathlib import Path

import numpy as np
import torch
import torch.nn.functional as F
from transformers.models.siglip2.image_processing_pil_siglip2 import Siglip2ImageProcessorPil

MAX_NUM_PATCHES = 576  # naflex_pre::MAX_NUM_PATCHES
POSITION_GRID = 16  # naflex_pre::POSITION_GRID


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


# The first three match make_siglip_goldens.py / goldens.rs; the last two
# exercise the aspect-ratio handling the fixed 384 crop never had.
IMAGES = {
    "gradient_640x480": lambda: gradient(640, 480),
    "lcg_noise_800x600": lambda: lcg_bytes(800 * 600 * 3).reshape(600, 800, 3),
    "small_320x240": lambda: lcg_bytes(320 * 240 * 3, seed=99).reshape(240, 320, 3),
    "portrait_300x900": lambda: lcg_bytes(300 * 900 * 3, seed=7).reshape(900, 300, 3),
    "panorama_1200x200": lambda: gradient(1200, 200),
}

# Patch rows kept verbatim: first, one in the middle, last real one.
SAMPLE_VALUES = 32

processor = Siglip2ImageProcessorPil(max_num_patches=MAX_NUM_PATCHES)

images = {}
for name, make in IMAGES.items():
    arr = make()
    out = processor(images=[arr], return_tensors="np")
    pv = out["pixel_values"][0].astype(np.float64)
    mask = out["pixel_attention_mask"][0]
    gh, gw = (int(v) for v in out["spatial_shapes"][0])
    n = gh * gw
    assert int(mask.sum()) == n
    rows = sorted({0, n // 2, n - 1})
    images[name] = {
        "width": int(arr.shape[1]),
        "height": int(arr.shape[0]),
        "grid_h": gh,
        "grid_w": gw,
        "sum": float(pv[:n].sum()),
        "sum_sq": float((pv[:n] ** 2).sum()),
        "rows": {str(r): pv[r, :SAMPLE_VALUES].tolist() for r in rows},
    }

# The position-embedding resize the model runs (bilinear, antialias), on a
# fixed pseudo-random grid, for grids that both up- and downsample.
gen = torch.Generator().manual_seed(0)
grid = torch.randn(1, 4, POSITION_GRID, POSITION_GRID, generator=gen)
interpolations = []
for gh, gw in [(24, 24), (18, 32), (8, 72), (16, 16), (5, 3)]:
    resized = F.interpolate(grid, size=(gh, gw), mode="bilinear", align_corners=False, antialias=True)
    interpolations.append({"grid_h": gh, "grid_w": gw, "resized": resized[0].flatten().tolist()})

dst = Path(__file__).with_name("naflex_pre_goldens.json")
json.dump(
    {
        "max_num_patches": MAX_NUM_PATCHES,
        "images": images,
        "position_grid": grid[0].flatten().tolist(),
        "position_interpolations": interpolations,
    },
    open(dst, "w"),
)
print("wrote", dst)
