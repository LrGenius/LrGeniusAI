#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11,<3.13"
# dependencies = [
#     "facenet-pytorch>=2.6.0",
#     "numpy<2.0.0",
#     "onnx>=1.16.0",
#     "onnxruntime>=1.17.0",
#     "pillow<10.3.0",
#     "torch<2.3.0",
# ]
# ///
"""Produce the two ONNX files the backend's face pipeline needs.

  * ``yunet_face_detection.onnx``  — OpenCV Zoo's YuNet detector, downloaded
    as-is (it is already ONNX, there is nothing to export) and pinned by commit
    and SHA-256.
  * ``facenet_vggface2.onnx``      — ``facenet-pytorch``'s Inception-ResNet-v1
    with the VGGFace2 weights, exported at a fixed 1x3x160x160 input.

Both replace InsightFace's ``buffalo_l`` pack (SCRFD + ArcFace), which is
licensed for non-commercial research only and so could never be published with
this project. YuNet is Apache-2.0 and facenet-pytorch is MIT, which is what
lets ``/v1/models/assets/downloads`` fetch these like every other model family.

**This script deliberately does not join ``scripts/pyproject.toml``.**
``facenet-pytorch`` pins ``torch<2.3`` and ``numpy<2.0``; the shared export
project is on ``torch>=2.11`` and ``numpy>=2.4`` for SigLIP2 and BioCLIP.
Rather than hold those back to the older pins for one small model, the
dependencies live in the PEP 723 header above and it runs standalone::

    uv run --no-project server-rs/scripts/export_face_models.py --output-dir face-assets

Run with ``--verify-only`` against an existing directory to re-check assets
without re-downloading or re-exporting them.
"""

from __future__ import annotations

import argparse
import hashlib
import sys
import urllib.request
from pathlib import Path

import numpy as np
import onnxruntime as ort
import torch
from facenet_pytorch import InceptionResnetV1

# Pinned to the commit that last touched the file rather than `main`, so a
# later re-run cannot silently produce a different detector than the one the
# Rust port in `lrg-ml/src/yunet.rs` was verified against. The 2023mar model
# is the one whose 12-output cls/obj/bbox/kps layout that port decodes.
YUNET_COMMIT = "f12e12798e8314f7c074a6656816c048dcc95b7a"
# `github.com/.../raw/...`, not `raw.githubusercontent.com`: opencv_zoo keeps
# its models in Git LFS, and only the former resolves the pointer to the real
# 232 KB binary. The latter hands back a 131-byte text pointer that ONNX
# Runtime rejects with an unhelpful parse error.
YUNET_URL = (
    f"https://github.com/opencv/opencv_zoo/raw/{YUNET_COMMIT}"
    "/models/face_detection_yunet/face_detection_yunet_2023mar.onnx"
)
YUNET_SHA256 = "8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4"

YUNET_NAME = "yunet_face_detection.onnx"
FACENET_NAME = "facenet_vggface2.onnx"

# Must match `lrg_ml::facenet` and `lrg_ml::yunet`.
FACENET_INPUT_SIZE = 160
FACENET_EMBED_DIM = 512
YUNET_INPUT_SIZE = 640
YUNET_OUTPUT_NAMES = [
    "cls_8", "cls_16", "cls_32",
    "obj_8", "obj_16", "obj_32",
    "bbox_8", "bbox_16", "bbox_32",
    "kps_8", "kps_16", "kps_32",
]  # fmt: skip


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def fetch_yunet(dest: Path) -> None:
    print(f"Downloading YuNet from opencv_zoo@{YUNET_COMMIT[:12]} ...")
    with urllib.request.urlopen(YUNET_URL) as response:
        dest.write_bytes(response.read())
    got = sha256(dest)
    if got != YUNET_SHA256:
        dest.unlink(missing_ok=True)
        raise SystemExit(
            f"YuNet checksum mismatch: expected {YUNET_SHA256}, got {got}. "
            "The pinned commit should make this impossible; do not publish."
        )
    print(f"  {dest.name}: {dest.stat().st_size / 1e6:.1f} MB, sha256 ok")


def export_facenet(dest: Path) -> None:
    print("Exporting facenet-pytorch InceptionResnetV1 (vggface2) ...")
    model = InceptionResnetV1(pretrained="vggface2").eval()
    dummy = torch.randn(1, 3, FACENET_INPUT_SIZE, FACENET_INPUT_SIZE)
    # Fixed batch of 1, no dynamic axes: the backend embeds one face per
    # `Session::run`, and a static shape lets onnxruntime plan the graph once.
    torch.onnx.export(
        model,
        dummy,
        dest,
        input_names=["input"],
        output_names=["embedding"],
        opset_version=17,
        do_constant_folding=True,
    )
    print(f"  {dest.name}: {dest.stat().st_size / 1e6:.1f} MB")


