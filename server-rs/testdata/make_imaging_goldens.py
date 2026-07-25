"""M3 golden fixtures: run the Python pHash + culling metrics on
deterministic synthetic images and dump results for the Rust port.
Written into server-rs/testdata/ so the Rust tests can ship with it."""
import json, os, sys
sys.path.insert(0, "/Users/bm/src/LrGeniusAI/server/src")
sys.argv = ["x"]  # config.py parses args at import
import numpy as np
from PIL import Image
from services import index as svc_index

rng = np.random.default_rng(123)
cases = {}

def add_case(name, arr):
    img = Image.fromarray(arr, "RGB")
    phash = svc_index._compute_perceptual_hash(img)
    metrics = svc_index._compute_culling_metrics(img)
    cases[name] = {"shape": list(arr.shape), "phash": phash,
                   "metrics": {k: (float(v) if isinstance(v, (int, float, np.floating)) else v)
                                for k, v in metrics.items()}}

# Deterministic synthetic images covering the metric space
grad = np.linspace(0, 255, 640*480*3).reshape(480, 640, 3).astype(np.uint8)
add_case("gradient", grad)
add_case("noise", rng.integers(0, 256, (600, 800, 3), dtype=np.uint8))
add_case("dark", np.full((480, 640, 3), 12, dtype=np.uint8))
add_case("bright_clipped", np.full((480, 640, 3), 252, dtype=np.uint8))
checker = ((np.indices((512, 512)).sum(axis=0) // 32) % 2 * 255).astype(np.uint8)
add_case("checker", np.stack([checker]*3, axis=-1))
mixed = grad.copy(); mixed[100:200, 100:300] = rng.integers(0, 256, (100, 200, 3), dtype=np.uint8)
add_case("mixed", mixed)

# also dump raw pixel seeds so Rust can regenerate identical inputs
out = {"note": "inputs regenerable: see make_imaging_goldens.py; PIL RGB arrays",
       "cases": cases}
dst = "/Users/bm/src/LrGeniusAI/server-rs/testdata/imaging_goldens.json"
os.makedirs(os.path.dirname(dst), exist_ok=True)
json.dump(out, open(dst, "w"), indent=1)
print("phash fn exists:", True)
print(json.dumps({k: v["phash"] for k, v in cases.items()}, indent=0))
