"""
Edit style training service.

Manages the `edit_training` ChromaDB collection that stores the user's own
Lightroom develop settings as few-shot examples.  When the AI generates a new
edit recipe it queries this collection by CLIP visual similarity and injects
the closest matches as style examples into the LLM prompt.

Enhanced with multi-criteria features:
  - Exposure metrics (luminance, contrast, highlight/shadow ratios)
  - Scene-type tags via CLIP zero-shot text probing
  - EXIF-based categorical fields (focal-length bucket, time-of-day, camera)
  - Statistics endpoint for the style-profile UI
"""

from __future__ import annotations

import json
from datetime import datetime
from typing import Any, Dict, List, Optional

import numpy as np

from config import logger

# Lazy ChromaDB globals – initialized on first use.
_chroma_client = None
_training_collection = None

COLLECTION_NAME = "edit_training"
EMBEDDING_DIM = (
    1152  # CLIP ViT-L/14 dimension used by the main image_embeddings collection
)

# ---------------------------------------------------------------------------
# Scene-type probe texts for CLIP zero-shot classification
# ---------------------------------------------------------------------------

_SCENE_PROBES: Dict[str, str] = {
    "scene_portrait": "a portrait photo of a person",
    "scene_landscape": "a landscape or nature photo",
    "scene_architecture": "an architectural or building photo",
    "scene_wildlife": "a wildlife or animal photo",
    "scene_event": "an event, wedding, or celebration photo",
    "scene_street": "a street photography or urban scene photo",
    "scene_macro": "a macro or close-up detail photo",
    "scene_interior": "an interior or indoor room photo",
    "scene_exterior": "an outdoor or exterior photo",
    "scene_golden_hour": "a photo taken at golden hour or sunset",
    "scene_studio": "a studio or controlled-light photo",
    "scene_action": "an action, sports, or motion photo",
}

_SCENE_THRESHOLD = 0.22  # cosine similarity threshold for a tag to be "present"

# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------


def _ensure_initialized() -> None:
    global _chroma_client, _training_collection
    if _training_collection is not None:
        return

    import config

    if not config.DB_PATH:
        logger.debug("edit_training initialization skipped: DB_PATH not set yet.")
        return

    import chromadb
    from chromadb.config import Settings

    logger.info(
        "Initializing edit_training ChromaDB collection (lazy at %s)...", config.DB_PATH
    )
    _chroma_client = chromadb.PersistentClient(
        path=config.DB_PATH,
        settings=Settings(anonymized_telemetry=False),
    )
    try:
        _training_collection = _chroma_client.get_collection(name=COLLECTION_NAME)
        logger.info("Loaded existing edit_training collection.")
    except Exception:
        _training_collection = _chroma_client.create_collection(name=COLLECTION_NAME)
        logger.info("Created new edit_training collection.")


def _dummy_embedding() -> List[float]:
    return np.zeros(EMBEDDING_DIM, dtype=np.float32).tolist()


def _safe_unit(value: float) -> float:
    return max(0.0, min(1.0, float(value)))


# ---------------------------------------------------------------------------
# Exposure metrics (computed from JPEG preview bytes)
# ---------------------------------------------------------------------------


