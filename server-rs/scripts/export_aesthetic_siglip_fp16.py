"""
Export somepago/AestheticSigLIP to ONNX for the Rust `ort` runtime, fp16.

AestheticSigLIP is a SigLIP 2 So400m NaFlex encoder fine-tuned to score photos
1..9 (Apache-2.0). It is its own backbone, not a head on the embedding the
search model stores: the score comes from the pooled output plus the masked
mean of two intermediate layers (taps), fed to an MLP over score buckets, and
the score is the expectation over the bucket centers.

Same four inputs as the SigLIP2 image tower (see `naflex_towers.py`), but at
the 256 patches it was trained with:

    pixel_values          [1, 256, 768] f32
    pixel_attention_mask  [1, 256]      i64
    pos_interp_h          [gh, 16]      f32
    pos_interp_w          [gw, 16]      f32

Outputs: `score` [1] (the expectation, 1..9) and `bucket_probs` [1, K].

**Supply chain.** `best.pt` is a pickle and the model code is third-party
Python that this script imports, so both are pinned: the repository revision,
the SHA-256 of the checkpoint, and the SHA-256 of every file that is imported
(all of which were read before pinning). Anything else fails before it runs.
The checkpoint is loaded with `weights_only=True` and an allow-list that holds
only the dataclasses of its config and numpy's scalar types.

Run once per release:
    cd server-rs
    uv run --project scripts --with onnxscript \
        python scripts/export_aesthetic_siglip_fp16.py --output-dir /tmp/aesthetic-export

That writes `aesthetic_siglip_fp16.onnx` and `aesthetic_goldens.json` (reference
scores from the authors' own fp32 code and preprocessing) and verifies the fp16
ONNX against them. Check the wrappers first on a tiny random model:
    uv run --project scripts --with onnxscript python scripts/export_aesthetic_siglip_fp16.py --self-test
"""

from __future__ import annotations

import argparse
import faulthandler
import hashlib
import json
import sys
import tempfile
from pathlib import Path

import numpy as np

from naflex_towers import POSITION_GRID, encode_vision, example_inputs, interp_matrix

faulthandler.enable()
sys.stdout.reconfigure(line_buffering=True)

REPO = "somepago/AestheticSigLIP"
REVISION = "25cb379f31159d4bae26c4314ec9482fea03dddd"
CHECKPOINT = "best.pt"
CHECKPOINT_SHA256 = "1b6d3bc3711b4cb2c3eadbb92e2b9224aed0fad168d8b703ea11f23b5798eaab"
CODE_SHA256 = {
    "config.py": "cb60830ddb5d94095321d7294c2a2757882f566eaaa4af0e1b87a003f3f00566",
    "model/__init__.py": "ea9560967318576a3cea77ed06981be0a76931ea8e34a516a519e084350e4bba",
    "model/aes_head.py": "3bac1db3988f23417e07e297bb1ecca8f1113b6a89edab8093239d2f67721cf0",
    "model/siglip2.py": "88ebf4645877bd545b2234230b074f251844515a817dd090e5dcb5b61730bc96",
    "naflex.py": "2601bfc4d9034fb5563a04cc06b487a0ee7b3ae75f3552837ae5f5fd2a9b054b",
}

# Must match lrg-ml: aesthetic::{MAX_NUM_PATCHES, MODEL_ID}.
MAX_NUM_PATCHES = 256
MODEL_ID = "aesthetic-siglip2-naflex-256/fp16"
OPSET = 18

AESTHETIC_ONNX = "aesthetic_siglip_fp16.onnx"
GOLDENS_JSON = "aesthetic_goldens.json"
PRE_GOLDENS_JSON = "aesthetic_pre_goldens.json"


