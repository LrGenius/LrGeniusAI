"""
Face detection and embedding service using InsightFace.
Provides face detection, 512-dim embeddings, thumbnails, and lightweight
face-quality metadata for indexing and culling.
"""

from __future__ import annotations

import io
import base64
from typing import List, Dict, Any

import os
import numpy as np
from PIL import Image

from config import logger, CULLING_CONFIG

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
        _face_app = FaceAnalysis(
            name="buffalo_l", root=root, providers=["CPUExecutionProvider"]
        )
        _face_app.prepare(ctx_id=0, det_size=(640, 640))
        logger.info("InsightFace FaceAnalysis (buffalo_l) loaded.")
        return _face_app
    except Exception as e:
        logger.error(f"Failed to load InsightFace: {e}", exc_info=True)
        raise


def unload_face_app():
    """Unload the InsightFace model to free memory."""
    global _face_app
    if _face_app is None:
        return
    logger.info("Unloading InsightFace FaceAnalysis model...")
    _face_app = None
    import gc

    gc.collect()
    logger.info("Unloaded InsightFace model.")


def _compute_face_sharpness(crop_rgb: np.ndarray) -> float:
    if crop_rgb.size == 0:
        return 0.0
    gray = (
        0.299 * crop_rgb[:, :, 0].astype(np.float32)
        + 0.587 * crop_rgb[:, :, 1].astype(np.float32)
        + 0.114 * crop_rgb[:, :, 2].astype(np.float32)
    ) / 255.0
    if gray.shape[0] < 3 or gray.shape[1] < 3:
        return 0.0
    center = gray[1:-1, 1:-1]
    laplacian = (
        -4.0 * center
        + gray[:-2, 1:-1]
        + gray[2:, 1:-1]
        + gray[1:-1, :-2]
        + gray[1:-1, 2:]
    )
    variance = float(np.var(laplacian))
    denominator = CULLING_CONFIG["face_metrics"]["face_sharpness_denominator"]
    return max(0.0, min(1.0, variance / (variance + denominator)))


def _compute_eye_openness_proxy(
    crop_rgb: np.ndarray, bbox_list: List[int], keypoints
) -> float:
    """
    Lightweight proxy for eye openness using vertical gradient energy near eye landmarks.
    This is not a full landmark-based blink detector, but works as a cheap signal.
    """
    if crop_rgb.size == 0 or not bbox_list or keypoints is None:
        return 0.0
    try:
        kps = np.asarray(keypoints, dtype=np.float32)
    except Exception:
        return 0.0
    if kps.ndim != 2 or kps.shape[0] < 2 or kps.shape[1] < 2:
        return 0.0

    x1, y1, x2, y2 = bbox_list
    gray = (
        0.299 * crop_rgb[:, :, 0].astype(np.float32)
        + 0.587 * crop_rgb[:, :, 1].astype(np.float32)
        + 0.114 * crop_rgb[:, :, 2].astype(np.float32)
    ) / 255.0
    h, w = gray.shape[:2]
    if h < 6 or w < 6:
        return 0.0

    face_span = max(4.0, float(min(x2 - x1, y2 - y1)))
    patch_ratio = CULLING_CONFIG["face_metrics"]["eye_patch_ratio"]
    patch_radius_min = CULLING_CONFIG["face_metrics"]["eye_patch_radius_min"]
    patch_radius_max = CULLING_CONFIG["face_metrics"]["eye_patch_radius_max"]
    patch_radius = int(
        max(patch_radius_min, min(patch_radius_max, round(face_span * patch_ratio)))
    )
    eye_scores = []

    # InsightFace 5-point format starts with left and right eye.
    for eye_idx in [0, 1]:
        ex = int(round(kps[eye_idx, 0] - x1))
        ey = int(round(kps[eye_idx, 1] - y1))
        px1 = max(0, ex - patch_radius)
        py1 = max(0, ey - patch_radius)
        px2 = min(w, ex + patch_radius + 1)
        py2 = min(h, ey + patch_radius + 1)
        if px2 - px1 < 3 or py2 - py1 < 3:
            continue
        patch = gray[py1:py2, px1:px2]
        # Eyes-open tends to keep stronger local vertical gradients than shut eyelids.
        vgrad = np.abs(np.diff(patch, axis=0))
        score_raw = float(np.mean(vgrad))
        denominator = CULLING_CONFIG["face_metrics"]["eye_openness_denominator"]
        score = max(0.0, min(1.0, score_raw / (score_raw + denominator)))
        eye_scores.append(score)

    if not eye_scores:
        return 0.0
    return float(sum(eye_scores) / len(eye_scores))