def compute_exposure_metrics(image_bytes: bytes) -> Dict[str, float]:
    """Compute proxy RAW exposure characteristics from an image.

    Returns a dict of float metrics (all 0..1 normalized) suitable for
    storage as ChromaDB metadata and multi-criteria matching.
    """
    try:
        from PIL import Image
        import io

        image = Image.open(io.BytesIO(image_bytes)).convert("RGB")
        # Downscale for speed – we only need aggregate statistics
        if max(image.size) > 512:
            scale = 512 / float(max(image.size))
            new_size = (
                max(32, int(round(image.size[0] * scale))),
                max(32, int(round(image.size[1] * scale))),
            )
            image = image.resize(new_size, Image.Resampling.BILINEAR)

        rgb = np.asarray(image, dtype=np.float32) / 255.0
        gray = 0.299 * rgb[:, :, 0] + 0.587 * rgb[:, :, 1] + 0.114 * rgb[:, :, 2]

        lum_mean = float(np.mean(gray))
        lum_std = float(np.std(gray))

        highlight_ratio = float(np.mean(gray >= 0.92))
        shadow_ratio = float(np.mean(gray <= 0.08))
        midtone_ratio = float(np.mean((gray > 0.2) & (gray < 0.8)))

        # Colorfulness: mean chroma in rg-yb space
        rg = np.abs(rgb[:, :, 0] - rgb[:, :, 1])
        yb = np.abs(0.5 * (rgb[:, :, 0] + rgb[:, :, 1]) - rgb[:, :, 2])
        colorfulness = _safe_unit(float(np.mean(np.sqrt(rg**2 + yb**2))) / 0.35)

        # Warm/cool proxy: ratio of red channel mean to blue channel mean in highlights
        highlight_mask = gray > 0.7
        if np.any(highlight_mask):
            r_mean = float(np.mean(rgb[:, :, 0][highlight_mask]))
            b_mean = float(np.mean(rgb[:, :, 2][highlight_mask]))
            warmth_proxy = _safe_unit((r_mean - b_mean + 1.0) / 2.0)
        else:
            warmth_proxy = 0.5

        # Contrast via Michelson
        lum_max = float(np.percentile(gray, 97))
        lum_min = float(np.percentile(gray, 3))
        if (lum_max + lum_min) > 0:
            contrast = _safe_unit((lum_max - lum_min) / (lum_max + lum_min))
        else:
            contrast = 0.0

        return {
            "exp_luminance_mean": round(lum_mean, 4),
            "exp_luminance_std": round(lum_std, 4),
            "exp_highlight_ratio": round(highlight_ratio, 4),
            "exp_shadow_ratio": round(shadow_ratio, 4),
            "exp_midtone_ratio": round(midtone_ratio, 4),
            "exp_colorfulness": round(colorfulness, 4),
            "exp_warmth_proxy": round(warmth_proxy, 4),
            "exp_contrast": round(contrast, 4),
        }
    except Exception as exc:
        logger.warning("compute_exposure_metrics failed: %s", exc)
        return {
            "exp_luminance_mean": 0.5,
            "exp_luminance_std": 0.0,
            "exp_highlight_ratio": 0.0,
            "exp_shadow_ratio": 0.0,
            "exp_midtone_ratio": 0.0,
            "exp_colorfulness": 0.0,
            "exp_warmth_proxy": 0.5,
            "exp_contrast": 0.0,
        }


# ---------------------------------------------------------------------------
# Scene-type tagging via CLIP zero-shot probing
# ---------------------------------------------------------------------------


def compute_scene_tags(image_embedding: Optional[List[float]]) -> List[str]:
    """Return list of scene-type tag strings present in the image.

    Uses the image CLIP embedding compared against pre-computed text embeddings
    for each scene probe.  Returns tags whose cosine similarity exceeds
    ``_SCENE_THRESHOLD``.  Gracefully returns [] if CLIP is unavailable.
    """
    if image_embedding is None:
        return []

    try:
        import torch
        import torch.nn.functional as F
        import server_lifecycle
        from config import TORCH_DEVICE

        clip_model = server_lifecycle.get_model()
        clip_processor = server_lifecycle.get_processor()
        if clip_model is None or clip_processor is None:
            return []

        img_vec = (
            torch.tensor(image_embedding, dtype=torch.float32)
            .unsqueeze(0)
            .to(TORCH_DEVICE)
        )
        img_vec = F.normalize(img_vec, p=2, dim=1)

        tags: List[str] = []
        tokenize_fn = getattr(clip_model, "tokenize", None) or _get_clip_tokenize()
        if tokenize_fn is None:
            return []

        with torch.no_grad():
            for tag_name, probe_text in _SCENE_PROBES.items():
                try:
                    tokens = tokenize_fn([probe_text]).to(TORCH_DEVICE)
                    text_features = clip_model.encode_text(tokens)
                    text_vec = F.normalize(text_features, p=2, dim=1)
                    similarity = float((img_vec * text_vec).sum().cpu())
                    if similarity >= _SCENE_THRESHOLD:
                        tags.append(tag_name)
                except Exception:
                    pass

        return tags

    except Exception as exc:
        logger.debug("compute_scene_tags failed (non-critical): %s", exc)
        return []


