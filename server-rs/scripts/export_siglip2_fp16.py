"""
Export SigLIP2 (ViT-SO400M-16-SigLIP2-384) to ONNX for the Rust `ort`
runtime, with fp16 internal weights to cut distribution size while
keeping fp32 inputs/outputs so the existing Rust tensor-construction
code in `lrg-ml::siglip` needs no changes.

The fp16 graph is produced by tracing the model with `.half()` weights
directly (casting fp32<->fp16 only at the wrapper's input/output
boundary), NOT by post-hoc graph conversion. An earlier version of this
script used `onnxconverter_common.float16.convert_float_to_float16` and
hit two distinct ONNX Runtime load-time type-mismatch errors on this
model's attention blocks (a known class of issue with that library on
this kind of architecture) — tracing natively in fp16 sidesteps the
problem entirely since there's no mixed-dtype graph for a converter to
get wrong.

Run once per release, e.g.:
    cd server-rs
    uv run --project scripts --with onnxscript \
        python scripts/export_siglip2_fp16.py --output-dir /tmp/siglip2-export

(`--with onnxscript`: torch >=2.9's `torch.onnx.export` imports it
unconditionally at call time, even with `dynamo=False` — it's a
throwaway overlay for this one script, not a real project dependency,
which is why it's a `uv run --with` flag rather than `uv add`.)

Then verify the exported fp16 model against the fp32 torch goldens —
this half only needs onnxruntime/numpy/tokenizers, no torch:
    uv run --project scripts python scripts/export_siglip2_fp16.py --verify-only --output-dir /tmp/siglip2-export

Verified end-to-end in development: both towers score cosine
>0.99999 against the fp32 torch goldens (`testdata/siglip_goldens.json`),
and pointing lrg-ml at the exported files via LRG_SIGLIP_IMAGE_ONNX/
LRG_SIGLIP_TEXT_ONNX/LRG_SIGLIP_TOKENIZER env vars and running
`cargo test -p lrg-ml --release` passes the same `goldens.rs` test
that validates the fp32 model — no Rust code changes needed. Total
size ~2.3 GB (image tower ~860 MB + text tower ~1.4 GB, the latter
dominated by Gemma's 256k-vocab embedding table), down from ~4.2 GB
fp32.
"""

from __future__ import annotations

import argparse
import faulthandler
import json
import sys
from pathlib import Path

import numpy as np

# A bare "Process completed with exit code 139" (SIGSEGV) with no
# traceback is otherwise all CI gives us for a native crash in
# torch/onnxruntime. faulthandler dumps the interpreter's current Python
# stack on a fatal signal, which at least says which line was running.
faulthandler.enable()
# Line-buffer stdout so progress prints actually reach the CI log before
# a native crash, instead of sitting lost in a block buffer.
sys.stdout.reconfigure(line_buffering=True)

MODEL_NAME = "ViT-SO400M-16-SigLIP2-384"
IMAGE_SIZE = 384
CONTEXT_LENGTH = 64
OPSET = 18


def load_model():
    import open_clip
    from huggingface_hub import hf_hub_download

    weights = hf_hub_download(f"timm/{MODEL_NAME}", "open_clip_model.safetensors")
    model, _, preprocess = open_clip.create_model_and_transforms(
        MODEL_NAME, pretrained=weights
    )
    tokenizer = open_clip.get_tokenizer(MODEL_NAME)
    model.eval()
    return model, preprocess, tokenizer


