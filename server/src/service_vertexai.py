"""
Google Vertex AI Multimodal Embeddings for images and text.
Used optionally in parallel to SigLIP2 embeddings; stored in a separate ChromaDB collection.
Config: vertex_project_id and vertex_location from Lightroom plugin or env vars.
"""

import base64
import os
from typing import List, Optional

from config import logger

# Cache: (project, location) -> (PredictionServiceClient, endpoint_path)
_vertex_client_cache: dict = {}
# Last config from an index request; used by search when no explicit config is passed
_last_vertex_config: Optional[tuple] = None  # (project, location) or None
VERTEX_EMBEDDING_DIM = 1408
_VERTEX_MODEL_NAME = "multimodalembedding@001"


def _detect_mime_type(image_bytes: bytes) -> str:
    """Best-effort MIME detection for common image formats supported by the endpoint."""
    if image_bytes.startswith(b"\x89PNG\r\n\x1a\n"):
        return "image/png"
    if image_bytes.startswith(b"\xff\xd8\xff"):
        return "image/jpeg"
    # Most Lightroom-exported previews are JPEG; use as safe default.
    return "image/jpeg"


def _resolve_config(vertex_project_id=None, vertex_location=None):
    """Resolve project and location from options or environment."""
    project = (
        (vertex_project_id or "").strip()
        or os.environ.get("GOOGLE_CLOUD_PROJECT")
        or os.environ.get("VERTEX_PROJECT_ID")
    )
    location = (vertex_location or "").strip() or os.environ.get(
        "VERTEX_LOCATION", "us-central1"
    )
    return project, location


def _get_vertex_client_and_endpoint(vertex_project_id=None, vertex_location=None):
    """Lazy-load Vertex PredictionServiceClient + model endpoint path."""
    global _last_vertex_config, _vertex_client_cache
    project, location = _resolve_config(vertex_project_id, vertex_location)
    if not project and _last_vertex_config:
        project, location = _last_vertex_config
    if not project:
        logger.warning(
            "Vertex AI: Project ID not set (plugin preferences or GOOGLE_CLOUD_PROJECT); Vertex embeddings disabled."
        )
        return None
    key = (project, location)
    if key in _vertex_client_cache:
        return _vertex_client_cache[key]
    try:
        from google.cloud import aiplatform_v1

        # Set quota project for local ADC (gcloud auth application-default login)
        # Required by aiplatform API when using user credentials
        if "GOOGLE_CLOUD_QUOTA_PROJECT" not in os.environ:
            os.environ["GOOGLE_CLOUD_QUOTA_PROJECT"] = project

        api_endpoint = f"{location}-aiplatform.googleapis.com"
        client = aiplatform_v1.PredictionServiceClient(
            client_options={"api_endpoint": api_endpoint}
        )
        endpoint = f"projects/{project}/locations/{location}/publishers/google/models/{_VERTEX_MODEL_NAME}"
        _vertex_client_cache[key] = (client, endpoint)
        _last_vertex_config = (project, location)
        logger.info(
            "Vertex AI PredictionService initialized (project=%s, location=%s, model=%s).",
            project,
            location,
            _VERTEX_MODEL_NAME,
        )
        return client, endpoint
    except Exception as e:
        logger.warning("Vertex AI not available: %s", e, exc_info=True)
        return None


