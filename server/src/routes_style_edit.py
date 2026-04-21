"""
Flask blueprint: POST /style_edit

LLM-free style-matched edit endpoint.  Given a photo and its JPEG preview,
the style engine matches the photo against the user's saved training examples
and returns an interpolated Lightroom edit recipe — no LLM call required.

When the training set is too small or confidence is too low and
``use_llm_fallback=true``, the request is transparently forwarded to
the regular LLM-backed edit pipeline (re-using existing few-shot injection).
"""

from __future__ import annotations


from flask import Blueprint, jsonify, request

from config import logger
from routes_index import _extract_options, _extract_photo_ids
import service_chroma as chroma_service
import service_style_engine as style_engine
from service_style_engine import CONFIDENCE_LOW
from routes_edit import _persist_edit_recipe, _success_payload

style_edit_bp = Blueprint("style_edit", __name__)


def _get_clip_embedding(photo_id: str):
    """Re-use the CLIP embedding already stored in ChromaDB for this photo."""
    try:
        existing = chroma_service.get_image(photo_id)
        if existing and existing.get("ids") and existing.get("embeddings"):
            import numpy as np

            raw_emb = existing["embeddings"][0]
            if raw_emb is not None:
                emb_arr = np.asarray(raw_emb, dtype=np.float32)
                if emb_arr.size > 0 and not np.allclose(emb_arr, 0.0):
                    return emb_arr.tolist()
    except Exception as exc:
        logger.debug("Could not retrieve CLIP embedding for %s: %s", photo_id, exc)
    return None


@style_edit_bp.route("/style_edit", methods=["POST"])
def style_edit():
    """Generate a style-matched edit recipe.

    Multipart/form-data fields:
        image           (file, JPEG/PNG preview — required)
        photo_id        (str — required)
        use_llm_fallback (bool string "true"/"false" — default: "false")
        focal_length    (number, mm — optional for better matching)
        capture_time    (float, unix timestamp — optional for time-of-day bucket)

    Standard options passed through ``_extract_options``:
        provider, model, api_key, language, temperature, etc.
    """
    logger.info("Style edit request received")

    images = request.files.getlist("image")
    photo_ids = _extract_photo_ids(request.form)
    options = _extract_options(request.form)

    if not images or not photo_ids or len(images) != len(photo_ids):
        return jsonify(
            {
                "error": "Mismatch between number of images and photo IDs, or no images provided"
            }
        ), 400
    if len(images) != 1:
        return jsonify(
            {
                "error": "The /style_edit endpoint currently supports exactly one photo per request"
            }
        ), 400

    file = images[0]
    photo_id = photo_ids[0]
    if not file or not photo_id:
        return jsonify({"error": "Missing file or photo_id"}), 400

    image_bytes = file.read()
    use_llm_fallback = request.form.get("use_llm_fallback", "false").lower() in (
        "1",
        "true",
        "yes",
    )

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

    # Re-use existing CLIP embedding from index (best-effort)
    clip_embedding = _get_clip_embedding(photo_id)

    # -------------------------------------------------------------------
    # Run the style engine
    # -------------------------------------------------------------------
    result = style_engine.generate_style_edit(
        photo_id=photo_id,
        image_bytes=image_bytes,
        focal_length=focal_length,
        capture_time_unix=capture_time_unix,
        clip_embedding=clip_embedding,
        min_confidence=CONFIDENCE_LOW,
    )

    # -------------------------------------------------------------------
    # LLM fallback when style engine couldn't produce a confident result
    # -------------------------------------------------------------------
    if (
        result.engine == "none" or result.confidence < CONFIDENCE_LOW
    ) and use_llm_fallback:
        logger.info(
            "Style engine confidence %.3f below threshold for photo_id=%s, falling back to LLM",
            result.confidence,
            photo_id,
        )
        from service_metadata import get_analysis_service
        import service_training as training_service

        # Inject training examples as few-shot context in the LLM request
        if clip_embedding is not None:
            training_examples = training_service.query_similar_training_examples(
                clip_embedding, n_results=3
            )
        else:
            training_examples = []
        options["use_training_style"] = False  # already handled above
        options["_injected_training_examples"] = training_examples

        analysis_service = get_analysis_service()
        llm_response = analysis_service.generate_edit_recipe_single(
            photo_id, image_bytes, options
        )

        if not llm_response.success or not llm_response.recipe:
            return jsonify(
                {
                    "status": "error",
                    "engine": "llm",
                    "error": llm_response.error or "LLM edit generation failed",
                }
            ), 500

        _persist_edit_recipe(photo_id, file.filename, llm_response.recipe, options)
        payload = _success_payload(
            photo_id, llm_response.recipe, options, warning=llm_response.warning
        )
        payload["engine"] = "llm"
        payload["confidence"] = round(result.confidence, 3)
        payload["matched_examples"] = result.matched_count
        payload["style_engine_note"] = result.warning
        payload["input_tokens"] = llm_response.input_tokens
        payload["output_tokens"] = llm_response.output_tokens
        return jsonify(payload), 200

    # -------------------------------------------------------------------
    # Style engine had no result and fallback disabled — return error
    # -------------------------------------------------------------------
    if result.engine == "none":
        return jsonify(
            {
                "status": "error",
                "engine": "none",
                "confidence": 0.0,
                "matched_examples": 0,
                "error": result.warning or "Style engine could not produce a result.",
            }
        ), 422  # Unprocessable – not a server error, just insufficient data

    # -------------------------------------------------------------------
    # Successful style engine result
    # -------------------------------------------------------------------
    if not result.recipe:
        return jsonify(
            {
                "status": "error",
                "engine": "style",
                "confidence": round(result.confidence, 3),
                "matched_examples": result.matched_count,
                "error": "Style engine returned an empty recipe.",
            }
        ), 500

    _persist_edit_recipe(photo_id, file.filename, result.recipe, options)

    payload = _success_payload(photo_id, result.recipe, options, warning=result.warning)
    payload["engine"] = "style"
    payload["confidence"] = round(result.confidence, 3)
    payload["matched_examples"] = result.matched_count
    payload["matched_filenames"] = result.matched_filenames
    return jsonify(payload), 200
