#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11,<3.13"
# dependencies = [
#     "tensorflow-cpu>=2.17; sys_platform == 'linux'",
#     "tensorflow>=2.17; sys_platform != 'linux'",
#     "tf2onnx>=1.16.1",
#     "onnx>=1.16.0",
#     "onnxruntime>=1.17.0",
#     "numpy",
#     "pillow",
#     "mediapipe>=0.10.14",
# ]
# ///
"""Produce the two ONNX files the backend's face-state analysis needs.

  * ``face_mesh_v2.onnx``        — MediaPipe Face Mesh V2: 478 face landmarks
    (iris included) and a face-presence logit from a 256x256 face crop.
  * ``face_blendshapes_v2.onnx`` — MediaPipe Blendshape V2: 52 ARKit-style
    expression coefficients (``eyeBlinkLeft``, ``mouthSmileRight``, ...) from
    146 of those landmarks.

Both come out of Google's ``face_landmarker.task`` bundle (a zip of TFLite
models), pinned by URL and SHA-256, and are converted with tf2onnx. Both model
cards state **Apache License, Version 2.0**; their out-of-scope uses are
life-critical decisions and identifying people, neither of which culling does.
The cards also name the limits the backend has to respect: faces turned more
than 80 degrees away, less than half visible, or too small to rescale to the
model input without loss. ``lrg_ml::face_state`` leaves those faces unjudged.

**Standalone, like ``export_face_models.py``.** TensorFlow (needed by tf2onnx
and for the TFLite reference) does not belong in the shared export project, so
the dependencies live in the PEP 723 header above::

    uv run --no-project server-rs/scripts/export_face_state.py --output-dir face-state-assets

Every export is checked against the TFLite originals before it is written, and
fails rather than leave a file behind that disagrees with them.

``--goldens PATH`` additionally runs Python MediaPipe's own FaceLandmarker on
``server-rs/testdata/astronaut.jpg`` and writes what the Rust port is checked
against (``lrg-ml/tests/face_state_goldens.rs``).
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import sys
import tempfile
import urllib.request
import zipfile
from pathlib import Path

import numpy as np
import onnx
import onnxruntime as ort

# Version 1 of the float16 bundle; the path is versioned, the hash is what
# guarantees nobody swapped the bytes behind it.
TASK_URL = (
    "https://storage.googleapis.com/mediapipe-models/face_landmarker/"
    "face_landmarker/float16/1/face_landmarker.task"
)
TASK_SHA256 = "64184e229b263107bc2b804c6625db1341ff2bb731874b0bcc2fe6544e0bc9ff"

MESH_NAME = "face_mesh_v2.onnx"
BLENDSHAPES_NAME = "face_blendshapes_v2.onnx"

# Must match `lrg_ml::face_state`.
MESH_INPUT_SIZE = 256
MESH_LANDMARKS = 478
BLENDSHAPE_POINTS = 146
BLENDSHAPE_COUNT = 52

# What the Rust side fetches by name. tf2onnx keeps TFLite's own names
# ("input_12", "Identity", ...), which say nothing and would change if Google
# re-exported the model; these are renamed here once.
MESH_IO = {"input": "image", "outputs": ["landmarks", "presence", "tongue"]}
BLENDSHAPE_IO = {"input": "points", "outputs": ["blendshapes"]}

# From mediapipe/tasks/cc/vision/face_landmarker/face_blendshapes_graph.cc
# (kLandmarksSubsetIdxs); `lrg_ml::face_state::BLENDSHAPE_LANDMARKS` mirrors it
# and the golden test pins the two together.
BLENDSHAPE_LANDMARKS = [
    0, 1, 4, 5, 6, 7, 8, 10, 13, 14, 17, 21, 33, 37, 39,
    40, 46, 52, 53, 54, 55, 58, 61, 63, 65, 66, 67, 70, 78, 80,
    81, 82, 84, 87, 88, 91, 93, 95, 103, 105, 107, 109, 127, 132, 133,
    136, 144, 145, 146, 148, 149, 150, 152, 153, 154, 155, 157, 158, 159, 160,
    161, 162, 163, 168, 172, 173, 176, 178, 181, 185, 191, 195, 197, 234, 246,
    249, 251, 263, 267, 269, 270, 276, 282, 283, 284, 285, 288, 291, 293, 295,
    296, 297, 300, 308, 310, 311, 312, 314, 317, 318, 321, 323, 324, 332, 334,
    336, 338, 356, 361, 362, 365, 373, 374, 375, 377, 378, 379, 380, 381, 382,
    384, 385, 386, 387, 388, 389, 390, 397, 398, 400, 402, 405, 409, 415, 454,
    466, 468, 469, 470, 471, 472, 473, 474, 475, 476, 477,
]  # fmt: skip

# Upper eyelid -> lower eyelid, both eyes (from the iris-averaging contours in
# mediapipe/tasks/cc/vision/face_landmarker/tensors_to_face_landmarks_graph.cc).
# Moving each upper point onto its lower partner closes the eye geometrically;
# the goldens use it to pin the "eyes closed" path through the blendshape
# model, since no closed-eye photograph ships with this repository.
EYELID_PAIRS = [
    (246, 7), (161, 163), (160, 144), (159, 145), (158, 153), (157, 154), (173, 155),
    (466, 249), (388, 390), (387, 373), (386, 374), (385, 380), (384, 381), (398, 382),
]  # fmt: skip

ASTRONAUT = Path(__file__).resolve().parent.parent / "testdata" / "astronaut.jpg"


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def fetch_task(workdir: Path) -> Path:
    print("Downloading face_landmarker.task ...")
    with urllib.request.urlopen(TASK_URL) as response:
        data = response.read()
    got = sha256(data)
    if got != TASK_SHA256:
        raise SystemExit(
            f"face_landmarker.task checksum mismatch: expected {TASK_SHA256}, got {got}. "
            "Do not publish."
        )
    print(f"  {len(data) / 1e6:.1f} MB, sha256 ok")
    # Kept whole as well: the goldens' FaceLandmarker loads the bundle itself.
    (workdir / "bundle.task").write_bytes(data)
    with zipfile.ZipFile(io.BytesIO(data)) as bundle:
        bundle.extractall(workdir)
    for name in ("face_landmarks_detector.tflite", "face_blendshapes.tflite"):
        if not (workdir / name).is_file():
            raise SystemExit(f"{name} is missing from face_landmarker.task")
    return workdir


def rename_io(model: onnx.ModelProto, input_name: str, output_names: list[str]) -> None:
    """Gives the graph's single input and its outputs stable names."""
    graph = model.graph
    if len(graph.input) != 1 or len(graph.output) != len(output_names):
        raise SystemExit(
            f"unexpected graph signature: {len(graph.input)} input(s), "
            f"{len(graph.output)} output(s), wanted 1 and {len(output_names)}"
        )
    renames = {graph.input[0].name: input_name}
    renames.update({o.name: n for o, n in zip(graph.output, output_names)})
    for value in list(graph.input) + list(graph.output):
        value.name = renames.get(value.name, value.name)
    for node in graph.node:
        for i, name in enumerate(node.input):
            node.input[i] = renames.get(name, name)
        for i, name in enumerate(node.output):
            node.output[i] = renames.get(name, name)


