#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11,<3.13"
# dependencies = [
#     "rfdetr[onnx]==1.11.2",
#     "onnx>=1.16.0",
#     "onnxruntime>=1.20.0",
#     "onnxconverter-common>=1.14.0",
#     "numpy",
#     "pillow",
# ]
# ///
"""Produce the ONNX file the backend finds a photo's subject with.

  * ``subject_rfdetr_seg_nano.onnx`` — RF-DETR-Seg Nano (Roboflow): up to 100
    objects of the 80 COCO classes, each with a box, a score per class and a
    78x78 mask, from a 312x312 RGB image.

Culling needs to know *where the subject is* to tell a soft subject on a sharp
background (focus missed) from a sharp subject on a soft one (shallow depth of
field, a pan). Faces answer that for portraits; this answers it for an athlete,
an animal, a car.

**Licence.** The rfdetr README: "RF-DETR-Seg, every size (Nano through
2XLarge)" is Apache 2.0, and "`pip install rfdetr` gives you only Apache 2.0
RF-DETR code and models, so it's commercially usable out of the box." It is
trained on Microsoft COCO, whose annotations are CC BY 4.0.

**Standalone, like ``export_face_state.py``.** rfdetr brings its own torch and
transformers pins, which do not belong in the shared export project::

    uv run --no-project server-rs/scripts/export_subject_rfdetr.py --output-dir subject-assets

The checkpoint is pinned by SHA-256 on top of rfdetr's own MD5 check. The
export is compared with the PyTorch model on the reference portrait before it
is written; a float16 copy is kept only when it agrees with float32 as closely
as float32 agrees with PyTorch.

``--goldens PATH`` writes what the Rust port (``lrg_ml::subject``) is checked
against in ``lrg-ml/tests/subject_goldens.rs``: the detections of the exported
model on ``server-rs/testdata/astronaut.jpg``.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import sys
import tempfile
from pathlib import Path

import numpy as np
import onnx
import onnxruntime as ort
from PIL import Image

CHECKPOINT_SHA256 = "a44613a4ecd6b5ba61a62002c600b0b6cb7a9da2936a45317ec4b62c635fb99b"
ONNX_NAME = "subject_rfdetr_seg_nano.onnx"

# Must match `lrg_ml::subject`.
INPUT_SIZE = 312
MASK_SIZE = 78
QUERIES = 100
CLASS_SLOTS = 91
MEAN = np.array([0.485, 0.456, 0.406], dtype=np.float32)
STD = np.array([0.229, 0.224, 0.225], dtype=np.float32)

SERVER_RS = Path(__file__).resolve().parents[1]
REFERENCE = SERVER_RS / "testdata" / "astronaut.jpg"


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def bilinear_half_pixel(rgb: np.ndarray, size: int) -> np.ndarray:
    """What RF-DETR's own predict() does, and what `lrg_ml::subject` ports:
    bilinear on the float image, half-pixel centres, no antialiasing, to a
    square (the aspect ratio is not kept)."""
    h, w = rgb.shape[:2]
    out = np.empty((size, size, rgb.shape[2]), dtype=np.float32)
    ys = np.clip((np.arange(size) + 0.5) * (h / size) - 0.5, 0, h - 1)
    xs = np.clip((np.arange(size) + 0.5) * (w / size) - 0.5, 0, w - 1)
    y0 = np.floor(ys).astype(int)
    x0 = np.floor(xs).astype(int)
    y1 = np.minimum(y0 + 1, h - 1)
    x1 = np.minimum(x0 + 1, w - 1)
    fy = (ys - y0)[:, None, None]
    fx = (xs - x0)[None, :, None]
    top = rgb[y0][:, x0] * (1 - fx) + rgb[y0][:, x1] * fx
    bottom = rgb[y1][:, x0] * (1 - fx) + rgb[y1][:, x1] * fx
    out[:] = top * (1 - fy) + bottom * fy
    return out


def preprocess(path: Path) -> np.ndarray:
    rgb = np.asarray(Image.open(path).convert("RGB"), dtype=np.float32) / 255.0
    resized = bilinear_half_pixel(rgb, INPUT_SIZE)
    return ((resized - MEAN) / STD).transpose(2, 0, 1)[None].astype(np.float32)


def run(path: Path, onnx_path: Path) -> dict[str, np.ndarray]:
    session = ort.InferenceSession(str(onnx_path), providers=["CPUExecutionProvider"])
    names = [o.name for o in session.get_outputs()]
    outputs = session.run(None, {session.get_inputs()[0].name: preprocess(path)})
    return dict(zip(names, outputs))


def detections(outputs: dict[str, np.ndarray], threshold: float = 0.3) -> list[dict]:
    """One detection per query: its best class, as `lrg_ml::subject` decodes."""
    logits = outputs["labels"][0]
    scores = 1.0 / (1.0 + np.exp(-logits))
    out = []
    for q in range(logits.shape[0]):
        cls = int(np.argmax(scores[q]))
        score = float(scores[q, cls])
        if score < threshold:
            continue
        cx, cy, w, h = (float(v) for v in outputs["dets"][0, q])
        mask = outputs["masks"][0, q] > 0.0
        out.append(
            {
                "query": q,
                "class_id": cls,
                "score": score,
                "box": [cx - w / 2, cy - h / 2, cx + w / 2, cy + h / 2],
                "mask_area": float(mask.mean()),
                "mask_rows": [int(i) for i in np.nonzero(mask.any(axis=1))[0][[0, -1]]]
                if mask.any()
                else None,
            }
        )
    return sorted(out, key=lambda d: -d["score"])


def export(output_dir: Path) -> Path:
    from rfdetr import RFDETRSegNano

    model = RFDETRSegNano()
    checkpoint = Path.home() / ".roboflow" / "models" / "rf-detr-seg-nano.pt"
    if not checkpoint.exists() or sha256(checkpoint) != CHECKPOINT_SHA256:
        sys.exit(
            f"{checkpoint} does not match the pinned SHA-256; refusing to export "
            "weights nobody has looked at"
        )
    with tempfile.TemporaryDirectory() as tmp:
        exported = Path(model.export(output_dir=tmp, verbose=False, opset_version=17))
        fp32 = output_dir / "fp32.onnx"
        shutil.copy(exported, fp32)

    # The PyTorch model on the same input: the export must say the same.
    import torch

    torch_model = model.model.model.to("cpu").eval()
    with torch.inference_mode():
        ref = torch_model(torch.from_numpy(preprocess(REFERENCE)))
    ref_logits = ref["pred_logits"][0].numpy()
    got = run(REFERENCE, fp32)
    err32 = float(np.abs(got["labels"][0] - ref_logits).max())
    print(f"fp32 export vs PyTorch: max logit difference {err32:.2e}")
    if err32 > 1e-2:
        sys.exit("the fp32 export disagrees with PyTorch")

    from onnxconverter_common import float16

    fp16_model = float16.convert_float_to_float16(
        onnx.load(str(fp32)), keep_io_types=True
    )
    fp16 = output_dir / "fp16.onnx"
    onnx.save(fp16_model, str(fp16))
    try:
        got16 = run(REFERENCE, fp16)
        err16 = float(np.abs(got16["labels"][0] - got["labels"][0]).max())
        mask16 = float(np.mean((got16["masks"][0] > 0) != (got["masks"][0] > 0)))
        print(
            f"fp16 vs fp32: max logit difference {err16:.2e}, "
            f"mask pixels flipped {mask16:.2%}"
        )
    except Exception as e:  # noqa: BLE001 - an fp16 graph ORT cannot load is just not used
        # Measured with onnxconverter-common 1.14: the converted graph leaves
        # the first patch-embedding Conv with a float input and float16
        # weights, which ONNX Runtime refuses to load.
        print(f"fp16 conversion unusable ({str(e).splitlines()[0][:160]})")
        err16, mask16 = float("inf"), float("inf")

    target = output_dir / ONNX_NAME
    if err16 < 0.05 and mask16 < 0.002:
        fp16.replace(target)
        fp32.unlink()
        print(f"kept float16: {target}")
    else:
        fp32.replace(target)
        fp16.unlink()
        print(f"kept float32 (no usable float16 copy): {target}")
    return target


def main() -> None:
    parser = argparse.ArgumentParser(description=(__doc__ or "").split("\n\n")[0])
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--goldens", type=Path)
    args = parser.parse_args()
    args.output_dir.mkdir(parents=True, exist_ok=True)

    target = export(args.output_dir)
    session = ort.InferenceSession(str(target), providers=["CPUExecutionProvider"])
    shapes = {o.name: o.shape for o in session.get_outputs()}
    expected = {
        "dets": [1, QUERIES, 4],
        "labels": [1, QUERIES, CLASS_SLOTS],
        "masks": [1, QUERIES, MASK_SIZE, MASK_SIZE],
    }
    if shapes != expected or session.get_inputs()[0].shape != [1, 3, INPUT_SIZE, INPUT_SIZE]:
        sys.exit(f"unexpected model signature: {shapes}")
    print(f"{target.name}: {target.stat().st_size / 1e6:.1f} MB, sha256 {sha256(target)}")

    if args.goldens:
        outputs = run(REFERENCE, target)
        golden = {
            "image": "astronaut.jpg",
            "model": ONNX_NAME,
            "input_size": INPUT_SIZE,
            # The preprocessed tensor at a few fixed positions, so a port that
            # resizes or normalises differently fails before decoding.
            "input_samples": [
                [c, y, x, float(preprocess(REFERENCE)[0, c, y, x])]
                for c, y, x in [(0, 0, 0), (1, 100, 150), (2, 311, 311), (0, 200, 40)]
            ],
            "detections": detections(outputs),
        }
        args.goldens.write_text(json.dumps(golden, indent=1) + "\n")
        print(f"wrote {args.goldens}")


if __name__ == "__main__":
    main()
