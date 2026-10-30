#!/usr/bin/env python3
# /// script
# requires-python = ">=3.10,<3.13"
# dependencies = [
#     "huggingface_hub>=0.24",
#     "insightface==0.7.3",
#     "numpy<2.0.0",
#     "onnxruntime>=1.17.0",
#     "opencv-python-headless>=4.8",
# ]
# ///
"""Reference embedding for the AuraFace recognizer, made by InsightFace itself.

The backend runs AuraFace (``fal/AuraFace-v1``, ``glintr100.onnx``) through its
own Rust preprocessing (``lrg-ml/src/auraface.rs``: the ArcFace 5-point warp,
RGB, ``(v - 127.5) / 127.5``). This script computes the embedding of the face
in ``testdata/astronaut.jpg`` the way AuraFace is published to be used —
InsightFace's ``norm_crop`` and ``ArcFaceONNX.get_feat`` — from the same five
landmarks the FaceNet golden already stores, and writes it into
``testdata/face_goldens.json`` as ``auraface_embedding``. The golden test
``auraface_matches_insightface_reference`` then checks the Rust chain against
it (cosine > 0.99).

Run::

    uv run --no-project server-rs/scripts/auraface_reference.py --revision main

and pin the printed commit as ``AURAFACE_REVISION`` in
``lrg-api/src/routes/face_recognizer.rs`` once the test passes.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

import cv2
import numpy as np
from huggingface_hub import HfApi, hf_hub_download
from insightface.model_zoo.arcface_onnx import ArcFaceONNX
from insightface.utils import face_align

REPO = "fal/AuraFace-v1"
FILE = "glintr100.onnx"
ROOT = Path(__file__).resolve().parents[1]
GOLDENS = ROOT / "testdata" / "face_goldens.json"
IMAGE = ROOT / "testdata" / "astronaut.jpg"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--revision", default="main")
    parser.add_argument("--output", type=Path, help="also copy the ONNX file here")
    args = parser.parse_args()

    commit = HfApi().model_info(REPO, revision=args.revision).sha
    path = Path(hf_hub_download(REPO, FILE, revision=commit))
    data = path.read_bytes()
    print(f"{REPO}@{commit} {FILE}: {len(data)} bytes, sha256 {hashlib.sha256(data).hexdigest()}")
    if args.output:
        args.output.write_bytes(data)

    model = ArcFaceONNX(model_file=str(path))
    model.prepare(ctx_id=-1)
    print(f"InsightFace picked input_mean={model.input_mean} input_std={model.input_std}")

    goldens = json.loads(GOLDENS.read_text())
    face = goldens["astronaut_512"]["faces"][0]
    kps = np.array(face["kps"], dtype=np.float32)
    img_bgr = cv2.imread(str(IMAGE))
    aligned = face_align.norm_crop(img_bgr, landmark=kps, image_size=112)
    emb = model.get_feat(aligned).flatten()
    emb = emb / np.linalg.norm(emb)

    face["auraface_embedding"] = [float(v) for v in emb]
    goldens["astronaut_512"]["auraface_source"] = {
        "repo": REPO,
        "revision": commit,
        "file": FILE,
        "sha256": hashlib.sha256(data).hexdigest(),
        "input_mean": float(model.input_mean),
        "input_std": float(model.input_std),
    }
    GOLDENS.write_text(json.dumps(goldens))
    print(f"wrote auraface_embedding ({emb.shape[0]}-d) to {GOLDENS}")


if __name__ == "__main__":
    main()
