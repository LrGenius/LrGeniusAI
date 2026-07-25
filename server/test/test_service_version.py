"""Tests for services/version.py — version normalisation, dev fallback, and
compatibility check logic."""

from services.version import (
    _is_default_dev_plugin,
    _is_dev_backend,
    _normalize_version,
    check_plugin_backend_version,
)


class TestNormalizeVersion:
    def test_none_returns_none(self):
        assert _normalize_version(None) is None

    def test_empty_string_returns_none(self):
        assert _normalize_version("") is None

    def test_non_string_returns_none(self):
        assert _normalize_version(123) is None  # type: ignore[arg-type]

    def test_simple_semver(self):
        assert _normalize_version("1.2.3") == "1.2.3"

    def test_v_prefix_stripped(self):
        assert _normalize_version("v1.2.3") == "1.2.3"

    def test_extra_prerelease_suffix_truncated(self):
        # Only the major.minor.patch part should be returned.
        assert _normalize_version("1.2.3-beta.1") == "1.2.3"

    def test_dev_suffix_truncated(self):
        assert _normalize_version("0.0.0-dev") == "0.0.0"

    def test_v_prefix_with_dev_suffix(self):
        assert _normalize_version("v0.0.0-dev") == "0.0.0"

    def test_whitespace_stripped(self):
        assert _normalize_version("  2.3.4  ") == "2.3.4"

    def test_garbage_input_returns_none(self):
        assert _normalize_version("not-a-version") is None

    def test_partial_version_returns_none(self):
        assert _normalize_version("1.2") is None

    def test_dev_version_999(self):
        assert _normalize_version("9.9.9") == "9.9.9"


class TestIsDevBackend:
    def test_both_none(self):
        assert _is_dev_backend(None, None) is False

    def test_version_contains_dev(self):
        assert _is_dev_backend("0.0.0-dev", None) is True

    def test_release_tag_contains_dev(self):
        assert _is_dev_backend("1.0.0", "v0.0.0-dev") is True

    def test_dev_case_insensitive(self):
        assert _is_dev_backend("1.0.0-DEV", None) is True

    def test_neither_contains_dev(self):
        assert _is_dev_backend("1.2.3", "v1.2.3") is False

    def test_empty_strings(self):
        assert _is_dev_backend("", "") is False


class TestIsDefaultDevPlugin:
    def test_999_returns_true(self):
        assert _is_default_dev_plugin("9.9.9") is True

    def test_other_version_returns_false(self):
        assert _is_default_dev_plugin("1.0.0") is False

    def test_none_returns_false(self):
        assert _is_default_dev_plugin(None) is False

    def test_999_with_extra_chars_returns_false(self):
        assert _is_default_dev_plugin("9.9.9-rc1") is False


class TestCheckPluginBackendVersion:
    def test_missing_plugin_version_incompatible(self, mocker):
        mocker.patch("services.version.BACKEND_VERSION", "1.0.0")
        mocker.patch("services.version.BACKEND_RELEASE_TAG", "v1.0.0")
        mocker.patch("services.version.BACKEND_BUILD", 42)

        result = check_plugin_backend_version(None)

        assert result["compatible"] is False
        assert "missing or invalid" in result["reason"]
        assert result["plugin_version"] is None

    def test_invalid_plugin_version_incompatible(self, mocker):
        mocker.patch("services.version.BACKEND_VERSION", "1.0.0")
        mocker.patch("services.version.BACKEND_RELEASE_TAG", "v1.0.0")
        mocker.patch("services.version.BACKEND_BUILD", 0)

        result = check_plugin_backend_version("not-a-version")

        assert result["compatible"] is False
        assert "missing or invalid" in result["reason"]

    def test_exact_version_match_compatible(self, mocker):
        mocker.patch("services.version.BACKEND_VERSION", "1.2.3")
        mocker.patch("services.version.BACKEND_RELEASE_TAG", "v1.2.3")
        mocker.patch("services.version.BACKEND_BUILD", 5)

        result = check_plugin_backend_version("1.2.3", plugin_build=5)

        assert result["compatible"] is True
        assert result["reason"] == "exact version match"

    def test_version_mismatch_incompatible(self, mocker):
        mocker.patch("services.version.BACKEND_VERSION", "1.2.3")
        mocker.patch("services.version.BACKEND_RELEASE_TAG", "v1.2.3")
        mocker.patch("services.version.BACKEND_BUILD", 5)

        result = check_plugin_backend_version("1.2.4")

        assert result["compatible"] is False
        assert "differ" in result["reason"]

    def test_dev_backend_accepts_999_plugin(self, mocker):
        mocker.patch("services.version.BACKEND_VERSION", "0.0.0-dev")
        mocker.patch("services.version.BACKEND_RELEASE_TAG", "v0.0.0-dev")
        mocker.patch("services.version.BACKEND_BUILD", 0)

        result = check_plugin_backend_version("9.9.9")

        assert result["compatible"] is True
        assert "dev fallback" in result["reason"]

    def test_dev_backend_rejects_non_999_plugin(self, mocker):
        mocker.patch("services.version.BACKEND_VERSION", "0.0.0-dev")
        mocker.patch("services.version.BACKEND_RELEASE_TAG", "v0.0.0-dev")
        mocker.patch("services.version.BACKEND_BUILD", 0)

        result = check_plugin_backend_version("1.0.0")

        # "0.0.0" (normalized from 0.0.0-dev) != "1.0.0"
        assert result["compatible"] is False

    def test_v_prefix_plugin_version_accepted(self, mocker):
        mocker.patch("services.version.BACKEND_VERSION", "2.0.0")
        mocker.patch("services.version.BACKEND_RELEASE_TAG", "v2.0.0")
        mocker.patch("services.version.BACKEND_BUILD", 1)

        result = check_plugin_backend_version("v2.0.0")

        assert result["compatible"] is True

    def test_response_contains_all_expected_keys(self, mocker):
        mocker.patch("services.version.BACKEND_VERSION", "1.0.0")
        mocker.patch("services.version.BACKEND_RELEASE_TAG", "v1.0.0")
        mocker.patch("services.version.BACKEND_BUILD", 3)

        result = check_plugin_backend_version(
            "1.0.0", plugin_build=3, plugin_release_tag="v1.0.0"
        )

        expected_keys = {
            "backend_version",
            "backend_release_tag",
            "backend_build",
            "plugin_version",
            "plugin_release_tag",
            "plugin_build",
            "compatible",
            "reason",
        }
        assert expected_keys.issubset(result.keys())

    def test_plugin_and_backend_info_echoed_back(self, mocker):
        mocker.patch("services.version.BACKEND_VERSION", "3.0.0")
        mocker.patch("services.version.BACKEND_RELEASE_TAG", "v3.0.0")
        mocker.patch("services.version.BACKEND_BUILD", 99)

        result = check_plugin_backend_version(
            "3.0.0", plugin_build=7, plugin_release_tag="v3.0.0"
        )

        assert result["backend_version"] == "3.0.0"
        assert result["backend_build"] == 99
        assert result["plugin_build"] == 7
        assert result["plugin_release_tag"] == "v3.0.0"
