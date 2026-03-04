"""
Face detection and embedding service using InsightFace.
Provides face detection, 512-dim embeddings, and face thumbnails for indexing.
"""
from __future__ import annotations

import io
import base64
from typing import List, Tuple, Optional

import os
import numpy as np
from PIL import Image

from config import logger

# Lazy-loaded FaceAnalysis app
_face_app = None


def _get_face_app():
    """Lazy-load InsightFace FaceAnalysis (detection + recognition)."""
    global _face_app
    if _face_app is not None:
        return _face_app
    try:
        from insightface.app import FaceAnalysis
        root = os.environ.get("INSIGHTFACE_ROOT", os.path.expanduser("~/.insightface"))
        _face_app = FaceAnalysis(name="buffalo_l", root=root, providers=["CPUExecutionProvider"])
        _face_app.prepare(ctx_id=0, det_size=(640, 640))
        logger.info("InsightFace FaceAnalysis (buffalo_l) loaded.")
        return _face_app
    except Exception as e:
        logger.error(f"Failed to load InsightFace: {e}", exc_info=True)
        raise


def detect_faces(image_bytes: bytes) -> List[Tuple[List[float], str]]:
    """
    Detect faces in an image and return embedding + base64 thumbnail for each.

    Args:
        image_bytes: Raw image bytes (JPEG/PNG etc.)

    Returns:
        List of (embedding_512, thumbnail_base64_jpeg) per face.
        Embedding is L2-normalized 512-dim list of floats.
        Thumbnail is base64-encoded JPEG of the cropped face (max 112x112).
    """
    app = _get_face_app()
    img = np.array(Image.open(io.BytesIO(image_bytes)).convert("RGB"))
    faces = app.get(img)

    results = []
    for face in faces:
        emb = getattr(face, "embedding", None)
        bbox = getattr(face, "bbox", None)
        if emb is None:
            continue
        emb = np.array(emb, dtype=np.float32)
        # L2-normalize for cosine similarity in Chroma
        norm = np.linalg.norm(emb)
        if norm > 1e-6:
            emb = (emb / norm).tolist()
        else:
            emb = emb.tolist()

        thumbnail_b64 = ""
        if bbox is not None and len(bbox) >= 4:
            x1, y1, x2, y2 = [int(round(x)) for x in bbox[:4]]
            h, w = img.shape[:2]
            x1, y1 = max(0, x1), max(0, y1)
            x2, y2 = min(w, x2), min(h, y2)
            if x2 > x1 and y2 > y1:
                crop = img[y1:y2, x1:x2]
                thumb = Image.fromarray(crop).resize((112, 112), Image.Resampling.LANCZOS)
                buf = io.BytesIO()
                thumb.save(buf, format="JPEG", quality=85)
                thumbnail_b64 = base64.standard_b64encode(buf.getvalue()).decode("ascii")

        results.append((emb, thumbnail_b64))

    return results