def _to_plain_python(value):
    """Best-effort conversion for proto-plus / protobuf values to native Python types."""
    if value is None or isinstance(value, (str, int, float, bool)):
        return value
    if isinstance(value, dict):
        return {k: _to_plain_python(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [_to_plain_python(v) for v in value]

    # google.protobuf.struct_pb2.Value
    if hasattr(value, "WhichOneof"):
        try:
            kind = value.WhichOneof("kind")
            if kind == "null_value":
                return None
            if kind == "number_value":
                return float(value.number_value)
            if kind == "string_value":
                return value.string_value
            if kind == "bool_value":
                return bool(value.bool_value)
            if kind == "struct_value":
                return {
                    k: _to_plain_python(v) for k, v in value.struct_value.fields.items()
                }
            if kind == "list_value":
                return [_to_plain_python(v) for v in value.list_value.values]
        except Exception:
            pass

    # Generic protobuf message
    if hasattr(value, "DESCRIPTOR"):
        try:
            from google.protobuf import json_format

            return json_format.MessageToDict(value)
        except Exception:
            pass

    # proto-plus map/list composites
    if hasattr(value, "items"):
        try:
            return {k: _to_plain_python(v) for k, v in value.items()}
        except Exception:
            pass
    try:
        return [_to_plain_python(v) for v in value]
    except Exception:
        return value


def _extract_embedding(prediction_value, field_name: str) -> Optional[List[float]]:
    """Extract embedding list from prediction payload across response container types."""
    payload = _to_plain_python(prediction_value)
    if not isinstance(payload, dict):
        return None

    embedding = payload.get(field_name)
    if isinstance(embedding, dict) and "values" in embedding:
        embedding = embedding["values"]
    if not isinstance(embedding, list) or not embedding:
        return None
    try:
        return [float(v) for v in embedding]
    except Exception:
        return None


def is_available(vertex_project_id=None, vertex_location=None) -> bool:
    """Return True if Vertex AI embeddings can be used (project configured and model loadable)."""
    return (
        _get_vertex_client_and_endpoint(vertex_project_id, vertex_location) is not None
    )


def get_image_embeddings(
    image_bytes_list: List[bytes], vertex_project_id=None, vertex_location=None
) -> List[Optional[List[float]]]:
    """
    Generate Vertex AI image embeddings for a list of images.
    One request per image (API limit). Returns one embedding per input; None on failure.
    """
    client_and_endpoint = _get_vertex_client_and_endpoint(
        vertex_project_id, vertex_location
    )
    if client_and_endpoint is None:
        return [None] * len(image_bytes_list)
    client, endpoint = client_and_endpoint
    from google.protobuf import struct_pb2

    results: List[Optional[List[float]]] = []
    for i, img_bytes in enumerate(image_bytes_list):
        try:
            image_obj = {
                "bytesBase64Encoded": base64.b64encode(img_bytes).decode("ascii"),
                "mimeType": _detect_mime_type(img_bytes),
            }
            instance = struct_pb2.Value(
                struct_value=struct_pb2.Struct(
                    fields={
                        "image": struct_pb2.Value(
                            struct_value=struct_pb2.Struct(
                                fields={
                                    "bytesBase64Encoded": struct_pb2.Value(
                                        string_value=image_obj["bytesBase64Encoded"]
                                    ),
                                    "mimeType": struct_pb2.Value(
                                        string_value=image_obj["mimeType"]
                                    ),
                                }
                            )
                        )
                    }
                )
            )
            parameters = struct_pb2.Value(
                struct_value=struct_pb2.Struct(
                    fields={
                        "dimension": struct_pb2.Value(
                            number_value=float(VERTEX_EMBEDDING_DIM)
                        )
                    }
                )
            )

            response = client.predict(
                endpoint=endpoint,
                instances=[instance],
                parameters=parameters,
            )
            if response.predictions:
                image_embedding = _extract_embedding(
                    response.predictions[0], "imageEmbedding"
                )
            else:
                image_embedding = None

            if image_embedding:
                # Normalize for cosine similarity (Chroma uses L2 distance)
                import numpy as np

                vec = np.array(image_embedding, dtype=np.float32)
                norm = np.linalg.norm(vec)
                if norm > 1e-6:
                    vec = vec / norm
                results.append(vec.tolist())
            else:
                results.append(None)
        except Exception as e:
            logger.warning(
                "Vertex embedding failed for image %s: %s", i, e, exc_info=True
            )
            results.append(None)
    return results


def get_text_embedding(
    text: str, vertex_project_id=None, vertex_location=None
) -> Optional[List[float]]:
    """
    Generate Vertex AI text embedding for a search query.
    Used when searching with the Vertex collection.
    """
    client_and_endpoint = _get_vertex_client_and_endpoint(
        vertex_project_id, vertex_location
    )
    if client_and_endpoint is None or not text or not text.strip():
        return None
    client, endpoint = client_and_endpoint
    from google.protobuf import struct_pb2

    try:
        instance = struct_pb2.Value(
            struct_value=struct_pb2.Struct(
                fields={"text": struct_pb2.Value(string_value=text.strip())}
            )
        )
        parameters = struct_pb2.Value(
            struct_value=struct_pb2.Struct(
                fields={
                    "dimension": struct_pb2.Value(
                        number_value=float(VERTEX_EMBEDDING_DIM)
                    )
                }
            )
        )
        response = client.predict(
            endpoint=endpoint,
            instances=[instance],
            parameters=parameters,
        )
        if response.predictions:
            text_embedding = _extract_embedding(
                response.predictions[0], "textEmbedding"
            )
        else:
            text_embedding = None
        if text_embedding:
            import numpy as np

            vec = np.array(text_embedding, dtype=np.float32)
            norm = np.linalg.norm(vec)
            if norm > 1e-6:
                vec = vec / norm
            return vec.tolist()
    except Exception as e:
        logger.warning("Vertex text embedding failed: %s", e, exc_info=True)
    return None
