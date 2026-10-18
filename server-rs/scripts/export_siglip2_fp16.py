"""
Export SigLIP2 So400m NaFlex (google/siglip2-so400m-patch16-naflex) to ONNX
for the Rust `ort` runtime, with fp16 internal weights to cut distribution
size while keeping fp32 inputs/outputs.

The fp16 graph is produced by tracing the model with `.half()` weights
directly (casting only at the wrapper's input/output boundary), NOT by
post-hoc graph conversion: `convert_float_to_float16` produced ONNX Runtime
load-time type mismatches on this architecture's attention blocks.

Image tower inputs (what `lrg-ml::siglip` feeds, see `naflex_pre.rs`):

    pixel_values          [1, 576, 768] f32   patchified, normalized, padded
    pixel_attention_mask  [1, 576]      i64   1 for real patches
    pos_interp_h          [gh, 16]      f32   rows of the position-embedding resize
    pos_interp_w          [gw, 16]      f32   columns of the same

Why `pos_interp_*` exist: transformers resizes the 16x16 position-embedding
grid to the image's (gh, gw) with `F.interpolate(..., antialias=True)`, whose
output size is data dependent, which no ONNX export survives. A bilinear
resize is separable, so it is `rows @ grid @ cols^T`; the Rust side computes
`rows`/`cols` (Pillow's resampling weights, the same ones the image resize
uses) and the graph does the two matmuls. Likewise the additive attention
mask is built here instead of tracing `create_bidirectional_mask`, and the
attention-pooling head is written out instead of `nn.MultiheadAttention`
(whose fast path does not export).

Run once per release:
    cd server-rs
    uv run --project scripts --with onnxscript \
        python scripts/export_siglip2_fp16.py --output-dir /tmp/siglip2-export

(`--with onnxscript`: torch >=2.9's `torch.onnx.export` imports it
unconditionally at call time, even with `dynamo=False`.)

That writes, next to the ONNX files, `siglip2_naflex_tokenizer.json` and
`siglip_goldens.json` (reference embeddings from transformers' own
`Siglip2Model` in fp32, driven by its own processor), and verifies the fp16
ONNX against them. Copy `siglip_goldens.json` to `testdata/` and publish it
as a release asset with the models.

Re-verify existing output without re-exporting:
    uv run --project scripts python scripts/export_siglip2_fp16.py --verify-only --output-dir /tmp/siglip2-export

Check the wrappers on a tiny random-weight model first (no download, seconds):
    uv run --project scripts --with onnxscript python scripts/export_siglip2_fp16.py --self-test
"""

from __future__ import annotations

import argparse
import faulthandler
import json
import sys
import tempfile
from pathlib import Path

import numpy as np

from naflex_towers import MASK_FILL, PATCH_DIM, POSITION_GRID, encode_vision, example_inputs, interp_matrix  # noqa: F401

# A bare SIGSEGV with no traceback is otherwise all CI gives us for a native
# crash in torch/onnxruntime.
faulthandler.enable()
sys.stdout.reconfigure(line_buffering=True)

REPO = "google/siglip2-so400m-patch16-naflex"
# Must match lrg-ml: naflex_pre::{MAX_NUM_PATCHES, POSITION_GRID}, siglip::MODEL_ID.
MAX_NUM_PATCHES = 576
MODEL_ID = "siglip2-so400m-naflex-576/fp16"
CONTEXT_LENGTH = 64
OPSET = 18

IMAGE_ONNX = "siglip2_naflex_image_fp16.onnx"
TEXT_ONNX = "siglip2_naflex_text_fp16.onnx"
TOKENIZER_JSON = "siglip2_naflex_tokenizer.json"
GOLDENS_JSON = "siglip_goldens.json"



def load_model():
    from transformers import Siglip2Model

    # eager: the SDPA path would trace a mask-handling branch we do not want.
    model = Siglip2Model.from_pretrained(REPO, attn_implementation="eager")
    model.eval()
    return model


