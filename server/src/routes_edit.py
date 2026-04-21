from datetime import datetime
import base64
import json

from flask import Blueprint, jsonify, request

from config import logger
from routes_index import _extract_options, _extract_photo_ids
import service_chroma as chroma_service
from service_metadata import get_analysis_service


edit_bp = Blueprint("edit", __name__)


def _has_items(values) -> bool:
    if values is None:
        return False
    try:
        return len(values) > 0
    except TypeError:
        return False


def _persist_edit_recipe(
    photo_id: str, filename: str | None, recipe: dict, options: dict
) -> None:
    catalog_id = options.get("catalog_id")
    existing = chroma_service.get_image(photo_id)
    existing_has_ids = existing is not None and _has_items(existing.get("ids"))
    existing_has_metadatas = existing is not None and _has_items(
        existing.get("metadatas")
    )
    existing_has_embeddings = existing is not None and _has_items(
        existing.get("embeddings")
    )

    existing_meta = (
        dict(existing["metadatas"][0])
        if existing_has_ids and existing_has_metadatas
        else {}
    )
    existing_embedding = None
    if existing_has_ids and existing_has_embeddings:
        try:
            existing_embedding = existing["embeddings"][0]
        except (IndexError, KeyError, TypeError):
            existing_embedding = None

    metadata = existing_meta.copy()
    if filename:
        metadata["filename"] = filename
    metadata["edit_recipe"] = json.dumps(recipe, ensure_ascii=False)
    metadata["edit_summary"] = recipe.get("summary", "")
    metadata["edit_warnings"] = json.dumps(
        recipe.get("warnings", []), ensure_ascii=False
    )
    metadata["edit_model"] = (
        options.get("model") or metadata.get("edit_model") or metadata.get("model")
    )
    if options.get("provider"):
        metadata["edit_provider"] = options["provider"]
    metadata["edit_run_date"] = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    metadata.setdefault("provider", options.get("provider"))
    metadata.setdefault("model", options.get("model"))
    metadata.setdefault(
        "has_embedding", bool(existing_meta.get("has_embedding", False))
    )

    if existing_has_ids:
        chroma_service.update_image(
            photo_id, metadata, embedding=existing_embedding, catalog_id=catalog_id
        )
    else:
        chroma_service.add_image(
            photo_id, existing_embedding, metadata, catalog_id=catalog_id
        )


def _success_payload(
    photo_id: str, recipe: dict, options: dict, warning: str | None = None
) -> dict:
    now = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    payload = {
        "status": "success",
        "photo_id": photo_id,
        "uuid": photo_id,
        "edit": recipe,
        "edit_summary": recipe.get("summary", ""),
        "edit_warnings": recipe.get("warnings", []),
        "edit_model": options.get("model"),
        "edit_rundate": now,
    }
    if warning:
        payload["warning"] = warning
    return payload


@edit_bp.route("/edit", methods=["POST"])
def generate_edit_recipe():
    logger.info("Edit recipe request received")
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
                "error": "The /edit endpoint currently supports exactly one photo per request"
            }
        ), 400

    file = images[0]
    photo_id = photo_ids[0]
    if not file or not photo_id:
        return jsonify({"error": "Missing file or photo_id"}), 400

    analysis_service = get_analysis_service()
    response = analysis_service.generate_edit_recipe_single(
        photo_id, file.read(), options
    )
    if not response.success or not response.recipe:
        return jsonify(
            {"status": "error", "error": response.error or "Edit generation failed"}
        ), 500

    _persist_edit_recipe(photo_id, file.filename, response.recipe, options)
    payload = _success_payload(
        photo_id, response.recipe, options, warning=response.warning
    )
    payload["input_tokens"] = response.input_tokens
    payload["output_tokens"] = response.output_tokens
    return jsonify(payload), 200


@edit_bp.route("/edit_base64", methods=["POST"])
def generate_edit_recipe_base64():
    logger.info("Edit recipe base64 request received")
    data = request.get_json() or {}
    image_b64 = data.get("image")
    photo_id = data.get("photo_id") or data.get("uuid")
    filename = data.get("filename")

    if not image_b64 or not photo_id or not filename:
        return jsonify(
            {"error": "Missing required fields: image, photo_id, filename"}
        ), 400

    options = _extract_options(data)
    analysis_service = get_analysis_service()
    response = analysis_service.generate_edit_recipe_single(
        photo_id, base64.b64decode(image_b64.encode("ascii")), options
    )
    if not response.success or not response.recipe:
        return jsonify(
            {"status": "error", "error": response.error or "Edit generation failed"}
        ), 500

    _persist_edit_recipe(photo_id, filename, response.recipe, options)
    payload = _success_payload(
        photo_id, response.recipe, options, warning=response.warning
    )
    payload["input_tokens"] = response.input_tokens
    payload["output_tokens"] = response.output_tokens
    return jsonify(payload), 200