def _get_clip_tokenize():
    """Retrieve open_clip tokenizer (lazy import)."""
    try:
        import open_clip

        return open_clip.get_tokenizer("ViT-L-14")
    except Exception:
        try:
            import clip

            return clip.tokenize
        except Exception:
            return None


# ---------------------------------------------------------------------------
# EXIF / catalog field bucketing
# ---------------------------------------------------------------------------


def focal_length_bucket(focal_length_mm: Optional[float]) -> str:
    """Map focal length in mm to a categorical bucket."""
    if focal_length_mm is None:
        return "unknown"
    fl = float(focal_length_mm)
    if fl < 20:
        return "ultra_wide"
    if fl < 35:
        return "wide"
    if fl < 70:
        return "normal"
    if fl < 135:
        return "short_tele"
    if fl < 300:
        return "tele"
    return "super_tele"


def time_of_day_bucket(capture_unix: Optional[float]) -> str:
    """Map a Unix timestamp to a categorical time-of-day bucket (local hour)."""
    if capture_unix is None:
        return "unknown"
    try:
        dt = datetime.fromtimestamp(capture_unix)
        hour = dt.hour
        if 5 <= hour < 8:
            return "dawn"
        if 8 <= hour < 12:
            return "morning"
        if 12 <= hour < 17:
            return "afternoon"
        if 17 <= hour < 20:
            return "evening"
        return "night"
    except Exception:
        return "unknown"


# ---------------------------------------------------------------------------
# Develop settings normalisation for interpolation
# ---------------------------------------------------------------------------

# Mapping from Lightroom develop keys to canonical recipe key names used
# by edit_recipe.GLOBAL_FIELD_RANGES.  Only numeric sliders that are safe
# to interpolate are listed here.
_LR_TO_CANONICAL: Dict[str, str] = {
    "Exposure2012": "exposure",
    "Contrast2012": "contrast",
    "Highlights2012": "highlights",
    "Shadows2012": "shadows",
    "Whites2012": "whites",
    "Blacks2012": "blacks",
    "Temp": "temperature",
    "Tint": "tint",
    "Texture": "texture",
    "Clarity2012": "clarity",
    "Dehaze": "dehaze",
    "Vibrance": "vibrance",
    "Saturation": "saturation",
    "Sharpness": "sharpening",
    "LuminanceSmoothing": "noise_reduction",
    "ColorNoiseReduction": "color_noise_reduction",
    "PostCropVignetteAmount": "vignette",
    "GrainAmount": "grain",
    "ParametricHighlights": "tone_curve_highlights",
    "ParametricLights": "tone_curve_lights",
    "ParametricDarks": "tone_curve_darks",
    "ParametricShadows": "tone_curve_shadows",
}


def normalize_develop_settings_for_style(
    develop_settings: Dict[str, Any],
) -> Dict[str, float]:
    """Convert raw LR develop settings dict to canonical float form for interpolation."""
    canonical: Dict[str, float] = {}
    for lr_key, canon_key in _LR_TO_CANONICAL.items():
        raw = develop_settings.get(lr_key)
        if raw is not None and isinstance(raw, (int, float)):
            canonical[canon_key] = round(float(raw), 4)
    return canonical


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------


