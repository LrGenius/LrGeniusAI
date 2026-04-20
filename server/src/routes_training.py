"""
Flask blueprint for edit-style training endpoints.

Routes
------
POST /training/add      – Store a new training example (with EXIF + image for exposure/scene analysis)
GET  /training/list     – List all stored training examples (no embeddings)
GET  /training/count    – Return { "count": N }
GET  /training/stats    – Return aggregate style-profile statistics
DELETE /training/<id>   – Remove one training example by photo_id
DELETE /training        – Clear all training examples
"""
from __future__ import annotations

import json

from flask import Blueprint, jsonify, request
from PIL import Image
import io

from config import logger
import service_training as training_service

training_bp = Blueprint("training", __name__)


def _compute_clip_embedding(image_bytes: bytes):
    """Compute a CLIP embedding for the supplied JPEG/PNG bytes.

    Re-uses the global CLIP model that is already loaded by service_index
    when the server starts.  Returns None when the model is not available.
    """
    try:
        import torch
        import torch.nn.functional as F
        import server_lifecycle
        from config import TORCH_DEVICE

        clip_model = server_lifecycle.get_model()
        clip_processor = server_lifecycle.get_processor()
        if clip_model is None or clip_processor is None:
            return None

        image = Image.open(io.BytesIO(image_bytes)).convert("RGB")
        image_tensor = clip_processor(image).unsqueeze(0).to(TORCH_DEVICE)
        with torch.no_grad():
            features = clip_model.encode_image(image_tensor)
            normalized = F.normalize(features, p=2, dim=1)
            return normalized.cpu().numpy()[0].tolist()
    except Exception as exc:
        logger.warning("Could not compute CLIP embedding for training example: %s", exc)
        return None


# ---------------------------------------------------------------------------
# POST /training/add
# ---------------------------------------------------------------------------

@training_bp.route("/training/add", methods=["POST"])
def add_training_example():
    """Accept a multipart/form-data upload with:
        - photo_id          (form field, required)
        - develop_settings  (form field, JSON string, required)
        - image             (file, optional – used to compute CLIP embedding + exposure/scene metrics)
        - label             (form field, optional)
        - summary           (form field, optional)
        - focal_length      (form field, float mm, optional)
        - capture_time      (form field, float unix timestamp, optional)
        - camera_make       (form field, string, optional)
        - camera_model      (form field, string, optional)
        - iso               (form field, float, optional)
        - aperture          (form field, float, optional)
        - shutter_speed     (form field, string, optional)
    """
    photo_id = request.form.get("photo_id", "").strip()
    if not photo_id:
        return jsonify({"error": "photo_id is required"}), 400

    dev_settings_raw = request.form.get("develop_settings", "")
    try:
        develop_settings = json.loads(dev_settings_raw) if dev_settings_raw else {}
    except (ValueError, TypeError):
        return jsonify({"error": "develop_settings must be valid JSON"}), 400

    label = request.form.get("label", "").strip() or None
    summary = request.form.get("summary", "").strip() or None
    filename = None

    # Optional EXIF fields for richer matching
    focal_length: float | None = None
    try:
        fl_raw = request.form.get("focal_length")
        if fl_raw:
            focal_length = float(fl_raw)
    except (TypeError, ValueError):
        pass

    capture_time_unix: float | None = None
    try:
        ct_raw = request.form.get("capture_time")
        if ct_raw:
            capture_time_unix = float(ct_raw)
    except (TypeError, ValueError):
        pass

    def _opt_str(key):
        val = request.form.get(key, "").strip()
        return val or None

    camera_make = _opt_str("camera_make")
    camera_model_str = _opt_str("camera_model")
    shutter_speed = _opt_str("shutter_speed")

    iso: float | None = None
    try:
        iso_raw = request.form.get("iso")
        if iso_raw:
            iso = float(iso_raw)
    except (TypeError, ValueError):
        pass

    aperture: float | None = None
    try:
        ap_raw = request.form.get("aperture")
        if ap_raw:
            aperture = float(ap_raw)
    except (TypeError, ValueError):
        pass

    # Compute CLIP embedding from uploaded image (best-effort).
    embedding = None
    image_bytes_data = None
    image_file = request.files.get("image")
    if image_file:
        filename = image_file.filename or None
        try:
            image_bytes_data = image_file.read()
            embedding = _compute_clip_embedding(image_bytes_data)
        except Exception as exc:
            warning_msg = f"Failed to read image for training embedding: {exc}"
            logger.warning(warning_msg)

    try:
        training_service.add_training_example(
            photo_id=photo_id,
            develop_settings=develop_settings,
            embedding=embedding,
            label=label,
            filename=filename,
            summary=summary,
            image_bytes=image_bytes_data,
            focal_length=focal_length,
            capture_time_unix=capture_time_unix,
            camera_make=camera_make,
            camera_model=camera_model_str,
            iso=iso,
            aperture=aperture,
            shutter_speed=shutter_speed,
        )
        count = training_service.get_training_count()
        response_data = {"status": "ok", "photo_id": photo_id, "total_count": count}
        if image_file and embedding is None:
            response_data["warning"] = "Could not compute CLIP embedding for training example. AI style prediction may be less accurate."
        return jsonify(response_data), 200
    except Exception as exc:
        logger.error("Failed to add training example photo_id=%s: %s", photo_id, exc, exc_info=True)
        return jsonify({"error": str(exc)}), 500


