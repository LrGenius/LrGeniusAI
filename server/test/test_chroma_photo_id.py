"""Tests for pure helpers in services/chroma.py (no ChromaDB required).

Covers photo-id normalization, catalog id (de)serialization, metadata
shaping, and the result-extraction helper.
"""

import json

import numpy as np

from services.chroma import (
    _ensure_photo_metadata,
    _first_result_item,
    _normalize_photo_id,
    _parse_catalog_ids,
    _serialize_catalog_ids,
    CATALOG_IDS_FIELD,
    LEGACY_UUID_FIELD,
    PHOTO_ID_FIELD,
)


class TestNormalizePhotoId:
    def test_returns_photo_id_when_present(self):
        assert _normalize_photo_id(photo_id="abc") == "abc"

    def test_strips_whitespace(self):
        assert _normalize_photo_id(photo_id="  abc  ") == "abc"

    def test_falls_back_to_legacy_uuid(self):
        assert _normalize_photo_id(legacy_uuid="legacy-1") == "legacy-1"

    def test_returns_none_when_both_missing(self):
        assert _normalize_photo_id() is None

    def test_empty_string_normalizes_to_none(self):
        assert _normalize_photo_id(photo_id="   ") is None


class TestCatalogIds:
    def test_parse_returns_empty_for_missing_metadata(self):
        assert _parse_catalog_ids({}) == set()
        assert _parse_catalog_ids(None) == set()

    def test_parse_handles_json_string(self):
        meta = {CATALOG_IDS_FIELD: json.dumps(["cat-1", "cat-2"])}
        assert _parse_catalog_ids(meta) == {"cat-1", "cat-2"}

    def test_parse_handles_list_directly(self):
        meta = {CATALOG_IDS_FIELD: ["cat-1", "cat-2"]}
        assert _parse_catalog_ids(meta) == {"cat-1", "cat-2"}

    def test_parse_invalid_json_returns_empty(self):
        meta = {CATALOG_IDS_FIELD: "{not valid json"}
        assert _parse_catalog_ids(meta) == set()

    def test_serialize_produces_sorted_json_list(self):
        result = _serialize_catalog_ids({"b", "a", "c"})
        assert json.loads(result) == ["a", "b", "c"]

    def test_serialize_empty_set_returns_bracket_pair(self):
        assert _serialize_catalog_ids(set()) == "[]"

    def test_round_trip(self):
        original = {"x", "y", "z"}
        meta = {CATALOG_IDS_FIELD: _serialize_catalog_ids(original)}
        assert _parse_catalog_ids(meta) == original


class TestEnsurePhotoMetadata:
    def test_adds_photo_id_field(self):
        out = _ensure_photo_metadata("pid-1", {})
        assert out[PHOTO_ID_FIELD] == "pid-1"

    def test_defaults_legacy_uuid_to_photo_id(self):
        out = _ensure_photo_metadata("pid-1", {})
        assert out[LEGACY_UUID_FIELD] == "pid-1"

    def test_uses_explicit_legacy_uuid_when_given(self):
        out = _ensure_photo_metadata("pid-1", {}, legacy_uuid="old-uuid")
        assert out[LEGACY_UUID_FIELD] == "old-uuid"

    def test_preserves_existing_legacy_uuid(self):
        out = _ensure_photo_metadata("pid-1", {LEGACY_UUID_FIELD: "kept"})
        assert out[LEGACY_UUID_FIELD] == "kept"

    def test_does_not_mutate_input(self):
        original = {"key": "value"}
        out = _ensure_photo_metadata("pid", original)
        assert original == {"key": "value"}
        assert out is not original


class TestFirstResultItem:
    def test_none_returns_default(self):
        assert _first_result_item(None) is None
        assert _first_result_item(None, default="x") == "x"

    def test_empty_list_returns_default(self):
        assert _first_result_item([], default="x") == "x"

    def test_returns_first_of_list(self):
        assert _first_result_item([1, 2, 3]) == 1

    def test_handles_numpy_array(self):
        arr = np.array([10, 20, 30])
        assert _first_result_item(arr) == 10

    def test_empty_numpy_array_returns_default(self):
        arr = np.array([])
        assert _first_result_item(arr, default="d") == "d"