def add_training_example(
    photo_id: str,
    develop_settings: Dict[str, Any],
    embedding: Optional[List[float]],
    *,
    label: Optional[str] = None,
    filename: Optional[str] = None,
    summary: Optional[str] = None,
    image_bytes: Optional[bytes] = None,
    focal_length: Optional[float] = None,
    capture_time_unix: Optional[float] = None,
    camera_make: Optional[str] = None,
    camera_model: Optional[str] = None,
    iso: Optional[float] = None,
    aperture: Optional[float] = None,
    shutter_speed: Optional[str] = None,
) -> None:
    """Store or overwrite a training example.

    Args:
        photo_id:         Stable photo identifier (same as main collection).
        develop_settings: Raw Lightroom develop settings dict captured from the photo.
        embedding:        CLIP embedding for the source photo (1152-d float list).
                          Falls back to a zero-dummy when None.
        label:            Optional user-facing style label (e.g. "Wedding").
        filename:         Original filename for display purposes.
        summary:          Optional short description of the edit style.
        image_bytes:      Raw image bytes for exposure metric computation.
        focal_length:     Focal length in mm from EXIF.
        capture_time_unix: Capture time as Unix timestamp.
        camera_make:      Camera manufacturer string.
        camera_model:     Camera model string.
        iso:              ISO value.
        aperture:         Aperture f-number.
        shutter_speed:    Shutter speed string (e.g. "1/250").
    """
    _ensure_initialized()
    if _training_collection is None:
        logger.warning(
            "add_training_example skipped: service not initialized (DB_PATH missing)."
        )
        return
    if not photo_id:
        raise ValueError("photo_id is required")

    metadata: Dict[str, Any] = {
        "photo_id": photo_id,
        "develop_settings": json.dumps(develop_settings, ensure_ascii=False),
        "canonical_settings": json.dumps(
            normalize_develop_settings_for_style(develop_settings), ensure_ascii=False
        ),
        "captured_at": datetime.now().strftime("%Y-%m-%d %H:%M:%S"),
        "has_embedding": embedding is not None,
    }
    if label:
        metadata["label"] = label
    if filename:
        metadata["filename"] = filename
    if summary:
        metadata["summary"] = summary

    # EXIF categorical fields
    metadata["focal_length_bucket"] = focal_length_bucket(focal_length)
    metadata["time_of_day_bucket"] = time_of_day_bucket(capture_unix=capture_time_unix)
    if camera_make:
        metadata["camera_make"] = camera_make[:64]
    if camera_model:
        metadata["camera_model"] = camera_model[:64]
    if iso is not None:
        metadata["iso"] = float(iso)
    if aperture is not None:
        metadata["aperture"] = float(aperture)
    if shutter_speed:
        metadata["shutter_speed"] = str(shutter_speed)[:16]

    # Exposure metrics from JPEG preview
    if image_bytes:
        exp_metrics = compute_exposure_metrics(image_bytes)
        metadata.update(exp_metrics)

    # Scene-type tags from CLIP zero-shot
    scene_tags = compute_scene_tags(embedding)
    metadata["scene_tags"] = json.dumps(scene_tags, ensure_ascii=False)

    emb = embedding if embedding is not None else _dummy_embedding()

    # Upsert: update if already present, add otherwise.
    existing = _training_collection.get(ids=[photo_id], include=[])
    if existing and existing.get("ids"):
        _training_collection.update(
            ids=[photo_id], embeddings=[emb], metadatas=[metadata]
        )
        logger.info(
            "Updated training example photo_id=%s scene_tags=%s", photo_id, scene_tags
        )
    else:
        _training_collection.add(ids=[photo_id], embeddings=[emb], metadatas=[metadata])
        logger.info(
            "Added training example photo_id=%s scene_tags=%s", photo_id, scene_tags
        )


def delete_training_example(photo_id: str) -> bool:
    """Remove a training example.

    Returns True when the item existed and was deleted, False otherwise.
    """
    _ensure_initialized()
    if _training_collection is None:
        return False
    if not photo_id:
        return False
    existing = _training_collection.get(ids=[photo_id], include=[])
    if not existing or not existing.get("ids"):
        return False
    _training_collection.delete(ids=[photo_id])
    logger.info("Deleted training example photo_id=%s", photo_id)
    return True


def get_training_count() -> int:
    """Return the number of stored training examples."""
    _ensure_initialized()
    if _training_collection is None:
        return 0
    result = _training_collection.get(include=[], limit=1_000_000)
    return len(result.get("ids") or [])


