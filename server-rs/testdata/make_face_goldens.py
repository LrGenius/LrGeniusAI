"""M5 golden fixtures: run the real production detect_faces() (InsightFace
buffalo_l via services/face.py) on real face photos, dumping bbox/kps/
det_score/embedding/quality-metrics so the Rust SCRFD+ArcFace port can be
verified end-to-end."""
import json, os, sys
sys.path.insert(0, "/Users/bm/src/LrGeniusAI/server/src")
sys.argv = ["x"]
import numpy as np
from PIL import Image
from skimage import data

from services import face as svc_face

SCRATCH = os.path.dirname(os.path.abspath(__file__))
astronaut = data.astronaut()  # 512x512x3 uint8 RGB, one real face
Image.fromarray(astronaut).save(os.path.join(SCRATCH, "astronaut.jpg"), quality=95)

cases = {}
for name, arr in [("astronaut_512", astronaut)]:
    img = Image.fromarray(arr, "RGB")
    buf_path = os.path.join(SCRATCH, f"{name}.jpg")
    img.save(buf_path, quality=95)
    with open(buf_path, "rb") as f:
        image_bytes = f.read()
    results = svc_face.detect_faces(image_bytes, pil_image=img)
    cases[name] = {
        "width": img.width,
        "height": img.height,
        "faces": [
            {
                "bbox": r["bbox"],
                "area_ratio": r["area_ratio"],
                "sharpness": r["sharpness"],
                "det_score": r["det_score"],
                "center_proximity": r["center_proximity"],
                "eye_openness": r["eye_openness"],
                "blink_penalty": r["blink_penalty"],
                "occlusion": r["occlusion"],
                "embedding": r["embedding"],
            }
            for r in results
        ],
    }
    print(name, "faces found:", len(results))
    for r in results:
        print("  bbox:", r["bbox"], "det_score:", r["det_score"])

dst = "/Users/bm/src/LrGeniusAI/server-rs/testdata/face_goldens.json"
json.dump(cases, open(dst, "w"))
print("wrote", dst)