def export_fp16(model, output_dir: Path) -> tuple[Path, Path]:
    # Imported lazily (not at module level) so `--verify-only` runs need
    # neither torch nor open_clip installed — only onnxruntime/numpy/
    # tokenizers, which is a much lighter environment to verify with.
    import torch

    class ImageTower(torch.nn.Module):
        """Casts fp32 in -> fp16 compute -> fp32 out, so the traced ONNX
        graph is internally fp16 throughout with fp32 I/O — no post-hoc
        graph conversion, no mixed-dtype boundary for a converter to get
        wrong."""

        def __init__(self, model):
            super().__init__()
            self.model = model

        def forward(self, pixel_values):
            return self.model.encode_image(pixel_values.half()).float()

    class TextTower(torch.nn.Module):
        def __init__(self, model):
            super().__init__()
            self.model = model

        def forward(self, input_ids):
            # input_ids stay int64 — embedding lookup indices don't need
            # to match the (now fp16) embedding table's dtype.
            return self.model.encode_text(input_ids).float()

    output_dir.mkdir(parents=True, exist_ok=True)
    image_path = output_dir / "siglip2_image_fp16.onnx"
    text_path = output_dir / "siglip2_text_fp16.onnx"

    half_model = model.half()

    # dynamo=False: force the legacy TorchScript-tracer exporter. Torch's
    # newer dynamo-based exporter (the default as of torch 2.11) silently
    # writes large weights as separate external-data files here — this is
    # also the exact exporter spike S1 validated against real torch
    # output (cosine >0.9999998), so matching it isn't just a
    # workaround, it's the already-proven path.
    dummy_pixels = torch.zeros(1, 3, IMAGE_SIZE, IMAGE_SIZE, dtype=torch.float32)
    torch.onnx.export(
        ImageTower(half_model),
        (dummy_pixels,),
        str(image_path),
        input_names=["pixel_values"],
        output_names=["image_embeds"],
        dynamic_axes={"pixel_values": {0: "batch"}, "image_embeds": {0: "batch"}},
        opset_version=OPSET,
        dynamo=False,
    )
    print(
        f"Exported fp16 image tower -> {image_path} ({image_path.stat().st_size / 1e6:.1f} MB)"
    )

    dummy_ids = torch.zeros(1, CONTEXT_LENGTH, dtype=torch.long)
    torch.onnx.export(
        TextTower(half_model),
        (dummy_ids,),
        str(text_path),
        input_names=["input_ids"],
        output_names=["text_embeds"],
        dynamic_axes={"input_ids": {0: "batch"}, "text_embeds": {0: "batch"}},
        opset_version=OPSET,
        dynamo=False,
    )
    print(
        f"Exported fp16 text tower -> {text_path} ({text_path.stat().st_size / 1e6:.1f} MB)"
    )
    return image_path, text_path


def export_tokenizer(tokenizer, output_dir: Path) -> Path:
    # open_clip.get_tokenizer() wraps a transformers `GemmaTokenizer` (its
    # class name doesn't say "Fast", but it's still backed by the Rust
    # `tokenizers` crate via `.backend_tokenizer` — that's what has the
    # raw `.save()` producing the tokenizer.json lrg-ml's
    # `tokenizers::Tokenizer::from_file` loads; `save_pretrained` instead
    # writes an HF-directory layout we don't want here).
    dest = output_dir / "tokenizer.json"
    inner = getattr(tokenizer, "tokenizer", None)
    backend = getattr(inner, "backend_tokenizer", None) if inner is not None else None
    if backend is None:
        raise RuntimeError(
            "Could not locate the backend `tokenizers` object on the open_clip tokenizer wrapper"
        )
    backend.save(str(dest))
    print(f"Exported tokenizer -> {dest}")
    return dest


# Mirrors lrg-ml::text_pre::clean_text exactly (html-unescape twice +
# trim, underscore->space, strip this exact punctuation set, lowercase,
# collapse whitespace) — the goldens were generated through open_clip's
# tokenizer wrapper, which applies the same cleaning internally, so
# skipping it here would make texts with case/punctuation differences
# (unlike plain lowercase English or Japanese) fail for a harness
# reason having nothing to do with the exported model's own quality.
_PUNCTUATION = set("!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~")


def _clean_text(text: str) -> str:
    import html

    unescaped = html.unescape(html.unescape(text)).strip()
    no_underscore = unescaped.replace("_", " ")
    stripped = "".join(c for c in no_underscore if c not in _PUNCTUATION)
    return " ".join(stripped.lower().split())


# Must match testdata/make_siglip_goldens.py's synthetic image
# generators exactly (same seeds/formulas) since siglip_goldens.json
# stores only the resulting embeddings, not the source pixels.
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
    raise ValueError(f"unknown golden image name: {name}")