def list_training_examples() -> List[Dict[str, Any]]:
    """Return all training examples as a list of dicts (no embeddings)."""
    _ensure_initialized()
    if _training_collection is None:
        return []
    result = _training_collection.get(include=["metadatas"], limit=1_000_000)
    ids = result.get("ids") or []
    metadatas = result.get("metadatas") or []
    examples = []
    for i, pid in enumerate(ids):
        meta = dict(metadatas[i]) if i < len(metadatas) else {}
        examples.append(
            {
                "photo_id": pid,
                "filename": meta.get("filename", ""),
                "label": meta.get("label", ""),
                "summary": meta.get("summary", ""),
                "captured_at": meta.get("captured_at", ""),
                "has_embedding": bool(meta.get("has_embedding", False)),
                "focal_length_bucket": meta.get("focal_length_bucket", "unknown"),
                "time_of_day_bucket": meta.get("time_of_day_bucket", "unknown"),
                "scene_tags": _safe_json_list(meta.get("scene_tags", "[]")),
            }
        )
    examples.sort(key=lambda x: x["captured_at"], reverse=True)
    return examples


def get_training_stats() -> Dict[str, Any]:
    """Return aggregate statistics over all training examples for the style profile UI.

    Returns:
        {
            "count": int,
            "has_enough_examples": bool,
            "readiness": "cold_start" | "limited" | "active",
            "scene_distribution": { "scene_portrait": 3, ... },
            "exposure": { "mean_luminance": 0.45, "mean_contrast": 0.6, ... },
            "focal_buckets": { "normal": 5, "tele": 2, ... },
            "time_of_day": { "afternoon": 7, ... },
        }
    """
    _ensure_initialized()
    if _training_collection is None:
        return {
            "count": 0,
            "has_enough_examples": False,
            "readiness": "cold_start",
            "scene_distribution": {},
            "focal_buckets": {},
            "time_of_day": {},
            "camera_distribution": {},
            "exposure": {},
        }
    result = _training_collection.get(include=["metadatas"], limit=1_000_000)
    ids = result.get("ids") or []
    metadatas = result.get("metadatas") or []
    count = len(ids)

    scene_dist: Dict[str, int] = {}
    focal_dist: Dict[str, int] = {}
    tod_dist: Dict[str, int] = {}
    camera_dist: Dict[str, int] = {}
    exp_means: List[float] = []
    exp_contrasts: List[float] = []
    exp_colorfulness: List[float] = []

    for meta in metadatas:
        if not isinstance(meta, dict):
            continue
        tags = _safe_json_list(meta.get("scene_tags", "[]"))
        for tag in tags:
            scene_dist[tag] = scene_dist.get(tag, 0) + 1

        fb = meta.get("focal_length_bucket", "unknown")
        focal_dist[fb] = focal_dist.get(fb, 0) + 1

        tod = meta.get("time_of_day_bucket", "unknown")
        tod_dist[tod] = tod_dist.get(tod, 0) + 1

        cam = meta.get("camera_model", meta.get("camera_make", "unknown"))
        camera_dist[cam] = camera_dist.get(cam, 0) + 1

        if "exp_luminance_mean" in meta:
            exp_means.append(float(meta["exp_luminance_mean"]))
        if "exp_contrast" in meta:
            exp_contrasts.append(float(meta["exp_contrast"]))
        if "exp_colorfulness" in meta:
            exp_colorfulness.append(float(meta["exp_colorfulness"]))

    if count == 0:
        readiness = "cold_start"
    elif count < 10:
        readiness = "warming_up"
    elif count < 50:
        readiness = "limited"
    else:
        readiness = "active"

    exposure_stats: Dict[str, Any] = {}
    if exp_means:
        exposure_stats["mean_luminance"] = round(sum(exp_means) / len(exp_means), 3)
    if exp_contrasts:
        exposure_stats["mean_contrast"] = round(
            sum(exp_contrasts) / len(exp_contrasts), 3
        )
    if exp_colorfulness:
        exposure_stats["mean_colorfulness"] = round(
            sum(exp_colorfulness) / len(exp_colorfulness), 3
        )

    return {
        "count": count,
        "has_enough_examples": count >= 10,
        "readiness": readiness,
        "scene_distribution": scene_dist,
        "focal_buckets": focal_dist,
        "time_of_day": tod_dist,
        "camera_distribution": camera_dist,
        "exposure": exposure_stats,
    }


