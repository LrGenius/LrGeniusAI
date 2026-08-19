#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11,<3.13"
# dependencies = [
#     "facenet-pytorch>=2.6.0",
#     "numpy<2.0.0",
#     "opencv-python>=4.9.0",
#     "pillow<10.3.0",
#     "scikit-image>=0.22.0",
#     "torch<2.3.0",
# ]
# ///
"""Golden fixtures for the face pipeline: run the *reference* implementations
— OpenCV's ``FaceDetectorYN`` and ``facenet-pytorch``'s Inception-ResNet-v1 —
over the test photo and dump bbox / landmarks / score / embedding, so the Rust
ports in ``lrg-ml/src/{yunet,facenet}.rs`` can be checked end to end.

Run it after ``scripts/export_face_models.py`` has produced the ONNX files::

    uv run --no-project server-rs/testdata/make_face_goldens.py \\
        --models-dir face-assets

The point is that nothing here shares code with the Rust side: the letterbox,
the landmark alignment and the standardization are all written out again
against OpenCV/skimage/torch. A port that agrees with this agrees with the
upstream algorithms, not merely with itself.

Its predecessor drove InsightFace's ``buffalo_l`` through the Python backend
that used to live in ``server/``; both are gone. See ``lrg_ml::faces::MODEL_ID``
for why swapping those models invalidates stored embeddings.

``--faces-dir`` additionally reports intra- vs inter-person cosine separation
over a directory of ``<person>_<n>.jpg`` files. That is the measurement to re-run
when changing ``FACENET_ZOOM_OUT`` in ``lrg-ml/src/umeyama.rs`` or the clustering
distance threshold — neither can be judged from one photo.
"""

from __future__ import annotations

import argparse
import json
from collections import defaultdict
from pathlib import Path

import cv2
import numpy as np
import torch
from facenet_pytorch import InceptionResnetV1
from PIL import Image
from skimage.transform import SimilarityTransform

HERE = Path(__file__).resolve().parent

# All of these must match the Rust constants they mirror.
INPUT_SIZE = 640  # lrg_ml::yunet::INPUT_SIZE
DET_THRESH = 0.6  # lrg_ml::yunet::DET_THRESH
NMS_THRESH = 0.3  # lrg_ml::yunet::NMS_THRESH
FACENET_INPUT_SIZE = 160  # lrg_ml::facenet::INPUT_SIZE
FACENET_ZOOM_OUT = 1.25  # lrg_ml::umeyama::FACENET_ZOOM_OUT
ARCFACE_DST = np.array(
    [
        [38.2946, 51.6963],
        [73.5318, 51.5014],
        [56.0252, 71.7366],
        [41.5493, 92.3655],
        [70.7299, 92.2041],
    ],
    dtype=np.float64,
)


def facenet_template(size: int) -> np.ndarray:
    """`ARCFACE_DST` mapped into a size x size canvas, scaled about its centre."""
    scale = (size / 112.0) / FACENET_ZOOM_OUT
    return (ARCFACE_DST - 56.0) * scale + size / 2.0


def letterbox(rgb: np.ndarray) -> tuple[np.ndarray, float]:
    """Resize preserving aspect ratio into a zero-padded square canvas.

    Mirrors `lrg_ml::yunet::letterbox`, including its use of cv2's INTER_LINEAR
    (not Pillow's bilinear, which widens its kernel when downscaling).
    """
    height, width = rgb.shape[:2]
    im_ratio = height / width
    if im_ratio > 1.0:
        new_height = INPUT_SIZE
        new_width = int(new_height / im_ratio)
    else:
        new_width = INPUT_SIZE
        new_height = int(new_width * im_ratio)
    det_scale = new_height / height

    resized = cv2.resize(rgb, (new_width, new_height), interpolation=cv2.INTER_LINEAR)
    canvas = np.zeros((INPUT_SIZE, INPUT_SIZE, 3), dtype=np.uint8)
    canvas[:new_height, :new_width] = resized
    return canvas, det_scale


def detect(detector: cv2.FaceDetectorYN, rgb: np.ndarray) -> list[dict]:
    canvas, det_scale = letterbox(rgb)
    # FaceDetectorYN wraps blobFromImage with swapRB=False, so it wants BGR —
    # the same channel reversal `lrg_ml::yunet::to_blob` does by hand.
    _, faces = detector.detect(canvas[:, :, ::-1].copy())
    if faces is None:
        return []

    out = []
    for row in faces:
        x, y, w, h = row[0:4].astype(np.float64) / det_scale
        kps = row[4:14].astype(np.float64).reshape(5, 2) / det_scale
        out.append(
            {
                "bbox": [x, y, x + w, y + h],
                "kps": kps.tolist(),
                "det_score": float(row[14]),
            }
        )
    return out