def verify_yunet(path: Path) -> None:
    session = ort.InferenceSession(str(path), providers=["CPUExecutionProvider"])
    names = [o.name for o in session.get_outputs()]
    missing = [n for n in YUNET_OUTPUT_NAMES if n not in names]
    if missing:
        raise SystemExit(
            f"YuNet is missing output(s) {missing}; lrg_ml::yunet fetches all of "
            f"{YUNET_OUTPUT_NAMES} by name. Got: {names}"
        )

    blob = np.zeros((1, 3, YUNET_INPUT_SIZE, YUNET_INPUT_SIZE), dtype=np.float32)
    outputs = session.run(YUNET_OUTPUT_NAMES, {session.get_inputs()[0].name: blob})
    by_name = dict(zip(YUNET_OUTPUT_NAMES, outputs))
    # The Rust decoder indexes each head as `r * grid + c` with
    # `grid = 640 / stride`, and reads 1/1/4/10 values per cell. A shape
    # mismatch there decodes into plausible boxes in the wrong places, so it is
    # checked here rather than discovered as bad detections.
    for stride in (8, 16, 32):
        cells = (YUNET_INPUT_SIZE // stride) ** 2
        for head, width in (("cls", 1), ("obj", 1), ("bbox", 4), ("kps", 10)):
            got = by_name[f"{head}_{stride}"]
            if got.reshape(-1).size != cells * width:
                raise SystemExit(
                    f"{head}_{stride}: expected {cells}x{width} values for a "
                    f"{YUNET_INPUT_SIZE}px input, got shape {got.shape}"
                )
    print(f"  verified: 12 outputs, cell grids match a {YUNET_INPUT_SIZE}px input")


def verify_facenet(path: Path) -> None:
    session = ort.InferenceSession(str(path), providers=["CPUExecutionProvider"])
    torch_model = InceptionResnetV1(pretrained="vggface2").eval()

    rng = np.random.default_rng(0)
    # Standardized the same way `lrg_ml::facenet::to_blob` does, so this checks
    # the export over the input range the backend actually feeds it.
    pixels = rng.integers(0, 256, (1, 3, FACENET_INPUT_SIZE, FACENET_INPUT_SIZE))
    blob = ((pixels.astype(np.float32) - 127.5) / 128.0).astype(np.float32)

    onnx_out = session.run(None, {session.get_inputs()[0].name: blob})[0]
    with torch.no_grad():
        torch_out = torch_model(torch.from_numpy(blob)).numpy()

    if onnx_out.shape != (1, FACENET_EMBED_DIM):
        raise SystemExit(
            f"expected a (1, {FACENET_EMBED_DIM}) embedding, got {onnx_out.shape}"
        )

    norm = float(np.linalg.norm(onnx_out))
    # The backend L2-normalizes anyway, but an export that dropped the model's
    # own final `F.normalize` would change what every stored cosine distance
    # means, so it is asserted rather than papered over.
    if abs(norm - 1.0) > 1e-3:
        raise SystemExit(f"embedding is not L2-normalized (norm {norm:.6f})")

    cosine = float(
        np.dot(onnx_out.ravel(), torch_out.ravel())
        / (np.linalg.norm(onnx_out) * np.linalg.norm(torch_out))
    )
    if cosine < 0.9999:
        raise SystemExit(f"ONNX/torch embedding cosine {cosine:.6f} is too low")
    print(f"  verified: 512-d, L2-normalized, torch cosine {cosine:.6f}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, default=Path("face-assets"))
    parser.add_argument(
        "--verify-only",
        action="store_true",
        help="check assets already in --output-dir instead of producing them",
    )
    args = parser.parse_args()

    out = args.output_dir
    out.mkdir(parents=True, exist_ok=True)
    yunet, facenet = out / YUNET_NAME, out / FACENET_NAME

    if not args.verify_only:
        fetch_yunet(yunet)
        export_facenet(facenet)

    for path in (yunet, facenet):
        if not path.is_file():
            raise SystemExit(f"missing {path}")

    print("Verifying YuNet ...")
    verify_yunet(yunet)
    print("Verifying FaceNet ...")
    verify_facenet(facenet)

    print(f"\nFace model assets ready in {out}/:")
    for path in (yunet, facenet):
        print(f"  {path.name}  {path.stat().st_size / 1e6:7.1f} MB  {sha256(path)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
