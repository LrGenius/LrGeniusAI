"""Tests for _parse_grouping_params in routes/search.py.

Uses the real Flask app from geniusai_server (same pattern as test_routes_db.py)
so that Blueprints are registered and route-level imports work correctly.
"""

import pytest

from geniusai_server import app


@pytest.fixture
def client():
    app.config["TESTING"] = True
    with app.test_client() as c:
        yield c


# Import the helper directly after the app is available.
from routes.search import _parse_grouping_params  # noqa: E402


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

_VALID_PHOTO_IDS = ["photo-1", "photo-2", "photo-3"]


def _valid_data(**overrides):
    base = {
        "photo_ids": _VALID_PHOTO_IDS,
        "phash_threshold": "auto",
        "clip_threshold": "auto",
        "time_delta_seconds": 2,
        "culling_preset": "default",
    }
    base.update(overrides)
    return base


# ---------------------------------------------------------------------------
# Success paths
# ---------------------------------------------------------------------------


class TestParseGroupingParamsSuccess:
    def test_valid_data_returns_params_dict(self):
        params, err_resp, err_code = _parse_grouping_params(_valid_data())
        assert err_resp is None
        assert err_code is None
        assert params["photo_ids"] == _VALID_PHOTO_IDS

    def test_phash_threshold_auto_kept_as_string(self):
        params, _, _ = _parse_grouping_params(_valid_data(phash_threshold="auto"))
        assert params["phash_threshold"] == "auto"

    def test_phash_threshold_numeric_string_parsed(self):
        params, _, _ = _parse_grouping_params(_valid_data(phash_threshold="8"))
        assert params["phash_threshold"] == 8

    def test_phash_threshold_int_kept(self):
        params, _, _ = _parse_grouping_params(_valid_data(phash_threshold=5))
        assert params["phash_threshold"] == 5

    def test_clip_threshold_auto_kept_as_string(self):
        params, _, _ = _parse_grouping_params(_valid_data(clip_threshold="auto"))
        assert params["clip_threshold"] == "auto"

    def test_clip_threshold_numeric_string_parsed_as_float(self):
        params, _, _ = _parse_grouping_params(_valid_data(clip_threshold="0.85"))
        assert params["clip_threshold"] == pytest.approx(0.85)

    def test_time_delta_defaults_to_1(self):
        data = _valid_data()
        del data["time_delta_seconds"]
        params, _, _ = _parse_grouping_params(data)
        assert params["time_delta_seconds"] == 1

    def test_time_delta_string_parsed_as_int(self):
        params, _, _ = _parse_grouping_params(_valid_data(time_delta_seconds="10"))
        assert params["time_delta_seconds"] == 10

    def test_culling_preset_default_accepted(self):
        params, _, _ = _parse_grouping_params(_valid_data(culling_preset="default"))
        assert params["culling_preset"] == "default"

    def test_culling_preset_normalised_to_lowercase(self):
        params, _, _ = _parse_grouping_params(_valid_data(culling_preset="DEFAULT"))
        assert params["culling_preset"] == "default"

    def test_result_contains_all_expected_keys(self):
        params, _, _ = _parse_grouping_params(_valid_data())
        assert set(params.keys()) == {
            "photo_ids",
            "phash_threshold",
            "clip_threshold",
            "time_delta_seconds",
            "culling_preset",
        }

    def test_uuids_key_accepted_as_alias_for_photo_ids(self):
        data = {
            "uuids": _VALID_PHOTO_IDS,
            "culling_preset": "default",
        }
        params, err_resp, _ = _parse_grouping_params(data)
        assert err_resp is None
        assert params["photo_ids"] == _VALID_PHOTO_IDS


# ---------------------------------------------------------------------------
# Error paths — each must return (None, response, 400)
# ---------------------------------------------------------------------------


class TestParseGroupingParamsErrors:
    def _assert_error(self, data, fragment):
        """Assert the helper returns an error response containing *fragment*.

        _parse_grouping_params calls Flask's jsonify() internally, so the call
        must happen inside an app context.
        """
        with app.app_context():
            params, err_resp, err_code = _parse_grouping_params(data)
            assert params is None
            assert err_code == 400
            payload = err_resp.get_json()
        assert "error" in payload
        assert fragment.lower() in payload["error"].lower()

    def test_phash_threshold_bad_string_returns_error(self):
        self._assert_error(_valid_data(phash_threshold="bad"), "phash_threshold")

    def test_clip_threshold_bad_string_returns_error(self):
        self._assert_error(_valid_data(clip_threshold="bad"), "clip_threshold")

    def test_time_delta_bad_string_returns_error(self):
        self._assert_error(_valid_data(time_delta_seconds="bad"), "time_delta_seconds")

    def test_culling_preset_invalid_returns_error(self):
        self._assert_error(
            _valid_data(culling_preset="invalid_preset"), "culling_preset"
        )

    def test_missing_photo_ids_returns_error(self):
        data = _valid_data()
        del data["photo_ids"]
        self._assert_error(data, "photo_ids")

    def test_photo_ids_not_a_list_returns_error(self):
        self._assert_error(_valid_data(photo_ids="not-a-list"), "photo_ids")

    def test_photo_ids_empty_list_returns_error(self):
        self._assert_error(_valid_data(photo_ids=[]), "photo_ids")

    def test_culling_preset_error_includes_available_presets(self):
        with app.app_context():
            _, err_resp, _ = _parse_grouping_params(_valid_data(culling_preset="nope"))
            payload = err_resp.get_json()
        assert "available_presets" in payload
        assert isinstance(payload["available_presets"], list)
        assert "default" in payload["available_presets"]