def make_tiny_model():
    """Random-weight model with the real architecture's shape rules, small
    enough to export in seconds. The position grid must be 16x16."""
    import torch
    from transformers import Siglip2Config, Siglip2Model

    torch.manual_seed(0)
    config = Siglip2Config(
        text_config=dict(
            vocab_size=1000,
            hidden_size=64,
            intermediate_size=128,
            num_hidden_layers=2,
            num_attention_heads=4,
            max_position_embeddings=CONTEXT_LENGTH,
            projection_size=64,
        ),
        vision_config=dict(
            hidden_size=64,
            intermediate_size=128,
            num_hidden_layers=2,
            num_attention_heads=4,
            num_patches=POSITION_GRID * POSITION_GRID,
            patch_size=16,
        ),
        attn_implementation="eager",
    )
    model = Siglip2Model(config)
    model.eval()
    return model


def build_wrappers(model):
    import torch

    class ImageTower(torch.nn.Module):
        """fp32 in -> fp16 compute -> fp32 out, exactly the NaFlex forward of
        `Siglip2VisionTransformer` (see `naflex_towers.encode_vision`)."""

        def __init__(self, vision):
            super().__init__()
            self.v = vision

        def forward(self, pixel_values, pixel_attention_mask, pos_interp_h, pos_interp_w):
            pooled, _ = encode_vision(self.v, pixel_values, pixel_attention_mask, pos_interp_h, pos_interp_w)
            return pooled.float()

    class TextTower(torch.nn.Module):
        def __init__(self, text_model):
            super().__init__()
            self.t = text_model

        def forward(self, input_ids):
            # No attention mask: the model was trained on max_length padding
            # and transformers' own recipe passes none. ids stay int64.
            return self.t(input_ids=input_ids).pooler_output.float()

    return ImageTower(model.vision_model), TextTower(model.text_model)


def export_fp16(model, output_dir: Path) -> tuple[Path, Path]:
    import torch

    output_dir.mkdir(parents=True, exist_ok=True)
    image_path = output_dir / IMAGE_ONNX
    text_path = output_dir / TEXT_ONNX

    image_tower, text_tower = build_wrappers(model.half())

    # dynamo=False: the legacy TorchScript tracer. The dynamo exporter writes
    # large weights as external-data files, and this is the path the earlier
    # 384 export was validated on.
    with torch.no_grad():
        torch.onnx.export(
            image_tower,
            example_inputs(MAX_NUM_PATCHES, 18, 32),
            str(image_path),
            input_names=["pixel_values", "pixel_attention_mask", "pos_interp_h", "pos_interp_w"],
            output_names=["image_embeds"],
            dynamic_axes={"pos_interp_h": {0: "grid_h"}, "pos_interp_w": {0: "grid_w"}},
            opset_version=OPSET,
            dynamo=False,
        )
    print(f"Exported fp16 image tower -> {image_path} ({image_path.stat().st_size / 1e6:.1f} MB)")

    dummy_ids = torch.zeros(1, CONTEXT_LENGTH, dtype=torch.long)
    with torch.no_grad():
        torch.onnx.export(
            text_tower,
            (dummy_ids,),
            str(text_path),
            input_names=["input_ids"],
            output_names=["text_embeds"],
            dynamic_axes={"input_ids": {0: "batch"}, "text_embeds": {0: "batch"}},
            opset_version=OPSET,
            dynamo=False,
        )
    print(f"Exported fp16 text tower -> {text_path} ({text_path.stat().st_size / 1e6:.1f} MB)")
    return image_path, text_path


def export_tokenizer(output_dir: Path) -> Path:
    """The repo's own tokenizer already is the shape lrg-ml expects: Gemma
    BPE, `<eos>` (id 1) appended by a TemplateProcessing post-processor, no
    `<bos>`, pad id 0, 64 tokens. Assert that rather than assume it, then
    save the raw `tokenizers` object (not `save_pretrained`'s directory)."""
    from transformers import AutoTokenizer

    backend = AutoTokenizer.from_pretrained(REPO).backend_tokenizer
    # The saved tokenizer pads to 64; the real tokens end before the first pad.
    ids = [i for i in backend.encode("a photo of a dog").ids if i != 0]
    if ids[-1] != 1 or ids[0] == 2:
        raise RuntimeError(f"unexpected special tokens in tokenizer output: {ids}")
    dest = output_dir / TOKENIZER_JSON
    backend.save(str(dest))
    print(f"Exported tokenizer -> {dest}")
    return dest


# Mirrors lrg-ml::text_pre::clean_text (html-unescape twice + trim, underscore
# to space, strip this punctuation set, lowercase, collapse whitespace). The
# goldens go through it so they test the Rust cleaning too.
_PUNCTUATION = set("!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~")