def convert(tflite: Path, dest: Path, io_names: dict) -> None:
    import tf2onnx

    print(f"Converting {tflite.name} -> {dest.name} ...")
    model, _ = tf2onnx.convert.from_tflite(str(tflite), opset=17)
    rename_io(model, io_names["input"], io_names["outputs"])
    onnx.checker.check_model(model)
    onnx.save(model, dest)
    print(f"  {dest.name}: {dest.stat().st_size / 1e6:.1f} MB")


def tflite_run(path: Path, inputs: np.ndarray) -> list[np.ndarray]:
    import tensorflow as tf

    interpreter = tf.lite.Interpreter(model_path=str(path))
    interpreter.allocate_tensors()
    interpreter.set_tensor(interpreter.get_input_details()[0]["index"], inputs)
    interpreter.invoke()
    return [interpreter.get_tensor(d["index"]) for d in interpreter.get_output_details()]


def onnx_run(path: Path, inputs: np.ndarray) -> list[np.ndarray]:
    session = ort.InferenceSession(str(path), providers=["CPUExecutionProvider"])
    return session.run(None, {session.get_inputs()[0].name: inputs})


def check_parity(label: str, reference: list[np.ndarray], got: list[np.ndarray], tol: float):
    if len(reference) != len(got):
        raise SystemExit(f"{label}: {len(got)} outputs, TFLite has {len(reference)}")
    for i, (a, b) in enumerate(zip(reference, got)):
        a = np.asarray(a, dtype=np.float64).reshape(-1)
        b = np.asarray(b, dtype=np.float64).reshape(-1)
        if a.shape != b.shape:
            raise SystemExit(f"{label} output {i}: shape {b.shape}, TFLite {a.shape}")
        diff = float(np.max(np.abs(a - b))) if a.size else 0.0
        scale = max(1.0, float(np.max(np.abs(a))))
        print(f"  {label} output {i}: max |diff| {diff:.2e} (scale {scale:.1f})")
        if diff > tol * scale:
            raise SystemExit(f"{label} output {i} disagrees with TFLite by {diff}; do not publish")