def _sha256(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def fetch_authors_code() -> Path:
    """The authors' config/model/preprocessing code at the pinned revision,
    each file checked against the hash it had when it was reviewed."""
    from huggingface_hub import snapshot_download

    root = Path(snapshot_download(REPO, revision=REVISION, allow_patterns=sorted(CODE_SHA256)))
    for name, want in CODE_SHA256.items():
        got = _sha256(root / name)
        if got != want:
            raise RuntimeError(f"{name} at {REPO}@{REVISION} is {got}, expected {want}; review it before pinning")
    return root


def load_authors_modules(root: Path):
    """Import the reviewed files. `config` and `model` are top-level names in
    their repo, so the checkout goes on sys.path."""
    sys.path.insert(0, str(root))
    import config  # type: ignore
    import naflex  # type: ignore
    from model import AestheticModel  # type: ignore

    return config, naflex, AestheticModel


def load_checkpoint(config_module):
    import torch
    from huggingface_hub import hf_hub_download

    path = Path(hf_hub_download(REPO, CHECKPOINT, revision=REVISION))
    got = _sha256(path)
    if got != CHECKPOINT_SHA256:
        raise RuntimeError(f"{CHECKPOINT} is {got}, expected {CHECKPOINT_SHA256}")
    # The pickle holds the training config (two dataclasses) and numpy scalars
    # in its metrics; nothing else is allowed to be constructed.
    torch.serialization.add_safe_globals(
        [
            config_module.TrainConfig,
            config_module.SigLIP2VisionConfig,
            np._core.multiarray.scalar,
            np.dtype,
            np.dtypes.Float32DType,
            np.dtypes.Float64DType,
            np.dtypes.Int64DType,
        ]
    )
    ckpt = torch.load(path, map_location="cpu", weights_only=True)
    state = ckpt.get("ema_state_dict") or ckpt["model_state_dict"]
    return ckpt["config"], state


def make_tiny(config_module, AestheticModel):
    """Random-weight model with the real architecture's shape rules, small
    enough to export in seconds. The position grid must be 16x16."""
    import torch

    torch.manual_seed(0)
    cfg = config_module.TrainConfig(
        vision=config_module.SigLIP2VisionConfig(
            hidden_size=64,
            intermediate_size=128,
            num_hidden_layers=4,
            num_attention_heads=4,
            max_num_patches=POSITION_GRID * POSITION_GRID,
        ),
        tap_layers=[1, 2],
        head_hidden=[32, 16],
        score_buckets=[0, 3, 4, 5, 6, 7, 10],
    )
    model = AestheticModel(cfg)
    # Random init leaves the head near-constant; spread it so the comparison
    # can tell a wrong feature from a right one.
    for p in model.head.parameters():
        torch.nn.init.normal_(p, std=0.3)
    model.eval()
    return cfg, model.state_dict()


def build_hf_vision(cfg, state):
    """The transformers vision tower carrying the authors' weights. Their
    module names are transformers' own, under a `vision.` prefix."""
    from transformers import Siglip2VisionConfig, Siglip2VisionModel

    v = cfg.vision
    hf_cfg = Siglip2VisionConfig(
        hidden_size=v.hidden_size,
        intermediate_size=v.intermediate_size,
        num_hidden_layers=v.num_hidden_layers,
        num_attention_heads=v.num_attention_heads,
        num_channels=v.num_channels,
        patch_size=v.patch_size,
        num_patches=v.max_num_patches,
        layer_norm_eps=v.layer_norm_eps,
        hidden_act="gelu_pytorch_tanh",
        attn_implementation="eager",
    )
    model = Siglip2VisionModel(hf_cfg)
    weights = {k[len("vision.") :]: t for k, t in state.items() if k.startswith("vision.")}
    model.load_state_dict(weights, strict=True)
    model.eval()
    return model


def build_authors_model(cfg, state, AestheticModel):
    model = AestheticModel(cfg)
    model.load_state_dict(state, strict=True)
    model.eval()
    return model


def build_tower(cfg, state, vision):
    """fp32 in -> fp16 encoder -> fp32 head -> (score, bucket_probs)."""
    import torch

    from_state_head = {k[len("head.") :]: t for k, t in state.items() if k.startswith("head.")}

    class AestheticTower(torch.nn.Module):
        def __init__(self):
            super().__init__()
            self.v = vision
            self.tap_layers = list(cfg.tap_layers)
            # The head is tiny and sees features of very different scales
            # (LayerNorm over the concatenation), so it stays fp32.
            self.norm = torch.nn.LayerNorm(len(self.tap_layers) * cfg.vision.hidden_size + cfg.vision.hidden_size)
            self.net = torch.nn.Sequential(
                *[
                    m
                    for m in _head_layers(cfg)
                ]
            )
            self.register_buffer("centers", state["bucket_centers"].float().clone())

        def forward(self, pixel_values, pixel_attention_mask, pos_interp_h, pos_interp_w):
            pooled, taps = encode_vision(
                self.v, pixel_values, pixel_attention_mask, pos_interp_h, pos_interp_w, self.tap_layers
            )
            features = torch.cat([pooled.float()] + taps, dim=-1)
            logits = self.net(self.norm(features))
            probs = torch.softmax(logits, dim=-1)
            return (probs * self.centers).sum(dim=-1), probs

    tower = AestheticTower()
    tower.norm.load_state_dict({k[len("norm.") :]: t for k, t in from_state_head.items() if k.startswith("norm.")})
    tower.net.load_state_dict({k[len("net.") :]: t for k, t in from_state_head.items() if k.startswith("net.")})
    return tower


def _head_layers(cfg):
    """The authors' `AestheticHead.net`, layer for layer: Linear, GELU, Dropout
    between all but the last hidden pair, then the output Linear. Indices must
    match the checkpoint (`head.net.0`, `.3`, `.4` for the shipped config)."""
    import torch

    dims = [(len(cfg.tap_layers) + 1) * cfg.vision.hidden_size] + list(cfg.head_hidden)
    layers = []
    for i in range(len(dims) - 1):
        layers.append(torch.nn.Linear(dims[i], dims[i + 1]))
        if i < len(dims) - 2:
            layers.append(torch.nn.GELU())
            layers.append(torch.nn.Dropout(cfg.head_dropout))
    layers.append(torch.nn.Linear(dims[-1], cfg.num_buckets))
    return layers


def export_fp16(cfg, state, output_dir: Path) -> Path:
    import torch

    output_dir.mkdir(parents=True, exist_ok=True)
    path = output_dir / AESTHETIC_ONNX
    vision = build_hf_vision(cfg, state)
    tower = build_tower(cfg, state, vision)
    tower.v.half()  # the encoder only; the head and centers stay fp32
    tower.eval()
    max_patches = cfg.vision.max_num_patches
    gh, gw = (12, 20) if max_patches >= 240 else (6, 8)
    with torch.no_grad():
        torch.onnx.export(
            tower,
            example_inputs(max_patches, gh, gw),
            str(path),
            input_names=["pixel_values", "pixel_attention_mask", "pos_interp_h", "pos_interp_w"],
            output_names=["score", "bucket_probs"],
            dynamic_axes={"pos_interp_h": {0: "grid_h"}, "pos_interp_w": {0: "grid_w"}},
            opset_version=OPSET,
            dynamo=False,
        )
    print(f"Exported fp16 aesthetic tower -> {path} ({path.stat().st_size / 1e6:.1f} MB)")
    return path


# Must match goldens.rs / naflex_pre_goldens.rs `build_image` (same seeds and
# formulas): the goldens store scores, not pixels.
def _lcg_bytes(n, seed=42):
    out = np.empty(n, dtype=np.uint8)
    x = seed
    for i in range(n):
        x = (x * 1103515245 + 12345) & 0x7FFFFFFF
        out[i] = (x >> 16) & 0xFF
    return out


def _gradient(w, h):
    n = w * h * 3
    vals = (np.arange(n, dtype=np.float64) * 255.0 / (n - 1)).astype(np.uint8)
    return vals.reshape(h, w, 3)


def _make_golden_image(name: str) -> np.ndarray:
    if name == "gradient_640x480":
        return _gradient(640, 480)
    if name == "lcg_noise_800x600":
        return _lcg_bytes(800 * 600 * 3).reshape(600, 800, 3)
    if name == "small_320x240":
        return _lcg_bytes(320 * 240 * 3, seed=99).reshape(240, 320, 3)
    if name == "portrait_300x900":
        return _lcg_bytes(300 * 900 * 3, seed=7).reshape(900, 300, 3)
    if name == "panorama_1200x200":
        return _gradient(1200, 200)
    raise ValueError(f"unknown golden image name: {name}")


GOLDEN_IMAGES = [
    "gradient_640x480",
    "lcg_noise_800x600",
    "small_320x240",
    "portrait_300x900",
    "panorama_1200x200",
]
# Sizes for the grid heuristic golden: aspect ratios from extreme panoramas to
# extreme portraits, tiny and huge, squares, and the awkward in-betweens.
GRID_SIZES = [
    (w, h)
    for w, h in [
        (640, 480), (800, 600), (320, 240), (300, 900), (1200, 200), (1000, 1000), (1, 1), (16, 16), (50, 3000),
        (3000, 50), (6000, 4000), (4000, 6000), (5472, 3648), (3648, 5472), (1920, 1080), (1080, 1920), (4032, 3024),
        (3024, 4032), (1000, 667), (667, 1000), (1024, 768), (768, 1024), (2048, 1365), (1365, 2048), (500, 333),
        (333, 500), (2560, 1080), (1080, 2560), (7680, 2160), (123, 457), (457, 123), (255, 256), (256, 255),
        (100, 400), (400, 100), (1600, 1200), (1200, 1600), (3840, 2160), (2160, 3840), (60, 60), (17, 5),
    ]
]


def authors_inputs(arr: np.ndarray, naflex_module):
    """The tensors `siglip.rs` would feed, from the authors' own preprocessing."""
    import torch
    from PIL import Image

    patches, (gh, gw) = naflex_module.preprocess_image(Image.fromarray(arr, "RGB"))
    n = patches.shape[0]
    padded = torch.zeros(MAX_NUM_PATCHES, patches.shape[1])
    padded[:n] = patches
    mask = torch.zeros(MAX_NUM_PATCHES, dtype=torch.long)
    mask[:n] = 1
    return {
        "pixel_values": padded[None].numpy().astype(np.float32),
        "pixel_attention_mask": mask[None].numpy().astype(np.int64),
        "pos_interp_h": interp_matrix(gh).numpy(),
        "pos_interp_w": interp_matrix(gw).numpy(),
    }, patches, (gh, gw)


def reference_score(authors_model, patches, grid):
    """The authors' own forward in fp32: batch of one, no padding."""
    import torch

    with torch.inference_mode():
        logits = authors_model(
            patches[None], torch.tensor([list(grid)], dtype=torch.long), torch.ones(1, patches.shape[0])
        )
        probs = torch.softmax(logits, dim=-1)[0]
        return float(authors_model.logits_to_score(logits)[0]), probs.numpy()


def _session(path: Path):
    import onnxruntime as ort

    # Pinning threads avoids a known segfault on cgroup-restricted hosts
    # (microsoft/onnxruntime#7207), i.e. GitHub Actions runners.
    opts = ort.SessionOptions()
    opts.intra_op_num_threads = 1
    opts.inter_op_num_threads = 1
    return ort.InferenceSession(str(path), sess_options=opts, providers=["CPUExecutionProvider"])


def write_goldens(authors_model, naflex_module, output_dir: Path):
    images = {}
    pre = {"max_num_patches": MAX_NUM_PATCHES, "images": {}, "grids": {}}
    for name in GOLDEN_IMAGES:
        arr = _make_golden_image(name)
        _, patches, grid = authors_inputs(arr, naflex_module)
        score, probs = reference_score(authors_model, patches, grid)
        images[name] = {"width": int(arr.shape[1]), "height": int(arr.shape[0]), "score": score, "probs": probs.tolist()}
        n = grid[0] * grid[1]
        pv = patches.numpy().astype(np.float64)
        rows = sorted({0, n // 2, n - 1})
        pre["images"][name] = {
            "grid_h": grid[0],
            "grid_w": grid[1],
            "sum": float(pv.sum()),
            "sum_sq": float((pv**2).sum()),
            "rows": {str(r): pv[r, :32].tolist() for r in rows},
        }
    for w, h in GRID_SIZES:
        pre["grids"][f"{w}x{h}"] = list(naflex_module.compute_patch_grid(h, w, 16, MAX_NUM_PATCHES))
    goldens = output_dir / GOLDENS_JSON
    goldens.write_text(json.dumps({"model_id": MODEL_ID, "images": images}))
    pre_path = output_dir / PRE_GOLDENS_JSON
    pre_path.write_text(json.dumps(pre))
    print(f"Wrote goldens -> {goldens}, {pre_path}")
    return goldens


def verify(output_dir: Path, goldens_path: Path, naflex_module, tolerance=0.05) -> bool:
    goldens = json.loads(goldens_path.read_text())
    sess = _session(output_dir / AESTHETIC_ONNX)
    ok = True
    print("\n=== Verifying fp16 ONNX against the authors' fp32 scores ===")
    for name, entry in goldens["images"].items():
        inputs, _, _ = authors_inputs(_make_golden_image(name), naflex_module)
        score, probs = sess.run(["score", "bucket_probs"], inputs)
        diff = abs(float(score[0]) - entry["score"])
        tv = 0.5 * float(np.abs(probs[0] - np.array(entry["probs"])).sum())
        good = diff < tolerance
        ok &= good
        print(
            f"  '{name}': score {float(score[0]):.3f} vs {entry['score']:.3f} (diff {diff:.4f}), "
            f"bucket distribution distance {tv:.4f} [{'OK' if good else 'FAIL'}]"
        )
    return ok


def self_test() -> bool:
    """Export a tiny random-weight model and compare the ONNX graph with the
    authors' own forward, over grids that pad, fill, and are non-square. This
    is the check that the transformers-based wrapper equals their model."""
    root = fetch_authors_code()
    config_module, naflex_module, AestheticModel = load_authors_modules(root)
    cfg, state = make_tiny(config_module, AestheticModel)
    reference = build_authors_model(cfg, state, AestheticModel)
    ok = True
    with tempfile.TemporaryDirectory() as tmp:
        out = Path(tmp)
        export_fp16(cfg, state, out)
        sess = _session(out / AESTHETIC_ONNX)
        print("\n=== Self-test: tiny random model ===")
        scores = []
        for name in GOLDEN_IMAGES:
            arr = _make_golden_image(name)
            inputs, patches, grid = authors_inputs(arr, naflex_module)
            # The tiny model has a 16x16 grid but the same 256-patch budget.
            score, _ = sess.run(["score", "bucket_probs"], inputs)
            want, _ = reference_score(reference, patches, grid)
            scores.append(want)
            diff = abs(float(score[0]) - want)
            good = diff < 0.02
            ok &= good
            print(f"  '{name}' grid {grid}: {float(score[0]):.4f} vs {want:.4f} (diff {diff:.4f}) [{'OK' if good else 'FAIL'}]")
        spread = max(scores) - min(scores)
        ok &= spread > 0.01
        print(f"  reference spread {spread:.4f} [{'OK' if spread > 0.01 else 'FAIL'}]")
    return ok


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--verify-only", action="store_true", help="Skip export; verify existing output-dir contents")
    parser.add_argument("--self-test", action="store_true", help="Check the wrapper on a tiny random model")
    args = parser.parse_args()

    if args.self_test:
        sys.exit(0 if self_test() else 1)
    if args.output_dir is None:
        parser.error("--output-dir is required")

    root = fetch_authors_code()
    config_module, naflex_module, AestheticModel = load_authors_modules(root)
    goldens_path = args.output_dir / GOLDENS_JSON
    if not args.verify_only:
        cfg, state = load_checkpoint(config_module)
        args.output_dir.mkdir(parents=True, exist_ok=True)
        authors = build_authors_model(cfg, state, AestheticModel)
        write_goldens(authors, naflex_module, args.output_dir)
        export_fp16(cfg, state, args.output_dir)

    if not verify(args.output_dir, goldens_path, naflex_module):
        print("\nVerification FAILED — the fp16 export differs from the authors' fp32 scores.", file=sys.stderr)
        sys.exit(1)
    print("\nVerification passed.")


if __name__ == "__main__":
    main()