def _clean_text(text: str) -> str:
    import html

    unescaped = html.unescape(html.unescape(text)).strip()
    no_underscore = unescaped.replace("_", " ")
    stripped = "".join(c for c in no_underscore if c not in _PUNCTUATION)
    return " ".join(stripped.lower().split())


def _token_ids(tok, text: str) -> np.ndarray:
    ids = tok.encode(_clean_text(text)).ids[:CONTEXT_LENGTH]
    return np.array([ids + [0] * (CONTEXT_LENGTH - len(ids))], dtype=np.int64)


# Must match goldens.rs / naflex_pre_goldens.rs `build_image` (same seeds and
# formulas): the goldens store embeddings, not pixels.
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
GOLDEN_TEXTS = [
    "a photo of a dog on a beach at sunset",
    "portrait of a woman with red hair",
    "ein Foto von einem Hund am Strand",
    "夕日のビーチにいる犬の写真",
    "Photo_of a CAT!!!  with   spaces",
]


def image_inputs(arr: np.ndarray, processor):
    """The tensors `siglip.rs` would feed, from transformers' own processor."""
    out = processor(images=[arr], return_tensors="np")
    gh, gw = (int(v) for v in out["spatial_shapes"][0])
    return {
        "pixel_values": out["pixel_values"].astype(np.float32),
        "pixel_attention_mask": out["pixel_attention_mask"].astype(np.int64),
        "pos_interp_h": interp_matrix(gh).numpy(),
        "pos_interp_w": interp_matrix(gw).numpy(),
    }, out["spatial_shapes"]


def reference_image_embedding(model, ort_inputs, spatial_shapes) -> np.ndarray:
    """fp32 transformers forward, with spatial_shapes and its own resize."""
    import torch

    with torch.no_grad():
        out = model.get_image_features(
            pixel_values=torch.from_numpy(ort_inputs["pixel_values"]),
            pixel_attention_mask=torch.from_numpy(ort_inputs["pixel_attention_mask"]),
            spatial_shapes=torch.from_numpy(np.asarray(spatial_shapes)),
        )
    return out.pooler_output[0].float().numpy()


def reference_text_embedding(model, ids: np.ndarray) -> np.ndarray:
    import torch

    with torch.no_grad():
        out = model.get_text_features(input_ids=torch.from_numpy(ids))
    return out.pooler_output[0].float().numpy()


def _cosine(a, b) -> float:
    a = np.asarray(a, dtype=np.float64)
    b = np.asarray(b, dtype=np.float64)
    return float(a @ b / (np.linalg.norm(a) * np.linalg.norm(b)))


def _session(path: Path):
    import onnxruntime as ort

    # Pinning threads avoids a known segfault on cgroup-restricted hosts
    # (microsoft/onnxruntime#7207), i.e. GitHub Actions runners.
    opts = ort.SessionOptions()
    opts.intra_op_num_threads = 1
    opts.inter_op_num_threads = 1
    return ort.InferenceSession(str(path), sess_options=opts, providers=["CPUExecutionProvider"])


def write_goldens(model, output_dir: Path) -> Path:
    from tokenizers import Tokenizer
    from transformers.models.siglip2.image_processing_pil_siglip2 import Siglip2ImageProcessorPil

    processor = Siglip2ImageProcessorPil(max_num_patches=MAX_NUM_PATCHES)
    images = {}
    for name in GOLDEN_IMAGES:
        arr = _make_golden_image(name)
        inputs, shapes = image_inputs(arr, processor)
        images[name] = {
            "width": int(arr.shape[1]),
            "height": int(arr.shape[0]),
            "embedding": reference_image_embedding(model, inputs, shapes).tolist(),
        }
    tok = Tokenizer.from_file(str(output_dir / TOKENIZER_JSON))
    texts = {t: reference_text_embedding(model, _token_ids(tok, t)).tolist() for t in GOLDEN_TEXTS}
    dest = output_dir / GOLDENS_JSON
    dest.write_text(json.dumps({"model_id": MODEL_ID, "images": images, "texts": texts}))
    print(f"Wrote goldens -> {dest}")
    return dest


