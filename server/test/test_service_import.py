"""Tests for services/import_.py — import_metadata_task control flow."""

import json
from unittest.mock import MagicMock, patch


from services.import_ import import_metadata_task


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _make_chroma_mock(existing_ids=None):
    """Return a mock chroma_service.

    existing_ids: list of photo_ids that are considered "in DB".
    If None, get_image always returns an empty result (not in DB).
    """
    mock = MagicMock()
    if existing_ids is None:
        mock.get_image.return_value = {"ids": []}
    else:

        def _get_image(photo_id):
            if photo_id in existing_ids:
                return {"ids": [photo_id], "metadatas": [{}]}
            return {"ids": []}

        mock.get_image.side_effect = _get_image
    return mock


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


class TestImportMetadataTaskMissingPhotoId:
    def test_item_missing_photo_id_increments_failure(self):
        mock_chroma = _make_chroma_mock()
        with patch("services.import_.chroma_service", mock_chroma):
            success, failure = import_metadata_task([{"title": "no id here"}])
        assert success == 0
        assert failure == 1
        mock_chroma.add_image.assert_not_called()

    def test_item_with_uuid_used_as_photo_id_when_no_photo_id(self):
        mock_chroma = _make_chroma_mock()
        with patch("services.import_.chroma_service", mock_chroma):
            success, failure = import_metadata_task(
                [{"uuid": "uuid-only", "title": "A Title"}]
            )
        assert success == 1
        assert failure == 0
        mock_chroma.add_image.assert_called_once()

    def test_empty_photo_id_string_treated_as_missing(self):
        mock_chroma = _make_chroma_mock()
        with patch("services.import_.chroma_service", mock_chroma):
            success, failure = import_metadata_task([{"photo_id": "", "title": "x"}])
        assert success == 0
        assert failure == 1


class TestImportMetadataTaskNoMetadataFields:
    def test_item_with_no_metadata_fields_is_skipped(self):
        mock_chroma = _make_chroma_mock()
        with patch("services.import_.chroma_service", mock_chroma):
            success, failure = import_metadata_task([{"photo_id": "p1"}])
        assert success == 0
        assert failure == 1
        mock_chroma.add_image.assert_not_called()
        mock_chroma.update_image.assert_not_called()

    def test_empty_keywords_list_counts_as_no_metadata(self):
        mock_chroma = _make_chroma_mock()
        with patch("services.import_.chroma_service", mock_chroma):
            success, failure = import_metadata_task(
                [{"photo_id": "p1", "keywords": []}]
            )
        assert success == 0
        assert failure == 1

    def test_empty_title_string_counts_as_no_metadata(self):
        mock_chroma = _make_chroma_mock()
        with patch("services.import_.chroma_service", mock_chroma):
            success, failure = import_metadata_task([{"photo_id": "p1", "title": ""}])
        assert success == 0
        assert failure == 1

    def test_empty_caption_string_counts_as_no_metadata(self):
        mock_chroma = _make_chroma_mock()
        with patch("services.import_.chroma_service", mock_chroma):
            success, failure = import_metadata_task([{"photo_id": "p1", "caption": ""}])
        assert success == 0
        assert failure == 1


class TestImportMetadataTaskAddNewRecord:
    def test_item_not_in_db_calls_add_image(self):
        mock_chroma = _make_chroma_mock()
        with patch("services.import_.chroma_service", mock_chroma):
            success, failure = import_metadata_task(
                [{"photo_id": "new-1", "title": "My Photo"}]
            )
        assert success == 1
        assert failure == 0
        mock_chroma.add_image.assert_called_once()
        mock_chroma.update_image.assert_not_called()

    def test_add_image_called_with_photo_id_and_none_embedding(self):
        mock_chroma = _make_chroma_mock()
        with patch("services.import_.chroma_service", mock_chroma):
            import_metadata_task([{"photo_id": "new-2", "caption": "A caption"}])
        args = mock_chroma.add_image.call_args
        assert args[0][0] == "new-2"
        assert args[0][1] is None  # embedding is None for metadata-only

    def test_add_image_metadata_includes_photo_id(self):
        mock_chroma = _make_chroma_mock()
        with patch("services.import_.chroma_service", mock_chroma):
            import_metadata_task([{"photo_id": "new-3", "title": "T"}])
        metadata_arg = mock_chroma.add_image.call_args[0][2]
        assert metadata_arg["photo_id"] == "new-3"

    def test_keywords_serialised_to_json(self):
        mock_chroma = _make_chroma_mock()
        kw = ["Nature", "Travel"]
        with patch("services.import_.chroma_service", mock_chroma):
            import_metadata_task([{"photo_id": "p1", "keywords": kw}])
        metadata_arg = mock_chroma.add_image.call_args[0][2]
        assert json.loads(metadata_arg["keywords"]) == kw

    def test_flattened_keywords_added(self):
        mock_chroma = _make_chroma_mock()
        with patch("services.import_.chroma_service", mock_chroma):
            import_metadata_task([{"photo_id": "p1", "keywords": ["A", "B"]}])
        metadata_arg = mock_chroma.add_image.call_args[0][2]
        assert "flattened_keywords" in metadata_arg

    def test_catalog_id_passed_to_add_image(self):
        mock_chroma = _make_chroma_mock()
        with patch("services.import_.chroma_service", mock_chroma):
            import_metadata_task([{"photo_id": "p1", "title": "T"}], catalog_id="cat-1")
        kwargs = mock_chroma.add_image.call_args[1]
        assert kwargs.get("catalog_id") == "cat-1"


