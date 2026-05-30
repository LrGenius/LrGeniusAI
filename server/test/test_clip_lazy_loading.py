"""Regression tests for CLIP/SigLIP lazy loading in process_image_task.

Metadata-only (LLM) runs must succeed even when the CLIP model is absent or
disabled in the plugin settings. Previously process_image_task loaded the CLIP
model unconditionally, so a metadata-only request failed when the model could
not be loaded.
"""

import io
from unittest import mock

from PIL import Image

from providers.base import MetadataGenerationResponse
from services import index as index_service


def _jpeg_bytes() -> bytes:
    buf = io.BytesIO()
    Image.new("RGB", (16, 16), (123, 200, 50)).save(buf, format="JPEG")
    return buf.getvalue()


def _metadata_only_options():
    return {
        "provider": "test",
        "model": "test-model",
        "regenerate_metadata": True,
        "compute_embeddings": False,  # CLIP disabled / not ready
        "compute_metadata": True,
        "compute_faces": False,
        "compute_vertexai": False,
    }


def test_metadata_only_does_not_load_clip_model():
    """When no embeddings are requested, the CLIP model must not be loaded."""
    image_triplets = [(_jpeg_bytes(), "photo-1", "photo-1.jpg")]

    fake_analysis = mock.Mock()
    fake_analysis.analyze_batch.return_value = (
        None,
        [
            MetadataGenerationResponse(
                uuid="photo-1",
                success=True,
                title="A title",
                caption="A caption",
            )
        ],
    )

    with (
        mock.patch.object(
            index_service, "get_analysis_service", return_value=fake_analysis
        ),
        mock.patch.object(index_service, "server_lifecycle") as fake_lifecycle,
        mock.patch.object(index_service, "chroma_service") as fake_chroma,
        mock.patch.object(index_service, "exif_service") as fake_exif,
    ):
        fake_exif.extract_location_tags.return_value = None
        fake_chroma.get_image.return_value = None

        success, failure, errors = index_service.process_image_task(
            image_triplets, _metadata_only_options()
        )[:3]

    fake_lifecycle.get_model.assert_not_called()
    fake_lifecycle.get_processor.assert_not_called()
    assert success == 1
    assert failure == 0
    assert errors == []
    fake_chroma.add_image.assert_called_once()


def test_metadata_only_succeeds_even_if_clip_load_would_fail():
    """Even if get_model would raise, a metadata-only run still succeeds."""
    image_triplets = [(_jpeg_bytes(), "photo-1", "photo-1.jpg")]

    fake_analysis = mock.Mock()
    fake_analysis.analyze_batch.return_value = (
        None,
        [MetadataGenerationResponse(uuid="photo-1", success=True, title="A title")],
    )

    with (
        mock.patch.object(
            index_service, "get_analysis_service", return_value=fake_analysis
        ),
        mock.patch.object(index_service, "server_lifecycle") as fake_lifecycle,
        mock.patch.object(index_service, "chroma_service") as fake_chroma,
        mock.patch.object(index_service, "exif_service") as fake_exif,
    ):
        fake_lifecycle.get_model.side_effect = RuntimeError("model not downloaded")
        fake_exif.extract_location_tags.return_value = None
        fake_chroma.get_image.return_value = None

        success, failure, errors = index_service.process_image_task(
            image_triplets, _metadata_only_options()
        )[:3]

    assert success == 1
    assert failure == 0
    assert errors == []


def test_embeddings_run_still_loads_clip_model():
    """When embeddings are requested, the CLIP model is still loaded."""
    image_triplets = [(_jpeg_bytes(), "photo-1", "photo-1.jpg")]

    fake_analysis = mock.Mock()
    fake_analysis.analyze_batch.return_value = ([[0.1, 0.2, 0.3]], None)

    options = _metadata_only_options()
    options["compute_embeddings"] = True
    options["compute_metadata"] = False

    with (
        mock.patch.object(
            index_service, "get_analysis_service", return_value=fake_analysis
        ),
        mock.patch.object(index_service, "server_lifecycle") as fake_lifecycle,
        mock.patch.object(index_service, "chroma_service") as fake_chroma,
        mock.patch.object(index_service, "exif_service") as fake_exif,
    ):
        fake_lifecycle.get_model.return_value = mock.Mock()
        fake_lifecycle.get_processor.return_value = mock.Mock()
        fake_exif.extract_location_tags.return_value = None
        fake_chroma.get_image.return_value = None

        index_service.process_image_task(image_triplets, options)

    fake_lifecycle.get_model.assert_called_once()
    fake_lifecycle.get_processor.assert_called_once()