# ---------------------------------------------------------------------------
# GET /training/list
# ---------------------------------------------------------------------------

@training_bp.route("/training/list", methods=["GET"])
def list_training_examples():
    try:
        examples = training_service.list_training_examples()
        return jsonify({"status": "ok", "examples": examples, "count": len(examples)}), 200
    except Exception as exc:
        logger.error("Failed to list training examples: %s", exc, exc_info=True)
        return jsonify({"error": str(exc)}), 500


# ---------------------------------------------------------------------------
# GET /training/stats
# ---------------------------------------------------------------------------

@training_bp.route("/training/stats", methods=["GET"])
def get_training_stats():
    """Return aggregate style-profile statistics for the plugin UI."""
    try:
        stats = training_service.get_training_stats()
        return jsonify({"status": "ok", **stats}), 200
    except Exception as exc:
        logger.error("Failed to get training stats: %s", exc, exc_info=True)
        return jsonify({"error": str(exc)}), 500


# ---------------------------------------------------------------------------
# GET /training/count
# ---------------------------------------------------------------------------

@training_bp.route("/training/count", methods=["GET"])
def get_training_count():
    try:
        count = training_service.get_training_count()
        return jsonify({"count": count}), 200
    except Exception as exc:
        logger.error("Failed to get training count: %s", exc, exc_info=True)
        return jsonify({"error": str(exc)}), 500


# ---------------------------------------------------------------------------
# DELETE /training/<photo_id>
# ---------------------------------------------------------------------------

@training_bp.route("/training/<path:photo_id>", methods=["DELETE"])
def delete_training_example(photo_id: str):
    try:
        deleted = training_service.delete_training_example(photo_id)
        if deleted:
            count = training_service.get_training_count()
            return jsonify({"status": "ok", "photo_id": photo_id, "total_count": count}), 200
        return jsonify({"error": f"No training example found for photo_id={photo_id}"}), 404
    except Exception as exc:
        logger.error("Failed to delete training example photo_id=%s: %s", photo_id, exc, exc_info=True)
        return jsonify({"error": str(exc)}), 500


# ---------------------------------------------------------------------------
# DELETE /training  (clear all)
# ---------------------------------------------------------------------------

@training_bp.route("/training", methods=["DELETE"])
def clear_training_examples():
    try:
        removed = training_service.clear_all_training_examples()
        return jsonify({"status": "ok", "removed": removed}), 200
    except Exception as exc:
        logger.error("Failed to clear training examples: %s", exc, exc_info=True)
        return jsonify({"error": str(exc)}), 500
