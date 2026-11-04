"""Golden fixtures for `lrg_imaging::focus`: scikit-image's
`skimage.measure.blur_effect` on inputs the Rust test can rebuild exactly.

The synthetic cases are formulas (sines, a checkerboard, a 31-bit LCG) so the
Rust side regenerates identical float64 patches and the comparison can be
tight. The astronaut cases go through two different JPEG decoders (Pillow
here, the `image` crate there), which may disagree by a level here and there,
so they carry a looser tolerance -- they exist to show the port agrees on a
real face, eyes included, not to pin the last digit.

Run:  uv run --with scikit-image --with pillow \
        python server-rs/testdata/make_focus_goldens.py
"""

import json
import math
import os

import numpy as np
from PIL import Image
from skimage.measure import blur_effect

HERE = os.path.dirname(os.path.abspath(__file__))


def lcg_luma(w, h, seed=42):
    """31-bit LCG (glibc constants), byte = (x >> 16) & 0xFF, as luma 0..1."""
    out = np.empty(w * h, dtype=np.float64)
    x = seed
    for i in range(w * h):
        x = (x * 1103515245 + 12345) & 0x7FFFFFFF
        out[i] = ((x >> 16) & 0xFF) / 255.0
    return out.reshape(h, w)


def sines(w, h):
    out = np.empty((h, w), dtype=np.float64)
    for y in range(h):
        for x in range(w):
            v = (
                0.5
                + 0.18 * math.sin(x * 1.3) * math.cos(y * 0.9)
                + 0.12 * math.sin(x * 0.37 + y * 0.21)
                + 0.08 * math.sin((x - y) * 2.1)
            )
            out[y, x] = min(max(v, 0.0), 1.0)
    return out


def checker(w, h, cell):
    out = np.empty((h, w), dtype=np.float64)
    for y in range(h):
        for x in range(w):
            out[y, x] = float(((x // cell) + (y // cell)) % 2)
    return out


def astronaut_region(rect):
    rgb = np.asarray(
        Image.open(os.path.join(HERE, "astronaut.jpg")).convert("RGB")
    ).astype(np.float64)
    luma = (0.299 * rgb[..., 0] + 0.587 * rgb[..., 1] + 0.114 * rgb[..., 2]) / 255.0
    x0, y0 = int(math.floor(rect[0])), int(math.floor(rect[1]))
    x1, y1 = int(math.ceil(rect[2])), int(math.ceil(rect[3]))
    return luma[y0:y1, x0:x1]


cases = []


def add(name, arr, windows=(11,), tolerance=1e-9, rect=None):
    for window in windows:
        case = {
            "name": name,
            "width": int(arr.shape[1]),
            "height": int(arr.shape[0]),
            "window": window,
            "blur_effect": float(blur_effect(arr, h_size=window)),
            "tolerance": tolerance,
        }
        if rect is not None:
            case["rect"] = rect
        cases.append(case)


add("sines", sines(48, 48), windows=(5, 11))
add("sines_wide", sines(61, 23))
add("lcg", lcg_luma(32, 40), windows=(3, 11))
add("checker4", checker(40, 40, 4))

# The eyes and face of the reference portrait (eye centres at about (203, 101)
# and (248, 103), interocular distance 45 px, so a 0.25 * IOD half-side).
for name, rect in [
    ("astronaut_right_eye", [191.75, 89.75, 214.25, 112.25]),
    ("astronaut_left_eye", [236.75, 91.75, 259.25, 114.25]),
    ("astronaut_face", [175.0, 70.0, 275.0, 190.0]),
]:
    add(name, astronaut_region(rect), tolerance=1e-2, rect=rect)

with open(os.path.join(HERE, "focus_goldens.json"), "w") as f:
    json.dump({"cases": cases}, f, indent=1)
    f.write("\n")
print(f"wrote {len(cases)} cases")
