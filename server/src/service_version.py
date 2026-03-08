import re

from version_info import BACKEND_BUILD, BACKEND_RELEASE_TAG, BACKEND_VERSION


def get_backend_version_info() -> dict:
    return {
        "backend_version": BACKEND_VERSION,
        "backend_release_tag": BACKEND_RELEASE_TAG,
        "backend_build": BACKEND_BUILD,
    }


def check_plugin_backend_version(plugin_version: str | None, plugin_build: int | None = None, plugin_release_tag: str | None = None) -> dict:
    backend = get_backend_version_info()
    normalized_backend = _normalize_version(backend["backend_version"])
    normalized_plugin = _normalize_version(plugin_version)

    if not normalized_plugin:
        return {
            **backend,
            "plugin_version": plugin_version,
            "plugin_release_tag": plugin_release_tag,
            "plugin_build": plugin_build,
            "compatible": False,
            "reason": "plugin_version is missing or invalid",
        }

    compatible = normalized_plugin == normalized_backend
    reason = "exact version match" if compatible else "plugin and backend version differ"

    return {
        **backend,
        "plugin_version": plugin_version,
        "plugin_release_tag": plugin_release_tag,
        "plugin_build": plugin_build,
        "compatible": compatible,
        "reason": reason,
    }


def _normalize_version(raw: str | None) -> str | None:
    if not raw or not isinstance(raw, str):
        return None
    candidate = raw.strip()
    if candidate.startswith("v"):
        candidate = candidate[1:]
    match = re.match(r"^(\d+)\.(\d+)\.(\d+)", candidate)
    if not match:
        return None
    return f"{match.group(1)}.{match.group(2)}.{match.group(3)}"