def _compute_occlusion_proxy(
    det_score: float, center_proximity: float, eye_openness: float
) -> float:
    return max(
        0.0,
        min(
            1.0,
            1.0
            - (
                CULLING_CONFIG["face_metrics"]["occlusion_det_weight"]
                * max(0.0, min(1.0, det_score))
                + CULLING_CONFIG["face_metrics"]["occlusion_center_weight"]
                * max(0.0, min(1.0, center_proximity))
                + CULLING_CONFIG["face_metrics"]["occlusion_eye_weight"]
                * max(0.0, min(1.0, eye_openness))
            ),
        ),
    )


def detect_faces(image_bytes: bytes) -> List[Dict[str, Any]]:
    """
    Detect faces in an image and return embedding, thumbnail, and quality metadata.

    Args:
        image_bytes: Raw image bytes (JPEG/PNG etc.)

    Returns:
        List of dicts with keys:
        - embedding: L2-normalized 512-dim list of floats
        - thumbnail: base64-encoded JPEG of the cropped face (max 112x112)
        - bbox: [x1, y1, x2, y2]
        - area_ratio: relative face area in the full image
        - sharpness: normalized face sharpness estimate (0..1)
        - det_score: detector confidence if available
        - center_proximity: how central the face is (0..1)
    """
    app = _get_face_app()
    img = np.array(Image.open(io.BytesIO(image_bytes)).convert("RGB"))
    faces = app.get(img)
    image_height, image_width = img.shape[:2]
    image_area = float(max(1, image_width * image_height))

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
        bbox_list = []
        area_ratio = 0.0
        sharpness = 0.0
        center_proximity = 0.0
        eye_openness = 0.0
        occlusion = 0.0
        if bbox is not None and len(bbox) >= 4:
            x1, y1, x2, y2 = [int(round(x)) for x in bbox[:4]]
            h, w = img.shape[:2]
            x1, y1 = max(0, x1), max(0, y1)
            x2, y2 = min(w, x2), min(h, y2)
            if x2 > x1 and y2 > y1:
                crop = img[y1:y2, x1:x2]
                bbox_list = [x1, y1, x2, y2]
                area_ratio = max(0.0, min(1.0, ((x2 - x1) * (y2 - y1)) / image_area))
                sharpness = _compute_face_sharpness(crop)
                eye_openness = _compute_eye_openness_proxy(
                    crop, bbox_list, getattr(face, "kps", None)
                )
                center_x = (x1 + x2) / 2.0
                center_y = (y1 + y2) / 2.0
                offset_x = abs((center_x / max(1.0, image_width)) - 0.5) * 2.0
                offset_y = abs((center_y / max(1.0, image_height)) - 0.5) * 2.0
                center_proximity = max(
                    0.0, min(1.0, 1.0 - ((offset_x + offset_y) / 2.0))
                )
                thumb = Image.fromarray(crop).resize(
                    (112, 112), Image.Resampling.LANCZOS
                )
                buf = io.BytesIO()
                thumb.save(buf, format="JPEG", quality=85)
                thumbnail_b64 = base64.standard_b64encode(buf.getvalue()).decode(
                    "ascii"
                )
                det_score = float(getattr(face, "det_score", 0.0) or 0.0)
                occlusion = _compute_occlusion_proxy(
                    det_score, center_proximity, eye_openness
                )

        results.append(
            {
                "embedding": emb,
                "thumbnail": thumbnail_b64,
                "bbox": bbox_list,
                "area_ratio": round(area_ratio, 4),
                "sharpness": round(sharpness, 4),
                "det_score": float(getattr(face, "det_score", 0.0) or 0.0),
                "center_proximity": round(center_proximity, 4),
                "eye_openness": round(eye_openness, 4),
                "blink_penalty": round(max(0.0, min(1.0, 1.0 - eye_openness)), 4),
                "occlusion": round(occlusion, 4),
            }
        )

    return results
