"""M3 golden fixtures: run the Python pHash + culling metrics on
deterministic synthetic images and dump results for the Rust port.

Inputs use only formulas reproducible in Rust (linear gradients, checkers,
constants, and a 31-bit LCG) -- no numpy RNG -- so the Rust tests regenerate
identical pixel buffers instead of shipping megabytes of pixels.

Run:  cd server && uv run python ../server-rs/testdata/make_imaging_goldens.py
"""

import json
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "../../server/src"))
sys.argv = ["x"]  # config.py parses argv at import time

import numpy as np
from PIL import Image
from services import index as svc_index


def lcg_bytes(n, seed=42):
    """31-bit LCG (glibc constants), byte = (x >> 16) & 0xFF."""
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


def checker(size, cell):
    board = ((np.indices((size, size)).sum(axis=0) // cell) % 2 * 255).astype(np.uint8)
    return np.stack([board] * 3, axis=-1)


cases = {}


def add_case(name, arr):
    img = Image.fromarray(arr, "RGB")
    cases[name] = {
        "width": int(arr.shape[1]),
        "height": int(arr.shape[0]),
        "phash": svc_index._compute_perceptual_hash(img),
        "metrics": {
            k: float(v) for k, v in svc_index._compute_culling_metrics(img).items()
        },
    }


add_case("gradient_640x480", gradient(640, 480))
add_case("lcg_noise_800x600", lcg_bytes(800 * 600 * 3).reshape(600, 800, 3))
add_case("dark_640x480", np.full((480, 640, 3), 12, dtype=np.uint8))
add_case("bright_640x480", np.full((480, 640, 3), 252, dtype=np.uint8))
add_case("checker_512", checker(512, 32))
mixed = gradient(640, 480).copy()
mixed[100:200, 100:300] = lcg_bytes(100 * 200 * 3, seed=7).reshape(100, 200, 3)
add_case("mixed_640x480", mixed)
# Small image: exercises the no-resize path (< 512).
add_case("small_320x240", lcg_bytes(320 * 240 * 3, seed=99).reshape(240, 320, 3))

dst = os.path.join(os.path.dirname(os.path.abspath(__file__)), "imaging_goldens.json")
json.dump(
    {"note": "inputs regenerated in Rust tests (see tests/goldens.rs)", "cases": cases},
    open(dst, "w"),
    indent=1,
)
print(json.dumps({k: v["phash"] for k, v in cases.items()}, indent=0))