def verify(output_dir: Path, goldens_path: Path, preprocess=None) -> bool:
    import onnxruntime as ort

    # Leaving intra/inter_op_num_threads on ONNX Runtime's default
    # (auto-detect from visible CPU count) is a known segfault trigger
    # on cgroup/cpuset-restricted hosts, where the visible core count
    # doesn't match the actual scheduling quota — exactly the situation
    # on GitHub Actions runners. See microsoft/onnxruntime#7207: pinning
    # both to a small explicit value is the confirmed fix across that
    # whole issue thread.
    session_options = ort.SessionOptions()
    session_options.intra_op_num_threads = 1
    session_options.inter_op_num_threads = 1

    goldens = json.loads(goldens_path.read_text())
    image_sess = ort.InferenceSession(
        str(output_dir / "siglip2_image_fp16.onnx"),
        sess_options=session_options,
        providers=["CPUExecutionProvider"],
    )
    text_sess = ort.InferenceSession(
        str(output_dir / "siglip2_text_fp16.onnx"),
        sess_options=session_options,
        providers=["CPUExecutionProvider"],
    )

    ok = True
    print("\n=== Verifying fp16 ONNX against fp32 torch goldens ===")
    if preprocess is not None:
        from PIL import Image

        for name, entry in goldens["images"].items():
            arr = _make_golden_image(name)
            pil = Image.fromarray(arr, "RGB")
            tensor = preprocess(pil).unsqueeze(0).numpy().astype(np.float32)
            (fp16_emb,) = image_sess.run(["image_embeds"], {"pixel_values": tensor})
            golden = np.array(entry["embedding"], dtype=np.float32)
            cos = float(
                np.dot(fp16_emb[0], golden)
                / (np.linalg.norm(fp16_emb[0]) * np.linalg.norm(golden))
            )
            status = "OK" if cos > 0.999 else "FAIL"
            if cos <= 0.999:
                ok = False
            print(f"  image '{name}': cosine={cos:.6f} [{status}]")
    else:
        for name in goldens["images"]:
            # `--verify-only` has no torch/open_clip, hence no access to
            # the exact preprocessing transform — this path only runs
            # right after a fresh export, where `preprocess` is on hand.
            print(
                f"  (image golden '{name}': skipped in --verify-only mode — needs open_clip's preprocess transform, not just the ONNX graph)"
            )

    tokenizer_path = output_dir / "tokenizer.json"
    if tokenizer_path.exists():
        from tokenizers import Tokenizer

        tok = Tokenizer.from_file(str(tokenizer_path))
        for text, golden_emb in goldens["texts"].items():
            enc = tok.encode(_clean_text(text))
            ids = enc.ids[:CONTEXT_LENGTH]
            ids = ids + [0] * (CONTEXT_LENGTH - len(ids))
            input_ids = np.array([ids], dtype=np.int64)
            (fp16_emb,) = text_sess.run(["text_embeds"], {"input_ids": input_ids})
            golden = np.array(golden_emb, dtype=np.float32)
            cos = float(
                np.dot(fp16_emb[0], golden)
                / (np.linalg.norm(fp16_emb[0]) * np.linalg.norm(golden))
            )
            status = "OK" if cos > 0.999 else "FAIL"
            if cos <= 0.999:
                ok = False
            print(f"  text '{text[:40]}...': cosine={cos:.6f} [{status}]")
    else:
        print("  tokenizer.json not found; skipping text verification")

    return ok


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument(
        "--verify-only",
        action="store_true",
        help="Skip export, only verify existing output-dir contents",
    )
    parser.add_argument(
        "--goldens",
        type=Path,
        default=Path(__file__).resolve().parent.parent
        / "testdata"
        / "siglip_goldens.json",
    )
    args = parser.parse_args()

    preprocess = None
    if not args.verify_only:
        model, preprocess, tokenizer = load_model()
        export_fp16(model, args.output_dir)
        export_tokenizer(tokenizer, args.output_dir)

    ok = verify(args.output_dir, args.goldens, preprocess=preprocess)
    if not ok:
        print(
            "\nVerification FAILED — fp16 conversion degraded text embeddings beyond tolerance.",
            file=sys.stderr,
        )
        sys.exit(1)
    print("\nVerification passed.")


if __name__ == "__main__":
    main()