def query_similar_training_examples(
    query_embedding: List[float],
    n_results: int = 5,
) -> List[Dict[str, Any]]:
    """Return up to n_results training examples closest to query_embedding.

    Each result dict contains:
        photo_id, develop_settings (dict), canonical_settings (dict),
        label, filename, summary, distance, scene_tags, exp_luminance_mean,
        exp_contrast, focal_length_bucket, time_of_day_bucket.

    Returns an empty list when no training examples exist or embedding is None.
    """
    _ensure_initialized()
    if _training_collection is None or query_embedding is None:
        return []

    count = get_training_count()
    if count == 0:
        return []

    n_results = min(n_results, count)
    try:
        result = _training_collection.query(
            query_embeddings=[query_embedding],
            n_results=n_results,
            include=["metadatas", "distances"],
        )
    except Exception as exc:
        logger.error("query_similar_training_examples failed: %s", exc, exc_info=True)
        return []

    ids = (result.get("ids") or [[]])[0]
    metadatas = (result.get("metadatas") or [[]])[0]
    distances = (result.get("distances") or [[]])[0]

    examples = []
    for i, pid in enumerate(ids):
        meta = dict(metadatas[i]) if i < len(metadatas) else {}
        dev_settings_raw = meta.get("develop_settings", "{}")
        try:
            dev_settings = json.loads(dev_settings_raw)
        except (ValueError, TypeError):
            dev_settings = {}

        canonical_raw = meta.get("canonical_settings", "{}")
        try:
            canonical_settings = json.loads(canonical_raw)
        except (ValueError, TypeError):
            canonical_settings = {}

        examples.append(
            {
                "photo_id": pid,
                "develop_settings": dev_settings,
                "canonical_settings": canonical_settings,
                "label": meta.get("label", ""),
                "filename": meta.get("filename", ""),
                "summary": meta.get("summary", ""),
                "distance": float(distances[i]) if i < len(distances) else 1.0,
                "scene_tags": _safe_json_list(meta.get("scene_tags", "[]")),
                "exp_luminance_mean": float(meta.get("exp_luminance_mean", 0.5)),
                "exp_contrast": float(meta.get("exp_contrast", 0.0)),
                "exp_colorfulness": float(meta.get("exp_colorfulness", 0.0)),
                "exp_warmth_proxy": float(meta.get("exp_warmth_proxy", 0.5)),
                "exp_highlight_ratio": float(meta.get("exp_highlight_ratio", 0.0)),
                "exp_shadow_ratio": float(meta.get("exp_shadow_ratio", 0.0)),
                "focal_length_bucket": meta.get("focal_length_bucket", "unknown"),
                "time_of_day_bucket": meta.get("time_of_day_bucket", "unknown"),
            }
        )
    return examples


def clear_all_training_examples() -> int:
    """Delete every training example. Returns the number removed."""
    _ensure_initialized()
    if _training_collection is None:
        return 0
    result = _training_collection.get(include=[], limit=1_000_000)
    ids = result.get("ids") or []
    if not ids:
        return 0
    _training_collection.delete(ids=ids)
    logger.info("Cleared all %d training examples.", len(ids))
    return len(ids)


# ---------------------------------------------------------------------------
# Private helpers
# ---------------------------------------------------------------------------


def _safe_json_list(value: Any) -> List[str]:
    """Safely decode a JSON string to a list, returning [] on failure."""
    if isinstance(value, list):
        return [str(v) for v in value]
    try:
        parsed = json.loads(value or "[]")
        if isinstance(parsed, list):
            return [str(v) for v in parsed]
    except Exception:
        pass
    return []
