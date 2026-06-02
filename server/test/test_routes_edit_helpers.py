"""Tests for the pure helper functions in routes/edit.py."""

from routes.edit import _has_items, _success_payload


class TestHasItems:
    def test_none_returns_false(self):
        assert _has_items(None) is False

    def test_empty_list_returns_false(self):
        assert _has_items([]) is False

    def test_non_empty_list_returns_true(self):
        assert _has_items([1, 2]) is True

    def test_empty_dict_returns_false(self):
        assert _has_items({}) is False

    def test_non_empty_dict_returns_true(self):
        assert _has_items({"a": 1}) is True

    def test_object_without_len_returns_false(self):
        # Objects that raise TypeError on len() → False
        assert _has_items(object()) is False

    def test_string_with_content_returns_true(self):
        assert _has_items("abc") is True

    def test_empty_string_returns_false(self):
        assert _has_items("") is False


class TestSuccessPayload:
    def _make_recipe(self, **extra):
        base = {"summary": "Boost exposure", "warnings": ["minor clipping"]}
        base.update(extra)
        return base

    def test_contains_required_fields(self):
        recipe = self._make_recipe()
        options = {"model": "gpt-4o", "provider": "chatgpt"}
        payload = _success_payload("photo_123", recipe, options)

        assert payload["status"] == "success"
        assert payload["photo_id"] == "photo_123"
        assert payload["uuid"] == "photo_123"
        assert payload["edit"] is recipe
        assert payload["edit_summary"] == "Boost exposure"
        assert payload["edit_warnings"] == ["minor clipping"]
        assert payload["edit_model"] == "gpt-4o"

    def test_warning_included_when_provided(self):
        recipe = self._make_recipe()
        payload = _success_payload("p1", recipe, {}, warning="model degraded")
        assert payload["warning"] == "model degraded"

    def test_no_warning_key_when_not_provided(self):
        recipe = self._make_recipe()
        payload = _success_payload("p1", recipe, {})
        assert "warning" not in payload

    def test_edit_rundate_is_present(self):
        payload = _success_payload("p1", self._make_recipe(), {})
        assert "edit_rundate" in payload
        assert payload["edit_rundate"] != ""

    def test_missing_summary_defaults_to_empty(self):
        recipe = {"warnings": []}
        payload = _success_payload("p1", recipe, {})
        assert payload["edit_summary"] == ""

    def test_missing_warnings_defaults_to_empty(self):
        recipe = {"summary": "ok"}
        payload = _success_payload("p1", recipe, {})
        assert payload["edit_warnings"] == []