def embed(model: InceptionResnetV1, rgb: np.ndarray, kps: np.ndarray) -> list[float]:
    dst = facenet_template(FACENET_INPUT_SIZE)
    # `estimate` is deprecated from skimage 0.26 in favour of the classmethod,
    # which older versions don't have; the pin only sets a floor.
    if hasattr(SimilarityTransform, "from_estimate"):
        transform = SimilarityTransform.from_estimate(kps, dst)
    else:
        transform = SimilarityTransform()
        transform.estimate(kps, dst)
    aligned = cv2.warpAffine(
        rgb,
        transform.params[0:2, :],
        (FACENET_INPUT_SIZE, FACENET_INPUT_SIZE),
        borderValue=0.0,
    )
    blob = (aligned.astype(np.float32) - 127.5) / 128.0
    tensor = torch.from_numpy(blob).permute(2, 0, 1).unsqueeze(0)
    with torch.no_grad():
        return model(tensor).numpy().ravel().astype(float).tolist()


def report_separation(
    detector: cv2.FaceDetectorYN, model: InceptionResnetV1, faces_dir: Path
) -> None:
    """Print intra- vs inter-person cosine similarity over `<person>_<n>.jpg`."""
    by_person: dict[str, list[np.ndarray]] = defaultdict(list)
    for path in sorted(faces_dir.iterdir()):
        if path.suffix.lower() not in {".jpg", ".jpeg", ".png"}:
            continue
        rgb = np.array(Image.open(path).convert("RGB"))
        found = detect(detector, rgb)
        if not found:
            print(f"  {path.name}: no face detected, skipped")
            continue
        best = max(found, key=lambda f: f["det_score"])
        vector = np.array(embed(model, rgb, np.array(best["kps"])))
        by_person[path.stem.rsplit("_", 1)[0]].append(vector / np.linalg.norm(vector))

    intra, inter = [], []
    people = sorted(by_person)
    for i, person in enumerate(people):
        vectors = by_person[person]
        for a in range(len(vectors)):
            for b in range(a + 1, len(vectors)):
                intra.append(float(vectors[a] @ vectors[b]))
            for other in people[i + 1 :]:
                inter.extend(float(vectors[a] @ v) for v in by_person[other])

    print(f"\nSeparation over {len(people)} people, {sum(map(len, by_person.values()))} faces:")
    for label, values in (("same person", intra), ("different people", inter)):
        if values:
            arr = np.array(values)
            print(
                f"  {label:17s} cosine  mean {arr.mean():.3f}  "
                f"min {arr.min():.3f}  max {arr.max():.3f}  (n={len(arr)})"
            )
    if intra and inter:
        # The clustering default lives in `routes::faces::cluster` as a cosine
        # *distance*, i.e. 1 - similarity.
        print(
            f"  suggested distance threshold: "
            f"{1.0 - (np.array(intra).min() + np.array(inter).max()) / 2.0:.2f}"
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--models-dir", type=Path, required=True)
    parser.add_argument("--faces-dir", type=Path, default=None)
    parser.add_argument("--output", type=Path, default=HERE / "face_goldens.json")
    args = parser.parse_args()

    detector = cv2.FaceDetectorYN.create(
        str(args.models_dir / "yunet_face_detection.onnx"),
        "",
        (INPUT_SIZE, INPUT_SIZE),
        DET_THRESH,
        NMS_THRESH,
        5000,
    )
    model = InceptionResnetV1(pretrained="vggface2").eval()

    cases = {}
    image = Image.open(HERE / "astronaut.jpg").convert("RGB")
    rgb = np.array(image)
    faces = detect(detector, rgb)
    print(f"astronaut_512: {len(faces)} face(s)")
    for face in faces:
        face["embedding"] = embed(model, rgb, np.array(face["kps"]))
        print(f"  bbox {[round(v, 1) for v in face['bbox']]} score {face['det_score']:.4f}")
        # Point 0 is the subject's right eye and must land image-left of point
        # 1, which is what lets these landmarks feed the ArcFace-ordered
        # template unchanged. See `lrg_ml::yunet`'s module docs.
        assert face["kps"][0][0] < face["kps"][1][0], "landmark order is mirrored"
    cases["astronaut_512"] = {
        "width": image.width,
        "height": image.height,
        "faces": faces,
    }

    args.output.write_text(json.dumps(cases))
    print(f"wrote {args.output}")

    if args.faces_dir:
        report_separation(detector, model, args.faces_dir)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