def verify(output_dir: Path, goldens_path: Path) -> bool:
    from tokenizers import Tokenizer
    from transformers.models.siglip2.image_processing_pil_siglip2 import Siglip2ImageProcessorPil

    goldens = json.loads(goldens_path.read_text())
    image_sess = _session(output_dir / IMAGE_ONNX)
    text_sess = _session(output_dir / TEXT_ONNX)
    processor = Siglip2ImageProcessorPil(max_num_patches=MAX_NUM_PATCHES)

    ok = True
    print("\n=== Verifying fp16 ONNX against fp32 transformers goldens ===")
    for name, entry in goldens["images"].items():
        inputs, _ = image_inputs(_make_golden_image(name), processor)
        (emb,) = image_sess.run(["image_embeds"], inputs)
        cos = _cosine(emb[0], entry["embedding"])
        ok &= cos > 0.999
        print(f"  image '{name}': cosine={cos:.6f} [{'OK' if cos > 0.999 else 'FAIL'}]")

    tok = Tokenizer.from_file(str(output_dir / TOKENIZER_JSON))
    for text, golden in goldens["texts"].items():
        (emb,) = text_sess.run(["text_embeds"], {"input_ids": _token_ids(tok, text)})
        cos = _cosine(emb[0], golden)
        ok &= cos > 0.999
        print(f"  text '{text[:40]}': cosine={cos:.6f} [{'OK' if cos > 0.999 else 'FAIL'}]")
    return ok


def self_test() -> bool:
    """Export a tiny random-weight model and compare the ONNX graphs with the
    transformers forward, over grids that pad, fill, and are non-square. This
    is the check that the hand-written wrappers equal the real forward."""
    from transformers.models.siglip2.image_processing_pil_siglip2 import Siglip2ImageProcessorPil

    model = make_tiny_model()
    ok = True
    with tempfile.TemporaryDirectory() as tmp:
        out = Path(tmp)
        export_fp16(model, out)  # converts `model` to half in place
        # Reference in fp32: rebuild with the same seed.
        reference = make_tiny_model()
        image_sess = _session(out / IMAGE_ONNX)
        text_sess = _session(out / TEXT_ONNX)
        processor = Siglip2ImageProcessorPil(max_num_patches=MAX_NUM_PATCHES)
        print("\n=== Self-test: tiny random model ===")
        references = []
        for name in GOLDEN_IMAGES:
            inputs, shapes = image_inputs(_make_golden_image(name), processor)
            (emb,) = image_sess.run(["image_embeds"], inputs)
            want = reference_image_embedding(reference, inputs, shapes)
            references.append(want)
            cos = _cosine(emb[0], want)
            ok &= cos > 0.999
            print(f"  image '{name}' grid {tuple(shapes[0])}: cosine={cos:.6f} [{'OK' if cos > 0.999 else 'FAIL'}]")
        # A near-constant random model would pass any comparison; make sure
        # the references actually tell the images apart.
        spread = max(1.0 - _cosine(a, b) for a in references for b in references)
        ok &= spread > 1e-3
        print(f"  reference spread (1 - min pairwise cosine): {spread:.4f} [{'OK' if spread > 1e-3 else 'FAIL'}]")
        rng = np.random.default_rng(0)
        ids = rng.integers(2, 1000, size=(1, CONTEXT_LENGTH)).astype(np.int64)
        (emb,) = text_sess.run(["text_embeds"], {"input_ids": ids})
        cos = _cosine(emb[0], reference_text_embedding(reference, ids))
        ok &= cos > 0.999
        print(f"  text: cosine={cos:.6f} [{'OK' if cos > 0.999 else 'FAIL'}]")
    return ok


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--verify-only", action="store_true", help="Skip export; verify existing output-dir contents")
    parser.add_argument("--self-test", action="store_true", help="Check the wrappers on a tiny random model")
    args = parser.parse_args()

    if args.self_test:
        sys.exit(0 if self_test() else 1)
    if args.output_dir is None:
        parser.error("--output-dir is required")

    goldens_path = args.output_dir / GOLDENS_JSON
    if not args.verify_only:
        model = load_model()
        # Goldens first: they need the fp32 weights, and export_fp16 halves
        # the model in place.
        args.output_dir.mkdir(parents=True, exist_ok=True)
        export_tokenizer(args.output_dir)
        write_goldens(model, args.output_dir)
        export_fp16(model, args.output_dir)

    if not verify(args.output_dir, goldens_path):
        print("\nVerification FAILED — fp16 export differs from the fp32 reference beyond tolerance.", file=sys.stderr)
        sys.exit(1)
    print("\nVerification passed.")


if __name__ == "__main__":
    main()