class TestImportMetadataTaskUpdateExistingRecord:
    def test_item_in_db_calls_update_image(self):
        mock_chroma = _make_chroma_mock(existing_ids=["existing-1"])
        with patch("services.import_.chroma_service", mock_chroma):
            success, failure = import_metadata_task(
                [{"photo_id": "existing-1", "title": "Updated"}]
            )
        assert success == 1
        assert failure == 0
        mock_chroma.update_image.assert_called_once()
        mock_chroma.add_image.assert_not_called()

    def test_update_image_called_with_correct_photo_id(self):
        mock_chroma = _make_chroma_mock(existing_ids=["ex-2"])
        with patch("services.import_.chroma_service", mock_chroma):
            import_metadata_task([{"photo_id": "ex-2", "caption": "Cap"}])
        args = mock_chroma.update_image.call_args[0]
        assert args[0] == "ex-2"

    def test_catalog_id_passed_to_update_image(self):
        mock_chroma = _make_chroma_mock(existing_ids=["ex-3"])
        with patch("services.import_.chroma_service", mock_chroma):
            import_metadata_task(
                [{"photo_id": "ex-3", "title": "T"}], catalog_id="cat-9"
            )
        kwargs = mock_chroma.update_image.call_args[1]
        assert kwargs.get("catalog_id") == "cat-9"


class TestImportMetadataTaskExceptionHandling:
    def test_exception_during_get_image_increments_failure(self):
        mock_chroma = MagicMock()
        mock_chroma.get_image.side_effect = RuntimeError("DB error")
        with patch("services.import_.chroma_service", mock_chroma):
            success, failure = import_metadata_task([{"photo_id": "p1", "title": "T"}])
        assert success == 0
        assert failure == 1

    def test_exception_during_update_increments_failure(self):
        mock_chroma = _make_chroma_mock(existing_ids=["p2"])
        mock_chroma.update_image.side_effect = RuntimeError("write error")
        with patch("services.import_.chroma_service", mock_chroma):
            success, failure = import_metadata_task([{"photo_id": "p2", "title": "T"}])
        assert success == 0
        assert failure == 1

    def test_exception_during_add_increments_failure(self):
        mock_chroma = _make_chroma_mock()
        mock_chroma.add_image.side_effect = RuntimeError("add failed")
        with patch("services.import_.chroma_service", mock_chroma):
            success, failure = import_metadata_task([{"photo_id": "p3", "title": "T"}])
        assert success == 0
        assert failure == 1


class TestImportMetadataTaskMultipleItems:
    def test_multiple_items_counted_correctly(self):
        mock_chroma = _make_chroma_mock(existing_ids=["existing"])
        with patch("services.import_.chroma_service", mock_chroma):
            success, failure = import_metadata_task(
                [
                    {"photo_id": "new-a", "title": "New A"},
                    {"photo_id": "existing", "caption": "Cap"},
                    {"photo_id": "new-b", "alt_text": "Alt"},
                    {"title": "no id"},  # failure
                ]
            )
        assert success == 3
        assert failure == 1

    def test_empty_list_returns_zero_zero(self):
        mock_chroma = _make_chroma_mock()
        with patch("services.import_.chroma_service", mock_chroma):
            success, failure = import_metadata_task([])
        assert success == 0
        assert failure == 0

    def test_all_metadata_fields_accepted(self):
        """title, caption, alt_text, and keywords are all valid metadata fields."""
        mock_chroma = _make_chroma_mock()
        with patch("services.import_.chroma_service", mock_chroma):
            success, failure = import_metadata_task(
                [
                    {
                        "photo_id": "full",
                        "title": "T",
                        "caption": "C",
                        "alt_text": "A",
                        "keywords": ["k1", "k2"],
                    }
                ]
            )
        assert success == 1
        assert failure == 0
        metadata_arg = mock_chroma.add_image.call_args[0][2]
        assert metadata_arg["title"] == "T"
        assert metadata_arg["caption"] == "C"
        assert metadata_arg["alt_text"] == "A"