def verify(workdir: Path, out: Path) -> None:
    """The ONNX files against the TFLite models they came from."""
    rng = np.random.default_rng(1234)
    mesh_tflite = workdir / "face_landmarks_detector.tflite"
    shapes_tflite = workdir / "face_blendshapes.tflite"

    # A real face crop as well as noise: noise exercises every weight, a face
    # exercises the ranges the model actually sees.
    from PIL import Image

    face = Image.open(ASTRONAUT).convert("RGB").crop((140, 50, 310, 220))
    face = face.resize((MESH_INPUT_SIZE, MESH_INPUT_SIZE), Image.BILINEAR)
    crops = [
        np.asarray(face, dtype=np.float32)[None] / 255.0,
        rng.random((1, MESH_INPUT_SIZE, MESH_INPUT_SIZE, 3), dtype=np.float32),
    ]
    for i, crop in enumerate(crops):
        check_parity(
            f"mesh[{i}]", tflite_run(mesh_tflite, crop), onnx_run(out / MESH_NAME, crop), 1e-3
        )

    points = [
        rng.random((1, BLENDSHAPE_POINTS, 2), dtype=np.float32) * 400 + 100,
        rng.random((1, BLENDSHAPE_POINTS, 2), dtype=np.float32) * 2000,
    ]
    for i, p in enumerate(points):
        check_parity(
            f"blendshapes[{i}]",
            tflite_run(shapes_tflite, p),
            onnx_run(out / BLENDSHAPES_NAME, p),
            1e-3,
        )

    session = ort.InferenceSession(str(out / MESH_NAME), providers=["CPUExecutionProvider"])
    names = [o.name for o in session.get_outputs()]
    if names != MESH_IO["outputs"] or session.get_inputs()[0].name != MESH_IO["input"]:
        raise SystemExit(f"mesh signature changed: {session.get_inputs()[0].name} -> {names}")


def write_goldens(workdir: Path, out: Path, dest: Path) -> None:
    """MediaPipe's own answer on the reference portrait, plus a closed-eye
    variant of its landmarks run through the exported blendshape model."""
    import mediapipe as mp
    from mediapipe.tasks.python import BaseOptions, vision

    options = vision.FaceLandmarkerOptions(
        base_options=BaseOptions(model_asset_path=str(workdir / "bundle.task")),
        output_face_blendshapes=True,
        num_faces=1,
    )
    image = mp.Image.create_from_file(str(ASTRONAUT))
    with vision.FaceLandmarker.create_from_options(options) as landmarker:
        result = landmarker.detect(image)
    if len(result.face_landmarks) != 1:
        raise SystemExit(f"expected one face on astronaut.jpg, got {len(result.face_landmarks)}")
    width, height = image.width, image.height
    landmarks = [[p.x * width, p.y * height, p.z * width] for p in result.face_landmarks[0]]
    blendshapes = {c.category_name: float(c.score) for c in result.face_blendshapes[0]}

    # The same landmarks with both eyes shut, through the exported model.
    closed = np.array([lm[:2] for lm in landmarks], dtype=np.float32)
    for upper, lower in EYELID_PAIRS:
        closed[upper] = closed[lower]
    subset = closed[BLENDSHAPE_LANDMARKS][None]
    session = ort.InferenceSession(str(out / BLENDSHAPES_NAME), providers=["CPUExecutionProvider"])
    closed_scores = session.run(None, {"points": subset})[0].reshape(-1)
    names = list(blendshapes.keys())
    closed_shapes = {n: float(v) for n, v in zip(names, closed_scores)}

    goldens = {
        "image": "astronaut.jpg",
        "width": width,
        "height": height,
        "landmarks_px": landmarks,
        "blendshapes": blendshapes,
        "eyelid_pairs": EYELID_PAIRS,
        "blendshape_landmarks": BLENDSHAPE_LANDMARKS,
        "eyes_closed_blendshapes": closed_shapes,
        "mediapipe_version": mp.__version__,
    }
    dest.write_text(json.dumps(goldens, indent=1) + "\n")
    print(
        f"Wrote {dest}: open eyes blink {blendshapes['eyeBlinkLeft']:.2f}/"
        f"{blendshapes['eyeBlinkRight']:.2f}, closed {closed_shapes['eyeBlinkLeft']:.2f}/"
        f"{closed_shapes['eyeBlinkRight']:.2f}, smile {blendshapes['mouthSmileLeft']:.2f}"
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--goldens", type=Path, help="also write face_state_goldens.json here")
    args = parser.parse_args()
    out = args.output_dir
    out.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory() as tmp:
        workdir = fetch_task(Path(tmp))
        convert(workdir / "face_landmarks_detector.tflite", out / MESH_NAME, MESH_IO)
        convert(workdir / "face_blendshapes.tflite", out / BLENDSHAPES_NAME, BLENDSHAPE_IO)
        verify(workdir, out)
        if args.goldens:
            write_goldens(workdir, out, args.goldens)
    print("ok")


if __name__ == "__main__":
    sys.exit(main())
